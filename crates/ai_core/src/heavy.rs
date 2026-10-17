//! Heavy mode: at regular points of a session, a fresh game loaded from a snapshot of the
//! session must continue exactly like the session does.
//!
//! A load already checks itself: it saves the restored world again and compares
//! ([`snapshot`](crate::snapshot)). But it does so in the same world, where state the snapshot
//! does not hold (a resource without `add_snapshot_resource`, a `Local` in a gameplay system,
//! a cache `add_snapshot_rebuild` does not build) survives the load untouched, and the bug
//! hides. Only a game built afresh has none of it. So, every `every` ticks from `offset`:
//!
//! 1. at tick t, the snapshot S(t) of this game (A) is kept in memory, with where its replay
//!    recorder, its rules and its journal stand;
//! 2. A plays on to t + span as it would anyway: a replay, a scenario, fuzz inputs;
//! 3. at t + span a twin B is built by the game's own builder ([`HeavyTwin`]), loads, puts
//!    S(t) in and plays what A's recorder holds since S(t): a replay from a snapshot;
//! 4. B's snapshot at t + span must be A's, field for field, its state hash A's, and its
//!    gameplay lines since S(t) (events, warnings, errors) A's.
//!
//! A difference stops the game like a violated invariant (`stopped: heavy check failed at
//! tick T: …`), saves S(t), both snapshots at T and the session's replay, and the check stays
//! off until `assert_reset`. The twin is built and played between ticks, outside A's
//! schedule, where `step` (and a stand's driver) collects a tick's failures: heavy mode checks
//! stepped time, and `resume` refuses.
//!
//! Off (the default) none of this exists: no resource, nothing in the journal, the replay or
//! a snapshot.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use serde::Serialize;

use crate::invariants::{self, RulesState};
use crate::journal::{self, AiJournal, JournalKind};
use crate::replay::{self, AiReplay, Check, Replay, ReplayHashing};
use crate::sim::{self, Halt, StepEnd};
use crate::snapshot::{self, Snapshot, SNAPSHOTS_DIR};

/// The longest span of a heavy point, in ticks.
pub const MAX_SPAN: u64 = 600;

/// Differences a failed check writes into the journal, a line each.
const MAX_LISTED: usize = 10;

/// When heavy points start and how long each runs (`--heavy N`, `--heavy-span K`,
/// `--heavy-offset O`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeavyConfig {
    /// A point starts every this many ticks.
    pub every: u64,
    /// Ticks the original plays on before a twin is compared with it (1 to [`MAX_SPAN`]).
    pub span: u64,
    /// The tick of the first point.
    pub offset: u64,
}

impl HeavyConfig {
    /// A point every `every` ticks from tick 0, each `every` ticks long (at most [`MAX_SPAN`]).
    pub fn every(every: u64) -> Self {
        Self { every, span: every.clamp(1, MAX_SPAN), offset: 0 }
    }

    /// Why the game cannot run with this config, if it cannot.
    pub fn check(&self) -> Result<(), String> {
        if self.every == 0 {
            return Err("--heavy needs at least 1 tick between heavy points".into());
        }
        if self.span == 0 || self.span > MAX_SPAN {
            return Err(format!("--heavy-span is 1 to {MAX_SPAN} ticks, not {}", self.span));
        }
        Ok(())
    }

    /// Whether a point starts at `tick`.
    fn starts_at(&self, tick: u64) -> bool {
        tick >= self.offset && (tick - self.offset).is_multiple_of(self.every)
    }

    /// The flags that check the same points: `--heavy 30 --heavy-span 30 --heavy-offset 0`.
    pub fn flags(&self) -> String {
        format!("--heavy {} --heavy-span {} --heavy-offset {}", self.every, self.span, self.offset)
    }
}

/// How heavy mode builds a twin: the game's own builder. `shards::run` and the test apps of
/// `shards::testing` put it in; a game without it runs no heavy checks, and says so.
#[derive(Resource, Clone)]
pub struct HeavyTwin {
    build: Arc<dyn Fn() -> App + Send + Sync>,
    wait: Arc<dyn Fn(&mut App) -> Result<(), String> + Send + Sync>,
}

