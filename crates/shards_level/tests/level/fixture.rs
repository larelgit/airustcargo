//! The committed fixture level: `assets/arena.gltf` is what `common::arena()` writes, with
//! the content that names it. The sandbox and the template use the same files. Regenerate
//! them after changing the arena:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p shards_level --test level fixture
//! ```

use crate::common::*;

/// Every file of the fixture asset folder: path, and what the code writes there.
fn files() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("arena.gltf", arena().gltf().into_bytes()),
        ("content/levels/arena.level.ron", format!("{ARENA_LEVEL}\n").into_bytes()),
        ("content/props/skeleton.prefab.ron", format!("{SKELETON_PREFAB}\n").into_bytes()),
    ]
}

#[test]
fn the_committed_fixture_is_what_the_builder_writes() {
    let update = std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some();
    for (path, want) in files() {
        let at = assets().join(path);
        if update {
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(&at, &want).unwrap();
        }
        let have = std::fs::read(&at).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", at.display()));
        assert!(
            have == want,
            "{path} is out of date; regenerate with `SHARDS_UPDATE_FIXTURES=1 cargo test -p shards_level --test level fixture`"
        );
    }
}

#[test]
fn the_fixture_is_the_json_blender_would_export() {
    // Custom properties are plain JSON on the node, the buffer is embedded.
    let text = std::fs::read_to_string(assets().join("arena.gltf")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let start = doc["nodes"].as_array().unwrap().iter().find(|n| n["name"] == "PlayerStart").unwrap();
    assert_eq!(start["extras"], serde_json::json!({"marker": "spawn", "name": "start"}));
    assert!(doc["buffers"][0]["uri"].as_str().unwrap().starts_with("data:application/octet-stream;base64,"));
    assert_eq!(doc["asset"]["version"], "2.0");
}
