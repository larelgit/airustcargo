//! The generator: a pipeline of named steps per [`Stage`], attempts, checks and a fallback.
//!
//! ```text
//! seed ─► Points (required points) ─► Roads ─► Cells (required cells) ─► Fill (socket solver)
//!      ─► Secondary (more points) ─► Decorate ─► Checks ─► manifest
//!                     any step fails or a check fails ─► next attempt ─► … ─► fallback tile map
//! ```
//!
//! Steps are plain functions on a [`GenContext`]: the engine registers one per stage
//! (`engine.points`, `engine.roads`, ...) so a game can work with data alone, and a game
//! adds its own after them or replaces a stage
//! ([`WorldAppExt`](crate::WorldAppExt)). Generation is a pure function of the seed, the
//! region's generator version, the attempt and the content: every step draws from
//! [`GenContext::rng`], keyed by those and the step's name, never from the tick's `SimRng`.
//!
//! A step fails with `Err("reason: details")`; the part before the first colon groups
//! failures in [batch reports](crate::report).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};
use std::fmt;

use ai_core::DetRng;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::ContentDb;

use crate::checks::{self, CheckFailure, CheckInput, RegionGraph};
use crate::content::{resolve_set, DungeonDef, PoiDef, RegionDef, SocketRulesDef, TileDef, TileMapDef};
use crate::grid::{degrees_of, mm, turn, turns_of, Dir};
use crate::manifest::{content_hash, fnv, Layout, ManifestLink, ManifestPoi, PlacedTile, RegionManifest, FORMAT};
use crate::sockets::{parse_edge, Rules};
use crate::solver::{and_assign, has, intersects, is_empty, ones, subset, LimitBits, SolveError, Solver, TileInfo, TileSet};

/// The stages of generation, in the order they run.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// The required points of interest: which cells hold them.
    Points,
    /// Roads between the required points.
    Roads,
    /// What must be where: roads, minimum counts.
    Cells,
    /// The socket solver fills every cell.
    Fill,
    /// More points of interest, in free slots.
    Secondary,
    /// Decoration picked per tile.
    Decorate,
    /// The region's checks.
    Checks,
}

impl Stage {
    pub const ALL: [Stage; 7] = [Stage::Points, Stage::Roads, Stage::Cells, Stage::Fill, Stage::Secondary, Stage::Decorate, Stage::Checks];

    pub fn name(self) -> &'static str {
        match self {
            Stage::Points => "points",
            Stage::Roads => "roads",
            Stage::Cells => "cells",
            Stage::Fill => "fill",
            Stage::Secondary => "secondary",
            Stage::Decorate => "decorate",
            Stage::Checks => "checks",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A generation step. `Err("reason: details")` fails the attempt.
pub type StepFn = fn(&mut GenContext) -> Result<(), String>;

/// A game's check: the failures it finds (none: passed).
pub type CheckFn = fn(&CheckInput) -> Vec<CheckFailure>;

/// One named step.
#[derive(Clone, Debug)]
pub struct Step {
    pub stage: Stage,
    pub name: String,
    pub run: StepFn,
}

/// The steps of every stage and the game's named checks. Starts with the engine's own
/// steps; [`WorldAppExt`](crate::WorldAppExt) changes it.
#[derive(Resource, Clone, Debug)]
pub struct WorldGen {
    steps: Vec<Step>,
    checks: BTreeMap<String, CheckFn>,
}

impl Default for WorldGen {
    fn default() -> Self {
        let engine: [(Stage, &str, StepFn); 7] = [
            (Stage::Points, "engine.points", place_points),
            (Stage::Roads, "engine.roads", plan_roads),
            (Stage::Cells, "engine.cells", require_cells),
            (Stage::Fill, "engine.fill", fill),
            (Stage::Secondary, "engine.secondary", place_secondary),
            (Stage::Decorate, "engine.decorate", decorate),
            (Stage::Checks, "engine.checks", run_checks),
        ];
        let steps = engine.into_iter().map(|(stage, name, run)| Step { stage, name: name.to_owned(), run }).collect();
        Self { steps, checks: BTreeMap::new() }
    }
}

impl WorldGen {
    /// Adds a step after the stage's others.
    pub fn add_step(&mut self, stage: Stage, name: &str, run: StepFn) {
        self.steps.push(Step { stage, name: name.to_owned(), run });
    }

    /// Replaces every step of `stage` (the engine's included) with this one.
    pub fn replace_stage(&mut self, stage: Stage, name: &str, run: StepFn) {
        self.steps.retain(|s| s.stage != stage);
        self.add_step(stage, name, run);
    }

    /// A check regions name in `checks.custom`. A second one with the same name replaces it.
    pub fn add_check(&mut self, name: &str, check: CheckFn) {
        self.checks.insert(name.to_owned(), check);
    }

    /// The steps of `stage`, in order.
    pub fn steps(&self, stage: Stage) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(move |s| s.stage == stage)
    }

    pub fn check(&self, name: &str) -> Option<CheckFn> {
        self.checks.get(name).copied()
    }

    pub fn check_names(&self) -> Vec<&str> {
        self.checks.keys().map(String::as_str).collect()
    }
}

// --- results -----------------------------------------------------------------------------

/// Why one attempt failed.
#[derive(Clone, Debug, PartialEq)]
pub struct AttemptFailure {
    /// From 1.
    pub attempt: u32,
    pub stage: Stage,
    /// The step that failed (`engine.fill`), or `engine.checks` for check failures.
    pub step: String,
    /// What groups failures in reports: `check reachable`, `fill: no tile fits`.
    pub rule: String,
    pub message: String,
    pub cells: Vec<[i32; 2]>,
}

impl fmt::Display for AttemptFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "attempt {} failed: {}", self.attempt, self.message)
    }
}

