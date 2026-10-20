//! `settings` and `settings_set`, and a game's own groups.

use serde_json::json;
use shards::ai_core::{AiReplay, JournalKind};
use shards::core::SettingsStore;
use shards::prelude::*;
use shards::settings::{AccessibilitySettings, SettingsAppExt, SettingsChanged, SettingsGroup};

use crate::common::*;

#[test]
fn settings_lists_every_group_as_lines() {
    let mut app = game();
    let all = ok(&mut app, "settings", json!({}));
    for line in [
        "graphics.fullscreen = false",
        "graphics.resolution = \"auto\"",
        "audio.master = 1",
        "controls.bindings = {}",
        "controls.mouse_sensitivity = 1",
        "controls.aim_sensitivity = 1",
        "accessibility.camera_shake = 1",
        "accessibility.flashes = \"full\"",
        "accessibility.fov_deg = 70",
        "accessibility.hold_to_toggle = false",
    ] {
        assert!(all.lines().any(|l| l == line), "`{line}` in\n{all}");
    }
    assert!(all.lines().last().unwrap().starts_with("# not saved"), "{all}");

    let only = ok(&mut app, "settings", json!({"group": "audio"}));
    assert!(only.lines().filter(|l| !l.starts_with('#')).all(|l| l.starts_with("audio.")), "{only}");
    let typo = fails(&mut app, "settings", json!({"group": "acessibility"}));
    assert!(typo.contains("did you mean `accessibility`") && typo.contains("groups: graphics, audio, controls, accessibility"), "{typo}");

    // With a folder, it says where.
    let dir = TempDir::new("lines");
    let mut kept = game_in(&dir);
    let out = ok(&mut kept, "settings", json!({"group": "audio"}));
    assert!(out.contains("# kept in") && out.contains("settings.toml"), "{out}");
}

#[test]
fn a_value_is_read_by_the_type_of_the_setting() {
    let mut app = game();
    // A change is shown as it was and is, and journaled.
    let out = set(&mut app, "accessibility", "camera_shake", "0.3");
    assert!(out.starts_with("ok:") && out.contains("accessibility.camera_shake 1 → 0.3"), "{out}");
    assert_eq!(app.world().resource::<AccessibilitySettings>().camera_shake, 0.3);
    assert_eq!(lines(&app, JournalKind::Event, "settings: accessibility.camera_shake 1 → 0.3").len(), 1);

    // Words for booleans, case for choices, quotes for text.
    set(&mut app, "accessibility", "hold_to_toggle", "yes");
    assert!(app.world().resource::<AccessibilitySettings>().hold_to_toggle);
    set(&mut app, "accessibility", "flashes", "Reduced");
    assert_eq!(app.world().resource::<AccessibilitySettings>().flashes, "Reduced", "text is kept as typed; the reading ignores case");
    set(&mut app, "graphics", "resolution", "\"1600x900\"");
    assert_eq!(app.world().resource::<shards::settings::GraphicsSettings>().resolution, "1600x900");

    // The same value again is nothing to do.
    let again = set(&mut app, "accessibility", "camera_shake", "0.3");
    assert!(again.contains("nothing changed"), "{again}");

    // `shards act settings_set ... value=0.3` sends a number, `value=true` a boolean: the same as their text.
    let number = ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "fov_deg", "value": 95}));
    assert!(number.contains("accessibility.fov_deg 70 → 95"), "{number}");
    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "text_scale", "value": 1.25}));
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "invert_y", "value": true}));
    ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.parry", "value": ["Key(KeyP)"]}));
    let w = app.world();
    assert_eq!(w.resource::<AccessibilitySettings>().text_scale, 1.25);
    assert!(w.resource::<shards::settings::ControlSettings>().invert_y);
    assert_eq!(w.resource::<shards::settings::ControlSettings>().bindings["gameplay.parry"], ["Key(KeyP)"]);
}

