//! The first-person camera (F1b): the view of a character that looks (`Intent::look`, F1a).
//!
//! It is the other camera of the engine, not a mode of the third-person one: a game puts
//! [`FirstPersonCamera`] on its camera entity (or sets `mode: FirstPerson` in the `camera` content,
//! which puts it on every [`ThirdPersonCamera`](super::ThirdPersonCamera) entity). Per frame it
//!
//! 1. turns the look with the mouse and the right stick (`PreUpdate`, after the input map): the
//!    `look` axis times the player's sensitivity (or `aim_sensitivity` while
//!    [`CameraOffsets::aiming`](super::CameraOffsets) says the player aims), inverted when the player
//!    asked, held to the body's pitch limits, and written into the local player's `Intent::look`
//!    **only on a frame the axis moved**, so a look an agent wrote with the `look` tool, a recorded
//!    replay's inputs and a body that is not looked through are never overwritten;
//! 2. puts the camera where the body's eye is drawn (`PostUpdate`): the interpolated feet plus the
//!    body's `eye_height`, eased on steps, dipped on a landing, optionally bobbing and leaning, and
//!    turned by whatever `Intent::look` holds (the camera reads the look it does not own, so a replay
//!    or a tool turns the picture too), then shaken as the third-person camera is.
//!
//! The simulation never reads the camera: the eye and the ray come from `Intent` and the body
//! (`shards_motion::{eye_of, look_dir}`), so a headless run, a replay and a window agree.
//!
//! This file is the window-free half: the component, the numbers (content: `mode` and `first_person`
//! of the `camera` kind) and the maths, all tested without Bevy's renderer. The systems are in
//! [`systems`] behind the `render` feature.

#[cfg(feature = "render")]
pub(super) mod systems;
mod tool;

use bevy::math::ops;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_core::{flat, Gaze, Intent};

use super::math::{critically_damped_step, damp, wrap_angle};
use super::{CameraTuning, ThirdPersonCamera};
use crate::controller::CharacterParams;

/// The render layer of the looked-through body's own model (and everything under it): the first-person
/// camera does not draw it. Lights that carry no `RenderLayers` of their own get it (and
/// [`VIEW_MODEL_LAYER`]) while a first-person camera exists, so the body still casts its shadow; a
/// camera that should see the body adds this layer to its own `RenderLayers`. High on purpose: a game's
/// own layers (a minimap, a preview) are the low numbers.
pub const OWN_BODY_LAYER: usize = 62;

/// The render layer of the view model (the arms and the weapon): only the view-model camera draws it.
pub const VIEW_MODEL_LAYER: usize = 63;

/// Which camera the game plays through ([`CameraTuning::mode`](super::CameraTuning)).
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraMode {
    /// The orbit camera, as it always was.
    #[default]
    ThirdPerson,
    /// Every [`ThirdPersonCamera`](super::ThirdPersonCamera) entity also gets a [`FirstPersonCamera`]
    /// (and the third-person camera stands still).
    FirstPerson,
}

/// A camera that sits at the eye of a character. Put it on the camera entity; the body it looks
/// through is `target`, else the entity with `LocalPlayer`. Writes the local player's `Intent::look`.
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default)]
pub struct FirstPersonCamera {
    /// The character to look through. [`Entity::PLACEHOLDER`] (the default): the entity with
    /// `LocalPlayer`, whichever it is now.
    pub target: Entity,
    /// What the eye has smoothed so far (presentation).
    pub eye: EyeState,
    /// Where the body was drawn on the last frame (`None` until the first): a body that moved farther
    /// than `CameraTuning::body_snap_distance` since was placed, not walked, and the eye starts over.
    pub subject_seen: Option<Vec3>,
    /// Whether the camera is on (`Camera::is_active`, which the `render` feature copies here every frame, before
    /// the input map is read): a camera that is switched off is nobody's view, so the keyboard's walk, the
    /// orbit camera's lock-on and the ears leave it out. A camera nobody switched is on.
    pub active: bool,
    /// The body whose look this camera has seeded from its heading: the first time it looks through a body that has
    /// no `Intent::look`, it writes one (the heading, level), so that a body that turns by its movement does not
    /// carry the view round with it before the first mouse move. Once per body: a look cleared later is not made again.
    pub seeded_for: Option<Entity>,
}

impl Default for FirstPersonCamera {
    fn default() -> Self {
        Self { target: Entity::PLACEHOLDER, eye: EyeState::default(), subject_seen: None, active: true, seeded_for: None }
    }
}

impl FirstPersonCamera {
    /// Whether this camera is on and looks through `body`: it names it, or it names none (the default) and
    /// `body` is the local player (`local`).
    pub fn looks_through(&self, body: Entity, local: bool) -> bool {
        self.active && (self.target == body || (self.target == Entity::PLACEHOLDER && local))
    }

    /// A camera looking through `target`.
    pub fn looking_through(target: Entity) -> Self {
        Self { target, ..default() }
    }
}

