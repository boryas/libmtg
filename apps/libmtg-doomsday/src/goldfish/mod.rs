//! Doomsday goldfish Monte-Carlo simulator.
//!
//! Drop the opponent (it takes inert turns), race the [`DoomsdayStrategy`] to cast
//! Doomsday, and measure *how fast* (cast-turn distribution) and with *how many
//! layers of protection* in hand. Aggregate stats converge by the law of large
//! numbers, so runs are unseeded — no reproducibility knob needed.
//!
//! This is the baseline (reuses `DoomsdayStrategy`); a dedicated cast-ASAP
//! strategy and a wasm/web frontend are later steps.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{dd_card_evaluator, DoomsdayStrategy, MatchupInfo};
use libmtg_engine::{
    build_catalog, run_game, AlwaysPass, GameEvent, ObjId, Objective, PlayerId, Scenario, SimState,
    Strategy,
};
use rand::SeedableRng;
use serde::Serialize;

/// SPIKE: backward "can we cast Doomsday?" over the resource model (capabilities
/// read from IR; mana by subtraction, the gap by recomputed sufficiency).
pub mod recipe;

/// The cast-Doomsday-ASAP strategy that follows the `recipe` solver. See
/// [`strategy::DDGoldfishStrategy`].
pub mod strategy;

/// Pluggable opening-hand mulligan policies for the goldfish pilot.
pub mod mull;

pub use mull::{should_mulligan, HandSignals, MullMode};
pub use strategy::{DDGoldfishStrategy, DEFAULT_CUTOFF};

/// v1 "protection layers": disruption the Doomsday player holds to protect the
/// combo turn. Counted by name in hand at resolution. Mana-castability weighting
/// is the documented v2.
pub const DEFAULT_PROTECTION: &[&str] =
    &["Force of Will", "Daze", "Force of Negation", "Thoughtseize"];

/// Goldfish objective: end the run the instant our Doomsday resolves. Unlike the
/// pilegen objective it has no side effects (no life accounting) — goldfish reads
/// *when* DD resolved (`SimState::current_turn`) and *what protection* is held off
/// the final state.
#[derive(Default)]
pub struct GoldfishObjective;

impl Objective for GoldfishObjective {
    fn observe(&mut self, event: &GameEvent, state: &mut SimState) -> bool {
        if let GameEvent::SpellResolved { card_id, controller } = event {
            return *controller == PlayerId::Us
                && state
                    .objects
                    .get(card_id)
                    .map_or(false, |o| o.catalog_key == "Doomsday");
        }
        false
    }
}

/// Aggregated results of a goldfish run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GoldfishStats {
    pub games: u32,
    /// Games where Doomsday never resolved within the turn cap.
    pub fails: u32,
    /// turn → number of games where DD resolved on that turn.
    pub cast_turn: BTreeMap<u8, u32>,
    /// (#protection cards in hand at resolution) → number of games.
    pub protection: BTreeMap<u32, u32>,
    /// The cutoff turn the (ASAP) strategy played to (0 = N/A, e.g. baseline).
    pub cutoff: u8,
    /// mulligans-taken (0..3) → number of games KEPT at that level (hand size 7−k).
    pub mull_count: BTreeMap<u8, u32>,
    /// mulligans-taken → Σ of the kept hand's predicted P(cast by cutoff); divide by
    /// `mull_count` for the average predicted probability at that hand size.
    pub mull_pred_sum: BTreeMap<u8, f64>,
    /// Games that cast by the cutoff whose OPENING hand already had a guaranteed
    /// (no-draw) line by the cutoff.
    pub deterministic_cast: u32,
    /// Games that cast by the cutoff by DRAWING / cantripping into the line (opening
    /// hand had no guaranteed line).
    pub stochastic_cast: u32,
    /// Among games that MISSED the cutoff (ASAP, didn't cast): what the cutoff-state
    /// hand was missing — mana (couldn't make BBB this turn), the payoff (no Doomsday
    /// in hand), both, or neither (had both but didn't cast — a sequencing/timing gap).
    pub miss_mana: u32,
    pub miss_payoff: u32,
    pub miss_both: u32,
    pub miss_neither: u32,
    /// A handful of sample games (the first few of the run) for flavor: the kept
    /// opening hand, mulligans taken, and the cast turn (or none = never cast).
    pub samples: Vec<SampleGame>,
}

