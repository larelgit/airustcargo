//! The input action map: devices (fed to the `ButtonInput` resources and `Gamepad`
//! components by hand, as Bevy's input plugin would) become `Intent`.

use std::path::Path;

use ai_core::{AiCorePlugin, AiJournal, AiKind, JournalKind, SimConfig};
use bevy::input::gamepad::GamepadInput;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{DODGE, GLIDE, JUMP, SPRINT};
use shards_core::{EngineConfig, Intent, LocalPlayer, Order, SettingsStore, Target};
use shards_motion::input::ActionBinding;
use shards_motion::*;
use shards_settings::{AccessibilitySettings, ControlSettings, SettingsStorage};

use crate::engine::*;

/// The input map on a bare app. `legacy` says where the pre-settings `input.ron` is; `store`
/// keeps the settings in that folder (`<dir>/settings.toml`), as `shards::run` would.
fn input_app_in(store: Option<&Path>, legacy: Option<InputSettings>) -> App {
    input_app_full(store, legacy, |_| {})
}

/// [`input_app_in`], and `after_install` gets the app between the store starting and the first frame.
fn input_app_full(store: Option<&Path>, legacy: Option<InputSettings>, after_install: impl FnOnce(&mut App)) -> App {
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
    if let Some(legacy) = legacy {
        app.insert_resource(legacy);
    }
    if let Some(dir) = store {
        let config = EngineConfig { settings: SettingsStore::Dir(dir.display().to_string()), ..default() };
        shards_settings::install(&mut app, &config).unwrap();
    }
    after_install(&mut app);
    app.finish();
    app.cleanup();
    app.update();
    app
}

fn input_app_with(legacy: Option<InputSettings>) -> App {
    input_app_in(None, legacy)
}

fn input_app() -> App {
    input_app_with(None)
}

