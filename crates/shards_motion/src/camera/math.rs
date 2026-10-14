//! The camera's math as plain functions: where the orbit puts the camera, how a wall
//! pulls it in, which target a lock-on picks and where it switches to, how a shake moves
//! it. No ECS and no window, so each is unit-tested headless.
//!
//! Conventions: yaw turns the camera about +Y (0: behind a character facing -Z, camera on
//! +Z; positive yaw moves the camera toward +X, which turns the view to the left); pitch
//! positive puts the camera above, looking down.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

use crate::physics::{LayerMask, PhysicsWorld, QueryFilter, Shape};

/// Unit vector from the focus point to the camera.
pub fn orbit_offset(yaw: f32, pitch: f32) -> Vec3 {
    Quat::from_rotation_y(yaw) * Quat::from_rotation_x(-pitch) * Vec3::Z
}

/// Where the camera sits when orbiting `focus` at `distance`.
pub fn orbit_position(focus: Vec3, yaw: f32, pitch: f32, distance: f32) -> Vec3 {
    focus + orbit_offset(yaw, pitch) * distance
}

/// The yaw and pitch of a camera at `focus + offset` that looks at `focus`, and the
/// inverse of [`orbit_offset`].
pub fn orbit_angles(offset: Vec3) -> (f32, f32) {
    let offset = offset.normalize_or(Vec3::Z);
    (offset.x.atan2(offset.z), offset.y.clamp(-1.0, 1.0).asin())
}

/// Pulls the camera in when a wall stands between the focus and `desired`: a sphere of
/// `radius` is swept from `focus` toward `desired` against the world layer, and the camera
/// stops where it touches, never closer to the focus than `min_distance`. `exclude`
/// colliders (the followed character's) never block.
pub fn resolve_camera_collision(
    physics: &PhysicsWorld,
    focus: Vec3,
    desired: Vec3,
    radius: f32,
    min_distance: f32,
    exclude: &[Entity],
) -> Vec3 {
    let delta = desired - focus;
    let distance = delta.length();
    let Ok(dir) = Dir3::new(delta) else {
        return desired;
    };
    let filter = exclude.iter().fold(QueryFilter::new(LayerMask::WORLD), |f, e| f.excluding(*e));
    match physics.shapecast(&Shape::sphere(radius), focus, Quat::IDENTITY, dir, distance, &filter) {
        Some(hit) => focus + *dir * hit.distance.max(min_distance.min(distance)),
        None => desired,
    }
}

/// An entity a lock-on can pick, by whatever key the caller uses (`AiId` bits).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockCandidate {
    pub key: u32,
    pub position: Vec3,
}

/// Where the lock-on looks from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LockView {
    pub eye: Vec3,
    /// The view direction.
    pub forward: Vec3,
    /// Candidates farther than this are out of reach (m).
    pub range: f32,
    /// Half the width of the cone around `forward` that candidates must lie in (degrees).
    pub half_angle_deg: f32,
}

impl LockView {
    fn angle_to(&self, position: Vec3) -> Option<f32> {
        let to = position - self.eye;
        (to.length_squared() > 1e-6).then(|| self.forward.angle_between(to))
    }

    fn reaches(&self, position: Vec3) -> bool {
        self.angle_to(position).is_some_and(|a| a <= self.half_angle_deg.to_radians())
            && position.distance(self.eye) <= self.range
    }
}

/// The candidate to lock on to: inside the cone and range, the one nearest the screen
/// center, a nearer one winning a near tie, then the lower key. Returns its index.
pub fn pick_lock_target(view: &LockView, candidates: &[LockCandidate]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| view.reaches(c.position))
        .min_by(|(_, a), (_, b)| {
            let score = |c: &LockCandidate| view.angle_to(c.position).unwrap_or(PI) + c.position.distance(view.eye) * 0.004;
            score(a).total_cmp(&score(b)).then(a.key.cmp(&b.key))
        })
        .map(|(i, _)| i)
}

/// Which way a lock-on switches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// Signed bearing of `position` from the view's forward on the ground plane: positive to
/// the left.
fn bearing(view: &LockView, position: Vec3) -> f32 {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let (forward, to) = (flat(view.forward), flat(position - view.eye));
    forward.cross(to).y.atan2(forward.dot(to))
}

/// The next target to the `side` of the current one (`current` is an index into
/// `candidates`): the candidate in reach with the nearest bearing past it. `None` when
/// there is nobody on that side.
pub fn next_lock_target(view: &LockView, candidates: &[LockCandidate], current: usize, side: Side) -> Option<usize> {
    let here = bearing(view, candidates.get(current)?.position);
    // A wider cone than picking: a flick should find what is beside the screen, too.
    let wide = LockView { half_angle_deg: (view.half_angle_deg * 1.6).min(179.0), ..*view };
    candidates
        .iter()
        .enumerate()
        .filter(|(i, c)| *i != current && wide.reaches(c.position))
        .map(|(i, c)| (i, bearing(view, c.position) - here, c.key))
        .filter(|(_, delta, _)| match side {
            Side::Left => *delta > 1e-4,
            Side::Right => *delta < -1e-4,
        })
        .min_by(|a, b| a.1.abs().total_cmp(&b.1.abs()).then(a.2.cmp(&b.2)))
        .map(|(i, ..)| i)
}

