//! Built-in tools every game gets: perception queries, sim control, reflection edits.

use std::time::Duration;

use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::{GetPath, PartialReflect, ReflectFromReflect, ReflectPath, TypeRegistration, TypeRegistry};
use schemars::JsonSchema;
use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::describe::facts;
use crate::id::{AiId, AI_ID_IS_MANAGED};
use crate::journal::{self, AiJournal, JournalKind};
use crate::observe::{self, entity_line, ObserveOptions};
use crate::session::{configure_session, AgentSessions, Scope, SessionChange};
use crate::observe::Digest;
use crate::sim::{self, ClockAction, SimGate, StepEnd, StepPoll, StepReport, StepRequest, FINISHED_KEPT};
use crate::tools::CallOutcome;
use crate::spatial::{self, relative, EYE_HEIGHT};
use crate::tools::{register_tool, AiTools, Call, Id, NoArgs, ToolKind, ToolResult};

pub(crate) fn register(world: &mut World) {
    use Scope::{Dev, Player};
    use ToolKind::{Action, Control, Query};
    register_tool(world, "observe", Player, Query, observe_tool);
    register_tool(world, "inspect", Player, Query, inspect_tool);
    register_tool(world, "nearby", Player, Query, nearby_tool);
    register_tool(world, "line_of_sight", Player, Query, line_of_sight_tool);
    register_tool(world, "journal", Player, Query, journal_tool);
    register_tool(world, "tools", Player, Query, tools_tool);

    register_tool(world, "pause", Dev, Control, pause_tool);
    register_tool(world, "resume", Dev, Control, resume_tool);
    register_tool(world, "speed", Dev, Control, speed_tool);
    register_tool(world, "step", Dev, Control, step_tool);
    register_tool(world, "step_poll", Dev, Query, step_poll_tool);
    register_tool(world, "possess", Dev, Control, possess_tool);
    register_tool(world, "teleport", Dev, Action, teleport_tool);
    register_tool(world, "despawn", Dev, Action, despawn_tool);
    register_tool(world, "get", Dev, Query, get_tool);
    register_tool(world, "set", Dev, Action, set_tool);
}

// --- perception ------------------------------------------------------------------

/// Look at the world: your status, what you see from your eyes (sorted by threat;
/// `occluded` when something is in the way), what is near but outside your field of
/// view (if the game sets one), and journal entries since your last look. Distances and
/// compass directions are relative to you; N = -Z, E = +X. `scr(x,y)`: where it is in
/// the game window, when there is one.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ObserveArgs {
    /// Show journal entries after this sequence number instead of "since my last look".
    pub since_seq: Option<u64>,
}

fn observe_tool(In(call): In<Call<ObserveArgs>>, world: &mut World) -> ToolResult {
    let agent = call.caller.agent.clone();
    let after = call
        .args
        .since_seq
        .unwrap_or_else(|| world.resource::<AgentSessions>().get(&agent).map_or(0, |s| s.seen_seq));
    let text = observe::observe(world, &ObserveOptions { subject: call.caller.subject, journal_after: Some(after) });
    let last = world.resource::<AiJournal>().last_seq();
    world.resource_mut::<AgentSessions>().get_or_create(&agent).seen_seq = last;
    Ok(text)
}

/// Everything known about one entity: its description, position and every gameplay
/// component with current values (reflection).
#[derive(Deserialize, JsonSchema)]
pub struct InspectArgs {
    pub id: Id,
}

fn inspect_tool(In(call): In<Call<InspectArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    observe::inspect(world, e, call.caller.subject)
}

/// AI-visible entities within a radius, nearest first (including ones out of view).
#[derive(Deserialize, JsonSchema, Default)]
pub struct NearbyArgs {
    /// Radius in meters. Default 15.
    pub radius: Option<f32>,
    /// Only entities whose line contains this text (case-insensitive): "skeleton", "boss".
    pub filter: Option<String>,
    /// Center of the search. Default: you.
    pub around: Option<Id>,
}

