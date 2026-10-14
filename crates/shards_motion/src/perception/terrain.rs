//! What a walker finds at a spot on the ground plane: the shared eyes of `clearance` and
//! `path_to`. One question, [`Terrain::probe`]: standing at height `from_y`, what would a
//! step to `(x, z)` find there?
//!
//! The answer is built from the same rules the controller walks by (`CharacterParams`): a
//! ray down for the floor, the slope limit, room for the capsule (steps up to `step_height`
//! do not count as obstacles), water deep enough to swim in.

use bevy::prelude::*;

use crate::controller::CharacterParams;
use crate::physics::{LayerMask, PhysicsWorld, QueryFilter, Shape, VolumeKind};

/// How far below the reference height a probe looks for a floor (m).
const FALL_PROBE: f32 = 60.0;
/// A capsule rests a skin above its floor; the room test adds this margin so it never
/// grazes the floor itself (m).
const GRAZE: f32 = 0.02;
/// A floor whose normal has a `y` above this is flat (about 2.6°).
const FLAT: f32 = 0.999;

/// Why a spot cannot be stood on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Block {
    /// Solid right at the reference height and above: taller than a character climbs.
    Wall,
    /// A floor steeper than `max_slope_deg` (the angle from flat, degrees).
    Steep(f32),
    /// Floor, but no room for the capsule: a gap too narrow, or too low.
    Tight,
}

/// What is at a spot, as seen from a walker standing at some height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Probe {
    /// Walkable ground at height `y` (a skin below the resting feet) with surface `normal`.
    Floor { y: f32, normal: Vec3 },
    /// Water deep enough to swim in, its surface at `surface`.
    Water { surface: f32 },
    /// Nothing below within the probe range: a bottomless drop.
    Void,
    Blocked(Block),
}

/// Reads the physics world the way a character of these `params` walks it.
pub struct Terrain<'a> {
    physics: &'a PhysicsWorld,
    pub params: &'a CharacterParams,
    filter: QueryFilter,
    min_ground_y: f32,
}

impl<'a> Terrain<'a> {
    /// `exclude`: an entity whose colliders never count (the walker itself).
    pub fn new(physics: &'a PhysicsWorld, params: &'a CharacterParams, exclude: Option<Entity>) -> Self {
        let mut filter = QueryFilter::new(LayerMask::WORLD);
        if let Some(entity) = exclude {
            filter = filter.excluding(entity);
        }
        Self { physics, params, filter, min_ground_y: params.max_slope_deg.clamp(0.0, 89.9).to_radians().cos() }
    }

    /// Height of the feet of a swimmer in water with its surface at `surface`.
    pub fn swim_height(&self, surface: f32) -> f32 {
        surface - self.params.float_depth
    }

    /// The first thing a step to `(x, z)` finds, looking from a walker whose feet are at
    /// `from_y`: floors from `mantle_height` above it down to the probe range.
    pub fn probe(&self, x: f32, z: f32, from_y: f32) -> Probe {
        self.survey(x, z, from_y).0
    }

    /// The top of the tallest solid that makes `(x, z)` [`Block::Tight`] for a walker whose
    /// feet are at `from_y` (`None`: the spot is not tight): how high the thing is that the
    /// body would stand against.
    pub fn blocker_top(&self, x: f32, z: f32, from_y: f32) -> Option<f32> {
        let blockers = self.survey(x, z, from_y).1;
        blockers.into_iter().filter_map(|e| self.physics.collider(e)).map(|c| c.aabb_max.y).reduce(f32::max)
    }

    /// [`probe`](Self::probe), and the solids in the way of a [`Block::Tight`] spot.
    fn survey(&self, x: f32, z: f32, from_y: f32) -> (Probe, Vec<Entity>) {
        let p = self.params;
        let reach = p.mantle_height + 0.1;
        let Some(hit) = self.physics.raycast(Vec3::new(x, from_y + reach, z), Dir3::NEG_Y, reach + FALL_PROBE, &self.filter)
        else {
            return (Probe::Void, Vec::new());
        };
        if hit.distance <= 1e-4 {
            return (Probe::Blocked(Block::Wall), Vec::new());
        }
        let (floor, normal) = (hit.point.y, hit.normal);
        // Deep water first: a swimmer does not care how steep the bottom is.
        if let Some(surface) = self.water_at(x, floor, z)
            && surface - floor > p.swim_depth
        {
            let blockers = self.in_the_way(x, z, self.swim_height(surface), 1.0, 0.0);
            return if blockers.is_empty() { (Probe::Water { surface }, blockers) } else { (Probe::Blocked(Block::Tight), blockers) };
        }
        if normal.y < self.min_ground_y {
            return (Probe::Blocked(Block::Steep(normal.y.clamp(-1.0, 1.0).acos().to_degrees())), Vec::new());
        }
        let blockers = self.in_the_way(x, z, floor, normal.y, p.step_height);
        if blockers.is_empty() { (Probe::Floor { y: floor, normal }, blockers) } else { (Probe::Blocked(Block::Tight), blockers) }
    }

