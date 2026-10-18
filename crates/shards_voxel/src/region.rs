//! The voxel region: the content kind `voxel_region` (`*.voxel_region.ron`), a region kind of
//! the world module ([`RegionKind`]).
//!
//! A voxel region is generated, built, saved, snapshot and reported through the same region
//! API as a grid of tiles: `regen region=<id>`, `change_region`, the [`RegionManifest`] (with
//! `layout: volume`, no tiles, the steps that made it with their versions), the points of
//! interest and the spawn (from the marks the steps leave), [`Marker`]s, the active radius,
//! the `region` save section. What is particular to it stays here:
//!
//! - **The manifest does not hold the volume.** It holds the seed, the attempt that passed and
//!   the steps with their versions, and [`spawn`] makes the volume again from them and the
//!   content (a pure function: the same inputs, the same cells). Its `content_hash` covers the
//!   region definition and what the palette's blocks are to the world (names, order, solid,
//!   liquid, foliage, tags), not their colours. A manifest whose steps this build has at another
//!   version, or whose content changed, is refused (naming the step), never built otherwise.
//! - **Spawning makes**, in this order (so the collider ids are the same every time): the
//!   terrain entity (a [`VoxelCollider`], `Name("voxel terrain <id>")`, no `AiId`); one water
//!   [`Volume`] per box of [`water_boxes`], its top `water.surface_drop` below the cells'; four
//!   sea boxes when the definition has a `sea` (north, south, west, east of the grid, out to
//!   `reach`); a [`Current`] on every water volume when the sea has a `current`; the
//!   definition's `markers`. Then the world module spawns the points of interest. The
//!   [`VoxelTerrain`] resource holds the volume, the palette and the hash.
//! - **No edge wall**: past the grid is the sea (with one) or nothing.
//! - **The rebuild is cached in-process** (snapshot loads and heavy-mode twins build the same
//!   region many times): the volume, the field and the water boxes, keyed by the manifest's
//!   canonical text and the code of its steps and checks, shared by `Arc`. The entities are
//!   spawned fresh every time, in the same order, so a cached build and a cold one are the
//!   same world, collider id for collider id. `SHARDS_VOXEL_CACHE=off` (or
//!   [`VoxelSettings::cache`]) turns it off.
//!
//! Until the in-memory navigation bake exists, a voxel region has no navmesh: NPCs plan over the
//! local grid, as the navigation module documents for a region without one.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shards_content::{ContentDb, ContentDef, Refs};
use shards_motion::{Collider, CollisionLayers, Current, Shape, Volume, VolumeKind, VoxelCollider, VoxelField};
use shards_world::generate::{AttemptFailure, GenError, Generated as RegionGenerated, Stage, WorldGen};
use shards_world::grid::mm;
use shards_world::manifest::{content_hash, fnv, Layout, ManifestPoi, RegionManifest, FORMAT};
use shards_world::{ActiveRegion, Marker, RegionKind, TileId};
use ai_core::journal::{self, JournalKind};

use crate::merge::{water_boxes, CellBox};
use crate::palette::{VoxelPalette, VoxelPaletteDef};
use crate::volume::VoxelVolume;
use crate::worldgen::{generate_attempt, generate_attempts, AttemptError, AttemptStage, GenSpec, Mark, VoxelSteps, MAX_ATTEMPTS};

/// The content kind of voxel regions.
pub const CONTENT: &str = "voxel_region";

/// How many built volumes the in-process cache keeps (the least recently used goes first).
const CACHE_SIZE: usize = 4;

/// The farthest a sea may reach past the grid, m: 100 km out, an `f32` position still has its
/// centimetres (its step there is under 1 cm).
pub const MAX_SEA_REACH: f32 = 100_000.0;

fn one() -> f32 {
    1.0
}

fn one_u32() -> u32 {
    1
}

fn one_u64() -> u64 {
    1
}

/// The water of a voxel region.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterDef {
    /// Metres the surface of every water volume lies below the top of its liquid cells (0: at
    /// the top). Less than a block.
    #[serde(default)]
    pub surface_drop: f32,
}

/// Sea around a voxel region's grid: four water boxes from the grid's edge out to `reach`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeaDef {
    /// The layer (cells from the volume's bottom) whose top is the sea's surface, less
    /// `water.surface_drop`: the same number as the `level` of a `fill_water` step.
    pub level: u32,
    /// Metres the sea reaches out from every side of the grid.
    pub reach: f32,
    /// The layer of the sea's floor (cells from the volume's bottom; may have a fraction): the
    /// bottom of the sea boxes. Nothing is solid under them past the grid.
    pub floor: f32,
    /// Water that turns swimmers back from the grid's edge; none: still water.
    #[serde(default)]
    pub current: Option<CurrentDef>,
}

/// A [`Current`] on every water volume of the region: toward the middle of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurrentDef {
    /// Metres inside the grid's edge where the push starts.
    pub starts: f32,
    /// Metres inside the edge where it is full.
    pub full: f32,
}