impl HeavyTwin {
    /// `build` makes a twin: the app the original is, finished, with nothing played, headless,
    /// paused and writing no files, its own heavy mode off. `wait` updates an app until it has
    /// loaded, or says why it has not.
    pub fn new(
        build: impl Fn() -> App + Send + Sync + 'static,
        wait: impl Fn(&mut App) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self { build: Arc::new(build), wait: Arc::new(wait) }
    }
}

/// A heavy check that failed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HeavyFailure {
    /// The tick of the snapshot the twin started from.
    pub from: u64,
    /// The tick the two games were compared at.
    pub tick: u64,
    /// What differs, original first: `e6 Boss: Health.current differs: 390.0 vs 400.0`.
    /// Never empty.
    pub differences: Vec<String>,
    /// The session's replay, saved for it, relative to the game's working directory.
    pub replay: Option<String>,
}

impl HeavyFailure {
    /// `heavy check failed at tick 90`
    pub fn headline(&self) -> String {
        format!("heavy check failed at tick {}", self.tick)
    }

    /// The first difference.
    pub fn first(&self) -> &str {
        self.differences.first().map_or("", String::as_str)
    }
}

/// Heavy mode's state (present only when it is on). Outside the simulation: not in snapshots,
/// replays or the state hash.
#[derive(Resource)]
pub struct AiHeavy {
    config: HeavyConfig,
    /// Save the snapshots and the replay of a failed check (off in test apps).
    files: bool,
    open: Vec<Point>,
    /// The recorder's generation and the tick [`at_tick`] last ran at.
    last: Option<(u64, u64)>,
    failures: Vec<HeavyFailure>,
    reported: usize,
    /// The tick a check failed at: no checks until `assert_reset`.
    fired: Option<u64>,
    /// Why there are no checks at all in this game, once said.
    off: Option<String>,
    checked: u64,
    skipped: u64,
    twin_time: Duration,
}

impl AiHeavy {
    pub fn new(config: HeavyConfig, files: bool) -> Self {
        Self {
            config,
            files,
            open: Vec::new(),
            last: None,
            failures: Vec::new(),
            reported: 0,
            fired: None,
            off: None,
            checked: 0,
            skipped: 0,
            twin_time: Duration::ZERO,
        }
    }

    pub fn config(&self) -> HeavyConfig {
        self.config
    }

    /// Every failed check this session, oldest first.
    pub fn failures(&self) -> &[HeavyFailure] {
        &self.failures
    }

    /// Checks that passed.
    pub fn checked(&self) -> u64 {
        self.checked
    }

    /// Points that could not be checked (the game was loading, a load restarted the session).
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// `heavy: every 30 ticks (span 30, from tick 0): 12 checked, 1 skipped, armed`, for
    /// `assert_list`.
    pub fn summary(&self) -> String {
        let HeavyConfig { every, span, offset } = self.config;
        let state = match (&self.off, self.fired) {
            (Some(why), _) => format!("off: {why}"),
            (None, Some(tick)) => format!("fired at tick {tick}, off until assert_reset"),
            (None, None) => "armed".to_owned(),
        };
        format!("heavy: every {every} ticks (span {span}, from tick {offset}): {} checked, {} skipped, {state}", self.checked, self.skipped)
    }
}

/// One heavy point: S(t) and what the original did since.
struct Point {
    tick: u64,
    end: u64,
    generation: u64,
    start: Snapshot,
    /// The recorder's length at S(t): its events from here on are the twin's.
    mark: usize,
    rules: RulesState,
    /// The components the replay checks had reported as left out by S(t).
    left_out: Vec<&'static str>,
    /// The original's journal: read up to here.
    seq: u64,
    /// Its gameplay lines since S(t).
    lines: Vec<String>,
    /// Lines that left the ring before they were read (then the journals are not compared).
    lost: u64,
}

pub(crate) fn plugin(app: &mut App, config: Option<HeavyConfig>, files: bool) {
    if let Some(config) = config {
        app.insert_resource(AiHeavy::new(config, files));
    }
}

/// Whether this game runs heavy checks (`--heavy`).
pub fn is_on(world: &World) -> bool {
    world.contains_resource::<AiHeavy>()
}

/// Time spent on twins so far: `step` leaves it out of its wall-clock budget.
pub fn twin_time(world: &World) -> Duration {
    world.get_resource::<AiHeavy>().map_or(Duration::ZERO, |h| h.twin_time)
}

