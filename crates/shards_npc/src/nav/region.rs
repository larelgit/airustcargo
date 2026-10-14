//! [`NavRegion`]: the navmesh of the region in the world, stitched from its tiles' bakes.
//!
//! Built when a region loads (the world module's [`RegionLoadedHooks`]), inside the call
//! that loads it, so no tick ever sees a region without it:
//!
//! 1. **Bakes**: each tile (and room) of the manifest, once per id: its bake file when the
//!    file is fresh (its hash matches the definition), else baked now, with a journal
//!    warning naming the files to bake (`--bake-nav`).
//! 2. **Placing**: every placed tile is an instance of its bake, turned with the tile. A
//!    bake is in its tile's own frame, so turning never touches its links; positions and
//!    directions turn on the way out.
//! 3. **Carving**: solids the bakes never saw (decor, props, a neighbour's wall reaching
//!    over the edge, colliders spawned later) switch off the nodes the body no longer fits
//!    on, checked with the motion module's terrain probe in the assembled world.
//! 4. **Stitching**: across every tile edge the region's sockets leave open (the world
//!    module's [`RegionGraph`](shards_world::RegionGraph)), each boundary node gets the move
//!    the motion planner would make into the facing column; edges whose sockets are not
//!    passable stay closed, whatever the geometry.
//! 5. **Navlinks**: level markers `navlink` with a `to` are one-way links (a drop, a
//!    climb, a jump) from the node at the marker to the node at `to`.
//! 6. **The tile graph**: portals (one per side and area of each tile), the costs between
//!    them inside each tile, the links between tiles; and which areas reach which at all.
//!
//! Everything is a pure function of the manifest, content and the physics world, built in
//! a fixed order: the same region gives the same navmesh, so NPC routes replay.

use std::collections::BTreeMap;
use std::sync::Arc;

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_content::{asset_root, ContentDb};
use shards_level::{LevelMarker, MarkerKind};
use shards_motion::perception::{step_onto, Probe, Stance, StepKind, Terrain};
use shards_motion::physics::sync_physics;
use shards_motion::{ColliderId, ColliderIds, PhysicsWorld};
use shards_world::grid::{rotation, turn, turns_of, Dir};
use shards_world::manifest::Layout;
use shards_world::{ActiveRegion, TileId};

use super::bake::{bake, dijkstra, solid};
use super::settings::{whole_cells, NavSettings};
use super::tile::{link_cost, NavTile, TileKind, MOVES};

/// Global node id: an instance's first id plus the node's index in its bake.
pub type NodeId = u32;

/// A placed tile: its bake, where and how it is turned.
#[derive(Clone, Debug)]
pub struct Instance {
    /// The manifest cell of its north-west corner.
    pub cell: [i32; 2],
    pub tile: String,
    pub kind: TileKind,
    /// Quarter turns clockwise.
    pub turns: u8,
    pub nav: Arc<NavTile>,
    /// Global grid column of its north-west corner.
    pub origin: [i32; 2],
    /// Grid columns it covers once turned.
    pub grid: [u32; 2],
    /// Its center in the world.
    pub center: Vec3,
    /// Nodes switched off by carving (empty: none).
    pub disabled: Vec<bool>,
}

impl Instance {
    /// Local column `(i, j)` → global column.
    pub fn to_global(&self, i: i32, j: i32) -> [i32; 2] {
        let [mut w, mut h] = [self.nav.size[0] as i32, self.nav.size[1] as i32];
        let (mut x, mut y) = (i, j);
        for _ in 0..self.turns {
            (x, y) = (h - 1 - y, x);
            (w, h) = (h, w);
        }
        let _ = w;
        [self.origin[0] + x, self.origin[1] + y]
    }

    /// Global column → local column (it may be outside the bake's grid).
    pub fn to_local(&self, [gx, gz]: [i32; 2]) -> (i32, i32) {
        let (mut x, mut y) = (gx - self.origin[0], gz - self.origin[1]);
        // The grid turned `turns` times is `grid`; undo one turn at a time.
        let [mut w, mut h] = [self.grid[0] as i32, self.grid[1] as i32];
        for _ in 0..self.turns {
            // One clockwise turn took (x, y) of an h'×w' grid to (h' - 1 - y, x) of a w×h one.
            (x, y) = (y, w - 1 - x);
            (w, h) = (h, w);
        }
        let _ = h;
        (x, y)
    }

    /// A local direction (`MOVES`) in the world.
    pub fn to_world_dir(&self, (di, dj): (i32, i32)) -> (i32, i32) {
        let (mut x, mut y) = (di, dj);
        for _ in 0..self.turns {
            (x, y) = (-y, x);
        }
        (x, y)
    }

    /// A world direction in the bake's frame.
    pub fn to_local_dir(&self, (dx, dz): (i32, i32)) -> (i32, i32) {
        let (mut x, mut y) = (dx, dz);
        for _ in 0..self.turns {
            (x, y) = (y, -x);
        }
        (x, y)
    }

    /// The world side of a side of the bake.
    pub fn world_side(&self, side: Dir) -> Dir {
        side.turned(self.turns)
    }

    /// The bake's side that faces `side` in the world.
    pub fn local_side(&self, side: Dir) -> Dir {
        side.turned(4 - self.turns % 4)
    }

    pub fn is_disabled(&self, n: u32) -> bool {
        self.disabled.get(n as usize).copied().unwrap_or(false)
    }
}

/// A link that is not inside one bake: across a tile edge, or a navlink.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extra {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: StepKind,
    pub cost: u32,
    pub rise: f32,
    /// A level's navlink (not a stitch).
    pub navlink: bool,
}

