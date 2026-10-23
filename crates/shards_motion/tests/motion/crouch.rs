//! Crouching (F2): the stance, the ceiling check, the crouch in the air, the speed, the eye, the
//! tool and the perception queries that take a stance.

use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{CROUCH, SPRINT};
use shards_motion::perception::{clearance, Hazard};
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

/// A body 1.8 m tall that crouches to 1.0 m in 0.2 s, walks crouched at 2 m/s at most, with an eye 1.6 m up
/// and no climb of its own (`mantle_height` is the step's), so that a ledge is only ever jumped.
fn crouching() -> CharacterParams {
    CharacterParams {
        crouch_height: Some(1.0),
        crouch_speed: Some(2.0),
        crouch_time: 0.2,
        eye_height: Some(1.6),
        mantle_height: 0.35,
        ..default()
    }
}

fn hero_of(app: &mut App, feet: Vec3, params: CharacterParams) -> Entity {
    let e = player(app.world_mut(), feet);
    app.world_mut().entity_mut(e).insert(params);
    step(app, 3);
    e
}

/// The capsule's height in the physics shape the body has now.
fn capsule_height(app: &App, e: Entity) -> f32 {
    match app.world().get::<Collider>(e).unwrap().shape {
        Shape::Capsule { half_height, radius } => 2.0 * (half_height + radius),
        other => panic!("not a capsule: {other:?}"),
    }
}

fn crouch(app: &App, e: Entity) -> Crouch {
    app.world().get::<Crouch>(e).copied().unwrap_or_default()
}

fn eye_above_feet(app: &App, e: Entity) -> f32 {
    eye_of(app.world(), e).unwrap().y - feet(app, e).y
}

fn hold(app: &mut App, e: Entity, on: bool) {
    edit_intent(app, e, |i| i.set_held(CROUCH, on));
}

fn walk_east(app: &mut App, e: Entity) {
    edit_intent(app, e, |i| i.move_dir = Vec3::X);
}

#[test]
fn holding_crouch_lowers_the_capsule_and_the_eye_over_crouch_time_and_letting_go_raises_them() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_of(&mut app, Vec3::ZERO, crouching());
    let rest = feet(&app, e);
    assert!((capsule_height(&app, e) - 1.8).abs() < 1e-5 && (eye_above_feet(&app, e) - 1.6).abs() < 1e-5);
    assert!(app.world().get::<Crouch>(e).is_none(), "a body that never crouched carries no stance");

    let said = ok(&mut app, "crouch", json!({}));
    assert!(said.starts_with("ok: crouch held; you go down to 1.0m over 0.20s, walking at most 2.0m/s"), "{said}");
    step(&mut app, 6);
    let half = crouch(&app, e);
    assert!(half.wanted && (0.4..0.6).contains(&half.depth), "{half:?}");
    assert!((capsule_height(&app, e) - (1.8 - 0.8 * half.depth)).abs() < 1e-4, "the capsule follows the depth");
    // The eye keeps its place in the capsule: 1.6 / 1.8 of whatever the height is now.
    assert!((eye_above_feet(&app, e) - 1.6 * capsule_height(&app, e) / 1.8).abs() < 1e-4);
    step(&mut app, 10);
    assert_eq!(crouch(&app, e), Crouch { depth: 1.0, wanted: true, blocked: false });
    assert!((capsule_height(&app, e) - 1.0).abs() < 1e-5, "{}", capsule_height(&app, e));
    assert!((eye_above_feet(&app, e) - 1.6 / 1.8).abs() < 1e-4, "{}", eye_above_feet(&app, e));
    assert!(feet(&app, e).distance(rest) < 1e-5, "on the ground the feet stay where they are");
    assert_eq!((seen(&app).crouched.len(), seen(&app).stood.len()), (1, 0));

    assert!(ok(&mut app, "crouch", json!({"hold": false})).starts_with("ok: crouch off; you stand up over"));
    step(&mut app, 16);
    assert_eq!(crouch(&app, e), Crouch::default());
    assert!((capsule_height(&app, e) - 1.8).abs() < 1e-5 && (eye_above_feet(&app, e) - 1.6).abs() < 1e-5);
    assert!(feet(&app, e).distance(rest) < 1e-5);
    assert_eq!((seen(&app).crouched.len(), seen(&app).stood.len()), (1, 1), "one of each");
}

