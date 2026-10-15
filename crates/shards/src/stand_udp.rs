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
//! session was full, or the host turned it away), or crashes later, ends the stand at once with that child's exit status and
//! a line saying which one; a child that joined and leaves on purpose (exit code 0, SIGTERM,
//! SIGINT, SIGHUP) or whose link the host closed (`net_kick`, a timeout) is journaled. `--stand N`
//! needs room for N clients in the session (`max_players` of the net definition): the host
//! checks it as soon as content is loaded, and starts no child before.

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

/// Ctrl+Break to the child's own process group (it leads one: see [`start_children`]), which Bevy's
/// Ctrl+C handler turns into a normal exit, as SIGTERM is on unix.
#[cfg(windows)]
fn signal(child: &Child) {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GenerateConsoleCtrlEvent(event: u32, group: u32) -> i32;
    }
    const CTRL_BREAK_EVENT: u32 = 1;
    // SAFETY: takes no pointers; a host without a console only gets an error back.
    unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
}

#[cfg(not(any(unix, windows)))]
fn signal(_: &Child) {}

/// Makes `app` the host of a stand over UDP: its clients start once its content is loaded (and
/// the session has room for them) and are let go when it exits.
pub fn install(app: &mut App, spec: StandSpec) {
    app.insert_resource(spec)
        .init_resource::<StandChildren>()
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

/// Starts the clients, from `watch_children` on the first frame after content is loaded and the
/// session was found to have room for them. The host's socket is open (`Startup`), so the
/// clients can be told where it is.
fn start_children(world: &mut World) {
    let spec = world.resource::<StandSpec>().clone();
    let port = world
        .get_resource::<NetEndpoint>()
        .and_then(|endpoint| endpoint.transport.local_addr())
        .and_then(|addr| addr.rsplit(':').next().and_then(|port| port.parse::<u16>().ok()));
    let Some(port) = port else {
        end_stand(world, "the host has no UDP socket, so its clients cannot be started", 2);
        return;
    };
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            end_stand(world, &format!("cannot find this program to start the clients: {e}"), 2);
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
        // Its own process group, so that Ctrl+Break reaches it alone (`signal`).
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
            command.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }
        match command.spawn() {
            Ok(child) => {
                journal::emit(world, JournalKind::Sim, format!("stand: c{n} started (pid {}), joining {addr}", child.id()));
                children.push(StandChild { name: format!("c{n}"), child, ready: !waits_for_files });
            }
            Err(e) => {
                world.insert_resource(StandChildren(children));
                end_stand(world, &format!("cannot start c{n} ({}): {e}", exe.display()), 2);
                return;
            }
        }
    }
    // Inserted, not written into the resource: the exit hooks take it away, and a start that
    // follows them must not be a panic.
    world.insert_resource(StandChildren(children));
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
    /// The host had admitted it to the session (it got a body). A client the host turned away
    /// after its hello has a slot, and never was; nor was one the host kicked or silenced while
    /// it was still joining.
    joined: bool,
    /// The host closed the client's link before it exited (`net_kick`, its silence timed out): a
    /// client that was admitted and leaves this way exits with an error code of its own, which is
    /// not a crash. It excuses only an admitted client: a kick or a silence during the join is
    /// still a client that never joined. A goodbye the client sent is not that either: it says
    /// nothing about why it went. Nor is a refusal.
    host_closed: bool,
    /// The host's session has ended: its clients are leaving, as they should.
    session_over: bool,
}

