//! `--stand N --stand-udp` with real processes: the game starts as the UDP host, starts its
//! client as a child process of its own (own `shards` port and token, `g.port.c1`), the client
//! joins over loopback, and both go when the host does. Real time, a few seconds, nothing
//! asserted about timing.

#![cfg(all(unix, feature = "remote"))]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use shards::serde_json::{json, Value};

/// The body of an HTTP response, chunked (`<hex size>\r\n<bytes>\r\n` … `0\r\n\r\n`) or with a
/// length: read as far as it goes, not to the end of the connection.
fn read_body(reader: &mut BufReader<TcpStream>) -> Option<Vec<u8>> {
    let mut chunked = false;
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim().to_lowercase();
        if line.is_empty() {
            break;
        }
        chunked |= line.starts_with("transfer-encoding:") && line.contains("chunked");
        if let Some(n) = line.strip_prefix("content-length:") {
            length = n.trim().parse().ok()?;
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim(), 16).ok()?;
            // The chunk and the line break after it.
            let mut chunk = vec![0u8; size + 2];
            reader.read_exact(&mut chunk).ok()?;
            if size == 0 {
                return Some(body);
            }
            body.extend_from_slice(&chunk[..size]);
        }
    }
    body.resize(length, 0);
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

/// A UDP stand in a folder of its own, ended with the test.
struct UdpStand {
    dir: PathBuf,
    host: Child,
}

impl UdpStand {
    fn start(clients: u8) -> UdpStand {
        UdpStand::start_after(clients, &[])
    }

