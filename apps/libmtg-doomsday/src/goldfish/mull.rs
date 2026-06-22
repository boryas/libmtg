//! Goldfish mulligan modes.
//!
//! Three pluggable opening-hand policies for the cast-Doomsday-ASAP pilot, so the
//! goldfish can show a deck under different mulligan disciplines (the headline
//! P(cast by cutoff) shifts a lot with how hard you mulligan):
//!
//! - [`MullMode::Keep7`] — never mulligan. Pure raw deck speed, no mulligan dynamics.
//! - [`MullMode::Realistic`] — a *player's-thought-process* heuristic: keep a hand
//!   with real mana and an actual plan to cast Doomsday; mulligan no-mana / flooded /
//!   do-nothing hands. Built and validated card-by-card against human judgement (see
//!   the `sample_hands` example). It deliberately does NOT consult `p_cast_by` — every
//!   keep/mull names an inspectable reason.
//! - [`MullMode::Aggressive`] — the fanatical race bar: keep only a guaranteed-or-nearly
//!   guaranteed fast Doomsday — a DETERMINISTIC line by the cutoff, or Doomsday + BB +
//!   a castable cantrip to dig the last black. No `p_cast_by`; speculative cantrip hands
//!   are thrown back, so it rewards decks with more fast enablers (rituals/petals/tutors).
//!
//! The `Realistic` rule is the load-bearing one. Its logic (the "G1 / blind" checklist):
//!
//! ```text
//! KEEP if any fires (the solver does the deterministic checks; heuristics do the rest):
//!   - at 4 cards (mulligans_taken >= 3);
//!   - a DETERMINISTIC Doomsday line by the cutoff   (this is "DD + a way to reach BBB");
//!   - Tamiyo plan:  Tamiyo + a blue land to cast it + (a look or a fetch to fuel);
//!   - combo-in-progress:  Doomsday in hand + a black source + a castable look;
//!   - dig:  two castable looks.
//! else MULL — i.e. no colored land, flooded (>= 4 lands), or just sources + filler.
//! ```
//!
//! A "look" is a card-selection effect you can actually pay for: a cast-to-dig cantrip
//! (Ponder/Brainstorm/Consider — needs blue) or a free cycling cantrip (Street Wraith —
//! always castable). Sources are counted the way the *game* sees them: only
//! unconditional colored lands are a mana base (Cavern of Souls, colored only for
//! creatures, and one-shots like Lotus Petal are not).

use libmtg_engine::{Color, PlayerId, SimState, SourceZone, ActivationTiming, CardDef};

use super::recipe::{self, CardRole};

/// Opening-hand discipline for the goldfish pilot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MullMode {
    /// Never mulligan — keep the opening 7.
    Keep7,
    /// Player-heuristic "keep a real plan" rule (the default).
    Realistic,
    /// Keep only a deterministic Doomsday line by the cutoff (the fanatical race bar).
    Aggressive,
}

impl Default for MullMode {
    fn default() -> Self { MullMode::Realistic }
}

impl MullMode {
    /// Parse a web/CLI token (`keep7` / `realistic` / `aggressive`); unknown → default.
    pub fn from_str_or_default(s: &str) -> MullMode {
        match s {
            "keep7" => MullMode::Keep7,
            "aggressive" => MullMode::Aggressive,
            _ => MullMode::Realistic,
        }
    }
}

/// Cards that contribute to no realistic plan in a Doomsday hand — they're read as if
/// they weren't in the hand (a junk-heavy hand thus has too few real cards to form a
/// plan, and mulligans). A curated list because they're context-free dead weight here:
/// the alt win-con and tutor-target (Thassa's Oracle), reanimation with no target
/// (Unearth), the hand-discard mana rock (LED), a marginal cantrip (Edge of Autumn), the
/// secondary win-con (Jace), and the utility lands (Wasteland, Cavern of Souls). Extend
/// as imported lists surface more. (Excess *real* lands are handled separately, as flood.)
const AIR: &[&str] = &[
    "Thassa's Oracle", "Unearth", "Lion's Eye Diamond", "Edge of Autumn",
    "Jace, Wielder of Mysteries", "Wasteland", "Cavern of Souls",
];

fn is_air(name: &str) -> bool { AIR.contains(&name) }