/// `name` exited with `status`. A child that leaves on purpose after joining (exit code 0, or
/// the code of a signal that ends a game: SIGTERM, SIGINT, SIGHUP) or whose link the host closed
/// is journaled. One that never started, never joined (the session was full, or it was turned away) or crashed ends the
/// stand, whatever goodbye it said on the way out.
fn verdict(name: &str, status: ExitStatus, seen: Seen) -> Verdict {
    let how = match (status.code(), exit_signal(status)) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("was ended by signal {signal}"),
        (None, None) => "exited".to_owned(),
    };
    let on_purpose = left_on_purpose(status);
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
                "client {name} {how} without being admitted to the host (refused, kicked or timed out while joining: is the session full?): see the host's journal and its output above"
            ),
        };
    }
    if on_purpose {
        return Verdict { fatal: false, code: 0, text: format!("client {name} {how} while the stand ran") };
    }
    if seen.host_closed {
        return Verdict { fatal: false, code: 0, text: format!("client {name} {how} after the host closed its link to it (net_kick, or its silence timed out)") };
    }
    Verdict { fatal: true, code, text: format!("client {name} {how} while the stand ran: it crashed, see its output above") }
}

/// Whether a game that exited with `status` left the way one does when it is asked to: code 0, or
/// 128 + a signal that ends a game (SIGTERM and SIGHUP, which it catches, and Ctrl+C's SIGINT),
/// or that signal itself.
#[cfg(unix)]
fn left_on_purpose(status: ExitStatus) -> bool {
    let ends_a_game = |signal: i32| signal == libc::SIGINT || crate::run::termination::CAUGHT.contains(&signal);
    match (status.code(), exit_signal(status)) {
        (Some(0), _) => true,
        (Some(code), _) => code > 128 && ends_a_game(code - 128),
        (None, Some(signal)) => ends_a_game(signal),
        (None, None) => false,
    }
}

/// Windows: code 0, 129 (its console closed) or 130 (Ctrl+C or Ctrl+Break, Bevy's handler).
#[cfg(not(unix))]
fn left_on_purpose(status: ExitStatus) -> bool {
    matches!(status.code(), Some(0 | 129 | 130))
}

#[cfg(unix)]
fn exit_signal(status: ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&status)
}

#[cfg(not(unix))]
fn exit_signal(_: ExitStatus) -> Option<i32> {
    None
}

