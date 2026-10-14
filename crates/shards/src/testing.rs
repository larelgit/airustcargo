//! Test apps: the game's own plugins on the headless engine, loaded and paused at
//! tick 0, driven through tools like an agent drives them.
//!
//! ```ignore
//! let mut app = shards::testing::test_app(|config| config.seed = 7, |app| {
//!     app.add_plugins(MyGamePlugin);
//! });
//! shards::testing::ok(&mut app, "step", json!({"ticks": 60}));
//! ```

use std::time::{Duration, Instant};

use ai_core::{call_tool, CallOutcome, SimGate};
use bevy::prelude::*;
use serde_json::Value;
use shards_core::{EngineConfig, EngineState};

use crate::EnginePlugins;

/// How long [`test_app`] waits for loading.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// The config of a test app: headless, testing, paused, no BRP server, no crash replay
/// files, no file watcher.
pub fn config() -> EngineConfig {
    EngineConfig {
        headless: true,
        testing: true,
        paused: true,
        port: None,
        crash_replays: false,
        hot_reload: false,
        ..default()
    }
}

/// Builds a headless test app: [`config`] changed by `configure`, the engine, then the
/// game's plugins from `setup`; finishes it and updates it until loading is done.
/// Returns it paused at tick 0, the world spawned. Panics, saying what is still loading,
/// after [`LOAD_TIMEOUT`].
pub fn test_app(configure: impl FnOnce(&mut EngineConfig), setup: impl FnOnce(&mut App)) -> App {
    let mut config = config();
    configure(&mut config);
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config));
    setup(&mut app);
    app.finish();
    app.cleanup();
    if let Err(e) = wait_until_loaded(&mut app, LOAD_TIMEOUT) {
        panic!("{e}");
    }
    app
}

/// Updates `app` until it has left the loading states and nothing holds the simulation.
/// For apps already finished (`App::finish`, `App::cleanup`).
pub fn wait_until_loaded(app: &mut App, timeout: Duration) -> Result<(), String> {
    let started = Instant::now();
    while let Some(waiting) = loading(app.world()) {
        if started.elapsed() > timeout {
            let secs = timeout.as_secs();
            return Err(format!("the game did not finish loading within {secs}s; still waiting for: {waiting}"));
        }
        app.update();
        // Files load on the IO threads.
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// What the game is still loading (the gate's reasons, or its state), `None` once it has
/// left the loading states and nothing holds the simulation.
pub fn loading(world: &World) -> Option<String> {
    let state = world.get_resource::<State<EngineState>>().map(|s| *s.get());
    let reasons = world.get_resource::<SimGate>().map(SimGate::reasons).unwrap_or_default();
    let loading = matches!(state, None | Some(EngineState::Loading | EngineState::RegionLoading));
    match (loading, reasons.is_empty()) {
        (false, true) => None,
        (_, false) => Some(reasons.join(", ")),
        (true, true) => Some(format!("{state:?}")),
    }
}

/// Calls `tool` as agent `dev` (dev scope, the player's body).
pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

/// [`call`] that must succeed: panics with the tool's error otherwise, and when a
/// gameplay system failed during it. (Test apps catch system panics like games do, and
/// `step` reports one as a successful call that `stopped: gameplay system …`; use
/// [`call`] in a test that expects it.)
pub fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    let text = out.result.as_str().unwrap_or_default();
    assert!(!text.contains(", stopped: gameplay "), "{tool} {args}: {text}");
    out
}
