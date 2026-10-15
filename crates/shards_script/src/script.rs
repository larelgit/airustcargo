//! The scenario file: its commands and how a line becomes one.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::args::{loose_value, parse_args};
use crate::condition::parse_condition;
use crate::lex::split_words;

/// What the "unknown command" error offers.
const COMMANDS: &str = "act, act?, step, observe, session, expect, invariant, mark, seed, stand";

/// The member a command runs on when no `@peer` says otherwise: the host of a stand, or the
/// only game of a single-game scenario.
pub const HOST: &str = "host";

/// The most clients a stand has (a host and two clients is the co-op session's size).
pub const MAX_STAND_CLIENTS: u8 = 2;

/// What the "unknown expectation" error offers.
const EXPECTATIONS: &str = "result, stopped, journal, !journal, field, shows";

/// A parsed scenario file.
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    /// Shown in reports and parse errors: the file name.
    pub name: String,
    pub lines: Vec<ScriptLine>,
}

/// One command of a scenario, with where it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptLine {
    /// 1-based line number in the file.
    pub number: usize,
    /// The line as written (comment cut off).
    pub text: String,
    /// The member the command names with an `@host`/`@c1`/`@c2` prefix (stand scenarios).
    pub peer: Option<String>,
    pub command: Command,
}

/// The `stand` line of a scenario: a host and `clients` clients on links with these
/// conditions (one way). `latency=150` means a 300 ms round trip, as `--net-sim` does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StandHeader {
    pub clients: u8,
    pub latency_ms: u32,
    pub jitter_ms: u32,
    /// Probability that a packet is dropped, 0..=1.
    pub loss: f32,
    /// Probability that a delivered packet is delivered twice, 0..=1.
    pub dup: f32,
}

impl StandHeader {
    /// The members, in the order a stand ticks them: `host`, `c1`, `c2`.
    pub fn members(&self) -> Vec<String> {
        std::iter::once(HOST.to_owned()).chain((1..=self.clients).map(|n| format!("c{n}"))).collect()
    }

    /// `host + 2 clients`.
    pub fn describe(&self) -> String {
        format!("host + {} client{}", self.clients, if self.clients == 1 { "" } else { "s" })
    }

    /// Whether the links have any conditions.
    pub fn has_conditions(&self) -> bool {
        self.latency_ms > 0 || self.jitter_ms > 0 || self.loss > 0.0 || self.dup > 0.0
    }

    /// The `--net-sim` value that makes a stand match: `latency=150,loss=0.02`; empty when the
    /// links have no conditions.
    pub fn net_sim(&self) -> String {
        let mut parts = Vec::new();
        if self.latency_ms > 0 {
            parts.push(format!("latency={}", self.latency_ms));
        }
        if self.jitter_ms > 0 {
            parts.push(format!("jitter={}", self.jitter_ms));
        }
        if self.loss > 0.0 {
            parts.push(format!("loss={}", self.loss));
        }
        if self.dup > 0.0 {
            parts.push(format!("dup={}", self.dup));
        }
        parts.join(",")
    }

    /// Checks the conditions a running stand reports (`net_status.conditions`, in the form
    /// `latency=150,jitter=0,loss=0.02,dup=0`; none at all counts as no conditions) against
    /// the header. The error names the `--net-sim` to start the stand with.
    pub fn check_conditions(&self, reported: Option<&str>) -> Result<(), String> {
        let (mut latency, mut jitter, mut loss, mut dup) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let text = reported.unwrap_or("").trim();
        if !matches!(text, "" | "none" | "off") {
            for part in text.split(',') {
                let unreadable = || format!("cannot read the stand's link conditions `{text}`");
                let (key, value) = part.split_once('=').ok_or_else(unreadable)?;
                let number: f64 = value.trim().parse().map_err(|_| unreadable())?;
                match key.trim() {
                    "latency" => latency = number,
                    "jitter" => jitter = number,
                    "loss" => loss = number,
                    "dup" => dup = number,
                    _ => {}
                }
            }
        }
        let same = (latency - f64::from(self.latency_ms)).abs() < 0.5
            && (jitter - f64::from(self.jitter_ms)).abs() < 0.5
            && (loss - f64::from(self.loss)).abs() < 1e-4
            && (dup - f64::from(self.dup)).abs() < 1e-4;
        if same {
            return Ok(());
        }
        let wanted = if self.has_conditions() { self.net_sim() } else { "no conditions".to_owned() };
        let running = if text.is_empty() { "none".to_owned() } else { text.to_owned() };
        let fix = if self.has_conditions() {
            format!("start it with `--net-sim {}`", self.net_sim())
        } else {
            "start it without --net-sim".to_owned()
        };
        Err(format!(
            "this scenario's `stand` line wants links with {wanted}, but the running stand has {running}: {fix} (or change the stand line)"
        ))
    }
}