fn journal_has(app: &App, kind: JournalKind, needle: &str) -> bool {
    app.world().resource::<AiJournal>().after(0).0.any(|e| e.kind == kind && e.text.contains(needle))
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
fn the_move_axis_is_turned_by_the_stored_yaw_not_by_a_shake() {
    // Shake moves the camera's picture (its `Transform`), never `ThirdPersonCamera::yaw`, and
    // gameplay reads only the yaw: a hit that shakes the view does not bend the walk.
    let mut app = input_app();
    let e = local_player(&mut app);
    let yaw = 0.7;
    let camera = app.world_mut().spawn((ThirdPersonCamera { yaw, ..default() }, Transform::from_rotation(Quat::from_rotation_y(yaw + 0.3)))).id();
    app.world_mut().resource_mut::<shards_motion::camera::CameraShake>().add_trauma(1.0);
    press(&mut app, KeyCode::KeyW);
    for _ in 0..5 {
        frame(&mut app);
    }
    let want = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
    let dir = intent(&app, e).move_dir;
    assert!((dir - want).length() < 1e-5, "the stored yaw {yaw} turns the walk, not the jittered view: {dir} vs {want}");
    assert!((app.world().get::<ThirdPersonCamera>(camera).unwrap().yaw - yaw).abs() < 1e-6);
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
    // A flick is measured in the raw pixels, whatever the binding scales them to.
    assert_eq!(app.world().resource::<ControlState>().look_pointer_px(), Vec2::new(40.0, 20.0));
    assert_eq!(app.world().resource::<ControlState>().look_stick(), Vec2::ZERO);
    app.insert_resource(AccumulatedMouseMotion::default());
    frame(&mut app);
    assert_eq!(app.world().resource::<ControlState>().axis("look"), Vec2::ZERO);
    assert_eq!(app.world().resource::<ControlState>().look_pointer_px(), Vec2::ZERO);
}

#[test]
fn the_look_stick_is_reported_as_a_deflection_before_its_scale_and_the_frame() {
    let mut app = input_app();
    local_player(&mut app);
    let pad = app.world_mut().spawn(Gamepad::default()).id();
    app.world_mut().get_mut::<Gamepad>(pad).unwrap().analog_mut().set(GamepadInput::Axis(GamepadAxis::RightStickX), 0.9);
    frame(&mut app);
    let state = app.world().resource::<ControlState>();
    // Past the 0.15 dead zone, rescaled: (0.9 - 0.15) / 0.85; the axis itself is that times the 2.8 rad/s and the frame.
    let want = (0.9 - 0.15) / 0.85;
    assert!((state.look_stick().x - want).abs() < 1e-4, "{}", state.look_stick());
    assert!(state.axis("look").x.abs() < 2.8 * want, "the axis is a per-frame turn, not the deflection");
    assert_eq!(state.look_pointer_px(), Vec2::ZERO);
}

#[test]
fn an_inverted_look_stick_keeps_its_flipped_side_in_the_flick() {
    let mut app = input_app();
    local_player(&mut app);
    app.world_mut()
        .resource_mut::<InputMap>()
        .rebind("gameplay", "look", 1, Binding::Stick { stick: shards_motion::input::Stick::Right, deadzone: 0.15, scale: -2.8 })
        .unwrap();
    let pad = app.world_mut().spawn(Gamepad::default()).id();
    app.world_mut().get_mut::<Gamepad>(pad).unwrap().analog_mut().set(GamepadInput::Axis(GamepadAxis::RightStickX), 0.9);
    frame(&mut app);
    let state = app.world().resource::<ControlState>();
    assert!(state.look_stick().x < 0.0, "the look axis turns left, so does the flick: {}", state.look_stick());
    assert!(state.axis("look").x < 0.0);
    assert!((state.look_stick().x.abs() - (0.9 - 0.15) / 0.85).abs() < 1e-4);
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
fn the_players_changes_are_kept_in_the_controls_settings_and_read_back() {
    let dir = temp_dir("input_save");
    let file = dir.join("settings.toml");

    let mut app = input_app_in(Some(&dir), None);
    let out = ok(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyJ)"}));
    assert!(out.contains("saved to") && out.contains("settings.toml"), "{out}");
    ok(&mut app, "rebind", json!({"action": "sprint", "mode": "toggle"}));
    {
        let controls = app.world().resource::<ControlSettings>();
        assert_eq!(controls.bindings["gameplay.jump"], ["Key(KeyJ)", "GamepadButton(South)"], "the slots of the action, the gamepad one as it was");
        assert_eq!(controls.hold_or_toggle["gameplay.sprint"], "toggle");
        assert_eq!(controls.bindings.len(), 1, "only what changed");
    }
    // Written a second after the last change (here: when the game exits).
    shards_settings::flush(app.world_mut());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.contains("[controls") && text.contains("gameplay.jump") && text.contains("KeyJ") && text.contains("toggle"), "{text}");
    assert!(!text.contains("dodge"), "only what changed is saved, so new default actions still reach old files:\n{text}");

    // A new run reads them back.
    let mut next = input_app_in(Some(&dir), None);
    let map = next.world().resource::<InputMap>().clone();
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings[0], Binding::Key(KeyCode::KeyJ));
    assert_eq!(map.action("gameplay", "sprint").unwrap().mode, BindMode::Toggle);
    assert_eq!(map.action("gameplay", "dodge"), InputMap::standard().action("gameplay", "dodge"));
    let e = local_player(&mut next);
    frame(&mut next);
    tap(&mut next, KeyCode::KeyJ);
    assert_eq!(pressed_names(&next, e), [JUMP]);

    // Nothing changed in that run: it writes nothing (the file stays as it was).
    let before = std::fs::metadata(&file).unwrap().modified().unwrap();
    shards_settings::flush(next.world_mut());
    assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), before);
}

