//! Event journal: what happened since the agent last looked.
//!
//! A snapshot without causes says little: "the boss entered phase 2" matters more
//! than its coordinates. Gameplay events (via observers), agent actions, sim control
//! and `warn!`/`error!` logs all land in one ring buffer, stamped with the tick.
//!
//! Log lines carry the tick they were logged at (read from [`TickClock`] by the
//! tracing layer) and are moved into the journal after every tick, once per frame, and
//! around every tool call. So a warning inside a long `step` keeps its own tick, ends
//! the step if `--until journal:` looks for it, and an action's own warnings come back
//! with that action. At most [`MAX_CAPTURED_PER_TICK`] lines per tick; the rest are
//! counted in one `N log lines dropped` line.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use bevy::log::tracing::field::{Field, Visit};
use bevy::log::tracing::{Event as TracingEvent, Level, Subscriber};
use bevy::log::tracing_subscriber::Layer;
use bevy::log::tracing_subscriber::layer::Context;
use bevy::ecs::world::DeferredWorld;
use bevy::log::BoxedLayer;
use bevy::prelude::*;
use serde::Serialize;

use crate::sim::{SimTick, TickClock};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    /// Gameplay event registered with `journal_event`.
    Event,
    /// An agent called an action tool.
    Agent,
    /// Pause / resume / step / speed changes.
    Sim,
    Warn,
    Error,
}

#[derive(Clone, Debug, Serialize)]
pub struct JournalEntry {
    /// Monotonic sequence number; use it as a cursor ("everything after 1287").
    pub seq: u64,
    pub tick: u64,
    pub kind: JournalKind,
    pub text: String,
}

impl std::fmt::Display for JournalEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            JournalKind::Event => write!(f, "{} {}", self.tick, self.text),
            JournalKind::Agent => write!(f, "{} [agent] {}", self.tick, self.text),
            JournalKind::Sim => write!(f, "{} [sim] {}", self.tick, self.text),
            JournalKind::Warn => write!(f, "{} [warn] {}", self.tick, self.text),
            JournalKind::Error => write!(f, "{} [error] {}", self.tick, self.text),
        }
    }
}

#[derive(Resource, Debug)]
pub struct AiJournal {
    entries: VecDeque<JournalEntry>,
    next_seq: u64,
    capacity: usize,
}

impl Default for AiJournal {
    fn default() -> Self {
        Self::with_capacity(2048)
    }
}

impl AiJournal {
    pub fn with_capacity(capacity: usize) -> Self {
        Self { entries: VecDeque::with_capacity(capacity.min(4096)), next_seq: 1, capacity: capacity.max(1) }
    }

    pub fn push(&mut self, tick: u64, kind: JournalKind, text: impl Into<String>) -> u64 {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(JournalEntry { seq, tick, kind, text: text.into() });
        seq
    }

    /// Sequence number of the newest entry (0 if empty).
    pub fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    /// Entries with `seq > after`, oldest first. The second value counts entries
    /// that were already evicted from the ring buffer (the agent missed them).
    pub fn after(&self, after: u64) -> (impl Iterator<Item = &JournalEntry>, u64) {
        let oldest = self.entries.front().map_or(self.next_seq, |e| e.seq);
        let missed = oldest.saturating_sub(after + 1);
        (self.entries.iter().filter(move |e| e.seq > after), missed)
    }

