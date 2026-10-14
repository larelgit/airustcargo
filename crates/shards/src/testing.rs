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

use ai_core::replay::{self, Replay};
use ai_core::sim::{StepEnd, StepReport};
use ai_core::{call_tool, AiReplay, CallOutcome, ReplayReport, SimGate, SystemFailure, Violation};
use bevy::prelude::*;
use serde_json::Value;
use shards_core::{EngineConfig, EngineState, SettingsStore};

use crate::EnginePlugins;

/// How long [`test_app`] waits for loading.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// The config of a test app: headless, testing, paused, no BRP server, no crash or
/// invariant replay files, no file watcher, no tick timing in replies and the journal (it
/// differs from run to run; a test that looks at it turns on `AiPerf::report`).
pub fn config() -> EngineConfig {
    EngineConfig {
        headless: true,
        testing: true,
        paused: true,
        port: None,
        crash_replays: false,
        report_perf: false,
        hot_reload: false,
        settings: SettingsStore::Off,
        ..default()
    }
}

/// The config of a game embedded beside another in one process (a stand's client): like
/// [`config`] (headless, paused, no logger, no run loop, no Ctrl+C handler), but with BRP
/// port `port`, and its log lines in its own journal.
pub fn embedded_config(port: Option<u16>) -> EngineConfig {
    EngineConfig { testing: false, embedded: true, port, ..config() }
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

/// What [`run_to`] met on the way.
#[derive(Clone, Debug, Default)]
pub struct RunStops {
    /// The gameplay failures that stopped it early (none: it got there).
    pub failures: Vec<SystemFailure>,
    /// Every invariant violated on the way, in order. A violation halts the game, but a
    /// rule that fired stays off, so the run goes on past it.
    pub violations: Vec<Violation>,
}

/// Runs ticks until tick `end`, waiting out loading on the way: a region change loads
/// behind the gate, and playback waits for it as the recording did. Stops early at a
/// failed gameplay system; goes on past violated invariants, collecting them.
pub fn run_to(app: &mut App, end: u64, load_timeout: Duration) -> Result<RunStops, String> {
    let mut stops = RunStops::default();
    loop {
        let left = end.saturating_sub(ai_core::sim::tick(app.world()));
        match ai_core::sim::step(app.world_mut(), left, Duration::from_secs(24 * 3600), |_| None)? {
            StepReport { end: StepEnd::SystemFailed(failures), .. } => {
                stops.failures = failures;
                return Ok(stops);
            }
            StepReport { end: StepEnd::InvariantViolated(violated), to_tick, .. } => {
                stops.violations.extend(violated);
                // The tick that broke a rule may also have started a load (a region change
                // behind the gate): the gate holds until it is waited out, here as below.
                if loading(app.world()).is_some() {
                    wait_until_loaded(app, load_timeout)?;
                }
                if to_tick >= end {
                    return Ok(stops);
                }
            }
            report if report.to_tick < end && loading(app.world()).is_some() => wait_until_loaded(app, load_timeout)?,
            _ => return Ok(stops),
        }
    }
}

/// Plays `recording` on `app` — the game built fresh with the recording's seed, finished but
/// not yet loaded — to its end, and returns how it compares (`--verify` in a test). A failed
/// gameplay system or a violated invariant on the way is an error.
pub fn verify(app: &mut App, recording: Replay) -> Result<ReplayReport, String> {
    let end = recording.end.tick;
    replay::play(app.world_mut(), recording)?;
    wait_until_loaded(app, LOAD_TIMEOUT)?;
    let stops = run_to(app, end, LOAD_TIMEOUT)?;
    if let Some(failure) = stops.failures.first() {
        return Err(format!("gameplay {}: {}", failure.headline(), failure.message));
    }
    if let Some(violation) = stops.violations.first() {
        return Err(format!("{}: {}", violation.headline(), violation.text));
    }
    app.update();
    app.world().resource::<AiReplay>().report().cloned().ok_or_else(|| "the replay did not finish".to_owned())
}

/// Calls `tool` as agent `dev` (dev scope, the player's body).
pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

/// [`call`] that must succeed: panics with the tool's error otherwise, and when the game
/// halted during it (a gameplay system failed, an invariant was violated). (Test apps catch
/// system panics like games do, and `step` reports one as a successful call that `stopped:
/// gameplay system …` or `stopped: invariant …`; use [`call`] in a test that expects it.)
pub fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    let text = out.result.as_str().unwrap_or_default();
    assert!(!text.contains(", stopped: gameplay "), "{tool} {args}: {text}");
    assert!(out.halted.is_none(), "{tool} {args}: the game halted: {text}");
    out
}
