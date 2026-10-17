//! The level files of the generated region. `assets/tiles/ruin.gltf` is Blender's own export:
//! `tools/blender/ruin_tile.py` builds the tile in Blender 5.2.2 LTS and exports it as glTF
//! Embedded, and the template keeps the same file. [`ruin`] is the tile's spec, written with
//! the glTF writer of `shards_level`'s tests; the test below holds the export to it. After
//! changing the ruin, change both, then:
//!
//! ```text
//! blender -b --factory-startup --python-exit-code 1 -P tools/blender/ruin_tile.py -- crates/sandbox/assets/tiles
//! cp crates/sandbox/assets/tiles/ruin.gltf template/assets/tiles/
//! cargo sandbox --bake-nav ruin        # its navigation, in assets/nav/ (and in template/)
//! ```
//!
//! In Blender the tile is a 16 × 16 m scene centered on the origin (-Y forward in Blender is -Z
//! north here), objects with *Custom Properties* (`collider = auto`, `marker = camp`, `socket =
//! grass`, ...), exported with *Include → Custom Properties*.

use std::path::{Path, PathBuf};

use serde_json::json;
use shards::level::{LevelAsset, LevelNode};

use crate::gltf::{Gltf, NodeSpec};

/// The ruin tile: ground, an L of broken wall with a way in, a fallen block, a column, the
/// camp marker where the tile's camp slot is, a landmark, and its edges' sockets.
pub fn ruin() -> Gltf {
    let mut g = Gltf::new();
    g.scene_props(json!({"tile": "ruin", "size": 16}));
    let cube = g.cube();
    g.root(NodeSpec::new("Ground").at([0.0, -0.5, 0.0]).scale([16.0, 1.0, 16.0]).mesh(cube).props(json!({"collider": "auto"})));
    // Three levels of hierarchy, as an artist groups things: Ruin > Walls > Wall.North.
    let ruin = g.root(NodeSpec::new("Ruin"));
    let walls = g.child(ruin, NodeSpec::new("Walls").at([-1.0, 0.0, 0.0]));
    g.child(walls, NodeSpec::new("Wall.North").at([-2.0, 1.5, -5.0]).scale([6.0, 3.0, 0.6]).mesh(cube).props(json!({"collider": "auto"})));
    g.child(walls, NodeSpec::new("Wall.West").at([-4.0, 1.5, -1.0]).scale([0.6, 3.0, 6.0]).mesh(cube).props(json!({"collider": "auto"})));
    g.child(ruin, NodeSpec::new("Rubble").at([3.0, 0.5, -4.0]).yaw(30.0).scale([2.0, 1.0, 1.2]).mesh(cube).props(json!({"collider": "auto"})));
    g.child(ruin, NodeSpec::new("Column").at([4.0, 2.0, 4.0]).scale([1.0, 4.0, 1.0]).mesh(cube).props(json!({"collider": "auto"})));
    g.root(NodeSpec::new("Camp").at([-2.0, 0.0, 2.0]).props(json!({"marker": "camp", "name": "ruin_camp"})));
    g.root(NodeSpec::new("Arch").at([1.0, 0.0, 1.0]).props(json!({"marker": "landmark", "name": "Ruin"})));
    for (name, at) in [("Edge.N", [0.0, 0.0, -8.0]), ("Edge.E", [8.0, 0.0, 0.0]), ("Edge.S", [0.0, 0.0, 8.0]), ("Edge.W", [-8.0, 0.0, 0.0])] {
        g.root(NodeSpec::new(name).at(at).props(json!({"socket": "grass"})));
    }
    g
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The nodes by path: Blender writes the scene's roots by name, the spec in the order made.
fn by_path(level: &LevelAsset) -> Vec<&LevelNode> {
    let mut nodes: Vec<&LevelNode> = level.nodes.iter().collect();
    nodes.sort_by(|a, b| a.path.cmp(&b.path));
    nodes
}

#[test]
fn the_committed_ruin_is_the_tile_the_spec_describes() {
    let sandbox = root().join("crates/sandbox/assets/tiles/ruin.gltf");
    let bytes = std::fs::read(&sandbox).unwrap();
    assert!(
        std::fs::read(root().join("template/assets/tiles/ruin.gltf")).unwrap() == bytes,
        "the template keeps the sandbox's ruin.gltf: copy it after an export"
    );
    let doc: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(doc["asset"]["generator"], "Khronos glTF Blender I/O v5.2.40", "Blender's export, not the spec's");
    assert!(doc["buffers"][0]["uri"].as_str().unwrap().starts_with("data:application/octet-stream;base64,"), "glTF Embedded");

    let export = LevelAsset::from_slice("tiles/ruin.gltf", &bytes).unwrap();
    let spec = LevelAsset::from_slice("tiles/ruin.gltf", ruin().gltf().as_bytes()).unwrap();
    assert_eq!(export.extras, spec.extras, "the scene's properties");
    let (export, spec) = (by_path(&export), by_path(&spec));
    let paths = |nodes: &[&LevelNode]| nodes.iter().map(|n| n.path.clone()).collect::<Vec<_>>();
    assert_eq!(paths(&export), paths(&spec));
    for (have, want) in export.iter().zip(&spec) {
        let path = &want.path;
        assert_eq!(have.extras, want.extras, "{path}: custom properties");
        assert_eq!(have.mesh.is_some(), want.mesh.is_some(), "{path}: a mesh");
        assert!(have.world.abs_diff_eq(want.world, 1e-5), "{path}: {:?}, the spec has {:?}", have.world, want.world);
        if let (Some(have), Some(want)) = (have.bounds, want.bounds) {
            assert!(have.min.abs_diff_eq(want.min, 1e-6) && have.max.abs_diff_eq(want.max, 1e-6), "{path}: {have:?}, the spec has {want:?}");
        }
    }
}
