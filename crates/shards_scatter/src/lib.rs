//! # shards_scatter — foliage and small props over the tiles (E10.2)
//!
//! Grass, bushes, rocks: content (`content/**/*.scatter.ron`, [`ScatterDef`]) says what is
//! strewn on which tiles of which regions, how densely, on which ground and with which looks;
//! the engine strews it over every tile a region builds, and takes it away with the tile.
//!
//! - **The same every run**: points thrown at random in the tile's square with the region's
//!   seed, the tile's cell and the scatter's id as keys ([`place`]), kept `spacing` apart, then
//!   dropped onto the ground under them: none where the ground is too steep (`max_slope`), out of
//!   the `height` band (a column's top), inside a solid or closer to one than `clearance` (a
//!   wall). The player's `graphics.foliage` (0 to 1) keeps the first part of the same list.
//! - **Drawn together**: the instances of a scatter share one mesh per level of detail and,
//!   through their look ([`shards_look`]), one material per look, so Bevy draws each (mesh,
//!   look) as one batch on the GPU. Each level of detail is an entity per instance with a
//!   `VisibilityRange` (`lods`, `fade`), so beyond `until` nothing of it is drawn, and Bevy's
//!   frustum culling skips what the camera does not see.
//! - **The picture's, not the simulation's** (feature `render`): placed only when the game draws,
//!   in a window or with `--offscreen`; headless nothing is strewn. No `AiId`, no collider, not in
//!   replays or snapshots: a tile's instances hang under the tile's root entity.
//! - **Agents**: `scatter` lists the scatters and what each has strewn; `scatter_set on=false`
//!   takes everything away, `on=true` strews it again (both in the developer menu, under
//!   `world`); `ai/status` has `scatter: {scatters, instances, entities, tiles, on}`; the
//!   density is `settings_set group=graphics key=foliage value=0.5`.
//! - **Checks**: `--check-content` finds tiles, tags, regions and looks no file defines, levels of
//!   detail out of order, a `fade` longer than a level, glTF meshes missing from the asset folder.

pub mod def;
#[cfg(feature = "render")]
pub mod mesh;
pub mod place;
#[cfg(feature = "render")]
mod render;
mod tools;

use std::collections::{BTreeMap, BTreeSet};

use ai_core::AiAppExt;
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_content::{ContentAppExt, ContentDb};
use shards_core::EngineAppExt;
use shards_world::TileId;

pub use def::{ScatterDef, ScatterLod, ScatterMesh};
pub use place::{Candidate, Ground, Placed, Skipped};
pub use tools::{ScatterArgs, ScatterSetArgs};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What is strewn, and how.
#[derive(Resource, Clone, Debug)]
pub struct ScatterState {
    /// Strewn at all (`scatter_set`).
    pub on: bool,
    /// The scatters as content had them when they were strewn, by id.
    pub defs: Vec<(String, ScatterDef)>,
    /// `graphics.foliage` when they were strewn.
    pub foliage: f32,
    /// Everything is taken away and strewn again on the next placement.
    pub redo: bool,
    /// Instances that found no place, by scatter, on the tiles strewn since the last redo.
    pub skipped: BTreeMap<String, Skipped>,
}

impl Default for ScatterState {
    fn default() -> Self {
        Self { on: true, defs: Vec::new(), foliage: 1.0, redo: false, skipped: BTreeMap::new() }
    }
}

/// On a tile's root: its scatters are strewn (even if none of them found a place there).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct ScatterTile;

/// The instances of one scatter with one look on one tile: a child of the tile's root, their
/// parent, carrying the `Look` they are drawn with.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct ScatterRoot {
    pub scatter: String,
    pub look: String,
    pub tile: TileId,
    /// Instances under it (each one entity per level of detail).
    pub instances: usize,
    /// Levels of detail of each instance.
    pub lods: usize,
}

