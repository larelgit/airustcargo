//! What an agent can ask the physics world: `clearance` (how far can I go, and what
//! stops me) and `path_to` (a route around what stops me). Both read the world through
//! [`Terrain`], the same rules the controller walks by, and need no window.
//!
//! The planner is a pure function on `&PhysicsWorld` ([`plan_path`]); `move_to path=true`
//! and the tests call the same code as the `path_to` query. A module that knows more of the
//! world (the navmesh of a loaded region) registers a [`RoutePlanner`], which `move_to
//! path=true` and `path_to` ask first, and whose word ends a `clearance` direction where it
//! knows no way on although the physics world shows ground.

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

pub use clearance::{clearance, reach_toward, Hazard, Reach, COMPASS, COMPASS_STEPS};
pub use path::{hop_allowed, plan_path, step_onto, Leg, PathPlan, PathRequest, Stance, Step, StepKind, CELL, COST_PER_METER};
pub use terrain::{Block, Probe, Terrain};

use crate::ai::MotionAi;
use crate::controller::{CharacterController, CharacterParams, Crouch, MotionMode, MotionState};
use crate::physics::{fresh, LayerMask, PhysicsWorld};
use crate::tools::TargetArg;

pub(crate) fn plugin(app: &mut App) {
    register_tool(app.world_mut(), "clearance", Scope::Player, ToolKind::Query, fresh::<ClearanceArgs>.pipe(clearance_tool));
    register_tool(app.world_mut(), "path_to", Scope::Player, ToolKind::Query, fresh::<PathToArgs>.pipe(path_to_tool));
}

/// A route planner that knows more of the world than the local grid: the navmesh of a
/// loaded region. `move_to path=true` and `path_to` ask it first and plan on the local grid
/// when it answers `None` (the walk is not in its part of the world, or not for this body);
/// `clearance` asks it how far each direction goes.
#[derive(Resource, Clone, Copy)]
pub struct RoutePlanner {
    /// Who answers, for messages: `navmesh`.
    pub name: &'static str,
    pub plan: fn(&World, &RouteRequest) -> Option<Result<PathPlan, String>>,
    /// How far a body goes from a point in one compass direction on what the planner knows
    /// (m, at most the range); `None` where it does not know the point or the body.
    pub reach: fn(&World, &ReachRequest) -> Option<f32>,
}

/// What `move_to path=true` and `path_to` ask a [`RoutePlanner`].
pub struct RouteRequest<'a> {
    pub walker: Entity,
    /// The walker's locomotion numbers.
    pub params: &'a CharacterParams,
    /// What the local planner would be asked.
    pub request: &'a PathRequest,
}

/// What `clearance` asks a [`RoutePlanner`] for each of the eight directions.
pub struct ReachRequest<'a> {
    /// The locomotion numbers of the body that would go.
    pub params: &'a CharacterParams,
    /// Where it stands (feet; a swimmer's float height).
    pub from: Vec3,
    /// The direction as a step between grid columns ([`COMPASS_STEPS`]): N is `(0, -1)`.
    pub step: (i32, i32),
    /// How far to look (m).
    pub range: f32,
}

/// A [`RoutePlanner`]'s reach ends a `clearance` direction (as [`Hazard::Closed`]) where it
/// is this much shorter than the physics world's (m): its grid is coarser than the march.
const CLOSED_MARGIN: f32 = 1.0;

/// Which size of body a perception query reads the world for (F2).
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StanceArg {
    /// The size the body has now (standing, crouched, or between): the default, so what the query
    /// answers is what walking on would find.
    #[default]
    Current,
    /// The body at its full height.
    Standing,
    /// The body crouched (its locomotion's `crouch_height`): the gaps and ducks only a crouch passes.
    Crouched,
}

impl StanceArg {
    /// How far into a crouch the body is that this asks about, given the depth it has now (`crouch`);
    /// an error says why a body that cannot crouch cannot be asked as crouched.
    pub(crate) fn depth(self, params: &CharacterParams, crouch: Option<&Crouch>) -> Result<f32, String> {
        match self {
            StanceArg::Current => Ok(crouch.map_or(0.0, |c| c.depth)),
            StanceArg::Standing => Ok(0.0),
            StanceArg::Crouched if params.can_crouch() => Ok(1.0),
            StanceArg::Crouched => Err("stance=crouched: this body cannot crouch (its locomotion has no `crouch_height`)".to_owned()),
        }
    }
}