/// A generated region and how it came about.
#[derive(Clone, Debug)]
pub struct Generated {
    pub manifest: RegionManifest,
    /// The attempts that failed before (all of them when the fallback was used).
    pub failures: Vec<AttemptFailure>,
    /// What went short of the data without failing: secondary points that found no slot,
    /// a fallback that does not pass the checks.
    pub remarks: Vec<String>,
}

/// No region came out: why, and every failed attempt.
#[derive(Clone, Debug)]
pub struct GenError {
    pub message: String,
    pub failures: Vec<AttemptFailure>,
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for GenError {
    fn from(message: String) -> Self {
        Self { message, failures: Vec::new() }
    }
}

// --- the context -------------------------------------------------------------------------

/// Where a point of interest sits in its tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Spot {
    /// One of the tile's `slots`.
    Slot(usize),
    /// One of the tile's own `pois`.
    Own(usize),
}

/// A point of interest's place, once its tile is known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoiPlace {
    /// World position.
    pub pos: Vec3,
    pub area: usize,
    pub spot: Spot,
}

/// A point of interest being planned.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedPoi {
    /// `poi` id.
    pub poi: String,
    pub cell: (u32, u32),
    /// Placed before the tiles: its cell holds a tile that takes it.
    pub required: bool,
    pub place: Option<PoiPlace>,
}

/// A road between two planned points (indices into [`GenContext::pois`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedLink {
    pub a: usize,
    pub b: usize,
    pub cells: Vec<(u32, u32)>,
}

/// One attempt at a grid region: what the steps read and build.
pub struct GenContext<'a> {
    /// Region id.
    pub region: &'a str,
    pub def: &'a RegionDef,
    pub db: &'a ContentDb,
    pub worldgen: &'a WorldGen,
    pub tiles: &'a TileSet,
    /// Definitions of `tiles.tiles`, in the same order.
    pub tile_defs: &'a [&'a TileDef],
    pub seed: u64,
    pub version: u32,
    /// From 0.
    pub attempt: u32,
    pub width: u32,
    pub height: u32,
    /// Options left per cell, row-major, `tiles.words` each.
    domains: Vec<u64>,
    /// What constrains each cell, for messages.
    notes: Vec<Vec<String>>,
    limits: Vec<LimitBits>,
    /// Points of interest so far, in manifest order.
    pub pois: Vec<PlannedPoi>,
    pub links: Vec<PlannedLink>,
    /// Roads were planned: every cell gets exactly the road sides in [`roads`](Self::roads).
    pub roads_planned: bool,
    /// Road sides per cell, row-major, N E S W.
    pub roads: Vec<[bool; 4]>,
    /// The option in every cell once filled.
    pub filled: Option<Vec<usize>>,
    /// Decoration prefab per cell.
    pub decor: Vec<Option<String>>,
    /// What the checks found.
    pub failures: Vec<CheckFailure>,
    pub remarks: Vec<String>,
    /// Salt of the step running now.
    step: u64,
}

impl<'a> GenContext<'a> {
    /// A blank grid: every option everywhere.
    fn blank(compiled: &'a Compiled<'a>, worldgen: &'a WorldGen, db: &'a ContentDb, seed: u64, attempt: u32) -> Self {
        let (w, h) = compiled.def.size;
        let cells = (w * h) as usize;
        let all = compiled.set.bits_where(|_, _| true);
        Self {
            region: &compiled.id,
            def: compiled.def,
            db,
            worldgen,
            tiles: &compiled.set,
            tile_defs: &compiled.defs,
            seed,
            version: compiled.def.version,
            attempt,
            width: w,
            height: h,
            domains: (0..cells).flat_map(|_| all.iter().copied()).collect(),
            notes: vec![Vec::new(); cells],
            limits: compiled.limits.clone(),
            pois: Vec::new(),
            links: Vec::new(),
            roads_planned: false,
            roads: vec![[false; 4]; cells],
            filled: None,
            decor: vec![None; cells],
            failures: Vec::new(),
            remarks: Vec::new(),
            step: 0,
        }
    }

    /// A random stream for the running step: keyed by seed, version, attempt and step.
    pub fn rng(&self) -> DetRng {
        DetRng::from_keys(&[self.seed, self.version as u64, self.attempt as u64, self.step])
    }

    /// Another stream of the running step, for independent decisions.
    pub fn rng_for(&self, salt: u64) -> DetRng {
        DetRng::from_keys(&[self.seed, self.version as u64, self.attempt as u64, self.step, salt])
    }

    pub fn cells(&self) -> usize {
        (self.width * self.height) as usize
    }

    /// Row-major index of `(x, y)`.
    pub fn index(&self, (x, y): (u32, u32)) -> usize {
        (y * self.width + x) as usize
    }

    pub fn cell(&self, index: usize) -> (u32, u32) {
        (index as u32 % self.width, index as u32 / self.width)
    }

    /// The neighbor of `cell` on side `dir`, if inside.
    pub fn neighbor(&self, (x, y): (u32, u32), dir: Dir) -> Option<(u32, u32)> {
        let (dx, dy) = dir.offset();
        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
        (nx >= 0 && ny >= 0 && (nx as u32) < self.width && (ny as u32) < self.height).then_some((nx as u32, ny as u32))
    }

