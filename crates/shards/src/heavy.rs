//! Heavy mode's twins ([`ai_core::heavy`]): every few ticks a fresh game, built by the game's
//! own builder, loads a snapshot of the game and must continue like it. The builder is what
//! [`run`](crate::run) and [`testing::heavy_app`](crate::testing::heavy_app) are given; the
//! twin is that game as a test app beside it: headless, paused, no server, no files, nothing
//! played, its own save folder, its own heavy mode off.

use std::sync::Arc;
use std::time::Duration;

use ai_core::HeavyTwin;
use bevy::prelude::*;
use shards_core::{EngineConfig, MenuLaunch, NetLaunch, SettingsStore, SteamLaunch};

/// A game's builder, shared with the twins it builds.
pub type Builder = Arc<dyn Fn(&EngineConfig) -> App + Send + Sync>;

/// How long a twin may take to load (a big region generates as it loads).
const TWIN_LOAD: Duration = Duration::from_secs(120);

/// The config of a twin of the game `config` describes.
pub fn twin_config(config: &EngineConfig) -> EngineConfig {
    let defaults = EngineConfig::default();
    EngineConfig {
        headless: true,
        offscreen: false,
        port: None,
        paused: true,
        crash_replays: false,
        hot_reload: false,
        // A test app stays one; a game's twin keeps its log lines in its own journal, beside it.
        embedded: !config.testing,
        settings: SettingsStore::Off,
        // The snapshot puts the game in; a menu would only stand in the way.
        menu: MenuLaunch::Skip,
        // The twin plays the host's recording alone: what the clients did is in it.
        net: NetLaunch::Off,
        net_sim: None,
        exit_with_host: false,
        // Its own folder (`testing::own_saves`), and no slot to load: the snapshot is the world.
        save_dir: defaults.save_dir,
        character_slot: None,
        world_slot: None,
        steam: SteamLaunch { connect: false, host: false, server: false, ..config.steam.clone() },
        heavy: None,
        coverage: false,
        ..config.clone()
    }
}

/// Gives `app`, built with `config` by `build`, the builder of its twins, when heavy mode is on.
pub fn install(app: &mut App, config: &EngineConfig, build: Builder) {
    if config.heavy.is_none() {
        return;
    }
    let twin = twin_config(config);
    app.insert_resource(HeavyTwin::new(
        move || {
            let mut app = build(&twin);
            crate::testing::own_saves(&mut app, &twin);
            app.finish();
            app.cleanup();
            app
        },
        |app| crate::testing::wait_until_loaded(app, TWIN_LOAD),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_twin_is_a_headless_paused_game_alone_without_files_or_its_own_heavy_mode() {
        let config = EngineConfig {
            heavy: Some(ai_core::HeavyConfig::every(30)),
            net: NetLaunch::Stand { clients: 2, udp: false },
            port: Some(15702),
            world_slot: Some("w".into()),
            save_dir: "elsewhere".into(),
            seed: 7,
            ..EngineConfig::default()
        };
        let twin = twin_config(&config);
        assert_eq!((twin.headless, twin.paused, twin.port, twin.crash_replays), (true, true, None, false));
        assert_eq!((twin.heavy, twin.net, twin.world_slot), (None, NetLaunch::Off, None));
        assert_eq!(twin.save_dir, EngineConfig::default().save_dir, "its own folder, which own_saves gives it");
        assert!(twin.embedded && !twin.testing, "a game's twin keeps its lines in its own journal");
        assert_eq!(twin.seed, 7, "the same game");
        let test = twin_config(&EngineConfig { testing: true, ..config });
        assert!(test.testing && !test.embedded, "a test app's twin is a test app");
    }
}
