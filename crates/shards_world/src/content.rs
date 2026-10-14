//! The world as content: tiles and their sockets, socket rules, regions, points of
//! interest, hand-made tile maps (fallbacks), rooms and dungeons.
//!
//! ```ron
//! // content/world/meadow.tile.ron
//! {
//!     "road_bend": (
//!         sockets: (n: "grass road grass", e: "grass road grass", s: "grass", w: "grass"),
//!         rotations: [0, 90, 180, 270],
//!         weight: 2.0,
//!         slots: [(at: (0, 0, 4), accepts: ["camp"])],
//!         geometry: Inline([Box(min: (-8, -1, -8), max: (8, 0, 8))]),
//!     ),
//! }
//! // content/world/meadow.region.ron
//! {
//!     "meadow": (
//!         size: (8, 8), tile_size: 16.0, version: 1,
//!         pois: [(poi: "spawn"), (poi: "camp", count: 2)],
//!         roads: true,
//!         checks: (distances: [(a: "spawn", b: "camp", min: 32.0)]),
//!         fallback: "meadow_safe",
//!     ),
//! }
//! ```
//!
//! Every definition is checked against the whole database when content loads: unknown
//! socket kinds, tiles, prefabs and points of interest are errors that name the file and
//! the closest id, and a region whose required points no tile can hold is one too.

use std::collections::BTreeSet;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{ContentDef, PrefabDef, Refs};
use shards_motion::{Shape, VolumeKind};

use crate::grid::{turns_of, Dir};
use crate::sockets::{parse_edge, Socket};

fn one() -> f32 {
    1.0
}

fn one_u32() -> u32 {
    1
}

fn yes() -> bool {
    true
}

fn no_turn() -> Vec<u16> {
    vec![0]
}

fn every_turn() -> Vec<u16> {
    vec![0, 90, 180, 270]
}

fn default_rules() -> String {
    "default".into()
}

fn default_spawn() -> String {
    "spawn".into()
}

fn default_attempts() -> u32 {
    8
}

fn default_backtracks() -> u32 {
    2000
}

fn one_cell() -> (u32, u32) {
    (1, 1)
}

// --- tiles -------------------------------------------------------------------------------

/// The sockets of a tile's four edges, each clockwise around the tile (see
/// [`sockets`](crate::sockets)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sockets {
    pub n: String,
    pub e: String,
    pub s: String,
    pub w: String,
}

impl Sockets {
    /// The edge texts, N, E, S, W.
    pub fn edges(&self) -> [&str; 4] {
        [&self.n, &self.e, &self.s, &self.w]
    }

    /// Parsed edges, N, E, S, W; an error names the edge.
    pub fn parse(&self) -> Result<[Vec<Socket>; 4], String> {
        let [n, e, s, w] = self.edges().map(parse_edge);
        let name = |dir: &str, r: Result<Vec<Socket>, String>| r.map_err(|e| format!("sockets.{dir}: {e}"));
        Ok([name("n", n)?, name("e", e)?, name("s", s)?, name("w", w)?])
    }
}

/// A point of interest a tile or room always has.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedPoi {
    /// `poi` content id.
    pub poi: String,
    /// Where, in the tile's frame (center at the origin, north -Z, before turning).
    pub at: Vec3,
    /// Which of the tile's `links` groups the point is in (0 when it has none).
    #[serde(default)]
    pub area: usize,
}

/// A place a generated point of interest may go.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoiSlot {
    /// In the tile's frame.
    pub at: Vec3,
    /// `poi` ids it takes.
    pub accepts: Vec<String>,
    /// Which of the tile's `links` groups the slot is in.
    #[serde(default)]
    pub area: usize,
}

/// Gameplay geometry of a tile or room, spawned with the region as top-level entities in
/// world space.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub enum TileGeometry {
    /// Nothing to collide with (a tile the game dresses itself).
    #[default]
    None,
    /// Pieces in the tile's frame, spawned in this order.
    Inline(Vec<TilePiece>),
    /// A level file (glTF from Blender, relative to the asset folder) whose nodes become the
    /// pieces, in the tile's frame: its origin is the tile's center, -Z is north, +Y is up.
    /// Needs a level loader ([`LevelSpawner`](crate::LevelSpawner); the engine registers
    /// `shards_level`'s). The file must exist: content that names a missing one is in error.
    Level { path: String },
}

