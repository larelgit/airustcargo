//! The sandbox end to end, headless, through `shards::ai_core::call_tool` (the entry point
//! behind BRP and `shards`): what an agent sees, does and gets back, in one test binary.
//! Tests that install process-wide state have files of their own (`log_capture.rs`,
//! `system_failures.rs`).

mod actions;
mod common;
mod content;
mod course;
mod reference_replays;
mod replays;
mod scenario_files;
mod scenarios;
mod shutdown;
mod snapshots;
