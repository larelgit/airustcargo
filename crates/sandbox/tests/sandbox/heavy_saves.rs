//! Saves in heavy mode's spirit. A save does not hold the whole simulation (a body's place in
//! a swing, an enemy's plan), so a game loaded from one cannot continue exactly like the game
//! that saved it, and heavy mode's twin does not apply. What must hold instead is idempotence:
//! at five evenly spaced ticks of each reference replay, the world and the character saved
//! (`save_game what=all`), loaded into a fresh game (`load_game`) and saved again come out the
//! same, section by section, byte for byte.
//!
//! A replay writes no files while it plays, so each tick's state goes into a fresh game, which
//! is not playing, through a snapshot of it; that game saves.

use std::path::{Path, PathBuf};
use std::time::Duration;

use shards::ai_core::replay;
use shards::ai_core::snapshot;
use shards::prelude::*;
use shards::save::{SaveConfig, SaveFile, SaveKind, Which};
use shards::serde_json::json;

use crate::common::*;

fn replays_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays")
}

/// The sections of `slot`'s save of `kind` in `app`'s folder, in their canonical bytes.
fn sections(app: &App, kind: SaveKind, slot: &str) -> String {
    let store = app.world().resource::<SaveConfig>().store();
    let path = store.path(kind, slot, Which::Save);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file = SaveFile::decode(&bytes).unwrap_or_else(|e| panic!("{}: {}", path.display(), e.detail()));
    file.sections.canonical()
}

/// Saves `app` (all of it) into `slot`, then a fresh game loads that slot and saves it as
/// `again`: every section must be the same.
fn assert_idempotent(what: &str, saved: &mut App, seed: u64) {
    ok(saved, "save_game", json!({"what": "all", "slot": "first"}));
    let mut loaded = sandbox::test_app(seed);
    let dir = saved.world().resource::<SaveConfig>().dir.clone();
    loaded.world_mut().resource_mut::<SaveConfig>().dir = dir;
    ok(&mut loaded, "load_game", json!({"what": "all", "slot": "first"}));
    ok(&mut loaded, "save_game", json!({"what": "all", "slot": "again"}));
    for kind in [SaveKind::World, SaveKind::Character] {
        let (first, again) = (sections(saved, kind, "first"), sections(&loaded, kind, "again"));
        assert!(first == again, "{what}: the {kind:?} saved again after a load differs:\n{first}\n{again}");
    }
}

#[test]
fn every_reference_replay_saves_what_it_loads() {
    let mut files: Vec<PathBuf> = std::fs::read_dir(replays_dir())
        .unwrap()
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(files.len() >= 7, "{files:?}");
    for file in files {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let recording = replay::load(&file).unwrap();
        let (seed, hz) = (recording.seed, recording.tick_hz);
        let from = recording.start.as_ref().map_or(0, |s| s.tick);
        let end = recording.end.tick;
        let config = EngineConfig { seed, tick_hz: hz, ..shards::testing::config() };
        let mut playing = sandbox::build_app(&config);
        playing.finish();
        playing.cleanup();
        replay::play(playing.world_mut(), recording).unwrap();
        shards::testing::wait_until_loaded(&mut playing, Duration::from_secs(60)).unwrap();
        for i in 1..=5 {
            let tick = from + (end - from) * i / 5;
            let stops = shards::testing::run_to(&mut playing, tick, Duration::from_secs(60)).unwrap();
            assert!(stops.failures.is_empty() && stops.violations.is_empty(), "{name}: {stops:?}");
            let state = snapshot::capture(playing.world()).unwrap();
            let mut saved = sandbox::test_app(seed);
            let report = snapshot::restore(saved.world_mut(), &state, false).unwrap_or_else(|e| panic!("{name} at {tick}: {e}"));
            assert!(report.exact(), "{name} at {tick}: {:?}", report.differences);
            assert_idempotent(&format!("{name} at tick {tick}"), &mut saved, seed);
        }
    }
}
