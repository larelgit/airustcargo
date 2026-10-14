//! Typed tools: an argument struct plus an ordinary Bevy system.
//!
//! `schemars` turns the argument struct into a JSON Schema (doc comments become
//! descriptions). The same schema documents the tool for agents (`shards tools`)
//! and validates its input. Calls go through [`call_tool`], which checks the agent's scope, runs
//! the system, survives panics, journals actions and returns what changed.
//!
//! ```ignore
//! /// Walk to an entity or a point.
//! #[derive(Deserialize, JsonSchema)]
//! struct MoveTo { target: Target }
//!
//! fn move_to(In(call): In<Call<MoveTo>>, mut q: Query<&mut Intent>) -> ToolResult {
//!     let me = call.caller.require_subject()?;
//!     q.get_mut(me).map_err(|_| "you cannot move".to_string())?.orders.push(Order::MoveTo(call.args.target));
//!     Ok("moving".into())
//! }
//!
//! app.add_ai_tool("move_to", Scope::Player, move_to);
//! ```

use std::any::TypeId;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::id::{AiId, AiIndex};
use crate::journal::{self, AiJournal, JournalKind};
use crate::observe::{diff, digest};
use crate::session::{caller, AgentSessions, Caller, Scope};
use crate::sim::{self, Halt, Halts};

pub type ToolResult<T = String> = Result<T, String>;

/// Input of every tool system: the parsed arguments and who is calling.
pub struct Call<A> {
    pub args: A,
    pub caller: Caller,
}

/// Arguments for tools that take none.
#[derive(Deserialize, JsonSchema, Default, Debug)]
pub struct NoArgs {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    /// Read-only: no journal entry, no diff.
    Query,
    /// Changes the world: journaled, returns the list of changes, recorded in replays.
    Action,
    /// Controls time or the session (pause, step, possess): journaled with a diff like
    /// an action, but never recorded in replays. It changes *when* things happen, not what.
    Control,
}

/// Where a call runs when this game is a client of a networked game (a [`Forwarder`] is
/// set). Without one, every tool runs here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetRoute {
    /// Here: queries, a player's own actions (they act through `Intent`, which travels by
    /// itself), time control (routed by [`StepRoute`](crate::sim::StepRoute)).
    Local,
    /// On the host: the call is handed to the forwarder, which sends it there.
    Host,
    /// Only on the host; refused here with this reason (`call it on the host`).
    HostOnly(&'static str),
}

impl NetRoute {
    /// The default for a tool of this kind and scope: dev actions change the shared world, so
    /// they run on the host; everything else runs here.
    pub fn default_for(kind: ToolKind, scope: Scope) -> NetRoute {
        match (kind, scope) {
            (ToolKind::Action, Scope::Dev) => NetRoute::Host,
            _ => NetRoute::Local,
        }
    }
}

/// Public description of a tool; `input_schema` is the JSON Schema of its arguments.
#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    pub kind: ToolKind,
    /// Where it runs on a client of a networked game.
    pub net: NetRoute,
    /// One-line form for prompts and errors: `move_to(target: id|{x,z}, run?: bool)`.
    pub signature: String,
    pub input_schema: Value,
}

/// The reason of the engine's [`NetRoute::HostOnly`] tools.
pub const ON_THE_HOST: &str = "call it on the host";

/// A call a client hands to its host ([`Forwarder`]).
#[derive(Clone, Debug, Serialize)]
pub struct ForwardedCall {
    /// The agent that made it.
    pub agent: String,
    pub scope: Scope,
    /// The player the agent is bound to, if any ([`AgentSession::player`](crate::AgentSession)).
    pub player: Option<u8>,
    /// The agent's subject, if it is set by id.
    pub subject: Option<AiId>,
    pub tool: String,
    pub args: Value,
    pub kind: ToolKind,
}

/// How a client hands [`NetRoute::Host`] calls to its host. `forward` returns what the call
/// answers here: the host's result, or a promise to poll (`{"pending": id}`).
#[derive(Clone)]
pub struct Forwarder {
    pub forward: fn(&mut World, ForwardedCall) -> ToolResult<Value>,
    /// The host, for messages: `127.0.0.1:7777`.
    pub host: String,
}

type RunFn = Box<dyn Fn(&mut World, Caller, Value) -> ToolResult<Value> + Send + Sync>;

/// A tool's registered system, once it ran.
type ToolSystemId<A, O> = SystemId<In<Call<A>>, ToolResult<O>>;

pub struct AiTool {
    pub spec: ToolSpec,
    run: RunFn,
}

