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
    let reaches = clearance(physics(&app), &params(), Vec3::ZERO, None, 15.0, 2.0, &[]);
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
fn the_tools_see_colliders_spawned_or_moved_in_the_same_breath() {
    // Scenario runners call tools back to back, with no frame in between to sync physics.
    let mut app = bare_engine("fresh_physics");
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);

    let wall = block(app.world_mut(), Vec3::new(-2.0, 0.0, -4.0), Vec3::new(2.0, 3.0, -3.0));
    let out = ok(&mut app, "clearance", json!({}));
    assert!(out.starts_with("N wall 2.6m"), "a wall that was just spawned: {out}");
    app.world_mut().get_mut::<Transform>(wall).unwrap().translation.z = -9.5;
    let out = ok(&mut app, "clearance", json!({}));
    assert!(out.starts_with("N wall 8.6m") || out.starts_with("N wall 8.5m"), "a wall that was just moved: {out}");

    // A pen of walls round the goal, just built: no route, instead of a straight line through them.
    block(app.world_mut(), Vec3::new(-6.0, 0.0, -8.0), Vec3::new(6.0, 3.0, -7.0));
    block(app.world_mut(), Vec3::new(-6.0, 0.0, -15.0), Vec3::new(6.0, 3.0, -14.0));
    block(app.world_mut(), Vec3::new(-7.0, 0.0, -15.0), Vec3::new(-6.0, 3.0, -7.0));
    block(app.world_mut(), Vec3::new(6.0, 0.0, -15.0), Vec3::new(7.0, 3.0, -7.0));
    let err = fails(&mut app, "path_to", json!({"target": {"x": 0, "z": -11}}));
    assert!(err.starts_with("no walkable route"), "{err}");
    let err = fails(&mut app, "move_to", json!({"target": {"x": 0, "z": -11}, "path": true}));
    assert!(err.starts_with("no walkable route"), "{err}");
    let _ = e;
}

#[test]
fn clearance_around_a_prop_does_not_see_the_prop_as_a_wall() {
    let mut app = app();
    floor(app.world_mut());
    player(app.world_mut(), Vec3::new(-8.0, 0.0, 0.0));
    // A crate is on the world layer, like the walls: it must not wall in its own surroundings.
    let crate_ = app
        .world_mut()
        .spawn((ai_core::AiKind::new("Crate"), Collider::standing_box(Vec3::splat(0.5)), Transform::from_xyz(5.0, 0.0, 0.0)))
        .id();
    let id = app.world().get::<ai_core::AiId>(crate_).unwrap().to_string();
    step(&mut app, 2);
    let around = ok(&mut app, "clearance", json!({"around": id}));
    let line = around.split(": ").nth(1).unwrap();
    assert!(line.split(" · ").all(|part| part.ends_with("free 15.0m")), "{around}");
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
    let reaches = clearance(physics(&app), &params(), Vec3::ZERO, None, 15.0, 2.0, &[]);
    let w = reach(&reaches, "W");
    assert!(matches!(w.hazard, Hazard::Ledge { rise } if (rise - 1.1).abs() < 0.05), "{w:?}");
    assert!((w.distance - 3.6).abs() < 0.05, "{w:?}");
    assert_eq!(w.describe(), "W ledge 3.6m (+1.1m)");
    assert_eq!(reach(&reaches, "E").hazard, Hazard::Wall);
}

/// A 3 m high wall 0.4 m thick at x = 3 across the whole way east, and what lies behind it.
fn thin_wall_with_something_behind(world: &mut World, behind: &str) {
    block(world, Vec3::new(-200.0, -1.0, -200.0), Vec3::new(3.0, 0.0, 200.0));
    block(world, Vec3::new(3.0, -6.0, -200.0), Vec3::new(3.4, 3.0, 200.0));
    match behind {
        "moat" => {
            block(world, Vec3::new(3.4, -6.0, -200.0), Vec3::new(200.0, -4.0, 200.0));
            volume(world, Vec3::new(3.4, -4.0, -200.0), Vec3::new(200.0, -0.5, 200.0), VolumeKind::Water);
        }
        "terrace" => {
            block(world, Vec3::new(3.4, -1.0, -200.0), Vec3::new(200.0, 1.0, 200.0));
        }
        "cliff" => {
            block(world, Vec3::new(3.4, -14.0, -200.0), Vec3::new(200.0, -12.0, 200.0));
        }
        other => panic!("nothing like {other}"),
    }
}

