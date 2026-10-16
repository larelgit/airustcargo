//! Draw commands per frame, counted in the render world: `draws` in `frames` of `ai/status` and in
//! the `perf` budgets ([`crate::frames`], [`crate::bench`]).
//!
//! Bevy 0.20 has no draw-call counter, and wgpu keeps none that the engine can read. What the
//! engine can read is what the render phases hold once the frame is batched: the draw functions
//! that the render graph will call, each of which issues one `draw_indexed` (or one multi-draw
//! indirect) on the pass. This module counts those calls, at the moment between Bevy's batching of
//! the phases and the render graph's use of them, over every view of the frame (every camera, and
//! every light's shadow views):
//!
//! * a **binned** phase (`Opaque3d`, `AlphaMask3d`, the prepasses, `Shadow`, the 2D ones) draws once per
//!   batch set that Bevy multi-draws (`multidrawable_meshes`: GPU culling), once per batch of
//!   `batchable_meshes` (one per bin: instances of one mesh with one material are one batch), once
//!   per entity of `unbatchable_meshes` and once per entity of `non_mesh_items`;
//! * a **sorted** phase (`Transparent3d`, `Transmissive3d`, `TransparentUi`, `Transparent2d`) draws
//!   once per item that begins a batch: Bevy's loop draws an item with a non-empty `batch_range` and
//!   skips the rest of that range, and this counts the same way.
//!
//! What this is not: the passes that are not phase items (the upscaling blit, tone mapping, bloom,
//! the other post-processing: each a full-screen triangle), compute dispatches, copies, and what a
//! GPU does inside a multi-draw (one indirect call counts once, whatever it draws). A phase item whose
//! pipeline is not compiled yet is counted, though Bevy skips it. Without GPU preprocessing (a GPU
//! with no compute shaders: WebGL) the batch sets of binned phases are private to Bevy, so there is
//! no number.

use bevy::core_pipeline::core_2d::{AlphaMask2d, Opaque2d, Transparent2d};
use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
use bevy::core_pipeline::deferred::{AlphaMask3dDeferred, Opaque3dDeferred};
use bevy::core_pipeline::prepass::{AlphaMask3dPrepass, Opaque3dPrepass};
use bevy::pbr::{Shadow, Transmissive3d};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{GpuPreprocessingMode, GpuPreprocessingSupport};
use bevy::render::render_phase::{BinnedPhaseItem, BinnedRenderPhase, SortedPhaseItem, SortedRenderPhase, ViewBinnedRenderPhases, ViewSortedRenderPhases};
use bevy::ui_render::TransparentUi;

/// The draws of one binned phase of one view.
pub fn binned_draws<BPI: BinnedPhaseItem>(phase: &BinnedRenderPhase<BPI>) -> u64 {
    let unbatchable: usize = phase.unbatchable_meshes.values().map(|bin| bin.entities.len()).sum();
    let non_mesh: usize = phase.non_mesh_items.values().map(|bin| bin.entities.len()).sum();
    (phase.multidrawable_meshes.len() + phase.batchable_meshes.len() + unbatchable + non_mesh) as u64
}

/// The draws of one sorted phase of one view: the loop of `SortedRenderPhase::render`, which draws
/// an item and skips the rest of its batch.
pub fn sorted_draws<I: SortedPhaseItem>(phase: &SortedRenderPhase<I>) -> u64 {
    let (mut index, mut draws) = (0, 0);
    while let Some((_, item)) = phase.items.get_index(index) {
        match item.batch_range().len() {
            0 => index += 1,
            batch => {
                draws += 1;
                index += batch;
            }
        }
    }
    draws
}

fn binned<BPI: BinnedPhaseItem>(world: &World) -> u64 {
    world.get_resource::<ViewBinnedRenderPhases<BPI>>().map_or(0, |phases| phases.values().map(binned_draws).sum())
}

fn sorted<I: SortedPhaseItem>(world: &World) -> u64 {
    world.get_resource::<ViewSortedRenderPhases<I>>().map_or(0, |phases| phases.values().map(sorted_draws).sum())
}

/// Every draw of the frame, in the render world; `None` where Bevy keeps the batches to itself.
pub fn count(world: &World) -> Option<u64> {
    let support = world.get_resource::<GpuPreprocessingSupport>()?;
    if support.max_supported_mode == GpuPreprocessingMode::None {
        return None;
    }
    Some(
        binned::<Opaque3d>(world)
            + binned::<AlphaMask3d>(world)
            + binned::<Opaque3dPrepass>(world)
            + binned::<AlphaMask3dPrepass>(world)
            + binned::<Opaque3dDeferred>(world)
            + binned::<AlphaMask3dDeferred>(world)
            + binned::<Shadow>(world)
            + binned::<Opaque2d>(world)
            + binned::<AlphaMask2d>(world)
            + sorted::<Transparent3d>(world)
            + sorted::<Transmissive3d>(world)
            + sorted::<Transparent2d>(world)
            + sorted::<TransparentUi>(world),
    )
}
