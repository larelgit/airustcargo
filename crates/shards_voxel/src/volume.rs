//! [`VoxelVolume`]: a box of blocks, stored in chunks.
//!
//! A cell `(x, y, z)` (x east, y up, z south, from 0) holds a block id: an index into the
//! [`VoxelPalette`](crate::VoxelPalette), `0` being air. Reads outside the volume are air and
//! writes outside it are ignored, so generation steps never have to clip.
//!
//! Storage is chunked (`chunk` cells a side, per axis): a chunk that holds one block only (all
//! air above the hills, all stone below them) is kept as that one byte; the others hold a byte
//! per cell, y fastest. Chunks are the unit of meshing, of hashing and (later) of streaming;
//! physics never sees them.
//!
//! Next to the cells the volume keeps one height per column, the **surface**: the height the
//! generator gave the column (raise and lower edits move it, carving does not). It is the
//! "natural ground" generation rules ask about (where water stands, where a stair starts), as
//! against the real top solid block, which a carved tunnel or a built tower changes.

use bevy::prelude::*;

/// The block id of air.
pub const AIR: u8 = 0;

/// One chunk: all one block, or a byte per cell.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Chunk {
    Uniform(u8),
    Dense(Box<[u8]>),
}

/// A box of blocks `size` cells big, each `block` m on a side, its cell `(0, 0, 0)` with its
/// least corner at `origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelVolume {
    size: UVec3,
    block: f32,
    origin: Vec3,
    chunk: UVec3,
    /// How many chunks along each axis.
    chunks_n: UVec3,
    /// Chunk `(cx, cy, cz)` at `(cz * n.y + cy) * n.x + cx`.
    chunks: Vec<Chunk>,
    /// Natural ground per column, `x * size.z + z`: the layer above the top block the generator
    /// laid (0: nothing).
    surface: Vec<u16>,
}

impl VoxelVolume {
    /// An empty (all air) volume. `size` must be a whole number of `chunk`s on every axis.
    pub fn new(size: UVec3, block: f32, origin: Vec3, chunk: UVec3) -> Result<Self, String> {
        if size.cmpeq(UVec3::ZERO).any() || chunk.cmpeq(UVec3::ZERO).any() {
            return Err(format!("size {size} and chunk {chunk} must have no zero side"));
        }
        if (size % chunk).cmpne(UVec3::ZERO).any() {
            return Err(format!("size {size} is not a whole number of {chunk} chunks"));
        }
        if size.y > u16::MAX as u32 {
            return Err(format!("a volume is at most {} cells tall", u16::MAX));
        }
        if !(block.is_finite() && block > 0.0) {
            return Err(format!("block size {block} must be > 0"));
        }
        let chunks_n = size / chunk;
        let count = (chunks_n.x * chunks_n.y * chunks_n.z) as usize;
        Ok(Self {
            size,
            block,
            origin,
            chunk,
            chunks_n,
            chunks: vec![Chunk::Uniform(AIR); count],
            surface: vec![0; (size.x * size.z) as usize],
        })
    }

    pub fn size(&self) -> UVec3 {
        self.size
    }

    pub fn block(&self) -> f32 {
        self.block
    }

    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    pub fn chunk(&self) -> UVec3 {
        self.chunk
    }

    pub fn chunk_count(&self) -> UVec3 {
        self.chunks_n
    }

    pub fn inside(&self, p: IVec3) -> bool {
        p.cmpge(IVec3::ZERO).all() && p.cmplt(self.size.as_ivec3()).all()
    }

    /// The cell holding world point `p` (it may be outside the volume).
    pub fn cell_at(&self, p: Vec3) -> IVec3 {
        ((p - self.origin) / self.block).floor().as_ivec3()
    }

    /// World position of the least corner of cell `c`.
    pub fn corner(&self, c: IVec3) -> Vec3 {
        self.origin + c.as_vec3() * self.block
    }

    fn locate(&self, p: IVec3) -> (usize, usize) {
        let p = p.as_uvec3();
        let c = p / self.chunk;
        let l = p % self.chunk;
        let chunk = ((c.z * self.chunks_n.y + c.y) * self.chunks_n.x + c.x) as usize;
        let cell = ((l.x * self.chunk.z + l.z) * self.chunk.y + l.y) as usize;
        (chunk, cell)
    }

    /// The block at `p`; air outside.
    pub fn get(&self, p: IVec3) -> u8 {
        if !self.inside(p) {
            return AIR;
        }
        let (chunk, cell) = self.locate(p);
        match &self.chunks[chunk] {
            Chunk::Uniform(id) => *id,
            Chunk::Dense(cells) => cells[cell],
        }
    }

