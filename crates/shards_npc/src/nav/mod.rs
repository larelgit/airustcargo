//! Navigation: the navmesh of the region (E6.1) and the nav agents that walk it.
//!
//! ## The navmesh
//!
//! Every tile (and dungeon room) is **baked** ahead of time into a grid of walkable cells
//! ([`NavTile`]): the tile's gameplay geometry is spawned alone in a scratch world by the
//! world module's own tile spawner and sampled with the motion module's terrain probe and
//! move rule ([`step_onto`](shards_motion::perception::step_onto)), so the navmesh, the
//! local planner (`path_to`) and the character controller agree on what can be walked,
//! climbed ([`CharacterParams::can_climb`]), dropped and swum. Bakes live in
//! `<assets>/nav/<tile>.navtile.ron` (rooms `.navroom.ron`), hashed against everything they
//! are made from: `--bake-nav [tiles|all]` (the game binary, [`bake_files`]) or the
//! `nav_bake` tool writes them; with a `nav` content definition ([`NavSettingsDef`]: cell
//! size, the body, the folder) a missing or stale one is a content error.
//!
//! When a region loads, its tiles are placed and turned, **carved** where solids the bakes
//! never saw stand (decor, props, colliders spawned later), **stitched** across every tile
//! edge the sockets leave open, and joined by the levels' `navlink` markers
//! ([`NavRegion`]). A tile without a fresh bake file is baked right then, with a journal
//! warning.
//!
//! A **route** ([`NavRegion::path`]) is A* over the tile graph for the corridor, grid A*
//! inside the corridor's tiles, then string pulling checked against the grid and the
//! physics world. Routes need no window and replay identically.
//!
//! ```ron
//! // content/nav/default.nav.ron
//! { "default": (cell: 0.5, agent: Some("grunt"), safe_drop: 2.0) }
//! ```
//!
//! ## Nav agents
//!
//! Where an NPC walks, by which way, without bumping into the others.
//!
//! - **Routes** come from [`NavPaths`], a seam: `fn(&World, from, to, radius) -> waypoints`.
//!   The default asks the navmesh inside a loaded region and the motion module's local grid
//!   planner ([`plan_path`]) everywhere else (and for bodies bigger than the baked one); a
//!   game may plug in its own. A goal in plain sight ([`plain_sight`]: on the navmesh a
//!   straight line of walking links, so never over a drop or a ledge; elsewhere a body-wide
//!   cast through the `WORLD` layer over ground without holes or deep drops) needs no route;
//!   another is planned again when the goal moves or every [`NpcTuning::repath`] seconds,
//!   which keeps route queries few however many NPCs chase. Where there is no route the NPC
//!   holds its ground until the next plan (it never walks blindly at the goal).
//! - **Following**: toward the next waypoint at the state's speed; arrived within `within`
//!   meters of the goal.
//! - **Avoidance** between NPCs: each looks [`NpcTuning::avoid_time`] seconds ahead at where
//!   the others are going and steers away from the closest approach (to its right when it
//!   would meet one head on, so two walkers pass each other), plus a push apart when already
//!   close.
//! - **Leash**: an NPC whose state keeps it leashed never steers out of its [`Home`] circle.
//!
//! The result is `Intent.move_dir` (and `aim`, presses, held inputs): NPCs drive their body
//! exactly as players do. `move_to path=true` follows navmesh routes too (the motion
//! module's [`RoutePlanner`] seam), and `nav_path` shows them.

pub mod bake;
#[cfg(feature = "render")]
pub(crate) mod overlay;
pub mod query;
pub mod region;
pub mod settings;
pub mod tile;
pub(crate) mod tools;

pub use query::{NavError, NavPath, NavQuery};
pub use region::{rebuild, refresh, NavBakes, NavRegion};
pub use settings::{NavSettings, NavSettingsDef};
pub use tile::NavTile;
pub use tools::{bake_files, BakeReport, NavOverlay};

