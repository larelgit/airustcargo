//! How the ground and the air steer a body (F7): [`AccelModel`], and the arithmetic of the `Projected` model as pure
//! functions the controller calls once a tick.
//!
//! **`Step`** (the default, and the only model before this one) steps the horizontal velocity along the straight line
//! to a target velocity by at most `acceleration * dt`: a body is always steered toward the speed its input asks for,
//! and the air scales the step by `air_control`.
//!
//! **`Projected`** is the model of *Quake* and *Half-Life* (the name says how it works: the speed a push can add is
//! limited by the speed the body already has *projected on the wished direction*, not by its total speed):
//!
//! ```text
//! ground:  friction (a drop of  max(speed, stop_speed) * friction * dt, down to 0)
//!          then   add = wish_speed - v . wish_dir
//!                 if add > 0:  v += wish_dir * min(add, ground_accel * wish_speed * dt)
//! air:     no friction, and   add = min(wish_speed, air_speed_cap) - v . wish_dir
//!                 if add > 0:  v += wish_dir * min(add, air_accel * wish_speed * dt)
//! ```
//!
//! The cap on the speed to be reached (not on the size of the step) is what makes air strafing work. A body flying
//! at 8 m/s can add only `air_speed_cap` (0.76 m/s) *along the direction it wishes*, so a wish at right angles to
//! the velocity still adds that much sideways, and the sum of the two is longer than either: the speed grows a little
//! every tick the wish is turned along with the velocity, which is a strafe jump. A wish along the velocity adds
//! nothing once the projected speed has passed the cap, so the air does not push a fast body faster.
//!
//! Every tick is written in closed form for the tick it is given: the friction is the exact solution of
//! `dv/dt = -friction * max(v, stop_speed)` over `dt` (an exponential above the stop speed, a straight line below it,
//! the crossing inside a tick handled), and the push is a clamped straight line, exact for a wish direction that is
//! held for the tick. A run whose wish is a function of time ends in the same place at 30, 60, 120 and 240 ticks a
//! second (the unit tests below and `tests/motion/air_move.rs` hold it to a tolerance). What does depend on the tick
//! rate is a strafe whose wish *follows the velocity* tick by tick: each tick is a fresh sideways nudge of up to
//! `air_speed_cap`, and the speed those nudges add is second order in their size, so the gain per second changes with
//! the number of ticks (as it did in the original games, whose players know it as the frame-rate effect). That is
//! content's to tune, with `air_accel`, `air_speed_cap` and the cap `max_air_speed`; the manual's "Air movement
//! (F7)" gives the numbers.

use bevy::math::ops;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// *Half-Life*'s units are 0.0254 m (a 72-unit player is 1.83 m tall).
const UNIT: f32 = 0.0254;
/// `Projected`'s default `ground_accel` and `air_accel`: *Half-Life*'s `sv_accelerate` and `sv_airaccelerate`, 10. They
/// are per second, as multiples of the wish speed: a body reaches the speed it wishes in 1/10 s from rest.
pub const DEFAULT_ACCEL: f32 = 10.0;
/// `Projected`'s default `air_speed_cap`: *Half-Life*'s 30 units per second (0.76 m/s).
pub const DEFAULT_AIR_SPEED_CAP: f32 = 30.0 * UNIT;
/// `Projected`'s default `friction`: *Half-Life*'s `sv_friction`, 4 per second.
pub const DEFAULT_FRICTION: f32 = 4.0;
/// `Projected`'s default `stop_speed`: *Half-Life*'s `sv_stopspeed`, 100 units per second (2.54 m/s).
pub const DEFAULT_STOP_SPEED: f32 = 100.0 * UNIT;

