//! `shards::run`: the standard `main` of a Shards game.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use ai_core::replay::{self, Replay};
#[cfg(feature = "render")]
use ai_core::sim::{StepEnd, StepReport};
use ai_core::{AiReplay, CheckDetail, ReplayReport, Violation};
use bevy::app::AppExit;
use bevy::prelude::*;
use clap::Parser;
use shards_content::{asset_root, check_dir, ContentKinds};
use shards_core::{EngineConfig, NetLaunch, SettingsStore};

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
    /// whether the game still matches the recording and every invariant it broke on the
    /// way, exit 0 (same, nothing broken), 1 (diverged, a gameplay system failed, an
    /// invariant broke) or 2 (could not play it).
    #[arg(long, requires = "replay")]
    verify: bool,
    /// With --verify: keep the window. The replay plays as fast as the machine allows, a
    /// batch of ticks between rendered frames, then the same report and exit code. Shows
    /// that a window plays the same game as headless (render entities, camera, input map
    /// and all); needs a display (`xvfb-run -a` on a server).
    #[arg(long, requires = "verify")]
    windowed: bool,
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
    tick_budget_ms: Option<Duration>,
    /// Read, parse and validate every content file, print the problems, exit 0 (clean)
    /// or 1. Runs nothing.
    #[arg(long)]
    check_content: bool,
    /// Generate REGION (a `region` or `dungeon` content id) for every seed of `--seeds`,
    /// print the batch report, exit 0 (every seed passed) or 1. Runs no game.
    #[arg(long, value_name = "REGION")]
    worldgen_report: Option<String>,
    /// Seeds for --worldgen-report: `1..1000` (both ends included), `7`, `1,5,10..20`.
    /// Default 1..100.
    #[arg(long, requires = "worldgen_report")]
    seeds: Option<String>,
    /// Bake the navigation of tiles into `<assets>/nav/` (TILES: `all`, the default, or tile
    /// and room ids separated by commas), print what was baked, exit 0 (all baked) or 1.
    /// Loads the game headless (content, level files) and runs no tick.
    #[arg(long, value_name = "TILES", num_args = 0..=1, default_missing_value = "all")]
    bake_nav: Option<String>,
    /// Host a networked game on this UDP port (default 7777); other games `--join` it.
    #[arg(long, value_name = "PORT", num_args = 0..=1, default_missing_value = "7777", conflicts_with_all = ["join", "stand"])]
    host: Option<u16>,
    /// Join the networked game hosted at ADDR (`127.0.0.1:7777`).
    #[arg(long, value_name = "ADDR", conflicts_with = "stand")]
    join: Option<String>,
    /// The name other players see.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
    /// A host and N clients from one command, each with its own `shards` port
    /// (`--port-file F` writes F, F.c1, F.c2, ...). Needs --headless.
    #[arg(long, value_name = "N", requires = "headless", value_parser = clap::value_parser!(u8).range(1..))]
    stand: Option<u8>,
    /// With --stand: the clients are child processes joining over UDP loopback, in real
    /// time, instead of apps in this process.
    #[arg(long, requires = "stand")]
    stand_udp: bool,
    /// Network conditions this game's links simulate: `latency=150,loss=0.02` (also
    /// `jitter=`, `duplicate=`).
    #[arg(long, value_name = "SPEC", value_parser = net_sim_spec)]
    net_sim: Option<String>,
    /// Where saves are written: a folder inside the game's folder (no absolute path, no `..`).
    #[arg(long, value_name = "DIR", default_value = "saves", value_parser = save_dir_name)]
    save_dir: String,
    /// Keep player settings in DIR instead of the platform's settings folder.
    #[arg(long, value_name = "DIR", conflicts_with = "no_settings")]
    settings_dir: Option<String>,
    /// Default player settings; read and write no settings files.
    #[arg(long)]
    no_settings: bool,
    /// The character to play: a save slot (`[a-z0-9_-]`, up to 32).
    #[arg(long, value_name = "SLOT", value_parser = slot_name)]
    character: Option<String>,
    /// The world to load: a save slot (`[a-z0-9_-]`, up to 32).
    #[arg(long, value_name = "SLOT", value_parser = slot_name)]
    world: Option<String>,
    /// A client exits when its session ends, instead of going back to the menu: with 0 when the
    /// host ended it, 1 when it was refused, kicked, lost the host or could not take the host's
    /// world. Exit 2 also applies when the session cannot be opened: an address that does not
    /// resolve, or the port of a `--host` game that is taken. A `--host` game does not exit
    /// when it ends the session.
    #[arg(long)]
    exit_with_host: bool,
}

/// `--tick-budget-ms`: milliseconds, as a `Duration` that cannot be zero or too big to hold.
fn positive_ms(text: &str) -> Result<Duration, String> {
    let ms: f64 = text.parse().map_err(|_| format!("`{text}` is not a number of milliseconds"))?;
    if !ms.is_finite() {
        return Err("the budget must be a finite number of milliseconds".into());
    }
    if ms <= 0.0 {
        return Err("the budget must be above 0 ms".into());
    }
    match Duration::try_from_secs_f64(ms / 1000.0) {
        Ok(budget) if budget.is_zero() => Err("the budget is below 1 ns".into()),
        Ok(budget) => Ok(budget),
        Err(_) => Err("the budget is too large".into()),
    }
}

