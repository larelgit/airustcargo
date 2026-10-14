//! Every file in `scenarios/` runs in-process on a fresh game, the way
//! `shards scenario FILE` runs it against a live one: the same commands, expectations
//! and invariants, no window.

use std::path::{Path, PathBuf};

use shards::ai_core::scenario::run_file;
use shards::serde_json::json;

use crate::common::ok;

/// The sandbox's reference scenarios, in file-name order.
pub fn scenario_files() -> Vec<PathBuf> {
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

pub fn scenario(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios").join(name)
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
/// each puts the world back before it starts.
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

/// The warden chooses its attacks with `SimRng`, which draws from the tick: a game that
/// played on for a while before the fight sees other attacks, in another order. The
/// combat prototype is won from any of them, the same way.
#[test]
fn the_combat_prototype_is_won_whatever_the_warden_chooses() {
    let mut failed = Vec::new();
    let mut openings = Vec::new();
    for before in [17, 131, 263, 409, 587, 761] {
        let mut app = sandbox::test_app(42);
        ok(&mut app, "step", json!({"ticks": before}));
        let report = run_file(app.world_mut(), scenario("combat_prototype.scenario"));
        if !report.passed {
            failed.push(format!("after {before} ticks:\n{report}"));
        }
        let attacks: Vec<String> = report
            .log
            .iter()
            .filter_map(|l| {
                l.detail.split_once("e6 winds up ").map(|(_, rest)| rest.split(' ').next().unwrap().to_owned())
            })
            .collect();
        openings.push(attacks);
    }
    assert!(failed.is_empty(), "{} fight(s) lost:\n\n{}", failed.len(), failed.join("\n\n"));
    let distinct: std::collections::HashSet<&Vec<String>> = openings.iter().collect();
    assert!(distinct.len() > 3, "the fights should differ: {openings:#?}");
}
