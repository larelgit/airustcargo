//! The sandbox's attack numbers are content: `assets/content/combat/attacks.attack.ron`.
//! Editing the file while the game runs changes the next hit, the edit is recorded,
//! and the replay plays the same fight.

use std::path::{Path, PathBuf};

use sandbox::SandboxPlugin;
use shards::ai_core::replay;
use shards::ai_core::{AiJournal, AiReplay, JournalKind};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::{call, ok};

const SWORD: &str = "\"sword\": (windup: 0.15, recover: 0.25, damage: 25.0,";

fn text(out: &shards::ai_core::CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// A copy of the sandbox's assets, to edit without touching the repository.
fn assets_copy(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sandbox_content_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets"), &dir);
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn app_on(dir: &Path) -> App {
    let asset_dir = dir.to_string_lossy().into_owned();
    shards::testing::test_app(move |config| config.asset_dir = asset_dir, |app| {
        app.add_plugins(SandboxPlugin);
    })
}

fn events(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Event).map(|e| e.text.clone()).collect()
}

#[test]
fn the_sandbox_content_is_clean() {
    let mut app = sandbox::test_app(42);
    assert_eq!(text(&ok(&mut app, "content_check", json!({}))), "ok: 7 definitions in 2 files");
    let sword = ok(&mut app, "content_get", json!({"kind": "attack", "id": "sword"})).result;
    assert_eq!(sword["def"]["damage"], json!(25.0));
    assert_eq!(sword["file"], "content/combat/attacks.attack.ron");
    assert!(events(&app).contains(&"content loaded: 7 definitions in 2 files".to_owned()), "{:?}", events(&app));
}

#[test]
fn an_edited_number_changes_the_next_hit_and_replays_the_same() {
    let dir = assets_copy("edit");
    let file = dir.join("content/combat/attacks.attack.ron");
    let mut app = app_on(&dir);
    ok(&mut app, "teleport", json!({"id": "e5", "x": 0, "z": -3.5}));
    ok(&mut app, "attack", json!({"target": "e6", "repeat": true}));
    ok(&mut app, "step", json!({"ticks": 30}));
    assert!(events(&app).iter().any(|l| l.contains("e5 -> e6 Sword -25")), "{:?}", events(&app));

    let original = std::fs::read_to_string(&file).unwrap();
    assert!(original.contains(SWORD), "the test edits this line: {SWORD}");
    std::fs::write(&file, original.replace(SWORD, &SWORD.replace("25.0", "40.0"))).unwrap();
    let reload = ok(&mut app, "content_reload", json!({}));
    assert_eq!(text(&reload), "content reloaded: combat/attacks.attack.ron (1 definition changed)");
    let out = ok(&mut app, "step", json!({"ticks": 60, "until": [{"journal": "Sword -40"}]}));
    assert!(text(&out).contains("stopped: journal:"), "the next swing hits for 40: {}", text(&out));
    ok(&mut app, "step", json!({"ticks": 90}));
    let recording = replay::save(app.world());

    // The file keeps the new number; the replay still begins with the old one.
    let mut again = app_on(&dir);
    replay::play(again.world_mut(), recording.clone()).unwrap();
    ok(&mut again, "step", json!({"ticks": recording.end.tick}));
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");
    let story = |app: &App| events(app).into_iter().filter(|l| l.contains(" -> ")).collect::<Vec<_>>();
    assert_eq!(story(&app), story(&again));
}

#[test]
fn a_prefab_skeleton_is_the_skeleton_the_code_spawns() {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "spawn", json!({"kind": "skeleton", "x": 4, "z": 6}));
    let spawned = text(&ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": 4, "z": 6})));
    assert_eq!(spawned, "spawned e10 skeleton at (4.0, 6.0)");
    // Everything but the id and the `Entity` (`[345v0]`).
    let inspect = |app: &mut App, id: &str| {
        let shown = text(&ok(app, "inspect", json!({"id": id})));
        shown.lines().skip(1).collect::<Vec<_>>().join("\n")
    };
    assert_eq!(inspect(&mut app, "e9"), inspect(&mut app, "e10"));

    let captain = ok(&mut app, "spawn_prefab", json!({"id": "skeleton_captain", "x": -4, "z": 6, "overrides": {"Health": {"current": 50}}}));
    assert_eq!(text(&captain), "spawned e11 skeleton_captain at (-4.0, 6.0)");
    let line = text(&ok(&mut app, "inspect", json!({"id": "e11"})));
    assert!(line.starts_with("e11 Skeleton \"Skeleton Captain\"") && line.contains("Health: hp 50/90"), "{line}");
    ok(&mut app, "step", json!({"ticks": 60}));
    let refused = call(&mut app, "spawn_prefab", json!({"id": "lich", "x": 0, "z": 0}));
    assert!(text(&refused).starts_with("unknown prefab `lich`; known: skeleton, skeleton_captain"), "{}", text(&refused));
}
