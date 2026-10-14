//! `RegionManifest`: a generated region as data, kept forever.
//!
//! The manifest is what saves store (E8) and what [`spawn_region`](crate::region::spawn_region)
//! builds the world from; loading one never runs the generator again, so a game update
//! that changes the generator does not rebuild old worlds. It names the seed, the
//! generator version, the attempt that passed (or the fallback), every tile with its turn,
//! the points of interest with their positions, the roads or doors between them, and a hash
//! of the content it was made from.
//!
//! [`RegionManifest::to_canonical`] is byte-stable: keys sorted, floats with at most three
//! decimals, one tile per line. The same seed, version and content give the same bytes;
//! [`RegionManifest::hash`] is FNV-1a over them.

use std::fmt::Write as _;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shards_content::ContentDb;

/// Format tag of manifests this build writes and reads.
pub const FORMAT: &str = "shards-region/1";

/// How the region is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// One tile per cell (`region` content).
    Grid,
    /// Rooms of several cells joined by doors (`dungeon` content).
    Rooms,
}

/// A tile (or room) where it was put.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacedTile {
    /// Its north-west cell (x east, y south).
    pub at: [i32; 2],
    /// Cells it covers once turned (rooms); 1×1 for tiles.
    #[serde(default = "one_by_one", skip_serializing_if = "is_one_by_one")]
    pub size: [u32; 2],
    /// `tile` or `room` id.
    pub tile: String,
    /// Degrees clockwise.
    pub rot: u16,
    /// The decoration prefab picked for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decor: Option<String>,
}

fn one_by_one() -> [u32; 2] {
    [1, 1]
}

fn is_one_by_one(size: &[u32; 2]) -> bool {
    *size == [1, 1]
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

fn is_origin(at: &[f32; 2]) -> bool {
    *at == [0.0, 0.0]
}

/// A point of interest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPoi {
    /// `camp.1`: its type and number, unique in the region.
    pub id: String,
    /// `poi` content id.
    pub poi: String,
    /// The cell it is in.
    pub cell: [i32; 2],
    /// World position, millimeters.
    pub pos: [f32; 3],
    /// The walkable area of its tile it is in.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub area: usize,
}

impl ManifestPoi {
    pub fn position(&self) -> Vec3 {
        Vec3::from_array(self.pos)
    }
}

/// A road between two points, or a door between two rooms.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestLink {
    /// `road` or `door`.
    pub kind: String,
    /// Point of interest ids (roads) or `room.N` (doors).
    pub a: String,
    pub b: String,
    /// The cells it runs through, `a` first.
    pub cells: Vec<[i32; 2]>,
}

/// A generated region: everything needed to build it again, nothing derived.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegionManifest {
    /// [`FORMAT`].
    pub format: String,
    /// `region` or `dungeon` id.
    pub region: String,
    pub layout: Layout,
    pub seed: u64,
    /// Generator version of the region definition.
    pub version: u32,
    /// The attempt that passed, from 1; 0 when the fallback was used.
    pub attempt: u32,
    /// The `tile_map` used when no attempt passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// Cells (x, y).
    pub size: [u32; 2],
    /// Edge of a cell, meters.
    pub tile_size: f32,
    /// The world position of the region's north-west corner, meters `(x, z)`. Left out of
    /// the canonical text when it is `(0, 0)`, so manifests of regions at the world's origin
    /// keep their bytes.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub origin: [f32; 2],
    /// Row-major for grids (one per cell); rooms by their north-west cell.
    pub tiles: Vec<PlacedTile>,
    pub pois: Vec<ManifestPoi>,
    pub links: Vec<ManifestLink>,
    /// The point of interest players appear at.
    pub spawn: Option<String>,
    /// FNV-1a over the definitions it was made from (hex).
    pub content_hash: String,
}