#[test]
fn a_settings_file_that_does_not_read_or_does_not_fit_is_the_defaults_and_a_warning() {
    // Not TOML at all: Bevy logs it and starts from the defaults.
    let dir = temp_dir("input_broken");
    std::fs::write(dir.join("settings.toml"), "[controls\nthis is not toml").unwrap();
    let app = input_app_in(Some(&dir), None);
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard());

    // TOML that names things the controls do not have: each is named, the rest applies.
    let dir = temp_dir("input_stale");
    std::fs::write(
        dir.join("settings.toml"),
        "[controls.bindings]\n\"gameplay.fly\" = [\"Key(KeyF)\"]\n\"gameplay.jump\" = [\"Space\"]\n\"gameplay.light\" = [\"Key(KeyL)\"]\n",
    )
    .unwrap();
    let mut app = input_app_in(Some(&dir), None);
    let map = app.world().resource::<InputMap>().clone();
    assert_eq!(map.action("gameplay", "light").unwrap().bindings, [Binding::Key(KeyCode::KeyL)], "the entry that fits applies");
    assert_eq!(map.action("gameplay", "jump"), InputMap::standard().action("gameplay", "jump"));
    assert!(journal_has(&app, JournalKind::Warn, "controls.bindings.gameplay.fly") && journal_has(&app, JournalKind::Warn, "controls.bindings.gameplay.jump"));
    // Saving something else keeps what this version cannot read.
    ok(&mut app, "rebind", json!({"action": "dodge", "binding": "Key(KeyV)"}));
    assert!(app.world().resource::<ControlSettings>().bindings.contains_key("gameplay.fly"));
    // ...and a binding the player wrote that does not parse is still what they wrote, not erased.
    assert_eq!(app.world().resource::<ControlSettings>().bindings["gameplay.jump"], ["Space"]);
    shards_settings::flush(app.world_mut());
    let saved = std::fs::read_to_string(dir.join("settings.toml")).unwrap();
    assert!(saved.contains("\"Space\"") && saved.contains("KeyV"), "{saved}");
}

#[test]
fn the_overrides_file_of_earlier_versions_is_imported_once() {
    let dir = temp_dir("input_legacy");
    let old = dir.join("config/input.ron");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, r#"(contexts: [(name: "gameplay", actions: [(action: "jump", bindings: [Key(KeyJ)]), (action: "sprint", bindings: [Key(ShiftLeft)], mode: Toggle)])])"#).unwrap();
    let legacy = || InputSettings { file: Some(old.clone()), ..default() };
    let settings = dir.join("settings.toml");

    let app = input_app_in(Some(&dir), Some(legacy()));
    let map = app.world().resource::<InputMap>().clone();
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
    assert_eq!(map.action("gameplay", "sprint").unwrap().mode, BindMode::Toggle);
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains("gameplay.jump") && text.contains("KeyJ"), "the settings file has them already:\n{text}");
    assert!(!old.exists() && old.with_extension("ron.imported").exists(), "the old file goes aside");
    assert!(journal_has(&app, JournalKind::Event, "imported the controls"));

    // The next run reads the settings, and the old file is not looked for again.
    std::fs::write(&old, r#"(contexts: [(name: "gameplay", actions: [(action: "jump", bindings: [Key(KeyZ)])])])"#).unwrap();
    let next = input_app_in(Some(&dir), Some(legacy()));
    assert_eq!(next.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
    assert!(old.exists(), "settings that hold controls already are not overwritten by an old file");
}

#[test]
fn an_explicit_old_file_is_read_without_a_settings_store_and_left_alone() {
    let dir = temp_dir("input_legacy_memory");
    let old = dir.join("input.ron");
    let text = r#"(contexts: [(name: "gameplay", actions: [(action: "jump", bindings: [Key(KeyJ)])])])"#;
    std::fs::write(&old, text).unwrap();
    let app = input_app_with(Some(InputSettings { file: Some(old.clone()), ..default() }));
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
    assert_eq!(std::fs::read_to_string(&old).unwrap(), text, "nowhere to put it: the file stays");

    // One that cannot be read is a warning and the defaults, and stays too.
    let broken = dir.join("broken.ron");
    std::fs::write(&broken, "(contexts: [oops").unwrap();
    let app = input_app_in(Some(&dir), Some(InputSettings { file: Some(broken.clone()), ..default() }));
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard());
    assert!(journal_has(&app, JournalKind::Warn, "using the default controls"));
    assert!(broken.exists());
}

const OLD_JUMP: &str = r#"(contexts: [(name: "gameplay", actions: [(action: "jump", bindings: [Key(KeyJ)])])])"#;

#[test]
fn a_game_that_did_not_ask_never_touches_the_shared_config_folder() {
    // `shards/input.ron` is the sandbox's: a game with the default `InputSettings` leaves it, with a
    // store of the platform's or of a folder somebody named.
    let dir = temp_dir("input_shared");
    let shared = dir.join("config/shards/input.ron");
    std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
    std::fs::write(&shared, OLD_JUMP).unwrap();
    for platform in [false, true] {
        let legacy = InputSettings { config_dir: Some(dir.join("config")), ..default() };
        let app = input_app_full(Some(&dir.join("store")), Some(legacy), |app| {
            app.world_mut().resource_mut::<SettingsStorage>().platform_default = platform;
        });
        assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard(), "platform folder: {platform}");
        assert!(shared.exists() && !shared.with_extension("ron.imported").exists(), "not renamed");
        assert!(!journal_has(&app, JournalKind::Event, "imported"));
    }
}

