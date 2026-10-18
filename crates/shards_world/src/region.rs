//! Regions in the world: building one from its manifest, replacing it, the start region.
//!
//! [`spawn_region`] never runs the generator: it builds what a manifest says, tile by tile
//! in row-major order (rooms by their north-west cell), each tile's pieces in file order,
//! then the points of interest. Everything gameplay touches is a top-level entity with a
//! world-space transform (tile center and turn ∘ piece), tagged [`TileId`] and
//! [`RegionPart`], so the same manifest gives the same `AiId`s and `ColliderId`s every run,
//! with a window or without.
//!
//! Tiles whose geometry is a level file (`Level { path }`) are spawned by the level loader
//! ([`LevelSpawner`]): the nodes of the file, at the tile's transform, tagged like the rest.
//! The loader loads every file the content uses while the game loads, behind the loading
//! gate, so building a region never waits for a file.
//!
//! A region change ([`change_region`], the `regen` and `region_load` tools) is an action:
//! replays record and repeat it at the same tick. It passes through
//! `EngineState::RegionLoading` (the loading screen) inside the call: the old region goes,
//! the new one comes, players move to its spawn, and the state returns to `InGame` without
//! running `OnEnter(InGame)` again. When another module still loads something, the loading
//! gate holds the ticks until it is done, so a region change never shifts a tick.
//!
//! Snapshots keep the region by its [`RegionBuild`]: the manifest and the first collider id
//! and level instance its tiles took. A snapshot taken in a region builds that region again
//! where it is loaded, before it is checked ([`snapshot_prepare`]), down to the same ids; one
//! taken without a region unloads the one in the world.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::journal::{self, JournalKind};
use ai_core::{AiId, AiKind, AiLandmark, AiPlayer, AiReplay, SimGate, SimSeed, Snapshot};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::prefab::check_prefab;
use shards_content::{spawn_prefab, ContentDb};
use shards_core::{loading_statuses, seed_ready, EngineState, Intent, LoadStatus, PlayerId};
use shards_motion::{Collider, ColliderId, ColliderIds, CollisionLayers, LayerMask, PhysicsWorld, QueryFilter, Shape, Volume};

use crate::checks::RegionGraph;
use crate::content::{level_path_problem, PoiDef, RegionDef, RoomDef, TileDef, TileGeometry, TilePiece, EDGE_WALL_DEPTH, EDGE_WALL_THICKNESS};
use crate::generate::{self, Generated, WorldGen};
use crate::kind::RegionKind;
use crate::grid::{rotation, turn, turns_of};
use crate::manifest::{Layout, PlacedTile, RegionManifest};

/// The cell a region entity came from: tooling, overlays, "where is this".
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct TileId {
    pub x: i32,
    pub y: i32,
}

/// Belongs to the region: despawned when it changes.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct RegionPart;

/// One box of the invisible wall around a region's edge (`edge_wall`): a collider with nothing to draw.
/// A game that gives every collider a mesh of its shape leaves these out.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct RegionEdge;

/// The root of a placed tile or room: no collider, not AI-visible. Tile visuals and overlays
/// hang on it.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct RegionTile {
    /// `tile` or `room` id.
    pub tile: String,
    /// Degrees clockwise.
    pub rot: u16,
    /// Cells covered (rooms are larger than 1×1).
    pub size: UVec2,
    /// The visual id the tile visual spawner gets.
    pub visual: Option<String>,
}

/// A point of interest: the AI lists it under LANDMARKS (its `AiKind` is the type's label).
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Poi {
    /// `camp.1`.
    pub id: String,
    /// `poi` content id.
    pub poi: String,
}

/// A named point from a tile's geometry (spawns, NPC homes, link ends), for game code and
/// other modules.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Marker {
    pub name: String,
}

/// What a level loader gets for one tile whose geometry is `Level { path }`.
pub struct LevelSpawn<'a> {
    /// The level file, as content wrote it.
    pub path: &'a str,
    /// The tile's world transform (its center, turned): a node goes to `transform ∘ node`.
    pub transform: Transform,
    pub tile: TileId,
    /// `tile` or `room` id.
    pub tile_id: &'a str,
}

/// Spawns a level file's gameplay nodes as top-level, world-space entities and returns them
/// in spawn order (the region tags them and despawns them with it).
pub type LevelSpawnFn = fn(&mut World, &LevelSpawn) -> Result<Vec<Entity>, String>;

/// Starts loading the level file `path` (once; asking again changes nothing).
pub type LevelRequestFn = fn(&mut World, &str);

/// Whether the level file `path` is loaded, as a loading-gate answer.
pub type LevelStatusFn = fn(&World, &str) -> LoadStatus;

/// Everything wrong with the loaded level file `path` as the geometry of a tile, found
/// before anything is spawned; `Ok` when it can be spawned.
pub type LevelCheckFn = fn(&World, &str) -> Result<(), String>;

/// Called with the entities of a region (or of an attempt to build one that failed) in the
/// order they are about to be despawned, before they are: the loader drops what it keeps
/// about them (spawned instances, visual scenes).
pub type LevelForgetFn = fn(&mut World, &[Entity]);

/// The number the loader gives the next file it places (loaders number their instances).
pub type LevelNextFn = fn(&World) -> u32;

/// Makes that number `n`: a region a snapshot puts back gets the numbers it had.
pub type LevelRestartFn = fn(&mut World, u32);

/// The level loader, if a module registered one
/// ([`WorldAppExt::set_level_spawner`](crate::WorldAppExt::set_level_spawner)). The umbrella
/// crate registers `shards_level`'s (`shards::TileLevelsPlugin`, part of `EnginePlugins`), so
/// a game needs no setup.
///
/// A region is built inside the call that changes it, and that call must never wait for a
/// file. So the world module asks the loader to [`request`](Self::request) every file the
/// content can use ([`level_paths`]) as soon as content is built, holds the loading gate and
/// the start region until each file's [`status`](Self::status) is ready, and refuses a region
/// change whose files the loader's [`check`](Self::check) rejects, before anything changes.
#[derive(Resource, Clone, Copy)]
pub struct LevelSpawner {
    pub spawn: LevelSpawnFn,
    pub request: LevelRequestFn,
    pub status: LevelStatusFn,
    pub check: LevelCheckFn,
    pub forget: LevelForgetFn,
    pub next_instance: LevelNextFn,
    pub restart_instances: LevelRestartFn,
}

