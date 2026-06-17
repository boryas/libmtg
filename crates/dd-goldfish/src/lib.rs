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

use doomsday::{dd_card_evaluator, DoomsdayStrategy, MatchupInfo};
use mtg_engine::{
    build_catalog, run_game, AlwaysPass, GameEvent, Objective, PlayerId, Scenario, SimState,
};
use rand::SeedableRng;
use serde::Serialize;

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

/// Run `games` goldfish simulations of `deck` (a `[(name, qty, board)]` list) and
/// aggregate the cast-turn + protection distributions. The opponent is inert
/// (`AlwaysPass`); the Doomsday player uses the baseline `DoomsdayStrategy`.
pub fn run_goldfish(
    deck: &[(String, i32, String)],
    games: u32,
    protection: &[&str],
    max_turns: u8,
) -> GoldfishStats {
    let catalog = build_catalog();
    let evaluator = dd_card_evaluator(MatchupInfo::default());
    // Inert opponent: enough basics that it never decks within the turn cap.
    let opp_deck: Vec<(String, i32, String)> = vec![("Island".to_string(), 60, "main".to_string())];
    let mut rng = rand::rngs::SmallRng::from_entropy();

    let mut stats = GoldfishStats {
        games,
        ..Default::default()
    };
    for _ in 0..games {
        let scenario = Scenario {
            us_label: "doomsday".to_string(),
            opp_label: "goldfish".to_string(),
            catalog: catalog.clone(),
            us_deck: deck.to_vec(),
            opp_deck: opp_deck.clone(),
            us_strategy: Box::new(DoomsdayStrategy::new(MatchupInfo::default())),
            opp_strategy: Box::new(AlwaysPass::new(PlayerId::Opp)),
            evaluate_card: Arc::clone(&evaluator),
            objective: Box::new(GoldfishObjective::default()),
            max_turns,
            on_play: None,
        };
        let state = run_game(scenario, &mut rng);
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
    }
    stats
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
}
