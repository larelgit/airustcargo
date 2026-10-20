//! The camera systems (the `render` feature): lock-on writes `Intent.target`, the orbit
//! follows, looks, keeps out of walls, shakes, and sets the field of view. Pure math is
//! tested in `camera::math`.

use std::time::Duration;

use ai_core::{AiCorePlugin, AiId, AiIndex, AiKind, SimConfig};
use bevy::camera::Projection;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use shards_core::{Intent, LocalPlayer};
use shards_motion::camera::{AddTrauma, CameraOffsets, CameraShake, CameraTuning, FlickTuning};
use shards_motion::*;
use shards_settings::AccessibilitySettings;

use crate::common::*;

struct Scene {
    app: App,
    player: Entity,
    camera: Entity,
    near: Entity,
    side: Entity,
}

fn scene() -> Scene {
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
        .init_resource::<ButtonInput<MouseButton>>();
    app.finish();
    app.cleanup();
    floor(app.world_mut());
    let player = app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO))).id();
    let camera = app
        .world_mut()
        .spawn((ThirdPersonCamera::default(), Projection::Perspective(PerspectiveProjection::default()), Transform::default()))
        .id();
    let enemy = |app: &mut App, x: f32| {
        app.world_mut().spawn((AiKind::new("Enemy"), LockOnTarget, Transform::from_xyz(x, 0.0, -10.0))).id()
    };
    let (near, side) = (enemy(&mut app, 0.5), enemy(&mut app, 6.0));
    app.update();
    app.update();
    Scene { app, player, camera, near, side }
}

fn id(app: &App, e: Entity) -> AiId {
    *app.world().get::<AiId>(e).unwrap()
}

fn frame(app: &mut App) {
    app.update();
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
    app.insert_resource(AccumulatedMouseMotion::default());
}

/// Lets `secs` of simulation time pass in a few frames: the camera tests start paused, so the
/// clock is switched on for them and off again.
fn run_sim(s: &mut Scene, secs: f32) {
    s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(secs / 4.0)));
    s.app.world_mut().resource_mut::<Time<Virtual>>().set_max_delta(Duration::from_secs(5));
    ai_core::sim::resume(s.app.world_mut());
    for _ in 0..4 {
        s.app.update();
    }
    s.app.world_mut().resource_mut::<Time<Virtual>>().pause();
    s.app.insert_resource(TimeUpdateStrategy::Automatic);
}

fn target(s: &Scene) -> Option<AiId> {
    s.app.world().get::<Intent>(s.player).unwrap().target
}

fn tap_lock(s: &mut Scene) {
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Tab);
    frame(&mut s.app);
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::Tab);
    frame(&mut s.app);
}

#[test]
fn lock_on_picks_the_target_nearest_the_center_and_lets_go_on_the_next_press() {
    let mut s = scene();
    assert_eq!(target(&s), None);
    tap_lock(&mut s);
    assert_eq!(target(&s), Some(id(&s.app, s.near)), "the one in front, not the one off to the side");
    tap_lock(&mut s);
    assert_eq!(target(&s), None);
}

#[test]
fn a_look_flick_switches_to_the_neighbour_and_back() {
    let mut s = scene();
    tap_lock(&mut s);
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(30.0, 0.0) });
    s.app.update();
    assert_eq!(target(&s), Some(id(&s.app, s.side)), "a flick to the right selects the target on the right");
    s.app.insert_resource(AccumulatedMouseMotion::default());
    // A switch needs a moment before the next one, on the simulation clock.
    run_sim(&mut s, 0.4);
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(-30.0, 0.0) });
    s.app.update();
    assert_eq!(target(&s), Some(id(&s.app, s.near)), "and back");
}

#[test]
fn a_lock_lets_go_when_the_target_is_gone_dead_or_far() {
    // Despawned.
    let mut s = scene();
    tap_lock(&mut s);
    s.app.world_mut().despawn(s.near);
    frame(&mut s.app);
    assert_eq!(target(&s), None);

    // Dead: no longer something to lock on to.
    let mut s = scene();
    tap_lock(&mut s);
    s.app.world_mut().entity_mut(s.near).remove::<LockOnTarget>();
    frame(&mut s.app);
    assert_eq!(target(&s), None);

    // Out of range.
    let mut s = scene();
    tap_lock(&mut s);
    s.app.world_mut().get_mut::<Transform>(s.near).unwrap().translation = Vec3::new(0.0, 0.0, -80.0);
    frame(&mut s.app);
    assert_eq!(target(&s), None);
}

