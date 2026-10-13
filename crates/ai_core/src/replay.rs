//! Replays: the same seed plus the same inputs gives the same game, so a session is
//! stored as its inputs, not its frames.
//!
//! Recording is always on, from tick 0, and costs almost nothing:
//! - **inputs**: components registered with
//!   [`replay_input`](crate::AiAppExt::replay_input) (the sandbox's `Intent`). Every
//!   change made *between* ticks (keyboard, network client, agent tool) is stored
//!   with the tick it took effect at. Changes made by gameplay systems inside a
//!   tick are deterministic and not stored.
//! - **calls**: every action tool call (`teleport`, `set`, `spawn`, `attack`, …) with
//!   the caller's scope and body. Control tools (`pause`, `step`, `possess`) change
//!   *when* things happen, not what, and are left out. Raw BRP edits
//!   (`world.insert_components`) bypass the tools and are not recorded.
//! - **checks**: a hash of the AI-visible state every [`CHECK_EVERY`] ticks.
//!
//! Playback needs a fresh world built with the same seed and tick rate. Events are
//! applied between ticks, exactly where they happened; input changes nobody
//! recorded (a human pressing keys while watching) are undone. Each check is
//! compared, so the report names the first tick where the game no longer matches
//! the recording — after a code change, that is a regression. When playback
//! reaches the end the game goes live: a replay doubles as a save point to branch
//! from. An action call from anyone during playback takes over and ends it.
//!
//! Recording continues during playback, so a branch saves as one whole replay.

use std::fmt;

use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::Mutable;
use bevy::ecs::system::SystemChangeTick;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::describe::all_facts;
use crate::id::{AiId, AiIndex};
use crate::journal::{self, JournalKind};
use crate::rng::SimSeed;
use crate::session::{AgentSessions, Caller, Scope};
use crate::sim::{self, SimTick};
use crate::tools::{call_tool, register_tool, Call, ToolKind, ToolResult};

/// Format tag written into every replay file.
pub const FORMAT: &str = "shards-replay/1";

/// Ticks between state checks (one second at 60 Hz).
pub const CHECK_EVERY: u64 = 60;

/// Folder for saved replays, relative to the game's working directory. Tools write
/// replays nowhere else (see [`paths`](crate::paths)).
pub const REPLAYS_DIR: &str = "replays";

/// A recorded session: everything needed to play it again, nothing derived.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Replay {
    pub format: String,
    pub seed: u64,
    pub tick_hz: f64,
    /// The recording ends after this many ticks, in this state.
    pub end: Check,
    /// In the order they happened.
    pub events: Vec<ReplayEvent>,
    /// State hashes along the way, every [`CHECK_EVERY`] ticks.
    pub checks: Vec<Check>,
}

/// State hash of the AI-visible world after `tick` ticks.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Check {
    pub tick: u64,
    /// Hex, so JSON tools that read numbers as doubles keep all 64 bits.
    #[serde(with = "hex_u64")]
    pub hash: u64,
}

/// Something that happened between tick `at` and tick `at + 1`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayEvent {
    /// An action tool call, replayed as the same agent with the same scope and body.
    Call { at: u64, agent: String, scope: Scope, subject: Option<AiId>, tool: String, args: Value },
    /// The new value of an input component.
    Input { at: u64, id: AiId, component: String, value: Value },
}

impl ReplayEvent {
    pub fn at(&self) -> u64 {
        match self {
            ReplayEvent::Call { at, .. } | ReplayEvent::Input { at, .. } => *at,
        }
    }
}

/// How a playback went.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// Tick where playback ended.
    pub tick: u64,
    /// Ticks the recording covers.
    pub end_tick: u64,
    /// State checks that matched the recording (the final state included).
    pub matched: u32,
    /// First tick whose state differed from the recording.
    pub diverged_at: Option<u64>,
    /// Why playback stopped before the end, e.g. "dev took over (teleport)".
    pub interrupted: Option<String>,
}

