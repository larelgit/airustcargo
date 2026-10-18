//! The camera's math as plain functions: where the orbit puts the camera, how a wall
//! pulls it in, which target a lock-on picks and where it switches to, how a shake moves
//! it. No ECS and no window, so each is unit-tested headless.
//!
//! Conventions: yaw turns the camera about +Y (0: behind a character facing -Z, camera on
//! +Z; positive yaw moves the camera toward +X, which turns the view to the left); pitch
//! positive puts the camera above, looking down.

use std::f32::consts::{PI, TAU};

use bevy::math::FloatPow;
use bevy::prelude::*;

use super::tuning::{ArmTuning, CameraOffsets, FlickTuning, FramingTuning, TraumaTuning};
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
    (ops::atan2(offset.x, offset.z), ops::asin(offset.y.clamp(-1.0, 1.0)))
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// The side a `lock_switch` press names in `dir.x`: right when positive, left when
    /// negative, none at 0 (or not a number).
    pub fn from_x(x: f32) -> Option<Side> {
        if x > 0.0 {
            Some(Side::Right)
        } else if x < 0.0 {
            Some(Side::Left)
        } else {
            None
        }
    }

    /// The `dir.x` of a `lock_switch` press for this side: `+1` right, `-1` left.
    pub fn x(self) -> f32 {
        match self {
            Side::Left => -1.0,
            Side::Right => 1.0,
        }
    }

    /// `left` or `right`.
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// Signed bearing of `position` from the view's forward on the ground plane: positive to
/// the left.
fn bearing(view: &LockView, position: Vec3) -> f32 {
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let (forward, to) = (flat(view.forward), flat(position - view.eye));
    ops::atan2(forward.cross(to).y, forward.dot(to))
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
    shake_offset_with(trauma, t, &TraumaTuning::default())
}

