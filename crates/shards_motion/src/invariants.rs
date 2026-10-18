//! The engine rules `motion.finite` and `motion.launch_sane` (on by default) and `voxel.body_in_solid` (opt-in: off unless a game
//! or a run turns it on with `--engine-invariant voxel.body_in_solid`).

use ai_core::{with_query, AiId, AiVisible, Offenders};
use bevy::prelude::*;

use crate::controller::{CharacterController, Launch, Mantle, MotionState};
use crate::physics::{Body, Collider};

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
    // `MotionState` is read per entity, not named in the query: a world that never registered
    // it still has its transforms judged.
    with_query::<(Entity, &AiId, &Transform), With<AiVisible>, _>(world, |query| {
        for (entity, id, transform) in query.iter(world) {
            let motion = world.get::<MotionState>(entity);
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

/// The name of [`body_in_solid`].
pub const BODY_IN_SOLID: &str = "voxel.body_in_solid";

/// The engine rule `voxel.body_in_solid`: no character (`CharacterController`, `Body::Kinematic`)
/// has the centre of its capsule (the `Transform` plus the collider's offset) in a solid cell of a
/// [`VoxelField`](crate::physics::VoxelField) it collides with. The controller keeps bodies out of
/// solids, so a centre in rock is a collision bug: a sweep that missed a cell seam, a
/// depenetration that pushed into the next cell. The centre is judged, not the feet, which rest on
/// a top face.
///
/// Carve-out: a body the controller did not place where it is now (`MotionState::placed` is not
/// its translation, or it has not been placed yet): a `teleport`, a spawn or a region change put it
/// there this tick, and the next tick's depenetration starts from there.
///
/// **Off by default.** Mantle, slide and sloped-step ticks may put the centre in a cell for a tick
/// (a body hugging a ledge corner), so the motion plugin offers the rule with
/// `add_opt_in_engine_invariant`: it is checked only when `EngineConfig::opt_in_invariants` names
/// it (`--engine-invariant voxel.body_in_solid`, for the fuzz runs on voxel terrain). It becomes an
/// ordinary engine rule, on by default, once those runs have passed (the voxel design, 6.4: twenty
/// seeds of `shards fuzz` on the voxel arena, each stop explained and carved out here with a test).
/// A world without fields never breaks it.
pub fn body_in_solid(world: &World) -> Option<String> {
    let physics = world.get_resource::<crate::physics::PhysicsWorld>()?;
    // A world without fields: nothing to judge.
    physics.fields().next()?;
    let mut bad = Offenders::default();
    type Character = (&'static AiId, &'static Transform, &'static MotionState, &'static Collider, &'static Body);
    with_query::<Character, With<CharacterController>, _>(world, |query| {
        for (id, tf, state, collider, body) in query.iter(world) {
            if *body != Body::Kinematic || state.placed != Some(tf.translation) {
                continue;
            }
            let centre = tf.translation + tf.rotation * collider.offset;
            for (_, field_id, field) in physics.fields() {
                let cell = field.cell_at(centre);
                if field.layers().memberships.intersects(collider.layers.filters) && field.solid(cell) {
                    bad.note(*id, || format!("capsule centre {centre} is in solid cell {cell} of field {field_id}"));
                    break;
                }
            }
        }
    })?;
    bad.report()
}

/// The engine rule `motion.launch_sane`: a [`Launch`] asks for a finite height above 0 and sits on
/// a character the controller moves (`CharacterController`, which brings `MotionState`).
///
/// The controller takes a launch in the tick it is put (the action machine puts it in `SimSet::Act`,
/// the controller reads it in `SimSet::Move`) and removes it; a launch on anything else is never
/// read and never removed, and one with no height or a NaN one launches the body at nothing or
/// into the sky. A game that launches by hand puts it before `SimSet::Move` on a character.
pub fn launch_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(Entity, &AiId, &Launch), (), _>(world, |query| {
        for (entity, id, launch) in query.iter(world) {
            if !(launch.height.is_finite() && launch.height > 0.0) {
                bad.note(*id, || format!("Launch height = {}: a finite height above 0", launch.height));
            } else if world.get::<CharacterController>(entity).is_none() {
                bad.note(*id, || "Launch on an entity with no CharacterController: nothing reads it".to_owned());
            }
        }
    })?;
    bad.report()
}
