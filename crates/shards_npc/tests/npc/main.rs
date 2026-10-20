//! `shards_npc` integration tests, one binary: headless arenas with test content, NPCs
//! driven by their behaviours, players placed (or played through tools) by the tests.

mod common;
mod audit;
mod avoid;
mod bodies;
mod director;
mod duel;
mod fsm;
mod nav;
mod navmesh;
#[cfg(feature = "render")]
mod overlay;
mod perception;
mod replay;
mod rules;
mod schedule;
mod snapshot;
mod tools;
mod voxel_nav;
mod warden;
