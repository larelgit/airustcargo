//! Content: actions, movesets and combat tuning, as a game writes them.
//!
//! An action is a timeline. Its phases give its length and the names agents read
//! (`windup heavy 0.32s`); windows on that timeline say what happens when: hit
//! windows with shapes, i-frames, guard and perfect guard, hyper-armor, cancel windows
//! into other actions. A window is a [`Span`]: a whole phase, from one phase to
//! another, or seconds since the start. Everything is data, so a light combo, a
//! heavy slam, a block, a parry, a dodge roll and a healing flask are the same machine
//! with different numbers.
//!
//! ```ron
//! // content/combat/sword.action.ron
//! {
//!     "light_1": (
//!         net_id: 1,
//!         phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
//!         cost: (stamina: 10),
//!         hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 90), damage: 20, poise: 15)],
//!         cancels: [(span: Secs(0.3, 0.6), into: [Combo("light", "light_2"), Input("dodge")])],
//!         displacement: [(0.0, 0.0), (0.3, 0.6)],
//!         anim: "light_1",
//!     ),
//! }
//! ```
//!
//! The `displacement` curve can come from the clip: `bake: (animset: "humanoid", clip: "light_1")`
//! names the clip whose root bone moves the body, `--bake-motion` (or the `motion_bake` tool)
//! writes the curve, and content checks say when the clip changed and the curve is stale
//! ([`crate::bake`]).

use std::collections::BTreeMap;
use std::fmt;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};
use shards_motion::SprintResume;

use crate::status::StatusDef;

/// Slack for comparing times that are sums of tick lengths (s).
pub const EPS: f32 = 1e-4;

/// A stretch of an action's timeline.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Span {
    /// A whole phase: `Phase("active")`.
    Phase(String),
    /// From the start of one phase to the end of another: `Phases("raise", "hold")`.
    Phases(String, String),
    /// Seconds since the action started: `Secs(0.30, 0.45)`.
    Secs(f32, f32),
}

/// One named part of an action: `("windup", 0.25)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Phase(pub String, pub f32);

/// A point in the actor's frame at the start of the action: meters forward (where it
/// faces), up (from the feet) and to the right.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Local {
    pub forward: f32,
    pub up: f32,
    pub right: f32,
}

impl Local {
    /// In Bevy's local frame (forward is -Z).
    pub fn to_vec3(self) -> Vec3 {
        Vec3::new(self.right, self.up, -self.forward)
    }
}

/// A hit window's shape, placed at [`HitDef::at`] and turned to the action's facing.
/// Sizes in meters, angles in degrees.
#[derive(Serialize, Deserialize, Reflect, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum HitShape {
    /// A ball around `at`.
    Sphere { radius: f32 },
    /// A thrust: a capsule from `at` forward `length` meters.
    Capsule { radius: f32, length: f32 },
    /// A sweep: a horizontal sector from `at`, `angle` wide, `range` long, standing
    /// `height` tall on `at`.
    Cone {
        range: f32,
        angle: f32,
        #[serde(default = "default_height")]
        height: f32,
    },
    /// A box centered on `at`: `width` across, `height` up, `depth` forward.
    Box { width: f32, height: f32, depth: f32 },
    /// A disc on the ground around `at` (a slam, a nova), `height` tall.
    Circle {
        radius: f32,
        #[serde(default = "default_height")]
        height: f32,
    },
    /// A strip along the ground from `at` forward (a shockwave): low, so a jump clears it.
    Line {
        length: f32,
        width: f32,
        #[serde(default = "default_line_height")]
        height: f32,
    },
}

fn default_height() -> f32 {
    2.0
}

fn default_line_height() -> f32 {
    0.5
}

impl HitShape {
    /// How far it reaches forward of `at`.
    pub fn forward_extent(&self) -> f32 {
        match *self {
            HitShape::Sphere { radius } => radius,
            HitShape::Capsule { radius, length } => length + radius,
            HitShape::Cone { range, .. } => range,
            HitShape::Box { depth, .. } => depth * 0.5,
            HitShape::Circle { radius, .. } => radius,
            HitShape::Line { length, .. } => length,
        }
    }

    /// Every size, for validation.
    fn sizes(&self) -> Vec<(&'static str, f32)> {
        match *self {
            HitShape::Sphere { radius } => vec![("radius", radius)],
            HitShape::Capsule { radius, length } => vec![("radius", radius), ("length", length)],
            HitShape::Cone { range, angle, height } => vec![("range", range), ("angle", angle), ("height", height)],
            HitShape::Box { width, height, depth } => vec![("width", width), ("height", height), ("depth", depth)],
            HitShape::Circle { radius, height } => vec![("radius", radius), ("height", height)],
            HitShape::Line { length, width, height } => vec![("length", length), ("width", width), ("height", height)],
        }
    }
}

