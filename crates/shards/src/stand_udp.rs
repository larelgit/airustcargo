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
//! The host watches its children. One that cannot be started, exits before its port file
//! appeared (a bad flag, a busy port, a crash at start), exits without ever having joined (the
//! session was full), or crashes later, ends the stand at once with that child's exit status and
//! a line saying which one; a child that joined and leaves on purpose (exit code 0, SIGTERM) is
//! journaled. `--stand N` needs room for N clients in the session (`max_players` of the net
//! definition), which the host checks as soon as content is loaded.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use ai_core::journal::{self, JournalKind};
use bevy::app::AppExit;
use bevy::prelude::*;
use shards_core::{EngineAppExt, EngineState};
use shards_net::{net_def, NetEndpoint, NetHost};

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

/// [`fail_stand`] from a `Last` system: the exit hooks (the session's end for the clients, the
/// saves, the children's) run now, in this system. `run_exit_hooks` is a `Last` system of its
/// own with no order against this one, so without this the host could leave in a frame in which
/// it ran first and saw no exit.
fn end_stand(world: &mut World, why: &str, code: u8) {
    fail_stand(world, why, code);
    shards_core::run_exit_hooks(world);
}

/// Removes the port files an earlier stand left beside `file` (`kill -9` leaves them): the
/// first `clients` of this stand's, and any past them. A file that is already there would
/// count as a child that has started, and as a member of this stand.
fn remove_stale_files(file: &Path, clients: u8) {
    let stale = |n: u8| member_file(file, &format!("c{n}"));
    for n in 1..=clients {
        // A folder of that name stays: the child cannot write it, which is its own failure.
        let _ = std::fs::remove_file(stale(n));
    }
    for n in clients.saturating_add(1)..=u8::MAX {
        if std::fs::remove_file(stale(n)).is_err() {
            break;
        }
    }
}

/// Whether the session has room for `clients` clients: the host is player 0.
fn room_for(clients: u8, max_players: u8) -> Result<(), String> {
    if clients < max_players {
        return Ok(());
    }
    Err(format!(
        "--stand {clients} needs room for {} players, but the session allows {max_players} (`max_players` of the net definition): \
         ask for --stand {} or fewer, or raise max_players",
        u16::from(clients) + 1,
        max_players.saturating_sub(1)
    ))
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
    if let (true, Some(file)) = (waits_for_files, &spec.port_file) {
        remove_stale_files(file, spec.clients);
    }
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

/// What the host knows of a child when it exits.
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// Its port file had appeared: it had started.
    ready: bool,
    /// The host had admitted it to the session (a refused client never is).
    joined: bool,
    /// The host's session has ended: its clients are leaving, as they should.
    session_over: bool,
}

/// `name` exited with `status`. A child that leaves on purpose after joining (exit code 0, or
/// the SIGTERM or SIGINT it turns into a normal exit) is journaled. One that never started,
/// never joined (the session was full) or crashed ends the stand.
fn verdict(name: &str, status: ExitStatus, seen: Seen) -> Verdict {
    let how = match (status.code(), exit_signal(status)) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("was ended by signal {signal}"),
        (None, None) => "exited".to_owned(),
    };
    let on_purpose = match (status.code(), exit_signal(status)) {
        // 128 + signal: how a game that was signalled leaves.
        (Some(code), _) => matches!(code, 0 | 130 | 143),
        (None, signal) => matches!(signal, Some(2 | 15)),
    };
    // The child's own exit code is the stand's (2 when it has none).
    let code = status.code().filter(|code| *code > 0 && *code < 256).map_or(2, |code| code as u8);
    if seen.session_over {
        return Verdict { fatal: false, code: 0, text: format!("client {name} {how} after the session ended") };
    }
    if !seen.ready {
        return Verdict { fatal: true, code, text: format!("client {name} {how} before it was ready (it never wrote its port file): see its output above") };
    }
    if !seen.joined {
        return Verdict {
            fatal: true,
            code,
            text: format!(
                "client {name} {how} without joining the host (it may have been refused: is the session full?): see the host's journal and its output above"
            ),
        };
    }
    if on_purpose {
        return Verdict { fatal: false, code: 0, text: format!("client {name} {how} while the stand ran") };
    }
    Verdict { fatal: true, code, text: format!("client {name} {how} while the stand ran: it crashed, see its output above") }
}

#[cfg(unix)]
fn exit_signal(status: ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&status)
}

#[cfg(not(unix))]
fn exit_signal(_: ExitStatus) -> Option<i32> {
    None
}

