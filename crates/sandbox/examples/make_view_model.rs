//! Writes the sandbox's view model, `crates/sandbox/assets/models/view_model.glb` (F1b): a neutral pair of
//! arms with a slab of a weapon in the right hand, made of boxes, with six clips. The repository holds
//! no one else's art. Run it again after changing the rig or a clip:
//!
//! ```text
//! cargo run -p sandbox --example make_view_model
//! ```
//!
//! The camera looks along -Z, so the arms reach toward -Z and a recoil is toward +Z. The rig is a
//! tree of named nodes (`vm_root`, `arm.R`, `hand.R`, `weapon`, `arm.L`, `hand.L`), each with a box
//! mesh; the clips move those nodes, so the file needs no skin. Clips (`content/anim/view_model.animset.ron`
//! maps them): `idle` (a slow breath), `walk` and `run` (the body's sway), `fire` (a recoil),
//! `reload` (the weapon tilts away and back) and `draw` (the weapon comes up). The file is the same
//! every time it is written.

use std::path::PathBuf;

use shards::bevy::math::ops;
use shards::serde_json::{json, Value};

/// One accessor's data, appended to the binary buffer.
#[derive(Default)]
struct Blob {
    bytes: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Blob {
    /// Appends `data` (float components of `width` per element) and returns its accessor's index.
    /// `bounds` writes the min and max (the spec asks for them on positions and animation times).
    fn floats(&mut self, data: &[f32], width: usize, target: Option<u32>, bounds: bool) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        for v in data {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        let mut view = json!({"buffer": 0, "byteOffset": offset, "byteLength": data.len() * 4});
        if let Some(target) = target {
            view["target"] = json!(target);
        }
        self.views.push(view);
        let kind = ["", "SCALAR", "VEC2", "VEC3", "VEC4"][width];
        let mut accessor = json!({"bufferView": self.views.len() - 1, "componentType": 5126, "count": data.len() / width, "type": kind});
        if bounds {
            let mut min = vec![f32::MAX; width];
            let mut max = vec![f32::MIN; width];
            for element in data.chunks(width) {
                for (i, v) in element.iter().enumerate() {
                    min[i] = min[i].min(*v);
                    max[i] = max[i].max(*v);
                }
            }
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// Appends 16-bit indices and returns the accessor's index.
    fn indices(&mut self, data: &[u16]) -> usize {
        while !self.bytes.len().is_multiple_of(4) {
            self.bytes.push(0);
        }
        let offset = self.bytes.len();
        for v in data {
            self.bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.views.push(json!({"buffer": 0, "byteOffset": offset, "byteLength": data.len() * 2, "target": 34963}));
        self.accessors.push(json!({"bufferView": self.views.len() - 1, "componentType": 5123, "count": data.len(), "type": "SCALAR"}));
        self.accessors.len() - 1
    }
}

/// The mesh of a box of half sizes `h`: 24 vertices with their own normals, 12 triangles.
fn box_mesh(blob: &mut Blob, h: [f32; 3], material: usize) -> Value {
    // Each face: its normal, then its four corners (counter-clockwise seen from outside).
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        ([0.0, 0.0, 1.0], [[-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0]]),
        ([0.0, 0.0, -1.0], [[1.0, -1.0, -1.0], [-1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [1.0, 1.0, -1.0]]),
        ([1.0, 0.0, 0.0], [[1.0, -1.0, 1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [1.0, 1.0, 1.0]]),
        ([-1.0, 0.0, 0.0], [[-1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0], [-1.0, 1.0, -1.0]]),
        ([0.0, 1.0, 0.0], [[-1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0]]),
        ([0.0, -1.0, 0.0], [[-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, -1.0, 1.0], [-1.0, -1.0, 1.0]]),
    ];
    let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::new());
    for (face, (normal, corners)) in faces.iter().enumerate() {
        for corner in corners {
            positions.extend([corner[0] * h[0], corner[1] * h[1], corner[2] * h[2]]);
            normals.extend(*normal);
        }
        let first = (face * 4) as u16;
        indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    let position = blob.floats(&positions, 3, Some(34962), true);
    let normal = blob.floats(&normals, 3, Some(34962), false);
    let index = blob.indices(&indices);
    json!({"primitives": [{"attributes": {"POSITION": position, "NORMAL": normal}, "indices": index, "material": material}]})
}

/// A rotation about X of `degrees`, as a quaternion (x, y, z, w).
fn about_x(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() * 0.5;
    [ops::sin(half), 0.0, 0.0, ops::cos(half)]
}

/// A rotation about Z of `degrees`.
fn about_z(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() * 0.5;
    [0.0, 0.0, ops::sin(half), ops::cos(half)]
}

/// One channel of a clip: a node's translation or rotation at the given times.
struct Track {
    node: usize,
    rotation: bool,
    times: Vec<f32>,
    values: Vec<Vec<f32>>,
}

fn translation(node: usize, keys: &[(f32, [f32; 3])]) -> Track {
    Track { node, rotation: false, times: keys.iter().map(|k| k.0).collect(), values: keys.iter().map(|k| k.1.to_vec()).collect() }
}

fn rotation(node: usize, keys: &[(f32, [f32; 4])]) -> Track {
    Track { node, rotation: true, times: keys.iter().map(|k| k.0).collect(), values: keys.iter().map(|k| k.1.to_vec()).collect() }
}

// Node indices.
const ROOT: usize = 0;
const ARM_R: usize = 1;
const HAND_R: usize = 2;
const WEAPON: usize = 3;
const ARM_L: usize = 4;
const HAND_L: usize = 5;

// Rest poses (node-local, metres).
const ARM_R_AT: [f32; 3] = [0.12, -0.15, -0.26];
const WEAPON_AT: [f32; 3] = [0.0, 0.036, -0.03];

fn clips() -> Vec<(&'static str, Vec<Track>)> {
    let [rx, ry, rz] = ARM_R_AT;
    let [wx, wy, wz] = WEAPON_AT;
    let level = about_x(0.0);
    vec![
        // A breath: the arm rises a few millimetres and the weapon tips a degree. 2 s, loops.
        (
            "idle",
            vec![
                translation(ARM_R, &[(0.0, [rx, ry, rz]), (1.0, [rx, ry + 0.004, rz]), (2.0, [rx, ry, rz])]),
                rotation(WEAPON, &[(0.0, level), (1.0, about_x(1.0)), (2.0, level)]),
            ],
        ),
        // The body's sway: left and right once, up and down twice, per stride of 0.8 s.
        (
            "walk",
            vec![translation(
                ROOT,
                &[(0.0, [0.0, 0.0, 0.0]), (0.2, [0.010, -0.008, 0.0]), (0.4, [0.0, 0.0, 0.0]), (0.6, [-0.010, -0.008, 0.0]), (0.8, [0.0, 0.0, 0.0])],
            )],
        ),
        (
            "run",
            vec![
                translation(
                    ROOT,
                    &[(0.0, [0.0, 0.0, 0.0]), (0.1375, [0.018, -0.014, 0.0]), (0.275, [0.0, 0.0, 0.0]), (0.4125, [-0.018, -0.014, 0.0]), (0.55, [0.0, 0.0, 0.0])],
                ),
                rotation(WEAPON, &[(0.0, level), (0.1375, about_x(3.0)), (0.275, level), (0.4125, about_x(3.0)), (0.55, level)]),
            ],
        ),
        // A shot: the weapon kicks back toward the camera and up, the arm follows, both settle.
        (
            "fire",
            vec![
                translation(WEAPON, &[(0.0, [wx, wy, wz]), (0.05, [wx, wy, wz + 0.045]), (0.35, [wx, wy, wz])]),
                rotation(WEAPON, &[(0.0, level), (0.05, about_x(9.0)), (0.35, level)]),
                translation(ARM_R, &[(0.0, [rx, ry, rz]), (0.06, [rx, ry, rz + 0.03]), (0.35, [rx, ry, rz])]),
            ],
        ),
        // A reload: the whole rig dips, the weapon rolls to the side, holds, and comes back.
        (
            "reload",
            vec![
                translation(ROOT, &[(0.0, [0.0, 0.0, 0.0]), (0.25, [0.0, -0.06, 0.0]), (0.8, [0.0, -0.06, 0.0]), (1.1, [0.0, 0.0, 0.0])]),
                rotation(WEAPON, &[(0.0, about_z(0.0)), (0.3, about_z(25.0)), (0.8, about_z(25.0)), (1.1, about_z(0.0))]),
            ],
        ),
        // Bringing it up from below.
        (
            "draw",
            vec![
                translation(ROOT, &[(0.0, [0.0, -0.30, 0.0]), (0.4, [0.0, 0.0, 0.0])]),
                rotation(ROOT, &[(0.0, about_x(-40.0)), (0.4, level)]),
            ],
        ),
    ]
}

fn main() {
    let mut blob = Blob::default();
    // Materials: a sleeve, a hand, a metal.
    let materials = json!([
        {"name": "Sleeve", "pbrMetallicRoughness": {"baseColorFactor": [0.24, 0.29, 0.36, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.9}},
        {"name": "Hand", "pbrMetallicRoughness": {"baseColorFactor": [0.78, 0.60, 0.48, 1.0], "metallicFactor": 0.0, "roughnessFactor": 0.8}},
        {"name": "Metal", "pbrMetallicRoughness": {"baseColorFactor": [0.16, 0.17, 0.20, 1.0], "metallicFactor": 0.6, "roughnessFactor": 0.45}},
    ]);
    // Meshes: a forearm, a hand, the weapon's slab.
    let meshes = vec![
        box_mesh(&mut blob, [0.028, 0.028, 0.14], 0),
        box_mesh(&mut blob, [0.036, 0.024, 0.04], 1),
        box_mesh(&mut blob, [0.016, 0.028, 0.10], 2),
    ];
    let nodes = json!([
        {"name": "vm_root", "children": [ARM_R, ARM_L]},
        {"name": "arm.R", "translation": ARM_R_AT, "mesh": 0, "children": [HAND_R]},
        {"name": "hand.R", "translation": [0.0, 0.0, -0.16], "mesh": 1, "children": [WEAPON]},
        {"name": "weapon", "translation": WEAPON_AT, "mesh": 2},
        {"name": "arm.L", "translation": [-0.13, -0.17, -0.25], "mesh": 0, "children": [HAND_L]},
        {"name": "hand.L", "translation": [0.0, 0.0, -0.15], "mesh": 1},
    ]);
    let mut animations = Vec::new();
    for (name, tracks) in clips() {
        let (mut channels, mut samplers) = (Vec::new(), Vec::new());
        for track in tracks {
            let input = blob.floats(&track.times, 1, None, true);
            let flat: Vec<f32> = track.values.iter().flatten().copied().collect();
            let output = blob.floats(&flat, if track.rotation { 4 } else { 3 }, None, false);
            samplers.push(json!({"input": input, "output": output, "interpolation": "LINEAR"}));
            channels.push(json!({
                "sampler": samplers.len() - 1,
                "target": {"node": track.node, "path": if track.rotation { "rotation" } else { "translation" }},
            }));
        }
        animations.push(json!({"name": name, "channels": channels, "samplers": samplers}));
    }
    let document = json!({
        "asset": {"version": "2.0", "generator": "shards sandbox example make_view_model"},
        "scene": 0,
        "scenes": [{"name": "view_model", "nodes": [ROOT]}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "animations": animations,
        "buffers": [{"byteLength": blob.bytes.len()}],
        "bufferViews": blob.views,
        "accessors": blob.accessors,
    });
    write_glb(&document, &blob.bytes, &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/view_model.glb"));
}

/// A binary glTF: the header, the JSON chunk padded with spaces, the buffer chunk padded with zeros.
fn write_glb(document: &Value, bin: &[u8], path: &std::path::Path) {
    let mut json = shards::serde_json::to_vec(document).expect("the document is JSON");
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).expect("the models folder");
    }
    std::fs::write(path, &out).expect("write the view model");
    println!("wrote {} ({} bytes)", path.display(), out.len());
}
