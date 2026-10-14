//! `Order::Attack`: walk into reach of the action the order's input starts, face the
//! target, press the input whenever it can start; with `repeat`, until the target dies
//! (cancel windows chain a combo by themselves). Only through `Intent`, like a player.
//! The order owns the aim point and, against another lock-on, the lock-on while it
//! lasts; an order whose input can never start by waiting (a raised guard, no charges)
//! is dropped with the reason in the journal instead of sitting there.

use ai_core::AiIndex;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent, Order};
use shards_motion::Collider;

use crate::def::{ActionDef, MovesetDef};
use crate::fighter::{Charges, Cooldowns, Dead, Fighter, Stamina};
use crate::machine::{can_start, ActionRejected, ActionState, FighterView};

/// Walks this close (a share of the action's reach) before attacking.
const APPROACH: f32 = 0.9;
/// Reach of an action without hit windows (m).
const MIN_REACH: f32 = 1.0;

type Attackers<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Fighter,
        &'static mut Intent,
        &'static ActionState,
        &'static Transform,
        Option<&'static Stamina>,
        Option<&'static Cooldowns>,
        Option<&'static Charges>,
        Has<Dead>,
    ),
>;

/// `SimSet::Input`, after core's `follow_orders`: carries out the front
/// `Order::Attack` of every fighter. Each fighter only writes its own `Intent`.
pub(crate) fn follow_attack_orders(
    mut commands: Commands,
    time: Res<Time>,
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    targets: Query<(&Transform, Has<Dead>, Option<&Collider>)>,
    mut fighters: Attackers,
) {
    let dt = time.delta_secs();
    for (entity, fighter, mut intent, state, tf, stamina, cooldowns, charges, dead) in &mut fighters {
        let Some(order) = intent.orders.front().cloned() else { continue };
        let Order::Attack { target, ref input, repeat } = order else { continue };
        if dead {
            continue;
        }
        let goal = index.get(target).and_then(|e| targets.get(e).ok()).filter(|(_, target_dead, _)| !target_dead);
        let Some((target_tf, _, collider)) = goal else {
            end(&mut intent);
            // The lock-on to a corpse goes with the order.
            if intent.target == Some(target) {
                intent.target = None;
            }
            continue;
        };
        let moveset = db.get::<MovesetDef>(&fighter.moveset);
        let reach = moveset
            .and_then(|m| m.action(input))
            .and_then(|a| db.get::<ActionDef>(a))
            .map_or(MIN_REACH, |d| d.reach().max(MIN_REACH));
        let radius = collider.map_or(0.0, |c| c.shape.half_extents().x);
        let to = flat(target_tf.translation - tf.translation);
        if intent.aim != Some(target_tf.translation) {
            intent.aim = Some(target_tf.translation);
        }
        // Attacks and facing follow a lock-on before the aim point: the order's target
        // takes it over, or the order would swing at the wrong enemy forever.
        if intent.target.is_some_and(|locked| locked != target) {
            intent.target = Some(target);
        }
        if to.length() > reach * APPROACH + radius {
            let dir = to.normalize_or_zero();
            if intent.move_dir != dir {
                intent.move_dir = dir;
            }
            continue;
        }
        if intent.move_dir != Vec3::ZERO {
            intent.move_dir = Vec3::ZERO;
        }
        let view = FighterView {
            db: &db,
            moveset: &fighter.moveset,
            state,
            stamina,
            cooldowns,
            charges,
            held: &intent.held,
            dead,
        };
        match can_start(&view, input, dt) {
            Ok(_) => {
                intent.press(input.clone());
                // A one-shot swing keeps the aim point: it tracks it while it winds up.
                if !repeat {
                    intent.orders.pop_front();
                }
            }
            // Waiting will not do (a raised guard, no charges, never enough stamina):
            // say so rather than sit silently at the front.
            Err(blocker) if blocker.ready_in().is_none() => {
                let reason = format!("order {} dropped: {}", order.describe(), blocker.message());
                commands.trigger(ActionRejected { entity, input: input.clone(), reason });
                end(&mut intent);
            }
            Err(_) => {}
        }
    }
}

/// The front order is over: it stops walking and its aim point goes.
fn end(intent: &mut Intent) {
    intent.orders.pop_front();
    if intent.aim.is_some() {
        intent.aim = None;
    }
    if intent.move_dir != Vec3::ZERO {
        intent.move_dir = Vec3::ZERO;
    }
}
