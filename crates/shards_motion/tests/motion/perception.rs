//! `clearance` and `path_to` on known layouts, and `move_to path=true` following a plan.

use bevy::prelude::*;
use serde_json::{json, Value};
use shards_motion::perception::{clearance, plan_path, Hazard, PathRequest, StepKind};
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

fn params() -> CharacterParams {
    CharacterParams::default()
}

fn physics(app: &App) -> &PhysicsWorld {
    app.world().resource::<PhysicsWorld>()
}

/// Lets the physics world catch up with entities spawned since the last frame.
fn settle(app: &mut App) {
    app.update();
}

/// North: a short wall 3 m away. East: the edge of a 10 m cliff, 6 m away. South: a pool
/// beyond a bank 5 m away. West: open ground for as far as anyone looks.
fn lay_out_the_surroundings(world: &mut World) {
    block(world, Vec3::new(-30.0, -10.0, -30.0), Vec3::new(6.0, 0.0, 5.0));
    block(world, Vec3::new(6.0, -12.0, -30.0), Vec3::new(60.0, -10.0, 30.0));
    block(world, Vec3::new(-30.0, -10.0, 5.0), Vec3::new(6.0, -3.0, 20.0));
    volume(world, Vec3::new(-30.0, -3.0, 5.0), Vec3::new(6.0, 0.0, 20.0), VolumeKind::Water);
    block(world, Vec3::new(-2.0, 0.0, -4.0), Vec3::new(2.0, 4.0, -3.0));
}

fn reach<'a>(reaches: &'a [perception::Reach], dir: &str) -> &'a perception::Reach {
    reaches.iter().find(|r| r.dir == dir).unwrap()
}

use shards_motion::perception;

#[test]
fn clearance_names_the_wall_the_cliff_the_water_and_the_open_way() {
    let mut app = app();
    lay_out_the_surroundings(app.world_mut());
    settle(&mut app);
    let reaches = clearance(physics(&app), &params(), Vec3::ZERO, None, 15.0, 2.0, None);
    assert_eq!(reaches.len(), 8);
    let dirs: Vec<&str> = reaches.iter().map(|r| r.dir).collect();
    assert_eq!(dirs, ["N", "NE", "E", "SE", "S", "SW", "W", "NW"]);

    // The wall's face is 3 m away; the body (r 0.4) reaches it after 2.6 m.
    let n = reach(&reaches, "N");
    assert_eq!(n.hazard, Hazard::Wall);
    assert!((n.distance - 2.6).abs() < 0.05, "{n:?}");
    // The cliff edge at 6 m, 10 m down.
    let e = reach(&reaches, "E");
    assert!(matches!(e.hazard, Hazard::Cliff { drop: Some(d) } if (d - 10.0).abs() < 0.05), "{e:?}");
    assert!((e.distance - 6.0).abs() < 0.05, "{e:?}");
    // The pool starts at the bank, 5 m away.
    let s = reach(&reaches, "S");
    assert_eq!(s.hazard, Hazard::Water);
    assert!((s.distance - 5.0).abs() < 0.05, "{s:?}");
    // Nothing to the west for the whole range.
    let w = reach(&reaches, "W");
    assert_eq!((w.hazard, w.distance), (Hazard::Free, 15.0));
}

#[test]
fn without_colliders_the_queries_say_so_instead_of_seeing_cliffs_everywhere() {
    let mut app = app();
    player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 1);
    for (tool, args) in [("clearance", json!({})), ("path_to", json!({"target": {"x": 3, "z": 3}}))] {
        let err = fails(&mut app, tool, args);
        assert!(err.contains("no colliders to measure against") && err.contains("`Collider`"), "{tool}: {err}");
    }
}