/// A voxel region (`*.voxel_region.ron`): the volume's shape, the palette, the generation
/// steps (with the versions the content expects) and their params, the checks, the marks it
/// must have, the water. Every number is the content's: the engine's defaults are neutral (no
/// sea, no current, no surface drop).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoxelRegionDef {
    /// Cells along x, y, z: a whole number of `chunk`s on every axis.
    pub size: UVec3,
    /// The edge of a cell, m.
    #[serde(default = "one")]
    pub block: f32,
    /// Cells per chunk; square in x and z (the manifest's cells are the chunks).
    pub chunk: UVec3,
    /// The `voxel_palette` its blocks come from.
    pub palette: String,
    /// The generator version: raise it when the region's output changes for the same seed.
    #[serde(default = "one_u32")]
    pub version: u32,
    /// The cell whose least corner lands on `at` (default: cell (0, 0, 0)): a cell at layer
    /// `y` has its top at `at.y` when the anchor is at layer `y + 1`.
    #[serde(default)]
    pub anchor: Option<UVec3>,
    /// Where the anchor lands in the world, m.
    #[serde(default)]
    pub at: Vec3,
    /// Attempts before the generation gives up (or takes the fallback seed).
    #[serde(default = "one_u32")]
    pub attempts: u32,
    /// The steps, in order, with the version of each the content expects.
    pub steps: Vec<(String, u32)>,
    /// Each step's parameters by step name.
    #[serde(default)]
    pub params: BTreeMap<String, Value>,
    /// The checks the finished volume must pass, in order: one failing fails the attempt.
    #[serde(default)]
    pub checks: Vec<String>,
    /// Marks the steps must make: one missing fails the attempt. Every mark a step makes is a
    /// point of interest (`camp.1`: type `camp`), except the `markers`.
    #[serde(default)]
    pub marks: Vec<String>,
    /// Marks that become [`Marker`]s (an NPC's home, a link's end) instead of points of
    /// interest.
    #[serde(default)]
    pub markers: Vec<String>,
    /// The point of interest players appear at (a mark the steps must make).
    #[serde(default)]
    pub spawn: Option<String>,
    #[serde(default)]
    pub water: WaterDef,
    #[serde(default)]
    pub sea: Option<SeaDef>,
    /// Within this many metres of the grid's edge, or past it, the `near_grid_edge` condition
    /// holds (0: past it only).
    #[serde(default)]
    pub near_edge: f32,
    /// The seed `--check-content` generates the region with, so a step that fails on every
    /// seed is a content error before any report runs.
    #[serde(default = "one_u64")]
    pub check_seed: u64,
    /// The seed whose first attempt stands in when no attempt of a seed passes (checked by
    /// `--check-content`); none: such a seed has no region.
    #[serde(default)]
    pub fallback_seed: Option<u64>,
    /// Characters farther than this from every player are dormant; none: the world settings'.
    #[serde(default)]
    pub active_radius: Option<f32>,
}

impl VoxelRegionDef {
    /// World position of the least corner of cell (0, 0, 0).
    pub fn origin(&self) -> Vec3 {
        self.at + VoxelVolume::origin_for_anchor(self.anchor.unwrap_or(UVec3::ZERO), self.block)
    }

    /// The marks every attempt must make: `marks`, then the spawn.
    pub fn required_marks(&self) -> Vec<String> {
        let mut marks = self.marks.clone();
        if let Some(spawn) = &self.spawn
            && !marks.contains(spawn)
        {
            marks.push(spawn.clone());
        }
        marks
    }

    /// What to generate for `seed`.
    pub fn spec(&self, seed: u64) -> GenSpec {
        GenSpec {
            size: self.size,
            block: self.block,
            origin: self.origin(),
            chunk: self.chunk,
            seed,
            version: self.version,
            attempts: self.attempts,
            steps: self.steps.clone(),
            params: self.params.clone(),
            checks: self.checks.clone(),
            marks: self.required_marks(),
        }
    }

    /// The grid in the ground plane (x, z), m.
    pub fn area(&self) -> Rect {
        let min = self.origin().xz();
        Rect::from_corners(min, min + Vec2::new(self.size.x as f32, self.size.z as f32) * self.block)
    }

