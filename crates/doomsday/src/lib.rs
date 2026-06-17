//! Doomsday content crate: the concrete Doomsday + generic-opponent strategies,
//! the DD turn planner, the Doomsday-resolved objective, and the dd-pilegen
//! scenario drivers (`simulate_game` / `generate_scenario`). All of this is
//! *content* built on top of the generic `mtg-engine` (which owns the `Strategy`
//! trait, the `Objective` trait, the IR, and the `run_game` loop).
//!
//! See the `project_engine_app_split` memory for the full split rationale.

use std::collections::HashMap;
use std::sync::Arc;

use mtg_engine::{
    run_game, CardDef, Color, ObjId, PlayerId, Scenario, SimState,
};

mod planner;
mod strategy;
mod objective;

pub use objective::DoomsdayResolvedObjective;
pub use strategy::{DoomsdayStrategy, GenericOppStrategy, MatchupInfo};

#[cfg(test)]
mod tests;

/// dd-pilegen driver: build the Doomsday `Scenario` (DD strategy + generic
/// opponent, matchup-parameterized card evaluator, Doomsday-resolved objective)
/// and run it on the generic engine loop.
pub fn simulate_game(
    deck_name: &str,
    opponent: &str,
    catalog: &HashMap<String, CardDef>,
    all_cards: &[(String, i32, String)],
    opp_cards: &[(String, i32, String)],
    rng: &mut impl rand::Rng,
) -> SimState {
    // Derive matchup info from opponent identity.
    let opp_is_blue = matches!(opponent, "Izzet Delver" | "UB Tempo" | "UR Delver");
    let dd_matchup = MatchupInfo {
        opp_has_counters: opp_is_blue,
        opp_fast_clock: opp_is_blue,
        fetch_colors: vec![Color::Blue, Color::Black],
    };
    let opp_matchup = MatchupInfo {
        opp_has_counters: true,  // DD plays FoW/Daze
        opp_fast_clock: false,   // DD is combo, not aggro
        fetch_colors: vec![Color::Blue, Color::Black], // TODO: derive from opponent deck
    };

    // Universal card evaluator callback (captures matchup info).
    let eval_dd_matchup = dd_matchup.clone();
    let eval_opp_matchup = opp_matchup.clone();
    let evaluate_card: Arc<dyn Fn(PlayerId, ObjId, &SimState) -> f64 + Send + Sync> =
        Arc::new(move |who, card_id, state| match who {
            PlayerId::Us => {
                let gap = strategy::dd_plan_gap(state, who, &eval_dd_matchup);
                strategy::dd_card_fills(card_id, &gap, state, who)
            }
            PlayerId::Opp => {
                let gap = strategy::opp_plan_gap(state, who, &eval_opp_matchup);
                strategy::opp_card_fills(card_id, &gap, state, who)
            }
        });

    run_game(
        Scenario {
            us_label: deck_name.to_string(),
            opp_label: opponent.to_string(),
            catalog: catalog.clone(),
            us_deck: all_cards.to_vec(),
            opp_deck: opp_cards.to_vec(),
            us_strategy: Box::new(DoomsdayStrategy::new(dd_matchup)),
            opp_strategy: Box::new(GenericOppStrategy::new(opp_matchup)),
            evaluate_card,
            objective: Box::new(DoomsdayResolvedObjective::default()),
            max_turns: 10,
            on_play: None,
        },
        rng,
    )
}

/// Generate a Doomsday scenario by simulating until our Doomsday resolves,
/// retrying losing/no-cast runs.
pub fn generate_scenario(
    deck_name: &str,
    opp_display: &str,
    catalog: &HashMap<String, CardDef>,
    all_cards: &[(String, i32, String)],
    opp_cards: &[(String, i32, String)],
) -> SimState {
    let mut rng = rand::thread_rng();
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let state =
            simulate_game(deck_name, opp_display, catalog, all_cards, opp_cards, &mut rng);
        if state.terminal {
            if attempts > 1 {
                eprintln!("  (generated after {} attempts)", attempts);
            }
            // All cards are already in their correct zones in state.objects.
            // Hand cards were moved to Hand zone by sim_draw during opening hand deal.
            return state;
        }
        let reason = if state.winner == Some(PlayerId::Opp) {
            format!("died on turn {}", state.current_turn)
        } else {
            format!("did not cast DD by turn {}", state.current_turn)
        };
        eprintln!("  attempt {} — retry ({})", attempts, reason);
    }
}
