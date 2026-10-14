//! A stand from the command line: `--peer`, `shards wait --peer all`, `shards stand` and
//! `shards stand stop`.
//!
//! `cargo sandbox --stand 2 --port 0 --port-file F` starts a host and two clients, each a game of
//! its own with its own `shards` port, and writes the ports to `F` (the host), `F.c1` and `F.c2`.
//! `--peer c1` (or `SHARDS_PEER=c1`) points any command at one member: the port is read from
//! its file, the token is looked up by that port as always.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{find_token, read_port_file, say, still_loading, Cli, Client};

/// The member of a stand a command talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peer {
    Host,
    /// `c1`, `c2`, …
    Client(u8),
    /// Every member: only `wait` and `stand` take it.
    All,
}

impl Peer {
    /// `host`, `c1`: what the scenario syntax and the port files call it.
    pub fn name(self) -> String {
        match self {
            Peer::Host => "host".to_owned(),
            Peer::Client(n) => format!("c{n}"),
            Peer::All => "all".to_owned(),
        }
    }
}

impl fmt::Display for Peer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

impl FromStr for Peer {
    type Err = String;

    fn from_str(text: &str) -> Result<Peer, String> {
        match text {
            "host" => Ok(Peer::Host),
            "all" => Ok(Peer::All),
            _ => text
                .strip_prefix('c')
                .and_then(|n| n.parse::<u8>().ok())
                .filter(|n| *n >= 1 && format!("c{n}") == text)
                .map(Peer::Client)
                .ok_or_else(|| format!("unknown peer `{text}`; use host, c1, c2 … (or all, for `wait` and `stand`)")),
        }
    }
}

/// The port file of one member beside the stand's own (`F` for the host, `F.c1` for the first
/// client): what `--port-file F` on the game wrote.
pub fn member_file(base: &Path, peer: Peer) -> PathBuf {
    match peer {
        Peer::Host | Peer::All => base.to_owned(),
        Peer::Client(n) => {
            let mut file = base.as_os_str().to_owned();
            file.push(format!(".c{n}"));
            PathBuf::from(file)
        }
    }
}

/// The clients whose port files exist beside `base`: `c1`, `c2`, … up to the first gap.
pub fn client_files(base: &Path) -> Vec<Peer> {
    (1..=u8::MAX).map(Peer::Client).take_while(|peer| member_file(base, *peer).exists()).collect()
}

const NEEDS_PORT_FILE: &str = "a stand's members are found by their port files: pass --port-file F (or SHARDS_PORT_FILE=F); \
                               `cargo sandbox --stand 2 --port 0 --port-file F` writes F for the host, F.c1 and F.c2 for the clients";

impl Cli {
    /// The stand's port file (the host's), or what to do about its absence.
    pub fn stand_base(&self) -> Result<&Path, String> {
        self.port_file.as_deref().ok_or_else(|| NEEDS_PORT_FILE.to_owned())
    }

    /// A client for one member of the stand, found by the stand's port files (a stand-wide
    /// command ignores `--port`: it names one game). Every member has a token of its own: the
    /// explicit `--token` is the host's, a client's is `SHARDS_TOKEN_C1`, `SHARDS_TOKEN_C2` …, and
    /// without them each is looked up by its port in `.shards/` from here upwards.
    pub fn client_for(&self, peer: Peer) -> Result<Client, String> {
        let port = read_port_file(&member_file(self.stand_base()?, peer))?;
        let here = std::env::current_dir().ok();
        let token = self.member_token(port, peer, |name| std::env::var(name).ok(), here.as_deref());
        Ok(Client::new(port, token).with_timeout(self.poll_timeout()))
    }

    /// The token for `peer` at `port`: see [`Cli::client_for`]. `env` reads an environment
    /// variable; token files are looked for from `from` upwards.
    pub fn member_token(&self, port: u16, peer: Peer, env: impl Fn(&str) -> Option<String>, from: Option<&Path>) -> Option<String> {
        let explicit = match peer {
            Peer::Client(n) => env(&format!("SHARDS_TOKEN_C{n}")),
            Peer::Host | Peer::All => self.token.clone(),
        };
        explicit.filter(|t| !t.is_empty()).or_else(|| find_token(port, from?))
    }

    /// A client for the game at `port` that a single command talks to, with this command line's
    /// token (`--token` names that game) and timeout.
    pub fn client_at(&self, port: u16) -> Client {
        let token = self.token.clone().or_else(|| find_token(port, &std::env::current_dir().ok()?));
        Client::new(port, token).with_timeout(self.poll_timeout())
    }

    /// The BRP port a single-game command talks to: `--port`, else the number in the port file
    /// (of the member `--peer` names), else the default port.
    pub fn port_of(&self, peer: Peer) -> Result<u16, String> {
        match (self.port, &self.port_file, peer) {
            (Some(port), _, _) => Ok(port),
            (None, Some(file), peer) => read_port_file(&member_file(file, peer)),
            (None, None, Peer::Client(n)) => {
                Err(format!("--peer c{n} needs the stand's port file: pass --port-file F (or SHARDS_PORT_FILE=F); {NEEDS_PORT_FILE}"))
            }
            (None, None, _) => Ok(crate::DEFAULT_PORT),
        }
    }
}

