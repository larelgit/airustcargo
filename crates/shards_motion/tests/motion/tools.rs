//! The movement tools: `jump`, `sprint`, `glide`, `move_to`, `move`, `stop`, and their errors.

use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{GLIDE, SPRINT};
use shards_core::{Order, Target};
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

#[test]
fn jump_works_where_a_jump_works_and_explains_where_it_does_not() {
    let mut app = app();
    // Ground to x = 20, then a pool 5 m deep whose far wall is too high to climb out of.
    block(app.world_mut(), Vec3::new(-50.0, -1.0, -20.0), Vec3::new(20.0, 0.0, 20.0));
    block(app.world_mut(), Vec3::new(20.0, -6.0, -20.0), Vec3::new(40.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(40.0, -6.0, -20.0), Vec3::new(41.0, 4.0, 20.0));
    volume(app.world_mut(), Vec3::new(20.0, -5.0, -20.0), Vec3::new(40.0, 0.0, 20.0), VolumeKind::Water);
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);

    assert_eq!(ok(&mut app, "jump", json!({})), "ok: jumping");
    step(&mut app, 12);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    assert_eq!(seen(&app).jumped.len(), 1);
    let late = ok(&mut app, "jump", json!({}));
    assert!(late.contains("only works within") && late.contains("right before landing"), "{late}");
    step(&mut app, 80);
    assert_eq!(state(&app, e).mode, MotionMode::Ground);

    app.world_mut().get_mut::<Transform>(e).unwrap().translation = Vec3::new(30.0, -1.2, 0.0);
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    let swim = fails(&mut app, "jump", json!({}));
    assert!(swim.contains("cannot jump while swimming") && swim.contains("move_to to climb out"), "{swim}");
    assert_eq!(intent(&app, e).pressed.len(), 0, "a refused jump presses nothing");
}

#[test]
fn jump_in_water_next_to_a_ledge_climbs_out() {
    let mut app = app();
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, 0.3, 20.0));
    volume(app.world_mut(), Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
    let e = player(app.world_mut(), Vec3::new(3.4, -1.1, 0.0));
    app.world_mut().get_mut::<Transform>(e).unwrap().look_to(Vec3::X, Vec3::Y);
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    let out = ok(&mut app, "jump", json!({}));
    assert!(out.starts_with("ok: jumping out of the water onto the ledge"), "{out}");
    step(&mut app, 60);
    assert_eq!(seen(&app).mantled.len(), 1);
}

#[test]
fn sprint_toggles_the_held_button() {
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    assert!(ok(&mut app, "sprint", json!({})).starts_with("ok: sprint on"));
    assert!(intent(&app, e).is_held(SPRINT));
    assert_eq!(ok(&mut app, "sprint", json!({"on": false})), "ok: sprint off");
    assert!(!intent(&app, e).is_held(SPRINT));
}

