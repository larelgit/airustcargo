//! Scenario files run in-process: the same text `shards scenario FILE` runs against a
//! live game, here against a fresh headless one. A scenario that gets stale (content
//! changed, an id moved) fails here before it fails in CI against a window.

use std::path::PathBuf;

use shards::ai_core::scenario::run_file;

use crate::SEED;

/// Every `*.scenario` in `scenarios/`, sorted.
fn scenario_files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "scenario"))
        .collect();
    files.sort();
    files
}

#[test]
fn there_is_a_smoke_scenario() {
    assert!(scenario_files().iter().any(|p| p.ends_with("smoke.scenario")), "{:?}", scenario_files());
}

#[test]
fn every_scenario_passes_on_a_fresh_game() {
    for path in scenario_files() {
        let mut app = my_game::test_app(SEED);
        let report = run_file(app.world_mut(), &path);
        assert!(report.passed, "{report}");
    }
}

#[test]
fn the_smoke_scenario_walks_to_the_grunt_and_defeats_it() {
    let mut app = my_game::test_app(SEED);
    let report = run_file(app.world_mut(), PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios/smoke.scenario"));
    assert!(report.passed, "{report}");
    // A real fight, not a lucky first tick, and well inside the scenario's own limits.
    assert!((100..2000).contains(&report.ticks), "the fight took {} ticks\n{report}", report.ticks);
}
