//! # shards_voxel — voxel terrain (G1 spike)
//!
//! A region's ground as a box of 1 m blocks: [`VoxelVolume`] (chunked storage, a natural-ground
//! height per column, generation edits), [`VoxelPalette`] (what each block id is), a neutral test
//! generator ([`testgen`]), boxes for convex-only physics ([`merge`]) and a chunk mesher with
//! vertex colours and corner darkening ([`mesh`]).
//!
//! This crate is the G1 spike: it exists to measure the collision candidates and the mesher
//! (`tests/spike.rs`, run in an optimised build) and to seed the engine module. See the design
//! note `engine-design-voxel.md` in the port documents.

pub mod merge;
pub mod mesh;
pub mod noise;
pub mod palette;
pub mod testgen;
pub mod volume;

pub use merge::{column_boxes, greedy_boxes, water_boxes, CellBox};
pub use mesh::{mesh_all, mesh_chunk, ChunkMeshes, MeshData, Meshing};
pub use noise::Noise;
pub use palette::{test_palette, BlockDef, VoxelPalette};
pub use testgen::TestWorld;
pub use volume::{VoxelVolume, AIR};
