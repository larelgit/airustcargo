//! Checks a generated region must pass: the rules are data
//! ([`CheckRules`](crate::content::CheckRules)), and games add their own by name
//! ([`WorldAppExt::add_worldgen_check`](crate::WorldAppExt::add_worldgen_check)).
//!
//! - `spawn`: there is a point players appear at.
//! - `reachable`: every point of interest can be walked to from the spawn, over edges whose
//!   facing sockets are passable, through tiles' walkable areas (`links`) and along their
//!   navlinks (`navlinks`, one way, as the navmesh takes them); in dungeons, through doors.
//! - `slope`: no tile is steeper than `max_slope`.
//! - `distance`: every `a` at least `min` from every `b`, and some `b` within `max` of
//!   every `a`.
//! - `isolated_roads`: every road network reaches a point of interest.
//!
//! Each failure names its rule and the cells concerned. Checks read a manifest and the
//! content it names, so they judge a hand-made fallback or a saved manifest the same way.

use std::fmt;

use shards_content::ContentDb;

use crate::content::{CheckRules, DungeonDef, RegionDef, RoomDef, SocketRulesDef, TileDef};
use crate::generate::WorldGen;
use crate::grid::{turns_of, Dir};
use crate::manifest::{Layout, ManifestPoi, RegionManifest};
use crate::sockets::{Rules, Socket};

/// A check that did not pass.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckFailure {
    /// `reachable`, `distance`, a custom check's name, ...
    pub rule: String,
    /// The cells concerned.
    pub cells: Vec<[i32; 2]>,
    /// What is wrong, naming the points and cells.
    pub message: String,
}

impl CheckFailure {
    pub fn new(rule: &str, cells: Vec<[i32; 2]>, message: impl Into<String>) -> Self {
        Self { rule: rule.to_owned(), cells, message: message.into() }
    }
}

impl fmt::Display for CheckFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "check {}: {}", self.rule, self.message)
    }
}

/// `(3, 4), (5, 1)`.
pub fn cells_text(cells: &[[i32; 2]]) -> String {
    let shown: Vec<String> = cells.iter().take(8).map(|[x, y]| format!("({x}, {y})")).collect();
    let more = cells.len().saturating_sub(shown.len());
    if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") }
}

/// What a check reads.
pub struct CheckInput<'a> {
    pub manifest: &'a RegionManifest,
    pub rules: &'a CheckRules,
    pub graph: &'a RegionGraph,
    pub db: &'a ContentDb,
}

/// Who can walk where: walkable areas of placed tiles joined across edges (grids) or
/// doors (rooms), both ways, and by the tiles' navlinks, one way; and the road network.
#[derive(Clone, Debug)]
pub struct RegionGraph {
    pub width: u32,
    pub height: u32,
    /// The placed tile (index into the manifest's tiles) covering each cell, row-major.
    pub cell_tile: Vec<Option<usize>>,
    /// First node of each placed tile (one node per walkable area).
    node_base: Vec<usize>,
    /// The nodes each node leads to, in the order they were joined.
    next: Vec<Vec<usize>>,
    /// Sides of each cell walkers cross into the neighbor.
    open: Vec<[bool; 4]>,
    /// Sides of each cell a road crosses.
    road: Vec<[bool; 4]>,
    /// Slope class of each placed tile.
    pub slope: Vec<u8>,
}

impl RegionGraph {
    /// The graph of `manifest`, from the content it names.
    pub fn build(db: &ContentDb, manifest: &RegionManifest) -> Result<RegionGraph, String> {
        let (w, h) = (manifest.size[0], manifest.size[1]);
        let cells = (w * h) as usize;
        let mut graph = RegionGraph {
            width: w,
            height: h,
            cell_tile: vec![None; cells],
            node_base: Vec::new(),
            next: Vec::new(),
            open: vec![[false; 4]; cells],
            road: vec![[false; 4]; cells],
            slope: Vec::new(),
        };
        match manifest.layout {
            Layout::Grid => graph.build_grid(db, manifest)?,
            Layout::Rooms => graph.build_rooms(db, manifest)?,
        }
        Ok(graph)
    }