impl ReplayReport {
    /// Played to the end and every check matched.
    pub fn identical(&self) -> bool {
        self.diverged_at.is_none() && self.interrupted.is_none()
    }
}

impl fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.interrupted, self.diverged_at) {
            (Some(why), _) => write!(f, "replay interrupted at tick {} of {}: {why}", self.tick, self.end_tick)?,
            (None, None) => write!(
                f,
                "replay finished at tick {}: identical to the recording ({} checks)",
                self.tick, self.matched
            )?,
            (None, Some(at)) => write!(f, "replay finished at tick {}: DIVERGED at tick {at}", self.tick)?,
        }
        if let (Some(_), Some(at)) = (&self.interrupted, self.diverged_at) {
            write!(f, " (it had diverged at tick {at})")?;
        }
        Ok(())
    }
}

struct Playback {
    replay: Replay,
    /// Index of the next event to apply.
    next: usize,
    /// Input events due before the current tick, picked up by the typed input systems.
    due_inputs: Vec<(AiId, String, Value)>,
    matched: u32,
    diverged_at: Option<u64>,
}

/// The recording of this session, and the playback in progress (if any).
#[derive(Resource, Default)]
pub struct AiReplay {
    events: Vec<ReplayEvent>,
    checks: Vec<Check>,
    /// Names of the registered input components.
    inputs: Vec<&'static str>,
    playback: Option<Playback>,
    report: Option<ReplayReport>,
    /// Set while playback itself calls tools, so its calls do not count as a takeover.
    replaying: bool,
    /// Action calls running right now. Calls made from inside another action are
    /// not recorded: replaying the outer one makes them again.
    action_depth: u32,
}

impl AiReplay {
    pub fn is_playing(&self) -> bool {
        self.playback.is_some()
    }

    /// The outcome of the last playback.
    pub fn report(&self) -> Option<&ReplayReport> {
        self.report.as_ref()
    }

    pub fn recorded_events(&self) -> usize {
        self.events.len()
    }
}

// --- recording -------------------------------------------------------------------

/// Called by [`call_tool`] before an action tool runs: records the call and, if a
/// replay is playing and the call is not its own, ends the playback.
pub(crate) fn begin_action(world: &mut World, caller: &Caller, tool: &str, args: &Value) {
    let at = sim::tick(world);
    let mut replay = world.resource_mut::<AiReplay>();
    replay.action_depth += 1;
    if replay.action_depth > 1 {
        return;
    }
    replay.events.push(ReplayEvent::Call {
        at,
        agent: caller.agent.clone(),
        scope: caller.scope,
        subject: caller.subject_id,
        tool: tool.to_owned(),
        args: args.clone(),
    });
    let takeover = replay.playback.is_some() && !replay.replaying;
    if takeover {
        interrupt(world, format!("{} took over ({tool})", caller.agent));
    }
}

/// Called by [`call_tool`] after an action tool ran (panics are caught inside).
pub(crate) fn end_action(world: &mut World) {
    let mut replay = world.resource_mut::<AiReplay>();
    replay.action_depth = replay.action_depth.saturating_sub(1);
}

/// Values of an input component at the end of the last tick, and when that was.
#[derive(Resource)]
struct InputMemory<C> {
    last: HashMap<Entity, C>,
    since: Tick,
}

impl<C> Default for InputMemory<C> {
    fn default() -> Self {
        Self { last: HashMap::default(), since: Tick::new(0) }
    }
}

/// End of every tick (and after `Startup`): remember the values the tick left behind.
fn remember_inputs<C: Component + Clone>(
    q: Query<(Entity, Ref<C>)>,
    mut removed: RemovedComponents<C>,
    mut memory: ResMut<InputMemory<C>>,
    ticks: SystemChangeTick,
) {
    for (entity, value) in &q {
        if value.is_changed() {
            memory.last.insert(entity, (*value).clone());
        }
    }
    for entity in removed.read() {
        memory.last.remove(&entity);
    }
    memory.since = ticks.this_run();
}

