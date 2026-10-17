//! The motion mode in an entity's coverage signature (`ai_core::coverage`).

use ai_core::AiAppExt;
use bevy::prelude::*;

use crate::controller::MotionState;

pub(crate) fn plugin(app: &mut App) {
    app.add_coverage_part("motion", |world, entity| {
        world.get::<MotionState>(entity).map(|m| format!("{:?}", m.mode).to_lowercase())
    });
}