/// Whether a lock-on still holds: the target is within `release_factor` times the pick
/// range (a lock lets go later than it picks, so it does not flicker at the edge).
pub fn lock_holds(view: &LockView, position: Vec3, release_factor: f32) -> bool {
    position.distance(view.eye) <= view.range * release_factor
}

/// What a shake does to the camera: a shift (m) and a turn (pitch, yaw, roll in radians).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ShakeOffset {
    pub translation: Vec3,
    pub rotation: Vec3,
}

/// A smooth, repeatable wobble: `trauma` (0..1) is squared so small hits stay subtle,
/// `t` is time in seconds. The same inputs give the same offset.
pub fn shake_offset(trauma: f32, t: f32) -> ShakeOffset {
    let amount = trauma.clamp(0.0, 1.0).powi(2);
    if amount == 0.0 {
        return ShakeOffset::default();
    }
    let noise = |channel: f32| {
        let phase = channel * 12.9898;
        0.5 * ((TAU * 7.3 * t + phase).sin() + (TAU * 11.7 * t + phase * 1.7).sin())
    };
    ShakeOffset {
        translation: Vec3::new(noise(1.0), noise(2.0), noise(3.0)) * 0.12 * amount,
        rotation: Vec3::new(noise(4.0), noise(5.0), noise(6.0)) * Vec3::new(2.0, 2.0, 3.0).map(f32::to_radians) * amount,
    }
}

/// Half the width of the cone around the crosshair in which aim assist works (radians).
pub const ASSIST_CONE: f32 = 0.18;

/// Bends a look (`look`: yaw and pitch change this frame) toward a target near the
/// crosshair. `error`: the yaw and pitch still to turn to center the target. The bend is as
/// strong as `strength` (0..1) says, grows as the target nears the crosshair, and only
/// helps while the player is turning: it never moves a still camera, and never turns past
/// the target.
pub fn aim_assist(look: Vec2, error: Vec2, strength: f32) -> Vec2 {
    let distance = error.length();
    if strength <= 0.0 || distance < 1e-6 || look.length_squared() < 1e-12 {
        return look;
    }
    let near = (1.0 - distance / ASSIST_CONE).clamp(0.0, 1.0);
    let bend = error * near * strength.clamp(0.0, 1.0) * (look.length() / distance).min(1.0);
    look + bend
}

/// Moves `current` toward `target` by an exponential catch-up of `rate` per second:
/// frame-rate independent.
pub fn damp(current: f32, target: f32, rate: f32, dt: f32) -> f32 {
    current + (target - current) * (1.0 - (-rate * dt).exp())
}

