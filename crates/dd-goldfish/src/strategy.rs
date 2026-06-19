//! `DDGoldfishStrategy` — cast Doomsday as fast as possible, before a cutoff turn.
//!
//! This is NOT a reuse of the baseline `DoomsdayStrategy`. Every *decision* is
//! driven by the backward resource solver in [`crate::recipe`]:
//!
//! - **When to go off:** cast Doomsday the instant a complete recipe is
//!   assemblable this turn (`recipe::sufficient`); then mechanically assemble the
//!   mana (land / petal / fetch / ritual) and cast it.
//! - **What to do otherwise:** develop a black source and dig with cantrips to
//!   raise `P(cast Doomsday by cutoff)`.
//! - **How to resolve each cantrip / tutor / fetch / scry:** the solver's
//!   recommendations — `recipe::best_top_choice` for Ponder, and the
//!   `recipe::dd_card_value` valuation for fetch/tutor targets, Flow State's dig,
//!   Brainstorm's put-back, scry, and surveil.
//! - **Whether to keep the opening hand:** a threshold on `recipe::p_cast_by`.
//!
//! The solver is the brain; this strategy is only the hands — it translates those
//! verdicts into engine `LegalAction`s. The mechanical "how to emit an action"
//! shape is cribbed from `DoomsdayStrategy`; none of its heuristics are.

use std::cmp::Ordering;

use mtg_engine::*;

use crate::recipe::{self, CardRole};

const DOOMSDAY: &str = "Doomsday";

/// The cutoff turn past which a Doomsday is assumed too slow (Wastelanded / raced).
/// The strategy plays to maximise `P(cast Doomsday by cutoff)`.
pub const DEFAULT_CUTOFF: u32 = 4;

/// Horizon for the "execute a slower line anyway" fallback when no line lands by the
/// cutoff and there's nothing to dig — so we still cast eventually rather than pass.
const FALLBACK_HORIZON: u32 = 10;

pub struct DDGoldfishStrategy {
    player_id: PlayerId,
    /// "Cast by this turn" objective (1-based). Drives mulligans + cantrip choices.
    cutoff: u32,
    /// Set by `order_top_library` (Ponder) when the solver wants to shuffle; consumed
    /// by the immediately-following `MayDo`(shuffle) `resolve_choice`.
    pending_shuffle: bool,
    /// A/B debug mode: also evaluate the reference value-table heuristic at each
    /// selection decision and log (`DIFF …`) wherever it disagrees with the
    /// principled policy. Off in normal play.
    compare: bool,
    decisions: Vec<String>,
}

impl DDGoldfishStrategy {
    pub fn new(cutoff: u32) -> Self {
        Self {
            player_id: PlayerId::Us,
            cutoff: cutoff.max(1),
            pending_shuffle: false,
            compare: false,
            decisions: Vec::new(),
        }
    }

    /// Like [`new`], but logs `DIFF …` at every selection decision where the
    /// reference value-table heuristic would choose differently (A/B debugging).
    pub fn new_comparing(cutoff: u32) -> Self {
        let mut s = Self::new(cutoff);
        s.compare = true;
        s
    }

    fn dlog(&mut self, msg: impl Into<String>) {
        self.decisions.push(msg.into());
    }

    /// Card name for logging.
    fn nm(state: &SimState, id: ObjId) -> String {
        state.objects.get(&id).map(|o| o.catalog_key.clone()).unwrap_or_else(|| "?".to_string())
    }

    /// Whether we actually *hold* the payoff (Doomsday) in hand. A tutor that can
    /// fetch Doomsday does NOT count — it must still be cast and resolved before we
    /// hold the payoff. This is what gates casting the tutor: `payoff_in_hand` is
    /// true when a tutor is in hand, so using it here would (wrongly) conclude the
    /// payoff is already secured and assemble mana for a Doomsday we never acquire.
    fn holds_payoff(&self, state: &SimState, who: PlayerId) -> bool {
        state
            .hand_of(who)
            .any(|c| recipe::card_role(state, who, c.id) == CardRole::Payoff)
    }

    /// A complete recipe is assemblable this turn — emit the next mana-assembly
    /// step toward casting Doomsday. We try the steps that don't spend a card first
    /// (land drop, then deploy a petal, then crack a fetch) and only cast a ritual
    /// if still short; `choose_action` casts Doomsday itself as soon as it becomes
    /// castable, so a ritual is only ever cast when genuinely needed.
    fn assemble_step(&self, state: &SimState, who: PlayerId, legal: &[LegalAction]) -> Option<LegalAction> {
        use CardRole::*;
        // 0. If the deterministic line runs through a tutor (Personal Tutor) and we
        //    don't yet hold Doomsday, cast the tutor to put it on top — otherwise we
        //    assemble BBB for a payoff we never acquire. Falls through to a land drop
        //    when no blue source is online yet, then casts the tutor on re-entry.
        if !self.holds_payoff(state, who) {
            if let Some(a) = first_cast(state, who, legal, &[PayoffTutor]) {
                return Some(a);
            }
        }
        // 1. Play a black source (untapped land / fetch / tapland).
        if let Some(a) = best_land_drop(state, who, legal, &[BlackLandUntapped, Fetch, BlackLandTapped]) {
            return Some(a);
        }
        // 2. Deploy a free mana artifact (Lotus Petal).
        if let Some(a) = first_cast(state, who, legal, &[Petal]) {
            return Some(a);
        }
        // 3. Crack a fetch already in play to get a black source online.
        if let Some(a) = fetch_activation(state, legal) {
            return Some(a);
        }
        // 4. Cast a ritual (Dark Ritual) to finish the mana.
        if let Some(a) = first_cast(state, who, legal, &[Ritual]) {
            return Some(a);
        }
        None
    }

