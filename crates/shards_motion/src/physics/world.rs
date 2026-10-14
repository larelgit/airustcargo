//! [`PhysicsWorld`]: world-space colliders and the queries on them. The only place
//! that talks to parry; everything public is in our own types (glam `Vec3`/`Quat`).

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use parry3d::math::Pose;
use parry3d::query::{self, Ray, ShapeCastOptions};
use parry3d::shape::{Ball, Capsule, Cone, Cuboid, Cylinder, Segment, Shape as ParryShape};

use super::{Body, ColliderData, ColliderId, CollisionLayers, QueryFilter, Shape, VolumeKind};

/// What a ray hit first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub entity: Entity,
    /// Along the ray from its origin; 0 when the origin is inside a collider.
    pub distance: f32,
    pub point: Vec3,
    /// Surface normal at `point`, facing the ray's origin.
    pub normal: Vec3,
}

/// Where a moving shape first touched something.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeHit {
    pub entity: Entity,
    /// How far the shape travels before touching; 0 when it starts overlapping.
    pub distance: f32,
    /// Contact point on the collider that was hit.
    pub point: Vec3,
    /// Surface normal of the collider that was hit, facing the moving shape.
    pub normal: Vec3,
}

/// A collider a shape sinks into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Penetration {
    pub entity: Entity,
    /// Push the shape this way (out of the collider)...
    pub normal: Vec3,
    /// ...this far (m) to separate them.
    pub depth: f32,
}

/// One collider in world space, as of the last sync.
#[derive(Clone, Debug)]
pub struct ColliderInfo {
    pub entity: Entity,
    pub id: ColliderId,
    pub shape: Shape,
    /// World position of the shape's center (entity position + rotated offset).
    pub position: Vec3,
    pub rotation: Quat,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
    pub layers: CollisionLayers,
    pub body: Body,
    /// Set for sensors.
    pub volume: Option<VolumeKind>,
    geom: Geom,
}

impl ColliderInfo {
    fn pose(&self) -> Pose {
        Pose::from_parts(self.position, self.rotation)
    }

    fn passes(&self, filter: &QueryFilter) -> bool {
        (filter.volumes || self.volume.is_none())
            && filter.mask.intersects(self.layers.memberships)
            && !filter.exclude.contains(&self.entity)
    }

    /// Whether `point` is inside the shape.
    pub fn contains(&self, point: Vec3) -> bool {
        aabb_contains(self.aabb_min, self.aabb_max, point) && self.geom.as_dyn().contains_point(&self.pose(), point)
    }
}

/// Our [`Shape`] as a parry shape. Small value types: kept by value, no allocation.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Geom {
    Cuboid(Cuboid),
    Ball(Ball),
    Capsule(Capsule),
    Cylinder(Cylinder),
    Cone(Cone),
    /// The core of a capsule (see [`Geom::split`]).
    Segment(Segment),
}

impl Geom {
    /// Negative or NaN sizes become 0, so a bad value in content cannot crash a query.
    pub(crate) fn new(shape: &Shape) -> Self {
        let len = |v: f32| if v >= 0.0 && v.is_finite() { v } else { 0.0 };
        match *shape {
            Shape::Cuboid { half } => Geom::Cuboid(Cuboid::new(Vec3::new(len(half.x), len(half.y), len(half.z)))),
            Shape::Sphere { radius } => Geom::Ball(Ball::new(len(radius))),
            Shape::Capsule { half_height, radius } => Geom::Capsule(Capsule::new_y(len(half_height), len(radius))),
            Shape::Cylinder { half_height, radius } => Geom::Cylinder(Cylinder::new(len(half_height), len(radius))),
            Shape::Cone { half_height, radius } => Geom::Cone(Cone::new(len(half_height), len(radius))),
        }
    }

    fn as_dyn(&self) -> &dyn ParryShape {
        match self {
            Geom::Cuboid(s) => s,
            Geom::Ball(s) => s,
            Geom::Capsule(s) => s,
            Geom::Cylinder(s) => s,
            Geom::Cone(s) => s,
            Geom::Segment(s) => s,
        }
    }

