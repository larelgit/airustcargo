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
//! (and untagged ones). The handler cannot capture its app (it is a plain `fn`, and a system runs on
//! a task-pool thread shared by every app), so [`handler_for`] binds one of `SLOTS` (256) monomorphized
//! copies to each live app and frees it when the app is dropped. An app past the last slot gets a
//! handler whose failures are logged and queued for nobody: tagging them with the "active" app, as the
//! fallback once did, handed the failure to whichever other app happened to be active, which halted an
//! unrelated test of the same process (`cargo test` runs tests as threads of one process; nextest, a
//! process per test, never showed it).

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
    if token == ORPHAN {
        error!(target: LOG_TARGET, "that failure belongs to no app (no handler slot left, or its app is gone): it halts nothing");
        return;
    }
    if let Ok(mut queue) = QUEUE.lock() {
        queue.push(Queued { token, kind, name, message, panicked });
    }
}

/// How many apps of one process get a handler bound to them. The handler cannot know its app
/// any other way (it is a plain `fn`, and a system runs on a shared task-pool thread), so an
/// app past this many live ones gets [`orphan_handler`], never another app's token. A game has
/// a handful of apps (a stand: a host and its guests), a test binary one per running test
/// thread and stand, a few hundred at most.
const SLOTS: usize = 256;

/// The token of a failure no app owns: nobody collects it (see [`orphan_handler`]).
const ORPHAN: u64 = u64::MAX;

/// Slot → the token of the app that holds it (0: free).
static SLOT_TOKENS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

fn slot_handler<const S: usize>(error: BevyError, ctx: ErrorContext) {
    match SLOT_TOKENS[S].load(Ordering::Relaxed) {
        // The slot was freed (its app is gone): the failure belongs to nobody.
        0 => handle(error, ctx, ORPHAN),
        token => handle(error, ctx, token),
    }
}

/// Handler of an app that got no slot: logs the failure (as every handler does) and keeps it
/// out of the queue, since any token it could use would be another app's.
fn orphan_handler(error: BevyError, ctx: ErrorContext) {
    handle(error, ctx, ORPHAN);
}

/// 16 consecutive slot handlers starting at `$k * 16`.
macro_rules! slot_row {
    ($k:literal) => {
        [
            slot_handler::<{ $k * 16 }> as ErrorHandler,
            slot_handler::<{ $k * 16 + 1 }>,
            slot_handler::<{ $k * 16 + 2 }>,
            slot_handler::<{ $k * 16 + 3 }>,
            slot_handler::<{ $k * 16 + 4 }>,
            slot_handler::<{ $k * 16 + 5 }>,
            slot_handler::<{ $k * 16 + 6 }>,
            slot_handler::<{ $k * 16 + 7 }>,
            slot_handler::<{ $k * 16 + 8 }>,
            slot_handler::<{ $k * 16 + 9 }>,
            slot_handler::<{ $k * 16 + 10 }>,
            slot_handler::<{ $k * 16 + 11 }>,
            slot_handler::<{ $k * 16 + 12 }>,
            slot_handler::<{ $k * 16 + 13 }>,
            slot_handler::<{ $k * 16 + 14 }>,
            slot_handler::<{ $k * 16 + 15 }>,
        ]
    };
}

/// The handler is a plain `fn` that cannot capture its app, but Bevy reads it per world:
/// one monomorphized copy per slot knows its app's token, whatever thread the failing
/// system ran on and whichever app was activated last. 16 rows of 16.
static SLOT_HANDLERS: [[ErrorHandler; 16]; SLOTS / 16] = [
    slot_row!(0),
    slot_row!(1),
    slot_row!(2),
    slot_row!(3),
    slot_row!(4),
    slot_row!(5),
    slot_row!(6),
    slot_row!(7),
    slot_row!(8),
    slot_row!(9),
    slot_row!(10),
    slot_row!(11),
    slot_row!(12),
    slot_row!(13),
    slot_row!(14),
    slot_row!(15),
];

fn slot_handler_at(slot: usize) -> ErrorHandler {
    SLOT_HANDLERS[slot / 16][slot % 16]
}

/// Frees the app's handler slot when its world is dropped.
#[derive(Resource)]
struct HandlerSlot(usize);

impl Drop for HandlerSlot {
    fn drop(&mut self) {
        SLOT_TOKENS[self.0].store(0, Ordering::Relaxed);
    }
}

/// An error handler that queues this world's failures under its own token: a free slot's
/// copy, or [`orphan_handler`] (failures logged, collected by nobody) when all slots are taken.
pub fn handler_for(world: &mut World) -> ErrorHandler {
    let token = crate::process::ensure_token(world);
    if let Some(slot) = world.get_resource::<HandlerSlot>() {
        return slot_handler_at(slot.0);
    }
    if let Some(i) = claim_slot(&SLOT_TOKENS, token) {
        world.insert_resource(HandlerSlot(i));
        return slot_handler_at(i);
    }
    error!(target: LOG_TARGET, "all {SLOTS} handler slots are taken: failures of this app are logged but will not halt it");
    orphan_handler
}

