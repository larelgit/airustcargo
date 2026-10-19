//! The reference replays in `tests/replays/` (repository root): the obstacle course, the
//! combat prototype, the combat director, an enemy on the navmesh, the generated region and
//! "go to boss phase" by snapshot (that one starts from its snapshot) and saves (save, get
//! hurt, load, die, load again, play on: the replay starts from the state after the last
//! load) and the voxel arena (a voxel region: its steps, ledges, pool and cave), played by their
//! scenarios on a fresh game. Each one is verified the
//! way CI and a person do it, by the game itself: `sandbox --replay FILE --verify` must say
//! `identical` and exit 0. A change that alters gameplay makes them diverge; record them
//! again on purpose:
//!
//! ```text
//! cargo test -p sandbox --test sandbox -- --ignored record_reference_replays
//! ```
//!
//! The same files verify in a window too (`--verify --windowed`, under `xvfb-run -a` on a
//! machine without a display).

use std::path::{Path, PathBuf};
use std::process::Command;

use shards::ai_core::replay;
use shards::ai_core::scenario::run_file;

use crate::scenario_files::scenario;

/// The scenarios recorded as reference replays, by the replay's file name.
const RECORDED: [(&str, &str); 14] = [
    ("obstacle_course.json", "obstacle_course.scenario"),
    ("combat_prototype.json", "combat_prototype.scenario"),
    ("npc_director.json", "npc_director.scenario"),
    ("npc_navmesh.json", "npc_navmesh.scenario"),
    ("region.json", "region.scenario"),
    ("snapshot.json", "snapshot.scenario"),
    ("saves.json", "saves.scenario"),
    ("reactions.json", "reactions.scenario"),
    ("voxel_arena.json", "voxel_arena.scenario"),
    ("timeline.json", "timeline.scenario"),
    ("context.json", "context.scenario"),
    ("body_motion.json", "body_motion.scenario"),
    ("body_state.json", "body_state.scenario"),
    ("encounter.json", "encounter.scenario"),
];

fn replays_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays")
}

#[test]
fn every_reference_replay_verifies_headless() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(replays_dir())
        .expect("tests/replays/ at the repository root")
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    for (name, _) in RECORDED {
        assert!(names.iter().any(|n| n == name), "{name} is missing from tests/replays/: {names:?}");
    }
    let mut failed = Vec::new();
    for file in &files {
        let out = Command::new(env!("CARGO_BIN_EXE_sandbox"))
            .arg("--replay")
            .arg(file)
            .arg("--verify")
            .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let report = stdout.lines().find(|l| l.starts_with("replay ")).unwrap_or_default().to_owned();
        // A gameplay component the checks leave out (it does not serialize) is a failure here,
        // not a warning: the reference replays would not see it change.
        let left_out = stdout.contains("replay checks leave out ");
        if out.status.code() != Some(0) || !report.contains("identical to the recording") || left_out {
            failed.push(format!(
                "{}: exit {:?}\n{stdout}{}",
                file.display(),
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

/// Plays each recorded scenario on a fresh game and writes its replay. Run by hand.
#[test]
#[ignore = "rewrites tests/replays/: run it when gameplay changed on purpose"]
fn record_reference_replays() {
    let _turn = crate::scenario_files::snapshot_file_turn();
    std::fs::create_dir_all(replays_dir()).unwrap();
    for (name, file) in RECORDED {
        let mut app = sandbox::test_app(42);
        let report = run_file(app.world_mut(), scenario(file));
        assert!(report.passed, "{report}");
        let recording = replay::save(app.world());
        let text = shards::serde_json::to_string_pretty(&recording).unwrap();
        std::fs::write(replays_dir().join(name), text + "\n").unwrap();
        println!("{name}: {} ticks, {} events", recording.end.tick, recording.events.len());
    }
}
