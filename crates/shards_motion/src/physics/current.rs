//! Water that pushes: the [`Current`] on a water volume and [`current_at`].
//!
//! A region with an edge (a voxel island in its sea) puts a [`Current`] on the water volumes
//! it spawns: near the edge of `area` the water pushes swimmers toward the middle, from nothing
//! `starts` metres inside the edge to full strength `full` metres inside it. This module only
//! says how strong and which way; moving bodies by it is the swim controller's (a later
//! change), so a current changes nothing in the world by itself.
//!
//! [`edge_distance`] is the one definition of "distance to the edge" the current, the voxel
//! tools and the conditions share, so they never disagree.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::{PhysicsWorld, VolumeKind};

/// Water that pushes swimmers toward the middle of `area` near its edge. On the entity of a
/// water [`Volume`](super::Volume); the region that spawns the volume puts it there.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Current {
    /// The area (x, z) whose edge the current turns swimmers back from: a voxel region's grid.
    pub area: Rect,
    /// Metres inside the edge where the push starts (strength 0 there).
    pub starts: f32,
    /// Metres inside the edge where the push is full (strength 1 from there out).
    pub full: f32,
}

impl Current {
    /// The push at `p`: toward the centre of `area` in the ground plane (y is 0), as long as
    /// its strength, `clamp((starts - edge) / (starts - full), 0, 1)` with `edge` the
    /// [`edge_distance`] of `p`. Zero at the centre and anywhere at least `starts` inside.
    /// A current with `starts <= full` is full wherever `edge < starts`.
    pub fn at(&self, p: Vec3) -> Vec3 {
        let strength = self.strength(p);
        if strength <= 0.0 {
            return Vec3::ZERO;
        }
        let to_centre = self.area.center() - p.xz();
        match to_centre.try_normalize() {
            Some(dir) => Vec3::new(dir.x, 0.0, dir.y) * strength,
            None => Vec3::ZERO,
        }
    }

    /// How strong the push is at `p`, 0 to 1.
    pub fn strength(&self, p: Vec3) -> f32 {
        let edge = edge_distance(self.area, p);
        if edge.is_nan() || edge >= self.starts {
            return 0.0;
        }
        let span = self.starts - self.full;
        if span <= 0.0 {
            return 1.0;
        }
        ((self.starts - edge) / span).clamp(0.0, 1.0)
    }

    /// Whether the numbers make a current: finite, `starts` above 0, `full` 0 or more and below
    /// `starts`, an area with a size. What content is checked against.
    pub fn problem(&self) -> Option<String> {
        if !(self.starts.is_finite() && self.full.is_finite() && self.starts > 0.0 && self.full >= 0.0 && self.full < self.starts) {
            return Some(format!("starts {} and full {}: starts above 0, full 0 or more and less than starts", self.starts, self.full));
        }
        let size = self.area.size();
        if !(size.x > 0.0 && size.y > 0.0 && size.is_finite()) {
            return Some(format!("the area {:?} has no size", self.area));
        }
        None
    }
}

/// The signed distance from `p` (its x and z) to the edge of `area`: positive inside, negative
/// outside. Inside it is the distance to the nearest side; outside, the largest overshoot past a
/// side, negated (at a corner region outside that is not the straight-line distance to the
/// corner; any signed distance would do, and every user shares this one).
pub fn edge_distance(area: Rect, p: Vec3) -> f32 {
    let (x, z) = (p.x, p.z);
    let dx = (x - area.min.x).min(area.max.x - x);
    let dz = (z - area.min.y).min(area.max.y - z);
    dx.min(dz)
}

/// The current at `p`: of the water volumes holding `p` that carry a [`Current`], the strongest
/// push (the first in collider id order on a tie); zero where no current reaches.
pub fn current_at(physics: &PhysicsWorld, world: &World, p: Vec3) -> Vec3 {
    let mut best = Vec3::ZERO;
    for (entity, kind) in physics.volumes_at(p, None) {
        if kind != VolumeKind::Water {
            continue;
        }
        let Some(current) = world.get::<Current>(entity) else { continue };
        let push = current.at(p);
        if push.length_squared() > best.length_squared() {
            best = push;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> Current {
        Current { area: Rect::new(0.0, 0.0, 100.0, 60.0), starts: 10.0, full: 2.0 }
    }

    #[test]
    fn the_edge_distance_is_positive_inside_and_negative_outside() {
        let area = Rect::new(0.0, 0.0, 100.0, 60.0);
        assert_eq!(edge_distance(area, Vec3::new(50.0, 3.0, 30.0)), 30.0, "the middle: the nearer sides are 30 m off");
        assert_eq!(edge_distance(area, Vec3::new(4.0, 0.0, 30.0)), 4.0);
        assert_eq!(edge_distance(area, Vec3::new(50.0, 0.0, 59.0)), 1.0);
        assert_eq!(edge_distance(area, Vec3::new(0.0, 0.0, 30.0)), 0.0, "on the edge");
        assert_eq!(edge_distance(area, Vec3::new(-5.0, 0.0, 30.0)), -5.0, "outside");
        assert_eq!(edge_distance(area, Vec3::new(-5.0, 0.0, -8.0)), -8.0, "past a corner: the larger overshoot");
    }

    #[test]
    fn a_current_pushes_toward_the_middle_near_the_edge_only() {
        let c = current();
        assert_eq!(c.at(Vec3::new(50.0, 0.0, 30.0)), Vec3::ZERO, "the middle");
        assert_eq!(c.at(Vec3::new(20.0, 0.0, 30.0)), Vec3::ZERO, "20 m in: past where it starts");
        assert_eq!(c.at(Vec3::new(10.0, 0.0, 30.0)), Vec3::ZERO, "exactly where it starts: nothing yet");
        let half = c.at(Vec3::new(6.0, 5.0, 30.0));
        assert!((half.length() - 0.5).abs() < 1e-6, "halfway between starts and full: {half}");
        assert!(half.x > 0.0 && half.y == 0.0, "east, toward the middle, flat: {half}");
        let full = c.at(Vec3::new(1.0, 0.0, 30.0));
        assert!((full.length() - 1.0).abs() < 1e-6, "inside `full`: full strength");
        assert!((c.at(Vec3::new(-50.0, 0.0, 30.0)).length() - 1.0).abs() < 1e-6, "outside: full");
        assert_eq!(c.strength(Vec3::new(98.5, 0.0, 30.0)), 1.0);
        let corner = c.at(Vec3::new(1.0, 0.0, 1.0));
        assert!(corner.x > 0.0 && corner.z > 0.0, "a corner pushes toward the centre: {corner}");
    }

    #[test]
    fn bad_numbers_are_named() {
        assert_eq!(current().problem(), None);
        assert!(Current { full: 10.0, ..current() }.problem().unwrap().contains("less than starts"));
        assert!(Current { starts: f32::NAN, ..current() }.problem().is_some());
        assert!(Current { area: Rect::new(0.0, 0.0, 0.0, 10.0), ..current() }.problem().unwrap().contains("no size"));
    }
}
