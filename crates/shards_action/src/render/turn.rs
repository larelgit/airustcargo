//! The model's turn (D1.3). A run of an action that keeps the body's facing (`face: Keep`), shown by
//! a mapping with `turn` seconds ([`ActionClip::turn`](crate::anim::ActionClip::turn)), turns the
//! model to the run's heading while the body goes on looking where it looked: a roll to the side
//! under lock-on rolls the model that way, and the body still faces its target. After the run the
//! model turns back over the same seconds.
//!
//! [`Turn`] is one body's bookkeeping, advanced once a frame by [`drive`](super::bridge::drive) on
//! the simulation's clock (a pause freezes it, a `step` moves it). The target is read live: a
//! lock-on that turns the body during the roll leaves the model on the roll's heading. The model
//! closes in on it exponentially, 99 % of the way in the turn's seconds, so a new heading mid-turn
//! never jumps. [`apply_model`](super::modes::apply_model) writes it on the set's `root_motion` bone
//! as a turn about the world's up, never on the model child that carries the render interpolation.
//! Presentation only: nothing here is read by the simulation.

use std::f32::consts::{PI, TAU};

use bevy::prelude::*;

/// `e^-SETTLE` is 1 %: the share of a turn still left after its seconds.
const SETTLE: f32 = 4.6;

/// Radians under which the model is on its target.
const ON_TARGET: f32 = 1e-4;

