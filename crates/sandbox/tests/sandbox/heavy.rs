//! Heavy mode (`--heavy N`) on the sandbox: every N ticks a fresh game loads a snapshot of the
//! session and must continue exactly like it. The reference replay passes it through the
//! game's own command line; three planted bugs of the kinds it exists for (state a snapshot
//! does not hold) fail it, and pass once fixed; a failure stops `step` like a broken invariant.

use std::path::{Path, PathBuf};
use std::process::Command;

use sandbox::gameplay::Player;
use sandbox::SandboxPlugin;
use shards::ai_core::heavy::AiHeavy;
use shards::ai_core::{AiJournal, JournalKind, SimTick};
use shards::prelude::*;
use shards::serde_json::json;
use shards::HeavyConfig;

use crate::common::*;

fn replays_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays")
}

/// The per-push run of CI: the combat prototype, checked every 300 ticks.
#[test]
fn the_combat_prototype_continues_alike_from_its_snapshots() {
    let out = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .arg("--replay")
        .arg(replays_dir().join("combat_prototype.json"))
        .args(["--verify", "--heavy", "300"])
        .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let verdict = stdout.lines().find(|l| l.starts_with("replay ")).unwrap_or_default();
    assert!(verdict.contains("identical to the recording"), "{stdout}");
    let checked: u64 = verdict
        .split("heavy: ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no heavy count in `{verdict}`"));
    assert!(checked >= 3, "{verdict}");
    assert!(verdict.contains(", 0 skipped"), "{verdict}");
}

#[test]
fn heavy_flags_are_checked_before_the_game_starts() {
    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_sandbox"))
            .args(args)
            .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (code, err) = run(&["--headless", "--no-remote", "--heavy", "30"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("add --paused"), "{err}");
    let (code, err) = run(&["--headless", "--paused", "--no-remote", "--heavy", "30", "--heavy-span", "601"]);
    assert_eq!(code, Some(2), "{err}");
    let (code, err) = run(&["--headless", "--paused", "--no-remote", "--heavy-span", "30"]);
    assert_eq!(code, Some(2), "--heavy-span needs --heavy: {err}");
}

// --- planted bugs ------------------------------------------------------------------

/// A value on the player's body that the planted systems change: the state the twin must
/// arrive at.
#[derive(Component, Reflect, Default, Clone)]
#[reflect(Component)]
struct Planted(u32);

fn plant(mut commands: Commands, bodies: Query<Entity, (With<Player>, Without<Planted>)>) {
    for body in &bodies {
        commands.entity(body).insert(Planted(0));
    }
}

/// Bug 1: a counter outside entities, which the snapshot does not hold unless registered.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Drift(u32);

fn drift(mut drift: ResMut<Drift>, mut planted: Query<&mut Planted>) {
    drift.0 += 1;
    for mut p in &mut planted {
        p.0 = p.0.wrapping_add(drift.0);
    }
}

/// Bug 2: a system's own `Local`, which no snapshot can hold.
fn every_fiftieth_call(mut calls: Local<u32>, mut planted: Query<&mut Planted>) {
    *calls += 1;
    if (*calls).is_multiple_of(50) {
        for mut p in &mut planted {
            p.0 += 1000;
        }
    }
}

/// Bug 2, fixed: the same rhythm from the simulation's own clock, which the snapshot holds.
fn every_fiftieth_tick(tick: Res<SimTick>, mut planted: Query<&mut Planted>) {
    if tick.0.is_multiple_of(50) {
        for mut p in &mut planted {
            p.0 += 1000;
        }
    }
}

/// Bug 3: a cache derived from the world (how many thousands the planted values hold), kept up
/// to date as they grow but not rebuilt after a load.
#[derive(Resource, Default)]
struct Thousands(u32);

fn grow(mut planted: Query<&mut Planted>, mut thousands: ResMut<Thousands>) {
    for mut p in &mut planted {
        let before = p.0 / 1000;
        p.0 += 37 + thousands.0;
        thousands.0 += p.0 / 1000 - before;
    }
}

fn rebuild_thousands(world: &mut World) {
    let mut planted = world.query::<&Planted>();
    let sum = planted.iter(world).map(|p| p.0 / 1000).sum();
    world.resource_mut::<Thousands>().0 = sum;
}

#[derive(Clone, Copy, PartialEq)]
enum Bug {
    Counter,
    Local,
    Cache,
}

/// The sandbox with `bug` planted, or with its fix.
fn planted(bug: Bug, fixed: bool) -> impl Fn(&mut App) + Send + Sync + 'static {
    move |app: &mut App| {
        app.add_plugins(SandboxPlugin).register_type::<Planted>().add_systems(FixedUpdate, plant.in_set(SimSet::Cleanup));
        match (bug, fixed) {
            (Bug::Counter, _) => {
                app.init_resource::<Drift>().add_systems(FixedUpdate, drift.in_set(SimSet::Cleanup).after(plant));
                if fixed {
                    app.add_snapshot_resource::<Drift>();
                }
            }
            (Bug::Local, false) => {
                app.add_systems(FixedUpdate, every_fiftieth_call.in_set(SimSet::Cleanup).after(plant));
            }
            (Bug::Local, true) => {
                app.add_systems(FixedUpdate, every_fiftieth_tick.in_set(SimSet::Cleanup).after(plant));
            }
            (Bug::Cache, _) => {
                app.init_resource::<Thousands>().add_systems(FixedUpdate, grow.in_set(SimSet::Cleanup).after(plant));
                if fixed {
                    app.add_snapshot_rebuild("thousands", rebuild_thousands);
                }
            }
        }
    }
}

