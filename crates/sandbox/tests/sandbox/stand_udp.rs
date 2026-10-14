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
        // The client leaves with the host; its files going is how we know.
        let started = Instant::now();
        while self.dir.join("g.port.c1").exists() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_udp_stand_starts_a_client_process_that_joins_and_leaves_with_the_host() {
    let mut stand = UdpStand::start(1);
    // The host and its client, each with a port of its own, the client connected.
    let (host, client) = stand.until(Duration::from_secs(90), "the client connected to the host", |s| {
        let host = s.status("g.port")?;
        let client = s.status("g.port.c1")?;
        let connected = client["net"]["session"].as_str().is_some_and(|text| text.starts_with("connected as p1"));
        (connected && host["loading"] == json!([])).then_some((host, client))
    });
    assert_eq!(host["net"]["role"], "host");
    assert_eq!(host["net"]["member"], "host");
    assert_eq!(host["net"]["stand"], json!({"clients": 1, "udp": true}));
    assert!(host["net"]["udp"].as_str().is_some(), "the host says where its socket is: {}", host["net"]);
    assert_eq!(client["net"]["role"], "client");
    assert_eq!(client["net"]["member"], "c1");
    assert_eq!(client["net"]["player"], 1);
    // Two processes: the client is a child of the host.
    assert_ne!(host["instance"]["pid"], client["instance"]["pid"]);
    assert_eq!(host["net"]["peers"][0]["player"], 1);
    assert_eq!(host["net"]["peers"][0]["body"], client["net"]["body"], "the host lists the body the client says is its own");
    // The same game: the host's seed.
    assert_eq!(client["seed"], host["seed"]);
    let client_pid = client["instance"]["pid"].as_i64().unwrap() as libc::pid_t;

    // `net_status` of the client: connected, in sync.
    let port = stand.port("g.port.c1").unwrap();
    let net = stand.brp(port, "ai/act", json!({"tool": "net_status", "args": {}})).expect("the client answers");
    assert_eq!(net["ok"], true, "{net}");
    assert_eq!(net["result"]["stage"], "Connected", "{net}");
    assert_eq!(net["result"]["desyncs"], 0, "{net}");

    // SIGTERM to the host ends the session, and the client goes with it, the way a game leaves.
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
    stand.until(Duration::from_secs(20), "the client's files gone: it left", |s| (!s.dir.join("g.port.c1").exists()).then_some(()));
    assert!(!stand.dir.join("g.port").exists(), "the host removed its port file");
    // The client process is gone (or on its way out as a zombie nobody has reaped yet).
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        // SAFETY: signal 0 only asks whether the process exists.
        let alive = unsafe { libc::kill(client_pid, 0) } == 0;
        let zombie = std::fs::read_to_string(format!("/proc/{client_pid}/stat")).is_ok_and(|s| s.contains(") Z "));
        if !alive || zombie {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("client process {client_pid} is still running");
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