/// An angle in radians, brought into `[-π, π)`.
pub(crate) fn wrap(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

/// The turn about the world's up (radians, as `Quat::from_rotation_y` takes it) that brings `from`
/// to `to`, both read on the ground plane. `None` when either points nowhere on it.
pub(crate) fn yaw_between(from: Vec3, to: Vec3) -> Option<f32> {
    let flat = |d: Vec3| Some(Vec2::new(d.x, d.z)).filter(|d| d.length_squared() > 1e-8);
    let (from, to) = (flat(from)?, flat(to)?);
    let yaw = |d: Vec2| ops::atan2(-d.x, -d.y);
    Some(wrap(yaw(to) - yaw(from)))
}

/// Per body: how far the model is turned from the body's facing, and what was written on the bone.
/// Kept in [`AnimationBound`](super::AnimationBound) and taken over by a new binding of the body.
#[derive(Clone, Debug, Default)]
pub(crate) struct Turn {
    /// Radians the model is turned by about the world's up, from where the body faces.
    yaw: f32,
    /// The seconds of the last turn: the turn back takes as long.
    secs: f32,
    last_clock: Option<f64>,
    /// What [`write`](Self::write) put on the bone's rotation at the last frame, and the turn that
    /// was in it: lets it tell a rotation the clips wrote again from one they left alone.
    pub(crate) written: Option<(Entity, Quat, Quat)>,
}

impl Turn {
    /// A new binding keeps the turn and what was written (the bone is the same after a rebuild of
    /// the graph, and another entity, so nothing is taken off it, after a new scene).
    pub(crate) fn taken_over(&self) -> Self {
        self.clone()
    }

    /// The picture was held for `secs` of the simulation's clock: the turn resumes where it stopped.
    pub(crate) fn shift(&mut self, secs: f64) {
        if let Some(t) = &mut self.last_clock {
            *t += secs;
        }
    }

    /// Radians the model is turned by now.
    pub(crate) fn yaw(&self) -> f32 {
        self.yaw
    }

    /// One frame on, at `clock`: toward `want` (the run's heading from the body's facing, and the
    /// seconds the turn takes) while a run turns the model, else back to the body's facing over the
    /// last turn's seconds.
    pub(crate) fn advance(&mut self, want: Option<(f32, f32)>, clock: f64) {
        let dt = self.last_clock.map_or(0.0, |last| (clock - last).max(0.0)) as f32;
        self.last_clock = Some(clock);
        let target = match want {
            Some((yaw, secs)) => {
                self.secs = secs;
                yaw
            }
            None => 0.0,
        };
        let left = wrap(target - self.yaw);
        if left.abs() < ON_TARGET || self.secs <= 0.0 {
            self.yaw = target;
            return;
        }
        let share = 1.0 - ops::exp(-dt * SETTLE / self.secs);
        self.yaw = wrap(self.yaw + left * share);
    }

    /// The turn written on `bone`'s rotation, on top of what the clips left this frame: about the
    /// world's up as seen from the bone's parent at `parent`. A rotation that is exactly what was
    /// written at the last frame is not the clips' (they have no curve for it): that turn is taken
    /// off first.
    pub(crate) fn write(&mut self, bone: Entity, transform: &mut Transform, parent: Option<&GlobalTransform>) {
        if self.yaw == 0.0 && self.written.is_none() {
            return;
        }
        let mut base = transform.rotation;
        if let Some((_, written, offset)) = self.written.filter(|(on, ..)| *on == bone)
            && base == written
        {
            base = offset.inverse() * base;
        }
        let up = parent
            .map(|p| p.affine().inverse().transform_vector3(Vec3::Y))
            .and_then(Vec3::try_normalize)
            .unwrap_or(Vec3::Y);
        let offset = Quat::from_axis_angle(up, self.yaw);
        transform.rotation = offset * base;
        self.written = (self.yaw != 0.0).then_some((bone, transform.rotation, offset));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn the_yaw_between_two_directions_is_what_turns_one_into_the_other() {
        let left = yaw_between(Vec3::NEG_Z, Vec3::NEG_X).unwrap();
        assert!(near(left, PI / 2.0), "a quarter turn to the left: {left}");
        let turned = Quat::from_rotation_y(left) * Vec3::NEG_Z;
        assert!(turned.distance(Vec3::NEG_X) < 1e-5, "{turned}");
        assert!(near(yaw_between(Vec3::NEG_Z, Vec3::X).unwrap(), -PI / 2.0));
        assert!(near(yaw_between(Vec3::NEG_Z, Vec3::Z).unwrap().abs(), PI), "straight back");
        assert_eq!(yaw_between(Vec3::NEG_Z, Vec3::Y), None, "straight up points nowhere on the ground");
        assert!(near(wrap(3.0 * PI / 2.0), -PI / 2.0));
    }

    #[test]
    fn the_model_settles_on_the_heading_in_the_turns_seconds_and_comes_back_as_fast() {
        let mut turn = Turn::default();
        turn.advance(Some((1.0, 0.1)), 0.0);
        assert_eq!(turn.yaw(), 0.0, "no time has passed");
        turn.advance(Some((1.0, 0.1)), 0.05);
        assert!(turn.yaw() > 0.8 && turn.yaw() < 0.95, "about 90 % of the way at half the seconds: {}", turn.yaw());
        turn.advance(Some((1.0, 0.1)), 0.2);
        assert!(near(turn.yaw(), 1.0), "{}", turn.yaw());
        // The run is over: back over the same seconds.
        turn.advance(None, 0.25);
        assert!(turn.yaw() > 0.05 && turn.yaw() < 0.2, "{}", turn.yaw());
        for i in 0..20 {
            turn.advance(None, 0.3 + f64::from(i) * 0.05);
        }
        assert_eq!(turn.yaw(), 0.0, "on the body's facing exactly, so nothing is written any more");
        // A paused clock moves nothing.
        turn.advance(Some((1.0, 0.1)), 1.25);
        turn.advance(Some((1.0, 0.1)), 1.25);
        assert_eq!(turn.yaw(), 0.0);
    }

    #[test]
    fn a_turn_across_the_back_takes_the_short_way() {
        let mut turn = Turn::default();
        turn.advance(Some((3.0, 0.1)), 0.0);
        turn.advance(Some((3.0, 0.1)), 0.2);
        assert!(near(turn.yaw(), 3.0));
        // The heading moves past straight back: the model keeps turning the same way, not all the way round.
        turn.advance(Some((-3.0, 0.1)), 0.21);
        assert!(turn.yaw().abs() > 3.0, "{}", turn.yaw());
    }

    #[test]
    fn the_turn_is_written_on_top_of_the_clips_and_taken_off_a_bone_they_left_alone() {
        let bone = Entity::PLACEHOLDER;
        let clip = Quat::from_rotation_x(0.3);
        let mut tf = Transform::from_rotation(clip);
        let mut turn = Turn { yaw: 0.5, ..default() };
        turn.write(bone, &mut tf, None);
        assert!(tf.rotation.angle_between(Quat::from_rotation_y(0.5) * clip) < 1e-3);
        // No clip wrote the bone this frame: the old turn comes off before the new one goes on.
        turn.yaw = 0.2;
        turn.write(bone, &mut tf, None);
        assert!(tf.rotation.angle_between(Quat::from_rotation_y(0.2) * clip) < 1e-3);
        // A clip wrote it again: the turn goes on top of the new pose.
        tf.rotation = Quat::from_rotation_x(-0.4);
        turn.write(bone, &mut tf, None);
        assert!(tf.rotation.angle_between(Quat::from_rotation_y(0.2) * Quat::from_rotation_x(-0.4)) < 1e-3);
        // Back on the facing: the turn comes off once and nothing more is written.
        turn.yaw = 0.0;
        turn.write(bone, &mut tf, None);
        assert!(tf.rotation.angle_between(Quat::from_rotation_x(-0.4)) < 1e-3);
        assert!(turn.written.is_none());
        // A parent turned on its side: the turn is still about the world's up.
        let parent = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_z(PI / 2.0)));
        let mut tf = Transform::default();
        let mut turn = Turn { yaw: 0.5, ..default() };
        turn.write(bone, &mut tf, Some(&parent));
        let world = parent.rotation() * tf.rotation;
        let expected = Quat::from_rotation_y(0.5) * parent.rotation();
        assert!(world.angle_between(expected) < 1e-3, "{world:?} vs {expected:?}");
    }
}
