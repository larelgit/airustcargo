//! A glTF file's animations, read without a window or an asset server: their names, lengths
//! and the phases they were timed to (for content checks of an [`AnimSet`](crate::AnimSet)),
//! and a bone's translation track (for the displacement bake, [`crate::bake`]).
//!
//! `.glb` and `.gltf` files are read; a `.gltf`'s buffers may be files next to it (their names
//! percent-decoded, as the glTF spec writes them and Bevy's loader reads them) or `data:` URIs.
//! The `gltf` crate stays inside this module: nothing of it is public.
//!
//! A file is content, and a content check must not hang or panic on a broken one. The node
//! graph is checked for cycles, and every accessor the reader will iterate (key times and
//! translations) is checked before the `gltf` crate's iterator sees it: float type, scalar or
//! vec3, at least one element, a `byteStride` that fits one, and all of it inside its buffer
//! view and buffer. Sparse accessors are not read (no exporter writes them for animations):
//! the error says so.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::math::{Quat, Vec3};
use gltf::accessor::{DataType, Dimensions};

/// The parts of a glTF file the animation checks and the bake need.
#[derive(Debug)]
pub struct ClipFile {
    /// Animation name -> what it is.
    animations: BTreeMap<String, Animation>,
    /// Node name -> where it sits.
    nodes: BTreeMap<String, NodeInfo>,
}

#[derive(Debug)]
struct Animation {
    /// Seconds: the last key of any channel.
    duration: f32,
    /// The `phases` extra (`"windup 0.2 active 0.1 recovery 0.3"`), as written.
    phases: Option<String>,
    /// Node name -> its translation track.
    translations: BTreeMap<String, Track>,
}

#[derive(Clone, Debug)]
struct NodeInfo {
    /// The node's own translation (its rest pose).
    rest: Vec3,
    /// Rotation and scale of its parents, the root first: what its translation is moved by.
    parents: Vec<(String, Quat, Vec3)>,
}

/// A translation track: keys and how to go between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    times: Vec<f32>,
    /// One value per key; three per key (in-tangent, value, out-tangent) for a cubic spline.
    values: Vec<Vec3>,
    interpolation: Interpolation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interpolation {
    Linear,
    Step,
    Cubic,
}

impl Track {
    /// The value at `t` seconds, held before the first key and after the last.
    pub fn sample(&self, t: f32) -> Vec3 {
        let value = |i: usize| if self.interpolation == Interpolation::Cubic { self.values[i * 3 + 1] } else { self.values[i] };
        let n = self.times.len();
        if n == 0 {
            return Vec3::ZERO;
        }
        if t <= self.times[0] {
            return value(0);
        }
        if t >= self.times[n - 1] {
            return value(n - 1);
        }
        let i = self.times.partition_point(|&k| k <= t) - 1;
        let (t0, t1) = (self.times[i], self.times[i + 1]);
        let dt = t1 - t0;
        if dt <= 0.0 {
            return value(i + 1);
        }
        let s = (t - t0) / dt;
        match self.interpolation {
            Interpolation::Step => value(i),
            Interpolation::Linear => value(i).lerp(value(i + 1), s),
            Interpolation::Cubic => {
                let (p0, m0) = (self.values[i * 3 + 1], self.values[i * 3 + 2] * dt);
                let (p1, m1) = (self.values[(i + 1) * 3 + 1], self.values[(i + 1) * 3] * dt);
                let (s2, s3) = (s * s, s * s * s);
                p0 * (2.0 * s3 - 3.0 * s2 + 1.0) + m0 * (s3 - 2.0 * s2 + s) + p1 * (-2.0 * s3 + 3.0 * s2) + m1 * (s3 - s2)
            }
        }
    }
}

