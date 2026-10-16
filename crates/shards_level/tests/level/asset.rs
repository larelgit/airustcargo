//! Reading a level file: the node tree, composed transforms, names, extras, mesh bounds,
//! both file flavors, and the asset loader.

use bevy::prelude::*;
use serde_json::json;
use shards_level::{LevelAsset, LevelNode};

use crate::common::*;
use crate::gltf::{Gltf, NodeSpec};

pub fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

fn position(node: &LevelNode) -> Vec3 {
    node.world.w_axis.truncate()
}

#[test]
fn the_arena_reads_in_file_order_with_paths_and_extras() {
    let level = read(&arena());
    let names: Vec<&str> = level.nodes.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(&names[..7], ["Floor", "Pillars", "Pillar.001", "Pillar.002", "Pillar.003", "Pillar.004", "Markers"]);
    assert_eq!(level.nodes.len(), 23);
    assert!(level.nodes.windows(2).all(|w| w[0].index < w[1].index), "file order");

    let pillar = level.node("Pillar.003").unwrap();
    assert_eq!(pillar.path, "Pillars/Pillar.003");
    assert_eq!(pillar.parent, Some(level.node("Pillars").unwrap().index));
    assert_eq!(pillar.extras["collider"], json!("auto"));
    assert_eq!(level.node_at("Markers/Spawns/PlayerStart").unwrap().name, "PlayerStart");
    assert_eq!(level.extras["theme"], json!("ruins"));
    assert_eq!(level.path, "test.gltf");
}

#[test]
fn transforms_compose_through_three_levels() {
    let level = read(&arena());
    // Markers: (-8, 0, 8) turned 90 degrees about Y; Spawns: (2, 0, 0) in it; PlayerStart: (0, 0, 1)
    // in that. (2, 0, 1) turned 90 degrees about Y is (1, 0, -2).
    let start = level.node("PlayerStart").unwrap();
    assert!(close(position(start), Vec3::new(-7.0, 0.0, 6.0)), "{}", position(start));
    // Its forward axis follows the turn too: local +Z is world +X.
    assert!(close(start.world.transform_vector3(Vec3::Z), Vec3::X));
    // A scaled node under an unturned empty.
    let pillar = level.node("Pillar.001").unwrap();
    assert!(close(position(pillar), Vec3::new(6.0, 2.0, 6.0)));
    assert!(close(pillar.world.transform_vector3(Vec3::Y), Vec3::new(0.0, 4.0, 0.0)));
}

#[test]
fn a_matrix_node_and_a_trs_child_compose() {
    let mut g = Gltf::new();
    #[rustfmt::skip]
    let translate = [1.0, 0.0, 0.0, 0.0,  0.0, 1.0, 0.0, 0.0,  0.0, 0.0, 1.0, 0.0,  10.0, 0.0, -5.0, 1.0];
    let parent = g.root(NodeSpec::new("Parent").matrix(translate));
    g.child(parent, NodeSpec::new("Child").at([0.0, 2.0, 0.0]).scale([2.0, 2.0, 2.0]));
    let level = read(&g);
    let child = level.node("Child").unwrap();
    assert!(close(position(child), Vec3::new(10.0, 2.0, -5.0)));
    assert!(close(child.world.transform_vector3(Vec3::X), Vec3::new(2.0, 0.0, 0.0)));
}

#[test]
fn glb_and_gltf_files_read_the_same() {
    let g = arena();
    let a = LevelAsset::from_slice("a.gltf", g.gltf().as_bytes()).unwrap();
    let b = LevelAsset::from_slice("a.glb", &g.glb()).unwrap();
    assert_eq!(a.nodes.len(), b.nodes.len());
    for (x, y) in a.nodes.iter().zip(&b.nodes) {
        assert_eq!((&x.path, &x.extras, x.world, x.bounds), (&y.path, &y.extras, y.world, y.bounds));
    }
    assert_ne!(a.digest, b.digest, "the digest follows the bytes");
    assert_eq!(a.digest, LevelAsset::from_slice("again.gltf", g.gltf().as_bytes()).unwrap().digest);
}

