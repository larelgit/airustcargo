//! `Intent` → [`LocomotionInput`]: how what a character wants (keyboard, agent, brain,
//! replay) becomes controller input.
//!
//! Runs in `SimSet::Act`, first: the action machine orders its systems
//! `.after(MotionSystems::Intent)` and then owns `external`, `locked`, `face` and may take
//! `sprint` away (no stamina). Nothing here resets those after it.

use ai_core::{AiIndex, SimSet};
use bevy::prelude::*;
use shards_core::input_names::{GLIDE, JUMP, SPRINT};
use shards_core::{flat, Intent};

use crate::controller::{LocomotionInput, MotionMode, MotionState};

/// The motion systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MotionSystems {
    /// `SimSet::Act`: [`intent_to_locomotion`] fills [`LocomotionInput`] from `Intent`
    /// (`move_dir`, held `sprint` and `glide`, the `jump` press, facing).
    Intent,
}

/// Put on a character by the `glide` tool: the glide is held until the character has been
/// in the air and lands (or reaches water), then released. A key held by a human is not
/// latched: the human lets go themselves.
#[derive(Component, Reflect, Default, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct GlideUntilLanded {
    /// Left the ground since the glide was asked for.
    pub flown: bool,
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<GlideUntilLanded>()
        .configure_sets(FixedUpdate, MotionSystems::Intent.in_set(SimSet::Act))
        .add_systems(FixedUpdate, intent_to_locomotion.in_set(MotionSystems::Intent));
}

/// What [`intent_to_locomotion`] reads and writes of a character.
type Steered = (Entity, &'static mut Intent, &'static mut LocomotionInput, &'static MotionState, &'static Transform, Option<&'static mut GlideUntilLanded>);

/// For every character with `Intent` and `LocomotionInput`: clears what the last tick's
/// actions left (`external`, `locked`, `face`), then takes
///
/// - `move_dir` from `Intent.move_dir` (flattened, at most length 1),
/// - `sprint` and `glide` from the held buttons of the same names,
/// - `jump` from the `jump` presses (consumed),
/// - `face` toward `Intent.aim`, else toward the lock-on `Intent.target`.
///
/// Dead or stunned characters are the action machine's business: it sets `locked`.
pub fn intent_to_locomotion(
    mut commands: Commands,
    index: Res<AiIndex>,
    positions: Query<&Transform>,
    mut characters: Query<Steered>,
) {
    for (entity, mut intent, mut input, state, tf, latch) in &mut characters {
        if input.external != Vec3::ZERO {
            input.external = Vec3::ZERO;
        }
        if input.locked {
            input.locked = false;
        }
        if input.face.is_some() {
            input.face = None;
        }

        if let Some(mut latch) = latch {
            match state.mode {
                MotionMode::Air | MotionMode::Glide if !latch.flown => latch.flown = true,
                MotionMode::Air | MotionMode::Glide => {}
                _ if latch.flown => {
                    intent.set_held(GLIDE, false);
                    commands.entity(entity).remove::<GlideUntilLanded>();
                }
                _ => {}
            }
        }

        let wish = flat(intent.move_dir);
        let wish = if wish.is_finite() { wish.clamp_length_max(1.0) } else { Vec3::ZERO };
        if input.move_dir != wish {
            input.move_dir = wish;
        }
        let (sprint, glide) = (intent.is_held(SPRINT), intent.is_held(GLIDE));
        if input.sprint != sprint {
            input.sprint = sprint;
        }
        if input.glide != glide {
            input.glide = glide;
        }
        if intent.pressed.iter().any(|p| p.input == JUMP) {
            while intent.take_press(JUMP).is_some() {}
            input.jump = true;
        }

        let toward = |point: Vec3| Some(flat(point - tf.translation)).filter(|d| d.is_finite() && d.length_squared() > 1e-4);
        let face = intent.aim.and_then(toward).or_else(|| {
            let target = index.get(intent.target?)?;
            toward(positions.get(target).ok()?.translation)
        });
        if face.is_some() {
            input.face = face;
        }
    }
}