    /// [`UdpStand::start`] in a folder that already holds the port files `stale` names: what an
    /// earlier stand left when it was killed.
    fn start_after(clients: u8, stale: &[&str]) -> UdpStand {
        // A folder per stand: tests of the same size run side by side, in one process.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("sandbox-stand-udp-{}-{clients}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for file in stale {
            // A port nobody listens on, as a killed game leaves it.
            std::fs::write(dir.join(file), "1\n").unwrap();
        }
        // Every process of the stand writes its output to one file, which a failure shows.
        let log = std::fs::File::create(dir.join("out.log")).unwrap();
        let host = Command::new(env!("CARGO_BIN_EXE_sandbox"))
            .args(["--stand", &clients.to_string(), "--stand-udp", "--headless", "--port", "0", "--port-file", "g.port"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        UdpStand { dir, host }
    }

    /// What the stand's processes printed, and the `net` part of each member's status: what a
    /// failed wait shows.
    fn report(&self) -> String {
        let log = std::fs::read_to_string(self.dir.join("out.log")).unwrap_or_default();
        let members: Vec<String> = ["g.port", "g.port.c1", "g.port.c2"]
            .iter()
            .map(|file| format!("{file}: {}", self.status(file).map_or_else(|| "no answer".to_owned(), |s| s["net"].to_string())))
            .collect();
        format!("{}\n--- output of the stand\n{log}", members.join("\n"))
    }

    /// The BRP port in `g.port` (`g.port.c1`, …), once the game has written it.
    fn port(&self, file: &str) -> Option<u16> {
        std::fs::read_to_string(self.dir.join(file)).ok()?.trim().parse().ok()
    }

    /// One BRP call to the game at `port`; `None` while it does not answer.
    fn brp(&self, port: u16, method: &str, params: Value) -> Option<Value> {
        let token = std::fs::read_to_string(self.dir.join(format!(".shards/{port}.token"))).ok()?;
        let mut params = params;
        params["token"] = json!(token.trim());
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
        write!(
            stream,
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .ok()?;
        let body = read_body(&mut BufReader::new(stream))?;
        let reply: Value = shards::serde_json::from_slice(&body).ok()?;
        reply.get("result").cloned()
    }

    fn status(&self, file: &str) -> Option<Value> {
        self.brp(self.port(file)?, "ai/status", Value::Null)
    }

    /// Waits for `what` to be `Some` (at most `limit`), saying what was not there in time.
    fn until<T>(&mut self, limit: Duration, what: &str, mut ready: impl FnMut(&UdpStand) -> Option<T>) -> T {
        let started = Instant::now();
        loop {
            if let Some(found) = ready(self) {
                return found;
            }
            assert!(self.host.try_wait().unwrap().is_none(), "the host exited while waiting for {what}\n{}", self.report());
            assert!(started.elapsed() < limit, "not there within {limit:?}: {what}\n{}", self.report());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits for the host to exit (at most `limit`) and returns how; a host that runs on is
    /// ended the polite way, with what the stand printed.
    fn exit_status(&mut self, limit: Duration, what: &str) -> std::process::ExitStatus {
        let started = Instant::now();
        loop {
            if let Some(status) = self.host.try_wait().unwrap() {
                return status;
            }
            assert!(started.elapsed() < limit, "the host runs on: {what}\n{}", self.report());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Waits for the clients' port files to go (at most `limit`): a client that leaves removes its
    /// file as it goes. Unlike [`UdpStand::until`] this is for after the host has exited.
    fn wait_for_client_files_gone(&self, limit: Duration, files: &[&str]) {
        let started = Instant::now();
        while files.iter().any(|file| self.dir.join(file).exists()) {
            assert!(started.elapsed() < limit, "the clients' files are still there after {limit:?}: {files:?}\n{}", self.report());
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn terminate(&self) {
        // SAFETY: a plain `kill` of our own child.
        assert_eq!(unsafe { libc::kill(self.host.id() as libc::pid_t, libc::SIGTERM) }, 0);
    }
}

impl Drop for UdpStand {
    fn drop(&mut self) {
        // A test that failed leaves the stand running: end it the polite way, then wait.
        let _ = unsafe { libc::kill(self.host.id() as libc::pid_t, libc::SIGTERM) };
        let started = Instant::now();
        while self.host.try_wait().ok().flatten().is_none() && started.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.host.kill();
        let _ = self.host.wait();
        // The clients leave with the host; their files going is how we know.
        let started = Instant::now();
        while (self.dir.join("g.port.c1").exists() || self.dir.join("g.port.c2").exists()) && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_udp_stand_starts_client_processes_that_join_and_leave_with_the_host() {
    // Port files an earlier (bigger) stand left behind: the host clears them before its clients start.
    let mut stand = UdpStand::start_after(2, &["g.port.c1", "g.port.c2", "g.port.c3", "g.port.c4"]);
    // The host and its two clients, each with a port of its own, both connected, and the host
    // listing them: all three statuses are read together, until they agree (the host sees a
    // client join a little before the client does, or after).
    let (host, c1, c2) = stand.until(Duration::from_secs(90), "both clients connected, and listed by the host", |s| {
        let host = s.status("g.port")?;
        let c1 = s.status("g.port.c1")?;
        let c2 = s.status("g.port.c2")?;
        let joined = |client: &Value| client["net"]["session"].as_str().is_some_and(|text| text.starts_with("connected as p"));
        let lists = |client: &Value| {
            host["net"]["peers"].as_array().is_some_and(|peers| {
                peers.iter().any(|p| p["player"] == client["net"]["player"] && p["body"] == client["net"]["body"] && !p["body"].is_null())
            })
        };
        (joined(&c1) && joined(&c2) && lists(&c1) && lists(&c2) && host["loading"] == json!([])).then_some((host, c1, c2))
    });
    assert_eq!(host["net"]["role"], "host");
    assert_eq!(host["net"]["member"], "host");
    assert_eq!(host["net"]["stand"], json!({"clients": 2, "udp": true}));
    assert!(host["net"]["udp"].as_str().is_some(), "the host says where its socket is: {}", host["net"]);
    for (client, name) in [(&c1, "c1"), (&c2, "c2")] {
        assert_eq!(client["net"]["role"], "client", "{name}");
        assert_eq!(client["net"]["member"], name);
        // The same game: the host's seed.
        assert_eq!(client["seed"], host["seed"], "{name}");
    }
    // Two processes that start together join in either order: players 1 and 2, one each.
    let mut players = [c1["net"]["player"].as_u64().unwrap(), c2["net"]["player"].as_u64().unwrap()];
    players.sort_unstable();
    assert_eq!(players, [1, 2]);
    // Three processes, and the clients are told apart: own BRP ports (the port files c1 and c2 were
    // written by processes starting together, each to a file of its own), own pids, own bodies.
    let ports = [stand.port("g.port").unwrap(), stand.port("g.port.c1").unwrap(), stand.port("g.port.c2").unwrap()];
    assert_eq!(ports.iter().collect::<std::collections::BTreeSet<_>>().len(), 3, "three ports: {ports:?}");
    assert_eq!(c1["instance"]["port"], ports[1], "F.c1 holds c1's port");
    assert_eq!(c2["instance"]["port"], ports[2], "F.c2 holds c2's port");
    let pids = [&host, &c1, &c2].map(|s| s["instance"]["pid"].as_i64().unwrap());
    assert_eq!(pids.iter().collect::<std::collections::BTreeSet<_>>().len(), 3, "three processes: {pids:?}");
    assert_ne!(c1["net"]["body"], c2["net"]["body"]);
    // The leftovers past this stand's two clients are gone: they are not members of it.
    for stale in ["g.port.c3", "g.port.c4"] {
        assert!(!stand.dir.join(stale).exists(), "{stale} was left by an earlier stand and the host cleared it");
    }

    // `net_status` of a client: connected, in sync.
    let net = stand.brp(ports[1], "ai/act", json!({"tool": "net_status", "args": {}})).expect("the client answers");
    assert_eq!(net["ok"], true, "{net}");
    assert_eq!(net["result"]["stage"], "Connected", "{net}");
    assert_eq!(net["result"]["desyncs"], 0, "{net}");

    // SIGTERM to the host ends the session, and the clients go with it, the way a game leaves.
    stand.terminate();
    let started = Instant::now();
    let code = loop {
        if let Some(status) = stand.host.try_wait().unwrap() {
            break status.code();
        }
        assert!(started.elapsed() < Duration::from_secs(30), "the host did not exit");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(code, Some(143), "exits on the signal like any game");
    // The host has exited and been reaped: `until` would ask it to be alive, so this only polls the files.
    stand.wait_for_client_files_gone(Duration::from_secs(20), &["g.port.c1", "g.port.c2"]);
    assert!(!stand.dir.join("g.port").exists(), "the host removed its port file");
    // The client processes are gone (or on their way out as zombies nobody has reaped yet).
    for pid in [pids[1], pids[2]] {
        let pid = pid as libc::pid_t;
        let started = Instant::now();
        loop {
            // SAFETY: signal 0 only asks whether the process exists.
            let alive = unsafe { libc::kill(pid, 0) } == 0;
            let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| s.contains(") Z "));
            if !alive || zombie {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "client process {pid} is still running");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A client that cannot start (here: its port file is a folder, so it cannot write it) ends the
/// stand at once with that client's exit status and says which one, instead of leaving the
/// host to run on and `wait --peer all` to time out.
#[test]
fn a_client_that_cannot_start_ends_the_stand_with_its_exit_status() {
    let dir = std::env::temp_dir().join(format!("sandbox-stand-udp-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("g.port.c1")).unwrap();
    let errors = std::fs::File::create(dir.join("stderr.log")).unwrap();
    let mut host = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .args(["--stand", "1", "--stand-udp", "--headless", "--port", "0", "--port-file", "g.port"])
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors)
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = host.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(60) {
            // SAFETY: a plain `kill` of our own child, the polite way.
            unsafe { libc::kill(host.id() as libc::pid_t, libc::SIGTERM) };
            let _ = host.wait();
            let log = std::fs::read_to_string(dir.join("stderr.log")).unwrap_or_default();
            panic!("the host runs on without its client: {log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let log = std::fs::read_to_string(dir.join("stderr.log")).unwrap_or_default();
    // The client's own exit code (2: it could not write its port file) is the host's.
    assert_eq!(status.code(), Some(2), "{log}");
    assert!(log.contains("cannot write the port file"), "the client said why: {log}");
    assert!(
        log.contains("error: stand: client c1 exited with code 2 before it was ready (it never wrote its port file): see its output above"),
        "the host said which client: {log}"
    );
    assert!(!dir.join("g.port").exists(), "the host removed its port file");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_udp_stand_is_real_time_and_says_so() {
    let output = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .args(["--stand", "1", "--stand-udp", "--headless", "--paused", "--port", "0"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("--stand-udp runs in real time") && error.contains("Drop --paused"), "{error}");
}

/// `--stand 3` when the session holds the host and two clients (`max_players` 3): the third
/// would be turned away, so the host says so and ends, instead of running on with a stand that
/// is not the one asked for. No client process is started for it: the check comes first.
#[test]
fn a_stand_the_session_has_no_room_for_is_refused() {
    let mut stand = UdpStand::start(3);
    let status = stand.exit_status(Duration::from_secs(90), "--stand 3 does not fit the session");
    let log = std::fs::read_to_string(stand.dir.join("out.log")).unwrap_or_default();
    assert_eq!(status.code(), Some(2), "{log}");
    assert!(
        log.contains("error: stand: --stand 3 needs room for 4 players, but the session allows 3 (`max_players` of the net definition)"),
        "{log}"
    );
    assert!(!stand.dir.join("g.port").exists(), "the host removed its port file");
    // Nothing was started to be torn down again: the host's journal would say `c1 started (pid …)`.
    assert!(!log.contains("started (pid"), "no client process was started: {log}");
    for client in ["g.port.c1", "g.port.c2", "g.port.c3"] {
        assert!(!stand.dir.join(client).exists(), "{client} was never written");
    }
}

/// A client that has joined and then crashes (here: killed with SIGKILL) ends the stand with a
/// status and says which client: a stand with a dead member is not the stand that was asked for.
#[test]
fn a_client_that_crashes_after_joining_ends_the_stand() {
    let mut stand = UdpStand::start(1);
    let c1 = stand.until(Duration::from_secs(90), "the client connected, and listed by the host", |s| {
        let host = s.status("g.port")?;
        let c1 = s.status("g.port.c1")?;
        let joined = c1["net"]["session"].as_str().is_some_and(|text| text.starts_with("connected as p"));
        let listed = host["net"]["peers"].as_array().is_some_and(|peers| peers.iter().any(|p| p["player"] == c1["net"]["player"]));
        (joined && listed).then_some(c1)
    });
    let pid = c1["instance"]["pid"].as_i64().unwrap() as libc::pid_t;
    // SAFETY: a plain `kill` of a process this test's stand started.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    let status = stand.exit_status(Duration::from_secs(60), "its client was killed");
    let log = std::fs::read_to_string(stand.dir.join("out.log")).unwrap_or_default();
    assert_eq!(status.code(), Some(2), "{log}");
    assert!(
        log.contains("error: stand: client c1 was ended by signal 9 while the stand ran: it crashed, see its output above"),
        "{log}"
    );
    assert!(!stand.dir.join("g.port").exists(), "the host removed its port file");
    // A killed client cannot remove its own file: the stand's drop would wait for it.
    let _ = std::fs::remove_file(stand.dir.join("g.port.c1"));
}

/// `net_kick` of a client of a UDP stand: the client leaves with an error code of its own (it was
/// sent away), the host journals that and the stand goes on for the others. It is not a crash.
#[test]
fn a_client_the_host_kicks_leaves_and_the_stand_goes_on() {
    let mut stand = UdpStand::start(2);
    let (host, c1, c2) = stand.until(Duration::from_secs(90), "both clients connected, and listed by the host", |s| {
        let host = s.status("g.port")?;
        let c1 = s.status("g.port.c1")?;
        let c2 = s.status("g.port.c2")?;
        let joined = |client: &Value| client["net"]["session"].as_str().is_some_and(|text| text.starts_with("connected as p"));
        let listed = |client: &Value| host["net"]["peers"].as_array().is_some_and(|p| p.iter().any(|p| p["player"] == client["net"]["player"]));
        (joined(&c1) && joined(&c2) && listed(&c1) && listed(&c2)).then_some((host, c1, c2))
    });
    let player = c1["net"]["player"].as_u64().unwrap();
    let kicked = stand.brp(stand.port("g.port").unwrap(), "ai/act", json!({"tool": "net_kick", "args": {"player": player}}));
    assert_eq!(kicked.as_ref().map(|k| &k["ok"]), Some(&json!(true)), "{kicked:?}");
    // c1 goes (its port file with it), the host says why, and neither the host nor c2 follows.
    stand.wait_for_client_files_gone(Duration::from_secs(30), &["g.port.c1"]);
    stand.until(Duration::from_secs(30), "the host to journal that it sent c1 away", |s| {
        std::fs::read_to_string(s.dir.join("out.log"))
            .ok()?
            .contains("client c1 exited with code 1 after the host closed its link to it (net_kick, or its silence timed out)")
            .then_some(())
    });
    std::thread::sleep(Duration::from_millis(1500));
    let log = std::fs::read_to_string(stand.dir.join("out.log")).unwrap_or_default();
    assert!(!log.contains("it crashed"), "a kick is not a crash: {log}");
    assert!(stand.host.try_wait().unwrap().is_none(), "the host runs on\n{log}");
    let after = stand.status("g.port.c2").expect("c2 still answers");
    assert!(after["net"]["session"].as_str().is_some_and(|text| text.starts_with("connected as p")), "{}", after["net"]);
    assert_eq!(after["net"]["player"], c2["net"]["player"]);
    assert_eq!(host["net"]["stand"], json!({"clients": 2, "udp": true}));
}

/// A client that is asked to quit (SIGTERM, as `kill` or a CI trap sends it) says goodbye to the
/// host and leaves with the code a signalled game leaves with: the host does not take that for a
/// crash, nor for its own doing. The stand goes on for the others.
#[test]
fn a_client_that_is_asked_to_quit_leaves_and_the_stand_goes_on() {
    let mut stand = UdpStand::start(2);
    let (c1, c2) = stand.until(Duration::from_secs(90), "both clients connected", |s| {
        let c1 = s.status("g.port.c1")?;
        let c2 = s.status("g.port.c2")?;
        let joined = |client: &Value| client["net"]["session"].as_str().is_some_and(|text| text.starts_with("connected as p"));
        (joined(&c1) && joined(&c2)).then_some((c1, c2))
    });
    let pid = c1["instance"]["pid"].as_i64().unwrap() as libc::pid_t;
    // SAFETY: a plain `kill` of a process this test's stand started.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    stand.wait_for_client_files_gone(Duration::from_secs(30), &["g.port.c1"]);
    stand.until(Duration::from_secs(30), "the host to journal that c1 left", |s| {
        std::fs::read_to_string(s.dir.join("out.log")).ok()?.contains("client c1 exited with code 143 while the stand ran").then_some(())
    });
    std::thread::sleep(Duration::from_millis(1500));
    let log = std::fs::read_to_string(stand.dir.join("out.log")).unwrap_or_default();
    assert!(!log.contains("it crashed") && !log.contains("after the host closed its link"), "{log}");
    assert!(stand.host.try_wait().unwrap().is_none(), "the host runs on\n{log}");
    let after = stand.status("g.port.c2").expect("c2 still answers");
    assert_eq!(after["net"]["player"], c2["net"]["player"]);
}
