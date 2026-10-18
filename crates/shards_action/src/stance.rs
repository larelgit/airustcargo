//! What a running action asks of the body besides its displacement: a turning speed of its own
//! (`ActionDef::turn_speed_deg`) and a stamina regeneration hold (`ActionDef::regen_hold`).
//!
//! Both are content options of the action, off unless it names them. They live in their own
//! systems, apart from the machine and the fighter resources, so the rules read in one place.

use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_motion::MoveModifiers;

use crate::def::ActionDef;
use crate::fighter::{Dead, Stamina};
use crate::machine::ActionState;

/// `ModifierSystems::Contribute`, after the statuses: a body whose running action names a
/// `turn_speed_deg` turns at that rate meanwhile (`MoveModifiers::turn_at`). The action is the one
/// the last tick left, since the modifiers are reset and filled before the machine runs; the
/// turn rate lags an action's start by one tick.
pub fn actions_to_modifiers<F: QueryFilter + 'static>(
    db: Res<ContentDb>,
    mut fighters: Query<(&ActionState, &mut MoveModifiers), F>,
) {
    for (state, mut modifiers) in &mut fighters {
        let Some(run) = &state.current else { continue };
        if let Some(rate) = db.get::<ActionDef>(&run.action).and_then(|d| d.turn_speed_deg) {
            modifiers.turn_at(rate);
        }
    }
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