impl RegionManifest {
    /// Canonical JSON: sorted keys, at most three decimals, one entry of each list per
    /// line. Same manifest, same bytes.
    pub fn to_canonical(&self) -> String {
        let value = serde_json::to_value(self).expect("a manifest is plain data");
        let mut out = String::new();
        let Value::Object(map) = &value else { unreachable!("a manifest is an object") };
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        out.push_str("{\n");
        for (i, key) in keys.iter().enumerate() {
            let _ = write!(out, "  {}: ", json_string(key));
            match &map[*key] {
                Value::Array(items) if items.iter().any(|v| v.is_object() || v.is_array()) => {
                    out.push_str("[\n");
                    for (j, item) in items.iter().enumerate() {
                        out.push_str("    ");
                        compact(item, &mut out);
                        out.push_str(if j + 1 < items.len() { ",\n" } else { "\n" });
                    }
                    out.push_str("  ]");
                }
                other => compact(other, &mut out),
            }
            out.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
        }
        out.push_str("}\n");
        out
    }

    /// Reads a manifest written by [`to_canonical`](Self::to_canonical) (or any JSON of the
    /// same shape).
    pub fn from_json(text: &str) -> Result<Self, String> {
        let manifest: RegionManifest = serde_json::from_str(text).map_err(|e| format!("not a region manifest: {e}"))?;
        if manifest.format != FORMAT {
            return Err(format!("a manifest of format `{}`; this build reads `{FORMAT}`", manifest.format));
        }
        Ok(manifest)
    }

    /// FNV-1a over the canonical bytes.
    pub fn hash(&self) -> u64 {
        fnv(self.to_canonical().as_bytes())
    }

    /// The world position of the region's north-west corner (y 0).
    pub fn corner(&self) -> Vec3 {
        Vec3::new(self.origin[0], 0.0, self.origin[1])
    }

    /// The center of cell `(x, y)` on the ground plane.
    pub fn cell_center(&self, [x, y]: [i32; 2]) -> Vec3 {
        self.corner() + Vec3::new((x as f32 + 0.5) * self.tile_size, 0.0, (y as f32 + 0.5) * self.tile_size)
    }

    /// The center of a placed tile or room.
    pub fn tile_center(&self, tile: &PlacedTile) -> Vec3 {
        let [x, y] = tile.at;
        let [w, h] = tile.size;
        self.corner() + Vec3::new((x as f32 + w as f32 * 0.5) * self.tile_size, 0.0, (y as f32 + h as f32 * 0.5) * self.tile_size)
    }

    /// The cell `pos` is in (it may be outside the region).
    pub fn cell_of(&self, pos: Vec3) -> [i32; 2] {
        let local = pos - self.corner();
        [(local.x / self.tile_size).floor() as i32, (local.z / self.tile_size).floor() as i32]
    }

    /// Whether `cell` is inside the region.
    pub fn contains(&self, [x, y]: [i32; 2]) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.size[0] && (y as u32) < self.size[1]
    }

    pub fn poi(&self, id: &str) -> Option<&ManifestPoi> {
        self.pois.iter().find(|p| p.id == id)
    }

    /// Where players appear: the spawn point of interest, else the middle of the region.
    pub fn spawn_position(&self) -> Vec3 {
        match self.spawn.as_deref().and_then(|id| self.poi(id)) {
            Some(poi) => poi.position(),
            None => self.corner() + Vec3::new(self.size[0] as f32 * self.tile_size * 0.5, 0.0, self.size[1] as f32 * self.tile_size * 0.5),
        }
    }

    /// `forest_a seed 42 v3`.
    pub fn title(&self) -> String {
        format!("{} seed {} v{}", self.region, self.seed, self.version)
    }
}

/// FNV-1a, 64 bits.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A JSON value on one line, keys sorted, floats with at most three decimals.
pub fn compact(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => out.push_str(&json_string(s)),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                compact(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&json_string(key));
                out.push_str(": ");
                compact(&map[*key], out);
            }
            out.push('}');
        }
    }
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).expect("strings serialize")
}

/// Integers as they are; other numbers with three decimals, trailing zeros dropped.
fn number(n: &serde_json::Number) -> String {
    if n.is_u64() || n.is_i64() {
        return n.to_string();
    }
    let f = n.as_f64().unwrap_or(0.0);
    let text = format!("{f:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".to_owned() } else { text.to_owned() }
}

