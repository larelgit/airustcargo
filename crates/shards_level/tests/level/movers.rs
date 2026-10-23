//! Movers in level files (F6): `mover=<id>` makes a node a mover, the nodes below it move with
//! it, the keys are in the node's frame, and the file is checked like any other.

use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_level::{FromLevel, PlacedLevels};
use shards_motion::{mover_go, Body, Mover, MoverPart, MoverState};

use crate::common::*;
use crate::gltf::{Gltf, NodeSpec};

const MOVERS: &str = r#"{
    "lift": (keys: [(), (at: (0.0, 4.0, 0.0))], speed: 2.0),
    "slide": (keys: [(), (at: (2.0, 0.0, 0.0))], speed: 2.0),
}"#;

/// A room: a floor, and a lift group (`Lifts`) holding the lift (`mover=lift`), its rail and a
/// lamp below the rail.
fn room(lift_def: &str) -> Gltf {
    let mut g = Gltf::new();
    let cube = g.cube();
    g.root(NodeSpec::new("Floor").at([0.0, -0.5, 0.0]).scale([10.0, 1.0, 10.0]).mesh(cube).props(json!({"collider": "box"})));
    let lifts = g.root(NodeSpec::new("Lifts").at([3.0, 0.0, 0.0]));
    let lift = g.child(lifts, NodeSpec::new("Lift").at([0.0, 0.1, 0.0]).scale([2.0, 0.2, 2.0]).mesh(cube).props(json!({"mover": lift_def, "collider": "box"})));
    let rail = g.child(lift, NodeSpec::new("Rail").at([0.4, 5.0, 0.0]).scale([0.05, 5.0, 0.05]).mesh(cube).props(json!({"collider": "box"})));
    g.child(rail, NodeSpec::new("Lamp").at([0.0, 0.5, 0.0]).props(json!({"marker": "custom:lamp"})));
    g
}