use ai_core::AiId;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent};
use shards_motion::perception::{plan_path, Block, PathPlan, PathRequest, Probe, RoutePlanner, RouteRequest, Terrain};
use shards_world::ActiveRegion;
use shards_motion::{
    CharacterController, CharacterParams, LayerMask, MotionAi, MotionState, PhysicsWorld, QueryFilter, Shape,
};

use crate::brain::{npcs_in_order, Home, Npc, NpcBrain, NpcDrive, LEASH_ROOM};
use crate::def::NpcTuning;

/// Pluggable routes: `find(world, from, to, body radius) -> waypoints` (the last one at
/// `to`), `None` when there is no way. The default ([`navmesh_path`]) asks the navmesh in a
/// loaded region and the motion module's local grid planner elsewhere.
#[derive(Resource, Clone, Copy)]
pub struct NavPaths {
    /// Who answers, for tools: `local grid`, `navmesh`.
    pub name: &'static str,
    pub find: fn(&World, Vec3, Vec3, f32) -> Option<Vec<Vec3>>,
}

impl Default for NavPaths {
    fn default() -> Self {
        Self { name: NAVMESH, find: navmesh_path }
    }
}

/// The name of the engine's own provider ([`NavPaths::default`]).
pub const NAVMESH: &str = "navmesh";

/// What the navmesh says about a walk of a body of `params`: `None` when it does not cover
/// it (no region, a goal or start outside it, a bigger body than the baked one, a walker
/// off the ground), else the route or why there is none.
pub fn navmesh_route(world: &World, from: Vec3, to: Vec3, params: &CharacterParams, goal_level: bool) -> Option<Result<NavPath, String>> {
    let nav = world.get_resource::<NavRegion>()?;
    if world.get_resource::<ActiveRegion>().map(|r| r.hash) != Some(nav.hash) {
        return None;
    }
    let physics = world.get_resource::<PhysicsWorld>()?;
    let query = NavQuery { from, to, goal_level, radius: params.radius, height: params.height, step_height: params.step_height };
    match nav.path(physics, &query) {
        Ok(path) => Some(Ok(path)),
        Err(NavError::NotCovered(_)) => None,
        Err(NavError::NoRoute(why)) => Some(Err(why)),
    }
}

/// The engine's routes: the navmesh's where it covers the walk, else the local grid's. Only
/// the radius is known here: the body is taken to be as tall as the baked one.
pub fn navmesh_path(world: &World, from: Vec3, to: Vec3, radius: f32) -> Option<Vec<Vec3>> {
    let height = world.get_resource::<NavRegion>().map_or(CharacterParams::default().height, |nav| nav.agent_height);
    let params = CharacterParams { radius, height, ..default() };
    match navmesh_route(world, from, to, &params, false) {
        Some(Ok(path)) => Some(path.points),
        Some(Err(_)) => None,
        None => local_grid_path(world, from, to, radius),
    }
}

/// The motion module's [`RoutePlanner`]: `move_to path=true` on the navmesh.
pub(crate) fn route_plan(world: &World, request: &RouteRequest) -> Option<Result<PathPlan, String>> {
    let r = request.request;
    navmesh_route(world, r.start, r.goal, request.params, r.goal_level).map(|route| route.map(|path| path.to_plan()))
}

/// The navmesh as the motion module's route planner.
pub(crate) const ROUTE_PLANNER: RoutePlanner = RoutePlanner { name: NAVMESH, plan: route_plan };

/// The route [`NavPaths`] gives right now (tools, tests).
pub fn find_path(world: &World, from: Vec3, to: Vec3, radius: f32) -> Option<Vec<Vec3>> {
    let paths = world.get_resource::<NavPaths>().copied().unwrap_or_default();
    (paths.find)(world, from, to, radius)
}

