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
        let dir = std::env::temp_dir().join(format!("sandbox-stand-udp-{}-{clients}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let host = Command::new(env!("CARGO_BIN_EXE_sandbox"))
            .args(["--stand", &clients.to_string(), "--stand-udp", "--headless", "--port", "0", "--port-file", "g.port"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        UdpStand { dir, host }
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
            assert!(self.host.try_wait().unwrap().is_none(), "the host exited while waiting for {what}");
            assert!(started.elapsed() < limit, "not there within {limit:?}: {what}");
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
    let mut stand = UdpStand::start(2);
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
    stand.until(Duration::from_secs(20), "the clients' files gone: they left", |s| {
        (!s.dir.join("g.port.c1").exists() && !s.dir.join("g.port.c2").exists()).then_some(())
    });
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
