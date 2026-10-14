//! Level files as tiles: `shards_level` is the loader behind `shards_world`'s
//! `geometry: Level(path: ...)`.
//!
//! The two crates do not know each other (a game can use either alone), so the umbrella
//! joins them: [`TileLevelsPlugin`], part of [`EnginePlugins`](crate::EnginePlugins), registers
//! a [`LevelSpawner`] that is made of `shards_level`'s own calls. A game on `EnginePlugins`
//! needs no setup.
//!
//! ```text
//! content/world/ruin.tile.ron                       tiles/ruin.gltf (Blender)
//!   "ruin": (sockets: (..), slots: [..],      ──►     Floor      collider = auto
//!            geometry: Level(path:                    Wall.001   collider = box, size = [1, 3, 6]
//!              "tiles/ruin.gltf"))                    Start      marker = spawn, name = start
//!                                                     Edge.N     socket = road
//! ```
//!
//! ## What a level tile is
//!
//! - **The file is in the tile's frame**: its origin is the tile's center, -Z is north, +Y is
//!   up, a meter is a meter (a 16 m tile is a 16 × 16 m floor). The tile's rotation turns the
//!   whole file by quarter turns clockwise, the same turn inline pieces get.
//! - **Nodes become entities as in any level** (see `shards_level`: colliders, volumes,
//!   markers, prefabs, components by name), in file order, each a **top-level entity with a
//!   world-space transform** (the tile's transform ∘ the node's), tagged with the tile's
//!   [`TileId`](shards_world::TileId) and [`RegionPart`](shards_world::RegionPart). They
//!   despawn with the region and get the same `AiId`s and `ColliderId`s in every run.
//! - **Markers are the tile's markers**: a node with a `marker` (or `socket`) property is
//!   also a [`Marker`](shards_world::Marker) named like the marker (`start`, the socket type
//!   `road`, ...), the way inline `Marker` pieces are, next to its
//!   [`LevelMarker`](shards_level::LevelMarker) (kind, `to` of a navlink) and what the AI
//!   sees of it (a spawn point, a camp).
//! - **What the generator matches stays in content.** The solver reads `sockets`, `slots`,
//!   `pois`, `links` from the tile's RON, never the file: generation is a pure function of
//!   seed, version and content, and runs without assets (`--worldgen-report`). The `socket`
//!   and slot markers an artist puts in the file are placed in the world for the game
//!   (connectors, props at the edges), not read back by the solver.
//! - **Visuals** (the `render` feature): one scene of the file per tile, under a root of its
//!   own that carries no gameplay, like any level.
//!
//! ## Loading
//!
//! A region change spawns synchronously inside a tool call, so it can never wait for a file.
//! Instead, once content is built, every file the content can use (`region::level_paths`) is
//! requested, and the loading gate (`tile levels`) holds the simulation and the start region
//! until all are in. A file that cannot be loaded stops loading and the journal says which
//! tiles are built from it. Before a region changes anything, every level file it uses is
//! checked (nodes, components, prefabs): a problem refuses the change and names the tile and
//! the file, and the old region stays. Content that names a file the asset folder lacks is
//! an error at `content check`, naming the tile and the path. What is wrong *inside* a file
//! is no content error (`content check` does not read the nodes of tile files; list a file
//! as a content `level` too and it does): the journal gets an error line for it, naming
//! the tiles, as soon as the file loads.
//!
//! When a file changes on disk (hot reload) `shards_level` replaces its instances; the
//! entities that come back are tagged again as they are made, inside the reload (a replay
//! plays without frames, so a frame later is too late).

use std::collections::BTreeMap;

use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::LoadStatus;
use shards_level::{
    check_level, despawn_level, level_asset, request_level, spawn_level_file, FromLevel, LevelInstances, LevelLibrary, LevelMarker, LevelPlugin,
};
use shards_world::{LevelSpawn, LevelSpawner, Marker, RegionPart, TileId, WorldAppExt, WorldPlugin};

/// Joins `shards_level` to `shards_world`: level files as the geometry of tiles and rooms.
/// Needs both plugins added first (`EnginePlugins` does it in order).
pub struct TileLevelsPlugin;

