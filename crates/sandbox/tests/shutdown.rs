//! `kill` and a closed terminal end the game the way Ctrl+C does: the files it wrote at
//! start (`.shards/<port>.token`, `--port-file`) go away with it.

#![cfg(all(unix, feature = "remote"))]

use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A headless game in a folder of its own, on a free port.
struct Game {
    dir: PathBuf,
    child: Child,
    port: u16,
}

impl Game {
    fn start(name: &str, ignore_hangup: bool) -> Game {
        let dir = std::env::temp_dir().join(format!("sandbox-shutdown-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sandbox"));
        cmd.args(["--headless", "--paused", "--port", "0", "--port-file", "port"])
            .current_dir(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if ignore_hangup {
            // What `nohup` does. SAFETY: `signal` is async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    libc::signal(libc::SIGHUP, libc::SIG_IGN);
                    Ok(())
                });
            }
        }
        let child = cmd.spawn().unwrap();
        let mut game = Game { dir, child, port: 0 };
        let started = Instant::now();
        // The port file comes last: the token and the signal handlers are in place.
        while game.port == 0 {
            match std::fs::read_to_string(game.dir.join("port")) {
                Ok(port) => game.port = port.trim().parse().unwrap(),
                Err(_) => {
                    assert!(game.child.try_wait().unwrap().is_none(), "the game exited at start");
                    assert!(started.elapsed() < Duration::from_secs(60), "the game did not start");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        game
    }

    fn token(&self) -> PathBuf {
        self.dir.join(format!(".shards/{}.token", self.port))
    }

    fn signal(&self, signal: libc::c_int) {
        // SAFETY: a plain `kill` of our own child.
        assert_eq!(unsafe { libc::kill(self.child.id() as libc::pid_t, signal) }, 0);
    }

    /// The exit code, or `None` if the game still runs after `wait`.
    fn exit_code(&mut self, wait: Duration) -> Option<i32> {
        let started = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status.code().expect("exited on its own, not killed by the signal"));
            }
            if started.elapsed() > wait {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn kill_and_hangup_remove_the_token_and_the_port_file() {
    for (signal, code) in [(libc::SIGTERM, 143), (libc::SIGHUP, 129)] {
        let mut game = Game::start(&format!("signal{signal}"), false);
        assert!(game.token().exists(), "the game writes its token at start");
        game.signal(signal);
        assert_eq!(game.exit_code(Duration::from_secs(20)), Some(code), "signal {signal}");
        assert!(!game.token().exists(), "signal {signal}: the token goes with the game");
        assert!(!game.dir.join("port").exists(), "signal {signal}: and so does the port file");
    }
}

#[test]
fn a_hangup_ignored_with_nohup_stays_ignored() {
    let mut game = Game::start("nohup", true);
    game.signal(libc::SIGHUP);
    assert_eq!(game.exit_code(Duration::from_millis(500)), None, "nohup: the game keeps running");
    assert!(game.token().exists());
    game.signal(libc::SIGTERM);
    assert_eq!(game.exit_code(Duration::from_secs(20)), Some(143));
    assert!(!game.token().exists());
}