    /// What is wrong with the definition by itself, one line each (the steps against the
    /// registry are checked apart, with it).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Err(e) = VoxelVolume::check_shape(self.size, self.block, self.origin(), self.chunk) {
            out.push(e);
        }
        if self.chunk.x != self.chunk.z {
            out.push(format!("chunk {} x {} x {}: square in x and z (the region's cells are its chunks)", self.chunk.x, self.chunk.y, self.chunk.z));
        }
        if self.chunk.y != self.size.y {
            out.push(format!(
                "chunk {} x {} x {}: as tall as the volume ({} cells): the region's cells are its chunks, one to a column",
                self.chunk.x, self.chunk.y, self.chunk.z, self.size.y
            ));
        }
        if let Some(anchor) = self.anchor
            && (anchor.x > self.size.x || anchor.y > self.size.y || anchor.z > self.size.z)
        {
            out.push(format!("anchor {anchor} is outside the volume ({} x {} x {} cells)", self.size.x, self.size.y, self.size.z));
        }
        if !self.at.is_finite() {
            out.push(format!("at {}: finite numbers", self.at));
        }
        if !(1..=MAX_ATTEMPTS).contains(&self.attempts) {
            out.push(format!("attempts: 1 to {MAX_ATTEMPTS}, not {}", self.attempts));
        }
        if self.version == 0 {
            out.push("version: from 1".to_owned());
        }
        for (field, list) in [("marks", &self.marks), ("markers", &self.markers)] {
            for (i, name) in list.iter().enumerate() {
                if name.is_empty() || name.chars().any(char::is_whitespace) {
                    out.push(format!("{field}[{i}] `{name}`: a name without spaces (`camp.1`)"));
                }
            }
        }
        if let Some(spawn) = &self.spawn
            && self.markers.contains(spawn)
        {
            out.push(format!("spawn `{spawn}` is one of the markers: players appear at a point of interest"));
        }
        let drop = self.water.surface_drop;
        if !(drop.is_finite() && drop >= 0.0 && drop < self.block) {
            out.push(format!("water.surface_drop {drop}: 0 or more and less than a block ({})", self.block));
        }
        if let Some(sea) = &self.sea {
            if !(sea.reach.is_finite() && sea.reach > 0.0 && sea.reach <= MAX_SEA_REACH) {
                out.push(format!("sea.reach {}: above 0 and at most {MAX_SEA_REACH} m (farther out a position loses its centimetres)", sea.reach));
            }
            if sea.level == 0 || sea.level > self.size.y {
                out.push(format!("sea.level {}: a layer from 1 to the volume's height ({})", sea.level, self.size.y));
            }
            if !(sea.floor.is_finite() && sea.floor >= 0.0 && sea.floor < sea.level as f32) {
                out.push(format!("sea.floor {}: a layer inside the volume and under sea.level ({})", sea.floor, sea.level));
            }
            if let Some(c) = &sea.current
                && let Some(problem) = (Current { area: Rect::new(0.0, 0.0, 1.0, 1.0), starts: c.starts, full: c.full }).problem()
            {
                out.push(format!("sea.current: {problem}"));
            }
        }
        if !(self.near_edge.is_finite() && self.near_edge >= 0.0) {
            out.push(format!("near_edge {}: 0 or more", self.near_edge));
        }
        if let Some(r) = self.active_radius
            && !(r.is_finite() && r > 0.0)
        {
            out.push(format!("active_radius {r}: above 0"));
        }
        out
    }

    /// The manifest's `content_hash`: the definition and the palette's semantics, hex.
    pub fn content_hash(db: &ContentDb, id: &str, palette: &VoxelPalette) -> String {
        let def = content_hash(db, &[(CONTENT, id)]);
        format!("{:016x}", fnv(format!("{def} {:016x}", palette.semantics()).as_bytes()))
    }
}

impl ContentDef for VoxelRegionDef {
    const KIND: &'static str = CONTENT;

    fn validate(&self, _: &str, refs: &mut Refs) {
        for problem in self.problems() {
            refs.error(problem);
        }
        refs.require::<VoxelPaletteDef>(&self.palette, "palette");
        // The steps and checks this game registered: a name it never registered, or another
        // version, is a content error here, not a surprise at the first `regen`.
        if let Some(steps) = refs.context::<VoxelSteps>() {
            let spec = self.spec(self.check_seed);
            for problem in steps.validate_steps(&spec.steps).into_iter().chain(steps.validate_checks(&spec.checks)) {
                refs.error(problem);
            }
            for key in self.params.keys() {
                if !self.steps.iter().any(|(name, _)| name == key) {
                    refs.error(format!("params: `{key}` is not a listed step"));
                }
            }
        }
    }
}

/// The voxel region in the world: its volume, palette and hash. Built with the region, from
/// its manifest; gone with it. Not simulation state: the region's manifest is (the snapshot
/// keeps it as the world module's `RegionBuild`), and this is made again from it.
#[derive(Resource, Clone, Debug)]
pub struct VoxelTerrain {
    /// The region definition's id.
    pub region: String,
    /// The manifest's seed, version and attempt (0: the fallback seed's volume).
    pub seed: u64,
    pub version: u32,
    pub attempt: u32,
    /// The seed the volume was generated with (the fallback's when `attempt` is 0).
    pub made_with: u64,
    pub steps: Vec<(String, u32)>,
    pub volume: Arc<VoxelVolume>,
    pub palette: Arc<VoxelPalette>,
    pub field: Arc<VoxelField>,
    /// The world hash ([`world_hash`](crate::world_hash)): what `voxel.volume` compares.
    pub hash: u64,
    /// The grid in the ground plane, m.
    pub area: Rect,
    pub water_boxes: usize,
    pub sea_boxes: usize,
    /// The current on its water, if the sea has one.
    pub current: Option<Current>,
    /// The definition's `near_edge`.
    pub near_edge: f32,
    /// How long making the volume, the field and the water boxes took (nearly nothing when
    /// `cached`).
    pub built_in: Duration,
    pub cached: bool,
}

