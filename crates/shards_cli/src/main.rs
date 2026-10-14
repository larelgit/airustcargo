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
//! shards --port-file F wait        # a game started with `--port 0 --port-file F`; waits for loading
//! shards content check             # every content file parsed and validated
//! shards worldgen --region meadow --seeds 1..1000 --report   # batch check of the generator
//! shards scenario a.scenario b.scenario   # commands, expectations, invariants: exit 0 / 1 failed / 2 error
//! shards tools
//! shards raw world.query '{"data":{"components":["sandbox::gameplay::combat::Health"]}}'
//! ```

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};
use shards_script::{parse_args, parse_condition};

mod scenario;

/// Bevy's BRP port, used when neither `--port` nor `--port-file` says otherwise.
const DEFAULT_PORT: u16 = 15702;

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
    /// near:A:B:METERS, field:ID:Component.path<N (or >N), named:NAME, or raw JSON.
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
            ExitCode::from(2)
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
                println!("{}", ready_line(&status));
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
                println!();
                print_outcome(&act("observe", json!({}))?, false);
            }
            return Ok(code);
        }
        Cmd::Act { tool, args } => act(tool, parse_args(args)?)?,
        Cmd::SaveReplay { path } => act("replay_save", json!({"path": path}))?,
        Cmd::SaveSnapshot { path } => act("snapshot_save", json!({"path": path}))?,
        Cmd::LoadSnapshot { path, force } => act("snapshot_load", json!({"path": path, "force": force}))?,
        Cmd::Scenario { files, quiet } => return scenario::run(client, files, *quiet, cli.json),
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

/// Text for humans and agents: the result, then what changed, then new journal lines.
fn print_outcome(out: &Value, as_json: bool) -> ExitCode {
    if as_json {
        println!("{}", serde_json::to_string_pretty(out).unwrap_or_default());
        return if out.get("ok") == Some(&Value::Bool(false)) { ExitCode::FAILURE } else { ExitCode::SUCCESS };
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
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
