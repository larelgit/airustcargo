//! Test worlds with the engine around the motion module (asset folder, content, core,
//! `Intent` as a replay input) and helpers to drive them with tools like an agent does.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ai_core::{call_tool, AiKind, AiPlayer, CallOutcome, SimGate};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde_json::Value;
use shards_content::ContentPlugin;
use shards_core::{CorePlugin, EngineConfig, EngineState, Intent};
use shards_motion::*;

use crate::common::Seen;

/// A character the way a game spawns one: `Intent`, and the agent's body.
pub fn player(world: &mut World, feet: Vec3) -> Entity {
    player_with(world, feet, "")
}

/// Like [`player`], moving by the `locomotion` content id `params`.
pub fn player_with(world: &mut World, feet: Vec3, params: &str) -> Entity {
    world.spawn((Name::new("Hero"), AiKind::new("Hero"), AiPlayer, character(params, feet))).id()
}

pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

/// A call that must succeed; returns its text.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    text(&out)
}

/// A call that must fail; returns the error.
pub fn fails(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, answered: {}", out.result);
    text(&out)
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// An asset folder of its own for each test.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shards_motion_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// The engine as a game has it, minus the window: asset server, content, core (states, the
/// loading gate, `Intent` as a replay input) and motion, on the asset folder `dir`. Loaded
/// and paused at tick 0.
pub fn engine_app(dir: &Path) -> App {
    let asset_dir = dir.to_string_lossy().into_owned();
    let config = EngineConfig {
        headless: true,
        port: None,
        paused: true,
        crash_replays: false,
        testing: true,
        asset_dir: asset_dir.clone(),
        ..default()
    };
    let mut app = App::new();
    let assets = AssetPlugin { file_path: asset_dir, watch_for_changes_override: Some(false), ..default() };
    app.add_plugins((MinimalPlugins, assets, StatesPlugin))
        .add_plugins(config.ai_core_plugin())
        .add_plugins(CorePlugin { config })
        .add_plugins(ContentPlugin)
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
    let started = Instant::now();
    while *app.world().resource::<State<EngineState>>().get() != EngineState::InGame || app.world().resource::<SimGate>().is_held() {
        assert!(started.elapsed() < Duration::from_secs(20), "still loading: {:?}", app.world().resource::<SimGate>().reasons());
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    app
}

/// `engine_app` on an empty asset folder.
pub fn bare_engine(name: &str) -> App {
    engine_app(&temp_dir(name))
}

pub fn intent(app: &App, e: Entity) -> Intent {
    app.world().get::<Intent>(e).unwrap().clone()
}

pub fn edit_intent(app: &mut App, e: Entity, f: impl FnOnce(&mut Intent)) {
    f(&mut app.world_mut().get_mut::<Intent>(e).unwrap());
}