/// How the ground and the air steer a body: [`CharacterParams::accel`](super::CharacterParams::accel).
///
/// Content (`*.locomotion.ron`):
///
/// ```ron
/// // Step, the default: `acceleration`, `deceleration` and `air_control` steer the body toward its target speed.
/// "hero": (run_speed: 5.5),
/// // Half-Life's numbers (any left out take them): the ground and the air as in Quake and Half-Life.
/// "runner": (run_speed: 8.1, accel: Projected(ground_accel: 10.0, air_accel: 10.0, air_speed_cap: 0.76, friction: 4.0, stop_speed: 2.54)),
/// ```
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub enum AccelModel {
    /// The velocity steps toward the target velocity at `acceleration`, `deceleration` and (in the air) `air_control`
    /// of the params: every body before F7.
    #[default]
    Step,
    /// *Quake*'s and *Half-Life*'s ground friction and push along the wished direction (see the [module](self) docs).
    /// The params' `acceleration`, `deceleration`, `air_control`, `axis_stepping` and `brake_only_without_input` do not
    /// steer the ground and the air under it (a swimmer still reads `acceleration`); the run, walk, sprint and crouch
    /// speeds and the modifiers' caps and factors still say what speed the body *wishes*.
    Projected {
        /// How fast the ground pushes toward the wished speed, per second as a multiple of that speed: the push of a
        /// tick is at most `ground_accel * wish_speed * dt` m/s. 10: from rest to full speed in 1/10 s. Above 0.
        #[serde(default = "default_accel")]
        ground_accel: f32,
        /// The same in the air, for a push along the wished direction. 0: no air control. The step is taken from
        /// the whole wish speed; only the speed to be reached is held to `air_speed_cap`.
        #[serde(default = "default_accel")]
        air_accel: f32,
        /// The most the air adds along the wished direction (m/s): the speed the body has along it is brought up to
        /// at most this (or to the wish speed, if that is lower). Above 0.
        #[serde(default = "default_air_speed_cap")]
        air_speed_cap: f32,
        /// Ground friction, per second: a body slows by `max(speed, stop_speed) * friction * dt` a tick (an
        /// exponential decay above `stop_speed`, a straight line below it). 0: ice.
        #[serde(default = "default_friction")]
        friction: f32,
        /// Below this speed (m/s) the friction takes a straight `stop_speed * friction` m/s² off, so a body comes to
        /// a stop instead of decaying forever. Above 0.
        #[serde(default = "default_stop_speed")]
        stop_speed: f32,
        /// The highest horizontal speed the air can push a body to (m/s); a body that is already faster (a sprint's
        /// take-off, a knock) keeps its speed and is pushed no faster. `None`: no cap, so strafing in the air can
        /// take a body above its run speed, as in the original games (a bunny hop). Whether a game wants that is
        /// content's call. Above 0.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_air_speed: Option<f32>,
    },
}

fn default_accel() -> f32 {
    DEFAULT_ACCEL
}
fn default_air_speed_cap() -> f32 {
    DEFAULT_AIR_SPEED_CAP
}
fn default_friction() -> f32 {
    DEFAULT_FRICTION
}
fn default_stop_speed() -> f32 {
    DEFAULT_STOP_SPEED
}

impl AccelModel {
    /// `Projected` with *Half-Life*'s numbers and no cap on the air speed.
    pub const HALF_LIFE: AccelModel = AccelModel::Projected {
        ground_accel: DEFAULT_ACCEL,
        air_accel: DEFAULT_ACCEL,
        air_speed_cap: DEFAULT_AIR_SPEED_CAP,
        friction: DEFAULT_FRICTION,
        stop_speed: DEFAULT_STOP_SPEED,
        max_air_speed: None,
    };

    /// The model is the default one (the written form leaves it out).
    pub fn is_step(&self) -> bool {
        *self == AccelModel::Step
    }

    /// Whether the body is steered by friction and a push along the wished direction.
    pub fn is_projected(&self) -> bool {
        matches!(self, AccelModel::Projected { .. })
    }

