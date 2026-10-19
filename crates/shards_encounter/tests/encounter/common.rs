//! A test game on the whole engine (`shards::testing`), with the content next to this file: a
//! flat floor, neutral units, the encounters of `waves.encounter.ron`. Players, volumes and
//! anchors are put in by the tests, which drive it tick by tick through the engine's tools.

#![allow(dead_code)]

use std::path::Path;

use ai_core::{AiId, AiIndex};
use serde_json::{json, Value};
use shards::content::db::{read_dir, Source};
use shards::content::{ContentDb, ContentKinds};
use shards::encounter::{EncounterRuns, Run};
use shards::motion::{Collider, CollisionLayers, Shape, Volume, VolumeKind};
use shards::prelude::*;
use shards::testing;

pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/encounter/assets");
pub const BAD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/encounter/assets_bad");

pub const DT: f32 = 1.0 / 60.0;

pub fn config(seed: u64) -> EngineConfig {
    EngineConfig { seed, asset_dir: ASSETS.into(), ..testing::config() }
}

/// The test game: a floor, nothing else. Tests add what they need.
pub fn game(app: &mut App) {
    app.add_systems(OnEnter(EngineState::InGame), floor);
}

fn floor(mut commands: Commands) {
    commands.spawn((Collider::new(Shape::cuboid(Vec3::new(80.0, 0.5, 80.0))), Transform::from_xyz(0.0, -0.5, 0.0)));
}

pub fn app(seed: u64) -> App {
    app_with(seed, |_| {})
}

pub fn app_with(seed: u64, setup: impl FnOnce(&mut App)) -> App {
    let app = testing::test_app(
        |c| *c = config(seed),
        |app| {
            game(app);
            setup(app);
        },
    );
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "test content has errors: {errors:#?}");
    app
}

/// The same game built but not loaded: for playing a replay on it.
pub fn fresh(seed: u64) -> App {
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config(seed)));
    game(&mut app);
    app.finish();
    app.cleanup();
    app
}

pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::ok(app, tool, args);
    match out.result {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

pub fn err(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail: {}", out.result);
    out.result.as_str().unwrap_or_default().to_owned()
}

/// Runs `n` ticks.
pub fn step(app: &mut App, n: u64) {
    ok(app, "step", json!({"ticks": n}));
}

/// Runs one tick.
pub fn tick(app: &mut App) {
    step(app, 1);
}

pub fn now(app: &App) -> u64 {
    ai_core::sim::tick(app.world())
}

/// Ticks until `done` holds (checked after each tick); the number of ticks run. Panics after
/// `limit`.
pub fn run_until(app: &mut App, limit: u64, mut done: impl FnMut(&App) -> bool) -> u64 {
    for n in 1..=limit {
        tick(app);
        if done(app) {
            return n;
        }
    }
    panic!("not reached in {limit} ticks");
}

// --- the world's things -----------------------------------------------------------------

/// A player body `n` at `at`, facing north (-Z). Player 0 is the AI's player.
pub fn player(app: &mut App, n: u8, at: Vec3) -> Entity {
    let (controller, intent, tf) = shards::motion::character("", at);
    let mut e = app.world_mut().spawn((
        Name::new(format!("Player {n}")),
        AiKind::new("Player"),
        controller,
        intent,
        tf.looking_to(Vec3::NEG_Z, Vec3::Y),
        PlayerId(n),
        Health::new(100.0),
        Team::new("heroes"),
    ));
    if n == 0 {
        e.insert(AiPlayer);
    }
    e.id()
}

pub fn move_to(app: &mut App, entity: Entity, at: Vec3) {
    app.world_mut().get_mut::<Transform>(entity).expect("a transform").translation = at;
}

/// A trigger volume `name`: a box of `half` extents centred at `at`.
pub fn trigger(app: &mut App, name: &str, at: Vec3, half: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Collider::new(Shape::cuboid(half)).with_layers(CollisionLayers::VOLUME),
            Volume { kind: VolumeKind::Trigger { name: name.into() } },
            Transform::from_translation(at),
        ))
        .id()
}

/// An anchor of `encounter` at `at` with the key `key`; returns the run's key (`world/<key>`).
pub fn place(app: &mut App, encounter: &str, key: &str, at: Vec3) -> String {
    shards::encounter::place_encounter(app.world_mut(), encounter, Some(key), Transform::from_translation(at)).expect("a placed anchor");
    format!("world/{key}")
}

/// Puts `Dead` on `entity`, the way the engine's death step does.
pub fn kill(app: &mut App, entity: Entity) {
    app.world_mut().entity_mut(entity).insert(shards::action::Dead);
}

pub fn revive(app: &mut App, entity: Entity) {
    app.world_mut().entity_mut(entity).remove::<shards::action::Dead>();
}

pub fn runs(app: &App) -> Vec<Run> {
    app.world().get_resource::<EncounterRuns>().map(|r| r.runs.clone()).unwrap_or_default()
}

pub fn run(app: &App, key: &str) -> Run {
    shards::encounter::run(app.world(), key).unwrap_or_else(|| panic!("no run {key}: {:?}", runs(app).iter().map(|r| &r.key).collect::<Vec<_>>())).clone()
}

pub fn state(app: &App, key: &str) -> shards::encounter::RunState {
    run(app, key).state
}