/// One piece of inline geometry, in the tile's frame (center at the origin, north -Z,
/// before the tile turns).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum TilePiece {
    /// Static level geometry: the box between two corners.
    Box { min: Vec3, max: Vec3 },
    /// Static level geometry of any shape, centered at `at`, turned `yaw` degrees clockwise.
    Solid {
        shape: Shape,
        at: Vec3,
        #[serde(default)]
        yaw: f32,
    },
    /// A sensor: water, an updraft, a named trigger.
    Volume { kind: VolumeKind, shape: Shape, at: Vec3 },
    /// A named point for game code and other modules: spawns, NPC homes, link ends.
    Marker { name: String, at: Vec3 },
    /// A content prefab (a chest, a guard), facing `yaw` degrees clockwise from north.
    Prefab {
        id: String,
        at: Vec3,
        #[serde(default)]
        yaw: f32,
    },
}

/// A macro tile: its edges, how it may turn, how often it appears, where points of
/// interest go and what it spawns. Kind `tile`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileDef {
    /// Edge sockets, each clockwise around the tile: `"grass"`, `"grass road grass"`, `"cliff@1"`.
    pub sockets: Sockets,
    /// Turns the generator may give it, degrees clockwise: 0, 90, 180, 270.
    #[serde(default = "no_turn")]
    pub rotations: Vec<u16>,
    /// Relative frequency; a tile's turns share it.
    #[serde(default = "one")]
    pub weight: f32,
    /// At least this many in every region that uses it.
    #[serde(default)]
    pub min_count: u32,
    /// At most this many.
    #[serde(default)]
    pub max_count: Option<u32>,
    /// For tile sets (`#forest`) and limits.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Points of interest it always has.
    #[serde(default)]
    pub pois: Vec<FixedPoi>,
    /// Places for generated points of interest.
    #[serde(default)]
    pub slots: Vec<PoiSlot>,
    /// Sides walkers get between inside the tile, in groups: `[[N, S], [E, W]]` for a
    /// river crossing without a bridge. Default: every side reaches every other.
    #[serde(default)]
    pub links: Option<Vec<Vec<Dir>>>,
    /// Slope class, 0 flat; a region's `checks.max_slope` caps it.
    #[serde(default)]
    pub slope: u8,
    /// Prefabs that dress it; the generator picks one per placed tile.
    #[serde(default)]
    pub decor: Vec<String>,
    /// What it spawns.
    #[serde(default)]
    pub geometry: TileGeometry,
    /// What a window draws for it: the id the tile visual spawner gets.
    #[serde(default)]
    pub visual: Option<String>,
}

impl TileDef {
    /// Number of walkable areas: link groups, or one.
    pub fn areas(&self) -> usize {
        self.links.as_ref().map_or(1, |l| l.len().max(1))
    }

    /// Whether a point of interest of type `poi` can be in this tile (a slot or its own).
    pub fn accepts(&self, poi: &str) -> bool {
        self.slots.iter().any(|s| s.accepts.iter().any(|a| a == poi)) || self.pois.iter().any(|p| p.poi == poi)
    }
}

impl ContentDef for TileDef {
    const KIND: &'static str = "tile";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if let Err(e) = self.sockets.parse() {
            refs.error(e);
        }
        check_turns(refs, &self.rotations, "rotations");
        if !self.weight.is_finite() || self.weight < 0.0 {
            refs.error(format!("weight {} must be zero or more", self.weight));
        }
        if let Some(max) = self.max_count
            && max < self.min_count
        {
            refs.error(format!("max_count {max} is below min_count {}", self.min_count));
        }
        if let Some(groups) = &self.links {
            let mut seen = BTreeSet::new();
            for (i, group) in groups.iter().enumerate() {
                if group.is_empty() {
                    refs.error(format!("links[{i}] is empty; list the sides that connect, e.g. [N, S]"));
                }
                for side in group {
                    if !seen.insert(*side) {
                        refs.error(format!("links: side {side} is in two groups; a side belongs to one walkable area"));
                    }
                }
            }
        }
        check_pois(refs, &self.pois, &self.slots, self.areas());
        for (i, prefab) in self.decor.iter().enumerate() {
            refs.require::<PrefabDef>(prefab, &format!("decor[{i}]"));
        }
        check_geometry(refs, &self.geometry);
    }
}

fn check_turns(refs: &mut Refs, rotations: &[u16], field: &str) {
    if rotations.is_empty() {
        refs.error(format!("{field} is empty; give at least [0]"));
    }
    let mut seen = BTreeSet::new();
    for r in rotations {
        if turns_of(*r).is_none() {
            refs.error(format!("{field}: {r} is not a quarter turn; use 0, 90, 180 or 270"));
        } else if !seen.insert(*r) {
            refs.error(format!("{field}: {r} is listed twice"));
        }
    }
}

