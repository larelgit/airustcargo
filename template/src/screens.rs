//! The HUD and menus. What they hold is content (`assets/content/screens.screen.ron`); the engine
//! draws them, moves through them with the keyboard, gamepad and mouse, and lets agents read and
//! press them (`ui_read`, `ui_navigate`, `ui_click`). A game adds only what is its own:
//!
//! - a **source** a widget binds to (`bind: "trophies"`, `"continue_note"`), read from the world
//!   when it is shown, and said in the language played: a message of
//!   `assets/locale/<language>/game.ftl`, with a number as a parameter, so each language picks its
//!   own plural form;
//! - a **game action** a button does (`action: "save_and_quit"`), when `close`, `open <screen>`,
//!   `quit` and `tool <name> {json}` are not enough.

use shards::ai_core::{call_tool, AgentSessions};
use shards::prelude::*;
use shards::serde_json::json;
use shards::ui::viewer;

use crate::progress::Trophies;

pub struct ScreensPlugin;

impl Plugin for ScreensPlugin {
    fn build(&self, app: &mut App) {
        app.add_ui_source("trophies", trophies)
            .add_ui_source("continue_note", continue_note)
            .add_ui_action("save_and_quit", save_and_quit);
    }
}

/// `2 trophies` (`2 трофея`): the grunts the hero this machine plays has defeated.
fn trophies(world: &World, _: &str) -> Option<UiValue> {
    let defeated = world.get::<Trophies>(viewer(world)?)?.defeated;
    Some(UiValue::Text(shards::locale::text(world, "hud-trophies", &Args::new().number("count", defeated))))
}

/// Under the main menu's Continue: that there is no save to continue yet, and nothing (the hint
/// hides) once there is one. The press itself is the engine's `continue_game`, which loads the
/// newest save.
fn continue_note(world: &World, _: &str) -> Option<UiValue> {
    let note = if shards::save::any_save(world) { String::new() } else { shards::locale::text(world, "menu-no-save", &Args::new()) };
    Some(UiValue::Text(note))
}

/// Saves the world and the hero, then ends the game; a save that fails keeps the game going,
/// so nothing is lost. The save is the player's own `save_game`, called as whoever pressed: an
/// agent with player scope may save only its hero (the world is the host's), so its press
/// saves that. A co-op guest saves nothing here: leaving, it gets its hero from the host.
fn save_and_quit(world: &mut World, press: &UiPress) -> Result<String, String> {
    if world.get_resource::<NetRole>().is_some_and(|role| !role.simulates()) {
        world.write_message(AppExit::Success);
        return Ok("the host saves your hero as you leave; the game ends".to_owned());
    }
    let sessions = world.resource::<AgentSessions>();
    let scope = sessions.get(&press.agent).map_or(sessions.default_scope, |s| s.scope);
    let what = if scope.allows(Scope::Dev) { "all" } else { "character" };
    let saved = call_tool(world, &press.agent, "save_game", json!({"what": what}));
    let text = saved.result.as_str().map_or_else(|| saved.result.to_string(), str::to_owned);
    if !saved.ok {
        return Err(format!("not saved, so the game goes on: {text}"));
    }
    world.write_message(AppExit::Success);
    Ok(format!("{text}; the game ends"))
}
