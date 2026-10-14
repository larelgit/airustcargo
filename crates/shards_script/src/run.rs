//! Runs a [`Script`] against a game through a [`Driver`].
//!
//! The runner owns every rule of the format (what a command does, how expectations
//! and invariants are checked, what `mark` counts from). A driver only moves JSON:
//! in-process through `ai_core::call_tool`, or over BRP through the `shards` CLI. So
//! the same file means the same thing in `cargo test` and against a running game.

use std::fmt;

use serde::Serialize;
use serde_json::{json, Value};

use crate::script::{id_value, Command, Expect, ParseErrors, Script, ScriptLine, SessionConfig};

/// The agent that runs the commands, until a `session --agent` switches it.
pub const DEFAULT_AGENT: &str = "scenario";

/// The agent that reads the game for `expect` and `invariant`: always dev scope, so a
/// scenario that plays as a player-scope agent can still look at everything, and the
/// journal cursor of the agent under test stays where its own commands left it.
pub const CHECK_AGENT: &str = "scenario-check";

/// How a tool call ended, as the game reports it (`CallOutcome`).
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    /// The tool's output, or the error message when `ok` is false.
    pub result: Value,
    /// The tick after the call, when the transport says.
    pub tick: Option<u64>,
}

impl Outcome {
    pub fn ok(result: impl Into<Value>) -> Self {
        Self { ok: true, result: result.into(), tick: None }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self { ok: false, result: Value::String(message.into()), tick: None }
    }

    /// The result as text: strings as they are, anything else as pretty JSON.
    pub fn text(&self) -> String {
        match &self.result {
            Value::String(s) => s.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_default(),
        }
    }

    /// The result on one line, for the log.
    fn summary(&self) -> String {
        match &self.result {
            Value::String(s) => one_line(s),
            other => clip(&other.to_string(), 160),
        }
    }
}

/// What the runner needs to know about the game before it starts (`ai/status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub tick: u64,
    pub seed: Option<u64>,
    /// Sequence number of the newest journal entry.
    pub journal_seq: u64,
}

/// One journal entry as the journal prints it (`93 e6 winds up IceSlam (1.00s)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalLine {
    pub seq: u64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalRead {
    /// Entries newer than the cursor, oldest first. Without the agents' own call echoes
    /// (`[agent] dev: step …`): a scenario's `step --until journal:died` must not find
    /// itself.
    pub lines: Vec<JournalLine>,
    /// Entries that left the ring buffer before they were read.
    pub missed: u64,
}

/// The game a scenario drives. `Err` means the transport failed (the game is gone, the
/// request was refused): the scenario cannot go on. A tool that fails is not an
/// error here but an [`Outcome`] with `ok: false`.
pub trait Driver {
    /// Calls `tool` as `agent` (`ai/act`).
    fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String>;

    /// Creates or reconfigures `agent` (`ai/session`). On success the result is
    /// `{agent, scope, subject}`; a refusal (unknown subject) is `ok: false`.
    fn session(&mut self, agent: &str, config: &SessionConfig) -> Result<Outcome, String>;

    fn status(&mut self) -> Result<Status, String>;

    /// Journal entries with `seq > after`.
    fn journal_since(&mut self, after: u64) -> Result<JournalRead, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The file does not parse (or cannot be read): nothing ran.
    Parse,
    /// An expectation, an invariant or a command failed: the game did not do what the
    /// scenario says.
    Scenario,
    /// The game could not be driven: connection lost, request refused.
    Driver,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub kind: FailureKind,
    /// The command line that failed (for an invariant: the one that broke it); 0 when
    /// the file as a whole is at fault.
    pub line: usize,
    pub command: String,
    pub message: String,
}

/// One executed command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogLine {
    pub line: usize,
    pub ok: bool,
    pub command: String,
    /// What came of it, on one line: the tool's answer, the matching journal entry, or
    /// why it failed.
    pub detail: String,
}

impl fmt::Display for LogLine {
    /// `ok   12 step 600 --until near:e5:e12:1.5 → ran 214 ticks, stopped: …`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:<4}{:>3} ", if self.ok { "ok" } else { "FAIL" }, self.line)?;
        if !self.command.is_empty() {
            write!(f, "{} → ", clip(&self.command, 100))?;
        }
        f.write_str(&self.detail)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub name: String,
    pub passed: bool,
    pub failure: Option<Failure>,
    /// Every command that ran, the failing one last.
    pub log: Vec<LogLine>,
    pub commands: usize,
    /// Game ticks that passed while the scenario ran.
    pub ticks: u64,
}

impl Report {
    fn new(name: &str) -> Self {
        Self { name: name.to_owned(), passed: true, failure: None, log: Vec::new(), commands: 0, ticks: 0 }
    }