/// The locomotion numbers of `entity` (its controller's, or the code defaults), from the
/// world.
pub(crate) fn params_in(world: &World, entity: Entity) -> CharacterParams {
    let own = world.get::<CharacterParams>(entity);
    match world.get::<CharacterController>(entity) {
        Some(controller) => controller.resolve(own, world.get_resource::<ContentDb>()).clone(),
        None => own.cloned().unwrap_or_default(),
    }
}

/// What the registered [`RoutePlanner`] answers for `me` walking toward `target`, and its
/// name: `None` when there is none, it does not cover the walk, or the call is not one it
/// can answer (the local planner then says what is wrong with it). A walk as a crouched body is
/// not one it can answer: the navmesh is baked for one size of body, the standing one, so it knows
/// no duct only a crouch passes, and the local grid, which measures the world for the size asked,
/// plans it.
pub(crate) fn planner_route(
    world: &World,
    me: Entity,
    target: &TargetArg,
    within: Option<f32>,
    max: Option<f32>,
    stance: StanceArg,
) -> Option<(&'static str, Result<PathPlan, String>)> {
    let planner = world.get_resource::<RoutePlanner>()?;
    let tf = world.get::<Transform>(me)?;
    // (A stance the body cannot be asked for is the local planner's to refuse, with its reason.)
    if stance.depth(&params_in(world, me), world.get::<Crouch>(me)).map_or(true, |depth| depth > 0.0) {
        return None;
    }
    let goal = match *target {
        TargetArg::Entity(id) => {
            let entity = world.get_resource::<AiIndex>()?.resolve(id.0).ok().filter(|e| *e != me)?;
            (world.get::<Transform>(entity)?.translation, true)
        }
        TargetArg::Point { x, z } if x.is_finite() && z.is_finite() => (Vec3::new(x, tf.translation.y, z), false),
        TargetArg::Point { .. } => return None,
    };
    let params = params_in(world, me);
    let swimming = swimming_at(world.get::<MotionState>(me));
    let safe_drop = world.get_resource::<MotionAi>().map_or(MotionAi::default().safe_drop, |ai| ai.safe_drop);
    let request = request_for(tf.translation, swimming, goal, within, max, safe_drop, me);
    let answer = (planner.plan)(world, &RouteRequest { walker: me, params: &params, request: &request })?;
    Some((planner.name, answer))
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
    (
        &'static Transform,
        Option<&'static CharacterController>,
        Option<&'static CharacterParams>,
        Option<&'static MotionState>,
        Option<&'static Crouch>,
    ),
>;

/// The locomotion numbers of `entity` (its controller's, or the code defaults), sized for `stance`:
/// the body as it stands, crouched, or as it is now.
pub(crate) fn params_of(bodies: &Bodies, db: Option<&ContentDb>, entity: Entity, stance: StanceArg) -> Result<CharacterParams, String> {
    let (params, crouch) = match bodies.get(entity) {
        Ok((_, Some(controller), own, _, crouch)) => (controller.resolve(own, db).clone(), crouch),
        Ok((_, None, Some(own), _, crouch)) => (own.clone(), crouch),
        _ => (CharacterParams::default(), None),
    };
    let depth = stance.depth(&params, crouch)?;
    Ok(params.at_depth(depth).into_owned())
}

/// The water surface a swimming character floats in.
pub(crate) fn swimming_at(state: Option<&MotionState>) -> Option<f32> {
    state.filter(|s| s.mode == MotionMode::Water).and_then(|s| s.water_surface)
}

/// Both queries measure against solids (colliders and voxel fields on `WORLD`); a world without
/// any would answer "cliff" everywhere.
fn no_physics(physics: &PhysicsWorld) -> Result<(), String> {
    if !physics.has_solid(LayerMask::WORLD) {
        return Err("this world has no colliders to measure against: give the ground and the walls a `Collider` \
                    (shards_motion), or use `observe` and `nearby`, which work from positions"
            .into());
    }
    Ok(())
}

/// How far you can go in each of the eight compass directions and what stops you there:
/// a wall, a cliff (with how deep it drops), water, a ledge you could climb, or, inside a
/// region, the end of its navmesh where the ground goes on but no way does (a tile edge that
/// does not connect, a spot blocked now). Looks 15 m by default. Read-only.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ClearanceArgs {
    /// Look from this entity's position instead of yours, with your body's size.
    pub around: Option<Id>,
    /// The size of body to measure with: `current` (default: as it stands now), `standing`, or `crouched`
    /// (the gaps and ducks only a crouch passes; the body's locomotion needs a `crouch_height`).
    #[serde(default)]
    pub stance: StanceArg,
}