impl LevelSpawner {
    /// A loader that only spawns: files need no loading, every file passes, nothing is kept
    /// or numbered. Replace the fields a real loader needs.
    pub fn new(spawn: LevelSpawnFn) -> Self {
        Self {
            spawn,
            request: |_, _| {},
            status: |_, _| LoadStatus::Ready,
            check: |_, _| Ok(()),
            forget: |_, _| {},
            next_instance: |_| 0,
            restart_instances: |_, _| {},
        }
    }
}

/// How the region in the world was built: its manifest and the first collider id and level
/// instance number its tiles took (in a world with no region: none). Saved in snapshots, so
/// a snapshot taken in a region builds it again, the same down to the ids, where it is
/// loaded ([`snapshot_prepare`]).
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[reflect(Resource, Default)]
pub struct RegionBuild {
    /// [`RegionManifest::to_canonical`].
    pub manifest: String,
    /// [`ColliderIds`] when it was built.
    pub first_collider: u32,
    /// The level loader's next instance number when it was built.
    pub first_level: u32,
}

/// Run after a region is built, inside the call that builds it (behind the loading screen,
/// before the next tick): modules derive their own data from it, in the order they were
/// registered ([`WorldAppExt::on_region_loaded`](crate::WorldAppExt::on_region_loaded)).
// SEAM(B4): `shards_npc` stitches the region's navmesh here.
#[derive(Resource, Clone, Default)]
pub struct RegionLoadedHooks(pub Vec<fn(&mut World)>);

/// Every level file tiles and rooms are built from, sorted, once each: what a level loader
/// loads while the game loads, so that building a region later never waits for a file.
pub fn level_paths(db: &ContentDb) -> Vec<String> {
    level_users(db).into_keys().collect()
}

/// Level file → the tiles and rooms built from it (ids, sorted), for messages that name both.
pub fn level_users(db: &ContentDb) -> BTreeMap<String, Vec<String>> {
    let tiles = db.iter::<TileDef>().map(|(id, t)| (id, &t.geometry));
    let rooms = db.iter::<RoomDef>().map(|(id, r)| (id, &r.geometry));
    let mut users: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, geometry) in tiles.chain(rooms) {
        if let TileGeometry::Level { path } = geometry {
            users.entry(path.clone()).or_default().push(id.to_owned());
        }
    }
    for ids in users.values_mut() {
        ids.sort();
        ids.dedup();
    }
    users
}

/// The level files the content uses, and which of them the loader has been asked for.
#[derive(Resource, Default)]
pub(crate) struct LevelFiles {
    /// [`level_paths`] of the content as it was last read.
    paths: Vec<String>,
    requested: BTreeSet<String>,
    /// Files whose contents were checked once they were loaded ([`report_level_files`]).
    checked: BTreeSet<String>,
    /// The content was read once since a loader was registered.
    synced: bool,
    /// The files a replay being played will build tiles from once its `content_apply` calls
    /// add them ([`shards_content::defs_ahead`]), and whether this playback was read for them.
    ahead: Vec<String>,
    ahead_read: bool,
}

/// `PostUpdate`, once content is built or changed (a hot reload that adds a tile too): asks
/// the loader for the files the content uses. In `PostUpdate`, so that the loading gate of
/// the next frame already waits for them.
pub(crate) fn request_level_files(
    db: Res<ContentDb>,
    spawner: Option<Res<LevelSpawner>>,
    replay: Option<Res<AiReplay>>,
    mut files: ResMut<LevelFiles>,
    mut commands: Commands,
) {
    let Some(spawner) = spawner else { return };
    // A playback began or ended: in a replay, a `content_apply` that adds a tile and the
    // `regen` that builds with it can be one tick apart or less, with no frame between to load
    // the tile's file, so the files are asked for (and the loading gate waits for them) first.
    let playing = replay.as_deref().and_then(AiReplay::playing);
    let new_playback = playing.is_some() != files.ahead_read;
    if !db.is_loaded() || (files.synced && !db.is_changed() && !spawner.is_changed() && !new_playback) {
        return;
    }
    if new_playback {
        files.ahead = playing.map(paths_ahead).unwrap_or_default();
        files.ahead_read = playing.is_some();
    }
    let request = spawner.request;
    files.paths = level_paths(&db);
    files.synced = true;
    // A path outside the asset folder gets no request: the asset server answers with a handle
    // that never loads (the loading gate reports it, see `levels_status`).
    let fresh: Vec<String> = files
        .paths
        .iter()
        .chain(&files.ahead)
        .filter(|p| level_path_problem(p).is_none() && !files.requested.contains(*p))
        .cloned()
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    for path in fresh {
        files.requested.insert(path.clone());
        commands.queue(move |world: &mut World| request(world, &path));
    }
}

/// The loading requirement of the files tiles are built from ([`levels_status`]).
pub(crate) const LEVELS_GATE: &str = "tile levels";

/// Playback hook ([`AiAppExt::add_playback_hook`](ai_core::AiAppExt::add_playback_hook)): the
/// files of the tiles and rooms the recording adds (with its start snapshot's content edits, or
/// its `content_apply` calls) are asked for as it is armed, and the gate holds while they load.
/// [`request_level_files`] would see the playback only in the next frame, and a playback armed
/// in a game that has loaded already may put its start in, and run its first tick, before one.
pub(crate) fn ask_for_files_ahead(world: &mut World) {
    let Some(spawner) = world.get_resource::<LevelSpawner>().copied() else { return };
    let Some(ahead) = world.resource::<AiReplay>().playing().map(paths_ahead) else { return };
    let asked: Vec<String> = ahead.iter().filter(|p| level_path_problem(p).is_none()).cloned().collect();
    let mut left = 0;
    for path in &asked {
        (spawner.request)(world, path);
        if let LoadStatus::Pending(_) = (spawner.status)(world, path) {
            left += 1;
        }
    }
    let mut files = world.resource_mut::<LevelFiles>();
    files.requested.extend(asked);
    files.ahead = ahead;
    files.ahead_read = true;
    if left > 0 {
        let why = if left == 1 { "1 level file left".to_owned() } else { format!("{left} level files left") };
        world.resource_mut::<SimGate>().hold(LEVELS_GATE, why);
    }
}

