//! The `scatter` content kind: what is strewn over a region's tiles (grass, bushes, rocks), how
//! densely, on which ground, with which looks, and which mesh is drawn near and far.

use bevy::math::Vec3;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{ContentDef, Refs};
use shards_look::LookDef;
use shards_world::{DungeonDef, RegionDef, RoomDef, TileDef};

/// The most instances per square meter a scatter asks for.
pub const MAX_DENSITY: f32 = 20.0;
/// The most levels of detail a scatter has.
pub const MAX_LODS: usize = 4;
/// The steepest lean of an instance, in degrees.
pub const MAX_TILT: f32 = 60.0;
/// The most blades in a tuft.
pub const MAX_BLADES: u32 = 16;

fn one() -> f32 {
    1.0
}

/// Things strewn over the tiles of a region (`content/**/*.scatter.ron`): instances thrown at
/// random in each matching tile, the same every run (the region's seed, the tile and the
/// scatter's id decide), dropped on the ground where it is flat and low enough, each with one of
/// the looks. Only drawn: a window or `--offscreen` shows it, the simulation never sees it.
///
/// ```ron
/// {
///     "grass": (
///         tiles: ["grass", "#meadow"],
///         density: 0.6, spacing: 0.4, scale: (0.7, 1.3), tilt: 8.0,
///         looks: ["grass", "grass_dry"],
///         fade: 4.0,
///         lods: [
///             (mesh: Blades(width: 0.08, height: 0.5, count: 7), until: 30.0),
///             (mesh: Blades(width: 0.1, height: 0.5, count: 3), until: 60.0),
///         ],
///     ),
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScatterDef {
    /// Tile and room ids, and `#tags` of theirs, it is strewn on; empty: every tile and room.
    pub tiles: Vec<String>,
    /// Region and dungeon ids it is strewn in; empty: all of them.
    pub regions: Vec<String>,
    /// Instances per square meter of a tile, before the player's `graphics.foliage` (0 to 1)
    /// thins them out. At most 20.
    pub density: f32,
    /// The least distance (m) between two instances of this scatter in a tile; 0: any.
    pub spacing: f32,
    /// Each instance's size, times the mesh: uniform between the two.
    pub scale: (f32, f32),
    /// The most an instance leans from upright, in degrees, toward a random side (0 to 60).
    pub tilt: f32,
    /// Ground steeper than this many degrees gets none.
    pub max_slope: f32,
    /// The heights (m) above the tile's origin the ground may have where an instance stands:
    /// the default keeps it on the tile's floor and off the tops of columns and walls. Ground
    /// inside a solid (a wall reaching above the top of the band) gets none either.
    pub height: (f32, f32),
    /// The room (m) an instance needs around it, above the ground: none closer to a wall or a
    /// column than this. 0: only where it stands is checked.
    pub clearance: f32,
    /// The looks (`content/**/*.look.ron`) instances are drawn with, one picked for each; a look
    /// gives the colour (the meshes are white) and the light. Give them no outline: it would
    /// give every instance a hull of its own and keep them from being drawn together.
    pub looks: Vec<String>,
    /// Instances cast shadows. Off by default: grass and pebbles do not need them.
    pub shadows: bool,
    /// Meters over which one level of detail dissolves into the next, and the last into nothing;
    /// 0: they swap at once. Less than the shortest level.
    pub fade: f32,
    /// The meshes, nearest first: each is drawn until its `until` meters from the camera, the
    /// next one from there. Beyond the last, nothing is drawn. 1 to 4 levels.
    pub lods: Vec<ScatterLod>,
}

impl Default for ScatterDef {
    fn default() -> Self {
        Self {
            tiles: Vec::new(),
            regions: Vec::new(),
            density: 0.5,
            spacing: 0.0,
            scale: (1.0, 1.0),
            tilt: 0.0,
            max_slope: 30.0,
            height: (-0.5, 0.5),
            clearance: 0.0,
            looks: Vec::new(),
            shadows: false,
            fade: 0.0,
            lods: Vec::new(),
        }
    }
}

/// One level of detail: the mesh, and the distance from the camera where it ends.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScatterLod {
    pub mesh: ScatterMesh,
    /// Meters from the camera.
    pub until: f32,
}