    fn index(&self, [x, y]: [i32; 2]) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height).then(|| (y as u32 * self.width + x as u32) as usize)
    }

    fn build_grid(&mut self, db: &ContentDb, manifest: &RegionManifest) -> Result<(), String> {
        let region = db.get::<RegionDef>(&manifest.region).ok_or_else(|| format!("no region `{}` in content", manifest.region))?;
        let rules = db
            .get::<SocketRulesDef>(&region.rules)
            .map(Rules::new)
            .ok_or_else(|| format!("region `{}`: no socket_rules `{}`", manifest.region, region.rules))?;
        // Turned edges and side → area of every placed tile.
        let mut edges: Vec<[Vec<Socket>; 4]> = Vec::new();
        let mut areas: Vec<[Option<usize>; 4]> = Vec::new();
        for (i, placed) in manifest.tiles.iter().enumerate() {
            let tile = db.get::<TileDef>(&placed.tile).ok_or_else(|| format!("tile `{}` is not in content", placed.tile))?;
            let turns = turns_of(placed.rot).ok_or_else(|| format!("tile `{}`: rot {} is not a quarter turn", placed.tile, placed.rot))?;
            let own = tile.sockets.parse().map_err(|e| format!("tile `{}`: {e}", placed.tile))?;
            let turned: [Vec<Socket>; 4] = std::array::from_fn(|d| own[(d + 4 - turns as usize) % 4].clone());
            let mut area = [None; 4];
            match &tile.links {
                None => area = [Some(0); 4],
                Some(groups) => {
                    for (g, group) in groups.iter().enumerate() {
                        for side in group {
                            area[side.turned(turns).index()] = Some(g);
                        }
                    }
                }
            }
            let cell = self.index(placed.at).ok_or_else(|| format!("tile {i} at {:?} is outside the region", placed.at))?;
            self.cell_tile[cell] = Some(i);
            let base = self.next.len();
            self.node_base.push(base);
            self.next.resize(base + tile.areas(), Vec::new());
            // A navlink leads from its area to another, not back (content checks the areas).
            for link in tile.navlinks.iter().filter(|l| l.from != l.to && l.from < tile.areas() && l.to < tile.areas()) {
                self.lead(base + link.from, base + link.to);
            }
            self.slope.push(tile.slope);
            edges.push(turned);
            areas.push(area);
        }
        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let Some(c) = self.index([x, y]) else { continue };
                let Some(t) = self.cell_tile[c] else { continue };
                for dir in Dir::ALL {
                    let (dx, dy) = dir.offset();
                    let Some(n) = self.index([x + dx, y + dy]) else {
                        // A road may leave the region over its border.
                        self.road[c][dir.index()] = rules.has_road(&edges[t][dir.index()]);
                        continue;
                    };
                    let Some(u) = self.cell_tile[n] else { continue };
                    let (mine, theirs) = (&edges[t][dir.index()], &edges[u][dir.opposite().index()]);
                    self.road[c][dir.index()] = rules.has_road(mine) && rules.has_road(theirs);
                    if !matches!(dir, Dir::E | Dir::S) {
                        continue;
                    }
                    if let (Some(a), Some(b)) = (areas[t][dir.index()], areas[u][dir.opposite().index()])
                        && rules.edges_passable(mine, theirs)
                    {
                        self.open[c][dir.index()] = true;
                        self.open[n][dir.opposite().index()] = true;
                        let (na, nb) = (self.node_base[t] + a, self.node_base[u] + b);
                        self.join(na, nb);
                    }
                }
            }
        }
        Ok(())
    }

    fn build_rooms(&mut self, db: &ContentDb, manifest: &RegionManifest) -> Result<(), String> {
        for (i, placed) in manifest.tiles.iter().enumerate() {
            db.get::<RoomDef>(&placed.tile).ok_or_else(|| format!("room `{}` is not in content", placed.tile))?;
            for dy in 0..placed.size[1] as i32 {
                for dx in 0..placed.size[0] as i32 {
                    let cell = self.index([placed.at[0] + dx, placed.at[1] + dy]).ok_or_else(|| format!("room {i} reaches outside the region"))?;
                    self.cell_tile[cell] = Some(i);
                }
            }
            self.node_base.push(self.next.len());
            self.next.push(Vec::new());
            self.slope.push(0);
        }
        for link in manifest.links.iter().filter(|l| l.kind == "door") {
            let (Some(a), Some(b)) = (room_index(&link.a), room_index(&link.b)) else { continue };
            if let [from, to] = link.cells[..]
                && let (Some(c), Some(n)) = (self.index(from), self.index(to))
                && let Some(dir) = Dir::ALL.into_iter().find(|d| d.offset() == (to[0] - from[0], to[1] - from[1]))
            {
                self.open[c][dir.index()] = true;
                self.open[n][dir.opposite().index()] = true;
            }
            if a < self.node_base.len() && b < self.node_base.len() {
                let (na, nb) = (self.node_base[a], self.node_base[b]);
                self.join(na, nb);
            }
        }
        Ok(())
    }

    /// Walkers get from `a` to `b`.
    fn lead(&mut self, a: usize, b: usize) {
        if !self.next[a].contains(&b) {
            self.next[a].push(b);
        }
    }

    /// Walkers get from `a` to `b` and back.
    fn join(&mut self, a: usize, b: usize) {
        self.lead(a, b);
        self.lead(b, a);
    }

    /// Whether walkers get from node `from` to node `to`.
    fn leads(&self, from: usize, to: usize) -> bool {
        let mut seen = vec![false; self.next.len()];
        let mut stack = vec![from];
        seen[from] = true;
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            for &m in &self.next[n] {
                if !seen[m] {
                    seen[m] = true;
                    stack.push(m);
                }
            }
        }
        false
    }

    /// Whether walkers cross from `cell` into its neighbor on side `dir`.
    pub fn open(&self, cell: [i32; 2], dir: Dir) -> bool {
        self.index(cell).is_some_and(|c| self.open[c][dir.index()])
    }

    /// Whether a road crosses side `dir` of `cell`.
    pub fn road(&self, cell: [i32; 2], dir: Dir) -> bool {
        self.index(cell).is_some_and(|c| self.road[c][dir.index()])
    }

    /// The walkable area a point of interest is in, as a node.
    fn node_of(&self, poi: &ManifestPoi) -> Option<usize> {
        let tile = self.cell_tile[self.index(poi.cell)?]?;
        let base = self.node_base[tile];
        let next = self.node_base.get(tile + 1).copied().unwrap_or(self.next.len());
        Some(base + poi.area.min(next - base - 1))
    }

    /// Whether walkers get from `from` to `to` (a navlink goes one way: a drop off a ledge
    /// reaches the ground below, not the ledge from there).
    pub fn reaches(&self, from: &ManifestPoi, to: &ManifestPoi) -> bool {
        match (self.node_of(from), self.node_of(to)) {
            (Some(x), Some(y)) => self.leads(x, y),
            _ => false,
        }
    }

    /// Whether `a` and `b` can be walked between, either way.
    pub fn connected(&self, a: &ManifestPoi, b: &ManifestPoi) -> bool {
        self.reaches(a, b) && self.reaches(b, a)
    }
}

