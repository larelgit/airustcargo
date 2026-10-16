//! A gameplay system that panics stops the game instead of killing it.
//!
//! In Bevy 0.20 a system panic goes to the [`FallbackErrorHandler`], whose default
//! (`match_severity`) panics again. That panic unwinds through
//! `World::try_schedule_scope` (`FixedMain`, then `FixedUpdate`), which does not put
//! the schedule back: every later `step` fails with "cannot run a tick here" and
//! real-time ticks silently stop, while the agent was told the game kept running.
//!
//! [`AiCorePlugin`](crate::AiCorePlugin) installs [`ai_core_error_handler`] instead. It
//! logs the failure and queues it (the handler is a plain `fn` without the world, so
//! the queue is global). The queue is collected after every tick, once per frame and
//! after every tool call: the failure is journaled (`system <name> panicked at tick N:
//! <msg>`), the game halts (it pauses), the session is saved as a crash
//! replay (`replays/crash-tick<N>.json`, once per failing system) to reproduce it with
//! and as a [crash report](crate::crash) for a bug report (`crashes/…`), and a `step` in
//! progress stops there.
//!
//! Several apps in one process share the queue: each app's handler queues its failures
//! under the app's [token](crate::process::AppToken), and an app collects only its own
//! (and untagged ones).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use bevy::ecs::error::{BevyError, ErrorContext, ErrorHandler, FallbackErrorHandler, Severity};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::crash;
use crate::journal::{self, JournalKind};
use crate::replay;
use crate::sim;

/// Log target of the handler's own `error!` line. The journal gets the failure from
/// [`collect`], so the log capture leaves this target out.
pub(crate) const LOG_TARGET: &str = "ai_core::failures";

/// What the handler saw; the tick is added when it is collected.
struct Queued {
    /// The app that was active when it failed ([`process`](crate::process)); 0: none yet.
    token: u64,
    kind: String,
    name: String,
    message: String,
    panicked: bool,
}

static QUEUE: Mutex<Vec<Queued>> = Mutex::new(Vec::new());

/// A system (observer, command, run condition) that panicked, or returned an error
/// Bevy would have panicked on.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemFailure {
    /// `system`, `observer`, `command` or `run condition`.
    pub kind: String,
    pub name: String,
    pub message: String,
    /// A panic, not a returned error.
    pub panicked: bool,
    /// The tick it happened in (the tick the game was at when it was collected).
    pub tick: u64,
    /// The crash replay saved for it, relative to the game's working directory.
    pub crash_replay: Option<String>,
    /// The crash report saved for it (`crashes/…`, or the player's own folder).
    pub crash_report: Option<String>,
}

impl SystemFailure {
    /// `system game::combat::apply_hits panicked at tick 50`
    pub fn headline(&self) -> String {
        let verb = if self.panicked { "panicked" } else { "failed" };
        format!("{} {} {verb} at tick {}", self.kind, self.name, self.tick)
    }
}

/// [`FallbackErrorHandler`] that never panics: errors Bevy would panic on (system
/// panics included) are logged and queued for [`collect`]; milder ones are logged at
/// their own level, as Bevy does. The failure is queued under the
/// [active](crate::process::active) app; [`AiCorePlugin`](crate::AiCorePlugin) installs a
/// copy bound to its own app instead (see [`handler_for`]).
pub fn ai_core_error_handler(error: BevyError, ctx: ErrorContext) {
    handle(error, ctx, crate::process::active());
}

fn handle(mut error: BevyError, ctx: ErrorContext, token: u64) {
    if error.severity() != Severity::Panic {
        bevy::ecs::error::match_severity(error, ctx);
        return;
    }
    let payload = error.take_payload();
    let panicked = payload.is_some();
    let message = match &payload {
        Some(payload) => crate::tools::panic_message(payload),
        None => error.to_string().lines().next().unwrap_or_default().trim().to_owned(),
    };
    let (kind, name) = (ctx.kind().to_owned(), ctx.name().to_string());
    let verb = if panicked { "panicked" } else { "failed" };
    error!(target: LOG_TARGET, "{kind} `{name}` {verb}: {message}");
    if let Ok(mut queue) = QUEUE.lock() {
        queue.push(Queued { token, kind, name, message, panicked });
    }
}