/// The first free slot of `table`, now held by `token`.
fn claim_slot(table: &[AtomicU64], token: u64) -> Option<usize> {
    table.iter().position(|slot| slot.compare_exchange(0, token, Ordering::Relaxed, Ordering::Relaxed).is_ok())
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

/// The crash replay and the crash report of a failure, once per failing system (again at its next
/// failure when the report could not be written), while the game writes them. What the panic hook
/// kept of the panic (where, the call stack) goes into the report.
fn save_crash_files(world: &mut World, failure: &mut SystemFailure) {
    let (id, kept) = failure.panicked.then(|| crash::reported(&failure.message)).flatten().unzip();
    let key = format!("{} {}", failure.kind, failure.name);
    let Some(mut state) = world.get_resource_mut::<AiFailures>() else {
        return;
    };
    if !state.crash_replays || !state.saved.insert(key.clone()) {
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
    report.claimed.extend(id);
    failure.crash_report = crash::save(world, &report);
    if failure.crash_report.is_none() {
        // No report: the system's next failure tries again.
        if let Some(mut state) = world.get_resource_mut::<AiFailures>() {
            state.saved.remove(&key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin).add_plugins(crate::AiCorePlugin {
            remote_port: None,
            crash_replays: false,
            sim: crate::SimConfig { start_paused: true, ..Default::default() },
            ..Default::default()
        });
        app.finish();
        app.cleanup();
        app
    }

    /// A system failure of `app`, raised on a thread of its own (as a task-pool thread would).
    fn fail_on_another_thread(app: &App) {
        let handler = app.world().resource::<FallbackErrorHandler>().0;
        std::thread::spawn(move || {
            let ctx = ErrorContext::System { name: DebugName::borrowed("planted::boom"), last_run: bevy::ecs::change_detection::Tick::new(0) };
            handler(BevyError::new(Severity::Panic, "boom"), ctx);
        })
        .join()
        .unwrap();
    }

    /// The live apps this test holds stay far below `SLOTS`: it runs beside tests that build apps
    /// of their own, and a test that used up the slots would give those the orphan handler.
    #[test]
    fn a_failure_reaches_only_its_own_app_whichever_thread_raised_it_and_whichever_app_is_active() {
        // More live apps than the old 32 slots, several threads building beside the failing one.
        let mut apps: Vec<App> = (0..40).map(|_| game()).collect();
        let busy: Vec<_> = (0..4).map(|_| std::thread::spawn(|| (0..4).map(|_| game()).collect::<Vec<_>>().len())).collect();
        let last = apps.len() - 1;
        for victim in [0, last] {
            // A bystander is the active app of the process.
            let bystander = 1 + victim % 2 * 10;
            crate::process::activate(apps[bystander].world());
            fail_on_another_thread(&apps[victim]);
            for (i, app) in apps.iter_mut().enumerate() {
                assert_eq!(!collect(app.world_mut()).is_empty(), i == victim, "app {i}, victim {victim}");
            }
        }
        for thread in busy {
            thread.join().unwrap();
        }
        // A freed slot is used again, with the new app's token.
        drop(apps.swap_remove(0));
        let mut again = game();
        fail_on_another_thread(&again);
        assert_eq!(collect(again.world_mut()).len(), 1);
    }

    /// An app past the last slot: its failure is nobody's, whichever app is active. The slot table
    /// here is a small local one, so no other test is starved of real slots.
    #[test]
    fn an_app_without_a_slot_gets_the_orphan_handler_and_its_failure_halts_no_other_app() {
        let table = [const { AtomicU64::new(0) }; 2];
        assert_eq!(claim_slot(&table, 7), Some(0));
        assert_eq!(claim_slot(&table, 8), Some(1));
        assert_eq!(claim_slot(&table, 9), None, "a full table gives no slot");
        table[0].store(0, Ordering::Relaxed);
        assert_eq!(claim_slot(&table, 9), Some(0), "a freed slot is reused");

        let mut bystander = game();
        crate::process::activate(bystander.world());
        std::thread::spawn(|| {
            let ctx = ErrorContext::System { name: DebugName::borrowed("planted::boom"), last_run: bevy::ecs::change_detection::Tick::new(0) };
            orphan_handler(BevyError::new(Severity::Panic, "boom"), ctx);
        })
        .join()
        .unwrap();
        assert!(collect(bystander.world_mut()).is_empty());
        assert!(QUEUE.lock().unwrap().iter().all(|q| q.token != ORPHAN));
    }
}
