//! # shards_world — regions: generation, manifests, loading, active zones, streaming
//!
//! A region is a grid of macro tiles with sockets on their edges (or a dungeon of rooms
//! joined door to door), made by a generator, checked, kept as a manifest and built in the
//! world from it.
//!
//! - [`content`]: the content kinds — `tile`, `socket_rules`, `region`, `poi`, `tile_map`
//!   (hand-made fallbacks), `room`, `dungeon`. Tile sets, rules and thresholds are the
//!   game's data.
//! - [`generate`]: the generator by stages (points → roads → required cells → fill →
//!   secondary points → decoration → checks), each a replaceable step; attempts with new
//!   random keys, then the fallback. Pure: seed, generator version and content decide.
//! - [`solver`]: the socket solver (sockets and turns, fixed cells, count limits,
//!   propagation, lowest-entropy-first choice, bounded backtracking).
//! - [`checks`]: reachability, slopes, distances between points of interest, isolated roads,
//!   and the game's own checks; each failure names its rule and cells.
//! - [`manifest`]: [`RegionManifest`], canonical and byte-stable, with its hash.
//! - [`dungeon`]: room graphs with a guaranteed path from entrance to goal.
//! - [`report`]: batch checks over many seeds (`worldgen_report`, `--worldgen-report`).
//! - [`region`]: building a manifest in the world, region changes through
//!   `EngineState::RegionLoading`, the start region.
//! - [`zones`]: [`ActiveZone`] / [`Dormant`] around players, every tick.
//! - [`stream`]: tile visuals loaded and unloaded by distance (windows only).
//!
//! Tile geometry is inline (boxes, solids, volumes, markers, prefabs in the RON) or a level
//! file from Blender (`geometry: Level(path: "tiles/ruin.gltf")`), which this crate does not
//! read: the level loader ([`LevelSpawner`]) does. `shards::EnginePlugins` registers
//! `shards_level`'s, so a game needs no setup; see `shards::TileLevelsPlugin` for what a
//! level tile is. The files are requested while the game loads, behind the loading gate, and a
//! file missing from the asset folder is a content error.
//!
//! ```ignore
//! app.add_plugins(EnginePlugins::new(config))        // WorldPlugin included
//!     .start_region("meadow")                        // generated with the world seed while loading
//!     .add_worldgen_step(Stage::Decorate, "flowers", flowers)
//!     .add_worldgen_check("river_crossings", crossings);
//! ```
//!
//! Tools: `regen`, `region_load`, `goto_tile` (dev actions, recorded in replays),
//! `worldgen_report` (dev query), `region_info`, and with a window `world_overlay` (tile
//! bounds, sockets, points, roads). `observe` gains `TILE` and `POI` lines, `ai/status` a
//! `world` field, `step --until named:near_poi` a stop condition.

pub mod checks;
pub mod content;
pub mod dungeon;
pub mod generate;
pub mod grid;
pub mod manifest;
pub mod region;
pub mod report;
pub mod sockets;
pub mod solver;
pub mod stream;
pub mod zones;

mod ai;
#[cfg(feature = "render")]
mod overlay;
mod tools;

use ai_core::{AiAppExt, SimSet};
use bevy::prelude::*;
use shards_content::ContentAppExt;
use shards_core::{CoreSystems, EngineAppExt};

pub use checks::{CheckFailure, CheckInput, RegionGraph};
pub use content::{
    CheckRules, DungeonDef, PoiDef, PoiRule, RegionDef, RoomDef, SocketRulesDef, TileDef, TileGeometry, TileMapDef, TilePiece,
};
pub use generate::{generate, CheckFn, GenContext, GenError, Generated, Stage, StepFn, WorldGen};
pub use grid::Dir;
pub use manifest::RegionManifest;
#[cfg(feature = "render")]
pub use overlay::TileOverlay;
pub use region::{
    change_region, despawn_region, level_paths, level_users, spawn_region, ActiveRegion, LevelCheckFn, LevelForgetFn, LevelNextFn,
    LevelRequestFn, LevelRestartFn, LevelSpawn, LevelSpawnFn, LevelSpawner, LevelStatusFn, Marker, Poi, RegionBuild, RegionEdge, RegionLoadedHooks,
    RegionPart, RegionTile, StartRegion, TileId,
};
pub use report::BatchReport;
pub use stream::{TileVisual, TileVisualFn, TileVisualSpawn};
pub use zones::{ActiveZone, Dormant};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{ActiveRegion, ActiveZone, Dormant, RegionManifest, Stage, TileId, WorldAppExt, WorldPlugin, WorldSettings};
}

/// How the world module behaves; insert it (or use [`WorldAppExt`]) before the app runs.
#[derive(Resource, Clone, Debug)]
pub struct WorldSettings {
    /// Region generated while the game loads, with the world seed; `None`: no region until
    /// `regen`.
    pub start_region: Option<String>,
    /// Characters farther than this (m) from every player are [`Dormant`]. A loaded region's
    /// `active_radius` wins; with neither and no region, no zones are kept.
    pub active_radius: Option<f32>,
    /// How much farther (m) an active character must go before it turns dormant.
    pub active_margin: f32,
    /// Tile visual LOD bands (m from the nearest player); beyond the last, unloaded.
    pub visual_lods: Vec<f32>,
    /// Stream tile visuals; `None`: only with a window.
    pub stream_visuals: Option<bool>,
}

