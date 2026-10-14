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

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_core::EngineAppExt;
use shards_net::NetEndpoint;

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

/// The children, until the host is done with them.
#[derive(Resource, Default)]
struct StandChildren(Vec<(String, Child)>);

impl Drop for StandChildren {
    /// A host that goes without its exit hook (a panic) still lets its clients go.
    fn drop(&mut self) {
        for (_, child) in &mut self.0 {
            signal(child);
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
    app.insert_resource(spec).init_resource::<StandChildren>().add_systems(PostStartup, spawn_children).add_exit_hook("stand-udp", end_children);
}

/// `PostStartup`: the host's socket is open (`Startup`), so the clients can be told where it is.
fn spawn_children(world: &mut World) {
    let spec = world.resource::<StandSpec>().clone();
    let port = world
        .get_resource::<NetEndpoint>()
        .and_then(|endpoint| endpoint.transport.local_addr())
        .and_then(|addr| addr.rsplit(':').next().and_then(|port| port.parse::<u16>().ok()));
    let Some(port) = port else {
        journal::emit(world, JournalKind::Error, "stand: the host has no UDP socket, so its clients were not started");
        return;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            journal::emit(world, JournalKind::Error, format!("stand: cannot find this program to start the clients: {e}"));
            return;
        }
    };
    let addr = format!("127.0.0.1:{port}");
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
                children.push((format!("c{n}"), child));
            }
            Err(e) => {
                journal::emit(world, JournalKind::Error, format!("stand: cannot start c{n}: {e}"));
            }
        }
    }
    world.resource_mut::<StandChildren>().0 = children;
}

/// Exit hook, after the session's own: the clients were told the session ended and leave
/// (`--exit-with-host`); the ones that do not are signalled.
fn end_children(world: &mut World) {
    let Some(mut children) = world.remove_resource::<StandChildren>() else { return };
    let left = |children: &mut StandChildren| children.0.retain_mut(|(_, child)| child.try_wait().map(|status| status.is_none()).unwrap_or(false));
    wait_until(&mut children, LEAVE_WAIT, left);
    for (_, child) in &children.0 {
        signal(child);
    }
    wait_until(&mut children, TERM_WAIT, left);
    for (name, child) in &children.0 {
        eprintln!("warning: stand client {name} (pid {}) is still running after SIGTERM; end it with `kill {}`", child.id(), child.id());
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
}
