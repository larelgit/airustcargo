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
//!   the caller's scope and body (a tool may take back one it refused,
//!   [`forget_call`]). Control tools (`pause`, `step`, `possess`) change
//!   *when* things happen, not what, and are left out. Raw BRP edits
//!   (`world.insert_components`) bypass the tools and are not recorded.
//! - **rules**: session invariants added, removed and re-armed (`assert_*`, see
//!   [`invariants`](crate::invariants)). They do not change the game, but playback makes
//!   them at the same ticks, so a replay catches the same violations.
//! - **checks**: a hash of the game state every `check_every` ticks ([`CHECK_EVERY`] by
//!   default): every AI-visible entity's id and gameplay components (see
//!   [`state_hash`]), not the text the AI reads. Next to each hash, its
//!   [`Breakdown`]: a hash per entity and component ([`CheckDetail`]).
//!
//! Playback needs a fresh world built with the same seed and tick rate. Events are
//! applied between ticks, exactly where they happened; input changes nobody
//! recorded (a human pressing keys while watching) are undone. Each check is
//! compared, so the report names the first tick where the game no longer matches
//! the recording — after a code change, that is a regression — and, from the
//! breakdowns, what differs there (`DIVERGED at tick 60: e6 Boss: Health differs`)
//! with its value in this run. When playback reaches the end the game goes live: a
//! replay doubles as a save point to branch from. An action call from anyone during
//! playback takes over and ends it.
//!
//! Recording continues during playback, so a branch saves as one whole replay.
//!
//! **From a snapshot.** A recording that starts at tick `N` instead of 0 carries the
//! [`Snapshot`] it started from (`start`, format `shards-replay/3`). Loading a snapshot
//! into a running game (`snapshot_load`) restarts the recording there: what came before
//! is not part of it any more, the snapshot is. Playback loads that snapshot as soon as
//! the game has loaded, then plays the events and compares the checks as usual, so
//! `--replay FILE --verify` works for such a replay too. Replays without a start are
//! `shards-replay/2` or `/3` files that begin at tick 0; this build plays both.

use std::any::TypeId;
use std::borrow::Cow;
use std::fmt;
use std::str::FromStr;

use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::Mutable;
use bevy::ecs::system::SystemChangeTick;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectSerializerProcessor, TypedReflectSerializer};
use bevy::reflect::TypeRegistry;
use schemars::JsonSchema;
use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

use crate::describe::{gameplay_components, AiKind};
use crate::id::{AiId, AiIndex, AiVisible};
use crate::invariants::{self, AssertOp};
use crate::journal::{self, JournalKind};
use crate::rng::SimSeed;
use crate::session::{AgentSessions, Caller, Scope};
use crate::sim::{self, SimTick};
use crate::snapshot::{self, Snapshot};
use crate::tools::{call_tool, register_tool, Call, ToolKind, ToolResult};

/// Format tag written into every replay file. Version 3 adds, each of them optional: a
/// starting snapshot (`start`), session rules (`assert` events), the check interval and
/// a breakdown of every check. A version 3 file without some of them plays as one
/// without them: from tick 0, no rules, a check every [`CHECK_EVERY`] ticks, hashes
/// alone. Version 2 files (hashes of game data, from tick 0) play as before, checked by
/// their hashes alone. Version 1 hashed the AI's text and cannot be checked by this
/// build.
pub const FORMAT: &str = "shards-replay/3";

const FORMAT_V2: &str = "shards-replay/2";
const FORMAT_V1: &str = "shards-replay/1";

/// Ticks between state checks by default (one second at 60 Hz), and in every
/// version 2 replay.
pub const CHECK_EVERY: u64 = 60;

/// Entities a divergence report lists; the rest are counted (`… and 7 more`).
pub const MAX_LISTED: usize = 10;

/// Longest value line in a divergence report (`now: Health { … }`), in characters.
const MAX_VALUE_CHARS: usize = 240;

/// What each check keeps next to its hash, to name what diverged. Per component makes
/// the biggest files: a hash for every component of every AI-visible entity.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckDetail {
    /// A hash per component: the report names entities and components, with values.
    #[default]
    Components,
    /// A hash per entity: the report names entities.
    Entities,
    /// The hash alone: the report names the tick.
    None,
}

impl CheckDetail {
    /// What a replay that does not say (version 2) has.
    fn hashes_only() -> Self {
        CheckDetail::None
    }
}

impl fmt::Display for CheckDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CheckDetail::Components => "components",
            CheckDetail::Entities => "entities",
            CheckDetail::None => "none",
        })
    }
}

impl FromStr for CheckDetail {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "components" => Ok(CheckDetail::Components),
            "entities" => Ok(CheckDetail::Entities),
            "none" => Ok(CheckDetail::None),
            _ => Err(format!("unknown check detail `{s}`: use components, entities or none")),
        }
    }
}

fn default_check_every() -> u64 {
    CHECK_EVERY
}

/// Folder for saved replays, relative to the game's working directory. Tools write
/// replays nowhere else (see [`paths`](crate::paths)).
pub const REPLAYS_DIR: &str = "replays";

/// A recorded session: everything needed to play it again, nothing derived.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Replay {
    pub format: String,
    pub seed: u64,
    pub tick_hz: f64,
    /// Ticks between state checks. Playback checks at the same ticks.
    #[serde(default = "default_check_every")]
    pub check_every: u64,
    /// What each check keeps next to its hash.
    #[serde(default = "CheckDetail::hashes_only")]
    pub check_detail: CheckDetail,
    /// The state the recording starts from, when it does not start at tick 0: playback
    /// loads it first. Every event and check is at or after its tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<Snapshot>,
    /// The recording ends after this many ticks, in this state.
    pub end: Check,
    /// In the order they happened.
    pub events: Vec<ReplayEvent>,
    /// State hashes along the way, every `check_every` ticks.
    pub checks: Vec<Check>,
}

/// State hash of the game after `tick` ticks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub tick: u64,
    /// Hex, so JSON tools that read numbers as doubles keep all 64 bits.
    #[serde(with = "hex_u64")]
    pub hash: u64,
    /// What the hash is made of; `None` with [`CheckDetail::None`] and in version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Breakdown>,
}

/// What a check's hash is made of, in the same pass: a hash per AI-visible entity (or
/// per component of it) and each [`add_replay_check`](crate::AiAppExt::add_replay_check)
/// value. Comparing two of them names what diverged.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// In id order; `{"e6": {"Health": "…", "Transform": "…"}}` in the file, or
    /// `{"e6": "…"}` with [`CheckDetail::Entities`].
    #[serde(with = "by_id")]
    pub entities: Vec<(AiId, EntityParts)>,
    /// `add_replay_check` values by name, in the order they are hashed.
    #[serde(with = "by_name", default, skip_serializing_if = "Vec::is_empty")]
    pub game: Vec<(String, u64)>,
}

/// One entity's share of a check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityParts {
    /// [`CheckDetail::Entities`]: one hash over everything the entity adds.
    Whole(u64),
    /// [`CheckDetail::Components`]: a hash per component, by short type name, sorted
    /// (a full type path where two components share the short name).
    Components(Vec<(Cow<'static, str>, u64)>),
}

/// Something that happened between tick `at` and tick `at + 1`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayEvent {
    /// An action tool call, replayed as the same agent with the same scope and body.
    Call { at: u64, agent: String, scope: Scope, subject: Option<AiId>, tool: String, args: Value },
    /// The new value of an input component.
    Input { at: u64, id: AiId, component: String, value: Value },
    /// A session rule added, removed or re-armed (`assert_*`).
    Assert { at: u64, op: AssertOp },
}

