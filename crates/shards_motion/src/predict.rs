//! What a client's correction needs of the motion module ([`PredictedMotion`]): the body's
//! motion mode, here and in the host's state, and moving the body the way the controller
//! itself would, so the next tick goes on from there instead of starting afresh.

use ai_core::state::{decode_value, ComponentValues};
use bevy::prelude::*;
use bevy::reflect::{FromReflect, TypePath};
use shards_core::PredictedMotion;

use crate::controller::MotionState;

pub(crate) fn plugin(app: &mut App) {
    app.insert_resource(PredictedMotion { mode, host_mode, shift, adopt });
}

fn mode(world: &World, body: Entity) -> Option<u8> {
    world.get::<MotionState>(body).map(|s| s.mode as u8)
}

fn decoded<T: FromReflect + TypePath>(world: &World, values: &ComponentValues) -> Option<T> {
    let value = values.get(T::type_path())?;
    let reflected = decode_value(world, T::type_path(), value).ok()?;
    T::from_reflect(reflected.as_ref())
}

fn host_mode(world: &World, values: &ComponentValues) -> Option<u8> {
    decoded::<MotionState>(world, values).map(|s| s.mode as u8)
}

/// The feet move by `by`, and the controller is told it left them there: speed, fall and
/// climb go on.
fn shift(world: &mut World, body: Entity, by: Vec3) {
    let Some(mut tf) = world.get_mut::<Transform>(body) else { return };
    tf.translation += by;
    let feet = tf.translation;
    if let Some(mut state) = world.get_mut::<MotionState>(body)
        && state.placed.is_some()
    {
        state.placed = Some(feet);
    }
}

fn adopt(world: &mut World, body: Entity, values: &ComponentValues, feet: Vec3) {
    let host = decoded::<MotionState>(world, values);
    let rotation = decoded::<Transform>(world, values).map(|t| t.rotation);
    if let Some(mut tf) = world.get_mut::<Transform>(body) {
        tf.translation = feet;
        if let Some(rotation) = rotation {
            tf.rotation = rotation;
        }
    }
    if let Some(mut host) = host
        && let Some(mut state) = world.get_mut::<MotionState>(body)
    {
        host.placed = Some(feet);
        *state = host;
    }
}
