//! # shards_look — how meshes look (E10.1)
//!
//! A game's style is content: `content/**/*.look.ron` names looks ([`LookDef`]), and the engine
//! draws meshes with them. A look keeps the mesh's own colour (its material's, a glTF file's,
//! times its vertex colours) or gives one, and says how light falls on it:
//!
//! - **Toon** (the default): the sun's light, with its shadows, in 2 to 8 flat bands; ambient
//!   light, lamps and spots evenly; nothing shines. **Matte**: soft light, no shine. **Unlit**: the
//!   colour as it is. **Lit**: Bevy's physically based light with a roughness and metalness.
//! - **Flat** faces: low-poly facets on any mesh (every triangle its own normal).
//! - An **outline** of a few pixels around the silhouette, the same width near and far, so a
//!   character stays readable against the ground: the mesh again, inside out, pushed out along
//!   normals averaged at its corners (no gaps), bending with a skinned mesh.
//! - A **glow** of its own: lamps, eyes, crystals.
//!
//! A look can **extend** another, a preset (`low_poly`: flat with an outline; `cel`: two bands):
//! what it leaves out comes from that one, and from the one that extends in turn.
//!
//! Which meshes: a [`Look`] on an entity draws its meshes and its descendants' (a prefab carries
//! it: `"Look": ("stone")`; a mesh's own wins over one above it), and a mesh without one takes
//! the look that names its glTF material (`materials`, or a look called like the material), so
//! levels and tiles from Blender take the style with them. A mesh with neither keeps the game's
//! material. Edit a look and the meshes change as the file is saved (hot reload).
//!
//! - **Agents**: `looks` lists the looks and the meshes each draws, `look_outlines on=false`
//!   hides the outlines (the developer menu has both, under `world`); `ai/status` has
//!   `looks: {looks, styled, outlined, outlines}`; `inspect` shows an entity's `Look`.
//! - **The picture's, not the simulation's**: only meshes change (feature `render`), the same in a
//!   window and with `--offscreen`; headless nothing is drawn and nothing is styled. A `Look` on
//!   a body is a component like any other: replays check it, co-op clients get it with the body.
//! - **Checks**: `--check-content` finds colours that are not `#rrggbb` or `#rrggbbaa`, bands out
//!   of 2..8, outlines out of 0..16 px, a glTF material two looks claim, a look that extends one
//!   no file defines or extends itself round a circle, and prefabs that name a look no file
//!   defines.

pub mod def;
#[cfg(feature = "render")]
pub mod mesh;
#[cfg(feature = "render")]
pub mod render;
mod tools;

use ai_core::AiAppExt;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shards_content::{ContentAppExt, ContentDb, PrefabDef, Refs};
use shards_core::EngineAppExt;

pub use def::{material_looks, LookDef, OutlineDef, Shading};
#[cfg(feature = "render")]
pub use render::{Looked, OutlineHull, OutlineMaterial, Toon, ToonMaterial, Usage};
pub use tools::{LookOutlinesArgs, LooksArgs};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Draws an entity's meshes, and its descendants', with look `id` (`content/**/*.look.ron`). A
/// mesh's own `Look` wins over one above it. In a prefab: `"Look": ("stone")`.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct Look(pub String);

impl Look {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// What the looks draw besides materials: the outlines, on or off (`look_outlines`).
#[derive(Resource, Clone, Copy, Debug)]
pub struct LookSettings {
    pub outlines: bool,
}

impl Default for LookSettings {
    fn default() -> Self {
        Self { outlines: true }
    }
}

/// The look systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LookSystems {
    /// `PostUpdate`, before transforms propagate (and so before visibility and bounds) and before
    /// the asset events go out, while something is drawn: meshes get their looks' materials, flat
    /// copies and outlines; looks that changed are undone first.
    Apply,
}

/// The look module: the `look` content kind and its check of prefabs, the [`Look`] component,
/// the tools and the status; with feature `render`, the materials, shaders and systems that
/// draw meshes with looks. Needs the engine core and content first; `shards::EnginePlugins`
/// adds it after them.
pub struct LookPlugin;

impl Plugin for LookPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "LookPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<LookDef>()
            .add_content_check::<PrefabDef>("look", check_prefab)
            .register_type::<Look>()
            .register_ai_component::<Look>(|look| look.0.clone())
            .init_resource::<LookSettings>()
            .add_engine_module("look", VERSION)
            .add_ai_status("looks", status)
            .add_dev_entry(
                "world",
                "look_outlines",
                "Outlines of the looks",
                ai_core::DevCommand::Live(|w, _| Ok(ai_core::dev::toggle_line("look_outlines", w.resource::<LookSettings>().outlines))),
            )
            .add_dev_entry("world", "looks", "Looks and what they draw", ai_core::DevCommand::Run("looks".into()));
        tools::register(app.world_mut());
        #[cfg(feature = "render")]
        render::plugin(app);
    }
}

/// A prefab's `Look` names a look some file defines.
fn check_prefab(prefab: &PrefabDef, _id: &str, refs: &mut Refs) {
    for (name, value) in &prefab.components {
        let ours = match refs.type_registry() {
            Some(registry) => shards_content::prefab::resolve_component(registry, name)
                .is_ok_and(|reg| reg.type_id() == std::any::TypeId::of::<Look>()),
            None => name == "Look",
        };
        // A value that does not read is the prefab's own error.
        if ours && let Ok(look) = ron::from_str::<Look>(value.get_ron()) {
            refs.require::<LookDef>(&look.0, "Look");
        }
    }
}

/// Whether this game draws a picture: a window, or `--offscreen`.
pub fn draws(world: &World) -> bool {
    world.contains_resource::<ai_core::display::Offscreen>()
        || world.try_query_filtered::<(), With<bevy::window::Window>>().is_some_and(|mut q| q.iter(world).next().is_some())
}

/// What the looks style in `world` now (nothing without feature `render`).
#[cfg(feature = "render")]
pub fn usage(world: &World) -> Usage {
    render::usage(world)
}

/// What the looks style in `world` now (nothing without feature `render`).
#[cfg(not(feature = "render"))]
pub fn usage(_: &World) -> Usage {
    Usage::default()
}

/// Without feature `render` nothing is styled.
#[cfg(not(feature = "render"))]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    pub styled: std::collections::BTreeMap<String, usize>,
    pub outlined: usize,
    pub unstyled: usize,
    pub waiting: usize,
}

#[cfg(not(feature = "render"))]
impl Usage {
    pub fn total(&self) -> usize {
        self.styled.values().sum()
    }
}

/// `ai/status` → `looks: {looks, styled, outlined, outlines}`.
fn status(world: &World) -> Value {
    let usage = usage(world);
    json!({
        "looks": world.get_resource::<ContentDb>().map_or(0, |db| db.ids::<LookDef>().count()),
        "styled": usage.total(),
        "outlined": usage.outlined,
        "outlines": world.get_resource::<LookSettings>().is_none_or(|s| s.outlines),
    })
}