impl VoxelTerrain {
    /// The signed distance (m) from `p` to the grid's edge: positive inside.
    pub fn edge_distance(&self, p: Vec3) -> f32 {
        shards_motion::edge_distance(self.area, p)
    }
}

/// How the voxel module builds regions.
#[derive(Resource, Clone, Copy, Debug)]
pub struct VoxelSettings {
    /// Keep built volumes in-process for the next build of the same region (snapshot loads,
    /// heavy-mode twins). Default on; `SHARDS_VOXEL_CACHE=off` turns it off.
    pub cache: bool,
}

impl Default for VoxelSettings {
    fn default() -> Self {
        let off = std::env::var("SHARDS_VOXEL_CACHE").is_ok_and(|v| matches!(v.trim(), "off" | "0" | "false" | "no"));
        Self { cache: !off }
    }
}

/// What a build makes from a manifest that does not depend on the world it goes into.
#[derive(Debug)]
struct Built {
    volume: Arc<VoxelVolume>,
    field: Arc<VoxelField>,
    water: Vec<CellBox>,
    marks: Vec<Mark>,
    hash: u64,
}

/// Built volumes by key, the most recently used last.
static CACHE: Mutex<Vec<(String, Arc<Built>)>> = Mutex::new(Vec::new());

/// Empties the in-process cache of built volumes (tests: a cold build after a warm one).
pub fn clear_cache() {
    CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}

/// How many built volumes the cache holds.
pub fn cached_volumes() -> usize {
    CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len()
}

/// The region kind the voxel module registers with the world module.
pub fn kind() -> RegionKind {
    RegionKind {
        content: CONTENT,
        layout: Layout::Volume,
        noun: "chunks",
        ids: |db| db.ids::<VoxelRegionDef>().map(str::to_owned).collect(),
        version: |db, id| db.get::<VoxelRegionDef>(id).map(|d| d.version),
        generate,
        check,
        spawn,
        unload: |world| {
            world.remove_resource::<VoxelTerrain>();
        },
        active_radius: |db, id| db.get::<VoxelRegionDef>(id).and_then(|d| d.active_radius),
        summary,
        content_check,
        data: None,
    }
}

/// `--check-content`: the region generates for its `check_seed` (on some attempt, not by the
/// fallback), and its `fallback_seed`'s first attempt passes.
fn content_check(db: &ContentDb, worldgen: &WorldGen, id: &str) -> Vec<String> {
    let (def, steps) = match (def_of(db, id), registry(worldgen)) {
        (Ok(def), Ok(steps)) => (def, steps),
        (Err(e), _) | (_, Err(e)) => return vec![e],
    };
    let palette = match VoxelPalette::from_db(db, &def.palette) {
        Ok(palette) => palette,
        Err(e) => return vec![e],
    };
    let mut out = Vec::new();
    if let Err(errors) = generate_attempts(steps, &palette, &def.spec(def.check_seed)) {
        let reasons: Vec<String> = errors.iter().map(|e| format!("attempt {}: {e}", e.attempt + 1)).collect();
        out.push(format!("check_seed {}: no attempt generates ({}); fix the step or check, or pick another check_seed", def.check_seed, reasons.join("; ")));
    }
    if let Some(fallback) = def.fallback_seed
        && let Err(e) = generate_attempt(steps, &palette, &def.spec(fallback), 0)
    {
        out.push(format!("fallback_seed {fallback}: its first attempt fails ({e}), so it cannot stand in for a seed that fails; pick a seed that passes"));
    }
    out
}

/// The step registry the world module's generator was given ([`RegionKind::data`]).
fn registry(worldgen: &WorldGen) -> Result<&VoxelSteps, String> {
    worldgen
        .kind(CONTENT)
        .and_then(|k| k.data::<VoxelSteps>())
        .ok_or_else(|| "the voxel module did not give the world module its steps (is VoxelPlugin added after WorldPlugin?)".to_owned())
}

/// The definition of `id`, or why there is none.
fn def_of<'a>(db: &'a ContentDb, id: &str) -> Result<&'a VoxelRegionDef, String> {
    db.get::<VoxelRegionDef>(id).ok_or_else(|| format!("voxel region `{id}` is not in the content"))
}

/// A failed attempt as the world module reports it (attempts from 1).
fn failure(e: &AttemptError) -> AttemptFailure {
    let (stage, step) = match e.stage {
        AttemptStage::Step => (Stage::Fill, e.name.clone()),
        _ => (Stage::Checks, "voxel.checks".to_owned()),
    };
    AttemptFailure { attempt: e.attempt + 1, stage, step, rule: format!("{} {}", e.stage.name(), e.name), message: e.to_string(), cells: Vec::new() }
}

