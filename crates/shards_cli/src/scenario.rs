//! `shards scenario FILE...`: scenario files against the running game.
//!
//! The format and every rule of it live in `shards_script`; this is only the BRP side
//! of its `Driver`: `ai/act`, `ai/session`, `ai/status` and `ai/journal`. The same
//! files run in-process under `cargo test` through `ai_core::scenario`.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::{json, Value};
use shards_script::{
    parse_journal_text, run_with, AgentScope, Driver, FailureKind, JournalRead, Outcome, Script, SessionConfig, Status, CHECK_AGENT,
};

use crate::Client;

struct ClientDriver<'a> {
    client: &'a Client,
}

impl Driver for ClientDriver<'_> {
    fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
        let reply = self.client.call("ai/act", json!({"agent": agent, "tool": tool, "args": args}))?;
        Ok(Outcome {
            ok: reply.get("ok").and_then(Value::as_bool).unwrap_or(true),
            result: reply.get("result").cloned().unwrap_or(Value::Null),
            tick: reply.get("tick").and_then(Value::as_u64),
        })
    }

    fn session(&mut self, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
        let params = json!({
            "agent": agent,
            "scope": config.scope.map(AgentScope::as_str),
            "subject": config.subject,
            "clear_subject": config.clear_subject,
        });
        match self.client.call("ai/session", params) {
            Ok(session) => Ok(Outcome::ok(session)),
            // The game answered and refused (an unknown subject): the scenario is wrong,
            // not the connection. `Client::call` prefixes such errors with the method.
            Err(e) => match e.strip_prefix("ai/session: ") {
                Some(refusal) => Ok(Outcome::err(refusal)),
                None => Err(e),
            },
        }
    }

    fn status(&mut self) -> Result<Status, String> {
        let status = self.client.call("ai/status", Value::Null)?;
        Ok(Status {
            tick: status["tick"].as_u64().unwrap_or(0),
            seed: status["seed"].as_u64(),
            journal_seq: status["journal_seq"].as_u64().unwrap_or(0),
        })
    }

    fn journal_since(&mut self, after: u64) -> Result<JournalRead, String> {
        let reply = self.client.call("ai/journal", json!({"agent": CHECK_AGENT, "since_seq": after, "last": 1_000_000}))?;
        match (reply.get("ok").and_then(Value::as_bool), reply.get("result").and_then(Value::as_str)) {
            (Some(true) | None, Some(text)) => Ok(parse_journal_text(text)),
            _ => Err(format!("ai/journal: unexpected reply {reply}")),
        }
    }
}

/// Reads and parses every file first (a typo in the third file should not show up
/// after the first two ran), then runs them in order against the game.
pub fn run(client: &Client, files: &[PathBuf], quiet: bool, as_json: bool) -> Result<ExitCode, String> {
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

    let mut driver = ClientDriver { client };
    let mut reports = Vec::new();
    for script in &scripts {
        let report = run_with(script, &mut driver, &mut |line| {
            if !as_json && (!quiet || !line.ok) {
                println!("{line}");
            }
        });
        if !as_json {
            println!("{}", report.summary());
        }
        let lost = report.failure.as_ref().is_some_and(|f| f.kind == FailureKind::Driver);
        reports.push(report);
        if lost {
            // The game is gone or refuses us: the next file would fail the same way.
            break;
        }
    }

    if as_json {
        println!("{}", serde_json::to_string_pretty(&reports).unwrap_or_default());
    } else if scripts.len() > 1 {
        let passed = reports.iter().filter(|r| r.passed).count();
        println!("{passed} of {} scenarios passed", scripts.len());
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
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// A pretend game on localhost that speaks the four `ai/*` methods the driver uses.
    /// Returns its port; the thread ends with the test process.
    fn fake_game() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut tick = 0u64;
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream);
                let mut length = 0usize;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header.trim().is_empty() {
                        break;
                    }
                    if let Some(n) = header.to_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                let params = &request["params"];
                let outcome = |ok: bool, result: Value, tick: u64| json!({"tool": "t", "ok": ok, "result": result, "tick": tick, "paused": true});
                let result = match request["method"].as_str().unwrap() {
                    "ai/status" => json!({"tick": tick, "seed": 42, "journal_seq": 0}),
                    "ai/session" if params["subject"] == "e99" => {
                        let reply = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32602, "message": "e99 does not exist"}});
                        respond(reader.into_inner(), &reply);
                        continue;
                    }
                    "ai/session" => json!({"agent": params["agent"], "scope": params["scope"].as_str().unwrap_or("dev"), "subject": null}),
                    "ai/act" => match params["tool"].as_str().unwrap() {
                        "step" => {
                            tick += params["args"]["ticks"].as_u64().unwrap_or(1);
                            outcome(true, json!(format!("ran 1 ticks (0 → {tick})")), tick)
                        }
                        "ping" => outcome(true, json!("pong"), tick),
                        "get" => outcome(true, json!(70.0), tick),
                        _ => outcome(false, json!("nope: no such tool"), tick),
                    },
                    "ai/journal" => outcome(
                        true,
                        json!("journal (newest seq 2)\n  #1 0 [agent] scenario: ping\n  #2 3 e6 phase 1->2"),
                        tick,
                    ),
                    other => panic!("unexpected method {other}"),
                };
                respond(reader.into_inner(), &json!({"jsonrpc": "2.0", "id": 1, "result": result}));
            }
        });
        port
    }

    fn respond(mut stream: std::net::TcpStream, reply: &Value) {
        let body = reply.to_string();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();
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
        let exit = |files: Vec<PathBuf>| format!("{:?}", run(&client, &files, true, false).unwrap());
        assert_eq!(exit(vec![good.clone()]), code(0));
        assert_eq!(exit(vec![good, bad]), code(1));
    }

    #[test]
    fn a_session_the_game_refuses_fails_the_scenario_not_the_connection() {
        let client = Client::new(fake_game(), None);
        let mut driver = ClientDriver { client: &client };
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
        let read = ClientDriver { client: &client }.journal_since(0).unwrap();
        assert_eq!(read.lines.len(), 1);
        assert_eq!(read.lines[0].text, "3 e6 phase 1->2");
    }

    #[test]
    fn no_game_is_a_connection_error_and_bad_files_run_nothing() {
        let free = TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let client = Client::new(free, None);
        let file = scenario_file("any.scenario", "observe\n");
        assert_eq!(format!("{:?}", run(&client, &[file], true, false).unwrap()), code(2));

        let typo = scenario_file("typo.scenario", "observe\nstpe 5\n");
        let err = run(&client, &[typo], true, false).unwrap_err();
        assert_eq!(err, "no scenario ran; fix the files above");
        let err = run(&client, &[PathBuf::from("no/such.scenario")], true, false).unwrap_err();
        assert_eq!(err, "no scenario ran; fix the files above");
    }
}