/// Start of every tick, before anything else: apply recorded inputs (playback),
/// undo unrecorded ones (playback), and record what changed since the last tick.
fn inputs_between_ticks<C: Component<Mutability = Mutable> + Clone + PartialEq + Serialize + DeserializeOwned>(
    mut q: Query<(Entity, &AiId, Mut<C>)>,
    index: Res<AiIndex>,
    memory: Res<InputMemory<C>>,
    mut replay: ResMut<AiReplay>,
    tick: Res<SimTick>,
    ticks: SystemChangeTick,
) {
    let name = crate::short_type_name::<C>();
    let replay = &mut *replay;
    if let Some(playback) = &mut replay.playback {
        let mut due = Vec::new();
        playback.due_inputs.retain(|(id, component, value)| {
            let mine = *component == name;
            if mine {
                due.push((*id, value.clone()));
            }
            !mine
        });
        for (entity, _, mut value) in &mut q {
            let changed = value.last_changed().is_newer_than(memory.since, ticks.this_run());
            if changed
                && let Some(last) = memory.last.get(&entity)
                && *value != *last
            {
                *value = last.clone();
            }
        }
        for (id, recorded) in due {
            let applied = index.get(id).and_then(|e| q.get_mut(e).ok()).zip(serde_json::from_value::<C>(recorded).ok());
            match applied {
                Some(((_, _, mut value), recorded)) => *value = recorded,
                // The entity the recording talks about is not there: the run has
                // already gone its own way; the next check reports it.
                None => warn!("replay: input for {id} ({name}) at tick {} has no target", tick.0),
            }
        }
    }

    for (entity, id, value) in &q {
        if !value.last_changed().is_newer_than(memory.since, ticks.this_run()) {
            continue;
        }
        if memory.last.get(&entity) == Some(&*value) {
            continue;
        }
        match serde_json::to_value(&*value) {
            Ok(v) => replay.events.push(ReplayEvent::Input { at: tick.0, id: *id, component: name.to_owned(), value: v }),
            Err(e) => warn!("replay: cannot record {name} of {id}: {e}"),
        }
    }
}

/// Registers input component `C`. Prefer [`AiAppExt::replay_input`](crate::AiAppExt::replay_input).
pub fn register_input<C>(app: &mut App)
where
    C: Component<Mutability = Mutable> + Clone + PartialEq + Serialize + DeserializeOwned,
{
    app.world_mut().get_resource_or_init::<AiReplay>().inputs.push(crate::short_type_name::<C>());
    app.init_resource::<InputMemory<C>>()
        .add_systems(PostStartup, remember_inputs::<C>)
        .add_systems(FixedFirst, inputs_between_ticks::<C>.in_set(ReplaySystems::Inputs))
        .add_systems(FixedLast, remember_inputs::<C>.in_set(ReplaySystems::Remember));
}

/// Order of the replay systems around the tick counter.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReplaySystems {
    /// `FixedFirst`: recorded calls are applied before the tick starts.
    Calls,
    /// `FixedFirst`: input components are applied / recorded.
    Inputs,
    /// `FixedLast`: state check every [`CHECK_EVERY`] ticks.
    Check,
    /// `FixedLast`: inputs as the tick left them.
    Remember,
}

// --- state hash ------------------------------------------------------------------

