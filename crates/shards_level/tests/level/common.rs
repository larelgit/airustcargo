//! Test worlds: the engine core, content from `assets/` next to this file (through the asset
//! server, behind the loading gate), motion and levels; driven by tools, like an agent drives
//! a game. Plus the game components level files name, and the arena fixture.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ai_core::journal::JournalKind;
use ai_core::sim::StepEnd;
use ai_core::{call_tool, AiJournal, CallOutcome, SimGate};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde_json::{json, Value};
use shards_content::ContentPlugin;
use shards_core::{CorePlugin, EngineConfig, EngineState};
use shards_level::{spawn_level, FromLevel, LevelAsset, LevelPlugin};
use shards_motion::MotionPlugin;

use crate::gltf::{Gltf, NodeSpec};

// Level files name the components of a game. These two come with the engine, so the arena
// fixture works in the sandbox and the template too: `Health` has fields, `Team` is a newtype.
pub use shards_action::{Health, Team};

/// Test-only components for nested values: text, a number and a list.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Loot {
    pub table: String,
    pub rolls: u32,
    pub tags: Vec<String>,
}

/// A component with no fields: a flag.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Locked;

/// An enum inside a component.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct Door {
    pub state: DoorState,
}

#[derive(Reflect, Clone, Debug, PartialEq)]
pub enum DoorState {
    Closed,
    Open,
    Stuck { by: String },
}

/// The folder of the test content and the arena level file.
pub fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/level/assets")
}

/// A loaded world with the engine's content database from `assets()`, paused at tick 0.
pub fn app() -> App {
    app_in(&assets())
}

pub fn app_in(dir: &Path) -> App {
    app_with(dir, |_| {})
}

/// [`app_in`] with the game's own setup before the app finishes.
pub fn app_with(dir: &Path, setup: impl FnOnce(&mut App)) -> App {
    let dir = dir.to_string_lossy().into_owned();
    let config = EngineConfig {
        headless: true,
        testing: true,
        paused: true,
        port: None,
        crash_replays: false,
        // Tick timing is wall-clock: a journal compared between two runs must not hold it.
        report_perf: false,
        hot_reload: false,
        seed: 7,
        asset_dir: dir.clone(),
        ..default()
    };
    let mut app = App::new();
    let assets = AssetPlugin { file_path: dir, watch_for_changes_override: Some(false), ..default() };
    app.add_plugins((MinimalPlugins, assets, StatesPlugin));
    app.add_plugins(config.ai_core_plugin())
        .add_plugins(CorePlugin { config })
        .add_plugins(ContentPlugin)
        .add_plugins(MotionPlugin)
        .add_plugins(LevelPlugin)
        // Prefabs and level files name these by reflection; without Bevy's
        // `reflect_auto_register` (a build without render) they must be registered by hand.
        .register_type::<Name>()
        .register_type::<Transform>()
        .register_type::<ai_core::AiKind>()
        .register_type::<Health>()
        .register_type::<Team>()
        .register_type::<Loot>()
        .register_type::<Locked>()
        .register_type::<Door>()
        .register_type::<DoorState>();
    setup(&mut app);
    app.finish();
    app.cleanup();
    wait_loaded(&mut app);
    app
}

/// `g` as a level asset, read from its `.gltf` bytes.
pub fn read(g: &Gltf) -> LevelAsset {
    LevelAsset::from_slice("test.gltf", g.gltf().as_bytes()).unwrap()
}

/// Spawns `g` at the origin; the entities in file order, or the error.
pub fn spawn(app: &mut App, g: &Gltf) -> Result<Vec<Entity>, String> {
    spawn_at(app, g, Transform::default())
}

pub fn spawn_at(app: &mut App, g: &Gltf, at: Transform) -> Result<Vec<Entity>, String> {
    spawn_level(app.world_mut(), &read(g), at)
}

/// A level of one node with these properties.
pub fn one(props: Value) -> Gltf {
    let mut g = Gltf::new();
    let cube = g.cube();
    g.root(NodeSpec::new("Thing").mesh(cube).props(props));
    g
}

