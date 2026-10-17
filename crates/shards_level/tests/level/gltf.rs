//! Level files without Blender: a small glTF writer that produces what Blender's exporter
//! produces for a scene of objects with custom properties: a node tree with TRS or matrix
//! transforms, meshes with `POSITION` bounds, and `extras` on the nodes
//! (*Include → Custom Properties*). Buffers are embedded as base64 data URIs (the `.gltf`
//! flavor) or in the binary chunk (`.glb`).

#![allow(dead_code)]

// `ops` comes from whoever includes this file (sandbox, shards_level and shards_world reach Bevy by different paths).
use super::ops;
use serde_json::{json, Map, Value};

/// A node as the exporter writes it.
#[derive(Clone, Debug, Default)]
pub struct NodeSpec {
    pub name: Option<String>,
    pub translation: Option<[f32; 3]>,
    pub rotation: Option<[f32; 4]>,
    pub scale: Option<[f32; 3]>,
    pub matrix: Option<[f32; 16]>,
    pub mesh: Option<usize>,
    pub extras: Option<Value>,
    pub children: Vec<usize>,
}

impl NodeSpec {
    pub fn new(name: &str) -> Self {
        NodeSpec { name: Some(name.to_owned()), ..Default::default() }
    }

    pub fn unnamed() -> Self {
        NodeSpec::default()
    }

    pub fn at(mut self, translation: [f32; 3]) -> Self {
        self.translation = Some(translation);
        self
    }

    /// A quarter turn (or any angle, in degrees) about Y.
    pub fn yaw(mut self, degrees: f32) -> Self {
        let half = degrees.to_radians() * 0.5;
        self.rotation = Some([0.0, ops::sin(half), 0.0, ops::cos(half)]);
        self
    }

    pub fn scale(mut self, scale: [f32; 3]) -> Self {
        self.scale = Some(scale);
        self
    }

    pub fn matrix(mut self, matrix: [f32; 16]) -> Self {
        self.matrix = Some(matrix);
        self
    }

    pub fn mesh(mut self, mesh: usize) -> Self {
        self.mesh = Some(mesh);
        self
    }

    /// The object's custom properties.
    pub fn props(mut self, extras: Value) -> Self {
        self.extras = Some(extras);
        self
    }

    fn json(&self) -> Value {
        let mut node = Map::new();
        if let Some(name) = &self.name {
            node.insert("name".into(), json!(name));
        }
        if let Some(m) = self.matrix {
            node.insert("matrix".into(), json!(m));
        }
        if let Some(t) = self.translation {
            node.insert("translation".into(), json!(t));
        }
        if let Some(r) = self.rotation {
            node.insert("rotation".into(), json!(r));
        }
        if let Some(s) = self.scale {
            node.insert("scale".into(), json!(s));
        }
        if let Some(mesh) = self.mesh {
            node.insert("mesh".into(), json!(mesh));
        }
        if !self.children.is_empty() {
            node.insert("children".into(), json!(self.children));
        }
        if let Some(extras) = &self.extras {
            node.insert("extras".into(), extras.clone());
        }
        Value::Object(node)
    }
}

/// A glTF document under construction.
#[derive(Default)]
pub struct Gltf {
    nodes: Vec<NodeSpec>,
    meshes: Vec<Value>,
    accessors: Vec<Value>,
    views: Vec<Value>,
    buffer: Vec<u8>,
    roots: Vec<usize>,
    scene_extras: Option<Value>,
    /// Leave out the `min`/`max` of POSITION accessors (the format requires them: the file is
    /// invalid without).
    pub without_bounds: bool,
}

impl Gltf {
    pub fn new() -> Self {
        Gltf::default()
    }

    pub fn scene_props(&mut self, extras: Value) {
        self.scene_extras = Some(extras);
    }

    /// A mesh of one box spanning `min..max` (24 vertices with normals, 36 indices), named.
    pub fn box_mesh(&mut self, name: &str, min: [f32; 3], max: [f32; 3]) -> usize {
        // (normal, u, v) with u × v = normal: counter-clockwise from outside.
        const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        let center = [0, 1, 2].map(|i| (min[i] + max[i]) * 0.5);
        let half = [0, 1, 2].map(|i| (max[i] - min[i]) * 0.5);
        let (mut positions, mut normals, mut indices) = (Vec::new(), Vec::new(), Vec::<u16>::new());
        for (face, (n, u, v)) in FACES.iter().enumerate() {
            for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                positions.push([0, 1, 2].map(|i| center[i] + half[i] * (n[i] + su * u[i] + sv * v[i])));
                normals.push(*n);
            }
            let base = (face * 4) as u16;
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let low = positions.iter().fold([f32::MAX; 3], |a, p| [0, 1, 2].map(|i| a[i].min(p[i])));
        let high = positions.iter().fold([f32::MIN; 3], |a, p| [0, 1, 2].map(|i| a[i].max(p[i])));

        let position_accessor = self.accessor(&floats(&positions), "VEC3", 5126, positions.len(), Some((low, high)), 34962);
        let normal_accessor = self.accessor(&floats(&normals), "VEC3", 5126, normals.len(), None, 34962);
        let index_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let index_accessor = self.accessor(&index_bytes, "SCALAR", 5123, indices.len(), None, 34963);
        self.meshes.push(json!({
            "name": name,
            "primitives": [{"attributes": {"POSITION": position_accessor, "NORMAL": normal_accessor}, "indices": index_accessor}],
        }));
        self.meshes.len() - 1
    }