/// The region kind's generator: attempts until one passes, else the fallback seed's first.
fn generate(db: &ContentDb, worldgen: &WorldGen, id: &str, seed: u64) -> Result<RegionGenerated, GenError> {
    let def = def_of(db, id)?;
    let steps = registry(worldgen)?;
    let palette = VoxelPalette::from_db(db, &def.palette)?;
    let hash = VoxelRegionDef::content_hash(db, id, &palette);
    let errors = match generate_attempts(steps, &palette, &def.spec(seed)) {
        Ok(made) => {
            let manifest = manifest(def, id, seed, made.attempt + 1, None, &made.marks, hash);
            let counts = counts(&made.volume, &palette, made.marks.len());
            return Ok(RegionGenerated { manifest, failures: Vec::new(), remarks: made.remarks, counts });
        }
        Err(errors) => errors,
    };
    if let [only] = errors.as_slice()
        && only.stage == AttemptStage::Spec
    {
        return Err(GenError::from(format!("voxel region `{id}`: {}", only.message)));
    }
    let failures: Vec<AttemptFailure> = errors.iter().map(failure).collect();
    let last = failures.last().map(|f| f.message.clone()).unwrap_or_default();
    let Some(fallback) = def.fallback_seed else {
        let message = format!(
            "voxel region `{id}` seed {seed}: no attempt passed ({} attempts); the last: {last}; give the region a `fallback_seed` that passes, or more attempts",
            def.attempts
        );
        return Err(GenError { message, failures });
    };
    match generate_attempt(steps, &palette, &def.spec(fallback), 0) {
        Ok(made) => {
            let mut remarks = made.remarks;
            remarks.push(format!("no attempt of seed {seed} passed: the fallback seed {fallback}"));
            let manifest = manifest(def, id, seed, 0, Some(fallback), &made.marks, hash);
            let counts = counts(&made.volume, &palette, made.marks.len());
            Ok(RegionGenerated { manifest, failures, remarks, counts })
        }
        Err(e) => Err(GenError { message: format!("voxel region `{id}` seed {seed}: no attempt passed (the last: {last}), and the fallback seed {fallback} fails too: {e}"), failures }),
    }
}

/// What the worldgen report sums: solid cells, water cells and columns, water boxes, marks.
fn counts(volume: &VoxelVolume, palette: &VoxelPalette, marks: usize) -> BTreeMap<String, u64> {
    let s = volume.size();
    let mut columns = 0u64;
    for x in 0..s.x as i32 {
        for z in 0..s.z as i32 {
            columns += u64::from(volume.column_top(x, z, |id| palette.liquid(id)) > 0);
        }
    }
    BTreeMap::from([
        ("solid cells".to_owned(), volume.count(|id| palette.solid(id)) as u64),
        ("water cells".to_owned(), volume.count(|id| palette.liquid(id)) as u64),
        ("water columns".to_owned(), columns),
        ("water boxes".to_owned(), water_boxes(volume, |id| palette.liquid(id), |id| palette.solid(id)).len() as u64),
        ("marks".to_owned(), marks as u64),
    ])
}

/// The manifest of a volume made for `seed` (by attempt `attempt`, from 1; 0 with the
/// fallback seed `fallback`).
fn manifest(def: &VoxelRegionDef, id: &str, seed: u64, attempt: u32, fallback: Option<u64>, marks: &[Mark], content_hash: String) -> RegionManifest {
    let (size, tile_size, origin) = grid_of(def);
    let pois = marks
        .iter()
        .filter(|m| !def.markers.contains(&m.name))
        .map(|m| ManifestPoi { id: m.name.clone(), poi: RegionManifest::poi_type(&m.name).to_owned(), cell: chunk_of(def, m.pos), pos: mm(m.pos), area: 0 })
        .collect();
    RegionManifest {
        format: FORMAT.to_owned(),
        region: id.to_owned(),
        layout: Layout::Volume,
        seed,
        version: def.version,
        attempt,
        fallback: fallback.map(|s| format!("seed {s}")),
        size,
        tile_size,
        origin,
        tiles: Vec::new(),
        pois,
        links: Vec::new(),
        spawn: def.spawn.clone(),
        content_hash,
        steps: def.steps.clone(),
    }
}

/// The grid a definition's manifest has: chunks along x and z, a chunk's edge (m) and the grid's
/// north-west corner, as the manifest stores them.
fn grid_of(def: &VoxelRegionDef) -> ([u32; 2], f32, [f32; 2]) {
    let corner = mm(def.origin());
    ([def.size.x / def.chunk.x, def.size.z / def.chunk.z], mm(Vec3::splat(def.chunk.x as f32 * def.block))[0], [corner[0], corner[2]])
}

