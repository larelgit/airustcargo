//! Markers: spawn points, sockets, navlinks, triggers, interactables, camps, landmarks.

use ai_core::{AiKind, AiLandmark, AiVisible};
use bevy::prelude::*;
use serde_json::json;
use shards_level::{check_level, LevelAsset, LevelMarker, MarkerKind, RequiredMarker};
use shards_motion::{Collider, CollisionLayers, Volume, VolumeKind};

use crate::asset::close;
use crate::common::*;
use crate::gltf::{Gltf, NodeSpec};

fn marker(app: &App, e: Entity) -> LevelMarker {
    app.world().get::<LevelMarker>(e).cloned().expect("a marker")
}

#[test]
fn every_kind_of_marker_becomes_a_component() {
    let mut app = app();
    let nodes = spawn_arena(&mut app);
    let kind = |path: &str| (marker(&app, nodes[path]).kind, marker(&app, nodes[path]).name);

    assert_eq!(kind("Markers/Spawns/PlayerStart"), (MarkerKind::SpawnPoint, "start".into()));
    assert_eq!(kind("Shrine"), (MarkerKind::Landmark, "Shrine".into()));
    assert_eq!(kind("GoalPad"), (MarkerKind::Trigger, "goal".into()));
    assert_eq!(kind("Gate"), (MarkerKind::Interactable, "gate".into()));
    assert_eq!(kind("BanditCamp"), (MarkerKind::Camp, "bandits".into()));
    // `socket = road` is the shorthand: the value is the socket type.
    assert_eq!(kind("Edge.N"), (MarkerKind::Socket, "road".into()));
    assert_eq!(kind("Edge.S"), (MarkerKind::Socket, "road".into()));
    assert_eq!(kind("Edge.E"), (MarkerKind::Socket, "river".into()));
    assert_eq!(kind("LadderBottom"), (MarkerKind::NavLink, "ladder".into()));

    // Nodes with nothing for gameplay spawn nothing: groups, plain meshes, link ends.
    for path in ["Pillars", "Markers", "Markers/Spawns", "Lamp", "LadderTop"] {
        assert!(!nodes.contains_key(path), "{path} should not spawn");
    }
}

#[test]
fn the_ai_sees_the_markers_that_matter() {
    let mut app = app();
    let nodes = spawn_arena(&mut app);
    let w = app.world();
    let kind = |path: &str| w.get::<AiKind>(nodes[path]).map(|k| k.0.to_string());
    assert_eq!(kind("Markers/Spawns/PlayerStart").as_deref(), Some("SpawnPoint"));
    assert_eq!(kind("GoalPad").as_deref(), Some("Trigger"));
    assert_eq!(kind("Gate").as_deref(), Some("Interactable"));
    assert_eq!(kind("BanditCamp").as_deref(), Some("Camp"));
    // A landmark is the compact LANDMARKS line, named by the marker.
    assert_eq!(kind("Shrine").as_deref(), Some("Shrine"));
    assert!(w.get::<AiLandmark>(nodes["Shrine"]).is_some());
    // Sockets and navlinks are plumbing: no id, not in `observe`.
    for path in ["Edge.N", "Edge.S", "Edge.E", "LadderBottom", "Floor", "Pillars/Pillar.001"] {
        assert!(w.get::<AiVisible>(nodes[path]).is_none(), "{path}");
    }
    let id = app.world().get::<ai_core::AiId>(nodes["Markers/Spawns/PlayerStart"]).unwrap().to_string();
    let marked = ok(&mut app, "inspect", json!({"id": id}));
    assert!(marked.contains("LevelMarker") && marked.contains("SpawnPoint"), "{marked}");
}

#[test]
fn a_marker_stands_where_the_file_puts_it() {
    let mut app = app();
    let nodes = spawn_arena(&mut app);
    let start = app.world().get::<Transform>(nodes["Markers/Spawns/PlayerStart"]).unwrap();
    assert!(close(start.translation, Vec3::new(-7.0, 0.0, 6.0)), "{}", start.translation);
    assert_eq!(start.scale, Vec3::ONE);
    // Its forward (-Z) is local -Z turned a quarter: world -X.
    assert!(close(*start.forward(), Vec3::NEG_X), "{}", start.forward());
}

