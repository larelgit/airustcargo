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
//! | `ai/status`   | none (`instance`: pid, seed, port, start) |
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
    let seed = world.get_resource::<SimSeed>().map(|s| s.0);
    let instance = world.get_resource::<AiInstance>().map(|i| {
        json!({"pid": i.pid, "seed": seed, "port": i.port, "started_at": i.started_at})
    });
    to_brp(json!({
        "instance": instance,
        "tick": sim::tick(world),
        "paused": sim::is_paused(world),
        "speed": sim::speed(world),
        "tick_hz": (1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64() * 1000.0).round() / 1000.0,
        "seed": seed,
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

/// `Ok` if nothing listens on `127.0.0.1:port`. Check before building the app:
/// Bevy's HTTP server loses its bind error in a background task, and the game would
/// run without a server while `shards` talks to whatever holds the port.
pub fn ensure_port_free(port: u16) -> Result<(), String> {
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            Err(format!("port {port} is busy (another game?). Pick another with --port"))
        }
        Err(e) => Err(format!("port {port} is not available ({e}). Pick another with --port")),
    }
}

/// A port nobody listens on right now (`--port 0`): bind port 0, take the number, close.
pub fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| format!("cannot find a free port: {e}"))
}

/// This running game, as `ai/status` reports it (`instance`).
#[derive(Resource, Clone, Debug)]
pub struct AiInstance {
    pub pid: u32,
    pub port: Option<u16>,
    /// UTC, e.g. `2026-10-13T19:28:05Z`.
    pub started_at: String,
}

impl AiInstance {
    pub fn new(port: Option<u16>) -> Self {
        Self { pid: std::process::id(), port, started_at: utc_timestamp(std::time::SystemTime::now()) }
    }
}

/// `2026-10-13T19:28:05Z` (no date crate for one timestamp).
fn utc_timestamp(time: std::time::SystemTime) -> String {
    let secs = time.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's `civil_from_days`).
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// The game's port written to a file (`--port-file`), so parallel agents can start
/// games on `--port 0` and find them. Removed again when dropped (the game exits).
pub struct PortFile(std::path::PathBuf);

impl PortFile {
    /// Writes `port` to `path` atomically: readers see the whole number or nothing.
    pub fn write(path: &std::path::Path, port: u16) -> Result<Self, String> {
        let part = path.with_extension("part");
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        std::fs::write(&part, format!("{port}\n"))
            .and_then(|()| std::fs::rename(&part, path))
            .map_err(|e| format!("cannot write the port file {}: {e}", path.display()))?;
        Ok(Self(path.to_owned()))
    }
}

impl Drop for PortFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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

    #[test]
    fn a_busy_port_is_reported_with_the_fix() {
        let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        let err = ensure_port_free(port).unwrap_err();
        assert_eq!(err, format!("port {port} is busy (another game?). Pick another with --port"));
        drop(taken);
        assert_eq!(ensure_port_free(port), Ok(()));
        assert_ne!(free_port().unwrap(), 0);
    }

    #[test]
    fn status_names_the_instance() {
        let port = free_port().unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin { remote_port: Some(port), ..Default::default() });
        app.finish();
        app.cleanup();
        let status = call(app.world_mut(), "ai/status", json!({})).unwrap();
        let instance = &status["instance"];
        assert_eq!(instance["pid"], json!(std::process::id()));
        assert_eq!(instance["port"], json!(port));
        assert_eq!(instance["seed"], json!(crate::SimConfig::default().seed));
        assert!(instance["started_at"].as_str().is_some_and(|t| t.ends_with('Z')), "{instance}");
    }

    #[test]
    fn timestamps_are_utc_iso_8601() {
        let at = |secs: u64| utc_timestamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_760_383_685), "2025-10-13T19:28:05Z");
    }

    #[test]
    fn the_port_file_holds_the_port_while_the_game_runs() {
        let dir = std::env::temp_dir().join(format!("ai_core_port_file_{}", std::process::id()));
        let path = dir.join("game.port");
        let file = PortFile::write(&path, 15711).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "15711\n");
        drop(file);
        assert!(!path.exists(), "removed when the game exits");
        let _ = std::fs::remove_dir_all(dir);
    }
}
