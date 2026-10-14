//! Headless arenas for NPCs: the engine core, content from `assets/` next to this file
//! (through the asset server, behind the loading gate), motion, actions and the NPC module;
//! a floor, and fighters spawned by the tests. Driven tick by tick, like a game runs.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use ai_core::journal::JournalKind;
use ai_core::sim::StepEnd;
use ai_core::{call_tool, AiId, AiJournal, AiKind, AiPlayer, CallOutcome, SimGate};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde_json::Value;
use shards_action::prelude::*;
use shards_action::{ActionDef, Invulnerable};
use shards_content::{ContentDb, ContentPlugin};
use shards_core::{CorePlugin, EngineConfig, EngineState, PlayerId};
use shards_motion::{character, Collider, Shape};
use shards_npc::prelude::*;
use shards_npc::Home;

pub const DT: f32 = 1.0 / 60.0;

/// North (-Z), south, east, west.
pub const N: Vec3 = Vec3::NEG_Z;
pub const S: Vec3 = Vec3::Z;
pub const E: Vec3 = Vec3::X;
pub const W: Vec3 = Vec3::NEG_X;

/// A loaded arena with a floor, paused at tick 0.
pub fn app() -> App {
    app_with(|_| {})
}

/// [`app`] with the game's own setup (hooks, systems) before it finishes.
pub fn app_with(setup: impl FnOnce(&mut App)) -> App {
    app_seeded(7, setup)
}

pub fn app_seeded(seed: u64, setup: impl FnOnce(&mut App)) -> App {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/npc/assets").to_owned();
    let config = EngineConfig {
        headless: true,
        testing: true,
        paused: true,
        port: None,
        crash_replays: false,
        hot_reload: false,
        seed,
        asset_dir: dir.clone(),
        ..default()
    };
    let mut app = App::new();
    let assets = AssetPlugin { file_path: dir, watch_for_changes_override: Some(false), ..default() };
    app.add_plugins((MinimalPlugins, assets, StatesPlugin));
    app.add_plugins(config.ai_core_plugin())
        .add_plugins(CorePlugin { config })
        .add_plugins(ContentPlugin)
        .add_plugins(NpcPlugin);
    setup(&mut app);
    app.finish();
    app.cleanup();
    let started = Instant::now();
    while *app.world().resource::<State<EngineState>>().get() != EngineState::InGame
        || app.world().resource::<SimGate>().is_held()
    {
        assert!(started.elapsed() < Duration::from_secs(20), "still loading: {:?}", app.world().resource::<SimGate>().reasons());
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
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

/// A fighter of `kind` with `moveset` on `team`, standing at `at` facing `facing`.
pub fn fighter(world: &mut World, kind: &str, moveset: &str, team: &str, at: Vec3, facing: Vec3) -> Entity {
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

/// A player (`PlayerId(n)`), in god mode so a fight lasts: hits land, nothing happens.
pub fn player(world: &mut World, n: u8, at: Vec3) -> Entity {
    let e = fighter(world, "Hero", "hero", "heroes", at, N);
    world.entity_mut(e).insert((PlayerId(n), Invulnerable));
    if n == 0 {
        world.entity_mut(e).insert(AiPlayer);
    }
    e
}

/// An NPC fighter of the monsters.
pub fn npc(world: &mut World, moveset: &str, npc: Npc, at: Vec3, facing: Vec3) -> Entity {
    let e = fighter(world, "Grunt", moveset, "monsters", at, facing);
    world.entity_mut(e).insert(npc);
    e
}

/// A grunt fighting by `director` with plan `plan`.
pub fn grunt(world: &mut World, behavior: &str, plan: &str, director: &str, at: Vec3) -> Entity {
    let toward = (-at).normalize_or(N);
    npc(world, "grunt", Npc::new(behavior, "grunt").with_plan(plan).with_director(director), at, toward)
}

pub fn home(world: &mut World, e: Entity, at: Vec3, leash: f32) {
    world.entity_mut(e).insert(Home::at(at, leash));
}

/// Runs `ticks` ticks; a gameplay panic fails the test.
pub fn step(app: &mut App, ticks: u64) {
    let report = ai_core::sim::step(app.world_mut(), ticks, Duration::from_secs(300), |_| None).unwrap();
    assert!(!matches!(report.end, StepEnd::SystemFailed(_)), "a system failed: {:?}", report.end);
}

/// Calls `tool` as agent `dev` (dev scope, the player's body).
pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

pub fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

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

/// Every journal line, errors included.
pub fn all_journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
}

/// Journal lines containing `needle`.
pub fn lines(app: &App, needle: &str) -> Vec<String> {
    journal(app).into_iter().filter(|l| l.contains(needle)).collect()
}

/// `e5` of an entity.
pub fn id(app: &App, e: Entity) -> String {
    aid(app, e).to_string()
}

pub fn aid(app: &App, e: Entity) -> AiId {
    *app.world().get::<AiId>(e).unwrap()
}

pub fn feet(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Transform>(e).unwrap().translation
}

pub fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

/// Puts a body somewhere between ticks (the controller starts afresh there).
pub fn teleport(app: &mut App, e: Entity, at: Vec3) {
    app.world_mut().get_mut::<Transform>(e).unwrap().translation = at;
}

pub fn state(app: &App, e: Entity) -> String {
    app.world().get::<NpcBrain>(e).unwrap().state.clone()
}

/// The running action's id, if any.
pub fn action(app: &App, e: Entity) -> Option<String> {
    app.world().get::<ActionState>(e).unwrap().current.as_ref().map(|r| r.action.clone())
}

/// Whether `e` is in an action that hits: it is attacking.
pub fn attacking(app: &App, e: Entity) -> bool {
    let db = app.world().resource::<ContentDb>();
    action(app, e).and_then(|a| db.get::<ActionDef>(&a).map(|d| !d.hits.is_empty())).unwrap_or(false)
}

/// The tags of action `id`.
pub fn tags(app: &App, action: &str) -> Vec<String> {
    app.world().resource::<ContentDb>().get::<ActionDef>(action).map(|d| d.tags.clone()).unwrap_or_default()
}

pub fn has_token(app: &App, e: Entity) -> bool {
    app.world().get::<AttackToken>(e).unwrap().held.is_some()
}

/// Actions started, by whom, at which tick: recorded by an observer.
#[derive(Resource, Default, Debug)]
pub struct Started(pub Vec<(u64, Entity, String)>);

/// Records every [`shards_action::ActionStarted`] into [`Started`].
pub fn record_starts(app: &mut App) {
    app.init_resource::<Started>().add_observer(
        |e: On<shards_action::ActionStarted>, tick: Res<ai_core::SimTick>, mut started: ResMut<Started>| {
            started.0.push((tick.0, e.entity, e.action.clone()));
        },
    );
}