/// `room.3` → 3.
fn room_index(id: &str) -> Option<usize> {
    id.strip_prefix("room.")?.parse().ok()
}

/// The check rules of the region or dungeon a manifest comes from.
pub fn rules_of<'a>(db: &'a ContentDb, manifest: &RegionManifest) -> Result<&'a CheckRules, String> {
    match manifest.layout {
        Layout::Grid => db.get::<RegionDef>(&manifest.region).map(|r| &r.checks),
        Layout::Rooms => db.get::<DungeonDef>(&manifest.region).map(|d| &d.checks),
    }
    .ok_or_else(|| format!("no {} `{}` in content", if manifest.layout == Layout::Grid { "region" } else { "dungeon" }, manifest.region))
}

/// Every check of the manifest's region: the engine's rules and the custom checks it names
/// (from `worldgen`; without it they are skipped).
pub fn check_manifest(db: &ContentDb, worldgen: Option<&WorldGen>, manifest: &RegionManifest) -> Result<Vec<CheckFailure>, String> {
    let rules = rules_of(db, manifest)?;
    let graph = RegionGraph::build(db, manifest)?;
    let input = CheckInput { manifest, rules, graph: &graph, db };
    let mut failures = engine_rules(&input);
    if let Some(worldgen) = worldgen {
        failures.extend(custom_checks(worldgen, &input));
    }
    Ok(failures)
}

/// The checks the region names, run.
pub fn custom_checks(worldgen: &WorldGen, input: &CheckInput) -> Vec<CheckFailure> {
    let mut failures = Vec::new();
    for name in &input.rules.custom {
        match worldgen.check(name) {
            Some(check) => failures.extend(check(input)),
            None => {
                let known = worldgen.check_names().join(", ");
                let message = format!("no check named `{name}` is registered (add_worldgen_check); registered: {known}");
                failures.push(CheckFailure::new("custom", Vec::new(), message));
            }
        }
    }
    failures
}

