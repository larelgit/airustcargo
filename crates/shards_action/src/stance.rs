//! What a running action asks of the body besides its displacement: a turning speed of its own
//! (`ActionDef::turn_speed_deg`) and a stamina regeneration hold (`ActionDef::regen_hold`).
//!
//! Both are content options of the action, off unless it names them. They live in their own
//! systems, apart from the machine and the fighter resources, so the rules read in one place.

use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::Predicted;
use shards_motion::MoveModifiers;

use crate::def::ActionDef;
use crate::fighter::{Dead, Stamina};
use crate::machine::{ActionState, Running};
use crate::predict::PredictedAction;

/// `ModifierSystems::Contribute`, after the statuses: a body whose running action names a
/// `turn_speed_deg` turns at that rate meanwhile (`MoveModifiers::turn_at`). The action is the one
/// the last tick left, since the modifiers are reset and filled before the machine runs; the
/// turn rate lags an action's start by one tick.
///
/// It is a rate of the **body** (the controller's turn toward `LocomotionInput::face`), never of
/// the run's own facing: an action's `turn` spans turn that, in the machine, and the body then
/// turns toward it once a tick at this rate. A client's own body reads only the action it shows
/// ([`PredictedAction`]; none while it shows none), as the prediction moves it.
#[allow(clippy::type_complexity, reason = "a Bevy query")]
pub fn actions_to_modifiers<F: QueryFilter + 'static>(
    db: Res<ContentDb>,
    mut fighters: Query<(&ActionState, Option<&PredictedAction>, Has<Predicted>, &mut MoveModifiers), F>,
) {
    for (state, shown, predicted, mut modifiers) in &mut fighters {
        let Some(run) = shown_run(state, shown, predicted) else { continue };
        if let Some(rate) = db.get::<ActionDef>(&run.action).and_then(|d| d.turn_speed_deg) {
            modifiers.turn_at(rate);
        }
    }
}

/// The run a body turns by: a client's predicted body, the action it shows, not the one the host
/// last replicated (a round trip behind), and none while it shows none (the prediction moves it
/// free then, see [`shown_state`](crate::shown_state)); any other body, its own state's.
fn shown_run<'a>(state: &'a ActionState, shown: Option<&'a PredictedAction>, predicted: bool) -> Option<&'a Running> {
    if predicted { shown.map(|p| &p.run) } else { state.current.as_ref() }
}

/// `ActionSystems::Fighters`, before the resources update: while a fighter's action has a
/// `regen_hold`, its stamina waits at least that long before it regenerates, so a raised guard
/// holds it back for as long as it is up and for the hold after it is let go.
pub fn hold_regen_during_actions(
    db: Res<ContentDb>,
    mut fighters: Query<(&ActionState, &mut Stamina, Has<Dead>)>,
) {
    for (state, mut stamina, dead) in &mut fighters {
        let Some(run) = &state.current else { continue };
        if dead {
            continue;
        }
        if let Some(hold) = db.get::<ActionDef>(&run.action).and_then(|d| d.regen_hold)
            && stamina.since_use > stamina.delay - hold
        {
            stamina.hold_regen(hold);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_of(action: &str, instance: u32) -> Running {
        let def: ActionDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(r#"(net_id: 1, phases: [("a", 0.4)], turn_speed_deg: 90)"#)
            .unwrap();
        Running::new(action, &def, instance, Vec3::NEG_Z, None, Some("block".into()))
    }

    #[test]
    fn a_predicted_body_turns_by_the_action_it_shows_and_by_none_while_it_shows_none() {
        // The host's replicated state still runs the guard, a round trip behind.
        let host = ActionState { current: Some(run_of("guard", 3)), started: 4 };
        let shown = PredictedAction {
            action: "roll".into(),
            input: Some("dodge".into()),
            expect_instance: 5,
            start_tick: 10,
            run: run_of("roll", 4),
            confirmed: false,
            extras: None,
        };
        assert_eq!(shown_run(&host, Some(&shown), true).map(|r| r.action.as_str()), Some("roll"), "the action it shows");
        assert_eq!(shown_run(&host, None, true), None, "shows none: moves free, turns free");
        assert_eq!(shown_run(&host, None, false).map(|r| r.action.as_str()), Some("guard"), "the host's own body");
        assert_eq!(shown_run(&host, Some(&shown), false).map(|r| r.action.as_str()), Some("guard"), "a stale one is not read");
    }
}