/// The chunk (x, z) of the grid a point is over: the manifest's cell of a mark. Generation keeps
/// marks over the grid ([`generate_attempt`] fails an attempt with one outside), so only a mark
/// exactly on the far edge needs the clamp: it belongs to the last chunk.
fn chunk_of(def: &VoxelRegionDef, pos: Vec3) -> [i32; 2] {
    let (origin, edge) = (def.origin(), def.chunk.x as f32 * def.block);
    let last = [(def.size.x / def.chunk.x) as i32 - 1, (def.size.z / def.chunk.z) as i32 - 1];
    [((pos.x - origin.x) / edge).floor().clamp(0.0, last[0] as f32) as i32, ((pos.z - origin.z) / edge).floor().clamp(0.0, last[1] as f32) as i32]
}

/// The seed and attempt (from 0) whose volume `manifest` names.
fn made_with(manifest: &RegionManifest) -> Result<(u64, u32), String> {
    if manifest.attempt > 0 {
        return Ok((manifest.seed, manifest.attempt - 1));
    }
    let seed = manifest.fallback.as_deref().and_then(|f| f.strip_prefix("seed ")).and_then(|s| s.parse().ok());
    seed.map(|s| (s, 0)).ok_or_else(|| format!("{}: attempt 0 names no fallback seed (`fallback: \"seed N\"`)", manifest.title()))
}

/// Whether content and this build can make `manifest`'s volume again, changing nothing.
fn check(world: &World, manifest: &RegionManifest) -> Result<(), String> {
    let title = manifest.title();
    let db = world.resource::<ContentDb>();
    let def = def_of(db, &manifest.region)?;
    let steps = world.get_resource::<VoxelSteps>().ok_or("the voxel module is not in this game")?;
    // The step code first: the one thing content cannot show.
    for (name, version) in &manifest.steps {
        match steps.version(name) {
            Some(now) if now == *version => {}
            Some(now) => {
                return Err(format!(
                    "{title} was made by voxel step `{name}` v{version}, and this build has `{name}` v{now}: its volume would come out otherwise, so it is not built; keep step `{name}` at v{version}, or generate the region again"
                ));
            }
            None => return Err(format!("{title} was made by voxel step `{name}` v{version}, which this build does not have: its volume cannot be made again")),
        }
    }
    if manifest.steps != def.steps {
        let list = |s: &[(String, u32)]| s.iter().map(|(n, v)| format!("{n} v{v}")).collect::<Vec<_>>().join(", ");
        return Err(format!("{title} was made by the steps {}, and its content now lists {}: generate the region again", list(&manifest.steps), list(&def.steps)));
    }
    if manifest.version != def.version {
        return Err(format!("{title} is of generator v{}, and its content is at v{} now: generate the region again", manifest.version, def.version));
    }
    let palette = VoxelPalette::from_db(db, &def.palette)?;
    let now = VoxelRegionDef::content_hash(db, &manifest.region, &palette);
    if now != manifest.content_hash {
        return Err(format!(
            "{title}: its definition or palette changed since it was made (content hash {} then, {now} now), so its volume would come out otherwise; generate it again",
            manifest.content_hash
        ));
    }
    // The grid the manifest says it has must be the one its definition makes: a manifest edited
    // by hand (a saved or snapshot file) is refused here, before anything changes.
    let (size, tile_size, origin) = grid_of(def);
    if (manifest.size, manifest.tile_size, manifest.origin) != (size, tile_size, origin) {
        return Err(format!(
            "{title} says its grid is {:?} chunks of {} m from {:?}, and its definition makes {size:?} chunks of {tile_size} m from {origin:?}: the manifest was changed; generate the region again",
            manifest.size, manifest.tile_size, manifest.origin
        ));
    }
    if let Some(poi) = manifest.pois.iter().find(|p| !(0..size[0] as i32).contains(&p.cell[0]) || !(0..size[1] as i32).contains(&p.cell[1])) {
        return Err(format!("{title}: point of interest `{}` is in chunk {:?}, outside its {size:?} chunks: the manifest was changed", poi.id, poi.cell));
    }
    if manifest.links.len() + manifest.tiles.len() > 0 {
        return Err(format!("{title}: a volume has no tiles or links, and the manifest names some: the manifest was changed"));
    }
    let (made, attempt) = made_with(manifest)?;
    let fits = if manifest.attempt > 0 { manifest.fallback.is_none() && attempt < def.attempts } else { Some(made) == def.fallback_seed };
    if !fits {
        return Err(format!(
            "{title}: attempt {} with fallback {:?} is not one its definition makes ({} attempts, fallback seed {:?}): the manifest was changed",
            manifest.attempt, manifest.fallback, def.attempts, def.fallback_seed
        ));
    }
    Ok(())
}