/// `circle 4.5m`, `cone 3m 90°`, `line 8m`: what an agent reads about a zone.
impl fmt::Display for HitShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = Meters;
        match *self {
            HitShape::Sphere { radius } => write!(f, "sphere {}m", m(radius)),
            HitShape::Capsule { length, .. } => write!(f, "thrust {}m", m(length)),
            HitShape::Cone { range, angle, .. } => write!(f, "cone {}m {}°", m(range), m(angle)),
            HitShape::Box { width, depth, .. } => write!(f, "box {}×{}m", m(width), m(depth)),
            HitShape::Circle { radius, .. } => write!(f, "circle {}m", m(radius)),
            HitShape::Line { length, width, .. } => write!(f, "line {}m×{}m", m(length), m(width)),
        }
    }
}

/// `2`, `4.5`, `0.25`: meters with no trailing zeros. It formats into a stack buffer, so
/// showing a zone (or comparing it with what is shown, every tick) builds no `String`.
struct Meters(f32);

impl fmt::Display for Meters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write;
        /// Room for the longest `f32` with two decimals: 39 digits, a sign and the point.
        struct Stack {
            buf: [u8; 48],
            len: usize,
        }
        impl Write for Stack {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                let end = self.len + s.len();
                self.buf.get_mut(self.len..end).ok_or(fmt::Error)?.copy_from_slice(s.as_bytes());
                self.len = end;
                Ok(())
            }
        }
        let mut stack = Stack { buf: [0; 48], len: 0 };
        write!(stack, "{:.2}", self.0)?;
        let s = std::str::from_utf8(&stack.buf[..stack.len]).map_err(|_| fmt::Error)?;
        let s = s.trim_end_matches('0').trim_end_matches('.');
        f.write_str(if s == "-0" { "0" } else { s })
    }
}

/// `2`, `4.5`, `0.25`: no trailing zeros.
pub(crate) fn meters(v: f32) -> String {
    Meters(v).to_string()
}

/// What a hit does beyond damage.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Reflect)]
pub enum HitFlag {
    /// Guards and perfect guards do nothing against it.
    Unblockable,
    /// A block of it always breaks the guard (a perfect guard still parries it).
    GuardBreak,
    /// A perfect guard only blocks it.
    NoParry,
}

/// One hit window: while `span` lasts, `shape` hits every hurtbox of another team
/// once per action (per `group`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HitDef {
    pub span: Span,
    pub shape: HitShape,
    /// Where the shape sits, in the actor's frame.
    #[serde(default)]
    pub at: Local,
    pub damage: f32,
    /// Poise damage: enough of it staggers.
    #[serde(default)]
    pub poise: f32,
    /// Status buildup: `[("bleed", 30)]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<(String, f32)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<HitFlag>,
    /// Free-form tags for the game's damage hooks: `["fire", "backstab"]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// A target is hit once per action and group: give a second swing of a spin its
    /// own group to let it hit again.
    #[serde(default)]
    pub group: u8,
}

/// A guard (block) window, with an optional perfect-guard (parry) window inside.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuardDef {
    pub span: Span,
    /// A hit inside this window is parried: no damage, the attacker staggers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perfect: Option<Span>,
    /// Damage that gets through a block (0: none).
    #[serde(default)]
    pub damage_mult: f32,
    /// Stamina a block costs per point of the hit's damage. Not enough stamina breaks
    /// the guard.
    #[serde(default = "one")]
    pub stamina_per_damage: f32,
    /// Poise damage that gets through a block.
    #[serde(default)]
    pub poise_mult: f32,
    /// Hits from within this arc around the facing are guarded (degrees, full angle).
    #[serde(default = "half_turn")]
    pub arc: f32,
}

fn one() -> f32 {
    1.0
}

fn half_turn() -> f32 {
    180.0
}

/// Where a press may go while a cancel window is open.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Transition {
    /// That moveset input starts its usual action: `Input("dodge")`.
    Input(String),
    /// That input starts this action instead (combo chains): `Combo("light", "light_2")`.
    Combo(String, String),
    /// Any input of the moveset.
    Any,
}

/// While `span` lasts, the action can be cut short by the inputs in `into`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CancelDef {
    pub span: Span,
    pub into: Vec<Transition>,
}

/// Hold to loop: while `input` is held, time loops inside `span` (a raised block). The
/// span must last, and a guard, i-frame or hyper-armor window that starts before it
/// must either end where it starts or cover all of it (else it reopens on each pass).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HoldDef {
    pub input: String,
    pub span: Span,
}

/// Something an action does to its actor at a moment.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
pub enum Effect {
    /// Restore health: `Heal(40)`.
    Heal(f32),
    /// Restore stamina.
    RestoreStamina(f32),
    /// Status buildup on the actor itself: `Status("regen", 100)`.
    Status(String, f32),
    /// A sound from the actor (the audio module plays the `*.sound.ron` of that name from the
    /// body): `Sound("whoosh")`. Decided with the rest of the action on the simulation's clock, so
    /// it is the same headless and in a window, and a co-op client hears the host's
    /// ([`ActionSound`](crate::ActionSound)); a clip's cue sounds only where a model is drawn.
    /// The audio module's content check names a sound that is not defined.
    Sound(String),
    /// A game hook registered with `add_action_effect`: `Custom("spawn_fireball")`.
    Custom(String),
}

