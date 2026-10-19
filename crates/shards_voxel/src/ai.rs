//! What agents can ask of a voxel region: `sea_current_at` (what a player could feel),
//! `voxel_info` (the developer's view of the volume), and the stop conditions `in_water`,
//! `in_current` and `near_grid_edge` (`step --until named:in_current`). The conditions read
//! physics and the region API, so they answer in a region of any kind (false where there is no
//! water, no current, or no region), and content that names them is valid in every region.

use ai_core::prelude::*;
use ai_core::spatial::fmt_pos;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{AiAppExt, DevCommand};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_motion::physics::sync_physics;
use shards_motion::{current_at, edge_distance, PhysicsWorld, VolumeKind};
use shards_world::ActiveRegion;

use crate::region::{VoxelTerrain, CONTENT};
use crate::volume::AIR;

pub(crate) fn register(app: &mut App) {
    register_tool(app.world_mut(), "sea_current_at", Scope::Player, ToolKind::Query, sea_current_at);
    register_tool(app.world_mut(), "voxel_info", Scope::Dev, ToolKind::Query, voxel_info);
    app.add_dev_entry("voxel", "voxel_info", "Voxel terrain: size, steps, hash, water", DevCommand::Run("voxel_info".into()));
    app.add_ai_condition("in_water", in_water).add_ai_condition("in_current", in_current).add_ai_condition("near_grid_edge", near_grid_edge);
}

/// How far above the feet `in_water` looks: a body standing on a pool's floor has its feet on
/// the bottom of the water volume.
const FEET_IN_WATER: f32 = 0.05;

/// The player's feet, if there is a player.
fn player_at(world: &World) -> Option<Vec3> {
    let player = ai_core::observe::player_entity(world)?;
    world.get::<Transform>(player).map(|t| t.translation)
}

/// The player's feet are in a water volume (wading or swimming), in a region of any kind.
fn in_water(world: &World) -> bool {
    let (Some(at), Some(physics)) = (player_at(world), world.get_resource::<PhysicsWorld>()) else { return false };
    physics.volumes_at(at + Vec3::Y * FEET_IN_WATER, None).iter().any(|(_, kind)| *kind == VolumeKind::Water)
}

/// The water around the player pushes it (a [`Current`](shards_motion::Current) is not zero
/// there).
fn in_current(world: &World) -> bool {
    let (Some(at), Some(physics)) = (player_at(world), world.get_resource::<PhysicsWorld>()) else { return false };
    current_at(physics, world, at) != Vec3::ZERO
}

/// The player is near the edge of the region's grid or past it: within a voxel region's
/// `near_edge` metres of it, and past it in a region of tiles (which has no such number).
fn near_grid_edge(world: &World) -> bool {
    let (Some(at), Some(region)) = (player_at(world), world.get_resource::<ActiveRegion>()) else { return false };
    let near = world.get_resource::<VoxelTerrain>().map_or(0.0, |t| t.near_edge.max(0.0));
    edge_distance(region.manifest.area(), at) < near
}

/// The current of the water at a point: which way it pushes (toward the middle of the voxel
/// grid, in the ground plane) and how hard (0 to 1), and how far the point is from the grid's
/// edge. Read-only; nothing moves bodies by it yet.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SeaCurrentAtArgs {
    /// The point `[x, y, z]`, m. Default: your body's feet.
    pub at: Option<[f32; 3]>,
}