/// Tool registry, ordered by name (stable output for agents and diffs).
#[derive(Resource, Default)]
pub struct AiTools {
    tools: BTreeMap<String, Arc<AiTool>>,
    /// Set on a client of a networked game: where [`NetRoute::Host`] calls go.
    forwarder: Option<Forwarder>,
}

impl AiTools {
    pub fn get(&self, name: &str) -> Option<Arc<AiTool>> {
        self.tools.get(name).cloned()
    }

    pub fn specs(&self, scope: Scope) -> Vec<ToolSpec> {
        self.tools.values().filter(|t| scope.allows(t.spec.scope)).map(|t| t.spec.clone()).collect()
    }

    pub fn names(&self, scope: Scope) -> Vec<String> {
        self.tools.values().filter(|t| scope.allows(t.spec.scope)).map(|t| t.spec.name.clone()).collect()
    }

    /// Changes where tool `name` runs on a client. Refused for a tool that does not exist or
    /// is running right now.
    pub fn set_route(&mut self, name: &str, route: NetRoute) -> Result<(), String> {
        let tool = self.tools.get_mut(name).ok_or_else(|| format!("no tool `{name}`"))?;
        let tool = Arc::get_mut(tool).ok_or_else(|| format!("`{name}` is running; set its route at startup"))?;
        tool.spec.net = route;
        Ok(())
    }

    /// Hands [`NetRoute::Host`] calls to `forwarder` from now on (and refuses
    /// [`NetRoute::HostOnly`] ones); `None`: every tool runs here again.
    pub fn set_forwarder(&mut self, forwarder: Option<Forwarder>) {
        self.forwarder = forwarder;
    }

    /// The forwarder, if this game is a client.
    pub fn forwarder(&self) -> Option<&Forwarder> {
        self.forwarder.as_ref()
    }
}

/// Registers `system` as tool `name`. Prefer [`AiAppExt::add_ai_tool`](crate::AiAppExt).
pub fn register_tool<A, O, M, S>(world: &mut World, name: &str, scope: Scope, kind: ToolKind, system: S)
where
    A: DeserializeOwned + JsonSchema + Send + Sync + 'static,
    O: Serialize + 'static,
    S: IntoSystem<In<Call<A>>, ToolResult<O>, M> + Clone + Send + Sync + 'static,
{
    let (mut description, input_schema) = tool_schema::<A>();
    // `NoArgs` is documented for Rust readers only: a tool that takes no arguments gets no
    // description rather than that sentence.
    if TypeId::of::<A>() == TypeId::of::<NoArgs>() {
        description.clear();
    }
    let signature = signature(name, &input_schema);
    let slot: Mutex<Option<ToolSystemId<A, O>>> = Mutex::new(None);
    let tool_name = name.to_owned();
    let run: RunFn = Box::new(move |world: &mut World, caller: Caller, args: Value| {
        let args = if args.is_null() { json!({}) } else { args };
        let args: A = serde_json::from_value(args).map_err(|e| format!("bad arguments for {tool_name}: {e}"))?;
        let id = {
            let mut slot = slot.lock().map_err(|_| "tool registry poisoned".to_owned())?;
            *slot.get_or_insert_with(|| world.register_system(system.clone()))
        };
        let schedules = fixed_schedules(world);
        match catch_unwind(AssertUnwindSafe(|| world.run_system_with(id, Call { args, caller }))) {
            Ok(Ok(Ok(out))) => serde_json::to_value(out).map_err(|e| format!("cannot serialize result: {e}")),
            Ok(Ok(Err(msg))) => Err(msg),
            Ok(Err(e)) => Err(format!("{tool_name} could not run: {e}")),
            Err(panic) => {
                // The panicked system is gone from its slot; register a fresh copy next time.
                if let Ok(mut slot) = slot.lock() {
                    *slot = None;
                }
                let _ = world.unregister_system(id);
                let msg = panic_message(&panic);
                journal::log(world, JournalKind::Error, format!("tool {tool_name} panicked: {msg}"));
                // A panic that unwound through a running schedule (a gameplay panic in
                // `step` with `catch_system_panics` off) took that schedule with it.
                let lost: Vec<&str> = schedules.iter().filter(|s| !fixed_schedules(world).contains(s)).copied().collect();
                if lost.is_empty() {
                    Err(format!("{tool_name} panicked: {msg} (the game kept running; the panic is in the journal)"))
                } else {
                    Err(format!(
                        "{tool_name} panicked: {msg} (the game lost its {} schedules: no more ticks, restart it)",
                        lost.join(", ")
                    ))
                }
            }
        }
    });
    let net = NetRoute::default_for(kind, scope);
    let spec = ToolSpec { name: name.to_owned(), description, scope, kind, net, signature, input_schema };
    world.get_resource_or_init::<AiTools>().tools.insert(name.to_owned(), Arc::new(AiTool { spec, run }));
}