    /// `step` or `projected`, for the AI.
    pub fn name(&self) -> &'static str {
        match self {
            AccelModel::Step => "step",
            AccelModel::Projected { .. } => "projected",
        }
    }

    /// Everything wrong with the numbers of a `Projected` model, to join into the definition's errors.
    pub(super) fn problems(&self) -> Vec<String> {
        let AccelModel::Projected { ground_accel, air_accel, air_speed_cap, friction, stop_speed, max_air_speed } = *self else {
            return Vec::new();
        };
        let mut errors = Vec::new();
        for (name, v) in [("ground_accel", ground_accel), ("air_speed_cap", air_speed_cap), ("stop_speed", stop_speed)] {
            if !(v > 0.0 && v.is_finite()) {
                errors.push(format!("accel: {name} must be > 0 (is {v})"));
            }
        }
        for (name, v) in [("air_accel", air_accel), ("friction", friction)] {
            if !(v >= 0.0 && v.is_finite()) {
                errors.push(format!("accel: {name} must be >= 0 (is {v})"));
            }
        }
        if max_air_speed.is_some_and(|v| !(v > 0.0 && v.is_finite())) {
            errors.push(format!("accel: max_air_speed must be > 0 (is {max_air_speed:?}); leave it out for no cap"));
        }
        errors
    }
}

/// The ground friction of one tick, in closed form: the horizontal speed after `dt` seconds of
/// `dv/dt = -friction * max(v, stop_speed)`. Above `stop_speed` the speed decays as `e^(-friction t)`; at `stop_speed`
/// it turns into a straight line down to 0 (`stop_speed * friction` m/s²), and a tick that crosses over takes the
/// exponential to the crossing and the line for the rest. The same over 1 tick as over 2 of half the length.
pub(crate) fn friction_speed(speed: f32, friction: f32, stop_speed: f32, dt: f32) -> f32 {
    if friction <= 0.0 || dt <= 0.0 || speed <= 0.0 {
        return speed.max(0.0);
    }
    if speed <= stop_speed {
        return (speed - friction * stop_speed * dt).max(0.0);
    }
    let decayed = speed * ops::exp(-friction * dt);
    if decayed >= stop_speed {
        return decayed;
    }
    // The tick crosses the stop speed: the exponential to there, a straight line for the rest.
    let to_stop = ops::ln(speed / stop_speed) / friction;
    (stop_speed - friction * stop_speed * (dt - to_stop)).max(0.0)
}

/// `v` after the ground friction of a tick (see [`friction_speed`]): the same direction at the slower speed.
pub(crate) fn apply_friction(v: Vec3, friction: f32, stop_speed: f32, dt: f32) -> Vec3 {
    let speed = v.length();
    if speed <= f32::EPSILON {
        return Vec3::ZERO;
    }
    let next = friction_speed(speed, friction, stop_speed, dt);
    if next >= speed { v } else { v * (next / speed) }
}

/// The push of a tick along `dir` (a unit vector): bring `v . dir` up toward `reach`, by at most `step`.
fn push(v: Vec3, dir: Vec3, reach: f32, step: f32) -> Vec3 {
    let add = reach - v.dot(dir);
    if add <= 0.0 { v } else { v + dir * add.min(step) }
}

/// The ground's push of a tick: toward `wish_speed` along `dir`, by at most `accel * wish_speed * dt`. `dir`
/// is a unit vector (a zero one pushes nothing).
pub(crate) fn ground_accelerate(v: Vec3, dir: Vec3, wish_speed: f32, accel: f32, dt: f32) -> Vec3 {
    if dir == Vec3::ZERO || wish_speed <= 0.0 {
        return v;
    }
    push(v, dir, wish_speed, accel * wish_speed * dt)
}

/// The air's push of a tick: the speed to be reached along `dir` is `wish_speed` held to `cap`, the step is taken from the whole
/// `wish_speed` (`accel * wish_speed * dt`, as in the original games: the cap is on where the push stops, not on how
/// hard it pushes).
pub(crate) fn air_accelerate(v: Vec3, dir: Vec3, wish_speed: f32, accel: f32, cap: f32, dt: f32) -> Vec3 {
    if dir == Vec3::ZERO || wish_speed <= 0.0 {
        return v;
    }
    push(v, dir, wish_speed.min(cap), accel * wish_speed * dt)
}