fn nearby_tool(In(call): In<Call<NearbyArgs>>, world: &mut World) -> ToolResult {
    let center_entity = match call.args.around {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let center = facts(world, center_entity).ok_or("center is not visible to the AI")?;
    let radius = call.args.radius.unwrap_or(15.0).clamp(0.5, 500.0);
    let lines = observe::nearby(world, center.pos, radius, call.args.filter.as_deref(), Some(center.id));
    if lines.is_empty() {
        return Ok(format!("nothing within {radius:.0}m of {}", center.id));
    }
    Ok(format!("within {radius:.0}m of {}:\n  {}", center.id, lines.join("\n  ")))
}

/// Is the line of sight between two entities clear (rays at eye height)?
#[derive(Deserialize, JsonSchema)]
pub struct LineOfSightArgs {
    /// Default: you.
    pub from: Option<Id>,
    pub to: Id,
}

fn line_of_sight_tool(In(call): In<Call<LineOfSightArgs>>, world: &mut World) -> ToolResult {
    let from = match call.args.from {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let to = call.args.to.entity(world)?;
    let (a, b) = (facts(world, from).ok_or("from: not visible")?, facts(world, to).ok_or("to: not visible")?);
    let clear = spatial::line_of_sight(world, a.pos + Vec3::Y * EYE_HEIGHT, b.pos + Vec3::Y * EYE_HEIGHT, &[from, to]);
    let verdict = if clear { "clear" } else { "blocked" };
    Ok(format!("{verdict}: {} → {} ({})", a.id, b.id, relative(a.pos, b.pos)))
}

/// Journal entries: gameplay events, agent actions, sim control, warnings and errors.
#[derive(Deserialize, JsonSchema, Default)]
pub struct JournalArgs {
    /// Only entries after this sequence number.
    pub since_seq: Option<u64>,
    /// At most this many newest entries. Default 30.
    pub last: Option<usize>,
}

fn journal_tool(In(call): In<Call<JournalArgs>>, journal: Res<AiJournal>) -> ToolResult {
    let (entries, missed) = journal.after(call.args.since_seq.unwrap_or(0));
    let entries: Vec<_> = entries.collect();
    let last = call.args.last.unwrap_or(30).max(1);
    let skip = entries.len().saturating_sub(last);
    let mut out = format!("journal (newest seq {})", journal.last_seq());
    if missed > 0 {
        out.push_str(&format!(", {missed} entries dropped"));
    }
    for e in &entries[skip..] {
        out.push_str(&format!("\n  #{} {e}", e.seq));
    }
    Ok(out)
}

/// The tools you may call, with signatures.
fn tools_tool(In(call): In<Call<NoArgs>>, tools: Res<AiTools>) -> ToolResult {
    let specs = tools.specs(call.caller.scope);
    let lines: Vec<String> = specs
        .iter()
        .map(|s| {
            let first = s.description.lines().next().unwrap_or("");
            format!("{}  [{}]  {}", s.signature, s.scope, first)
        })
        .collect();
    Ok(lines.join("\n"))
}

// --- time ------------------------------------------------------------------------

/// What `pause`, `resume` and `speed` do to this world, and what they answer. A driver of
/// deferred clock requests applies them with this.
pub fn apply_clock(world: &mut World, action: ClockAction) -> ToolResult {
    match action {
        ClockAction::Pause => {
            sim::pause(world);
            // Paused on purpose: the end of loading must not resume it.
            if let Some(mut gate) = world.get_resource_mut::<SimGate>() {
                gate.forget_pause();
            }
            journal::log(world, JournalKind::Sim, "paused");
            Ok(format!("paused at tick {}", sim::tick(world)))
        }
        ClockAction::Resume => {
            if let Some(loading) = sim::gate_message(world) {
                return Err(format!("{loading}; a game started without --paused runs by itself once loaded"));
            }
            sim::resume(world);
            journal::log(world, JournalKind::Sim, "resumed");
            Ok(format!("running from tick {}", sim::tick(world)))
        }
        ClockAction::Speed(x) => {
            sim::set_speed(world, x);
            journal::log(world, JournalKind::Sim, format!("speed {x}x"));
            Ok(format!("speed {}x", sim::speed(world)))
        }
    }
}

/// A clock tool on a world whose steps are deferred: queued, `{"pending": id}`.
fn clock(world: &mut World, action: ClockAction, agent: &str) -> ToolResult<Value> {
    match sim::deferred_queue(world) {
        Some(queue) => {
            let id = queue.lock().map_err(|_| "the step queue is poisoned".to_owned())?.push_clock(action, agent);
            Ok(json!({ "pending": id }))
        }
        None => apply_clock(world, action).map(Value::String),
    }
}

/// Freeze the simulation (rendering, UI and these tools keep working).
fn pause_tool(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult<Value> {
    clock(world, ClockAction::Pause, &call.caller.agent)
}

/// Unfreeze the simulation (real-time play at the current speed).
fn resume_tool(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult<Value> {
    clock(world, ClockAction::Resume, &call.caller.agent)
}

/// Real-time speed multiplier, e.g. 0.25 for slow motion. Does not affect `step`.
#[derive(Deserialize, JsonSchema)]
pub struct SpeedArgs {
    pub x: f32,
}

fn speed_tool(In(call): In<Call<SpeedArgs>>, world: &mut World) -> ToolResult<Value> {
    clock(world, ClockAction::Speed(call.args.x), &call.caller.agent)
}

/// Stop condition for `step`. Any matching condition stops the run. Invariants
/// (`assert_add`) use the same conditions.
#[derive(Deserialize, Serialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StopWhen {
    /// A new journal entry contains this text (case-insensitive), e.g. "phase", "died".
    Journal(String),
    /// The entity no longer exists.
    Gone(Id),
    /// The entity's observation line contains this text, e.g. {"id": 42, "text": "Windup"}.
    Shows { id: Id, text: String },
    /// Two entities are within `meters` of each other.
    Near { a: Id, b: Id, meters: f32 },
    /// A numeric component field crosses a threshold:
    /// {"id": 12, "component": "Health", "path": "current", "below": 30}.
    Field { id: Id, component: String, path: String, below: Option<f64>, above: Option<f64> },
    /// A condition the game registered by name.
    Named(String),
    /// A warning appeared in the journal: a `warn!` line, a component replay checks leave out, ….
    Warning,
    /// An error appeared in the journal: an `error!` line, a failed tool, a diverged replay, ….
    Error,
}

impl std::fmt::Display for StopWhen {
    /// In `shards step --until` terms: `field:e6:Health.current<0`, `journal:died`, `error`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StopWhen::Journal(text) => write!(f, "journal:{text}"),
            StopWhen::Gone(id) => write!(f, "gone:{id}"),
            StopWhen::Shows { id, text } => write!(f, "shows:{id}:{text}"),
            StopWhen::Near { a, b, meters } => write!(f, "near:{a}:{b}:{meters}"),
            StopWhen::Field { id, component, path, below, above } => {
                write!(f, "field:{id}:{component}.{path}")?;
                if let Some(below) = below {
                    write!(f, "<{below}")?;
                }
                match (below, above) {
                    (Some(_), Some(above)) => write!(f, " or >{above}"),
                    (None, Some(above)) => write!(f, ">{above}"),
                    _ => Ok(()),
                }
            }
            StopWhen::Named(name) => write!(f, "named:{name}"),
            StopWhen::Warning => f.write_str("warning"),
            StopWhen::Error => f.write_str("error"),
        }
    }
}

/// Predicate registered by the game: `app.add_ai_condition("boss_phase_2", |w| ...)`.
#[derive(Resource, Default)]
pub struct AiConditions(pub bevy::platform::collections::HashMap<String, fn(&World) -> bool>);

/// Advance the paused simulation by fixed ticks (60 per second by default) and return
/// what happened. With `until`, runs until a condition fires (at most `ticks`).
#[derive(Deserialize, JsonSchema, Default)]
pub struct StepArgs {
    /// Maximum ticks to run. Default: 1, or 600 (10 s) when `until` is given.
    pub ticks: Option<u64>,
    /// Stop as soon as any of these conditions is met.
    #[serde(default)]
    pub until: Vec<StopWhen>,
}

const MAX_STEP: u64 = 216_000; // one hour of game time

fn step_tool(In(call): In<Call<StepArgs>>, world: &mut World) -> ToolResult<Value> {
    let StepArgs { ticks, until } = call.args;
    let max = ticks.unwrap_or(if until.is_empty() { 1 } else { 600 }).min(MAX_STEP);
    for cond in &until {
        validate(world, cond)?;
    }
    let journal_mark = world.resource::<AiJournal>().last_seq();
    if let Some(queue) = sim::deferred_queue(world) {
        let mut queue = queue.lock().map_err(|_| "the step queue is poisoned".to_owned())?;
        let id = queue.push_step(max, until, &call.caller.agent, journal_mark);
        return Ok(json!({ "pending": id }));
    }
    let report = sim::step(world, max, Duration::from_secs(STEP_WALL_SECS), |w| check_until(w, &until, journal_mark))?;
    Ok(Value::String(step_report_text(world, &report, !until.is_empty())))
}

/// Wall-clock budget of one `step` call.
pub const STEP_WALL_SECS: u64 = 20;

/// What `step` answers for `report`: `ran 120 ticks (0 → 120), stopped: journal: …`, and
/// the `ticks:` timing line when the game reports it ([`perf`](crate::perf)).
/// `conditions`: the step had `until` conditions (so running all ticks says none was met).
pub fn step_report_text(world: &World, report: &StepReport, conditions: bool) -> String {
    let mut msg = format!("ran {} ticks ({} → {})", report.ticks(), report.from_tick, report.to_tick);
    match &report.end {
        StepEnd::Completed if !conditions => {}
        StepEnd::Completed => msg.push_str(", no condition met"),
        StepEnd::Condition(why) => msg.push_str(&format!(", stopped: {why}")),
        StepEnd::WallClock => msg.push_str(&format!(", stopped: {STEP_WALL_SECS} s wall-clock budget")),
        StepEnd::SystemFailed(failures) => {
            let first = &failures[0];
            msg.push_str(&format!(", stopped: gameplay {}", first.headline()));
            if failures.len() > 1 {
                msg.push_str(&format!(" (+{} more)", failures.len() - 1));
            }
            msg.push_str(&format!(": {}", first.message));
            if let Some(path) = failures.iter().find_map(|f| f.crash_replay.as_ref()) {
                msg.push_str(&format!("; crash replay: {path}"));
            }
        }
        StepEnd::InvariantViolated(violations) => {
            let first = &violations[0];
            msg.push_str(&format!(", stopped: {}", first.headline()));
            if violations.len() > 1 {
                msg.push_str(&format!(" (+{} more)", violations.len() - 1));
            }
            if let Some(path) = &first.replay {
                msg.push_str(&format!(" (replay: {path})"));
            }
        }
    }
    if let Some(line) = crate::perf::step_line(world, &report.timing) {
        msg.push('\n');
        msg.push_str(&line);
    }
    msg
}

/// After a tick of a step: the tick's log lines go into the journal, then the first
/// condition of `until` that holds says why the step stops (`None`: none does).
pub fn check_until(world: &mut World, until: &[StopWhen], journal_mark: u64) -> Option<String> {
    journal::drain_logs(world);
    check(world, until, journal_mark)
}

/// A deferred step, driven from outside the tool (a stand's driver): what the driver needs so
/// the step stops where a local one would and its answer is the one `step` gives.
///
/// ```ignore
/// let mut watch = StepWatch::begin(world, &request);
/// // run ticks; after each: if let Some(why) = watch.check(world) { stop with why }
/// let outcome = watch.finish(world, &report);
/// queue.lock().unwrap().complete(request.id, outcome);
/// ```
///
/// Run the ticks with [`sim::step`] (one call per member, a tick at a time if the members
/// must stay in lockstep): it stops at a failed gameplay system or a violated invariant
/// ([`StepEnd::SystemFailed`], [`StepEnd::InvariantViolated`]), and `finish` then reports
/// the halt (`halted`) the way a local `step` does.
pub struct StepWatch {
    until: Vec<StopWhen>,
    journal_mark: u64,
    before: Digest,
    origin: Option<Vec3>,
    /// Halts before the first tick: one after it is this step's.
    halts_before: u64,
}

impl StepWatch {
    /// Before the first tick: what the world looks like now, for the changes.
    pub fn begin(world: &mut World, request: &StepRequest) -> StepWatch {
        let caller = crate::session::caller(world, &request.requester);
        let origin = caller.subject.and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
        let halts_before = world.get_resource::<sim::Halts>().map_or(0, |h| h.count);
        StepWatch {
            until: request.until.clone(),
            journal_mark: request.journal_mark,
            before: observe::digest(world),
            origin,
            halts_before,
        }
    }

    /// After a tick: why the step stops here, if it does.
    pub fn check(&self, world: &mut World) -> Option<String> {
        check_until(world, &self.until, self.journal_mark)
    }

    /// After the last tick: the call's answer, with the changes and journal lines since the
    /// request.
    pub fn finish(self, world: &mut World, report: &StepReport) -> CallOutcome {
        journal::drain_logs(world);
        let text = step_report_text(world, report, !self.until.is_empty());
        let changes = crate::tools::clip_changes(observe::diff(&self.before, &observe::digest(world), self.origin));
        let journal: Vec<String> = world.resource::<AiJournal>().after(self.journal_mark).0.map(ToString::to_string).collect();
        let halts = world.get_resource::<sim::Halts>();
        let halted = halts.filter(|h| h.count > self.halts_before).and_then(|h| h.last.clone());
        CallOutcome { changes, journal, halted, ..crate::tools::finished(world, "step", Ok(Value::String(text))) }
    }
}

/// Ask how a deferred `step`, `pause`, `resume` or `speed` went: `{"pending": id}` while it
/// waits or runs, then its answer, as the call would have given it right away.
#[derive(Deserialize, JsonSchema)]
pub struct StepPollArgs {
    /// The id the call answered with (`{"pending": 3}`).
    pub id: u64,
}

fn step_poll_tool(In(call): In<Call<StepPollArgs>>, world: &mut World) -> ToolResult<Value> {
    let id = call.args.id;
    let Some(queue) = sim::deferred_queue(world) else {
        return Err("steps run right away in this game, so there is nothing to poll: `step` answers with its report".into());
    };
    let queue = queue.lock().map_err(|_| "the step queue is poisoned".to_owned())?;
    match queue.poll(id) {
        StepPoll::Pending => Ok(json!({ "pending": id })),
        StepPoll::Done(outcome) if outcome.ok => Ok(outcome.result),
        StepPoll::Done(outcome) => Err(outcome.result.as_str().map_or_else(|| outcome.result.to_string(), str::to_owned)),
        StepPoll::Unknown => Err(format!(
            "no request {id}: ids so far go up to {}, and only the last {FINISHED_KEPT} finished ones are kept",
            queue.last_id()
        )),
    }
}

/// Whether `cond` can be checked in this world: its entities exist, its name is known.
pub(crate) fn validate(world: &World, cond: &StopWhen) -> Result<(), String> {
    match cond {
        StopWhen::Gone(_) | StopWhen::Journal(_) | StopWhen::Warning | StopWhen::Error => Ok(()),
        StopWhen::Field { below: None, above: None, .. } => {
            Err("a field condition needs `below` or `above`, e.g. field:6:Health.current<30".into())
        }
        StopWhen::Shows { id, .. } | StopWhen::Field { id, .. } => id.entity(world).map(|_| ()),
        StopWhen::Near { a, b, .. } => a.entity(world).and(b.entity(world)).map(|_| ()),
        StopWhen::Named(name) => {
            let known = world.get_resource::<AiConditions>().map(|c| c.0.contains_key(name)).unwrap_or(false);
            if known {
                Ok(())
            } else {
                let mut names: Vec<String> =
                    world.get_resource::<AiConditions>().map(|c| c.0.keys().cloned().collect()).unwrap_or_default();
                names.sort();
                Err(format!("unknown condition `{name}`; known: {}", names.join(", ")))
            }
        }
    }
}

fn check(world: &World, until: &[StopWhen], journal_mark: u64) -> Option<String> {
    until.iter().find_map(|cond| condition_hit(world, cond, journal_mark))
}

/// What `cond` sees right now, or `None` while it is not met. Journal conditions look at
/// entries after `journal_mark` (agent lines left out: they only echo the calls).
pub(crate) fn condition_hit(world: &World, cond: &StopWhen, journal_mark: u64) -> Option<String> {
    let journal = || world.resource::<AiJournal>().after(journal_mark).0.filter(|e| e.kind != JournalKind::Agent);
    match cond {
        StopWhen::Journal(text) => {
            let needle = text.to_lowercase();
            journal().find(|e| e.text.to_lowercase().contains(&needle)).map(|e| format!("journal: {e}"))
        }
        StopWhen::Warning => journal().find(|e| e.kind == JournalKind::Warn).map(|e| format!("journal: {e}")),
        StopWhen::Error => journal().find(|e| e.kind == JournalKind::Error).map(|e| format!("journal: {e}")),
        StopWhen::Gone(id) => id.entity(world).is_err().then(|| format!("{id} is gone")),
        StopWhen::Shows { id, text } => {
            let needle = text.to_lowercase();
            id.entity(world).ok().and_then(|e| facts(world, e)).and_then(|f| {
                let line = entity_line(&f, None, None);
                line.to_lowercase().contains(&needle).then_some(line)
            })
        }
        StopWhen::Near { a, b, meters } => {
            let pa = a.entity(world).ok().and_then(|e| facts(world, e)).map(|f| f.pos);
            let pb = b.entity(world).ok().and_then(|e| facts(world, e)).map(|f| f.pos);
            match (pa, pb) {
                (Some(pa), Some(pb)) if pa.distance(pb) <= *meters => Some(format!("{a} within {:.1}m of {b}", pa.distance(pb))),
                _ => None,
            }
        }
        StopWhen::Field { id, component, path, below, above } => id.entity(world).ok().and_then(|e| {
            let v = read_number(world, e, component, path).ok()?;
            let low = below.is_some_and(|b| v < b);
            let high = above.is_some_and(|a| v > a);
            (low || high).then(|| format!("{id} {component}.{path} = {v}"))
        }),
        StopWhen::Named(name) => world
            .get_resource::<AiConditions>()
            .and_then(|c| c.0.get(name).copied())
            .and_then(|f| f(world).then(|| format!("condition {name}"))),
    }
}

// --- dev: possession, transforms, reflection ------------------------------------

/// Act as another entity: it becomes your "YOU" and the target of your player tools.
/// Without `id`, return to the default (the player character).
#[derive(Deserialize, JsonSchema, Default)]
pub struct PossessArgs {
    pub id: Option<Id>,
    /// Play as this player's body instead (a networked game's players: 0 is the host's),
    /// found again on every call, so a respawned body is still yours.
    pub player: Option<u8>,
}

fn possess_tool(In(call): In<Call<PossessArgs>>, world: &mut World) -> ToolResult {
    if let Some(id) = call.args.id {
        id.entity(world)?;
    }
    let subject = call.args.id.map(|i| i.0);
    let change = SessionChange {
        subject,
        player: call.args.player,
        clear_subject: subject.is_none() && call.args.player.is_none(),
        ..Default::default()
    };
    let session = configure_session(world, &call.caller.agent, &change)?;
    Ok(match (session.subject, session.player) {
        (Some(id), _) => format!("you are now {id}"),
        (_, Some(n)) => format!("you are now player {n}'s body, whichever entity that is"),
        _ => "you are the player character again".to_owned(),
    })
}

/// Move an entity instantly to (x, z) on the ground plane (y optional).
#[derive(Deserialize, JsonSchema)]
pub struct TeleportArgs {
    pub id: Id,
    pub x: f32,
    pub z: f32,
    pub y: Option<f32>,
}

fn teleport_tool(In(call): In<Call<TeleportArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    let mut tf = world.get_mut::<Transform>(e).ok_or("entity has no Transform")?;
    tf.translation.x = call.args.x;
    tf.translation.z = call.args.z;
    if let Some(y) = call.args.y {
        tf.translation.y = y;
    }
    Ok(format!("{} at {}", call.args.id, crate::spatial::fmt_pos(tf.translation)))
}

/// Remove an entity from the world.
#[derive(Deserialize, JsonSchema)]
pub struct DespawnArgs {
    pub id: Id,
}

fn despawn_tool(In(call): In<Call<DespawnArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    world.despawn(e);
    Ok(format!("{} despawned", call.args.id))
}

/// Read a component (or one field of it) as JSON via reflection.
#[derive(Deserialize, JsonSchema)]
pub struct GetArgs {
    pub id: Id,
    /// Component type, short name ("Health") or full path.
    pub component: String,
    /// Field path inside the component, e.g. "current" or "translation.x".
    pub path: Option<String>,
}

fn get_tool(In(call): In<Call<GetArgs>>, world: &mut World) -> ToolResult<Value> {
    let e = call.args.id.entity(world)?;
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let (reg, value) = reflect_component(world, &registry, e, &call.args.component)?;
    let value = match &call.args.path {
        Some(path) => path.as_str().reflect_element(value).map_err(|err| format!("{}: {err}", reg_name(reg)))?,
        None => value,
    };
    serde_json::to_value(TypedReflectSerializer::new(value, &registry)).map_err(|e| e.to_string())
}

/// Overwrite a component (or one field of it) from JSON via reflection. Use `get`
/// first to see the shape. Example: {"id": 12, "component": "Health", "path": "current", "value": 100}.
#[derive(Deserialize, JsonSchema)]
pub struct SetArgs {
    pub id: Id,
    pub component: String,
    pub path: Option<String>,
    pub value: Value,
}

fn set_tool(In(call): In<Call<SetArgs>>, world: &mut World) -> ToolResult {
    let SetArgs { id, component, path, value } = call.args;
    let e = id.entity(world)?;
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let reg = find_registration(world, &registry, e, &component)?;
    if reg.type_id() == std::any::TypeId::of::<AiId>() {
        // An in-place write would bypass the hooks that keep `AiIndex` in sync.
        return Err(AI_ID_IS_MANAGED.into());
    }
    let reflect_component = reg.data::<ReflectComponent>().ok_or("not a reflectable component")?;
    let name = reg_name(reg);
    let mut entity = world.entity_mut(e);
    let mut target = reflect_component
        .reflect_mut(&mut entity)
        .ok_or_else(|| format!("{id} has no {name} (or it is immutable)"))?;
    let field: &mut dyn PartialReflect = match &path {
        Some(p) => target.reflect_path_mut(p.as_str()).map_err(|err| format!("{name}: {err}"))?,
        None => target.as_partial_reflect_mut(),
    };
    let field_reg = field
        .get_represented_type_info()
        .and_then(|info| registry.get(info.type_id()))
        .ok_or("field type is not registered for reflection")?;
    let parsed = TypedReflectDeserializer::new(field_reg, &registry)
        .deserialize(value.clone())
        .map_err(|err| format!("value does not fit {}: {err}", field_reg.type_info().type_path_table().short_path()))?;
    // The value replaces what is there: applied, a shorter list (`[]`) would only overwrite
    // the first elements and leave the rest.
    let whole = field_reg.data::<ReflectFromReflect>().and_then(|from| from.from_reflect(parsed.as_ref()));
    let replaced = match (whole, field.try_as_reflect_mut()) {
        (Some(value), Some(field)) => field.set(value).is_ok(),
        _ => false,
    };
    if !replaced {
        field.try_apply(parsed.as_ref()).map_err(|err| err.to_string())?;
    }
    let at = path.map(|p| format!("{name}.{p}")).unwrap_or(name);
    Ok(format!("{id} {at} = {value}"))
}

fn reg_name(reg: &TypeRegistration) -> String {
    reg.type_info().type_path_table().short_path().to_owned()
}

fn find_registration<'r>(
    world: &World,
    registry: &'r TypeRegistry,
    entity: Entity,
    component: &str,
) -> Result<&'r TypeRegistration, String> {
    let found = registry
        .get_with_short_type_path(component)
        .or_else(|| registry.get_with_type_path(component))
        .filter(|r| r.data::<ReflectComponent>().is_some());
    if let Some(reg) = found {
        return Ok(reg);
    }
    if registry.is_ambiguous(component) {
        // Several types share the short name (a game's `Health` and an engine's): the
        // one this entity has is meant.
        let mine: Vec<&TypeRegistration> = world
            .inspect_entity(entity)
            .into_iter()
            .flatten()
            .filter_map(|(_, info)| registry.get(info.type_id()?))
            .filter(|r| r.data::<ReflectComponent>().is_some() && reg_name(r) == component)
            .collect();
        if let [only] = mine.as_slice() {
            return Ok(only);
        }
        let paths: Vec<&str> = registry
            .iter()
            .filter(|r| r.type_info().type_path_table().short_path() == component)
            .map(|r| r.type_info().type_path())
            .collect();
        return Err(format!("`{component}` names several components; use the full path: {}", paths.join(", ")));
    }
    let names = reflectable_components(world, registry, entity);
    Err(format!("unknown component `{component}`; this entity has: {}", names.join(", ")))
}

