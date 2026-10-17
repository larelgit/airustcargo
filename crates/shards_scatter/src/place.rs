//! Where a scatter's instances go on one tile. Points are thrown at random in the tile's square,
//! always the same ones (the region's seed, its id, the tile's cell and the scatter's id key the
//! random numbers, never the simulation's), each kept only if it is `spacing` away from those
//! before it. Each throw draws the same numbers whether it is kept or not, so the list at full
//! density is fixed, and the player's `graphics.foliage` keeps its first part: a thinner scatter
//! is the same scatter with fewer instances. Then each instance is dropped onto the ground
//! straight below the top of its `height` band: none where that ground is too steep, out of the
//! band (a column's top), inside a solid (a wall) or too close to one (`clearance`, a ball on the
//! ground along its normal).
//!
//! Nothing here touches the world: [`Ground`] answers the rays, and the physics world is one;
//! the strewing asks [`solids`], the static colliders around one tile.

use std::collections::HashMap;
use std::f32::consts::TAU;

use ai_core::DetRng;
use bevy::prelude::*;
use shards_motion::{Body, LayerMask, PhysicsWorld, QueryFilter, Shape};

use crate::def::{ScatterDef, MAX_TILE_ENTITIES, MIN_SPACING};

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

/// How many instances the scatter asks for on a tile `extent` meters across (X, Z) at full
/// density: `density × area`, before [`most`] caps it.
pub fn wanted(def: &ScatterDef, extent: Vec2) -> usize {
    let wanted = f64::from(def.density.max(0.0)) * f64::from(extent.x) * f64::from(extent.y);
    // Saturating: NaN is 0, too many is `usize::MAX`.
    wanted.round() as usize
}

/// The most instances of the scatter on one tile: [`MAX_TILE_ENTITIES`] mesh entities with its
/// levels of detail.
pub fn most(def: &ScatterDef) -> usize {
    MAX_TILE_ENTITIES / def.lods.len().max(1)
}

