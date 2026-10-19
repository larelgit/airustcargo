//! The camera next to a big body (R4): where a lock point high on a 10–15 m body lands in the
//! frame, and where the player does, through the real `orbit_camera`; with a fit
//! (`FramingTuning::fit`) the view turns up to keep both.

use std::time::Duration;

use ai_core::{AiCorePlugin, AiId, AiKind, SimConfig};
use bevy::camera::Projection;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use shards_core::{Intent, LocalPlayer};
use shards_motion::camera::{CameraTuning, FitTuning, FramingTuning};
use shards_motion::*;

use crate::common::*;

/// The frame's width over its height the measurements assume (the camera does not know it).
const ASPECT: f32 = 16.0 / 9.0;

struct Scene {
    app: App,
    player: Entity,
    camera: Entity,
    point: Entity,
}

/// The player at the origin facing -Z, the camera behind it on `tuning`, and a lock point `d` m
/// ahead at `h` m up (on a body too big to be seen whole), locked on.
fn scene(d: f32, h: f32, tuning: CameraTuning) -> Scene {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin {
            remote_port: None,
            crash_replays: false,
            sim: SimConfig { start_paused: true, ..default() },
            ..default()
        })
        .add_plugins(MotionPlugin)
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(1.0 / 60.0)));
    app.finish();
    app.cleanup();
    app.insert_resource(tuning);
    floor(app.world_mut());
    let player = app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO))).id();
    let camera = app
        .world_mut()
        .spawn((ThirdPersonCamera::default(), Projection::Perspective(PerspectiveProjection::default()), Transform::default()))
        .id();
    let point = app.world_mut().spawn((AiKind::new("Point"), LockOnTarget, LockPoint, Transform::from_xyz(0.0, h, -d))).id();
    app.update();
    let id = *app.world().get::<AiId>(point).unwrap();
    app.world_mut().get_mut::<Intent>(player).unwrap().target = Some(id);
    Scene { app, player, camera, point }
}

/// Lets the camera settle: two seconds of frames at 60 Hz.
fn settle(s: &mut Scene) {
    for _ in 0..120 {
        s.app.update();
    }
}

/// Where `p` lands in the frame: x and y from -1 to 1 at the edges (y up), from the camera's
/// transform and vertical field of view, at [`ASPECT`].
fn on_screen(s: &Scene, p: Vec3) -> Vec2 {
    let tf = s.app.world().get::<Transform>(s.camera).unwrap();
    let Projection::Perspective(lens) = s.app.world().get::<Projection>(s.camera).unwrap() else { unreachable!() };
    let v = tf.rotation.inverse() * (p - tf.translation);
    let half = ops::tan(lens.fov * 0.5);
    Vec2::new(v.x / -v.z / (half * ASPECT), v.y / -v.z / half)
}

fn point_at(s: &Scene) -> Vec3 {
    s.app.world().get::<Transform>(s.point).unwrap().translation
}

fn feet_of(s: &Scene) -> Vec3 {
    s.app.world().get::<Transform>(s.player).unwrap().translation
}

fn cam(s: &Scene) -> ThirdPersonCamera {
    s.app.world().get::<ThirdPersonCamera>(s.camera).unwrap().clone()
}

fn fitted() -> CameraTuning {
    CameraTuning { framing: FramingTuning { fit: Some(FitTuning::default()), ..default() }, ..default() }
}

/// [`fitted`], and a lower camera: the framed pitch may go down to level with the focus.
fn fitted_low() -> CameraTuning {
    CameraTuning { framing: FramingTuning { fit: Some(FitTuning::default()), pitch_clamp: (0.0, 0.7), ..default() }, ..default() }
}

/// The measurement R4 starts from: for lock points `d` m ahead and `h` m up, where the point
/// and the player's feet and head land in the frame (y: -1 bottom, 1 top), and the camera's
/// height, pitch and tilt; on the camera's defaults (no `camera` content), with the fit, and
/// with the fit and a lower camera. Printed, not checked: `cargo test -p shards_motion
/// --features render --test motion camera_big::measure -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement: prints the table"]
fn measure() {
    for (name, tuning) in [("defaults", CameraTuning::default()), ("fit", fitted()), ("fit, pitch_clamp (0.0, 0.7)", fitted_low())] {
        println!("{name}\n   d     h | point y | feet y | head y | camera y | pitch | tilt");
        for d in [2.0f32, 4.0, 6.0, 8.0] {
            for h in [1.2f32, 3.0, 6.0, 9.0, 12.0] {
                let mut s = scene(d, h, tuning.clone());
                settle(&mut s);
                let camera = s.app.world().get::<Transform>(s.camera).unwrap().translation;
                let (point, feet, c) = (point_at(&s), feet_of(&s), cam(&s));
                println!(
                    "{d:4.1} {h:5.1} | {:7.2} | {:6.2} | {:6.2} | {:8.2} | {:5.2} | {:4.2}",
                    on_screen(&s, point).y,
                    on_screen(&s, feet).y,
                    on_screen(&s, feet + Vec3::Y * 1.8).y,
                    camera.y,
                    c.pitch,
                    c.tilt,
                );
            }
        }
    }
}

