//! Ranged fighters (F11): an attack plan that keeps a distance band (`keep_range`) and fires only along
//! a clear line.
//!
//! - **The band** ([`keep_band`]): an NPC whose plan has `keep_range: (min, max)` backs off inside
//!   `min`, closes in beyond `max` (to three quarters of the way out from `min`), and inside the band
//!   holds its ground, or moves in its plan's `wait` role (a strafe, a circle) at the role's distance
//!   held inside the band. The director's waiting step and the planner both steer it so: it takes no
//!   slot of the director's `waiting` list.
//! - **The line of fire** ([`NpcFireLine`], [`fire_lines`]): every tick, after the brains decide and
//!   before the director grants, each active NPC with such a plan traces the ray a shot of its would
//!   fly to its target (its grant's, else its brain's): from its eye to the target's aim point,
//!   through the world as the tick found it ([`line_of_fire`]). An ally or a wall first refuses its
//!   options that fire a shot (`Refusal::LineOfFire`), so the director grants no token for them, the
//!   planner neither chooses nor presses them, and `npc_schedule` says why (`no line of fire: e9 in
//!   the way`). One trace per NPC per tick; `npc_schedule` reads the one of this tick.
//! - **Accuracy** is the shot's: `FireDef::aim_error_deg`, worse while the shooter moves
//!   (`aim_error_moving`) or far from its aim (`aim_error_far`). **Bursts** are actions: an action
//!   with several `Fire` effects is one press, so the plan, the token and the line of fire judge the
//!   burst, not each shot.

use ai_core::{AiId, AiIndex};
use bevy::prelude::*;
use shards_action::{line_of_fire, Dead, Downed, FireLine};
use shards_content::ContentDb;
use shards_core::flat;
use shards_motion::{aim_point_of, eye_of};

use crate::brain::{clamp_to_leash, npcs_in_order, Npc, NpcBrain};
use crate::def::{AttackPlanDef, WaitRole};
use crate::director::{wait_in_role, AttackToken, FightingItem};
use crate::planner::reach;

/// What stands in an NPC's line of fire ([`NpcFireLine::blocked`]).
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum LineBlock {
    /// A living body of its side: firing would go through a friend.
    Ally(AiId),
    /// A wall (a `WORLD` collider that is no body).
    Wall,
}

impl LineBlock {
    /// The ally in the way, if it is one.
    pub fn ally(&self) -> Option<AiId> {
        match self {
            LineBlock::Ally(id) => Some(*id),
            LineBlock::Wall => None,
        }
    }

    /// `e9 in the way`, `a wall in the way`.
    pub fn text(&self) -> String {
        match self {
            LineBlock::Ally(id) => format!("{id} in the way"),
            LineBlock::Wall => "a wall in the way".to_owned(),
        }
    }
}

/// The line of fire of an NPC whose plan keeps a range (F11), traced this tick ([`fire_lines`]): its
/// target, and what stands between it and the target's aim point. Gameplay state: kept in replays and
/// snapshots. Only NPCs whose attack plan has `keep_range` carry it.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcFireLine {
    /// The target it was traced to; `None`: it has none.
    pub target: Option<AiId>,
    /// What stands in the way; `None`: the line is clear.
    pub blocked: Option<LineBlock>,
}

impl NpcFireLine {
    /// What blocks the line to `target`, when it was traced to that target.
    pub fn blocking(&self, target: AiId) -> Option<LineBlock> {
        self.blocked.filter(|_| self.target == Some(target))
    }
}

/// The line of fire of NPC `npc` to `target` now: from its eye to the target's aim point, what a shot of
/// its would meet first ([`line_of_fire`]). `None`: the line is clear (or either is gone).
pub fn fire_line_to(world: &World, npc: Entity, target: Entity) -> Option<LineBlock> {
    let from = eye_of(world, npc).or_else(|| world.get::<Transform>(npc).map(|t| t.translation))?;
    let to = aim_point_of(world, target)?;
    let delta = to - from;
    match line_of_fire(world, npc, from, delta, delta.length() + 0.5, target) {
        FireLine::Ally { body, .. } => world.get::<AiId>(body).map(|id| LineBlock::Ally(*id)),
        FireLine::Wall { .. } => Some(LineBlock::Wall),
        FireLine::Target { .. } | FireLine::Nothing => None,
    }
}