#[test]
fn clearance_reads_like_the_example_in_the_docs() {
    let mut app = app();
    lay_out_the_surroundings(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    let out = ok(&mut app, "clearance", json!({}));
    let parts: Vec<&str> = out.split(" · ").collect();
    assert_eq!(parts.len(), 8, "{out}");
    assert!(parts[0].starts_with("N wall 2.6m"), "{out}");
    assert!(parts[2].starts_with("E cliff 6.0m (drop 10.0m)") || parts[2].starts_with("E cliff 5.9m (drop 10.0m)"), "{out}");
    assert!(parts[4].starts_with("S water 5.0m") || parts[4].starts_with("S water 4.9m"), "{out}");
    assert_eq!(parts[6], "W free 15.0m");

    // Another entity's surroundings, seen with your body.
    let far = app.world_mut().spawn((ai_core::AiKind::new("Marker"), Transform::from_xyz(-20.0, 0.0, 0.0))).id();
    let far_id = *app.world().get::<ai_core::AiId>(far).unwrap();
    let around = ok(&mut app, "clearance", json!({"around": far_id.to_string()}));
    assert!(around.starts_with(&format!("around {far_id} at (-20.0, 0.0): N free")), "{around}");
    let _ = e;
}

#[test]
fn clearance_sees_a_ledge_it_could_climb() {
    let mut app = app();
    floor(app.world_mut());
    // A 1.1 m block 4 m to the west, 2 m thick.
    block(app.world_mut(), Vec3::new(-6.0, 0.0, -4.0), Vec3::new(-4.0, 1.1, 4.0));
    // And a tall one to the east.
    block(app.world_mut(), Vec3::new(4.0, 0.0, -4.0), Vec3::new(6.0, 3.0, 4.0));
    settle(&mut app);
    let reaches = clearance(physics(&app), &params(), Vec3::ZERO, None, 15.0, 2.0, None);
    let w = reach(&reaches, "W");
    assert!(matches!(w.hazard, Hazard::Ledge { rise } if (rise - 1.1).abs() < 0.05), "{w:?}");
    assert!((w.distance - 3.6).abs() < 0.05, "{w:?}");
    assert_eq!(w.describe(), "W ledge 3.6m (+1.1m)");
    assert_eq!(reach(&reaches, "E").hazard, Hazard::Wall);
}

#[test]
fn line_of_sight_goes_through_physics_and_still_honours_occluder_boxes() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(-1.0, 0.0, -6.0), Vec3::new(1.0, 3.0, -5.0));
    player(app.world_mut(), Vec3::ZERO);
    let dummy = |app: &mut App, x: f32, z: f32| {
        let e = app.world_mut().spawn((ai_core::AiKind::new("Dummy"), Transform::from_xyz(x, 0.0, z))).id();
        app.world().get::<ai_core::AiId>(e).unwrap().to_string()
    };
    let behind_wall = dummy(&mut app, 0.0, -10.0);
    let in_the_open = dummy(&mut app, 6.0, -10.0);
    // A box with no collider: only the AI layer's occluder knows about it.
    app.world_mut().spawn((ai_core::AiOccluder::standing(Vec3::new(1.0, 2.0, 1.0)), Transform::from_xyz(-4.0, 0.0, -5.0)));
    let behind_box = dummy(&mut app, -8.0, -10.0);
    step(&mut app, 2);

    assert!(ok(&mut app, "line_of_sight", json!({"to": behind_wall})).starts_with("blocked"));
    assert!(ok(&mut app, "line_of_sight", json!({"to": in_the_open})).starts_with("clear"));
    assert!(ok(&mut app, "line_of_sight", json!({"to": behind_box})).starts_with("blocked"), "occluder boxes keep working");
    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.lines().any(|l| l.contains(&behind_wall) && l.ends_with("occluded")), "{seen}");
    assert!(seen.lines().any(|l| l.contains(&in_the_open) && !l.contains("occluded")), "{seen}");
}

/// A long wall across the way with a gap at x = 10..12.
pub fn wall_with_a_gap(world: &mut World) {
    floor(world);
    block(world, Vec3::new(-30.0, 0.0, -5.0), Vec3::new(10.0, 3.0, -4.0));
    block(world, Vec3::new(12.0, 0.0, -5.0), Vec3::new(40.0, 3.0, -4.0));
}