/// One sample game for display: the kept opening hand and its outcome.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SampleGame {
    /// Mulligans taken (0..=3); the kept hand has 7 − this many cards.
    pub mulls: u8,
    /// The kept opening hand (catalog keys), post-mulligan.
    pub hand: Vec<String>,
    /// Hands thrown back before the keep (each the 7 seen), in order.
    pub mulligans: Vec<Vec<String>>,
    /// Turn Doomsday resolved, or None if it never did by the cutoff.
    pub cast_turn: Option<u32>,
}

impl GoldfishStats {
    pub fn successes(&self) -> u32 {
        self.games - self.fails
    }

    pub fn fail_rate(&self) -> f64 {
        if self.games == 0 {
            0.0
        } else {
            self.fails as f64 / self.games as f64
        }
    }

    /// Cumulative P(cast by turn `t`) over all games (failures count as not-cast).
    pub fn cast_by(&self, t: u8) -> f64 {
        if self.games == 0 {
            return 0.0;
        }
        let n: u32 = self
            .cast_turn
            .iter()
            .filter(|(&turn, _)| turn <= t)
            .map(|(_, &c)| c)
            .sum();
        n as f64 / self.games as f64
    }

    pub fn mean_cast_turn(&self) -> f64 {
        let s = self.successes();
        if s == 0 {
            return f64::NAN;
        }
        let tot: u32 = self.cast_turn.iter().map(|(&t, &c)| t as u32 * c).sum();
        tot as f64 / s as f64
    }

    pub fn mean_protection(&self) -> f64 {
        let s = self.successes();
        if s == 0 {
            return f64::NAN;
        }
        let tot: u32 = self.protection.iter().map(|(&p, &c)| p * c).sum();
        tot as f64 / s as f64
    }
}

/// Card evaluator for the cast-ASAP goldfish. `DDGoldfishStrategy` makes every
/// selection decision itself via the solver objective (no value table), so the
/// engine's evaluator-defaulted paths are never consulted for our player; the
/// opponent is inert. A neutral constant is therefore correct — and keeps a value
/// table out of the model.
pub fn dd_goldfish_evaluator() -> Arc<dyn Fn(PlayerId, ObjId, &SimState) -> f64 + Send + Sync> {
    Arc::new(|_who, _id, _state| 0.5)
}

/// Shared goldfish loop: `games` simulations of `deck` against an inert opponent
/// (`AlwaysPass`), aggregating the cast-turn + protection distributions. The
/// Doomsday player's strategy is built fresh per game by `make_us`, with
/// `evaluator` installed as the card evaluator.
fn run_goldfish_inner<F>(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u8,
    make_us: F,
    evaluator: Arc<dyn Fn(PlayerId, ObjId, &SimState) -> f64 + Send + Sync>,
) -> GoldfishStats
where
    F: Fn() -> Box<dyn Strategy>,
{
    let catalog = build_catalog();
    // Inert opponent: enough basics that it never decks within the turn cap.
    let opp_deck: Vec<(String, i32, String)> = vec![("Island".to_string(), 60, "main".to_string())];
    let mut rng = rand::rngs::SmallRng::from_entropy();

    let mut stats = GoldfishStats {
        games,
        cutoff,
        ..Default::default()
    };
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: make_us(),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: Arc::clone(&evaluator),
            objective: Box::new(GoldfishObjective::default()),
            // The cutoff IS the horizon: there is no reason to simulate past the turn
            // by which the objective is judged. (`.max(1)` guards a degenerate 0.)
            max_turns: cutoff.max(1),
            on_play: None,
        };
        let state = run_game(scenario, &mut rng);
        // Keep a few sample games (the first handful) for flavor: opening hand + outcome.
        const SAMPLE_LIMIT: usize = 8;
        if stats.samples.len() < SAMPLE_LIMIT {
            let hand = state.opening_hand_us.clone();
            let mulligans = state.mulliganed_hands_us.clone();
            let mulls = mulligans.len() as u8;
            let cast_turn = state.terminal.then_some(state.current_turn as u32);
            stats.samples.push(SampleGame { mulls, hand, mulligans, cast_turn });
        }
        let cast_by_cutoff = state.terminal && cutoff > 0 && state.current_turn <= cutoff;
        if state.terminal {
            *stats.cast_turn.entry(state.current_turn).or_insert(0) += 1;
            let prot = state
                .hand_of(PlayerId::Us)
                .filter(|c| protection.contains(&c.catalog_key.as_str()))
                .count() as u32;
            *stats.protection.entry(prot).or_insert(0) += 1;
        } else {
            stats.fails += 1;
        }
        // Rich per-game stats from the strategy's machine-readable summary line:
        // "STATS mull=<k> pred=<p> det=<0|1>" (emitted only by the ASAP strategy).
        if let Some((mull, pred, det)) = parse_stats_line(&state.decision_log) {
            *stats.mull_count.entry(mull).or_insert(0) += 1;
            *stats.mull_pred_sum.entry(mull).or_insert(0.0) += pred;
            if cast_by_cutoff {
                if det {
                    stats.deterministic_cast += 1;
                } else {
                    stats.stochastic_cast += 1;
                }
            } else {
                use recipe::MissingElement;
                match recipe::missing_element(&state, PlayerId::Us) {
                    MissingElement::Mana => stats.miss_mana += 1,
                    MissingElement::Payoff => stats.miss_payoff += 1,
                    MissingElement::Both => stats.miss_both += 1,
                    MissingElement::Neither => stats.miss_neither += 1,
                }
            }
        }
    }
    stats
}