fn heavy_sandbox(bug: Bug, fixed: bool) -> App {
    shards::testing::heavy_app(|config| config.seed = 42, planted(bug, fixed), HeavyConfig { every: 30, span: 30, offset: 0 })
}

/// 150 ticks of `bug` (or its fix) in heavy mode: the step's answer.
fn run(bug: Bug, fixed: bool) -> (App, String) {
    let mut app = heavy_sandbox(bug, fixed);
    let out = act(&mut app, "step", json!({"ticks": 150}));
    assert!(out.ok, "{}", out.result);
    let text = text(&out).to_owned();
    (app, text)
}

fn assert_caught(bug: Bug, what: &str) {
    let (app, text) = run(bug, false);
    assert!(text.contains("stopped: heavy check failed at tick 60: e5 "), "{text}");
    assert!(text.contains(what), "{text}");
    let heavy = app.world().resource::<AiHeavy>();
    assert_eq!(heavy.failures().len(), 1);
    assert_eq!(heavy.failures()[0].from, 30, "the twin started from the snapshot of tick 30");

    let (app, text) = run(bug, true);
    assert!(!text.contains("stopped"), "fixed, it passes: {text}");
    let heavy = app.world().resource::<AiHeavy>();
    assert!(heavy.failures().is_empty());
    assert_eq!(heavy.checked(), 5, "points 0, 30, 60, 90, 120 (the one at 150 ends later)");
}

#[test]
fn a_counter_the_snapshot_does_not_hold_is_caught() {
    assert_caught(Bug::Counter, "Planted");
}

#[test]
fn a_local_in_a_gameplay_system_is_caught() {
    assert_caught(Bug::Local, "Planted");
}

#[test]
fn a_cache_not_rebuilt_after_a_load_is_caught() {
    assert_caught(Bug::Cache, "Planted");
}