/// The level files of the tiles and rooms `replay` adds as it plays.
fn paths_ahead(replay: &ai_core::replay::Replay) -> Vec<String> {
    let tiles = shards_content::defs_ahead::<TileDef>(replay).into_iter().map(|t| t.geometry);
    let rooms = shards_content::defs_ahead::<RoomDef>(replay).into_iter().map(|r| r.geometry);
    let paths: BTreeSet<String> = tiles
        .chain(rooms)
        .filter_map(|geometry| match geometry {
            TileGeometry::Level { path } => Some(path),
            _ => None,
        })
        .collect();
    paths.into_iter().collect()
}

/// `PostUpdate`: once a level file is loaded, journals what the loader finds wrong inside it
/// (once per file), naming the tiles built from it. A tile whose file is broken shows when
/// the game loads, not when a seed first picks it; the region change that uses it is refused
/// all the same.
pub(crate) fn report_level_files(world: &mut World) {
    let Some(spawner) = world.get_resource::<LevelSpawner>().copied() else { return };
    let Some(db) = world.get_resource::<ContentDb>().filter(|db| db.is_loaded()) else { return };
    let files = world.resource::<LevelFiles>();
    let waiting: Vec<String> = files.paths.iter().filter(|p| !files.checked.contains(*p) && level_path_problem(p).is_none()).cloned().collect();
    if waiting.is_empty() {
        return;
    }
    let users = level_users(db);
    let mut lines = Vec::new();
    for path in waiting {
        if (spawner.status)(world, &path) != LoadStatus::Ready {
            continue;
        }
        world.resource_mut::<LevelFiles>().checked.insert(path.clone());
        if let Err(why) = (spawner.check)(world, &path) {
            let tiles: Vec<String> = users.get(&path).into_iter().flatten().map(|t| format!("`{t}`")).collect();
            let s = if tiles.len() == 1 { "" } else { "s" };
            lines.push(format!("tile levels: tile{s} {}: level `{path}`: {why}", tiles.join(", ")));
        }
    }
    for line in lines {
        journal::emit(world, JournalKind::Error, line);
    }
}

/// The loading gate's view of the files tiles are built from: ready without a loader, or
/// when every file the content uses is loaded. A file that cannot be loaded names the tiles
/// built from it.
pub(crate) fn levels_status(world: &World) -> LoadStatus {
    let Some(spawner) = world.get_resource::<LevelSpawner>() else { return LoadStatus::Ready };
    let Some(db) = world.get_resource::<ContentDb>().filter(|db| db.is_loaded()) else { return LoadStatus::Ready };
    let files = world.resource::<LevelFiles>();
    if !files.synced {
        return LoadStatus::Pending("asking for the level files".into());
    }
    let (mut waiting, mut failed) = (0, Vec::new());
    for path in &files.paths {
        let status = match level_path_problem(path) {
            Some(why) => LoadStatus::Failed(why),
            None => (spawner.status)(world, path),
        };
        match status {
            LoadStatus::Ready => {}
            LoadStatus::Pending(_) => waiting += 1,
            LoadStatus::Failed(why) => failed.push((path, why)),
        }
    }
    // A file a replay will add waits too; one that cannot load does not hold the game: the
    // replayed call that builds with it says why.
    for path in files.ahead.iter().filter(|p| !files.paths.contains(*p) && level_path_problem(p).is_none()) {
        if let LoadStatus::Pending(_) = (spawner.status)(world, path) {
            waiting += 1;
        }
    }
    if !failed.is_empty() {
        let users = level_users(db);
        let lines: Vec<String> = failed
            .into_iter()
            .map(|(path, why)| {
                let tiles: Vec<String> = users.get(path).into_iter().flatten().map(|t| format!("`{t}`")).collect();
                let s = if tiles.len() == 1 { "" } else { "s" };
                format!("tile{s} {}: {why}", tiles.join(", "))
            })
            .collect();
        return LoadStatus::Failed(lines.join("; "));
    }
    match waiting {
        0 => LoadStatus::Ready,
        1 => LoadStatus::Pending("1 level file left".into()),
        n => LoadStatus::Pending(format!("{n} level files left")),
    }
}

/// The region in the world.
#[derive(Resource, Clone, Debug)]
pub struct ActiveRegion {
    pub manifest: RegionManifest,
    /// [`RegionManifest::hash`].
    pub hash: u64,
    /// Walkable areas and roads between tiles (`None` when content no longer describes
    /// every tile of the manifest).
    pub graph: Option<RegionGraph>,
    /// From the region's content, when it says.
    pub active_radius: Option<f32>,
    /// Entities spawned for it.
    pub entities: usize,
    /// The placed tile covering each cell, row-major.
    cells: Vec<Option<usize>>,
}

impl ActiveRegion {
    pub fn new(manifest: RegionManifest, graph: Option<RegionGraph>, active_radius: Option<f32>, entities: usize) -> Self {
        let [w, h] = manifest.size;
        let mut cells = vec![None; (w * h) as usize];
        for (i, t) in manifest.tiles.iter().enumerate() {
            for dy in 0..t.size[1] as i32 {
                for dx in 0..t.size[0] as i32 {
                    let (x, y) = (t.at[0] + dx, t.at[1] + dy);
                    if manifest.contains([x, y]) {
                        cells[(y as u32 * w + x as u32) as usize] = Some(i);
                    }
                }
            }
        }
        Self { hash: manifest.hash(), manifest, graph, active_radius, entities, cells }
    }

    /// The tile covering `cell`.
    pub fn tile_at_cell(&self, cell: [i32; 2]) -> Option<&PlacedTile> {
        if !self.manifest.contains(cell) {
            return None;
        }
        let i = self.cells[(cell[1] as u32 * self.manifest.size[0] + cell[0] as u32) as usize]?;
        self.manifest.tiles.get(i)
    }

    /// The cell `pos` is in and its tile, inside the region.
    pub fn tile_at(&self, pos: Vec3) -> Option<([i32; 2], &PlacedTile)> {
        let cell = self.manifest.cell_of(pos);
        self.tile_at_cell(cell).map(|t| (cell, t))
    }

    /// Where player `n` (0 first) appears: the spawn point, the others around it.
    pub fn spawn_point(&self, n: usize) -> Vec3 {
        const AROUND: [(f32, f32); 8] = [(1.5, 0.0), (-1.5, 0.0), (0.0, 1.5), (0.0, -1.5), (1.5, 1.5), (-1.5, 1.5), (1.5, -1.5), (-1.5, -1.5)];
        let spawn = self.manifest.spawn_position();
        if n == 0 {
            return spawn;
        }
        let (x, z) = AROUND[(n - 1) % AROUND.len()];
        let ring = (1 + (n - 1) / AROUND.len()) as f32;
        spawn + Vec3::new(x, 0.0, z) * ring
    }
}

