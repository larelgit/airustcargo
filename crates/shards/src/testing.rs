//! Test apps: the game's own plugins on the headless engine, loaded and paused at
//! tick 0, driven through tools like an agent drives them.
//!
//! ```ignore
//! let mut app = shards::testing::test_app(|config| config.seed = 7, |app| {
//!     app.add_plugins(MyGamePlugin);
//! });
//! shards::testing::ok(&mut app, "step", json!({"ticks": 60}));
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ai_core::replay::{self, Replay};
use ai_core::sim::{StepEnd, StepReport};
use ai_core::{call_tool, AiReplay, CallOutcome, ReplayReport, SimGate, SystemFailure, Violation};
use bevy::prelude::*;
use serde_json::Value;
use shards_core::{EngineConfig, EngineState, NetLaunch, SettingsStore};
use shards_net::{LinkConditions, MemoryHub, PeerId, Stand};
use shards_save::SaveConfig;
use shards_script::{Driver, JournalRead, Outcome, Report, Script, SessionConfig, Status};

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

/// A save folder an app of a test has to itself, removed with the app. A test app that does
/// not name `save_dir` writes there (a session that ends saves the world and the host's
/// character), so that tests, which run side by side, never share saves and leave none behind.
#[derive(Resource)]
struct TestSaves(PathBuf);

impl Drop for TestSaves {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Gives `app` a [`TestSaves`] folder unless its `config` names a `save_dir` of its own. A
/// test that sets `SaveConfig::dir` itself, after this, simply replaces it.
fn own_saves(app: &mut App, config: &EngineConfig) {
    static N: AtomicUsize = AtomicUsize::new(0);
    if config.save_dir != EngineConfig::default().save_dir {
        return;
    }
    let dir = std::env::temp_dir().join(format!("shards_test_saves_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    app.world_mut().resource_mut::<SaveConfig>().dir = dir.clone();
    app.insert_resource(TestSaves(dir));
}

/// Builds a headless test app: [`config`] changed by `configure`, the engine, then the
/// game's plugins from `setup`; finishes it and updates it until loading is done.
/// Returns it paused at tick 0, the world spawned. Panics, saying what is still loading,
/// after [`LOAD_TIMEOUT`].
pub fn test_app(configure: impl FnOnce(&mut EngineConfig), setup: impl FnOnce(&mut App)) -> App {
    let mut config = config();
    configure(&mut config);
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config.clone()));
    own_saves(&mut app, &config);
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

/// A stand: the host and `clients` clients (`c1`, `c2`, …) of the game `setup` adds, in this
/// process, on an in-memory hub with `links` on every link (`LinkConditions::NONE`: none).
/// `configure` changes every member's [`config`] (the host's seed is the session's). Returns it
/// settled: loaded, every client joined at the host's tick 0. Panics, saying who waits for
/// what, after [`LOAD_TIMEOUT`].
pub fn stand(clients: u8, links: LinkConditions, configure: impl Fn(&mut EngineConfig), setup: impl Fn(&mut App)) -> Stand {
    let mut stand = stand_host(clients, &configure, &setup);
    for n in 1..=clients {
        stand_client(&mut stand, &format!("c{n}"), links, &configure, &setup);
    }
    if let Err(e) = stand.settle() {
        panic!("{e}");
    }
    stand
}

/// A stand with only its host, loaded: clients join later with [`stand_client`].
pub fn stand_host(clients: u8, configure: &impl Fn(&mut EngineConfig), setup: &impl Fn(&mut App)) -> Stand {
    let mut config = config();
    configure(&mut config);
    config.net = NetLaunch::Stand { clients, udp: false };
    config.player_name = "host".into();
    let hub = MemoryHub::new(config.seed);
    let mut host = App::new();
    host.add_plugins(EnginePlugins::new(config.clone()));
    own_saves(&mut host, &config);
    setup(&mut host);
    crate::net::stand::attach(&mut host, &hub, "host", None);
    host.finish();
    host.cleanup();
    let mut stand = Stand::new(hub, vec![("host".into(), host)]);
    stand.load_timeout = LOAD_TIMEOUT;
    if let Err(e) = stand.settle() {
        panic!("{e}");
    }
    stand
}

/// Adds client `name` to `stand`: it joins during the stand's next ticks (or
/// [`Stand::settle`]). Returns its member index.
pub fn stand_client(
    stand: &mut Stand,
    name: &str,
    links: LinkConditions,
    configure: &impl Fn(&mut EngineConfig),
    setup: &impl Fn(&mut App),
) -> usize {
    let mut config = config();
    configure(&mut config);
    config.net = NetLaunch::Join { addr: "host".into() };
    config.player_name = name.to_owned();
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config.clone()));
    own_saves(&mut app, &config);
    setup(&mut app);
    // The host is the hub's first endpoint.
    let host: PeerId = 0;
    let id = crate::net::stand::attach(&mut app, &stand.hub, name, Some(host));
    let link = stand.hub.connect_both(host, id, links);
    app.finish();
    app.cleanup();
    stand.add(name, app, Some(link))
}