#[test]
fn a_tall_thin_wall_is_a_wall_whatever_lies_behind_it() {
    for behind in ["moat", "terrace", "cliff"] {
        let mut app = app();
        thin_wall_with_something_behind(app.world_mut(), behind);
        settle(&mut app);
        // However the 0.5 m probes fall against the wall's face.
        for offset in [0.0, 0.1, 0.2, 0.3, 0.4] {
            let reaches = clearance(physics(&app), &params(), Vec3::new(offset, 0.0, 0.0), None, 15.0, 2.0, &[]);
            let e = reach(&reaches, "E");
            assert_eq!(e.hazard, Hazard::Wall, "behind it: {behind}, from x = {offset}: {e:?}");
            assert!((e.distance - (2.6 - offset)).abs() < 0.05, "{behind}: {e:?}");
        }
    }
}

#[test]
fn the_planner_does_not_hop_a_tall_thin_wall_onto_what_is_behind_it() {
    for behind in ["moat", "terrace", "cliff"] {
        let mut app = app();
        thin_wall_with_something_behind(app.world_mut(), behind);
        settle(&mut app);
        for offset in [0.0, 0.1, 0.2, 0.3, 0.4] {
            let request = PathRequest { max_length: 400.0, ..request(Vec3::new(offset, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0)) };
            let plan = plan_path(physics(&app), &params(), &request);
            assert!(plan.is_err(), "behind it: {behind}, from x = {offset}: {:?}", plan.map(|p| p.summary()));
        }
    }
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

/// A 1 m pillar in the open, 5 m north of the origin.
fn pillar_in_the_open(world: &mut World) {
    floor(world);
    block(world, Vec3::new(-0.5, 0.0, -5.5), Vec3::new(0.5, 3.0, -4.5));
}

/// Whether the straight lines between `points` come nearer than `gap` to the pillar of
/// [`pillar_in_the_open`].
fn cuts_the_pillar(points: &[Vec3], gap: f32) -> Option<Vec3> {
    points.windows(2).find_map(|w| {
        let len = Vec2::new(w[1].x - w[0].x, w[1].z - w[0].z).length();
        (0..=(len / 0.05).ceil() as usize)
            .map(|k| w[0].lerp(w[1], k as f32 * 0.05 / len.max(1e-3)))
            .find(|p| p.x.abs() < 0.5 + gap && p.z > -5.5 - gap && p.z < -4.5 + gap)
    })
}

#[test]
fn a_path_around_a_pillar_keeps_every_bend_of_the_route() {
    let mut app = app();
    pillar_in_the_open(app.world_mut());
    settle(&mut app);
    let goal = Vec3::new(0.0, 0.0, -10.0);
    let plan = plan_path(physics(&app), &params(), &request(Vec3::ZERO, goal)).unwrap();
    assert!(plan.length > 10.1 && plan.length < 11.5, "round the pillar, not through it: {}", plan.summary());
    let mut points = vec![Vec3::ZERO];
    points.extend(plan.legs.iter().map(|l| l.end));
    assert_eq!(cuts_the_pillar(&points, 0.3), None, "the legs' ends are the route: {}", plan.summary());
    // Both legs head "N" (45° octants) but they are not one line: the bend is the way round.
    assert!(plan.legs.len() >= 2, "out to the side of the pillar and back in: {}", plan.summary());
}

#[test]
fn move_to_with_path_walks_round_a_pillar() {
    let mut app = bare_engine("move_to_path_pillar");
    pillar_in_the_open(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -10}, "path": true}));
    walk_until_there(&mut app, e, Vec3::new(0.0, 0.0, -10.0), 0.5, 1200);
    step(&mut app, 30);
    assert!(intent(&app, e).orders.is_empty(), "the orders are done: {:?}", intent(&app, e).orders);
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
    // No navmesh here: the local grid planned it.
    assert_eq!(out.result["provider"], "local grid");
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