#[test]
fn the_file_order_is_the_nodes_array_not_the_hierarchy() {
    let mut g = Gltf::new();
    let kid = g.add(NodeSpec::new("Kid"));
    let mut parent = NodeSpec::new("Parent");
    parent.children.push(kid);
    g.root(parent);
    let _orphan = g.add(NodeSpec::new("Orphan"));
    let level = read(&g);
    let names: Vec<(&str, &str)> = level.nodes.iter().map(|n| (n.name.as_str(), n.path.as_str())).collect();
    assert_eq!(names, [("Kid", "Parent/Kid"), ("Parent", "Parent")], "the orphan is in no scene");
}

#[test]
fn unnamed_nodes_get_a_name_from_their_index() {
    let mut g = Gltf::new();
    let root = g.root(NodeSpec::unnamed());
    g.child(root, NodeSpec::unnamed());
    let level = read(&g);
    assert_eq!(level.nodes[1].path, "node0/node1");
}

#[test]
fn mesh_bounds_come_from_the_position_accessor() {
    let mut g = Gltf::new();
    let beam = g.box_mesh("Beam", [-1.0, 0.0, -2.0], [3.0, 1.0, 2.0]);
    g.root(NodeSpec::new("Beam").mesh(beam));
    g.root(NodeSpec::new("Empty"));
    let level = read(&g);
    let bounds = level.node("Beam").unwrap().bounds.unwrap();
    assert!(close(bounds.min, Vec3::new(-1.0, 0.0, -2.0)) && close(bounds.max, Vec3::new(3.0, 1.0, 2.0)));
    assert!(close(bounds.center(), Vec3::new(1.0, 0.5, 0.0)) && close(bounds.half(), Vec3::new(2.0, 0.5, 2.0)));
    assert_eq!(level.node("Beam").unwrap().mesh.as_deref(), Some("Beam"));
    assert!(level.node("Empty").unwrap().bounds.is_none());
}

#[test]
fn a_mesh_without_bounds_is_a_broken_file_not_a_panic() {
    // glTF requires `min` and `max` on POSITION; the reader's validation says where they are missing.
    let mut g = Gltf::new();
    g.without_bounds = true;
    let cube = g.cube();
    g.root(NodeSpec::new("Cube").mesh(cube));
    let err = LevelAsset::from_slice("x.gltf", g.gltf().as_bytes()).unwrap_err();
    assert!(err.starts_with("not a valid glTF file") && err.contains("POSITION") && err.contains("min: Missing data"), "{err}");
}

/// The arena as `.gltf` JSON, changed by `edit`, read back.
fn arena_edited(edit: impl FnOnce(&mut serde_json::Value)) -> Result<LevelAsset, String> {
    let mut doc: serde_json::Value = serde_json::from_str(&arena().gltf()).unwrap();
    edit(&mut doc);
    LevelAsset::from_slice("arena.gltf", doc.to_string().as_bytes())
}

#[test]
fn required_extensions_are_no_reason_to_refuse_a_level_unless_meshes_are_compressed() {
    let nodes = read(&arena()).nodes.len();
    // What Blender marks required: punctual lights, texture mapping, WebP images. None of
    // them changes a node, a name, an extra or a bound; drawing them is Bevy's business.
    let required = ["KHR_lights_punctual", "KHR_texture_transform", "EXT_texture_webp", "KHR_materials_volume"];
    let level = arena_edited(|doc| {
        doc["extensionsUsed"] = json!(required);
        doc["extensionsRequired"] = json!(required);
    })
    .unwrap();
    assert_eq!(level.nodes.len(), nodes);
    // Compressed meshes: the bounds are not in plain buffers and Bevy cannot draw them.
    for (name, fix) in [
        ("KHR_draco_mesh_compression", "untick *Data → Draco Compression*"),
        ("EXT_meshopt_compression", "meshopt compression off"),
        ("KHR_mesh_quantization", "without mesh quantization"),
    ] {
        let err = arena_edited(|doc| doc["extensionsRequired"] = json!(["KHR_lights_punctual", name])).unwrap_err();
        assert!(err.starts_with(&format!("the meshes are compressed with `{name}`")) && err.contains(fix), "{err}");
    }
    // The rest of the validation still runs.
    let err = arena_edited(|doc| doc["nodes"][0]["mesh"] = json!(99)).unwrap_err();
    assert!(err.starts_with("not a valid glTF file") && err.contains("mesh"), "{err}");
}