#[test]
fn a_game_that_asked_imports_from_its_own_folder_in_the_platforms_settings_only() {
    let dir = temp_dir("input_asked");
    let old = dir.join("config/my-game/input.ron");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, OLD_JUMP).unwrap();
    let asked = || InputSettings { config_dir: Some(dir.join("config")), ..InputSettings::for_game("my-game") };

    // `--settings-dir`, a test, a headless run: not the user's folder, so not read.
    let app = input_app_in(Some(&dir.join("named")), Some(asked()));
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard());
    assert!(old.exists());

    // The platform's own settings folder: read, written, and only then renamed.
    let app = input_app_full(Some(&dir.join("platform")), Some(asked()), |app| {
        app.world_mut().resource_mut::<SettingsStorage>().platform_default = true;
    });
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
    assert!(std::fs::read_to_string(dir.join("platform/settings.toml")).unwrap().contains("gameplay.jump"));
    assert!(!old.exists() && old.with_extension("ron.imported").exists());
}

#[test]
fn the_old_file_stays_when_the_settings_could_not_be_written() {
    let dir = temp_dir("input_unwritable");
    let old = dir.join("input.ron");
    std::fs::write(&old, OLD_JUMP).unwrap();
    // A settings folder below a regular file cannot be made, even by root.
    let blocker = dir.join("not_a_folder");
    std::fs::write(&blocker, "file").unwrap();
    let legacy = || InputSettings { file: Some(old.clone()), ..default() };

    let app = input_app_in(Some(&blocker.join("settings")), Some(legacy()));
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)], "this session has them");
    assert!(old.exists() && !old.with_extension("ron.imported").exists(), "but the old file did not go aside");
    assert!(journal_has(&app, JournalKind::Warn, "could not write the imported controls"));
    assert!(!journal_has(&app, JournalKind::Event, "imported the controls"));

    // A run that can write finds it again.
    let next = input_app_in(Some(&dir.join("writable")), Some(legacy()));
    assert_eq!(next.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
    assert!(!old.exists() && old.with_extension("ron.imported").exists());
    assert!(std::fs::read_to_string(dir.join("writable/settings.toml")).unwrap().contains("gameplay.jump"));
}

#[test]
fn a_change_of_the_look_speed_does_not_touch_what_a_game_added_to_the_map() {
    let mut app = input_app();
    local_player(&mut app);
    frame(&mut app);
    // A game's own context and action, added by code after the first resolve.
    {
        let mut map = app.world_mut().resource_mut::<InputMap>();
        map.contexts.push(InputContext { name: "aim".into(), actions: vec![ActionBinding::new("fire", [Binding::Key(KeyCode::KeyG)])], drives_intent: true });
        map.contexts[0].actions.push(ActionBinding::new("emote", [Binding::Key(KeyCode::KeyB)]));
    }
    frame(&mut app);
    // The settings change: not the controls (the look speed), then the controls.
    set_control(&mut app, "mouse_sensitivity", "2");
    frame(&mut app);
    ok(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyJ)"}));
    set_control(&mut app, "bindings.gameplay.dodge", "[\"Key(KeyV)\"]");
    frame(&mut app);
    let map = app.world().resource::<InputMap>();
    assert_eq!(map.action("aim", "fire").unwrap().bindings, [Binding::Key(KeyCode::KeyG)], "the game's context is still there");
    assert!(map.action("gameplay", "emote").is_some(), "and its action");
    assert_eq!(map.action("gameplay", "dodge").unwrap().bindings, [Binding::Key(KeyCode::KeyV)], "the new binding applied");

    // `rebind` on what is not a default control says it lasts until the game ends, and writes nothing.
    let dir = temp_dir("input_extras");
    let mut kept = input_app_in(Some(&dir), None);
    kept.world_mut().resource_mut::<InputMap>().contexts[0].actions.push(ActionBinding::new("emote", [Binding::Key(KeyCode::KeyB)]));
    frame(&mut kept);
    let out = ok(&mut kept, "rebind", json!({"action": "emote", "binding": "Key(KeyN)"}));
    assert!(out.contains("not saved") && out.contains("gameplay.emote"), "{out}");
    assert!(kept.world().resource::<ControlSettings>().bindings.is_empty());
    assert!(ok(&mut kept, "rebind", json!({"action": "jump", "binding": "Key(KeyJ)"})).contains("saved to"));
}

