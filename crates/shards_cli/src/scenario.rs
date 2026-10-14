//! `shards scenario FILE...`: scenario files against the running game.
//!
//! The format and every rule of it live in `shards_script`; this is only the BRP side
//! of its `Driver`: `ai/act`, `ai/session`, `ai/status` and `ai/journal`. The same
//! files run in-process under `cargo test` through `ai_core::scenario`.
//!
//! A file with a `stand` line runs against a stand (`cargo sandbox --stand 2 --port-file F`):
//! the members are found by their port files (`F`, `F.c1`, `F.c2`) and each command goes to
//! the member it names. The stand's link conditions must be the line's.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};
use shards_script::{
    parse_journal_text, run_with, AgentScope, Driver, FailureKind, JournalRead, Outcome, Script, SessionConfig, StandHeader, Status,
    CHECK_AGENT, HOST,
};

use crate::peer::{client_files, Peer};
use crate::{say, Cli, Client};

struct ClientDriver<'a> {
    cli: &'a Cli,
    /// The game `--port`/`--port-file` names: a single game, or a stand's host.
    host: &'a Client,
    /// The stand's clients, found by their port files when a command first names them.
    clients: BTreeMap<String, Client>,
}

impl<'a> ClientDriver<'a> {
    fn new(cli: &'a Cli, host: &'a Client) -> Self {
        Self { cli, host, clients: BTreeMap::new() }
    }

    fn client(&mut self, peer: &str) -> Result<&Client, String> {
        if peer == HOST {
            return Ok(self.host);
        }
        if !self.clients.contains_key(peer) {
            let which: Peer = peer.parse()?;
            self.clients.insert(peer.to_owned(), self.cli.client_for(which)?);
        }
        Ok(&self.clients[peer])
    }
}

impl Driver for ClientDriver<'_> {
    fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
        self.call_on(HOST, agent, tool, args)
    }

    fn session(&mut self, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
        self.session_on(HOST, agent, config)
    }

    fn status(&mut self) -> Result<Status, String> {
        self.status_on(HOST)
    }

    fn journal_since(&mut self, after: u64) -> Result<JournalRead, String> {
        self.journal_since_on(HOST, after)
    }

    /// The host, and the clients that have a port file beside the host's.
    fn peers(&mut self) -> Result<Vec<String>, String> {
        let clients = self.cli.port_file.as_deref().map(client_files).unwrap_or_default();
        Ok(std::iter::once(HOST.to_owned()).chain(clients.into_iter().map(Peer::name)).collect())
    }

    /// The links of the running stand are the header's: `net_status` says how its host was
    /// started (`--net-sim`).
    fn check_stand(&mut self, header: &StandHeader) -> Result<(), String> {
        let status = self.call_on(HOST, CHECK_AGENT, "net_status", json!({}))?;
        if !status.ok {
            return Err(format!("cannot read the stand's links: {}", status.text()));
        }
        header.check_conditions(status.result["conditions"].as_str())
    }

    fn call_on(&mut self, peer: &str, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
        let reply = self.client(peer)?.act(agent, tool, args)?;
        Ok(Outcome {
            ok: reply.get("ok").and_then(Value::as_bool).unwrap_or(true),
            result: reply.get("result").cloned().unwrap_or(Value::Null),
            tick: reply.get("tick").and_then(Value::as_u64),
        })
    }

    fn session_on(&mut self, peer: &str, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
        let params = json!({
            "agent": agent,
            "scope": config.scope.map(AgentScope::as_str),
            "subject": config.subject,
            "player": config.player,
            "clear_subject": config.clear_subject,
        });
        match self.client(peer)?.call("ai/session", params) {
            Ok(session) => Ok(Outcome::ok(session)),
            // The game answered and refused (an unknown subject): the scenario is wrong,
            // not the connection. `Client::call` prefixes such errors with the method.
            Err(e) => match e.strip_prefix("ai/session: ") {
                Some(refusal) => Ok(Outcome::err(refusal)),
                None => Err(e),
            },
        }
    }

    fn status_on(&mut self, peer: &str) -> Result<Status, String> {
        let status = self.client(peer)?.call("ai/status", Value::Null)?;
        Ok(Status {
            tick: status["tick"].as_u64().unwrap_or(0),
            seed: status["seed"].as_u64(),
            journal_seq: status["journal_seq"].as_u64().unwrap_or(0),
        })
    }

    fn journal_since_on(&mut self, peer: &str, after: u64) -> Result<JournalRead, String> {
        let reply = self.client(peer)?.call("ai/journal", json!({"agent": CHECK_AGENT, "since_seq": after, "last": 1_000_000}))?;
        match (reply.get("ok").and_then(Value::as_bool), reply.get("result").and_then(Value::as_str)) {
            (Some(true) | None, Some(text)) => Ok(parse_journal_text(text)),
            _ => Err(format!("ai/journal: unexpected reply {reply}")),
        }
    }
}