/// Parse the strategy's `STATS mull=<k> pred=<p> det=<0|1>` summary, if present.
fn parse_stats_line(log: &[String]) -> Option<(u8, f64, bool)> {
    let line = log.iter().find(|l| l.starts_with("STATS "))?;
    let (mut mull, mut pred, mut det) = (None, None, None);
    for tok in line.split_whitespace() {
        if let Some(v) = tok.strip_prefix("mull=") {
            mull = v.parse::<u8>().ok();
        } else if let Some(v) = tok.strip_prefix("pred=") {
            pred = v.parse::<f64>().ok();
        } else if let Some(v) = tok.strip_prefix("det=") {
            det = v.parse::<u8>().ok().map(|n| n != 0);
        }
    }
    Some((mull?, pred?, det?))
}

/// Run `games` goldfish simulations of `deck` (a `[(name, qty, board)]` list) and
/// aggregate the cast-turn + protection distributions. The opponent is inert
/// (`AlwaysPass`); the Doomsday player uses the baseline `DoomsdayStrategy`.
pub fn run_goldfish(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u32,
) -> GoldfishStats {
    let evaluator = dd_card_evaluator(MatchupInfo::default());
    run_goldfish_inner(
        deck,
        games,
        protection,
        cutoff.min(u8::MAX as u32) as u8,
        || Box::new(DoomsdayStrategy::new(MatchupInfo::default())),
        evaluator,
    )
}

/// Run `games` goldfish simulations driving the cast-ASAP [`DDGoldfishStrategy`]
/// (which follows the `recipe` solver to combo by `cutoff`) under the default
/// ([`MullMode::Realistic`]) mulligan. The headline `P(cast by cutoff)` is
/// `stats.cast_by(cutoff)`.
pub fn run_goldfish_asap(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u32,
) -> GoldfishStats {
    run_goldfish_asap_mode(deck, games, protection, cutoff, MullMode::default())
}

/// Like [`run_goldfish_asap`], but with an explicit opening-hand [`MullMode`]
/// (Keep7 / Realistic / Aggressive) — the goldfish web/CLI mulligan selector.
pub fn run_goldfish_asap_mode(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u32,
    mode: MullMode,
) -> GoldfishStats {
    run_goldfish_inner(
        deck,
        games,
        protection,
        cutoff.min(u8::MAX as u32) as u8,
        move || Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, mode)),
        dd_goldfish_evaluator(),
    )
}

/// Debug A/B: run `games` SEEDED cast-ASAP games in compare mode and return a log
/// of, per game, the outcome plus every decision where the principled (objective)
/// policy disagrees with the reference value-table heuristic. Seeded so a run is
/// reproducible; the principled policy drives the game (the heuristic is only
/// shadow-evaluated on the same states), so this surfaces *where* and *why* they
/// differ without the two trajectories forking.
pub fn run_goldfish_compare(
    deck: &[(String, i32, String)],
    cutoff: u32,
    seed: u64,
    games: u32,
) -> Vec<String> {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> = vec![("Island".to_string(), 60, "main".to_string())];
    let mut rng = rand::rngs::SmallRng::seed_from_u64(seed);
    let mut out = Vec::new();
    for g in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::new_comparing(cutoff)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: (cutoff.min(u8::MAX as u32) as u8).max(1),
            on_play: None,
        };
        let state = run_game(scenario, &mut rng);
        let outcome = if state.terminal {
            format!("cast T{}", state.current_turn)
        } else {
            "FAILED".to_string()
        };
        let diffs: Vec<&String> = state.decision_log.iter().filter(|l| l.starts_with("DIFF")).collect();
        out.push(format!("── game {g} (seed {seed}): {outcome} — {} disagreement(s) ──", diffs.len()));
        for d in &diffs {
            out.push(format!("    {d}"));
        }
    }
    out
}

