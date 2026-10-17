//! The level pipeline on files Blender itself exported: `blender/` holds what
//! `tools/blender/outpost_level.py` writes with Blender 5.2.2 LTS (`outpost.glb`, the same
//! scene as glTF Separate with punctual lights, as glTF Embedded, with Draco compression, with
//! meshopt compression under each of its two extensions, and with an artist's mistakes), and the
//! content that names it. No Blender is needed to run these
//! tests; re-export after changing the script:
//!
//! ```text
//! blender -b --factory-startup --python-exit-code 1 -P tools/blender/outpost_level.py -- crates/shards_level/tests/level/blender
//! ```
//!
//! Positions are the ones the artist sees in Blender, turned from Z-up to Y-up by the
//! exporter: Blender `(x, y, z)` is `(x, z, -y)` here.

use std::path::{Path, PathBuf};

use ai_core::{AiKind, AiLandmark};
use bevy::prelude::*;
use serde_json::json;
use shards_level::{check_level, LevelAsset, LevelMarker, MarkerKind};
use shards_motion::{Collider, CollisionLayers, LayerMask, Shape, Volume, VolumeKind};

use crate::asset::close;
use crate::common::*;

/// The asset folder of the exported files and their content.
fn blender() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/level/blender")
}

fn read(file: &str) -> LevelAsset {
    LevelAsset::read(&blender(), file).unwrap_or_else(|e| panic!("{file}: {e}"))
}

fn position(level: &LevelAsset, path: &str) -> Vec3 {
    level.node_at(path).unwrap_or_else(|| panic!("no node `{path}`")).world.w_axis.truncate()
}

/// The world in which `outpost` (the content id of `outpost.glb`) stands at the origin, with
/// its entities by node path.
fn outpost() -> (App, std::collections::BTreeMap<String, Entity>) {
    let mut app = app_in(&blender());
    ok(&mut app, "spawn_level", json!({"id": "outpost"}));
    let nodes = by_node(&mut app);
    (app, nodes)
}

fn collider(app: &App, e: Entity) -> Collider {
    *app.world().get::<Collider>(e).expect("a collider")
}