#[test]
fn a_target_an_agent_set_is_not_taken_away_for_lacking_the_marker() {
    let mut s = scene();
    let rock = s.app.world_mut().spawn((AiKind::new("Rock"), Transform::from_xyz(0.0, 0.0, -8.0))).id();
    let rock_id = id(&s.app, rock);
    s.app.world_mut().get_mut::<Intent>(s.player).unwrap().target = Some(rock_id);
    for _ in 0..5 {
        frame(&mut s.app);
    }
    assert_eq!(target(&s), Some(rock_id));
    assert!(s.app.world().resource::<AiIndex>().get(rock_id).is_some());
}

#[test]
fn the_camera_never_lets_go_of_a_target_an_agent_set() {
    // The eye sits behind the player, so a target 24 m ahead of the player is some 29 m from
    // it: past what the camera lets go of (1.3 x its 20 m range). The camera is a view of
    // the game, not a rule of it: an agent's lock reads the same with and without a window.
    let rock = |s: &mut Scene, z: f32| {
        let rock = s.app.world_mut().spawn((AiKind::new("Rock"), Transform::from_xyz(0.0, 0.0, z))).id();
        id(&s.app, rock)
    };
    let set = |s: &mut Scene, to: AiId| s.app.world_mut().get_mut::<Intent>(s.player).unwrap().target = Some(to);

    let mut s = scene();
    let far = rock(&mut s, -24.0);
    set(&mut s, far);
    for _ in 0..5 {
        frame(&mut s.app);
    }
    assert_eq!(target(&s), Some(far), "the player and the lock are 24 m apart, and the camera has no say");

    // Even one the camera had picked once, when the agent has taken the lock over.
    let mut s = scene();
    tap_lock(&mut s);
    let near = id(&s.app, s.near);
    assert_eq!(target(&s), Some(near));
    let other = rock(&mut s, -5.0);
    set(&mut s, other);
    frame(&mut s.app);
    set(&mut s, near);
    s.app.world_mut().get_mut::<Transform>(s.near).unwrap().translation = Vec3::new(0.0, 0.0, -80.0);
    frame(&mut s.app);
    assert_eq!(target(&s), Some(near), "set by the agent, so kept");
}

#[test]
fn the_camera_follows_looks_and_stays_out_of_walls() {
    let mut s = scene();
    let at = |s: &Scene| s.app.world().get::<Transform>(s.camera).unwrap().translation;
    let cam = |s: &Scene| s.app.world().get::<ThirdPersonCamera>(s.camera).unwrap().clone();
    let before = at(&s);
    // Behind (+Z) and above the character, five and a half meters out.
    assert!(before.z > 4.0 && before.y > 2.0, "{before}");
    assert!((before - Vec3::new(0.0, 1.5, 0.0)).length() - 5.5 < 0.05, "{before}");

    // Turning the mouse turns the camera about the character, not the character.
    let yaw = cam(&s).yaw;
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(100.0, 0.0) });
    s.app.update();
    assert!((cam(&s).yaw - (yaw - 0.25)).abs() < 1e-4, "100 px at 0.0025: {}", cam(&s).yaw);
    s.app.insert_resource(AccumulatedMouseMotion::default());

    // The character walks off and the camera comes along.
    s.app.world_mut().get_mut::<Transform>(s.player).unwrap().translation = Vec3::new(30.0, 0.0, 0.0);
    for _ in 0..3 {
        frame(&mut s.app);
    }
    assert!((at(&s) - Vec3::new(30.0, 1.5, 0.0)).length() < 6.0, "{}", at(&s));

    // A wall right behind it pulls the camera in.
    s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap().yaw = 0.0;
    s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap().pitch = 0.0;
    block(s.app.world_mut(), Vec3::new(20.0, 0.0, 2.0), Vec3::new(40.0, 5.0, 3.0));
    frame(&mut s.app);
    frame(&mut s.app);
    assert!(at(&s).z < 2.0, "pulled in front of the wall at z = 2: {}", at(&s));
}