/// A ledge `height` m high and 2 m deep across the whole way, 5 m north of the origin.
fn ledge_across_the_way(world: &mut World, height: f32) {
    floor(world);
    block(world, Vec3::new(-60.0, 0.0, -7.0), Vec3::new(60.0, height, -5.0));
}

#[test]
fn the_planner_climbs_what_the_controller_climbs_and_nothing_higher() {
    // Mantle height is 1.6 m. The controller refuses anything above it (to a thousandth),
    // so a route over a 1.62 m ledge would be a walker standing at its foot for good.
    let goal = Vec3::new(0.0, 0.0, -10.0);
    for (height, climbs) in [(1.58, true), (1.62, false)] {
        let mut app = bare_engine("climb_rule");
        ledge_across_the_way(app.world_mut(), height);
        let e = player(app.world_mut(), Vec3::ZERO);
        step(&mut app, 2);
        let plan = plan_path(physics(&app), &params(), &request(Vec3::ZERO, goal));
        match plan {
            Ok(plan) => assert!(climbs && plan.summary().contains("(mantle "), "{height} m: planned {}", plan.summary()),
            Err(err) => assert!(!climbs && err.contains(&format!("{height:.2}m high (a character climbs at most 1.60m)")), "{height} m: {err}"),
        }
        // What the walker does, straight at it.
        ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -10}}));
        step(&mut app, 300);
        assert_eq!(seen(&app).mantled.len(), usize::from(climbs), "{height} m: the controller {:?}", feet(&app, e));
    }
}

