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
    /// Press, with `dir.x` the side (`+1` right, `-1` left): switch the lock-on target to the
    /// next one on that side (the action module's lock-on rules).
    pub const LOCK_SWITCH: &str = "lock_switch";
    /// Held: the attack assist is off (an action with an `assist` does not track the lock-on target).
    /// The controls hold it while the player has turned the assist off (`controls.attack_assist`).
    pub const ASSIST_OFF: &str = "assist_off";
    /// Press: reload the weapon in hand. An ordinary moveset input (`"reload": "pistol_reload"`): the
    /// moveset names the action, the `reload` tool presses it.
    pub const RELOAD: &str = "reload";
    /// Pressed with a slot suffix: `weapon.slot.1`, `weapon.slot.2`, ...: switch to the weapon in that
    /// slot of the body's `Arsenal` (the action module's weapon rules). Like `technique.N`.
    pub const WEAPON_SLOT: &str = "weapon.slot";
    /// Press: switch to the next weapon of the `Arsenal` (slot order, wrapping round). A game without
    /// lock-on binds the mouse wheel to this and [`WEAPON_PREV`]; the standard map binds it to `lock_switch`.
    pub const WEAPON_NEXT: &str = "weapon.next";
    /// Press: switch to the previous weapon of the `Arsenal`.
    pub const WEAPON_PREV: &str = "weapon.prev";
    /// The input a cancel window names to let a weapon switch through: `cancels: [(span: ..., into:
    /// [Input("weapon")])]` (or `Any`). A switch waits for such a window, or for the action to end.
    pub const WEAPON: &str = "weapon";
}

/// Where a character looks (F1a): a yaw and a pitch, in radians. It is the view of a character
/// that looks (a first-person camera, an agent's `look` tool, a turret's aim) written into
/// [`Intent::look`]; the simulation reads it, never a camera.
///
/// The axes are the engine's: **yaw 0 looks along -Z** (north, the way `move_dir` turns by a
/// camera yaw: `Quat::from_rotation_y(yaw) * -Z`), and a positive yaw turns left (towards -X);
/// **pitch 0 is level and a positive pitch looks up**. [`dir`](Self::dir) is the unit vector,
/// [`toward`](Self::toward) the angles from one point to another, and the two are each other's
/// inverse. The body's facing on the ground plane is the same yaw
/// ([`flat_dir`](Self::flat_dir)).
///
/// The type is named `Gaze`, not `Look`: `shards_look::Look` is a component prefabs and levels
/// name by its short name, and a second reflected type of that name would make `"Look": ("…")`
/// ambiguous.
#[derive(Reflect, Default, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Default)]
#[serde(default)]
pub struct Gaze {
    /// Turn about Y, radians. 0 looks along -Z; positive turns left (towards -X).
    pub yaw: f32,
    /// Up and down, radians. 0 is level, positive looks up. A reader clamps it to the pitch limits
    /// of the body's `locomotion` (default -89..89 degrees); the writer need not.
    pub pitch: f32,
}

impl Gaze {
    /// A look at `yaw` and `pitch`, in radians.
    pub const fn new(yaw: f32, pitch: f32) -> Self {
        Self { yaw, pitch }
    }

    /// The same, from degrees (what the `look` tool and the content speak).
    pub fn from_degrees(yaw: f32, pitch: f32) -> Self {
        Self::new(yaw.to_radians(), pitch.to_radians())
    }

    /// Whether both angles are finite numbers.
    pub fn is_finite(self) -> bool {
        self.yaw.is_finite() && self.pitch.is_finite()
    }

