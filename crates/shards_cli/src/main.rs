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
//! shards tools
//! shards raw world.query '{"data":{"components":["sandbox::gameplay::combat::Health"]}}'
//! ```

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use serde_json::{json, Map, Value};

#[derive(Parser)]
#[command(name = "shards", version, about = "Drive a running game through its ai/* BRP methods")]
struct Cli {
    /// Game's BRP port (each parallel instance gets its own).
    #[arg(long, short, global = true, env = "SHARDS_PORT", default_value_t = 15702)]
    port: u16,
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
    /// List the tools available to this agent.
    Tools {
        /// LLM tool definitions ([{name, description, input_schema}]) as JSON.
        #[arg(long)]
        llm: bool,
    },
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

struct Client {
    url: String,
    agent: ureq::Agent,
}

impl Client {
    fn new(port: u16) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build()
            .new_agent();
        Self { url: format!("http://127.0.0.1:{port}"), agent }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
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
            return Err(format!("{method}: {msg}"));
        }
        Ok(reply.get("result").cloned().unwrap_or(Value::Null))
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let client = Client::new(cli.port);
    match run(&cli, &client) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
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
        Cmd::Tools { llm } => {
            let tools = client.call("ai/tools", json!({"agent": agent, "format": if *llm { "llm" } else { "full" }}))?;
            if *llm || cli.json {
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
        Cmd::Wait { timeout } => {
            let start = Instant::now();
            loop {
                match client.call("ai/status", Value::Null) {
                    Ok(s) => {
                        println!("ready: tick {}", s.get("tick").cloned().unwrap_or_default());
                        return Ok(ExitCode::SUCCESS);
                    }
                    Err(_) if start.elapsed() < Duration::from_secs(*timeout) => {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    Err(e) => return Err(e),
                }
            }
        }
    };
    Ok(print_outcome(&outcome, cli.json))
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
/// `field:12:Health.current<30` · `named:boss_phase_2` · raw JSON.
fn parse_condition(s: &str) -> Result<Value, String> {
    if s.trim_start().starts_with('{') {
        return serde_json::from_str(s).map_err(|e| format!("bad condition JSON: {e}"));
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
    }

    #[test]
    fn args_parse() {
        assert_eq!(parse_args(&["target=42".into(), "queue=true".into()]).unwrap(), json!({"target": 42, "queue": true}));
        assert_eq!(parse_args(&["dir=W".into()]).unwrap(), json!({"dir": "W"}));
        assert_eq!(parse_args(&[r#"{"target":{"x":1,"z":2}}"#.into()]).unwrap(), json!({"target": {"x": 1, "z": 2}}));
    }
}
