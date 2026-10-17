//! The engine rule `motion.finite`.

use ai_core::{with_query, AiId, AiVisible, Offenders};
use bevy::prelude::*;

use crate::controller::{Mantle, MotionState};

/// How far a rotation's length may be from 1 before it counts as not normalised.
const ROTATION_TOLERANCE: f32 = 1e-3;

/// The engine rule `motion.finite`: every `AiVisible` entity has a finite `Transform` translation
/// and a finite, normalised rotation (length within 1e-3 of 1), and every number of its
/// [`MotionState`] is finite: `velocity`, `ground_normal`, `coyote_left`, `jump_buffer_left`,
/// the corners and times of a `mantle`, `fall_from_y`, `water_surface`, `updraft` and `placed`.
///
/// A NaN in a position spreads into physics, the navmesh and the replay hash, and shows up
/// ticks later as a body that vanished or a panic far from the cause; this names the first
/// tick and the entity. Scale is not judged (a scale of zero is a choice), and neither are
/// entities the AI cannot see: tiles' render children and effects carry no `AiVisible`. A
/// rotation that a level file or a save writes unnormalised (`Quat::from_array`) is a bug in
/// that writer: the rule is the reason to normalise it there.
pub fn motion_finite(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Transform, Option<&MotionState>), With<AiVisible>, _>(world, |query| {
        for (id, transform, motion) in query.iter(world) {
            if let Some(what) = transform_problem(transform).or_else(|| motion.and_then(motion_problem)) {
                bad.note(*id, || what);
            }
        }
    })?;
    bad.report()
}

fn transform_problem(t: &Transform) -> Option<String> {
    if !t.translation.is_finite() {
        return Some(format!("Transform.translation = {} is not finite", t.translation));
    }
    let q = t.rotation;
    if !q.is_finite() || (q.length() - 1.0).abs() > ROTATION_TOLERANCE {
        return Some(format!("Transform.rotation = {q} is not finite or not normalised (length {})", q.length()));
    }
    None
}

fn motion_problem(m: &MotionState) -> Option<String> {
    let vec3 = |name: &str, v: Vec3| (!v.is_finite()).then(|| format!("MotionState.{name} = {v} is not finite"));
    let float = |name: &str, v: f32| (!v.is_finite()).then(|| format!("MotionState.{name} = {v} is not finite"));
    vec3("velocity", m.velocity)
        .or_else(|| vec3("ground_normal", m.ground_normal))
        .or_else(|| float("coyote_left", m.coyote_left))
        .or_else(|| float("jump_buffer_left", m.jump_buffer_left))
        .or_else(|| m.mantle.as_ref().and_then(mantle_problem))
        .or_else(|| m.fall_from_y.and_then(|v| float("fall_from_y", v)))
        .or_else(|| m.water_surface.and_then(|v| float("water_surface", v)))
        .or_else(|| m.updraft.and_then(|v| (!v.is_finite()).then(|| format!("MotionState.updraft = {v} is not finite"))))
        .or_else(|| m.placed.and_then(|v| vec3("placed", v)))
}

fn mantle_problem(m: &Mantle) -> Option<String> {
    let finite = m.from.is_finite() && m.corner.is_finite() && m.to.is_finite() && m.t.is_finite() && m.duration.is_finite();
    (!finite).then(|| format!("MotionState.mantle = {m:?} is not finite"))
}
