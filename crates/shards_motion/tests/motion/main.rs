//! `shards_motion` integration tests, one binary: headless worlds driven tick by tick.

mod camera_content;
mod common;
mod controller;
mod controller_parity;
mod course;
mod drive;
mod edge_cases;
mod engine;
mod ground;
mod input;
mod launch;
mod modifiers;
mod modifiers_parity;
mod orders;
mod perception;
mod physics;
mod replays;
mod seams;
mod tools;
mod voxel;
#[cfg(feature = "render")]
mod camera;
#[cfg(feature = "render")]
mod camera_big;
