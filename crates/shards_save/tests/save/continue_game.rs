//! A main menu's Continue: `continue_game` starts the game and loads the newest save of each kind
//! into it; with no save it refuses and the game stays in its menu.

use std::fs;
use std::time::{Duration, SystemTime};

use serde_json::json;
use shards::save::{any_save, newest_slot, SaveConfig, SaveKind, SaveSlots};
use shards::testing;

use shards::prelude::*;

use crate::common::*;

/// The test game in a run that shows a main menu, saving into `dir`: after loading it is in the
/// menu, nothing spawned yet.
fn at_the_menu(dir: &TempDir) -> App {
    testing::test_app(
        |config| {
            config.asset_dir = ASSETS.into();
            config.after_loading = EngineState::MainMenu;
            config.menu = MenuLaunch::Show;
        },
        |app| {
            app.init_resource::<Calendar>().add_systems(OnEnter(EngineState::InGame), spawn_world);
            app.world_mut().resource_mut::<SaveConfig>().dir = dir.path().to_owned();
            register_saves(app);
        },
    )
}

fn state(app: &App) -> EngineState {
    *app.world().resource::<State<EngineState>>().get()
}

/// Saves a hero called `name` on day `day` into `slot`, both kinds.
fn save(dir: &TempDir, slot: &str, name: &str, day: u32) {
    let mut app = game(dir);
    set_hero(&mut app, Hero { name: name.into(), level: 3, ..Hero::default() });
    app.world_mut().resource_mut::<Calendar>().day = day;
    ok(&mut app, "save_game", json!({"what": "all", "slot": slot}));
}

/// Makes `slot`'s files an hour older, so which slot is newest does not hang on the clock's grain.
fn age(dir: &TempDir, slot: &str) {
    let then = SystemTime::now() - Duration::from_secs(3600);
    for kind in ["world", "character"] {
        let file = fs::File::options().write(true).open(dir.file(&format!("{kind}/{slot}.save"))).unwrap();
        file.set_modified(then).unwrap();
    }
}

#[test]
fn continue_loads_the_newest_save_of_each_kind_and_saves_on_into_it() {
    let dir = TempDir::new("continue");
    save(&dir, "camp", "Ada", 40);
    save(&dir, "ford", "Bo", 41);
    age(&dir, "camp");

    let mut app = at_the_menu(&dir);
    assert_eq!(state(&app), EngineState::MainMenu);
    assert!(any_save(app.world()));
    assert_eq!(newest_slot(app.world(), SaveKind::World).as_deref(), Some("ford"));
    let done = ok(&mut app, "continue_game", json!({}));
    assert_eq!(state(&app), EngineState::InGame, "{done}");
    assert!(done.contains("the game starts at tick 0; loaded world/ford, character/ford"), "{done}");
    assert_eq!((hero(&mut app).name, calendar(&app).day), ("Bo".to_owned(), 41));
    let slots = app.world().resource::<SaveSlots>();
    assert_eq!((slots.world.as_str(), slots.character.as_str()), ("ford", "ford"));
}

#[test]
fn a_slot_that_does_not_read_is_passed_over() {
    let dir = TempDir::new("continue_damaged");
    save(&dir, "camp", "Ada", 40);
    age(&dir, "camp");
    save(&dir, "ford", "Bo", 41);
    fs::write(dir.file("world/ford.save"), "not a save").unwrap();

    let mut app = at_the_menu(&dir);
    let done = ok(&mut app, "continue_game", json!({}));
    assert!(done.contains("loaded world/camp, character/ford"), "each kind its newest that reads: {done}");
    assert_eq!((hero(&mut app).name, calendar(&app).day), ("Bo".to_owned(), 40));
}

#[test]
fn with_no_save_continue_says_why_and_the_menu_stays() {
    let dir = TempDir::new("continue_none");
    let mut app = at_the_menu(&dir);
    assert!(!any_save(app.world()));
    let err = fails(&mut app, "continue_game", json!({}));
    assert!(err.starts_with("there is no save to continue yet"), "{err}");
    assert_eq!(state(&app), EngineState::MainMenu);

    // In the game there is no menu to leave.
    ok(&mut app, "new_game", json!({}));
    save(&dir, "camp", "Ada", 40);
    let err = fails(&mut app, "continue_game", json!({}));
    assert!(err.contains("the game is on already"), "{err}");
}
