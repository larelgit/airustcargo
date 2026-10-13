//! Every file in `scenarios/` runs in-process on a fresh game, the way
//! `shards scenario FILE` runs it against a live one: the same commands, expectations
//! and invariants, no window.

use std::path::{Path, PathBuf};

use ai_core::scenario::run_file;

/// The sandbox's reference scenarios, in file-name order.
fn scenario_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "scenario"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no *.scenario files in {}", dir.display());
    files
}

#[test]
fn every_scenario_file_passes_on_a_fresh_game() {
    let mut failed = Vec::new();
    for file in scenario_files() {
        let mut app = sandbox::test_app(42);
        let report = run_file(app.world_mut(), &file);
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} scenario(s) failed:\n\n{}", failed.len(), failed.join("\n\n"));
}

/// The scenarios also pass one after the other in the same game, the way
/// `shards scenario crates/sandbox/scenarios/*.scenario` runs them against one live game:
/// each puts the arena back before it starts.
#[test]
fn the_scenarios_also_pass_in_one_game_after_another() {
    let mut app = sandbox::test_app(42);
    let mut failed = Vec::new();
    for file in scenario_files() {
        let report = run_file(app.world_mut(), &file);
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} scenario(s) failed in a shared game:\n\n{}", failed.len(), failed.join("\n\n"));
}