/// The volume, field and water of `manifest`: from the cache, or made again.
fn build(world: &World, manifest: &RegionManifest) -> Result<(Arc<Built>, bool), String> {
    let db = world.resource::<ContentDb>();
    let def = def_of(db, &manifest.region)?;
    let steps = world.get_resource::<VoxelSteps>().ok_or("the voxel module is not in this game")?;
    let palette = VoxelPalette::from_db(db, &def.palette)?;
    let (seed, attempt) = made_with(manifest)?;
    let spec = def.spec(seed);
    let cache = world.get_resource::<VoxelSettings>().copied().unwrap_or_default().cache;
    let key = format!("{}\n{:x?}", manifest.to_canonical(), steps.code_of(&spec.steps, &spec.checks));
    if cache {
        let mut entries = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(i) = entries.iter().position(|(k, _)| *k == key) {
            let entry = entries.remove(i);
            let built = entry.1.clone();
            entries.push(entry);
            return Ok((built, true));
        }
    }
    let made = generate_attempt(steps, &palette, &spec, attempt).map_err(|e| format!("{} cannot be made again: {e}", manifest.title()))?;
    let volume = made.volume;
    let field = VoxelField::new(volume.origin(), volume.block(), volume.size(), |c| palette.solid(volume.get(c)));
    let water = water_boxes(&volume, |id| palette.liquid(id), |id| palette.solid(id));
    let built = Arc::new(Built { volume: Arc::new(volume), field: Arc::new(field), water, marks: made.marks, hash: made.hash });
    if cache {
        let mut entries = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|(k, _)| *k != key);
        entries.push((key, built.clone()));
        while entries.len() > CACHE_SIZE {
            entries.remove(0);
        }
    }
    Ok((built, false))
}

/// Spawns a water volume `half` from `centre`, with the region's current.
fn spawn_water(world: &mut World, name: String, centre: Vec3, half: Vec3, current: Option<Current>) -> Entity {
    let mut water = world.spawn((
        Name::new(name),
        Collider::new(Shape::cuboid(half)).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_translation(centre),
    ));
    if let Some(current) = current {
        water.insert(current);
    }
    water.id()
}