#[test]
fn a_value_that_does_not_fit_is_refused_and_says_what_does() {
    let mut app = game();
    let before = ok(&mut app, "settings", json!({}));
    let expect = |app: &mut shards::bevy::prelude::App, group: &str, key: &str, value: &str, needles: &[&str]| {
        let err = fails(app, "settings_set", json!({"group": group, "key": key, "value": value}));
        for needle in needles {
            assert!(err.contains(needle), "`{needle}` in `{err}` ({group}.{key}={value})");
        }
    };
    expect(&mut app, "accessibility", "hold_to_toggle", "maybe", &["accessibility.hold_to_toggle", "`maybe`", "true or false"]);
    expect(&mut app, "accessibility", "fov_deg", "wide", &["`wide`", "a number"]);
    expect(&mut app, "accessibility", "fov_deg", "200", &["accessibility.fov_deg 200", "30 to 120"]);
    expect(&mut app, "accessibility", "flashes", "blink", &["`blink`", "full, reduced, off"]);
    expect(&mut app, "accessibility", "camera_shake", "NaN", &["a number"]);
    expect(&mut app, "graphics", "resolution", "wide", &["graphics.resolution", "1920x1080"]);
    expect(&mut app, "graphics", "shadows", "ultra", &["off, low, medium, high"]);
    expect(&mut app, "audio", "master", "1.5", &["audio.master 1.5", "0 to 1"]);
    expect(&mut app, "controls", "mouse_sensitivity", "0", &["0.05 to 5"]);
    expect(&mut app, "controls", "aim_sensitivity", "9", &["controls.aim_sensitivity 9", "0.05 to 5"]);
    expect(&mut app, "controls", "hold_or_toggle.gameplay.sprint", "tap", &["press, hold, toggle"]);
    expect(&mut app, "controls", "bindings", "x", &["not JSON"]);
    // Names are named.
    expect(&mut app, "accessibility", "fov", "90", &["no setting `fov`", "did you mean `fov_deg`", "camera_shake"]);
    expect(&mut app, "sound", "master", "1", &["no settings group `sound`", "audio"]);
    expect(&mut app, "audio", "master.x", "1", &["audio.master is not a table"]);
    let nothing = fails(&mut app, "settings_set", json!({"group": "audio"}));
    assert!(nothing.contains("name a `key`") && nothing.contains("master, music, sfx, voice"), "{nothing}");
    let no_value = fails(&mut app, "settings_set", json!({"group": "audio", "key": "master"}));
    assert!(no_value.contains("give a `value`") && no_value.contains("a number"), "{no_value}");
    // After all that, nothing moved.
    assert_eq!(ok(&mut app, "settings", json!({})), before);
}

#[test]
fn reset_puts_back_the_default() {
    let mut app = game();
    set(&mut app, "audio", "music", "0.1");
    set(&mut app, "audio", "sfx", "0.2");
    set(&mut app, "controls", "bindings.gameplay.jump", "[\"Key(KeyJ)\"]");
    set(&mut app, "controls", "hold_or_toggle.gameplay.sprint", "toggle");

    let one = ok(&mut app, "settings_set", json!({"group": "audio", "key": "music", "reset": true}));
    assert!(one.contains("audio.music 0.1 → 0.8"), "{one}");
    assert_eq!(app.world().resource::<shards::settings::AudioSettings>().sfx, 0.2, "the others stay");

    // A table entry has no default: it goes.
    let entry = ok(&mut app, "settings_set", json!({"group": "controls", "key": "hold_or_toggle.gameplay.sprint", "reset": true}));
    assert!(entry.contains("controls.hold_or_toggle.gameplay.sprint \"toggle\" → (unset)") && entry.lines().count() == 3, "{entry}");

    // The whole group.
    let group = ok(&mut app, "settings_set", json!({"group": "audio", "reset": true}));
    assert!(group.contains("audio.sfx 0.2 → 1"), "{group}");
    assert_eq!(*app.world().resource::<shards::settings::AudioSettings>(), Default::default());
    assert_eq!(app.world().resource::<shards::settings::ControlSettings>().bindings.len(), 1, "another group is not touched");
}