    /// World position of a cell's center.
    pub fn center(&self, (x, y): (u32, u32)) -> Vec3 {
        let s = self.def.tile_size;
        Vec3::new((x as f32 + 0.5) * s, 0.0, (y as f32 + 0.5) * s)
    }

    fn domain(&self, cell: usize) -> &[u64] {
        let w = self.tiles.words;
        &self.domains[cell * w..(cell + 1) * w]
    }

    /// The options still possible in `cell`.
    pub fn options_at(&self, cell: (u32, u32)) -> Vec<usize> {
        ones(self.domain(self.index(cell))).collect()
    }

    /// Keeps only the options of `cell` for which `keep` holds; `why` says what asked
    /// (`road on N, S`) in the error when nothing is left.
    pub fn restrict(&mut self, cell: (u32, u32), why: impl Into<String>, keep: impl Fn(usize) -> bool) -> Result<(), String> {
        let bits = self.tiles.bits_where(|o, _| keep(o));
        let i = self.index(cell);
        self.notes[i].push(why.into());
        let w = self.tiles.words;
        and_assign(&mut self.domains[i * w..(i + 1) * w], &bits);
        if is_empty(self.domain(i)) {
            return Err(format!("no tile fits: ({}, {}) needs {}", cell.0, cell.1, self.notes[i].join("; ")));
        }
        Ok(())
    }

    /// Options whose tile can hold a point of interest of type `poi`.
    pub fn accepting(&self, poi: &str) -> Vec<u64> {
        self.tiles.bits_where(|_, o| self.tile_defs[o.tile].accepts(poi))
    }

    /// Whether `cell` can still hold a `poi`.
    pub fn can_hold(&self, cell: (u32, u32), poi: &str) -> bool {
        intersects(self.domain(self.index(cell)), &self.accepting(poi))
    }

    /// The filled tile of `cell`: its definition and quarter turns.
    pub fn tile_at(&self, cell: (u32, u32)) -> Option<(&'a str, &'a TileDef, u8)> {
        let option = *self.filled.as_ref()?.get(self.index(cell))?;
        let o = self.tiles.options[option];
        Some((self.tiles.tiles[o.tile].id.as_str(), self.tile_defs[o.tile], o.turns))
    }

    /// Fills every cell with the socket solver, then places the points of interest planned
    /// so far in their tiles (and adds the tiles' own points).
    pub fn solve(&mut self) -> Result<(), String> {
        let solver = Solver {
            set: self.tiles,
            width: self.width as usize,
            height: self.height as usize,
            limits: &self.limits,
            max_backtracks: self.def.backtracks,
        };
        let mut rng = self.rng();
        match solver.solve(self.domains.clone(), &mut rng) {
            Ok(solution) => self.set_filled(solution.options),
            Err(SolveError::Empty { cell }) => {
                let (x, y) = self.cell(cell);
                let why = if self.notes[cell].is_empty() { String::new() } else { format!(" ({})", self.notes[cell].join("; ")) };
                Err(format!("no tile fits: nothing can go at ({x}, {y}) next to its neighbors{why}"))
            }
            Err(e @ SolveError::Limit { .. }) => Err(format!("limits: {e}")),
            Err(e @ SolveError::Exhausted { .. }) => Err(format!("no tiling: {e}")),
            Err(e @ SolveError::GaveUp { .. }) => Err(format!("solver gave up: {e}; raise `backtracks` or loosen the constraints")),
        }
    }

    /// Takes a tiling (one option per cell) and places the points of interest in it.
    pub fn set_filled(&mut self, options: Vec<usize>) -> Result<(), String> {
        if options.len() != self.cells() {
            return Err(format!("fill: {} options for {} cells", options.len(), self.cells()));
        }
        self.filled = Some(options);
        self.place_pois()
    }

    /// Spots taken in each cell.
    fn used_spots(&self) -> BTreeSet<(usize, Spot)> {
        self.pois.iter().filter_map(|p| p.place.map(|place| (self.index(p.cell), place.spot))).collect()
    }

    /// The place of a `poi` in `cell`'s tile at `spot`.
    fn place_at(&self, cell: (u32, u32), spot: Spot) -> Option<PoiPlace> {
        let (_, def, turns) = self.tile_at(cell)?;
        let (at, area) = match spot {
            Spot::Slot(i) => def.slots.get(i).map(|s| (s.at, s.area))?,
            Spot::Own(i) => def.pois.get(i).map(|p| (p.at, p.area))?,
        };
        Some(PoiPlace { pos: Vec3::from_array(mm(self.center(cell) + turn(at, turns))), area, spot })
    }

    /// A free spot of `cell`'s tile for a `poi`: its own point of that type first, then a slot.
    pub fn free_spot(&self, cell: (u32, u32), poi: &str) -> Option<Spot> {
        let (_, def, _) = self.tile_at(cell)?;
        let used = self.used_spots();
        let i = self.index(cell);
        let own = def.pois.iter().enumerate().find(|(k, p)| p.poi == poi && !used.contains(&(i, Spot::Own(*k)))).map(|(k, _)| Spot::Own(k));
        own.or_else(|| {
            def.slots
                .iter()
                .enumerate()
                .find(|(k, s)| s.accepts.iter().any(|a| a == poi) && !used.contains(&(i, Spot::Slot(*k))))
                .map(|(k, _)| Spot::Slot(k))
        })
    }