#[test]
fn aiming_turns_the_view_by_the_aim_sensitivity() {
    let mut s = scene();
    {
        let mut settings = s.app.world_mut().resource_mut::<CameraSettings>();
        (settings.sensitivity, settings.aim_sensitivity, settings.aim_assist) = (2.0, 0.5, 0.0);
    }
    let turn = |s: &mut Scene| {
        let yaw = cam(s).yaw;
        s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(100.0, 0.0) });
        s.app.update();
        s.app.insert_resource(AccumulatedMouseMotion::default());
        cam(s).yaw - yaw
    };
    assert!((turn(&mut s) + 0.5).abs() < 1e-4, "free look: 100 px at 0.0025, times 2");
    // The game says the player aims: the same mouse turns the view by the aiming speed.
    s.app.insert_resource(CameraOffsets { aiming: true, ..default() });
    assert!((turn(&mut s) + 0.125).abs() < 1e-4, "aiming: times 0.5");
    s.app.insert_resource(CameraOffsets::default());
    assert!((turn(&mut s) + 0.5).abs() < 1e-4, "done aiming: the free speed again");
}

#[test]
fn field_of_view_and_shake_follow_the_settings() {
    let mut s = scene();
    s.app.world_mut().resource_mut::<CameraSettings>().fov_deg = 90.0;
    frame(&mut s.app);
    let Projection::Perspective(p) = s.app.world().get::<Projection>(s.camera).unwrap() else { panic!("perspective") };
    assert!((p.fov - 90f32.to_radians()).abs() < 1e-5);

    let steady = s.app.world().get::<Transform>(s.camera).unwrap().translation;
    s.app.world_mut().commands().trigger(AddTrauma(1.0));
    s.app.update();
    s.app.update();
    let shaken = s.app.world().get::<Transform>(s.camera).unwrap().translation;
    assert!((shaken - steady).length() > 1e-4, "a hit shakes the camera");

    // Switched off, hits do nothing.
    let mut s = scene();
    s.app.world_mut().resource_mut::<CameraSettings>().shake = false;
    frame(&mut s.app);
    let steady = s.app.world().get::<Transform>(s.camera).unwrap().translation;
    s.app.world_mut().commands().trigger(AddTrauma(1.0));
    s.app.update();
    s.app.update();
    assert!((s.app.world().get::<Transform>(s.camera).unwrap().translation - steady).length() < 1e-5);
    assert_eq!(s.app.world().resource::<CameraShake>().trauma, 0.0);
}

#[test]
fn the_players_settings_drive_the_camera_in_a_window() {
    let mut s = scene();
    s.app.world_mut().resource_mut::<AccessibilitySettings>().fov_deg = 100.0;
    s.app.world_mut().resource_mut::<AccessibilitySettings>().camera_shake = 0.0;
    frame(&mut s.app);
    frame(&mut s.app);
    let Projection::Perspective(p) = s.app.world().get::<Projection>(s.camera).unwrap() else { panic!("perspective") };
    assert!((p.fov - 100f32.to_radians()).abs() < 1e-5, "the accessibility field of view");
    // No shake at all: a hit does nothing.
    let steady = s.app.world().get::<Transform>(s.camera).unwrap().translation;
    s.app.world_mut().commands().trigger(AddTrauma(1.0));
    s.app.update();
    s.app.update();
    assert!((s.app.world().get::<Transform>(s.camera).unwrap().translation - steady).length() < 1e-5);
}

#[test]
fn motion_blur_follows_the_accessibility_setting() {
    use bevy::post_process::motion_blur::MotionBlur;
    let mut s = scene();
    let blur = s.app.world_mut().spawn(MotionBlur::default()).id();
    let read = |s: &Scene| {
        let b = s.app.world().get::<MotionBlur>(blur).unwrap();
        (b.shutter_angle, b.samples)
    };
    s.app.world_mut().resource_mut::<AccessibilitySettings>().motion_blur = 0.2;
    frame(&mut s.app);
    assert_eq!(read(&s), (0.2, 1));
    // None is off: the shutter never opens, so Bevy skips the pass.
    s.app.world_mut().resource_mut::<AccessibilitySettings>().motion_blur = 0.0;
    frame(&mut s.app);
    assert_eq!(read(&s), (0.0, 1));
    s.app.world_mut().resource_mut::<AccessibilitySettings>().motion_blur = 1.0;
    frame(&mut s.app);
    assert_eq!(read(&s), (1.0, 1));
}

