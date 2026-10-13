//! `Intent`: the only way anything drives a character.
//!
//! Keyboard, gamepad, agent tools, network clients and replays all write the same
//! component between ticks; gameplay only reads it (and consumes one-shot presses).
//! That keeps the AI honest (it plays with the controls a human has), and makes co-op,
//! bots and replays one architecture: the changes made to `Intent` between ticks are
//! the whole input of a replay.
//!
//! Two loops drive a character. A slow one (a human, or an agent calling `shards` every
//! few seconds) chooses *what*: [`Order`]s such as "walk to the pillar". The fixed tick
//! carries it out every tick ([`follow_orders`] steers, the action module attacks).

use std::collections::VecDeque;

use ai_core::{AiId, AiIndex};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Input names the engine modules understand. Games add their own names freely; a
/// press nobody consumes is dropped at the end of the tick.
pub mod input_names {
    pub const JUMP: &str = "jump";
    pub const SPRINT: &str = "sprint";
    pub const GLIDE: &str = "glide";
    pub const DODGE: &str = "dodge";
    pub const LIGHT: &str = "light";
    pub const HEAVY: &str = "heavy";
    pub const BLOCK: &str = "block";
    pub const PARRY: &str = "parry";
    /// Pressed with a slot suffix: `technique.1`, `technique.2`, ...
    pub const TECHNIQUE: &str = "technique";
    pub const USE_ITEM: &str = "use_item";
    pub const INTERACT: &str = "interact";
    pub const LOCK_ON: &str = "lock_on";
}

/// Everything that drives a character. Keyboard, gamepad, agent tools, network clients
/// and replays write it between ticks; gameplay only reads it (and consumes one-shot
/// presses). Registered as a replay input: what changes between ticks is recorded.
#[derive(Component, Reflect, Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Component, Default)]
#[serde(default)]
pub struct Intent {
    /// Desired movement on the ground plane (y = 0), length <= 1. Magnitude: <= 0.5
    /// walk, 1 run.
    pub move_dir: Vec3,
    /// Point to face (attacks go where you face).
    pub aim: Option<Vec3>,
    /// Lock-on target (camera + facing); `None`: free.
    pub target: Option<AiId>,
    /// Buttons held right now, sorted, unique: "sprint", "block", "glide", ...
    pub held: Vec<String>,
    /// One-shot presses since the last tick, in order. Gameplay consumes them during the
    /// tick ([`take_press`](Self::take_press)); whatever is left is cleared at the end of
    /// the tick, so a press never lingers into a later one.
    pub pressed: Vec<Press>,
    /// Multi-tick orders (agent conveniences): walk to, attack a target...
    pub orders: VecDeque<Order>,
}

/// One button press, with an optional direction (a dodge to the west).
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Press {
    pub input: String,
    pub dir: Option<Vec3>,
}

/// Where an order goes.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    Entity(AiId),
    Point(Vec3),
}

/// A multi-tick order, carried out by the fixed tick.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Order {
    /// Walk in a straight line until within `within` meters of the target
    /// ([`follow_orders`]).
    MoveTo { target: Target, within: f32 },
    /// Approach and use `input` (e.g. "light") on the target; `repeat` until it dies.
    /// Carried out by the game's action module, not by [`follow_orders`].
    Attack { target: AiId, input: String, repeat: bool },
}

impl Intent {
    /// A one-shot press of `input` this tick.
    pub fn press(&mut self, input: impl Into<String>) {
        self.pressed.push(Press { input: input.into(), dir: None });
    }

    /// A one-shot press of `input` towards `dir` (a dodge, a throw).
    pub fn press_dir(&mut self, input: impl Into<String>, dir: Vec3) {
        self.pressed.push(Press { input: input.into(), dir: Some(dir) });
    }

    /// Takes the oldest unconsumed press of `input`.
    pub fn take_press(&mut self, input: &str) -> Option<Press> {
        let at = self.pressed.iter().position(|p| p.input == input)?;
        Some(self.pressed.remove(at))
    }

    pub fn is_held(&self, input: &str) -> bool {
        self.held.binary_search_by(|h| h.as_str().cmp(input)).is_ok()
    }

