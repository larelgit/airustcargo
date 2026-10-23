//! Content kind `mover` (`*.mover.ron`): the keys a mover goes through, how fast, how long it
//! waits at each, whether it loops, and what it does when a body is in its way.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};

/// At most this many keys in one mover.
pub const MAX_KEYS: usize = 64;

/// The shortest a way between two keys may take (s). A mover then passes a bounded number of keys
/// in a step, so a step runs all of its time (`run::advance`).
pub const MIN_WAY: f32 = 0.01;

/// How far apart two poses may be and still be one (m, and the quaternion dot below 1).
const SAME_POSE: f32 = 1e-4;

/// One kind of mover: a door, a lift, a turning platform.
///
/// ```ron
/// {
///     "lift": (
///         keys: [(at: (0.0, 0.0, 0.0)), (at: (0.0, 4.0, 0.0))],
///         speed: 1.2,
///     ),
///     "turntable": (
///         keys: [(), (turn: (0.0, 360.0, 0.0), time: 8.0)],
///         loop: Cycle,
///     ),
///     "crusher": (
///         keys: [(), (at: (0.0, -2.6, 0.0))],
///         speed: 1.0,
///         blocked: Crush(damage: 10.0),
///     ),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoverDef {
    /// The poses it goes through, relative to where it was placed (the node's pose in the level
    /// file, the spawn's transform) and in that frame: a door turned in the level slides along its own
    /// axis. It starts at key 0.
    pub keys: Vec<MoverKey>,
    /// Meters per second along the way between two keys, unless the key reached gives a `time`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    /// Degrees per second, for a way that turns: the largest of the three angles' changes counts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_speed: Option<f32>,
    /// Seconds it rests at each key where it stops, unless the key gives its own `wait`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub wait: f32,
    /// Whether it runs by itself: `Off` (it goes only where it is sent), `PingPong` (up the keys and
    /// back), `Cycle` (up the keys, then from the last back to key 0 at once: the last key must be
    /// where key 0 is).
    #[serde(default, rename = "loop", skip_serializing_if = "is_default")]
    pub looping: MoverLoop,
    /// What it does when a body is in its way and cannot be pushed out of it.
    #[serde(default, skip_serializing_if = "is_default")]
    pub blocked: Blocked,
}

/// One pose of a mover.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MoverKey {
    /// Where it is (m), from the place it was put, in that place's frame.
    pub at: Vec3,
    /// How it is turned (degrees): about Y (yaw; positive turns counter-clockwise seen from above),
    /// then X, then Z. Angles are interpolated as numbers, so 0 → 360 is one whole turn.
    pub turn: Vec3,
    /// Seconds for the way between the key before and this one (both directions), in place of
    /// `speed` and `turn_speed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<f32>,
    /// Seconds it rests here when it stops here, in place of the mover's `wait`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<f32>,
}

/// Whether a mover runs by itself.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MoverLoop {
    /// It rests until sent to a key (`mover_go`), goes there through the keys between and rests.
    #[default]
    Off,
    /// Up the keys to the last, then back down to key 0, and again, resting `wait` at each key.
    PingPong,
    /// Up the keys to the last, which is key 0 again, and on, resting `wait` at each key.
    Cycle,
}

/// What a mover does when a body is in its way and cannot be pushed out of it (against a wall,
/// pinned under it).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum Blocked {
    /// It holds still where it is and goes on once the body is out of the way.
    #[default]
    Stop,
    /// It turns back to the key it came from (or, held as it leaves a key, stays there) and rests.
    Reverse,
    /// It holds still and hurts the body by `damage` when it is first held and every `every` seconds
    /// after (the damage is the action module's, through `deal_damage`).
    Crush {
        damage: f32,
        #[serde(default = "one_second", skip_serializing_if = "is_one")]
        every: f32,
    },
}

