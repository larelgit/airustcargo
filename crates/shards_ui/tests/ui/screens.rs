//! Screens: the HUD at the start, a menu that pauses, what agents read, hot edits, replays.

use serde_json::json;
use shards::ai_core::{replay, sim};
use shards::content::ContentDb;
use shards::prelude::*;
use shards::testing;
use shards::ui::ScreenDef;

use crate::common::*;

#[test]
fn the_hud_opens_at_the_start_shows_live_values_and_stays_out_of_observe() {
    let mut app = game();
    let tree = read(&mut app);
    assert!(tree.starts_with("SCREENS  hud\nhud (hud)"), "{tree}");
    assert_eq!(line(&tree, "hud/health"), "hud/health  bar \"HP\" 100/100 ← you.health");
    assert_eq!(line(&tree, "hud/hint"), "hud/hint  hint \"[Esc] Pause\"");
    assert_eq!(line(&tree, "hud/compass"), "hud/compass  compass 0° N ← you.heading", "the hero looks north");
    assert_eq!(line(&tree, "hud/allies"), "hud/allies  allies (nobody else) ← allies");

    // Values are read live: no frame has to pass after the change.
    hurt_to(&mut app, 40.0);
    assert_eq!(line(&read(&mut app), "hud/health"), "hud/health  bar \"HP\" 40/100 ← you.health");

    // Only the HUD: nothing in observe, nothing in the journal, no screen takes input.
    assert!(!observe(&mut app).contains("\nUI  "), "{}", observe(&mut app));
    assert!(journal(&app, "ui: ").is_empty(), "{:?}", journal(&app, "ui: "));
    let status = shards::ai_core::status::status(app.world());
    assert_eq!(status["ui"], json!({"screens": ["hud"], "input": null, "focus": null, "flash": null}));
}

#[test]
fn a_pausing_menu_pauses_single_player_and_closing_it_resumes() {
    let mut app = game();
    // Test games start paused; the player's game runs.
    testing::ok(&mut app, "resume", json!({}));
    app.update();
    assert!(!sim::is_paused(app.world()));

    let opened = ok(&mut app, "ui_open", json!({"screen": "pause"}));
    assert_eq!(opened, "opened pause (menu, pauses the game) · focus pause/resume");
    app.update();
    assert_eq!(*app.world().resource::<State<PauseState>>().get(), PauseState::Paused);
    assert!(sim::is_paused(app.world()), "single player: the menu stops the game");

    let seen = observe(&mut app);
    assert!(seen.contains("\nUI  pause \"Paused\" (menu, pauses the game) · focus pause/resume · ui_navigate, ui_click, ui_set\n"), "{seen}");
    assert!(seen.contains("\n  pause/resume  button \"Resume\" → close ◀ focus\n"), "{seen}");
    assert!(seen.contains("\n  pause/quit  button \"Quit\" → open confirm_quit\n"), "{seen}");
    assert_eq!(ok(&mut app, "ui_open", json!({"screen": "pause"})), "pause is open already · focus pause/resume");

    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "back"})), "closed pause; no menu is open");
    app.update();
    assert_eq!(*app.world().resource::<State<PauseState>>().get(), PauseState::Running);
    assert!(!sim::is_paused(app.world()), "closing the menu runs the game again");
    assert_eq!(journal(&app, "ui: "), ["ui: opened pause", "ui: closed pause"]);
}

#[test]
fn an_agents_resume_closes_the_menu_that_paused_the_game() {
    let mut app = game();
    testing::ok(&mut app, "resume", json!({}));
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    app.update();
    assert!(sim::is_paused(app.world()));
    testing::ok(&mut app, "resume", json!({}));
    app.update();
    app.update();
    assert_eq!(*app.world().resource::<State<PauseState>>().get(), PauseState::Running);
    assert!(!app.world().resource::<UiStack>().is_open("pause"), "the menu went with the pause");
}

#[test]
fn a_game_started_paused_stays_paused_when_its_menu_closes() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    app.update();
    ok(&mut app, "ui_close", json!({}));
    app.update();
    assert!(sim::is_paused(app.world()), "the menu did not pause it, so closing it does not resume it");
}

#[test]
fn an_edited_screen_is_built_again_and_keeps_its_focus() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    ok(&mut app, "ui_navigate", json!({"dir": "down"}));
    let before = serde_json::to_value(app.world().resource::<ContentDb>().get::<ScreenDef>("pause").unwrap()).unwrap();
    let mut after = before.clone();
    after["widgets"][1]["Button"]["label"] = json!("Options");
    let change = json!({"changes": [{"kind": "screen", "id": "pause", "file": "content/ui/menus.screen.ron", "def": after, "before": before}]});
    testing::ok(&mut app, "content_apply", change);
    app.update();
    let tree = read(&mut app);
    assert_eq!(line(&tree, "pause/settings"), "pause/settings  button \"Options\" → open settings ◀ focus", "{tree}");
}

#[test]
fn a_replay_presses_the_same_buttons() {
    let mut app = game();
    hurt_to(&mut app, 30.0);
    // The button calls a dev tool, so the dev presses it.
    let d = |app: &mut App, tool: &str, args| {
        let out = dev(app, tool, args);
        assert!(out.ok, "{tool}: {}", out.result);
        text(&out)
    };
    d(&mut app, "ui_open", json!({"screen": "pause"}));
    let pressed = d(&mut app, "ui_click", json!({"id": "pause/patch_up"}));
    assert!(pressed.starts_with("pressed pause/patch_up: heal: "), "{pressed}");
    assert_eq!(hero_health(&mut app), 100.0, "the button healed through the tool");
    testing::ok(&mut app, "step", json!({"ticks": 30}));
    d(&mut app, "ui_navigate", json!({"dir": "back"}));
    testing::ok(&mut app, "step", json!({"ticks": 30}));

    let recording = replay::save(app.world());
    let calls: Vec<&str> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            replay::ReplayEvent::Call { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["set", "ui_open", "ui_click", "ui_navigate"], "the heal inside the button is not recorded: the click makes it again");
    let mut fresh = game();
    let report = testing::verify(&mut fresh, recording).unwrap();
    assert_eq!(report.diverged_at, None, "{report:?}");
    assert_eq!(report.interrupted, None, "{report:?}");
    assert_eq!(hero_health(&mut fresh), 100.0);
}

#[test]
fn a_screen_that_opens_no_screen_is_refused_and_the_old_one_kept() {
    let mut app = game();
    let before = serde_json::to_value(app.world().resource::<ContentDb>().get::<ScreenDef>("pause").unwrap()).unwrap();
    let mut after = before.clone();
    after["widgets"][1]["Button"]["action"] = json!("open setings");
    let change = json!({"changes": [{"kind": "screen", "id": "pause", "file": "content/ui/menus.screen.ron", "def": after, "before": before}]});
    let out = dev(&mut app, "content_apply", change);
    assert!(!out.ok, "{}", out.result);
    let err = text(&out);
    assert!(err.contains("setings") && err.contains("menus.screen.ron"), "{err}");
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    assert_eq!(line(&read(&mut app), "pause/settings"), "pause/settings  button \"Settings\" → open settings");
}