impl ReplayEvent {
    pub fn at(&self) -> u64 {
        match self {
            ReplayEvent::Call { at, .. } | ReplayEvent::Input { at, .. } | ReplayEvent::Assert { at, .. } => *at,
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
    /// What differed at `diverged_at`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub divergence: Option<Divergence>,
}

impl ReplayReport {
    /// Played to the end and every check matched.
    pub fn identical(&self) -> bool {
        self.diverged_at.is_none() && self.interrupted.is_none()
    }

    /// The first line: `replay finished at tick 600: DIVERGED at tick 60: e6 Boss: Health differs`.
    pub fn headline(&self) -> String {
        let at = |at: u64| match &self.divergence {
            Some(d) => format!("{at}: {}", d.summary()),
            None => at.to_string(),
        };
        match (&self.interrupted, self.diverged_at) {
            (Some(why), None) => format!("replay interrupted at tick {} of {}: {why}", self.tick, self.end_tick),
            (Some(why), Some(tick)) => {
                format!("replay interrupted at tick {} of {}: {why} (it had diverged at tick {})", self.tick, self.end_tick, at(tick))
            }
            (None, None) => format!("replay finished at tick {}: identical to the recording ({} checks)", self.tick, self.matched),
            (None, Some(tick)) => format!("replay finished at tick {}: DIVERGED at tick {}", self.tick, at(tick)),
        }
    }

    /// What the components that diverged hold in this run, one line each:
    /// `e6 now: Health { current: 2075.0, max: 4000.0 }`.
    pub fn value_lines(&self) -> Vec<String> {
        self.divergence.as_ref().map(Divergence::value_lines).unwrap_or_default()
    }
}

/// The headline, then a line per diverged value.
impl fmt::Display for ReplayReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.headline())?;
        for line in self.value_lines() {
            write!(f, "\n  {line}")?;
        }
        Ok(())
    }
}

/// What a diverging check found different, from the breakdowns of the recording and
/// of this run.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    pub tick: u64,
    /// Entities that differ, are missing or extra, in id order; at most [`MAX_LISTED`].
    pub entities: Vec<EntityDiff>,
    /// Entities that differ beyond the list.
    pub more: usize,
    /// `add_replay_check` values that differ: `weather differs`, `weather missing`.
    pub checks: Vec<String>,
    /// Why nothing more specific can be said, e.g. `replay format 2`.
    pub no_details: Option<String>,
}

/// One entity in a [`Divergence`].
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct EntityDiff {
    pub id: AiId,
    /// The entity's `AiKind` in this run (`Boss`); empty when it is not there.
    pub kind: String,
    pub change: EntityChange,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityChange {
    /// In the recording, not in this run; what this run's id index knows about it.
    Missing { gone: Option<String> },
    /// In this run, not in the recording.
    Extra,
    /// Some of its components differ (or are missing or extra): their names, unless the
    /// recording has a hash per entity only, and each one's value in this run.
    Differs { components: Vec<String>, now: Vec<String> },
}

impl EntityDiff {
    /// `e6 Boss: Health, Transform differ` · `e9 missing` · `e12 extra (Skeleton)`.
    pub fn summary(&self) -> String {
        let label = if self.kind.is_empty() { self.id.to_string() } else { format!("{} {}", self.id, self.kind) };
        match &self.change {
            EntityChange::Missing { gone: Some(gone) } => format!("{} missing ({gone})", self.id),
            EntityChange::Missing { gone: None } => format!("{} missing", self.id),
            EntityChange::Extra if self.kind.is_empty() => format!("{} extra", self.id),
            EntityChange::Extra => format!("{} extra ({})", self.id, self.kind),
            EntityChange::Differs { components, .. } if components.is_empty() => format!("{label} differs"),
            EntityChange::Differs { components, .. } => {
                let verb = if components.len() == 1 { "differs" } else { "differ" };
                format!("{label}: {} {verb}", components.join(", "))
            }
        }
    }
}

impl Divergence {
    /// `e6 Boss: Health, Transform differ; e9 missing; … and 7 more`, or
    /// `no details: replay format 2`.
    pub fn summary(&self) -> String {
        if let Some(why) = &self.no_details {
            return format!("no details: {why}");
        }
        let mut items: Vec<String> = self.entities.iter().map(EntityDiff::summary).collect();
        if self.more > 0 {
            items.push(format!("… and {} more", self.more));
        }
        items.extend(self.checks.iter().map(|c| format!("replay check {c}")));
        items.join("; ")
    }

    /// `e6 now: Health { current: 2075.0, max: 4000.0 }` for each listed component.
    pub fn value_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for entity in &self.entities {
            if let EntityChange::Differs { now, .. } = &entity.change {
                lines.extend(now.iter().map(|value| format!("{} now: {value}", entity.id)));
            }
        }
        lines
    }
}

struct Playback {
    replay: Replay,
    /// The recording's starting snapshot, until the game has loaded and it is put in.
    pending_start: Option<Snapshot>,
    /// Index of the next event to apply.
    next: usize,
    /// Input events due before the current tick, picked up by the typed input systems.
    due_inputs: Vec<(AiId, String, Value)>,
    matched: u32,
    diverged_at: Option<u64>,
    divergence: Option<Divergence>,
}

/// The recording of this session, and the playback in progress (if any).
#[derive(Resource)]
pub struct AiReplay {
    events: Vec<ReplayEvent>,
    checks: Vec<Check>,
    /// The snapshot this recording starts from (`None`: tick 0).
    start: Option<Snapshot>,
    /// Ticks between checks; a playback uses its recording's.
    check_every: u64,
    /// What each check keeps; a playback uses its recording's.
    check_detail: CheckDetail,
    /// Names of the registered input components.
    inputs: Vec<&'static str>,
    /// Per registered input component: learn the values it has now (after a snapshot
    /// replaced the entities, the remembered ones belong to entities that are gone).
    memory_resets: Vec<fn(&mut World)>,
    /// Per registered input component: move what is remembered of an entity to the one
    /// that stands for it now (a refused load put the same entities back as new ones).
    memory_follows: Vec<FollowMemory>,
    playback: Option<Playback>,
    report: Option<ReplayReport>,
    /// Set while playback itself calls tools, so its calls do not count as a takeover.
    replaying: bool,
    /// Action calls running right now. Calls made from inside another action are
    /// not recorded: replaying the outer one makes them again.
    action_depth: u32,
    /// Where the running outermost action call is recorded, for [`forget_call`].
    current_call: Option<usize>,
    /// This game does not record its session ([`set_recording`]).
    not_recording: bool,
}

impl Default for AiReplay {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            checks: Vec::new(),
            start: None,
            check_every: CHECK_EVERY,
            check_detail: CheckDetail::default(),
            inputs: Vec::new(),
            memory_resets: Vec::new(),
            memory_follows: Vec::new(),
            playback: None,
            report: None,
            replaying: false,
            action_depth: 0,
            current_call: None,
            not_recording: false,
        }
    }
}

impl AiReplay {
    pub fn is_playing(&self) -> bool {
        self.playback.is_some()
    }

    /// The recording being played back, for modules that prepare the world for it
    /// before tick 1 (content restores the versions a recording started from).
    pub fn playing(&self) -> Option<&Replay> {
        self.playback.as_ref().map(|p| &p.replay)
    }

    /// Ticks between checks (at least 1) and what each keeps, from now on. Set them
    /// before the first tick: a replay stores one interval, and playback compares only
    /// the checks made at its ticks.
    pub fn set_checks(&mut self, every: u64, detail: CheckDetail) {
        self.check_every = every.max(1);
        self.check_detail = detail;
    }

    /// The outcome of the last playback.
    pub fn report(&self) -> Option<&ReplayReport> {
        self.report.as_ref()
    }

    pub fn recorded_events(&self) -> usize {
        self.events.len()
    }

    /// The snapshot this recording starts from, if it does not start at tick 0.
    pub fn started_from(&self) -> Option<&Snapshot> {
        self.start.as_ref()
    }

    /// Whether this game records its session (see [`set_recording`]).
    pub fn is_recording(&self) -> bool {
        !self.not_recording
    }
}

/// Why `replay_save` refuses in a game that does not record.
pub const NOT_RECORDING: &str =
    "this game does not record its session: the host records every player's; save the replay there";

/// Turns the recording of this session on or off. Off (a client of a networked game, whose
/// session the host records): no calls, inputs or checks are recorded, `replay_save` refuses
/// and says why, no crash replay is written. Playback is not affected.
pub fn set_recording(world: &mut World, on: bool) {
    let mut replay = world.resource_mut::<AiReplay>();
    replay.not_recording = !on;
    if !on {
        replay.events.clear();
        replay.checks.clear();
        replay.current_call = None;
    }
}

