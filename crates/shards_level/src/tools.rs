//! What agents can do with levels: list them and read their markers, spawn one, replace
//! the spawned ones after the file changed.

use ai_core::journal::{self, JournalKind};
use ai_core::prelude::*;
use ai_core::spatial::fmt_pos;
use ai_core::tools::{register_tool, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::db::closest;
use shards_content::ContentDb;
use shards_motion::{Collider, Volume};

use crate::def::LevelDef;
use crate::library::level_asset;
use crate::marker::{LevelMarker, MarkerKind};
use crate::props::check_level;
use crate::spawn::{reload_level, spawn_level_as, FromLevel, LevelInstances, SpawnedLevel};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "level_info", Scope::Player, ToolKind::Query, level_info);
    register_tool(world, "spawn_level", Scope::Dev, ToolKind::Action, spawn_level_tool);
    register_tool(world, "level_reload", Scope::Dev, ToolKind::Action, level_reload);
}

/// `spawn "start"`, `navlink "ladder" -> (11.0, 3.0, 5.5)`.
fn marker_text(marker: &LevelMarker) -> String {
    match marker.to {
        Some(to) => format!("{} \"{}\" -> {}", marker.kind, marker.name, fmt_pos(to)),
        None => format!("{} \"{}\"", marker.kind, marker.name),
    }
}

/// The marker's part of an entity's line in `observe`, unless the line says it already:
/// a spawn point's `SpawnPoint "start"` or a landmark's `Shrine` need no `spawn "start"`.
pub(crate) fn describe_marker(entity: EntityRef) -> Option<String> {
    let marker = entity.get::<LevelMarker>()?;
    let said = if marker.kind == MarkerKind::Landmark { marker.name.clone() } else { marker.kind.label() };
    if entity.get::<AiKind>().is_some_and(|kind| kind.0 == said.as_str()) {
        return None;
    }
    Some(marker_text(marker))
}

/// Level `id` of the content database, or an error that lists the levels there are.
pub(crate) fn def_of(world: &World, id: &str) -> Result<LevelDef, String> {
    let db = world.resource::<ContentDb>();
    if !db.is_loaded() {
        return Err("content is still loading; wait with `shards wait`".into());
    }
    if let Some(def) = db.get::<LevelDef>(id) {
        return Ok(def.clone());
    }
    let ids: Vec<&str> = db.ids::<LevelDef>().collect();
    let hint = closest(id, ids.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    let known = if ids.is_empty() {
        "there are none: add a content/**/*.level.ron file".to_owned()
    } else {
        format!("levels: {}", ids.join(", "))
    };
    Err(format!("no level `{id}`{hint}; {known}"))
}

/// Spawns the level `id` of the content database: its file loaded, its gameplay nodes
/// made entities. `at` of the definition unless a position is given. The same file can be
/// spawned again (each spawn is an instance).
pub fn spawn_level_by_id(world: &mut World, id: &str, at: Option<Transform>) -> Result<SpawnedLevel, String> {
    let def = def_of(world, id)?;
    let at = at.unwrap_or_else(|| Transform::from_translation(def.at));
    spawn_level_file(world, &def.file, at, id)
}

/// Spawns the already loaded level file `path` (see [`request_level`]) under `label`.
/// For games that place level files the content does not list: the tiles of a region.
pub fn spawn_level_file(world: &mut World, path: &str, at: Transform, label: &str) -> Result<SpawnedLevel, String> {
    let level = level_asset(world, path)?.clone();
    spawn_level_as(world, &level, at, label)
}

/// Spawn a level (content kind `level`, a glTF file from Blender): its objects become
/// colliders, volumes, markers and the components they name. Fails without spawning
/// anything when the file has problems; the error lists them. `level_info` lists levels.
#[derive(Deserialize, JsonSchema)]
pub struct SpawnLevelArgs {
    /// Level id, e.g. "arena".
    pub id: String,
    /// East (+X) in meters. Default: the definition's `at`.
    pub x: Option<f32>,
    /// Height. Default: the definition's `at`.
    pub y: Option<f32>,
    /// South (+Z) in meters. Default: the definition's `at`.
    pub z: Option<f32>,
}

fn spawn_level_tool(In(call): In<Call<SpawnLevelArgs>>, world: &mut World) -> ToolResult {
    let SpawnLevelArgs { id, x, y, z } = call.args;
    let def = def_of(world, &id)?;
    let at = Vec3::new(x.unwrap_or(def.at.x), y.unwrap_or(def.at.y), z.unwrap_or(def.at.z));
    let spawned = spawn_level_by_id(world, &id, Some(Transform::from_translation(at)))?;
    Ok(summarize(world, &id, at, &spawned))
}

/// `spawned level `arena` as instance 1 at (0.0, 0.0): 18 entities (7 solids, 3 volumes, 9 markers)`
/// and a line per marker.
fn summarize(world: &World, id: &str, at: Vec3, spawned: &SpawnedLevel) -> String {
    let (mut colliders, mut volumes) = (0, 0);
    let mut markers = Vec::new();
    for &entity in &spawned.entities {
        let Ok(entity) = world.get_entity(entity) else { continue };
        colliders += usize::from(entity.contains::<Collider>() && !entity.contains::<Volume>());
        volumes += usize::from(entity.contains::<Volume>());
        if let Some(marker) = entity.get::<LevelMarker>() {
            let pos = entity.get::<Transform>().map_or(Vec3::ZERO, |t| t.translation);
            markers.push(format!("  {} at {}", marker_text(marker), fmt_pos(pos)));
        }
    }
    let mut out = format!(
        "spawned level `{id}` as instance {} at {}: {} entities ({colliders} solids, {volumes} volumes, {} markers)",
        spawned.instance,
        fmt_pos(at),
        spawned.entities.len(),
        markers.len()
    );
    const SHOWN: usize = 12;
    for line in markers.iter().take(SHOWN) {
        out.push('\n');
        out.push_str(line);
    }
    if markers.len() > SHOWN {
        out.push_str(&format!("\n  … {} more markers (level_info lists them)", markers.len() - SHOWN));
    }
    out
}

/// What the levels of the game are: without `id`, every level with its file and how often it
/// is spawned; with `id`, its markers, whether the markers it requires are there, the
/// problems the check finds in the file, and the spawned instances.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LevelInfoArgs {
    /// Level id, e.g. "arena".
    pub id: Option<String>,
}

