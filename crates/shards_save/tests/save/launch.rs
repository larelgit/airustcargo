//! What the launch says about saves: `--save-dir`, `--world SLOT`, `--character SLOT`.

use std::fs;

use serde_json::json;
use shards::ai_core::{replay, sim, JournalKind};
use shards::core::NetLaunch;
use shards::save::{load_launch_slots, LaunchOutcome, SaveConfig, SaveFile, SaveSlots};
use shards::testing;

use shards::prelude::*;

use crate::common::*;

/// The test game started with `--world` / `--character` as given, saving into `dir`.
fn launched(dir: &TempDir, world: Option<&str>, character: Option<&str>) -> App {
    launched_with(dir, |config| {
        config.world_slot = world.map(str::to_owned);
        config.character_slot = character.map(str::to_owned);
    })
}

fn launched_with(dir: &TempDir, configure: impl FnOnce(&mut shards::EngineConfig)) -> App {
    testing::test_app(
        |config| {
            config.asset_dir = ASSETS.into();
            configure(config);
        },
        |app| {
            app.init_resource::<Calendar>().add_systems(OnEnter(EngineState::InGame), spawn_world);
            app.world_mut().resource_mut::<SaveConfig>().dir = dir.path().to_owned();
            register_saves(app);
        },
    )
}

/// A save of "Ada, level 7, day 40" in slot `camp`.
fn save_ada(dir: &TempDir) {
    let mut app = game(dir);
    set_hero(&mut app, Hero { name: "Ada".into(), level: 7, xp: 900, coins: Coins { gold: 5, silver: 2 }, ..Hero::default() });
    app.world_mut().resource_mut::<Calendar>().day = 40;
    set_chests(&mut app, &[Chest { open: true, loot: vec![1, 2] }]);
    ok(&mut app, "save_game", json!({"what": "all", "slot": "camp"}));
}

fn slots(app: &App) -> (String, String) {
    let s = app.world().resource::<SaveSlots>();
    (s.world.clone(), s.character.clone())
}

#[test]
fn the_named_slots_are_loaded_before_the_first_tick_and_the_recording_starts_from_them() {
    let dir = TempDir::new("launch");
    save_ada(&dir);
    let mut app = launched(&dir, Some("camp"), Some("camp"));
    assert_eq!(sim::tick(app.world()), 0, "loaded before the first tick");
    assert_eq!((hero(&mut app).name, calendar(&app).day), ("Ada".to_owned(), 40));
    assert_eq!(chests(&mut app)[0].1.loot, vec![1, 2]);
    assert_eq!(slots(&app), ("camp".to_owned(), "camp".to_owned()), "saving goes where the launch said");

    // It is a `load_game` call like any other: an agent's line in the journal, then the report.
    let calls = lines(&app, JournalKind::Agent, "launch: load_game");
    assert_eq!(calls.len(), 2, "{:?}", journal(&app));
    assert!(calls[0].contains("\"what\":\"world\"") && calls[1].contains("\"what\":\"character\"") && calls[1].contains("\"slot\":\"camp\""), "{calls:?}");
    assert_eq!(lines(&app, JournalKind::Sim, "save: world/camp loaded").len(), 1);
    assert!(lines(&app, JournalKind::Warn, "").is_empty() && lines(&app, JournalKind::Error, "").is_empty(), "{:?}", journal(&app));

    // A replay of this session starts from the loaded state, so a game without the flags plays it.
    step(&mut app, 5);
    let recording = replay::save(app.world());
    assert!(recording.start.is_some(), "the recording starts from the snapshot taken after the load");
    let mut plain = game(&TempDir::new("launch_plain"));
    let report = testing::verify(&mut plain, recording).unwrap();
    assert!(report.identical(), "{report}");
}

#[test]
fn one_flag_loads_one_kind() {
    let dir = TempDir::new("launch_one");
    save_ada(&dir);
    let mut app = launched(&dir, None, Some("camp"));
    assert_eq!((hero(&mut app).name, calendar(&app).day), ("Ada".to_owned(), 0), "only the character");
    assert_eq!(slots(&app), ("slot1".to_owned(), "camp".to_owned()));
    let mut app = launched(&dir, Some("camp"), None);
    assert_eq!((hero(&mut app).name, calendar(&app).day), (String::new(), 40), "only the world");
    assert_eq!(slots(&app), ("camp".to_owned(), "slot1".to_owned()));
    let mut app = launched(&dir, None, None);
    assert_eq!((hero(&mut app).name, calendar(&app).day), (String::new(), 0), "no flag, no load");
    assert!(lines(&app, JournalKind::Agent, "launch").is_empty());
}

