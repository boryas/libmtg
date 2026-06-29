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
    build_catalog, run_game, AlwaysPass, GameEvent, ObjId, Objective, PlayerId, PlayerState,
    Scenario, SimState, Strategy, Zone,
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

mod learned_gen;
pub mod learned_mull;

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
/// `count_car`: when true, popping The Fantasticar (its sacrifice trigger creating
/// the Construct tokens) also ends the run as a "send". When false the run is the
/// Doomsday-only baseline — the apples-to-apples comparison for the two-wincon speedup.
#[derive(Default)]
pub struct GoldfishObjective {
    pub count_car: bool,
}

impl Objective for GoldfishObjective {
    fn observe(&mut self, event: &GameEvent, state: &mut SimState) -> bool {
        match event {
            GameEvent::SpellResolved { card_id, controller } => {
                *controller == PlayerId::Us
                    && state.objects.get(card_id).map_or(false, |o| o.catalog_key == "Doomsday")
            }
            // The Fantasticar popped: its trigger created a "Fantasticar Construct".
            GameEvent::TokenCreated { controller, token_key, .. } => {
                self.count_car
                    && *controller == PlayerId::Us
                    && token_key == "Fantasticar Construct"
            }
            _ => false,
        }
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
    /// mulligans-taken → number of those kept games that actually CAST by the cutoff;
    /// divide by `mull_count` for the realized P(cast | kept at this hand size).
    pub mull_cast: BTreeMap<u8, u32>,
    /// Air content of the FIRST opening 7, split by its fate — Σ air cards + #hands for
    /// the 7s that were KEPT at 7 vs the 7s that were MULLIGANED. Lets us ask, within one
    /// deck, "do air-heavier 7s get thrown back?" (the Wasteland-is-air mull, isolated).
    pub kept7_air_sum: u64,
    pub kept7_count: u64,
    pub mull7_air_sum: u64,
    pub mull7_count: u64,
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
    on_play: Option<bool>, // None = randomize 50/50 per game (the default)
    count_car: bool,       // also end the run on a Fantasticar pop (two-wincon mode)
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
            objective: Box::new(GoldfishObjective { count_car }),
            // The cutoff IS the horizon: there is no reason to simulate past the turn
            // by which the objective is judged. (`.max(1)` guards a degenerate 0.)
            max_turns: cutoff.max(1),
            on_play,
            fixed_us_hand: None,
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
        // Air content of the FIRST opening 7, by its fate. If no mulligan was taken the
        // kept hand IS that 7; otherwise the first thrown-back hand is. (Isolates whether
        // air-heavier sevens get mulliganed, within a fixed deck.)
        let air = |h: &[String]| h.iter().filter(|c| mull::is_air(c)).count() as u64;
        if let Some(first_mull) = state.mulliganed_hands_us.first() {
            stats.mull7_air_sum += air(first_mull);
            stats.mull7_count += 1;
        } else if !state.opening_hand_us.is_empty() {
            stats.kept7_air_sum += air(&state.opening_hand_us);
            stats.kept7_count += 1;
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
                *stats.mull_cast.entry(mull).or_insert(0) += 1;
            }
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
        None, // randomized play/draw (default)
        false, // Doomsday-only
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
    run_goldfish_asap_mode(deck, games, protection, cutoff, MullMode::default(), None)
}

/// Like [`run_goldfish_asap`], but with an explicit opening-hand [`MullMode`]
/// (Keep7 / Realistic / Aggressive) — the goldfish web/CLI mulligan selector.
pub fn run_goldfish_asap_mode(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u32,
    mode: MullMode,
    on_play: Option<bool>,
) -> GoldfishStats {
    run_goldfish_inner(
        deck,
        games,
        protection,
        cutoff.min(u8::MAX as u32) as u8,
        move || Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, mode)),
        dd_goldfish_evaluator(),
        on_play,
        false, // Doomsday-only (see run_goldfish_send for the two-wincon variant)
    )
}

/// Realized two-wincon goldfish: like [`run_goldfish_asap_mode`], but when `car` is
/// true the pilot also pursues The Fantasticar pop and a pop counts as a "send"
/// (`stats.cast_by(cutoff)` then = P(send by cutoff)). Run with `car=false` for the
/// Doomsday-only baseline and `car=true` for the two-wincon number — same deck, same
/// mulligan, so the difference is exactly the car's contribution.
pub fn run_goldfish_send(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u32,
    mode: MullMode,
    on_play: Option<bool>,
    car: bool,
) -> GoldfishStats {
    run_goldfish_inner(
        deck,
        games,
        protection,
        cutoff.min(u8::MAX as u32) as u8,
        move || Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, mode).with_car(car)),
        dd_goldfish_evaluator(),
        on_play,
        car,
    )
}

