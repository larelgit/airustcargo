//! The controls in a menu: binding widgets show an action's inputs, an agent changes them with
//! `ui_set`, a player by confirming one and pressing the new key, mouse button or gamepad button.
//! Every change goes through the `controls` settings, which refuse an input another action reads.

use serde_json::json;
use shards::ai_core::replay;
use shards::bevy::input::gamepad::{RawGamepadButtonChangedEvent, RawGamepadEvent};
use shards::bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use shards::bevy::input::mouse::MouseButtonInput;
use shards::bevy::input::ButtonState;
use shards::motion::Binding;
use shards::prelude::*;
use shards::testing;
use shards::ui::CapturedClicks;

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

fn mouse(app: &mut App, button: MouseButton, state: ButtonState) {
    app.world_mut().write_message(MouseButtonInput { button, state, window: Entity::PLACEHOLDER });
    app.update();
}

/// A gamepad button down for a frame, up for a frame, as the gamepad driver says it.
fn pad_tap(app: &mut App, pad: Entity, button: GamepadButton) {
    for value in [1.0, 0.0] {
        app.world_mut().write_message(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(pad, button, value)));
        app.update();
    }
}

/// What the `controls` settings keep for `key` (`gameplay.jump`): only what differs from the defaults.
fn saved(app: &App, key: &str) -> Option<Vec<String>> {
    app.world().resource::<ControlSettings>().bindings.get(key).cloned()
}

fn waiting(app: &App) -> Option<String> {
    shards::ui::capturing(app.world())
}

/// The `keys` screen open, and a frame for the menu's controls to take over.
fn keys_open() -> App {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "keys"}));
    app.update();
    app
}

#[test]
fn an_agent_reads_the_controls_and_changes_them_with_ui_set() {
    let mut app = keys_open();
    let tree = read(&mut app);
    assert_eq!(
        line(&tree, "keys/jump"),
        "keys/jump  binding \"Jump\" Space · Pad South = Key(Space), GamepadButton(South) [controls.bindings.gameplay.jump] ◀ focus"
    );
    assert_eq!(line(&tree, "keys/jump_pad"), "keys/jump_pad  binding \"Jump (pad)\" Pad South = GamepadButton(South) [controls.bindings.gameplay.jump slot 1]");
    assert_eq!(line(&tree, "keys/confirm"), "keys/confirm  binding \"Confirm\" Enter · Pad South = Key(Enter), GamepadButton(South) [controls.bindings.menu.confirm]");
    let fly = line(&tree, "keys/fly");
    assert!(fly.starts_with("keys/fly  binding \"Fly\" (no action `fly` in context `gameplay`") && fly.contains("actions: move, look, jump"), "{fly}");

    // A key takes the keyboard's place, a gamepad button the gamepad's.
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyJ)"})), "keys/jump Space · Pad South → J · Pad South");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/jump", "value": "GamepadButton(Select)"})), "keys/jump J · Pad South → J · Pad Select");
    assert_eq!(saved(&app, "gameplay.jump"), Some(vec!["Key(KeyJ)".to_owned(), "GamepadButton(Select)".to_owned()]));
    // A widget with a slot changes that slot only; an input the action has already changes nothing.
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/jump_pad", "value": "GamepadButton(South)"})), "keys/jump_pad Pad Select → Pad South");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyJ)"})), "keys/jump is J · Pad South already");
    // The input map follows within a frame.
    app.update();
    let jump: Vec<String> = app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings.iter().map(Binding::text).collect();
    assert_eq!(jump, ["Key(KeyJ)", "GamepadButton(South)"]);
    // Another context is as good; `default` gives the defaults back, and the settings keep nothing.
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/confirm", "value": "Key(Space)"})), "keys/confirm Enter · Pad South → Space · Pad South");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/confirm", "value": "default"})), "keys/confirm Space · Pad South → Enter · Pad South");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/jump", "value": "default"})), "keys/jump J · Pad South → Space · Pad South");
    assert!(app.world().resource::<ControlSettings>().bindings.is_empty(), "the defaults are no setting");
    assert_eq!(
        journal(&app, "ui: keys/jump "),
        ["ui: keys/jump Space · Pad South → J · Pad South", "ui: keys/jump J · Pad South → J · Pad Select", "ui: keys/jump J · Pad South → Space · Pad South"]
    );
    assert!(!journal(&app, "settings: controls.bindings.gameplay.jump").is_empty(), "the settings say what changed");
}