    /// Not castable this turn — develop a black source and dig toward the cutoff.
    fn develop_and_dig(&self, state: &SimState, who: PlayerId, legal: &[LegalAction]) -> Option<LegalAction> {
        use CardRole::*;
        // a) Develop mana: play a land (untapped black first so it can cast a cantrip
        //    this turn, then fetch, tapland, any blue source).
        if let Some(a) = best_land_drop(state, who, legal,
            &[BlackLandUntapped, Fetch, BlackLandTapped, BlueSource]) {
            return Some(a);
        }
        // b) Crack a fetch in play to put a dual online (a usable U/B source).
        if let Some(a) = fetch_activation(state, legal) {
            return Some(a);
        }
        // c) If we don't hold Doomsday itself, cast a tutor for it (having the tutor
        //    in hand is not the same as holding the payoff — it must be cast).
        if !self.holds_payoff(state, who) {
            if let Some(a) = first_cast(state, who, legal, &[PayoffTutor]) {
                return Some(a);
            }
        }
        // d) Dig with a cantrip (Ponder / Brainstorm / Consider / Flow State / …).
        if let Some(a) = first_cast(state, who, legal, &[Cantrip]) {
            return Some(a);
        }
        None
    }

    /// P(cast by cutoff) if `id` were acquired now — staged on top (`to_top`, a tutor:
    /// drawn next turn) or drawn into hand this turn (a dig). Lets each search/dig pick
    /// the candidate that most advances the objective, with no value table.
    fn candidate_p(&self, state: &SimState, id: ObjId, to_top: bool) -> f64 {
        let who = self.player_id;
        let key = state.objects.get(&id).map(|o| o.catalog_key.clone()).unwrap_or_default();
        if to_top {
            recipe::p_cast_by_with_known_top(state, who, &[key.as_str()], false, self.cutoff)
        } else {
            recipe::p_cast_by_full(state, who, &[key.as_str()], &[], true, self.cutoff)
        }
    }

    /// E[TTD] if `id` were acquired now (staged on top / drawn to hand) — the same
    /// objective as `candidate_p` but in TURNS, which (unlike a probability) does NOT
    /// saturate. This is the tie-break that separates two by-cutoff lines: a fetched Sea
    /// pays a tutor's pip sooner than a Swamp, and staging Doomsday is a turn faster than
    /// staging a tutor for it (the chained-tutor +1 in `deterministic_cast_turn_full`).
    fn candidate_ttd(&self, state: &SimState, id: ObjId, to_top: bool) -> f64 {
        let who = self.player_id;
        let key = state.objects.get(&id).map(|o| o.catalog_key.clone()).unwrap_or_default();
        recipe::e_ttd_with_known_top(state, who, &[key.as_str()], !to_top, self.cutoff)
    }

    /// Whether `id` fills an outstanding requirement of the plan: the payoff (until one
    /// is secured), any black-mana source, or a cantrip while we still lack a
    /// deterministic line by the cutoff (so digging is still needed). Solver-derived
    /// (`card_role` / `payoff_in_hand` / `deterministic_cast_turn`) — no value table.
    /// Used for "which cards can I afford to lose" (Brainstorm put-back, mulligan
    /// bottoming); the visible-path requirement set will sharpen this later.
    fn card_needed(&self, state: &SimState, id: ObjId) -> bool {
        let who = self.player_id;
        match recipe::card_role(state, who, id) {
            // Keep the payoff itself — only a REDUNDANT copy is expendable. We keep the
            // FIRST payoff in hand (canonical) and treat any further copy as buriable.
            // (Bug guard: `payoff_in_hand` is true *because of this very card*, so we
            // must not read it as "already secured" and bury our own Doomsday.)
            CardRole::Payoff | CardRole::PayoffTutor => state
                .hand_of(who)
                .find(|c| matches!(recipe::card_role(state, who, c.id),
                                   CardRole::Payoff | CardRole::PayoffTutor))
                .map_or(true, |first| first.id == id),
            CardRole::Ritual
            | CardRole::Petal
            | CardRole::BlackLandUntapped
            | CardRole::Fetch
            | CardRole::BlackLandTapped
            | CardRole::BlueSource => true,
            CardRole::Cantrip => recipe::deterministic_cast_turn(state, who, self.cutoff.max(1))
                .map_or(true, |t| t > self.cutoff),
            CardRole::Other => false,
        }
    }
}

// ── Action-emission helpers ───────────────────────────────────────────────────
//
// Choosing WHICH legal action serves the solver's plan. The only judgement here is
// *land ordering* (which black source to play when several would serve) — a
// genuinely hard sub-problem the user has flagged as an allowed heuristic.
// Everything else is a structural role match: cards of the same role are
// interchangeable for the plan, and the solver decides *that* a role advances it.

