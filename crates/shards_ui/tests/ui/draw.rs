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
