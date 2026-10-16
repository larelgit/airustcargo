//! What a telegraph's decal draws, as numbers: a quad on the ground over the zone's footprint, and
//! the zone's shape in the quad's frame (a disc, a sector, a rectangle or a stadium) that the
//! decal's shader fills. The fill grows from the footprint's centroid, as the gizmo telegraph's
//! does, with the hit's urgency.

use std::f32::consts::PI;

use bevy::prelude::*;
use shards_action::draw::{centroid, footprint};
use shards_action::{HitShape, WorldShape};

/// Metres the quad reaches past the footprint on every side: the shader's soft edge fits in.
pub const MARGIN: f32 = 0.15;

/// A zone's shape in its decal's frame: `x` to the right of the zone's facing, `y` along it,
/// metres from the quad's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DecalShape {
    /// A sphere's widest disc, a ground circle, a cone of every way round.
    Disc { center: Vec2, radius: f32 },
    /// A cone: from `apex` along +y, `range` far, `half_angle` radians each side.
    Sector { apex: Vec2, range: f32, half_angle: f32 },
    /// A box or a line.
    Rect { center: Vec2, half: Vec2 },
    /// A capsule: the segment `a`..`b` widened by `radius`.
    Stadium { a: Vec2, b: Vec2, radius: f32 },
}

impl DecalShape {
    /// The shader's number for it.
    pub fn kind(&self) -> u32 {
        match self {
            DecalShape::Disc { .. } => 0,
            DecalShape::Sector { .. } => 1,
            DecalShape::Rect { .. } => 2,
            DecalShape::Stadium { .. } => 3,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            DecalShape::Disc { .. } => "disc",
            DecalShape::Sector { .. } => "sector",
            DecalShape::Rect { .. } => "rect",
            DecalShape::Stadium { .. } => "stadium",
        }
    }

    /// How far `p` (the quad's frame) is outside it: below 0 inside, 0 on its edge. The shader's
    /// function, here for tests.
    pub fn distance(&self, p: Vec2) -> f32 {
        match *self {
            DecalShape::Disc { center, radius } => (p - center).length() - radius,
            DecalShape::Sector { apex, range, half_angle } => {
                let q = p - apex;
                let round = q.length() - range;
                if half_angle >= PI - 1e-3 {
                    return round;
                }
                // Mirror to the right half; the edge ray at `half_angle` from +y.
                let q = Vec2::new(q.x.abs(), q.y);
                let edge = Vec2::new(ops::sin(half_angle), ops::cos(half_angle));
                let along = q.dot(edge).clamp(0.0, range);
                let to_edge = (q - edge * along).length();
                // Inside the wedge: the angle from +y is below the half angle.
                let inside_wedge = q.x * edge.y - q.y * edge.x <= 0.0;
                if inside_wedge { round.max(-to_edge) } else { to_edge.max(round) }
            }
            DecalShape::Rect { center, half } => {
                let d = (p - center).abs() - half;
                d.max(Vec2::ZERO).length() + d.x.max(d.y).min(0.0)
            }
            DecalShape::Stadium { a, b, radius } => {
                let ab = b - a;
                let t = if ab.length_squared() > 1e-8 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
                (p - (a + ab * t)).length() - radius
            }
        }
    }
}

/// A telegraph's decal: where its quad lies, how big, and what it fills.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoneDecal {
    /// The quad's centre, on the ground under the attacker (lifted a little).
    pub center: Vec3,
    /// The zone's facing on the ground (unit): the quad's +y.
    pub facing: Vec3,
    /// The quad's size: across the facing, along it (m).
    pub size: Vec2,
    pub shape: DecalShape,
    /// Where the fill grows from, in the quad's frame: the footprint's centroid.
    pub grow_from: Vec2,
}