    /// Puts `id` at `p`; `false` (nothing written) outside.
    pub fn set(&mut self, p: IVec3, id: u8) -> bool {
        if !self.inside(p) {
            return false;
        }
        let (chunk, cell) = self.locate(p);
        let per = (self.chunk.x * self.chunk.y * self.chunk.z) as usize;
        let slot = &mut self.chunks[chunk];
        match slot {
            Chunk::Uniform(old) if *old == id => {}
            Chunk::Uniform(old) => {
                let mut cells = vec![*old; per].into_boxed_slice();
                cells[cell] = id;
                *slot = Chunk::Dense(cells);
            }
            Chunk::Dense(cells) => cells[cell] = id,
        }
        true
    }

    /// Fills the cells `min..max` (max exclusive), clipped to the volume.
    pub fn fill(&mut self, min: IVec3, max: IVec3, id: u8) {
        let lo = min.max(IVec3::ZERO);
        let hi = max.min(self.size.as_ivec3());
        for x in lo.x..hi.x {
            for z in lo.z..hi.z {
                for y in lo.y..hi.y {
                    self.set(IVec3::new(x, y, z), id);
                }
            }
        }
    }

    /// Chunks that turned out all one block go back to one byte.
    pub fn compact(&mut self) {
        for chunk in &mut self.chunks {
            if let Chunk::Dense(cells) = chunk
                && let Some(&first) = cells.first()
                && cells.iter().all(|c| *c == first)
            {
                *chunk = Chunk::Uniform(first);
            }
        }
    }

    /// How many chunks keep a byte per cell, of all.
    pub fn dense_chunks(&self) -> (usize, usize) {
        (self.chunks.iter().filter(|c| matches!(c, Chunk::Dense(_))).count(), self.chunks.len())
    }

    /// Bytes the cells take (chunk headers included).
    pub fn memory(&self) -> usize {
        let dense: usize = self.chunks.iter().map(|c| if let Chunk::Dense(cells) = c { cells.len() } else { 0 }).sum();
        dense + self.chunks.len() * std::mem::size_of::<Chunk>() + self.surface.len() * 2
    }

    // --- columns --------------------------------------------------------------------------

    fn column(&self, x: i32, z: i32) -> Option<usize> {
        (x >= 0 && z >= 0 && (x as u32) < self.size.x && (z as u32) < self.size.z).then(|| (x as u32 * self.size.z + z as u32) as usize)
    }

    /// The natural ground of column `(x, z)`: the layer above the top block the generator laid
    /// there (raise and lower edits move it, other edits do not); 0 outside.
    pub fn surface(&self, x: i32, z: i32) -> u32 {
        self.column(x, z).map_or(0, |i| self.surface[i] as u32)
    }

    pub fn set_surface(&mut self, x: i32, z: i32, h: u32) {
        if let Some(i) = self.column(x, z) {
            self.surface[i] = h.min(self.size.y) as u16;
        }
    }

    /// The layer above the highest cell of column `(x, z)` that `solid` says yes to (0: none).
    pub fn top(&self, x: i32, z: i32, solid: impl Fn(u8) -> bool) -> u32 {
        if self.column(x, z).is_none() {
            return 0;
        }
        (0..self.size.y as i32).rev().find(|&y| solid(self.get(IVec3::new(x, y, z)))).map_or(0, |y| y as u32 + 1)
    }

    // --- edits (generation steps) -------------------------------------------------------------

    /// Raises column `(x, z)` to `h` (clamped to `1..=size.y`): cells from its surface up to
    /// `h - 1` become `fill`, the top one `top`. A column already as high stays.
    pub fn raise_column(&mut self, x: i32, z: i32, h: u32, fill: u8, top: u8) {
        let target = h.clamp(1, self.size.y);
        let surface = self.surface(x, z);
        if self.column(x, z).is_none() || surface >= target {
            return;
        }
        for y in surface..target {
            let id = if y + 1 == target { top } else { fill };
            self.set(IVec3::new(x, y as i32, z), id);
        }
        self.set_surface(x, z, target);
    }

    /// Lowers column `(x, z)` to `h` (clamped to `1..=size.y`): cells from `h` up to its surface
    /// become air, the one below `h` becomes `top`. A column already as low stays.
    pub fn lower_column(&mut self, x: i32, z: i32, h: u32, top: u8) {
        let target = h.clamp(1, self.size.y);
        let surface = self.surface(x, z);
        if self.column(x, z).is_none() || surface <= target {
            return;
        }
        for y in target..surface {
            self.set(IVec3::new(x, y as i32, z), AIR);
        }
        self.set(IVec3::new(x, target as i32 - 1, z), top);
        self.set_surface(x, z, target);
    }

    /// Every cell whose centre is within `radius` cells of `center` (a cell position) becomes
    /// `id` (air carves). Surfaces stay.
    pub fn sphere(&mut self, center: Vec3, radius: f32, id: u8) {
        let lo = (center - Vec3::splat(radius)).floor().as_ivec3();
        let hi = (center + Vec3::splat(radius)).ceil().as_ivec3();
        let r2 = radius * radius;
        for x in lo.x..=hi.x {
            for z in lo.z..=hi.z {
                for y in lo.y..=hi.y {
                    let c = IVec3::new(x, y, z);
                    if (c.as_vec3() + Vec3::splat(0.5) - center).length_squared() <= r2 {
                        self.set(c, id);
                    }
                }
            }
        }
    }

