//! The first-person camera (F1b): the mouse turns `Intent::look` (and only when it moves), the camera
//! sits at the eye of the body it looks through and turns by whatever the look holds, the own body is
//! on a layer the camera does not draw, and a game that asked for the mode gets it from content.

use std::time::Duration;

use ai_core::{AiCorePlugin, AiKind, SimConfig};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera, Camera3d, Projection};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::light::{DirectionalLight, PointLight};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use shards_core::{Gaze, Intent, LocalPlayer};
use shards_motion::camera::{
    BobTuning, CameraMode, CameraOffsets, CameraTuning, FirstPersonCamera, FirstPersonTuning, FromMode, OWN_BODY_LAYER,
    VIEW_MODEL_LAYER,
};
use shards_motion::*;

use crate::common::*;

struct Fp {
    app: App,
    player: Entity,
    camera: Entity,
}

/// A body that faces its look, eye 1.6 m, pitch held to ±80 degrees, at the origin; a first-person
/// camera on its own entity looking through the local player.
fn fp() -> Fp {
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
    // A frame is 16 ms, the same every time: the smoothing is a function of the frame time.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(16)));
    floor(app.world_mut());
    let params = CharacterParams { face: Facing::Look, eye_height: Some(1.6), pitch_limits_deg: (-80.0, 80.0), ..default() };
    let player = app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO), params)).id();
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), FirstPersonCamera::default(), Projection::Perspective(PerspectiveProjection::default()), Transform::default()))
        .id();
    app.update();
    app.update();
    Fp { app, player, camera }
}

fn frame(fp: &mut Fp) {
    fp.app.update();
    fp.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
    fp.app.insert_resource(AccumulatedMouseMotion::default());
}

/// A frame with the mouse moved by `pixels` (right, down).
fn mouse(fp: &mut Fp, pixels: Vec2) {
    fp.app.insert_resource(AccumulatedMouseMotion { delta: pixels });
    fp.app.update();
    fp.app.insert_resource(AccumulatedMouseMotion::default());
}

fn look(fp: &Fp) -> Option<Gaze> {
    fp.app.world().get::<Intent>(fp.player).unwrap().look
}

fn camera_at(fp: &Fp) -> Transform {
    *fp.app.world().get::<Transform>(fp.camera).unwrap()
}

fn set_look(fp: &mut Fp, look: Option<Gaze>) {
    fp.app.world_mut().get_mut::<Intent>(fp.player).unwrap().look = look;
}

#[test]
fn the_mouse_turns_the_look_and_the_camera_looks_along_it() {
    let mut s = fp();
    assert_eq!(look(&s), None, "a run that has not moved the mouse has no look");
    // 100 px right at the map's 0.0025 rad/px: a quarter radian to the right.
    mouse(&mut s, Vec2::new(100.0, 0.0));
    let turned = look(&s).expect("the mouse wrote a look");
    assert!((turned.yaw + 0.25).abs() < 1e-4 && turned.pitch == 0.0, "{turned:?}");
    // Up is up: the mouse moves up the screen by a negative y.
    mouse(&mut s, Vec2::new(0.0, -100.0));
    let up = look(&s).unwrap();
    assert!((up.pitch - 0.25).abs() < 1e-4 && (up.yaw - turned.yaw).abs() < 1e-6, "{up:?}");
    let forward = camera_at(&s).rotation * Vec3::NEG_Z;
    assert!(forward.distance(up.dir()) < 1e-4, "the camera looks along the look: {forward} against {}", up.dir());
}

