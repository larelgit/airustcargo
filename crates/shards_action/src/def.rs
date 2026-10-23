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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};
use shards_motion::{MotionMode, SprintResume};

use crate::combat::HitWeight;
use crate::status::StatusDef;

/// Slack for comparing times that are sums of tick lengths (s).
pub const EPS: f32 = 1e-4;

// Predicates for `skip_serializing_if`: a field at its default does not serialize, so content
// written before the field existed normalizes to the JSON it always had (replay checks hash it).
fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_one_u32(v: &u32) -> bool {
    *v == 1
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn yes() -> bool {
    true
}

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
#[derive(Serialize, Deserialize, Reflect, Clone, Copy, Debug, PartialEq, Default)]
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
    /// Walls stop it: the target is hit only if a ray from the attacker's chest to the
    /// target's chest meets no [`LayerMask::WORLD`](shards_motion::LayerMask) collider (the
    /// attacker's and the target's own colliders never count: the ray excludes both). Without it
    /// walls do not matter. A target the ray finds occluded is skipped and not marked, so it may
    /// still be hit later in the same window.
    LineOfSight,
    /// A perfect-evade window only evades it (no `PerfectEvaded`): ground zones.
    NoPerfectEvade,
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
    /// Reaction tier asked of the target: 0 none, 1 light, 2 heavy, 3 knockdown. `None`: the
    /// old rule (the moveset's `hit` flinch on any hit that does not break poise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<u8>,
    /// Push (m/s) a reaction to this hit starts with, when larger than the reaction's own.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub knockback: f32,
    /// Stamina a guard pays for this hit, before `GuardDef::stamina_per_damage`. `None`: the
    /// hit's damage (the old rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_damage: Option<f32>,
    /// At most this many targets in one tick, lowest `AiId` first among those the ledger lets
    /// through (a sweep's result cap); 0: every one. A projectile that does not pierce takes the
    /// first along its flight whatever this says.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub max_targets: u8,
    /// Seconds before the window may hit the same target again (a ground zone's pulse); 0: once per
    /// action and group (the old rule). Each target from its own first hit; at least 0.05 s.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub repeat: f32,
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
    /// A perfect guard pays this share of a block's cost (0: free, the old rule). When it
    /// cannot be paid the guard breaks, perfect or not. In `[0, 1]`: above 1 a perfect guard
    /// would cost more than the plain block it falls through to.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub perfect_cost_mult: f32,
    /// A perfect guard staggers the attacker (the old rule). `false`: the engine leaves the
    /// attacker alone; the game's `Parried` observer decides (poise, a recoil through `react`).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub parry_staggers: bool,
    /// Share of a guard-breaking hit's damage that gets through; `None`: `damage_mult`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_damage_mult: Option<f32>,
    /// Share of the hit's status buildup a block lets through (0: none, the old rule).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub status_mult: f32,
    /// Share of the hit's status buildup a guard break lets through (0: none, the old rule).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub break_status_mult: f32,
    /// The perfect window opens only for a guard a fresh press started ([`GuardPress`]): a held
    /// guard that comes back after an attack, a dodge or a hit is a plain block, and mashing the
    /// button parries nothing. `None`: every start of the guard has its perfect window (the old rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perfect_needs_press: Option<GuardPress>,
}

/// When a press of a guard's input arms its perfect window ([`GuardDef::perfect_needs_press`]).
/// A press is the button going down: a press in `Intent::pressed` or the input becoming held.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GuardPress {
    /// A press is fresh (armed) when it comes at least this long after the previous one (s).
    #[serde(default = "arm_gap")]
    pub gap: f32,
    /// The guard opens its perfect window only if it starts within this long of the fresh press (s).
    #[serde(default = "arm_entry")]
    pub entry: f32,
    /// A parry makes the next press fresh whatever the gap.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub rearm: bool,
}

fn arm_gap() -> f32 {
    0.35
}

fn arm_entry() -> f32 {
    0.15
}

impl Default for GuardPress {
    fn default() -> Self {
        Self { gap: arm_gap(), entry: arm_entry(), rearm: true }
    }
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
    /// A press of an input `into` lets through, made while this span is open and before the
    /// window opens, waits for the window instead of expiring after `input_buffer`: its life is
    /// the time until the window opens plus `input_buffer`. A window with a `queue` that starts
    /// where the action ends (`Secs(L, L)`) never cuts the action short: it holds the presses it
    /// lets through until the action is over, and they start from there as usual.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<Span>,
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
    /// A damage-absorbing pool on the actor ([`Barrier`](crate::Barrier)): the larger of its current and
    /// `amount`, for `seconds`. `of_max`: `amount` is a share of max health (`0.15` is 15 %).
    Barrier {
        amount: f32,
        seconds: f32,
        #[serde(default, skip_serializing_if = "is_false")]
        of_max: bool,
    },
    /// Combat objects for the actor's owner (projectiles, ground zones, summons), each running its
    /// own action: `Spawn((prefab: ["bolt"], action: ["bolt_fly"], at: (forward: 0.8, up: 1.2)))`
    /// ([`SpawnDef`](crate::objects::SpawnDef)).
    Spawn(crate::objects::SpawnDef),
    /// A game hook registered with `add_action_effect`: `Custom("spawn_fireball")`.
    Custom(String),
    /// Revives the running action's pair victim if it is downed
    /// ([`revive_downed`](crate::revive_downed)): `health` share of its max (`0.3` is 30 %), then
    /// `grace` seconds in which hits are ignored ([`Grace`](crate::Grace)). On a pair whose `select`
    /// takes a downed ally (`team: Ally, state: Downed`).
    Revive {
        health: f32,
        #[serde(default, skip_serializing_if = "is_zero")]
        grace: f32,
    },
    /// Moves charges from the charge `from` into the charge `charge` of the actor: up to the room
    /// `charge` has (`max - current`) and what `from` holds, at this moment, so a run cut before it
    /// moves nothing (a reload: `Refill(charge: "pistol", from: "pistol.reserve")`). `per_round`
    /// moves one charge instead of all that fit: shell-by-shell reloads put one such effect at each
    /// moment (or let a `hold` loop pass it again and again; an effect in a loop fires on every pass).
    /// [`Refilled`](crate::Refilled) says what moved.
    Refill {
        charge: String,
        from: String,
        #[serde(default, skip_serializing_if = "is_false")]
        per_round: bool,
    },
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
    /// Like `Move`, but with no press direction and no movement input the action goes **backward**,
    /// away from where the body faces (a standing dodge is a backstep). The facing is read when the
    /// action starts. With `face: Keep` ([`ActionDef::face`]) the body keeps that facing (unless a
    /// lock-on target or an aim point turns it), so the backstep hops back still looking ahead and a
    /// second one goes the same way; with `face: Run` (the default) the body turns to the action's
    /// direction like it does for any action, so a standing backstep turns it around. An action with
    /// `turn` spans starts from the body's facing whatever its steer says, so the content check
    /// refuses spans on a backstep (it would go forward).
    MoveOrBack,
}

/// Whether a run turns the body it runs on ([`ActionDef::face`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Face {
    /// The body faces where the run points (the old rule).
    #[default]
    Run,
    /// The run moves the body along its own facing but does not turn it: the body keeps the facing
    /// it had, and only what faces it outside actions (a lock-on target, an aim point) still turns
    /// it (a roll that leaves the body looking where it looked).
    Keep,
}

impl Face {
    fn is_run(&self) -> bool {
        *self == Face::Run
    }
}

/// Attack assist ([`ActionDef::assist`]): how far a run that tracks its lock-on target may be
/// drawn toward it, and when the assist is off.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssistDef {
    /// The facing stays within this many degrees of the direction to the target when the run
    /// started.
    pub angle: f32,
    /// The most (m) the assist's turning may add to the body's approach to the target over the
    /// run: the run's displacement along its turned facing, measured toward the target, beyond
    /// what the same displacement along the direction it started in would give. Past it the
    /// excess is taken off the displacement. `None`: no cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close: Option<f32>,
    /// The assist is off while a `LayerMask::WORLD` collider lies between the chests of the
    /// fighter and its target (at `CombatTuning::sight_height`): the run does not track it then.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sight: bool,
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
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Cost {
    /// Needed in full to start.
    pub stamina: f32,
    /// Seconds before the action can start again.
    pub cooldown: f32,
    /// `charge_count` of this named consumable (a healing flask; a weapon's rounds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub charge: Option<String>,
    /// How many of `charge` a run takes (a shotgun's two shells, a burst of three): all of them must
    /// be there to start. At least 1; needs `charge`.
    #[serde(skip_serializing_if = "is_one_u32")]
    pub charge_count: u32,
    /// Whether `stamina` must be there in full to start.
    #[serde(skip_serializing_if = "StaminaMode::is_full")]
    pub stamina_mode: StaminaMode,
    /// The cooldown slot this action shares with others (variants of one technique); `None`:
    /// its own id (the old rule).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_key: Option<String>,
    /// When the action ends before its windup is over ([`ActionDef::windup_end`]) by anything
    /// but its own end or a chain restart (forced, killed, released, or cancelled into an action
    /// that is not a `Combo` continuation; a dodge out of the windup counts), its cooldown is set
    /// to this share of `cooldown` instead (0.4). `None`: the full cooldown stays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_if_interrupted: Option<f32>,
}

/// Charge by hold ([`ActionDef::charge`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChargeDef {
    /// Seconds held before it may turn.
    pub after: f32,
    /// The charged action.
    pub into: String,
    /// The last second it may turn; default: the end of the windup ([`ActionDef::windup_end`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<f32>,
}

/// A leap ([`ActionDef::leap`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeapDef {
    /// Height of the arc (m): the body is launched up at `at` so that it peaks this high (with its
    /// own gravity, [`Launch`](shards_motion::Launch)).
    pub height: f32,
    /// Seconds into the action of the launch; the body must stand on the ground then (or a
    /// jump's coyote time from it) or nothing launches and the action goes on.
    pub at: f32,
    /// Choose the distance when the action starts; without it the authored `displacement` is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<LeapSearch>,
}

/// How a leap picks where it lands ([`LeapDef::search`], [`crate::leap_distance`]): candidate
/// distances from the farthest to the nearest, the first whose landing point is clear and level
/// enough wins.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LeapSearch {
    /// The shortest leap (m); a leap with no landing from `max` down to here is refused.
    pub min: f32,
    pub max: f32,
    /// How much shorter each next candidate is.
    pub step: f32,
    /// How much higher (m) the landing may be than the feet.
    pub max_rise: f32,
    /// How much lower.
    pub max_drop: f32,
    /// Height (m) of the wall ray above the feet.
    pub probe_height: f32,
    /// The body is placed this far above the floor for the overlap test.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lift: f32,
    /// With a lock-on target: leap toward it, as far as its edge.
    #[serde(default, skip_serializing_if = "is_false")]
    pub toward_target: bool,
    /// A landing where the water is deeper than this (m) is refused. `None`: water is no reason.
    /// The depth is the top of the highest water volume at the landing point above the floor there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_water_depth: Option<f32>,
}

/// How an action's stamina cost gates its start.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StaminaMode {
    /// The whole cost must be there (the old rule).
    #[default]
    Full,
    /// Any stamina above 0 lets it start; the cost drains stamina to 0 at most.
    Any,
}

impl StaminaMode {
    fn is_full(&self) -> bool {
        *self == StaminaMode::Full
    }
}

