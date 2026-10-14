//! A node's custom properties become components: short names and full paths, nested
//! values, text that holds JSON, prefab overrides, and errors that say which node and
//! which property.

use bevy::prelude::*;
use serde_json::json;
use shards_level::FromLevel;

use crate::common::*;
use crate::gltf::{Gltf, NodeSpec};

fn nothing_spawned(app: &mut App) -> bool {
    let world = app.world_mut();
    world.query::<&FromLevel>().iter(world).count() == 0
}

#[test]
fn components_come_by_short_name_with_nested_values() {
    let mut app = app();
    let g = one(json!({
        "Health": {"current": 40.0, "max": 50.0},
        "Team": "bandits",
        "Loot": {"table": "chest_a", "rolls": 2, "tags": ["gold", "rare"]},
        "Door": {"state": {"Stuck": {"by": "rust"}}},
        "Name": "Treasure",
    }));
    let entities = spawn(&mut app, &g).unwrap();
    let w = app.world();
    let e = entities[0];
    assert_eq!(w.get::<Health>(e), Some(&Health { current: 40.0, max: 50.0 }));
    assert_eq!(w.get::<Team>(e), Some(&Team("bandits".into())));
    assert_eq!(w.get::<Loot>(e), Some(&Loot { table: "chest_a".into(), rolls: 2, tags: vec!["gold".into(), "rare".into()] }));
    assert_eq!(w.get::<Door>(e), Some(&Door { state: DoorState::Stuck { by: "rust".into() } }));
    // What the file says beats what the engine derived (the node's name).
    assert_eq!(w.get::<Name>(e).map(Name::as_str), Some("Treasure"));
}

#[test]
fn a_full_type_path_names_a_component_too() {
    let mut app = app();
    let path = <Health as bevy::reflect::TypePath>::type_path();
    assert!(path.contains("::"), "{path}");
    let g = one(json!({ path: {"current": 1.0, "max": 2.0} }));
    let e = spawn(&mut app, &g).unwrap()[0];
    assert_eq!(app.world().get::<Health>(e), Some(&Health { current: 1.0, max: 2.0 }));
}

#[test]
fn blenders_text_properties_can_hold_json_and_flags() {
    let mut app = app();
    let g = one(json!({
        "Health": "{\"current\": 3, \"max\": 4}",
        "Loot": "{\"table\": \"x\"}",
        "Locked": true,
        "Door": "{\"state\": \"Open\"}",
    }));
    let e = spawn(&mut app, &g).unwrap()[0];
    let w = app.world();
    assert_eq!(w.get::<Health>(e), Some(&Health { current: 3.0, max: 4.0 }));
    // Fields the component's `Default` covers may be left out.
    assert_eq!(w.get::<Loot>(e), Some(&Loot { table: "x".into(), ..default() }));
    assert!(w.get::<Locked>(e).is_some());
    assert_eq!(w.get::<Door>(e).map(|d| d.state.clone()), Some(DoorState::Open));
}

#[test]
fn an_unknown_component_names_the_node_and_the_closest_one() {
    let mut app = app();
    let mut g = Gltf::new();
    let props = g.root(NodeSpec::new("Props"));
    g.child(props, NodeSpec::new("Crate.003").props(json!({"Helth": {"current": 1.0, "max": 1.0}})));
    let err = spawn(&mut app, &g).unwrap_err();
    assert!(err.starts_with("level `test.gltf` has 1 problem:"), "{err}");
    assert!(err.contains("  node `Props/Crate.003`: unknown component `Helth` (did you mean `Health`?)"), "{err}");
    assert!(nothing_spawned(&mut app));
}

#[test]
fn a_value_that_does_not_fit_names_the_component() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"Health": {"current": "lots", "max": 1.0}}))).unwrap_err();
    assert!(err.contains("node `Thing`: Health"), "{err}");
    // `Health` has a `Default`, so fields may be left out; `Door` has none.
    let health = spawn(&mut app, &one(json!({"Health": {"current": 1.0}}))).unwrap()[0];
    assert_eq!(app.world().get::<Health>(health).map(|h| h.current), Some(1.0));
    let err = spawn(&mut app, &one(json!({"Door": {}}))).unwrap_err();
    assert!(err.contains("Door is missing fields; give all of them: state"), "{err}");
    let err = spawn(&mut app, &one(json!({"Door": {"state": "Sideways"}}))).unwrap_err();
    assert!(err.contains("node `Thing`: Door"), "{err}");
}

#[test]
fn a_typo_in_a_level_property_is_not_taken_for_a_component() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"colider": "box"}))).unwrap_err();
    assert!(err.contains("unknown property `colider` (did you mean `collider`?)"), "{err}");
    // A lowercase word that is no property is still compared with the components.
    let err = spawn(&mut app, &one(json!({"health": {"current": 1.0, "max": 1.0}}))).unwrap_err();
    assert!(err.contains("unknown component `health` (did you mean `Health`?)") && err.contains("level properties are marker"), "{err}");
}