/// An [`Effect`] at `at` seconds into the action. Inside a hold loop it fires on every
/// pass.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimedEffect {
    pub at: f32,
    pub effect: Effect,
}

/// The warning an enemy gives: a ground zone with a symbol (danger reads without
/// colour) and a sound cue.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TelegraphDef {
    /// Seconds into the action when it shows.
    #[serde(default)]
    pub at: f32,
    #[serde(default = "default_symbol")]
    pub symbol: String,
    /// Sound cue id for the audio module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<String>,
    /// The zone shown; default: the shape of the first hit window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zone: Option<HitShape>,
    #[serde(default)]
    pub zone_at: Local,
}

fn default_symbol() -> String {
    "!".into()
}

/// Where an action points when it starts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Steer {
    /// The press's direction, else the lock-on target, the aim point, the movement
    /// input, the current facing.
    #[default]
    Aim,
    /// The press's direction, else the movement input, the current facing (dodges:
    /// a lock-on target does not pull a roll toward itself).
    Move,
}

/// Where an action's `displacement` is baked from: clip `clip` of animset `animset`, whose
/// `root_motion` bone moves along the ground ([`crate::bake`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MotionBake {
    pub animset: String,
    pub clip: String,
}

/// What starting an action costs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Cost {
    /// Needed in full to start.
    pub stamina: f32,
    /// Seconds before the action can start again.
    pub cooldown: f32,
    /// One charge of this named consumable (a healing flask).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge: Option<String>,
}

impl Cost {
    fn is_free(&self) -> bool {
        *self == Cost::default()
    }
}

/// An action (content kind `action`): players', enemies' and bosses' alike.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionDef {
    /// How the action is named on the wire; unique among actions.
    pub net_id: u16,
    /// In order; their durations add up to the action's length.
    pub phases: Vec<Phase>,
    #[serde(default, skip_serializing_if = "Cost::is_free")]
    pub cost: Cost,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hits: Vec<HitDef>,
    /// Invulnerable: hits are evaded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub iframes: Vec<Span>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardDef>,
    /// Poise damage is ignored: no stagger, no flinch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hyper_armor: Vec<Span>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cancels: Vec<CancelDef>,
    /// `(seconds, meters)` keys: how far along its facing the actor has moved by then,
    /// baked from the animation. Linear between keys, from `(0, 0)`. The character
    /// controller moves the body, so a wall stops a lunge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displacement: Vec<(f32, f32)>,
    /// The clip `displacement` is baked from (`--bake-motion`); content checks that the curve
    /// is not stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bake: Option<MotionBake>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<HoldDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<TimedEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegraph: Option<TelegraphDef>,
    #[serde(default)]
    pub steer: Steer,
    /// The facing follows the target (or aim) until this many seconds in, then stays.
    #[serde(default)]
    pub track_until: f32,
    /// `None`: the action owns the body. `Some(m)`: the actor walks at `m` times its
    /// speed meanwhile (0.5 for a raised block).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub move_speed: Option<f32>,
    /// Animation clip; the animation follows the action, never the other way round. In a game
    /// with animsets the content check wants one of them to play it ([`AnimSet`](crate::AnimSet)):
    /// leave it out for an action that shows no clip of its own (the locomotion clip plays on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// What a press turns into when a cancel window allows it.
#[derive(Clone, Debug, PartialEq)]
pub enum CancelTo {
    /// The moveset's action for the input.
    Moveset,
    /// This action (a combo link).
    Action(String),
}

/// Whether window `win` is open in the tick that went from `prev` to `t`: it touches
/// `(prev, t]`, or, right at the start (`t` = 0), it starts at 0.
pub fn touches(win: (f32, f32), prev: f32, t: f32) -> bool {
    if t <= EPS {
        win.0 <= EPS && win.1 >= 0.0
    } else {
        win.0 <= t + EPS && win.1 > prev + EPS
    }
}

impl ActionDef {
    /// Seconds from start to end.
    pub fn length(&self) -> f32 {
        self.phases.iter().map(|p| p.1.max(0.0)).sum()
    }

    /// Start and end of phase `name`.
    pub fn phase_span(&self, name: &str) -> Option<(f32, f32)> {
        let mut start = 0.0;
        for Phase(phase, secs) in &self.phases {
            let end = start + secs.max(0.0);
            if phase == name {
                return Some((start, end));
            }
            start = end;
        }
        None
    }

    /// `(from, to)` seconds of a span; `None` when it names an unknown phase.
    pub fn span(&self, span: &Span) -> Option<(f32, f32)> {
        match span {
            Span::Phase(name) => self.phase_span(name),
            Span::Phases(a, b) => Some((self.phase_span(a)?.0, self.phase_span(b)?.1)),
            Span::Secs(from, to) => Some((*from, *to)),
        }
    }