impl Default for Cost {
    fn default() -> Self {
        Self {
            stamina: 0.0,
            cooldown: 0.0,
            charge: None,
            charge_count: 1,
            stamina_mode: StaminaMode::default(),
            cooldown_key: None,
            cooldown_if_interrupted: None,
        }
    }
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
    /// Turning speed (degrees per second) of the body while this action runs, instead of the
    /// locomotion params' (a raised guard turns slower than a free walk). `None`: the params' rate
    /// (its locked rate while the action names a facing). Written to `MoveModifiers::turn_speed_deg`
    /// by [`actions_to_modifiers`](crate::actions_to_modifiers), which sees the action as it was at the
    /// start of the tick (on a client's predicted body, the action it shows). `0` means "turns at
    /// once", as everywhere in the locomotion params, not "cannot turn".
    ///
    /// A rate of the body, not of the run: the run's own facing (where its hits, guard and
    /// displacement point) turns only by [`turn`](Self::turn) (or snaps by `track_until`), and the
    /// controller then turns the body toward it once a tick at this rate. With both on one action
    /// each bounds its own: the run turns at the spans' rate and the body follows it no faster than
    /// this, so a lower rate here leaves the body behind its own swing, never turned twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_speed_deg: Option<f32>,
    /// While this action runs, stamina does not regenerate; when it ends (a block is let go) the
    /// regeneration waits this many seconds more. The wait is never shorter than the one the last
    /// spend left (`Stamina::delay`). `None`: the action holds nothing. A held guard with a
    /// `1.3` drop delay is `regen_hold: 1.3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regen_hold: Option<f32>,
    /// Animation clip; the animation follows the action, never the other way round. In a game
    /// with animsets the content check wants one of them to play it ([`AnimSet`](crate::AnimSet)):
    /// leave it out for an action that shows no clip of its own (the locomotion clip plays on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Reaction armour while a span is open (sampled at the instant of the tick, `start <= t <= end`):
    /// hits asking a tier at most `level` cause no reaction. Poise damage still lands (`hyper_armor` is the one that ignores poise).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub armor: Vec<(Span, u8)>,
    /// Inside `iframes`: a hit evaded while one of these is open too is a perfect evade.
    /// Sampled at the instant of the tick (`start <= t <= end`), like `armor`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub perfect_iframes: Vec<Span>,
    /// Charge by hold: while the input that started this action has been held on every tick,
    /// from `after` seconds on (until `until`) it turns into `into` at the same time, paying only
    /// the stamina difference; while that cannot be paid it tries again each tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charge: Option<ChargeDef>,
    /// Turn rate while tracking, per span, in degrees per second (the largest of the spans open
    /// in a tick, for the whole tick: a span that opens or closes inside a tick turns it a full
    /// tick); outside every span the facing holds. The run starts from the body's facing, with no
    /// snap, and turns toward its lock-on target, else the aim point, else the movement input (a
    /// player steering the attack). Empty: `track_until` (the old rule). Not both.
    ///
    /// The spans turn the run's facing (`Running::facing`: its hits, guard arc and displacement),
    /// not the body: with `face: Run` the body turns toward it in the controller, once a tick, at
    /// the controller's rate ([`turn_speed_deg`](Self::turn_speed_deg) of the action, else the
    /// locked rate, else the free one). A span faster than that rate leaves the body behind its swing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turn: Vec<(Span, f32)>,
    /// Attack assist: with a lock-on target, the facing stays within `angle` degrees of the
    /// direction to the target when the action started, the turning adds at most `close` metres
    /// of approach, and the assist is off (no tracking of the target) while the target is out of
    /// sight (`sight`) or the fighter turned it off (its `Intent` holds `assist_off`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assist: Option<AssistDef>,
    /// Seconds added to the action when its last hit window has passed and none of its hits
    /// landed (a hit, a block, a parry or a guard break).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub miss_recovery: f32,
    /// The action goes on only while this input is held; letting go ends it (`Released`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub while_held: Option<String>,
    /// Whether the run turns the body (`Run`, the default) or leaves its facing alone (`Keep`: a
    /// roll that moves the body without turning it).
    #[serde(default, skip_serializing_if = "Face::is_run")]
    pub face: Face,
    /// Weight floor of this action's hits (`Counter` for a counterattack): the `weight` of its
    /// `Damaged`, `Blocked` and `GuardBroken` events is at least this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<HitWeight>,
    /// A paired action (a finisher, a revive): it takes a victim when it starts ([`PairDef`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<PairDef>,
    /// A leap: a launch up at `leap.at` (the body must stand on the ground then) and, with a
    /// `search`, a distance chosen when the action starts (`displacement` is scaled to it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leap: Option<LeapDef>,
    /// The body walks while a span is open, at this share of its walking speed (the largest of the
    /// spans open in a tick); the action's own `displacement` still applies, and outside every span
    /// the action owns the body. Empty: `move_speed` decides (the old rule). Not both.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub walk: Vec<(Span, f32)>,
    /// Hits find nothing while a span is open (burrowed): no hit, no ledger mark, no pair.
    /// Sampled like `iframes`, over the whole tick ([`ActionDef::intangible_in`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intangible: Vec<Span>,
    /// While intangible the body also passes through other characters ([`Ghost`](shards_motion::Ghost)).
    #[serde(default, skip_serializing_if = "is_false")]
    pub ghost: bool,
    /// The action that starts when this one finishes (its time ran out; not when it is cut): a
    /// summon's pulse loops on itself, a zone's phases chain. Its `at: 0` effects come on the
    /// start, in the tick this one ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<String>,
    /// A projectile's flight action: the action it runs where it stops (a wall, the first target
    /// it meets, unless it pierces). None: it ends there. Not at its lifetime's end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
}

/// A paired action ([`ActionDef::pair`]): when it starts, the fighter takes a victim that
/// [`PairSelect`] finds, and the two are held together until the run ends, the victim is lost
/// or the attacker is cut.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairDef {
    pub select: PairSelect,
    /// Forced on the victim when the pair starts (a held pose); its length only has to outlast the
    /// strike: the strike's reaction replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub victim_action: Option<String>,
    /// The attacker's hits find only its victim while the pair lasts (nobody else in their shapes);
    /// other attackers can still hit the victim.
    #[serde(default, skip_serializing_if = "is_false")]
    pub only_victim: bool,
    /// Other attackers find the victim untouchable while the pair lasts (a revive). Off for a
    /// finisher: there only the attacker is restricted (`only_victim`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub exclusive: bool,
    /// A broken victim's poise stays broken at least this long from the pair's start (s).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub hold_break: f32,
    /// The pair's first landed hit on a broken victim ends its break (one pair per break).
    #[serde(default, skip_serializing_if = "is_false")]
    pub ends_break: bool,
    /// The pair is lost, and the action cut, when the two are farther apart than this (m, on the
    /// ground plane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_range: Option<f32>,
    /// Damage (> 0) to the attacker cuts the action.
    #[serde(default, skip_serializing_if = "is_false")]
    pub cancel_on_hurt: bool,
    /// Reaction tier a landed strike forces on a victim that is still in the pair's state and
    /// survives it, ignoring its armour and immunity and starting none (1 light, 2 heavy, 3
    /// knockdown; 0: none). It replaces the hit's own reaction.
    #[serde(default = "knockdown", skip_serializing_if = "is_knockdown")]
    pub strike_reaction: u8,
}

fn knockdown() -> u8 {
    3
}

fn is_knockdown(v: &u8) -> bool {
    *v == 3
}

/// Which fighter a paired action takes ([`PairDef::select`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PairSelect {
    /// Whose side the victim is on.
    #[serde(default, skip_serializing_if = "PairTeam::is_hostile")]
    pub team: PairTeam,
    /// The state it must be in.
    pub state: PairState,
    /// Reach (m) on the ground plane, added to the victim's body radius.
    pub range: f32,
    /// Largest height difference (m).
    pub height: f32,
    /// Largest angle (degrees, half-angle) between the angle origin ([`PairSelect::angle_from`]: the body's facing, not its lock-on
    /// direction or its movement input, unless asked) and the victim.
    pub angle: f32,
    /// Where `angle` is measured from: the body's facing (the default), or the direction the fighter means (`PairAngle::Intent`).
    #[serde(default, skip_serializing_if = "PairAngle::is_facing")]
    pub angle_from: PairAngle,
    /// No `LayerMask::WORLD` collider between the two chests (`CombatTuning::sight_height`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub line_of_sight: bool,
    /// The fighter's lock-on target wins when it qualifies; otherwise the nearest one does.
    #[serde(default, skip_serializing_if = "is_false")]
    pub prefer_target: bool,
    /// Offered only if no other player (a fighter with `PlayerId`) of the fighter's team, alive, is
    /// nearer to the victim by more than this (m, on the ground plane).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slack: Option<f32>,
}

/// Where a pair's `angle` is measured from ([`PairSelect::angle_from`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PairAngle {
    /// The body's facing (design 6.4 and the original game's finisher candidate): a player running away from a victim has no pair
    /// on it, a locked player whose body looks elsewhere is judged by the body.
    #[default]
    Facing,
    /// The direction the fighter means: the lock-on direction first, else the movement input's direction, else the body's
    /// facing. A locked fighter whose body looks away can still take the victim it is locked on to; one running away from it cannot.
    Intent,
}

impl PairAngle {
    fn is_facing(&self) -> bool {
        *self == PairAngle::Facing
    }
}

/// Whose side a pair victim is on ([`PairSelect::team`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PairTeam {
    /// One the fighter may hit (another team, or no team).
    #[default]
    Hostile,
    /// One of its own team (a revive).
    Ally,
}

impl PairTeam {
    fn is_hostile(&self) -> bool {
        *self == PairTeam::Hostile
    }
}