/// The default route: `shards_motion`'s grid A* (0.5 m cells) for a body of `radius` and the
/// controller's default numbers, searching up to twice the straight distance (plus 20 m).
pub fn local_grid_path(world: &World, from: Vec3, to: Vec3, radius: f32) -> Option<Vec<Vec3>> {
    let physics = world.get_resource::<PhysicsWorld>()?;
    let params = CharacterParams { radius, ..default() };
    let straight = flat(to - from).length();
    let request = PathRequest {
        start: from,
        swimming_at: None,
        goal: to,
        goal_level: false,
        within: 0.3,
        max_length: (straight * 2.0 + 20.0).min(300.0),
        safe_drop: world.get_resource::<MotionAi>().map_or(2.0, |m| m.safe_drop),
        exclude: None,
    };
    let plan = plan_path(physics, &params, &request).ok()?;
    let mut points: Vec<Vec3> = plan.waypoints().into_iter().map(|(p, _)| p).collect();
    match points.last_mut() {
        Some(last) => *last = Vec3::new(to.x, last.y, to.z),
        None => points.push(to),
    }
    Some(points)
}

/// An NPC's way to its goal and what it asks of its legs this tick.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NavAgent {
    pub goal: Option<Vec3>,
    /// Arrived within this many meters of the goal.
    pub within: f32,
    /// Movement input: 0.5 walks, 1 runs.
    pub speed: f32,
    /// Waypoints, the last at the goal; none while there is no way there (it holds).
    pub path: Vec<Vec3>,
    /// The waypoint it walks to.
    pub index: u32,
    /// The goal the path was planned for.
    pub planned_to: Option<Vec3>,
    /// The goal was in plain sight: no route needed.
    pub direct: bool,
    /// Seconds before a blocked route is planned again.
    pub repath: f32,
    /// At the goal (or without one).
    pub arrived: bool,
    /// The movement input it asks for this tick: after avoidance and the leash.
    pub desired: Vec3,
}

/// The body numbers of an NPC: its controller's (or its own), else the defaults.
fn params_of<'a>(
    db: &'a ContentDb,
    controller: Option<&'a CharacterController>,
    own: Option<&'a CharacterParams>,
) -> CharacterParams {
    match controller {
        Some(c) => c.resolve(own, Some(db)).clone(),
        None => own.cloned().unwrap_or_default(),
    }
}

/// Whether a body of `params` walks from `from` straight to `to`, so it needs no route: on
/// the navmesh where it covers the walk, a line of walking links (no drop, no ledge, nothing
/// carved) a body-wide sweep agrees with; elsewhere [`straight_clear`] over ground without a
/// hole or a drop deeper than the safe drop all along.
pub fn plain_sight(world: &World, from: Vec3, to: Vec3, params: &CharacterParams) -> bool {
    let Some(physics) = world.get_resource::<PhysicsWorld>() else { return true };
    if let Some(nav) = world.get_resource::<NavRegion>()
        && world.get_resource::<ActiveRegion>().map(|r| r.hash) == Some(nav.hash)
    {
        // Only the radius counts, as for routes (`navmesh_path`): the body is the baked one.
        let query = NavQuery { from, to, goal_level: false, radius: params.radius, height: nav.agent_height, step_height: params.step_height };
        if let Some(straight) = nav.straight_walk(physics, &query) {
            return straight;
        }
    }
    let safe_drop = world.get_resource::<MotionAi>().map_or(2.0, |m| m.safe_drop);
    straight_clear(physics, from, to, params.radius, params.step_height) && ground_along(physics, from, to, params, safe_drop)
}

/// Whether there is ground all along the line from `from` to `to` for a body of `params`:
/// no hole, no floor too steep to stand on, no drop deeper than `safe_drop` (what stands in
/// the way is [`straight_clear`]'s business).
fn ground_along(physics: &PhysicsWorld, from: Vec3, to: Vec3, params: &CharacterParams, safe_drop: f32) -> bool {
    let terrain = Terrain::new(physics, params, &[]);
    let delta = flat(to - from);
    let steps = (delta.length() / 0.5).ceil() as usize;
    let mut y = from.y;
    for k in 1..=steps {
        let at = from + delta * (k as f32 / steps as f32);
        match terrain.probe(at.x, at.z, y) {
            Probe::Floor { y: floor, .. } if floor >= y - safe_drop => y = floor,
            Probe::Water { surface } => y = terrain.swim_height(surface),
            Probe::Blocked(Block::Tight | Block::Wall) => {}
            _ => return false,
        }
    }
    true
}