/// `--net-sim`: refused when it does not parse, whatever the role (a typo must not give a
/// clean link to a run that claims 150 ms).
fn net_sim_spec(text: &str) -> Result<String, String> {
    text.parse::<shards_net::LinkConditions>().map(|_| text.to_owned())
}

/// What the ready line says of the session: the role and, for a host, its UDP port. A host
/// on port 0 has none yet (a free one is bound at startup; `shards wait` prints it).
fn net_ready(net: &NetLaunch, name: Option<&str>) -> String {
    // A game given a name says which member of a stand it is (`--name c1`).
    let member = name.map_or_else(String::new, |name| format!(" · member {name}"));
    match net {
        NetLaunch::Host { port: 0 } => format!("{member} · role host"),
        NetLaunch::Host { port } => format!("{member} · role host · net udp:{port}"),
        NetLaunch::Join { addr } => format!("{member} · role client · net udp:{addr}"),
        NetLaunch::Stand { clients, udp: true } => format!("{member} · role host · stand of {clients} over UDP (clients are child processes)"),
        _ => String::new(),
    }
}

/// The save folder: inside the game's folder, like `replays/`.
fn save_dir_name(dir: &str) -> Result<String, String> {
    shards_save::check_save_dir(dir).map(|()| dir.to_owned())
}

/// A save slot: a name, never a path.
fn slot_name(slot: &str) -> Result<String, String> {
    let valid = (1..=32).contains(&slot.len())
        && slot.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if valid { Ok(slot.to_owned()) } else { Err("a slot is 1-32 of a-z, 0-9, `_` and `-`: `slot1`, `my-run`".into()) }
}

impl Args {
    /// What the command line says about networking.
    fn net(&self) -> NetLaunch {
        match (self.host, &self.join, self.stand) {
            (Some(port), _, _) => NetLaunch::Host { port },
            (_, Some(addr), _) => NetLaunch::Join { addr: addr.clone() },
            (_, _, Some(clients)) => NetLaunch::Stand { clients, udp: self.stand_udp },
            _ => NetLaunch::Off,
        }
    }