fn check_pois(refs: &mut Refs, pois: &[FixedPoi], slots: &[PoiSlot], areas: usize) {
    for (i, poi) in pois.iter().enumerate() {
        refs.require::<PoiDef>(&poi.poi, &format!("pois[{i}].poi"));
        if poi.area >= areas {
            refs.error(format!("pois[{i}].area {} is past the last links group ({})", poi.area, areas - 1));
        }
    }
    for (i, slot) in slots.iter().enumerate() {
        if slot.accepts.is_empty() {
            refs.error(format!("slots[{i}].accepts is empty; name the points of interest it takes, e.g. [\"camp\"]"));
        }
        for (j, poi) in slot.accepts.iter().enumerate() {
            refs.require::<PoiDef>(poi, &format!("slots[{i}].accepts[{j}]"));
        }
        if slot.area >= areas {
            refs.error(format!("slots[{i}].area {} is past the last links group ({})", slot.area, areas - 1));
        }
    }
}

/// Why `path` cannot be a tile's level file: it is empty or leaves the asset folder.
pub(crate) fn level_path_problem(path: &str) -> Option<String> {
    if path.trim().is_empty() {
        return Some("Level path is empty; give the level file, e.g. Level(path: \"levels/ruin.glb\")".into());
    }
    if !std::path::Path::new(path).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return Some(format!("level file `{path}` must be a path inside the asset folder, like `tiles/ruin.gltf` (relative, no `..`)"));
    }
    None
}

/// A tile's level file is a path inside the asset folder, and the file is there (when the
/// content is checked with an asset folder, as the game and `content check` do).
fn check_level_file(refs: &mut Refs, path: &str) {
    if let Some(why) = level_path_problem(path) {
        return refs.error(format!("geometry: {why}"));
    }
    if let Some(root) = refs.asset_root()
        && !root.join(path).is_file()
    {
        refs.error(format!(
            "geometry: level file `{path}` not found in the asset folder; export it from Blender to that path, or fix the path"
        ));
    }
}

fn check_geometry(refs: &mut Refs, geometry: &TileGeometry) {
    match geometry {
        TileGeometry::None => {}
        TileGeometry::Level { path } => check_level_file(refs, path),
        TileGeometry::Inline(pieces) => {
            for (i, piece) in pieces.iter().enumerate() {
                match piece {
                    TilePiece::Box { min, max } if !(min.cmplt(*max).all()) => {
                        refs.error(format!("geometry[{i}]: Box min {min} must be below max {max} on every axis"));
                    }
                    TilePiece::Prefab { id, .. } => {
                        refs.require::<PrefabDef>(id, &format!("geometry[{i}].id"));
                    }
                    _ => {}
                }
            }
        }
    }
}

// --- socket rules ------------------------------------------------------------------------

/// Which socket kinds exist and which may meet. Kind `socket_rules`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SocketRulesDef {
    /// Every socket kind tiles may use (a typo in a tile is then an error).
    pub kinds: Vec<String>,
    /// Kinds that may meet besides each kind with itself; symmetric.
    #[serde(default)]
    pub pairs: Vec<(String, String)>,
    /// Kinds that meet nothing (sealed sides; a region border).
    #[serde(default)]
    pub forbid: Vec<String>,
    /// Kinds whose height may differ across the edge (cliff faces).
    #[serde(default)]
    pub any_height: Vec<String>,
    /// How many height levels facing sockets may differ by.
    #[serde(default)]
    pub height_tolerance: u32,
    /// Kinds walkers cross (reachability checks).
    #[serde(default)]
    pub passable: Vec<String>,
    /// Kinds that are roads (road planning, isolated road checks).
    #[serde(default)]
    pub roads: Vec<String>,
}

impl ContentDef for SocketRulesDef {
    const KIND: &'static str = "socket_rules";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if self.kinds.is_empty() {
            refs.error("kinds is empty; list the socket kinds tiles use, e.g. [\"grass\", \"road\"]");
        }
        let mut seen = BTreeSet::new();
        for kind in &self.kinds {
            if let Err(e) = parse_edge(kind).and_then(|e| if e.len() == 1 && e[0].height == 0 { Ok(()) } else { Err("a kind is one word".into()) }) {
                refs.error(format!("kinds: `{kind}`: {e}"));
            }
            if !seen.insert(kind) {
                refs.error(format!("kinds: `{kind}` is listed twice"));
            }
        }
        let known = |refs: &mut Refs, kind: &str, field: &str| {
            if !self.kinds.iter().any(|k| k == kind) {
                let hint = closest(kind, self.kinds.iter().map(String::as_str)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                refs.error(format!("{field}: `{kind}` is not in kinds{hint}"));
            }
        };
        for (i, (a, b)) in self.pairs.iter().enumerate() {
            known(refs, a, &format!("pairs[{i}]"));
            known(refs, b, &format!("pairs[{i}]"));
        }
        for (field, list) in [("forbid", &self.forbid), ("any_height", &self.any_height), ("passable", &self.passable), ("roads", &self.roads)] {
            for kind in list {
                known(refs, kind, field);
            }
        }
    }
}

