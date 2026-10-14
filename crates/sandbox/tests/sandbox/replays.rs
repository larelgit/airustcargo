//! Replays end to end: record a session, play it back in a fresh world, and get the
//! same game — or a report of the first tick where it stops being the same.

use sandbox::gameplay::Player;
use sandbox::SandboxPlugin;
use shards::ai_core::replay::{self, Replay, ReplayEvent};
use shards::ai_core::{call_tool, AiJournal, AiReplay, JournalKind};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{ok, state, BOSS};

/// What the keyboard does: straight into the player's `Intent`, between ticks.
fn press(app: &mut App, dir: Vec3) {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&mut Intent, With<Player>>();
    let mut intent = q.single_mut(world).unwrap();
    intent.orders.clear();
    intent.move_dir = dir;
}

/// Gameplay events only: agent, sim and log lines differ between a recording and
/// its playback (the playback reports itself).
fn story(app: &App) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == JournalKind::Event).map(ToString::to_string).collect()
}

/// A session with every kind of input a replay has to capture: dev edits, player
/// tools, keyboard, a spawn, possession, and a call after the last tick.
fn session(app: &mut App) {
    ok(app, "set", json!({"id": BOSS, "component": "Health", "path": "current", "value": 310}));
    ok(app, "attack", json!({"target": BOSS, "repeat": true}));
    ok(app, "step", json!({"ticks": 100}));
    press(app, Vec3::X);
    ok(app, "step", json!({"ticks": 20}));
    press(app, Vec3::ZERO);
    ok(app, "spawn_prefab", json!({"id": "skeleton", "x": 4, "z": 6}));
    ok(app, "possess", json!({"id": "e7"}));
    ok(app, "set", json!({"id": "e7", "component": "Health", "path": "current", "value": 5}));
    ok(app, "heal", json!({})); // heals "you": e7
    ok(app, "possess", json!({}));
    ok(app, "dodge", json!({"away_from": BOSS}));
    ok(app, "step", json!({"ticks": 200}));
    ok(app, "teleport", json!({"id": "e15", "x": -3, "z": 0}));
    ok(app, "step", json!({"ticks": 37}));
    ok(app, "move", json!({"dir": "W", "meters": 2}));
}

fn recorded(seed: u64) -> (App, Replay) {
    let mut app = sandbox::test_app(seed);
    session(&mut app);
    let replay = replay::save(app.world());
    // Through JSON, like a file.
    let replay = shards::serde_json::from_str(&shards::serde_json::to_string(&replay).unwrap()).unwrap();
    (app, replay)
}

fn report(app: &App) -> shards::ai_core::ReplayReport {
    app.world().resource::<AiReplay>().report().cloned().expect("playback has not finished")
}

#[test]
fn playback_reproduces_the_session() {
    let (mut original, replay) = recorded(42);
    assert_eq!(replay.end.tick, 357);
    assert!(
        replay.events.iter().any(|e| matches!(e, ReplayEvent::Input { at: 100, .. })),
        "keyboard input not recorded"
    );
    assert!(
        !replay
            .events
            .iter()
            .any(|e| matches!(e, ReplayEvent::Call { tool, .. } if tool == "step" || tool == "possess")),
        "control tools must not be recorded"
    );

    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), replay.clone()).unwrap();
    ok(&mut copy, "step", json!({"ticks": 50}));
    // Keys pressed while a replay plays are ignored.
    press(&mut copy, Vec3::NEG_X);
    ok(&mut copy, "step", json!({"ticks": 307}));
    copy.update(); // the trailing `move` call happens between ticks, like in the original

    let report = report(&copy);
    assert!(report.identical(), "{report}");
    assert_eq!(report.matched as usize, replay.checks.len() + 1);
    assert_eq!(state(&mut original), state(&mut copy));
    assert_eq!(story(&original), story(&copy));
    // The skeleton spawned mid-session exists in the copy too, and fights.
    assert!(story(&copy).iter().any(|l| l.contains("e15 -> e5 claw")), "{:#?}", story(&copy));
}