/// The mesh of a level of detail: one of the engine's shapes, made once and shared by every
/// instance that names the same one, or a mesh of a glTF file. Each stands on its origin, +Y up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ScatterMesh {
    /// A tuft of grass: `count` thin blades fanned around the middle, leaning out, `height` tall
    /// and `width` wide at the foot, darker at the foot than at the tip. Seen from both sides.
    Blades { width: f32, height: f32, count: u32 },
    /// A cone on the ground, `sides` faces around (3 to 32): a fir, a spike, a shrub.
    Cone { radius: f32, height: f32, sides: u32 },
    /// A lumpy ball, its foot a little under the ground: a rock, or a bush with a leafy look.
    /// `detail` 0 to 3 (20 to 320 faces), `squash` its height against its width (0.1 to 2).
    Rock {
        radius: f32,
        detail: u32,
        #[serde(default = "one")]
        squash: f32,
    },
    /// A box on the ground: a crate, a slab.
    Box { size: Vec3 },
    /// A mesh in a glTF file, as Bevy names it: `"foliage/bush.glb#Mesh0/Primitive0"`; drawn
    /// white, the look gives the colour.
    Gltf(String),
}

impl ScatterMesh {
    /// `blades 7 × 0.08 × 0.5 m`, `cone r 0.6 h 1.4 (6 sides)`, `rock r 0.4 detail 1`.
    pub fn describe(&self) -> String {
        match self {
            ScatterMesh::Blades { width, height, count } => format!("blades {count} × {width} × {height} m"),
            ScatterMesh::Cone { radius, height, sides } => format!("cone r {radius} h {height} ({sides} sides)"),
            ScatterMesh::Rock { radius, detail, squash } if *squash == 1.0 => format!("rock r {radius} detail {detail}"),
            ScatterMesh::Rock { radius, detail, squash } => format!("rock r {radius} detail {detail} squash {squash}"),
            ScatterMesh::Box { size } => format!("box {} × {} × {} m", size.x, size.y, size.z),
            ScatterMesh::Gltf(path) => format!("glTF {path}"),
        }
    }

    /// Seen from both sides (thin blades): drawn without back-face culling.
    pub fn two_sided(&self) -> bool {
        matches!(self, ScatterMesh::Blades { .. })
    }

    /// The glTF file it is read from, without the `#label`.
    pub fn file(&self) -> Option<&str> {
        match self {
            ScatterMesh::Gltf(path) => Some(path.split('#').next().unwrap_or(path)),
            _ => None,
        }
    }

    fn check(&self, field: &str, refs: &mut Refs) {
        let mut positive = |name: &str, value: f32| {
            if !(value.is_finite() && value > 0.0) {
                refs.error(format!("{field}: {name} is a length above 0, not {value}"));
            }
        };
        match self {
            ScatterMesh::Blades { width, height, count } => {
                positive("width", *width);
                positive("height", *height);
                if !(1..=MAX_BLADES).contains(count) {
                    refs.error(format!("{field}: a tuft has 1 to {MAX_BLADES} blades, not {count}"));
                }
            }
            ScatterMesh::Cone { radius, height, sides } => {
                positive("radius", *radius);
                positive("height", *height);
                if !(3..=32).contains(sides) {
                    refs.error(format!("{field}: a cone has 3 to 32 sides, not {sides}"));
                }
            }
            ScatterMesh::Rock { radius, detail, squash } => {
                positive("radius", *radius);
                if *detail > 3 {
                    refs.error(format!("{field}: a rock's detail is 0 to 3, not {detail}"));
                }
                if !(0.1..=2.0).contains(squash) {
                    refs.error(format!("{field}: a rock's squash is 0.1 to 2, not {squash}"));
                }
            }
            ScatterMesh::Box { size } => {
                positive("size.x", size.x);
                positive("size.y", size.y);
                positive("size.z", size.z);
            }
            ScatterMesh::Gltf(path) => check_file(field, path, refs),
        }
    }
}