/// The header as it is written in a file.
impl fmt::Display for StandHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stand clients={}", self.clients)?;
        if self.latency_ms > 0 {
            write!(f, " latency={}", self.latency_ms)?;
        }
        if self.jitter_ms > 0 {
            write!(f, " jitter={}", self.jitter_ms)?;
        }
        if self.loss > 0.0 {
            write!(f, " loss={}", self.loss)?;
        }
        if self.dup > 0.0 {
            write!(f, " dup={}", self.dup)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The scenario is written for this seed (checked against the game's `status`).
    Seed(u64),
    /// The scenario needs a stand: a host and clients, on links with these conditions.
    Stand(StandHeader),
    /// Call a tool. `may_fail` (`act?`): a failing call does not fail the scenario.
    Act { tool: String, args: Value, may_fail: bool },
    /// Advance the simulation. `ticks: None` leaves the default to the `step` tool.
    Step { ticks: Option<u64>, until: Vec<Condition> },
    Observe,
    /// Journal expectations count from here.
    Mark,
    /// Configure an agent and (with `agent`) switch to it for the following commands.
    Session { agent: Option<String>, config: SessionConfig },
    Expect(Expect),
    /// An expectation that must hold after every following `act` and `step`.
    Invariant(Expect),
}

/// A `--until` condition: the text as written and the JSON the `step` tool takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    pub text: String,
    pub json: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentScope {
    Player,
    Dev,
}

impl AgentScope {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentScope::Player => "player",
            AgentScope::Dev => "dev",
        }
    }
}

/// What `session` changes about an agent; `None`/`false` leave things as they are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionConfig {
    pub scope: Option<AgentScope>,
    /// The entity the agent acts as: `e7` or `7`.
    pub subject: Option<String>,
    /// Back to the player character.
    pub clear_subject: bool,
    /// Play as this player's body (`PlayerId`), found again on every call, so the binding
    /// survives a respawn (`ai/session player=N`).
    pub player: Option<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    fn parse(s: &str) -> Result<Cmp, String> {
        Ok(match s {
            "<" => Cmp::Lt,
            "<=" => Cmp::Le,
            ">" => Cmp::Gt,
            ">=" => Cmp::Ge,
            "==" => Cmp::Eq,
            "!=" => Cmp::Ne,
            _ => return Err(format!("unknown comparison `{s}`; use one of < <= > >= == !=")),
        })
    }

    pub fn holds(self, left: f64, right: f64) -> bool {
        match self {
            Cmp::Lt => left < right,
            Cmp::Le => left <= right,
            Cmp::Gt => left > right,
            Cmp::Ge => left >= right,
            Cmp::Eq => left == right,
            Cmp::Ne => left != right,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
            Cmp::Eq => "==",
            Cmp::Ne => "!=",
        }
    }
}

/// Something to check; used by `expect` and `invariant`.
#[derive(Clone, Debug, PartialEq)]
pub enum Expect {
    /// The last result text (`contains` is case-sensitive).
    Result { text: String, negate: bool },
    /// The last `step` ended on a condition whose description contains the text.
    Stopped(String),
    /// A journal entry since the scenario started (or the last `mark`) contains the text.
    Journal { text: String, negate: bool },
    /// A numeric component field, read with the `get` tool.
    Field { id: String, component: String, path: Option<String>, op: Cmp, value: f64 },
    /// The entity's description (what `inspect` shows above its components) contains the text.
    Shows { id: String, text: String },
}

/// The expectation in scenario syntax: what `expect` and `invariant` take.
impl fmt::Display for Expect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expect::Result { text, negate } => {
                write!(f, "result {}contains {}", if *negate { "!" } else { "" }, quote(text))
            }
            Expect::Stopped(text) => write!(f, "stopped {}", quote(text)),
            Expect::Journal { text, negate } => write!(f, "{}journal {}", if *negate { "!" } else { "" }, quote(text)),
            Expect::Field { id, component, path, op, value } => {
                write!(f, "field {id} {component}")?;
                if let Some(path) = path {
                    write!(f, ".{path}")?;
                }
                write!(f, " {} {value}", op.symbol())
            }
            Expect::Shows { id, text } => write!(f, "shows {id} {}", quote(text)),
        }
    }
}

/// `"text"` the way the scenario lexer reads it back.
fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A line that is not a valid command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub name: String,
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.name, self.line, self.message)
    }
}

/// Every bad line of a file, so one run shows all the typos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseErrors(pub Vec<ParseError>);

impl fmt::Display for ParseErrors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lines: Vec<String> = self.0.iter().map(ToString::to_string).collect();
        f.write_str(&lines.join("\n"))
    }
}

impl std::error::Error for ParseErrors {}

