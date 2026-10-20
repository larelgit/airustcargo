//! Settings between runs: the file, when it is written, what a broken one does.

use std::time::Duration;

use bevy::time::TimeUpdateStrategy;
use shards::ai_core::JournalKind;
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
        ("controls", "aim_sensitivity", "0.5"),
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
fn a_rebinding_is_kept_and_the_next_run_starts_with_it() {
    use shards::motion::{Binding, InputMap};
    let dir = TempDir::new("rebind");
    let mut app = game_in(&dir);
    let out = ok(&mut app, "rebind", serde_json::json!({"action": "jump", "binding": "Key(KeyJ)"}));
    assert!(out.contains("saved to") && out.contains("settings.toml"), "{out}");
    run_exit_hooks(app.world_mut());
    drop(app);

    let again = game_in(&dir);
    let map = again.world().resource::<InputMap>();
    assert_eq!(map.action("gameplay", "jump").unwrap().bindings[0], Binding::Key(shards::bevy::prelude::KeyCode::KeyJ));
    assert_eq!(map.action("gameplay", "dodge"), InputMap::standard().action("gameplay", "dodge"), "the rest is the defaults");
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
fn a_file_that_is_not_toml_is_kept_aside_before_the_next_save() {
    let dir = TempDir::new("broken");
    let broken = "[graphics\nthis = is not toml";
    std::fs::write(dir.file(), broken).unwrap();
    let mut app = game_in(&dir);
    assert_eq!(*app.world().resource::<GraphicsSettings>(), GraphicsSettings::default(), "no good copy: the defaults");
    let bad = dir.path().join("settings.toml.bad");
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), broken, "the player's text is kept");
    let said = lines(&app, JournalKind::Warn, "did not read");
    assert!(said.len() == 1 && said[0].contains("settings.toml.bad") && said[0].contains("starting from the defaults"), "{said:?}");

    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert!(text.contains("vsync = false") && !text.contains("not toml"), "{text}");
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), broken, "the save did not touch the copy");
    drop(app);

    // Broken again: the first copy is never overwritten.
    let again = "[audio\nmaster = ";
    std::fs::write(dir.file(), again).unwrap();
    let _app = game_in(&dir);
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), broken);
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.toml.bad.1")).unwrap(), again);
}

#[test]
fn a_broken_file_starts_the_game_from_the_last_good_copy() {
    let dir = TempDir::new("backup");
    let mut app = game_in(&dir);
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    assert!(dir.path().join("settings.toml.bak").exists(), "a write that checked out leaves a copy");
    drop(app);

    std::fs::write(dir.file(), "[graphics\noops").unwrap();
    let app = game_in(&dir);
    assert!(!app.world().resource::<GraphicsSettings>().vsync, "the last good settings are back");
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.toml.bad")).unwrap(), "[graphics\noops");
    assert!(lines(&app, JournalKind::Warn, "the last good copy").len() == 1);
}

/// A full disk: Bevy's write fails, and it renames the half-written file over the good one all the same.
#[cfg(target_os = "linux")]
#[test]
fn a_write_that_fails_does_not_cost_the_settings() {
    let dir = TempDir::new("full");
    let mut app = game_in(&dir);
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    // From here on the temporary file is a device that is always full.
    std::os::unix::fs::symlink("/dev/full", dir.path().join("settings.toml.new")).unwrap();
    set(&mut app, "audio", "master", "0.3");
    shards::settings::flush(app.world_mut());

    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert!(text.contains("master = 0.3"), "the write was tried again and worked:\n{text}\n{:?}", lines(&app, JournalKind::Error, "settings:"));
    assert!(std::fs::symlink_metadata(dir.file()).unwrap().file_type().is_file(), "no link, no partial file");
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.toml.bak")).unwrap(), text, "and the copy is what the file holds");
    let failed = lines(&app, JournalKind::Error, "failed");
    assert!(failed.len() == 1 && failed[0].contains("not a regular file") && failed[0].contains("the last good copy"), "{failed:?}");
}

/// What keeps Bevy from writing its temporary file `settings.toml.new` until it is lifted, and from
/// renaming it over the settings. On unix a folder of that name; on Windows a folder would be
/// renamed over the settings file, so the file is held open there, shared with nobody.
struct Blocked {
    path: std::path::PathBuf,
    #[cfg(windows)]
    held: std::fs::File,
}

impl Blocked {
    fn new(dir: &TempDir) -> Self {
        let path = dir.path().join("settings.toml.new");
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            let held = std::fs::OpenOptions::new().read(true).write(true).create_new(true).share_mode(0).open(&path).unwrap();
            Self { path, held }
        }
        #[cfg(not(windows))]
        {
            std::fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    fn lift(self) {
        #[cfg(windows)]
        {
            drop(self.held);
            std::fs::remove_file(&self.path).unwrap();
        }
        #[cfg(not(windows))]
        std::fs::remove_dir(&self.path).unwrap();
    }
}

/// The temporary file cannot be made at all: the good file stays, the journal reports, and it gives up.
#[test]
fn a_write_that_cannot_happen_is_reported_and_given_up() {
    let dir = TempDir::new("blocked");
    let mut app = game_in(&dir);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(4)));
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    let blocked = Blocked::new(&dir);

    set(&mut app, "audio", "master", "0.3");
    for _ in 0..8 {
        app.update();
    }
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("master = 0.4"), "the good file stayed");
    assert!(lines(&app, JournalKind::Error, "was not written").len() >= 3, "each try is reported: {:?}", lines(&app, JournalKind::Error, "settings:"));
    assert_eq!(lines(&app, JournalKind::Error, "giving up").len(), 1);
    let updates = |app: &mut shards::bevy::prelude::App| (0..4).for_each(|_| app.update());
    updates(&mut app);
    assert_eq!(lines(&app, JournalKind::Error, "giving up").len(), 1, "and it stops");

    // The obstacle goes, and the next change is written again.
    blocked.lift();
    set(&mut app, "audio", "master", "0.2");
    updates(&mut app);
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("master = 0.2"));
}

