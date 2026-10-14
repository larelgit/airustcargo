//! `shards_world` integration tests, one binary: the generator on test content, checks on
//! hand-made manifests, dungeons, regions in a headless game (loading, changes, replays,
//! tools), active zones and tile streaming.

mod checks;
mod common;
mod dungeon;
mod generation;
mod nav;
mod regions;
mod zones;