    /// Whether any of `spans` is open in the tick `(prev, t]`.
    pub fn any_open(&self, spans: &[Span], prev: f32, t: f32) -> bool {
        spans.iter().filter_map(|s| self.span(s)).any(|w| touches(w, prev, t))
    }

    /// The phase at `t` and when it ends.
    pub fn phase_at(&self, t: f32) -> Option<(&str, f32)> {
        let mut end = 0.0;
        for Phase(name, secs) in &self.phases {
            end += secs.max(0.0);
            if t < end - EPS {
                return Some((name, end));
            }
        }
        self.phases.last().map(|p| (p.0.as_str(), end))
    }

    /// Meters along the facing moved by `t`.
    pub fn displacement_at(&self, t: f32) -> f32 {
        let (mut t0, mut d0) = (0.0, 0.0);
        for &(t1, d1) in &self.displacement {
            if t <= t1 {
                if t1 - t0 <= EPS {
                    return d1;
                }
                return d0 + (d1 - d0) * ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            }
            (t0, d0) = (t1, d1);
        }
        d0
    }

    /// How far ahead of the actor it can hit, lunge included (m): how close an
    /// `attack` order walks before pressing.
    pub fn reach(&self) -> f32 {
        self.hits
            .iter()
            .filter_map(|h| {
                let (from, _) = self.span(&h.span)?;
                Some(h.at.forward + h.shape.forward_extent() + self.displacement_at(from))
            })
            .fold(0.0, f32::max)
    }

    /// Whether a cancel window open in `(prev, t]` lets `input` through, and into what.
    pub fn cancel_into(&self, input: &str, prev: f32, t: f32) -> Option<CancelTo> {
        self.cancels
            .iter()
            .filter(|c| self.span(&c.span).is_some_and(|w| touches(w, prev, t)))
            .find_map(|c| transition(&c.into, input))
    }

    /// When the next cancel window that lets `input` through opens after `t`, and what
    /// the press turns into there.
    pub fn next_cancel(&self, input: &str, t: f32) -> Option<(f32, CancelTo)> {
        self.cancels
            .iter()
            .filter_map(|c| Some((self.span(&c.span)?, transition(&c.into, input)?)))
            .filter(|(w, _)| w.0 > t + EPS)
            .min_by(|a, b| a.0 .0.total_cmp(&b.0 .0))
            .map(|(w, to)| (w.0, to))
    }

    /// The first hit window that has not ended by `t`.
    pub fn next_hit(&self, t: f32) -> Option<(usize, (f32, f32))> {
        self.hits
            .iter()
            .enumerate()
            .filter_map(|(i, h)| Some((i, self.span(&h.span)?)))
            .filter(|(_, w)| w.1 > t + EPS)
            .min_by(|a, b| a.1 .0.total_cmp(&b.1 .0).then(a.0.cmp(&b.0)))
    }

    /// The zone a telegraph shows: its own, else the first hit window's.
    pub fn telegraph_zone(&self) -> Option<(HitShape, Local)> {
        match &self.telegraph {
            Some(TelegraphDef { zone: Some(shape), zone_at, .. }) => Some((*shape, *zone_at)),
            _ => self.hits.first().map(|h| (h.shape, h.at)),
        }
    }
}

/// What `input` turns into under `into`.
fn transition(into: &[Transition], input: &str) -> Option<CancelTo> {
    into.iter().find_map(|tr| match tr {
        Transition::Input(i) if i == input => Some(CancelTo::Moveset),
        Transition::Combo(i, action) if i == input => Some(CancelTo::Action(action.clone())),
        Transition::Any => Some(CancelTo::Moveset),
        _ => None,
    })
}

impl ContentDef for ActionDef {
    const KIND: &'static str = "action";

