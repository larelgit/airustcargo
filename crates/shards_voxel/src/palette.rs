//! The block palette: what each block id is. It is the content kind `voxel_palette`
//! (`content/**/*.voxel_palette.ron`, [`VoxelPaletteDef`]), and [`VoxelPalette`] is its runtime
//! form, with flat tables read by id.
//!
//! ```ron
//! {
//!     "island": (
//!         blocks: [
//!             (name: "grass",  top: "#5b8f3a", side: "#6f5a3c", bottom: "#5c4730", tags: ["ground"]),
//!             (name: "dirt",   top: "#6f5a3c"),
//!             (name: "leaves", top: "#3f7f2f", solid: false, foliage: true),
//!             (name: "glow",   top: "#ffd27f", emissive: true),
//!             (name: "water",  top: "#2a6f9f", solid: false, liquid: true),
//!         ],
//!         shade: (up: 1.0, down: 0.5, x: 0.7, z: 0.85),
//!         ao: (both: 0.5, step: 0.15),
//!         look: Some("voxel_terrain"),
//!     ),
//! }
//! ```
//!
//! A block's id is its place in `blocks` plus one (0 is air); a palette holds at most 255. The
//! colours, the face shade, the corner darkening and the per-block variation are all content: no
//! number of a particular game is in the engine, and the defaults here are neutral. Meshing
//! (a later PR) reads them; the generation, the hash and the merge helpers read only the
//! solid, liquid and foliage flags and the names.

use serde::{Deserialize, Serialize};
use shards_content::{parse_color, ContentDb, ContentDef, Refs};
use shards_look::LookDef;

use crate::volume::AIR;

/// The most blocks a palette holds: ids are bytes, 0 is air.
pub const MAX_BLOCKS: usize = 255;

fn yes() -> bool {
    true
}

/// One block kind of a palette. Its id is its place in `blocks` plus one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockDef {
    /// Unique in the palette; steps and rules name blocks by it. Not `air`.
    pub name: String,
    /// Colour of the top face (`#rrggbb`, sRGB).
    pub top: String,
    /// Colour of the sides (default: the top's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// Colour of the bottom face (default: the sides').
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<String>,
    /// Bodies stand on it and cannot pass (physics, navigation). A `liquid` or `foliage` block is
    /// never solid: say `solid: false` with them.
    #[serde(default = "yes")]
    pub solid: bool,
    /// Drawn in the foliage mesh: faces only toward air, no corner darkening cast on others, and
    /// neighbours keep their faces toward it. Bodies and sight pass through it.
    #[serde(default)]
    pub foliage: bool,
    /// Water and the like: never solid, not meshed as a block (the water surface is drawn).
    #[serde(default)]
    pub liquid: bool,
    /// Lit by itself: no face shade, no corner darkening.
    #[serde(default)]
    pub emissive: bool,
    /// Free words for a game's rules (`ground`, `tree`, `ore`); see [`VoxelPalette::tagged`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl BlockDef {
    /// A solid block of one colour.
    pub fn solid(name: &str, color: &str) -> Self {
        Self { name: name.to_owned(), top: color.to_owned(), side: None, bottom: None, solid: true, foliage: false, liquid: false, emissive: false, tags: Vec::new() }
    }
}

/// The brightness factor of each face direction, `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShadeDef {
    pub up: f32,
    pub down: f32,
    /// The two faces across x.
    pub x: f32,
    /// The two faces across z.
    pub z: f32,
}

impl Default for ShadeDef {
    fn default() -> Self {
        Self { up: 1.0, down: 0.5, x: 0.75, z: 0.85 }
    }
}

/// Corner darkening: a corner with both side neighbours filled gets `both`, otherwise
/// `1 - step * (side1 + side2 + corner)`; both `0..=1`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AoDef {
    pub both: f32,
    pub step: f32,
}

impl Default for AoDef {
    fn default() -> Self {
        Self { both: 0.5, step: 0.2 }
    }
}

/// Per-block colour variation, `0..=1` of the colour: `block` for every cell by itself, `patch`
/// for patches of neighbouring cells. Zero (the default) is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VariationDef {
    pub block: f32,
    pub patch: f32,
}

/// How a chunk is turned into a mesh (read by meshing, a later PR).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Meshing {
    /// One quad per visible face: corner darkening is exact.
    #[default]
    Faces,
    /// Equal faces merged into larger quads: fewer triangles, darkening only where it is even.
    Greedy,
}