    /// A file that did not parse: nothing ran.
    pub fn parse_failed(name: &str, errors: &ParseErrors) -> Self {
        let mut report = Self::new(name);
        report.passed = false;
        report.log = errors
            .0
            .iter()
            .map(|e| LogLine { line: e.line, ok: false, command: String::new(), detail: e.message.clone() })
            .collect();
        report.failure = errors.0.first().map(|e| Failure {
            kind: FailureKind::Parse,
            line: e.line,
            command: String::new(),
            message: e.message.clone(),
        });
        report
    }

    /// A file that could not be read.
    pub fn unreadable(name: &str, message: impl Into<String>) -> Self {
        let mut report = Self::new(name);
        report.passed = false;
        report.failure = Some(Failure { kind: FailureKind::Parse, line: 0, command: String::new(), message: message.into() });
        report
    }

    /// `PASS boss_fight.scenario (14 commands, 214 ticks)` or `FAIL boss_fight.scenario:9: …`.
    pub fn summary(&self) -> String {
        match &self.failure {
            None => format!("PASS {} ({} commands, {} ticks)", self.name, self.commands, self.ticks),
            Some(f) if f.line == 0 => format!("FAIL {}: {}", self.name, f.message),
            Some(f) => format!("FAIL {}:{}: {}", self.name, f.line, f.message),
        }
    }
}

/// The per-command log, then the summary: what a failed test should print.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.log {
            writeln!(f, "{line}")?;
        }
        f.write_str(&self.summary())
    }
}

/// Runs `script` to its end or its first failure.
pub fn run<D: Driver + ?Sized>(script: &Script, driver: &mut D) -> Report {
    run_with(script, driver, &mut |_| {})
}

/// Like [`run`], and tells `on_line` about every command as soon as it ran, so a long
/// scenario can be followed live.
pub fn run_with<D: Driver + ?Sized>(script: &Script, driver: &mut D, on_line: &mut dyn FnMut(&LogLine)) -> Report {
    let mut report = Report::new(&script.name);
    let mut run = Run::new(driver);
    let started = match run.start() {
        Ok(tick) => tick,
        Err(fail) => {
            report.passed = false;
            report.failure = Some(fail.at(0, ""));
            return report;
        }
    };
    for line in &script.lines {
        let result = run.execute(line);
        let log = LogLine {
            line: line.number,
            ok: result.is_ok(),
            command: line.text.clone(),
            detail: match &result {
                Ok(detail) => detail.clone(),
                Err(fail) => fail.message.clone(),
            },
        };
        on_line(&log);
        report.log.push(log);
        report.commands += 1;
        if let Err(fail) = result {
            report.passed = false;
            report.failure = Some(fail.at(line.number, &line.text));
            break;
        }
    }
    report.ticks = run.tick.saturating_sub(started);
    report
}

struct Fail {
    kind: FailureKind,
    message: String,
}

impl Fail {
    fn check(message: impl Into<String>) -> Self {
        Self { kind: FailureKind::Scenario, message: message.into() }
    }

    fn lost(message: String) -> Self {
        Self { kind: FailureKind::Driver, message }
    }

    fn at(self, line: usize, command: &str) -> Failure {
        Failure { kind: self.kind, line, command: command.to_owned(), message: self.message }
    }
}

struct Invariant {
    line: usize,
    expect: Expect,
}

/// How the last `step` ended, from its message:
/// `ran 214 ticks (0 → 214), stopped: condition boss_phase_2`.
struct StepEnd {
    ran: String,
    stopped: Option<String>,
}

impl StepEnd {
    fn parse(message: &str) -> Self {
        match message.split_once(", stopped: ") {
            Some((ran, why)) => Self { ran: ran.to_owned(), stopped: Some(why.trim().to_owned()) },
            None => Self { ran: message.trim().to_owned(), stopped: None },
        }
    }

    /// A step the game cut short is a failure, though the call succeeded: a gameplay
    /// system crashed, or the ticks did not fit the wall-clock budget. What the
    /// scenario expects next would be checked on a world that did not get there.
    fn cut_short(&self) -> Result<(), Fail> {
        match &self.stopped {
            Some(why) if why.starts_with("gameplay ") => Err(Fail::check(format!(
                "the step {}, stopped: {why} (`act? step …` lets a step crash on purpose)",
                self.ran
            ))),
            Some(why) if why.contains("wall-clock budget") => Err(Fail::check(format!(
                "the step {}, stopped: {why}; step in smaller pieces",
                self.ran
            ))),
            _ => Ok(()),
        }
    }
}

struct Run<'d, D: ?Sized> {
    driver: &'d mut D,
    agent: String,
    seed: Option<u64>,
    /// Tick after the newest call.
    tick: u64,
    last_result: Option<String>,
    last_step: Option<StepEnd>,
    /// Journal expectations look at entries after this sequence number.
    mark: u64,
    marked_at: Option<usize>,
    invariants: Vec<Invariant>,
}

