//! The HUD and menus: what is the game's own in them (src/screens.rs) works.

use my_game::progress::Trophies;
use shards::ai_core::AiIndex;
use shards::bevy::ecs::message::Messages;
use shards::prelude::*;
use shards::serde_json::json;

use crate::{fails, fight, say, SEED};

/// The line of widget `id` in what `ui_read` says.
fn line(tree: &str, id: &str) -> String {
    tree.lines().map(str::trim).find(|l| l.starts_with(&format!("{id}  "))).unwrap_or_else(|| panic!("no {id} in\n{tree}")).to_owned()
}

#[test]
fn the_hud_shows_the_trophies_of_the_hero() {
    let mut app = my_game::test_app(SEED);
    let hud = say(&mut app, "ui_read", json!({"screen": "hud"}));
    assert_eq!(line(&hud, "hud/trophies"), "hud/trophies  text \"Trophies 0\" ← trophies");
    fight(&mut app);
    let hero = app.world().resource::<AiIndex>().get(shards::ai_core::AiId(1)).unwrap();
    let defeated = app.world().get::<Trophies>(hero).unwrap().defeated;
    assert_eq!(defeated, 1, "the fight defeats the grunt");
    let hud = say(&mut app, "ui_read", json!({"screen": "hud"}));
    assert_eq!(line(&hud, "hud/trophies"), "hud/trophies  text \"Trophies 1\" ← trophies");
}

#[test]
fn save_and_quit_saves_first_and_then_ends_the_game() {
    let mut app = my_game::test_app(SEED);
    say(&mut app, "ui_open", json!({"screen": "pause"}));
    say(&mut app, "ui_click", json!({"id": "pause/quit"}));
    let done = say(&mut app, "ui_click", json!({"id": "confirm_quit/yes"}));
    assert!(done.starts_with("pressed confirm_quit/yes: ") && done.ends_with("; the game ends"), "{done}");
    assert!(done.contains("world/slot1"), "{done}");
    let exits = app.world().resource::<Messages<AppExit>>();
    assert_eq!(exits.iter_current_update_messages().count(), 1, "the game is asked to end");
}

#[test]
fn a_menu_button_is_refused_while_a_question_is_open() {
    let mut app = my_game::test_app(SEED);
    say(&mut app, "ui_open", json!({"screen": "pause"}));
    say(&mut app, "ui_click", json!({"id": "pause/quit"}));
    let err = fails(&mut app, "ui_click", json!({"id": "pause/resume"}));
    assert_eq!(err, "pause/resume is under confirm_quit: answer or close that first");
}
