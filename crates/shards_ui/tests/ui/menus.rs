//! Menus as a player goes through them: focus, pages, settings, questions, grids, mistakes.

use serde_json::json;
use shards::prelude::*;
use shards::ui::UiAppExt;

use crate::common::*;

#[test]
fn an_agent_goes_through_the_menus_by_the_ui_tree_alone() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "down"})), "focus pause/settings (button \"Settings\")");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "confirm"})), "pressed pause/settings: opened settings (menu) · focus settings/tabs");

    let tree = read(&mut app);
    assert!(tree.starts_with("SCREENS  settings ◀ takes input · pause · hud\nsettings \"Settings\" (menu)\n"), "{tree}");
    assert_eq!(line(&tree, "settings/tabs"), "settings/tabs  tabs [display] controls ◀ focus");
    assert_eq!(line(&tree, "settings/text_size"), "settings/text_size  slider \"Text size\" 1 (0.5..3 step 0.25) [accessibility.text_scale]");
    assert_eq!(line(&tree, "settings/flashes"), "settings/flashes  choice \"Flashes\" full (full|reduced|off) [accessibility.flashes]");
    assert_eq!(line(&tree, "settings/controls"), "settings/controls  panel \"Controls\" (page not shown)");
    assert!(!tree.contains("settings/hold"), "what is on a hidden page is not listed: {tree}");

    // A slider moves by its step and sets the player's setting with it.
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "down"})), "focus settings/text_size (slider \"Text size\")");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "right"})), "settings/text_size 1 → 1.25");
    assert_eq!(app.world().resource::<AccessibilitySettings>().text_scale, 1.25);
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "settings/text_size", "value": 9})), "settings/text_size 1.25 → 3", "clamped to the slider");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "settings/text_size", "value": "1.6"})), "settings/text_size 3 → 1.5", "snapped to a step");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "settings/flashes", "value": "Reduced"})), "settings/flashes full → reduced");
    assert_eq!(app.world().resource::<AccessibilitySettings>().flashes, "reduced");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "settings/flashes", "value": "reduced"})), "settings/flashes is reduced already");

    // What is on another page is out of reach until the tabs turn to it.
    let hidden = refused(&mut app, "ui_click", json!({"id": "settings/hold"}));
    assert!(hidden.contains("is on a page that is not shown"), "{hidden}");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "settings/tabs", "value": "Controls"})), "settings/tabs display → controls");
    assert_eq!(ok(&mut app, "ui_click", json!({"id": "settings/hold"})), "settings/hold off → on");
    assert!(app.world().resource::<AccessibilitySettings>().hold_to_toggle);

    // Back closes the top screen; the one below keeps its focus.
    assert_eq!(ok(&mut app, "ui_click", json!({"id": "settings/back"})), "pressed settings/back: closed settings; pause is on top · focus pause/settings");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "back"})), "closed pause; no menu is open");
    assert_eq!(
        journal(&app, "ui: "),
        [
            "ui: opened pause",
            "ui: opened settings",
            "ui: pause/settings pressed (open settings)",
            "ui: settings/text_size 1 → 1.25",
            "ui: settings/text_size 1.25 → 3",
            "ui: settings/text_size 3 → 1.5",
            "ui: settings/flashes full → reduced",
            "ui: settings/tabs display → controls",
            "ui: settings/hold off → on",
            "ui: closed settings",
            "ui: settings/back pressed (back)",
            "ui: closed pause",
        ]
    );
}

#[test]
fn focus_wraps_and_a_toggle_or_choice_turns_with_left_and_right() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "up"})), "focus pause/quit (button \"Quit\")", "up from the first wraps to the last");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "next"})), "focus pause/resume (button \"Resume\")");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "prev"})), "focus pause/quit (button \"Quit\")");
    ok(&mut app, "ui_click", json!({"id": "pause/settings"}));
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "right"})), "settings/tabs display → controls");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "right"})), "settings/tabs controls → display", "the last page wraps");
    ok(&mut app, "ui_navigate", json!({"dir": "down"}));
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "down"})), "focus settings/flashes (choice \"Flashes\")");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "left"})), "settings/flashes full → off", "left from the first option wraps");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "confirm"})), "settings/flashes off → full", "confirm turns a choice to the next");
    let slider = refused(&mut app, "ui_click", json!({"id": "settings/text_size"}));
    assert_eq!(slider, "settings/text_size is a slider: `left`/`right` move it, `ui_set` sets it");
}

#[derive(Resource)]
struct GaveUp(String);