/// What a member that is not ready yet is waiting for: what holds its simulation, or (a client)
/// the host that has not let it in yet.
fn not_ready(status: &Value) -> Vec<String> {
    let mut reasons = still_loading(status);
    if status["net"]["session"].as_str().is_some_and(|s| s.starts_with("joining")) {
        reasons.push(format!("net: {}", status["net"]["session"].as_str().unwrap_or_default()));
    }
    reasons
}

/// Polls `ai/status` of the game `connect` finds (its port file may not exist yet) until it
/// answers and has finished loading; returns that status. Gives up at `deadline` with what is
/// wrong (no game at the port, still loading, …).
pub fn wait_member(connect: impl Fn() -> Result<Client, String>, deadline: Instant) -> Result<Value, String> {
    loop {
        let answer = connect().and_then(|client| client.call("ai/status", Value::Null));
        match answer {
            Ok(status) if not_ready(&status).is_empty() => return Ok(status),
            Ok(status) if Instant::now() >= deadline => {
                return Err(format!("the game answers but is still loading: {}", not_ready(&status).join(", ")));
            }
            Ok(_) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(250)),
            Err(e) => return Err(e),
        }
    }
}

/// How many clients the stand has: what its host says it was started with, else the port files
/// there are.
fn expected_clients(host: &Value, base: &Path) -> u8 {
    match host["net"]["stand"]["clients"].as_u64() {
        Some(n) => n.min(u64::from(u8::MAX)) as u8,
        None => client_files(base).len() as u8,
    }
}

