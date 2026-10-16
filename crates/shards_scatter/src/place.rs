//! Where a scatter's instances go on one tile. Points are thrown at random in the tile's square,
//! always the same ones (the region's seed, its id, the tile's cell and the scatter's id key the
//! random numbers, never the simulation's), each kept only if it is `spacing` away from those
//! before it. Each throw draws the same numbers whether it is kept or not, so the list at full
//! density is fixed, and the player's `graphics.foliage` keeps its first part: a thinner scatter
//! is the same scatter with fewer instances. Then each instance is dropped onto the ground
//! straight below the top of its `height` band: none where that ground is too steep, out of the
//! band (a column's top), inside a solid (a wall) or too close to one (`clearance`).
//!
//! Nothing here touches the world: [`Ground`] answers the rays, and the physics world is one.

use std::collections::HashMap;
use std::f32::consts::TAU;

use ai_core::DetRng;
use bevy::prelude::*;
use shards_motion::{LayerMask, PhysicsWorld, QueryFilter, Shape};

use crate::def::ScatterDef;

/// One thrown point, in the tile's square, at full density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// From the tile's center, along world X and Z (meters).
    pub offset: Vec2,
    /// Turn about +Y, radians.
    pub yaw: f32,
    /// The side it leans toward (radians about +Y) and how far (radians from upright).
    pub lean: (f32, f32),
    pub scale: f32,
    /// Index into the scatter's `looks`.
    pub look: usize,
}

impl Candidate {
    /// Its rotation: turned, then leaned.
    pub fn rotation(&self) -> Quat {
        let (side, angle) = self.lean;
        let axis = Vec3::new(side.cos(), 0.0, side.sin());
        Quat::from_axis_angle(axis, angle) * Quat::from_rotation_y(self.yaw)
    }
}

/// An instance on the ground, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub at: Vec3,
    pub rotation: Quat,
    pub scale: f32,
    pub look: usize,
}

/// FNV-1a of `text`: ids as random keys.
pub fn fnv64(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// The random numbers of scatter `scatter` on cell `(x, y)` (the tile's north-west cell) of
/// region `region` made with `seed`.
pub fn rng_for(seed: u64, region: &str, (x, y): (i32, i32), scatter: &str) -> DetRng {
    DetRng::from_keys(&[seed, fnv64(region), x as i64 as u64, y as i64 as u64, fnv64(scatter)])
}

/// The scatter's instances on a tile `extent` meters across (X, Z) at full density, in the order
/// they were thrown: `density × area` of them, fewer when `spacing` leaves no room (four throws
/// per instance at most).
pub fn candidates(def: &ScatterDef, rng: &mut DetRng, extent: Vec2) -> Vec<Candidate> {
    let wanted = (def.density.max(0.0) * extent.x * extent.y).round() as usize;
    if wanted == 0 || def.looks.is_empty() {
        return Vec::new();
    }
    let (small, large) = def.scale;
    let tilt = def.tilt.to_radians();
    let looks = def.looks.len() as u32;
    let spacing = def.spacing.max(0.0);
    // Instances by cell of `spacing`: lookups only, never iterated.
    let mut near: HashMap<(i32, i32), Vec<Vec2>> = HashMap::new();
    let cell = |p: Vec2| ((p.x / spacing).floor() as i32, (p.y / spacing).floor() as i32);
    let mut kept = Vec::with_capacity(wanted);
    for _ in 0..wanted * 4 {
        if kept.len() == wanted {
            break;
        }
        // Every throw draws the same numbers, kept or not.
        let offset = Vec2::new(rng.range(-0.5, 0.5) * extent.x, rng.range(-0.5, 0.5) * extent.y);
        let yaw = rng.range(0.0, TAU);
        let lean = (rng.range(0.0, TAU), rng.f32() * tilt);
        let scale = rng.range(small, large);
        let look = rng.below(looks) as usize;
        if spacing > 0.0 {
            let (cx, cz) = cell(offset);
            let crowded = (cx - 1..=cx + 1)
                .flat_map(|x| (cz - 1..=cz + 1).map(move |z| (x, z)))
                .filter_map(|c| near.get(&c))
                .flatten()
                .any(|other| other.distance_squared(offset) < spacing * spacing);
            if crowded {
                continue;
            }
            near.entry((cx, cz)).or_default().push(offset);
        }
        kept.push(Candidate { offset, yaw, lean, scale, look });
    }
    kept
}

/// How many of `all` instances `graphics.foliage` keeps (0 to 1).
pub fn thinned(all: usize, foliage: f32) -> usize {
    let foliage = if foliage.is_finite() { foliage.clamp(0.0, 1.0) } else { 1.0 };
    ((all as f32 * foliage).round() as usize).min(all)
}

/// What is solid around a tile.
pub trait Ground {
    /// The first solid straight down from `from` within `max` meters: how far, the point and
    /// its normal. Distance 0 when `from` is inside a solid.
    fn down(&self, from: Vec3, max: f32) -> Option<(f32, Vec3, Vec3)>;

    /// Whether a ball of `radius` at `at` touches a solid.
    fn blocked(&self, at: Vec3, radius: f32) -> bool;
}

/// The static world: what [`LayerMask::WORLD`] collides with (not characters, not volumes).
impl Ground for PhysicsWorld {
    fn down(&self, from: Vec3, max: f32) -> Option<(f32, Vec3, Vec3)> {
        let hit = self.raycast(from, Dir3::NEG_Y, max, &QueryFilter::new(LayerMask::WORLD))?;
        Some((hit.distance, hit.point, hit.normal))
    }

    fn blocked(&self, at: Vec3, radius: f32) -> bool {
        !self.overlaps(&Shape::Sphere { radius }, at, Quat::IDENTITY, &QueryFilter::new(LayerMask::WORLD)).is_empty()
    }
}

/// Why instances found no place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    /// No ground in the band, or a solid where it would stand.
    pub ground: usize,
    pub steep: usize,
    /// Closer to a solid than `clearance`.
    pub crowded: usize,
}