#[test]
fn a_slot_with_no_save_is_a_new_game_not_an_error() {
    let dir = TempDir::new("launch_new");
    let mut app = launched(&dir, Some("fresh"), Some("fresh"));
    assert_eq!((hero(&mut app).name, calendar(&app).day), (String::new(), 0));
    assert_eq!(slots(&app), ("fresh".to_owned(), "fresh".to_owned()));
    assert_eq!(lines(&app, JournalKind::Sim, "save: --world fresh: no world save in that slot yet, starting a new game").len(), 1, "{:?}", journal(&app));
    assert_eq!(lines(&app, JournalKind::Sim, "save: --character fresh: no character save in that slot yet").len(), 1);
    assert!(lines(&app, JournalKind::Warn, "").is_empty() && lines(&app, JournalKind::Error, "").is_empty(), "{:?}", journal(&app));
    assert!(lines(&app, JournalKind::Agent, "launch").is_empty(), "no load was called");
    ok(&mut app, "save_game", json!({"what": "all"}));
    assert!(dir.file("world/fresh.save").exists() && dir.file("character/fresh.save").exists(), "the first save creates the slot");
}

#[test]
fn a_slot_that_cannot_be_loaded_is_an_error_and_is_left_alone() {
    let dir = TempDir::new("launch_bad");
    save_ada(&dir);
    // Every copy of `world/bad` is damaged; `character/newer` is from a newer game.
    fs::create_dir_all(dir.file("world")).unwrap();
    fs::write(dir.file("world/bad.save"), b"junk").unwrap();
    let ada = fs::read(dir.file("character/camp.save")).unwrap();
    let mut file = SaveFile::decode(&ada).unwrap();
    file.sections.0.get_mut("hero").unwrap().version = 9;
    fs::write(dir.file("character/newer.save"), file.encode()).unwrap();
    let before = (fs::read(dir.file("world/bad.save")).unwrap(), fs::read(dir.file("character/newer.save")).unwrap());

    let mut app = launched(&dir, Some("bad"), Some("newer"));
    let errors = lines(&app, JournalKind::Error, "save: --");
    assert_eq!(errors.len(), 2, "{:?}", journal(&app));
    assert!(errors[0].starts_with("save: --world bad was not loaded, starting without it: cannot load world/bad, every copy is unusable"), "{}", errors[0]);
    assert!(
        errors[1].starts_with("save: --character newer was not loaded, starting without it: character/newer was saved by a newer version of the game (section hero v9 > v1)"),
        "{}",
        errors[1]
    );
    assert_eq!((hero(&mut app).name, calendar(&app).day), (String::new(), 0), "the game starts as it is");
    assert_eq!(before, (fs::read(dir.file("world/bad.save")).unwrap(), fs::read(dir.file("character/newer.save")).unwrap()), "and what could not be read is untouched");
    assert!(!dir.file("world/bad.save.damaged").exists());
    // Saving into the unreadable slot is refused for the newer one (never overwritten) ...
    let err = fails(&mut app, "save_game", json!({"what": "character"}));
    assert!(err.contains("so this build does not overwrite it"), "{err}");
    // ... and sets the damaged one aside, as for any damaged primary.
    ok(&mut app, "save_game", json!({"what": "world"}));
    assert!(dir.file("world/bad.save.damaged").exists());
}

#[test]
fn a_replay_that_plays_loads_nothing_at_launch() {
    let dir = TempDir::new("launch_replay");
    save_ada(&dir);
    let mut plain = game(&TempDir::new("launch_replay_a"));
    step(&mut plain, 10);
    let recording = replay::save(plain.world());

    // The flags are on, but the recording is what is being played: its start is a fresh world.
    let mut app = App::new();
    app.add_plugins(shards::EnginePlugins::new(shards::EngineConfig {
        asset_dir: ASSETS.into(),
        world_slot: Some("camp".into()),
        character_slot: Some("camp".into()),
        ..testing::config()
    }));
    app.init_resource::<Calendar>().add_systems(OnEnter(EngineState::InGame), spawn_world);
    register_saves(&mut app);
    app.world_mut().resource_mut::<SaveConfig>().dir = dir.path().to_owned();
    app.finish();
    app.cleanup();
    let report = testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "{report}");
    assert_eq!(hero(&mut app).name, "", "Ada was not loaded");
    assert!(lines(&app, JournalKind::Agent, "launch").is_empty(), "{:?}", journal(&app));
}