#[test]
fn a_still_mouse_writes_nothing_and_a_look_somebody_else_wrote_is_the_one_drawn() {
    let mut s = fp();
    for _ in 0..5 {
        frame(&mut s);
    }
    assert_eq!(look(&s), None, "no mouse motion, no look");
    // An agent's `look` tool (or a recorded input) writes the look; the camera turns to it.
    let written = Gaze::from_degrees(90.0, 20.0);
    set_look(&mut s, Some(written));
    for _ in 0..3 {
        frame(&mut s);
    }
    assert_eq!(look(&s), Some(written), "the camera did not touch it");
    let forward = camera_at(&s).rotation * Vec3::NEG_Z;
    assert!(forward.distance(written.dir()) < 1e-4, "{forward}");
    // The mouse then turns on from that look, not from some angle of its own.
    mouse(&mut s, Vec2::new(0.0, 0.0));
    assert_eq!(look(&s), Some(written));
    mouse(&mut s, Vec2::new(40.0, 0.0));
    let after = look(&s).unwrap();
    assert!((after.yaw - (written.yaw - 0.1)).abs() < 1e-4 && (after.pitch - written.pitch).abs() < 1e-6, "{after:?}");
    // A look cleared by an agent is not made again until the mouse moves: the camera falls back on the heading.
    set_look(&mut s, None);
    for _ in 0..3 {
        frame(&mut s);
    }
    assert_eq!(look(&s), None);
    let forward = camera_at(&s).rotation * Vec3::NEG_Z;
    assert!(forward.distance(Vec3::NEG_Z) < 1e-4, "the body's heading, level: {forward}");
}

#[test]
fn the_pitch_is_held_to_the_bodys_limits_and_the_settings_apply() {
    let mut s = fp();
    mouse(&mut s, Vec2::new(0.0, -100_000.0));
    assert!((look(&s).unwrap().pitch - 80f32.to_radians()).abs() < 1e-5, "the body's 80 degrees, not the engine's 89");
    mouse(&mut s, Vec2::new(0.0, 100_000.0));
    assert!((look(&s).unwrap().pitch + 80f32.to_radians()).abs() < 1e-5);
    set_look(&mut s, Some(Gaze::default()));

    // Sensitivity, aim sensitivity (while the game says the player aims) and inversion.
    {
        let mut settings = s.app.world_mut().resource_mut::<CameraSettings>();
        (settings.sensitivity, settings.aim_sensitivity) = (2.0, 0.5);
    }
    mouse(&mut s, Vec2::new(100.0, 0.0));
    assert!((look(&s).unwrap().yaw + 0.5).abs() < 1e-4, "100 px at 0.0025, times 2");
    set_look(&mut s, Some(Gaze::default()));
    s.app.insert_resource(CameraOffsets { aiming: true, ..default() });
    mouse(&mut s, Vec2::new(100.0, 0.0));
    assert!((look(&s).unwrap().yaw + 0.125).abs() < 1e-4, "aiming: times 0.5");
    s.app.insert_resource(CameraOffsets::default());
    set_look(&mut s, Some(Gaze::default()));
    s.app.world_mut().resource_mut::<CameraSettings>().invert_y = true;
    mouse(&mut s, Vec2::new(0.0, -100.0));
    assert!(look(&s).unwrap().pitch < 0.0, "inverted: the mouse up looks down");
}

#[test]
fn the_camera_sits_at_the_eye_and_the_field_of_view_follows_the_setting() {
    let mut s = fp();
    s.app.world_mut().resource_mut::<CameraSettings>().fov_deg = 100.0;
    frame(&mut s);
    assert!(camera_at(&s).translation.distance(Vec3::new(0.0, 1.6, 0.0)) < 1e-4, "{}", camera_at(&s).translation);
    let Projection::Perspective(p) = s.app.world().get::<Projection>(s.camera).unwrap() else { panic!("perspective") };
    assert!((p.fov - 100f32.to_radians()).abs() < 1e-5);
    // Walked off, the camera comes at once (no catching up: it is at the body's eye).
    s.app.world_mut().get_mut::<Transform>(s.player).unwrap().translation = Vec3::new(30.0, 0.0, -4.0);
    frame(&mut s);
    assert!(camera_at(&s).translation.distance(Vec3::new(30.0, 1.6, -4.0)) < 1e-3, "{}", camera_at(&s).translation);
    // The game's offsets: a wider view, a dip.
    s.app.insert_resource(CameraOffsets { fov_add_deg: 10.0, focus_dip_m: 0.2, ..default() });
    frame(&mut s);
    let Projection::Perspective(p) = s.app.world().get::<Projection>(s.camera).unwrap() else { panic!("perspective") };
    assert!((p.fov - 110f32.to_radians()).abs() < 1e-5);
    assert!((camera_at(&s).translation.y - 1.4).abs() < 1e-3);
}