fn reflect_component<'w, 'r>(
    world: &'w World,
    registry: &'r TypeRegistry,
    entity: Entity,
    component: &str,
) -> Result<(&'r TypeRegistration, &'w dyn PartialReflect), String> {
    let reg = find_registration(world, registry, entity, component)?;
    let rc = reg.data::<ReflectComponent>().ok_or("not a component")?;
    let value = rc.reflect(world.entity(entity)).ok_or_else(|| format!("entity has no {}", reg_name(reg)))?;
    Ok((reg, value.as_partial_reflect()))
}

fn reflectable_components(world: &World, registry: &TypeRegistry, entity: Entity) -> Vec<String> {
    let Ok(infos) = world.inspect_entity(entity) else {
        return Vec::new();
    };
    let mut names: Vec<String> = infos
        .filter_map(|(_, info)| info.type_id())
        .filter_map(|t| registry.get(t))
        .filter(|r| r.data::<ReflectComponent>().is_some())
        // Our own markers are noise here; a game's `AimAssist` is not.
        .filter(|r| !r.type_info().type_path().starts_with("ai_core::"))
        .map(reg_name)
        .collect();
    names.sort();
    names
}

/// Reads a numeric field via reflection (f32/f64/integers) as f64.
pub fn read_number(world: &World, entity: Entity, component: &str, path: &str) -> Result<f64, String> {
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let (_, value) = reflect_component(world, &registry, entity, component)?;
    let field = path.reflect_element(value).map_err(|e| e.to_string())?;
    macro_rules! try_num {
        ($($t:ty),*) => {
            $(if let Some(v) = field.try_downcast_ref::<$t>() { return Ok(*v as f64); })*
        };
    }
    try_num!(f32, f64, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);
    Err(format!("{component}.{path} is not a number"))
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;
    use serde_json::json;

    use crate::prelude::*;

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct Bag {
        items: Vec<String>,
        weight: f32,
    }

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: SimConfig { seed: 1, start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .register_type::<Bag>();
        app.world_mut().spawn((AiKind::new("Mule"), Bag { items: vec!["rope".into(), "lamp".into()], weight: 3.0 }));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn bag(app: &mut App) -> Bag {
        let world = app.world_mut();
        world.query::<&Bag>().single(world).unwrap().clone()
    }

    #[test]
    fn set_replaces_lists_instead_of_writing_over_their_first_elements() {
        let mut app = game();
        let set = |app: &mut App, path: Option<&str>, value: serde_json::Value| {
            let out = crate::call_tool(app.world_mut(), "dev", "set", json!({"id": "e1", "component": "Bag", "path": path, "value": value}));
            assert!(out.ok, "{}", out.result);
        };
        set(&mut app, Some("items"), json!([]));
        assert_eq!(bag(&mut app).items, Vec::<String>::new(), "an empty list empties it");
        set(&mut app, Some("items"), json!(["map"]));
        assert_eq!(bag(&mut app).items, ["map"]);
        set(&mut app, None, json!({"items": [], "weight": 1.5}));
        assert_eq!(bag(&mut app), Bag { items: Vec::new(), weight: 1.5 }, "the whole component, lists included");
        set(&mut app, Some("weight"), json!(2));
        assert_eq!(bag(&mut app).weight, 2.0);
    }

    #[derive(Component)]
    struct Walker;

    #[derive(Event)]
    struct Rang;

    fn walk(mut walkers: Query<&mut Transform, With<Walker>>, tick: Res<SimTick>, mut commands: Commands) {
        for mut t in &mut walkers {
            t.translation.x += 1.0;
        }
        if tick.0 == 4 {
            commands.trigger(Rang);
        }
    }

    fn stepper(deferred: bool) -> App {
        use crate::AiAppExt;
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                report_perf: false,
                sim: SimConfig { start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .journal_event::<Rang>(|_, _| Some("the bell rang".into()))
            .add_systems(FixedUpdate, walk.in_set(SimSet::Resolve));
        if deferred {
            app.insert_resource(crate::sim::StepRoute::Deferred(Default::default()));
        }
        app.world_mut().spawn((AiKind::new("Walker"), Walker, Transform::default()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    /// What a stand's driver does between frames.
    fn drive(app: &mut App) {
        let world = app.world_mut();
        let queue = crate::sim::deferred_queue(world).unwrap();
        let (steps, clock) = {
            let mut queue = queue.lock().unwrap();
            (queue.take_steps(), queue.take_clock())
        };
        for request in clock {
            let result = super::apply_clock(world, request.action).map(serde_json::Value::String);
            let outcome = crate::tools::finished(world, request.action.tool(), result);
            queue.lock().unwrap().complete(request.id, outcome);
        }
        for request in steps {
            let watch = super::StepWatch::begin(world, &request);
            let wall = std::time::Duration::from_secs(super::STEP_WALL_SECS);
            let report = crate::sim::step(world, request.ticks, wall, |w| watch.check(w)).unwrap();
            let outcome = watch.finish(world, &report);
            queue.lock().unwrap().complete(request.id, outcome);
        }
    }

    #[test]
    fn a_deferred_step_answers_later_exactly_as_a_local_one() {
        let (mut local, mut deferred) = (stepper(false), stepper(true));
        let args = json!({"ticks": 10, "until": [{"journal": "rang"}]});
        let direct = crate::call_tool(local.world_mut(), "dev", "step", args.clone());
        assert_eq!(direct.result, "ran 4 ticks (0 → 4), stopped: journal: 4 the bell rang");

        let asked = crate::call_tool(deferred.world_mut(), "dev", "step", args);
        assert_eq!(asked.result, json!({"pending": 1}));
        let poll = |app: &mut App, id: u64| crate::call_tool(app.world_mut(), "dev", "step_poll", json!({"id": id}));
        assert_eq!(poll(&mut deferred, 1).result, json!({"pending": 1}));
        deferred.update();
        assert_eq!(crate::sim::tick(deferred.world()), 0, "nothing runs until the driver does");
        drive(&mut deferred);
        deferred.update();
        assert_eq!(poll(&mut deferred, 1).result, direct.result);
        let queue = crate::sim::deferred_queue(deferred.world()).unwrap();
        let crate::sim::StepPoll::Done(outcome) = queue.lock().unwrap().poll(1) else { panic!("not done") };
        assert_eq!((outcome.tick, &outcome.changes, &outcome.journal), (direct.tick, &direct.changes, &direct.journal));
        assert!(!outcome.changes.is_empty() && !outcome.journal.is_empty());

        // The clock tools go the same way.
        assert_eq!(crate::call_tool(deferred.world_mut(), "dev", "speed", json!({"x": 2})).result, json!({"pending": 2}));
        drive(&mut deferred);
        assert_eq!(poll(&mut deferred, 2).result, "speed 2x");
        assert!(poll(&mut deferred, 9).result.as_str().unwrap().starts_with("no request 9"));
        assert!(!poll(&mut local, 1).ok, "nothing to poll where steps run right away");
    }

    fn walked_too_far(world: &World) -> Option<String> {
        let mut q = world.try_query_filtered::<&Transform, With<Walker>>()?;
        q.iter(world).find(|t| t.translation.x > 2.5).map(|t| format!("x = {}", t.translation.x))
    }

    /// A violated invariant stops a deferred step at the same tick, with the same answer and
    /// the same halt, as a local one.
    #[test]
    fn a_deferred_step_halts_at_a_violation_as_a_local_one() {
        use crate::AiAppExt;
        let (mut local, mut deferred) = (stepper(false), stepper(true));
        local.add_ai_invariant("fence", walked_too_far);
        deferred.add_ai_invariant("fence", walked_too_far);
        let args = json!({"ticks": 10});
        let direct = crate::call_tool(local.world_mut(), "dev", "step", args.clone());
        assert_eq!(direct.result, "ran 3 ticks (0 → 3), stopped: invariant fence violated at tick 3");
        let halt = direct.halted.clone().expect("the local step halted");

        assert_eq!(crate::call_tool(deferred.world_mut(), "dev", "step", args).result, json!({"pending": 1}));
        drive(&mut deferred);
        let queue = crate::sim::deferred_queue(deferred.world()).unwrap();
        let crate::sim::StepPoll::Done(outcome) = queue.lock().unwrap().poll(1) else { panic!("not done") };
        assert_eq!((&outcome.result, outcome.tick, outcome.halted.as_ref()), (&direct.result, 3, Some(&halt)));
        assert!(crate::sim::is_paused(deferred.world()));
    }
}