impl<'d, D: Driver + ?Sized> Run<'d, D> {
    fn new(driver: &'d mut D) -> Self {
        Self {
            driver,
            agent: DEFAULT_AGENT.to_owned(),
            seed: None,
            tick: 0,
            last_result: None,
            last_step: None,
            mark: 0,
            marked_at: None,
            invariants: Vec::new(),
        }
    }

    /// Reads the game's tick, seed and journal position and sets up the two agents.
    fn start(&mut self) -> Result<u64, Fail> {
        let status = self.driver.status().map_err(Fail::lost)?;
        self.seed = status.seed;
        self.tick = status.tick;
        self.mark = status.journal_seq;
        // A fresh body each time: an earlier scenario may have left the agent possessing someone.
        let default = SessionConfig { scope: Some(crate::AgentScope::Dev), clear_subject: true, ..Default::default() };
        for agent in [DEFAULT_AGENT, CHECK_AGENT] {
            let out = self.driver.session(agent, &default).map_err(Fail::lost)?;
            if !out.ok {
                return Err(Fail::lost(format!("cannot set up agent `{agent}`: {}", out.text())));
            }
        }
        Ok(status.tick)
    }

    fn execute(&mut self, line: &ScriptLine) -> Result<String, Fail> {
        match &line.command {
            Command::Seed(want) => match self.seed {
                Some(got) if got == *want => Ok(format!("seed {got}")),
                Some(got) => Err(Fail::check(format!(
                    "this scenario is written for seed {want}, but the game runs seed {got}; start the game with seed {want} or change the `seed` line"
                ))),
                None => Err(Fail::check("the game does not report its seed, so `seed N` cannot be checked")),
            },
            Command::Act { tool, args, may_fail } => {
                let (out, text) = self.call(tool, args.clone())?;
                if tool == "step" && out.ok {
                    let step = StepEnd::parse(&text);
                    if !may_fail {
                        step.cut_short()?;
                    }
                    self.last_step = Some(step);
                }
                let detail = match (out.ok, may_fail) {
                    (true, _) => out.summary(),
                    (false, true) => format!("failed, as allowed: {}", out.summary()),
                    (false, false) => return Err(Fail::check(format!("{tool} failed: {text}"))),
                };
                self.check_invariants()?;
                Ok(detail)
            }
            Command::Step { ticks, until } => {
                let mut args = json!({"until": until.iter().map(|c| c.json.clone()).collect::<Vec<_>>()});
                if let Some(ticks) = ticks {
                    args["ticks"] = json!(ticks);
                }
                let (out, text) = self.call("step", args)?;
                if !out.ok {
                    return Err(Fail::check(format!("step failed: {text}")));
                }
                let step = StepEnd::parse(&text);
                step.cut_short()?;
                self.last_step = Some(step);
                self.check_invariants()?;
                Ok(out.summary())
            }
            Command::Observe => {
                let (out, text) = self.call("observe", json!({}))?;
                if !out.ok {
                    return Err(Fail::check(format!("observe failed: {text}")));
                }
                Ok(format!("{} lines", text.lines().count()))
            }
            Command::Mark => {
                self.mark = self.driver.status().map_err(Fail::lost)?.journal_seq;
                self.marked_at = Some(line.number);
                Ok(format!("journal counts from entry {}", self.mark + 1))
            }
            Command::Session { agent, config } => {
                let name = agent.clone().unwrap_or_else(|| self.agent.clone());
                let out = self.driver.session(&name, config).map_err(Fail::lost)?;
                if !out.ok {
                    return Err(Fail::check(format!("session {name}: {}", out.text())));
                }
                self.agent = name;
                let word = |key: &str| out.result.get(key).and_then(Value::as_str).map(str::to_owned);
                let mut detail = format!("acting as {}", self.agent);
                if let Some(scope) = word("scope") {
                    detail.push_str(&format!(", {scope} scope"));
                }
                if let Some(subject) = word("subject") {
                    detail.push_str(&format!(", as {subject}"));
                }
                Ok(detail)
            }
            Command::Expect(expect) => self.eval(expect),
            Command::Invariant(expect) => {
                self.invariants.push(Invariant { line: line.number, expect: expect.clone() });
                Ok(format!("must hold after every act and step from here on: {expect}"))
            }
        }
    }

    /// Calls a tool as the current agent; remembers the tick and the result text.
    fn call(&mut self, tool: &str, args: Value) -> Result<(Outcome, String), Fail> {
        let agent = self.agent.clone();
        let out = self.driver.call(&agent, tool, args).map_err(Fail::lost)?;
        if let Some(tick) = out.tick {
            self.tick = tick;
        }
        let text = out.text();
        self.last_result = Some(text.clone());
        Ok((out, text))
    }

