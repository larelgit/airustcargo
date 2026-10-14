//! The input action map: devices (fed to the `ButtonInput` resources and `Gamepad`
//! components by hand, as Bevy's input plugin would) become `Intent`.

use ai_core::{AiCorePlugin, AiJournal, AiKind, JournalKind, SimConfig};
use bevy::input::gamepad::GamepadInput;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{DODGE, GLIDE, JUMP, SPRINT};
use shards_core::{Intent, LocalPlayer, Order, Target};
use shards_motion::*;

use crate::engine::*;

fn input_app_with(settings: Option<InputSettings>) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin {
            remote_port: None,
            crash_replays: false,
            sim: SimConfig { start_paused: true, ..default() },
            ..default()
        })
        .add_plugins(MotionPlugin)
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>();
    if let Some(settings) = settings {
        app.insert_resource(settings);
    }
    app.finish();
    app.cleanup();
    app.update();
    app
}

fn input_app() -> App {
    input_app_with(None)
}

fn local_player(app: &mut App) -> Entity {
    app.world_mut().spawn((AiKind::new("Hero"), LocalPlayer, character("", Vec3::ZERO))).id()
}

/// One frame, then what Bevy's input systems do at the start of the next: forget the edges.
fn frame(app: &mut App) {
    app.update();
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
    app.world_mut().resource_mut::<ButtonInput<MouseButton>>().clear();
    let world = app.world_mut();
    let mut pads = world.query::<&mut Gamepad>();
    for mut gamepad in pads.iter_mut(world) {
        gamepad.digital_mut().clear();
    }
}

fn keys(app: &mut App) -> Mut<'_, ButtonInput<KeyCode>> {
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>()
}

fn press(app: &mut App, key: KeyCode) {
    keys(app).press(key);
}

fn release(app: &mut App, key: KeyCode) {
    keys(app).release(key);
}

fn tap(app: &mut App, key: KeyCode) {
    press(app, key);
    frame(app);
    release(app, key);
    frame(app);
}

fn pressed_names(app: &App, e: Entity) -> Vec<String> {
    intent(app, e).pressed.iter().map(|p| p.input.clone()).collect()
}

#[test]
fn keys_steer_and_a_human_takes_over_from_orders() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);
    press(&mut app, KeyCode::KeyW);
    frame(&mut app);
    assert_eq!(intent(&app, e).move_dir, Vec3::NEG_Z, "W is north");
    press(&mut app, KeyCode::KeyD);
    frame(&mut app);
    let dir = intent(&app, e).move_dir;
    assert!((dir - Vec3::new(1.0, 0.0, -1.0).normalize()).length() < 1e-5, "a diagonal is not faster: {dir}");
    release(&mut app, KeyCode::KeyW);
    release(&mut app, KeyCode::KeyD);
    frame(&mut app);
    assert_eq!(intent(&app, e).move_dir, Vec3::ZERO);

    // An agent's order gives way to the human's keys, but a quiet keyboard leaves it alone.
    edit_intent(&mut app, e, |i| {
        i.orders.push_back(Order::MoveTo { target: Target::Point(Vec3::X * 9.0), within: 0.3 });
        i.move_dir = Vec3::X;
    });
    frame(&mut app);
    assert_eq!((intent(&app, e).orders.len(), intent(&app, e).move_dir), (1, Vec3::X));
    press(&mut app, KeyCode::ArrowLeft);
    frame(&mut app);
    assert!(intent(&app, e).orders.is_empty() && intent(&app, e).move_dir == Vec3::NEG_X);
}

#[test]
fn the_move_axis_is_turned_by_the_camera() {
    let mut app = input_app();
    let e = local_player(&mut app);
    app.world_mut().spawn(ThirdPersonCamera { yaw: std::f32::consts::FRAC_PI_2, ..default() });
    frame(&mut app);
    press(&mut app, KeyCode::KeyW);
    frame(&mut app);
    let dir = intent(&app, e).move_dir;
    assert!((dir - Vec3::NEG_X).length() < 1e-5, "with the camera on +X, forward is -X: {dir}");
}

