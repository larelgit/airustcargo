//! `shards` — talk to a running game through its `ai/*` BRP methods.
//!
//! Made for three users at once: you in a terminal, a coding agent driving it through
//! bash (plain text out, no tool definitions eating its context), and scripts/tests.
//!
//! ```text
//! shards observe
//! shards step 120 --until journal:phase --until gone:57
//! shards act move_to target=42
//! shards act dodge dir=W
//! shards shot                      # PNG of the window with eN marks, plus a legend
//! shards save-replay               # this session as a replay file (cargo sandbox --replay F)
//! shards save-snapshot             # the state at this tick as a file (snapshots/tick<T>.json)
//! shards load-snapshot snapshots/tick120.json   # back to it: the next step continues from there
//! shards assert add hp 'field:6:Health.current<0'   # halt the game when it breaks
//! shards fuzz --seed 7 --steps 300 # random inputs until a rule breaks
//! shards --port-file F wait        # a game started with `--port 0 --port-file F`; waits for loading
//! shards --port-file F wait --peer all   # a stand (`--stand 2`): its host and every client
//! shards --peer c1 observe         # one member of a stand, found by F.c1
//! shards stand                     # the stand's members: role, player, body, port, ping
//! shards stand stop                # the host ends the session, then the stand's process ends
//! shards content check             # every content file parsed and validated
//! shards worldgen --region meadow --seeds 1..1000 --report   # batch check of the generator
//! shards scenario a.scenario b.scenario   # commands, expectations, invariants: exit 0 / 1 failed / 2 error
//! shards tools
//! shards raw world.query '{"data":{"components":["sandbox::gameplay::combat::Health"]}}'
//! ```
//!
//! Exit codes: 0 done · 1 the tool refused (the error says why) · 2 `shards` itself
//! failed (no game at the port, no token, a bad command line) · 3 the game halted: a
//! `step` stopped at a failed gameplay system or a violated invariant.

use std::io::{self, ErrorKind, Write};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};
use shards_script::{parse_args, parse_condition};

#[cfg(test)]
mod fake;
mod peer;
mod scenario;

use peer::Peer;

/// Bevy's BRP port, used when neither `--port` nor `--port-file` says otherwise.
const DEFAULT_PORT: u16 = 15702;

/// How often a deferred `step` is asked whether it is done.
const POLL_EVERY: Duration = Duration::from_millis(20);

/// How long `shards` waits for a deferred `step` (and for `wait`, `shot` and `stand stop` to
/// finish) unless `--timeout` says otherwise.
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Exit codes besides 0. Clap exits with 2 on a bad command line as well.
mod exit {
    /// The tool refused: the error says why and how to fix the call.
    pub const TOOL_ERROR: u8 = 1;
    /// `shards` itself failed: no game at the port, no token, a bad argument.
    pub const SHARDS_ERROR: u8 = 2;
    /// The game halted during the call: a gameplay system failed or an invariant was
    /// violated, and `step` stopped there.
    pub const HALTED: u8 = 3;
}