/// The entities of all spawned levels by their node's path in the file.
pub fn by_node(app: &mut App) -> std::collections::BTreeMap<String, Entity> {
    let world = app.world_mut();
    world.query::<(Entity, &FromLevel)>().iter(world).map(|(e, from)| (from.node.clone(), e)).collect()
}

/// Spawns the arena (the committed fixture) at the origin and indexes it by node path.
pub fn spawn_arena(app: &mut App) -> std::collections::BTreeMap<String, Entity> {
    let level = LevelAsset::read(&assets(), "arena.gltf").unwrap();
    spawn_level(app.world_mut(), &level, Transform::default()).unwrap();
    by_node(app)
}

/// Updates until the engine is in game and nothing holds the simulation.
pub fn wait_loaded(app: &mut App) {
    let started = Instant::now();
    while *app.world().resource::<State<EngineState>>().get() != EngineState::InGame || app.world().resource::<SimGate>().is_held() {
        assert!(started.elapsed() < Duration::from_secs(20), "still loading: {:?}", app.world().resource::<SimGate>().reasons());
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Updates until `done` holds.
pub fn update_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(20), "gave up waiting for: {what}");
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Runs `ticks` ticks; a gameplay panic fails the test.
pub fn step(app: &mut App, ticks: u64) {
    let report = ai_core::sim::step(app.world_mut(), ticks, Duration::from_secs(120), |_| None).unwrap();
    assert!(!matches!(report.end, StepEnd::SystemFailed(_)), "a system failed: {:?}", report.end);
}

/// Calls `tool` as agent `dev`.
pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

/// [`call`] that must succeed; returns its text.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    text(&out)
}

/// [`call`] that must fail; returns the error.
pub fn fails(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, got: {}", out.result);
    text(&out)
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// Journal lines (`tick text`) of every kind.
pub fn journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
}

pub fn errors(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Error).map(ToString::to_string).collect()
}