impl Script {
    /// Parses a scenario. `name` is the file name, for error messages and reports.
    pub fn parse(name: &str, text: &str) -> Result<Script, ParseErrors> {
        let mut lines: Vec<ScriptLine> = Vec::new();
        let mut errors = Vec::new();
        let mut stand: Option<StandHeader> = None;
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for (index, raw) in text.lines().enumerate() {
            let number = index + 1;
            let fail = |message: String| ParseError { name: name.to_owned(), line: number, message };
            let split = match split_words(raw) {
                Ok(split) => split,
                Err(message) => {
                    errors.push(fail(message));
                    continue;
                }
            };
            let Some((first_word, rest)) = split.words.split_first() else {
                continue;
            };
            // `@c1 act …`: the member the command runs on.
            let (peer, head, args) = match first_word.strip_prefix('@') {
                Some(peer) => match rest.split_first() {
                    Some((head, args)) => (Some(peer), head, args),
                    None => {
                        errors.push(fail(format!("`@{peer}` needs a command after it, e.g. `@{peer} act move_to target=e6`")));
                        continue;
                    }
                },
                None => (None, first_word, rest),
            };
            if let Some(peer) = peer
                && let Err(message) = check_peer(peer, stand.as_ref())
            {
                errors.push(fail(message));
                continue;
            }
            // `seed` and `stand` head the file, in either order, each once.
            let ctx = Ctx {
                header_open: lines.iter().all(|l| matches!(l.command, Command::Seed(_) | Command::Stand(_))),
                seed_seen: lines.iter().any(|l| matches!(l.command, Command::Seed(_))),
                stand_seen: lines.iter().any(|l| matches!(l.command, Command::Stand(_))),
                has_peer: peer.is_some(),
            };
            match parse_command(head, args, &ctx) {
                Ok(command) => {
                    if let Command::Stand(header) = &command {
                        stand = Some(*header);
                    }
                    lines.push(ScriptLine { number, text: split.code, peer: peer.map(str::to_owned), command });
                }
                Err(message) => errors.push(fail(message)),
            }
        }
        if errors.is_empty() { Ok(Script { name: name.to_owned(), lines }) } else { Err(ParseErrors(errors)) }
    }

    /// The `stand` line, when the scenario has one.
    pub fn stand(&self) -> Option<&StandHeader> {
        self.lines.iter().find_map(|l| match &l.command {
            Command::Stand(header) => Some(header),
            _ => None,
        })
    }

    /// The `seed` line, when the scenario has one.
    pub fn seed(&self) -> Option<u64> {
        self.lines.iter().find_map(|l| match l.command {
            Command::Seed(seed) => Some(seed),
            _ => None,
        })
    }
}

/// What the lines before a command say about where it may stand.
struct Ctx {
    /// Only `seed` and `stand` came before.
    header_open: bool,
    seed_seen: bool,
    stand_seen: bool,
    /// The line starts with `@peer`.
    has_peer: bool,
}

/// `@host`, `@c1`, `@c2`: a member of this scenario's stand.
fn check_peer(peer: &str, stand: Option<&StandHeader>) -> Result<(), String> {
    let Some(stand) = stand else {
        return Err(format!(
            "`@{peer}`: this scenario has no stand; start the file with `stand clients=N`, then `@host`, `@c1` … name the member"
        ));
    };
    if peer == HOST {
        return Ok(());
    }
    let clients = (1..=stand.clients).map(|n| format!("c{n}")).collect::<Vec<_>>().join(", ");
    let noun = if stand.clients == 1 { "client" } else { "clients" };
    let client = peer.strip_prefix('c').and_then(|n| n.parse::<u8>().ok()).filter(|n| format!("c{n}") == peer);
    match client {
        Some(n) if (1..=stand.clients).contains(&n) => Ok(()),
        Some(_) => Err(format!("@{peer}: this stand has {noun} {clients}")),
        None => Err(format!("@{peer}: a member is `host` or one of the {noun} {clients}")),
    }
}

fn parse_command(head: &str, args: &[String], ctx: &Ctx) -> Result<Command, String> {
    if ctx.has_peer && matches!(head, "seed" | "stand") {
        return Err(format!("`{head}` is about the whole scenario, not one member; drop the @peer"));
    }
    match head {
        "stand" => {
            if ctx.stand_seen {
                return Err("a scenario has one `stand` line".into());
            }
            if !ctx.header_open {
                return Err("`stand` must come before every other command (only `seed` may come first)".into());
            }
            parse_stand(args).map(Command::Stand)
        }
        "seed" => {
            if ctx.seed_seen || !ctx.header_open {
                return Err("`seed` must come before every other command".into());
            }
            let [n] = args else { return Err("seed needs one number, e.g. `seed 42`".into()) };
            let seed = match n.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16),
                None => n.parse(),
            };
            seed.map(Command::Seed).map_err(|_| format!("seed needs a whole number, got `{n}`"))
        }
        "act" | "act?" => {
            let Some((tool, rest)) = args.split_first() else {
                return Err(format!("{head} needs a tool name, e.g. `{head} attack target=e6`"));
            };
            let args = parse_args(rest).map_err(|e| format!("{head} {tool}: {e}"))?;
            Ok(Command::Act { tool: tool.clone(), args, may_fail: head == "act?" })
        }
        "step" => parse_step(args),
        "observe" | "mark" => {
            if let Some(extra) = args.first() {
                return Err(format!("{head} takes no arguments, got `{extra}`"));
            }
            Ok(if head == "observe" { Command::Observe } else { Command::Mark })
        }
        "session" => parse_session(args),
        "expect" => parse_expect(args).map(Command::Expect),
        "invariant" => {
            // `invariant field …` and `invariant expect field …` both read naturally.
            let args = match args.split_first() {
                Some((word, rest)) if word == "expect" => rest,
                _ => args,
            };
            parse_expect(args).map(Command::Invariant)
        }
        other => Err(format!("unknown command `{other}`; expected {COMMANDS}")),
    }
}

