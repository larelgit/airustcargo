//! The sandbox end to end, headless, through `shards::ai_core::call_tool` (the entry point
//! behind BRP and `shards`): what an agent sees, does and gets back, in one test binary.
//! Tests that install process-wide state have files of their own (`log_capture.rs`,
//! `system_failures.rs`), and so do the ones that time ticks (`perf.rs`).

mod actions;
mod common;
mod content;
mod coverage;
mod course;
mod fuzz;
// `gltf` takes its math from `ops` (each includer reaches Bevy by its own path).
use shards::bevy::math::ops;
// The glTF writer of shards_level's tests: the region's level files are made with it.
#[path = "../../../shards_level/tests/level/gltf.rs"]
mod gltf;
mod heavy;
mod heavy_saves;
mod engine_rules;
mod invariants;
mod net;
mod net_coop;
mod net_guards;
mod net_lifecycle;
mod net_predict;
mod net_sync;
mod net_seams;
#[cfg(feature = "render")]
mod offscreen;
mod reference_replays;
mod replays;
mod saves;
mod scenario_files;
mod scenarios;
mod settings;
mod shutdown;
mod snapshots;
mod state_audit;
mod soak;
mod stand_udp;
mod tiles;
mod tool_contracts;
