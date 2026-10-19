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
//! - [`VoxelRegionDef`]: the content kind `voxel_region` (`*.voxel_region.ron`), a region kind of
//!   the world module ([`region`]): `regen region=<id>` generates it, builds its terrain (a
//!   voxel field in physics), its water volumes, its sea and current, and its points of
//!   interest, behind the same region API as tiles; [`VoxelTerrain`] is the region in the
//!   world. Tools: `sea_current_at`, `voxel_info`; conditions `in_water`, `in_current`,
//!   `near_grid_edge` (they read physics and the region API, so they answer in any region).
//!
//! What is not here yet: meshing (a later PR of the design). The region's navigation is the NPC
//! module's: it bakes the chunks in memory when the region loads. The module costs nothing in a
//! game without a voxel region: it registers content kinds, a registry, a region kind and its
//! tools, and adds no system.
//!
//! **The rule for games:** the volume's own types ([`VoxelVolume`], [`VoxelGen`], the palette's
//! cells) are for generation steps. Gameplay reads a voxel region through the region API of the
//! world module and through physics, as it reads a region of tiles, so content can swap one
//! kind of region for the other.
//!
//! Determinism: nothing here calls a platform `sin`, `powf` or `exp`; random numbers come from
//! `DetRng` keyed by the generation's inputs, ordered collections are `BTreeMap`s or indices.

pub mod worldgen;
mod ai;
pub mod merge;
pub mod noise;
pub mod palette;
pub mod region;
pub mod volume;

use bevy::prelude::*;
use shards_content::ContentAppExt;
use shards_core::EngineAppExt;

pub use worldgen::{
    block_named, fill_water, generate, generate_attempt, generate_attempts, noise_hills, step_set_hash, world_hash, AttemptError, AttemptStage, CheckFn,
    FillWaterParams, GenSpec, Generated, Mark, NoiseHillsParams, StepFn, VoxelGen, VoxelSteps, MAX_ATTEMPTS,
};
pub use region::{CurrentDef, SeaDef, VoxelRegionDef, VoxelSettings, VoxelTerrain, WaterDef};
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
        share_steps(self.world_mut());
        self
    }

    fn add_voxel_check(&mut self, name: &str, run: CheckFn) -> &mut Self {
        let result = self.world_mut().get_resource_or_init::<VoxelSteps>().add_check(name, run);
        if let Err(e) = result {
            panic!("{e}");
        }
        share_steps(self.world_mut());
        self
    }
}

/// Hands a copy of the step registry to what reads it outside the app's world: content checks
/// (a region naming a step this game never registered is a content error) and the world
/// module's generator (reports and saves clone it). Again after every registration.
fn share_steps(world: &mut World) {
    let Some(steps) = world.get_resource::<VoxelSteps>().cloned() else { return };
    if let Some(mut kinds) = world.get_resource_mut::<shards_content::ContentKinds>() {
        kinds.set_context(steps.clone());
    }
    if let Some(mut worldgen) = world.get_resource_mut::<shards_world::WorldGen>() {
        worldgen.set_kind_data(region::CONTENT, steps);
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
        app.add_content::<VoxelPaletteDef>()
            .add_content::<VoxelRegionDef>()
            .init_resource::<VoxelSteps>()
            .init_resource::<VoxelSettings>()
            .add_engine_module("voxel", VERSION);
        // A voxel region is a region kind of the world module: without it there are no regions.
        if app.is_plugin_added::<shards_world::WorldPlugin>() {
            use ai_core::AiAppExt;
            use shards_world::WorldAppExt;
            app.add_region_kind(region::kind())
                // Absent outside a voxel region: replays and snapshots of other worlds keep their bytes.
                .add_optional_replay_check("voxel.volume", region::volume_check)
                .add_optional_snapshot_check(
                    "voxel.volume",
                    "the voxel volume differs: the region was made again from other content or another version of a generation step",
                    region::volume_check,
                )
                // The volume compares once the world module has built the snapshot's region.
                .add_snapshot_prepare_after("voxel", "region", &["voxel.volume"], |_, _| Ok(None))
                .on_content_applied(region::content_applied);
            ai::register(app);
        }
        share_steps(app.world_mut());
    }

    fn finish(&self, app: &mut App) {
        // Added before the world module, this plugin could not register its region kind (the
        // world module was not there yet): every `regen` of a voxel region would say "unknown
        // region". A mistake in the game's build, said where it is made.
        let registered = app.world().get_resource::<shards_world::WorldGen>().is_some_and(|g| g.kind(region::CONTENT).is_some());
        assert!(
            !app.is_plugin_added::<shards_world::WorldPlugin>() || registered,
            "VoxelPlugin was added before WorldPlugin, so voxel regions are not a region kind of this game: add VoxelPlugin after WorldPlugin (shards::EnginePlugins does)"
        );
        share_steps(app.world_mut());
    }
}
