use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ai_core::replay::{self, Replay};
use ai_core::sim::{StepEnd, StepReport};
use ai_core::{AiReplay, CheckDetail};
use bevy::app::AppExit;
use clap::Parser;
use sandbox::{build_app, GameConfig};

/// Sandbox arena for the ai_core layer.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// No window and no rendering (CI, parallel playtests).
    #[arg(long)]
    headless: bool,
    /// World seed: same seed + same inputs = same game.
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// BRP port for `shards` (use different ports for parallel instances; 0 picks a
    /// free one). A busy port is an error: exit code 3.
    #[arg(long, default_value_t = ai_core::DEFAULT_PORT)]
    port: u16,
    /// Write the BRP port into this file once the game starts (removed on exit), for
    /// `shards --port-file` / SHARDS_PORT_FILE. Pairs well with `--port 0`.
    #[arg(long, value_name = "PATH")]
    port_file: Option<PathBuf>,
    /// Do not start the BRP server.
    #[arg(long)]
    no_remote: bool,
    /// Start paused; advance with `shards step`.
    #[arg(long)]
    paused: bool,
    /// Simulation ticks per second.
    #[arg(long, default_value_t = 60.0)]
    hz: f64,
    /// Play a replay back (`shards save-replay`). Seed and tick rate come from the
    /// file; when it ends, the game goes live.
    #[arg(long, value_name = "FILE")]
    replay: Option<PathBuf>,
    /// With --replay: no window, run it to the end as fast as possible, print whether
    /// the game still matches the recording and every invariant it broke on the way,
    /// exit 0 (same, nothing broken) or 1 (diverged, a system failed, an invariant broke).
    #[arg(long, requires = "replay")]
    verify: bool,
    /// Ticks between replay checks, stored in the replay. Record with 1 and `--verify`
    /// names the exact tick a changed build diverges at. A replay checks at its own.
    #[arg(
        long,
        value_name = "TICKS",
        default_value_t = ai_core::replay::CHECK_EVERY,
        value_parser = clap::value_parser!(u64).range(1..),
        conflicts_with = "replay"
    )]
    check_every: u64,
    /// What each replay check keeps to name what diverged: components (entities,
    /// components and their values), entities, or none (the tick only).
    #[arg(long, value_name = "DETAIL", default_value_t = CheckDetail::Components, conflicts_with = "replay")]
    check_detail: CheckDetail,
    /// A tick slower than this is over budget: `step` counts it and the journal names
    /// it (`perf`). Default: the tick's own duration (16.7 ms at 60 Hz). Debug builds
    /// run slower than `--release`.
    #[arg(long, value_name = "MS", value_parser = positive_ms)]
    tick_budget_ms: Option<f64>,
}

fn positive_ms(text: &str) -> Result<f64, String> {
    match text.parse::<f64>() {
        Ok(ms) if ms > 0.0 && ms.is_finite() => Ok(ms),
        Ok(_) => Err("the budget must be above 0 ms".into()),
        Err(_) => Err(format!("`{text}` is not a number of milliseconds")),
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    let mut replay = match args.replay.as_deref().map(replay::load).transpose() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut cfg = GameConfig {
        headless: args.headless,
        seed: args.seed,
        port: None,
        paused: args.paused,
        tick_hz: args.hz,
        crash_replays: true,
        check_every: args.check_every,
        check_detail: args.check_detail,
        tick_budget: args.tick_budget_ms.map(|ms| Duration::from_secs_f64(ms / 1000.0)),
        report_perf: true,
    };
    if let Some(r) = &replay {
        cfg.seed = r.seed;
        cfg.tick_hz = r.tick_hz;
    }
    if args.verify
        && let Some(r) = replay.take()
    {
        return verify(cfg, r);
    }
    // Before anything is built: a second game on a busy port would run without a
    // server while `shards` quietly drives the first one.
    cfg.port = match remote_port(&args) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(3);
        }
    };
    // From here on `kill` ends the game through `AppExit`, like Ctrl+C: the token
    // and the port file are removed on the way out.
    termination::catch_signals();
    let mut app = build_app(&cfg);
    app.add_systems(bevy::app::First, termination::exit_when_signalled);
    if let Some(r) = replay
        && let Err(e) = replay::play(app.world_mut(), r)
    {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    let _port_file = match write_port_file(&args, cfg.port) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    println!("{}", ai_core::ready_line(0, std::process::id(), cfg.seed, cfg.port));
    exit_code(app.run())
}

/// The port the BRP server will listen on: `--port`, or a free one for `--port 0`.
#[cfg(feature = "remote")]
fn remote_port(args: &Args) -> Result<Option<u16>, String> {
    if args.no_remote {
        return Ok(None);
    }
    let port = if args.port == 0 { ai_core::remote::free_port()? } else { args.port };
    ai_core::remote::ensure_port_free(port)?;
    Ok(Some(port))
}

#[cfg(not(feature = "remote"))]
fn remote_port(_: &Args) -> Result<Option<u16>, String> {
    Ok(None)
}

#[cfg(feature = "remote")]
fn write_port_file(args: &Args, port: Option<u16>) -> Result<Option<ai_core::remote::PortFile>, String> {
    match (&args.port_file, port) {
        (Some(path), Some(port)) => ai_core::remote::PortFile::write(path, port).map(Some),
        (Some(_), None) => Err("--port-file needs the BRP server (drop --no-remote)".into()),
        (None, _) => Ok(None),
    }
}