/// A change the store gave up on is not lost: the exit hook writes it once more, whatever Bevy's
/// marks of what changed say (its "last saved" mark moved on with every try that failed).
#[test]
fn a_change_given_up_on_is_written_when_the_game_exits() {
    let dir = TempDir::new("blocked_exit");
    let mut app = game_in(&dir);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs(4)));
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    let blocked = Blocked::new(&dir);
    set(&mut app, "audio", "master", "0.3");
    for _ in 0..8 {
        app.update();
    }
    assert_eq!(lines(&app, JournalKind::Error, "giving up").len(), 1);
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("master = 0.4"), "the good file stayed");

    // The player frees the disk and quits without touching a setting again.
    blocked.lift();
    run_exit_hooks(app.world_mut());
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("master = 0.3"), "written at exit after all");
    // And it is written once: a second exit has nothing left to do.
    std::fs::remove_file(dir.file()).unwrap();
    run_exit_hooks(app.world_mut());
    assert!(!dir.file().exists(), "nothing was unsaved any more");
}

/// A full disk that stays full: a link to a device that is always full as the temporary file makes
/// Bevy's write fail, and its rename uses the link up, so the link is made again before each try.
#[cfg(target_os = "linux")]
fn fill_the_disk(dir: &TempDir) {
    let _ = std::os::unix::fs::symlink("/dev/full", dir.path().join("settings.toml.new"));
}

/// The way out of a window: the frame with the `AppExit` runs the exit hook and the store's own
/// look in no set order. Whichever comes first, what is on disk afterwards is a file that read.
#[cfg(target_os = "linux")]
#[test]
fn a_write_that_fails_in_the_exit_frame_leaves_the_last_good_copy_in_place() {
    let dir = TempDir::new("full_exit_frame");
    let mut app = game_in(&dir);
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    app.update();
    fill_the_disk(&dir);
    set(&mut app, "audio", "master", "0.3");
    app.world_mut().write_message(shards::bevy::app::AppExit::Success);
    app.update();

    assert!(std::fs::symlink_metadata(dir.file()).unwrap().file_type().is_file(), "no link, no partial file");
    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert_eq!(text, std::fs::read_to_string(dir.path().join("settings.toml.bak")).unwrap(), "the file is the last copy that read");
    assert!(text.contains("master = 0.3"), "and the change was written by the retry:\n{text}\n{:?}", lines(&app, JournalKind::Error, "settings:"));
}

