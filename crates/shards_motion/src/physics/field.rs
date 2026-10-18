//! [`VoxelField`]: a static grid of solid cells that [`PhysicsWorld`](super::PhysicsWorld) asks
//! besides its colliders (G1 spike, collision candidate (b)).
//!
//! A ray walks the cells it crosses (Amanatides–Woo); a moving or overlapping shape is tested
//! against the solid cells under its bounding box, each as a box. Cost grows with the cells a
//! query touches, never with the size of the field, and a hit knows its cell: what upstream
//! code reads off a collider's bounding box (how tall a blocker is, where the floor under a
//! solid is) comes from the cell's column instead.

use bevy::prelude::*;

use super::CollisionLayers;

/// Solid cells `size` big, each `cell` m on a side, the least corner of cell `(0, 0, 0)` at
/// `origin`. Outside it nothing is solid.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelField {
    pub origin: Vec3,
    pub cell: f32,
    pub size: UVec3,
    pub layers: CollisionLayers,
    /// One bit per cell, `(y * size.z + z) * size.x + x`.
    bits: Vec<u64>,
}

/// What a ray met in a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldRay {
    pub distance: f32,
    pub normal: Vec3,
    pub cell: IVec3,
}

impl VoxelField {
    pub fn new(origin: Vec3, cell: f32, size: UVec3, solid: impl Fn(IVec3) -> bool) -> Self {
        let count = (size.x * size.y * size.z) as usize;
        let mut bits = vec![0u64; count.div_ceil(64)];
        for y in 0..size.y as i32 {
            for z in 0..size.z as i32 {
                for x in 0..size.x as i32 {
                    let c = IVec3::new(x, y, z);
                    if solid(c) {
                        let i = Self::index_of(size, c);
                        bits[i / 64] |= 1 << (i % 64);
                    }
                }
            }
        }
        Self { origin, cell, size, layers: CollisionLayers::WORLD, bits }
    }

    fn index_of(size: UVec3, c: IVec3) -> usize {
        ((c.y as u32 * size.z + c.z as u32) * size.x + c.x as u32) as usize
    }

    pub fn inside(&self, c: IVec3) -> bool {
        c.cmpge(IVec3::ZERO).all() && c.cmplt(self.size.as_ivec3()).all()
    }

    pub fn solid(&self, c: IVec3) -> bool {
        if !self.inside(c) {
            return false;
        }
        let i = Self::index_of(self.size, c);
        self.bits[i / 64] & (1 << (i % 64)) != 0
    }

    /// Solid with a face toward a cell that is not: the only cells a shape coming from outside
    /// can meet first.
    pub fn exposed(&self, c: IVec3) -> bool {
        self.solid(c) && [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z].iter().any(|d| !self.solid(c + *d))
    }

    pub fn memory(&self) -> usize {
        self.bits.len() * 8
    }

    /// World centre of cell `c`.
    pub fn center(&self, c: IVec3) -> Vec3 {
        self.origin + (c.as_vec3() + Vec3::splat(0.5)) * self.cell
    }

    /// The cells (inclusive range) a world box touches, clipped to the field; `None` if it
    /// misses the field or is not finite.
    pub fn cells_in(&self, lo: Vec3, hi: Vec3) -> Option<(IVec3, IVec3)> {
        if !(lo.is_finite() && hi.is_finite()) {
            return None;
        }
        let a = ((lo - self.origin) / self.cell).floor();
        let b = ((hi - self.origin) / self.cell).floor();
        let top = self.size.as_vec3() - Vec3::ONE;
        if b.cmplt(Vec3::ZERO).any() || a.cmpgt(top).any() {
            return None;
        }
        Some((a.max(Vec3::ZERO).as_ivec3(), b.min(top).as_ivec3()))
    }

    /// The top (world height) of the run of solid cells holding the cell at world
    /// `(x, at, z)`, `None` if that cell is not solid: how tall the blocker there is.
    pub fn column_top(&self, x: f32, z: f32, at: f32) -> Option<f32> {
        let c = ((Vec3::new(x, at, z) - self.origin) / self.cell).floor().as_ivec3();
        if c.x < 0 || c.z < 0 || c.x >= self.size.x as i32 || c.z >= self.size.z as i32 {
            return None;
        }
        let mut y = c.y;
        if !self.solid(IVec3::new(c.x, y, c.z)) {
            return None;
        }
        while y + 1 < self.size.y as i32 && self.solid(IVec3::new(c.x, y + 1, c.z)) {
            y += 1;
        }
        Some(self.origin.y + (y + 1) as f32 * self.cell)
    }

    /// The bottom (world height) of the run of solid cells holding the cell at world
    /// `(x, at, z)`, `None` if that cell is not solid: where the floor under a solid is.
    pub fn column_bottom(&self, x: f32, z: f32, at: f32) -> Option<f32> {
        let mut c = ((Vec3::new(x, at, z) - self.origin) / self.cell).floor().as_ivec3();
        if !self.solid(c) {
            return None;
        }
        while c.y > 0 && self.solid(c - IVec3::Y) {
            c.y -= 1;
        }
        Some(self.origin.y + c.y as f32 * self.cell)
    }