// --- points of interest ------------------------------------------------------------------

/// A type of point of interest: what the AI calls it, what stands there. Kind `poi`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoiDef {
    /// Its `AiKind` in observations; default: the id with a capital letter.
    #[serde(default)]
    pub label: Option<String>,
    /// Prefab spawned at the point with the region (a campfire, a boss).
    #[serde(default)]
    pub prefab: Option<String>,
}

impl PoiDef {
    /// `Camp` for `camp` unless a label says otherwise.
    pub fn label_of(def: Option<&PoiDef>, id: &str) -> String {
        if let Some(label) = def.and_then(|d| d.label.clone()) {
            return label;
        }
        let mut chars = id.chars();
        chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
    }
}

impl ContentDef for PoiDef {
    const KIND: &'static str = "poi";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if let Some(prefab) = &self.prefab {
            refs.require::<PrefabDef>(prefab, "prefab");
        }
    }
}

// --- regions -----------------------------------------------------------------------------

/// A cell with a given tile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedTile {
    /// Cell (x east, y south).
    pub at: (u32, u32),
    pub tile: String,
    /// Degrees clockwise; default: any of the tile's rotations.
    #[serde(default)]
    pub rot: Option<u16>,
}

/// How many tiles of one id or `#tag` a region may have.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    /// A tile id or `#tag`.
    pub of: String,
    #[serde(default)]
    pub min: u32,
    #[serde(default)]
    pub max: Option<u32>,
}

/// Points of interest of one type that a region must have.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoiRule {
    /// `poi` id.
    pub poi: String,
    #[serde(default = "one_u32")]
    pub count: u32,
    /// Cells it may be in (x, y); empty: anywhere.
    #[serde(default)]
    pub cells: Vec<(u32, u32)>,
}

/// How far apart points of interest of two types are, in meters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DistanceRule {
    pub a: String,
    pub b: String,
    /// Every `a` at least this far from every `b`.
    #[serde(default)]
    pub min: Option<f32>,
    /// Every `a` has a `b` within this distance.
    #[serde(default)]
    pub max: Option<f32>,
}

/// What a generated region must pass; failing ones are retried.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRules {
    /// Every point of interest reachable from the spawn over passable sockets and tile links.
    #[serde(default = "yes")]
    pub reachable: bool,
    /// The steepest tile slope class allowed.
    #[serde(default)]
    pub max_slope: Option<u8>,
    #[serde(default)]
    pub distances: Vec<DistanceRule>,
    /// Every road leads to a point of interest.
    #[serde(default = "yes")]
    pub isolated_roads: bool,
    /// Checks the game registered (`add_worldgen_check`), by name.
    #[serde(default)]
    pub custom: Vec<String>,
}

impl Default for CheckRules {
    fn default() -> Self {
        Self { reachable: true, max_slope: None, distances: Vec::new(), isolated_roads: true, custom: Vec::new() }
    }
}

/// A region made of macro tiles: its size, tile set, rules, required points, checks and
/// what to do when no attempt passes. Kind `region`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionDef {
    /// Cells across (x, east) and down (y, south).
    pub size: (u32, u32),
    /// Edge of a tile, meters.
    pub tile_size: f32,
    /// Generator version: part of every random key, so a new version gives old seeds new
    /// worlds while saved manifests stay as they were.
    pub version: u32,
    /// `socket_rules` id.
    #[serde(default = "default_rules")]
    pub rules: String,
    /// Tile ids and `#tags`; empty: every tile.
    #[serde(default)]
    pub tiles: Vec<String>,
    /// The socket every edge on the region's border must match (`"cliff"`); none: free.
    #[serde(default)]
    pub border: Option<String>,
    #[serde(default)]
    pub fixed: Vec<FixedTile>,
    /// Counts per tile or tag, on top of the tiles' own `min_count`/`max_count`.
    #[serde(default)]
    pub limits: Vec<Limit>,
    /// Points of interest every world has, placed first.
    #[serde(default)]
    pub pois: Vec<PoiRule>,
    /// Connect the required points by roads (a spanning tree).
    #[serde(default)]
    pub roads: bool,
    /// Points placed in free slots after the tiles, as many as fit.
    #[serde(default)]
    pub secondary: Vec<PoiRule>,
    #[serde(default)]
    pub checks: CheckRules,
    /// Tries before the fallback.
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    /// Solver backtracks allowed per attempt.
    #[serde(default = "default_backtracks")]
    pub backtracks: u32,
    /// `tile_map` used when no attempt passes.
    #[serde(default)]
    pub fallback: Option<String>,
    /// The point of interest players appear at.
    #[serde(default = "default_spawn")]
    pub spawn: String,
    /// NPCs farther than this from every player are dormant (meters).
    #[serde(default)]
    pub active_radius: Option<f32>,
}

