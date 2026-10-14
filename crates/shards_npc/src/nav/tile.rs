//! [`NavTile`]: one tile's baked navigation, and its file.
//!
//! A tile is baked alone, in its own frame: north-west corner at the origin, north -Z, not
//! turned. Its grid has `size` cells of `cell` meters; a **node** is a cell at one floor
//! level (stacked floors, a ledge above a path, are separate nodes) where the baked body
//! stands, or swims. **Links** are the moves between nodes the motion planner's own rule
//! allows ([`step_onto`](shards_motion::perception::step_onto)): walk, swim, mantle, drop,
//! to the eight neighbours and two-cell hops over the foot of a ledge. Links never leave
//! the tile; the region stitches tiles at their edges.
//!
//! Nodes in **areas** (what reaches what, links taken both ways), **portals** (the nodes
//! of one area along one side) and the **portal costs** (the cheapest way between portals
//! inside the tile) are what the region's tile graph is made of. The tile also remembers
//! the colliders it was baked with, so a region tells them from things the bake never saw.
//!
//! The file (`<assets>/nav/<id>.navtile.ron`, rooms `.navroom.ron`) is a RON header with
//! the counts a person reads and the hash of what it was baked from, around the packed
//! grid in base64. Same tile, same bytes.

use std::fmt::Write as _;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_motion::perception::{StepKind, COST_PER_METER};
use shards_motion::Shape;
use shards_world::grid::Dir;

/// Format tag of bake files this build writes and reads.
pub const FORMAT: &str = "shards-navtile/1";

/// Version of the sampling: part of every bake hash, so files baked by another version read
/// as stale. Bump it whenever the bake, or the motion module's terrain probe or move rule it
/// samples with, changes what a tile bakes to.
pub const BAKE_VERSION: u32 = 1;

/// Moves to the eight neighbours, clockwise from north (N = -Z, E = +X), then the four
/// two-cell hops (N, E, S, W): a link's direction code is its index here.
pub const MOVES: [(i32, i32); 12] = [
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
    (-1, -1),
    (0, -2),
    (2, 0),
    (0, 2),
    (-2, 0),
];

/// Swimming moves cost this many times their length, mantles and drops more: the motion
/// planner's own costs ([`step_onto`](shards_motion::perception::step_onto)).
pub fn link_cost(kind: StepKind, dist: f32, rise: f32) -> u32 {
    let unit = COST_PER_METER;
    let base = (dist * unit) as u32;
    match kind {
        StepKind::Walk => base,
        StepKind::Swim => (dist * 2.0 * unit) as u32,
        StepKind::Mantle => base + ((3.0 + rise * 0.5) * unit) as u32,
        StepKind::Drop => base + (rise * 0.4 * unit) as u32,
    }
}

fn kind_code(kind: StepKind) -> u8 {
    match kind {
        StepKind::Walk => 0,
        StepKind::Swim => 1,
        StepKind::Mantle => 2,
        StepKind::Drop => 3,
    }
}

fn kind_of(code: u8) -> StepKind {
    match code & 3 {
        0 => StepKind::Walk,
        1 => StepKind::Swim,
        2 => StepKind::Mantle,
        _ => StepKind::Drop,
    }
}

/// What a bake is of: a region tile or a dungeon room.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileKind {
    Tile,
    Room,
}

impl TileKind {
    /// The content kind: `tile`, `room`.
    pub fn content_kind(self) -> &'static str {
        match self {
            TileKind::Tile => "tile",
            TileKind::Room => "room",
        }
    }

    /// The bake file's extension: `navtile.ron`, `navroom.ron`.
    pub fn extension(self) -> &'static str {
        match self {
            TileKind::Tile => "navtile.ron",
            TileKind::Room => "navroom.ron",
        }
    }
}

/// A cell at one floor level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavNode {
    /// Column (east, `0..size.x`) and row (south, `0..size.y`) in the tile's frame.
    pub i: u16,
    pub j: u16,
    /// Feet height (a swimmer's float height), millimeters: what the file stores.
    pub y_mm: i32,
    /// The `y` of the floor's normal, 1/10000 (10000: flat, and in water).
    pub up_q: u16,
    pub swim: bool,
    /// Walkable area (links taken both ways) inside the tile.
    pub area: u16,
}

impl NavNode {
    pub fn y(&self) -> f32 {
        self.y_mm as f32 / 1000.0
    }

    pub fn up(&self) -> f32 {
        self.up_q as f32 / 10000.0
    }
}

/// A move from one node to another inside the tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavTileLink {
    /// Index into [`NavTile::nodes`].
    pub to: u32,
    /// Index into [`MOVES`].
    pub code: u8,
    pub kind: StepKind,
    /// [`link_cost`].
    pub cost: u32,
    /// Height climbed or fallen (m).
    pub rise: f32,
}