impl ClipFile {
    /// Reads `path`: an error says what is wrong with it.
    pub fn read(path: &Path) -> Result<ClipFile, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot be read ({e})"))?;
        Self::parse(&bytes, path.parent())
    }

    /// A file's bytes; `dir` is where a `.gltf`'s buffer files are.
    pub fn parse(bytes: &[u8], dir: Option<&Path>) -> Result<ClipFile, String> {
        let gltf = gltf::Gltf::from_slice(bytes).map_err(|e| format!("is not a valid glTF file ({e})"))?;
        let mut buffers: Vec<Vec<u8>> = Vec::new();
        for buffer in gltf.document.buffers() {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => gltf.blob.clone().ok_or("names a binary chunk it does not have")?,
                gltf::buffer::Source::Uri(uri) => match uri.strip_prefix("data:") {
                    Some(data) => {
                        let (_, encoded) = data.split_once(";base64,").ok_or("has a data: URI that is not base64")?;
                        decode_base64(encoded).ok_or("has a data: URI that is not valid base64")?
                    }
                    None => {
                        let name = percent_decode(uri).ok_or_else(|| format!("names buffer `{uri}`, which is not UTF-8 once decoded"))?;
                        let file = dir.unwrap_or(Path::new(".")).join(name);
                        std::fs::read(&file).map_err(|e| format!("names buffer `{uri}`, which cannot be read ({e})"))?
                    }
                },
            };
            if data.len() < buffer.length() {
                return Err(format!("has a buffer of {} bytes where it says {}", data.len(), buffer.length()));
            }
            buffers.push(data);
        }
        let data = |b: gltf::Buffer| buffers.get(b.index()).map(Vec::as_slice);

        let mut parent_of: BTreeMap<usize, usize> = BTreeMap::new();
        for node in gltf.document.nodes() {
            for child in node.children() {
                parent_of.insert(child.index(), node.index());
            }
        }
        let all: Vec<gltf::Node> = gltf.document.nodes().collect();
        let name_of = |i: usize| all[i].name().map_or_else(|| format!("#{i}"), str::to_owned);
        let mut nodes = BTreeMap::new();
        for node in &all {
            let Some(name) = node.name() else { continue };
            let (rest, _, _) = node.transform().decomposed();
            let mut parents = Vec::new();
            let mut at = node.index();
            while let Some(&p) = parent_of.get(&at) {
                // A chain has fewer parents than the file has nodes; more means it loops.
                if parents.len() >= all.len() {
                    return Err(format!("has nodes that form a cycle (`{}` is its own ancestor)", name_of(p)));
                }
                let (_, r, s) = all[p].transform().decomposed();
                parents.push((name_of(p), Quat::from_array(r), Vec3::from_array(s)));
                at = p;
            }
            parents.reverse();
            nodes.entry(name.to_owned()).or_insert(NodeInfo { rest: Vec3::from_array(rest), parents });
        }

        let mut animations = BTreeMap::new();
        for (i, animation) in gltf.document.animations().enumerate() {
            let name = animation.name().map_or_else(|| format!("#{i}"), str::to_owned);
            let mut duration: f32 = 0.0;
            let mut translations = BTreeMap::new();
            for channel in animation.channels() {
                let sampler = channel.sampler();
                check_accessor(&sampler.input(), Dimensions::Scalar, &buffers)
                    .map_err(|e| format!("animation `{name}` has key times that {e}"))?;
                let reader = channel.reader(data);
                let times: Vec<f32> = reader.read_inputs().ok_or_else(|| format!("animation `{name}` has a channel without key times"))?.collect();
                duration = times.iter().copied().fold(duration, f32::max);
                if channel.target().property() != gltf::animation::Property::Translation {
                    continue;
                }
                check_accessor(&sampler.output(), Dimensions::Vec3, &buffers)
                    .map_err(|e| format!("animation `{name}` has translations that {e}"))?;
                let Some(gltf::animation::util::ReadOutputs::Translations(values)) = reader.read_outputs() else {
                    return Err(format!("animation `{name}` has a translation channel without values"));
                };
                let interpolation = match channel.sampler().interpolation() {
                    gltf::animation::Interpolation::Linear => Interpolation::Linear,
                    gltf::animation::Interpolation::Step => Interpolation::Step,
                    gltf::animation::Interpolation::CubicSpline => Interpolation::Cubic,
                };
                let values: Vec<Vec3> = values.map(Vec3::from_array).collect();
                let per_key = if interpolation == Interpolation::Cubic { 3 } else { 1 };
                if values.len() != times.len() * per_key {
                    return Err(format!("animation `{name}` has {} translation values for {} keys", values.len(), times.len()));
                }
                let node = channel.target().node().index();
                translations.insert(name_of(node), Track { times, values, interpolation });
            }
            let phases = animation.extras().as_ref().and_then(|raw| {
                let value: serde_json::Value = serde_json::from_str(raw.get()).ok()?;
                value.get("phases")?.as_str().map(str::to_owned)
            });
            animations.insert(name, Animation { duration, phases, translations });
        }
        Ok(ClipFile { animations, nodes })
    }

    /// The names of its animations, sorted.
    pub fn animation_names(&self) -> Vec<&str> {
        self.animations.keys().map(String::as_str).collect()
    }

    pub fn has_animation(&self, name: &str) -> bool {
        self.animations.contains_key(name)
    }

    /// Seconds animation `name` lasts.
    pub fn duration(&self, name: &str) -> Option<f32> {
        self.animations.get(name).map(|a| a.duration)
    }

    /// The phases animation `name` was timed to (its `phases` extra, as Blender writes an
    /// action's custom property `phases`: `"windup 0.2 active 0.1 recovery 0.3"`); `None`
    /// without one, an error when it does not read as names and seconds.
    pub fn phases(&self, name: &str) -> Option<Result<Vec<(String, f32)>, String>> {
        let text = self.animations.get(name)?.phases.as_deref()?;
        let words: Vec<&str> = text.split_whitespace().collect();
        let parsed = (words.len().is_multiple_of(2) && !words.is_empty())
            .then(|| {
                words
                    .chunks(2)
                    .map(|pair| pair[1].parse::<f32>().ok().filter(|s| *s >= 0.0).map(|s| (pair[0].to_owned(), s)))
                    .collect::<Option<Vec<_>>>()
            })
            .flatten();
        Some(parsed.ok_or_else(|| format!("its `phases` extra `{text}` is not names and seconds (`windup 0.2 active 0.1`)")))
    }

    pub fn has_node(&self, name: &str) -> bool {
        self.nodes.contains_key(name)
    }

    /// The rest translation of node `name`.
    pub fn rest(&self, name: &str) -> Option<Vec3> {
        self.nodes.get(name).map(|n| n.rest)
    }

    /// The first parent of node `name` that turns it, or scales it unevenly: its translation
    /// is then not a move in the model's own axes.
    pub fn turned_parent(&self, name: &str) -> Option<&str> {
        let node = self.nodes.get(name)?;
        node.parents
            .iter()
            .find(|(_, r, s)| r.angle_between(Quat::IDENTITY) > 1e-4 || (s.x - s.y).abs() > 1e-4 || (s.y - s.z).abs() > 1e-4)
            .map(|(p, ..)| p.as_str())
    }

    /// How much the parents of node `name` scale its translation.
    pub fn parent_scale(&self, name: &str) -> f32 {
        self.nodes.get(name).map_or(1.0, |n| n.parents.iter().map(|(_, _, s)| s.x).product())
    }

    /// The translation track of node `name` in animation `animation`: `None` when the animation
    /// does not move it.
    pub fn translation(&self, animation: &str, name: &str) -> Option<&Track> {
        self.animations.get(animation)?.translations.get(name)
    }
}

