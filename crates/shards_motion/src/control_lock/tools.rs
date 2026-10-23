//! What agents do with the control lock: the dev tools `lock_controls` and `unlock_controls`, and
//! the refusals of the player tools a lock takes away (`move` while movement is locked says why).

use ai_core::prelude::*;
use ai_core::{label, AiId, AiIndex};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_core::Target;

use super::{lock_controls, unlock_controls, ControlLock, ControlLocks, LookLock};

pub(super) fn plugin(app: &mut App) {
    app.add_ai_tool("lock_controls", Scope::Dev, lock_tool).add_ai_tool("unlock_controls", Scope::Dev, unlock_tool);
    // The player's tools act through `Intent`, so the lock already stops them; a guard says so in words.
    for tool in ["move", "move_to", "jump", "sprint", "glide"] {
        app.add_ai_tool_guard(tool, movement_refusal);
    }
    for tool in [
        "attack", "light_attack", "heavy_attack", "block", "parry", "dodge", "fire", "reload", "use_item", "use_technique", "weapon", "interact", "lock_on",
    ] {
        app.add_ai_tool_guard(tool, actions_refusal);
    }
    // A dodge is a movement as well.
    app.add_ai_tool_guard("dodge", movement_refusal);
}

fn refusal(world: &World, caller: &Caller, what: &str, held_by: fn(&ControlLocks) -> bool) -> Result<(), String> {
    let Some(me) = caller.subject else { return Ok(()) };
    let Some(locks) = world.get::<ControlLocks>(me).filter(|l| held_by(l)) else { return Ok(()) };
    let reasons = locks.reasons().collect::<Vec<_>>().join(", ");
    Err(format!("your {what} locked ({reasons}): a scene has the controls; a dev agent can `unlock_controls`"))
}

fn movement_refusal(world: &World, caller: &Caller) -> Result<(), String> {
    refusal(world, caller, "movement is", ControlLocks::movement)
}

fn actions_refusal(world: &World, caller: &Caller) -> Result<(), String> {
    refusal(world, caller, "actions are", ControlLocks::actions)
}

/// Locks a body's controls for a reason: no movement, no actions, and/or a look held within a cone.
/// The player "looks but does not move or act" with `movement=true actions=true`. Several reasons can
/// hold one body; each lets go of its own with `unlock_controls`. Locking again under the same reason
/// replaces its lock.
#[derive(Deserialize, JsonSchema)]
pub struct LockArgs {
    /// Whose controls, e.g. "e1". Default: you.
    pub id: Option<Id>,
    /// Who holds the lock (default "dev").
    pub reason: Option<String>,
    /// Take movement away: walking, jumping, sprinting, gliding, dodging.
    #[serde(default)]
    pub movement: bool,
    /// Take the other inputs away: attacks, shots, reloads, weapon switches, use, lock-on, guard.
    #[serde(default)]
    pub actions: bool,
    /// Hold the look within `deg` degrees of this: a body ("e9") or a point ("x,y,z").
    pub at: Option<String>,
    /// The cone's half-angle in degrees (0 to 180), with `at`.
    pub deg: Option<f32>,
}

fn target_of(world: &World, text: &str) -> Result<Target, String> {
    let text = text.trim();
    if let Some(id) = AiId::parse(text) {
        world.resource::<AiIndex>().resolve(id)?;
        return Ok(Target::Entity(id));
    }
    let numbers: Vec<f32> = text.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    match numbers[..] {
        [x, y, z] if text.split(',').count() == 3 => Ok(Target::Point(Vec3::new(x, y, z))),
        _ => Err(format!("`at` is a body (`e9`) or a point (`x,y,z`), not `{text}`")),
    }
}

fn lock_tool(In(call): In<Call<LockArgs>>, world: &mut World) -> ToolResult {
    let LockArgs { id, reason, movement, actions, at, deg } = call.args;
    let entity = match id {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let look = match (at, deg) {
        (None, None) => LookLock::Free,
        (Some(at), Some(deg)) => LookLock::Cone { at: target_of(world, &at)?, deg },
        (Some(_), None) => return Err("a look cone needs `deg` too (its half-angle in degrees)".into()),
        (None, Some(_)) => return Err("a look cone needs `at` too (a body `e9` or a point `x,y,z`)".into()),
    };
    let lock = ControlLock { movement, actions, look };
    if lock.is_empty() {
        return Err("nothing to lock: give movement=true and/or actions=true and/or at= with deg= (a look cone)".into());
    }
    let reason = reason.unwrap_or_else(|| "dev".to_owned());
    lock_controls(world, entity, &reason, lock)?;
    Ok(format!("ok: {} controls locked ({reason}): {}", label(world, entity), lock.describe(world)))
}

/// Lets go of the lock a reason holds on a body; the other reasons keep theirs.
#[derive(Deserialize, JsonSchema)]
pub struct UnlockArgs {
    /// Whose controls, e.g. "e1". Default: you.
    pub id: Option<Id>,
    /// The reason that locked them (default "dev").
    pub reason: Option<String>,
    /// Let go of every reason at once.
    #[serde(default)]
    pub all: bool,
}

fn unlock_tool(In(call): In<Call<UnlockArgs>>, world: &mut World) -> ToolResult {
    let UnlockArgs { id, reason, all } = call.args;
    let entity = match id {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let held: Vec<String> = world.get::<ControlLocks>(entity).map(|l| l.reasons().map(str::to_owned).collect()).unwrap_or_default();
    let who = label(world, entity);
    if held.is_empty() {
        return Err(format!("{who} has no locked controls"));
    }
    if all {
        for r in &held {
            unlock_controls(world, entity, r);
        }
        return Ok(format!("ok: {who} controls unlocked ({})", held.join(", ")));
    }
    let reason = reason.unwrap_or_else(|| "dev".to_owned());
    if !unlock_controls(world, entity, &reason) {
        return Err(format!("{who} has no lock under reason `{reason}`; it holds: {}", held.join(", ")));
    }
    let left = world.get::<ControlLocks>(entity).map_or(0, |l| l.held.len());
    Ok(if left == 0 { format!("ok: {who} controls unlocked ({reason})") } else { format!("ok: {who} controls unlocked ({reason}); still locked by {left} other") })
}
