//! A gameplay system that panics stops the game; it does not kill it.
//!
//! Bevy's error handler is a plain `fn`, so failures queue up globally: these tests
//! have their own file (their own process) and run one at a time.

use std::sync::Mutex;

use shards::ai_core::replay;
use shards::ai_core::{call_tool, AiFailures, AiJournal, AiReplay, CallOutcome, JournalKind};
use shards::bevy::app::FixedMain;
use shards::bevy::time::TimeUpdateStrategy;
use shards::prelude::*;
use shards::serde_json::{json, Value};

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Gives way exactly once, at tick 50.
fn cracking_ice(tick: Res<SimTick>) {
    if tick.0 == 50 {
        panic!("the ice gave way");
    }
}

const SYSTEM: &str = "system_failures::cracking_ice";

fn icy_app() -> App {
    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, cracking_ice.in_set(SimSet::Resolve));
    app
}

fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = act(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

fn text(out: &CallOutcome) -> &str {
    out.result.as_str().unwrap_or_default()
}

fn journal(app: &App, kind: JournalKind) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == kind).map(ToString::to_string).collect()
}

fn has_fixed_schedules(app: &App) -> bool {
    let schedules = app.world().resource::<Schedules>();
    schedules.contains(FixedMain) && schedules.contains(FixedUpdate)
}

#[test]
fn a_panic_stops_the_step_at_its_tick() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = icy_app();
    let out = ok(&mut app, "step", json!({"ticks": 100}));
    assert!(
        text(&out).starts_with(&format!("ran 50 ticks (0 → 50), stopped: gameplay system {SYSTEM} panicked at tick 50")),
        "{}",
        text(&out)
    );
    assert!(text(&out).contains("the ice gave way"), "{}", text(&out));
    assert_eq!(shards::ai_core::sim::tick(app.world()), 50);
    let line = format!("50 [error] system {SYSTEM} panicked at tick 50: the ice gave way");
    assert!(journal(&app, JournalKind::Error).contains(&line), "{:?}", journal(&app, JournalKind::Error));
    assert!(out.journal.contains(&line), "the step reports it too: {:?}", out.journal);
}

#[test]
fn the_game_runs_on_after_a_failure() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = icy_app();
    ok(&mut app, "step", json!({"ticks": 100}));
    assert!(has_fixed_schedules(&app), "FixedMain and FixedUpdate must survive the panic");
    let out = ok(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(text(&out), "ran 10 ticks (50 → 60)");
    assert_eq!(shards::ai_core::sim::tick(app.world()), 60);
    assert!(has_fixed_schedules(&app));
}

#[test]
fn real_time_play_pauses_on_a_failure() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = icy_app();
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1)); // one tick per frame
    ok(&mut app, "resume", json!({}));
    for _ in 0..80 {
        app.update();
    }
    assert!(shards::ai_core::sim::is_paused(app.world()), "the failure must pause the game");
    assert_eq!(shards::ai_core::sim::tick(app.world()), 50, "no tick after the failing one");
    assert!(journal(&app, JournalKind::Sim).iter().any(|l| l.contains(&format!("paused: system {SYSTEM} panicked at tick 50"))));
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l.contains("panicked at tick 50: the ice gave way")));
    assert!(has_fixed_schedules(&app));
}

#[test]
fn the_crash_replay_reproduces_the_failure() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = icy_app();
    app.world_mut().resource_mut::<AiFailures>().crash_replays = true;
    ok(&mut app, "attack", json!({"target": "e6", "repeat": true}));
    let out = ok(&mut app, "step", json!({"ticks": 100}));
    assert!(text(&out).ends_with("; crash replay: replays/crash-tick50.json"), "{}", text(&out));
    assert!(journal(&app, JournalKind::Sim).iter().any(|l| l.contains("crash replay saved: replays/crash-tick50.json")));
    let path = std::env::current_dir().unwrap().join("replays/crash-tick50.json");
    let recording = replay::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(recording.end.tick, 50);

    // The same build plays it back into the same failure, at the same tick.
    let mut again = icy_app();
    replay::play(again.world_mut(), recording).unwrap();
    let out = ok(&mut again, "step", json!({"ticks": 100}));
    assert!(text(&out).contains(&format!("stopped: gameplay system {SYSTEM} panicked at tick 50")), "{}", text(&out));
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");
}

