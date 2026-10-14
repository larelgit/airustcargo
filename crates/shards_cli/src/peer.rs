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
    /// command ignores `--port`: it names one game).
    pub fn client_for(&self, peer: Peer) -> Result<Client, String> {
        let port = read_port_file(&member_file(self.stand_base()?, peer))?;
        Ok(self.client_at(port))
    }

    /// A client for the game at `port`, with this command line's token and timeout.
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

/// `shards wait --peer all`: waits for the host and every client, prints a line per member and
/// checks that they are one stand.
pub fn wait_all(cli: &Cli, timeout: Duration) -> Result<(), String> {
    let base = cli.stand_base()?;
    let deadline = Instant::now() + timeout;
    let host = wait_member(|| cli.client_for(Peer::Host), deadline)?;
    let clients = expected_clients(&host, base);
    let mut members = vec![("host".to_owned(), host)];
    for n in 1..=clients {
        let status = wait_member(|| cli.client_for(Peer::Client(n)), deadline).map_err(|e| format!("c{n}: {e}"))?;
        members.push((format!("c{n}"), status));
    }
    for (name, status) in &members {
        say(member_line(name, status));
    }
    check_same_stand(&members)?;
    let seed = members[0].1["seed"].clone();
    say(format!("stand: host + {clients} client{} · seed {seed} · one session", if clients == 1 { "" } else { "s" }));
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

/// The members of the stand, host first, with a client each; the host's `ai/status` too.
fn members_of(cli: &Cli) -> Result<(Vec<(String, Client)>, Value), String> {
    let base = cli.stand_base()?;
    let host = cli.client_for(Peer::Host)?;
    let status = host.call("ai/status", Value::Null)?;
    let clients = expected_clients(&status, base);
    let mut members = vec![("host".to_owned(), host)];
    for n in 1..=clients {
        members.push((format!("c{n}"), cli.client_for(Peer::Client(n))?));
    }
    Ok((members, status))
}

/// `shards stand`: the members of the stand, what each plays, and how they are linked.
pub fn show(cli: &Cli) -> Result<(), String> {
    let (members, host) = members_of(cli)?;
    let agent = cli.agent.as_str();
    let mut rows = Vec::new();
    let mut links = Value::Null;
    for (name, client) in &members {
        let read = client.call("ai/status", Value::Null).and_then(|status| {
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
    if cli.json {
        let out = json!({ "seed": host["seed"], "links": links, "members": rows.iter().map(Row::to_json).collect::<Vec<_>>() });
        say(serde_json::to_string_pretty(&out).unwrap_or_default());
        return Ok(());
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
    Ok(())
}

/// Whether the game at `client` still answers.
fn answers(client: &Client) -> bool {
    client.call("ai/status", Value::Null).is_ok()
}

/// Waits until none of `members` answers any more, or `limit` has passed; returns those that
/// still do.
fn wait_gone(members: &[(String, Client)], limit: Duration) -> Vec<&str> {
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
pub fn stop(cli: &Cli, kill: &dyn Fn(u32) -> Result<(), String>) -> Result<(), String> {
    let (members, host) = members_of(cli)?;
    let agent = cli.agent.as_str();
    let mut pids = Vec::new();
    for (name, client) in &members {
        let pid = if name == "host" { host["instance"]["pid"].as_u64() } else { client.call("ai/status", Value::Null).ok().and_then(|s| s["instance"]["pid"].as_u64()) };
        if let Some(pid) = pid {
            pids.push((name.clone(), pid as u32));
        }
    }
    let Some(&(_, host_pid)) = pids.first() else { return Err("the host reports no pid: is this a Shards game?".into()) };
    // Tell the clients first: their characters are saved before the process goes.
    let ended = members[0].1.act(agent, "net_end", json!({"reason": "stand stopped"}))?;
    if ended["ok"] == Value::Bool(true) {
        say(format!("session ended: {}", ended["result"].as_str().unwrap_or("ok").trim_start_matches("session ended: ")));
    } else {
        say(format!("note: net_end: {}", ended["result"].as_str().unwrap_or("refused")));
    }
    kill(host_pid)?;
    let mut alive = wait_gone(&members, cli.timeout_or(15));
    if !alive.is_empty() {
        // Children of a UDP stand that outlived their host: each has its own process.
        for (name, pid) in &pids {
            if *pid != host_pid && alive.contains(&name.as_str()) {
                kill(*pid)?;
            }
        }
        alive = wait_gone(&members, Duration::from_secs(cli.timeout_or(15).as_secs().min(5)));
    }
    if !alive.is_empty() {
        return Err(format!("{} still answer{} after SIGTERM: end them yourself (`kill PID`, not -9, so they remove their port files)", alive.join(", "), if alive.len() == 1 { "s" } else { "" }));
    }
    let mut list: Vec<String> = Vec::new();
    for (name, pid) in &pids {
        list.push(format!("{name} pid {pid}"));
    }
    say(format!("stand stopped: {}", list.join(", ")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use clap::Parser;

    use super::*;
    use crate::fake::{reply, serve};

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from([&["shards"], args].concat()).unwrap_or_else(|e| panic!("{e}"))
    }

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
    fn the_peer_comes_from_the_environment_too_and_a_typo_is_refused() {
        assert_eq!(cli(&["--peer", "c2", "status"]).peer, Some(Peer::Client(2)));
        let refused = Cli::try_parse_from(["shards", "--peer", "c0", "status"]).err().unwrap().to_string();
        assert!(refused.contains("unknown peer `c0`"), "{refused}");
        assert_eq!(cli(&["status"]).peer, None);
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
        let members = stand_of_two();
        for (i, (_, status)) in members.into_iter().enumerate() {
            let (dead, ended) = (dead.clone(), ended.clone());
            let port = serve(move |method, params| {
                if dead.load(Ordering::SeqCst) {
                    return None;
                }
                Some(match method {
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
    }

    #[test]
    fn wait_all_waits_for_every_member_and_checks_the_session() {
        let dir = scratch("waitall");
        pretend_stand(&dir, Arc::new(AtomicBool::new(false)), Arc::default());
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "--peer", "all", "wait"]);
        assert_eq!(c.peer, Some(Peer::All));
        wait_all(&c, Duration::from_secs(5)).unwrap();
        // The members are what the host says there are.
        let (members, host) = members_of(&c).unwrap();
        assert_eq!(members.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["host", "c1", "c2"]);
        assert_eq!(host["net"]["stand"]["clients"], 2);
        let _ = std::fs::remove_dir_all(dir);
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
        stop(&c, &kill).unwrap();
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
        assert!(e.contains("still answer after SIGTERM") || e.contains("still answers after SIGTERM"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pids_that_are_not_games_are_not_signalled() {
        assert!(terminate(0).unwrap_err().contains("refusing"));
        assert!(terminate(1).unwrap_err().contains("refusing"));
        assert!(terminate(std::process::id()).unwrap_err().contains("refusing"));
    }
}
