//! The HUD and menus: what is the game's own in them (src/screens.rs) works.

use my_game::progress::Trophies;
use shards::ai_core::{call_tool, configure_session, AiIndex, SessionChange};
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
    assert_eq!(line(&hud, "hud/trophies"), "hud/trophies  text \"0 trophies\" ← trophies");
    fight(&mut app);
    let hero = app.world().resource::<AiIndex>().get(shards::ai_core::AiId(1)).unwrap();
    let defeated = app.world().get::<Trophies>(hero).unwrap().defeated;
    assert_eq!(defeated, 1, "the fight defeats the grunt");
    let hud = say(&mut app, "ui_read", json!({"screen": "hud"}));
    assert_eq!(line(&hud, "hud/trophies"), "hud/trophies  text \"1 trophy\" ← trophies");
}

#[test]
fn the_hud_and_menus_switch_language_while_the_game_runs() {
    let mut app = my_game::test_app(SEED);
    say(&mut app, "ui_open", json!({"screen": "pause"}));
    say(&mut app, "settings_set", json!({"group": "locale", "key": "language", "value": "ru"}));
    let tree = say(&mut app, "ui_read", json!({}));
    assert_eq!(line(&tree, "hud/trophies"), "hud/trophies  text \"0 трофеев\" ← trophies", "the number picks Russian's plural form");
    assert_eq!(line(&tree, "hud/hint"), "hud/hint  hint \"[Esc] Меню\"");
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Продолжить\" → close ◀ focus");
    say(&mut app, "ui_close", json!({"screen": "pause"}));
    fight(&mut app);
    let hud = say(&mut app, "ui_read", json!({"screen": "hud"}));
    assert_eq!(line(&hud, "hud/trophies"), "hud/trophies  text \"1 трофей\" ← trophies");
    let err = fails(&mut app, "settings_set", json!({"group": "locale", "key": "language", "value": "de"}));
    assert!(err.contains("`de` is not a language of this game: en (English), ru (Русский)"), "{err}");
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
fn a_player_scope_agent_saves_its_hero_and_quits() {
    let mut app = my_game::test_app(SEED);
    let player = SessionChange { scope: Some(Scope::Player), ..default() };
    configure_session(app.world_mut(), "playtester", &player).unwrap();
    for (tool, args) in [("ui_open", json!({"screen": "pause"})), ("ui_click", json!({"id": "pause/quit"}))] {
        let out = call_tool(app.world_mut(), "playtester", tool, args);
        assert!(out.ok, "{tool}: {}", out.result);
    }
    let out = call_tool(app.world_mut(), "playtester", "ui_click", json!({"id": "confirm_quit/yes"}));
    let done = out.result.as_str().unwrap_or_default().to_owned();
    assert!(out.ok && done.ends_with("; the game ends"), "{done}");
    assert!(done.contains("character/slot1") && !done.contains("world/"), "only the hero is saved: {done}");
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