#[test]
fn a_body_without_a_crouch_height_cannot_crouch_and_the_tool_says_why() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_of(&mut app, Vec3::ZERO, CharacterParams::default());
    let err = fails(&mut app, "crouch", json!({}));
    assert!(err.contains("cannot crouch") && err.contains("crouch_height"), "{err}");
    // The raw input does nothing either: no stance, no change of the capsule, no event.
    hold(&mut app, e, true);
    step(&mut app, 30);
    assert!(app.world().get::<Crouch>(e).is_none());
    assert!((capsule_height(&app, e) - 1.8).abs() < 1e-5);
    assert_eq!((seen(&app).crouched.len(), seen(&app).stood.len()), (0, 0));
    assert!(ok(&mut app, "crouch", json!({"hold": false})).contains("cannot crouch anyway"));
}

#[test]
fn a_crouched_body_passes_under_a_low_ceiling_and_stands_only_where_there_is_room() {
    let mut app = app();
    floor(app.world_mut());
    // A duct 6 m long, walled in on both sides, its ceiling 1.3 m up: room for the 1.0 m crouch, not for 1.8 m.
    block(app.world_mut(), Vec3::new(3.0, 1.3, -2.0), Vec3::new(9.0, 1.6, 2.0));
    block(app.world_mut(), Vec3::new(3.0, 0.0, -3.0), Vec3::new(9.0, 5.0, -2.0));
    block(app.world_mut(), Vec3::new(3.0, 0.0, 2.0), Vec3::new(9.0, 5.0, 3.0));
    let e = hero_of(&mut app, Vec3::ZERO, crouching());

    // Standing, it stops at the mouth of the duct.
    walk_east(&mut app, e);
    step(&mut app, 120);
    assert!(feet(&app, e).x < 2.7, "stopped at the duct: {}", feet(&app, e));

    // Crouched, it walks through.
    hold(&mut app, e, true);
    step(&mut app, 360);
    assert!(feet(&app, e).x > 9.5, "through: {}", feet(&app, e));
    assert_eq!(crouch(&app, e).depth, 1.0);
}

#[test]
fn standing_up_waits_while_the_space_above_is_blocked_and_happens_once_it_is_clear() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 1.3, -2.0), Vec3::new(9.0, 1.6, 2.0));
    let e = hero_of(&mut app, Vec3::new(5.0, 0.0, 0.0), CharacterParams { crouch_time: 0.0, ..crouching() });
    hold(&mut app, e, true);
    step(&mut app, 3);
    assert_eq!(crouch(&app, e).depth, 1.0, "no crouch_time: at once");
    assert_eq!(seen(&app).crouched.len(), 1);

    // Letting go under the ceiling changes nothing, and the tool and the observe line say why.
    hold(&mut app, e, false);
    step(&mut app, 30);
    assert_eq!(crouch(&app, e), Crouch { depth: 1.0, wanted: false, blocked: true });
    assert_eq!(seen(&app).stood.len(), 0, "a stand-up that did not happen fires no event");
    assert!((capsule_height(&app, e) - 1.0).abs() < 1e-5, "it never stood into the ceiling");
    let said = ok(&mut app, "crouch", json!({"hold": false}));
    assert!(said.contains("no room to stand") && said.contains("above your head") && said.contains("move out from under it"), "{said}");
    let line = ai_core::describe::facts(app.world(), e).unwrap().fragments_text();
    assert!(line.contains("crouched, no room to stand"), "{line}");

    // Walked out from under it (at a crouch's pace), it stands, once.
    walk_east(&mut app, e);
    step(&mut app, 240);
    assert!(feet(&app, e).x > 9.5, "{}", feet(&app, e));
    assert_eq!(crouch(&app, e), Crouch::default());
    assert!((capsule_height(&app, e) - 1.8).abs() < 1e-5);
    assert_eq!(seen(&app).stood.len(), 1);
}