/// A glTF mesh path is `file#label`, the file inside the asset folder, there.
fn check_file(field: &str, path: &str, refs: &mut Refs) {
    let (file, label) = path.split_once('#').unwrap_or((path, ""));
    if file.trim().is_empty() {
        return refs.error(format!("{field}: Gltf names no file; give a mesh like \"foliage/bush.glb#Mesh0/Primitive0\""));
    }
    if label.is_empty() {
        return refs.error(format!(
            "{field}: Gltf `{path}` names no mesh in the file; add `#Mesh0/Primitive0` (the first mesh's first primitive)"
        ));
    }
    if !std::path::Path::new(file).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return refs.error(format!("{field}: `{file}` must be a path inside the asset folder, like `foliage/bush.glb` (relative, no `..`)"));
    }
    if let Some(root) = refs.asset_root()
        && !root.join(file).is_file()
    {
        refs.error(format!("{field}: `{file}` not found in the asset folder; export it there, or fix the path"));
    }
}

impl ScatterDef {
    /// `0.6/m² · spacing 0.4 m · 2 LODs to 60 m · looks grass, grass_dry · on grass, #meadow`.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("{}/m²", self.density)];
        if self.spacing > 0.0 {
            parts.push(format!("spacing {} m", self.spacing));
        }
        let lods = self.lods.len();
        let until = self.lods.last().map_or(0.0, |lod| lod.until);
        parts.push(format!("{lods} LOD{} to {until} m", if lods == 1 { "" } else { "s" }));
        parts.push(format!("looks {}", self.looks.join(", ")));
        if self.shadows {
            parts.push("shadows".into());
        }
        if !self.tiles.is_empty() {
            parts.push(format!("on {}", self.tiles.join(", ")));
        }
        if !self.regions.is_empty() {
            parts.push(format!("in {}", self.regions.join(", ")));
        }
        parts.join(" · ")
    }

    /// Whether it is strewn on tile or room `tile` (with `tags`).
    pub fn on_tile(&self, tile: &str, tags: &[String]) -> bool {
        self.tiles.is_empty()
            || self.tiles.iter().any(|entry| match entry.strip_prefix('#') {
                Some(tag) => tags.iter().any(|t| t == tag),
                None => entry == tile,
            })
    }

    /// Whether it is strewn in region or dungeon `region`.
    pub fn in_region(&self, region: &str) -> bool {
        self.regions.is_empty() || self.regions.iter().any(|r| r == region)
    }

    /// The distance from the camera where level `lod` starts and where it ends.
    pub fn lod_span(&self, lod: usize) -> (f32, f32) {
        let start = if lod == 0 { 0.0 } else { self.lods[lod - 1].until };
        (start, self.lods[lod].until)
    }
}

impl ContentDef for ScatterDef {
    const KIND: &'static str = "scatter";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(self.density.is_finite() && (0.0..=MAX_DENSITY).contains(&self.density)) {
            refs.error(format!("density: 0 to {MAX_DENSITY} instances per square meter, not {}", self.density));
        }
        if !(self.spacing.is_finite() && self.spacing >= 0.0) {
            refs.error(format!("spacing: meters, 0 or more, not {}", self.spacing));
        }
        let (small, large) = self.scale;
        if !(small.is_finite() && large.is_finite() && small > 0.0 && small <= large) {
            refs.error(format!("scale: ({small}, {large}) is not two sizes above 0, the smaller first"));
        }
        if !(0.0..=MAX_TILT).contains(&self.tilt) {
            refs.error(format!("tilt: 0 to {MAX_TILT} degrees, not {}", self.tilt));
        }
        if !(0.0..=90.0).contains(&self.max_slope) {
            refs.error(format!("max_slope: 0 to 90 degrees, not {}", self.max_slope));
        }
        let (low, high) = self.height;
        if !(low.is_finite() && high.is_finite() && low <= high) {
            refs.error(format!("height: ({low}, {high}) is not two heights, the lower first"));
        }
        if !(self.clearance.is_finite() && self.clearance >= 0.0) {
            refs.error(format!("clearance: meters, 0 or more, not {}", self.clearance));
        }
        if self.looks.is_empty() {
            refs.error("looks is empty; name at least one look (content/**/*.look.ron), it gives the colour");
        }
        for (i, look) in self.looks.iter().enumerate() {
            refs.require::<LookDef>(look, &format!("looks[{i}]"));
        }
        self.check_lods(refs);
        self.check_tiles(refs);
        for (i, region) in self.regions.iter().enumerate() {
            let db = refs.db();
            if db.get::<RegionDef>(region).is_none() && db.get::<DungeonDef>(region).is_none() {
                let known = db.ids::<RegionDef>().chain(db.ids::<DungeonDef>());
                let hint = closest(region, known).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                refs.error(format!("regions[{i}]: no region or dungeon `{region}`{hint}"));
            }
        }
    }
}

