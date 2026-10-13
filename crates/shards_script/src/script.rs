//! The scenario file: its commands and how a line becomes one.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::args::{loose_value, parse_args};
use crate::condition::parse_condition;
use crate::lex::split_words;

/// What the "unknown command" error offers.
const COMMANDS: &str = "act, act?, step, observe, session, expect, invariant, mark, seed";

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
    pub command: Command,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// The scenario is written for this seed (checked against the game's `status`).
    Seed(u64),
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
        let mut lines = Vec::new();
        let mut errors = Vec::new();
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
            let Some((head, args)) = split.words.split_first() else {
                continue;
            };
            match parse_command(head, args, lines.is_empty()) {
                Ok(command) => lines.push(ScriptLine { number, text: split.code, command }),
                Err(message) => errors.push(fail(message)),
            }
        }
        if errors.is_empty() { Ok(Script { name: name.to_owned(), lines }) } else { Err(ParseErrors(errors)) }
    }
}

fn parse_command(head: &str, args: &[String], first: bool) -> Result<Command, String> {
    match head {
        "seed" => {
            if !first {
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
    const USAGE: &str = "session [--agent NAME] [--scope player|dev] [--subject ID] [--clear-subject]";
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
            "--clear-subject" => config.clear_subject = true,
            other => return Err(format!("unknown option `{other}` for session; use {USAGE}")),
        }
    }
    if config.subject.is_some() && config.clear_subject {
        return Err("--subject and --clear-subject contradict each other".into());
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
                config: SessionConfig { scope: Some(AgentScope::Player), subject: Some("e7".into()), clear_subject: false }
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
            "t.scenario:2: unknown command `stpe`; expected act, act?, step, observe, session, expect, invariant, mark, seed"
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
        assert_eq!(error("session"), "t.scenario:1: session changes nothing; use session [--agent NAME] [--scope player|dev] [--subject ID] [--clear-subject]");
        assert_eq!(error("session --scope god"), "t.scenario:1: unknown scope `god`; use player or dev");
        assert!(error("session --subject e7 --clear-subject").contains("contradict"));
    }

    #[test]
    fn seed_comes_first() {
        assert_eq!(only("seed 42"), Command::Seed(42));
        assert_eq!(only("seed 0x5EED"), Command::Seed(0x5EED));
        assert_eq!(error("observe\nseed 42"), "t.scenario:2: `seed` must come before every other command");
        assert_eq!(error("seed 42\nseed 43"), "t.scenario:2: `seed` must come before every other command");
        assert_eq!(error("seed"), "t.scenario:1: seed needs one number, e.g. `seed 42`");
        assert_eq!(error("seed many"), "t.scenario:1: seed needs a whole number, got `many`");
    }

    #[test]
    fn every_bad_line_is_reported() {
        let errors = Script::parse("t.scenario", "stpe\nobserve\nactt x\n").unwrap_err();
        assert_eq!(errors.0.iter().map(|e| e.line).collect::<Vec<_>>(), [1, 3]);
    }
}