impl Plugin for TileLevelsPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<LevelPlugin>() && app.is_plugin_added::<WorldPlugin>(),
            "TileLevelsPlugin needs LevelPlugin and WorldPlugin added first (shards::EnginePlugins adds all three)"
        );
        app.init_resource::<TileLevels>()
            .set_level_spawner(LevelSpawner { spawn, request, status, check, forget, next_instance, restart_instances })
            .add_observer(retag_level_entity)
            .add_observer(retag_level_marker);
    }
}

/// The level instances that stand for tiles, and the tile each is.
#[derive(Resource, Default, Debug)]
struct TileLevels {
    instances: BTreeMap<u32, TileId>,
}

fn next_instance(world: &World) -> u32 {
    world.get_resource::<LevelInstances>().map_or(1, LevelInstances::peek_next)
}

fn restart_instances(world: &mut World, next: u32) {
    if let Some(mut instances) = world.get_resource_mut::<LevelInstances>() {
        instances.restart_at(next);
    }
}

fn request(world: &mut World, path: &str) {
    request_level(world, path);
}

fn status(world: &World, path: &str) -> LoadStatus {
    world.resource::<LevelLibrary>().status(world, path)
}

/// Everything `shards_level` finds wrong with the file, as one error.
fn check(world: &World, path: &str) -> Result<(), String> {
    let level = level_asset(world, path)?;
    let registry = world.resource::<AppTypeRegistry>().read();
    let problems = check_level(level, &registry, world.get_resource::<ContentDb>(), &[]);
    if problems.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = problems.iter().map(|p| format!("  {p}")).collect();
    let count = if problems.len() == 1 { "1 problem".to_owned() } else { format!("{} problems", problems.len()) };
    Err(format!("{count}:\n{}", lines.join("\n")))
}

fn spawn(world: &mut World, request: &LevelSpawn) -> Result<Vec<Entity>, String> {
    let spawned = spawn_level_file(world, request.path, request.transform, request.path)?;
    world.resource_mut::<TileLevels>().instances.insert(spawned.instance, request.tile);
    for &entity in &spawned.entities {
        mark(world, entity);
    }
    Ok(spawned.entities)
}

/// A node with a marker is a [`Marker`] of the tile too.
fn mark(world: &mut World, entity: Entity) {
    if let Some(name) = world.get::<LevelMarker>(entity).map(|m| m.name.clone()) {
        world.entity_mut(entity).insert(Marker { name });
    }
}

/// The region is about to despawn `doomed`: the level entities go in their order, then every
/// instance the region's tiles made (and its visual scene) is dropped, those whose file spawned
/// no entity to find them by included. A region owns all of them.
fn forget(world: &mut World, doomed: &[Entity]) {
    for &entity in doomed {
        if world.get::<FromLevel>(entity).is_some() {
            world.despawn(entity);
        }
    }
    for instance in std::mem::take(&mut world.resource_mut::<TileLevels>().instances).into_keys() {
        despawn_level(world, instance);
    }
}

/// An entity of a tile's level that came back from a reload (`level_reload` replaces the
/// instance's entities) is tagged like the first spawn made it, as it is made: tags are part
/// of the replay hash, and a region change finds its entities by them. A first spawn is
/// tagged by [`spawn`], after the instance is known.
fn retag(entity: Entity, levels: &Query<(&FromLevel, Option<&LevelMarker>)>, tiles: &TileLevels, commands: &mut Commands) {
    let Ok((from, marker)) = levels.get(entity) else { return };
    let Some(&tile) = tiles.instances.get(&from.instance) else { return };
    let mut tagged = commands.entity(entity);
    tagged.insert((tile, RegionPart));
    if let Some(marker) = marker {
        tagged.insert(Marker { name: marker.name.clone() });
    }
}

fn retag_level_entity(add: On<Add<FromLevel>>, levels: Query<(&FromLevel, Option<&LevelMarker>)>, tiles: Res<TileLevels>, mut commands: Commands) {
    retag(add.entity, &levels, &tiles, &mut commands);
}

/// The marker comes after [`FromLevel`] in the node, so a marker needs its own look.
fn retag_level_marker(add: On<Add<LevelMarker>>, levels: Query<(&FromLevel, Option<&LevelMarker>)>, tiles: Res<TileLevels>, mut commands: Commands) {
    retag(add.entity, &levels, &tiles, &mut commands);
}
