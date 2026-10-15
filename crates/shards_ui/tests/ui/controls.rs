//! The player's keys in menus: the pause key opens `pause`, the `menu` context of the input map
//! moves through it, and every key is a `ui_*` call of agent `input`.

use shards::bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use shards::bevy::input::ButtonState;
use shards::motion::InputContexts;
use shards::prelude::*;

use crate::common::*;

fn key(app: &mut App, key_code: KeyCode, state: ButtonState) {
    let logical_key = Key::Unidentified(NativeKey::Unidentified);
    app.world_mut().write_message(KeyboardInput { key_code, logical_key, state, text: None, repeat: false, window: Entity::PLACEHOLDER });
}

/// Down for a frame, up for a frame.
fn tap(app: &mut App, code: KeyCode) {
    key(app, code, ButtonState::Pressed);
    app.update();
    key(app, code, ButtonState::Released);
    app.update();
}

fn top_context(app: &App) -> String {
    app.world().resource::<InputContexts>().top().to_owned()
}

#[test]
fn keys_open_walk_and_close_the_menus_as_calls_a_replay_records() {
    let mut app = game();
    assert_eq!(top_context(&app), "gameplay");
    tap(&mut app, KeyCode::Escape);
    assert!(app.world().resource::<UiStack>().is_open("pause"), "Escape is the pause key");
    assert_eq!(top_context(&app), "menu", "the character hears nothing while the menu is open");

    tap(&mut app, KeyCode::ArrowDown);
    tap(&mut app, KeyCode::Enter);
    assert!(app.world().resource::<UiStack>().is_open("settings"), "down to Settings, Enter presses it");
    tap(&mut app, KeyCode::KeyS);
    tap(&mut app, KeyCode::ArrowRight);
    assert_eq!(app.world().resource::<AccessibilitySettings>().text_scale, 1.25, "S is down too, right moves the slider");

    tap(&mut app, KeyCode::Escape);
    assert!(!app.world().resource::<UiStack>().is_open("settings"), "in a menu Escape is back");
    tap(&mut app, KeyCode::Escape);
    assert!(!app.world().resource::<UiStack>().is_open("pause"));
    app.update();
    assert_eq!(top_context(&app), "gameplay");

    let calls = journal(&app, "input: ui_");
    assert_eq!(
        calls,
        [
            "input: ui_open {\"screen\":\"pause\"}",
            "input: ui_navigate {\"dir\":\"down\"}",
            "input: ui_navigate {\"dir\":\"confirm\"}",
            "input: ui_navigate {\"dir\":\"down\"}",
            "input: ui_navigate {\"dir\":\"right\"}",
            "input: ui_navigate {\"dir\":\"back\"}",
            "input: ui_navigate {\"dir\":\"back\"}",
        ]
    );
}
