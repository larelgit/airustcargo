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
//! shards assert add hp 'field:6:Health.current<0'   # halt the game when it breaks
//! shards fuzz --seed 7 --steps 300 # random inputs until a rule breaks
//! shards --port-file F wait        # a game started with `--port 0 --port-file F`
//! shards tools
//! shards raw world.query '{"data":{"components":["sandbox::gameplay::combat::Health"]}}'
//! ```
//!
//! Exit codes: 0 done · 1 the tool refused (the error says why) · 2 `shards` itself
//! failed (no game at the port, no token, a bad command line) · 3 the game halted: a
//! `step` stopped at a failed gameplay system or a violated invariant.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};

/// Bevy's BRP port, used when neither `--port` nor `--port-file` says otherwise.
const DEFAULT_PORT: u16 = 15702;

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
    /// folder upwards (the game writes it into its working directory at start).
    #[arg(long, global = true, env = "SHARDS_TOKEN", hide_env_values = true)]
    token: Option<String>,
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
        /// Seconds to wait for the file.
        #[arg(long, default_value_t = 10)]
        timeout: u64,
    },
    /// Save the session so far as a replay: seed, inputs, action calls, state checks.
    /// Play it with `cargo sandbox --replay FILE` (add `--verify` to check a build).
    SaveReplay {
        /// Default: replays/tick<T>.json next to the game.
        path: Option<String>,
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
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        clear_subject: bool,
    },
    /// Any BRP method (world.query, world.get_components, rpc.discover, ...).
    Raw { method: String, params: Option<String> },
    /// Wait until the game answers (for scripts).
    Wait {
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
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
}

