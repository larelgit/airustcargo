//! Spatial helpers for perception: line of sight, compass directions, distances.
//!
//! Positions are reported relative to the viewer ("8.4m NE"), which is what an agent
//! can act on. Compass convention: **N = -Z, E = +X** (Bevy's default forward is -Z).

use bevy::prelude::*;

/// Blocks line of sight: an oriented box with these half extents, centered at
/// `offset` in the entity's local space (rotation respected, scale ignored).
#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct AiOccluder {
    pub half_extents: Vec3,
    pub offset: Vec3,
}

impl AiOccluder {
    pub fn new(half_extents: Vec3) -> Self {
        Self { half_extents, offset: Vec3::ZERO }
    }

    /// A box standing on the entity's origin (pillars, walls, crates).
    pub fn standing(half_extents: Vec3) -> Self {
        Self { half_extents, offset: Vec3::Y * half_extents.y }
    }

    /// World-space transform of the box center.
    pub fn box_transform(&self, tf: &Transform) -> Transform {
        Transform { translation: tf.translation + tf.rotation * self.offset, rotation: tf.rotation, scale: Vec3::ONE }
    }
}

/// Height of the "eyes" above an entity's origin, for line-of-sight rays.
pub const EYE_HEIGHT: f32 = 1.0;

/// Pluggable line of sight: `(world, from, to, entities to ignore) -> clear?`.
/// The default tests rays against [`AiOccluder`] boxes; a voxel game would replace
/// it with a DDA walk through its chunks.
#[derive(Resource, Clone, Copy)]
pub struct AiLineOfSight(pub fn(&World, Vec3, Vec3, &[Entity]) -> bool);

impl Default for AiLineOfSight {
    fn default() -> Self {
        Self(occluder_line_of_sight)
    }
}

/// `true` if nothing blocks the segment `from → to`. Occluders on `ignore` (the
/// viewer and the target themselves) never block.
pub fn line_of_sight(world: &World, from: Vec3, to: Vec3, ignore: &[Entity]) -> bool {
    let los = world.get_resource::<AiLineOfSight>().copied().unwrap_or_default();
    (los.0)(world, from, to, ignore)
}

fn occluder_line_of_sight(world: &World, from: Vec3, to: Vec3, ignore: &[Entity]) -> bool {
    let Some(mut q) = world.try_query::<(Entity, &AiOccluder, &Transform)>() else {
        return true;
    };
    !q.iter(world)
        .filter(|(e, ..)| !ignore.contains(e))
        .any(|(_, occ, tf)| segment_hits_box(from, to, &occ.box_transform(tf), occ.half_extents))
}

/// Slab test of a segment against an oriented box.
pub fn segment_hits_box(from: Vec3, to: Vec3, tf: &Transform, half: Vec3) -> bool {
    let inv = tf.rotation.inverse();
    let a = inv * (from - tf.translation);
    let b = inv * (to - tf.translation);
    let d = b - a;
    let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
    for axis in 0..3 {
        let (o, dir, h) = (a[axis], d[axis], half[axis]);
        if dir.abs() < 1e-6 {
            if o.abs() > h {
                return false;
            }
            continue;
        }
        let (mut near, mut far) = ((-h - o) / dir, (h - o) / dir);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// 8-way compass direction of a horizontal offset. N = -Z, E = +X.
pub fn compass(offset: Vec3) -> &'static str {
    let (x, z) = (offset.x, offset.z);
    if x.abs() < 1e-4 && z.abs() < 1e-4 {
        return "here";
    }
    let angle = ops::atan2(x, -z).to_degrees().rem_euclid(360.0);
    const NAMES: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    NAMES[(((angle + 22.5) / 45.0) as usize) % 8]
}

/// Unit vector for a compass name (`"n"`, `"NE"`, ...). Horizontal plane.
pub fn compass_vector(name: &str) -> Option<Vec3> {
    let v = match name.trim().to_ascii_uppercase().as_str() {
        "N" => Vec3::new(0.0, 0.0, -1.0),
        "NE" => Vec3::new(1.0, 0.0, -1.0),
        "E" => Vec3::new(1.0, 0.0, 0.0),
        "SE" => Vec3::new(1.0, 0.0, 1.0),
        "S" => Vec3::new(0.0, 0.0, 1.0),
        "SW" => Vec3::new(-1.0, 0.0, 1.0),
        "W" => Vec3::new(-1.0, 0.0, 0.0),
        "NW" => Vec3::new(-1.0, 0.0, -1.0),
        _ => return None,
    };
    Some(v.normalize())
}

/// `8.4m NE`, plus `+3.0m up` / `-2.0m down` when the height difference matters.
pub fn relative(from: Vec3, to: Vec3) -> String {
    let d = to - from;
    let flat = Vec3::new(d.x, 0.0, d.z);
    let mut s = format!("{:.1}m {}", flat.length(), compass(flat));
    if d.y.abs() >= 1.0 {
        s.push_str(&format!(" {:+.1}m {}", d.y, if d.y > 0.0 { "up" } else { "down" }));
    }
    s
}

/// `(3.0, -4.2)` on the ground plane, `(3.0, 2.0, -4.2)` when off the ground.
pub fn fmt_pos(p: Vec3) -> String {
    if p.y.abs() >= 0.05 {
        format!("({:.1}, {:.1}, {:.1})", p.x, p.y, p.z)
    } else {
        format!("({:.1}, {:.1})", p.x, p.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compass_follows_minus_z_north() {
        assert_eq!(compass(Vec3::new(0.0, 0.0, -5.0)), "N");
        assert_eq!(compass(Vec3::new(5.0, 0.0, -5.0)), "NE");
        assert_eq!(compass(Vec3::new(5.0, 0.0, 0.0)), "E");
        assert_eq!(compass(Vec3::new(0.0, 0.0, 5.0)), "S");
        assert_eq!(compass(Vec3::new(-5.0, 0.0, 0.1)), "W");
        assert_eq!(compass_vector("ne").map(compass), Some("NE"));
    }

    #[test]
    fn rays_hit_boxes() {
        let pillar = Transform::from_xyz(0.0, 2.0, 0.0);
        let half = Vec3::new(1.0, 2.0, 1.0);
        assert!(segment_hits_box(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(5.0, 1.0, 0.0), &pillar, half));
        assert!(!segment_hits_box(Vec3::new(-5.0, 1.0, 3.0), Vec3::new(5.0, 1.0, 3.0), &pillar, half));
        assert!(!segment_hits_box(Vec3::new(-5.0, 1.0, 0.0), Vec3::new(-2.0, 1.0, 0.0), &pillar, half));
        let rotated = Transform::from_xyz(0.0, 2.0, 0.0).with_rotation(Quat::from_rotation_y(45f32.to_radians()));
        assert!(segment_hits_box(Vec3::new(-1.3, 1.0, -1.3), Vec3::new(1.3, 1.0, 1.3), &rotated, half));
    }
}
