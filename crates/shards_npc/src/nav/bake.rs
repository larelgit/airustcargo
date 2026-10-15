//! Baking: a tile's gameplay geometry spawned alone in a scratch world (by the world
//! module's own tile spawner, or the level module's for tiles built from level files), then
//! sampled on a grid with the motion module's terrain probe and move rule.
//!
//! Sampling, per grid column: every upward face is a candidate floor (rays down, from above
//! the tile through each solid in turn); one the baked body stands on (or swims above) is a
//! node. From every node, the moves the motion planner would make to the eight neighbours
//! (no cutting corners) and the two-cell hops over the foot of a ledge become links, and
//! the floors they find become nodes too: the bake floods the tile with the planner's own
//! rule, so the navmesh and `path_to` agree on what can be walked, climbed and swum.
//!
//! Everything runs in a fixed order on integers and the physics queries' deterministic
//! answers, and nodes are numbered by position afterwards: the same tile gives the same
//! bytes.

use std::collections::{BTreeMap, BinaryHeap, HashMap};

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_motion::perception::{hop_allowed, step_onto, Probe, Stance, Step, StepKind, Terrain};
use shards_motion::physics::sync_physics;
use shards_motion::{Body, ColliderIds, LayerMask, PhysicsWorld, QueryFilter};
use shards_world::grid::Dir;
use shards_world::manifest::{Layout, PlacedTile, RegionManifest, FORMAT as MANIFEST_FORMAT};
use shards_world::TileGeometry;

use super::settings::{bake_hash, geometry_of, whole_cells, NavSettings};
use super::tile::{columns_of, BakedCollider, NavNode, NavTile, NavTileLink, Portal, TileKind, MOVES};

/// Bakes `kind` `id` for regions of `tile_size` m cells: spawns its geometry alone in a
/// scratch world (`main` lends the content, the type registry and loaded level files) and
/// samples it. `root`: the asset folder, for the hash of a level file.
pub fn bake(main: &World, root: Option<&std::path::Path>, kind: TileKind, id: &str, tile_size: f32, settings: &NavSettings) -> Result<NavTile, String> {
    let db = main.get_resource::<ContentDb>().ok_or("no content database")?;
    let what = kind.content_kind();
    let (geometry, footprint) = geometry_of(db, kind, id).ok_or_else(|| format!("no {what} `{id}` in content"))?;
    if !whole_cells(tile_size, settings.cell) {
        return Err(format!("a tile size of {tile_size} m is not a whole number of {} m cells", settings.cell));
    }
    let hash = bake_hash(db, root, kind, id, tile_size, settings)?;
    let mut scratch = scratch_world(main, db);
    spawn_alone(main, &mut scratch, kind, id, &geometry, footprint, tile_size)?;
    scratch.run_system_once(sync_physics).map_err(|e| format!("physics sync: {e}"))?;
    let physics = scratch.resource::<PhysicsWorld>();
    let per = (tile_size / settings.cell).round() as u32;
    let size = [footprint[0] * per, footprint[1] * per];
    let sampled = Sampler::new(physics, settings, size).run();
    let tile = assemble(kind, id, hash, tile_size, settings.cell, size, sampled, colliders_of(physics))?;
    // Through the file's own encoding: a tile baked now and one read from its file are the
    // same, down to the costs.
    NavTile::from_file(&tile.to_file())
}

/// An empty world with what spawning a tile reads: content, the type registry (prefabs and
/// level components), collider ids, a physics world.
fn scratch_world(main: &World, db: &ContentDb) -> World {
    let mut scratch = World::new();
    if let Some(registry) = main.get_resource::<AppTypeRegistry>() {
        scratch.insert_resource(registry.clone());
    } else {
        scratch.init_resource::<AppTypeRegistry>();
    }
    scratch.insert_resource(db.clone());
    scratch.init_resource::<ColliderIds>();
    scratch.init_resource::<PhysicsWorld>();
    scratch
}

/// The tile alone, its north-west corner at the origin, not turned: the region module's
/// own builder for inline geometry, the level module for a level file.
fn spawn_alone(main: &World, scratch: &mut World, kind: TileKind, id: &str, geometry: &TileGeometry, footprint: [u32; 2], tile_size: f32) -> Result<(), String> {
    let placed = PlacedTile { at: [0, 0], size: footprint, tile: id.to_owned(), rot: 0, decor: None };
    match geometry {
        TileGeometry::Level { path } => {
            let level = shards_level::level_asset(main, path)
                .map_err(|e| format!("its level file `{path}` is not loaded ({e}); bake it in a game that loads its levels"))?;
            let center = Vec3::new(footprint[0] as f32 * tile_size * 0.5, 0.0, footprint[1] as f32 * tile_size * 0.5);
            shards_level::spawn_level(scratch, level, Transform::from_translation(center)).map(|_| ())
        }
        TileGeometry::None | TileGeometry::Inline(_) => {
            let manifest = RegionManifest {
                format: MANIFEST_FORMAT.to_owned(),
                region: format!("bake {id}"),
                layout: if kind == TileKind::Room { Layout::Rooms } else { Layout::Grid },
                seed: 0,
                version: 0,
                attempt: 0,
                fallback: None,
                size: footprint,
                tile_size,
                origin: [0.0, 0.0],
                tiles: vec![placed],
                pois: Vec::new(),
                links: Vec::new(),
                spawn: None,
                content_hash: String::new(),
            };
            shards_world::spawn_region(scratch, &manifest).map(|_| ())
        }
    }
}