    /// The unit cube centered on the origin, like Blender's default cube scaled to 1 m.
    pub fn cube(&mut self) -> usize {
        self.box_mesh("Cube", [-0.5; 3], [0.5; 3])
    }

    fn accessor(&mut self, data: &[u8], kind: &str, component: u32, count: usize, bounds: Option<([f32; 3], [f32; 3])>, target: u32) -> usize {
        while !self.buffer.len().is_multiple_of(4) {
            self.buffer.push(0);
        }
        self.views.push(json!({"buffer": 0, "byteOffset": self.buffer.len(), "byteLength": data.len(), "target": target}));
        self.buffer.extend_from_slice(data);
        let mut accessor = json!({"bufferView": self.views.len() - 1, "componentType": component, "count": count, "type": kind});
        if let (Some((low, high)), false) = (bounds, self.without_bounds) {
            accessor["min"] = json!(low);
            accessor["max"] = json!(high);
        }
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }

    /// Adds a node to the file; returns its index. `nodes` order is file order.
    pub fn add(&mut self, node: NodeSpec) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Adds a node as a scene root.
    pub fn root(&mut self, node: NodeSpec) -> usize {
        let index = self.add(node);
        self.roots.push(index);
        index
    }

    /// Adds a node under `parent`.
    pub fn child(&mut self, parent: usize, node: NodeSpec) -> usize {
        let index = self.add(node);
        self.nodes[parent].children.push(index);
        index
    }

    /// The node called `name`, to edit as an artist would between two exports.
    pub fn node_mut(&mut self, name: &str) -> &mut NodeSpec {
        self.nodes.iter_mut().find(|n| n.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no node `{name}`"))
    }

    /// Makes an existing node a scene root.
    pub fn make_root(&mut self, node: usize) {
        self.roots.push(node);
    }

    pub fn document(&self) -> Value {
        let mut doc = json!({
            "asset": {"version": "2.0", "generator": "shards_level test fixture (stands in for Blender's exporter)"},
            "scene": 0,
            "scenes": [{"name": "Scene", "nodes": self.roots}],
            "nodes": self.nodes.iter().map(NodeSpec::json).collect::<Vec<_>>(),
        });
        if let Some(extras) = &self.scene_extras {
            doc["scenes"][0]["extras"] = extras.clone();
        }
        if !self.meshes.is_empty() {
            doc["meshes"] = json!(self.meshes);
            doc["accessors"] = json!(self.accessors);
            doc["bufferViews"] = json!(self.views);
        }
        doc
    }

    /// A `.gltf` file: JSON with the buffer embedded.
    pub fn gltf(&self) -> String {
        let mut doc = self.document();
        if !self.buffer.is_empty() {
            let uri = format!("data:application/octet-stream;base64,{}", base64(&self.buffer));
            doc["buffers"] = json!([{"byteLength": self.buffer.len(), "uri": uri}]);
        }
        let mut text = serde_json::to_string_pretty(&doc).unwrap();
        text.push('\n');
        text
    }

    /// A `.glb` file: the JSON chunk and the binary chunk.
    pub fn glb(&self) -> Vec<u8> {
        let mut doc = self.document();
        let mut bin = self.buffer.clone();
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        if !bin.is_empty() {
            doc["buffers"] = json!([{"byteLength": bin.len()}]);
        }
        let mut json = serde_json::to_vec(&doc).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        let total = 12 + 8 + json.len() + if bin.is_empty() { 0 } else { 8 + bin.len() };
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json);
        if !bin.is_empty() {
            out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
            out.extend_from_slice(b"BIN\0");
            out.extend_from_slice(&bin);
        }
        out
    }
}

fn floats<const N: usize>(items: &[[f32; N]]) -> Vec<u8> {
    items.iter().flatten().flat_map(|f| f.to_le_bytes()).collect()
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn base64_matches_the_standard_examples() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
}
