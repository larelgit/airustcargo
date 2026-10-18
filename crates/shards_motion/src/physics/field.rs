//! [`VoxelField`]: a static grid of solid cells that [`PhysicsWorld`](super::PhysicsWorld) asks
//! besides its colliders. A [`VoxelCollider`](super::VoxelCollider) puts one in.
//!
//! A ray walks the cells it crosses (Amanatides–Woo) and tests the solid ones it touches as
//! boxes (a ray along a seam between solid cells is inside them); a moving or overlapping shape
//! is tested against the solid cells under its bounding box, each as a box (a cast that covers
//! several cells merges them into column runs first; see `world.rs`). Cost grows with the cells
//! a query touches, never with the size of the field. A field has no bounding box of its own
//! for readers: what upstream code used to read off a
//! collider's box (how tall a blocker is, where the floor under a solid is) it asks through the
//! hooks [`PhysicsWorld::extent_at`](super::PhysicsWorld::extent_at),
//! [`top_under`](super::PhysicsWorld::top_under) and
//! [`solid_span`](super::PhysicsWorld::solid_span), which answer from the cells' column runs.
//!
//! No transcendental function anywhere: the walk is `floor`, division and comparisons.

use bevy::prelude::*;
use parry3d::math::Pose;
use parry3d::query::{Ray, RayCast};
use parry3d::shape::Cuboid;

use super::world::ray_hits_aabb;
use super::CollisionLayers;

/// How near (relative, in cells) a ray must be to a face to touch the cells on both sides of it,
/// and two of its face crossings to be one (through an edge or a corner): far above rounding,
/// far below any distance that matters.
const NEAR: f32 = 1e-5;

/// Solid cells `size` big, each `cell` m on a side, the least corner of cell `(0, 0, 0)` at
/// `origin`, in world space (the entity's `Transform` is not read). Outside it nothing is solid.
/// Built once; it never changes (an edit makes a new field).
#[derive(Clone, PartialEq)]
pub struct VoxelField {
    origin: Vec3,
    cell: f32,
    size: UVec3,
    layers: CollisionLayers,
    /// One bit per cell, `(x * size.z + z) * size.y + y`: a column's cells are next to each other.
    bits: Vec<u64>,
    solid_cells: u64,
    /// The lowest and the highest layer (`y`) a solid cell is in.
    layers_used: Option<(u32, u32)>,
    /// Casts merge the cells under them into column runs (the default; see
    /// [`with_cast_merge`](Self::with_cast_merge)).
    merge_casts: bool,
    /// FNV-1a over the size, the cell, the origin, the cast mode and the bits (not the layers).
    hash: u64,
}

impl std::fmt::Debug for VoxelField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VoxelField {{ {}x{}x{} cells of {} m at {}, {} solid, {:?}{}, hash {:016x} }}",
            self.size.x,
            self.size.y,
            self.size.z,
            self.cell,
            self.origin,
            self.solid_cells,
            self.layers,
            if self.merge_casts { "" } else { ", casts cell by cell" },
            self.hash
        )
    }
}

/// What a ray met in a field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldRay {
    /// Along the ray from its origin (m); 0 when the origin is inside a solid cell.
    pub distance: f32,
    /// The face's normal, facing the ray's origin; zero when the origin is inside.
    pub normal: Vec3,
    pub cell: IVec3,
}

impl VoxelField {
    /// Cells for which `solid` says yes are solid. On [`CollisionLayers::WORLD`].
    ///
    /// # Panics
    ///
    /// When `cell` is not positive and finite, `origin` is not finite, or the field has more
    /// than 2^32 cells.
    pub fn new(origin: Vec3, cell: f32, size: UVec3, mut solid: impl FnMut(IVec3) -> bool) -> Self {
        assert!(cell > 0.0 && cell.is_finite(), "a voxel field's cell must be a positive length, not {cell}");
        assert!(origin.is_finite(), "a voxel field's origin must be finite, not {origin}");
        let count = u64::from(size.x) * u64::from(size.y) * u64::from(size.z);
        assert!(count <= u64::from(u32::MAX), "a voxel field of {size} cells is too big (at most 2^32 cells)");
        let mut bits = vec![0u64; (count as usize).div_ceil(64)];
        let (mut solid_cells, mut low, mut high) = (0u64, u32::MAX, 0u32);
        for x in 0..size.x {
            for z in 0..size.z {
                for y in 0..size.y {
                    if solid(IVec3::new(x as i32, y as i32, z as i32)) {
                        let i = ((x * size.z + z) * size.y + y) as usize;
                        bits[i / 64] |= 1 << (i % 64);
                        solid_cells += 1;
                        low = low.min(y);
                        high = high.max(y);
                    }
                }
            }
        }
        let mut field = Self {
            origin,
            cell,
            size,
            layers: CollisionLayers::WORLD,
            bits,
            solid_cells,
            layers_used: (solid_cells > 0).then_some((low, high)),
            merge_casts: true,
            hash: 0,
        };
        field.hash = field.fnv();
        field
    }