fn parse_stand(args: &[String]) -> Result<StandHeader, String> {
    const USAGE: &str = "stand clients=N [latency=MS] [jitter=MS] [loss=P] [dup=P], e.g. `stand clients=2 latency=150 loss=0.02`";
    let mut header = StandHeader { clients: 0, latency_ms: 0, jitter_ms: 0, loss: 0.0, dup: 0.0 };
    let mut seen: Vec<&str> = Vec::new();
    for word in args {
        let Some((key, value)) = word.split_once('=') else {
            return Err(format!("stand: expected key=value, got `{word}`; use {USAGE}"));
        };
        let key = match key {
            "duplicate" => "dup",
            "latency_ms" => "latency",
            "jitter_ms" => "jitter",
            other => other,
        };
        if seen.contains(&key) {
            return Err(format!("stand: `{key}` is given twice"));
        }
        let ms = |value: &str| {
            value
                .strip_suffix("ms")
                .unwrap_or(value)
                .trim()
                .parse::<u32>()
                .map_err(|_| format!("stand: {key} is whole milliseconds, e.g. {key}=150, got `{value}`"))
        };
        let probability = |value: &str| {
            let (number, scale) = match value.strip_suffix('%') {
                Some(n) => (n, 0.01),
                None => (value, 1.0),
            };
            number
                .trim()
                .parse::<f32>()
                .ok()
                .map(|p| p * scale)
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .ok_or_else(|| format!("stand: {key} is a probability from 0 to 1 (or a percentage), e.g. {key}=0.02, got `{value}`"))
        };
        match key {
            "clients" => {
                header.clients = value
                    .parse::<u8>()
                    .ok()
                    .filter(|n| (1..=MAX_STAND_CLIENTS).contains(n))
                    .ok_or_else(|| format!("stand: clients must be 1 or {MAX_STAND_CLIENTS} (a host and one or two clients), got `{value}`"))?;
            }
            "latency" => header.latency_ms = ms(value)?,
            "jitter" => header.jitter_ms = ms(value)?,
            "loss" => header.loss = probability(value)?,
            "dup" => header.dup = probability(value)?,
            other => return Err(format!("stand: unknown key `{other}`; use clients, latency, jitter, loss, dup")),
        }
        seen.push(key);
    }
    if !seen.contains(&"clients") {
        return Err(format!("stand needs clients=N; use {USAGE}"));
    }
    Ok(header)
}

fn parse_step(args: &[String]) -> Result<Command, String> {
    let mut ticks = None;
    let mut until = Vec::new();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let condition = match word.as_str() {
            "--until" | "-u" => {
                Some(words.next().ok_or("`--until` needs a condition, e.g. --until journal:died")?.as_str())
            }
            _ => word.strip_prefix("--until="),
        };
        if let Some(text) = condition {
            let json = parse_condition(text).map_err(|e| format!("bad --until `{text}`: {e}"))?;
            until.push(Condition { text: text.to_owned(), json });
        } else if word.starts_with('-') && word.parse::<f64>().is_err() {
            return Err(format!("unknown option `{word}` for step; use step [N] [--until CONDITION]..."));
        } else if ticks.is_some() {
            return Err(format!("step takes one tick count; `{word}` is a second one (conditions go after --until)"));
        } else {
            ticks = Some(word.parse::<u64>().map_err(|_| format!("step needs a whole number of ticks, got `{word}`"))?);
        }
    }
    Ok(Command::Step { ticks, until })
}

fn parse_session(args: &[String]) -> Result<Command, String> {
    const USAGE: &str = "session [--agent NAME] [--scope player|dev] [--subject ID | --player N] [--clear-subject]";
    let mut agent = None;
    let mut config = SessionConfig::default();
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let (flag, inline) = match word.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (word.as_str(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline.clone().or_else(|| words.next().cloned()).ok_or_else(|| format!("`{flag}` needs {what}; use {USAGE}"))
        };
        match flag {
            "--agent" => agent = Some(value("a name")?),
            "--scope" => {
                config.scope = Some(match value("player or dev")?.to_lowercase().as_str() {
                    "player" => AgentScope::Player,
                    "dev" => AgentScope::Dev,
                    other => return Err(format!("unknown scope `{other}`; use player or dev")),
                });
            }
            "--subject" => {
                let id = value("an entity id")?;
                check_id(&id)?;
                config.subject = Some(id);
            }
            "--player" => {
                let n = value("a player number")?;
                config.player = Some(n.parse().map_err(|_| format!("--player needs a player number such as 0 or 1, got `{n}`"))?);
            }
            "--clear-subject" => config.clear_subject = true,
            other => return Err(format!("unknown option `{other}` for session; use {USAGE}")),
        }
    }
    if config.subject.is_some() && config.clear_subject {
        return Err("--subject and --clear-subject contradict each other".into());
    }
    if config.player.is_some() && (config.subject.is_some() || config.clear_subject) {
        return Err("--player binds the agent to a player's body; it contradicts --subject and --clear-subject".into());
    }
    if agent.is_none() && config == SessionConfig::default() {
        return Err(format!("session changes nothing; use {USAGE}"));
    }
    Ok(Command::Session { agent, config })
}

