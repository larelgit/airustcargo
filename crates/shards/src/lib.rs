//! # shards — the Shards engine as one dependency
//!
//! A game depends on this crate alone and gets a deterministic fixed-tick core with
//! the AI layer ([`ai_core`]), engine states and the loading gate ([`core`]), the
//! content database ([`content`]), physics, the character controller, input and
//! camera ([`motion`]), data-driven combat ([`action`]), levels from Blender ([`level`]),
//! generated regions ([`world`]), enemy AI ([`npc`]), saves ([`save`]), the player's settings
//! ([`settings`]), screens and menus ([`ui`]) and their languages ([`locale`]), sound and music
//! ([`audio`]), how meshes look ([`look`]), foliage over the tiles ([`scatter`]), effects and
//! telegraph decals ([`fx`]), the developer menu and console ([`dev`]); Bevy and serde come
//! re-exported.
//!
//! ```ignore
//! use shards::prelude::*;
//!
//! fn main() -> std::process::ExitCode {
//!     shards::run(|config| {
//!         let mut app = App::new();
//!         app.add_plugins(EnginePlugins::new(EngineConfig { title: "My game".into(), ..config.clone() }))
//!             .add_plugins(MyGamePlugin);
//!         app
//!     })
//! }
//! ```
//!
//! [`run`] is the standard `main`: `--headless --seed --port --port-file --no-remote
//! --paused --hz --replay --verify [--windowed] --check-every N
//! --check-detail components|entities|none --tick-budget-ms MS --check-content
//! --worldgen-report REGION --seeds 1..1000 --bake-nav [TILES] --settings-dir DIR --no-settings`.
//! [`testing::test_app`] builds the same game headless for tests, loaded and paused at tick 0. The
//! player's settings start after the game's builder returned, so every settings group a game
//! registers is there ([`settings::install`]).
//!
//! A game without a `bevy` dependency of its own uses Bevy's derives through the
//! prelude (`use shards::prelude::*` in the module). serde and schemars derives need
//! the crate path once per type:
//!
//! ```ignore
//! #[derive(Serialize, Deserialize, JsonSchema)]
//! #[serde(crate = "shards::serde")]
//! #[schemars(crate = "shards::schemars")]
//! struct Args { target: Id }
//! ```

mod audio_wiring;
pub mod bench;
pub mod coop;
#[cfg(feature = "render")]
pub mod frames;
mod net_wiring;
#[cfg(feature = "render")]
mod offscreen;
mod plugins;
mod run;
pub mod save_sections;
mod stand_udp;
mod ui_wiring;
#[cfg(feature = "render")]
mod window_icon;
pub mod testing;
pub mod tile_levels;

pub use ai_core;
pub use bevy;
pub use schemars;
pub use serde;
pub use serde_json;
pub use shards_action as action;
pub use shards_audio as audio;
pub use shards_content as content;
pub use shards_core as core;
pub use shards_dev as dev;
pub use shards_fx as fx;
pub use shards_level as level;
pub use shards_look as look;
pub use shards_locale as locale;
pub use shards_motion as motion;
pub use shards_net as net;
pub use shards_npc as npc;
pub use shards_save as save;
pub use shards_scatter as scatter;
pub use shards_script as script;
pub use shards_settings as settings;
#[cfg(feature = "steam")]
pub use shards_steam as steam;
pub use shards_ui as ui;
pub use shards_world as world;

pub use plugins::EnginePlugins;
pub use run::run;
pub use shards_core::EngineConfig;
pub use tile_levels::TileLevelsPlugin;
pub use audio_wiring::EngineAudioPlugin;
pub use ui_wiring::{EngineUiPlugin, PAUSE_SCREEN};

// glam keeps its SIMD. Its `scalar-math` feature (parry3d's `enhanced-determinism` turns it on, and
// a feature is on for the whole build once any crate asks) drops the 16-byte alignment of `Vec4` and
// `Mat4`. Bevy 0.20 sizes its GPU mesh buffers by the Rust layout (`size_of::<MeshUniform>()` in
// `UninitBufferVec`), so the buffers stop matching the shader's struct, 168 bytes against 176, and
// wgpu stops the renderer: "Binding size 168 of Buffer with '' label is less than minimum 176". A
// headless build would run, but replays and co-op need every build of a game to compute alike, so
// the build is refused here. README, "Время и детерминизм": what determinism the engine promises
// without `scalar-math`.
const _: () = assert!(
    std::mem::align_of::<bevy::math::Vec4>() == 16 && std::mem::align_of::<bevy::math::Mat4>() == 16,
    "glam is built with `scalar-math` (parry3d's `enhanced-determinism` turns it on): Vec4 and Mat4 lose their \
     16-byte alignment, Bevy 0.20's MeshUniform no longer matches its shader (168 bytes against 176) and \
     rendering stops. Keep glam's SIMD: see `parry3d` in the engine's Cargo.toml."
);

/// Everything a game module needs in scope.
pub mod prelude {
    pub use ai_core::prelude::*;
    pub use bevy::prelude::*;
    pub use schemars::JsonSchema;
    pub use serde::{Deserialize, Serialize};
    pub use shards_action::prelude::*;
    pub use shards_audio::prelude::*;
    pub use shards_content::prelude::*;
    pub use shards_core::prelude::*;
    pub use shards_level::prelude::*;
    pub use shards_locale::prelude::*;
    pub use shards_motion::prelude::*;
    pub use shards_npc::prelude::*;
    pub use shards_save::prelude::*;
    pub use shards_settings::prelude::*;
    pub use shards_ui::prelude::*;
    pub use shards_world::prelude::*;

    pub use crate::EnginePlugins;

    // Bevy's derives (`Component`, `Reflect`, `States`, `Asset`, ...) name these crates
    // when the game has no `bevy` dependency of its own; these aliases answer them.
    pub use bevy::app as bevy_app;
    pub use bevy::asset as bevy_asset;
    pub use bevy::ecs as bevy_ecs;
    pub use bevy::reflect as bevy_reflect;
    pub use bevy::state as bevy_state;
    #[cfg(feature = "render")]
    pub use bevy::{
        animation as bevy_animation, extract as bevy_extract, gizmos as bevy_gizmos, image as bevy_image,
        material as bevy_material, render as bevy_render,
    };
}