fn set_control(app: &mut App, key: &str, value: &str) -> String {
    ok(app, "settings_set", json!({"group": "controls", "key": key, "value": value}))
}

#[test]
fn settings_set_refuses_what_rebind_refuses() {
    let mut app = input_app();
    local_player(&mut app);
    frame(&mut app);
    let refuses = |app: &mut App, key: &str, value: &str, needles: &[&str]| {
        let err = fails(app, "settings_set", json!({"group": "controls", "key": key, "value": value}));
        for needle in needles {
            assert!(err.contains(needle), "`{needle}` in `{err}` ({key}={value})");
        }
    };
    // An action that does not exist, a context that does not, a typo in the mode's action.
    refuses(&mut app, "bindings.gameplay.jmp", "[\"Key(KeyJ)\"]", &["controls.bindings.gameplay.jmp", "no action `jmp` in context `gameplay`", "did you mean `jump`", "actions: "]);
    refuses(&mut app, "bindings.Gameplay.jump", "[\"Key(KeyJ)\"]", &["no input context `Gameplay`", "did you mean `gameplay`", "contexts: gameplay, menu"]);
    refuses(&mut app, "hold_or_toggle.gameplay.sprnt", "toggle", &["controls.hold_or_toggle.gameplay.sprnt", "no action `sprnt`", "did you mean `sprint`"]);
    // An input another action of the context reads: W is inside the move binding, E is `interact`.
    // The way out is named for the caller of `settings_set` (a player's agent has no `rebind`), the dev route after it.
    refuses(
        &mut app,
        "bindings.gameplay.jump",
        "[\"Key(KeyW)\"]",
        &["already bound to `move` in `gameplay`", "free it first: settings_set group=controls key=bindings.gameplay.move", "a dev session can take it over with `rebind context=gameplay action=jump binding='Key(KeyW)' force=true`"],
    );
    refuses(&mut app, "bindings.gameplay.jump", "[\"Key(KeyE)\"]", &["already bound to `interact`"]);
    assert_eq!(*app.world().resource::<ControlSettings>(), ControlSettings::default(), "nothing of it got in");
    frame(&mut app);
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard());

    // What is free works, and so does taking an input by freeing it first.
    set_control(&mut app, "bindings.gameplay.interact", "[\"Key(KeyG)\"]");
    set_control(&mut app, "bindings.gameplay.jump", "[\"Key(KeyE)\"]");
    frame(&mut app);
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyE)]);

    // Taking an input back is a clash too: giving `interact` its default E back would put two actions on E.
    let bindings = |app: &App, key: &str| app.world().resource::<ControlSettings>().bindings.get(key).cloned();
    let err = fails(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.interact", "reset": true}));
    assert!(err.contains("controls.bindings.gameplay.interact") && err.contains("`jump`") && err.contains("Key(KeyE)"), "{err}");
    assert!(err.contains("free it first: settings_set group=controls key=bindings.gameplay.jump"), "{err}");
    assert_eq!(bindings(&app, "gameplay.interact"), Some(vec!["Key(KeyG)".to_owned()]), "nothing changed");
    // A table that leaves the entry out is the same removal.
    let err = fails(&mut app, "settings_set", json!({"group": "controls", "key": "bindings", "value": {"gameplay.jump": ["Key(KeyE)"]}}));
    assert!(err.contains("controls.bindings.gameplay.interact") && err.contains("`jump`"), "{err}");
    assert_eq!(bindings(&app, "gameplay.interact"), Some(vec!["Key(KeyG)".to_owned()]), "nothing changed");
    // Freeing the input first lets it through: the map is the defaults again.
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.jump", "reset": true}));
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.interact", "reset": true}));
    frame(&mut app);
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard());

    // The same when the other action took the input with `rebind … force`: dodge takes Space from jump.
    ok(&mut app, "rebind", json!({"action": "dodge", "binding": "Key(Space)", "force": true}));
    let err = fails(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.jump", "reset": true}));
    assert!(err.contains("controls.bindings.gameplay.jump") && err.contains("`dodge`") && err.contains("Key(Space)"), "{err}");
    // A reset of everything is always allowed: it is the defaults.
    ok(&mut app, "settings_set", json!({"group": "controls", "reset": true}));
    frame(&mut app);
    assert_eq!(*app.world().resource::<InputMap>(), InputMap::standard());
}