fn sea_current_at(In(call): In<Call<SeaCurrentAtArgs>>, world: &mut World) -> ToolResult<Value> {
    let at = match call.args.at {
        Some([x, y, z]) if x.is_finite() && y.is_finite() && z.is_finite() => Vec3::new(x, y, z),
        Some(_) => return Err("at: three finite numbers, `sea_current_at at=[x,y,z]`".into()),
        None => player_at(world).ok_or("no body to ask for: give a point, `sea_current_at at=[x,y,z]`")?,
    };
    // Tools run between ticks: the volumes must be where the next tick sees them.
    let _ = world.run_system_cached(sync_physics);
    let push = world.get_resource::<PhysicsWorld>().map_or(Vec3::ZERO, |physics| current_at(physics, world, at));
    let round = |v: f32| (v * 1000.0).round() / 1000.0;
    let mut out = json!({
        "at": fmt_pos(at),
        "current": [round(push.x), round(push.y), round(push.z)],
        "strength": round(push.length()),
    });
    if push == Vec3::ZERO {
        out["note"] = json!("no current here");
    }
    if let Some(terrain) = world.get_resource::<VoxelTerrain>() {
        out["edge"] = json!(round(terrain.edge_distance(at)));
    }
    Ok(out)
}

/// The voxel region in the world: its size, chunks, palette, seed, the steps that made it with
/// their versions, its hash, the water boxes and sea boxes, the colliders every query reads
/// (`wide volumes`), the memory of the field, and whether its last build came from the cache
/// (with timing on, how long it took). Read-only.
#[derive(Deserialize, JsonSchema, Default)]
pub struct VoxelInfoArgs {
    /// Count the cells of each block too (one pass over the volume). Default true.
    pub blocks: Option<bool>,
}

fn voxel_info(In(call): In<Call<VoxelInfoArgs>>, world: &mut World) -> ToolResult<Value> {
    let _ = world.run_system_cached(sync_physics);
    let Some(t) = world.get_resource::<VoxelTerrain>() else {
        return Err(format!("no voxel region in the world: make one with `regen region=<id>` (a `{CONTENT}` definition)"));
    };
    let v = &t.volume;
    let (size, chunk, chunks) = (v.size(), v.chunk(), v.chunk_count());
    let mut out = json!({
        "region": format!("{} seed {} v{}", t.region, t.seed, t.version),
        "attempt": t.attempt,
        "made_with_seed": t.made_with,
        "size": format!("{} x {} x {} cells of {} m", size.x, size.y, size.z, v.block()),
        "chunk": format!("{} x {} x {}", chunk.x, chunk.y, chunk.z),
        "chunks": format!("{} x {} x {}", chunks.x, chunks.y, chunks.z),
        "origin": fmt_pos(v.origin()),
        "palette": t.palette.def().blocks.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        "steps": t.steps.iter().map(|(n, ver)| format!("{n} v{ver}")).collect::<Vec<_>>(),
        "hash": format!("{:016x}", t.hash),
        "water_boxes": t.water_boxes,
        "sea_boxes": t.sea_boxes,
        "current": t.current.map(|c| format!("from {} m inside the edge, full at {} m", c.starts, c.full)),
        "wide_volumes": world.get_resource::<PhysicsWorld>().map_or(0, PhysicsWorld::wide_colliders),
        "field": json!({"solid_cells": t.field.solid_cells(), "memory_bytes": t.field.memory()}),
        "volume_memory_bytes": v.memory(),
        "cached": t.cached,
        "nav": "baked in memory, chunk by chunk, when the region loads (on machines that simulate): see `nav_info`",
        "terrain": format!("entity `voxel terrain {}` (no AiId): a voxel field, {} solid cells", t.region, t.field.solid_cells()),
    });
    // Wall time differs from run to run: shown only where timing is reported (not in tests
    // that compare whole answers, `report_perf: false`).
    if world.get_resource::<ai_core::perf::AiPerf>().is_some_and(|p| p.report) {
        out["built"] = json!(format!("{:.1} ms{}", t.built_in.as_secs_f64() * 1000.0, if t.cached { " (from the cache)" } else { "" }));
    }
    if call.args.blocks.unwrap_or(true) {
        let mut counts = serde_json::Map::new();
        for (i, b) in t.palette.def().blocks.iter().enumerate() {
            let id = i as u8 + 1;
            counts.insert(b.name.clone(), json!(v.count(|c| c == id)));
        }
        counts.insert("air".to_owned(), json!(v.count(|c| c == AIR)));
        out["blocks"] = Value::Object(counts);
    }
    Ok(out)
}