/// A write that leaves a bad file puts the good one back and waits before the next try, as the
/// manual (docs/manual.md) says; when the tries are over the exit hook writes the change once more.
#[cfg(target_os = "linux")]
#[test]
fn a_write_that_leaves_a_bad_file_is_tried_again_a_pause_later() {
    let dir = TempDir::new("pause");
    let mut app = game_in(&dir);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(1200)));
    app.update();
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    // A frame between the writes: Bevy tells a change from the last write by ticks, and one made
    // within the tick of a write is missed (the store catches that case on its own, later).
    app.update();
    let stored = |dir: &TempDir| std::fs::read_to_string(dir.file()).unwrap();
    let failed = |app: &shards::bevy::prelude::App| lines(app, JournalKind::Error, "failed").len();

    fill_the_disk(&dir);
    set(&mut app, "audio", "master", "0.3");
    app.update();
    assert_eq!(failed(&app), 1, "the debounce is over: the write was tried and failed");
    assert!(stored(&dir).contains("master = 0.4"), "the good file is back, and the change is not put on it in the same frame");
    // 1.2 s and 2.4 s later it is not time yet.
    for _ in 0..2 {
        fill_the_disk(&dir);
        app.update();
    }
    assert!(stored(&dir).contains("master = 0.4") && failed(&app) == 1, "no second try yet");
    // 3.6 s later it is, and the disk is still full.
    fill_the_disk(&dir);
    app.update();
    assert_eq!(failed(&app), 2, "tried again after the pause: {:?}", lines(&app, JournalKind::Error, "settings:"));
    assert!(stored(&dir).contains("master = 0.4"));

    // Two more tries, then the journal gives up, once.
    for _ in 0..9 {
        fill_the_disk(&dir);
        app.update();
    }
    assert_eq!((failed(&app), lines(&app, JournalKind::Error, "giving up").len()), (3, 1), "{:?}", lines(&app, JournalKind::Error, "settings:"));
    assert!(stored(&dir).contains("master = 0.4"));

    // The disk has room again, and the player quits.
    let _ = std::fs::remove_file(dir.path().join("settings.toml.new"));
    run_exit_hooks(app.world_mut());
    assert!(stored(&dir).contains("master = 0.3"), "the change is written at exit after all:\n{}", stored(&dir));
}

#[test]
fn a_hand_edit_made_while_the_game_runs_is_not_taken_for_a_failed_write() {
    let dir = TempDir::new("hand_edit");
    let mut app = game_in(&dir);
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    let good = std::fs::read_to_string(dir.file()).unwrap();

    // The player saves the file in an editor, with a typo.
    let typo = "[graphics\noops";
    std::fs::write(dir.file(), typo).unwrap();
    app.update();
    app.update();
    let bad = dir.path().join("settings.toml.bad");
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), typo, "the player's text is kept, as at the start");
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), good, "the last good copy is in its place");
    let said = lines(&app, JournalKind::Warn, "was edited");
    assert!(said.len() == 1 && said[0].contains("settings.toml.bad") && said[0].contains("the last good copy"), "{said:?}");
    assert!(lines(&app, JournalKind::Error, "settings:").is_empty(), "no write failed: {:?}", lines(&app, JournalKind::Error, "settings:"));
    assert!(!app.world().resource::<GraphicsSettings>().vsync, "the settings in memory are untouched");

    // Closing the game does not lose it either.
    run_exit_hooks(app.world_mut());
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), typo);
    assert!(!dir.path().join("settings.toml.bad.1").exists());
}

#[test]
fn a_valid_hand_edit_made_while_the_game_runs_is_left_alone() {
    let dir = TempDir::new("hand_edit_valid");
    let mut app = game_in(&dir);
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    let edited = "[audio]\nmaster = 0.9\n";
    std::fs::write(dir.file(), edited).unwrap();
    app.update();
    app.update();
    run_exit_hooks(app.world_mut());
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), edited, "nothing changed in the game: nothing is written over it");
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.toml.bak")).unwrap(), edited, "it is the last copy that read");
    assert!(!dir.path().join("settings.toml.bad").exists());
    assert!(lines(&app, JournalKind::Error, "settings:").is_empty() && lines(&app, JournalKind::Warn, "settings:").is_empty());
    drop(app);
    assert_eq!(game_in(&dir).world().resource::<AudioSettings>().master, 0.9, "the next start reads it");
}