/// Failed checks since the last call, for the `step` that is running (or a stand's driver).
pub fn take_unreported(world: &mut World) -> Vec<HeavyFailure> {
    let Some(mut heavy) = world.get_resource_mut::<AiHeavy>() else {
        return Vec::new();
    };
    let new = heavy.failures[heavy.reported..].to_vec();
    heavy.reported = heavy.failures.len();
    new
}

/// Turns the checks back on after one failed (`assert_reset`). Whether they were off.
pub(crate) fn rearm(world: &mut World) -> bool {
    world.get_resource_mut::<AiHeavy>().is_some_and(|mut heavy| heavy.fired.take().is_some())
}

/// What `step` and a stand's driver call between two ticks. `after_tick`: right after a tick
/// ran (and its failures were collected), when a point that ends here is checked; before the
/// first tick of a step (`false`) a point can only start. Starts the point of this tick, once.
/// Nothing without heavy mode.
pub fn at_tick(world: &mut World, after_tick: bool) {
    let Some(heavy) = world.get_resource::<AiHeavy>() else {
        return;
    };
    let tick = sim::tick(world);
    let generation = world.get_resource::<AiReplay>().map_or(0, AiReplay::generation);
    if heavy.last == Some((generation, tick)) {
        return;
    }
    world.resource_mut::<AiHeavy>().last = Some((generation, tick));
    read_journal(world);
    // A load restarted the recording, or ticks ran past the hook: those points cannot be
    // checked.
    drop_points(world, "the session was restarted from a snapshot or a save", |p| p.generation != generation);
    drop_points(world, "ticks ran past their end without a check", |p| p.end < tick || (!after_tick && p.end == tick));
    if after_tick && crate::failures::failed_this_tick(world) {
        drop_points(world, "a gameplay system failed", |_| true);
        return;
    }
    let mut heavy = world.resource_mut::<AiHeavy>();
    let (due, open): (Vec<Point>, Vec<Point>) = std::mem::take(&mut heavy.open).into_iter().partition(|p| p.end == tick);
    heavy.open = open;
    for point in due {
        if world.resource::<AiHeavy>().fired.is_some() {
            world.resource_mut::<AiHeavy>().skipped += 1;
            continue;
        }
        check(world, point);
    }
    let heavy = world.resource::<AiHeavy>();
    if heavy.fired.is_none() && heavy.off.is_none() && heavy.config.starts_at(tick) {
        start(world, tick, generation);
    }
}

/// Says once why this game runs no heavy checks.
fn turn_off(world: &mut World, why: &str) {
    let mut heavy = world.resource_mut::<AiHeavy>();
    if heavy.off.is_some() {
        return;
    }
    heavy.off = Some(why.to_owned());
    heavy.open.clear();
    journal::log(world, JournalKind::Warn, format!("heavy mode is on, but {why}: no heavy checks"));
}

fn drop_points(world: &mut World, why: &str, drop: impl Fn(&Point) -> bool) {
    let mut heavy = world.resource_mut::<AiHeavy>();
    let ticks: Vec<String> = heavy.open.iter().filter(|p| drop(p)).map(|p| p.tick.to_string()).collect();
    if ticks.is_empty() {
        return;
    }
    heavy.open.retain(|p| !drop(p));
    heavy.skipped += ticks.len() as u64;
    let points = if ticks.len() == 1 { "point" } else { "points" };
    journal::log(world, JournalKind::Sim, format!("heavy {points} from tick {} not checked: {why}", ticks.join(", ")));
}

/// The gameplay lines of the journal since each open point last read it.
fn read_journal(world: &mut World) {
    let world_ref: &World = world;
    let Some(journal) = world_ref.get_resource::<AiJournal>() else {
        return;
    };
    let mut read = Vec::new();
    for point in &world_ref.resource::<AiHeavy>().open {
        let (entries, missed) = journal.after(point.seq);
        let lines: Vec<String> = entries.filter(|e| gameplay(e.kind)).map(ToString::to_string).collect();
        read.push((lines, missed));
    }
    let last = journal.last_seq();
    let mut heavy = world.resource_mut::<AiHeavy>();
    for (point, (lines, missed)) in heavy.open.iter_mut().zip(read) {
        point.lines.extend(lines);
        point.lost += missed;
        point.seq = last;
    }
}

/// The kinds of line a twin must write as the original did.
fn gameplay(kind: JournalKind) -> bool {
    matches!(kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error)
}

