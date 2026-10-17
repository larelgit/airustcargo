//! World tools for agents: `regen`, `region_load`, `goto_tile` (dev actions, recorded in
//! replays), `worldgen_report` (dev query) and `region_info` (what a player could know).

use ai_core::prelude::*;
use ai_core::spatial::fmt_pos;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::SimSeed;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use shards_content::ContentDb;

use crate::checks::cells_text;
use crate::generate::WorldGen;
use crate::manifest::RegionManifest;
use crate::region::{self, ActiveRegion};

pub(crate) fn register(world: &mut World) {
    use Scope::{Dev, Player};
    use ToolKind::{Action, Query};
    register_tool(world, "regen", Dev, Action, regen);
    register_tool(world, "region_load", Dev, Action, region_load);
    register_tool(world, "goto_tile", Dev, Action, goto_tile);
    register_tool(world, "worldgen_report", Dev, Query, worldgen_report);
    register_tool(world, "region_info", Player, Query, region_info);
}

/// Generate a region (or dungeon) and change to it behind the loading screen: the old
/// region's entities go, the new ones come, players move to its spawn point. Recorded in
/// replays like any action, so playback regenerates at the same tick.
#[derive(Deserialize, JsonSchema, Default)]
pub struct RegenArgs {
    /// `region` or `dungeon` content id. Default: the region in the world.
    pub region: Option<String>,
    /// Default: the current seed + 1 for the same region, the world seed for another.
    pub seed: Option<u64>,
}

fn regen(In(call): In<Call<RegenArgs>>, world: &mut World) -> ToolResult {
    let active = world.get_resource::<ActiveRegion>().map(|r| (r.manifest.region.clone(), r.manifest.seed));
    let region = match (call.args.region, &active) {
        (Some(region), _) => region,
        (None, Some((current, _))) => current.clone(),
        (None, None) => {
            let known: Vec<&str> = {
                let db = world.resource::<ContentDb>();
                db.ids::<crate::content::RegionDef>().chain(db.ids::<crate::content::DungeonDef>()).collect()
            };
            return Err(format!("no region in the world yet: name one, e.g. regen region={}", known.first().unwrap_or(&"<id>")));
        }
    };
    let seed = match (call.args.seed, &active) {
        (Some(seed), _) => seed,
        (None, Some((current, seed))) if *current == region => seed.wrapping_add(1),
        (None, _) => world.get_resource::<SimSeed>().map_or(0, |s| s.0),
    };
    let (generated, spawned) = region::regen(world, &region, seed)?;
    let m = &generated.manifest;
    let how = match &m.fallback {
        Some(map) => format!("the fallback `{map}`: no attempt of {} passed", generated.failures.len()),
        None => format!("attempt {}", m.attempt),
    };
    let at = world.get_resource::<ActiveRegion>().map(|r| fmt_pos(r.spawn_point(0))).unwrap_or_default();
    Ok(format!("ok: {} ({how}, {} tiles, {} POIs, {} entities), spawn at {at}", m.title(), m.tiles.len(), m.pois.len(), spawned.entities))
}

/// Build a region from a saved manifest, as it was (nothing is generated again), and change
/// to it like `regen` does. Content must still have every tile it names.
#[derive(Deserialize, JsonSchema)]
pub struct RegionLoadArgs {
    /// The manifest: the object `region_info manifest=true` shows, or its text.
    pub manifest: Value,
}

fn region_load(In(call): In<Call<RegionLoadArgs>>, world: &mut World) -> ToolResult {
    let manifest = match call.args.manifest {
        Value::String(text) => RegionManifest::from_json(&text)?,
        other => RegionManifest::from_json(&other.to_string())?,
    };
    let title = manifest.title();
    let spawned = region::change_region(world, manifest)?;
    Ok(format!("ok: {title} loaded from its manifest ({} tiles, {} entities)", spawned.tiles, spawned.entities))
}

/// Teleport to the center of a tile, onto the ground there.
#[derive(Deserialize, JsonSchema)]
pub struct GotoTileArgs {
    /// Column, from 0 at the west edge.
    pub x: i32,
    /// Row, from 0 at the north edge.
    pub y: i32,
    /// Who to move. Default: you.
    pub id: Option<Id>,
}

