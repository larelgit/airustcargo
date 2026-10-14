//! Spawning: file order, placement, instances, despawning, reloading a changed file.

use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_level::{despawn_level, level_asset, spawn_level, FromLevel, LevelAsset, LevelInstances};
use shards_motion::{ColliderId, PhysicsWorld};

use crate::asset::close;
use crate::common::*;
use crate::gltf::NodeSpec;

/// Everything gameplay can tell apart about the arena's entities, in file order.
fn fingerprint(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut found: Vec<(u32, String)> = world
        .query::<(&FromLevel, &Transform, Option<&AiId>, Option<&ColliderId>)>()
        .iter(world)
        .map(|(from, t, id, collider)| {
            (from.index, format!("{} {:?} {:?} {:?} {:?}", from.node, id, collider, t.translation.to_array(), t.rotation.to_array()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, line)| line).collect()
}

#[test]
fn entities_come_in_file_order_with_the_same_ids_every_time() {
    let (mut a, mut b) = (app(), app());
    spawn_arena(&mut a);
    spawn_arena(&mut b);
    let (fa, fb) = (fingerprint(&mut a), fingerprint(&mut b));
    assert_eq!(fa, fb);
    assert_eq!(fa.len(), 18, "the arena has 18 nodes with something for gameplay:\n{fa:#?}");
    assert!(fa[0].starts_with("Floor "), "{}", fa[0]);

    // AiIds count up through the visible ones in file order.
    let nodes = by_node(&mut a);
    let id = |path: &str| a.world().get::<AiId>(nodes[path]).unwrap().0;
    let visible = ["Markers/Spawns/PlayerStart", "Shrine", "GoalPad", "Guard.001", "Gate", "BanditCamp"];
    let ids: Vec<u32> = visible.iter().map(|p| id(p)).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
}

#[test]
fn the_placement_moves_and_turns_every_node() {
    let mut app = app();
    let level = LevelAsset::read(&assets(), "arena.gltf").unwrap();
    let at = Transform::from_xyz(50.0, 0.0, -20.0).with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
    spawn_level(app.world_mut(), &level, at).unwrap();
    let nodes = by_node(&mut app);
    let pos = |path: &str| app.world().get::<Transform>(nodes[path]).unwrap().translation;
    // Half a turn about Y flips x and z: (-7, 0, 6) -> (7, 0, -6), then the move.
    assert!(close(pos("Markers/Spawns/PlayerStart"), Vec3::new(57.0, 0.0, -26.0)));
    assert!(close(pos("Pillars/Pillar.001"), Vec3::new(44.0, 2.0, -26.0)));
    // The collider turned with it: Pillar.003 was at (6, 2, -6).
    app.update();
    let hit = app.world().resource::<PhysicsWorld>().raycast(Vec3::new(44.0, 10.0, -14.0), Dir3::NEG_Y, 20.0, &Default::default());
    assert_eq!(hit.map(|h| h.entity), Some(nodes["Pillars/Pillar.003"]));
}

#[test]
fn the_same_file_spawns_again_as_a_new_instance() {
    let mut app = app();
    let level = LevelAsset::read(&assets(), "arena.gltf").unwrap();
    let first = spawn_level(app.world_mut(), &level, Transform::default()).unwrap();
    let second = spawn_level(app.world_mut(), &level, Transform::from_xyz(100.0, 0.0, 0.0)).unwrap();
    assert_eq!(first.len(), second.len());
    let instance = |e: Entity| app.world().get::<FromLevel>(e).unwrap().instance;
    assert_eq!((instance(first[0]), instance(second[0])), (1, 2));
    assert_eq!(app.world().get::<FromLevel>(first[0]).unwrap().level, "arena.gltf", "labelled by its path when spawned from code");
    let instances: Vec<(u32, String)> = app.world().resource::<LevelInstances>().iter().map(|i| (i.id, i.path.clone())).collect();
    assert_eq!(instances, [(1, "arena.gltf".to_owned()), (2, "arena.gltf".to_owned())]);

    app.update();
    let colliders = app.world().resource::<PhysicsWorld>().len();
    assert_eq!(colliders, 20);
    // Removing one instance leaves the other whole.
    assert_eq!(despawn_level(app.world_mut(), 1), first.len());
    app.update();
    assert_eq!(app.world().resource::<PhysicsWorld>().len(), 10);
    assert!(first.iter().all(|e| app.world().get_entity(*e).is_err()));
    assert!(second.iter().all(|e| app.world().get_entity(*e).is_ok()));
    assert_eq!(app.world().resource::<LevelInstances>().iter().map(|i| i.id).collect::<Vec<_>>(), [2]);
    assert_eq!(despawn_level(app.world_mut(), 1), 0, "already gone");
}

#[test]
fn a_level_loaded_by_the_gate_is_there_at_tick_zero() {
    let app = app();
    assert_eq!(shards_core_tick(&app), 0);
    let level = level_asset(app.world(), "arena.gltf").expect("loaded before the first tick");
    assert_eq!(level.nodes.len(), 23);
    assert!(level_asset(app.world(), "nowhere.gltf").unwrap_err().contains("was never requested"));
}

fn shards_core_tick(app: &App) -> u64 {
    ai_core::sim::tick(app.world())
}

#[test]
fn a_missing_level_file_is_reported_and_does_not_hold_the_game() {
    let dir = temp_assets(
        "missing",
        &[("content/levels/gone.level.ron", br#"{ "gone": (file: "missing.gltf") }"#.as_slice())],
    );
    let mut app = app_in(&dir);
    assert!(errors(&app).iter().any(|e| e.contains("level: cannot load `missing.gltf`")), "{:#?}", journal(&app));
    let err = fails(&mut app, "spawn_level", json!({"id": "gone"}));
    assert!(err.contains("level file `missing.gltf` failed to load"), "{err}");
    let report = ok(&mut app, "content_check", json!({}));
    assert!(report.contains("cannot read `missing.gltf`"), "{report}");
}

#[test]
fn an_edited_file_replaces_the_spawned_level_through_a_recorded_action() {
    let mut level = arena();
    let dir = arena_assets("reload", &level);
    let mut app = app_in(&dir);
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let nodes = by_node(&mut app);
    let old = nodes["Pillars/Pillar.001"];
    assert_eq!(app.world().get::<Transform>(old).unwrap().translation.x, 6.0);

    // The artist moves a pillar and exports again; the asset server reloads the file.
    level.node_mut("Pillar.001").translation = Some([7.5, 2.0, 6.0]);
    std::fs::write(dir.join("arena.gltf"), level.gltf()).unwrap();
    app.world().resource::<AssetServer>().reload("arena.gltf");
    update_until(&mut app, "the instance follows the file", |app| journal(app).iter().any(|l| l.contains("level reloaded")));

    let nodes = by_node(&mut app);
    let new = nodes["Pillars/Pillar.001"];
    assert_ne!(old, new, "replaced, not moved");
    assert!(app.world().get_entity(old).is_err());
    assert_eq!(app.world().get::<Transform>(new).unwrap().translation.x, 7.5);
    let from = app.world().get::<FromLevel>(new).unwrap();
    assert_eq!((from.instance, from.level.as_str()), (1, "arena"), "the instance keeps its number and label");
    assert_eq!(app.world().resource::<LevelInstances>().iter().count(), 1);
    assert!(journal(&app).iter().any(|l| l.contains("level reloaded: arena (instance 1, 18 entities)")), "{:#?}", journal(&app));

    // The reload was an action: a replay has it.
    let recording = ai_core::replay::save(app.world());
    let tools: Vec<String> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            ai_core::replay::ReplayEvent::Call { tool, args, .. } => Some(format!("{tool} {args}")),
            _ => None,
        })
        .collect();
    assert_eq!(tools, [r#"spawn_level {"id":"arena"}"#, r#"level_reload {"file":"arena.gltf"}"#]);
}

#[test]
fn a_file_with_problems_leaves_the_spawned_level_as_it_was() {
    let dir = arena_assets("broken", &arena());
    let mut app = app_in(&dir);
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let before = fingerprint(&mut app);

    // Re-exported with a typo in a component name.
    let mut level = arena();
    level.node_mut("Crate.003").extras.as_mut().unwrap()["Helth"] = json!({"current": 1.0, "max": 1.0});
    std::fs::write(dir.join("arena.gltf"), level.gltf()).unwrap();
    app.world().resource::<AssetServer>().reload("arena.gltf");
    update_until(&mut app, "the refusal", |app| errors(app).iter().any(|e| e.contains("kept as it was")));

    let refusal = errors(&app).into_iter().find(|e| e.contains("kept as it was")).unwrap();
    assert!(refusal.contains("arena (instance 1) kept as it was") && refusal.contains("unknown component `Helth` (did you mean `Health`?)"), "{refusal}");
    assert_eq!(fingerprint(&mut app), before, "the old level is still there");
    assert_eq!(app.world().resource::<LevelInstances>().iter().count(), 1);

    // The tool says the same.
    let err = fails(&mut app, "level_reload", json!({"id": "arena"}));
    assert!(err.contains("kept as it was") && err.contains("Helth"), "{err}");
    let err = fails(&mut app, "level_reload", json!({"id": "nowhere"}));
    assert!(err.contains("no level `nowhere`"), "{err}");
}

#[test]
fn reloading_without_a_spawned_level_says_what_is_spawned() {
    let mut app = app();
    let err = fails(&mut app, "level_reload", json!({}));
    assert!(err.contains("no spawned level matches; no level is spawned (spawn_level)"), "{err}");
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let err = fails(&mut app, "level_reload", json!({"file": "other.gltf"}));
    assert!(err.contains("spawned: arena (instance 1)"), "{err}");
    let done = ok(&mut app, "level_reload", json!({}));
    assert_eq!(done, "level reloaded: arena (instance 1, 18 entities)");
}

#[test]
fn nodes_that_only_group_or_show_spawn_no_entity() {
    let mut app = app();
    let mut plain = crate::gltf::Gltf::new();
    let cube = plain.cube();
    let group = plain.root(NodeSpec::new("Group"));
    plain.child(group, NodeSpec::new("Visual").mesh(cube));
    assert_eq!(spawn(&mut app, &plain).unwrap(), []);
    assert_eq!(app.world().resource::<LevelInstances>().iter().count(), 1, "still an instance, with nothing in it");
}