fn request(start: Vec3, goal: Vec3) -> PathRequest {
    PathRequest {
        start,
        swimming_at: None,
        goal,
        goal_level: false,
        within: 0.3,
        max_length: 80.0,
        safe_drop: 2.0,
        exclude: None,
    }
}

#[test]
fn a_path_goes_around_a_wall_through_the_gap() {
    let mut app = app();
    wall_with_a_gap(app.world_mut());
    settle(&mut app);
    let goal = Vec3::new(0.0, 0.0, -12.0);
    let plan = plan_path(physics(&app), &params(), &request(Vec3::ZERO, goal)).unwrap();
    // Straight to the gap's near corner and on to the goal is 24.2 m; the route may not be
    // much longer than that.
    assert!(plan.length > 24.0 && plan.length < 27.0, "{}", plan.summary());
    assert!(plan.legs.len() >= 2 && plan.legs.iter().all(|l| l.kind == StepKind::Walk), "{}", plan.summary());
    // Through the gap, not through the wall: where the route crosses the wall's line (z = -4.5).
    let mut points = vec![Vec3::ZERO];
    points.extend(plan.legs.iter().map(|l| l.end));
    let crossings: Vec<f32> = points
        .windows(2)
        .filter(|w| (w[0].z + 4.5) * (w[1].z + 4.5) < 0.0)
        .map(|w| w[0].x + (w[1].x - w[0].x) * (-4.5 - w[0].z) / (w[1].z - w[0].z))
        .collect();
    assert!(crossings.len() == 1 && crossings[0] > 10.0 && crossings[0] < 12.0, "{crossings:?} in {}", plan.summary());
    let last = plan.legs.last().unwrap().end;
    assert!(Vec2::new(last.x - goal.x, last.z - goal.z).length() < 0.1, "ends on the point: {last}");
    assert!(plan.summary().starts_with("path ") && plan.summary().contains(" turns: "), "{}", plan.summary());

    // The same world gives the same plan, however often it is asked.
    assert_eq!(plan, plan_path(physics(&app), &params(), &request(Vec3::ZERO, goal)).unwrap());
}

#[test]
fn path_to_answers_with_a_summary_and_the_waypoints() {
    let mut app = app();
    wall_with_a_gap(app.world_mut());
    player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    let out = call(&mut app, "path_to", json!({"target": {"x": 0, "z": -12}}));
    assert!(out.ok, "{}", out.result);
    let summary = out.result["path"].as_str().unwrap();
    assert!(summary.starts_with("path ") && summary.contains("turns"), "{summary}");
    let waypoints = out.result["waypoints"].as_array().unwrap();
    assert!(waypoints.len() >= 2);
    assert_eq!(waypoints[0]["mode"], "walk");
    let last = waypoints.last().unwrap();
    assert!(last["x"].as_f64().unwrap().abs() < 0.1 && (last["z"].as_f64().unwrap() + 12.0).abs() < 0.1, "{last}");
    assert_eq!(out.result["turns"].as_u64().unwrap() as usize, waypoints.len());
}

#[test]
fn a_ledge_in_the_way_is_a_mantle_in_the_path() {
    let mut app = app();
    floor(app.world_mut());
    // A wall 1.1 m high across the whole way: nothing to go around.
    block(app.world_mut(), Vec3::new(-60.0, 0.0, -5.0), Vec3::new(60.0, 1.1, -4.0));
    settle(&mut app);
    let plan = plan_path(physics(&app), &params(), &request(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0))).unwrap();
    let mantle = plan.legs.iter().find(|l| l.kind == StepKind::Mantle).unwrap_or_else(|| panic!("{}", plan.summary()));
    assert!((mantle.rise - 1.1).abs() < 0.05, "{}", plan.summary());
    assert!(plan.summary().contains("(mantle 1.1m)"), "{}", plan.summary());
    // And then down the other side.
    assert!(plan.legs.iter().any(|l| l.kind == StepKind::Drop), "{}", plan.summary());
}