/// Point t: S(t) and where the recorder, the rules and the journal stand.
fn start(world: &mut World, tick: u64, generation: u64) {
    let recording = world.get_resource::<AiReplay>().is_some_and(AiReplay::is_recording);
    if !recording {
        turn_off(world, "this game does not record its session (a co-op client: the host checks it)");
        return;
    }
    if !world.contains_resource::<HeavyTwin>() {
        turn_off(world, "this game has no builder for a twin (run it with shards::run, or test it with shards::testing::heavy_app)");
        return;
    }
    if let Some(loading) = sim::gate_message(world) {
        world.resource_mut::<AiHeavy>().skipped += 1;
        journal::log(world, JournalKind::Sim, format!("heavy point at tick {tick} not checked: {loading}"));
        return;
    }
    let start = match snapshot::capture(world) {
        Ok(start) => start,
        Err(why) => {
            world.resource_mut::<AiHeavy>().skipped += 1;
            journal::log(world, JournalKind::Sim, format!("heavy point at tick {tick} not checked: {why}"));
            return;
        }
    };
    let point = Point {
        tick,
        end: tick + world.resource::<AiHeavy>().config.span,
        generation,
        start,
        mark: world.resource::<AiReplay>().recorded_events(),
        rules: invariants::rules_state(world),
        left_out: world.get_resource::<ReplayHashing>().map(ReplayHashing::reported_left_out).unwrap_or_default(),
        seq: world.get_resource::<AiJournal>().map_or(0, AiJournal::last_seq),
        lines: Vec::new(),
        lost: 0,
    };
    world.resource_mut::<AiHeavy>().open.push(point);
}

/// What the twin came to.
struct TwinRun {
    state: Snapshot,
    lines: Vec<String>,
    lost: u64,
}

/// The point ends here: a twin plays it, and must come to the same state.
fn check(world: &mut World, point: Point) {
    let tick = sim::tick(world);
    let original = match snapshot::capture(world) {
        Ok(original) => original,
        Err(why) => {
            world.resource_mut::<AiHeavy>().skipped += 1;
            journal::log(world, JournalKind::Sim, format!("heavy point from tick {} not checked: {why}", point.tick));
            return;
        }
    };
    let hash = replay::state_hash(world);
    let recorder = world.resource::<AiReplay>();
    let (check_every, check_detail) = recorder.check_settings();
    let events = recorder.events_from(point.mark).to_vec();
    debug_assert!(
        events.iter().all(|e| (point.tick..tick).contains(&e.at())),
        "heavy: the events since tick {} reach outside ticks {}..{tick}",
        point.tick,
        point.tick
    );
    let recording = Replay {
        format: replay::FORMAT.to_owned(),
        seed: point.start.seed,
        tick_hz: point.start.tick_hz,
        check_every,
        check_detail,
        start: Some(point.start.clone()),
        end: Check { tick, hash, parts: None },
        events,
        checks: Vec::new(),
    };
    let twin = world.resource::<HeavyTwin>().clone();
    let started = Instant::now();
    let run = catch_unwind(AssertUnwindSafe(|| play_twin(&twin, recording, &point, tick))).unwrap_or_else(|panic| {
        let message = crate::crash::panic_text(panic.as_ref());
        crate::crash::handled(&message);
        Err(format!("the twin panicked: {message}"))
    });
    crate::process::activate(world);
    world.resource_mut::<AiHeavy>().twin_time += started.elapsed();
    let (found, restored) = match run {
        Ok(run) => {
            let mut found = snapshot::differences(&original, &run.state);
            if let [only] = found.as_mut_slice()
                && only.starts_with("state_hash ")
            {
                only.push_str(" (the state is the same: a replay check outside the snapshot, add_replay_check, did not come back)");
            }
            if point.lost == 0
                && run.lost == 0
                && let Some(line) = journal_difference(point.tick, &point.lines, &run.lines)
            {
                found.push(line);
            }
            (found, Some(run.state))
        }
        Err(why) => (vec![why], None),
    };
    if found.is_empty() {
        world.resource_mut::<AiHeavy>().checked += 1;
        return;
    }
    fail(world, &point, &original, restored.as_ref(), found);
}