/// A navlink marker as the navmesh took it.
#[derive(Clone, Debug, PartialEq)]
pub struct NavLinkUse {
    /// The marker's name.
    pub name: String,
    pub from: Vec3,
    pub to: Vec3,
    /// The nodes it joins, when both ends are on walkable ground.
    pub nodes: Option<(NodeId, NodeId)>,
}

/// A node of the tile graph: a portal of an instance, or a navlink's end.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GraphNode {
    pub inst: u32,
    /// The node it measures from (`None`: every member is switched off).
    pub node: Option<NodeId>,
    /// Its portal in the bake, for a portal.
    pub portal: Option<u32>,
}

/// The tile graph: portals and navlink ends, joined inside tiles and across them.
#[derive(Clone, Debug, Default)]
pub(crate) struct TileGraph {
    pub nodes: Vec<GraphNode>,
    /// The graph nodes of each instance: its portals, then its navlink ends.
    pub of_instance: Vec<Vec<u32>>,
    /// `edges[first[n]..first[n + 1]]` leave node `n`: `(to, cost)`.
    pub first: Vec<u32>,
    pub edges: Vec<(u32, u32)>,
}

/// What a build did, for the journal and the tools.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NavReport {
    /// Tiles whose bake file was read.
    pub from_files: Vec<String>,
    /// Tiles baked while loading: no bake file.
    pub missing: Vec<String>,
    /// Tiles baked while loading: a stale or unreadable bake file.
    pub stale: Vec<String>,
    /// Tiles that could not be baked, and why: they have no walkable ground.
    pub failed: Vec<(String, String)>,
    /// Links across tile edges.
    pub stitched: usize,
    /// Tile edges the sockets keep closed although both sides have ground.
    pub closed_edges: usize,
    /// Nodes switched off by carving.
    pub carved: usize,
}

/// Solids the bakes did not have, and the nodes each switched off.
#[derive(Clone, Debug, Default)]
struct Carve {
    /// The next collider id at the last look: newer ones have not been carved yet.
    next_id: u32,
    records: Vec<(ColliderId, Vec<NodeId>)>,
}

/// The navmesh of the region in the world (see the module docs).
#[derive(Resource, Clone, Debug)]
pub struct NavRegion {
    /// [`ActiveRegion::hash`] of the region it was built for.
    pub hash: u64,
    /// `meadow seed 42 v1`.
    pub title: String,
    pub cell: f32,
    pub tile_size: f32,
    /// Grid columns per region cell.
    pub per: u32,
    /// Region cells (x, y).
    pub size: [u32; 2],
    /// The body the tiles were baked for.
    pub agent_radius: f32,
    pub agent_height: f32,
    pub safe_drop: f32,
    pub instances: Vec<Instance>,
    /// Instance covering each region cell, row-major (`u32::MAX`: none).
    cell_instance: Vec<u32>,
    /// First global id of each instance, and the total at the end.
    base: Vec<u32>,
    /// Sorted by `from`, then `to`.
    pub(crate) extras: Vec<Extra>,
    pub navlinks: Vec<NavLinkUse>,
    pub(crate) graph: TileGraph,
    /// First label of each instance's areas.
    area_base: Vec<u32>,
    /// Which areas reach which (links taken both ways): a label per area.
    components: Vec<u32>,
    pub report: NavReport,
    carve: Carve,
}

/// Bakes read or made this run, by hash: a region loaded again bakes nothing twice.
#[derive(Resource, Default, Clone)]
pub struct NavBakes {
    pub tiles: BTreeMap<String, Arc<NavTile>>,
}

/// How a tile's bake came to be.
enum Source {
    File,
    Missing,
    Stale,
    Failed(String),
}

impl NavRegion {
    /// Global grid column of a world position.
    pub fn column_of(&self, pos: Vec3) -> [i32; 2] {
        [(pos.x / self.cell).floor() as i32, (pos.z / self.cell).floor() as i32]
    }

    /// The world position of a global column's center (y 0).
    pub fn column_center(&self, [gx, gz]: [i32; 2]) -> Vec3 {
        Vec3::new((gx as f32 + 0.5) * self.cell, 0.0, (gz as f32 + 0.5) * self.cell)
    }

    /// The instance covering global column `col`.
    pub fn instance_at(&self, [gx, gz]: [i32; 2]) -> Option<u32> {
        if gx < 0 || gz < 0 {
            return None;
        }
        let (cx, cy) = (gx as u32 / self.per, gz as u32 / self.per);
        if cx >= self.size[0] || cy >= self.size[1] {
            return None;
        }
        let i = self.cell_instance[(cy * self.size[0] + cx) as usize];
        (i != u32::MAX).then_some(i)
    }

    /// The nodes of global column `col`, lowest first.
    pub fn nodes_at(&self, col: [i32; 2]) -> std::ops::Range<NodeId> {
        let Some(inst) = self.instance_at(col) else { return 0..0 };
        let instance = &self.instances[inst as usize];
        let (i, j) = instance.to_local(col);
        let range = instance.nav.column(i, j);
        let base = self.base[inst as usize];
        base + range.start..base + range.end
    }

    /// Instance and local index of a global node.
    pub fn split(&self, node: NodeId) -> (u32, u32) {
        let inst = self.base.partition_point(|b| *b <= node) - 1;
        (inst as u32, node - self.base[inst])
    }

    /// Global id of node `n` of instance `inst`.
    pub fn global(&self, inst: u32, n: u32) -> NodeId {
        self.base[inst as usize] + n
    }

    /// Every node of the region.
    pub fn node_count(&self) -> u32 {
        *self.base.last().unwrap_or(&0)
    }