#[derive(Parser)]
#[command(name = "shards", version, about = "Drive a running game through its ai/* BRP methods")]
struct Cli {
    /// Game's BRP port (each parallel instance gets its own). Default 15702.
    #[arg(long, short, global = true, env = "SHARDS_PORT")]
    port: Option<u16>,
    /// Read the port from the file the game wrote (`cargo sandbox --port 0 --port-file F`).
    /// `--port` wins when both are given.
    #[arg(long, global = true, env = "SHARDS_PORT_FILE", value_name = "PATH")]
    port_file: Option<std::path::PathBuf>,
    /// The game's access token. Default: `.shards/<port>.token`, looked up from this
    /// folder upwards (the game writes it into its working directory at start). Every game of
    /// a stand has its own token: in `wait --peer all`, `stand` and `scenario` this one is the
    /// host's, and a client's comes from `SHARDS_TOKEN_C1`, `SHARDS_TOKEN_C2` … (or its file).
    #[arg(long, global = true, env = "SHARDS_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Which game of a stand (`cargo sandbox --stand 2 --port-file F`) a command talks to:
    /// `host` (the default), `c1`, `c2`. Its port is read from F, F.c1, F.c2. `all` is for
    /// `wait` (every member) and `stand`.
    #[arg(long, global = true, env = "SHARDS_PEER", value_name = "MEMBER")]
    peer: Option<Peer>,
    /// Seconds to wait for something that takes a while: a deferred `step` of a stand (default
    /// 60), `wait` (60), `shot` (10), `stand stop` (15). The `SHARDS_TIMEOUT` environment
    /// variable works too.
    #[arg(long, global = true, env = "SHARDS_TIMEOUT", value_name = "SECONDS")]
    timeout: Option<u64>,
    /// Agent name: each agent has its own journal cursor, scope and body.
    #[arg(long, short, global = true, env = "SHARDS_AGENT", default_value = "dev")]
    agent: String,
    /// Print raw JSON responses.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// What you see now, plus journal entries since your last look.
    Observe,
    /// Advance the paused simulation. Conditions: journal:TEXT, gone:ID, shows:ID:TEXT,
    /// near:A:B:METERS, field:ID:Component.path<N (or >N), named:NAME, warning, error,
    /// or raw JSON. Exit code 3 when the game halted (a failed system, a broken rule).
    Step {
        /// Maximum ticks (default 1, or 600 with --until).
        ticks: Option<u64>,
        #[arg(long, short)]
        until: Vec<String>,
        /// Print a fresh observation afterwards.
        #[arg(long, short)]
        observe: bool,
    },
    /// Call any tool. ARGS: one JSON object, or key=value pairs (values parsed as JSON when possible).
    Act { tool: String, args: Vec<String> },
    /// Screenshot of the game window with every eN id drawn on it; prints the path
    /// (waits until the file is written) and where each entity is in the image.
    Shot {
        /// Leave the id marks out.
        #[arg(long)]
        no_marks: bool,
        /// Where to save the PNG (default: shots/tick<T>-<N>.png next to the game).
        #[arg(long, short)]
        out: Option<String>,
    },
    /// Save the session so far as a replay: seed, inputs, action calls, state checks.
    /// Play it with `cargo sandbox --replay FILE` (add `--verify` to check a build).
    SaveReplay {
        /// Default: replays/tick<T>.json next to the game.
        path: Option<String>,
    },
    /// Save the simulation state at this tick as a snapshot: every entity with its id and
    /// components, the tick, the id counters. Load it with `load-snapshot` (here, or in a
    /// fresh game with the same seed and content); a replay recorded after loading one
    /// starts from it.
    SaveSnapshot {
        /// Default: snapshots/tick<T>.json next to the game.
        path: Option<String>,
    },
    /// Load a snapshot written by `save-snapshot`: the next `step` continues as the original
    /// session did ("go to boss phase" is a snapshot taken there). Refused, changing nothing,
    /// when the game is not the one it was taken in; the error says what differs.
    LoadSnapshot {
        /// A file inside snapshots/.
        path: String,
        /// Load although the seed, engine version or content differ.
        #[arg(long)]
        force: bool,
    },
    /// Run scenario files against the game: shards commands with expectations and
    /// invariants, one command per line (format: see shards_script, README "Сценарии").
    /// Exit code 0 all passed, 1 a scenario failed, 2 usage or connection error.
    Scenario {
        #[arg(required = true, value_name = "FILE")]
        files: Vec<std::path::PathBuf>,
        /// Print only the commands that failed and the verdicts.
        #[arg(long, short)]
        quiet: bool,
    },
    /// Rules the game must keep, checked after every tick: the game's own and this
    /// session's. A broken rule halts the game, and `step` stops there (exit code 3).
    Assert {
        #[command(subcommand)]
        op: AssertCmd,
    },
    /// Random player inputs (move, attack, dodge; targets from `observe`) between short
    /// steps, until the game halts or the steps run out. Prints the replay to reproduce
    /// it with; exit code 3 if the game halted.
    Fuzz {
        /// Seed of the inputs (the game has its own): the same seed against the same
        /// fresh game makes the same calls.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Inputs to make, each followed by a step.
        #[arg(long, default_value_t = 200)]
        steps: u32,
        /// Longest step after an input, in ticks.
        #[arg(long, default_value_t = 30)]
        max_ticks: u64,
    },
    /// List the tools available to this agent (--json: with argument schemas).
    Tools,
    /// Everything about one entity.
    Inspect { id: String },
    /// Entities around you (or around --around).
    Nearby {
        radius: Option<f32>,
        #[arg(long, short)]
        filter: Option<String>,
        #[arg(long)]
        around: Option<String>,
    },
    /// Journal entries.
    Journal {
        #[arg(long)]
        since: Option<u64>,
        #[arg(long)]
        last: Option<usize>,
    },
    Pause,
    Resume,
    /// Real-time speed multiplier.
    Speed { x: f32 },
    /// Tick, pause state, seed, agents, tools.
    Status,
    /// Configure this agent: scope (player|dev) and body.
    Session {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, conflicts_with_all = ["player", "clear_subject"])]
        subject: Option<String>,
        /// Play as the body of this player (0 is the host's), found again on every call: it
        /// survives a respawn.
        #[arg(long, conflicts_with = "clear_subject")]
        player: Option<u8>,
        #[arg(long)]
        clear_subject: bool,
    },
    /// The members of a stand (`cargo sandbox --stand 2 --port-file F`): role, player, body,
    /// port, ping and desyncs of the host and every client. `stand stop` has the host end the
    /// session (the clients are told and keep their characters), then ends the stand's process.
    /// A client that is already gone is named, and the exit code is 1.
    Stand {
        #[command(subcommand)]
        op: Option<StandCmd>,
    },
    /// Content files: `check` reads every file from disk and validates it (exit 1 on
    /// problems). Without a running game: `cargo sandbox --check-content`.
    Content {
        #[command(subcommand)]
        cmd: ContentCmd,
    },
    /// Generate a region for many seeds in the running game and print the batch report:
    /// seeds passed, failed attempts by reason, distances between points of interest, panics
    /// (exit 1 unless every seed passed). Without a game: `<game> --worldgen-report R --seeds S`.
    Worldgen {
        /// `region` or `dungeon` content id.
        #[arg(long)]
        region: String,
        /// `1..1000` (both ends included), `7`, `1,5,10..20`.
        #[arg(long, default_value = "1..100")]
        seeds: String,
        /// Print the batch report (what this command does; the flag reads well in scripts).
        #[arg(long)]
        report: bool,
    },
    /// Any BRP method (world.query, world.get_components, rpc.discover, ...).
    Raw { method: String, params: Option<String> },
    /// Wait until the game answers and has finished loading (for scripts). With `--peer all`:
    /// the host and every client of a stand, one line each, and a check that they are one
    /// session. Gives up after --timeout seconds (60).
    Wait,
}

#[derive(Subcommand)]
enum StandCmd {
    /// End the stand: the host ends the session, then gets SIGTERM; waits until no member answers.
    Stop,
}

#[derive(Subcommand)]
enum ContentCmd {
    /// Parse and validate every content file on disk; applies nothing.
    Check,
}

#[derive(Subcommand)]
enum AssertCmd {
    /// Add (or replace) a rule: the game halts when WHEN becomes true. WHEN is a --until
    /// condition: `shards assert add hp 'field:6:Health.current<0'`, `… quiet error`.
    Add { name: String, when: String },
    /// Every rule, armed or fired.
    List,
    /// Remove a rule added with `assert add`.
    Remove { name: String },
    /// Re-arm every rule that fired.
    Reset,
}

struct Client {
    url: String,
    agent: ureq::Agent,
    port: u16,
    token: Option<String>,
    /// How long `act` waits for a deferred `step`.
    timeout: Duration,
}

