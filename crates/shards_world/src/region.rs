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

use std::collections::{BTreeMap, BTreeSet};

use ai_core::journal::{self, JournalKind};
use ai_core::{AiId, AiKind, AiLandmark, AiPlayer, SimGate};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{spawn_prefab, ContentDb};
use shards_core::{loading_statuses, EngineConfig, EngineState, Intent, LoadStatus, PlayerId};
use shards_motion::{Collider, ColliderId, CollisionLayers, LayerMask, PhysicsWorld, QueryFilter, Shape, Volume};

use crate::checks::RegionGraph;
use crate::content::{PoiDef, RoomDef, TileDef, TileGeometry, TilePiece};
use crate::generate::{self, Generated, WorldGen};
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
}

impl LevelSpawner {
    /// A loader that only spawns: files need no loading, every file passes, nothing is kept.
    /// Replace the fields a real loader needs.
    pub fn new(spawn: LevelSpawnFn) -> Self {
        Self { spawn, request: |_, _| {}, status: |_, _| LoadStatus::Ready, check: |_, _| Ok(()), forget: |_, _| {} }
    }
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
    /// The content was read once since a loader was registered.
    synced: bool,
}

/// `PostUpdate`, once content is built or changed (a hot reload that adds a tile too): asks
/// the loader for the files the content uses. In `PostUpdate`, so that the loading gate of
/// the next frame already waits for them.
pub(crate) fn request_level_files(
    db: Res<ContentDb>,
    spawner: Option<Res<LevelSpawner>>,
    mut files: ResMut<LevelFiles>,
    mut commands: Commands,
) {
    let Some(spawner) = spawner else { return };
    if !db.is_loaded() || (files.synced && !db.is_changed() && !spawner.is_changed()) {
        return;
    }
    let request = spawner.request;
    files.paths = level_paths(&db);
    files.synced = true;
    let fresh: Vec<String> = files.paths.iter().filter(|p| !files.requested.contains(*p)).cloned().collect();
    for path in fresh {
        files.requested.insert(path.clone());
        commands.queue(move |world: &mut World| request(world, &path));
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
        match (spawner.status)(world, path) {
            LoadStatus::Ready => {}
            LoadStatus::Pending(_) => waiting += 1,
            LoadStatus::Failed(why) => failed.push((path, why)),
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
/// not loadable, problems in their nodes). Checked before anything changes.
fn plan(world: &World, manifest: &RegionManifest) -> Result<Vec<TilePlan>, String> {
    let db = world.resource::<ContentDb>();
    let loader = world.get_resource::<LevelSpawner>().copied();
    let mut checked = BTreeSet::new();
    let mut missing = Vec::new();
    let mut plans = Vec::new();
    for t in &manifest.tiles {
        let found = match manifest.layout {
            Layout::Grid => db.get::<TileDef>(&t.tile).map(|d| (d.geometry.clone(), d.visual.clone())),
            Layout::Rooms => db.get::<RoomDef>(&t.tile).map(|d| (d.geometry.clone(), d.visual.clone())),
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
        let kind = if manifest.layout == Layout::Grid { "tile" } else { "room" };
        return Err(format!(
            "the manifest of {} names {kind}s content does not have: {} (it was made from other content, hash {})",
            manifest.title(),
            missing.join(", "),
            manifest.content_hash
        ));
    }
    Ok(plans)
}

/// Builds `manifest` in the world (it does not generate anything). Nothing is left behind
/// when it fails.
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
            Err(e)
        }
    }
}

fn spawn_parts(world: &mut World, manifest: &RegionManifest, plans: &[TilePlan], spawned: &mut Vec<Entity>) -> Result<(), String> {
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
    Ok(())
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
    world.remove_resource::<ActiveRegion>();
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
/// world brought up to date first.
pub fn ground_below(world: &mut World, at: Vec3, exclude: &[Entity]) -> Option<f32> {
    // The same pure function of the ECS the ticks run: it changes no tick's outcome.
    world.run_system_cached(shards_motion::physics::sync_physics).ok()?;
    let physics = world.get_resource::<PhysicsWorld>()?;
    let filter = QueryFilter { mask: LayerMask::WORLD, exclude: exclude.to_vec(), volumes: false };
    physics.raycast(at + Vec3::Y * 2.0, Dir3::NEG_Y, 50.0, &filter).map(|hit| hit.point.y)
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
    let tiles = if m.layout == Layout::Grid { "tiles" } else { "rooms" };
    format!("region generated: {} ({how}, {} {tiles}, {} POIs)", m.title(), m.tiles.len(), m.pois.len())
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

/// Replaces the region in the world with `manifest`: through `RegionLoading` (from the game
/// or a menu), old region out, new region in, players to its spawn, back to `InGame` (or held
/// by the loading gate while another module still loads). Refuses, changing nothing, a
/// manifest whose tiles content lacks.
pub fn change_region(world: &mut World, manifest: RegionManifest) -> Result<RegionSpawned, String> {
    plan(world, &manifest)?;
    let trip = matches!(state(world), Some(EngineState::InGame | EngineState::MainMenu));
    if trip {
        set_state(world, EngineState::RegionLoading);
    }
    if let Some(old) = world.get_resource::<ActiveRegion>().map(|r| r.manifest.title()) {
        let removed = despawn_region(world);
        journal::emit(world, JournalKind::Event, format!("region unloaded: {old} ({removed} entities)"));
    }
    let result = install(world, manifest);
    if trip {
        finish_loading(world);
    }
    result
}

/// Spawns `manifest`, makes it the active region and moves the players there.
fn install(world: &mut World, manifest: RegionManifest) -> Result<RegionSpawned, String> {
    let spawned = spawn_region(world, &manifest)?;
    let (graph, radius) = {
        let db = world.resource::<ContentDb>();
        let radius = match manifest.layout {
            Layout::Grid => db.get::<crate::content::RegionDef>(&manifest.region).and_then(|d| d.active_radius),
            Layout::Rooms => db.get::<crate::content::DungeonDef>(&manifest.region).and_then(|d| d.active_radius),
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
    let what = format!("{} tiles, {} entities{players}", spawned.tiles, spawned.entities);
    journal::emit(world, JournalKind::Event, format!("region loaded: {title} ({what})"));
    if let Some(RegionLoadedHooks(hooks)) = world.get_resource::<RegionLoadedHooks>().cloned() {
        for hook in hooks {
            hook(world);
        }
    }
    Ok(spawned)
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
    let seed = world.get_resource::<EngineConfig>().map_or(0, |c| c.seed);
    let outcome = regen(world, &region, seed);
    let next = match outcome {
        Ok(_) => StartRegion::Done,
        Err(why) => StartRegion::Failed(format!("start region: {why}")),
    };
    world.insert_resource(next);
}