#[test]
fn reset_reads_its_key_as_carefully_as_set_does() {
    let mut app = game();
    set(&mut app, "graphics", "resolution", "1600x900");
    let before = ok(&mut app, "settings", json!({}));
    for (group, key, needle) in [
        ("audio", "master.x", "audio.master is not a table"),
        ("graphics", "resolution.width", "graphics.resolution is not a table"),
        ("accessibility", "fov", "did you mean `fov_deg`"),
    ] {
        let err = fails(&mut app, "settings_set", json!({"group": group, "key": key, "reset": true}));
        assert!(err.contains(needle), "{group}.{key}: {err}");
    }
    assert_eq!(ok(&mut app, "settings", json!({})), before, "nothing moved");
    // An entry of a real table that is not there is already so, as the manual (docs/manual.md) says.
    let out = ok(&mut app, "settings_set", json!({"group": "controls", "key": "bindings.gameplay.nope", "reset": true}));
    assert!(out.contains("nothing changed"), "{out}");
}

#[test]
fn a_change_of_settings_is_not_part_of_the_game() {
    let mut app = game();
    let recorded = app.world().resource::<AiReplay>().recorded_events();
    set(&mut app, "accessibility", "fov_deg", "100");
    set(&mut app, "controls", "invert_y", "true");
    assert_eq!(app.world().resource::<AiReplay>().recorded_events(), recorded, "a control tool: replays do not record it");
}

/// The groups `SettingsChanged` named, in order.
#[derive(Resource, Default)]
struct Heard(Vec<String>);

#[test]
fn what_follows_a_group_hears_it_change_before_the_call_returns() {
    let mut app = game_with(SettingsStore::Off, |app| {
        app.init_resource::<Heard>().add_observer(|changed: On<SettingsChanged>, mut heard: ResMut<Heard>| heard.0.push(changed.group.clone()));
    });
    set(&mut app, "audio", "music", "0.1");
    assert_eq!(app.world().resource::<Heard>().0, ["audio"], "once, from settings_set itself");
    ok(&mut app, "settings_set", json!({"group": "audio", "key": "music", "value": "0.1"}));
    assert_eq!(app.world().resource::<Heard>().0, ["audio"], "nothing changed: nothing heard");

    // Code that changes a group itself is heard on the next frame.
    app.world_mut().resource_mut::<AccessibilitySettings>().fov_deg = 100.0;
    app.update();
    assert_eq!(app.world().resource::<Heard>().0, ["audio", "accessibility"]);
}

// --- a game's own group ---

#[derive(Reflect, Default, Clone, Copy, Debug, PartialEq)]
enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

#[derive(Resource, Reflect, SettingsGroup, Default, Clone, Debug, PartialEq)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "tutorial")]
struct TutorialSettings {
    show_hints: bool,
    hint_delay: u8,
    language: String,
    difficulty: Difficulty,
    dismissed: Vec<String>,
}

fn with_tutorial(app: &mut shards::bevy::prelude::App) {
    app.add_settings_group::<TutorialSettings>();
}

#[test]
fn a_game_adds_its_own_group() {
    let dir = TempDir::new("game_group");
    let mut app = game_with(SettingsStore::Dir(dir.path().display().to_string()), with_tutorial);
    let listed = ok(&mut app, "settings", json!({"group": "tutorial"}));
    assert!(listed.contains("tutorial.difficulty = \"Normal\"") && listed.contains("tutorial.dismissed = []"), "{listed}");

    set(&mut app, "tutorial", "difficulty", "hard");
    set(&mut app, "tutorial", "language", "Українська");
    set(&mut app, "tutorial", "show_hints", "on");
    set(&mut app, "tutorial", "dismissed", "[\"jump\", \"dodge\"]");
    set(&mut app, "tutorial", "hint_delay", "30");
    let w = app.world().resource::<TutorialSettings>().clone();
    assert_eq!(w, TutorialSettings { show_hints: true, hint_delay: 30, language: "Українська".into(), difficulty: Difficulty::Hard, dismissed: vec!["jump".into(), "dodge".into()] });

    // Types are checked against the Rust type.
    let range = fails(&mut app, "settings_set", json!({"group": "tutorial", "key": "hint_delay", "value": "300"}));
    assert!(range.contains("300 is out of range (0 to 255)"), "{range}");
    let neg = fails(&mut app, "settings_set", json!({"group": "tutorial", "key": "hint_delay", "value": "-1"}));
    assert!(neg.contains("out of range"), "{neg}");
    let word = fails(&mut app, "settings_set", json!({"group": "tutorial", "key": "difficulty", "value": "brutal"}));
    assert!(word.contains("Easy, Normal, Hard"), "{word}");

    // It is saved with the others and read back.
    shards::settings::flush(app.world_mut());
    let text = std::fs::read_to_string(dir.file()).unwrap();
    assert!(text.contains("[tutorial]") && text.contains("difficulty = \"Hard\""), "{text}");
    drop(app);
    let again = game_with(SettingsStore::Dir(dir.path().display().to_string()), with_tutorial);
    assert_eq!(*again.world().resource::<TutorialSettings>(), w);
}

