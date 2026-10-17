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
use std::sync::Arc;
use std::time::{Duration, Instant};

use ai_core::replay::{self, Replay};
use ai_core::sim::{StepEnd, StepReport};
use ai_core::{call_tool, AiReplay, CallOutcome, HeavyConfig, HeavyFailure, ReplayReport, SimGate, SystemFailure, Violation};
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
        engine_invariants: true,
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

/// Gives `app` a [`TestSaves`] folder unless its `config` names a `save_dir` of its own (or it
/// has one already). A test that sets `SaveConfig::dir` itself, after this, simply replaces it.
pub(crate) fn own_saves(app: &mut App, config: &EngineConfig) {
    static N: AtomicUsize = AtomicUsize::new(0);
    if config.save_dir != EngineConfig::default().save_dir || app.world().contains_resource::<TestSaves>() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("shards_test_saves_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    // A game without the save module saves nothing.
    let Some(mut saves) = app.world_mut().get_resource_mut::<SaveConfig>() else { return };
    saves.dir = dir.clone();
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
    /// Every heavy check that failed on the way (heavy mode: [`heavy_app`]). The checks stay
    /// off after one, so the run goes on past it.
    pub heavy: Vec<HeavyFailure>,
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
            StepReport { end: StepEnd::HeavyFailed(failed), to_tick, .. } => {
                stops.heavy.extend(failed);
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
    if let Some(failure) = stops.heavy.first() {
        return Err(format!("{}: {}", failure.headline(), failure.differences.join("; ")));
    }
    app.update();
    app.world().resource::<AiReplay>().report().cloned().ok_or_else(|| "the replay did not finish".to_owned())
}

/// The game a heavy test app is: [`config`] changed by `configure` with heavy mode `heavy`, and
/// the builder that makes it, and its twins, from the engine and `setup`.
fn heavy_game(
    configure: impl Fn(&mut EngineConfig) + Send + Sync + 'static,
    setup: impl Fn(&mut App) + Send + Sync + 'static,
    heavy: HeavyConfig,
) -> (EngineConfig, crate::heavy::Builder) {
    let mut config = config();
    configure(&mut config);
    config.heavy = Some(heavy);
    let build: crate::heavy::Builder = Arc::new(move |config: &EngineConfig| {
        let mut app = App::new();
        app.add_plugins(EnginePlugins::new(config.clone()));
        own_saves(&mut app, config);
        setup(&mut app);
        app
    });
    (config, build)
}

/// [`heavy_game`] built and finished, not loaded: a recording is played on it before it loads.
fn heavy_unloaded(config: &EngineConfig, build: crate::heavy::Builder) -> App {
    let mut app = build(config);
    crate::heavy::install(&mut app, config, build);
    app.finish();
    app.cleanup();
    app
}

/// [`test_app`] in heavy mode (`--heavy`): every `heavy.every` ticks a twin, built again from
/// `configure` and `setup`, loads a snapshot of the app and must continue like it
/// ([`ai_core::heavy`]); `step` stops at a check that fails (`StepEnd::HeavyFailed`). The twin
/// is built like the app, so `configure` and `setup` are `Fn`.
pub fn heavy_app(
    configure: impl Fn(&mut EngineConfig) + Send + Sync + 'static,
    setup: impl Fn(&mut App) + Send + Sync + 'static,
    heavy: HeavyConfig,
) -> App {
    let (config, build) = heavy_game(configure, setup, heavy);
    let mut app = heavy_unloaded(&config, build);
    if let Err(e) = wait_until_loaded(&mut app, LOAD_TIMEOUT) {
        panic!("{e}");
    }
    app
}

/// What [`verify_heavy`] found.
#[derive(Clone, Debug)]
pub struct HeavyVerdict {
    /// How the playback compares with the recording.
    pub report: ReplayReport,
    /// Heavy checks that passed on the way.
    pub checked: u64,
    /// Heavy points that could not be checked (a load restarted the session, or the journal
    /// lost lines before they were compared).
    pub skipped: u64,
}

/// [`verify`] in heavy mode: plays `recording` on the game `configure` and `setup` build (with
/// the recording's seed and tick rate), a twin checking every `heavy.every` ticks. A failed
/// heavy check is an error, like a violated invariant.
pub fn verify_heavy(
    recording: Replay,
    configure: impl Fn(&mut EngineConfig) + Send + Sync + 'static,
    setup: impl Fn(&mut App) + Send + Sync + 'static,
    heavy: HeavyConfig,
) -> Result<HeavyVerdict, String> {
    let (seed, hz) = (recording.seed, recording.tick_hz);
    let configure = move |config: &mut EngineConfig| {
        configure(config);
        config.seed = seed;
        config.tick_hz = hz;
    };
    let (config, build) = heavy_game(configure, setup, heavy);
    let mut app = heavy_unloaded(&config, build);
    let report = verify(&mut app, recording)?;
    let state = app.world().resource::<ai_core::heavy::AiHeavy>();
    Ok(HeavyVerdict { report, checked: state.checked(), skipped: state.skipped() })
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
        Ok(ai_core::scenario::outcome_of(out))
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

// --- fuzzing with shrinking ----------------------------------------------------------

/// A fuzz run that stopped on something, shrunk to the few rounds that still stop on it.
#[derive(Clone, Debug)]
pub struct Finding {
    pub stop: shards_script::stop::Stop,
    /// The game's own words for it: the halt reason, or the refused call and its answer.
    pub text: String,
    /// The rounds as played.
    pub before: Vec<shards_script::fuzz::Round>,
    /// What is left. Equal to `before` when the session did not stop again on a fresh game
    /// (`reproduced` is `false`), so there was nothing to shrink with.
    pub shrunk: shards_script::shrink::Shrunk,
    /// A fresh game stopped on the same thing when it played all of `before`.
    pub reproduced: bool,
}

impl Finding {
    /// The scenario file for the shrunk session, started with game seed `seed`; `heavy` is the heavy
    /// mode the run had on, as `(every, span, offset)`.
    pub fn scenario(&self, seed: u64, heavy: Option<(u64, u64, u64)>) -> String {
        let strict = match &self.stop {
            shards_script::stop::Stop::Chaos { tool } => Some(tool.as_str()),
            _ => None,
        };
        let title = self.stop.title();
        let header = shards_script::regress::Header { seed, stop: &self.text, stop_kind: &title, before: &self.before, heavy, strict };
        shards_script::regress::scenario_text(&header, &self.shrunk.rounds)
    }

    /// `<YYYYMMDD>-<stop>-seed<S>.scenario`.
    pub fn file_name(&self, date: &str, seed: u64) -> String {
        shards_script::regress::file_name(date, &self.stop.slug(), seed)
    }
}

/// What [`fuzz_and_shrink`] did.
#[derive(Debug)]
pub struct Fuzzed {
    pub report: shards_script::fuzz::FuzzReport,
    /// Set when the run stopped on a halt or a broken chaos round trip.
    pub finding: Option<Finding>,
}

/// Plays `plan` on the game `build` makes (a test app, or a heavy one) and, if it stops on a
/// broken rule, a failing system, a heavy check or a chaos round trip that did not come back, shrinks
/// the session: each attempt plays candidate rounds on a **fresh** game from `build` (the same seed,
/// the same heavy mode) and counts when it stops on the same thing
/// ([`Stop::same_as`](shards_script::stop::Stop::same_as)). At most `budget` attempts; the first
/// one plays the whole session again, to see that a fresh game stops on it at all.
pub fn fuzz_and_shrink(mut build: impl FnMut() -> App, plan: &shards_script::fuzz::FuzzPlan, budget: u32) -> Result<Fuzzed, String> {
    use shards_script::{fuzz, shrink, stop::Stop};
    let mut app = build();
    let tools = ai_core::scenario::tool_list(app.world());
    let report = fuzz::run(&mut ai_core::scenario::WorldDriver { world: app.world_mut() }, plan, &tools)?;
    let Some(stop) = Stop::of(&report.ended) else {
        return Ok(Fuzzed { report, finding: None });
    };
    let text = match &report.ended {
        fuzz::Ended::Halted { reason, .. } => reason.clone(),
        fuzz::Ended::ChaosBroke { call, why } => format!("{call} was refused: {why}"),
        _ => String::new(),
    };
    let mut attempt = |rounds: &[fuzz::Round]| {
        let mut fresh = build();
        shrink::replays_stop(&mut ai_core::scenario::WorldDriver { world: fresh.world_mut() }, &plan.agent, rounds, &stop)
    };
    let before = report.played.clone();
    let reproduced = attempt(&before)?;
    let shrunk = match reproduced {
        true => shrink::shrink(&before, budget.saturating_sub(1), &mut attempt)?,
        false => shrink::Shrunk { rounds: before.clone(), attempts: 1, exhausted: false },
    };
    let shrunk = shrink::Shrunk { attempts: shrunk.attempts + u32::from(reproduced), ..shrunk };
    Ok(Fuzzed { report, finding: Some(Finding { stop, text, before, shrunk, reproduced }) })
}
