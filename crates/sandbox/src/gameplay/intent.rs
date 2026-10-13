//! `Intent`: the only way anything drives a character.
//!
//! Keyboard, network client, AI agent and replay all write the same component; the
//! gameplay systems only ever read it. That keeps the AI honest (it plays with the
//! same controls as a human) and makes co-op, bots and replays one architecture.

use std::collections::VecDeque;

use ai_core::prelude::*;
use ai_core::AiIndex;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::combat::{Action, AttackKind};

/// Per-tick controls plus a queue of multi-tick orders.
///
/// Two loops: a slow one (a human, or an agent calling `shards` every few seconds) chooses *what*
/// (`orders`: "walk to the pillar", "attack e42"), the fast tick loop carries it out
/// every tick (steering, attacking when in range).
///
/// Being the only input also makes it the whole of a replay: the changes made to it
/// between ticks are recorded (`replay_input::<Intent>()`), and setting them again at
/// the same ticks replays the game.
#[derive(Component, Reflect, Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct Intent {
    /// Desired ground-plane movement, length ≤ 1.
    pub move_dir: Vec3,
    /// Point to face (attacks go where you face).
    pub aim: Option<Vec3>,
    /// One-shot: start this attack on the next tick (consumed).
    pub attack: Option<AttackKind>,
    /// One-shot: dodge in this direction on the next tick (consumed).
    pub dodge: Option<Vec3>,
    pub orders: VecDeque<Order>,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    Entity(AiId),
    Point(Vec3),
}

#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Order {
    /// Walk until within `within` meters of the target.
    MoveTo { target: Target, within: f32 },
    /// Approach and attack; `repeat` keeps attacking until the target dies.
    Attack { target: AiId, repeat: bool },
}

impl Order {
    pub fn describe(&self) -> String {
        match self {
            Order::MoveTo { target: Target::Entity(id), .. } => format!("move_to {id}"),
            Order::MoveTo { target: Target::Point(p), .. } => format!("move_to ({:.1}, {:.1})", p.x, p.z),
            Order::Attack { target, repeat: true } => format!("attack {target} (repeat)"),
            Order::Attack { target, .. } => format!("attack {target}"),
        }
    }
}

/// Fast loop: turns the front order into per-tick controls.
pub fn follow_orders(
    index: Res<AiIndex>,
    positions: Query<(&Transform, Option<&Action>)>,
    mut actors: Query<(Entity, &mut Intent, &Action)>,
) {
    for (me, mut intent, action) in &mut actors {
        if intent.orders.is_empty() || matches!(action, Action::Dead { .. }) {
            continue;
        }
        let Ok((my_tf, _)) = positions.get(me) else { continue };
        let here = my_tf.translation;
        let resolve = |t: Target| match t {
            Target::Point(p) => Some(p),
            Target::Entity(id) => index
                .get(id)
                .and_then(|e| positions.get(e).ok())
                .filter(|(_, a)| !matches!(a, Some(Action::Dead { .. })))
                .map(|(tf, _)| tf.translation),
        };
        let order = intent.orders.front().cloned();
        match order {
            Some(Order::MoveTo { target, within }) => match resolve(target) {
                Some(goal) if flat(goal - here).length() > within => {
                    intent.move_dir = flat(goal - here).normalize_or_zero();
                }
                _ => {
                    intent.move_dir = Vec3::ZERO;
                    intent.orders.pop_front();
                }
            },
            Some(Order::Attack { target, repeat }) => match resolve(Target::Entity(target)) {
                None => {
                    intent.move_dir = Vec3::ZERO;
                    intent.orders.pop_front();
                }
                Some(goal) => {
                    let range = AttackKind::Sword.spec().reach() * 0.85;
                    if flat(goal - here).length() > range {
                        intent.move_dir = flat(goal - here).normalize_or_zero();
                    } else {
                        intent.move_dir = Vec3::ZERO;
                        intent.aim = Some(goal);
                        if matches!(action, Action::Idle) {
                            intent.attack = Some(AttackKind::Sword);
                            if !repeat {
                                intent.orders.pop_front();
                            }
                        }
                    }
                }
            },
            None => {}
        }
    }
}

pub fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}
