//! `--stand N --stand-udp`: a host and N clients from one command, over real UDP.
//!
//! The process is the UDP host (as `--host 0`: a free port) and starts N child processes of the
//! same game, `--join 127.0.0.1:<port> --headless --port 0 --port-file F.cK --name cK`, once the
//! host's socket is open. Real time only: UDP peers cannot be stepped together, so an agent
//! steps this stand by waiting (`shards step N` on the host runs N ticks back to back, and the
//! clients follow). The in-process stand (`--stand N`) is the one that can be paused.
//!
//! Every child is a game of its own with its own `shards` port and token (`F.c1`, `F.c2`, …
//! hold the ports). They leave with the host: `--exit-with-host` ends them when the session
//! ends, the host signals (SIGTERM) what is still there when it exits, and on Linux a child
//! also gets SIGTERM if the host dies without cleaning up.
//!
//! The host watches its children. One that cannot be started, or exits before its port file
//! appeared (a bad flag, a busy port, a crash at start), ends the stand at once with that
//! child's exit status and a line saying which one; a child that leaves later is journaled.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use ai_core::journal::{self, JournalKind};
use bevy::app::AppExit;
use bevy::prelude::*;
use shards_core::EngineAppExt;
use shards_net::{NetEndpoint, NetHost};

/// How long the host waits for its clients to leave on their own once the session has ended.
const LEAVE_WAIT: Duration = Duration::from_secs(4);

/// How long it waits for the ones it signalled.
const TERM_WAIT: Duration = Duration::from_secs(5);

/// What the children are started with: the host's own command line, as far as it matters to
/// them.
#[derive(Resource, Clone, Debug)]
pub struct StandSpec {
    pub clients: u8,
    pub seed: u64,
    pub hz: f64,
    pub net_sim: Option<String>,
    pub tick_budget: Option<Duration>,
    /// The host's `--port-file F`: a child writes `F.cK`.
    pub port_file: Option<PathBuf>,
    pub no_remote: bool,
    /// The host's `--save-dir`: a child keeps its character below it (`<dir>/stand-cK`), so no
    /// two games of the stand write the same files.
    pub save_dir: String,
}

/// The port file of member `name` beside `path`: `F` → `F.c1`.
pub fn member_file(path: &Path, name: &str) -> PathBuf {
    let mut file = path.as_os_str().to_owned();
    file.push(format!(".{name}"));
    PathBuf::from(file)
}

/// The command line of child `n`, joining the host at `addr`.
pub fn child_args(spec: &StandSpec, n: u8, addr: &str) -> Vec<String> {
    let mut args: Vec<String> = ["--headless", "--no-settings", "--exit-with-host"].map(str::to_owned).into();
    let mut with = |flag: &str, value: String| args.extend([flag.to_owned(), value]);
    with("--join", addr.to_owned());
    with("--name", format!("c{n}"));
    with("--seed", spec.seed.to_string());
    with("--hz", spec.hz.to_string());
    with("--save-dir", format!("{}/stand-c{n}", spec.save_dir.trim_end_matches('/')));
    if let Some(sim) = &spec.net_sim {
        with("--net-sim", sim.clone());
    }
    if let Some(budget) = spec.tick_budget {
        with("--tick-budget-ms", (budget.as_secs_f64() * 1000.0).to_string());
    }
    if spec.no_remote {
        args.push("--no-remote".into());
    } else {
        let mut with = |flag: &str, value: String| args.extend([flag.to_owned(), value]);
        with("--port", "0".into());
        if let Some(file) = &spec.port_file {
            with("--port-file", member_file(file, &format!("c{n}")).display().to_string());
        }
    }
    args
}

/// One child process of the stand.
struct StandChild {
    name: String,
    child: Child,
    /// Its port file has appeared: it has started (a child without one is ready from the start).
    ready: bool,
}

/// The children, until the host is done with them.
#[derive(Resource, Default)]
struct StandChildren(Vec<StandChild>);

impl Drop for StandChildren {
    /// A host that goes without its exit hook (a panic) still lets its clients go.
    fn drop(&mut self) {
        for child in &self.0 {
            signal(&child.child);
        }
    }
}

/// SIGTERM, which a game turns into a normal exit (token and port file removed).
#[cfg(unix)]
fn signal(child: &Child) {
    // SAFETY: `kill` with a pid and a signal number takes no pointers.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
}

#[cfg(not(unix))]
fn signal(_: &Child) {}