/// How many apps of one process get a handler bound to them; more fall back to the
/// active app's token.
const SLOTS: usize = 32;

/// Slot → the token of the app that holds it (0: free).
static SLOT_TOKENS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

fn slot_handler<const S: usize>(error: BevyError, ctx: ErrorContext) {
    handle(error, ctx, SLOT_TOKENS[S].load(Ordering::Relaxed));
}

macro_rules! slot_handlers {
    ($($slot:literal)*) => { [$(slot_handler::<$slot> as ErrorHandler),*] };
}

/// The handler is a plain `fn` that cannot capture its app, but Bevy reads it per world:
/// one monomorphized copy per slot knows its app's token, whatever thread the failing
/// system ran on and whichever app was activated last.
static SLOT_HANDLERS: [ErrorHandler; SLOTS] =
    slot_handlers!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31);

/// Frees the app's handler slot when its world is dropped.
#[derive(Resource)]
struct HandlerSlot(usize);

impl Drop for HandlerSlot {
    fn drop(&mut self) {
        SLOT_TOKENS[self.0].store(0, Ordering::Relaxed);
    }
}

/// An error handler that queues this world's failures under its own token: a free slot's
/// copy, or [`ai_core_error_handler`] when all slots are taken.
pub fn handler_for(world: &mut World) -> ErrorHandler {
    let token = crate::process::ensure_token(world);
    if let Some(slot) = world.get_resource::<HandlerSlot>() {
        return SLOT_HANDLERS[slot.0];
    }
    for (i, slot) in SLOT_TOKENS.iter().enumerate() {
        if slot.compare_exchange(0, token, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            world.insert_resource(HandlerSlot(i));
            return SLOT_HANDLERS[i];
        }
    }
    ai_core_error_handler
}

/// Failures collected but not reported by a `step` yet, and crash replay settings.
#[derive(Resource)]
pub struct AiFailures {
    unreported: Vec<SystemFailure>,
    /// Save a crash replay for each newly failing system. `false` in tests.
    pub crash_replays: bool,
    /// Systems that already have a crash replay this session: replaying the first one
    /// reproduces the failure, more copies would only fill the folder.
    saved: HashSet<String>,
    /// A gameplay system failed in the tick that just ran. Set after every tick, so a tick
    /// that runs again after a rewind (a snapshot was loaded) starts clean.
    failed_this_tick: bool,
}

impl AiFailures {
    pub fn new(crash_replays: bool) -> Self {
        Self { unreported: Vec::new(), crash_replays, saved: HashSet::default(), failed_this_tick: false }
    }
}

/// A gameplay system failed in the tick that just ran: its state is half-updated, and the
/// failure already halted the game, so invariants leave that tick alone.
pub(crate) fn failed_this_tick(world: &World) -> bool {
    world.get_resource::<AiFailures>().is_some_and(|f| f.failed_this_tick)
}

impl Default for AiFailures {
    fn default() -> Self {
        Self::new(true)
    }
}

pub(crate) fn plugin(app: &mut App, catch_system_panics: bool, crash_replays: bool) {
    if catch_system_panics {
        let handler = handler_for(app.world_mut());
        app.insert_resource(FallbackErrorHandler(handler));
    }
    app.insert_resource(AiFailures::new(crash_replays))
        .add_systems(FixedLast, collect_after_tick.after(journal::drain_logs))
        .add_systems(Last, collect_between_ticks);
}

/// Right after a tick: the exact tick, and in real time no further tick this frame.
pub(crate) fn collect_after_tick(world: &mut World) {
    let failed = !collect(world).is_empty();
    if let Some(mut state) = world.get_resource_mut::<AiFailures>() {
        state.failed_this_tick = failed;
    }
}

