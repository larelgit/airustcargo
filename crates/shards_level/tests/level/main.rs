//! `shards_level` integration tests, one binary. Blender is not needed to run them: `gltf` writes
//! glTF files the way Blender's exporter does (a node tree, meshes, custom properties as
//! `extras`), `common` builds headless worlds with the engine's content database, the
//! committed fixture `assets/arena.gltf` is what the sandbox and the template use, and
//! `from_blender` reads files Blender 5.2.2 really exported (`blender/`).

mod asset;
mod colliders;
mod common;
mod content;
mod fixture;
mod from_blender;
// `gltf` takes its math from `ops` (each includer reaches Bevy by its own path).
use bevy::math::ops;
mod gltf;
mod markers;
mod props;
mod replay;
#[cfg(feature = "render")]
mod render;
mod spawn;