/// Makes `app` the host of a stand over UDP: its clients start once its socket is open and are
/// let go when it exits.
pub fn install(app: &mut App, spec: StandSpec) {
    app.insert_resource(spec)
        .init_resource::<StandChildren>()
        .add_systems(PostStartup, spawn_children)
        .add_systems(Last, watch_children)
        .add_exit_hook("stand-udp", end_children);
}

/// The stand cannot go on: the reason on stderr, the child's exit code (or 2) as the host's.
fn fail_stand(world: &mut World, why: &str, code: u8) {
    journal::emit(world, JournalKind::Error, format!("stand: {why}"));
    eprintln!("error: stand: {why}");
    world.write_message(AppExit::from_code(code));
}

/// `PostStartup`: the host's socket is open (`Startup`), so the clients can be told where it is.
fn spawn_children(world: &mut World) {
    let spec = world.resource::<StandSpec>().clone();
    let port = world
        .get_resource::<NetEndpoint>()
        .and_then(|endpoint| endpoint.transport.local_addr())
        .and_then(|addr| addr.rsplit(':').next().and_then(|port| port.parse::<u16>().ok()));
    let Some(port) = port else {
        fail_stand(world, "the host has no UDP socket, so its clients cannot be started", 2);
        return;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            fail_stand(world, &format!("cannot find this program to start the clients: {e}"), 2);
            return;
        }
    };
    let addr = format!("127.0.0.1:{port}");
    // Without a port file to wait for, a child counts as started once it is spawned.
    let waits_for_files = spec.port_file.is_some() && !spec.no_remote;
    let mut children = Vec::new();
    for n in 1..=spec.clients {
        let mut command = Command::new(&exe);
        command.args(child_args(&spec, n, &addr)).stdin(Stdio::null());
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: the closure runs between fork and exec and makes one async-signal-safe call.
            unsafe {
                command.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                    Ok(())
                });
            }
        }
        match command.spawn() {
            Ok(child) => {
                journal::emit(world, JournalKind::Sim, format!("stand: c{n} started (pid {}), joining {addr}", child.id()));
                children.push(StandChild { name: format!("c{n}"), child, ready: !waits_for_files });
            }
            Err(e) => {
                world.resource_mut::<StandChildren>().0 = children;
                fail_stand(world, &format!("cannot start c{n} ({}): {e}", exe.display()), 2);
                return;
            }
        }
    }
    world.resource_mut::<StandChildren>().0 = children;
}

/// What a child's exit means: whether it ends the stand, the code the host exits with, and the
/// line that says it.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    fatal: bool,
    code: u8,
    text: String,
}

/// `name` exited with `status`; `ready`: its port file had appeared; `session_over`: the host's
/// session has ended (its clients are leaving, as they should).
fn verdict(name: &str, status: ExitStatus, ready: bool, session_over: bool) -> Verdict {
    let how = match (status.code(), exit_signal(status)) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("was ended by signal {signal}"),
        (None, None) => "exited".to_owned(),
    };
    if ready || session_over {
        let why = if session_over { "after the session ended" } else { "while the stand ran" };
        return Verdict { fatal: false, code: 0, text: format!("client {name} {how} {why}") };
    }
    // Before its port file appeared: it never started. Its own exit code is the stand's.
    let code = status.code().filter(|code| *code > 0 && *code < 256).map_or(2, |code| code as u8);
    Verdict { fatal: true, code, text: format!("client {name} {how} before it was ready (it never wrote its port file): see its output above") }
}

#[cfg(unix)]
fn exit_signal(status: ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&status)
}

#[cfg(not(unix))]
fn exit_signal(_: ExitStatus) -> Option<i32> {
    None
}

/// `Last`: the children that exited since the last frame. One that never started ends the stand
/// with its exit status; the others are journaled.
fn watch_children(world: &mut World) {
    let Some(mut children) = world.remove_resource::<StandChildren>() else { return };
    let files = world.resource::<StandSpec>().port_file.clone();
    let session_over = world.get_resource::<NetHost>().is_some_and(|host| host.ended.is_some());
    let mut left = Vec::new();
    children.0.retain_mut(|c| {
        if !c.ready {
            // A file, not a folder of that name: what the child wrote, whole.
            c.ready = files.as_ref().is_some_and(|file| member_file(file, &c.name).is_file());
        }
        match c.child.try_wait() {
            Ok(Some(status)) => {
                left.push(verdict(&c.name, status, c.ready, session_over));
                false
            }
            _ => true,
        }
    });
    world.insert_resource(children);
    for v in left {
        if v.fatal {
            fail_stand(world, &v.text, v.code);
        } else {
            journal::emit(world, JournalKind::Warn, format!("stand: {}", v.text));
        }
    }
}