    /// The first solid cell along the ray within `max` m (`dir` unit length). A ray that starts
    /// in a solid cell hits it at 0, facing back along the ray.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<FieldRay> {
        if !(origin.is_finite() && dir.is_finite()) || max < 0.0 {
            return None;
        }
        let o = (origin - self.origin) / self.cell;
        let reach = if max.is_finite() { max / self.cell } else { f32::MAX };
        let size = self.size.as_vec3();
        // Clip to the field's box.
        let (mut t0, mut t1, mut axis0) = (0.0_f32, reach, None);
        for a in 0..3 {
            let (oa, da) = (o[a], dir[a]);
            if da.abs() < 1e-12 {
                if oa < 0.0 || oa >= size[a] {
                    return None;
                }
                continue;
            }
            let (n, f) = ((0.0 - oa) / da, (size[a] - oa) / da);
            let (near, far) = if n < f { (n, f) } else { (f, n) };
            if near > t0 {
                t0 = near;
                axis0 = Some(a);
            }
            t1 = t1.min(far);
            if t0 > t1 {
                return None;
            }
        }
        let start = o + dir * t0;
        let mut cell = start.floor().as_ivec3().clamp(IVec3::ZERO, self.size.as_ivec3() - IVec3::ONE);
        let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
        let mut normal = match axis0 {
            Some(a) => {
                let mut n = Vec3::ZERO;
                n[a] = -(step[a] as f32);
                n
            }
            None => -dir,
        };
        let mut t_max = Vec3::splat(f32::INFINITY);
        let mut t_delta = Vec3::splat(f32::INFINITY);
        for a in 0..3 {
            if step[a] != 0 {
                let next = if step[a] > 0 { cell[a] as f32 + 1.0 } else { cell[a] as f32 };
                t_max[a] = t0 + (next - start[a]) / dir[a];
                t_delta[a] = 1.0 / dir[a].abs();
            }
        }
        let mut t = t0;
        loop {
            if self.solid(cell) {
                return Some(FieldRay { distance: t * self.cell, normal, cell });
            }
            let a = if t_max.x <= t_max.y && t_max.x <= t_max.z {
                0
            } else if t_max.y <= t_max.z {
                1
            } else {
                2
            };
            t = t_max[a];
            if t > t1 {
                return None;
            }
            cell[a] += step[a];
            if cell[a] < 0 || cell[a] >= self.size[a] as i32 {
                return None;
            }
            t_max[a] += t_delta[a];
            normal = Vec3::ZERO;
            normal[a] = -(step[a] as f32);
        }
    }
}

fn sign(v: f32) -> i32 {
    if v > 0.0 {
        1
    } else if v < 0.0 {
        -1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_meet_the_first_solid_cell_and_its_face() {
        // A floor at y = 0 (one layer) and a pillar at (5, 1..4, 5).
        let f = VoxelField::new(Vec3::new(-8.0, -1.0, -8.0), 1.0, UVec3::new(16, 8, 16), |c| c.y == 0 || (c.x == 5 && c.z == 5 && c.y < 4));
        let down = f.raycast(Vec3::new(0.5, 5.0, 0.5), Vec3::NEG_Y, 20.0).unwrap();
        assert!((down.distance - 5.0).abs() < 1e-5 && down.normal == Vec3::Y, "{down:?}");
        let side = f.raycast(Vec3::new(-6.0, 1.5, -2.5), Vec3::X, 20.0).unwrap();
        assert_eq!((side.cell, side.normal), (IVec3::new(5, 2, 5), Vec3::NEG_X));
        assert!((side.distance - 3.0).abs() < 1e-5, "{side:?}");
        assert!(f.raycast(Vec3::new(-6.0, 1.5, -2.5), Vec3::X, 2.5).is_none(), "too short");
        assert!(f.raycast(Vec3::new(0.5, 20.0, 0.5), Vec3::Y, 50.0).is_none(), "above and away");
        let from_above = f.raycast(Vec3::new(0.5, 50.0, 0.5), Vec3::NEG_Y, 100.0).unwrap();
        assert!((from_above.distance - 50.0).abs() < 1e-4, "enters the field from outside");
        let inside = f.raycast(Vec3::new(0.5, -0.5, 0.5), Vec3::X, 3.0).unwrap();
        assert_eq!(inside.distance, 0.0);
        assert_eq!(f.column_top(-2.5, -2.5, 1.5), Some(3.0), "the pillar's top: cells 0..4 above y = -1");
        assert_eq!(f.column_top(0.5, 0.5, 3.0), None, "air at that height");
        assert_eq!(f.column_top(0.5, 0.5, -0.5), Some(0.0));
        assert_eq!(f.column_bottom(-2.5, -2.5, 2.5), Some(-1.0), "the pillar stands on the floor");
        assert_eq!(f.column_bottom(0.5, 0.5, 3.0), None);
    }

    #[test]
    fn a_diagonal_ray_matches_a_slab_test() {
        let f = VoxelField::new(Vec3::ZERO, 0.5, UVec3::new(32, 16, 32), |c| c.y < 2 + (c.x + c.z) % 5);
        for i in 0..200 {
            let a = i as f32 * 0.37;
            let origin = Vec3::new(8.0 + ops::cos(a) * 3.0, 7.0, 8.0 + ops::sin(a) * 3.0);
            let dir = Vec3::new(ops::cos(a * 1.7) * 0.3, -1.0 - (i % 3) as f32, ops::sin(a * 1.3) * 0.3).normalize();
            let hit = f.raycast(origin, dir, 30.0).expect("the floor is everywhere");
            let p = origin + dir * hit.distance;
            // The hit point is on the cell's face, and every cell before it on the way is air.
            let cell_min = f.origin + hit.cell.as_vec3() * f.cell;
            assert!(p.cmpge(cell_min - 1e-3).all() && p.cmple(cell_min + Vec3::splat(f.cell) + 1e-3).all(), "{i}: {p} in {:?}", hit.cell);
            let mut t = 0.0;
            while t < hit.distance - 1e-3 {
                let c = ((origin + dir * t - f.origin) / f.cell).floor().as_ivec3();
                assert!(!f.solid(c), "{i}: a solid cell {c} before the hit");
                t += 0.01;
            }
        }
    }
}
