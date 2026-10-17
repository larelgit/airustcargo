//! The player's keys in menus: the pause key opens `pause`, the `menu` context of the input map
//! moves through it (keys, a gamepad's D-pad and left stick), and every key is a `ui_*` call of agent `input`.

use shards::bevy::input::gamepad::{GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent};
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

/// A gamepad connected as Bevy's gilrs plugin connects one; its buttons and stick are raw events.
fn connect_pad(app: &mut App) -> Entity {
    let pad = app.world_mut().spawn_empty().id();
    let connected = GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None };
    app.world_mut().write_message(GamepadConnectionEvent::new(pad, connected));
    app.update();
    pad
}

/// Down for a frame, up for a frame.
fn press(app: &mut App, pad: Entity, button: GamepadButton) {
    for value in [1.0, 0.0] {
        app.world_mut().write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(pad, button, value)));
        app.update();
    }
}

/// The left stick pushed to (`x`, `y`) for a frame, then let go.
fn flick(app: &mut App, pad: Entity, x: f32, y: f32) {
    for (x, y) in [(x, y), (0.0, 0.0)] {
        for (axis, value) in [(GamepadAxis::LeftStickX, x), (GamepadAxis::LeftStickY, y)] {
            app.world_mut().write_message(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(pad, axis, value)));
        }
        app.update();
    }
}

#[test]
fn a_gamepad_walks_the_menus_with_the_d_pad_and_the_stick() {
    let mut app = game();
    let pad = connect_pad(&mut app);
    press(&mut app, pad, GamepadButton::Start);
    assert!(app.world().resource::<UiStack>().is_open("pause"), "Start is the pause button");
    assert_eq!(top_context(&app), "menu");

    press(&mut app, pad, GamepadButton::DPadDown);
    press(&mut app, pad, GamepadButton::South);
    assert!(app.world().resource::<UiStack>().is_open("settings"), "down to Settings, South (A, ✕) presses it");
    flick(&mut app, pad, 0.0, -0.9);
    press(&mut app, pad, GamepadButton::DPadRight);
    assert_eq!(app.world().resource::<AccessibilitySettings>().text_scale, 1.25, "the stick is down too, right moves the slider");

    press(&mut app, pad, GamepadButton::East);
    assert!(!app.world().resource::<UiStack>().is_open("settings"), "East (B, ○) is back");
    press(&mut app, pad, GamepadButton::East);
    assert!(!app.world().resource::<UiStack>().is_open("pause"));
    app.update();
    assert_eq!(top_context(&app), "gameplay");

    // The same calls as the keyboard's: a replay repeats them, whatever pressed them.
    assert_eq!(
        journal(&app, "input: ui_"),
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