/// The recording starts from the world as it is now: what [`snapshot_load`](crate::snapshot)
/// does after a load, for whoever else rebuilds the world (loading a saved game, a client
/// taking the host's world). A playback in progress ends, with `reason`. Returns how many
/// recorded events were dropped.
pub fn restart_from_current(world: &mut World, reason: &str) -> Result<usize, String> {
    let start = crate::snapshot::capture(world)?;
    if world.resource::<AiReplay>().playback.is_some() {
        interrupt(world, reason.to_owned());
    }
    Ok(restart_from(world, start, false))
}

/// A snapshot was loaded: the recording starts here now. What was recorded before is
/// dropped (a replay is one run from one start), a playback in progress is over when
/// `interrupt_playback` (someone loaded a snapshot into it; playback itself putting its
/// own start in does not), and the input memories learn the entities that are there now.
///
/// Session rules are not part of a snapshot: every rule is re-armed and the session's
/// rules are recorded again at the start, so playback from the snapshot checks the same
/// rules from the same tick.
pub(crate) fn restart_from(world: &mut World, start: Snapshot, interrupt_playback: bool) -> usize {
    if interrupt_playback && world.resource::<AiReplay>().playback.is_some() {
        interrupt(world, format!("a snapshot was loaded (tick {})", start.tick));
    }
    let rules = invariants::restarted(world);
    let at = start.tick;
    let mut replay = world.resource_mut::<AiReplay>();
    let dropped = replay.events.len();
    replay.events.clear();
    replay.checks.clear();
    replay.start = Some(start);
    replay.current_call = None;
    if !replay.not_recording {
        replay.events.extend(rules.into_iter().map(|op| ReplayEvent::Assert { at, op }));
    }
    let resets = replay.memory_resets.clone();
    for reset in resets {
        reset(world);
    }
    dropped
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
    // Whether a game records its session does not change what its playback does.
    let takeover = replay.playback.is_some() && !replay.replaying;
    if !replay.not_recording {
        replay.current_call = Some(replay.events.len());
        replay.events.push(ReplayEvent::Call {
            at,
            agent: caller.agent.clone(),
            scope: caller.scope,
            subject: caller.subject_id,
            tool: tool.to_owned(),
            args: args.clone(),
        });
    }
    if takeover {
        interrupt(world, format!("{} took over ({tool})", caller.agent));
    }
}

/// Called by [`call_tool`] after an action tool ran (panics are caught inside).
pub(crate) fn end_action(world: &mut World) {
    let mut replay = world.resource_mut::<AiReplay>();
    replay.action_depth = replay.action_depth.saturating_sub(1);
    if replay.action_depth == 0 {
        replay.current_call = None;
    }
}

/// Takes the running action call out of the recording: for a tool that refused before
/// it changed anything, so what playback reads from recorded calls comes only from
/// calls that applied (content restores a replay's starting definitions from the
/// `before` of its `content_apply` calls). A call made inside another action is not
/// recorded, so there is nothing to take.
pub fn forget_call(world: &mut World) {
    let mut replay = world.resource_mut::<AiReplay>();
    if replay.action_depth == 1
        && let Some(index) = replay.current_call.take()
    {
        replay.events.remove(index);
    }
}

/// A change to the session's rules: recorded like a call, but it never takes over a
/// playback (rules do not change the game).
pub(crate) fn record_assert(world: &mut World, op: AssertOp) {
    let at = sim::tick(world);
    if let Some(mut replay) = world.get_resource_mut::<AiReplay>().filter(|r| !r.not_recording) {
        replay.events.push(ReplayEvent::Assert { at, op });
    }
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

    if replay.not_recording {
        return;
    }
    // In id order, not query order: the window and other modules change how entities are
    // grouped, and the same session must record the same file with and without them.
    let mut changed: Vec<(AiId, &C)> = q
        .iter()
        .filter(|(entity, _, value)| {
            value.last_changed().is_newer_than(memory.since, ticks.this_run()) && memory.last.get(entity) != Some(&**value)
        })
        .map(|(_, id, value)| (*id, value.into_inner()))
        .collect();
    changed.sort_unstable_by_key(|(id, _)| *id);
    for (id, value) in changed {
        match serde_json::to_value(value) {
            Ok(v) => replay.events.push(ReplayEvent::Input { at: tick.0, id, component: name.to_owned(), value: v }),
            Err(e) => warn!("replay: cannot record {name} of {id}: {e}"),
        }
    }
}

/// Moves what an input component remembers from old entities to the ones that stand for them.
type FollowMemory = fn(&mut World, &HashMap<Entity, Entity>);

/// The entities were put back as they were, as new ones (`moved`: old → new): a refused
/// snapshot load undid what it had built. The input memories follow them, so an input
/// changed since the last tick is still recorded as one, and an input that is what it was
/// is not.
pub(crate) fn follow_entities(world: &mut World, moved: &HashMap<Entity, Entity>) {
    let follows = world.get_resource::<AiReplay>().map(|r| r.memory_follows.clone()).unwrap_or_default();
    for follow in follows {
        follow(world, moved);
    }
}

fn follow_memory<C: Component + Clone>(world: &mut World, moved: &HashMap<Entity, Entity>) {
    let mut memory = world.resource_mut::<InputMemory<C>>();
    let last = std::mem::take(&mut memory.last);
    memory.last = last.into_iter().map(|(entity, value)| (moved.get(&entity).copied().unwrap_or(entity), value)).collect();
}

/// After a snapshot replaced the entities: remember what the new ones hold, as of now.
fn reset_memory<C: Component + Clone>(world: &mut World) {
    let last: HashMap<Entity, C> = world.query::<(Entity, &C)>().iter(world).map(|(e, c)| (e, c.clone())).collect();
    // Writes made from here on are newer than this; what was restored is not.
    let since = world.increment_change_tick();
    let mut memory = world.resource_mut::<InputMemory<C>>();
    memory.last = last;
    memory.since = since;
}

/// Registers input component `C`. Prefer [`AiAppExt::replay_input`](crate::AiAppExt::replay_input).
pub fn register_input<C>(app: &mut App)
where
    C: Component<Mutability = Mutable> + Clone + PartialEq + Serialize + DeserializeOwned,
{
    {
        let mut replay = app.world_mut().get_resource_or_init::<AiReplay>();
        replay.inputs.push(crate::short_type_name::<C>());
        replay.memory_resets.push(reset_memory::<C>);
        replay.memory_follows.push(follow_memory::<C>);
    }
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
    /// `FixedLast`: state check every `check_every` ticks.
    Check,
    /// `FixedLast`: inputs as the tick left them.
    Remember,
}

// --- state hash ------------------------------------------------------------------

/// A game's own contribution to every replay check.
pub type ReplayCheck = fn(&World) -> u64;

/// A contribution to every replay check for state that may be absent: `None` adds nothing,
/// not even the check's name.
pub type OptionalReplayCheck = fn(&World) -> Option<u64>;

#[derive(Clone, Copy)]
enum HashedCheck {
    Always(ReplayCheck),
    WhenSome(OptionalReplayCheck),
}

/// What goes into [`state_hash`] besides AI-visible entities, and what stays out.
#[derive(Resource, Default)]
pub struct ReplayHashing {
    /// Components left out ([`AiAppExt::replay_ignore`](crate::AiAppExt::replay_ignore)).
    ignored: HashSet<TypeId>,
    /// Game data outside AI-visible entities
    /// ([`AiAppExt::add_replay_check`](crate::AiAppExt::add_replay_check)), by name.
    checks: Vec<(String, HashedCheck)>,
    /// Components already reported as not serializable.
    reported: HashSet<&'static str>,
}

impl ReplayHashing {
    pub fn ignore(&mut self, component: TypeId) {
        self.ignored.insert(component);
    }

    /// Whether component `component` is left out of replay checks
    /// ([`replay_ignore`](crate::AiAppExt::replay_ignore)).
    pub fn is_ignored(&self, component: TypeId) -> bool {
        self.ignored.contains(&component)
    }