/// A palette in content (`*.voxel_palette.ron`): the blocks of a volume and how they are drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoxelPaletteDef {
    pub blocks: Vec<BlockDef>,
    #[serde(default)]
    pub shade: ShadeDef,
    #[serde(default)]
    pub ao: AoDef,
    #[serde(default)]
    pub variation: VariationDef,
    #[serde(default)]
    pub meshing: Meshing,
    /// The look (`*.look.ron`) of the terrain mesh, of the foliage mesh and of the water surface.
    /// Unset: the meshes use the engine's default material.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foliage_look: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_look: Option<String>,
}

impl VoxelPaletteDef {
    /// A palette of `blocks` and neutral defaults for the rest.
    pub fn new(blocks: Vec<BlockDef>) -> Self {
        Self {
            blocks,
            shade: ShadeDef::default(),
            ao: AoDef::default(),
            variation: VariationDef::default(),
            meshing: Meshing::default(),
            look: None,
            foliage_look: None,
            water_look: None,
        }
    }

    /// Everything wrong with the palette itself (not its references to looks), one line each.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.blocks.is_empty() {
            out.push("blocks: a palette needs at least one block".to_owned());
        }
        if self.blocks.len() > MAX_BLOCKS {
            out.push(format!("blocks: {} blocks, a palette holds at most {MAX_BLOCKS} (ids are bytes, 0 is air)", self.blocks.len()));
        }
        for (i, block) in self.blocks.iter().enumerate() {
            let name = &block.name;
            let at = format!("blocks[{i}] `{name}`");
            if name.is_empty() {
                out.push(format!("blocks[{i}]: a block needs a name"));
            } else if name == "air" {
                out.push(format!("{at}: `air` is the empty cell (id 0), not a block"));
            } else if self.blocks[..i].iter().any(|b| b.name == *name) {
                out.push(format!("{at}: the name is used by an earlier block"));
            }
            for (face, color) in [("top", Some(&block.top)), ("side", block.side.as_ref()), ("bottom", block.bottom.as_ref())] {
                if let Some(color) = color
                    && let Err(e) = parse_color(color)
                {
                    out.push(format!("{at}: {face}: {e}"));
                }
            }
            if block.liquid && block.solid {
                out.push(format!("{at}: a liquid is never solid: add `solid: false`"));
            }
            if block.foliage && block.solid {
                out.push(format!("{at}: foliage is never solid (bodies pass through leaves): add `solid: false`"));
            }
            if block.liquid && block.foliage {
                out.push(format!("{at}: a block is a liquid or foliage, not both"));
            }
        }
        for (what, value) in [
            ("shade.up", self.shade.up),
            ("shade.down", self.shade.down),
            ("shade.x", self.shade.x),
            ("shade.z", self.shade.z),
            ("ao.both", self.ao.both),
            ("ao.step", self.ao.step),
            ("variation.block", self.variation.block),
            ("variation.patch", self.variation.patch),
        ] {
            if !(value.is_finite() && (0.0..=1.0).contains(&value)) {
                out.push(format!("{what}: 0 to 1, not {value}"));
            }
        }
        out
    }
}

impl ContentDef for VoxelPaletteDef {
    const KIND: &'static str = "voxel_palette";

    fn validate(&self, _: &str, refs: &mut Refs) {
        for problem in self.problems() {
            refs.error(problem);
        }
        for (field, look) in [("look", &self.look), ("foliage_look", &self.foliage_look), ("water_look", &self.water_look)] {
            if let Some(look) = look {
                refs.require::<LookDef>(look, field);
            }
        }
    }
}

/// A palette ready to be read by id: the flat tables generation, the merge helpers and meshing
/// ask for every cell. Built from a [`VoxelPaletteDef`] ([`from_def`](Self::from_def)); it does
/// not change afterwards.
#[derive(Clone, Debug)]
pub struct VoxelPalette {
    def: VoxelPaletteDef,
    solid: [bool; 256],
    liquid: [bool; 256],
    foliage: [bool; 256],
    emissive: [bool; 256],
    /// Per id: top, side, bottom, sRGB bytes.
    colors: Vec<[[u8; 3]; 3]>,
}

impl VoxelPalette {
    /// The palette `def` describes; the error lists what [`VoxelPaletteDef::problems`] found.
    pub fn from_def(def: &VoxelPaletteDef) -> Result<Self, String> {
        let problems = def.problems();
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        let mut p = Self {
            def: def.clone(),
            solid: [false; 256],
            liquid: [false; 256],
            foliage: [false; 256],
            emissive: [false; 256],
            colors: vec![[[0; 3]; 3]; 256],
        };
        for (i, b) in def.blocks.iter().enumerate() {
            let id = i + 1;
            let rgb = |text: &str| {
                let c = parse_color(text).expect("problems() parsed every colour");
                [c[0], c[1], c[2]]
            };
            let top = rgb(&b.top);
            let side = b.side.as_deref().map_or(top, rgb);
            let bottom = b.bottom.as_deref().map_or(side, rgb);
            p.colors[id] = [top, side, bottom];
            p.solid[id] = b.solid;
            p.liquid[id] = b.liquid;
            p.foliage[id] = b.foliage;
            p.emissive[id] = b.emissive;
        }
        Ok(p)
    }

