//! The sandbox is data: its world layout, characters, movement and combat numbers live in
//! `assets/content/`. Editing a file while the game runs changes the next hit (or the next
//! tick's geometry), the edit is recorded, and the replay plays the same game.

use std::path::{Path, PathBuf};

use sandbox::gameplay::LayoutPiece;
use sandbox::SandboxPlugin;
use shards::ai_core::replay;
use shards::ai_core::AiLandmark;
use shards::ai_core::AiReplay;
use shards::motion::{ColliderId, Volume, VolumeKind};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;

const LIGHT_1: &str = "hits: [(span: Phase(\"active\"), shape: Cone(range: 2.2, angle: 110), damage: 20, poise: 12)],";

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
    shards::testing::test_app(
        move |config| config.asset_dir = asset_dir,
        |app| {
            app.add_plugins(SandboxPlugin);
        },
    )
}

/// What the engine's window layer looks after (the third-person camera, the ground
/// telegraphs, the off-screen markers) is the entity with `LocalPlayer`: the player.
#[test]
fn the_player_is_the_local_player() {
    let mut app = sandbox::test_app(42);
    let world = app.world_mut();
    let local: Vec<Entity> = world.query_filtered::<Entity, With<LocalPlayer>>().iter(world).collect();
    let players: Vec<Entity> = world.query_filtered::<Entity, With<sandbox::gameplay::Player>>().iter(world).collect();
    assert_eq!(local, players, "exactly the player has LocalPlayer");
    assert_eq!(local.len(), 1);
}

#[test]
fn the_sandbox_content_is_clean() {
    let mut app = sandbox::test_app(42);
    assert_eq!(text(&ok(&mut app, "content_check", json!({}))), "ok: 69 definitions in 22 files");
    let light = ok(&mut app, "content_get", json!({"kind": "action", "id": "light_1"})).result;
    assert_eq!(light["def"]["hits"][0]["damage"], json!(20.0));
    assert_eq!(light["file"], "content/combat/player.action.ron");
    let kinds = text(&ok(&mut app, "content_list", json!({}))).to_owned();
    for kind in ["action", "moveset", "prefab", "locomotion", "layout", "combat_tuning", "behavior", "perception", "director", "attack_plan", "tile", "region", "poi", "nav", "level"] {
        assert!(kinds.contains(kind), "{kind} missing: {kinds}");
    }
    assert!(events(&app).contains(&"0 content loaded: 69 definitions in 22 files".to_owned()), "{:?}", events(&app));
}

#[test]
fn the_world_is_spawned_from_its_layout() {
    let mut app = sandbox::test_app(42);
    let seen = text(&ok(&mut app, "observe", json!({}))).to_owned();
    assert!(seen.contains("YOU  e5 Player"), "{seen}");
    assert!(seen.contains("e6 Boss \"Frost Warden\"  hp 600/600"), "{seen}");
    assert!(seen.contains("LANDMARKS  e3 Pillar"), "{seen}");
    for (id, name) in [("e9", "Stairs"), ("e10", "Ledge"), ("e11", "Pool"), ("e12", "Cliff"), ("e13", "Goal")] {
        let line = text(&ok(&mut app, "inspect", json!({"id": id}))).to_owned();
        assert!(line.starts_with(&format!("{id} {name}")), "{line}");
    }
    let world = app.world_mut();
    let pieces = world.query::<&LayoutPiece>().iter(world).count();
    assert_eq!(pieces, 30 + 1 + 2 + 5, "boxes, the ramp, water and updraft, landmarks");
    // In spawn order (query order follows archetypes, which other modules add to).
    let mut volumes: Vec<(ColliderId, String)> =
        world.query::<(&ColliderId, &Volume)>().iter(world).map(|(id, v)| (*id, format!("{}", v.kind))).collect();
    volumes.sort();
    let kinds: Vec<String> = volumes.into_iter().map(|(_, kind)| kind).collect();
    assert_eq!(kinds, ["water", "updraft", "trigger \"goal\""]);
}

#[test]
fn an_edited_number_changes_the_next_hit_and_replays_the_same() {
    let dir = assets_copy("edit");
    let file = dir.join("content/combat/player.action.ron");
    let mut app = app_on(&dir);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": -3}));
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut app, 30);
    assert!(events(&app).iter().any(|l| l.contains("e5 -> e6 light_1 -20")), "{:?}", events(&app));

    let original = std::fs::read_to_string(&file).unwrap();
    assert!(original.contains(LIGHT_1), "the test edits this line: {LIGHT_1}");
    std::fs::write(&file, original.replacen(LIGHT_1, &LIGHT_1.replace("damage: 20", "damage: 40"), 1)).unwrap();
    let reload = ok(&mut app, "content_reload", json!({}));
    assert_eq!(text(&reload), "content reloaded: combat/player.action.ron (1 definition changed)");
    let out = ok(&mut app, "step", json!({"ticks": 120, "until": [{"journal": "light_1 -40"}]}));
    assert!(text(&out).contains("stopped: journal:"), "the next combo opens for 40: {}", text(&out));
    step(&mut app, 90);
    let recording = replay::save(app.world());

    // The file keeps the new number; the replay still begins with the old one.
    let mut again = app_on(&dir);
    replay::play(again.world_mut(), recording.clone()).unwrap();
    step(&mut again, recording.end.tick);
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");
    let hits = |app: &App| events(app).into_iter().filter(|l| l.contains(" -> ")).collect::<Vec<_>>();
    assert_eq!(hits(&app), hits(&again));
}