#[test]
fn a_navlink_knows_where_it_ends_in_the_world() {
    let mut app = app();
    let nodes = spawn_arena(&mut app);
    assert!(close(marker(&app, nodes["LadderBottom"]).to.unwrap(), Vec3::new(11.0, 3.0, 5.5)));

    // Placed turned and moved, the end goes with it: (11, 3, 5.5) turned 90 degrees about Y is
    // (5.5, 3, -11).
    let mut other = crate::common::app();
    let level = LevelAsset::read(&assets(), "arena.gltf").unwrap();
    let at = Transform::from_xyz(100.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    shards_level::spawn_level(other.world_mut(), &level, at).unwrap();
    let nodes = by_node(&mut other);
    assert!(close(marker(&other, nodes["LadderBottom"]).to.unwrap(), Vec3::new(105.5, 3.0, -11.0)));
    let start = other.world().get::<Transform>(nodes["Markers/Spawns/PlayerStart"]).unwrap();
    assert!(close(start.translation, Vec3::new(106.0, 0.0, 7.0)), "{}", start.translation);
}

#[test]
fn a_navlink_needs_a_real_end() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"marker": "navlink"}))).unwrap_err();
    assert!(err.contains("a navlink needs `to`: the name of the node where it ends"), "{err}");
    let mut g = Gltf::new();
    g.root(NodeSpec::new("Top"));
    g.root(NodeSpec::new("Bottom").props(json!({"marker": "navlink", "to": "Tp"})));
    let err = spawn(&mut app, &g).unwrap_err();
    assert!(err.contains("node `Bottom`: navlink `to` names no node: `Tp` (did you mean `Top`?)"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "spawn", "to": "x"}))).unwrap_err();
    assert!(err.contains("`to` does nothing on this node: it is where a navlink ends"), "{err}");
}

#[test]
fn a_navlink_can_point_at_the_object_itself() {
    // Blender's Data-Block property: the exporter writes the object as {"name", "type"}.
    let mut app = app();
    let mut g = Gltf::new();
    g.root(NodeSpec::new("Top").at([0.0, 3.0, 0.0]));
    g.root(NodeSpec::new("Bottom").props(json!({"marker": "navlink", "to": {"name": "Top", "type": "Object"}})));
    let e = spawn(&mut app, &g).unwrap()[0];
    assert!(close(marker(&app, e).to.unwrap(), Vec3::new(0.0, 3.0, 0.0)));

    // Another kind of data-block, or something that names nothing: one problem each.
    let err = spawn(&mut app, &one(json!({"marker": "navlink", "to": {"name": "Stone", "type": "Material"}}))).unwrap_err();
    assert!(err.starts_with("level `test.gltf` has 1 problem:"), "{err}");
    assert!(err.contains("`to` points at the Material `Stone`; pick the object (Data-Block type Object)"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "navlink", "to": 3}))).unwrap_err();
    assert!(err.starts_with("level `test.gltf` has 1 problem:"), "{err}");
    assert!(err.contains("`to` must be a node's name or the object itself (a Data-Block property), found a number"), "{err}");
}

#[test]
fn marker_words_are_checked() {
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"marker": "spawn", "socket": "road"}))).unwrap_err();
    assert!(err.contains("`socket` makes a socket marker, but `marker` says `spawn`"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": 3}))).unwrap_err();
    assert!(err.contains("`marker` must be text, found a number"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "vendor"}))).unwrap_err();
    assert!(err.contains("unknown marker kind `vendor`") && err.contains("custom:<name>"), "{err}");

    // A kind of the game's own; the node's name when no `name` is given.
    let e = spawn(&mut app, &one(json!({"marker": "custom:vendor"}))).unwrap()[0];
    assert_eq!((marker(&app, e).kind, marker(&app, e).name), (MarkerKind::Custom("vendor".into()), "Thing".into()));
    assert!(app.world().get::<AiVisible>(e).is_none());
    let e = spawn(&mut app, &one(json!({"marker": "socket"}))).unwrap()[0];
    assert_eq!(marker(&app, e).name, "Thing");
}