    /// The unit vector this looks along. `(0, 0)` is -Z, a positive pitch tilts it up.
    pub fn dir(self) -> Vec3 {
        let (sin_yaw, cos_yaw) = ops::sin_cos(self.yaw);
        let (sin_pitch, cos_pitch) = ops::sin_cos(self.pitch);
        Vec3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch)
    }

    /// The unit vector of the yaw alone, on the ground plane: the direction a body that faces
    /// this look faces.
    pub fn flat_dir(self) -> Vec3 {
        let (sin_yaw, cos_yaw) = ops::sin_cos(self.yaw);
        Vec3::new(-sin_yaw, 0.0, -cos_yaw)
    }

    /// The look from `from` to `to`; `None` when they are the same point or either is not
    /// finite. Straight up or down (no horizontal distance) has no yaw: it is 0 there, with a
    /// pitch of plus or minus a quarter turn.
    pub fn toward(from: Vec3, to: Vec3) -> Option<Self> {
        let d = to - from;
        if !d.is_finite() || d.length_squared() <= 1e-12 {
            return None;
        }
        let across = Vec2::new(d.x, d.z).length();
        let yaw = if across <= 1e-6 { 0.0 } else { ops::atan2(-d.x, -d.z) };
        Some(Self { yaw, pitch: ops::atan2(d.y, across) })
    }

    /// The same look with its pitch held between `min` and `max` (radians).
    pub fn with_pitch_between(self, min: f32, max: f32) -> Self {
        Self { yaw: self.yaw, pitch: self.pitch.clamp(min.min(max), max) }
    }
}

/// Everything that drives a character. Keyboard, gamepad, agent tools, network clients
/// and replays write it between ticks; gameplay only reads it (and consumes one-shot
/// presses). Registered as a replay input: what changes between ticks is recorded.
///
/// Reflected and hashed through its own serde form (`Serialize`, `Deserialize` as reflect type
/// data), which leaves `look` out while it is `None`: a replay check, a snapshot or a recorded
/// input of a run that never looks is byte for byte what it was before the field existed.
#[derive(Component, Reflect, Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Component, Default, Serialize, Deserialize)]
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
    /// Where the character looks (F1a); `None`: it does not look (every character that never
    /// writes it, and the third-person ones). A body whose `locomotion` says `face: Look` turns to
    /// its yaw every tick and steers its actions by it; `shards_motion::look_dir` is the
    /// direction, clamped to the body's pitch limits, and `shards_motion::eye_of` the point it
    /// looks from. Written between ticks by a first-person camera or the `look` tool; recorded
    /// like every other input.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[reflect(default)]
    pub look: Option<Gaze>,
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