    fn check_invariants(&mut self) -> Result<(), Fail> {
        for index in 0..self.invariants.len() {
            let (line, expect) = (self.invariants[index].line, self.invariants[index].expect.clone());
            if let Err(fail) = self.eval(&expect) {
                return Err(match fail.kind {
                    FailureKind::Scenario => Fail::check(format!("invariant `{expect}` (line {line}) broke: {}", fail.message)),
                    _ => fail,
                });
            }
        }
        Ok(())
    }

    /// Checks one expectation. `Ok` carries what was found, for the log.
    fn eval(&mut self, expect: &Expect) -> Result<String, Fail> {
        match expect {
            Expect::Result { text, negate } => {
                let Some(result) = &self.last_result else {
                    return Err(Fail::check("there is no result to look at yet; run act, step or observe first"));
                };
                match (result.contains(text.as_str()), negate) {
                    (true, false) => Ok(format!("result contains {text:?}")),
                    (false, true) => Ok(format!("result lacks {text:?}")),
                    (false, false) => {
                        Err(Fail::check(format!("the result does not contain {text:?} (exact case); it is: {}", clip(result, 300))))
                    }
                    (true, true) => Err(Fail::check(format!("the result contains {text:?}, which it should not: {}", clip(result, 300)))),
                }
            }
            Expect::Stopped(text) => {
                let Some(step) = &self.last_step else {
                    return Err(Fail::check("no `step` has run yet, so nothing can have stopped"));
                };
                match &step.stopped {
                    Some(why) if contains_ci(why, text) => Ok(why.clone()),
                    Some(why) => Err(Fail::check(format!("the last step stopped on `{why}`, which does not contain {text:?}"))),
                    None => Err(Fail::check(format!(
                        "the last step ran to completion ({}); no condition stopped it, so {text:?} never happened",
                        step.ran
                    ))),
                }
            }
            Expect::Journal { text, negate } => {
                let read = self.driver.journal_since(self.mark).map_err(Fail::lost)?;
                let hit = read.lines.iter().find(|l| contains_ci(&l.text, text));
                let since = match self.marked_at {
                    Some(line) => format!("since `mark` on line {line}"),
                    None => "since the scenario started".to_owned(),
                };
                let dropped = if read.missed > 0 {
                    format!(" ({} older entries left the journal's buffer; use `mark` more often)", read.missed)
                } else {
                    String::new()
                };
                match (hit, negate) {
                    (Some(line), false) => Ok(line.text.clone()),
                    (None, true) if read.missed > 0 => Err(Fail::check(format!(
                        "no journal entry {since} contains {text:?}, but {} older entries left the journal's buffer \
                         unread, so one of them might; use `mark` more often or step in smaller pieces",
                        read.missed
                    ))),
                    (None, true) => Ok(format!("no entry contains {text:?}")),
                    (None, false) => {
                        let newest: Vec<&str> = read.lines.iter().rev().take(4).rev().map(|l| l.text.as_str()).collect();
                        let tail = if newest.is_empty() {
                            "the journal is empty then".to_owned()
                        } else {
                            format!("newest entries: {}", newest.join(" | "))
                        };
                        Err(Fail::check(format!("no journal entry {since} contains {text:?}{dropped}; {tail}")))
                    }
                    (Some(line), true) => {
                        Err(Fail::check(format!("a journal entry {since} contains {text:?}, which it should not: {}", line.text)))
                    }
                }
            }
            Expect::Field { id, component, path, op, value } => {
                let name = match path {
                    Some(path) => format!("{id} {component}.{path}"),
                    None => format!("{id} {component}"),
                };
                let mut args = json!({"id": id_value(id), "component": component});
                if let Some(path) = path {
                    args["path"] = json!(path);
                }
                let out = self.driver.call(CHECK_AGENT, "get", args).map_err(Fail::lost)?;
                if !out.ok {
                    return Err(Fail::check(format!("cannot read {name}: {}", out.text())));
                }
                let Some(now) = out.result.as_f64() else {
                    return Err(Fail::check(format!(
                        "{name} is not a number ({}); `expect field` compares numbers, so name a numeric field: Component.path",
                        clip(&out.text(), 120)
                    )));
                };
                if op.holds(now, *value) {
                    Ok(format!("{name} = {now}"))
                } else {
                    Err(Fail::check(format!("{name} is {now}, which is not {} {value}", op.symbol())))
                }
            }
            Expect::Shows { id, text } => {
                let out = self.driver.call(CHECK_AGENT, "inspect", json!({"id": id_value(id)})).map_err(Fail::lost)?;
                if !out.ok {
                    return Err(Fail::check(format!("cannot inspect {id}: {}", out.text())));
                }
                let full = out.text();
                // The description the AI reads; the reflection dump below it is not "shown".
                let shown = full.split("\ncomponents:").next().unwrap_or(&full);
                if contains_ci(shown, text) {
                    Ok(format!("{id} shows {text:?}"))
                } else {
                    let summary: Vec<&str> = shown.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
                    Err(Fail::check(format!("{id} does not show {text:?}; it shows: {}", clip(&summary.join(" | "), 300))))
                }
            }
        }
    }
}