#[test]
fn the_level_is_the_default_scene_not_the_first() {
    // Blender writes every scene of the `.blend`, sorted by name, and makes the active one
    // the default: a kit of instanced collections in its own scene comes first.
    let level = arena_edited(|doc| {
        let nodes = doc["nodes"].as_array_mut().unwrap();
        nodes.push(json!({"name": "KitPiece", "extras": {"collider": "box", "size": 1}}));
        let kit = json!({"name": "Kit", "nodes": [nodes.len() - 1]});
        let level = doc["scenes"][0].clone();
        doc["scenes"] = json!([kit, level]);
        doc["scene"] = json!(1);
    })
    .unwrap();
    assert_eq!(level.scene, Some(1));
    assert!(level.node("KitPiece").is_none());
    assert_eq!(level.extras["theme"], "ruins", "the default scene's properties");
    assert_eq!(read(&arena()).scene, Some(0));
}

#[test]
fn extras_that_are_not_an_object_are_a_problem_of_the_node() {
    let mut g = Gltf::new();
    g.root(NodeSpec::new("Odd").props(json!("just text")));
    let node = read(&g).nodes.remove(0);
    assert!(node.extras.is_empty());
    assert_eq!(node.extras_problem.as_deref(), Some("extras must be an object of custom properties, found text"));
}

#[test]
fn files_that_are_not_levels_say_so() {
    assert!(LevelAsset::from_slice("x", b"hello").unwrap_err().starts_with("not a valid glTF file"));
    let err = LevelAsset::read(&assets(), "nothing.gltf").unwrap_err();
    assert!(err.contains("cannot read `nothing.gltf`") && err.contains("asset folder"), "{err}");
    for path in ["../secret.gltf", "/etc/passwd", ""] {
        let err = LevelAsset::read(&assets(), path).unwrap_err();
        assert!(err.contains("must be a path inside the asset folder"), "{path}: {err}");
    }
    assert_eq!(LevelAsset::read(&assets(), "arena.gltf").unwrap().path, "arena.gltf");
}

#[test]
fn the_asset_server_loads_a_level_without_taking_gltf_from_bevy() {
    let mut app = app();
    let server = app.world().resource::<AssetServer>().clone();
    let handle = server.load::<LevelAsset>("arena.gltf");
    update_until(&mut app, "the level loads", |app| app.world().resource::<Assets<LevelAsset>>().get(&handle).is_some());
    let level = app.world().resource::<Assets<LevelAsset>>().get(&handle).unwrap();
    assert_eq!(level.path, "arena.gltf");
    assert_eq!(level.nodes.len(), 23);
    // The loader claims no extension: Bevy's own glTF loader keeps `.gltf` and `.glb`.
    for extension in ["gltf", "glb", "level.gltf"] {
        assert!(bevy::tasks::block_on(server.get_asset_loader_with_extension(extension)).is_err(), "{extension}");
    }
    // A file that is not a level fails the load, with the reason.
    let broken = server.load::<LevelAsset>("content/levels/arena.level.ron");
    update_until(&mut app, "the bad file fails", |app| matches!(app.world().resource::<AssetServer>().load_state(&broken), bevy::asset::LoadState::Failed(_)));
}
