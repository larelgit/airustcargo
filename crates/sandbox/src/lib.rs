//! Sandbox arena for `ai_core`: a player, two skeletons and the Frost Warden boss.
//!
//! Run windowed: `cargo sandbox` · headless: `cargo sandbox --headless --paused`
//! Then drive it from another terminal: `cargo shards observe`, `cargo shards step 60`.

pub mod ai;
pub mod gameplay;
#[cfg(feature = "render")]
pub mod visuals;

use std::time::Duration;

use ai_core::prelude::*;
use ai_core::AiPerception;
use bevy::app::{ScheduleRunnerPlugin, TerminalCtrlCHandlerPlugin};
use bevy::log::LogPlugin;
use bevy::prelude::*;

#[derive(Clone, Debug)]
pub struct GameConfig {
    /// No window, no GPU: CI, parallel playtests, servers.
    pub headless: bool,
    pub seed: u64,
    /// BRP port for `shards` (`None`: no HTTP server).
    pub port: Option<u16>,
    pub paused: bool,
    pub tick_hz: f64,
    /// A gameplay failure saves `replays/crash-tick<N>.json`.
    pub crash_replays: bool,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            headless: false,
            seed: 42,
            port: Some(ai_core::DEFAULT_PORT),
            paused: false,
            tick_hz: 60.0,
            crash_replays: true,
        }
    }
}

fn ai_plugin(cfg: &GameConfig) -> AiCorePlugin {
    AiCorePlugin {
        sim: SimConfig { tick_hz: cfg.tick_hz, seed: cfg.seed, start_paused: cfg.paused },
        perception: AiPerception::default(),
        remote_port: cfg.port,
        crash_replays: cfg.crash_replays,
        ..default()
    }
}

pub fn build_app(cfg: &GameConfig) -> App {
    let mut app = App::new();
    let log = LogPlugin { custom_layer: ai_core::capture_log_layer, ..default() };
    if cfg.headless {
        let frame = Duration::from_secs_f64(1.0 / 60.0);
        // Ctrl+C exits through `AppExit`, so files written at start (--port-file) are removed.
        app.add_plugins((MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(frame)), TerminalCtrlCHandlerPlugin, log));
    } else {
        #[cfg(feature = "render")]
        {
            let window = WindowPlugin {
                primary_window: Some(Window {
                    title: format!("Shards sandbox · seed {} · BRP :{}", cfg.seed, cfg.port.unwrap_or(0)),
                    resolution: (1280, 720).into(),
                    ..default()
                }),
                ..default()
            };
            app.add_plugins(DefaultPlugins.set(log).set(window)).add_plugins(visuals::VisualsPlugin);
        }
        #[cfg(not(feature = "render"))]
        panic!("built without the `render` feature: run with --headless");
    }
    app.add_plugins(ai_plugin(cfg)).add_plugins((gameplay::GameplayPlugin, ai::GameAiPlugin));
    app
}

/// Paused, headless, no logging, no HTTP and no crash replay files: drive it with
/// `ai_core::call_tool`.
pub fn test_app(seed: u64) -> App {
    let cfg = GameConfig { headless: true, seed, port: None, paused: true, crash_replays: false, ..default() };
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(ai_plugin(&cfg))
        .add_plugins((gameplay::GameplayPlugin, ai::GameAiPlugin));
    app.finish();
    app.cleanup();
    app.update();
    app
}