    fn validate(&self, id: &str, refs: &mut Refs) {
        let hold = self.hold.as_ref().and_then(|h| self.span(&h.span)).filter(|(a, b)| b - a > EPS);
        let mut check = Check { def: self, refs, length: self.length(), hold };
        check.phases();
        check.number("cost.stamina", self.cost.stamina);
        check.number("cost.cooldown", self.cost.cooldown);
        check.number("track_until", self.track_until);
        if let Some(m) = self.move_speed {
            check.number("move_speed", m);
        }
        for (i, hit) in self.hits.iter().enumerate() {
            let at = format!("hits[{i}]");
            check.span(&format!("{at}.span"), &hit.span);
            check.number(&format!("{at}.damage"), hit.damage);
            check.number(&format!("{at}.poise"), hit.poise);
            for (name, size) in hit.shape.sizes() {
                if !(size > 0.0 && size.is_finite()) {
                    check.refs.error(format!("{at}.shape.{name} must be > 0 (is {size})"));
                }
            }
            if let HitShape::Cone { angle, .. } = hit.shape
                && angle > 360.0
            {
                check.refs.error(format!("{at}.shape.angle is at most 360 (is {angle})"));
            }
            for (status, amount) in &hit.status {
                check.refs.require::<StatusDef>(status, &format!("{at}.status"));
                check.number(&format!("{at}.status `{status}`"), *amount);
            }
        }
        for (i, span) in self.iframes.iter().enumerate() {
            check.window(&format!("iframes[{i}]"), span);
        }
        for (i, span) in self.hyper_armor.iter().enumerate() {
            check.window(&format!("hyper_armor[{i}]"), span);
        }
        if let Some(guard) = &self.guard {
            check.window("guard.span", &guard.span);
            if let Some(perfect) = &guard.perfect {
                check.window("guard.perfect", perfect);
            }
            check.number("guard.damage_mult", guard.damage_mult);
            check.number("guard.stamina_per_damage", guard.stamina_per_damage);
            check.number("guard.poise_mult", guard.poise_mult);
            if !(guard.arc > 0.0 && guard.arc <= 360.0) {
                check.refs.error(format!("guard.arc must be in (0, 360] degrees (is {})", guard.arc));
            }
        }
        for (i, cancel) in self.cancels.iter().enumerate() {
            check.span(&format!("cancels[{i}].span"), &cancel.span);
            for tr in &cancel.into {
                if let Transition::Combo(_, action) = tr {
                    check.refs.require::<ActionDef>(action, &format!("cancels[{i}].into"));
                }
            }
        }
        check.displacement();
        if let Some(bake) = &self.bake {
            crate::bake::check(self, id, bake, check.refs);
        }
        if let Some(anim) = &self.anim {
            crate::anim::check_action_clip(id, anim, &self.tags, check.refs);
        }
        if let Some(hold) = &self.hold {
            check.span("hold.span", &hold.span);
            if let Some((a, b)) = self.span(&hold.span)
                && a <= b
                && b - a <= EPS
            {
                check.refs.error(format!(
                    "hold.span {}..{} is empty; a held action loops inside it, so it must last > 0 s",
                    meters(a),
                    meters(b)
                ));
            }
        }
        for (i, effect) in self.effects.iter().enumerate() {
            check.time(&format!("effects[{i}].at"), effect.at);
            match &effect.effect {
                Effect::Heal(v) | Effect::RestoreStamina(v) => check.number(&format!("effects[{i}]"), *v),
                Effect::Status(status, v) => {
                    check.refs.require::<StatusDef>(status, &format!("effects[{i}]"));
                    check.number(&format!("effects[{i}]"), *v);
                }
                // The sound is checked where the mixer is (`AudioDef`), as a telegraph's is.
                Effect::Sound(_) | Effect::Custom(_) => {}
            }
        }
        if let Some(telegraph) = &self.telegraph {
            check.time("telegraph.at", telegraph.at);
            if self.telegraph_zone().is_none() {
                check.refs.error("telegraph has no zone: give it `zone` or the action a hit window");
            }
        }
        let others: Vec<String> = check
            .refs
            .iter::<ActionDef>()
            .filter(|(other, def)| *other != id && def.net_id == self.net_id)
            .map(|(o, _)| o.to_owned())
            .collect();
        if let Some(other) = others.first() {
            check.refs.error(format!(
                "net_id {} is also used by action `{other}`; net ids name actions on the wire, so each must be unique",
                self.net_id
            ));
        }
    }
}

/// Validation helpers that name the field.
struct Check<'d, 'r, 'a> {
    def: &'d ActionDef,
    refs: &'r mut Refs<'a>,
    length: f32,
    /// The hold loop's span, when it has a usable one.
    hold: Option<(f32, f32)>,
}

