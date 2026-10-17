//! A minimal game on the Shards engine, to copy and grow.
//!
//! It depends on one crate, `shards`. The engine gives it a deterministic fixed-tick core,
//! the AI layer (`shards observe`, `shards step`, scenarios, replays), a content database,
//! a character controller with physics, and data-driven combat. This crate only adds what
//! makes it *this* game:
//!
//! | module | what |
//! |---|---|
//! | [`arena`] | the level as content (`assets/content/arena.arena.ron`) and the system that builds it |
//! | [`brain`] | the grunt: notices the hero and attacks it, through `Intent` like a player |
//! | [`ai`] | what an agent can do that is this game's (`taunt`) and wait for (`arena_cleared`) |
//! | [`progress`] | what a save keeps that is this game's: a hero's trophies, the yard's fallen grunts |
//! | [`screens`] | the HUD and menus are content (`assets/content/screens.screen.ron`); this adds the trophies a HUD shows and the menu's save-and-quit |
//! | `visuals` | meshes, light and the camera when there is a window (`render` feature) |
//!
//! All numbers are content in `assets/content/`: how the hero moves, every action's timing
//! and damage, movesets, fighter stats (prefabs), the level. Edit them while the game runs.
//!
//! The engine does more with content alone, no code here: the valley (`content/region/`) is a
//! region its generator makes from a seed, one of its tiles a level file from Blender
//! (`assets/tiles/ruin.gltf`); its raiders are NPCs of the engine (`content/npc/`) that walk the
//! navmesh baked into `assets/nav/` and keep to their camps. A dev goes there with
//! `shards act regen region=valley seed=42`.
//!
//! Saves and co-op come with the engine too. `--world SLOT --character SLOT` load a save at the
//! start and autosaves write it; `save_game`, `load_game` and `save_check` are tools. The game
//! adds only what is its own to a save ([`progress`]). Two players play the yard together with
//! `--stand 1` (host and client in one process), or `--host 0` and `--join ADDR` on two machines:
//! the second hero comes from the same prefab (`content/session.net.ron`), and its trophies go
//! home with it.
//!
//! ```text
//! cargo game                              # window: WASD, mouse, Space, Ctrl, left click
//! cargo game --headless --paused &        # no window, time stands still until `shards step`
//! shards wait && shards observe           # what the world looks like, as text
//! shards scenario scenarios/smoke.scenario
//! ```

// Bevy queries name every component they touch, which clippy finds complex.
#![allow(clippy::type_complexity)]

pub mod ai;
pub mod arena;
pub mod brain;
pub mod progress;
pub mod screens;
#[cfg(feature = "render")]
pub mod visuals;

use shards::prelude::*;

/// Everything that is this game, on top of the engine: its content kinds, its level, its
/// brains and what its AI agents can do. Add it after [`EnginePlugins`].
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        // The player's settings (rebound keys, volume, accessibility) are kept in the user's config folder, under this
        // game's name (`my_game` becomes the folder `my-game`). Spelled like the package, so the rename of the package
        // finds it: games made from the template must not share one folder.
        app.insert_resource(SettingsAppId::new("my_game"));
        // Earlier versions of the template kept rebound keys in `~/.config/my_game/input.ron`; asking
        // for them here imports them once. A game that never shipped that file deletes this line.
        app.insert_resource(shards::motion::InputSettings::for_game("my_game"));
        app.add_plugins((arena::ArenaPlugin, brain::BrainPlugin, ai::GameAiPlugin, progress::ProgressPlugin, screens::ScreensPlugin));
    }
}

/// The app `shards::run` runs: the engine from `config` (which the command line filled:
/// `--headless`, `--seed`, ...), this game, and its looks when something is drawn (a window, `--offscreen`).
pub fn build_app(config: &EngineConfig) -> App {
    let mut app = App::new();
    // The title and the version name the game in its window and its crash reports.
    let game = EngineConfig { title: "My game".into(), version: Some(env!("CARGO_PKG_VERSION").into()), ..config.clone() };
    app.add_plugins(EnginePlugins::new(game)).add_plugins(GamePlugin);
    if config.draws() {
        #[cfg(feature = "render")]
        app.add_plugins(visuals::VisualsPlugin);
        #[cfg(not(feature = "render"))]
        panic!("built without the `render` feature: run with --headless");
    }
    app
}

/// A headless test game: paused at tick 0 with the arena spawned, no logger, no HTTP, no
/// crash replay files. Drive it with `shards::testing::{call, ok}` or scenarios.
pub fn test_app(seed: u64) -> App {
    shards::testing::test_app(|config| config.seed = seed, |app| {
        app.add_plugins(GamePlugin);
    })
}