    pub fn add_check(&mut self, name: &str, check: ReplayCheck) {
        self.push_check(name, HashedCheck::Always(check));
    }

    /// [`add_check`](Self::add_check) for state that may be absent (no level placed): while
    /// `check` returns `None` the hash is what it would be without the check. So the hash
    /// depends on the state only, never on whether the state existed before (a snapshot taken
    /// before the first level loads exactly after one came and went).
    pub fn add_optional_check(&mut self, name: &str, check: OptionalReplayCheck) {
        self.push_check(name, HashedCheck::WhenSome(check));
    }

    fn push_check(&mut self, name: &str, check: HashedCheck) {
        self.checks.retain(|(n, _)| n != name);
        self.checks.push((name.to_owned(), check));
    }
}

/// FNV-1a over the game state: for every AI-visible entity in `AiId` order, its id and
/// each gameplay component ([`is_gameplay_type`](crate::describe::is_gameplay_type),
/// the rule `inspect` uses), serialized by reflection, in type name order; then every
/// [`add_replay_check`](crate::AiAppExt::add_replay_check). Labels and describe
/// fragments are presentation and stay out: rewording a fragment or adding a
/// describer keeps old replays valid. Components only a window adds are engine types
/// and stay out too.
pub fn state_hash(world: &World) -> u64 {
    hash_state(world, CheckDetail::None).0.hash
}

/// The check of the current state, with its breakdown at `detail`, and the components
/// left out because they do not serialize. The breakdown comes from the same bytes in
/// the same pass; the hash is the same at every detail.
fn hash_state(world: &World, detail: CheckDetail) -> (Check, Vec<&'static str>) {
    let tick = sim::tick(world);
    let mut h = Fnv::new();
    h.u64(tick);
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let hashing = world.get_resource::<ReplayHashing>();
    let entity_ids = EntitiesAsIds(world);
    let mut parts = (detail != CheckDetail::None).then(Breakdown::default);
    let mut failed = Vec::new();
    let mut buf = Vec::new();
    let Some(index) = world.get_resource::<AiIndex>() else {
        return (Check { tick, hash: h.0, parts }, failed);
    };
    for id in index.ids() {
        let Some(entity) = index.get(id).filter(|e| world.get::<AiVisible>(*e).is_some()) else {
            continue;
        };
        h.u64(id.0 as u64);
        let mut whole = Fnv::new();
        whole.u64(id.0 as u64);
        let mut components = Vec::new();
        let gameplay = gameplay_components(world, &registry, entity);
        let mut keys = match detail {
            CheckDetail::Components => component_keys(&gameplay),
            _ => Vec::new(),
        }
        .into_iter();
        for (path, value) in &gameplay {
            let key = keys.next();
            let ignored = value
                .get_represented_type_info()
                .is_some_and(|info| hashing.is_some_and(|c| c.ignored.contains(&info.type_id())));
            if ignored {
                continue;
            }
            buf.clear();
            let serializer = TypedReflectSerializer::with_processor(*value, &registry, &entity_ids);
            match serde_json::to_writer(&mut buf, &serializer) {
                Ok(()) => {
                    h.bytes(path.as_bytes());
                    h.bytes(&buf);
                    match (detail, key) {
                        (CheckDetail::Components, Some(key)) => {
                            let mut one = Fnv::new();
                            one.bytes(path.as_bytes());
                            one.bytes(&buf);
                            components.push((key, one.0));
                        }
                        (CheckDetail::Entities, _) => {
                            whole.bytes(path.as_bytes());
                            whole.bytes(&buf);
                        }
                        _ => {}
                    }
                }
                Err(_) => failed.push(*path),
            }
        }
        if let Some(parts) = &mut parts {
            let mine = match detail {
                CheckDetail::Entities => EntityParts::Whole(whole.0),
                _ => {
                    components.sort();
                    EntityParts::Components(components)
                }
            };
            parts.entities.push((id, mine));
        }
    }
    for (name, check) in hashing.map(|c| c.checks.as_slice()).unwrap_or_default() {
        // An optional check that has nothing adds nothing: not to the hash, not to the parts.
        let value = match check {
            HashedCheck::Always(check) => check(world),
            HashedCheck::WhenSome(check) => match check(world) {
                Some(value) => value,
                None => continue,
            },
        };
        h.bytes(name.as_bytes());
        h.u64(value);
        if let Some(parts) = &mut parts {
            parts.game.push((name.clone(), value));
        }
    }
    failed.sort_unstable();
    failed.dedup();
    (Check { tick, hash: h.0, parts }, failed)
}