#[test]
fn a_step_is_eased_and_a_landing_dips() {
    let mut s = fp();
    // Standing on the ground (the state a tick would have left).
    s.app.world_mut().entity_mut(s.player).insert(MotionState { grounded: true, ..default() });
    frame(&mut s);
    // The body steps up 0.3 m between two frames: the eye follows over a moment, not at once.
    s.app.world_mut().get_mut::<Transform>(s.player).unwrap().translation.y = 0.3;
    frame(&mut s);
    let first = camera_at(&s).translation.y;
    assert!(first > 1.6 && first < 1.9 - 0.05, "short of the body's eye, which is 1.9: {first}");
    for _ in 0..90 {
        frame(&mut s);
    }
    assert!((camera_at(&s).translation.y - 1.9).abs() < 0.01, "caught up: {}", camera_at(&s).translation.y);

    // A hard landing of the body the camera looks through knocks the eye down, and it settles back.
    let player = s.player;
    s.app.world_mut().commands().trigger(Landed { entity: player, fall_height: 3.0, impact_speed: 8.0, tier: 0, jumped: false });
    let mut lowest = f32::MAX;
    for _ in 0..90 {
        frame(&mut s);
        lowest = lowest.min(camera_at(&s).translation.y);
    }
    assert!(lowest < 1.9 - 0.04, "a dip: {lowest}");
    assert!((camera_at(&s).translation.y - 1.9).abs() < 0.01, "settled");
    // Somebody else's landing does not dip it.
    let other = s.app.world_mut().spawn(AiKind::new("Other")).id();
    s.app.world_mut().commands().trigger(Landed { entity: other, fall_height: 3.0, impact_speed: 8.0, tier: 0, jumped: false });
    frame(&mut s);
    frame(&mut s);
    assert!((camera_at(&s).translation.y - 1.9).abs() < 1e-3);
}

#[test]
fn bob_and_lean_are_off_until_the_game_asks() {
    let mut s = fp();
    // Walking sideways at 4 m/s (state a tick would have left).
    s.app.world_mut().entity_mut(s.player).insert(MotionState { velocity: Vec3::new(4.0, 0.0, 0.0), grounded: true, ..default() });
    for _ in 0..30 {
        frame(&mut s);
    }
    let still = camera_at(&s);
    assert!((still.translation.y - 1.6).abs() < 1e-6 && (still.rotation * Vec3::X).y.abs() < 1e-6, "no bob, no lean");

    let tuning = CameraTuning {
        first_person: FirstPersonTuning {
            bob: Some(BobTuning::default()),
            roll: Some(shards_motion::camera::RollTuning::default()),
            ..default()
        },
        ..default()
    };
    s.app.insert_resource(tuning);
    let mut moved = false;
    for _ in 0..60 {
        frame(&mut s);
        let at = camera_at(&s);
        moved |= (at.translation.y - 1.6).abs() > 1e-4;
    }
    assert!(moved, "the bob moves the eye");
    // Moving right (+X) while looking north: the lean is to the right, the view's right side lower.
    assert!((camera_at(&s).rotation * Vec3::X).y < -0.01, "{}", camera_at(&s).rotation * Vec3::X);
}

#[test]
fn the_walk_is_relative_to_the_look_under_a_first_person_camera_whatever_the_body_faces() {
    let mut s = fp();
    // A body that faces its movement (the default) under the same camera: W walks the way the view points.
    s.app.world_mut().entity_mut(s.player).insert(CharacterParams::default());
    set_look(&mut s, Some(Gaze::from_degrees(90.0, 0.0)));
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    frame(&mut s);
    let walk = s.app.world().get::<Intent>(s.player).unwrap().move_dir;
    assert!(walk.distance(Vec3::NEG_X) < 1e-4, "looking west, forward is west: {walk}");
}