fn parse_expect(args: &[String]) -> Result<Expect, String> {
    let Some((head, rest)) = args.split_first() else {
        return Err(format!("expect what? Use one of: {EXPECTATIONS}"));
    };
    Ok(match head.as_str() {
        "result" => {
            let (negate, text) = match rest.split_first() {
                Some((word, text)) if word == "contains" => (false, text),
                Some((word, text)) if word == "!contains" => (true, text),
                _ => return Err("expect result needs `contains \"TEXT\"` or `!contains \"TEXT\"`".into()),
            };
            Expect::Result { text: one_text("expect result contains", text)?, negate }
        }
        "stopped" => Expect::Stopped(one_text("expect stopped", rest)?),
        "journal" | "!journal" => {
            let form = if head == "journal" { "expect journal" } else { "expect !journal" };
            Expect::Journal { text: one_text(form, rest)?, negate: head.starts_with('!') }
        }
        "field" => {
            const USAGE: &str = "expect field needs ID Component.path OP NUMBER, e.g. expect field e5 Health.current > 0";
            let [id, target, op, value] = rest else { return Err(USAGE.into()) };
            check_id(id)?;
            let (component, path) = match target.split_once('.') {
                Some((c, p)) if !c.is_empty() && !p.is_empty() => (c.to_owned(), Some(p.to_owned())),
                None if !target.is_empty() => (target.clone(), None),
                _ => return Err(format!("`{target}` is not Component.path; {USAGE}")),
            };
            let value = value
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("expect field compares with a number, got `{value}`"))?;
            Expect::Field { id: id.clone(), component, path, op: Cmp::parse(op)?, value }
        }
        "shows" => {
            let [id, text] = rest else {
                return Err("expect shows needs ID \"TEXT\", e.g. expect shows e6 \"phase 2\"".into());
            };
            check_id(id)?;
            Expect::Shows { id: id.clone(), text: text.clone() }
        }
        other => return Err(format!("unknown expectation `{other}`; expected {EXPECTATIONS}")),
    })
}

/// The one text argument of an expectation, with a hint when it was not quoted.
fn one_text(form: &str, words: &[String]) -> Result<String, String> {
    match words {
        [text] => Ok(text.clone()),
        [] => Err(format!("{form} needs the text to look for, e.g. {form} \"phase 1->2\"")),
        _ => Err(format!("{form} takes one text; quote it if it has spaces: {}", quote(&words.join(" ")))),
    }
}

/// `e6` or `6`, the shapes the tools take.
fn check_id(id: &str) -> Result<(), String> {
    let digits = id.strip_prefix(['e', 'E']).unwrap_or(id);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && digits.bytes().any(|b| b != b'0') {
        Ok(())
    } else {
        Err(format!("`{id}` is not an entity id; use e6 or 6"))
    }
}

