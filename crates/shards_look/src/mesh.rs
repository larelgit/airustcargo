//! The meshes a look draws beside the game's own: a flat-faced copy (low-poly facets) and the
//! outline's hull. Both are made once per source mesh, from its data in the main world; a mesh
//! whose data has gone to the GPU only, or that is no triangle list, gets neither.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;

/// A copy of `mesh` with flat faces: every triangle its own three vertices, facing its own way,
/// every other attribute (colours, UVs, skin) kept. `None` when it cannot be flattened: not a
/// triangle list, no positions, morph targets (they count vertices), data on the GPU only.
pub fn flat(mesh: &Mesh) -> Option<Mesh> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList || mesh.try_has_morph_targets().unwrap_or(true) {
        return None;
    }
    mesh.try_attribute(Mesh::ATTRIBUTE_POSITION).ok()?.as_float3()?;
    let mut flat = mesh.clone();
    flat.try_duplicate_vertices().ok()?;
    flat.try_compute_flat_normals().ok()?;
    Some(flat)
}

/// The outline's hull: `mesh`'s positions, triangles and skin, with normals averaged over every
/// vertex at the same place, so pushing the vertices out along them opens no gap where faces
/// meet at an edge (a cube's corners, a flat-shaded mesh). `None` as for [`flat`] (morph targets
/// would move the mesh away from its hull), or for no triangles at all.
pub fn hull(mesh: &Mesh) -> Option<Mesh> {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList || mesh.try_has_morph_targets().unwrap_or(true) {
        return None;
    }
    let positions = mesh.try_attribute(Mesh::ATTRIBUTE_POSITION).ok()?.as_float3()?.to_vec();
    let indices: Vec<usize> = match mesh.try_indices_option().ok()? {
        Some(indices) => indices.iter().collect(),
        None => (0..positions.len()).collect(),
    };
    if indices.len() < 3 || indices.iter().any(|&i| i >= positions.len()) {
        return None;
    }
    let normals = welded_normals(&positions, &indices);
    let mut hull = Mesh::new(PrimitiveTopology::TriangleList, mesh.asset_usage | RenderAssetUsages::RENDER_WORLD);
    hull.try_insert_attribute(Mesh::ATTRIBUTE_POSITION, positions).ok()?;
    hull.try_insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals).ok()?;
    for skin in [Mesh::ATTRIBUTE_JOINT_INDEX, Mesh::ATTRIBUTE_JOINT_WEIGHT] {
        if let Some(values) = mesh.try_attribute_option(skin).ok()? {
            hull.try_insert_attribute(skin, values.clone()).ok()?;
        }
    }
    if let Some(indices) = mesh.try_indices_option().ok()? {
        hull.try_insert_indices(indices.clone()).ok()?;
    }
    Some(hull)
}

/// Per vertex, the normals of the triangles that touch its place (positions within 0.1 mm are
/// one place), each weighted by its angle there: however a face is cut into triangles, it counts
/// once, so a cube's corner points along its diagonal.
fn welded_normals(positions: &[[f32; 3]], indices: &[usize]) -> Vec<[f32; 3]> {
    let place = |p: [f32; 3]| p.map(|c| (c * 1e4).round() as i64);
    let mut places: HashMap<[i64; 3], Vec3> = HashMap::new();
    for triangle in indices.as_chunks::<3>().0 {
        let corners = triangle.map(|i| Vec3::from(positions[i]));
        let Some(face) = (corners[1] - corners[0]).cross(corners[2] - corners[0]).try_normalize() else {
            continue;
        };
        for k in 0..3 {
            let at = corners[k];
            let angle = (corners[(k + 1) % 3] - at).angle_between(corners[(k + 2) % 3] - at);
            *places.entry(place(positions[triangle[k]])).or_default() += face * angle;
        }
    }
    positions.iter().map(|p| places.get(&place(*p)).and_then(|n| n.try_normalize()).unwrap_or(Vec3::Y).to_array()).collect()
}

/// The normals of `mesh`, when it has them as three floats.
pub fn normals(mesh: &Mesh) -> Option<&[[f32; 3]]> {
    match mesh.try_attribute(Mesh::ATTRIBUTE_NORMAL).ok()? {
        VertexAttributeValues::Float32x3(values) => Some(values),
        _ => None,
    }
}

/// How many indices a mesh draws (vertices without indices).
pub fn drawn_vertices(mesh: &Mesh) -> usize {
    match mesh.try_indices_option() {
        Ok(Some(Indices::U16(i))) => i.len(),
        Ok(Some(Indices::U32(i))) => i.len(),
        _ => mesh.count_vertices(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_mesh_has_three_vertices_a_triangle_all_facing_its_way() {
        let sphere = Mesh::from(Sphere::new(1.0));
        let triangles = drawn_vertices(&sphere) / 3;
        let flat = flat(&sphere).unwrap();
        assert_eq!(flat.count_vertices(), triangles * 3);
        assert!(flat.try_indices_option().unwrap().is_none());
        let positions = flat.try_attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap();
        let normals = normals(&flat).unwrap();
        for (corners, faces) in positions.as_chunks::<3>().0.iter().zip(normals.as_chunks::<3>().0) {
            assert!(faces[0] == faces[1] && faces[1] == faces[2], "one normal a face");
            let centre = (Vec3::from(corners[0]) + Vec3::from(corners[1]) + Vec3::from(corners[2])) / 3.0;
            assert!(Vec3::from(faces[0]).dot(centre) > 0.0, "facing out");
        }
        assert!(flat.try_attribute(Mesh::ATTRIBUTE_UV_0).is_ok(), "other attributes stay");
    }

    #[test]
    fn a_hull_points_every_corner_of_a_cube_out_along_its_diagonal() {
        // 24 vertices: each corner three times, once per face, with that face's normal.
        let cube = Mesh::from(Cuboid::new(2.0, 2.0, 2.0));
        let hull = hull(&cube).unwrap();
        let positions = hull.try_attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap();
        for (p, n) in positions.iter().zip(normals(&hull).unwrap()) {
            let diagonal = Vec3::from(*p).normalize();
            assert!(Vec3::from(*n).dot(diagonal) > 0.999, "{p:?} pushed along {n:?}");
        }
        assert_eq!(drawn_vertices(&hull), drawn_vertices(&cube), "the same triangles");
        assert!(hull.try_attribute(Mesh::ATTRIBUTE_UV_0).is_err(), "only what the outline draws with");
    }

    #[test]
    fn what_is_no_triangle_list_is_not_flattened_or_outlined() {
        let lines = Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]]);
        assert!(flat(&lines).is_none() && hull(&lines).is_none());
        let empty = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        assert!(flat(&empty).is_none() && hull(&empty).is_none());
    }

    #[test]
    fn a_mesh_with_morph_targets_is_not_flattened_or_outlined() {
        let mut face = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        assert!(flat(&face).is_some() && hull(&face).is_some());
        face.set_morph_targets(vec![bevy::mesh::morph::MorphAttributes::default(); face.count_vertices()]);
        assert!(flat(&face).is_none() && hull(&face).is_none());
    }

    #[test]
    fn a_mesh_whose_data_went_to_the_gpu_is_not_flattened_or_outlined() {
        let mut sphere = Mesh::from(Sphere::new(1.0));
        sphere.take_gpu_data().unwrap();
        assert!(flat(&sphere).is_none() && hull(&sphere).is_none());
    }
}