/// The engine's checks, in a fixed order.
pub fn engine_rules(input: &CheckInput) -> Vec<CheckFailure> {
    let mut failures = Vec::new();
    let m = input.manifest;
    let spawn = m.spawn.as_deref().and_then(|id| m.poi(id));
    if spawn.is_none() {
        failures.push(CheckFailure::new("spawn", Vec::new(), "no point of interest to spawn players at"));
    }
    if input.rules.reachable
        && let Some(spawn) = spawn
    {
        let cut: Vec<&ManifestPoi> = m.pois.iter().filter(|p| !input.graph.reaches(spawn, p)).collect();
        if !cut.is_empty() {
            let names: Vec<String> = cut.iter().map(|p| format!("{} at ({}, {})", p.id, p.cell[0], p.cell[1])).collect();
            let message = format!("cannot reach {} from {}", names.join(", "), spawn.id);
            failures.push(CheckFailure::new("reachable", cut.iter().map(|p| p.cell).collect(), message));
        }
    }
    if let Some(max) = input.rules.max_slope {
        let steep: Vec<[i32; 2]> =
            m.tiles.iter().zip(&input.graph.slope).filter(|(_, s)| **s > max).map(|(t, _)| t.at).collect();
        if !steep.is_empty() {
            failures.push(CheckFailure::new("slope", steep.clone(), format!("tiles steeper than {max} at {}", cells_text(&steep))));
        }
    }
    for rule in &input.rules.distances {
        fn of<'m>(m: &'m RegionManifest, kind: &'m str) -> impl Iterator<Item = &'m ManifestPoi> + 'm {
            m.pois.iter().filter(move |p| p.poi == kind)
        }
        let of = |kind| of(m, kind);
        if let Some(min) = rule.min {
            for a in of(&rule.a) {
                for b in of(&rule.b).filter(|b| b.id != a.id) {
                    let d = a.position().distance(b.position());
                    if d < min && (rule.a != rule.b || a.id < b.id) {
                        let message = format!("{} and {} are {d:.1}m apart, closer than {min}m", a.id, b.id);
                        failures.push(CheckFailure::new("distance", vec![a.cell, b.cell], message));
                    }
                }
            }
        }
        if let Some(max) = rule.max {
            for a in of(&rule.a) {
                let nearest = of(&rule.b).filter(|b| b.id != a.id).map(|b| a.position().distance(b.position())).fold(f32::INFINITY, f32::min);
                if nearest > max {
                    let what = if nearest.is_finite() { format!("the nearest is {nearest:.1}m away") } else { "there is none".to_owned() };
                    let message = format!("{} has no {} within {max}m; {what}", a.id, rule.b);
                    failures.push(CheckFailure::new("distance", vec![a.cell], message));
                }
            }
        }
    }
    if input.rules.isolated_roads && m.layout == Layout::Grid {
        failures.extend(isolated_roads(input));
    }
    failures
}

/// Road networks (cells joined by road sides) that hold no point of interest.
fn isolated_roads(input: &CheckInput) -> Vec<CheckFailure> {
    let g = input.graph;
    let (w, h) = (g.width as i32, g.height as i32);
    let cells = (w * h) as usize;
    let on_road = |c: usize| g.road[c].iter().any(|r| *r);
    let mut seen = vec![false; cells];
    let mut failures = Vec::new();
    for start in 0..cells {
        if seen[start] || !on_road(start) {
            continue;
        }
        let mut network = Vec::new();
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(c) = stack.pop() {
            let (x, y) = ((c as i32) % w, (c as i32) / w);
            network.push([x, y]);
            for dir in Dir::ALL {
                let (dx, dy) = dir.offset();
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w || ny >= h || !g.road[c][dir.index()] {
                    continue;
                }
                let n = (ny * w + nx) as usize;
                if !seen[n] {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        network.sort_unstable_by_key(|[x, y]| (*y, *x));
        let served = input.manifest.pois.iter().any(|p| network.contains(&p.cell));
        if !served {
            let message = format!("the road through {} reaches no point of interest", cells_text(&network));
            failures.push(CheckFailure::new("isolated_roads", network, message));
        }
    }
    failures
}