#[test]
fn properties_that_do_nothing_are_errors() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"size": [1, 1, 1], "Locked": true}))).unwrap_err();
    assert!(err.contains("`size` does nothing on this node: it shapes a collider"), "{err}");
    let err = spawn(&mut app, &one(json!({"name": "start", "Locked": true}))).unwrap_err();
    assert!(err.contains("`name` does nothing on this node"), "{err}");
    let err = spawn(&mut app, &one(json!({"accel": 3, "marker": "spawn"}))).unwrap_err();
    assert!(err.contains("`accel` does nothing"), "{err}");
}

#[test]
fn the_engine_keeps_the_pose_and_the_ids() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"Transform": {}, "AiId": 3}))).unwrap_err();
    assert!(err.contains("`Transform` cannot be set here: the pose comes from the node's place in the file"), "{err}");
    assert!(err.contains("`AiId` cannot be set here: AiId is assigned by the engine"), "{err}");
}

#[test]
fn blenders_own_keys_and_empty_nodes_are_left_alone() {
    let mut app = app();
    let mut g = Gltf::new();
    g.root(NodeSpec::new("Group"));
    let cube = g.cube();
    g.root(NodeSpec::new("Lamp").mesh(cube));
    g.root(NodeSpec::new("Quiet").props(json!({"_RNA_UI": {"x": 1}})));
    assert_eq!(spawn(&mut app, &g).unwrap(), []);
}

#[test]
fn a_prefab_node_spawns_the_prefab_and_patches_it_field_by_field() {
    let mut app = app();
    let level = read(&arena());
    let entities = shards_level::spawn_level(app.world_mut(), &level, Transform::from_xyz(100.0, 0.0, 0.0)).unwrap();
    let guard = entities
        .iter()
        .copied()
        .find(|e| app.world().get::<FromLevel>(*e).is_some_and(|f| f.node == "Guard.001"))
        .expect("the guard");
    let w = app.world();
    // The prefab's max, the node's current.
    assert_eq!(w.get::<Health>(guard), Some(&Health { current: 10.0, max: 40.0 }));
    assert_eq!(w.get::<Team>(guard), Some(&Team("undead".into())));
    assert_eq!(w.get::<Name>(guard).map(Name::as_str), Some("Skeleton"), "the prefab's name beats the node's");
    assert_eq!(w.get::<ai_core::AiKind>(guard).map(|k| k.0.as_ref()), Some("Skeleton"));
    assert!(w.get::<ai_core::AiId>(guard).is_some());
    // Placed like the node: (4, 0, -4) in the file, the level at x = 100.
    assert_eq!(w.get::<Transform>(guard).unwrap().translation, Vec3::new(104.0, 0.0, -4.0));
}

#[test]
fn prefab_nodes_are_checked_down_to_the_overrides() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"prefab": "skelton"}))).unwrap_err();
    assert!(err.contains("node `Thing`: unknown prefab `skelton` (did you mean `skeleton`?)"), "{err}");
    let err = spawn(&mut app, &one(json!({"prefab": "skeleton", "Health": {"current": "x"}}))).unwrap_err();
    assert!(err.contains("node `Thing`: overrides: Health"), "{err}");
    let err = spawn(&mut app, &one(json!({"prefab": "skeleton", "Helth": {"current": 1}}))).unwrap_err();
    assert!(err.contains("unknown component `Helth` (did you mean `Health`?)"), "{err}");
    let err = spawn(&mut app, &one(json!({"prefab": ""}))).unwrap_err();
    assert!(err.contains("`prefab` is empty"), "{err}");
    assert!(nothing_spawned(&mut app));
}

#[test]
fn every_problem_is_listed_and_nothing_is_spawned() {
    let mut app = app();
    let mut g = Gltf::new();
    let cube = g.cube();
    g.root(NodeSpec::new("Fine").mesh(cube).props(json!({"collider": "auto"})));
    g.root(NodeSpec::new("A").props(json!({"Helth": {}})));
    g.root(NodeSpec::new("B").props(json!({"marker": "spwn"})));
    g.root(NodeSpec::new("C").props(json!({"collider": "teapot", "mesh": 1})));
    let colliders = app.world().resource::<shards_motion::PhysicsWorld>().len();
    let entities = app.world().iter_entities().count();
    let err = spawn(&mut app, &g).unwrap_err();
    assert!(err.starts_with("level `test.gltf` has 4 problems:"), "{err}");
    for needle in [
        "node `A`: unknown component `Helth` (did you mean `Health`?)",
        "node `B`: `marker`: unknown marker kind `spwn` (did you mean `spawn`?)",
        "node `C`: unknown collider `teapot`",
        "node `C`: unknown property `mesh`",
    ] {
        assert!(err.contains(needle), "no `{needle}` in {err}");
    }
    assert!(nothing_spawned(&mut app));
    assert_eq!(app.world().resource::<shards_motion::PhysicsWorld>().len(), colliders);
    assert_eq!(app.world().iter_entities().count(), entities);
}