    /// A rounded shape as its core plus a radius. GJK's closest points between shapes
    /// that touch are ill-defined (any normal will do), so contacts are measured
    /// between the cores, which stay a radius apart.
    fn split(&self) -> (Geom, f32) {
        match self {
            Geom::Capsule(c) => (Geom::Segment(c.segment), c.radius),
            Geom::Ball(b) => (Geom::Ball(Ball::new(0.0)), b.radius),
            other => (*other, 0.0),
        }
    }

    fn aabb(&self, pose: &Pose) -> (Vec3, Vec3) {
        let aabb = self.as_dyn().compute_aabb(pose);
        (aabb.mins, aabb.maxs)
    }
}

/// Signed distance between two posed shapes (negative: how deep they overlap), the
/// unit normal from the first toward the second, and the closest point on the second.
/// `None` when they are farther apart than `prediction`.
fn separation(p1: &Pose, g1: &Geom, p2: &Pose, g2: &Geom, prediction: f32) -> Option<(f32, Vec3, Vec3)> {
    let ((c1, r1), (c2, r2)) = (g1.split(), g2.split());
    let contact = query::contact(p1, c1.as_dyn(), p2, c2.as_dyn(), prediction + r1 + r2).ok()??;
    let normal = contact.normal1.try_normalize()?;
    Some((contact.dist - r1 - r2, normal, contact.point2 - normal * r2))
}

/// GJK may overestimate a time of impact by this much (m), so later candidates are
/// still cast a little beyond the best refined hit so far.
const REFINE_SLACK: f32 = 0.01;

/// GJK's time of impact is only good to a few millimeters (its tolerance is relative).
/// Newton steps on the signed distance along the cast make it exact to float precision
/// and give the true surface normal. Grazing hits keep GJK's answer.
/// Only pairs with a rounded shape: two sharp shapes that touch have no well-defined
/// contact normal. `None`: the true impact lies beyond `max`.
fn refine(geom: &Geom, pose: &Pose, dir: Vec3, max: f32, c: &ColliderInfo, hit: ShapeHit) -> Option<ShapeHit> {
    if hit.distance <= 0.0 || geom.split().1 + c.geom.split().1 <= 0.0 {
        return Some(hit);
    }
    let other = c.pose();
    let mut best = hit;
    let mut t = hit.distance;
    for _ in 0..3 {
        let at = Pose::from_parts(pose.translation + dir * t, pose.rotation);
        let Some((dist, normal, point)) = separation(&at, geom, &other, &c.geom, 0.1) else {
            break;
        };
        let closing = dir.dot(normal);
        if closing < 0.05 {
            break;
        }
        best = ShapeHit { entity: c.entity, distance: t, point, normal: -normal };
        let next = (t + dist / closing).max(0.0);
        if next > max {
            return None;
        }
        if (next - t).abs() < 1e-6 {
            break;
        }
        t = next;
        best.distance = t;
    }
    Some(best)
}

/// Whether moving along `dir` takes `geom` nearer to `c`, by more than rounding. By
/// convexity a shape that does not draw nearer at the start never meets it.
fn approaches(geom: &Geom, pose: &Pose, dir: Vec3, c: &ColliderInfo) -> bool {
    separation(pose, geom, &c.pose(), &c.geom, REFINE_SLACK).is_none_or(|(_, normal, _)| dir.dot(normal) > 1e-4)
}

/// How a cast treats a shape that already overlaps something at its start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartOverlap {
    /// Report it as a hit at distance 0 (public queries).
    Hit,
    /// Ignore it if the motion leads out of the overlap (or along it), so a body that
    /// ended up inside a wall can still walk away from it (the character controller).
    LetOut,
}

/// Every collider in world space, ordered by [`ColliderId`], plus the queries.
/// Kept current by [`sync_physics`](super::sync_physics); usable from `&World`.
#[derive(Resource, Default, Debug)]
pub struct PhysicsWorld {
    /// Sorted by id. New colliders have the highest id, so they are pushed at the end.
    colliders: Vec<ColliderInfo>,
    ids: HashMap<Entity, ColliderId>,
}

