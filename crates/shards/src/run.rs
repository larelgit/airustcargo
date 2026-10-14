//! `shards::run`: the standard `main` of a Shards game.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ai_core::replay::{self, Replay};
use ai_core::sim::{StepEnd, StepReport};
use ai_core::AiReplay;
use bevy::app::AppExit;
use bevy::prelude::*;
use clap::Parser;
use shards_content::{asset_root, check_dir, ContentKinds};
use shards_core::EngineConfig;

/// A game on the Shards engine.
#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    /// No window and no rendering (CI, parallel playtests, servers).
    #[arg(long)]
    headless: bool,
    /// World seed: same seed + same inputs = same game.
    #[arg(long, default_value_t = EngineConfig::default().seed)]
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
    #[arg(long, default_value_t = EngineConfig::default().tick_hz)]
    hz: f64,
    /// Play a replay back (`shards save-replay`). Seed and tick rate come from the
    /// file; when it ends, the game goes live.
    #[arg(long, value_name = "FILE")]
    replay: Option<PathBuf>,
    /// With --replay: no window, load, run it to the end as fast as possible, print
    /// whether the game still matches the recording, exit 0 (same), 1 (diverged or a
    /// gameplay system failed) or 2 (could not play it).
    #[arg(long, requires = "replay")]
    verify: bool,
    /// Read, parse and validate every content file, print the problems, exit 0 (clean)
    /// or 1. Runs nothing.
    #[arg(long)]
    check_content: bool,
}