/// Debug 2×2 cell: the **baseline `DoomsdayStrategy` gameplay** but with the
/// **aggressive `p_cast_by` mulligan** swapped in (`AggroMullStrategy`). Isolates how
/// much of the ASAP edge is the mulligan vs the in-game play.
pub fn run_goldfish_baseline_aggro(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u32,
) -> GoldfishStats {
    run_goldfish_inner(
        deck,
        games,
        protection,
        cutoff.min(u8::MAX as u32) as u8,
        move || Box::new(strategy::AggroMullStrategy::new(
            Box::new(DoomsdayStrategy::new(MatchupInfo::default())),
            cutoff,
        )),
        dd_card_evaluator(MatchupInfo::default()),
    )
}

/// Debug: **calibration** of the strategy's `P(cast by cutoff)` estimate. Run
/// `games` games; for each, record the kept opening hand's *predicted* P (logged as
/// `CALIB …`) and the *realized* outcome (did Doomsday actually resolve by `cutoff`).
/// Bucket the predictions into deciles and report, per bucket, the mean prediction
/// vs the observed success rate. A well-calibrated `g` has observed ≈ predicted on
/// the diagonal; systematic gaps (e.g. observed ≫ predicted in cantrip-rich buckets)
/// expose where the estimator is wrong. Returns `(mean_predicted, observed, n)` × 10.
pub fn run_goldfish_calibration(
    deck: &[(String, i32, String)],
    cutoff: u32,
    games: u32,
) -> Vec<(f64, f64, u32)> {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> = vec![("Island".to_string(), 60, "main".to_string())];
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let mut sum_pred = [0.0f64; 10];
    let mut succ = [0u32; 10];
    let mut n = [0u32; 10];
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::new(cutoff)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: (cutoff.min(u8::MAX as u32) as u8).max(1),
            on_play: None,
        };
        let state = run_game(scenario, &mut rng);
        let Some(pred) = state
            .decision_log
            .iter()
            .find_map(|l| l.strip_prefix("CALIB ").and_then(|s| s.trim().parse::<f64>().ok()))
        else {
            continue;
        };
        let success = state.terminal && (state.current_turn as u32) <= cutoff;
        let b = ((pred * 10.0) as usize).min(9);
        sum_pred[b] += pred;
        n[b] += 1;
        if success {
            succ[b] += 1;
        }
    }
    (0..10)
        .map(|b| {
            let cnt = n[b].max(1) as f64;
            (sum_pred[b] / cnt, succ[b] as f64 / cnt, n[b])
        })
        .collect()
}

/// Debug: dump the full trace of games where the strategy held a *deterministic*
/// line by the cutoff (kept-hand `CALIB == 1.0`) yet failed to cast by the cutoff —
/// the calibration's ~2.5% "guaranteed but didn't happen" gap. For each such game it
/// prints the kept hand + per-turn intent (decision log) + the engine's actual plays
/// (`state.log`), so we can see whether the solver over-claimed or the strategy
/// mis-executed. Stops after `max_dumps`.
pub fn run_goldfish_audit_det(
    deck: &[(String, i32, String)],
    cutoff: u32,
    seed: u64,
    games: u32,
    max_dumps: u32,
) -> Vec<String> {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> = vec![("Island".to_string(), 60, "main".to_string())];
    let mut rng = rand::rngs::SmallRng::seed_from_u64(seed);
    let mut out = Vec::new();
    let mut dumps = 0u32;
    for g in 0..games {
        if dumps >= max_dumps {
            break;
        }
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::new(cutoff)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: (cutoff.min(u8::MAX as u32) as u8).max(1),
            on_play: None,
        };
        let state = run_game(scenario, &mut rng);
        let calib = state
            .decision_log
            .iter()
            .find_map(|l| l.strip_prefix("CALIB ").and_then(|s| s.trim().parse::<f64>().ok()))
            .unwrap_or(0.0);
        let cast_by = state.terminal && (state.current_turn as u32) <= cutoff;
        if calib >= 0.9999 && !cast_by {
            dumps += 1;
            let outcome = if state.terminal {
                format!("cast T{}", state.current_turn)
            } else {
                "never cast".to_string()
            };
            out.push(format!("════ deterministic (CALIB=1.0) but FAILED #{dumps} (game {g}) — {outcome} ════"));
            for l in state.decision_log.iter().filter(|l| l.starts_with("KEPT") || l.starts_with('T')) {
                out.push(format!("  intent: {l}"));
            }
            for l in &state.log {
                out.push(format!("  play:   {l}"));
            }
        }
    }
    out
}