#[test]
fn motion_blur_off_and_on_again_keeps_the_games_sample_count() {
    use bevy::post_process::motion_blur::MotionBlur;
    let mut s = scene();
    let blur = s.app.world_mut().spawn(MotionBlur { samples: 4, ..default() }).id();
    for strength in [0.0, 0.7, 0.0, 1.0] {
        s.app.world_mut().resource_mut::<AccessibilitySettings>().motion_blur = strength;
        frame(&mut s.app);
        let b = s.app.world().get::<MotionBlur>(blur).unwrap();
        assert_eq!((b.shutter_angle, b.samples), (strength as f32, 4), "at {strength}");
    }
}

fn cam(s: &Scene) -> ThirdPersonCamera {
    s.app.world().get::<ThirdPersonCamera>(s.camera).unwrap().clone()
}

fn camera_at(s: &Scene) -> Vec3 {
    s.app.world().get::<Transform>(s.camera).unwrap().translation
}

/// A scene whose frames are 0.1 s of real time each (the camera's clock), the auto-centre (K2)
/// on or off, no aim assist.
fn timed_scene(auto_centre: bool) -> Scene {
    let mut s = scene();
    s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(100)));
    {
        let mut settings = s.app.world_mut().resource_mut::<CameraSettings>();
        // No aim assist: a look turns the view by exactly what the mouse says.
        (settings.auto_centre, settings.aim_assist) = (auto_centre, 0.0);
    }
    s
}

/// The player walks `frames` frames at 2 m/s along `dir`, facing it; the camera's yaw after.
fn walk(s: &mut Scene, dir: Vec3, frames: usize) -> f32 {
    walk_by(s, dir, 0.2, frames)
}

/// The player moves `step` meters a frame along `dir`, facing it, for `frames` frames; the
/// camera's yaw after.
fn walk_by(s: &mut Scene, dir: Vec3, step: f32, frames: usize) -> f32 {
    for _ in 0..frames {
        let mut tf = s.app.world_mut().get_mut::<Transform>(s.player).unwrap();
        tf.translation += dir * step;
        tf.look_to(dir, Vec3::Y);
        s.app.update();
    }
    cam(s).yaw
}

#[test]
fn left_alone_the_view_turns_behind_a_walking_body_when_the_player_wants_it() {
    use std::f32::consts::FRAC_PI_2;
    // Off (the default): walking east leaves the view where the player left it.
    let mut s = timed_scene(false);
    assert!(!CameraSettings::default().auto_centre);
    assert_eq!(walk(&mut s, Vec3::X, 40), 0.0);

    // On: nothing for the first 1.5 s, then the view turns behind the body, west of it.
    let mut s = timed_scene(true);
    assert_eq!(walk(&mut s, Vec3::X, 14), 0.0, "1.4 s: the look not left alone long enough");
    let behind = walk(&mut s, Vec3::X, 30);
    assert!((behind + FRAC_PI_2).abs() < 0.01, "behind a body walking east: {behind}");

    // A look starts the wait over: the player's own turn stays for another 1.5 s.
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(40.0, 0.0) });
    s.app.update();
    s.app.insert_resource(AccumulatedMouseMotion::default());
    let looked = cam(&s).yaw;
    assert!((looked - (behind - 0.1)).abs() < 1e-4, "{looked}");
    assert_eq!(walk(&mut s, Vec3::NEG_Z, 14), looked);
    // Then it turns behind the body walking away from it, north.
    let north = walk(&mut s, Vec3::NEG_Z, 30);
    assert!(north.abs() < 0.01, "{north}");

    // A body walking toward the camera does not swing it round.
    let mut s = timed_scene(true);
    assert_eq!(walk(&mut s, Vec3::Z, 40), 0.0);
    // Nor does one standing still.
    let mut s = timed_scene(true);
    s.app.world_mut().get_mut::<Transform>(s.player).unwrap().look_to(Vec3::X, Vec3::Y);
    assert_eq!(walk(&mut s, Vec3::new(1e-4, 0.0, 0.0), 40), 0.0);
}

