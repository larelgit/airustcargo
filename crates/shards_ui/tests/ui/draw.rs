//! With a window, widgets become Bevy UI nodes that screen readers can read.

use accesskit::Role;
use serde_json::json;
use shards::bevy::a11y::AccessibilityNode;
use shards::bevy::input_focus::InputFocus;
use shards::bevy::window::Window;
use shards::prelude::*;
use shards::ui::UiWidget;

use crate::common::*;

/// The entity of widget `path`.
fn widget(app: &mut App, path: &str) -> Entity {
    let mut q = app.world_mut().query::<(Entity, &UiWidget)>();
    q.iter(app.world()).find(|(_, w)| w.path == path).map(|(e, _)| e).unwrap_or_else(|| panic!("no widget {path}"))
}

fn a11y(app: &mut App, path: &str) -> accesskit::Node {
    let e = widget(app, path);
    app.world().get::<AccessibilityNode>(e).unwrap_or_else(|| panic!("{path} has no accessibility node")).0.clone()
}

/// The texts under `e`, its parts' and its own.
fn texts(app: &App, e: Entity) -> Vec<String> {
    let mut out: Vec<String> = app.world().get::<Text>(e).map(|t| t.0.clone()).into_iter().collect();
    for c in app.world().get::<Children>(e).map(|c| c.iter().collect::<Vec<_>>()).unwrap_or_default() {
        out.extend(texts(app, c));
    }
    out
}

/// A headless game with a window entity: what drawing needs, without a GPU.
fn windowed() -> App {
    let mut app = game();
    app.world_mut().spawn(Window::default());
    app.update();
    app
}

#[test]
fn with_a_window_widgets_are_nodes_with_roles_labels_and_values() {
    let mut app = windowed();
    let health = widget(&mut app, "hud/health");
    assert!(app.world().get::<Node>(health).is_some(), "the HUD is drawn");
    assert_eq!(texts(&app, health), ["HP  100/100"]);
    let node = a11y(&mut app, "hud/health");
    assert_eq!((node.role(), node.label(), node.numeric_value(), node.max_numeric_value()), (Role::ProgressIndicator, Some("HP"), Some(100.0), Some(100.0)));

    // A value change moves the bar in place.
    hurt_to(&mut app, 40.0);
    app.update();
    assert_eq!(texts(&app, health), ["HP  40/100"]);
    assert_eq!(a11y(&mut app, "hud/health").numeric_value(), Some(40.0));

    // A menu: a dialog with buttons, the focused one outlined and focused for screen readers.
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    app.update();
    let root = widget(&mut app, "pause");
    assert_eq!(a11y(&mut app, "pause").role(), Role::Dialog);
    assert_eq!(a11y(&mut app, "pause").label(), Some("Paused"));
    assert!(app.world().get::<ChildOf>(root).is_some(), "a menu sits in a frame that dims the game");
    let resume = widget(&mut app, "pause/resume");
    assert_eq!((a11y(&mut app, "pause/resume").role(), a11y(&mut app, "pause/resume").label()), (Role::Button, Some("Resume")));
    assert_eq!(texts(&app, resume), ["Resume"]);
    assert!(app.world().get::<Outline>(resume).is_some(), "focus is drawn");
    assert_eq!(app.world().resource::<InputFocus>().get(), Some(resume));

    ok(&mut app, "ui_navigate", json!({"dir": "down"}));
    app.update();
    let settings = widget(&mut app, "pause/settings");
    assert!(app.world().get::<Outline>(resume).is_none());
    assert!(app.world().get::<Outline>(settings).is_some());
    assert_eq!(app.world().resource::<InputFocus>().get(), Some(settings));

    // Settings: a slider with its range, a choice with its value, a hidden page.
    ok(&mut app, "ui_navigate", json!({"dir": "confirm"}));
    app.update();
    let slider = a11y(&mut app, "settings/text_size");
    assert_eq!((slider.role(), slider.numeric_value(), slider.min_numeric_value(), slider.max_numeric_value()), (Role::Slider, Some(1.0), Some(0.5), Some(3.0)));
    assert_eq!(a11y(&mut app, "settings/flashes").value(), Some("full"), "what the option says, without the arrows drawn around it");
    let controls = widget(&mut app, "settings/controls");
    assert_eq!(app.world().get::<Node>(controls).unwrap().display, Display::None, "the page not shown");
    ok(&mut app, "ui_set", json!({"id": "settings/tabs", "value": "controls"}));
    app.update();
    assert_eq!(app.world().get::<Node>(controls).unwrap().display, Display::Flex);
    let display = widget(&mut app, "settings/display");
    assert_eq!(app.world().get::<Node>(display).unwrap().display, Display::None);

    // The player's text size scales the fonts.
    let hint = widget(&mut app, "hud/hint");
    let before = app.world().get::<TextFont>(hint).unwrap().font_size;
    ok(&mut app, "ui_set", json!({"id": "settings/tabs", "value": "display"}));
    ok(&mut app, "ui_set", json!({"id": "settings/text_size", "value": 2}));
    app.update();
    let after = app.world().get::<TextFont>(hint).unwrap().font_size;
    assert_eq!((before, after), (FontSize::Px(15.0), FontSize::Px(30.0)));

    // Closing takes the frames with the screens.
    ok(&mut app, "ui_navigate", json!({"dir": "back"}));
    ok(&mut app, "ui_navigate", json!({"dir": "back"}));
    app.update();
    app.update();
    let mut frames = app.world_mut().query_filtered::<Entity, (With<GlobalZIndex>, Without<UiWidget>)>();
    assert_eq!(frames.iter(app.world()).count(), 1, "only the HUD's frame is left");
}

