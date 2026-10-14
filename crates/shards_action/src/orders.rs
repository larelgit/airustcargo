//! `Order::Attack`: walk into reach of the action the order's input starts, face the
//! target, press the input whenever it can start; with `repeat`, until the target dies
//! (cancel windows chain a combo by themselves). Only through `Intent`, like a player.

use ai_core::AiIndex;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent, Order};
use shards_motion::Collider;

use crate::def::{ActionDef, MovesetDef};
use crate::fighter::{Charges, Cooldowns, Dead, Fighter, Stamina};
use crate::machine::{can_start, ActionState, FighterView};

/// Walks this close (a share of the action's reach) before attacking.
const APPROACH: f32 = 0.9;
/// Reach of an action without hit windows (m).
const MIN_REACH: f32 = 1.0;

type Attackers<'w, 's> = Query<
    'w,
    's,
    (
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
    time: Res<Time>,
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    targets: Query<(&Transform, Has<Dead>, Option<&Collider>)>,
    mut fighters: Attackers,
) {
    let dt = time.delta_secs();
    for (fighter, mut intent, state, tf, stamina, cooldowns, charges, dead) in &mut fighters {
        let Some(Order::Attack { target, input, repeat }) = intent.orders.front().cloned() else { continue };
        if dead {
            continue;
        }
        let goal = index.get(target).and_then(|e| targets.get(e).ok()).filter(|(_, target_dead, _)| !target_dead);
        let Some((target_tf, _, collider)) = goal else {
            intent.orders.pop_front();
            if intent.move_dir != Vec3::ZERO {
                intent.move_dir = Vec3::ZERO;
            }
            continue;
        };
        let moveset = db.get::<MovesetDef>(&fighter.moveset);
        let reach = moveset
            .and_then(|m| m.action(&input))
            .and_then(|a| db.get::<ActionDef>(a))
            .map_or(MIN_REACH, |d| d.reach().max(MIN_REACH));
        let radius = collider.map_or(0.0, |c| c.shape.half_extents().x);
        let to = flat(target_tf.translation - tf.translation);
        if intent.aim != Some(target_tf.translation) {
            intent.aim = Some(target_tf.translation);
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
        if can_start(&view, &input, dt).is_ok() {
            intent.press(input);
            if !repeat {
                intent.orders.pop_front();
            }
        }
    }
}