#[test]
fn keys_held_before_the_player_exists_reach_it_when_it_appears() {
    let mut app = input_app();
    press(&mut app, KeyCode::KeyW);
    press(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    frame(&mut app);
    let e = local_player(&mut app);
    frame(&mut app);
    let now = intent(&app, e);
    assert_eq!(now.move_dir, Vec3::NEG_Z, "W has been down the whole time");
    assert!(now.is_held(SPRINT), "so has shift");
}

#[test]
fn the_controls_follow_the_local_player_to_a_new_body() {
    let mut app = input_app();
    let first = local_player(&mut app);
    let second = app.world_mut().spawn((AiKind::new("Hero"), character("", Vec3::X * 4.0))).id();
    press(&mut app, KeyCode::KeyW);
    press(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(intent(&app, first).is_held(SPRINT) && intent(&app, first).move_dir == Vec3::NEG_Z);

    // The marker moves on (a respawn, a hand-over) with the keys still down.
    app.world_mut().entity_mut(first).remove::<LocalPlayer>();
    app.world_mut().entity_mut(second).insert(LocalPlayer);
    frame(&mut app);
    let (old, new) = (intent(&app, first), intent(&app, second));
    assert_eq!((new.move_dir, new.is_held(SPRINT)), (Vec3::NEG_Z, true), "the new body gets what is down");
    assert_eq!((old.move_dir, old.is_held(SPRINT)), (Vec3::ZERO, false), "the old one is not left running");

    // An agent's own steering of the old body is its own: only what the keys wrote is let go.
    edit_intent(&mut app, first, |i| i.move_dir = Vec3::X);
    release(&mut app, KeyCode::KeyW);
    frame(&mut app);
    assert_eq!(intent(&app, first).move_dir, Vec3::X);
    assert_eq!(intent(&app, second).move_dir, Vec3::ZERO);
}

#[test]
fn intent_is_left_untouched_when_nothing_changes() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);
    let stamp = |app: &App| app.world().entity(e).get_ref::<Intent>().unwrap().last_changed().get();
    let quiet = stamp(&app);
    for _ in 0..5 {
        frame(&mut app);
    }
    assert_eq!(stamp(&app), quiet, "an idle keyboard writes nothing (replays record every change)");
    press(&mut app, KeyCode::KeyW);
    frame(&mut app);
    let moving = stamp(&app);
    assert_ne!(moving, quiet);
    for _ in 0..5 {
        frame(&mut app);
    }
    assert_eq!(stamp(&app), moving, "a held key writes once, not every frame");
}

#[test]
fn press_hold_and_toggle_modes() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);

    // Press: once per tap, not while held.
    press(&mut app, KeyCode::Space);
    frame(&mut app);
    frame(&mut app);
    assert_eq!(pressed_names(&app, e), [JUMP], "held down for two frames, one press");
    edit_intent(&mut app, e, |i| i.pressed.clear());
    release(&mut app, KeyCode::Space);
    frame(&mut app);
    assert!(pressed_names(&app, e).is_empty());

    // Hold: held while down.
    press(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(intent(&app, e).is_held(SPRINT));
    release(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(!intent(&app, e).is_held(SPRINT));

    // Toggle (accessibility): a tap switches it on, the next tap off.
    app.world_mut().resource_mut::<InputMap>().set_mode("gameplay", "sprint", BindMode::Toggle).unwrap();
    tap(&mut app, KeyCode::ShiftLeft);
    assert!(intent(&app, e).is_held(SPRINT), "on after a tap, though the key is up");
    tap(&mut app, KeyCode::ShiftLeft);
    assert!(!intent(&app, e).is_held(SPRINT), "off after the next");
    let state = app.world().resource::<ControlState>();
    assert!(!state.held("sprint"));
}

#[test]
fn a_dodge_goes_where_you_steer() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);
    tap(&mut app, KeyCode::ControlLeft);
    let press = intent(&app, e).pressed[0].clone();
    assert_eq!((press.input.as_str(), press.dir), (DODGE, None), "standing still: no direction, the action picks one");
    edit_intent(&mut app, e, |i| i.pressed.clear());
    press_key(&mut app, KeyCode::KeyA);
    press_key(&mut app, KeyCode::ControlLeft);
    frame(&mut app);
    let press = intent(&app, e).pressed[0].clone();
    assert_eq!(press.dir, Some(Vec3::NEG_X));
}

