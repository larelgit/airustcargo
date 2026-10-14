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
    /// With --verify: keep the window. The replay plays as fast as the machine allows, a
    /// batch of ticks between rendered frames, then the same report and exit code. Shows
    /// that a window plays the same game as headless (render entities, camera, input map
    /// and all); needs a display (`xvfb-run -a` on a server).
    #[arg(long, requires = "verify")]
    windowed: bool,
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
        return if args.windowed { windowed::verify(&build, config, r) } else { verify(&build, config, r) };
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
    // A recording that starts from a snapshot has it put in by now, or could not (other
    // content, another build): then it cannot be played at all.
    if let Some(report) = app.world().resource::<AiReplay>().report().filter(|r| r.interrupted.is_some()) {
        eprintln!("error: {report}");
        return ExitCode::from(2);
    }
    let started = Instant::now();
    let remaining = end.saturating_sub(ai_core::sim::tick(app.world()));
    let failures = match ai_core::sim::step(app.world_mut(), remaining, Duration::from_secs(24 * 3600), |_| None) {
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

/// `--verify --windowed`: the same check as [`verify`], in a window.
mod windowed {
    use super::*;

    /// Ticks played between two rendered frames.
    #[cfg(feature = "render")]
    const BATCH: u64 = 60;

    /// What the frame-by-frame driver needs to know.
    #[cfg(feature = "render")]
    #[derive(Resource)]
    struct Verifying {
        end: u64,
        /// The tick a recording that starts from a snapshot begins at (0 otherwise): the
        /// snapshot goes in before the first tick, so the ticks to play start counting there.
        from: u64,
        /// When loading was done and the playback began.
        started: Option<Instant>,
        frames: u64,
        created: Instant,
    }

    /// Plays `recording` in a window: the game's own app with a window, loaded, then
    /// [`BATCH`] ticks per frame until the recording's end; prints the report like a
    /// headless `--verify` and exits 0 (same), 1 (diverged or a gameplay system failed) or
    /// 2 (could not play it).
    #[cfg(feature = "render")]
    pub fn verify(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, recording: Replay) -> ExitCode {
        // Writes nothing, serves nothing, starts paused: time moves only by the driver's steps.
        let config =
            EngineConfig { headless: false, port: None, paused: true, crash_replays: false, hot_reload: false, ..config };
        let (end, from) = (recording.end.tick, recording.start.as_ref().map_or(0, |s| s.tick));
        let mut app = build(&config);
        // Before loading, as a live `--replay` does.
        if let Err(e) = replay::play(app.world_mut(), recording) {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
        // Frames keep coming without focus or input (a virtual display has neither).
        app.insert_resource(bevy::winit::WinitSettings::continuous())
            .insert_resource(Verifying { end, from, started: None, frames: 0, created: Instant::now() })
            .add_systems(Update, drive);
        exit_code(app.run())
    }

    #[cfg(not(feature = "render"))]
    pub fn verify(_: &impl Fn(&EngineConfig) -> App, _: EngineConfig, _: Replay) -> ExitCode {
        eprintln!("error: --windowed needs a window: build with the `render` feature");
        ExitCode::from(2)
    }

    /// One frame: wait for loading, play a batch of ticks, or report and exit.
    #[cfg(feature = "render")]
    fn drive(world: &mut World) {
        let loading = crate::testing::loading(world);
        let mut state = world.resource_mut::<Verifying>();
        state.frames += 1;
        if let Some(waiting) = loading {
            if state.created.elapsed() > Duration::from_secs(60) {
                eprintln!("error: the game did not finish loading within 60s; still waiting for: {waiting}");
                world.write_message(AppExit::from_code(2));
            }
            return;
        }
        let (end, from) = (state.end, state.from);
        let (started, frames) = (*state.started.get_or_insert_with(Instant::now), state.frames);
        let took = |world: &World| {
            format!("[{} ticks in {:.2}s, {frames} frames]", ai_core::sim::tick(world), started.elapsed().as_secs_f64())
        };
        // The playback finishes in `Last` of the frame that ran its last tick.
        if let Some(report) = world.resource::<AiReplay>().report().cloned() {
            println!("{report} {}", took(world));
            // 2: it could not be played (its snapshot did not load).
            let code = if report.interrupted.is_some() { 2 } else { 1 };
            world.write_message(if report.identical() { AppExit::Success } else { AppExit::from_code(code) });
            return;
        }
        // Until the snapshot is in (the first tick puts it in), the game is at tick 0 but
        // its recording starts at `from`.
        let tick = ai_core::sim::tick(world).max(from);
        if tick >= end {
            return;
        }
        match ai_core::sim::step(world, BATCH.min(end - tick), Duration::from_secs(24 * 3600), |_| None) {
            Ok(StepReport { end: StepEnd::SystemFailed(failures), .. }) => {
                let failure = &failures[0];
                println!("gameplay {}: {} {}", failure.headline(), failure.message, took(world));
                world.write_message(AppExit::from_code(1));
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("error: {e}");
                world.write_message(AppExit::from_code(2));
            }
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
    fn windowed_is_a_way_to_verify() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert!(parse(&["game", "--windowed"]).is_err(), "--windowed alone means nothing");
        assert!(parse(&["game", "--replay", "r.json", "--windowed"]).is_err());
        let args = parse(&["game", "--replay", "r.json", "--verify", "--windowed"]).unwrap();
        assert!(args.verify && args.windowed);
    }

    /// Tests that write into `snapshots/` take turns: the last one out removes the folder.
    static FOLDER: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A file in `snapshots/` of the working directory that goes away with the test.
    struct Scratch(String, #[allow(dead_code, reason = "held until the test is over")] std::sync::MutexGuard<'static, ()>);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let turn = FOLDER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            Scratch(format!("snapshots/shards_verify_{name}_{}.json", std::process::id()), turn)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_dir("snapshots");
        }
    }

    /// A session that loads a snapshot and goes on: its replay starts from the snapshot.
    fn recorded_from_a_snapshot(config: &EngineConfig, file: &Scratch) -> Replay {
        record(config, |app| {
            ok(app, "step", json!({"ticks": 30}));
            ok(app, "snapshot_save", json!({"path": file.0}));
            // Later, elsewhere: the first 30 ticks and this teleport are not part of the replay.
            ok(app, "teleport", json!({"id": "e1", "x": 9, "z": 9}));
            ok(app, "step", json!({"ticks": 20}));
            ok(app, "snapshot_load", json!({"path": file.0}));
            ok(app, "step", json!({"ticks": 10}));
            ok(app, "teleport", json!({"id": "e1", "x": 3, "z": 0}));
            ok(app, "step", json!({"ticks": 70}));
        })
    }

    #[test]
    fn verify_plays_a_replay_that_starts_from_a_snapshot() {
        let config = content("snapshot", "Totem");
        let file = Scratch::new("ok");
        let recording = recorded_from_a_snapshot(&config, &file);
        assert_eq!(recording.start.as_ref().map(|s| s.tick), Some(30));
        assert_eq!((recording.end.tick, recording.events.len()), (110, 1), "{:?}", recording.events);
        // Only the file is needed to play it: the snapshot is inside.
        let _ = std::fs::remove_file(&file.0);
        assert_eq!(verify(&game, config, recording), ExitCode::SUCCESS);
    }

    #[test]
    fn verify_cannot_play_a_replay_whose_snapshot_this_build_cannot_load() {
        let config = content("snapshot_other", "Totem");
        let file = Scratch::new("other");
        let mut recording = recorded_from_a_snapshot(&config, &file);
        recording.start.as_mut().unwrap().checks.insert("content".into(), "00000000000000ff".into());
        // Exit code 2: it could not be played (1 is a replay that diverged).
        assert_eq!(verify(&game, config, recording), ExitCode::from(2));
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
