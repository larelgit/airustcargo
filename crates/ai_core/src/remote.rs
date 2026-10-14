//! `ai/*` methods on the Bevy Remote Protocol (JSON-RPC 2.0 over HTTP).
//!
//! BRP itself already exposes `world.query`, `world.get_components`,
//! `world.insert_components`, ...; these methods add the agent-level layer on top.
//! Every call goes through [`call_tool`], like the game's own dev keys and the tests.
//!
//! | method        | params (plus `token`)                     |
//! |---------------|-------------------------------------------|
//! | `ai/act`      | `{agent?, tool, args?}`                   |
//! | `ai/observe`  | `{agent?, since_seq?}`                    |
//! | `ai/step`     | `{agent?, ticks?, until?}`                |
//! | `ai/journal`  | `{agent?, since_seq?, last?}`             |
//! | `ai/shot`     | `{agent?, marks?, path?}`                 |
//! | `ai/tools`    | `{agent?}`                                |
//! | `ai/session`  | `{agent, scope?, subject?}`               |
//! | `ai/status`   | none (`instance`: pid, seed, port, start; `loading`: what holds the simulation) |
//!
//! Full world access: compile it into dev builds only. It listens on 127.0.0.1, and
//! every method, Bevy's own included, wants `params.token`: the random [`AiToken`]
//! the game writes to `.shards/<port>.token` (owner-only) at start and removes on exit.

use bevy::prelude::*;
use bevy_remote::{BrpError, BrpResult, RemoteMethodSystemId, RemoteMethods};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::id::{AiId, AI_ID_IS_MANAGED};
use crate::rng::SimSeed;
use crate::session::{configure_session, session_json, AgentSessions, Scope, SessionChange};
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

/// `ai/step_poll {id}`: a finished deferred request answers with its whole outcome (changes and
/// journal lines included, like `ai/step` would have); otherwise the `step_poll` tool answers.
fn ai_step_poll(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: AgentParams = parse(params)?;
    if let Some(id) = p.rest.get("id").and_then(Value::as_u64)
        && let Some(queue) = crate::sim::deferred_queue(world)
        && let Ok(queue) = queue.lock()
        && let crate::sim::StepPoll::Done(outcome) = queue.poll(id)
    {
        return to_brp(outcome);
    }
    to_brp(call_tool(world, &p.agent, "step_poll", Value::Object(p.rest)))
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
    /// Play as this player's body (`PlayerId`), found again on every call.
    player: Option<u8>,
}

fn ai_session(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let p: SessionParams = parse(params)?;
    let change = SessionChange { scope: p.scope, subject: p.subject.map(|i| i.0), player: p.player, clear_subject: p.clear_subject };
    let session = configure_session(world, &p.agent, &change)
        .map_err(|message| BrpError { code: bevy_remote::error_codes::INVALID_PARAMS, message, data: None })?;
    to_brp(session_json(&session))
}