#[test]
fn a_failed_check_halts_like_a_broken_invariant_and_stays_off_until_reset() {
    let mut app = heavy_sandbox(Bug::Counter, false);
    let out = act(&mut app, "step", json!({"ticks": 150}));
    assert_eq!(out.halted.as_ref().map(|h| h.reason.as_str()), Some("heavy check failed at tick 60"));
    assert_eq!(shards::ai_core::sim::tick(app.world()), 60, "the step stopped at the check");
    let journal = app.world().resource::<AiJournal>();
    let errors: Vec<String> =
        journal.after(0).0.filter(|e| e.kind == JournalKind::Error).map(|e| e.text.clone()).collect();
    assert!(
        errors.iter().any(|e| e.starts_with("heavy check failed at tick 60 (a fresh game from tick 30, ") && e.contains("e5 ")),
        "{errors:?}"
    );
    // A test app writes no files.
    assert!(!errors.iter().any(|e| e.contains("not saved")), "{errors:?}");

    // Off now: the session goes on, and the next points are not checked.
    let out = act(&mut app, "step", json!({"ticks": 90}));
    assert!(out.ok && out.halted.is_none(), "{}", out.result);
    assert!(text(&ok(&mut app, "assert_list", json!({}))).contains("fired at tick 60"));
    // `assert_reset` turns them on again: the counter still drifts, so the next one fails.
    assert!(text(&ok(&mut app, "assert_reset", json!({}))).contains("heavy"));
    let out = act(&mut app, "step", json!({"ticks": 90}));
    assert!(text(&out).contains("stopped: heavy check failed at tick 210"), "{}", out.result);
}

#[test]
fn heavy_mode_refuses_real_time() {
    let mut app = heavy_sandbox(Bug::Counter, true);
    let error = fails(&mut app, "resume", json!({}));
    assert!(error.contains("heavy checks run between stepped ticks"), "{error}");
}

/// More gameplay lines in a tick than the journal holds.
fn chatter(tick: Res<SimTick>, mut journal: ResMut<AiJournal>) {
    for i in 0..20 {
        journal.push(tick.0, JournalKind::Event, format!("chatter {i}"));
    }
}

/// The state of such a point is compared, its journal cannot be: it is not counted as checked.
#[test]
fn a_point_whose_journal_lines_were_lost_is_skipped_not_passed() {
    let setup = |app: &mut App| {
        app.add_plugins(SandboxPlugin)
            .insert_resource(AiJournal::with_capacity(16))
            .add_systems(FixedUpdate, chatter.in_set(SimSet::Cleanup));
    };
    let mut app = shards::testing::heavy_app(|config| config.seed = 42, setup, HeavyConfig { every: 30, span: 30, offset: 0 });
    let out = act(&mut app, "step", json!({"ticks": 30}));
    assert!(out.ok && out.halted.is_none(), "{}", out.result);
    let last = app.world().resource::<AiJournal>().after(0).0.last().map(|e| e.text.clone());
    let last = last.unwrap_or_default();
    assert!(
        last.starts_with("heavy point from tick 0 not checked: the state is the same, but the journal lost ")
            && last.ends_with(" of the twin before they were compared (more lines in a tick than it holds)"),
        "{last}"
    );
    let out = act(&mut app, "step", json!({"ticks": 60}));
    assert!(out.ok && out.halted.is_none(), "{}", out.result);
    let heavy = app.world().resource::<AiHeavy>();
    assert_eq!((heavy.checked(), heavy.skipped()), (0, 3), "points 0, 30 and 60");
}

#[test]
fn heavy_mode_off_is_invisible() {
    let app = sandbox::test_app(42);
    assert!(!app.world().contains_resource::<AiHeavy>());
    assert!(!app.world().contains_resource::<shards::ai_core::HeavyTwin>());
}

#[test]
fn an_engine_rule_that_fired_is_fired_in_the_twin_too() {
    // The twin takes over which rules have fired. If it did not, it would meet the same broken
    // state, fire the engine rule on its own and halt, and the check would fail on a rule, not on state.
    let mut app = heavy_sandbox(Bug::Counter, true);
    ok(&mut app, "set", json!({"id": "e5", "component": "Stamina", "path": "current", "value": -5}));
    let out = act(&mut app, "step", json!({"ticks": 20}));
    assert!(text(&out).contains("stopped: invariant action.resources_in_range violated at tick 1"), "{}", out.result);
    let out = act(&mut app, "step", json!({"ticks": 200}));
    assert!(out.ok && out.halted.is_none(), "{}", out.result);
    assert!(text(&ok(&mut app, "assert_list", json!({}))).contains("action.resources_in_range  engine rule  fired at tick 1"));
}
