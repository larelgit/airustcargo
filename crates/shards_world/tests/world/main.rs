//! `shards_world` integration tests, one binary: the generator on test content, checks on
//! hand-made manifests, dungeons, regions in a headless game (loading, changes, replays,
//! tools), region changes that go wrong, active zones and tile streaming, and tiles built
//! from level files.

mod changes;
mod checks;
mod common;
mod dungeon;
mod generation;
// `gltf` takes its math from `ops` (each includer reaches Bevy by its own path).
use bevy::math::ops;
// The writer of the glTF files Blender would export, from `shards_level`'s tests: the level
// tiles here are generated, not committed.
#[path = "../../../shards_level/tests/level/gltf.rs"]
mod gltf;
mod levels;
mod nav;
mod regions;
mod zones;
