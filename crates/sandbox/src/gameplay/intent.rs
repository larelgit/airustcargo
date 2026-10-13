//! The engine's `Intent` drives every character here: keyboard, agent tools, brains
//! and replays write it, gameplay reads it. The engine walks `Order::MoveTo`; attack
//! orders are this game's ([`attack_orders`]) until the engine's action module takes
//! them over.

use shards::ai_core::AiIndex;
use shards::prelude::*;

pub use shards::core::{flat, Intent, Order, Press, Target};

use super::combat::{Action, AttackKind};

/// `SimSet::Input`, after the engine's orders: the front `Order::Attack` walks into
/// range of its target and presses its attack (every tick while idle when `repeat`,
/// once otherwise); it ends when the target dies or is gone.
pub fn attack_orders(
    index: Res<AiIndex>,
    attacks: Res<ContentDb>,
    positions: Query<(&Transform, Option<&Action>)>,
    mut actors: Query<(Entity, &mut Intent, &Action)>,
) {
    for (me, mut intent, action) in &mut actors {
        if matches!(action, Action::Dead { .. }) {
            continue;
        }
        let Some(Order::Attack { target, input, repeat }) = intent.orders.front().cloned() else { continue };
        let Ok((my_tf, _)) = positions.get(me) else { continue };
        let here = my_tf.translation;
        let goal = index
            .get(target)
            .and_then(|e| positions.get(e).ok())
            .filter(|(_, a)| !matches!(a, Some(Action::Dead { .. })))
            .map(|(tf, _)| tf.translation);
        let Some(goal) = goal else {
            intent.move_dir = Vec3::ZERO;
            intent.orders.pop_front();
            continue;
        };
        let reach = AttackKind::from_id(&input).map_or(0.0, |kind| kind.def(&attacks).reach());
        if flat(goal - here).length() > reach * 0.85 {
            intent.move_dir = flat(goal - here).normalize_or_zero();
            continue;
        }
        intent.move_dir = Vec3::ZERO;
        intent.aim = Some(goal);
        if matches!(action, Action::Idle) {
            intent.press(input);
            if !repeat {
                intent.orders.pop_front();
            }
        }
    }
}
