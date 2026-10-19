//! Test content written to a fresh assets folder: combat definitions, an animset and a
//! skinned glTF model in two files that share the skeleton; or the humanoid made in Blender
//! (`tools/blender/humanoid.py`, the sandbox's `models/`) with its second file of clips.

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
        let (s, c) = shards::bevy::math::ops::sin_cos(degrees.to_radians() * 0.5);
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
    // Walking on while it lasts: the humanoid test drinks with its arm and keeps the legs' walk.
    "drink": (net_id: 5, phases: [("drink", 1.0), ("recover", 0.3)], move_speed: 0.3),
}"#;

pub const MOVESETS: &str = r#"{
    "hero": (inputs: {"light": "light_1", "dodge": "roll", "block": "block", "drink": "drink"}, stagger: Some("roll"), hit: Some("roll"), death: Some("roll")),
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

/// The humanoid's set: its clips, one (`bow`) from the second file, which the block shows.
pub const HUMANOID: &str = r#"{ "humanoid": (
    source: "models/humanoid.glb",
    clips: {"idle": (), "light_1": (), "roll": (), "bow": (source: "models/humanoid_gestures.glb")},
    locomotion: {"idle": "idle"},
    actions: {"light_1": "light_1", "roll": "roll", "block": "bow"},
    root_motion: "root",
) }"#;

/// The humanoid's set with layers: the walk the legs go on with, and the drink (from the second
/// file) that the arms, the head and the spine take over while the action runs.
pub const HUMANOID_LAYERED: &str = r#"{ "humanoid": (
    source: "models/humanoid.glb",
    clips: {"idle": (), "walk": (), "drink": (source: "models/humanoid_gestures.glb")},
    locomotion: {"idle": "idle", "walk": "walk"},
    layers: {"upper": (root: "spine")},
    actions: {"drink": (clip: "drink", layer: "upper")},
    root_motion: "root",
) }"#;

/// A fresh assets folder with the combat content, the humanoid (`models/humanoid.glb`) and its
/// clips for the same skeleton in a second file (`models/humanoid_gestures.glb`).
pub fn humanoid_assets(name: &str) -> PathBuf {
    humanoid_assets_with(name, HUMANOID)
}

/// [`humanoid_assets`] with another animset.
pub fn humanoid_assets_with(name: &str, animsets: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let models = Path::new(env!("CARGO_MANIFEST_DIR")).join("../sandbox/assets/models");
    std::fs::create_dir_all(dir.join("models")).unwrap();
    for file in ["humanoid.glb", "humanoid_gestures.glb"] {
        std::fs::copy(models.join(file), dir.join("models").join(file)).unwrap();
    }
    for (path, text) in [
        ("content/combat/actions.action.ron", ACTIONS),
        ("content/combat/movesets.moveset.ron", MOVESETS),
        ("content/anim/humanoid.animset.ron", animsets),
    ] {
        let path: &Path = path.as_ref();
        std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
        std::fs::write(dir.join(path), text).unwrap();
    }
    dir
}

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

/// The props of the sockets tests: a sword, a greatsword, a variant of the sword and a flask,
/// plain boxes.
pub const KIT_PROPS: &str = r#"{
    "sword": (size: (0.05, 0.9, 0.02), grip: (0.0, 0.3, 0.0)),
    "gilded_sword": (size: (0.06, 0.95, 0.03)),
    "great": (size: (0.1, 1.4, 0.04)),
}"#;

/// The humanoid with sockets: the hero carries a sword in the right hand (the brute a greatsword),
/// the drink puts it on the hip for a while, and a weapon switch is an equip of a tenth of a second.
pub const KIT: &str = r#"{ "humanoid": (
    source: "models/humanoid.glb",
    clips: {"idle": (), "walk": (), "light_1": (), "roll": (), "drink": (source: "models/humanoid_gestures.glb")},
    locomotion: {"idle": "idle", "walk": "walk"},
    layers: {"upper": (root: "spine")},
    actions: {"light_1": "light_1", "roll": "roll", "drink": (clip: "drink", layer: "upper")},
    by_moveset: {"brute": {"idle": "walk"}},
    sockets: {
        "hand_r": (bone: "hand.R"),
        "hip": (bone: "hips", offset: (0.25, 0.0, 0.0), turn: (0.0, 90.0, 0.0)),
        "back": (bone: "spine"),
    },
    carry: {"hero": [(prop: "sword", socket: "hand_r")], "brute": [(prop: "great", socket: "hand_r")]},
    mounts: {
        "drink": (moves: [(prop: "sword", to: Some("hip"), span: Secs(0.2, 0.8))], on_cut: {"sword": 0.1}),
        "@moveset": (
            moves: [(prop: "sword", to: Some("back"), span: Secs(0.05, 0.1))],
            previous: Some(Secs(0.0, 0.1)),
            ended_by: ["parried"],
        ),
    },
    root_motion: "root",
) }"#;

/// [`humanoid_assets_with`] with the props file too.
pub fn kit_assets(name: &str, animsets: &str) -> PathBuf {
    let dir = humanoid_assets_with(name, animsets);
    std::fs::create_dir_all(dir.join("content/anim")).unwrap();
    std::fs::write(dir.join("content/anim/props.prop.ron"), KIT_PROPS).unwrap();
    dir
}