/// The arena the committed fixture `assets/arena.gltf` is generated from. What an artist
/// makes in Blender, exported with custom properties:
///
/// ```text
/// Floor            box, collider = auto
/// Pillars          empty
///   Pillar.001-4   boxes 1x4x1, collider = auto
/// Markers          empty, turned 90 degrees
///   Spawns         empty
///     PlayerStart  marker = spawn, name = start
/// Shrine           marker = landmark
/// Pond             volume = water          Vent  volume = updraft, accel, max_speed
/// GoalPad          marker = trigger, name = goal
/// Guard.001        prefab = skeleton, Health = {"current": 10}
/// Crate.003        collider = box, size, Health
/// Gate             marker = interactable, collider = box, size
/// BanditCamp       marker = camp           Edge.N/S/E  socket = road / river
/// LadderBottom     marker = navlink, to = LadderTop
/// ```
pub fn arena() -> Gltf {
    let mut g = Gltf::new();
    g.scene_props(json!({"theme": "ruins"}));
    let cube = g.cube();

    g.root(NodeSpec::new("Floor").at([0.0, -0.25, 0.0]).scale([24.0, 0.5, 24.0]).mesh(cube).props(json!({"collider": "auto"})));

    let pillars = g.root(NodeSpec::new("Pillars"));
    for (n, (x, z)) in [(6.0, 6.0), (-6.0, 6.0), (6.0, -6.0), (-6.0, -6.0)].into_iter().enumerate() {
        let name = format!("Pillar.00{}", n + 1);
        g.child(pillars, NodeSpec::new(&name).at([x, 2.0, z]).scale([1.0, 4.0, 1.0]).mesh(cube).props(json!({"collider": "auto"})));
    }

    // Three levels of hierarchy: Markers (turned 90 degrees) > Spawns > PlayerStart.
    let markers = g.root(NodeSpec::new("Markers").at([-8.0, 0.0, 8.0]).yaw(90.0));
    let spawns = g.child(markers, NodeSpec::new("Spawns").at([2.0, 0.0, 0.0]));
    g.child(spawns, NodeSpec::new("PlayerStart").at([0.0, 0.0, 1.0]).props(json!({"marker": "spawn", "name": "start"})));

    g.root(NodeSpec::new("Shrine").at([0.0, 0.0, -9.0]).props(json!({"marker": "landmark", "name": "Shrine"})));
    g.root(NodeSpec::new("Pond").at([9.0, -1.0, -3.0]).scale([5.0, 2.0, 5.0]).mesh(cube).props(json!({"volume": "water"})));
    g.root(
        NodeSpec::new("Vent")
            .at([-9.0, 3.0, -3.0])
            .scale([3.0, 6.0, 3.0])
            .mesh(cube)
            .props(json!({"volume": "updraft", "accel": 12, "max_speed": 8})),
    );
    g.root(NodeSpec::new("GoalPad").at([0.0, 1.0, -6.0]).scale([3.0, 2.0, 3.0]).mesh(cube).props(json!({"marker": "trigger", "name": "goal"})));
    g.root(NodeSpec::new("Guard.001").at([4.0, 0.0, -4.0]).props(json!({"prefab": "skeleton", "Health": {"current": 10}})));
    g.root(NodeSpec::new("Crate.003").at([-3.0, 0.5, -3.0]).mesh(cube).props(json!({
        "collider": "box",
        "size": [1.0, 1.0, 1.0],
        "Health": {"current": 5.0, "max": 5.0},
    })));
    g.root(NodeSpec::new("Gate").at([11.0, 1.0, 0.0]).props(json!({
        "marker": "interactable", "name": "gate", "collider": "box", "size": [0.5, 2.0, 3.0],
    })));
    g.root(NodeSpec::new("BanditCamp").at([-9.0, 0.0, -9.0]).props(json!({"marker": "camp", "name": "bandits"})));
    g.root(NodeSpec::new("Edge.N").at([0.0, 0.0, -12.0]).props(json!({"socket": "road"})));
    g.root(NodeSpec::new("Edge.S").at([0.0, 0.0, 12.0]).props(json!({"socket": "road"})));
    g.root(NodeSpec::new("Edge.E").at([12.0, 0.0, 0.0]).props(json!({"socket": "river"})));
    g.root(NodeSpec::new("LadderBottom").at([11.0, 0.0, 6.0]).props(json!({"marker": "navlink", "name": "ladder", "to": "LadderTop"})));
    g.root(NodeSpec::new("LadderTop").at([11.0, 3.0, 5.5]));
    // A mesh without properties: visual only.
    g.root(NodeSpec::new("Lamp").at([2.0, 3.0, 2.0]).scale([0.2, 0.2, 0.2]).mesh(cube));
    g
}

/// Content the arena needs: its level definition and the prefab it names.
pub const ARENA_LEVEL: &str = r#"{
    "arena": (
        file: "arena.gltf",
        required_markers: [
            (kind: "spawn", name: "start"),
            (kind: "landmark"),
            (kind: "trigger", name: "goal"),
            (kind: "socket", name: "road", min: 2),
        ],
        at: (0.0, 0.0, 0.0),
    ),
}"#;

pub const SKELETON_PREFAB: &str = r#"{
    "skeleton": (
        components: {
            "Name": "Skeleton",
            "AiKind": ("Skeleton"),
            "Health": (current: 40.0, max: 40.0),
            "Team": ("undead"),
        },
    ),
}"#;

/// Writes `files` (path → bytes) into a fresh asset folder under the system temp dir.
pub fn temp_assets(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shards_level_{}_{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, bytes) in files {
        let at = dir.join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, bytes).unwrap();
    }
    dir
}

/// A temp asset folder with the arena content and `arena.gltf` from `level`.
pub fn arena_assets(name: &str, level: &Gltf) -> PathBuf {
    temp_assets(
        name,
        &[
            ("arena.gltf", level.gltf().as_bytes()),
            ("content/levels/arena.level.ron", ARENA_LEVEL.as_bytes()),
            ("content/props/skeleton.prefab.ron", SKELETON_PREFAB.as_bytes()),
        ],
    )
}