/// Marks a [`FirstPersonCamera`] the engine put on a camera because the `camera` content says
/// `mode: FirstPerson` (and takes off again when the mode goes back); one a game spawned is its own.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct FromMode;

/// Registers the first-person camera's tool.
pub(super) fn plugin(app: &mut App) {
    tool::plugin(app);
}

/// This session's choice of the camera mode, over the `camera` content's: the `camera_mode` tool sets
/// it (an agent or a developer looks through the first-person camera without editing a file), and
/// `None` (the default) leaves the content in charge. Presentation: it is not saved, recorded or replayed.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraModeOverride(pub Option<CameraMode>);

/// `PreUpdate`, after the tuning is resolved: in `FirstPerson` mode every
/// [`ThirdPersonCamera`] entity also gets a [`FirstPersonCamera`] looking through what it followed (the
/// orbit camera stays on it, standing still), and when the mode goes back the ones the engine added are
/// taken off again. A camera the game made first-person itself is its own and is left alone.
pub(super) fn apply_mode(
    mut commands: Commands,
    tuning: Res<CameraTuning>,
    chosen: Res<CameraModeOverride>,
    orbiting: Query<(Entity, &ThirdPersonCamera), Without<FirstPersonCamera>>,
    made: Query<Entity, (With<FirstPersonCamera>, With<FromMode>)>,
) {
    match chosen.0.unwrap_or(tuning.mode) {
        CameraMode::FirstPerson => {
            for (entity, orbit) in &orbiting {
                commands.entity(entity).insert((FirstPersonCamera::looking_through(orbit.target), FromMode));
            }
        }
        CameraMode::ThirdPerson => {
            for entity in &made {
                commands.entity(entity).remove::<(FirstPersonCamera, FromMode)>();
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------------------------

/// Names a `skip_serializing_if` predicate for a field whose default is `$value`.
macro_rules! is_default {
    ($name:ident, $ty:ty, $value:expr) => {
        fn $name(v: &$ty) -> bool {
            *v == $value
        }
    };
}

is_default!(step_ease_is_default, Option<f32>, Some(25.0));
is_default!(max_trail_is_default, f32, 0.6);
is_default!(landing_is_default, Option<LandingTuning>, Some(LandingTuning::DEFAULT));
is_default!(omega_is_default, f32, 14.0);
is_default!(kick_is_default, f32, 0.45);
is_default!(min_impact_is_default, f32, 3.0);
is_default!(max_dip_is_default, f32, 0.12);
is_default!(bob_vertical_is_default, f32, 0.02);
is_default!(bob_sway_is_default, f32, 0.012);
is_default!(bob_stride_is_default, f32, 1.8);
is_default!(bob_min_speed_is_default, f32, 0.5);
is_default!(bob_fade_is_default, f32, 8.0);
is_default!(roll_deg_is_default, f32, 0.5);
is_default!(roll_max_is_default, f32, 2.5);
is_default!(roll_rate_is_default, f32, 10.0);

/// What the first-person camera does with the eye: [`CameraTuning::first_person`](super::CameraTuning).
/// The defaults give a stable view (a short ease on steps, a small dip on a hard landing) with no bob
/// and no lean; the game tunes the rest.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct FirstPersonTuning {
    /// How fast the eye catches up with the body's after a step or a crouch while the body stands on
    /// the ground (1/s, the exact exponential): stairs do not jerk the picture. `None`: the eye follows
    /// the body exactly. In the air the eye follows the body at once (the ease only runs out).
    #[serde(skip_serializing_if = "step_ease_is_default")]
    pub step_ease: Option<f32>,
    /// The farthest the eased eye may trail the body's own (m).
    #[serde(skip_serializing_if = "max_trail_is_default")]
    pub max_trail_m: f32,
    /// The dip of a landing: the eye is knocked down by the impact and settles back, critically
    /// damped. `None`: no dip.
    #[serde(skip_serializing_if = "landing_is_default")]
    pub landing: Option<LandingTuning>,
    /// A walking bob. `None` (the default): none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bob: Option<BobTuning>,
    /// A lean into a sideways walk. `None` (the default): none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<RollTuning>,
    /// The id of the `view_model` definition drawn when the weapon in hand (the body's moveset) names
    /// none. `None`: such a body shows no view model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_model: Option<String>,
}

impl Default for FirstPersonTuning {
    fn default() -> Self {
        Self { step_ease: Some(25.0), max_trail_m: 0.6, landing: Some(LandingTuning::DEFAULT), bob: None, roll: None, view_model: None }
    }
}

/// The landing dip ([`FirstPersonTuning::landing`]).
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct LandingTuning {
    /// How fast the dip settles (1/s, critically damped: no overshoot). Higher is snappier.
    #[serde(skip_serializing_if = "omega_is_default")]
    pub omega: f32,
    /// The speed the dip starts with, per m/s of the impact (m/s per m/s).
    #[serde(skip_serializing_if = "kick_is_default")]
    pub kick: f32,
    /// A landing slower than this (m/s) does not dip.
    #[serde(skip_serializing_if = "min_impact_is_default")]
    pub min_impact: f32,
    /// The deepest the eye goes (m), however hard the landing.
    #[serde(skip_serializing_if = "max_dip_is_default")]
    pub max_dip_m: f32,
}

impl LandingTuning {
    const DEFAULT: Self = Self { omega: 14.0, kick: 0.45, min_impact: 3.0, max_dip_m: 0.12 };
}

impl Default for LandingTuning {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A walking bob ([`FirstPersonTuning::bob`]): the eye rises and falls twice and sways once per stride.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct BobTuning {
    /// The most the eye rises and falls (m).
    #[serde(skip_serializing_if = "bob_vertical_is_default")]
    pub vertical_m: f32,
    /// The most it sways to the side (m).
    #[serde(skip_serializing_if = "bob_sway_is_default")]
    pub sway_m: f32,
    /// Metres walked per full cycle (a left and a right step).
    #[serde(skip_serializing_if = "bob_stride_is_default")]
    pub stride_m: f32,
    /// Slower than this (m/s) on the ground there is no bob.
    #[serde(skip_serializing_if = "bob_min_speed_is_default")]
    pub min_speed: f32,
    /// How fast the bob comes in and dies away (1/s).
    #[serde(skip_serializing_if = "bob_fade_is_default")]
    pub fade_rate: f32,
}

impl Default for BobTuning {
    fn default() -> Self {
        Self { vertical_m: 0.02, sway_m: 0.012, stride_m: 1.8, min_speed: 0.5, fade_rate: 8.0 }
    }
}

/// A lean into a sideways walk ([`FirstPersonTuning::roll`]): strafing right tilts the view to the right.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct RollTuning {
    /// Degrees of roll per m/s of sideways speed.
    #[serde(skip_serializing_if = "roll_deg_is_default")]
    pub deg_per_mps: f32,
    /// The most it leans (degrees).
    #[serde(skip_serializing_if = "roll_max_is_default")]
    pub max_deg: f32,
    /// How fast the lean follows the walk (1/s).
    #[serde(skip_serializing_if = "roll_rate_is_default")]
    pub rate: f32,
}

impl Default for RollTuning {
    fn default() -> Self {
        Self { deg_per_mps: 0.5, max_deg: 2.5, rate: 10.0 }
    }
}

impl FirstPersonTuning {
    /// Whether this is what the code does without a definition.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// What is wrong with these numbers, one line each, named `first_person.<field>`; empty when fine.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let finite = |name: &str, v: f32, out: &mut Vec<String>| {
            if !v.is_finite() {
                out.push(format!("first_person.{name} is {v}, it must be a finite number"));
                return false;
            }
            true
        };
        let positive = |name: &str, v: f32, out: &mut Vec<String>| {
            if finite(name, v, out) && v <= 0.0 {
                out.push(format!("first_person.{name} is {v}, it must be above 0"));
            }
        };
        let non_negative = |name: &str, v: f32, out: &mut Vec<String>| {
            if finite(name, v, out) && v < 0.0 {
                out.push(format!("first_person.{name} is {v}, it cannot be negative"));
            }
        };
        if let Some(rate) = self.step_ease {
            positive("step_ease", rate, &mut out);
        }
        non_negative("max_trail_m", self.max_trail_m, &mut out);
        if let Some(l) = &self.landing {
            positive("landing.omega", l.omega, &mut out);
            non_negative("landing.kick", l.kick, &mut out);
            non_negative("landing.min_impact", l.min_impact, &mut out);
            non_negative("landing.max_dip_m", l.max_dip_m, &mut out);
        }
        if let Some(b) = &self.bob {
            non_negative("bob.vertical_m", b.vertical_m, &mut out);
            non_negative("bob.sway_m", b.sway_m, &mut out);
            positive("bob.stride_m", b.stride_m, &mut out);
            non_negative("bob.min_speed", b.min_speed, &mut out);
            positive("bob.fade_rate", b.fade_rate, &mut out);
        }
        if let Some(r) = &self.roll {
            non_negative("roll.deg_per_mps", r.deg_per_mps, &mut out);
            non_negative("roll.max_deg", r.max_deg, &mut out);
            if r.max_deg > 15.0 {
                out.push(format!("first_person.roll.max_deg is {} degrees, more than 15 is not a lean", r.max_deg));
            }
            positive("roll.rate", r.rate, &mut out);
        }
        if let Some(id) = &self.view_model
            && id.is_empty()
        {
            out.push("first_person.view_model is empty: name a `view_model` definition, or leave it out".to_owned());
        }
        out
    }
}

// ---------------------------------------------------------------------------------------------
// The look
// ---------------------------------------------------------------------------------------------

/// The heading of a body, level: where it looks when it has no look yet. A body with no position looks north.
pub fn heading_of(body: Option<&Transform>) -> Gaze {
    body.and_then(|tf| Gaze::toward(Vec3::ZERO, flat(*tf.forward()))).unwrap_or_default()
}

/// The look a first-person camera has through a body: what its `Intent::look` holds (finite, with the pitch held
/// to the body's limits), else the body's heading, level. The camera turns it with the mouse and draws by it, and
/// the input map walks by its yaw, so the two never disagree about where forward is.
pub fn look_through(intent: Option<&Intent>, params: Option<&CharacterParams>, body: Option<&Transform>) -> Gaze {
    let held = intent.and_then(|i| i.look).filter(|l| l.is_finite());
    match (held, params) {
        (Some(look), Some(p)) => p.clamp_look(look),
        (Some(look), None) => look,
        (None, _) => heading_of(body),
    }
}

/// `current` turned by this frame's `look` axis (`x` right, `y` up, radians per unit already scaled by
/// the map) at `speed` (the player's sensitivity). Mouse right turns the view right (a negative yaw:
/// a positive yaw turns left), mouse up looks up; `invert_y` flips the second. The yaw wraps into
/// -π..π and the pitch is held between `limits` (radians, lowest and highest).
pub fn turn_look(current: Gaze, axis: Vec2, speed: f32, invert_y: bool, limits: (f32, f32)) -> Gaze {
    let up = if invert_y { -1.0 } else { 1.0 };
    let (lo, hi) = if limits.0.is_finite() && limits.1.is_finite() {
        (limits.0.min(limits.1), limits.0.max(limits.1))
    } else {
        (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2)
    };
    Gaze {
        yaw: wrap_angle(current.yaw - axis.x * speed),
        pitch: (current.pitch + axis.y * speed * up).clamp(lo, hi),
    }
}

/// The camera's rotation for a look, with `roll` radians of lean to the right (a negative turn about
/// the view axis): its forward is exactly [`Gaze::dir`].
pub fn view_rotation(look: Gaze, roll: f32) -> Quat {
    Quat::from_euler(EulerRot::YXZ, look.yaw, look.pitch, 0.0) * Quat::from_rotation_z(-roll)
}

// ---------------------------------------------------------------------------------------------
// The eye
// ---------------------------------------------------------------------------------------------

/// What the eye has smoothed so far: a field of [`FirstPersonCamera`], advanced once a frame by
/// [`step`](Self::step). All of it is presentation; nothing here reaches the simulation.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Default)]
pub struct EyeState {
    /// Where the body's own eye was on the last frame (world y); `None` before the first.
    pub last_y: Option<f32>,
    /// How far the eased eye sits above the body's own eye (m; below, when negative).
    pub trail: f32,
    /// How far a landing has pushed the eye down (m) and how fast (m/s, downward).
    pub dip: f32,
    pub dip_speed: f32,
    /// The impact speed of a landing not yet turned into a dip (m/s; the `Landed` event notes it).
    pub impact: f32,
    /// Where in a stride the bob is (radians), and how much of it shows (0..1).
    pub stride: f32,
    pub bob: f32,
    /// The lean (radians, positive to the right).
    pub lean: f32,
}

/// What the body did this frame, for [`EyeState::step`].
#[derive(Clone, Copy, Debug)]
pub struct EyeFrame {
    /// Where the body's own eye is (world y).
    pub eye_y: f32,
    /// The body was placed (a teleport, a respawn): the eye starts over.
    pub placed: bool,
    /// The body stands on the ground.
    pub grounded: bool,
    /// How fast it walks on the ground (m/s).
    pub speed: f32,
    /// How fast it moves to the right of where it looks (m/s; negative to the left).
    pub side_speed: f32,
    /// The frame time (s).
    pub dt: f32,
}

/// Where the eye is moved from the body's own, and how it is tilted: [`EyeState::step`]'s answer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EyeShift {
    /// Added to the eye's height (m): the ease on a step, the dip of a landing, the bob.
    pub lift: f32,
    /// Added to the eye along the view's right (m): the sway of the bob.
    pub sway: f32,
    /// The lean (radians, positive to the right).
    pub roll: f32,
}

