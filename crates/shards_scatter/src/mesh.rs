//! The engine's scatter shapes as meshes: a tuft of blades, a cone, a lumpy rock, a box. Each
//! stands on its origin with +Y up, is made once per shape (the same numbers make the same mesh,
//! for every scatter that names it) and is white: the look gives the colour.

use std::f32::consts::TAU;

use ai_core::DetRng;
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::def::ScatterMesh;
use crate::place::fnv64;

/// The mesh of `shape`; `None` for a glTF mesh (the asset server loads it).
pub fn build(shape: &ScatterMesh) -> Option<Mesh> {
    // The same shape is jittered the same way in every run.
    let mut rng = DetRng::from_keys(&[fnv64(&shape.describe())]);
    match *shape {
        ScatterMesh::Blades { width, height, count } => Some(blades(width, height, count, &mut rng)),
        ScatterMesh::Cone { radius, height, sides } => Some(cone(radius, height, sides)),
        ScatterMesh::Rock { radius, detail, squash } => rock(radius, detail, squash, &mut rng),
        ScatterMesh::Box { size } => Some(Cuboid::from_size(size).mesh().build().translated_by(Vec3::Y * size.y * 0.5)),
        ScatterMesh::Gltf(_) => None,
    }
}

/// A tuft: `count` blades around the middle, each a triangle from a foot `width` wide to a tip
/// that leans outward, 70% to 100% of `height` tall. The normals point mostly up, so a tuft takes
/// the light as the ground under it does; the vertex colours darken the feet.
fn blades(width: f32, height: f32, count: u32, rng: &mut DetRng) -> Mesh {
    let mut positions = Vec::with_capacity(count as usize * 3);
    let mut normals = Vec::with_capacity(count as usize * 3);
    let mut colors = Vec::with_capacity(count as usize * 3);
    let foot = [0.45, 0.45, 0.45, 1.0];
    for i in 0..count {
        let angle = (i as f32 + rng.range(-0.3, 0.3)) / count as f32 * TAU;
        let out = Vec3::new(angle.cos(), 0.0, angle.sin());
        let across = Vec3::new(-out.z, 0.0, out.x) * width * 0.5;
        let base = if count == 1 { Vec3::ZERO } else { out * width * 0.6 };
        let tall = height * rng.range(0.7, 1.0);
        let tip = base + out * tall * rng.range(0.15, 0.4) + Vec3::Y * tall;
        let normal = (Vec3::Y * 2.0 + out * 0.5).normalize();
        positions.extend([base - across, base + across, tip].map(|v| v.to_array()));
        normals.extend([normal.to_array(); 3]);
        colors.extend([foot, foot, [1.0; 4]]);
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
}

/// A cone on the ground, flat faces, no bottom.
fn cone(radius: f32, height: f32, sides: u32) -> Mesh {
    let apex = Vec3::Y * height;
    let ring = |i: u32| {
        let angle = i as f32 / sides as f32 * TAU;
        Vec3::new(angle.cos() * radius, 0.0, angle.sin() * radius)
    };
    let mut triangles = Vec::with_capacity(sides as usize);
    for i in 0..sides {
        triangles.push([ring(i), apex, ring(i + 1)]);
    }
    faceted(&triangles)
}

/// A ball of ico faces (`detail` subdivisions), each corner pushed in or out a little, `squash`
/// times as tall as wide, a fifth of it under the ground.
fn rock(radius: f32, detail: u32, squash: f32, rng: &mut DetRng) -> Option<Mesh> {
    let mut mesh = Sphere::new(radius).mesh().ico(detail).ok()?;
    let positions: Vec<[f32; 3]> = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?.as_float3()?.to_vec();
    // Corners at the same place move together, so the surface stays closed.
    let mut moved: Vec<([i64; 3], f32)> = Vec::new();
    let lumpy: Vec<[f32; 3]> = positions
        .iter()
        .map(|p| {
            let key = p.map(|c| (c * 1e4).round() as i64);
            let factor = match moved.iter().find(|(k, _)| *k == key) {
                Some((_, f)) => *f,
                None => {
                    let f = rng.range(0.82, 1.12);
                    moved.push((key, f));
                    f
                }
            };
            let v = Vec3::from(*p) * factor;
            [v.x, v.y * squash + radius * squash * 0.6, v.z]
        })
        .collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, lumpy);
    mesh.remove_attribute(Mesh::ATTRIBUTE_UV_0);
    mesh.compute_smooth_normals();
    Some(mesh)
}

