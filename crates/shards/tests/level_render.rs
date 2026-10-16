//! The visual side of a level on the real engine: one glTF file read twice, by the level
//! loader for gameplay and by Bevy's own glTF loader for looks, with no render-only entity
//! among the gameplay ones. The headless engine keeps its render plugins without a GPU, so
//! scenes load here as they do in a window.
//!
//! The levels are fixtures of `shards_level`'s own tests: `arena.gltf`, written by the helper
//! that stands in for Blender's exporter, and `outpost.glb`, a real export of Blender 5.2.2.

#![cfg(feature = "render")]

use std::time::{Duration, Instant};

use shards::ai_core::{AiId, AiIndex};
use shards::bevy::world_serialization::WorldAssetRoot;
use shards::level::{FromLevel, LevelVisual};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::ok;

fn arena_dir() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../shards_level/tests/level/assets").to_owned()
}

fn update_until(app: &mut App, what: &str, done: impl Fn(&mut App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(30), "gave up waiting for: {what}");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn is_below(world: &World, mut entity: Entity, ancestor: Entity) -> bool {
    while let Some(parent) = world.get::<ChildOf>(entity) {
        entity = parent.parent();
        if entity == ancestor {
            return true;
        }
    }
    false
}

#[test]
fn a_level_shows_as_a_scene_beside_its_gameplay() {
    let mut app = shards::testing::test_app(|config| config.asset_dir = arena_dir(), |_| {});
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:#?}");

    ok(&mut app, "spawn_level", json!({"id": "arena", "x": 10.0}));
    let ids_before = app.world().resource::<AiIndex>().ids();
    let gameplay: Vec<Entity> = {
        let world = app.world_mut();
        world.query::<(Entity, &FromLevel)>().iter(world).map(|(e, _)| e).collect()
    };
    assert_eq!(gameplay.len(), 18);

    // One root for the scene, placed like the level, holding Bevy's scene of the same file.
    let root = {
        let world = app.world_mut();
        let roots: Vec<Entity> = world.query_filtered::<Entity, With<LevelVisual>>().iter(world).collect();
        assert_eq!(roots.len(), 1);
        roots[0]
    };
    assert!(app.world().get::<WorldAssetRoot>(root).is_some());
    assert_eq!(app.world().get::<Transform>(root).unwrap().translation, Vec3::new(10.0, 0.0, 0.0));

    // The scene loads: the file's ten meshes appear below the root, under the names of the
    // Blender objects.
    update_until(&mut app, "the scene is instantiated", |app| {
        let world = app.world_mut();
        let meshes: Vec<Entity> = world.query_filtered::<Entity, With<Mesh3d>>().iter(world).collect();
        meshes.iter().filter(|m| is_below(world, **m, root)).count() == 10
    });
    let world = app.world_mut();
    let names: Vec<String> = world
        .query::<(Entity, &Name)>()
        .iter(world)
        .filter(|(e, _)| is_below(world, *e, root))
        .map(|(_, name)| name.as_str().to_owned())
        .collect();
    for wanted in ["Floor", "Pillar.001", "Pond", "Lamp", "PlayerStart"] {
        assert!(names.iter().any(|n| n == wanted), "no `{wanted}` in the scene: {names:?}");
    }

    // Looks add no gameplay: the same 18 entities, none inside the scene, no new AiId.
    let gameplay_now: Vec<Entity> = world.query::<(Entity, &FromLevel)>().iter(world).map(|(e, _)| e).collect();
    assert_eq!(gameplay_now, gameplay);
    assert!(gameplay.iter().all(|e| !is_below(world, *e, root)));
    assert!(gameplay.iter().all(|e| world.get::<Mesh3d>(*e).is_none()));
    assert_eq!(world.resource::<AiIndex>().ids(), ids_before);
    let _ = AiId(0);
}

/// The components the Blender level names that are not the engine's (`shards_level`'s tests
/// register the same).
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Loot {
    table: String,
    rolls: u32,
    tags: Vec<String>,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Locked;

#[test]
fn a_blender_export_shows_the_scene_its_level_comes_from() {
    // `outpost.glb`, exported by Blender 5.2.2: scene 0 is `Kit` (the instanced collection's
    // own scene), the level is the default scene, 1.
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../shards_level/tests/level/blender").to_owned();
    let mut app = shards::testing::test_app(
        |config| config.asset_dir = dir,
        |app| {
            app.register_type::<Loot>().register_type::<Locked>();
        },
    );
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    ok(&mut app, "spawn_level", json!({"id": "outpost"}));
    let root = {
        let world = app.world_mut();
        world.query_filtered::<Entity, With<LevelVisual>>().single(world).unwrap()
    };
    // The level's eleven meshes, the barrel of each collection instance among them.
    update_until(&mut app, "the scene is instantiated", |app| {
        let world = app.world_mut();
        let meshes: Vec<Entity> = world.query_filtered::<Entity, With<Mesh3d>>().iter(world).collect();
        meshes.iter().filter(|m| is_below(world, **m, root)).count() == 11
    });
    let world = app.world_mut();
    let names: Vec<String> = world
        .query::<(Entity, &Name)>()
        .iter(world)
        .filter(|(e, _)| is_below(world, *e, root))
        .map(|(_, name)| name.as_str().to_owned())
        .collect();
    // Bevy names the scene's root after it.
    for wanted in ["Outpost", "Ground", "Wall North", "Säule", "Костёр", "Barrels.A", "Barrels.B"] {
        assert!(names.iter().any(|n| n == wanted), "no `{wanted}` in the scene: {names:?}");
    }
    assert!(!names.iter().any(|n| n == "Kit"), "{names:?}");
}
