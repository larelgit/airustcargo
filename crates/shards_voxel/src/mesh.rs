//! Chunk meshes: one quad per block face that shows, coloured per vertex (the palette's face
//! colour, a per-face shade, a little per-block variation, and corner darkening where blocks
//! meet), or greedy: neighbouring faces of the same block and the same shade merged into one
//! quad. Presentation only: the mesher reads the volume and changes nothing.
//!
//! Two meshes per chunk, as the reference game draws them: terrain (solid and other opaque
//! blocks) and foliage (leaves: faces only toward air, never darkening their neighbours), and
//! water surface quads on top of every liquid column.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

use crate::noise::cell_hash;
use crate::palette::VoxelPalette;
use crate::volume::{VoxelVolume, AIR};

/// Normals of the six faces, in the order the reference game iterates them.
pub const DIRS: [IVec3; 6] = [IVec3::Y, IVec3::NEG_Y, IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z];
/// `(u, v)` axes of each face with `u × v` = the normal: corners in order `(0,0) (1,0) (1,1) (0,1)`
/// turn counter-clockwise seen from outside.
const AXES: [(IVec3, IVec3); 6] =
    [(IVec3::Z, IVec3::X), (IVec3::X, IVec3::Z), (IVec3::Y, IVec3::Z), (IVec3::Z, IVec3::Y), (IVec3::X, IVec3::Y), (IVec3::Y, IVec3::X)];
/// Light on each face: top full, bottom dark, the sides between.
const SHADE: [f32; 6] = [1.0, 0.4, 0.66, 0.66, 0.8, 0.8];

/// How faces become quads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meshing {
    /// One quad per face, with per-block colour variation.
    Faces,
    /// Faces of one block kind and one even shade merged into rectangles; no per-block variation.
    Greedy,
}