/// Whether a body of `radius` walks from `from` straight to `to` without touching the
/// `WORLD` layer (above its step height).
pub fn straight_clear(physics: &PhysicsWorld, from: Vec3, to: Vec3, radius: f32, step_height: f32) -> bool {
    let delta = flat(to - from);
    let distance = delta.length();
    let Ok(dir) = Dir3::new(delta) else { return true };
    let lift = Vec3::Y * (step_height + radius + 0.05);
    let filter = QueryFilter::new(LayerMask::WORLD);
    let ball = Shape::sphere((radius * 0.95).max(0.05));
    physics.shapecast(&ball, from + lift, Quat::IDENTITY, dir, distance, &filter).is_none_or(|hit| hit.distance >= distance - 0.05)
}

/// `NpcSystems::Navigate`, first: routes for the agents whose goal is new, moved or blocked.
pub(crate) fn nav_paths(world: &mut World) {
    // The navmesh belongs to the region in the world and knows the solids spawned since.
    refresh(world);
    let dt = world.resource::<Time>().delta_secs();
    let mut updated: Vec<(Entity, NavAgent)> = Vec::new();
    {
        let world: &World = world;
        let Some(db) = world.get_resource::<ContentDb>() else { return };
        if !world.contains_resource::<PhysicsWorld>() {
            return;
        }
        let tuning = NpcTuning::of(db);
        for (_, entity) in npcs_in_order(world) {
            let e = world.entity(entity);
            let (Some(agent), Some(drive), Some(brain), Some(tf)) =
                (e.get::<NavAgent>(), e.get::<NpcDrive>(), e.get::<NpcBrain>(), e.get::<Transform>())
            else {
                continue;
            };
            let mut next = agent.clone();
            next.goal = drive.goal.filter(|_| brain.active);
            next.within = drive.within;
            next.speed = drive.speed;
            next.repath = (next.repath - dt).max(0.0);
            let Some(goal) = next.goal else {
                if !next.path.is_empty() || next.planned_to.is_some() {
                    next.path.clear();
                    next.planned_to = None;
                    next.index = 0;
                }
                if next != *agent {
                    updated.push((entity, next));
                }
                continue;
            };
            let here = tf.translation;
            let params = params_of(db, e.get::<CharacterController>(), e.get::<CharacterParams>());
            let away = flat(goal - here).length();
            let moved = next.planned_to.is_none_or(|p| flat(p - goal).length() > (0.2 * away).max(0.75));
            if moved || (!next.direct && next.repath <= 0.0) {
                next.direct = plain_sight(world, here, goal, &params);
                // No route: no path, and it holds where it is until it plans again.
                next.path = if next.direct {
                    vec![goal]
                } else {
                    find_path(world, here, goal, params.radius).filter(|p| !p.is_empty()).unwrap_or_default()
                };
                next.index = 0;
                next.planned_to = Some(goal);
                next.repath = tuning.repath;
            } else if let Some(last) = next.path.last_mut() {
                // A goal that moved a little: the last leg follows it.
                *last = goal;
            }
            if next != *agent {
                updated.push((entity, next));
            }
        }
    }
    for (entity, next) in updated {
        if let Some(mut agent) = world.get_mut::<NavAgent>(entity) {
            *agent = next;
        }
    }
}

/// Another body an agent steers clear of.
#[derive(Clone, Copy)]
struct Other {
    id: AiId,
    pos: Vec3,
    vel: Vec3,
    radius: f32,
}

type Agents<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static AiId,
        &'static mut NavAgent,
        &'static NpcDrive,
        &'static NpcBrain,
        &'static Transform,
        Option<&'static Home>,
        Option<&'static MotionState>,
        Option<&'static CharacterController>,
        Option<&'static CharacterParams>,
    ),
    With<Npc>,
>;