/// [`shake_offset`] with the amplitudes of `tuning` (its `decay` is the caller's business).
pub fn shake_offset_with(trauma: f32, t: f32, tuning: &TraumaTuning) -> ShakeOffset {
    let amount = trauma.clamp(0.0, 1.0).squared();
    if amount == 0.0 {
        return ShakeOffset::default();
    }
    let noise = |channel: f32| {
        let phase = channel * 12.9898;
        0.5 * (ops::sin(TAU * 7.3 * t + phase) + ops::sin(TAU * 11.7 * t + phase * 1.7))
    };
    ShakeOffset {
        translation: Vec3::new(noise(1.0), noise(2.0), noise(3.0)) * tuning.shift_m * amount,
        rotation: Vec3::new(noise(4.0), noise(5.0), noise(6.0))
            * Vec3::new(tuning.pitch_deg, tuning.yaw_deg, tuning.roll_deg).map(f32::to_radians)
            * amount,
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
    current + (target - current) * (1.0 - ops::exp(-rate * dt))
}

/// One step of a critically damped spring: `x` (with velocity `v`) settles on `target` at
/// angular rate `omega` (1/s, no overshoot). The closed form, so it is exact for any `dt` and
/// frame-rate independent: one step of `2 dt` is the same as two of `dt`. A landing dip is
/// this with `target` 0 and a kick added to `v`. Returns the new `(x, v)`.
pub fn critically_damped_step(x: f32, v: f32, target: f32, omega: f32, dt: f32) -> (f32, f32) {
    let d = x - target;
    let c = v + omega * d;
    let decay = ops::exp(-omega * dt);
    (target + (d + c * dt) * decay, (v - omega * c * dt) * decay)
}

/// The tilt range in force: the tuning's `base` (lowest, highest) narrowed by the game's
/// [`CameraOffsets`]. An override outside `base` is held to it, so an override only narrows,
/// and the lowest never ends above the highest.
pub fn pitch_limits(base: (f32, f32), offsets: &CameraOffsets) -> (f32, f32) {
    let (lo, hi) = (base.0.min(base.1), base.0.max(base.1));
    let pick = |over: Option<f32>, default: f32| over.filter(|v| v.is_finite()).map_or(default, |v| v.clamp(lo, hi));
    let low = pick(offsets.pitch_min, lo);
    (low, pick(offsets.pitch_max, hi).max(low))
}

/// Turns a locked-on camera toward framing `target`: behind the line from the target to
/// `focus`, the yaw and the pitch each catching up at their own exact exponential rate over
/// `dt`. A target nearer than `min_horizontal` (on the ground, from `focus`) leaves the camera
/// as it is. Returns the new `(yaw, pitch)`; the caller clamps the pitch to its limits.
pub fn lock_framing(yaw: f32, pitch: f32, focus: Vec3, target: Vec3, framing: &FramingTuning, dt: f32) -> (f32, f32) {
    let offset = focus - target;
    if Vec2::new(offset.x, offset.z).length() < framing.min_horizontal {
        return (yaw, pitch);
    }
    let (want_yaw, line_pitch) = orbit_angles(offset);
    let (clamp_lo, clamp_hi) = framing.pitch_clamp;
    let want_pitch = framing.pitch_target.unwrap_or_else(|| line_pitch.clamp(clamp_lo, clamp_hi.max(clamp_lo)));
    (
        wrap_angle(yaw + wrap_angle(want_yaw - yaw) * (1.0 - ops::exp(-framing.yaw_rate * dt))),
        damp(pitch, want_pitch, framing.pitch_rate, dt),
    )
}

/// The nominal length of the arm after a frame: it follows `wanted` (the camera's `distance`)
/// at `arm.follow_rate`, or is `wanted` at once without one (or before it has a length).
pub fn arm_length(current: f32, wanted: f32, arm: &ArmTuning, dt: f32) -> f32 {
    match arm.follow_rate {
        Some(rate) if current > 0.0 => damp(current, wanted, rate, dt),
        _ => wanted,
    }
}

/// How far out the camera stands this frame. `nominal` is the arm's length this frame and
/// `previous_nominal` last frame (both after `arm_length`'s own smoothing); `free` is how far
/// it could stand along the arm (`nominal` when nothing is in the way, else where the wall is);
/// `previous` is where it stood last frame (0: nowhere yet). A wall shortens the arm at once;
/// once it lets go, the shortfall below the nominal length shrinks at `arm.follow_rate` (at
/// once without one). The rate acts on the shortfall, not on the length, so a plain zoom is
/// smoothed once (by `arm_length`) and a wall release is frame-rate independent. Never past
/// `free`, so never into a wall, and never closer than `arm.min_distance` (as far as `free`
/// allows).
pub fn arm_reach(previous: f32, previous_nominal: f32, nominal: f32, free: f32, arm: &ArmTuning, dt: f32) -> f32 {
    let reach = match arm.follow_rate {
        Some(rate) if previous > 0.0 => {
            let wall = (nominal - free).max(0.0);
            let kept = (previous_nominal - previous).max(0.0) * ops::exp(-rate * dt);
            nominal - wall.max(kept)
        }
        _ => free,
    };
    reach.max(arm.min_distance.min(free)).min(free)
}

/// What the look flick has accumulated, for switching the lock-on target. Pure state: the
/// system feeds it the mouse's pixels, the stick's deflection and the frame times, and acts
/// on what it returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlickState {
    /// Horizontal mouse motion summed so far (px, positive right).
    pub accum_px: f32,
    /// Seconds of the simulation clock left before the next switch.
    pub cooldown: f32,
    /// Whether the stick has come back below its re-arm level since its last switch.
    pub stick_ready: bool,
    /// Whether the mouse wheel has stood still for a frame since its last switch.
    pub wheel_ready: bool,
}

impl Default for FlickState {
    fn default() -> Self {
        Self { accum_px: 0.0, cooldown: 0.0, stick_ready: true, wheel_ready: true }
    }
}

impl FlickState {
    /// Runs the cooldown down by `sim_dt`, the frame's time on the simulation clock (0 while
    /// the game is paused).
    pub fn tick(&mut self, sim_dt: f32) {
        self.cooldown = (self.cooldown - sim_dt).max(0.0);
    }