#[test]
fn a_reset_is_not_refused_for_a_clash_the_settings_already_had() {
    // A hand-edited file has `interact` on E (its default, spelled out) and `jump` on E too. Resetting
    // `interact` gives it E, which `jump` reads: the same two actions that shared it before, nothing new.
    let dir = temp_dir("input_reset_old_clash");
    std::fs::write(dir.join("settings.toml"), "[controls.bindings]\n\"gameplay.interact\" = [\"Key(KeyE)\"]\n\"gameplay.jump\" = [\"Key(KeyE)\"]\n").unwrap();
    let mut app = input_app_in(Some(&dir), None);
    local_player(&mut app);
    frame(&mut app);
    assert!(journal_has(&app, JournalKind::Warn, "both read"), "the file's clash is said at the start");
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.interact", "reset": true}));
    let c = app.world().resource::<ControlSettings>();
    assert!(!c.bindings.contains_key("gameplay.interact") && c.bindings.contains_key("gameplay.jump"));
}

#[test]
fn settings_set_judges_the_map_in_play_with_what_the_game_added() {
    let mut app = input_app();
    local_player(&mut app);
    frame(&mut app);
    // A game's own context and action, added by code: not settings, but they read inputs.
    {
        let mut map = app.world_mut().resource_mut::<InputMap>();
        map.contexts.push(InputContext { name: "aim".into(), actions: vec![ActionBinding::new("fire", [Binding::Key(KeyCode::KeyH)])], drives_intent: true });
        map.contexts[0].actions.push(ActionBinding::new("emote", [Binding::Key(KeyCode::KeyG)]));
    }
    frame(&mut app);
    // An input an added action reads is taken, for `settings_set` as for `rebind`.
    let err = fails(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.jump", "value": "[\"Key(KeyG)\"]"}));
    assert!(err.contains("already bound to `emote` in `gameplay`"), "{err}");
    // `settings_set` can never move `emote`, so the refusal must not send the caller there; it names the dev route instead.
    assert!(!err.contains("free it first: settings_set"), "{err}");
    assert!(err.contains("a control the game adds itself") && err.contains("pick another input") && err.contains("force=true"), "{err}");
    assert!(fails(&mut app, "rebind", json!({"action": "jump", "binding": "Key(KeyG)"})).contains("emote"));
    // An entry for a control the game added is not a setting; the refusal says so instead of saying it does not exist,
    // when `input_bindings` lists it.
    for (key, value) in [("bindings.aim.fire", "[\"Key(KeyJ)\"]"), ("bindings.gameplay.emote", "[\"Key(KeyJ)\"]"), ("hold_or_toggle.aim.fire", "toggle")] {
        let err = fails(&mut app, "settings_set", json!({"group": "controls", "key": key, "value": value}));
        assert!(err.contains("a control the game adds itself") && !err.contains("no input context") && !err.contains("no action"), "{key}: {err}");
    }
    assert!(ok(&mut app, "input_bindings", json!({"context": "aim"})).contains("fire"));
    // A context or action nobody has is still unknown, and the list is the one `input_bindings` gives.
    let err = fails(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.aimm.fire", "value": "[\"Key(KeyJ)\"]"}));
    assert!(err.contains("no input context `aimm`") && err.contains("did you mean `aim`") && err.contains("contexts: gameplay, menu, aim"), "{err}");
    assert_eq!(*app.world().resource::<ControlSettings>(), ControlSettings::default(), "nothing of it got in");
}