/// Explicit, inspectable signals over the opening hand — plain card counts plus one
/// trustworthy deterministic-solver fact. No `p_cast_by`. Named-air cards (see [`AIR`])
/// are excluded from every count, as if they weren't drawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct HandSignals {
    /// Lands (a fetch counts) that tap blue UNCONDITIONALLY.
    pub blue_lands: u32,
    /// Lands (a fetch counts) that tap black UNCONDITIONALLY.
    pub black_lands: u32,
    /// Distinct lands tapping U or B (a dual counts once) — the real mana base.
    pub colored_lands: u32,
    /// All lands incl. colorless (Wasteland) — for the flood check.
    pub lands: u32,
    /// One-shot any-colour mana (Lotus Petal): not a land, not a base.
    pub petals: u32,
    /// Dark Ritual: needs a black seed, so not a base source either.
    pub rituals: u32,
    /// "Looks" you can actually pay for: cast-to-dig cantrips that have blue available,
    /// plus free cycling cantrips (Street Wraith) which are always castable.
    pub castable_looks: u32,
    /// Doomsday (or a tutor that finds it) in hand.
    pub has_dd: bool,
    /// Tamiyo, Inquisitive Student in hand.
    pub has_tamiyo: bool,
    /// A fetch in hand (fixes colours / fuels the Tamiyo flip).
    pub fetch: bool,
    /// Real non-mana, non-dig, non-payoff cards — interaction (Force/Daze/Thoughtseize)
    /// and threat bodies (Tamiyo/Murktide/Bowmasters). The "support" that carries a
    /// thin-dig hand over the top.
    pub supporters: u32,
    /// The solver finds a guaranteed Doomsday line by the cutoff.
    pub det_line: bool,
    /// The hand can deterministically flip Tamiyo by turn 2 (the fast-Tamiyo plan).
    pub tami_fast_flip: bool,
}

impl HandSignals {
    /// A STABLE blue source — a blue land. A lone Petal does NOT count: casting a cantrip
    /// by burning your only one-shot isn't a reliable dig (the difference between a Swamp
    /// hand and an Underground Sea hand).
    fn stable_blue(&self) -> bool { self.blue_lands >= 1 }
    /// A real mana base: at least one colored LAND (one-shots don't count).
    fn mana_base(&self) -> bool { self.colored_lands >= 1 }
    /// A real black source toward BBB (land or Petal — a bare Ritual needs a seed).
    fn black_src(&self) -> bool { self.black_lands >= 1 || self.petals >= 1 }
    /// Flooded — too many real (non-air) colored lands; excess lands are dead weight.
    fn flooded(&self) -> bool { self.colored_lands >= 4 }
}

/// Does `def` tap for `color` from the battlefield UNCONDITIONALLY (default timing,
/// no condition) — mirrors the solver's source rule. Lotus Petal (any colour) and
/// duals count; Cavern of Souls (colored only for creature spells) does not.
fn taps_for(def: &CardDef, color: Color) -> bool {
    def.mana_abilities().iter().any(|ma| {
        matches!(ma.source_zone, SourceZone::Battlefield)
            && ma.timing == ActivationTiming::Default
            && ma.condition.is_none()
            && ma.produces.contains(&color)
    })
}

/// A castable card-selection "look": a cast-to-dig cantrip (Ponder/Brainstorm/Consider)
/// only when there's a STABLE blue source to cast it off (a blue land, not a one-shot
/// Petal), or a free cycling cantrip (Street Wraith) regardless of mana.
fn is_castable_look(def: &CardDef, stable_blue: bool) -> bool {
    (def.digs_on_resolve() && stable_blue) || def.has_cycling_draw()
}

