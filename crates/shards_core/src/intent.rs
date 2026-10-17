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
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Input names the engine modules understand. Games add their own names freely; a
/// press nobody consumes is dropped at the end of the tick.
pub mod input_names {
    /// Press: jump (or climb out of water).
    pub const JUMP: &str = "jump";
    /// Held: run faster.
    pub const SPRINT: &str = "sprint";
    /// Held in the air: glide.
    pub const GLIDE: &str = "glide";
    /// Press, with a direction: dodge roll.
    pub const DODGE: &str = "dodge";
    /// Press: light attack.
    pub const LIGHT: &str = "light";
    /// Press: heavy attack.
    pub const HEAVY: &str = "heavy";
    /// Held: block.
    pub const BLOCK: &str = "block";
    /// Press: parry.
    pub const PARRY: &str = "parry";
    /// Pressed with a slot suffix: `technique.1`, `technique.2`, ...
    pub const TECHNIQUE: &str = "technique";
    /// Press: use the selected item.
    pub const USE_ITEM: &str = "use_item";
    /// Press: talk, open, pick up.
    pub const INTERACT: &str = "interact";
    /// Press: lock onto a target, or let go.
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
    /// Input name: `dodge`, `light`, or a game's own.
    pub input: String,
    /// Direction on the ground plane, for inputs that have one.
    pub dir: Option<Vec3>,
}

/// Where an order goes.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Target {
    /// An entity, followed as it moves.
    Entity(AiId),
    /// A point in the world.
    Point(Vec3),
}

/// A multi-tick order, carried out by the fixed tick.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Order {
    /// Walk in a straight line until within `within` meters of the target
    /// ([`follow_orders`]).
    MoveTo {
        /// Where to walk.
        target: Target,
        /// Meters from the target that count as arrived.
        within: f32,
    },
    /// Approach and use `input` (e.g. "light") on the target; `repeat` until it dies.
    /// Carried out by the game's action module, not by [`follow_orders`].
    Attack {
        /// Whom to attack.
        target: AiId,
        /// The press that attacks, e.g. "light".
        input: String,
        /// Keep attacking until the target dies.
        repeat: bool,
    },
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

    /// Whether `input` is held right now.
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
///
/// `F` picks the actors: `()` in the simulation, `With<Predicted>` where a client
/// predicts its own body ([`NetSet::Predict`](crate::NetSet::Predict)): the same code.
pub fn follow_orders<F: QueryFilter + 'static>(
    index: Res<AiIndex>,
    positions: Query<&Transform>,
    mut actors: Query<(Entity, &mut Intent), F>,
) {
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

/// The engine rule `core.intent_sane`: every [`Intent`] has a finite `move_dir` no longer than
/// 1 (plus 1e-4 for rounding) and a finite `aim`.
///
/// `Intent` is the only way anything drives a character, and every writer clamps what it writes:
/// the keyboard and gamepad (`clamp_length_max(1.0)`), [`follow_orders`], the action module's
/// `approach`, the NPC brain, and the host's copy of a network client's frame (which sanitizes
/// it). The movement code is still built to survive garbage, so this rule does not guard the
/// controller; it names the writer that forgot to clamp. A test that writes garbage on
/// purpose to show the controller tolerates it turns the engine rules off
/// (`EngineConfig::engine_invariants`). The vertical part of `move_dir` is not judged, only
/// the length.
pub fn intent_sane(world: &World) -> Option<String> {
    let mut bad = ai_core::Offenders::default();
    ai_core::with_query::<(&AiId, &Intent), (), _>(world, |query| {
        for (id, intent) in query.iter(world) {
            let dir = intent.move_dir;
            if !dir.is_finite() || dir.length() > 1.0 + 1e-4 {
                bad.note(*id, || format!("Intent.move_dir = {dir} is not finite or longer than 1"));
            } else if let Some(aim) = intent.aim.filter(|a| !a.is_finite()) {
                bad.note(*id, || format!("Intent.aim = {aim} is not finite"));
            }
        }
    })?;
    bad.report()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_sane_names_a_move_that_is_too_long_or_not_finite() {
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        let walker = w.spawn((AiId(0), Intent { move_dir: Vec3::new(0.6, 0.0, 0.8), aim: Some(Vec3::X), ..default() })).id();
        assert_eq!(intent_sane(&w), None, "a full-length move is fine");
        w.get_mut::<Intent>(walker).unwrap().move_dir = Vec3::new(1.00005, 0.0, 0.0);
        assert_eq!(intent_sane(&w), None, "rounding is tolerated");
        w.get_mut::<Intent>(walker).unwrap().move_dir = Vec3::new(5.0, 0.0, 0.0);
        let text = intent_sane(&w).unwrap();
        assert!(text.starts_with("e1 Intent.move_dir = ") && text.contains("longer than 1"), "{text}");
        w.get_mut::<Intent>(walker).unwrap().move_dir = Vec3::NAN;
        assert!(intent_sane(&w).is_some());
        let mut intent = w.get_mut::<Intent>(walker).unwrap();
        intent.move_dir = Vec3::ZERO;
        intent.aim = Some(Vec3::new(f32::INFINITY, 0.0, 0.0));
        assert!(intent_sane(&w).unwrap().contains("Intent.aim"));
    }

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