    /// The global column of a node.
    pub fn node_column(&self, node: NodeId) -> [i32; 2] {
        let (inst, n) = self.split(node);
        let instance = &self.instances[inst as usize];
        let local = &instance.nav.nodes[n as usize];
        instance.to_global(local.i as i32, local.j as i32)
    }

    /// Where a node is: feet (a swimmer's float height), at its column's center.
    pub fn node_pos(&self, node: NodeId) -> Vec3 {
        let (inst, n) = self.split(node);
        let local = &self.instances[inst as usize].nav.nodes[n as usize];
        let c = self.column_center(self.node_column(node));
        Vec3::new(c.x, local.y(), c.z)
    }

    pub fn is_swim(&self, node: NodeId) -> bool {
        let (inst, n) = self.split(node);
        self.instances[inst as usize].nav.nodes[n as usize].swim
    }

    pub fn is_disabled(&self, node: NodeId) -> bool {
        let (inst, n) = self.split(node);
        self.instances[inst as usize].is_disabled(n)
    }

    /// The links out of `node` that are not inside its bake.
    pub fn extras_of(&self, node: NodeId) -> &[Extra] {
        let start = self.extras.partition_point(|e| e.from < node);
        let end = self.extras.partition_point(|e| e.from <= node);
        &self.extras[start..end]
    }

    /// Every move out of `node`: `(to, kind, cost, rise)`. Switched-off ends are left out.
    pub fn moves(&self, node: NodeId, out: &mut Vec<(NodeId, StepKind, u32, f32)>) {
        out.clear();
        let (inst, n) = self.split(node);
        let instance = &self.instances[inst as usize];
        if instance.is_disabled(n) {
            return;
        }
        let base = self.base[inst as usize];
        for link in instance.nav.links_of(n) {
            if !instance.is_disabled(link.to) {
                out.push((base + link.to, link.kind, link.cost, link.rise));
            }
        }
        for extra in self.extras_of(node) {
            if !self.is_disabled(extra.to) {
                out.push((extra.to, extra.kind, extra.cost, extra.rise));
            }
        }
    }

    /// The walking move from `node` to the neighbouring global column `(dx, dz)` away
    /// (one step: orthogonal or diagonal), if there is one.
    pub fn walk_toward(&self, node: NodeId, (dx, dz): (i32, i32)) -> Option<NodeId> {
        let (inst, n) = self.split(node);
        let instance = &self.instances[inst as usize];
        let local = instance.to_local_dir((dx, dz));
        let code = MOVES.iter().take(8).position(|m| *m == local)? as u8;
        let base = self.base[inst as usize];
        if let Some(link) = instance.nav.links_of(n).iter().find(|l| l.code == code && l.kind == StepKind::Walk) {
            return (!instance.is_disabled(link.to)).then_some(base + link.to);
        }
        let target = {
            let c = self.node_column(node);
            [c[0] + dx, c[1] + dz]
        };
        self.extras_of(node)
            .iter()
            .find(|e| e.kind == StepKind::Walk && !e.navlink && self.node_column(e.to) == target)
            .filter(|e| !self.is_disabled(e.to))
            .map(|e| e.to)
    }

    /// The area label of a node across the region: two nodes with different labels never
    /// reach each other.
    pub fn component(&self, node: NodeId) -> u32 {
        let (inst, n) = self.split(node);
        let area = self.instances[inst as usize].nav.nodes[n as usize].area as u32;
        self.components[(self.area_base[inst as usize] + area) as usize]
    }

    /// Whether a body of `params` plans on this navmesh: not bigger than the one baked.
    pub fn fits(&self, radius: f32, height: f32) -> bool {
        radius <= self.agent_radius + 0.01 && height <= self.agent_height + 0.01
    }

    /// `64 tiles, 16384 nodes, 896 links across edges, 2 navlinks`.
    pub fn summary(&self) -> String {
        let links = self.navlinks.iter().filter(|l| l.nodes.is_some()).count();
        format!(
            "{} tiles, {} nodes, {} links across edges, {} navlinks",
            self.instances.len(),
            self.node_count(),
            self.report.stitched,
            links
        )
    }
}

/// `RegionLoadedHooks`: the navmesh of the region that just loaded, journaled.
pub(crate) fn region_loaded(world: &mut World) {
    rebuild(world);
}

/// Builds the navmesh of the region in the world now (or removes a stale one when there
/// is no region, or it cannot be built), and journals what it did.
pub fn rebuild(world: &mut World) {
    world.remove_resource::<NavRegion>();
    if world.get_resource::<ActiveRegion>().is_none() {
        return;
    }
    match build(world) {
        Ok(nav) => {
            let report = nav.report.clone();
            let line = format!("navmesh: {} ({})", nav.title, nav.summary());
            world.insert_resource(nav);
            journal::emit(world, JournalKind::Event, line);
            let mut baked: Vec<String> = Vec::new();
            if !report.missing.is_empty() {
                baked.push(format!("no bake file: {}", report.missing.join(", ")));
            }
            if !report.stale.is_empty() {
                baked.push(format!("stale: {}", report.stale.join(", ")));
            }
            if !baked.is_empty() {
                let n = report.missing.len() + report.stale.len();
                journal::emit(
                    world,
                    JournalKind::Warn,
                    format!("navmesh: {n} tiles baked while loading ({}); bake them with `--bake-nav all` (or the nav_bake tool)", baked.join("; ")),
                );
            }
            for (tile, why) in report.failed {
                journal::emit(world, JournalKind::Warn, format!("navmesh: tile `{tile}` cannot be baked ({why}); NPCs plan on the local grid there"));
            }
        }
        Err(why) => {
            journal::emit(world, JournalKind::Warn, format!("navmesh: none for this region: {why}; NPCs plan on the local grid"));
        }
    }
}