fn press_key(app: &mut App, key: KeyCode) {
    press(app, key);
}

#[test]
fn contexts_stack_and_only_the_top_one_fires() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);
    press(&mut app, KeyCode::ShiftLeft);
    press(&mut app, KeyCode::KeyW);
    frame(&mut app);
    assert!(intent(&app, e).is_held(SPRINT) && intent(&app, e).move_dir == Vec3::NEG_Z);

    // A menu opens: gameplay stops reading, and what it held is let go.
    app.world_mut().resource_mut::<InputContexts>().push("menu");
    frame(&mut app);
    let now = intent(&app, e);
    assert!(!now.is_held(SPRINT) && now.move_dir == Vec3::ZERO, "{now:?}");
    release(&mut app, KeyCode::KeyW);
    tap(&mut app, KeyCode::Space);
    assert!(pressed_names(&app, e).is_empty(), "jump is not a menu control");
    tap(&mut app, KeyCode::Enter);
    assert!(pressed_names(&app, e).is_empty(), "menu actions are for the menu: Intent hears nothing");
    press(&mut app, KeyCode::Enter);
    frame(&mut app);
    assert!(app.world().resource::<ControlState>().just_pressed("confirm"), "the UI reads ControlState");
    release(&mut app, KeyCode::Enter);
    frame(&mut app);

    // Closing it gives the keys that are still down back.
    assert_eq!(app.world_mut().resource_mut::<InputContexts>().pop().as_deref(), Some("menu"));
    frame(&mut app);
    assert!(intent(&app, e).is_held(SPRINT), "shift is still down");
}

#[test]
fn a_gamepad_steers_and_presses() {
    let mut app = input_app();
    let e = local_player(&mut app);
    let pad = app.world_mut().spawn(Gamepad::default()).id();
    frame(&mut app);
    {
        let mut gamepad = app.world_mut().get_mut::<Gamepad>(pad).unwrap();
        gamepad.analog_mut().set(GamepadInput::Axis(GamepadAxis::LeftStickX), 0.8);
        gamepad.analog_mut().set(GamepadInput::Axis(GamepadAxis::LeftStickY), 0.05);
        gamepad.digital_mut().press(GamepadButton::South);
        gamepad.digital_mut().press(GamepadButton::LeftThumb);
    }
    frame(&mut app);
    let now = intent(&app, e);
    // Past the dead zone: (0.8 - 0.15) / 0.85 of the way, and the 0.05 wobble hardly counts.
    assert!((now.move_dir.x - 0.65 / 0.85).abs() < 0.02 && now.move_dir.z.abs() < 0.06, "{}", now.move_dir);
    assert_eq!(pressed_names(&app, e), [JUMP]);
    assert!(now.is_held(SPRINT));

    // Inside the dead zone, nothing.
    app.world_mut().get_mut::<Gamepad>(pad).unwrap().analog_mut().set(GamepadInput::Axis(GamepadAxis::LeftStickX), 0.1);
    frame(&mut app);
    assert_eq!(intent(&app, e).move_dir, Vec3::ZERO);
}