#[test]
fn a_short_teleport_starts_the_wait_over_and_a_stop_stops_the_turn_at_once() {
    use std::f32::consts::FRAC_PI_2;
    // A body placed 5 m away (past `body_snap_distance`, short of the focus's `snap_distance`)
    // was not walked there: the wait starts over on that frame.
    let mut s = timed_scene(true);
    assert_eq!(walk(&mut s, Vec3::X, 14), 0.0);
    assert_eq!(walk_by(&mut s, Vec3::X, 5.0, 1), 0.0);
    assert_eq!(cam(&s).look_idle_s, 0.1, "the teleport's frame starts the wait over");
    assert_eq!(walk(&mut s, Vec3::X, 13), 0.0, "1.4 s after the teleport");
    assert!(walk(&mut s, Vec3::X, 2) < 0.0, "then it turns");

    // A body that stops from a run stops the turn on the same frame, while the focus still
    // catches up with it.
    let mut s = timed_scene(true);
    let turning = walk_by(&mut s, Vec3::X, 0.6, 20);
    assert!(turning < 0.0 && turning > -FRAC_PI_2 + 0.1, "turning, not yet behind: {turning}");
    let focus = cam(&s).focus;
    assert_eq!(walk_by(&mut s, Vec3::X, 0.0, 1), turning, "standing still");
    assert!(cam(&s).focus.x - focus.x > 0.05, "the focus still caught up");
}

#[test]
fn a_lock_frames_the_same_with_the_auto_centre_on() {
    let locked = |on: bool| {
        let mut s = timed_scene(on);
        tap_lock(&mut s);
        assert_eq!(target(&s), Some(id(&s.app, s.near)));
        walk(&mut s, Vec3::X, 40);
        assert_eq!(target(&s), Some(id(&s.app, s.near)), "still locked");
        (cam(&s).yaw, cam(&s).look_idle_s)
    };
    let (on, off) = (locked(true), locked(false));
    assert!((on.0 - off.0).abs() < 1e-6, "{on:?} {off:?}");
    assert_eq!((on.1, off.1), (0.0, 0.0), "a lock starts the wait over");
}

#[test]
fn a_paused_game_does_not_run_the_flick_cooldown_down() {
    let mut s = scene();
    tap_lock(&mut s);
    let (near, side) = (id(&s.app, s.near), id(&s.app, s.side));
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(30.0, 0.0) });
    s.app.update();
    assert_eq!(target(&s), Some(side));
    s.app.insert_resource(AccumulatedMouseMotion::default());
    s.app.update();

    // Wall-clock time passes, the game stays paused (Time<Virtual> is paused in these tests): the cooldown is where it was.
    assert!(s.app.world().resource::<Time<Virtual>>().is_paused());
    s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(400)));
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(-30.0, 0.0) });
    s.app.update();
    assert!(s.app.world().resource::<Time<Real>>().delta_secs() >= 0.39, "real time did pass");
    s.app.insert_resource(TimeUpdateStrategy::Automatic);
    assert_eq!(target(&s), Some(side), "paused: the cooldown has not run down, so no switch");
    s.app.insert_resource(AccumulatedMouseMotion::default());

    // Simulation time passes: it can switch back.
    run_sim(&mut s, 0.4);
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(-30.0, 0.0) });
    s.app.update();
    assert_eq!(target(&s), Some(near));
}

#[test]
fn the_flick_numbers_are_the_tunings() {
    let mut s = scene();
    // A 110 px threshold with a sum that drains: 30 px does nothing now.
    s.app.world_mut().resource_mut::<CameraTuning>().flick =
        FlickTuning { mouse_threshold_px: 110.0, mouse_decay_px_s: Some(600.0), ..default() };
    tap_lock(&mut s);
    let (near, side) = (id(&s.app, s.near), id(&s.app, s.side));
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(30.0, 0.0) });
    s.app.update();
    assert_eq!(target(&s), Some(near));
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(150.0, 0.0) });
    s.app.update();
    assert_eq!(target(&s), Some(side));
}