pub(crate) fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Which of the fixed-tick schedules are in the world (a running one is taken out).
fn fixed_schedules(world: &World) -> Vec<&'static str> {
    use bevy::app::{FixedFirst, FixedLast, FixedMain, FixedPostUpdate, FixedPreUpdate};
    let Some(schedules) = world.get_resource::<Schedules>() else {
        return Vec::new();
    };
    [
        ("FixedMain", schedules.contains(FixedMain)),
        ("FixedFirst", schedules.contains(FixedFirst)),
        ("FixedPreUpdate", schedules.contains(FixedPreUpdate)),
        ("FixedUpdate", schedules.contains(FixedUpdate)),
        ("FixedPostUpdate", schedules.contains(FixedPostUpdate)),
        ("FixedLast", schedules.contains(FixedLast)),
    ]
    .into_iter()
    .filter_map(|(name, present)| present.then_some(name))
    .collect()
}

/// The result of one tool call, as returned to any transport.
#[derive(Clone, Debug, Serialize)]
pub struct CallOutcome {
    pub tool: String,
    pub ok: bool,
    /// Tool output on success, error message on failure.
    pub result: Value,
    /// Tick after the call.
    pub tick: u64,
    pub paused: bool,
    /// What changed in AI-visible state (action tools only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<String>,
    /// Journal entries written during the call (action tools only).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub journal: Vec<String>,
    /// The game halted during the call: a `step` stopped at a failed gameplay system or
    /// a violated invariant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub halted: Option<Halt>,
}

const MAX_CHANGES: usize = 60;

/// At most [`MAX_CHANGES`] changes, and a line saying how many more there were.
pub(crate) fn clip_changes(mut changes: Vec<String>) -> Vec<String> {
    if changes.len() > MAX_CHANGES {
        let extra = changes.len() - MAX_CHANGES;
        changes.truncate(MAX_CHANGES);
        changes.push(format!("… {extra} more changes"));
    }
    changes
}

/// The answer of a call to `tool` that ended with `result`, on the world as it is now, with no
/// changes and no journal lines: for a driver that completes deferred requests. It says no
/// halt; [`StepWatch::finish`](crate::builtin::StepWatch::finish) fills in the one its step met.
pub fn finished(world: &World, tool: &str, result: ToolResult<Value>) -> CallOutcome {
    let (ok, result) = match result {
        Ok(value) => (true, value),
        Err(message) => (false, Value::String(message)),
    };
    CallOutcome {
        tool: tool.to_owned(),
        ok,
        result,
        tick: sim::tick(world),
        paused: sim::is_paused(world),
        changes: Vec::new(),
        journal: Vec::new(),
        halted: None,
    }
}

/// Arguments longer than this are cut in the journal line (replays keep them whole):
/// a content edit carries whole definitions.
const MAX_JOURNALED_ARGS: usize = 200;

/// `text` cut to about `max` bytes, on a character boundary, saying how long it was.
fn clip(text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let cut = (0..=max).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}… ({} bytes)", &text[..cut], text.len())
}