/// Land-drop preference ordinal — the allowed land-ordering heuristic. An untapped
/// black source is best (mana now), then a fetch (mana now via a crack), then a
/// tapland (mana next turn), then a bare blue source.
fn land_priority(role: CardRole) -> u8 {
    match role {
        CardRole::BlackLandUntapped => 4,
        CardRole::Fetch => 3,
        CardRole::BlackLandTapped => 2,
        CardRole::BlueSource => 1,
        _ => 0,
    }
}

/// Last-ditch trinary keep-ordering — **cantrip > relevant > irrelevant** — used ONLY
/// when the principled rules (det-ttd / min-ttd / p_cast_by) don't separate the
/// options (sanctioned alongside land ordering). Cantrips rank highest because their
/// downstream digging is value the objective can't score (chained cantrips are
/// intractable); combo-relevant cards (mana / payoff / tutors) outrank dead bricks.
/// Higher = keep longer / bury later.
fn keep_rank(state: &SimState, who: PlayerId, id: ObjId) -> u8 {
    match recipe::card_role(state, who, id) {
        CardRole::Cantrip => 2,
        CardRole::Other => 0, // irrelevant brick
        _ => 1,               // relevant: payoff / tutor / ritual / petal / land / blue source
    }
}

/// Acquire-target dominance (the tutor analog of `land_priority` for fetches): the
/// payoff itself beats a tutor for it — staging Doomsday skips the tutor's extra cast.
/// Used only as the LAST tie-break, when both P(cast) and min-ttd are equal (tight
/// mana, where neither line is deterministic and the stochastic estimate can't see the
/// payoff-arrival delay). Without it the tie falls to the last candidate, which can be
/// a redundant tutor → tutor→tutor loop. Higher = acquire first.
fn payoff_rank(state: &SimState, who: PlayerId, id: ObjId) -> u8 {
    match recipe::card_role(state, who, id) {
        CardRole::Payoff => 2,
        CardRole::PayoffTutor => 1,
        _ => 0,
    }
}


/// The legal `LandDrop` whose role is in `want`, chosen by `land_priority`.
fn best_land_drop(state: &SimState, who: PlayerId, legal: &[LegalAction], want: &[CardRole]) -> Option<LegalAction> {
    legal.iter()
        .filter_map(|a| match a {
            LegalAction::LandDrop(id) => {
                let role = recipe::card_role(state, who, *id);
                want.contains(&role).then(|| (a.clone(), land_priority(role)))
            }
            _ => None,
        })
        .max_by_key(|(_, pri)| *pri)
        .map(|(a, _)| a)
}

/// The first legal `CastSpell` whose role is in `want`. No ranking — same-role
/// cards are interchangeable (any ritual makes black, any petal); the solver
/// already decided this role advances the plan.
fn first_cast(state: &SimState, who: PlayerId, legal: &[LegalAction], want: &[CardRole]) -> Option<LegalAction> {
    legal.iter()
        .find(|a| matches!(a, LegalAction::CastSpell { card_id, .. }
            if want.contains(&recipe::card_role(state, who, *card_id))))
        .cloned()
}

/// Every ordered sequence drawn from a subset of `items` (including the empty
/// sequence). For the tiny look-at-top sets (≤3 cards) the strategy enumerates
/// arrangements to score each via the objective.
fn ordered_subsets<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    let n = items.len();
    let mut out: Vec<Vec<T>> = Vec::new();
    for mask in 0u32..(1u32 << n) {
        let subset: Vec<T> = (0..n).filter(|i| mask & (1 << i) != 0).map(|i| items[i].clone()).collect();
        out.extend(permutations(&subset));
    }
    out
}

fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for i in 0..items.len() {
        let mut rest = items.to_vec();
        let head = rest.remove(i);
        for mut p in permutations(&rest) {
            p.insert(0, head.clone());
            out.push(p);
        }
    }
    out
}

/// A legal `CastSpell` for the named card (the payoff — Doomsday).
fn cast_named(state: &SimState, legal: &[LegalAction], name: &str) -> Option<LegalAction> {
    legal.iter()
        .find(|a| matches!(a, LegalAction::CastSpell { card_id, .. }
            if state.objects.get(card_id).map_or(false, |c| c.catalog_key == name)))
        .cloned()
}

/// Translate one emitted [`recipe::LineStep`] into the matching legal engine action
/// for *this* window, if it is available now. The strategy executes the first step
/// of the freshly-emitted line that is currently legal, so the engine's state
/// advance + re-emission walks the multi-turn line.
fn line_step_action(step: recipe::LineStep, legal: &[LegalAction]) -> Option<LegalAction> {
    use recipe::LineStep;
    let hits = |a: &LegalAction| match step {
        LineStep::PlayLand(id) => matches!(a, LegalAction::LandDrop(x) if *x == id),
        LineStep::CrackFetch(id) => {
            matches!(a, LegalAction::ActivateAbility { source_id, .. } if *source_id == id)
        }
        LineStep::CastPetal(id)
        | LineStep::CastRitual(id)
        | LineStep::CastTutor(id)
        | LineStep::CastDoomsday(id) => {
            matches!(a, LegalAction::CastSpell { card_id, .. } if *card_id == id)
        }
    };
    legal.iter().find(|a| hits(a)).cloned()
}