    /// The mouse moved `px` horizontally (right is positive) over a frame of `frame_dt` real
    /// seconds. Unlocked, the sum is dropped. Locked, the sum drains by the tuning's decay (or
    /// is dropped every frame without one), `px` is added, and a sum past the threshold with
    /// the cooldown over is a flick: the side comes back and the sum starts again. The sum
    /// keeps adding while the cooldown blocks a switch.
    pub fn mouse(&mut self, flick: &FlickTuning, px: f32, frame_dt: f32, locked: bool) -> Option<Side> {
        if !locked {
            self.accum_px = 0.0;
            return None;
        }
        let drained = match flick.mouse_decay_px_s {
            Some(decay) => self.accum_px.signum() * (self.accum_px.abs() - decay * frame_dt).max(0.0),
            None => 0.0,
        };
        self.accum_px = drained + px;
        if self.cooldown > 0.0 || self.accum_px.abs() < flick.mouse_threshold_px {
            return None;
        }
        let side = if self.accum_px > 0.0 { Side::Right } else { Side::Left };
        self.accum_px = 0.0;
        Some(side)
    }

    /// The right stick is deflected `x` sideways (right is positive, -1..1). Called every frame
    /// so the stick can re-arm: it must come back below `stick_rearm` after a switch (no such
    /// rule without one). A deflection past the threshold on a locked camera with the
    /// cooldown over is a flick.
    pub fn stick(&mut self, flick: &FlickTuning, x: f32, locked: bool) -> Option<Side> {
        if flick.stick_rearm.is_some_and(|rearm| x.abs() < rearm) {
            self.stick_ready = true;
        }
        let ready = flick.stick_rearm.is_none() || self.stick_ready;
        if !locked || !ready || self.cooldown > 0.0 || x.abs() < flick.stick_threshold {
            return None;
        }
        self.stick_ready = false;
        Some(if x > 0.0 { Side::Right } else { Side::Left })
    }

    /// The mouse wheel turned `notch` this frame (right is positive, 0 while it stands still):
    /// one switch per turn of the wheel. After a switch it must stand still for a frame before
    /// it can switch again, so a touchpad's long or coasting scroll is one switch, not one per
    /// cooldown. A turn on a locked camera with the cooldown over switches.
    pub fn wheel(&mut self, notch: f32, locked: bool) -> Option<Side> {
        if notch == 0.0 {
            self.wheel_ready = true;
            return None;
        }
        if !locked || !self.wheel_ready || self.cooldown > 0.0 {
            return None;
        }
        self.wheel_ready = false;
        Side::from_x(notch)
    }

    /// A lock was just taken from nothing, with the stick at `x`: a stick already pushed over
    /// cannot flick the fresh target (the click that locked on came with it) until it comes
    /// back below its re-arm level.
    pub fn lock_taken(&mut self, flick: &FlickTuning, x: f32) {
        if let Some(rearm) = flick.stick_rearm {
            self.stick_ready = x.abs() < rearm;
        }
    }

    /// A flick was acted on: `switched` says whether it found a target to switch to. The
    /// cooldown starts after a switch, and after a miss only if the tuning says so.
    pub fn flicked(&mut self, flick: &FlickTuning, switched: bool) {
        if switched || flick.cooldown_on_miss {
            self.cooldown = flick.cooldown_s;
        }
    }
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

    #[test]
    fn a_critically_damped_step_is_exact_for_any_frame_rate() {
        let (omega, target) = (12.5, 0.3);
        let (x, v) = (-0.12, 0.8);
        let (x1, v1) = critically_damped_step(x, v, target, omega, 0.032);
        let (xa, va) = critically_damped_step(x, v, target, omega, 0.016);
        let (x2, v2) = critically_damped_step(xa, va, target, omega, 0.016);
        assert!((x1 - x2).abs() < 1e-5 && (v1 - v2).abs() < 1e-5, "{x1} {x2} {v1} {v2}");
        // Eight frames of 4 ms are one frame of 32 ms.
        let (mut xs, mut vs) = (x, v);
        for _ in 0..8 {
            (xs, vs) = critically_damped_step(xs, vs, target, omega, 0.004);
        }
        assert!((xs - x1).abs() < 1e-5 && (vs - v1).abs() < 1e-5);
        // It settles on the target without crossing it, and a zero step changes nothing.
        let (mut x, mut v) = (1.0, 0.0);
        for _ in 0..120 {
            (x, v) = critically_damped_step(x, v, 0.0, 10.0, 1.0 / 60.0);
            assert!(x >= 0.0, "no overshoot from rest: {x}");
        }
        assert!(x < 1e-3 && v.abs() < 1e-2);
        let (x0, v0) = critically_damped_step(0.4, -2.0, 1.0, 9.0, 0.0);
        assert!((x0 - 0.4).abs() < 1e-6 && v0 == -2.0);
    }