impl Check<'_, '_, '_> {
    fn number(&mut self, field: &str, v: f32) {
        if !(v >= 0.0 && v.is_finite()) {
            self.refs.error(format!("{field} must be a number >= 0 (is {v})"));
        }
    }

    fn time(&mut self, field: &str, t: f32) {
        if !(t >= 0.0 && t <= self.length + EPS) {
            self.refs.error(format!("{field} {t} is outside the action (0..{})", meters(self.length)));
        }
    }

    fn phases(&mut self) {
        if self.def.phases.is_empty() {
            self.refs.error("phases is empty; give at least one, e.g. [(\"active\", 0.3)]");
            return;
        }
        let mut seen: Vec<&str> = Vec::new();
        for Phase(name, secs) in &self.def.phases {
            if name.is_empty() {
                self.refs.error("a phase has no name");
            } else if seen.contains(&name.as_str()) {
                self.refs.error(format!("phase `{name}` appears twice; spans name phases, so names must be unique"));
            }
            seen.push(name);
            if !(*secs >= 0.0 && secs.is_finite()) {
                self.refs.error(format!("phase `{name}` must last >= 0 s (is {secs})"));
            }
        }
        if self.length <= 0.0 {
            self.refs.error("the phases last 0 s in all; an action needs some length");
        }
    }

    fn span(&mut self, field: &str, span: &Span) {
        match self.def.span(span) {
            None => {
                let names: Vec<&str> = self.def.phases.iter().map(|p| p.0.as_str()).collect();
                let wanted = match span {
                    Span::Phase(a) | Span::Phases(a, _) if self.def.phase_span(a).is_none() => a.as_str(),
                    Span::Phases(_, b) => b.as_str(),
                    _ => "",
                };
                let hint = shards_content::db::closest(wanted, names.iter().copied())
                    .map(|c| format!(" (did you mean `{c}`?)"))
                    .unwrap_or_default();
                self.refs
                    .error(format!("{field} names an unknown phase `{wanted}`{hint}; phases: {}", names.join(", ")));
            }
            Some((from, to)) => {
                if !(from >= 0.0 && from <= to && to <= self.length + EPS) {
                    self.refs.error(format!(
                        "{field} {}..{} must lie inside the action (0..{}) with from <= to",
                        meters(from),
                        meters(to),
                        meters(self.length)
                    ));
                }
            }
        }
    }

    /// [`span`](Self::span) for a defensive window. One that starts before the hold span
    /// and ends inside it would reopen on every loop pass (the loop replays the part
    /// of it after the hold's start), so a parry window meant for the raise stays on.
    fn window(&mut self, field: &str, span: &Span) {
        self.span(field, span);
        let (Some((a, b)), Some((from, to))) = (self.hold, self.def.span(span)) else { return };
        if from < a - EPS && to > a + EPS && to < b - EPS {
            self.refs.error(format!(
                "{field} {}..{} starts before the hold span {}..{} and ends inside it: it would reopen on every loop \
                 pass; end it where the hold starts (e.g. `Phase(\"raise\")`) or cover the whole hold",
                meters(from),
                meters(to),
                meters(a),
                meters(b)
            ));
        }
    }

    fn displacement(&mut self) {
        let mut last = 0.0;
        for &(t, d) in &self.def.displacement {
            if t < last - EPS || !d.is_finite() {
                self.refs.error(format!("displacement keys must go forward in time (key ({t}, {d}) after {last})"));
                return;
            }
            self.time("displacement", t);
            last = t;
        }
    }
}

/// Which action each input starts (content kind `moveset`), plus the actions the
/// engine starts by itself.
///
/// ```ron
/// {
///     "hero": (
///         inputs: {"light": "light_1", "heavy": "heavy", "dodge": "roll", "block": "block",
///                  "parry": "parry", "use_item.flask": "drink"},
///         stagger: "stagger",
///         death: "death",
///     ),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MovesetDef {
    /// Input name (`light`, `dodge`, `technique.1`, `use_item.flask`) → action id.
    pub inputs: BTreeMap<String, String>,
    /// Forced when poise breaks, a guard breaks or a parry lands. Without it the fighter
    /// never staggers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger: Option<String>,
    /// Forced by a hit that does not stagger (a flinch); none: no flinch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit: Option<String>,
    /// Played once on death.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub death: Option<String>,
}

impl MovesetDef {
    /// The action `input` starts.
    pub fn action(&self, input: &str) -> Option<&str> {
        self.inputs.get(input).map(String::as_str)
    }
}

impl ContentDef for MovesetDef {
    const KIND: &'static str = "moveset";

    fn validate(&self, _: &str, refs: &mut Refs) {
        for (input, action) in &self.inputs {
            if input.is_empty() || input.contains(char::is_whitespace) {
                refs.error(format!("input `{input}` must be a name without spaces, e.g. `light`"));
            }
            refs.require::<ActionDef>(action, &format!("inputs.{input}"));
        }
        for (slot, action) in [("stagger", &self.stagger), ("hit", &self.hit), ("death", &self.death)] {
            if let Some(action) = action {
                refs.require::<ActionDef>(action, slot);
            }
        }
    }
}

/// Engine-wide combat numbers (content kind `combat_tuning`, id `default`). Missing
/// fields keep their defaults.
///
/// ```ron
/// // content/combat/tuning.combat_tuning.ron
/// { "default": (input_buffer: 0.15, max_statuses: 3) }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CombatTuning {
    /// How long a press waits for its action to become possible (s).
    pub input_buffer: f32,
    /// Statuses active at once on one fighter; more wait at full buildup.
    pub max_statuses: u32,
    /// Stamina sprinting costs per second.
    pub sprint_stamina_per_s: f32,
    /// When a land sprint that ran the stamina dry may start again (a fast swim has its own rule in
    /// the locomotion params). `OnRelease` (the default): after the sprint button is let go.
    /// `Fraction(f)`: once the stamina is at least `f` of its maximum, button held or not;
    /// `Fraction(0)`: the sprint simply stops at an empty bar and goes on as soon as there is any.
    #[serde(skip_serializing_if = "is_on_release")]
    pub sprint_resume: SprintResume,
    /// How far `lock_on` looks for a target (m).
    pub lock_on_range: f32,
}

fn is_on_release(v: &SprintResume) -> bool {
    *v == SprintResume::OnRelease
}

impl Default for CombatTuning {
    fn default() -> Self {
        Self {
            input_buffer: 0.15,
            max_statuses: 3,
            sprint_stamina_per_s: 10.0,
            sprint_resume: SprintResume::OnRelease,
            lock_on_range: 20.0,
        }
    }
}