    /// How much higher than a step a floor `dist` meters away may be and still be walked
    /// onto, because the ground between slopes: `dist` times the tangent of the steeper tilt
    /// of the two floors (`from_up`, `to_up`: the `y` of their normals, 1 for flat). Nothing
    /// between two flat floors, where a difference in height is a step or a ledge.
    pub fn slope_rise(&self, dist: f32, from_up: f32, to_up: f32) -> f32 {
        let up = from_up.min(to_up);
        if up > FLAT {
            return 0.0;
        }
        // Floors steeper than the limit are not ground (`probe`), so the tilt is capped there.
        let up = up.max(self.min_ground_y);
        dist * (1.0 - up * up).max(0.0).sqrt() / up
    }

    /// The highest water surface at the foot of a column, if the floor there is under water.
    fn water_at(&self, x: f32, floor: f32, z: f32) -> Option<f32> {
        self.physics
            .volumes_at(Vec3::new(x, floor + 0.05, z), None)
            .into_iter()
            .filter(|(_, kind)| matches!(kind, VolumeKind::Water))
            .filter_map(|(entity, _)| self.physics.collider(entity).map(|c| c.aabb_max.y))
            .reduce(f32::max)
    }

    /// The solids the capsule standing at `feet_y` overlaps. It is lifted by `climb` (what
    /// the controller steps over) and by the tilt of the floor (a capsule on a slope sits
    /// higher than the ray point), and shortened to keep its top where it was.
    fn in_the_way(&self, x: f32, z: f32, feet_y: f32, normal_y: f32, climb: f32) -> Vec<Entity> {
        let p = self.params;
        let radius = p.radius.max(0.01);
        let lift = climb + radius * (1.0 / normal_y.max(0.1) - 1.0);
        let height = (p.height - lift).max(2.0 * radius);
        let center = Vec3::new(x, feet_y + p.skin_width + GRAZE + lift + height * 0.5, z);
        self.physics.overlaps(&Shape::capsule(radius, height), center, Quat::IDENTITY, &self.filter)
    }

    /// Whether a standing capsule fits with its feet at `feet_y` (no lift for steps).
    pub fn room_for(&self, x: f32, z: f32, feet_y: f32) -> bool {
        self.in_the_way(x, z, feet_y, 1.0, 0.0).is_empty()
    }

    /// Top of the solid at `(x, z)` that a probe from `from_y` found in the way, if any is
    /// found within `up` meters above: for saying how high a wall is.
    pub fn top_of(&self, x: f32, z: f32, from_y: f32, up: f32) -> Option<f32> {
        let origin = Vec3::new(x, from_y + up, z);
        self.physics.raycast(origin, Dir3::NEG_Y, up + FALL_PROBE, &self.filter).map(|hit| hit.point.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{Collider, ColliderId};

    /// A world with just colliders, synced by hand.
    fn world(boxes: &[(Vec3, Vec3)]) -> PhysicsWorld {
        let mut physics = PhysicsWorld::default();
        let mut ids = 0;
        for (min, max) in boxes {
            ids += 1;
            let half = (*max - *min) * 0.5;
            let collider = Collider::new(Shape::cuboid(half));
            let tf = Transform::from_translation(*min + half);
            physics.upsert((Entity::from_raw_u32(ids).unwrap(), &ColliderId(ids), &collider, &tf, None, None));
        }
        physics
    }

    #[test]
    fn a_slope_lifts_a_floor_by_its_tilt() {
        let (physics, params) = (PhysicsWorld::default(), CharacterParams::default());
        let terrain = Terrain::new(&physics, &params, None);
        let up = |degrees: f32| degrees.to_radians().cos();
        assert_eq!(terrain.slope_rise(0.5, 1.0, up(1.0)), 0.0, "flat enough to be a step");
        let gentle = terrain.slope_rise(0.5, up(3.6), 1.0);
        assert!((gentle - 0.5 * 3.6f32.to_radians().tan()).abs() < 1e-4, "{gentle}");
        assert!((terrain.slope_rise(0.5, 1.0, up(30.0)) - 0.5 * 30f32.to_radians().tan()).abs() < 1e-4, "the steeper of the two counts");
        assert!((terrain.slope_rise(0.5, up(60.0), 1.0) - 0.5).abs() < 1e-4, "capped at the walkable limit (45°)");
    }

    #[test]
    fn floors_walls_and_cliffs() {
        let physics = world(&[
            (Vec3::new(-20.0, -1.0, -20.0), Vec3::new(0.0, 0.0, 20.0)),
            (Vec3::new(2.0, -1.0, -20.0), Vec3::new(20.0, 0.0, 20.0)),
            (Vec3::new(5.0, 0.0, -2.0), Vec3::new(6.0, 3.0, 2.0)),
        ]);
        let params = CharacterParams::default();
        let terrain = Terrain::new(&physics, &params, None);
        assert!(matches!(terrain.probe(-3.0, 0.0, 0.0), Probe::Floor { y, .. } if y.abs() < 1e-4));
        assert_eq!(terrain.probe(1.0, 0.0, 0.0), Probe::Void, "the gap between the slabs has no floor");
        assert_eq!(terrain.probe(5.5, 0.0, 0.0), Probe::Blocked(Block::Wall));
        assert_eq!(terrain.probe(4.7, 0.0, 0.0), Probe::Blocked(Block::Tight), "the wall is within the body's reach");
        assert!(matches!(terrain.probe(3.5, 0.0, 0.0), Probe::Floor { .. }));
    }
}