    #[test]
    fn a_landing_kick_dips_by_its_depth_at_its_time() {
        // A kick of -depth * omega * e at rest reaches exactly `-depth` after 1/omega seconds:
        // the closed form the dip is built on.
        let (depth, time) = (0.12, 0.08);
        let omega = 1.0 / time;
        let (x, _) = critically_damped_step(0.0, -depth * omega * std::f32::consts::E, 0.0, omega, time);
        assert!((x + depth).abs() < 1e-5, "{x}");
    }

    #[test]
    fn the_pitch_limits_only_narrow_and_never_cross() {
        let base = (-0.45, 1.35);
        assert_eq!(pitch_limits(base, &CameraOffsets::default()), base);
        let swim = CameraOffsets { pitch_max: Some(0.2), ..default() };
        assert_eq!(pitch_limits(base, &swim), (-0.45, 0.2));
        let raised = CameraOffsets { pitch_min: Some(0.0), pitch_max: Some(0.5), ..default() };
        assert_eq!(pitch_limits(base, &raised), (0.0, 0.5));
        // An override outside the range is held to it: a cap below the lowest tilt pins the pitch there.
        let deep = CameraOffsets { pitch_max: Some(-2.0), ..default() };
        assert_eq!(pitch_limits(base, &deep), (-0.45, -0.45));
        let wide = CameraOffsets { pitch_min: Some(-3.0), pitch_max: Some(3.0), ..default() };
        assert_eq!(pitch_limits(base, &wide), base);
        // A lowest above the highest wins; a broken number is ignored.
        let crossed = CameraOffsets { pitch_min: Some(1.0), pitch_max: Some(0.5), ..default() };
        assert_eq!(pitch_limits(base, &crossed), (1.0, 1.0));
        let nan = CameraOffsets { pitch_max: Some(f32::NAN), ..default() };
        assert_eq!(pitch_limits(base, &nan), base);
        // The limits applied to a pitch: a swimming cap holds the stored pitch down.
        let (lo, hi) = pitch_limits(base, &swim);
        assert_eq!(1.0_f32.clamp(lo, hi), 0.2);
    }

    #[test]
    fn lock_framing_catches_up_at_exact_rates_whatever_the_frame_rate() {
        let framing = FramingTuning { yaw_rate: 4.0, pitch_rate: 2.0, pitch_target: Some(0.25), ..default() };
        let (focus, target) = (Vec3::new(0.0, 1.5, 0.0), Vec3::new(10.0, 1.0, 0.0));
        // The camera should stand on the -X side of the focus: yaw -PI/2.
        let (y1, p1) = lock_framing(0.0, 0.0, focus, target, &framing, 0.1);
        let (ya, pa) = lock_framing(0.0, 0.0, focus, target, &framing, 0.05);
        let (y2, p2) = lock_framing(ya, pa, focus, target, &framing, 0.05);
        assert!((y1 - y2).abs() < 1e-5 && (p1 - p2).abs() < 1e-5, "{y1} {y2} {p1} {p2}");
        let want = -std::f32::consts::FRAC_PI_2;
        assert!((y1 - want * (1.0 - ops::exp(-0.4))).abs() < 1e-5, "yaw rate 4/s over 0.1 s: {y1}");
        assert!((p1 - 0.25 * (1.0 - ops::exp(-0.2))).abs() < 1e-5, "pitch rate 2/s toward the fixed target: {p1}");
    }

