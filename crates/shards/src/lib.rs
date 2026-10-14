//! # shards — the Shards engine as one dependency
//!
//! A game depends on this crate alone and gets a deterministic fixed-tick core with
//! the AI layer ([`ai_core`]), engine states and the loading gate ([`core`]), the
//! content database ([`content`]) and data-driven combat ([`action`]); Bevy and serde
//! come re-exported.
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
//! --paused --hz --replay --verify --check-content`. [`testing::test_app`] builds the
//! same game headless for tests, loaded and paused at tick 0.
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

mod plugins;
mod run;
pub mod testing;

pub use ai_core;
pub use bevy;
pub use schemars;
pub use serde;
pub use serde_json;
pub use shards_action as action;
pub use shards_content as content;
pub use shards_core as core;
pub use shards_motion as motion;

pub use plugins::EnginePlugins;
pub use run::run;
pub use shards_core::EngineConfig;

/// Everything a game module needs in scope.
pub mod prelude {
    pub use ai_core::prelude::*;
    pub use bevy::prelude::*;
    pub use schemars::JsonSchema;
    pub use serde::{Deserialize, Serialize};
    pub use shards_action::prelude::*;
    pub use shards_content::prelude::*;
    pub use shards_core::prelude::*;

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
