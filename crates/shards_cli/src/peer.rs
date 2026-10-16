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

use crate::{find_token, read_port_file, say, still_loading, CallError, Cli, Client};

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

/// Whether `path` names a client's port file (`F.c1`, `F.c2` …): the files a stand's host writes
/// for its clients, by the suffix [`member_file`] gives them, which the host of a UDP stand also
/// clears when an earlier stand left them.
pub fn is_client_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    matches!(name.rsplit_once('.').map(|(_, suffix)| suffix.parse::<Peer>()), Some(Ok(Peer::Client(_))))
}

/// The clients whose port files exist beside `base`: `c1`, `c2`, … up to the first gap.
pub fn client_files(base: &Path) -> Vec<Peer> {
    (1..=u8::MAX).map(Peer::Client).take_while(|peer| member_file(base, *peer).exists()).collect()
}

/// What `--peer all` is told by a command that talks to one game.
pub const ALL_NEEDS_A_MEMBER: &str = "--peer all is for `wait` and `stand`; name one member: host, c1, c2";

const NEEDS_PORT_FILE: &str = "a stand's members are found by their port files: pass --port-file F (or SHARDS_PORT_FILE=F); \
                               `cargo sandbox --stand 2 --port 0 --port-file F` writes F for the host, F.c1 and F.c2 for the clients";

impl Cli {
    /// The stand's port file (the host's), or what to do about its absence.
    pub fn stand_base(&self) -> Result<&Path, String> {
        self.port_file.as_deref().ok_or_else(|| NEEDS_PORT_FILE.to_owned())
    }

    /// A client for one member of the stand, found by the stand's port files (a stand-wide
    /// command ignores `--port`: it names one game). See [`Cli::member_token`] for its token.
    pub fn client_for(&self, peer: Peer) -> Result<Client, String> {
        let port = read_port_file(&member_file(self.stand_base()?, peer))?;
        Ok(self.client_in(port, peer, &|name| std::env::var(name).ok(), std::env::current_dir().ok().as_deref()))
    }

    /// The client a single command talks to: the game `--port`, else the port file of the member
    /// `--peer` names, else the default port. A member gets its own token (see
    /// [`Cli::member_token`]), so `--peer c1 observe` works with `SHARDS_TOKEN_C1` or from the
    /// game's folder, whatever `--token` or `SHARDS_TOKEN` says about the host.
    pub fn client(&self) -> Result<Client, String> {
        self.client_with(&|name| std::env::var(name).ok(), std::env::current_dir().ok().as_deref())
    }

    /// [`Cli::client`] with the environment and the folder to look for token files from given.
    pub fn client_with(&self, env: &dyn Fn(&str) -> Option<String>, from: Option<&Path>) -> Result<Client, String> {
        self.client_as(self.member(), env, from)
    }

    /// The client for the game of `peer`, found like [`Cli::client`] finds the game `--peer` names.
    fn client_as(&self, peer: Peer, env: &dyn Fn(&str) -> Option<String>, from: Option<&Path>) -> Result<Client, String> {
        Ok(self.client_in(self.port_of(peer)?, peer, env, from))
    }

    /// The client for the stand's host (or a game on its own), whatever `--peer` says: `--port`, else
    /// the host's port file, else the default port. For a stand scenario, which names its members
    /// itself.
    pub fn host_client(&self) -> Result<Client, String> {
        self.client_as(Peer::Host, &|name| std::env::var(name).ok(), std::env::current_dir().ok().as_deref())
    }

    /// The member a single command talks to: the one `--peer` names. `--port` names a game, not a
    /// member, so with it the game is the host's: its token is the one given, else its file.
    pub fn member(&self) -> Peer {
        if self.port.is_some() { Peer::Host } else { self.peer.unwrap_or(Peer::Host) }
    }

    fn client_in(&self, port: u16, peer: Peer, env: &dyn Fn(&str) -> Option<String>, from: Option<&Path>) -> Client {
        Client::new(port, self.member_token(port, peer, env, from)).of(peer).with_timeout(self.poll_timeout())
    }

