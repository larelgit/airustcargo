//! The `look` content kind: how a surface is drawn. A look names a colour (or keeps the mesh's
//! own), how light falls on it, flat faces, an outline and a glow; the glTF materials it stands
//! for come with it. A look can extend another (a preset such as `low_poly`): what it leaves out
//! comes from that one.

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
///     "low_poly": (flat: true, outline: Some((width: 2.0, color: "#0b0f1a"))),
///     "stone": (color: Some("#6b6f78"), shading: Toon(bands: 3), outline: Some((width: 1.5))),
///     "hero": (extends: "low_poly", color: Some("#408cff")),
///     "crystal": (shading: Unlit, glow: Some("#7fe0ff"), materials: ["Crystal", "Crystal.001"]),
/// }
/// ```
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LookDef {
    /// The look this one starts from (a preset): every field this one leaves out comes from it,
    /// and from the look it extends in turn; `materials` excepted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// The surface's colour, times the mesh's texture and vertex colours: `#rrggbb` keeps the
    /// alpha of the mesh's material (glass stays see-through), `#rrggbbaa` gives one. Left out,
    /// the mesh keeps its own: the colour its material has (a game's, a glTF file's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// How light falls on it: `Toon(bands: 3)` unless this look or one it extends says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shading: Option<Shading>,
    /// Faces drawn flat, one normal per triangle: the facets of low-poly art, on any mesh. Off
    /// unless this look or one it extends says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flat: Option<bool>,
    /// A line of a few pixels around the silhouette, the same width near and far. A mesh whose
    /// material is see-through (any alpha mode but `Opaque`) gets none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline: Option<OutlineDef>,
    /// Light of its own, `#rrggbb` (lamps, eyes, crystals): shown whatever the light around.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glow: Option<String>,
    /// The glTF materials (as Blender names them) drawn with this look, besides the material
    /// named like the look itself. Not taken from the look it extends: a material has one look.
    pub materials: Vec<String>,
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
    /// The look as drawn: each field it leaves out taken from the look it extends (the nearest
    /// in the chain that sets it), then the defaults (toon in 3 bands, not flat). `extends` and
    /// `materials` stay its own. A chain that names a look `looks` does not have, or goes round
    /// in a circle, stops there (the content check names it).
    pub fn resolve<'a>(&self, looks: impl Fn(&str) -> Option<&'a LookDef>) -> LookDef {
        let mut look = self.clone();
        let mut seen: Vec<String> = Vec::new();
        let mut next = self.extends.clone();
        while let Some(id) = next.take() {
            if seen.contains(&id) {
                break;
            }
            let Some(base) = looks(&id) else { break };
            seen.push(id);
            look.color = look.color.or_else(|| base.color.clone());
            look.shading = look.shading.or(base.shading);
            look.flat = look.flat.or(base.flat);
            look.outline = look.outline.or_else(|| base.outline.clone());
            look.glow = look.glow.or_else(|| base.glow.clone());
            next = base.extends.clone();
        }
        look.shading.get_or_insert_default();
        look.flat.get_or_insert(false);
        look
    }

    /// `#61636e · toon 3 bands · flat · outline 2px #000000 · glow #ffd933 · materials Rock,
    /// Rock.001 · extends low_poly`: the fields it sets, with the defaults for those it leaves
    /// out. [`resolve`](Self::resolve) it first to say how it is drawn.
    pub fn describe(&self) -> String {
        let mut parts = vec![self.shading.unwrap_or_default().describe()];
        if let Some(color) = &self.color {
            parts.insert(0, color.clone());
        }
        if self.flat == Some(true) {
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
        if let Some(base) = &self.extends {
            parts.push(format!("extends {base}"));
        }
        parts.join(" · ")
    }
}

