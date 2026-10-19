//! `Order::Attack`: walk into reach of the action the order's input starts, face the
//! target, press the input whenever it can start; with `repeat`, until the target dies
//! (cancel windows chain a combo by themselves). Only through `Intent`, like a player.
//! The order owns the aim point and, against another lock-on, the lock-on while it
//! lasts; an order whose input can never start by waiting (a raised guard, no charges)
//! is dropped with the reason in the journal instead of sitting there.

use ai_core::{AiId, AiIndex};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent, Order, Predicted};
use shards_motion::{CharacterController, CharacterParams, Collider, MotionState, PhysicsWorld};

use crate::def::{ActionDef, MovesetDef};
use crate::combat::Downed;
use crate::fighter::{Charges, Cooldowns, Dead, Fighter, Stamina};
use crate::context::ContextInputs;
use crate::leap::{radius_of, LeapSite};
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
        Option<&'static MotionState>,
        Option<&'static CharacterController>,
        Option<&'static CharacterParams>,
        Has<Dead>,
        Has<Downed>,
        Option<&'static ContextInputs>,
    ),
>;

/// Where an order's target stands, whether it is dead, its body, whether it is down.
type Target = (&'static Transform, Has<Dead>, Option<&'static Collider>, Has<Downed>);

/// A client's predicted body: its fighter, intent, place, whether it is dead and whether it is down.
type Walker = (&'static Fighter, &'static mut Intent, &'static Transform, Has<Dead>, Has<Downed>);

/// `SimSet::Input`, after core's `follow_orders`: carries out the front
/// `Order::Attack` of every fighter. Each fighter only writes its own `Intent`.
pub(crate) fn follow_attack_orders(
    mut commands: Commands,
    time: Res<Time>,
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    physics: Res<PhysicsWorld>,
    targets: Query<Target>,
    mut fighters: Attackers,
) {
    let dt = time.delta_secs();
    for (entity, fighter, mut intent, state, tf, stamina, cooldowns, charges, motion, controller, params, dead, downed, context) in &mut fighters {
        let Some(order) = intent.orders.front().cloned() else { continue };
        let Order::Attack { target, ref input, repeat } = order else { continue };
        if dead || downed {
            continue;
        }
        let goal = index.get(target).and_then(|e| targets.get(e).ok()).filter(|(_, target_dead, _, target_down)| !target_dead && !target_down);
        let Some((target_tf, _, collider, _)) = goal else {
            end(&mut intent);
            // The lock-on to a corpse goes with the order.
            if intent.target == Some(target) {
                intent.target = None;
            }
            continue;
        };
        if !approach(&db, fighter, &mut intent, tf.translation, target, input, target_tf.translation, collider) {
            continue;
        }
        let site = controller.map(|c| {
            let target = Some((target_tf.translation, radius_of(collider)));
            LeapSite::body(&physics, entity, (tf.translation, *tf.forward()), c.resolve(params, Some(&db)), &intent, target)
        });
        let view = FighterView {
            stamina,
            cooldowns,
            charges,
            mode: motion.map(|m| m.mode),
            off_ground: crate::machine::off_ground(motion),
            leap: site,
            context,
            ..FighterView::new(&db, &fighter.moveset, state, &intent.held, dead)
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

/// Aims at the order's target (`there`) and takes over the lock-on, walks towards it until
/// in reach of the action `input` starts: whether it is in reach (and stands).
#[allow(clippy::too_many_arguments)]
fn approach(
    db: &ContentDb,
    fighter: &Fighter,
    intent: &mut Intent,
    here: Vec3,
    target: AiId,
    input: &str,
    there: Vec3,
    collider: Option<&Collider>,
) -> bool {
    let reach = db
        .get::<MovesetDef>(&fighter.moveset)
        .and_then(|m| m.action(input))
        .and_then(|a| db.get::<ActionDef>(a))
        .map_or(MIN_REACH, |d| d.reach().max(MIN_REACH));
    let radius = collider.map_or(0.0, |c| c.shape.half_extents().x);
    let to = flat(there - here);
    if intent.aim != Some(there) {
        intent.aim = Some(there);
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
        return false;
    }
    if intent.move_dir != Vec3::ZERO {
        intent.move_dir = Vec3::ZERO;
    }
    true
}

/// `NetSet::Predict` (`ActionSystems::PredictOrders`): a client's own body walks into reach
/// of its front `Order::Attack` as the host walks it, instead of waiting for the host's
/// state. Only the walk: the presses, a dropped order and its end are the host's (its state
/// brings the swings, which the prediction adopts, and the order queue), and what this
/// writes into `Intent` is undone after the tick (`NetSet::PredictEnd`).
pub(crate) fn steer_attack_orders(
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    targets: Query<Target>,
    mut fighters: Query<Walker, With<Predicted>>,
) {
    for (fighter, mut intent, tf, dead, downed) in &mut fighters {
        let Some(Order::Attack { target, input, .. }) = intent.orders.front().cloned() else { continue };
        if dead || downed {
            continue;
        }
        let goal = index.get(target).and_then(|e| targets.get(e).ok()).filter(|(_, target_dead, _, target_down)| !target_dead && !target_down);
        match goal {
            Some((there, _, collider, _)) => {
                approach(&db, fighter, &mut intent, tf.translation, target, &input, there.translation, collider);
            }
            None => end(&mut intent),
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
