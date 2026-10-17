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
//!   (connectors, props at the edges), not read back by the solver. Content's check holds the
//!   two together: a socket marker stands on an edge of the tile, at a part of it where the
//!   tile's `sockets` have its kind (`socket marker `road` (node `Road.E`) is on the east edge,
//!   where sockets.e has `grass``), at the size of every region that uses the tile.
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
//! an error at `content check`, naming the tile and the path. So is what is wrong *inside* a
//! file: `content check` reads a tile's file as it reads a `level` definition's (the file must
//! read, its nodes must make sense), and each problem is an error of the tile that names the
//! file and the node (`tile `ruin`: geometry: tiles/ruin.gltf: node `Pillar`: unknown component
//! `Helth` (did you mean `Health`?)`). A file that is also a content `level` is reported by
//! the `level` alone. A file that breaks after content was checked (edited on disk while the
//! game runs) still gets the journal's error line, naming the tiles, as soon as it loads.
//!
//! When a file changes on disk (hot reload) `shards_level` replaces its instances; the
//! entities that come back are tagged again as they are made, inside the reload (a replay
//! plays without frames, so a frame later is too late).

use std::collections::BTreeMap;
use std::path::{Component, Path};

use bevy::prelude::*;
use shards_content::{ContentAppExt, ContentDb, Refs};
use shards_core::LoadStatus;
use shards_level::{
    check_level, despawn_level, level_asset, request_level, spawn_level_file, FromLevel, LevelAsset, LevelCounter, LevelDef, LevelLibrary,
    LevelMarker, LevelPlugin, MarkerKind,
};
use shards_world::content::{RegionDef, TileDef, TileGeometry};
use shards_world::sockets::parse_edge;
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
            .add_content_check::<TileDef>("level file", check_tile_file)
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
    world.get_resource::<LevelCounter>().map_or(1, LevelCounter::peek_next)
}

