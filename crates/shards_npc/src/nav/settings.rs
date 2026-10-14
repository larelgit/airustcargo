//! How tiles are baked: [`NavSettingsDef`] (content kind `nav`), what a bake hashes, which
//! tiles there are to bake and at what size.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, PrefabDef, Refs};
use shards_motion::CharacterParams;
use shards_world::content::{resolve_set, DungeonDef, RegionDef, RoomDef, TileDef, TileGeometry, TilePiece};
use shards_world::manifest::{compact, fnv};

use super::tile::{file_hash, hex, TileKind, BAKE_VERSION, FORMAT};

/// How tiles are baked into navigation grids, and for which body. Kind `nav`; the engine
/// reads the definition `default` (the code defaults without one).
///
/// ```ron
/// // content/nav/default.nav.ron
/// { "default": (cell: 0.5, agent: Some("grunt"), safe_drop: 2.0) }
/// ```
///
/// With a `nav` definition, content checks every bake file: a missing or stale one is a
/// content error (`content check`, `--check-content`). Either way a region bakes what has
/// no fresh file when it loads, and says so in the journal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NavSettingsDef {
    /// Edge of a grid cell (m). A tile's edge must be a whole number of cells.
    pub cell: f32,
    /// The `locomotion` definition of the body tiles are baked for (its radius, height,
    /// step height, slope limit and climbing); the code defaults without one. Bigger bodies
    /// plan on the local grid instead.
    pub agent: Option<String>,
    /// Drops deeper than this are cliffs (m).
    pub safe_drop: f32,
    /// The folder of bake files, in the asset folder.
    pub dir: String,
    /// Whether content checks the bake files.
    pub check_bakes: bool,
}

impl Default for NavSettingsDef {
    fn default() -> Self {
        Self { cell: 0.5, agent: None, safe_drop: 2.0, dir: "nav".into(), check_bakes: true }
    }
}

impl NavSettingsDef {
    /// The id the engine reads.
    pub const ID: &'static str = "default";
}

impl ContentDef for NavSettingsDef {
    const KIND: &'static str = "nav";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if id != Self::ID {
            refs.error(format!("the engine reads only the nav settings `{}`; rename this one", Self::ID));
            return;
        }
        if !(self.cell.is_finite() && (0.05..=4.0).contains(&self.cell)) {
            refs.error(format!("cell {} must be between 0.05 and 4 m", self.cell));
            return;
        }
        if !(self.safe_drop.is_finite() && self.safe_drop >= 0.0) {
            refs.error(format!("safe_drop {} must be zero or more", self.safe_drop));
        }
        if self.dir.trim().is_empty() || Path::new(&self.dir).is_absolute() || self.dir.contains("..") {
            refs.error(format!("dir `{}` must be a folder inside the asset folder, e.g. \"nav\"", self.dir));
            return;
        }
        if let Some(agent) = &self.agent {
            refs.require::<CharacterParams>(agent, "agent");
        }
        let db = refs.db();
        for target in bake_targets(db) {
            for size in &target.sizes {
                if !whole_cells(*size, self.cell) {
                    refs.error(format!(
                        "{} `{}` is used at a tile size of {size} m, which is not a whole number of {} m cells; \
                         pick a cell that divides it",
                        target.kind.content_kind(),
                        target.id,
                        self.cell
                    ));
                }
            }
        }
        if !self.check_bakes {
            return;
        }
        let Some(root) = refs.asset_root().map(Path::to_path_buf) else { return };
        let settings = NavSettings::from_def(self, db);
        for target in bake_targets(db) {
            let shown = format!("{}/{}", self.dir, file_name(target.kind, &target.id));
            let what = format!("{} `{}`", target.kind.content_kind(), target.id);
            match std::fs::read_to_string(settings.file(&root, target.kind, &target.id)) {
                Err(_) => refs.error(format!(
                    "{what} has no bake file ({shown}); bake it with `--bake-nav {}` (or `--bake-nav all`)",
                    target.id
                )),
                Ok(text) => match file_hash(&text) {
                    Err(e) => refs.error(format!("{shown}: {e}")),
                    Ok(hash) => {
                        let fresh = target.sizes.iter().any(|s| bake_hash(db, Some(&root), target.kind, &target.id, *s, &settings).is_ok_and(|h| h == hash));
                        if !fresh {
                            refs.error(format!(
                                "the bake of {what} ({shown}) is stale: the {}, its prefabs or level file, or the nav settings \
                                 changed since it was baked; bake it again with `--bake-nav {}`",
                                target.kind.content_kind(),
                                target.id
                            ));
                        }
                    }
                },
            }
        }
    }
}

/// [`NavSettingsDef`] resolved against content: the body is known.
#[derive(Clone, Debug, PartialEq)]
pub struct NavSettings {
    pub cell: f32,
    pub safe_drop: f32,
    /// The body tiles are baked for.
    pub agent: CharacterParams,
    pub dir: String,
}

impl Default for NavSettings {
    fn default() -> Self {
        Self::from_def(&NavSettingsDef::default(), &ContentDb::default())
    }
}

impl NavSettings {
    /// The settings content has (`nav` `default`), else the defaults.
    pub fn of(db: &ContentDb) -> NavSettings {
        match db.get::<NavSettingsDef>(NavSettingsDef::ID) {
            Some(def) => Self::from_def(def, db),
            None => Self::from_def(&NavSettingsDef::default(), db),
        }
    }

    pub fn from_def(def: &NavSettingsDef, db: &ContentDb) -> NavSettings {
        let agent = def.agent.as_deref().and_then(|id| db.get::<CharacterParams>(id)).cloned().unwrap_or_default();
        NavSettings { cell: def.cell, safe_drop: def.safe_drop, agent, dir: def.dir.clone() }
    }