    /// The palette content has under `id`.
    pub fn from_db(db: &ContentDb, id: &str) -> Result<Self, String> {
        let def = db.get::<VoxelPaletteDef>(id).ok_or_else(|| format!("no voxel palette `{id}` in content"))?;
        Self::from_def(def).map_err(|e| format!("voxel palette `{id}`: {e}"))
    }

    /// The content form.
    pub fn def(&self) -> &VoxelPaletteDef {
        &self.def
    }

    /// How many blocks (ids `1..=len`).
    pub fn len(&self) -> usize {
        self.def.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.def.blocks.is_empty()
    }

    /// The block under `id`; none for air and for an id past the palette.
    pub fn block(&self, id: u8) -> Option<&BlockDef> {
        if id == AIR { None } else { self.def.blocks.get(id as usize - 1) }
    }

    /// The id of the block called `name`.
    pub fn id(&self, name: &str) -> Option<u8> {
        self.def.blocks.iter().position(|b| b.name == name).map(|i| i as u8 + 1)
    }

    /// The name of the block under `id`.
    pub fn name(&self, id: u8) -> Option<&str> {
        self.block(id).map(|b| b.name.as_str())
    }

    /// Bodies stand on it and cannot pass: not air, not foliage, not liquid.
    pub fn solid(&self, id: u8) -> bool {
        self.solid[id as usize]
    }

    pub fn liquid(&self, id: u8) -> bool {
        self.liquid[id as usize]
    }

    pub fn foliage(&self, id: u8) -> bool {
        self.foliage[id as usize]
    }

    pub fn emissive(&self, id: u8) -> bool {
        self.emissive[id as usize]
    }

    /// Drawn as a block (not air, not liquid).
    pub fn visible(&self, id: u8) -> bool {
        id != AIR && !self.liquid[id as usize] && (id as usize) <= self.def.blocks.len()
    }

    /// Casts corner darkening on its neighbours: visible, not foliage, not emissive.
    pub fn occludes(&self, id: u8) -> bool {
        self.visible(id) && !self.foliage[id as usize] && !self.emissive[id as usize]
    }

    /// The ids whose block carries `tag`, in id order.
    pub fn tagged(&self, tag: &str) -> Vec<u8> {
        self.def.blocks.iter().enumerate().filter(|(_, b)| b.tags.iter().any(|t| t == tag)).map(|(i, _)| i as u8 + 1).collect()
    }