#[test]
fn a_failure_while_loading_keeps_the_game_paused_after_it() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(EngineConfig { paused: false, ..shards::testing::config() }))
        .add_systems(OnEnter(EngineState::InGame), missing_floor);
    app.finish();
    app.cleanup();
    shards::testing::wait_until_loaded(&mut app, std::time::Duration::from_secs(20)).unwrap();
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1)); // one tick per frame, if it runs
    for _ in 0..5 {
        app.update();
    }
    assert!(shards::ai_core::sim::is_paused(app.world()), "opening the gate must not resume a failed game");
    assert_eq!(shards::ai_core::sim::tick(app.world()), 0);
    let paused = journal(&app, JournalKind::Sim);
    assert!(paused.iter().any(|l| l.contains("paused: system system_failures::missing_floor panicked at tick 0")), "{paused:?}");
}

/// An `OnEnter` that fails to spawn the world.
fn missing_floor() {
    panic!("the floor is missing");
}

/// Gives way at tick 3 while `Boom` is set: another run of the same ticks does not.
#[derive(Resource)]
struct Boom(bool);

fn flaky_ice(tick: Res<SimTick>, boom: Res<Boom>) {
    if tick.0 == 3 && boom.0 {
        panic!("the ice gave way");
    }
}

#[test]
fn a_rewind_past_a_failure_checks_the_rules_of_the_tick_that_failed() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = sandbox::test_app(42);
    app.insert_resource(Boom(true))
        .add_systems(FixedUpdate, flaky_ice.in_set(SimSet::Resolve))
        // Broken at tick 3 and not before or after.
        .add_ai_invariant("only_at_three", |w| (shards::ai_core::sim::tick(w) == 3).then(|| "broken at tick 3".to_owned()));
    ok(&mut app, "step", json!({"ticks": 1}));
    let before = shards::ai_core::snapshot::capture(app.world()).unwrap();

    // The failure halted tick 3, and its rules were left alone.
    let out = ok(&mut app, "step", json!({"ticks": 10}));
    assert!(text(&out).contains("stopped: gameplay system system_failures::flaky_ice panicked at tick 3"), "{}", text(&out));
    assert!(app.world().resource::<shards::ai_core::AiInvariants>().violations().is_empty());

    // Back to tick 1 and on another way (the system does not fail now): tick 3 is a tick like
    // any other, and the rule that is broken in it fires.
    app.world_mut().resource_mut::<Boom>().0 = false;
    shards::ai_core::snapshot::restore(app.world_mut(), &before, false).unwrap();
    let out = ok(&mut app, "step", json!({"ticks": 5}));
    assert_eq!(text(&out), "ran 2 ticks (1 → 3), stopped: invariant only_at_three violated at tick 3");
}

#[test]
fn a_panicking_tool_is_not_a_gameplay_failure() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = icy_app();
    app.add_ai_tool("boom", Scope::Dev, |_: In<Call<NoArgs>>| -> ToolResult { panic!("kaboom") });
    let out = act(&mut app, "boom", json!({}));
    assert!(text(&out).starts_with("boom panicked: kaboom (the game kept running"), "{}", out.result);
    assert!(has_fixed_schedules(&app));
    assert!(!journal(&app, JournalKind::Sim).iter().any(|l| l.contains("paused")), "a tool panic does not pause");
    // A command the tool queued that fails is the tool's, too.
    app.add_ai_tool("orphan", Scope::Dev, |_: In<Call<NoArgs>>, mut commands: Commands| -> ToolResult {
        let gone = commands.spawn_empty().id();
        commands.entity(gone).despawn();
        commands.entity(gone).insert(Name::new("nobody"));
        Ok("ok".into())
    });
    let out = ok(&mut app, "orphan", json!({}));
    assert!(out.journal.iter().any(|l| l.contains("[error] tool orphan: command")), "{:?}", out.journal);
    ok(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(shards::ai_core::sim::tick(app.world()), 10, "the tool's failure does not stop the next step");
}

#[test]
fn without_the_handler_the_lost_schedules_are_named() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin {
            sim: SimConfig { start_paused: true, ..default() },
            remote_port: None,
            catch_system_panics: false,
            crash_replays: false,
            ..default()
        })
        .add_systems(FixedUpdate, cracking_ice.in_set(SimSet::Resolve));
    app.finish();
    app.cleanup();
    app.update();
    let out = act(&mut app, "step", json!({"ticks": 100}));
    assert!(!out.ok);
    assert!(text(&out).contains("the game lost its FixedMain, FixedUpdate schedules"), "{}", text(&out));
}