/// Triangles, each with its own corners and its face's normal, wound to face away from the
/// vertical axis (or up, on it).
fn faceted(triangles: &[[Vec3; 3]]) -> Mesh {
    let mut positions = Vec::with_capacity(triangles.len() * 3);
    let mut normals = Vec::with_capacity(triangles.len() * 3);
    for &[a, b, c] in triangles {
        let mut normal = (b - a).cross(c - a).normalize_or(Vec3::Y);
        let middle = (a + b + c) / 3.0;
        let outward = Vec3::new(middle.x, 0.0, middle.z).try_normalize().unwrap_or(Vec3::Y);
        let corners = if normal.dot(outward) >= 0.0 {
            [a, b, c]
        } else {
            normal = -normal;
            [a, c, b]
        };
        positions.extend(corners.map(|v| v.to_array()));
        normals.extend([normal.to_array(); 3]);
    }
    let count = positions.len() as u32;
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32((0..count).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(mesh: &Mesh) -> Vec<Vec3> {
        mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap().iter().map(|p| Vec3::from(*p)).collect()
    }

    fn triangles(mesh: &Mesh) -> usize {
        mesh.indices().map_or(mesh.count_vertices(), |i| i.len()) / 3
    }

    #[test]
    fn a_tuft_is_a_triangle_per_blade_standing_on_the_ground() {
        let shape = ScatterMesh::Blades { width: 0.1, height: 0.5, count: 7 };
        let tuft = build(&shape).unwrap();
        assert_eq!((tuft.count_vertices(), triangles(&tuft)), (21, 7));
        let ps = positions(&tuft);
        assert!(ps.iter().all(|p| p.y >= 0.0 && p.y <= 0.5), "between the ground and its height");
        assert!(ps.iter().any(|p| p.y > 0.34), "tips stand at least 70% tall");
        assert!(tuft.attribute(Mesh::ATTRIBUTE_COLOR).is_some(), "feet darker than tips");
        let normals = tuft.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().as_float3().unwrap();
        assert!(normals.iter().all(|n| n[1] > 0.9), "lit like the ground: normals mostly up");
        assert_eq!(positions(&build(&shape).unwrap()), ps, "the same shape, the same mesh");
    }

    #[test]
    fn cones_rocks_and_boxes_stand_on_their_origin() {
        let cone = build(&ScatterMesh::Cone { radius: 0.5, height: 1.2, sides: 6 }).unwrap();
        assert_eq!(triangles(&cone), 6);
        let ps = positions(&cone);
        assert!(ps.iter().all(|p| p.y >= 0.0) && ps.iter().any(|p| (p.y - 1.2).abs() < 1e-5));
        let normals = cone.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap().as_float3().unwrap();
        for (corner, normal) in ps.iter().zip(normals) {
            let side = Vec3::new(corner.x, 0.0, corner.z);
            assert!(side.length() < 1e-4 || Vec3::from(*normal).dot(side) > 0.0, "faces look outward");
        }

        let rock = build(&ScatterMesh::Rock { radius: 1.0, detail: 1, squash: 0.5 }).unwrap();
        assert_eq!(triangles(&rock), 80, "detail 1: 80 faces");
        let ys: Vec<f32> = positions(&rock).iter().map(|p| p.y).collect();
        let (low, high) = (ys.iter().copied().fold(f32::MAX, f32::min), ys.iter().copied().fold(f32::MIN, f32::max));
        assert!(low < 0.0 && low > -0.4 && high > 0.6 && high < 0.95, "squashed, its foot under the ground: {low}..{high}");
        assert_eq!(triangles(&build(&ScatterMesh::Rock { radius: 1.0, detail: 0, squash: 1.0 }).unwrap()), 20);

        let crate_ = build(&ScatterMesh::Box { size: Vec3::new(1.0, 0.6, 1.0) }).unwrap();
        let ys: Vec<f32> = positions(&crate_).iter().map(|p| p.y).collect();
        assert!(ys.iter().all(|y| (0.0..=0.6001).contains(y)));
        assert!(build(&ScatterMesh::Gltf("bush.glb#Mesh0/Primitive0".into())).is_none());
    }
}
