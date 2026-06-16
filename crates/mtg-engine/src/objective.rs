//! Simulation objectives — when does a run end, and what does it capture.
//!
//! An [`Objective`] observes the game's event stream (consulted by
//! [`crate::fire_event`] for every event that actually happens, post-prohibition
//! and post-replacement) and decides when the simulation terminates. This
//! replaces the former `Action::EndSimulation` / `SimState.success` sentinel —
//! a fake "Doomsday" card whose only job was to end the sim. Termination is an
//! *application* concern, not an engine effect, so it lives here.
//!
//! The Doomsday applications terminate on Doomsday *resolving*. A future
//! "proper" full game would terminate on a `GameOver` event through the very
//! same primitive, with no special-casing.

use crate::{GameEvent, PlayerId, SimState};

/// Decides simulation termination off the event stream. `observe` is called for
/// every event that fires; returning `true` ends the simulation. Implementors
/// may mutate state to record app-specific capture (e.g. pre-resolution life).
pub(crate) trait Objective {
    fn observe(&mut self, event: &GameEvent, state: &mut SimState) -> bool;
}

/// dd-pilegen objective: the run ends when our Doomsday resolves. Captures
/// pre-resolution life for the "X → Y" display and applies Doomsday's
/// lose-half-life accounting. In this engine Doomsday's pile-building is
/// deferred to the human (via the web UI), so resolution is the stopping point —
/// the card body itself is a no-op and this objective owns the terminal logic.
#[derive(Default)]
pub(crate) struct DoomsdayResolvedObjective;

impl Objective for DoomsdayResolvedObjective {
    fn observe(&mut self, event: &GameEvent, state: &mut SimState) -> bool {
        if let GameEvent::SpellResolved { card_id, controller } = event {
            if *controller == PlayerId::Us
                && state.objects.get(card_id).map_or(false, |o| o.catalog_key == "Doomsday")
            {
                let life = state.player(PlayerId::Us).life;
                state.life_before_dd = Some(life);
                state.player_mut(PlayerId::Us).life = life / 2;
                return true;
            }
        }
        false
    }
}
