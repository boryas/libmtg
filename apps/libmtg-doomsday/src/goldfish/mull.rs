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
//! - [`MullMode::Aggressive`] — the original race bar: a high threshold on the solver's
//!   `p_cast_by`, loosening as you mulligan. Optimal-ish but fanatical; it ships almost
//!   every imperfect 7 and so flattens deck-speed differences.
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
    /// High `p_cast_by` bar, loosening as you mulligan (the original behaviour).
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

/// Explicit, inspectable signals over the opening hand — plain card counts plus one
/// trustworthy deterministic-solver fact. No `p_cast_by`.
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
    /// The solver finds a guaranteed Doomsday line by the cutoff.
    pub det_line: bool,
}

impl HandSignals {
    /// Can the hand make blue at all (to cast a cantrip / Tamiyo's pip)? A lone Petal
    /// counts (for one blue spell).
    fn can_blue(&self) -> bool { self.blue_lands >= 1 || self.petals >= 1 }
    /// A real mana base: at least one colored LAND (one-shots don't count).
    fn mana_base(&self) -> bool { self.colored_lands >= 1 }
    /// A real black source toward BBB (land or Petal — a bare Ritual needs a seed).
    fn black_src(&self) -> bool { self.black_lands >= 1 || self.petals >= 1 }
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
/// when blue is available, or a free cycling cantrip (Street Wraith) regardless.
fn is_castable_look(def: &CardDef, can_blue: bool) -> bool {
    (def.digs_on_resolve() && can_blue) || def.has_cycling_draw()
}

/// Compute the [`HandSignals`] for `who`'s current hand at the given cutoff.
pub fn hand_signals(state: &SimState, who: PlayerId, cutoff: u32) -> HandSignals {
    let mut s = HandSignals {
        det_line: recipe::deterministic_cast_turn(state, who, cutoff).is_some(),
        ..Default::default()
    };
    // Pass 1: mana base (so `can_blue` is known before classifying looks).
    for card in state.hand_of(who) {
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
    }
    // Pass 2: castable looks (needs the mana base from pass 1).
    let can_blue = s.can_blue();
    for card in state.hand_of(who) {
        if let Some(def) = state.catalog.get(&card.catalog_key) {
            if is_castable_look(def, can_blue) { s.castable_looks += 1; }
        }
    }
    s
}

/// The "Realistic" keep decision for an opening hand (the G1 checklist). Returns true
/// to KEEP. Mulligan-depth handling (always keep at 4) is applied by [`should_mulligan`].
pub fn realistic_keep(s: &HandSignals) -> bool {
    // A guaranteed line (incl. DD + reachable BBB) is a snap keep, over everything.
    if s.det_line { return true; }
    // Bad shapes: no real mana, or flooded.
    if !s.mana_base() { return false; }
    if s.lands >= 4 { return false; }
    // A real plan to assemble Doomsday:
    // - Doomsday in hand + a black source + a look to find the rest of the mana;
    if s.has_dd && s.black_src() && s.castable_looks >= 1 { return true; }
    // - a Tamiyo plan: deployable (blue land) + something to fuel the flip;
    if s.has_tamiyo && s.blue_lands >= 1 && (s.castable_looks >= 1 || s.fetch) { return true; }
    // - enough digging to find the combo AND a black source to actually cast it. Two
    //   cantrips with no black mana just dig toward a Doomsday you can't pay for.
    if s.castable_looks >= 2 && s.black_src() { return true; }
    false
}

/// The original Aggressive bar: a high threshold on the solver's `p_cast_by`, loosening
/// as you mulligan. `KEEP7` env retained for the apples-to-apples gameplay experiment.
pub fn aggressive_keep(state: &SimState, who: PlayerId, cutoff: u32, mulligans_taken: u32) -> bool {
    if std::env::var("KEEP7").is_ok() { return true; }
    let p = recipe::p_cast_by(state, who, cutoff);
    let threshold = match mulligans_taken {
        0 => 0.55,
        1 => 0.38,
        _ => 0.20,
    };
    p >= threshold
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
        MullMode::Aggressive => aggressive_keep(state, who, cutoff, mulligans_taken),
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
            // ── mulls ──
            (false, &["Lotus Petal", "Ponder", "Murktide Regent", "Tamiyo, Inquisitive Student", "Wasteland", "Dark Ritual", "Dark Ritual"]), // B (no colored land)
            (false, &["Tamiyo, Inquisitive Student", "Force of Will", "Daze", "Ponder", "Edge of Autumn", "Brainstorm", "Thoughtseize"]), // C (no land)
            (false, &["Cavern of Souls", "Force of Will", "Murktide Regent", "Ponder", "Wasteland", "Swamp", "Orcish Bowmasters"]), // D (Ponder uncastable, no blue)
            (false, &["Consider", "Swamp", "Force of Will", "Orcish Bowmasters", "Edge of Autumn", "Lotus Petal", "Lotus Petal"]), // H (1 look, no DD/Tamiyo)
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