impl Default for WorldSettings {
    fn default() -> Self {
        Self { start_region: None, active_radius: None, active_margin: 8.0, visual_lods: vec![96.0, 256.0], stream_visuals: None }
    }
}

/// The world module's systems, for ordering a game's systems against them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorldSystems {
    /// `SimSet::Input`, before orders are followed: [`ActiveZone`] / [`Dormant`].
    Zones,
}

/// Registration API for games.
pub trait WorldAppExt {
    /// Generates region `id` while the game loads, with the world seed.
    fn start_region(&mut self, id: &str) -> &mut Self;

    /// Adds a generation step after the others of `stage`.
    fn add_worldgen_step(&mut self, stage: Stage, name: &str, step: StepFn) -> &mut Self;

    /// Replaces every step of `stage`, the engine's included.
    fn replace_worldgen_stage(&mut self, stage: Stage, name: &str, step: StepFn) -> &mut Self;

    /// A check regions run when their `checks.custom` names it.
    fn add_worldgen_check(&mut self, name: &str, check: CheckFn) -> &mut Self;

    /// Registers the loader of tiles built from level files (`TileGeometry::Level`). The
    /// umbrella crate does it for `shards_level`; a game calls this for a loader of its own.
    fn set_level_spawner(&mut self, spawner: LevelSpawner) -> &mut Self;

    /// Makes what a window draws for a tile, per LOD ([`stream`]).
    fn set_tile_visual_spawner(&mut self, spawner: TileVisualFn) -> &mut Self;

    /// Runs `hook` each time a region has been built, inside the call that builds it
    /// ([`RegionLoadedHooks`]).
    fn on_region_loaded(&mut self, hook: fn(&mut World)) -> &mut Self;
}

impl WorldAppExt for App {
    fn start_region(&mut self, id: &str) -> &mut Self {
        self.world_mut().get_resource_or_init::<WorldSettings>().start_region = Some(id.to_owned());
        self
    }

    fn add_worldgen_step(&mut self, stage: Stage, name: &str, step: StepFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<WorldGen>().add_step(stage, name, step);
        self
    }

    fn replace_worldgen_stage(&mut self, stage: Stage, name: &str, step: StepFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<WorldGen>().replace_stage(stage, name, step);
        self
    }

    fn add_worldgen_check(&mut self, name: &str, check: CheckFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<WorldGen>().add_check(name, check);
        self
    }

    fn set_level_spawner(&mut self, spawner: LevelSpawner) -> &mut Self {
        self.insert_resource(spawner);
        self
    }

    fn set_tile_visual_spawner(&mut self, spawner: TileVisualFn) -> &mut Self {
        self.insert_resource(stream::TileVisualSpawner(spawner));
        self
    }

    fn on_region_loaded(&mut self, hook: fn(&mut World)) -> &mut Self {
        self.world_mut().get_resource_or_init::<RegionLoadedHooks>().0.push(hook);
        self
    }
}

/// Tiles, regions and dungeons as content, the generator, region loading, active zones,
/// tile streaming and the world tools. Needs the core, content and motion plugins
/// (`shards::EnginePlugins` adds them all).
pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_content::<TileDef>()
            .add_content::<SocketRulesDef>()
            .add_content::<RegionDef>()
            .add_content::<PoiDef>()
            .add_content::<TileMapDef>()
            .add_content::<RoomDef>()
            .add_content::<DungeonDef>()
            .register_type::<TileId>()
            .register_type::<RegionPart>()
            .register_type::<RegionEdge>()
            .register_type::<RegionTile>()
            .register_type::<Poi>()
            .register_type::<Marker>()
            .register_type::<ActiveZone>()
            .register_type::<Dormant>()
            // Snapshots: the region in the world is saved as its manifest (and the ids it took)
            // and built again before a load is checked, so its static colliders and level
            // files compare with the snapshot's.
            .add_snapshot_resource::<RegionBuild>()
            .add_snapshot_prepare("region", &["static_colliders", "levels"], region::snapshot_prepare)
            .init_resource::<WorldSettings>()
            .init_resource::<WorldGen>()
            .init_resource::<StartRegion>()
            .init_resource::<region::LevelFiles>()
            .require_loaded("region", region::start_status)
            .require_loaded("tile levels", region::levels_status)
            .add_engine_module("world", VERSION)
            .configure_sets(FixedUpdate, WorldSystems::Zones.in_set(SimSet::Input).before(CoreSystems::FollowOrders))
            .add_systems(FixedUpdate, zones::update_zones.in_set(WorldSystems::Zones))
            .add_systems(PreStartup, queue_start_region)
            .add_systems(PostUpdate, (region::request_level_files, region::report_level_files).chain())
            .add_systems(Update, (region::build_start_region, stream::stream_tile_visuals.run_if(stream::streaming)).chain());
        ai::plugin(app);
        tools::register(app.world_mut());
        tools::dev_entries(app);
        #[cfg(feature = "render")]
        overlay::plugin(app);
    }
}

/// The start region the settings ask for, waiting for content.
fn queue_start_region(settings: Res<WorldSettings>, mut start: ResMut<StartRegion>) {
    if let Some(region) = &settings.start_region
        && *start == StartRegion::None
    {
        *start = StartRegion::Pending(region.clone());
    }
}
