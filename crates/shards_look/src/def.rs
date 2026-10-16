//! The `look` content kind: how a surface is drawn. A look names a colour (or keeps the mesh's
//! own), how light falls on it, flat faces, an outline and a glow; the glTF materials it stands
//! for come with it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use shards_content::{parse_color, ContentDef, Refs};

/// The most light bands a toon look has; more look smooth again.
pub const MAX_BANDS: u32 = 8;
/// The widest outline, in pixels.
pub const MAX_OUTLINE: f32 = 16.0;

/// How a surface is drawn (`content/**/*.look.ron`): a mesh takes it from a [`Look`](crate::Look)
/// on itself or an entity above it, or from the name of its glTF material.
///
/// ```ron
/// {
///     "stone": (color: Some("#6b6f78"), shading: Toon(bands: 3), outline: Some((width: 1.5))),
///     "crystal": (shading: Unlit, glow: Some("#7fe0ff"), materials: ["Crystal", "Crystal.001"]),
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LookDef {
    /// The surface's colour, `#rrggbb`, times the mesh's texture and vertex colours. Left out, the
    /// mesh keeps its own: the colour its material has (a game's, a glTF file's).
    pub color: Option<String>,
    /// How light falls on it.
    pub shading: Shading,
    /// Faces drawn flat, one normal per triangle: the facets of low-poly art, on any mesh.
    pub flat: bool,
    /// A line of a few pixels around the silhouette, the same width near and far.
    pub outline: Option<OutlineDef>,
    /// Light of its own, `#rrggbb` (lamps, eyes, crystals): shown whatever the light around.
    pub glow: Option<String>,
    /// The glTF materials (as Blender names them) drawn with this look, besides the material
    /// named like the look itself.
    pub materials: Vec<String>,
}

impl Default for LookDef {
    fn default() -> Self {
        Self { color: None, shading: Shading::default(), flat: false, outline: None, glow: None, materials: Vec::new() }
    }
}

/// How light falls on a surface.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Shading {
    /// The sun's light (every directional light, with its shadows) in `bands` flat steps, from
    /// unlit to fully lit: 2 is the classic two-tone cel look. Ambient light, lamps and spots
    /// light it evenly, without bands; nothing shines.
    Toon {
        #[serde(default = "three")]
        bands: u32,
    },
    /// Soft light without shine: fully rough, no specular highlight.
    Matte,
    /// No light: the colour as it is, the same in the sun and in the dark.
    Unlit,
    /// Bevy's physically based light with this roughness and metalness, each 0 to 1.
    Lit { roughness: f32, metallic: f32 },
}

fn three() -> u32 {
    3
}

impl Default for Shading {
    fn default() -> Self {
        Shading::Toon { bands: three() }
    }
}

impl Shading {
    /// `toon 3 bands`, `matte`, `unlit`, `lit roughness 0.4 metallic 1`.
    pub fn describe(&self) -> String {
        match self {
            Shading::Toon { bands } => format!("toon {bands} bands"),
            Shading::Matte => "matte".into(),
            Shading::Unlit => "unlit".into(),
            Shading::Lit { roughness, metallic } => format!("lit roughness {roughness} metallic {metallic}"),
        }
    }
}

/// An outline around the silhouette.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutlineDef {
    /// In pixels of the picture: the same near and far.
    pub width: f32,
    /// `#rrggbb`.
    pub color: String,
}

impl Default for OutlineDef {
    fn default() -> Self {
        Self { width: 2.0, color: "#000000".into() }
    }
}

impl LookDef {
    /// `toon 3 bands · flat · outline 2px #000000 · glow #ffd933 · materials Rock, Rock.001`.
    pub fn describe(&self) -> String {
        let mut parts = vec![self.shading.describe()];
        if let Some(color) = &self.color {
            parts.insert(0, color.clone());
        }
        if self.flat {
            parts.push("flat".into());
        }
        if let Some(outline) = &self.outline {
            parts.push(format!("outline {}px {}", outline.width, outline.color));
        }
        if let Some(glow) = &self.glow {
            parts.push(format!("glow {glow}"));
        }
        if !self.materials.is_empty() {
            parts.push(format!("materials {}", self.materials.join(", ")));
        }
        parts.join(" · ")
    }
}

impl ContentDef for LookDef {
    const KIND: &'static str = "look";