/// Emit a labeled keep-all-7 dataset for the mulligan-learning bake-off. Runs `games`
/// **Keep7** games (keep every dealt 7, on the play) and writes one CSV row per game:
/// the count of each distinct deck card in the opening 7, plus the solver-only signals
/// (`det_line`, `tami_flip`, `p_cast`) computed on that opening hand, plus the realized
/// `win` (cast Doomsday by `cutoff`). The property tags are derived downstream (Python)
/// from the card-name counts, so the tag table can be iterated without re-running.
pub fn run_goldfish_dump(deck: &[(String, i32, String)], games: u32, cutoff: u32) -> String {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> =
        vec![("Island".to_string(), 60, "main".to_string())];
    let mut cards: Vec<String> = deck.iter().map(|(n, _, _)| n.clone()).collect();
    cards.sort();
    cards.dedup();
    // Full deck as a flat multiset (for reconstructing the post-keep library).
    let full: Vec<String> = deck
        .iter()
        .flat_map(|(n, q, _)| std::iter::repeat(n.clone()).take((*q).max(0) as usize))
        .collect();
    let cutoff_u8 = (cutoff.min(u8::MAX as u32) as u8).max(1);
    let mut rng = rand::rngs::SmallRng::from_entropy();

    let mut out = String::new();
    // Quote card names — some (Jace, Tamiyo) contain commas.
    let header: Vec<String> = cards.iter().map(|c| format!("\"{c}\"")).collect();
    out.push_str(&header.join(","));
    out.push_str(",det_line,tami_flip,p_cast,realistic,aggressive,win\n");

    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, MullMode::Keep7)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: cutoff_u8,
            on_play: Some(true), // labels are on the play
            fixed_us_hand: None,
        };
        let state = run_game(scenario, &mut rng);
        let hand = state.opening_hand_us.clone();
        if hand.is_empty() {
            continue;
        }
        let win = u8::from(state.terminal && state.current_turn <= cutoff_u8);

        // Reconstruct opening hand + remaining library to compute the solver signals on
        // the *opening* hand (det_line / p_cast_by are draw-order-independent).
        let mut s2 = SimState::new(PlayerState::new("us"), PlayerState::new("opp"));
        s2.catalog = catalog.clone();
        for h in &hand {
            s2.place_card(PlayerId::Us, h, Zone::Hand { known: false });
        }
        let mut lib = full.clone();
        for h in &hand {
            if let Some(i) = lib.iter().position(|c| c == h) {
                lib.remove(i);
            }
        }
        for c in &lib {
            s2.place_card(PlayerId::Us, c, Zone::Library);
        }
        let sig = mull::hand_signals(&s2, PlayerId::Us, cutoff);
        let p = recipe::p_cast_by(&s2, PlayerId::Us, cutoff);
        let realistic = u8::from(mull::realistic_keep(&sig, 0));
        let aggressive = u8::from(mull::aggressive_keep(&s2, PlayerId::Us, cutoff));

        let counts: Vec<String> = cards
            .iter()
            .map(|c| hand.iter().filter(|h| h.as_str() == c).count().to_string())
            .collect();
        out.push_str(&counts.join(","));
        out.push_str(&format!(
            ",{},{},{:.4},{},{},{}\n",
            u8::from(sig.det_line),
            u8::from(sig.tami_fast_flip),
            p,
            realistic,
            aggressive,
            win
        ));
    }
    out
}

/// Estimate a SPECIFIC opening hand's P(cast DD by `cutoff`) by replaying that exact
/// 7 under `games` fresh library shuffles (the hand is forced, no mulligan, on the play).
/// This is the model-free per-hand probability — the ground truth to check the learned
/// model against. Returns the win fraction.
pub fn run_goldfish_fixed_hand(
    deck: &[(String, i32, String)],
    hand: &[String],
    cutoff: u32,
    games: u32,
    on_play: bool,
) -> f64 {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> =
        vec![("Island".to_string(), 60, "main".to_string())];
    let cutoff_u8 = (cutoff.min(u8::MAX as u32) as u8).max(1);
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let mut wins = 0u32;
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, MullMode::Keep7)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: cutoff_u8,
            on_play: Some(on_play),
            fixed_us_hand: Some(hand.to_vec()),
        };
        let state = run_game(scenario, &mut rng);
        if state.terminal && state.current_turn <= cutoff_u8 {
            wins += 1;
        }
    }
    wins as f64 / games as f64
}

/// Estimate a hand's E[time-to-Doomsday], censored at `horizon` (never-cast counts as `horizon`).
/// LOWER is better. Runs each game cast-ASAP to `horizon` turns and averages the cast turn (or
/// `horizon` if it never casts). This is the label for the E[TTD] mulligan objective — unlike the
/// binary cast-by-T3, it has no cliff (a reliable T4 cast counts) and rewards reliability.
pub fn run_goldfish_fixed_hand_ttd(
    deck: &[(String, i32, String)],
    hand: &[String],
    horizon: u32,
    games: u32,
    on_play: bool,
) -> f64 {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> =
        vec![("Island".to_string(), 60, "main".to_string())];
    let horizon_u8 = (horizon.min(u8::MAX as u32) as u8).max(1);
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let mut sum_ttd = 0.0f64;
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(horizon, MullMode::Keep7)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: horizon_u8,
            on_play: Some(on_play),
            fixed_us_hand: Some(hand.to_vec()),
        };
        let state = run_game(scenario, &mut rng);
        sum_ttd += if state.terminal && state.current_turn <= horizon_u8 {
            state.current_turn as f64
        } else {
            horizon_u8 as f64
        };
    }
    sum_ttd / games as f64
}