#[test]
fn a_host_or_client_is_left_to_the_network_code_which_has_a_hook() {
    let dir = TempDir::new("launch_net");
    save_ada(&dir);
    let camp = |config: &mut shards::EngineConfig| {
        config.world_slot = Some("camp".into());
        config.character_slot = Some("camp".into());
    };
    // A host (on a free port: test runs go side by side) plays a world of its own.
    let mut app = launched_with(&dir, |config| {
        config.net = NetLaunch::Host { port: 0 };
        camp(config);
    });
    assert_eq!((hero(&mut app).name, calendar(&app).day), (String::new(), 0), "a host: nothing loads by itself");
    assert_eq!(slots(&app), ("camp".to_owned(), "camp".to_owned()));
    let done = load_launch_slots(app.world_mut());
    assert_eq!(done.iter().map(|l| (l.kind.as_str(), l.slot.as_str(), l.outcome.clone())).collect::<Vec<_>>(), [
        ("world", "camp", LaunchOutcome::Loaded),
        ("character", "camp", LaunchOutcome::Loaded)
    ]);
    assert_eq!((hero(&mut app).name, calendar(&app).day), ("Ada".to_owned(), 40));

    // A client's world is its host's: it waits for it (none answers here), and nothing of
    // the slots loads by itself meanwhile.
    let mut client = waiting(&dir, |config| {
        config.net = NetLaunch::Join { addr: "127.0.0.1:9".into() };
        camp(config);
    });
    for _ in 0..5 {
        client.update();
    }
    assert!(matches!(client.world().resource::<shards::net::NetState>(), shards::net::NetState::Joining(_)));
    assert_eq!(calendar(&client).day, 0, "a client: nothing loads by itself");
    assert_eq!(slots(&client), ("camp".to_owned(), "camp".to_owned()));
}

/// The test game as [`launched_with`] builds it, without waiting for it to load (a client
/// loads when its host's world arrives).
fn waiting(dir: &TempDir, configure: impl FnOnce(&mut shards::EngineConfig)) -> App {
    let mut config = testing::config();
    config.asset_dir = ASSETS.into();
    configure(&mut config);
    let mut app = App::new();
    app.add_plugins(shards::EnginePlugins::new(config));
    app.init_resource::<Calendar>().add_systems(OnEnter(EngineState::InGame), spawn_world);
    app.world_mut().resource_mut::<SaveConfig>().dir = dir.path().to_owned();
    register_saves(&mut app);
    app.finish();
    app.cleanup();
    app
}

#[test]
fn the_save_folder_comes_from_the_launch() {
    let app = testing::test_app(|config| config.save_dir = "saves/qa-run".into(), |_| {});
    assert_eq!(app.world().resource::<SaveConfig>().dir, std::path::PathBuf::from("saves/qa-run"));
    let field = shards::ai_core::status::status(app.world());
    assert_eq!(field["save"]["dir"], "saves/qa-run");
}

#[test]
#[should_panic(expected = "EngineConfig.save_dir: the save folder `/var/saves` is absolute")]
fn an_absolute_save_folder_is_refused() {
    drop(testing::test_app(|config| config.save_dir = "/var/saves".into(), |_| {}));
}

#[test]
#[should_panic(expected = "EngineConfig.save_dir: the save folder `../elsewhere` goes up with `..`")]
fn a_save_folder_outside_the_game_folder_is_refused() {
    drop(testing::test_app(|config| config.save_dir = "../elsewhere".into(), |_| {}));
}

#[test]
#[should_panic(expected = "EngineConfig.world_slot / character_slot: slot name `../x` is not allowed")]
fn a_slot_that_is_a_path_is_refused() {
    drop(testing::test_app(|config| config.world_slot = Some("../x".into()), |_| {}));
}
