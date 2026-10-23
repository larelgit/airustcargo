//! `Intent` → [`LocomotionInput`]: how what a character wants (keyboard, agent, brain,
//! replay) becomes controller input.
//!
//! Runs in `SimSet::Act`, first: the action machine orders its systems
//! `.after(MotionSystems::Intent)` and then owns `external`, `locked`, `face` and may take
//! `sprint` away (no stamina). Nothing here resets those after it.

use ai_core::{AiAppExt, AiIndex, SimSet};
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::input_names::{GLIDE, JUMP, SPRINT};
use shards_core::{flat, follow_orders, Intent, NetSet, Predicted};

use crate::controller::{CharacterController, CharacterParams, LocomotionInput, MotionMode, MotionState};

/// The motion systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MotionSystems {
    /// `SimSet::Act`, before everything that reads `Intent` (the lock-on switch, [`MotionSystems::Intent`],
    /// the weapons, the action machine): the control lock (F12) takes what it locks from `Intent` for the
    /// tick and holds the look inside its cones ([`control_lock`](crate::control_lock)).
    Lock,
    /// `FixedLast`, before the replay check: the control lock gives back what it took, so `Intent`
    /// between ticks is what the world wrote.
    Unlock,
    /// `SimSet::Act`: [`intent_to_locomotion`] fills [`LocomotionInput`] from `Intent`
    /// (`move_dir`, held `sprint` and `glide`, the `jump` press, facing).
    Intent,
    /// `SimSet::Input`, after `CoreSystems::FollowOrders`: [`end_stalled_orders`](crate::end_stalled_orders)
    /// ends a `move_to` order that makes no progress. The action module steers its `Order::Attack`
    /// after it.
    StallOrders,
    /// `NetSet::Predict`, first: `follow_orders` for a client's own body ([`Predicted`]).
    /// The action module steers its `Order::Attack` after it.
    PredictOrders,
    /// `NetSet::Predict`, after [`MotionSystems::PredictOrders`]: [`intent_to_locomotion`]
    /// for a client's own body. The action machine's prediction comes after it, the
    /// controller's ([`ControllerSystems::PredictMove`](crate::ControllerSystems::PredictMove)) last.
    PredictIntent,
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
        // The latch is the own body's on a client (its `glide` tool runs there).
        .net_predicted::<GlideUntilLanded>()
        .configure_sets(FixedUpdate, MotionSystems::Intent.in_set(SimSet::Act))
        .configure_sets(FixedUpdate, (MotionSystems::PredictOrders, MotionSystems::PredictIntent).chain().in_set(NetSet::Predict))
        .add_systems(
            FixedUpdate,
            (
                intent_to_locomotion::<()>.in_set(MotionSystems::Intent),
                follow_orders::<With<Predicted>>.in_set(MotionSystems::PredictOrders),
                intent_to_locomotion::<With<Predicted>>.in_set(MotionSystems::PredictIntent),
            ),
        );
}

/// What [`intent_to_locomotion`] reads and writes of a character.
type Steered = (
    Entity,
    &'static mut Intent,
    &'static mut LocomotionInput,
    &'static MotionState,
    &'static Transform,
    Option<&'static mut GlideUntilLanded>,
    Option<&'static CharacterController>,
    Option<&'static CharacterParams>,
);

/// For every character with `Intent` and `LocomotionInput`: clears what the last tick's
/// actions left (`external`, `locked`, `face`), then takes
///
/// - `move_dir` from `Intent.move_dir` (flattened, at most length 1),
/// - `sprint` and `glide` from the held buttons of the same names,
/// - `jump` from the `jump` presses (consumed),
/// - `face` toward the yaw of `Intent.look` when the body's `locomotion` says `face: Look`, else
///   toward `Intent.aim`, else toward the lock-on `Intent.target`.
///
/// Dead or stunned characters are the action machine's business: it sets `locked`.
///
/// `F` picks the characters: `()` in the simulation, `With<Predicted>` where a client
/// predicts its own body.
pub fn intent_to_locomotion<F: QueryFilter + 'static>(
    mut commands: Commands,
    index: Res<AiIndex>,
    db: Option<Res<ContentDb>>,
    positions: Query<&Transform>,
    mut characters: Query<Steered, F>,
) {
    for (entity, mut intent, mut input, state, tf, latch, controller, own) in &mut characters {
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
        // A body that faces its look faces it before anything else: the look is where it aims.
        let looked = controller.and_then(|c| c.resolve(own, db.as_deref()).bound_look(&intent)).map(|gaze| gaze.flat_dir());
        let face = looked.or_else(|| intent.aim.and_then(toward)).or_else(|| {
            let target = index.get(intent.target?)?;
            toward(positions.get(target).ok()?.translation)
        });
        if face.is_some() {
            input.face = face;
        }
    }
}
