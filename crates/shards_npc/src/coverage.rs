//! The behaviour state in an entity's coverage signature (`ai_core::coverage`), and the states
//! content declares for the `coverage` tool to compare with.

use ai_core::AiAppExt;
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::{BehaviorDef, NpcBrain};

pub(crate) fn plugin(app: &mut App) {
    app.add_coverage_part("npc", |world, entity| {
        let brain = world.get::<NpcBrain>(entity)?;
        (!brain.state.is_empty()).then(|| brain.state.clone())
    })
    .add_coverage_catalog(
        "behaviour states",
        "npc",
        |world| {
            // `behavior/state`, so two behaviours that both have `idle` are two states; the part
            // knows only the state, which reaches it in every behaviour that has it.
            world.get_resource::<ContentDb>().map_or_else(Vec::new, |db| {
                db.iter::<BehaviorDef>().flat_map(|(_, def)| def.states.keys().cloned()).collect()
            })
        },
        |value| vec![value.to_owned()],
    );
}