impl PhysicsWorld {
    pub fn len(&self) -> usize {
        self.colliders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colliders.is_empty()
    }

    /// All colliders in id order.
    pub fn colliders(&self) -> impl Iterator<Item = &ColliderInfo> {
        self.colliders.iter()
    }

    pub fn collider(&self, entity: Entity) -> Option<&ColliderInfo> {
        self.ids.get(&entity).and_then(|id| self.by_id(*id))
    }

    pub fn by_id(&self, id: ColliderId) -> Option<&ColliderInfo> {
        self.slot(id).ok().map(|i| &self.colliders[i])
    }

    fn slot(&self, id: ColliderId) -> Result<usize, usize> {
        self.colliders.binary_search_by_key(&id, |c| c.id)
    }

    // --- maintenance (sync and hooks) ---------------------------------------------

    pub(crate) fn upsert(&mut self, (entity, id, collider, tf, body, volume): ColliderData) {
        if let Some(old) = self.ids.get(&entity).copied()
            && old != *id
        {
            self.remove(entity);
        }
        let geom = Geom::new(&collider.shape);
        let rotation = tf.rotation.normalize();
        let position = tf.translation + rotation * collider.offset;
        let (aabb_min, aabb_max) = geom.aabb(&Pose::from_parts(position, rotation));
        let info = ColliderInfo {
            entity,
            id: *id,
            shape: collider.shape,
            position,
            rotation,
            aabb_min,
            aabb_max,
            layers: collider.layers,
            body: body.copied().unwrap_or_default(),
            volume: volume.map(|v| v.kind.clone()),
            geom,
        };
        match self.slot(*id) {
            Ok(i) => self.colliders[i] = info,
            Err(i) => self.colliders.insert(i, info),
        }
        self.ids.insert(entity, *id);
    }

    pub(crate) fn remove(&mut self, entity: Entity) {
        if let Some(id) = self.ids.remove(&entity)
            && let Ok(i) = self.slot(id)
        {
            self.colliders.remove(i);
        }
    }

    pub(crate) fn clear_volume(&mut self, entity: Entity) {
        if let Some(id) = self.ids.get(&entity).copied()
            && let Ok(i) = self.slot(id)
        {
            self.colliders[i].volume = None;
        }
    }

