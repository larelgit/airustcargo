//! Boxes out of a volume, for physics as it is (convex colliders): the collision candidate (c).
//!
//! [`column_boxes`]: every column is a list of runs of solid cells `[y0, y1)`; runs with the same
//! `y0..y1` in neighbouring columns are merged greedily into rectangles (along x, then z). A
//! box's top is the real top of every column under it and its bottom the real bottom, so what
//! upstream code reads off a collider's bounding box stays true: the top of a blocker
//! (`Terrain::blocker_top`), the floor under a solid (the nav bake's seed), the surface of water.
//!
//! [`greedy_boxes`] (counted only): fewer boxes, grown in x, z, then y over all solid cells, but
//! a box's top is no longer the top of the ground under it.

use bevy::prelude::*;

use crate::volume::VoxelVolume;

/// A box of cells `min..max` (max exclusive), in cell coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellBox {
    pub min: IVec3,
    pub max: IVec3,
}

impl CellBox {
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
            let mut col = Vec::new();
            let mut start: Option<u16> = None;
            for y in 0..s.y as i32 {
                let on = keep(v.get(IVec3::new(x, y, z)));
                match (on, start) {
                    (true, None) => start = Some(y as u16),
                    (false, Some(y0)) => {
                        col.push((y0, y as u16));
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(y0) = start {
                col.push((y0, s.y as u16));
            }
            out.push(col);
        }
    }
    out
}

/// Column runs of `keep` cells, merged into rectangles of equal runs.
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

/// Greedy boxes over every `keep` cell: grow along x, then z, then y. Fewer boxes than
/// [`column_boxes`] on hills (the rock below the lowest neighbour merges into slabs), but a box's
/// top is not the ground's.
pub fn greedy_boxes(v: &VoxelVolume, keep: impl Fn(u8) -> bool) -> Vec<CellBox> {
    let s = v.size().as_ivec3();
    let idx = |p: IVec3| ((p.y * s.z + p.z) * s.x + p.x) as usize;
    let mut free: Vec<bool> = vec![false; (s.x * s.y * s.z) as usize];
    for y in 0..s.y {
        for z in 0..s.z {
            for x in 0..s.x {
                let p = IVec3::new(x, y, z);
                free[idx(p)] = keep(v.get(p));
            }
        }
    }
    let mut out = Vec::new();
    for y in 0..s.y {
        for z in 0..s.z {
            for x in 0..s.x {
                if !free[idx(IVec3::new(x, y, z))] {
                    continue;
                }
                let mut x1 = x + 1;
                while x1 < s.x && free[idx(IVec3::new(x1, y, z))] {
                    x1 += 1;
                }
                let mut z1 = z + 1;
                while z1 < s.z && (x..x1).all(|xx| free[idx(IVec3::new(xx, y, z1))]) {
                    z1 += 1;
                }
                let mut y1 = y + 1;
                while y1 < s.y && (z..z1).all(|zz| (x..x1).all(|xx| free[idx(IVec3::new(xx, y1, zz))])) {
                    y1 += 1;
                }
                for yy in y..y1 {
                    for zz in z..z1 {
                        for xx in x..x1 {
                            free[idx(IVec3::new(xx, yy, zz))] = false;
                        }
                    }
                }
                out.push(CellBox { min: IVec3::new(x, y, z), max: IVec3::new(x1, y1, z1) });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(boxes: &[CellBox]) -> i32 {
        boxes.iter().map(|b| (b.max - b.min).element_product()).sum()
    }

    #[test]
    fn boxes_cover_every_solid_cell_once() {
        let mut v = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        for x in 0..16 {
            for z in 0..16 {
                v.raise_column(x, z, 1 + ((x * 7 + z * 3) % 5) as u32, 2, 1);
            }
        }
        v.set(IVec3::new(3, 2, 3), 0);
        v.set(IVec3::new(9, 6, 9), 3);
        let solid = |id: u8| id != 0;
        let total = (0..16).flat_map(|x| (0..16).flat_map(move |z| (0..8).map(move |y| IVec3::new(x, y, z)))).filter(|p| solid(v.get(*p))).count() as i32;
        let cols = column_boxes(&v, solid);
        let greedy = greedy_boxes(&v, solid);
        assert_eq!(cells(&cols), total);
        assert_eq!(cells(&greedy), total);
        assert!(greedy.len() <= cols.len());
        // Column boxes: the top of every box is the top of a run.
        for b in &cols {
            for x in b.min.x..b.max.x {
                for z in b.min.z..b.max.z {
                    assert!(!solid(v.get(IVec3::new(x, b.max.y, z))), "{b:?} ends where its columns' runs end");
                }
            }
        }
    }

    #[test]
    fn a_lake_over_a_bowl_is_a_few_boxes_and_a_cave_under_it_stops_one() {
        // Ground of uneven height 1..5 under water to y = 6, and air at y = 0 under (4, 4).
        let mut v = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        for x in 0..16 {
            for z in 0..16 {
                let h = 1 + ((x * 7 + z * 3) % 5) as u32;
                v.raise_column(x, z, h, 2, 1);
                v.fill(IVec3::new(x, h as i32, z), IVec3::new(x + 1, 6, z + 1), 9);
            }
        }
        v.set(IVec3::new(4, 0, 4), 0);
        let boxes = water_boxes(&v, |id| id == 9, |id| id != 0 && id != 9);
        assert_eq!(boxes.len(), 5, "around the cave column: {boxes:?}");
        assert!(boxes.iter().all(|b| b.max.y == 6), "the tops are the surface");
        let cave = boxes.iter().find(|b| (b.min.x..b.max.x).contains(&4) && (b.min.z..b.max.z).contains(&4)).unwrap();
        assert_eq!((cave.min.y, cave.max - cave.min), (1, IVec3::new(1, 5, 1)), "stops above the cave");

        // An island and a shore, solid above the water, do not split a lake.
        let mut v = VoxelVolume::new(UVec3::new(16, 8, 16), 1.0, Vec3::ZERO, UVec3::splat(8)).unwrap();
        for x in 0..16 {
            for z in 0..16 {
                let land = (7..10).contains(&x) && (7..10).contains(&z) || x == 15;
                v.raise_column(x, z, if land { 7 } else { 2 }, 2, 1);
                if !land {
                    v.fill(IVec3::new(x, 2, z), IVec3::new(x + 1, 6, z + 1), 9);
                }
            }
        }
        let boxes = water_boxes(&v, |id| id == 9, |id| id != 0 && id != 9);
        assert_eq!(boxes, vec![CellBox { min: IVec3::ZERO, max: IVec3::new(16, 6, 16) }]);
    }
}
