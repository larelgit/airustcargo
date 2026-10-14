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

mod scenario;

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
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        clear_subject: bool,
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
    /// Wait until the game answers and has finished loading (for scripts).
    Wait {
        #[arg(long, default_value_t = 60)]
        timeout: u64,
    },
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

/// Polls `ai/status` until the game answers and has finished loading, then prints who
/// answered.
fn wait(cli: &Cli, timeout: Duration) -> Result<ExitCode, String> {
    let start = Instant::now();
    loop {
        let answer = cli.client().and_then(|client| client.call("ai/status", Value::Null));
        match answer {
            Ok(status) if still_loading(&status).is_empty() => {
                say(ready_line(&status));
                return Ok(ExitCode::SUCCESS);
            }
            Ok(status) if start.elapsed() >= timeout => {
                return Err(format!("the game answers but is still loading: {}", still_loading(&status).join(", ")));
            }
            Ok(_) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(e),
        }
    }
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
                say("");
                print_outcome(&act("observe", json!({}))?, false);
            }
            return Ok(code);
        }
        Cmd::Act { tool, args } => act(tool, parse_args(args)?)?,
        Cmd::SaveReplay { path } => act("replay_save", json!({"path": path}))?,
        Cmd::SaveSnapshot { path } => act("snapshot_save", json!({"path": path}))?,
        Cmd::LoadSnapshot { path, force } => act("snapshot_load", json!({"path": path, "force": force}))?,
        Cmd::Scenario { files, quiet } => return scenario::run(client, files, *quiet, cli.json),
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
        Cmd::Session { scope, subject, clear_subject } => {
            let s = client.call(
                "ai/session",
                json!({"agent": agent, "scope": scope, "subject": subject, "clear_subject": clear_subject}),
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
    fn wait_waits_for_loading_to_finish() {
        assert_eq!(still_loading(&json!({"loading": ["content: 2 files left"]})), ["content: 2 files left"]);
        assert!(still_loading(&json!({"loading": []})).is_empty());
        assert!(still_loading(&json!({"tick": 0})).is_empty(), "older games count as loaded");
    }

    #[test]
    fn the_ready_line_names_the_instance() {
        let status = json!({"tick": 0, "seed": 1, "instance": {"pid": 4242, "seed": 1, "port": 15711}});
        assert_eq!(ready_line(&status), "ready: tick 0 · pid 4242 · seed 1 · port 15711");
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