/// The state a pair victim must be in ([`PairSelect::state`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairState {
    /// Its poise is broken (and it is not down).
    Broken,
    /// Any (alive, not down, not paired by another).
    Any,
    /// It is down ([`Downed`](crate::Downed)): a revive, with `team: Ally`. The other states never
    /// take a downed body.
    Downed,
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

    /// Whether any of `spans` is open at the instant `t` (`start <= t <= end`), not over the whole
    /// tick that ends there: the sampling the `armor` and `perfect_iframes` windows use, so a window
    /// whose end falls between two ticks is not open for one more tick than its length says.
    pub fn any_open_at(&self, spans: &[Span], t: f32) -> bool {
        spans.iter().filter_map(|s| self.span(s)).any(|w| w.0 <= t + EPS && t <= w.1 + EPS)
    }

    /// The highest reaction armour level of the `armor` spans open at the instant `t`
    /// (see [`ActionDef::any_open_at`]); 0 if none.
    pub fn armor_at(&self, t: f32) -> u8 {
        self.armor
            .iter()
            .filter(|(span, _)| self.span(span).is_some_and(|w| w.0 <= t + EPS && t <= w.1 + EPS))
            .map(|(_, level)| *level)
            .max()
            .unwrap_or(0)
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
            .filter(|c| !self.holds_to_end(c) && self.span(&c.span).is_some_and(|w| touches(w, prev, t)))
            .find_map(|c| transition(&c.into, input))
    }

    /// When the next cancel window that lets `input` through opens after `t`, and what
    /// the press turns into there.
    pub fn next_cancel(&self, input: &str, t: f32) -> Option<(f32, CancelTo)> {
        self.cancels
            .iter()
            .filter(|c| !self.holds_to_end(c))
            .filter_map(|c| Some((self.span(&c.span)?, transition(&c.into, input)?)))
            .filter(|(w, _)| w.0 > t + EPS)
            .min_by(|a, b| a.0 .0.total_cmp(&b.0 .0))
            .map(|(w, to)| (w.0, to))
    }

    /// A queue-only window: one with a `queue` that starts where the action ends. It never cuts
    /// the action short; it holds presses until the action is over ([`CancelDef::queue`]).
    fn holds_to_end(&self, c: &CancelDef) -> bool {
        c.queue.is_some() && self.span(&c.span).is_some_and(|w| w.0 >= self.length() - EPS)
    }

    /// When the window a press of `input` made in the tick `(prev, t]` waits for opens: the
    /// earliest cancel window that lets `input` through, whose `queue` span is open in that tick
    /// and which opens after `t`. `None`: the press lives `input_buffer` as usual.
    pub fn queue_until(&self, input: &str, prev: f32, t: f32) -> Option<f32> {
        self.cancels
            .iter()
            .filter(|c| c.queue.as_ref().and_then(|q| self.span(q)).is_some_and(|q| touches(q, prev, t)))
            .filter(|c| transition(&c.into, input).is_some())
            .filter_map(|c| self.span(&c.span).map(|w| w.0))
            .filter(|open| *open > t + EPS)
            .min_by(f32::total_cmp)
    }

    /// When the windup is over: the start of the first hit window, else the end of the first
    /// phase (an action with no hits). A charge may turn until then, and a cut before it keeps
    /// only `cost.cooldown_if_interrupted` of the cooldown.
    pub fn windup_end(&self) -> f32 {
        let first_hit = self.hits.iter().filter_map(|h| self.span(&h.span)).map(|w| w.0).min_by(f32::total_cmp);
        first_hit.unwrap_or_else(|| self.phases.first().map_or(0.0, |p| p.1.max(0.0)))
    }

    /// When the last hit window closes (0 without hits): the miss check comes then.
    pub fn last_hit_end(&self) -> f32 {
        self.hits.iter().filter_map(|h| self.span(&h.span)).map(|w| w.1).fold(0.0, f32::max)
    }

    /// The cooldown slot of this action, whose id is `id`.
    pub fn cooldown_key<'s>(&'s self, id: &'s str) -> &'s str {
        self.cost.cooldown_key.as_deref().unwrap_or(id)
    }

    /// The fastest turn rate (degrees per second) of the `turn` spans open in the tick
    /// `(prev, t]`; `None` when none is.
    pub fn turn_rate(&self, prev: f32, t: f32) -> Option<f32> {
        self.turn
            .iter()
            .filter(|(span, _)| self.span(span).is_some_and(|w| touches(w, prev, t)))
            .map(|(_, rate)| *rate)
            .max_by(f32::total_cmp)
    }

    /// The share of its walking speed the body walks at in the tick `(prev, t]`: the largest of the
    /// `walk` spans open in it; `None` when none is (the action owns the body).
    pub fn walk_factor(&self, prev: f32, t: f32) -> Option<f32> {
        self.walk
            .iter()
            .filter(|(span, _)| self.span(span).is_some_and(|w| touches(w, prev, t)))
            .map(|(_, m)| *m)
            .max_by(f32::total_cmp)
    }

    /// Whether an `intangible` span is open in the tick `(prev, t]` (the tick counts as a whole, like
    /// `iframes`): hits, pairs and perception find nothing of the body.
    pub fn intangible_in(&self, prev: f32, t: f32) -> bool {
        self.any_open(&self.intangible, prev, t)
    }

    /// Whether the body passes through other characters in the tick `(prev, t]`: the action has
    /// `ghost` and is intangible then.
    pub fn ghost_in(&self, prev: f32, t: f32) -> bool {
        self.ghost && self.intangible_in(prev, t)
    }

    /// Whether a run of this action keeps a [`RunExtras`](crate::RunExtras): it charges, turns
    /// by spans, assists or has miss recovery.
    pub fn needs_extras(&self) -> bool {
        self.charge.is_some() || !self.turn.is_empty() || self.assist.is_some() || self.miss_recovery > 0.0
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

    /// The [`Effect::Refill`]s of this action, in order: `(charge, from, per_round)`.
    pub fn refills(&self) -> impl Iterator<Item = (&str, &str, bool)> + '_ {
        self.effects.iter().filter_map(|e| match &e.effect {
            Effect::Refill { charge, from, per_round } => Some((charge.as_str(), from.as_str(), *per_round)),
            _ => None,
        })
    }

    /// Whether a run of this action would move something into `charge` now, given what the fighter
    /// holds: it has a `Refill` into `charge` whose target has room and whose source has a charge.
    pub fn refills_now(&self, charge: &str, charges: Option<&crate::Charges>) -> bool {
        self.refills().any(|(into, from, _)| {
            into == charge
                && charges.is_some_and(|c| {
                    c.get(into).is_some_and(|t| t.current < t.max) && c.get(from).is_some_and(|s| s.current > 0)
                })
        })
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
        match &self.cost.charge {
            Some(charge) if charge.is_empty() => {
                check.refs.error("cost.charge is empty; name the consumable it takes, e.g. `flask`");
            }
            Some(_) if self.cost.charge_count == 0 => {
                check.refs.error("cost.charge_count must be at least 1 (is 0): it is how many charges a run takes");
            }
            None if self.cost.charge_count != 1 => check.refs.error(format!(
                "cost.charge_count {} counts the charges `cost.charge` takes, and the cost names none: give `charge`, or leave the count out",
                self.cost.charge_count
            )),
            _ => {}
        }
        check.number("track_until", self.track_until);
        if let Some(m) = self.move_speed {
            check.number("move_speed", m);
        }
        if let Some(t) = self.turn_speed_deg {
            check.number("turn_speed_deg", t);
        }
        if let Some(h) = self.regen_hold {
            check.number("regen_hold", h);
        }
        for (i, hit) in self.hits.iter().enumerate() {
            let at = format!("hits[{i}]");
            check.span(&format!("{at}.span"), &hit.span);
            check.number(&format!("{at}.damage"), hit.damage);
            check.number(&format!("{at}.poise"), hit.poise);
            check.number(&format!("{at}.knockback"), hit.knockback);
            if let Some(g) = hit.guard_damage {
                check.number(&format!("{at}.guard_damage"), g);
            }
            if let Some(tier) = hit.reaction
                && tier > 3
            {
                check.refs.error(format!("{at}.reaction must be a tier 0 to 3 (is {tier}): 1 light, 2 heavy, 3 knockdown"));
            }
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
            if hit.repeat != 0.0 && !(hit.repeat >= MIN_REPEAT && hit.repeat.is_finite()) {
                check.refs.error(format!("{at}.repeat must be 0 (once per run) or at least {MIN_REPEAT} s (is {})", hit.repeat));
            }
        }
        for (i, span) in self.iframes.iter().enumerate() {
            check.window(&format!("iframes[{i}]"), span);
        }
        for (i, span) in self.hyper_armor.iter().enumerate() {
            check.window(&format!("hyper_armor[{i}]"), span);
        }
        for (i, (span, level)) in self.armor.iter().enumerate() {
            check.span(&format!("armor[{i}]"), span);
            if *level > 3 {
                check.refs.error(format!("armor[{i}] level must be 0 to 3 (is {level}): a reaction tier at most this causes no reaction"));
            }
        }
        for (i, span) in self.perfect_iframes.iter().enumerate() {
            check.window(&format!("perfect_iframes[{i}]"), span);
            check.inside_iframes(&format!("perfect_iframes[{i}]"), span);
        }
        if let Some(guard) = &self.guard {
            check.window("guard.span", &guard.span);
            if let Some(perfect) = &guard.perfect {
                check.window("guard.perfect", perfect);
            }
            check.number("guard.damage_mult", guard.damage_mult);
            check.number("guard.stamina_per_damage", guard.stamina_per_damage);
            check.number("guard.poise_mult", guard.poise_mult);
            check.number("guard.status_mult", guard.status_mult);
            check.number("guard.break_status_mult", guard.break_status_mult);
            if let Some(share) = guard.break_damage_mult {
                check.number("guard.break_damage_mult", share);
            }
            if !(0.0..=1.0).contains(&guard.perfect_cost_mult) {
                check.refs.error(format!(
                    "guard.perfect_cost_mult must be between 0 and 1 (is {}): above 1 a perfect guard would cost more than the plain block it falls through to",
                    guard.perfect_cost_mult
                ));
            }
            if !(guard.arc > 0.0 && guard.arc <= 360.0) {
                check.refs.error(format!("guard.arc must be in (0, 360] degrees (is {})", guard.arc));
            }
            if let Some(press) = &guard.perfect_needs_press {
                check.number("guard.perfect_needs_press.gap", press.gap);
                check.number("guard.perfect_needs_press.entry", press.entry);
                if guard.perfect.is_none() {
                    check.refs.error("guard.perfect_needs_press arms a perfect window, and the guard has none: give guard.perfect");
                }
            }
        }
        if let Some(pair) = &self.pair {
            check.pair(pair);
        }
        for (i, cancel) in self.cancels.iter().enumerate() {
            check.span(&format!("cancels[{i}].span"), &cancel.span);
            for tr in &cancel.into {
                if let Transition::Combo(_, action) = tr {
                    check.refs.require::<ActionDef>(action, &format!("cancels[{i}].into"));
                }
            }
            if let Some(queue) = &cancel.queue {
                check.span(&format!("cancels[{i}].queue"), queue);
            }
        }
        check.timeline(id);
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
                Effect::Barrier { amount, seconds, .. } => {
                    for (name, v) in [("amount", *amount), ("seconds", *seconds)] {
                        if !(v > 0.0 && v.is_finite()) {
                            check.refs.error(format!("effects[{i}] Barrier {name} must be a number above 0 (is {v})"));
                        }
                    }
                }
                Effect::Spawn(spawn) => check.spawn(&format!("effects[{i}] Spawn"), spawn),
                // The sound is checked where the mixer is (`AudioDef`), as a telegraph's is.
                Effect::Sound(_) | Effect::Custom(_) => {}
                Effect::Refill { charge, from, .. } => {
                    for (name, value) in [("charge", charge), ("from", from)] {
                        if value.is_empty() {
                            check.refs.error(format!("effects[{i}] Refill {name} is empty; name the charge, e.g. `pistol`"));
                        }
                    }
                    if charge == from {
                        check.refs.error(format!(
                            "effects[{i}] Refill moves charges from `{from}` into `{charge}`, the same charge: name the reserve it draws from"
                        ));
                    }
                }
                Effect::Revive { health, grace } => {
                    if !(*health > 0.0 && *health <= 1.0) {
                        check.refs.error(format!("effects[{i}] Revive health is a share of max health, above 0 and at most 1 (is {health})"));
                    }
                    if !(*grace >= 0.0 && grace.is_finite()) {
                        check.refs.error(format!("effects[{i}] Revive grace must be a number of seconds, 0 or more (is {grace})"));
                    }
                    if self.pair.as_ref().is_none_or(|p| p.select.state != PairState::Downed) {
                        check.refs.error(format!(
                            "effects[{i}] Revive revives the pair's victim: give the action a `pair` whose select has `state: Downed`"
                        ));
                    }
                }
            }
        }
        if let Some(then) = &self.then {
            check.refs.require::<ActionDef>(then, "then");
        }
        if let Some(impact) = &self.impact {
            check.refs.require::<ActionDef>(impact, "impact");
        }
        if let Some(circle) = spawn_circle(id, check.refs) {
            check.refs.error(format!(
                "spawns and impacts go round in a circle ({circle}): each would start the next for ever; break the circle"
            ));
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

/// The shortest repeat of a hit ([`HitDef::repeat`]): three ticks.
pub const MIN_REPEAT: f32 = 0.05;

/// A circle from action `id` back to itself along `then`, `impact` and `Spawn` edges that takes at
/// least one `impact` or `Spawn` edge (a loop of `then` alone is one object pulsing, and allowed):
/// `a -> Spawn b -> Spawn a`. The shortest one, the edges taken in their order in the definitions.
fn spawn_circle(id: &str, refs: &Refs) -> Option<String> {
    // Breadth first over (action, has the path taken a spawn or impact edge yet).
    let mut seen: Vec<(String, bool)> = vec![(id.to_owned(), false)];
    let mut came: Vec<Option<(usize, String)>> = vec![None];
    let mut at = 0;
    while at < seen.len() {
        let (node, spawned) = seen[at].clone();
        if let Some(def) = refs.get::<ActionDef>(&node) {
            let spawns = def.effects.iter().filter_map(|e| match &e.effect {
                Effect::Spawn(s) => Some(s.action.iter()),
                _ => None,
            });
            let edges = def
                .then
                .iter()
                .map(|t| ("then", t, spawned))
                .chain(def.impact.iter().map(|t| ("impact", t, true)))
                .chain(spawns.flatten().map(|t| ("Spawn", t, true)));
            for (edge, next, flag) in edges {
                if next == id && flag {
                    let mut path = vec![format!("{edge} {next}")];
                    let mut back = at;
                    while let Some((from, edge)) = &came[back] {
                        path.push(format!("{edge} {}", seen[back].0));
                        back = *from;
                    }
                    path.push(id.to_owned());
                    path.reverse();
                    return Some(path.join(" -> "));
                }
                if !seen.iter().any(|(n, f)| n == next && *f == flag) {
                    seen.push((next.clone(), flag));
                    came.push(Some((at, edge.to_owned())));
                }
            }
        }
        at += 1;
    }
    None
}

/// Whether prefab `id` (or one it extends) has a component named `name` (`Projectile` or a path
/// ending in `::Projectile`).
fn prefab_has(refs: &Refs, id: &str, name: &str) -> bool {
    let mut at = id.to_owned();
    for _ in 0..16 {
        let Some(prefab) = refs.get::<shards_content::PrefabDef>(&at) else { return false };
        if prefab.components.keys().any(|k| k == name || k.ends_with(&format!("::{name}"))) {
            return true;
        }
        match &prefab.extends {
            Some(next) => at = next.clone(),
            None => return false,
        }
    }
    false
}

impl Check<'_, '_, '_> {
    /// A [`Spawn`](Effect::Spawn) effect's fields.
    fn spawn(&mut self, at: &str, def: &crate::objects::SpawnDef) {
        use crate::objects::{SpawnAim, SpawnPattern};
        if def.prefab.is_empty() {
            self.refs.error(format!("{at}.prefab is empty; name the prefab each object is spawned from"));
        }
        for prefab in &def.prefab {
            self.refs.require::<shards_content::PrefabDef>(prefab, &format!("{at}.prefab"));
        }
        if def.action.is_empty() {
            self.refs.error(format!("{at}.action is empty; name the action each object runs"));
        }
        for action in &def.action {
            self.refs.require::<ActionDef>(action, &format!("{at}.action"));
        }
        if def.count == 0 {
            self.refs.error(format!("{at}.count must be at least 1"));
        }
        self.number(&format!("{at}.delay"), def.delay);
        if let Some(depth) = def.ground
            && !(depth > 0.0 && depth.is_finite())
        {
            self.refs.error(format!("{at}.ground must be a number above 0 (is {depth})"));
        }
        let mut finite = vec![("at.forward", def.at.forward), ("at.up", def.at.up), ("at.right", def.at.right)];
        if let SpawnAim::Target { height } = def.aim {
            finite.push(("aim.height", height));
        }
        match def.pattern {
            SpawnPattern::Fan { spread } => finite.push(("pattern.spread", spread)),
            SpawnPattern::Line { start, step } => finite.extend([("pattern.start", start), ("pattern.step", step)]),
            SpawnPattern::Radial { lines, start, step } => {
                if lines == 0 {
                    self.refs.error(format!("{at}.pattern.lines must be at least 1"));
                }
                finite.extend([("pattern.start", start), ("pattern.step", step)]);
            }
        }
        for (name, v) in finite {
            if !v.is_finite() {
                self.refs.error(format!("{at}.{name} must be a number (is {v})"));
            }
        }
        self.spawn_counts(at, def);
        self.flight_shapes(at, def);
    }

    /// One spawn must fit under the caps in `CombatTuning`: more than `max_objects_per_owner` (or
    /// `max_objects`) in one `Spawn` is always cut short, so it is a content mistake.
    fn spawn_counts(&mut self, at: &str, def: &crate::objects::SpawnDef) {
        let tuning = self.refs.get::<CombatTuning>(CombatTuning::ID).cloned().unwrap_or_default();
        let cap = tuning.max_objects.min(tuning.max_objects_per_owner) as usize;
        if def.total() > cap {
            self.refs.error(format!(
                "{at} spawns {} objects at once, over the caps of the combat tuning ({cap} = the smaller of max_objects and max_objects_per_owner); lower count or lines",
                def.total()
            ));
        }
    }

    /// A projectile's hit windows sweep the segment it flew only when they are spheres; any other
    /// shape of its flight action would be tested where it stands and let it through at speed.
    fn flight_shapes(&mut self, at: &str, def: &crate::objects::SpawnDef) {
        let pairs = def.prefab.len().max(def.action.len());
        for i in 0..pairs {
            let (Some(prefab), Some(action)) = (def.prefab.get(i).or(def.prefab.last()), def.action.get(i).or(def.action.last())) else { continue };
            if !prefab_has(self.refs, prefab, "Projectile") {
                continue;
            }
            let Some(flight) = self.refs.get::<ActionDef>(action) else { continue };
            if let Some((n, _)) = flight.hits.iter().enumerate().find(|(_, h)| !matches!(h.shape, HitShape::Sphere { .. })) {
                self.refs.error(format!(
                    "{at}: `{action}` is the flight of the projectile `{prefab}`, and its hits[{n}] is not a Sphere; only spheres sweep the segment a projectile flies (use a Sphere, or an `impact` action for a burst)"
                ));
            }
        }
    }

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

    /// A paired action's numbers and the victim action it forces.
    fn pair(&mut self, pair: &PairDef) {
        let s = &pair.select;
        for (name, v) in [("range", s.range), ("height", s.height), ("angle", s.angle)] {
            self.number(&format!("pair.select.{name}"), v);
        }
        if s.angle > 180.0 {
            self.refs.error(format!("pair.select.angle is a half-angle, at most 180 degrees (is {})", s.angle));
        }
        if let Some(slack) = s.slack {
            self.number("pair.select.slack", slack);
        }
        self.number("pair.hold_break", pair.hold_break);
        if let Some(range) = pair.break_range {
            self.number("pair.break_range", range);
        }
        if pair.strike_reaction > 3 {
            self.refs.error(format!(
                "pair.strike_reaction must be a tier 0 to 3 (is {}): 1 light, 2 heavy, 3 knockdown, 0 none",
                pair.strike_reaction
            ));
        }
        if let Some(victim) = &pair.victim_action {
            self.refs.require::<ActionDef>(victim, "pair.victim_action");
        }
        if self.def.charge.is_some() {
            // A charge turns the run into another action, and the pair would still name this one.
            self.refs.error("a paired action has no charge: the charged action would run on with a pair that names this one; leave out `charge` or `pair`");
        }
    }

    /// A perfect-evade window lies inside the union of the i-frames, else it never opens.
    fn inside_iframes(&mut self, field: &str, span: &Span) {
        let Some((from, to)) = self.def.span(span) else { return };
        let mut frames: Vec<(f32, f32)> = self.def.iframes.iter().filter_map(|s| self.def.span(s)).collect();
        frames.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Merge touching windows, then ask whether one holds the whole span.
        let mut merged: Vec<(f32, f32)> = Vec::new();
        for w in frames {
            match merged.last_mut() {
                Some(last) if w.0 <= last.1 + EPS => last.1 = last.1.max(w.1),
                _ => merged.push(w),
            }
        }
        if !merged.iter().any(|w| from >= w.0 - EPS && to <= w.1 + EPS) {
            self.refs.error(format!(
                "{field} {}..{} lies outside the i-frames: a perfect-evade window outside the i-frames never opens",
                meters(from),
                meters(to)
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

    /// The leap, the walk spans, the intangible spans and ghost.
    fn body_and_motion(&mut self) {
        let def = self.def;
        if let Some(leap) = &def.leap {
            if !(leap.height > 0.0 && leap.height.is_finite()) {
                self.refs.error(format!("leap.height must be a number > 0 (is {}): the peak of the arc in metres", leap.height));
            }
            if !(leap.at >= 0.0 && leap.at < self.length - EPS) {
                self.refs.error(format!(
                    "leap.at {} must lie in the action [0, {}): it is when the body launches",
                    leap.at,
                    meters(self.length)
                ));
            }
            if def.hold.is_some() {
                self.refs.error("a leap and a hold loop in one action: the loop would pass the launch again; leave one out");
            }
            if leap.search.is_some() && (!def.turn.is_empty() || def.assist.is_some()) {
                self.refs.error(
                    "a leap with a search and `turn` spans or an `assist`: the run would turn away from the direction the search chose, and the landing with it; leave them out",
                );
            }
            if let Some(s) = &leap.search {
                if def.displacement_at(self.length) <= EPS {
                    self.refs.error(
                        "leap.search chooses a distance and scales `displacement` to it, which is empty or 0: give the action the displacement of its longest leap",
                    );
                }
                for (name, v) in [
                    ("min", s.min),
                    ("max", s.max),
                    ("max_rise", s.max_rise),
                    ("max_drop", s.max_drop),
                    ("lift", s.lift),
                ] {
                    self.number(&format!("leap.search.{name}"), v);
                }
                if !(s.step > 0.0 && s.step.is_finite()) {
                    self.refs.error(format!("leap.search.step must be a number > 0 (is {}): each candidate is this much shorter", s.step));
                }
                if !(s.probe_height > 0.0 && s.probe_height.is_finite()) {
                    self.refs.error(format!(
                        "leap.search.probe_height must be a number > 0 (is {}): the height of the wall ray above the feet",
                        s.probe_height
                    ));
                }
                if s.min > s.max {
                    self.refs.error(format!("leap.search.min {} is above max {}", s.min, s.max));
                }
                if let Some(depth) = s.max_water_depth {
                    self.number("leap.search.max_water_depth", depth);
                }
            }
        }
        for (i, (span, m)) in def.walk.iter().enumerate() {
            self.span(&format!("walk[{i}]"), span);
            self.number(&format!("walk[{i}] share"), *m);
        }
        if !def.walk.is_empty() && def.move_speed.is_some() {
            self.refs.error("walk and move_speed both say how the body walks during the action; give one (walk spans, or move_speed)");
        }
        for (i, span) in def.intangible.iter().enumerate() {
            self.window(&format!("intangible[{i}]"), span);
        }
        if def.ghost && def.intangible.is_empty() {
            self.refs.error("ghost passes through characters while intangible, and the action has no intangible span: give one, or leave ghost out");
        }
    }

    /// Charge, turn spans, assist, miss recovery, `while_held` and the cooldown key and share.
    fn timeline(&mut self, id: &str) {
        let def = self.def;
        if let Some(charge) = &def.charge {
            if !(charge.after >= 0.0 && charge.after < self.length - EPS) {
                self.refs.error(format!(
                    "charge.after {} must lie in the action [0, {}): it is when the held input turns it",
                    charge.after,
                    meters(self.length)
                ));
            }
            match &charge.until {
                Some(until) => {
                    self.time("charge.until", *until);
                    if *until < charge.after - EPS {
                        self.refs.error(format!("charge.until {until} comes before charge.after {}", charge.after));
                    }
                }
                None if def.windup_end() < charge.after - EPS => self.refs.error(format!(
                    "charge.after {} is past the end of the windup ({}), where a charge stops turning by default; \
                     give charge.until",
                    charge.after,
                    meters(def.windup_end())
                )),
                None => {}
            }
            if charge.into == id {
                self.refs.error("charge.into is the action itself: it would turn into itself on every tick");
            } else if self.refs.require::<ActionDef>(&charge.into, "charge.into") {
                // a -> b -> a would turn back and forth on every tick it is held.
                let mut chain = vec![id.to_owned()];
                let mut next = Some(charge.into.clone());
                while let Some(at) = next {
                    if chain.contains(&at) {
                        if at == id {
                            chain.push(at);
                            self.refs.error(format!(
                                "charge turns in a circle ({}): a held input would turn it on every tick",
                                chain.join(" -> ")
                            ));
                        }
                        break;
                    }
                    next = self.refs.get::<ActionDef>(&at).and_then(|d| d.charge.as_ref()).map(|c| c.into.clone());
                    chain.push(at);
                }
            }
        }
        for (i, (span, rate)) in def.turn.iter().enumerate() {
            self.span(&format!("turn[{i}]"), span);
            self.number(&format!("turn[{i}] rate"), *rate);
        }
        if !def.turn.is_empty() && def.track_until > 0.0 {
            self.refs.error("turn and track_until both say how the action turns; give one (turn spans, or track_until)");
        }
        if !def.turn.is_empty() && def.steer == Steer::MoveOrBack {
            self.refs.error(
                "a `steer: MoveOrBack` action with turn spans starts from the body's facing, so a standing backstep would go \
                 forward; leave the turn spans out (or use steer: Move)",
            );
        }
        if let Some(assist) = &def.assist {
            if !(assist.angle > 0.0 && assist.angle <= 180.0) {
                self.refs.error(format!("assist.angle must be in (0, 180] degrees (is {})", assist.angle));
            }
            if let Some(close) = assist.close {
                self.number("assist.close", close);
            }
            if def.turn.is_empty() && def.track_until <= 0.0 {
                self.refs.error("assist limits how the action tracks its target, and it does not track: give it turn spans or track_until");
            } else if matches!(def.steer, Steer::Move | Steer::MoveOrBack) {
                self.refs.error(format!(
                    "assist limits tracking a lock-on target, which a `steer: {:?}` action never does; leave assist out",
                    def.steer
                ));
            }
        }
        self.number("miss_recovery", def.miss_recovery);
        if def.miss_recovery > 0.0 && def.hits.is_empty() {
            self.refs.error("miss_recovery needs hit windows: it is added when none of them landed");
        }
        if let Some(input) = &def.while_held
            && (input.is_empty() || input.contains(char::is_whitespace))
        {
            self.refs.error(format!("while_held `{input}` must be an input name without spaces, e.g. `interact`"));
        }
        if def.cost.cooldown_key.as_deref() == Some("") {
            self.refs.error("cost.cooldown_key is empty; name the shared slot, or leave it out for the action's own");
        }
        if let Some(share) = def.cost.cooldown_if_interrupted {
            if !(0.0..=1.0).contains(&share) {
                self.refs.error(format!(
                    "cost.cooldown_if_interrupted must be between 0 and 1 (is {share}): the share of the cooldown that stays"
                ));
            } else if def.cost.cooldown <= 0.0 {
                self.refs.error("cost.cooldown_if_interrupted is a share of cost.cooldown, which is 0: give the cooldown, or leave the share out");
            }
        }
        self.body_and_motion();
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
    /// Forced when the fighter goes down (a lethal hook's `Fate::Down`): its downed pose, an ordinary
    /// action: make it at least as long as the bleed-out time (a body down longer than its pose
    /// plays idle while it still ignores hits). `None`: its action is cut and it stands still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downed: Option<String>,
    /// Reaction actions by tier: light (1), heavy (2), knockdown (3). Empty: the old flinch (`hit`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<ReactionDef>,
    /// Tiers at or below this never cause a reaction (a heavy enemy: 1).
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub reaction_armor: u8,
    /// Seconds after a reaction starts in which tiers 1-2 cause no new one.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reaction_immunity: f32,
    /// Seconds of the same immunity granted when a reaction or a stagger ends.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reaction_protection: f32,
    /// Seconds poise damage is ignored after a poise break ends.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub break_protection: f32,
    /// Forced by a guard break; `None`: `stagger` (the old rule).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_broken: Option<String>,
    /// A guard break pushes the body: `(scale, decel)` starts a push at `scale` times the body's
    /// current flat velocity, slowing by `decel` m/s per s. `None`: no push.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_broken_push: Option<(f32, f32)>,
    /// How presses wait for their action ([`BufferMode`]); default: each input its own.
    #[serde(default, skip_serializing_if = "BufferMode::is_per_input")]
    pub buffer: BufferMode,
    /// While the body is in a motion mode named here, only actions with one of these tags start, and
    /// a running action without one ends when the mode is entered: `[(Water, ["afloat"])]`. A list,
    /// not a map: `MotionMode` has no `Ord`. Forced actions (a stagger, a death) are never cut.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_in: Vec<(MotionMode, Vec<String>)>,
    /// Statuses active at once on this fighter; `None`: `CombatTuning::max_statuses`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_statuses: Option<u32>,
    /// What the fighter remembers, and for how long ([`MemoryDef`]): a parry, the end of a dodge.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remember: Vec<MemoryDef>,
    /// Inputs that start another action while their conditions hold ([`BindingDef`]): a light
    /// attack that is a finisher next to a broken enemy, a counter after a parry. For each input
    /// the first binding whose conditions all hold wins, over `inputs` and over every cancel
    /// window's transition, a `Combo` one included; an input none of whose bindings holds goes on
    /// as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<BindingDef>,
    /// The action that starts instead when an action of this moveset cannot pay its `cost.charge` and
    /// the reload can help: an empty magazine reloads on the press that would have fired
    /// (`Some("pistol_reload")`, an action with a `Refill`). It starts only when it would move
    /// something (room in the charge, a charge in the reserve); otherwise the press is refused as
    /// usual (`pistol empty: no pistol.reserve left`). `None`: the press is refused and the player reloads by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_reload: Option<String>,
    /// What this moveset is as a weapon: the charges it brings, which one is the magazine and which
    /// the reserve ([`WeaponDef`]). `None`: not a weapon (or one the game wires by hand).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<WeaponDef>,
}

/// A moveset as a weapon ([`MovesetDef::weapon`]): what its `Arsenal` slot brings and shows.
///
/// ```ron
/// "weapon": (
///     name: "@weapon-sidearm",
///     magazine: "sidearm",
///     reserve: "sidearm.reserve",
///     charges: {"sidearm": (max: 6), "sidearm.reserve": (max: 36, start: 12)},
/// )
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct WeaponDef {
    /// What the player reads for the weapon (`you.weapon` in a HUD): a Fluent `@id` or plain text.
    /// `None`: the moveset's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The charge that holds the rounds in the weapon (what `cost.charge` of its fire takes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magazine: Option<String>,
    /// The charge that holds the rest of its ammo (what a `Refill` draws from).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserve: Option<String>,
    /// The charges a body gets (those it lacks) when the weapon is granted or selected, by name. Every
    /// charge the moveset's actions take or refill is declared here or in a prefab's `Charges`; two
    /// weapons sharing a reserve declare it alike.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub charges: BTreeMap<String, Stock>,
}

/// One charge a weapon brings ([`WeaponDef::charges`]).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Stock {
    /// The most the charge holds.
    pub max: u32,
    /// What it holds when the body gets it; `None`: full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u32>,
}

impl Stock {
    /// What a body that gets the charge starts with: `start` (at most `max`), else full.
    pub fn initial(&self) -> u32 {
        self.start.map_or(self.max, |s| s.min(self.max))
    }
}

/// Something a fighter remembers for `seconds` ([`MovesetDef::remember`]): when `on` happens it
/// remembers `key` (again: the time starts over); when one of `forget` happens it forgets it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MemoryDef {
    pub key: String,
    pub on: Recall,
    pub seconds: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forget: Vec<Forget>,
}

/// What makes a fighter remember ([`MemoryDef::on`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Recall {
    /// It parried a hit (a perfect guard).
    Parried,
    /// It evaded a hit inside a perfect-evade window.
    PerfectEvaded,
    /// It blocked a hit.
    Blocked,
    /// It evaded a hit (i-frames).
    Evaded,
    /// One of its hits landed (a `Damaged` it caused that was not ignored).
    Landed,
    /// It took damage (> 0).
    Hurt,
    /// An action of its with this tag ended (any end but a charge's turn).
    Ended(String),
    /// An action of its with this tag started.
    Started(String),
}

/// What makes a fighter forget ([`MemoryDef::forget`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Forget {
    /// It took damage (> 0).
    Hurt,
    /// A reaction, a stagger or a guard break started on it.
    Reacted,
    /// An action of its with this tag started.
    Started(String),
    /// A binding that needs the memory (`When::Remembers`) started its action.
    Used,
}

/// One context binding ([`MovesetDef::context`]): `input` starts `action` while every one of
/// `when` holds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BindingDef {
    pub input: String,
    pub action: String,
    /// All must hold. A binding never holds for the action the fighter runs now (a running
    /// finisher must not restart itself through a cancel window).
    pub when: Vec<When>,
}