/// `NpcSystems::Navigate`, second: every agent follows its route, steers clear of the other
/// NPCs and stays on its leash.
pub(crate) fn nav_steer(db: Res<ContentDb>, mut agents: Agents) {
    let tuning = NpcTuning::of(&db);
    let mut others: Vec<Other> = agents
        .iter()
        .map(|(_, id, _, _, _, tf, _, motion, controller, own)| Other {
            id: *id,
            pos: tf.translation,
            vel: motion.map_or(Vec3::ZERO, |m| flat(m.velocity)),
            radius: params_of(&db, controller, own).radius,
        })
        .collect();
    others.sort_by_key(|o| o.id);
    let mut order: Vec<(AiId, Entity)> = agents.iter().map(|(e, id, ..)| (*id, e)).collect();
    order.sort();
    for (id, entity) in order {
        let Ok((_, _, mut agent, drive, brain, tf, home, _, controller, own)) = agents.get_mut(entity) else { continue };
        let params = params_of(&db, controller, own);
        let here = tf.translation;
        let mut next = agent.clone();
        next.desired = Vec3::ZERO;
        match next.goal.filter(|_| brain.active) {
            None => next.arrived = true,
            Some(goal) if flat(goal - here).length() <= next.within.max(0.05) => next.arrived = true,
            // No way there: it holds (and plans again every `repath` seconds).
            Some(_) if next.path.is_empty() => next.arrived = false,
            Some(goal) => {
                next.arrived = false;
                let last = next.path.len().saturating_sub(1);
                let mut index = (next.index as usize).min(last);
                while index < last && flat(next.path[index] - here).length() <= tuning.waypoint_reach {
                    index += 1;
                }
                next.index = index as u32;
                let toward = next.path.get(index).copied().unwrap_or(goal);
                let dir = flat(toward - here).try_normalize().unwrap_or(Vec3::ZERO);
                let speed = next.speed.clamp(0.0, 1.0);
                let (top, _) = params.ground_speed(speed, false);
                let me = Other { id, pos: here, vel: dir * top, radius: params.radius };
                let mut vel = avoid(&me, &others, &tuning);
                if drive.leashed
                    && let Some(home) = home
                {
                    vel = keep_on_leash(vel, here, home.pos.unwrap_or(here), home.leash);
                }
                next.desired = if top > 0.0 { (vel / top).clamp_length_max(1.0) * speed } else { Vec3::ZERO };
            }
        }
        if *agent != next {
            *agent = next;
        }
    }
}

/// `me.vel` bent away from the NPCs it would come too close to within the look-ahead.
fn avoid(me: &Other, others: &[Other], tuning: &NpcTuning) -> Vec3 {
    let speed = me.vel.length();
    if speed <= 1e-4 {
        return me.vel;
    }
    let ahead = tuning.avoid_time.max(0.05);
    let right = Vec3::new(-me.vel.z, 0.0, me.vel.x) / speed;
    let mut push = Vec3::ZERO;
    for o in others.iter().filter(|o| o.id != me.id) {
        let rel = flat(o.pos - me.pos);
        let room = me.radius + o.radius + tuning.avoid_margin;
        let reach = room + (speed + o.vel.length()) * ahead;
        let d = rel.length();
        if d > reach {
            continue;
        }
        // Already close: straight apart.
        if d < room {
            let away = if d > 1e-4 { -rel / d } else { -right };
            push += away * (room - d) / room * speed * 1.5;
        }
        // Where the two come closest within the look-ahead.
        let rv = o.vel - me.vel;
        let a = rv.length_squared();
        let t = if a > 1e-6 { (-rel.dot(rv) / a).clamp(0.0, ahead) } else { 0.0 };
        let closest = rel + rv * t;
        let gap = closest.length();
        if gap < room && t > 0.0 {
            // Head on (the other right in its way): pass on the right.
            let away = if gap > 0.05 * room { -closest / gap } else { right };
            push += away * (room - gap) / room * (1.0 - t / ahead).max(0.15) * speed * 2.0;
        }
    }
    let vel = me.vel + push;
    // Never back up because of others: at worst stand aside.
    let forward = me.vel / speed;
    let along = vel.dot(forward);
    let vel = if along < 0.0 { vel - forward * along } else { vel };
    vel.clamp_length_max(speed)
}