/// What [`spawn_region`] built.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionSpawned {
    pub tiles: usize,
    pub pois: usize,
    pub entities: usize,
}

/// What one tile spawns, read from content before anything is spawned.
struct TilePlan {
    id: String,
    geometry: TileGeometry,
    visual: Option<String>,
}

/// The content of every tile `manifest` names, or what stops it from being built: tiles
/// content lacks, level files without a level loader or that the loader rejects (not loaded,
/// not loadable, problems in their nodes), prefabs that cannot be spawned. Checked before
/// anything changes.
fn plan(world: &World, manifest: &RegionManifest) -> Result<Vec<TilePlan>, String> {
    let db = world.resource::<ContentDb>();
    if manifest.layout == Layout::Volume {
        // A volume has no tiles: its kind says whether content (and the code that made it)
        // can build it again; the world module checks the prefabs of its points.
        if !manifest.tiles.is_empty() {
            return Err(format!("the manifest of {} is a volume and names {} tiles: a volume has none", manifest.title(), manifest.tiles.len()));
        }
        let kind = kind_for(world, manifest)?;
        (kind.check)(world, manifest)?;
        check_prefabs(world, manifest, &[])?;
        return Ok(Vec::new());
    }
    let loader = world.get_resource::<LevelSpawner>().copied();
    let mut checked = BTreeSet::new();
    let mut missing = Vec::new();
    let mut plans = Vec::new();
    for t in &manifest.tiles {
        let found = match manifest.layout {
            Layout::Grid => db.get::<TileDef>(&t.tile).map(|d| (d.geometry.clone(), d.visual.clone())),
            Layout::Rooms => db.get::<RoomDef>(&t.tile).map(|d| (d.geometry.clone(), d.visual.clone())),
            // Refused above: a volume names no tiles.
            Layout::Volume => None,
        };
        if let Some((TileGeometry::Level { path }, _)) = &found {
            let Some(loader) = &loader else {
                return Err(format!(
                    "tile `{}` is built from the level file `{path}`, and no level loader is registered \
                     (add the levels module, or give the tile Inline geometry)",
                    t.tile
                ));
            };
            // Once per tile: the file is the same for every cell it covers.
            if checked.insert(t.tile.as_str()) {
                (loader.check)(world, path).map_err(|e| format!("tile `{}`: level `{path}`: {e}", t.tile))?;
            }
        }
        match found {
            Some((geometry, visual)) => plans.push(TilePlan { id: t.tile.clone(), geometry, visual }),
            None => missing.push(t.tile.clone()),
        }
        if turns_of(t.rot).is_none() {
            return Err(format!("tile `{}` at {:?}: rot {} is not 0, 90, 180 or 270", t.tile, t.at, t.rot));
        }
    }
    if !missing.is_empty() {
        missing.sort();
        missing.dedup();
        let kind = match manifest.layout {
            Layout::Grid => "tile",
            Layout::Rooms => "room",
            Layout::Volume => "chunk",
        };
        return Err(format!(
            "the manifest of {} names {kind}s content does not have: {} (it was made from other content, hash {})",
            manifest.title(),
            missing.join(", "),
            manifest.content_hash
        ));
    }
    check_prefabs(world, manifest, &plans)?;
    Ok(plans)
}

/// Whether content can build `manifest` now: every tile or room it names, the level files and
/// prefabs they are made of, the decor and the prefabs of its points of interest. What
/// [`change_region`] checks before it changes anything, for a caller that wants to know first
/// (a save that says where its world came from only once it can be built). Changes nothing.
pub fn check_manifest(world: &World, manifest: &RegionManifest) -> Result<(), String> {
    plan(world, manifest).map(drop)
}

/// Every prefab the build will spawn (tile pieces, decor, points of interest) can be: they are
/// only read while spawning, and by then the old region is gone.
fn check_prefabs(world: &World, manifest: &RegionManifest, plans: &[TilePlan]) -> Result<(), String> {
    let db = world.resource::<ContentDb>();
    let registry = world.resource::<AppTypeRegistry>().read();
    let mut checked = BTreeSet::new();
    let mut check = |id: &str| if checked.insert(id.to_owned()) { check_prefab(db, &registry, id, None) } else { Ok(()) };
    for (placed, plan) in manifest.tiles.iter().zip(plans) {
        let (x, y) = (placed.at[0], placed.at[1]);
        if let TileGeometry::Inline(pieces) = &plan.geometry {
            for piece in pieces {
                if let TilePiece::Prefab { id, .. } = piece {
                    check(id).map_err(|e| format!("tile `{}` at ({x}, {y}): {e}", plan.id))?;
                }
            }
        }
        if let Some(decor) = &placed.decor {
            check(decor).map_err(|e| format!("tile `{}` decor: {e}", plan.id))?;
        }
    }
    for poi in &manifest.pois {
        if let Some(prefab) = db.get::<PoiDef>(&poi.poi).and_then(|d| d.prefab.as_deref()) {
            check(prefab).map_err(|e| format!("poi `{}`: {e}", poi.poi))?;
        }
    }
    Ok(())
}

/// The region kind that builds `manifest` (a [`Layout::Volume`] one), or why there is none.
fn kind_for(world: &World, manifest: &RegionManifest) -> Result<RegionKind, String> {
    let db = world.resource::<ContentDb>();
    world.get_resource::<WorldGen>().and_then(|g| g.kind_for(db, manifest)).cloned().ok_or_else(|| {
        format!("{} is a {:?} region, and no region kind of that layout is registered (is its module in this game?)", manifest.title(), manifest.layout)
    })
}

/// Builds `manifest` in the world (it does not generate anything; a region kind may make its
/// volume again from the manifest's seed). Nothing is left behind when it fails.
pub fn spawn_region(world: &mut World, manifest: &RegionManifest) -> Result<RegionSpawned, String> {
    let plans = plan(world, manifest)?;
    let mut spawned = Vec::new();
    match spawn_parts(world, manifest, &plans, &mut spawned) {
        Ok(()) => Ok(RegionSpawned { tiles: manifest.tiles.len(), pois: manifest.pois.len(), entities: spawned.len() }),
        Err(e) => {
            let undone: Vec<Entity> = spawned.into_iter().rev().collect();
            forget_levels(world, &undone);
            for entity in undone {
                if let Ok(e) = world.get_entity_mut(entity) {
                    e.despawn();
                }
            }
            unload_kinds(world);
            Err(e)
        }
    }
}

