//! How tiles are baked: [`NavSettingsDef`] (content kind `nav`), what a bake hashes, which
//! tiles there are to bake and at what size.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, PrefabDef, Refs};
use shards_motion::CharacterParams;
use shards_world::content::{resolve_set, DungeonDef, RegionDef, RoomDef, TileDef, TileGeometry, TilePiece};
use shards_world::manifest::{compact, fnv};

use super::tile::{file_hash, hex, TileKind, BAKE_VERSION, FORMAT};

/// The body class every navmesh has: the `agent` of the nav settings. Its bakes are in the
/// bake folder itself (`nav/<tile>.navtile.ron`), the other classes' in a folder each
/// (`nav/<class>/<tile>.navtile.ron`).
pub const DEFAULT_BODY: &str = "agent";

/// How tiles are baked into navigation grids, and for which bodies. Kind `nav`; the engine
/// reads the definition `default` (the code defaults without one).
///
/// ```ron
/// // content/nav/default.nav.ron
/// { "default": (cell: 0.5, agent: Some("grunt"), safe_drop: 2.0,
///                bodies: {"large": (locomotion: Some("ogre"))}) }
/// ```
///
/// Every tile is baked once per body class: `agent`, then each of `bodies`. An NPC routes on
/// the tightest class its body fits (the smallest radius at least its own); a body wider
/// than every class plans on the local grid.
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
    /// step height, slope limit and climbing); the code defaults without one. The body class
    /// `agent`.
    pub agent: Option<String>,
    /// More body classes, by name (lowercase letters, digits, `_`): each is baked into its
    /// own folder, `<dir>/<name>/`.
    pub bodies: BTreeMap<String, NavBodyDef>,
    /// Drops deeper than this are cliffs (m).
    pub safe_drop: f32,
    /// The folder of bake files, in the asset folder.
    pub dir: String,
    /// Whether content checks the bake files.
    pub check_bakes: bool,
}

impl Default for NavSettingsDef {
    fn default() -> Self {
        Self { cell: 0.5, agent: None, safe_drop: 2.0, bodies: BTreeMap::new(), dir: "nav".into(), check_bakes: true }
    }
}

