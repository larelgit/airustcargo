//! [`VoxelVolume`]: a box of blocks, stored in chunks.
//!
//! A cell `(x, y, z)` (x east, y up, z south, from 0) holds a block id: an index into the
//! [`VoxelPalette`](crate::VoxelPalette), `0` being air. Reads outside the volume are air and
//! writes outside it are ignored, so generation steps never have to clip.
//!
//! Storage is chunked (`chunk` cells a side, per axis; the size of a chunk is a parameter of the
//! volume, not a constant): a chunk that holds one block only (all air above the hills, all stone
//! below them) is kept as that one byte; the others hold a byte per cell, y fastest. Chunks are
//! the unit of meshing, of hashing and (later) of streaming; physics never sees them.
//!
//! Next to the cells the volume keeps one height per column, the **surface**: the height the
//! generator gave the column (raise and lower edits move it, carving does not). It is the
//! "natural ground" generation rules ask about (where water stands, where a stair starts), as
//! against the real top solid block ([`column_top`](VoxelVolume::column_top)), which a carved
//! tunnel or a built tower changes.
//!
//! **Edits are for generation.** [`set`](VoxelVolume::set), [`fill`](VoxelVolume::fill),
//! [`clear`](VoxelVolume::clear), [`carve`](VoxelVolume::carve), [`sphere`](VoxelVolume::sphere),
//! [`tube`](VoxelVolume::tube), [`raise_column`](VoxelVolume::raise_column),
//! [`lower_column`](VoxelVolume::lower_column) and [`flatten`](VoxelVolume::flatten) are
//! called by generation steps (through [`VoxelGen`](crate::VoxelGen)); once a region is built the
//! volume is read-only (there is no runtime destruction).

use bevy::prelude::*;

/// The block id of air.
pub const AIR: u8 = 0;

/// The most cells a volume holds (a byte each): 128 Mi, far above any region the engine draws.
pub const MAX_CELLS: u64 = 1 << 27;

/// The most chunks a volume holds: the chunk table is allocated whole, so a tiny chunk over a
/// big volume must be refused before anything is built.
pub const MAX_CHUNKS: u64 = 1 << 16;

/// One chunk: all one block, or a byte per cell.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Chunk {
    Uniform(u8),
    Dense(Box<[u8]>),
}

/// A box of blocks `size` cells big, each `block` m on a side, its cell `(0, 0, 0)` with its
/// least corner at `origin`. Two volumes are equal when they hold the same world (a
/// [`compact`](Self::compact)ed copy equals the original).
#[derive(Clone, Debug)]
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

impl PartialEq for VoxelVolume {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size
            && self.chunk == other.chunk
            && self.block == other.block
            && self.origin == other.origin
            && self.surface == other.surface
            && self.chunks.iter().zip(&other.chunks).all(|pair| match pair {
                (Chunk::Uniform(a), Chunk::Uniform(b)) => a == b,
                (Chunk::Dense(a), Chunk::Dense(b)) => a == b,
                (Chunk::Uniform(u), Chunk::Dense(d)) | (Chunk::Dense(d), Chunk::Uniform(u)) => d.iter().all(|c| c == u),
            })
    }
}

impl VoxelVolume {
    /// Checks a shape without allocating anything: what [`new`](Self::new) and the generation
    /// spec's validation share, so checking content never builds a volume.
    pub fn check_shape(size: UVec3, block: f32, origin: Vec3, chunk: UVec3) -> Result<(), String> {
        if size.cmpeq(UVec3::ZERO).any() {
            return Err(format!("size {size} must have no zero side: give every side at least one chunk"));
        }
        if chunk.cmpeq(UVec3::ZERO).any() {
            return Err(format!("chunk {chunk} must have no zero side"));
        }
        if (size % chunk).cmpne(UVec3::ZERO).any() {
            let up = (size + chunk - UVec3::ONE) / chunk * chunk;
            return Err(format!("size {size} is not a whole number of {chunk} chunks: try {up}, or change the chunk size"));
        }
        if size.y > u16::MAX as u32 {
            return Err(format!("a volume is at most {} cells tall, not {}", u16::MAX, size.y));
        }
        let cells = size.x as u64 * size.y as u64 * size.z as u64;
        if cells > MAX_CELLS {
            return Err(format!("size {size} is {cells} cells: a volume holds at most {MAX_CELLS}"));
        }
        let n = size / chunk;
        let count = n.x as u64 * n.y as u64 * n.z as u64;
        if count > MAX_CHUNKS {
            return Err(format!("size {size} in {chunk} chunks is {count} chunks: a volume holds at most {MAX_CHUNKS}; use a bigger chunk"));
        }
        if !(block.is_finite() && block > 0.0) {
            return Err(format!("block size {block} must be above 0 and finite"));
        }
        if !origin.is_finite() {
            return Err(format!("origin {origin} must be finite"));
        }
        Ok(())
    }