/// Every region kind forgets what it kept of the region outside its entities.
fn unload_kinds(world: &mut World) {
    let kinds: Vec<RegionKind> = world.get_resource::<WorldGen>().map(|g| g.kinds().to_vec()).unwrap_or_default();
    for kind in kinds {
        (kind.unload)(world);
    }
}

fn spawn_parts(world: &mut World, manifest: &RegionManifest, plans: &[TilePlan], spawned: &mut Vec<Entity>) -> Result<(), String> {
    if manifest.layout == Layout::Volume {
        // The kind's own parts first (terrain, water), then the points of interest below, as
        // tiles come before them.
        let kind = kind_for(world, manifest)?;
        for entity in (kind.spawn)(world, manifest)? {
            world.entity_mut(entity).insert(RegionPart);
            spawned.push(entity);
        }
    }
    for (placed, plan) in manifest.tiles.iter().zip(plans) {
        let turns = turns_of(placed.rot).unwrap_or(0);
        let center = manifest.tile_center(placed);
        let turned = rotation(turns);
        let tile = TileId { x: placed.at[0], y: placed.at[1] };
        let root = Transform::from_translation(center).with_rotation(turned);
        let name = Name::new(format!("tile ({}, {}) {}", tile.x, tile.y, plan.id));
        let region_tile = RegionTile { tile: plan.id.clone(), rot: placed.rot, size: UVec2::new(placed.size[0], placed.size[1]), visual: plan.visual.clone() };
        spawned.push(world.spawn((region_tile, tile, RegionPart, root, name)).id());
        let at = |local: Vec3| Transform::from_translation(center + turn(local, turns)).with_rotation(turned);
        match &plan.geometry {
            TileGeometry::None => {}
            TileGeometry::Inline(pieces) => {
                for piece in pieces {
                    let entity = match piece {
                        TilePiece::Box { min, max } => {
                            let half = (*max - *min) * 0.5;
                            world.spawn((Collider::new(Shape::Cuboid { half }), at((*min + *max) * 0.5))).id()
                        }
                        TilePiece::Solid { shape, at: local, yaw } => {
                            let tf = at(*local);
                            let tf = tf.with_rotation(tf.rotation * Quat::from_rotation_y(-yaw.to_radians()));
                            world.spawn((Collider::new(*shape), tf)).id()
                        }
                        TilePiece::Volume { kind, shape, at: local } => world
                            .spawn((Collider::new(*shape).with_layers(CollisionLayers::VOLUME), Volume { kind: kind.clone() }, at(*local)))
                            .id(),
                        TilePiece::Marker { name, at: local } => {
                            world.spawn((Marker { name: name.clone() }, Name::new(name.clone()), at(*local))).id()
                        }
                        TilePiece::Prefab { id, at: local, yaw } => {
                            let tf = at(*local);
                            let tf = tf.with_rotation(tf.rotation * Quat::from_rotation_y(-yaw.to_radians()));
                            spawn_prefab(world, id, tf, None).map_err(|e| format!("tile `{}` at ({}, {}): {e}", plan.id, tile.x, tile.y))?
                        }
                    };
                    world.entity_mut(entity).insert((tile, RegionPart));
                    spawned.push(entity);
                }
            }
            TileGeometry::Level { path } => {
                let Some(LevelSpawner { spawn, .. }) = world.get_resource::<LevelSpawner>().copied() else {
                    return Err(format!(
                        "tile `{}` is built from the level file `{path}`, and no level loader is registered \
                         (add the levels module, or give the tile Inline geometry)",
                        plan.id
                    ));
                };
                let request = LevelSpawn { path, transform: root, tile, tile_id: &plan.id };
                let entities = spawn(world, &request).map_err(|e| format!("tile `{}`: level `{path}`: {e}", plan.id))?;
                for entity in entities {
                    world.entity_mut(entity).insert((tile, RegionPart));
                    spawned.push(entity);
                }
            }
        }
        if let Some(decor) = &placed.decor {
            let entity = spawn_prefab(world, decor, root, None).map_err(|e| format!("tile `{}` decor: {e}", plan.id))?;
            world.entity_mut(entity).insert((tile, RegionPart));
            spawned.push(entity);
        }
    }
    for poi in &manifest.pois {
        let (label, prefab) = {
            let def = world.resource::<ContentDb>().get::<PoiDef>(&poi.poi);
            (PoiDef::label_of(def, &poi.poi), def.and_then(|d| d.prefab.clone()))
        };
        let tile = TileId { x: poi.cell[0], y: poi.cell[1] };
        let tf = Transform::from_translation(poi.position());
        let marker = (AiLandmark, AiKind::new(label), Name::new(poi.id.clone()), Poi { id: poi.id.clone(), poi: poi.poi.clone() });
        spawned.push(world.spawn((marker, tile, RegionPart, tf)).id());
        if let Some(prefab) = prefab {
            let entity = spawn_prefab(world, &prefab, tf, None).map_err(|e| format!("poi `{}`: {e}", poi.poi))?;
            world.entity_mut(entity).insert((tile, RegionPart));
            spawned.push(entity);
        }
    }
    // Last, so the tiles and points keep the collider ids they had without it.
    if let Some(height) = world.get_resource::<ContentDb>().and_then(|db| edge_wall(db, manifest)) {
        for (half, center) in edge_wall_boxes(manifest, height) {
            let wall = (Name::new("region edge"), Collider::new(Shape::Cuboid { half }), Transform::from_translation(center), RegionPart, RegionEdge);
            spawned.push(world.spawn(wall).id());
        }
    }
    Ok(())
}

/// The height of the wall around `manifest`'s edge: its region's `edge_wall` (a grid; a
/// dungeon's rooms have walls of their own).
pub fn edge_wall(db: &ContentDb, manifest: &RegionManifest) -> Option<f32> {
    if manifest.layout != Layout::Grid {
        return None;
    }
    db.get::<RegionDef>(&manifest.region)?.edge_wall
}