    /// A tube of `radius` from `a` to `b` (cell positions): [`sphere`](Self::sphere)s every half cell.
    pub fn tube(&mut self, a: Vec3, b: Vec3, radius: f32, id: u8) {
        let steps = ((b - a).length() * 2.0).ceil().max(1.0) as u32;
        for i in 0..=steps {
            self.sphere(a.lerp(b, i as f32 / steps as f32), radius, id);
        }
    }

    /// Flattens the disc of `radius` around column `center` to height `h`: higher columns are
    /// lowered, lower ones raised (`fill` under a `top`).
    pub fn flatten(&mut self, center: Vec2, radius: f32, h: u32, fill: u8, top: u8) {
        let lo = (center - Vec2::splat(radius)).floor().as_ivec2();
        let hi = (center + Vec2::splat(radius)).ceil().as_ivec2();
        for x in lo.x..=hi.x {
            for z in lo.y..=hi.y {
                if (Vec2::new(x as f32 + 0.5, z as f32 + 0.5) - center).length() <= radius {
                    self.lower_column(x, z, h, top);
                    self.raise_column(x, z, h, fill, top);
                }
            }
        }
    }

    // --- hashing ------------------------------------------------------------------------------

    /// FNV-1a over the size, block size, origin, every cell (chunk by chunk) and every
    /// surface: two volumes with the same hash hold the same world.
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::default();
        for v in [self.size.x, self.size.y, self.size.z, self.chunk.x, self.chunk.y, self.chunk.z] {
            h.bytes(&v.to_le_bytes());
        }
        for v in [self.block, self.origin.x, self.origin.y, self.origin.z] {
            h.bytes(&v.to_bits().to_le_bytes());
        }
        let per = (self.chunk.x * self.chunk.y * self.chunk.z) as usize;
        for chunk in &self.chunks {
            match chunk {
                // A uniform chunk hashes as its dense form would, so compacting changes nothing.
                Chunk::Uniform(id) => h.repeat(*id, per),
                Chunk::Dense(cells) => h.bytes(cells),
            }
        }
        for s in &self.surface {
            h.bytes(&s.to_le_bytes());
        }
        h.0
    }
}

/// FNV-1a, 64 bits.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn repeat(&mut self, b: u8, n: usize) {
        for _ in 0..n {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> VoxelVolume {
        VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::new(8, 8, 8)).unwrap()
    }

    #[test]
    fn outside_reads_air_and_ignores_writes() {
        let mut v = small();
        assert!(!v.set(IVec3::new(-1, 0, 0), 3));
        assert!(!v.set(IVec3::new(0, 8, 0), 3));
        assert_eq!(v.get(IVec3::new(16, 0, 0)), AIR);
        assert!(v.set(IVec3::new(15, 7, 15), 3));
        assert_eq!(v.get(IVec3::new(15, 7, 15)), 3);
        assert_eq!(v.dense_chunks(), (1, 4));
    }

    #[test]
    fn size_must_be_whole_chunks() {
        assert!(VoxelVolume::new(UVec3::new(10, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).is_err());
        assert!(VoxelVolume::new(UVec3::new(16, 8, 16), 0.0, Vec3::ZERO, UVec3::splat(8)).is_err());
    }

    #[test]
    fn raise_and_lower_move_the_surface_carving_does_not() {
        let mut v = small();
        v.raise_column(3, 4, 5, 2, 1);
        assert_eq!((v.surface(3, 4), v.get(IVec3::new(3, 4, 4)), v.get(IVec3::new(3, 0, 4))), (5, 1, 2));
        v.lower_column(3, 4, 2, 1);
        assert_eq!((v.surface(3, 4), v.get(IVec3::new(3, 1, 4)), v.get(IVec3::new(3, 2, 4))), (2, 1, AIR));
        v.raise_column(5, 5, 6, 2, 1);
        v.sphere(Vec3::new(5.5, 3.5, 5.5), 1.0, AIR);
        assert_eq!(v.surface(5, 5), 6, "carving leaves the natural ground where it was");
        assert_eq!(v.get(IVec3::new(5, 3, 5)), AIR);
        assert_eq!(v.top(5, 5, |id| id != AIR), 6);
    }

    #[test]
    fn compacting_keeps_the_hash() {
        let mut v = small();
        v.fill(IVec3::ZERO, IVec3::new(16, 8, 16), 3);
        v.set(IVec3::new(1, 1, 1), 4);
        v.set(IVec3::new(1, 1, 1), 3);
        let before = v.hash();
        v.compact();
        assert_eq!(v.dense_chunks().0, 0);
        assert_eq!(v.hash(), before);
        v.set(IVec3::new(9, 2, 9), AIR);
        assert_ne!(v.hash(), before);
    }
}