/// The static solids of the scratch world, in collider order.
fn colliders_of(physics: &PhysicsWorld) -> Vec<BakedCollider> {
    physics
        .colliders()
        .filter(|c| solid(c))
        .map(|c| BakedCollider { shape: c.shape, pos: c.position, rot: c.rotation })
        .collect()
}

/// Static level geometry: what a bake samples and a region compares against.
pub(crate) fn solid(c: &shards_motion::ColliderInfo) -> bool {
    c.volume.is_none() && c.body == Body::Static && c.layers.memberships.intersects(LayerMask::WORLD)
}

/// A node as sampled: its column, feet, way of standing.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Raw {
    pub i: i32,
    pub j: i32,
    pub pos: Vec3,
    pub swim: bool,
    pub up: f32,
}

/// A link as sampled: from, to, direction code, kind.
pub(crate) type RawLink = (usize, usize, u8, StepKind);

/// `(i, j, level, swimming)`: a cell at one floor level, as the motion planner keys it.
type Key = (i32, i32, i32, bool);

fn level(y: f32) -> i32 {
    (y * 4.0).round() as i32
}

/// The flood over one tile.
pub(crate) struct Sampler<'a> {
    terrain: Terrain<'a>,
    physics: &'a PhysicsWorld,
    cell: f32,
    safe_drop: f32,
    size: [u32; 2],
    probes: HashMap<(i32, i32, i32), Probe>,
    nodes: Vec<Raw>,
    index: HashMap<Key, usize>,
    links: Vec<RawLink>,
}

impl<'a> Sampler<'a> {
    pub(crate) fn new(physics: &'a PhysicsWorld, settings: &'a NavSettings, size: [u32; 2]) -> Self {
        Self {
            terrain: Terrain::new(physics, &settings.agent, &[]),
            physics,
            cell: settings.cell,
            safe_drop: settings.safe_drop,
            size,
            probes: HashMap::new(),
            nodes: Vec::new(),
            index: HashMap::new(),
            links: Vec::new(),
        }
    }

    fn centre(&self, i: i32) -> f32 {
        (i as f32 + 0.5) * self.cell
    }

    fn inside(&self, i: i32, j: i32) -> bool {
        i >= 0 && j >= 0 && (i as u32) < self.size[0] && (j as u32) < self.size[1]
    }

    fn probe(&mut self, i: i32, j: i32, from_y: f32) -> Probe {
        let (x, z) = (self.centre(i), self.centre(j));
        let terrain = &self.terrain;
        *self.probes.entry((i, j, level(from_y))).or_insert_with(|| terrain.probe(x, z, from_y))
    }

    pub(crate) fn run(mut self) -> (Vec<Raw>, Vec<RawLink>) {
        self.seed();
        let mut at = 0;
        while at < self.nodes.len() {
            self.expand(at);
            at += 1;
        }
        (self.nodes, self.links)
    }

    /// Every floor of every column the body stands on, or swims above.
    fn seed(&mut self) {
        let solids = self.physics.colliders().filter(|c| c.volume.is_none() && c.layers.memberships.intersects(LayerMask::WORLD));
        let (top, bottom) = solids.fold((f32::MIN, f32::MAX), |(t, b), c| (t.max(c.aabb_max.y), b.min(c.aabb_min.y)));
        if top < bottom {
            return;
        }
        let top = top + 1.0;
        let bottom = bottom - 1.0;
        let filter = QueryFilter::new(LayerMask::WORLD);
        for j in 0..self.size[1] as i32 {
            for i in 0..self.size[0] as i32 {
                let (x, z) = (self.centre(i), self.centre(j));
                let mut from = top;
                for _ in 0..32 {
                    if from <= bottom {
                        break;
                    }
                    let Some(hit) = self.physics.raycast(Vec3::new(x, from, z), Dir3::NEG_Y, from - bottom, &filter) else { break };
                    if hit.distance <= 1e-4 {
                        // Inside a solid: go on below it.
                        let below = self.physics.collider(hit.entity).map_or(from - 0.25, |c| c.aabb_min.y - 0.01);
                        from = below.min(from - 0.01);
                        continue;
                    }
                    let y = hit.point.y;
                    if hit.normal.y > 0.0 {
                        match self.probe(i, j, y) {
                            Probe::Floor { y: floor, normal } if (floor - y).abs() < 0.02 => {
                                self.node(Raw { i, j, pos: Vec3::new(x, floor, z), swim: false, up: normal.y });
                            }
                            Probe::Water { surface } => {
                                let swim = self.terrain.swim_height(surface);
                                self.node(Raw { i, j, pos: Vec3::new(x, swim, z), swim: true, up: 1.0 });
                            }
                            _ => {}
                        }
                    }
                    from = y - 0.01;
                }
            }
        }
    }