/// `vel` without the part that takes a body at `here` out of its leash.
fn keep_on_leash(vel: Vec3, here: Vec3, home: Vec3, leash: f32) -> Vec3 {
    let out = flat(here - home);
    let d = out.length();
    if d < leash - LEASH_ROOM * 0.5 || d <= 1e-4 {
        return vel;
    }
    let n = out / d;
    let outward = vel.dot(n);
    if outward > 0.0 { vel - n * outward } else { vel }
}

/// `NpcSystems::Drive`: what NPCs decided goes into their `Intent`, like a keyboard's: the
/// movement, the point to face, the presses, the held inputs. An NPC with orders of its own
/// (an agent took it over with `move_to` or `attack`) follows those instead.
pub(crate) fn drive_intents(mut npcs: Query<(&NpcBrain, &NavAgent, &mut NpcDrive, &mut Intent), With<Npc>>) {
    for (brain, agent, mut drive, mut intent) in &mut npcs {
        if !intent.orders.is_empty() {
            // Its own presses are dropped and what it held is let go (the director and the
            // planner leave it out of the fight meanwhile).
            if !drive.press.is_empty() {
                drive.press.clear();
            }
            if !drive.held.is_empty() {
                for input in std::mem::take(&mut drive.held) {
                    intent.set_held(&input, false);
                }
            }
            continue;
        }
        let (move_dir, aim) = if brain.active { (agent.desired, drive.face) } else { (Vec3::ZERO, None) };
        if intent.move_dir != move_dir {
            intent.move_dir = move_dir;
        }
        if intent.aim != aim {
            intent.aim = aim;
        }
        if !drive.press.is_empty() {
            let presses = std::mem::take(&mut drive.press);
            for input in presses {
                intent.press(input);
            }
        }
        let hold = if brain.active { drive.hold.clone() } else { Vec::new() };
        if drive.held != hold {
            for input in drive.held.iter().filter(|h| !hold.contains(h)) {
                intent.set_held(input, false);
            }
            for input in &hold {
                intent.set_held(input, true);
            }
            drive.held = hold;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walker(id: u32, x: f32, vx: f32) -> Other {
        Other { id: AiId(id), pos: Vec3::new(x, 0.0, 0.0), vel: Vec3::new(vx, 0.0, 0.0), radius: 0.4 }
    }

    #[test]
    fn head_on_walkers_each_pass_on_their_right() {
        let tuning = NpcTuning::default();
        let (a, b) = (walker(1, -2.0, 2.0), walker(2, 2.0, -2.0));
        let (va, vb) = (avoid(&a, &[a, b], &tuning), avoid(&b, &[a, b], &tuning));
        // Facing +X the right is +Z (south); facing -X it is -Z.
        assert!(va.z > 0.1 && vb.z < -0.1, "{va} {vb}");
        assert!(va.x > 0.0 && vb.x < 0.0, "neither backs up: {va} {vb}");
        let far = walker(3, 40.0, -2.0);
        assert_eq!(avoid(&a, &[a, far], &tuning), a.vel, "nobody near: straight on");
    }

    #[test]
    fn the_leash_stops_the_outward_part_only() {
        let home = Vec3::ZERO;
        let at_edge = Vec3::new(9.9, 0.0, 0.0);
        let v = keep_on_leash(Vec3::new(1.0, 0.0, 1.0), at_edge, home, 10.0);
        assert!(v.x.abs() < 1e-5 && (v.z - 1.0).abs() < 1e-5, "{v}");
        assert_eq!(keep_on_leash(Vec3::new(-1.0, 0.0, 0.0), at_edge, home, 10.0), Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(keep_on_leash(Vec3::X, Vec3::new(3.0, 0.0, 0.0), home, 10.0), Vec3::X, "well inside");
    }
}