/// The navmesh is the region's: built for it and up to date with the colliders spawned since
/// (carved). Inside ticks and in tools; a pure function of the world, so playback agrees.
pub fn refresh(world: &mut World) {
    let Some(region) = world.get_resource::<ActiveRegion>().map(|r| r.hash) else { return };
    if world.get_resource::<NavRegion>().is_none_or(|nav| nav.hash != region) {
        rebuild(world);
        return;
    }
    let next = world.get_resource::<ColliderIds>().map_or(0, |ids| ids.peek().0);
    let stale = world.resource::<NavRegion>().carve.next_id != next
        || world.resource::<NavRegion>().carve.records.iter().any(|(id, _)| world.resource::<PhysicsWorld>().by_id(*id).is_none());
    if !stale {
        return;
    }
    let _ = world.run_system_cached(sync_physics);
    let mut nav = world.remove_resource::<NavRegion>().expect("checked above");
    let tiles = tile_ids(world);
    let settings = NavSettings::of(world.resource::<ContentDb>());
    let physics = world.resource::<PhysicsWorld>();
    let changed = nav.recarve(physics, &settings, &tiles, next);
    if changed {
        nav.build_graph();
    }
    world.insert_resource(nav);
}

/// The `TileId` of every region entity with a collider.
fn tile_ids(world: &mut World) -> BTreeMap<ColliderId, TileId> {
    let mut q = world.query::<(&ColliderId, &TileId)>();
    q.iter(world).map(|(c, t)| (*c, *t)).collect()
}