/// Runs a tool on behalf of `agent`. The single entry point: BRP (`shards`), the
/// game's own dev keys, tests and replays all call tools through here.
pub fn call_tool(world: &mut World, agent: &str, tool: &str, args: Value) -> CallOutcome {
    crate::process::activate(world);
    let caller = caller(world, agent);
    let halts = |world: &World| world.get_resource::<Halts>().map_or(0, |h| h.count);
    let outcome = |world: &World, ok: bool, result: Value, changes, journal, halts_before: u64| CallOutcome {
        tool: tool.to_owned(),
        ok,
        result,
        tick: sim::tick(world),
        paused: sim::is_paused(world),
        changes,
        journal,
        halted: (halts(world) > halts_before).then(|| world.resource::<Halts>().last.clone()).flatten(),
    };

    let Some(entry) = world.get_resource::<AiTools>().and_then(|t| t.get(tool)) else {
        let names = world.get_resource::<AiTools>().map(|t| t.names(caller.scope)).unwrap_or_default();
        let msg = format!("unknown tool `{tool}`; available: {}", names.join(", "));
        return outcome(world, false, Value::String(msg), Vec::new(), Vec::new(), halts(world));
    };
    if !caller.scope.allows(entry.spec.scope) {
        let msg = format!(
            "`{tool}` needs {} scope; agent `{}` has {} scope",
            entry.spec.scope, caller.agent, caller.scope
        );
        return outcome(world, false, Value::String(msg), Vec::new(), Vec::new(), halts(world));
    }

    // A client: what changes the shared world runs on the host.
    if let Some(forwarder) = world.resource::<AiTools>().forwarder.clone() {
        match entry.spec.net {
            NetRoute::Local => {}
            NetRoute::HostOnly(reason) => {
                let msg = format!("`{tool}` runs on the host only: {reason} ({})", forwarder.host);
                return outcome(world, false, Value::String(msg), Vec::new(), Vec::new(), halts(world));
            }
            NetRoute::Host => {
                let args_text = if args.is_null() { String::new() } else { clip(args.to_string(), MAX_JOURNALED_ARGS) };
                journal::drain_logs(world);
                let text = format!("{}: {tool} {args_text} (on the host {})", caller.agent, forwarder.host);
                let mark = journal::log(world, JournalKind::Agent, text);
                let player = world.resource::<AgentSessions>().get(&caller.agent).and_then(|s| s.player);
                let call = ForwardedCall {
                    agent: caller.agent.clone(),
                    scope: caller.scope,
                    player,
                    subject: caller.subject_id,
                    tool: tool.to_owned(),
                    args,
                    kind: entry.spec.kind,
                };
                let result = (forwarder.forward)(world, call);
                journal::drain_logs(world);
                let journal: Vec<String> = world.resource::<AiJournal>().after(mark).0.map(ToString::to_string).collect();
                let (ok, value) = match result {
                    Ok(v) => (true, v),
                    Err(e) => (false, Value::String(e)),
                };
                // A client runs no rules: a halt is the host's, and its answer says so.
                return outcome(world, ok, value, Vec::new(), journal, halts(world));
            }
        }
    }

    // What failed before this call is gameplay's (journaled, the game pauses); what
    // fails during it is the tool's.
    crate::failures::collect(world);
    let halts_before = halts(world);
    if entry.spec.kind == ToolKind::Query {
        let result = (entry.run)(world, caller, args);
        crate::failures::blame_tool(world, tool);
        let (ok, value) = match result {
            Ok(v) => (true, v),
            Err(e) => (false, Value::String(teach(e, &entry.spec))),
        };
        return outcome(world, ok, value, Vec::new(), Vec::new(), halts_before);
    }

    let is_action = entry.spec.kind == ToolKind::Action;
    // An action while loading would run on a world `OnEnter` has not spawned yet, and
    // its replay would run on the spawned one.
    if is_action && let Some(loading) = sim::gate_message(world) {
        return outcome(world, false, Value::String(loading), Vec::new(), Vec::new(), halts_before);
    }
    if is_action {
        crate::replay::begin_action(world, &caller, tool, &args);
    }
    let origin = caller.subject.and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
    let before = digest(world);
    let args_text = if args.is_null() { String::new() } else { clip(args.to_string(), MAX_JOURNALED_ARGS) };
    // Earlier log lines go in before the mark: they are not this call's.
    journal::drain_logs(world);
    let mark = journal::log(world, JournalKind::Agent, format!("{}: {tool} {args_text}", caller.agent));
    let result = (entry.run)(world, caller, args);
    if is_action {
        crate::replay::end_action(world);
    }
    crate::failures::blame_tool(world, tool);
    // A warning the call caused comes back with it.
    journal::drain_logs(world);
    let after = digest(world);

    let changes = clip_changes(diff(&before, &after, origin));
    let journal: Vec<String> = world.resource::<AiJournal>().after(mark).0.map(ToString::to_string).collect();
    let (ok, value) = match result {
        Ok(v) => (true, v),
        Err(e) => (false, Value::String(teach(e, &entry.spec))),
    };
    outcome(world, ok, value, changes, journal, halts_before)
}

/// Bad-argument errors get the expected signature appended so the agent can fix the call.
fn teach(error: String, spec: &ToolSpec) -> String {
    if error.starts_with("bad arguments") {
        format!("{error}. Expected: {}", spec.signature)
    } else {
        error
    }
}

// --- schemas ---------------------------------------------------------------------

