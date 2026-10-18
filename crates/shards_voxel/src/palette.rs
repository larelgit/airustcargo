//! The block palette: what each block id is. In the engine this becomes the content kind
//! `voxel_palette` (`content/**/*.voxel_palette.ron`); the spike keeps it as plain data.
//!
//! ```ron
//! {
//!     "default": (blocks: [
//!         (name: "grass", top: "#386f40", side: "#634a30", bottom: "#4f3b29"),
//!         (name: "stone", top: "#737885", side: "#696d7a", bottom: "#575c66"),
//!         (name: "leaves", top: "#367345", foliage: true, solid: false),
//!         (name: "water", top: "#3aa", liquid: true, solid: false),
//!         (name: "glow", top: "#ffa84d", emissive: true),
//!     ]),
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::volume::AIR;

/// One block kind. Its id is its place in the palette, from 1 (0 is air).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlockDef {
    pub name: String,
    /// Linear RGB of the top face, the sides and the bottom (sides and bottom default to the top).
    pub top: [f32; 3],
    pub side: Option<[f32; 3]>,
    pub bottom: Option<[f32; 3]>,
    /// Bodies stand on it and cannot pass (physics, navigation).
    pub solid: bool,
    /// Drawn in the foliage mesh: faces only toward air, no corner shading cast on others, and
    /// neighbours keep their faces toward it.
    pub foliage: bool,
    /// Water and the like: never solid, not meshed as a block (the water mesh draws it).
    pub liquid: bool,
    /// Lit by itself: no face shade, no corner darkening.
    pub emissive: bool,
    /// Free words for game rules (`flammable`, `ore`).
    pub tags: Vec<String>,
}

impl Default for BlockDef {
    fn default() -> Self {
        Self { name: String::new(), top: [1.0; 3], side: None, bottom: None, solid: true, foliage: false, liquid: false, emissive: false, tags: Vec::new() }
    }
}

impl BlockDef {
    pub fn new(name: &str, top: [f32; 3], side: [f32; 3], bottom: [f32; 3]) -> Self {
        Self { name: name.to_owned(), top, side: Some(side), bottom: Some(bottom), ..Self::default() }
    }

    pub fn foliage(mut self) -> Self {
        self.foliage = true;
        self.solid = false;
        self
    }

    pub fn emissive(mut self) -> Self {
        self.emissive = true;
        self
    }

    /// The colour of the face whose normal points along `dir` (0..6: +Y, -Y, +X, -X, +Z, -Z).
    pub fn face_color(&self, dir: usize) -> [f32; 3] {
        match dir {
            0 => self.top,
            1 => self.bottom.or(self.side).unwrap_or(self.top),
            _ => self.side.unwrap_or(self.top),
        }
    }
}

/// Per-block facts in flat tables, read by id: what the mesher and the collider builders ask
/// for every cell.
#[derive(Clone, Debug)]
pub struct VoxelPalette {
    pub blocks: Vec<BlockDef>,
    solid: [bool; 256],
    visible: [bool; 256],
    foliage: [bool; 256],
    /// Occludes corners (solid, drawn, not foliage, not emissive).
    occludes: [bool; 256],
}

impl VoxelPalette {
    /// `blocks[0]` is id 1.
    pub fn new(blocks: Vec<BlockDef>) -> Result<Self, String> {
        if blocks.len() > 255 {
            return Err(format!("{} blocks: a palette holds at most 255", blocks.len()));
        }
        let mut p = Self { blocks, solid: [false; 256], visible: [false; 256], foliage: [false; 256], occludes: [false; 256] };
        for (i, b) in p.blocks.iter().enumerate() {
            let id = i + 1;
            p.solid[id] = b.solid && !b.liquid;
            p.visible[id] = !b.liquid;
            p.foliage[id] = b.foliage;
            p.occludes[id] = !b.liquid && !b.foliage;
        }
        Ok(p)
    }

    pub fn def(&self, id: u8) -> Option<&BlockDef> {
        if id == AIR { None } else { self.blocks.get(id as usize - 1) }
    }

    /// The id of the block called `name`.
    pub fn id(&self, name: &str) -> Option<u8> {
        self.blocks.iter().position(|b| b.name == name).map(|i| i as u8 + 1)
    }

    pub fn solid(&self, id: u8) -> bool {
        self.solid[id as usize]
    }

    pub fn visible(&self, id: u8) -> bool {
        self.visible[id as usize]
    }

    pub fn is_foliage(&self, id: u8) -> bool {
        self.foliage[id as usize]
    }

    pub fn occludes(&self, id: u8) -> bool {
        self.occludes[id as usize]
    }
}

/// A neutral test palette: ground, rock, sand, snow, wood, leaves, brick and a glowing block.
pub fn test_palette() -> VoxelPalette {
    VoxelPalette::new(vec![
        BlockDef::new("grass", [0.22, 0.49, 0.25], [0.39, 0.29, 0.19], [0.31, 0.23, 0.16]),
        BlockDef::new("dirt", [0.46, 0.33, 0.20], [0.42, 0.30, 0.18], [0.34, 0.24, 0.15]),
        BlockDef::new("stone", [0.45, 0.47, 0.52], [0.41, 0.43, 0.48], [0.34, 0.36, 0.40]),
        BlockDef::new("sand", [0.89, 0.77, 0.46], [0.83, 0.71, 0.42], [0.68, 0.58, 0.34]),
        BlockDef::new("snow", [0.93, 0.94, 0.97], [0.84, 0.86, 0.92], [0.68, 0.70, 0.77]),
        BlockDef::new("wood", [0.40, 0.27, 0.16], [0.36, 0.24, 0.14], [0.29, 0.19, 0.11]),
        BlockDef::new("leaves", [0.21, 0.45, 0.27], [0.18, 0.39, 0.24], [0.16, 0.32, 0.22]).foliage(),
        BlockDef::new("brick", [0.46, 0.43, 0.40], [0.50, 0.44, 0.38], [0.36, 0.34, 0.32]),
        BlockDef::new("glow", [1.0, 0.66, 0.30], [1.0, 0.66, 0.30], [1.0, 0.66, 0.30]).emissive(),
        BlockDef { liquid: true, solid: false, ..BlockDef::new("water", [0.22, 0.68, 0.70], [0.22, 0.68, 0.70], [0.10, 0.34, 0.58]) },
    ])
    .expect("ten blocks fit")
}