/// For a fixed hand: mean (cards-in-hand, protection-in-hand) AT THE MOMENT DD is cast,
/// conditional on casting by `cutoff`. Protection = DEFAULT_PROTECTION. A proxy for the
/// resources/interaction you retain when you actually go off.
pub fn run_goldfish_fixed_hand_resources(
    deck: &[(String, i32, String)],
    hand: &[String],
    cutoff: u32,
    games: u32,
    on_play: bool,
) -> (f64, f64) {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> =
        vec![("Island".to_string(), 60, "main".to_string())];
    let cutoff_u8 = (cutoff.min(u8::MAX as u32) as u8).max(1);
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let (mut sum_cards, mut sum_prot, mut casts) = (0.0f64, 0.0f64, 0u32);
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, MullMode::Keep7)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: cutoff_u8,
            on_play: Some(on_play),
            fixed_us_hand: Some(hand.to_vec()),
        };
        let state = run_game(scenario, &mut rng);
        if state.terminal && state.current_turn <= cutoff_u8 {
            casts += 1;
            sum_cards += state.hand_of(PlayerId::Us).count() as f64;
            sum_prot += state
                .hand_of(PlayerId::Us)
                .filter(|c| DEFAULT_PROTECTION.contains(&c.catalog_key.as_str()))
                .count() as f64;
        }
    }
    if casts == 0 {
        (0.0, 0.0)
    } else {
        (sum_cards / casts as f64, sum_prot / casts as f64)
    }
}

/// Draw `n` random opening 7-card hands from `deck` (for the per-hand explorer).
pub fn deal_opening_hands(deck: &[(String, i32, String)], n: usize) -> Vec<Vec<String>> {
    use rand::seq::SliceRandom;
    let mut pool: Vec<String> = Vec::new();
    for (name, count, _section) in deck {
        for _ in 0..(*count).max(0) {
            pool.push(name.clone());
        }
    }
    let mut rng = rand::rngs::SmallRng::from_entropy();
    (0..n)
        .map(|_| pool.choose_multiple(&mut rng, 7).cloned().collect())
        .collect()
}

/// Every measured stat for one fixed opening hand, in a single sim pass: how often / how fast it
/// casts Doomsday, the same conditioned on bringing interaction, and the resources held at cast.
#[derive(Serialize)]
pub struct HandSimReport {
    pub games: u32,
    /// P(cast DD by `cutoff`).
    pub p_cast: f64,
    /// E[turns-to-Doomsday], censored at `horizon`.
    pub e_ttd: f64,
    /// P(cast DD by `cutoff` AND holding >=1 protection at cast).
    pub p_cast_intr: f64,
    /// E[turns-to-(Doomsday-with-protection)], censored at `horizon`.
    pub e_ttd_intr: f64,
    /// Mean protection in hand at cast (| cast by `cutoff`).
    pub protection_at_cast: f64,
    /// Mean cards in hand at cast (| cast by `cutoff`).
    pub cards_at_cast: f64,
}

pub fn run_goldfish_fixed_hand_report(
    deck: &[(String, i32, String)],
    hand: &[String],
    cutoff: u32,
    horizon: u32,
    games: u32,
    on_play: bool,
) -> HandSimReport {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> =
        vec![("Island".to_string(), 60, "main".to_string())];
    let horizon_u8 = (horizon.min(u8::MAX as u32) as u8).max(1);
    let cutoff_u8 = (cutoff.min(u8::MAX as u32) as u8).max(1);
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let (mut n_cast, mut n_cast_intr, mut n_castcut) = (0u32, 0u32, 0u32);
    let (mut sum_ttd, mut sum_ttd_intr, mut sum_prot, mut sum_cards) = (0.0f64, 0.0, 0.0, 0.0);
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, MullMode::Keep7)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: horizon_u8,
            on_play: Some(on_play),
            fixed_us_hand: Some(hand.to_vec()),
        };
        let state = run_game(scenario, &mut rng);
        let cast = state.terminal;
        let turn = state.current_turn;
        let prot = if cast {
            state
                .hand_of(PlayerId::Us)
                .filter(|c| DEFAULT_PROTECTION.contains(&c.catalog_key.as_str()))
                .count()
        } else {
            0
        };
        let cast_intr = cast && prot >= 1;
        sum_ttd += if cast { turn as f64 } else { horizon_u8 as f64 };
        sum_ttd_intr += if cast_intr { turn as f64 } else { horizon_u8 as f64 };
        if cast && turn <= cutoff_u8 {
            n_cast += 1;
            n_castcut += 1;
            sum_prot += prot as f64;
            sum_cards += state.hand_of(PlayerId::Us).count() as f64;
            if prot >= 1 {
                n_cast_intr += 1;
            }
        }
    }
    let g = games.max(1) as f64;
    HandSimReport {
        games,
        p_cast: n_cast as f64 / g,
        e_ttd: sum_ttd / g,
        p_cast_intr: n_cast_intr as f64 / g,
        e_ttd_intr: sum_ttd_intr / g,
        protection_at_cast: if n_castcut > 0 { sum_prot / n_castcut as f64 } else { 0.0 },
        cards_at_cast: if n_castcut > 0 { sum_cards / n_castcut as f64 } else { 0.0 },
    }
}