#[test]
fn the_look_axis_collects_mouse_and_stick_for_the_camera() {
    let mut app = input_app();
    local_player(&mut app);
    app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(40.0, -20.0) });
    frame(&mut app);
    let look = app.world().resource::<ControlState>().axis("look");
    // 40 px right, 20 px up, 0.0025 per pixel.
    assert!((look - Vec2::new(0.1, 0.05)).length() < 1e-5, "{look}");
    app.insert_resource(AccumulatedMouseMotion::default());
    frame(&mut app);
    assert_eq!(app.world().resource::<ControlState>().axis("look"), Vec2::ZERO);
}

#[test]
fn rebinding_moves_a_control_and_conflicts_name_the_other_action() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);
    let out = ok(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyJ)"}));
    assert_eq!(out, "ok: gameplay/jump: slot 0 = Key(KeyJ)");
    tap(&mut app, KeyCode::Space);
    assert!(pressed_names(&app, e).is_empty(), "Space is no jump any more");
    tap(&mut app, KeyCode::KeyJ);
    assert_eq!(pressed_names(&app, e), [JUMP]);

    let clash = fails(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyW)"}));
    assert!(clash.contains("already bound to `move` in `gameplay`") && clash.contains("force"), "{clash}");
    let forced = ok(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyW)", "force": true}));
    assert!(forced.contains("taken from move"), "{forced}");
    let typo = fails(&mut app, "rebind", json!({"action": "jmp", "binding": "Key(KeyJ)"}));
    assert!(typo.contains("no action `jmp`") && typo.contains("did you mean `jump`"), "{typo}");
    let bad = fails(&mut app, "rebind", json!({"action": "jump", "binding": "Space"}));
    assert!(bad.contains("not a binding") && bad.contains("Key(KeyE)"), "{bad}");
    let nothing = fails(&mut app, "rebind", json!({"action": "jump"}));
    assert!(nothing.contains("nothing to change"), "{nothing}");
    let mode = ok(&mut app, "rebind", json!({"action": "glide", "mode": "toggle"}));
    assert!(mode.contains("mode toggle"), "{mode}");
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "glide").unwrap().mode, BindMode::Toggle);
    // A refused change changes nothing at all.
    let before = app.world().resource::<InputMap>().clone();
    fails(&mut app, "rebind", json!({"action": "light", "binding": "Key(KeyQ)", "mode": "fly"}));
    assert_eq!(*app.world().resource::<InputMap>(), before);
    let _ = GLIDE;
}

#[test]
fn input_bindings_lists_the_controls() {
    let mut app = input_app();
    let out = ok(&mut app, "input_bindings", json!({}));
    assert!(out.starts_with("gameplay (active)\n"), "{out}");
    assert!(out.contains("jump") && out.contains("Key(Space) · GamepadButton(South)"), "{out}");
    assert!(out.contains("[hold] Key(ShiftLeft)") && out.contains("other contexts: menu"), "{out}");
    let menu = ok(&mut app, "input_bindings", json!({"context": "menu"}));
    assert!(menu.starts_with("menu\n") && menu.contains("confirm"), "{menu}");
    let none = fails(&mut app, "input_bindings", json!({"context": "gamplay"}));
    assert!(none.contains("no input context `gamplay` (did you mean `gameplay`?)"), "{none}");
}

