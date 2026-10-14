//! Navigation tools: `nav_bake` (dev: bake files), `nav_overlay` (dev: draw the navmesh),
//! `nav_info` (the navmesh of the region); and [`bake_files`], what `--bake-nav` runs.

use std::fmt;
use std::sync::Arc;

use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_content::db::closest;
use shards_content::{asset_root, ContentDb};

use super::bake::bake;
use super::region::{refresh, NavBakes, NavRegion};
use super::settings::{bake_targets, file_name, NavSettings};

pub(crate) fn register(world: &mut World) {
    world.init_resource::<NavOverlay>();
    register_tool(world, "nav_bake", Scope::Dev, ToolKind::Control, nav_bake);
    register_tool(world, "nav_overlay", Scope::Dev, ToolKind::Query, nav_overlay);
    register_tool(world, "nav_info", Scope::Dev, ToolKind::Query, nav_info);
}

/// What [`bake_files`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BakeReport {
    /// The folder, relative to the assets: `nav`.
    pub dir: String,
    /// `(id, file, summary)` of every file written.
    pub written: Vec<(String, String, String)>,
    /// `(id, why)` of the tiles that could not be baked.
    pub failed: Vec<(String, String)>,
}

impl BakeReport {
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

impl fmt::Display for BakeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.written.len();
        write!(f, "baked {n} {} into {}/", if n == 1 { "tile" } else { "tiles" }, self.dir)?;
        for (id, file, summary) in &self.written {
            write!(f, "\n  {id}: {file} ({summary})")?;
        }
        for (id, why) in &self.failed {
            write!(f, "\n  {id}: FAILED: {why}")?;
        }
        Ok(())
    }
}

/// Bakes tiles and rooms into their bake files under the asset folder: `only` names ids
/// (comma separated), `None` or `all` every tile a region uses and every room a dungeon
/// uses, each at the first tile size its regions have. An unknown id is an error that names
/// the known ones.
pub fn bake_files(world: &mut World, only: Option<&str>) -> Result<BakeReport, String> {
    let db = world.get_resource::<ContentDb>().ok_or("content is not loaded")?;
    if !db.is_loaded() {
        return Err("content is still loading; wait with `shards wait`".into());
    }
    let settings = NavSettings::of(db);
    let targets = bake_targets(db);
    let wanted: Option<Vec<String>> = only.filter(|o| o.trim() != "all").map(|o| o.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect());
    if let Some(ids) = &wanted {
        for id in ids {
            if !targets.iter().any(|t| &t.id == id) {
                let known = db.ids::<shards_world::TileDef>().any(|t| t == id) || db.ids::<shards_world::RoomDef>().any(|t| t == id);
                if known {
                    return Err(format!("`{id}` is in no region (or dungeon): there is no tile size to bake it at; add it to one"));
                }
                let hint = closest(id, targets.iter().map(|t| t.id.as_str())).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                return Err(format!("no tile or room `{id}` to bake{hint}; `nav_bake` without `tile` bakes them all"));
            }
        }
    }
    let root = asset_root(world);
    let mut report = BakeReport { dir: settings.dir.clone(), ..default() };
    for target in targets.into_iter().filter(|t| wanted.as_ref().is_none_or(|w| w.contains(&t.id))) {
        let size = target.sizes[0];
        match bake(world, Some(&root), target.kind, &target.id, size, &settings) {
            Ok(tile) => {
                let path = settings.file(&root, target.kind, &target.id);
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
                }
                std::fs::write(&path, tile.to_file()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
                let summary = tile.summary();
                let hash = tile.hash.clone();
                world.get_resource_or_init::<NavBakes>().tiles.insert(hash, Arc::new(tile));
                report.written.push((target.id.clone(), file_name(target.kind, &target.id), summary));
            }
            Err(why) => report.failed.push((target.id.clone(), why)),
        }
    }
    Ok(report)
}

/// Bake tiles into their navigation bake files (`<assets>/nav/<tile>.navtile.ron`), as
/// `--bake-nav` does: one tile or room, or every tile a region uses. A region loaded after
/// reads them; the one in the world keeps its navmesh.
#[derive(Deserialize, JsonSchema, Default)]
pub struct NavBakeArgs {
    /// A tile or room id, or several separated by commas. Default: all.
    pub tile: Option<String>,
}

fn nav_bake(In(call): In<Call<NavBakeArgs>>, world: &mut World) -> ToolResult {
    let report = bake_files(world, call.args.tile.as_deref())?;
    if report.ok() { Ok(report.to_string()) } else { Err(report.to_string()) }
}

/// Whether the navigation overlay is drawn.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct NavOverlay {
    pub enabled: bool,
}

/// Draw the navmesh in the game window: walkable cells (green, water blue, carved red),
/// links across tile edges, navlinks, the routes NPCs follow; and the tile bounds and
/// sockets of the world overlay.
#[derive(Deserialize, JsonSchema)]
pub struct NavOverlayArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn nav_overlay(In(call): In<Call<NavOverlayArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    world.resource_mut::<NavOverlay>().enabled = on;
    #[cfg(feature = "render")]
    if let Some(mut tiles) = world.get_resource_mut::<shards_world::TileOverlay>() {
        tiles.enabled = on;
    }
    Ok(format!("nav overlay {}", if on { "on" } else { "off" }))
}

/// The navmesh of the region in the world: its tiles, nodes, links across tile edges,
/// navlinks, carved cells, and which tiles were baked while loading. Read-only.
fn nav_info(In(_): In<Call<NoArgs>>, world: &mut World) -> ToolResult<Value> {
    refresh(world);
    let Some(nav) = world.get_resource::<NavRegion>() else {
        return Err("no navmesh: no region is loaded (NPCs and `move_to path=true` plan on the local grid); load one with `regen`".into());
    };
    let links: Vec<Value> = nav
        .navlinks
        .iter()
        .map(|l| {
            let mut link = json!({
                "name": l.name,
                "from": ai_core::spatial::fmt_pos(l.from),
                "to": ai_core::spatial::fmt_pos(l.to),
                "used": l.nodes.is_some(),
            });
            if let Some(why) = &l.unused {
                link["why"] = json!(why);
            }
            link
        })
        .collect();
    let r = &nav.report;
    Ok(json!({
        "region": nav.title,
        "summary": nav.summary(),
        "cell": nav.cell,
        "agent": {"radius": nav.agent_radius, "height": nav.agent_height},
        "from_files": r.from_files,
        "baked_while_loading": r.missing.iter().chain(&r.stale).collect::<Vec<_>>(),
        "failed": r.failed.iter().map(|(t, why)| format!("{t}: {why}")).collect::<Vec<_>>(),
        "closed_edges": r.closed_edges,
        "carved_cells": r.carved,
        "navlinks": links,
    }))
}
