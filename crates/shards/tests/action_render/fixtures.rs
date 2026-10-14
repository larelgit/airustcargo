//! Test content written to a fresh assets folder: combat definitions, an animset and a
//! skinned glTF model in two files that share the skeleton.

use std::path::{Path, PathBuf};

use shards::serde_json::json;

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn floats(data: &mut Vec<u8>, values: &[f32]) {
    data.extend(values.iter().flat_map(|f| f.to_le_bytes()));
}

/// A one-triangle mesh skinned to two bones, `Armature` > `Hips` > `Spine`, next to a `Body`
/// node that carries the mesh. With `animated`, two animations turn the spine about Z: `Idle`
/// by 10 degrees, `Slash` by 90, over one second.
pub fn skinned_gltf(animated: bool) -> String {
    let mut data: Vec<u8> = Vec::new();
    floats(&mut data, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]); // 0..36: positions
    data.extend([0u8, 1, 0, 0].repeat(3)); // 36..48: joints
    for _ in 0..3 {
        floats(&mut data, &[0.5, 0.5, 0.0, 0.0]); // 48..96: weights
    }
    data.extend([0u16, 1, 2, 0].iter().flat_map(|i| i.to_le_bytes())); // 96..104: indices, padded
    for lift in [-1.0f32, -1.5] {
        // 104..232: inverse bind matrices of the two joints
        floats(&mut data, &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, lift, 0.0, 1.0]);
    }
    floats(&mut data, &[0.0, 1.0]); // 232..240: key times
    for degrees in [10.0f32, 90.0] {
        // 240..272 and 272..304: the rotations about Z at the two keys
        let (s, c) = (degrees.to_radians() * 0.5).sin_cos();
        floats(&mut data, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, s, c]);
    }
    assert_eq!(data.len(), 304);
    let mut doc = json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0, 3]}],
        "nodes": [
            {"name": "Armature", "children": [1]},
            {"name": "Hips", "translation": [0.0, 1.0, 0.0], "children": [2]},
            {"name": "Spine", "translation": [0.0, 0.5, 0.0]},
            {"name": "Body", "mesh": 0, "skin": 0},
        ],
        "skins": [{"joints": [1, 2], "inverseBindMatrices": 4, "skeleton": 0}],
        "meshes": [{"name": "Body", "primitives": [{"attributes": {"POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2}, "indices": 3}]}],
        "buffers": [{"uri": format!("data:application/octet-stream;base64,{}", base64(&data)), "byteLength": 304}],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 12},
            {"buffer": 0, "byteOffset": 48, "byteLength": 48},
            {"buffer": 0, "byteOffset": 96, "byteLength": 6},
            {"buffer": 0, "byteOffset": 104, "byteLength": 128},
            {"buffer": 0, "byteOffset": 232, "byteLength": 8},
            {"buffer": 0, "byteOffset": 240, "byteLength": 32},
            {"buffer": 0, "byteOffset": 272, "byteLength": 32},
        ],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5121, "count": 3, "type": "VEC4"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC4"},
            {"bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR"},
            {"bufferView": 4, "componentType": 5126, "count": 2, "type": "MAT4"},
            {"bufferView": 5, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0.0], "max": [1.0]},
            {"bufferView": 6, "componentType": 5126, "count": 2, "type": "VEC4"},
            {"bufferView": 7, "componentType": 5126, "count": 2, "type": "VEC4"},
        ],
    });
    if animated {
        let clip = |name: &str, output: u32| {
            json!({
                "name": name,
                "samplers": [{"input": 5, "output": output, "interpolation": "LINEAR"}],
                "channels": [{"sampler": 0, "target": {"node": 2, "path": "rotation"}}],
            })
        };
        doc["animations"] = json!([clip("Idle", 6), clip("Slash", 7)]);
    }
    doc.to_string()
}

pub const ACTIONS: &str = r#"{
    "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20)]),
    "roll": (net_id: 2, phases: [("roll", 0.4)], iframes: [Secs(0.0, 0.18)], displacement: [(0.35, 3.0)], steer: Move),
    "block": (net_id: 3, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
        hold: (input: "block", span: Phase("hold")),
        guard: (span: Phases("raise", "hold"), perfect: Phase("raise"), damage_mult: 0.1, arc: 140), move_speed: 0.4),
    "slam": (net_id: 4, phases: [("windup", 0.8), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Circle(radius: 3.0), damage: 20)],
        telegraph: (symbol: "⚠", sound: "growl")),
}"#;

pub const MOVESETS: &str = r#"{
    "hero": (inputs: {"light": "light_1", "dodge": "roll", "block": "block"}),
    "brute": (inputs: {"light": "slam"}),
}"#;

pub const ANIMSETS: &str = r#"{ "humanoid": (
    source: "models/humanoid.gltf",
    clips: {
        "idle": (name: "Idle", cues: [(at: 0.5, cue: "breath")]),
        "slash": (name: "Slash", cues: [(at: 0.5, cue: "swoosh")]),
    },
    locomotion: {"idle": "idle"},
    actions: {"light_1": "slash"},
) }"#;

/// A fresh assets folder under the test temp dir: content, the model (no animations) and the
/// library (the same skeleton with its clips).
pub fn assets(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        ("content/combat/actions.action.ron", ACTIONS.to_owned()),
        ("content/combat/movesets.moveset.ron", MOVESETS.to_owned()),
        ("content/anim/humanoid.animset.ron", ANIMSETS.to_owned()),
        ("models/model.gltf", skinned_gltf(false)),
        ("models/humanoid.gltf", skinned_gltf(true)),
    ] {
        let path: &Path = path.as_ref();
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), text).unwrap();
    }
    dir
}