fn restart_instances(world: &mut World, next: u32) {
    if let Some(mut counter) = world.get_resource_mut::<LevelCounter>() {
        counter.restart_at(next);
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

// --- the content check of a tile's file -----------------------------------------------------

/// How far from an edge a socket marker may stand and still be on it, as a share of the tile's
/// size (1.6 m of a 16 m tile): an artist puts it on the edge, or just inside.
const ON_EDGE: f32 = 0.1;

/// The edges in the order of `sockets`, and their fields.
const EDGES: [(&str, &str); 4] = [("north", "n"), ("east", "e"), ("south", "s"), ("west", "w")];

/// Content's check of a tile built from a level file (when content loads, on a hot edit, at
/// `content check`). The file is read once and checked the way a `level` definition's file is:
/// it must read, and every problem `shards_level` finds in its nodes (a component that does not
/// exist, a prefab that is missing, a navlink that ends nowhere, ...) is an error of the tile
/// that names the file and the node. Then the file's `socket` markers against the tile's
/// `sockets`, at every size a region uses the tile at: each marker stands on an edge, at a part
/// of it whose socket is of the marker's kind (and height, when the marker names one). Markers
/// are optional: a socket the file does not mark is not checked. And the file's `navlink`
/// markers against the tile's `navlinks`: the generator's reachability check takes a navlink
/// only as the tile lists it, so each one the file has is listed, under a name of its own, and
/// each one listed is in the file.
///
/// A file that a `level` definition names too is that definition's to report: it checks the same
/// things (and its `required_markers`), under its own name, and one problem of one file must
/// not be two errors. The tile then adds only what is its own, the sockets and the navlinks.
fn check_tile_file(tile: &TileDef, id: &str, refs: &mut Refs) {
    let TileGeometry::Level { path } = &tile.geometry else { return };
    let Some(root) = refs.asset_root() else { return };
    // The tile's own check names a path out of the asset folder and a file that is not there.
    if !Path::new(path).components().all(|c| matches!(c, Component::Normal(_))) || !root.join(path).is_file() {
        return;
    }
    // A `level` definition reads its file only with a type registry: without one (a bare
    // `ContentDb::build`), the tile says what the file is like, and nobody says it twice.
    let by_level = refs.type_registry().is_some() && refs.iter::<LevelDef>().any(|(_, def)| Path::new(&def.file) == Path::new(path));
    let level = match LevelAsset::read(root, path) {
        Ok(level) => level,
        // The reason names the file already: `tiles/ruin.gltf`: ...
        Err(why) => {
            if !by_level {
                refs.error(format!("geometry: {why}"));
            }
            return;
        }
    };
    // Without a type registry components cannot be told from typos.
    if let Some(registry) = refs.type_registry()
        && !by_level
    {
        for problem in check_level(&level, registry, Some(refs.db()), &[]) {
            refs.error(format!("geometry: {path}: {problem}"));
        }
    }
    for problem in socket_problems(tile, id, &level, refs).into_iter().chain(navlink_problems(tile, &level)) {
        refs.error(format!("geometry: {path}: {problem}"));
    }
}

/// What does not match between the file's navlink markers and the tile's `navlinks`, once each.
fn navlink_problems(tile: &TileDef, level: &LevelAsset) -> Vec<String> {
    let markers: Vec<_> = level.markers().into_iter().filter(|m| m.kind == MarkerKind::NavLink).collect();
    let mut problems = Vec::new();
    for (i, marker) in markers.iter().enumerate() {
        // A name the file uses again is said once, at its second marker.
        let earlier: Vec<_> = markers[..i].iter().filter(|m| m.name == marker.name).collect();
        if let [first] = earlier[..] {
            problems.push(format!(
                "two navlinks are called `{}` (nodes `{}` and `{}`): give one a `name` property, so the tile's navlinks tell them apart",
                marker.name, first.node, marker.node
            ));
        } else if earlier.is_empty() && !tile.navlinks.iter().any(|l| l.name == marker.name) {
            problems.push(format!(
                "navlink `{}` (node `{}`) is not in the tile's navlinks, so the generator's reachability check does not know it: list it with the links groups it starts and ends in, e.g. (name: \"{}\", from: 0, to: 1)",
                marker.name, marker.node, marker.name
            ));
        }
    }
    for link in &tile.navlinks {
        if !markers.iter().any(|m| m.name == link.name) {
            problems.push(format!("navlinks: `{}` is no navlink marker of the file", link.name));
        }
    }
    problems
}

/// What does not match between the file's socket markers and the tile's `sockets`, once each.
fn socket_problems(tile: &TileDef, id: &str, level: &LevelAsset, refs: &Refs) -> Vec<String> {
    let markers: Vec<_> = level.markers().into_iter().filter(|m| m.kind == MarkerKind::Socket).collect();
    if markers.is_empty() {
        return Vec::new();
    }
    // The tile's own check reports edges that do not parse.
    let Ok(edges) = tile.sockets.parse() else { return Vec::new() };
    let mut problems = Vec::new();
    let mut say = |problem: String| {
        if !problems.contains(&problem) {
            problems.push(problem);
        }
    };
    let mut sockets = Vec::new();
    for marker in &markers {
        match parse_edge(&marker.name) {
            Ok(parts) if parts.len() == 1 => sockets.push((marker, parts[0].clone())),
            _ => say(format!("socket marker `{}` (node `{}`) names no socket: give one kind, like `socket = road` or `road@1`", marker.name, marker.node)),
        }
    }
    // Sizes only regions give: a tile in none has no edges to stand on yet.
    let tiles: Vec<(&str, &TileDef)> = refs.iter::<TileDef>().collect();
    let mut sizes: Vec<(f32, &str)> = Vec::new();
    for (region_id, region) in refs.iter::<RegionDef>() {
        let uses = region.tile_ids(tiles.iter().copied()).is_ok_and(|ids| ids.contains(&id));
        if uses && !sizes.iter().any(|(size, _)| *size == region.tile_size) {
            sizes.push((region.tile_size, region_id));
        }
    }
    for (size, region) in sizes {
        let half = size / 2.0;
        for (marker, socket) in &sockets {
            let p = marker.position;
            let off = [(p.z + half).abs(), (p.x - half).abs(), (p.z - half).abs(), (p.x + half).abs()];
            let edge = (0..4).min_by(|a, b| off[*a].total_cmp(&off[*b])).unwrap_or(0);
            if off[edge] > ON_EDGE * size {
                say(format!(
                    "socket marker `{}` (node `{}`) at x {:.1}, z {:.1} is on no edge: the {size} m tiles of region `{region}` have them at x and z = ±{half}",
                    marker.name, marker.node, p.x, p.z
                ));
                continue;
            }
            // Clockwise along the edge, as `sockets` lists its parts.
            let along = [p.x + half, p.z + half, half - p.x, half - p.z][edge] / size;
            let parts = &edges[edge];
            let part = ((along * parts.len() as f32).max(0.0) as usize).min(parts.len() - 1);
            let want = &parts[part];
            // A marker without a height matches the part at any height.
            if want.kind != socket.kind || (marker.name.contains('@') && want.height != socket.height) {
                let (name, field) = EDGES[edge];
                say(format!(
                    "socket marker `{}` (node `{}`) is on the {name} edge, where sockets.{field} has `{want}`{}",
                    marker.name,
                    marker.node,
                    part_of(part, parts.len())
                ));
            }
        }
    }
    problems
}

/// ` (part 2 of 3)`, or nothing for an edge of one part.
fn part_of(part: usize, parts: usize) -> String {
    if parts == 1 { String::new() } else { format!(" (part {} of {parts})", part + 1) }
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

#[cfg(test)]
mod tests {
    use bevy::ecs::reflect::AppTypeRegistry;
    use shards_content::db::Source;
    use shards_content::ContentKinds;

    use super::*;

    /// The errors of a tile and a `level` that name one file that does not read, with or without
    /// a type registry.
    fn errors(registry: bool) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!("shards_tile_levels_{}_{registry}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tiles")).unwrap();
        std::fs::write(dir.join("tiles/bad.gltf"), "not a glTF").unwrap();
        let mut kinds = ContentKinds::default();
        kinds.register::<TileDef>();
        kinds.register::<LevelDef>();
        kinds.add_check::<TileDef>("level file", check_tile_file);
        kinds.set_asset_root(dir.clone());
        if registry {
            kinds.set_type_registry(AppTypeRegistry::default());
        }
        let sources = vec![
            Source {
                path: "content/t.tile.ron".into(),
                text: r#"{ "bad": (sockets: (n: "a", e: "a", s: "a", w: "a"), geometry: Level(path: "tiles/bad.gltf")) }"#.into(),
            },
            Source { path: "content/l.level.ron".into(), text: r#"{ "bad": (file: "tiles/bad.gltf") }"#.into() },
        ];
        let errors = ContentDb::build(&kinds, &sources).errors().map(ToString::to_string).collect();
        let _ = std::fs::remove_dir_all(&dir);
        errors
    }

    #[test]
    fn a_file_that_does_not_read_is_said_once_by_whoever_reads_it() {
        // With a registry the `level` reads its file and says so; the tile keeps quiet.
        let with = errors(true);
        assert!(with.len() == 1 && with[0].starts_with("content/l.level.ron: level `bad`: `tiles/bad.gltf`: "), "{with:#?}");
        // Without one the `level` does not read it, so the tile does.
        let without = errors(false);
        assert!(without.len() == 1 && without[0].starts_with("content/t.tile.ron: tile `bad`: geometry: `tiles/bad.gltf`: "), "{without:#?}");
    }
}
