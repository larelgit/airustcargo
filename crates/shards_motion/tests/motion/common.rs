//! Headless test worlds: `MinimalPlugins` + `AiCorePlugin` (paused, no HTTP) + `MotionPlugin`,
//! advanced tick by tick with `ai_core::sim::step`.

#![allow(dead_code)]

use std::time::Duration;

use ai_core::sim::StepEnd;
use ai_core::{AiCorePlugin, SimConfig};
use bevy::prelude::*;
use shards_motion::*;

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin {
            remote_port: None,
            crash_replays: false,
            sim: SimConfig { start_paused: true, ..default() },
            ..default()
        })
        .add_plugins(MotionPlugin)
        .init_resource::<Seen>()
        .add_observer(|e: On<Landed>, mut seen: ResMut<Seen>| seen.landed.push(*e.event()))
        .add_observer(|e: On<Jumped>, mut seen: ResMut<Seen>| seen.jumped.push(*e.event()))
        .add_observer(|e: On<Mantled>, mut seen: ResMut<Seen>| seen.mantled.push(*e.event()))
        .add_observer(|e: On<ModeChanged>, mut seen: ResMut<Seen>| seen.modes.push(*e.event()))
        .add_observer(|e: On<VolumeEntered>, mut seen: ResMut<Seen>| seen.entered.push(*e.event()))
        .add_observer(|e: On<VolumeExited>, mut seen: ResMut<Seen>| seen.exited.push(*e.event()));
    app.finish();
    app.cleanup();
    app.update();
    app
}

/// Every motion event, in order.
#[derive(Resource, Default, Debug)]
pub struct Seen {
    pub landed: Vec<Landed>,
    pub jumped: Vec<Jumped>,
    pub mantled: Vec<Mantled>,
    pub modes: Vec<ModeChanged>,
    pub entered: Vec<VolumeEntered>,
    pub exited: Vec<VolumeExited>,
}

pub fn seen(app: &App) -> &Seen {
    app.world().resource::<Seen>()
}

/// Runs `ticks` ticks; a gameplay panic fails the test.
pub fn step(app: &mut App, ticks: u64) {
    let report = ai_core::sim::step(app.world_mut(), ticks, Duration::from_secs(120), |_| None).unwrap();
    assert!(!matches!(report.end, StepEnd::SystemFailed(_)), "a system failed: {:?}", report.end);
}

/// Runs ticks one by one, calling `each` before every tick; stops early when it
/// returns `false`. Returns the ticks run.
pub fn drive(app: &mut App, max_ticks: u64, mut each: impl FnMut(&mut World) -> bool) -> u64 {
    for n in 0..max_ticks {
        if !each(app.world_mut()) {
            return n;
        }
        step(app, 1);
    }
    max_ticks
}

/// A static box between two corners.
pub fn block(world: &mut World, min: Vec3, max: Vec3) -> Entity {
    let half = (max - min) * 0.5;
    world.spawn((Collider::new(Shape::cuboid(half)), Transform::from_translation(min + half))).id()
}

/// A box turned by `rot` about its center.
pub fn turned_block(world: &mut World, center: Vec3, half: Vec3, rot: Quat) -> Entity {
    world.spawn((Collider::new(Shape::cuboid(half)), Transform::from_translation(center).with_rotation(rot))).id()
}

/// Flat ground: the top of a big slab at y = 0.
pub fn floor(world: &mut World) -> Entity {
    block(world, Vec3::new(-200.0, -1.0, -200.0), Vec3::new(200.0, 0.0, 200.0))
}

pub fn volume(world: &mut World, min: Vec3, max: Vec3, kind: VolumeKind) -> Entity {
    let half = (max - min) * 0.5;
    world
        .spawn((
            Collider::new(Shape::cuboid(half)).with_layers(CollisionLayers::VOLUME),
            Volume { kind },
            Transform::from_translation(min + half),
        ))
        .id()
}

pub fn hero(world: &mut World, feet: Vec3) -> Entity {
    hero_with(world, feet, CharacterParams::default())
}

/// A character moved by its own numbers; no `Intent`, so tests write `LocomotionInput` directly.
pub fn hero_with(world: &mut World, feet: Vec3, params: CharacterParams) -> Entity {
    world.spawn((Name::new("Hero"), CharacterController::default(), params, Transform::from_translation(feet))).id()
}

pub fn feet(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Transform>(e).unwrap().translation
}

pub fn state(app: &App, e: Entity) -> MotionState {
    app.world().get::<MotionState>(e).unwrap().clone()
}

pub fn set_input(world: &mut World, e: Entity, input: LocomotionInput) {
    *world.get_mut::<LocomotionInput>(e).unwrap() = input;
}

pub fn walk(world: &mut World, e: Entity, dir: Vec3) {
    world.get_mut::<LocomotionInput>(e).unwrap().move_dir = dir;
}

pub fn press_jump(world: &mut World, e: Entity) {
    world.get_mut::<LocomotionInput>(e).unwrap().jump = true;
}

/// The character's capsule is not inside any static geometry (beyond float noise).
pub fn assert_clear(app: &App, e: Entity) {
    let world = app.world();
    let params = world.get::<CharacterParams>(e).cloned().unwrap_or_default();
    let params = &params;
    let at = world.get::<Transform>(e).unwrap().translation + Vec3::Y * params.height * 0.5;
    let shape = Shape::capsule(params.radius, params.height);
    let filter = QueryFilter::new(LayerMask::WORLD).excluding(e);
    let deep: Vec<Penetration> = world
        .resource::<PhysicsWorld>()
        .penetrations(&shape, at, Quat::IDENTITY, &filter)
        .into_iter()
        .filter(|p| p.depth > 1e-3)
        .collect();
    assert!(deep.is_empty(), "character at {at} sinks into {deep:?}");
}
