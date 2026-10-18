//! Boxes out of a volume, for physics as it is (convex colliders).
//!
//! [`column_boxes`] is the **fallback** collision (candidate (c) of the design) and the
//! reference the voxel field is measured against: every column is a list of runs of solid cells
//! `[y0, y1)`; runs with the same `y0..y1` in neighbouring columns are merged greedily into
//! rectangles (along x, then whole rows along z). A box's top is the real top of every column
//! under it and its bottom the real bottom, so what upstream code reads off a collider's bounding
//! box stays true: the top of a blocker, the floor under a solid, the surface of water.
//!
//! [`water_boxes`] is what water uses either way: a lake over a bowl is a few boxes, not one per
//! depth and per bay.
//!
//! A 3D greedy merge gives about half the boxes but box tops inside the ground, which makes a
//! blocker's top under-report a wall taller than a capsule; it is not provided.

use bevy::prelude::*;

use crate::volume::VoxelVolume;

/// A box of cells `min..max` (max exclusive), in cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellBox {
    pub min: IVec3,
    pub max: IVec3,
}

impl CellBox {
    /// Cells along each axis.
    pub fn extent(&self) -> IVec3 {
        self.max - self.min
    }

    /// How many cells the box spans.
    pub fn cells(&self) -> i64 {
        let e = self.extent();
        e.x as i64 * e.y as i64 * e.z as i64
    }

    /// Whether cell `p` is in the box.
    pub fn contains(&self, p: IVec3) -> bool {
        p.cmpge(self.min).all() && p.cmplt(self.max).all()
    }

    /// World centre and half extents.
    pub fn world(&self, v: &VoxelVolume) -> (Vec3, Vec3) {
        let (lo, hi) = (v.corner(self.min), v.corner(self.max));
        ((lo + hi) * 0.5, (hi - lo) * 0.5)
    }
}

/// Runs of cells `keep` says yes to, per column, `x * size.z + z`.
fn runs(v: &VoxelVolume, keep: &impl Fn(u8) -> bool) -> Vec<Vec<(u16, u16)>> {
    let s = v.size();
    let mut out = Vec::with_capacity((s.x * s.z) as usize);
    for x in 0..s.x as i32 {
        for z in 0..s.z as i32 {
            out.push(v.column_runs(x, z, keep).into_iter().map(|(a, b)| (a as u16, b as u16)).collect());
        }
    }
    out
}

/// Column runs of `keep` cells, merged into rectangles of equal runs. Every `keep` cell is in
/// exactly one box, and every cell of a box is a `keep` cell. Boxes come in z, then x, then y
/// order of their least corner, so the list is the same on every run.
pub fn column_boxes(v: &VoxelVolume, keep: impl Fn(u8) -> bool) -> Vec<CellBox> {
    merge_runs(v.size(), runs(v, &keep), |_, _, _| false)
}

/// Runs per column (`x * size.z + z`) merged greedily into rectangles: along x, then whole rows
/// along z. A box of run `r` may also cross a column `spare(x, z, r)` says it may, without a run
/// of its own there.
fn merge_runs(s: UVec3, cols: Vec<Vec<(u16, u16)>>, spare: impl Fn(i32, i32, (u16, u16)) -> bool) -> Vec<CellBox> {
    let (w, d) = (s.x as i32, s.z as i32);
    let mut used: Vec<Vec<bool>> = cols.iter().map(|c| vec![false; c.len()]).collect();
    let at = |x: i32, z: i32| (x * d + z) as usize;
    // The index of the free run `r` in column (x, z), if it has one.
    let find = |cols: &Vec<Vec<(u16, u16)>>, used: &Vec<Vec<bool>>, x: i32, z: i32, r: (u16, u16)| -> Option<usize> {
        let i = at(x, z);
        cols[i].iter().position(|run| *run == r).filter(|&k| !used[i][k])
    };
    let mut out = Vec::new();
    for z in 0..d {
        for x in 0..w {
            for k in 0..cols[at(x, z)].len() {
                if used[at(x, z)][k] {
                    continue;
                }
                let run = cols[at(x, z)][k];
                // Along x...
                let mut x1 = x + 1;
                let takes = |used: &Vec<Vec<bool>>, x: i32, z: i32| find(&cols, used, x, z, run).is_some() || spare(x, z, run);
                while x1 < w && takes(&used, x1, z) {
                    x1 += 1;
                }
                // ...then whole rows along z.
                let mut z1 = z + 1;
                while z1 < d && (x..x1).all(|xx| takes(&used, xx, z1)) {
                    z1 += 1;
                }
                for zz in z..z1 {
                    for xx in x..x1 {
                        if let Some(k) = find(&cols, &used, xx, zz, run) {
                            used[at(xx, zz)][k] = true;
                        }
                    }
                }
                out.push(CellBox { min: IVec3::new(x, run.0 as i32, z), max: IVec3::new(x1, run.1 as i32, z1) });
            }
        }
    }
    out
}