/// `member=c1 role=client player=1 body=e23 port=41235 pid=4242 seed=42 tick=4`: one member of
/// `wait --peer all`, for scripts to grep.
pub fn member_line(name: &str, status: &Value) -> String {
    let text = |v: &Value| match v {
        Value::Null => "?".to_owned(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let net = &status["net"];
    let mut line = format!("member={name} role={}", text(&net["role"]));
    if net["player"].is_number() {
        line.push_str(&format!(" player={}", text(&net["player"])));
    }
    if net["body"].is_string() {
        line.push_str(&format!(" body={}", text(&net["body"])));
    }
    line.push_str(&format!(
        " port={} pid={} seed={} tick={}",
        text(&status["instance"]["port"]),
        text(&status["instance"]["pid"]),
        text(&status["seed"]),
        text(&status["tick"])
    ));
    if let Some(udp) = net["udp"].as_str() {
        line.push_str(&format!(" udp={udp}"));
    }
    line
}

/// The members report one stand: the same seed, a host and clients with their own player
/// numbers, and the host lists each client as the player and body it says it is.
pub fn check_same_stand(members: &[(String, Value)]) -> Result<(), String> {
    let Some(((_, host), clients)) = members.split_first() else { return Err("no member answered".into()) };
    if host["net"]["role"] != "host" {
        return Err(format!("the first member is not a host: it reports role {}", host["net"]["role"]));
    }
    for (name, status) in members {
        if status["seed"] != host["seed"] {
            return Err(format!(
                "{name} plays seed {} but the host seed {}: these games are not one stand",
                status["seed"], host["seed"]
            ));
        }
    }
    let listed: Vec<(Option<u64>, Option<&str>)> = host["net"]["peers"]
        .as_array()
        .map(|peers| peers.iter().map(|p| (p["player"].as_u64(), p["body"].as_str())).collect())
        .unwrap_or_default();
    let mut players = Vec::new();
    for (name, status) in clients {
        let net = &status["net"];
        if net["role"] != "client" {
            return Err(format!("{name} is not a client: it reports role {}", net["role"]));
        }
        let (Some(player), body) = (net["player"].as_u64(), net["body"].as_str()) else {
            return Err(format!("{name} has not joined a session yet: it reports no player ({})", net["session"]));
        };
        if players.contains(&player) {
            return Err(format!("{name} reports player {player}, which another member has too"));
        }
        players.push(player);
        if !listed.iter().any(|(p, b)| *p == Some(player) && *b == body) {
            let shown: Vec<String> = listed.iter().map(|(p, b)| format!("p{} {}", p.unwrap_or(0), b.unwrap_or("?"))).collect();
            return Err(format!(
                "{name} reports player {player} (body {}) but the host lists: {}: they are not in one session",
                body.unwrap_or("?"),
                if shown.is_empty() { "no players".to_owned() } else { shown.join(", ") }
            ));
        }
    }
    Ok(())
}

/// Waits for the host and every client to be ready, then until they are one stand, and returns
/// their statuses (host first). A UDP stand's host is ready long before its clients (processes
/// of their own) have joined and lists no players then, so the verdict is on fresh statuses of
/// all members, asked again until the host lists every client or `deadline` passes.
pub fn settle(cli: &Cli, deadline: Instant) -> Result<Vec<(String, Value)>, String> {
    let base = cli.stand_base()?;
    let host = wait_member(|| cli.client_for(Peer::Host), deadline)?;
    let clients = expected_clients(&host, base);
    for n in 1..=clients {
        wait_member(|| cli.client_for(Peer::Client(n)), deadline).map_err(|e| format!("c{n}: {e}"))?;
    }
    loop {
        let read = || -> Result<Vec<(String, Value)>, String> {
            let mut members = vec![("host".to_owned(), cli.client_for(Peer::Host)?.call("ai/status", Value::Null)?)];
            for n in 1..=clients {
                let status = cli.client_for(Peer::Client(n)).and_then(|c| c.call("ai/status", Value::Null)).map_err(|e| format!("c{n}: {e}"))?;
                members.push((format!("c{n}"), status));
            }
            Ok(members)
        };
        let verdict = read().and_then(|members| check_same_stand(&members).map(|()| members));
        match verdict {
            Ok(members) => return Ok(members),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// What `wait --peer all` prints: a line per member, then the stand.
pub fn report_lines(members: &[(String, Value)]) -> Vec<String> {
    let clients = members.len().saturating_sub(1);
    let mut lines: Vec<String> = members.iter().map(|(name, status)| member_line(name, status)).collect();
    let seed = members.first().map_or(Value::Null, |(_, host)| host["seed"].clone());
    lines.push(format!("stand: host + {clients} client{} · seed {seed} · one session", if clients == 1 { "" } else { "s" }));
    lines
}

/// `shards wait --peer all`: waits for the host and every client, prints a line per member and
/// checks that they are one stand.
pub fn wait_all(cli: &Cli, timeout: Duration) -> Result<(), String> {
    let members = settle(cli, Instant::now() + timeout)?;
    for line in report_lines(&members) {
        say(line);
    }
    Ok(())
}

/// One row of `shards stand`.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub member: String,
    pub role: String,
    pub player: Option<u64>,
    pub body: Option<String>,
    pub port: Option<u64>,
    pub ping_ms: Option<f64>,
    pub desyncs: Option<u64>,
    pub tick: Option<u64>,
    /// Why the member could not be read.
    pub error: Option<String>,
}

impl Row {
    fn from_replies(member: &str, status: &Value, net: &Value) -> Row {
        let state = &status["net"];
        let text = |v: &Value| v.as_str().map(str::to_owned);
        Row {
            member: member.to_owned(),
            role: text(&state["role"]).unwrap_or_else(|| "?".into()),
            player: state["player"].as_u64(),
            body: text(&state["body"]),
            port: status["instance"]["port"].as_u64(),
            // A client's round trip; the host has one per client.
            ping_ms: net["rtt_ms"].as_f64(),
            desyncs: state["desyncs"].as_u64().or_else(|| net["desyncs"].as_u64()),
            tick: status["tick"].as_u64(),
            error: None,
        }
    }

    fn unreachable(member: &str, why: String) -> Row {
        Row {
            member: member.to_owned(),
            role: "?".into(),
            player: None,
            body: None,
            port: None,
            ping_ms: None,
            desyncs: None,
            tick: None,
            error: Some(why),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "member": self.member, "role": self.role, "player": self.player, "body": self.body, "port": self.port,
            "ping_ms": self.ping_ms, "desyncs": self.desyncs, "tick": self.tick, "error": self.error,
        })
    }
}

/// The members as an aligned table: role, player, body, port, ping, desyncs, tick.
pub fn table(rows: &[Row]) -> Vec<String> {
    let dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
    let mut cells: Vec<[String; 8]> = vec![["MEMBER", "ROLE", "PLAYER", "BODY", "PORT", "PING", "DESYNCS", "TICK"].map(str::to_owned)];
    for row in rows {
        cells.push([
            row.member.clone(),
            row.role.clone(),
            dash(row.player.map(|p| p.to_string())),
            dash(row.body.clone()),
            dash(row.port.map(|p| p.to_string())),
            dash(row.ping_ms.map(|ms| format!("{ms:.0} ms"))),
            dash(row.desyncs.map(|d| d.to_string())),
            dash(row.tick.map(|t| t.to_string())),
        ]);
    }
    let widths: Vec<usize> = (0..8).map(|col| cells.iter().map(|row| row[col].chars().count()).max().unwrap_or(0)).collect();
    let mut lines: Vec<String> = cells
        .iter()
        .map(|row| {
            let line: Vec<String> = row.iter().zip(&widths).map(|(cell, width)| format!("{cell:<width$}")).collect();
            line.join("  ").trim_end().to_owned()
        })
        .collect();
    for row in rows {
        if let Some(why) = &row.error {
            lines.push(format!("{}: {why}", row.member));
        }
    }
    lines
}

/// A member of the stand: its client, or why it has none (its port file is gone: it has left).
struct Member {
    name: String,
    client: Result<Client, String>,
}

/// The stand's members, host first, and the host's `ai/status`. Only the host must be there: a
/// client that has left (its process ended and removed its port file) is a member without a
/// client, which `stand` shows as missing and `stand stop` goes on without.
fn members_of(cli: &Cli) -> Result<(Vec<Member>, Value), String> {
    let base = cli.stand_base()?;
    let host = cli.client_for(Peer::Host)?;
    let status = host.call("ai/status", Value::Null)?;
    let clients = expected_clients(&status, base);
    let mut members = vec![Member { name: "host".to_owned(), client: Ok(host) }];
    for n in 1..=clients {
        members.push(Member { name: format!("c{n}"), client: cli.client_for(Peer::Client(n)) });
    }
    Ok((members, status))
}

/// `shards stand`: the members of the stand, what each plays, and how they are linked. `true`
/// when every member answered; a missing one is a row with the reason, and the command's exit
/// code says so.
pub fn show(cli: &Cli) -> Result<bool, String> {
    let (members, host) = members_of(cli)?;
    let agent = cli.agent.as_str();
    let mut rows = Vec::new();
    let mut links = Value::Null;
    for Member { name, client } in &members {
        let read = client.as_ref().map_err(Clone::clone).and_then(|client| {
            let status = client.call("ai/status", Value::Null)?;
            let net = client.act(agent, "net_status", json!({}))?;
            Ok((status, net))
        });
        match read {
            Ok((status, net)) => {
                let result = net.get("result").cloned().unwrap_or(Value::Null);
                if name == "host" {
                    links = json!({"conditions": result["conditions"], "transport": result["transport"]});
                }
                rows.push(Row::from_replies(name, &status, &result));
            }
            Err(e) => rows.push(Row::unreachable(name, e)),
        }
    }
    let complete = rows.iter().all(|row| row.error.is_none());
    if cli.json {
        let out = json!({ "seed": host["seed"], "links": links, "members": rows.iter().map(Row::to_json).collect::<Vec<_>>() });
        say(serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(complete);
    }
    let clients = members.len() - 1;
    let how = match host["net"]["stand"]["udp"].as_bool() {
        Some(true) => "UDP loopback",
        _ => "in this process",
    };
    let conditions = links["conditions"].as_str().map_or_else(String::new, |c| format!(" · links {c}"));
    say(format!("stand: host + {clients} client{} ({how}) · seed {}{conditions}", if clients == 1 { "" } else { "s" }, host["seed"]));
    for line in table(&rows) {
        say(line);
    }
    Ok(complete)
}

/// Whether the game at `client` still answers.
fn answers(client: &Client) -> bool {
    client.call("ai/status", Value::Null).is_ok()
}

/// Waits until none of `members` answers any more, or `limit` has passed; returns those that
/// still do.
fn wait_gone<'a>(members: &'a [(String, &Client)], limit: Duration) -> Vec<&'a str> {
    let started = Instant::now();
    loop {
        let alive: Vec<&str> = members.iter().filter(|(_, client)| answers(client)).map(|(name, _)| name.as_str()).collect();
        if alive.is_empty() || started.elapsed() >= limit {
            return alive;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Ends the process `pid` the polite way: SIGTERM, which the game turns into a normal exit (the
/// token and port files go away with it), never `kill -9`.
#[cfg(unix)]
pub fn terminate(pid: u32) -> Result<(), String> {
    if pid <= 1 || pid == std::process::id() {
        return Err(format!("refusing to signal pid {pid}: it is not a game's"));
    }
    // SAFETY: `kill` with a pid and a signal number takes no pointers.
    match unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } {
        0 => Ok(()),
        _ => Err(format!("cannot signal pid {pid}: {}", std::io::Error::last_os_error())),
    }
}

#[cfg(not(unix))]
pub fn terminate(pid: u32) -> Result<(), String> {
    Err(format!("`shards stand stop` signals processes on unix only; end pid {pid} yourself"))
}

/// `shards stand stop`: the host ends the session (every client is told and keeps its
/// character), then the stand's process gets SIGTERM, and the command waits until no member
/// answers. A stand of child processes (`--stand-udp`) is ended by its host; a child that is
/// still there afterwards is signalled too.
///
/// A client that is already gone (its port file removed, or it does not answer) is reported
/// and left out; the rest of the stand is stopped all the same. Returns `false` when a member
/// was missing, for the exit code.
pub fn stop(cli: &Cli, kill: &dyn Fn(u32) -> Result<(), String>) -> Result<bool, String> {
    let (members, host) = members_of(cli)?;
    let agent = cli.agent.as_str();
    // Who is here to stop (name, client, pid), and who was not.
    let mut present: Vec<(String, &Client, u32)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for Member { name, client } in &members {
        let status = match client {
            Ok(client) if name == "host" => Ok((client, host.clone())),
            Ok(client) => client.call("ai/status", Value::Null).map(|status| (client, status)),
            Err(e) => Err(e.clone()),
        };
        match status {
            Ok((client, status)) => match status["instance"]["pid"].as_u64() {
                Some(pid) => present.push((name.clone(), client, pid as u32)),
                None => missing.push(format!("{name}: it reports no pid, so it is not a Shards game")),
            },
            Err(why) => missing.push(format!("{name}: {why}")),
        }
    }
    let Some(&(_, host_client, host_pid)) = present.first().filter(|(name, ..)| name == "host") else {
        return Err("the host reports no pid: is this a Shards game?".into());
    };
    // Tell the clients first: their characters are saved before the process goes.
    let ended = host_client.act(agent, "net_end", json!({"reason": "stand stopped"}))?;
    if ended["ok"] == Value::Bool(true) {
        say(format!("session ended: {}", ended["result"].as_str().unwrap_or("ok").trim_start_matches("session ended: ")));
    } else {
        say(format!("note: net_end: {}", ended["result"].as_str().unwrap_or("refused")));
    }
    kill(host_pid)?;
    let answering: Vec<(String, &Client)> = present.iter().map(|(name, client, _)| (name.clone(), *client)).collect();
    let mut alive = wait_gone(&answering, cli.timeout_or(15));
    if !alive.is_empty() {
        // Children of a UDP stand that outlived their host: each has its own process.
        for (name, _, pid) in &present {
            if *pid != host_pid && alive.contains(&name.as_str()) {
                kill(*pid)?;
            }
        }
        alive = wait_gone(&answering, Duration::from_secs(cli.timeout_or(15).as_secs().min(5)));
    }
    if !alive.is_empty() {
        return Err(format!(
            "{} still answer{} after SIGTERM: end them yourself (`kill PID`, not -9, so they remove their port files)",
            alive.join(", "),
            if alive.len() == 1 { "s" } else { "" }
        ));
    }
    let list: Vec<String> = present.iter().map(|(name, _, pid)| format!("{name} pid {pid}")).collect();
    say(format!("stand stopped: {}", list.join(", ")));
    for why in &missing {
        say(format!("missing: {why}"));
    }
    Ok(missing.is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::fake::{parse as cli, reply, serve};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shards_peer_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn peers_are_the_host_clients_or_all() {
        assert_eq!("host".parse(), Ok(Peer::Host));
        assert_eq!("c1".parse(), Ok(Peer::Client(1)));
        assert_eq!("c12".parse(), Ok(Peer::Client(12)));
        assert_eq!("all".parse(), Ok(Peer::All));
        for bad in ["c0", "c01", "c", "client1", "C1", "h", ""] {
            let e = bad.parse::<Peer>().unwrap_err();
            assert_eq!(e, format!("unknown peer `{bad}`; use host, c1, c2 … (or all, for `wait` and `stand`)"));
        }
        assert_eq!(Peer::Client(2).to_string(), "c2");
    }

    #[test]
    fn a_peer_is_found_by_its_port_file() {
        let dir = scratch("files");
        let base = dir.join("g.port");
        for (file, port) in [("g.port", "15711"), ("g.port.c1", "15712"), ("g.port.c2", "15713")] {
            std::fs::write(dir.join(file), format!("{port}\n")).unwrap();
        }
        let path = base.display().to_string();
        assert_eq!(member_file(&base, Peer::Host), base);
        assert_eq!(member_file(&base, Peer::Client(2)), dir.join("g.port.c2"));
        assert_eq!(cli(&["--port-file", &path, "status"]).port_of(Peer::Host), Ok(15711));
        assert_eq!(cli(&["--port-file", &path, "--peer", "c1", "status"]).port(), Ok(15712));
        assert_eq!(cli(&["--port-file", &path, "--peer", "c2", "status"]).port(), Ok(15713));
        assert_eq!(cli(&["--port-file", &path, "--peer", "host", "status"]).port(), Ok(15711));
        assert_eq!(cli(&["--port-file", &path, "--peer", "c1", "--port", "15799", "status"]).port(), Ok(15799), "--port wins");
        assert_eq!(client_files(&base), [Peer::Client(1), Peer::Client(2)]);
        std::fs::remove_file(dir.join("g.port.c2")).unwrap();
        assert_eq!(client_files(&base), [Peer::Client(1)]);
        // A member whose game has not started yet.
        let e = cli(&["--port-file", &path, "--peer", "c2", "status"]).port().unwrap_err();
        assert!(e.contains("g.port.c2") && e.contains("is the game running with --port-file?"), "{e}");
        // No port file: a client cannot be found, the host is the default game.
        let e = cli(&["--peer", "c1", "status"]).port().unwrap_err();
        assert!(e.starts_with("--peer c1 needs the stand's port file: pass --port-file F"), "{e}");
        assert_eq!(cli(&["--peer", "host", "status"]).port(), Ok(crate::DEFAULT_PORT));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_peer_is_a_member_and_a_typo_is_refused() {
        assert_eq!(cli(&["--peer", "c2", "status"]).peer, Some(Peer::Client(2)));
        assert_eq!(cli(&["--peer", "all", "wait"]).peer, Some(Peer::All));
        let refused = crate::fake::try_parse(&["--peer", "c0", "status"]).err().unwrap().to_string();
        assert!(refused.contains("unknown peer `c0`"), "{refused}");
        // `SHARDS_PEER` is the same option (`every_option_has_its_variable` in main.rs).
        assert_eq!(cli(&["status"]).peer, None);
    }

    #[test]
    fn every_member_has_a_token_of_its_own() {
        let dir = scratch("tokens");
        std::fs::create_dir_all(dir.join(".shards")).unwrap();
        for (port, token) in [(15711, "host-file"), (15712, "c1-file"), (15713, "c2-file")] {
            std::fs::write(dir.join(format!(".shards/{port}.token")), format!("{token}\n")).unwrap();
        }
        // The token files are looked up from a folder upwards (the working directory).
        let from = Some(dir.as_path());
        let none = |_: &str| None;
        let with_env = |name: &str| (name == "SHARDS_TOKEN_C2").then(|| "c2-env".to_owned());
        let plain = cli(&["status"]);
        let explicit = cli(&["--token", "host-explicit", "status"]);
        let found = [
            // No explicit token: each member's own file.
            plain.member_token(15711, Peer::Host, none, from),
            plain.member_token(15712, Peer::Client(1), none, from),
            plain.member_token(15713, Peer::Client(2), none, from),
            // An explicit token is the host's: the clients keep looking for their own.
            explicit.member_token(15711, Peer::Host, none, from),
            explicit.member_token(15712, Peer::Client(1), none, from),
            // A client's own variable beats its file, and only for that client.
            explicit.member_token(15713, Peer::Client(2), with_env, from),
            explicit.member_token(15712, Peer::Client(1), with_env, from),
            // Nothing found: none.
            plain.member_token(15799, Peer::Client(1), none, from),
        ];
        let found: Vec<Option<&str>> = found.iter().map(Option::as_deref).collect();
        assert_eq!(
            found,
            [
                Some("host-file"),
                Some("c1-file"),
                Some("c2-file"),
                Some("host-explicit"),
                Some("c1-file"),
                Some("c2-env"),
                Some("c1-file"),
                None
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn net(role: &str, member: &str, player: Option<u64>, body: Option<&str>) -> Value {
        let mut net = json!({"role": role, "member": member, "session": "connected"});
        if let Some(p) = player {
            net["player"] = json!(p);
        }
        if let Some(b) = body {
            net["body"] = json!(b);
        }
        net
    }

    fn status(port: u64, seed: u64, net: Value) -> Value {
        json!({"tick": 4, "seed": seed, "instance": {"pid": 4242, "port": port}, "loading": [], "net": net})
    }

    fn stand_of_two() -> Vec<(String, Value)> {
        let mut host = net("host", "host", Some(0), Some("e5"));
        host["peers"] = json!([{"player": 1, "name": "c1", "body": "e15"}, {"player": 2, "name": "c2", "body": "e16"}]);
        host["stand"] = json!({"clients": 2, "udp": false});
        vec![
            ("host".into(), status(15711, 42, host)),
            ("c1".into(), status(15712, 42, net("client", "c1", Some(1), Some("e15")))),
            ("c2".into(), status(15713, 42, net("client", "c2", Some(2), Some("e16")))),
        ]
    }

    #[test]
    fn a_member_line_names_role_player_body_and_port() {
        let members = stand_of_two();
        assert_eq!(member_line("host", &members[0].1), "member=host role=host player=0 body=e5 port=15711 pid=4242 seed=42 tick=4");
        assert_eq!(member_line("c1", &members[1].1), "member=c1 role=client player=1 body=e15 port=15712 pid=4242 seed=42 tick=4");
        let mut udp = members[0].1.clone();
        udp["net"]["udp"] = json!("0.0.0.0:54321");
        assert!(member_line("host", &udp).ends_with(" udp=0.0.0.0:54321"));
        let single = json!({"tick": 0, "seed": 1, "instance": {"pid": 1, "port": 2}, "net": {"role": "single"}});
        assert_eq!(member_line("host", &single), "member=host role=single port=2 pid=1 seed=1 tick=0");
    }

    #[test]
    fn the_members_must_report_one_stand() {
        let members = stand_of_two();
        assert_eq!(check_same_stand(&members), Ok(()));

        let mut other_seed = members.clone();
        other_seed[2].1["seed"] = json!(7);
        assert_eq!(check_same_stand(&other_seed).unwrap_err(), "c2 plays seed 7 but the host seed 42: these games are not one stand");

        let mut wrong_body = members.clone();
        wrong_body[1].1["net"]["body"] = json!("e99");
        let e = check_same_stand(&wrong_body).unwrap_err();
        assert!(e.starts_with("c1 reports player 1 (body e99) but the host lists: p1 e15, p2 e16"), "{e}");

        let mut same_player = members.clone();
        same_player[2].1["net"]["player"] = json!(1);
        assert_eq!(check_same_stand(&same_player).unwrap_err(), "c2 reports player 1, which another member has too");

        let mut joining = members.clone();
        joining[1].1["net"] = json!({"role": "client", "session": "joining: connecting to host"});
        assert!(check_same_stand(&joining).unwrap_err().starts_with("c1 has not joined a session yet"));

        let mut not_a_host = members.clone();
        not_a_host[0].1["net"]["role"] = json!("single");
        assert!(check_same_stand(&not_a_host).unwrap_err().starts_with("the first member is not a host"));
        let mut not_a_client = members.clone();
        not_a_client[1].1["net"]["role"] = json!("host");
        assert!(check_same_stand(&not_a_client).unwrap_err().starts_with("c1 is not a client"));
        assert_eq!(check_same_stand(&[]).unwrap_err(), "no member answered");
    }

    #[test]
    fn a_member_that_is_still_joining_is_not_ready() {
        let loaded = status(1, 1, net("client", "c1", Some(1), Some("e15")));
        assert!(not_ready(&loaded).is_empty());
        let joining = status(1, 1, json!({"role": "client", "session": "joining: connecting to host"}));
        assert_eq!(not_ready(&joining), ["net: joining: connecting to host"]);
        let mut loading = loaded.clone();
        loading["loading"] = json!(["net: waiting for the world"]);
        assert_eq!(not_ready(&loading), ["net: waiting for the world"]);
    }

    #[test]
    fn the_table_lines_up_and_names_what_could_not_be_read() {
        let members = stand_of_two();
        let ping = json!({"rtt_ms": 300.0});
        let rows = vec![
            Row::from_replies("host", &members[0].1, &json!({})),
            Row::from_replies("c1", &members[1].1, &ping),
            Row::unreachable("c2", "cannot reach the game at http://127.0.0.1:15713".into()),
        ];
        assert_eq!(
            table(&rows),
            [
                "MEMBER  ROLE    PLAYER  BODY  PORT   PING    DESYNCS  TICK",
                "host    host    0       e5    15711  -       -        4",
                "c1      client  1       e15   15712  300 ms  -        4",
                "c2      ?       -       -     -      -       -        -",
                "c2: cannot reach the game at http://127.0.0.1:15713",
            ]
        );
        assert_eq!(rows[1].to_json()["ping_ms"], 300.0);
    }

    /// A pretend stand: a host and two clients, each a server of its own that answers
    /// `ai/status`, `net_status` and `net_end` from `members` until `dead` is set.
    fn pretend_stand(dir: &Path, dead: Arc<AtomicBool>, ended: Arc<AtomicUsize>) {
        pretend_stand_with(dir, dead, ended, 0);
    }

    /// [`pretend_stand`] whose host lists no players in its first `late` answers to `ai/status`
    /// (a UDP stand's host, ready before its clients joined). Returns how often the host was asked.
    fn pretend_stand_with(dir: &Path, dead: Arc<AtomicBool>, ended: Arc<AtomicUsize>, late: usize) -> Arc<AtomicUsize> {
        let members = stand_of_two();
        let host_asked = Arc::new(AtomicUsize::new(0));
        for (i, (_, status)) in members.into_iter().enumerate() {
            let (dead, ended, host_asked) = (dead.clone(), ended.clone(), host_asked.clone());
            let port = serve(move |method, params| {
                if dead.load(Ordering::SeqCst) {
                    return None;
                }
                Some(match method {
                    "ai/status" if i == 0 && host_asked.fetch_add(1, Ordering::SeqCst) < late => {
                        let mut early = status.clone();
                        early["net"]["peers"] = json!([]);
                        reply(early)
                    }
                    "ai/status" => reply(status.clone()),
                    "ai/act" if params["tool"] == "net_status" => {
                        reply(json!({"tool": "net_status", "ok": true, "result": {"rtt_ms": 300.0, "conditions": "latency=150,jitter=0,loss=0.02,dup=0", "transport": "hub host"}}))
                    }
                    "ai/act" if params["tool"] == "net_end" => {
                        ended.fetch_add(1, Ordering::SeqCst);
                        reply(json!({"tool": "net_end", "ok": true, "result": "session ended: stand stopped"}))
                    }
                    other => panic!("unexpected {other} {params}"),
                })
            });
            let file = if i == 0 { dir.join("g.port") } else { dir.join(format!("g.port.c{i}")) };
            std::fs::write(file, format!("{port}\n")).unwrap();
        }
        host_asked
    }

    #[test]
    fn wait_all_waits_for_every_member_and_reports_each() {
        let dir = scratch("waitall");
        pretend_stand(&dir, Arc::new(AtomicBool::new(false)), Arc::default());
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "--peer", "all", "wait"]);
        assert_eq!(c.peer, Some(Peer::All));
        let members = settle(&c, Instant::now() + Duration::from_secs(5)).unwrap();
        // What `wait --peer all` prints: a line per member, then the stand.
        assert_eq!(
            report_lines(&members),
            [
                "member=host role=host player=0 body=e5 port=15711 pid=4242 seed=42 tick=4",
                "member=c1 role=client player=1 body=e15 port=15712 pid=4242 seed=42 tick=4",
                "member=c2 role=client player=2 body=e16 port=15713 pid=4242 seed=42 tick=4",
                "stand: host + 2 clients · seed 42 · one session",
            ]
        );
        wait_all(&c, Duration::from_secs(5)).unwrap();
        // The members are what the host says there are.
        let (members, host) = members_of(&c).unwrap();
        assert_eq!(members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["host", "c1", "c2"]);
        assert!(members.iter().all(|m| m.client.is_ok()));
        assert_eq!(host["net"]["stand"]["clients"], 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A UDP stand's host is ready long before its clients have joined: it lists no players yet.
    /// `wait --peer all` judges fresh statuses, until the host lists them all.
    #[test]
    fn wait_all_asks_again_until_the_host_lists_every_client() {
        let dir = scratch("waitlate");
        let asked = pretend_stand_with(&dir, Arc::new(AtomicBool::new(false)), Arc::default(), 3);
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "--peer", "all", "wait"]);
        let members = settle(&c, Instant::now() + Duration::from_secs(10)).expect("the host lists them in the end");
        assert_eq!(members[0].1["net"]["peers"].as_array().map(Vec::len), Some(2), "the verdict is on a fresh host status");
        assert!(asked.load(Ordering::SeqCst) >= 4, "the host was asked again after it listed nobody: {}", asked.load(Ordering::SeqCst));
        // A host that never lists them fails the wait with the mismatch, at the deadline.
        let never = scratch("waitnever");
        pretend_stand_with(&never, Arc::new(AtomicBool::new(false)), Arc::default(), usize::MAX);
        let path = never.join("g.port").display().to_string();
        let e = settle(&cli(&["--port-file", &path, "wait"]), Instant::now() + Duration::from_millis(500)).unwrap_err();
        assert!(e.starts_with("c1 reports player 1 (body e15) but the host lists: no players"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(never);
    }

    #[test]
    fn wait_all_without_a_port_file_says_where_the_members_are_written() {
        let e = wait_all(&cli(&["--peer", "all", "wait"]), Duration::from_secs(1)).unwrap_err();
        assert!(e.starts_with("a stand's members are found by their port files: pass --port-file F"), "{e}");
    }

    #[test]
    fn wait_all_gives_up_on_a_client_that_never_joins() {
        let dir = scratch("waitslow");
        pretend_stand(&dir, Arc::new(AtomicBool::new(false)), Arc::default());
        // c2 never starts: its port file is not there.
        std::fs::remove_file(dir.join("g.port.c2")).unwrap();
        let path = dir.join("g.port").display().to_string();
        let started = Instant::now();
        let e = wait_all(&cli(&["--port-file", &path, "--peer", "all", "wait"]), Duration::from_millis(600)).unwrap_err();
        assert!(e.starts_with("c2: cannot read the port file"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stand_stop_ends_the_session_then_the_process_and_waits_for_every_member() {
        let dir = scratch("stop");
        let (dead, ended) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        pretend_stand(&dir, dead.clone(), ended.clone());
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "stand", "stop"]);
        let signalled = std::sync::Mutex::new(Vec::new());
        let kill = |pid: u32| {
            signalled.lock().unwrap().push(pid);
            // The game exits: its servers stop answering.
            dead.store(true, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(stop(&c, &kill), Ok(true), "everyone was there");
        assert_eq!(ended.load(Ordering::SeqCst), 1, "the host ended the session once");
        assert_eq!(*signalled.lock().unwrap(), [4242], "the stand is one process: one signal");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stand_stop_names_the_members_that_would_not_go() {
        let dir = scratch("stuck");
        pretend_stand(&dir, Arc::new(AtomicBool::new(false)), Arc::default());
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "--timeout", "1", "stand", "stop"]);
        // Nothing is signalled for real, and nobody stops answering.
        let e = stop(&c, &|_| Ok(())).unwrap_err();
        assert_eq!(
            e,
            "host, c1, c2 still answer after SIGTERM: end them yourself (`kill PID`, not -9, so they remove their port files)"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A client that has left (its process ended and removed its port file) does not stop the
    /// rest from being stopped or shown: it is named, and the exit code says so.
    #[test]
    fn a_client_that_is_gone_is_named_and_the_rest_of_the_stand_goes_on() {
        let dir = scratch("gone");
        let (dead, ended) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        pretend_stand(&dir, dead.clone(), ended.clone());
        std::fs::remove_file(dir.join("g.port.c2")).unwrap();
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "stand"]);
        // `stand`: c2 is a row of its own, the others are read.
        assert_eq!(show(&c), Ok(false), "something is missing");
        let (members, _) = members_of(&c).unwrap();
        let why = members[2].client.as_ref().err().expect("c2 has no client");
        assert!(why.contains("g.port.c2") && why.contains("is the game running with --port-file?"), "{why}");
        assert!(members[0].client.is_ok() && members[1].client.is_ok());
        // `stand stop`: the session ends, the process gets its signal, c1 and the host are waited for.
        let signalled = std::sync::Mutex::new(Vec::new());
        let kill = |pid: u32| {
            signalled.lock().unwrap().push(pid);
            dead.store(true, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(stop(&c, &kill), Ok(false));
        assert_eq!(ended.load(Ordering::SeqCst), 1);
        assert_eq!(*signalled.lock().unwrap(), [4242]);
        // A stand with everyone there is complete.
        let whole = scratch("whole");
        pretend_stand(&whole, Arc::new(AtomicBool::new(false)), Arc::default());
        let path = whole.join("g.port").display().to_string();
        assert_eq!(show(&cli(&["--port-file", &path, "stand"])), Ok(true));
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(whole);
    }

    #[test]
    fn pids_that_are_not_games_are_not_signalled() {
        assert!(terminate(0).unwrap_err().contains("refusing"));
        assert!(terminate(1).unwrap_err().contains("refusing"));
        assert!(terminate(std::process::id()).unwrap_err().contains("refusing"));
    }
}