/// The tile ids `entries` (ids and `#tags`; empty: all) name, sorted, or what is wrong.
pub fn resolve_set<'a, T: 'a>(
    entries: &[String],
    all: impl Iterator<Item = (&'a str, &'a T)> + Clone,
    tags: impl Fn(&T) -> &[String],
    kind: &str,
) -> Result<Vec<&'a str>, String> {
    let mut out = BTreeSet::new();
    if entries.is_empty() {
        out.extend(all.clone().map(|(id, _)| id));
    }
    for entry in entries {
        match entry.strip_prefix('#') {
            Some(tag) => {
                let tagged: Vec<&str> = all.clone().filter(|(_, def)| tags(def).iter().any(|t| t == tag)).map(|(id, _)| id).collect();
                if tagged.is_empty() {
                    let known: BTreeSet<&str> = all.clone().flat_map(|(_, d)| tags(d).iter().map(String::as_str)).collect();
                    let hint = closest(tag, known.iter().copied()).map(|c| format!(" (did you mean `#{c}`?)")).unwrap_or_default();
                    return Err(format!("no {kind} has tag `{tag}`{hint}"));
                }
                out.extend(tagged);
            }
            None => match all.clone().find(|(id, _)| *id == entry.as_str()) {
                Some((id, _)) => {
                    out.insert(id);
                }
                None => {
                    let hint = closest(entry, all.clone().map(|(id, _)| id)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                    return Err(format!("unknown {kind} `{entry}`{hint}"));
                }
            },
        }
    }
    Ok(out.into_iter().collect())
}

impl RegionDef {
    /// The tiles of this region (its set plus the fixed ones), sorted by id.
    pub fn tile_ids<'a>(&self, tiles: impl Iterator<Item = (&'a str, &'a TileDef)> + Clone) -> Result<Vec<&'a str>, String> {
        let mut ids = resolve_set(&self.tiles, tiles.clone(), |t: &TileDef| &t.tags, "tile")?;
        for fixed in &self.fixed {
            if let Some((id, _)) = tiles.clone().find(|(id, _)| *id == fixed.tile)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }
}

impl ContentDef for RegionDef {
    const KIND: &'static str = "region";