/// `(description, input_schema)` for an argument type: draft 2020-12, subschemas
/// inlined (readable on its own, no `$ref` to chase), always `"type": "object"`.
pub fn tool_schema<A: JsonSchema>() -> (String, Value) {
    let schema = SchemaSettings::draft2020_12()
        .with(|s| {
            s.inline_subschemas = true;
            s.meta_schema = None;
        })
        .into_generator()
        .into_root_schema_for::<A>();
    let mut value = schema.to_value();
    let obj = value.as_object_mut().expect("tool argument schema must be an object");
    obj.remove("title");
    let description = obj.remove("description").and_then(|d| d.as_str().map(str::to_owned)).unwrap_or_default();
    obj.insert("type".into(), "object".into());
    obj.entry("properties").or_insert_with(|| Value::Object(Map::new()));
    (description.trim().to_owned(), value)
}

/// Compact signature from a schema: `step(ticks?: integer, until?: [...])`.
pub fn signature(name: &str, schema: &Value) -> String {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let params: Vec<String> = schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| {
            props
                .iter()
                .map(|(k, v)| {
                    let opt = if required.contains(&k.as_str()) { "" } else { "?" };
                    format!("{k}{opt}: {}", type_summary(v, 0))
                })
                .collect()
        })
        .unwrap_or_default();
    format!("{name}({})", params.join(", "))
}

fn type_summary(v: &Value, depth: usize) -> String {
    if depth > 2 {
        return "…".into();
    }
    if let Some(values) = v.get("enum").and_then(Value::as_array) {
        return values.iter().map(|x| x.to_string()).collect::<Vec<_>>().join("|");
    }
    if let Some(c) = v.get("const") {
        return c.to_string();
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(options) = v.get(key).and_then(Value::as_array) {
            return options.iter().map(|o| type_summary(o, depth + 1)).collect::<Vec<_>>().join(" | ");
        }
    }
    if let Some(props) = v.get("properties").and_then(Value::as_object) {
        let required: Vec<&str> = v
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let fields: Vec<String> = props
            .iter()
            .map(|(k, p)| {
                let opt = if required.contains(&k.as_str()) { "" } else { "?" };
                format!("{k}{opt}: {}", type_summary(p, depth + 1))
            })
            .collect();
        return format!("{{{}}}", fields.join(", "));
    }
    if let Some(items) = v.get("items") {
        return format!("[{}]", type_summary(items, depth + 1));
    }
    match v.get("type") {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(ts)) => {
            let ts: Vec<&str> = ts.iter().filter_map(Value::as_str).filter(|t| *t != "null").collect();
            ts.join("|")
        }
        _ => "any".into(),
    }
}

// --- argument types shared by tools ----------------------------------------------

/// An entity id: `42` or `"e42"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Id(pub AiId);

impl Id {
    pub fn entity(self, world: &World) -> Result<Entity, String> {
        world.resource::<AiIndex>().resolve(self.0)
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Num(u64),
            Text(String),
        }
        let parsed = match Raw::deserialize(d)? {
            Raw::Num(n) => u32::try_from(n).ok().filter(|&n| n > 0).map(AiId),
            Raw::Text(s) => AiId::parse(&s),
        };
        parsed.map(Id).ok_or_else(|| serde::de::Error::custom("expected an entity id like 42 or \"e42\""))
    }
}

impl JsonSchema for Id {
    fn schema_name() -> Cow<'static, str> {
        "Id".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": ["integer", "string"],
            "description": "Entity id as shown in observations: 42 or \"e42\"."
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Teleport an entity.
    #[derive(Deserialize, JsonSchema)]
    #[allow(dead_code)]
    struct Teleport {
        /// Who.
        id: Id,
        x: f32,
        z: f32,
        y: Option<f32>,
    }