    pub(crate) fn refresh_kinematic<'a>(&mut self, mut read: impl FnMut(Entity) -> Option<ColliderData<'a>>) {
        let moving: Vec<Entity> =
            self.colliders.iter().filter(|c| c.body == Body::Kinematic).map(|c| c.entity).collect();
        for entity in moving {
            if let Some(data) = read(entity) {
                self.upsert(data);
            }
        }
    }

    // --- queries ------------------------------------------------------------------

    /// The first collider along the ray within `max` meters.
    pub fn raycast(&self, origin: Vec3, dir: Dir3, max: f32, filter: &QueryFilter) -> Option<RayHit> {
        let ray = Ray::new(origin, *dir);
        let mut best: Option<RayHit> = None;
        let mut reach = max;
        for c in self.colliders.iter().filter(|c| c.passes(filter)) {
            if !ray_hits_aabb(origin, *dir, reach, c.aabb_min, c.aabb_max) {
                continue;
            }
            let Some(hit) = c.geom.as_dyn().cast_ray_and_get_normal(&c.pose(), &ray, reach, true) else {
                continue;
            };
            // Strict: on a tie the lower id (seen first) wins.
            if best.is_none_or(|b| hit.time_of_impact < b.distance) {
                reach = hit.time_of_impact;
                best = Some(RayHit {
                    entity: c.entity,
                    distance: hit.time_of_impact,
                    point: origin + *dir * hit.time_of_impact,
                    normal: hit.normal,
                });
            }
        }
        best
    }

    /// Moves `shape` (centered at `origin`, turned by `rot`) along `dir` and reports the
    /// first collider it touches within `max` meters. A shape that starts overlapping
    /// something hits it at distance 0.
    pub fn shapecast(
        &self,
        shape: &Shape,
        origin: Vec3,
        rot: Quat,
        dir: Dir3,
        max: f32,
        filter: &QueryFilter,
    ) -> Option<ShapeHit> {
        let pose = Pose::from_parts(origin, rot.normalize());
        self.sweep(&Geom::new(shape), &pose, *dir, max, filter, StartOverlap::Hit)
    }

    /// Shape cast with a ready parry shape and pose; `dir` must be unit length.
    pub(crate) fn sweep(
        &self,
        geom: &Geom,
        pose: &Pose,
        dir: Vec3,
        max: f32,
        filter: &QueryFilter,
        start: StartOverlap,
    ) -> Option<ShapeHit> {
        let (lo, hi) = geom.aabb(pose);
        let travel = dir * max.max(0.0);
        let (sweep_min, sweep_max) = (lo.min(lo + travel), hi.max(hi + travel));
        let mut best: Option<ShapeHit> = None;
        let mut reach = max;
        for c in self.colliders.iter().filter(|c| c.passes(filter)) {
            if !aabb_overlap(sweep_min, sweep_max, c.aabb_min, c.aabb_max) {
                continue;
            }
            let options = ShapeCastOptions {
                max_time_of_impact: reach,
                target_distance: 0.0,
                stop_at_penetration: start == StartOverlap::Hit,
                compute_impact_geometry_on_penetration: true,
            };
            let other = c.pose();
            let Ok(Some(hit)) =
                query::cast_shapes(pose, dir, geom.as_dyn(), &other, Vec3::ZERO, c.geom.as_dyn(), options)
            else {
                continue;
            };
            let rough = ShapeHit {
                entity: c.entity,
                distance: hit.time_of_impact.max(0.0),
                point: other * hit.witness2,
                normal: -(pose.rotation * hit.normal1).normalize_or(Vec3::Y),
            };
            // Touching at the start and sliding along (a body resting against a wall) is
            // no hit. Parry tells that from GJK's normal, which is noise for shapes that
            // touch and reports a hit just past the start; the cores' normal is clean.
            if start == StartOverlap::LetOut && rough.distance < REFINE_SLACK && !approaches(geom, pose, dir, c) {
                continue;
            }
            let Some(hit) = refine(geom, pose, dir, max, c, rough) else {
                continue;
            };
            // Strict: on a tie the lower id (seen first) wins.
            if best.is_none_or(|b| hit.distance < b.distance) {
                reach = (hit.distance + REFINE_SLACK).min(max);
                best = Some(hit);
            }
        }
        best
    }

    /// Entities whose colliders overlap `shape` at `pos`, in id order.
    pub fn overlaps(&self, shape: &Shape, pos: Vec3, rot: Quat, filter: &QueryFilter) -> Vec<Entity> {
        let geom = Geom::new(shape);
        let pose = Pose::from_parts(pos, rot.normalize());
        self.overlapping(&geom, &pose, filter).map(|c| c.entity).collect()
    }

    pub(crate) fn overlapping<'a>(
        &'a self,
        geom: &'a Geom,
        pose: &'a Pose,
        filter: &'a QueryFilter,
    ) -> impl Iterator<Item = &'a ColliderInfo> + 'a {
        let (lo, hi) = geom.aabb(pose);
        self.colliders.iter().filter(move |c| {
            c.passes(filter)
                && aabb_overlap(lo, hi, c.aabb_min, c.aabb_max)
                && query::intersection_test(pose, geom.as_dyn(), &c.pose(), c.geom.as_dyn())
                    .is_ok_and(|t| t.intersecting)
        })
    }

    /// Colliders `shape` sinks into, and how to push it out of each, in id order.
    pub fn penetrations(&self, shape: &Shape, pos: Vec3, rot: Quat, filter: &QueryFilter) -> Vec<Penetration> {
        let geom = Geom::new(shape);
        let pose = Pose::from_parts(pos, rot.normalize());
        self.closer_than(&geom, &pose, filter, 0.0)
    }

    /// Colliders nearer to the shape than `margin` (penetrating ones included), with
    /// how far to push the shape to put it `margin` away from each, in id order.
    pub(crate) fn closer_than(&self, geom: &Geom, pose: &Pose, filter: &QueryFilter, margin: f32) -> Vec<Penetration> {
        let (lo, hi) = geom.aabb(pose);
        let (lo, hi) = (lo - Vec3::splat(margin), hi + Vec3::splat(margin));
        self.colliders
            .iter()
            .filter(|c| c.passes(filter) && aabb_overlap(lo, hi, c.aabb_min, c.aabb_max))
            .filter_map(|c| {
                let (dist, normal, _) = separation(pose, geom, &c.pose(), &c.geom, margin)?;
                (dist < margin).then_some(Penetration { entity: c.entity, normal: -normal, depth: margin - dist })
            })
            .collect()
    }

    /// Volumes containing `point`, in id order. `filter` narrows them by layer and
    /// exclusions (its `volumes` flag is implied).
    pub fn volumes_at(&self, point: Vec3, filter: Option<&QueryFilter>) -> Vec<(Entity, VolumeKind)> {
        self.volumes_containing(point, filter).filter_map(|c| Some((c.entity, c.volume.clone()?))).collect()
    }

    pub(crate) fn volumes_containing<'a>(
        &'a self,
        point: Vec3,
        filter: Option<&'a QueryFilter>,
    ) -> impl Iterator<Item = &'a ColliderInfo> + 'a {
        self.colliders.iter().filter(move |c| {
            c.volume.is_some()
                && filter.is_none_or(|f| f.mask.intersects(c.layers.memberships) && !f.exclude.contains(&c.entity))
                && c.contains(point)
        })
    }

    /// Volumes overlapping a shape, in id order.
    pub(crate) fn volumes_touching<'a>(&'a self, geom: &'a Geom, pose: &'a Pose) -> impl Iterator<Item = &'a ColliderInfo> + 'a {
        let (lo, hi) = geom.aabb(pose);
        self.colliders.iter().filter(move |c| {
            c.volume.is_some()
                && aabb_overlap(lo, hi, c.aabb_min, c.aabb_max)
                && query::intersection_test(pose, geom.as_dyn(), &c.pose(), c.geom.as_dyn())
                    .is_ok_and(|t| t.intersecting)
        })
    }
}

