//! Headless test worlds: `MinimalPlugins` + `AiCorePlugin` (paused, no HTTP) + physics,
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
        .add_plugins(PhysicsPlugin);
    app.finish();
    app.cleanup();
    app.update();
    app
}

/// Runs `ticks` ticks; a gameplay panic fails the test.
pub fn step(app: &mut App, ticks: u64) {
    let report = ai_core::sim::step(app.world_mut(), ticks, Duration::from_secs(120), |_| None).unwrap();
    assert!(!matches!(report.end, StepEnd::SystemFailed(_)), "a system failed: {:?}", report.end);
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
