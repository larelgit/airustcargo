//! `ai/*` methods on the Bevy Remote Protocol (JSON-RPC 2.0 over HTTP).
//!
//! BRP itself already exposes `world.query`, `world.get_components`,
//! `world.insert_components`, ...; these methods add the agent-level layer on top.
//! Every call goes through [`call_tool`], exactly like the in-process [`AiLink`](crate::AiLink).
//!
//! | method        | params                                   |
//! |---------------|------------------------------------------|
//! | `ai/act`      | `{agent?, tool, args?}`                  |
//! | `ai/observe`  | `{agent?, since_seq?}`                   |
//! | `ai/step`     | `{agent?, ticks?, until?}`               |
//! | `ai/journal`  | `{agent?, since_seq?, last?}`            |
//! | `ai/tools`    | `{agent?, format?: "full"\|"llm"}`       |
//! | `ai/session`  | `{agent, scope?, subject?}`              |
//! | `ai/status`   | none                                     |
//!
//! Full world access: compile it into dev builds only, it listens on 127.0.0.1.

use bevy::prelude::*;
use bevy_remote::{BrpError, BrpResult, RemoteMethodSystemId, RemoteMethods};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::id::AiIndex;
use crate::journal::AiJournal;
use crate::rng::SimSeed;
use crate::session::{AgentSessions, Scope};
use crate::sim;
use crate::tools::{call_tool, AiTools, Id};

pub const DEFAULT_AGENT: &str = "dev";

fn default_agent() -> String {
    DEFAULT_AGENT.to_owned()
}

fn parse<T: for<'de> Deserialize<'de> + Default>(params: Option<Value>) -> Result<T, BrpError> {
    match params {
        None | Some(Value::Null) => Ok(T::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| BrpError {
            code: bevy_remote::error_codes::INVALID_PARAMS,
            message: e.to_string(),
            data: None,
        }),
    }
}

fn to_brp<T: serde::Serialize>(v: T) -> BrpResult {
    serde_json::to_value(v).map_err(BrpError::internal)
}

#[derive(Deserialize, Default)]
struct ActParams {
    #[serde(default = "default_agent")]
    agent: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    args: Value,
}

fn ai_act(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: ActParams = parse(params)?;
    if p.tool.is_empty() {
        return Err(BrpError {
            code: bevy_remote::error_codes::INVALID_PARAMS,
            message: "missing `tool`".into(),
            data: None,
        });
    }
    to_brp(call_tool(world, &p.agent, &p.tool, p.args))
}

#[derive(Deserialize)]
struct AgentParams {
    #[serde(default = "default_agent")]
    agent: String,
    #[serde(flatten)]
    rest: serde_json::Map<String, Value>,
}

impl Default for AgentParams {
    fn default() -> Self {
        Self { agent: default_agent(), rest: Default::default() }
    }
}

/// `ai/<tool>` shortcut: params minus `agent` become the tool's args.
fn shortcut(tool: &'static str) -> impl Fn(In<Option<Value>>, &mut World) -> BrpResult + Clone {
    move |In(params): In<Option<Value>>, world: &mut World| {
        let p: AgentParams = parse(params)?;
        to_brp(call_tool(world, &p.agent, tool, Value::Object(p.rest)))
    }
}

#[derive(Deserialize, Default)]
struct ToolsParams {
    #[serde(default = "default_agent")]
    agent: String,
    /// `full` (default) or `llm`: `[{name, description, input_schema}]` for an LLM API.
    #[serde(default)]
    format: Option<String>,
}

fn ai_tools(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: ToolsParams = parse(params)?;
    let scope = world.resource_mut::<AgentSessions>().get_or_create(&p.agent).scope;
    let specs = world.resource::<AiTools>().specs(scope);
    match p.format.as_deref() {
        Some("llm") => to_brp(
            specs
                .iter()
                .map(|s| json!({"name": s.name, "description": s.description, "input_schema": s.input_schema}))
                .collect::<Vec<_>>(),
        ),
        _ => to_brp(specs),
    }
}

#[derive(Deserialize, Default)]
struct SessionParams {
    #[serde(default = "default_agent")]
    agent: String,
    scope: Option<Scope>,
    subject: Option<Id>,
    /// Reset `subject` to the player character.
    #[serde(default)]
    clear_subject: bool,
}

fn ai_session(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: SessionParams = parse(params)?;
    if let Some(id) = p.subject {
        id.entity(world).map_err(|m| BrpError { code: bevy_remote::error_codes::INVALID_PARAMS, message: m, data: None })?;
    }
    let mut sessions = world.resource_mut::<AgentSessions>();
    let current = sessions.get_or_create(&p.agent).clone();
    let subject = if p.clear_subject { None } else { p.subject.map(|i| i.0).or(current.subject) };
    let s = sessions.configure(&p.agent, p.scope.unwrap_or(current.scope), subject).clone();
    to_brp(json!({"agent": s.name, "scope": s.scope, "subject": s.subject.map(|i| i.to_string()), "seen_seq": s.seen_seq}))
}

fn ai_status(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    to_brp(json!({
        "tick": sim::tick(world),
        "paused": sim::is_paused(world),
        "speed": sim::speed(world),
        "tick_hz": (1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64() * 1000.0).round() / 1000.0,
        "seed": world.get_resource::<SimSeed>().map(|s| s.0),
        "entities": world.resource::<AiIndex>().len(),
        "journal_seq": world.resource::<AiJournal>().last_seq(),
        "agents": world.resource::<AgentSessions>().names(),
        "tools": world.resource::<AiTools>().names(Scope::Dev),
    }))
}

/// Registers the `ai/*` methods. Runs in `Plugin::finish`, after `RemotePlugin` built
/// its method table (it would overwrite methods inserted earlier).
pub(crate) fn register_methods(world: &mut World) {
    if !world.contains_resource::<RemoteMethods>() {
        warn!("ai_core: RemotePlugin missing, ai/* methods not registered");
        return;
    }
    let methods = [
        ("ai/act", world.register_system(ai_act)),
        ("ai/observe", world.register_system(shortcut("observe"))),
        ("ai/step", world.register_system(shortcut("step"))),
        ("ai/journal", world.register_system(shortcut("journal"))),
        ("ai/tools", world.register_system(ai_tools)),
        ("ai/session", world.register_system(ai_session)),
        ("ai/status", world.register_system(ai_status)),
    ];
    let mut table = world.resource_mut::<RemoteMethods>();
    for (name, id) in methods {
        table.insert(name, RemoteMethodSystemId::Instant(id));
    }
}

/// Fails fast with a clear message: Bevy's HTTP server drops bind errors silently.
pub(crate) fn check_port(port: u16) {
    if let Err(e) = std::net::TcpListener::bind(("127.0.0.1", port)) {
        error!("ai_core: BRP port {port} is not available ({e}); `shards` will not reach this game. Pick another with --port.");
    }
}
