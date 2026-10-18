//! The camera's numbers as content (kind `camera`) and the offsets hook, on a headless engine
//! app: no window, so only what does not need `render`.

use bevy::prelude::*;
use serde_json::json;
use shards_motion::camera::{CameraOffsets, CameraTuning};

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
fn a_typo_in_a_camera_definition_is_an_error() {
    let dir = temp_dir("camera_typo");
    write(&dir, "content/camera.camera.ron", r#"{ "default": (pich: (-1.0, 1.0)) }"#);
    let app = engine_app(&dir);
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.iter().any(|e| e.contains("content/camera.camera.ron") && e.contains("pich")), "{errors:?}");
}
