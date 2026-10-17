//! The developer menu and console on the whole engine: every GDD 32.8 item has an entry whose
//! tool exists, entries and lines run through `call_tool` as the calling agent, switches flip,
//! a game's entries come from content, Tab completes.

use std::path::{Path, PathBuf};

use serde_json::json;
use shards::ai_core::CallOutcome;
use shards::dev::{complete, menu_entries, DevConsole};
use shards::npc::NpcOverlay;
use shards::prelude::*;
use shards::testing;

/// An asset folder of the test's own, removed when it is dropped.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_dev_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content")).unwrap();
        for (file, text) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The engine alone, on `folder`'s content.
fn app(folder: &Folder) -> App {
    let dir = folder.path().display().to_string();
    testing::test_app(|config| config.asset_dir = dir, |_| {})
}

fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

fn ok(app: &mut App, tool: &str, args: serde_json::Value) -> String {
    text(&testing::ok(app, tool, args))
}

fn fails(app: &mut App, tool: &str, args: serde_json::Value) -> String {
    let out = testing::call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, got: {}", out.result);
    text(&out)
}

/// A game's entries: one it runs, one it fills.
const GAME_MENU: &str = r#"{
    "boss_phase_2": (group: "arena", label: "Boss phase 2", run: "snapshot_load path=snapshots/boss_phase2.json"),
    "spawn_crate": (group: "arena", label: "A crate here...", fill: "spawn_prefab id= x=0 z=0"),
}"#;

#[test]
fn every_item_of_the_gdd_list_has_an_entry_whose_tool_exists() {
    let folder = Folder::new("gdd", &[("content/dev/game.devmenu.ron", GAME_MENU)]);
    let mut app = app(&folder);
    let list = ok(&mut app, "dev_menu", json!({}));
    // GDD 32.8: the seed, hundreds of regions, reachability, the boss's phase, immortality and
    // slow motion, the network's journal, the saves' check, hitboxes, the AI's state, lag.
    for id in [
        "region_info",
        "worldgen_200",
        "region_check",
        "boss_phase_2",
        "god",
        "slow",
        "net_overlay",
        "save_check",
        "combat_overlay",
        "npc_overlay",
        "net_lag",
    ] {
        assert!(list.lines().any(|l| l.trim_start().starts_with(id)), "no entry `{id}`:\n{list}");
    }
    assert!(!list.contains("no tool `"), "every entry's tool exists:\n{list}");
    // Sections in the order the engine registered them, the game's after.
    let groups: Vec<&str> = list.lines().filter(|l| !l.starts_with(' ') && !l.starts_with("dev menu")).collect();
    assert_eq!(groups, ["time", "world", "combat", "ai", "net", "saves", "arena"], "{list}");
    // Before a region exists, its checks say why they do nothing.
    let check = list.lines().find(|l| l.trim_start().starts_with("region_check")).unwrap();
    assert!(check.ends_with("(not here: no region in the world: make one with `regen region=<id>`)"), "{check}");
    let fill = list.lines().find(|l| l.trim_start().starts_with("spawn_crate")).unwrap();
    assert!(fill.ends_with("spawn_prefab id= x=0 z=0 (finished in the console)"), "{fill}");
    let entries = menu_entries(app.world());
    assert_eq!(entries.iter().filter(|e| e.group == "arena").count(), 2);
}

#[test]
fn entries_and_lines_run_through_the_same_tools() {
    let folder = Folder::new("run", &[("content/dev/game.devmenu.ron", GAME_MENU)]);
    let mut app = app(&folder);
    // A switch runs the line of its other state.
    let on = ok(&mut app, "dev_menu", json!({"run": "npc_overlay"}));
    assert!(on.starts_with("npc overlay on"), "{on}");
    assert!(app.world().resource::<NpcOverlay>().enabled);
    let list = ok(&mut app, "dev_menu", json!({}));
    assert!(list.lines().any(|l| l.trim_start().starts_with("npc_overlay") && l.ends_with("npc_overlay on=false")), "{list}");
    assert_eq!(ok(&mut app, "dev_menu", json!({"run": "npc_overlay"})), "npc overlay off");
    // An entry with values to fill, one with nothing to act on, one that is not there.
    let fill = fails(&mut app, "dev_menu", json!({"run": "spawn_crate"}));
    assert!(fill.starts_with("`spawn_crate` needs values: run `spawn_prefab id= x=0 z=0`"), "{fill}");
    let none = fails(&mut app, "dev_menu", json!({"run": "worldgen_200"}));
    assert_eq!(none, "`worldgen_200` does nothing here: no region in the world: make one with `regen region=<id>`");
    let missing = fails(&mut app, "dev_menu", json!({"run": "fly"}));
    assert!(missing.starts_with("no entry `fly` in the dev menu; entries: pause, step, slow,"), "{missing}");
    // A game's entry: its tool answers, here that the file is not there.
    let boss = fails(&mut app, "dev_menu", json!({"run": "boss_phase_2"}));
    assert!(boss.contains("boss_phase2.json"), "{boss}");
}