/// Replay a fixed hand and return play-by-play traces for the first `n_win` winning
/// and `n_loss` losing games (per-turn intent from the decision log + the engine's
/// actual plays from `state.log`), to diagnose why a hand over/under-performs.
pub fn run_goldfish_fixed_hand_trace(
    deck: &[(String, i32, String)],
    hand: &[String],
    cutoff: u32,
    n_win: u32,
    n_loss: u32,
    on_play: bool,
) -> Vec<String> {
    let catalog = build_catalog();
    let opp_deck: Vec<(String, i32, String)> =
        vec![("Island".to_string(), 60, "main".to_string())];
    let cutoff_u8 = (cutoff.min(u8::MAX as u32) as u8).max(1);
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let mut out = Vec::new();
    let (mut got_w, mut got_l) = (0u32, 0u32);
    let mut games = 0u32;
    while (got_w < n_win || got_l < n_loss) && games < 200_000 {
        games += 1;
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(cutoff, MullMode::Keep7)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective::default()),
            max_turns: cutoff_u8,
            on_play: Some(on_play),
            fixed_us_hand: Some(hand.to_vec()),
        };
        let state = run_game(scenario, &mut rng);
        let win = state.terminal && state.current_turn <= cutoff_u8;
        let want = if win { got_w < n_win } else { got_l < n_loss };
        if !want {
            continue;
        }
        if win {
            got_w += 1;
            out.push(format!("════ WIN #{got_w} — cast T{} ════", state.current_turn));
        } else {
            got_l += 1;
            let o = if state.terminal {
                format!("cast T{} (past cutoff)", state.current_turn)
            } else {
                "never cast".to_string()
            };
            out.push(format!("════ LOSS #{got_l} — {o} ════"));
        }
        for l in state.decision_log.iter().filter(|l| l.starts_with("KEPT") || l.starts_with('T')) {
            out.push(format!("  intent: {l}"));
        }
        for l in &state.log {
            out.push(format!("  play:   {l}"));
        }
    }
    out
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
            fixed_us_hand: None,
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
        None, // randomized play/draw (default)
        false, // Doomsday-only
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
            fixed_us_hand: None,
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
            fixed_us_hand: None,
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

/// "vroomsday 8 guys" — the user's Legacy Doomsday + The Fantasticar list
/// (moxfield.com/decks/GQGfAbcEa3qBQAdBgVgl1w), 60-card mainboard. Two payoffs:
/// resolve Doomsday, or pop The Fantasticar off its fourth-noncreature-spell trigger.
pub fn vroomsday_deck() -> Vec<(String, i32, String)> {
    [
        ("Underground Sea", 4),
        ("Polluted Delta", 4),
        ("Verdant Catacombs", 1),
        ("Flooded Strand", 1),
        ("Misty Rainforest", 1),
        ("Scalding Tarn", 1),
        ("Island", 1),
        ("Swamp", 1),
        ("Undercity Sewers", 1),
        ("Cavern of Souls", 1),
        ("Lotus Petal", 4),
        ("Lion's Eye Diamond", 1),
        ("Mishra's Bauble", 4),
        ("Dark Ritual", 4),
        ("Doomsday", 4),
        ("The Fantasticar", 4),
        ("Brainstorm", 4),
        ("Ponder", 4),
        ("Consider", 1),
        ("Edge of Autumn", 1),
        ("Street Wraith", 1),
        ("Force of Will", 4),
        ("Daze", 4),
        ("Thoughtseize", 3),
        ("Thassa's Oracle", 1),
    ]
    .iter()
    .map(|(n, q)| (n.to_string(), *q, "main".to_string()))
    .collect()
}

/// Deterministic two-wincon "send" report (the backward-solver headline). For each
/// of `games` random opening hands (Keep7, on the play unless `on_draw`), ask the
/// solver: can it *guarantee* a send by `cutoff` with no blind draws — via Doomsday
/// (`deterministic_cast_turn`), via The Fantasticar (`car_pop_turn`), or either
/// (`deterministic_send_turn`)? Splits the either-wincon sends by whether the hand
/// also holds ≥1 disruption (`protection`). This measures the deck's redundancy,
/// independent of how well any pilot plays the line out.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SendReport {
    pub games: u32,
    pub cutoff: u8,
    /// Hands that can guarantee Doomsday by `cutoff`.
    pub dd_by: u32,
    /// Hands that can guarantee a car pop by `cutoff`.
    pub car_by: u32,
    /// Hands that can guarantee EITHER (the union — Doomsday or car).
    pub send_by: u32,
    /// Of the `send_by` hands, those also holding ≥1 protection card.
    pub send_protected: u32,
    /// Of the `send_by` hands, those holding 0 protection.
    pub send_naked: u32,
}

impl SendReport {
    pub fn pct(&self, n: u32) -> f64 {
        if self.games == 0 { 0.0 } else { 100.0 * n as f64 / self.games as f64 }
    }
}

