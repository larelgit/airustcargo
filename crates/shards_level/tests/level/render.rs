//! The `render` feature: the level's visual scene hangs under one root that carries no
//! gameplay, so a window changes nothing the simulation or the AI can see. (The scene
//! really loading through Bevy's glTF loader is tested on the full engine, in
//! `crates/shards/tests/level_render.rs`.)

use ai_core::replay::state_hash;
use bevy::prelude::*;
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use serde_json::json;
use shards_level::{FromLevel, LevelInstances, LevelVisual};

use crate::common::*;

/// An app that can show scenes, and one that cannot (a server).
fn with_scenes() -> App {
    app_with(&assets(), |app| {
        app.init_asset::<WorldAsset>();
    })
}

#[test]
fn the_visual_scene_hangs_under_one_root_of_its_own() {
    let mut app = with_scenes();
    ok(&mut app, "spawn_level", json!({"id": "arena", "x": 4.0}));
    let world = app.world_mut();
    let roots: Vec<(Entity, LevelVisual, Transform)> =
        world.query::<(Entity, &LevelVisual, &Transform)>().iter(world).map(|(e, v, t)| (e, v.clone(), *t)).collect();
    assert_eq!(roots.len(), 1);
    let (root, visual, at) = &roots[0];
    assert_eq!(visual.instance, 1);
    assert_eq!(at.translation, Vec3::new(4.0, 0.0, 0.0), "placed like the level");
    assert!(world.get::<WorldAssetRoot>(*root).is_some());
    assert_eq!(world.resource::<LevelInstances>().get(1).unwrap().visual, Some(*root));
    // No gameplay on it, no visuals on gameplay.
    assert!(world.get::<FromLevel>(*root).is_none() && world.get::<ai_core::AiVisible>(*root).is_none());
    let gameplay: Vec<Entity> = world.query::<(Entity, &FromLevel)>().iter(world).map(|(e, _)| e).collect();
    assert_eq!(gameplay.len(), 18);
    assert!(gameplay.iter().all(|e| world.get::<WorldAssetRoot>(*e).is_none() && world.get::<LevelVisual>(*e).is_none()));

    // Removing the instance removes the scene too.
    shards_level::despawn_level(world, 1);
    assert!(world.get_entity(*root).is_err());
}

#[test]
fn a_window_changes_nothing_gameplay_can_see() {
    let (mut shown, mut headless) = (with_scenes(), app());
    for app in [&mut shown, &mut headless] {
        ok(app, "spawn_level", json!({"id": "arena"}));
        step(app, 30);
    }
    let visuals = |app: &mut App| {
        let world = app.world_mut();
        world.query::<&LevelVisual>().iter(world).count()
    };
    assert_eq!((visuals(&mut shown), visuals(&mut headless)), (1, 0));
    assert_eq!(state_hash(shown.world()), state_hash(headless.world()), "the same AI-visible world");
    assert_eq!(by_node(&mut shown).len(), by_node(&mut headless).len());
    let ids = |app: &mut App| -> Vec<_> {
        let nodes = by_node(app);
        nodes.iter().map(|(n, e)| (n.clone(), app.world().get::<ai_core::AiId>(*e).copied())).collect()
    };
    assert_eq!(ids(&mut shown), ids(&mut headless));
}