impl ScatterDef {
    fn check_lods(&self, refs: &mut Refs) {
        if !(1..=MAX_LODS).contains(&self.lods.len()) {
            refs.error(format!("lods: 1 to {MAX_LODS} levels of detail, not {}", self.lods.len()));
        }
        let mut before = 0.0;
        let mut shortest = f32::INFINITY;
        for (i, lod) in self.lods.iter().enumerate() {
            lod.mesh.check(&format!("lods[{i}].mesh"), refs);
            if !(lod.until.is_finite() && lod.until > before) {
                refs.error(format!("lods[{i}].until: {} must be farther than {before} m, where the level before ends", lod.until));
            }
            shortest = shortest.min(lod.until - before);
            before = lod.until;
        }
        if !(self.fade.is_finite() && self.fade >= 0.0) {
            refs.error(format!("fade: meters, 0 or more, not {}", self.fade));
        } else if self.fade > 0.0 && shortest.is_finite() && self.fade >= shortest {
            refs.error(format!("fade: {} m is not shorter than the shortest level of detail ({shortest} m)", self.fade));
        }
    }

    fn check_tiles(&self, refs: &mut Refs) {
        let db = refs.db();
        let tags = || db.iter::<TileDef>().flat_map(|(_, t)| t.tags.iter()).chain(db.iter::<RoomDef>().flat_map(|(_, r)| r.tags.iter()));
        for (i, entry) in self.tiles.iter().enumerate() {
            match entry.strip_prefix('#') {
                Some(tag) => {
                    if !tags().any(|t| t == tag) {
                        let hint = closest(tag, tags().map(String::as_str)).map(|c| format!(" (did you mean `#{c}`?)")).unwrap_or_default();
                        refs.error(format!("tiles[{i}]: no tile or room has tag `{tag}`{hint}"));
                    }
                }
                None => {
                    if db.get::<TileDef>(entry).is_none() && db.get::<RoomDef>(entry).is_none() {
                        let known = db.ids::<TileDef>().chain(db.ids::<RoomDef>());
                        let hint = closest(entry, known).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                        refs.error(format!("tiles[{i}]: no tile or room `{entry}`{hint}"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shards_content::db::Source;
    use shards_content::{ContentDb, ContentKinds};

    fn errors(scatter: &str) -> Vec<String> {
        let mut kinds = ContentKinds::default();
        for register in [
            ContentKinds::register::<ScatterDef>,
            ContentKinds::register::<LookDef>,
            ContentKinds::register::<TileDef>,
            ContentKinds::register::<RoomDef>,
            ContentKinds::register::<RegionDef>,
            ContentKinds::register::<DungeonDef>,
        ] {
            register(&mut kinds);
        }
        let file = |path: &str, text: &str| Source { path: format!("content/{path}"), text: text.to_owned() };
        let sources = [
            file("a.scatter.ron", scatter),
            file("a.look.ron", r##"{ "grass": (color: Some("#40a040")), "stone": (outline: Some((width: 1.0))) }"##),
            file(
                "a.tile.ron",
                r#"{ "field": (sockets: (n: "g", e: "g", s: "g", w: "g"), tags: ["meadow"]) }"#,
            ),
        ];
        ContentDb::build(&kinds, &sources).errors().filter(|e| e.kind.as_deref() == Some("scatter")).map(|e| e.message.clone()).collect()
    }

    const TUFT: &str = "(mesh: Blades(width: 0.1, height: 0.5, count: 5), until: 30.0)";

    #[test]
    fn a_scatter_reads_short_and_says_what_it_is() {
        let def: ScatterDef = ron::from_str(&format!(r#"(looks: ["grass"], lods: [{TUFT}], tiles: ["field"])"#)).unwrap();
        assert_eq!((def.density, def.height, def.max_slope, def.shadows), (0.5, (-0.5, 0.5), 30.0, false));
        assert_eq!(def.describe(), "0.5/m² · 1 LOD to 30 m · looks grass · on field");
        assert!(def.on_tile("field", &[]) && !def.on_tile("road", &[]) && def.in_region("anywhere"));
        let tagged = ScatterDef { tiles: vec!["#meadow".into()], ..def.clone() };
        assert!(tagged.on_tile("road", &["meadow".into()]) && !tagged.on_tile("field", &[]));
        let rock: ScatterMesh = ron::from_str("Rock(radius: 0.4, detail: 1)").unwrap();
        assert_eq!(rock, ScatterMesh::Rock { radius: 0.4, detail: 1, squash: 1.0 });
        assert_eq!(rock.describe(), "rock r 0.4 detail 1");
        assert!(ron::from_str::<ScatterDef>("(desnity: 1.0)").is_err(), "unknown fields are refused");
        assert!(ron::from_str::<ScatterMesh>("Cone(radius: 1.0, height: 1.0, sides: 5, tip: 1.0)").is_err());
    }

    #[test]
    fn good_content_has_no_errors() {
        let good = format!(
            r##"{{ "grass": (tiles: ["field", "#meadow"], looks: ["grass"], fade: 4.0, lods: [{TUFT}, (mesh: Rock(radius: 0.3, detail: 0), until: 60.0)]) }}"##
        );
        assert_eq!(errors(&good), Vec::<String>::new());
    }

    #[test]
    fn every_mistake_is_named() {
        let bad = r##"{ "bad": (
            tiles: ["feild", "#medow"], regions: ["nowhere"], density: 50.0, spacing: -1.0, scale: (2.0, 1.0),
            tilt: 90.0, max_slope: 100.0, height: (1.0, 0.0), looks: ["stone", "gras"], fade: 40.0,
            lods: [
                (mesh: Blades(width: 0.0, height: 0.5, count: 40), until: 30.0),
                (mesh: Cone(radius: 1.0, height: 1.0, sides: 2), until: 20.0),
                (mesh: Rock(radius: 1.0, detail: 5, squash: 0.0), until: 50.0),
                (mesh: Gltf("../bush.glb"), until: 60.0),
                (mesh: Box(size: (1.0, 0.0, 1.0)), until: 70.0),
            ],
        ) }"##;
        let errors = errors(bad);
        let expected = [
            "density: 0 to 20",
            "spacing: meters, 0 or more",
            "scale: (2, 1) is not two sizes",
            "tilt: 0 to 60 degrees",
            "max_slope: 0 to 90 degrees",
            "height: (1, 0) is not two heights",
            "looks[1] -> unknown look `gras` (did you mean `grass`?)",
            "lods: 1 to 4 levels of detail, not 5",
            "lods[0].mesh: width is a length above 0",
            "lods[0].mesh: a tuft has 1 to 16 blades, not 40",
            "lods[1].mesh: a cone has 3 to 32 sides, not 2",
            "lods[1].until: 20 must be farther than 30 m",
            "lods[2].mesh: a rock's detail is 0 to 3, not 5",
            "lods[2].mesh: a rock's squash is 0.1 to 2, not 0",
            "lods[3].mesh: Gltf `../bush.glb` names no mesh in the file",
            "lods[4].mesh: size.y is a length above 0",
            "fade: 40 m is not shorter than the shortest level of detail",
            "tiles[0]: no tile or room `feild` (did you mean `field`?)",
            "tiles[1]: no tile or room has tag `medow` (did you mean `#meadow`?)",
            "regions[0]: no region or dungeon `nowhere`",
        ];
        for expected in expected {
            assert!(errors.iter().any(|e| e.contains(expected)), "no error says `{expected}`: {errors:#?}");
        }
        assert_eq!(errors.len(), expected.len(), "{errors:#?}");
        let no_looks = self::errors("{ \"bare\": () }");
        assert!(no_looks.iter().any(|e| e.contains("looks is empty")) && no_looks.iter().any(|e| e.contains("lods: 1 to 4")), "{no_looks:#?}");
    }
}
