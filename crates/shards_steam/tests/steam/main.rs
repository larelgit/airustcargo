//! Steam end to end on the whole engine, on the fake Steam of this process: a lobby host and its
//! clients over the fake relay, a game server and a client that joins it by its id, sessions taken
//! and refused, achievements as content and who unlocks them, saves mirrored to the cloud and taken
//! from it at startup, and what the tools say. Nothing here talks to the Steam client.

mod achievements;
mod cloud;
mod session;
mod tools;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::Value;
use shards::ai_core::journal::JournalKind;
use shards::ai_core::{call_tool, AiJournal, CallOutcome};
use shards::core::{EngineConfig, NetLaunch, SteamLaunch};
use shards::net::{MemoryHub, Stand};
use shards::prelude::*;
use shards::save::SaveConfig;
use shards::testing;
use shards_steam::fake::{FIRST_SERVER, FIRST_USER};
use shards_steam::{FakeSteam, FakeUser, SteamPlugin, SteamSession};

/// The tests' achievements, languages and the body a joining player gets.
pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/steam/assets");

/// The second and third users of a fake Steam.
pub const SECOND_USER: u64 = FIRST_USER + 1;
pub const THIRD_USER: u64 = FIRST_USER + 2;

/// A save folder of an app's own, removed with it.
#[derive(Resource)]
pub struct Saves(pub PathBuf);

impl Drop for Saves {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A fresh, empty save folder for one app of one test.
pub fn save_folder() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("shards_steam_saves_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The test config on the tests' assets, changed by `configure`.
pub fn config(configure: impl FnOnce(&mut EngineConfig)) -> EngineConfig {
    let mut config = EngineConfig { asset_dir: ASSETS.to_owned(), ..testing::config() };
    configure(&mut config);
    config
}

/// The engine with the Steam module on `user` of a fake Steam, its saves in `saves`: built and
/// finished, not loaded yet.
pub fn build(config: EngineConfig, user: FakeUser, saves: PathBuf) -> App {
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config));
    app.add_plugins(SteamPlugin);
    shards_steam::use_fake(&mut app, user);
    app.world_mut().resource_mut::<SaveConfig>().dir = saves.clone();
    app.insert_resource(Saves(saves));
    app.finish();
    app.cleanup();
    app
}

/// [`build`], loaded and paused at tick 0, with a save folder of its own.
pub fn game(config: EngineConfig, user: FakeUser) -> App {
    game_in(config, user, save_folder())
}

/// [`game`] with its saves in `saves`.
pub fn game_in(config: EngineConfig, user: FakeUser, saves: PathBuf) -> App {
    let mut app = build(config, user, saves);
    if let Err(e) = testing::wait_until_loaded(&mut app, testing::LOAD_TIMEOUT) {
        panic!("{e}");
    }
    app
}

/// A single game (no session) on a fake Steam of its own, as user `FIRST_USER`.
pub fn single() -> (App, FakeSteam) {
    let steam = FakeSteam::new(MemoryHub::new(1));
    let app = game(config(|_| {}), steam.user(FIRST_USER, "player"));
    (app, steam)
}

/// A lobby host's config (`--steam-host`).
pub fn lobby_host(config: &mut EngineConfig) {
    config.net = NetLaunch::Host { port: 0 };
    config.steam = SteamLaunch { host: true, ..SteamLaunch::default() };
    config.player_name = "host".into();
}

/// A game server's config (`--steam-server`).
pub fn game_server(config: &mut EngineConfig) {
    config.net = NetLaunch::Host { port: 0 };
    config.steam = SteamLaunch { server: true, ..SteamLaunch::default() };
    config.player_name = "server".into();
}

/// A client's config: `--join steam:<id>` (`None`: `--join steam:`, waiting for an invite).
pub fn client(config: &mut EngineConfig, name: &str, id: Option<u64>) {
    config.net = NetLaunch::Join { addr: id.map_or_else(|| "steam:".to_owned(), |id| format!("steam:{id}")) };
    config.player_name = name.to_owned();
}

/// A stand on `steam`'s relay with only its host, `host`, loaded and its lobby made (or logged on).
pub fn stand_with_host(steam: &FakeSteam, host: App) -> Stand {
    let mut stand = Stand::new(steam.hub(), vec![("host".into(), host)]);
    stand.load_timeout = testing::LOAD_TIMEOUT;
    if let Err(e) = stand.settle() {
        panic!("{e}");
    }
    // The lobby (or the log-on) answers at the next pump.
    for _ in 0..3 {
        stand.frame();
    }
    stand
}

/// Adds a client app to `stand` and settles it: every client joined (or ended).
pub fn add_client(stand: &mut Stand, name: &str, app: App) -> usize {
    let member = stand.add(name, app, None);
    if let Err(e) = stand.settle() {
        panic!("{e}");
    }
    member
}

/// The host's Steam id in the session (the lobby's owner, the game server).
pub fn session(world: &World) -> &SteamSession {
    world.resource::<SteamSession>()
}

pub fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// The text of a call that must succeed.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    text(&out)
}

/// The error of a call that must fail.
pub fn err(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail: {}", out.result);
    text(&out)
}

/// The journal's lines of `kind` (any kind: `None`), in order.
pub fn journal(world: &World, kind: Option<JournalKind>) -> Vec<String> {
    world.resource::<AiJournal>().after(0).0.filter(|e| kind.is_none_or(|k| e.kind == k)).map(|e| e.text.clone()).collect()
}

/// Whether a journal line has `needle`.
pub fn journaled(world: &World, needle: &str) -> bool {
    journal(world, None).iter().any(|line| line.contains(needle))
}

/// Frames (no ticks) of `app` until `done`, at most a second of them.
pub fn frames_until(app: &mut App, done: impl Fn(&World) -> bool) {
    for _ in 0..1000 {
        if done(app.world()) {
            return;
        }
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("not done after 1000 frames");
}

/// The game server's id on a fake Steam.
pub const SERVER: u64 = FIRST_SERVER;