/// The longest frame the eye's smoothing believes (s): a stall is not a long walk.
const MAX_DT: f32 = 0.1;

impl EyeState {
    /// A landing at `impact_speed` m/s (the `Landed` event): the dip starts on the next [`step`](Self::step).
    pub fn land(&mut self, impact_speed: f32) {
        if impact_speed.is_finite() {
            self.impact = self.impact.max(impact_speed);
        }
    }

    /// One frame. Exact for any `dt` (the exponentials and the spring are closed forms).
    pub fn step(&mut self, tuning: &FirstPersonTuning, frame: EyeFrame) -> EyeShift {
        let dt = if frame.dt.is_finite() { frame.dt.clamp(0.0, MAX_DT) } else { 0.0 };
        // What the body did, as numbers a view can use.
        let sane = |v: f32| if v.is_finite() { v } else { 0.0 };
        let (speed, side_speed) = (sane(frame.speed).max(0.0), sane(frame.side_speed));
        if frame.placed || !frame.eye_y.is_finite() {
            *self = Self { stride: self.stride, ..Self::default() };
        }

        // Steps: while the body stands, the eased eye stays where it was when the body's eye jumps,
        // then catches up. In the air the body's eye is followed at once and only the ease runs out.
        match (tuning.step_ease, self.last_y) {
            (Some(rate), Some(last)) if frame.eye_y.is_finite() => {
                if frame.grounded {
                    self.trail -= frame.eye_y - last;
                }
                let reach = tuning.max_trail_m.max(0.0);
                self.trail = damp(self.trail, 0.0, rate, dt).clamp(-reach, reach);
            }
            _ => self.trail = 0.0,
        }
        self.last_y = frame.eye_y.is_finite().then_some(frame.eye_y);

        // The landing: a hard one kicks the dip, which settles back without overshoot.
        match &tuning.landing {
            Some(l) => {
                if self.impact >= l.min_impact {
                    // The peak of a kick `v` settling at `omega` is `v / (omega e)`: cap it at `max_dip_m`.
                    let cap = l.max_dip_m * l.omega * std::f32::consts::E;
                    self.dip_speed += (self.impact * l.kick).min(cap);
                }
                (self.dip, self.dip_speed) = critically_damped_step(self.dip, self.dip_speed, 0.0, l.omega, dt);
            }
            None => (self.dip, self.dip_speed) = (0.0, 0.0),
        }
        self.impact = 0.0;

        // The bob: a stride's worth of ground walked is a cycle.
        let (mut up, mut sway) = (0.0, 0.0);
        match &tuning.bob {
            Some(b) => {
                let walking = frame.grounded && speed >= b.min_speed;
                self.bob = damp(self.bob, if walking { 1.0 } else { 0.0 }, b.fade_rate, dt);
                if walking {
                    self.stride = (self.stride + speed * dt / b.stride_m * std::f32::consts::TAU).rem_euclid(std::f32::consts::TAU);
                }
                up = b.vertical_m * self.bob * ops::sin(2.0 * self.stride);
                sway = b.sway_m * self.bob * ops::sin(self.stride);
            }
            None => self.bob = 0.0,
        }

        // The lean.
        match &tuning.roll {
            Some(r) => {
                let most = r.max_deg.max(0.0);
                let want = (side_speed * r.deg_per_mps).clamp(-most, most).to_radians();
                self.lean = damp(self.lean, sane(want), r.rate, dt);
            }
            None => self.lean = 0.0,
        }

        EyeShift { lift: self.trail - self.dip + up, sway, roll: self.lean }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    const DT: f32 = 1.0 / 60.0;

    fn frame(eye_y: f32, grounded: bool) -> EyeFrame {
        EyeFrame { eye_y, placed: false, grounded, speed: 0.0, side_speed: 0.0, dt: DT }
    }

    #[test]
    fn the_defaults_give_a_stable_view_and_serialize_to_nothing() {
        let t = FirstPersonTuning::default();
        assert!(t.is_default() && t.bob.is_none() && t.roll.is_none(), "bob and lean are off by default");
        assert!(t.step_ease.is_some() && t.landing.is_some());
        assert!(t.problems().is_empty(), "{:?}", t.problems());
        assert_eq!(ron::to_string(&t).unwrap(), "()");
        let set: FirstPersonTuning = ron::from_str("(bob: Some(()), roll: Some((max_deg: 4.0)), landing: None, step_ease: None)").unwrap();
        assert_eq!((set.bob, set.landing, set.step_ease), (Some(BobTuning::default()), None, None));
        assert_eq!(set.roll.unwrap().max_deg, 4.0);
        assert_eq!(ron::from_str::<FirstPersonTuning>(&ron::to_string(&set).unwrap()).unwrap(), set, "a round trip");
        assert!(ron::from_str::<FirstPersonTuning>("(stepease: None)").is_err(), "a typo is an error");
    }

    #[test]
    fn validation_names_the_field() {
        let bad = |edit: fn(&mut FirstPersonTuning)| {
            let mut t = FirstPersonTuning::default();
            edit(&mut t);
            t.problems()
        };
        assert!(bad(|t| t.step_ease = Some(0.0))[0].contains("first_person.step_ease"));
        assert!(bad(|t| t.max_trail_m = -1.0)[0].contains("max_trail_m"));
        assert!(bad(|t| t.landing.as_mut().unwrap().omega = 0.0)[0].contains("landing.omega"));
        assert!(bad(|t| t.landing.as_mut().unwrap().max_dip_m = f32::NAN)[0].contains("finite"));
        assert!(bad(|t| t.bob = Some(BobTuning { stride_m: 0.0, ..default() }))[0].contains("bob.stride_m"));
        assert!(bad(|t| t.roll = Some(RollTuning { max_deg: 40.0, ..default() }))[0].contains("not a lean"));
        assert!(bad(|t| t.view_model = Some(String::new()))[0].contains("view_model is empty"));
    }

    #[test]
    fn the_mouse_turns_the_view_the_way_it_moves() {
        let limits = ((-85.0_f32).to_radians(), 85.0_f32.to_radians());
        // The axis is `x` right, `y` up. Right turns right: a smaller yaw (a positive one turns left).
        let right = turn_look(Gaze::default(), Vec2::new(0.1, 0.0), 1.0, false, limits);
        assert!(right.yaw < 0.0 && right.pitch == 0.0, "{right:?}");
        let left = turn_look(Gaze::default(), Vec2::new(-0.1, 0.0), 1.0, false, limits);
        assert!(left.yaw > 0.0 && (left.yaw + right.yaw).abs() < 1e-6);
        // Up looks up, and an inverted mouse looks down.
        assert!(turn_look(Gaze::default(), Vec2::new(0.0, 0.1), 1.0, false, limits).pitch > 0.0);
        assert!(turn_look(Gaze::default(), Vec2::new(0.0, 0.1), 1.0, true, limits).pitch < 0.0);
        // The sensitivity scales the turn.
        let slow = turn_look(Gaze::default(), Vec2::new(0.1, 0.1), 0.5, false, limits);
        let fast = turn_look(Gaze::default(), Vec2::new(0.1, 0.1), 2.0, false, limits);
        assert!((fast.yaw / slow.yaw - 4.0).abs() < 1e-4 && (fast.pitch / slow.pitch - 4.0).abs() < 1e-4);
    }

    #[test]
    fn the_pitch_is_held_to_the_limits_and_the_yaw_wraps() {
        let limits = ((-30.0_f32).to_radians(), 45.0_f32.to_radians());
        let up = turn_look(Gaze::default(), Vec2::new(0.0, 5.0), 1.0, false, limits);
        let down = turn_look(Gaze::default(), Vec2::new(0.0, -5.0), 1.0, false, limits);
        assert_eq!((up.pitch, down.pitch), (limits.1, limits.0));
        // Limits given the wrong way round still hold.
        assert_eq!(turn_look(Gaze::default(), Vec2::new(0.0, 5.0), 1.0, false, (limits.1, limits.0)).pitch, limits.1);
        // Turning left through half a turn stays within -π..π.
        let mut look = Gaze::default();
        for _ in 0..100 {
            look = turn_look(look, Vec2::new(-0.1, 0.0), 1.0, false, limits);
            assert!(look.yaw.abs() <= PI + 1e-5, "{}", look.yaw);
        }
        // A small turn keeps what was set: no drift when the axis is still.
        let held = Gaze::from_degrees(33.0, 12.0);
        assert_eq!(turn_look(held, Vec2::ZERO, 1.0, false, limits), Gaze { yaw: wrap_angle(held.yaw), pitch: held.pitch });
    }

    #[test]
    fn the_view_looks_along_the_gaze() {
        for (yaw, pitch) in [(0.0, 0.0), (90.0, 0.0), (-90.0, 30.0), (135.0, -45.0), (0.0, 80.0)] {
            let look = Gaze::from_degrees(yaw, pitch);
            let forward = view_rotation(look, 0.0) * Vec3::NEG_Z;
            assert!(forward.distance(look.dir()) < 1e-5, "{yaw} {pitch}: {forward} against {}", look.dir());
        }
        // Yaw 0 looks north (-Z), a quarter turn looks west (-X).
        assert!((view_rotation(Gaze::from_degrees(90.0, 0.0), 0.0) * Vec3::NEG_Z).distance(Vec3::NEG_X) < 1e-5);
        // A lean to the right drops the view's right side and leaves the forward alone.
        let level = view_rotation(Gaze::default(), 0.0);
        let leaning = view_rotation(Gaze::default(), 0.2);
        assert!((leaning * Vec3::NEG_Z).distance(level * Vec3::NEG_Z) < 1e-6);
        assert!((leaning * Vec3::X).y < -0.1 && (level * Vec3::X).y.abs() < 1e-6, "{}", (leaning * Vec3::X));
    }

    #[test]
    fn a_step_is_eased_and_a_walk_is_followed() {
        let tuning = FirstPersonTuning::default();
        let mut eye = EyeState::default();
        let mut y = 1.6;
        assert_eq!(eye.step(&tuning, frame(y, true)).lift, 0.0, "the first frame has nothing to ease from");
        for _ in 0..30 {
            assert_eq!(eye.step(&tuning, frame(y, true)).lift, 0.0, "standing still");
        }
        // A step of 0.3 m in one frame: the eased eye stays almost where it was, then catches up.
        y += 0.3;
        let first = eye.step(&tuning, frame(y, true)).lift;
        assert!(first < -0.15 && first > -0.3, "most of the step is still to come: {first}");
        let mut last = first;
        for _ in 0..60 {
            let lift = eye.step(&tuning, frame(y, true)).lift;
            assert!(lift >= last - 1e-6, "it only climbs: {lift} after {last}");
            last = lift;
        }
        assert!(last.abs() < 0.01, "a second later it has caught up: {last}");
        // Down a step is the mirror.
        y -= 0.3;
        assert!(eye.step(&tuning, frame(y, true)).lift > 0.15);
    }

    #[test]
    fn in_the_air_the_eye_follows_at_once() {
        let tuning = FirstPersonTuning::default();
        let mut eye = EyeState::default();
        eye.step(&tuning, frame(1.6, true));
        // A jump: the body's eye rises 0.15 a frame, and the camera is on it.
        let mut y = 1.6;
        for _ in 0..10 {
            y += 0.15;
            assert!(eye.step(&tuning, frame(y, false)).lift.abs() < 1e-6, "no lag in a jump");
        }
        // A step ease still running out when the jump starts is not made longer.
        y += 0.3;
        eye.step(&tuning, frame(y, true));
        assert!(eye.trail < -0.1);
        for _ in 0..30 {
            y += 0.1;
            eye.step(&tuning, frame(y, false));
        }
        assert!(eye.trail.abs() < 0.01, "the ease ran out while airborne: {}", eye.trail);
    }

    #[test]
    fn without_a_step_ease_the_eye_is_the_bodys() {
        let tuning = FirstPersonTuning { step_ease: None, ..default() };
        let mut eye = EyeState::default();
        eye.step(&tuning, frame(1.6, true));
        assert_eq!(eye.step(&tuning, frame(1.9, true)).lift, 0.0);
    }

    #[test]
    fn a_placed_body_starts_the_eye_over() {
        let tuning = FirstPersonTuning::default();
        let mut eye = EyeState::default();
        eye.step(&tuning, frame(1.6, true));
        eye.step(&tuning, frame(1.9, true));
        eye.land(10.0);
        assert!(eye.trail != 0.0);
        let placed = eye.step(&tuning, EyeFrame { placed: true, ..frame(9.0, true) });
        assert_eq!((placed.lift, eye.trail, eye.dip, eye.dip_speed), (0.0, 0.0, 0.0, 0.0), "nothing of the old place");
    }

    #[test]
    fn a_hard_landing_dips_and_settles_without_overshoot() {
        let tuning = FirstPersonTuning::default();
        let l = tuning.landing.unwrap();
        let mut eye = EyeState::default();
        eye.step(&tuning, frame(1.6, true));
        eye.land(8.0);
        let mut deepest: f32 = 0.0;
        let mut lifts = Vec::new();
        for _ in 0..90 {
            let lift = eye.step(&tuning, frame(1.6, true)).lift;
            deepest = deepest.min(lift);
            lifts.push(lift);
        }
        assert!(deepest < -0.04 && deepest >= -l.max_dip_m - 1e-4, "a dip, within the cap: {deepest}");
        assert!(lifts.iter().all(|&v| v <= 1e-6), "the eye never rises above its own (no overshoot)");
        assert!(lifts.last().unwrap().abs() < 1e-3, "settled after 1.5 s: {}", lifts.last().unwrap());
        // The deepest point is where the spring says: a bit after 1/omega.
        let at = lifts.iter().position(|&v| v == deepest).unwrap() as f32 * DT;
        assert!((0.04..0.14).contains(&at), "{at}");
    }

    #[test]
    fn a_soft_landing_does_not_dip_and_a_harder_one_dips_deeper_up_to_the_cap() {
        let tuning = FirstPersonTuning::default();
        let depth = |impact: f32| {
            let mut eye = EyeState::default();
            eye.step(&tuning, frame(1.6, true));
            eye.land(impact);
            (0..60).map(|_| eye.step(&tuning, frame(1.6, true)).lift).fold(0.0_f32, f32::min)
        };
        assert_eq!(depth(2.0), 0.0, "under min_impact");
        let (soft, hard, brutal) = (depth(4.0), depth(8.0), depth(40.0));
        assert!(soft > hard && hard > brutal - 1e-6, "{soft} {hard} {brutal}");
        assert!(brutal >= -0.12 - 2e-3, "capped: {brutal}");
        let off = FirstPersonTuning { landing: None, ..default() };
        let mut eye = EyeState::default();
        eye.step(&off, frame(1.6, true));
        eye.land(20.0);
        assert_eq!(eye.step(&off, frame(1.6, true)).lift, 0.0, "no landing definition, no dip");
    }

    #[test]
    fn the_smoothing_does_not_depend_on_the_frame_rate() {
        let tuning = FirstPersonTuning::default();
        let run = |frames: u32| {
            let mut eye = EyeState::default();
            let dt = 0.5 / frames as f32;
            let at = |y: f32| EyeFrame { dt, ..frame(y, true) };
            eye.step(&tuning, at(1.6));
            eye.step(&tuning, at(1.9));
            eye.land(8.0);
            let mut shift = EyeShift::default();
            for _ in 1..frames {
                shift = eye.step(&tuning, at(1.9));
            }
            shift.lift
        };
        let (slow, fast) = (run(30), run(240));
        assert!((slow - fast).abs() < 0.01, "half a second at 60 or 480 Hz alike: {slow} against {fast}");
    }

    #[test]
    fn the_bob_follows_the_stride_and_fades_in_and_out() {
        let tuning = FirstPersonTuning { bob: Some(BobTuning::default()), ..default() };
        let b = BobTuning::default();
        let mut eye = EyeState::default();
        let walk = |speed: f32| EyeFrame { speed, ..frame(1.6, true) };
        let mut top: f32 = 0.0;
        let mut widest: f32 = 0.0;
        let mut at_start = None;
        for i in 0..240 {
            let shift = eye.step(&tuning, walk(4.0));
            at_start.get_or_insert(shift.lift.abs());
            top = top.max(shift.lift.abs());
            widest = widest.max(shift.sway.abs());
            assert!(shift.lift.abs() <= b.vertical_m + 1e-6 && shift.sway.abs() <= b.sway_m + 1e-6, "frame {i}");
        }
        assert!(top > 0.9 * b.vertical_m && widest > 0.9 * b.sway_m, "the bob reaches its size: {top} {widest}");
        assert!(at_start.unwrap() < 0.2 * b.vertical_m, "it fades in");
        // A stride is `stride_m` metres: at 4 m/s one cycle takes 0.45 s, so after 1.8 m the phase is back.
        let mut eye = EyeState::default();
        for _ in 0..27 {
            eye.step(&tuning, walk(4.0));
        }
        assert!(eye.stride.min(std::f32::consts::TAU - eye.stride) < 0.05, "a 1.8 m stride in 27 frames at 4 m/s: {}", eye.stride);
        // Standing still the bob fades away; slower than min_speed there is none; in the air neither.
        for _ in 0..120 {
            eye.step(&tuning, walk(0.0));
        }
        assert!(eye.bob < 0.01 && eye.step(&tuning, walk(0.0)).lift.abs() < 1e-3);
        let mut flying = EyeState::default();
        for _ in 0..60 {
            assert_eq!(flying.step(&tuning, EyeFrame { speed: 4.0, ..frame(1.6, false) }).sway, 0.0);
        }
        // Off by default: nothing moves.
        let mut still = EyeState::default();
        let plain = FirstPersonTuning::default();
        for _ in 0..60 {
            assert_eq!(still.step(&plain, walk(4.0)), EyeShift::default());
        }
    }

    #[test]
    fn the_lean_follows_the_sideways_walk_and_is_capped() {
        let tuning = FirstPersonTuning { roll: Some(RollTuning::default()), ..default() };
        let r = RollTuning::default();
        let mut eye = EyeState::default();
        let strafe = |side: f32| EyeFrame { side_speed: side, ..frame(1.6, true) };
        let mut last = EyeShift::default();
        for _ in 0..120 {
            last = eye.step(&tuning, strafe(4.0));
        }
        // 4 m/s at 0.5 degrees per m/s is 2 degrees to the right, under the 2.5 cap.
        assert!((last.roll - 2.0_f32.to_radians()).abs() < 1e-3, "{}", last.roll);
        for _ in 0..120 {
            last = eye.step(&tuning, strafe(-20.0));
        }
        assert!((last.roll + r.max_deg.to_radians()).abs() < 1e-3, "to the left, capped: {}", last.roll);
        for _ in 0..120 {
            last = eye.step(&tuning, strafe(0.0));
        }
        assert!(last.roll.abs() < 1e-3, "back to level");
    }

    #[test]
    fn nonsense_in_gives_a_finite_view() {
        let tuning = FirstPersonTuning { bob: Some(BobTuning::default()), roll: Some(RollTuning::default()), ..default() };
        let mut eye = EyeState::default();
        eye.step(&tuning, frame(1.6, true));
        for bad in [f32::NAN, f32::INFINITY, -1.0e30] {
            let shift = eye.step(&tuning, EyeFrame { eye_y: bad, speed: bad, side_speed: bad, dt: bad, ..frame(1.6, true) });
            assert!(shift.lift.is_finite() && shift.sway.is_finite() && shift.roll.is_finite(), "{bad}: {shift:?}");
        }
        eye.land(f32::NAN);
        assert_eq!(eye.impact, 0.0);
        let shift = eye.step(&tuning, frame(1.6, true));
        assert!(shift.lift.is_finite() && eye.trail.is_finite());
    }
}
