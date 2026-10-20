//! The camera's numbers as content (kind `camera`) and the offsets hook, on a headless engine
//! app: no window, so only what does not need `render`.

use bevy::prelude::*;
use serde_json::json;
use shards_motion::camera::{CameraOffsets, CameraTuning, FitTuning};

use crate::engine::*;

fn tuning(app: &App) -> CameraTuning {
    app.world().resource::<CameraTuning>().clone()
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

#[test]
fn without_a_camera_definition_the_camera_runs_on_the_old_numbers() {
    let dir = temp_dir("camera_none");
    write(&dir, "content/hero.locomotion.ron", r#"{ "hero": () }"#);
    let mut app = engine_app(&dir);
    frames(&mut app, 3);
    assert_eq!(tuning(&app), CameraTuning::default());
    assert_eq!(tuning(&app).pitch, (-0.45, 1.35));
    assert!(app.world().resource::<CameraOffsets>().is_neutral());
}

#[test]
fn a_camera_definition_sets_the_tuning_and_a_reload_changes_it() {
    let dir = temp_dir("camera_content");
    write(&dir, "content/camera.camera.ron", r#"{ "default": (pitch: (-1.1, 0.5), follow_rate: Some(12.0), arm: (follow_rate: Some(6.0))) }"#);
    let mut app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:?}");
    frames(&mut app, 3);
    let t = tuning(&app);
    assert_eq!((t.pitch, t.follow_rate, t.arm.follow_rate), ((-1.1, 0.5), Some(12.0), Some(6.0)));
    assert_eq!(t.arm.radius, 0.25, "what the file leaves out keeps its default");

    write(&dir, "content/camera.camera.ron", r#"{ "default": (flick: (mouse_threshold_px: 110.0, mouse_decay_px_s: Some(600.0), cooldown_s: 0.3)) }"#);
    let out = ok(&mut app, "content_reload", json!({}));
    assert!(out.contains("1 definition changed"), "{out}");
    frames(&mut app, 2);
    let t = tuning(&app);
    assert_eq!((t.flick.mouse_threshold_px, t.flick.mouse_decay_px_s, t.flick.cooldown_s), (110.0, Some(600.0), 0.3));
    assert_eq!(t.pitch, (-0.45, 1.35), "an edit replaces the definition: the pitch is the default again");

    // Taking the definition away puts the code defaults back.
    std::fs::remove_file(dir.join("content/camera.camera.ron")).unwrap();
    write(&dir, "content/other.camera.ron", r#"{ "elsewhere": () }"#);
    ok(&mut app, "content_reload", json!({}));
    frames(&mut app, 2);
    assert_eq!(tuning(&app), CameraTuning::default());
}

#[test]
fn a_camera_definition_out_of_range_is_an_error_naming_its_file_and_field() {
    let dir = temp_dir("camera_bad");
    write(
        &dir,
        "content/camera.camera.ron",
        r#"{ "default": (pitch: (0.5, -0.5), arm: (follow_rate: Some(-2.0)), flick: (stick_threshold: 0.8, stick_rearm: Some(0.9))) }"#,
    );
    let mut app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    for needle in ["pitch (0.5, -0.5)", "arm.follow_rate", "flick.stick_rearm"] {
        assert!(errors.iter().any(|e| e.contains("content/camera.camera.ron") && e.contains(needle)), "{needle}: {errors:?}");
    }
    // A first load keeps the broken definition in the database; the camera does not take it.
    frames(&mut app, 3);
    assert_eq!(tuning(&app), CameraTuning::default());
}

#[test]
fn a_fit_is_read_with_its_defaults_and_bad_numbers_are_errors() {
    let dir = temp_dir("camera_fit");
    write(&dir, "content/camera.camera.ron", r#"{ "default": (framing: (fit: Some((margin: 0.1)), pitch_clamp: (0.0, 0.7))) }"#);
    let mut app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:?}");
    frames(&mut app, 3);
    let fit = tuning(&app).framing.fit.expect("a fit");
    assert_eq!((fit.margin, fit.tilt_max, fit.rate), (0.1, 0.6, 6.0), "what the file leaves out keeps its default");
    let written = ron::to_string(&FitTuning::default()).unwrap();
    assert_eq!(written, "()", "a default fit writes no field");

    let dir = temp_dir("camera_fit_bad");
    write(&dir, "content/camera.camera.ron", r#"{ "default": (framing: (fit: Some((margin: 1.0, tilt_max: 2.0, rate: 0.0)))) }"#);
    let app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    for needle in ["framing.fit.margin", "framing.fit.tilt_max", "framing.fit.rate"] {
        assert!(errors.iter().any(|e| e.contains("content/camera.camera.ron") && e.contains(needle)), "{needle}: {errors:?}");
    }
}

#[test]
fn a_typo_in_a_camera_definition_is_an_error() {
    let dir = temp_dir("camera_typo");
    write(&dir, "content/camera.camera.ron", r#"{ "default": (pich: (-1.0, 1.0)) }"#);
    let app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.iter().any(|e| e.contains("content/camera.camera.ron") && e.contains("pich")), "{errors:?}");
}

#[test]
fn a_lock_on_definition_is_read_and_one_out_of_range_is_an_error_naming_its_file_and_field() {
    use shards_motion::camera::LockOnDef;
    let dir = temp_dir("lock_on_content");
    write(&dir, "content/lock.lock_on.ron", r#"{ "default": (range: 25.0, weights: (danger: 2.0), obstruction_s: None) }"#);
    let mut app = engine_app(&dir);
    frames(&mut app, 2);
    let db = app.world().resource::<shards_content::ContentDb>();
    let def = LockOnDef::active(Some(db)).expect("the rules are on");
    assert_eq!((def.range, def.weights.danger, def.weights.distance, def.obstruction_s), (25.0, 2.0, 1.0, None));

    let dir = temp_dir("lock_on_bad");
    write(&dir, "content/lock.lock_on.ron", r#"{ "default": (range: -1.0, aim_height: 1.5, weights: (view: -0.5)) }"#);
    let app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    for needle in ["range is -1", "aim_height is 1.5", "weights.view is -0.5"] {
        assert!(errors.iter().any(|e| e.contains("content/lock.lock_on.ron") && e.contains(needle)), "{needle}: {errors:?}");
    }
}

#[test]
fn a_shot_plays_only_where_the_game_is_drawn_and_a_bad_one_names_its_file_and_key() {
    let dir = temp_dir("camera_shot");
    write(
        &dir,
        "content/scenes.shot.ron",
        r#"{
            "still": (keys: [(at: 0.0, eye: At((0.0, 5.0, 5.0)), look: At((0.0, 0.0, 0.0)))]),
            "muddled": (keys: [
                (at: 2.0, eye: At((0.0, 1.0, 0.0)), look: At((0.0, 1.0, 0.0))),
                (at: 1.0, eye: At((0.0, 5.0, 5.0)), look: At((0.0, 0.0, 0.0)), fov_deg: 150.0),
            ], blend_out_s: -1.0),
        }"#,
    );
    // A misspelt field of a point is an error, not a point at the body's origin.
    write(&dir, "content/typo.shot.ron", r#"{"typo": (keys: [(at: 0.0, eye: On(role: "boss", ofset: (0.0, 3.0, 0.0)), look: At((0.0, 0.0, 0.0)))])}"#);
    let mut app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    for needle in [
        "keys[0]: `eye` and `look` are the same place",
        "keys[1].at 1: not after the key before it (2); keys go in time order",
        "keys[1].fov_deg 150: 10 to 120 degrees",
        "blend_out_s -1: 0 to 10 s",
    ] {
        assert!(errors.iter().any(|e| e.contains("content/scenes.shot.ron") && e.contains(needle)), "{needle}: {errors:?}");
    }
    assert!(errors.iter().any(|e| e.contains("content/typo.shot.ron") && e.contains("ofset")), "{errors:?}");
    assert_eq!(errors.len(), 5, "{errors:?}");
    let e = fails(&mut app, "play_shot", json!({"id": "muddled"}));
    assert!(e.contains("shot `muddled` cannot play: keys[0]: `eye` and `look` are the same place"), "{e}");
    // No window, no camera: nothing to play a shot on.
    let e = fails(&mut app, "play_shot", json!({"id": "still"}));
    assert!(e.contains("no camera to play a shot on") && e.contains("a headless game has none"), "{e}");
    assert_eq!(fails(&mut app, "stop_shot", json!({})), "no shot is playing");
    assert!(app.world().resource::<shards_motion::camera::Shots>().playing().is_none());
}