/// A condition of a binding ([`BindingDef::when`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum When {
    /// A memory with this key is live.
    Remembers(String),
    /// The binding's action is paired and its `pair.select` finds a victim now.
    Pair,
    /// Stamina above this (a fighter without stamina always has enough).
    StaminaAbove(f32),
    /// A game condition registered with `add_context_condition`.
    Condition(String),
}

/// How a fighter's presses wait in its [`InputBuffer`](crate::InputBuffer).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum BufferMode {
    /// Every input keeps its newest press, which waits `input_buffer` seconds (or for its
    /// `queue` window) and is tried again on every tick until it starts or runs out (the old rule).
    #[default]
    PerInput,
    /// One press at a time. `priority` ranks inputs (`[("dodge", 60), ("technique.1", 55),
    /// ("technique.2", 55), ("light", 30)]`; inputs not listed rank 0): a press that ranks below
    /// the one waiting is dropped (rejected); an equal or higher one takes the slot; a press of the
    /// waiting input again keeps the later of the two expiries. The press in the slot is tried
    /// again on every tick until it starts or its time runs out, as in `PerInput`.
    SingleSlot {
        #[serde(default)]
        priority: Vec<(String, i32)>,
    },
}

impl BufferMode {
    fn is_per_input(&self) -> bool {
        *self == BufferMode::PerInput
    }

