//! What the AI sees of this game beyond what the engine shows. The engine's modules
//! describe characters (health, stamina, poise, actions, motion, an NPC's state and token),
//! journal fights, movement and NPC decisions, and give agents their tools (`move_to`,
//! `attack`, `dodge`, `block`, `glide`, `path_to`, `npc_state`, `regen`, …); the sandbox adds
//! its boss's phase, the goal pad, stop conditions, and the invariants it must keep (health
//! in range, bodies inside the world).

use shards::action::ActionDef;
use shards::prelude::*;

use crate::gameplay::fate::{PhaseChanged, ReachedGoal};
use crate::gameplay::layout::BuiltLayouts;
use crate::gameplay::{Boss, BoundsDef, Player};

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
            .add_ai_condition("three_attacking", |w| npcs_attacking(w) >= 3)
            .add_ai_invariant("health_in_range", health_in_range)
            .add_ai_invariant("bodies_in_world", bodies_in_world);
    }
}

/// The lowest id first, and how many more: `e6 … (+1 more)`.
fn first_of(mut broken: Vec<(AiId, String)>) -> Option<String> {
    broken.sort_by_key(|(id, _)| *id);
    let more = broken.len().saturating_sub(1);
    let (id, what) = broken.into_iter().next()?;
    Some(if more > 0 { format!("{id} {what} (+{more} more)") } else { format!("{id} {what}") })
}

/// Health stays within `0..=max`: hits stop at 0 and healing at the max, so anything else
/// is a bug (or a dev `set`).
pub fn health_in_range(world: &World) -> Option<String> {
    let mut q = world.try_query::<(&AiId, &Health)>()?;
    let broken = q
        .iter(world)
        .filter(|(_, h)| !(0.0..=h.max).contains(&h.current))
        .map(|(id, h)| (*id, format!("Health.current = {} outside 0..={}", h.current, h.max)))
        .collect();
    first_of(broken)
}

/// Every character stays over the world ([`BoundsDef`] `world`): what the built layouts
/// (the arena, its corridor, the obstacle course) and the loaded region cover on the
/// ground, `margin` wider, and above `fall_below`. Movement and collisions keep it there.
pub fn bodies_in_world(world: &World) -> Option<String> {
    let bounds = world.get_resource::<ContentDb>()?.get::<BoundsDef>("world")?;
    let layouts = world.get_resource::<BuiltLayouts>().map(|b| &b.0[..]).unwrap_or_default();
    let mut areas: Vec<Rect> = layouts.iter().filter_map(|(_, def)| def.footprint()).collect();
    if let Some(region) = world.get_resource::<ActiveRegion>() {
        let m = &region.manifest;
        let corner = m.corner().xz();
        areas.push(Rect::from_corners(corner, corner + Vec2::new(m.size[0] as f32, m.size[1] as f32) * m.tile_size));
    }
    let areas: Vec<Rect> = areas.into_iter().map(|a| a.inflate(bounds.margin)).collect();
    let mut q = world.try_query_filtered::<(&AiId, &Transform), With<CharacterController>>()?;
    let broken = q
        .iter(world)
        .filter_map(|(id, tf)| {
            let p = tf.translation;
            let out = if p.y < bounds.fall_below {
                format!("fell out of the world: y = {:.1}", p.y)
            } else if !areas.iter().any(|a| a.contains(p.xz())) {
                format!("outside the world: x = {:.1}, z = {:.1}", p.x, p.z)
            } else {
                return None;
            };
            Some((*id, out))
        })
        .collect();
    first_of(broken)
}