#[test]
fn a_glide_asked_on_the_ground_opens_when_you_fall_and_ends_when_you_land() {
    let mut app = bare_engine("glide_latch");
    // A ledge at the top, a canyon floor 12 m below.
    block(app.world_mut(), Vec3::new(-50.0, -1.0, -50.0), Vec3::new(5.0, 0.0, 50.0));
    block(app.world_mut(), Vec3::new(-50.0, -13.0, -50.0), Vec3::new(100.0, -12.0, 50.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    let armed = ok(&mut app, "glide", json!({}));
    assert!(armed.contains("armed"), "{armed}");
    assert!(intent(&app, e).is_held(GLIDE));
    ok(&mut app, "move", json!({"dir": "E", "meters": 14}));
    let mut modes = Vec::new();
    for _ in 0..900 {
        step(&mut app, 1);
        let mode = state(&app, e).mode;
        if modes.last() != Some(&mode) {
            modes.push(mode);
        }
        if mode == MotionMode::Ground && modes.len() > 2 {
            break;
        }
    }
    assert_eq!(modes, [MotionMode::Ground, MotionMode::Air, MotionMode::Glide, MotionMode::Ground]);
    step(&mut app, 3);
    assert!(!intent(&app, e).is_held(GLIDE), "landing lets go of a glide the tool held");
    assert!(app.world().get::<GlideUntilLanded>(e).is_none());
    let landed = seen(&app).landed.last().copied().unwrap();
    assert!(landed.impact_speed < 3.0, "a glider lands softly: {landed:?}");
    assert!(landed.fall_height > 11.0, "{landed:?}");

    // `on=false` lets go in the air too.
    app.world_mut().get_mut::<Transform>(e).unwrap().translation = Vec3::new(10.0, 30.0, 0.0);
    step(&mut app, 30);
    ok(&mut app, "glide", json!({}));
    assert!(ok(&mut app, "glide", json!({"on": false})).contains("off"));
    assert!(!intent(&app, e).is_held(GLIDE) && app.world().get::<GlideUntilLanded>(e).is_none());
}

#[test]
fn glide_is_refused_in_water_and_jump_while_gliding() {
    let mut app = app();
    block(app.world_mut(), Vec3::new(-50.0, -13.0, -50.0), Vec3::new(50.0, -12.0, 50.0));
    volume(app.world_mut(), Vec3::new(-50.0, -12.0, -50.0), Vec3::new(50.0, -2.0, 50.0), VolumeKind::Water);
    let e = player(app.world_mut(), Vec3::new(0.0, 20.0, 0.0));
    step(&mut app, 20);
    ok(&mut app, "glide", json!({}));
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Glide);
    let jump = fails(&mut app, "jump", json!({}));
    assert!(jump.contains("cannot jump while gliding") && jump.contains("glide on=false"), "{jump}");
    ok(&mut app, "glide", json!({"on": false}));
    step(&mut app, 600);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    let glide = fails(&mut app, "glide", json!({}));
    assert!(glide.contains("cannot glide in water"), "{glide}");
}

#[test]
fn move_to_walks_move_goes_a_distance_and_stop_cancels() {
    let mut app = bare_engine("move_to");
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    let other = app.world_mut().spawn((ai_core::AiKind::new("Dummy"), Transform::from_xyz(8.0, 0.0, 0.0))).id();
    let other_id = *app.world().get::<AiId>(other).unwrap();
    step(&mut app, 3);

    let out = ok(&mut app, "move_to", json!({"target": other_id.to_string(), "within": 1.0}));
    assert_eq!(out, "ok: move_to e2, 8.0m E away");
    assert_eq!(intent(&app, e).orders.len(), 1);
    step(&mut app, 200);
    assert!(feet(&app, e).distance(Vec3::new(8.0, 0.0, 0.0)) <= 1.2, "{}", feet(&app, e));
    assert!(intent(&app, e).orders.is_empty(), "arrival pops the order");

    ok(&mut app, "move", json!({"dir": "N", "meters": 6}));
    step(&mut app, 200);
    assert!((feet(&app, e).z + 6.0).abs() < 0.5, "{}", feet(&app, e));

    ok(&mut app, "move_to", json!({"target": {"x": -20, "z": 0}}));
    ok(&mut app, "move_to", json!({"target": {"x": -20, "z": 10}, "queue": true}));
    assert_eq!(intent(&app, e).orders.len(), 2, "queue appends");
    assert_eq!(ok(&mut app, "stop", json!({})), "ok: stopped");
    assert!(intent(&app, e).orders.is_empty() && intent(&app, e).move_dir == Vec3::ZERO);
}

#[test]
fn movement_tools_teach_the_fix() {
    let mut app = bare_engine("tools_teach");
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    let me = *app.world().get::<AiId>(e).unwrap();
    step(&mut app, 1);
    let itself = fails(&mut app, "move_to", json!({"target": me.to_string()}));
    assert!(itself.contains("is you; give another entity or a point"), "{itself}");
    let gone = fails(&mut app, "move_to", json!({"target": "e99"}));
    assert!(gone.contains("e99 does not exist"), "{gone}");
    // JSON has no infinity; a number that big arrives as null, which is no number.
    let nan = fails(&mut app, "move_to", json!({"target": {"x": f64::INFINITY, "z": 0}}));
    assert!(nan.contains("bad arguments"), "{nan}");
    let dir = fails(&mut app, "move", json!({"dir": "up", "meters": 3}));
    assert!(dir.contains("unknown direction `up`") && dir.contains("NW"), "{dir}");
    let far = fails(&mut app, "move", json!({"dir": "N", "meters": -1}));
    assert!(far.contains("positive"), "{far}");

    // A body without Intent or controller says what is missing.
    let rock = app.world_mut().spawn((ai_core::AiKind::new("Rock"), Transform::default())).id();
    let rock_id = *app.world().get::<AiId>(rock).unwrap();
    ai_core::call_tool(app.world_mut(), "dev", "possess", json!({"id": rock_id.to_string()}));
    let stuck = fails(&mut app, "move_to", json!({"target": {"x": 1, "z": 1}}));
    assert!(stuck.contains("no Intent"), "{stuck}");
    let nojump = fails(&mut app, "jump", json!({}));
    assert!(nojump.contains("cannot jump") && nojump.contains("CharacterController"), "{nojump}");

    // An order for a point keeps its meaning for the engine's own order follower.
    ai_core::call_tool(app.world_mut(), "dev", "possess", json!({}));
    ok(&mut app, "move_to", json!({"target": {"x": 3, "z": 4}}));
    assert_eq!(intent(&app, e).orders[0], Order::MoveTo { target: Target::Point(Vec3::new(3.0, 0.0, 4.0)), within: 0.3 });
}

#[test]
fn a_teleport_starts_the_motion_afresh_where_it_lands() {
    // Mid-fall: the landing is measured from where the body was put, falling from rest.
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::new(0.0, 30.0, 0.0));
    let id = app.world().get::<AiId>(e).unwrap().to_string();
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    ok(&mut app, "teleport", json!({"id": id, "x": 0, "y": 0.5, "z": 0}));
    step(&mut app, 30);
    let landed = &seen(&app).landed;
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert!((landed[0].fall_height - 0.48).abs() < 0.01, "fell from the teleport: {landed:?}");
    let from_rest = (2.0 * 20.0 * 0.48_f32).sqrt();
    assert!((landed[0].impact_speed - from_rest).abs() < 0.4, "from rest, not at the old fall's speed: {landed:?}");

    // Mid-climb: the climb stays behind, the body stays where it was put.
    let mut app = crate::common::app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(20.0, 1.2, 5.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    let id = app.world().get::<AiId>(e).unwrap().to_string();
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::X);
    let ticks = drive(&mut app, 120, |w| w.get::<MotionState>(e).unwrap().mode != MotionMode::Mantle);
    assert!(ticks < 120, "never started climbing");
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::ZERO);
    ok(&mut app, "teleport", json!({"id": id, "x": -10, "y": 0, "z": 5}));
    step(&mut app, 30);
    let s = state(&app, e);
    assert_eq!((s.mode, s.mantle), (MotionMode::Ground, None), "{s:?}");
    let at = feet(&app, e);
    assert!((at - Vec3::new(-10.0, 0.02, 5.0)).length() < 1e-3, "stayed where it was put: {at}");
    assert!(seen(&app).landed.iter().all(|l| l.fall_height < 0.1), "{:?}", seen(&app).landed);
}