/// The navmesh of the region in the world.
fn build(world: &mut World) -> Result<NavRegion, String> {
    let region = world.get_resource::<ActiveRegion>().ok_or("no region")?.clone();
    let graph = region.graph.clone().ok_or("content no longer describes every tile of its manifest")?;
    let manifest = &region.manifest;
    let settings = NavSettings::of(world.resource::<ContentDb>());
    if !whole_cells(manifest.tile_size, settings.cell) {
        return Err(format!("its tile size {} m is not a whole number of {} m cells (nav settings `cell`)", manifest.tile_size, settings.cell));
    }
    let per = (manifest.tile_size / settings.cell).round() as u32;
    let kind = if manifest.layout == Layout::Rooms { TileKind::Room } else { TileKind::Tile };
    let mut report = NavReport::default();

    // Bakes, once per id, in id order.
    let mut ids: Vec<&str> = manifest.tiles.iter().map(|t| t.tile.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let mut bakes: BTreeMap<String, Arc<NavTile>> = BTreeMap::new();
    for id in ids {
        let (nav, source) = tile_nav(world, kind, id, manifest.tile_size, &settings);
        match source {
            Source::File => report.from_files.push(id.to_owned()),
            Source::Missing => report.missing.push(id.to_owned()),
            Source::Stale => report.stale.push(id.to_owned()),
            Source::Failed(why) => report.failed.push((id.to_owned(), why)),
        }
        bakes.insert(id.to_owned(), nav);
    }

    // Instances in manifest order.
    let mut instances = Vec::with_capacity(manifest.tiles.len());
    let mut base = vec![0u32];
    let mut area_base = vec![0u32];
    let mut cell_instance = vec![u32::MAX; (manifest.size[0] * manifest.size[1]) as usize];
    for (index, placed) in manifest.tiles.iter().enumerate() {
        let nav = bakes[&placed.tile].clone();
        let turns = turns_of(placed.rot).ok_or_else(|| format!("tile `{}`: rot {} is not a quarter turn", placed.tile, placed.rot))?;
        let [w, h] = nav.size;
        let grid = if turns % 2 == 1 { [h, w] } else { [w, h] };
        for (c, slot) in graph.cell_tile.iter().enumerate() {
            if *slot == Some(index) {
                cell_instance[c] = index as u32;
            }
        }
        base.push(base[index] + nav.nodes.len() as u32);
        area_base.push(area_base[index] + nav.areas as u32);
        instances.push(Instance {
            cell: placed.at,
            tile: placed.tile.clone(),
            kind,
            turns,
            origin: [placed.at[0] * per as i32, placed.at[1] * per as i32],
            grid,
            center: manifest.tile_center(placed),
            nav,
            disabled: Vec::new(),
        });
    }
    let _ = world.run_system_cached(sync_physics);
    let tiles = tile_ids(world);
    let physics = world.resource::<PhysicsWorld>();
    let next_id = world.get_resource::<ColliderIds>().map_or(0, |ids| ids.peek().0);
    let mut nav = NavRegion {
        hash: region.hash,
        title: manifest.title(),
        cell: settings.cell,
        tile_size: manifest.tile_size,
        per,
        size: manifest.size,
        agent_radius: settings.agent.radius,
        agent_height: settings.agent.height,
        safe_drop: settings.safe_drop,
        instances,
        cell_instance,
        base,
        extras: Vec::new(),
        navlinks: Vec::new(),
        graph: TileGraph::default(),
        area_base,
        components: Vec::new(),
        report,
        carve: Carve::default(),
    };
    nav.recarve(physics, &settings, &tiles, next_id);
    nav.stitch(physics, &settings, &graph);
    let markers = navlink_markers(world);
    nav.add_navlinks(&settings, markers);
    nav.build_graph();
    Ok(nav)
}

/// A tile's bake: from this run's bakes, its file when fresh, else baked now.
fn tile_nav(world: &mut World, kind: TileKind, id: &str, tile_size: f32, settings: &NavSettings) -> (Arc<NavTile>, Source) {
    let root = asset_root(world);
    let db = world.resource::<ContentDb>();
    let hash = match super::settings::bake_hash(db, Some(&root), kind, id, tile_size, settings) {
        Ok(hash) => hash,
        Err(why) => return (Arc::new(empty_tile(kind, id, tile_size, settings)), Source::Failed(why)),
    };
    let file = std::fs::read_to_string(settings.file(&root, kind, id)).ok();
    let from_file = file.as_deref().map(|text| NavTile::from_file(text).ok().filter(|t| t.hash == hash));
    let source = match &from_file {
        Some(Some(_)) => Source::File,
        Some(None) => Source::Stale,
        None => Source::Missing,
    };
    if let Some(known) = world.get_resource::<NavBakes>().and_then(|b| b.tiles.get(&hash)).cloned() {
        return (known, source);
    }
    let made = match from_file.flatten() {
        Some(tile) => Ok(tile),
        None => bake(world, Some(&root), kind, id, tile_size, settings),
    };
    match made {
        Ok(tile) => {
            let tile = Arc::new(tile);
            world.get_resource_or_init::<NavBakes>().tiles.insert(hash, tile.clone());
            (tile, source)
        }
        Err(why) => (Arc::new(empty_tile(kind, id, tile_size, settings)), Source::Failed(why)),
    }
}

/// A tile without walkable ground: what a tile that cannot be baked counts as.
fn empty_tile(kind: TileKind, id: &str, tile_size: f32, settings: &NavSettings) -> NavTile {
    let per = (tile_size / settings.cell).round().max(1.0) as u32;
    NavTile {
        kind,
        id: id.to_owned(),
        hash: String::new(),
        tile_size,
        cell: settings.cell,
        size: [per, per],
        nodes: Vec::new(),
        first_link: vec![0],
        links: Vec::new(),
        columns: vec![0; (per * per) as usize + 1],
        areas: 0,
        portals: Vec::new(),
        portal_costs: Vec::new(),
        colliders: Vec::new(),
    }
}

/// Navlink markers with an end, in a fixed order: `(name, from, to)`.
fn navlink_markers(world: &mut World) -> Vec<(String, Vec3, Vec3)> {
    let mut q = world.query::<(&LevelMarker, &Transform)>();
    let mut out: Vec<(String, Vec3, Vec3)> = q
        .iter(world)
        .filter(|(m, _)| m.kind == MarkerKind::NavLink)
        .filter_map(|(m, tf)| m.to.map(|to| (m.name.clone(), tf.translation, to)))
        .collect();
    let bits = |v: Vec3| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
    out.sort_by(|a, b| (bits(a.1), bits(a.2), &a.0).cmp(&(bits(b.1), bits(b.2), &b.0)));
    out
}

/// Whether two shapes are the same within float noise.
fn same_shape(a: &shards_motion::Shape, b: &shards_motion::Shape) -> bool {
    use shards_motion::Shape::*;
    let close = |x: f32, y: f32| (x - y).abs() < 1e-4;
    match (a, b) {
        (Cuboid { half: x }, Cuboid { half: y }) => x.abs_diff_eq(*y, 1e-4),
        (Sphere { radius: x }, Sphere { radius: y }) => close(*x, *y),
        (Capsule { half_height: h1, radius: r1 }, Capsule { half_height: h2, radius: r2 })
        | (Cylinder { half_height: h1, radius: r1 }, Cylinder { half_height: h2, radius: r2 })
        | (Cone { half_height: h1, radius: r1 }, Cone { half_height: h2, radius: r2 }) => close(*h1, *h2) && close(*r1, *r2),
        _ => false,
    }
}

impl NavRegion {
    /// The instances whose footprint, widened by `margin`, overlaps the box `min..max`.
    fn instances_near(&self, min: Vec3, max: Vec3, margin: f32) -> Vec<u32> {
        let s = self.tile_size;
        let cells = |lo: f32, hi: f32, n: u32| {
            let a = ((lo - margin) / s).floor().max(0.0) as i64;
            let b = ((hi + margin) / s).floor().min(n as f32 - 1.0) as i64;
            a..=b
        };
        let mut out = Vec::new();
        for y in cells(min.z, max.z, self.size[1]) {
            for x in cells(min.x, max.x, self.size[0]) {
                if x < 0 || y < 0 {
                    continue;
                }
                let i = self.cell_instance[(y as u32 * self.size[0] + x as u32) as usize];
                if i != u32::MAX && !out.contains(&i) {
                    out.push(i);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Whether collider `c` is one the bake of instance `inst` was made with.
    fn baked_with(&self, inst: u32, c: &shards_motion::ColliderInfo) -> bool {
        let instance = &self.instances[inst as usize];
        let half = Vec3::new(instance.grid[0] as f32, 0.0, instance.grid[1] as f32) * self.cell * 0.5;
        // The bake's frame is not turned: its own half extents.
        let half_bake = if instance.turns % 2 == 1 { Vec3::new(half.z, 0.0, half.x) } else { half };
        let turned = rotation(instance.turns);
        instance.nav.colliders.iter().any(|b| {
            let pos = instance.center + turn(b.pos - half_bake, instance.turns);
            let rot = turned * b.rot;
            same_shape(&b.shape, &c.shape) && pos.distance(c.position) < 2e-3 && rot.dot(c.rotation).abs() > 0.99999
        })
    }

    /// Whether the baked body still stands at `node` in the world as it is.
    fn still_stands(&self, terrain: &Terrain, node: NodeId) -> bool {
        let pos = self.node_pos(node);
        match terrain.probe(pos.x, pos.z, pos.y) {
            Probe::Floor { y, .. } => !self.is_swim(node) && (y - pos.y).abs() < 0.13,
            Probe::Water { surface } => self.is_swim(node) && (terrain.swim_height(surface) - pos.y).abs() < 0.13,
            _ => false,
        }
    }

    /// Carves solids the bakes never saw: colliders spawned since the last look, and those
    /// gone since (their nodes are checked again). Whether anything changed.
    fn recarve(&mut self, physics: &PhysicsWorld, settings: &NavSettings, tiles: &BTreeMap<ColliderId, TileId>, next_id: u32) -> bool {
        let terrain = Terrain::new(physics, &settings.agent, &[]);
        let mut changed = false;
        // Gone: their nodes may be free again.
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.carve.records).into_iter().partition(|(id, _)| physics.by_id(*id).is_none());
        self.carve.records = kept;
        let mut again: Vec<NodeId> = gone.into_iter().flat_map(|(_, nodes)| nodes).collect();
        again.sort_unstable();
        again.dedup();
        for node in again {
            let still_carved = self.carve.records.iter().any(|(_, nodes)| nodes.contains(&node));
            if !still_carved && self.still_stands(&terrain, node) {
                let (inst, n) = self.split(node);
                self.instances[inst as usize].disabled[n as usize] = false;
                self.report.carved = self.report.carved.saturating_sub(1);
                changed = true;
            }
        }
        // New: every static solid from the first id not looked at yet.
        let from = self.carve.next_id;
        self.carve.next_id = next_id;
        let r = settings.agent.radius + self.cell * 0.5 + 0.05;
        let height = settings.agent.height;
        let step = settings.agent.step_height;
        let new: Vec<&shards_motion::ColliderInfo> = physics.colliders().filter(|c| c.id.0 >= from && solid(c)).collect();
        for c in new {
            let owner = tiles.get(&c.id).and_then(|t| {
                let cell = [t.x, t.y];
                self.instances.iter().position(|i| i.cell == cell).map(|i| i as u32)
            });
            let mut carved = Vec::new();
            for inst in self.instances_near(c.aabb_min, c.aabb_max, r) {
                if Some(inst) == owner && self.baked_with(inst, c) {
                    continue;
                }
                let instance = &self.instances[inst as usize];
                let lo = self.column_of(c.aabb_min - Vec3::new(r, 0.0, r));
                let hi = self.column_of(c.aabb_max + Vec3::new(r, 0.0, r));
                for gz in lo[1]..=hi[1] {
                    for gx in lo[0]..=hi[0] {
                        if self.instance_at([gx, gz]) != Some(inst) {
                            continue;
                        }
                        let (i, j) = instance.to_local([gx, gz]);
                        for n in instance.nav.column(i, j) {
                            let node = self.global(inst, n);
                            let y = instance.nav.nodes[n as usize].y();
                            // Below the step it could not be in the body's way; far above, neither.
                            if c.aabb_max.y <= y + step || c.aabb_min.y > y + height + 0.1 || self.is_disabled(node) {
                                continue;
                            }
                            if !self.still_stands(&terrain, node) {
                                carved.push(node);
                            }
                        }
                    }
                }
            }
            if carved.is_empty() {
                continue;
            }
            for &node in &carved {
                let (inst, n) = self.split(node);
                let instance = &mut self.instances[inst as usize];
                if instance.disabled.is_empty() {
                    instance.disabled = vec![false; instance.nav.nodes.len()];
                }
                instance.disabled[n as usize] = true;
            }
            self.report.carved += carved.len();
            self.carve.records.push((c.id, carved));
            changed = true;
        }
        changed
    }

    /// Links across every tile edge the sockets leave open: each boundary node gets the move
    /// the motion planner would make into the facing column, both ways.
    fn stitch(&mut self, physics: &PhysicsWorld, settings: &NavSettings, graph: &shards_world::RegionGraph) {
        let terrain = Terrain::new(physics, &settings.agent, &[]);
        let per = self.per as i32;
        let mut extras = Vec::new();
        for y in 0..self.size[1] as i32 {
            for x in 0..self.size[0] as i32 {
                for dir in [Dir::E, Dir::S] {
                    let (dx, dz) = dir.offset();
                    let (nx, ny) = (x + dx, y + dz);
                    if nx >= self.size[0] as i32 || ny >= self.size[1] as i32 {
                        continue;
                    }
                    let a = self.cell_instance[(y as u32 * self.size[0] + x as u32) as usize];
                    let b = self.cell_instance[(ny as u32 * self.size[0] + nx as u32) as usize];
                    if a == u32::MAX || b == u32::MAX || a == b {
                        continue;
                    }
                    let open = graph.open([x, y], dir);
                    let mut linked = 0;
                    for k in 0..per {
                        let ca = if dir == Dir::E { [(x + 1) * per - 1, y * per + k] } else { [x * per + k, (y + 1) * per - 1] };
                        let cb = [ca[0] + dx, ca[1] + dz];
                        if !open {
                            // Counted, for the report: both sides have ground, the sockets close it.
                            if !self.nodes_at(ca).is_empty() && !self.nodes_at(cb).is_empty() {
                                linked += 1;
                            }
                            continue;
                        }
                        linked += self.link_columns(&terrain, settings, ca, cb, &mut extras);
                        linked += self.link_columns(&terrain, settings, cb, ca, &mut extras);
                    }
                    if !open && linked > 0 {
                        self.report.closed_edges += 1;
                    }
                }
            }
        }
        self.report.stitched = extras.len();
        self.extras.extend(extras);
        self.extras.sort_by_key(|e| (e.from, e.to));
    }

    /// The moves from the nodes of column `from` into column `to`; how many.
    fn link_columns(&self, terrain: &Terrain, settings: &NavSettings, from: [i32; 2], to: [i32; 2], out: &mut Vec<Extra>) -> usize {
        let targets = self.nodes_at(to);
        if targets.is_empty() {
            return 0;
        }
        let c = self.column_center(to);
        let mut made = 0;
        for a in self.nodes_at(from) {
            if self.is_disabled(a) {
                continue;
            }
            let (inst, n) = self.split(a);
            let local = self.instances[inst as usize].nav.nodes[n as usize];
            let pos = self.node_pos(a);
            let stance = Stance { pos, swimming: local.swim, up: local.up() };
            let probe = terrain.probe(c.x, c.z, pos.y);
            let Some(step) = step_onto(terrain, stance, (c.x, c.z), probe, self.cell, settings.safe_drop) else { continue };
            let found = targets
                .clone()
                .filter(|b| !self.is_disabled(*b) && self.is_swim(*b) == step.swimming)
                .map(|b| (b, (self.node_pos(b).y - step.pos.y).abs()))
                .filter(|(_, dy)| *dy < 0.13)
                .min_by(|x, y| x.1.total_cmp(&y.1).then(x.0.cmp(&y.0)));
            if let Some((b, _)) = found {
                out.push(Extra { from: a, to: b, kind: step.kind, cost: step.cost, rise: step.rise, navlink: false });
                made += 1;
            }
        }
        made
    }

    /// The node nearest `pos` within `reach` meters (flat), preferring its own height:
    /// `(node, flat distance)`.
    pub fn nearest_node(&self, pos: Vec3, reach: f32, band: f32) -> Option<NodeId> {
        let col = self.column_of(pos);
        let rings = (reach / self.cell).ceil() as i32 + 1;
        let mut best: Option<(f32, NodeId)> = None;
        for dz in -rings..=rings {
            for dx in -rings..=rings {
                for node in self.nodes_at([col[0] + dx, col[1] + dz]) {
                    if self.is_disabled(node) {
                        continue;
                    }
                    let at = self.node_pos(node);
                    let flat = Vec2::new(at.x - pos.x, at.z - pos.z).length();
                    let dy = (at.y - pos.y).abs();
                    if flat > reach || dy > band {
                        continue;
                    }
                    let score = flat + dy * 0.5;
                    if best.is_none_or(|(s, n)| score < s || (score == s && node < n)) {
                        best = Some((score, node));
                    }
                }
            }
        }
        best.map(|(_, n)| n)
    }

    /// Navlink markers as one-way links between the nodes at their two ends.
    fn add_navlinks(&mut self, settings: &NavSettings, markers: Vec<(String, Vec3, Vec3)>) {
        let reach = (settings.agent.radius * 2.0 + self.cell).max(1.0);
        for (name, from, to) in markers {
            let ends = self.nearest_node(from, reach, 1.5).zip(self.nearest_node(to, reach, 1.5));
            if let Some((a, b)) = ends
                && a != b
            {
                let (pa, pb) = (self.node_pos(a), self.node_pos(b));
                let dy = pb.y - pa.y;
                let step = settings.agent.step_height;
                let kind = if dy > step {
                    StepKind::Mantle
                } else if dy < -step {
                    StepKind::Drop
                } else {
                    StepKind::Walk
                };
                let flat = Vec2::new(pb.x - pa.x, pb.z - pa.z).length();
                let rise = if kind == StepKind::Walk { 0.0 } else { dy.abs() };
                let cost = link_cost(kind, flat.max(self.cell), rise);
                self.extras.push(Extra { from: a, to: b, kind, cost, rise, navlink: true });
            }
            self.navlinks.push(NavLinkUse { name, from, to, nodes: ends.filter(|(a, b)| a != b) });
        }
        self.extras.sort_by_key(|e| (e.from, e.to));
    }

    /// The tile graph and the areas that reach each other, from the instances, their
    /// switched-off nodes and the links between tiles.
    pub(crate) fn build_graph(&mut self) {
        let mut graph = TileGraph { of_instance: vec![Vec::new(); self.instances.len()], ..default() };
        // Portals of each instance.
        for (inst, instance) in self.instances.iter().enumerate() {
            for (p, portal) in instance.nav.portals.iter().enumerate() {
                let node = if instance.is_disabled(portal.rep) {
                    let [nx, nz] = instance.nav.size;
                    portal
                        .nodes
                        .iter()
                        .filter(|n| !instance.is_disabled(**n))
                        .min_by_key(|n| {
                            let node = &instance.nav.nodes[**n as usize];
                            let (along, len) = if matches!(portal.side, Dir::N | Dir::S) { (node.i as u32, nx) } else { (node.j as u32, nz) };
                            ((2 * along + 1).abs_diff(len), **n)
                        })
                        .copied()
                } else {
                    Some(portal.rep)
                };
                graph.of_instance[inst].push(graph.nodes.len() as u32);
                graph.nodes.push(GraphNode { inst: inst as u32, node: node.map(|n| self.base[inst] + n), portal: Some(p as u32) });
            }
        }
        // Navlink ends.
        let mut ends: Vec<NodeId> = self.extras.iter().filter(|e| e.navlink).flat_map(|e| [e.from, e.to]).collect();
        ends.sort_unstable();
        ends.dedup();
        let mut end_index: BTreeMap<NodeId, u32> = BTreeMap::new();
        for node in ends {
            let (inst, _) = self.split(node);
            end_index.insert(node, graph.nodes.len() as u32);
            graph.of_instance[inst as usize].push(graph.nodes.len() as u32);
            graph.nodes.push(GraphNode { inst, node: Some(node), portal: None });
        }
        let mut edges: Vec<(u32, u32, u32)> = Vec::new();
        // Inside each instance: the bake's costs, or measured again where it is carved or has navlink ends.
        for (inst, instance) in self.instances.iter().enumerate() {
            let keys = &graph.of_instance[inst];
            let plain = instance.disabled.is_empty() && keys.len() == instance.nav.portals.len();
            for &a in keys {
                let Some(from) = graph.nodes[a as usize].node else { continue };
                let dist = if plain { None } else { Some(dijkstra(&instance.nav, from - self.base[inst], Some(&instance.disabled))) };
                for &b in keys {
                    let Some(to) = graph.nodes[b as usize].node.filter(|_| a != b) else { continue };
                    let cost = match (&dist, graph.nodes[a as usize].portal, graph.nodes[b as usize].portal) {
                        (None, Some(p), Some(q)) => instance.nav.portal_cost(p as usize, q as usize),
                        (Some(dist), ..) => Some(dist[(to - self.base[inst]) as usize]).filter(|c| *c != u32::MAX),
                        _ => None,
                    };
                    if let Some(cost) = cost {
                        edges.push((a, b, cost));
                    }
                }
            }
        }
        // Across tiles: a stitch joins the portals of the two sides; a navlink its two ends.
        for extra in &self.extras {
            let pair = if extra.navlink {
                end_index.get(&extra.from).copied().zip(end_index.get(&extra.to).copied())
            } else {
                self.portal_node(&graph, extra.from, extra.to).zip(self.portal_node(&graph, extra.to, extra.from))
            };
            if let Some((a, b)) = pair {
                edges.push((a, b, extra.cost));
            }
        }
        edges.sort_unstable();
        edges.dedup_by_key(|e| (e.0, e.1));
        graph.first = vec![0; graph.nodes.len() + 1];
        for &(a, ..) in &edges {
            graph.first[a as usize + 1] += 1;
        }
        for n in 0..graph.nodes.len() {
            graph.first[n + 1] += graph.first[n];
        }
        graph.edges = edges.into_iter().map(|(_, b, c)| (b, c)).collect();
        self.graph = graph;
        self.find_components();
    }

    /// The graph node of the portal `node` is in, on the side facing `toward`.
    fn portal_node(&self, graph: &TileGraph, node: NodeId, toward: NodeId) -> Option<u32> {
        let (inst, n) = self.split(node);
        let instance = &self.instances[inst as usize];
        let (a, b) = (self.node_column(node), self.node_column(toward));
        let world_side = match (b[0] - a[0], b[1] - a[1]) {
            (0, -1) => Dir::N,
            (1, 0) => Dir::E,
            (0, 1) => Dir::S,
            (-1, 0) => Dir::W,
            _ => return None,
        };
        let area = instance.nav.nodes[n as usize].area;
        let p = instance.nav.portal(instance.local_side(world_side), area)?;
        graph.of_instance[inst as usize].get(p).copied()
    }

    /// Labels areas that reach each other (links taken both ways).
    fn find_components(&mut self) {
        let total = *self.area_base.last().unwrap_or(&0) as usize;
        let mut parent: Vec<u32> = (0..total as u32).collect();
        fn find(parent: &mut [u32], mut n: u32) -> u32 {
            while parent[n as usize] != n {
                parent[n as usize] = parent[parent[n as usize] as usize];
                n = parent[n as usize];
            }
            n
        }
        let area_of = |nav: &NavRegion, node: NodeId| {
            let (inst, n) = nav.split(node);
            nav.area_base[inst as usize] + nav.instances[inst as usize].nav.nodes[n as usize].area as u32
        };
        for extra in &self.extras {
            let (a, b) = (area_of(self, extra.from), area_of(self, extra.to));
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb {
                parent[ra.max(rb) as usize] = ra.min(rb);
            }
        }
        self.components = (0..total as u32).map(|a| find(&mut parent, a)).collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::tile::NavNode;

    fn instance(turns: u8, size: [u32; 2]) -> Instance {
        let nav = NavTile {
            kind: TileKind::Tile,
            id: "t".into(),
            hash: String::new(),
            tile_size: 4.0,
            cell: 0.5,
            size,
            nodes: vec![NavNode { i: 0, j: 0, y_mm: 0, up_q: 10000, swim: false, area: 0 }],
            first_link: vec![0, 0],
            links: Vec::new(),
            columns: Vec::new(),
            areas: 1,
            portals: Vec::new(),
            portal_costs: Vec::new(),
            colliders: Vec::new(),
        };
        let grid = if turns % 2 == 1 { [size[1], size[0]] } else { size };
        Instance { cell: [0, 0], tile: "t".into(), kind: TileKind::Tile, turns, nav: Arc::new(nav), origin: [10, 20], grid, center: Vec3::ZERO, disabled: Vec::new() }
    }

    #[test]
    fn columns_and_directions_turn_with_the_tile_and_back() {
        for turns in 0..4 {
            let inst = instance(turns, [8, 6]);
            for (i, j) in [(0, 0), (7, 0), (3, 5), (7, 5)] {
                let g = inst.to_global(i, j);
                assert_eq!(inst.to_local(g), (i, j), "turns {turns}: ({i}, {j}) → {g:?}");
                assert!((0..inst.grid[0] as i32).contains(&(g[0] - 10)) && (0..inst.grid[1] as i32).contains(&(g[1] - 20)));
            }
            for d in MOVES {
                assert_eq!(inst.to_local_dir(inst.to_world_dir(d)), d);
            }
            // A step east in the bake is a step toward its east side's world side.
            let (dx, dz) = inst.to_world_dir((1, 0));
            assert_eq!(Dir::ALL.iter().find(|d| d.offset() == (dx, dz)).copied(), Some(inst.world_side(Dir::E)));
            for side in Dir::ALL {
                assert_eq!(inst.local_side(inst.world_side(side)), side);
            }
        }
        // One turn: the north-west corner goes to the north-east.
        let inst = instance(1, [8, 6]);
        assert_eq!(inst.to_global(0, 0), [10 + 5, 20]);
    }
}