    /// The same cells on other layers (a field is on [`CollisionLayers::WORLD`] unless told).
    pub fn with_layers(mut self, layers: CollisionLayers) -> Self {
        self.layers = layers;
        self
    }

    /// The same cells, cast against cell by cell (`false`) instead of merged into column runs
    /// (`true`, the default): what tests compare the merge with. Cell by cell, a cast answers as a
    /// world of one cuboid collider per cell (ids in `(x, y, z)` order) does, to the bit; merged,
    /// the same up to rounding, and faster. It is part of the field and of its
    /// [`hash`](Self::hash) (so the physics snapshot check sees it), not a switch of the world:
    /// nothing flips it under a running game.
    #[doc(hidden)]
    pub fn with_cast_merge(mut self, on: bool) -> Self {
        self.merge_casts = on;
        self.hash = self.fnv();
        self
    }

    /// Whether casts merge the cells under them into column runs
    /// ([`with_cast_merge`](Self::with_cast_merge)).
    pub fn merges_casts(&self) -> bool {
        self.merge_casts
    }

    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    /// Edge of a cell (m).
    pub fn cell(&self) -> f32 {
        self.cell
    }

    /// Cells along x, y and z.
    pub fn size(&self) -> UVec3 {
        self.size
    }

    pub fn layers(&self) -> CollisionLayers {
        self.layers
    }

    /// How many cells are solid.
    pub fn solid_cells(&self) -> u64 {
        self.solid_cells
    }

    /// Bytes the bits take.
    pub fn memory(&self) -> usize {
        self.bits.len() * 8
    }

    /// FNV-1a over the size, the cell, the origin, the cast mode and every bit (not the layers):
    /// what the physics snapshot check compares.
    pub fn hash(&self) -> u64 {
        self.hash
    }