#[test]
fn a_hit_shakes_the_view_and_never_the_stored_yaw_the_walk_is_turned_by() {
    let mut s = scene();
    s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap().yaw = 0.7;
    frame(&mut s.app);
    s.app.world_mut().commands().trigger(AddTrauma(1.0));
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    let steady = s.app.world().get::<Transform>(s.camera).unwrap().rotation;
    let mut moved_view = false;
    for _ in 0..6 {
        s.app.update();
        moved_view |= s.app.world().get::<Transform>(s.camera).unwrap().rotation.angle_between(steady) > 1e-4;
        assert!((cam(&s).yaw - 0.7).abs() < 1e-6, "the stored yaw has no shake in it: {}", cam(&s).yaw);
        let want = Quat::from_rotation_y(0.7) * Vec3::NEG_Z;
        let dir = s.app.world().get::<Intent>(s.player).unwrap().move_dir;
        assert!((dir - want).length() < 1e-5, "the walk is turned by the yaw, not the jittered view: {dir}");
    }
    assert!(moved_view, "the view did shake");
}

#[test]
fn the_offsets_add_to_the_base_values_and_leave_no_trace() {
    let mut s = scene();
    let fov = |s: &Scene| match s.app.world().get::<Projection>(s.camera).unwrap() {
        Projection::Perspective(p) => p.fov.to_degrees(),
        _ => panic!("perspective"),
    };
    frame(&mut s.app);
    frame(&mut s.app);
    let (base_fov, base_at, base_cam) = (fov(&s), camera_at(&s), cam(&s));
    assert!((base_fov - 70.0).abs() < 1e-3);

    // The sprint widens the view by 5 degrees; the player's setting is still the base.
    s.app.insert_resource(CameraOffsets { fov_add_deg: 5.0, ..default() });
    frame(&mut s.app);
    assert!((fov(&s) - 75.0).abs() < 1e-3, "{}", fov(&s));
    assert_eq!(s.app.world().resource::<CameraSettings>().fov_deg, 70.0, "the setting is not written");
    // The sum is held to the engine's limit.
    s.app.insert_resource(CameraOffsets { fov_add_deg: 80.0, ..default() });
    frame(&mut s.app);
    assert!((fov(&s) - 120.0).abs() < 1e-3);

    // A system that wrote a NaN or an infinity does not poison the view.
    s.app.insert_resource(CameraOffsets { fov_add_deg: f32::NAN, focus_dip_m: f32::INFINITY, ..default() });
    frame(&mut s.app);
    assert!((fov(&s) - 70.0).abs() < 1e-3 && camera_at(&s).is_finite(), "{} {}", fov(&s), camera_at(&s));

    // A landing dip lowers the view without tilting it, and the focus keeps no dip.
    s.app.insert_resource(CameraOffsets { focus_dip_m: 0.12, ..default() });
    frame(&mut s.app);
    let dipped = camera_at(&s);
    assert!(
        (dipped.y - (base_at.y - 0.12)).abs() < 1e-4 && (dipped.x - base_at.x).abs() < 1e-4 && (dipped.z - base_at.z).abs() < 1e-4,
        "{dipped} vs {base_at}"
    );
    assert_eq!(cam(&s).focus, base_cam.focus, "the stored focus is untouched");

    // Written back to neutral, everything is as it was the very next frame: the camera kept nothing.
    s.app.insert_resource(CameraOffsets::default());
    frame(&mut s.app);
    assert!((fov(&s) - base_fov).abs() < 1e-3);
    assert!((camera_at(&s) - base_at).length() < 1e-4, "{} vs {base_at}", camera_at(&s));
}