    fn place_pois(&mut self) -> Result<(), String> {
        for i in 0..self.pois.len() {
            if self.pois[i].place.is_some() {
                continue;
            }
            let (cell, poi) = (self.pois[i].cell, self.pois[i].poi.clone());
            let spot = self.free_spot(cell, &poi).ok_or_else(|| {
                let tile = self.tile_at(cell).map_or("?", |(id, ..)| id);
                format!("no slot: `{poi}` at ({}, {}) found no free slot in tile `{tile}`", cell.0, cell.1)
            })?;
            self.pois[i].place = self.place_at(cell, spot);
        }
        // The tiles' own points nobody claimed, row-major, in file order.
        for index in 0..self.cells() {
            let cell = self.cell(index);
            let Some((_, def, _)) = self.tile_at(cell) else { continue };
            let used = self.used_spots();
            for (k, own) in def.pois.iter().enumerate() {
                if !used.contains(&(index, Spot::Own(k))) {
                    let place = self.place_at(cell, Spot::Own(k));
                    self.pois.push(PlannedPoi { poi: own.poi.clone(), cell, required: false, place });
                }
            }
        }
        Ok(())
    }

    pub fn remark(&mut self, text: impl Into<String>) {
        self.remarks.push(text.into());
    }

    /// The manifest of this attempt as it stands.
    pub fn manifest(&self, content_hash: &str) -> Result<RegionManifest, String> {
        let filled = self.filled.as_ref().ok_or("fill: no step filled the grid (Stage::Fill must call ctx.solve())")?;
        let tiles = filled
            .iter()
            .enumerate()
            .map(|(i, &option)| {
                let (x, y) = self.cell(i);
                let o = self.tiles.options[option];
                PlacedTile {
                    at: [x as i32, y as i32],
                    size: [1, 1],
                    tile: self.tiles.tiles[o.tile].id.clone(),
                    rot: degrees_of(o.turns),
                    decor: self.decor.get(i).cloned().flatten(),
                }
            })
            .collect();
        let mut counters: BTreeMap<&str, u32> = BTreeMap::new();
        let mut pois = Vec::with_capacity(self.pois.len());
        for p in &self.pois {
            let place = p.place.ok_or_else(|| format!("no slot: `{}` at ({}, {}) was never placed in its tile", p.poi, p.cell.0, p.cell.1))?;
            let n = counters.entry(p.poi.as_str()).or_default();
            pois.push(ManifestPoi {
                id: format!("{}.{n}", p.poi),
                poi: p.poi.clone(),
                cell: [p.cell.0 as i32, p.cell.1 as i32],
                pos: mm(place.pos),
                area: place.area,
            });
            *n += 1;
        }
        let links = self
            .links
            .iter()
            .map(|l| ManifestLink {
                kind: "road".into(),
                a: pois[l.a].id.clone(),
                b: pois[l.b].id.clone(),
                cells: l.cells.iter().map(|&(x, y)| [x as i32, y as i32]).collect(),
            })
            .collect();
        let spawn = pois.iter().find(|p| p.poi == self.def.spawn).map(|p| p.id.clone());
        Ok(RegionManifest {
            format: FORMAT.into(),
            region: self.region.to_owned(),
            layout: Layout::Grid,
            seed: self.seed,
            version: self.version,
            attempt: self.attempt + 1,
            fallback: None,
            size: [self.width, self.height],
            tile_size: self.def.tile_size,
            tiles,
            pois,
            links,
            spawn,
            content_hash: content_hash.to_owned(),
        })
    }
}

// --- compiling a region ------------------------------------------------------------------

/// A region's content made ready for many seeds: compile once, generate often.
pub struct Compiled<'a> {
    pub id: String,
    pub def: &'a RegionDef,
    pub set: TileSet,
    /// Definitions of `set.tiles`, same order.
    pub defs: Vec<&'a TileDef>,
    pub limits: Vec<LimitBits>,
    /// The content hash manifests of this region carry.
    pub content_hash: String,
}