    fn fnv(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |bytes: &[u8]| {
            for b in bytes {
                h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        for v in [self.size.x, self.size.y, self.size.z, self.cell.to_bits()] {
            eat(&v.to_le_bytes());
        }
        for v in self.origin.to_array() {
            eat(&v.to_bits().to_le_bytes());
        }
        eat(&[u8::from(self.merge_casts)]);
        for word in &self.bits {
            eat(&word.to_le_bytes());
        }
        h
    }

    /// The field's box in the world: from `origin` to `origin + size * cell`.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        (self.origin, self.origin + self.size.as_vec3() * self.cell)
    }

    /// World height of the bottom of the lowest solid cell and of the top of the highest one;
    /// `None` when no cell is solid.
    pub fn solid_span(&self) -> Option<(f32, f32)> {
        let (low, high) = self.layers_used?;
        Some((self.layer_y(low as i32), self.layer_y(high as i32 + 1)))
    }

    /// World height of the bottom of layer `y`.
    fn layer_y(&self, y: i32) -> f32 {
        self.origin.y + y as f32 * self.cell
    }

    pub fn inside(&self, c: IVec3) -> bool {
        c.cmpge(IVec3::ZERO).all() && c.cmplt(self.size.as_ivec3()).all()
    }

    fn index(&self, c: IVec3) -> usize {
        ((c.x as u32 * self.size.z + c.z as u32) * self.size.y + c.y as u32) as usize
    }

    /// Whether cell `c` is solid (never outside the field).
    pub fn solid(&self, c: IVec3) -> bool {
        if !self.inside(c) {
            return false;
        }
        let i = self.index(c);
        self.bits[i / 64] & (1 << (i % 64)) != 0
    }

    /// Solid with a face toward a cell that is not (outside the field counts as not solid):
    /// the only cells a shape coming from outside can meet first.
    pub fn exposed(&self, c: IVec3) -> bool {
        self.solid(c) && [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z].iter().any(|d| !self.solid(c + *d))
    }

    /// The cell holding world point `p` (a point on a face between two cells is in the upper
    /// one); may be outside the field.
    pub fn cell_at(&self, p: Vec3) -> IVec3 {
        ((p - self.origin) / self.cell).floor().as_ivec3()
    }

    /// World position of the least corner of cell `c`.
    pub fn corner(&self, c: IVec3) -> Vec3 {
        self.origin + c.as_vec3() * self.cell
    }

    /// World centre of cell `c`.
    pub fn center(&self, c: IVec3) -> Vec3 {
        self.origin + (c.as_vec3() + Vec3::splat(0.5)) * self.cell
    }

    /// The cells (an inclusive range) whose closed boxes touch the closed world box `lo..=hi`,
    /// clipped to the field: a box that ends exactly on a face takes the cell on its other side
    /// too, as an overlap test of two boxes does. An infinite bound reaches the field's edge (a
    /// cast of unlimited reach). `None` if it misses the field or a bound is not a number.
    pub fn cells_in(&self, lo: Vec3, hi: Vec3) -> Option<(IVec3, IVec3)> {
        if lo.is_nan() || hi.is_nan() {
            return None;
        }
        let a = ((lo - self.origin) / self.cell).ceil() - Vec3::ONE;
        let b = ((hi - self.origin) / self.cell).floor();
        let top = self.size.as_vec3() - Vec3::ONE;
        if self.size.cmpeq(UVec3::ZERO).any() || b.cmplt(Vec3::ZERO).any() || a.cmpgt(top).any() || a.cmpgt(b).any() {
            return None;
        }
        Some((a.max(Vec3::ZERO).as_ivec3(), b.min(top).as_ivec3()))
    }

    /// The run of solid cells in `c`'s column that holds `c`: its first layer and the layer
    /// after its last (`y0..y1`). `None` when `c` is not solid.
    pub fn run(&self, c: IVec3) -> Option<(i32, i32)> {
        if !self.solid(c) {
            return None;
        }
        let (mut y0, mut y1) = (c.y, c.y + 1);
        while self.solid(IVec3::new(c.x, y0 - 1, c.z)) {
            y0 -= 1;
        }
        while self.solid(IVec3::new(c.x, y1, c.z)) {
            y1 += 1;
        }
        Some((y0, y1))
    }

    /// World bottom and top of the [`run`](Self::run) holding `c`.
    pub fn run_extent(&self, c: IVec3) -> Option<(f32, f32)> {
        let (y0, y1) = self.run(c)?;
        Some((self.layer_y(y0), self.layer_y(y1)))
    }

    /// Cell `c` as parry sees it: a cuboid at the cell's centre.
    pub(crate) fn cuboid(&self) -> Cuboid {
        Cuboid::new(Vec3::splat(self.cell * 0.5))
    }

    /// The first solid cell along the ray within `max` m (`dir` unit length), tested as the box
    /// it is, so the distance, point and normal are those of a cuboid collider in its place. A
    /// ray that starts inside a solid cell hits it at 0 with a zero normal (as parry says for a
    /// solid shape). Among cells the ray meets at the same distance (along an edge or a face
    /// between two cells) one it meets as a box wins over one it meets only along a seam (as a
    /// cuboid per cell would answer), then the lowest in `(x, y, z)` order. One difference from cuboid
    /// colliders side by side: a ray in the plane of a face between two solid cells is inside the
    /// solid and meets it, where parry may let it through between the cuboids
    /// (`along_seam`).
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<FieldRay> {
        if !(origin.is_finite() && dir.is_finite()) || max.is_nan() || max < 0.0 || self.solid_cells == 0 {
            return None;
        }
        let o = (origin - self.origin) / self.cell;
        // How far the walk goes (in cells): a cell entered a hair past the reach can still be
        // touched within `max` (rounding).
        let reach = max / self.cell;
        let stop = if reach.is_finite() { reach + reach.max(1.0) * 1e-5 } else { f32::INFINITY };
        let size = self.size.as_vec3();
        // Clip the ray to the field's box (in cells), `NEAR` wide: a ray that grazes the box's
        // edge or corner may touch a cell there.
        let (mut t0, mut t1, mut entry) = (0.0_f32, stop, None);
        for a in 0..3 {
            let (oa, da) = (o[a], dir[a]);
            if da == 0.0 {
                let slack = NEAR * size[a].max(1.0);
                if oa < -slack || oa > size[a] + slack {
                    return None;
                }
                continue;
            }
            let (n, f) = ((0.0 - oa) / da, (size[a] - oa) / da);
            let (near, far) = if n < f { (n, f) } else { (f, n) };
            if near > t0 {
                t0 = near;
                entry = Some(a);
            }
            t1 = t1.min(far);
            if t0 > t1 + NEAR * t1.abs().max(1.0) {
                return None;
            }
        }
        let step = IVec3::new(sign(dir.x), sign(dir.y), sign(dir.z));
        let start = o + dir * t0;
        // The cell the ray is in, and the axes along which its place lies on a face between that
        // cell and the one behind it (`behind` in `ray_at`), which it touches there too: the
        // face it starts on, a face it runs along, the faces it just crossed. The cell may be
        // just outside the field (a ray along the field's top face is in the layer above it).
        //
        // Where the ray is on a face, and which faces it crosses together (through an edge or a
        // corner), is decided with a tolerance of `NEAR`: rounding must not split a crossing
        // through an edge into two, which would skip a cell the ray grazes there. A cell taken
        // in too many is harmless (each is tested exactly, as the box it is); one left out is
        // a missed hit.
        let mut cell = IVec3::ZERO;
        let mut on_face = [false; 3];
        for a in 0..3 {
            if Some(a) == entry {
                cell[a] = if step[a] > 0 { 0 } else { self.size[a] as i32 - 1 };
                continue;
            }
            let k = start[a].round();
            on_face[a] = (start[a] - k).abs() <= NEAR * k.abs().max(1.0);
            // On a face going down an axis, the ray is in the cell below the face.
            cell[a] = match (on_face[a], step[a] < 0) {
                (true, true) => k as i32 - 1,
                (true, false) => k as i32,
                (false, _) => start[a].floor() as i32,
            };
        }
        // When the ray reaches the next face on each axis, from its origin (not summed step by
        // step, so crossings through one edge get the same number).
        let next = |a: usize, c: i32| {
            let face = if step[a] > 0 { c as f32 + 1.0 } else { c as f32 };
            (face - o[a]) / dir[a]
        };
        let mut t_max = Vec3::splat(f32::INFINITY);
        for a in 0..3 {
            if step[a] != 0 {
                t_max[a] = next(a, cell[a]);
            }
        }
        let ray = Ray::new(origin, dir);
        let mut best: Option<(FieldRay, bool)> = None;
        loop {
            if let Some(hit) = self.ray_at(&ray, best.map_or(max, |(b, _)| b.distance), cell, on_face, step)
                && ahead(hit, best)
            {
                best = Some(hit);
            }
            let t = t_max.min_element();
            // After a hit the walk goes on only to the faces at its distance: a cell reached
            // there (through an edge or a corner, by a ray going down an axis) can be met as
            // near and win the tie (`ahead`); one reached later is met farther.
            let until = best.map_or(stop, |(b, _)| {
                let at = b.distance / self.cell;
                at + NEAR * at.max(1.0)
            });
            if !(t <= until && t.is_finite()) {
                return best.map(|(b, _)| b);
            }
            let together = t + NEAR * t.abs().max(1.0);
            let mut left = false;
            for a in 0..3 {
                if step[a] == 0 {
                    continue;
                }
                on_face[a] = t_max[a] <= together;
                if on_face[a] {
                    cell[a] += step[a];
                    t_max[a] = next(a, cell[a]);
                    left |= cell[a] < 0 || cell[a] >= self.size[a] as i32;
                }
            }
            // Out of the field: the cells inside behind the faces it went out by are the last
            // it touches.
            if left {
                if let Some(hit) = self.ray_at(&ray, best.map_or(max, |(b, _)| b.distance), cell, on_face, step)
                    && ahead(hit, best)
                {
                    best = Some(hit);
                }
                return best.map(|(b, _)| b);
            }
        }
    }

    /// The nearest hit among the solid cells holding the ray's current place: `cell`, and the
    /// cells behind each face it lies on (`on_face`), tested as boxes in `(x, y, z)` order.
    /// Where a ray that lies in the plane of a face of solid cell `c` meets it, if that face is
    /// shared with another solid cell. parry's ray against a box misses a ray in the plane of
    /// two of its faces at once (a ray along the edge where four cells meet: its slab test
    /// divides zero by zero), so cuboid colliders side by side let such a ray through the
    /// floor they make; inside a field it is inside the solid. The cell's closed box on the
    /// grid (`corner(c)..corner(c + 1)`, shared bit for bit with its neighbours) is met where
    /// the ray enters it, if, along each axis the ray does not move on, it is strictly inside
    /// the box or on a face with a solid cell behind it. A ray in the plane of an outer face
    /// grazes it and meets nothing there, as parry says for a box.
    fn along_seam(&self, ray: &Ray, reach: f32, c: IVec3) -> Option<(f32, Vec3)> {
        let (lo, hi) = (self.corner(c), self.corner(c + IVec3::ONE));
        let (o, d) = (ray.origin, ray.dir);
        let (mut near, mut far, mut entry) = (0.0_f32, reach, None);
        let mut seam = false;
        for a in 0..3 {
            let unit = IVec3::AXES[a];
            if d[a] == 0.0 {
                if o[a] > lo[a] && o[a] < hi[a] {
                    continue;
                }
                let behind = if o[a] == lo[a] {
                    c - unit
                } else if o[a] == hi[a] {
                    c + unit
                } else {
                    return None;
                };
                if !self.solid(behind) {
                    return None;
                }
                seam = true;
                continue;
            }
            let (t1, t2) = ((lo[a] - o[a]) / d[a], (hi[a] - o[a]) / d[a]);
            // A ray that starts on a face and goes in enters by it at 0: that face's normal, as
            // parry gives (the first such axis; an axis it goes along, or out of, is no entry).
            if t1.min(t2) > near || (entry.is_none() && t1.min(t2) >= near) {
                near = t1.min(t2);
                entry = Some(a);
            }
            far = far.min(t1.max(t2));
        }
        if !seam || near > far {
            return None;
        }
        // The face it enters by (none when it starts inside: a zero normal, as parry gives).
        let normal = entry.map_or(Vec3::ZERO, |a| -d[a].signum() * IVec3::AXES[a].as_vec3());
        Some((near, normal))
    }

    fn ray_at(&self, ray: &Ray, max: f32, cell: IVec3, on_face: [bool; 3], step: IVec3) -> Option<(FieldRay, bool)> {
        let mut best: Option<(FieldRay, bool)> = None;
        let mut options = [[0i32; 2]; 3];
        let mut counts = [1usize; 3];
        for a in 0..3 {
            // The cell behind a face: below it for a ray going up (or along it), above it for one
            // going down.
            let behind = if step[a] < 0 { cell[a] + 1 } else { cell[a] - 1 };
            options[a] = [cell[a].min(behind), cell[a].max(behind)];
            if on_face[a] {
                counts[a] = 2;
            } else {
                options[a] = [cell[a], cell[a]];
            }
        }
        let cuboid = self.cuboid();
        for &x in &options[0][..counts[0]] {
            for &y in &options[1][..counts[1]] {
                for &z in &options[2][..counts[2]] {
                    let c = IVec3::new(x, y, z);
                    if !self.solid(c) {
                        continue;
                    }
                    // As a world of one collider per cell tests each: its box first, then the shape,
                    // within the nearest hit so far.
                    let (centre, half) = (self.center(c), cuboid.half_extents);
                    let reach = best.map_or(max, |(b, _)| b.distance);
                    let boxed = ray_hits_aabb(ray.origin, ray.dir, reach, centre - half, centre + half)
                        .then(|| cuboid.cast_ray_and_get_normal(&Pose::from_translation(centre), ray, reach, true))
                        .flatten()
                        .map(|hit| (hit.time_of_impact, hit.normal, false));
                    let hit = boxed.or_else(|| self.along_seam(ray, reach, c).map(|(distance, normal)| (distance, normal, true)));
                    let Some((distance, normal, seam)) = hit else {
                        continue;
                    };
                    let hit = (FieldRay { distance, normal, cell: c }, seam);
                    if ahead(hit, best) {
                        best = Some(hit);
                    }
                }
            }
        }
        best
    }
}

/// Whether `hit` goes before `best` (each with whether it was met only along a seam, as
/// `along_seam` says): it is nearer; or as near (the same `f32`) and met as a box where `best` was
/// met only along a seam (a cuboid per cell would have only the box's answer); or as near, met
/// alike and lower in `(x, y, z)` order (x first, as integers).
fn ahead(hit: (FieldRay, bool), best: Option<(FieldRay, bool)>) -> bool {
    let (h, h_seam) = hit;
    best.is_none_or(|(b, b_seam)| h.distance < b.distance || (h.distance == b.distance && (h_seam, h.cell.to_array()) < (b_seam, b.cell.to_array())))
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

    /// A tiny xorshift: the same cases on every machine.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u32) -> i32 {
            (self.next() % u64::from(n)) as i32
        }
        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    /// Every solid cell tested as its own cuboid, in `(x, y, z)` order, the strictly nearest
    /// winning: what a world of one collider per cell (ids in that order) answers, plus the
    /// rays along a seam between solid cells that such a world lets through.
    fn brute(f: &VoxelField, origin: Vec3, dir: Vec3, max: f32) -> Option<FieldRay> {
        cuboids(f, origin, dir, max, true)
    }

