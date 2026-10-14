//! What the AI sees of this game beyond what the engine shows. The engine's modules
//! describe characters (health, stamina, poise, actions, motion, an NPC's state and token),
//! journal fights, movement and NPC decisions, and give agents their tools (`move_to`,
//! `attack`, `dodge`, `block`, `glide`, `path_to`, `npc_state`, `regen`, …); the sandbox adds
//! its boss's phase, the goal pad, and stop conditions.

use shards::action::ActionDef;
use shards::prelude::*;

use crate::gameplay::fate::{PhaseChanged, ReachedGoal};
use crate::gameplay::{Boss, Player};

/// How many NPCs swing an attack (an action with hits, from its windup to the end of its
/// recovery) right now.
pub fn npcs_attacking(world: &World) -> usize {
    let (Some(db), Some(mut q)) = (world.get_resource::<ContentDb>(), world.try_query_filtered::<&ActionState, With<Npc>>()) else {
        return 0;
    };
    q.iter(world)
        .filter(|state| state.current.as_ref().is_some_and(|r| db.get::<ActionDef>(&r.action).is_some_and(|d| !d.hits.is_empty())))
        .count()
}

pub struct GameAiPlugin;

impl Plugin for GameAiPlugin {
    fn build(&self, app: &mut App) {
        app.register_ai_component::<Boss>(|b| format!("phase {}", b.phase))
            .journal_event::<PhaseChanged>(|e, w| Some(format!("{} phase {}->{}", label(w, e.boss), e.from, e.to)))
            .journal_event::<ReachedGoal>(|e, w| Some(format!("{} reached goal", label(w, e.entity))))
            .add_ai_condition("boss_phase_2", |w| {
                w.try_query::<&Boss>().is_some_and(|mut q| q.iter(w).any(|b| b.phase >= 2))
            })
            .add_ai_condition("boss_dead", |w| {
                w.try_query_filtered::<(), (With<Boss>, With<Dead>)>().is_some_and(|mut q| q.iter(w).next().is_some())
            })
            .add_ai_condition("player_dead", |w| {
                w.try_query_filtered::<(), (With<Player>, With<Dead>)>().is_some_and(|mut q| q.iter(w).next().is_some())
            })
            // The director's promise, checked from the outside: with one player, never three
            // monsters swinging at once (`step --until named:three_attacking` must not stop).
            .add_ai_condition("three_attacking", |w| npcs_attacking(w) >= 3);
    }
}
