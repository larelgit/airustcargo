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
use shards_core::{EngineConfig, MenuLaunch, NetLaunch, SettingsStore, SteamLaunch, SteamLobby};

/// The engine's rules that are off by default, as `--engine-invariant` names them
/// (`EngineConfig::opt_in_invariants`): none today (`voxel.body_in_solid`, the first, is on by
/// default since its fuzz runs).
const OPT_IN_INVARIANTS: [&str; 0] = [];

/// A game on the Shards engine.
#[derive(Parser, Debug)]
#[command(version, group(clap::ArgGroup::new("lobby_host").args(["steam_host", "stand_steam"])))]
struct Args {
    /// No window and no rendering (CI, parallel playtests, servers).
    #[arg(long)]
    headless: bool,
    /// No window, but the GPU draws: the cameras render into an image of the window's size that
    /// `shards shot` captures. For CI and agents on a machine with a GPU (or a software one).
    #[arg(long, conflicts_with_all = ["headless", "stand", "verify", "check_content", "worldgen_report", "bake_nav", "bake_motion", "smoke"])]
    offscreen: bool,
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
    /// Heavy mode: every N ticks a fresh copy of the game (a twin, built by this game's own
    /// builder) loads a snapshot of it, plays what it did for --heavy-span ticks, and must come
    /// to the same state and the same journal. A difference stops the game like a broken
    /// invariant (`step` exits 3, `--verify` exits 1) and saves snapshots/heavy-tick<T>-*.json
    /// and replays/heavy-tick<T>.json. The checks run between stepped ticks: with --paused
    /// (`shards step`, scenarios, fuzz), --replay FILE --verify, or --stand.
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u64).range(1..),
        conflicts_with_all = [
            "windowed", "check_content", "worldgen_report", "bake_nav", "bake_motion", "smoke", "perf",
            "host", "join", "stand_udp", "steam_host", "steam_server",
        ]
    )]
    heavy: Option<u64>,
    /// With --heavy: the ticks each heavy point plays before its twin is compared (default N, at
    /// most 600).
    #[arg(long, value_name = "K", requires = "heavy", value_parser = clap::value_parser!(u64).range(1..=ai_core::heavy::MAX_SPAN))]
    heavy_span: Option<u64>,
    /// With --heavy: the tick of the first heavy point (default 0). A nightly run passes seed % N,
    /// so that over many nights every tick phase is checked.
    #[arg(long, value_name = "O", requires = "heavy")]
    heavy_offset: Option<u64>,
    /// Keep state coverage: the `coverage` tool lists the states the session reached (an entity's kind,
    /// motion mode, action and phase, statuses, stamina level, behaviour state) and what content declares that it
    /// never did. Only reads the world; for fuzzing and soak.
    #[arg(long, conflicts_with_all = ["check_content", "worldgen_report", "bake_nav", "bake_motion", "smoke", "perf"])]
    coverage: bool,
    /// A tick slower than this is over budget: `step` counts it and the journal names
    /// it (`perf`). Default: the tick's own duration (16.7 ms at 60 Hz). Debug builds
    /// run slower than `--release`.
    #[arg(long, value_name = "MS", value_parser = positive_ms)]
    tick_budget_ms: Option<Duration>,
    /// A frame slower than this is over budget (`frames` in `ai/status`: a window or --offscreen).
    /// Default 16.7 ms: 60 frames a second.
    #[arg(long, value_name = "MS", value_parser = positive_ms)]
    frame_budget_ms: Option<Duration>,
    /// Read, parse and validate every content file, print the problems, exit 0 (clean)
    /// or 1. Runs nothing.
    #[arg(long)]
    check_content: bool,
    /// Generate REGION (a region content id of any kind: `region`, `dungeon`, `voxel_region`) for every seed of `--seeds`,
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
    /// Bake the displacement curves of actions from the root bone of their clips (an action's
    /// `bake`) into their content files (ACTIONS: `all`, the default, or action ids separated by
    /// commas), print what was baked, exit 0 (all baked) or 1. Loads the game headless (content)
    /// and runs no tick.
    #[arg(long, value_name = "ACTIONS", num_args = 0..=1, default_missing_value = "all", conflicts_with = "bake_nav")]
    bake_motion: Option<String>,
    /// Load headless and run TICKS ticks as fast as they go, print how long it took, exit 0, or 1
    /// when the game did not load, a gameplay system panicked or an invariant broke. Writes
    /// nothing. Checks a build that has no `shards` server (a build for players).
    #[arg(
        long,
        value_name = "TICKS",
        value_parser = clap::value_parser!(u64).range(1..),
        conflicts_with_all = ["replay", "check_content", "worldgen_report", "bake_nav", "bake_motion"]
    )]
    smoke: Option<u64>,
    /// Run the performance autotest ID (a `perf` content definition: the scene, how long, the
    /// budgets): load, go to the scene, warm up, measure the ticks as fast as they go (with
    /// --offscreen: play in real time, and measure the frames and the GPU too), print a line per
    /// budget, exit 0 (all met), 1 (one exceeded, or the game failed on the way) or 2 (it could not
    /// run). A debug build measures without judging: budgets are for --release. Writes nothing.
    #[arg(long, value_name = "ID", conflicts_with_all = ["replay", "check_content", "worldgen_report", "bake_nav", "bake_motion", "smoke"])]
    perf: Option<String>,
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
    /// With --stand: the members meet over a fake Steam of this process instead of the plain in-memory
    /// transport (a game built with the `steam` feature; the real Steam is never reached): the host
    /// makes a Steam lobby (friends only, or --steam-lobby TYPE), and the clients wait for one, so
    /// `steam_lobbies` finds the lobby and `steam_join` enters it.
    #[arg(long, requires = "stand", conflicts_with = "stand_udp")]
    stand_steam: bool,
    /// Network conditions this game's links simulate: `latency=150,loss=0.02` (also
    /// `jitter=`, `duplicate=`).
    #[arg(long, value_name = "SPEC", value_parser = net_sim_spec)]
    net_sim: Option<String>,
    /// Where saves are written: a folder inside the game's folder (no absolute path, no `..`).
    #[arg(long, value_name = "DIR", default_value = "saves", value_parser = save_dir_name)]
    save_dir: String,
    /// Engine modules to leave out, separated by commas: `audio,scatter`. `ai/status` lists the
    /// modules that run (`engine.modules`); a module others need goes only with them.
    #[arg(long, value_name = "MODULES", value_delimiter = ',')]
    without: Vec<String>,
    /// Do not check the engine's own invariant rules (`shards assert list` shows them as `engine
    /// rule`). They are on by default and off in a `dist` build; `--verify` always checks them.
    #[arg(long)]
    no_engine_invariants: bool,
    /// Also check an engine rule that is off by default (the flag once a rule). None is off by
    /// default today: the flag takes no name yet.
    #[arg(
        long = "engine-invariant",
        value_name = "NAME",
        conflicts_with = "no_engine_invariants",
        value_parser = clap::builder::PossibleValuesParser::new(OPT_IN_INVARIANTS)
    )]
    engine_invariant: Vec<String>,
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
    /// Show the game's main menu after loading even without a window (an agent goes through it with
    /// `ui_*` tools, `new_game`, `continue_game`). A game with a main menu shows it by default only
    /// with a window or --offscreen, and never in replays, checks or sessions. A game without one
    /// goes straight in either way.
    #[arg(
        long,
        conflicts_with_all = [
            "no_menu", "replay", "check_content", "worldgen_report", "bake_nav", "bake_motion", "smoke", "perf",
            "host", "join", "stand", "steam_host", "steam_server", "world", "character",
        ]
    )]
    menu: bool,
    /// Go straight into the game past its main menu, with a window or --offscreen too.
    #[arg(long)]
    no_menu: bool,
    /// Host the session in a Steam lobby (a game built with the `steam` feature): friends join
    /// through an invite, or with `--join steam:<lobby>`, over the Steam relay; no UDP port is opened.
    #[arg(long, conflicts_with_all = ["host", "join", "stand", "steam_server"])]
    steam_host: bool,
    /// With --steam-host (or --stand-steam): who sees the lobby. `friends` (the default: friends see
    /// it and join), `private` (only an invite brings a player in), `invisible` (friends do not see
    /// it, a lobby search finds it), `public` (friends see it, and a lobby search finds it). A search
    /// never finds a `friends` or a `private` lobby.
    #[arg(long, value_name = "TYPE", requires = "lobby_host", conflicts_with = "steam_server", value_parser = steam_lobby)]
    steam_lobby: Option<SteamLobby>,
    /// Host as an anonymous Steam game server: no lobby and no Steam user. Players join it by its
    /// Steam id (`--join steam:<id>`; `shards status` and the tool `steam` show it), over the relay.
    #[arg(long, conflicts_with_all = ["host", "join", "stand"])]
    steam_server: bool,
    /// Only Steam's relays carry the session: no direct route between the machines (ICE off).
    #[arg(long)]
    steam_relay_only: bool,
    /// The game's Steam app id (480: Valve's test app), when Steam did not start the game: instead
    /// of a `steam_appid.txt`, which a game never ships.
    #[arg(long, value_name = "ID", value_parser = clap::value_parser!(u32).range(1..))]
    steam_app: Option<u32>,
}