/// `Last`: the children that exited since the last frame. One that never started or joined, or
/// crashed, ends the stand with its exit status; the others are journaled. Once content is loaded
/// it also checks that the session has room for the clients.
fn watch_children(world: &mut World, mut sized: Local<bool>) {
    if !*sized && world.get_resource::<State<EngineState>>().is_some_and(|state| *state.get() != EngineState::Loading) {
        *sized = true;
        let clients = world.resource::<StandSpec>().clients;
        if let Err(why) = room_for(clients, net_def(world).max_players) {
            end_stand(world, &why, 2);
            return;
        }
    }
    let Some(mut children) = world.remove_resource::<StandChildren>() else { return };
    let files = world.resource::<StandSpec>().port_file.clone();
    let (session_over, joined): (bool, Vec<String>) = match world.get_resource::<NetHost>() {
        Some(host) => (host.ended.is_some(), host.slots.values().map(|slot| slot.name.clone()).collect()),
        None => (false, Vec::new()),
    };
    let mut left = Vec::new();
    children.0.retain_mut(|c| {
        if !c.ready {
            // A file, not a folder of that name: what the child wrote, whole.
            c.ready = files.as_ref().is_some_and(|file| member_file(file, &c.name).is_file());
        }
        match c.child.try_wait() {
            Ok(Some(status)) => {
                left.push(verdict(&c.name, status, Seen { ready: c.ready, joined: joined.contains(&c.name), session_over }));
                false
            }
            _ => true,
        }
    });
    world.insert_resource(children);
    let mut ended = false;
    for v in left {
        if v.fatal {
            fail_stand(world, &v.text, v.code);
            ended = true;
        } else {
            journal::emit(world, JournalKind::Warn, format!("stand: {}", v.text));
        }
    }
    if ended {
        shards_core::run_exit_hooks(world);
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

    fn seen(ready: bool, joined: bool, session_over: bool) -> Seen {
        Seen { ready, joined, session_over }
    }

    #[test]
    #[cfg(unix)]
    fn a_child_that_never_started_ends_the_stand_with_its_exit_status() {
        // Exited with code 3 before its port file appeared (a busy port, a bad flag).
        assert_eq!(
            verdict("c2", status_of("exit 3"), seen(false, false, false)),
            Verdict {
                fatal: true,
                code: 3,
                text: "client c2 exited with code 3 before it was ready (it never wrote its port file): see its output above".into()
            }
        );
        // Code 0 before it was ready is still a failure (exit code 2), and so is a signal.
        let v = verdict("c1", status_of("exit 0"), seen(false, false, false));
        assert!(v.fatal && v.code == 2 && v.text.starts_with("client c1 exited with code 0 before it was ready"), "{v:?}");
        let v = verdict("c1", status_of("kill -TERM $$"), seen(false, false, false));
        assert!(v.fatal && v.code == 2 && v.text.starts_with("client c1 was ended by signal 15 before it was ready"), "{v:?}");
    }

    /// A client the host refused (`--stand 3` with room for two) wrote its port file and then left
    /// with code 0: that is a stand that is not the one asked for.
    #[test]
    #[cfg(unix)]
    fn a_child_that_never_joined_ends_the_stand_too() {
        let v = verdict("c3", status_of("exit 0"), seen(true, false, false));
        assert!(v.fatal && v.code == 2, "{v:?}");
        assert!(v.text.starts_with("client c3 exited with code 0 without joining the host (it may have been refused: is the session full?)"), "{}", v.text);
        let v = verdict("c3", status_of("exit 4"), seen(true, false, false));
        assert!(v.fatal && v.code == 4, "its own code");
    }

    #[test]
    #[cfg(unix)]
    fn a_child_that_leaves_on_purpose_is_only_journaled_and_one_that_crashes_is_not() {
        // Joined, then left the way a game does: code 0, or the SIGTERM it turns into 143.
        for script in ["exit 0", "exit 143", "exit 130", "kill -TERM $$"] {
            let v = verdict("c1", status_of(script), seen(true, true, false));
            assert!(!v.fatal && v.code == 0 && v.text.ends_with("while the stand ran"), "{script}: {v:?}");
        }
        // A crash after joining: a panic's code 101, any other code, a signal that is not a request to end.
        let v = verdict("c1", status_of("exit 101"), seen(true, true, false));
        assert_eq!(
            v,
            Verdict { fatal: true, code: 101, text: "client c1 exited with code 101 while the stand ran: it crashed, see its output above".into() }
        );
        let v = verdict("c1", status_of("kill -SEGV $$"), seen(true, true, false));
        assert!(v.fatal && v.code == 2 && v.text.starts_with("client c1 was ended by signal 11 while the stand ran: it crashed"), "{v:?}");
        // The session is over: clients are meant to leave, whatever they did and however early.
        for (ready, joined) in [(false, false), (true, false), (true, true)] {
            let v = verdict("c1", status_of("exit 1"), seen(ready, joined, true));
            assert_eq!(v, Verdict { fatal: false, code: 0, text: "client c1 exited with code 1 after the session ended".into() });
        }
    }

    #[test]
    fn the_session_needs_room_for_the_clients_asked_for() {
        // The host is player 0: max_players 3 holds two clients.
        assert_eq!(room_for(1, 3), Ok(()));
        assert_eq!(room_for(2, 3), Ok(()));
        let e = room_for(3, 3).unwrap_err();
        assert_eq!(
            e,
            "--stand 3 needs room for 4 players, but the session allows 3 (`max_players` of the net definition): \
             ask for --stand 2 or fewer, or raise max_players"
        );
        assert!(room_for(255, 16).is_err() && room_for(15, 16).is_ok());
    }

    #[test]
    fn a_port_file_left_by_an_earlier_stand_is_removed_before_the_children_start() {
        let dir = std::env::temp_dir().join(format!("shards_stand_stale_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let host = dir.join("g.port");
        std::fs::write(&host, "1\n").unwrap();
        for name in ["c1", "c2", "c3", "c4"] {
            std::fs::write(member_file(&host, name), "1\n").unwrap();
        }
        // A folder is not removed (the child cannot write it: that is its own failure), and a
        // gap ends the sweep past this stand's clients.
        std::fs::create_dir(member_file(&host, "c6")).unwrap();
        std::fs::write(member_file(&host, "c7"), "1\n").unwrap();
        remove_stale_files(&host, 2);
        assert!(host.is_file(), "the host's own file is not touched");
        for name in ["c1", "c2", "c3", "c4"] {
            assert!(!member_file(&host, name).exists(), "{name}");
        }
        assert!(member_file(&host, "c6").is_dir() && member_file(&host, "c7").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A pretend host: `watch_children` alone in `Last`, a probe for an exit hook, and a real child
    /// process (`sh -c script`) standing for client `c1`. Runs frames until the child is gone.
    #[cfg(unix)]
    fn watch(script: &str, ready: bool, joined: bool) -> (App, usize) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static HOOKED: AtomicUsize = AtomicUsize::new(0);
        fn probe(_: &mut World) {
            HOOKED.fetch_add(1, Ordering::SeqCst);
        }
        HOOKED.store(0, Ordering::SeqCst);
        // A folder of its own, so no port file of another run is taken for the child's.
        let dir = std::env::temp_dir().join(format!("shards_stand_watch_{}", std::process::id()));
        let mut app = App::new();
        app.insert_resource(StandSpec { port_file: Some(dir.join("g.port")), ..spec() })
            .init_resource::<StandChildren>()
            .add_systems(Last, watch_children)
            .add_exit_hook("probe", probe);
        if joined {
            let mut host = NetHost::new();
            host.slots.insert(1, shards_net::host::Slot { name: "c1".into(), ..Default::default() });
            app.insert_resource(host);
        }
        let child = Command::new("sh").args(["-c", script]).stdin(Stdio::null()).spawn().expect("sh runs");
        app.world_mut().resource_mut::<StandChildren>().0.push(StandChild { name: "c1".into(), child, ready });
        let started = Instant::now();
        loop {
            app.update();
            if app.world().resource::<StandChildren>().0.is_empty() {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(20), "the child is still there");
            std::thread::sleep(Duration::from_millis(10));
        }
        let hooked = HOOKED.load(Ordering::SeqCst);
        (app, hooked)
    }

    /// The exit hooks run in the frame that ends the stand, by the system that ends it: nothing
    /// orders `run_exit_hooks` against it, and a host that left in a frame where the hooks came
    /// first would skip the session's end, the saves and the other children.
    #[test]
    #[cfg(unix)]
    fn a_failing_child_ends_the_stand_and_the_exit_hooks_run_once_with_it() {
        let (mut app, hooked) = watch("exit 3", false, false);
        assert_eq!(hooked, 1, "the hooks ran with the failure, without a run_exit_hooks system in the app");
        assert!(matches!(app.should_exit(), Some(AppExit::Error(code)) if code.get() == 3), "{:?}", app.should_exit());
        // Whatever ends the game afterwards does not run them again.
        shards_core::run_exit_hooks(app.world_mut());
        app.update();
        assert!(app.world().resource::<shards_core::ExitHooks>().ran());
        // A refused client: its file appeared, the host never admitted it.
        let (app, hooked) = watch("exit 0", true, false);
        assert_eq!(hooked, 1);
        assert!(matches!(app.should_exit(), Some(AppExit::Error(code)) if code.get() == 2), "{:?}", app.should_exit());
        // A joined client that crashed.
        let (app, hooked) = watch("exit 101", true, true);
        assert_eq!(hooked, 1);
        assert!(matches!(app.should_exit(), Some(AppExit::Error(code)) if code.get() == 101), "{:?}", app.should_exit());
    }

    #[test]
    #[cfg(unix)]
    fn a_client_that_joined_and_leaves_on_purpose_does_not_end_the_stand() {
        let (app, hooked) = watch("exit 0", true, true);
        assert_eq!(hooked, 0);
        assert_eq!(app.should_exit(), None);
    }
}