#[test]
fn the_players_changes_are_saved_and_loaded_again() {
    let dir = temp_dir("input_save");
    let file = dir.join("config/input.ron");
    let settings = || InputSettings { file: Some(file.clone()), ..default() };

    let mut app = input_app_with(Some(settings()));
    assert!(!file.exists());
    let out = ok(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyJ)", "save": true}));
    assert!(out.contains("saved to") && file.exists(), "{out}");
    ok(&mut app, "rebind", json!({"action": "sprint", "mode": "toggle", "save": true}));
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("KeyJ") && text.contains("Toggle"), "{text}");
    assert!(!text.contains("dodge"), "only what changed is saved, so new default actions still reach old files:\n{text}");
    assert!(!file.with_extension("ron.tmp").exists(), "written through a temporary file");

    // A new run reads them back.
    let mut next = input_app_with(Some(settings()));
    let map = next.world().resource::<InputMap>().clone();
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings[0], Binding::Key(KeyCode::KeyJ));
    assert_eq!(map.action("gameplay", "sprint").unwrap().mode, BindMode::Toggle);
    assert_eq!(map.action("gameplay", "dodge"), InputMap::standard().action("gameplay", "dodge"));
    let e = local_player(&mut next);
    frame(&mut next);
    tap(&mut next, KeyCode::KeyJ);
    assert_eq!(pressed_names(&next, e), [JUMP]);

    // A file nobody can read does not stop the game: the defaults, and a warning.
    std::fs::write(&file, "(contexts: [oops").unwrap();
    let broken = input_app_with(Some(settings()));
    assert_eq!(*broken.world().resource::<InputMap>(), InputMap::standard());
    let said = broken.world().resource::<AiJournal>().after(0).0.any(|e| e.kind == JournalKind::Warn && e.text.contains("using the default controls"));
    assert!(said);
}

#[test]
fn a_rebind_does_not_destroy_a_controls_file_that_could_not_be_read() {
    let dir = temp_dir("input_bad_file");
    let file = dir.join("input.ron");
    let hand_edited = "(contexts: [ (name: \"gameplay\", actions: [ (action: \"jump\", bindings: [Key(KeyJ)]), oops";
    std::fs::write(&file, hand_edited).unwrap();
    let mut app = input_app_with(Some(InputSettings { file: Some(file.clone()), ..default() }));
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard(), "the defaults, and a warning");

    // The first save says the old file is kept, and keeps it.
    let out = ok(&mut app, "rebind", json!({"action": "sprint", "binding": "Key(KeyV)", "save": true}));
    let backup = file.with_extension("ron.bad");
    assert!(out.contains("saved to") && out.contains("could not be read") && out.contains("input.ron.bad"), "{out}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), hand_edited, "everything the player had written is still there");
    assert!(InputMap::load_overrides(&file).is_ok(), "and the new file reads");

    // Later saves are on a file that reads: the copy of the broken one stays as it was.
    let again = ok(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyU)", "save": true}));
    assert!(!again.contains("could not be read"), "{again}");
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), hand_edited);
}

#[test]
fn content_sets_the_defaults_and_an_edit_keeps_the_players_changes() {
    let dir = temp_dir("input_content");
    let controls = |jump: &str| format!(r#"{{ "default": (contexts: [(name: "gameplay", actions: [
        (action: "jump", bindings: [Key({jump})]),
        (action: "sprint", bindings: [Key(ShiftLeft)], mode: Hold),
    ])]) }}"#);
    write(&dir, "content/controls.input_map.ron", &controls("KeyZ"));
    let mut app = engine_app(&dir);
    app.init_resource::<ButtonInput<KeyCode>>().init_resource::<ButtonInput<MouseButton>>();
    frame(&mut app);
    let map = app.world().resource::<InputMap>().clone();
    assert_eq!(map.contexts.len(), 1, "the game's own map, not the built-in one");
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyZ)]);

    ok(&mut app, "rebind", json!({"action": "sprint", "binding": "Key(KeyV)"}));
    write(&dir, "content/controls.input_map.ron", &controls("KeyX"));
    ok(&mut app, "content_reload", json!({}));
    frame(&mut app);
    let map = app.world().resource::<InputMap>().clone();
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyX)], "the new default");
    assert_eq!(map.action("gameplay", "sprint").unwrap().bindings, [Binding::Key(KeyCode::KeyV)], "the player's change stays");

    // Two actions on one key is a content error that names both.
    write(&dir, "content/controls.input_map.ron", r#"{ "default": (contexts: [(name: "gameplay", actions: [
        (action: "jump", bindings: [Key(Space)]), (action: "dodge", bindings: [Key(Space)]) ])]) }"#);
    let out = ok(&mut app, "content_check", json!({}));
    assert!(out.contains("`jump` and `dodge` both read the input of Key(Space)"), "{out}");
}