impl Skipped {
    pub fn total(&self) -> usize {
        self.ground + self.steep + self.crowded
    }

    pub fn add(&mut self, other: Skipped) {
        self.ground += other.ground;
        self.steep += other.steep;
        self.crowded += other.crowded;
    }
}

/// `candidates` of a tile centered at `center`, dropped onto `ground`: those that find a place,
/// in order, and how many did not.
pub fn settle(def: &ScatterDef, center: Vec3, candidates: &[Candidate], ground: &impl Ground) -> (Vec<Placed>, Skipped) {
    let (low, high) = def.height;
    // A little above the band, so ground at its very top is found, and a solid that reaches
    // above it (a wall) holds the ray's start.
    let top = high + 0.05;
    let reach = top - low + 0.05;
    let flattest = def.max_slope.to_radians().cos() - 1e-4;
    let mut skipped = Skipped::default();
    let mut placed = Vec::with_capacity(candidates.len());
    for c in candidates {
        let from = Vec3::new(center.x + c.offset.x, center.y + top, center.z + c.offset.y);
        let Some((_, point, normal)) = ground.down(from, reach).filter(|(d, p, _)| *d > 0.0 && p.y - center.y >= low) else {
            skipped.ground += 1;
            continue;
        };
        if normal.y < flattest {
            skipped.steep += 1;
            continue;
        }
        if def.clearance > 0.0 && ground.blocked(point + Vec3::Y * (def.clearance + 0.05), def.clearance) {
            skipped.crowded += 1;
            continue;
        }
        placed.push(Placed { at: point, rotation: c.rotation(), scale: c.scale, look: c.look });
    }
    (placed, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scatter(density: f32, spacing: f32) -> ScatterDef {
        ScatterDef { density, spacing, looks: vec!["a".into(), "b".into()], scale: (0.5, 2.0), tilt: 20.0, ..ScatterDef::default() }
    }

    fn thrown(def: &ScatterDef, seed: u64, cell: (i32, i32)) -> Vec<Candidate> {
        candidates(def, &mut rng_for(seed, "meadow", cell, "grass"), Vec2::splat(16.0))
    }

    #[test]
    fn the_same_keys_throw_the_same_points_and_others_do_not() {
        let def = scatter(1.0, 0.0);
        let first = thrown(&def, 42, (3, 4));
        assert_eq!(first.len(), 256, "density × area");
        assert_eq!(first, thrown(&def, 42, (3, 4)));
        assert_ne!(first, thrown(&def, 43, (3, 4)), "another seed");
        assert_ne!(first, thrown(&def, 42, (4, 3)), "another cell");
        assert_ne!(first, candidates(&def, &mut rng_for(42, "meadow", (3, 4), "rocks"), Vec2::splat(16.0)), "another scatter");
        for c in &first {
            assert!(c.offset.x.abs() <= 8.0 && c.offset.y.abs() <= 8.0, "inside the tile: {c:?}");
            assert!((0.5..=2.0).contains(&c.scale) && c.look < 2 && c.lean.1 <= 20f32.to_radians());
            let up = c.rotation() * Vec3::Y;
            assert!(up.angle_between(Vec3::Y) <= 20f32.to_radians() + 1e-4, "leans 20° at most: {up}");
        }
        assert!(first.iter().any(|c| c.look == 0) && first.iter().any(|c| c.look == 1), "both looks are used");
    }

    #[test]
    fn spacing_keeps_instances_apart_and_a_thinner_scatter_is_the_start_of_the_same_one() {
        let def = scatter(2.0, 0.5);
        let all = thrown(&def, 7, (0, 0));
        assert!(all.len() > 300 && all.len() <= 512, "spacing leaves room for most: {}", all.len());
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert!(a.offset.distance(b.offset) >= 0.5, "{a:?} and {b:?} are closer than spacing");
            }
        }
        assert_eq!(thinned(all.len(), 1.0), all.len());
        assert_eq!(thinned(all.len(), 0.0), 0);
        assert_eq!(thinned(10, 0.25), 3, "rounded");
        assert_eq!(thinned(10, f32::NAN), 10, "a broken setting keeps them all");
        let crowded = scatter(20.0, 1.0);
        let few = thrown(&crowded, 7, (0, 0));
        assert!(few.len() < 400, "a 1 m spacing fits far fewer than 20 a square meter: {}", few.len());
        assert!(thrown(&scatter(0.0, 0.0), 7, (0, 0)).is_empty());
    }

    /// Flat ground at y 0, a 2 m column at x ±1, z ±1 (4 m tall) and a slope rising along +x
    /// past x = 4.
    struct Fake;

    impl Ground for Fake {
        fn down(&self, from: Vec3, max: f32) -> Option<(f32, Vec3, Vec3)> {
            let (y, normal) = if from.x.abs() <= 1.0 && from.z.abs() <= 1.0 {
                (4.0, Vec3::Y)
            } else if from.x > 4.0 {
                ((from.x - 4.0) * 0.1, Vec3::new(-1.0, 1.0, 0.0).normalize())
            } else {
                (0.0, Vec3::Y)
            };
            let distance = (from.y - y).max(0.0);
            (distance <= max).then_some((distance, Vec3::new(from.x, y.min(from.y), from.z), normal))
        }

        fn blocked(&self, at: Vec3, radius: f32) -> bool {
            at.x.abs() <= 1.0 + radius && at.z.abs() <= 1.0 + radius && at.y <= 4.0 + radius
        }
    }

    #[test]
    fn instances_stand_on_flat_ground_in_their_band_and_clear_of_solids() {
        let def = ScatterDef { looks: vec!["a".into()], ..ScatterDef::default() };
        let at = |x: f32, z: f32| Candidate { offset: Vec2::new(x, z), yaw: 0.0, lean: (0.0, 0.0), scale: 1.0, look: 0 };
        let points = [at(-5.0, -5.0), at(0.0, 0.0), at(6.0, 0.0), at(1.5, 0.0), at(-6.0, 3.0)];
        let (placed, skipped) = settle(&def, Vec3::ZERO, &points, &Fake);
        let xs: Vec<f32> = placed.iter().map(|p| p.at.x).collect();
        assert_eq!(xs, [-5.0, 1.5, -6.0], "the column's top and the 45° slope get none");
        assert!(placed.iter().all(|p| p.at.y == 0.0));
        assert_eq!(skipped, Skipped { ground: 1, steep: 1, crowded: 0 });

        let steep_ok = ScatterDef { max_slope: 50.0, ..def.clone() };
        assert_eq!(settle(&steep_ok, Vec3::ZERO, &points, &Fake).0.len(), 4, "a 45° slope under 50° is fine");
        let roomy = ScatterDef { clearance: 1.0, ..def.clone() };
        let (placed, skipped) = settle(&roomy, Vec3::ZERO, &points, &Fake);
        assert_eq!((placed.len(), skipped.crowded), (2, 1), "1.5 m from the column's middle is too close with 1 m of room");
        let raised = ScatterDef { height: (3.5, 4.5), ..def };
        let (placed, _) = settle(&raised, Vec3::ZERO, &points, &Fake);
        assert_eq!(placed.iter().map(|p| p.at).collect::<Vec<_>>(), [Vec3::new(0.0, 4.0, 0.0)], "a band at the column's top: only there");
    }
}