/// A body class of the nav settings besides `agent`: a body the tiles are baked for too.
///
/// ```ron
/// "large": (locomotion: Some("ogre")),
/// "wide": (radius: Some(1.2), height: Some(2.6)),
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NavBodyDef {
    /// The `locomotion` definition of the body (radius, height, step height, slope limit,
    /// climbing, swimming); without one, the `agent`'s.
    pub locomotion: Option<String>,
    /// Its radius instead of the locomotion's (m).
    pub radius: Option<f32>,
    /// Its height instead of the locomotion's (m). With either, the body must still be a
    /// capsule its locomotion allows: at least as tall as it is wide, and wider than twice
    /// the skin width (content checks it).
    pub height: Option<f32>,
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
        for (name, body) in &self.bodies {
            if name == DEFAULT_BODY {
                refs.error(format!("bodies: `{DEFAULT_BODY}` is the body `agent` names; call this class something else, e.g. `large`"));
            } else if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
                refs.error(format!("bodies: `{name}` must be lowercase letters, digits and `_`: it names the folder of its bakes"));
            }
            if let Some(locomotion) = &body.locomotion {
                refs.require::<CharacterParams>(locomotion, &format!("bodies.{name}.locomotion"));
            }
            for (field, value) in [("radius", body.radius), ("height", body.height)] {
                if let Some(v) = value.filter(|v| !(v.is_finite() && *v > 0.0)) {
                    refs.error(format!("bodies.{name}.{field} {v} must be more than 0 m"));
                }
            }
        }
        let db = refs.db();
        let classes = NavSettings::classes(self, db);
        // A class's own radius and height stand instead of its body's: they must still make a
        // capsule its locomotion allows (the rest of the body is checked as a `locomotion`).
        for ((name, body), class) in self.bodies.iter().zip(classes.iter().skip(1)) {
            let sized = [body.radius, body.height].into_iter().flatten().all(|v| v.is_finite() && v > 0.0);
            if !sized || (body.radius.is_none() && body.height.is_none()) {
                continue;
            }
            let CharacterParams { radius, height, skin_width, .. } = class.agent;
            if height < 2.0 * radius {
                refs.error(format!(
                    "bodies.{name}: height {height} is less than the capsule's diameter {}; give the class a height of at \
                     least {} m or a smaller radius",
                    2.0 * radius,
                    2.0 * radius
                ));
            }
            if body.radius.is_some() && skin_width >= radius * 0.5 {
                refs.error(format!(
                    "bodies.{name}: skin_width {skin_width} is too large for radius {radius}; give the class a radius \
                     above {} m (twice its locomotion's skin_width)",
                    2.0 * skin_width
                ));
            }
        }
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
        for settings in classes {
            // The agent's files are named as ever; another class's say whose they are.
            let body = if settings.is_default() { String::new() } else { format!(" for the body `{}`", settings.body) };
            for target in bake_targets(db) {
                let shown = settings.shown(target.kind, &target.id);
                let what = format!("{} `{}`", target.kind.content_kind(), target.id);
                match std::fs::read_to_string(settings.file(&root, target.kind, &target.id)) {
                    Err(_) => refs.error(format!(
                        "{what} has no bake file{body} ({shown}); bake it with `--bake-nav {}` (or `--bake-nav all`)",
                        target.id
                    )),
                    Ok(text) => match file_hash(&text) {
                        Err(e) => refs.error(format!("{shown}: {e}")),
                        Ok(hash) => {
                            let fresh = target.sizes.iter().any(|s| bake_hash(db, Some(&root), target.kind, &target.id, *s, &settings).is_ok_and(|h| h == hash));
                            if !fresh {
                                refs.error(format!(
                                    "the bake of {what}{body} ({shown}) is stale: the {}, its prefabs or level file, or the nav \
                                     settings changed since it was baked; bake it again with `--bake-nav {}`",
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
}

/// [`NavSettingsDef`] resolved against content for one body class: the body is known.
#[derive(Clone, Debug, PartialEq)]
pub struct NavSettings {
    pub cell: f32,
    pub safe_drop: f32,
    /// The body tiles are baked for.
    pub agent: CharacterParams,
    pub dir: String,
    /// Its body class: [`DEFAULT_BODY`], or a name of [`NavSettingsDef::bodies`].
    pub body: String,
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

    /// The settings of the body class `agent`.
    pub fn from_def(def: &NavSettingsDef, db: &ContentDb) -> NavSettings {
        let agent = def.agent.as_deref().and_then(|id| db.get::<CharacterParams>(id)).cloned().unwrap_or_default();
        NavSettings { cell: def.cell, safe_drop: def.safe_drop, agent, dir: def.dir.clone(), body: DEFAULT_BODY.to_owned() }
    }

    /// The settings of every body class content has: `agent` first, then the others by name.
    pub fn all(db: &ContentDb) -> Vec<NavSettings> {
        match db.get::<NavSettingsDef>(NavSettingsDef::ID) {
            Some(def) => Self::classes(def, db),
            None => Self::classes(&NavSettingsDef::default(), db),
        }
    }

    /// The settings of every body class of `def`: `agent` first, then the others by name. A
    /// class without a locomotion has the agent's body; its own radius and height stand
    /// instead of the locomotion's when given.
    pub fn classes(def: &NavSettingsDef, db: &ContentDb) -> Vec<NavSettings> {
        let base = Self::from_def(def, db);
        let mut out = vec![base.clone()];
        for (name, body) in &def.bodies {
            let mut agent =
                body.locomotion.as_deref().and_then(|id| db.get::<CharacterParams>(id)).cloned().unwrap_or_else(|| base.agent.clone());
            if let Some(radius) = body.radius {
                agent.radius = radius;
            }
            if let Some(height) = body.height {
                agent.height = height;
            }
            out.push(NavSettings { agent, body: name.clone(), ..base.clone() });
        }
        out
    }

    /// Whether it is the class `agent`.
    pub fn is_default(&self) -> bool {
        self.body == DEFAULT_BODY
    }

    /// The bake file of `id` inside the bake folder: `grass.navtile.ron`, for another class
    /// `large/grass.navtile.ron`.
    pub fn in_dir(&self, kind: TileKind, id: &str) -> String {
        if self.is_default() { file_name(kind, id) } else { format!("{}/{}", self.body, file_name(kind, id)) }
    }

    /// The bake file of `id` as content errors name it: `nav/large/grass.navtile.ron`.
    pub fn shown(&self, kind: TileKind, id: &str) -> String {
        format!("{}/{}", self.dir, self.in_dir(kind, id))
    }

    /// Where the bake file of `id` is.
    pub fn file(&self, root: &Path, kind: TileKind, id: &str) -> PathBuf {
        root.join(&self.dir).join(self.in_dir(kind, id))
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
/// and version, the definition, the prefabs its geometry spawns (a level file's nodes
/// included) with every prefab they extend, its level file's bytes (read under `root`), the
/// tile size and the nav settings with the body (and its class, but for `agent`). A bake
/// file whose hash differs is stale.
pub fn bake_hash(db: &ContentDb, root: Option<&Path>, kind: TileKind, id: &str, tile_size: f32, settings: &NavSettings) -> Result<String, String> {
    let what = kind.content_kind();
    let def = db.get_json(what, id).ok_or_else(|| format!("no {what} `{id}` in content"))?;
    let mut text = format!("{FORMAT} v{BAKE_VERSION}\n{what} {id} ");
    compact(def, &mut text);
    text.push_str(&format!("\nsize {tile_size:?} cell {:?} drop {:?}\nagent ", settings.cell, settings.safe_drop));
    let agent = serde_json::to_value(&settings.agent).map_err(|e| e.to_string())?;
    compact(&agent, &mut text);
    if !settings.is_default() {
        // Two classes of the same numbers still have bakes of their own.
        text.push_str(&format!("\nbody {}", settings.body));
    }
    if let Some((geometry, _)) = geometry_of(db, kind, id) {
        match geometry {
            TileGeometry::None => {}
            TileGeometry::Inline(pieces) => {
                let prefabs: Vec<String> =
                    pieces.iter().filter_map(|p| if let TilePiece::Prefab { id, .. } = p { Some(id.clone()) } else { None }).collect();
                hash_prefabs(db, prefabs, &mut text);
            }
            TileGeometry::Level { path } => {
                let bytes = root.and_then(|r| std::fs::read(r.join(&path)).ok());
                let digest = bytes.as_deref().map_or_else(|| "unreadable".to_owned(), |b| hex(fnv(b)));
                text.push_str(&format!("\nlevel {path} {digest}"));
                // The prefabs its nodes spawn are part of its geometry too.
                if let Some(level) = bytes.and_then(|b| shards_level::LevelAsset::from_slice(&path, &b).ok()) {
                    let prefabs: Vec<String> =
                        level.nodes.iter().filter_map(|n| n.extras.get("prefab").and_then(|p| p.as_str()).map(str::to_owned)).collect();
                    hash_prefabs(db, prefabs, &mut text);
                }
            }
        }
    }
    Ok(hex(fnv(text.as_bytes())))
}

/// Appends each of `prefabs` (sorted, once) to a bake hash's text, then every prefab it
/// extends: a change down a chain changes the hash.
fn hash_prefabs(db: &ContentDb, mut prefabs: Vec<String>, text: &mut String) {
    prefabs.sort_unstable();
    prefabs.dedup();
    for prefab in prefabs {
        text.push_str(&format!("\nprefab {prefab} "));
        match db.get_json(PrefabDef::KIND, &prefab) {
            Some(json) => compact(json, text),
            None => text.push_str("missing"),
        }
        // Its parents, nearest first (a circle stops where it closes).
        let mut seen = vec![prefab.clone()];
        let mut next = db.get::<PrefabDef>(&prefab).and_then(|p| p.extends.clone());
        while let Some(parent) = next.filter(|p| !seen.contains(p)) {
            text.push_str(&format!("\nextends {parent} "));
            match db.get_json(PrefabDef::KIND, &parent) {
                Some(json) => compact(json, text),
                None => text.push_str("missing"),
            }
            next = db.get::<PrefabDef>(&parent).and_then(|p| p.extends.clone());
            seen.push(parent);
        }
    }
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
