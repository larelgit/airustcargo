//! The camera systems (the `render` feature): lock-on writes `Intent.target`, the orbit
//! follows, looks, keeps out of walls, shakes, and sets the field of view. Pure math is
//! tested in `camera::math`.

use ai_core::{AiCorePlugin, AiId, AiIndex, AiKind, SimConfig};
use bevy::camera::Projection;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use shards_core::{Intent, LocalPlayer};
use shards_motion::camera::{AddTrauma, CameraShake};
use shards_motion::*;

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
    // A switch needs a moment before the next one (real time, as the player feels it).
    s.app.update();
    std::thread::sleep(std::time::Duration::from_millis(400));
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
