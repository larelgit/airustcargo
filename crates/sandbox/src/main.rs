use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ai_core::replay::{self, Replay};
use ai_core::AiReplay;
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
    /// the game still matches the recording, exit 0 (same) or 1 (diverged).
    #[arg(long, requires = "replay")]
    verify: bool,
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
    let mut cfg =
        GameConfig { headless: args.headless, seed: args.seed, port: None, paused: args.paused, tick_hz: args.hz };
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
    let mut app = build_app(&cfg);
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

/// Plays a replay headless, as fast as the CPU allows, and reports.
fn verify(cfg: GameConfig, replay: Replay) -> ExitCode {
    let cfg = GameConfig { headless: true, port: None, paused: true, ..cfg };
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
    if let Err(e) = ai_core::sim::step(app.world_mut(), end, Duration::from_secs(24 * 3600), |_| None) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    app.update(); // calls made after the last tick, then the final comparison
    let Some(report) = app.world().resource::<AiReplay>().report().cloned() else {
        eprintln!("error: the replay did not finish");
        return ExitCode::from(2);
    };
    println!("{report} [{end} ticks in {:.2}s]", started.elapsed().as_secs_f64());
    if report.identical() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