    /// Where the bake file of `id` is.
    pub fn file(&self, root: &Path, kind: TileKind, id: &str) -> PathBuf {
        root.join(&self.dir).join(file_name(kind, id))
    }
}

/// `grass.navtile.ron`.
pub fn file_name(kind: TileKind, id: &str) -> String {
    format!("{id}.{}", kind.extension())
}

/// Whether `size` meters is a whole number of `cell`s.
pub fn whole_cells(size: f32, cell: f32) -> bool {
    let n = size / cell;
    n >= 1.0 && (n - n.round()).abs() < 1e-3
}

/// A tile (or room) to bake, and the tile sizes the regions (or dungeons) using it have,
/// smallest first: a bake file holds the first.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeTarget {
    pub kind: TileKind,
    pub id: String,
    pub sizes: Vec<f32>,
}

/// Every tile some region uses and every room some dungeon uses, by kind and id. A tile in
/// no region has no size to bake at.
pub fn bake_targets(db: &ContentDb) -> Vec<BakeTarget> {
    let mut found: Vec<(TileKind, String, f32)> = Vec::new();
    let tiles: Vec<(&str, &TileDef)> = db.iter::<TileDef>().collect();
    let rooms: Vec<(&str, &RoomDef)> = db.iter::<RoomDef>().collect();
    for (_, region) in db.iter::<RegionDef>() {
        if let Ok(ids) = region.tile_ids(tiles.iter().copied()) {
            found.extend(ids.into_iter().map(|id| (TileKind::Tile, id.to_owned(), region.tile_size)));
        }
    }
    for (_, dungeon) in db.iter::<DungeonDef>() {
        let mut ids: Vec<String> = resolve_set(&dungeon.rooms, rooms.iter().copied(), |r: &RoomDef| &r.tags, "room")
            .map(|ids| ids.into_iter().map(str::to_owned).collect())
            .unwrap_or_default();
        ids.extend([dungeon.entrance.clone(), dungeon.goal.clone()]);
        ids.extend(dungeon.fixed.iter().cloned());
        for id in ids {
            if db.get::<RoomDef>(&id).is_some() {
                found.push((TileKind::Room, id, dungeon.cell_size));
            }
        }
    }
    found.retain(|(_, _, size)| size.is_finite() && *size > 0.0);
    found.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)).then(a.2.total_cmp(&b.2)));
    let mut targets: Vec<BakeTarget> = Vec::new();
    for (kind, id, size) in found {
        match targets.last_mut() {
            Some(t) if t.kind == kind && t.id == id => {
                if t.sizes.last().is_none_or(|s| s.to_bits() != size.to_bits()) {
                    t.sizes.push(size);
                }
            }
            _ => targets.push(BakeTarget { kind, id, sizes: vec![size] }),
        }
    }
    targets
}

/// The geometry of a tile or room, and its footprint in region cells (not turned).
pub fn geometry_of(db: &ContentDb, kind: TileKind, id: &str) -> Option<(TileGeometry, [u32; 2])> {
    match kind {
        TileKind::Tile => db.get::<TileDef>(id).map(|t| (t.geometry.clone(), [1, 1])),
        TileKind::Room => db.get::<RoomDef>(id).map(|r| (r.geometry.clone(), [r.size.0, r.size.1])),
    }
}

/// Hex FNV-1a over everything a bake of `id` at `tile_size` is made from: the bake format
/// and version, the definition, the prefabs its geometry spawns, its level file's bytes
/// (read under `root`), the tile size and the nav settings with the body. A bake file whose
/// hash differs is stale.
pub fn bake_hash(db: &ContentDb, root: Option<&Path>, kind: TileKind, id: &str, tile_size: f32, settings: &NavSettings) -> Result<String, String> {
    let what = kind.content_kind();
    let def = db.get_json(what, id).ok_or_else(|| format!("no {what} `{id}` in content"))?;
    let mut text = format!("{FORMAT} v{BAKE_VERSION}\n{what} {id} ");
    compact(def, &mut text);
    text.push_str(&format!("\nsize {tile_size:?} cell {:?} drop {:?}\nagent ", settings.cell, settings.safe_drop));
    let agent = serde_json::to_value(&settings.agent).map_err(|e| e.to_string())?;
    compact(&agent, &mut text);
    if let Some((geometry, _)) = geometry_of(db, kind, id) {
        match geometry {
            TileGeometry::None => {}
            TileGeometry::Inline(pieces) => {
                let mut prefabs: Vec<&str> =
                    pieces.iter().filter_map(|p| if let TilePiece::Prefab { id, .. } = p { Some(id.as_str()) } else { None }).collect();
                prefabs.sort_unstable();
                prefabs.dedup();
                for prefab in prefabs {
                    text.push_str(&format!("\nprefab {prefab} "));
                    match db.get_json(PrefabDef::KIND, prefab) {
                        Some(json) => compact(json, &mut text),
                        None => text.push_str("missing"),
                    }
                }
            }
            TileGeometry::Level { path } => {
                let digest = root.and_then(|r| std::fs::read(r.join(&path)).ok()).map_or_else(|| "unreadable".to_owned(), |b| hex(fnv(&b)));
                text.push_str(&format!("\nlevel {path} {digest}"));
            }
        }
    }
    Ok(hex(fnv(text.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_sizes_must_be_whole_cells() {
        assert!(whole_cells(16.0, 0.5) && whole_cells(8.0, 0.25) && whole_cells(0.5, 0.5));
        assert!(!whole_cells(16.2, 0.5) && !whole_cells(0.25, 0.5));
    }
}
