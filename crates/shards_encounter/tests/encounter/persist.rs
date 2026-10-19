//! Snapshots resume a fight tick for tick; a world save keeps only what was won.

use shards::prelude::*;
use serde_json::json;
use shards::ai_core::snapshot::{self, differences};
use shards::ai_core::{replay, sim, AiJournal, JournalKind};
use shards::encounter::{EncounterRuns, RunState};

use crate::common::*;

fn story_after(app: &App, n: u64) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal
        .after(0)
        .0
        .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

fn assert_same_state(what: &str, a: &App, b: &App) {
    assert_eq!(sim::tick(a.world()), sim::tick(b.world()), "{what}: tick");
    let (sa, sb) = (snapshot::capture(a.world()).unwrap(), snapshot::capture(b.world()).unwrap());
    let diff = differences(&sa, &sb);
    assert!(diff.is_empty(), "{what}: the states differ:\n{}", diff.join("\n"));
    assert_eq!(replay::state_hash(a.world()), replay::state_hash(b.world()), "{what}: state hash");
}

fn player_body(app: &mut App) -> Entity {
    app.world_mut().query_filtered::<Entity, With<PlayerId>>().iter(app.world()).next().unwrap()
}

/// A fight in `ring_lock`: wave 1 spawned and killed, the second wave `left` ticks from spawning.
fn between(left: u32) -> App {
    let mut app = watched(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "ring_lock", "l", Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": key}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    tick(&mut app);
    let want = RunState::Between { left };
    run_until(&mut app, 60, |a| state(a, &key) == want || matches!(state(a, &key), RunState::Running));
    assert_eq!(state(&app, &key), want);
    app
}

/// The proof: `a` is a game in the situation to prove; snapshot it, go on, load the snapshot into
/// a fresh game, go on the same way: the same state byte for byte and the same journal.
fn assert_continues_alike(what: &str, mut a: App, after: impl Fn(&mut App)) {
    let snapshot = snapshot::capture(a.world()).unwrap();
    let n = sim::tick(a.world());
    after(&mut a);
    let mut b = watched(1);
    let report = snapshot::restore(b.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(report.exact(), "{what}: restored differently: {:?}", report.differences);
    assert_eq!(sim::tick(b.world()), n);
    assert_eq!(replay::state_hash(b.world()), snapshot.state_hash, "{what}: the loaded state is the saved one");
    after(&mut b);
    assert_same_state(what, &a, &b);
    assert_eq!(story_after(&a, n), story_after(&b, n), "{what}: the journal from tick {n} on");
    assert!(sim::tick(a.world()) > n);
}

#[test]
fn a_snapshot_between_waves_spawns_the_next_wave_at_the_same_tick_with_the_same_ids() {
    let a = between(17);
    assert_continues_alike("between waves", a, |app| step(app, 40));
}

#[test]
fn a_snapshot_mid_wave_and_in_a_wipe_resumes_the_same() {
    let mut a = between(12);
    run_until(&mut a, 60, |app| matches!(state(app, "world/l"), RunState::Running));
    assert_continues_alike("mid wave", a, |app| step(app, 20));

    let mut a = between(12);
    run_until(&mut a, 60, |app| matches!(state(app, "world/l"), RunState::Running));
    let p0 = player_body(&mut a);
    kill(&mut a, p0);
    step(&mut a, 3);
    assert!(matches!(state(&a, "world/l"), RunState::Wiped { .. }));
    assert_continues_alike("wiped", a, |app| {
        let p0 = player_body(app);
        revive(app, p0);
        step(app, 200);
    });
}

#[test]
fn a_snapshot_without_runs_removes_the_runs_a_world_has() {
    let a = app(1);
    let snapshot = snapshot::capture(a.world()).unwrap();
    assert!(!snapshot.resources.keys().any(|k| k.contains("EncounterRuns")), "no runs, no resource in the snapshot");
    let mut b = between(10);
    assert!(b.world().get_resource::<EncounterRuns>().is_some());
    let report = snapshot::restore(b.world_mut(), &snapshot, false).unwrap();
    assert!(b.world().get_resource::<EncounterRuns>().is_none(), "stale runs would count the replaced members as out");
    let notes = format!("{report:?}");
    assert!(notes.contains("encounter runs removed"), "{notes}");
}

// --- world saves ------------------------------------------------------------------------

#[test]
fn a_world_save_keeps_the_won_keys_and_restores_them_silently() {
    let mut app = watched(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "once_one", "o", Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": key}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Won);
    assert_eq!(saw(&app, "completed"), 1);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "qa-encounter"}));
    // The world forgets (a fresh world would), then loads.
    ok(&mut app, "encounter_reset", json!({"key": "all", "forget": true}));
    assert!(app.world().get_resource::<EncounterRuns>().is_none());
    ok(&mut app, "load_game", json!({"what": "world", "slot": "qa-encounter"}));
    assert!(journal_has(&app, "encounters: 1 won restored (world/o)"), "{:?}", journal(&app));
    assert_eq!(app.world().resource::<EncounterRuns>().completed, vec![key.clone()]);
    step(&mut app, 2);
    assert_eq!(state(&app, &key), RunState::Won, "made again from its anchor: won, with no members");
    assert!(run(&app, &key).members.is_empty());
    assert_eq!(saw(&app, "completed"), 1, "no EncounterCompleted by a load");
    assert_eq!(count(&app, "world/o: won"), 1);
}

#[test]
fn a_world_saved_mid_fight_loads_the_fight_fresh() {
    let mut app = watched(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "ring_lock", "l", Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": key}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    let member = members(&app, &key)[0].1;
    ok(&mut app, "save_game", json!({"what": "world", "slot": "qa-fight"}));
    ok(&mut app, "load_game", json!({"what": "world", "slot": "qa-fight"}));
    // Nothing of the fight is saved: the member is gone, the run is made fresh on the next tick.
    if let Some(runs) = app.world().get_resource::<EncounterRuns>() {
        assert!(runs.runs.is_empty());
    }
    assert!(app.world().get_entity(member).is_err());
    step(&mut app, 2);
    assert_eq!(state(&app, &key), RunState::Armed, "made fresh: it waits for its start again");
    assert_eq!(run(&app, &key).wave, 0);
}