/// A [`Stand`] as the game a stand scenario drives: member `host` is the stand's host, `c1`,
/// `c2` its clients. Calls go through [`Stand::call_as`], so a deferred `step` runs as
/// stand ticks and answers like the tool would. Build one with [`stand`] and hand it to
/// [`run_stand_script`], or let [`run_stand_scenario`] do both.
pub struct StandDriver<'a> {
    pub stand: &'a mut Stand,
}

impl StandDriver<'_> {
    fn index(&self, peer: &str) -> Result<usize, String> {
        self.stand.index(peer).ok_or_else(|| format!("this stand has no member `{peer}`"))
    }
}

impl Driver for StandDriver<'_> {
    fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
        self.call_on(HOST_MEMBER, agent, tool, args)
    }

    fn session(&mut self, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
        self.session_on(HOST_MEMBER, agent, config)
    }

    fn status(&mut self) -> Result<Status, String> {
        self.status_on(HOST_MEMBER)
    }

    fn journal_since(&mut self, after: u64) -> Result<JournalRead, String> {
        self.journal_since_on(HOST_MEMBER, after)
    }

    fn peers(&mut self) -> Result<Vec<String>, String> {
        Ok(self.stand.members.iter().map(|m| m.name.clone()).collect())
    }

    fn call_on(&mut self, peer: &str, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
        let member = self.index(peer)?;
        let out = self.stand.call_as(member, agent, tool, args);
        Ok(Outcome { ok: out.ok, result: out.result, tick: Some(out.tick) })
    }

    fn session_on(&mut self, peer: &str, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
        let member = self.index(peer)?;
        ai_core::activate(self.stand.world(member));
        Ok(ai_core::scenario::configure_agent(self.stand.world_mut(member), agent, config))
    }

    fn status_on(&mut self, peer: &str) -> Result<Status, String> {
        let member = self.index(peer)?;
        ai_core::activate(self.stand.world(member));
        Ok(ai_core::scenario::world_status(self.stand.world_mut(member)))
    }

    fn journal_since_on(&mut self, peer: &str, after: u64) -> Result<JournalRead, String> {
        let member = self.index(peer)?;
        ai_core::activate(self.stand.world(member));
        Ok(ai_core::scenario::world_journal_since(self.stand.world_mut(member), after))
    }
}

/// Runs a parsed stand scenario against `stand` (the host first, then `c1`, `c2`, as
/// [`stand`] builds them). A header that does not fit the stand is a failure in the report.
pub fn run_stand_script(stand: &mut Stand, script: &Script) -> Report {
    shards_script::run(script, &mut StandDriver { stand })
}

/// What [`run_stand_scenario`] made: the report and the stand it ran on, left where the
/// scenario ended (look at the members' journals and worlds, or run it on).
pub struct StandRun {
    pub report: Report,
    /// `None` when the file could not be read or parsed, or has no `stand` line: nothing ran.
    pub stand: Option<Stand>,
}

/// Reads the stand scenario `path`, builds the stand its `stand` line describes (the game
/// `setup` adds to every member; the file's `seed` line is the session's seed; the line's
/// link conditions are on every client's link) and runs it. The same file runs against a
/// live stand with `shards scenario FILE`.
pub fn run_stand_scenario(setup: impl Fn(&mut App), path: impl AsRef<std::path::Path>) -> StandRun {
    let path = path.as_ref();
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    match std::fs::read_to_string(path) {
        Ok(text) => run_stand_str(setup, &name, &text),
        Err(e) => StandRun { report: Report::unreadable(&name, format!("cannot read {}: {e}", path.display())), stand: None },
    }
}

/// [`run_stand_scenario`] for scenario `text`; `name` stands for the file in errors and reports.
pub fn run_stand_str(setup: impl Fn(&mut App), name: &str, text: &str) -> StandRun {
    let nothing = |report| StandRun { report, stand: None };
    let script = match Script::parse(name, text) {
        Ok(script) => script,
        Err(errors) => return nothing(Report::parse_failed(name, &errors)),
    };
    let Some(header) = script.stand().copied() else {
        let hint = "it has no `stand` line; run a single-game scenario with ai_core::scenario::run_file";
        return nothing(Report::unreadable(name, hint));
    };
    let links = LinkConditions { latency_ms: header.latency_ms, jitter_ms: header.jitter_ms, loss: header.loss, duplicate: header.dup };
    let seed = script.seed();
    let mut built = stand(
        header.clients,
        links,
        |config| {
            if let Some(seed) = seed {
                config.seed = seed;
            }
        },
        setup,
    );
    let report = run_stand_script(&mut built, &script);
    StandRun { report, stand: Some(built) }
}

/// The host of a stand, as a scenario names it.
const HOST_MEMBER: &str = shards_script::HOST;