/// Builds a twin, puts S(t) in and plays `recording` to tick `end`.
fn play_twin(twin: &HeavyTwin, recording: Replay, point: &Point, end: u64) -> Result<TwinRun, String> {
    // The twin's engine lines (`content loaded`, `navmesh: …`) are its journal's, not the terminal's.
    let _quiet = crate::process::quiet();
    // Until it has a token of its own, what the twin logs would land in this game's journal.
    let mut app = {
        let _muted = crate::process::mute();
        (twin.build)()
    };
    crate::process::activate(app.world());
    replay::play(app.world_mut(), recording).map_err(|why| format!("the twin cannot play from tick {}: {why}", point.tick))?;
    (twin.wait)(&mut app).map_err(|why| format!("the twin did not load: {why}"))?;
    // The start goes in once the game has loaded, in the frame that saw it done.
    for _ in 0..3 {
        if app.world().resource::<AiReplay>().started_from().is_some() {
            break;
        }
        app.update();
    }
    let replay = app.world().resource::<AiReplay>();
    if let Some(report) = replay.report().filter(|r| r.interrupted.is_some()) {
        return Err(format!("the twin cannot start from the snapshot of tick {}: {report}", point.tick));
    }
    if replay.started_from().is_none() {
        return Err(format!("the twin did not put in the snapshot of tick {}", point.tick));
    }
    if let Some(why) = replay.playback_divergence() {
        return Err(format!("the snapshot of tick {} does not load exactly into a fresh game: {why}", point.tick));
    }
    invariants::set_rules_state(app.world_mut(), &point.rules);
    app.world_mut().get_resource_or_init::<ReplayHashing>().mark_reported(&point.left_out);
    let mut seq = app.world().resource::<AiJournal>().last_seq();
    let (mut lines, mut lost) = (Vec::new(), 0);
    loop {
        let tick = sim::tick(app.world());
        if tick >= end {
            break;
        }
        if sim::gate_message(app.world()).is_some() {
            (twin.wait)(&mut app).map_err(|why| format!("the twin did not load at tick {tick}: {why}"))?;
            continue;
        }
        let report = sim::step(app.world_mut(), 1, Duration::from_secs(3600), |_| None)?;
        let journal = app.world().resource::<AiJournal>();
        let (entries, missed) = journal.after(seq);
        lines.extend(entries.filter(|e| gameplay(e.kind)).map(ToString::to_string));
        lost += missed;
        seq = journal.last_seq();
        if let StepEnd::SystemFailed(failures) = &report.end {
            let first = &failures[0];
            return Err(format!("the twin's gameplay {}: {}", first.headline(), first.message));
        }
        if report.ticks() == 0 && sim::gate_message(app.world()).is_none() {
            return Err(format!("the twin stopped at tick {tick}"));
        }
    }
    let state = snapshot::capture(app.world()).map_err(|why| format!("the twin's state cannot be saved: {why}"))?;
    Ok(TwinRun { state, lines, lost })
}

/// The first gameplay line where the twin's journal since tick `from` is not the original's.
fn journal_difference(from: u64, original: &[String], twin: &[String]) -> Option<String> {
    let i = original.iter().zip(twin).position(|(a, b)| a != b).unwrap_or(original.len().min(twin.len()));
    match (original.get(i), twin.get(i)) {
        (Some(a), Some(b)) => Some(format!("journal since tick {from}, line {}, differs: `{a}` vs `{b}`", i + 1)),
        (Some(a), None) => Some(format!("journal since tick {from}: `{a}` only in the original")),
        (None, Some(b)) => Some(format!("journal since tick {from}: `{b}` only in the twin")),
        (None, None) => None,
    }
}