#[test]
fn the_console_answers_each_line_and_keeps_them() {
    let folder = Folder::new("console", &[]);
    let mut app = app(&folder);
    let answer = ok(&mut app, "console", json!({"line": "npc_overlay"}));
    assert!(answer.starts_with("npc overlay on"), "{answer}");
    assert!(fails(&mut app, "console", json!({"line": "fly high=true"})).starts_with("unknown tool `fly`; available: "));
    assert_eq!(fails(&mut app, "console", json!({"line": "npc_overlay on"})), "expected key=value, got `on`");
    assert!(fails(&mut app, "console", json!({"line": "console line=god"})).starts_with("`console` runs lines itself"));
    assert!(ok(&mut app, "console", json!({"line": "help"})).starts_with("a line is a tool call as `shards act` takes it"));
    // A query's JSON answer, pretty.
    let status = ok(&mut app, "console", json!({"line": "net_status"}));
    assert!(status.starts_with("{\n") && status.contains("\"session\": \"offline\""), "{status}");
    let console = app.world().resource::<DevConsole>();
    assert_eq!(console.history, ["npc_overlay", "fly high=true", "npc_overlay on", "console line=god", "help", "net_status"]);
    let log: Vec<&str> = console.log.iter().map(String::as_str).collect();
    assert_eq!(&log[..2], ["> npc_overlay", "npc overlay on"], "{log:?}");
    assert!(log.contains(&"error: expected key=value, got `on`"), "{log:?}");
    ok(&mut app, "console", json!({"line": "clear"}));
    assert!(app.world().resource::<DevConsole>().log.is_empty());
    // Shown and hidden in a window as their keys do: one panel at a time.
    assert_eq!(ok(&mut app, "console", json!({"open": true})), "console open");
    assert!(app.world().resource::<DevConsole>().open);
    ok(&mut app, "dev_menu", json!({"open": true}));
    let console = app.world().resource::<DevConsole>();
    assert!(console.menu_open && !console.open);
    assert_eq!(fails(&mut app, "console", json!({})), "give `line` (a tool call: `god`, `speed x=0.25`) or `open`");
}

#[test]
fn the_engines_debug_keys_wait_while_a_panel_has_the_keyboard() {
    use bevy::input::keyboard::{Key, KeyboardInput};
    use bevy::input::ButtonState;
    use shards::ai_core::dev::DevKeyboard;

    let folder = Folder::new("keys", &[]);
    let mut app = app(&folder);
    let window = app.world_mut().spawn(Window::default()).id();
    // F7 down for a frame, then up: the NPC overlay's key.
    let f7 = |app: &mut App| {
        for state in [ButtonState::Pressed, ButtonState::Released] {
            let key = KeyboardInput { key_code: KeyCode::F7, logical_key: Key::F7, state, text: None, repeat: false, window };
            app.world_mut().write_message(key);
            app.update();
        }
    };
    ok(&mut app, "console", json!({"open": true}));
    app.update();
    assert_eq!(*app.world().resource::<DevKeyboard>(), DevKeyboard(true));
    f7(&mut app);
    assert!(!app.world().resource::<NpcOverlay>().enabled, "F7 typed into the console toggles nothing");
    ok(&mut app, "console", json!({"open": false}));
    app.update();
    f7(&mut app);
    assert!(app.world().resource::<NpcOverlay>().enabled);
}

#[test]
fn tab_completes_tool_names_then_their_arguments() {
    let folder = Folder::new("tab", &[]);
    let app = app(&folder);
    let world = app.world();
    assert_eq!(complete(world, "npc_ov"), ("npc_overlay ".into(), vec!["npc_overlay".to_owned()]));
    let (line, offered) = complete(world, "nav_");
    assert_eq!(line, "nav_");
    assert!(offered.contains(&"nav_overlay".to_owned()) && offered.contains(&"nav_info".to_owned()), "{offered:?}");
    assert_eq!(complete(world, "snapshot_l"), ("snapshot_load ".into(), vec!["snapshot_load".to_owned()]));
    assert_eq!(complete(world, "snapshot_load p"), ("snapshot_load path=".into(), vec!["path=".to_owned()]));
    // Names already given are not offered again; values are not completed.
    let (line, offered) = complete(world, "snapshot_load path=x ");
    assert_eq!((line.as_str(), offered), ("snapshot_load path=x force=", vec!["force=".to_owned()]));
    assert_eq!(complete(world, "speed x=0"), ("speed x=0".into(), Vec::new()));
    assert_eq!(complete(world, "zzz"), ("zzz".into(), Vec::new()));
}

#[test]
fn a_broken_entry_is_a_content_error_that_names_the_fix() {
    let folder = Folder::new("broken", &[(
        "content/dev/bad.devmenu.ron",
        r#"{ "both": (label: "Both", run: "god", fill: "god"), "none": (label: "None"), "quote": (label: "Q", run: "say text='hi") }"#,
    )]);
    let mut app = app(&folder);
    let report = ok(&mut app, "content_check", json!({}));
    assert!(report.contains("devmenu `both`: give `run` or `fill`, not both"), "{report}");
    assert!(report.contains("devmenu `none`: give `run` (a line it runs: `god`) or `fill`"), "{report}");
    assert!(report.contains("devmenu `quote`: run: unclosed `'` quote"), "{report}");
}