    fn validate(&self, _: &str, refs: &mut Refs) {
        let (w, h) = self.size;
        if w == 0 || h == 0 || w > 256 || h > 256 {
            refs.error(format!("size ({w}, {h}) must be 1..=256 cells each way"));
        }
        if !(self.tile_size.is_finite() && self.tile_size > 0.0) {
            refs.error(format!("tile_size {} must be a positive number of meters", self.tile_size));
        }
        if self.attempts == 0 {
            refs.error("attempts is 0; give at least 1");
        }
        let rules = refs.require::<SocketRulesDef>(&self.rules, "rules").then(|| refs.get::<SocketRulesDef>(&self.rules)).flatten();
        let all: Vec<(&str, &TileDef)> = refs.iter::<TileDef>().collect();
        // Without a tile set, the checks that need one are skipped; the others still run.
        let ids = match self.tile_ids(all.iter().copied()) {
            Ok(ids) if ids.is_empty() => {
                refs.error("tiles: the set is empty; define tiles (content/**/*.tile.ron) or list some");
                ids
            }
            Ok(ids) => ids,
            Err(e) => {
                refs.error(format!("tiles: {e}"));
                Vec::new()
            }
        };
        let known_set = !ids.is_empty();
        let set: Vec<(&str, &TileDef)> = all.iter().copied().filter(|(id, _)| ids.contains(id)).collect();
        if let Some(rules) = rules {
            for (id, tile) in &set {
                let Ok(edges) = tile.sockets.parse() else { continue };
                for socket in edges.iter().flatten() {
                    if !rules.kinds.contains(&socket.kind) {
                        let hint = closest(&socket.kind, rules.kinds.iter().map(String::as_str)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                        refs.error(format!("tile `{id}` uses socket kind `{}`, which rules `{}` does not list{hint}", socket.kind, self.rules));
                        break;
                    }
                }
            }
            if let Some(border) = &self.border {
                match parse_edge(border) {
                    Ok(edge) if edge.len() == 1 && rules.kinds.contains(&edge[0].kind) => {}
                    Ok(_) => refs.error(format!("border `{border}` must be one socket of a kind rules `{}` lists", self.rules)),
                    Err(e) => refs.error(format!("border: {e}")),
                }
            }
            if self.roads && rules.roads.is_empty() {
                refs.error(format!("roads: true, but rules `{}` names no road kinds; add `roads: [\"road\"]` to it", self.rules));
            }
        }
        for (i, fixed) in self.fixed.iter().enumerate() {
            if fixed.at.0 >= w || fixed.at.1 >= h {
                refs.error(format!("fixed[{i}].at {:?} is outside the {w}×{h} region", fixed.at));
            }
            if refs.require::<TileDef>(&fixed.tile, &format!("fixed[{i}].tile"))
                && let (Some(rot), Some(tile)) = (fixed.rot, refs.get::<TileDef>(&fixed.tile))
                && !tile.rotations.contains(&rot)
            {
                refs.error(format!("fixed[{i}].rot {rot} is not among `{}`'s rotations {:?}", fixed.tile, tile.rotations));
            }
        }
        for (i, limit) in self.limits.iter().enumerate() {
            if known_set
                && let Err(e) = resolve_set(std::slice::from_ref(&limit.of), set.iter().copied(), |t: &TileDef| &t.tags, "tile in this region")
            {
                refs.error(format!("limits[{i}].of: {e}"));
            }
            if limit.max.is_some_and(|max| max < limit.min) {
                refs.error(format!("limits[{i}]: max is below min"));
            }
        }
        for (field, list) in [("pois", &self.pois), ("secondary", &self.secondary)] {
            for (i, rule) in list.iter().enumerate() {
                if !refs.require::<PoiDef>(&rule.poi, &format!("{field}[{i}].poi")) {
                    continue;
                }
                if known_set && !set.iter().any(|(_, t)| t.accepts(&rule.poi)) {
                    refs.error(format!(
                        "{field}[{i}]: no tile in the set can hold a `{}`; give one a slot (slots: [(at: (0, 0, 0), accepts: [\"{}\"])])",
                        rule.poi, rule.poi
                    ));
                }
                for cell in &rule.cells {
                    if cell.0 >= w || cell.1 >= h {
                        refs.error(format!("{field}[{i}].cells: {cell:?} is outside the {w}×{h} region"));
                    }
                }
            }
        }
        for (i, rule) in self.checks.distances.iter().enumerate() {
            refs.require::<PoiDef>(&rule.a, &format!("checks.distances[{i}].a"));
            refs.require::<PoiDef>(&rule.b, &format!("checks.distances[{i}].b"));
            if let (Some(min), Some(max)) = (rule.min, rule.max)
                && max < min
            {
                refs.error(format!("checks.distances[{i}]: max {max} is below min {min}"));
            }
        }
        if refs.require::<PoiDef>(&self.spawn, "spawn") {
            let placed = self.pois.iter().any(|r| r.poi == self.spawn) || set.iter().any(|(_, t)| t.pois.iter().any(|p| p.poi == self.spawn));
            if known_set && !placed {
                refs.error(format!(
                    "spawn: nothing places a `{0}`, so players would have nowhere to appear; add (poi: \"{0}\") to `pois`",
                    self.spawn
                ));
            }
        }
        if let Some(fallback) = &self.fallback
            && refs.require::<TileMapDef>(fallback, "fallback")
            && let Some(map) = refs.get::<TileMapDef>(fallback)
            && let Ok(size) = map.size()
            && size != self.size
        {
            refs.error(format!("fallback `{fallback}` is {}×{}, the region {w}×{h}", size.0, size.1));
        }
    }
}

// --- hand-made maps ----------------------------------------------------------------------

/// A point of interest on a hand-made map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapPoi {
    pub poi: String,
    /// Cell (x, y); the tile there must have a slot for it.
    pub at: (u32, u32),
}

/// A hand-made region layout: what a region falls back to when no attempt passes. Kind
/// `tile_map`.
///
/// ```ron
/// { "meadow_safe": (rows: ["road_end:90 road road_end:270", "grass grass grass"], pois: [(poi: "spawn", at: (0, 0))]) }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TileMapDef {
    /// One string per row, north first: `tile` or `tile:degrees` per cell, separated by spaces.
    pub rows: Vec<String>,
    #[serde(default)]
    pub pois: Vec<MapPoi>,
}