/// `inspect`-style `journal` text into entries (what `ai/journal` returns over BRP):
///
/// ```text
/// journal (newest seq 41), 3 entries dropped
///   #12 93 e6 winds up IceSlam (1.00s)
/// ```
///
/// Agents' own call echoes are left out, as [`JournalRead::lines`] promises.
pub fn parse_journal_text(text: &str) -> JournalRead {
    let mut read = JournalRead::default();
    let mut lines = text.lines();
    if let Some(header) = lines.next() {
        read.missed = header.split(", ").skip(1).find_map(|part| part.strip_suffix(" entries dropped")?.parse().ok()).unwrap_or(0);
    }
    let mut current: Option<JournalLine> = None;
    for line in lines {
        let entry = line.strip_prefix("  #").and_then(|rest| {
            let (seq, text) = rest.split_once(' ')?;
            Some(JournalLine { seq: seq.parse().ok()?, text: text.to_owned() })
        });
        match entry {
            Some(entry) => read.lines.extend(current.replace(entry)),
            // An entry whose text has line breaks.
            None => {
                if let Some(entry) = &mut current {
                    entry.text.push('\n');
                    entry.text.push_str(line);
                }
            }
        }
    }
    read.lines.extend(current);
    read.lines.retain(|l| !is_agent_echo(&l.text));
    read
}