    /// How `input` ranks in a single slot (0 when it is not listed, and always 0 per input).
    pub fn rank(&self, input: &str) -> i32 {
        match self {
            BufferMode::PerInput => 0,
            BufferMode::SingleSlot { priority } => priority.iter().find(|(i, _)| i == input).map_or(0, |(_, r)| *r),
        }
    }
}

/// One reaction tier: the forced action (its length is the reaction's duration) and its push.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReactionDef {
    pub action: String,
    /// Push speed at the start (m/s) along the hit's direction.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub push: f32,
    /// How fast the push slows (m/s per s). Above 0 for every reaction: a hit's `knockback`
    /// lives in another definition, so `push: 0, decel: 0` met by `knockback: 3` would never stop.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub decel: f32,
}

impl MovesetDef {
    /// The action `input` starts.
    pub fn action(&self, input: &str) -> Option<&str> {
        self.inputs.get(input).map(String::as_str)
    }

    /// Whether `only_in` names `mode` at all (then it refuses some actions there): asked before an action's definition is read.
    pub fn gates(&self, mode: MotionMode) -> bool {
        self.only_in.iter().any(|(m, _)| *m == mode)
    }

    /// Whether `def` may run while the body is in motion mode `mode`: no `only_in` names the mode,
    /// or `def` has one of its tags.
    pub fn allows(&self, mode: MotionMode, def: &ActionDef) -> bool {
        self.only_in.iter().filter(|(m, _)| *m == mode).all(|(_, tags)| def.tags.iter().any(|t| tags.contains(t)))
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
        for (slot, action) in
            [("stagger", &self.stagger), ("hit", &self.hit), ("death", &self.death), ("downed", &self.downed), ("guard_broken", &self.guard_broken)]
        {
            if let Some(action) = action {
                refs.require::<ActionDef>(action, slot);
            }
        }
        if !(self.reactions.is_empty() || self.reactions.len() == 3) {
            refs.error(format!(
                "reactions has {} entries; give exactly three (light, heavy, knockdown) or none for the old flinch",
                self.reactions.len()
            ));
        }
        for (i, reaction) in self.reactions.iter().enumerate() {
            let at = format!("reactions[{i}]");
            refs.require::<ActionDef>(&reaction.action, &at);
            for (name, v) in [("push", reaction.push), ("decel", reaction.decel)] {
                if !(v >= 0.0 && v.is_finite()) {
                    refs.error(format!("{at}.{name} must be a number >= 0 (is {v})"));
                }
            }
            if reaction.decel <= 0.0 {
                refs.error(format!(
                    "{at}.decel must be above 0 (is {}): any hit's `knockback` may start a push, and a push with no deceleration never stops",
                    reaction.decel
                ));
            }
        }
        for (name, v) in [
            ("reaction_immunity", self.reaction_immunity),
            ("reaction_protection", self.reaction_protection),
            ("break_protection", self.break_protection),
        ] {
            if !(v >= 0.0 && v.is_finite()) {
                refs.error(format!("{name} must be a number >= 0 (is {v})"));
            }
        }
        if self.reaction_armor > 3 {
            refs.error(format!("reaction_armor must be 0 to 3 (is {}): a tier at most this causes no reaction", self.reaction_armor));
        }
        if let Some((scale, decel)) = self.guard_broken_push
            && !(scale >= 0.0 && scale.is_finite() && decel > 0.0 && decel.is_finite())
        {
            refs.error(format!(
                "guard_broken_push ({scale}, {decel}) needs a scale >= 0 and a deceleration above 0, or the push never stops"
            ));
        }
        for (i, (mode, tags)) in self.only_in.iter().enumerate() {
            if self.only_in[..i].iter().any(|(other, _)| other == mode) {
                refs.error(format!("only_in names {mode:?} twice; give each mode one list of tags"));
            }
            if tags.iter().any(|t| t.is_empty() || t.contains(char::is_whitespace)) {
                refs.error(format!("only_in {mode:?} has a tag that is empty or has spaces; tags are names like `afloat`"));
            }
            // A tag no action of the moveset carries (a typo) would leave the body unable to act in that mode.
            let carried: Vec<&ActionDef> = self.inputs.values().filter_map(|a| refs.get::<ActionDef>(a)).collect();
            if carried.len() == self.inputs.len() {
                for tag in tags.iter().filter(|t| !carried.iter().any(|d| d.tags.contains(t))) {
                    refs.error(format!(
                        "only_in {mode:?} names the tag `{tag}`, which no action among the inputs carries: the body could not act in that mode (a typo?)"
                    ));
                }
            }
        }
        if let BufferMode::SingleSlot { priority } = &self.buffer {
            for (i, (input, _)) in priority.iter().enumerate() {
                if input.is_empty() || input.contains(char::is_whitespace) {
                    refs.error(format!("buffer priority input `{input}` must be a name without spaces, e.g. `dodge`"));
                } else if priority[..i].iter().any(|(other, _)| other == input) {
                    refs.error(format!("buffer priority lists `{input}` twice; give each input one rank"));
                }
            }
        }
        self.validate_context(refs);
        self.validate_weapon(refs);
    }
}

/// Every charge name the prefabs' `Charges` components give a body (a healing flask's, say).
fn prefab_charge_names(refs: &Refs) -> BTreeSet<String> {
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }
    #[derive(Deserialize)]
    struct Listed(Vec<Named>);
    refs.iter::<shards_content::PrefabDef>()
        .flat_map(|(_, prefab)| {
            prefab
                .components
                .iter()
                .filter(|(name, _)| name.as_str() == "Charges" || name.ends_with("::Charges"))
                .filter_map(|(_, value)| ron::from_str::<Listed>(value.get_ron()).ok())
                .flat_map(|listed| listed.0.into_iter().map(|n| n.name))
        })
        .collect()
}

impl MovesetDef {
    /// `auto_reload` and `weapon`: the reload exists and refills; the charges the weapon's actions take
    /// and refill are declared (here, or in a prefab's `Charges`); the magazine and the reserve are
    /// among them; two weapons that declare one charge declare it alike.
    fn validate_weapon(&self, refs: &mut Refs) {
        if let Some(reload) = &self.auto_reload
            && refs.require::<ActionDef>(reload, "auto_reload")
            && refs.get::<ActionDef>(reload).is_some_and(|d| d.refills().next().is_none())
        {
            refs.error(format!(
                "auto_reload -> action `{reload}` has no Refill effect: an empty magazine would start something that reloads nothing"
            ));
        }
        let Some(weapon) = &self.weapon else { return };
        for (name, stock) in &weapon.charges {
            if name.is_empty() || name.chars().any(char::is_control) {
                refs.error(format!("weapon.charges has the name `{name}`; a charge is named by a short text without control characters"));
            }
            if stock.max == 0 {
                refs.error(format!("weapon.charges `{name}`: max must be at least 1"));
            }
            if let Some(start) = stock.start
                && start > stock.max
            {
                refs.error(format!("weapon.charges `{name}`: start {start} is above max {}", stock.max));
            }
        }
        // Two weapons that share a charge (a common reserve) have one place for it in a body: one max.
        for (other, moveset) in refs.iter::<MovesetDef>() {
            let Some(theirs) = &moveset.weapon else { continue };
            for (name, stock) in &weapon.charges {
                if let Some(there) = theirs.charges.get(name)
                    && there.max != stock.max
                {
                    refs.error(format!(
                        "weapon.charges `{name}`: max {} here, {} in moveset `{other}`; a body has one `{name}`, so weapons that share it declare the same max",
                        stock.max, there.max
                    ));
                }
            }
        }
        let known: BTreeSet<String> = weapon.charges.keys().cloned().chain(prefab_charge_names(refs)).collect();
        let names = |known: &BTreeSet<String>| known.iter().cloned().collect::<Vec<_>>().join(", ");
        for (field, charge) in [("weapon.magazine", &weapon.magazine), ("weapon.reserve", &weapon.reserve)] {
            if let Some(charge) = charge
                && !known.contains(charge)
            {
                refs.error(format!("{field} -> charge `{charge}` is declared nowhere: not in weapon.charges, not in a prefab's Charges ({})", names(&known)));
            }
        }
        let actions = self.inputs.iter().map(|(input, action)| (format!("inputs.{input}"), action)).chain(self.auto_reload.iter().map(|a| ("auto_reload".to_owned(), a)));
        for (field, id) in actions {
            let Some(def) = refs.get::<ActionDef>(id) else { continue };
            if let Some(charge) = &def.cost.charge
                && !known.contains(charge)
            {
                refs.error(format!(
                    "{field} -> action `{id}` costs the charge `{charge}`, which this weapon does not have: declare it in weapon.charges, or in a prefab's Charges ({})",
                    names(&known)
                ));
            }
            for (charge, from, _) in def.refills() {
                for name in [charge, from] {
                    if !known.contains(name) {
                        refs.error(format!(
                            "{field} -> action `{id}` refills with the charge `{name}`, which this weapon does not have: declare it in weapon.charges, or in a prefab's Charges ({})",
                            names(&known)
                        ));
                    }
                }
            }
        }
    }