/// Compiles region `id`: its tile set with the socket rules, the count limits, the hash of
/// the content involved.
pub fn compile<'a>(db: &'a ContentDb, id: &str, def: &'a RegionDef) -> Result<Compiled<'a>, String> {
    let rules_def = db
        .get::<SocketRulesDef>(&def.rules)
        .ok_or_else(|| format!("region `{id}`: no socket_rules `{}` (define it in a content/**/*.socket_rules.ron file)", def.rules))?;
    let all: Vec<(&str, &TileDef)> = db.iter::<TileDef>().collect();
    let ids = def.tile_ids(all.iter().copied()).map_err(|e| format!("region `{id}`: tiles: {e}"))?;
    if ids.is_empty() {
        return Err(format!("region `{id}`: its tile set is empty"));
    }
    let defs: Vec<&TileDef> = ids.iter().filter_map(|t| db.get::<TileDef>(t)).collect();
    let infos = ids.iter().zip(&defs).map(|(t, d)| TileInfo::from_def(t, d)).collect::<Result<Vec<_>, _>>()?;
    let set = TileSet::new(Rules::new(rules_def), infos);
    let mut limits = Vec::new();
    for (t, tile) in set.tiles.iter().enumerate() {
        if tile.min_count > 0 || tile.max_count.is_some() {
            let bits = set.bits_where(|_, o| o.tile == t);
            limits.push(LimitBits { name: format!("tile `{}`", tile.id), bits, min: tile.min_count, max: tile.max_count });
        }
    }
    let entries: Vec<(&str, &TileDef)> = ids.iter().copied().zip(defs.iter().copied()).collect();
    for limit in &def.limits {
        let of = resolve_set(std::slice::from_ref(&limit.of), entries.iter().copied(), |t: &TileDef| &t.tags, "tile in this region")
            .map_err(|e| format!("region `{id}`: limits: {e}"))?;
        let bits = set.bits_where(|_, o| of.contains(&set.tiles[o.tile].id.as_str()));
        limits.push(LimitBits { name: format!("limit `{}`", limit.of), bits, min: limit.min, max: limit.max });
    }
    let mut parts: Vec<(&str, &str)> = vec![("region", id), ("socket_rules", def.rules.as_str())];
    parts.extend(ids.iter().map(|t| ("tile", *t)));
    let mut pois: BTreeSet<&str> = def.pois.iter().chain(&def.secondary).map(|r| r.poi.as_str()).collect();
    pois.insert(def.spawn.as_str());
    for d in &defs {
        pois.extend(d.pois.iter().map(|p| p.poi.as_str()));
        pois.extend(d.slots.iter().flat_map(|s| s.accepts.iter().map(String::as_str)));
    }
    parts.extend(pois.into_iter().map(|p| ("poi", p)));
    if let Some(fallback) = &def.fallback {
        parts.push(("tile_map", fallback.as_str()));
        if let Some(map) = db.get::<TileMapDef>(fallback) {
            for (tile, _) in map.cells().unwrap_or_default() {
                if !ids.contains(&tile.as_str())
                    && let Some((t, _)) = db.iter::<TileDef>().find(|(t, _)| *t == tile)
                {
                    parts.push(("tile", t));
                }
            }
        }
    }
    let content_hash = content_hash(db, &parts);
    Ok(Compiled { id: id.to_owned(), def, set, defs, limits, content_hash })
}

// --- running -----------------------------------------------------------------------------

/// Generates region or dungeon `id` for `seed`: attempts until one passes, else the
/// fallback. Pure: the same content, steps and seed give the same manifest.
pub fn generate(db: &ContentDb, worldgen: &WorldGen, id: &str, seed: u64) -> Result<Generated, GenError> {
    if let Some(def) = db.get::<RegionDef>(id) {
        let compiled = compile(db, id, def)?;
        return generate_compiled(db, worldgen, &compiled, seed);
    }
    if let Some(def) = db.get::<DungeonDef>(id) {
        return crate::dungeon::generate(db, worldgen, id, def, seed);
    }
    Err(GenError::from(unknown_region(db, id)))
}