    /// Holds or releases `input`; [`held`](Self::held) stays sorted and unique.
    pub fn set_held(&mut self, input: &str, on: bool) {
        match (self.held.binary_search_by(|h| h.as_str().cmp(input)), on) {
            (Err(at), true) => self.held.insert(at, input.to_owned()),
            (Ok(at), false) => {
                self.held.remove(at);
            }
            _ => {}
        }
    }
}

impl Order {
    /// `move_to e6`, `move_to (3.0, -4.0)`, `attack e6 (repeat)`: for observations.
    pub fn describe(&self) -> String {
        match self {
            Order::MoveTo { target: Target::Entity(id), .. } => format!("move_to {id}"),
            Order::MoveTo { target: Target::Point(p), .. } => format!("move_to ({:.1}, {:.1})", p.x, p.z),
            Order::Attack { target, repeat: true, .. } => format!("attack {target} (repeat)"),
            Order::Attack { target, .. } => format!("attack {target}"),
        }
    }
}

/// `order move_to e6 (+1)`: the front order, for the AI's line about the entity.
pub(crate) fn describe_orders(entity: EntityRef) -> Option<String> {
    let intent = entity.get::<Intent>()?;
    let first = intent.orders.front()?;
    Some(match intent.orders.len() - 1 {
        0 => format!("order {}", first.describe()),
        more => format!("order {} (+{more})", first.describe()),
    })
}

/// `v` on the ground plane (y = 0).
pub fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// `SimSet::Input`: steers towards the front [`Order::MoveTo`] in a straight line and
/// pops it on arrival (or when its target entity is gone). Other orders are left to
/// the modules that understand them.
pub fn follow_orders(index: Res<AiIndex>, positions: Query<&Transform>, mut actors: Query<(Entity, &mut Intent)>) {
    for (me, mut intent) in &mut actors {
        let Some(Order::MoveTo { target, within }) = intent.orders.front().cloned() else { continue };
        let Ok(here) = positions.get(me).map(|t| t.translation) else { continue };
        let goal = match target {
            Target::Point(p) => Some(p),
            Target::Entity(id) => index.get(id).and_then(|e| positions.get(e).ok()).map(|t| t.translation),
        };
        match goal {
            Some(goal) if flat(goal - here).length() > within => intent.move_dir = flat(goal - here).normalize_or_zero(),
            _ => {
                intent.move_dir = Vec3::ZERO;
                intent.orders.pop_front();
            }
        }
    }
}

/// `SimSet::Cleanup`: presses nobody consumed this tick are dropped. Inside the tick,
/// so it is deterministic and never recorded as input.
pub fn clear_presses(mut intents: Query<&mut Intent>) {
    for mut intent in &mut intents {
        if !intent.pressed.is_empty() {
            intent.pressed.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_stays_sorted_and_unique() {
        let mut intent = Intent::default();
        intent.set_held("sprint", true);
        intent.set_held("block", true);
        intent.set_held("sprint", true);
        assert_eq!(intent.held, ["block", "sprint"]);
        assert!(intent.is_held("block") && !intent.is_held("glide"));
        intent.set_held("block", false);
        intent.set_held("glide", false);
        assert_eq!(intent.held, ["sprint"]);
    }

    #[test]
    fn presses_are_taken_oldest_first() {
        let mut intent = Intent::default();
        intent.press("light");
        intent.press_dir("dodge", Vec3::X);
        intent.press("light");
        assert_eq!(intent.take_press("dodge"), Some(Press { input: "dodge".into(), dir: Some(Vec3::X) }));
        assert_eq!(intent.take_press("dodge"), None);
        assert!(intent.take_press("light").is_some());
        assert_eq!(intent.pressed.len(), 1);
    }

    #[test]
    fn intent_round_trips_through_json() {
        let mut intent = Intent { move_dir: Vec3::X, target: Some(AiId(6)), ..default() };
        intent.press("jump");
        intent.orders.push_back(Order::Attack { target: AiId(6), input: "light".into(), repeat: true });
        let json = serde_json::to_value(&intent).unwrap();
        assert_eq!(serde_json::from_value::<Intent>(json).unwrap(), intent);
        assert_eq!(serde_json::from_value::<Intent>(serde_json::json!({})).unwrap(), Intent::default());
    }
}