/// Compute the [`HandSignals`] for `who`'s current hand at the given cutoff. Named-air
/// cards (see [`AIR`]) are skipped in every count, as if they weren't drawn.
pub fn hand_signals(state: &SimState, who: PlayerId, cutoff: u32) -> HandSignals {
    let mut s = HandSignals {
        det_line: recipe::deterministic_cast_turn(state, who, cutoff).is_some(),
        // Fast Tamiyo plan: a deterministic flip by turn 2 (see `recipe::tamiyo_flip_turn`).
        tami_fast_flip: recipe::tamiyo_flip_turn(state, who, 2).is_some(),
        ..Default::default()
    };
    // Pass 1: mana base (so `can_blue` is known before classifying looks). Skip air.
    for card in state.hand_of(who) {
        if is_air(&card.catalog_key) { continue; }
        let role = recipe::card_role(state, who, card.id);
        let Some(def) = state.catalog.get(&card.catalog_key) else { continue };
        if def.is_land() {
            s.lands += 1;
            let is_fetch = matches!(role, CardRole::Fetch);
            let u = is_fetch || taps_for(def, Color::Blue);
            let b = is_fetch || taps_for(def, Color::Black);
            if u { s.blue_lands += 1; }
            if b { s.black_lands += 1; }
            if u || b { s.colored_lands += 1; }
        }
        match role {
            CardRole::Payoff | CardRole::PayoffTutor => s.has_dd = true,
            CardRole::Petal => s.petals += 1,
            CardRole::Ritual => s.rituals += 1,
            CardRole::Fetch => s.fetch = true,
            _ => {}
        }
        if card.catalog_key == "Tamiyo, Inquisitive Student" { s.has_tamiyo = true; }
        // A "supporter" = a real card that isn't mana, a dig, or the payoff: i.e.
        // interaction (Force/Daze/Thoughtseize) or a threat body (Tamiyo/Murktide/…).
        let is_dig = def.digs_on_resolve() || def.has_cycling_draw();
        let is_mana = def.is_land() || matches!(role, CardRole::Ritual | CardRole::Petal);
        let is_payoff = matches!(role, CardRole::Payoff | CardRole::PayoffTutor);
        if !is_dig && !is_mana && !is_payoff { s.supporters += 1; }
    }
    // Pass 2: castable looks (needs the mana base from pass 1). Skip air. A blue cantrip
    // counts only off a STABLE blue source (a blue land), not a one-shot Petal.
    let stable_blue = s.stable_blue();
    for card in state.hand_of(who) {
        if is_air(&card.catalog_key) { continue; }
        if let Some(def) = state.catalog.get(&card.catalog_key) {
            if is_castable_look(def, stable_blue) { s.castable_looks += 1; }
        }
    }
    s
}

/// The "Realistic" keep decision for an opening hand — keep iff the (air-filtered) hand
/// has a viable plan. Returns true to KEEP. Mulligan-depth handling (always keep at 4)
/// is applied by [`should_mulligan`]. The plans:
/// - **fast dd**: a guaranteed line, or Doomsday + a black source + a dig for the mana;
/// - **tami flip**: a deterministic Tamiyo flip by turn 2;
/// - **dig**: a black source + either two castable looks, or one look backed by a real
///   supporter on a stable (≥2-land) base — both only if not flooded.
pub fn realistic_keep(s: &HandSignals) -> bool {
    // Strong combo plans — keep regardless of air.
    if s.det_line { return true; }
    if !s.mana_base() { return false; } // no real colored land → nothing to cast off
    if s.has_dd && s.black_src() && s.castable_looks >= 1 { return true; } // fast dd (have payoff)
    if s.tami_fast_flip { return true; } // tami flip
    // Dig plans — need a black source and a non-flooded mana base.
    if s.black_src() && !s.flooded() {
        if s.castable_looks >= 2 { return true; } // enough dig to find the combo
        // one look is enough with real backup (interaction / a threat body); a thin dig
        // with no support (just mana) isn't a plan.
        if s.castable_looks >= 1 && s.supporters >= 1 { return true; }
    }
    false
}

/// Aggressive: the fanatical race bar. Keep only a hand that is a guaranteed-or-nearly
/// guaranteed fast Doomsday, mulliganing anything speculative. Two ways in:
///   1. a DETERMINISTIC line by the cutoff (the solver guarantees it); or
///   2. near-deterministic: Doomsday in hand + **BB** (two black units — lands/petals)
///      + a castable cantrip to dig the third black / the finisher.
/// No `p_cast_by`. (`KEEP7` env retained for the apples-to-apples gameplay experiment.)
pub fn aggressive_keep(state: &SimState, who: PlayerId, cutoff: u32) -> bool {
    if std::env::var("KEEP7").is_ok() { return true; }
    let s = hand_signals(state, who, cutoff);
    if s.det_line { return true; }
    s.has_dd && (s.black_lands + s.petals) >= 2 && s.castable_looks >= 1
}