#[test]
fn without_a_window_nothing_is_drawn() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    app.update();
    let resume = widget(&mut app, "pause/resume");
    assert!(app.world().get::<Node>(resume).is_none(), "a headless game builds no UI nodes");
}

#[test]
fn text_that_names_no_font_draws_with_the_engines_which_has_russian() {
    let app = windowed();
    let fonts = app.world().resource::<Assets<shards::bevy::text::Font>>();
    let font = fonts.get(AssetId::default()).expect("a font behind the default handle");
    assert!(font.data.data() == shards::ui::DEFAULT_FONT, "the engine's font, not Bevy's ASCII one");
    let face = ttf_parser::Face::parse(shards::ui::DEFAULT_FONT, 0).expect("a font file");
    let russian = "Съешь же ещё этих мягких французских булок, да выпей чаю. ЁЖ ЙЪЫЭЮЯ №5 — «ok» … ← ↑ → ↓";
    for c in russian.chars().chain('!'..='~') {
        assert!(face.glyph_index(c).is_some(), "no letter for {c:?}");
    }
    // What the engine's crates count on it to have, they find in the font itself.
    for c in (0..0x3000u32).filter_map(char::from_u32).filter(|&c| shards::ai_core::display::default_font_has(c)) {
        assert!(face.glyph_index(c).is_some(), "default_font_has({c:?}) but the font has no letter for it");
    }
    assert!(!shards::ai_core::display::default_font_has('⚠') && face.glyph_index('⚠').is_none());
}

#[test]
fn a_binding_is_drawn_with_its_inputs_and_asks_for_one_while_it_waits() {
    let mut app = windowed();
    ok(&mut app, "ui_open", json!({"screen": "keys"}));
    app.update();
    let jump = widget(&mut app, "keys/jump");
    assert_eq!(texts(&app, jump), ["Jump", "Space · Pad South"], "the inputs as a player reads them");
    let node = a11y(&mut app, "keys/jump");
    assert_eq!((node.role(), node.label(), node.value()), (Role::Button, Some("Jump"), Some("Space · Pad South")));

    ok(&mut app, "ui_click", json!({"id": "keys/jump"}));
    app.update();
    assert_eq!(texts(&app, jump), ["Jump", "Press a key or button… (Esc cancels)"]);
    assert_eq!(a11y(&mut app, "keys/jump").value(), Some("Press a key or button… (Esc cancels)"));

    // A refused input: the row says which action holds it, and a screen reader hears it too.
    refused(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyE)"}));
    app.update();
    assert_eq!(texts(&app, jump), ["Jump", "Space · Pad South", "E is already used by Interact"]);
    assert_eq!(a11y(&mut app, "keys/jump").description(), Some("E is already used by Interact"));
    ok(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyJ)"}));
    app.update();
    assert_eq!(texts(&app, jump), ["Jump", "J · Pad South"]);
}