impl ZoneDecal {
    /// The decal of `shape` on the ground at height `y`.
    pub fn new(shape: &WorldShape, y: f32) -> Self {
        let (o, f, r) = (shape.origin, shape.facing, shape.right());
        let local = |p: Vec3| Vec2::new((p - o).dot(r), (p - o).dot(f));
        let ring = footprint(shape, y);
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for p in &ring {
            let q = local(*p);
            (lo, hi) = (lo.min(q), hi.max(q));
        }
        // A sector's arc bulges between its points; a circle's polygon sits inside it.
        let bulge = match shape.shape {
            HitShape::Cone { range, .. } | HitShape::Sphere { radius: range } | HitShape::Circle { radius: range, .. } => range * 0.02,
            HitShape::Capsule { radius, .. } => radius * 0.02,
            _ => 0.0,
        };
        let (lo, hi) = (lo - Vec2::splat(MARGIN + bulge), hi + Vec2::splat(MARGIN + bulge));
        let mid = (lo + hi) * 0.5;
        let shape_in_quad = match shape.shape {
            HitShape::Sphere { radius } | HitShape::Circle { radius, .. } => DecalShape::Disc { center: -mid, radius },
            HitShape::Capsule { radius, length } => {
                DecalShape::Stadium { a: -mid, b: Vec2::new(0.0, length) - mid, radius }
            }
            HitShape::Cone { range, angle, .. } => {
                let half_angle = (angle.to_radians() * 0.5).min(PI);
                if half_angle >= PI - 1e-3 {
                    DecalShape::Disc { center: -mid, radius: range }
                } else {
                    DecalShape::Sector { apex: -mid, range, half_angle }
                }
            }
            HitShape::Box { width, depth, .. } => DecalShape::Rect { center: -mid, half: Vec2::new(width, depth) * 0.5 },
            HitShape::Line { length, width, .. } => {
                DecalShape::Rect { center: Vec2::new(0.0, length * 0.5) - mid, half: Vec2::new(width * 0.5, length * 0.5) }
            }
        };
        let middle = centroid(&ring);
        Self {
            center: Vec3::new(o.x, y, o.z) + r * mid.x + f * mid.y,
            facing: f,
            size: hi - lo,
            shape: shape_in_quad,
            grow_from: local(middle) - mid,
        }
    }

    /// The quad's rotation: its -Z (Bevy's forward) along the facing, its +X to the right.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(ops::atan2(-self.facing.x, -self.facing.z))
    }

    /// A world point in the quad's frame.
    pub fn local(&self, p: Vec3) -> Vec2 {
        let right = Vec3::new(-self.facing.z, 0.0, self.facing.x);
        Vec2::new((p - self.center).dot(right), (p - self.center).dot(self.facing))
    }

    /// Whether the fill, grown to `fill` (0 to 1) of the zone, covers world point `p`: the shape
    /// scaled about [`grow_from`](Self::grow_from), as the shader tests it.
    pub fn filled(&self, p: Vec3, fill: f32) -> bool {
        let q = self.local(p);
        let k = fill.max(0.04);
        self.shape.distance(self.grow_from + (q - self.grow_from) / k) <= 0.0
    }
}

#[cfg(test)]
mod tests {
    use shards_action::Local;

    use super::*;

    fn placed(shape: HitShape, at: Vec3, facing: Vec3) -> WorldShape {
        WorldShape::new(shape, Local::default(), at, facing)
    }

    #[test]
    fn the_quad_covers_the_footprint_and_the_shape_is_the_zone() {
        // A cone of 90° facing +X from (2, 0, 1): the sector from its apex, 4 m.
        let cone = placed(HitShape::Cone { range: 4.0, angle: 90.0, height: 2.0 }, Vec3::new(2.0, 0.0, 1.0), Vec3::X);
        let decal = ZoneDecal::new(&cone, 0.04);
        assert!(decal.rotation().mul_vec3(Vec3::NEG_Z).abs_diff_eq(Vec3::X, 1e-5), "the quad looks along the facing");
        assert!(decal.rotation().mul_vec3(Vec3::X).abs_diff_eq(Vec3::Z, 1e-5), "its +X is the right of the facing");
        // Every point of the zone is on the quad and in the shape; what is behind is not.
        let half = decal.size * 0.5;
        for p in footprint(&cone, 0.04) {
            let q = decal.local(p);
            assert!(q.x.abs() <= half.x && q.y.abs() <= half.y, "{p} at {q} outside the quad {half}");
            assert!(decal.shape.distance(q) < 1e-3, "{p} at {q}: {}", decal.shape.distance(q));
        }
        let ahead = Vec3::new(5.0, 0.0, 1.0);
        let behind = Vec3::new(0.5, 0.0, 1.0);
        let wide = Vec3::new(3.0, 0.0, 3.5);
        assert!(decal.shape.distance(decal.local(ahead)) < 0.0);
        assert!((decal.shape.distance(decal.local(behind)) - 1.5).abs() < 1e-4, "1.5 m behind the apex");
        assert!(decal.shape.distance(decal.local(wide)) > 0.0, "outside the 45° each side");
        assert!(matches!(decal.shape, DecalShape::Sector { .. }));
        // The quad is the sector's bounds and a margin: 4 m deep, 2·4·sin 45° wide.
        let wanted = Vec2::new(8.0 * ops::sin(45f32.to_radians()), 4.0) + Vec2::splat(2.0 * (MARGIN + 0.08));
        assert!(decal.size.abs_diff_eq(wanted, 0.05), "{} vs {wanted}", decal.size);
    }

