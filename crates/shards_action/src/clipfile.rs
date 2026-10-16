//! A glTF file's animations, read without a window or an asset server: their names, lengths
//! and the phases they were timed to (for content checks of an [`AnimSet`](crate::AnimSet)),
//! and a bone's translation track (for the displacement bake, [`crate::bake`]).
//!
//! `.glb` and `.gltf` files are read; a `.gltf`'s buffers may be files next to it or
//! `data:` URIs. The `gltf` crate stays inside this module: nothing of it is public.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::math::{Quat, Vec3};

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
                        let file = dir.unwrap_or(Path::new(".")).join(uri);
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
                let reader = channel.reader(data);
                let times: Vec<f32> = reader.read_inputs().ok_or_else(|| format!("animation `{name}` has a channel without key times"))?.collect();
                duration = times.iter().copied().fold(duration, f32::max);
                if channel.target().property() != gltf::animation::Property::Translation {
                    continue;
                }
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
        let mut json = serde_json::to_vec(&json).unwrap();
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
}