/// The four boxes of an edge wall `height` meters high, as (half extents, center): just outside
/// the region on every side, overlapping at the corners, from [`EDGE_WALL_DEPTH`] below the
/// ground plane to `height` above it.
pub fn edge_wall_boxes(manifest: &RegionManifest, height: f32) -> [(Vec3, Vec3); 4] {
    let (t, y_half) = (EDGE_WALL_THICKNESS, (height + EDGE_WALL_DEPTH) * 0.5);
    let y = (height - EDGE_WALL_DEPTH) * 0.5;
    let min = manifest.corner();
    let size = Vec3::new(manifest.size[0] as f32, 0.0, manifest.size[1] as f32) * manifest.tile_size;
    let (half_x, half_z) = (size.x * 0.5 + t, size.z * 0.5 + t);
    let mid = min + size * 0.5;
    [
        // north and south, across the corners
        (Vec3::new(half_x, y_half, t * 0.5), Vec3::new(mid.x, y, min.z - t * 0.5)),
        (Vec3::new(half_x, y_half, t * 0.5), Vec3::new(mid.x, y, min.z + size.z + t * 0.5)),
        // west and east
        (Vec3::new(t * 0.5, y_half, half_z), Vec3::new(min.x - t * 0.5, y, mid.z)),
        (Vec3::new(t * 0.5, y_half, half_z), Vec3::new(min.x + size.x + t * 0.5, y, mid.z)),
    ]
}

/// Despawns every entity of the region (in id order) and forgets it; how many went.
pub fn despawn_region(world: &mut World) -> usize {
    let mut parts: Vec<(Option<AiId>, Option<ColliderId>, Entity)> = world
        .query_filtered::<(Entity, Option<&AiId>, Option<&ColliderId>), With<RegionPart>>()
        .iter(world)
        .map(|(e, id, collider)| (id.copied(), collider.copied(), e))
        .collect();
    parts.sort_unstable();
    let count = parts.len();
    let doomed: Vec<Entity> = parts.into_iter().map(|(.., entity)| entity).collect();
    forget_levels(world, &doomed);
    for entity in doomed {
        if let Ok(e) = world.get_entity_mut(entity) {
            e.despawn();
        }
    }
    unload_kinds(world);
    world.remove_resource::<ActiveRegion>();
    world.remove_resource::<RegionBuild>();
    count
}

/// Tells the level loader (if any) that `doomed` are about to be despawned.
fn forget_levels(world: &mut World, doomed: &[Entity]) {
    if let Some(LevelSpawner { forget, .. }) = world.get_resource::<LevelSpawner>().copied() {
        forget(world, doomed);
    }
}

/// Players, in `PlayerId` then `AiId` order: entities with `PlayerId` or `AiPlayer`.
pub fn players(world: &mut World) -> Vec<Entity> {
    let mut q = world.query_filtered::<(Entity, Option<&PlayerId>, Option<&AiId>), Or<(With<PlayerId>, With<AiPlayer>)>>();
    let mut players: Vec<(u8, AiId, Entity)> =
        q.iter(world).map(|(e, player, id)| (player.map_or(u8::MAX, |p| p.0), id.copied().unwrap_or_default(), e)).collect();
    players.sort_unstable();
    players.into_iter().map(|(.., e)| e).collect()
}

/// The height of the ground under `at` (a ray down from 2 m above it), with the physics
/// world brought up to date first. A ray that starts inside a solid (the floor stands higher
/// than `at`, as on a plateau tile) starts again above that solid's top.
pub fn ground_below(world: &mut World, at: Vec3, exclude: &[Entity]) -> Option<f32> {
    // The same pure function of the ECS the ticks run: it changes no tick's outcome.
    world.run_system_cached(shards_motion::physics::sync_physics).ok()?;
    let physics = world.get_resource::<PhysicsWorld>()?;
    let filter = QueryFilter { mask: LayerMask::WORLD, exclude: exclude.to_vec(), volumes: false };
    let mut from = at + Vec3::Y * 2.0;
    // Each round climbs out of one more solid; a few stacked floors are all there is to find.
    for _ in 0..8 {
        let hit = physics.raycast(from, Dir3::NEG_Y, 50.0, &filter)?;
        if hit.distance > 0.0 {
            return Some(hit.point.y);
        }
        // Above the solid the ray started in: a collider's box top, a voxel field's run of cells.
        from.y = physics.extent_at(hit.entity, hit.point)?.1 + 0.05;
    }
    None
}

/// Moves every player to the region's spawn (on the ground there) and drops their orders,
/// which pointed into the old region. Returns where the first one went.
fn move_players(world: &mut World) -> Option<Vec3> {
    let region = world.get_resource::<ActiveRegion>()?.clone();
    let mut first = None;
    for (n, entity) in players(world).into_iter().enumerate() {
        let mut at = region.spawn_point(n);
        if let Some(y) = ground_below(world, at, &[entity]) {
            at.y = y;
        }
        if let Some(mut tf) = world.get_mut::<Transform>(entity) {
            tf.translation = at;
        }
        if let Some(mut intent) = world.get_mut::<Intent>(entity)
            && (!intent.orders.is_empty() || intent.move_dir != Vec3::ZERO)
        {
            intent.orders.clear();
            intent.move_dir = Vec3::ZERO;
        }
        first.get_or_insert(at);
    }
    first
}

/// `region generated: forest_a seed 42 v3 (attempt 2, 64 tiles, 5 POIs)`.
fn generated_line(generated: &Generated) -> String {
    let m = &generated.manifest;
    let how = match &m.fallback {
        Some(map) => format!("fallback {map} after {} attempts", generated.failures.len()),
        None => format!("attempt {}", m.attempt),
    };
    let (count, tiles) = match m.layout {
        Layout::Grid => (m.tiles.len(), "tiles"),
        Layout::Rooms => (m.tiles.len(), "rooms"),
        // A volume's cells are its chunks.
        Layout::Volume => ((m.size[0] * m.size[1]) as usize, "chunks"),
    };
    format!("region generated: {} ({how}, {count} {tiles}, {} POIs)", m.title(), m.pois.len())
}

/// Journals how a region came about: its failed attempts, remarks and the result.
pub fn journal_generated(world: &mut World, generated: &Generated) {
    let title = generated.manifest.title();
    for failure in &generated.failures {
        journal::emit(world, JournalKind::Warn, format!("worldgen {title}: {failure}"));
    }
    for remark in &generated.remarks {
        journal::emit(world, JournalKind::Warn, format!("worldgen {title}: {remark}"));
    }
    journal::emit(world, JournalKind::Event, generated_line(generated));
}

fn state(world: &World) -> Option<EngineState> {
    world.get_resource::<State<EngineState>>().map(|s| *s.get())
}

fn set_state(world: &mut World, next: EngineState) {
    world.resource_mut::<NextState<EngineState>>().set(next);
    let _ = world.try_run_schedule(StateTransition);
}