    /// The node at `raw`'s cell and level, made if new.
    fn node(&mut self, raw: Raw) -> usize {
        let key = (raw.i, raw.j, level(raw.pos.y), raw.swim);
        if let Some(&n) = self.index.get(&key) {
            return n;
        }
        self.nodes.push(raw);
        self.index.insert(key, self.nodes.len() - 1);
        self.nodes.len() - 1
    }

    /// The move from `from` to cell `(i, j)` (`dist`: a two-cell hop's length).
    fn step(&mut self, from: Stance, i: i32, j: i32, dist: Option<f32>) -> Option<Step> {
        let (x, z) = (self.centre(i), self.centre(j));
        let dist = dist.unwrap_or_else(|| Vec2::new(x - from.pos.x, z - from.pos.z).length());
        let probe = self.probe(i, j, from.pos.y);
        step_onto(&self.terrain, from, (x, z), probe, dist, self.safe_drop)
    }

    /// The links out of node `at`, as the motion planner moves: eight neighbours without
    /// cutting corners, a hop over the foot of a ledge.
    fn expand(&mut self, at: usize) {
        let raw = self.nodes[at];
        let from = Stance { pos: raw.pos, swimming: raw.swim, up: raw.up };
        for (k, &(di, dj)) in MOVES.iter().take(8).enumerate() {
            let (i, j) = (raw.i + di, raw.j + dj);
            if !self.inside(i, j) {
                continue;
            }
            let diagonal = k % 2 == 1;
            if diagonal && (self.step(from, raw.i + di, raw.j, None).is_none() || self.step(from, raw.i, raw.j + dj, None).is_none()) {
                continue;
            }
            if let Some(step) = self.step(from, i, j, None) {
                self.link(at, i, j, k as u8, step);
            } else if !diagonal && matches!(self.probe(i, j, from.pos.y), Probe::Blocked(shards_motion::perception::Block::Tight)) {
                let (hi, hj) = (raw.i + 2 * di, raw.j + 2 * dj);
                if self.inside(hi, hj)
                    && let Some(step) = self.step(from, hi, hj, Some(2.0 * self.cell))
                    && hop_allowed(step.kind, from.swimming)
                {
                    self.link(at, hi, hj, 8 + k as u8 / 2, step);
                }
            }
        }
    }

    fn link(&mut self, from: usize, i: i32, j: i32, code: u8, step: Step) {
        let to = self.node(Raw { i, j, pos: step.pos, swim: step.swimming, up: step.up });
        self.links.push((from, to, code, step.kind));
    }
}