/// `look W yaw 90.0° pitch 10.0°`: where the character looks, for the AI's line about the entity
/// (the heading as a compass point, the angles as the `look` tool takes them).
pub(crate) fn describe_look(entity: EntityRef) -> Option<String> {
    let look = entity.get::<Intent>()?.look.filter(|l| l.is_finite())?;
    Some(format!("look {} yaw {:.1}° pitch {:.1}°", ai_core::spatial::compass(look.dir()), look.yaw.to_degrees(), look.pitch.to_degrees()))
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
/// 1 (plus 1e-4 for rounding), a finite `aim` and a finite `look`.
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
            } else if let Some(look) = intent.look.filter(|l| !l.is_finite()) {
                bad.note(*id, || format!("Intent.look = {look:?} is not finite"));
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

    #[test]
    fn gaze_axes_are_the_engines() {
        use std::f32::consts::FRAC_PI_2;
        let near = |a: Vec3, b: Vec3| a.distance(b) < 1e-5;
        assert!(near(Gaze::new(0.0, 0.0).dir(), Vec3::NEG_Z), "yaw 0 looks along -Z");
        assert!(near(Gaze::new(FRAC_PI_2, 0.0).dir(), Vec3::NEG_X), "a positive yaw turns left");
        assert!(near(Gaze::new(-FRAC_PI_2, 0.0).dir(), Vec3::X));
        assert!(near(Gaze::new(std::f32::consts::PI, 0.0).dir(), Vec3::Z));
        assert!(near(Gaze::new(0.0, FRAC_PI_2).dir(), Vec3::Y), "a positive pitch looks up");
        assert!(near(Gaze::new(0.7, -FRAC_PI_2).dir(), Vec3::NEG_Y));
        // The body's yaw is the move axes' turn by the same angle (`Quat::from_rotation_y`).
        for yaw in [-3.0_f32, -1.0, 0.0, 0.4, 2.5] {
            assert!(near(Gaze::new(yaw, 0.3).flat_dir(), Quat::from_rotation_y(yaw) * Vec3::NEG_Z), "yaw {yaw}");
            assert!((Gaze::new(yaw, 0.3).dir().length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn gaze_toward_is_the_inverse_of_dir() {
        let from = Vec3::new(1.0, 1.6, -2.0);
        for yaw in [-3.0_f32, -1.5, -0.2, 0.0, 0.9, 2.0, 3.1] {
            for pitch in [-1.4_f32, -0.5, 0.0, 0.3, 1.3] {
                let look = Gaze::new(yaw, pitch);
                let back = Gaze::toward(from, from + look.dir() * 7.5).unwrap();
                assert!((back.yaw - yaw).abs() < 1e-4 && (back.pitch - pitch).abs() < 1e-4, "{look:?} -> {back:?}");
            }
        }
        // A point to the east and level: yaw -90 degrees; one above and ahead: 45 up.
        let east = Gaze::toward(Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0)).unwrap();
        assert!((east.yaw + std::f32::consts::FRAC_PI_2).abs() < 1e-5 && east.pitch.abs() < 1e-5, "{east:?}");
        let up = Gaze::toward(Vec3::ZERO, Vec3::new(0.0, 3.0, -3.0)).unwrap();
        assert!(up.yaw.abs() < 1e-5 && (up.pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-5, "{up:?}");
    }

    #[test]
    fn gaze_straight_up_or_down_and_bad_input_do_not_make_nan() {
        let up = Gaze::toward(Vec3::ZERO, Vec3::new(0.0, 2.0, 0.0)).unwrap();
        assert_eq!(up.yaw, 0.0, "no yaw straight up");
        assert!((up.pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-6 && up.is_finite());
        let down = Gaze::toward(Vec3::new(1.0, 5.0, 1.0), Vec3::new(1.0, 1.0, 1.0)).unwrap();
        assert_eq!(down.yaw, 0.0);
        assert!((down.pitch + std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!(up.dir().is_finite() && down.dir().is_finite());
        assert_eq!(Gaze::toward(Vec3::ONE, Vec3::ONE), None, "the same point has no direction");
        assert_eq!(Gaze::toward(Vec3::ZERO, Vec3::NAN), None);
        assert_eq!(Gaze::toward(Vec3::INFINITY, Vec3::ZERO), None);
        assert!(!Gaze::new(f32::NAN, 0.0).is_finite() && !Gaze::new(0.0, f32::INFINITY).is_finite());
        let clamped = Gaze::new(1.0, 1.5).with_pitch_between(-0.5, 0.5);
        assert_eq!((clamped.yaw, clamped.pitch), (1.0, 0.5));
        assert_eq!(Gaze::new(1.0, -1.5).with_pitch_between(-0.5, 0.5).pitch, -0.5);
        assert_eq!(Gaze::from_degrees(90.0, -45.0), Gaze::new(std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_4));
    }

    #[test]
    fn intent_sane_names_a_look_that_is_not_finite() {
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        let viewer = w.spawn((AiId(0), Intent { look: Some(Gaze::new(1.0, 0.2)), ..default() })).id();
        assert_eq!(intent_sane(&w), None);
        w.get_mut::<Intent>(viewer).unwrap().look = Some(Gaze::new(f32::NAN, 0.0));
        assert!(intent_sane(&w).unwrap().contains("Intent.look"));
    }

    #[test]
    fn look_is_left_out_of_an_intent_until_it_is_set() {
        let mut intent = Intent { move_dir: Vec3::X, ..default() };
        assert!(serde_json::to_value(&intent).unwrap().get("look").is_none(), "an input recorded before the field existed reads the same");
        intent.look = Some(Gaze::new(0.5, -0.25));
        let json = serde_json::to_value(&intent).unwrap();
        assert_eq!(json["look"], serde_json::json!({"yaw": 0.5, "pitch": -0.25}));
        assert_eq!(serde_json::from_value::<Intent>(json).unwrap(), intent);
    }

    /// The bytes a replay check, a snapshot and `inspect` see (bevy's reflection serializer).
    fn reflected(intent: &Intent, registry: &bevy::reflect::TypeRegistry) -> String {
        let serializer = bevy::reflect::serde::TypedReflectSerializer::new(intent, registry);
        serde_json::to_string(&serializer).unwrap()
    }

    #[test]
    fn reflection_writes_an_intent_without_look_byte_for_byte_as_before() {
        let mut registry = bevy::reflect::TypeRegistry::default();
        registry.register::<Intent>();
        let mut intent = Intent { move_dir: Vec3::new(0.6, 0.0, 0.8), aim: Some(Vec3::X), target: Some(AiId(6)), ..default() };
        intent.set_held("sprint", true);
        intent.press_dir("dodge", Vec3::Z);
        intent.orders.push_back(Order::MoveTo { target: Target::Entity(AiId(6)), within: 1.5 });
        intent.orders.push_back(Order::Attack { target: AiId(7), input: "light".into(), repeat: true });
        intent.orders.push_back(Order::MoveTo { target: Target::Point(Vec3::new(1.0, 0.0, 2.0)), within: 0.3 });
        // Written out by hand, as the field-by-field reflection serializer wrote it before `look` existed.
        let legacy = concat!(
            r#"{"move_dir":[0.6,0.0,0.8],"aim":[1.0,0.0,0.0],"target":6,"held":["sprint"],"#,
            r#""pressed":[{"input":"dodge","dir":[0.0,0.0,1.0]}],"#,
            r#""orders":[{"MoveTo":{"target":{"Entity":6},"within":1.5}},"#,
            r#"{"Attack":{"target":7,"input":"light","repeat":true}},"#,
            r#"{"MoveTo":{"target":{"Point":[1.0,0.0,2.0]},"within":0.3}}]}"#
        );
        assert_eq!(reflected(&intent, &registry), legacy);
        let empty = r#"{"move_dir":[0.0,0.0,0.0],"aim":null,"target":null,"held":[],"pressed":[],"orders":[]}"#;
        assert_eq!(reflected(&Intent::default(), &registry), empty);
        // With a look the field is there, last.
        let looking = Intent { look: Some(Gaze::new(1.0, 0.5)), ..default() };
        assert_eq!(reflected(&looking, &registry), empty.replace(r#"[]}"#, r#"[],"look":{"yaw":1.0,"pitch":0.5}}"#));
    }

    #[test]
    fn reflection_reads_a_snapshot_written_before_look_existed() {
        use bevy::reflect::serde::TypedReflectDeserializer;
        use serde::de::DeserializeSeed;
        let mut registry = bevy::reflect::TypeRegistry::default();
        registry.register::<Intent>();
        let reg = registry.get(std::any::TypeId::of::<Intent>()).unwrap();
        let read = |text: &str| {
            let value = TypedReflectDeserializer::new(reg, &registry).deserialize(&mut serde_json::Deserializer::from_str(text)).unwrap();
            <Intent as bevy::reflect::FromReflect>::from_reflect(value.as_ref()).expect("an Intent")
        };
        let old = r#"{"move_dir":[0.0,0.0,1.0],"aim":null,"target":6,"held":[],"pressed":[],"orders":[]}"#;
        assert_eq!(read(old), Intent { move_dir: Vec3::Z, target: Some(AiId(6)), ..default() });
        assert_eq!(read(r#"{"look":{"yaw":2.0,"pitch":-0.5}}"#).look, Some(Gaze::new(2.0, -0.5)));
    }
}
