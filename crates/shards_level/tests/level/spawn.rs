//! Spawning: file order, placement, instances, despawning, reloading a changed file.

use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_content::ContentAppExt;
use shards_level::{despawn_level, level_asset, spawn_level, FromLevel, LevelAsset, LevelInstances, LevelMarker};
use shards_motion::{Collider, ColliderId, PhysicsWorld};

use crate::asset::close;
use crate::common::*;
use crate::gltf::NodeSpec;

/// Everything gameplay can tell apart about the arena's entities, in file order.
fn fingerprint(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut found: Vec<(u32, String)> = world
        .query::<(&FromLevel, &Transform, Option<&AiId>, Option<&ColliderId>, Option<&Collider>, Option<&LevelMarker>)>()
        .iter(world)
        .map(|(from, t, id, collider_id, collider, marker)| {
            let pose = (t.translation.to_array(), t.rotation.to_array(), t.scale.to_array());
            (from.index, format!("{} {id:?} {collider_id:?} {pose:?} {collider:?} {marker:?}", from.node))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, line)| line).collect()
}

/// FNV-1a over the lines of [`fingerprint`].
fn fingerprint_hash(app: &mut App) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in fingerprint(app).join("\n").bytes() {
        h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// What the arena spawns, as a number. It depends on nothing but the file, the engine's
/// numbering and the physics shapes, so it is the same in a headless build
/// (`cargo test -p shards_level`) and with the `render` feature (`cargo test --workspace`):
/// both check this value. (Update it when the arena or the numbering changes on purpose.)
const ARENA_FINGERPRINT: u64 = 17602491870142193101;

#[test]
fn the_arena_spawns_the_same_world_in_every_build() {
    let mut app = app();
    spawn_arena(&mut app);
    assert_eq!(fingerprint_hash(&mut app), ARENA_FINGERPRINT, "{:#?}", fingerprint(&mut app));
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
fn a_game_places_level_files_the_content_does_not_list() {
    // The tiles of a region: files nobody listed, requested by path, waited for, placed.
    let mut tile = crate::gltf::Gltf::new();
    let cube = tile.cube();
    tile.root(NodeSpec::new("Ground").scale([8.0, 0.5, 8.0]).mesh(cube).props(json!({"collider": "auto"})));
    tile.root(NodeSpec::new("North").at([0.0, 0.0, -4.0]).props(json!({"socket": "road"})));
    tile.root(NodeSpec::new("South").at([0.0, 0.0, 4.0]).props(json!({"socket": "road"})));
    let dir = temp_assets("tiles", &[("tiles/ruin_a.gltf", tile.gltf().as_bytes())]);
    let mut app = app_in(&dir);

    let path = "tiles/ruin_a.gltf";
    let status = |app: &App| app.world().resource::<shards_level::LevelLibrary>().status(app.world(), path);
    assert!(matches!(status(&app), shards_core::LoadStatus::Failed(why) if why.contains("was never requested")));
    let err = shards_level::spawn_level_file(app.world_mut(), path, Transform::default(), "tile").unwrap_err();
    assert!(err.contains("was never requested"), "{err}");

    shards_level::request_level(app.world_mut(), path);
    update_until(&mut app, "the tile loads", |app| status(app) == shards_core::LoadStatus::Ready);

    // Sockets can be read from the file before anything is spawned.
    let sockets: Vec<(String, Vec3)> = level_asset(app.world(), path)
        .unwrap()
        .markers()
        .into_iter()
        .map(|m| (m.name, m.position))
        .collect();
    assert_eq!(sockets, [("road".to_owned(), Vec3::new(0.0, 0.0, -4.0)), ("road".to_owned(), Vec3::new(0.0, 0.0, 4.0))]);

    // Two tiles, one next to the other and one turned.
    let a = shards_level::spawn_level_file(app.world_mut(), path, Transform::from_xyz(0.0, 0.0, 0.0), "tile").unwrap();
    let turned = Transform::from_xyz(8.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    let b = shards_level::spawn_level_file(app.world_mut(), path, turned, "tile").unwrap();
    assert_eq!((a.instance, b.instance, a.entities.len(), b.entities.len()), (1, 2, 3, 3));
    let socket_at = |app: &App, e: Entity| app.world().get::<Transform>(e).unwrap().translation;
    // The turned tile's north socket (0, 0, -4) turns to (-4, 0, 0): at x = 4.
    assert!(close(socket_at(&app, b.entities[1]), Vec3::new(4.0, 0.0, 0.0)));
    assert!(close(socket_at(&app, b.entities[2]), Vec3::new(12.0, 0.0, 0.0)));
    assert_eq!(app.world().get::<FromLevel>(b.entities[0]).unwrap().level, "tile");

    // A region change removes a tile by its instance.
    assert_eq!(despawn_level(app.world_mut(), a.instance), 3);
    assert_eq!(app.world().resource::<LevelInstances>().iter().map(|i| i.id).collect::<Vec<_>>(), [2]);
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

/// Holds content while present, as a co-op session does.
#[derive(Resource)]
struct Held;

fn held(world: &World) -> Option<String> {
    world.contains_resource::<Held>().then(|| "during the test's session would desync it".to_owned())
}

#[test]
fn a_file_changed_while_content_is_held_is_not_spawned_until_the_change_applies() {
    let mut level = arena();
    let dir = arena_assets("held", &level);
    let mut app = app_with(&dir, |app| {
        app.add_content_guard(held);
    });
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    app.world_mut().insert_resource(Held);
    level.node_mut("Pillar.001").translation = Some([7.5, 2.0, 6.0]);
    std::fs::write(dir.join("arena.gltf"), level.gltf()).unwrap();
    app.world().resource::<AssetServer>().reload("arena.gltf");
    update_until(&mut app, "the change waits", |app| journal(app).iter().any(|l| l.contains("a level file changed on disk and waits")));

    // The asset server has the new file, the other machines the old one: no spawn from it.
    let refused = fails(&mut app, "spawn_level", json!({"id": "arena"}));
    assert_eq!(refused, "refused: `arena.gltf` changed on disk, and a level spawn from the changed file during the test's session would desync it");
    let at = Transform::from_xyz(40.0, 0.0, 0.0);
    assert_eq!(shards_level::spawn_level_file(app.world_mut(), "arena.gltf", at, "tile").err(), Some(refused), "a region's tile neither");
    assert_eq!(app.world().resource::<LevelInstances>().iter().count(), 1);

    // Held no more: the change applies, and the file spawns again.
    app.world_mut().remove_resource::<Held>();
    update_until(&mut app, "the change applies", |app| journal(app).iter().any(|l| l.contains("level reloaded")));
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let recording = ai_core::replay::save(app.world());
    let tools: Vec<&str> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            ai_core::replay::ReplayEvent::Call { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tools, ["spawn_level", "level_reload", "spawn_level"], "the refusal is not recorded");
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
fn a_degenerate_pose_leaves_the_spawned_level_as_it_was() {
    let mut level = arena();
    let dir = arena_assets("flat", &level);
    let mut app = app_in(&dir);
    ok(&mut app, "spawn_level", json!({"id": "arena"}));
    let before = fingerprint(&mut app);
    assert!(!before.is_empty());

    // An object flattened with `S Z 0` in Blender exports a zero scale: no usable pose.
    level.node_mut("Pillar.001").scale = Some([1.0, 0.0, 1.0]);
    std::fs::write(dir.join("arena.gltf"), level.gltf()).unwrap();
    app.world().resource::<AssetServer>().reload("arena.gltf");
    update_until(&mut app, "the refusal", |app| errors(app).iter().any(|e| e.contains("kept as it was")));

    let refusal = errors(&app).into_iter().find(|e| e.contains("kept as it was")).unwrap();
    assert!(refusal.contains("arena (instance 1) kept as it was") && refusal.contains("not a valid pose") && refusal.contains("Pillar.001"), "{refusal}");
    assert_eq!(fingerprint(&mut app), before, "the old level is still there");
    assert_eq!(app.world().resource::<LevelInstances>().iter().count(), 1);

    // The tool says the same, and the instance is still there to reload.
    let err = fails(&mut app, "level_reload", json!({"id": "arena"}));
    assert!(err.contains("kept as it was") && err.contains("not a valid pose"), "{err}");
    assert_eq!(fingerprint(&mut app), before);
    assert_eq!(app.world().resource::<LevelInstances>().iter().count(), 1);

    // The artist fixes the file: the level follows it, under the same instance number.
    level.node_mut("Pillar.001").scale = Some([1.0, 1.0, 1.0]);
    level.node_mut("Pillar.001").translation = Some([7.5, 2.0, 6.0]);
    std::fs::write(dir.join("arena.gltf"), level.gltf()).unwrap();
    app.world().resource::<AssetServer>().reload("arena.gltf");
    update_until(&mut app, "the fixed file", |app| journal(app).iter().any(|l| l.contains("level reloaded: arena (instance 1")));
    assert_eq!(app.world().resource::<LevelInstances>().iter().map(|i| i.id).collect::<Vec<_>>(), [1]);
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

#[test]
fn a_level_goes_in_its_file_order_whatever_order_the_world_lists_it_in() {
    // The entities with an `AiId` leave tombstones in the order they go, which a snapshot's twin must repeat: it is the
    // node order of the file, not the order of a query (which follows the tables the entities sit in).
    #[derive(Component)]
    struct Moved;
    let mut app = app();
    let nodes = spawn_arena(&mut app);
    // The first entity changes table: a query now lists it after the others.
    app.world_mut().entity_mut(nodes["Markers/Spawns/PlayerStart"]).insert(Moved);
    app.world_mut().entity_mut(nodes["Shrine"]).insert(Moved);
    let before: Vec<u32> = app.world().resource::<ai_core::AiIndex>().graveyard().map(|t| t.id.0).collect();
    let instance = app.world().resource::<LevelInstances>().iter().next().unwrap().id;
    despawn_level(app.world_mut(), instance);
    let buried: Vec<u32> = app.world().resource::<ai_core::AiIndex>().graveyard().map(|t| t.id.0).skip(before.len()).collect();
    assert_eq!(buried, [1, 2, 3, 4, 5, 6], "the visible nodes of the arena, in file order");
}