/// `--tick-budget-ms`, `--frame-budget-ms`: milliseconds, as a `Duration` that cannot be zero or too big to hold.
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

/// `--steam-lobby`: one of Steam's lobby types.
fn steam_lobby(text: &str) -> Result<SteamLobby, String> {
    text.parse()
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

/// [`net_ready`], for a session over Steam too: its host has no UDP port, and its client's address
/// is a Steam one (`net steam:109775240917319681`).
fn session_ready(config: &EngineConfig, name: Option<&str>) -> String {
    let member = name.map_or_else(String::new, |name| format!(" · member {name}"));
    match &config.net {
        NetLaunch::Host { .. } if config.steam.server => format!("{member} · role host · net steam game server"),
        NetLaunch::Host { .. } if config.steam.host => format!("{member} · role host · net steam lobby"),
        NetLaunch::Join { addr } if addr.starts_with("steam:") => format!("{member} · role client · net {addr}"),
        net => net_ready(net, name),
    }
}

/// The command line as the game's options read it: with the `steam` feature, Steam's
/// `+connect_lobby <lobby>` (a game Steam starts into a friend's lobby) becomes `--join
/// steam:<lobby>`. Without it, a Steam session is refused with what to build.
fn command_line() -> Result<Args, String> {
    #[cfg(feature = "steam")]
    let args = Args::parse_from(shards_steam::connect_lobby_args(std::env::args_os())?);
    #[cfg(not(feature = "steam"))]
    let args = Args::parse();
    check_without(&args)?;
    if args.wants_steam() && !cfg!(feature = "steam") {
        return Err("this game is built without Steam: --steam-host, --steam-lobby, --stand-steam, --steam-server, --steam-relay-only, --steam-app and --join steam:… need \
                    the `steam` feature of `shards` (and `steam_sdk` to reach the Steam client; --stand-steam never does)"
            .into());
    }
    #[cfg(feature = "steam")]
    if let Some(Err(e)) = args.join.as_deref().and_then(shards_steam::parse_address) {
        return Err(e);
    }
    Ok(args)
}

/// The save folder: inside the game's folder, like `replays/`.
/// `--without`: modules a game may leave out, and nothing else on the command line asks for one.
fn check_without(args: &Args) -> Result<(), String> {
    let without = crate::modules::check_without(&args.without).map_err(|e| format!("--without: {e}"))?;
    let session = args.host.is_some() || args.join.is_some() || args.stand.is_some() || args.steam_host || args.steam_server || args.stand_steam;
    if session && without.contains(&"net") {
        return Err("--without net leaves out co-op sessions: drop --host, --join, --stand, --steam-host and --steam-server, or keep `net`".into());
    }
    if args.wants_steam() && without.contains(&"steam") {
        return Err("--without steam leaves out Steam: drop --steam-host, --steam-lobby, --stand-steam, --steam-server,                     --steam-relay-only, --steam-app and --join steam:…, or keep `steam`"
            .into());
    }
    if (args.character.is_some() || args.world.is_some()) && without.contains(&"save") {
        return Err("--without save leaves out saves: drop --character and --world, or keep `save`".into());
    }
    if args.worldgen_report.is_some() && without.contains(&"world") {
        return Err("--without world leaves out the region generator: drop --worldgen-report, or keep `world`".into());
    }
    Ok(())
}

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
    /// Whether the command line asks for Steam: a lobby, a server, a stand over the fake client, an app id.
    fn wants_steam(&self) -> bool {
        self.steam_host
            || self.steam_server
            || self.steam_lobby.is_some()
            || self.stand_steam
            || self.steam_relay_only
            || self.steam_app.is_some()
            || self.join.as_deref().is_some_and(|a| a.starts_with("steam:"))
    }

    /// What the command line says about networking.
    fn net(&self) -> NetLaunch {
        if self.steam_host || self.steam_server {
            // The Steam relay carries the session: no UDP port.
            return NetLaunch::Host { port: 0 };
        }
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
            offscreen: self.offscreen,
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
            menu: match (self.menu, self.no_menu) {
                (true, _) => MenuLaunch::Show,
                (_, true) => MenuLaunch::Skip,
                _ => MenuLaunch::Auto,
            },
            without: self.without.clone(),
            coverage: self.coverage,
            heavy: self.heavy.map(|every| {
                let defaults = ai_core::HeavyConfig::every(every);
                ai_core::HeavyConfig {
                    span: self.heavy_span.unwrap_or(defaults.span),
                    offset: self.heavy_offset.unwrap_or(defaults.offset),
                    ..defaults
                }
            }),
            engine_invariants: defaults.engine_invariants && !self.no_engine_invariants,
            opt_in_invariants: self.engine_invariant.clone(),
            check_every: self.check_every,
            check_detail: self.check_detail,
            tick_budget: self.tick_budget_ms,
            frame_budget: self.frame_budget_ms,
            // `connect` is set for a game that is played, not checked (`run`).
            steam: SteamLaunch {
                connect: false,
                app_id: self.steam_app,
                // A stand over a fake Steam has a lobby host too; its net launch stays the stand's.
                host: self.steam_host || self.stand_steam,
                server: self.steam_server,
                relay_only: self.steam_relay_only,
                lobby: self.steam_lobby.unwrap_or_default(),
            },
            ..EngineConfig::default()
        }
    }
}