#[test]
fn the_path_over_a_pool_swims() {
    let mut app = app();
    pool_course(app.world_mut());
    settle(&mut app);
    let plan = plan_path(physics(&app), &params(), &request(Vec3::new(-5.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 0.0))).unwrap();
    let kinds: Vec<StepKind> = plan.legs.iter().map(|l| l.kind).collect();
    assert!(kinds.contains(&StepKind::Swim) && kinds.contains(&StepKind::Mantle), "{}", plan.summary());
    let text = plan.summary();
    assert!(text.contains("(swim)") && text.contains("(mantle "), "{text}");
}

/// Two decks and a pool between them, walled in so that the only way across is to swim.
pub fn pool_course(world: &mut World) {
    block(world, Vec3::new(-10.0, -4.0, -5.0), Vec3::new(0.0, 0.0, 5.0));
    block(world, Vec3::new(0.0, -4.0, -5.0), Vec3::new(12.0, -3.0, 5.0));
    volume(world, Vec3::new(0.0, -3.0, -5.0), Vec3::new(12.0, 0.0, 5.0), VolumeKind::Water);
    block(world, Vec3::new(12.0, -4.0, -5.0), Vec3::new(25.0, 0.0, 5.0));
    block(world, Vec3::new(-10.0, -4.0, -6.0), Vec3::new(25.0, 3.0, -5.0));
    block(world, Vec3::new(-10.0, -4.0, 5.0), Vec3::new(25.0, 3.0, 6.0));
}

#[test]
fn an_enclosed_goal_fails_with_what_stands_in_the_way() {
    let mut app = app();
    floor(app.world_mut());
    // A pen of 3 m walls around (0, -12).
    block(app.world_mut(), Vec3::new(-4.0, 0.0, -17.0), Vec3::new(4.0, 3.0, -16.0));
    block(app.world_mut(), Vec3::new(-4.0, 0.0, -8.0), Vec3::new(4.0, 3.0, -7.0));
    block(app.world_mut(), Vec3::new(-5.0, 0.0, -17.0), Vec3::new(-4.0, 3.0, -7.0));
    block(app.world_mut(), Vec3::new(4.0, 0.0, -17.0), Vec3::new(5.0, 3.0, -7.0));
    settle(&mut app);
    let err = plan_path(physics(&app), &params(), &request(Vec3::ZERO, Vec3::new(0.0, 0.0, -12.0))).unwrap_err();
    assert!(err.starts_with("no walkable route to (0.0, -12.0)"), "{err}");
    assert!(err.contains("a wall 3.0m high (a character climbs at most 1.6m)"), "{err}");
    assert!(err.contains("short") && err.contains("cells explored"), "{err}");
}

#[test]
fn a_goal_over_a_cliff_says_cliff() {
    let mut app = app();
    block(app.world_mut(), Vec3::new(-20.0, -10.0, -20.0), Vec3::new(0.0, 0.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -10.0, -20.0), Vec3::new(20.0, 0.0, 20.0));
    settle(&mut app);
    let err = plan_path(physics(&app), &params(), &request(Vec3::new(-5.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0))).unwrap_err();
    assert!(err.contains("no walkable route") && err.contains("a drop with no floor in reach"), "{err}");
}

#[test]
fn bad_starts_and_long_routes_are_explained() {
    let mut app = app();
    wall_with_a_gap(app.world_mut());
    settle(&mut app);
    let air = plan_path(physics(&app), &params(), &request(Vec3::new(0.0, 5.0, 0.0), Vec3::new(3.0, 0.0, 0.0))).unwrap_err();
    assert!(air.contains("5.0m above the ground") && air.contains("land first"), "{air}");
    let short = PathRequest { max_length: 10.0, ..request(Vec3::ZERO, Vec3::new(0.0, 0.0, -12.0)) };
    let err = plan_path(physics(&app), &params(), &short).unwrap_err();
    assert!(err.contains("12m away, farther than max 10m"), "{err}");
    // Near enough as the crow flies, but the way around the wall is outside a small search.
    let around = PathRequest { max_length: 20.0, ..request(Vec3::ZERO, Vec3::new(0.0, 0.0, -12.0)) };
    let err = plan_path(physics(&app), &params(), &around).unwrap_err();
    assert!(err.starts_with("no walkable route") && err.contains("raise `max` to search wider"), "{err}");
    let nan = plan_path(physics(&app), &params(), &request(Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0))).unwrap_err();
    assert!(nan.contains("finite"), "{nan}");
    let here = plan_path(physics(&app), &params(), &request(Vec3::ZERO, Vec3::new(0.2, 0.0, 0.0))).unwrap();
    assert_eq!(here.summary(), "already there");
}