#[test]
fn an_emptied_or_deleted_file_is_left_alone_while_the_game_runs() {
    let dir = TempDir::new("hand_edit_empty");
    let mut app = game_in(&dir);
    set(&mut app, "audio", "master", "0.4");
    shards::settings::flush(app.world_mut());
    // An editor that truncates the file before it writes the new text is caught half way.
    std::fs::write(dir.file(), "").unwrap();
    app.update();
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "", "not restored under the editor's feet");
    std::fs::write(dir.file(), "[audio]\nmusic = 0.2\n").unwrap();
    app.update();
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("music = 0.2"));
    std::fs::remove_file(dir.file()).unwrap();
    app.update();
    assert!(!dir.file().exists(), "a deleted file stays deleted until something is written");
    assert!(lines(&app, JournalKind::Error, "settings:").is_empty() && lines(&app, JournalKind::Warn, "settings:").is_empty());
    // A change made after that is written as usual.
    set(&mut app, "audio", "sfx", "0.5");
    run_exit_hooks(app.world_mut());
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("sfx = 0.5"));
}

#[cfg(unix)]
#[test]
fn a_settings_file_that_is_a_link_is_read_through() {
    let dir = TempDir::new("link");
    let real = dir.path().join("dotfiles.toml");
    std::fs::write(&real, "[graphics]\nvsync = false\n").unwrap();
    std::os::unix::fs::symlink(&real, dir.file()).unwrap();
    let app = game_in(&dir);
    assert!(!app.world().resource::<GraphicsSettings>().vsync, "the linked settings are read");
    assert!(lines(&app, JournalKind::Warn, "did not read").is_empty(), "{:?}", lines(&app, JournalKind::Warn, "settings:"));
    assert!(!dir.path().join("settings.toml.bad").exists());
    assert!(std::fs::symlink_metadata(dir.file()).unwrap().file_type().is_symlink(), "the link stays until a save");
    drop(app);

    // A link to what is not a settings file is still moved aside.
    let other = TempDir::new("link_to_a_folder");
    std::os::unix::fs::symlink(other.path(), other.file()).unwrap();
    let app = game_in(&other);
    assert!(other.path().join("settings.toml.bad").exists());
    assert!(lines(&app, JournalKind::Warn, "not a regular file").len() == 1);
}

#[test]
fn an_unusable_entry_that_stays_in_the_file_is_kept_aside_once() {
    let dir = TempDir::new("controls_twice");
    let written = "[controls.bindings]\n\"gameplay.jump\" = [\"Key(KeyJ)\"]\n\"gameplay.heavy\" = 7\n";
    std::fs::write(dir.file(), written).unwrap();
    // Nothing changes in these runs, so nothing is written and the entry stays in the file.
    for start in 1..=3 {
        let mut app = game_in(&dir);
        let said = lines(&app, JournalKind::Warn, "the file as it was is kept as");
        assert!(said.len() == 1 && said[0].ends_with("settings.toml.bad"), "start {start}: {said:?}");
        run_exit_hooks(app.world_mut());
    }
    let bad = dir.path().join("settings.toml.bad");
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), written);
    assert!(!dir.path().join("settings.toml.bad.1").exists(), "one copy of one text");
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), written, "and the file is as the player left it");

    // Another text is another copy.
    let other = "[controls.bindings]\n\"gameplay.heavy\" = 8\n";
    std::fs::write(dir.file(), other).unwrap();
    let _app = game_in(&dir);
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.toml.bad.1")).unwrap(), other);
}

#[test]
fn an_empty_file_where_there_was_a_good_copy_is_a_write_that_went_wrong() {
    let dir = TempDir::new("emptied");
    let mut app = game_in(&dir);
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    drop(app);

    // What a write that failed on a full disk, or a process killed half way, leaves: nothing but,
    // at most, whitespace.
    for emptied in ["", " \n\t\n"] {
        std::fs::write(dir.file(), emptied).unwrap();
        let app = game_in(&dir);
        assert!(!app.world().resource::<GraphicsSettings>().vsync, "the last good settings are back");
        let said = lines(&app, JournalKind::Warn, "the last good copy");
        assert!(said.len() == 1 && said[0].contains("is empty"), "{said:?}");
        assert!(std::fs::read_to_string(dir.file()).unwrap().contains("vsync = false"));
    }
}