    /// The config the command line describes (the BRP port is settled later).
    fn config(&self) -> EngineConfig {
        let defaults = EngineConfig::default();
        let settings = match (&self.settings_dir, self.no_settings) {
            (_, true) => SettingsStore::Off,
            (Some(dir), false) => SettingsStore::Dir(dir.clone()),
            (None, false) => SettingsStore::Default,
        };
        EngineConfig {
            headless: self.headless,
            seed: self.seed,
            port: None,
            paused: self.paused,
            tick_hz: self.hz,
            net: self.net(),
            player_name: self.name.clone().unwrap_or(defaults.player_name),
            net_sim: self.net_sim.clone(),
            save_dir: self.save_dir.clone(),
            settings,
            character_slot: self.character.clone(),
            world_slot: self.world.clone(),
            exit_with_host: self.exit_with_host,
            check_every: self.check_every,
            check_detail: self.check_detail,
            tick_budget: self.tick_budget_ms,
            ..EngineConfig::default()
        }
    }
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
    let mut config = args.config();
    if matches!(config.net, NetLaunch::Stand { udp: true, .. }) {
        if args.paused {
            eprintln!(
                "error: --stand-udp runs in real time: UDP games cannot be stepped together, so --paused has no meaning here. \
                 Drop --paused (an agent steps this stand by waiting, e.g. `shards step 120` on the host), or use --stand N without --stand-udp for a stand that stops"
            );
            return ExitCode::from(2);
        }
        // Its own name in the session, and in its ready line.
        config.player_name = args.name.clone().unwrap_or_else(|| "host".into());
    }
    if config.net != NetLaunch::Off && (args.replay.is_some() || args.check_content || args.worldgen_report.is_some() || args.bake_nav.is_some()) {
        eprintln!("error: --host, --join and --stand start a session; drop them to verify, check or bake");
        return ExitCode::from(2);
    }
    if let Some(r) = &recording {
        config.seed = r.seed;
        config.tick_hz = r.tick_hz;
    }
    if args.check_content {
        return check_content(&build, config);
    }
    if let Some(region) = &args.worldgen_report {
        return worldgen_report(&build, config, region, args.seeds.as_deref().unwrap_or("1..100"));
    }
    if let Some(tiles) = &args.bake_nav {
        return bake_nav(&build, config, tiles);
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
    if let NetLaunch::Stand { clients, udp: false } = config.net {
        return stand::run(&build, &args, config, clients);
    }
    let mut app = build(&config);
    termination::catch_console_close();
    // Player settings are read now, not earlier: Bevy's settings plugin scans the type registry
    // once, so every group the game registered while building must be there.
    if let Err(e) = shards_settings::install(&mut app, &config) {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    app.add_systems(First, termination::exit_when_signalled);
    // A stand over UDP: this process is the host, and starts its clients once its socket is open.
    if let NetLaunch::Stand { clients, udp: true } = config.net {
        crate::stand_udp::install(
            &mut app,
            crate::stand_udp::StandSpec {
                clients,
                seed: config.seed,
                hz: config.tick_hz,
                net_sim: config.net_sim.clone(),
                tick_budget: config.tick_budget,
                port_file: args.port_file.clone(),
                no_remote: args.no_remote,
                save_dir: config.save_dir.clone(),
            },
        );
    }
    if let Some(r) = recording
        && let Err(e) = replay::play(app.world_mut(), r)
    {
        eprintln!("error: {e}");
        return ExitCode::from(2);
    }
    let _port_file = match write_port_file(args.port_file.as_deref(), config.port) {
        Ok(file) => file,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let net = net_ready(&config.net, args.name.as_deref().or(matches!(config.net, NetLaunch::Stand { .. }).then_some("host")));
    println!("{}{net}", ai_core::ready_line(0, std::process::id(), config.seed, config.port));
    exit_code(app.run())
}

/// `--stand N`: the host and N clients as apps of this process on an in-memory network
/// (`shards_net::stand::Stand`), each with its own BRP port, port file (`F`, `F.c1`, …) and
/// ready line. Time is the stand's: paused, `step` on any member runs every member, `resume`
/// runs them in real time. Ctrl+C or `kill` ends the session the way a host quitting does.
mod stand {
    use super::*;
    use shards_net::{LinkConditions, MemoryHub, NetState, Stand};

    /// Hub time a stand's shutdown may take for the end of the session to reach its clients.
    const SHUTDOWN_MS: u64 = 5_000;

    use crate::stand_udp::member_file;

    /// The character slot of stand client `n`: named after it, because the members share the save
    /// folder.
    pub(super) fn client_slot(n: u8) -> String {
        format!("c{n}")
    }

    /// Why a stand cannot start with `--character slot`: the host would play the file of a client.
    pub(super) fn slot_clash(character: Option<&str>, clients: u8) -> Option<String> {
        let slot = character?;
        (1..=clients).any(|n| client_slot(n) == slot).then(|| {
            format!("--character {slot} is the slot of stand client {slot}, whose character is kept there; name another slot for the host")
        })
    }

    pub fn run(build: &impl Fn(&EngineConfig) -> App, args: &Args, config: EngineConfig, clients: u8) -> ExitCode {
        if let Some(clash) = slot_clash(config.character_slot.as_deref(), clients) {
            eprintln!("error: {clash}");
            return ExitCode::from(2);
        }
        // `--net-sim` was checked when the command line was parsed.
        let links: LinkConditions = config.net_sim.as_deref().and_then(|spec| spec.parse().ok()).unwrap_or(LinkConditions::NONE);
        let mut ports = vec![config.port];
        for _ in 0..clients {
            ports.push(match config.port {
                Some(_) => match free_port() {
                    Ok(port) => Some(port),
                    Err(e) => {
                        eprintln!("error: {e}");
                        return ExitCode::from(3);
                    }
                },
                None => None,
            });
        }
        let host_name = args.name.clone().unwrap_or_else(|| "host".into());
        let mut stand = match members(build, &config, &host_name, links, &ports) {
            Ok(stand) => stand,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::from(2);
            }
        };
        termination::catch_console_close();
        if let Err(e) = stand.settle() {
            eprintln!("error: {e}");
            return ExitCode::from(1);
        }
        let mut files = Vec::new();
        if let Some(path) = &args.port_file {
            for (i, member) in stand.members.iter().enumerate() {
                let file = if i == 0 { path.clone() } else { member_file(path, &member.name) };
                match write_port_file(Some(&file), ports[i]) {
                    Ok(f) => files.push(f),
                    Err(e) => {
                        eprintln!("error: {e}");
                        return ExitCode::from(2);
                    }
                }
            }
        }
        for (i, member) in stand.members.iter().enumerate() {
            let world = member.app.world();
            let role = match world.get_resource::<NetState>() {
                Some(NetState::Connected { player, body }) => {
                    format!("role client · player {player}{}", body.map_or_else(String::new, |b| format!(" · body e{b}")))
                }
                Some(NetState::Ended(why)) => format!("role client · ended: {why}"),
                _ => "role host · player 0".to_owned(),
            };
            let line = ai_core::ready_line(ai_core::sim::tick(world), std::process::id(), config.seed, ports[i]);
            println!("{line} · member {} · {role}", member.name);
        }
        stand.running = !args.paused;
        loop {
            if stand.serve() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // The host's exit hooks end the session; the clients hear it (over the links'
        // latency and loss: the hub's clock runs on), then leave too.
        let exit = stand.shut_down(SHUTDOWN_MS);
        drop(files);
        exit_code(exit)
    }

    /// The stand's apps, built and attached to one hub, not loaded yet: the host (`host_name`)
    /// and a client per further entry of `ports` (their BRP ports).
    pub(super) fn members(
        build: &impl Fn(&EngineConfig) -> App,
        config: &EngineConfig,
        host_name: &str,
        links: LinkConditions,
        ports: &[Option<u16>],
    ) -> Result<Stand, String> {
        // The stand owns time: every member is paused and ticks when it says.
        let host_config = EngineConfig { paused: true, player_name: host_name.to_owned(), port: ports[0], ..config.clone() };
        let hub = MemoryHub::new(config.seed);
        let mut host = build(&host_config);
        // The host is the player at this machine: its settings are read as a game's of its own
        // are (`--settings-dir`, `--no-settings`). The clients keep none (`SettingsStore::Off`).
        shards_settings::install(&mut host, &host_config)?;
        host.add_systems(First, termination::exit_when_signalled);
        shards_net::stand::attach(&mut host, &hub, "host", None);
        host.finish();
        host.cleanup();
        let mut apps = vec![("host".to_owned(), host)];
        let mut client_links = Vec::new();
        for (n, port) in ports.iter().enumerate().skip(1) {
            let number = u8::try_from(n).unwrap_or(u8::MAX);
            let name = format!("c{n}");
            let client_config = EngineConfig {
                embedded: true,
                headless: true,
                paused: true,
                port: *port,
                net: NetLaunch::Join { addr: "host".into() },
                player_name: name.clone(),
                settings: SettingsStore::Off,
                // Its own character file, named after it: members share the save folder.
                character_slot: Some(client_slot(number)),
                world_slot: None,
                ..config.clone()
            };
            let mut app = build(&client_config);
            let id = shards_net::stand::attach(&mut app, &hub, &name, Some(0));
            client_links.push(hub.connect_both(0, id, links));
            app.finish();
            app.cleanup();
            apps.push((name, app));
        }
        let mut stand = Stand::new(hub, apps);
        for (i, link) in client_links.into_iter().enumerate() {
            stand.members[i + 1].link = Some(link);
        }
        stand.load_timeout = Duration::from_secs(120);
        Ok(stand)
    }

    #[cfg(feature = "remote")]
    fn free_port() -> Result<u16, String> {
        ai_core::remote::free_port()
    }

    #[cfg(not(feature = "remote"))]
    fn free_port() -> Result<u16, String> {
        Ok(0)
    }
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
fn write_port_file(path: Option<&std::path::Path>, port: Option<u16>) -> Result<Option<ai_core::remote::PortFile>, String> {
    match (path, port) {
        (Some(path), Some(port)) => ai_core::remote::PortFile::write(path, port).map(Some),
        (Some(_), None) => Err("--port-file needs the BRP server (drop --no-remote)".into()),
        (None, _) => Ok(None),
    }
}

#[cfg(not(feature = "remote"))]
fn write_port_file(path: Option<&std::path::Path>, _: Option<u16>) -> Result<Option<()>, String> {
    match path {
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
    let stops = match crate::testing::run_to(&mut app, end, Duration::from_secs(60)) {
        Ok(stops) => stops,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    app.update(); // calls made after the last tick, then the final comparison
    let report = app.world().resource::<AiReplay>().report().cloned();
    let took = format!("[{} ticks in {:.2}s]", ai_core::sim::tick(app.world()), started.elapsed().as_secs_f64());
    let failure = stops.failures.first().map(|f| format!("gameplay {}: {}", f.headline(), f.message));
    match print_verdict(report.as_ref(), failure.as_deref(), &stops.violations, &took) {
        Some(true) => ExitCode::SUCCESS,
        Some(false) => ExitCode::FAILURE,
        None => {
            eprintln!("error: the replay did not finish");
            ExitCode::from(2)
        }
    }
}

/// Prints what `--verify` found: every violation (`invariant hp violated at tick 31: …`), the
/// verdict line (`replay finished at tick 600: identical to the recording (10 checks); 1
/// invariant violated [600 ticks in 1.20s]`), then what diverged holds in this run, a line
/// per value. Returns whether it passed; `None` when there is nothing to say (no report, no
/// failure).
fn print_verdict(report: Option<&ReplayReport>, failure: Option<&str>, violations: &[Violation], took: &str) -> Option<bool> {
    for v in violations {
        println!("{}: {}", v.headline(), v.text);
    }
    let broken = match violations.len() {
        0 => String::new(),
        1 => "; 1 invariant violated".to_owned(),
        n => format!("; {n} invariants violated"),
    };
    let passed = match (report, failure) {
        (_, Some(failure)) => {
            let shown = report.map(|r| format!("{}; ", r.headline())).unwrap_or_default();
            println!("{shown}{failure}{broken} {took}");
            false
        }
        (Some(r), None) => {
            println!("{}{broken} {took}", r.headline());
            r.identical() && violations.is_empty()
        }
        (None, None) => return None,
    };
    // What diverged, then its values in this run: `  e6 now: Health { … }`.
    for line in report.iter().flat_map(|r| r.value_lines()) {
        println!("  {line}");
    }
    Some(passed)
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
        /// Invariants broken on the way: listed at the end, the run goes on past them.
        violations: Vec<Violation>,
    }

    /// Plays `recording` in a window: the game's own app with a window, loaded, then
    /// [`BATCH`] ticks per frame until the recording's end; prints the report like a
    /// headless `--verify` and exits 0 (same, nothing broken), 1 (diverged, a gameplay
    /// system failed, an invariant broke) or 2 (could not play it).
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
            .insert_resource(Verifying {
                end,
                from,
                started: None,
                frames: 0,
                created: Instant::now(),
                violations: Vec::new(),
            })
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
            let violations = std::mem::take(&mut world.resource_mut::<Verifying>().violations);
            let passed = print_verdict(Some(&report), None, &violations, &took(world)) == Some(true);
            // 2: it could not be played (its snapshot did not load).
            let code = if report.interrupted.is_some() { 2 } else { 1 };
            world.write_message(if passed { AppExit::Success } else { AppExit::from_code(code) });
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
                let failure = format!("gameplay {}: {}", failure.headline(), failure.message);
                let violations = std::mem::take(&mut world.resource_mut::<Verifying>().violations);
                print_verdict(None, Some(&failure), &violations, &took(world));
                world.write_message(AppExit::from_code(1));
            }
            // A rule that fired stays off unless the recording re-armed it: go on.
            Ok(StepReport { end: StepEnd::InvariantViolated(violated), .. }) => {
                world.resource_mut::<Verifying>().violations.extend(violated);
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

/// `--worldgen-report REGION --seeds 1..1000`: the region generated for every seed from the
/// content on disk, the batch report printed, exit 0 (every seed passed) or 1. For CI.
fn worldgen_report(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, region: &str, seeds: &str) -> ExitCode {
    let config = EngineConfig { headless: true, testing: true, port: None, paused: true, crash_replays: false, hot_reload: false, ..config };
    let app = build(&config);
    let kinds = app.world().resource::<ContentKinds>().clone();
    let root = asset_root(app.world());
    let sources = match shards_content::db::read_dir(&root) {
        Ok(sources) => sources,
        Err(e) => {
            println!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let db = shards_content::ContentDb::build(&kinds, &sources);
    let errors = db.errors().count();
    if errors > 0 {
        println!("warning: content has {errors} errors (--check-content lists them); generating with what loaded");
    }
    let worldgen = app.world().resource::<shards_world::WorldGen>().clone();
    match shards_world::report::run(&db, &worldgen, region, seeds) {
        Ok(report) => {
            print!("{report}");
            if report.all_passed() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(e) => {
            println!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `--bake-nav [TILES]`: the game loaded headless (content, level files), the navigation of
/// its tiles baked into their bake files, the report printed; exit 0 (every tile baked) or 1.
fn bake_nav(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, tiles: &str) -> ExitCode {
    let config = EngineConfig { headless: true, testing: true, port: None, paused: true, crash_replays: false, hot_reload: false, ..config };
    let mut app = build(&config);
    app.finish();
    app.cleanup();
    if let Err(e) = crate::testing::wait_until_loaded(&mut app, crate::testing::LOAD_TIMEOUT) {
        println!("error: {e}");
        return ExitCode::FAILURE;
    }
    match shards_npc::bake_files(app.world_mut(), Some(tiles)) {
        Ok(report) => {
            println!("{report}");
            if report.ok() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Err(e) => {
            println!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// `kill` (SIGTERM) and a closed terminal (SIGHUP; on Windows a closed console) end the game the
/// way Ctrl+C does: through `AppExit`, so what it wrote at start (the BRP token, `--port-file`) is
/// removed. A signal the game was started with ignored (`nohup`) stays ignored; a
/// second signal exits at once.
pub(crate) mod termination {
    use std::sync::atomic::{AtomicI32, Ordering};

    use bevy::app::AppExit;
    use bevy::ecs::world::World;

    /// The first termination signal received; 0: none yet.
    static RECEIVED: AtomicI32 = AtomicI32::new(0);

    /// The signals the game catches and leaves on, with code 128 + the signal. (Ctrl+C, SIGINT,
    /// is Bevy's own handler, with the same kind of exit.) The stand tells a client that left
    /// this way from one that crashed.
    #[cfg(unix)]
    pub(crate) const CAUGHT: [libc::c_int; 2] = [libc::SIGTERM, libc::SIGHUP];

    #[cfg(unix)]
    pub fn catch_signals() {
        let handler: extern "C" fn(libc::c_int) = on_signal;
        for signal in CAUGHT {
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

    /// Windows: the console's closing (its window closed, a log-off, a shutdown) is SIGHUP's
    /// counterpart. Ctrl+C and Ctrl+Break are left to Bevy's handler.
    #[cfg(windows)]
    pub fn catch_signals() {
        catch_console_close();
    }

    #[cfg(not(any(unix, windows)))]
    pub fn catch_signals() {}

    /// Windows asks the console handler registered last first, and the one of Bevy's Ctrl+C
    /// plugin answers every event, the console's closing too, after which Windows ends the
    /// process at once. Called again once the game is built, this puts the engine's in front.
    #[cfg(windows)]
    pub fn catch_console_close() {
        // SAFETY: registers (and first takes out) a handler function; no other pointers.
        unsafe {
            windows::SetConsoleCtrlHandler(Some(on_console_event), 0);
            windows::SetConsoleCtrlHandler(Some(on_console_event), 1);
        }
    }

    #[cfg(not(windows))]
    pub fn catch_console_close() {}

    #[cfg(windows)]
    mod windows {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            pub fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
        }
    }

    /// SIGHUP's number: a closed console ends the game with code 129, as a closed terminal does.
    #[cfg(windows)]
    const CONSOLE_CLOSED: i32 = 1;

    /// Runs on a thread of its own. Windows ends the process when this returns, and five seconds
    /// after the event at the latest, so it waits: the game sees the flag on its next frame, runs
    /// its exit hooks and returns from `main`, which ends the process (this thread with it).
    #[cfg(windows)]
    unsafe extern "system" fn on_console_event(event: u32) -> i32 {
        const CTRL_CLOSE_EVENT: u32 = 2;
        const CTRL_LOGOFF_EVENT: u32 = 5;
        const CTRL_SHUTDOWN_EVENT: u32 = 6;
        if !matches!(event, CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT) {
            return 0;
        }
        let _ = RECEIVED.compare_exchange(0, CONSOLE_CLOSED, Ordering::SeqCst, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(4500));
        1
    }

    #[cfg(unix)]
    extern "C" fn on_signal(signal: libc::c_int) {
        if RECEIVED.compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            // SAFETY: `_exit` is async-signal-safe (unlike `std::process::exit`).
            unsafe { libc::_exit(128 + signal) };
        }
    }

    /// Exit code 128 + signal, as a shell reports a process the signal killed. The exit
    /// hooks run right away (once: the `Last` system that sees this `AppExit` skips them).
    pub fn exit_when_signalled(world: &mut World) {
        let signal = RECEIVED.load(Ordering::SeqCst);
        if signal != 0 {
            world.write_message(AppExit::from_code((128 + signal) as u8));
            shards_core::run_exit_hooks(world);
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
    fn the_tick_budget_is_a_duration_the_game_can_hold() {
        let parse = |ms: &str| Args::try_parse_from(["game".to_owned(), format!("--tick-budget-ms={ms}")]);
        let budget = |ms: &str| parse(ms).unwrap().config().tick_budget;
        assert_eq!(budget("5"), Some(Duration::from_millis(5)));
        assert_eq!(budget("16.7"), Some(Duration::from_micros(16_700)));
        assert_eq!(Args::try_parse_from(["game"]).unwrap().config().tick_budget, None);
        // Too big for a Duration: a refusal at the command line, not a panic in `config`.
        assert!(parse("1e30").unwrap_err().to_string().contains("the budget is too large"));
        assert!(parse("999999999999999999999999").is_err());
        // So small that it rounds to nothing: every tick would be over budget.
        assert!(parse("1e-10").unwrap_err().to_string().contains("the budget is below 1 ns"));
        for bad in ["inf", "nan", "-inf"] {
            assert!(parse(bad).unwrap_err().to_string().contains("finite"), "{bad}");
        }
        for bad in ["0", "-3"] {
            assert!(parse(bad).unwrap_err().to_string().contains("above 0 ms"), "{bad}");
        }
        assert!(parse("soon").unwrap_err().to_string().contains("is not a number of milliseconds"));
    }

    #[test]
    fn windowed_is_a_way_to_verify() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert!(parse(&["game", "--windowed"]).is_err(), "--windowed alone means nothing");
        assert!(parse(&["game", "--replay", "r.json", "--windowed"]).is_err());
        let args = parse(&["game", "--replay", "r.json", "--verify", "--windowed"]).unwrap();
        assert!(args.verify && args.windowed);
    }

    #[test]
    fn a_stand_hosts_character_is_never_a_clients_slot() {
        assert_eq!(stand::client_slot(2), "c2");
        assert!(stand::slot_clash(None, 2).is_none());
        assert!(stand::slot_clash(Some("hero"), 2).is_none());
        assert!(stand::slot_clash(Some("c3"), 2).is_none(), "there is no client 3");
        let clash = stand::slot_clash(Some("c2"), 2).unwrap();
        assert!(clash.contains("--character c2") && clash.contains("name another slot for the host"), "{clash}");
    }

    #[test]
    fn network_save_and_settings_flags_are_checked_and_stored() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        let refused = |args: &[&str]| parse(args).unwrap_err().to_string();
        assert!(refused(&["game", "--host", "--join", "1.2.3.4:7777"]).contains("cannot be used with"));
        assert!(refused(&["game", "--join", "1.2.3.4:7777", "--stand", "2", "--headless"]).contains("cannot be used with"));
        assert!(refused(&["game", "--host", "--stand", "2", "--headless"]).contains("cannot be used with"));
        assert!(refused(&["game", "--stand", "2"]).contains("--headless"), "a stand runs headless");
        assert!(refused(&["game", "--stand", "0", "--headless"]).contains("0"), "a stand has clients");
        assert!(refused(&["game", "--stand-udp"]).contains("--stand"));
        for role in [&["--host"][..], &["--join", "1.2.3.4:7777"], &["--stand", "2", "--headless"]] {
            let args: Vec<&str> = ["game"].into_iter().chain(role.iter().copied()).chain(["--net-sim", "latency=15O"]).collect();
            assert!(refused(&args).contains("--net-sim"), "a typo in --net-sim is refused for {role:?}: {}", refused(&args));
        }
        assert!(refused(&["game", "--no-settings", "--settings-dir", "s"]).contains("cannot be used with"));
        assert!(refused(&["game", "--world", "../etc"]).contains("a slot is 1-32 of a-z"));
        assert!(refused(&["game", "--character", "Hero"]).contains("a slot is 1-32 of a-z"));
        assert!(refused(&["game", "--save-dir", "/var/saves"]).contains("is absolute"));
        assert!(refused(&["game", "--save-dir", "../saves"]).contains("goes up with `..`"));
        assert!(refused(&["game", "--save-dir", ""]).contains("is empty"));
        assert_eq!(parse(&["game", "--save-dir", "saves/qa-1"]).unwrap().config().save_dir, "saves/qa-1");

        let plain = parse(&["game"]).unwrap().config();
        assert_eq!(plain.net, NetLaunch::Off);
        assert_eq!((plain.save_dir.as_str(), &plain.settings, plain.player_name.as_str()), ("saves", &SettingsStore::Default, "player"));
        let host = parse(&["game", "--host"]).unwrap().config();
        assert_eq!(host.net, NetLaunch::Host { port: shards_core::DEFAULT_NET_PORT });
        assert_eq!(parse(&["game", "--host", "9000"]).unwrap().config().net, NetLaunch::Host { port: 9000 });
        let stand = parse(&["game", "--stand", "2", "--stand-udp", "--headless", "--net-sim", "latency=150"]).unwrap().config();
        assert_eq!((stand.net, stand.net_sim.as_deref()), (NetLaunch::Stand { clients: 2, udp: true }, Some("latency=150")));
        let join = parse(&[
            "game", "--join", "127.0.0.1:7777", "--name", "Ann", "--character", "ann-1", "--world", "w_2", "--exit-with-host",
            "--save-dir", "my_saves", "--no-settings",
        ])
        .unwrap()
        .config();
        assert_eq!(join.net, NetLaunch::Join { addr: "127.0.0.1:7777".into() });
        assert_eq!((join.player_name.as_str(), join.character_slot.as_deref(), join.world_slot.as_deref()), ("Ann", Some("ann-1"), Some("w_2")));
        assert!(join.exit_with_host);
        assert_eq!((join.save_dir.as_str(), join.settings), ("my_saves", SettingsStore::Off));
        let dir = parse(&["game", "--settings-dir", "/tmp/s"]).unwrap().config();
        assert_eq!(dir.settings, SettingsStore::Dir("/tmp/s".into()));
    }

    #[test]
    fn the_help_of_exit_with_host_says_what_the_codes_mean() {
        use clap::CommandFactory;
        let help = Args::command().render_long_help().to_string();
        let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(help.contains("--exit-with-host A client exits when its session ends"), "a client's flag: {help}");
        assert!(help.contains("with 0 when the host ended it, 1 when it was refused"), "{help}");
        assert!(help.contains("Exit 2 also applies when the session cannot be opened"), "{help}");
        assert!(help.contains("A `--host` game does not exit when it ends the session"), "{help}");
    }

    /// A folder that goes away with the test, whether it ends or fails.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_stands_host_reads_the_players_settings_and_its_clients_keep_none() {
        let scratch = TempDir(std::env::temp_dir().join(format!("shards_stand_settings_{}", std::process::id())));
        let dir = &scratch.0;
        let _ = std::fs::remove_dir_all(dir);
        let config = EngineConfig {
            net: NetLaunch::Stand { clients: 1, udp: false },
            settings: SettingsStore::Dir(dir.to_string_lossy().into_owned()),
            ..testing::config()
        };
        let stand = stand::members(&game, &config, "host", shards_net::LinkConditions::NONE, &[None, None]).unwrap();
        let file = |member: usize| stand.world(member).get_resource::<shards_settings::SettingsStorage>().map(|s| s.file.clone());
        assert_eq!(file(0), Some(Some(dir.join("settings.toml"))), "the host's settings live in --settings-dir");
        assert_eq!(file(1), None, "a client of the stand keeps none");
    }

    #[test]
    fn the_ready_line_names_a_hosts_port_only_when_it_is_known() {
        assert_eq!(net_ready(&NetLaunch::Host { port: 9000 }, None), " · role host · net udp:9000");
        assert_eq!(net_ready(&NetLaunch::Host { port: 0 }, None), " · role host", "port 0 is picked at startup: `shards wait` prints it");
        assert_eq!(net_ready(&NetLaunch::Join { addr: "1.2.3.4:7777".into() }, None), " · role client · net udp:1.2.3.4:7777");
        assert_eq!(net_ready(&NetLaunch::Off, None), "");
        // A named game says which member of a stand it is.
        assert_eq!(
            net_ready(&NetLaunch::Join { addr: "127.0.0.1:7777".into() }, Some("c1")),
            " · member c1 · role client · net udp:127.0.0.1:7777"
        );
        assert_eq!(
            net_ready(&NetLaunch::Stand { clients: 2, udp: true }, Some("host")),
            " · member host · role host · stand of 2 over UDP (clients are child processes)"
        );
        assert_eq!(net_ready(&NetLaunch::Stand { clients: 2, udp: false }, Some("host")), "", "an in-process stand prints a line per member itself");
    }

    #[test]
    fn an_embedded_game_is_driven_from_outside_but_keeps_its_port() {
        let config = testing::embedded_config(Some(1));
        assert!(config.embedded && !config.testing && config.headless && config.paused);
        assert_eq!(config.ai_core_plugin().remote_port, Some(1), "its own BRP port");
        assert_eq!(config.settings, SettingsStore::Off);
        assert_eq!(testing::config().ai_core_plugin().remote_port, None);

        let mut app = game(&testing::embedded_config(None));
        app.finish();
        app.cleanup();
        testing::wait_until_loaded(&mut app, testing::LOAD_TIMEOUT).unwrap();
        assert!(app.world().contains_resource::<ai_core::journal::LogCapture>(), "its log lines go to its journal");
        ok(&mut app, "step", json!({"ticks": 3}));
        assert_eq!(ai_core::sim::tick(app.world()), 3);
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

    #[test]
    fn worldgen_report_checks_a_region_over_seeds_without_a_game() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert!(parse(&["game", "--seeds", "1..10"]).is_err(), "--seeds belongs to --worldgen-report");
        let args = parse(&["game", "--worldgen-report", "tiny", "--seeds", "1..10"]).unwrap();
        assert_eq!((args.worldgen_report.as_deref(), args.seeds.as_deref()), (Some("tiny"), Some("1..10")));

        let config = content("worldgen", "Totem");
        let dir = std::path::PathBuf::from(&config.asset_dir).join("content");
        let files = [
            ("w.socket_rules.ron", r#"{ "default": (kinds: ["grass"], passable: ["grass"]) }"#),
            ("w.tile.ron", r#"{ "field": (sockets: (n: "grass", e: "grass", s: "grass", w: "grass"), slots: [(at: (0.0, 0.0, 0.0), accepts: ["spawn"])]) }"#),
            ("w.poi.ron", r#"{ "spawn": () }"#),
            ("w.region.ron", r#"{ "tiny": (size: (2, 2), tile_size: 8.0, version: 1, pois: [(poi: "spawn")]) }"#),
        ];
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        assert_eq!(worldgen_report(&game, config.clone(), "tiny", "1..50"), ExitCode::SUCCESS);
        assert_eq!(worldgen_report(&game, config, "tinny", "1..5"), ExitCode::FAILURE, "an unknown region fails");
    }

    #[test]
    fn bake_nav_writes_the_bake_files_content_checks() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert_eq!(parse(&["game", "--bake-nav"]).unwrap().bake_nav.as_deref(), Some("all"));
        assert_eq!(parse(&["game", "--bake-nav", "field,rock"]).unwrap().bake_nav.as_deref(), Some("field,rock"));

        let config = content("bake_nav", "Totem");
        let dir = std::path::PathBuf::from(&config.asset_dir);
        let files = [
            ("w.socket_rules.ron", r#"{ "default": (kinds: ["grass"], passable: ["grass"]) }"#),
            ("w.tile.ron", r#"{ "field": (sockets: (n: "grass", e: "grass", s: "grass", w: "grass"), slots: [(at: (0.0, 0.0, 0.0), accepts: ["spawn"])], geometry: Inline([Box(min: (-4.0, -1.0, -4.0), max: (4.0, 0.0, 4.0))])) }"#),
            ("w.poi.ron", r#"{ "spawn": () }"#),
            ("w.region.ron", r#"{ "tiny": (size: (2, 2), tile_size: 8.0, version: 1, pois: [(poi: "spawn")]) }"#),
            ("w.nav.ron", r#"{ "default": (cell: 0.5) }"#),
        ];
        for (name, text) in files {
            std::fs::write(dir.join("content").join(name), text).unwrap();
        }
        assert_eq!(check_content(&game, config.clone()), ExitCode::FAILURE, "a nav definition wants bake files");
        assert_eq!(bake_nav(&game, config.clone(), "all"), ExitCode::SUCCESS);
        let baked = std::fs::read_to_string(dir.join("nav/field.navtile.ron")).unwrap();
        assert!(baked.contains("format: \"shards-navtile/1\""), "{baked}");
        assert_eq!(check_content(&game, config.clone()), ExitCode::SUCCESS, "fresh bakes are clean content");
        assert_eq!(bake_nav(&game, config, "feild"), ExitCode::FAILURE, "an unknown tile fails");
    }
}
