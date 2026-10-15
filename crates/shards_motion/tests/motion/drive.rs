//! `Intent` steering the controller, locomotion numbers from content, and the movement tools.

use ai_core::{AiId, AiJournal, JournalKind};
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{GLIDE, JUMP, SPRINT};
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

fn warnings(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Warn).map(|e| e.text.clone()).collect()
}

fn forward(app: &App, e: Entity) -> Vec3 {
    *app.world().get::<Transform>(e).unwrap().forward()
}

#[test]
fn move_dir_and_held_buttons_steer_the_controller() {
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| {
        i.move_dir = Vec3::X;
        i.set_held(SPRINT, true);
    });
    step(&mut app, 60);
    let p = CharacterParams::default();
    let s = state(&app, e);
    assert!((s.velocity.x - p.sprint_speed).abs() < 1e-3 && s.sprinting, "{s:?}");
    assert!(app.world().get::<LocomotionInput>(e).unwrap().sprint);

    edit_intent(&mut app, e, |i| i.set_held(SPRINT, false));
    step(&mut app, 30);
    assert!((state(&app, e).velocity.x - p.run_speed).abs() < 1e-3, "back to running");

    // A move longer than 1 is a run, not a faster run; a vertical part is ignored.
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::new(5.0, 3.0, 0.0));
    step(&mut app, 30);
    assert!((state(&app, e).velocity.x - p.run_speed).abs() < 1e-3);
    assert!(state(&app, e).velocity.y.abs() < 1e-3);

    // Garbage stays out of the controller.
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::splat(f32::NAN));
    step(&mut app, 5);
    assert!(feet(&app, e).is_finite());
}

#[test]
fn the_jump_press_is_taken_once_and_the_rest_is_left() {
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| {
        i.press("light");
        i.press(JUMP);
        i.press(JUMP);
    });
    step(&mut app, 1);
    let pressed: Vec<String> = intent(&app, e).pressed.iter().map(|p| p.input.clone()).collect();
    assert_eq!(pressed, ["light"], "both jump presses are consumed, `light` waits for whoever wants it");
    assert_eq!(seen(&app).jumped.len(), 1, "two presses in one tick are one jump");
    assert_eq!(state(&app, e).mode, MotionMode::Air);
}

#[test]
fn a_held_glide_opens_when_falling_and_closing_it_drops_you() {
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::new(0.0, 40.0, 0.0));
    step(&mut app, 20);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    edit_intent(&mut app, e, |i| i.set_held(GLIDE, true));
    step(&mut app, 60);
    let s = state(&app, e);
    assert_eq!(s.mode, MotionMode::Glide);
    assert!(s.velocity.y >= -CharacterParams::default().glide_fall_speed - 1e-3, "{s:?}");
    edit_intent(&mut app, e, |i| i.set_held(GLIDE, false));
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
}

#[test]
fn facing_follows_the_aim_point_then_the_lock_on_target() {
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    let dummy = app.world_mut().spawn((ai_core::AiKind::new("Dummy"), Transform::from_xyz(0.0, 0.0, -10.0))).id();
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| i.aim = Some(Vec3::new(10.0, 0.0, 0.0)));
    step(&mut app, 30);
    assert!((forward(&app, e) - Vec3::X).length() < 1e-3, "aim: {}", forward(&app, e));

    let id = *app.world().get::<AiId>(dummy).unwrap();
    edit_intent(&mut app, e, |i| {
        i.aim = None;
        i.target = Some(id);
    });
    step(&mut app, 30);
    assert!((forward(&app, e) - Vec3::NEG_Z).length() < 1e-3, "target: {}", forward(&app, e));

    // Aim wins over a lock-on while both are set: an attack order points the body at its victim.
    edit_intent(&mut app, e, |i| i.aim = Some(Vec3::new(-10.0, 0.0, 0.0)));
    step(&mut app, 30);
    assert!((forward(&app, e) + Vec3::X).length() < 1e-3, "aim over target: {}", forward(&app, e));
}