    pub fn since_tick(&self, tick: u64) -> impl Iterator<Item = &JournalEntry> {
        self.entries.iter().filter(move |e| e.tick >= tick)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Appends to the journal at the current tick.
pub fn log(world: &mut World, kind: JournalKind, text: impl Into<String>) -> u64 {
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    world.resource_mut::<AiJournal>().push(tick, kind, text)
}

/// Log target of [`emit`]'s console line: the capture layer leaves it out, because
/// the entry is in the journal already.
pub const EMIT_TARGET: &str = "ai_core::journal";

/// Journals `text` and prints it on the console (`warn!`/`error!` by kind, `info!`
/// otherwise), once each: for engine messages that must reach the journal even in
/// apps without the log capture (tests), and the terminal too.
pub fn emit(world: &mut World, kind: JournalKind, text: impl Into<String>) -> u64 {
    let text = text.into();
    match kind {
        JournalKind::Error => error!(target: EMIT_TARGET, "{text}"),
        JournalKind::Warn => warn!(target: EMIT_TARGET, "{text}"),
        _ => info!(target: EMIT_TARGET, "{text}"),
    }
    match world.get_resource::<AiJournal>() {
        Some(_) => log(world, kind, text),
        None => 0,
    }
}

/// Formatter for a gameplay event; `None` skips the entry.
pub type EventFormatter<E> = fn(&E, &World) -> Option<String>;

pub(crate) fn journal_observer<E: Event>(format: EventFormatter<E>) -> impl FnMut(On<E>, DeferredWorld) {
    move |event: On<E>, mut world: DeferredWorld| {
        let Some(text) = format(event.event(), &world) else {
            return;
        };
        let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
        if let Some(mut journal) = world.get_resource_mut::<AiJournal>() {
            journal.push(tick, JournalKind::Event, text);
        }
    }
}

// --- tracing capture -------------------------------------------------------------

struct CapturedLog {
    tick: u64,
    level: Level,
    target: String,
    message: String,
}

/// Lines captured since the last [`drain_logs`].
#[derive(Default)]
struct Captured {
    lines: Vec<CapturedLog>,
    /// Tick of the newest line, and how many lines of that tick were kept.
    tick: u64,
    kept: usize,
    /// `(tick, lines dropped)`, oldest first.
    dropped: Vec<(u64, u64)>,
}

/// Shared buffer between the tracing layer (any thread) and the ECS.
#[derive(Resource, Clone, Default)]
pub struct LogCapture(Arc<Mutex<Captured>>);

/// Log lines kept per tick (and per frame between ticks).
pub const MAX_CAPTURED_PER_TICK: usize = 64;

/// The tracing subscriber is global and set once per process, while every app keeps its
/// own buffer: `(app token, buffer, clock)`, oldest first. The layer writes into the
/// [active](crate::process::active) app's buffer, or the newest one when the active app has
/// none (an app alone in its process never notices the difference).
static CAPTURE_TARGETS: Mutex<Vec<(u64, LogCapture, TickClock)>> = Mutex::new(Vec::new());

/// The buffer the layer writes into right now.
fn capture_target() -> Option<(LogCapture, TickClock)> {
    let targets = CAPTURE_TARGETS.lock().ok()?;
    let active = crate::process::active();
    let (_, capture, clock) = targets.iter().rev().find(|(token, ..)| *token == active).or_else(|| targets.last())?;
    Some((capture.clone(), clock.clone()))
}

/// Takes the app's buffer out of [`CAPTURE_TARGETS`] when its world is dropped.
#[derive(Resource)]
struct CaptureRegistration(u64);

impl Drop for CaptureRegistration {
    fn drop(&mut self) {
        if let Ok(mut targets) = CAPTURE_TARGETS.lock() {
            targets.retain(|(token, ..)| *token != self.0);
        }
    }
}

struct CaptureLayer;

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &TracingEvent<'_>, _ctx: Context<'_, S>) {
        let level = *event.metadata().level();
        // Failures are journaled by `failures::collect`, with their tick, and `emit`
        // journals its own lines; once is enough.
        let target = event.metadata().target();
        if level > Level::WARN || target == crate::failures::LOG_TARGET || target == EMIT_TARGET {
            return;
        }
        let Some((capture, clock)) = capture_target() else {
            return;
        };
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let tick = clock.get();
        let Ok(mut buf) = capture.0.lock() else {
            return;
        };
        if buf.tick != tick {
            buf.tick = tick;
            buf.kept = 0;
        }
        if buf.kept < MAX_CAPTURED_PER_TICK {
            buf.kept += 1;
            buf.lines.push(CapturedLog { tick, level, target: event.metadata().target().to_owned(), message: visitor.0 });
        } else {
            match buf.dropped.last_mut() {
                Some((t, n)) if *t == tick => *n += 1,
                _ => buf.dropped.push((tick, 1)),
            }
        }
    }
}

struct MessageVisitor(String);

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

/// Plug into Bevy's logger so `warn!`/`error!` reach the journal:
///
/// ```ignore
/// DefaultPlugins.set(LogPlugin { custom_layer: ai_core::capture_log_layer, ..default() })
/// ```
///
/// Several apps in one process: the first one installs the layer (the subscriber is
/// global), every app gets its own buffer through [`register_capture`], and lines go to the
/// [active](crate::process) app.
pub fn capture_log_layer(app: &mut App) -> Option<BoxedLayer> {
    register_capture(app);
    Some(Box::new(CaptureLayer))
}

/// Gives this app its own log buffer in the process-wide capture layer, without installing
/// a layer: for an app embedded next to another that installed it (no `LogPlugin` of its
/// own, like a client in an in-process stand). Lines logged while the app is
/// [active](crate::process::activate) land in its journal. Registering again does nothing.
pub fn register_capture(app: &mut App) {
    let world = app.world_mut();
    let token = crate::process::ensure_token(world);
    if world.contains_resource::<CaptureRegistration>() {
        return;
    }
    let capture = LogCapture::default();
    let clock = world.get_resource_or_init::<TickClock>().clone();
    world.insert_resource(capture.clone());
    if let Ok(mut targets) = CAPTURE_TARGETS.lock() {
        targets.push((token, capture, clock));
    }
    world.insert_resource(CaptureRegistration(token));
}

/// Moves captured log lines into the journal, each with the tick it was logged at.
/// Runs after every tick and once per frame; call it before reading the journal when
/// code that may log just ran.
pub fn drain_logs(world: &mut World) {
    let Some(capture) = world.get_resource::<LogCapture>() else {
        return;
    };
    let (lines, dropped) = match capture.0.lock() {
        Ok(mut buf) => {
            buf.kept = 0;
            (std::mem::take(&mut buf.lines), std::mem::take(&mut buf.dropped))
        }
        Err(_) => return,
    };
    let Some(mut journal) = world.get_resource_mut::<AiJournal>() else {
        return;
    };
    for log in lines {
        let kind = if log.level == Level::ERROR { JournalKind::Error } else { JournalKind::Warn };
        journal.push(log.tick, kind, format!("{}: {}", log.target, log.message));
    }
    for (tick, n) in dropped {
        let text = format!("{n} log lines dropped (limit {MAX_CAPTURED_PER_TICK} per tick)");
        journal.push(tick, JournalKind::Warn, text);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_reports_missed_entries() {
        let mut j = AiJournal::with_capacity(3);
        for i in 0..5 {
            j.push(i, JournalKind::Event, format!("e{i}"));
        }
        let (entries, missed) = j.after(0);
        let texts: Vec<_> = entries.map(|e| e.text.clone()).collect();
        assert_eq!(texts, ["e2", "e3", "e4"]);
        assert_eq!(missed, 2);
        let (entries, missed) = j.after(4);
        assert_eq!(entries.count(), 1);
        assert_eq!(missed, 0);
    }
}
