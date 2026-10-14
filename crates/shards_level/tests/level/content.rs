//! Levels as content: the `level` kind, the check that finds what is wrong with a file,
//! and the tools.

use ai_core::{AiPlayer, AiVisible};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_content::db::Source;
use shards_content::{check_dir, ContentDb, ContentKinds};
use shards_level::{spawn_level_by_id, FromLevel, LevelDef};
use shards_motion::character;

use crate::common::*;
use crate::gltf::{Gltf, NodeSpec};

/// A small level with what the arena's definition requires (a spawn point `start`, a landmark,
/// a trigger `goal`, two `road` sockets) and a crate under `Props` with these properties.
fn small(crate_props: Value) -> Gltf {
    let mut g = small_without_start(crate_props);
    g.root(NodeSpec::new("PlayerStart").props(json!({"marker": "spawn", "name": "start"})));
    g
}

fn small_without_start(crate_props: Value) -> Gltf {
    let mut g = Gltf::new();
    let cube = g.cube();
    g.root(NodeSpec::new("Floor").scale([10.0, 1.0, 10.0]).mesh(cube).props(json!({"collider": "auto"})));
    g.root(NodeSpec::new("Shrine").props(json!({"marker": "landmark", "name": "Shrine"})));
    g.root(NodeSpec::new("GoalPad").scale([2.0, 2.0, 2.0]).mesh(cube).props(json!({"marker": "trigger", "name": "goal"})));
    g.root(NodeSpec::new("Edge.N").props(json!({"socket": "road"})));
    g.root(NodeSpec::new("Edge.S").props(json!({"socket": "road"})));
    let props = g.root(NodeSpec::new("Props"));
    g.child(props, NodeSpec::new("Crate.003").mesh(cube).props(crate_props));
    g
}

/// `check_dir` of `dir` with the game's kinds: what `--check-content` does.
fn check(dir: &std::path::Path) -> Result<shards_content::Summary, Vec<String>> {
    let app = app();
    let kinds = app.world().resource::<ContentKinds>().clone();
    check_dir(dir, &kinds).map_err(|errors| errors.iter().map(ToString::to_string).collect())
}

#[test]
fn the_arena_is_a_level_in_the_content_database() {
    let mut app = app();
    let db = app.world().resource::<ContentDb>();
    assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
    let def = db.get::<LevelDef>("arena").unwrap();
    assert_eq!(def.file, "arena.gltf");
    assert_eq!(def.required_markers.len(), 4);
    assert_eq!(db.file_of::<LevelDef>("arena"), Some("content/levels/arena.level.ron"));

    let listing = ok(&mut app, "content_list", json!({}));
    assert!(listing.contains("level  1"), "{listing}");
    let got = ok(&mut app, "content_get", json!({"kind": "level", "id": "arena"}));
    assert!(got.contains("\"required_markers\"") && got.contains("\"kind\":\"spawn\""), "{got}");
    // `content check` reads everything again from disk, with or without a running game.
    assert_eq!(ok(&mut app, "content_check", json!({})), "ok: 2 definitions in 2 files");
    assert_eq!(check(&assets()).unwrap().definitions, 2);
}

#[test]
fn a_bad_component_name_is_found_with_the_file_the_level_and_the_node_path() {
    let dir = arena_assets("bad_component", &small(json!({"Helth": {"current": 1.0, "max": 1.0}})));
    assert_eq!(
        check(&dir).unwrap_err(),
        ["content/levels/arena.level.ron: level `arena`: node `Props/Crate.003`: unknown component `Helth` (did you mean `Health`?)"]
    );
    // The good one passes.
    let dir = arena_assets("good_small", &small(json!({"Health": {"current": 1.0, "max": 1.0}})));
    assert_eq!(check(&dir).unwrap().definitions, 2);
}

#[test]
fn a_missing_required_marker_is_found() {
    let dir = arena_assets("missing_marker", &small_without_start(json!({})));
    assert_eq!(
        check(&dir).unwrap_err(),
        [
            "content/levels/arena.level.ron: level `arena`: required marker missing: `spawn` named `start` in `arena.gltf` (found 0, need 1); \
             add the custom property marker = \"spawn\" and name = \"start\" to an object in Blender"
        ]
    );
    // Present, but not enough of them.
    let mut g = small(json!({}));
    g.node_mut("Edge.S").extras = Some(json!({"socket": "river"}));
    let errors = check(&arena_assets("one_road", &g)).unwrap_err();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(errors[0].contains("`socket` named `road` in `arena.gltf` (found 1, need 2)"), "{}", errors[0]);
}