impl CombatTuning {
    /// The id the engine reads.
    pub const ID: &'static str = "default";

    /// The tuning in `db`, or the defaults.
    pub fn of(db: &shards_content::ContentDb) -> CombatTuning {
        db.get::<CombatTuning>(Self::ID).cloned().unwrap_or_default()
    }
}

impl ContentDef for CombatTuning {
    const KIND: &'static str = "combat_tuning";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if id != Self::ID {
            refs.error(format!("the engine reads only the tuning `{}`; rename this one", Self::ID));
        }
        for (name, v) in [
            ("input_buffer", self.input_buffer),
            ("sprint_stamina_per_s", self.sprint_stamina_per_s),
            ("lock_on_range", self.lock_on_range),
        ] {
            if !(v >= 0.0 && v.is_finite()) {
                refs.error(format!("{name} must be a number >= 0 (is {v})"));
            }
        }
        if let SprintResume::Fraction(f) = self.sprint_resume
            && !(0.0..=1.0).contains(&f)
        {
            refs.error(format!("sprint_resume Fraction({f}) must be between 0 and 1"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(ron: &str) -> ActionDef {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(ron)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn light() -> ActionDef {
        def(r#"(
            net_id: 1,
            phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
            hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 90), damage: 20)],
            cancels: [(span: Secs(0.3, 0.5), into: [Combo("light", "light_2"), Input("dodge")])],
            displacement: [(0.1, 0.0), (0.3, 0.6)],
        )"#)
    }

    /// The text `meters` made before it stopped building a `String` for each number.
    fn meters_by_format(v: f32) -> String {
        let s = format!("{v:.2}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" { "0".to_owned() } else { s.to_owned() }
    }

    #[test]
    fn meters_are_written_without_trailing_zeros() {
        for v in [
            0.0, -0.0, 0.001, -0.001, 0.25, 1.0, 2.0, 4.5, 90.0, 100.0, 1000.0, 12.345, -3.5, 1e10, f32::MAX, f32::MIN,
            f32::MIN_POSITIVE, f32::NAN, f32::INFINITY, f32::NEG_INFINITY,
        ] {
            assert_eq!(meters(v), meters_by_format(v), "{v}");
        }
        assert_eq!(HitShape::Cone { range: 3.0, angle: 90.0, height: 2.0 }.to_string(), "cone 3m 90°");
        assert_eq!(HitShape::Line { length: 8.0, width: 0.5, height: 0.5 }.to_string(), "line 8m×0.5m");
    }

    #[test]
    fn spans_resolve_phases_and_seconds() {
        let d = light();
        assert!((d.length() - 0.6).abs() < 1e-6);
        assert_eq!(d.span(&Span::Phase("active".into())), Some((0.2, 0.3)));
        let both = d.span(&Span::Phases("windup".into(), "active".into())).unwrap();
        assert!((both.0 - 0.0).abs() < 1e-6 && (both.1 - 0.3).abs() < 1e-6);
        assert_eq!(d.span(&Span::Phase("nope".into())), None);
        assert_eq!(d.phase_at(0.0), Some(("windup", 0.2)));
        assert_eq!(d.phase_at(0.25).map(|p| p.0), Some("active"));
        assert_eq!(d.phase_at(0.59).map(|p| p.0), Some("recovery"));
    }

    #[test]
    fn displacement_interpolates_between_keys() {
        let d = light();
        assert_eq!(d.displacement_at(0.0), 0.0);
        assert_eq!(d.displacement_at(0.1), 0.0);
        assert!((d.displacement_at(0.2) - 0.3).abs() < 1e-5);
        assert!((d.displacement_at(0.3) - 0.6).abs() < 1e-5);
        assert!((d.displacement_at(5.0) - 0.6).abs() < 1e-5, "holds the last key");
        let ramp = ActionDef { displacement: vec![(0.5, 2.0)], ..light() };
        assert!((ramp.displacement_at(0.25) - 1.0).abs() < 1e-5, "from (0, 0) to the first key");
    }

    #[test]
    fn windows_touch_the_tick_that_crosses_them() {
        assert!(touches((0.2, 0.3), 0.19, 0.21), "opens mid-tick");
        assert!(touches((0.2, 0.3), 0.29, 0.31), "closes mid-tick");
        assert!(!touches((0.2, 0.3), 0.3, 0.32), "closed");
        assert!(!touches((0.2, 0.3), 0.15, 0.19), "not yet");
        assert!(touches((0.2, 0.205), 0.19, 0.21), "shorter than a tick still counts");
        assert!(touches((0.0, 0.18), 0.0, 0.0), "open at the very start");
        assert!(!touches((0.1, 0.2), 0.0, 0.0));
        assert!(!touches((0.0, 0.1), 0.1, 0.1), "a hold loop jumping back to 0.1 does not reopen [0, 0.1]");
        assert!(touches((0.0, 0.4), 0.1, 0.1));
    }

    #[test]
    fn cancel_windows_choose_combo_or_moveset() {
        let d = light();
        assert_eq!(d.cancel_into("light", 0.29, 0.31), Some(CancelTo::Action("light_2".into())));
        assert_eq!(d.cancel_into("dodge", 0.4, 0.42), Some(CancelTo::Moveset));
        assert_eq!(d.cancel_into("heavy", 0.4, 0.42), None);
        assert_eq!(d.cancel_into("light", 0.1, 0.12), None, "before the window");
        assert_eq!(d.next_cancel("light", 0.1), Some((0.3, CancelTo::Action("light_2".into()))));
        assert_eq!(d.next_cancel("light", 0.35), None, "already open");
        assert_eq!(d.next_cancel("heavy", 0.0), None);
    }

    #[test]
    fn reach_counts_the_lunge() {
        let d = light();
        assert!((d.reach() - (2.0 + 0.3)).abs() < 1e-5, "{}", d.reach());
    }

    #[test]
    fn validation_names_the_field_and_the_fix() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<StatusDef>();
        let actions = r#"{
            "a": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1)],
                hits: [(span: Phase("actve"), shape: Sphere(radius: 1), damage: 5, status: [("bleed", 10)])],
                cancels: [(span: Secs(0.1, 0.9), into: [Combo("light", "b")])],
                displacement: [(0.2, 1.0), (0.1, 2.0)]),
            "c": (net_id: 1, phases: [("x", 0.5)], telegraph: ()),
        }"#;
        let movesets = r#"{ "m": (inputs: {"light": "a", "heavy": "hevy"}, stagger: "c") }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let expected = [
            "content/a.action.ron: action `a`: hits[0].span names an unknown phase `actve` (did you mean `active`?); phases: windup, active",
            "content/a.action.ron: action `a`: hits[0].status -> unknown status `bleed`",
            "content/a.action.ron: action `a`: cancels[0].span 0.1..0.9 must lie inside the action (0..0.3) with from <= to",
            "content/a.action.ron: action `a`: cancels[0].into -> unknown action `b` (did you mean `a`?)",
            "content/a.action.ron: action `a`: displacement keys must go forward in time (key (0.1, 2) after 0.2)",
            "content/a.action.ron: action `a`: net_id 1 is also used by action `c`; net ids name actions on the wire, so each must be unique",
            "content/a.action.ron: action `c`: telegraph has no zone: give it `zone` or the action a hit window",
            "content/a.action.ron: action `c`: net_id 1 is also used by action `a`; net ids name actions on the wire, so each must be unique",
            "content/m.moveset.ron: moveset `m`: inputs.heavy -> unknown action `hevy`",
        ];
        assert_eq!(errors, expected);
    }

    #[test]
    fn a_hold_loop_must_hold_and_not_reopen_windows() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        let actions = r#"{
            "ok": (net_id: 1, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
                hold: (input: "block", span: Phase("hold")),
                guard: (span: Phases("raise", "hold"), perfect: Phase("raise")),
                iframes: [Secs(0.15, 0.3), Phases("hold", "lower")], hyper_armor: [Phases("raise", "hold")]),
            "straddles": (net_id: 2, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
                hold: (input: "block", span: Phase("hold")),
                guard: (span: Secs(0.0, 0.2), perfect: Secs(0.0, 0.12)),
                iframes: [Secs(0.05, 0.3)], hyper_armor: [Secs(0.0, 0.15)]),
            "empty": (net_id: 3, phases: [("raise", 0.2), ("hold", 0.0), ("lower", 0.1)],
                hold: (input: "block", span: Phase("hold")), guard: (span: Phase("raise"))),
        }"#;
        let sources = [shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() }];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let hold = "starts before the hold span 0.1..0.4 and ends inside it: it would reopen on every loop pass; \
                    end it where the hold starts (e.g. `Phase(\"raise\")`) or cover the whole hold";
        let expected = [
            "content/a.action.ron: action `empty`: hold.span 0.2..0.2 is empty; a held action loops inside it, so it must last > 0 s".to_owned(),
            format!("content/a.action.ron: action `straddles`: iframes[0] 0.05..0.3 {hold}"),
            format!("content/a.action.ron: action `straddles`: hyper_armor[0] 0..0.15 {hold}"),
            format!("content/a.action.ron: action `straddles`: guard.span 0..0.2 {hold}"),
            format!("content/a.action.ron: action `straddles`: guard.perfect 0..0.12 {hold}"),
        ];
        assert_eq!(errors, expected);
    }

    #[test]
    fn shapes_read_short() {
        assert_eq!(HitShape::Circle { radius: 4.5, height: 2.0 }.to_string(), "circle 4.5m");
        assert_eq!(HitShape::Cone { range: 3.0, angle: 90.0, height: 2.0 }.to_string(), "cone 3m 90°");
        assert_eq!(HitShape::Line { length: 8.0, width: 1.0, height: 0.5 }.to_string(), "line 8m×1m");
    }
}