fn clearance_tool(
    In(call): In<Call<ClearanceArgs>>,
    world: &World,
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
    let (tf, _, _, state, _) = bodies.get(subject).map_err(|_| "that entity has no position".to_owned())?;
    // Another entity is looked at with the caller's body: its width decides the gaps. Neither
    // the body nor the subject (a crate on the world layer) stands in its own way.
    let body = me.unwrap_or(subject);
    let params = params_of(&bodies, db.as_deref(), body, call.args.stance)?;
    let mut reaches = clearance(&physics, &params, tf.translation, swimming_at(state), ai.clearance_range, ai.safe_drop, &[body, subject]);
    if let Some(planner) = world.get_resource::<RoutePlanner>() {
        for (reach, step) in reaches.iter_mut().zip(COMPASS_STEPS) {
            let ask = ReachRequest { params: &params, from: tf.translation, step, range: ai.clearance_range };
            if let Some(distance) = (planner.reach)(world, &ask)
                && distance + CLOSED_MARGIN < reach.distance
            {
                *reach = Reach { distance, hazard: Hazard::Closed { by: planner.name }, ..*reach };
            }
        }
    }
    let line = reaches.iter().map(|r| r.describe()).collect::<Vec<_>>().join(" · ");
    Ok(match call.args.around {
        Some(id) => format!("around {} at {}: {line}", id, fmt_pos(tf.translation)),
        None => line,
    })
}

/// Plans a route over what you can walk, swim and climb, around walls and cliffs, and
/// reports it: `path 23.5m, 4 turns: 6.0m NE → 4.5m N (mantle 1.1m) → 9.0m E (swim)`.
/// Returns the waypoints too, and who planned it (`provider`): inside a region its navmesh,
/// elsewhere the local grid. `move_to path=true` follows such a route. Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct PathToArgs {
    /// An entity id or a ground point {x, z}.
    pub target: TargetArg,
    /// Arrive within this many meters of the target. Default 1.5 for an entity, 0.3 for a point.
    pub within: Option<f32>,
    /// Longest route to accept, meters. Default 80 on the local grid, any length on a
    /// region's navmesh.
    pub max: Option<f32>,
    /// The size of body to plan for: `current` (default: as it stands now), `standing`, or `crouched`
    /// (a route through the ducks only a crouch passes; hold `crouch` while you follow it). A
    /// crouched route is planned on the local grid: the navmesh is baked for the standing body.
    #[serde(default)]
    pub stance: StanceArg,
}

#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn path_to_tool(
    In(call): In<Call<PathToArgs>>,
    world: &World,
    index: Res<AiIndex>,
    physics: Res<PhysicsWorld>,
    ai: Res<MotionAi>,
    db: Option<Res<ContentDb>>,
    bodies: Bodies,
) -> ToolResult<Value> {
    let me = call.caller.require_subject()?;
    let PathToArgs { target, within, max, stance } = call.args;
    let (plan, provider) = match planner_route(world, me, &target, within, max, stance) {
        Some((name, answer)) => {
            let plan = answer.map_err(|why| format!("{name}: {why}"))?;
            if let Some(max) = max.map(|m| m.clamp(1.0, 300.0))
                && plan.length > max
            {
                return Err(format!(
                    "the shortest route ({name}) is {:.1}m, longer than max {max:.0}m; raise `max` or go in stages",
                    plan.length
                ));
            }
            (plan, name)
        }
        None => (plan_for(me, &target, within, max, stance, &index, &physics, &ai, db.as_deref(), &bodies)?, "local grid"),
    };
    let mut out = plan.to_json();
    out["provider"] = provider.into();
    Ok(out)
}

/// The route the `path_to` and `move_to path=true` tools plan for `me` toward `target`.
#[allow(clippy::too_many_arguments, reason = "everything the two tools read, passed once")]
pub(crate) fn plan_for(
    me: Entity,
    target: &TargetArg,
    within: Option<f32>,
    max: Option<f32>,
    stance: StanceArg,
    index: &AiIndex,
    physics: &PhysicsWorld,
    ai: &MotionAi,
    db: Option<&ContentDb>,
    bodies: &Bodies,
) -> Result<PathPlan, String> {
    no_physics(physics)?;
    let (tf, _, _, state, _) = bodies.get(me).map_err(|_| "your body has no position".to_owned())?;
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
    plan_path(physics, &params_of(bodies, db, me, stance)?, &request)
}