#[test]
fn rebind_and_input_bindings_start_from_the_controls_a_settings_set_just_made() {
    let mut app = input_app();
    local_player(&mut app);
    frame(&mut app);
    // No frame between the calls, as in a scenario or a test: the map has not followed the settings yet.
    set_control(&mut app, "bindings.gameplay.jump", "[\"Key(KeyJ)\"]");
    let listed = ok(&mut app, "input_bindings", json!({}));
    assert!(listed.lines().any(|l| l.trim_start().starts_with("jump") && l.contains("Key(KeyJ)") && !l.contains("Space")), "{listed}");
    ok(&mut app, "rebind", json!({"action": "dodge", "binding": "Key(KeyV)"}));
    assert_eq!(app.world().resource::<ControlSettings>().bindings["gameplay.jump"], ["Key(KeyJ)"], "the setting survived the rebind");
    frame(&mut app);
    let map = app.world().resource::<InputMap>();
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
    assert_eq!(map.action("gameplay", "dodge").unwrap().bindings[0], Binding::Key(KeyCode::KeyV));

    // The other way: a reset, then a rebind of something else, does not bring the reset binding back.
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.jump", "reset": true}));
    ok(&mut app, "rebind", json!({"action": "light", "binding": "Key(KeyL)"}));
    assert!(!app.world().resource::<ControlSettings>().bindings.contains_key("gameplay.jump"), "the reset did not come back");
    frame(&mut app);
    let map = app.world().resource::<InputMap>();
    assert_eq!(map.action("gameplay", "jump"), InputMap::standard().action("gameplay", "jump"));
    assert_eq!(map.action("gameplay", "light").unwrap().bindings[0], Binding::Key(KeyCode::KeyL));
}

#[test]
fn what_a_file_left_behind_does_not_stop_settings_set() {
    // Stale entries (an action this version lacks, a binding that does not read) are named at the start,
    // kept in the file, and no reason to refuse an unrelated change.
    let dir = temp_dir("input_stale_set");
    std::fs::write(
        dir.join("settings.toml"),
        "[controls.bindings]\n\"gameplay.fly\" = [\"Key(KeyF)\"]\n\"gameplay.jump\" = [\"Key(Spce)\", \"GamepadButton(South)\"]\n\
         [controls.hold_or_toggle]\n\"gameplay.block\" = \"tap\"\n",
    )
    .unwrap();
    let mut app = input_app_in(Some(&dir), None);
    local_player(&mut app);
    frame(&mut app);
    assert!(journal_has(&app, JournalKind::Warn, "controls.bindings.gameplay.jump") && journal_has(&app, JournalKind::Warn, "controls.hold_or_toggle.gameplay.block"));
    set_control(&mut app, "bindings.gameplay.light", "[\"Key(KeyL)\"]");
    ok(&mut app, "rebind", json!({"action": "dodge", "binding": "Key(KeyV)"}));
    // The entries that never applied are still there, so a save does not erase what the player wrote.
    let c = app.world().resource::<ControlSettings>();
    assert_eq!(c.bindings["gameplay.jump"], ["Key(Spce)", "GamepadButton(South)"]);
    assert_eq!(c.bindings["gameplay.fly"], ["Key(KeyF)"]);
    assert_eq!(c.bindings["gameplay.light"], ["Key(KeyL)"], "and so is what settings_set just set, though the map had not followed it when `rebind` ran");
    assert_eq!(c.hold_or_toggle["gameplay.block"], "tap");
    // Fixing one is setting it; the check judges that alone.
    set_control(&mut app, "bindings.gameplay.jump", "[\"Key(KeyJ)\"]");
    frame(&mut app);
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyJ)]);
}

#[test]
fn a_clash_in_a_file_is_said() {
    let dir = temp_dir("input_clash");
    std::fs::write(dir.join("settings.toml"), "[controls.bindings]\n\"gameplay.jump\" = [\"Key(KeyW)\"]\n").unwrap();
    let app = input_app_in(Some(&dir), None);
    let said = app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Warn && e.text.contains("both read")).count();
    assert_eq!(said, 1, "named once");
    let text = app.world().resource::<AiJournal>().after(0).0.find(|e| e.text.contains("both read")).map(|e| e.text.clone()).unwrap();
    assert!(text.contains("`jump`") && text.contains("`move`") && text.contains("both read") && text.contains("KeyW"), "{text}");
    // It is the player's choice and applies; the engine only says so.
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump").unwrap().bindings, [Binding::Key(KeyCode::KeyW)]);
}