#[test]
fn a_marker_the_line_does_not_name_says_so_in_its_fragment() {
    let mut app = app();
    // A vendor the game made visible with a kind of its own: the line says `Vendor`, the
    // fragment says the marker.
    let e = spawn(&mut app, &one(json!({"marker": "custom:vendor", "name": "shop", "AiKind": "Vendor"}))).unwrap()[0];
    let facts = ai_core::describe::facts(app.world(), e).unwrap();
    assert_eq!(facts.fragment("marker"), Some("custom:vendor \"shop\""), "{facts:?}");
    // A spawn point's own kind already says it.
    let nodes = spawn_arena(&mut app);
    let facts = ai_core::describe::facts(app.world(), nodes["Markers/Spawns/PlayerStart"]).unwrap();
    assert_eq!((facts.label.as_str(), facts.fragment("marker")), ("SpawnPoint \"start\"", None));
    let facts = ai_core::describe::facts(app.world(), nodes["Shrine"]).unwrap();
    assert_eq!((facts.label.as_str(), facts.fragment("marker")), ("Shrine", None));
}

#[test]
fn a_trigger_volume_is_a_trigger_marker_and_the_other_way_round() {
    let mut app = app();
    // `volume = trigger` alone is a trigger marker named by the node...
    let a = spawn(&mut app, &one(json!({"volume": "trigger"}))).unwrap()[0];
    assert_eq!((marker(&app, a).kind, marker(&app, a).name), (MarkerKind::Trigger, "Thing".into()));
    assert_eq!(app.world().get::<Volume>(a).map(|v| v.kind.clone()), Some(VolumeKind::Trigger { name: "Thing".into() }));
    // ... and `marker = trigger` alone has the volume.
    let b = spawn(&mut app, &one(json!({"marker": "trigger", "name": "goal"}))).unwrap()[0];
    assert_eq!(app.world().get::<Volume>(b).map(|v| v.kind.clone()), Some(VolumeKind::Trigger { name: "goal".into() }));
    assert_eq!(app.world().get::<Collider>(b).unwrap().layers, CollisionLayers::VOLUME);
    // They cannot be something else too.
    let err = spawn(&mut app, &one(json!({"marker": "trigger", "volume": "water"}))).unwrap_err();
    assert!(err.contains("`marker` is `trigger`, so the volume is a trigger, but `volume` says `water`"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "camp", "volume": "trigger"}))).unwrap_err();
    assert!(err.contains("a trigger volume is a `trigger` marker"), "{err}");
}

#[test]
fn the_file_lists_its_markers_without_a_world() {
    let level = LevelAsset::read(&assets(), "arena.gltf").unwrap();
    let markers = level.markers();
    let kinds: Vec<String> = markers.iter().map(|m| format!("{} {}", m.kind, m.name)).collect();
    assert_eq!(
        kinds,
        [
            "spawn start",
            "landmark Shrine",
            "trigger goal",
            "interactable gate",
            "camp bandits",
            "socket road",
            "socket road",
            "socket river",
            "navlink ladder"
        ]
    );
    assert!(close(markers[0].position, Vec3::new(-7.0, 0.0, 6.0)));
    assert_eq!(markers[0].node, "Markers/Spawns/PlayerStart");
}

#[test]
fn required_markers_that_are_missing_are_found() {
    let app = app();
    let level = LevelAsset::read(&assets(), "arena.gltf").unwrap();
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let need = |kind: &str, name: Option<&str>, min: u32| RequiredMarker { kind: kind.parse().unwrap(), name: name.map(str::to_owned), min };

    let good = [need("spawn", Some("start"), 1), need("socket", Some("road"), 2), need("landmark", None, 1)];
    assert_eq!(check_level(&level, &registry, None, &good), []);

    let bad = [need("spawn", Some("finish"), 1), need("socket", Some("road"), 3), need("camp", None, 1)];
    let problems: Vec<String> = check_level(&level, &registry, None, &bad).iter().map(ToString::to_string).collect();
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert_eq!(
        problems[0],
        "required marker missing: `spawn` named `finish` in `arena.gltf` (found 0, need 1); add the custom property marker = \"spawn\" and name = \"finish\" to an object in Blender"
    );
    assert!(problems[1].contains("`socket` named `road`") && problems[1].contains("(found 2, need 3)"), "{}", problems[1]);
}
