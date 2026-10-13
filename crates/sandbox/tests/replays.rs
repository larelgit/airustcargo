//! Replays end to end: record a session, play it back in a fresh world, and get the
//! same game — or a report of the first tick where it stops being the same.

use ai_core::prelude::*;
use ai_core::replay::{self, Replay, ReplayEvent};
use ai_core::{call_tool, AiJournal, AiReplay, CallOutcome, JournalKind};
use bevy::prelude::*;
use sandbox::gameplay::combat::{Action, Health};
use sandbox::gameplay::intent::Intent;
use sandbox::gameplay::Player;
use serde_json::{json, Value};

const BOSS: &str = "e6";

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = call_tool(app.world_mut(), "dev", tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

/// What the keyboard does: straight into the player's `Intent`, between ticks.
fn press(app: &mut App, dir: Vec3) {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&mut Intent, With<Player>>();
    let mut intent = q.single_mut(world).unwrap();
    intent.orders.clear();
    intent.move_dir = dir;
}

/// Every AI-visible entity, bit-exact: position, rotation, health, action.
fn state(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &Transform, Option<&Health>, Option<&Action>)>();
    let mut lines: Vec<String> = q
        .iter(world)
        .map(|(id, tf, hp, action)| format!("{id} {:?} {:?} {hp:?} {action:?}", tf.translation, tf.rotation))
        .collect();
    lines.sort();
    lines
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
    ok(app, "set", json!({"id": BOSS, "component": "Health", "path": "current", "value": 2100}));
    ok(app, "attack", json!({"target": BOSS, "repeat": true}));
    ok(app, "step", json!({"ticks": 100}));
    press(app, Vec3::X);
    ok(app, "step", json!({"ticks": 20}));
    press(app, Vec3::ZERO);
    ok(app, "spawn", json!({"kind": "skeleton", "x": 4, "z": 6}));
    ok(app, "possess", json!({"id": "e7"}));
    ok(app, "set", json!({"id": "e7", "component": "Health", "path": "current", "value": 5}));
    ok(app, "heal", json!({})); // heals "you": e7
    ok(app, "possess", json!({}));
    ok(app, "dodge", json!({"away_from": BOSS}));
    ok(app, "step", json!({"ticks": 200}));
    ok(app, "teleport", json!({"id": "e9", "x": -3, "z": 0}));
    ok(app, "step", json!({"ticks": 37}));
    ok(app, "move", json!({"dir": "W", "meters": 2}));
}

fn recorded(seed: u64) -> (App, Replay) {
    let mut app = sandbox::test_app(seed);
    session(&mut app);
    let replay = replay::save(app.world());
    // Through JSON, like a file.
    let replay = serde_json::from_str(&serde_json::to_string(&replay).unwrap()).unwrap();
    (app, replay)
}

fn report(app: &App) -> ai_core::ReplayReport {
    app.world().resource::<AiReplay>().report().cloned().expect("playback has not finished")
}

#[test]
fn playback_reproduces_the_session() {
    let (mut original, replay) = recorded(42);
    assert_eq!(replay.end.tick, 357);
    assert!(replay.events.iter().any(|e| matches!(e, ReplayEvent::Input { at: 100, .. })), "keyboard input not recorded");
    assert!(
        !replay.events.iter().any(|e| matches!(e, ReplayEvent::Call { tool, .. } if tool == "step" || tool == "possess")),
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
    assert!(story(&copy).iter().any(|l| l.contains("e9 -> e5 Claw")), "{:#?}", story(&copy));
}

#[test]
fn a_changed_game_reports_the_first_diverging_tick() {
    let (_, mut replay) = recorded(42);
    // As if the boss had spawned with less health in a newer build.
    let ReplayEvent::Call { args, .. } = &mut replay.events[0] else { panic!("first event is the set call") };
    args["value"] = json!(2000);

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
    assert_eq!(ai_core::sim::tick(copy.world()), 140);
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
            let out = call_tool(world, &call.caller.agent, "spawn", json!({"kind": "skeleton", "x": x, "z": 8}));
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
    let path = std::env::temp_dir().join(format!("sandbox_replay_{}.json", std::process::id()));
    let out = ok(&mut app, "replay_save", json!({"path": path.display().to_string()}));
    let text = out.result.as_str().unwrap();
    assert!(text.contains("357 ticks") && text.contains("seed 42"), "{text}");
    let loaded = replay::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    assert_eq!(loaded, replay);
}