#[test]
fn a_changed_game_reports_the_first_diverging_tick() {
    let (_, mut replay) = recorded(42);
    // As if the boss had spawned with less health in a newer build.
    let ReplayEvent::Call { args, .. } = &mut replay.events[0] else { panic!("first event is the set call") };
    args["value"] = json!(299);

    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), replay).unwrap();
    ok(&mut copy, "step", json!({"ticks": 357}));
    copy.update();

    let report = report(&copy);
    assert_eq!(report.diverged_at, Some(60), "{report}");
    assert!(report.to_string().contains("DIVERGED at tick 60"), "{report}");
    let errors: Vec<String> = copy
        .world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == JournalKind::Error)
        .map(ToString::to_string)
        .collect();
    assert!(errors.iter().any(|e| e.contains("replay diverged at tick 60")), "{errors:?}");
}

#[test]
fn an_action_during_playback_takes_over() {
    let (_, replay) = recorded(42);
    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), replay).unwrap();
    ok(&mut copy, "step", json!({"ticks": 130}));
    ok(&mut copy, "teleport", json!({"id": "e5", "x": 0, "z": 0}));

    let report = report(&copy);
    assert_eq!(report.interrupted.as_deref(), Some("dev took over (teleport)"), "{report}");
    assert_eq!(report.tick, 130);
    assert!(!copy.world().resource::<AiReplay>().is_playing());
    // From here on the game is live.
    ok(&mut copy, "step", json!({"ticks": 10}));
    assert_eq!(shards::ai_core::sim::tick(copy.world()), 140);
}

#[test]
fn a_branch_saves_as_one_replay() {
    let (_, replay) = recorded(42);
    let mut branch = sandbox::test_app(42);
    replay::play(branch.world_mut(), replay).unwrap();
    ok(&mut branch, "step", json!({"ticks": 357}));
    branch.update();
    assert!(report(&branch).identical());
    // Live again: play on differently.
    ok(&mut branch, "attack", json!({"target": "e8", "repeat": true}));
    ok(&mut branch, "step", json!({"ticks": 90}));
    let whole = replay::save(branch.world());
    assert_eq!(whole.end.tick, 447);

    let mut again = sandbox::test_app(42);
    replay::play(again.world_mut(), whole).unwrap();
    ok(&mut again, "step", json!({"ticks": 447}));
    again.update();
    assert!(report(&again).identical(), "{}", report(&again));
    assert_eq!(state(&mut branch), state(&mut again));
}