#[cfg(not(feature = "remote"))]
fn write_port_file(args: &Args, _: Option<u16>) -> Result<Option<()>, String> {
    match args.port_file {
        Some(_) => Err("--port-file needs the BRP server: build with the `remote` feature".into()),
        None => Ok(None),
    }
}

fn exit_code(exit: AppExit) -> ExitCode {
    match exit {
        AppExit::Success => ExitCode::SUCCESS,
        AppExit::Error(code) => ExitCode::from(code.get()),
    }
}

/// Plays a replay headless, as fast as the CPU allows, and reports. A gameplay system
/// that fails on the way counts as a difference (exit 1), even when the state matches:
/// a crash replay reproduces its crash, a regular one must not crash. So does every
/// invariant broken on the way: each is listed, and playback goes on to the end.
fn verify(cfg: GameConfig, replay: Replay) -> ExitCode {
    // Verifying writes nothing, not even a crash replay over the one being verified.
    let cfg = GameConfig { headless: true, port: None, paused: true, crash_replays: false, ..cfg };
    let end = replay.end.tick;
    let mut app = build_app(&cfg);
    app.finish();
    app.cleanup();
    app.update(); // Startup: the arena exists, tick 0
    if let Err(e) = replay::play(app.world_mut(), replay) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    let started = Instant::now();
    let mut violations = Vec::new();
    let mut failures = Vec::new();
    while ai_core::sim::tick(app.world()) < end {
        let left = end - ai_core::sim::tick(app.world());
        match ai_core::sim::step(app.world_mut(), left, Duration::from_secs(24 * 3600), |_| None) {
            Ok(StepReport { end: StepEnd::SystemFailed(failed), .. }) => {
                failures = failed;
                break;
            }
            // A rule that fired stays off unless the recording re-armed it: go on.
            Ok(StepReport { end: StepEnd::InvariantViolated(violated), .. }) => violations.extend(violated),
            Ok(_) => break,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        }
    }
    app.update(); // calls made after the last tick, then the final comparison
    for v in &violations {
        println!("{}: {}", v.headline(), v.text);
    }
    let report = app.world().resource::<AiReplay>().report().cloned();
    let broken = match violations.len() {
        0 => String::new(),
        1 => "; 1 invariant violated".to_owned(),
        n => format!("; {n} invariants violated"),
    };
    let took = format!("[{} ticks in {:.2}s]", ai_core::sim::tick(app.world()), started.elapsed().as_secs_f64());
    // What diverged, then its values in this run: `  e6 now: Health { … }`.
    let values = |report: &Option<ai_core::ReplayReport>| {
        for line in report.iter().flat_map(|r| r.value_lines()) {
            println!("  {line}");
        }
    };
    match (&report, failures.first()) {
        (_, Some(failure)) => {
            let shown = report.as_ref().map(|r| format!("{}; ", r.headline())).unwrap_or_default();
            println!("{shown}gameplay {}: {}{broken} {took}", failure.headline(), failure.message);
            values(&report);
            ExitCode::FAILURE
        }
        (Some(r), None) => {
            println!("{}{broken} {took}", r.headline());
            values(&report);
            if r.identical() && violations.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        (None, None) => {
            eprintln!("error: the replay did not finish");
            ExitCode::from(2)
        }
    }
}

/// `kill` (SIGTERM) and a closed terminal (SIGHUP) end the game the way Ctrl+C does:
/// through `AppExit`, so what it wrote at start (the BRP token, `--port-file`) is
/// removed. A signal the game was started with ignored (`nohup`) stays ignored; a
/// second signal exits at once.
mod termination {
    use std::sync::atomic::{AtomicI32, Ordering};

    use bevy::app::AppExit;
    use bevy::ecs::message::MessageWriter;

    /// The first termination signal received; 0: none yet.
    static RECEIVED: AtomicI32 = AtomicI32::new(0);

    #[cfg(unix)]
    pub fn catch_signals() {
        let handler: extern "C" fn(libc::c_int) = on_signal;
        for signal in [libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: plain `sigaction` calls with valid pointers; the handler only
            // makes async-signal-safe calls.
            unsafe {
                let mut old: libc::sigaction = std::mem::zeroed();
                if libc::sigaction(signal, std::ptr::null(), &mut old) != 0 || old.sa_sigaction == libc::SIG_IGN {
                    continue;
                }
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = handler as libc::sighandler_t;
                action.sa_flags = libc::SA_RESTART;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(signal, &action, std::ptr::null_mut());
            }
        }
    }

    #[cfg(not(unix))]
    pub fn catch_signals() {}

    #[cfg(unix)]
    extern "C" fn on_signal(signal: libc::c_int) {
        if RECEIVED.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            // SAFETY: `_exit` is async-signal-safe (unlike `std::process::exit`).
            unsafe { libc::_exit(128 + signal) };
        }
    }

    /// Exit code 128 + signal, as a shell reports a process the signal killed.
    pub fn exit_when_signalled(mut exit: MessageWriter<AppExit>) {
        let signal = RECEIVED.load(Ordering::SeqCst);
        if signal != 0 {
            exit.write(AppExit::from_code((128 + signal) as u8));
        }
    }
}
