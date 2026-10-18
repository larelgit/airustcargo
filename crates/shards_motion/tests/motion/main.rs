//! `shards_motion` integration tests, one binary: headless worlds driven tick by tick.

mod common;
mod controller;
mod course;
mod drive;
mod edge_cases;
mod engine;
mod input;
mod modifiers;
mod perception;
mod physics;
mod replays;
mod tools;
#[cfg(feature = "render")]
mod camera;