/// A pool whose bank is a ledge with its top `top` m above the surface (y = 0).
fn pool_with_a_bank(world: &mut World, top: f32) {
    block(world, Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(world, Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, top, 20.0));
    volume(world, Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
}

#[test]
fn a_swimmer_climbs_out_where_the_planner_and_the_jump_tool_say_it_can() {
    // A swimmer floats 1.1 m below the surface and the controller adds its skin to the
    // rise: a bank 0.5 m over the water is 1.62 m up, 0.45 m is 1.57 m.
    for (top, climbs) in [(0.45, true), (0.5, false)] {
        let mut app = app();
        pool_with_a_bank(app.world_mut(), top);
        let e = player(app.world_mut(), Vec3::new(3.4, -1.1, 0.0));
        app.world_mut().get_mut::<Transform>(e).unwrap().look_to(Vec3::X, Vec3::Y);
        step(&mut app, 30);
        assert_eq!(state(&app, e).mode, MotionMode::Water);
        let start = Vec3::new(3.0, -1.1, 0.0);
        let request = PathRequest { swimming_at: Some(0.0), ..request(start, Vec3::new(7.0, 0.0, 0.0)) };
        let plan = plan_path(physics(&app), &params(), &request);
        let jump = call(&mut app, "jump", json!({}));
        step(&mut app, 60);
        let climbed = !seen(&app).mantled.is_empty();
        assert_eq!(climbed, climbs, "bank {top} m over the water: the controller");
        assert_eq!(plan.is_ok(), climbs, "bank {top} m over the water: the planner said {plan:?}");
        assert_eq!(jump.ok, climbs, "bank {top} m over the water: the jump tool said {}", jump.result);
    }
}

/// Ground tilted 3.6° about the x axis (so it is level going east, and not flat), and a
/// 0.8 m high, 2 m deep ledge across the way east at x = 5.
fn tilted_ground_with_a_ledge(world: &mut World) {
    turned_block(world, Vec3::new(0.0, -0.5, 0.0), Vec3::new(200.0, 0.5, 200.0), Quat::from_rotation_x(3.6f32.to_radians()));
    block(world, Vec3::new(5.0, -3.0, -20.0), Vec3::new(7.0, 0.8, 20.0));
}

#[test]
fn a_gentle_slope_does_not_make_a_ledge_walkable() {
    // The slope between two floors lifts one over the other by the tilt, not by the steepest
    // slope a character can walk: ground a few degrees off flat is no excuse for a 0.8 m step.
    let mut app = bare_engine("tilted_ledge");
    tilted_ground_with_a_ledge(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    let reaches = clearance(physics(&app), &params(), feet(&app, e), None, 15.0, 2.0, &[]);
    let east = reach(&reaches, "E");
    assert!(matches!(east.hazard, Hazard::Ledge { rise } if (rise - 0.8).abs() < 0.05), "{east:?}");
    let plan = plan_path(physics(&app), &params(), &request(feet(&app, e), Vec3::new(10.0, 0.0, 0.0))).unwrap();
    assert!(plan.summary().contains("(mantle 0.8m)"), "{}", plan.summary());

    ok(&mut app, "move_to", json!({"target": {"x": 10, "z": 0}, "path": true}));
    walk_until_there(&mut app, e, Vec3::new(10.0, 0.8, 0.0), 0.5, 900);
    assert_eq!(seen(&app).mantled.len(), 1, "climbed once, as planned");
}

/// Ground, a 16.7° ramp up 3 m over 10 m, and a platform on top.
fn ramp_course(world: &mut World) {
    block(world, Vec3::new(-30.0, -1.0, -6.0), Vec3::new(0.0, 0.0, 6.0));
    let angle = ops::atan(0.3f32);
    let normal = Vec3::new(-ops::sin(angle), ops::cos(angle), 0.0);
    let middle = Vec3::new(5.0, 1.5, 0.0) - normal * 0.25;
    turned_block(world, middle, Vec3::new(5.22, 0.25, 6.0), Quat::from_rotation_z(angle));
    block(world, Vec3::new(10.0, -1.0, -6.0), Vec3::new(30.0, 3.0, 6.0));
}

#[test]
fn a_ramp_is_walked_not_climbed() {
    let mut app = bare_engine("ramp");
    ramp_course(app.world_mut());
    let e = player(app.world_mut(), Vec3::new(-5.0, 0.0, 0.0));
    app.update();
    let plan = plan_path(physics(&app), &params(), &request(Vec3::new(-5.0, 0.0, 0.0), Vec3::new(18.0, 3.0, 0.0))).unwrap();
    assert!(plan.legs.iter().all(|l| l.kind == StepKind::Walk), "a slope under the limit is ground: {}", plan.summary());
    assert!(plan.length > 22.9 && plan.length < 25.0, "{}", plan.summary());

    ok(&mut app, "move_to", json!({"target": {"x": 18, "z": 0}, "path": true}));
    walk_until_there(&mut app, e, Vec3::new(18.0, 3.0, 0.0), 0.6, 1200);
    assert!((feet(&app, e).y - 3.0).abs() < 0.1, "up on the platform: {}", feet(&app, e));
    assert!(seen(&app).mantled.is_empty() && seen(&app).jumped.is_empty());
}

#[test]
fn deep_water_over_a_steep_bottom_still_swims() {
    let mut app = app();
    // A pool whose floor is a 60° ramp: no ground to walk on, 3 m of water at the deep end.
    block(app.world_mut(), Vec3::new(-20.0, -10.0, -10.0), Vec3::new(0.0, 0.0, 10.0));
    let angle = 60f32.to_radians();
    turned_block(app.world_mut(), Vec3::new(2.5, -2.0, 0.0), Vec3::new(3.0, 0.2, 10.0), Quat::from_rotation_z(-angle));
    volume(app.world_mut(), Vec3::new(0.0, -6.0, -10.0), Vec3::new(10.0, 0.0, 10.0), VolumeKind::Water);
    settle(&mut app);
    let params = CharacterParams::default();
    let terrain = shards_motion::perception::Terrain::new(physics(&app), &params, &[]);
    // Over the ramp where the water is deeper than a character can stand in.
    let probed = terrain.probe(4.0, 0.0, 0.0);
    assert!(matches!(probed, shards_motion::perception::Probe::Water { .. }), "{probed:?}");
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
fn move_to_with_path_stops_within_the_distance_asked_not_twice_that() {
    let mut app = bare_engine("move_to_path_within");
    wall_with_a_gap(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    let marker = app.world_mut().spawn((ai_core::AiKind::new("Marker"), Transform::from_xyz(0.0, 0.0, -12.0))).id();
    let id = app.world().get::<ai_core::AiId>(marker).unwrap().to_string();
    step(&mut app, 2);
    // An entity: within 1.5 m by default. The plan's last leg already ends that close; the
    // walk must not stop another 1.5 m short of where that leg ends.
    ok(&mut app, "move_to", json!({"target": id, "path": true}));
    step(&mut app, 900);
    assert!(intent(&app, e).orders.is_empty(), "the orders are done: {:?}", intent(&app, e).orders);
    let left = Vec2::new(feet(&app, e).x, feet(&app, e).z + 12.0).length();
    assert!(left <= 1.5 + 0.05 && left > 0.9, "stops 1.5 m from the marker, not 3 m: {left}");
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

#[test]
fn move_to_with_path_and_queue_is_refused_while_orders_are_pending() {
    let mut app = bare_engine("move_to_path_queue");
    wall_with_a_gap(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    // Nothing pending: the route starts where the walker stands, so it can be queued.
    let first = ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -12}, "path": true, "queue": true}));
    assert!(first.starts_with("ok: following path "), "{first}");
    let legs = intent(&app, e).orders.len();

    // Orders pending: a route planned from here would not start where they end.
    let second = fails(&mut app, "move_to", json!({"target": {"x": 20, "z": -12}, "path": true, "queue": true}));
    assert!(second.contains("pending") && second.contains("path=true") && second.contains("without `queue`"), "{second}");
    assert_eq!(intent(&app, e).orders.len(), legs, "a refusal changes nothing");
    // Plain queued orders are straight lines from wherever the last one ends: still fine.
    ok(&mut app, "move_to", json!({"target": {"x": 20, "z": -12}, "queue": true}));
    assert_eq!(intent(&app, e).orders.len(), legs + 1);
}

#[test]
fn move_to_with_path_already_in_range_stops_a_walk_under_way() {
    let mut app = bare_engine("move_to_path_in_range");
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -40}}));
    step(&mut app, 40);
    assert!(intent(&app, e).move_dir != Vec3::ZERO, "on its way");

    // Somebody stands a meter ahead: nothing to plan, and the walk to the old goal ends here.
    let at = feet(&app, e);
    let near = app.world_mut().spawn((ai_core::AiKind::new("Dummy"), Transform::from_translation(at + Vec3::new(0.0, 0.0, -1.0)))).id();
    let id = app.world().get::<ai_core::AiId>(near).unwrap().to_string();
    let out = ok(&mut app, "move_to", json!({"target": id, "path": true}));
    assert!(out.starts_with("ok: already within"), "{out}");
    assert!(intent(&app, e).orders.is_empty());
    assert_eq!(intent(&app, e).move_dir, Vec3::ZERO, "the old walk is over with its orders");
    step(&mut app, 120);
    let stopped = feet(&app, e);
    assert!(Vec2::new(stopped.x - at.x, stopped.z - at.z).length() < 1.0, "it kept running: {at} -> {stopped}");
}


