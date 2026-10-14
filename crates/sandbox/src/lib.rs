//! Sandbox for the Shards engine, built only on the `shards` crate, the way a game is.
//!
//! - **Arena** (the combat prototype): the player, two skeletons and the Frost Warden, a boss
//!   with four telegraphed attacks and an enraged second phase.
//! - **Obstacle course** east of it: stairs, a ledge to mantle, a pool to swim, a cliff
//!   and a gap only a glide (with an updraft) crosses, to a goal pad.
//! - **The wilds** south of it: a region the engine generates from a seed (8 × 8 tiles of
//!   16 m, one of them a level file from Blender, `assets/tiles/ruin.gltf`), with skeleton
//!   camps and a den in a pillar hall, walked over its navmesh. A dev goes there with
//!   `regen region=wilds seed=42`; the bake files of its tiles are in `assets/nav/`.
//!
//! The monsters are the engine's NPCs: their behaviours, perception, the combat director
//! and attack plans are content (`content/npc/`).
//!
//! Everything that is numbers or definitions is content in `assets/content/`: the world
//! layout, the characters (prefabs), how they move (locomotion), what they can do (actions,
//! movesets). Edit it while the game runs.
//!
//! Run windowed: `cargo sandbox` · headless: `cargo sandbox --headless --paused`
//! Then drive it from another terminal: `cargo shards observe`, `cargo shards step 60`.

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
        // The window title carries the seed and the port, so it cannot name the settings folder.
        app.insert_resource(SettingsAppId::new("shards-sandbox"));
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

/// Paused at tick 0 with the world spawned, headless, no logging, no HTTP and no
/// crash replay files: drive it with `shards::ai_core::call_tool`.
pub fn test_app(seed: u64) -> App {
    shards::testing::test_app(
        |config| config.seed = seed,
        |app| {
            app.add_plugins(SandboxPlugin);
        },
    )
}