/// Whether to MULLIGAN this hand under `mode`. Always keep at 4 cards
/// (`mulligans_taken >= 3`); otherwise dispatch to the mode's keep rule.
pub fn should_mulligan(
    mode: MullMode,
    state: &SimState,
    who: PlayerId,
    cutoff: u32,
    mulligans_taken: u32,
) -> bool {
    if mulligans_taken >= 3 { return false; } // always keep the 4-card hand
    let keep = match mode {
        MullMode::Keep7 => true,
        MullMode::Realistic => realistic_keep(&hand_signals(state, who, cutoff)),
        MullMode::Aggressive => aggressive_keep(state, who, cutoff),
    };
    !keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use libmtg_engine::{build_catalog, PlayerState, SimState, Zone};

    // The sample tempo DD list — fills the library so fetch targets / deterministic
    // lines resolve. (Same list the `sample_hands` example deals from.)
    const TEMPO: &[(&str, i32)] = &[
        ("Underground Sea", 3), ("Polluted Delta", 4), ("Flooded Strand", 1),
        ("Misty Rainforest", 1), ("Scalding Tarn", 1), ("Marsh Flats", 1),
        ("Island", 1), ("Swamp", 1), ("Undercity Sewers", 2), ("Wasteland", 3),
        ("Cavern of Souls", 1), ("Lotus Petal", 2), ("Lion's Eye Diamond", 1),
        ("Dark Ritual", 4), ("Doomsday", 4), ("Brainstorm", 4), ("Ponder", 4),
        ("Consider", 1), ("Edge of Autumn", 1), ("Force of Will", 4), ("Daze", 3),
        ("Thoughtseize", 2), ("Street Wraith", 1), ("Thassa's Oracle", 1),
        ("Unearth", 1), ("Tamiyo, Inquisitive Student", 4), ("Orcish Bowmasters", 2),
        ("Murktide Regent", 2),
    ];

    /// Build an opening-hand state (hand in hand, the rest of the tempo deck in
    /// library) and return the `Realistic` keep decision.
    fn realistic_keeps(hand: &[&str]) -> bool {
        let catalog = build_catalog();
        let mut pool: Vec<String> = Vec::new();
        for (n, q) in TEMPO { for _ in 0..*q { pool.push((*n).to_string()); } }
        for h in hand {
            if let Some(i) = pool.iter().position(|c| c == h) { pool.remove(i); }
        }
        let mut s = SimState::new(PlayerState::new("us"), PlayerState::new("opp"));
        s.catalog = catalog;
        for h in hand { s.place_card(PlayerId::Us, h, Zone::Hand { known: false }); }
        for n in &pool { s.place_card(PlayerId::Us, n, Zone::Library); }
        realistic_keep(&hand_signals(&s, PlayerId::Us, 4))
    }

    /// Every opening hand we hand-judged during design, with the human "blind" verdict.
    /// The Realistic rule must reproduce all of them. (true = KEEP)
    #[test]
    fn realistic_matches_human_blind_calls() {
        let cases: &[(bool, &[&str])] = &[
            // ── keeps ──
            (true,  &["Tamiyo, Inquisitive Student", "Doomsday", "Scalding Tarn", "Daze", "Ponder", "Murktide Regent", "Polluted Delta"]), // A
            (true,  &["Force of Will", "Misty Rainforest", "Brainstorm", "Doomsday", "Ponder", "Undercity Sewers", "Dark Ritual"]), // E (det line)
            (true,  &["Brainstorm", "Doomsday", "Misty Rainforest", "Orcish Bowmasters", "Polluted Delta", "Tamiyo, Inquisitive Student", "Murktide Regent"]), // F
            (true,  &["Misty Rainforest", "Dark Ritual", "Dark Ritual", "Doomsday", "Daze", "Tamiyo, Inquisitive Student", "Orcish Bowmasters"]), // G (det line)
            (true,  &["Tamiyo, Inquisitive Student", "Lion's Eye Diamond", "Ponder", "Flooded Strand", "Doomsday", "Street Wraith", "Force of Will"]), // I
            (true,  &["Dark Ritual", "Undercity Sewers", "Scalding Tarn", "Dark Ritual", "Ponder", "Swamp", "Tamiyo, Inquisitive Student"]), // J
            (true,  &["Tamiyo, Inquisitive Student", "Ponder", "Scalding Tarn", "Force of Will", "Polluted Delta", "Murktide Regent", "Orcish Bowmasters"]), // K
            (true,  &["Brainstorm", "Scalding Tarn", "Edge of Autumn", "Polluted Delta", "Ponder", "Cavern of Souls", "Thoughtseize"]), // L
            (true,  &["Ponder", "Underground Sea", "Brainstorm", "Edge of Autumn", "Unearth", "Brainstorm", "Polluted Delta"]), // M
            (true,  &["Swamp", "Thoughtseize", "Unearth", "Misty Rainforest", "Consider", "Brainstorm", "Wasteland"]), // N
            (true,  &["Daze", "Consider", "Tamiyo, Inquisitive Student", "Underground Sea", "Ponder", "Dark Ritual", "Ponder"]), // O
            (true,  &["Marsh Flats", "Street Wraith", "Wasteland", "Daze", "Edge of Autumn", "Wasteland", "Ponder"]), // P
            // one land but a dig + a wall of interaction/threats (low air) — a keep.
            (true,  &["Underground Sea", "Thoughtseize", "Force of Will", "Murktide Regent", "Murktide Regent", "Ponder", "Force of Will"]), // batch #12
            // H but the land is a blue Sea, so Consider IS castable off stable mana — a
            // terrible-but-lowest-end keep (dig + support).
            (true,  &["Consider", "Underground Sea", "Force of Will", "Orcish Bowmasters", "Edge of Autumn", "Lotus Petal", "Lotus Petal"]), // H-with-Sea
            // ── mulls ──
            (false, &["Lotus Petal", "Ponder", "Murktide Regent", "Tamiyo, Inquisitive Student", "Wasteland", "Dark Ritual", "Dark Ritual"]), // B (no colored land)
            (false, &["Tamiyo, Inquisitive Student", "Force of Will", "Daze", "Ponder", "Edge of Autumn", "Brainstorm", "Thoughtseize"]), // C (no land)
            (false, &["Cavern of Souls", "Force of Will", "Murktide Regent", "Ponder", "Wasteland", "Swamp", "Orcish Bowmasters"]), // D (Ponder uncastable, no blue)
            // H: its only land is a black Swamp, so Consider is castable only by burning a
            // one-shot Petal — not a real dig. No stable look ⇒ no plan ⇒ mull.
            (false, &["Consider", "Swamp", "Force of Will", "Orcish Bowmasters", "Edge of Autumn", "Lotus Petal", "Lotus Petal"]), // H
            // round 4: two Ponders but NO black source — digs toward an uncastable DD.
            (false, &["Thoughtseize", "Ponder", "Daze", "Unearth", "Island", "Ponder", "Wasteland"]), // r4-#12
            // round 4: explosive mana but only one look and no DD/Tamiyo — speculative.
            (false, &["Scalding Tarn", "Ponder", "Lotus Petal", "Wasteland", "Misty Rainforest", "Lotus Petal", "Dark Ritual"]), // r4-#13
        ];
        let mut wrong = Vec::new();
        for (i, (want, hand)) in cases.iter().enumerate() {
            let got = realistic_keeps(hand);
            if got != *want {
                wrong.push(format!(
                    "  case {i}: want {} got {} — [{}]",
                    if *want { "KEEP" } else { "MULL" },
                    if got { "KEEP" } else { "MULL" },
                    hand.join(", ")));
            }
        }
        assert!(wrong.is_empty(), "Realistic rule disagreed with human calls:\n{}", wrong.join("\n"));
    }

    #[test]
    fn keep7_never_mulligans_and_floor_holds() {
        // Keep7 always keeps; the 4-card hand is always kept in every mode.
        let catalog = build_catalog();
        let mut s = SimState::new(PlayerState::new("us"), PlayerState::new("opp"));
        s.catalog = catalog;
        for n in ["Island", "Wasteland", "Force of Will"] { // a junk 3-ish hand
            s.place_card(PlayerId::Us, n, Zone::Hand { known: false });
        }
        assert!(!should_mulligan(MullMode::Keep7, &s, PlayerId::Us, 4, 0));
        assert!(!should_mulligan(MullMode::Realistic, &s, PlayerId::Us, 4, 3)); // 4-card: keep
    }
}