#[test]
fn the_own_body_is_on_a_layer_the_camera_does_not_draw_and_the_lights_share_it() {
    let mut s = fp();
    let world = s.app.world_mut();
    let model = world.spawn((Mesh3d::default(), ChildOf(s.player))).id();
    let nested = world.spawn((Mesh3d::default(), RenderLayers::layer(3), ChildOf(model))).id();
    let bare = world.spawn(ChildOf(s.player)).id();
    let elsewhere = world.spawn(Mesh3d::default()).id();
    let sun = world.spawn(DirectionalLight::default()).id();
    let bulb = world.spawn((PointLight::default(), RenderLayers::layer(5))).id();
    let mirror = world.spawn(Camera3d::default()).id();
    frame(&mut s);
    frame(&mut s);
    let world = s.app.world();
    let layers = |e: Entity| world.get::<RenderLayers>(e).cloned();
    let own = RenderLayers::layer(OWN_BODY_LAYER);
    assert_eq!(layers(model), Some(own.clone()), "a mesh of the body");
    assert_eq!(layers(nested), Some(own.clone()), "however deep");
    assert_eq!(layers(bare), None, "an entity with no mesh has nothing to hide");
    assert_eq!(layers(elsewhere), None, "a mesh of something else is drawn");
    assert_eq!(layers(s.camera), None, "the first-person camera draws the default layer alone");
    assert!(!own.intersects(&RenderLayers::default()), "so it never sees the own body");

    // The lights: the sun had none and gets the world's, the own body's (its shadow) and the view model's.
    let shared = RenderLayers::from_layers(&[0, OWN_BODY_LAYER, VIEW_MODEL_LAYER]);
    assert_eq!(layers(sun), Some(shared), "a light without layers");
    assert_eq!(layers(bulb), Some(RenderLayers::layer(5)), "a light the game gave layers is its own");
    // A second camera of the world sees the body too.
    assert_eq!(layers(mirror), Some(RenderLayers::from_layers(&[0, OWN_BODY_LAYER])));

    // The camera stops looking through the body: everything goes back.
    s.app.world_mut().entity_mut(s.camera).remove::<FirstPersonCamera>();
    frame(&mut s);
    frame(&mut s);
    let world = s.app.world();
    let layers = |e: Entity| world.get::<RenderLayers>(e).cloned();
    assert_eq!((layers(model), layers(sun), layers(mirror)), (None, None, None), "the layers the engine gave are taken away");
    assert_eq!(layers(nested), Some(RenderLayers::layer(3)), "what a mesh had is back");
    assert_eq!(layers(bulb), Some(RenderLayers::layer(5)));
}

#[test]
fn a_body_that_is_no_longer_looked_through_is_drawn_again() {
    let mut s = fp();
    let model = s.app.world_mut().spawn((Mesh3d::default(), ChildOf(s.player))).id();
    let other = s.app.world_mut().spawn((AiKind::new("Other"), Transform::default())).id();
    let other_model = s.app.world_mut().spawn((Mesh3d::default(), ChildOf(other))).id();
    frame(&mut s);
    let own = Some(RenderLayers::layer(OWN_BODY_LAYER));
    assert_eq!(s.app.world().get::<RenderLayers>(model).cloned(), own);
    assert_eq!(s.app.world().get::<RenderLayers>(other_model).cloned(), None);
    // The camera is pointed at another body (a possession, a spectator): the hidden mesh comes back, the other goes.
    s.app.world_mut().get_mut::<FirstPersonCamera>(s.camera).unwrap().target = other;
    frame(&mut s);
    frame(&mut s);
    assert_eq!(s.app.world().get::<RenderLayers>(model).cloned(), None);
    assert_eq!(s.app.world().get::<RenderLayers>(other_model).cloned(), own);
}

#[test]
fn the_camera_content_can_ask_for_the_first_person_mode() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin { remote_port: None, crash_replays: false, sim: SimConfig { start_paused: true, ..default() }, ..default() })
        .add_plugins(MotionPlugin)
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>();
    app.finish();
    app.cleanup();
    let player = app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO))).id();
    let orbit = app.world_mut().spawn((Camera3d::default(), ThirdPersonCamera::following(player), Transform::default())).id();
    let own = app.world_mut().spawn((Camera3d::default(), FirstPersonCamera::default(), Transform::default())).id();
    app.update();
    assert!(!app.world().entity(orbit).contains::<FirstPersonCamera>(), "the mode is third person unless content says");

    app.insert_resource(CameraTuning { mode: CameraMode::FirstPerson, ..default() });
    app.update();
    let made = app.world().entity(orbit);
    assert!(made.contains::<FirstPersonCamera>() && made.contains::<FromMode>() && made.contains::<ThirdPersonCamera>());
    assert_eq!(made.get::<FirstPersonCamera>().unwrap().target, player, "it looks through what the orbit camera followed");

    app.insert_resource(CameraTuning::default());
    app.update();
    let back = app.world().entity(orbit);
    assert!(!back.contains::<FirstPersonCamera>() && !back.contains::<FromMode>() && back.contains::<ThirdPersonCamera>());
    assert!(app.world().entity(own).contains::<FirstPersonCamera>(), "a camera the game made first-person stays so");
}