/// `93 [agent] dev: step {…}`: the journal's record of an agent's call.
fn is_agent_echo(entry: &str) -> bool {
    entry.split_once(' ').is_some_and(|(tick, rest)| tick.bytes().all(|b| b.is_ascii_digit()) && rest.starts_with("[agent] "))
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// The first non-empty line, marked when there is more.
fn one_line(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    let more = lines.next().is_some();
    clip(&format!("{first}{}", if more { " …" } else { "" }), 160)
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::AgentScope;

    /// A pretend game: a counter, a health value, a journal and a few tools.
    #[derive(Default)]
    struct Fake {
        tick: u64,
        seed: u64,
        hp: f64,
        journal: Vec<(u64, String)>,
        calls: Vec<(String, String, Value)>,
        sessions: Vec<(String, SessionConfig)>,
        gone: bool,
        /// Journal entries that left the buffer unread.
        missed: u64,
    }

    impl Fake {
        fn new() -> Self {
            Self { seed: 42, hp: 100.0, ..Default::default() }
        }

        fn log(&mut self, text: &str) {
            let seq = self.journal.len() as u64 + 1;
            self.journal.push((seq, format!("{} {text}", self.tick)));
        }
    }

    impl Driver for Fake {
        fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
            self.calls.push((agent.to_owned(), tool.to_owned(), args.clone()));
            if self.gone {
                return Err("cannot reach the game at http://127.0.0.1:15702".into());
            }
            let mut out = match tool {
                "attack" => Outcome::ok("ok: attacking e6 until it dies"),
                "hurt" => {
                    self.hp -= args["amount"].as_f64().unwrap_or(1.0);
                    self.log("e5 hurt");
                    Outcome::ok(format!("hp {}", self.hp))
                }
                "boom" => Outcome::err("bad arguments for boom: missing field `x`. Expected: boom(x: number)"),
                "step" => {
                    let ticks = args["ticks"].as_u64().unwrap_or(1);
                    if ticks == 666 {
                        self.tick += 50;
                        Outcome::ok("ran 50 ticks (0 → 50), stopped: gameplay system game::ice panicked at tick 50: the ice gave way")
                    } else if ticks == 777 {
                        self.tick += 300;
                        Outcome::ok("ran 300 ticks (0 → 300), stopped: 20 s wall-clock budget")
                    } else if args["until"].as_array().is_some_and(|u| !u.is_empty()) {
                        self.tick += 5;
                        self.log("e6 phase 1->2");
                        Outcome::ok("ran 5 ticks (0 → 5), stopped: journal: 5 e6 phase 1->2")
                    } else {
                        self.tick += ticks;
                        Outcome::ok(format!("ran {ticks} ticks"))
                    }
                }
                "observe" => Outcome::ok("tick 5\nYOU e5 Player hp 100/100"),
                "get" if args["path"] == "nope" => Outcome::ok(Value::Null),
                "get" => Outcome::ok(self.hp),
                "inspect" => Outcome::ok("e6 Boss \"Frost Warden\"\nat (0.0, 1.0)\n  Boss: phase 2\ncomponents:\n  Boss { phase: 7 }"),
                _ => Outcome::err(format!("unknown tool `{tool}`")),
            };
            out.tick = Some(self.tick);
            Ok(out)
        }

        fn session(&mut self, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
            self.sessions.push((agent.to_owned(), config.clone()));
            if config.subject.as_deref() == Some("e99") {
                return Ok(Outcome::err("e99 does not exist"));
            }
            let scope = config.scope.map_or("dev", AgentScope::as_str);
            Ok(Outcome::ok(json!({"agent": agent, "scope": scope, "subject": config.subject})))
        }

        fn status(&mut self) -> Result<Status, String> {
            Ok(Status { tick: self.tick, seed: Some(self.seed), journal_seq: self.journal.len() as u64 })
        }

        fn journal_since(&mut self, after: u64) -> Result<JournalRead, String> {
            let lines = self.journal.iter().filter(|(seq, _)| *seq > after).map(|(seq, text)| JournalLine { seq: *seq, text: text.clone() });
            Ok(JournalRead { lines: lines.collect(), missed: self.missed })
        }
    }

    fn run_text(text: &str) -> (Report, Fake) {
        let script = Script::parse("t.scenario", text).unwrap_or_else(|e| panic!("{e}"));
        let mut game = Fake::new();
        let report = run(&script, &mut game);
        (report, game)
    }

    fn failure(text: &str) -> Failure {
        let (report, _) = run_text(text);
        assert!(!report.passed, "expected a failure:\n{report}");
        report.failure.unwrap()
    }

    #[test]
    fn a_passing_scenario_logs_every_command() {
        let (report, game) = run_text(
            r#"
seed 42
act attack target=e6 repeat=true
step 10
step --until journal:phase
expect stopped "phase 1->2"
expect result contains "stopped: journal"
expect journal "PHASE 1->2"
expect !journal "died"
expect field e5 Health.current == 100
expect shows e6 "phase 2"
observe
expect result contains "YOU e5"
"#,
        );
        assert!(report.passed, "{report}");
        assert_eq!((report.commands, report.ticks), (12, 15));
        assert_eq!(report.log[1].to_string(), "ok    3 act attack target=e6 repeat=true → ok: attacking e6 until it dies");
        assert_eq!(report.log[3].detail, "ran 5 ticks (0 → 5), stopped: journal: 5 e6 phase 1->2");
        assert_eq!(report.summary(), "PASS t.scenario (12 commands, 15 ticks)");
        // Commands run as the default agent; checks as their own dev-scope agent.
        let agents: Vec<(&str, &str)> = game.calls.iter().map(|(a, t, _)| (a.as_str(), t.as_str())).collect();
        assert_eq!(agents[0], (DEFAULT_AGENT, "attack"));
        assert!(agents.contains(&(CHECK_AGENT, "get")) && agents.contains(&(CHECK_AGENT, "inspect")), "{agents:?}");
        let get = game.calls.iter().find(|c| c.1 == "get").unwrap();
        assert_eq!(get.2, json!({"id": "e5", "component": "Health", "path": "current"}));
        // Both agents start dev, with a fresh body.
        assert!(game.sessions.iter().all(|(_, c)| c.scope == Some(AgentScope::Dev) && c.clear_subject), "{:?}", game.sessions);
    }

    #[test]
    fn a_failing_act_fails_the_scenario_with_the_tools_message() {
        let f = failure("observe\nact boom\nstep 5");
        assert_eq!(f.kind, FailureKind::Scenario);
        assert_eq!(f.line, 2);
        assert_eq!(f.command, "act boom");
        assert_eq!(f.message, "boom failed: bad arguments for boom: missing field `x`. Expected: boom(x: number)");
        let (report, _) = run_text("observe\nact boom\nstep 5");
        assert_eq!(report.commands, 2, "nothing runs after the failure");
        assert_eq!(report.summary(), format!("FAIL t.scenario:2: {}", f.message));
    }

    #[test]
    fn act_question_mark_lets_a_call_fail_and_keeps_the_error_for_expect() {
        let (report, _) = run_text(
            "act? boom\nexpect result contains \"Expected: boom(x: number)\"\nexpect result !contains \"unknown tool\"\nact? fly\nexpect result contains \"unknown tool `fly`\"",
        );
        assert!(report.passed, "{report}");
        assert!(report.log[0].detail.starts_with("failed, as allowed: bad arguments for boom"), "{}", report.log[0].detail);
    }

    #[test]
    fn expect_result_is_case_sensitive_and_says_what_it_saw() {
        let f = failure("act attack\nexpect result contains \"OK: attacking\"");
        assert_eq!(f.line, 2);
        assert_eq!(f.message, "the result does not contain \"OK: attacking\" (exact case); it is: ok: attacking e6 until it dies");
        let f = failure("act attack\nexpect result !contains \"attacking\"");
        assert!(f.message.starts_with("the result contains \"attacking\", which it should not"), "{}", f.message);
        let f = failure("expect result contains \"x\"");
        assert_eq!(f.message, "there is no result to look at yet; run act, step or observe first");
    }

    #[test]
    fn expect_stopped_needs_a_condition_to_have_fired() {
        let f = failure("step 30\nexpect stopped \"phase\"");
        assert_eq!(f.message, "the last step ran to completion (ran 30 ticks); no condition stopped it, so \"phase\" never happened");
        let f = failure("step --until journal:phase\nexpect stopped \"gone\"");
        assert!(f.message.starts_with("the last step stopped on `journal: 5 e6 phase 1->2`"), "{}", f.message);
        let f = failure("act attack\nexpect stopped \"x\"");
        assert_eq!(f.message, "no `step` has run yet, so nothing can have stopped");
        // `act step` counts as a step too.
        let (report, _) = run_text("act step ticks=3 until='[{\"journal\":\"phase\"}]'\nexpect stopped \"phase\"");
        assert!(report.passed, "{report}");
    }

    #[test]
    fn journal_expectations_count_from_the_start_or_the_mark() {
        let (report, _) = run_text("act hurt amount=10\nexpect journal \"hurt\"\nmark\nexpect !journal \"hurt\"\nact hurt amount=1\nexpect journal \"HURT\"");
        assert!(report.passed, "{report}");
        let f = failure("act hurt\nmark\nexpect journal \"hurt\"");
        assert_eq!(f.message, "no journal entry since `mark` on line 2 contains \"hurt\"; the journal is empty then");
        let f = failure("act hurt\nexpect journal \"phase\"");
        assert_eq!(f.message, "no journal entry since the scenario started contains \"phase\"; newest entries: 0 e5 hurt");
        let f = failure("act hurt\nexpect !journal \"hurt\"");
        assert_eq!(f.message, "a journal entry since the scenario started contains \"hurt\", which it should not: 0 e5 hurt");
    }

    #[test]
    fn a_step_the_game_cut_short_fails_unless_allowed() {
        let f = failure("step 666\nexpect !journal \"died\"");
        assert_eq!(f.line, 1);
        assert_eq!(
            f.message,
            "the step ran 50 ticks (0 → 50), stopped: gameplay system game::ice panicked at tick 50: the ice gave way \
             (`act? step …` lets a step crash on purpose)"
        );
        assert_eq!(failure("act step ticks=666").line, 1, "`act step` is a step too");
        let (report, _) = run_text("act? step ticks=666\nexpect stopped \"panicked\"");
        assert!(report.passed, "{report}");
        let f = failure("step 777");
        assert_eq!(f.message, "the step ran 300 ticks (0 → 300), stopped: 20 s wall-clock budget; step in smaller pieces");
    }

    #[test]
    fn a_negated_journal_expectation_fails_when_entries_were_lost() {
        let script = Script::parse("t.scenario", "act hurt\nexpect journal \"hurt\"\nexpect !journal \"died\"").unwrap();
        let mut game = Fake::new();
        game.missed = 3;
        let f = run(&script, &mut game).failure.unwrap();
        assert_eq!(f.line, 3, "what is there is still found");
        assert_eq!(
            f.message,
            "no journal entry since the scenario started contains \"died\", but 3 older entries left the journal's buffer \
             unread, so one of them might; use `mark` more often or step in smaller pieces"
        );
    }

    #[test]
    fn journal_ignores_what_happened_before_the_scenario() {
        let script = Script::parse("t.scenario", "expect !journal \"old\"").unwrap();
        let mut game = Fake::new();
        game.log("old news");
        assert!(run(&script, &mut game).passed);
    }

    #[test]
    fn expect_field_compares_numbers() {
        let (report, _) = run_text("act hurt amount=30\nexpect field e5 Health.current < 100\nexpect field e5 Health.current >= 70\nexpect field e5 Health.current != 100\nexpect field e5 Health.current <= 70");
        assert!(report.passed, "{report}");
        let f = failure("act hurt amount=30\nexpect field e5 Health.current > 90");
        assert_eq!(f.message, "e5 Health.current is 70, which is not > 90");
        let f = failure("expect field e5 Health.nope > 1");
        assert_eq!(f.message, "e5 Health.nope is not a number (null); `expect field` compares numbers, so name a numeric field: Component.path");
    }

    #[test]
    fn expect_shows_reads_the_description_not_the_dump() {
        let (report, _) = run_text("expect shows e6 \"PHASE 2\"\nexpect shows e6 frost");
        assert!(report.passed, "{report}");
        let f = failure("expect shows e6 \"phase: 7\"");
        assert!(f.message.starts_with("e6 does not show \"phase: 7\"; it shows: e6 Boss \"Frost Warden\" | at (0.0, 1.0) | Boss: phase 2"), "{}", f.message);
    }

    #[test]
    fn an_invariant_is_checked_after_every_act_and_step() {
        let f = failure(
            "invariant field e5 Health.current > 50\nact hurt amount=20\nstep 5\nact hurt amount=40\nstep 5\nact hurt amount=1",
        );
        assert_eq!(f.line, 4, "the command that broke it");
        assert_eq!(f.command, "act hurt amount=40");
        assert_eq!(f.message, "invariant `field e5 Health.current > 50` (line 1) broke: e5 Health.current is 40, which is not > 50");
        // Not checked after `observe` or `mark`, and not on declaration.
        let (report, _) = run_text("act hurt amount=60\ninvariant field e5 Health.current > 50\nobserve\nmark\nact? fly");
        assert!(!report.passed);
        assert_eq!(report.failure.unwrap().line, 5);
    }

    #[test]
    fn invariants_can_watch_the_journal() {
        let f = failure("invariant !journal \"hurt\"\nstep 3\nact hurt\nstep 3");
        assert_eq!((f.line, f.command.as_str()), (3, "act hurt"));
    }

    #[test]
    fn session_switches_the_agent_for_the_following_commands() {
        let (report, game) = run_text("session --agent bot --scope player\nact attack\nsession --agent scenario\nact attack");
        assert!(report.passed, "{report}");
        assert_eq!(report.log[0].detail, "acting as bot, player scope");
        let who: Vec<&str> = game.calls.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(who, ["bot", "scenario"]);
        let bot = game.sessions.iter().find(|(a, c)| a == "bot" && c.scope.is_some()).unwrap();
        assert_eq!(bot.1.scope, Some(AgentScope::Player));
        // The checks keep their own agent while the scenario plays as a player.
        let (_, game) = run_text("session --scope player\nexpect field e5 Health.current > 0");
        assert!(game.calls.iter().any(|c| c.0 == CHECK_AGENT && c.1 == "get"));
        let f = failure("session --subject e99");
        assert_eq!(f.message, "session scenario: e99 does not exist");
    }

    #[test]
    fn the_seed_line_is_checked_against_the_game() {
        let f = failure("seed 7\nobserve");
        assert_eq!(f.line, 1);
        assert!(f.message.starts_with("this scenario is written for seed 7, but the game runs seed 42"), "{}", f.message);
    }

    #[test]
    fn a_lost_game_is_a_driver_failure_not_a_scenario_failure() {
        let script = Script::parse("t.scenario", "observe\nact attack").unwrap();
        let mut game = Fake::new();
        game.gone = true;
        let report = run(&script, &mut game);
        let f = report.failure.unwrap();
        assert_eq!((f.kind, f.line), (FailureKind::Driver, 1));
        assert!(f.message.starts_with("cannot reach the game"));
    }

    #[test]
    fn progress_is_reported_live() {
        let script = Script::parse("t.scenario", "observe\nact? fly\nstep 2").unwrap();
        let mut seen = Vec::new();
        let report = run_with(&script, &mut Fake::new(), &mut |l| seen.push(l.line));
        assert_eq!(seen, [1, 2, 3]);
        assert!(report.passed);
    }

    #[test]
    fn a_report_prints_as_a_log_and_a_verdict() {
        let (report, _) = run_text("observe\nact boom");
        let text = report.to_string();
        assert!(text.starts_with("ok    1 observe → 2 lines\nFAIL  2 act boom → boom failed: bad arguments"), "{text}");
        assert!(text.ends_with(&format!("FAIL t.scenario:2: {}", report.failure.as_ref().unwrap().message)), "{text}");
        let errors = Script::parse("t.scenario", "stpe").unwrap_err();
        let parse = Report::parse_failed("t.scenario", &errors);
        assert_eq!(parse.failure.as_ref().unwrap().kind, FailureKind::Parse);
        assert!(parse.summary().starts_with("FAIL t.scenario:1: unknown command `stpe`"), "{}", parse.summary());
        assert_eq!(Report::unreadable("x.scenario", "no such file").summary(), "FAIL x.scenario: no such file");
    }

    #[test]
    fn journal_text_is_read_back() {
        let read = parse_journal_text(
            "journal (newest seq 41), 3 entries dropped\n  #12 93 e6 winds up IceSlam (1.00s)\n  #13 93 [agent] dev: step {\"until\":[]}\n  #14 94 [sim] paused\n  #15 95 [warn] two\nlines\n",
        );
        assert_eq!(read.missed, 3);
        let seqs: Vec<u64> = read.lines.iter().map(|l| l.seq).collect();
        assert_eq!(seqs, [12, 14, 15], "agent echoes are dropped");
        assert_eq!(read.lines[0].text, "93 e6 winds up IceSlam (1.00s)");
        assert_eq!(read.lines[2].text, "95 [warn] two\nlines");
        assert_eq!(parse_journal_text("journal (newest seq 0)").lines, []);
    }
}