/// Every line commented out is the way to ask for the defaults: text, not a write that went wrong.
#[test]
fn a_file_with_only_comments_is_the_defaults_and_stays_as_it_is() {
    let dir = TempDir::new("commented_out");
    let mut app = game_in(&dir);
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    let backup = dir.path().join("settings.toml.bak");
    let good = std::fs::read_to_string(&backup).unwrap();
    drop(app);

    std::fs::write(dir.file(), "# off\n").unwrap();
    let mut app = game_in(&dir);
    assert!(app.world().resource::<GraphicsSettings>().vsync, "the defaults, not the settings of the copy");
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "# off\n", "the player's text is still there");
    assert!(lines(&app, JournalKind::Warn, "settings:").is_empty(), "{:?}", lines(&app, JournalKind::Warn, "settings:"));
    assert!(!dir.path().join("settings.toml.bad").exists());
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), good, "the copy is not replaced by a file without settings");
    // A run that changes nothing writes nothing over it.
    run_exit_hooks(app.world_mut());
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "# off\n");
}

/// The copy is written beside the file and renamed over it; a folder in the name of that file stops
/// it, as a full disk would. The copy stays where it is, and the journal says it did not come back.
#[test]
fn an_empty_file_whose_copy_cannot_be_put_back_is_said() {
    let dir = TempDir::new("emptied_stuck");
    let mut app = game_in(&dir);
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    drop(app);

    std::fs::write(dir.file(), "").unwrap();
    std::fs::create_dir(dir.path().join("settings.toml.restore")).unwrap();
    let app = game_in(&dir);
    assert!(app.world().resource::<GraphicsSettings>().vsync, "the defaults: the copy could not be read in");
    let said = lines(&app, JournalKind::Warn, "settings:");
    let (file, backup) = (dir.file().display().to_string(), dir.path().join("settings.toml.bak").display().to_string());
    assert!(
        said.len() == 1 && said[0].contains(&format!("{file} is empty")) && said[0].contains(&backup) && said[0].contains("could not be put back") && said[0].contains("kept"),
        "{said:?}"
    );
    assert!(std::fs::read_to_string(&backup).unwrap().contains("vsync = false"), "the copy is untouched");
}

#[test]
fn deleting_the_file_resets_the_settings_for_good() {
    let dir = TempDir::new("deleted");
    let mut app = game_in(&dir);
    set(&mut app, "graphics", "vsync", "false");
    shards::settings::flush(app.world_mut());
    drop(app);

    std::fs::remove_file(dir.file()).unwrap();
    // Two starts that change nothing: the empty file of the first must not read as a failed write at the second.
    for start in 1..=2 {
        let app = game_in(&dir);
        assert!(app.world().resource::<GraphicsSettings>().vsync, "start {start}: the defaults");
        assert!(lines(&app, JournalKind::Warn, "settings:").is_empty());
    }
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "");
}

#[test]
fn a_change_a_game_makes_to_a_group_is_written_without_anyone_asking() {
    use shards::prelude::*;

    #[derive(Resource, Reflect, SettingsGroup, Default)]
    #[reflect(Resource, SettingsGroup, Default)]
    #[settings_group(group = "hints")]
    struct Hints {
        shown: u32,
    }

    let dir = TempDir::new("autosave");
    let mut app = game_with(SettingsStore::Dir(dir.path().display().to_string()), |app| {
        app.add_settings_group::<Hints>();
    });
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(1200)));
    app.update();
    // A settings screen of the game: no tool, no flush, no exit hook.
    app.world_mut().resource_mut::<AudioSettings>().master = 0.4;
    app.world_mut().resource_mut::<Hints>().shown = 7;
    assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), "");
    app.update();
    app.update();
    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert!(text.contains("master = 0.4") && text.contains("shown = 7"), "{text}");
    assert_eq!(lines(&app, JournalKind::Event, "settings: audio.master 1 → 0.4").len(), 1);
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