impl TileMapDef {
    /// Cells, row-major: (tile id, quarter turns).
    pub fn cells(&self) -> Result<Vec<(String, u8)>, String> {
        let mut cells = Vec::new();
        for (r, row) in self.rows.iter().enumerate() {
            for token in row.split_whitespace() {
                let (tile, turns) = match token.split_once(':') {
                    Some((tile, deg)) => {
                        let turns = deg.parse::<u16>().ok().and_then(turns_of).ok_or_else(|| {
                            format!("rows[{r}]: `{token}`: the turn is 0, 90, 180 or 270 degrees (`tile:90`)")
                        })?;
                        (tile, turns)
                    }
                    None => (token, 0),
                };
                cells.push((tile.to_owned(), turns));
            }
        }
        Ok(cells)
    }

    /// (width, height), or what is uneven.
    pub fn size(&self) -> Result<(u32, u32), String> {
        let widths: Vec<usize> = self.rows.iter().map(|r| r.split_whitespace().count()).collect();
        match widths.first() {
            None | Some(0) => Err("rows is empty; one string per row, `tile` or `tile:90` per cell".into()),
            Some(&w) => match widths.iter().position(|&x| x != w) {
                Some(r) => Err(format!("rows[{r}] has {} cells, rows[0] has {w}", widths[r])),
                None => Ok((w as u32, widths.len() as u32)),
            },
        }
    }
}

impl ContentDef for TileMapDef {
    const KIND: &'static str = "tile_map";

    fn validate(&self, _: &str, refs: &mut Refs) {
        let size = match self.size() {
            Ok(size) => size,
            Err(e) => return refs.error(e),
        };
        let cells = match self.cells() {
            Ok(cells) => cells,
            Err(e) => return refs.error(e),
        };
        for (i, (tile, turns)) in cells.iter().enumerate() {
            let row = i / size.0 as usize;
            if refs.require::<TileDef>(tile, &format!("rows[{row}]"))
                && let Some(def) = refs.get::<TileDef>(tile)
                && !def.rotations.contains(&crate::grid::degrees_of(*turns))
            {
                refs.error(format!("rows[{row}]: `{tile}` may not turn {}°; its rotations are {:?}", crate::grid::degrees_of(*turns), def.rotations));
            }
        }
        for (i, poi) in self.pois.iter().enumerate() {
            refs.require::<PoiDef>(&poi.poi, &format!("pois[{i}].poi"));
            let (x, y) = poi.at;
            if x >= size.0 || y >= size.1 {
                refs.error(format!("pois[{i}].at {:?} is outside the {}×{} map", poi.at, size.0, size.1));
                continue;
            }
            let tile = &cells[(y * size.0 + x) as usize].0;
            if let Some(def) = refs.get::<TileDef>(tile)
                && !def.accepts(&poi.poi)
            {
                refs.error(format!("pois[{i}]: tile `{tile}` at {:?} has no slot for `{}`", poi.at, poi.poi));
            }
        }
    }
}

// --- dungeons ----------------------------------------------------------------------------

/// A door on a room's footprint.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Door {
    /// Footprint cell (x east, y south) the door is in.
    pub at: (u32, u32),
    /// The side of that cell it opens on (on the footprint's border).
    pub side: Dir,
}

/// A dungeon room module. Kind `room`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomDef {
    /// Footprint in cells (w east, h south).
    #[serde(default = "one_cell")]
    pub size: (u32, u32),
    pub doors: Vec<Door>,
    /// Turns it may take, degrees clockwise.
    #[serde(default = "every_turn")]
    pub rotations: Vec<u16>,
    #[serde(default = "one")]
    pub weight: f32,
    #[serde(default)]
    pub max_count: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Points of interest it always has (the entrance: `spawn`; the goal: `boss`).
    #[serde(default)]
    pub pois: Vec<FixedPoi>,
    #[serde(default)]
    pub slots: Vec<PoiSlot>,
    /// In the room's frame: its footprint's center at the origin, north -Z.
    #[serde(default)]
    pub geometry: TileGeometry,
    #[serde(default)]
    pub visual: Option<String>,
}

impl ContentDef for RoomDef {
    const KIND: &'static str = "room";

    fn validate(&self, _: &str, refs: &mut Refs) {
        let (w, h) = self.size;
        if w == 0 || h == 0 || w > 64 || h > 64 {
            refs.error(format!("size ({w}, {h}) must be 1..=64 cells each way"));
            return;
        }
        if self.doors.is_empty() {
            refs.error("doors is empty; a room needs at least one door to be reached");
        }
        for (i, door) in self.doors.iter().enumerate() {
            let (x, y) = door.at;
            let on_border = x < w
                && y < h
                && match door.side {
                    Dir::N => y == 0,
                    Dir::S => y == h - 1,
                    Dir::W => x == 0,
                    Dir::E => x == w - 1,
                };
            if !on_border {
                refs.error(format!("doors[{i}]: a door at {:?} on side {} is not on the {w}×{h} footprint's border", door.at, door.side));
            }
        }
        check_turns(refs, &self.rotations, "rotations");
        if !self.weight.is_finite() || self.weight < 0.0 {
            refs.error(format!("weight {} must be zero or more", self.weight));
        }
        check_pois(refs, &self.pois, &self.slots, 1);
        check_geometry(refs, &self.geometry);
    }
}