fn movers_app(name: &str, level: &Gltf) -> App {
    let dir = temp_assets(
        name,
        &[
            ("movers.gltf", level.gltf().as_bytes()),
            ("content/levels/movers.level.ron", br#"{ "movers": (file: "movers.gltf") }"#),
            ("content/test.mover.ron", MOVERS.as_bytes()),
        ],
    );
    app_in(&dir)
}

fn pose(app: &App, e: Entity) -> Transform {
    *app.world().get::<Transform>(e).unwrap()
}

#[test]
fn a_mover_node_and_the_nodes_below_it_move_together() {
    let mut app = movers_app("movers_together", &room("lift"));
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    ok(&mut app, "spawn_level", json!({"id": "movers", "x": 10.0}));
    let nodes = by_node(&mut app);
    let (lift, rail, lamp, floor) = (nodes["Lifts/Lift"], nodes["Lifts/Lift/Rail"], nodes["Lifts/Lift/Rail/Lamp"], nodes["Floor"]);
    assert_eq!(app.world().get::<Mover>(lift), Some(&Mover { def: "lift".into() }));
    assert_eq!(app.world().get::<ai_core::AiKind>(lift).map(|k| k.0.as_ref()), Some("Mover"));
    let lift_id = *app.world().get::<AiId>(lift).unwrap();
    for part in [rail, lamp] {
        let linked = app.world().get::<MoverPart>(part).unwrap_or_else(|| panic!("{part} moves with the lift"));
        assert_eq!(linked.mover, lift_id);
        assert!(app.world().get::<AiId>(part).is_some_and(|id| id.0 > 0), "a part has its id");
    }
    assert!(app.world().get::<MoverPart>(floor).is_none() && app.world().get::<Mover>(floor).is_none());
    let rest = [pose(&app, lift), pose(&app, rail), pose(&app, lamp)];
    assert!((rest[0].translation - Vec3::new(13.0, 0.1, 0.0)).length() < 1e-5, "{rest:?}");

    step(&mut app, 2);
    mover_go(app.world_mut(), lift, 1).unwrap();
    step(&mut app, 1);
    assert_eq!(app.world().get::<Body>(rail), Some(&Body::Kinematic), "a moving part is kinematic");
    // 4 m at 2 m/s.
    step(&mut app, 125);
    let state = app.world().get::<MoverState>(lift).unwrap();
    assert!(state.from == 1 && state.to == 1, "{state:?}");
    for (e, was) in [lift, rail, lamp].into_iter().zip(rest) {
        let now = pose(&app, e);
        assert!((now.translation - was.translation - Vec3::Y * 4.0).length() < 1e-4, "{e}: {was:?} -> {now:?}");
        assert!(now.rotation.angle_between(was.rotation) < 1e-5);
    }
    assert_eq!(pose(&app, floor).translation, Vec3::new(10.0, -0.5, 0.0));
    assert_eq!(app.world().get::<Body>(rail), Some(&Body::Static), "at rest it is laid again");
}

#[test]
fn a_mover_moves_in_the_frame_it_was_put_in() {
    // The lift group turned a quarter about Y: the slide's +X key goes along the world's -Z.
    let mut level = room("slide");
    level.node_mut("Lifts").rotation = NodeSpec::new("x").yaw(90.0).rotation;
    let mut app = movers_app("movers_frame", &level);
    ok(&mut app, "spawn_level", json!({"id": "movers"}));
    let nodes = by_node(&mut app);
    let (lift, rail) = (nodes["Lifts/Lift"], nodes["Lifts/Lift/Rail"]);
    let rest = (pose(&app, lift), pose(&app, rail));
    step(&mut app, 2);
    ok(&mut app, "mover_go", json!({"name": "Lift", "key": 1}));
    step(&mut app, 70);
    let moved = pose(&app, lift).translation - rest.0.translation;
    assert!((moved - Vec3::new(0.0, 0.0, -2.0)).length() < 1e-4, "{moved}");
    assert!((pose(&app, rail).translation - rest.1.translation - moved).length() < 1e-4);
}

#[test]
fn a_snapshot_keeps_the_parts_with_their_mover() {
    let mut app = movers_app("movers_snapshot", &room("lift"));
    ok(&mut app, "spawn_level", json!({"id": "movers"}));
    step(&mut app, 2);
    ok(&mut app, "mover_go", json!({"name": "Lift", "key": 1}));
    step(&mut app, 50);
    let saved = ai_core::snapshot::capture(app.world_mut()).expect("capture");
    let run = |app: &mut App| {
        step(app, 100);
        let nodes = by_node(app);
        (pose(app, nodes["Lifts/Lift"]), pose(app, nodes["Lifts/Lift/Rail"]))
    };
    let first = run(&mut app);
    ai_core::snapshot::restore(app.world_mut(), &saved, true).expect("restore");
    let second = run(&mut app);
    assert_eq!(first, second, "the restored lift and its rail end where they did");
    assert!((first.0.translation.y - 4.1).abs() < 1e-4, "{first:?}");
}

#[test]
fn a_mover_names_a_definition_the_content_has() {
    let mut app = movers_app("movers_unknown", &room("lfit"));
    let err = fails(&mut app, "spawn_level", json!({"id": "movers"}));
    assert!(
        err.contains("node `Lifts/Lift`: `mover`: no mover definition `lfit` (did you mean `lift`?); movers: lift, slide"),
        "{err}"
    );
    let info = ok(&mut app, "level_info", json!({"id": "movers"}));
    assert!(info.contains("no mover definition `lfit`"), "{info}");
}

#[test]
fn movers_do_not_nest_and_an_empty_one_is_a_problem() {
    let mut level = room("lift");
    level.node_mut("Rail").extras = Some(json!({"collider": "box", "mover": "slide"}));
    level.node_mut("Floor").extras = Some(json!({"collider": "box", "mover": ""}));
    let mut app = movers_app("movers_nested", &level);
    let err = fails(&mut app, "spawn_level", json!({"id": "movers"}));
    assert!(err.contains("node `Lifts/Lift/Rail`: a mover inside the mover `Lifts/Lift`: movers do not nest"), "{err}");
    assert!(err.contains("node `Floor`: `mover` is empty"), "{err}");
}

#[test]
fn level_despawn_takes_a_placed_level_away() {
    let mut app = movers_app("movers_despawn", &room("lift"));
    ok(&mut app, "spawn_level", json!({"id": "movers"}));
    ok(&mut app, "spawn_level", json!({"id": "movers", "x": 30.0}));
    assert_eq!(app.world().resource::<PlacedLevels>().levels.len(), 2);
    let out = ok(&mut app, "level_despawn", json!({"instance": 1}));
    assert!(out.contains("level taken away: movers (instance 1, 4 entities)"), "{out}");
    assert_eq!(app.world().resource::<PlacedLevels>().levels.len(), 1);
    let out = ok(&mut app, "level_despawn", json!({"id": "movers"}));
    assert!(out.contains("movers (instance 2, 4 entities)"), "{out}");
    assert!(app.world().get_resource::<PlacedLevels>().is_none(), "the last one takes the record with it");
    let world = app.world_mut();
    assert_eq!(world.query::<&FromLevel>().iter(world).count(), 0);
    let err = fails(&mut app, "level_despawn", json!({"id": "movers"}));
    assert!(err.contains("no spawned level matches; no level is spawned"), "{err}");
    let err = fails(&mut app, "level_despawn", json!({}));
    assert!(err.contains("name what goes"), "{err}");
}

/// The picture (`render`): the mover's node in the level's scene is drawn where the mover stands,
/// and the scene's nodes below it come along. The scene is built here by hand, as Bevy's glTF
/// loader builds it (named nodes under the level's picture root): this app has no loader.
#[cfg(feature = "render")]
#[test]
fn a_movers_picture_follows_the_mover() {
    use bevy::world_serialization::WorldAsset;
    use shards_level::{LevelInstances, MoverPicture};

    let dir = temp_assets(
        "movers_picture",
        &[
            ("movers.gltf", room("lift").gltf().as_bytes()),
            ("content/levels/movers.level.ron", br#"{ "movers": (file: "movers.gltf") }"#),
            ("content/test.mover.ron", MOVERS.as_bytes()),
        ],
    );
    let mut app = app_with(&dir, |app| {
        app.init_asset::<WorldAsset>();
        // Global transforms, which a window's app has.
        if !app.is_plugin_added::<TransformPlugin>() {
            app.add_plugins(TransformPlugin);
        }
    });
    ok(&mut app, "spawn_level", json!({"id": "movers", "x": 10.0}));
    let root = app.world().resource::<LevelInstances>().get(1).and_then(|i| i.visual).expect("the picture's root");
    let world = app.world_mut();
    let lifts = world.spawn((Name::new("Lifts"), Transform::from_xyz(3.0, 0.0, 0.0), ChildOf(root))).id();
    let lift = world.spawn((Name::new("Lift"), Transform::from_xyz(0.0, 0.1, 0.0).with_scale(Vec3::new(2.0, 0.2, 2.0)), ChildOf(lifts))).id();
    let rail = world.spawn((Name::new("Rail"), Transform::from_xyz(0.2, 25.0, 0.0), ChildOf(lift))).id();
    for _ in 0..3 {
        app.update();
    }
    assert!(app.world().get::<MoverPicture>(lift).is_some(), "the lift's node is found by its path");
    let global = |app: &App, e: Entity| app.world().get::<GlobalTransform>(e).unwrap().translation();
    let rest = (global(&app, lift), global(&app, rail));
    assert!((rest.0 - Vec3::new(13.0, 0.1, 0.0)).length() < 1e-5, "{rest:?}");
    ok(&mut app, "mover_go", json!({"name": "Lift", "key": 1}));
    step(&mut app, 130);
    app.update();
    let mover = by_node(&mut app)["Lifts/Lift"];
    let now = (global(&app, lift), global(&app, rail));
    assert!((now.0 - pose(&app, mover).translation).length() < 1e-4, "drawn where the mover is: {now:?}");
    assert!((now.1 - rest.1 - Vec3::Y * 4.0).length() < 1e-4, "the rail's picture comes along: {rest:?} -> {now:?}");
}