/// Follow the solved deterministic line: emit the first of its steps that is legal
/// this window. `None` only when the emitter can't build a line (exotic source it
/// doesn't model) — the caller logs + falls back so we can measure that gap.
fn follow_line(state: &SimState, who: PlayerId, legal: &[LegalAction], max_turn: u32)
    -> Option<LegalAction>
{
    let line = recipe::deterministic_line(state, who, max_turn)?;
    // A real line may have nothing to do *this* window (it matures next turn) — return
    // an explicit Pass so the caller doesn't fall back as if the emitter failed.
    Some(line.steps.iter().find_map(|&s| line_step_action(s, legal)).unwrap_or(LegalAction::Pass))
}

/// A legal fetch-land activation (sac → search), if any.
fn fetch_activation(state: &SimState, legal: &[LegalAction]) -> Option<LegalAction> {
    legal.iter()
        .find(|a| matches!(a, LegalAction::ActivateAbility { source_id, ability_index }
            if state.def_of(*source_id)
                .and_then(|d| d.abilities().get(*ability_index))
                .map_or(false, |ab| ab.is_fetch_ability())))
        .cloned()
}

// ── Reference value-table policy (A/B comparison ONLY — never drives play) ─────
// The OLD heuristic choices, used solely so compare mode can diff them against the
// principled (objective-driven) policy. See `recipe::dd_card_value`.

fn heur_pick(state: &SimState, who: PlayerId, choices: &[ObjId]) -> Option<ObjId> {
    choices.iter().copied().max_by(|&a, &b| {
        recipe::dd_card_value(state, who, a)
            .partial_cmp(&recipe::dd_card_value(state, who, b))
            .unwrap_or(Ordering::Equal)
    })
}

fn heur_surveil_bin(state: &SimState, who: PlayerId, id: ObjId) -> bool {
    recipe::dd_card_value(state, who, id) < 0.3
}

fn heur_scry_keep(state: &SimState, who: PlayerId, top: &[ObjId]) -> Vec<ObjId> {
    let mut keep: Vec<ObjId> = top.iter().copied()
        .filter(|&id| recipe::dd_card_value(state, who, id) >= 0.3)
        .collect();
    keep.sort_by(|&a, &b| recipe::dd_card_value(state, who, b)
        .partial_cmp(&recipe::dd_card_value(state, who, a)).unwrap_or(Ordering::Equal));
    keep
}

fn heur_bury(state: &SimState, who: PlayerId, count: usize, candidates: &[ObjId]) -> Vec<ObjId> {
    let mut scored: Vec<(ObjId, f64)> = candidates.iter()
        .map(|&id| (id, recipe::dd_card_value(state, who, id)))
        .collect();
    scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
    scored.into_iter().take(count).map(|(id, _)| id).collect()
}

fn names(state: &SimState, ids: &[ObjId]) -> String {
    ids.iter().map(|&i| DDGoldfishStrategy::nm(state, i)).collect::<Vec<_>>().join(",")
}

impl Strategy for DDGoldfishStrategy {
    fn player_id(&self) -> PlayerId { self.player_id }

    fn drain_decisions(&mut self) -> Vec<String> { std::mem::take(&mut self.decisions) }

    fn declare_attackers(&mut self, _state: &SimState) -> Vec<(ObjId, Option<ObjId>)> { Vec::new() }
    fn declare_blockers(&mut self, _state: &SimState) -> Vec<(ObjId, ObjId)> { Vec::new() }

    fn choose_action(&mut self, state: &SimState, ap: PlayerId,
                     legal: &[LegalAction]) -> LegalAction {
        let who = self.player_id;
        // Goldfish: we act only on our own main phase with an empty stack. We never
        // interact on the opponent's turn or respond to the stack — we just pass to
        // let our own spells (rituals, cantrips) resolve, then keep assembling.
        if ap != who { return LegalAction::Pass; }
        let in_main = matches!(state.current_phase,
            Some(TurnPosition::Phase(PhaseKind::PreCombatMain))
            | Some(TurnPosition::Phase(PhaseKind::PostCombatMain)));
        if !in_main || !state.stack.is_empty() { return LegalAction::Pass; }

        // 1) Cast Doomsday the instant it is castable.
        if let Some(a) = cast_named(state, legal, DOOMSDAY) {
            self.dlog(format!("T{}: cast Doomsday", state.current_turn));
            return a;
        }
        // 2) SETTLE vs GAMBLE — keyed on the solver's det-ttd / min-ttd (no thresholds).
        let det = recipe::deterministic_cast_turn(state, who, self.cutoff.max(1));
        let mn = recipe::min_ttd(state, who, self.cutoff.max(1));
        if det.is_some() {
            // A GUARANTEED line lands by the cutoff → SETTLE: execute it, and ONLY it.
            // We do NOT dig here — digging could spend a resource the line needs (e.g.
            // sac a petal to cantrip, then whiff and fail to cast). If there's nothing
            // to assemble this window, pass and let the line mature next turn.
            self.dlog(format!("T{}: settle (det-ttd={:?}, min-ttd={:?})", state.current_turn, det, mn));
            // FOLLOW the solved line — execute its next legal step, nothing else.
            if let Some(a) = follow_line(state, who, legal, self.cutoff.max(1)) {
                return a;
            }
            // Emitter gap (an unmodelled source): log it so we can measure how often
            // the follow-the-line path is incomplete, then fall back.
            self.dlog(format!("T{}: settle FALLBACK — line emitter gap", state.current_turn));
            return self.assemble_step(state, who, legal).unwrap_or(LegalAction::Pass);
        }
        // 3) No guaranteed line by the cutoff → GAMBLE. Per the policy: if even the
        //    optimistic min-ttd is > cutoff (no possible win with the current cards),
        //    FILTER to actions that can pull min-ttd back under — first rip a fetch (it
        //    shuffles AND fetches a black source). There's no by-cutoff line to
        //    cannibalize, so this is free. Otherwise develop + dig to optimize E(ttd)
        //    toward the live optimistic line.
        if mn.is_none() {
            if let Some(a) = fetch_activation(state, legal) {
                self.dlog(format!("T{}: min-ttd>cutoff → rip fetch to lower it", state.current_turn));
                return a;
            }
        }
        if let Some(a) = self.develop_and_dig(state, who, legal) {
            self.dlog(format!("T{}: gamble (min-ttd={:?})", state.current_turn, mn));
            return a;
        }
        // 4) No by-cutoff line and nothing left to dig — follow a slower deterministic
        //    line if one exists (better late than never).
        if let Some(a) = follow_line(state, who, legal, FALLBACK_HORIZON) {
            return a;
        }
        LegalAction::Pass
    }