/// `Last`: on the first frame after content is loaded, checks that the session has room for the
/// clients and starts them; from then on, the children that exited since the last frame. One that
/// never started or joined, or crashed, ends the stand with its exit status; the others are
/// journaled.
fn watch_children(world: &mut World, mut started: Local<bool>) {
    if !*started && world.get_resource::<State<EngineState>>().is_some_and(|state| *state.get() != EngineState::Loading) {
        *started = true;
        // A stand that has already ended (a signal in the frame loading finished ran the exit
        // hooks, which let the children go and took the resource) starts nothing.
        if world.get_resource::<shards_core::ExitHooks>().is_some_and(|hooks| hooks.ran()) || !world.contains_resource::<StandChildren>() {
            return;
        }
        let clients = world.resource::<StandSpec>().clients;
        if let Err(why) = room_for(clients, net_def(world).max_players) {
            end_stand(world, &why, 2);
            return;
        }
        start_children(world);
    }
    let Some(mut children) = world.remove_resource::<StandChildren>() else { return };
    let files = world.resource::<StandSpec>().port_file.clone();
    let host = world.get_resource::<NetHost>();
    let session_over = host.is_some_and(|host| host.ended.is_some());
    let mut left = Vec::new();
    children.0.retain_mut(|c| {
        if !c.ready {
            // A file, not a folder of that name: what the child wrote, whole.
            c.ready = files.as_ref().is_some_and(|file| member_file(file, &c.name).is_file());
        }
        match c.child.try_wait() {
            Ok(Some(status)) => {
                let joined = host.is_some_and(|host| host.slots.values().any(|slot| slot.name == c.name && slot.admitted));
                let host_closed = host.is_some_and(|host| host.link_closed_by_host(&c.name));
                left.push(verdict(&c.name, status, Seen { ready: c.ready, joined, host_closed, session_over }));
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
        eprintln!(
            "warning: stand client {} (pid {}) is still running after it was told to leave; end it with `shards quit` (its port file) or `kill {}` on unix",
            c.name,
            c.child.id(),
            c.child.id()
        );
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
        Seen { ready, joined, host_closed: false, session_over }
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
        assert!(
            v.text.starts_with("client c3 exited with code 0 without being admitted to the host (refused, kicked or timed out while joining: is the session full?)"),
            "{}",
            v.text
        );
        let v = verdict("c3", status_of("exit 4"), seen(true, false, false));
        assert!(v.fatal && v.code == 4, "its own code");
    }

    #[test]
    #[cfg(unix)]
    fn a_child_that_leaves_on_purpose_is_only_journaled_and_one_that_crashes_is_not() {
        // Joined, then left the way a game does when it is asked to: code 0, or 128 + the signal
        // that ends a game (SIGTERM 143, SIGINT 130, SIGHUP 129), or that signal itself.
        for script in ["exit 0", "exit 143", "exit 130", "exit 129", "kill -TERM $$", "kill -INT $$", "kill -HUP $$"] {
            let v = verdict("c1", status_of(script), seen(true, true, false));
            assert!(!v.fatal && v.code == 0 && v.text.ends_with("while the stand ran"), "{script}: {v:?}");
        }
        // A crash after joining: a panic's code 101, any other code, a signal that is not a request to end.
        let v = verdict("c1", status_of("exit 101"), seen(true, true, false));
        assert_eq!(
            v,
            Verdict { fatal: true, code: 101, text: "client c1 exited with code 101 while the stand ran: it crashed, see its output above".into() }
        );
        let v = verdict("c1", status_of("exit 1"), seen(true, true, false));
        assert!(v.fatal && v.code == 1, "an error exit nobody asked for is a crash: {v:?}");
        let v = verdict("c1", status_of("kill -SEGV $$"), seen(true, true, false));
        assert!(v.fatal && v.code == 2 && v.text.starts_with("client c1 was ended by signal 11 while the stand ran: it crashed"), "{v:?}");
        // The session is over: clients are meant to leave, whatever they did and however early.
        for (ready, joined) in [(false, false), (true, false), (true, true)] {
            let v = verdict("c1", status_of("exit 1"), seen(ready, joined, true));
            assert_eq!(v, Verdict { fatal: false, code: 0, text: "client c1 exited with code 1 after the session ended".into() });
        }
    }

    /// A client whose link the host closed (`net_kick`, a timeout) leaves with an error code of its
    /// own (`--exit-with-host`, `client::end`): the host knows why it went, so it is not a crash. A
    /// deliberate exit keeps its own line, whoever closed the link.
    #[test]
    #[cfg(unix)]
    fn a_client_whose_link_the_host_closed_is_only_journaled_whatever_code_it_leaves_with() {
        let closed = Seen { ready: true, joined: true, host_closed: true, session_over: false };
        let v = verdict("c1", status_of("exit 1"), closed);
        assert_eq!(
            v,
            Verdict {
                fatal: false,
                code: 0,
                text: "client c1 exited with code 1 after the host closed its link to it (net_kick, or its silence timed out)".into()
            }
        );
        // A deliberate exit is that, and says so: the host having closed the link does not rename it.
        for script in ["exit 0", "exit 143", "kill -TERM $$"] {
            let v = verdict("c1", status_of(script), closed);
            assert!(!v.fatal && v.code == 0 && v.text.ends_with("while the stand ran"), "{script}: {v:?}");
        }
    }

    /// A client that says goodbye and then exits with an error did not leave because the host asked:
    /// `take_world` sends `Bye` ("cannot take the world") and exits 1, and that is a dead member.
    #[test]
    #[cfg(unix)]
    fn a_client_that_said_goodbye_and_exited_with_an_error_is_a_crash() {
        let said_goodbye = Seen { ready: true, joined: true, host_closed: false, session_over: false };
        let v = verdict("c1", status_of("exit 1"), said_goodbye);
        assert!(v.fatal && v.code == 1, "{v:?}");
        assert!(v.text.ends_with("while the stand ran: it crashed, see its output above"), "{}", v.text);
        // ... and one that says goodbye on a deliberate exit (a SIGTERM, `quit`) left on purpose.
        let v = verdict("c1", status_of("exit 143"), said_goodbye);
        assert!(!v.fatal && v.text == "client c1 exited with code 143 while the stand ran", "{v:?}");
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

    /// How many times the exit hooks ran in a pretend host's world: a count of its own, so tests that
    /// run side by side (the harness runs them on threads) never share one.
    #[derive(Resource, Default)]
    struct Hooked(usize);

    fn probe(world: &mut World) {
        world.resource_mut::<Hooked>().0 += 1;
    }

    /// What the host's session says about client `c1`.
    #[derive(Clone, Copy, PartialEq)]
    enum Session {
        /// It never joined: no slot (the session was full).
        Never,
        /// The host turned it away after its hello: it has a slot, and was never admitted. A refusal
        /// records no closer.
        Refused,
        /// The host kicked it, or its silence timed out, while it was still joining: it has a slot that
        /// says the host closed the link, and was never admitted.
        KickedWhileJoining,
        /// It joined and is in the session.
        Joined,
        /// It joined and the host closed its link (the slot is vacated).
        HostClosed,
        /// It joined and said goodbye itself (the slot is vacated).
        SaidGoodbye,
    }

    /// A pretend host: `watch_children` alone in `Last`, a probe for an exit hook, and a real child
    /// process (`sh -c script`) standing for client `c1`. Runs frames until the child is gone.
    #[cfg(unix)]
    fn watch(script: &str, ready: bool, session: Session) -> App {
        // A folder of its own, so no port file of another run is taken for the child's.
        let dir = std::env::temp_dir().join(format!("shards_stand_watch_{}", std::process::id()));
        let mut app = App::new();
        app.insert_resource(StandSpec { port_file: Some(dir.join("g.port")), ..spec() })
            .init_resource::<StandChildren>()
            .init_resource::<Hooked>()
            .add_systems(Last, watch_children)
            .add_exit_hook("probe", probe);
        if session != Session::Never {
            let mut host = NetHost::new();
            let (left_at, closed_by) = match session {
                // The host closed the link of a game that was never admitted: it never joined, and a
                // kick or a silence does not excuse that.
                Session::KickedWhileJoining => (None, Some(shards_net::host::Closer::Host)),
                Session::HostClosed => (Some(5), Some(shards_net::host::Closer::Host)),
                Session::SaidGoodbye => (Some(5), Some(shards_net::host::Closer::Peer)),
                _ => (None, None),
            };
            let admitted = !matches!(session, Session::Refused | Session::KickedWhileJoining);
            host.slots.insert(1, shards_net::host::Slot { name: "c1".into(), left_at, closed_by, admitted, ..Default::default() });
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
        app
    }

    fn exits_with(app: &App, code: u8) -> bool {
        matches!(app.should_exit(), Some(AppExit::Error(c)) if c.get() == code)
    }

    /// The exit hooks run in the frame that ends the stand, by the system that ends it: nothing
    /// orders `run_exit_hooks` against it, and a host that left in a frame where the hooks came
    /// first would skip the session's end, the saves and the other children.
    #[test]
    #[cfg(unix)]
    fn a_failing_child_ends_the_stand_and_the_exit_hooks_run_once_with_it() {
        let mut app = watch("exit 3", false, Session::Never);
        assert_eq!(app.world().resource::<Hooked>().0, 1, "the hooks ran with the failure, without a run_exit_hooks system in the app");
        assert!(exits_with(&app, 3), "{:?}", app.should_exit());
        // Whatever ends the game afterwards does not run them again.
        shards_core::run_exit_hooks(app.world_mut());
        app.update();
        assert_eq!(app.world().resource::<Hooked>().0, 1, "once");
        assert!(app.world().resource::<shards_core::ExitHooks>().ran());
        // A refused client: its file appeared, the host never admitted it.
        let app = watch("exit 0", true, Session::Never);
        assert_eq!(app.world().resource::<Hooked>().0, 1);
        assert!(exits_with(&app, 2), "{:?}", app.should_exit());
        // A client the host turned away after its hello (the slot exists, nobody was admitted, no
        // closer is recorded), and one the host kicked or silenced while it was joining (the slot says
        // the host closed the link): neither ever joined, and both end the stand.
        for session in [Session::Refused, Session::KickedWhileJoining] {
            let app = watch("exit 1", true, session);
            assert_eq!(app.world().resource::<Hooked>().0, 1);
            assert!(exits_with(&app, 1), "{:?}", app.should_exit());
        }
        // A joined client that crashed.
        let app = watch("exit 101", true, Session::Joined);
        assert_eq!(app.world().resource::<Hooked>().0, 1);
        assert!(exits_with(&app, 101), "{:?}", app.should_exit());
    }

    #[test]
    #[cfg(unix)]
    fn a_client_that_joined_and_leaves_on_purpose_does_not_end_the_stand() {
        for script in ["exit 0", "exit 129"] {
            let app = watch(script, true, Session::Joined);
            assert_eq!(app.world().resource::<Hooked>().0, 0, "{script}");
            assert_eq!(app.should_exit(), None, "{script}");
        }
    }

    /// `net_kick` of a client of a UDP stand: it exits with code 1, the host closed its link, and
    /// the stand goes on for the others.
    #[test]
    #[cfg(unix)]
    fn a_client_the_host_kicked_does_not_end_the_stand() {
        let app = watch("exit 1", true, Session::HostClosed);
        assert_eq!(app.world().resource::<Hooked>().0, 0);
        assert_eq!(app.should_exit(), None);
        // The same exit from a client the host still holds is a crash.
        let app = watch("exit 1", true, Session::Joined);
        assert_eq!(app.world().resource::<Hooked>().0, 1);
        assert!(exits_with(&app, 1), "{:?}", app.should_exit());
        // So is the same exit from one that said goodbye itself (it could not take the world).
        let app = watch("exit 1", true, Session::SaidGoodbye);
        assert_eq!(app.world().resource::<Hooked>().0, 1);
        assert!(exits_with(&app, 1), "{:?}", app.should_exit());
        // ... while a deliberate exit after a goodbye is only journaled.
        let app = watch("exit 143", true, Session::SaidGoodbye);
        assert_eq!(app.should_exit(), None);
    }

    /// A pretend host that has left `Loading`: a stand whose clients `watch_children` starts.
    #[cfg(unix)]
    fn in_game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::state::app::StatesPlugin)
            .insert_state(EngineState::InGame)
            .insert_resource(spec())
            .init_resource::<StandChildren>()
            .init_resource::<Hooked>()
            .add_systems(Last, watch_children)
            .add_exit_hook("probe", probe);
        app
    }

    /// A signal in the frame in which loading finishes runs the exit hooks (which take the
    /// children away) before `watch_children` sees the state change: it must start nothing, and
    /// not panic on the resource the hooks removed.
    #[test]
    #[cfg(unix)]
    fn a_stand_that_ended_in_the_frame_loading_finished_starts_no_clients() {
        let mut app = in_game();
        shards_core::run_exit_hooks(app.world_mut());
        app.world_mut().remove_resource::<StandChildren>();
        assert_eq!(app.world().resource::<Hooked>().0, 1);
        app.update();
        // Had it started the clients, the host's missing socket would have ended the stand again.
        assert_eq!(app.should_exit(), None, "nothing was started, so nothing failed");
        assert!(app.world().get_resource::<StandChildren>().is_none());
        assert_eq!(app.world().resource::<Hooked>().0, 1, "once");
        app.update();
    }

    /// The other side of that: a stand that has not ended starts its clients on the first frame
    /// after loading, and a host without a UDP socket says so and ends.
    #[test]
    #[cfg(unix)]
    fn a_stand_starts_its_clients_on_the_first_frame_after_loading() {
        let mut app = in_game();
        app.update();
        assert!(exits_with(&app, 2), "no socket to tell the clients about: {:?}", app.should_exit());
        assert_eq!(app.world().resource::<Hooked>().0, 1, "the hooks ran with it");
    }
}