impl Client {
    fn new(port: u16, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { url: format!("http://127.0.0.1:{port}"), agent, port, token, timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS) }
    }

    fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// `ai/act`. A game that cannot run ticks by itself (a member of a stand: one tick is a tick
    /// of every member) answers `step`, `pause`, `resume` and `speed` with `{"pending": id}`;
    /// this polls `ai/step_poll` every 20 ms until the request is done and returns what the call
    /// would have answered, `halted` and journal lines included. Any other game answers right
    /// away and nothing is polled.
    fn act(&self, agent: &str, tool: &str, args: Value) -> Result<Value, String> {
        let reply = self.call("ai/act", json!({"agent": agent, "tool": tool, "args": args}))?;
        match pending_id(tool, &reply) {
            Some(id) => self.poll(agent, id),
            None => Ok(reply),
        }
    }

    fn poll(&self, agent: &str, id: u64) -> Result<Value, String> {
        let started = Instant::now();
        loop {
            let reply = self.call("ai/step_poll", json!({"agent": agent, "id": id}))?;
            if pending_id("step_poll", &reply) != Some(id) {
                return Ok(reply);
            }
            if started.elapsed() >= self.timeout {
                return Err(format!(
                    "request {id} is still running after {}s; it goes on in the game: ask again with `shards act step_poll id={id}`, or wait longer with --timeout",
                    self.timeout.as_secs()
                ));
            }
            std::thread::sleep(POLL_EVERY);
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let params = with_token(params, self.token.as_deref())?;
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut resp = self.agent.post(&self.url).send_json(&body).map_err(|e| match e {
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => {
                format!("cannot reach the game at {} ({e}). Is it running? Try `cargo sandbox`.", self.url)
            }
            e => format!("request failed: {e}"),
        })?;
        let reply: Value = resp.body_mut().read_json().map_err(|e| format!("bad response: {e}"))?;
        if let Some(err) = reply.get("error") {
            let msg = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
            if self.token.is_none() && msg.starts_with("unauthorized") {
                let here = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
                return Err(format!("{method}: {msg} (no .shards/{}.token in {here} or above)", self.port));
            }
            if msg.starts_with("unauthorized") {
                // A stand has a token for every member: one token cannot open them all.
                return Err(format!(
                    "{method}: {msg} (the token given is not this game's, the one of port {}: every game of a stand has its own, \
                     in .shards/{}.token; for a client of a stand, SHARDS_TOKEN_C1, SHARDS_TOKEN_C2 …)",
                    self.port, self.port
                ));
            }
            return Err(format!("{method}: {msg}"));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

/// The id a deferred request answered with: `step`, `pause`, `resume`, `speed` (and the poll
/// itself) say `{"pending": 3}` as their result while it runs.
fn pending_id(tool: &str, reply: &Value) -> Option<u64> {
    if !matches!(tool, "step" | "pause" | "resume" | "speed" | "step_poll") || reply.get("ok") == Some(&Value::Bool(false)) {
        return None;
    }
    let result = reply.get("result")?.as_object()?;
    if result.len() != 1 {
        return None;
    }
    result.get("pending")?.as_u64()
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match (&cli.cmd, cli.peer) {
        // The game may still be starting: its port file may not exist yet.
        (Cmd::Wait, Some(Peer::All)) => peer::wait_all(&cli, cli.timeout_or(DEFAULT_TIMEOUT_SECS)).map(|()| ExitCode::SUCCESS),
        (Cmd::Wait, _) => wait(&cli, cli.timeout_or(DEFAULT_TIMEOUT_SECS)),
        (Cmd::Stand { op }, _) => stand(&cli, op.as_ref()),
        (_, Some(Peer::All)) => Err("--peer all is for `wait` and `stand`; name one member: host, c1, c2".to_owned()),
        _ => cli.client().and_then(|client| run(&cli, &client)),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(exit::SHARDS_ERROR)
        }
    }
}

/// `shards stand` and `shards stand stop`: exit code 1 when a member was missing (it is
/// named), after everything that could be done was.
fn stand(cli: &Cli, op: Option<&StandCmd>) -> Result<ExitCode, String> {
    let complete = match op {
        None => peer::show(cli)?,
        Some(StandCmd::Stop) => peer::stop(cli, &peer::terminate)?,
    };
    Ok(if complete { ExitCode::SUCCESS } else { ExitCode::from(exit::TOOL_ERROR) })
}

impl Cli {
    /// The BRP port of the game a command talks to: `--port`, else the number in `--port-file`
    /// (the file of the member `--peer` names), else the default.
    fn port(&self) -> Result<u16, String> {
        self.port_of(self.peer.unwrap_or(Peer::Host))
    }

    fn client(&self) -> Result<Client, String> {
        Ok(self.client_at(self.port()?))
    }

    /// `--timeout` or `default` seconds.
    fn timeout_or(&self, default: u64) -> Duration {
        Duration::from_secs(self.timeout.unwrap_or(default))
    }

    /// How long to wait for a deferred `step`.
    fn poll_timeout(&self) -> Duration {
        self.timeout_or(DEFAULT_TIMEOUT_SECS)
    }
}

/// `.shards/<port>.token` in `start` or the nearest folder above it that has one.
fn find_token(port: u16, start: &std::path::Path) -> Option<String> {
    let name = format!("{port}.token");
    start.ancestors().find_map(|dir| std::fs::read_to_string(dir.join(".shards").join(&name)).ok()).map(|t| t.trim().to_owned())
}

/// Every method wants the game's token in its params.
fn with_token(params: Value, token: Option<&str>) -> Result<Value, String> {
    let Some(token) = token else {
        return Ok(params);
    };
    match params {
        Value::Null => Ok(json!({"token": token})),
        Value::Object(mut map) => {
            map.insert("token".into(), token.into());
            Ok(Value::Object(map))
        }
        _ => Err("params must be a JSON object: it carries the game's token".into()),
    }
}

fn read_port_file(path: &std::path::Path) -> Result<u16, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read the port file {} ({e}); is the game running with --port-file?", path.display()))?;
    text.trim().parse().map_err(|_| format!("the port file {} holds no port number: `{}`", path.display(), text.trim()))
}

/// Polls `ai/status` until the game answers and has finished loading, then prints who
/// answered.
fn wait(cli: &Cli, timeout: Duration) -> Result<ExitCode, String> {
    // The game may still be starting: its port file may not exist yet.
    let status = peer::wait_member(|| cli.client(), Instant::now() + timeout)?;
    say(ready_line(&status));
    Ok(ExitCode::SUCCESS)
}

/// What holds the game's simulation (`content: 2 files left`); empty once it can
/// tick. A game older than the `loading` field counts as loaded.
fn still_loading(status: &Value) -> Vec<String> {
    let reasons = status.get("loading").and_then(Value::as_array);
    reasons.map(|r| r.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned)).collect()).unwrap_or_default()
}