    /// `remember` and `context`: keys unique, times above 0, bindings naming actions and keys that exist.
    fn validate_context(&self, refs: &mut Refs) {
        for (i, memory) in self.remember.iter().enumerate() {
            let at = format!("remember[{i}]");
            if memory.key.is_empty() {
                refs.error(format!("{at}.key is empty; name the memory, e.g. `parried`"));
            } else if self.remember[..i].iter().any(|m| m.key == memory.key) {
                refs.error(format!("{at}: key `{}` is remembered twice; give each memory one entry", memory.key));
            }
            if !(memory.seconds > 0.0 && memory.seconds.is_finite()) {
                refs.error(format!("{at}.seconds must be above 0 (is {}): a memory that lasts no time is never seen", memory.seconds));
            }
            for tag in memory.forget.iter().filter_map(|f| if let Forget::Started(t) = f { Some(t) } else { None }).chain(match &memory.on {
                Recall::Ended(t) | Recall::Started(t) => Some(t),
                _ => None,
            }) {
                if tag.is_empty() {
                    refs.error(format!("{at} names an empty tag; tags are the actions' `tags`, e.g. `dodge`"));
                } else if !refs.iter::<ActionDef>().any(|(_, a)| a.tags.iter().any(|t| t == tag)) {
                    // A tag no action carries never fires: a typo, caught here instead of in play.
                    refs.error(format!("{at} names the tag `{tag}`, which no action carries; tags are the actions' `tags`, e.g. `dodge`"));
                }
            }
        }
        for (i, binding) in self.context.iter().enumerate() {
            let at = format!("context[{i}]");
            if binding.input.is_empty() || binding.input.contains(char::is_whitespace) {
                refs.error(format!("{at}.input `{}` must be an input name without spaces, e.g. `light`", binding.input));
            }
            let action = refs.require::<ActionDef>(&binding.action, &format!("{at}.action"));
            let paired = refs.get::<ActionDef>(&binding.action).is_some_and(|d| d.pair.is_some());
            for when in &binding.when {
                match when {
                    When::Pair if action && !paired => refs.error(format!(
                        "{at}: `Pair` needs a paired action, and `{}` has no `pair`",
                        binding.action
                    )),
                    When::Remembers(key) if !self.remember.iter().any(|m| m.key == *key) => {
                        let mut keys: Vec<&str> = self.remember.iter().map(|m| m.key.as_str()).filter(|k| !k.is_empty()).collect();
                        keys.sort_unstable();
                        keys.dedup();
                        refs.error(format!(
                            "{at}: `Remembers(\"{key}\")` names no memory of this moveset; it remembers: {}",
                            if keys.is_empty() { "nothing".to_owned() } else { keys.join(", ") }
                        ))
                    }
                    When::StaminaAbove(v) if !v.is_finite() => refs.error(format!("{at}: StaminaAbove({v}) must be a number")),
                    When::Condition(name) if name.is_empty() => refs.error(format!("{at}: Condition names nothing")),
                    _ => {}
                }
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
    /// A sprint that costs nothing (the body's `MoveModifiers::sprint_cost` is 0, or a fast swim with
    /// no drain) holds the stamina regeneration back this many seconds past the last tick of the
    /// sprint, instead of the full `Stamina::delay` a paid spend sets; never shorter than the wait
    /// that is already running. `None` (the default): a free sprint restarts the whole delay.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_sprint_regen_hold: Option<f32>,
    /// How far `lock_on` looks for a target (m).
    pub lock_on_range: f32,
    /// Share of a body's height where a `LineOfSight` hit's ray starts and ends (a chest).
    #[serde(skip_serializing_if = "is_default_sight_height")]
    pub sight_height: f32,
    /// Combat objects (projectiles, zones, summons) alive at once in the world: a spawn past it
    /// lays down the indices that fit and says so in the journal.
    #[serde(skip_serializing_if = "is_default_max_objects")]
    pub max_objects: u32,
    /// Combat objects alive at once of one owner.
    #[serde(skip_serializing_if = "is_default_max_objects_per_owner")]
    pub max_objects_per_owner: u32,
}

fn is_on_release(v: &SprintResume) -> bool {
    *v == SprintResume::OnRelease
}

/// The chest height `LineOfSight` rays use unless the tuning says otherwise.
const SIGHT_HEIGHT: f32 = 0.6;

fn is_default_sight_height(v: &f32) -> bool {
    *v == SIGHT_HEIGHT
}

/// The combat objects alive at once unless the tuning says otherwise: in the world, of one owner.
const MAX_OBJECTS: u32 = 256;
const MAX_OBJECTS_PER_OWNER: u32 = 96;

fn is_default_max_objects(v: &u32) -> bool {
    *v == MAX_OBJECTS
}

fn is_default_max_objects_per_owner(v: &u32) -> bool {
    *v == MAX_OBJECTS_PER_OWNER
}

impl Default for CombatTuning {
    fn default() -> Self {
        Self {
            input_buffer: 0.15,
            max_statuses: 3,
            sprint_stamina_per_s: 10.0,
            sprint_resume: SprintResume::OnRelease,
            free_sprint_regen_hold: None,
            lock_on_range: 20.0,
            sight_height: SIGHT_HEIGHT,
            max_objects: MAX_OBJECTS,
            max_objects_per_owner: MAX_OBJECTS_PER_OWNER,
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
            ("sight_height", self.sight_height),
        ] {
            if !(v >= 0.0 && v.is_finite()) {
                refs.error(format!("{name} must be a number >= 0 (is {v})"));
            }
        }
        if let Some(h) = self.free_sprint_regen_hold
            && !(h >= 0.0 && h.is_finite())
        {
            refs.error(format!("free_sprint_regen_hold must be a number >= 0 (is {h})"));
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
    fn a_memory_tag_no_action_carries_is_an_error() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<StatusDef>();
        let actions = r#"{
            "roll": (net_id: 1, phases: [("roll", 0.3)], tags: ["dodge"]),
            "stagger": (net_id: 2, phases: [("stagger", 0.5)]),
        }"#;
        let movesets = r#"{ "m": (inputs: {"dodge": "roll"}, stagger: "stagger", remember: [
            (key: "dodged", on: Ended("dodge"), seconds: 0.3),
            (key: "typo", on: Ended("dodgee"), seconds: 0.3, forget: [Started("rol")]),
        ]) }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        assert_eq!(errors.len(), 2, "{errors:#?}");
        assert!(errors.iter().any(|e| e.contains("remember[1]") && e.contains("`dodgee`") && e.contains("no action carries")), "{errors:#?}");
        assert!(errors.iter().any(|e| e.contains("`rol`")), "{errors:#?}");
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

    #[test]
    fn armor_at_takes_the_highest_open_level() {
        let d = def(
            r#"(net_id: 1, phases: [("windup", 0.4), ("active", 0.2), ("recovery", 0.4)],
                armor: [(Phase("windup"), 1), (Secs(0.3, 0.7), 3), (Phase("recovery"), 2)])"#,
        );
        assert_eq!(d.armor_at(0.1), 1);
        assert_eq!(d.armor_at(0.31), 3, "the higher of the two open at once");
        assert_eq!(d.armor_at(0.5), 3);
        assert_eq!(d.armor_at(0.7), 3, "the end instant is inside the window");
        assert_eq!(d.armor_at(0.7167), 2, "a tick past a window that ends between ticks is outside it");
        assert_eq!(d.armor_at(0.0), 1, "open at the very start");
        assert_eq!(light().armor_at(0.5), 0, "no armour windows: 0");
        assert!(d.any_open_at(&[Span::Secs(0.105, 0.215)], 0.2));
        assert!(!d.any_open_at(&[Span::Secs(0.105, 0.215)], 13.0 / 60.0), "tick 13 is past 0.215");
    }