    fn take_mulligan(&mut self, state: &SimState, mulligans_taken: u32) -> bool {
        let who = self.player_id;
        let p = recipe::p_cast_by(state, who, self.cutoff);
        // Ship hands too unlikely to combo by the cutoff: for a *race*, a slow hand is
        // nearly worthless, so demand a high P(cast by cutoff) and re-draw otherwise.
        // The bar loosens as we mulligan (fewer cards is worth more than a dead 7).
        // (Empirically ~optimal on the sample list at cutoff 4; the principled,
        // per-deck version is the self-calibrated indifference curve — see below.)
        let threshold = match mulligans_taken {
            0 => 0.55,
            1 => 0.38,
            _ => 0.20,
        };
        // KEEP7 experiment: always keep the opening 7 to remove all mulligan dynamics
        // and compare pure gameplay speed apples-to-apples.
        let mull = if std::env::var("KEEP7").is_ok() {
            false
        } else {
            mulligans_taken < 3 && p < threshold // always keep at 4 cards
        };
        if !mull {
            // Calibration probe: the kept hand's predicted P(cast by cutoff), to be
            // compared against the realized outcome (see `run_goldfish_calibration`).
            self.dlog(format!("CALIB {:.4}", p));
            let det = recipe::deterministic_cast_turn(state, who, self.cutoff);
            let hand: Vec<ObjId> = state.hand_of(who).map(|c| c.id).collect();
            self.dlog(format!("KEPT det={:?} [{}]", det, names(state, &hand)));
            // Machine-readable per-game summary for the aggregate stats (mull level,
            // predicted P, and whether the opening hand already had a deterministic
            // line by the cutoff).
            self.dlog(format!("STATS mull={} pred={:.4} det={}",
                mulligans_taken, p, det.is_some() as u8));
        }
        self.dlog(format!("T0: mull#{} P(cast by {})={:.2} → {}",
            mulligans_taken, self.cutoff, p, if mull { "MULL" } else { "KEEP" }));
        if self.compare {
            // Reference: the baseline DoomsdayStrategy's category-rule mulligan.
            let heur_mull = doomsday::dd_should_mulligan(state, who, mulligans_taken);
            if heur_mull != mull {
                let hand: Vec<ObjId> = state.hand_of(who).map(|c| c.id).collect();
                self.dlog(format!(
                    "DIFF mulligan#{}: principled={} (P={:.2}<thr {:.2}) heuristic={} hand=[{}]",
                    mulligans_taken,
                    if mull { "MULL" } else { "KEEP" }, p, threshold,
                    if heur_mull { "MULL" } else { "KEEP" },
                    names(state, &hand)));
            }
        }
        mull
    }

    // ── Cantrip / selection resolution: driven by the solver ──────────────────

    /// Ponder (and any "put back in any order, then maybe shuffle"): the keep-vs-
    /// shuffle + ordering decision falls out of `P(cast by cutoff)`.
    fn order_top_library(&mut self, cards: &[ObjId], state: &SimState) -> Vec<ObjId> {
        let who = self.player_id;
        let revealed: Vec<String> = cards.iter()
            .filter_map(|id| state.objects.get(id).map(|c| c.catalog_key.clone()))
            .collect();
        // Ponder offers a shuffle (a `MayDo` follows this `OrderTop`).
        let (choice, _p) = recipe::best_top_choice(state, who, &revealed, true, self.cutoff);
        match choice {
            recipe::TopChoice::Shuffle => {
                self.pending_shuffle = true;
                cards.to_vec() // order is irrelevant — we'll shuffle it away
            }
            recipe::TopChoice::Keep(order_keys) => {
                self.pending_shuffle = false;
                // Map the chosen key ordering back onto the actual card ids.
                let mut remaining: Vec<ObjId> = cards.to_vec();
                let mut out: Vec<ObjId> = Vec::new();
                for key in &order_keys {
                    if let Some(pos) = remaining.iter().position(|id|
                        state.objects.get(id).map_or(false, |c| &c.catalog_key == key))
                    {
                        out.push(remaining.remove(pos));
                    }
                }
                out.extend(remaining); // safety: append anything unmatched
                out
            }
        }
    }

