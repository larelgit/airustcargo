//! # shards_voxel — voxel terrain, the core
//!
//! A region's ground as a box of blocks, built by code and never changed by play:
//!
//! - [`VoxelVolume`]: chunked storage (the chunk size is a parameter, a region's `chunk:`;
//!   64 x 32 x 64 is the port's), block ids (a byte, 0 is air), solid / liquid / foliage queries
//!   through the palette, the natural **surface** per column and the real top
//!   ([`column_top`](VoxelVolume::column_top), [`column_runs`](VoxelVolume::column_runs)), the
//!   generation edits (raise and lower a column, clear, carve, flatten, fill, sphere, tube) and
//!   [`hash`](VoxelVolume::hash), an FNV over a canonical form.
//! - [`VoxelPaletteDef`] / [`VoxelPalette`]: the content kind `voxel_palette`
//!   (`*.voxel_palette.ron`): blocks, colours, face shade, corner darkening, looks; validated
//!   when content loads.
//! - [`Noise`]: deterministic value noise on integer hashes, `+ - * /` and `floor` only.
//! - [`VoxelSteps`], [`GenSpec`], [`VoxelGen`], [`generate`]: generation as **steps** and
//!   **checks** registered by name (steps with a version, `add_voxel_step`), run in order with
//!   attempts; a pure function of (seed, version, step set, params, palette) through `DetRng`.
//!   Stock steps: `noise_hills`, `fill_water`.
//! - [`column_boxes`] (the fallback collision and the reference a voxel field is measured
//!   against) and [`water_boxes`] (water for every voxel region).
//!
//! What is not here yet: the region kind, collision, navigation and meshing (later PRs of the
//! design). The module costs nothing in a game without a voxel region: it registers a content
//! kind and a registry and adds no system.
//!
//! Determinism: nothing here calls a platform `sin`, `powf` or `exp`; random numbers come from
//! `DetRng` keyed by the generation's inputs, ordered collections are `BTreeMap`s or indices.

pub mod worldgen;
pub mod merge;
pub mod noise;
pub mod palette;
pub mod volume;

use bevy::prelude::*;
use shards_content::ContentAppExt;
use shards_core::EngineAppExt;

pub use worldgen::{
    block_named, fill_water, generate, noise_hills, step_set_hash, world_hash, CheckFn, FillWaterParams, GenSpec, Generated, Mark, NoiseHillsParams, StepFn, VoxelGen,
    VoxelSteps, MAX_ATTEMPTS,
};
pub use merge::{column_boxes, water_boxes, CellBox};
pub use noise::Noise;
pub use palette::{AoDef, BlockDef, Meshing, ShadeDef, VariationDef, VoxelPalette, VoxelPaletteDef, MAX_BLOCKS};
pub use volume::{VoxelVolume, AIR, MAX_CELLS};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Registration of voxel generation, on the `App` while it is built.
pub trait VoxelAppExt {
    /// Registers generation step `name` at `version`, for regions that list `(name, version)`.
    /// Raise the version when the step's output changes. Panics on a name already taken, a name
    /// that is not lowercase letters, digits and `_`, and version 0: it is a mistake in the
    /// game's build, not a runtime condition.
    fn add_voxel_step(&mut self, name: &str, version: u32, run: StepFn) -> &mut Self;

    /// Registers a check regions can name; a failed check fails the generation attempt. Panics
    /// like [`add_voxel_step`](Self::add_voxel_step).
    fn add_voxel_check(&mut self, name: &str, run: CheckFn) -> &mut Self;
}

impl VoxelAppExt for App {
    fn add_voxel_step(&mut self, name: &str, version: u32, run: StepFn) -> &mut Self {
        let result = self.world_mut().get_resource_or_init::<VoxelSteps>().add_step(name, version, run);
        if let Err(e) = result {
            panic!("{e}");
        }
        self
    }

    fn add_voxel_check(&mut self, name: &str, run: CheckFn) -> &mut Self {
        let result = self.world_mut().get_resource_or_init::<VoxelSteps>().add_check(name, run);
        if let Err(e) = result {
            panic!("{e}");
        }
        self
    }
}

/// The voxel module: the `voxel_palette` content kind and the registry of steps and checks (with
/// the stock steps). Needs the engine core and content first; `shards::EnginePlugins` adds it
/// after them.
pub struct VoxelPlugin;

impl Plugin for VoxelPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "VoxelPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<VoxelPaletteDef>().init_resource::<VoxelSteps>().add_engine_module("voxel", VERSION);
    }
}
