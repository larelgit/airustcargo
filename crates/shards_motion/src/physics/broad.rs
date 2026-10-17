//! The broad phase of [`PhysicsWorld`](super::PhysicsWorld): which colliders a query has to test.
//!
//! A grid over the ground plane (x, z) in levels of growing cells ([`LEVELS`]). A collider is in
//! every cell its bounding box covers on the first level whose cell is at least as wide as the box
//! (four cells, as a rule), and one wider than the last level is on a list every query reads. A
//! query looks up the cells its box (or a ray's segment) covers on each level that has colliders.
//!
//! It only narrows the search: it hands out every collider the old scan of all colliders would have
//! tested after its bounding-box check (and perhaps a few more), and the queries test them as they
//! always did, in [`ColliderId`](super::ColliderId) order. A cell is `floor(v / edge)`, which never
//! decreases as `v` grows, so two boxes that touch share a cell; a ray's cells cover its segment
//! with a margin far wider than the rounding of the slab test. Where the cells to read would cost
//! more than testing every collider (a query box as big as the world, a very long ray), or the
//! query is not finite, the caller tests every collider instead. Nothing here is ordered by
//! `Entity` or by a map's order: cells are looked up, never iterated.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

/// Cell edges of the levels, meters, smallest first.
pub(crate) const LEVELS: [f32; 4] = [2.0, 16.0, 128.0, 1024.0];

/// Extra margin around a ray's segment, meters, on top of a share of its coordinates: the slab
/// test of a ray against a box rounds, and a box it lets through must be in a cell read.
const RAY_PAD: f32 = 0.01;

/// Where a collider is in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cells {
    /// Cells `min..=max` (x, z) of level `level`.
    Grid { level: u8, min: IVec2, max: IVec2 },
    /// Wider than the widest level.
    Wide,
}

impl Cells {
    /// The cells of a (finite) bounding box.
    pub(crate) fn of(min: Vec3, max: Vec3) -> Cells {
        let width = (max.x - min.x).max(max.z - min.z);
        match LEVELS.iter().position(|edge| width <= *edge) {
            Some(level) => Cells::Grid { level: level as u8, min: cell(min, LEVELS[level]), max: cell(max, LEVELS[level]) },
            None => Cells::Wide,
        }
    }
}

/// The cell of `v` on a level of `edge` m cells. `as` saturates: boxes beyond `i32` cells share the
/// last one instead of wrapping around.
fn cell(v: Vec3, edge: f32) -> IVec2 {
    IVec2::new((v.x / edge).floor() as i32, (v.z / edge).floor() as i32)
}

/// Cells from `a` to `b` along one axis: up to 2^32, so the product of two can pass `u64` and
/// counts of cells saturate (a saturated count is over any budget).
fn span(a: i32, b: i32) -> u64 {
    (i64::from(b) - i64::from(a) + 1).max(0) as u64
}

/// The slots of colliders by cell.
#[derive(Default, Debug)]
pub(crate) struct Grid {
    cells: HashMap<(u8, IVec2), Vec<u32>>,
    /// How many colliders each level has: empty levels are not read.
    per_level: [u32; LEVELS.len()],
    wide: Vec<u32>,
}

impl Grid {
    pub(crate) fn insert(&mut self, slot: u32, cells: Cells) {
        match cells {
            Cells::Wide => self.wide.push(slot),
            Cells::Grid { level, min, max } => {
                self.per_level[level as usize] += 1;
                for z in min.y..=max.y {
                    for x in min.x..=max.x {
                        self.cells.entry((level, IVec2::new(x, z))).or_default().push(slot);
                    }
                }
            }
        }
    }

    pub(crate) fn remove(&mut self, slot: u32, cells: Cells) {
        match cells {
            Cells::Wide => {
                if let Some(i) = self.wide.iter().position(|s| *s == slot) {
                    self.wide.swap_remove(i);
                }
            }
            Cells::Grid { level, min, max } => {
                self.per_level[level as usize] -= 1;
                for z in min.y..=max.y {
                    for x in min.x..=max.x {
                        let key = (level, IVec2::new(x, z));
                        let Some(bucket) = self.cells.get_mut(&key) else { continue };
                        if let Some(i) = bucket.iter().position(|s| *s == slot) {
                            bucket.swap_remove(i);
                        }
                        if bucket.is_empty() {
                            self.cells.remove(&key);
                        }
                    }
                }
            }
        }
    }