#[test]
fn bad_values_a_hand_edit_left_do_not_stop_other_settings() {
    let dir = TempDir::new("two_bad");
    std::fs::write(dir.file(), "[graphics]\nresolution = \"huge\"\nrender_scale = 40.0\n[accessibility]\nfov_deg = 400.0\nflashes = \"strobe\"\n").unwrap();
    let mut app = game_in(&dir);
    // Fixing one of two bad values, or setting an unrelated one, is that call's business alone.
    let out = set(&mut app, "graphics", "resolution", "auto");
    assert!(out.contains("graphics.resolution \"huge\" → \"auto\""), "{out}");
    assert_eq!(app.world().resource::<GraphicsSettings>().render_scale, 40.0, "the other one is still there, still its owner's to fix");
    set(&mut app, "graphics", "render_scale", "1");
    set(&mut app, "accessibility", "text_scale", "1.2");
    set(&mut app, "graphics", "vsync", "false");
    // What a call sets is still checked.
    let too_big = fails(&mut app, "settings_set", serde_json::json!({"group": "graphics", "key": "render_scale", "value": 50}));
    assert!(too_big.contains("graphics.render_scale 50 is out of range"), "{too_big}");
    let bad_word = fails(&mut app, "settings_set", serde_json::json!({"group": "accessibility", "key": "flashes", "value": "blink"}));
    assert!(bad_word.contains("full, reduced, off"), "{bad_word}");
    // And a bad value that was there is fixed by setting it right, or by a reset of its key.
    set(&mut app, "accessibility", "fov_deg", "90");
    ok(&mut app, "settings_set", serde_json::json!({"group": "accessibility", "key": "flashes", "reset": true}));
    assert_eq!(app.world().resource::<AccessibilitySettings>().check(), Ok(()));
}

#[test]
fn nan_in_a_file_is_the_default_and_is_said() {
    let dir = TempDir::new("nan");
    std::fs::write(dir.file(), "[accessibility]\ncamera_shake = nan\nfov_deg = inf\ntext_scale = 2.0\n").unwrap();
    let mut app = game_in(&dir);
    let a = app.world().resource::<AccessibilitySettings>();
    assert_eq!((a.camera_shake, a.fov_deg, a.text_scale), (1.0, 70.0, 2.0), "the defaults where it was not a number, the rest as it was");
    assert_eq!(lines(&app, JournalKind::Warn, "was not a number").len(), 2);
    let listed = ok(&mut app, "settings", serde_json::json!({"group": "accessibility"}));
    assert!(!listed.contains("null"), "{listed}");
    set(&mut app, "accessibility", "motion_blur", "0.1");
}

#[test]
fn one_bad_entry_in_the_controls_does_not_cost_the_others() {
    let dir = TempDir::new("controls_entries");
    // Unquoted, `gameplay.parry` is a nested table in TOML, which is not a list: Bevy skips the whole table.
    std::fs::write(dir.file(), "[controls.bindings]\n\"gameplay.jump\" = [\"Key(KeyJ)\"]\ngameplay.parry = [\"Key(KeyP)\"]\n").unwrap();
    let mut app = game_in(&dir);
    {
        let c = app.world().resource::<ControlSettings>();
        assert_eq!(c.bindings["gameplay.jump"], ["Key(KeyJ)"]);
        assert_eq!(c.bindings["gameplay.parry"], ["Key(KeyP)"], "read as the dotted key it was meant to be");
    }
    let said = lines(&app, JournalKind::Warn, "controls.bindings.gameplay.parry");
    assert!(said.len() == 1 && said[0].contains("without quotes"), "{said:?}");
    // The next save has all of it, quoted.
    set(&mut app, "audio", "master", "0.5");
    shards::settings::flush(app.world_mut());
    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert!(text.contains("\"gameplay.jump\"") && text.contains("\"gameplay.parry\""), "{text}");
}

#[test]
fn an_entry_that_cannot_be_used_is_named_and_the_file_as_it_was_is_kept() {
    let dir = TempDir::new("controls_dropped");
    let written = "[controls.bindings]\n\"gameplay.jump\" = [\"Key(KeyJ)\"]\n\"gameplay.heavy\" = 7\n";
    std::fs::write(dir.file(), written).unwrap();
    let mut app = game_in(&dir);
    let c = app.world().resource::<ControlSettings>();
    assert_eq!(c.bindings.keys().map(String::as_str).collect::<Vec<_>>(), ["gameplay.jump"]);
    let said = lines(&app, JournalKind::Warn, "controls.bindings.gameplay.heavy");
    assert!(said.len() == 1 && said[0].contains("expected a list of binding texts, found integer"), "{said:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.toml.bad")).unwrap(), written);
    set(&mut app, "audio", "master", "0.5");
    shards::settings::flush(app.world_mut());
    assert!(std::fs::read_to_string(dir.file()).unwrap().contains("gameplay.jump"));
}