#[test]
fn an_input_another_action_reads_is_refused_and_the_row_names_the_action() {
    let mut app = keys_open();
    let err = refused(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyE)"}));
    let direct = refused(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.jump", "value": ["Key(KeyE)", "GamepadButton(South)"]}));
    assert_eq!(err, format!("keys/jump: {direct} (on this screen: keys/interact)"), "the settings' refusal, and where the other action is");
    assert!(direct.contains("Key(KeyE) is already bound to `interact` in `gameplay`; free it first"), "{direct}");
    assert!(app.world().resource::<ControlSettings>().bindings.is_empty(), "refused: nothing changed");
    assert_eq!(
        line(&read(&mut app), "keys/jump"),
        "keys/jump  binding \"Jump\" Space · Pad South = Key(Space), GamepadButton(South) [controls.bindings.gameplay.jump] · E is already used by Interact ◀ focus"
    );
    // An action with no row on the screen is named as the input map names it.
    let err = refused(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyW)"}));
    assert!(err.contains("Key(KeyW) is already bound to `move`") && !err.contains("on this screen"), "{err}");
    assert!(line(&read(&mut app), "keys/jump").contains(" · W is already used by move"));

    // Freed first, the input is the jump's; the row says nothing more.
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/interact", "value": "Key(KeyG)"})), "keys/interact E · Pad North → G · Pad North");
    assert_eq!(ok(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Key(KeyE)"})), "keys/jump Space · Pad South → E · Pad South");
    assert!(!line(&read(&mut app), "keys/jump").contains("already used"));
    assert_eq!(saved(&app, "gameplay.interact"), Some(vec!["Key(KeyG)".to_owned(), "GamepadButton(North)".to_owned()]));

    // What is no button, no input or no action is refused with what to give.
    let axis = refused(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Stick(stick: Left)"}));
    assert!(axis.starts_with("keys/jump takes a key, a mouse button or a gamepad button, not an axis like Stick("), "{axis}");
    let typo = refused(&mut app, "ui_set", json!({"id": "keys/jump", "value": "Spacebar"}));
    assert!(typo.contains("`Spacebar` is not a binding") && typo.contains("Key(KeyE)"), "{typo}");
    let fly = refused(&mut app, "ui_set", json!({"id": "keys/fly", "value": "Key(KeyF)"}));
    assert!(fly.starts_with("keys/fly: no action `fly` in context `gameplay`"), "{fly}");
    assert_eq!(saved(&app, "gameplay.jump"), Some(vec!["Key(KeyE)".to_owned(), "GamepadButton(South)".to_owned()]), "refusals change nothing");
}

#[test]
fn an_agent_starts_and_stops_a_wait_and_moving_on_stops_it_too() {
    let mut app = keys_open();
    let waits = ok(&mut app, "ui_click", json!({"id": "keys/jump"}));
    assert!(waits.starts_with("keys/jump waits for an input: ") && waits.ends_with("`ui_set id=keys/jump value=Key(KeyK)`"), "{waits}");
    assert_eq!(waiting(&app).as_deref(), Some("keys/jump"));
    assert!(line(&read(&mut app), "keys/jump").contains(" · waiting for an input (Escape cancels; ui_set id=keys/jump value=Key(KeyK))"));
    let again = ok(&mut app, "ui_navigate", json!({"dir": "confirm"}));
    assert!(again.starts_with("keys/jump is waiting for an input already"), "{again}");
    // `back` stops the wait, and the screen stays.
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "back"})), "keys/jump stopped waiting for an input; it stays Space · Pad South");
    assert!(app.world().resource::<UiStack>().is_open("keys"));
    assert_eq!(waiting(&app), None);
    // Moving on stops it too; a wait answered with `ui_set` ends.
    ok(&mut app, "ui_click", json!({"id": "keys/jump"}));
    assert_eq!(ok(&mut app, "ui_navigate", json!({"dir": "down"})), "focus keys/interact (binding \"Interact\")");
    assert_eq!(waiting(&app), None);
    ok(&mut app, "ui_click", json!({"id": "keys/interact"}));
    ok(&mut app, "ui_set", json!({"id": "keys/interact", "value": "Key(KeyG)"}));
    assert_eq!(waiting(&app), None);
    let left = refused(&mut app, "ui_set", json!({"id": "keys/back", "value": "on"}));
    assert_eq!(left, "keys/back is a button: `ui_set` takes sliders, toggles, choices, tabs and bindings");
    assert_eq!(
        journal(&app, "ui: keys/"),
        [
            "ui: keys/jump waits for an input",
            "ui: keys/jump stopped waiting for an input",
            "ui: keys/jump waits for an input",
            "ui: keys/jump stopped waiting for an input",
            "ui: keys/interact waits for an input",
            "ui: keys/interact E · Pad North → G · Pad North",
        ]
    );
}