/// The frontier game's `course` arena: the floor in pieces round a basin (4 m thick, so a ray
/// down reports the floor's height with float noise), standing boxes, tilted ramps, a fence.
fn course_arena(world: &mut World) {
    let thick = 4.0f32;
    let (xs, zs) = ([-30.0f32, 13.0, 23.0, 30.0], [-30.0f32, 8.0, 16.0, 30.0]);
    for xw in xs.windows(2) {
        for zw in zs.windows(2) {
            if xw[0] == 13.0 && zw[0] == 8.0 {
                continue; // the basin
            }
            let (hx, hz) = ((xw[1] - xw[0]) * 0.5, (zw[1] - zw[0]) * 0.5);
            world.spawn((Collider::new(Shape::cuboid(Vec3::new(hx, thick * 0.5, hz))), Transform::from_xyz(xw[0] + hx, -thick * 0.5, zw[0] + hz)));
        }
    }
    world.spawn((Collider::new(Shape::cuboid(Vec3::new(5.0, 0.5, 4.0))), Transform::from_xyz(18.0, -3.5, 12.0)));
    for (x, z, hx, hz) in [(0.0, -30.0, 30.0, 0.5), (0.0, 30.0, 30.0, 0.5), (-30.0, 0.0, 0.5, 30.0), (30.0, 0.0, 0.5, 30.0)] {
        world.spawn((Collider::standing_box(Vec3::new(hx, 2.0, hz)), Transform::from_xyz(x, 0.0, z)));
    }
    let boxes = [(-8.0, 8.0, 1.5, 0.5, 1.5), (-3.0, 8.0, 1.5, 0.65, 1.5), (2.0, 8.0, 1.5, 0.8, 1.5), (7.0, 8.0, 1.5, 1.0, 1.5), (-14.0, -2.0, 3.0, 0.5, 3.0)]
        .into_iter()
        .chain([(-4.0, -4.0, 3.0, 1.0, 3.0), (-4.0, 0.5, 1.5, 0.5, 1.5), (8.0, 2.0, 2.0, 0.625, 1.5), (8.0, -1.0, 2.0, 1.25, 1.5), (8.0, -4.0, 2.0, 1.875, 1.5), (8.0, -7.0, 2.0, 2.5, 1.5)]);
    for (x, z, hx, hy, hz) in boxes {
        world.spawn((Collider::standing_box(Vec3::new(hx, hy, hz)), Transform::from_xyz(x, 0.0, z)));
    }
    for (x, z, deg) in [(-8.0f32, 16.0f32, 20.0f32), (0.0, 16.0, 40.0), (8.0, 16.0, 55.0)] {
        let rise = deg.to_radians();
        let mid = 3.0 * ops::sin(rise) - 0.25 * ops::cos(rise);
        world.spawn((Collider::new(Shape::cuboid(Vec3::new(1.5, 0.25, 3.0))), Transform::from_xyz(x, mid, z).with_rotation(Quat::from_rotation_x(rise))));
    }
}