#[test]
fn a_crouch_walk_is_capped_at_crouch_speed_and_never_a_sprint() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_of(&mut app, Vec3::ZERO, crouching());
    let speed = |app: &App| state(app, e).velocity.length();
    walk_east(&mut app, e);
    edit_intent(&mut app, e, |i| i.set_held(SPRINT, true));
    step(&mut app, 60);
    assert!(state(&app, e).sprinting && speed(&app) > 7.0, "standing, it sprints: {}", speed(&app));
    hold(&mut app, e, true);
    step(&mut app, 60);
    assert!((speed(&app) - 2.0).abs() < 0.05, "crouched: {}", speed(&app));
    assert!(!state(&app, e).sprinting, "a crouched body does not sprint");
    // Without a cap in the params, a crouch only takes the sprint away.
    let free = hero_of(&mut app, Vec3::new(0.0, 0.0, 10.0), CharacterParams { crouch_speed: None, ..crouching() });
    edit_intent(&mut app, free, |i| {
        i.move_dir = Vec3::X;
        i.set_held(CROUCH, true);
        i.set_held(SPRINT, true);
    });
    step(&mut app, 60);
    let run = state(&app, free).velocity.length();
    assert!((run - 5.0).abs() < 0.05 && !state(&app, free).sprinting, "a run, not a sprint: {run}");
}

/// A run at a ledge `top` metres high whose face is 6 m from the start, jumping 2.1 m before it; crouching
/// (when `crouch`) a few ticks into the jump. Where the feet are after 80 ticks, and the body's mode.
fn jump_at_a_ledge(top: f32, crouch: bool) -> (Vec3, MotionMode) {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(6.0, 0.0, -3.0), Vec3::new(10.0, top, 3.0));
    let e = hero_of(&mut app, Vec3::ZERO, crouching());
    walk_east(&mut app, e);
    let mut jumped_at = None;
    for tick in 0..140 {
        let x = feet(&app, e).x;
        if jumped_at.is_none() && x >= 3.9 {
            edit_intent(&mut app, e, |i| i.press("jump"));
            jumped_at = Some(tick);
        }
        if crouch && jumped_at.is_some_and(|at| tick == at + 3) {
            hold(&mut app, e, true);
        }
        step(&mut app, 1);
    }
    (feet(&app, e), state(&app, e).mode)
}

#[test]
fn a_crouch_in_the_air_pulls_the_feet_up_and_clears_a_ledge_a_plain_jump_misses() {
    // The jump peaks 1.2 m up; the ledge is 1.7 m. A plain jump meets its face.
    let (plain, _) = jump_at_a_ledge(1.7, false);
    assert!(plain.x < 6.0 && plain.y < 0.5, "a plain jump stops at the ledge: {plain}");
    // Crouched in the air the capsule loses 0.8 m from the bottom: the feet clear the edge and land on top.
    let (up, mode) = jump_at_a_ledge(1.7, true);
    assert!(up.x > 6.4 && (up.y - 1.7).abs() < 0.1 && mode == MotionMode::Ground, "on the ledge: {up} {mode:?}");
}

#[test]
fn the_air_shift_keeps_the_head_where_it_was_and_the_body_lands_on_its_crouched_feet() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_of(&mut app, Vec3::new(0.0, 6.0, 0.0), crouching());
    step(&mut app, 2);
    let head = |app: &App| feet(app, e).y + capsule_height(app, e);
    let before = head(&app);
    let falling = state(&app, e).velocity.y;
    hold(&mut app, e, true);
    step(&mut app, 13);
    // The head fell as any head does in 13 ticks (velocity v0 and gravity 20), the crouch added nothing to it.
    let t = 13.0 / 60.0;
    let fall = -(falling * t - 0.5 * 20.0 * t * t);
    assert!((before - head(&app) - fall).abs() < 0.05, "head {before} -> {}, a plain fall is {fall}", head(&app));
    assert_eq!(crouch(&app, e).depth, 1.0);
    // It lands on its crouched feet.
    step(&mut app, 120);
    assert_eq!(state(&app, e).mode, MotionMode::Ground);
    assert!(feet(&app, e).y < 0.1, "{}", feet(&app, e));
    assert!((capsule_height(&app, e) - 1.0).abs() < 1e-5);
}