/// Run the deterministic send report (see [`SendReport`]).
pub fn deterministic_send_report(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    cutoff: u8,
    on_draw: bool,
) -> SendReport {
    use rand::seq::SliceRandom;
    let catalog = build_catalog();
    // Expand the mainboard to a flat list of card names.
    let names: Vec<String> = deck
        .iter()
        .filter(|(_, _, board)| board == "main")
        .flat_map(|(n, q, _)| std::iter::repeat(n.clone()).take((*q).max(0) as usize))
        .collect();
    let mut rng = rand::rngs::SmallRng::from_entropy();
    let hand_size = if on_draw { 8 } else { 7 }; // on the draw you've seen one extra card by your first main

    let mut r = SendReport { games, cutoff, ..Default::default() };
    let cutoff_u32 = cutoff.max(1) as u32;
    for _ in 0..games {
        let mut shuffled = names.clone();
        shuffled.shuffle(&mut rng);
        let mut s = SimState::new(PlayerState::new("us"), PlayerState::new("opp"));
        s.catalog = catalog.clone();
        for (i, n) in shuffled.iter().enumerate() {
            if i < hand_size {
                s.place_card(PlayerId::Us, n, Zone::Hand { known: false });
            } else {
                s.place_card(PlayerId::Us, n, Zone::Library);
            }
        }
        let dd = recipe::deterministic_cast_turn(&s, PlayerId::Us, cutoff_u32).is_some();
        let car = recipe::car_pop_turn(&s, PlayerId::Us, cutoff_u32).is_some();
        let send = dd || car;
        if dd { r.dd_by += 1; }
        if car { r.car_by += 1; }
        if send {
            r.send_by += 1;
            let prot = s.hand_of(PlayerId::Us)
                .filter(|c| protection.contains(&c.catalog_key.as_str()))
                .count();
            if prot >= 1 { r.send_protected += 1; } else { r.send_naked += 1; }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DEBUG (`cargo test debug_trace_car_game -- --ignored --nocapture`): run ONE
    /// car-enabled game on a fixed hand and dump the decision log + whether the car popped.
    #[test]
    #[ignore]
    fn debug_trace_car_game() {
        // Car-only deck (no Doomsday) so the dig has only the car to aim at.
        let deck: Vec<(String, i32, String)> = [
            ("Underground Sea", 4), ("Polluted Delta", 4), ("Verdant Catacombs", 1),
            ("Flooded Strand", 1), ("Misty Rainforest", 1), ("Scalding Tarn", 1),
            ("Island", 1), ("Swamp", 1), ("Undercity Sewers", 1), ("Cavern of Souls", 1),
            ("Lotus Petal", 6), ("Lion's Eye Diamond", 1), ("Mishra's Bauble", 7),
            ("Dark Ritual", 4), ("The Fantasticar", 4), ("Brainstorm", 4), ("Ponder", 4),
            ("Consider", 1), ("Edge of Autumn", 1), ("Street Wraith", 1),
            ("Force of Will", 4), ("Daze", 4), ("Thoughtseize", 3),
        ].iter().map(|(n, q)| (n.to_string(), *q, "main".to_string())).collect();
        let catalog = build_catalog();
        // The user's example: dig for the car WITHOUT spending the petal/bauble/ritual.
        let hand: Vec<String> = ["Underground Sea", "Underground Sea", "Dark Ritual",
            "Lotus Petal", "Mishra's Bauble", "Ponder", "Ponder"]
            .iter().map(|s| s.to_string()).collect();
        // Engine verdict on this exact opening (before any play): does the solver SEE a
        // deterministic car line? If yes but the game below fails to pop, execution is broken.
        {
            let mut s0 = SimState::new(
                libmtg_engine::PlayerState::new("us"), libmtg_engine::PlayerState::new("opp"));
            s0.catalog = catalog.clone();
            for h in &hand { s0.place_card(PlayerId::Us, h, libmtg_engine::Zone::Hand { known: false }); }
            for (n, q, _) in &deck { for _ in 0..*q { s0.place_card(PlayerId::Us, n, libmtg_engine::Zone::Library); } }
            println!("ENGINE car_pop_turn(cutoff 3) = {:?}", recipe::car_pop_turn(&s0, PlayerId::Us, 3));
            println!("ENGINE deterministic_send_turn = {:?}", recipe::deterministic_send_turn(&s0, PlayerId::Us, 3));
        }
        let mut rng = rand::rngs::SmallRng::from_entropy();
        let scenario = Scenario {
            us_label: "us".into(), opp_label: "opp".into(),
            catalog: catalog.clone(), us_deck: deck.clone(),
            opp_deck: vec![("Island".to_string(), 60, "main".to_string())],
            us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(3, MullMode::Keep7).with_car(true)),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: dd_goldfish_evaluator(),
            objective: Box::new(GoldfishObjective { count_car: true }),
            max_turns: 3, on_play: Some(true), fixed_us_hand: Some(hand.clone()),
        };
        let state = run_game(scenario, &mut rng);
        println!("\nhand: {:?}", hand);
        println!("popped/sent: {}  (terminal turn {})", state.terminal, state.current_turn);
        println!("--- engine play log (actual casts) ---");
        for line in &state.log { println!("  {line}"); }
        println!("--- decision log ---");
        for line in &state.decision_log { println!("{line}"); }
    }

    /// DEBUG (`cargo test debug_twohead_hand -- --ignored --nocapture`): the user's
    /// fuel-rich two-headed hand on the FULL deck (both wincons). It has the mana + fuel +
    /// two Ponders to dig into EITHER payoff, so it should send ~every game, ~50/50
    /// car/Doomsday. Reports the send rate, the car/Doomsday split, and dumps no-send traces.
    #[test]
    #[ignore]
    fn debug_twohead_hand() {
        let deck = vroomsday_deck(); // full deck: 4 Doomsday + 4 Fantasticar
        let catalog = build_catalog();
        let hand: Vec<String> = ["Underground Sea", "Underground Sea", "Dark Ritual",
            "Lotus Petal", "Mishra's Bauble", "Ponder", "Ponder"]
            .iter().map(|s| s.to_string()).collect();
        let mut rng = rand::rngs::SmallRng::from_entropy();
        let (mut via_car, mut via_dd, mut fail_shown) = (0u32, 0u32, 0u32);
        // Partition by WHICH payoff(s) were found, and the send rate within each — the
        // conversion test: P(send | only car), P(send | only dd), P(send | both) should all be 100%.
        let (mut only_car, mut only_car_sent) = (0u32, 0u32);
        let (mut only_dd, mut only_dd_sent) = (0u32, 0u32);
        let (mut both, mut both_sent) = (0u32, 0u32);
        const N: u32 = 500;
        for _ in 0..N {
            let scenario = Scenario {
                us_label: "us".into(), opp_label: "opp".into(),
                catalog: catalog.clone(), us_deck: deck.clone(),
                opp_deck: vec![("Island".to_string(), 60, "main".to_string())],
                us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(3, MullMode::Keep7).with_car(true)),
                opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
                evaluate_card: dd_goldfish_evaluator(),
                objective: Box::new(GoldfishObjective { count_car: true }),
                max_turns: 3, on_play: Some(true), fixed_us_hand: Some(hand.clone()),
            };
            let state = run_game(scenario, &mut rng);
            let car_via = state.objects.values().any(|o| o.catalog_key == "Fantasticar Construct");
            let cars_lib = state.library_of(PlayerId::Us).filter(|c| c.catalog_key == "The Fantasticar").count();
            let dds_lib = state.library_of(PlayerId::Us).filter(|c| c.catalog_key == "Doomsday").count();
            let car_d = cars_lib < 4;
            let dd_d = dds_lib < 4;
            if state.terminal {
                if car_via { via_car += 1; } else { via_dd += 1; }
            }
            let bucket = match (car_d, dd_d) {
                (true, false) => { only_car += 1; if state.terminal { only_car_sent += 1; } Some("ONLY CAR") }
                (false, true) => { only_dd += 1; if state.terminal { only_dd_sent += 1; } None }
                (true, true) => { both += 1; if state.terminal { both_sent += 1; } Some("BOTH") }
                (false, false) => None,
            };
            // Dump the pure car-conversion failures: only-a-car-found (no DD fallback) that didn't send.
            if bucket == Some("ONLY CAR") && !state.terminal && fail_shown < 8 {
                fail_shown += 1;
                println!("\n===== ONLY CAR FOUND, NO SEND (turn {}) =====", state.current_turn);
                for line in &state.log { println!("  {line}"); }
            }
        }
        let pct = |a: u32, b: u32| 100.0 * a as f64 / b.max(1) as f64;
        println!("\n=== two-headed hand (sea sea ritual petal bauble ponder ponder) — N={N}, full deck, T3 ===");
        println!("P(send | found ONLY car) = {:.1}%  (n={only_car})  <-- pure car conversion, TARGET 100%", pct(only_car_sent, only_car));
        println!("P(send | found ONLY dd)  = {:.1}%  (n={only_dd})", pct(only_dd_sent, only_dd));
        println!("P(send | found BOTH)     = {:.1}%  (n={both})", pct(both_sent, both));
        println!("selection split of sends: via car={via_car}  via doomsday={via_dd}");
    }

    /// DEBUG (`cargo test debug_car_only_decomp -- --ignored --nocapture`): on a CAR-ONLY
    /// deck (Doomsday/Oracle removed, fuel added — every send is a car pop), decompose the
    /// T3 rate into P(car drawn) × P(fire | car drawn) to locate the bottleneck (drawing
    /// the car vs assembling/sequencing the pop), and dump traces of drawn-but-no-pop games.
    #[test]
    #[ignore]
    fn debug_car_only_decomp() {
        let car_only: Vec<(String, i32, String)> = [
            ("Underground Sea", 4), ("Polluted Delta", 4), ("Verdant Catacombs", 1),
            ("Flooded Strand", 1), ("Misty Rainforest", 1), ("Scalding Tarn", 1),
            ("Island", 1), ("Swamp", 1), ("Undercity Sewers", 1), ("Cavern of Souls", 1),
            ("Lotus Petal", 6), ("Lion's Eye Diamond", 1), ("Mishra's Bauble", 7),
            ("Dark Ritual", 4), ("The Fantasticar", 4), ("Brainstorm", 4), ("Ponder", 4),
            ("Consider", 1), ("Edge of Autumn", 1), ("Street Wraith", 1),
            ("Force of Will", 4), ("Daze", 4), ("Thoughtseize", 3),
        ].iter().map(|(n, q)| (n.to_string(), *q, "main".to_string())).collect();
        let catalog = build_catalog();
        let mut rng = rand::rngs::SmallRng::from_entropy();
        let (mut drawn, mut fired, mut shown) = (0u32, 0u32, 0u32);
        let (mut det, mut det_fired) = (0u32, 0u32);
        const N: u32 = 3000;
        for _ in 0..N {
            let scenario = Scenario {
                us_label: "us".into(), opp_label: "opp".into(),
                catalog: catalog.clone(), us_deck: car_only.clone(),
                opp_deck: vec![("Island".to_string(), 60, "main".to_string())],
                us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(3, MullMode::Keep7).with_car(true)),
                opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
                evaluate_card: dd_goldfish_evaluator(),
                objective: Box::new(GoldfishObjective { count_car: true }),
                max_turns: 3, on_play: Some(true), fixed_us_hand: None,
            };
            let state = run_game(scenario, &mut rng);
            let car_fired = state.objects.values().any(|o| o.catalog_key == "Fantasticar Construct");
            let cars_in_lib = state.library_of(PlayerId::Us)
                .filter(|c| c.catalog_key == "The Fantasticar").count();
            if cars_in_lib < 4 { drawn += 1; }
            if car_fired { fired += 1; }
            // Reconstruct the OPENING state (no draws/play yet) for the engine's deterministic
            // verdict at game start — the only fair "was a line THERE" signal (the final-state
            // verdict is meaningless: resources get spent during play). car_pop_turn reads only
            // hand+board, so the empty library is fine.
            let mut s0 = SimState::new(
                libmtg_engine::PlayerState::new("us"), libmtg_engine::PlayerState::new("opp"));
            s0.catalog = catalog.clone();
            for h in &state.opening_hand_us {
                s0.place_card(PlayerId::Us, h, libmtg_engine::Zone::Hand { known: false });
            }
            let opening_line = recipe::car_pop_turn(&s0, PlayerId::Us, 3);
            if opening_line.is_some() {
                det += 1;
                if car_fired { det_fired += 1; }
                if !car_fired && shown < 8 {
                    shown += 1;
                    let open: Vec<&str> = state.opening_hand_us.iter().map(|n| abbrev_g(n)).collect();
                    println!("\n===== ENGINE SAW LINE {:?} BUT NO POP (turn {}) =====", opening_line, state.current_turn);
                    println!("opening: {}", open.join(" "));
                    for line in &state.log { println!("  {line}"); }
                }
            }
        }
        println!("\n=== car-only decomposition (N={N}, Keep7, on play, cutoff T3) ===");
        println!("P(car drawn by T3)         = {:.1}%", 100.0 * drawn as f64 / N as f64);
        println!("P(fire)                    = {:.1}%", 100.0 * fired as f64 / N as f64);
        println!("P(fire | car drawn)        = {:.1}%", 100.0 * fired as f64 / drawn.max(1) as f64);
        println!("P(opening has a det LINE)  = {:.1}%  (n={det})", 100.0 * det as f64 / N as f64);
        println!("P(fire | opening det LINE) = {:.1}%  <-- EXECUTION fidelity", 100.0 * det_fired as f64 / det.max(1) as f64);
    }

    /// DEBUG (`cargo test debug_car_game_logs -- --ignored --nocapture`): run car-enabled
    /// vroomsday games and dump the play-by-play for a few of each outcome (sent via car,
    /// sent via Doomsday, or no send) so we can see how the strategy actually plays.
    #[test]
    #[ignore]
    fn debug_car_game_logs() {
        let deck = vroomsday_deck();
        let catalog = build_catalog();
        let mut rng = rand::rngs::SmallRng::from_entropy();
        let (mut n_car, mut n_dd, mut n_none) = (0, 0, 0);
        const WANT: usize = 3;
        let mut games = 0;
        while (n_car < WANT || n_dd < WANT || n_none < WANT) && games < 5000 {
            games += 1;
            let scenario = Scenario {
                us_label: "us".into(), opp_label: "opp".into(),
                catalog: catalog.clone(), us_deck: deck.clone(),
                opp_deck: vec![("Island".to_string(), 60, "main".to_string())],
                us_strategy: Box::new(DDGoldfishStrategy::with_mull_mode(3, MullMode::Keep7).with_car(true)),
                opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
                evaluate_card: dd_goldfish_evaluator(),
                objective: Box::new(GoldfishObjective { count_car: true }),
                max_turns: 3, on_play: Some(true), fixed_us_hand: None,
            };
            let state = run_game(scenario, &mut rng);
            let via_car = state.objects.values().any(|o| o.catalog_key == "Fantasticar Construct");
            let (label, slot) = if state.terminal && via_car { ("CAR", &mut n_car) }
                else if state.terminal { ("DOOMSDAY", &mut n_dd) }
                else { ("no-send", &mut n_none) };
            if *slot >= WANT { continue; }
            *slot += 1;
            let open: Vec<&str> = state.opening_hand_us.iter().map(|n| abbrev_g(n)).collect();
            println!("\n========= sent via {label} (turn {}) =========", state.current_turn);
            println!("opening: {}", open.join(" "));
            for line in &state.log { println!("  {line}"); }
        }
        println!("\n(scanned {games} games)");
    }

    fn abbrev_g(name: &str) -> &str {
        match name {
            "The Fantasticar" => "car", "Doomsday" => "dd", "Dark Ritual" => "ritual",
            "Lotus Petal" => "petal", "Mishra's Bauble" => "bauble", "Lion's Eye Diamond" => "led",
            "Thoughtseize" => "ts", "Brainstorm" => "bs", "Ponder" => "ponder", "Consider" => "consider",
            "Edge of Autumn" => "edge", "Street Wraith" => "wraith", "Force of Will" => "fow",
            "Daze" => "daze", "Thassa's Oracle" => "oracle", "Underground Sea" => "sea",
            "Island" => "island", "Swamp" => "swamp", "Undercity Sewers" => "sewers", "Cavern of Souls" => "cavern",
            "Polluted Delta" | "Verdant Catacombs" | "Flooded Strand" | "Misty Rainforest" | "Scalding Tarn" => "fetch",
            other => other,
        }
    }

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
    fn vroomsday_deck_is_sixty_and_all_known() {
        let deck = vroomsday_deck();
        let total: i32 = deck.iter().map(|(_, q, _)| q).sum();
        assert_eq!(total, 60, "vroomsday mainboard should be 60 cards");
        let cat = build_catalog();
        for (n, _, _) in &deck {
            assert!(cat.get(n).is_some(), "vroomsday card not in catalog: {n}");
        }
    }

    #[test]
    fn realized_car_pop_sends_without_any_doomsday() {
        // A car-only deck (no Doomsday): the Doomsday-only pilot can NEVER send, while
        // the two-wincon pilot pops The Fantasticar off its free noncreature spells.
        // Exercises the full realized chain: strategy → cast car + free spells → engine
        // 4th-spell trigger → TokenCreated → objective.
        let deck: Vec<(String, i32, String)> = [
            ("The Fantasticar", 8), ("Lotus Petal", 12), ("Mishra's Bauble", 12),
            ("Lion's Eye Diamond", 4), ("Underground Sea", 16), ("Dark Ritual", 8),
        ]
        .iter()
        .map(|(n, q)| (n.to_string(), *q, "main".to_string()))
        .collect();
        let dd = run_goldfish_send(&deck, 300, DEFAULT_PROTECTION, 4, MullMode::Keep7, Some(true), false);
        let car = run_goldfish_send(&deck, 300, DEFAULT_PROTECTION, 4, MullMode::Keep7, Some(true), true);
        assert_eq!(dd.successes(), 0, "no Doomsday in the deck → DD-only pilot can't send");
        assert!(car.successes() > 0, "two-wincon pilot should pop the car at least sometimes");
    }

    #[test]
    fn car_enabled_never_lowers_the_send_rate() {
        // On the real list, the car is a SECOND wincon — enabling it can only add sends.
        let deck = vroomsday_deck();
        let dd = run_goldfish_send(&deck, 400, DEFAULT_PROTECTION, 3, MullMode::Realistic, Some(true), false);
        let car = run_goldfish_send(&deck, 400, DEFAULT_PROTECTION, 3, MullMode::Realistic, Some(true), true);
        assert!(
            car.cast_by(3) + 0.05 >= dd.cast_by(3),
            "car-enabled send {:.3} should be >= DD-only {:.3}",
            car.cast_by(3), dd.cast_by(3)
        );
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

#[cfg(test)]
mod learned_explorer_smoke {
    use super::*;
    fn tempo_deck() -> Vec<(String, i32, String)> {
        [("Underground Sea",4),("Polluted Delta",4),("Misty Rainforest",1),("Scalding Tarn",1),
         ("Flooded Strand",1),("Bloodstained Mire",1),("Undercity Sewers",1),("Cavern of Souls",1),
         ("Island",1),("Swamp",1),("Wasteland",3),("Lotus Petal",2),("Lion's Eye Diamond",1),
         ("Dark Ritual",4),("Doomsday",4),("Thassa's Oracle",1),("Jace, Wielder of Mysteries",1),
         ("Brainstorm",4),("Ponder",3),("Consider",1),("Flow State",4),("Edge of Autumn",1),
         ("Street Wraith",1),("Tamiyo, Inquisitive Student",4),("Murktide Regent",2),
         ("Force of Will",4),("Daze",2),("Thoughtseize",2)]
            .iter().map(|(n,c)| (n.to_string(), *c, "main".to_string())).collect()
    }
    #[test]
    fn report_and_estimates_smoke() {
        let deck = tempo_deck();
        let hands = deal_opening_hands(&deck, 4);
        assert_eq!(hands.len(), 4);
        assert!(hands.iter().all(|h| h.len() == 7));
        let hand: Vec<String> = ["Doomsday","Dark Ritual","Underground Sea","Brainstorm","Ponder","Force of Will","Daze"]
            .iter().map(|s| s.to_string()).collect();
        let refs: Vec<&str> = hand.iter().map(|s| s.as_str()).collect();
        let est = learned_mull::hand_estimates(&refs, true);
        let rep = run_goldfish_fixed_hand_report(&deck, &hand, 3, 10, 200, true);
        let sug = learned_mull::keep_suggestion(&refs, true, 6);
        println!("EST p_cast={:.3} e_ttd={:.2} R={:.2} resolve={:.2}",
            est.p_cast, est.e_ttd, est.resources, est.resolve);
        println!("KEEP6 speed: keeps={} bottom={:?} | intr: keeps={} bottom={:?}",
            sug.speed.keeps, sug.speed.bottom, sug.interactive.keeps, sug.interactive.bottom);
        println!("SIM p_cast={:.3} e_ttd={:.2} p_intr={:.3} e_ttd_intr={:.2} prot@cast={:.2} cards@cast={:.2}",
            rep.p_cast, rep.e_ttd, rep.p_cast_intr, rep.e_ttd_intr, rep.protection_at_cast, rep.cards_at_cast);
        assert!(est.p_cast > 0.3 && est.p_cast < 1.2, "p_cast estimate sane");
        assert!(rep.p_cast > 0.3, "DD+ritual+source casts often");
        assert!(rep.e_ttd >= 1.0 && rep.e_ttd <= 10.0, "ttd in horizon");
        assert!(rep.p_cast_intr <= rep.p_cast + 1e-9, "intr is a subset of cast");
        assert_eq!(sug.speed.keep.len(), 6, "keep_size respected");
        assert_eq!(sug.speed.bottom.len(), 1, "one bottomed at keep-6");
    }
}