#[test]
fn the_third_person_camera_stands_still_under_a_first_person_one_and_lock_on_waits() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin { remote_port: None, crash_replays: false, sim: SimConfig { start_paused: true, ..default() }, ..default() })
        .add_plugins(MotionPlugin)
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>();
    app.finish();
    app.cleanup();
    floor(app.world_mut());
    let params = CharacterParams { face: Facing::Look, eye_height: Some(1.6), ..default() };
    let player = app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO), params)).id();
    let camera = app
        .world_mut()
        .spawn((Camera3d::default(), ThirdPersonCamera::following(player), FirstPersonCamera::default(), Projection::Perspective(default()), Transform::default()))
        .id();
    app.world_mut().spawn((AiKind::new("Enemy"), LockOnTarget, Transform::from_xyz(0.0, 0.0, -10.0)));
    app.update();
    app.update();
    let eye = app.world().get::<Transform>(camera).unwrap().translation;
    assert!(eye.distance(Vec3::new(0.0, 1.6, 0.0)) < 1e-3, "the first-person camera has the transform, not the orbit's: {eye}");
    // The lock-on key finds nothing to lock with a first-person camera.
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Tab);
    app.update();
    assert_eq!(app.world().get::<Intent>(player).unwrap().target, None);
}

#[test]
fn the_camera_mode_tool_chooses_the_mode_for_the_session() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin { remote_port: None, crash_replays: false, sim: SimConfig { start_paused: true, ..default() }, ..default() })
        .add_plugins(MotionPlugin)
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>();
    app.finish();
    app.cleanup();
    let player = app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO))).id();
    let orbit = app.world_mut().spawn((Camera3d::default(), ThirdPersonCamera::following(player), Transform::default())).id();
    app.update();
    let tool = |app: &mut App, args: serde_json::Value| {
        let out = ai_core::call_tool(app.world_mut(), "dev", "camera_mode", args);
        (out.ok, out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned))
    };
    let (ok, said) = tool(&mut app, serde_json::json!({}));
    assert!(ok && said.contains("camera mode: third person (the camera content; the content says third person); 0 first-person cameras now"), "{said}");

    let (ok, said) = tool(&mut app, serde_json::json!({"mode": "first"}));
    assert!(ok && said.contains("first person (this session's choice; the content says third person)"), "{said}");
    app.update();
    assert!(app.world().entity(orbit).contains::<FirstPersonCamera>(), "the orbit camera's entity looks through its body now");
    let (_, said) = tool(&mut app, serde_json::json!({}));
    assert!(said.contains("1 first-person camera now"), "{said}");

    let (ok, said) = tool(&mut app, serde_json::json!({"mode": "content"}));
    assert!(ok && said.contains("third person (the camera content"), "{said}");
    app.update();
    assert!(!app.world().entity(orbit).contains::<FirstPersonCamera>(), "and back");

    let (ok, said) = tool(&mut app, serde_json::json!({"mode": "orbit"}));
    assert!(!ok && said.contains("no mode `orbit`: say `first`, `third` or `content`"), "{said}");
}

