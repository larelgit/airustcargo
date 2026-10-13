//! `ai/*` methods on the Bevy Remote Protocol (JSON-RPC 2.0 over HTTP).
//!
//! BRP itself already exposes `world.query`, `world.get_components`,
//! `world.insert_components`, ...; these methods add the agent-level layer on top.
//! Every call goes through [`call_tool`], like the game's own dev keys and the tests.
//!
//! | method        | params                                   |
//! |---------------|------------------------------------------|
//! | `ai/act`      | `{agent?, tool, args?}`                  |
//! | `ai/observe`  | `{agent?, since_seq?}`                   |
//! | `ai/step`     | `{agent?, ticks?, until?}`               |
//! | `ai/journal`  | `{agent?, since_seq?, last?}`            |
//! | `ai/shot`     | `{agent?, marks?, path?}`                |
//! | `ai/tools`    | `{agent?}`                               |
//! | `ai/session`  | `{agent, scope?, subject?}`              |
//! | `ai/status`   | none                                     |
//!
//! Full world access: compile it into dev builds only, it listens on 127.0.0.1.

use bevy::prelude::*;
use bevy_remote::{BrpError, BrpResult, RemoteMethodSystemId, RemoteMethods};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::id::{AiId, AiIndex, AI_ID_IS_MANAGED};
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
}

/// Every tool this agent may call: name, description, scope, kind, signature and
/// the JSON Schema of its arguments.
fn ai_tools(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: ToolsParams = parse(params)?;
    let scope = world.resource_mut::<AgentSessions>().get_or_create(&p.agent).scope;
    to_brp(world.resource::<AiTools>().specs(scope))
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
        "replay": crate::replay::status(world),
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
        ("ai/shot", world.register_system(shortcut("shot"))),
        ("ai/tools", world.register_system(ai_tools)),
        ("ai/session", world.register_system(ai_session)),
        ("ai/status", world.register_system(ai_status)),
    ];
    let mut table = world.resource_mut::<RemoteMethods>();
    for (name, id) in methods {
        table.insert(name, RemoteMethodSystemId::Instant(id));
    }
    guard_ai_id(world);
}

/// BRP's `world.mutate_components` writes in place, past the hooks that keep
/// [`AiIndex`] in sync, so it refuses `AiId` the way the `set` tool does.
fn guard_ai_id(world: &mut World) {
    let method = bevy_remote::builtin_methods::BRP_MUTATE_COMPONENTS_METHOD;
    let Some(&RemoteMethodSystemId::Instant(mutate)) = world.resource::<RemoteMethods>().get(method) else {
        return;
    };
    let guarded = world.register_system(move |In(params): In<Option<Value>>, world: &mut World| -> BrpResult {
        let component = params.as_ref().and_then(|p| p.get("component")).and_then(Value::as_str);
        if component == Some(<AiId as TypePath>::type_path()) {
            return Err(BrpError::component_error(AI_ID_IS_MANAGED));
        }
        world.run_system_with(mutate, params).unwrap_or_else(|e| Err(BrpError::internal(e)))
    });
    world.resource_mut::<RemoteMethods>().insert(method, RemoteMethodSystemId::Instant(guarded));
}

/// Fails fast with a clear message: Bevy's HTTP server drops bind errors silently.
pub(crate) fn check_port(port: u16) {
    if let Err(e) = std::net::TcpListener::bind(("127.0.0.1", port)) {
        error!("ai_core: BRP port {port} is not available ({e}); `shards` will not reach this game. Pick another with --port.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(world: &mut World, method: &str, params: Value) -> BrpResult {
        let Some(&RemoteMethodSystemId::Instant(id)) = world.resource::<RemoteMethods>().get(method) else {
            panic!("{method} is not an instant method");
        };
        world.run_system_with(id, Some(params)).expect("method system runs")
    }

    #[test]
    fn brp_mutation_of_ai_id_is_refused() {
        let mut world = World::new();
        let original = world.register_system(|In(_): In<Option<Value>>| -> BrpResult { Ok(json!("mutated")) });
        let mut methods = RemoteMethods::new();
        methods.insert(bevy_remote::builtin_methods::BRP_MUTATE_COMPONENTS_METHOD, RemoteMethodSystemId::Instant(original));
        world.insert_resource(methods);
        guard_ai_id(&mut world);

        let mutate = bevy_remote::builtin_methods::BRP_MUTATE_COMPONENTS_METHOD;
        let params = |component: &str| json!({"entity": 1, "component": component, "path": "", "value": 99});
        let refused = call(&mut world, mutate, params("ai_core::id::AiId")).unwrap_err();
        assert_eq!(refused.message, AI_ID_IS_MANAGED);
        assert_eq!(call(&mut world, mutate, params("game::Health")).unwrap(), json!("mutated"));
    }
}