    #[test]
    fn every_hit_shape_has_its_decal_shape() {
        let at = Vec3::new(-1.0, 3.0, 2.0);
        let facing = Vec3::new(0.0, 0.0, -1.0);
        let shape = |s: HitShape| ZoneDecal::new(&placed(s, at, facing), 3.0);
        let disc = shape(HitShape::Circle { radius: 3.0, height: 1.0 });
        assert_eq!(disc.shape.name(), "disc");
        assert!(disc.center.abs_diff_eq(Vec3::new(-1.0, 3.0, 2.0), 1e-3), "{}", disc.center);
        assert!(disc.shape.distance(disc.local(at + Vec3::X * 2.9)) < 0.0 && disc.shape.distance(disc.local(at + Vec3::X * 3.1)) > 0.0);
        assert_eq!(shape(HitShape::Sphere { radius: 1.0 }).shape.name(), "disc");
        assert_eq!(shape(HitShape::Cone { range: 2.0, angle: 360.0, height: 1.0 }).shape.name(), "disc", "every way round");
        // A line: from the origin along the facing, `length` long.
        let line = shape(HitShape::Line { length: 6.0, width: 1.0, height: 1.0 });
        assert_eq!(line.shape.name(), "rect");
        assert!(line.shape.distance(line.local(at + facing * 5.5)) < 0.0);
        assert!(line.shape.distance(line.local(at - facing * 0.5)) > 0.0, "nothing behind its start");
        assert!(line.shape.distance(line.local(at + facing * 3.0 + Vec3::X * 0.6)) > 0.0, "half a metre each side");
        // A box: centred on its point.
        let boxed = shape(HitShape::Box { width: 2.0, height: 1.0, depth: 4.0 });
        assert!(boxed.shape.distance(boxed.local(at - facing * 1.9)) < 0.0 && boxed.shape.distance(boxed.local(at + facing * 1.9)) < 0.0);
        // A capsule: a stadium from the origin along the facing.
        let capsule = shape(HitShape::Capsule { radius: 0.5, length: 3.0 });
        assert_eq!(capsule.shape.name(), "stadium");
        assert!(capsule.shape.distance(capsule.local(at + facing * 3.4)) < 0.0);
        assert!(capsule.shape.distance(capsule.local(at + facing * 3.6)) > 0.0);
        assert!(capsule.shape.distance(capsule.local(at - facing * 0.4)) < 0.0, "round behind its start");
    }

    #[test]
    fn the_fill_grows_from_the_middle_to_the_edge_with_the_urgency() {
        let circle = placed(HitShape::Circle { radius: 4.0, height: 1.0 }, Vec3::ZERO, Vec3::Z);
        let decal = ZoneDecal::new(&circle, 0.0);
        let at = |x: f32| Vec3::new(x, 0.0, 0.0);
        assert!(decal.filled(at(0.1), 0.0), "a speck at once");
        assert!(decal.filled(at(1.9), 0.5) && !decal.filled(at(2.1), 0.5), "half way at half the urgency");
        assert!(decal.filled(at(3.9), 1.0) && !decal.filled(at(4.1), 1.0), "the whole zone when it lands");
        // A sector grows from its centroid, inside the sector, not from the apex.
        let cone = placed(HitShape::Cone { range: 4.0, angle: 60.0, height: 1.0 }, Vec3::ZERO, Vec3::Z);
        let decal = ZoneDecal::new(&cone, 0.0);
        let middle = decal.grow_from;
        assert!(decal.shape.distance(middle) < 0.0, "the centroid is inside: {middle}");
        assert!(!decal.filled(Vec3::new(0.0, 0.0, 0.2), 0.3), "the apex fills last");
        assert!(decal.filled(Vec3::new(0.0, 0.0, 0.2), 1.0));
    }
}
