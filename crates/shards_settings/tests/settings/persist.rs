//! Settings between runs: the file, when it is written, what a broken one does.

use std::time::Duration;

use bevy::time::TimeUpdateStrategy;
use shards::core::{run_exit_hooks, EngineConfig, ExitHooks, SettingsStore};
use shards::settings::{AccessibilitySettings, AudioSettings, ControlSettings, FlashLevel, GraphicsSettings, Installed, SettingsStorage};
use shards::testing;

use crate::common::*;

/// Changes a value in every group, the way a player does.
fn change_everything(app: &mut shards::bevy::prelude::App) {
    for (group, key, value) in [
        ("graphics", "fullscreen", "true"),
        ("graphics", "vsync", "false"),
        ("graphics", "resolution", "1920x1080"),
        ("graphics", "render_scale", "1.5"),
        ("graphics", "shadows", "low"),
        ("audio", "master", "0.5"),
        ("audio", "music", "0.25"),
        ("audio", "sfx", "0.75"),
        ("audio", "voice", "0.1"),
        ("controls", "mouse_sensitivity", "2.5"),
        ("controls", "invert_y", "true"),
        ("controls", "bindings.gameplay.jump", "[\"Key(KeyJ)\", \"GamepadButton(South)\"]"),
        ("controls", "hold_or_toggle.gameplay.block", "toggle"),
        ("accessibility", "camera_shake", "0.3"),
        ("accessibility", "motion_blur", "0.2"),
        ("accessibility", "flashes", "reduced"),
        ("accessibility", "text_scale", "1.5"),
        ("accessibility", "fov_deg", "95"),
        ("accessibility", "hold_to_toggle", "true"),
    ] {
        set(app, group, key, value);
    }
}

#[test]
fn every_group_survives_a_restart() {
    let dir = TempDir::new("restart");
    let mut app = game_in(&dir);
    let (graphics, audio, controls, accessibility) = {
        let w = app.world();
        (w.resource::<GraphicsSettings>().clone(), w.resource::<AudioSettings>().clone(), w.resource::<ControlSettings>().clone(), w.resource::<AccessibilitySettings>().clone())
    };
    assert_eq!((graphics, audio, controls, accessibility), Default::default(), "a fresh folder: the defaults");

    change_everything(&mut app);
    shards::settings::flush(app.world_mut());
    let text = std::fs::read_to_string(dir.file()).unwrap();
    for table in ["[graphics]", "[audio]", "[accessibility]", "[controls"] {
        assert!(text.contains(table), "{table} in the file:\n{text}");
    }
    assert!(text.contains("render_scale = 1.5") && text.contains("flashes = \"reduced\"") && text.contains("Key(KeyJ)"), "{text}");
    let expected = {
        let w = app.world();
        (w.resource::<GraphicsSettings>().clone(), w.resource::<AudioSettings>().clone(), w.resource::<ControlSettings>().clone(), w.resource::<AccessibilitySettings>().clone())
    };
    drop(app);

    // A second run, a second app: the same values from the file alone.
    let again = game_in(&dir);
    let w = again.world();
    let got = (w.resource::<GraphicsSettings>().clone(), w.resource::<AudioSettings>().clone(), w.resource::<ControlSettings>().clone(), w.resource::<AccessibilitySettings>().clone());
    assert_eq!(got, expected);
    assert_eq!(got.3.flash_level(), FlashLevel::Reduced);
    assert_eq!(got.2.bindings["gameplay.jump"], ["Key(KeyJ)", "GamepadButton(South)"]);
    assert_eq!(got.2.hold_or_toggle["gameplay.block"], "toggle");
}

#[test]
fn a_run_that_changed_nothing_writes_nothing() {
    let dir = TempDir::new("quiet");
    let mut app = game_in(&dir);
    assert!(dir.file().exists(), "the folder is made at the start, with an empty file Bevy reads as the defaults");
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "");
    shards::settings::flush(app.world_mut());
    app.update();
    shards::settings::flush(app.world_mut());
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "", "nothing changed, nothing written");
}

#[test]
fn the_exit_hook_saves_what_changed() {
    let dir = TempDir::new("exit");
    let mut app = game_in(&dir);
    assert!(app.world().resource::<ExitHooks>().names().any(|n| n == "settings"));
    set(&mut app, "accessibility", "fov_deg", "100");
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "", "not yet: a second after the last change, or at exit");
    run_exit_hooks(app.world_mut());
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("fov_deg = 100.0"));
}