fn one_second() -> f32 {
    1.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

impl MoverKey {
    /// The turn as a rotation: yaw, then pitch, then roll.
    pub fn rotation(&self) -> Quat {
        euler(self.turn)
    }
}

/// Degrees about Y, X and Z as a rotation (yaw first).
pub(crate) fn euler(turn: Vec3) -> Quat {
    Quat::from_euler(EulerRot::YXZ, turn.y.to_radians(), turn.x.to_radians(), turn.z.to_radians())
}

impl MoverDef {
    /// How many keys it has.
    pub fn len(&self) -> u32 {
        self.keys.len() as u32
    }

    /// Never: a definition has at least two keys (the check refuses fewer).
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Seconds it rests at `key` when it stops there.
    pub fn wait_at(&self, key: u32) -> f32 {
        self.keys.get(key as usize).and_then(|k| k.wait).unwrap_or(self.wait).max(0.0)
    }

    /// Seconds the way between the neighbouring keys `a` and `b` takes (either direction): the
    /// later key's `time`, else the longer of the move at `speed` and the turn at `turn_speed`. 0 for
    /// a way the definition gives no time (the check refuses it).
    pub fn duration(&self, a: u32, b: u32) -> f32 {
        let (Some(ka), Some(kb)) = (self.keys.get(a as usize), self.keys.get(b as usize)) else { return 0.0 };
        if let Some(time) = self.keys[a.max(b) as usize].time {
            return time;
        }
        let distance = ka.at.distance(kb.at);
        let angle = (kb.turn - ka.turn).abs().max_element();
        let by_move = self.speed.filter(|s| *s > 0.0).map_or(0.0, |s| distance / s);
        let by_turn = self.turn_speed.filter(|s| *s > 0.0).map_or(0.0, |s| angle / s);
        by_move.max(by_turn)
    }

    /// Seconds the shortest way between neighbouring keys takes (infinite with fewer than 2 keys).
    pub fn shortest_way(&self) -> f32 {
        (1..self.len()).map(|i| self.duration(i - 1, i)).fold(f32::INFINITY, f32::min)
    }

    /// The pose between keys `from` and `to`, `t` of the way (0: at `from`), relative to the mover's
    /// origin: a place and a rotation.
    pub fn local(&self, from: u32, to: u32, t: f32) -> (Vec3, Quat) {
        let key = |k: u32| self.keys.get(k as usize).cloned().unwrap_or_default();
        let (a, b) = (key(from), key(to));
        if from == to || t <= 0.0 {
            return (a.at, a.rotation());
        }
        (a.at.lerp(b.at, t), euler(a.turn.lerp(b.turn, t)))
    }
}

/// The problems with one way between keys: none, or what to add.
fn way_problem(def: &MoverDef, i: usize) -> Option<String> {
    let duration = def.duration(i as u32 - 1, i as u32);
    if duration >= MIN_WAY {
        return None;
    }
    if duration > 0.0 {
        return Some(format!(
            "keys[{i}]: the way from key {} takes {duration} s, under the shortest a way may take ({MIN_WAY} s): give key {i} a longer `time`, or the mover a lower speed",
            i - 1
        ));
    }
    let (a, b) = (&def.keys[i - 1], &def.keys[i]);
    let distance = a.at.distance(b.at);
    let angle = (b.turn - a.turn).abs().max_element();
    let mut want = Vec::new();
    if distance > 0.0 && def.speed.is_none() {
        want.push(format!("it moves {distance:.2} m: give the mover a `speed`"));
    }
    if angle > 0.0 && def.turn_speed.is_none() {
        want.push(format!("it turns {angle:.0}°: give the mover a `turn_speed`"));
    }
    if want.is_empty() {
        want.push(format!("it stays where key {} is: give key {i} a `time` (a stop is `wait`)", i - 1));
    } else {
        want.push(format!("or give key {i} a `time`"));
    }
    Some(format!("keys[{i}]: the way from key {} takes no time; {}", i - 1, want.join(", ")))
}

impl ContentDef for MoverDef {
    const KIND: &'static str = "mover";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if self.keys.len() < 2 {
            refs.error(format!("keys: a mover needs at least 2 keys (where it starts and where it goes), found {}", self.keys.len()));
        }
        if self.keys.len() > MAX_KEYS {
            refs.error(format!("keys: at most {MAX_KEYS} keys, found {}", self.keys.len()));
        }
        let mut sound = true;
        for (i, key) in self.keys.iter().enumerate() {
            if !key.at.is_finite() {
                refs.error(format!("keys[{i}].at must be finite meters, found {}", key.at));
                sound = false;
            }
            if !key.turn.is_finite() {
                refs.error(format!("keys[{i}].turn must be finite degrees, found {}", key.turn));
                sound = false;
            }
            if let Some(time) = key.time
                && !(time.is_finite() && time > 0.0)
            {
                refs.error(format!("keys[{i}].time must be seconds above 0, found {time}"));
                sound = false;
            }
            if let Some(wait) = key.wait
                && !(wait.is_finite() && wait >= 0.0)
            {
                refs.error(format!("keys[{i}].wait must be seconds, 0 or more, found {wait}"));
            }
        }
        if self.keys.first().is_some_and(|k| k.time.is_some()) {
            refs.error("keys[0].time: key 0 is where the mover starts; a `time` belongs to the way into a key, from the key before it");
        }
        if let Some(speed) = self.speed
            && !(speed.is_finite() && speed > 0.0)
        {
            refs.error(format!("speed must be meters per second above 0, found {speed}"));
            sound = false;
        }
        if let Some(turn) = self.turn_speed
            && !(turn.is_finite() && turn > 0.0)
        {
            refs.error(format!("turn_speed must be degrees per second above 0, found {turn}"));
            sound = false;
        }
        if !(self.wait.is_finite() && self.wait >= 0.0) {
            refs.error(format!("wait must be seconds, 0 or more, found {}", self.wait));
        }
        if let Blocked::Crush { damage, every } = self.blocked {
            if !(damage.is_finite() && damage > 0.0) {
                refs.error(format!("blocked: Crush damage must be above 0, found {damage}"));
            }
            if !(every.is_finite() && every > 0.0) {
                refs.error(format!("blocked: Crush every must be seconds above 0, found {every}"));
            }
        }
        if !sound || self.keys.len() < 2 {
            return;
        }
        for i in 1..self.keys.len() {
            if let Some(problem) = way_problem(self, i) {
                refs.error(problem);
            }
        }
        if self.looping == MoverLoop::Cycle {
            let (first, last) = (&self.keys[0], &self.keys[self.keys.len() - 1]);
            let same_place = first.at.distance(last.at) <= SAME_POSE;
            let same_turn = first.rotation().dot(last.rotation()).abs() >= 1.0 - SAME_POSE;
            if !(same_place && same_turn) {
                refs.error(format!(
                    "loop: Cycle goes from the last key back to key 0 at once, so keys[{}] must be where key 0 is (`at` and `turn`; 360° is 0°)",
                    self.keys.len() - 1
                ));
            }
        }
    }
}