/// Whether the `gltf` crate can iterate `accessor` as floats of `dimensions` without a panic
/// (it asserts on a type, a stride or a count it does not expect, and subtracts 1 from the
/// count): `Err` is the end of a sentence about the accessor, saying what is wrong with it.
fn check_accessor(accessor: &gltf::Accessor, dimensions: Dimensions, buffers: &[Vec<u8>]) -> Result<(), String> {
    let (found_type, found_dims) = (accessor.data_type(), accessor.dimensions());
    if found_type != DataType::F32 || found_dims != dimensions {
        return Err(format!("are {found_dims:?} of {found_type:?}, not {dimensions:?} of F32"));
    }
    if accessor.sparse().is_some() {
        return Err("are a sparse accessor, which this reader does not read".into());
    }
    let count = accessor.count();
    if count == 0 {
        return Err("are an accessor with a count of 0".into());
    }
    let Some(view) = accessor.view() else { return Err("are an accessor without a buffer view".into()) };
    let element = accessor.size();
    let stride = match view.stride() {
        Some(stride) if stride < element => {
            return Err(format!("have a byteStride of {stride}, less than the {element} bytes of one element"));
        }
        Some(stride) => stride,
        None => element,
    };
    let buffer_len = buffers.get(view.buffer().index()).map_or(0, Vec::len);
    if view.offset().checked_add(view.length()).is_none_or(|end| end > buffer_len) {
        return Err(format!("sit in a buffer view of {} bytes at {}, past the {buffer_len} bytes of its buffer", view.length(), view.offset()));
    }
    // The last element ends where the iterator expects it, inside the view.
    let end = stride.checked_mul(count - 1).and_then(|n| n.checked_add(accessor.offset())).and_then(|n| n.checked_add(element));
    if end.is_none_or(|end| end > view.length()) {
        return Err(format!("read {count} elements from {} bytes of buffer view", view.length()));
    }
    Ok(())
}