/// What each component is called in a breakdown: its short type name, or its full path
/// where another component of the entity has the same short name.
fn component_keys(components: &[(&'static str, &dyn PartialReflect)]) -> Vec<Cow<'static, str>> {
    let short = |value: &dyn PartialReflect| value.get_represented_type_info().map(|info| info.type_path_table().short_path());
    let shorts: Vec<Option<&'static str>> = components.iter().map(|(_, value)| short(*value)).collect();
    components
        .iter()
        .zip(&shorts)
        .map(|((path, _), name)| match name {
            Some(name) if shorts.iter().filter(|other| *other == &Some(*name)).count() == 1 => Cow::Borrowed(*name),
            _ => Cow::Borrowed(*path),
        })
        .collect()
}

/// The check of the current state at this session's detail, journaling (once each) the
/// components it had to leave out.
fn check_now(world: &mut World) -> Check {
    let detail = world.resource::<AiReplay>().check_detail;
    let (check, failed) = hash_state(world, detail);
    for path in failed {
        let new = world.get_resource_or_init::<ReplayHashing>().reported.insert(path);
        if new {
            let text = format!("replay checks leave out {path}: it does not serialize through reflection");
            journal::log(world, JournalKind::Warn, text);
        }
    }
    check
}

/// `Entity` values hash as the `AiId` they point to: render entities a window spawns
/// shift `Entity` indices, never ids.
struct EntitiesAsIds<'w>(&'w World);

/// What a processor returns: serialized, or the serializer handed back untouched.
type Processed<S> = Result<Result<<S as serde::Serializer>::Ok, S>, <S as serde::Serializer>::Error>;

impl ReflectSerializerProcessor for EntitiesAsIds<'_> {
    fn try_serialize<S>(&self, value: &dyn PartialReflect, _: &TypeRegistry, serializer: S) -> Processed<S>
    where
        S: serde::Serializer,
    {
        match value.try_downcast_ref::<Entity>() {
            Some(entity) => {
                let id = self.0.get::<AiId>(*entity).map_or_else(|| "untracked".to_owned(), ToString::to_string);
                serializer.serialize_str(&id).map(Ok)
            }
            None => Ok(Err(serializer)),
        }
    }
}

pub(crate) struct Fnv(pub(crate) u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
        // Length-terminated, so "ab"+"c" and "a"+"bc" differ.
        self.u64_raw(bytes.len() as u64);
    }

    pub(crate) fn u64(&mut self, v: u64) {
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
    if tick == 0 || !tick.is_multiple_of(world.resource::<AiReplay>().check_every) {
        return;
    }
    let replay = world.resource::<AiReplay>();
    if replay.not_recording && replay.playback.is_none() {
        return;
    }
    let check = check_now(world);
    let replay = world.resource::<AiReplay>();
    // Only the first divergence is explained: later ones follow from it.
    let found = replay.playback.as_ref().and_then(|playback| {
        let expected = playback.replay.checks.iter().find(|c| c.tick == tick)?;
        let first = expected.hash != check.hash && playback.diverged_at.is_none();
        Some((expected.hash == check.hash, first.then(|| divergence(world, &playback.replay, expected, &check))))
    });
    let mut replay = world.resource_mut::<AiReplay>();
    if !replay.not_recording {
        replay.checks.push(check);
    }
    let (Some(playback), Some((matched, found))) = (&mut replay.playback, found) else {
        return;
    };
    if matched {
        playback.matched += 1;
    }
    let Some(found) = found else {
        return;
    };
    playback.diverged_at = Some(tick);
    playback.divergence = Some(found.clone());
    journal_divergence(world, &found);
}

/// `replay diverged at tick 60: e6 Boss: Health differs`, then a line per value.
fn journal_divergence(world: &mut World, found: &Divergence) {
    journal::log(world, JournalKind::Error, format!("replay diverged at tick {}: {}", found.tick, found.summary()));
    for line in found.value_lines() {
        journal::log(world, JournalKind::Sim, format!("replay: {line}"));
    }
}

/// What differs between the recorded check and this run's, both at the same tick.
fn divergence(world: &World, replay: &Replay, recorded: &Check, live: &Check) -> Divergence {
    let mut found = Divergence { tick: live.tick, entities: Vec::new(), more: 0, checks: Vec::new(), no_details: None };
    let (Some(then), Some(now)) = (&recorded.parts, &live.parts) else {
        found.no_details = Some(if replay.format == FORMAT_V2 {
            "replay format 2".to_owned()
        } else {
            format!("the recording keeps hashes only (check detail {})", replay.check_detail)
        });
        return found;
    };

    // Both lists are in id order.
    enum Side {
        Recorded,
        Live,
        Both(Vec<String>),
    }
    let mut changed: Vec<(AiId, Side)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    loop {
        match (then.entities.get(i), now.entities.get(j)) {
            (Some((was, before)), Some((is, after))) if was == is => {
                if let Some(components) = differing(before, after) {
                    changed.push((*was, Side::Both(components)));
                }
                i += 1;
                j += 1;
            }
            (Some((was, _)), Some((is, _))) if was < is => {
                changed.push((*was, Side::Recorded));
                i += 1;
            }
            (Some((was, _)), None) => {
                changed.push((*was, Side::Recorded));
                i += 1;
            }
            (_, Some((is, _))) => {
                changed.push((*is, Side::Live));
                j += 1;
            }
            (None, None) => break,
        }
    }
    let index = world.resource::<AiIndex>();
    found.more = changed.len().saturating_sub(MAX_LISTED);
    for (id, side) in changed.into_iter().take(MAX_LISTED) {
        let entity = index.get(id);
        let kind = entity.and_then(|e| world.get::<AiKind>(e)).map(|k| k.0.to_string()).unwrap_or_default();
        let change = match side {
            Side::Both(components) => {
                let now = entity.map(|e| live_values(world, e, &components)).unwrap_or_default();
                EntityChange::Differs { components, now }
            }
            Side::Live => EntityChange::Extra,
            Side::Recorded => EntityChange::Missing {
                gone: index.tombstone(id).map(|t| match t.renumbered_to {
                    Some(new) => format!("{} renumbered to {new} at tick {}", t.label, t.tick),
                    None => format!("{} despawned at tick {}", t.label, t.tick),
                }),
            },
        };
        found.entities.push(EntityDiff { id, kind, change });
    }

    for (name, value) in &then.game {
        match now.game.iter().find(|(n, _)| n == name) {
            Some((_, v)) if v == value => {}
            Some(_) => found.checks.push(format!("{name} differs")),
            None => found.checks.push(format!("{name} missing")),
        }
    }
    for (name, _) in now.game.iter().filter(|(n, _)| !then.game.iter().any(|(m, _)| m == n)) {
        found.checks.push(format!("{name} extra"));
    }
    if found.entities.is_empty() && found.checks.is_empty() {
        found.no_details = Some("every entity and replay check matches its recorded hash".to_owned());
    }
    found
}

/// The components whose hashes differ (or that one side lacks); `None` if none do.
/// Empty when the recording has a hash per entity only.
fn differing(before: &EntityParts, after: &EntityParts) -> Option<Vec<String>> {
    match (before, after) {
        (EntityParts::Components(then), EntityParts::Components(now)) => {
            let mut names: Vec<String> = then
                .iter()
                .filter(|(name, hash)| !now.iter().any(|(n, h)| n == name && h == hash))
                .chain(now.iter().filter(|(name, _)| !then.iter().any(|(n, _)| n == name)))
                .map(|(name, _)| name.to_string())
                .collect();
            names.sort();
            names.dedup();
            (!names.is_empty()).then_some(names)
        }
        (EntityParts::Whole(then), EntityParts::Whole(now)) if then == now => None,
        _ => Some(Vec::new()),
    }
}

/// What `entity` holds now for each named component, one line each:
/// `Health { current: 2075.0, max: 4000.0 }`, or `no Telegraph`.
fn live_values(world: &World, entity: Entity, names: &[String]) -> Vec<String> {
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let gameplay = gameplay_components(world, &registry, entity);
    let keys = component_keys(&gameplay);
    names
        .iter()
        .map(|name| match keys.iter().position(|key| key == name) {
            Some(i) => one_line(&without_paths(&format!("{:?}", gameplay[i].1))),
            None => format!("no {name}"),
        })
        .collect()
}

/// `sandbox::gameplay::combat::Health { current: 1.0 }` → `Health { current: 1.0 }`.
/// String values (`"a::b"`) stay as they are.
fn without_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Where the identifier being written started: a `::` after it drops it.
    let mut word = 0;
    let mut quoted = false;
    let mut escaped = false;
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        if !quoted
            && let Some(after) = rest.strip_prefix("::")
        {
            out.truncate(word);
            rest = after;
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
        if quoted {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => quoted = false,
                _ => {}
            }
        } else if c == '"' {
            quoted = true;
        }
        if !(c.is_alphanumeric() || c == '_') {
            word = out.len();
        }
    }
    out
}

/// At most [`MAX_VALUE_CHARS`] characters on one line.
fn one_line(text: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(MAX_VALUE_CHARS) {
        Some((cut, _)) => format!("{}…", &flat[..cut]),
        None => flat,
    }
}

// --- playback --------------------------------------------------------------------