#[test]
fn what_actions_set_is_cleared_for_the_next_tick() {
    let mut app = app();
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    set_input(app.world_mut(), e, LocomotionInput { locked: true, face: Some(Vec3::X), external: Vec3::X, ..default() });
    step(&mut app, 1);
    let input = app.world().get::<LocomotionInput>(e).unwrap();
    assert!(!input.locked && input.face.is_none() && input.external == Vec3::ZERO, "{input:?}");
    assert_eq!(feet(&app, e).x, 0.0, "nothing moved it: the actions run after this system and set them again");
}

const HERO_RON: &str = r#"{ "hero": (run_speed: 4.0, walk_speed: 1.5), "frail": (jump_height: 0.5) }"#;

#[test]
fn locomotion_content_sets_the_numbers_and_a_reload_changes_them_on_the_next_tick() {
    let dir = temp_dir("locomotion");
    write(&dir, "content/hero.locomotion.ron", HERO_RON);
    let mut app = engine_app(&dir);
    floor(app.world_mut());
    let e = player_with(app.world_mut(), Vec3::ZERO, "hero");
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::X);
    step(&mut app, 60);
    assert!((state(&app, e).velocity.x - 4.0).abs() < 1e-3, "{:?}", state(&app, e));

    write(&dir, "content/hero.locomotion.ron", r#"{ "hero": (run_speed: 6.0), "frail": (jump_height: 0.5) }"#);
    let out = ok(&mut app, "content_reload", json!({}));
    assert!(out.contains("1 definition changed"), "{out}");
    step(&mut app, 30);
    assert!((state(&app, e).velocity.x - 6.0).abs() < 1e-3, "the next ticks use the edited file: {:?}", state(&app, e));

    // The capsule follows the numbers, too.
    write(&dir, "content/hero.locomotion.ron", r#"{ "hero": (radius: 0.6, height: 2.2), "frail": (jump_height: 0.5) }"#);
    ok(&mut app, "content_reload", json!({}));
    step(&mut app, 2);
    let collider = app.world().get::<Collider>(e).unwrap();
    assert!(matches!(collider.shape, Shape::Capsule { radius, .. } if (radius - 0.6).abs() < 1e-6), "{collider:?}");
}

#[test]
fn a_locomotion_nobody_defined_runs_on_defaults_and_says_so_once() {
    let dir = temp_dir("dangling");
    write(&dir, "content/hero.locomotion.ron", HERO_RON);
    let mut app = engine_app(&dir);
    floor(app.world_mut());
    let e = player_with(app.world_mut(), Vec3::ZERO, "ghost");
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::X);
    step(&mut app, 60);
    assert!((state(&app, e).velocity.x - CharacterParams::default().run_speed).abs() < 1e-3);
    let said: Vec<String> = warnings(&app).into_iter().filter(|w| w.contains("locomotion `ghost`")).collect();
    assert_eq!(said.len(), 1, "once, not every tick: {said:?}");
    assert!(said[0].contains("not defined in content") && said[0].contains("*.locomotion.ron"), "{}", said[0]);
}

#[test]
fn a_typo_in_locomotion_content_is_an_error_with_its_file() {
    let dir = temp_dir("typo");
    write(&dir, "content/hero.locomotion.ron", r#"{ "hero": (walk_spead: 3.0), "slow": (float_depth: 5.0) }"#);
    let mut app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.iter().any(|e| e.contains("content/hero.locomotion.ron") && e.contains("walk_spead")), "{errors:?}");
    // The broken file is reported; `content_check` agrees.
    let out = ok(&mut app, "content_check", json!({}));
    assert!(out.contains("walk_spead"), "{out}");
}

#[test]
fn the_engine_with_motion_has_a_total_order_in_the_fixed_tick() {
    use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
    let mut app = bare_engine("order");
    let strict = ScheduleBuildSettings { ambiguity_detection: LogLevel::Error, ..default() };
    let result = app.world_mut().schedule_scope(FixedUpdate, |world, schedule| {
        schedule.set_build_settings(strict);
        schedule.initialize(world).map(|_| ()).map_err(|e| e.to_string(schedule.graph(), world))
    });
    if let Err(e) = result {
        panic!("ambiguous FixedUpdate systems:\n{e}");
    }
}