/// `ready: tick 0 · pid 4242 · seed 1 · port 15711`, the line the game prints at start.
fn ready_line(status: &Value) -> String {
    let field = |v: &Value| match v {
        Value::Null => "?".to_owned(),
        other => other.to_string(),
    };
    let instance = &status["instance"];
    let mut line = format!(
        "ready: tick {} · pid {} · seed {} · port {}",
        field(&status["tick"]),
        field(&instance["pid"]),
        field(&status["seed"]),
        field(&instance["port"])
    );
    // A member of a network session: who it is there.
    let net = &status["net"];
    if let Some(role) = net["role"].as_str().filter(|r| *r != "single") {
        if let Some(member) = net["member"].as_str().filter(|m| !m.is_empty()) {
            line.push_str(&format!(" · member {member}"));
        }
        line.push_str(&format!(" · role {role}"));
        if let Some(player) = net["player"].as_u64() {
            line.push_str(&format!(" · player {player}"));
        }
        if let Some(body) = net["body"].as_str() {
            line.push_str(&format!(" · body {body}"));
        }
        // A host's bound UDP address: what `--join` takes (`--host 0` picks a free port).
        if let Some(udp) = net["udp"].as_str() {
            line.push_str(&format!(" · udp {udp}"));
        }
    }
    line
}