/// `NpcSystems::Decide`, after the brains: every active NPC whose attack plan keeps a range, in `AiId`
/// order, traces its line of fire to its target (its grant's, else its brain's) into its
/// [`NpcFireLine`]; the others carry none. The plan is read only for an active NPC with one, as the
/// brains read it.
pub(crate) fn fire_lines(world: &mut World) {
    let lines: Vec<(Entity, Option<NpcFireLine>)> = {
        let (Some(db), Some(index)) = (world.get_resource::<ContentDb>(), world.get_resource::<AiIndex>()) else { return };
        npcs_in_order(world)
            .into_iter()
            .filter_map(|(_, entity)| {
                let had = world.get::<NpcFireLine>(entity).is_some();
                let brain = world.get::<NpcBrain>(entity)?;
                let plan = world.get::<Npc>(entity)?.plan.as_deref().filter(|_| brain.active);
                let ranged = plan.and_then(|p| db.get::<AttackPlanDef>(p)).is_some_and(|p| p.keep_range.is_some());
                if !ranged {
                    return had.then_some((entity, None));
                }
                let target = world.get::<AttackToken>(entity).and_then(|t| t.held.as_ref()).map(|g| g.target).or(brain.target);
                let body = target
                    .and_then(|id| index.get(id))
                    .filter(|e| world.get::<Dead>(*e).is_none() && world.get::<Downed>(*e).is_none());
                let blocked = body.and_then(|e| fire_line_to(world, entity, e));
                Some((entity, Some(NpcFireLine { target, blocked })))
            })
            .collect()
    };
    for (entity, line) in lines {
        let Ok(mut e) = world.get_entity_mut(entity) else { continue };
        match line {
            Some(line) => match e.get_mut::<NpcFireLine>() {
                Some(mut now) => {
                    if *now != line {
                        *now = line;
                    }
                }
                None => {
                    e.insert(line);
                }
            },
            None => {
                e.remove::<NpcFireLine>();
            }
        }
    }
}

/// Steers a ranged NPC by its band `(min, max)` around its target at `center` (see the module docs):
/// back off inside `min`, close in beyond `max`, and inside the band its `wait` role at the role's
/// distance held inside the band, or (with none) stand. It faces the target.
pub(crate) fn keep_band(f: &mut FightingItem, (min, max): (f32, f32), role: Option<&WaitRole>, center: Vec3, home: Option<(Vec3, f32)>, dt: f32) {
    let here = f.transform.translation;
    let bearing = flat(here - center);
    let distance = bearing.length();
    let leashed = f.drive.leashed;
    let leash = |p: Vec3| if leashed { clamp_to_leash(home.map(|h| h.0), home.map_or(0.0, |h| h.1), p) } else { p };
    if distance < min - 1e-4 {
        let away = bearing.try_normalize().unwrap_or(-*f.transform.forward());
        f.drive.goal = Some(leash(center + away * (min + 0.3)));
        f.drive.within = 0.2;
        f.brain.role = None;
    } else if distance > max + 1e-4 {
        let goal = leash(center);
        f.drive.goal = Some(goal);
        f.drive.within = reach((min + (max - min) * 0.75).max(0.3), goal, center);
        f.brain.role = None;
    } else {
        match role {
            Some(role) => {
                let held = WaitRole { distance: role.distance.clamp(min, max), ..role.clone() };
                wait_in_role(f, &held, center, home, dt);
            }
            None => f.drive.goal = None,
        }
    }
    f.drive.face = Some(center);
}

/// `keeping range 8–14m` for an NPC whose plan keeps a range, for tools (reads content with `peek`).
pub fn band_text(db: &ContentDb, npc: &Npc) -> Option<String> {
    let (min, max) = npc.plan.as_deref().and_then(|p| db.peek::<AttackPlanDef>(p))?.keep_range?;
    Some(format!("keeping range {}–{}m", trim(min), trim(max)))
}

/// `8`, `8.5`.
fn trim(v: f32) -> String {
    let text = format!("{v:.1}");
    text.strip_suffix(".0").map_or(text.clone(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_names_the_friend_or_the_wall() {
        assert_eq!(LineBlock::Ally(AiId(9)).text(), "e9 in the way");
        assert_eq!(LineBlock::Wall.text(), "a wall in the way");
        let line = NpcFireLine { target: Some(AiId(5)), blocked: Some(LineBlock::Wall) };
        assert_eq!(line.blocking(AiId(5)), Some(LineBlock::Wall));
        assert_eq!(line.blocking(AiId(6)), None, "traced to another target: says nothing of this one");
        assert_eq!(trim(8.0), "8");
        assert_eq!(trim(8.5), "8.5");
    }
}