    /// [`brute`], `seams`: with the rays along seams.
    fn cuboids(f: &VoxelField, origin: Vec3, dir: Vec3, max: f32, seams: bool) -> Option<FieldRay> {
        let ray = Ray::new(origin, dir);
        let mut best: Option<(FieldRay, bool)> = None;
        for x in 0..f.size.x as i32 {
            for y in 0..f.size.y as i32 {
                for z in 0..f.size.z as i32 {
                    let c = IVec3::new(x, y, z);
                    if !f.solid(c) {
                        continue;
                    }
                    let (centre, half, reach) = (f.center(c), f.cuboid().half_extents, best.map_or(max, |(b, _)| b.distance));
                    let hit = ray_hits_aabb(origin, dir, reach, centre - half, centre + half)
                        .then(|| f.cuboid().cast_ray_and_get_normal(&Pose::from_translation(centre), &ray, reach, true))
                        .flatten()
                        .map(|hit| (hit.time_of_impact, hit.normal, false))
                        .or_else(|| seams.then(|| f.along_seam(&ray, reach, c).map(|(d, n)| (d, n, true))).flatten());
                    // Cells in `(x, y, z)` order: a box's answer wins a tie with a seam's, then
                    // the first cell (as `ahead` says).
                    if let Some((distance, normal, seam)) = hit
                        && ahead((FieldRay { distance, normal, cell: c }, seam), best)
                    {
                        best = Some((FieldRay { distance, normal, cell: c }, seam));
                    }
                }
            }
        }
        best.map(|(b, _)| b)
    }