#[test]
fn an_edited_layout_rebuilds_the_course_on_the_next_tick() {
    let dir = assets_copy("layout");
    let file = dir.join("content/world/course.layout.ron");
    let mut app = app_on(&dir);
    let updraft = |app: &mut App| {
        let world = app.world_mut();
        let found: Vec<(f32, f32)> = world
            .query::<&Volume>()
            .iter(world)
            .filter_map(|v| match v.kind {
                VolumeKind::Updraft { accel, max_speed } => Some((accel, max_speed)),
                _ => None,
            })
            .collect();
        found
    };
    assert_eq!(updraft(&mut app), [(20.0, 5.0)]);
    let before = text(&ok(&mut app, "inspect", json!({"id": "e13"}))).lines().nth(1).unwrap().to_owned();

    let original = std::fs::read_to_string(&file).unwrap();
    let edited = original.replace("Updraft(accel: 20.0, max_speed: 5.0)", "Updraft(accel: 30.0, max_speed: 8.0)");
    assert_ne!(edited, original);
    std::fs::write(&file, edited).unwrap();
    ok(&mut app, "content_reload", json!({}));
    step(&mut app, 1);
    assert_eq!(updraft(&mut app), [(30.0, 8.0)], "the volume was rebuilt");
    // Landmarks keep their ids and places.
    let after = text(&ok(&mut app, "inspect", json!({"id": "e13"}))).lines().nth(1).unwrap().to_owned();
    assert_eq!(before, after);
    assert!(text(&ok(&mut app, "inspect", json!({"id": "e9"}))).starts_with("e9 Stairs"));
}

#[test]
fn an_added_layout_is_built_and_a_removed_one_goes() {
    let dir = assets_copy("layout_added");
    let file = dir.join("content/world/annex.layout.ron");
    let mut app = app_on(&dir);
    // The pieces the annex built, its landmark, and the pillars standing (a prefab it spawns).
    let annex = |app: &mut App| -> (usize, usize, usize) {
        let world = app.world_mut();
        let pieces = world.query::<&LayoutPiece>().iter(world).filter(|p| p.layout == "annex").count();
        let gates = world.query_filtered::<&Name, With<AiLandmark>>().iter(world).filter(|n| n.as_str() == "Annex Gate").count();
        let pillars = world.query::<&sandbox::gameplay::Pillar>().iter(world).count();
        (pieces, gates, pillars)
    };
    let built = |app: &App| -> Vec<String> {
        app.world().resource::<sandbox::gameplay::layout::BuiltLayouts>().0.iter().map(|(id, _)| id.clone()).collect()
    };
    let (_, _, pillars) = annex(&mut app);
    assert_eq!((annex(&mut app).0, built(&app)), (0, ["arena", "course", "watch"].map(String::from).to_vec()));

    // A new file while the game runs: built like the others at start, prefabs included.
    std::fs::write(
        &file,
        r#"{ "annex": (
            solids: [(min: (50.0, -1.0, 50.0), max: (54.0, 0.0, 54.0))],
            landmarks: [(name: "Annex Gate", at: (52.0, 0.0, 52.0))],
            spawns: [(prefab: "pillar", at: (52.0, 0.0, 53.0))],
        ) }"#,
    )
    .unwrap();
    ok(&mut app, "content_reload", json!({}));
    step(&mut app, 1);
    assert_eq!(annex(&mut app), (2, 1, pillars + 1), "a box and a landmark, and its pillar");
    assert_eq!(built(&app), ["arena", "course", "watch", "annex"]);
    step(&mut app, 1);
    assert_eq!(annex(&mut app), (2, 1, pillars + 1), "built once");

    // The file goes: its geometry and landmark with it, the pillar it spawned stays.
    std::fs::remove_file(&file).unwrap();
    ok(&mut app, "content_reload", json!({}));
    step(&mut app, 1);
    assert_eq!(annex(&mut app), (0, 0, pillars + 1));
    assert_eq!(built(&app), ["arena", "course", "watch"]);
    // The others were left alone.
    assert!(text(&ok(&mut app, "inspect", json!({"id": "e9"}))).starts_with("e9 Stairs"));
}

#[test]
fn prefabs_spawn_the_sandbox_characters() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    let spawned = text(&ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": 2, "z": 8}))).to_owned();
    // e14 is the arena's third skeleton (`watch.layout.ron`).
    assert_eq!(spawned, "spawned e15 skeleton at (2.0, 8.0)");
    let line = text(&ok(&mut app, "inspect", json!({"id": "e15"}))).to_owned();
    assert!(line.starts_with("e15 Skeleton") && line.contains("hp 40/40"), "{line}");

    let captain = ok(
        &mut app,
        "spawn_prefab",
        json!({"id": "skeleton_captain", "x": -4, "z": 6, "overrides": {"Health": {"current": 50}}}),
    );
    assert_eq!(text(&captain), "spawned e16 skeleton_captain at (-4.0, 6.0)");
    let line = text(&ok(&mut app, "inspect", json!({"id": "e16"}))).to_owned();
    assert!(line.starts_with("e16 Skeleton \"Skeleton Captain\"") && line.contains("Health: hp 50/90"), "{line}");

    // The new skeleton fights: it walks up to the player (12 m away... within its 18 m) and claws.
    ok(&mut app, "god", json!({}));
    step(&mut app, 240);
    assert!(events(&app).iter().any(|l| l.contains("e15 -> e5 claw")), "{:?}", events(&app));
    let refused = act(&mut app, "spawn_prefab", json!({"id": "lich", "x": 0, "z": 0}));
    assert!(
        text(&refused).starts_with("unknown prefab `lich`; known: camp_skeleton, den_guard, frost_warden, pillar, player, skeleton"),
        "{}",
        text(&refused)
    );
}
