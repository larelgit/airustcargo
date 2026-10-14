//! What an agent can ask the physics world: `clearance` (how far can I go, and what
//! stops me) and `path_to` (a route around what stops me). Both read the world through
//! [`Terrain`], the same rules the controller walks by, and need no window.
//!
//! The planner is a pure function on `&PhysicsWorld` ([`plan_path`]); `move_to path=true`
//! and the tests call the same code as the `path_to` query. A module that knows more of the
//! world (the navmesh of a loaded region) registers a [`RoutePlanner`], which `move_to
//! path=true` asks first.

mod clearance;
mod path;
mod terrain;

use ai_core::prelude::*;
use ai_core::spatial::fmt_pos;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::AiIndex;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use shards_content::ContentDb;

pub use clearance::{clearance, reach_toward, Hazard, Reach};
pub use path::{hop_allowed, plan_path, step_onto, Leg, PathPlan, PathRequest, Stance, Step, StepKind, CELL, COST_PER_METER};
pub use terrain::{Block, Probe, Terrain};

use crate::ai::MotionAi;
use crate::controller::{CharacterController, CharacterParams, MotionMode, MotionState};
use crate::physics::{fresh, LayerMask, PhysicsWorld};
use crate::tools::TargetArg;

pub(crate) fn plugin(app: &mut App) {
    register_tool(app.world_mut(), "clearance", Scope::Player, ToolKind::Query, fresh::<ClearanceArgs>.pipe(clearance_tool));
    register_tool(app.world_mut(), "path_to", Scope::Player, ToolKind::Query, fresh::<PathToArgs>.pipe(path_to_tool));
}

/// A route planner that knows more of the world than the local grid: the navmesh of a
/// loaded region. `move_to path=true` asks it first and plans on the local grid when it
/// answers `None` (the walk is not in its part of the world, or not for this body).
#[derive(Resource, Clone, Copy)]
pub struct RoutePlanner {
    /// Who answers, for messages: `navmesh`.
    pub name: &'static str,
    pub plan: fn(&World, &RouteRequest) -> Option<Result<PathPlan, String>>,
}

/// What `move_to path=true` asks a [`RoutePlanner`].
pub struct RouteRequest<'a> {
    pub walker: Entity,
    /// The walker's locomotion numbers.
    pub params: &'a CharacterParams,
    /// What the local planner would be asked.
    pub request: &'a PathRequest,
}

/// What `move_to path=true` and `path_to` plan for a walker at `start` toward `target`
/// (`goal_level`: a body standing somewhere, so the floor must be near its feet).
#[allow(clippy::too_many_arguments, reason = "every number of a request, named once")]
pub(crate) fn request_for(
    start: Vec3,
    swimming_at: Option<f32>,
    (goal, goal_level): (Vec3, bool),
    within: Option<f32>,
    max: Option<f32>,
    safe_drop: f32,
    exclude: Entity,
) -> PathRequest {
    let within = within.unwrap_or(if goal_level { 1.5 } else { 0.3 });
    PathRequest {
        start,
        swimming_at,
        goal,
        goal_level,
        within: within.max(0.0),
        max_length: max.unwrap_or(80.0).clamp(1.0, 300.0),
        safe_drop,
        exclude: Some(exclude),
    }
}

/// The parts of a body the perception tools read.
pub(crate) type Bodies<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, Option<&'static CharacterController>, Option<&'static CharacterParams>, Option<&'static MotionState>),
>;

/// The locomotion numbers of `entity` (its controller's, or the code defaults).
pub(crate) fn params_of(bodies: &Bodies, db: Option<&ContentDb>, entity: Entity) -> CharacterParams {
    match bodies.get(entity) {
        Ok((_, Some(controller), own, _)) => controller.resolve(own, db).clone(),
        Ok((_, None, Some(own), _)) => own.clone(),
        _ => CharacterParams::default(),
    }
}

/// The water surface a swimming character floats in.
pub(crate) fn swimming_at(state: Option<&MotionState>) -> Option<f32> {
    state.filter(|s| s.mode == MotionMode::Water).and_then(|s| s.water_surface)
}