    fn random_field(rng: &mut Rng, size: UVec3, origin: Vec3, cell: f32) -> VoxelField {
        // Columns of random heights, with cells flipped here and there (floating slabs, holes):
        // seams of every kind.
        let tops: Vec<i32> = (0..size.x * size.z).map(|_| rng.below(size.y + 1)).collect();
        let flips: Vec<bool> = (0..size.x * size.y * size.z).map(|_| rng.below(7) == 0).collect();
        VoxelField::new(origin, cell, size, |c| {
            let column = (c.x as u32 * size.z + c.z as u32) as usize;
            let at = ((c.x as u32 * size.z + c.z as u32) * size.y + c.y as u32) as usize;
            (c.y < tops[column]) != flips[at]
        })
    }

    #[test]
    fn rays_meet_what_a_cuboid_per_cell_meets() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let mut hits = 0;
        for round in 0..60 {
            let size = UVec3::new(1 + rng.below(6) as u32, 1 + rng.below(6) as u32, 1 + rng.below(6) as u32);
            let cell = [1.0, 0.5, 0.25, 2.0, 0.3, 0.7][round % 6];
            let origin = Vec3::new(rng.below(5) as f32 - 2.0, rng.below(5) as f32 - 2.0, rng.below(5) as f32 - 2.0) * cell;
            let f = random_field(&mut rng, size, origin, cell);
            let (lo, hi) = f.bounds();
            for _ in 0..400 {
                // Starts on lattice points and faces (seams) half the time, inside and around the field.
                let mut pick = |a: usize| {
                    let p = lo[a] - cell + rng.unit() * (hi[a] - lo[a] + 2.0 * cell);
                    if rng.below(2) == 0 { origin[a] + ((p - origin[a]) / cell).round() * cell } else { p }
                };
                let from = Vec3::new(pick(0), pick(1), pick(2));
                let dir = match rng.below(4) {
                    // Along an axis: rays down the seams of columns, along walls.
                    0 => [Vec3::X, Vec3::NEG_X, Vec3::Y, Vec3::NEG_Y, Vec3::Z, Vec3::NEG_Z][rng.below(6) as usize],
                    // Through edges and corners.
                    1 => Vec3::new(rng.below(3) as f32 - 1.0, rng.below(3) as f32 - 1.0, rng.below(3) as f32 - 1.0)
                        .try_normalize()
                        .unwrap_or(Vec3::NEG_Y),
                    _ => Vec3::new(rng.unit() - 0.5, rng.unit() - 0.5, rng.unit() - 0.5).try_normalize().unwrap_or(Vec3::Y),
                };
                let max = [f32::INFINITY, 3.0 * cell, 0.0, 20.0][rng.below(4) as usize];
                let (got, want) = (f.raycast(from, dir, max), brute(&f, from, dir, max));
                assert_eq!(got, want, "{f:?}: ray from {from} along {dir} within {max}");
                hits += usize::from(want.is_some());
            }
        }
        assert!(hits > 6000, "too few hits to mean anything: {hits}");
    }

    #[test]
    fn a_ray_along_a_seam_between_solid_cells_is_in_the_solid() {
        // A floor two cells deep, its top at 0, of cells 0.5 m and 0.2 m (their boxes' bounds
        // on the grid are exact and inexact floats).
        for (cell, n) in [(0.5, 8), (0.2, 20)] {
            let f = VoxelField::new(Vec3::new(-2.0, -2.0 * cell, -2.0), cell, UVec3::new(n, 4, n), |c| c.y < 2);
            let mut through = 0;
            // Straight down every line where four cells of the floor meet.
            for i in 1..n as i32 {
                for k in 1..n as i32 {
                    let from = f.corner(IVec3::new(i, 4, k));
                    let hit = f.raycast(from, Vec3::NEG_Y, 10.0).unwrap_or_else(|| panic!("the ray down at {from} meets the floor"));
                    // The cell a cuboid per cell names where one meets the ray (the first in
                    // `(x, y, z)` order of those parry says it meets), else the lowest of the four.
                    let boxed = cuboids(&f, from, Vec3::NEG_Y, 10.0, false);
                    let cell = boxed.map_or(IVec3::new(i - 1, 1, k - 1), |h| h.cell);
                    assert_eq!((hit.distance, hit.normal, hit.cell), (from.y, Vec3::Y, cell), "at {from}: the floor's top, of that cell");
                    through += usize::from(boxed.is_none());
                }
            }
            // How many of these rays cuboid colliders side by side let through is parry's to say
            // (with parry3d 0.31.1: none at 0.5 m, whose bounds are exact floats; at 0.2 m its
            // test in the box's own frame, the ray minus the centre, rounds some just outside
            // both neighbours). Printed, not held: the field's answer above is what is.
            // Along the seam between the floor's two layers, from outside: it meets the first
            // cell's side. Along the floor's top (air above): a graze, as for a box.
            let k = n as i32 / 2;
            let seam = f.corner(IVec3::new(0, 1, k)) - Vec3::X;
            assert_eq!(f.raycast(seam, Vec3::X, 10.0).map(|h| (h.distance, h.normal, h.cell)), Some((1.0, Vec3::NEG_X, IVec3::new(0, 0, k - 1))));
            let top = f.corner(IVec3::new(0, 2, k)) - Vec3::X;
            assert_eq!(f.raycast(top, Vec3::X, 10.0), brute(&f, top, Vec3::X, 10.0), "a graze along the top is what cuboids say");
            // From a seam on the floor's top face, straight down: met at once, by the face. What
            // a cuboid per cell says where one meets it (parry, on its face: the face's normal,
            // or zero if rounding put the start inside), the face's normal where all let it through.
            let on_top = f.corner(IVec3::new(1, 2, k));
            let hit = f.raycast(on_top, Vec3::NEG_Y, 10.0).unwrap();
            let want = cuboids(&f, on_top, Vec3::NEG_Y, 10.0, false).map_or(Vec3::Y, |h| h.normal);
            assert_eq!((hit.distance, hit.normal), (0.0, want), "at {on_top}: not the zero normal of a start inside");
            println!("cells of {cell} m: {through} of {} rays down a seam pass between cuboids side by side", (n - 1) * (n - 1));
        }
    }

    #[test]
    fn a_ray_starting_in_a_cell_hits_it_at_once_and_one_out_of_reach_misses() {
        let f = VoxelField::new(Vec3::ZERO, 1.0, UVec3::new(4, 4, 4), |c| c.y < 2);
        let inside = f.raycast(Vec3::new(1.5, 1.5, 1.5), Vec3::NEG_Y, 10.0).unwrap();
        assert_eq!((inside.distance, inside.normal, inside.cell), (0.0, Vec3::ZERO, IVec3::new(1, 1, 1)));
        let down = f.raycast(Vec3::new(1.5, 3.5, 1.5), Vec3::NEG_Y, 10.0).unwrap();
        assert_eq!((down.distance, down.normal, down.cell), (1.5, Vec3::Y, IVec3::new(1, 1, 1)));
        assert_eq!(f.raycast(Vec3::new(1.5, 3.5, 1.5), Vec3::NEG_Y, 1.4), None);
        assert_eq!(f.raycast(Vec3::new(1.5, 3.5, 1.5), Vec3::Y, 10.0), None);
        assert_eq!(f.raycast(Vec3::new(9.0, 1.5, 1.5), Vec3::X, 10.0), None, "going away");
        let side = f.raycast(Vec3::new(9.0, 1.5, 1.5), Vec3::NEG_X, 10.0).unwrap();
        assert_eq!((side.distance, side.normal, side.cell), (5.0, Vec3::X, IVec3::new(3, 1, 1)));
        // On the seam between two columns, both are touched: the lower x wins the tie.
        let seam = f.raycast(Vec3::new(2.0, 3.0, 1.5), Vec3::NEG_Y, 10.0).unwrap();
        assert_eq!((seam.distance, seam.cell), (1.0, IVec3::new(1, 1, 1)));
        assert_eq!(f.raycast(Vec3::new(f32::NAN, 3.0, 1.5), Vec3::NEG_Y, 10.0), None);
        assert_eq!(VoxelField::new(Vec3::ZERO, 1.0, UVec3::new(3, 3, 3), |_| false).raycast(Vec3::ONE, Vec3::X, 9.0), None);
    }

    #[test]
    fn runs_spans_and_the_cells_under_a_box() {
        // Column (0, 0): solid 0..2 and 3..5 (a tunnel at 2); column (1, 0): nothing.
        let f = VoxelField::new(Vec3::new(0.0, -2.0, 0.0), 0.5, UVec3::new(2, 6, 1), |c| c.x == 0 && c.y != 2 && c.y < 5);
        assert_eq!(f.solid_cells(), 4);
        assert_eq!(f.run(IVec3::new(0, 1, 0)), Some((0, 2)));
        assert_eq!(f.run(IVec3::new(0, 3, 0)), Some((3, 5)));
        assert_eq!(f.run(IVec3::new(0, 2, 0)), None);
        assert_eq!(f.run_extent(IVec3::new(0, 4, 0)), Some((-0.5, 0.5)));
        assert_eq!(f.solid_span(), Some((-2.0, 0.5)));
        assert_eq!(f.bounds(), (Vec3::new(0.0, -2.0, 0.0), Vec3::new(1.0, 1.0, 0.5)));
        assert!(f.exposed(IVec3::new(0, 0, 0)) && !f.solid(IVec3::new(0, 2, 0)));
        // Inclusive, as two boxes overlap: a box ending on a face takes the cell beyond.
        assert_eq!(f.cells_in(Vec3::new(0.1, -1.4, 0.1), Vec3::new(0.4, -1.0, 0.4)), Some((IVec3::new(0, 1, 0), IVec3::new(0, 2, 0))));
        assert_eq!(f.cells_in(Vec3::new(0.5, -2.0, 0.0), Vec3::new(0.5, -2.0, 0.0)), Some((IVec3::new(0, 0, 0), IVec3::new(1, 0, 0))));
        assert_eq!(f.cells_in(Vec3::new(-3.0, -9.0, -3.0), Vec3::new(9.0, 9.0, 9.0)), Some((IVec3::ZERO, IVec3::new(1, 5, 0))));
        assert_eq!(f.cells_in(Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0)), None);
        assert_eq!(f.cells_in(Vec3::splat(f32::NAN), Vec3::ONE), None);
        // A cast of unlimited reach: its box runs to the field's edge.
        assert_eq!(f.cells_in(Vec3::new(0.1, -1.4, 0.1), Vec3::new(f32::INFINITY, f32::INFINITY, 0.4)), Some((IVec3::new(0, 1, 0), IVec3::new(1, 5, 0))));
        assert_eq!(f.cells_in(Vec3::splat(f32::NEG_INFINITY), Vec3::splat(f32::NEG_INFINITY)), None);
        assert_eq!(VoxelField::new(Vec3::ZERO, 1.0, UVec3::ONE, |_| false).solid_span(), None);
    }

    #[test]
    fn a_buried_cell_is_not_exposed() {
        let f = VoxelField::new(Vec3::ZERO, 1.0, UVec3::splat(3), |_| true);
        assert!(!f.exposed(IVec3::ONE), "all six neighbours solid");
        assert!(f.exposed(IVec3::new(0, 1, 1)), "the field's edge counts as air");
    }

    #[test]
    fn the_hash_follows_the_cells_the_size_and_the_place() {
        let hash = |origin: Vec3, cell: f32, size: UVec3, hole: bool| {
            VoxelField::new(origin, cell, size, |c| !(hole && c == IVec3::ONE)).hash()
        };
        let one = hash(Vec3::ZERO, 1.0, UVec3::splat(3), false);
        assert_eq!(one, hash(Vec3::ZERO, 1.0, UVec3::splat(3), false));
        let others = [
            hash(Vec3::ZERO, 1.0, UVec3::splat(3), true),
            hash(Vec3::X, 1.0, UVec3::splat(3), false),
            hash(Vec3::ZERO, 0.5, UVec3::splat(3), false),
            hash(Vec3::ZERO, 1.0, UVec3::new(3, 3, 4), false),
            VoxelField::new(Vec3::ZERO, 1.0, UVec3::splat(3), |_| true).with_cast_merge(false).hash(),
        ];
        assert!(others.iter().all(|h| *h != one), "{one:x} {others:x?}");
        let f = VoxelField::new(Vec3::ZERO, 1.0, UVec3::splat(3), |_| true);
        let other = CollisionLayers::new(super::super::LayerMask::WORLD, super::super::LayerMask::NONE);
        assert_eq!(f.clone().with_layers(other).hash(), f.hash(), "the snapshot check hashes the layers beside it");
    }
}
