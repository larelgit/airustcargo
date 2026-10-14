//! Replays: every session is recorded from tick 0 as inputs, and playing it back must give
//! the same game. A change to gameplay that is not meant to change the game shows up here
//! (and in CI, which also verifies the checked-in files through the game's own binary:
//! `cargo game --replay tests/replays/smoke.json --verify`).
//!
//! The reference replay is re-recorded, deliberately, with
//! `cargo test --test game record_reference_replays -- --ignored`.

use std::path::PathBuf;

use shards::ai_core::replay::{self, Replay};
use shards::ai_core::{AiId, AiIndex, AiReplay, ReplayReport};
use shards::prelude::*;

use crate::{fight, state, step, story, SEED};

fn replays_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/replays")
}

fn report(app: &App) -> ReplayReport {
    app.world().resource::<AiReplay>().report().cloned().expect("the playback has not reached its end")
}

#[test]
fn a_recorded_session_plays_back_identically() {
    let mut original = my_game::test_app(SEED);
    fight(&mut original);
    // Through JSON, like a file.
    let recording: Replay = shards::serde_json::from_str(&shards::serde_json::to_string(&replay::save(original.world())).unwrap()).unwrap();
    assert!(recording.end.tick >= 1000, "the session ran {} ticks", recording.end.tick);

    let mut copy = my_game::test_app(recording.seed);
    let end = recording.end.tick;
    replay::play(copy.world_mut(), recording).unwrap();
    step(&mut copy, end);
    copy.update(); // calls made after the last tick, then the final comparison

    let report = report(&copy);
    assert!(report.identical(), "{report}");
    assert_eq!(state(&mut original), state(&mut copy));
    assert_eq!(story(&original), story(&copy));
}

#[test]
fn a_changed_game_reports_the_tick_it_stopped_matching() {
    let mut original = my_game::test_app(SEED);
    fight(&mut original);
    let recording = replay::save(original.world());
    let end = recording.end.tick;

    // The same recording on a game where the grunt starts with less health, as after a
    // balance change that was not meant to touch this fight.
    let mut changed = my_game::test_app(SEED);
    let grunt = changed.world().resource::<AiIndex>().get(AiId(2)).unwrap();
    changed.world_mut().get_mut::<Health>(grunt).unwrap().current = 10.0;
    replay::play(changed.world_mut(), recording).unwrap();
    step(&mut changed, end);
    changed.update();

    let report = report(&changed);
    assert!(!report.identical(), "a different game must not verify");
    assert_eq!(report.diverged_at, Some(60), "{report}");
    assert!(report.to_string().contains("DIVERGED at tick 60"), "{report}");
}

/// Every `tests/replays/*.json`: recorded by an earlier build, still the same game.
#[test]
fn reference_replays_still_match() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(replays_dir())
        .map(|dir| dir.map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "json")).collect())
        .unwrap_or_default();
    files.sort();
    assert!(!files.is_empty(), "no reference replays in {}; record them (see the module docs)", replays_dir().display());
    for path in files {
        let recording = replay::load(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let end = recording.end.tick;
        let mut app = my_game::test_app(recording.seed);
        replay::play(app.world_mut(), recording).unwrap();
        step(&mut app, end);
        app.update();
        let report = report(&app);
        assert!(report.identical(), "{}: {report}\nrecord it again if the change is intended", path.display());
    }
}

/// Not a test: writes the checked-in reference replay (`--ignored`). It records the smoke
/// scenario, so the replay is the game as a scenario plays it.
#[test]
#[ignore = "writes tests/replays/smoke.json"]
fn record_reference_replays() {
    let mut app = my_game::test_app(SEED);
    let scenario = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios/smoke.scenario");
    let result = shards::ai_core::scenario::run_file(app.world_mut(), scenario);
    assert!(result.passed, "{result}");
    std::fs::create_dir_all(replays_dir()).unwrap();
    let path = replays_dir().join("smoke.json");
    std::fs::write(&path, shards::serde_json::to_string_pretty(&replay::save(app.world())).unwrap()).unwrap();
    println!("wrote {}", path.display());
}