    #[test]
    fn validation_names_bad_tiers_and_perfect_windows() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        let actions = r#"{
            "a": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1)],
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5, reaction: 4, knockback: -1, guard_damage: -2)],
                armor: [(Phase("windup"), 4), (Phase("nope"), 1)],
                iframes: [Secs(0.0, 0.1)], perfect_iframes: [Secs(0.05, 0.2)],
                guard: (span: Phase("windup"), perfect_cost_mult: 1.5, status_mult: -1)),
            "ok": (net_id: 2, phases: [("roll", 0.4)], cost: (stamina: 20, stamina_mode: Any),
                iframes: [Secs(0.0, 0.1), Secs(0.1, 0.2)], perfect_iframes: [Secs(0.05, 0.15)]),
            "r": (net_id: 3, phases: [("react", 0.3)]),
        }"#;
        let movesets = r#"{
            "few": (inputs: {}, reactions: [(action: "r", push: 1, decel: 5)]),
            "stuck": (inputs: {}, reactions: [(action: "r", push: 1, decel: 5), (action: "r", decel: 0), (action: "x", decel: 5)],
                reaction_armor: 4, reaction_immunity: -1, guard_broken: "y", guard_broken_push: (0.2, 0.0)),
        }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let has = |part: &str| errors.iter().any(|e| e.contains(part));
        assert!(has("hits[0].reaction must be a tier 0 to 3 (is 4)"), "{errors:#?}");
        assert!(has("hits[0].knockback must be a number >= 0") && has("hits[0].guard_damage must be a number >= 0"));
        assert!(has("armor[0] level must be 0 to 3 (is 4)") && has("armor[1] names an unknown phase `nope`"));
        assert!(has("perfect_iframes[0] 0.05..0.2 lies outside the i-frames: a perfect-evade window outside the i-frames never opens"));
        assert!(!has("action `ok`"), "touching i-frames form one window: {errors:#?}");
        assert!(has("guard.perfect_cost_mult must be between 0 and 1 (is 1.5)"), "above 1 it costs more than the block");
        assert!(has("guard.status_mult must be a number >= 0"));
        assert!(has("moveset `few`: reactions has 1 entries; give exactly three"));
        assert!(has("reactions[1].decel must be above 0 (is 0)"), "a push that never stops: {errors:#?}");
        assert!(has("reactions[2] -> unknown action `x`") && has("guard_broken -> unknown action `y`"));
        assert!(has("reaction_armor must be 0 to 3 (is 4)") && has("reaction_immunity must be a number >= 0"));
        assert!(has("guard_broken_push (0.2, 0) needs a scale >= 0 and a deceleration above 0"));
    }

    #[test]
    fn zero_decel_reaction_fails_validation() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        let sources = [
            shards_content::db::Source {
                path: "content/a.action.ron".into(),
                text: r#"{ "r": (net_id: 1, phases: [("react", 0.3)]) }"#.into(),
            },
            shards_content::db::Source {
                path: "content/m.moveset.ron".into(),
                text: r#"{ "m": (inputs: {}, reactions: [(action: "r", push: 0, decel: 0), (action: "r", decel: 1), (action: "r", decel: 1)]) }"#
                    .into(),
            },
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].contains("reactions[0].decel must be above 0 (is 0)"), "{}", errors[0]);
    }

    #[test]
    fn perfect_cost_mult_above_one_fails_validation() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        let sources = [shards_content::db::Source {
            path: "content/a.action.ron".into(),
            text: r#"{
                "g": (net_id: 1, phases: [("g", 0.3)], guard: (span: Phase("g"), perfect: Phase("g"), perfect_cost_mult: 1.01)),
                "h": (net_id: 2, phases: [("g", 0.3)], guard: (span: Phase("g"), perfect: Phase("g"), perfect_cost_mult: 1.0)),
            }"#
            .into(),
        }];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert!(errors[0].contains("action `g`: guard.perfect_cost_mult must be between 0 and 1 (is 1.01)"), "{}", errors[0]);
    }

    #[test]
    fn validation_names_bad_timelines() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        let actions = r#"{
            "a": (net_id: 1, phases: [("windup", 0.4), ("active", 0.1)],
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)],
                charge: (after: 0.2, into: "b"), turn: [(Phase("nope"), -5)], track_until: 0.2, assist: (angle: 200, close: -1),
                cost: (cooldown: 1, cooldown_key: "", cooldown_if_interrupted: 1.5)),
            "b": (net_id: 2, phases: [("windup", 0.4), ("active", 0.1)],
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)], charge: (after: 0.1, into: "a")),
            "c": (net_id: 3, phases: [("windup", 0.3)], charge: (after: 0.5, into: "c"), miss_recovery: 0.5,
                while_held: "two words"),
            "d": (net_id: 4, phases: [("windup", 0.3), ("active", 0.1)],
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)], charge: (after: 0.35, into: "x")),
            "e": (net_id: 5, phases: [("windup", 0.3), ("active", 0.1)], charge: (after: 0.2, into: "a", until: 0.1),
                cancels: [(span: Secs(0.4, 0.4), into: [Input("light")], queue: Phase("gone"))]),
            "f": (net_id: 7, phases: [("windup", 0.3)], assist: (angle: 20)),
            "g": (net_id: 8, phases: [("roll", 0.3)], steer: Move, track_until: 0.2, assist: (angle: 20)),
            "i": (net_id: 10, phases: [("roll", 0.3)], steer: MoveOrBack, track_until: 0.2, assist: (angle: 20)),
            "j": (net_id: 11, phases: [("roll", 0.3)], steer: MoveOrBack, turn: [(Phase("roll"), 90)]),
            "h": (net_id: 9, phases: [("windup", 0.3)], cost: (stamina: 5, cooldown_if_interrupted: 0.4)),
            "ok": (net_id: 6, phases: [("windup", 0.3), ("active", 0.1)],
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)], charge: (after: 0.35, into: "a", until: 0.38),
                turn: [(Phase("windup"), 90)], assist: (angle: 30, close: 0.4, sight: true), miss_recovery: 0.4,
                while_held: "heavy", face: Keep,
                cancels: [(span: Secs(0.4, 0.4), into: [Input("light")], queue: Phase("windup"))]),
        }"#;
        let movesets = r#"{
            "slot": (inputs: {}, buffer: SingleSlot(priority: [("dodge", 60), ("dodge", 50), ("two words", 1)])),
            "fine": (inputs: {}, buffer: SingleSlot(priority: [("technique.1", 55), ("technique.2", 55)])),
        }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let has = |part: &str| errors.iter().any(|e| e.contains(part));
        assert!(has("action `a`: charge turns in a circle (a -> b -> a)"), "{errors:#?}");
        assert!(has("turn[0] names an unknown phase `nope`") && has("turn[0] rate must be a number >= 0"), "{errors:#?}");
        assert!(has("turn and track_until both say how the action turns"));
        assert!(has("assist.angle must be in (0, 180] degrees (is 200)") && has("assist.close must be a number >= 0"), "{errors:#?}");
        assert!(has("action `f`: assist limits how the action tracks its target, and it does not track"), "{errors:#?}");
        assert!(has("action `g`: assist limits tracking a lock-on target, which a `steer: Move` action never does"), "{errors:#?}");
        assert!(has("action `i`: assist limits tracking a lock-on target, which a `steer: MoveOrBack` action never does"), "{errors:#?}");
        assert!(
            has("action `j`: a `steer: MoveOrBack` action with turn spans starts from the body's facing, so a standing backstep would go forward"),
            "{errors:#?}"
        );        assert!(has("action `h`: cost.cooldown_if_interrupted is a share of cost.cooldown, which is 0"), "{errors:#?}");
        assert!(has("cost.cooldown_key is empty") && has("cost.cooldown_if_interrupted must be between 0 and 1 (is 1.5)"));
        assert!(has("action `c`: charge.after 0.5 must lie in the action [0, 0.3)"), "{errors:#?}");
        assert!(has("charge.into is the action itself") && has("miss_recovery needs hit windows"));
        assert!(has("while_held `two words` must be an input name without spaces"));
        assert!(has("action `d`: charge.after 0.35 is past the end of the windup (0.3)"), "{errors:#?}");
        assert!(has("charge.into -> unknown action `x`"), "{errors:#?}");
        assert!(has("action `e`: charge.until 0.1 comes before charge.after 0.2"), "{errors:#?}");
        assert!(has("cancels[0].queue names an unknown phase `gone`"), "{errors:#?}");
        assert!(!has("action `ok`"), "{errors:#?}");
        assert!(has("moveset `slot`: buffer priority lists `dodge` twice"), "{errors:#?}");
        assert!(has("buffer priority input `two words` must be a name without spaces"));
        assert!(!has("moveset `fine`"), "equal ranks are fine: {errors:#?}");
    }

    #[test]
    fn validation_names_bad_body_fields() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        let actions = r#"{
            "a": (net_id: 1, phases: [("crouch", 0.2), ("air", 0.5)],
                leap: (height: 0, at: 0.7, search: (min: 5, max: 2, step: 0, max_rise: -1, max_drop: 1, probe_height: 0, max_water_depth: -2))),
            "b": (net_id: 2, phases: [("crouch", 0.2), ("air", 0.5)], displacement: [(0.7, 4.0)], leap: (height: 1, at: 0.1, search: (min: 1, max: 4, step: 0.5, max_rise: 0.5, max_drop: 1, probe_height: 1, lift: 0.05, toward_target: true, max_water_depth: 1))),
            "c": (net_id: 3, phases: [("crouch", 0.2), ("air", 0.5)], leap: (height: 1, at: 0.1, search: (min: 1, max: 4, step: 0.5, max_rise: 0.5, max_drop: 1, probe_height: 1))),
            "d": (net_id: 4, phases: [("spin", 1.0)], walk: [(Phase("nope"), -0.5)], move_speed: 0.5),
            "e": (net_id: 5, phases: [("under", 1.0)], ghost: true),
            "f": (net_id: 6, phases: [("dig", 0.3), ("under", 0.6)], intangible: [Phase("under")], ghost: true, walk: [(Phase("dig"), 0.5)]),
            "g": (net_id: 7, phases: [("windup", 0.3), ("loop", 0.5)], hold: (input: "block", span: Phase("loop")), leap: (height: 1, at: 0.1)),
            "h": (net_id: 8, phases: [("crouch", 0.2), ("air", 0.5)], displacement: [(0.7, 4.0)], turn: [(Phase("crouch"), 90)], leap: (height: 1, at: 0.1, search: (min: 1, max: 4, step: 0.5, max_rise: 0.5, max_drop: 1, probe_height: 1))),
            "t": (net_id: 9, phases: [("a", 0.2)], tags: ["afloat", "glide"]),
        }"#;
        let movesets = r#"{
            "twice": (inputs: {}, only_in: [(Water, ["afloat"]), (Water, ["swim"])]),
            "blank": (inputs: {}, only_in: [(Water, ["two words"])]),
            "typo": (inputs: {"x": "t"}, only_in: [(Water, ["afloat", "afoat"])]),
            "fine": (inputs: {"x": "t"}, only_in: [(Water, ["afloat"]), (Glide, ["afloat", "glide"])]),
        }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let has = |part: &str| errors.iter().any(|e| e.contains(part));
        assert!(has("leap.height must be a number > 0 (is 0)"), "{errors:#?}");
        assert!(has("leap.at 0.7 must lie in the action [0, 0.7)"), "{errors:#?}");
        assert!(has("leap.search chooses a distance and scales `displacement` to it, which is empty or 0"), "{errors:#?}");
        assert!(has("leap.search.max_rise must be a number >= 0 (is -1)") && has("leap.search.step must be a number > 0 (is 0)"), "{errors:#?}");
        assert!(has("leap.search.probe_height must be a number > 0") && has("leap.search.min 5 is above max 2"), "{errors:#?}");
        assert!(has("leap.search.max_water_depth must be a number >= 0 (is -2)"), "{errors:#?}");
        assert!(has("action `c`: leap.search chooses a distance"), "{errors:#?}");
        assert!(!has("action `b`"), "{errors:#?}");
        assert!(has("walk[0] names an unknown phase `nope`") && has("walk[0] share must be a number >= 0"), "{errors:#?}");
        assert!(has("walk and move_speed both say how the body walks"), "{errors:#?}");
        assert!(has("action `e`: ghost passes through characters while intangible"), "{errors:#?}");
        assert!(!has("action `f`"), "{errors:#?}");
        assert!(has("action `g`: a leap and a hold loop in one action"), "{errors:#?}");
        assert!(has("action `h`: a leap with a search and `turn` spans or an `assist`"), "{errors:#?}");
        assert!(has("moveset `typo`: only_in Water names the tag `afoat`") && !has("moveset `typo`: only_in Water names the tag `afloat`"), "{errors:#?}");
        assert!(has("moveset `twice`: only_in names Water twice"), "{errors:#?}");
        assert!(has("moveset `blank`: only_in Water has a tag that is empty or has spaces"), "{errors:#?}");
        assert!(!has("moveset `fine`"), "{errors:#?}");
    }

    #[test]
    fn only_in_lets_the_tagged_actions_through_in_the_named_mode() {
        let moveset: MovesetDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(r#"(inputs: {}, only_in: [(Water, ["afloat"])])"#)
            .unwrap();
        let tagged = def(r#"(net_id: 1, phases: [("a", 0.2)], tags: ["afloat", "x"])"#);
        let plain = def(r#"(net_id: 2, phases: [("a", 0.2)])"#);
        assert!(moveset.allows(MotionMode::Water, &tagged));
        assert!(!moveset.allows(MotionMode::Water, &plain));
        assert!(moveset.allows(MotionMode::Ground, &plain), "other modes are not named");
    }

    #[test]
    fn timeline_fields_keep_old_json() {
        let d = def(
            r#"(net_id: 1, phases: [("windup", 0.2), ("active", 0.1)], cost: (stamina: 10, cooldown: 1),
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)],
                cancels: [(span: Phase("active"), into: [Input("dodge")])])"#,
        );
        let json = serde_json::to_string(&d).unwrap();
        for new in [
            "charge",
            "turn",
            "assist",
            "miss_recovery",
            "while_held",
            "queue",
            "cooldown_key",
            "cooldown_if_interrupted",
            "face",
            "leap",
            "walk",
            "intangible",
            "ghost",
            "only_in",
            "then",
            "impact",
            "max_targets",
            "repeat",
        ] {
            assert!(!json.contains(&format!("\"{new}\"")), "`{new}` leaked into {json}");
        }
        let moveset: MovesetDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(r#"(inputs: {"light": "a"}, buffer: PerInput)"#)
            .unwrap();
        assert_eq!(serde_json::to_string(&moveset).unwrap(), r#"{"inputs":{"light":"a"}}"#);
        let slot: MovesetDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(r#"(inputs: {}, buffer: SingleSlot(priority: [("dodge", 60)]))"#)
            .unwrap();
        assert_eq!(slot.buffer.rank("dodge"), 60);
        assert_eq!(slot.buffer.rank("light"), 0);
        assert!(serde_json::to_string(&slot).unwrap().contains("SingleSlot"));
    }

    #[test]
    fn new_fields_keep_old_json() {
        // Content written before the hit-outcome fields serializes without a trace of them: the content
        // half of every replay check hashes this JSON.
        let d = def(
            r#"(net_id: 1, phases: [("windup", 0.2), ("active", 0.1)], cost: (stamina: 10),
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)],
                guard: (span: Phase("windup"), perfect: Phase("windup")))"#,
        );
        let json = serde_json::to_string(&d).unwrap();
        for new in [
            "reaction",
            "knockback",
            "guard_damage",
            "perfect_cost_mult",
            "parry_staggers",
            "break_damage_mult",
            "status_mult",
            "break_status_mult",
            "stamina_mode",
            "armor",
            "perfect_iframes",
        ] {
            assert!(!json.contains(&format!("\"{new}\"")), "`{new}` leaked into {json}");
        }
        let moveset: MovesetDef = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(r#"(inputs: {"light": "a"}, stagger: "s")"#).unwrap();
        let json = serde_json::to_string(&moveset).unwrap();
        assert_eq!(json, r#"{"inputs":{"light":"a"},"stagger":"s"}"#);
        let tuning = serde_json::to_string(&CombatTuning::default()).unwrap();
        assert!(!tuning.contains("sight_height") && !tuning.contains("max_objects"), "{tuning}");
        // And a field set to something else does show.
        let set: MovesetDef = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(r#"(inputs: {}, reaction_immunity: 0.2)"#).unwrap();
        assert!(serde_json::to_string(&set).unwrap().contains("reaction_immunity"));
    }

    #[test]
    fn context_fields_keep_old_json() {
        // Content written before B3 serializes without a trace of its fields: the per-definition
        // replay checks hash this JSON.
        let d = def(
            r#"(net_id: 1, phases: [("windup", 0.2), ("active", 0.1)],
                hits: [(span: Phase("active"), shape: Sphere(radius: 1), damage: 5)],
                guard: (span: Phase("windup"), perfect: Phase("windup")))"#,
        );
        let json = serde_json::to_string(&d).unwrap();
        for new in ["weight", "pair", "perfect_needs_press"] {
            assert!(!json.contains(&format!("\"{new}\"")), "`{new}` leaked into {json}");
        }
        let moveset: MovesetDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(r#"(inputs: {"light": "a"})"#)
            .unwrap();
        let json = serde_json::to_string(&moveset).unwrap();
        assert!(!json.contains("remember") && !json.contains("context"), "{json}");
        // The press rule's defaults, and a pair's knockdown, stay out too.
        let guarded = def(
            r#"(net_id: 1, phases: [("raise", 0.2)], guard: (span: Phase("raise"), perfect: Phase("raise"), perfect_needs_press: ()),
                pair: (select: (state: Broken, range: 2, height: 1, angle: 90)))"#,
        );
        let press = guarded.guard.as_ref().and_then(|g| g.perfect_needs_press).unwrap();
        assert_eq!(press, GuardPress { gap: 0.35, entry: 0.15, rearm: true });
        let pair = guarded.pair.as_ref().unwrap();
        assert_eq!(pair.strike_reaction, 3);
        let json = serde_json::to_string(&guarded).unwrap();
        assert!(!json.contains("rearm") && !json.contains("strike_reaction") && !json.contains("\"team\""), "{json}");
    }

    fn errors_of(actions: &str, movesets: &str) -> Vec<String> {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<crate::status::StatusDef>();
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
        ];
        shards_content::ContentDb::build(&kinds, &sources).errors().map(|e| e.to_string()).collect()
    }

    #[test]
    fn a_paired_action_with_a_charge_is_a_content_error() {
        let actions = r#"{
            "fin": (net_id: 1, phases: [("a", 0.3)], pair: (select: (state: Broken, range: 2, height: 1, angle: 60)),
                    charge: (after: 0.2, into: "fin_charged")),
            "fin_charged": (net_id: 2, phases: [("a", 0.3)]),
        }"#;
        let errors = errors_of(actions, "{}");
        assert_eq!(
            errors,
            ["content/a.action.ron: action `fin`: a paired action has no charge: the charged action would run on with a pair that names this one; leave out `charge` or `pair`"],
        );
    }

    #[test]
    fn context_content_is_checked() {
        let actions = r#"{
            "jab": (net_id: 1, phases: [("a", 0.3)]),
            "fin": (net_id: 2, phases: [("a", 0.3)], pair: (select: (state: Broken, range: -1, height: 1, angle: 200),
                    victim_action: "helf", hold_break: -0.5, strike_reaction: 4)),
            "guard": (net_id: 3, phases: [("a", 0.3)], guard: (span: Phase("a"), perfect_needs_press: (gap: -1))),
        }"#;
        let movesets = r#"{
            "m": (inputs: {"light": "jab"},
                remember: [(key: "parried", on: Parried, seconds: 0.5), (key: "parried", on: Blocked, seconds: 0),
                           (key: "", on: Ended(""), seconds: 1)],
                context: [(input: "light", action: "jab", when: [Pair]), (input: "light", action: "fni", when: [Remembers("dodged")]),
                          (input: "heavy", action: "fin", when: [Pair, Condition("")])]),
        }"#;
        let errors = errors_of(actions, movesets);
        let expected = [
            "content/a.action.ron: action `fin`: pair.select.range must be a number >= 0 (is -1)",
            "content/a.action.ron: action `fin`: pair.select.angle is a half-angle, at most 180 degrees (is 200)",
            "content/a.action.ron: action `fin`: pair.hold_break must be a number >= 0 (is -0.5)",
            "content/a.action.ron: action `fin`: pair.strike_reaction must be a tier 0 to 3 (is 4): 1 light, 2 heavy, 3 knockdown, 0 none",
            "content/a.action.ron: action `fin`: pair.victim_action -> unknown action `helf`",
            "content/a.action.ron: action `guard`: guard.perfect_needs_press.gap must be a number >= 0 (is -1)",
            "content/a.action.ron: action `guard`: guard.perfect_needs_press arms a perfect window, and the guard has none: give guard.perfect",
            "content/m.moveset.ron: moveset `m`: remember[1]: key `parried` is remembered twice; give each memory one entry",
            "content/m.moveset.ron: moveset `m`: remember[1].seconds must be above 0 (is 0): a memory that lasts no time is never seen",
            "content/m.moveset.ron: moveset `m`: remember[2].key is empty; name the memory, e.g. `parried`",
            "content/m.moveset.ron: moveset `m`: remember[2] names an empty tag; tags are the actions' `tags`, e.g. `dodge`",
            "content/m.moveset.ron: moveset `m`: context[0]: `Pair` needs a paired action, and `jab` has no `pair`",
            "content/m.moveset.ron: moveset `m`: context[1].action -> unknown action `fni` (did you mean `fin`?)",
            "content/m.moveset.ron: moveset `m`: context[1]: `Remembers(\"dodged\")` names no memory of this moveset; it remembers: parried",
            "content/m.moveset.ron: moveset `m`: context[2]: Condition names nothing",
        ];
        for line in expected {
            assert!(errors.iter().any(|e| e == line), "missing `{line}` in {errors:#?}");
        }
        assert_eq!(errors.len(), expected.len(), "{errors:#?}");
    }
}