#[test]
fn clearance_and_path_to_measure_the_world_with_the_asked_stance() {
    let mut app = app();
    floor(app.world_mut());
    // The duct again, and walls that leave no way around it within the planner's 80 m.
    block(app.world_mut(), Vec3::new(3.0, 1.3, -2.0), Vec3::new(9.0, 1.6, 2.0));
    block(app.world_mut(), Vec3::new(3.0, 0.0, -60.0), Vec3::new(9.0, 5.0, -2.0));
    block(app.world_mut(), Vec3::new(3.0, 0.0, 2.0), Vec3::new(9.0, 5.0, 60.0));
    let e = hero_of(&mut app, Vec3::ZERO, crouching());
    let physics = app.world().resource::<PhysicsWorld>();
    let params = crouching();
    let east = |params: &CharacterParams| {
        let reaches = clearance(physics, params, Vec3::ZERO, None, 15.0, 2.0, &[e]);
        reaches.into_iter().find(|r| r.dir == "E").unwrap()
    };
    let (standing, ducked) = (east(&params), east(&params.at_depth(1.0)));
    assert_eq!(standing.hazard, Hazard::Wall, "{standing:?}");
    assert!((standing.distance - 2.6).abs() < 0.1, "{standing:?}");
    assert_eq!((ducked.hazard, ducked.distance), (Hazard::Free, 15.0), "the crouched body goes the whole way");

    // The same through the tools: `stance` picks the size, and `current` (the default) is the body's own.
    let line = |app: &mut App, stance: &str| ok(app, "clearance", json!({"stance": stance}));
    let (up, down) = (line(&mut app, "standing"), line(&mut app, "crouched"));
    assert_ne!(up, down);
    assert_eq!(ok(&mut app, "clearance", json!({})), up, "standing now, so current is standing");
    let err = fails(&mut app, "clearance", json!({"stance": "crawling"}));
    assert!(err.contains("crawling") || err.contains("stance"), "{err}");

    let route = fails(&mut app, "path_to", json!({"target": {"x": 14, "z": 0}, "stance": "standing"}));
    assert!(route.contains("no route") || route.contains("route"), "standing finds no way through: {route}");
    let plan = call(&mut app, "path_to", json!({"target": {"x": 14, "z": 0}, "stance": "crouched"}));
    assert!(plan.ok, "{}", plan.result);
    assert_eq!(plan.result["provider"], json!("local grid"));
    // A crouched body is asked for as it is when it is: current follows the stance.
    hold(&mut app, e, true);
    step(&mut app, 20);
    let now = call(&mut app, "path_to", json!({"target": {"x": 14, "z": 0}}));
    assert!(now.ok, "the crouched body's own route: {}", now.result);
}

#[test]
fn a_crouched_stance_that_a_body_cannot_have_is_refused_by_the_queries() {
    let mut app = app();
    floor(app.world_mut());
    hero_of(&mut app, Vec3::ZERO, CharacterParams::default());
    let err = fails(&mut app, "clearance", json!({"stance": "crouched"}));
    assert!(err.contains("cannot crouch") && err.contains("crouch_height"), "{err}");
    let err = fails(&mut app, "path_to", json!({"target": {"x": 5, "z": 0}, "stance": "crouched"}));
    assert!(err.contains("cannot crouch"), "{err}");
}

#[test]
fn the_stance_is_reflected_so_snapshots_and_replays_hold_it() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_of(&mut app, Vec3::ZERO, crouching());
    hold(&mut app, e, true);
    step(&mut app, 20);
    let registry = app.world().resource::<AppTypeRegistry>().read();
    assert!(registry.get(std::any::TypeId::of::<Crouch>()).is_some_and(|t| t.data::<ReflectComponent>().is_some()));
}