/// The check failed: the journal lists what differs, the files are saved, the game halts.
fn fail(world: &mut World, point: &Point, original: &Snapshot, restored: Option<&Snapshot>, found: Vec<String>) {
    let tick = original.tick;
    let replay = if world.resource::<AiHeavy>().files { save_files(world, point, original, restored) } else { None };
    let failure = HeavyFailure { from: point.tick, tick, differences: found, replay };
    let count = failure.differences.len();
    let differences = if count == 1 { "1 difference".to_owned() } else { format!("{count} differences") };
    journal::log(
        world,
        JournalKind::Error,
        format!("{} (a fresh game from tick {}, {differences}): {}", failure.headline(), failure.from, failure.first()),
    );
    for line in failure.differences.iter().skip(1).take(MAX_LISTED - 1) {
        journal::log(world, JournalKind::Error, format!("heavy: {line}"));
    }
    if count > MAX_LISTED {
        journal::log(world, JournalKind::Error, format!("heavy: … and {} more", count - MAX_LISTED));
    }
    let mut heavy = world.resource_mut::<AiHeavy>();
    heavy.fired = Some(tick);
    heavy.failures.push(failure.clone());
    let others: Vec<String> = heavy.open.drain(..).map(|p| p.tick.to_string()).collect();
    heavy.skipped += others.len() as u64;
    if !others.is_empty() {
        let text = format!("heavy points from tick {} not checked: a check failed", others.join(", "));
        journal::log(world, JournalKind::Sim, text);
    }
    sim::halt(world, Halt { reason: failure.headline(), replay: failure.replay.clone() });
}

/// `snapshots/heavy-tick<t>.json`, `…-tick<T>-original.json`, `…-tick<T>-restored.json` and
/// the session's replay `replays/heavy-tick<T>.json`; the replay's path.
fn save_files(world: &mut World, point: &Point, original: &Snapshot, restored: Option<&Snapshot>) -> Option<String> {
    let tick = original.tick;
    let snapshots = [
        (format!("{SNAPSHOTS_DIR}/heavy-tick{}.json", point.tick), Some(&point.start)),
        (format!("{SNAPSHOTS_DIR}/heavy-tick{tick}-original.json"), Some(original)),
        (format!("{SNAPSHOTS_DIR}/heavy-tick{tick}-restored.json"), restored),
    ];
    let mut saved = Vec::new();
    for (path, snapshot) in snapshots {
        let Some(snapshot) = snapshot else { continue };
        match snapshot.write(&path) {
            Ok(_) => saved.push(path),
            Err(e) => {
                journal::log(world, JournalKind::Error, format!("heavy: {path} not saved: {e}"));
            }
        }
    }
    let path = format!("{}/heavy-tick{tick}.json", replay::REPLAYS_DIR);
    let session = replay::save(world);
    let flags = world.resource::<AiHeavy>().config.flags();
    match replay::write(&session, &path) {
        Ok(_) => {
            let text = format!(
                "heavy check saved: {path} (play it with --replay {path} --verify {flags}), {}",
                saved.join(", ")
            );
            journal::log(world, JournalKind::Sim, text);
            Some(path)
        }
        Err(e) => {
            journal::log(world, JournalKind::Error, format!("heavy: replay not saved: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_config_checks_its_numbers_and_names_its_points() {
        let config = HeavyConfig::every(30);
        assert_eq!((config.span, config.offset), (30, 0), "the span is N by default");
        assert_eq!(HeavyConfig::every(5000).span, MAX_SPAN, "and never more than {MAX_SPAN} ticks");
        assert!(config.check().is_ok());
        assert!(HeavyConfig::every(0).check().unwrap_err().contains("at least 1 tick"));
        let long = HeavyConfig { span: MAX_SPAN + 1, ..config };
        assert_eq!(long.check().unwrap_err(), format!("--heavy-span is 1 to {MAX_SPAN} ticks, not {}", MAX_SPAN + 1));

        let shifted = HeavyConfig { every: 30, span: 10, offset: 7 };
        let points: Vec<u64> = (0..100).filter(|t| shifted.starts_at(*t)).collect();
        assert_eq!(points, [7, 37, 67, 97]);
        assert_eq!(shifted.flags(), "--heavy 30 --heavy-span 10 --heavy-offset 7");
    }

    #[test]
    fn the_journal_difference_names_the_first_line_that_differs() {
        let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let original = lines(&["e6 hits e5 for 30", "e5 dies"]);
        assert_eq!(journal_difference(60, &original, &original), None);
        assert_eq!(
            journal_difference(60, &original, &lines(&["e6 hits e5 for 30", "e5 lives"])).unwrap(),
            "journal since tick 60, line 2, differs: `e5 dies` vs `e5 lives`"
        );
        assert_eq!(
            journal_difference(60, &original, &lines(&["e6 hits e5 for 30"])).unwrap(),
            "journal since tick 60: `e5 dies` only in the original"
        );
        assert_eq!(journal_difference(60, &[], &lines(&["e5 dies"])).unwrap(), "journal since tick 60: `e5 dies` only in the twin");
    }
}