/// The standard game `main`: parses the command line into an [`EngineConfig`], calls
/// `build` with it (the game adds `EnginePlugins::new(config.clone())` and its own
/// plugins), and runs, verifies a replay or checks content. Refuses a busy BRP port
/// (exit code 3) before anything is built; `kill` and a closed terminal exit through
/// `AppExit` like Ctrl+C, so the token and the port file go away with the game.
pub fn run(build: impl Fn(&EngineConfig) -> App) -> ExitCode {
    let args = Args::parse();
    let mut recording = match args.replay.as_deref().map(replay::load).transpose() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut config = EngineConfig { headless: args.headless, seed: args.seed, port: None, paused: args.paused, tick_hz: args.hz, ..default() };
    if let Some(r) = &recording {
        config.seed = r.seed;
        config.tick_hz = r.tick_hz;
    }
    if args.check_content {
        return check_content(&build, config);
    }
    if args.verify
        && let Some(r) = recording.take()
    {
        return verify(&build, config, r);
    }
    // Before anything is built: a second game on a busy port would run without a
    // server while `shards` quietly drives the first one.
    config.port = match remote_port(&args) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(3);
        }
    };
    // From here on `kill` ends the game through `AppExit`, like Ctrl+C: the token
    // and the port file are removed on the way out.
    termination::catch_signals();
    let mut app = build(&config);
    app.add_systems(First, termination::exit_when_signalled);
    if let Some(r) = recording
        && let Err(e) = replay::play(app.world_mut(), r)
    {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    let _port_file = match write_port_file(&args, config.port) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    println!("{}", ai_core::ready_line(0, std::process::id(), config.seed, config.port));
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
/// a crash replay reproduces its crash, a regular one must not crash.
fn verify(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, recording: Replay) -> ExitCode {
    // Verifying writes nothing, not even a crash replay over the one being verified.
    let config = EngineConfig { headless: true, port: None, paused: true, crash_replays: false, hot_reload: false, ..config };
    let end = recording.end.tick;
    let mut app = build(&config);
    app.finish();
    app.cleanup();
    // Before loading, as a live `--replay` does: the world is spawned from the content
    // the recording started with.
    if let Err(e) = replay::play(app.world_mut(), recording) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    if let Err(e) = crate::testing::wait_until_loaded(&mut app, Duration::from_secs(60)) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    let started = Instant::now();
    let failures = match ai_core::sim::step(app.world_mut(), end, Duration::from_secs(24 * 3600), |_| None) {
        Ok(StepReport { end: StepEnd::SystemFailed(failures), .. }) => failures,
        Ok(_) => Vec::new(),
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    app.update(); // calls made after the last tick, then the final comparison
    let report = app.world().resource::<AiReplay>().report().cloned();
    let took = format!("[{} ticks in {:.2}s]", ai_core::sim::tick(app.world()), started.elapsed().as_secs_f64());
    match (&report, failures.first()) {
        (_, Some(failure)) => {
            let shown = report.as_ref().map(|r| format!("{r}; ")).unwrap_or_default();
            println!("{shown}gameplay {}: {} {took}", failure.headline(), failure.message);
            ExitCode::FAILURE
        }
        (Some(report), None) => {
            println!("{report} {took}");
            if report.identical() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        (None, None) => {
            eprintln!("error: the replay did not finish");
            ExitCode::from(2)
        }
    }
}

/// `--check-content`: every content file read from disk, parsed and validated; the
/// problems printed one per line. For CI.
fn check_content(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig) -> ExitCode {
    let config = EngineConfig { headless: true, testing: true, port: None, paused: true, crash_replays: false, hot_reload: false, ..config };
    let app = build(&config);
    let kinds = app.world().resource::<ContentKinds>().clone();
    let root = asset_root(app.world());
    match check_dir(&root, &kinds) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(errors) => {
            for error in &errors {
                println!("{error}");
            }
            println!("{} content errors in {}", errors.len(), root.display());
            ExitCode::FAILURE
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, ok};
    use crate::EnginePlugins;
    use serde_json::json;
    use shards_core::EngineState;

    /// A game that spawns its one totem from content, like games are told to.
    fn game(config: &EngineConfig) -> App {
        let mut app = App::new();
        app.add_plugins(EnginePlugins::new(config.clone())).add_systems(OnEnter(EngineState::InGame), |world: &mut World| {
            shards_content::spawn_prefab(world, "totem", Transform::default(), None).unwrap();
        });
        app
    }

    fn content(name: &str, totem: &str) -> EngineConfig {
        let dir = std::env::temp_dir().join(format!("shards_verify_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content")).unwrap();
        write_totem(&dir, totem);
        EngineConfig { asset_dir: dir.to_string_lossy().into_owned(), ..testing::config() }
    }

    fn write_totem(dir: &std::path::Path, name: &str) {
        let prefab = format!(r#"{{ "totem": (components: {{ "AiKind": ("Totem"), "Name": "{name}" }}) }}"#);
        std::fs::write(dir.join("content/totem.prefab.ron"), prefab).unwrap();
    }

    fn record(config: &EngineConfig, session: impl FnOnce(&mut App)) -> Replay {
        let mut app = game(config);
        app.finish();
        app.cleanup();
        testing::wait_until_loaded(&mut app, testing::LOAD_TIMEOUT).unwrap();
        session(&mut app);
        replay::save(app.world())
    }

    #[test]
    fn verify_spawns_the_world_from_the_content_the_replay_started_with() {
        let config = content("edited", "Totem");
        let dir = std::path::PathBuf::from(&config.asset_dir);
        let recording = record(&config, |app| {
            ok(app, "step", json!({"ticks": 10}));
            write_totem(&dir, "Idol");
            ok(app, "content_reload", json!({}));
            ok(app, "step", json!({"ticks": 60}));
        });
        // The file says Idol now; the recording spawned a Totem.
        assert_eq!(verify(&game, config, recording), ExitCode::SUCCESS);
    }

    #[test]
    fn verify_plays_a_replay_that_ends_at_tick_zero_on_the_spawned_world() {
        let config = content("zero", "Totem");
        let recording = record(&config, |app| {
            ok(app, "teleport", json!({"id": "e1", "x": 3, "z": 0}));
        });
        assert_eq!(recording.end.tick, 0);
        assert_eq!(verify(&game, config, recording), ExitCode::SUCCESS);
    }
}
