//! Typed tools: an argument struct plus an ordinary Bevy system.
//!
//! `schemars` turns the argument struct into a JSON Schema (doc comments become
//! descriptions). The same schema is the tool definition for an LLM and the input
//! validation. Calls go through [`call_tool`], which checks the agent's scope, runs
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
use crate::session::{caller, Caller, Scope};
use crate::sim;

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
    /// Changes the world: journaled, returns the list of changes.
    Action,
}

/// Public description of a tool; `input_schema` is ready for an LLM `tools` field.
#[derive(Clone, Debug, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub scope: Scope,
    pub kind: ToolKind,
    /// One-line form for prompts and errors: `move_to(target: id|{x,z}, run?: bool)`.
    pub signature: String,
    pub input_schema: Value,
}

type RunFn = Box<dyn Fn(&mut World, Caller, Value) -> ToolResult<Value> + Send + Sync>;

pub struct AiTool {
    pub spec: ToolSpec,
    run: RunFn,
}

/// Tool registry, ordered by name (stable order keeps LLM prompt caches warm).
#[derive(Resource, Default)]
pub struct AiTools(BTreeMap<String, Arc<AiTool>>);

impl AiTools {
    pub fn get(&self, name: &str) -> Option<Arc<AiTool>> {
        self.0.get(name).cloned()
    }

    pub fn specs(&self, scope: Scope) -> Vec<ToolSpec> {
        self.0.values().filter(|t| scope.allows(t.spec.scope)).map(|t| t.spec.clone()).collect()
    }

    pub fn names(&self, scope: Scope) -> Vec<String> {
        self.0.values().filter(|t| scope.allows(t.spec.scope)).map(|t| t.spec.name.clone()).collect()
    }
}

/// Registers `system` as tool `name`. Prefer [`AiAppExt::add_ai_tool`](crate::AiAppExt).
pub fn register_tool<A, O, M, S>(world: &mut World, name: &str, scope: Scope, kind: ToolKind, system: S)
where
    A: DeserializeOwned + JsonSchema + Send + Sync + 'static,
    O: Serialize + 'static,
    S: IntoSystem<In<Call<A>>, ToolResult<O>, M> + Clone + Send + Sync + 'static,
{
    let (description, input_schema) = tool_schema::<A>();
    let signature = signature(name, &input_schema);
    let slot: Mutex<Option<SystemId<In<Call<A>>, ToolResult<O>>>> = Mutex::new(None);
    let tool_name = name.to_owned();
    let run: RunFn = Box::new(move |world: &mut World, caller: Caller, args: Value| {
        let args = if args.is_null() { json!({}) } else { args };
        let args: A = serde_json::from_value(args).map_err(|e| format!("bad arguments for {tool_name}: {e}"))?;
        let id = {
            let mut slot = slot.lock().map_err(|_| "tool registry poisoned".to_owned())?;
            *slot.get_or_insert_with(|| world.register_system(system.clone()))
        };
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
                Err(format!("{tool_name} panicked: {msg} (the game kept running; the panic is in the journal)"))
            }
        }
    });
    let spec = ToolSpec { name: name.to_owned(), description, scope, kind, signature, input_schema };
    world.get_resource_or_init::<AiTools>().0.insert(name.to_owned(), Arc::new(AiTool { spec, run }));
}

fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
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
}

const MAX_CHANGES: usize = 60;

/// Runs a tool on behalf of `agent`. The single entry point for every transport:
/// BRP, the in-process [`AiLink`](crate::AiLink), tests, an in-game agent.
pub fn call_tool(world: &mut World, agent: &str, tool: &str, args: Value) -> CallOutcome {
    let caller = caller(world, agent);
    let outcome = |world: &World, ok: bool, result: Value, changes, journal| CallOutcome {
        tool: tool.to_owned(),
        ok,
        result,
        tick: sim::tick(world),
        paused: sim::is_paused(world),
        changes,
        journal,
    };

    let Some(entry) = world.get_resource::<AiTools>().and_then(|t| t.get(tool)) else {
        let names = world.get_resource::<AiTools>().map(|t| t.names(caller.scope)).unwrap_or_default();
        let msg = format!("unknown tool `{tool}`; available: {}", names.join(", "));
        return outcome(world, false, Value::String(msg), Vec::new(), Vec::new());
    };
    if !caller.scope.allows(entry.spec.scope) {
        let msg = format!(
            "`{tool}` needs {} scope; agent `{}` has {} scope",
            entry.spec.scope, caller.agent, caller.scope
        );
        return outcome(world, false, Value::String(msg), Vec::new(), Vec::new());
    }

    if entry.spec.kind == ToolKind::Query {
        let result = (entry.run)(world, caller, args);
        let (ok, value) = match result {
            Ok(v) => (true, v),
            Err(e) => (false, Value::String(teach(e, &entry.spec))),
        };
        return outcome(world, ok, value, Vec::new(), Vec::new());
    }

    let origin = caller.subject.and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
    let before = digest(world);
    let args_text = if args.is_null() { String::new() } else { args.to_string() };
    let mark = journal::log(world, JournalKind::Agent, format!("{}: {tool} {args_text}", caller.agent));
    let result = (entry.run)(world, caller, args);
    let after = digest(world);

    let mut changes = diff(&before, &after, origin);
    if changes.len() > MAX_CHANGES {
        let extra = changes.len() - MAX_CHANGES;
        changes.truncate(MAX_CHANGES);
        changes.push(format!("… {extra} more changes"));
    }
    let journal: Vec<String> = world.resource::<AiJournal>().after(mark).0.map(ToString::to_string).collect();
    let (ok, value) = match result {
        Ok(v) => (true, v),
        Err(e) => (false, Value::String(teach(e, &entry.spec))),
    };
    outcome(world, ok, value, changes, journal)
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
/// inlined (LLM tool schemas handle `$ref` poorly), always `"type": "object"`.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    fn id_accepts_numbers_and_strings() {
        let a: Id = serde_json::from_value(json!(42)).unwrap();
        let b: Id = serde_json::from_value(json!("e42")).unwrap();
        assert_eq!(a, b);
        assert!(serde_json::from_value::<Id>(json!("x")).is_err());
        assert!(serde_json::from_value::<Id>(json!(0)).is_err());
    }
}
