//! `shards_world` integration tests, one binary: the generator on test content, checks on
//! hand-made manifests, dungeons, regions in a headless game (loading, changes, replays,
//! tools), active zones and tile streaming, and tiles built from level files.

mod checks;
mod common;
mod dungeon;
mod generation;
// The writer of the glTF files Blender would export, from `shards_level`'s tests: the level
// tiles here are generated, not committed.
#[path = "../../../shards_level/tests/level/gltf.rs"]
mod gltf;
mod levels;
mod nav;
mod regions;
mod zones;