#[test]
fn a_body_with_no_step_height_sees_flat_floor_as_free_not_as_ledges() {
    // `step_height: 0` (no auto step): the float noise of a ray's hit height used to read as a
    // ledge of 0.0 m in every direction ("ledge 0.5m (+0.0m)") at some spots of a thick floor.
    let mut app = app();
    course_arena(app.world_mut());
    settle(&mut app);
    let p = CharacterParams { radius: 0.4, height: 1.7, step_height: 0.0, mantle_height: 1.3, skin_width: 0.001, ..default() };
    for (x, z) in [(4.9, 0.0), (4.6, 0.0), (0.0, 22.0), (0.0, 0.0), (2.0, 3.0), (5.5, 0.0), (-3.0, -10.0)] {
        let reaches = clearance(physics(&app), &p, Vec3::new(x, 0.0, z), None, 15.0, 2.0, &[]);
        let line = reaches.iter().map(|r| r.describe()).collect::<Vec<_>>().join(" · ");
        for r in &reaches {
            assert!(!matches!(r.hazard, Hazard::Ledge { rise } if rise < 0.05), "at ({x}, {z}): {line}");
            assert!(r.distance > 0.09 || r.hazard == Hazard::Wall, "at ({x}, {z}): {line}");
        }
    }
    let at = |x, z| clearance(physics(&app), &p, Vec3::new(x, 0.0, z), None, 15.0, 2.0, &[]);
    assert_eq!(reach(&at(0.0, 22.0), "W"), &perception::Reach { dir: "W", distance: 15.0, hazard: Hazard::Free });
    let e = *reach(&at(4.9, 0.0), "E");
    assert_eq!(e.hazard, Hazard::Wall);
    assert!((e.distance - 0.7).abs() < 0.05, "{e:?}");
    // With no drop safe at all (`safe_drop: 0`) the same noise is no cliff either: the
    // tolerance holds downward too.
    for (x, z) in [(4.9, 0.0), (4.6, 0.0), (0.0, 22.0), (0.0, 0.0), (2.0, 3.0), (5.5, 0.0), (-3.0, -10.0)] {
        let reaches = clearance(physics(&app), &p, Vec3::new(x, 0.0, z), None, 15.0, 0.0, &[]);
        let line = reaches.iter().map(|r| r.describe()).collect::<Vec<_>>().join(" · ");
        for r in &reaches {
            assert!(!matches!(r.hazard, Hazard::Cliff { drop: Some(d) } if d < 0.05), "at ({x}, {z}): {line}");
        }
    }
}