#[test]
fn the_lock_on_button_only_lets_go_under_a_first_person_camera() {
    let mut s = fp();
    let enemy = s.app.world_mut().spawn((AiKind::new("Enemy"), LockOnTarget, Transform::from_xyz(0.0, 0.0, -10.0))).id();
    s.app.update();
    let enemy_id = *s.app.world().get::<ai_core::AiId>(enemy).unwrap();
    let tap = |s: &mut Fp| {
        s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::Tab);
        frame(s);
        s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::Tab);
        frame(s);
    };
    let target = |s: &Fp| s.app.world().get::<Intent>(s.player).unwrap().target;
    // Nothing is picked: the orbit camera's lock-on is not there, though a body that could be locked is in front.
    tap(&mut s);
    assert_eq!(target(&s), None);
    // A lock that is held (a tool's, or the orbit camera's before the mode changed) is let go of.
    s.app.world_mut().get_mut::<Intent>(s.player).unwrap().target = Some(enemy_id);
    frame(&mut s);
    assert_eq!(target(&s), Some(enemy_id), "the camera does not take it away by itself");
    tap(&mut s);
    assert_eq!(target(&s), None);
    let lines: Vec<String> = s.app.world().resource::<ai_core::AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(lines.iter().any(|l| l.contains("lock released")), "{lines:?}");
}

#[test]
fn a_first_person_camera_of_another_body_or_one_switched_off_leaves_the_orbit_camera_alone() {
    // A first-person camera that looks through another body (a spectator's view of somebody else).
    let mut s = super::scene();
    let other = s.app.world_mut().spawn((AiKind::new("Other"), Transform::default())).id();
    s.app.world_mut().spawn((Camera3d::default(), FirstPersonCamera::looking_through(other), Transform::default()));
    super::tap_lock(&mut s);
    assert!(super::target(&s).is_some(), "the orbit camera still picks a target for the local player");
    // The walk is turned by the orbit camera's yaw, not by a look the local player has not got from a camera.
    s.app.world_mut().get_mut::<Intent>(s.player).unwrap().target = None;
    s.app.world_mut().get_mut::<Intent>(s.player).unwrap().look = Some(Gaze::from_degrees(90.0, 0.0));
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    s.app.update();
    let walk = s.app.world().get::<Intent>(s.player).unwrap().move_dir;
    assert!(walk.distance(Vec3::NEG_Z) < 1e-3, "the orbit camera behind a body that looks north: forward is north, not the stale look's west: {walk}");

    // One that looks through the local player but is switched off.
    let mut s = super::scene();
    s.app.world_mut().spawn((Camera3d::default(), Camera { is_active: false, ..default() }, FirstPersonCamera::default(), Transform::default()));
    super::tap_lock(&mut s);
    assert!(super::target(&s).is_some(), "an inactive first-person camera does not turn the local player's view off");
}

#[test]
fn a_camera_that_is_switched_off_is_nobodys_view() {
    let mut s = fp();
    assert!(s.app.world().get::<FirstPersonCamera>(s.camera).unwrap().active);
    s.app.world_mut().get_mut::<Camera>(s.camera).unwrap().is_active = false;
    frame(&mut s);
    assert!(!s.app.world().get::<FirstPersonCamera>(s.camera).unwrap().active, "the first-person camera knows it is off");
    // The walk is then the orbit camera's (here none: north), not the look of a camera nobody sees through.
    s.app.world_mut().get_mut::<Intent>(s.player).unwrap().look = Some(Gaze::from_degrees(90.0, 0.0));
    s.app.world_mut().entity_mut(s.player).insert(CharacterParams::default());
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    frame(&mut s);
    let walk = s.app.world().get::<Intent>(s.player).unwrap().move_dir;
    assert!(walk.distance(Vec3::NEG_Z) < 1e-3, "{walk}");
    // The own body is shown again while the camera is off.
    let model = s.app.world_mut().spawn((Mesh3d::default(), ChildOf(s.player))).id();
    frame(&mut s);
    assert!(s.app.world().get::<RenderLayers>(model).is_none());
    s.app.world_mut().get_mut::<Camera>(s.camera).unwrap().is_active = true;
    frame(&mut s);
    frame(&mut s);
    assert_eq!(s.app.world().get::<RenderLayers>(model).cloned(), Some(RenderLayers::layer(OWN_BODY_LAYER)), "and hidden when it is on");
}