#[test]
fn without_a_fit_a_point_high_on_a_big_body_leaves_the_top_of_the_frame() {
    let mut s = scene(2.0, 7.0, CameraTuning::default());
    settle(&mut s);
    let (point, feet) = (on_screen(&s, point_at(&s)), on_screen(&s, feet_of(&s)));
    assert!(point.y > 1.0, "the point is above the top edge: {point}");
    assert!(feet.y > -0.5, "while the bottom of the frame is empty below the feet: {feet}");
    assert_eq!(cam(&s).tilt, 0.0);
}

#[test]
fn a_fit_keeps_a_high_lock_point_in_frame_with_the_player() {
    for (d, h) in [(2.0, 6.0), (4.0, 8.0), (6.0, 9.0), (8.0, 12.0)] {
        let mut s = scene(d, h, fitted());
        settle(&mut s);
        let feet = feet_of(&s);
        let (point, head) = (on_screen(&s, point_at(&s)), on_screen(&s, feet + Vec3::Y * 1.8));
        let feet = on_screen(&s, feet);
        assert!((point.y - 0.85).abs() < 0.02, "{d} m ahead, {h} m up: on the top margin, {point}");
        assert!(feet.y > -0.86 && head.y < point.y, "{d} m ahead, {h} m up: the player below it, in frame: feet {feet}, head {head}");
        assert!(point.x.abs() < 0.01, "and in the middle: {point}");
    }
}

#[test]
fn when_both_cannot_be_kept_the_player_is() {
    let mut s = scene(2.0, 12.0, fitted());
    settle(&mut s);
    let (point, feet) = (on_screen(&s, point_at(&s)), on_screen(&s, feet_of(&s)));
    assert!((feet.y + 0.85).abs() < 0.02, "the feet on the bottom margin: {feet}");
    assert!(point.y > 1.0, "the point off the top: {point}");
}

#[test]
fn a_target_the_frame_already_holds_is_framed_as_without_a_fit() {
    // A lock point at chest height, and a plain body (framed at its chest): no turn, the very
    // same camera.
    for h in [1.2, 0.0] {
        let mut a = scene(4.0, h, CameraTuning::default());
        let mut b = scene(4.0, h, fitted());
        for s in [&mut a, &mut b] {
            if h == 0.0 {
                let point = s.point;
                s.app.world_mut().entity_mut(point).remove::<LockPoint>();
            }
            settle(s);
        }
        assert_eq!(cam(&b).tilt, 0.0);
        assert_eq!(a.app.world().get::<Transform>(a.camera), b.app.world().get::<Transform>(b.camera));
    }
}

#[test]
fn the_view_comes_back_to_the_player_after_the_lock() {
    let mut s = scene(2.0, 9.0, fitted());
    settle(&mut s);
    assert!(cam(&s).tilt > 0.2, "{}", cam(&s).tilt);
    let player = s.player;
    s.app.world_mut().get_mut::<Intent>(player).unwrap().target = None;
    settle(&mut s);
    assert!(cam(&s).tilt < 1e-4, "{}", cam(&s).tilt);
    let tf = *s.app.world().get::<Transform>(s.camera).unwrap();
    let to_focus = (cam(&s).focus - tf.translation).normalize();
    assert!(tf.forward().dot(to_focus) > 0.9999, "looks at the focus again");
}

#[test]
fn the_feet_and_tilt_max_hold_the_tilt_on_the_very_next_frame() {
    let mut s = scene(2.0, 9.0, fitted());
    settle(&mut s);
    let held = cam(&s).tilt;
    assert!(held > 0.2, "{held}");
    // A lower maximum while the fit holds the view up: no easing down to it.
    s.app.world_mut().resource_mut::<CameraTuning>().framing.fit.as_mut().unwrap().tilt_max = 0.1;
    s.app.update();
    assert!(cam(&s).tilt <= 0.1, "{}", cam(&s).tilt);
    s.app.insert_resource(fitted());
    settle(&mut s);
    // A shorter arm (as a wall makes it) sees the feet lower in the frame: the tilt drops at
    // once, so the feet never leave the bottom margin on the way.
    let camera = s.camera;
    s.app.world_mut().get_mut::<ThirdPersonCamera>(camera).unwrap().distance = 2.5;
    for frame in 0..30 {
        s.app.update();
        let feet = on_screen(&s, feet_of(&s));
        assert!(feet.y > -0.851, "frame {frame}: the feet at {feet}, tilt {}", cam(&s).tilt);
    }
    assert!(cam(&s).tilt < held, "{} against {held}", cam(&s).tilt);
}