/// `angle` turned into -π..π.
pub fn wrap_angle(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Collider;

    #[test]
    fn the_orbit_puts_the_camera_behind_and_above() {
        assert!((orbit_position(Vec3::Y, 0.0, 0.0, 5.0) - Vec3::new(0.0, 1.0, 5.0)).length() < 1e-5);
        let up = orbit_position(Vec3::ZERO, 0.0, std::f32::consts::FRAC_PI_2, 4.0);
        assert!((up - Vec3::Y * 4.0).length() < 1e-5, "looking straight down from above: {up}");
        // A quarter turn moves the camera to +X.
        let side = orbit_position(Vec3::ZERO, std::f32::consts::FRAC_PI_2, 0.0, 3.0);
        assert!((side - Vec3::X * 3.0).length() < 1e-5, "{side}");
        // The angles read back.
        let (yaw, pitch) = orbit_angles(orbit_offset(0.7, 0.3));
        assert!((yaw - 0.7).abs() < 1e-5 && (pitch - 0.3).abs() < 1e-5);
    }

    fn wall_at(z: f32) -> PhysicsWorld {
        let mut physics = PhysicsWorld::default();
        let collider = Collider::new(Shape::cuboid(Vec3::new(10.0, 10.0, 0.5)));
        let tf = Transform::from_xyz(0.0, 1.0, z);
        physics.upsert((Entity::from_raw_u32(1).unwrap(), &crate::physics::ColliderId(1), &collider, &tf, None, None));
        physics
    }

    #[test]
    fn a_wall_pulls_the_camera_in_and_nothing_else_does() {
        let physics = wall_at(4.0);
        let focus = Vec3::new(0.0, 1.5, 0.0);
        let free = resolve_camera_collision(&physics, focus, Vec3::new(0.0, 1.5, -5.0), 0.2, 0.5, &[]);
        assert_eq!(free, Vec3::new(0.0, 1.5, -5.0), "nothing behind the focus on that side");
        let pulled = resolve_camera_collision(&physics, focus, Vec3::new(0.0, 1.5, 6.0), 0.2, 0.5, &[]);
        // The wall's face is at z = 3.5; the camera sphere stops 0.2 short of it.
        assert!((pulled.z - 3.3).abs() < 0.02 && pulled.x == 0.0, "{pulled}");
        // Never inside the head, even with a wall right behind it.
        let tight = resolve_camera_collision(&wall_at(1.0), focus, Vec3::new(0.0, 1.5, 6.0), 0.2, 0.5, &[]);
        assert!(tight.z >= 0.5 - 1e-4, "{tight}");
        // The followed character's own collider is excluded.
        let physics = wall_at(4.0);
        let through = resolve_camera_collision(&physics, focus, Vec3::new(0.0, 1.5, 6.0), 0.2, 0.5, &[Entity::from_raw_u32(1).unwrap()]);
        assert_eq!(through.z, 6.0);
    }

    fn view() -> LockView {
        LockView { eye: Vec3::new(0.0, 1.5, 0.0), forward: Vec3::NEG_Z, range: 20.0, half_angle_deg: 50.0 }
    }

    fn at(key: u32, x: f32, z: f32) -> LockCandidate {
        LockCandidate { key, position: Vec3::new(x, 0.0, z) }
    }

    #[test]
    fn the_lock_on_picks_what_is_nearest_the_center_in_range() {
        let candidates = [at(1, 6.0, -10.0), at(2, 0.5, -15.0), at(3, 0.0, -30.0), at(4, 0.0, 5.0)];
        // 1 is off to the side, 2 almost dead ahead, 3 out of range, 4 behind.
        assert_eq!(pick_lock_target(&view(), &candidates), Some(1));
        assert_eq!(pick_lock_target(&view(), &candidates[2..]), None);
        // The same score picks the lower key, whatever the order.
        let twins = [at(9, 3.0, -10.0), at(5, -3.0, -10.0)];
        assert_eq!(pick_lock_target(&view(), &twins), Some(1));
        assert_eq!(pick_lock_target(&view(), &[]), None);
    }

    #[test]
    fn a_flick_switches_to_the_neighbour_on_that_side() {
        let candidates = [at(1, -8.0, -10.0), at(2, -2.0, -10.0), at(3, 3.0, -10.0), at(4, 9.0, -10.0)];
        // From the second (left of center): left is the first, right is the third.
        assert_eq!(next_lock_target(&view(), &candidates, 1, Side::Left), Some(0));
        assert_eq!(next_lock_target(&view(), &candidates, 1, Side::Right), Some(2));
        // From the far left there is nobody further left.
        assert_eq!(next_lock_target(&view(), &candidates, 0, Side::Left), None);
        assert_eq!(next_lock_target(&view(), &candidates, 3, Side::Right), None);
    }

    #[test]
    fn a_lock_lets_go_further_out_than_it_picks() {
        let v = view();
        assert!(lock_holds(&v, Vec3::new(0.0, 0.0, -24.0), 1.3));
        assert!(!lock_holds(&v, Vec3::new(0.0, 0.0, -27.0), 1.3));
    }

    #[test]
    fn a_shake_is_bounded_repeatable_and_off_without_trauma() {
        assert_eq!(shake_offset(0.0, 1.23), ShakeOffset::default());
        let a = shake_offset(1.0, 0.37);
        assert_eq!(a, shake_offset(1.0, 0.37));
        assert!(a.translation.abs().max_element() <= 0.12 + 1e-6 && a.rotation.abs().max_element() <= 3f32.to_radians() + 1e-6);
        let small = shake_offset(0.3, 0.37);
        assert!(small.translation.length() < a.translation.length(), "trauma squared: a small hit is subtle");
        assert_ne!(shake_offset(1.0, 0.1), shake_offset(1.0, 0.2), "it moves over time");
    }

    #[test]
    fn aim_assist_bends_a_look_toward_a_near_target_by_its_strength() {
        let error = Vec2::new(0.06, 0.0);
        let look = Vec2::new(0.1, 0.0);
        assert_eq!(aim_assist(look, error, 0.0), look, "off");
        let half = aim_assist(look, error, 0.5);
        let full = aim_assist(look, error, 1.0);
        assert!(half.x > look.x && full.x > half.x, "stronger assist bends more: {half} {full}");
        assert!(aim_assist(Vec2::ZERO, error, 1.0) == Vec2::ZERO, "a still camera stays still");
        // A target far from the crosshair is left alone.
        let far = Vec2::new(0.5, 0.0);
        assert_eq!(aim_assist(look, far, 1.0), look);
        // Looking only a little never turns past the target.
        let slow = aim_assist(Vec2::new(0.01, 0.0), error, 1.0);
        assert!(slow.x < error.x, "{slow}");
    }

    #[test]
    fn damping_is_frame_rate_independent() {
        let one = damp(0.0, 1.0, 5.0, 0.1);
        let two = damp(damp(0.0, 1.0, 5.0, 0.05), 1.0, 5.0, 0.05);
        assert!((one - two).abs() < 1e-5);
        assert!((wrap_angle(3.0 * PI) - PI).abs() < 1e-5 || (wrap_angle(3.0 * PI) + PI).abs() < 1e-5);
    }
}