/// A game built as a Windows program without a console (`windows_subsystem = "windows"`: what
/// players start from Explorer) writes to the terminal it was started from, if there is one:
/// `--check-content`, `--help` and errors stay readable there. Output that was redirected (to a
/// file, a pipe) goes there as before.
#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleWindow() -> *mut core::ffi::c_void;
        fn AttachConsole(process: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: two calls without arguments that point anywhere.
    unsafe {
        if GetConsoleWindow().is_null() {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn attach_console() {}

/// The standard game `main`: parses the command line into an [`EngineConfig`], calls
/// `build` with it (the game adds `EnginePlugins::new(config.clone())` and its own
/// plugins), and runs, verifies a replay or checks content. Refuses a busy BRP port
/// (exit code 3) before anything is built; `kill` and a closed terminal exit through
/// `AppExit` like Ctrl+C, so the token and the port file go away with the game. `build` is
/// called again for each twin of heavy mode (`--heavy`), so it is shared: `Send + Sync`.
pub fn run(build: impl Fn(&EngineConfig) -> App + Send + Sync + 'static) -> ExitCode {
    let shared: crate::heavy::Builder = std::sync::Arc::new(build);
    let build = |config: &EngineConfig| shared(config);
    attach_console();
    let args = match command_line() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut recording = match args.replay.as_deref().map(replay::load).transpose() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let mut config = args.config();
    if config.offscreen && !cfg!(feature = "render") {
        eprintln!("error: --offscreen draws on the GPU, and this game is built without the `render` feature: run it with --headless");
        return ExitCode::from(2);
    }
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
    let checks = args.replay.is_some()
        || args.check_content
        || args.worldgen_report.is_some()
        || args.bake_nav.is_some()
        || args.bake_motion.is_some()
        || args.smoke.is_some()
        || args.perf.is_some();
    if checks {
        // A replay is played, and a check made, on the game as it starts without a person: no menu.
        config.menu = MenuLaunch::Skip;
    }
    if config.net != NetLaunch::Off && checks {
        eprintln!(
            "error: --host, --join, --stand, --steam-host and --steam-server start a session; drop them to verify, check, bake, --smoke or --perf"
        );
        return ExitCode::from(2);
    }
    if let Some(r) = &recording {
        config.seed = r.seed;
        config.tick_hz = r.tick_hz;
    }
    if let Some(heavy) = &config.heavy {
        if let Err(e) = heavy.check() {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
        // A stand's driver checks its host in real time too; a game alone only between steps.
        if !(args.paused || args.verify || args.stand.is_some()) {
            eprintln!(
                "error: --heavy checks between stepped ticks: add --paused (and `shards step` it), \
                 or use it with --replay FILE --verify or --stand"
            );
            return ExitCode::from(2);
        }
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
    if let Some(actions) = &args.bake_motion {
        return bake_motion(&build, config, actions);
    }
    if let Some(ticks) = args.smoke {
        return smoke(&build, config, ticks);
    }
    if let Some(id) = &args.perf {
        return perf(&build, config, id);
    }
    if args.verify
        && let Some(r) = recording.take()
    {
        return if args.windowed { windowed::verify(&build, config, r) } else { verify(&shared, config, r) };
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
    // From here on a panic leaves a crash report: the hook keeps what it can, and the game
    // writes the report, or `guarded` does when the panic takes the game with it.
    ai_core::crash::install_panic_hook();
    let reports = config.crash_replays;
    if let NetLaunch::Stand { clients, udp: false } = config.net {
        return guarded(reports, None, || stand::run(&shared, &args, config, clients));
    }
    // A game that is played talks to Steam (with the `steam_sdk` feature): checks, tests and the
    // in-process stand never do.
    config.steam.connect = true;
    guarded(reports, None, || play(&shared, &args, config, recording))
}

/// Runs the game. A panic that unwinds out of it took the world with it: what the panic hook kept
/// goes into a crash report (`reports` off: none), its path to stderr, and the process exits with
/// 101, as after any panic. `base`: the report goes there instead of the working directory.
fn guarded(reports: bool, base: Option<&std::path::Path>, game: impl FnOnce() -> ExitCode) -> ExitCode {
    let payload = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(game)) {
        Ok(code) => return code,
        Err(payload) => payload,
    };
    if !reports {
        std::panic::resume_unwind(payload);
    }
    let report = ai_core::crash::fatal(payload.as_ref());
    let written = match base {
        Some(base) => ai_core::crash::write_in(base, &report),
        None => ai_core::crash::write(&report),
    };
    match written {
        Ok(path) => eprintln!("error: the game crashed: {}; crash report: {}", report.what.message, ai_core::crash::shown(&path)),
        Err(e) => eprintln!("error: the game crashed: {}; the crash report was not written: {e}", report.what.message),
    }
    ExitCode::from(101)
}

/// The game itself, from `build` to the end of its run.
fn play(build: &crate::heavy::Builder, args: &Args, config: EngineConfig, recording: Option<Replay>) -> ExitCode {
    let mut app = match built(&|config: &EngineConfig| build(config), &config) {
        Ok(app) => app,
        Err(code) => return code,
    };
    // The twin of a heavy check plays as the game, not as the player at this machine (settings,
    // Steam): it is built from what the command line said.
    crate::heavy::install(&mut app, &config, build.clone());
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
                without: config.without.clone(),
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
    let net = session_ready(&config, args.name.as_deref().or(matches!(config.net, NetLaunch::Stand { .. }).then_some("host")));
    println!("{}{net}", ai_core::ready_line(0, std::process::id(), config.seed, config.port));
    exit_code(app.run())
}

/// `build(config)`. Offscreen without a GPU, Bevy panics while it builds the renderer: that is no
/// crash but a machine that cannot draw, so it ends with what to do instead (exit 2).
fn built(build: &impl Fn(&EngineConfig) -> App, config: &EngineConfig) -> Result<App, ExitCode> {
    if !config.offscreen {
        return Ok(build(config));
    }
    let payload = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(config))) {
        Ok(app) => return Ok(app),
        Err(payload) => payload,
    };
    let message = ai_core::crash::panic_text(payload.as_ref());
    if !message.starts_with("Unable to find a GPU") {
        std::panic::resume_unwind(payload);
    }
    ai_core::crash::handled(&message);
    eprintln!(
        "error: --offscreen draws on the GPU, and there is none: {message} \
         Run with --headless (no pictures), or let a software renderer draw: WGPU_FORCE_FALLBACK_ADAPTER=1"
    );
    Err(ExitCode::from(2))
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

    /// How a stand's members reach one another.
    enum Wire {
        /// The stand's in-memory hub: a client says hello to the host at once.
        Hub,
        /// A fake Steam of this process whose relay is the hub (`--stand-steam`): the host makes a lobby,
        /// and a client waits for one (`steam_lobbies`, then `steam_join`). The hub's links add no
        /// delay: `--net-sim` conditions the session's transport over the relay (`conditioner_from`), as
        /// it does over the real Steam, and once.
        #[cfg(feature = "steam")]
        Steam(shards_steam::FakeSteam),
    }

    impl Wire {
        /// The wire a stand with `config` has, on `hub`.
        #[cfg_attr(not(feature = "steam"), allow(unused_variables))]
        fn of(config: &EngineConfig, hub: &MemoryHub) -> Self {
            #[cfg(feature = "steam")]
            if config.steam.host {
                return Wire::Steam(shards_steam::FakeSteam::new(hub.clone()));
            }
            Wire::Hub
        }

        /// What a client's `--join` names.
        fn host_address(&self) -> &'static str {
            match self {
                Wire::Hub => "host",
                #[cfg(feature = "steam")]
                Wire::Steam(_) => "steam:",
            }
        }

        /// Gives member `n` (0: the host) of the stand, `app`, called `name`, its way to the others;
        /// the client's link to the host, when the wire has one (`links` conditions it).
        #[cfg_attr(not(feature = "steam"), allow(unused_variables))]
        fn attach(&self, app: &mut App, hub: &MemoryHub, n: usize, name: &str, links: LinkConditions) -> Option<shards_net::LinkId> {
            match self {
                Wire::Hub => {
                    let id = shards_net::stand::attach(app, hub, name, (n > 0).then_some(0));
                    (n > 0).then(|| hub.connect_both(0, id, links))
                }
                #[cfg(feature = "steam")]
                Wire::Steam(steam) => {
                    shards_steam::use_fake(app, steam.user(shards_steam::fake::FIRST_USER + n as u64, name));
                    None
                }
            }
        }
    }

    pub fn run(build: &crate::heavy::Builder, args: &Args, config: EngineConfig, clients: u8) -> ExitCode {
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
        let mut stand = match members(&|config: &EngineConfig| build(config), &config, &host_name, links, &ports, Some(build)) {
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
                // A client over Steam that has no lobby yet: `steam_lobbies` finds one, `steam_join` enters it.
                Some(NetState::Joining(why)) => format!("role client · joining: {why}"),
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
        twin: Option<&crate::heavy::Builder>,
    ) -> Result<Stand, String> {
        // The stand owns time: every member is paused and ticks when it says.
        let host_config = EngineConfig { paused: true, player_name: host_name.to_owned(), port: ports[0], ..config.clone() };
        let hub = MemoryHub::new(config.seed);
        let wire = Wire::of(config, &hub);
        let mut host = build(&host_config);
        // The host is the player at this machine: its settings are read as a game's of its own
        // are (`--settings-dir`, `--no-settings`). The clients keep none (`SettingsStore::Off`).
        shards_settings::install(&mut host, &host_config)?;
        host.add_systems(First, termination::exit_when_signalled);
        // Heavy mode checks the host: the clients do not simulate.
        if let Some(twin) = twin {
            crate::heavy::install(&mut host, &host_config, twin.clone());
        }
        wire.attach(&mut host, &hub, 0, "host", links);
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
                net: NetLaunch::Join { addr: wire.host_address().into() },
                player_name: name.clone(),
                settings: SettingsStore::Off,
                // Its own character file, named after it: members share the save folder.
                character_slot: Some(client_slot(number)),
                world_slot: None,
                // The lobby is the host's: a client has none to make.
                steam: SteamLaunch { lobby: SteamLobby::default(), host: false, ..config.steam.clone() },
                heavy: None,
                ..config.clone()
            };
            let mut app = build(&client_config);
            client_links.push(wire.attach(&mut app, &hub, n, &name, links));
            app.finish();
            app.cleanup();
            apps.push((name, app));
        }
        let mut stand = Stand::new(hub, apps);
        for (i, link) in client_links.into_iter().enumerate() {
            stand.members[i + 1].link = link;
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
fn verify(build: &crate::heavy::Builder, config: EngineConfig, recording: Replay) -> ExitCode {
    // Verifying writes nothing, not even a crash replay over the one being verified (a failed
    // heavy check saves what it found: heavy mode is asked for, to find out).
    let config =
        EngineConfig { headless: true, port: None, paused: true, crash_replays: false, hot_reload: false, engine_invariants: true, ..config };
    let end = recording.end.tick;
    let mut app = build(&config);
    crate::heavy::install(&mut app, &config, build.clone());
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
    for warning in left_out_warnings(app.world()) {
        println!("{warning}");
    }
    let report = app.world().resource::<AiReplay>().report().cloned();
    let took = format!("[{} ticks in {:.2}s]", ai_core::sim::tick(app.world()), started.elapsed().as_secs_f64());
    let failure = stops.failures.first().map(|f| format!("gameplay {}: {}", f.headline(), f.message));
    let heavy = app.world().get_resource::<ai_core::heavy::AiHeavy>().map(|h| HeavyRun {
        failures: stops.heavy.clone(),
        checked: h.checked(),
        skipped: h.skipped(),
    });
    match print_verdict(report.as_ref(), failure.as_deref(), &stops.violations, heavy.as_ref(), &took) {
        Some(true) => ExitCode::SUCCESS,
        Some(false) => ExitCode::FAILURE,
        None => {
            eprintln!("error: the replay did not finish");
            ExitCode::from(2)
        }
    }
}

/// A gameplay component the checks cannot hash is a hole in every check: `--verify` says so,
/// one line each (the reference replay test fails on them).
fn left_out_warnings(world: &World) -> Vec<String> {
    let paths = world.get_resource::<ai_core::replay::ReplayHashing>().map(|h| h.reported_left_out()).unwrap_or_default();
    paths.into_iter().map(|path| format!("warning: replay checks leave out {path}: it does not serialize through reflection")).collect()
}

/// What heavy mode found in a `--verify --heavy N`.
struct HeavyRun {
    failures: Vec<ai_core::HeavyFailure>,
    checked: u64,
    skipped: u64,
}

/// Prints what `--verify` found: every violation (`invariant hp violated at tick 31: …`) and
/// failed heavy check, the verdict line (`replay finished at tick 600: identical to the
/// recording (10 checks); 1 invariant violated [600 ticks in 1.20s]`), then what diverged
/// holds in this run, a line per value. Returns whether it passed; `None` when there is
/// nothing to say (no report, no failure).
fn print_verdict(
    report: Option<&ReplayReport>,
    failure: Option<&str>,
    violations: &[Violation],
    heavy: Option<&HeavyRun>,
    took: &str,
) -> Option<bool> {
    for v in violations {
        println!("{}: {}", v.headline(), v.text);
    }
    let heavy_failures = heavy.map_or(&[][..], |h| &h.failures[..]);
    for f in heavy_failures {
        println!("{} (a fresh game from tick {}): {}", f.headline(), f.from, f.first());
        for line in f.differences.iter().skip(1).take(9) {
            println!("  {line}");
        }
        if let Some(path) = &f.replay {
            println!("  replay: {path}");
        }
    }
    let mut broken = match violations.len() {
        0 => String::new(),
        1 => "; 1 invariant violated".to_owned(),
        n => format!("; {n} invariants violated"),
    };
    if let Some(h) = heavy {
        let failed = match h.failures.len() {
            0 => String::new(),
            1 => ", 1 failed".to_owned(),
            n => format!(", {n} failed"),
        };
        broken.push_str(&format!("; heavy: {} checked{failed}, {} skipped", h.checked, h.skipped));
    }
    let violations_or_heavy = !violations.is_empty() || !heavy_failures.is_empty();
    let passed = match (report, failure) {
        (_, Some(failure)) => {
            let shown = report.map(|r| format!("{}; ", r.headline())).unwrap_or_default();
            println!("{shown}{failure}{broken} {took}");
            false
        }
        (Some(r), None) => {
            println!("{}{broken} {took}", r.headline());
            r.identical() && !violations_or_heavy
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
            EngineConfig { headless: false, port: None, paused: true, crash_replays: false, hot_reload: false, engine_invariants: true, ..config };
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
            let passed = print_verdict(Some(&report), None, &violations, None, &took(world)) == Some(true);
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
                print_verdict(None, Some(&failure), &violations, None, &took(world));
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
    // The loading picture is shown before content loads, so the game names it in its config.
    let picture = app.world().resource::<EngineConfig>().loading_image.clone();
    let missing = picture.filter(|path| !root.join(path).is_file()).map(|path| {
        format!("loading_image: no file `{path}` under the asset folder (the picture the game's EngineConfig names for loading screens)")
    });
    // Regions of the kinds other modules add are generated once (their check seeds), so a
    // step that can never pass is a content error before any report runs.
    let generated = || -> Vec<shards_content::ContentError> {
        let Some(worldgen) = app.world().get_resource::<shards_world::WorldGen>() else { return Vec::new() };
        if worldgen.kinds().is_empty() {
            return Vec::new();
        }
        let Ok(sources) = shards_content::db::read_dir(&root) else { return Vec::new() };
        let mut kinds = kinds.clone();
        kinds.set_asset_root(root.clone());
        shards_world::kind::generation_problems(&shards_content::ContentDb::build(&kinds, &sources), worldgen)
    };
    let checked = check_dir(&root, &kinds).and_then(|summary| {
        let problems = generated();
        if problems.is_empty() { Ok(summary) } else { Err(problems) }
    });
    match (checked, missing) {
        (Ok(summary), None) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        (outcome, missing) => {
            let errors: Vec<String> = outcome.err().unwrap_or_default().iter().map(ToString::to_string).chain(missing).collect();
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

/// `--bake-motion [ACTIONS]`: the game's content loaded headless, the displacement of its actions
/// baked from their clips into their content files, the report printed; exit 0 (every action
/// baked) or 1.
fn bake_motion(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, actions: &str) -> ExitCode {
    let config = EngineConfig { headless: true, testing: true, port: None, paused: true, crash_replays: false, hot_reload: false, ..config };
    let mut app = build(&config);
    app.finish();
    app.cleanup();
    if let Err(e) = crate::testing::wait_until_loaded(&mut app, crate::testing::LOAD_TIMEOUT) {
        println!("error: {e}");
        return ExitCode::FAILURE;
    }
    match shards_action::bake::bake_files(app.world(), Some(actions)) {
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

/// `--smoke TICKS`: the game loaded headless and run for TICKS ticks as fast as they go, then
/// `ok: loaded in 1.20s, 600 ticks in 0.84s` and exit 0; or what stopped it (`error: the game did
/// not finish loading …`, `gameplay system … panicked …`, every invariant it broke) and exit 1.
/// Writes nothing: no settings, no crash replay. `shards package` runs it on the build it made.
fn smoke(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, ticks: u64) -> ExitCode {
    let config =
        EngineConfig { headless: true, port: None, paused: true, crash_replays: false, hot_reload: false, engine_invariants: true, ..config };
    let started = Instant::now();
    let mut app = build(&config);
    app.finish();
    app.cleanup();
    if let Err(e) = crate::testing::wait_until_loaded(&mut app, crate::testing::LOAD_TIMEOUT) {
        println!("error: {e}");
        return ExitCode::FAILURE;
    }
    let loaded = started.elapsed();
    let started = Instant::now();
    let Some(end) = ai_core::sim::tick(app.world()).checked_add(ticks) else {
        println!("error: --smoke {ticks}: the game cannot count that many ticks");
        return ExitCode::FAILURE;
    };
    let stops = match crate::testing::run_to(&mut app, end, crate::testing::LOAD_TIMEOUT) {
        Ok(stops) => stops,
        Err(e) => {
            println!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    for v in &stops.violations {
        println!("{}: {}", v.headline(), v.text);
    }
    if let Some(failure) = stops.failures.first() {
        println!("gameplay {}: {}", failure.headline(), failure.message);
        return ExitCode::FAILURE;
    }
    if !stops.violations.is_empty() {
        return ExitCode::FAILURE;
    }
    println!("ok: loaded in {:.2}s, {ticks} ticks in {:.2}s", loaded.as_secs_f64(), started.elapsed().as_secs_f64());
    ExitCode::SUCCESS
}

/// `--perf ID`: the performance autotest ([`crate::bench`]). Prints where it ran, what it measured,
/// a line per budget and the verdict: exit 0 (every budget met; a debug build, which judges
/// nothing, too), 1 (a budget exceeded, or the game failed on the way) or 2 (no such definition,
/// no GPU offscreen). Writes nothing.
fn perf(build: &impl Fn(&EngineConfig) -> App, config: EngineConfig, id: &str) -> ExitCode {
    let def = match crate::bench::read(build, &config, id) {
        Ok(def) => def,
        Err(e) => {
            println!("error: {e}");
            return ExitCode::from(2);
        }
    };
    let config = crate::bench::config(&def, config);
    let mut app = match built(build, &config) {
        Ok(app) => app,
        Err(code) => return code,
    };
    app.finish();
    app.cleanup();
    let measured = match crate::bench::play(&mut app, &def) {
        Ok(measured) => measured,
        Err(e) => {
            println!("error: perf {id}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let judged = !cfg!(debug_assertions);
    let (width, height) = config.window;
    let display = ai_core::display::status(app.world());
    let place = match display["gpu"].as_str().filter(|_| config.offscreen) {
        Some(gpu) => format!("offscreen {width}x{height} on {gpu}"),
        None => "headless".to_owned(),
    };
    println!("perf {id}: {} build, {place}", if judged { "release" } else { "debug" });
    match &def.region {
        Some(r) => {
            let took = measured.region_load.unwrap_or_default().as_secs_f64();
            let baked = measured
                .nav_bake
                .map(|(bakes, took)| format!("; its navigation baked in {:.2}s ({bakes} tiles x body classes)", took.as_secs_f64()))
                .unwrap_or_default();
            println!("loaded in {:.2}s, region {} seed {} in {took:.2}s of it{baked}", measured.load.as_secs_f64(), r.region, r.seed);
        }
        None => println!("loaded in {:.2}s", measured.load.as_secs_f64()),
    }
    println!("{}", measured.line());
    let verdicts = crate::bench::judge(&def.budgets, &measured, judged);
    for verdict in &verdicts {
        println!("{}", verdict.line());
    }
    if !judged {
        println!("perf {id}: not judged: a debug build runs slower than the players' (the budgets are for --release)");
        return ExitCode::SUCCESS;
    }
    let checked = verdicts.iter().filter(|v| v.met.is_some()).count();
    match verdicts.iter().filter(|v| v.met == Some(false)).count() {
        0 => {
            println!("perf {id}: all {checked} budgets met");
            ExitCode::SUCCESS
        }
        exceeded => {
            println!("perf {id}: {exceeded} of {checked} budgets exceeded");
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
        app.add_plugins(EnginePlugins::new(config.clone())).add_systems(OnEnter(EngineState::InGame), spawn_totem);
        app
    }

    /// The test game's world: one totem.
    fn spawn_totem(world: &mut World) {
        shards_content::spawn_prefab(world, "totem", Transform::default(), None).unwrap();
    }

    /// [`game`], as `run` shares it.
    fn built() -> crate::heavy::Builder {
        std::sync::Arc::new(game)
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
        assert_eq!(verify(&built(), config, recording), ExitCode::SUCCESS);
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
    fn the_flag_for_rules_off_by_default_takes_only_the_names_offered() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert!(parse(&["game"]).unwrap().config().opt_in_invariants.is_empty(), "none named");
        // `voxel.body_in_solid` is on by default now: it is not offered, and nothing else is.
        let on = parse(&["game", "--engine-invariant", "voxel.body_in_solid"]).unwrap_err().to_string();
        assert!(on.contains("invalid value") && on.contains("voxel.body_in_solid"), "{on}");
        assert!(OPT_IN_INVARIANTS.is_empty());
    }

    #[test]
    fn offscreen_is_a_way_to_play_not_to_check() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        let config = parse(&["game", "--offscreen", "--paused"]).unwrap().config();
        assert!(config.offscreen && !config.headless && config.draws());
        assert!(parse(&["game", "--offscreen", "--replay", "r.json"]).unwrap().config().offscreen, "a replay plays offscreen");
        for clash in [&["--headless"][..], &["--smoke", "60"], &["--check-content"], &["--replay", "r.json", "--verify"], &["--stand", "2"]] {
            let args: Vec<&str> = ["game", "--offscreen"].into_iter().chain(clash.iter().copied()).collect();
            assert!(parse(&args).unwrap_err().to_string().contains("cannot be used with"), "{clash:?}");
        }
        assert!(!parse(&["game", "--headless"]).unwrap().config().draws());
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
        // Modules left out: a list, checked as a whole and against what else is asked for.
        let without = parse(&["game", "--without", "audio,scatter", "--without", "fx"]).unwrap();
        assert_eq!(without.config().without, ["audio", "scatter", "fx"]);
        assert_eq!(check_without(&without), Ok(()));
        let modules = |args: &[&str]| check_without(&parse(args).unwrap()).unwrap_err();
        assert!(modules(&["game", "--without", "motion"]).starts_with("--without: `motion` runs in every game"));
        assert!(modules(&["game", "--without", "locale"]).contains("`ui` (screens, menus and the HUD) needs `locale`"));
        // `steam` needs `net`: built in, it goes too, or `net` alone is refused before the session is asked about.
        assert!(modules(&["game", "--without", "net,steam", "--host"]).starts_with("--without net leaves out co-op sessions"));
        if cfg!(feature = "steam") {
            assert!(modules(&["game", "--without", "net"]).contains("`steam` (") && modules(&["game", "--without", "net"]).contains("needs `net`"));
        }
        assert!(modules(&["game", "--without", "save", "--character", "ann"]).starts_with("--without save leaves out saves"));
        assert!(modules(&["game", "--without", "world", "--worldgen-report", "wilds"]).starts_with("--without world leaves out the region generator"));
        for steam in [&["--steam-host"][..], &["--steam-server"], &["--headless", "--stand", "1", "--stand-steam"], &["--join", "steam:90071992547409921"], &["--steam-app", "480"]] {
            let args: Vec<&str> = ["game", "--without", "steam"].into_iter().chain(steam.iter().copied()).collect();
            assert!(modules(&args).starts_with("--without steam leaves out Steam"), "{steam:?}: {}", modules(&args));
        }

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
        let stand = stand::members(&game, &config, "host", shards_net::LinkConditions::NONE, &[None, None], None).unwrap();
        let file = |member: usize| stand.world(member).get_resource::<shards_settings::SettingsStorage>().map(|s| s.file.clone());
        assert_eq!(file(0), Some(Some(dir.join("settings.toml"))), "the host's settings live in --settings-dir");
        assert_eq!(file(1), None, "a client of the stand keeps none");
    }

    /// Gameplay state a check cannot hash: an `Instant` does not serialize through reflection.
    #[derive(Component, Reflect)]
    #[reflect(Component)]
    struct Stamped {
        at: std::time::Instant,
    }

    #[test]
    fn verify_names_the_components_the_checks_leave_out() {
        let config = content("left_out", "totem");
        let recording = record(&config, |app| {
            ok(app, "step", json!({ "ticks": 61 }));
        });
        let mut clean = game(&config);
        clean.finish();
        clean.cleanup();
        testing::verify(&mut clean, recording.clone()).unwrap();
        assert!(left_out_warnings(clean.world()).is_empty());
        // The same game with a component the checks cannot hash.
        let mut app = game(&config);
        // After the totem is spawned: two unordered systems of one schedule run in whichever order it sorts them.
        let stamp = |mut commands: Commands, totems: Query<Entity, With<ai_core::AiId>>| {
            for totem in &totems {
                commands.entity(totem).insert(Stamped { at: std::time::Instant::now() });
            }
        };
        app.register_type::<Stamped>().add_systems(OnEnter(EngineState::InGame), stamp.after(spawn_totem));
        app.finish();
        app.cleanup();
        testing::verify(&mut app, recording).unwrap();
        assert_eq!(
            left_out_warnings(app.world()),
            ["warning: replay checks leave out shards::run::tests::Stamped: it does not serialize through reflection"]
        );
    }

    /// A stand of a host and one client, with heavy mode every 30 ticks: the client gets a body.
    fn heavy_stand(name: &str, build: fn(&EngineConfig) -> App) -> shards_net::Stand {
        let config = EngineConfig {
            net: NetLaunch::Stand { clients: 1, udp: false },
            heavy: Some(ai_core::HeavyConfig::every(30)),
            ..content(name, "totem")
        };
        std::fs::write(std::path::Path::new(&config.asset_dir).join("content/players.prefab.ron"), r#"{ "player": (components: { "AiKind": ("Player") }) }"#).unwrap();
        let twin: crate::heavy::Builder = std::sync::Arc::new(build);
        let mut stand = stand::members(&build, &config, "host", shards_net::LinkConditions::NONE, &[None, None], Some(&twin)).unwrap();
        stand.settle().unwrap();
        stand
    }

    /// The host's own count, which no snapshot holds, in the totem's name.
    #[derive(Resource, Default)]
    struct Drift(u32);

    fn drifting(config: &EngineConfig) -> App {
        let mut app = game(config);
        app.init_resource::<Drift>().add_systems(
            FixedUpdate,
            (|mut drift: ResMut<Drift>, mut names: Query<&mut Name>| {
                drift.0 += 1;
                for mut name in names.iter_mut().filter(|n| n.as_str().starts_with("totem")) {
                    name.set(format!("totem {}", drift.0));
                }
            })
            .in_set(ai_core::SimSet::Resolve),
        );
        app
    }

    /// `--stand N --heavy`: the host is checked (the clients do not simulate), its guest's body
    /// and input with it, and a difference halts the stand as a broken invariant does.
    #[test]
    fn heavy_mode_on_a_stand_checks_the_host_and_halts_it_on_a_difference() {
        let mut stand = heavy_stand("stand_heavy", game);
        stand.run(120);
        assert!(matches!(stand.world(1).resource::<shards_net::NetState>(), shards_net::NetState::Connected { player: 1, .. }));
        let heavy = stand.world(0).resource::<ai_core::heavy::AiHeavy>();
        assert!(heavy.checked() >= 3 && heavy.failures().is_empty(), "{}", heavy.summary());
        assert!(stand.world(1).get_resource::<ai_core::heavy::AiHeavy>().is_none(), "a client is not checked");

        let mut stand = heavy_stand("stand_heavy_drift", drifting);
        let halted = (0..120).find_map(|_| Some(stand.tick().unwrap()).filter(|h| !h.is_empty())).expect("the host's drift is found");
        let [(0, shards_net::stand::MemberHalt::Heavy(failure))] = &halted[..] else { panic!("{halted:?}") };
        assert_eq!((failure.from, failure.tick), (30, 60), "{failure:?}");
        assert!(failure.first().contains("Name") && failure.first().contains("totem 60"), "{failure:?}");
    }

    #[test]
    fn a_stand_over_a_fake_steam_is_asked_for_with_its_own_flag_and_takes_the_lobby_type() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        let stand = parse(&["game", "--headless", "--stand", "2", "--stand-steam", "--steam-lobby", "invisible"]).unwrap().config();
        assert_eq!((stand.net, stand.steam.host, stand.steam.lobby), (NetLaunch::Stand { clients: 2, udp: false }, true, SteamLobby::Invisible));
        assert!(!stand.steam.connect, "a stand never reaches the real Steam");
        let plain = parse(&["game", "--headless", "--stand", "2"]).unwrap().config();
        assert!(!plain.steam.host, "a plain stand makes no lobby");
        for refused in [
            &["game", "--stand-steam"][..],
            &["game", "--headless", "--stand", "1", "--stand-udp", "--stand-steam"],
            &["game", "--headless", "--stand", "1", "--steam-lobby", "public"],
            &["game", "--headless", "--stand", "1", "--stand-steam", "--steam-host"],
        ] {
            assert!(parse(refused).is_err(), "{refused:?}");
        }
    }

    /// `--stand-steam`: the host of the stand makes a lobby over the fake Steam, the client waits for
    /// one, finds it with `steam_lobbies`, enters it with `steam_join` and plays without a desync.
    #[cfg(feature = "steam")]
    #[test]
    fn a_stand_over_a_fake_steam_has_a_lobby_host_and_a_client_that_finds_and_enters_it() {
        use shards_net::{NetState, Stand};
        let config = EngineConfig {
            net: NetLaunch::Stand { clients: 1, udp: false },
            steam: SteamLaunch { host: true, lobby: SteamLobby::Invisible, ..SteamLaunch::default() },
            // 50 ms each way, over the relay.
            net_sim: Some("latency=50".into()),
            ..content("stand_steam", "totem")
        };
        // The body a joining player gets.
        std::fs::write(std::path::Path::new(&config.asset_dir).join("content/players.prefab.ron"), r#"{ "player": (components: { "AiKind": ("Player") }) }"#).unwrap();
        let mut stand = stand::members(&game, &config, "host", shards_net::LinkConditions::NONE, &[None, None], None).unwrap();
        stand.settle().unwrap();
        fn session(stand: &Stand, member: usize) -> &shards_steam::SteamSession {
            stand.world(member).resource::<shards_steam::SteamSession>()
        }
        let lobby = session(&stand, 0).lobby_id().expect("the host made its lobby");
        assert_eq!((session(&stand, 0).role, session(&stand, 0).lobby_type), (shards_steam::SteamRole::Host, Some(SteamLobby::Invisible)));
        assert_eq!(session(&stand, 1).role, shards_steam::SteamRole::Client);
        assert_eq!(session(&stand, 1).lobby_id(), None, "the client waits for a lobby");
        assert_eq!(ai_core::status::status(stand.world(0))["net"]["stand"]["clients"], 1, "`shards wait --peer all` counts the stand's clients");

        let found = stand.call(1, "steam_lobbies", serde_json::json!({}));
        assert!(found.ok && found.result.as_str().unwrap().contains(&format!("lobby {lobby} ")), "{}", found.result);
        let joined = stand.call(1, "steam_join", serde_json::json!({ "lobby": lobby }));
        assert!(joined.ok, "{}", joined.result);
        stand.settle().unwrap();
        stand.run(240);
        assert!(matches!(stand.world(1).resource::<NetState>(), NetState::Connected { player: 1, .. }), "{:?}", stand.world(1).resource::<NetState>());
        // `--net-sim` conditions the transport once: a round trip is twice the one way, not four times.
        let status = stand.call(1, "net_status", serde_json::json!({})).result;
        let rtt = status["rtt_ms"].as_f64().unwrap_or_else(|| panic!("no rtt in: {status}"));
        assert!((80.0..=140.0).contains(&rtt), "50 ms each way is a round trip of about 100 ms, not {rtt}: {status}");
        assert_eq!(ai_core::status::status(stand.world(1))["net"]["desyncs"], 0);
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
    fn steam_sessions_host_without_a_port_and_say_so_in_the_ready_line() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        let lobby = parse(&["game", "--steam-host"]).unwrap().config();
        assert_eq!((lobby.net.clone(), lobby.steam.host, lobby.steam.server), (NetLaunch::Host { port: 0 }, true, false));
        assert!(!lobby.steam.connect, "only `run` connects, for a game that is played");
        assert_eq!(session_ready(&lobby, None), " · role host · net steam lobby");

        let server = parse(&["game", "--steam-server", "--steam-relay-only", "--steam-app", "480"]).unwrap().config();
        assert_eq!(server.net, NetLaunch::Host { port: 0 });
        assert_eq!(server.steam, SteamLaunch { connect: false, app_id: Some(480), host: false, server: true, relay_only: true, lobby: SteamLobby::Friends });
        assert_eq!(session_ready(&server, Some("srv")), " · member srv · role host · net steam game server");

        let client = parse(&["game", "--join", "steam:90071992547409921"]).unwrap().config();
        assert_eq!(session_ready(&client, None), " · role client · net steam:90071992547409921");
        let udp = parse(&["game", "--join", "1.2.3.4:7777"]).unwrap().config();
        assert_eq!(session_ready(&udp, None), " · role client · net udp:1.2.3.4:7777", "a UDP session reads as before");

        for clash in [&["game", "--steam-host", "--host"][..], &["game", "--steam-host", "--steam-server"], &["game", "--steam-server", "--join", "x:1"]] {
            assert!(parse(clash).is_err(), "{clash:?}");
        }
        assert!(parse(&["game", "--steam-app", "0"]).is_err(), "no app 0");

        // The lobby's type: friends only unless --steam-lobby says otherwise, and only for a lobby host.
        assert_eq!(lobby.steam.lobby, SteamLobby::Friends);
        for (name, kind) in [("friends", SteamLobby::Friends), ("private", SteamLobby::Private), ("invisible", SteamLobby::Invisible), ("public", SteamLobby::Public)] {
            assert_eq!(parse(&["game", "--steam-host", "--steam-lobby", name]).unwrap().config().steam.lobby, kind);
        }
        let typo = parse(&["game", "--steam-host", "--steam-lobby", "secret"]).unwrap_err().to_string();
        assert!(typo.contains("`secret` is no lobby type: friends, private, invisible, public"), "{typo}");
        assert!(parse(&["game", "--steam-lobby", "private"]).is_err(), "a lobby type without a lobby host");
        assert!(parse(&["game", "--steam-server", "--steam-lobby", "private"]).is_err(), "a game server has no lobby");

        // Steam's `+connect_lobby` is no flag of the game's: with `steam` it becomes `--join
        // steam:<lobby>` before this (`shards_steam::connect_lobby_args`), without it clap refuses it.
        let steam_start = parse(&["game", "+connect_lobby", "109775240917319681"]).unwrap_err();
        assert_eq!(steam_start.kind(), clap::error::ErrorKind::UnknownArgument);
        assert!(steam_start.to_string().starts_with("error: unexpected argument '+connect_lobby' found"), "{steam_start}");
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
        assert_eq!(verify(&built(), config, recording), ExitCode::SUCCESS);
    }

    #[test]
    fn verify_cannot_play_a_replay_whose_snapshot_this_build_cannot_load() {
        let config = content("snapshot_other", "Totem");
        let file = Scratch::new("other");
        let mut recording = recorded_from_a_snapshot(&config, &file);
        recording.start.as_mut().unwrap().checks.insert("content".into(), "00000000000000ff".into());
        // Exit code 2: it could not be played (1 is a replay that diverged).
        assert_eq!(verify(&built(), config, recording), ExitCode::from(2));
    }

    #[test]
    fn verify_plays_a_replay_that_ends_at_tick_zero_on_the_spawned_world() {
        let config = content("zero", "Totem");
        let recording = record(&config, |app| {
            ok(app, "teleport", json!({"id": "e1", "x": 3, "z": 0}));
        });
        assert_eq!(recording.end.tick, 0);
        assert_eq!(verify(&built(), config, recording), ExitCode::SUCCESS);
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

    #[test]
    fn bake_motion_writes_the_curves_content_checks() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert_eq!(parse(&["game", "--bake-motion"]).unwrap().bake_motion.as_deref(), Some("all"));
        assert_eq!(parse(&["game", "--bake-motion", "roll,light_1"]).unwrap().bake_motion.as_deref(), Some("roll,light_1"));
        assert!(parse(&["game", "--bake-motion", "--bake-nav"]).is_err(), "one bake at a time");
        assert!(parse(&["game", "--bake-motion", "--offscreen"]).is_err());

        let config = content("bake_motion", "Totem");
        let dir = std::path::PathBuf::from(&config.asset_dir);
        let model = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../sandbox/assets/models/humanoid.glb")).unwrap();
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("models/humanoid.glb"), model).unwrap();
        let set = r#"{ "humanoid": (source: "models/humanoid.glb", clips: {"roll": (), "idle": ()}, root_motion: "root") }"#;
        let actions = r#"{
    "roll": (
        net_id: 1,
        phases: [("roll", 0.4), ("recover", 0.1)],
        displacement: [(0.35, 4.0)],
        bake: (animset: "humanoid", clip: "roll"),
    ),
}"#;
        std::fs::write(dir.join("content/h.animset.ron"), set).unwrap();
        std::fs::write(dir.join("content/a.action.ron"), actions).unwrap();
        assert_eq!(check_content(&game, config.clone()), ExitCode::FAILURE, "the hand-made curve is not the clip's");
        assert_eq!(bake_motion(&game, config.clone(), "all"), ExitCode::SUCCESS);
        let baked = std::fs::read_to_string(dir.join("content/a.action.ron")).unwrap();
        assert!(!baked.contains("displacement: [(0.35, 4.0)]") && baked.contains("bake: (animset"), "{baked}");
        assert_eq!(check_content(&game, config.clone()), ExitCode::SUCCESS, "a fresh bake is clean content");
        assert_eq!(bake_motion(&game, config, "rol"), ExitCode::FAILURE, "an unknown action fails");
    }

    #[test]
    fn perf_plays_a_definition_and_names_what_stops_it() {
        let parse = |args: &[&str]| Args::try_parse_from(args.iter().copied());
        assert_eq!(parse(&["game", "--perf", "quick", "--offscreen"]).unwrap().perf.as_deref(), Some("quick"));
        assert!(parse(&["game", "--perf", "quick", "--smoke", "5"]).is_err(), "one check at a time");

        let config = content("perf", "Totem");
        let defs = r#"{
            "quick": (setup: ["teleport id=e1 x=3 z=0"], warmup: 10, ticks: 60, budgets: (entities: Some(100000))),
            "broken": (setup: ["teleport id=e9 x=3 z=0"], ticks: 10),
        }"#;
        std::fs::write(std::path::PathBuf::from(&config.asset_dir).join("content/t.perf.ron"), defs).unwrap();
        let def = crate::bench::read(&game, &config, "quick").unwrap();
        let mut app = game(&crate::bench::config(&def, config.clone()));
        app.finish();
        app.cleanup();
        let measured = crate::bench::play(&mut app, &def).unwrap();
        assert_eq!((measured.ticks.ticks, measured.frames, measured.region_load), (60, None, None));
        assert_eq!(ai_core::sim::tick(app.world()), 70, "the warmup, then the ticks measured");
        let totem = ai_core::Id(ai_core::AiId(1)).entity(app.world()).unwrap();
        assert_eq!(app.world().get::<Transform>(totem).unwrap().translation.x, 3.0, "the setup ran");
        assert!(measured.entities > 0 && measured.line().starts_with("60 ticks in "), "{}", measured.line());

        assert_eq!(perf(&game, config.clone(), "quick"), ExitCode::SUCCESS);
        assert_eq!(perf(&game, config.clone(), "broken"), ExitCode::FAILURE, "a setup line that fails stops the run");
        assert_eq!(perf(&game, config, "quik"), ExitCode::from(2), "no such definition");
    }

    #[test]
    fn a_panic_that_takes_the_game_with_it_leaves_a_crash_report() {
        ai_core::crash::install_panic_hook();
        let base = std::env::temp_dir().join(format!("shards_guarded_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(guarded(true, Some(&base), || ExitCode::from(7)), ExitCode::from(7), "a game that ends is left alone");
        assert!(!base.exists(), "and writes no report");

        assert_eq!(guarded(true, Some(&base), || panic!("the floor is lava")), ExitCode::from(101));
        let files: Vec<_> = std::fs::read_dir(base.join("crashes")).unwrap().map(|f| f.unwrap().path()).collect();
        assert_eq!(files.len(), 1, "{files:?}");
        let report: ai_core::crash::CrashReport = serde_json::from_str(&std::fs::read_to_string(&files[0]).unwrap()).unwrap();
        assert_eq!(report.format, ai_core::crash::FORMAT);
        assert_eq!(report.what.message, "the floor is lava");
        assert!(report.what.location.as_deref().is_some_and(|l| l.contains("run.rs")), "{:?}", report.what);
        assert!(!report.what.backtrace.is_empty(), "the hook kept the call stack");
        assert!(!report.system.os.is_empty());
        assert!(report.replay.is_none() && report.no_replay.as_deref().is_some_and(|w| w.contains("took the world")));
        let err = replay::load(&files[0]).unwrap_err();
        assert!(err.contains("is a crash report without a replay (the panic unwound"), "{err}");
        let _ = std::fs::remove_dir_all(base);
    }
}