fn ai_status(In(_): In<Option<Value>>, world: &mut World) -> BrpResult {
    let seed = world.get_resource::<SimSeed>().map(|s| s.0);
    let instance = world.get_resource::<AiInstance>().map(|i| {
        json!({"pid": i.pid, "seed": seed, "port": i.port, "started_at": i.started_at})
    });
    let mut status = crate::status::status(world);
    status["instance"] = instance.unwrap_or(Value::Null);
    to_brp(status)
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
        ("ai/step_poll", world.register_system(ai_step_poll)),
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
/// [`AiIndex`](crate::AiIndex) in sync, so it refuses `AiId` the way the `set` tool does.
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

// --- access token ------------------------------------------------------------------

/// Folder in the game's working directory with one token file per port.
pub const TOKEN_DIR: &str = ".shards";

/// JSON-RPC error code for a missing or wrong token (the implementation-defined range).
pub const UNAUTHORIZED: i16 = -32001;

/// What a call without the right token gets back.
pub const UNAUTHORIZED_MESSAGE: &str = "unauthorized: run shards from the game folder or set SHARDS_TOKEN";

/// The secret every BRP call must carry as `params.token`. Any program on the machine
/// can reach a socket on 127.0.0.1; only one that can read the game's folder (or is
/// handed `SHARDS_TOKEN`) knows the token.
#[derive(Resource, Clone)]
pub struct AiToken(String);

impl AiToken {
    /// 128 random bits, hex.
    pub fn random() -> Self {
        Self(random_hex(16))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `.shards/<port>.token`, relative to the game's working directory: where `shards`
/// looks for the token (from its own folder upwards).
pub fn token_path(port: u16) -> std::path::PathBuf {
    std::path::Path::new(TOKEN_DIR).join(format!("{port}.token"))
}

/// The token on disk while the game runs: readable by its owner only, removed when
/// dropped (when the game exits).
#[derive(Resource)]
pub struct TokenFile {
    path: std::path::PathBuf,
    token: String,
}

impl TokenFile {
    pub fn write(path: &std::path::Path, token: &AiToken) -> Result<Self, String> {
        let fail = |e: std::io::Error| format!("cannot write the token file {}: {e}", path.display());
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(fail)?;
        }
        // A fresh file gets fresh permissions; the rename makes it appear whole.
        let part = path.with_extension("token.part");
        let _ = std::fs::remove_file(&part);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&part).map_err(fail)?;
        std::io::Write::write_all(&mut file, format!("{}\n", token.as_str()).as_bytes()).map_err(fail)?;
        std::fs::rename(&part, path).map_err(fail)?;
        Ok(Self { path: path.to_owned(), token: token.as_str().to_owned() })
    }
}

impl Drop for TokenFile {
    fn drop(&mut self) {
        // A newer game may own this port (and the file) by now.
        if std::fs::read_to_string(&self.path).is_ok_and(|t| t.trim() == self.token) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    let from_os = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf));
    if from_os.is_err() {
        // `RandomState` keys come from the OS as well.
        use std::hash::{BuildHasher, Hasher};
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        for (i, chunk) in buf.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_usize(i);
            h.write_u128(nanos);
            h.write_u32(std::process::id());
            chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Checks `params.token` and takes it out, so the method sees the params it expects
/// (`{"token": …}` alone becomes no params at all).
fn authorize(world: &World, mut params: Option<Value>) -> Result<Option<Value>, BrpError> {
    let expected = world.get_resource::<AiToken>().map(AiToken::as_str);
    let given = match &mut params {
        Some(Value::Object(map)) => map.remove("token"),
        _ => None,
    };
    let ok = match (expected, given.as_ref().and_then(Value::as_str)) {
        (Some(expected), Some(given)) => same_secret(expected, given),
        _ => false,
    };
    if !ok {
        return Err(BrpError { code: UNAUTHORIZED, message: UNAUTHORIZED_MESSAGE.into(), data: None });
    }
    if matches!(&params, Some(Value::Object(map)) if map.is_empty()) {
        params = None;
    }
    Ok(params)
}

/// Compares in time that does not depend on where the strings differ.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Wrapper systems already in the method table.
#[derive(Resource, Default)]
struct ProtectedMethods(bevy::platform::collections::HashSet<Entity>);

fn method_entity(method: RemoteMethodSystemId) -> Entity {
    match method {
        RemoteMethodSystemId::Instant(id) => id.entity(),
        RemoteMethodSystemId::Watching(id) => id.entity(),
    }
}

/// Puts every method in [`RemoteMethods`] behind the token: Bevy's `world.*`,
/// `registry.*`, `rpc.discover`, the `+watch` ones, and `ai/*`. Each entry is replaced
/// by a wrapper that checks and strips `params.token`, then runs the original system.
/// Wrappers are not wrapped again, so this can run any number of times.
pub fn protect_methods(world: &mut World) {
    let Some(methods) = world.get_resource::<RemoteMethods>() else {
        return;
    };
    let protected = world.get_resource::<ProtectedMethods>();
    let open: Vec<(String, RemoteMethodSystemId)> = methods
        .methods()
        .into_iter()
        .filter_map(|name| methods.get(&name).copied().map(|id| (name, id)))
        .filter(|(_, id)| protected.is_none_or(|p| !p.0.contains(&method_entity(*id))))
        .collect();
    for (name, method) in open {
        let wrapper = match method {
            RemoteMethodSystemId::Instant(inner) => RemoteMethodSystemId::Instant(world.register_system(
                move |In(params): In<Option<Value>>, world: &mut World| -> BrpResult {
                    let params = authorize(world, params)?;
                    world.run_system_with(inner, params).unwrap_or_else(|e| Err(BrpError::internal(e)))
                },
            )),
            RemoteMethodSystemId::Watching(inner) => RemoteMethodSystemId::Watching(world.register_system(
                move |In(params): In<Option<Value>>, world: &mut World| -> BrpResult<Option<Value>> {
                    let params = authorize(world, params)?;
                    world.run_system_with(inner, params).unwrap_or_else(|e| Err(BrpError::internal(e)))
                },
            )),
        };
        world.get_resource_or_init::<ProtectedMethods>().0.insert(method_entity(wrapper));
        world.resource_mut::<RemoteMethods>().insert(name, wrapper);
    }
}

/// Before every batch of requests: covers methods added after `Plugin::finish`.
pub(crate) fn protect_new_methods(methods: Option<Res<RemoteMethods>>, mut commands: Commands) {
    if methods.is_some_and(|m| m.is_changed()) {
        commands.queue(protect_methods);
    }
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
        let token = app.world().resource::<AiToken>().as_str().to_owned();
        assert!(call(app.world_mut(), "ai/status", json!({})).is_err(), "no token, no status");
        let status = call(app.world_mut(), "ai/status", json!({"token": token})).unwrap();
        let instance = &status["instance"];
        assert_eq!(instance["pid"], json!(std::process::id()));
        assert_eq!(instance["port"], json!(port));
        assert_eq!(instance["seed"], json!(crate::SimConfig::default().seed));
        assert!(instance["started_at"].as_str().is_some_and(|t| t.ends_with('Z')), "{instance}");
        let file = token_path(port);
        assert_eq!(std::fs::read_to_string(&file).unwrap().trim(), token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(app);
        assert!(!file.exists(), "the token file goes with the game");
    }

    #[test]
    fn status_has_the_timing_of_the_last_ticks() {
        let port = free_port().unwrap();
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin).add_plugins(crate::AiCorePlugin {
            remote_port: Some(port),
            tick_budget: Some(std::time::Duration::from_millis(250)),
            ..Default::default()
        });
        app.finish();
        app.cleanup();
        let token = app.world().resource::<AiToken>().as_str().to_owned();
        let none = call(app.world_mut(), "ai/status", json!({"token": token})).unwrap();
        assert_eq!(
            none["perf"],
            json!({"ticks": 0, "avg_ms": 0.0, "max_ms": 0.0, "worst_tick": null, "over_budget": 0, "budget_ms": 250.0})
        );
        crate::sim::step(app.world_mut(), 3, std::time::Duration::from_secs(5), |_| None).unwrap();
        let status = call(app.world_mut(), "ai/status", json!({"token": token})).unwrap();
        let perf = &status["perf"];
        assert_eq!((perf["ticks"].as_u64(), perf["over_budget"].as_u64()), (Some(3), Some(0)), "{perf}");
        let (avg, max) = (perf["avg_ms"].as_f64().unwrap(), perf["max_ms"].as_f64().unwrap());
        assert!(avg > 0.0 && max >= avg, "{perf}");
        assert!((1..=3).contains(&perf["worst_tick"].as_u64().unwrap()), "{perf}");
    }

    #[test]
    fn every_method_wants_the_token() {
        let mut world = World::new();
        world.insert_resource(AiToken("secret".into()));
        let echo = world.register_system(|In(params): In<Option<Value>>| -> BrpResult { Ok(json!({"params": params})) });
        let watch = world.register_system(|In(params): In<Option<Value>>| -> BrpResult<Option<Value>> {
            Ok(Some(json!({"watching": params})))
        });
        let mut methods = RemoteMethods::new();
        methods.insert("world.query", RemoteMethodSystemId::Instant(echo));
        methods.insert("world.get_components+watch", RemoteMethodSystemId::Watching(watch));
        world.insert_resource(methods);
        protect_methods(&mut world);
        protect_methods(&mut world); // wrappers are not wrapped twice

        let refused = |r: BrpResult| {
            let e = r.unwrap_err();
            assert_eq!((e.code, e.message.as_str()), (UNAUTHORIZED, UNAUTHORIZED_MESSAGE));
        };
        refused(call(&mut world, "world.query", json!({"data": {}})));
        refused(call(&mut world, "world.query", json!({"token": "guess", "data": {}})));
        refused(call(&mut world, "world.query", json!({"token": 1})));
        let Some(&RemoteMethodSystemId::Instant(query)) = world.resource::<RemoteMethods>().get("world.query") else {
            panic!("world.query is gone");
        };
        refused(world.run_system_with(query, None).unwrap());
        assert_eq!(call(&mut world, "world.query", json!({"token": "secret", "data": {}})).unwrap(), json!({"params": {"data": {}}}));
        assert_eq!(call(&mut world, "world.query", json!({"token": "secret"})).unwrap(), json!({"params": null}));

        let Some(&RemoteMethodSystemId::Watching(watching)) = world.resource::<RemoteMethods>().get("world.get_components+watch")
        else {
            panic!("the watching method is gone");
        };
        let mut watch_call = |params: Value| world.run_system_with(watching, Some(params)).unwrap();
        assert!(watch_call(json!({"entity": 1})).is_err());
        assert_eq!(watch_call(json!({"token": "secret", "entity": 1})).unwrap(), Some(json!({"watching": {"entity": 1}})));
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
