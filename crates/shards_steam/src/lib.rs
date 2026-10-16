//! Shards on Steam (E10.5): lobbies and invites, the Steam relay as the co-op session's transport,
//! achievements as content, saves mirrored to Steam Cloud.
//!
//! The module is off by default (feature `steam` of `shards`). A game a player runs
//! ([`shards_core::SteamLaunch::connect`]) talks to the Steam client when it is built with the
//! Steamworks SDK (feature `steam_sdk`), and says it has no Steam otherwise; tests, checks and
//! stands run on a fake Steam of this process ([`fake`], [`use_fake`]). Nothing gameplay depends on Steam:
//! a session over the relay is the same session as over UDP, an achievement changes nothing in the
//! world, a cloud copy is only a copy of a save on disk.
//!
//! * `--steam-host`: a host in a new lobby (friends only, or `--steam-lobby private`, `invisible`,
//!   `public`); `--join steam:<lobby>` (and Steam's `+connect_lobby <lobby>`, see [`launch`]) enters
//!   one, the lobby's owner is the host. Relay sessions are taken only from the lobby ([`session`]).
//! * `--steam-server`: a host logged on as an anonymous Steam game server, without a lobby;
//!   `--join steam:<its id>` joins it over the relay. `--steam-relay-only` turns direct routes off,
//!   `--steam-app N` names the app; `steam` shows each peer's route (relayed, which relay, ping).
//! * `content/**/*.achievement.ron` ([`AchievementDef`]); the game unlocks them with
//!   [`UnlockAchievement`] or [`unlock_achievement`] ([`achievements`]).
//! * Every save written goes to Steam Cloud; at startup a newer cloud copy is taken, one from a
//!   newer version of the game never, and it is never overwritten ([`cloud`]).
//! * `steam_lobbies` searches for the lobbies of this game and build that a player can join: the
//!   host writes the game, engine, content and rules into its lobby's data, and Steam lists only
//!   `public` and `invisible` lobbies ([`lobbies`]).
//! * Tools: `steam`, `steam_lobbies`, `steam_host`, `steam_join`, `steam_invite`, `achievement_unlock`;
//!   `ai/status` field `steam`; the developer menu's `steam` section.

pub mod achievements;
mod backend;
pub mod cloud;
pub mod fake;
pub mod launch;
mod lobbies;
#[cfg(feature = "sdk")]
mod real;
pub mod session;
mod tools;
pub mod transport;

use ai_core::{AiAppExt, DevCommand};
use bevy::prelude::*;
use shards_content::ContentAppExt;
use shards_core::{EngineAppExt, EngineConfig};
use shards_net::{ExternalTransport, NetClock, NetEventAppExt, NetSystems};

pub use achievements::{flush_achievements, unlock_achievement, AchievementDef, Achievements, UnlockAchievement, Unlocked};
pub use backend::{Route, SteamId};
use backend::{Backend, Unavailable};
pub use cloud::Cloud;
pub use fake::{FakeStats, FakeSteam, FakeUser};
pub use launch::{account_type, connect_lobby_args, parse_address, SteamAddress};
pub use session::{SteamRole, SteamSession, SteamState};
pub use transport::RelayStats;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why a build without the SDK does not reach Steam.
pub const NO_SDK: &str = "this build has no Steamworks SDK: build the game with the `steam_sdk` feature of `shards`";

/// The Steam this game talks to: the real client, a fake, or none (and why).
#[derive(Resource)]
pub struct Steam {
    pub(crate) backend: Box<dyn Backend>,
}

impl Steam {
    /// `steam` (the real client), `fake`, or `none`.
    pub fn kind(&self) -> &'static str {
        self.backend.info().kind
    }

    /// This machine's user, when there is a Steam.
    pub fn user(&self) -> Option<SteamId> {
        self.backend.info().user
    }

    /// Why there is no Steam (`none`).
    pub fn unavailable(&self) -> Option<String> {
        if self.backend.available() { None } else { self.backend.info().note }
    }
}

/// Starts the Steam client before Bevy makes its renderer, so that Steam's overlay (invites, the
/// achievement popup, Shift+Tab) can hook the graphics device as it is created: a game a player
/// plays (`SteamLaunch::connect`) built with the SDK. `shards::EnginePlugins` adds it ahead of
/// `DefaultPlugins`' renderer; [`SteamPlugin`] then takes that client. Without it `SteamPlugin`
/// starts Steam itself, later, and the overlay may miss the window. Anything else (no SDK, a test,
/// a check) does nothing here.
pub struct SteamClientPlugin {
    #[cfg_attr(not(feature = "sdk"), allow(dead_code))]
    config: EngineConfig,
}

impl SteamClientPlugin {
    pub fn new(config: &EngineConfig) -> Self {
        Self { config: config.clone() }
    }
}