    /// The token for `peer` at `port`. Every member of a stand has a token of its own. The
    /// host's is the explicit `--token` (`SHARDS_TOKEN`), else `.shards/<port>.token`, looked up
    /// from `from` upwards. A client's is `SHARDS_TOKEN_C1`, `SHARDS_TOKEN_C2` …, else its file,
    /// and the explicit `--token` only as the last resort (it is the host's, so it works for a
    /// client only when the user gave the client's on purpose). `env` reads an environment
    /// variable.
    pub fn member_token(&self, port: u16, peer: Peer, env: impl Fn(&str) -> Option<String>, from: Option<&Path>) -> Option<String> {
        let given = |token: Option<String>| token.filter(|t| !t.is_empty());
        let file = || find_token(port, from?);
        match peer {
            Peer::Client(n) => given(env(&format!("SHARDS_TOKEN_C{n}"))).or_else(file).or_else(|| given(self.token.clone())),
            Peer::Host | Peer::All => given(self.token.clone()).or_else(file),
        }
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

/// How one poll of a member went.
enum Look {
    /// It answers and has finished loading.
    Ready(Value),
    /// Not yet (no port file, no answer, still loading, joining): asking again may help.
    Waiting(String),
    /// It was there and is not any more: waiting does not bring it back.
    Gone(String),
    /// It refused the token twice in a row: polling does not mend that.
    Refused(String),
}

/// What polling remembers of one member: that it answered once (so a silence after that is its
/// end, not its start), that it refused the token (once is allowed to be a token file that is
/// not written yet), and, for the host of a stand or a game on its own, that its port file was
/// read (a game that is there and writes its file first, and whose file is gone later, has ended,
/// whether or not it ever answered).
#[derive(Default)]
struct Watch {
    seen: bool,
    refused: bool,
    filed: bool,
    /// Whether a port file that was read and is gone means the game ended. Not for a client: the
    /// host of a UDP stand removes a client's file left by an earlier stand just after `wait` may
    /// have read it.
    host: bool,
}

impl Watch {
    /// A watch on a game; `host`: it is a host's (or a game on its own), not a client's.
    fn new(host: bool) -> Self {
        Self { host, ..Self::default() }
    }

    /// Asks `ai/status` of the game `connect` finds (its port file may not exist yet).
    fn look(&mut self, connect: &dyn Fn() -> Result<Client, String>) -> Look {
        let answer = match connect() {
            Ok(client) => {
                self.filed = true;
                client.try_call("ai/status", Value::Null)
            }
            Err(e) if self.filed && self.host => {
                return Look::Gone(format!("the game's port file is gone ({e}): its process ended; its own output says why"));
            }
            Err(e) => Err(CallError::Unreachable(e)),
        };
        match answer {
            Ok(status) => {
                self.seen = true;
                self.refused = false;
                let reasons = not_ready(&status);
                if reasons.is_empty() {
                    Look::Ready(status)
                } else {
                    Look::Waiting(format!("the game answers but is still loading: {}", reasons.join(", ")))
                }
            }
            Err(CallError::Unauthorized(e)) if std::mem::replace(&mut self.refused, true) => Look::Refused(e),
            Err(CallError::Unreachable(e)) if self.seen => Look::Gone(format!("the game was there and is gone ({e}); its own output says why")),
            Err(CallError::Unauthorized(e) | CallError::Unreachable(e) | CallError::Other(e)) => Look::Waiting(e),
        }
    }
}

/// How long to wait between two polls.
const WAIT_EVERY: Duration = Duration::from_millis(100);

/// Polls `ai/status` of the game `connect` finds (its port file may not exist yet) until it
/// answers and has finished loading; returns that status. Gives up at `deadline` with what is
/// wrong (no game at the port, still loading, …), and at once when waiting cannot help: the
/// game answered and is gone (or, for a host, its port file was read and is gone), or refuses the
/// token. `host`: the game is a host's (or a game on its own), not a client's.
pub fn wait_member(connect: impl Fn() -> Result<Client, String>, deadline: Instant, host: bool) -> Result<Value, String> {
    let mut watch = Watch::new(host);
    loop {
        match watch.look(&connect) {
            Look::Ready(status) => return Ok(status),
            Look::Gone(why) | Look::Refused(why) => return Err(why),
            Look::Waiting(why) if Instant::now() >= deadline => return Err(why),
            Look::Waiting(_) => std::thread::sleep(WAIT_EVERY),
        }
    }
}

/// How many clients the stand has: what its host says it was started with, else the port files
/// there are beside `base` (a host that was not started as a stand knows no clients).
pub fn expected_clients(host: &Value, base: Option<&Path>) -> u8 {
    match host["net"]["stand"]["clients"].as_u64() {
        Some(n) => n.min(u64::from(u8::MAX)) as u8,
        None => base.map_or(0, |base| client_files(base).len() as u8),
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

/// Why the members do not report one stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// The stand is still forming: a client has not joined, the host does not list it yet.
    /// Asking again may help.
    Forming(String),
    /// These are not one stand, or a game is not what its place in the stand says (a role, a
    /// seed, a player number). Asking again does not change that.
    Wrong(String),
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (Mismatch::Forming(why) | Mismatch::Wrong(why)) = self;
        f.write_str(why)
    }
}

/// The members report one stand: the same seed, a host and clients with their own player
/// numbers, and the host lists each client as the player and body it says it is.
pub fn check_same_stand(members: &[(String, Value)]) -> Result<(), Mismatch> {
    let Some(((_, host), clients)) = members.split_first() else { return Err(Mismatch::Forming("no member answered".into())) };
    if host["net"]["role"] != "host" {
        return Err(Mismatch::Wrong(format!("the first member is not a host: it reports role {}", host["net"]["role"])));
    }
    for (name, status) in members {
        if status["seed"] != host["seed"] {
            return Err(Mismatch::Wrong(format!(
                "{name} plays seed {} but the host seed {}: these games are not one stand",
                status["seed"], host["seed"]
            )));
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
            return Err(Mismatch::Wrong(format!("{name} is not a client: it reports role {}", net["role"])));
        }
        let (Some(player), body) = (net["player"].as_u64(), net["body"].as_str()) else {
            return Err(Mismatch::Forming(format!("{name} has not joined a session yet: it reports no player ({})", net["session"])));
        };
        if players.contains(&player) {
            return Err(Mismatch::Wrong(format!("{name} reports player {player}, which another member has too")));
        }
        players.push(player);
        if !listed.iter().any(|(p, b)| *p == Some(player) && *b == body) {
            let shown: Vec<String> = listed.iter().map(|(p, b)| format!("p{} {}", p.unwrap_or(0), b.unwrap_or("?"))).collect();
            return Err(Mismatch::Forming(format!(
                "{name} reports player {player} (body {}) but the host lists: {}: they are not in one session",
                body.unwrap_or("?"),
                if shown.is_empty() { "no players".to_owned() } else { shown.join(", ") }
            )));
        }
    }
    Ok(())
}

/// How one round of [`settle`] went.
enum Round {
    /// Every member answered, loaded: their statuses (host first), judged next.
    Answered(Vec<(String, Value)>),
    /// Not all there yet.
    Waiting(String),
    /// Waiting cannot help: a member refuses the token, or the stand is over.
    Failed(String),
}

/// Asks the host, then each client the host says the stand has. The host comes first: when it
/// was there and is gone, the stand is over, whatever the clients still answer.
fn round(cli: &Cli, base: &Path, host: &mut Watch, clients: &mut Vec<Watch>) -> Round {
    let status = match host.look(&|| cli.client_for(Peer::Host)) {
        Look::Ready(status) => status,
        Look::Waiting(why) => return Round::Waiting(why),
        Look::Gone(why) => return Round::Failed(format!("the host ended the stand: {why}")),
        Look::Refused(why) => return Round::Failed(why),
    };
    clients.resize_with(usize::from(expected_clients(&status, Some(base))), || Watch::new(false));
    let mut members = vec![("host".to_owned(), status)];
    let mut waiting = None;
    for (i, watch) in clients.iter_mut().enumerate() {
        let n = (i + 1) as u8;
        match watch.look(&|| cli.client_for(Peer::Client(n))) {
            Look::Ready(status) => members.push((format!("c{n}"), status)),
            Look::Waiting(why) => {
                waiting.get_or_insert(format!("c{n}: {why}"));
            }
            Look::Gone(why) | Look::Refused(why) => return Round::Failed(format!("c{n}: {why}")),
        }
    }
    match waiting {
        Some(why) => Round::Waiting(why),
        None => Round::Answered(members),
    }
}

/// Waits for the host and every client to be ready, then until they are one stand, and returns
/// their statuses (host first). A UDP stand's host is ready long before its clients (processes
/// of their own) have joined and lists no players then, so the verdict is on fresh statuses of
/// all members, asked again until the host lists every client or `deadline` passes.
///
/// It gives up before the deadline when waiting cannot help: the host or a client was there and
/// is gone (the host ended the stand), a member refuses its token, or the members are
/// not one stand and say so twice in a row (a game started without `--stand`, another stand's
/// leftover port file).
pub fn settle(cli: &Cli, deadline: Instant) -> Result<Vec<(String, Value)>, String> {
    let base = cli.stand_base()?;
    let (mut host, mut clients) = (Watch::new(true), Vec::new());
    let mut wrong: Option<String> = None;
    loop {
        let why = match round(cli, base, &mut host, &mut clients) {
            Round::Failed(why) => return Err(why),
            Round::Waiting(why) => {
                wrong = None;
                why
            }
            Round::Answered(members) => match check_same_stand(&members) {
                Ok(()) => return Ok(members),
                Err(Mismatch::Forming(why)) => {
                    wrong = None;
                    why
                }
                Err(Mismatch::Wrong(why)) => {
                    if wrong.as_ref() == Some(&why) {
                        return Err(why);
                    }
                    wrong = Some(why.clone());
                    why
                }
            },
        };
        if Instant::now() >= deadline {
            return Err(why);
        }
        std::thread::sleep(WAIT_EVERY);
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
    let clients = expected_clients(&status, Some(base));
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

/// Ends the game at `client`, process `pid`, the polite way: its `quit` tool, which ends it as
/// Ctrl+C does (its token and port file go with it), on every platform. A game from before
/// `quit` gets SIGTERM on unix, which it turns into the same exit; never `kill -9`.
pub fn end_game(client: &Client, agent: &str, pid: u32) -> Result<(), String> {
    let out = client.act(agent, "quit", json!({}))?;
    if out["ok"] == Value::Bool(true) {
        return Ok(());
    }
    let why = out["result"].as_str().unwrap_or("refused");
    if why.starts_with("unknown tool `quit`") {
        return terminate(pid);
    }
    Err(format!("pid {pid} would not quit: {why}"))
}

/// Ends the process `pid` with SIGTERM, which the game turns into a normal exit (the token and
/// port files go away with it), never `kill -9`.
pub fn terminate(pid: u32) -> Result<(), String> {
    if pid <= 1 || pid == std::process::id() {
        return Err(format!("refusing to signal pid {pid}: it is not a game's"));
    }
    signal(pid)
}

#[cfg(unix)]
fn signal(pid: u32) -> Result<(), String> {
    // SAFETY: `kill` with a pid and a signal number takes no pointers.
    match unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) } {
        0 => Ok(()),
        _ => Err(format!("cannot signal pid {pid}: {}", std::io::Error::last_os_error())),
    }
}

#[cfg(not(unix))]
fn signal(pid: u32) -> Result<(), String> {
    Err(format!("pid {pid} has no `quit`, and Windows has no signal for another console's process: close its window"))
}

/// Whether a process with this pid runs: `None` when that cannot be found out here.
#[cfg(unix)]
pub fn running(pid: u32) -> Option<bool> {
    // 0 would ask about this process's group.
    let pid = libc::pid_t::try_from(pid).ok().filter(|&pid| pid > 0)?;
    // SAFETY: `kill` with a pid and signal 0 takes no pointers and sends nothing.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Some(true);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Some(false),
        // There, but another user's.
        Some(libc::EPERM) => Some(true),
        _ => None,
    }
}

/// `std` cannot ask Windows about another process: `tasklist` lists it while it runs.
#[cfg(windows)]
pub fn running(pid: u32) -> Option<bool> {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    // `"sandbox.exe","4242","Console","1","123,456 K"`, or a line saying no task matches.
    let field = format!("\",\"{pid}\",\"");
    Some(String::from_utf8_lossy(&out.stdout).lines().any(|line| line.contains(&field)))
}

#[cfg(not(any(unix, windows)))]
pub fn running(_pid: u32) -> Option<bool> {
    None
}

/// `shards stand stop`: the host ends the session (every client is told and keeps its
/// character), then the host is asked to quit (`end`: [`end_game`]), and the command waits until
/// no member answers. A stand of child processes (`--stand-udp`) is ended by its host; a child
/// that is still there afterwards is asked too.
///
/// A client that is already gone (its port file removed, or it does not answer) is reported
/// and left out; the rest of the stand is stopped all the same. Returns `false` when a member
/// was missing, for the exit code.
pub fn stop(cli: &Cli, end: &dyn Fn(&Client, u32) -> Result<(), String>) -> Result<bool, String> {
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
    end(host_client, host_pid)?;
    let answering: Vec<(String, &Client)> = present.iter().map(|(name, client, _)| (name.clone(), *client)).collect();
    let mut alive = wait_gone(&answering, cli.timeout_or(15));
    if !alive.is_empty() {
        // Children of a UDP stand that outlived their host: each has its own process.
        for (name, client, pid) in &present {
            if *pid != host_pid && alive.contains(&name.as_str()) {
                end(client, *pid)?;
            }
        }
        alive = wait_gone(&answering, Duration::from_secs(cli.timeout_or(15).as_secs().min(5)));
    }
    if !alive.is_empty() {
        return Err(format!(
            "{} still answer{} after being asked to quit: end them yourself (`shards quit` with their port file, or \
             `kill PID` on unix, not -9, so they remove their port files)",
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
    use std::sync::{Arc, Mutex};

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
        // What can change as the stand forms, and what never will.
        let forming = |m: &[(String, Value)]| match check_same_stand(m).unwrap_err() {
            Mismatch::Forming(why) => why,
            Mismatch::Wrong(why) => panic!("expected a stand that is still forming: {why}"),
        };
        let wrong = |m: &[(String, Value)]| match check_same_stand(m).unwrap_err() {
            Mismatch::Wrong(why) => why,
            Mismatch::Forming(why) => panic!("expected a stand that is wrong: {why}"),
        };

        let mut other_seed = members.clone();
        other_seed[2].1["seed"] = json!(7);
        assert_eq!(wrong(&other_seed), "c2 plays seed 7 but the host seed 42: these games are not one stand");

        let mut wrong_body = members.clone();
        wrong_body[1].1["net"]["body"] = json!("e99");
        let e = forming(&wrong_body);
        assert!(e.starts_with("c1 reports player 1 (body e99) but the host lists: p1 e15, p2 e16"), "{e}");

        let mut same_player = members.clone();
        same_player[2].1["net"]["player"] = json!(1);
        assert_eq!(wrong(&same_player), "c2 reports player 1, which another member has too");

        let mut joining = members.clone();
        joining[1].1["net"] = json!({"role": "client", "session": "joining: connecting to host"});
        assert!(forming(&joining).starts_with("c1 has not joined a session yet"));

        let mut unlisted = members.clone();
        unlisted[0].1["net"]["peers"] = json!([]);
        assert!(forming(&unlisted).contains("but the host lists: no players"));

        let mut not_a_host = members.clone();
        not_a_host[0].1["net"]["role"] = json!("single");
        assert!(wrong(&not_a_host).starts_with("the first member is not a host"));
        let mut not_a_client = members.clone();
        not_a_client[1].1["net"]["role"] = json!("host");
        assert!(wrong(&not_a_client).starts_with("c1 is not a client"));
        assert_eq!(forming(&[]), "no member answered");
        assert_eq!(Mismatch::Wrong("x".into()).to_string(), "x");
    }

    #[test]
    fn a_clients_port_file_is_told_by_its_suffix() {
        for client in ["g.port.c1", "g.port.c12", "/tmp/x/F.c255", "F.c2"] {
            assert!(is_client_file(Path::new(client)), "{client}");
        }
        // The host's own, and names that the host of a stand does not clear for a client.
        for other in ["g.port", "g.cache", "c1", "F.c0", "F.c01", "F.c256", "F.c1.part", "F.c", "F.cx", ""] {
            assert!(!is_client_file(Path::new(other)), "{other}");
        }
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

    type Tokens = Arc<Mutex<Vec<Option<String>>>>;

    /// A pretend game that answers `ai/status` with `status` `answers` times, then goes away (it
    /// drops every connection). It records the token of each call it gets.
    fn pretend_member(status: Value, answers: usize, tokens: Tokens) -> u16 {
        let asked = AtomicUsize::new(0);
        serve(move |method, params| {
            tokens.lock().unwrap().push(params["token"].as_str().map(str::to_owned));
            if method != "ai/status" {
                return Some(crate::fake::error(&format!("unexpected {method}")));
            }
            (asked.fetch_add(1, Ordering::SeqCst) < answers).then(|| reply(status.clone()))
        })
    }

    /// The port files of a stand: `g.port` for the first of `members`, `g.port.cN` for the rest.
    /// Each is a status and how often that game answers before it goes away.
    fn lay_out(dir: &Path, members: Vec<(Value, usize)>) {
        for (i, (status, answers)) in members.into_iter().enumerate() {
            let port = pretend_member(status, answers, Tokens::default());
            let file = if i == 0 { dir.join("g.port") } else { dir.join(format!("g.port.c{i}")) };
            std::fs::write(file, format!("{port}\n")).unwrap();
        }
    }

    #[test]
    fn a_single_command_to_a_client_carries_that_clients_token() {
        let dir = scratch("single_tokens");
        let elsewhere = scratch("single_tokens_elsewhere");
        let seen: [Tokens; 2] = Default::default();
        let members = stand_of_two();
        let ports = [0, 1].map(|i| pretend_member(members[i].1.clone(), usize::MAX, seen[i].clone()));
        std::fs::write(dir.join("g.port"), format!("{}\n", ports[0])).unwrap();
        std::fs::write(dir.join("g.port.c1"), format!("{}\n", ports[1])).unwrap();
        std::fs::create_dir_all(dir.join(".shards")).unwrap();
        std::fs::write(dir.join(format!(".shards/{}.token", ports[0])), "host-file\n").unwrap();
        std::fs::write(dir.join(format!(".shards/{}.token", ports[1])), "c1-file\n").unwrap();
        let file = dir.join("g.port").display().to_string();
        let c1_port = ports[1].to_string();
        let env = |name: &str| (name == "SHARDS_TOKEN_C1").then(|| "c1-env".to_owned());
        let no_env = |_: &str| None;
        // The token the game `which` received from `shards <args>` (`Cli::client`, what every
        // single command and `wait --peer c1` go through).
        let sent = |args: &[String], env: &dyn Fn(&str) -> Option<String>, from: &Path, which: usize| -> Option<String> {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            cli(&args).client_with(env, Some(from)).unwrap().call("ai/status", Value::Null).unwrap();
            seen[which].lock().unwrap().last().cloned().expect("the game was asked")
        };
        let c1 = |extra: &[&str]| -> Vec<String> { ["--port-file", &file, "--peer", "c1"].iter().chain(extra).chain(&["status"]).map(|s| s.to_string()).collect() };
        let host_token = c1(&["--token", "host-explicit"]);
        // A client's own variable first, then its own file; the host's token only as a last resort.
        assert_eq!(sent(&host_token, &env, &dir, 1).as_deref(), Some("c1-env"));
        assert_eq!(sent(&host_token, &no_env, &dir, 1).as_deref(), Some("c1-file"), "not the host's token");
        assert_eq!(sent(&host_token, &no_env, &elsewhere, 1).as_deref(), Some("host-explicit"));
        assert_eq!(sent(&c1(&[]), &no_env, &elsewhere, 1), None, "nothing found: no token");
        assert_eq!(sent(&c1(&[]), &env, &elsewhere, 1).as_deref(), Some("c1-env"));
        // The host: `--token` first, then its file; `SHARDS_TOKEN_C1` is not its.
        let host = |extra: &[&str]| -> Vec<String> { ["--port-file", &file].iter().chain(extra).chain(&["status"]).map(|s| s.to_string()).collect() };
        assert_eq!(sent(&host(&["--token", "host-explicit"]), &env, &dir, 0).as_deref(), Some("host-explicit"));
        assert_eq!(sent(&host(&[]), &env, &dir, 0).as_deref(), Some("host-file"));
        assert_eq!(sent(&host(&["--peer", "host"]), &env, &dir, 0).as_deref(), Some("host-file"));
        // `--port` names a game, not a member: the token is the one given, else that game's file.
        let port = |extra: &[&str]| -> Vec<String> { ["--port", &c1_port, "--peer", "c1"].iter().chain(extra).chain(&["status"]).map(|s| s.to_string()).collect() };
        assert_eq!(sent(&port(&["--token", "given"]), &env, &dir, 1).as_deref(), Some("given"));
        assert_eq!(sent(&port(&[]), &env, &dir, 1).as_deref(), Some("c1-file"));
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(elsewhere);
    }

    #[test]
    fn an_unauthorized_answer_says_which_token_to_set() {
        let port = serve(|_, _| Some(crate::fake::error("unauthorized: bad token")));
        let say = |client: Client| client.call("ai/status", Value::Null).unwrap_err();
        let host = say(Client::new(port, None));
        assert!(host.contains(&format!("no .shards/{port}.token in")) && !host.contains("SHARDS_TOKEN_C"), "{host}");
        let given = say(Client::new(port, Some("x".into())));
        assert!(given.contains("the token given is not this game's") && given.contains("SHARDS_TOKEN_C1"), "{given}");
        let c2_without = say(Client::new(port, None).of(Peer::Client(2)));
        assert!(c2_without.contains("no SHARDS_TOKEN_C2 set and no .shards/"), "{c2_without}");
        let c2_with = say(Client::new(port, Some("x".into())).of(Peer::Client(2)));
        assert!(c2_with.contains("the token sent is not c2's") && c2_with.contains("set SHARDS_TOKEN_C2"), "{c2_with}");
    }

    /// Waiting cannot help a game that refuses the token, nor one that answered and went away; a
    /// game that was never there is waited for until the deadline.
    #[test]
    fn wait_stops_at_once_when_waiting_cannot_help() {
        let long = || Instant::now() + Duration::from_secs(30);
        let started = Instant::now();
        let refusing = serve(|_, _| Some(crate::fake::error("unauthorized: bad token")));
        let e = wait_member(|| Ok(Client::new(refusing, None)), long(), true).unwrap_err();
        assert!(e.starts_with("ai/status: unauthorized: bad token"), "{e}");

        let mut loading = status(1, 1, json!({"role": "single"}));
        loading["loading"] = json!(["content: 2 files left"]);
        let leaving = pretend_member(loading, 2, Tokens::default());
        let e = wait_member(|| Ok(Client::new(leaving, None)), long(), true).unwrap_err();
        assert!(e.starts_with("the game was there and is gone (cannot reach the game at"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10), "neither waited for the deadline");

        let begun = Instant::now();
        let e = wait_member(|| Err("cannot read the port file g.port".to_owned()), Instant::now() + Duration::from_millis(300), true)
            .unwrap_err();
        assert_eq!(e, "cannot read the port file g.port");
        assert!(begun.elapsed() >= Duration::from_millis(300), "a game that never was there is waited for");
    }

    /// A host whose port file was read and is gone by the next poll has ended, even if it never
    /// answered (it lived for a few hundred milliseconds: a client that could not start ended it).
    /// A client's file read and gone is not that: the host of a UDP stand removes the leftover
    /// of an earlier stand just after `wait` may have read it, and the real client writes it again.
    #[test]
    fn a_host_whose_port_file_vanished_unanswered_has_ended_but_a_clients_file_is_waited_for() {
        let nobody = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        let reads = AtomicUsize::new(0);
        // The first read finds a file (a port nothing answers at), the later ones find none.
        let file_then_none = || {
            if reads.fetch_add(1, Ordering::SeqCst) == 0 { Ok(Client::new(nobody, None)) } else { Err("cannot read the port file g.port".to_owned()) }
        };
        let started = Instant::now();
        let e = wait_member(file_then_none, started + Duration::from_secs(30), true).unwrap_err();
        assert!(e.starts_with("the game's port file is gone (cannot read the port file g.port): its process ended"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10), "no wait for the deadline");
        // The same for a client's file: the wait goes on to the deadline and says what it cannot reach.
        reads.store(0, Ordering::SeqCst);
        let begun = Instant::now();
        let e = wait_member(file_then_none, begun + Duration::from_millis(1500), false).unwrap_err();
        assert_eq!(e, "cannot read the port file g.port");
        assert!(begun.elapsed() >= Duration::from_millis(1500));
    }

    /// `wait --peer all` fails at once on what waiting cannot mend, with the mismatch.
    #[test]
    fn wait_all_stops_at_once_on_members_that_are_not_one_stand() {
        let started = Instant::now();
        let settle_fast = |dir: &PathBuf| {
            let path = dir.join("g.port").display().to_string();
            settle(&cli(&["--port-file", &path, "--peer", "all", "wait"]), Instant::now() + Duration::from_secs(30)).unwrap_err()
        };
        // A game that was not started as a stand.
        let alone = scratch("fast_alone");
        lay_out(&alone, vec![(status(15711, 42, json!({"role": "single"})), usize::MAX)]);
        let e = settle_fast(&alone);
        assert_eq!(e, "the first member is not a host: it reports role \"single\"");
        // A port file left by another stand: its client plays another seed.
        let stale = scratch("fast_seed");
        let mut members = stand_of_two();
        members[2].1["seed"] = json!(7);
        lay_out(&stale, members.into_iter().map(|(_, status)| (status, usize::MAX)).collect());
        assert_eq!(settle_fast(&stale), "c2 plays seed 7 but the host seed 42: these games are not one stand");
        // Two clients that claim the same player.
        let twins = scratch("fast_twins");
        let mut members = stand_of_two();
        members[2].1["net"]["player"] = json!(1);
        lay_out(&twins, members.into_iter().map(|(_, status)| (status, usize::MAX)).collect());
        assert_eq!(settle_fast(&twins), "c2 reports player 1, which another member has too");
        assert!(started.elapsed() < Duration::from_secs(10), "no wait for the 30 s deadline");
        for dir in [alone, stale, twins] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// A host that answered and is gone has ended the stand: `wait --peer all` says so at once
    /// instead of waiting for a client that will never come.
    #[test]
    fn wait_all_stops_at_once_when_the_host_has_ended_the_stand() {
        let started = Instant::now();
        let dir = scratch("fast_ended");
        let members = stand_of_two();
        // c2 never starts; the host goes away after a few rounds.
        lay_out(&dir, vec![(members[0].1.clone(), 3), (members[1].1.clone(), usize::MAX)]);
        let path = dir.join("g.port").display().to_string();
        let e = settle(&cli(&["--port-file", &path, "--peer", "all", "wait"]), Instant::now() + Duration::from_secs(30)).unwrap_err();
        assert!(e.starts_with("the host ended the stand: the game was there and is gone (cannot reach the game at"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
        // A client that went away the same way is named.
        let lost = scratch("fast_lost");
        lay_out(&lost, vec![(members[0].1.clone(), usize::MAX), (members[1].1.clone(), 3)]);
        let path = lost.join("g.port").display().to_string();
        let e = settle(&cli(&["--port-file", &path, "--peer", "all", "wait"]), Instant::now() + Duration::from_secs(30)).unwrap_err();
        assert!(e.starts_with("c1: the game was there and is gone"), "{e}");
        // A client that refuses its token twice in a row is named, with the variable to set.
        let refused = scratch("fast_refused");
        lay_out(&refused, vec![(members[0].1.clone(), usize::MAX)]);
        let port = serve(|_, _| Some(crate::fake::error("unauthorized: bad token")));
        std::fs::write(refused.join("g.port.c1"), format!("{port}\n")).unwrap();
        let path = refused.join("g.port").display().to_string();
        let e = settle(&cli(&["--port-file", &path, "--peer", "all", "wait"]), Instant::now() + Duration::from_secs(30)).unwrap_err();
        assert!(e.starts_with("c1: ai/status: unauthorized: bad token") && e.contains("SHARDS_TOKEN_C1"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
        for dir in [dir, lost, refused] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// The stand's host wrote its port file, never answered, and removed the file again (it ended
    /// in its first frames: a client could not start). `wait --peer all` read the file once: it says
    /// the host ended the stand, instead of waiting for a file that will not come back.
    #[test]
    fn wait_all_stops_at_once_when_the_hosts_port_file_vanishes_unanswered() {
        let dir = scratch("vanish");
        let nobody = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(dir.join("g.port"), format!("{nobody}\n")).unwrap();
        let ended = std::thread::spawn({
            let file = dir.join("g.port");
            move || {
                std::thread::sleep(Duration::from_millis(400));
                std::fs::remove_file(file).unwrap();
            }
        });
        let path = dir.join("g.port").display().to_string();
        let started = Instant::now();
        let e = settle(&cli(&["--port-file", &path, "--peer", "all", "wait"]), started + Duration::from_secs(30)).unwrap_err();
        ended.join().unwrap();
        assert!(e.starts_with("the host ended the stand: the game's port file is gone (cannot read the port file"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A client's port file left by an earlier stand points nowhere: the wait goes on until the
    /// new client rewrites it. A file past the host's count is not a member at all.
    #[test]
    fn a_stale_port_file_is_neither_a_member_nor_a_reason_to_give_up() {
        let dir = scratch("stale");
        pretend_stand(&dir, Arc::new(AtomicBool::new(false)), Arc::default());
        // c1's file was left by a client that is gone; c3's by a bigger stand.
        let real = std::fs::read_to_string(dir.join("g.port.c1")).unwrap();
        let free = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(dir.join("g.port.c1"), format!("{free}\n")).unwrap();
        std::fs::write(dir.join("g.port.c3"), format!("{free}\n")).unwrap();
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "--peer", "all", "wait"]);
        let rewrite = std::thread::spawn({
            let file = dir.join("g.port.c1");
            move || {
                std::thread::sleep(Duration::from_millis(400));
                std::fs::write(file, real).unwrap();
            }
        });
        let members = settle(&c, Instant::now() + Duration::from_secs(20)).expect("the new c1 replaces the stale file");
        rewrite.join().unwrap();
        assert_eq!(members.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["host", "c1", "c2"], "c3 is not in this stand");
        // Without the client, the wait runs to its deadline and says what it cannot reach.
        std::fs::write(dir.join("g.port.c1"), format!("{free}\n")).unwrap();
        let e = settle(&c, Instant::now() + Duration::from_millis(400)).unwrap_err();
        assert!(e.starts_with("c1: cannot reach the game at"), "{e}");
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
        let kill = |_: &Client, pid: u32| {
            signalled.lock().unwrap().push(pid);
            // The game exits: its servers stop answering.
            dead.store(true, Ordering::SeqCst);
            Ok(())
        };
        assert_eq!(stop(&c, &kill), Ok(true), "everyone was there");
        assert_eq!(ended.load(Ordering::SeqCst), 1, "the host ended the session once");
        assert_eq!(*signalled.lock().unwrap(), [4242], "the stand is one process: asked once");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stand_stop_names_the_members_that_would_not_go() {
        let dir = scratch("stuck");
        pretend_stand(&dir, Arc::new(AtomicBool::new(false)), Arc::default());
        let path = dir.join("g.port").display().to_string();
        let c = cli(&["--port-file", &path, "--timeout", "1", "stand", "stop"]);
        // Nobody is asked for real, and nobody stops answering.
        let e = stop(&c, &|_, _| Ok(())).unwrap_err();
        assert_eq!(
            e,
            "host, c1, c2 still answer after being asked to quit: end them yourself (`shards quit` with their port file, or \
             `kill PID` on unix, not -9, so they remove their port files)"
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
        let kill = |_: &Client, pid: u32| {
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

    /// A game is asked to quit, on every platform; one that refuses says why; one from before
    /// `quit` gets the signal path (refused here for pid 1, before anything is sent).
    #[test]
    fn a_game_is_asked_to_quit() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let port = serve({
            let asked = asked.clone();
            move |method, params| {
                asked.lock().unwrap().push(format!("{method} {}", params["tool"].as_str().unwrap_or("")));
                Some(reply(json!({"ok": true, "result": "quitting: the game ends in 0.2s, after its exit hooks"})))
            }
        });
        assert_eq!(end_game(&Client::new(port, None), "cli", 4242), Ok(()));
        assert_eq!(*asked.lock().unwrap(), ["ai/act quit"]);
        let inside = serve(|_, _| Some(reply(json!({"ok": false, "result": "this game runs inside its stand's process"}))));
        assert_eq!(end_game(&Client::new(inside, None), "cli", 4242), Err("pid 4242 would not quit: this game runs inside its stand's process".into()));
        let old = serve(|_, _| Some(reply(json!({"ok": false, "result": "unknown tool `quit`; available: observe"}))));
        assert!(end_game(&Client::new(old, None), "cli", 1).unwrap_err().contains("refusing to signal pid 1"));
    }

    #[test]
    fn pids_that_are_not_games_are_not_signalled() {
        assert!(terminate(0).unwrap_err().contains("refusing"));
        assert!(terminate(1).unwrap_err().contains("refusing"));
        assert!(terminate(std::process::id()).unwrap_err().contains("refusing"));
    }
}