#[test]
fn calls_made_inside_a_tool_are_recorded_once() {
    /// A game tool built from other tools: spawns a pair of skeletons.
    fn pair(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
        for x in [-2, 2] {
            let out = call_tool(world, &call.caller.agent, "spawn_prefab", json!({"id": "skeleton", "x": x, "z": 8}));
            if !out.ok {
                return Err(out.result.to_string());
            }
        }
        Ok("two skeletons".into())
    }
    let app_with_pair = || {
        let mut app = sandbox::test_app(42);
        app.add_ai_tool("pair", Scope::Dev, pair);
        app
    };

    let mut original = app_with_pair();
    ok(&mut original, "pair", json!({}));
    ok(&mut original, "step", json!({"ticks": 90}));
    let replay = replay::save(original.world());
    let tools: Vec<&str> = replay
        .events
        .iter()
        .filter_map(|e| match e {
            ReplayEvent::Call { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tools, ["pair"]);

    let mut copy = app_with_pair();
    replay::play(copy.world_mut(), replay).unwrap();
    ok(&mut copy, "step", json!({"ticks": 90}));
    copy.update();
    assert!(report(&copy).identical(), "{}", report(&copy));
    assert_eq!(state(&mut original), state(&mut copy), "the inner spawns ran twice or not at all");
}

#[test]
fn playback_needs_a_fresh_world_with_the_same_seed() {
    let (_, replay) = recorded(42);

    let mut other_seed = sandbox::test_app(7);
    let err = replay::play(other_seed.world_mut(), replay.clone()).unwrap_err();
    assert!(err.contains("recorded with seed 42; this world has seed 7"), "{err}");

    let mut started = sandbox::test_app(42);
    ok(&mut started, "step", json!({"ticks": 10}));
    let err = replay::play(started.world_mut(), replay.clone()).unwrap_err();
    assert!(err.contains("fresh world at tick 0; this one is at tick 10"), "{err}");

    let mut edited = sandbox::test_app(42);
    ok(&mut edited, "heal", json!({}));
    let err = replay::play(edited.world_mut(), replay).unwrap_err();
    assert!(err.contains("already recorded 1 events"), "{err}");
}

#[test]
fn replay_save_writes_a_file_that_loads_back() {
    let (mut app, replay) = recorded(42);
    let name = format!("replays/test/save-{}.json", std::process::id());
    let out = ok(&mut app, "replay_save", json!({"path": name}));
    let text = out.result.as_str().unwrap();
    assert!(text.contains("357 ticks") && text.contains("seed 42"), "{text}");
    let path = std::env::current_dir().unwrap().join(&name);
    let loaded = replay::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(loaded, replay);
}

#[test]
fn replay_save_writes_only_inside_replays() {
    let mut app = sandbox::test_app(42);
    let outside = std::env::temp_dir().join(format!("sandbox_replay_{}.json", std::process::id()));
    for path in [outside.display().to_string(), "replays/../escaped.json".into(), "boss.json".into()] {
        let out = call_tool(app.world_mut(), "dev", "replay_save", json!({"path": path}));
        assert!(!out.ok, "{path} must be refused");
        let err = out.result.as_str().unwrap();
        assert!(err.starts_with("path must be inside replays/, e.g. replays/boss-fight.json"), "{err}");
    }
    assert!(
        !outside.exists()
            && !std::path::Path::new("escaped.json").exists()
            && !std::path::Path::new("boss.json").exists()
    );
}

#[test]
fn checks_hash_the_game_not_the_text_the_ai_reads() {
    let (_, replay) = recorded(42);
    // A newer build that describes the same game differently.
    let mut copy = sandbox::test_app(42);
    copy.register_ai_describer("mood", |e| e.contains::<Health>().then(|| "grim".to_owned()));
    let seen = ok(&mut copy, "observe", json!({}));
    assert!(seen.result.as_str().unwrap().contains("grim"), "the extra describer is in use");
    replay::play(copy.world_mut(), replay).unwrap();
    ok(&mut copy, "step", json!({"ticks": 357}));
    copy.update();
    assert!(report(&copy).identical(), "{}", report(&copy));
}

#[test]
fn a_version_1_replay_is_refused_with_the_reason() {
    let (_, mut replay) = recorded(42);
    replay.format = "shards-replay/1".into();
    let mut copy = sandbox::test_app(42);
    let err = replay::play(copy.world_mut(), replay).unwrap_err();
    assert!(err.contains("recorded by an older build") && err.contains("record it again"), "{err}");
}

/// Cosmetic state: may differ between a recording and its playback.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Sparkle(f32);

/// Game state outside AI-visible entities.
#[derive(Resource, Default)]
struct Weather(u64);

/// The game with a sparkle on the player and a weather check; what the checks cover is
/// part of the build, the same for the recording and its playback.
fn sparkling_app(ignore_sparkle: bool) -> App {
    let mut app = sandbox::test_app(42);
    app.register_type::<Sparkle>()
        .init_resource::<Weather>()
        .add_replay_check("weather", |w| w.resource::<Weather>().0);
    if ignore_sparkle {
        app.replay_ignore::<Sparkle>();
    }
    let player = app.world().resource::<shards::ai_core::AiIndex>().get(AiId(5)).unwrap();
    app.world_mut().entity_mut(player).insert(Sparkle(0.0));
    app
}

/// Records the session, then plays it back changing the sparkle and/or the weather
/// directly in the world after 100 ticks.
fn meddle(ignore_sparkle: bool, sparkle: bool, weather: bool) -> shards::ai_core::ReplayReport {
    let mut original = sparkling_app(ignore_sparkle);
    session(&mut original);
    let replay = replay::save(original.world());

    let mut app = sparkling_app(ignore_sparkle);
    replay::play(app.world_mut(), replay).unwrap();
    ok(&mut app, "step", json!({"ticks": 100}));
    if sparkle {
        let player = app.world().resource::<shards::ai_core::AiIndex>().get(AiId(5)).unwrap();
        app.world_mut().get_mut::<Sparkle>(player).unwrap().0 = 1.0;
    }
    if weather {
        app.world_mut().resource_mut::<Weather>().0 = 7;
    }
    ok(&mut app, "step", json!({"ticks": 257}));
    app.update();
    report(&app)
}

#[test]
fn games_choose_what_the_checks_cover() {
    assert!(meddle(true, false, false).identical());
    let ignored = meddle(true, true, false);
    assert!(ignored.identical(), "replay_ignore::<Sparkle>: {ignored}");
    let counted = meddle(false, true, false);
    assert_eq!(counted.diverged_at, Some(120), "without replay_ignore the sparkle counts: {counted}");
    let weather = meddle(true, false, true);
    assert_eq!(weather.diverged_at, Some(120), "add_replay_check covers the resource: {weather}");
}

/// A gameplay component that points at another entity.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct Grudge(Entity);

#[test]
fn a_window_does_not_change_the_hash() {
    let mut app = sandbox::test_app(42);
    let before = replay::state_hash(app.world());
    // What the window's visuals add to gameplay entities: all engine types.
    let ids: Vec<Entity> = {
        let index = app.world().resource::<shards::ai_core::AiIndex>();
        index.ids().into_iter().filter_map(|id| index.get(id)).collect()
    };
    for entity in ids {
        app.world_mut().entity_mut(entity).insert((Visibility::default(), GlobalTransform::from_xyz(1.0, 2.0, 3.0)));
        app.world_mut().spawn((Transform::default(), ChildOf(entity)));
    }
    assert_eq!(replay::state_hash(app.world()), before, "visual components are not game data");
    ok(&mut app, "set", json!({"id": "e6", "component": "Health", "path": "current", "value": 10}));
    assert_ne!(replay::state_hash(app.world()), before, "game data is");
}

#[test]
fn an_entity_in_a_component_hashes_as_its_id() {
    // Render entities shift `Entity` indices between a window and headless; ids stay.
    let grudge = |app: &mut App| {
        app.register_type::<Grudge>();
        let index = app.world().resource::<shards::ai_core::AiIndex>();
        let (player, boss) = (index.get(AiId(5)).unwrap(), index.get(AiId(6)).unwrap());
        app.world_mut().entity_mut(player).insert(Grudge(boss));
        boss
    };
    let mut a = sandbox::test_app(42);
    // In b five more entities come before the world: the same boss, another `Entity`.
    let mut b = shards::testing::test_app(
        |config| config.seed = 42,
        |app| {
            app.add_plugins(SandboxPlugin).add_systems(Startup, |mut commands: Commands| {
                commands.spawn_batch((0..5).map(|_| Transform::default()));
            });
        },
    );
    let (boss_a, boss_b) = (grudge(&mut a), grudge(&mut b));
    assert_ne!(boss_a, boss_b);
    assert_eq!(replay::state_hash(a.world()), replay::state_hash(b.world()));
}

/// Reflects, but cannot be serialized: opaque without `ReflectSerialize`.
#[derive(Reflect, Clone, Default)]
#[reflect(opaque)]
#[allow(dead_code)] // only reflection looks inside
struct Secret(u8);

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Locked {
    secret: Secret,
}

#[test]
fn a_component_that_does_not_serialize_is_left_out_once() {
    let mut app = sandbox::test_app(42);
    app.register_type::<Locked>();
    let player = app.world().resource::<shards::ai_core::AiIndex>().get(AiId(5)).unwrap();
    app.world_mut().entity_mut(player).insert(Locked::default());
    let before = replay::state_hash(app.world());
    app.world_mut().get_mut::<Locked>(player).unwrap().secret = Secret(9);
    assert_eq!(replay::state_hash(app.world()), before, "left out of the hash");
    ok(&mut app, "step", json!({"ticks": 180}));
    let warnings: Vec<String> = app
        .world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == JournalKind::Warn && e.text.contains("leave out"))
        .map(ToString::to_string)
        .collect();
    assert_eq!(warnings.len(), 1, "once, not at every check: {warnings:?}");
    assert!(warnings[0].contains("replays::Locked: it does not serialize"), "{warnings:?}");
}