/// A dungeon: a main path of rooms from an entrance to a goal, with branches. Kind `dungeon`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DungeonDef {
    pub version: u32,
    /// Edge of a footprint cell, meters.
    pub cell_size: f32,
    /// First room of the main path (a hand-made one: a fixed node).
    pub entrance: String,
    /// Last room of the main path.
    pub goal: String,
    /// Rooms between them and on branches: ids and `#tags`.
    #[serde(default)]
    pub rooms: Vec<String>,
    /// Hand-made rooms that must be on the main path.
    #[serde(default)]
    pub fixed: Vec<String>,
    /// Rooms between entrance and goal (min, max), the fixed ones included: a minimum below
    /// the number of fixed rooms is raised to it.
    pub path: (u32, u32),
    /// Extra rooms off the main path (min, max).
    #[serde(default)]
    pub branches: (u32, u32),
    /// The most cells the whole dungeon may span (x, y); none: no limit.
    #[serde(default)]
    pub max_size: Option<(u32, u32)>,
    #[serde(default)]
    pub secondary: Vec<PoiRule>,
    #[serde(default)]
    pub checks: CheckRules,
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    #[serde(default = "default_backtracks")]
    pub backtracks: u32,
    #[serde(default = "default_spawn")]
    pub spawn: String,
    #[serde(default)]
    pub active_radius: Option<f32>,
}

impl ContentDef for DungeonDef {
    const KIND: &'static str = "dungeon";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(self.cell_size.is_finite() && self.cell_size > 0.0) {
            refs.error(format!("cell_size {} must be a positive number of meters", self.cell_size));
        }
        if self.attempts == 0 {
            refs.error("attempts is 0; give at least 1");
        }
        if self.path.1 < self.path.0 || self.branches.1 < self.branches.0 {
            refs.error("path and branches are (min, max) with min <= max");
        }
        if self.fixed.len() as u32 > self.path.1 {
            refs.error(format!("{} fixed rooms do not fit a main path of at most {}", self.fixed.len(), self.path.1));
        }
        refs.require::<RoomDef>(&self.entrance, "entrance");
        refs.require::<RoomDef>(&self.goal, "goal");
        for (i, room) in self.fixed.iter().enumerate() {
            refs.require::<RoomDef>(room, &format!("fixed[{i}]"));
            // A fixed room is never the last of the path, so it needs a door in and a door out.
            if let Some(doors) = refs.get::<RoomDef>(room).map(|r| r.doors.len()).filter(|n| *n < 2) {
                refs.error(format!(
                    "fixed[{i}]: room `{room}` has {doors} door{}, but a fixed room stands between the entrance and the goal; \
                     give it two (one to come in by, one to leave by)",
                    if doors == 1 { "" } else { "s" }
                ));
            }
        }
        let all: Vec<(&str, &RoomDef)> = refs.iter::<RoomDef>().collect();
        match resolve_set(&self.rooms, all.iter().copied(), |r: &RoomDef| &r.tags, "room") {
            Ok(ids) => {
                let through = ids.iter().filter_map(|id| refs.get::<RoomDef>(id)).any(|r| r.doors.len() >= 2);
                if self.path.1 > self.fixed.len() as u32 && !through {
                    refs.error("rooms: none has two doors, so no room can stand between two others on the main path");
                }
            }
            Err(e) => refs.error(format!("rooms: {e}")),
        }
        for (i, rule) in self.secondary.iter().enumerate() {
            refs.require::<PoiDef>(&rule.poi, &format!("secondary[{i}].poi"));
        }
        for (i, rule) in self.checks.distances.iter().enumerate() {
            refs.require::<PoiDef>(&rule.a, &format!("checks.distances[{i}].a"));
            refs.require::<PoiDef>(&rule.b, &format!("checks.distances[{i}].b"));
        }
        if refs.require::<PoiDef>(&self.spawn, "spawn")
            && let Some(entrance) = refs.get::<RoomDef>(&self.entrance)
            && !entrance.pois.iter().any(|p| p.poi == self.spawn)
        {
            refs.error(format!(
                "spawn: entrance `{}` has no `{1}` point; add pois: [(poi: \"{1}\", at: (0, 0, 0))] to it",
                self.entrance, self.spawn
            ));
        }
    }
}