    fn validate(&self, id: &str, refs: &mut Refs) {
        for (field, color) in [("color", &self.color), ("glow", &self.glow)] {
            if let Some(color) = color
                && let Err(e) = parse_color(color)
            {
                refs.error(format!("{field}: {e}"));
            }
        }
        match self.shading {
            Shading::Toon { bands } if !(2..=MAX_BANDS).contains(&bands) => {
                refs.error(format!("shading: Toon has 2 to {MAX_BANDS} bands, not {bands}"));
            }
            Shading::Lit { roughness, metallic } => {
                for (field, value) in [("roughness", roughness), ("metallic", metallic)] {
                    if !(0.0..=1.0).contains(&value) {
                        refs.error(format!("shading: Lit {field} is 0 to 1, not {value}"));
                    }
                }
            }
            _ => {}
        }
        if let Some(outline) = &self.outline {
            if !(outline.width > 0.0 && outline.width <= MAX_OUTLINE) {
                refs.error(format!("outline: width is above 0 and at most {MAX_OUTLINE} pixels, not {}", outline.width));
            }
            if let Err(e) = parse_color(&outline.color) {
                refs.error(format!("outline: color: {e}"));
            }
        }
        for material in &self.materials {
            if material.trim().is_empty() {
                refs.error("materials: an empty name; give a glTF material's name, as Blender shows it");
            } else if material != id && refs.get::<LookDef>(material).is_some() {
                refs.error(format!("materials: `{material}` is the name of look `{material}`, which draws that material already"));
            } else if let Some((other, _)) = refs.iter::<LookDef>().find(|(other, def)| *other < id && def.materials.contains(material)) {
                refs.error(format!("materials: `{material}` is drawn with look `{other}` already; a material has one look"));
            }
        }
    }
}

/// glTF material name → the look that draws it: each look draws the material of its own name and
/// those its `materials` names (the content check keeps a name to one look).
pub fn material_looks<'a>(looks: impl IntoIterator<Item = (&'a str, &'a LookDef)>) -> HashMap<String, String> {
    let mut by_material = HashMap::new();
    for (id, def) in looks {
        for material in &def.materials {
            by_material.entry(material.clone()).or_insert_with(|| id.to_owned());
        }
        // Its own name first, whatever another look lists.
        by_material.insert(id.to_owned(), id.to_owned());
    }
    by_material
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_reads_short_and_says_what_it_is() {
        let look: LookDef = ron::from_str(r##"(color: Some("#6b6f78"), outline: Some((width: 1.5)), flat: true)"##).unwrap();
        assert_eq!(look.shading, Shading::Toon { bands: 3 }, "toon in three bands unless it says");
        assert_eq!(look.outline, Some(OutlineDef { width: 1.5, color: "#000000".into() }));
        assert_eq!(look.describe(), "#6b6f78 · toon 3 bands · flat · outline 1.5px #000000");
        let lit: LookDef = ron::from_str("(shading: Lit(roughness: 0.4, metallic: 1.0), materials: [\"Metal\"])").unwrap();
        assert_eq!(lit.describe(), "lit roughness 0.4 metallic 1 · materials Metal");
        let two: LookDef = ron::from_str("(shading: Toon(bands: 2))").unwrap();
        assert_eq!(two.shading, Shading::Toon { bands: 2 });
        assert!(ron::from_str::<LookDef>("(colour: Some(\"#fff\"))").is_err(), "unknown fields are refused");
    }

    #[test]
    fn a_material_is_drawn_by_the_look_of_its_name_then_by_the_one_that_names_it() {
        let stone = LookDef { materials: vec!["Rock".into(), "Ruin".into()], ..LookDef::default() };
        let ruin = LookDef::default();
        let by_material = material_looks([("ruin", &ruin), ("stone", &stone)]);
        let look = |material: &str| by_material.get(material).map(String::as_str);
        assert_eq!(look("Rock"), Some("stone"));
        assert_eq!((look("ruin"), look("stone")), (Some("ruin"), Some("stone")), "their own names");
        assert_eq!(look("Ruin"), Some("stone"), "names are matched as they are written");
        assert_eq!(look("Grass"), None);
        let claims_ruin = LookDef { materials: vec!["ruin".into()], ..LookDef::default() };
        let by_material = material_looks([("a", &claims_ruin), ("ruin", &ruin)]);
        assert_eq!(by_material["ruin"], "ruin", "a look's own name wins over another's list");
    }
}
