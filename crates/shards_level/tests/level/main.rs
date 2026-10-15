//! `shards_level` integration tests, one binary. There is no Blender here: `gltf` writes
//! the files Blender's exporter writes (a node tree, meshes, custom properties as `extras`),
//! `common` builds headless worlds with the engine's content database, and the committed
//! fixture `assets/arena.gltf` is what the sandbox and the template use.

mod asset;
mod colliders;
mod common;
mod content;
mod fixture;
mod gltf;
mod markers;
mod props;
mod replay;
#[cfg(feature = "render")]
mod render;
mod spawn;