fn aabb_overlap(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.cmple(b_max).all() && b_min.cmple(a_max).all()
}

fn aabb_contains(min: Vec3, max: Vec3, p: Vec3) -> bool {
    min.cmple(p).all() && p.cmple(max).all()
}

/// Slab test: does the segment `origin → origin + dir * max` touch the box?
fn ray_hits_aabb(origin: Vec3, dir: Vec3, max: f32, min: Vec3, max_corner: Vec3) -> bool {
    let (mut t0, mut t1) = (0.0_f32, max);
    for axis in 0..3 {
        let (o, d) = (origin[axis], dir[axis]);
        if d.abs() < 1e-12 {
            if o < min[axis] || o > max_corner[axis] {
                return false;
            }
            continue;
        }
        let (a, b) = ((min[axis] - o) / d, (max_corner[axis] - o) / d);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
        if t0 > t1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_slab_test() {
        let (min, max) = (Vec3::splat(-1.0), Vec3::splat(1.0));
        assert!(ray_hits_aabb(Vec3::new(-5.0, 0.0, 0.0), Vec3::X, 10.0, min, max));
        assert!(!ray_hits_aabb(Vec3::new(-5.0, 0.0, 0.0), Vec3::X, 3.0, min, max));
        assert!(!ray_hits_aabb(Vec3::new(-5.0, 2.0, 0.0), Vec3::X, 10.0, min, max));
        assert!(ray_hits_aabb(Vec3::ZERO, Vec3::Y, 0.5, min, max), "inside counts");
    }
}