fn arrived(app: &App, e: Entity, goal: Vec3, within: f32) -> bool {
    let at = feet(app, e);
    Vec2::new(at.x - goal.x, at.z - goal.z).length() <= within
}

/// Steps until `e` is within `within` of `goal`, or fails after `max` ticks.
fn walk_until_there(app: &mut App, e: Entity, goal: Vec3, within: f32, max: u64) {
    let mut ticks = 0;
    while !arrived(app, e, goal, within) {
        assert!(ticks < max, "still at {} after {ticks} ticks; orders {:?}; mode {:?}", feet(app, e), intent(app, e).orders, state(app, e).mode);
        step(app, 10);
        ticks += 10;
    }
}

#[test]
fn move_to_with_path_follows_the_route_around_the_wall() {
    let mut app = bare_engine("move_to_path");
    wall_with_a_gap(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    let goal = Vec3::new(0.0, 0.0, -12.0);

    // Straight, it ends at the wall.
    ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -12}}));
    step(&mut app, 240);
    assert!(feet(&app, e).z > -4.0 + 0.3, "stopped by the wall: {}", feet(&app, e));
    ok(&mut app, "stop", json!({}));

    let out = ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -12}, "path": true}));
    assert!(out.starts_with("ok: following path ") && out.contains("turns"), "{out}");
    assert!(intent(&app, e).orders.len() >= 2, "one order per leg");
    walk_until_there(&mut app, e, goal, 0.5, 1500);
    step(&mut app, 30);
    assert!(intent(&app, e).orders.is_empty(), "the orders are done: {:?}", intent(&app, e).orders);
    assert!(seen(&app).mantled.is_empty());
}

#[test]
fn move_to_with_path_climbs_and_swims_where_the_route_does() {
    let mut app = bare_engine("move_to_path_pool");
    pool_course(app.world_mut());
    let e = player(app.world_mut(), Vec3::new(-5.0, 0.0, 0.0));
    step(&mut app, 2);
    let out = ok(&mut app, "move_to", json!({"target": {"x": 20, "z": 0}, "path": true}));
    assert!(out.contains("(swim)") && out.contains("(mantle "), "{out}");
    walk_until_there(&mut app, e, Vec3::new(20.0, 0.0, 0.0), 0.6, 2400);
    assert!(seen(&app).modes.iter().any(|m| m.to == MotionMode::Water), "swam: {:?}", seen(&app).modes);
    assert_eq!(seen(&app).mantled.len(), 1, "climbed out once");
    assert_eq!(state(&app, e).mode, MotionMode::Ground);
}

#[test]
fn move_to_with_path_refuses_what_has_no_route_and_changes_nothing() {
    let mut app = bare_engine("move_to_path_refused");
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(-4.0, 0.0, -17.0), Vec3::new(4.0, 3.0, -16.0));
    block(app.world_mut(), Vec3::new(-4.0, 0.0, -8.0), Vec3::new(4.0, 3.0, -7.0));
    block(app.world_mut(), Vec3::new(-5.0, 0.0, -17.0), Vec3::new(-4.0, 3.0, -7.0));
    block(app.world_mut(), Vec3::new(4.0, 0.0, -17.0), Vec3::new(5.0, 3.0, -7.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "move_to", json!({"target": {"x": 5, "z": 5}}));
    let err = fails(&mut app, "move_to", json!({"target": {"x": 0, "z": -12}, "path": true}));
    assert!(err.contains("no walkable route") && err.contains("Without path=true move_to walks a straight line"), "{err}");
    assert_eq!(intent(&app, e).orders.len(), 1, "the order from before is still there");
    let _: Value = json!(null);
}