/// Hex FNV-1a over the canonical JSON of definitions `(kind, id)` (sorted, each once):
/// the content a manifest was made from.
pub fn content_hash(db: &ContentDb, defs: &[(&str, &str)]) -> String {
    let mut defs: Vec<(&str, &str)> = defs.to_vec();
    defs.sort_unstable();
    defs.dedup();
    let mut text = String::new();
    for (kind, id) in defs {
        text.push_str(kind);
        text.push(' ');
        text.push_str(id);
        text.push(' ');
        match db.get_json(kind, id) {
            Some(json) => compact(json, &mut text),
            None => text.push_str("missing"),
        }
        text.push('\n');
    }
    format!("{:016x}", fnv(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> RegionManifest {
        RegionManifest {
            format: FORMAT.into(),
            region: "meadow".into(),
            layout: Layout::Grid,
            seed: u64::MAX - 5,
            version: 3,
            attempt: 2,
            fallback: None,
            size: [2, 1],
            tile_size: 16.0,
            origin: [0.0, 0.0],
            tiles: vec![
                PlacedTile { at: [0, 0], size: [1, 1], tile: "road_end".into(), rot: 90, decor: Some("rocks".into()) },
                PlacedTile { at: [1, 0], size: [1, 1], tile: "camp".into(), rot: 270, decor: None },
            ],
            pois: vec![ManifestPoi { id: "spawn.0".into(), poi: "spawn".into(), cell: [0, 0], pos: [8.0, 0.1, 8.125], area: 0 }],
            links: vec![],
            spawn: Some("spawn.0".into()),
            content_hash: "00ff".into(),
        }
    }

    #[test]
    fn canonical_text_is_sorted_rounded_and_reads_back_to_the_same_bytes() {
        let m = manifest();
        let text = m.to_canonical();
        let keys: Vec<&str> = text.lines().filter(|l| l.starts_with("  \"")).map(|l| l.trim().split('"').nth(1).unwrap()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "{text}");
        assert!(text.contains("\"seed\": 18446744073709551610"), "all 64 bits of the seed: {text}");
        assert!(text.contains("\"pos\": [8, 0.1, 8.125]"), "{text}");
        assert!(text.contains("    {\"at\": [0, 0], \"decor\": \"rocks\", \"rot\": 90, \"tile\": \"road_end\"},\n"), "{text}");
        assert!(!text.contains("\"size\": [1, 1]"), "1×1 is the default: {text}");
        let back = RegionManifest::from_json(&text).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.to_canonical(), text);
        assert_eq!(back.hash(), m.hash());
        let mut other = m.clone();
        other.version = 4;
        assert_ne!(other.hash(), m.hash());
    }

    #[test]
    fn an_origin_moves_the_cells_and_is_written_only_when_set() {
        let mut m = manifest();
        assert!(!m.to_canonical().contains("origin"), "a region at the world's origin keeps its bytes");
        assert_eq!(m.cell_center([1, 0]), Vec3::new(24.0, 0.0, 8.0));
        m.origin = [-64.0, 80.5];
        let text = m.to_canonical();
        assert!(text.contains("\"origin\": [-64, 80.5]"), "{text}");
        assert_eq!(RegionManifest::from_json(&text).unwrap(), m);
        assert_eq!(m.cell_center([1, 0]), Vec3::new(-40.0, 0.0, 88.5));
        assert_eq!(m.tile_center(&m.tiles[0]), Vec3::new(-56.0, 0.0, 88.5));
        assert_eq!(m.cell_of(Vec3::new(-40.0, 3.0, 88.5)), [1, 0]);
        assert_eq!(m.cell_of(Vec3::new(-65.0, 0.0, 80.0)), [-1, -1], "west and north of the corner: outside");
        m.spawn = None;
        assert_eq!(m.spawn_position(), Vec3::new(-48.0, 0.0, 88.5), "the middle of the region");
    }

    #[test]
    fn foreign_formats_are_refused() {
        let mut m = manifest();
        m.format = "shards-region/0".into();
        let err = RegionManifest::from_json(&m.to_canonical()).unwrap_err();
        assert!(err.contains("this build reads `shards-region/1`"), "{err}");
        assert!(RegionManifest::from_json("{}").unwrap_err().starts_with("not a region manifest"));
    }

    #[test]
    fn numbers_are_written_one_way() {
        let n = |v: f64| number(&serde_json::Number::from_f64(v).unwrap());
        assert_eq!(n(16.0), "16");
        assert_eq!(n(0.1f32 as f64), "0.1");
        assert_eq!(n(-0.0001), "0");
        assert_eq!(n(-2.5), "-2.5");
    }
}