/// Both queries measure against colliders; a world without any would answer "cliff" everywhere.
fn no_physics(physics: &PhysicsWorld) -> Result<(), String> {
    let ground = physics.colliders().any(|c| c.volume.is_none() && c.layers.memberships.intersects(LayerMask::WORLD));
    if !ground {
        return Err("this world has no colliders to measure against: give the ground and the walls a `Collider` \
                    (shards_motion), or use `observe` and `nearby`, which work from positions"
            .into());
    }
    Ok(())
}

/// How far you can go in each of the eight compass directions and what stops you there:
/// a wall, a cliff (with how deep it drops), water, or a ledge you could climb. Looks
/// 15 m by default. Read-only.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ClearanceArgs {
    /// Look from this entity's position instead of yours, with your body's size.
    pub around: Option<Id>,
}

fn clearance_tool(
    In(call): In<Call<ClearanceArgs>>,
    index: Res<AiIndex>,
    physics: Res<PhysicsWorld>,
    ai: Res<MotionAi>,
    db: Option<Res<ContentDb>>,
    bodies: Bodies,
) -> ToolResult {
    no_physics(&physics)?;
    let me = call.caller.subject;
    let subject = match call.args.around {
        Some(id) => index.resolve(id.0)?,
        None => call.caller.require_subject()?,
    };
    let (tf, _, _, state) = bodies.get(subject).map_err(|_| "that entity has no position".to_owned())?;
    // Another entity is looked at with the caller's body: its width decides the gaps. Neither
    // the body nor the subject (a crate on the world layer) stands in its own way.
    let body = me.unwrap_or(subject);
    let params = params_of(&bodies, db.as_deref(), body);
    let reaches = clearance(&physics, &params, tf.translation, swimming_at(state), ai.clearance_range, ai.safe_drop, &[body, subject]);
    let line = reaches.iter().map(|r| r.describe()).collect::<Vec<_>>().join(" · ");
    Ok(match call.args.around {
        Some(id) => format!("around {} at {}: {line}", id, fmt_pos(tf.translation)),
        None => line,
    })
}

/// Plans a route over what you can walk, swim and climb, around walls and cliffs, and
/// reports it: `path 23.5m, 4 turns: 6.0m NE → 4.5m N (mantle 1.1m) → 9.0m E (swim)`.
/// Returns the waypoints too. `move_to path=true` follows such a route. Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct PathToArgs {
    /// An entity id or a ground point {x, z}.
    pub target: TargetArg,
    /// Arrive within this many meters of the target. Default 1.5 for an entity, 0.3 for a point.
    pub within: Option<f32>,
    /// Longest route to accept, meters. Default 80.
    pub max: Option<f32>,
}

fn path_to_tool(
    In(call): In<Call<PathToArgs>>,
    index: Res<AiIndex>,
    physics: Res<PhysicsWorld>,
    ai: Res<MotionAi>,
    db: Option<Res<ContentDb>>,
    bodies: Bodies,
) -> ToolResult<Value> {
    let me = call.caller.require_subject()?;
    let plan = plan_for(me, &call.args.target, call.args.within, call.args.max, &index, &physics, &ai, db.as_deref(), &bodies)?;
    Ok(plan.to_json())
}

/// The route the `path_to` and `move_to path=true` tools plan for `me` toward `target`.
#[allow(clippy::too_many_arguments, reason = "everything the two tools read, passed once")]
pub(crate) fn plan_for(
    me: Entity,
    target: &TargetArg,
    within: Option<f32>,
    max: Option<f32>,
    index: &AiIndex,
    physics: &PhysicsWorld,
    ai: &MotionAi,
    db: Option<&ContentDb>,
    bodies: &Bodies,
) -> Result<PathPlan, String> {
    no_physics(physics)?;
    let (tf, _, _, state) = bodies.get(me).map_err(|_| "your body has no position".to_owned())?;
    let (goal, goal_level) = match *target {
        TargetArg::Entity(id) => {
            let entity = index.resolve(id.0)?;
            if entity == me {
                return Err(format!("{id} is you; give another entity or a point {{x, z}}"));
            }
            let at = bodies.get(entity).map(|b| b.0.translation).map_err(|_| format!("{id} has no position to walk to"))?;
            (at, true)
        }
        TargetArg::Point { x, z } => (Vec3::new(x, tf.translation.y, z), false),
    };
    let request = request_for(tf.translation, swimming_at(state), (goal, goal_level), within, max, ai.safe_drop, me);
    plan_path(physics, &params_of(bodies, db, me), &request)
}