#[test]
fn settings_and_the_map_follow_each_other() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);

    // The map edited directly (a game's own menu) becomes a setting...
    app.world_mut().resource_mut::<InputMap>().rebind("gameplay", "jump", 0, Binding::Key(KeyCode::KeyJ)).unwrap();
    frame(&mut app);
    assert_eq!(app.world().resource::<ControlSettings>().bindings["gameplay.jump"], ["Key(KeyJ)", "GamepadButton(South)"]);

    // ...and a setting changed (`settings_set`, a hand edit) becomes the map.
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.heavy", "value": "[\"Key(KeyH)\"]"}));
    frame(&mut app);
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "heavy").unwrap().bindings, [Binding::Key(KeyCode::KeyH)]);
    tap(&mut app, KeyCode::KeyH);
    assert_eq!(pressed_names(&app, e), [shards_core::input_names::HEAVY]);
    // A binding that does not read is refused, naming it.
    let bad = fails(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.heavy", "value": "[\"Hh\"]"}));
    assert!(bad.contains("controls.bindings.gameplay.heavy") && bad.contains("not a binding"), "{bad}");

    // Resetting a setting puts the default control back.
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.heavy", "reset": true}));
    frame(&mut app);
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "heavy"), InputMap::standard().action("gameplay", "heavy"));
    // The echo of one side is never written back by the other: nothing flips on later frames.
    let settings = app.world().resource::<ControlSettings>().clone();
    for _ in 0..3 {
        frame(&mut app);
    }
    assert_eq!(*app.world().resource::<ControlSettings>(), settings);
}

#[test]
fn hold_to_toggle_turns_held_buttons_into_toggles() {
    let mut app = input_app();
    let e = local_player(&mut app);
    frame(&mut app);
    // As designed: sprint is held while Shift is down.
    press(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(intent(&app, e).is_held(SPRINT));
    release(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(!intent(&app, e).is_held(SPRINT));

    // The accessibility setting: one press holds it, the next lets go.
    app.world_mut().resource_mut::<AccessibilitySettings>().hold_to_toggle = true;
    tap(&mut app, KeyCode::ShiftLeft);
    assert!(intent(&app, e).is_held(SPRINT), "still held after the key came up");
    assert!(ok(&mut app, "input_bindings", json!({})).contains("hold_to_toggle is on"));
    tap(&mut app, KeyCode::ShiftLeft);
    assert!(!intent(&app, e).is_held(SPRINT), "the second press lets go");

    // Off again: back to holding.
    app.world_mut().resource_mut::<AccessibilitySettings>().hold_to_toggle = false;
    press(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(intent(&app, e).is_held(SPRINT));
    release(&mut app, KeyCode::ShiftLeft);
    frame(&mut app);
    assert!(!intent(&app, e).is_held(SPRINT));
}

#[test]
fn the_look_and_accessibility_settings_reach_the_camera() {
    let mut app = input_app();
    assert_eq!(*app.world().resource::<CameraSettings>(), CameraSettings::default(), "the defaults agree");
    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "fov_deg", "value": "95"}));
    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "camera_shake", "value": "0.3"}));
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "mouse_sensitivity", "value": "2"}));
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "invert_y", "value": "true"}));
    app.update();
    let camera = app.world().resource::<CameraSettings>().clone();
    assert_eq!((camera.fov_deg, camera.shake, camera.shake_intensity, camera.sensitivity, camera.invert_y), (95.0, true, 0.3, 2.0, true));
    // No shake at all is off, not "very little".
    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "camera_shake", "value": "0"}));
    app.update();
    assert!(!app.world().resource::<CameraSettings>().shake);
    // What the camera has beyond the player's choices is not touched.
    assert_eq!(app.world().resource::<CameraSettings>().lock_range, CameraSettings::default().lock_range);
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

    // A context name with a dot is a content error too: the player's changes are stored as `context.action`,
    // and `ui.inventory.close` would read back as the action `inventory.close` of a context `ui`.
    write(&dir, "content/controls.input_map.ron", r#"{ "default": (contexts: [(name: "ui.inventory", actions: [
        (action: "close", bindings: [Key(Escape)]) ])]) }"#);
    let out = ok(&mut app, "content_check", json!({}));
    assert!(out.contains("context `ui.inventory`: a context name cannot contain `.`") && out.contains("`ui_inventory`"), "{out}");
}