/// The entities of run `key`'s members, in `AiId` order.
pub fn members(app: &App, key: &str) -> Vec<(AiId, Entity)> {
    let index = app.world().resource::<AiIndex>();
    run(app, key).members.iter().filter_map(|m| index.get(m.id).map(|e| (m.id, e))).collect()
}

/// The living members of a wave of a run.
pub fn wave_members(app: &App, key: &str, wave: u32) -> Vec<Entity> {
    let index = app.world().resource::<AiIndex>();
    run(app, key)
        .members
        .iter()
        .filter(|m| m.wave == wave && m.counted)
        .filter_map(|m| index.get(m.id))
        .filter(|e| app.world().get::<shards::action::Dead>(*e).is_none())
        .collect()
}

/// Every journal line.
pub fn journal(app: &App) -> Vec<String> {
    app.world().resource::<ai_core::AiJournal>().after(0).0.map(|e| e.text.clone()).collect()
}

pub fn journal_has(app: &App, text: &str) -> bool {
    journal(app).iter().any(|l| l.contains(text))
}

pub fn count(app: &App, text: &str) -> usize {
    journal(app).iter().filter(|l| l.contains(text)).count()
}

/// The content on disk plus the files of `dir`'s `content/` folder, built with the app's kinds.
pub fn db_with_dir(app: &App, dir: &str) -> ContentDb {
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources: Vec<Source> = read_dir(Path::new(dir)).expect("test content");
    // The shared prefab names live in each folder's own prefab file.
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    ContentDb::build(&kinds, &sources)
}

/// The errors of `db`, as lines.
pub fn errors_of(db: &ContentDb) -> Vec<String> {
    db.errors().map(ToString::to_string).collect()
}

/// Content: the test folder with `files` (path under `content/` → text) added or replaced.
pub fn db_edited(app: &App, files: &[(&str, &str)]) -> ContentDb {
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources: Vec<Source> = read_dir(Path::new(ASSETS)).expect("test content");
    for (path, text) in files {
        let wanted = format!("content/{path}");
        match sources.iter_mut().find(|s| s.path == wanted) {
            Some(source) => source.text = (*text).to_owned(),
            None => sources.push(Source { path: wanted, text: (*text).to_owned() }),
        }
    }
    ContentDb::build(&kinds, &sources)
}

// --- watching events ---------------------------------------------------------------------

use shards::encounter::{
    EncounterCompleted, EncounterReset, EncounterResumed, EncounterStarted, EncounterWiped, WaveCleared, WaveStarted, WaveWarned,
};

/// The events a test saw, as short lines (`started 1`, `wiped 1 rewound`).
#[derive(Resource, Default)]
pub struct Seen(pub Vec<String>);

/// A game's own way for a participant to be out besides dead or down (the engine's `downed`
/// rule counts a downed body out by itself).
#[derive(Component, Reflect, Default, Clone, Copy)]
#[reflect(Component, Default)]
pub struct Benched;

/// A test app with the events recorded in [`Seen`], the `Benched` out rule registered, and the
/// hits counted.
pub fn watched(seed: u64) -> App {
    app_with(seed, watch)
}

pub fn watch(app: &mut App) {
    use shards::encounter::EncounterAppExt;
    app.init_resource::<Seen>()
        .register_type::<Benched>()
        .add_encounter_out_rule("benched", |world, entity| world.get::<Benched>(entity).is_some())
        .add_observer(|e: On<EncounterStarted>, mut s: ResMut<Seen>| s.0.push(format!("encounter-started {}/{}", e.participants, e.of)))
        .add_observer(|e: On<WaveWarned>, mut s: ResMut<Seen>| s.0.push(format!("warned {} cue={}", e.wave, e.cue.is_some())))
        .add_observer(|e: On<WaveStarted>, mut s: ResMut<Seen>| s.0.push(format!("started {} members={}", e.wave, e.members.len())))
        .add_observer(|e: On<WaveCleared>, mut s: ResMut<Seen>| s.0.push(format!("cleared {}", e.wave)))
        .add_observer(|_: On<EncounterCompleted>, mut s: ResMut<Seen>| s.0.push("completed".into()))
        .add_observer(|e: On<EncounterWiped>, mut s: ResMut<Seen>| {
            s.0.push(format!("wiped {} rewound={} between={}", e.wave, e.rewound, e.between))
        })
        .add_observer(|e: On<EncounterResumed>, mut s: ResMut<Seen>| s.0.push(format!("resumed {}", e.wave)))
        .add_observer(|e: On<EncounterReset>, mut s: ResMut<Seen>| s.0.push(format!("reset {:?}", e.why)))
        .add_observer(|_: On<shards::action::Died>, mut s: ResMut<Seen>| s.0.push("died".into()))
        .add_observer(|_: On<shards::action::Healed>, mut s: ResMut<Seen>| s.0.push("healed".into()));
}

pub fn seen(app: &App) -> Vec<String> {
    app.world().resource::<Seen>().0.clone()
}

pub fn saw(app: &App, prefix: &str) -> usize {
    seen(app).iter().filter(|s| s.starts_with(prefix)).count()
}

/// Remembers `key` as a won encounter, as a win or a loaded save would.
pub fn remember(app: &mut App, key: &str) {
    let mut runs = app.world_mut().get_resource_or_init::<EncounterRuns>();
    runs.completed.push(key.to_owned());
    runs.completed.sort();
}