/// Starts playing `replay` back. The world must be fresh (tick 0, nothing recorded
/// yet) and built with the replay's seed and tick rate. Events are applied as ticks
/// run; step or resume the game to play it. Start it before the game loads when you
/// can (right after building the app): modules then prepare the world before it is
/// spawned (content restores the definitions the recording started from). A recording
/// that starts from a snapshot has it put in as soon as the game has loaded, before the
/// first tick: the world the game spawned is replaced by the snapshot's.
pub fn play(world: &mut World, replay: Replay) -> Result<(), String> {
    if replay.format == FORMAT_V1 {
        return Err(format!(
            "this replay was recorded by an older build ({FORMAT_V1}: its checks hash the text the AI reads, \
             not the game state), so this build cannot check it; record it again"
        ));
    }
    if replay.format != FORMAT && replay.format != FORMAT_V2 {
        return Err(format!(
            "not a replay this build can play: format `{}`, expected `{FORMAT}` (or `{FORMAT_V2}`)",
            replay.format
        ));
    }
    if replay.start.is_some() && replay.format != FORMAT {
        return Err(format!("a replay that starts from a snapshot is `{FORMAT}`, this one says `{}`", replay.format));
    }
    if replay.check_every == 0 {
        return Err("the replay says it checks every 0 ticks; it needs at least 1".into());
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
    if let Some(start) = &replay.start {
        if start.seed != replay.seed || (start.tick_hz - replay.tick_hz).abs() > 1e-3 {
            return Err(format!(
                "the replay's snapshot is from seed {} at {} ticks/s, the replay itself from seed {} at {}",
                start.seed, start.tick_hz, replay.seed, replay.tick_hz
            ));
        }
        if start.tick > replay.end.tick {
            return Err(format!("the replay ends at tick {}, before its snapshot (tick {})", replay.end.tick, start.tick));
        }
    }
    let from = replay.start.as_ref().map_or(0, |s| s.tick);
    let mut last_at = from;
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
    let pending_start = replay.start.clone();
    let mut state = world.resource_mut::<AiReplay>();
    state.report = None;
    // Check at the recording's ticks with its detail. The recording made from here on
    // keeps them too, so a branch saves with the same checks throughout.
    state.check_every = replay.check_every;
    if replay.format != FORMAT_V2 {
        state.check_detail = replay.check_detail;
    }
    state.playback = Some(Playback {
        replay,
        pending_start,
        next: 0,
        due_inputs: Vec::new(),
        matched: 0,
        diverged_at: None,
        divergence: None,
    });
    let text = match from {
        0 => format!("replay started: {end} ticks"),
        from => format!("replay started: ticks {from} to {end}, from a snapshot"),
    };
    journal::log(world, JournalKind::Sim, text);
    Ok(())
}

/// Applies the calls due after the current tick and queues its inputs; at the end
/// of the recording, compares the final state and finishes. Runs before every tick
/// and once per frame (so a paused game finishes right at the last tick), but not
/// while the game loads: a recording that ends at tick 0 finishes on the spawned world.
fn advance_playback(world: &mut World, at_tick_start: bool) {
    let end = match &world.resource::<AiReplay>().playback {
        Some(p) => p.replay.end.tick,
        None => return,
    };
    // Once the game has loaded (its own world is spawned), before anything else.
    if world.resource::<AiReplay>().playback.as_ref().is_some_and(|p| p.pending_start.is_some())
        && sim::gate_message(world).is_none()
    {
        apply_start(world);
        if world.resource::<AiReplay>().playback.is_none() {
            return;
        }
    }
    let tick = sim::tick(world);
    if !at_tick_start && (tick != end || sim::gate_message(world).is_some()) {
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
            ReplayEvent::Assert { op, .. } => {
                if let Err(e) = invariants::apply(world, &op) {
                    // The run has gone its own way already (an entity the rule names is
                    // missing): the checks report it.
                    journal::log(world, JournalKind::Warn, format!("replay: rule change at tick {tick} failed: {e}"));
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

/// Puts the recording's starting snapshot in. A snapshot this game cannot load (other
/// content, another build) ends the playback with the reason: it cannot be played.
fn apply_start(world: &mut World) {
    let Some(start) = world.resource_mut::<AiReplay>().playback.as_mut().and_then(|p| p.pending_start.take()) else {
        return;
    };
    let tick = start.tick;
    match snapshot::restore(world, &start, false) {
        Ok(report) => {
            // The playback's recording starts where the recording being played did.
            restart_from(world, start, false);
            journal::log(world, JournalKind::Sim, format!("replay starts from its snapshot at tick {tick}: {report}"));
            if !report.exact() {
                // The game is not where the recording was: report it from the first tick.
                journal::log(
                    world,
                    JournalKind::Error,
                    format!("replay diverged at tick {tick}: {}", report.explain_differences()),
                );
                // `--verify` prints the headline, not the journal: it says what the restore found.
                let found: Vec<&str> = report.differences.iter().take(3).map(String::as_str).collect();
                let more = report.differences.len().saturating_sub(found.len());
                let mut why = format!("the start does not restore exactly: {}", found.join("; "));
                if more > 0 {
                    why.push_str(&format!("; … (+{more})"));
                }
                if let Some(playback) = &mut world.resource_mut::<AiReplay>().playback {
                    playback.diverged_at.get_or_insert(tick);
                    playback.divergence.get_or_insert(Divergence {
                        tick,
                        entities: Vec::new(),
                        more: 0,
                        checks: Vec::new(),
                        no_details: Some(why),
                    });
                }
            }
        }
        Err(why) => {
            journal::log(world, JournalKind::Error, format!("replay cannot start from its snapshot (tick {tick}): {why}"));
            interrupt(world, format!("cannot start from its snapshot (tick {tick}): {why}"));
        }
    }
}

/// One recorded call, as its agent with the recorded scope and body; the agent's
/// live session is restored afterwards.
fn replay_call(world: &mut World, agent: &str, scope: Scope, subject: Option<AiId>, tool: &str, args: Value) {
    let saved = world.resource_mut::<AgentSessions>().get_or_create(agent).clone();
    // The recorded call ran as `subject`, never as a player body the live session is bound to.
    world.resource_mut::<AgentSessions>().configure(agent, scope, subject).player = None;
    world.resource_mut::<AiReplay>().replaying = true;
    call_tool(world, agent, tool, args);
    world.resource_mut::<AiReplay>().replaying = false;
    world.resource_mut::<AgentSessions>().configure(agent, saved.scope, saved.subject).player = saved.player;
}

fn finish(world: &mut World) {
    let check = check_now(world);
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
        divergence: playback.divergence,
    };
    if check.hash == playback.replay.end.hash {
        report.matched += 1;
    } else if report.diverged_at.is_none() {
        let found = divergence(world, &playback.replay, &playback.replay.end, &check);
        journal_divergence(world, &found);
        report.diverged_at = Some(tick);
        report.divergence = Some(found);
    }
    let kind = if report.identical() { JournalKind::Sim } else { JournalKind::Error };
    journal::log(world, kind, report.headline());
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
        divergence: playback.divergence,
    };
    journal::log(world, JournalKind::Sim, report.headline());
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
        check_every: replay.check_every,
        check_detail: replay.check_detail,
        start: replay.start.clone(),
        end: hash_state(world, replay.check_detail).0,
        events: replay.events.clone(),
        checks: replay.checks.clone(),
    }
}

/// Reads a replay file, or the replay inside a [crash report](crate::crash).
pub fn load(path: &std::path::Path) -> Result<Replay, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    #[derive(Deserialize)]
    struct Format {
        format: String,
    }
    #[derive(Deserialize)]
    struct Report {
        replay: Option<Replay>,
        no_replay: Option<String>,
    }
    if serde_json::from_str::<Format>(&text).is_ok_and(|f| f.format == crate::crash::FORMAT) {
        let report: Report = serde_json::from_str(&text).map_err(|e| format!("{} is a crash report whose replay cannot be read: {e}", path.display()))?;
        return report.replay.ok_or_else(|| {
            let why = report.no_replay.unwrap_or_else(|| "none was recorded".to_owned());
            format!("{} is a crash report without a replay ({why})", path.display())
        });
    }
    serde_json::from_str(&text).map_err(|e| format!("{} is not a replay: {e}", path.display()))
}

/// Recording and playback state for `ai/status`.
pub fn status(world: &World) -> Value {
    let replay = world.resource::<AiReplay>();
    json!({
        "recorded_events": replay.events.len(),
        "starts_at_tick": replay.start.as_ref().map(|s| s.tick),
        "checks": {"every": replay.check_every, "detail": replay.check_detail},
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
    if !world.resource::<AiReplay>().is_recording() {
        return Err(NOT_RECORDING.to_owned());
    }
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

pub(crate) fn plugin(app: &mut App, check_every: u64, check_detail: CheckDetail) {
    app.init_resource::<AiReplay>();
    app.world_mut().resource_mut::<AiReplay>().set_checks(check_every, check_detail);
    app.init_resource::<ReplayHashing>()
        .configure_sets(
            FixedFirst,
            (ReplaySystems::Calls, ReplaySystems::Inputs).chain().before(sim::advance_tick),
        )
        .configure_sets(FixedLast, (ReplaySystems::Check, ReplaySystems::Remember).chain())
        .add_systems(FixedFirst, (|w: &mut World| advance_playback(w, true)).in_set(ReplaySystems::Calls))
        .add_systems(FixedLast, check_state.in_set(ReplaySystems::Check))
        .add_systems(Last, |w: &mut World| advance_playback(w, false));
    register_tool(app.world_mut(), "replay_save", Scope::Dev, ToolKind::Query, replay_save_tool);
    // The host records every player's session.
    let _ = app
        .world_mut()
        .resource_mut::<crate::AiTools>()
        .set_route("replay_save", crate::tools::NetRoute::HostOnly(crate::tools::ON_THE_HOST));
}

pub(crate) mod hex_u64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::hex(*v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let text = String::deserialize(d)?;
        super::parse_hex(&text).map_err(serde::de::Error::custom)
    }
}

fn hex(v: u64) -> String {
    format!("{v:016x}")
}

fn parse_hex(text: &str) -> Result<u64, String> {
    u64::from_str_radix(text, 16).map_err(|e| format!("`{text}` is not a hex hash: {e}"))
}

/// A hex hash (`"…"`, [`CheckDetail::Entities`]) or component name → hex hash.
impl Serialize for EntityParts {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            EntityParts::Whole(hash) => s.serialize_str(&hex(*hash)),
            EntityParts::Components(components) => {
                let mut map = s.serialize_map(Some(components.len()))?;
                for (name, hash) in components {
                    map.serialize_entry(name, &hex(*hash))?;
                }
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for EntityParts {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Parts;
        impl<'de> Visitor<'de> for Parts {
            type Value = EntityParts;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a hex hash, or a map of component names to hex hashes")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<EntityParts, E> {
                parse_hex(text).map(EntityParts::Whole).map_err(E::custom)
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<EntityParts, A::Error> {
                let mut components = Vec::new();
                while let Some((name, hash)) = map.next_entry::<String, String>()? {
                    components.push((Cow::Owned(name), parse_hex(&hash).map_err(de::Error::custom)?));
                }
                components.sort();
                Ok(EntityParts::Components(components))
            }
        }
        d.deserialize_any(Parts)
    }
}

/// Entity parts as a map keyed by id: `{"e6": …}`, read back in id order.
mod by_id {
    use super::*;

    pub fn serialize<S: Serializer>(entities: &[(AiId, EntityParts)], s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(entities.len()))?;
        for (id, parts) in entities {
            map.serialize_entry(&id.to_string(), parts)?;
        }
        map.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<(AiId, EntityParts)>, D::Error> {
        struct Entities;
        impl<'de> Visitor<'de> for Entities {
            type Value = Vec<(AiId, EntityParts)>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map of entity ids (e6) to their hashes")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entities = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    let id = AiId::parse(&key).ok_or_else(|| de::Error::custom(format!("`{key}` is not an entity id")))?;
                    entities.push((id, map.next_value()?));
                }
                entities.sort_by_key(|(id, _)| *id);
                Ok(entities)
            }
        }
        d.deserialize_map(Entities)
    }
}

/// `add_replay_check` values as a map of name → hex, in the order they were hashed.
mod by_name {
    use super::*;

    pub fn serialize<S: Serializer>(values: &[(String, u64)], s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(values.len()))?;
        for (name, value) in values {
            map.serialize_entry(name, &hex(*value))?;
        }
        map.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<(String, u64)>, D::Error> {
        struct Values;
        impl<'de> Visitor<'de> for Values {
            type Value = Vec<(String, u64)>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map of replay check names to hex values")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some((name, value)) = map.next_entry::<String, String>()? {
                    values.push((name, parse_hex(&value).map_err(de::Error::custom)?));
                }
                Ok(values)
            }
        }
        d.deserialize_map(Values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_and_checks_have_a_readable_json_shape() {
        let check = Check { tick: 60, hash: 0xdead_beef_0123_4567, parts: None };
        assert_eq!(serde_json::to_value(&check).unwrap(), json!({"tick": 60, "hash": "deadbeef01234567"}));
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
        let mut r =
            ReplayReport { tick: 600, end_tick: 600, matched: 11, diverged_at: None, interrupted: None, divergence: None };
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

    #[derive(Component, Clone, PartialEq, Serialize, Deserialize, Default)]
    struct Knob(u32);

    fn game() -> App {
        use crate::AiAppExt;
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: crate::SimConfig { start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .replay_input::<Knob>();
        app.world_mut().spawn((crate::AiKind::new("Dial"), Knob(0), Transform::default()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn turn_and_run(app: &mut App, to: u32, ticks: u64) {
        let dial = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        app.world_mut().get_mut::<Knob>(dial).unwrap().0 = to;
        let out = crate::call_tool(app.world_mut(), "dev", "teleport", json!({"id": 1, "x": to, "z": 0}));
        assert!(out.ok, "{}", out.result);
        sim::step(app.world_mut(), ticks, std::time::Duration::from_secs(5), |_| None).unwrap();
    }

    #[test]
    fn a_game_that_does_not_record_records_nothing_and_says_why() {
        let mut app = game();
        set_recording(app.world_mut(), false);
        turn_and_run(&mut app, 3, CHECK_EVERY * 2);
        let replay = app.world().resource::<AiReplay>();
        assert!(!replay.is_recording());
        assert_eq!((replay.recorded_events(), replay.checks.len()), (0, 0));
        let out = crate::call_tool(app.world_mut(), "dev", "replay_save", json!({}));
        assert!(!out.ok && out.result.as_str().unwrap().contains("the host records"), "{}", out.result);

        set_recording(app.world_mut(), true);
        turn_and_run(&mut app, 4, CHECK_EVERY);
        let replay = app.world().resource::<AiReplay>();
        assert_eq!((replay.recorded_events(), replay.checks.len()), (2, 1), "a call and an input, one check");

        let dropped = restart_from_current(app.world_mut(), "a saved game was loaded").unwrap();
        assert_eq!(dropped, 2);
        let replay = app.world().resource::<AiReplay>();
        assert_eq!(replay.started_from().map(|s| s.tick), Some(CHECK_EVERY * 3));
        assert_eq!(replay.recorded_events(), 0);
    }

    #[test]
    fn a_takeover_ends_playback_even_when_the_game_does_not_record() {
        let mut app = game();
        // A recording of a fresh game that ends later: nothing to replay but the ticks.
        let mut replay = save(app.world());
        replay.end.tick = 120;
        set_recording(app.world_mut(), false);
        play(app.world_mut(), replay).unwrap();
        assert!(app.world().resource::<AiReplay>().is_playing());

        let out = crate::call_tool(app.world_mut(), "dev", "teleport", json!({"id": 1, "x": 3, "z": 0}));
        assert!(out.ok, "{}", out.result);
        let state = app.world().resource::<AiReplay>();
        assert!(!state.is_playing(), "the action interrupted the playback");
        assert_eq!(state.report().and_then(|r| r.interrupted.as_deref()), Some("dev took over (teleport)"));
        assert_eq!(state.recorded_events(), 0, "and nothing was recorded");
    }

    #[derive(Component)]
    struct Shiny;

    /// The inputs of one tick are recorded in id order, whatever order a query meets them in
    /// (a window's render components regroup entities; the file must not change with them).
    #[test]
    fn inputs_of_a_tick_are_recorded_in_id_order() {
        let mut app = game();
        let second = app.world_mut().spawn((crate::AiKind::new("Dial"), Knob(0), Transform::default())).id();
        app.update();
        let first = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        // Out of its table and back in: the first dial now sits after the second.
        app.world_mut().entity_mut(first).insert(Shiny);
        app.world_mut().entity_mut(first).remove::<Shiny>();
        let world = app.world_mut();
        let met: Vec<AiId> = world.query_filtered::<&AiId, With<Knob>>().iter(world).copied().collect();
        assert_eq!(met, [AiId(2), AiId(1)], "the test needs query order to differ from id order");
        app.world_mut().get_mut::<Knob>(second).unwrap().0 = 7;
        app.world_mut().get_mut::<Knob>(first).unwrap().0 = 5;
        sim::step(app.world_mut(), 1, std::time::Duration::from_secs(5), |_| None).unwrap();
        let recorded: Vec<(AiId, serde_json::Value)> = app
            .world()
            .resource::<AiReplay>()
            .events
            .iter()
            .filter_map(|e| match e {
                ReplayEvent::Input { id, value, .. } => Some((*id, value.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(recorded, [(AiId(1), json!(5)), (AiId(2), json!(7))]);
    }

    #[test]
    fn breakdowns_read_back_as_written() {
        let check = Check {
            tick: 60,
            hash: 1,
            parts: Some(Breakdown {
                entities: vec![
                    (AiId(6), EntityParts::Components(vec![("Health".into(), 0xa), ("Transform".into(), 0xb)])),
                    (AiId(12), EntityParts::Whole(0xc)),
                ],
                game: vec![("weather".into(), 0xd)],
            }),
        };
        let v = serde_json::to_value(&check).unwrap();
        assert_eq!(
            v["parts"],
            json!({
                "entities": {
                    "e6": {"Health": "000000000000000a", "Transform": "000000000000000b"},
                    "e12": "000000000000000c",
                },
                "game": {"weather": "000000000000000d"},
            })
        );
        // Maps come back in id and name order, whatever order a tool wrote them in.
        let back: Check = serde_json::from_value(v).unwrap();
        assert_eq!(back, check);
        let shuffled = json!({"tick": 60, "hash": "01", "parts": {"entities": {
            "e12": "0c", "e6": {"Transform": "0b", "Health": "0a"}
        }, "game": {"weather": "0d"}}});
        assert_eq!(serde_json::from_value::<Check>(shuffled).unwrap(), check);
        let bad = json!({"tick": 60, "hash": "01", "parts": {"entities": {"boss": "0c"}}});
        assert!(serde_json::from_value::<Check>(bad).unwrap_err().to_string().contains("`boss` is not an entity id"));
    }

    #[test]
    fn version_2_files_read_with_hashes_only() {
        let v2 = json!({
            "format": FORMAT_V2, "seed": 7, "tick_hz": 60.0,
            "end": {"tick": 120, "hash": "02"},
            "events": [],
            "checks": [{"tick": 60, "hash": "01"}],
        });
        let replay: Replay = serde_json::from_value(v2).unwrap();
        assert_eq!((replay.check_every, replay.check_detail), (CHECK_EVERY, CheckDetail::None));
        assert_eq!(replay.checks, vec![Check { tick: 60, hash: 1, parts: None }]);
    }

    /// One version 3: a file with a start and no check settings (written before the check
    /// settings existed) and one with check settings and no start both read, each without
    /// what it does not say.
    #[test]
    fn version_3_files_read_without_what_they_do_not_say() {
        let no_settings = json!({
            "format": FORMAT, "seed": 7, "tick_hz": 60.0,
            "end": {"tick": 120, "hash": "02"},
            "events": [{"input": {"at": 0, "id": 1, "component": "Knob", "value": 3}}],
            "checks": [{"tick": 60, "hash": "01"}],
        });
        let replay: Replay = serde_json::from_value(no_settings).unwrap();
        assert_eq!((replay.check_every, replay.check_detail, replay.start.is_none()), (CHECK_EVERY, CheckDetail::None, true));
        let settings = json!({
            "format": FORMAT, "seed": 7, "tick_hz": 60.0, "check_every": 30, "check_detail": "entities",
            "end": {"tick": 60, "hash": "02", "parts": {"entities": {"e1": "0a"}}},
            "events": [{"assert": {"at": 0, "op": {"add": {"name": "hp", "when": "error"}}}}],
            "checks": [{"tick": 30, "hash": "01", "parts": {"entities": {"e1": "0b"}}}],
        });
        let replay: Replay = serde_json::from_value(settings).unwrap();
        assert_eq!((replay.check_every, replay.check_detail, replay.start.is_none()), (30, CheckDetail::Entities, true));
        let written = serde_json::to_value(&replay).unwrap();
        assert!(written.get("start").is_none(), "no start, no field: {written}");
    }

    #[test]
    fn values_lose_module_paths_and_fit_one_line() {
        assert_eq!(
            without_paths("sandbox::gameplay::combat::Health { current: 2075.0, max: 4000.0 }"),
            "Health { current: 2075.0, max: 4000.0 }"
        );
        assert_eq!(
            without_paths("Some(glam::Vec3 { x: 1.0 }) Vec<a::B<c::D>>"),
            "Some(Vec3 { x: 1.0 }) Vec<B<D>>"
        );
        assert_eq!(without_paths(r#"Name("a::b \" c::d")"#), r#"Name("a::b \" c::d")"#);
        assert_eq!(one_line("Health {\n    current: 1.0,\n}"), "Health { current: 1.0, }");
        let long = one_line(&"x".repeat(MAX_VALUE_CHARS + 10));
        assert_eq!(long.chars().count(), MAX_VALUE_CHARS + 1);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn divergences_name_entities_components_and_the_rest() {
        let differs = |id: u32, kind: &str, components: &[&str]| EntityDiff {
            id: AiId(id),
            kind: kind.into(),
            change: EntityChange::Differs {
                components: components.iter().map(|c| c.to_string()).collect(),
                now: vec![format!("{} {{ .. }}", components.first().unwrap_or(&"?"))],
            },
        };
        let d = Divergence {
            tick: 60,
            entities: vec![
                differs(6, "Boss", &["Health", "Transform"]),
                differs(7, "Pillar", &["Transform"]),
                EntityDiff { id: AiId(9), kind: String::new(), change: EntityChange::Missing { gone: None } },
                EntityDiff {
                    id: AiId(10),
                    kind: String::new(),
                    change: EntityChange::Missing { gone: Some("Skeleton despawned at tick 40".into()) },
                },
                EntityDiff { id: AiId(12), kind: "Skeleton".into(), change: EntityChange::Extra },
                EntityDiff { id: AiId(13), kind: "Bat".into(), change: EntityChange::Differs { components: vec![], now: vec![] } },
            ],
            more: 7,
            checks: vec!["weather differs".into()],
            no_details: None,
        };
        assert_eq!(
            d.summary(),
            "e6 Boss: Health, Transform differ; e7 Pillar: Transform differs; e9 missing; \
             e10 missing (Skeleton despawned at tick 40); e12 extra (Skeleton); e13 Bat differs; \
             … and 7 more; replay check weather differs"
        );
        assert_eq!(d.value_lines(), vec!["e6 now: Health { .. }", "e7 now: Transform { .. }"]);

        let mut r = ReplayReport {
            tick: 600,
            end_tick: 600,
            matched: 0,
            diverged_at: Some(60),
            interrupted: None,
            divergence: Some(d.clone()),
        };
        assert!(r.headline().starts_with("replay finished at tick 600: DIVERGED at tick 60: e6 Boss: Health, Transform differ;"));
        assert!(r.to_string().ends_with("\n  e6 now: Health { .. }\n  e7 now: Transform { .. }"));

        let v2 = Divergence { entities: vec![], more: 0, checks: vec![], no_details: Some("replay format 2".into()), ..d };
        r.divergence = Some(v2);
        assert_eq!(r.to_string(), "replay finished at tick 600: DIVERGED at tick 60: no details: replay format 2");
    }

    /// Who a tool saw as the caller's body.
    #[derive(Resource, Default)]
    struct SeenBodies(Vec<Option<AiId>>);

    fn who(In(call): In<Call<crate::tools::NoArgs>>, world: &mut World) -> ToolResult {
        world.resource_mut::<SeenBodies>().0.push(call.caller.subject_id);
        Ok("ok".into())
    }

    /// Player 1 plays the first dial.
    fn first_dial_is_player_one(world: &World, n: u8) -> Option<AiId> {
        (n == 1).then(|| world.resource::<AiIndex>().get(AiId(1)).map(|_| AiId(1))).flatten()
    }

    /// A replayed call runs as its recorded body, and the live session, a player binding
    /// included, is exactly what it was before it.
    #[test]
    fn a_replayed_call_restores_a_player_bound_session() {
        let mut app = game();
        let world = app.world_mut();
        world.spawn((crate::AiKind::new("Hero"), crate::AiPlayer));
        world.init_resource::<SeenBodies>();
        register_tool(world, "who", Scope::Player, ToolKind::Query, who);
        world.resource_mut::<AgentSessions>().set_player_resolver(first_dial_is_player_one);

        let out = call_tool(world, "p1", "possess", json!({"player": 1}));
        assert!(out.ok, "{}", out.result);
        let session = |world: &World| {
            let s = world.resource::<AgentSessions>().get("p1").unwrap();
            (s.scope, s.subject, s.player)
        };
        let bound = (Scope::Dev, None, Some(1));
        assert_eq!(session(world), bound);

        // Recorded with the hero (e2) as the body: the player binding must not decide it ...
        replay_call(world, "p1", Scope::Dev, Some(AiId(2)), "who", json!({}));
        assert_eq!(session(world), bound, "the binding to player 1 survives a replayed call");
        // ... nor when the recording had the default body (the AiPlayer entity, e2).
        replay_call(world, "p1", Scope::Dev, None, "who", json!({}));
        assert_eq!(session(world), bound);
        assert_eq!(world.resource::<SeenBodies>().0, [Some(AiId(2)), Some(AiId(2))], "never player 1's body e1");

        // The live binding itself still resolves to its body afterwards.
        call_tool(world, "p1", "who", json!({}));
        assert_eq!(world.resource::<SeenBodies>().0[2], Some(AiId(1)));
    }
}
