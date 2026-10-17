//! The main menu and the loading picture: a player meets the menu after loading (a window,
//! `--offscreen`, `--menu`), tests and replays go straight in; New game starts the same game a run
//! without the menu starts, Continue the newest save, or says why there is none.

use my_game::progress::Trophies;
use shards::ai_core::replay::{self, Replay};
use shards::ai_core::{AiId, AiIndex, AiReplay};
use shards::prelude::*;
use shards::save::{SaveConfig, SaveSlots};
use shards::serde_json::json;

use crate::{fails, fight, journal, say, state, step, SEED};

/// The game as a run that shows its main menu has it after loading: in the menu, the yard built,
/// no hero yet. The config is `build_app`'s (checked below), the saves the test's own.
fn at_the_menu() -> App {
    shards::testing::test_app(
        |config| {
            config.seed = SEED;
            config.after_loading = EngineState::MainMenu;
            config.menu = MenuLaunch::Show;
        },
        |app| {
            app.add_plugins(my_game::GamePlugin);
        },
    )
}

fn engine_state(app: &App) -> EngineState {
    *app.world().resource::<State<EngineState>>().get()
}

/// The line of widget `id` in what `ui_read` says.
fn line(tree: &str, id: &str) -> String {
    tree.lines().map(str::trim).find(|l| l.starts_with(&format!("{id}  "))).unwrap_or_else(|| panic!("no {id} in\n{tree}")).to_owned()
}

#[test]
fn a_player_meets_the_main_menu_and_tests_go_straight_in() {
    let app = my_game::build_app(&shards::testing::config());
    let config = app.world().resource::<EngineConfig>();
    assert_eq!(config.after_loading, EngineState::MainMenu, "the game has a main menu");
    assert_eq!(config.loading_image.as_deref(), Some("ui/loading.png"));
    assert_eq!(config.start_state(), EngineState::InGame, "a headless test goes straight in");
    let windowed = EngineConfig { headless: false, testing: false, ..config.clone() };
    assert_eq!(windowed.start_state(), EngineState::MainMenu, "a window shows the menu");
    assert_eq!(EngineConfig { menu: MenuLaunch::Skip, ..windowed }.start_state(), EngineState::InGame, "--no-menu");

    let mut straight = my_game::test_app(SEED);
    assert_eq!(engine_state(&straight), EngineState::InGame);
    assert!(say(&mut straight, "ui_read", json!({})).contains("hud/health"), "the HUD is up, no menu");
}

#[test]
fn the_loading_picture_is_a_small_16_by_9_png() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ui/loading.png");
    let bytes = std::fs::read(path).expect("assets/ui/loading.png: `cargo run -p sandbox --example make_art` in the engine draws it");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "a PNG");
    let number = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    let (width, height) = (number(16), number(20));
    assert_eq!(width * 9, height * 16, "16:9, not {width}×{height}");
    assert!(bytes.len() < 200 * 1024, "{} KB: keep the repository light", bytes.len() / 1024);
}

#[test]
fn new_game_from_the_main_menu_starts_the_game_a_run_without_it_starts() {
    let mut app = at_the_menu();
    assert_eq!(engine_state(&app), EngineState::MainMenu);
    let players = app.world_mut().query_filtered::<Entity, With<AiPlayer>>().iter(app.world()).count();
    assert_eq!(players, 0, "no hero before the game starts");
    let tree = say(&mut app, "ui_read", json!({}));
    assert_eq!(line(&tree, "main_menu/new_game"), "main_menu/new_game  button \"New game\" → tool new_game ◀ focus");
    assert!(line(&tree, "main_menu/no_save").contains("\"No saved game yet: start a new one\""), "{tree}");
    assert!(line(&tree, "main_menu/art").starts_with("main_menu/art  image \"A lone tower on a hill under a dusk sky\" ui/loading.png"), "{tree}");
    assert!(!tree.contains("hud/"), "the HUD waits for the game");

    // In Russian too, switched in the menu.
    say(&mut app, "settings_set", json!({"group": "locale", "key": "language", "value": "ru"}));
    let tree = say(&mut app, "ui_read", json!({}));
    assert!(line(&tree, "main_menu/new_game").contains("\"Новая игра\""), "{tree}");
    assert!(line(&tree, "main_menu/no_save").contains("\"Сохранений пока нет: начните новую игру\""), "{tree}");
    say(&mut app, "settings_set", json!({"group": "locale", "key": "language", "value": "en"}));

    let pressed = say(&mut app, "ui_click", json!({"id": "main_menu/new_game"}));
    assert!(pressed.contains("the game starts at tick 0"), "{pressed}");
    assert_eq!(engine_state(&app), EngineState::InGame);
    let tree = say(&mut app, "ui_read", json!({}));
    assert!(tree.contains("hud/health") && !tree.contains("main_menu/"), "{tree}");

    // The same world, ids and all, as straight in; and it plays the same.
    let mut straight = my_game::test_app(SEED);
    assert_eq!(state(&mut app), state(&mut straight));
    fight(&mut app);
    fight(&mut straight);
    assert_eq!(state(&mut app), state(&mut straight));
}

#[test]
fn a_session_begun_in_the_main_menu_replays_straight_in() {
    let mut original = at_the_menu();
    say(&mut original, "ui_click", json!({"id": "main_menu/new_game"}));
    fight(&mut original);
    let recording: Replay = shards::serde_json::from_str(&shards::serde_json::to_string(&replay::save(original.world())).unwrap()).unwrap();

    let mut copy = my_game::test_app(recording.seed);
    let end = recording.end.tick;
    replay::play(copy.world_mut(), recording).unwrap();
    step(&mut copy, end);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().expect("the playback reached its end");
    assert!(report.identical(), "{report}");
    assert_eq!(state(&mut original), state(&mut copy));
}

#[test]
fn continue_without_a_save_says_why_and_stays_in_the_menu() {
    let mut app = at_the_menu();
    let err = fails(&mut app, "ui_click", json!({"id": "main_menu/continue"}));
    assert!(err.contains("calls continue_game: there is no save to continue yet"), "{err}");
    assert_eq!(engine_state(&app), EngineState::MainMenu);
    let err = fails(&mut app, "continue_game", json!({}));
    assert!(err.contains("`new_game` starts a game"), "{err}");
}

#[test]
fn continue_plays_on_from_the_newest_save() {
    // A game played and saved, into the folder the menu's game then reads.
    let mut first = my_game::test_app(SEED);
    fight(&mut first);
    say(&mut first, "save_game", json!({"slot": "camp"}));
    let dir = first.world().resource::<SaveConfig>().dir.clone();

    let mut app = at_the_menu();
    app.world_mut().resource_mut::<SaveConfig>().dir = dir;
    let tree = say(&mut app, "ui_read", json!({}));
    assert!(line(&tree, "main_menu/no_save").contains("hidden"), "there is a save now: {tree}");

    let pressed = say(&mut app, "ui_click", json!({"id": "main_menu/continue"}));
    assert!(pressed.contains("loaded world/camp, character/camp"), "{pressed}");
    assert_eq!(engine_state(&app), EngineState::InGame);
    let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
    assert_eq!(app.world().get::<Trophies>(hero).unwrap().defeated, 1, "the hero's trophy came back");
    let slots = app.world().resource::<SaveSlots>();
    assert_eq!((slots.world.as_str(), slots.character.as_str()), ("camp", "camp"), "the game saves on into the slot it continued");
    assert!(journal(&app).iter().any(|l| l.contains("save: world/camp")), "{:?}", journal(&app));
}