/// `unknown region `x` (did you mean `y`?); regions: …`.
pub fn unknown_region(db: &ContentDb, id: &str) -> String {
    let known: Vec<&str> = db.ids::<RegionDef>().chain(db.ids::<DungeonDef>()).collect();
    let hint = closest(id, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    if known.is_empty() {
        format!("unknown region `{id}`: there is no `region` or `dungeon` content (content/**/*.region.ron)")
    } else {
        format!("unknown region `{id}`{hint}; regions: {}", known.join(", "))
    }
}

/// [`generate`] for a compiled grid region.
pub fn generate_compiled(db: &ContentDb, worldgen: &WorldGen, compiled: &Compiled, seed: u64) -> Result<Generated, GenError> {
    let def = compiled.def;
    let mut failures = Vec::new();
    for attempt in 0..def.attempts {
        let mut ctx = GenContext::blank(compiled, worldgen, db, seed, attempt);
        match run_attempt(&mut ctx, compiled) {
            Ok(manifest) => {
                let remarks = std::mem::take(&mut ctx.remarks);
                return Ok(Generated { manifest, failures, remarks });
            }
            Err(failure) => failures.push(failure),
        }
    }
    let attempts = def.attempts;
    let Some(fallback) = &def.fallback else {
        let last = failures.last().map(|f| format!("; the last: {}", f.message)).unwrap_or_default();
        return Err(GenError {
            message: format!(
                "region `{}` seed {seed}: no attempt of {attempts} passed{last}; give it a `fallback` tile map, more `attempts` or looser rules",
                compiled.id
            ),
            failures,
        });
    };
    let (manifest, problems) = fallback_manifest(db, worldgen, compiled, fallback, seed).map_err(|e| GenError { message: e, failures: failures.clone() })?;
    let mut remarks = vec![format!("no attempt of {attempts} passed; used the fallback `{fallback}`")];
    remarks.extend(problems.iter().map(|p| format!("the fallback `{fallback}` fails {p}")));
    Ok(Generated { manifest, failures, remarks })
}

/// Runs every stage on `ctx`; the manifest, or why the attempt failed.
fn run_attempt(ctx: &mut GenContext, compiled: &Compiled) -> Result<RegionManifest, AttemptFailure> {
    let attempt = ctx.attempt + 1;
    let fail = |stage: Stage, step: &str, message: String, cells: Vec<[i32; 2]>| {
        let reason = message.split(':').next().unwrap_or(&message).trim().to_owned();
        AttemptFailure { attempt, stage, step: step.to_owned(), rule: format!("{stage}: {reason}"), message: format!("{stage}: {message}"), cells }
    };
    apply_region_constraints(ctx).map_err(|e| fail(Stage::Cells, "engine.region", e, Vec::new()))?;
    for stage in Stage::ALL {
        let steps: Vec<Step> = ctx.worldgen.steps(stage).cloned().collect();
        for step in steps {
            ctx.step = fnv(format!("{}/{}", stage.name(), step.name).as_bytes());
            (step.run)(ctx).map_err(|e| fail(stage, &step.name, e, Vec::new()))?;
        }
    }
    if let Some(first) = ctx.failures.first() {
        let others = ctx.failures.len() - 1;
        let more = if others > 0 { format!(" (+{others} more)") } else { String::new() };
        return Err(AttemptFailure {
            attempt,
            stage: Stage::Checks,
            step: "engine.checks".into(),
            rule: format!("check {}", first.rule),
            message: format!("{first}{more}"),
            cells: first.cells.clone(),
        });
    }
    ctx.manifest(&compiled.content_hash).map_err(|e| fail(Stage::Checks, "engine.manifest", e, Vec::new()))
}

/// The region's own data, before any step: the border socket and fixed tiles.
fn apply_region_constraints(ctx: &mut GenContext) -> Result<(), String> {
    if let Some(border) = &ctx.def.border {
        let edge = parse_edge(border).map_err(|e| format!("border: {e}"))?;
        let facing: Vec<Vec<u64>> = Dir::ALL.iter().map(|d| ctx.tiles.facing_bits(*d, &edge)).collect();
        for index in 0..ctx.cells() {
            let cell = ctx.cell(index);
            for dir in Dir::ALL {
                if ctx.neighbor(cell, dir).is_none() {
                    let bits = &facing[dir.index()];
                    ctx.restrict(cell, format!("border `{border}` on {dir}"), |o| has(bits, o))?;
                }
            }
        }
    }
    for fixed in &ctx.def.fixed {
        let tile = ctx.tiles.tile_index(&fixed.tile).ok_or_else(|| format!("fixed tile `{}` is not in the tile set", fixed.tile))?;
        let turns = fixed.rot.and_then(turns_of);
        let options = ctx.tiles.options.clone();
        ctx.restrict((fixed.at.0, fixed.at.1), format!("fixed to `{}`", fixed.tile), |o| {
            options[o].tile == tile && turns.is_none_or(|t| options[o].turns == t)
        })?;
    }
    Ok(())
}

// --- the engine's steps ------------------------------------------------------------------

/// `engine.points`: a cell for every required point of interest, at random among those
/// whose tiles can hold it, apart from the others as far as the region's `min` distances
/// ask (cell centers).
pub fn place_points(ctx: &mut GenContext) -> Result<(), String> {
    let mut rng = ctx.rng();
    let rules = ctx.def.pois.clone();
    for rule in &rules {
        for n in 0..rule.count {
            let allowed: Vec<(u32, u32)> = if rule.cells.is_empty() {
                (0..ctx.cells()).map(|i| ctx.cell(i)).collect()
            } else {
                rule.cells.clone()
            };
            let taken: BTreeSet<(u32, u32)> = ctx.pois.iter().map(|p| p.cell).collect();
            let candidates: Vec<(u32, u32)> = allowed
                .into_iter()
                .filter(|c| c.0 < ctx.width && c.1 < ctx.height && !taken.contains(c) && ctx.can_hold(*c, &rule.poi))
                .filter(|c| far_enough(ctx, &rule.poi, *c))
                .collect();
            let Some(&cell) = rng.pick(&candidates) else {
                return Err(format!("no cell: no free cell can hold `{}` #{} apart from the points placed so far", rule.poi, n + 1));
            };
            let accepting = ctx.accepting(&rule.poi);
            ctx.restrict(cell, format!("a slot for `{}`", rule.poi), |o| has(&accepting, o))?;
            ctx.pois.push(PlannedPoi { poi: rule.poi.clone(), cell, required: true, place: None });
        }
    }
    Ok(())
}

/// Whether a `poi` at `cell` keeps the `min` distances to the points placed so far.
fn far_enough(ctx: &GenContext, poi: &str, cell: (u32, u32)) -> bool {
    let here = ctx.center(cell);
    ctx.def.checks.distances.iter().filter_map(|r| r.min.map(|min| (r, min))).all(|(rule, min)| {
        ctx.pois.iter().all(|p| {
            let pair = (rule.a == poi && rule.b == p.poi) || (rule.b == poi && rule.a == p.poi);
            !pair || here.distance(ctx.center(p.cell)) >= min
        })
    })
}

/// `engine.roads`: with `roads: true`, joins the required points by a spanning tree of
/// roads (shortest first), each found by a cheapest path over random cell costs that
/// prefers roads already planned.
pub fn plan_roads(ctx: &mut GenContext) -> Result<(), String> {
    if !ctx.def.roads {
        return Ok(());
    }
    ctx.roads_planned = true;
    let required: Vec<usize> = (0..ctx.pois.len()).filter(|&i| ctx.pois[i].required).collect();
    if required.len() < 2 {
        return Ok(());
    }
    let mut rng = ctx.rng();
    let cells = ctx.cells();
    let base: Vec<u32> = (0..cells).map(|_| 1 + rng.below(3)).collect();
    // Which sides of each cell can still carry a road (fixed tiles, borders...).
    let roady: Vec<Vec<u64>> = Dir::ALL.iter().map(|d| ctx.tiles.bits_where(|o, _| ctx.tiles.rules.has_road(ctx.tiles.edge(o, *d)))).collect();
    let can_road: Vec<[bool; 4]> = (0..cells).map(|c| std::array::from_fn(|d| intersects(ctx.domain(c), &roady[d]))).collect();
    let poi_cells: Vec<usize> = ctx.pois.iter().map(|p| ctx.index(p.cell)).collect();
    // Prim over Manhattan distances: shortest link out of the tree first, ties by index.
    let manhattan = |a: usize, b: usize| {
        let (pa, pb) = (ctx.pois[a].cell, ctx.pois[b].cell);
        pa.0.abs_diff(pb.0) + pa.1.abs_diff(pb.1)
    };
    let mut tree = vec![required[0]];
    let mut edges = Vec::new();
    while tree.len() < required.len() {
        let best = tree
            .iter()
            .flat_map(|&a| required.iter().filter(|b| !tree.contains(b)).map(move |&b| (a, b)))
            .min_by_key(|&(a, b)| (manhattan(a, b), a, b));
        let Some((a, b)) = best else { break };
        tree.push(b);
        edges.push((a, b));
    }
    for (a, b) in edges {
        let (from, to) = (poi_cells[a], poi_cells[b]);
        let cost: Vec<u32> = (0..cells)
            .map(|c| {
                let on_road = ctx.roads[c].iter().any(|r| *r);
                let other_point = poi_cells.contains(&c) && c != from && c != to;
                if on_road { 1 } else { base[c] + if other_point { 8 } else { 0 } }
            })
            .collect();
        let path = cheapest_path(ctx, from, to, &cost, &can_road).ok_or_else(|| {
            let (fa, fb) = (ctx.pois[a].cell, ctx.pois[b].cell);
            format!(
                "no road: no way for a road from `{}` at ({}, {}) to `{}` at ({}, {}) over cells that can carry one",
                ctx.pois[a].poi, fa.0, fa.1, ctx.pois[b].poi, fb.0, fb.1
            )
        })?;
        for pair in path.windows(2) {
            let (c, n) = (ctx.cell(pair[0]), ctx.cell(pair[1]));
            let dir = Dir::ALL.into_iter().find(|d| ctx.neighbor(c, *d) == Some(n)).expect("path steps are neighbors");
            ctx.roads[pair[0]][dir.index()] = true;
            ctx.roads[pair[1]][dir.opposite().index()] = true;
        }
        ctx.links.push(PlannedLink { a, b, cells: path.iter().map(|&c| ctx.cell(c)).collect() });
    }
    Ok(())
}

/// Cheapest path from `from` to `to` (Dijkstra, ties by cell index), stepping only across
/// sides that can carry a road on both cells (`open`).
fn cheapest_path(ctx: &GenContext, from: usize, to: usize, cost: &[u32], open: &[[bool; 4]]) -> Option<Vec<usize>> {
    let cells = ctx.cells();
    let mut dist = vec![u32::MAX; cells];
    let mut prev = vec![usize::MAX; cells];
    let mut heap = BinaryHeap::new();
    dist[from] = 0;
    heap.push(Reverse((0u32, from)));
    while let Some(Reverse((d, c))) = heap.pop() {
        if c == to {
            break;
        }
        if d > dist[c] {
            continue;
        }
        for dir in Dir::ALL {
            let Some(n) = ctx.neighbor(ctx.cell(c), dir) else { continue };
            let n = ctx.index(n);
            if !open[c][dir.index()] || !open[n][dir.opposite().index()] {
                continue;
            }
            let nd = d + cost[n];
            if nd < dist[n] {
                dist[n] = nd;
                prev[n] = c;
                heap.push(Reverse((nd, n)));
            }
        }
    }
    if dist[to] == u32::MAX {
        return None;
    }
    let mut path = vec![to];
    while *path.last().unwrap_or(&from) != from {
        path.push(prev[*path.last()?]);
    }
    path.reverse();
    Some(path)
}

/// `engine.cells`: roads where planned and nowhere else (with `roads: true`), and tiles with
/// a minimum count put in random cells that can take them.
pub fn require_cells(ctx: &mut GenContext) -> Result<(), String> {
    if ctx.roads_planned {
        for index in 0..ctx.cells() {
            let want = ctx.roads[index];
            let sides: Vec<String> = Dir::ALL.iter().filter(|d| want[d.index()]).map(ToString::to_string).collect();
            let why = if sides.is_empty() { "no road".to_owned() } else { format!("road on {} only", sides.join(", ")) };
            let tiles = ctx.tiles;
            ctx.restrict(ctx.cell(index), why, |o| Dir::ALL.iter().all(|d| tiles.rules.has_road(tiles.edge(o, *d)) == want[d.index()]))?;
        }
    }
    let mut rng = ctx.rng();
    let w = ctx.tiles.words;
    for limit in ctx.limits.clone() {
        if limit.min == 0 {
            continue;
        }
        let certain = (0..ctx.cells()).filter(|&c| subset(ctx.domain(c), &limit.bits)).count() as u32;
        let mut open: Vec<usize> = (0..ctx.cells()).filter(|&c| intersects(ctx.domain(c), &limit.bits) && !subset(ctx.domain(c), &limit.bits)).collect();
        for _ in certain..limit.min {
            if open.is_empty() {
                return Err(format!("limits: {} needs {} cells, and only {certain} more can take it", limit.name, limit.min));
            }
            let cell = open.remove(rng.below(open.len() as u32) as usize);
            and_assign(&mut ctx.domains[cell * w..(cell + 1) * w], &limit.bits);
            ctx.notes[cell].push(format!("{} (min {})", limit.name, limit.min));
        }
    }
    Ok(())
}

/// `engine.fill`: the socket solver ([`GenContext::solve`]).
pub fn fill(ctx: &mut GenContext) -> Result<(), String> {
    ctx.solve()
}

/// `engine.secondary`: the region's `secondary` points in free slots of cells that hold no
/// point yet, at random; as many as fit (a shortfall is remarked, not failed).
pub fn place_secondary(ctx: &mut GenContext) -> Result<(), String> {
    if ctx.filled.is_none() {
        return Ok(());
    }
    let mut rng = ctx.rng();
    for rule in ctx.def.secondary.clone() {
        let taken: BTreeSet<(u32, u32)> = ctx.pois.iter().map(|p| p.cell).collect();
        let mut spots: Vec<((u32, u32), Spot)> = (0..ctx.cells())
            .map(|i| ctx.cell(i))
            .filter(|c| !taken.contains(c) && (rule.cells.is_empty() || rule.cells.contains(c)))
            .filter_map(|c| ctx.free_spot(c, &rule.poi).map(|s| (c, s)))
            .collect();
        let mut placed = 0;
        while placed < rule.count && !spots.is_empty() {
            let (cell, spot) = spots.remove(rng.below(spots.len() as u32) as usize);
            let place = ctx.place_at(cell, spot);
            ctx.pois.push(PlannedPoi { poi: rule.poi.clone(), cell, required: false, place });
            placed += 1;
        }
        if placed < rule.count {
            ctx.remark(format!("secondary `{}`: {placed} of {} found a free slot", rule.poi, rule.count));
        }
    }
    Ok(())
}

/// `engine.decorate`: one of each tile's `decor` prefabs, per cell.
pub fn decorate(ctx: &mut GenContext) -> Result<(), String> {
    let mut rng = ctx.rng();
    for index in 0..ctx.cells() {
        let Some((_, def, _)) = ctx.tile_at(ctx.cell(index)) else { continue };
        ctx.decor[index] = rng.pick(&def.decor).cloned();
    }
    Ok(())
}

/// `engine.checks`: the region's rules and the custom checks it names.
pub fn run_checks(ctx: &mut GenContext) -> Result<(), String> {
    let manifest = ctx.manifest("")?;
    let graph = RegionGraph::build(ctx.db, &manifest).map_err(|e| format!("graph: {e}"))?;
    let input = CheckInput { manifest: &manifest, rules: &ctx.def.checks, graph: &graph, db: ctx.db };
    let mut failures = checks::engine_rules(&input);
    failures.extend(checks::custom_checks(ctx.worldgen, &input));
    ctx.failures.extend(failures);
    Ok(())
}

// --- fallback ----------------------------------------------------------------------------

/// The region's hand-made fallback as a manifest, and what its checks say about it.
fn fallback_manifest(db: &ContentDb, worldgen: &WorldGen, compiled: &Compiled, map_id: &str, seed: u64) -> Result<(RegionManifest, Vec<CheckFailure>), String> {
    let id = &compiled.id;
    let map = db.get::<TileMapDef>(map_id).ok_or_else(|| format!("region `{id}`: fallback `{map_id}` is not a tile_map in content"))?;
    let size = map.size().map_err(|e| format!("fallback `{map_id}`: {e}"))?;
    if size != compiled.def.size {
        return Err(format!("fallback `{map_id}` is {}×{}, region `{id}` {}×{}", size.0, size.1, compiled.def.size.0, compiled.def.size.1));
    }
    let cells = map.cells().map_err(|e| format!("fallback `{map_id}`: {e}"))?;
    let mut ids: Vec<&str> = cells.iter().map(|(t, _)| t.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let defs: Vec<&TileDef> = ids
        .iter()
        .map(|t| db.get::<TileDef>(t).ok_or_else(|| format!("fallback `{map_id}`: unknown tile `{t}`")))
        .collect::<Result<_, _>>()?;
    let infos = ids.iter().zip(&defs).map(|(t, d)| TileInfo::from_def(t, d)).collect::<Result<Vec<_>, _>>()?;
    let set = TileSet::new(compiled.set.rules.clone(), infos);
    let local = Compiled { id: id.clone(), def: compiled.def, set, defs, limits: Vec::new(), content_hash: compiled.content_hash.clone() };
    let mut ctx = GenContext::blank(&local, worldgen, db, seed, compiled.def.attempts);
    let options = cells
        .iter()
        .map(|(tile, turns)| {
            let t = local.set.tile_index(tile).expect("listed above");
            local.set.option(t, *turns).ok_or_else(|| format!("fallback `{map_id}`: `{tile}` may not turn {}°", degrees_of(*turns)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    for poi in &map.pois {
        ctx.pois.push(PlannedPoi { poi: poi.poi.clone(), cell: poi.at, required: true, place: None });
    }
    ctx.set_filled(options).map_err(|e| format!("fallback `{map_id}`: {e}"))?;
    ctx.step = fnv(b"fallback/decorate");
    decorate(&mut ctx)?;
    let mut manifest = ctx.manifest(&compiled.content_hash)?;
    manifest.attempt = 0;
    manifest.fallback = Some(map_id.to_owned());
    let problems = checks::check_manifest(db, Some(worldgen), &manifest)?;
    Ok((manifest, problems))
}

/// The `poi` definition of `id` (labels, prefabs), if content has one.
pub fn poi_def<'a>(db: &'a ContentDb, id: &str) -> Option<&'a PoiDef> {
    db.get::<PoiDef>(id)
}