/// FNV-1a over everything the AI can see: ids, labels, fragments, exact positions
/// and facings. Two worlds with the same hash look the same to every agent.
pub fn state_hash(world: &World) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    h.u64(sim::tick(world));
    for f in all_facts(world) {
        h.u64(f.id.0 as u64);
        h.bytes(f.label.as_bytes());
        for (key, text) in &f.fragments {
            h.bytes(key.as_bytes());
            h.bytes(text.as_bytes());
        }
        for v in [f.pos.x, f.pos.y, f.pos.z] {
            h.u64(v.to_bits() as u64);
        }
        if let Some(facing) = f.facing {
            for v in [facing.x, facing.y, facing.z] {
                h.u64(v.to_bits() as u64);
            }
        }
    }
    h.0
}

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        // Length-terminated, so "ab"+"c" and "a"+"bc" differ.
        self.u64_raw(bytes.len() as u64);
    }

    fn u64(&mut self, v: u64) {
        self.u64_raw(v);
    }

    fn u64_raw(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn check_state(world: &mut World) {
    let tick = sim::tick(world);
    if tick == 0 || !tick.is_multiple_of(CHECK_EVERY) {
        return;
    }
    let hash = state_hash(world);
    let mut replay = world.resource_mut::<AiReplay>();
    replay.checks.push(Check { tick, hash });
    let Some(playback) = &mut replay.playback else {
        return;
    };
    let Some(expected) = playback.replay.checks.iter().find(|c| c.tick == tick) else {
        return;
    };
    let first_divergence = if expected.hash == hash {
        playback.matched += 1;
        false
    } else if playback.diverged_at.is_none() {
        playback.diverged_at = Some(tick);
        true
    } else {
        false
    };
    if first_divergence {
        journal::log(world, JournalKind::Error, format!("replay diverged at tick {tick}: the state differs from the recording"));
    }
}

// --- playback --------------------------------------------------------------------

/// Starts playing `replay` back. The world must be fresh (tick 0, nothing recorded
/// yet) and built with the replay's seed and tick rate. Events are applied as ticks
/// run; step or resume the game to play it.
pub fn play(world: &mut World, replay: Replay) -> Result<(), String> {
    if replay.format != FORMAT {
        return Err(format!("not a replay this build can play: format `{}`, expected `{FORMAT}`", replay.format));
    }
    let tick = sim::tick(world);
    if tick != 0 {
        return Err(format!("playback needs a fresh world at tick 0; this one is at tick {tick}"));
    }
    let seed = world.get_resource::<SimSeed>().map(|s| s.0);
    if seed != Some(replay.seed) {
        return Err(format!(
            "the replay was recorded with seed {}; this world has seed {}",
            replay.seed,
            seed.map_or("none".into(), |s| s.to_string())
        ));
    }
    let hz = 1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64();
    if (hz - replay.tick_hz).abs() > 1e-3 {
        return Err(format!("the replay runs at {} ticks/s; this world at {hz:.3}", replay.tick_hz));
    }
    let state = world.resource::<AiReplay>();
    if state.playback.is_some() {
        return Err("a replay is already playing".into());
    }
    if !state.events.is_empty() {
        return Err(format!("this world already recorded {} events; playback needs a fresh one", state.events.len()));
    }
    let mut last_at = 0;
    for event in &replay.events {
        if event.at() < last_at || event.at() > replay.end.tick {
            return Err(format!("events out of order or past the end (tick {})", event.at()));
        }
        last_at = event.at();
        if let ReplayEvent::Input { component, .. } = event
            && !state.inputs.contains(&component.as_str())
        {
            return Err(format!(
                "the replay sets input `{component}`, which this game does not register (known: {})",
                state.inputs.join(", ")
            ));
        }
    }
    let end = replay.end.tick;
    let mut state = world.resource_mut::<AiReplay>();
    state.report = None;
    state.playback = Some(Playback { replay, next: 0, due_inputs: Vec::new(), matched: 0, diverged_at: None });
    journal::log(world, JournalKind::Sim, format!("replay started: {end} ticks"));
    Ok(())
}

/// Applies the calls due after the current tick and queues its inputs; at the end
/// of the recording, compares the final state and finishes. Runs before every tick
/// and once per frame (so a paused game finishes right at the last tick).
fn advance_playback(world: &mut World, at_tick_start: bool) {
    let tick = sim::tick(world);
    let end = match &world.resource::<AiReplay>().playback {
        Some(p) => p.replay.end.tick,
        None => return,
    };
    if !at_tick_start && tick != end {
        return;
    }
    loop {
        let event = {
            let mut replay = world.resource_mut::<AiReplay>();
            let Some(playback) = &mut replay.playback else { return };
            match playback.replay.events.get(playback.next) {
                Some(e) if e.at() <= tick => {
                    playback.next += 1;
                    e.clone()
                }
                _ => break,
            }
        };
        match event {
            ReplayEvent::Call { agent, scope, subject, tool, args, .. } => {
                replay_call(world, &agent, scope, subject, &tool, args);
            }
            ReplayEvent::Input { id, component, value, .. } => {
                if let Some(p) = &mut world.resource_mut::<AiReplay>().playback {
                    p.due_inputs.push((id, component, value));
                }
            }
        }
        // A replayed call can end the playback (only if it was interrupted).
        if world.resource::<AiReplay>().playback.is_none() {
            return;
        }
    }
    if tick >= end {
        finish(world);
    }
}

/// One recorded call, as its agent with the recorded scope and body; the agent's
/// live session is restored afterwards.
fn replay_call(world: &mut World, agent: &str, scope: Scope, subject: Option<AiId>, tool: &str, args: Value) {
    let saved = world.resource_mut::<AgentSessions>().get_or_create(agent).clone();
    world.resource_mut::<AgentSessions>().configure(agent, scope, subject);
    world.resource_mut::<AiReplay>().replaying = true;
    call_tool(world, agent, tool, args);
    world.resource_mut::<AiReplay>().replaying = false;
    world.resource_mut::<AgentSessions>().configure(agent, saved.scope, saved.subject);
}

fn finish(world: &mut World) {
    let hash = state_hash(world);
    let tick = sim::tick(world);
    let Some(playback) = world.resource_mut::<AiReplay>().playback.take() else {
        return;
    };
    let mut report = ReplayReport {
        tick,
        end_tick: playback.replay.end.tick,
        matched: playback.matched,
        diverged_at: playback.diverged_at,
        interrupted: None,
    };
    if hash == playback.replay.end.hash {
        report.matched += 1;
    } else if report.diverged_at.is_none() {
        report.diverged_at = Some(tick);
    }
    let kind = if report.identical() { JournalKind::Sim } else { JournalKind::Error };
    journal::log(world, kind, report.to_string());
    world.resource_mut::<AiReplay>().report = Some(report);
}

fn interrupt(world: &mut World, why: String) {
    let tick = sim::tick(world);
    let Some(playback) = world.resource_mut::<AiReplay>().playback.take() else {
        return;
    };
    let report = ReplayReport {
        tick,
        end_tick: playback.replay.end.tick,
        matched: playback.matched,
        diverged_at: playback.diverged_at,
        interrupted: Some(why),
    };
    journal::log(world, JournalKind::Sim, report.to_string());
    world.resource_mut::<AiReplay>().report = Some(report);
}

// --- saving ----------------------------------------------------------------------

/// The recording so far, ending in the current state.
pub fn save(world: &World) -> Replay {
    let replay = world.resource::<AiReplay>();
    Replay {
        format: FORMAT.to_owned(),
        seed: world.get_resource::<SimSeed>().map_or(0, |s| s.0),
        tick_hz: 1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64(),
        end: Check { tick: sim::tick(world), hash: state_hash(world) },
        events: replay.events.clone(),
        checks: replay.checks.clone(),
    }
}

/// Reads a replay file.
pub fn load(path: &std::path::Path) -> Result<Replay, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not a replay: {e}", path.display()))
}