impl Plugin for SteamClientPlugin {
    #[cfg_attr(not(feature = "sdk"), allow(unused_variables))]
    fn build(&self, app: &mut App) {
        #[cfg(feature = "sdk")]
        if self.config.steam.connect && !app.world().contains_resource::<Started>() {
            app.insert_resource(Started(start(&self.config)));
        }
    }
}

/// The Steam client [`SteamClientPlugin`] started (or why it did not), for [`SteamPlugin`].
#[cfg(feature = "sdk")]
#[derive(Resource)]
struct Started(Result<real::RealBackend, String>);

/// The Steam module: see the crate's documentation.
pub struct SteamPlugin;

impl Plugin for SteamPlugin {
    fn build(&self, app: &mut App) {
        let config = app.world().get_resource::<EngineConfig>().cloned().unwrap_or_default();
        let backend = choose(app.world_mut(), &config);
        if session::wants_relay(&config) {
            // `shards_net` opens no UDP socket: the relay's transport comes once the lobby is there.
            let real_time = !matches!(backend.clock(), NetClock::Hub(_));
            app.insert_resource(ExternalTransport { name: "steam", real_time });
        }
        app.add_engine_module("steam", VERSION)
            .add_content::<AchievementDef>()
            .net_event::<UnlockAchievement>()
            .init_resource::<Achievements>()
            .init_resource::<Cloud>()
            .insert_resource(session::SteamSession::from_launch(&config))
            .insert_resource(Steam { backend })
            .add_observer(achievements::on_unlock)
            .add_observer(cloud::on_save_written)
            .add_systems(PreStartup, cloud::pull)
            .add_systems(First, session::pump)
            // A stand's tick runs no `First`: the callbacks run before the session reads the relay.
            .add_systems(FixedFirst, session::pump.before(NetSystems::Receive))
            .add_systems(Last, achievements::store_when_due)
            .add_exit_hook("steam: store the achievements, leave the lobby", session::on_exit)
            .add_ai_status("steam", tools::status)
            .add_dev_entry("steam", "steam", "Steam: who, lobby, relay", DevCommand::Run("steam".into()))
            .add_dev_entry("steam", "steam_lobbies", "Find lobbies", DevCommand::Run("steam_lobbies".into()))
            .add_dev_entry("steam", "steam_invite", "Invite a friend", DevCommand::Run("steam_invite".into()))
            .add_dev_entry("steam", "steam_join", "Enter a lobby...", DevCommand::Fill("steam_join lobby=".into()))
            .add_dev_entry("steam", "achievement_unlock", "Unlock an achievement...", DevCommand::Fill("achievement_unlock id=".into()));
        tools::register(app.world_mut());
    }
}

/// The Steam of this run: the real client (or a game server) when the game is played and built
/// with the SDK, none (and why) when it should be and cannot, else a fake of its own.
fn choose(world: &mut World, config: &EngineConfig) -> Box<dyn Backend> {
    if config.steam.connect {
        return connect(world, config);
    }
    let steam = FakeSteam::local(config.seed);
    let user = if config.steam.server {
        steam.server(fake::FIRST_SERVER, "server")
    } else {
        steam.user(fake::FIRST_USER, if config.player_name.is_empty() { "player" } else { config.player_name.as_str() })
    };
    user.set_relay_only(config.steam.relay_only);
    Box::new(user)
}

/// The Steam client, or a game server: started now.
#[cfg(feature = "sdk")]
fn start(config: &EngineConfig) -> Result<real::RealBackend, String> {
    let launch = &config.steam;
    if launch.server {
        let product = if config.title.is_empty() { "shards" } else { config.title.as_str() };
        real::RealBackend::init_server(launch.app_id, launch.relay_only, product)
    } else {
        real::RealBackend::init(launch.app_id, launch.relay_only)
    }
}

/// The real Steam: the one [`SteamClientPlugin`] started before the renderer, else started now.
#[cfg(feature = "sdk")]
fn connect(world: &mut World, config: &EngineConfig) -> Box<dyn Backend> {
    let started = world.remove_resource::<Started>().map_or_else(|| start(config), |s| s.0);
    match started {
        Ok(steam) => Box::new(steam),
        Err(reason) => Box::new(Unavailable { reason, app_id: config.steam.app_id }),
    }
}

#[cfg(not(feature = "sdk"))]
fn connect(_: &mut World, config: &EngineConfig) -> Box<dyn Backend> {
    Box::new(Unavailable { reason: NO_SDK.into(), app_id: config.steam.app_id })
}

/// Runs `app` on `user` of a fake Steam (a player, or a game server: [`FakeSteam::server`]), as in
/// a test or a stand: call it after adding the plugins and before the first update.
pub fn use_fake(app: &mut App, user: FakeUser) {
    let real_time = user.steam().real_time();
    if let Some(mut external) = app.world_mut().get_resource_mut::<ExternalTransport>() {
        external.real_time = real_time;
    }
    user.set_relay_only(app.world().get_resource::<EngineConfig>().is_some_and(|c| c.steam.relay_only));
    app.insert_resource(Steam { backend: Box::new(user) });
}