    /// An empty (all air) volume. `size` must be a whole number of `chunk`s on every axis, no
    /// side zero, at most [`u16::MAX`] cells tall, [`MAX_CELLS`] cells and [`MAX_CHUNKS`] chunks
    /// in all; `block` (the edge of a cell, m) above 0 and finite; `origin` finite. See
    /// [`check_shape`](Self::check_shape).
    pub fn new(size: UVec3, block: f32, origin: Vec3, chunk: UVec3) -> Result<Self, String> {
        Self::check_shape(size, block, origin, chunk)?;
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

    /// The origin that puts the lattice point `anchor` (the least corner of cell `anchor`) at
    /// the world origin: how a region says "this cell's top is world y = 0".
    pub fn origin_for_anchor(anchor: UVec3, block: f32) -> Vec3 {
        -(anchor.as_vec3() * block)
    }

    /// Cells along each axis.
    pub fn size(&self) -> UVec3 {
        self.size
    }

    /// The edge of a cell, m.
    pub fn block(&self) -> f32 {
        self.block
    }

    /// World position of the least corner of cell `(0, 0, 0)`.
    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    /// Cells per chunk along each axis.
    pub fn chunk(&self) -> UVec3 {
        self.chunk
    }

    /// Chunks along each axis.
    pub fn chunk_count(&self) -> UVec3 {
        self.chunks_n
    }

    /// The block every cell of chunk `c` holds, if the chunk is stored as one byte (so a mesher
    /// can skip an empty or solid chunk without reading its cells); `None` for a chunk stored
    /// per cell or outside the volume. A chunk that is uniform but stored per cell reads `None`
    /// until [`compact`](Self::compact).
    pub fn chunk_uniform(&self, c: IVec3) -> Option<u8> {
        match self.chunks.get(self.chunk_index(c)?)? {
            Chunk::Uniform(id) => Some(*id),
            Chunk::Dense(_) => None,
        }
    }

    /// The cells of chunk `c` when it is stored per cell, in storage order (x, then z, then y
    /// fastest, local to the chunk); `None` for a uniform chunk or outside the volume.
    pub fn chunk_cells(&self, c: IVec3) -> Option<&[u8]> {
        match self.chunks.get(self.chunk_index(c)?)? {
            Chunk::Uniform(_) => None,
            Chunk::Dense(cells) => Some(cells),
        }
    }

    fn chunk_index(&self, c: IVec3) -> Option<usize> {
        (c.cmpge(IVec3::ZERO).all() && c.cmplt(self.chunks_n.as_ivec3()).all())
            .then(|| ((c.z as u32 * self.chunks_n.y + c.y as u32) * self.chunks_n.x + c.x as u32) as usize)
    }

    /// Whether cell `p` is in the volume.
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

    /// Chunks that turned out all one block go back to one byte. The hash does not change.
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

    /// Bytes the cells and surfaces take (chunk headers included).
    pub fn memory(&self) -> usize {
        let dense: usize = self.chunks.iter().map(|c| if let Chunk::Dense(cells) = c { cells.len() } else { 0 }).sum();
        dense + self.chunks.len() * std::mem::size_of::<Chunk>() + self.surface.len() * 2
    }

    /// How many cells `keep` says yes to.
    pub fn count(&self, keep: impl Fn(u8) -> bool) -> usize {
        let per = (self.chunk.x * self.chunk.y * self.chunk.z) as usize;
        self.chunks
            .iter()
            .map(|chunk| match chunk {
                Chunk::Uniform(id) => if keep(*id) { per } else { 0 },
                Chunk::Dense(cells) => cells.iter().filter(|c| keep(**c)).count(),
            })
            .sum()
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

    /// Sets the natural ground of column `(x, z)` (clamped to the volume's height) without
    /// touching a cell. Ignored outside.
    pub fn set_surface(&mut self, x: i32, z: i32, h: u32) {
        if let Some(i) = self.column(x, z) {
            self.surface[i] = h.min(self.size.y) as u16;
        }
    }

    /// The layer above the highest cell of column `(x, z)` that `filter` says yes to (0: none,
    /// and outside). This is the real top, as against the [`surface`](Self::surface).
    pub fn column_top(&self, x: i32, z: i32, filter: impl Fn(u8) -> bool) -> u32 {
        if self.column(x, z).is_none() {
            return 0;
        }
        (0..self.size.y as i32).rev().find(|&y| filter(self.get(IVec3::new(x, y, z)))).map_or(0, |y| y as u32 + 1)
    }

    /// The runs of cells `filter` says yes to in column `(x, z)`, bottom to top, as `[y0, y1)`
    /// pairs (a cave under a hill is two runs). Empty outside.
    pub fn column_runs(&self, x: i32, z: i32, filter: impl Fn(u8) -> bool) -> Vec<(u32, u32)> {
        let mut runs = Vec::new();
        if self.column(x, z).is_none() {
            return runs;
        }
        let mut start: Option<u32> = None;
        for y in 0..self.size.y {
            match (filter(self.get(IVec3::new(x, y as i32, z))), start) {
                (true, None) => start = Some(y),
                (false, Some(y0)) => {
                    runs.push((y0, y));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(y0) = start {
            runs.push((y0, self.size.y));
        }
        runs
    }

    // --- edits (generation steps) -------------------------------------------------------------

    /// Fills the cells `min..max` (max exclusive), clipped to the volume, with `id`. Surfaces
    /// stay.
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

    /// Carves the cells `min..max` (max exclusive), clipped to the volume, to air. Surfaces
    /// stay: a tunnel does not move the natural ground over it.
    pub fn carve(&mut self, min: IVec3, max: IVec3) {
        self.fill(min, max, AIR);
    }

    /// Turns the cells of `min..max` (max exclusive, clipped) that `filter` says yes to into
    /// air: "clear the trees near the arena" is `clear(min, max, |id| tree_ids.contains(&id))`.
    /// Surfaces stay.
    pub fn clear(&mut self, min: IVec3, max: IVec3, filter: impl Fn(u8) -> bool) {
        let lo = min.max(IVec3::ZERO);
        let hi = max.min(self.size.as_ivec3());
        for x in lo.x..hi.x {
            for z in lo.z..hi.z {
                for y in lo.y..hi.y {
                    let p = IVec3::new(x, y, z);
                    let id = self.get(p);
                    if id != AIR && filter(id) {
                        self.set(p, AIR);
                    }
                }
            }
        }
    }

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

    /// Every cell whose centre is within `radius` cells of `center` (a position in cell units)
    /// becomes `id` (air carves). Surfaces stay.
    pub fn sphere(&mut self, center: Vec3, radius: f32, id: u8) {
        // Clipped to the volume: a huge radius costs the volume, not its bounding box.
        let lo = (center - Vec3::splat(radius)).floor().as_ivec3().max(IVec3::ZERO);
        let hi = (center + Vec3::splat(radius)).ceil().as_ivec3().min(self.size.as_ivec3() - IVec3::ONE);
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

    /// A tube of `radius` from `a` to `b` (positions in cell units): [`sphere`](Self::sphere)s
    /// every half cell along it.
    pub fn tube(&mut self, a: Vec3, b: Vec3, radius: f32, id: u8) {
        let len = (b - a).length();
        if !(len.is_finite() && radius.is_finite()) {
            return;
        }
        let steps = (len * 2.0).ceil().max(1.0);
        // Only the spheres that can touch the volume: the segment is clipped to the box grown by
        // the radius (the slab method), and the positions stay those of the unclipped tube.
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        let (min, max) = (Vec3::splat(-radius - 1.0), self.size.as_vec3() + Vec3::splat(radius + 1.0));
        let d = b - a;
        for k in 0..3 {
            if d[k] == 0.0 {
                if a[k] < min[k] || a[k] > max[k] {
                    return;
                }
            } else {
                let (u, v) = ((min[k] - a[k]) / d[k], (max[k] - a[k]) / d[k]);
                t0 = t0.max(u.min(v));
                t1 = t1.min(u.max(v));
            }
        }
        if t0 > t1 {
            return;
        }
        let first = (t0 * steps).floor().max(0.0) as u64;
        let last = (t1 * steps).ceil().min(steps) as u64;
        for i in first..=last {
            self.sphere(a.lerp(b, i as f32 / steps), radius, id);
        }
    }

    /// Flattens the disc of `radius` (cells) around the column position `center` to height `h`:
    /// higher columns are lowered, lower ones raised (`fill` under a `top`).
    pub fn flatten(&mut self, center: Vec2, radius: f32, h: u32, fill: u8, top: u8) {
        let lo = (center - Vec2::splat(radius)).floor().as_ivec2().max(IVec2::ZERO);
        let hi = (center + Vec2::splat(radius)).ceil().as_ivec2().min(IVec2::new(self.size.x as i32, self.size.z as i32) - IVec2::ONE);
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

    /// FNV-1a over a canonical form: the size, the chunk size, the block size and origin (as
    /// `f32` bits), every cell chunk by chunk (chunk index order, each chunk's cells in storage
    /// order; a uniform chunk hashes as its dense form would, so [`compact`](Self::compact)
    /// changes nothing) and every surface. Two volumes with the same hash hold the same world.
    /// No `HashMap`, no pointer and no clock goes in: it is the same on every run and every
    /// machine of one architecture.
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::default();
        for v in [self.size.x, self.size.y, self.size.z, self.chunk.x, self.chunk.y, self.chunk.z] {
            h.bytes(&v.to_le_bytes());
        }
        for v in [self.block, self.origin.x, self.origin.y, self.origin.z] {
            // `+ 0.0` turns -0.0 into +0.0: an anchor at 0 and a zero origin are the same volume.
            h.bytes(&(v + 0.0).to_bits().to_le_bytes());
        }
        let per = (self.chunk.x * self.chunk.y * self.chunk.z) as usize;
        for chunk in &self.chunks {
            match chunk {
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
pub(crate) struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }

    pub fn repeat(&mut self, b: u8, n: usize) {
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
        v.fill(IVec3::new(-5, -5, -5), IVec3::new(2, 2, 2), 4);
        assert_eq!(v.count(|id| id == 4), 8, "a fill is clipped to the volume");
        v.raise_column(-1, 0, 4, 2, 1);
        v.lower_column(99, 0, 1, 1);
        assert_eq!((v.surface(-1, 0), v.column_top(99, 0, |id| id != AIR)), (0, 0));
    }

    #[test]
    fn size_chunk_and_block_are_validated() {
        let ok = |size: UVec3, block: f32, chunk: UVec3| VoxelVolume::new(size, block, Vec3::ZERO, chunk);
        assert!(ok(UVec3::new(64, 32, 64), 1.0, UVec3::new(64, 32, 64)).is_ok(), "one chunk is a volume");
        assert!(ok(UVec3::new(256, 32, 256), 0.5, UVec3::new(64, 32, 64)).is_ok());
        let err = ok(UVec3::new(10, 8, 16), 1.0, UVec3::splat(8)).unwrap_err();
        assert!(err.contains("not a whole number of"), "{err}");
        assert!(ok(UVec3::new(16, 8, 16), 1.0, UVec3::new(8, 3, 8)).is_err(), "8 is not whole chunks of 3");
        assert!(ok(UVec3::new(16, 0, 16), 1.0, UVec3::splat(8)).unwrap_err().contains("zero side"));
        assert!(ok(UVec3::new(16, 8, 16), 1.0, UVec3::new(8, 0, 8)).unwrap_err().contains("chunk"));
        for block in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(ok(UVec3::new(16, 8, 16), block, UVec3::splat(8)).unwrap_err().contains("block size"), "{block}");
        }
        assert!(VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::new(f32::NAN, 0.0, 0.0), UVec3::splat(8)).is_err());
        let tall = ok(UVec3::new(8, 65_536, 8), 1.0, UVec3::new(8, 65_536, 8)).unwrap_err();
        assert!(tall.contains("at most 65535 cells tall"), "{tall}");
        let big = ok(UVec3::new(8192, 64, 8192), 1.0, UVec3::new(64, 64, 64)).unwrap_err();
        assert!(big.contains("at most"), "{big}");
    }

    #[test]
    fn a_chunk_of_any_shape_stores_and_reads_back() {
        for chunk in [UVec3::new(4, 8, 4), UVec3::new(8, 4, 16), UVec3::new(16, 8, 16)] {
            let mut v = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, chunk).unwrap();
            for x in 0..16 {
                for y in 0..8 {
                    for z in 0..16 {
                        v.set(IVec3::new(x, y, z), ((x * 5 + y * 3 + z * 7) % 11) as u8);
                    }
                }
            }
            for x in 0..16 {
                for y in 0..8 {
                    for z in 0..16 {
                        assert_eq!(v.get(IVec3::new(x, y, z)), ((x * 5 + y * 3 + z * 7) % 11) as u8, "{chunk}");
                    }
                }
            }
        }
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
        assert_eq!(v.column_top(5, 5, |id| id != AIR), 6);
        // A column already as high (low) stays; the targets clamp to 1..=size.y.
        v.raise_column(5, 5, 3, 2, 1);
        v.lower_column(3, 4, 9, 1);
        assert_eq!((v.surface(5, 5), v.surface(3, 4)), (6, 2));
        v.raise_column(7, 7, 99, 2, 1);
        assert_eq!(v.surface(7, 7), 8);
        v.lower_column(7, 7, 0, 1);
        assert_eq!((v.surface(7, 7), v.get(IVec3::new(7, 0, 7)), v.get(IVec3::new(7, 1, 7))), (1, 1, AIR));
    }

    #[test]
    fn column_runs_and_tops_see_a_cave() {
        let mut v = small();
        v.raise_column(2, 2, 6, 2, 1);
        v.carve(IVec3::new(2, 2, 2), IVec3::new(3, 4, 3));
        assert_eq!(v.column_runs(2, 2, |id| id != AIR), vec![(0, 2), (4, 6)]);
        assert_eq!(v.column_top(2, 2, |id| id != AIR), 6);
        assert_eq!(v.column_top(2, 2, |id| id == 2), 5, "the filter picks the block kind: fill 2 is up to y = 4, the top block is 1");
        assert_eq!(v.column_top(9, 9, |id| id != AIR), 0);
        assert!(v.column_runs(-1, 0, |id| id != AIR).is_empty() && v.column_runs(0, 0, |id| id != AIR).is_empty());
        v.fill(IVec3::new(4, 0, 4), IVec3::new(5, 8, 5), 3);
        assert_eq!(v.column_runs(4, 4, |id| id == 3), vec![(0, 8)], "a run may reach the ceiling");
        assert_eq!(v.surface(2, 2), 6, "carving leaves the surface");
    }

    #[test]
    fn clear_removes_only_what_the_filter_names() {
        let mut v = small();
        v.fill(IVec3::ZERO, IVec3::new(4, 2, 4), 3);
        v.fill(IVec3::new(0, 2, 0), IVec3::new(4, 4, 4), 5);
        v.clear(IVec3::new(1, 0, 1), IVec3::new(3, 4, 3), |id| id == 5);
        assert_eq!(v.get(IVec3::new(1, 2, 1)), AIR);
        assert_eq!(v.get(IVec3::new(1, 1, 1)), 3, "stone is not in the filter");
        assert_eq!(v.get(IVec3::new(0, 2, 0)), 5, "outside the box");
        assert_eq!(v.get(IVec3::new(3, 3, 3)), 5, "max is exclusive");
    }

    #[test]
    fn flatten_makes_a_disc_level_from_both_sides() {
        let mut v = small();
        for x in 0..16 {
            for z in 0..16 {
                v.raise_column(x, z, 1 + ((x * 3 + z) % 6) as u32, 2, 1);
            }
        }
        v.flatten(Vec2::new(8.0, 8.0), 4.0, 4, 2, 1);
        for x in 0..16 {
            for z in 0..16 {
                let inside = (Vec2::new(x as f32 + 0.5, z as f32 + 0.5) - Vec2::new(8.0, 8.0)).length() <= 4.0;
                if inside {
                    assert_eq!((v.surface(x, z), v.column_top(x, z, |id| id != AIR)), (4, 4), "({x}, {z})");
                    assert_eq!(v.get(IVec3::new(x, 3, z)), 1);
                } else {
                    assert_eq!(v.surface(x, z), 1 + ((x * 3 + z) % 6) as u32, "outside the disc nothing changes");
                }
            }
        }
    }

    #[test]
    fn tube_and_sphere_carve_inside_their_radius() {
        let mut v = VoxelVolume::new(UVec3::new(16, 16, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        v.fill(IVec3::ZERO, IVec3::splat(16), 2);
        v.tube(Vec3::new(2.5, 8.5, 8.5), Vec3::new(13.5, 8.5, 8.5), 1.2, AIR);
        for x in 3..13 {
            assert_eq!(v.get(IVec3::new(x, 8, 8)), AIR, "the axis is open at {x}");
        }
        assert_eq!(v.get(IVec3::new(8, 8, 12)), 2, "far from the axis stays");
        let open = v.count(|id| id == AIR);
        assert!(open > 20 && open < 160, "{open}");
        v.sphere(Vec3::new(8.0, 3.0, 8.0), 2.0, 7);
        assert_eq!(v.get(IVec3::new(8, 3, 8)), 7);
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

    #[test]
    fn the_hash_is_a_fixed_function_of_the_world() {
        // A golden value: the hash of this small world must not change between runs, builds or
        // machines (replays and snapshots compare it). If it changes on purpose, the volume's
        // canonical form changed: say so in the changelog.
        let mut v = small();
        for x in 0..16 {
            for z in 0..16 {
                v.raise_column(x, z, 1 + ((x * 7 + z * 3) % 5) as u32, 2, 1);
            }
        }
        v.sphere(Vec3::new(8.0, 3.0, 8.0), 1.5, AIR);
        let a = v.hash();
        let mut w = small();
        for x in (0..16).rev() {
            for z in (0..16).rev() {
                w.raise_column(x, z, 1 + ((x * 7 + z * 3) % 5) as u32, 2, 1);
            }
        }
        w.sphere(Vec3::new(8.0, 3.0, 8.0), 1.5, AIR);
        assert_eq!(a, w.hash(), "the order of independent edits does not matter");
        assert_eq!(a, GOLDEN_SMALL, "{a:#018x}");
        // The size, chunk size, block size and origin are part of the world.
        let other_chunk = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::new(4, 8, 4)).unwrap();
        assert_ne!(small().hash(), other_chunk.hash());
        let moved = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::new(0.0, -1.0, 0.0), UVec3::new(8, 8, 8)).unwrap();
        assert_ne!(small().hash(), moved.hash());
        let finer = VoxelVolume::new(UVec3::new(16, 8, 16), 0.5, Vec3::ZERO, UVec3::new(8, 8, 8)).unwrap();
        assert_ne!(small().hash(), finer.hash());
        // The surface is part of it too, though no cell changes.
        let mut s = small();
        let before = s.hash();
        s.set_surface(1, 1, 3);
        assert_ne!(s.hash(), before);
    }

    const GOLDEN_SMALL: u64 = 0x1bc22e89b74ee614;

    #[test]
    fn an_anchor_puts_its_lattice_point_at_the_world_origin() {
        let origin = VoxelVolume::origin_for_anchor(UVec3::new(160, 12, 96), 1.0);
        let v = VoxelVolume::new(UVec3::new(256, 32, 256), 1.0, origin, UVec3::new(64, 32, 64)).unwrap();
        assert_eq!(v.corner(IVec3::new(160, 12, 96)), Vec3::ZERO);
        assert_eq!(v.cell_at(Vec3::new(0.2, -0.2, 0.2)), IVec3::new(160, 11, 96), "just below the anchor is the cell under it");
    }

    #[test]
    fn a_shape_is_checked_without_allocating_it() {
        // 134 M one-cell chunks would be 2 GB of headers: refused by the check alone.
        let e = VoxelVolume::check_shape(UVec3::splat(512), 1.0, Vec3::ZERO, UVec3::ONE).unwrap_err();
        assert!(e.contains("chunks") && e.contains("bigger chunk"), "{e}");
        assert!(VoxelVolume::new(UVec3::splat(512), 1.0, Vec3::ZERO, UVec3::ONE).is_err());
        // One-cell columns over the cap on cells: the check says so, nothing is built.
        assert!(VoxelVolume::check_shape(UVec3::new(1 << 27, 2, 1), 1.0, Vec3::ZERO, UVec3::new(1 << 20, 2, 1)).is_err());
        // The default chunk over a big region is fine.
        VoxelVolume::check_shape(UVec3::new(1024, 64, 1024), 1.0, Vec3::ZERO, UVec3::new(64, 32, 64)).unwrap();
        let e = VoxelVolume::check_shape(UVec3::new(10, 8, 16), 1.0, Vec3::ZERO, UVec3::new(8, 8, 8)).unwrap_err();
        assert!(e.contains("try [16, 8, 16]"), "{e}");
    }

    #[test]
    fn edits_far_beyond_the_volume_return_at_once() {
        let mut v = small();
        v.sphere(Vec3::new(8.0, 4.0, 8.0), 1e9, 3);
        assert_eq!(v.count(|id| id == 3), 16 * 8 * 16, "a huge sphere fills the volume");
        v.tube(Vec3::new(-1e6, 4.0, 4.0), Vec3::new(1e6, 4.0, 4.0), 1.0, AIR);
        assert_eq!(v.get(IVec3::new(5, 4, 4)), AIR, "a long tube carves the part inside");
        assert_eq!(v.get(IVec3::new(5, 1, 4)), 3);
        v.tube(Vec3::new(1e30, 0.0, 0.0), Vec3::new(-1e30, 0.0, 0.0), 1.0, AIR);
        v.tube(Vec3::new(0.0, 0.0, f32::INFINITY), Vec3::ZERO, 1.0, AIR);
        v.sphere(Vec3::splat(f32::NAN), f32::NAN, 1);
        v.flatten(Vec2::new(8.0, 8.0), 1e9, 3, 2, 1);
        assert_eq!(v.surface(0, 0), 3);
        assert_eq!(v.surface(15, 15), 3);
        // The clipped tube is the unclipped tube: the same cells inside.
        let (mut a, mut b) = (small(), small());
        a.tube(Vec3::new(1.0, 2.0, 3.0), Vec3::new(14.0, 5.0, 9.0), 1.5, 2);
        b.tube(Vec3::new(1.0, 2.0, 3.0), Vec3::new(14.0, 5.0, 9.0), 1.5, 2);
        assert_eq!(a, b);
        assert!(a.count(|id| id == 2) > 20);
    }

    #[test]
    fn negative_zero_and_zero_hash_alike_and_compacting_changes_no_equality() {
        let anchor = VoxelVolume::origin_for_anchor(UVec3::new(0, 4, 0), 1.0);
        let a = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, anchor, UVec3::new(8, 8, 8)).unwrap();
        let b = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::new(0.0, -4.0, 0.0), UVec3::new(8, 8, 8)).unwrap();
        assert_eq!(a.hash(), b.hash(), "-0.0 and 0.0 are the same origin");
        let mut v = small();
        v.set(IVec3::ZERO, 2);
        v.set(IVec3::ZERO, AIR);
        let c = v.clone();
        v.compact();
        assert_eq!(v, c, "a dense all-air chunk equals its uniform form");
        assert_eq!(v.hash(), c.hash());
        assert_eq!(v.chunk_uniform(IVec3::ZERO), Some(AIR));
        assert_eq!(c.chunk_uniform(IVec3::ZERO), None);
        assert_eq!(c.chunk_cells(IVec3::ZERO).map(<[u8]>::len), Some(512));
        assert!(v.chunk_cells(IVec3::ZERO).is_none() && v.chunk_uniform(IVec3::new(2, 0, 0)).is_none());
        v.set(IVec3::new(1, 1, 1), 5);
        assert_ne!(v, c);
    }
}