#[test]
fn the_pitch_limits_come_from_the_tuning_and_a_cap_clamps_the_stored_pitch() {
    let mut s = scene();
    s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap().pitch = 1.3;
    frame(&mut s.app);
    assert_eq!(cam(&s).pitch, 1.3, "inside the default limits");

    // Godot-style limits: the tuning clamps the stored pitch.
    s.app.world_mut().resource_mut::<CameraTuning>().pitch = (-1.1, 0.5);
    frame(&mut s.app);
    assert_eq!(cam(&s).pitch, 0.5);

    // A swim cap, which the game lifts again, clamps the stored pitch too.
    s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap().pitch = 0.3;
    s.app.insert_resource(CameraOffsets { pitch_max: Some(0.1), ..default() });
    frame(&mut s.app);
    assert_eq!(cam(&s).pitch, 0.1, "the cap clamps the stored pitch");
    s.app.insert_resource(CameraOffsets::default());
    frame(&mut s.app);
    assert_eq!(cam(&s).pitch, 0.1, "lifting the cap does not push it back up");

    // The mouse stops at the tuning's limit (down is pitch up: a downward motion raises the camera).
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(0.0, -2000.0) });
    s.app.update();
    let top = cam(&s).pitch;
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(0.0, 2000.0) });
    s.app.update();
    let bottom = cam(&s).pitch;
    let (hi, lo) = (top.max(bottom), top.min(bottom));
    assert!((hi - 0.5).abs() < 1e-6 && (lo + 1.1).abs() < 1e-6, "limits (-1.1, 0.5), got {lo} and {hi}");
}

#[test]
fn the_arm_grows_back_after_a_wall_at_the_tunings_rate() {
    let wall_then_free = |rate: Option<f32>| {
        let mut s = scene();
        s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(16)));
        s.app.world_mut().resource_mut::<CameraTuning>().arm.follow_rate = rate;
        {
            let mut c = s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap();
            c.yaw = 0.0;
            c.pitch = 0.0;
        }
        for _ in 0..3 {
            frame(&mut s.app);
        }
        let open = cam(&s).arm_reach;
        assert!((open - 5.5).abs() < 0.05, "nothing in the way: {open}");
        let wall = block(s.app.world_mut(), Vec3::new(-20.0, 0.0, 2.0), Vec3::new(20.0, 5.0, 3.0));
        frame(&mut s.app);
        frame(&mut s.app);
        let shut = cam(&s).arm_reach;
        s.app.world_mut().despawn(wall);
        frame(&mut s.app);
        let one = cam(&s).arm_reach;
        for _ in 0..200 {
            frame(&mut s.app);
        }
        (open, shut, one, cam(&s).arm_reach)
    };

    let (open, shut, one, end) = wall_then_free(Some(4.0));
    assert!((0.7..2.0).contains(&shut), "a wall pulls it in at once: {shut}");
    assert!(one > shut && one < shut + 0.5, "one frame grows it a little, not back: {one} from {shut}");
    assert!((end - open).abs() < 0.05, "and it ends where it started: {end}");

    let (_, shut, one, _) = wall_then_free(None);
    assert!(shut < 2.0);
    assert!((one - 5.5).abs() < 0.05, "without a rate it is back at once, as the camera always did: {one}");
}

/// The scene with a `lock_on` definition: the lock-on rules are on.
fn with_lock_rules(s: &mut Scene) {
    use shards_content::db::Source;
    use shards_content::{ContentDb, ContentKinds};
    let mut kinds = ContentKinds::default();
    kinds.register::<shards_motion::camera::LockOnDef>();
    let db = ContentDb::build(&kinds, &[Source { path: "content/lock.lock_on.ron".into(), text: "{ \"default\": () }".into() }]);
    assert_eq!(db.errors().count(), 0);
    s.app.insert_resource(db);
}

fn switch_presses(s: &Scene) -> Vec<f32> {
    let intent = s.app.world().get::<Intent>(s.player).unwrap();
    intent.pressed.iter().filter(|p| p.input == "lock_switch").map(|p| p.dir.unwrap().x).collect()
}

#[test]
fn with_lock_rules_the_camera_picks_by_score_and_leaves_the_switch_to_the_simulation() {
    let mut s = scene();
    with_lock_rules(&mut s);
    tap_lock(&mut s);
    assert_eq!(target(&s), Some(id(&s.app, s.near)), "the scored pick: near and in the middle of the view");
    // A flick to the right is a `lock_switch` press to the right, which the action module
    // carries out on the next tick (this scene has none): the camera does not switch itself.
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(30.0, 0.0) });
    s.app.update();
    s.app.insert_resource(AccumulatedMouseMotion::default());
    assert_eq!(target(&s), Some(id(&s.app, s.near)));
    assert_eq!(switch_presses(&s), [1.0]);
    // A flick to the left finds nobody there: no press (and, by the tuning, no cooldown).
    run_sim(&mut s, 0.4);
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(-30.0, 0.0) });
    s.app.update();
    s.app.insert_resource(AccumulatedMouseMotion::default());
    assert_eq!(switch_presses(&s), [1.0]);
}