impl ContentDef for LookDef {
    const KIND: &'static str = "look";
    // Only what a machine shows or plays, or what lies outside a session: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, id: &str, refs: &mut Refs) {
        if let Some(base) = &self.extends
            && refs.require::<LookDef>(base, "extends")
        {
            let mut chain = vec![id.to_owned()];
            let mut next = Some(base.clone());
            while let Some(name) = next {
                if chain.contains(&name) {
                    chain.push(name);
                    refs.error(format!("extends goes round in a circle: {}", chain.join(" -> ")));
                    break;
                }
                next = refs.get::<LookDef>(&name).and_then(|look| look.extends.clone());
                chain.push(name);
            }
        }
        // Each value is checked where it is set: a look that extends a broken one is not broken too.
        for (field, color) in [("color", &self.color), ("glow", &self.glow)] {
            if let Some(color) = color
                && let Err(e) = parse_color(color)
            {
                refs.error(format!("{field}: {e}"));
            }
        }
        match self.shading {
            Some(Shading::Toon { bands }) if !(2..=MAX_BANDS).contains(&bands) => {
                refs.error(format!("shading: Toon has 2 to {MAX_BANDS} bands, not {bands}"));
            }
            Some(Shading::Lit { roughness, metallic }) => {
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

    /// A look as content reads it (`Some` may be left out).
    fn read(text: &str) -> LookDef {
        options().from_str(text).unwrap()
    }

    fn options() -> ron::Options {
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
    }

    #[test]
    fn a_look_reads_short_and_says_what_it_is() {
        let look = read(r##"(color: Some("#6b6f78"), outline: Some((width: 1.5)), flat: true)"##);
        assert_eq!(look.shading, None);
        assert_eq!(look.resolve(|_| None).shading, Some(Shading::Toon { bands: 3 }), "toon in three bands unless it says");
        assert_eq!(look.outline, Some(OutlineDef { width: 1.5, color: "#000000".into() }));
        assert_eq!(look.describe(), "#6b6f78 · toon 3 bands · flat · outline 1.5px #000000");
        let lit = read("(shading: Lit(roughness: 0.4, metallic: 1.0), materials: [\"Metal\"])");
        assert_eq!(lit.describe(), "lit roughness 0.4 metallic 1 · materials Metal");
        assert_eq!(read("(shading: Toon(bands: 2))").shading, Some(Shading::Toon { bands: 2 }));
        assert_eq!(read("(shading: Some(Matte), flat: Some(false))").flat, Some(false), "`Some` may be written too");
        assert!(options().from_str::<LookDef>("(colour: Some(\"#fff\"))").is_err(), "unknown fields are refused");
    }

    #[test]
    fn a_look_takes_what_it_leaves_out_from_the_looks_it_extends() {
        let looks: HashMap<&str, LookDef> = HashMap::from([
            ("low_poly", read(r##"(flat: true, outline: Some((width: 2.0, color: "#0b0f1a")), glow: Some("#ffffff"), materials: ["Rock"])"##)),
            ("hero", read(r##"(extends: "low_poly", color: Some("#408cff"), outline: Some((width: 3.0, color: "#0b1a33")))"##)),
            ("captain", read(r#"(extends: "hero", shading: Matte, flat: false)"#)),
        ]);
        let find = |id: &str| looks.get(id);
        // One step: its own colour and outline, the preset's flat faces and glow; never its materials.
        let hero = looks["hero"].resolve(find);
        let outline = OutlineDef { width: 3.0, color: "#0b1a33".into() };
        assert_eq!((hero.flat, hero.outline.as_ref(), hero.glow.as_deref()), (Some(true), Some(&outline), Some("#ffffff")));
        assert_eq!((hero.shading, hero.materials.len(), hero.extends.as_deref()), (Some(Shading::Toon { bands: 3 }), 0, Some("low_poly")));
        assert_eq!(hero.describe(), "#408cff · toon 3 bands · flat · outline 3px #0b1a33 · glow #ffffff · extends low_poly");
        // Two steps, and what it sets wins, `false` too.
        let captain = looks["captain"].resolve(find);
        assert_eq!((captain.color.as_deref(), captain.shading, captain.flat), (Some("#408cff"), Some(Shading::Matte), Some(false)));
        assert_eq!(captain.outline, Some(outline));
        assert_eq!(captain.describe(), "#408cff · matte · outline 3px #0b1a33 · glow #ffffff · extends hero");
        // The defaults where nothing in the chain says, and a look it extends that is not there.
        let alone = read(r#"(extends: "nowhere")"#).resolve(find);
        assert_eq!((alone.shading, alone.flat, alone.outline), (Some(Shading::default()), Some(false), None));
    }

    #[test]
    fn a_circle_of_looks_ends_where_it_comes_round() {
        let looks: HashMap<&str, LookDef> =
            HashMap::from([("a", read(r##"(extends: "b", color: Some("#ff0000"))"##)), ("b", read(r#"(extends: "a", flat: true)"#))]);
        let a = looks["a"].resolve(|id| looks.get(id));
        assert_eq!((a.color.as_deref(), a.flat), (Some("#ff0000"), Some(true)));
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
