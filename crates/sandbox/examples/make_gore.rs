//! Writes the sandbox's gore assets (F9): two neutral mark textures, `assets/textures/marks/bullet_hole.png`
//! and `blood_splat.png` (white on clear, so the mark's colour tints them), and a chunk of debris,
//! `assets/models/chunk.glb` (a jittered box about a unit across). The repository holds no one else's art.
//! Run it again after changing a shape:
//!
//! ```text
//! cargo run -p sandbox --example make_gore
//! ```
//!
//! The shapes are drawn from a seeded generator: the files come out the same every time.

use std::path::{Path, PathBuf};

use shards::bevy::asset::RenderAssetUsages;
use shards::bevy::image::Image;
use shards::bevy::math::ops;
use shards::bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use shards::serde_json::json;

fn main() {
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    png(&assets.join("textures/marks/bullet_hole.png"), 64, &bullet_hole(64));
    png(&assets.join("textures/marks/blood_splat.png"), 128, &blood_splat(128));
    chunk(&assets.join("models/chunk.glb"));
}

/// A small seeded generator (SplitMix64): the shapes are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

fn smooth(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A crater: a dark pit with a ragged rim, a pale ring of dust round it and four hairline cracks. RGBA, white
/// where it is dark (the mark's colour tints it) with the alpha carrying the shape. The edge is clear, as a decal's must be.
fn bullet_hole(side: usize) -> Vec<u8> {
    let mut rng = Rng(0xB011_E7);
    let cracks: Vec<(f32, f32)> = (0..4).map(|i| (i as f32 * 1.57 + rng.between(-0.4, 0.4), rng.between(0.55, 0.9))).collect();
    let mut data = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = ((x as f32 + 0.5) / side as f32 * 2.0 - 1.0, (y as f32 + 0.5) / side as f32 * 2.0 - 1.0);
            let r = (dx * dx + dy * dy).sqrt();
            let angle = ops::atan2(dy, dx);
            let rim = 0.22 + 0.04 * ops::sin(angle * 5.0) + 0.02 * ops::cos(angle * 11.0);
            let pit = 1.0 - smooth(rim - 0.05, rim, r);
            let dust = (1.0 - smooth(0.25, 0.9, r)) * 0.35;
            let crack = cracks
                .iter()
                .map(|&(a, reach)| {
                    let (sin, cos) = ops::sin_cos(a);
                    let along = dx * cos + dy * sin;
                    let across = (-dx * sin + dy * cos).abs();
                    if along > rim && along < reach { (1.0 - smooth(0.0, 0.025, across)) * (1.0 - (along - rim) / (reach - rim)) } else { 0.0 }
                })
                .fold(0.0f32, f32::max);
            let alpha = (pit.max(dust).max(crack * 0.8)).clamp(0.0, 1.0) * (1.0 - smooth(0.9, 1.0, r));
            let shade = if pit > 0.5 { 20 } else { 70 };
            data.extend([shade, shade, shade, (alpha * 255.0).round() as u8]);
        }
    }
    data
}

/// A splatter: a blot of overlapping discs with a few droplets thrown from it, soft at the edge. White with the
/// alpha carrying the shape: the mark's colour makes it blood.
fn blood_splat(side: usize) -> Vec<u8> {
    let mut rng = Rng(0xB100D);
    // (x, y, radius) in -1..1: a heavy middle, lobes round it, and droplets out on rays from the middle.
    let mut discs = vec![(0.0, 0.0, 0.34)];
    for i in 0..7 {
        let a = i as f32 * 0.9 + rng.between(-0.3, 0.3);
        let d = rng.between(0.18, 0.36);
        discs.push((d * ops::cos(a), d * ops::sin(a), rng.between(0.12, 0.2)));
    }
    for i in 0..10 {
        let a = i as f32 * 0.63 + rng.between(-0.25, 0.25);
        let d = rng.between(0.5, 0.88);
        discs.push((d * ops::cos(a), d * ops::sin(a), rng.between(0.025, 0.07)));
    }
    let mut data = Vec::with_capacity(side * side * 4);
    for y in 0..side {
        for x in 0..side {
            let (px, py) = ((x as f32 + 0.5) / side as f32 * 2.0 - 1.0, (y as f32 + 0.5) / side as f32 * 2.0 - 1.0);
            let cover = discs.iter().map(|&(cx, cy, r)| 1.0 - smooth(r - 0.03, r + 0.03, ((px - cx) * (px - cx) + (py - cy) * (py - cy)).sqrt())).fold(0.0f32, f32::max);
            let rim = 1.0 - smooth(0.92, 1.0, (px * px + py * py).sqrt());
            data.extend([255, 255, 255, (cover * rim * 255.0).round() as u8]);
        }
    }
    data
}