#[test]
fn a_reset_goes_back_to_what_the_game_started_with() {
    let own = || AccessibilitySettings { fov_deg: 90.0, camera_shake: 0.5, ..Default::default() };
    // No store, and a store whose file lays another number over the game's.
    let dir = TempDir::new("game_defaults");
    std::fs::write(dir.file(), "[accessibility]\nfov_deg = 100.0\n").unwrap();
    for (name, mut app) in [
        ("no store", game_with(SettingsStore::Off, |app| {
            app.insert_resource(own());
        })),
        ("a store", game_with(SettingsStore::Dir(dir.path().display().to_string()), |app| {
            app.insert_resource(own());
        })),
    ] {
        let fov = |app: &shards::bevy::prelude::App| app.world().resource::<AccessibilitySettings>().fov_deg;
        assert_eq!(fov(&app), if name == "a store" { 100.0 } else { 90.0 }, "{name}: the file is laid over the game's value");
        set(&mut app, "accessibility", "fov_deg", "110");
        ok(&mut app, "settings_set", serde_json::json!({"group": "accessibility", "key": "fov_deg", "reset": true}));
        assert_eq!(fov(&app), 90.0, "{name}: the key goes back to the game's number, not the engine's 70");
        set(&mut app, "accessibility", "fov_deg", "110");
        set(&mut app, "accessibility", "text_scale", "2");
        ok(&mut app, "settings_set", serde_json::json!({"group": "accessibility", "reset": true}));
        assert_eq!(*app.world().resource::<AccessibilitySettings>(), own(), "{name}: the whole group too");
    }
}

#[test]
fn a_game_that_asked_for_nothing_never_reads_the_shared_controls_folder() {
    use shards::motion::{InputMap, InputSettings};

    // `shards/input.ron` in a config folder is the sandbox's: a game that did not ask for an import leaves it alone.
    let dir = TempDir::new("shared_folder");
    let shared = dir.path().join("config/shards");
    std::fs::create_dir_all(&shared).unwrap();
    let old = r#"(contexts: [(name: "gameplay", actions: [(action: "jump", bindings: [Key(KeyJ)])])])"#;
    std::fs::write(shared.join("input.ron"), old).unwrap();
    let store = SettingsStore::Dir(dir.path().join("settings").display().to_string());
    let app = game_with(store, |app| {
        app.insert_resource(InputSettings { config_dir: Some(dir.path().join("config")), ..Default::default() });
    });
    assert_eq!(app.world().resource::<InputMap>().action("gameplay", "jump"), InputMap::standard().action("gameplay", "jump"));
    assert_eq!(std::fs::read_to_string(shared.join("input.ron")).unwrap(), old, "not renamed either");
    assert!(lines(&app, JournalKind::Event, "imported").is_empty());
    assert!(app.world().resource::<ControlSettings>().bindings.is_empty());
}

#[test]
fn a_file_saved_before_hit_stop_existed_loads_and_takes_the_defaults_for_it() {
    // A settings file from an older build: an accessibility table without `hit_stop` and
    // `hit_stop_local_only`. The values it has are kept, the ones it lacks are the defaults.
    let dir = TempDir::new("older_file");
    std::fs::write(dir.file(), "[accessibility]\ncamera_shake = 0.3\nfov_deg = 95.0\n").unwrap();
    let app = game_in(&dir);
    let a = app.world().resource::<AccessibilitySettings>();
    assert_eq!(a.camera_shake, 0.3);
    assert_eq!(a.fov_deg, 95.0);
    assert_eq!(a.hit_stop, AccessibilitySettings::default().hit_stop, "the missing hit_stop is the default");
    assert_eq!(a.hit_stop_local_only, AccessibilitySettings::default().hit_stop_local_only);
    assert!(a.subtitles, "the missing subtitles are on, as by default");
}