#[test]
fn a_pressed_key_becomes_the_binding_and_escape_stops_the_wait_without_closing_the_screen() {
    let mut app = keys_open();
    tap(&mut app, KeyCode::Enter);
    assert_eq!(waiting(&app).as_deref(), Some("keys/jump"), "Enter confirms the focused row: it waits");
    tap(&mut app, KeyCode::KeyK);
    assert_eq!(waiting(&app), None);
    assert_eq!(saved(&app, "gameplay.jump"), Some(vec!["Key(KeyK)".to_owned(), "GamepadButton(South)".to_owned()]));

    // An arrow another action reads: refused and named, and the focus stays where it was.
    tap(&mut app, KeyCode::Enter);
    tap(&mut app, KeyCode::ArrowDown);
    let tree = read(&mut app);
    assert!(line(&tree, "keys/jump").ends_with(" · Down is already used by move ◀ focus"), "{tree}");

    // Escape stops the wait and is no binding; the screen stays open until the next Escape.
    tap(&mut app, KeyCode::Enter);
    tap(&mut app, KeyCode::Escape);
    assert_eq!(waiting(&app), None);
    assert!(app.world().resource::<UiStack>().is_open("keys"), "Escape stopped the wait, not the screen");
    assert_eq!(saved(&app, "gameplay.jump"), Some(vec!["Key(KeyK)".to_owned(), "GamepadButton(South)".to_owned()]));
    tap(&mut app, KeyCode::Escape);
    assert!(!app.world().resource::<UiStack>().is_open("keys"));
    assert_eq!(
        journal(&app, "input: ui_"),
        [
            "input: ui_navigate {\"dir\":\"confirm\"}",
            "input: ui_set {\"id\":\"keys/jump\",\"value\":\"Key(KeyK)\"}",
            "input: ui_navigate {\"dir\":\"confirm\"}",
            "input: ui_set {\"id\":\"keys/jump\",\"value\":\"Key(ArrowDown)\"}",
            "input: ui_navigate {\"dir\":\"confirm\"}",
            "input: ui_navigate {\"dir\":\"back\"}",
            "input: ui_navigate {\"dir\":\"back\"}",
        ]
    );
}

#[test]
fn a_gamepad_button_becomes_the_binding_and_start_stops_the_wait() {
    let mut app = keys_open();
    let pad = app.world_mut().spawn(Gamepad::default()).id();
    app.update();
    ok(&mut app, "ui_click", json!({"id": "keys/jump"}));
    pad_tap(&mut app, pad, GamepadButton::Select);
    assert_eq!(waiting(&app), None);
    assert_eq!(saved(&app, "gameplay.jump"), Some(vec!["Key(Space)".to_owned(), "GamepadButton(Select)".to_owned()]), "the gamepad's input changed");
    ok(&mut app, "ui_click", json!({"id": "keys/jump"}));
    pad_tap(&mut app, pad, GamepadButton::Start);
    assert_eq!(waiting(&app), None);
    assert!(app.world().resource::<UiStack>().is_open("keys"));
    assert_eq!(saved(&app, "gameplay.jump"), Some(vec!["Key(Space)".to_owned(), "GamepadButton(Select)".to_owned()]), "Start is no binding");
}

#[test]
fn a_mouse_button_becomes_the_binding_and_its_click_presses_nothing() {
    let mut app = keys_open();
    ok(&mut app, "ui_click", json!({"id": "keys/interact"}));
    mouse(&mut app, MouseButton::Back, ButtonState::Pressed);
    assert_eq!(saved(&app, "gameplay.interact"), Some(vec!["Mouse(Back)".to_owned(), "GamepadButton(North)".to_owned()]));
    assert_eq!(app.world().resource::<CapturedClicks>().0, [MouseButton::Back], "the click its release makes is held back");
    mouse(&mut app, MouseButton::Back, ButtonState::Released);
    app.update();
    assert!(app.world().resource::<CapturedClicks>().0.is_empty(), "let go a frame ago: forgotten");
    assert_eq!(line(&read(&mut app), "keys/interact").split(" = ").next(), Some("keys/interact  binding \"Interact\" Mouse Back · Pad North"));
}

#[test]
fn a_replay_changes_the_controls_as_the_keys_did() {
    let mut app = keys_open();
    tap(&mut app, KeyCode::Enter);
    tap(&mut app, KeyCode::KeyK);
    testing::ok(&mut app, "step", json!({"ticks": 10}));
    let recording = replay::save(app.world());
    let calls: Vec<&str> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            replay::ReplayEvent::Call { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["ui_open", "ui_navigate", "ui_set"], "the settings change inside `ui_set` is made again by it");
    let mut fresh = game();
    let report = testing::verify(&mut fresh, recording).unwrap();
    assert_eq!(report.diverged_at, None, "{report:?}");
    assert_eq!(report.interrupted, None, "{report:?}");
    assert_eq!(saved(&fresh, "gameplay.jump"), Some(vec!["Key(KeyK)".to_owned(), "GamepadButton(South)".to_owned()]));
}