#[test]
fn a_group_toml_cannot_hold_is_refused_when_it_is_added() {
    #[derive(Resource, Reflect, SettingsGroup, Default)]
    #[reflect(Resource, SettingsGroup, Default)]
    struct BigCounter {
        fine: i64,
        score: u64,
    }

    #[derive(Resource, Reflect, SettingsGroup, Default)]
    #[reflect(Resource, SettingsGroup, Default)]
    struct KeyedByNumber {
        by_slot: std::collections::BTreeMap<u32, String>,
    }

    #[derive(Resource, Reflect, SettingsGroup, Default)]
    #[reflect(Resource, Default)]
    struct NotSaved {
        flag: bool,
    }

    fn refused(add: fn(&mut shards::bevy::prelude::App)) -> String {
        let mut app = shards::bevy::prelude::App::new();
        app.add_plugins(shards::bevy::prelude::MinimalPlugins)
            .add_plugins(shards::ai_core::AiCorePlugin { remote_port: None, crash_replays: false, ..Default::default() })
            .add_plugins(shards::settings::ShardsSettingsPlugin);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| add(&mut app))).expect_err("should have been refused");
        caught.downcast_ref::<String>().cloned().or_else(|| caught.downcast_ref::<&str>().map(|s| (*s).to_owned())).unwrap_or_default()
    }
    let big = refused(|app| {
        app.add_settings_group::<BigCounter>();
    });
    assert!(big.contains("BigCounter.score") && big.contains("u64") && big.contains("TOML"), "{big}");
    let keyed = refused(|app| {
        app.add_settings_group::<KeyedByNumber>();
    });
    assert!(keyed.contains("KeyedByNumber.by_slot") && keyed.contains("u32") && keyed.contains("String"), "{keyed}");
    let unsaved = refused(|app| {
        app.add_settings_group::<NotSaved>();
    });
    assert!(unsaved.contains("NotSaved") && unsaved.contains("#[reflect(Resource, SettingsGroup, Default)]"), "{unsaved}");
}

#[test]
fn a_group_in_a_file_of_its_own_is_refused() {
    // Bevy would keep it in keys.toml, which the engine does not create, check, back up or watch,
    // while the `settings` tool says settings.toml: refused, naming the type and the attribute.
    #[derive(Resource, Reflect, SettingsGroup, Default)]
    #[reflect(Resource, SettingsGroup, Default)]
    #[settings_group(group = "keys", file = "keys")]
    struct OwnFile {
        flag: bool,
    }

    // Naming the engine's own file is the default, spelled out.
    #[derive(Resource, Reflect, SettingsGroup, Default)]
    #[reflect(Resource, SettingsGroup, Default)]
    #[settings_group(group = "plain", file = "settings")]
    struct SameFile {
        flag: bool,
    }

    let mut app = shards::bevy::prelude::App::new();
    app.add_plugins(shards::bevy::prelude::MinimalPlugins)
        .add_plugins(shards::ai_core::AiCorePlugin { remote_port: None, crash_replays: false, ..Default::default() })
        .add_plugins(shards::settings::ShardsSettingsPlugin);
    app.add_settings_group::<SameFile>();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        app.add_settings_group::<OwnFile>();
    }))
    .expect_err("should have been refused");
    let said = caught.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(said.contains("OwnFile") && said.contains("file = \"keys\"") && said.contains("settings.toml only"), "{said}");
}