/// Whether a state change can take effect now. Not inside a state transition (a game calling
/// [`change_region`] from `OnEnter(InGame)`): its schedule is out of the world while it runs,
/// so a requested state would only wait in `NextState` and enter the same state again, and
/// again, every frame.
fn can_change_state(world: &World) -> bool {
    world.resource::<Schedules>().contains(StateTransition)
}

/// Replaces the region in the world with `manifest`: through `RegionLoading` (from the game
/// or a menu; called from a state transition such as `OnEnter(InGame)` it replaces the region
/// without the loading screen), old region out, new region in, players to its spawn, back to
/// `InGame` (or held by the loading gate while another module still loads). Refuses, changing
/// nothing, a manifest whose tiles or prefabs content lacks.
pub fn change_region(world: &mut World, manifest: RegionManifest) -> Result<RegionSpawned, String> {
    plan(world, &manifest)?;
    let trip = matches!(state(world), Some(EngineState::InGame | EngineState::MainMenu)) && can_change_state(world);
    if trip {
        set_state(world, EngineState::RegionLoading);
    }
    if let Some(old) = world.get_resource::<ActiveRegion>().map(|r| r.manifest.title()) {
        let removed = despawn_region(world);
        journal::emit(world, JournalKind::Event, format!("region unloaded: {old} ({removed} entities)"));
    }
    let id = manifest.region.clone();
    let result = install(world, manifest);
    if trip {
        finish_loading(world);
    }
    if result.is_ok() {
        // A host sends its clients the new world (a rebase).
        ai_core::world_rebuilt(world, format!("region {id}"));
    }
    result
}

/// Spawns `manifest`, makes it the active region and moves the players there.
fn install(world: &mut World, manifest: RegionManifest) -> Result<RegionSpawned, String> {
    let first_collider = world.get_resource::<ColliderIds>().map_or(0, |ids| ids.peek().0);
    let first_level = world.get_resource::<LevelSpawner>().copied().map_or(0, |l| (l.next_instance)(world));
    let spawned = spawn_region(world, &manifest)?;
    world.insert_resource(RegionBuild { manifest: manifest.to_canonical(), first_collider, first_level });
    let kind = if manifest.layout == Layout::Volume { Some(kind_for(world, &manifest)?) } else { None };
    let (graph, radius) = {
        let db = world.resource::<ContentDb>();
        let radius = match manifest.layout {
            Layout::Grid => db.get::<crate::content::RegionDef>(&manifest.region).and_then(|d| d.active_radius),
            Layout::Rooms => db.get::<crate::content::DungeonDef>(&manifest.region).and_then(|d| d.active_radius),
            Layout::Volume => kind.as_ref().and_then(|k| (k.active_radius)(db, &manifest.region)),
        };
        (RegionGraph::build(db, &manifest).ok(), radius)
    };
    let title = manifest.title();
    world.insert_resource(ActiveRegion::new(manifest, graph, radius, spawned.entities));
    crate::ai::register_conditions(world);
    let players = match move_players(world) {
        Some(at) => format!(", players at {}", ai_core::spatial::fmt_pos(at)),
        None => String::new(),
    };
    let what = match &kind {
        // `voxel_arena seed 7 v1 (128 x 32 x 128, 3 water boxes, 9 entities, players at …)`.
        Some(kind) => format!("{}, {} entities{players}", (kind.summary)(world), spawned.entities),
        None => format!("{} tiles, {} entities{players}", spawned.tiles, spawned.entities),
    };
    journal::emit(world, JournalKind::Event, format!("region loaded: {title} ({what})"));
    if let Some(RegionLoadedHooks(hooks)) = world.get_resource::<RegionLoadedHooks>().cloned() {
        for hook in hooks {
            hook(world);
        }
    }
    Ok(spawned)
}

/// Snapshot prepare (registered by [`WorldPlugin`](crate::WorldPlugin)): the region of the
/// snapshot (its [`RegionBuild`]) is built again when the world has another one, or the same
/// one built with other ids; the region in the world goes when the snapshot was taken
/// without one. Inside the load, with no loading screen: the snapshot puts back players and
/// characters, and the collider and level counters, right after. Refuses, changing nothing,
/// a manifest content cannot build; a load refused later (or a build that fails half way)
/// runs this again with the world as it was, which builds the old region back.
pub fn snapshot_prepare(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    let saved = match snapshot.resources.get(<RegionBuild as TypePath>::type_path()) {
        Some(value) => Some(serde_json::from_value::<RegionBuild>(value.clone()).map_err(|e| format!("its region record does not read: {e}"))?),
        None => None,
    };
    let current = world.get_resource::<RegionBuild>().cloned();
    if saved == current {
        return Ok(None);
    }
    let Some(build) = saved else {
        let title = world.get_resource::<ActiveRegion>().map(|r| r.manifest.title()).unwrap_or_default();
        let removed = despawn_region(world);
        journal::emit(world, JournalKind::Event, format!("region unloaded: {title} ({removed} entities)"));
        return Ok(Some(format!("{title} unloaded: the snapshot was taken without one")));
    };
    let manifest = RegionManifest::from_json(&build.manifest)?;
    request_files(world, &manifest)?;
    plan(world, &manifest)?;
    if let Some(old) = world.get_resource::<ActiveRegion>().map(|r| r.manifest.title()) {
        let removed = despawn_region(world);
        journal::emit(world, JournalKind::Event, format!("region unloaded: {old} ({removed} entities)"));
    }
    // The ids the tiles had: the snapshot sets the counters to its own right after.
    if let Some(mut ids) = world.get_resource_mut::<ColliderIds>() {
        ids.restart_at(ColliderId(build.first_collider));
    }
    if let Some(LevelSpawner { restart_instances, .. }) = world.get_resource::<LevelSpawner>().copied() {
        restart_instances(world, build.first_level);
    }
    let title = manifest.title();
    let spawned = install(world, manifest)?;
    Ok(Some(format!("{title} built again from its manifest ({} entities)", spawned.entities)))
}