fn run(cli: &Cli, client: &Client) -> Result<ExitCode, String> {
    let agent = cli.agent.as_str();
    let act = |tool: &str, args: Value| client.act(agent, tool, args);
    let outcome = match &cli.cmd {
        Cmd::Observe => act("observe", json!({}))?,
        Cmd::Step { ticks, until, observe } => {
            let until: Vec<Value> = until.iter().map(|u| parse_condition(u)).collect::<Result<_, _>>()?;
            let mut args = json!({"until": until});
            if let Some(t) = ticks {
                args["ticks"] = json!(t);
            }
            let out = act("step", args)?;
            let code = print_outcome(&out, cli.json);
            if *observe && !cli.json {
                say("");
                print_outcome(&act("observe", json!({}))?, false);
            }
            return Ok(code);
        }
        Cmd::Act { tool, args } => act(tool, parse_args(args)?)?,
        Cmd::SaveReplay { path } => act("replay_save", json!({"path": path}))?,
        Cmd::SaveSnapshot { path } => act("snapshot_save", json!({"path": path}))?,
        Cmd::LoadSnapshot { path, force } => act("snapshot_load", json!({"path": path, "force": force}))?,
        Cmd::Scenario { files, quiet } => return scenario::run(cli, client, files, *quiet, cli.json),
        Cmd::Assert { op } => match op {
            AssertCmd::Add { name, when } => act("assert_add", json!({"name": name, "when": parse_condition(when)?}))?,
            AssertCmd::List => act("assert_list", json!({}))?,
            AssertCmd::Remove { name } => act("assert_remove", json!({"name": name}))?,
            AssertCmd::Reset => act("assert_reset", json!({}))?,
        },
        Cmd::Fuzz { seed, steps, max_ticks } => return fuzz(&act, *seed, *steps, *max_ticks),
        Cmd::Shot { no_marks, out } => {
            let reply = act("shot", json!({"marks": !no_marks, "path": out}))?;
            if reply.get("ok") != Some(&Value::Bool(true)) {
                return Ok(print_outcome(&reply, cli.json));
            }
            let ticket = reply.get("result").cloned().unwrap_or_default();
            let path = |k: &str| ticket.get(k).and_then(Value::as_str).map(str::to_owned);
            let (png, legend) = path("path").zip(path("legend_path")).ok_or("shot: unexpected reply")?;
            wait_for_file(&png, cli.timeout_or(10))?;
            // Written before the PNG is renamed into place, so it is there now.
            let info: Value = std::fs::read_to_string(&legend)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .ok_or_else(|| format!("{png} is saved, but its legend {legend} is not readable"))?;
            if cli.json {
                say(serde_json::to_string_pretty(&info).unwrap_or_default());
            } else {
                print_shot(&info);
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Tools => {
            let tools = client.call("ai/tools", json!({"agent": agent}))?;
            if cli.json {
                say(serde_json::to_string_pretty(&tools).unwrap_or_default());
            } else {
                for t in tools.as_array().into_iter().flatten() {
                    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("");
                    say(format!("{}  [{} {}]", s("signature"), s("scope"), s("kind")));
                    for line in s("description").lines() {
                        say(format!("    {line}"));
                    }
                }
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Inspect { id } => act("inspect", json!({"id": id}))?,
        Cmd::Nearby { radius, filter, around } => {
            let mut args = Map::new();
            if let Some(r) = radius {
                args.insert("radius".into(), json!(r));
            }
            if let Some(f) = filter {
                args.insert("filter".into(), json!(f));
            }
            if let Some(a) = around {
                args.insert("around".into(), json!(a));
            }
            act("nearby", Value::Object(args))?
        }
        Cmd::Journal { since, last } => act("journal", json!({"since_seq": since, "last": last}))?,
        Cmd::Pause => act("pause", json!({}))?,
        Cmd::Resume => act("resume", json!({}))?,
        Cmd::Speed { x } => act("speed", json!({"x": x}))?,
        Cmd::Status => {
            let s = client.call("ai/status", Value::Null)?;
            if !cli.json {
                say(ready_line(&s));
            }
            say(serde_json::to_string_pretty(&s).unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Session { scope, subject, player, clear_subject } => {
            let s = client.call(
                "ai/session",
                json!({"agent": agent, "scope": scope, "subject": subject, "player": player, "clear_subject": clear_subject}),
            )?;
            say(serde_json::to_string_pretty(&s).unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Content { cmd: ContentCmd::Check } => {
            let out = act("content_check", json!({}))?;
            let clean = out.get("result").and_then(Value::as_str).is_some_and(|r| r.starts_with("ok:"));
            let code = print_outcome(&out, cli.json);
            return Ok(if clean { code } else { ExitCode::FAILURE });
        }
        Cmd::Worldgen { region, seeds, report: _ } => {
            let out = act("worldgen_report", json!({"region": region, "seeds": seeds}))?;
            let clean = out.get("result").and_then(Value::as_str).is_some_and(|r| r.starts_with("ok:"));
            let code = print_outcome(&out, cli.json);
            return Ok(if clean { code } else { ExitCode::FAILURE });
        }
        Cmd::Raw { method, params } => {
            let params = match params {
                Some(p) => serde_json::from_str(p).map_err(|e| format!("params must be JSON: {e}"))?,
                None => Value::Null,
            };
            say(serde_json::to_string_pretty(&client.call(method, params)?).unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Wait => return wait(cli, cli.timeout_or(DEFAULT_TIMEOUT_SECS)),
        Cmd::Stand { op } => return stand(cli, op.as_ref()),
    };
    Ok(print_outcome(&outcome, cli.json))
}

/// The game writes the PNG under a temporary name and renames it when complete, so
/// the file existing means it is ready.
fn wait_for_file(path: &str, timeout: Duration) -> Result<(), String> {
    let start = Instant::now();
    while !std::path::Path::new(path).exists() {
        if start.elapsed() > timeout {
            return Err(format!(
                "the game did not save {path} within {}s (window minimized? see `shards journal`)",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn print_shot(info: &Value) {
    let n = |k: &str| info.get(k).and_then(Value::as_u64).unwrap_or(0);
    let marks = if info.get("marks") == Some(&Value::Bool(true)) { "marks on" } else { "no marks" };
    let path = info.get("path").and_then(Value::as_str).unwrap_or("");
    say(format!("saved {path} ({}x{}, tick {}, {marks})", n("width"), n("height"), n("tick")));
    let legend: Vec<&str> =
        info.get("legend").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    if legend.is_empty() {
        say("LEGEND  nothing AI-visible in the picture");
    } else {
        say("LEGEND (image pixels)");
        for line in legend {
            say(format!("  {line}"));
        }
    }
}

/// Prints one line of command output, as `println!` does. A reader that has gone away
/// (`shards observe | head -1`) is the end of the output, not an error: the rest of the
/// output is dropped quietly instead of panicking, and the command still finishes and
/// exits with its own code (a failed `shards scenario` stays a failure). Any other write
/// error ends the process with code 2.
fn say(line: impl std::fmt::Display) {
    static GONE: AtomicBool = AtomicBool::new(false);
    if GONE.load(Ordering::Relaxed) {
        return;
    }
    if let Err(e) = write_line(&mut io::stdout().lock(), line) {
        if e.kind() == ErrorKind::BrokenPipe {
            GONE.store(true, Ordering::Relaxed);
            return;
        }
        let _ = writeln!(io::stderr(), "error: cannot write to stdout: {e}");
        std::process::exit(2);
    }
}

/// `line` and a newline written to `out`: the bytes `println!` writes.
fn write_line(out: &mut impl Write, line: impl std::fmt::Display) -> io::Result<()> {
    writeln!(out, "{line}")
}

/// 0, or why not: the tool refused (1), the game halted during the call (3).
fn outcome_code(out: &Value) -> u8 {
    if out.get("ok") == Some(&Value::Bool(false)) {
        exit::TOOL_ERROR
    } else if out.get("halted").is_some_and(|h| !h.is_null()) {
        exit::HALTED
    } else {
        0
    }
}

/// Text for humans and agents: the result, then what changed, then new journal lines.
fn print_outcome(out: &Value, as_json: bool) -> ExitCode {
    let code = ExitCode::from(outcome_code(out));
    if as_json {
        say(serde_json::to_string_pretty(out).unwrap_or_default());
        return code;
    }
    let ok = out.get("ok").and_then(Value::as_bool).unwrap_or(true);
    let result = out.get("result").cloned().unwrap_or(Value::Null);
    let text = match &result {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    if ok {
        say(text.trim_end());
    } else {
        eprintln!("error: {text}");
    }
    let list = |key: &str| -> Vec<String> {
        out.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let changes = list("changes");
    if !changes.is_empty() {
        say("CHANGES");
        for c in changes {
            say(format!("  {c}"));
        }
    }
    let journal = list("journal");
    if !journal.is_empty() {
        say("JOURNAL");
        for j in journal {
            say(format!("  {j}"));
        }
    }
    code
}

const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

/// SplitMix64: tiny and seedable, plenty to pick inputs with.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

/// Ids of the entities an observation lists (not your own, not journal lines).
fn ids_in(observation: &str) -> Vec<String> {
    let is_id = |t: &&str| t.len() > 1 && t.starts_with('e') && t[1..].bytes().all(|b| b.is_ascii_digit());
    observation
        .lines()
        .skip(1) // header: tick, pause, seed, you
        .take_while(|l| !l.starts_with("SINCE"))
        .filter(|l| !l.starts_with("YOU"))
        .flat_map(|l| l.split([' ', '·']))
        .filter(is_id)
        .map(str::to_owned)
        .collect()
}

/// One random player input: a tool and its arguments.
fn random_input(rng: &mut SplitMix, ids: &[String]) -> (&'static str, Value) {
    let dir = |rng: &mut SplitMix| DIRS[rng.below(DIRS.len() as u64) as usize];
    let target = |rng: &mut SplitMix| ids.get(rng.below(ids.len() as u64) as usize).cloned();
    match rng.below(3) {
        0 => ("move", json!({"dir": dir(rng), "meters": 1 + rng.below(8)})),
        1 => match target(rng) {
            Some(id) if rng.below(2) == 0 => ("attack", json!({"target": id, "repeat": rng.below(2) == 0})),
            // `attack` needs a target; a plain swing in the facing direction is `light_attack`.
            _ => ("light_attack", json!({})),
        },
        _ => match target(rng) {
            Some(id) if rng.below(2) == 0 => ("dodge", json!({"away_from": id})),
            _ => ("dodge", json!({"dir": dir(rng)})),
        },
    }
}

/// Random player inputs between short steps until the game halts or `rounds` ran out.
/// A refused input is part of the game: the next round goes on.
fn fuzz(act: &dyn Fn(&str, Value) -> Result<Value, String>, seed: u64, rounds: u32, max_ticks: u64) -> Result<ExitCode, String> {
    let mut rng = SplitMix(seed);
    let mut from = None;
    for round in 1..=rounds {
        let seen = act("observe", json!({}))?;
        from.get_or_insert(seen["tick"].as_u64().unwrap_or(0));
        let ids = ids_in(seen["result"].as_str().unwrap_or_default());
        let (tool, args) = random_input(&mut rng, &ids);
        act(tool, args)?;
        let out = act("step", json!({"ticks": 1 + rng.below(max_ticks)}))?;
        if out.get("ok") == Some(&Value::Bool(false)) {
            return Ok(print_outcome(&out, false));
        }
        if out.get("halted").is_some_and(|h| !h.is_null()) {
            let tick = out["tick"].as_u64().unwrap_or(0);
            say(format!("fuzz seed {seed}: the game halted after {round} inputs (ticks {} → {tick})", from.unwrap_or(0)));
            print_outcome(&out, false);
            let replay = match out["halted"]["replay"].as_str() {
                Some(path) => path.to_owned(),
                None => saved_replay(act, &format!("replays/fuzz-seed{seed}-tick{tick}.json"))?,
            };
            say(format!("replay: {replay}"));
            return Ok(ExitCode::from(exit::HALTED));
        }
    }
    let tick = act("observe", json!({}))?["tick"].as_u64().unwrap_or(0);
    let replay = saved_replay(act, &format!("replays/fuzz-seed{seed}.json"))?;
    say(format!("fuzz seed {seed}: {rounds} inputs, nothing broke (ticks {} → {tick})", from.unwrap_or(0)));
    say(format!("replay: {replay}"));
    Ok(ExitCode::SUCCESS)
}

/// Saves the session as a replay; returns the path the game wrote.
fn saved_replay(act: &dyn Fn(&str, Value) -> Result<Value, String>, path: &str) -> Result<String, String> {
    let out = act("replay_save", json!({"path": path}))?;
    match out["result"].as_str() {
        // "saved PATH: 120 ticks, …"
        Some(text) if out["ok"] == Value::Bool(true) => {
            Ok(text.trim_start_matches("saved ").split_once(": ").map_or(path, |(saved, _)| saved).to_owned())
        }
        _ => Err(format!("replay_save: {}", out["result"])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_tell_what_happened() {
        let step = |extra: Value| {
            let mut out = json!({"tool": "step", "ok": true, "result": "ran 1 ticks (0 → 1)", "tick": 1, "paused": true});
            out.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            out
        };
        assert_eq!(outcome_code(&step(json!({}))), 0);
        assert_eq!(outcome_code(&step(json!({"ok": false, "result": "unknown condition `x`"}))), exit::TOOL_ERROR);
        let halted = json!({"halted": {"reason": "invariant hp violated at tick 1", "replay": "replays/invariant-hp-tick1.json"}});
        assert_eq!(outcome_code(&step(halted)), exit::HALTED);
        assert_eq!(outcome_code(&step(json!({"halted": {"reason": "system x panicked at tick 50"}}))), exit::HALTED);
        assert_eq!(outcome_code(&step(json!({"halted": null}))), 0);
        let codes = [0, exit::TOOL_ERROR, exit::SHARDS_ERROR, exit::HALTED];
        assert_eq!(codes, [0, 1, 2, 3], "README lists these");
    }

    #[test]
    fn fuzz_inputs_come_from_the_seed() {
        let observation = "tick 0 (0.00s) | paused | speed 1x | seed 42 | you e5\n\
                           YOU  e5 Player  hp 100/100\n\
                           SEEN (by threat)\n  e6 Boss \"Frost Warden\"  hp 4000/4000  18.0m N\n  e7 Skeleton  12.6m W\n\
                           LANDMARKS  e3 Pillar 8.6m W · e4 Pillar 8.6m E\n\
                           SINCE seq 0\n  0 e5 -> e9 Sword";
        assert_eq!(ids_in(observation), ["e6", "e7", "e3", "e4"]);
        let run = |seed| {
            let mut rng = SplitMix(seed);
            (0..20).map(|_| random_input(&mut rng, &ids_in(observation))).collect::<Vec<_>>()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        let tools: Vec<&str> = run(7).iter().map(|(t, _)| *t).collect();
        assert!(["move", "attack", "light_attack", "dodge"].iter().all(|t| tools.contains(t)), "{tools:?}");
        // `attack` takes a target (a call without one is refused), with entities seen or not.
        for ids in [ids_in(observation), Vec::new()] {
            let mut rng = SplitMix(3);
            let inputs: Vec<_> = (0..200).map(|_| random_input(&mut rng, &ids)).collect();
            assert!(inputs.iter().filter(|(t, _)| *t == "attack").all(|(_, args)| args["target"].is_string()), "{inputs:?}");
            assert!(inputs.iter().any(|(t, _)| *t == "light_attack"), "a plain swing is still sent");
        }
    }

    #[test]
    fn the_port_comes_from_the_port_file() {
        let dir = std::env::temp_dir().join(format!("shards_port_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("game.port");
        std::fs::write(&file, "15711\n").unwrap();
        assert_eq!(read_port_file(&file), Ok(15711));

        let cli = |args: &[&str]| crate::fake::parse(&[args, &["status"]].concat());
        let path = file.display().to_string();
        assert_eq!(cli(&["--port-file", &path]).port(), Ok(15711));
        assert_eq!(cli(&["--port-file", &path, "--port", "15799"]).port(), Ok(15799), "--port wins");

        std::fs::write(&file, "busy").unwrap();
        assert!(read_port_file(&file).unwrap_err().contains("holds no port number"));
        std::fs::remove_file(&file).unwrap();
        assert!(read_port_file(&file).unwrap_err().contains("is the game running with --port-file?"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_token_is_found_by_port_from_here_upwards() {
        let base = std::env::temp_dir().join(format!("shards_token_{}", std::process::id()));
        let deep = base.join("game/crates/sandbox");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(base.join("game/.shards")).unwrap();
        std::fs::write(base.join("game/.shards/15711.token"), "abc123\n").unwrap();
        assert_eq!(find_token(15711, &deep).as_deref(), Some("abc123"));
        assert_eq!(find_token(15711, &base.join("game")).as_deref(), Some("abc123"));
        assert_eq!(find_token(15712, &deep), None, "another port's game");
        assert_eq!(find_token(15711, &base), None, "not below the game folder");
        let _ = std::fs::remove_dir_all(base);

        assert_eq!(with_token(Value::Null, Some("t")).unwrap(), json!({"token": "t"}));
        assert_eq!(with_token(json!({"data": 1}), Some("t")).unwrap(), json!({"data": 1, "token": "t"}));
        assert_eq!(with_token(json!({"data": 1}), None).unwrap(), json!({"data": 1}));
        assert!(with_token(json!([1]), Some("t")).is_err());
    }

    #[test]
    fn wait_waits_for_loading_to_finish() {
        assert_eq!(still_loading(&json!({"loading": ["content: 2 files left"]})), ["content: 2 files left"]);
        assert!(still_loading(&json!({"loading": []})).is_empty());
        assert!(still_loading(&json!({"tick": 0})).is_empty(), "older games count as loaded");
    }

    #[test]
    fn the_ready_line_names_the_instance() {
        let status = json!({"tick": 0, "seed": 1, "instance": {"pid": 4242, "seed": 1, "port": 15711}});
        assert_eq!(ready_line(&status), "ready: tick 0 · pid 4242 · seed 1 · port 15711");
        let single = json!({"tick": 0, "seed": 1, "instance": {"pid": 4242, "port": 15711}, "net": {"role": "single"}});
        assert_eq!(ready_line(&single), "ready: tick 0 · pid 4242 · seed 1 · port 15711");
        let member = json!({"tick": 9, "seed": 1, "instance": {"pid": 4242, "port": 15712},
            "net": {"role": "client", "member": "c1", "player": 1, "body": "e23"}});
        assert_eq!(ready_line(&member), "ready: tick 9 · pid 4242 · seed 1 · port 15712 · member c1 · role client · player 1 · body e23");
        let host = json!({"tick": 0, "seed": 1, "instance": {"pid": 4242, "port": 15711},
            "net": {"role": "host", "member": "host", "player": 0, "body": "e5", "udp": "0.0.0.0:54321"}});
        assert_eq!(ready_line(&host), "ready: tick 0 · pid 4242 · seed 1 · port 15711 · member host · role host · player 0 · body e5 · udp 0.0.0.0:54321");
    }

    #[test]
    fn a_deferred_step_is_polled_until_it_is_done() {
        use crate::fake::{reply, serve};
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Mutex};

        let asked: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
        let log = asked.clone();
        let polls = AtomicU64::new(0);
        let done = json!({"tool": "step", "ok": true, "result": "ran 120 ticks (0 → 120)", "tick": 120, "paused": true,
            "changes": ["e5 moved"], "journal": ["60 e6 winds up slam"], "halted": null});
        let finished = done.clone();
        let port = serve(move |method, params| {
            log.lock().unwrap().push((method.to_owned(), params.clone()));
            Some(match method {
                "ai/act" if params["tool"] == "step" || params["tool"] == "speed" => {
                    reply(json!({"tool": params["tool"], "ok": true, "result": {"pending": 4}, "tick": 0, "paused": true}))
                }
                "ai/act" => reply(json!({"tool": params["tool"], "ok": true, "result": {"pending": 9}, "tick": 0, "paused": true})),
                "ai/step_poll" if polls.fetch_add(1, Ordering::SeqCst) < 2 => {
                    reply(json!({"tool": "step_poll", "ok": true, "result": {"pending": 4}, "tick": 0, "paused": true}))
                }
                "ai/step_poll" => reply(finished.clone()),
                other => panic!("unexpected {other}"),
            })
        });
        let client = Client::new(port, None).with_timeout(Duration::from_secs(5));
        // `step` answers pending twice, and the report comes from the third poll: the whole outcome, as a local step.
        let out = client.act("dev", "step", json!({"ticks": 120})).unwrap();
        assert_eq!(out, done);
        assert_eq!(outcome_code(&out), 0);
        let methods: Vec<String> = asked.lock().unwrap().iter().map(|(m, _)| m.clone()).collect();
        assert_eq!(methods, ["ai/act", "ai/step_poll", "ai/step_poll", "ai/step_poll"]);
        assert_eq!(asked.lock().unwrap()[1].1, json!({"agent": "dev", "id": 4}), "the poll carries the id and the agent");

        // A tool that is not a clock tool is not polled, whatever it answers.
        asked.lock().unwrap().clear();
        let out = client.act("dev", "mystery", json!({})).unwrap();
        assert_eq!(out["result"], json!({"pending": 9}));
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    /// `pause`, `resume` and `speed` of a stand are deferred like `step`: each is polled by its
    /// own id until its own answer comes.
    #[test]
    fn the_clock_tools_are_polled_by_their_own_ids() {
        use crate::fake::{reply, serve};
        use std::collections::HashMap;
        use std::sync::Mutex;

        let tools: Mutex<HashMap<u64, String>> = Mutex::default();
        let polled: Mutex<HashMap<u64, u32>> = Mutex::default();
        let port = serve(move |method, params| {
            Some(match method {
                "ai/act" => {
                    let mut tools = tools.lock().unwrap();
                    let id = tools.len() as u64 + 1;
                    tools.insert(id, params["tool"].as_str().unwrap().to_owned());
                    reply(json!({"tool": params["tool"], "ok": true, "result": {"pending": id}, "tick": 0, "paused": true}))
                }
                "ai/step_poll" => {
                    let id = params["id"].as_u64().unwrap();
                    let times = *polled.lock().unwrap().entry(id).and_modify(|n| *n += 1).or_insert(1);
                    let tool = tools.lock().unwrap()[&id].clone();
                    let result = if times < 2 { json!({"pending": id}) } else { json!(format!("{tool} done as request {id}")) };
                    reply(json!({"tool": tool, "ok": true, "result": result, "tick": 0, "paused": true}))
                }
                other => panic!("unexpected {other}"),
            })
        });
        let client = Client::new(port, None).with_timeout(Duration::from_secs(5));
        for (id, tool, args) in [(1, "pause", json!({})), (2, "resume", json!({})), (3, "speed", json!({"x": 2}))] {
            let out = client.act("dev", tool, args).unwrap();
            assert_eq!(out["result"], format!("{tool} done as request {id}"));
        }
    }

    #[test]
    fn a_game_that_runs_steps_at_once_is_never_polled() {
        use crate::fake::{reply, serve};
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let port = serve(move |method, _| {
            log.lock().unwrap().push(method.to_owned());
            Some(reply(json!({"tool": "step", "ok": true, "result": "ran 1 ticks (0 → 1)", "tick": 1, "paused": true})))
        });
        let client = Client::new(port, None);
        let out = client.act("dev", "step", json!({})).unwrap();
        assert_eq!(out["result"], "ran 1 ticks (0 → 1)");
        assert_eq!(*seen.lock().unwrap(), ["ai/act"], "no step_poll for a game that answers right away");
    }

    #[test]
    fn a_step_that_takes_too_long_says_how_to_ask_again() {
        use crate::fake::{reply, serve};
        let port = serve(|method, _| {
            Some(reply(json!({"tool": method, "ok": true, "result": {"pending": 7}, "tick": 0, "paused": true})))
        });
        let client = Client::new(port, None).with_timeout(Duration::from_millis(100));
        let started = Instant::now();
        let e = client.act("dev", "step", json!({"ticks": 99999})).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            e,
            "request 7 is still running after 0s; it goes on in the game: ask again with `shards act step_poll id=7`, or wait longer with --timeout"
        );
    }

    #[test]
    fn a_refused_or_unknown_request_is_an_answer_not_a_wait() {
        assert_eq!(pending_id("step", &json!({"ok": true, "result": {"pending": 3}})), Some(3));
        assert_eq!(pending_id("pause", &json!({"ok": true, "result": {"pending": 3}})), Some(3));
        assert_eq!(pending_id("step", &json!({"ok": false, "result": "no request 3"})), None);
        assert_eq!(pending_id("step", &json!({"ok": true, "result": "ran 1 ticks"})), None);
        assert_eq!(pending_id("step", &json!({"ok": true, "result": {"pending": 3, "more": 1}})), None);
        assert_eq!(pending_id("observe", &json!({"ok": true, "result": {"pending": 3}})), None);
    }

    /// The options that `README`, AGENTS.md and the stand workflow say can come from the
    /// environment do read their variable; `fake::parse` switches them off for the other tests.
    #[test]
    fn every_option_has_its_variable() {
        use clap::CommandFactory;
        let command = Cli::command();
        let variable = |id: &str| {
            command.get_arguments().find(|arg| arg.get_id() == id).and_then(|arg| arg.get_env()).map(|v| v.to_string_lossy().into_owned())
        };
        for (id, name) in [
            ("port", "SHARDS_PORT"),
            ("port_file", "SHARDS_PORT_FILE"),
            ("peer", "SHARDS_PEER"),
            ("timeout", "SHARDS_TIMEOUT"),
            ("token", "SHARDS_TOKEN"),
            ("agent", "SHARDS_AGENT"),
        ] {
            assert_eq!(variable(id).as_deref(), Some(name), "--{id}");
        }
        // And the helper that parses without them really does: no variable reaches the Cli.
        let cli = crate::fake::parse(&["status"]);
        assert_eq!((cli.port, cli.peer, cli.timeout, cli.port_file, cli.token), (None, None, None, None, None));
    }

    #[test]
    fn peer_timeout_and_player_are_global_or_per_command() {
        let parse = |args: &[&str]| crate::fake::parse(args);
        assert_eq!(parse(&["--timeout", "5", "wait"]).timeout, Some(5));
        assert_eq!(parse(&["wait", "--timeout", "5"]).timeout, Some(5), "a global option may follow the command");
        assert_eq!(parse(&["shot", "--timeout", "3"]).timeout_or(10), Duration::from_secs(3));
        assert_eq!(parse(&["shot"]).timeout_or(10), Duration::from_secs(10));
        assert_eq!(parse(&["wait"]).poll_timeout(), Duration::from_secs(60));
        assert_eq!(parse(&["--peer", "c1", "session", "--player", "2"]).peer, Some(Peer::Client(1)));
        let both = crate::fake::try_parse(&["session", "--player", "1", "--subject", "e6"]).err().unwrap().to_string();
        assert!(both.contains("cannot be used with"), "{both}");
        assert!(matches!(parse(&["stand"]).cmd, Cmd::Stand { op: None }));
        assert!(matches!(parse(&["stand", "stop"]).cmd, Cmd::Stand { op: Some(StandCmd::Stop) }));
    }

    #[test]
    fn output_lines_are_written_as_println_writes_them() {
        let mut out: Vec<u8> = Vec::new();
        write_line(&mut out, "CHANGES").unwrap();
        write_line(&mut out, "  e5 moved").unwrap();
        assert_eq!(out, b"CHANGES\n  e5 moved\n");
    }

    /// A reader that has gone away: every write fails the way a closed pipe does.
    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn a_closed_pipe_is_a_broken_pipe_error_that_say_ends_quietly() {
        let err = write_line(&mut ClosedPipe, "observe").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::BrokenPipe);
    }
}