    /// The only "may" we drive is Ponder's shuffle, flagged by `order_top_library`.
    fn resolve_choice(&mut self, _source: ObjId, req: &ChoiceRequest, _state: &SimState) -> ChoiceResult {
        match req {
            ChoiceRequest::Mode(_) => {
                let yes = self.pending_shuffle;
                self.pending_shuffle = false;
                ChoiceResult::Mode(if yes { 1 } else { 0 })
            }
            ChoiceRequest::Color => ChoiceResult::Color(Color::Black),
            ChoiceRequest::CreatureType => ChoiceResult::CreatureType("Wizard".to_string()),
            ChoiceRequest::CardName => ChoiceResult::CardName(String::new()),
            ChoiceRequest::WardPayment { .. } => ChoiceResult::Bool(true),
            ChoiceRequest::MayPutOnBattlefield { .. } => ChoiceResult::OptionalObject(None),
            ChoiceRequest::MayAttach => ChoiceResult::Bool(true),
        }
    }

    /// Searches and digs, by source:
    /// - **fetch land** → pick a black source, untapped-preferred (a land ordering —
    ///   the allowed heuristic; the *colour* requirement comes from the goal).
    /// - **tutor to top** (Personal Tutor) → the candidate maximizing P(cast by
    ///   cutoff) with it staged on top (drawn next turn) — i.e. Doomsday.
    /// - **dig to hand** (Flow State) → the candidate maximizing P(cast by cutoff)
    ///   with it added to hand now.
    fn choose_for_effect(&mut self, effect_id: ObjId, choices: &[ObjId], state: &SimState) -> Option<ObjId> {
        if choices.is_empty() { return None; }
        let who = self.player_id;
        let src_def = state.def_of(effect_id)
            .or_else(|| state.objects.get(&effect_id).and_then(|o| state.catalog.get(&o.catalog_key)));
        let is_fetch = src_def.map_or(false, |d| d.abilities().iter().any(|a| a.is_fetch_ability()));
        let principled = if is_fetch {
            // A fetched land enters now (like a dig to hand): pick the candidate that
            // most advances the objective, NOT merely "a black source". This prefers a
            // dual that also supplies the colour the line needs next (e.g. blue for the
            // tutor's pip) over a same-priority off-colour land (Underground Sea vs a
            // plain Swamp both rank as untapped-black, but only the Sea pays the pip).
            // Ties (objective-indifferent) break on land priority — untapped first.
            choices.iter().copied().max_by(|&a, &b| {
                self.candidate_p(state, a, false)
                    .partial_cmp(&self.candidate_p(state, b, false))
                    .unwrap_or(Ordering::Equal)
                    // Tie-break on min-ttd (lower = faster): separates a Sea (pays the pip
                    // sooner) from a same-priority Swamp where P(cast) saturates equal.
                    .then_with(|| self.candidate_ttd(state, b, false)
                        .partial_cmp(&self.candidate_ttd(state, a, false)).unwrap_or(Ordering::Equal))
                    .then_with(|| land_priority(recipe::card_role(state, who, a))
                        .cmp(&land_priority(recipe::card_role(state, who, b))))
            })
        } else {
            // Tutor stages on top (drawn next turn); a dig goes to hand now. Primary key
            // is the objective P(cast by cutoff); the min-ttd tie-break then separates the
            // by-cutoff lines that probability saturates equal — staging Doomsday (payoff
            // next turn) beats staging a tutor for it (payoff a turn later), for free.
            let to_top = src_def.map_or(false, |d| d.library_top_tutor().is_some());
            choices.iter().copied().max_by(|&a, &b| {
                self.candidate_p(state, a, to_top)
                    .partial_cmp(&self.candidate_p(state, b, to_top))
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| self.candidate_ttd(state, b, to_top)
                        .partial_cmp(&self.candidate_ttd(state, a, to_top)).unwrap_or(Ordering::Equal))
                    // Last resort (tight mana, both lines stochastic): the payoff beats a
                    // tutor for it, so we never loop tutor→tutor.
                    .then_with(|| payoff_rank(state, who, a).cmp(&payoff_rank(state, who, b)))
                    .then_with(|| keep_rank(state, who, a).cmp(&keep_rank(state, who, b)))
            })
        };
        if self.compare {
            let heur = heur_pick(state, who, choices);
            if heur != principled {
                let kind = if is_fetch { "fetch" } else { "dig/tutor" };
                self.dlog(format!("DIFF {} T{}: principled={} heuristic={} from [{}]",
                    kind, state.current_turn,
                    principled.map(|i| Self::nm(state, i)).unwrap_or_default(),
                    heur.map(|i| Self::nm(state, i)).unwrap_or_default(),
                    names(state, choices)));
            }
        }
        principled
    }

    /// Consider's surveil = the unified policy on {keep-on-top, bin}: maximize
    /// P(cast by cutoff) (the E(ttd) proxy), which already subsumes the min-ttd
    /// feasibility filter — an infeasible keep scores P = 0, and a card that's merely
    /// useless still wastes Consider's draw, scoring below a fresh unknown draw → bin.
    /// (So "minimum above cutoff on the keep → bin" is the P = 0 corner of this rule.)
    fn surveil_choice(&mut self, id: ObjId, state: &SimState) -> bool {
        let who = self.player_id;
        let Some(key) = state.objects.get(&id).map(|o| o.catalog_key.clone()) else { return false };
        let keep_p = recipe::p_cast_by_with_known_top(state, who, &[key.as_str()], true, self.cutoff);
        let base_p = recipe::p_cast_by(state, who, self.cutoff);
        let principled_bin = keep_p < base_p; // a known keep worse than an unknown draw → bin
        if self.compare {
            let heur_bin = heur_surveil_bin(state, who, id);
            if heur_bin != principled_bin {
                self.dlog(format!("DIFF surveil T{} {}: principled={} heuristic={} (keep_p={:.3} base_p={:.3})",
                    state.current_turn, key,
                    if principled_bin { "BIN" } else { "KEEP" },
                    if heur_bin { "BIN" } else { "KEEP" }, keep_p, base_p));
            }
        }
        principled_bin
    }

    /// Preordain's scry: choose the kept-on-top arrangement (rest to bottom) that
    /// maximizes P(cast by cutoff) — enumerated over the (≤2-card) reveal.
    fn scry(&mut self, top: &[ObjId], state: &SimState) -> (Vec<ObjId>, Vec<ObjId>) {
        let who = self.player_id;
        let keyed: Vec<(ObjId, String)> = top.iter()
            .filter_map(|&id| state.objects.get(&id).map(|o| (id, o.catalog_key.clone())))
            .collect();
        let mut best_keep: Vec<ObjId> = Vec::new();
        let mut best_p = recipe::p_cast_by(state, who, self.cutoff); // keep nothing on top
        for arrangement in ordered_subsets(&keyed) {
            let keys: Vec<&str> = arrangement.iter().map(|(_, k)| k.as_str()).collect();
            let p = recipe::p_cast_by_with_known_top(state, who, &keys, true, self.cutoff);
            if p > best_p {
                best_p = p;
                best_keep = arrangement.iter().map(|(id, _)| *id).collect();
            }
        }
        let bottom: Vec<ObjId> = top.iter().copied().filter(|id| !best_keep.contains(id)).collect();
        if self.compare {
            let heur_keep = heur_scry_keep(state, who, top);
            if heur_keep != best_keep {
                self.dlog(format!("DIFF scry T{}: principled_keep=[{}] heuristic_keep=[{}] of [{}]",
                    state.current_turn, names(state, &best_keep), names(state, &heur_keep), names(state, top)));
            }
        }
        (best_keep, bottom)
    }

    /// Brainstorm's put-back: bury the cards the plan does not need. INTERIM — driven
    /// by the solver's gap (`mana_gap` + `payoff_in_hand`), the recomputed "sufficient
    /// cards" set, NOT a value table. ("Which cards can I afford to lose" is exactly
    /// what the visible-path requirement set will answer fully.)
    fn put_on_library(&mut self, count: usize, candidates: &[ObjId], _top: bool,
                      state: &SimState) -> Vec<ObjId> {
        let who = self.player_id;
        // Keep the combo pieces the plan needs (`card_needed`); among the rest
        // (expendable), the last-ditch trinary weight buries irrelevant bricks before
        // relevant cards before cantrips.
        let mut expendable: Vec<ObjId> = candidates.iter().copied()
            .filter(|&id| !self.card_needed(state, id))
            .collect();
        expendable.sort_by_key(|&id| keep_rank(state, who, id));
        let mut buried: Vec<ObjId> = expendable.into_iter().take(count).collect();
        // If too few are expendable, we must still bury `count` — pad with the
        // lowest-keep-rank of the remaining (needed) cards.
        if buried.len() < count {
            let mut rest: Vec<ObjId> = candidates.iter().copied().filter(|id| !buried.contains(id)).collect();
            rest.sort_by_key(|&id| keep_rank(state, who, id));
            for id in rest {
                if buried.len() >= count { break; }
                buried.push(id);
            }
        }
        buried.truncate(count);
        if self.compare {
            let heur = heur_bury(state, self.player_id, count, candidates);
            let pa: std::collections::HashSet<ObjId> = buried.iter().copied().collect();
            let hb: std::collections::HashSet<ObjId> = heur.iter().copied().collect();
            if pa != hb {
                self.dlog(format!("DIFF brainstorm-bury T{}: principled=[{}] heuristic=[{}] of [{}]",
                    state.current_turn, names(state, &buried), names(state, &heur), names(state, candidates)));
            }
        }
        buried
    }

    // ── Required trait methods ─────────────────────────────────────────────────

    fn plan_gap(&self, _state: &SimState) -> TargetGap { TargetGap::default() }

    /// London-mulligan bottoming (a mulligan decision — an allowed exception):
    /// bottom the cards the plan doesn't need (`card_needed`, solver-derived); pad
    /// with extras if everything is needed.
    fn london_bottom(&self, state: &SimState, n: usize) -> Vec<ObjId> {
        let who = self.player_id;
        let hand: Vec<ObjId> = state.hand_of(who).map(|c| c.id).collect();
        let mut bottom: Vec<ObjId> = hand.iter().copied().filter(|&id| !self.card_needed(state, id)).collect();
        if bottom.len() < n {
            for &id in &hand {
                if bottom.len() >= n { break; }
                if !bottom.contains(&id) { bottom.push(id); }
            }
        }
        bottom.truncate(n);
        bottom
    }

    fn card_fills(&self, _card_id: ObjId, _gap: &TargetGap, _state: &SimState) -> f64 { 0.0 }
}