fn close_f(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn yaw_of(t: &Transform) -> f32 {
    let (axis, angle) = t.rotation.to_axis_angle();
    if angle.abs() < 1e-4 { 0.0 } else { (angle * axis.y.signum()).to_degrees() }
}

#[test]
fn the_fixtures_are_what_blender_exported() {
    let text = std::fs::read_to_string(blender().join("outpost_lights.gltf")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(doc["asset"]["generator"], "Khronos glTF Blender I/O v5.2.40");
    assert_eq!(doc["buffers"][0]["uri"], "outpost_lights.bin");
    let embedded: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(blender().join("outpost_embedded.gltf")).unwrap()).unwrap();
    assert_eq!(embedded["asset"]["generator"], "Khronos glTF Blender I/O v5.2.40");
    let buffers = embedded["buffers"].as_array().unwrap();
    assert_eq!(buffers.len(), 1);
    assert!(buffers[0]["uri"].as_str().unwrap().starts_with("data:application/octet-stream;base64,"), "the buffer is inside the file");
    for glb in ["outpost.glb", "outpost_draco.glb", "outpost_meshopt.glb", "outpost_meshopt_khr.glb", "outpost_broken.glb"] {
        let bytes = std::fs::read(blender().join(glb)).unwrap();
        assert_eq!(&bytes[..4], b"glTF", "{glb} is glTF Binary");
    }
}

#[test]
fn nodes_are_named_by_their_path_through_the_blender_hierarchy() {
    let level = read("outpost.glb");
    let paths: Vec<&str> = level.nodes.iter().map(|n| n.path.as_str()).collect();
    // The default scene only, in file order. The exporter writes the collection instanced by
    // `Barrels.A` and `Barrels.B` below each of them, objects of the same name; the scene
    // the collection lives in (`Kit`, scene 0) is not the level. The sun and the camera are
    // not exported with the default options.
    assert_eq!(
        paths,
        [
            "Barrels.A/Barrel",
            "Barrels.A",
            "Barrels.B/Barrel",
            "Barrels.B",
            "Boulder",
            "Crate.001",
            "Edge E",
            "Edge N",
            "Edge S",
            "Goal Zone",
            "Ground",
            "Guard",
            "Ladder Bottom",
            "Ladder Top",
            "Lamp Post",
            "Lever",
            "Markers/Костёр",
            "Markers",
            "Player Start",
            "Pond",
            "Rock",
            "Ruins/Shrine",
            "Ruins/Säule",
            "Ruins",
            "Spire",
            "Totem Pole",
            "Vendor",
            "Vent",
            "Wall North",
        ]
    );
    assert_eq!(level.scene, Some(1), "the scene the file names as its default");
    assert_eq!(level.extras["theme"], "outpost", "the scene's custom properties");
    assert_eq!(level.node("Säule").unwrap().mesh.as_deref(), Some("Pillar"), "the mesh data's own name");
}

#[test]
fn the_glb_and_the_separate_gltf_read_alike() {
    let (glb, gltf) = (read("outpost.glb"), read("outpost_lights.gltf"));
    // The separate file has the sun too (punctual lights ticked); nothing else differs.
    let others: Vec<_> = gltf.nodes.iter().filter(|n| n.name != "Sun").collect();
    assert_eq!(others.len(), glb.nodes.len());
    for (a, b) in glb.nodes.iter().zip(others) {
        assert_eq!((&a.path, &a.extras, &a.mesh, a.bounds), (&b.path, &b.extras, &b.mesh, b.bounds));
        assert!(a.world.abs_diff_eq(b.world, 1e-6), "{}", a.path);
    }
}

#[test]
fn the_glb_and_the_embedded_gltf_read_and_spawn_alike() {
    // glTF Embedded: the buffer is a base64 `data:` URI inside the `.gltf`, nothing beside it.
    let (glb, embedded) = (read("outpost.glb"), read("outpost_embedded.gltf"));
    assert_eq!(embedded.nodes.len(), glb.nodes.len());
    for (a, b) in glb.nodes.iter().zip(&embedded.nodes) {
        assert_eq!((&a.path, &a.extras, &a.mesh, a.bounds), (&b.path, &b.extras, &b.mesh, b.bounds));
        assert!(a.world.abs_diff_eq(b.world, 1e-6), "{}", a.path);
    }
    let (a, b) = (glb.markers(), embedded.markers());
    assert_eq!(a.len(), b.len());
    for (a, b) in a.iter().zip(&b) {
        assert_eq!((&a.kind, &a.name, &a.node), (&b.kind, &b.name, &b.node));
        assert!(a.position.abs_diff_eq(b.position, 1e-5), "{}", a.node);
    }
    let mut app = app_in(&blender());
    let info = ok(&mut app, "level_info", json!({"id": "outpost_embedded"}));
    assert!(info.starts_with("level outpost_embedded: outpost_embedded.gltf, at (0.0, 0.0)\n29 nodes, 10 markers\n"), "{info}");
    assert!(info.ends_with("checks: ok"), "{info}");
    let placed = ok(&mut app, "spawn_level", json!({"id": "outpost_embedded", "x": 50.0}));
    assert!(placed.starts_with("spawned level `outpost_embedded` as instance 1 at (50.0, 0.0): 22 entities"), "{placed}");
}

#[test]
fn world_transforms_are_blender_turned_to_y_up() {
    let level = read("outpost.glb");
    // Blender (x, y, z) -> (x, z, -y).
    let expect = [
        ("Ground", Vec3::new(0.0, -0.25, 0.0)),
        ("Player Start", Vec3::new(0.0, 0.0, 8.0)),
        ("Ladder Top", Vec3::new(10.0, 3.0, -8.0)),
        ("Lever", Vec3::new(4.0, 1.0, -9.0)),
        // Parented with Ctrl+P (the parent inverse) under a turned, scaled empty: where it
        // was before parenting.
        ("Ruins/Säule", Vec3::new(8.0, 2.0, 6.0)),
        ("Ruins/Shrine", Vec3::new(6.0, 0.0, 8.0)),
        // Parented without inverse: (2, 0, 0) in an empty turned 90 degrees at (-8, -8, 0).
        ("Markers/Костёр", Vec3::new(-8.0, 0.0, 6.0)),
        // Collection instances: the kit's (0, 0, 0.5) in each instancer's space.
        ("Barrels.A/Barrel", Vec3::new(5.0, 0.5, 0.0)),
        ("Barrels.B/Barrel", Vec3::new(5.0, 0.5, -2.0)),
    ];
    for (path, want) in expect {
        let have = position(&level, path);
        assert!(close(have, want), "{path}: {have} != {want}");
    }
    // The pillar's scale (0.5, 0.5, 2) in Blender is (0.5, 2, 0.5) in the world.
    let (scale, rotation, _) = level.node_at("Ruins/Säule").unwrap().world.to_scale_rotation_translation();
    assert!(close(scale, Vec3::new(0.5, 2.0, 0.5)), "{scale}");
    assert!(rotation.angle_between(Quat::IDENTITY) < 1e-4);
    // The array modifier is applied: the wall's mesh is four blocks long.
    let wall = level.node("Wall North").unwrap().bounds.unwrap();
    assert!(close(wall.min, Vec3::new(-1.0, -1.0, -1.0)) && close(wall.max, Vec3::new(7.0, 1.0, 1.0)), "{wall:?}");
}

#[test]
fn colliders_have_the_size_the_artist_sees() {
    let (app, nodes) = outpost();
    let at = |path: &str| *app.world().get::<Transform>(nodes[path]).unwrap();

    // A cube scaled (12, 12, 0.25) in Blender: 24 x 0.5 x 24 m.
    let ground = collider(&app, nodes["Ground"]);
    assert_eq!(ground.shape, Shape::Cuboid { half: Vec3::new(12.0, 0.25, 12.0) });
    assert_eq!((ground.layers, at("Ground").scale), (CollisionLayers::WORLD, Vec3::ONE));

    // Four blocks along Blender X, scaled (1, 0.25, 1.5): 8 m long, 3 m tall, 0.5 m thick,
    // its middle 3 m from the object's origin.
    let wall = collider(&app, nodes["Wall North"]);
    assert_eq!(wall.shape, Shape::Cuboid { half: Vec3::new(4.0, 1.5, 0.25) });
    assert!(close(wall.offset, Vec3::new(3.0, 0.0, 0.0)));
    assert!(close(at("Wall North").translation + wall.offset, Vec3::new(0.0, 1.5, -10.0)));

    // A cylinder of radius 1 and depth 2, scaled (0.5, 0.5, 2) and parented: 0.5 m by 4 m.
    let pillar = collider(&app, nodes["Ruins/Säule"]);
    let Shape::Cylinder { half_height, radius } = pillar.shape else { panic!("{:?}", pillar.shape) };
    assert!(close_f(half_height, 2.0) && close_f(radius, 0.5), "{:?}", pillar.shape);
    assert!(close(at("Ruins/Säule").translation, Vec3::new(8.0, 2.0, 6.0)));

    // `size = [2, 2, 2]` (a float array) on a cube scaled 0.5; `layers` a list of names.
    let crate_ = collider(&app, nodes["Crate.001"]);
    assert_eq!(crate_.shape, Shape::Cuboid { half: Vec3::splat(0.5) });
    assert_eq!(crate_.layers, CollisionLayers::new(LayerMask::WORLD | LayerMask::game(3), LayerMask::ALL));

    // The other shapes take their sizes from the mesh: a UV sphere of radius 1, a cylinder of
    // radius 0.3 and depth 3 as a capsule (3 m tall, caps included), a cone of radius 1, depth 2.
    let Shape::Sphere { radius } = collider(&app, nodes["Boulder"]).shape else { panic!("Boulder") };
    assert!(close_f(radius, 1.0), "{radius}");
    let totem = collider(&app, nodes["Totem Pole"]).shape;
    let Shape::Capsule { half_height, radius } = totem else { panic!("{totem:?}") };
    assert!(close_f(radius, 0.3) && close_f(half_height + radius, 1.5), "{totem:?}");
    let spire = collider(&app, nodes["Spire"]).shape;
    let Shape::Cone { half_height, radius } = spire else { panic!("{spire:?}") };
    assert!(close_f(half_height, 1.0) && close_f(radius, 1.0), "{spire:?}");
    assert!(close(at("Spire").translation, Vec3::new(-10.0, 1.0, -10.0)));

    // Both instances of the barrel collection, the second turned with its instancer.
    for path in ["Barrels.A/Barrel", "Barrels.B/Barrel"] {
        let barrel = collider(&app, nodes[path]);
        let Shape::Cylinder { half_height, radius } = barrel.shape else { panic!("{:?}", barrel.shape) };
        assert!(close_f(half_height, 0.5) && close_f(radius, 0.4), "{path}: {:?}", barrel.shape);
    }
    assert!(close_f(yaw_of(&at("Barrels.B/Barrel")), 90.0));
}

#[test]
fn volumes_and_the_trigger_are_sensors() {
    let (app, nodes) = outpost();
    let volume = |path: &str| app.world().get::<Volume>(nodes[path]).expect("a volume").kind.clone();

    assert_eq!(volume("Pond"), VolumeKind::Water);
    assert_eq!(collider(&app, nodes["Pond"]).shape, Shape::Cuboid { half: Vec3::new(3.0, 0.5, 2.0) });
    // `accel = 12` is an integer property, `max_speed = 8.0` a float.
    assert_eq!(volume("Vent"), VolumeKind::Updraft { accel: 12.0, max_speed: 8.0 });
    let vent = collider(&app, nodes["Vent"]);
    let Shape::Cylinder { half_height, radius } = vent.shape else { panic!("{:?}", vent.shape) };
    assert!(close_f(half_height, 3.0) && close_f(radius, 1.0));
    assert_eq!(vent.layers, CollisionLayers::VOLUME);

    assert_eq!(volume("Goal Zone"), VolumeKind::Trigger { name: "goal".into() });
    assert_eq!(collider(&app, nodes["Goal Zone"]).shape, Shape::Cuboid { half: Vec3::new(2.0, 1.0, 2.0) });
}

#[test]
fn every_marker_kind_comes_through() {
    let (app, nodes) = outpost();
    let marker = |path: &str| app.world().get::<LevelMarker>(nodes[path]).cloned().unwrap_or_else(|| panic!("{path}: no marker"));
    let kind = |path: &str| (marker(path).kind, marker(path).name);

    assert_eq!(kind("Player Start"), (MarkerKind::SpawnPoint, "start".into()));
    assert_eq!(kind("Ruins/Shrine"), (MarkerKind::Landmark, "Old Shrine".into()));
    assert_eq!(kind("Markers/Костёр"), (MarkerKind::Camp, "bandits".into()));
    assert_eq!(kind("Lever"), (MarkerKind::Interactable, "lever".into()));
    assert_eq!(kind("Goal Zone"), (MarkerKind::Trigger, "goal".into()));
    assert_eq!(kind("Edge N"), (MarkerKind::Socket, "road".into()));
    assert_eq!(kind("Edge S"), (MarkerKind::Socket, "road".into()));
    assert_eq!(kind("Edge E"), (MarkerKind::Socket, "river".into()));
    assert_eq!(kind("Vendor"), (MarkerKind::Custom("vendor".into()), "Vendor".into()));
    // `to` is a Data-Block property: the exporter writes the object as {"name", "type"}.
    assert_eq!(kind("Ladder Bottom"), (MarkerKind::NavLink, "ladder".into()));
    assert!(close(marker("Ladder Bottom").to.unwrap(), Vec3::new(10.0, 3.0, -8.0)));

    // A marker's facing is the empty's: turned 90 degrees about Blender Z, about Y here.
    let start = app.world().get::<Transform>(nodes["Player Start"]).unwrap();
    assert!(close_f(yaw_of(start), 90.0), "{start:?}");
    assert!(close_f(yaw_of(app.world().get::<Transform>(nodes["Ruins/Shrine"]).unwrap()), 30.0));

    // The AI's view of them.
    let ai = |path: &str| app.world().get::<AiKind>(nodes[path]).map(|k| k.0.to_string());
    assert_eq!(ai("Player Start").as_deref(), Some("SpawnPoint"));
    assert_eq!(ai("Ruins/Shrine").as_deref(), Some("Old Shrine"));
    assert!(app.world().get::<AiLandmark>(nodes["Ruins/Shrine"]).is_some());

    // Groups, plain meshes, link ends and instancers spawn nothing.
    for path in ["Ruins", "Markers", "Barrels.A", "Barrels.B", "Ladder Top", "Lamp Post", "Rock"] {
        assert!(!nodes.contains_key(path), "{path} should not spawn");
    }
    assert_eq!(nodes.len(), 22);
}

#[test]
fn prefabs_and_components_come_from_every_kind_of_property() {
    let (app, nodes) = outpost();
    let w = app.world();

    // `prefab = bandit` with a dict property (`Health = {"current": 10.0}`, the panel's
    // Python type) merged into the prefab's, and a text `Team = outlaws`.
    let guard = nodes["Guard"];
    assert_eq!(w.get::<Name>(guard).map(Name::as_str), Some("Bandit"));
    assert_eq!(w.get::<Health>(guard), Some(&Health { current: 10.0, max: 40.0 }));
    assert_eq!(w.get::<Team>(guard), Some(&Team("outlaws".into())));
    assert!(close_f(yaw_of(w.get::<Transform>(guard).unwrap()).abs(), 180.0));

    // JSON typed as text, and a boolean property for a component without fields.
    let crate_ = nodes["Crate.001"];
    assert_eq!(w.get::<Health>(crate_), Some(&Health { current: 5.0, max: 5.0 }));
    assert!(w.get::<Locked>(crate_).is_some());

    // A dict with a nested list of text, on the object inside the instanced collection.
    for path in ["Barrels.A/Barrel", "Barrels.B/Barrel"] {
        let loot = Loot { table: "barrel".into(), rolls: 1, tags: vec!["junk".into(), "wood".into()] };
        assert_eq!(w.get::<Loot>(nodes[path]), Some(&loot), "{path}");
    }
}

#[test]
fn content_check_and_level_info_read_the_real_export() {
    let mut app = app_in(&blender());
    assert_eq!(ok(&mut app, "content_check", json!({})), "ok: 3 definitions in 2 files");
    let info = ok(&mut app, "level_info", json!({"id": "outpost"}));
    assert!(info.starts_with("level outpost: outpost.glb, at (0.0, 0.0)\n29 nodes, 10 markers\n"), "{info}");
    assert!(info.contains("camp \"bandits\" at (-8.0, 6.0)  Markers/Костёр"), "{info}");
    assert!(info.ends_with("checks: ok"), "{info}");

    // Placed by the tool, and replaced from the file in place.
    let placed = ok(&mut app, "spawn_level", json!({"id": "outpost", "x": 50.0}));
    assert!(placed.starts_with("spawned level `outpost` as instance 1 at (50.0, 0.0): 22 entities"), "{placed}");
    let ground = |app: &mut App| by_node(app)["Ground"];
    let before = ground(&mut app);
    ok(&mut app, "level_reload", json!({"id": "outpost"}));
    let after = ground(&mut app);
    assert_ne!(before, after, "the entities are new");
    assert_eq!(app.world().get::<Transform>(after).unwrap().translation, Vec3::new(50.0, -0.25, 0.0));
}

#[test]
fn a_file_with_punctual_lights_reads_without_the_render_feature() {
    // Blender marks `KHR_lights_punctual` required; the light is a node that spawns nothing.
    let level = read("outpost_lights.gltf");
    let sun = level.node("Sun").unwrap();
    assert!(sun.extras.is_empty() && sun.mesh.is_none());
    let mut app = app_in(&blender());
    let spawned = shards_level::spawn_level(app.world_mut(), &level, Transform::default()).unwrap();
    assert_eq!(spawned.len(), 22);
}

#[test]
fn draco_compression_is_refused_with_the_fix() {
    let err = LevelAsset::read(&blender(), "outpost_draco.glb").unwrap_err();
    assert!(err.contains("KHR_draco_mesh_compression"), "{err}");
    assert!(err.contains("Draco Compression"), "the fix: {err}");
}

#[test]
fn meshopt_compression_is_refused_with_the_fix_under_either_extension() {
    // *Data → Meshopt Compression* writes `EXT_meshopt_compression` by default and
    // `KHR_meshopt_compression` when its *Meshopt Extension* says so; both keep `POSITION`
    // bounds, so only the refusal stops a level Bevy cannot draw.
    for (glb, extension) in
        [("outpost_meshopt.glb", "EXT_meshopt_compression"), ("outpost_meshopt_khr.glb", "KHR_meshopt_compression")]
    {
        let bytes = std::fs::read(blender().join(glb)).unwrap();
        let doc = gltf::Gltf::from_slice_without_validation(&bytes).unwrap().document.into_json();
        assert_eq!(doc.extensions_required, [extension], "{glb} is compressed: Blender found its meshopt library");
        let err = LevelAsset::read(&blender(), glb).unwrap_err();
        assert!(err.contains(&format!("`{extension}`")), "{glb}: {err}");
        assert!(err.contains("untick *Data → Meshopt Compression*"), "{glb}, the fix: {err}");
    }
}

#[test]
fn the_mistakes_of_a_broken_export_name_their_nodes() {
    let level = read("outpost_broken.glb");
    let app = app_in(&blender());
    let registry = app.world().resource::<AppTypeRegistry>().read();
    let problems: Vec<String> = check_level(&level, &registry, None, &[]).iter().map(ToString::to_string).collect();
    let expect = [
        "node `Barrels.A/Barrel`: unknown component `Lot`",
        "node `Barrels.B/Barrel`: unknown component `Lot`",
        "node `Crate.001`: unknown component `Helth` (did you mean `Health`?)",
        "node `Ladder Bottom`: navlink `to` names no node: `Ladder Tpo` (did you mean `Ladder Top`?)",
        "node `Rock`: custom properties on mesh `Rock` are not read: `collider`;",
        "node `Ruins/Säule`: unknown collider `cylindre` (did you mean `cylinder`?)",
    ];
    for want in expect {
        assert!(problems.iter().any(|p| p.starts_with(want)), "no `{want}` in {problems:#?}");
    }
    assert_eq!(problems.len(), expect.len(), "{problems:#?}");
    drop(registry);

    // Spawning refuses it whole, naming the same nodes.
    let mut app = app;
    let err = shards_level::spawn_level(app.world_mut(), &level, Transform::default()).unwrap_err();
    assert!(err.starts_with("level `outpost_broken.glb` has 6 problems:"), "{err}");
    assert!(err.contains("\n  node `Ruins/Säule`: ") && !err.contains("Markers/Костёр"), "{err}");
}

#[test]
fn a_broken_re_export_is_refused_and_the_fixed_one_replaces_the_level() {
    let file = |path: &str| std::fs::read(blender().join(path)).unwrap();
    let (good, broken) = (file("outpost.glb"), file("outpost_broken.glb"));
    let (level, prefab) = (file("content/levels/outpost.level.ron"), file("content/props/bandit.prefab.ron"));
    let dir = temp_assets(
        "blender_reload",
        &[("outpost.glb", &good), ("content/levels/outpost.level.ron", &level), ("content/props/bandit.prefab.ron", &prefab)],
    );
    let mut app = app_in(&dir);
    ok(&mut app, "spawn_level", json!({"id": "outpost"}));
    let ground = by_node(&mut app)["Ground"];

    // The artist exports the version with mistakes over the file: the asset server reloads it,
    // and the level stays as it was.
    std::fs::write(dir.join("outpost.glb"), &broken).unwrap();
    app.world().resource::<AssetServer>().reload("outpost.glb");
    update_until(&mut app, "the refusal", |app| errors(app).iter().any(|e| e.contains("kept as it was")));
    let refusal = errors(&app).into_iter().find(|e| e.contains("kept as it was")).unwrap();
    assert!(refusal.contains("outpost (instance 1) kept as it was"), "{refusal}");
    assert!(refusal.contains("node `Crate.001`: unknown component `Helth` (did you mean `Health`?)"), "{refusal}");
    assert_eq!(by_node(&mut app)["Ground"], ground);

    // Fixed and exported again: the instance follows the file.
    std::fs::write(dir.join("outpost.glb"), &good).unwrap();
    app.world().resource::<AssetServer>().reload("outpost.glb");
    update_until(&mut app, "the fixed file", |app| journal(app).iter().any(|l| l.contains("level reloaded: outpost (instance 1, 22 entities)")));
    assert_ne!(by_node(&mut app)["Ground"], ground, "replaced");
}