/// Numbers the sampled nodes by position, links them, finds areas and portals and the
/// costs between portals.
#[allow(clippy::too_many_arguments, reason = "the header of a bake and what was sampled, named once")]
fn assemble(
    kind: TileKind,
    id: &str,
    hash: String,
    tile_size: f32,
    cell: f32,
    size: [u32; 2],
    (raw, raw_links): (Vec<Raw>, Vec<RawLink>),
    colliders: Vec<BakedCollider>,
) -> Result<NavTile, String> {
    let mm = |y: f32| (y * 1000.0).round() as i32;
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by_key(|&n| (raw[n].j, raw[n].i, mm(raw[n].pos.y), raw[n].swim));
    let mut renumber = vec![0u32; raw.len()];
    for (new, &old) in order.iter().enumerate() {
        renumber[old] = new as u32;
    }
    let mut nodes: Vec<NavNode> = order
        .iter()
        .map(|&n| {
            let r = raw[n];
            NavNode { i: r.i as u16, j: r.j as u16, y_mm: mm(r.pos.y), up_q: (r.up.clamp(0.0, 1.0) * 10000.0).round() as u16, swim: r.swim, area: 0 }
        })
        .collect();
    let mut links: Vec<(u32, u8, u32, StepKind)> = raw_links.iter().map(|&(a, b, code, kind)| (renumber[a], code, renumber[b], kind)).collect();
    links.sort_by_key(|&(a, code, ..)| (a, code));
    links.dedup_by_key(|&mut (a, code, ..)| (a, code));
    let columns = columns_of(&nodes, size)?;
    if columns.windows(2).any(|w| w[1] - w[0] > 255) {
        return Err("a column with more than 255 floors".into());
    }
    let mut first_link = vec![0u32; nodes.len() + 1];
    for &(a, ..) in &links {
        first_link[a as usize + 1] += 1;
    }
    for n in 0..nodes.len() {
        first_link[n + 1] += first_link[n];
    }
    // Areas: what reaches what, links taken both ways.
    let mut parent: Vec<u32> = (0..nodes.len() as u32).collect();
    fn find(parent: &mut [u32], mut n: u32) -> u32 {
        while parent[n as usize] != n {
            parent[n as usize] = parent[parent[n as usize] as usize];
            n = parent[n as usize];
        }
        n
    }
    for &(a, _, b, _) in &links {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra.max(rb) as usize] = ra.min(rb);
        }
    }
    let mut labels: BTreeMap<u32, u16> = BTreeMap::new();
    for n in 0..nodes.len() as u32 {
        let root = find(&mut parent, n);
        let next = labels.len() as u16;
        nodes[n as usize].area = *labels.entry(root).or_insert(next);
    }
    let tile_links: Vec<NavTileLink> =
        links.iter().map(|&(_, code, to, kind)| NavTileLink { to, code, kind, cost: 0, rise: 0.0 }).collect();
    let mut tile = NavTile {
        kind,
        id: id.to_owned(),
        hash,
        tile_size,
        cell,
        size,
        nodes,
        first_link,
        links: tile_links,
        columns,
        areas: labels.len() as u16,
        portals: Vec::new(),
        portal_costs: Vec::new(),
        colliders,
    };
    // The costs as a file read gives them, before measuring between portals.
    tile = NavTile::from_file(&tile.to_file())?;
    tile.portals = portals_of(&tile);
    tile.portal_costs = portal_costs(&tile);
    Ok(tile)
}

/// The nodes of each area along each side, the member nearest the side's middle first.
fn portals_of(tile: &NavTile) -> Vec<Portal> {
    let [nx, nz] = tile.size;
    let mut portals = Vec::new();
    for side in Dir::ALL {
        let mut by_area: BTreeMap<u16, Vec<u32>> = BTreeMap::new();
        for (n, node) in tile.nodes.iter().enumerate() {
            let on = match side {
                Dir::N => node.j == 0,
                Dir::E => node.i as u32 == nx - 1,
                Dir::S => node.j as u32 == nz - 1,
                Dir::W => node.i == 0,
            };
            if on {
                by_area.entry(node.area).or_default().push(n as u32);
            }
        }
        for (area, nodes) in by_area {
            let off_middle = |n: &u32| {
                let node = &tile.nodes[*n as usize];
                let (along, len) = if matches!(side, Dir::N | Dir::S) { (node.i as u32, nx) } else { (node.j as u32, nz) };
                (2 * along + 1).abs_diff(len)
            };
            let rep = *nodes.iter().min_by_key(|n| (off_middle(n), **n)).expect("a portal has nodes");
            portals.push(Portal { side, area, nodes, rep });
        }
    }
    portals
}

/// Cheapest costs from every portal's rep to every other's, inside the tile.
fn portal_costs(tile: &NavTile) -> Vec<u32> {
    let p = tile.portals.len();
    let mut costs = vec![u32::MAX; p * p];
    for (a, portal) in tile.portals.iter().enumerate() {
        let dist = dijkstra(tile, portal.rep, None);
        for (b, other) in tile.portals.iter().enumerate() {
            costs[a * p + b] = dist[other.rep as usize];
        }
    }
    costs
}

/// Cheapest costs from `from` to every node of the tile over its links (`u32::MAX`:
/// unreachable), skipping `disabled` nodes.
pub(crate) fn dijkstra(tile: &NavTile, from: u32, disabled: Option<&[bool]>) -> Vec<u32> {
    let off = |n: u32| disabled.is_some_and(|d| d.get(n as usize).copied().unwrap_or(false));
    let mut dist = vec![u32::MAX; tile.nodes.len()];
    if off(from) {
        return dist;
    }
    let mut open = BinaryHeap::new();
    dist[from as usize] = 0;
    open.push(std::cmp::Reverse((0u32, from)));
    while let Some(std::cmp::Reverse((d, n))) = open.pop() {
        if d != dist[n as usize] {
            continue;
        }
        for link in tile.links_of(n) {
            if off(link.to) {
                continue;
            }
            let next = d.saturating_add(link.cost);
            if next < dist[link.to as usize] {
                dist[link.to as usize] = next;
                open.push(std::cmp::Reverse((next, link.to)));
            }
        }
    }
    dist
}