/// The nodes of one area along one side of the tile (in its own frame).
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    pub side: Dir,
    pub area: u16,
    /// Node indices, in node order.
    pub nodes: Vec<u32>,
    /// The member nearest the middle of the side: where the tile graph measures from.
    pub rep: u32,
}

/// A solid the tile was baked with, in its frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakedCollider {
    pub shape: Shape,
    pub pos: Vec3,
    pub rot: Quat,
}

/// One tile's baked navigation (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct NavTile {
    pub kind: TileKind,
    pub id: String,
    /// Hex hash of everything it was baked from ([`bake_hash`](super::bake::bake_hash)).
    pub hash: String,
    /// Edge of a region cell (m) it was baked for.
    pub tile_size: f32,
    /// Grid cell (m).
    pub cell: f32,
    /// Grid cells across (x) and down (z), not turned.
    pub size: [u32; 2],
    /// Sorted by row, column, height.
    pub nodes: Vec<NavNode>,
    /// The links of node `n` are `links[first_link[n]..first_link[n + 1]]`, by direction code.
    pub first_link: Vec<u32>,
    pub links: Vec<NavTileLink>,
    /// The nodes of column `(i, j)` are `columns[c]..columns[c + 1]` with `c = j * size.x + i`.
    pub columns: Vec<u32>,
    /// Number of areas.
    pub areas: u16,
    /// By side (N, E, S, W), then area.
    pub portals: Vec<Portal>,
    /// `portal_costs[p * portals + q]`: the cheapest way from portal `p`'s rep to `q`'s
    /// inside the tile; `u32::MAX`: none.
    pub portal_costs: Vec<u32>,
    pub colliders: Vec<BakedCollider>,
}

impl NavTile {
    /// Node indices of column `(i, j)`, lowest first.
    pub fn column(&self, i: i32, j: i32) -> std::ops::Range<u32> {
        if i < 0 || j < 0 || i as u32 >= self.size[0] || j as u32 >= self.size[1] {
            return 0..0;
        }
        let c = (j as u32 * self.size[0] + i as u32) as usize;
        self.columns[c]..self.columns[c + 1]
    }

    /// The links out of node `n`.
    pub fn links_of(&self, n: u32) -> &[NavTileLink] {
        &self.links[self.first_link[n as usize] as usize..self.first_link[n as usize + 1] as usize]
    }

    /// Where node `n` is in the tile's frame (feet).
    pub fn local_pos(&self, n: u32) -> Vec3 {
        let node = &self.nodes[n as usize];
        Vec3::new((node.i as f32 + 0.5) * self.cell, node.y(), (node.j as f32 + 0.5) * self.cell)
    }

    /// The cost from portal `p` to portal `q` inside the tile.
    pub fn portal_cost(&self, p: usize, q: usize) -> Option<u32> {
        let c = self.portal_costs[p * self.portals.len() + q];
        (c != u32::MAX).then_some(c)
    }

    /// The portal of `side` and `area`.
    pub fn portal(&self, side: Dir, area: u16) -> Option<usize> {
        self.portals.iter().position(|p| p.side == side && p.area == area)
    }

    /// `1024 nodes, 1 area, 4 portals`.
    pub fn summary(&self) -> String {
        let areas = if self.areas == 1 { "area" } else { "areas" };
        format!("{} nodes, {} {areas}, {} portals", self.nodes.len(), self.areas, self.portals.len())
    }

    /// The file's text: same tile, same bytes.
    pub fn to_file(&self) -> String {
        let header = FileHeader {
            format: FORMAT.to_owned(),
            kind: self.kind,
            id: self.id.clone(),
            hash: self.hash.clone(),
            tile_size: self.tile_size,
            cell: self.cell,
            size: (self.size[0], self.size[1]),
            nodes: self.nodes.len() as u32,
            links: self.links.len() as u32,
            areas: self.areas as u32,
            portals: self.portals.len() as u32,
            data: base64_encode(&self.pack()),
        };
        let pretty = ron::ser::PrettyConfig::new().indentor("    ".to_owned()).new_line("\n".to_owned());
        let body = ron::ser::to_string_pretty(&header, pretty).expect("a header is plain data");
        let kind = self.kind.content_kind();
        format!(
            "// The baked navigation of {kind} `{}`: made by `--bake-nav` or the `nav_bake` tool from the {kind}'s\n\
             // definition. Do not edit; bake it again after changing the {kind}.\n{body}\n",
            self.id
        )
    }