/// `My%20Rig.bin` -> `My Rig.bin`: the `%XX` escapes of a URI reference decoded, as Bevy's loader
/// does before it reads a buffer file (the glTF spec has file names percent-encoded). An escape
/// that is not two hex digits stays as written; `None` when the bytes are not UTF-8.
fn percent_decode(uri: &str) -> Option<String> {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escape = if bytes[i] == b'%' && i + 2 < bytes.len() { hex(bytes[i + 1]).zip(hex(bytes[i + 2])) } else { None };
        match escape {
            Some((high, low)) => {
                out.push(high * 16 + low);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

fn hex(c: u8) -> Option<u8> {
    char::from(c).to_digit(16).map(|d| d as u8)
}

/// Standard base64 (with or without padding); `None` on anything else.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn floats(data: &mut Vec<u8>, values: &[f32]) {
        data.extend(values.iter().flat_map(|f| f.to_le_bytes()));
    }

    /// A test animation: `(name, phases, times, root translations)`.
    pub(crate) type TestClip<'a> = (&'a str, Option<&'a str>, &'a [f32], &'a [[f32; 3]]);

    /// A `.glb` with nodes `Rig` (its transform: `rig`, e.g. `{"scale": [2, 2, 2]}`) > `root` > `hips`
    /// and one animation per [`TestClip`]; the root's track is linear.
    pub(crate) fn glb(rig: serde_json::Value, animations: &[TestClip]) -> Vec<u8> {
        let mut data = Vec::new();
        let (mut views, mut accessors, mut anims) = (Vec::new(), Vec::new(), Vec::new());
        for (name, phases, times, values) in animations {
            let start = data.len();
            floats(&mut data, times);
            views.push(serde_json::json!({"buffer": 0, "byteOffset": start, "byteLength": times.len() * 4}));
            let max = times.iter().copied().fold(0.0f32, f32::max);
            accessors.push(serde_json::json!({"bufferView": views.len() - 1, "componentType": 5126, "count": times.len(),
                "type": "SCALAR", "min": [0.0], "max": [max]}));
            let start = data.len();
            for v in values.iter() {
                floats(&mut data, v);
            }
            views.push(serde_json::json!({"buffer": 0, "byteOffset": start, "byteLength": values.len() * 12}));
            accessors.push(serde_json::json!({"bufferView": views.len() - 1, "componentType": 5126, "count": values.len(), "type": "VEC3"}));
            let mut anim = serde_json::json!({
                "name": name,
                "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}],
                "samplers": [{"input": accessors.len() - 2, "output": accessors.len() - 1, "interpolation": "LINEAR"}],
            });
            if let Some(phases) = phases {
                anim["extras"] = serde_json::json!({"phases": phases});
            }
            anims.push(anim);
        }
        if data.is_empty() {
            data.extend([0u8; 4]);
        }
        let mut rig_node = serde_json::json!({"name": "Rig", "children": [1]});
        for (k, v) in rig.as_object().into_iter().flatten() {
            rig_node[k] = v.clone();
        }
        let json = serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [
                rig_node,
                {"name": "root", "children": [2]},
                {"name": "hips", "translation": [0.0, 1.0, 0.0]},
            ],
            "animations": anims,
            "buffers": [{"byteLength": data.len()}],
            "bufferViews": views,
            "accessors": accessors,
        });
        pack(&json, data)
    }

    /// A `.glb` of `json` (the document) and `data` (its binary chunk).
    fn pack(json: &serde_json::Value, mut data: Vec<u8>) -> Vec<u8> {
        let mut json = serde_json::to_vec(json).unwrap();
        while !json.len().is_multiple_of(4) {
            json.push(b' ');
        }
        while !data.len().is_multiple_of(4) {
            data.push(0);
        }
        let total = 12 + 8 + json.len() + 8 + data.len();
        let mut out = Vec::with_capacity(total);
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend((total as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(json);
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(data);
        out
    }

    /// The smallest document with a translation animation `a` on node `root` (key times 0 and 1
    /// in a 8-byte view, two translations in a 24-byte view, 32 bytes of binary): the tests of
    /// broken files change one thing in it.
    fn tiny() -> serde_json::Value {
        serde_json::json!({
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "root", "children": [1]}, {"name": "hips"}],
            "animations": [{
                "name": "a",
                "channels": [{"sampler": 0, "target": {"node": 0, "path": "translation"}}],
                "samplers": [{"input": 0, "output": 1, "interpolation": "LINEAR"}],
            }],
            "buffers": [{"byteLength": 32}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 8},
                {"buffer": 0, "byteOffset": 8, "byteLength": 24},
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0.0], "max": [1.0]},
                {"bufferView": 1, "componentType": 5126, "count": 2, "type": "VEC3"},
            ],
        })
    }

    fn tiny_data() -> Vec<u8> {
        let mut data = Vec::new();
        floats(&mut data, &[0.0, 1.0]);
        floats(&mut data, &[0.0, 0.0, 0.0, 0.0, 0.0, 2.0]);
        data
    }

    fn parse_tiny(json: &serde_json::Value) -> Result<ClipFile, String> {
        ClipFile::parse(&pack(json, tiny_data()), None)
    }

    #[test]
    fn reads_names_lengths_phases_and_tracks() {
        let bytes = glb(
            serde_json::json!({}),
            &[
                ("lunge", Some("windup 0.2 active 0.1"), &[0.0, 0.2, 0.3], &[[0.0; 3], [0.0; 3], [0.0, 0.0, 0.6]]),
                ("idle", None, &[0.0, 2.0], &[[0.0; 3], [0.0; 3]]),
            ],
        );
        let file = ClipFile::parse(&bytes, None).unwrap();
        assert_eq!(file.animation_names(), ["idle", "lunge"]);
        assert!((file.duration("lunge").unwrap() - 0.3).abs() < 1e-6);
        assert_eq!(file.phases("lunge"), Some(Ok(vec![("windup".into(), 0.2), ("active".into(), 0.1)])));
        assert_eq!(file.phases("idle"), None);
        let track = file.translation("lunge", "root").unwrap();
        assert!((track.sample(0.25).z - 0.3).abs() < 1e-6, "halfway between the keys");
        assert_eq!(track.sample(9.0).z, 0.6, "held after the last key");
        assert_eq!(file.rest("hips"), Some(Vec3::Y));
        assert_eq!(file.turned_parent("root"), None);
        assert!(file.translation("lunge", "hips").is_none());
    }

    #[test]
    fn a_bad_phases_extra_and_a_turned_parent_are_named() {
        let bytes = glb(serde_json::json!({}), &[("x", Some("windup fast"), &[0.0, 1.0], &[[0.0; 3], [0.0; 3]])]);
        let file = ClipFile::parse(&bytes, None).unwrap();
        assert!(file.phases("x").unwrap().unwrap_err().contains("`windup fast` is not names and seconds"));
        let turned = glb(serde_json::json!({"rotation": [0.0, 1.0, 0.0, 0.0]}), &[]);
        let file = ClipFile::parse(&turned, None).unwrap();
        assert_eq!(file.turned_parent("root"), Some("Rig"));
        assert!(ClipFile::parse(b"not a gltf", None).unwrap_err().starts_with("is not a valid glTF file"));
    }

    #[test]
    fn base64_decodes() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("AAEC").unwrap(), [0, 1, 2]);
        assert!(decode_base64("a*b").is_none());
    }

    #[test]
    fn the_smallest_document_reads() {
        let file = parse_tiny(&tiny()).unwrap();
        assert_eq!(file.animation_names(), ["a"]);
        assert_eq!(file.duration("a"), Some(1.0));
        assert_eq!(file.translation("a", "root").unwrap().sample(0.5).z, 1.0);
    }

    #[test]
    fn nodes_that_form_a_cycle_are_an_error_not_a_hang() {
        for nodes in [
            // A node that is its own child.
            serde_json::json!([{"name": "root", "children": [0]}]),
            // Two that are each other's.
            serde_json::json!([{"name": "root", "children": [1]}, {"name": "hips", "children": [0]}]),
            // One hanging off a loop it is not in.
            serde_json::json!([{"name": "root", "children": [1]}, {"name": "hips", "children": [2]}, {"name": "tail", "children": [1]}]),
        ] {
            let mut json = tiny();
            json["nodes"] = nodes.clone();
            let error = parse_tiny(&json).unwrap_err();
            assert!(error.contains("form a cycle"), "{nodes}: {error}");
        }
    }

    #[test]
    fn an_accessor_the_gltf_crate_would_panic_on_is_an_error() {
        // Each change breaks the document one way; the `gltf` crate's iterator panics on the
        // first three in a debug build (a count of 0, a stride under the element, a wrong type).
        let cases: [(&str, serde_json::Value, &str); 8] = [
            ("/accessors/0/count", serde_json::json!(0), "key times that are an accessor with a count of 0"),
            ("/accessors/1/count", serde_json::json!(0), "translations that are an accessor with a count of 0"),
            ("/bufferViews/1/byteStride", serde_json::json!(4), "translations that have a byteStride of 4, less than the 12 bytes"),
            ("/accessors/0/type", serde_json::json!("VEC3"), "key times that are Vec3 of F32, not Scalar of F32"),
            ("/accessors/1/componentType", serde_json::json!(5123), "translations that are Vec3 of U16, not Vec3 of F32"),
            ("/accessors/1/count", serde_json::json!(3), "translations that read 3 elements from 24 bytes"),
            ("/accessors/1/count", serde_json::json!(u64::MAX), "translations that read"),
            ("/bufferViews/1/byteOffset", serde_json::json!(4096), "translations that sit in a buffer view of 24 bytes at 4096"),
        ];
        for (pointer, value, expected) in cases {
            let mut json = tiny();
            let (parent, key) = pointer.rsplit_once('/').unwrap();
            json.pointer_mut(parent).unwrap()[key] = value.clone();
            let error = parse_tiny(&json).unwrap_err();
            assert!(error.contains(expected), "{pointer} = {value}: {error}");
        }
        let mut sparse = tiny();
        sparse["accessors"][0]["sparse"] = serde_json::json!({
            "count": 1,
            "indices": {"bufferView": 0, "componentType": 5123},
            "values": {"bufferView": 0},
        });
        assert!(parse_tiny(&sparse).unwrap_err().contains("sparse accessor"));
    }

    #[test]
    fn percent_escapes_decode_as_bevy_does() {
        assert_eq!(percent_decode("My%20Rig.bin").as_deref(), Some("My Rig.bin"));
        assert_eq!(percent_decode("caf%C3%A9.bin").as_deref(), Some("café.bin"));
        assert_eq!(percent_decode("plain.bin").as_deref(), Some("plain.bin"));
        // What is not an escape stays as written.
        assert_eq!(percent_decode("100%").as_deref(), Some("100%"));
        assert_eq!(percent_decode("a%2").as_deref(), Some("a%2"));
        assert_eq!(percent_decode("a%zzb").as_deref(), Some("a%zzb"));
        assert_eq!(percent_decode("a%2/b").as_deref(), Some("a%2/b"));
        // Bytes that are not UTF-8 once decoded.
        assert_eq!(percent_decode("%ff.bin"), None);
    }

    #[test]
    fn a_gltf_reads_a_buffer_file_with_an_escaped_name() {
        let dir = std::env::temp_dir().join(format!("shards_action_clipfile_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut json = tiny();
        json["buffers"][0]["uri"] = serde_json::json!("My%20Rig.bin");
        std::fs::write(dir.join("My Rig.gltf"), serde_json::to_vec(&json).unwrap()).unwrap();
        std::fs::write(dir.join("My Rig.bin"), tiny_data()).unwrap();
        let read = ClipFile::read(&dir.join("My Rig.gltf"));
        // A buffer that is not there is reported under the name the file gives it.
        json["buffers"][0]["uri"] = serde_json::json!("Missing%20Rig.bin");
        std::fs::write(dir.join("Missing.gltf"), serde_json::to_vec(&json).unwrap()).unwrap();
        let missing = ClipFile::read(&dir.join("Missing.gltf"));
        let _ = std::fs::remove_dir_all(&dir);
        let file = read.unwrap();
        assert_eq!(file.duration("a"), Some(1.0));
        assert!(missing.unwrap_err().contains("names buffer `Missing%20Rig.bin`, which cannot be read"));
    }
}