    /// sRGB bytes of the face of block `id` whose normal points along `dir` (0..6: +Y, -Y, +X, -X,
    /// +Z, -Z); black for air and for an id past the palette.
    pub fn face_color(&self, id: u8, dir: usize) -> [u8; 3] {
        let [top, side, bottom] = self.colors[id as usize];
        match dir {
            0 => top,
            1 => bottom,
            _ => side,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Content files read RON with implicit `Some`, so `side: "#fff"` works.
    fn parse(text: &str) -> VoxelPaletteDef {
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).unwrap()
    }

    #[test]
    fn a_palette_reads_short_and_gets_neutral_defaults() {
        let def = parse(
            r##"(blocks: [
                (name: "grass", top: "#5b8f3a", side: "#6f5a3c", bottom: "#5c4730", tags: ["ground"]),
                (name: "dirt", top: "#6f5a3c"),
                (name: "leaves", top: "#3f7f2f", solid: false, foliage: true),
                (name: "glow", top: "#ffd27f", emissive: true),
                (name: "water", top: "#2a6f9f", solid: false, liquid: true),
            ])"##,
        );
        assert!(def.problems().is_empty(), "{:?}", def.problems());
        assert_eq!((def.shade, def.ao, def.variation, def.meshing), (ShadeDef::default(), AoDef::default(), VariationDef::default(), Meshing::Faces));
        let p = VoxelPalette::from_def(&def).unwrap();
        let (grass, dirt, leaves, glow, water) = (p.id("grass").unwrap(), p.id("dirt").unwrap(), p.id("leaves").unwrap(), p.id("glow").unwrap(), p.id("water").unwrap());
        assert_eq!([grass, dirt, leaves, glow, water], [1, 2, 3, 4, 5]);
        assert_eq!(p.id("air"), None);
        assert_eq!(p.name(leaves), Some("leaves"));
        assert_eq!(p.block(AIR).map(|b| b.name.as_str()), None);
        assert_eq!(p.block(200).map(|b| b.name.as_str()), None);
        // Solid is not air, foliage or liquid.
        assert_eq!([AIR, grass, dirt, leaves, glow, water].map(|id| p.solid(id)), [false, true, true, false, true, false]);
        assert_eq!([AIR, grass, leaves, water].map(|id| p.visible(id)), [false, true, true, false]);
        assert_eq!([grass, leaves, glow, water].map(|id| p.occludes(id)), [true, false, false, false]);
        assert!(p.foliage(leaves) && p.liquid(water) && p.emissive(glow) && !p.liquid(grass));
        assert_eq!(p.tagged("ground"), vec![grass]);
        assert!(p.tagged("nothing").is_empty());
        // Colours: sides default to the top, the bottom to the sides.
        assert_eq!(p.face_color(grass, 0), [0x5b, 0x8f, 0x3a]);
        assert_eq!(p.face_color(grass, 2), [0x6f, 0x5a, 0x3c]);
        assert_eq!(p.face_color(grass, 1), [0x5c, 0x47, 0x30]);
        assert_eq!([0, 1, 4].map(|d| p.face_color(dirt, d)), [[0x6f, 0x5a, 0x3c]; 3]);
        assert_eq!(p.face_color(AIR, 0), [0; 3]);
    }

    #[test]
    fn a_palette_round_trips_through_json_as_content_does() {
        let def = parse(r##"(blocks: [(name: "a", top: "#fff", side: "#000")], look: "x", meshing: Greedy, ao: (both: 0.3))"##);
        let json = serde_json::to_value(&def).unwrap();
        assert_eq!(serde_json::from_value::<VoxelPaletteDef>(json).unwrap(), def);
        assert_eq!((def.ao.both, def.ao.step, def.meshing), (0.3, 0.2, Meshing::Greedy));
    }

    #[test]
    fn what_is_wrong_is_named_per_block() {
        let def = parse(
            r##"(blocks: [
                (name: "a", top: "#zzz"),
                (name: "a", top: "#fff", side: "red"),
                (name: "air", top: "#fff"),
                (name: "", top: "#fff"),
                (name: "w", top: "#fff", liquid: true),
                (name: "f", top: "#fff", foliage: true),
                (name: "both", top: "#fff", solid: false, liquid: true, foliage: true),
            ], shade: (up: 1.5), ao: (step: -0.1), variation: (patch: 2.0))"##,
        );
        let problems = def.problems();
        let has = |needle: &str| assert!(problems.iter().any(|p| p.contains(needle)), "{needle}: {problems:#?}");
        has("blocks[0] `a`: top: `#zzz` is not a colour");
        has("blocks[1] `a`: the name is used by an earlier block");
        has("blocks[1] `a`: side: `red` is not a colour");
        has("blocks[2] `air`: `air` is the empty cell");
        has("blocks[3]: a block needs a name");
        has("blocks[4] `w`: a liquid is never solid");
        has("blocks[5] `f`: foliage is never solid");
        has("blocks[6] `both`: a block is a liquid or foliage, not both");
        has("shade.up: 0 to 1, not 1.5");
        has("ao.step: 0 to 1, not -0.1");
        has("variation.patch: 0 to 1, not 2");
        assert!(VoxelPalette::from_def(&def).unwrap_err().contains("blocks[0]"));
        let nan = VoxelPaletteDef { shade: ShadeDef { x: f32::NAN, ..ShadeDef::default() }, ..VoxelPaletteDef::new(vec![BlockDef::solid("s", "#888")]) };
        assert!(nan.problems()[0].starts_with("shade.x"), "{:?}", nan.problems());
    }

    #[test]
    fn a_palette_holds_at_most_255_blocks_and_at_least_one() {
        let many = |n: usize| VoxelPaletteDef::new((0..n).map(|i| BlockDef::solid(&format!("b{i}"), "#888")).collect());
        assert!(many(255).problems().is_empty());
        assert!(VoxelPalette::from_def(&many(255)).is_ok());
        let over = many(256).problems();
        assert!(over.len() == 1 && over[0].contains("at most 255"), "{over:?}");
        assert_eq!(many(0).problems(), vec!["blocks: a palette needs at least one block".to_owned()]);
        let p = VoxelPalette::from_def(&many(255)).unwrap();
        assert_eq!((p.len(), p.id("b254"), p.solid(255), p.visible(255)), (255, Some(255), true, true));
    }

    #[test]
    fn unknown_fields_are_errors() {
        assert!(ron::from_str::<VoxelPaletteDef>(r##"(blocks: [], shadow: ())"##).is_err());
        assert!(ron::from_str::<VoxelPaletteDef>(r##"(blocks: [(name: "a", top: "#fff", colour: "x")])"##).is_err());
    }
}