/// Exit hook, after the session's own: the clients were told the session ended and leave
/// (`--exit-with-host`); the ones that do not are signalled.
fn end_children(world: &mut World) {
    let Some(mut children) = world.remove_resource::<StandChildren>() else { return };
    let left = |children: &mut StandChildren| children.0.retain_mut(|c| c.child.try_wait().map(|status| status.is_none()).unwrap_or(false));
    wait_until(&mut children, LEAVE_WAIT, left);
    for c in &children.0 {
        signal(&c.child);
    }
    wait_until(&mut children, TERM_WAIT, left);
    for c in &children.0 {
        eprintln!("warning: stand client {} (pid {}) is still running after SIGTERM; end it with `kill {}`", c.name, c.child.id(), c.child.id());
    }
    // What is still there stays: the host is going, and a dropped `StandChildren` signals once more.
}

/// Calls `prune` until it leaves no child or `limit` has passed.
fn wait_until(children: &mut StandChildren, limit: Duration, prune: impl Fn(&mut StandChildren)) {
    let started = Instant::now();
    loop {
        prune(children);
        if children.0.is_empty() || started.elapsed() >= limit {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> StandSpec {
        StandSpec {
            clients: 2,
            seed: 42,
            hz: 60.0,
            net_sim: Some("latency=150,loss=0.02".into()),
            tick_budget: None,
            port_file: Some(PathBuf::from("/tmp/g.port")),
            no_remote: false,
            save_dir: "saves".into(),
        }
    }

    #[test]
    fn a_child_joins_the_host_with_its_own_port_name_and_saves() {
        let args = child_args(&spec(), 2, "127.0.0.1:54321");
        assert_eq!(
            args,
            [
                "--headless", "--no-settings", "--exit-with-host", "--join", "127.0.0.1:54321", "--name", "c2", "--seed", "42", "--hz", "60",
                "--save-dir", "saves/stand-c2", "--net-sim", "latency=150,loss=0.02", "--port", "0", "--port-file", "/tmp/g.port.c2"
            ]
        );
    }

    #[test]
    fn a_child_without_a_server_has_no_port_file_and_budgets_pass_through() {
        let spec = StandSpec { no_remote: true, port_file: None, net_sim: None, tick_budget: Some(Duration::from_millis(5)), save_dir: "saves/".into(), ..spec() };
        let args = child_args(&spec, 1, "127.0.0.1:1");
        assert!(args.windows(2).any(|w| w == ["--tick-budget-ms", "5"]), "{args:?}");
        assert!(args.contains(&"--no-remote".to_owned()) && !args.contains(&"--port".to_owned()) && !args.contains(&"--net-sim".to_owned()), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--save-dir", "saves/stand-c1"]), "one slash: {args:?}");
    }

    /// The exit status of a real process: `sh -c 'exit N'`, or killed by a signal.
    fn status_of(script: &str) -> ExitStatus {
        Command::new("sh").args(["-c", script]).status().expect("sh runs")
    }

    #[test]
    #[cfg(unix)]
    fn a_child_that_never_started_ends_the_stand_with_its_exit_status() {
        // Exited with code 3 before its port file appeared (a busy port, a bad flag).
        assert_eq!(
            verdict("c2", status_of("exit 3"), false, false),
            Verdict {
                fatal: true,
                code: 3,
                text: "client c2 exited with code 3 before it was ready (it never wrote its port file): see its output above".into()
            }
        );
        // Code 0 before it was ready is still a failure (exit code 2), and so is a signal.
        let v = verdict("c1", status_of("exit 0"), false, false);
        assert!(v.fatal && v.code == 2 && v.text.starts_with("client c1 exited with code 0 before it was ready"), "{v:?}");
        let v = verdict("c1", status_of("kill -TERM $$"), false, false);
        assert!(v.fatal && v.code == 2 && v.text.starts_with("client c1 was ended by signal 15 before it was ready"), "{v:?}");
    }

    #[test]
    #[cfg(unix)]
    fn a_child_that_leaves_later_is_only_journaled() {
        // Ready (its port file was there): a later exit does not end the stand.
        let v = verdict("c1", status_of("exit 1"), true, false);
        assert_eq!(v, Verdict { fatal: false, code: 0, text: "client c1 exited with code 1 while the stand ran".into() });
        // The session is over: clients are meant to leave, even one that was quicker than a look.
        let v = verdict("c1", status_of("exit 0"), false, true);
        assert_eq!(v, Verdict { fatal: false, code: 0, text: "client c1 exited with code 0 after the session ended".into() });
    }
}