    #[test]
    fn lock_framing_follows_the_line_to_the_target_within_its_clamp_by_default() {
        let framing = FramingTuning::default();
        // A target far below: the line's pitch is large, held to 0.7.
        let (_, p) = lock_framing(0.0, 0.0, Vec3::new(0.0, 20.0, 0.0), Vec3::new(0.0, 0.0, -2.0), &framing, 1.0);
        assert!((p - 0.7 * (1.0 - ops::exp(-7.0))).abs() < 1e-4, "{p}");
        // Level with it: held up to 0.15.
        let (_, p) = lock_framing(0.0, 0.0, Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, -9.0), &framing, 1.0);
        assert!((p - 0.15 * (1.0 - ops::exp(-7.0))).abs() < 1e-4, "{p}");
    }

    #[test]
    fn lock_framing_leaves_a_target_too_close_alone() {
        let framing = FramingTuning { min_horizontal: 0.5, ..default() };
        let focus = Vec3::new(0.0, 1.5, 0.0);
        assert_eq!(lock_framing(0.3, 0.2, focus, Vec3::new(0.3, 0.0, 0.0), &framing, 0.1), (0.3, 0.2));
        assert_ne!(lock_framing(0.3, 0.2, focus, Vec3::new(0.6, 0.0, 0.0), &framing, 0.1), (0.3, 0.2));
        // By default it always frames, even a target on the focus line.
        assert_ne!(lock_framing(0.3, 0.2, focus, Vec3::new(0.0, 0.0, 0.0), &FramingTuning::default(), 0.1), (0.3, 0.2));
    }

    #[test]
    fn the_arm_shortens_at_once_and_grows_back_at_its_rate() {
        let arm = ArmTuning { follow_rate: Some(6.0), ..default() };
        // A wall: from 5 m to 2 m in one frame (the nominal length stays 5).
        assert_eq!(arm_reach(5.0, 5.0, 5.0, 2.0, &arm, 0.016), 2.0);
        // The wall lets go: the shortfall shrinks at 6/s, the exact exponential.
        let one = arm_reach(2.0, 5.0, 5.0, 5.0, &arm, 0.1);
        assert!((one - (5.0 - 3.0 * ops::exp(-0.6))).abs() < 1e-5, "{one}");
        let half = arm_reach(arm_reach(2.0, 5.0, 5.0, 5.0, &arm, 0.05), 5.0, 5.0, 5.0, &arm, 0.05);
        assert!((one - half).abs() < 1e-5, "frame-rate independent");
        // Never past what is free, and at the end exactly there.
        let mut reach = 2.0;
        for _ in 0..600 {
            reach = arm_reach(reach, 5.0, 5.0, 5.0, &arm, 1.0 / 60.0);
            assert!(reach <= 5.0);
        }
        assert!((reach - 5.0).abs() < 1e-3);
        // A wall that comes back while it is growing stops it at once.
        assert_eq!(arm_reach(3.0, 5.0, 5.0, 2.5, &arm, 0.016), 2.5);
        // Without a rate it grows back at once, as before.
        assert_eq!(arm_reach(2.0, 5.0, 5.0, 5.0, &ArmTuning::default(), 0.016), 5.0);
        // Before it has stood anywhere it stands where it is free.
        assert_eq!(arm_reach(0.0, 0.0, 4.0, 4.0, &arm, 0.016), 4.0);
    }

    #[test]
    fn a_plain_zoom_is_smoothed_once_whatever_the_frame_rate() {
        let arm = ArmTuning { follow_rate: Some(6.0), ..default() };
        let zoom = |steps: u32| {
            let dt = 0.5 / steps as f32;
            let (mut length, mut reach) = (3.0, 3.0);
            for _ in 0..steps {
                let next = arm_length(length, 6.0, &arm, dt);
                reach = arm_reach(reach, length, next, next, &arm, dt);
                length = next;
            }
            (length, reach)
        };
        for steps in [1, 4, 30] {
            let (length, reach) = zoom(steps);
            assert!((reach - length).abs() < 1e-5, "no second stage at {steps} steps: {reach} vs {length}");
        }
    }

    #[test]
    fn the_arm_is_never_closer_than_its_minimum_unless_the_room_is() {
        let arm = ArmTuning { follow_rate: Some(6.0), min_distance: 0.7, ..default() };
        assert_eq!(arm_reach(2.0, 5.0, 5.0, 0.2, &arm, 0.016), 0.2, "a free room smaller than the minimum is all there is");
        assert!(arm_reach(2.0, 5.0, 5.0, 1.0, &arm, 0.016) >= 0.7);
        // Growing back from a start below the minimum jumps up to it.
        assert!(arm_reach(0.1, 5.0, 5.0, 5.0, &arm, 0.001) >= 0.7);
    }

    #[test]
    fn the_nominal_arm_follows_the_distance_at_its_rate() {
        let arm = ArmTuning { follow_rate: Some(6.0), ..default() };
        let next = arm_length(5.0, 3.1, &arm, 0.1);
        assert!((next - (5.0 + (3.1 - 5.0) * (1.0 - ops::exp(-0.6)))).abs() < 1e-5);
        assert_eq!(arm_length(5.0, 3.1, &ArmTuning::default(), 0.1), 3.1, "instant without a rate");
        assert_eq!(arm_length(0.0, 3.1, &arm, 0.1), 3.1, "the first frame stands at the distance");
    }

    #[test]
    fn a_mouse_flick_is_a_sum_that_drains_and_waits_out_its_cooldown() {
        let flick = FlickTuning { mouse_threshold_px: 100.0, mouse_decay_px_s: Some(600.0), cooldown_s: 0.3, ..default() };
        let mut s = FlickState::default();
        let dt = 1.0 / 60.0;
        // 60 px a frame at 60 Hz: 60, then 60 - 10 + 60 = 110, past 100: the second frame is the flick.
        assert_eq!(s.mouse(&flick, 60.0, dt, true), None);
        assert_eq!(s.mouse(&flick, 60.0, dt, true), Some(Side::Right));
        assert_eq!(s.accum_px, 0.0, "a flick starts the sum again");
        // Slow motion never reaches it: 5 px a frame drains faster than it adds (10 a frame).
        for _ in 0..200 {
            assert_eq!(s.mouse(&flick, 5.0, dt, true), None);
        }
        assert!(s.accum_px <= 5.0 + 1e-4);
        // The other way is the other side.
        let mut s = FlickState::default();
        assert_eq!(s.mouse(&flick, -200.0, dt, true), Some(Side::Left));
        // A flick that comes in the cooldown is not lost: the sum keeps adding, and goes off when it ends.
        s.flicked(&flick, true);
        assert_eq!(s.cooldown, 0.3);
        assert_eq!(s.mouse(&flick, 150.0, dt, true), None);
        assert!(s.accum_px > 100.0);
        s.tick(0.2);
        assert_eq!(s.mouse(&flick, 0.0, 0.0, true), None, "0.1 s of cooldown left");
        s.tick(0.15);
        assert_eq!(s.mouse(&flick, 0.0, 0.0, true), Some(Side::Right));
        // Without a lock the sum is dropped.
        s.accum_px = 90.0;
        assert_eq!(s.mouse(&flick, 90.0, dt, false), None);
        assert_eq!(s.accum_px, 0.0);
    }

    #[test]
    fn without_a_decay_only_one_frames_motion_counts() {
        // The camera before it had numbers: a threshold on one frame of look.
        let flick = FlickTuning::default();
        let mut s = FlickState::default();
        assert_eq!(s.mouse(&flick, 10.0, 0.016, true), None);
        assert_eq!(s.mouse(&flick, 10.0, 0.016, true), None, "10 + 10 is not one 20 px frame");
        assert_eq!(s.mouse(&flick, 30.0, 0.016, true), Some(Side::Right));
        assert_eq!(s.mouse(&flick, -30.0, 0.016, true), Some(Side::Left), "no latch of its own");
    }

    #[test]
    fn a_cooldown_that_does_not_tick_does_not_run_down() {
        let flick = FlickTuning::default();
        let mut s = FlickState::default();
        s.flicked(&flick, true);
        for _ in 0..1000 {
            s.tick(0.0);
        }
        assert_eq!(s.cooldown, 0.35, "a paused simulation clock gives it no time");
        assert_eq!(s.mouse(&flick, 100.0, 0.016, true), None);
        s.tick(0.36);
        assert_eq!(s.cooldown, 0.0);
        assert_eq!(s.mouse(&flick, 100.0, 0.016, true), Some(Side::Right));
        // A miss starts no cooldown unless asked to.
        s.flicked(&flick, false);
        assert_eq!(s.cooldown, 0.0);
        s.flicked(&FlickTuning { cooldown_on_miss: true, ..default() }, false);
        assert_eq!(s.cooldown, 0.35);
    }

    #[test]
    fn a_stick_flick_needs_the_stick_to_come_back() {
        let flick = FlickTuning { stick_threshold: 0.8, stick_rearm: Some(0.3), cooldown_s: 0.3, ..default() };
        let mut s = FlickState::default();
        assert_eq!(s.stick(&flick, 0.5, true), None, "not far enough");
        assert_eq!(s.stick(&flick, 0.9, true), Some(Side::Right));
        s.flicked(&flick, true);
        s.tick(1.0);
        assert_eq!(s.stick(&flick, 0.9, true), None, "held over: not re-armed");
        assert_eq!(s.stick(&flick, 0.4, true), None, "between the two levels is not back");
        assert_eq!(s.stick(&flick, 0.1, true), None);
        assert_eq!(s.stick(&flick, -0.9, true), Some(Side::Left));
        // A fresh lock with the stick already over: not until it comes back.
        s.tick(1.0);
        s.lock_taken(&flick, 0.95);
        assert_eq!(s.stick(&flick, 0.95, true), None);
        s.lock_taken(&flick, 0.1);
        assert_eq!(s.stick(&flick, 0.95, true), Some(Side::Right));
        // Unlocked, a stick does nothing; the cooldown holds it.
        assert_eq!(s.stick(&flick, 0.0, false), None);
        s.flicked(&flick, true);
        assert_eq!(s.stick(&flick, 0.95, true), None);
    }

    #[test]
    fn without_a_rearm_a_held_stick_flicks_again_after_each_cooldown() {
        let flick = FlickTuning::default();
        let mut s = FlickState::default();
        assert_eq!(s.stick(&flick, 1.0, true), Some(Side::Right));
        s.flicked(&flick, true);
        assert_eq!(s.stick(&flick, 1.0, true), None);
        s.tick(0.4);
        assert_eq!(s.stick(&flick, 1.0, true), Some(Side::Right));
    }

    #[test]
    fn a_turn_of_the_wheel_is_one_switch_however_long_it_scrolls() {
        let flick = FlickTuning { cooldown_s: 0.3, ..default() };
        let mut s = FlickState::default();
        assert_eq!(s.wheel(1.0, false), None, "unlocked: nothing");
        assert_eq!(s.wheel(-1.0, true), Some(Side::Left));
        s.flicked(&flick, true);
        // A touchpad that keeps scrolling (and coasting) past the cooldown: still that one switch.
        for _ in 0..60 {
            s.tick(1.0 / 60.0);
            assert_eq!(s.wheel(-0.4, true), None);
        }
        // It stood still for a frame: the next turn switches, once the cooldown is over.
        assert_eq!(s.wheel(0.0, true), None);
        assert_eq!(s.wheel(2.0, true), Some(Side::Right));
        s.flicked(&flick, true);
        assert_eq!(s.wheel(0.0, true), None);
        assert_eq!(s.wheel(1.0, true), None, "the cooldown holds a fresh turn");
        s.tick(0.4);
        assert_eq!(s.wheel(1.0, true), Some(Side::Right), "a turn the cooldown held switches when it is over");
    }

    #[test]
    fn the_shake_amplitudes_are_the_tunings() {
        let a = shake_offset_with(1.0, 0.37, &TraumaTuning::default());
        assert_eq!(a, shake_offset(1.0, 0.37), "the defaults are the old shake");
        let flat = shake_offset_with(1.0, 0.37, &TraumaTuning { shift_m: 0.0, roll_deg: 0.0, ..default() });
        assert_eq!(flat.translation, Vec3::ZERO);
        assert_eq!(flat.rotation.z, 0.0);
        assert_eq!((flat.rotation.x, flat.rotation.y), (a.rotation.x, a.rotation.y));
        let big = shake_offset_with(1.0, 0.37, &TraumaTuning { pitch_deg: 4.0, ..default() });
        assert!((big.rotation.x - 2.0 * a.rotation.x).abs() < 1e-6);
    }
}
