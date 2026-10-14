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
//! <msg>`), the game pauses, the session is saved as a crash replay
//! (`replays/crash-tick<N>.json`, once per failing system) to reproduce it with, and
//! a `step` in progress stops there.

use std::sync::Mutex;

use bevy::ecs::error::{BevyError, ErrorContext, FallbackErrorHandler, Severity};
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::journal::{self, JournalKind};
use crate::replay;
use crate::sim::{self, SimGate};

/// Log target of the handler's own `error!` line. The journal gets the failure from
/// [`collect`], so the log capture leaves this target out.
pub(crate) const LOG_TARGET: &str = "ai_core::failures";

/// What the handler saw; the tick is added when it is collected.
struct Queued {
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
/// their own level, as Bevy does.
pub fn ai_core_error_handler(mut error: BevyError, ctx: ErrorContext) {
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
        queue.push(Queued { kind, name, message, panicked });
    }
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
}

impl AiFailures {
    pub fn new(crash_replays: bool) -> Self {
        Self { unreported: Vec::new(), crash_replays, saved: HashSet::default() }
    }
}

impl Default for AiFailures {
    fn default() -> Self {
        Self::new(true)
    }
}

pub(crate) fn plugin(app: &mut App, catch_system_panics: bool, crash_replays: bool) {
    if catch_system_panics {
        app.insert_resource(FallbackErrorHandler(ai_core_error_handler));
    }
    app.insert_resource(AiFailures::new(crash_replays))
        .add_systems(FixedLast, collect_after_tick.after(journal::drain_logs))
        .add_systems(Last, collect_between_ticks);
}

/// Right after a tick: the exact tick, and in real time no further tick this frame.
fn collect_after_tick(world: &mut World) {
    collect(world);
}

fn take_queue() -> Vec<Queued> {
    let mut queued = QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
    // Systems of one tick fail on several threads: keep the journal stable.
    queued.sort_by(|a, b| (&a.kind, &a.name).cmp(&(&b.kind, &b.name)));
    queued
}

/// Takes the queued failures: journals them, saves a crash replay, pauses the game.
/// Returns what it took (also kept for the `step` running right now).
pub fn collect(world: &mut World) -> Vec<SystemFailure> {
    let queued = take_queue();
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
        })
        .collect();
    for failure in &mut failures {
        journal::log(world, JournalKind::Error, format!("{}: {}", failure.headline(), failure.message));
        failure.crash_replay = save_crash_replay(world, failure);
    }
    if let Some(mut state) = world.get_resource_mut::<AiFailures>() {
        state.unreported.extend(failures.iter().cloned());
    }
    // A failure while loading (an `OnEnter` spawning the world) finds real time paused
    // by the gate: the pause becomes the failure's, so opening the gate keeps it.
    let gate_paused = world.get_resource_mut::<SimGate>().is_some_and(|mut gate| gate.forget_pause());
    if !sim::is_paused(world) || gate_paused {
        sim::pause(world);
        // Real-time play may have more ticks due this frame: not after a failure.
        let mut fixed = world.resource_mut::<Time<Fixed>>();
        let overstep = fixed.overstep();
        fixed.discard_overstep(overstep);
        journal::log(world, JournalKind::Sim, format!("paused: {}", failures[0].headline()));
    }
    failures
}

/// Failures collected since the last call, for the `step` that is running.
pub fn take_unreported(world: &mut World) -> Vec<SystemFailure> {
    world.get_resource_mut::<AiFailures>().map(|mut f| std::mem::take(&mut f.unreported)).unwrap_or_default()
}

/// Once per frame: failures outside ticks (or after this tick's collection). Nobody is
/// stepping here, so they are journaled and the game is paused, nothing more.
fn collect_between_ticks(world: &mut World) {
    collect(world);
    take_unreported(world);
}

/// After a tool call: a failure there (a command the tool queued) is the tool's, not
/// gameplay's. It is journaled with the tool's name; the game keeps running.
pub(crate) fn blame_tool(world: &mut World, tool: &str) {
    for q in take_queue() {
        let verb = if q.panicked { "panicked" } else { "failed" };
        journal::log(world, JournalKind::Error, format!("tool {tool}: {} {} {verb}: {}", q.kind, q.name, q.message));
    }
}

fn save_crash_replay(world: &mut World, failure: &SystemFailure) -> Option<String> {
    let key = format!("{} {}", failure.kind, failure.name);
    let mut state = world.get_resource_mut::<AiFailures>()?;
    if !state.crash_replays || !state.saved.insert(key) {
        return None;
    }
    let recording = replay::save(world);
    let path = format!("{}/crash-tick{}.json", replay::REPLAYS_DIR, recording.end.tick);
    match replay::write(&recording, &path) {
        Ok(_) => {
            journal::log(world, JournalKind::Sim, format!("crash replay saved: {path} (play it with --replay to reproduce)"));
            Some(path)
        }
        Err(e) => {
            journal::log(world, JournalKind::Error, format!("crash replay not saved: {e}"));
            None
        }
    }
}