/// Recording and playback state for `ai/status`.
pub fn status(world: &World) -> Value {
    let replay = world.resource::<AiReplay>();
    json!({
        "recorded_events": replay.events.len(),
        "playing": replay.playback.as_ref().map(|p| json!({
            "end_tick": p.replay.end.tick,
            "events_left": p.replay.events.len() - p.next,
            "diverged_at": p.diverged_at,
        })),
        "last_report": replay.report.as_ref().map(ToString::to_string),
    })
}

/// Save this session as a replay file: the seed, every input and action call since
/// tick 0, and state checks. Play it back in a fresh game with the same seed to get
/// exactly this game again (and to see where a changed build diverges from it).
#[derive(Deserialize, JsonSchema, Default)]
pub struct ReplaySaveArgs {
    /// Where to write it, inside replays/ in the game's working directory, e.g.
    /// "replays/boss-fight.json". Default: replays/tick<T>.json.
    pub path: Option<String>,
}

/// A good path for the errors that refuse a bad one.
const REPLAY_PATH_EXAMPLE: &str = "replays/boss-fight.json";

/// Writes `replay` to `requested` (a path inside `replays/`); returns where it went.
pub fn write(replay: &Replay, requested: &str) -> Result<std::path::PathBuf, String> {
    let path = crate::paths::inside(REPLAYS_DIR, requested, REPLAY_PATH_EXAMPLE)?;
    let text = serde_json::to_string_pretty(replay).map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

fn replay_save_tool(In(call): In<Call<ReplaySaveArgs>>, world: &mut World) -> ToolResult {
    let replay = save(world);
    let requested = call.args.path.unwrap_or_else(|| format!("{REPLAYS_DIR}/tick{}.json", replay.end.tick));
    let path = write(&replay, &requested)?;
    Ok(format!(
        "saved {}: {} ticks, {} events, {} checks, seed {}",
        path.display(),
        replay.end.tick,
        replay.events.len(),
        replay.checks.len() + 1,
        replay.seed
    ))
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<AiReplay>()
        .configure_sets(
            FixedFirst,
            (ReplaySystems::Calls, ReplaySystems::Inputs).chain().before(sim::advance_tick),
        )
        .configure_sets(FixedLast, (ReplaySystems::Check, ReplaySystems::Remember).chain())
        .add_systems(FixedFirst, (|w: &mut World| advance_playback(w, true)).in_set(ReplaySystems::Calls))
        .add_systems(FixedLast, check_state.in_set(ReplaySystems::Check))
        .add_systems(Last, |w: &mut World| advance_playback(w, false));
    register_tool(app.world_mut(), "replay_save", Scope::Dev, ToolKind::Query, replay_save_tool);
}

mod hex_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{v:016x}"))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let text = String::deserialize(d)?;
        u64::from_str_radix(&text, 16).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_checks_have_a_readable_json_shape() {
        let check = Check { tick: 60, hash: 0xdead_beef_0123_4567 };
        assert_eq!(serde_json::to_value(check).unwrap(), json!({"tick": 60, "hash": "deadbeef01234567"}));
        let back: Check = serde_json::from_value(json!({"tick": 60, "hash": "deadbeef01234567"})).unwrap();
        assert_eq!(back, check);

        let call = ReplayEvent::Call {
            at: 3,
            agent: "dev".into(),
            scope: Scope::Dev,
            subject: Some(AiId(5)),
            tool: "teleport".into(),
            args: json!({"id": 6, "x": 1, "z": 2}),
        };
        let v = serde_json::to_value(&call).unwrap();
        assert_eq!(v["call"]["at"], 3);
        assert_eq!(v["call"]["scope"], "dev");
        assert_eq!(serde_json::from_value::<ReplayEvent>(v).unwrap(), call);
    }

    #[test]
    fn hash_separates_fields() {
        let (mut a, mut b) = (Fnv(1), Fnv(1));
        a.bytes(b"ab");
        a.bytes(b"c");
        b.bytes(b"a");
        b.bytes(b"bc");
        assert_ne!(a.0, b.0);
    }

    #[test]
    fn report_says_what_happened() {
        let mut r = ReplayReport { tick: 600, end_tick: 600, matched: 11, diverged_at: None, interrupted: None };
        assert_eq!(r.to_string(), "replay finished at tick 600: identical to the recording (11 checks)");
        r.diverged_at = Some(240);
        assert_eq!(r.to_string(), "replay finished at tick 600: DIVERGED at tick 240");
        r.tick = 300;
        r.interrupted = Some("dev took over (teleport)".into());
        assert_eq!(
            r.to_string(),
            "replay interrupted at tick 300 of 600: dev took over (teleport) (it had diverged at tick 240)"
        );
    }
}