fn goto_tile(In(call): In<Call<GotoTileArgs>>, world: &mut World) -> ToolResult {
    let GotoTileArgs { x, y, id } = call.args;
    let entity = match id {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let region = world.get_resource::<ActiveRegion>().ok_or("no region in the world; make one with `regen region=<id>`")?;
    let [w, h] = region.manifest.size;
    let Some(tile) = region.tile_at_cell([x, y]).cloned() else {
        return Err(format!("({x}, {y}) is outside {} (tiles 0..{w} × 0..{h})", region.manifest.title()));
    };
    let center = region.manifest.cell_center([x, y]);
    let ground = region::ground_below(world, center, &[entity])
        .ok_or_else(|| format!("tile ({x}, {y}) {} has no ground under its center to stand on; try a neighbor", tile.tile))?;
    let at = Vec3::new(center.x, ground, center.z);
    let mut tf = world.get_mut::<Transform>(entity).ok_or("that entity has no Transform")?;
    tf.translation = at;
    let who = world.get::<AiId>(entity).map_or_else(|| format!("{entity}"), ToString::to_string);
    Ok(format!("{who} at tile ({x}, {y}) {} rot {}: {}", tile.tile, tile.rot, fmt_pos(at)))
}

/// Generate a region for many seeds without building any and report: how many passed, why
/// attempts failed (with example seeds), distances between points of interest, generator
/// panics with their seed. The first line starts with `ok:` when every seed passed.
#[derive(Deserialize, JsonSchema)]
pub struct WorldgenReportArgs {
    /// `region` or `dungeon` content id.
    pub region: String,
    /// `1..1000` (both ends included), `7`, or `1,5,10..20`. Default 1..100.
    pub seeds: Option<String>,
}

fn worldgen_report(In(call): In<Call<WorldgenReportArgs>>, world: &mut World) -> ToolResult {
    let worldgen = world.resource::<WorldGen>().clone();
    let seeds = call.args.seeds.unwrap_or_else(|| "1..100".into());
    let report = crate::report::run(world.resource::<ContentDb>(), &worldgen, &call.args.region, &seeds)?;
    Ok(report.to_string().trim_end().to_owned())
}

/// The region in the world: its seed, size, points of interest; with `manifest`, the whole
/// manifest (what saves keep and `region_load` takes).
#[derive(Deserialize, JsonSchema, Default)]
pub struct RegionInfoArgs {
    /// Include the manifest.
    #[serde(default)]
    pub manifest: bool,
}

fn region_info(In(call): In<Call<RegionInfoArgs>>, region: Option<Res<ActiveRegion>>, db: Res<ContentDb>) -> ToolResult<Value> {
    let region = region.ok_or("no region in the world (a dev makes one with `regen region=<id>`)")?;
    let m = &region.manifest;
    let pois: Vec<String> = m.pois.iter().map(|p| format!("{} at {}", p.id, fmt_pos(p.position()))).collect();
    let origin = match &m.fallback {
        Some(map) => format!("fallback {map}"),
        None => format!("attempt {}", m.attempt),
    };
    let mut out = serde_json::json!({
        "region": m.title(),
        "made": origin,
        "size": format!("{}×{} tiles of {}m", m.size[0], m.size[1], m.tile_size),
        "spawn": m.spawn,
        "pois": pois,
        "links": m.links.iter().map(|l| format!("{} {}–{} through {}", l.kind, l.a, l.b, cells_text(&l.cells))).collect::<Vec<_>>(),
        "manifest_hash": format!("{:016x}", region.hash),
    });
    if m.origin != [0.0, 0.0] {
        out["north_west_corner"] = serde_json::json!(fmt_pos(m.corner()));
    }
    if let Some(height) = region::edge_wall(&db, m) {
        out["edge"] = serde_json::json!(format!("walled, {height}m high"));
    }
    if call.args.manifest {
        out["manifest"] = serde_json::to_value(m).map_err(|e| e.to_string())?;
    }
    Ok(out)
}