fn png(path: &Path, side: usize, rgba: &[u8]) {
    let size = Extent3d { width: side as u32, height: side as u32, depth_or_array_layers: 1 };
    let image = Image::new(size, TextureDimension::D2, rgba.to_vec(), TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    let picture = image.try_into_dynamic().expect("an RGBA8 image converts").to_rgba8();
    std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("the folder can be made");
    picture.save(path).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    println!("{} ({side}×{side}, {} KB)", path.display(), std::fs::metadata(path).map_or(0, |m| m.len()) / 1024);
}

/// A rock-like chunk: a box about a unit across whose eight corners are pulled in and out, flat-shaded (four
/// vertices a face, each face its own normal), as a binary glTF with one mesh and no material.
fn chunk(path: &Path) {
    let mut rng = Rng(0xC4_0C);
    let corner = |sx: f32, sy: f32, sz: f32, rng: &mut Rng| [sx * rng.between(0.34, 0.5), sy * rng.between(0.3, 0.46), sz * rng.between(0.34, 0.5)];
    let mut corners = Vec::new();
    for (sx, sy, sz) in [(-1.0, -1.0, -1.0), (1.0, -1.0, -1.0), (1.0, 1.0, -1.0), (-1.0, 1.0, -1.0), (-1.0, -1.0, 1.0), (1.0, -1.0, 1.0), (1.0, 1.0, 1.0), (-1.0, 1.0, 1.0)] {
        corners.push(corner(sx, sy, sz, &mut rng));
    }
    // The six faces as corner indices, wound counter-clockwise seen from outside.
    let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 4, 7, 3], [1, 2, 6, 5], [0, 1, 5, 4], [3, 7, 6, 2]];
    let (mut positions, mut normals, mut indices) = (Vec::<f32>::new(), Vec::<f32>::new(), Vec::<u16>::new());
    for face in faces {
        let p: Vec<[f32; 3]> = face.iter().map(|&i| corners[i]).collect();
        let (a, b) = ([p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]], [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]]);
        let n = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let start = (positions.len() / 3) as u16;
        for corner in &p {
            positions.extend(corner);
            normals.extend(n.map(|v| v / len));
        }
        indices.extend([start, start + 1, start + 2, start, start + 2, start + 3]);
    }
    let (mut bin, mut views, mut accessors) = (Vec::<u8>::new(), Vec::new(), Vec::new());
    let mut push = |bytes: Vec<u8>, target: u32, component: u32, count: usize, kind: &str, bounds: Option<([f32; 3], [f32; 3])>| {
        while bin.len() % 4 != 0 {
            bin.push(0);
        }
        views.push(json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len(), "target": target}));
        bin.extend(bytes);
        let mut accessor = json!({"bufferView": views.len() - 1, "componentType": component, "count": count, "type": kind});
        if let Some((min, max)) = bounds {
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        accessors.push(accessor);
    };
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in positions.chunks(3) {
        for axis in 0..3 {
            lo[axis] = lo[axis].min(p[axis]);
            hi[axis] = hi[axis].max(p[axis]);
        }
    }
    let floats = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    push(floats(&positions), 34962, 5126, positions.len() / 3, "VEC3", Some((lo, hi)));
    push(floats(&normals), 34962, 5126, normals.len() / 3, "VEC3", None);
    push(indices.iter().flat_map(|i| i.to_le_bytes()).collect(), 34963, 5123, indices.len(), "SCALAR", None);
    let document = json!({
        "asset": {"version": "2.0", "generator": "make_gore"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "chunk", "mesh": 0}],
        "meshes": [{"name": "chunk", "primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2}]}],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    let mut text = document.to_string().into_bytes();
    while text.len() % 4 != 0 {
        text.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let total = 12 + 8 + text.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend(b"glTF");
    glb.extend(2u32.to_le_bytes());
    glb.extend((total as u32).to_le_bytes());
    glb.extend((text.len() as u32).to_le_bytes());
    glb.extend(b"JSON");
    glb.extend(&text);
    glb.extend((bin.len() as u32).to_le_bytes());
    glb.extend(b"BIN\0");
    glb.extend(&bin);
    std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("the folder can be made");
    std::fs::write(path, &glb).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
    println!("{} ({} bytes)", path.display(), glb.len());
}
