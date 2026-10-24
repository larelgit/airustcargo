//! Markers: spawn points, sockets, navlinks, triggers, interactables, camps, landmarks.

use ai_core::{AiKind, AiLandmark, AiVisible};
use bevy::prelude::*;
use serde_json::json;
use shards_level::{check_level, LevelAsset, LevelMarker, MarkerKind, RequiredMarker};
use shards_motion::{Collider, CollisionLayers, LayerMask, Usable, Volume, VolumeKind};

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

#[test]
fn an_interactable_is_usable_and_its_node_is_its_use_area() {
    let mut app = app();
    // A mesh and nothing more: a use area on the USE layer, which collides with nothing.
    let lever = spawn(&mut app, &one(json!({"marker": "interactable", "name": "lever"}))).unwrap()[0];
    assert_eq!(app.world().get::<Usable>(lever), Some(&Usable { name: "lever".into(), ..default() }));
    assert_eq!(app.world().get::<Collider>(lever).unwrap().layers, CollisionLayers::USE);
    assert!(!CollisionLayers::USE.interacts_with(&CollisionLayers::CHARACTER) && !CollisionLayers::USE.interacts_with(&CollisionLayers::WORLD));
    // With a collider of its own it is solid and usable: a door is a wall and a target.
    let nodes = spawn_arena(&mut app);
    let gate = app.world().get::<Collider>(nodes["Gate"]).unwrap().layers;
    assert_eq!((gate.memberships, gate.filters), (LayerMask::WORLD | LayerMask::USE, LayerMask::ALL));
    assert_eq!(app.world().get::<Usable>(nodes["Gate"]).map(|u| u.name.as_str()), Some("gate"));
    // The shape comes from the node as a volume's does: `size` shapes a node with no mesh.
    let panel = spawn(&mut app, &bare(NodeSpec::new("Panel").props(json!({"marker": "interactable", "size": [0.5, 0.4, 0.1]})))).unwrap()[0];
    let panel = app.world().get::<Collider>(panel).unwrap();
    assert_eq!((panel.layers, panel.shape.half_extents()), (CollisionLayers::USE, Vec3::new(0.25, 0.2, 0.05)));
    // `layers` names more of it; the USE layer is added whatever it says.
    let both = spawn(&mut app, &one(json!({"marker": "interactable", "collider": "box", "layers": ["game0"]}))).unwrap()[0];
    assert_eq!(app.world().get::<Collider>(both).unwrap().layers.memberships, LayerMask::game(0) | LayerMask::USE);
    // The other markers are not usable and keep their colliders.
    let nodes = by_node(&mut app);
    assert!(app.world().get::<Usable>(nodes["GoalPad"]).is_none() && app.world().get::<Usable>(nodes["Shrine"]).is_none());
}

#[test]
fn a_bare_empty_interactable_is_a_marker_without_a_use_area() {
    // The committed exports have one (the outpost's lever): it loads as it always did, and nothing can look at it.
    let mut app = app();
    let e = spawn(&mut app, &bare(NodeSpec::new("Lever").props(json!({"marker": "interactable", "name": "lever"})))).unwrap()[0];
    assert!(app.world().get::<Usable>(e).is_some() && app.world().get::<Collider>(e).is_none());
    // `observe` says so, so an agent does not wait for a look to meet it.
    let facts = ai_core::describe::facts(app.world(), e).unwrap();
    assert_eq!(facts.fragment("usable"), Some("usable (no use area)"), "{facts:?}");
}

#[test]
fn how_an_interactable_is_used_comes_from_its_properties() {
    let mut app = app();
    let valve = spawn(
        &mut app,
        &one(json!({"marker": "interactable", "name": "valve", "hold_s": 1.5, "use_range": "3", "prompt": "Turn the valve"})),
    )
    .unwrap()[0];
    let usable = app.world().get::<Usable>(valve).unwrap();
    assert_eq!((usable.hold_s, usable.use_range, usable.prompt.as_str()), (1.5, Some(3.0), "Turn the valve"));
    assert_eq!(usable.prompt_text(), "Turn the valve");
    // `observe` says what a look would not: that it takes time, and from how far.
    let facts = ai_core::describe::facts(app.world(), valve).unwrap();
    assert_eq!(facts.fragment("usable"), Some("usable (hold 1.5s, reach 3.0m)"), "{facts:?}");
    // Left out, a press, the body's reach and the name.
    let plain = spawn(&mut app, &one(json!({"marker": "interactable", "name": "switch"}))).unwrap()[0];
    let usable = app.world().get::<Usable>(plain).unwrap();
    assert_eq!((usable.hold_s, usable.use_range, usable.prompt_text()), (0.0, None, "switch"));
    // Zero seconds is a press; every number is checked, and the error names the node and the property.
    spawn(&mut app, &one(json!({"marker": "interactable", "hold_s": 0}))).unwrap();
    let err = spawn(&mut app, &one(json!({"marker": "interactable", "hold_s": -1}))).unwrap_err();
    assert!(err.contains("node `Thing`: `hold_s` must be zero or more (seconds to hold the button), found -1"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "interactable", "use_range": 0}))).unwrap_err();
    assert!(err.contains("node `Thing`: `use_range` must be above zero, found 0"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "interactable", "hold_s": 1e300}))).unwrap_err();
    assert!(err.contains("node `Thing`: `hold_s` is too big for a number here"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "interactable", "use_range": 150}))).unwrap_err();
    assert!(err.contains("node `Thing`: `use_range` must be at most 100 (metres, how far a use is looked for), found 150"), "{err}");
    let err = spawn(&mut app, &one(json!({"marker": "interactable", "hold_s": "long", "prompt": 3}))).unwrap_err();
    assert!(err.contains("`hold_s` must be a number, found text") && err.contains("`prompt` must be text, found a number"), "{err}");
    // They say how a thing is used: on a node that is no interactable they do nothing, and the check says so.
    for key in ["hold_s", "use_range", "prompt"] {
        let err = spawn(&mut app, &one(json!({"marker": "camp", key: 1.0}))).unwrap_err();
        assert!(err.contains(&format!("`{key}` does nothing on this node: it says how an `interactable` is used")), "{key}: {err}");
    }
}

/// A level of the one node `node`, no mesh.
fn bare(node: NodeSpec) -> Gltf {
    let mut g = Gltf::new();
    g.root(node);
    g
}

#[test]
fn a_usable_thing_can_also_be_made_by_naming_the_component() {
    // A node that is no `interactable` marker names the component and the layer itself, as a prefab would: the fields it leaves
    // out are the defaults.
    let mut app = app();
    let e = spawn(&mut app, &one(json!({"Usable": {"name": "tablet", "hold_s": 2.0}, "collider": "box", "layers": ["use"]}))).unwrap()[0];
    assert_eq!(app.world().get::<Usable>(e), Some(&Usable { name: "tablet".into(), hold_s: 2.0, ..default() }));
    assert_eq!(app.world().get::<Collider>(e).unwrap().layers.memberships, LayerMask::USE);
    assert!(app.world().get::<LevelMarker>(e).is_none());
}

#[test]
fn an_interactable_cannot_also_be_a_sensor_volume() {
    // A look never meets a sensor (a ray skips them unless it asks), so the thing could not be used: the check says so.
    let mut app = app();
    let err = spawn(&mut app, &one(json!({"marker": "interactable", "volume": "water"}))).unwrap_err();
    assert!(err.contains("node `Thing`: an `interactable` cannot also be a `volume`"), "{err}");
}