    /// Reads a bake file; an error says what is wrong with it.
    pub fn from_file(text: &str) -> Result<NavTile, String> {
        let header = read_header(text)?;
        let bytes = base64_decode(&header.data).ok_or("its data is not base64")?;
        let mut tile = NavTile {
            kind: header.kind,
            id: header.id,
            hash: header.hash,
            tile_size: header.tile_size,
            cell: header.cell,
            size: [header.size.0, header.size.1],
            nodes: Vec::new(),
            first_link: Vec::new(),
            links: Vec::new(),
            columns: Vec::new(),
            areas: header.areas as u16,
            portals: Vec::new(),
            portal_costs: Vec::new(),
            colliders: Vec::new(),
        };
        tile.unpack(&bytes).map_err(|e| format!("its data is damaged ({e}); bake it again"))?;
        if tile.nodes.len() as u32 != header.nodes || tile.links.len() as u32 != header.links {
            return Err("its counts do not match its data; bake it again".into());
        }
        Ok(tile)
    }

    /// The grid, packed: nodes with their links (direction code and kind, the target's slot
    /// in its column), portals, portal costs, colliders. Little-endian.
    fn pack(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.nodes.len() * 24);
        out.extend_from_slice(b"NAV1");
        put_u32(&mut out, self.nodes.len() as u32);
        for (n, node) in self.nodes.iter().enumerate() {
            put_u16(&mut out, node.i);
            put_u16(&mut out, node.j);
            out.extend_from_slice(&node.y_mm.to_le_bytes());
            put_u16(&mut out, node.up_q);
            out.push(node.swim as u8);
            put_u16(&mut out, node.area);
            let links = self.links_of(n as u32);
            out.push(links.len() as u8);
            for link in links {
                let target = &self.nodes[link.to as usize];
                let slot = link.to - self.column(target.i as i32, target.j as i32).start;
                out.push(link.code | (kind_code(link.kind) << 4));
                out.push(slot as u8);
            }
        }
        put_u32(&mut out, self.portals.len() as u32);
        for portal in &self.portals {
            out.push(portal.side.index() as u8);
            put_u16(&mut out, portal.area);
            put_u32(&mut out, portal.rep);
            put_u32(&mut out, portal.nodes.len() as u32);
            for n in &portal.nodes {
                put_u32(&mut out, *n);
            }
        }
        for cost in &self.portal_costs {
            put_u32(&mut out, *cost);
        }
        put_u32(&mut out, self.colliders.len() as u32);
        for c in &self.colliders {
            let (tag, a, b, d) = match c.shape {
                Shape::Cuboid { half } => (0u8, half.x, half.y, half.z),
                Shape::Sphere { radius } => (1, radius, 0.0, 0.0),
                Shape::Capsule { half_height, radius } => (2, half_height, radius, 0.0),
                Shape::Cylinder { half_height, radius } => (3, half_height, radius, 0.0),
                Shape::Cone { half_height, radius } => (4, half_height, radius, 0.0),
            };
            out.push(tag);
            for v in [a, b, d, c.pos.x, c.pos.y, c.pos.z, c.rot.x, c.rot.y, c.rot.z, c.rot.w] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    fn unpack(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != b"NAV1" {
            return Err("unknown data version".into());
        }
        let [nx, nz] = self.size;
        if nx == 0 || nz == 0 || nx > 4096 || nz > 4096 {
            return Err(format!("a {nx}×{nz} grid"));
        }
        let count = r.u32()? as usize;
        // (node, direction code, kind, slot) until every column is known.
        let mut pending: Vec<(u32, u8, u8)> = Vec::new();
        let mut first_raw = vec![0u32];
        for _ in 0..count {
            let i = r.u16()?;
            let j = r.u16()?;
            let y_mm = r.i32()?;
            let up_q = r.u16()?;
            let swim = r.u8()? != 0;
            let area = r.u16()?;
            let links = r.u8()?;
            if i as u32 >= nx || j as u32 >= nz {
                return Err(format!("node at ({i}, {j}) outside the grid"));
            }
            self.nodes.push(NavNode { i, j, y_mm, up_q, swim, area });
            for _ in 0..links {
                let code = r.u8()?;
                let slot = r.u8()?;
                pending.push((self.nodes.len() as u32 - 1, code, slot));
            }
            first_raw.push(pending.len() as u32);
        }
        self.columns = columns_of(&self.nodes, self.size)?;
        self.first_link = first_raw;
        for (from, code, slot) in pending {
            let dir = (code & 0x0f) as usize;
            let &(di, dj) = MOVES.get(dir).ok_or("a link with an unknown direction")?;
            let node = self.nodes[from as usize];
            let column = self.column(node.i as i32 + di, node.j as i32 + dj);
            let to = column.start + slot as u32;
            if to >= column.end {
                return Err("a link to a node that is not there".into());
            }
            let kind = kind_of(code >> 4);
            let (a, b) = (self.local_pos(from), self.local_pos(to));
            let dist = if dir >= 8 { 2.0 * self.cell } else { Vec2::new(b.x - a.x, b.z - a.z).length() };
            let rise = (b.y - a.y).abs();
            let rise = if matches!(kind, StepKind::Mantle | StepKind::Drop) { rise } else { 0.0 };
            self.links.push(NavTileLink { to, code: dir as u8, kind, cost: link_cost(kind, dist, rise), rise });
        }
        let portals = r.u32()? as usize;
        for _ in 0..portals {
            let side = Dir::from_index(r.u8()? as usize);
            let area = r.u16()?;
            let rep = r.u32()?;
            let members = r.u32()? as usize;
            let mut nodes = Vec::with_capacity(members.min(1 << 16));
            for _ in 0..members {
                nodes.push(r.u32()?);
            }
            if rep as usize >= self.nodes.len() || nodes.iter().any(|n| *n as usize >= self.nodes.len()) {
                return Err("a portal names a node that is not there".into());
            }
            self.portals.push(Portal { side, area, nodes, rep });
        }
        for _ in 0..portals * portals {
            self.portal_costs.push(r.u32()?);
        }
        let colliders = r.u32()? as usize;
        for _ in 0..colliders {
            let tag = r.u8()?;
            let mut v = [0.0f32; 10];
            for slot in &mut v {
                *slot = r.f32()?;
            }
            let shape = match tag {
                0 => Shape::Cuboid { half: Vec3::new(v[0], v[1], v[2]) },
                1 => Shape::Sphere { radius: v[0] },
                2 => Shape::Capsule { half_height: v[0], radius: v[1] },
                3 => Shape::Cylinder { half_height: v[0], radius: v[1] },
                4 => Shape::Cone { half_height: v[0], radius: v[1] },
                _ => return Err("a collider of an unknown shape".into()),
            };
            self.colliders.push(BakedCollider { shape, pos: Vec3::new(v[3], v[4], v[5]), rot: Quat::from_xyzw(v[6], v[7], v[8], v[9]) });
        }
        if r.at != bytes.len() {
            return Err("bytes left over".into());
        }
        Ok(())
    }
}

/// The column index of sorted `nodes` (see [`NavTile::columns`]).
pub(crate) fn columns_of(nodes: &[NavNode], [nx, nz]: [u32; 2]) -> Result<Vec<u32>, String> {
    let mut columns = vec![0u32; (nx * nz) as usize + 1];
    let mut last = None;
    for node in nodes {
        let c = node.j as u32 * nx + node.i as u32;
        if last.is_some_and(|l| l > c) {
            return Err("nodes out of order".into());
        }
        last = Some(c);
        columns[c as usize + 1] += 1;
    }
    for c in 0..(nx * nz) as usize {
        columns[c + 1] += columns[c];
    }
    Ok(columns)
}

/// What a person sees at the top of a bake file.
#[derive(Serialize, Deserialize)]
struct FileHeader {
    format: String,
    kind: TileKind,
    id: String,
    hash: String,
    tile_size: f32,
    cell: f32,
    size: (u32, u32),
    nodes: u32,
    links: u32,
    areas: u32,
    portals: u32,
    data: String,
}

fn read_header(text: &str) -> Result<FileHeader, String> {
    let header: FileHeader = ron::from_str(text).map_err(|e| format!("not a bake file: {e}"))?;
    if header.format != FORMAT {
        return Err(format!("a bake of format `{}`; this build reads `{FORMAT}`: bake it again", header.format));
    }
    Ok(header)
}

/// The hash a bake file says it was baked from, without unpacking it.
pub fn file_hash(text: &str) -> Result<String, String> {
    read_header(text).map(|h| h.hash)
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.bytes.len()).ok_or("it ends too soon")?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("two bytes")))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn i32(&mut self) -> Result<i32, String> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().expect("four bytes")))
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * k)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// [`base64_encode`] backwards; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| BASE64.iter().position(|b| *b == c).map(|v| v as u32);
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for chunk in text.chunks(4) {
        let pad = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (k, c) in chunk.iter().enumerate() {
            let v = if k >= 4 - pad { 0 } else { value(*c)? };
            n = n << 6 | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

/// `0123abcd...`: 16 hex digits.
pub fn hex(hash: u64) -> String {
    let mut out = String::with_capacity(16);
    let _ = write!(out, "{hash:016x}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for len in 0..12 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let text = base64_encode(&bytes);
            assert_eq!(base64_decode(&text).unwrap(), bytes, "{text}");
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("TWE").is_none() && base64_decode("T!E=").is_none());
    }

    #[test]
    fn costs_follow_the_motion_planner() {
        assert_eq!(link_cost(StepKind::Walk, 0.5, 0.0), 500);
        assert_eq!(link_cost(StepKind::Swim, 0.5, 0.0), 1000);
        assert_eq!(link_cost(StepKind::Mantle, 0.5, 1.0), 500 + 3500);
        assert_eq!(link_cost(StepKind::Drop, 0.5, 1.5), 500 + 600);
    }
}
