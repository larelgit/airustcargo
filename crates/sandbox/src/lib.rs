//! Sandbox arena for the Shards engine: a player, two skeletons and the Frost Warden
//! boss. Built only on the `shards` crate, the way a game is.
//!
//! Run windowed: `cargo sandbox` · headless: `cargo sandbox --headless --paused`
//! Then drive it from another terminal: `cargo shards observe`, `cargo shards step 60`.
//! Attack numbers live in `assets/content/combat/attacks.attack.ron`; edit them while
//! the game runs.

// Bevy queries name every component they touch.
#![allow(clippy::type_complexity)]

pub mod ai;
pub mod gameplay;
#[cfg(feature = "render")]
pub mod visuals;

use shards::prelude::*;

/// The sandbox game on top of the engine: gameplay and what the AI sees of it.
pub struct SandboxPlugin;

impl Plugin for SandboxPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((gameplay::GameplayPlugin, ai::GameAiPlugin));
    }
}

/// The app `shards::run` runs: the engine from `config`, the sandbox, and its looks
/// when there is a window.
pub fn build_app(config: &EngineConfig) -> App {
    let title = format!("Shards sandbox · seed {} · BRP :{}", config.seed, config.port.unwrap_or(0));
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(EngineConfig { title, ..config.clone() })).add_plugins(SandboxPlugin);
    if !config.headless && !config.testing {
        #[cfg(feature = "render")]
        app.add_plugins(visuals::VisualsPlugin);
        #[cfg(not(feature = "render"))]
        panic!("built without the `render` feature: run with --headless");
    }
    app
}

/// Paused at tick 0 with the arena spawned, headless, no logging, no HTTP and no
/// crash replay files: drive it with `shards::ai_core::call_tool`.
pub fn test_app(seed: u64) -> App {
    shards::testing::test_app(|config| config.seed = seed, |app| {
        app.add_plugins(SandboxPlugin);
    })
}