    #[test]
    fn schema_has_description_and_signature() {
        let (description, schema) = tool_schema::<Teleport>();
        assert_eq!(description, "Teleport an entity.");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["id"]["description"], "Who.");
        assert_eq!(signature("teleport", &schema), "teleport(id: integer|string, x: number, y?: number, z: number)");
        let (_, empty) = tool_schema::<NoArgs>();
        assert_eq!(empty["type"], "object");
    }

    #[test]
    fn a_tool_without_arguments_has_no_description() {
        fn ping(In(_): In<Call<NoArgs>>) -> ToolResult {
            Ok("pong".into())
        }
        fn teleport(In(_): In<Call<Teleport>>) -> ToolResult {
            Ok("teleported".into())
        }
        let mut world = World::new();
        register_tool(&mut world, "ping", Scope::Player, ToolKind::Query, ping);
        register_tool(&mut world, "teleport", Scope::Dev, ToolKind::Action, teleport);
        let tools = world.resource::<AiTools>();
        // Empty, not the rustdoc of `NoArgs` ...
        assert_eq!(tools.get("ping").unwrap().spec.description, "");
        // ... while a tool with arguments keeps its own.
        assert_eq!(tools.get("teleport").unwrap().spec.description, "Teleport an entity.");
    }

    #[test]
    fn long_arguments_are_clipped_on_a_char_boundary() {
        assert_eq!(clip("short".into(), 10), "short");
        assert_eq!(clip("ééééé".into(), 5), "éé… (10 bytes)");
    }

    #[test]
    fn id_accepts_numbers_and_strings() {
        let a: Id = serde_json::from_value(json!(42)).unwrap();
        let b: Id = serde_json::from_value(json!("e42")).unwrap();
        assert_eq!(a, b);
        assert!(serde_json::from_value::<Id>(json!("x")).is_err());
        assert!(serde_json::from_value::<Id>(json!(0)).is_err());
    }

    #[derive(Resource, Default)]
    struct Forwarded(Vec<ForwardedCall>);

    fn fake_forward(world: &mut World, call: ForwardedCall) -> ToolResult<Value> {
        world.resource_mut::<Forwarded>().0.push(call);
        Ok(json!({"pending": 7}))
    }

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: crate::SimConfig { start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .init_resource::<Forwarded>();
        app.world_mut().spawn((crate::AiKind::new("Crate"), Transform::default()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    #[test]
    fn a_client_hands_dev_actions_to_its_host_and_refuses_host_only_tools() {
        let mut app = game();
        let route = |app: &App, name: &str| app.world().resource::<AiTools>().get(name).unwrap().spec.net;
        assert_eq!(route(&app, "teleport"), NetRoute::Host);
        assert_eq!(route(&app, "observe"), NetRoute::Local);
        assert_eq!(route(&app, "step"), NetRoute::Local);
        assert_eq!(route(&app, "replay_save"), NetRoute::HostOnly(ON_THE_HOST));
        assert_eq!(route(&app, "snapshot_load"), NetRoute::HostOnly(ON_THE_HOST));
        for rules in ["assert_add", "assert_list", "assert_remove", "assert_reset"] {
            assert_eq!(route(&app, rules), NetRoute::Host, "{rules}: the rules are checked on the host");
        }
        let teleport = |x: f32| json!({"id": 1, "x": x, "z": 0});

        // No forwarder: everything runs here.
        let out = call_tool(app.world_mut(), "dev", "teleport", teleport(2.0));
        assert!(out.ok && !out.changes.is_empty(), "{out:?}");

        let forwarder = Forwarder { forward: fake_forward, host: "127.0.0.1:7777".into() };
        app.world_mut().resource_mut::<AiTools>().set_forwarder(Some(forwarder));
        let out = call_tool(app.world_mut(), "dev", "teleport", teleport(5.0));
        assert!(out.ok, "{out:?}");
        assert_eq!(out.result, json!({"pending": 7}));
        let journal = app.world().resource::<AiJournal>();
        let line = journal.after(journal.last_seq() - 1).0.next().unwrap().to_string();
        assert_eq!(line, "0 [agent] dev: teleport {\"id\":1,\"x\":5.0,\"z\":0} (on the host 127.0.0.1:7777)");
        let sent = &app.world().resource::<Forwarded>().0;
        assert_eq!((sent.len(), sent[0].tool.as_str(), sent[0].args["x"].as_f64()), (1, "teleport", Some(5.0)));
        let crate_x = |app: &mut App| app.world_mut().query::<&Transform>().single(app.world()).unwrap().translation.x;
        assert_eq!(crate_x(&mut app), 2.0, "the host moves it, not this game");

        let out = call_tool(app.world_mut(), "dev", "snapshot_save", json!({}));
        assert!(!out.ok);
        assert_eq!(out.result, "`snapshot_save` runs on the host only: call it on the host (127.0.0.1:7777)");
        assert!(call_tool(app.world_mut(), "dev", "observe", json!({})).ok, "queries run here");

        app.world_mut().resource_mut::<AiTools>().set_forwarder(None);
        assert!(call_tool(app.world_mut(), "dev", "teleport", teleport(9.0)).ok);
        assert_eq!(crate_x(&mut app), 9.0);
        assert_eq!(app.world().resource::<Forwarded>().0.len(), 1);
    }
}