#[test]
fn every_problem_of_a_level_is_reported_on_its_own_line() {
    let mut g = small_without_start(json!({"Helth": {}}));
    g.node_mut("Edge.N").extras = Some(json!({"marker": "navlink"}));
    g.root(NodeSpec::new("Guard").props(json!({"prefab": "skelton"})));
    let errors = check(&arena_assets("many", &g)).unwrap_err();
    let level = "content/levels/arena.level.ron: level `arena`: ";
    assert_eq!(
        errors,
        [
            format!("{level}node `Edge.N`: a navlink needs `to`: the name of the node where it ends"),
            format!("{level}node `Props/Crate.003`: unknown component `Helth` (did you mean `Health`?)"),
            format!("{level}node `Guard`: unknown prefab `skelton` (did you mean `skeleton`?); known: skeleton"),
            format!("{level}required marker missing: `spawn` named `start` in `arena.gltf` (found 0, need 1); add the custom property marker = \"spawn\" and name = \"start\" to an object in Blender"),
            format!("{level}required marker missing: `socket` named `road` in `arena.gltf` (found 1, need 2); add the custom property marker = \"socket\" and name = \"road\" to an object in Blender"),
        ]
    );
}

#[test]
fn a_level_definition_is_checked_for_its_file() {
    let level = small(json!({}));
    let ron = |body: &str| format!("{{ \"arena\": ({body}) }}");
    let case = |name: &str, body: &str| -> Vec<String> {
        let dir = temp_assets(
            name,
            &[("arena.gltf", level.gltf().as_bytes()), ("notes.gltf", b"not a model"), ("content/levels/arena.level.ron", ron(body).as_bytes())],
        );
        check(&dir).unwrap_err()
    };
    let at = "content/levels/arena.level.ron: level `arena`: ";
    let errors = case("outside", r#"file: "../arena.gltf""#);
    assert_eq!(errors, [format!("{at}`../arena.gltf` must be a path inside the asset folder, like `levels/arena.gltf` (relative, no `..`)")]);
    let errors = case("absent", r#"file: "levels/none.gltf""#);
    assert!(errors[0].starts_with(&format!("{at}cannot read `levels/none.gltf`")) && errors[0].contains("(levels live in the asset folder"), "{errors:?}");
    let errors = case("garbage", r#"file: "notes.gltf""#);
    assert!(errors[0].starts_with(&format!("{at}`notes.gltf`: not a valid glTF file")), "{errors:?}");
    let errors = case("zero", r#"file: "arena.gltf", required_markers: [(kind: "spawn", min: 0)]"#);
    assert_eq!(errors, [format!("{at}required marker `spawn` has min 0, which requires nothing; use 1 or more")]);
    // A typo in a field or a marker kind is a parse error with the line.
    let errors = case("typo", "file: \"arena.gltf\",\n required_markers: [(kind: \"spwn\")]");
    assert!(errors[0].starts_with("content/levels/arena.level.ron:2:") && errors[0].contains("did you mean `spawn`?"), "{errors:?}");
    let errors = case("field", r#"file: "arena.gltf", att: (1.0, 0.0, 0.0)"#);
    assert!(errors[0].contains("Unexpected field named `att`"), "{errors:?}");
}

#[test]
fn without_an_asset_folder_only_the_definition_is_checked() {
    let mut kinds = ContentKinds::default();
    kinds.register::<LevelDef>();
    let sources = [Source { path: "content/a.level.ron".into(), text: r#"{ "a": (file: "anything.gltf") }"#.into() }];
    assert_eq!(ContentDb::build(&kinds, &sources).errors().count(), 0);
    let sources = [Source { path: "content/a.level.ron".into(), text: r#"{ "a": (file: "/etc/passwd") }"#.into() }];
    assert_eq!(ContentDb::build(&kinds, &sources).errors().count(), 1);
}

#[test]
fn spawn_level_places_the_level_where_the_definition_says_unless_told_otherwise() {
    let mut files = vec![("arena.gltf", arena().gltf().into_bytes()), ("content/props/skeleton.prefab.ron", SKELETON_PREFAB.as_bytes().to_vec())];
    files.push((
        "content/levels/arena.level.ron",
        br#"{ "arena": (file: "arena.gltf", at: (5.0, 0.0, 7.0)) }"#.to_vec(),
    ));
    let files: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (*p, b.as_slice())).collect();
    let mut app = app_in(&temp_assets("at", &files));

    let text = ok(&mut app, "spawn_level", json!({"id": "arena"}));
    assert!(text.starts_with("spawned level `arena` as instance 1 at (5.0, 7.0): 18 entities (7 solids, 3 volumes, 9 markers)"), "{text}");
    let nodes = by_node(&mut app);
    let start = app.world().get::<Transform>(nodes["Markers/Spawns/PlayerStart"]).unwrap().translation;
    assert!((start - Vec3::new(-2.0, 0.0, 13.0)).length() < 1e-4, "{start}");

    // x, y and z override one by one; the rest stays.
    let text = ok(&mut app, "spawn_level", json!({"id": "arena", "x": 100.0, "y": 2.0}));
    assert!(text.starts_with("spawned level `arena` as instance 2 at (100.0, 2.0, 7.0)"), "{text}");

    // From code: no position means the definition's.
    let spawned = spawn_level_by_id(app.world_mut(), "arena", None).unwrap();
    let from = app.world().get::<FromLevel>(spawned.entities[0]).unwrap();
    assert_eq!((from.level.as_str(), from.instance, from.node.as_str()), ("arena", 3, "Floor"));
    let floor = app.world().get::<Transform>(spawned.entities[0]).unwrap().translation;
    assert!((floor - Vec3::new(5.0, -0.25, 7.0)).length() < 1e-4, "{floor}");
}

#[test]
fn the_spawn_tool_lists_the_markers_and_refuses_with_a_hint() {
    let mut app = app();
    let text = ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "spawned level `arena` as instance 1 at (0.0, 0.0): 18 entities (7 solids, 3 volumes, 9 markers)");
    assert_eq!(lines[1], "  spawn \"start\" at (-7.0, 6.0)");
    assert!(lines.contains(&"  navlink \"ladder\" -> (11.0, 3.0, 5.5) at (11.0, 6.0)"), "{lines:#?}");
    assert_eq!(lines.len(), 1 + 9, "{lines:#?}");

    let err = fails(&mut app, "spawn_level", json!({"id": "arna"}));
    assert_eq!(err, "no level `arna` (did you mean `arena`?); levels: arena");
    let err = fails(&mut app, "spawn_level", json!({}));
    assert!(err.starts_with("bad arguments for spawn_level") && err.contains("Expected: spawn_level(id: string"), "{err}");
    // A tool that changes the world is an action: the journal has it.
    assert!(journal(&app).iter().any(|l| l.contains("dev: spawn_level {\"id\":\"arena\"}")), "{:#?}", journal(&app));
}

#[test]
fn level_info_lists_levels_markers_checks_and_instances() {
    let mut app = app();
    assert_eq!(ok(&mut app, "level_info", json!({})), "levels: 1\n  arena  arena.gltf  0 spawned");
    ok(&mut app, "spawn_level", json!({"id": "arena", "x": 40}));
    assert_eq!(ok(&mut app, "level_info", json!({})), "levels: 1\n  arena  arena.gltf  1 spawned");

    let info = ok(&mut app, "level_info", json!({"id": "arena"}));
    let lines: Vec<&str> = info.lines().collect();
    assert_eq!(lines[0], "level arena: arena.gltf, at (0.0, 0.0)");
    assert_eq!(lines[1], "23 nodes, 9 markers");
    assert_eq!(lines[2], "  spawn \"start\" at (-7.0, 6.0)  Markers/Spawns/PlayerStart");
    assert!(lines.contains(&"checks: ok"), "{info}");
    assert!(lines.last().unwrap().starts_with("instance 1: 18 entities at (40.0, 0.0)"), "{info}");

    let err = fails(&mut app, "level_info", json!({"id": "arna"}));
    assert!(err.contains("did you mean `arena`?"), "{err}");

    // The check is the same one `content check` runs: a file with problems shows them here.
    let mut broken = app_in(&arena_assets("info_problems", &small(json!({"Helth": {}}))));
    let info = ok(&mut broken, "level_info", json!({"id": "arena"}));
    assert!(info.contains("checks: 1 problem\n  node `Props/Crate.003`: unknown component `Helth`"), "{info}");
}

#[test]
fn the_ai_sees_a_spawned_level() {
    let mut app = app();
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let (controller, intent, transform) = character("", Vec3::new(0.0, 0.5, 4.0));
    app.world_mut().spawn((Name::new("Hero"), ai_core::AiKind::new("Hero"), AiPlayer, controller, intent, transform));
    step(&mut app, 2);
    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.contains("LANDMARKS  e2 Shrine 13.0m N"), "{seen}");
    // The line says what the marker is once: `SpawnPoint "start"`, not again as `spawn "start"`.
    let start = seen.lines().find(|l| l.contains("SpawnPoint")).expect("the spawn point");
    assert!(start.contains("SpawnPoint \"start\"") && !start.contains("spawn \"start\""), "{start}");
    assert!(seen.lines().any(|l| l.contains("Trigger \"goal\"")) && seen.lines().any(|l| l.contains("Camp \"bandits\"")), "{seen}");
    // Sockets and navlinks stay out of the AI's view.
    assert!(!seen.contains("Socket") && !seen.contains("NavLink"), "{seen}");
    let world = app.world_mut();
    let visible = world.query::<(&FromLevel, &AiVisible)>().iter(world).count();
    assert_eq!(visible, 6, "start, shrine, goal, guard, gate, camp");
}