/// The id as the tools take it: a number, or `"e6"`.
pub fn id_value(id: &str) -> Value {
    loose_value(id)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse(text: &str) -> Script {
        Script::parse("t.scenario", text).unwrap_or_else(|e| panic!("{e}"))
    }

    fn only(text: &str) -> Command {
        let mut script = parse(text);
        assert_eq!(script.lines.len(), 1, "{script:?}");
        script.lines.remove(0).command
    }

    fn error(text: &str) -> String {
        Script::parse("t.scenario", text).unwrap_err().to_string()
    }

    #[test]
    fn a_whole_file_parses() {
        let script = parse(
            r#"
# The boss, quickly.
seed 42
act attack target=e6 repeat=true   # walks into range
act? teleport id=e5
step 120
step --until named:boss_phase_2 -u "journal:phase 1->2"
observe
mark
session --agent bot --scope player --subject e7
expect result contains "ok: attacking"
expect stopped "boss_phase_2"
expect !journal "died"
expect field e5 Health.current >= 1
expect shows e6 "phase 2"
invariant field e5 Health.current > 0
"#,
        );
        let kinds: Vec<&str> = script
            .lines
            .iter()
            .map(|l| match &l.command {
                Command::Seed(_) => "seed",
                Command::Stand(_) => "stand",
                Command::Act { may_fail: false, .. } => "act",
                Command::Act { .. } => "act?",
                Command::Step { .. } => "step",
                Command::Observe => "observe",
                Command::Mark => "mark",
                Command::Session { .. } => "session",
                Command::Expect(_) => "expect",
                Command::Invariant(_) => "invariant",
            })
            .collect();
        assert_eq!(
            kinds,
            ["seed", "act", "act?", "step", "step", "observe", "mark", "session", "expect", "expect", "expect", "expect", "expect", "invariant"]
        );
        let numbers: Vec<usize> = script.lines.iter().map(|l| l.number).collect();
        assert_eq!(numbers[..4], [3, 4, 5, 6], "line numbers count blank and comment lines");
        assert_eq!(script.lines[1].text, "act attack target=e6 repeat=true", "the comment is cut off");
    }

    #[test]
    fn act_takes_pairs_or_json() {
        assert_eq!(
            only("act attack target=e6 repeat=true"),
            Command::Act { tool: "attack".into(), args: json!({"target": "e6", "repeat": true}), may_fail: false }
        );
        assert_eq!(
            only(r#"act? set '{"id":"e6","component":"Health","path":"current","value":5}'"#),
            Command::Act {
                tool: "set".into(),
                args: json!({"id": "e6", "component": "Health", "path": "current", "value": 5}),
                may_fail: true
            }
        );
        assert_eq!(only("act stop"), Command::Act { tool: "stop".into(), args: json!({}), may_fail: false });
        assert_eq!(only(r#"act say text="a b""#), Command::Act { tool: "say".into(), args: json!({"text": "a b"}), may_fail: false });
    }

    #[test]
    fn step_takes_ticks_and_conditions() {
        let Command::Step { ticks, until } = only("step 600 --until near:e5:e12:1.5 --until=gone:e7") else { panic!() };
        assert_eq!(ticks, Some(600));
        assert_eq!(until.len(), 2);
        assert_eq!(until[0].text, "near:e5:e12:1.5");
        assert_eq!(until[0].json, json!({"near": {"a": "e5", "b": "e12", "meters": 1.5}}));
        assert_eq!(until[1].json, json!({"gone": "e7"}));
        assert_eq!(only("step"), Command::Step { ticks: None, until: vec![] });
        let Command::Step { ticks, .. } = only("step --until journal:x 30") else { panic!() };
        assert_eq!(ticks, Some(30), "the count may come after the conditions");
    }

    #[test]
    fn session_flags() {
        assert_eq!(
            only("session --agent bot --scope=Player --subject e7"),
            Command::Session {
                agent: Some("bot".into()),
                config: SessionConfig {
                    scope: Some(AgentScope::Player),
                    subject: Some("e7".into()),
                    ..Default::default()
                }
            }
        );
        assert_eq!(
            only("session --player 1"),
            Command::Session { agent: None, config: SessionConfig { player: Some(1), ..Default::default() } }
        );
        assert_eq!(
            only("session --agent bot --player=2 --scope dev"),
            Command::Session {
                agent: Some("bot".into()),
                config: SessionConfig { scope: Some(AgentScope::Dev), player: Some(2), ..Default::default() }
            }
        );
        assert_eq!(
            only("session --clear-subject"),
            Command::Session { agent: None, config: SessionConfig { clear_subject: true, ..Default::default() } }
        );
    }

    #[test]
    fn expectations_print_back_as_they_parse() {
        for form in [
            r#"result contains "ok: attacking""#,
            r#"result !contains "Expected:""#,
            r#"stopped "condition boss_phase_2""#,
            r#"journal "e6 phase 1->2""#,
            r#"!journal "died""#,
            "field e5 Health.current > 0",
            "field e6 Transform.translation.x <= -1.5",
            "field e6 Boss.phase == 2",
            "field e6 Boss.phase != 1",
            r#"shows e6 "phase 2""#,
            r#"result contains "say \"hi\" \\ there""#,
        ] {
            let Command::Expect(expect) = only(&format!("expect {form}")) else { panic!("{form}") };
            assert_eq!(expect.to_string(), form);
            let Command::Invariant(again) = only(&format!("invariant {expect}")) else { panic!("{form}") };
            assert_eq!(again, expect);
        }
        assert_eq!(only("invariant expect !journal \"x\""), only("invariant !journal \"x\""));
    }

    #[test]
    fn errors_name_the_line_and_the_fix() {
        let e = error("act attack\nstpe 5");
        assert_eq!(
            e,
            "t.scenario:2: unknown command `stpe`; expected act, act?, step, observe, session, expect, invariant, mark, seed, stand"
        );
        assert_eq!(error("step 5 6"), "t.scenario:1: step takes one tick count; `6` is a second one (conditions go after --until)");
        assert_eq!(error("step --until"), "t.scenario:1: `--until` needs a condition, e.g. --until journal:died");
        assert_eq!(error("step --until nope"), "t.scenario:1: bad --until `nope`: bad condition `nope`");
        assert_eq!(error("step soon"), "t.scenario:1: step needs a whole number of ticks, got `soon`");
        assert_eq!(error("step --fast"), "t.scenario:1: unknown option `--fast` for step; use step [N] [--until CONDITION]...");
        assert_eq!(error("act"), "t.scenario:1: act needs a tool name, e.g. `act attack target=e6`");
        assert_eq!(error("act? dodge dir"), "t.scenario:1: act? dodge: expected key=value, got `dir`");
        assert_eq!(error("observe now"), "t.scenario:1: observe takes no arguments, got `now`");
        assert_eq!(error("expect"), "t.scenario:1: expect what? Use one of: result, stopped, journal, !journal, field, shows");
        assert_eq!(
            error("expect hp e5 > 0"),
            "t.scenario:1: unknown expectation `hp`; expected result, stopped, journal, !journal, field, shows"
        );
        assert_eq!(
            error("expect journal phase 2"),
            "t.scenario:1: expect journal takes one text; quote it if it has spaces: \"phase 2\""
        );
        assert_eq!(
            error("expect journal"),
            "t.scenario:1: expect journal needs the text to look for, e.g. expect journal \"phase 1->2\""
        );
        assert_eq!(
            error("expect result is ok"),
            "t.scenario:1: expect result needs `contains \"TEXT\"` or `!contains \"TEXT\"`"
        );
        assert!(error("expect field e5 Health.current").contains("expect field needs ID Component.path OP NUMBER"));
        assert_eq!(error("expect field e5 Health.current = 5"), "t.scenario:1: unknown comparison `=`; use one of < <= > >= == !=");
        assert_eq!(error("expect field e5 Health.current > lots"), "t.scenario:1: expect field compares with a number, got `lots`");
        assert_eq!(error("expect field boss Health.current > 1"), "t.scenario:1: `boss` is not an entity id; use e6 or 6");
        assert_eq!(error("expect journal \"open"), "t.scenario:1: unclosed `\"` quote");
        assert_eq!(
            error("session"),
            "t.scenario:1: session changes nothing; use session [--agent NAME] [--scope player|dev] [--subject ID | --player N] [--clear-subject]"
        );
        assert_eq!(error("session --scope god"), "t.scenario:1: unknown scope `god`; use player or dev");
        assert!(error("session --subject e7 --clear-subject").contains("contradict"));
        assert_eq!(error("session --player one"), "t.scenario:1: --player needs a player number such as 0 or 1, got `one`");
        assert!(error("session --player 1 --subject e7").contains("contradicts --subject"));
        assert!(error("session --player 1 --clear-subject").contains("contradicts --subject"));
        assert!(error("session --player").contains("`--player` needs a player number"));
    }

    #[test]
    fn a_stand_header_names_the_members_and_the_links() {
        let Command::Stand(header) = only("stand clients=2 latency=150 loss=0.02") else { panic!() };
        assert_eq!(header, StandHeader { clients: 2, latency_ms: 150, jitter_ms: 0, loss: 0.02, dup: 0.0 });
        assert_eq!(header.members(), ["host", "c1", "c2"]);
        assert_eq!(header.describe(), "host + 2 clients");
        assert_eq!(header.net_sim(), "latency=150,loss=0.02");
        assert_eq!(header.to_string(), "stand clients=2 latency=150 loss=0.02");
        // Every key, the aliases, units.
        let Command::Stand(all) = only("stand clients=1 latency=40ms jitter_ms=5 loss=2% duplicate=0.5") else { panic!() };
        assert_eq!((all.clients, all.latency_ms, all.jitter_ms, all.dup), (1, 40, 5, 0.5));
        assert!((all.loss - 0.02).abs() < 1e-6);
        assert_eq!(all.describe(), "host + 1 client");
        let Command::Stand(plain) = only("stand clients=2") else { panic!() };
        assert!(!plain.has_conditions());
        assert_eq!(plain.net_sim(), "");
        // The header comes first, `seed` before or after it; the rest of the file follows.
        let script = parse("stand clients=2 latency=150\nseed 42\n@c1 act lock_on\nstep 5\n@host expect journal \"x\"\n");
        assert_eq!(script.stand(), Some(&header_with(2, 150)));
        assert_eq!(script.seed(), Some(42));
        assert_eq!(parse("seed 42\nstand clients=1\n").stand().map(|h| h.clients), Some(1));
        assert_eq!(parse("observe\n").stand(), None);
    }

    fn header_with(clients: u8, latency_ms: u32) -> StandHeader {
        StandHeader { clients, latency_ms, jitter_ms: 0, loss: 0.0, dup: 0.0 }
    }

    #[test]
    fn a_stand_header_is_checked_against_the_links_a_stand_reports() {
        let header = header_with(2, 150);
        assert!(header.check_conditions(Some("latency=150,jitter=0,loss=0,dup=0")).is_ok());
        let lossy = StandHeader { loss: 0.02, ..header };
        assert!(lossy.check_conditions(Some("latency=150,jitter=0,loss=0.02,dup=0")).is_ok());
        let plain = header_with(1, 0);
        assert!(plain.check_conditions(None).is_ok(), "no conditions at all is no conditions");
        assert!(plain.check_conditions(Some("")).is_ok());
        let e = lossy.check_conditions(Some("latency=150,jitter=0,loss=0,dup=0")).unwrap_err();
        assert_eq!(
            e,
            "this scenario's `stand` line wants links with latency=150,loss=0.02, but the running stand has latency=150,jitter=0,loss=0,dup=0: \
             start it with `--net-sim latency=150,loss=0.02` (or change the stand line)"
        );
        let e = plain.check_conditions(Some("latency=20,jitter=0,loss=0,dup=0")).unwrap_err();
        assert!(e.contains("wants links with no conditions") && e.contains("start it without --net-sim"), "{e}");
        assert!(header.check_conditions(None).unwrap_err().contains("but the running stand has none"));
        assert!(header.check_conditions(Some("fast")).unwrap_err().contains("cannot read the stand's link conditions `fast`"));
    }

    #[test]
    fn commands_name_the_member_they_run_on() {
        let script = parse(
            "stand clients=2\n@c1 act move_to target=e6\n@c2 step 5 --until journal:x\nact lock_on\n@host expect journal \"x\"\n\
             @c2 invariant field e5 Health.current > 0\n@c1 mark\n@c1 session --player 1\n@c1 observe\n",
        );
        let peers: Vec<Option<&str>> = script.lines.iter().map(|l| l.peer.as_deref()).collect();
        assert_eq!(peers, [None, Some("c1"), Some("c2"), None, Some("host"), Some("c2"), Some("c1"), Some("c1"), Some("c1")]);
        assert_eq!(script.lines[1].text, "@c1 act move_to target=e6", "the log shows the member");
        let Command::Step { ticks, until } = &script.lines[2].command else { panic!() };
        assert_eq!((*ticks, until.len()), (Some(5), 1));
    }

    #[test]
    fn stand_errors_name_the_line_and_the_fix() {
        assert_eq!(error("stand"), "t.scenario:1: stand needs clients=N; use stand clients=N [latency=MS] [jitter=MS] [loss=P] [dup=P], e.g. `stand clients=2 latency=150 loss=0.02`");
        assert!(error("stand latency=150").contains("stand needs clients=N"));
        assert_eq!(error("stand clients=3"), "t.scenario:1: stand: clients must be 1 or 2 (a host and one or two clients), got `3`");
        assert!(error("stand clients=0").contains("clients must be 1 or 2"));
        assert!(error("stand clients=two").contains("got `two`"));
        assert_eq!(error("stand clients=2 lag=5"), "t.scenario:1: stand: unknown key `lag`; use clients, latency, jitter, loss, dup");
        assert_eq!(error("stand clients=2 clients=1"), "t.scenario:1: stand: `clients` is given twice");
        assert!(error("stand clients=2 latency=fast").contains("latency is whole milliseconds"));
        assert!(error("stand clients=2 loss=1.5").contains("loss is a probability from 0 to 1"));
        assert!(error("stand clients=2 dup=-1").contains("dup is a probability"));
        assert!(error("stand 2").contains("expected key=value, got `2`"));
        assert_eq!(error("stand clients=1\nstand clients=2"), "t.scenario:2: a scenario has one `stand` line");
        assert_eq!(error("observe\nstand clients=2"), "t.scenario:2: `stand` must come before every other command (only `seed` may come first)");
        // The peer must be a member of the stand.
        assert_eq!(error("stand clients=2\n@c3 act ping"), "t.scenario:2: @c3: this stand has clients c1, c2");
        assert_eq!(error("stand clients=1\n@c2 act ping"), "t.scenario:2: @c2: this stand has client c1");
        assert_eq!(error("stand clients=2\n@c0 act ping"), "t.scenario:2: @c0: this stand has clients c1, c2");
        assert_eq!(error("stand clients=2\n@bob act ping"), "t.scenario:2: @bob: a member is `host` or one of the clients c1, c2");
        assert_eq!(error("stand clients=2\n@c01 act ping"), "t.scenario:2: @c01: a member is `host` or one of the clients c1, c2");
        assert!(error("@c1 act ping").contains("`@c1`: this scenario has no stand; start the file with `stand clients=N`"));
        assert!(error("@host observe").contains("this scenario has no stand"));
        assert_eq!(error("stand clients=2\n@c1"), "t.scenario:2: `@c1` needs a command after it, e.g. `@c1 act move_to target=e6`");
        assert!(error("stand clients=2\n@c1 seed 42").contains("`seed` is about the whole scenario"));
        assert!(error("@c1 stand clients=2").contains("no stand") || error("@c1 stand clients=2").contains("whole scenario"));
        // A peer before the header is not a member yet.
        assert!(error("@c1 act ping\nstand clients=2").contains("this scenario has no stand"));
        // Every bad line is still reported.
        let errors = Script::parse("t.scenario", "stand clients=2\n@c3 observe\nstpe\n@c1 observe\n").unwrap_err();
        assert_eq!(errors.0.iter().map(|e| e.line).collect::<Vec<_>>(), [2, 3]);
    }

    #[test]
    fn seed_comes_first() {
        assert_eq!(only("seed 42"), Command::Seed(42));
        assert_eq!(only("seed 0x5EED"), Command::Seed(0x5EED));
        assert_eq!(error("observe\nseed 42"), "t.scenario:2: `seed` must come before every other command");
        assert_eq!(error("seed 42\nseed 43"), "t.scenario:2: `seed` must come before every other command");
        assert_eq!(error("stand clients=1\nact ping\nseed 42"), "t.scenario:3: `seed` must come before every other command");
        assert_eq!(error("seed"), "t.scenario:1: seed needs one number, e.g. `seed 42`");
        assert_eq!(error("seed many"), "t.scenario:1: seed needs a whole number, got `many`");
    }

    #[test]
    fn every_bad_line_is_reported() {
        let errors = Script::parse("t.scenario", "stpe\nobserve\nactt x\n").unwrap_err();
        assert_eq!(errors.0.iter().map(|e| e.line).collect::<Vec<_>>(), [1, 3]);
    }
}