/// Asks the loader for the level files `manifest`'s tiles are built from, and refuses while one
/// is still loading. A snapshot taken after a content edit added a tile names a file this game
/// may never have asked for (a replay that starts from it asks as it is armed, see
/// [`ask_for_files_ahead`]); asked for now, it is in a moment later, and the same load succeeds.
fn request_files(world: &mut World, manifest: &RegionManifest) -> Result<(), String> {
    let Some(loader) = world.get_resource::<LevelSpawner>().copied() else { return Ok(()) };
    let paths: BTreeSet<String> = {
        let db = world.resource::<ContentDb>();
        manifest
            .tiles
            .iter()
            .filter_map(|t| match manifest.layout {
                Layout::Grid => db.get::<TileDef>(&t.tile).map(|d| d.geometry.clone()),
                Layout::Rooms => db.get::<RoomDef>(&t.tile).map(|d| d.geometry.clone()),
                // No tiles, no level files.
                Layout::Volume => None,
            })
            .filter_map(|geometry| match geometry {
                TileGeometry::Level { path } if level_path_problem(&path).is_none() => Some(path),
                _ => None,
            })
            .collect()
    };
    let mut loading = Vec::new();
    for path in paths {
        (loader.request)(world, &path);
        if let LoadStatus::Pending(_) = (loader.status)(world, &path) {
            loading.push(format!("`{path}`"));
        }
    }
    match loading.as_slice() {
        [] => Ok(()),
        [one] => Err(format!(
            "its tiles are built from the level file {one}, which this game is loading only now: load the snapshot again in a moment"
        )),
        many => Err(format!(
            "its tiles are built from the level files {}, which this game is loading only now: load the snapshot again in a moment",
            many.join(", ")
        )),
    }
}

/// Back to `InGame` now if nothing else loads; else the gate holds the ticks under each
/// pending requirement and goes back once they are ready.
fn finish_loading(world: &mut World) {
    let pending: Vec<(String, String)> = loading_statuses(world)
        .into_iter()
        .filter_map(|(label, status)| match status {
            LoadStatus::Ready => None,
            LoadStatus::Pending(why) => Some((label, why)),
            LoadStatus::Failed(why) => Some((label, format!("failed: {why}"))),
        })
        .collect();
    if pending.is_empty() {
        set_state(world, EngineState::InGame);
        return;
    }
    let mut gate = world.resource_mut::<SimGate>();
    for (label, why) in pending {
        gate.hold(&label, why);
    }
}

/// Generates `region` with `seed` (pure) and changes to it.
pub fn regen(world: &mut World, region: &str, seed: u64) -> Result<(Generated, RegionSpawned), String> {
    let worldgen = world.resource::<WorldGen>().clone();
    let generated = generate::generate(world.resource::<ContentDb>(), &worldgen, region, seed).map_err(|e| {
        let attempts: Vec<String> = e.failures.iter().map(|f| format!("  {f}")).collect();
        if attempts.is_empty() { e.message } else { format!("{}\n{}", e.message, attempts.join("\n")) }
    })?;
    journal_generated(world, &generated);
    let spawned = change_region(world, generated.manifest.clone())?;
    Ok((generated, spawned))
}

// --- the start region --------------------------------------------------------------------

/// The region generated while the game loads.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub enum StartRegion {
    /// None asked for.
    #[default]
    None,
    /// Generated once content is loaded, with the world seed.
    Pending(String),
    Failed(String),
    Done,
}

/// The loading gate's view: a start region holds the first tick until it stands.
pub(crate) fn start_status(world: &World) -> LoadStatus {
    match world.get_resource::<StartRegion>() {
        Some(StartRegion::Pending(region)) => LoadStatus::Pending(format!("generating {region}")),
        Some(StartRegion::Failed(why)) => LoadStatus::Failed(why.clone()),
        _ => LoadStatus::Ready,
    }
}

/// `Update` while loading: once content is in, generates and builds the start region (before
/// `OnEnter(InGame)` spawns the game, so its ids come first).
pub(crate) fn build_start_region(world: &mut World) {
    let Some(StartRegion::Pending(region)) = world.get_resource::<StartRegion>().cloned() else {
        return;
    };
    if state(world) != Some(EngineState::Loading) || !world.resource::<ContentDb>().is_loaded() {
        return;
    }
    // Building spawns synchronously, so the files tiles are made of must be in already (the
    // loading gate reports a file that cannot load).
    if levels_status(world) != LoadStatus::Ready {
        return;
    }
    // A joining client generates the host's region: it waits for the host's seed.
    if !seed_ready(world) {
        return;
    }
    let seed = world.get_resource::<SimSeed>().map_or(0, |s| s.0);
    let outcome = regen(world, &region, seed);
    let next = match outcome {
        Ok(_) => StartRegion::Done,
        Err(why) => StartRegion::Failed(format!("start region: {why}")),
    };
    world.insert_resource(next);
}

#[cfg(test)]
mod tests {
    use super::*;
    use shards_motion::{VoxelCollider, VoxelField};

    /// A floor (top 0), a plateau 3..5 over x 2..6 (a tunnel under it) and a pillar 0..10 at
    /// x 8..9, of 0.5 m cells; or the same as boxes.
    fn world(field: bool) -> World {
        let mut world = World::new();
        world.init_resource::<ColliderIds>();
        world.init_resource::<PhysicsWorld>();
        if field {
            let solid = |c: IVec3| c.y < 2 || ((4..12).contains(&c.x) && (8..12).contains(&c.y)) || ((16..18).contains(&c.x) && c.y < 22);
            world.spawn(VoxelCollider::new(VoxelField::new(Vec3::new(0.0, -1.0, 0.0), 0.5, UVec3::new(20, 24, 8), solid)));
        } else {
            for (centre, half) in [(Vec3::new(5.0, -0.5, 2.0), Vec3::new(5.0, 0.5, 2.0)), (Vec3::new(4.0, 4.0, 2.0), Vec3::new(2.0, 1.0, 2.0)), (Vec3::new(8.5, 5.0, 2.0), Vec3::new(0.5, 5.0, 2.0))] {
                world.spawn((Collider::new(Shape::cuboid(half)), Transform::from_translation(centre)));
            }
        }
        world
    }

    #[test]
    fn the_ground_below_is_the_run_of_cells_a_ray_meets_or_climbs_out_of() {
        for field in [true, false] {
            let mut world = world(field);
            let mut ground = |x: f32, y: f32| ground_below(&mut world, Vec3::new(x, y, 2.25), &[]);
            assert_eq!(ground(1.25, 0.0), Some(0.0), "the floor");
            assert_eq!(ground(4.25, 0.5), Some(0.0), "in the tunnel: its floor, not the plateau above");
            assert_eq!(ground(4.25, 4.5), Some(5.0), "on the plateau");
            assert_eq!(ground(8.25, 7.0), Some(10.0), "from inside the pillar: out over its top");
        }
    }
}