/// This app's failures (and untagged ones); other apps' stay queued for them.
fn take_queue(world: &World) -> Vec<Queued> {
    let own = world.get_resource::<crate::process::AppToken>().map_or(0, |t| t.0);
    let mut queued = Vec::new();
    if let Ok(mut queue) = QUEUE.lock() {
        let (mine, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *queue).into_iter().partition(|q| q.token == own || q.token == 0);
        *queue = others;
        queued = mine;
    }
    // Systems of one tick fail on several threads: keep the journal stable.
    queued.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));
    queued
}

/// Takes the queued failures: journals them, saves a crash replay, pauses the game.
/// Returns what it took (also kept for the `step` running right now).
pub fn collect(world: &mut World) -> Vec<SystemFailure> {
    crate::process::activate(world);
    let queued = take_queue(world);
    if queued.is_empty() {
        return Vec::new();
    }
    let tick = sim::tick(world);
    let mut failures: Vec<SystemFailure> = queued
        .into_iter()
        .map(|q| SystemFailure {
            kind: q.kind,
            name: q.name,
            message: q.message,
            panicked: q.panicked,
            tick,
            crash_replay: None,
            crash_report: None,
        })
        .collect();
    for failure in &mut failures {
        journal::log(world, JournalKind::Error, format!("{}: {}", failure.headline(), failure.message));
        save_crash_files(world, failure);
    }
    if let Some(mut state) = world.get_resource_mut::<AiFailures>() {
        state.unreported.extend(failures.iter().cloned());
    }
    let replay = failures.iter().find_map(|f| f.crash_replay.clone());
    sim::halt(world, sim::Halt { reason: failures[0].headline(), replay });
    failures
}

/// Failures collected since the last call, for the `step` that is running.
pub fn take_unreported(world: &mut World) -> Vec<SystemFailure> {
    world.get_resource_mut::<AiFailures>().map(|mut f| std::mem::take(&mut f.unreported)).unwrap_or_default()
}

/// Once per frame: failures outside ticks (or after this tick's collection). Nobody is
/// stepping here, so they are journaled and the game is paused, nothing more.
pub(crate) fn collect_between_ticks(world: &mut World) {
    collect(world);
    take_unreported(world);
}

/// After a tool call: a failure there (a command the tool queued) is the tool's, not
/// gameplay's. It is journaled with the tool's name; the game keeps running.
pub(crate) fn blame_tool(world: &mut World, tool: &str) {
    for q in take_queue(world) {
        let verb = if q.panicked { "panicked" } else { "failed" };
        journal::log(world, JournalKind::Error, format!("tool {tool}: {} {} {verb}: {}", q.kind, q.name, q.message));
    }
}

/// The crash replay and the crash report of a failure, once per failing system, while the game
/// writes them. What the panic hook kept of the panic (where, the call stack) goes into the report.
fn save_crash_files(world: &mut World, failure: &mut SystemFailure) {
    let kept = failure.panicked.then(|| crash::handled(&failure.message)).flatten();
    let key = format!("{} {}", failure.kind, failure.name);
    let Some(mut state) = world.get_resource_mut::<AiFailures>() else {
        return;
    };
    if !state.crash_replays || !state.saved.insert(key) {
        return;
    }
    let recording = world.get_resource::<replay::AiReplay>().is_some_and(replay::AiReplay::is_recording).then(|| replay::save(world));
    if let Some(recording) = &recording {
        let path = format!("{}/crash-tick{}.json", replay::REPLAYS_DIR, recording.end.tick);
        match replay::write(recording, &path) {
            Ok(_) => {
                journal::log(world, JournalKind::Sim, format!("crash replay saved: {path} (play it with --replay to reproduce)"));
                failure.crash_replay = Some(path);
            }
            Err(e) => {
                journal::log(world, JournalKind::Error, format!("crash replay not saved: {e}"));
            }
        }
    }
    let what = crash::What { headline: failure.headline(), message: failure.message.clone(), ..kept.unwrap_or_default() };
    let mut report = crash::report(world, what, recording);
    report.replay_file = failure.crash_replay.clone();
    failure.crash_report = crash::save(world, &report);
}