// ── AggroMullStrategy: experiment wrapper (baseline gameplay + aggressive mull) ──
//
// Completes the 2×2 (gameplay × mulligan): runs an inner strategy's gameplay
// verbatim but swaps in the aggressive `p_cast_by`-threshold mulligan, to measure
// how much of the ASAP edge is the mulligan vs the in-game play. Debug-only.

pub struct AggroMullStrategy {
    inner: Box<dyn Strategy>,
    cutoff: u32,
}

impl AggroMullStrategy {
    pub fn new(inner: Box<dyn Strategy>, cutoff: u32) -> Self {
        Self { inner, cutoff: cutoff.max(1) }
    }
}

impl Strategy for AggroMullStrategy {
    // The one override: the aggressive P(cast by cutoff) threshold mulligan.
    fn take_mulligan(&mut self, state: &SimState, mulligans_taken: u32) -> bool {
        let who = self.inner.player_id();
        let p = recipe::p_cast_by(state, who, self.cutoff);
        let threshold = match mulligans_taken { 0 => 0.55, 1 => 0.38, _ => 0.20 };
        mulligans_taken < 3 && p < threshold
    }

    // Forward exactly the methods DoomsdayStrategy overrides — its gameplay verbatim.
    // Everything else falls through to the trait default, which is precisely what
    // DoomsdayStrategy uses for those, so the wrapper is identical there.
    fn choose_action(&mut self, s: &SimState, ap: PlayerId, l: &[LegalAction]) -> LegalAction { self.inner.choose_action(s, ap, l) }
    fn choose_mana_ability(&mut self, s: &SimState, w: PlayerId, a: &[ManaAbilityOption], m: &ManaCost) -> Option<ManaActivation> { self.inner.choose_mana_ability(s, w, a, m) }
    fn announce(&mut self, s: &SimState, c: ObjId, o: &AnnounceOptions) -> AnnounceChoice { self.inner.announce(s, c, o) }
    fn declare_attackers(&mut self, s: &SimState) -> Vec<(ObjId, Option<ObjId>)> { self.inner.declare_attackers(s) }
    fn declare_blockers(&mut self, s: &SimState) -> Vec<(ObjId, ObjId)> { self.inner.declare_blockers(s) }
    fn player_id(&self) -> PlayerId { self.inner.player_id() }
    fn plan_gap(&self, s: &SimState) -> TargetGap { self.inner.plan_gap(s) }
    fn card_fills(&self, c: ObjId, g: &TargetGap, s: &SimState) -> f64 { self.inner.card_fills(c, g, s) }
    fn drain_decisions(&mut self) -> Vec<String> { self.inner.drain_decisions() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtg_engine::{build_catalog, PlayerState, Zone};

    /// Personal Tutor's filter is "a sorcery", and a Doomsday deck also runs other
    /// sorceries (Thoughtseize, Edge of Autumn). The strategy MUST stage Doomsday (the
    /// payoff) — staging a brick on top wastes the whole tutor and the draw.
    #[test]
    fn personal_tutor_stages_doomsday_not_another_sorcery() {
        let mut s = SimState::new(PlayerState::new("us"), PlayerState::new("opp"));
        s.catalog = build_catalog();
        // Three blue/black sources in play so the staged-Doomsday line is castable.
        for _ in 0..3 {
            s.place_card(PlayerId::Us, "Underground Sea", Zone::Battlefield);
        }
        let pt = s.place_card(PlayerId::Us, "Personal Tutor", Zone::Graveyard);
        let dd = s.place_card(PlayerId::Us, "Doomsday", Zone::Library);
        let th = s.place_card(PlayerId::Us, "Thoughtseize", Zone::Library);
        let edge = s.place_card(PlayerId::Us, "Edge of Autumn", Zone::Library);

        let mut strat = DDGoldfishStrategy::new(4);
        let pick = strat.choose_for_effect(pt, &[dd, th, edge], &s);
        assert_eq!(pick, Some(dd), "Personal Tutor must stage Doomsday, not a brick sorcery");
    }

    /// With a 4-Personal-Tutor build, PT's candidates often include BOTH Doomsday and
    /// another Personal Tutor. With tight mana (no deterministic line), the stochastic
    /// model scores staging either as "1 useful card" — a tie — so a naive `max_by`
    /// could stage the redundant tutor (looping tutor→tutor, never casting). The payoff
    /// must dominate: staging Doomsday skips the tutor's extra cast.
    #[test]
    fn personal_tutor_stages_doomsday_over_a_redundant_tutor() {
        let mut s = SimState::new(PlayerState::new("us"), PlayerState::new("opp"));
        s.catalog = build_catalog();
        // One untapped Sea: enough to cast a tutor, NOT enough for a deterministic BBB
        // line — so staging DD vs another PT ties on the stochastic estimate.
        s.place_card(PlayerId::Us, "Underground Sea", Zone::Battlefield);
        let pt = s.place_card(PlayerId::Us, "Personal Tutor", Zone::Graveyard);
        let dd = s.place_card(PlayerId::Us, "Doomsday", Zone::Library);
        let other_pt = s.place_card(PlayerId::Us, "Personal Tutor", Zone::Library);

        let mut strat = DDGoldfishStrategy::new(4);
        // Doomsday FIRST, the tutor LAST — the order that made a bare `max_by` pick the
        // tutor (it returns the last of equal elements).
        let pick = strat.choose_for_effect(pt, &[dd, other_pt], &s);
        assert_eq!(pick, Some(dd), "must stage the payoff, never loop on a redundant tutor");
    }
}