#[test]
fn before_the_first_mouse_move_forward_is_the_way_the_camera_already_looks() {
    // An active first-person camera over a body that faces east and has no look yet: the camera draws by the
    // body's heading, and the keys walk the same way, not by an orbit camera's yaw or by north.
    let mut s = fp();
    s.app.world_mut().entity_mut(s.player).insert(CharacterParams::default());
    s.app.world_mut().get_mut::<Transform>(s.player).unwrap().rotation = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    s.app.world_mut().spawn(ThirdPersonCamera::following(s.player));
    assert_eq!(look(&s), None);
    s.app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    frame(&mut s);
    let forward = camera_at(&s).rotation * Vec3::NEG_Z;
    let walk = s.app.world().get::<Intent>(s.player).unwrap().move_dir;
    assert!(forward.distance(Vec3::X) < 1e-4, "the view looks east: {forward}");
    assert!(walk.distance(Vec3::X) < 1e-4, "and W walks east, as the view looks: {walk}");
}

#[test]
fn a_plain_camera_that_becomes_a_first_person_one_gives_the_own_body_layer_back() {
    let mut s = fp();
    // A second view of the world (a mirror) given the layer while the first-person camera exists.
    let mirror = s.app.world_mut().spawn(Camera3d::default()).id();
    frame(&mut s);
    frame(&mut s);
    let both = RenderLayers::from_layers(&[0, OWN_BODY_LAYER]);
    assert_eq!(s.app.world().get::<RenderLayers>(mirror).cloned(), Some(both));
    // The game (or `mode: FirstPerson`) makes it a first-person camera: it draws the world alone, and so never the body.
    s.app.world_mut().entity_mut(mirror).insert(FirstPersonCamera::default());
    frame(&mut s);
    frame(&mut s);
    assert_eq!(s.app.world().get::<RenderLayers>(mirror).cloned(), None, "it must not see the body it hides");
    // The camera that carries its own layers is left alone.
    let own = s.app.world_mut().spawn((Camera3d::default(), RenderLayers::layer(7))).id();
    s.app.world_mut().entity_mut(own).insert(FirstPersonCamera::default());
    frame(&mut s);
    frame(&mut s);
    assert_eq!(s.app.world().get::<RenderLayers>(own).cloned(), Some(RenderLayers::layer(7)));
}

#[test]
fn the_crouch_shows_in_the_first_person_and_the_step_ease_does_not_fight_it() {
    use shards_motion::controller::Crouch;
    let mut s = fp();
    // A body that can crouch: its eye keeps its place in a capsule that goes from 1.8 m to 1.0 m.
    s.app.world_mut().entity_mut(s.player).insert((
        CharacterParams { face: Facing::Look, eye_height: Some(1.6), crouch_height: Some(1.0), crouch_time: 0.2, ..default() },
        MotionState { grounded: true, ..default() },
        Crouch::default(),
    ));
    frame(&mut s);
    assert!((camera_at(&s).translation.y - 1.6).abs() < 1e-4, "standing: the standing eye");
    let eye = |s: &Fp| shards_motion::eye_of(s.app.world(), s.player).unwrap().y;

    // The controller eases the depth tick by tick (here one step a frame, in 12 of them); the camera follows
    // `eye_of` within a few centimetres, never above it while going down, and lands exactly on it.
    let mut worst: f32 = 0.0;
    for step in 1..=12 {
        s.app.world_mut().get_mut::<Crouch>(s.player).unwrap().depth = step as f32 / 12.0;
        frame(&mut s);
        let (camera, body) = (camera_at(&s).translation.y, eye(&s));
        assert!(camera >= body - 1e-4, "the eased eye is above or on the body's own eye while it goes down: {camera} against {body}");
        worst = worst.max(camera - body);
    }
    assert!(worst < 0.13, "the lag of the ease behind a 0.2 s crouch of 71 cm (3.7 m/s) is 12 cm at the most: {worst}");
    for _ in 0..40 {
        frame(&mut s);
    }
    let (camera, body) = (camera_at(&s).translation.y, eye(&s));
    assert!((camera - body).abs() < 1e-3 && (body - 0.8889).abs() < 1e-3, "crouched: the camera is at the crouched eye, {camera} against {body}");

    // Standing up: the same the other way.
    s.app.world_mut().get_mut::<Crouch>(s.player).unwrap().depth = 0.0;
    for _ in 0..40 {
        frame(&mut s);
    }
    assert!((camera_at(&s).translation.y - 1.6).abs() < 1e-3);
}