impl Client {
    fn new(port: u16, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { url: format!("http://127.0.0.1:{port}"), agent, port, token }
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
            return Err(format!("{method}: {msg}"));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.cmd {
        // The game may still be starting: its port file may not exist yet.
        Cmd::Wait { timeout } => wait(&cli, Duration::from_secs(*timeout)),
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

impl Cli {
    /// `--port`, else the number in `--port-file`, else the default.
    fn port(&self) -> Result<u16, String> {
        match (self.port, &self.port_file) {
            (Some(port), _) => Ok(port),
            (None, Some(file)) => read_port_file(file),
            (None, None) => Ok(DEFAULT_PORT),
        }
    }

    fn client(&self) -> Result<Client, String> {
        let port = self.port()?;
        let token = self.token.clone().or_else(|| find_token(port, &std::env::current_dir().ok()?));
        Ok(Client::new(port, token))
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

/// Polls `ai/status` until the game answers, then prints who answered.
fn wait(cli: &Cli, timeout: Duration) -> Result<ExitCode, String> {
    let start = Instant::now();
    loop {
        let answer = cli.client().and_then(|client| client.call("ai/status", Value::Null));
        match answer {
            Ok(status) => {
                println!("{}", ready_line(&status));
                return Ok(ExitCode::SUCCESS);
            }
            Err(_) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(e),
        }
    }
}

/// `ready: tick 0 · pid 4242 · seed 1 · port 15711`, the line the game prints at start.
fn ready_line(status: &Value) -> String {
    let field = |v: &Value| match v {
        Value::Null => "?".to_owned(),
        other => other.to_string(),
    };
    let instance = &status["instance"];
    format!(
        "ready: tick {} · pid {} · seed {} · port {}",
        field(&status["tick"]),
        field(&instance["pid"]),
        field(&status["seed"]),
        field(&instance["port"])
    )
}

fn run(cli: &Cli, client: &Client) -> Result<ExitCode, String> {
    let agent = cli.agent.as_str();
    let act = |tool: &str, args: Value| client.call("ai/act", json!({"agent": agent, "tool": tool, "args": args}));
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
                println!();
                print_outcome(&act("observe", json!({}))?, false);
            }
            return Ok(code);
        }
        Cmd::Act { tool, args } => act(tool, parse_args(args)?)?,
        Cmd::SaveReplay { path } => act("replay_save", json!({"path": path}))?,
        Cmd::Assert { op } => match op {
            AssertCmd::Add { name, when } => act("assert_add", json!({"name": name, "when": parse_condition(when)?}))?,
            AssertCmd::List => act("assert_list", json!({}))?,
            AssertCmd::Remove { name } => act("assert_remove", json!({"name": name}))?,
            AssertCmd::Reset => act("assert_reset", json!({}))?,
        },
        Cmd::Fuzz { seed, steps, max_ticks } => return fuzz(&act, *seed, *steps, *max_ticks),
        Cmd::Shot { no_marks, out, timeout } => {
            let reply = act("shot", json!({"marks": !no_marks, "path": out}))?;
            if reply.get("ok") != Some(&Value::Bool(true)) {
                return Ok(print_outcome(&reply, cli.json));
            }
            let ticket = reply.get("result").cloned().unwrap_or_default();
            let path = |k: &str| ticket.get(k).and_then(Value::as_str).map(str::to_owned);
            let (png, legend) = path("path").zip(path("legend_path")).ok_or("shot: unexpected reply")?;
            wait_for_file(&png, Duration::from_secs(*timeout))?;
            // Written before the PNG is renamed into place, so it is there now.
            let info: Value = std::fs::read_to_string(&legend)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .ok_or_else(|| format!("{png} is saved, but its legend {legend} is not readable"))?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
            } else {
                print_shot(&info);
            }
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Tools => {
            let tools = client.call("ai/tools", json!({"agent": agent}))?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&tools).unwrap_or_default());
            } else {
                for t in tools.as_array().into_iter().flatten() {
                    let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("");
                    println!("{}  [{} {}]", s("signature"), s("scope"), s("kind"));
                    for line in s("description").lines() {
                        println!("    {line}");
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
                println!("{}", ready_line(&s));
            }
            println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Session { scope, subject, clear_subject } => {
            let s = client.call(
                "ai/session",
                json!({"agent": agent, "scope": scope, "subject": subject, "clear_subject": clear_subject}),
            )?;
            println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Raw { method, params } => {
            let params = match params {
                Some(p) => serde_json::from_str(p).map_err(|e| format!("params must be JSON: {e}"))?,
                None => Value::Null,
            };
            println!("{}", serde_json::to_string_pretty(&client.call(method, params)?).unwrap_or_default());
            return Ok(ExitCode::SUCCESS);
        }
        Cmd::Wait { timeout } => return wait(cli, Duration::from_secs(*timeout)),
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
    println!("saved {path} ({}x{}, tick {}, {marks})", n("width"), n("height"), n("tick"));
    let legend: Vec<&str> =
        info.get("legend").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    if legend.is_empty() {
        println!("LEGEND  nothing AI-visible in the picture");
    } else {
        println!("LEGEND (image pixels)");
        for line in legend {
            println!("  {line}");
        }
    }
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
        println!("{}", serde_json::to_string_pretty(out).unwrap_or_default());
        return code;
    }
    let ok = out.get("ok").and_then(Value::as_bool).unwrap_or(true);
    let result = out.get("result").cloned().unwrap_or(Value::Null);
    let text = match &result {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    if ok {
        println!("{}", text.trim_end());
    } else {
        eprintln!("error: {text}");
    }
    let list = |key: &str| -> Vec<String> {
        out.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    };
    let changes = list("changes");
    if !changes.is_empty() {
        println!("CHANGES");
        for c in changes {
            println!("  {c}");
        }
    }
    let journal = list("journal");
    if !journal.is_empty() {
        println!("JOURNAL");
        for j in journal {
            println!("  {j}");
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
            _ => ("attack", json!({})),
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
            println!("fuzz seed {seed}: the game halted after {round} inputs (ticks {} → {tick})", from.unwrap_or(0));
            print_outcome(&out, false);
            let replay = match out["halted"]["replay"].as_str() {
                Some(path) => path.to_owned(),
                None => saved_replay(act, &format!("replays/fuzz-seed{seed}-tick{tick}.json"))?,
            };
            println!("replay: {replay}");
            return Ok(ExitCode::from(exit::HALTED));
        }
    }
    let tick = act("observe", json!({}))?["tick"].as_u64().unwrap_or(0);
    let replay = saved_replay(act, &format!("replays/fuzz-seed{seed}.json"))?;
    println!("fuzz seed {seed}: {rounds} inputs, nothing broke (ticks {} → {tick})", from.unwrap_or(0));
    println!("replay: {replay}");
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

/// One JSON object, or `key=value` pairs where values are JSON if they parse as JSON.
fn parse_args(args: &[String]) -> Result<Value, String> {
    if args.is_empty() {
        return Ok(json!({}));
    }
    if args.len() == 1 && args[0].trim_start().starts_with('{') {
        return serde_json::from_str(&args[0]).map_err(|e| format!("arguments are not valid JSON: {e}"));
    }
    let mut map = Map::new();
    for a in args {
        let (k, v) = a.split_once('=').ok_or_else(|| format!("expected key=value, got `{a}`"))?;
        map.insert(k.to_owned(), loose_value(v));
    }
    Ok(Value::Object(map))
}

fn loose_value(v: &str) -> Value {
    serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_owned()))
}

/// `journal:phase` · `gone:57` · `shows:42:windup` · `near:12:42:3` ·
/// `field:12:Health.current<30` · `named:boss_phase_2` · `warning` · `error` · raw JSON.
fn parse_condition(s: &str) -> Result<Value, String> {
    if s.trim_start().starts_with('{') {
        return serde_json::from_str(s).map_err(|e| format!("bad condition JSON: {e}"));
    }
    if s == "warning" || s == "error" {
        return Ok(json!(s));
    }
    let (kind, rest) = s.split_once(':').ok_or_else(|| format!("bad condition `{s}`"))?;
    Ok(match kind {
        "journal" => json!({"journal": rest}),
        "gone" => json!({"gone": loose_value(rest)}),
        "named" => json!({"named": rest}),
        "shows" => {
            let (id, text) = rest.split_once(':').ok_or("shows:ID:TEXT")?;
            json!({"shows": {"id": loose_value(id), "text": text}})
        }
        "near" => {
            let parts: Vec<&str> = rest.split(':').collect();
            let [a, b, m] = parts[..] else { return Err("near:A:B:METERS".into()) };
            let meters: f32 = m.parse().map_err(|_| "near: METERS must be a number")?;
            json!({"near": {"a": loose_value(a), "b": loose_value(b), "meters": meters}})
        }
        "field" => {
            let (id, expr) = rest.split_once(':').ok_or("field:ID:Component.path<N")?;
            let (lhs, op, rhs) = if let Some((l, r)) = expr.split_once('<') {
                (l, "below", r)
            } else if let Some((l, r)) = expr.split_once('>') {
                (l, "above", r)
            } else {
                return Err("field condition needs < or >".into());
            };
            let (component, path) = lhs.split_once('.').ok_or("field: use Component.path")?;
            let n: f64 = rhs.parse().map_err(|_| "field: threshold must be a number")?;
            json!({"field": {"id": loose_value(id), "component": component, "path": path, op: n}})
        }
        _ => return Err(format!("unknown condition kind `{kind}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_parse() {
        assert_eq!(parse_condition("journal:phase").unwrap(), json!({"journal": "phase"}));
        assert_eq!(parse_condition("gone:57").unwrap(), json!({"gone": 57}));
        assert_eq!(parse_condition("shows:e42:windup").unwrap(), json!({"shows": {"id": "e42", "text": "windup"}}));
        assert_eq!(parse_condition("near:12:42:3").unwrap(), json!({"near": {"a": 12, "b": 42, "meters": 3.0}}));
        assert_eq!(
            parse_condition("field:12:Health.current<30").unwrap(),
            json!({"field": {"id": 12, "component": "Health", "path": "current", "below": 30.0}})
        );
        assert_eq!(parse_condition("warning").unwrap(), json!("warning"));
        assert_eq!(parse_condition("error").unwrap(), json!("error"));
        assert!(parse_condition("warnings").is_err());
    }

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
        assert!(["move", "attack", "dodge"].iter().all(|t| tools.contains(t)), "{tools:?}");
    }

    #[test]
    fn the_port_comes_from_the_port_file() {
        let dir = std::env::temp_dir().join(format!("shards_port_file_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("game.port");
        std::fs::write(&file, "15711\n").unwrap();
        assert_eq!(read_port_file(&file), Ok(15711));

        let cli = |args: &[&str]| Cli::try_parse_from([&["shards"], args, &["status"]].concat()).unwrap();
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
    fn the_ready_line_names_the_instance() {
        let status = json!({"tick": 0, "seed": 1, "instance": {"pid": 4242, "seed": 1, "port": 15711}});
        assert_eq!(ready_line(&status), "ready: tick 0 · pid 4242 · seed 1 · port 15711");
    }

    #[test]
    fn args_parse() {
        assert_eq!(parse_args(&["target=42".into(), "queue=true".into()]).unwrap(), json!({"target": 42, "queue": true}));
        assert_eq!(parse_args(&["dir=W".into()]).unwrap(), json!({"dir": "W"}));
        assert_eq!(parse_args(&[r#"{"target":{"x":1,"z":2}}"#.into()]).unwrap(), json!({"target": {"x": 1, "z": 2}}));
    }
}