/// A sample Doomsday decklist, used by the CLI and tests until text/URL decklist
/// input lands. Mirrors the validation deck the engine tests exercised.
pub fn sample_doomsday_deck() -> Vec<(String, i32, String)> {
    [
        ("Underground Sea", 3),
        ("Polluted Delta", 4),
        ("Flooded Strand", 1),
        ("Misty Rainforest", 1),
        ("Scalding Tarn", 1),
        ("Marsh Flats", 1),
        ("Island", 1),
        ("Swamp", 1),
        ("Undercity Sewers", 2),
        ("Wasteland", 3),
        ("Cavern of Souls", 1),
        ("Lotus Petal", 2),
        ("Lion's Eye Diamond", 1),
        ("Dark Ritual", 4),
        ("Doomsday", 4),
        ("Brainstorm", 4),
        ("Ponder", 4),
        ("Consider", 1),
        ("Edge of Autumn", 1),
        ("Force of Will", 4),
        ("Daze", 3),
        ("Thoughtseize", 2),
        ("Street Wraith", 1),
        ("Thassa's Oracle", 1),
        ("Unearth", 1),
        ("Tamiyo, Inquisitive Student", 4),
        ("Orcish Bowmasters", 2),
        ("Murktide Regent", 2),
    ]
    .iter()
    .map(|(n, q)| (n.to_string(), *q, "main".to_string()))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goldfish_runs_and_casts_sometimes() {
        let stats = run_goldfish(&sample_doomsday_deck(), 40, DEFAULT_PROTECTION, 10);
        assert_eq!(stats.games, 40);
        assert_eq!(stats.successes() + stats.fails, 40);
        // The baseline DD strategy should resolve Doomsday in at least one of 40 games.
        assert!(stats.successes() > 0, "expected at least one cast in 40 games");
        // Every recorded cast turn is within the cap.
        assert!(stats.cast_turn.keys().all(|&t| (1..=10).contains(&t)));
    }

    #[test]
    fn asap_casts_doomsday_reliably() {
        // The cast-ASAP strategy, following the recipe solver, should resolve
        // Doomsday in the large majority of goldfish games given a generous horizon.
        // (Cutoff = horizon here, so a long cutoff exercises the full cast tail.)
        let stats = run_goldfish_asap(&sample_doomsday_deck(), 300, DEFAULT_PROTECTION, 10);
        assert_eq!(stats.games, 300);
        assert!(
            stats.fail_rate() < 0.2,
            "ASAP strategy fail rate too high: {:.2} (cast turns: {:?})",
            stats.fail_rate(),
            stats.cast_turn
        );
        assert!(stats.cast_turn.keys().all(|&t| (1..=10).contains(&t)));
    }

    #[test]
    fn asap_is_at_least_as_fast_as_baseline_by_cutoff() {
        // The whole point: an aggressive cast-ASAP pilot should cast Doomsday by the
        // cutoff turn at least as often as the (mana-development-oriented) baseline.
        let deck = sample_doomsday_deck();
        let cutoff = 4u8;
        let asap = run_goldfish_asap(&deck, 600, DEFAULT_PROTECTION, cutoff as u32);
        let base = run_goldfish(&deck, 600, DEFAULT_PROTECTION, cutoff as u32);
        let asap_by = asap.cast_by(cutoff);
        let base_by = base.cast_by(cutoff);
        // Generous slack for Monte-Carlo noise; the directional claim is what matters.
        assert!(
            asap_by + 0.05 >= base_by,
            "ASAP P(cast by {cutoff})={asap_by:.3} should be >= baseline {base_by:.3}"
        );
    }
}
