//! The content files are valid, and the game's own tool and condition behave. The
//! engine checks every file when the game loads (schema, duplicate ids, references:
//! `arena -> prefab -> moveset -> action`); this fails the build on any problem.

use shards::ai_core::AiIndex;
use shards::core::flat;
use shards::content::{asset_root, check_dir, ContentKinds};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::call;

use crate::{fails, say, step, SEED};

#[test]
fn content_loads_without_a_single_error() {
    let app = my_game::test_app(SEED);
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "content problems:\n{}", errors.join("\n"));
}

/// What `cargo game --check-content` does in CI: read every file from disk, no world.
#[test]
fn check_content_is_clean() {
    let app = my_game::build_app(&shards::testing::config());
    let kinds = app.world().resource::<ContentKinds>().clone();
    let summary = check_dir(&asset_root(app.world()), &kinds).unwrap_or_else(|errors| {
        panic!("content problems:\n{}", errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))
    });
    assert_eq!((summary.kinds["arena"], summary.kinds["prefab"]), (1, 4), "{summary:?}");
    // The valley: its tiles, the level file one of them is (checked node by node), the region,
    // the navmesh settings (every tile's bake file fresh), the raider's behaviour.
    let valley = ["tile", "level", "region", "nav", "behavior"].map(|kind| summary.kinds[kind]);
    assert_eq!(valley, [4, 1, 1, 1, 1], "{summary:?}");
}

#[test]
fn a_typo_in_the_level_is_an_error_that_names_the_fix() {
    // What `--check-content` says about a level that spawns a prefab that does not exist.
    let dir = std::env::temp_dir().join(format!("my_game_typo_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("content")).unwrap();
    std::fs::write(dir.join("content/arena.arena.ron"), r#"{ "yard": (half: 20.0, spawns: [(key: "g", prefab: "grnt", at: (0.0, 0.0)), (key: "g", prefab: "grunt", at: (2.0, 0.0))]) }"#).unwrap();
    std::fs::write(dir.join("content/characters.prefab.ron"), r#"{ "grunt": (components: {}) }"#).unwrap();
    let app = my_game::build_app(&shards::testing::config());
    let kinds = app.world().resource::<ContentKinds>().clone();
    let errors = check_dir(&dir, &kinds).unwrap_err();
    let _ = std::fs::remove_dir_all(dir);
    let text = errors.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
    assert!(text.contains("arena `yard`: spawns[0].prefab -> unknown prefab `grnt` (did you mean `grunt`?)"), "{text}");
    assert!(text.contains("spawns[1].key `g` is the key of spawns[0] too: a save names one character by it"), "{text}");
}

#[test]
fn the_world_is_as_content_says() {
    let mut app = my_game::test_app(SEED);
    let seen = say(&mut app, "observe", json!({}));
    assert!(seen.contains("e1 Hero") && seen.contains("hp 100/100"), "{seen}");
    assert!(seen.contains("Wall"), "walls are landmarks: {seen}");
    let index = app.world().resource::<AiIndex>();
    assert!(index.get(shards::ai_core::AiId(1)).is_some() && index.get(shards::ai_core::AiId(2)).is_some());
    // 80 hp, 20 and 28 damage a light attack: the numbers are in content.
    let grunt = say(&mut app, "inspect", json!({"id": "e2"}));
    assert!(grunt.contains("Grunt") && grunt.contains("80"), "{grunt}");
}

#[test]
fn taunt_pulls_a_grunt_from_beyond_its_aggro_range() {
    let mut app = my_game::test_app(SEED);
    // Where the grunt stands on the floor (the height is the controller's skin).
    let at = |app: &App| {
        let grunt = app.world().resource::<AiIndex>().get(shards::ai_core::AiId(2)).unwrap();
        flat(app.world().get::<Transform>(grunt).unwrap().translation)
    };
    let start = at(&app);
    step(&mut app, 120);
    assert_eq!(at(&app), start, "22 m away is out of its aggro range (14 m): it stands still");

    let out = say(&mut app, "taunt", json!({"target": "e2"}));
    assert_eq!(out, "ok: e2 comes for e1");
    step(&mut app, 120);
    assert!((at(&app) - start).length() > 3.0, "the grunt walked {} m", (at(&app) - start).length());
}

#[test]
fn taunt_errors_say_what_to_do() {
    let mut app = my_game::test_app(SEED);
    let wall = fails(&mut app, "taunt", json!({"target": "e3"}));
    assert!(wall.contains("e3 is not a grunt") && wall.contains("observe"), "{wall}");
    let missing = fails(&mut app, "taunt", json!({"target": "e99"}));
    assert!(missing.contains("e99"), "{missing}");
    let args = fails(&mut app, "taunt", json!({}));
    assert!(args.contains("target"), "{args}");
}

#[test]
fn the_arena_is_not_cleared_until_every_grunt_is_dead() {
    let mut app = my_game::test_app(SEED);
    let cleared = |app: &mut App| {
        let out = call(app, "step", json!({"ticks": 1, "until": [{"named": "arena_cleared"}]}));
        assert!(out.ok, "{}", out.result);
        out.result.as_str().unwrap_or_default().contains("stopped")
    };
    assert!(!cleared(&mut app));
    say(&mut app, "set", json!({"id": "e2", "component": "Health", "path": "current", "value": 0}));
    assert!(cleared(&mut app), "a grunt with no health left dies, and then the arena is cleared");
}

/// Balance without a rebuild: edit a number in a content file, tell the game to reload
/// (a running game does it by itself when the file is saved), and the next hit uses it.
#[test]
fn editing_a_number_in_content_changes_the_next_hit() {
    // A copy of the assets, so the edit leaves the real files alone.
    let dir = std::env::temp_dir().join(format!("my_game_balance_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/assets")), &dir);
    let content = dir.join("content");
    let asset_dir = dir.to_string_lossy().into_owned();
    let mut app = shards::testing::test_app(
        |config| {
            config.seed = SEED;
            config.asset_dir = asset_dir;
        },
        |app| {
            app.add_plugins(my_game::GamePlugin);
        },
    );

    let hero = content.join("hero.action.ron");
    let text = std::fs::read_to_string(&hero).unwrap();
    assert!(text.contains("damage: 20, poise: 12"), "the numbers of light_1 moved; update this test");
    std::fs::write(&hero, text.replace("damage: 20, poise: 12", "damage: 50, poise: 12")).unwrap();
    let reloaded = say(&mut app, "content_reload", json!({}));
    assert!(reloaded.contains("hero.action.ron"), "{reloaded}");

    say(&mut app, "teleport", json!({"id": "e1", "x": 0, "z": -7.5}));
    say(&mut app, "attack", json!({"target": "e2", "repeat": true}));
    step(&mut app, 240);
    let hits: Vec<String> = crate::story(&app).into_iter().filter(|l| l.contains("e1 -> e2")).collect();
    assert!(hits.first().is_some_and(|hit| hit.contains("light_1 -50")), "{hits:?}");
    let _ = std::fs::remove_dir_all(dir);
}

/// `from` and everything under it, copied to `to`.
fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}