/// Water boxes: the `liquid` runs of every column, each reaching down through the `solid` cells
/// under it, merged into rectangles of equal runs; a box also crosses columns that are solid all
/// through its run (the shore, an island). A body is never inside a solid, so the parts of a box
/// in the ground change nothing, and the top of every box is the water's surface: a lake over a
/// bowl is a few boxes, not one per depth and per bay. Air below a liquid run (a cave under a
/// lake) stops the box at the cave's ceiling.
///
/// Every liquid cell is in exactly one box; no box contains an air cell (its cells are liquid or
/// solid).
pub fn water_boxes(v: &VoxelVolume, liquid: impl Fn(u8) -> bool, solid: impl Fn(u8) -> bool) -> Vec<CellBox> {
    let s = v.size();
    // Per column: liquid runs, extended down through solid cells.
    let mut cols = runs(v, &liquid);
    for x in 0..s.x as i32 {
        for z in 0..s.z as i32 {
            for run in cols[(x * s.z as i32 + z) as usize].iter_mut() {
                while run.0 > 0 && solid(v.get(IVec3::new(x, run.0 as i32 - 1, z))) {
                    run.0 -= 1;
                }
            }
        }
    }
    let dry = |x: i32, z: i32, r: (u16, u16)| (r.0..r.1).all(|y| solid(v.get(IVec3::new(x, y as i32, z))));
    merge_runs(s, cols, dry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::DetRng;

    const STONE: u8 = 2;
    const GRASS: u8 = 1;
    const WATER: u8 = 9;

    fn solid(id: u8) -> bool {
        id != 0 && id != WATER
    }

    /// A seeded volume: hills of height 1..=`hills`, caves, floating blocks, and a lake over the
    /// low ground up to `level`.
    fn world(seed: u64, size: UVec3, hills: u32, level: u32) -> VoxelVolume {
        let mut rng = DetRng::from_keys(&[seed, 77]);
        let mut v = VoxelVolume::new(size, 1.0, Vec3::ZERO, UVec3::new(size.x.min(4), size.y, size.z.min(4))).unwrap();
        for x in 0..size.x as i32 {
            for z in 0..size.z as i32 {
                // Blocky terrain: neighbouring columns often equal, so boxes merge.
                let h = 1 + (((x / 3) * 7 + (z / 2) * 5) as u32 + rng.below(2)) % hills;
                v.raise_column(x, z, h, STONE, GRASS);
                v.fill(IVec3::new(x, h as i32, z), IVec3::new(x + 1, level.max(h) as i32, z + 1), if h < level { WATER } else { 0 });
            }
        }
        for _ in 0..size.x * size.z / 12 {
            let p = IVec3::new(rng.below(size.x) as i32, rng.below(size.y) as i32, rng.below(size.z) as i32);
            let len = 1 + rng.below(4) as i32;
            v.carve(p, p + IVec3::new(len, 1 + rng.below(2) as i32, 1));
            if rng.chance(0.2) {
                v.set(p + IVec3::Y * 3, STONE);
            }
        }
        v
    }

    fn cells_of(v: &VoxelVolume, f: impl Fn(u8) -> bool) -> Vec<IVec3> {
        let s = v.size().as_ivec3();
        let mut out = Vec::new();
        for x in 0..s.x {
            for z in 0..s.z {
                for y in 0..s.y {
                    let p = IVec3::new(x, y, z);
                    if f(v.get(p)) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn column_boxes_cover_every_solid_cell_once_with_true_tops_and_bottoms() {
        for seed in 0..12 {
            let v = world(seed, UVec3::new(24, 12, 20), 8, 4);
            let boxes = column_boxes(&v, solid);
            let want = cells_of(&v, solid);
            // Every solid cell in exactly one box, no box cell that is not solid.
            let mut covered = std::collections::BTreeMap::<(i32, i32, i32), u32>::new();
            for b in &boxes {
                assert!(b.cells() > 0);
                for x in b.min.x..b.max.x {
                    for z in b.min.z..b.max.z {
                        for y in b.min.y..b.max.y {
                            assert!(solid(v.get(IVec3::new(x, y, z))), "seed {seed}: {b:?} holds a non-solid cell at ({x}, {y}, {z})");
                            *covered.entry((x, y, z)).or_default() += 1;
                        }
                    }
                }
            }
            assert_eq!(covered.len(), want.len(), "seed {seed}");
            assert!(covered.values().all(|&n| n == 1), "seed {seed}: a cell is in two boxes");
            assert!(want.iter().all(|p| covered.contains_key(&(p.x, p.y, p.z))), "seed {seed}");
            // The box's bottom and top are the run's true ends in every column under it.
            for b in &boxes {
                for x in b.min.x..b.max.x {
                    for z in b.min.z..b.max.z {
                        assert!(!solid(v.get(IVec3::new(x, b.max.y, z))) || b.max.y == v.size().y as i32, "seed {seed}: {b:?} stops short at ({x}, {z})");
                        assert!(b.min.y == 0 || !solid(v.get(IVec3::new(x, b.min.y - 1, z))), "seed {seed}: {b:?} starts late at ({x}, {z})");
                    }
                }
            }
            // The tops agree with the column queries.
            let top_of = |x: i32, z: i32| boxes.iter().filter(|b| b.contains(IVec3::new(x, b.min.y, z))).map(|b| b.max.y as u32).max().unwrap_or(0);
            for x in 0..24 {
                for z in 0..20 {
                    assert_eq!(top_of(x, z), v.column_top(x, z, solid), "seed {seed} ({x}, {z})");
                }
            }
            assert!(boxes.len() < want.len(), "merging does something: {} boxes for {} cells", boxes.len(), want.len());
        }
    }

    #[test]
    fn column_boxes_are_the_same_every_run_and_merge_flat_ground_into_one() {
        let v = world(3, UVec3::new(16, 8, 16), 5, 0);
        assert_eq!(column_boxes(&v, solid), column_boxes(&v, solid));
        let mut flat = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        flat.fill(IVec3::ZERO, IVec3::new(16, 3, 16), STONE);
        assert_eq!(column_boxes(&flat, solid), vec![CellBox { min: IVec3::ZERO, max: IVec3::new(16, 3, 16) }]);
        assert!(column_boxes(&VoxelVolume::new(UVec3::new(8, 8, 8), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap(), solid).is_empty());
    }

    /// The water boxes against the definition, cell by cell.
    fn check_water(seed: u64, v: &VoxelVolume) -> usize {
        let boxes = water_boxes(v, |id| id == WATER, solid);
        let liquid = cells_of(v, |id| id == WATER);
        let mut in_boxes = std::collections::BTreeMap::<(i32, i32, i32), u32>::new();
        for b in &boxes {
            for x in b.min.x..b.max.x {
                for z in b.min.z..b.max.z {
                    // The column of a box: solid up to some layer, then liquid up to the box's
                    // top, and the liquid is the column's whole run (the cell above is not water).
                    let ys: Vec<u8> = (b.min.y..b.max.y).map(|y| v.get(IVec3::new(x, y, z))).collect();
                    assert!(ys.iter().all(|&id| id == WATER || solid(id)), "seed {seed}: {b:?} holds air in ({x}, {z}): {ys:?}");
                    let dry = ys.iter().all(|&id| solid(id));
                    if !dry {
                        let first = ys.iter().position(|&id| id == WATER).unwrap();
                        assert!(ys[..first].iter().all(|&id| solid(id)) && ys[first..].iter().all(|&id| id == WATER), "seed {seed}: {b:?} column ({x}, {z}) is not ground under water: {ys:?}");
                        assert_ne!(v.get(IVec3::new(x, b.max.y, z)), WATER, "seed {seed}: {b:?} ends under more water at ({x}, {z})");
                    }
                    for y in b.min.y..b.max.y {
                        if v.get(IVec3::new(x, y, z)) == WATER {
                            *in_boxes.entry((x, y, z)).or_default() += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(in_boxes.len(), liquid.len(), "seed {seed}: every water cell is in a box");
        assert!(in_boxes.values().all(|&n| n == 1), "seed {seed}: a water cell is in two boxes");
        assert!(liquid.iter().all(|p| in_boxes.contains_key(&(p.x, p.y, p.z))), "seed {seed}");
        boxes.len()
    }

    #[test]
    fn water_boxes_hold_every_water_cell_once_and_never_an_air_cell() {
        let mut total = 0;
        for seed in 0..16 {
            let v = world(seed, UVec3::new(24, 14, 20), 7, 6);
            total += check_water(seed, &v);
        }
        assert!(total > 0);
        // No water: no boxes.
        let dry = world(1, UVec3::new(16, 8, 16), 4, 0);
        assert!(water_boxes(&dry, |id| id == WATER, solid).is_empty());
    }

    #[test]
    fn a_lake_over_a_bowl_is_a_few_boxes_and_a_cave_under_it_stops_one() {
        // Ground of uneven height 1..5 under water to y = 6, and air at y = 0 under (4, 4).
        let mut v = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        for x in 0..16 {
            for z in 0..16 {
                let h = 1 + ((x * 7 + z * 3) % 5) as u32;
                v.raise_column(x, z, h, STONE, GRASS);
                v.fill(IVec3::new(x, h as i32, z), IVec3::new(x + 1, 6, z + 1), WATER);
            }
        }
        v.set(IVec3::new(4, 0, 4), 0);
        let boxes = water_boxes(&v, |id| id == WATER, solid);
        assert_eq!(boxes.len(), 5, "around the cave column: {boxes:?}");
        assert!(boxes.iter().all(|b| b.max.y == 6), "the tops are the surface");
        let cave = boxes.iter().find(|b| b.contains(IVec3::new(4, b.min.y, 4))).unwrap();
        assert_eq!((cave.min.y, cave.extent()), (1, IVec3::new(1, 5, 1)), "stops above the cave");
        check_water(0, &v);

        // An island and a shore, solid above the water, do not split a lake.
        let mut v = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        for x in 0..16 {
            for z in 0..16 {
                let land = (7..10).contains(&x) && (7..10).contains(&z) || x == 15;
                v.raise_column(x, z, if land { 7 } else { 2 }, STONE, GRASS);
                if !land {
                    v.fill(IVec3::new(x, 2, z), IVec3::new(x + 1, 6, z + 1), WATER);
                }
            }
        }
        let boxes = water_boxes(&v, |id| id == WATER, solid);
        assert_eq!(boxes, vec![CellBox { min: IVec3::ZERO, max: IVec3::new(16, 6, 16) }]);
        check_water(1, &v);
    }

    #[test]
    fn a_box_converts_to_world_space() {
        let v = VoxelVolume::new(UVec3::new(8, 8, 8), 0.5, Vec3::new(-2.0, -1.0, 0.0), UVec3::splat(8)).unwrap();
        let b = CellBox { min: IVec3::new(2, 0, 0), max: IVec3::new(6, 4, 2) };
        assert_eq!(b.world(&v), (Vec3::new(0.0, 0.0, 0.5), Vec3::new(1.0, 1.0, 0.5)));
        assert_eq!((b.cells(), b.extent()), (32, IVec3::new(4, 4, 2)));
    }
}
