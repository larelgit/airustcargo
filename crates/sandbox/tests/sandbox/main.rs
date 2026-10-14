//! The sandbox end to end, headless, through `shards::ai_core::call_tool` (the entry point
//! behind BRP and `shards`): what an agent sees, does and gets back, in one test binary.
//! Tests that install process-wide state have files of their own (`log_capture.rs`,
//! `system_failures.rs`), and so do the ones that time ticks (`perf.rs`).

mod actions;
mod common;
mod content;
mod course;
// The glTF writer of shards_level's tests: the region's level files are made with it.
#[path = "../../../shards_level/tests/level/gltf.rs"]
mod gltf;
mod invariants;
mod net;
mod net_guards;
mod net_lifecycle;
mod net_sync;
mod net_seams;
mod reference_replays;
mod replays;
mod saves;
mod scenario_files;
mod scenarios;
mod settings;
mod shutdown;
mod snapshots;
mod tiles;
