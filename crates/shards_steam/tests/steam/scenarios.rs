//! The scenarios of this crate (`scenarios/*.scenario`) on a stand over the fake Steam: the host of
//! the stand makes an `invisible` lobby and a client waits for one, as `cargo sandbox --stand 1
//! --stand-steam --steam-lobby invisible` starts them. The same files run against that live stand.

use std::path::{Path, PathBuf};

use shards::core::SteamLobby;
use shards::net::MemoryHub;
use shards::script::Script;
use shards::testing::run_stand_script;
use shards_steam::FakeSteam;

use crate::lobbies::{hosted, waiting};

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
fn every_scenario_passes_on_a_stand_over_the_fake_steam() {
    let mut failed = Vec::new();
    for file in scenario_files() {
        let name = file.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let script = Script::parse(&name, &text).unwrap_or_else(|e| panic!("{e}"));
        let header = script.stand().copied().unwrap_or_else(|| panic!("{name} has no `stand` line: these scenarios need a stand"));
        assert_eq!(header.clients, 1, "{name}: the stand has the host and one client");

        let steam = FakeSteam::new(MemoryHub::new(1));
        let (mut stand, _) = hosted(&steam, SteamLobby::Invisible);
        waiting(&mut stand, &steam);
        let report = run_stand_script(&mut stand, &script);
        println!("{report}");
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} scenario(s) failed:\n\n{}", failed.len(), failed.join("\n\n"));
}