/// The scatter's instances on a tile `extent` meters across (X, Z) at full density, in the order
/// they were thrown: `density × area` of them, at most [`most`], fewer when `spacing` leaves no
/// room (four throws per instance at most).
pub fn candidates(def: &ScatterDef, rng: &mut DetRng, extent: Vec2) -> Vec<Candidate> {
    let wanted = wanted(def, extent).min(most(def));
    if wanted == 0 || def.looks.is_empty() {
        return Vec::new();
    }
    let (small, large) = def.scale;
    let tilt = def.tilt.to_radians();
    let looks = def.looks.len() as u32;
    // Content keeps it 0 or a centimeter at least (a smaller one is an error, read as that).
    let spacing = if def.spacing > 0.0 { def.spacing.max(MIN_SPACING) } else { 0.0 };
    // Instances by cell of `spacing`: lookups only, never iterated.
    let mut near: HashMap<(i32, i32), Vec<Vec2>> = HashMap::new();
    // Float to int saturates; so do the neighbours below.
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
            let crowded = (cx.saturating_sub(1)..=cx.saturating_add(1))
                .flat_map(|x| (cz.saturating_sub(1)..=cz.saturating_add(1)).map(move |z| (x, z)))
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

/// The colliders of a physics world in [`LayerMask::WORLD`] (not characters, not volumes); the
/// strewing asks the part [`solids`] keeps.
impl Ground for PhysicsWorld {
    fn down(&self, from: Vec3, max: f32) -> Option<(f32, Vec3, Vec3)> {
        let hit = self.raycast(from, Dir3::NEG_Y, max, &QueryFilter::new(LayerMask::WORLD))?;
        Some((hit.distance, hit.point, hit.normal))
    }

    fn blocked(&self, at: Vec3, radius: f32) -> bool {
        !self.overlaps(&Shape::Sphere { radius }, at, Quat::IDENTITY, &QueryFilter::new(LayerMask::WORLD)).is_empty()
    }
}

/// What a scatter's instances on a tile centered at `center`, `extent` meters across (X, Z), can
/// stand on or bump into: the static colliders of the world ([`Body::Static`], in
/// [`LayerMask::WORLD`], no volume) that reach into the tile's column, where its rays and
/// clearance checks go. A collider that moves (a platform, a door on rails) is never ground nor a
/// solid: an instance would stay where it no longer is. And the queries scan the few colliders of
/// the tile, not the whole world's.
pub fn solids(physics: &PhysicsWorld, def: &ScatterDef, center: Vec3, extent: Vec2) -> PhysicsWorld {
    let (low, high) = def.height;
    // A clearance ball touches what is within twice its radius (and the 5 cm under it) of the
    // ground; the rays go 5 cm past the band.
    let pad = 2.0 * def.clearance.max(0.0) + 0.1;
    let half = extent * 0.5;
    let min = Vec3::new(center.x - half.x - pad, center.y + low - pad, center.z - half.y - pad);
    let max = Vec3::new(center.x + half.x + pad, center.y + high + pad, center.z + half.y + pad);
    physics.subset(|c| {
        c.body == Body::Static
            && c.volume.is_none()
            && c.layers.memberships.intersects(LayerMask::WORLD)
            && c.aabb_min.cmple(max).all()
            && min.cmple(c.aabb_max).all()
    })
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
        // The ball sits on the ground along its normal: on a slope it clears the slope itself
        // (straight up, it would sink into the slope above anything steeper than ~20°).
        let up = normal.try_normalize().unwrap_or(Vec3::Y);
        if def.clearance > 0.0 && ground.blocked(point + up * (def.clearance + 0.05), def.clearance) {
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

    #[test]
    fn a_tiny_spacing_and_a_dense_scatter_stay_within_bounds() {
        // Below a centimeter (an error in content) it is read as one: no overflow, no crowd of
        // instances in one saturated cell.
        let tiny = scatter(1.0, 1e-9);
        let thrown = thrown(&tiny, 7, (0, 0));
        assert_eq!(thrown.len(), 256, "a centimeter apart, all of them fit");
        // At most MAX_TILE_ENTITIES mesh entities a tile: the first instances of the same list.
        let mut dense = scatter(20.0, 0.0);
        dense.lods = vec![crate::def::ScatterLod { mesh: crate::def::ScatterMesh::Box { size: Vec3::ONE }, until: 10.0 }; 4];
        let big = Vec2::splat(64.0);
        assert_eq!((wanted(&dense, big), most(&dense)), (81920, 2500));
        let capped = candidates(&dense, &mut rng_for(1, "meadow", (0, 0), "grass"), big);
        assert_eq!(capped.len(), 2500);
        let fewer = ScatterDef { density: 0.5, ..dense.clone() };
        let first = candidates(&fewer, &mut rng_for(1, "meadow", (0, 0), "grass"), big);
        assert_eq!(first.len(), 2048);
        assert_eq!(first[..10], capped[..10], "the same throws");
        let broken = ScatterDef { density: f32::INFINITY, ..dense.clone() };
        assert_eq!((wanted(&broken, big), candidates(&broken, &mut rng_for(1, "m", (0, 0), "g"), big).len()), (usize::MAX, 2500));
        assert_eq!(wanted(&ScatterDef { density: f32::NAN, ..dense }, big), 0);
    }

    /// A physics world of `colliders` (shape, pose, body), synced as the engine syncs it.
    fn physics(colliders: &[(Shape, Transform, Body)]) -> PhysicsWorld {
        let mut world = World::new();
        world.init_resource::<PhysicsWorld>();
        world.init_resource::<shards_motion::ColliderIds>();
        for (shape, tf, body) in colliders {
            world.spawn((shards_motion::Collider::new(*shape), *tf, *body));
        }
        world.run_system_cached(shards_motion::physics::sync_physics).unwrap();
        world.remove_resource::<PhysicsWorld>().unwrap()
    }

    #[test]
    fn clearance_is_room_above_the_slope_underfoot_and_walls_still_take_it() {
        // Ground 25° steep rising toward +x (its surface 5 cm over the origin), and bushes that
        // need 0.7 m of room: straight up, a ball that size sinks into any slope over ~21°.
        let slope = (Shape::cuboid(Vec3::new(10.0, 0.5, 10.0)), Transform::from_xyz(0.0, -0.5, 0.0).with_rotation(Quat::from_rotation_z(25f32.to_radians())), Body::Static);
        let def = ScatterDef { looks: vec!["a".into()], clearance: 0.7, height: (-2.0, 2.0), ..ScatterDef::default() };
        let at = |x: f32, z: f32| Candidate { offset: Vec2::new(x, z), yaw: 0.0, lean: (0.0, 0.0), scale: 1.0, look: 0 };
        let (placed, skipped) = settle(&def, Vec3::ZERO, &[at(0.0, 0.0), at(0.0, 3.0)], &physics(&[slope]));
        assert_eq!((placed.len(), skipped), (2, Skipped::default()), "the slope is ground, not a wall");
        assert!((placed[0].at.y - 0.05).abs() < 0.01, "{:?}", placed[0]);
        // A wall along the slope, 0.5 m to the side of the second: too close for it.
        let wall = (Shape::cuboid(Vec3::new(5.0, 3.0, 0.1)), Transform::from_xyz(0.0, 0.0, 3.6), Body::Static);
        let (placed, skipped) = settle(&def, Vec3::ZERO, &[at(0.0, 0.0), at(0.0, 3.0)], &physics(&[slope, wall]));
        assert_eq!((placed.len(), skipped.crowded), (1, 1));
        assert_eq!(placed[0].at.z, 0.0);
        // Steeper than max_slope: none, whatever the room.
        let steep = (slope.0, Transform::from_xyz(0.0, -0.5, 0.0).with_rotation(Quat::from_rotation_z(35f32.to_radians())), Body::Static);
        assert_eq!(settle(&def, Vec3::ZERO, &[at(0.0, 0.0)], &physics(&[steep])).1.steep, 1);
    }

    #[test]
    fn only_static_colliders_near_the_tile_are_its_solids() {
        let floor = (Shape::cuboid(Vec3::new(8.0, 0.5, 8.0)), Transform::from_xyz(0.0, -0.5, 0.0), Body::Static);
        // A platform over the middle, in the height band: it moves, so it is neither ground nor
        // in the way. The same slab static is ground.
        let slab = |body| (Shape::cuboid(Vec3::new(2.0, 0.05, 2.0)), Transform::from_xyz(0.0, 0.3, 0.0), body);
        let far = (Shape::cuboid(Vec3::ONE), Transform::from_xyz(40.0, 0.0, 0.0), Body::Static);
        let def = ScatterDef { looks: vec!["a".into()], clearance: 0.5, ..ScatterDef::default() };
        let extent = Vec2::splat(16.0);
        let thrown = candidates(&def, &mut rng_for(3, "meadow", (0, 0), "grass"), extent);
        let strew = |colliders: &[(Shape, Transform, Body)]| {
            let solids = solids(&physics(colliders), &def, Vec3::ZERO, extent);
            (solids.len(), settle(&def, Vec3::ZERO, &thrown, &solids))
        };
        let (n, alone) = strew(&[floor]);
        assert_eq!(n, 1);
        let (n, moving) = strew(&[floor, slab(Body::Kinematic), far]);
        assert_eq!(n, 1, "neither the platform nor what is 40 m away");
        assert_eq!(moving, alone, "a moving collider changes nothing");
        let (n, still) = strew(&[floor, slab(Body::Static), far]);
        assert_eq!(n, 2);
        let on_slab = still.0.iter().filter(|p| (p.at.y - 0.35).abs() < 1e-3).count();
        assert!(on_slab > 0 && still.0 != alone.0, "the static slab is ground: {on_slab} on it");
    }
}