#[test]
fn a_change_is_written_a_second_later() {
    let dir = TempDir::new("deferred");
    let mut app = game_in(&dir);
    // Time moves 1.2 s per update, as it would in a window.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(1200)));
    app.update();
    set(&mut app, "audio", "master", "0.4");
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "");
    app.update();
    app.update();
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("master = 0.4"), "written by itself");
}

#[test]
fn without_a_store_nothing_is_read_or_written() {
    let mut app = game();
    assert!(app.world().get_resource::<SettingsStorage>().is_none());
    set(&mut app, "audio", "master", "0.4");
    assert_eq!(app.world().resource::<AudioSettings>().master, 0.4, "the change holds for this run");
    shards::settings::flush(app.world_mut());
    let out = ok(&mut app, "settings", serde_json::json!({}));
    assert!(out.contains("not saved"), "{out}");

    // `Default` (the platform's folder) is a store for a window only: a headless game, an agent's playtest,
    // neither reads nor changes the player's own settings.
    let mut bare = shards::bevy::prelude::App::new();
    let config = EngineConfig { settings: SettingsStore::Default, headless: true, ..Default::default() };
    assert_eq!(shards::settings::install(&mut bare, &config), Ok(Installed::No));
    let embedded = EngineConfig { settings: SettingsStore::Default, embedded: true, ..Default::default() };
    assert_eq!(shards::settings::install(&mut bare, &embedded), Ok(Installed::No));
    assert_eq!(shards::settings::install(&mut bare, &EngineConfig { settings: SettingsStore::Off, ..Default::default() }), Ok(Installed::No));
}

#[test]
fn an_absolute_folder_is_the_folder() {
    let dir = TempDir::new("absolute");
    let nested = dir.path().join("a/b");
    let mut app = game_with(SettingsStore::Dir(nested.display().to_string()), |_| {});
    let storage = app.world().resource::<SettingsStorage>().clone();
    assert_eq!(storage.file, Some(nested.join("settings.toml")));
    set(&mut app, "audio", "sfx", "0.5");
    shards::settings::flush(app.world_mut());
    assert!(nested.join("settings.toml").exists(), "not under the platform's settings folder, which Bevy would join the name to");
}

#[test]
fn a_headless_game_with_a_settings_folder_works_without_a_window() {
    let dir = TempDir::new("headless");
    let mut app = game_in(&dir);
    let windows = app.world_mut().query::<&shards::bevy::window::Window>().iter(app.world()).count();
    assert_eq!(windows, 0, "no window");
    set(&mut app, "graphics", "fullscreen", "true");
    app.update();
    testing::ok(&mut app, "step", serde_json::json!({"ticks": 5}));
    shards::settings::flush(app.world_mut());
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("fullscreen = true"), "the value is kept, nothing to apply it to");
}

#[test]
fn a_file_that_is_not_toml_is_the_defaults_and_the_next_save_replaces_it() {
    let dir = TempDir::new("broken");
    std::fs::write(dir.file(), "[graphics\nthis = is not toml").unwrap();
    let mut app = game_in(&dir);
    assert_eq!(*app.world().resource::<GraphicsSettings>(), GraphicsSettings::default());
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert!(text.contains("vsync = false") && !text.contains("not toml"), "{text}");
}

#[test]
fn a_hand_edited_file_is_not_trusted() {
    let dir = TempDir::new("handmade");
    std::fs::write(
        dir.file(),
        r#"
[accessibility]
fov_deg = 400.0
flashes = "strobe"
unknown_key = 3

[audio]
master = 0.5
music = "loud"

[graphics]
resolution = "huge"
render_scale = 40.0
"#,
    )
    .unwrap();
    let mut app = game_in(&dir);
    let w = app.world();
    let (accessibility, audio, graphics) = (w.resource::<AccessibilitySettings>(), w.resource::<AudioSettings>(), w.resource::<GraphicsSettings>());
    // Missing keys keep their defaults, a key that does not fit is skipped, unknown keys are ignored.
    assert_eq!(audio.master, 0.5);
    assert_eq!(audio.music, AudioSettings::default().music);
    assert_eq!(accessibility.text_scale, AccessibilitySettings::default().text_scale);
    // Whatever is there, what is used is in range.
    assert_eq!((accessibility.fov(), accessibility.flash_level()), (120.0, FlashLevel::Full));
    assert_eq!((graphics.window_size(), graphics.scale()), (None, 2.0));
    app.update();
    let camera = app.world().resource::<shards::motion::CameraSettings>();
    assert_eq!(camera.fov_deg, 120.0, "the camera gets the clamped one");
}