#[test]
fn with_lock_rules_the_wheel_switches_and_the_camera_lets_go_only_when_the_player_cancels() {
    use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
    let mut s = scene();
    with_lock_rules(&mut s);
    tap_lock(&mut s);
    // The wheel turned toward the player: to the right.
    s.app.insert_resource(AccumulatedMouseScroll { unit: MouseScrollUnit::Line, delta: Vec2::new(0.0, -1.0) });
    s.app.update();
    assert_eq!(switch_presses(&s), [1.0]);
    // A touchpad that keeps scrolling past the cooldown is still that one switch ...
    s.app.insert_resource(AccumulatedMouseScroll { unit: MouseScrollUnit::Pixel, delta: Vec2::new(0.0, -3.0) });
    run_sim(&mut s, 0.6);
    assert_eq!(switch_presses(&s), [1.0]);
    // ... and after a frame standing still the next turn switches again.
    s.app.insert_resource(AccumulatedMouseScroll::default());
    frame(&mut s.app);
    s.app.insert_resource(AccumulatedMouseScroll { unit: MouseScrollUnit::Line, delta: Vec2::new(0.0, -1.0) });
    frame(&mut s.app);
    s.app.insert_resource(AccumulatedMouseScroll::default());
    assert_eq!(switch_presses(&s), [1.0, 1.0]);
    // Far beyond the camera's own release distance: the simulation decides, not the camera.
    s.app.world_mut().get_mut::<Transform>(s.near).unwrap().translation = Vec3::new(0.5, 0.0, -60.0);
    frame(&mut s.app);
    frame(&mut s.app);
    assert_eq!(target(&s), Some(id(&s.app, s.near)));
    // The player's cancel lets go, and says so as a local line (the camera is input).
    tap_lock(&mut s);
    assert_eq!(target(&s), None);
    let near = id(&s.app, s.near);
    let journal = s.app.world().resource::<ai_core::AiJournal>();
    let local: Vec<String> = journal.after(0).0.filter(|e| e.kind == ai_core::JournalKind::Local).map(|e| e.to_string()).collect();
    assert!(local.iter().any(|l| l.ends_with(&format!("lock released: {near}"))), "{local:?}");
}

/// The player moved from x = 0 to x = 1 over the last tick, and half of the next is owed: the
/// focus is where the body is drawn (x = 0.5), not where the tick left it.
fn halfway_scene(interpolation: bool) -> Scene {
    let mut s = scene();
    s.app.insert_resource(shards_settings::GraphicsSettings { interpolation, ..default() });
    s.app.world_mut().spawn((Transform::default(), ChildOf(s.player)));
    s.app.update(); // the child is adopted
    for (tick, x) in [(1, 0.0), (2, 1.0)] {
        ai_core::sim::set_tick(s.app.world_mut(), tick);
        s.app.world_mut().get_mut::<Transform>(s.player).unwrap().translation.x = x;
        s.app.world_mut().run_schedule(FixedLast);
    }
    // A running clock that adds no time: frames, no ticks.
    s.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    ai_core::sim::resume(s.app.world_mut());
    let half = s.app.world().resource::<Time<Fixed>>().timestep() / 2;
    s.app.world_mut().resource_mut::<Time<Fixed>>().accumulate_overstep(half);
    // The focus lands on the followed body at once instead of catching up.
    s.app.world_mut().get_mut::<ThirdPersonCamera>(s.camera).unwrap().focus = Vec3::ZERO;
    s.app.update();
    s
}

#[test]
fn the_camera_follows_the_body_where_it_is_drawn_between_ticks() {
    let s = halfway_scene(true);
    let focus = cam(&s).focus;
    assert!((focus.x - 0.5).abs() < 1e-3, "{focus}");
    assert_eq!(s.app.world().get::<Transform>(s.player).unwrap().translation.x, 1.0, "the body itself is the tick's");
}

#[test]
fn with_interpolation_off_the_camera_follows_the_tick_pose() {
    let s = halfway_scene(false);
    assert!((cam(&s).focus.x - 1.0).abs() < 1e-3, "{}", cam(&s).focus);
}
