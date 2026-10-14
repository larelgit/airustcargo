//! What the AI sees of this game beyond what the engine shows. The engine's modules
//! describe characters (health, stamina, poise, actions, motion), journal fights and
//! movement, and give agents their tools (`move_to`, `attack`, `dodge`, `block`, `glide`,
//! `path_to`, …); the sandbox adds its boss's phase, the goal pad, and stop conditions.

use shards::prelude::*;

use crate::gameplay::fate::{PhaseChanged, ReachedGoal};
use crate::gameplay::{Boss, Player};

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
            });
    }
}