/// `after` (the velocity at the end of an air tick) held to `max` m/s, but never below the speed the tick started
/// with (`before`): a cap stops the air pushing a body faster, it does not brake one that came in faster.
pub(crate) fn limit_air_speed(before: Vec3, after: Vec3, max: f32) -> Vec3 {
    let limit = max.max(before.length());
    let speed = after.length();
    if speed > limit { after * (limit / speed) } else { after }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn friction_follows_the_hand_computed_decay() {
        // 8 m/s, friction 4, stop speed 2.5: above the stop speed `v e^(-4 t)`. After 0.1 s: 8 * e^(-0.4) = 5.3626.
        let v = friction_speed(8.0, 4.0, 2.5, 0.1);
        assert!(close(v, 8.0 * 0.670_320_05, 1e-4), "{v}");
        // At and under the stop speed it is a straight line of `4 * 2.5 = 10` m/s²: 2.0 m/s for 0.05 s loses 0.5.
        assert!(close(friction_speed(2.0, 4.0, 2.5, 0.05), 1.5, 1e-6));
        // ... and it ends at 0, not below.
        assert_eq!(friction_speed(0.4, 4.0, 2.5, 0.05), 0.0);
        // A tick across the stop speed: 8 -> 2.5 takes ln(3.2) / 4 = 0.290 783 s, then 10 m/s² for the rest of 0.3 s: 2.5 - 10 * 0.009 217 = 2.407 83.
        let across = friction_speed(8.0, 4.0, 2.5, 0.3);
        assert!(close(across, 2.5 - 10.0 * (0.3 - ops::ln(3.2) / 4.0), 1e-4) && close(across, 2.4078, 1e-3), "{across}");
        // No friction, no time or no speed change nothing.
        assert_eq!(friction_speed(5.0, 0.0, 2.5, 0.1), 5.0);
        assert_eq!(friction_speed(5.0, 4.0, 2.5, 0.0), 5.0);
        assert_eq!(friction_speed(0.0, 4.0, 2.5, 0.1), 0.0);
    }

    #[test]
    fn friction_is_the_same_whatever_the_tick_rate() {
        // One second of friction from 8 m/s: the exponential to the stop speed (0.29 s), the line to rest (0.25 s later).
        let run = |ticks: u32, from: f32| {
            let dt = 1.0 / ticks as f32;
            (0..ticks).fold(from, |v, _| friction_speed(v, 4.0, 2.5, dt))
        };
        for from in [8.0, 3.0, 2.0] {
            let at_60 = run(60, from);
            for ticks in [30, 120, 240] {
                let v = run(ticks, from);
                assert!(close(v, at_60, 1e-4), "{from} m/s, {ticks} ticks a second: {v} vs {at_60}");
            }
        }
        // And over 0.2 s it is the exact `8 e^(-0.8)`, whatever the split.
        let exact = 8.0 * ops::exp(-0.8);
        for ticks in [1, 6, 12, 24] {
            let dt = 0.2 / ticks as f32;
            let v = (0..ticks).fold(8.0, |v, _| friction_speed(v, 4.0, 2.5, dt));
            assert!(close(v, exact, 2e-4), "{ticks} ticks: {v} vs {exact}");
        }
    }

    #[test]
    fn friction_matches_the_original_games_per_frame_rule() {
        // The original takes `max(speed, stop_speed) * friction * dt` off every frame (Euler). The closed form is the
        // exact solution of the same law, so the two agree to a few percent at 60 and 120 frames a second: 0.2 s from 8 m/s.
        let original = |frames: u32| {
            let dt = 1.0 / frames as f32;
            (0..frames / 5).fold(8.0f32, |v, _| (v - v.max(2.5) * 4.0 * dt).max(0.0))
        };
        let closed = |ticks: u32| (0..ticks / 5).fold(8.0f32, |v, _| friction_speed(v, 4.0, 2.5, 1.0 / ticks as f32));
        for frames in [60, 120] {
            let (old, new) = (original(frames), closed(frames));
            assert!((old - new).abs() / new < 0.03, "{frames} frames: the original {old}, the closed form {new}");
        }
    }

    #[test]
    fn the_ground_push_follows_the_hand_computed_step() {
        // Wish 8 m/s east, `ground_accel` 10, a tick of 1/60 s: the push is at most 10 * 8 / 60 = 1.3333 m/s.
        let east = Vec3::X;
        let v = ground_accelerate(Vec3::ZERO, east, 8.0, 10.0, 1.0 / 60.0);
        assert!(close(v.x, 4.0 / 3.0, 1e-5) && v.z == 0.0, "{v}");
        // 7.5 m/s east already: only 0.5 is left to add, less than the step.
        assert!(close(ground_accelerate(Vec3::X * 7.5, east, 8.0, 10.0, 1.0 / 60.0).x, 8.0, 1e-6));
        // Faster than the wish along it: nothing is added, and nothing is taken away (friction does that).
        assert_eq!(ground_accelerate(Vec3::X * 9.0, east, 8.0, 10.0, 1.0 / 60.0), Vec3::X * 9.0);
        // A sideways speed is untouched by a push along another line: 3 m/s north, wish east from rest along it.
        let v = ground_accelerate(Vec3::new(0.0, 0.0, -3.0), east, 8.0, 10.0, 1.0 / 60.0);
        assert!(close(v.x, 4.0 / 3.0, 1e-5) && v.z == -3.0, "{v}");
        // No wish, no push.
        assert_eq!(ground_accelerate(Vec3::X, Vec3::ZERO, 8.0, 10.0, 0.1), Vec3::X);
        assert_eq!(ground_accelerate(Vec3::X, east, 0.0, 10.0, 0.1), Vec3::X);
    }

    #[test]
    fn the_air_push_adds_the_capped_speed_along_the_wish_and_no_more() {
        let cap = 0.76;
        let dt = 1.0 / 60.0;
        // Flying east at 8 m/s, wish north (a right angle): `add = cap - 0 = 0.76` and the step is 10 * 8 / 60 = 1.33,
        // so the push is the cap, 0.76 m/s north. Speed: sqrt(8^2 + 0.76^2) = 8.0361 m/s.
        let north = -Vec3::Z;
        let v = air_accelerate(Vec3::X * 8.0, north, 8.0, 10.0, cap, dt);
        assert!(close(v.z, -0.76, 1e-6) && v.x == 8.0, "{v}");
        assert!(close(v.length(), (64.0f32 + 0.76 * 0.76).sqrt(), 1e-5));
        // Pushed again the same way, there is nothing to add: the speed along the wish is already the cap.
        assert_eq!(air_accelerate(v, north, 8.0, 10.0, cap, dt), v);
        // A wish along the velocity adds nothing to a fast body ...
        assert_eq!(air_accelerate(Vec3::X * 8.0, Vec3::X, 8.0, 10.0, cap, dt), Vec3::X * 8.0);
        // ... but a slow one (0.2 m/s along it) is brought up to the cap: add = 0.56 < the step 1.33.
        assert!(close(air_accelerate(Vec3::X * 0.2, Vec3::X, 8.0, 10.0, cap, dt).x, 0.76, 1e-6));
        // The step comes from the whole wish speed: a smaller `air_accel` limits it: 0.5 * 8 / 60 = 0.0667 m/s of 0.76.
        let slow = air_accelerate(Vec3::X * 8.0, north, 8.0, 0.5, cap, dt);
        assert!(close(slow.z, -0.5 * 8.0 / 60.0, 1e-6), "{slow}");
        // A wish speed under the cap is the speed to be reached (a walk of 0.5 m/s adds up to 0.5 m/s), and its step is from 0.5.
        let walk = air_accelerate(Vec3::ZERO, Vec3::X, 0.5, 10.0, cap, dt);
        assert!(close(walk.x, 10.0 * 0.5 / 60.0, 1e-6), "{walk}");
        // Pushing against the velocity brakes at the full step: wish west at 8 m/s east, add = 0.76 + 8: step 1.33.
        let brake = air_accelerate(Vec3::X * 8.0, -Vec3::X, 8.0, 10.0, cap, dt);
        assert!(close(brake.x, 8.0 - 4.0 / 3.0, 1e-5), "{brake}");
    }

    #[test]
    fn a_steady_air_wish_ends_in_the_same_place_at_every_tick_rate() {
        // The push is a clamped straight line: a wish held for a second gives the same velocity at 30 to 240 ticks a second.
        let run = |ticks: u32| {
            let dt = 1.0 / ticks as f32;
            let wish = Vec3::new(1.0, 0.0, -1.0).normalize();
            (0..ticks).fold(Vec3::X * 8.0, |v, _| air_accelerate(v, wish, 8.0, 10.0, 0.76, dt))
        };
        let at_60 = run(60);
        for ticks in [30, 120, 240] {
            assert!(run(ticks).distance(at_60) < 1e-4, "{ticks}: {} vs {at_60}", run(ticks));
        }
    }

    #[test]
    fn a_wish_turned_ninety_degrees_over_a_second_keeps_the_speed_at_every_tick_rate() {
        // The wish rotates from east to north at 90 degrees a second while the body flies east at 8 m/s.
        let run = |ticks: u32| {
            let dt = 1.0 / ticks as f32;
            (0..ticks).fold(Vec3::X * 8.0, |v, i| {
                let turned = std::f32::consts::FRAC_PI_2 * (i as f32 * dt);
                let wish = Vec3::new(ops::cos(turned), 0.0, -ops::sin(turned));
                air_accelerate(v, wish, 8.0, 10.0, 0.76, dt)
            })
        };
        let at_60 = run(60);
        assert!(at_60.length() >= 8.0 * 0.99, "the speed is kept: {}", at_60.length());
        for ticks in [30, 120, 240] {
            let v = run(ticks);
            assert!((v.length() - at_60.length()).abs() / at_60.length() < 0.01, "{ticks}: {} vs {}", v.length(), at_60.length());
        }
    }

    #[test]
    fn a_strafe_that_follows_the_velocity_gains_speed_and_that_gain_depends_on_the_tick() {
        // Each tick the wish is a right angle to the velocity: a fresh nudge of the capped speed sideways, and the speed
        // grows by about nudge^2 / (2 speed) a tick. Here the numbers are Half-Life's, and one second at 60 ticks gains
        // a lot; the gain per second is not the same at other rates (it grows with the ticks until `air_accel * wish_speed
        // * dt` falls under the cap, then falls), which is why the manual calls it content's to tune.
        let run = |ticks: u32| {
            let dt = 1.0 / ticks as f32;
            (0..ticks).fold(Vec3::X * 8.13, |v, _| {
                let right_angle = Vec3::new(v.z, 0.0, -v.x).normalize();
                air_accelerate(v, right_angle, 8.13, 10.0, 0.762, dt).with_y(0.0)
            })
        };
        let (at_30, at_60, at_120) = (run(30).length(), run(60).length(), run(120).length());
        assert!(at_60 > 8.13 * 1.15, "a strafe gains speed: {at_60}");
        assert!(at_30 > 8.13 && at_120 > 8.13, "{at_30} {at_120}");
    }

    #[test]
    fn the_air_speed_cap_holds_a_gain_and_keeps_a_faster_take_off() {
        let before = Vec3::X * 8.0;
        let after = Vec3::new(8.0, 0.0, -0.76);
        // No harm below the cap: 8.04 < 9.
        assert_eq!(limit_air_speed(before, after, 9.0), after);
        // A cap under the take-off speed does not brake the body, it only stops the gain: held to 8.0 m/s.
        let held = limit_air_speed(before, after, 6.0);
        assert!(close(held.length(), 8.0, 1e-5) && held.z < 0.0, "{held}");
        // A cap above the take-off speed is the limit: held to 8.02.
        let held = limit_air_speed(before, after, 8.02);
        assert!(close(held.length(), 8.02, 1e-5), "{held}");
    }

    #[test]
    fn the_written_form_leaves_the_default_model_out_and_a_projected_one_reads_back() {
        let written = ron::to_string(&super::super::CharacterParams::default()).unwrap();
        assert!(!written.contains("Projected") && !written.contains("Step") && !written.contains("air_speed_cap"), "{written}");
        // A definition with a few numbers takes Half-Life's for the rest.
        let p: super::super::CharacterParams = ron::from_str("(accel: Projected(air_accel: 12.0))").unwrap();
        let AccelModel::Projected { ground_accel, air_accel, air_speed_cap, friction, stop_speed, max_air_speed } = p.accel else {
            panic!("{:?}", p.accel);
        };
        assert_eq!(
            (ground_accel, air_accel, air_speed_cap, friction, stop_speed, max_air_speed),
            (10.0, 12.0, 30.0 * 0.0254, 4.0, 100.0 * 0.0254, None)
        );
        let again = ron::to_string(&p).unwrap();
        assert!(again.contains("Projected(") && !again.contains("max_air_speed"), "{again}");
        assert_eq!(ron::from_str::<super::super::CharacterParams>(&again).unwrap(), p);
        let capped: super::super::CharacterParams = ron::from_str("(accel: Projected(max_air_speed: Some(9.0)))").unwrap();
        assert!(ron::to_string(&capped).unwrap().contains("max_air_speed:Some(9.0)"));
        assert_eq!(ron::from_str::<super::super::CharacterParams>("(accel: Step)").unwrap().accel, AccelModel::Step);
        assert_eq!(AccelModel::HALF_LIFE, ron::from_str::<super::super::CharacterParams>("(accel: Projected())").unwrap().accel);
    }

    #[test]
    fn a_typo_in_the_model_is_an_error_and_bad_numbers_are_named() {
        use super::super::CharacterParams;
        assert!(ron::from_str::<CharacterParams>("(accel: Projected(air_acel: 12.0))").is_err(), "an unknown field is an error");
        // Half-Life-like numbers, one of them replaced.
        let with = |pick: usize, value: f32| {
            let mut v = [10.0, 10.0, 0.76, 4.0, 2.54];
            v[pick] = value;
            AccelModel::Projected { ground_accel: v[0], air_accel: v[1], air_speed_cap: v[2], friction: v[3], stop_speed: v[4], max_air_speed: None }
        };
        let bad = |accel: AccelModel| CharacterParams { accel, ..default() }.check().unwrap_err();
        assert!(bad(with(0, 0.0)).contains("ground_accel must be > 0"));
        assert!(bad(with(1, -1.0)).contains("air_accel must be >= 0"));
        assert!(bad(with(2, 0.0)).contains("air_speed_cap must be > 0"));
        assert!(bad(with(3, f32::NAN)).contains("friction must be >= 0"));
        assert!(bad(with(4, 0.0)).contains("stop_speed must be > 0"));
        let capped = AccelModel::Projected { ground_accel: 10.0, air_accel: 10.0, air_speed_cap: 0.76, friction: 4.0, stop_speed: 2.54, max_air_speed: Some(0.0) };
        assert!(bad(capped).contains("max_air_speed must be > 0"));
        CharacterParams { accel: AccelModel::HALF_LIFE, ..default() }.check().unwrap();
        // No air control and ice are allowed.
        CharacterParams { accel: with(1, 0.0), ..default() }.check().unwrap();
        CharacterParams { accel: with(3, 0.0), ..default() }.check().unwrap();
    }
}