/// Reads and parses every file first (a typo in the third file should not show up
/// after the first two ran), then runs them in order against the game.
pub fn run(cli: &Cli, client: &Client, files: &[PathBuf], quiet: bool, as_json: bool) -> Result<ExitCode, String> {
    let mut scripts = Vec::new();
    let mut unreadable = false;
    for path in files {
        let name = path.display().to_string();
        match std::fs::read_to_string(path) {
            Ok(text) => match Script::parse(&name, &text) {
                Ok(script) => scripts.push(script),
                Err(errors) => {
                    unreadable = true;
                    eprintln!("{errors}");
                }
            },
            Err(e) => {
                unreadable = true;
                eprintln!("{name}: cannot read the file ({e})");
            }
        }
    }
    if unreadable {
        return Err("no scenario ran; fix the files above".into());
    }

    // A stand scenario names its members itself (@host, @c1): its host is the stand's, found by
    // the port file, whatever `--peer` or SHARDS_PEER says about the game a single command talks to.
    let stand_host = match (scripts.iter().any(|s| s.stand().is_some()), cli.port_file.is_some()) {
        (true, true) => {
            if let Some(peer) = cli.peer.filter(|peer| *peer != Peer::Host) {
                eprintln!("note: --peer {peer} is ignored for a stand scenario: it names its members itself (@host, @c1 …)");
            }
            Some(cli.client_for(Peer::Host)?)
        }
        _ => None,
    };
    let mut reports = Vec::new();
    for script in &scripts {
        let mut driver = ClientDriver::new(cli, if script.stand().is_some() { stand_host.as_ref().unwrap_or(client) } else { client });
        let report = run_with(script, &mut driver, &mut |line| {
            if !as_json && (!quiet || !line.ok) {
                say(line);
            }
        });
        if !as_json {
            say(report.summary());
        }
        let lost = report.failure.as_ref().is_some_and(|f| f.kind == FailureKind::Driver);
        reports.push(report);
        if lost {
            // The game is gone or refuses us: the next file would fail the same way.
            break;
        }
    }

    if as_json {
        say(serde_json::to_string_pretty(&reports).unwrap_or_default());
    } else if scripts.len() > 1 {
        let passed = reports.iter().filter(|r| r.passed).count();
        say(format!("{passed} of {} scenarios passed", scripts.len()));
    }
    let failure = reports.iter().filter_map(|r| r.failure.as_ref()).map(|f| f.kind);
    Ok(if failure.clone().any(|k| k == FailureKind::Driver) {
        ExitCode::from(2)
    } else if failure.count() > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::fake::{error, reply, serve};

    /// A pretend game on localhost that speaks the four `ai/*` methods the driver uses.
    /// Returns its port; the thread ends with the test process.
    fn fake_game() -> u16 {
        let tick = AtomicU64::new(0);
        serve(move |method, params| {
            let outcome = |ok: bool, result: Value, tick: u64| json!({"tool": "t", "ok": ok, "result": result, "tick": tick, "paused": true});
            let now = tick.load(Ordering::SeqCst);
            Some(match method {
                "ai/status" => reply(json!({"tick": now, "seed": 42, "journal_seq": 0})),
                "ai/session" if params["subject"] == "e99" => error("e99 does not exist"),
                "ai/session" => reply(json!({"agent": params["agent"], "scope": params["scope"].as_str().unwrap_or("dev"), "subject": null})),
                "ai/act" => reply(match params["tool"].as_str().unwrap() {
                    "step" => {
                        let to = tick.fetch_add(params["args"]["ticks"].as_u64().unwrap_or(1), Ordering::SeqCst) + params["args"]["ticks"].as_u64().unwrap_or(1);
                        outcome(true, json!(format!("ran 1 ticks (0 → {to})")), to)
                    }
                    "ping" => outcome(true, json!("pong"), now),
                    "get" => outcome(true, json!(70.0), now),
                    _ => outcome(false, json!("nope: no such tool"), now),
                }),
                "ai/journal" => reply(outcome(true, json!("journal (newest seq 2)\n  #1 0 [agent] scenario: ping\n  #2 3 e6 phase 1->2"), now)),
                other => panic!("unexpected method {other}"),
            })
        })
    }

    fn scenario_file(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shards_scenario_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    /// `ExitCode` cannot be compared, only printed.
    fn code(n: u8) -> String {
        format!("{:?}", ExitCode::from(n))
    }

    fn cli(args: &[&str]) -> Cli {
        crate::fake::parse(&[args, &["status"]].concat())
    }

    #[test]
    fn scenarios_run_against_a_game_over_brp() {
        let client = Client::new(fake_game(), None);
        let good = scenario_file(
            "good.scenario",
            "seed 42\nact ping\nexpect result contains \"pong\"\nact? fly\nexpect result contains \"nope\"\nstep 3\n\
             expect journal \"phase 1->2\"\nexpect !journal \"scenario: ping\"\nexpect field e5 Health.current < 100\n\
             session --agent bot --scope player\nact ping\n",
        );
        let bad = scenario_file("bad.scenario", "act ping\nexpect result contains \"ping\"\n");
        let exit = |files: Vec<PathBuf>| format!("{:?}", run(&cli(&[]), &client, &files, true, false).unwrap());
        assert_eq!(exit(vec![good.clone()]), code(0));
        assert_eq!(exit(vec![good, bad]), code(1));
    }

    #[test]
    fn a_session_the_game_refuses_fails_the_scenario_not_the_connection() {
        let client = Client::new(fake_game(), None);
        let cli = cli(&[]);
        let mut driver = ClientDriver::new(&cli, &client);
        let config = SessionConfig { subject: Some("e99".into()), ..Default::default() };
        let out = driver.session("scenario", &config).unwrap();
        assert!(!out.ok);
        assert_eq!(out.text(), "e99 does not exist");
        let ok = driver.session("scenario", &SessionConfig { scope: Some(AgentScope::Player), ..Default::default() }).unwrap();
        assert_eq!(ok.result["scope"], "player");
    }

    #[test]
    fn the_journal_comes_back_without_agent_echoes() {
        let client = Client::new(fake_game(), None);
        let cli = cli(&[]);
        let read = ClientDriver::new(&cli, &client).journal_since(0).unwrap();
        assert_eq!(read.lines.len(), 1);
        assert_eq!(read.lines[0].text, "3 e6 phase 1->2");
    }

    #[test]
    fn no_game_is_a_connection_error_and_bad_files_run_nothing() {
        let free = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let client = Client::new(free, None);
        let file = scenario_file("any.scenario", "observe\n");
        assert_eq!(format!("{:?}", run(&cli(&[]), &client, &[file], true, false).unwrap()), code(2));

        let typo = scenario_file("typo.scenario", "observe\nstpe 5\n");
        let err = run(&cli(&[]), &client, &[typo], true, false).unwrap_err();
        assert_eq!(err, "no scenario ran; fix the files above");
        let err = run(&cli(&[]), &client, &[PathBuf::from("no/such.scenario")], true, false).unwrap_err();
        assert_eq!(err, "no scenario ran; fix the files above");
    }

    /// What a pretend stand's member was asked.
    struct Asked {
        member: String,
        method: String,
        params: Value,
    }

    type Calls = Arc<Mutex<Vec<Asked>>>;

    /// How often `member` was asked `method` (and, for `ai/act`, `tool`; `""` for any other).
    fn asked(calls: &Calls, member: &str, method: &str, tool: &str) -> usize {
        let log = calls.lock().unwrap();
        log.iter().filter(|a| a.member == member && a.method == method && a.params["tool"].as_str().unwrap_or("") == tool).count()
    }

    /// A pretend stand of a host and two clients, each its own server, port files beside `base`.
    /// A `step` on any of them is deferred: it answers `{"pending": 1}` and `ai/step_poll` says
    /// pending twice before the report. `links` is what the host's `net_status` reports.
    fn pretend_stand(base: &std::path::Path, links: &'static str) -> Calls {
        let calls = Calls::default();
        for (i, name) in ["host", "c1", "c2"].into_iter().enumerate() {
            let calls = calls.clone();
            let polls = AtomicU64::new(0);
            let port = serve(move |method, params| {
                let tool = params["tool"].as_str().unwrap_or("").to_owned();
                calls.lock().unwrap().push(Asked { member: name.to_owned(), method: method.to_owned(), params: params.clone() });
                let outcome = |tool: &str, result: Value| json!({"tool": tool, "ok": true, "result": result, "tick": 7, "paused": true});
                Some(match (method, tool.as_str()) {
                    ("ai/status", _) => reply(json!({"tick": 5, "seed": 42, "journal_seq": 0})),
                    ("ai/session", _) => reply(json!({"agent": params["agent"], "scope": "dev", "subject": null, "player": params["player"]})),
                    ("ai/act", "step") => reply(outcome("step", json!({"pending": 1}))),
                    ("ai/step_poll", _) if polls.fetch_add(1, Ordering::SeqCst) < 2 => reply(outcome("step_poll", json!({"pending": 1}))),
                    ("ai/step_poll", _) => reply(outcome("step", json!("ran 3 ticks (5 → 8)"))),
                    ("ai/act", "net_status") => reply(outcome("net_status", json!({"conditions": links}))),
                    ("ai/act", "get") => reply(outcome("get", json!(if name == "c1" { 4.0 } else { 9.0 }))),
                    ("ai/act", _) => reply(outcome(&tool, json!("ok"))),
                    ("ai/journal", _) => reply(outcome("journal", json!("journal (newest seq 0)"))),
                    other => panic!("unexpected {other:?}"),
                })
            });
            let file = if i == 0 { base.to_owned() } else { crate::peer::member_file(base, Peer::Client(i as u8)) };
            std::fs::write(file, format!("{port}\n")).unwrap();
        }
        calls
    }

    fn stand_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shards_stand_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }


    #[test]
    fn a_stand_scenario_goes_to_the_members_it_names() {
        let dir = stand_dir("run");
        let base = dir.join("g.port");
        let calls = pretend_stand(&base, "latency=150,jitter=0,loss=0.02,dup=0");
        let path = base.display().to_string();
        let cli = cli(&["--port-file", &path]);
        let host = cli.client_for(Peer::Host).unwrap();
        let file = scenario_file(
            "stand.scenario",
            "stand clients=2 latency=150 loss=0.02\nseed 42\n@c1 act ping\n@c2 step 3\n@c1 expect field e5 Transform.translation.x < 5\n\
             @c2 expect field e5 Transform.translation.x > 5\n@c1 session --player 1\n",
        );
        let exit = run(&cli, &host, &[file], true, false).unwrap();
        assert_eq!(format!("{exit:?}"), code(0));
        assert_eq!(asked(&calls, "c1", "ai/act", "ping"), 1);
        assert_eq!(asked(&calls, "host", "ai/act", "ping"), 0, "the ping went to c1 alone");
        assert_eq!(asked(&calls, "c2", "ai/act", "step"), 1);
        assert_eq!(asked(&calls, "c2", "ai/step_poll", ""), 3, "pending twice, then the report");
        assert_eq!(asked(&calls, "c1", "ai/act", "get") + asked(&calls, "c2", "ai/act", "get"), 2);
        assert_eq!(asked(&calls, "host", "ai/act", "net_status"), 1, "the links were checked on the host");
        // `session --player 1` reached c1's `ai/session` as player 1, and nobody else's.
        let log = calls.lock().unwrap();
        let bound: Vec<&str> = log.iter().filter(|a| a.method == "ai/session" && a.params["player"] == 1).map(|a| a.member.as_str()).collect();
        assert_eq!(bound, ["c1"], "the binding is c1's alone");
        assert!(log.iter().filter(|a| a.method == "ai/session" && a.member != "c1").all(|a| a.params["player"].is_null()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stand_scenario_wants_the_links_of_its_header() {
        let dir = stand_dir("links");
        let base = dir.join("g.port");
        let calls = pretend_stand(&base, "latency=0,jitter=0,loss=0,dup=0");
        let path = base.display().to_string();
        let cli = cli(&["--port-file", &path]);
        let host = cli.client_for(Peer::Host).unwrap();
        let script = Script::parse("links.scenario", "stand clients=2 latency=150 loss=0.02\n@c1 act ping\n").unwrap();
        let failure = shards_script::run(&script, &mut ClientDriver::new(&cli, &host)).failure.expect("the links differ");
        assert_eq!((failure.kind, failure.line), (FailureKind::Driver, 0));
        assert_eq!(
            failure.message,
            "this scenario's `stand` line wants links with latency=150,loss=0.02, but the running stand has \
             latency=0,jitter=0,loss=0,dup=0: start it with `--net-sim latency=150,loss=0.02` (or change the stand line)"
        );
        assert_eq!(asked(&calls, "c1", "ai/act", "ping"), 0, "nothing ran");
        // As a command: a driver failure is exit code 2.
        let file = scenario_file("links.scenario", "stand clients=2 latency=150 loss=0.02\n@c1 act ping\n");
        assert_eq!(format!("{:?}", run(&cli, &host, &[file], false, true).unwrap()), code(2));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `--peer c1` (or `SHARDS_PEER=c1` left in the shell) names the game a single command talks to.
    /// A stand scenario names its members itself: its `@host` is the stand's host.
    #[test]
    fn a_stand_scenario_ignores_the_peer_option_and_a_single_game_scenario_obeys_it() {
        let dir = stand_dir("peer");
        let base = dir.join("g.port");
        let calls = pretend_stand(&base, "latency=150,jitter=0,loss=0.02,dup=0");
        let path = base.display().to_string();
        let cli = cli(&["--port-file", &path, "--peer", "c1"]);
        let client = cli.client().expect("what main hands to scenario::run: c1's client");
        assert_eq!(cli.port(), Ok(read_port(&crate::peer::member_file(&base, Peer::Client(1)))));
        let stand = scenario_file("peer-stand.scenario", "stand clients=2 latency=150 loss=0.02\n@host act ping\n@c1 act ping\nact ping\n");
        let exit = run(&cli, &client, &[stand], true, false).unwrap();
        assert_eq!(format!("{exit:?}"), code(0));
        assert_eq!(asked(&calls, "host", "ai/act", "ping"), 2, "@host and the unprefixed line are the host's");
        assert_eq!(asked(&calls, "c1", "ai/act", "ping"), 1, "@c1 is c1");
        assert_eq!(asked(&calls, "host", "ai/act", "net_status"), 1, "the links are the host's");
        assert_eq!(asked(&calls, "c1", "ai/act", "net_status"), 0);
        // A scenario without a stand line drives the game `--peer` names: c1.
        let single = scenario_file("peer-single.scenario", "act ping\n");
        let exit = run(&cli, &client, &[single], true, false).unwrap();
        assert_eq!(format!("{exit:?}"), code(0));
        assert_eq!(asked(&calls, "c1", "ai/act", "ping"), 2, "the single-game scenario ran on c1");
        assert_eq!(asked(&calls, "host", "ai/act", "ping"), 2, "and not on the host");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn read_port(file: &std::path::Path) -> u16 {
        std::fs::read_to_string(file).unwrap().trim().parse().unwrap()
    }

    #[test]
    fn a_game_on_its_own_is_not_a_stand() {
        let client = Client::new(fake_game(), None);
        let file = scenario_file("alone.scenario", "stand clients=2\nobserve\n");
        // No port file: the host is all there is.
        let exit = run(&cli(&[]), &client, std::slice::from_ref(&file), true, false).unwrap();
        assert_eq!(format!("{exit:?}"), code(2));
        let script = Script::parse("alone.scenario", "stand clients=2\nobserve\n").unwrap();
        let failure = shards_script::run(&script, &mut ClientDriver::new(&cli(&[]), &client)).failure.unwrap();
        assert_eq!(failure.kind, FailureKind::Driver);
        assert!(
            failure.message.starts_with("this scenario needs a stand of host + 2 clients: run cargo sandbox --stand 2 --headless --paused"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn the_members_of_a_stand_are_the_clients_with_port_files() {
        let dir = stand_dir("peers");
        let base = dir.join("g.port");
        pretend_stand(&base, "");
        let path = base.display().to_string();
        let cli_with = cli(&["--port-file", &path]);
        let host = cli_with.client_for(Peer::Host).unwrap();
        assert_eq!(ClientDriver::new(&cli_with, &host).peers().unwrap(), ["host", "c1", "c2"]);
        std::fs::remove_file(crate::peer::member_file(&base, Peer::Client(2))).unwrap();
        assert_eq!(ClientDriver::new(&cli_with, &host).peers().unwrap(), ["host", "c1"]);
        assert_eq!(ClientDriver::new(&cli(&[]), &host).peers().unwrap(), ["host"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
