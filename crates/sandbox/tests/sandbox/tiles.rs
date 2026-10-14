//! The level files of the generated region, made the way Blender's glTF exporter makes them
//! (there is no Blender here: the writer `shards_level`'s tests use). `assets/tiles/ruin.gltf`
//! is what [`ruin`] builds, and the template keeps the same file. After changing the ruin:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox tiles
//! cargo sandbox --bake-nav ruin        # its navigation, in assets/nav/
//! ```
//!
//! In Blender the same tile is a 16 × 16 m scene centered on the origin (-Y forward in
//! Blender is -Z north here), objects with *Custom Properties* (`collider = auto`, `marker =
//! camp`, `socket = grass`, ...), exported as glTF with *Include → Custom Properties*.

use std::path::{Path, PathBuf};

use serde_json::json;

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

/// The committed level files and what the builders write there.
fn files() -> Vec<(PathBuf, String)> {
    let text = ruin().gltf();
    vec![
        (root().join("crates/sandbox/assets/tiles/ruin.gltf"), text.clone()),
        (root().join("template/assets/tiles/ruin.gltf"), text),
    ]
}

#[test]
fn the_committed_level_files_are_what_the_builder_writes() {
    let update = std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some();
    for (path, want) in files() {
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", path.display()));
        assert!(
            have == want,
            "{} is out of date; regenerate with `SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox tiles`, then bake",
            path.display()
        );
    }
}