/// The four sea boxes around `area` (north, south, west, east), `reach` out, from `floor` up to
/// `top`, as (name, centre, half extents).
pub fn sea_boxes(area: Rect, reach: f32, floor: f32, top: f32) -> [(&'static str, Vec3, Vec3); 4] {
    let (min, max) = (area.min, area.max);
    let (y, hy) = ((floor + top) * 0.5, (top - floor) * 0.5);
    let wide = (max.x - min.x) * 0.5 + reach;
    let mid_x = (min.x + max.x) * 0.5;
    let mid_z = (min.y + max.y) * 0.5;
    let half_reach = reach * 0.5;
    [
        ("north", Vec3::new(mid_x, y, min.y - half_reach), Vec3::new(wide, hy, half_reach)),
        ("south", Vec3::new(mid_x, y, max.y + half_reach), Vec3::new(wide, hy, half_reach)),
        ("west", Vec3::new(min.x - half_reach, y, mid_z), Vec3::new(half_reach, hy, (max.y - min.y) * 0.5)),
        ("east", Vec3::new(max.x + half_reach, y, mid_z), Vec3::new(half_reach, hy, (max.y - min.y) * 0.5)),
    ]
}

/// Builds `manifest`'s terrain, water and markers (the world module adds the points of
/// interest after them), and the [`VoxelTerrain`].
fn spawn(world: &mut World, manifest: &RegionManifest) -> Result<Vec<Entity>, String> {
    let started = Instant::now();
    let (built, cached) = build(world, manifest)?;
    let built_in = started.elapsed();
    let (def, palette) = {
        let db = world.resource::<ContentDb>();
        let def = def_of(db, &manifest.region)?.clone();
        let palette = VoxelPalette::from_db(db, &def.palette)?;
        (def, palette)
    };
    let (seed, _) = made_with(manifest)?;
    let area = def.area();
    let current = def.sea.and_then(|s| s.current).map(|c| Current { area, starts: c.starts, full: c.full });
    let mut out = vec![world.spawn((Name::new(format!("voxel terrain {}", manifest.region)), VoxelCollider { field: built.field.clone() }, Transform::default())).id()];
    let drop = def.water.surface_drop;
    for (i, b) in built.water.iter().enumerate() {
        let (centre, half) = b.world(&built.volume);
        let (top, bottom) = (centre.y + half.y - drop, centre.y - half.y);
        let (centre, half) = (Vec3::new(centre.x, (top + bottom) * 0.5, centre.z), Vec3::new(half.x, (top - bottom) * 0.5, half.z));
        out.push(spawn_water(world, format!("voxel water {i}"), centre, half, current));
    }
    let mut seas = 0;
    if let Some(sea) = def.sea {
        let base = built.volume.origin().y;
        let (floor, top) = (base + sea.floor * def.block, base + sea.level as f32 * def.block - drop);
        for (side, centre, half) in sea_boxes(area, sea.reach, floor, top) {
            out.push(spawn_water(world, format!("voxel sea {side}"), centre, half, current));
            seas += 1;
        }
    }
    for mark in built.marks.iter().filter(|m| def.markers.contains(&m.name)) {
        // In its chunk, as a marker of a tile is in its tile.
        let [x, y] = chunk_of(&def, mark.pos);
        let marker = (Name::new(mark.name.clone()), Marker { name: mark.name.clone() }, TileId { x, y }, Transform::from_translation(mark.pos));
        out.push(world.spawn(marker).id());
    }
    world.insert_resource(VoxelTerrain {
        region: manifest.region.clone(),
        seed: manifest.seed,
        version: manifest.version,
        attempt: manifest.attempt,
        made_with: seed,
        steps: manifest.steps.clone(),
        volume: built.volume.clone(),
        palette: Arc::new(palette),
        field: built.field.clone(),
        hash: built.hash,
        area,
        water_boxes: built.water.len(),
        sea_boxes: seas,
        current,
        near_edge: def.near_edge,
        built_in,
        cached,
    });
    Ok(out)
}

/// `128 x 32 x 128, 3 water boxes` (and the sea): what the `region loaded` line says.
fn summary(world: &World) -> String {
    let Some(t) = world.get_resource::<VoxelTerrain>() else { return "no voxel terrain".to_owned() };
    let s = t.volume.size();
    let boxes = if t.water_boxes == 1 { "box" } else { "boxes" };
    let mut text = format!("{} x {} x {}, {} water {boxes}", s.x, s.y, s.z, t.water_boxes);
    if t.sea_boxes > 0 {
        text.push_str(&format!(", {} sea boxes", t.sea_boxes));
    }
    text
}

/// `voxel.volume` for replays and snapshots: the hash of the voxel region in the world.
pub fn volume_check(world: &World) -> Option<u64> {
    world.get_resource::<VoxelTerrain>().map(|t| t.hash)
}

/// After a content change is applied (`content_apply`, `content_reload`, a file edited on
/// disk): the voxel region in the world is not patched in place. When what it was made from
/// changed (its manifest's content hash: the definition, its params, the palette's blocks), it
/// is generated again for its seed and changed to, as `regen` does; players keep their places
/// where there is still ground under them, the others go to the spawn. A change the picture
/// alone reads (colours, looks) changes no hash: only the palette the picture reads is
/// replaced. A change that no longer generates leaves the region as it was built, and says so.
/// On a co-op client (it does not simulate) nothing is made again: the region is the host's.
/// What it did, when it did something, is journaled and is a line of the call's answer
/// (`voxel region isle seed 7 v1 regenerated (hash … -> …)`).
pub(crate) fn content_applied(world: &mut World) -> Option<String> {
    let say = |world: &mut World, kind: JournalKind, line: String| {
        journal::emit(world, kind, line.clone());
        Some(line)
    };
    let old = world.get_resource::<VoxelTerrain>().map(|t| t.hash)?;
    let manifest = world.get_resource::<ActiveRegion>().map(|r| r.manifest.clone())?;
    if manifest.layout != Layout::Volume {
        return None;
    }
    let title = manifest.title();
    let found = {
        let db = world.resource::<ContentDb>();
        def_of(db, &manifest.region).and_then(|def| {
            let palette = VoxelPalette::from_db(db, &def.palette)?;
            Ok((VoxelRegionDef::content_hash(db, &manifest.region, &palette), palette, def.block))
        })
    };
    let (now, palette, block) = match found {
        Ok(found) => found,
        Err(e) => return say(world, JournalKind::Warn, format!("voxel region {title}: {e}; the region in the world stays as it was built")),
    };
    if now == manifest.content_hash {
        world.resource_mut::<VoxelTerrain>().palette = Arc::new(palette);
        return None;
    }
    // A co-op client is shown the host's world: it never changes region by itself.
    if world.get_resource::<shards_core::NetRole>().is_some_and(|role| !role.simulates()) {
        return say(world, JournalKind::Warn, format!("voxel region {title}: content changed on this machine, which does not simulate; the host's region stays"));
    }
    let generated = {
        let (db, worldgen) = (world.resource::<ContentDb>(), world.resource::<WorldGen>());
        shards_world::generate::generate(db, worldgen, &manifest.region, manifest.seed)
    };
    let generated = match generated {
        Ok(generated) => generated,
        Err(e) => {
            return say(world, JournalKind::Warn, format!("voxel region {title}: the changed content does not generate ({e}); the region in the world stays as it was built"));
        }
    };
    let bodies: Vec<(Entity, Vec3)> =
        shards_world::region::players(world).into_iter().filter_map(|e| Some((e, world.get::<Transform>(e)?.translation))).collect();
    shards_world::region::journal_generated(world, &generated);
    if let Err(e) = shards_world::change_region(world, generated.manifest) {
        return say(world, JournalKind::Warn, format!("voxel region {title}: cannot build it again ({e})"));
    }
    for (body, at) in bodies {
        if let Some(y) = shards_world::region::ground_below(world, at, &[body])
            && (y - at.y).abs() <= block
            && let Some(mut tf) = world.get_mut::<Transform>(body)
        {
            tf.translation = Vec3::new(at.x, y, at.z);
        }
    }
    let new = world.get_resource::<VoxelTerrain>().map_or(0, |t| t.hash);
    say(world, JournalKind::Event, format!("voxel region {title} regenerated (hash {old:016x} -> {new:016x})"))
}
