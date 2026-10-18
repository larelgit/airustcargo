//! World tools for agents: `regen`, `region_load`, `goto_tile` (dev actions, recorded in
//! replays), `worldgen_report` (dev query) and `region_info` (what a player could know).

use ai_core::prelude::*;
use ai_core::spatial::fmt_pos;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{DevCommand, SimSeed};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use shards_content::ContentDb;

use crate::checks::cells_text;
use crate::generate::WorldGen;
use crate::manifest::{Layout, RegionManifest};
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

/// The developer menu's world entries: the seed, the generator's checks for this seed and
/// for many, a tile to go to, another region.
pub(crate) fn dev_entries(app: &mut App) {
    /// The region in the world, or why the entry does nothing here.
    fn manifest(world: &World) -> Result<&RegionManifest, String> {
        world.get_resource::<ActiveRegion>().map(|r| &r.manifest).ok_or_else(|| "no region in the world: make one with `regen region=<id>`".to_owned())
    }
    app.add_dev_entry("world", "region_info", "Region and its seed", DevCommand::Run("region_info".into()))
        .add_dev_entry(
            "world",
            "region_check",
            "Check this seed: rules, reachability",
            DevCommand::Live(|w, _| manifest(w).map(|m| format!("worldgen_report region={} seeds={}", m.region, m.seed))),
        )
        .add_dev_entry(
            "world",
            "worldgen_200",
            "Generate 200 regions like this one",
            DevCommand::Live(|w, _| manifest(w).map(|m| format!("worldgen_report region={} seeds=1..200", m.region))),
        )
        .add_dev_entry("world", "goto_tile", "Go to tile...", DevCommand::Fill("goto_tile x= y=".into()))
        .add_dev_entry("world", "regen", "New region...", DevCommand::Fill("regen region= seed=".into()));
}

/// Generate a region (or dungeon) and change to it behind the loading screen: the old
/// region's entities go, the new ones come, players move to its spawn point. Recorded in
/// replays like any action, so playback regenerates at the same tick.
#[derive(Deserialize, JsonSchema, Default)]
pub struct RegenArgs {
    /// A region content id of any kind (`region`, `dungeon`, `voxel_region`, ...). Default: the region in the world.
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
            let known = crate::generate::known_regions(world.resource::<ContentDb>(), world.get_resource::<WorldGen>());
            return Err(format!("no region in the world yet: name one, e.g. regen region={}", known.first().map_or("<id>", String::as_str)));
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
    Ok(format!("ok: {} ({how}, {}, {} POIs, {} entities), spawn at {at}", m.title(), cells_of(m), m.pois.len(), spawned.entities))
}

/// What a region is made of: `64 tiles`, `9 rooms`, `16 chunks`.
fn cells_of(m: &RegionManifest) -> String {
    match m.layout {
        Layout::Grid => format!("{} tiles", m.tiles.len()),
        Layout::Rooms => format!("{} rooms", m.tiles.len()),
        Layout::Volume => format!("{} chunks", u64::from(m.size[0]) * u64::from(m.size[1])),
    }
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
    let (title, cells) = (manifest.title(), cells_of(&manifest));
    let spawned = region::change_region(world, manifest)?;
    Ok(format!("ok: {title} loaded from its manifest ({cells}, {} entities)", spawned.entities))
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
    // A volume's cells are its chunks: no tile, the chunk's centre.
    let tile = match region.manifest.layout {
        Layout::Volume if region.manifest.contains([x, y]) => None,
        _ => match region.tile_at_cell([x, y]).cloned() {
            Some(tile) => Some(tile),
            None => {
                let cells = if region.manifest.layout == Layout::Volume { "chunks" } else { "tiles" };
                return Err(format!("({x}, {y}) is outside {} ({cells} 0..{w} × 0..{h})", region.manifest.title()));
            }
        },
    };
    let center = region.manifest.cell_center([x, y]);
    let name = tile.as_ref().map_or_else(|| format!("chunk ({x}, {y})"), |t| format!("tile ({x}, {y}) {}", t.tile));
    let ground = region::ground_below(world, center, &[entity]).ok_or_else(|| format!("{name} has no ground under its center to stand on; try a neighbor"))?;
    let at = Vec3::new(center.x, ground, center.z);
    let mut tf = world.get_mut::<Transform>(entity).ok_or("that entity has no Transform")?;
    tf.translation = at;
    let who = world.get::<AiId>(entity).map_or_else(|| format!("{entity}"), ToString::to_string);
    match tile {
        Some(tile) => Ok(format!("{who} at tile ({x}, {y}) {} rot {}: {}", tile.tile, tile.rot, fmt_pos(at))),
        None => Ok(format!("{who} at chunk ({x}, {y}): {}", fmt_pos(at))),
    }
}

/// Generate a region for many seeds without building any and report: how many passed, why
/// attempts failed (with example seeds), distances between points of interest, generator
/// panics with their seed. The first line starts with `ok:` when every seed passed.
#[derive(Deserialize, JsonSchema)]
pub struct WorldgenReportArgs {
    /// A region content id of any kind (`region`, `dungeon`, `voxel_region`, ...).
    pub region: String,
    /// `1..1000` (both ends included), `7`, or `1,5,10..20`. Default 1..100.
    pub seeds: Option<SeedsArg>,
}

/// Seeds as text, or one seed as a number (`seeds=7` reads as one).
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SeedsArg {
    One(u64),
    Text(String),
}

fn worldgen_report(In(call): In<Call<WorldgenReportArgs>>, world: &mut World) -> ToolResult {
    let worldgen = world.resource::<WorldGen>().clone();
    let seeds = match call.args.seeds {
        Some(SeedsArg::One(seed)) => seed.to_string(),
        Some(SeedsArg::Text(text)) => text,
        None => "1..100".into(),
    };
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
    let cells = match m.layout {
        Layout::Grid | Layout::Rooms => "tiles",
        Layout::Volume => "chunks",
    };
    let mut out = serde_json::json!({
        "region": m.title(),
        "made": origin,
        "size": format!("{}×{} {cells} of {}m", m.size[0], m.size[1], m.tile_size),
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