/// Vertex arrays of one mesh.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bytes(&self) -> usize {
        self.positions.len() * 12 + self.normals.len() * 12 + self.colors.len() * 16 + self.indices.len() * 4
    }

    /// A quad of corners `c` (counter-clockwise from outside), each with its own colour; the
    /// diagonal is chosen by the corner shades so the darkening does not crease.
    fn quad(&mut self, c: [Vec3; 4], normal: Vec3, colors: [[f32; 4]; 4], ao: [f32; 4]) {
        let base = self.positions.len() as u32;
        for i in 0..4 {
            self.positions.push(c[i].to_array());
            self.normals.push(normal.to_array());
            self.colors.push(colors[i]);
        }
        let order = if ao[0] + ao[2] <= ao[1] + ao[3] { [0, 1, 2, 0, 2, 3] } else { [0, 1, 3, 1, 2, 3] };
        self.indices.extend(order.map(|i| base + i));
    }

    /// The CPU-side Bevy mesh (no GPU upload: that happens when it is first drawn).
    pub fn into_mesh(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// The meshes of one chunk, positions relative to the chunk's least corner.
#[derive(Clone, Debug, Default)]
pub struct ChunkMeshes {
    pub chunk: UVec3,
    pub terrain: MeshData,
    pub foliage: MeshData,
    pub water: MeshData,
}

/// The cells of a chunk and one cell around it, read once (neighbour reads in the loops stay local).
struct Padded {
    size: IVec3,
    cells: Vec<u8>,
}

impl Padded {
    fn new(v: &VoxelVolume, lo: IVec3, size: IVec3) -> Self {
        let p = size + IVec3::splat(2);
        let mut cells = Vec::with_capacity((p.x * p.y * p.z) as usize);
        for x in -1..size.x + 1 {
            for z in -1..size.z + 1 {
                for y in -1..size.y + 1 {
                    cells.push(v.get(lo + IVec3::new(x, y, z)));
                }
            }
        }
        Self { size, cells }
    }

    /// The block at local `p` (`-1..=size`).
    fn at(&self, p: IVec3) -> u8 {
        let q = p + IVec3::ONE;
        let s = self.size + IVec3::splat(2);
        self.cells[((q.x * s.z + q.z) * s.y + q.y) as usize]
    }
}

/// Corner darkening of corner `(cu, cv)` of the face of local cell `p` toward `dir`: the
/// cells beside the corner in front of the face. Two sides shut it: 0.55.
fn corner_ao(pad: &Padded, palette: &VoxelPalette, p: IVec3, dir: usize, cu: i32, cv: i32) -> f32 {
    let (u, v) = AXES[dir];
    let front = p + DIRS[dir];
    let (su, sv) = (u * (2 * cu - 1), v * (2 * cv - 1));
    let occ = |q: IVec3| palette.occludes(pad.at(q)) as u32;
    let (s1, s2, c) = (occ(front + su), occ(front + sv), occ(front + su + sv));
    if s1 == 1 && s2 == 1 { 0.55 } else { 1.0 - 0.15 * (s1 + s2 + c) as f32 }
}

/// Whether the face of a `id` block toward a `nb` block shows.
fn shows(palette: &VoxelPalette, id: u8, nb: u8) -> bool {
    if palette.is_foliage(id) { nb == AIR || (!palette.visible(nb)) } else { !palette.occludes(nb) }
}

const CORNERS: [(i32, i32); 4] = [(0, 0), (1, 0), (1, 1), (0, 1)];

/// The corner of the face of cell `p` toward `dir` at `(cu, cv)` (local cell units).
fn corner(p: IVec3, dir: usize, cu: i32, cv: i32) -> Vec3 {
    let n = DIRS[dir];
    let (u, v) = AXES[dir];
    // The face plane: the far side of the cell along a positive normal.
    let base = p + n.max(IVec3::ZERO);
    (base + u * cu + v * cv).as_vec3()
}

/// Meshes chunk `chunk` (chunk coordinates) of `v`.
pub fn mesh_chunk(v: &VoxelVolume, palette: &VoxelPalette, chunk: UVec3, meshing: Meshing) -> ChunkMeshes {
    let size = v.chunk().as_ivec3();
    let lo = chunk.as_ivec3() * size;
    let pad = Padded::new(v, lo, size);
    let block = v.block();
    let mut out = ChunkMeshes { chunk, ..default() };
    if pad.cells.iter().all(|id| *id == AIR) {
        return out;
    }
    // Water: a quad at the top of every liquid cell with air (or foliage) above it.
    for x in 0..size.x {
        for z in 0..size.z {
            for y in 0..size.y {
                let p = IVec3::new(x, y, z);
                let id = pad.at(p);
                if palette.def(id).is_some_and(|d| d.liquid) && pad.at(p + IVec3::Y) == AIR {
                    let top = (y + 1) as f32 - 0.05;
                    let c = [Vec3::new(x as f32, top, z as f32), Vec3::new(x as f32, top, z as f32 + 1.0), Vec3::new(x as f32 + 1.0, top, z as f32 + 1.0), Vec3::new(x as f32 + 1.0, top, z as f32)];
                    let col = palette.def(id).map_or([1.0; 3], |d| d.top);
                    let rgba = [col[0], col[1], col[2], 1.0];
                    out.water.quad(c.map(|c| c * block), Vec3::Y, [rgba; 4], [1.0; 4]);
                }
            }
        }
    }
    match meshing {
        Meshing::Faces => faces(&pad, palette, lo, block, &mut out),
        Meshing::Greedy => greedy(&pad, palette, block, &mut out),
    }
    out
}

fn faces(pad: &Padded, palette: &VoxelPalette, lo: IVec3, block: f32, out: &mut ChunkMeshes) {
    let size = pad.size;
    for x in 0..size.x {
        for z in 0..size.z {
            for y in 0..size.y {
                let p = IVec3::new(x, y, z);
                let id = pad.at(p);
                if id == AIR || !palette.visible(id) {
                    continue;
                }
                let Some(def) = palette.def(id) else { continue };
                let foliage = palette.is_foliage(id);
                let world = lo + p;
                let patch = cell_hash(world.x >> 2, 0, world.z >> 2, 0x5eed);
                let vary = (0.9 + 0.16 * cell_hash(world.x, world.y, world.z, 0xb10c)) * (0.94 + 0.12 * patch);
                for dir in 0..6 {
                    if !shows(palette, id, pad.at(p + DIRS[dir])) {
                        continue;
                    }
                    let base = def.face_color(dir);
                    let (shade, plain) = if def.emissive { (1.0, true) } else { (SHADE[dir] * vary, foliage) };
                    let ao = CORNERS.map(|(cu, cv)| if plain { 1.0 } else { corner_ao(pad, palette, p, dir, cu, cv) });
                    let colors = ao.map(|a| [base[0] * shade * a, base[1] * shade * a, base[2] * shade * a, 1.0]);
                    let c = CORNERS.map(|(cu, cv)| corner(p, dir, cu, cv) * block);
                    let mesh = if foliage { &mut out.foliage } else { &mut out.terrain };
                    mesh.quad(c, DIRS[dir].as_vec3(), colors, ao);
                }
            }
        }
    }
}

/// Greedy: per face direction and slice, a mask of face keys (block id and an even shade, or a
/// one-off key for a face whose corners differ), merged into rectangles of equal keys.
fn greedy(pad: &Padded, palette: &VoxelPalette, block: f32, out: &mut ChunkMeshes) {
    let size = pad.size;
    for dir in 0..6 {
        let n = DIRS[dir];
        let (u, v) = AXES[dir];
        let axis = |a: IVec3, s: IVec3| a.x * s.x + a.y * s.y + a.z * s.z;
        let (dn, du, dv) = (axis(n.abs(), size), axis(u, size), axis(v, size));
        let mut mask: Vec<Option<(u32, u8, [f32; 4])>> = vec![None; (du * dv) as usize];
        for s in 0..dn {
            mask.iter_mut().for_each(|m| *m = None);
            let mut odd = 1u32 << 16;
            for j in 0..dv {
                for i in 0..du {
                    let p = n.abs() * s + u * i + v * j;
                    let id = pad.at(p);
                    if id == AIR || !palette.visible(id) || !shows(palette, id, pad.at(p + n)) {
                        continue;
                    }
                    let def = palette.def(id).expect("visible blocks have a definition");
                    let plain = def.emissive || palette.is_foliage(id);
                    let ao = CORNERS.map(|(cu, cv)| if plain { 1.0 } else { corner_ao(pad, palette, p, dir, cu, cv) });
                    let level = |a: f32| ((1.0 - a) / 0.15).round() as u32;
                    let key = if ao.iter().all(|a| *a == ao[0]) {
                        (id as u32) | (level(ao[0]) << 8)
                    } else {
                        odd += 1;
                        odd
                    };
                    mask[(j * du + i) as usize] = Some((key, id, ao));
                }
            }
            for j in 0..dv {
                let mut i = 0;
                while i < du {
                    let Some((key, id, ao)) = mask[(j * du + i) as usize] else {
                        i += 1;
                        continue;
                    };
                    let same = |m: &Option<(u32, u8, [f32; 4])>| m.is_some_and(|(k, _, _)| k == key);
                    let mut w = 1;
                    while i + w < du && same(&mask[(j * du + i + w) as usize]) {
                        w += 1;
                    }
                    let mut h = 1;
                    while j + h < dv && (i..i + w).all(|ii| same(&mask[((j + h) * du + ii) as usize])) {
                        h += 1;
                    }
                    for jj in j..j + h {
                        for ii in i..i + w {
                            mask[(jj * du + ii) as usize] = None;
                        }
                    }
                    let def = palette.def(id).expect("visible");
                    let base = def.face_color(dir);
                    let shade = if def.emissive { 1.0 } else { SHADE[dir] };
                    let colors = ao.map(|a| [base[0] * shade * a, base[1] * shade * a, base[2] * shade * a, 1.0]);
                    let p = n.abs() * s + u * i + v * j;
                    let origin = corner(p, dir, 0, 0);
                    let (uw, vh) = ((u * w).as_vec3(), (v * h).as_vec3());
                    let c = [origin, origin + uw, origin + uw + vh, origin + vh].map(|c| c * block);
                    let mesh = if palette.is_foliage(id) { &mut out.foliage } else { &mut out.terrain };
                    mesh.quad(c, n.as_vec3(), colors, ao);
                    i += w;
                }
            }
        }
    }
}

/// Meshes every chunk, in chunk order.
pub fn mesh_all(v: &VoxelVolume, palette: &VoxelPalette, meshing: Meshing) -> Vec<ChunkMeshes> {
    let n = v.chunk_count();
    let mut out = Vec::with_capacity((n.x * n.y * n.z) as usize);
    for cz in 0..n.z {
        for cy in 0..n.y {
            for cx in 0..n.x {
                out.push(mesh_chunk(v, palette, UVec3::new(cx, cy, cz), meshing));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::test_palette;

    #[test]
    fn a_lone_block_has_six_faces_and_two_blocks_ten() {
        let p = test_palette();
        let stone = p.id("stone").unwrap();
        let mut v = VoxelVolume::new(UVec3::splat(8), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        v.set(IVec3::new(3, 3, 3), stone);
        let m = mesh_chunk(&v, &p, UVec3::ZERO, Meshing::Faces);
        assert_eq!(m.terrain.triangles(), 12);
        v.set(IVec3::new(4, 3, 3), stone);
        let m = mesh_chunk(&v, &p, UVec3::ZERO, Meshing::Faces);
        assert_eq!(m.terrain.triangles(), 20);
        let g = mesh_chunk(&v, &p, UVec3::ZERO, Meshing::Greedy);
        assert_eq!(g.terrain.triangles(), 12, "greedy merges the two blocks' faces");
    }

    #[test]
    fn faces_turn_outward() {
        let p = test_palette();
        let mut v = VoxelVolume::new(UVec3::splat(8), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        v.set(IVec3::new(2, 2, 2), p.id("stone").unwrap());
        let m = mesh_chunk(&v, &p, UVec3::ZERO, Meshing::Faces).terrain;
        for t in m.indices.chunks(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(m.positions[i as usize]));
            let n = Vec3::from(m.normals[t[0] as usize]);
            assert!((b - a).cross(c - a).dot(n) > 0.0, "counter-clockwise seen from outside");
        }
    }

    #[test]
    fn corners_darken_where_blocks_meet() {
        let p = test_palette();
        let stone = p.id("stone").unwrap();
        let mut v = VoxelVolume::new(UVec3::splat(8), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        v.fill(IVec3::new(0, 0, 0), IVec3::new(8, 1, 8), stone);
        v.set(IVec3::new(4, 1, 4), stone);
        let m = mesh_chunk(&v, &p, UVec3::ZERO, Meshing::Faces).terrain;
        let dark = m.colors.iter().filter(|c| c[0] < p.def(stone).unwrap().top[0] * 0.8).count();
        assert!(dark > 0, "the floor around the block is darker at its foot");
    }
}