#[cfg(test)]
mod weapon_tests {
    use super::*;

    /// The errors of `actions` and `movesets` (and `prefabs`, whose `Charges` the weapon check reads).
    fn errors_of(actions: &str, movesets: &str, prefabs: &str) -> Vec<String> {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<crate::status::StatusDef>();
        kinds.register::<shards_content::PrefabDef>();
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/m.moveset.ron".into(), text: movesets.into() },
            shards_content::db::Source { path: "content/p.prefab.ron".into(), text: prefabs.into() },
        ];
        shards_content::ContentDb::build(&kinds, &sources).errors().map(|e| e.to_string()).collect()
    }

    fn assert_errors(errors: &[String], expected: &[&str]) {
        for line in expected {
            assert!(errors.iter().any(|e| e == line), "missing `{line}` in {errors:#?}");
        }
        assert_eq!(errors.len(), expected.len(), "{errors:#?}");
    }

    const ACTIONS: &str = r#"{
        "shoot": (net_id: 1, phases: [("a", 0.3)], cost: (charge: "pistol")),
        "reload": (net_id: 2, phases: [("a", 0.3)], effects: [(at: 0.3, effect: Refill(charge: "pistol", from: "pistol.reserve"))]),
        "pump": (net_id: 3, phases: [("a", 0.3)], cost: (charge: "shells", charge_count: 2)),
        "pull": (net_id: 4, phases: [("a", 0.3)], effects: [(at: 0.3, effect: Refill(charge: "shells", from: "rockets"))]),
        "flask": (net_id: 5, phases: [("a", 0.3)], cost: (charge: "flask")),
    }"#;

    #[test]
    fn a_weapon_that_declares_what_it_uses_is_clean() {
        let movesets = r#"{
            "pistol": (
                inputs: {"light": "shoot", "reload": "reload", "use_item.flask": "flask"},
                auto_reload: "reload",
                weapon: (magazine: "pistol", reserve: "pistol.reserve", charges: {"pistol": (max: 6), "pistol.reserve": (max: 36, start: 12)}),
            ),
            "plain": (inputs: {"light": "shoot"}),
        }"#;
        // The flask is a prefab's: declared there, not by the weapon. A moveset with no `weapon` is not checked.
        let prefabs = r#"{"hero": (components: {"Charges": ([(name: "flask", current: 3, max: 3)])})}"#;
        assert_errors(&errors_of(ACTIONS, movesets, prefabs), &[]);
    }

    #[test]
    fn a_cost_or_a_refill_on_a_charge_the_weapon_lacks_is_an_error_that_names_the_field() {
        let movesets = r#"{
            "stray": (
                inputs: {"light": "shoot", "reload": "reload", "technique.1": "pull", "technique.2": "pump"},
                weapon: (magazine: "mag", reserve: "pistol.reserve", charges: {"pistol.reserve": (max: 36)}),
            ),
        }"#;
        let tail = "which this weapon does not have: declare it in weapon.charges, or in a prefab's Charges (pistol.reserve)";
        let errors = errors_of(ACTIONS, movesets, "{}");
        assert_errors(
            &errors,
            &[
                "content/m.moveset.ron: moveset `stray`: weapon.magazine -> charge `mag` is declared nowhere: not in weapon.charges, not in a prefab's Charges (pistol.reserve)",
                &format!("content/m.moveset.ron: moveset `stray`: inputs.light -> action `shoot` costs the charge `pistol`, {tail}"),
                &format!("content/m.moveset.ron: moveset `stray`: inputs.reload -> action `reload` refills with the charge `pistol`, {tail}"),
                &format!("content/m.moveset.ron: moveset `stray`: inputs.technique.1 -> action `pull` refills with the charge `shells`, {tail}"),
                &format!("content/m.moveset.ron: moveset `stray`: inputs.technique.1 -> action `pull` refills with the charge `rockets`, {tail}"),
                &format!("content/m.moveset.ron: moveset `stray`: inputs.technique.2 -> action `pump` costs the charge `shells`, {tail}"),
            ],
        );
    }

    #[test]
    fn auto_reload_names_an_action_that_refills() {
        let movesets = r#"{
            "ghost": (inputs: {"light": "shoot"}, auto_reload: "relod"),
            "idle": (inputs: {"light": "shoot"}, auto_reload: "flask"),
        }"#;
        assert_errors(
            &errors_of(ACTIONS, movesets, "{}"),
            &[
                "content/m.moveset.ron: moveset `ghost`: auto_reload -> unknown action `relod` (did you mean `reload`?)",
                "content/m.moveset.ron: moveset `idle`: auto_reload -> action `flask` has no Refill effect: an empty magazine would start something that reloads nothing",
            ],
        );
    }

    #[test]
    fn charge_counts_and_refills_are_checked_where_they_are_written() {
        let actions = r#"{
            "free": (net_id: 1, phases: [("a", 0.3)], cost: (charge_count: 2)),
            "none": (net_id: 2, phases: [("a", 0.3)], cost: (charge: "x", charge_count: 0)),
            "blank": (net_id: 3, phases: [("a", 0.3)], cost: (charge: "")),
            "same": (net_id: 4, phases: [("a", 0.3)], effects: [(at: 0.1, effect: Refill(charge: "x", from: "x"))]),
            "empty": (net_id: 5, phases: [("a", 0.3)], effects: [(at: 0.1, effect: Refill(charge: "", from: "y"))]),
        }"#;
        assert_errors(
            &errors_of(actions, "{}", "{}"),
            &[
                "content/a.action.ron: action `free`: cost.charge_count 2 counts the charges `cost.charge` takes, and the cost names none: give `charge`, or leave the count out",
                "content/a.action.ron: action `none`: cost.charge_count must be at least 1 (is 0): it is how many charges a run takes",
                "content/a.action.ron: action `blank`: cost.charge is empty; name the consumable it takes, e.g. `flask`",
                "content/a.action.ron: action `same`: effects[0] Refill moves charges from `x` into `x`, the same charge: name the reserve it draws from",
                "content/a.action.ron: action `empty`: effects[0] Refill charge is empty; name the charge, e.g. `pistol`",
            ],
        );
    }

    #[test]
    fn weapons_that_share_a_charge_declare_the_same_max_and_start_inside_it() {
        let movesets = r#"{
            "a": (inputs: {"light": "shoot"}, weapon: (charges: {"pistol": (max: 6)})),
            "b": (inputs: {"light": "shoot"}, weapon: (charges: {"pistol": (max: 8, start: 9), "zero": (max: 0)})),
        }"#;
        assert_errors(
            &errors_of(ACTIONS, movesets, "{}"),
            &[
                "content/m.moveset.ron: moveset `a`: weapon.charges `pistol`: max 6 here, 8 in moveset `b`; a body has one `pistol`, so weapons that share it declare the same max",
                "content/m.moveset.ron: moveset `b`: weapon.charges `pistol`: max 8 here, 6 in moveset `a`; a body has one `pistol`, so weapons that share it declare the same max",
                "content/m.moveset.ron: moveset `b`: weapon.charges `pistol`: start 9 is above max 8",
                "content/m.moveset.ron: moveset `b`: weapon.charges `zero`: max must be at least 1",
            ],
        );
    }

    #[test]
    fn the_new_fields_leave_an_old_definition_exactly_as_it_was() {
        // A cost and a moveset that say nothing of weapons serialize as they always did: the replay hash of the
        // content they are in does not move.
        let cost: Cost = ron::from_str("(charge: Some(\"flask\"))").unwrap();
        assert_eq!(cost.charge_count, 1);
        assert_eq!(serde_json::to_string(&cost).unwrap(), r#"{"stamina":0.0,"cooldown":0.0,"charge":"flask"}"#);
        assert!(Cost::default().is_free());
        let moveset: MovesetDef = ron::from_str("(inputs: {\"light\": \"jab\"})").unwrap();
        assert_eq!(serde_json::to_string(&moveset).unwrap(), r#"{"inputs":{"light":"jab"}}"#);
        let counted: Cost = ron::from_str("(charge: Some(\"shell\"), charge_count: 2)").unwrap();
        assert_eq!(serde_json::to_string(&counted).unwrap(), r#"{"stamina":0.0,"cooldown":0.0,"charge":"shell","charge_count":2}"#);
    }
}