    fn collect(&self, level: usize, min: IVec2, max: IVec2, out: &mut Vec<u32>) {
        for z in min.y..=max.y {
            for x in min.x..=max.x {
                if let Some(bucket) = self.cells.get(&(level as u8, IVec2::new(x, z))) {
                    out.extend_from_slice(bucket);
                }
            }
        }
    }

    /// Adds to `out` the slots of the colliders in the cells the box `lo..hi` covers (some more
    /// than once). `false` (and `out` left alone): more than `budget` cells, or a box that is not
    /// finite; test every collider instead.
    pub(crate) fn near_box(&self, lo: Vec3, hi: Vec3, budget: u64, out: &mut Vec<u32>) -> bool {
        if !(lo.is_finite() && hi.is_finite()) {
            return false;
        }
        // A box turned inside out (a negative margin) still touches what covers it.
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let mut ranges = [None; LEVELS.len()];
        let mut cells: u64 = 0;
        for (level, edge) in LEVELS.iter().enumerate() {
            if self.per_level[level] == 0 {
                continue;
            }
            let (min, max) = (cell(lo, *edge), cell(hi, *edge));
            cells = cells.saturating_add(span(min.x, max.x).saturating_mul(span(min.y, max.y)));
            if cells > budget {
                return false;
            }
            ranges[level] = Some((min, max));
        }
        for (level, range) in ranges.into_iter().enumerate() {
            if let Some((min, max)) = range {
                self.collect(level, min, max, out);
            }
        }
        out.extend_from_slice(&self.wide);
        true
    }

    /// [`near_box`](Self::near_box) for the segment `origin → origin + dir * max`: on each level the
    /// segment goes in pieces no wider on the ground than a cell (a ray straight down is one
    /// piece), each piece's box grown by a margin.
    pub(crate) fn near_ray(&self, origin: Vec3, dir: Vec3, max: f32, budget: u64, out: &mut Vec<u32>) -> bool {
        if !(origin.is_finite() && dir.is_finite() && max.is_finite()) {
            return false;
        }
        let len = max.max(0.0);
        let end = origin + dir * len;
        let across = len * (dir.x * dir.x + dir.z * dir.z).sqrt();
        let pad = Vec3::splat(RAY_PAD + origin.abs().max_element().max(end.abs().max_element()) * 1e-5);
        let start = out.len();
        let mut cells: u64 = 0;
        for (level, edge) in LEVELS.iter().enumerate() {
            if self.per_level[level] == 0 {
                continue;
            }
            let pieces = (across / edge).ceil().max(1.0);
            if pieces > budget as f32 {
                out.truncate(start);
                return false;
            }
            let pieces = pieces as u32;
            let at = |i: u32| if i == pieces { end } else { origin + dir * (len * i as f32 / pieces as f32) };
            for i in 0..pieces {
                let (a, b) = (at(i), at(i + 1));
                let (min, max) = (cell(a.min(b) - pad, *edge), cell(a.max(b) + pad, *edge));
                cells = cells.saturating_add(span(min.x, max.x).saturating_mul(span(min.y, max.y)));
                if cells > budget {
                    out.truncate(start);
                    return false;
                }
                self.collect(level, min, max, out);
            }
        }
        out.extend_from_slice(&self.wide);
        true
    }

    /// Every slot the grid holds, with its cells, as many times as it is listed: for tests that
    /// check the grid against the colliders.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> Vec<(u32, Option<(u8, IVec2)>)> {
        let mut out: Vec<(u32, Option<(u8, IVec2)>)> =
            self.cells.iter().flat_map(|(key, slots)| slots.iter().map(move |s| (*s, Some(*key)))).collect();
        out.extend(self.wide.iter().map(|s| (*s, None)));
        out.sort_unstable_by_key(|(s, key)| (*s, key.map(|(l, c)| (l, c.x, c.y))));
        out
    }

    #[cfg(test)]
    pub(crate) fn per_level(&self) -> [u32; LEVELS.len()] {
        self.per_level
    }
}
