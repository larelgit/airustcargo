//! Headless fight arenas: the engine core, content from `assets/` next to this file
//! (through the asset server, behind the loading gate), motion and actions; a floor,
//! and fighters spawned by the tests. Driven tick by tick, through tools and
//! `Intent`, like an agent or a keyboard drives a game.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use ai_core::journal::JournalKind;
use ai_core::sim::StepEnd;
use ai_core::{call_tool, AiJournal, AiKind, AiPlayer, CallOutcome, SimGate};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde_json::Value;
use shards_action::prelude::*;
use shards_action::Charges;
use shards_content::ContentPlugin;
use shards_core::{CorePlugin, EngineConfig, EngineState, Intent};
use shards_motion::{character, Collider, Shape};

pub const DT: f32 = 1.0 / 60.0;

/// North (-Z), south, east.
pub const N: Vec3 = Vec3::NEG_Z;
pub const S: Vec3 = Vec3::Z;
pub const E: Vec3 = Vec3::X;

/// A loaded arena with a floor, paused at tick 0.
pub fn app() -> App {
    app_with(|_| {})
}

/// [`app`] with the game's own setup (hooks, systems) before it finishes.
pub fn app_with(setup: impl FnOnce(&mut App)) -> App {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/combat/assets").to_owned();
    let config = EngineConfig {
        headless: true,
        testing: true,
        paused: true,
        port: None,
        crash_replays: false,
        hot_reload: false,
        seed: 7,
        asset_dir: dir.clone(),
        ..default()
    };
    let mut app = App::new();
    let assets = AssetPlugin { file_path: dir, watch_for_changes_override: Some(false), ..default() };
    app.add_plugins((MinimalPlugins, assets, StatesPlugin))
        .add_plugins(config.ai_core_plugin())
        .add_plugins(CorePlugin { config })
        .add_plugins(ContentPlugin)
        .add_plugins(ActionPlugin);
    setup(&mut app);
    app.finish();
    app.cleanup();
    let started = Instant::now();
    while *app.world().resource::<State<EngineState>>().get() != EngineState::InGame
        || app.world().resource::<SimGate>().is_held()
    {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "still loading: {:?}",
            app.world().resource::<SimGate>().reasons()
        );
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    let errors: Vec<String> =
        app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "test content has errors: {errors:#?}");
    floor(app.world_mut());
    app
}

pub fn floor(world: &mut World) -> Entity {
    world.spawn((Collider::new(Shape::cuboid(Vec3::new(60.0, 0.5, 60.0))), Transform::from_xyz(0.0, -0.5, 0.0))).id()
}

/// A wall: a static box between two corners.
pub fn wall(world: &mut World, min: Vec3, max: Vec3) -> Entity {
    let half = (max - min) * 0.5;
    world.spawn((Collider::new(Shape::cuboid(half)), Transform::from_translation(min + half))).id()
}

/// A fighter of `kind` with `moveset` on `team`, standing at `at` facing `facing`:
/// 100 hp, 100 stamina (30/s after 0.8 s), 30 poise.
pub fn fighter(world: &mut World, kind: &str, moveset: &str, team: &str, at: Vec3, facing: Vec3) -> Entity {
    // No `locomotion` id: the controller's code defaults.
    let (controller, intent, tf) = character("", at);
    world
        .spawn((
            Name::new(kind.to_owned()),
            AiKind::new(kind.to_owned()),
            Fighter::new(moveset),
            Health::new(100.0),
            Stamina::new(100.0, 30.0, 0.8),
            Poise::new(30.0, 10.0, 2.0),
            Team::new(team),
            controller,
            intent,
            tf.looking_to(facing, Vec3::Y),
        ))
        .id()
}

/// The player: tools act as it. Two flasks.
pub fn hero(world: &mut World, moveset: &str, at: Vec3, facing: Vec3) -> Entity {
    let e = fighter(world, "Hero", moveset, "heroes", at, facing);
    world.entity_mut(e).insert((AiPlayer, Charges::new([("flask", 2)])));
    e
}

pub fn brute(world: &mut World, at: Vec3, facing: Vec3) -> Entity {
    fighter(world, "Brute", "brute", "monsters", at, facing)
}

/// Runs `ticks` ticks; a gameplay panic fails the test.
pub fn step(app: &mut App, ticks: u64) {
    let report = ai_core::sim::step(app.world_mut(), ticks, Duration::from_secs(120), |_| None).unwrap();
    assert!(!matches!(report.end, StepEnd::SystemFailed(_)), "a system failed: {:?}", report.end);
}

/// A keyboard press between ticks.
pub fn press(app: &mut App, e: Entity, input: &str) {
    app.world_mut().get_mut::<Intent>(e).unwrap().press(input);
}

pub fn hold(app: &mut App, e: Entity, input: &str, on: bool) {
    app.world_mut().get_mut::<Intent>(e).unwrap().set_held(input, on);
}

/// Calls `tool` as agent `dev` (dev scope, the hero's body).
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

/// Gameplay journal lines (`tick text`).
pub fn journal(app: &App) -> Vec<String> {
    app.world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == JournalKind::Event)
        .map(ToString::to_string)
        .collect()
}

/// Journal lines containing `needle`.
pub fn lines(app: &App, needle: &str) -> Vec<String> {
    journal(app).into_iter().filter(|l| l.contains(needle)).collect()
}

/// `e5` of an entity.
pub fn id(app: &App, e: Entity) -> String {
    app.world().get::<ai_core::AiId>(e).unwrap().to_string()
}

/// The running action's id, or `idle`.
pub fn action(app: &App, e: Entity) -> String {
    app.world().get::<ActionState>(e).unwrap().current.as_ref().map_or_else(|| "idle".to_owned(), |r| r.action.clone())
}

pub fn hp(app: &App, e: Entity) -> f32 {
    app.world().get::<Health>(e).unwrap().current
}

pub fn stamina(app: &App, e: Entity) -> f32 {
    app.world().get::<Stamina>(e).unwrap().current
}

pub fn feet(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Transform>(e).unwrap().translation
}