fn level_info(In(call): In<Call<LevelInfoArgs>>, world: &World) -> ToolResult {
    let instances = world.get_resource::<LevelInstances>();
    let Some(id) = call.args.id else {
        let db = world.resource::<ContentDb>();
        if !db.is_loaded() {
            return Err("content is still loading; wait with `shards wait`".into());
        }
        let ids: Vec<&str> = db.ids::<LevelDef>().collect();
        let mut out = format!("levels: {}", ids.len());
        for id in ids {
            let spawned = instances.map_or(0, |i| i.iter().filter(|i| i.label == id).count());
            let file = db.get::<LevelDef>(id).map(|d| d.file.as_str()).unwrap_or_default();
            out.push_str(&format!("\n  {id}  {file}  {} spawned", spawned));
        }
        return Ok(out);
    };
    let def = def_of(world, &id)?;
    let mut out = format!("level {id}: {}, at {}", def.file, fmt_pos(def.at));
    let level = match level_asset(world, &def.file) {
        Ok(level) => level,
        Err(why) => return Ok(format!("{out}\n{why}")),
    };
    out.push_str(&format!("\n{} nodes", level.nodes.len()));
    let markers = level.markers();
    out.push_str(&format!(", {} markers", markers.len()));
    for marker in &markers {
        out.push_str(&format!("\n  {} \"{}\" at {}  {}", marker.kind, marker.name, fmt_pos(marker.position), marker.node));
    }
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let problems = check_level(level, &registry, Some(world.resource::<ContentDb>()), &def.required_markers);
    if problems.is_empty() {
        out.push_str("\nchecks: ok");
    } else {
        let count = if problems.len() == 1 { "1 problem".to_owned() } else { format!("{} problems", problems.len()) };
        out.push_str(&format!("\nchecks: {count}"));
        for problem in &problems {
            out.push_str(&format!("\n  {problem}"));
        }
    }
    for instance in instances.into_iter().flat_map(|i| i.iter()).filter(|i| i.label == id) {
        let entities = world.iter_entities().filter(|e| e.get::<FromLevel>().is_some_and(|f| f.instance == instance.id)).count();
        out.push_str(&format!("\ninstance {}: {entities} entities at {}", instance.id, fmt_pos(instance.at.translation)));
    }
    Ok(out)
}

/// Replace the spawned instances of a level with the file as it is now (the asset server
/// reloads a changed file by itself, and the instances follow through this tool, so a replay
/// has it). Entities the game changed come back as the file says. A file with problems
/// leaves the instances as they were. Without arguments: every spawned level.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LevelReloadArgs {
    /// Level id, e.g. "arena".
    pub id: Option<String>,
    /// Or the asset path of the file, e.g. "levels/arena.gltf".
    pub file: Option<String>,
}

fn level_reload(In(call): In<Call<LevelReloadArgs>>, world: &mut World) -> ToolResult {
    let LevelReloadArgs { id, file } = call.args;
    let file_of_id = match &id {
        Some(id) => Some(def_of(world, id)?.file),
        None => None,
    };
    let mut wanted: Vec<(u32, String)> = world
        .get_resource::<LevelInstances>()
        .into_iter()
        .flat_map(|instances| instances.iter())
        .filter(|i| {
            // An instance counts by the id it was spawned under or by its file.
            let by_id = id.as_ref().is_some_and(|id| i.label == *id) || file_of_id.as_ref().is_some_and(|f| i.path == *f);
            let by_file = file.as_ref().is_some_and(|f| i.path == *f);
            (id.is_none() && file.is_none()) || by_id || by_file
        })
        .map(|i| (i.id, i.label.clone()))
        .collect();
    wanted.sort();
    if wanted.is_empty() {
        let spawned: Vec<String> =
            world.get_resource::<LevelInstances>().into_iter().flat_map(|i| i.iter()).map(|i| format!("{} (instance {})", i.label, i.id)).collect();
        let list = if spawned.is_empty() { "no level is spawned (spawn_level)".to_owned() } else { format!("spawned: {}", spawned.join(", ")) };
        return Err(format!("no spawned level matches; {list}"));
    }
    let mut done = Vec::new();
    for (instance, label) in wanted {
        let entities = reload_level(world, instance).map_err(|why| format!("{label} (instance {instance}) kept as it was: {why}"))?;
        done.push(format!("{label} (instance {instance}, {entities} entities)"));
    }
    let text = format!("level reloaded: {}", done.join(", "));
    journal::emit(world, JournalKind::Event, text.clone());
    Ok(text)
}