/// One level of detail of one instance: a mesh entity under its [`ScatterRoot`].
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScatterInstance {
    /// 0 for the nearest.
    pub lod: u8,
}

/// The scatter systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScatterSystems {
    /// `Update`, when something is drawn: tiles that are new get their scatters; content, the
    /// player's `graphics.foliage` or `scatter_set` changing strews everything again.
    Place,
}

/// The scatter module: the `scatter` content kind and its checks, the tools, the status and the
/// developer menu's entries; with feature `render`, the meshes and the systems that strew them.
/// Needs the engine core, content, settings, the world and the looks first; `shards::EnginePlugins`
/// adds it after them.
pub struct ScatterPlugin;

impl Plugin for ScatterPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "ScatterPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<ScatterDef>()
            .init_resource::<ScatterState>()
            .add_engine_module("scatter", VERSION)
            .add_ai_status("scatter", status)
            .add_dev_entry(
                "world",
                "scatter_set",
                "Foliage and props (scatter)",
                ai_core::DevCommand::Live(|w, _| Ok(ai_core::dev::toggle_line("scatter_set", w.resource::<ScatterState>().on))),
            )
            .add_dev_entry("world", "scatter", "Scatters and what they strew", ai_core::DevCommand::Run("scatter".into()))
            .add_systems(Update, watch.in_set(ScatterSystems::Place));
        tools::register(app.world_mut());
        #[cfg(feature = "render")]
        render::plugin(app);
    }
}

/// Whether this game draws a picture: a window, or `--offscreen`.
pub fn draws(world: &World) -> bool {
    shards_look::draws(world)
}

/// Content's scatters and the player's foliage, against what was strewn: a change strews again.
fn watch(db: Res<ContentDb>, settings: Option<Res<shards_settings::GraphicsSettings>>, mut state: ResMut<ScatterState>) {
    if db.is_changed() {
        let defs: Vec<(String, ScatterDef)> = db.iter::<ScatterDef>().map(|(id, def)| (id.to_owned(), def.clone())).collect();
        if defs != state.defs {
            state.defs = defs;
            state.redo = true;
        }
    }
    let foliage = settings.map_or(1.0, |s| s.foliage_density());
    if foliage != state.foliage {
        state.foliage = foliage;
        state.redo = true;
    }
}

/// What is strewn in `world` now.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Strewn {
    /// Instances per scatter.
    pub instances: BTreeMap<String, usize>,
    /// Tiles with at least one instance, per scatter.
    pub tiles: BTreeMap<String, BTreeSet<(i32, i32)>>,
    /// Mesh entities: instances times their levels of detail.
    pub entities: usize,
}

impl Strewn {
    pub fn total(&self) -> usize {
        self.instances.values().sum()
    }

    /// Tiles with anything strewn on them.
    pub fn tiles(&self) -> usize {
        self.tiles.values().flatten().collect::<BTreeSet<_>>().len()
    }
}

/// What is strewn in `world` now.
pub fn strewn(world: &World) -> Strewn {
    let mut strewn = Strewn::default();
    if let Some(mut roots) = world.try_query::<&ScatterRoot>() {
        for root in roots.iter(world) {
            *strewn.instances.entry(root.scatter.clone()).or_default() += root.instances;
            strewn.tiles.entry(root.scatter.clone()).or_default().insert((root.tile.x, root.tile.y));
            strewn.entities += root.instances * root.lods;
        }
    }
    strewn
}

/// `ai/status` → `scatter: {scatters, instances, entities, tiles, on}`.
fn status(world: &World) -> Value {
    let strewn = strewn(world);
    json!({
        "scatters": world.get_resource::<ContentDb>().map_or(0, |db| db.ids::<ScatterDef>().count()),
        "instances": strewn.total(),
        "entities": strewn.entities,
        "tiles": strewn.tiles(),
        "on": world.get_resource::<ScatterState>().is_none_or(|s| s.on),
    })
}