#[test]
fn a_modal_must_be_answered_and_a_game_action_does_what_the_game_says() {
    let mut app = game_with(|app| {
        app.add_ui_action("give_up", |world, press| {
            world.insert_resource(GaveUp(press.agent.clone()));
            Ok("you gave up".to_owned())
        });
    });
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    assert_eq!(ok(&mut app, "ui_click", json!({"id": "pause/quit"})), "pressed pause/quit: opened confirm_quit (modal) · focus confirm_quit/yes");
    assert_eq!(refused(&mut app, "ui_click", json!({"id": "pause/resume"})), "pause/resume is under confirm_quit: answer or close that first");
    assert_eq!(
        refused(&mut app, "ui_navigate", json!({"dir": "back"})),
        "confirm_quit cannot be closed with back: answer it (confirm_quit/yes, confirm_quit/no)"
    );
    assert!(refused(&mut app, "ui_close", json!({"screen": "confirm_quit"})).starts_with("confirm_quit cannot be closed so"));
    let tree = read(&mut app);
    assert!(tree.contains("confirm_quit \"Give up?\" (modal, back does not close it)"), "{tree}");

    // A row: left and right walk it, and stop at its ends.
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "right"})), "focus confirm_quit/no (button \"No\")");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "right"})), "nothing right of confirm_quit/no");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "left"})), "focus confirm_quit/yes (button \"Yes\")");
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "confirm"})), "pressed confirm_quit/yes: you gave up");
    assert_eq!(app.world().resource::<GaveUp>().0, PLAYER, "the action knows who pressed");
    assert_eq!(journal(&app, "confirm_quit/yes"), ["ui: confirm_quit/yes pressed (give_up)"]);
}

#[test]
fn a_game_action_nobody_handles_is_refused_naming_the_ones_there_are() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "confirm_quit"}));
    let err = refused(&mut app, "ui_click", json!({"id": "confirm_quit/yes"}));
    assert_eq!(err, "confirm_quit/yes does `give_up`, which this game does not handle (add_ui_action); game actions: none");
    assert!(journal(&app, "pressed").is_empty(), "a refused press is not journaled");
}

#[test]
fn arrows_walk_a_grid_both_ways_and_a_comparison_reads_before_and_after() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "shop"}));
    let mut go = |dir: &str| ok(&mut app, "ui_navigate", json!({"dir": dir}));
    assert_eq!(go("right"), "focus shop/b (button \"B\")");
    assert_eq!(go("right"), "focus shop/c (button \"C\")");
    assert_eq!(go("right"), "nothing right of shop/c", "a row of the grid ends");
    assert_eq!(go("down"), "nothing below shop/c", "the second row has two");
    assert_eq!(go("left"), "focus shop/b (button \"B\")");
    assert_eq!(go("down"), "focus shop/e (button \"E\")");
    assert_eq!(go("left"), "focus shop/d (button \"D\")");
    assert_eq!(go("left"), "nothing left of shop/d", "left does not wrap into the row above");
    assert_eq!(go("up"), "focus shop/a (button \"A\")");
    let tree = read(&mut app);
    assert_eq!(line(&tree, "shop/goods"), "shop/goods  grid 3 columns");
    assert_eq!(line(&tree, "shop/sword"), "shop/sword  compare \"Sword\" damage 12 → 15 (+3) · speed 5 → 4 (-1)");
}

#[test]
fn mistakes_say_how_to_fix_the_call() {
    let mut app = game();
    let err = refused(&mut app, "ui_open", json!({"screen": "paws"}));
    assert_eq!(err, "no screen `paws`; screens: confirm_quit, hud, pause, settings, shop");
    assert_eq!(refused(&mut app, "ui_open", json!({"screen": "setting"})), "no screen `setting` (did you mean `settings`?); screens: confirm_quit, hud, pause, settings, shop");
    let err = refused(&mut app, "ui_click", json!({"id": "pause/resume"}));
    assert!(err.starts_with("no widget `pause/resume`: screen `pause` is not open (open: hud)"), "{err}");
    assert!(refused(&mut app, "ui_navigate", json!({"dir": "down"})).starts_with("no menu is open (only the HUD"));
    assert!(refused(&mut app, "ui_click", json!({"id": "hud/health"})).ends_with("is on the HUD, which takes no input"));

    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    let err = refused(&mut app, "ui_click", json!({"id": "pause/resum"}));
    assert_eq!(err, "no widget `pause/resum` on pause; it has pause/resume, pause/settings, pause/patch_up, pause/quit");
    let err = refused(&mut app, "ui_navigate", json!({"dir": "sideways"}));
    assert_eq!(err, "no direction `sideways`: up, down, left, right, next, prev, confirm or back");
    let err = refused(&mut app, "ui_set", json!({"id": "pause/resume", "value": 1}));
    assert_eq!(err, "pause/resume is a button: `ui_set` takes sliders, toggles, choices and tabs");
    ok(&mut app, "ui_click", json!({"id": "pause/settings"}));
    let err = refused(&mut app, "ui_set", json!({"id": "settings/text_size", "value": "big"}));
    assert_eq!(err, "settings/text_size is a slider: a number from 0.5 to 3, not `big`");
    let err = refused(&mut app, "ui_set", json!({"id": "settings/tabs", "value": "audio"}));
    assert_eq!(err, "settings/tabs has pages display, controls (or 1 to 2), not `audio`");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "settings/tabs", "value": 2})), "settings/tabs display → controls", "pages count from 1");

    // A player cannot reach a dev tool through a button: the button calls it as the player.
    ok(&mut app, "ui_navigate", json!({"dir": "back"}));
    let err = refused(&mut app, "ui_click", json!({"id": "pause/patch_up"}));
    assert_eq!(err, "pause/patch_up calls heal: `heal` needs dev scope; agent `agent` has player scope");
}
