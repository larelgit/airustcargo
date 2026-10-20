//! Every number the camera decides by, as data: [`CameraTuning`] (content kind `camera`,
//! hot-reloadable) and [`CameraOffsets`], the hook through which a game's own systems add to
//! the camera's base values for a while (sprint field of view, a landing dip, a swim pitch cap).
//!
//! The defaults are the numbers the camera had as Rust constants before they became content, so
//! a game without a `camera` definition sees no change.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};

/// Names a `skip_serializing_if` predicate for a field whose default is `$value`: a field that
/// has its default is left out of the serialized form (so adding a field never changes the
/// content hash of a definition that does not set it).
macro_rules! is_default {
    ($name:ident, $ty:ty, $value:expr) => {
        fn $name(v: &$ty) -> bool {
            *v == $value
        }
    };
}

is_default!(pitch_is_default, (f32, f32), (-0.45, 1.35));
is_default!(release_factor_is_default, f32, 1.3);
is_default!(snap_distance_is_default, f32, 12.0);
is_default!(body_snap_distance_is_default, f32, 4.0);
is_default!(yaw_rate_is_default, f32, 7.0);
is_default!(pitch_rate_is_default, f32, 7.0);
is_default!(pitch_clamp_is_default, (f32, f32), (0.15, 0.7));
is_default!(zero_f32, f32, 0.0);
is_default!(radius_is_default, f32, 0.25);
is_default!(arm_min_is_default, f32, 0.7);
is_default!(mouse_threshold_is_default, f32, 14.0);
is_default!(cooldown_is_default, f32, 0.35);
is_default!(stick_threshold_is_default, f32, 0.75);
is_default!(trauma_decay_is_default, f32, 1.4);
is_default!(shift_is_default, f32, 0.12);
is_default!(pitch_deg_is_default, f32, 2.0);
is_default!(yaw_deg_is_default, f32, 2.0);
is_default!(roll_deg_is_default, f32, 3.0);
is_default!(fit_margin_is_default, f32, 0.15);
is_default!(tilt_max_is_default, f32, 0.6);
is_default!(fit_rate_is_default, f32, 6.0);
is_default!(delay_is_default, f32, 1.5);
is_default!(centre_rate_is_default, f32, 2.5);
is_default!(min_speed_is_default, f32, 0.5);
is_default!(max_turn_is_default, f32, 150.0);

fn is_false(v: &bool) -> bool {
    !*v
}

/// How a camera locked on to a target turns to frame it. Today's behaviour is kept: the camera
/// goes behind the line from the target to the camera's focus, and the pitch follows that line
/// too (clamped to `pitch_clamp`) unless `pitch_target` names a fixed one.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct FramingTuning {
    /// How fast the yaw catches up with the framing (1/s, exact exponential).
    #[serde(skip_serializing_if = "yaw_rate_is_default")]
    pub yaw_rate: f32,
    /// How fast the pitch catches up (1/s).
    #[serde(skip_serializing_if = "pitch_rate_is_default")]
    pub pitch_rate: f32,
    /// A fixed pitch to frame at (radians, positive looks down). `None`: the pitch of the line
    /// from the target to the focus, within `pitch_clamp`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_target: Option<f32>,
    /// The range the geometric pitch is held to when `pitch_target` is `None` (radians).
    #[serde(skip_serializing_if = "pitch_clamp_is_default")]
    pub pitch_clamp: (f32, f32),
    /// Nearer than this to the target (m, on the ground plane, from the focus) the camera stops
    /// framing: the direction to it is not defined. 0: always frames.
    #[serde(skip_serializing_if = "zero_f32")]
    pub min_horizontal: f32,
    /// Keeping the player and the locked target in frame together (R4). `None` (the default):
    /// the camera looks at the focus, and a target high on a big body may leave the top of
    /// the frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitTuning>,
}

impl Default for FramingTuning {
    fn default() -> Self {
        Self { yaw_rate: 7.0, pitch_rate: 7.0, pitch_target: None, pitch_clamp: (0.15, 0.7), min_horizontal: 0.0, fit: None }
    }
}

/// The view of a camera locked on to a target high on a big body (R4). The camera still
/// stands where the framing puts it, but the view turns up from the focus as far as it
/// takes to keep the target inside the frame's top margin, and never so far that the
/// player's feet leave the bottom margin: when both cannot be kept, the player is. Never
/// down: a target below the focus is framed as before.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct FitTuning {
    /// How far inside the top and bottom edges the target and the feet are kept, as a share
    /// of half the frame's height (0 at the edge, below 1).
    #[serde(skip_serializing_if = "fit_margin_is_default")]
    pub margin: f32,
    /// The most the view turns up from the focus (radians).
    #[serde(skip_serializing_if = "tilt_max_is_default")]
    pub tilt_max: f32,
    /// How fast the turn catches up, and goes back to 0 after the lock (1/s, exact exponential).
    #[serde(skip_serializing_if = "fit_rate_is_default")]
    pub rate: f32,
}

impl Default for FitTuning {
    fn default() -> Self {
        Self { margin: 0.15, tilt_max: 0.6, rate: 6.0 }
    }
}

/// How a free-look camera turns back behind the body it follows when the player leaves the look
/// alone and walks (K2). Only while the player's `controls.camera_auto_centre` is on
/// (`CameraSettings::auto_centre`); a lock frames as it does.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct AutoCentreTuning {
    /// How long the look must be left alone first (real seconds).
    #[serde(skip_serializing_if = "delay_is_default")]
    pub delay_s: f32,
    /// How fast the camera turns behind the body (1/s, exact exponential).
    #[serde(skip_serializing_if = "centre_rate_is_default")]
    pub rate: f32,
    /// The body must move at least this fast on the ground (m/s, as it is drawn): one standing
    /// still keeps the view the player left.
    #[serde(skip_serializing_if = "min_speed_is_default")]
    pub min_speed: f32,
    /// A body facing more than this from the camera's forward (degrees) does not swing it round:
    /// one walking toward the camera keeps the view.
    #[serde(skip_serializing_if = "max_turn_is_default")]
    pub max_turn_deg: f32,
    /// A tilt to ease toward with the yaw (radians, positive looks down). `None`: the pitch is
    /// left as the player set it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f32>,
}

impl Default for AutoCentreTuning {
    fn default() -> Self {
        Self { delay_s: 1.5, rate: 2.5, min_speed: 0.5, max_turn_deg: 150.0, pitch: None }
    }
}

/// The camera's arm: how far it stands from the focus and how a wall pulls it in.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct ArmTuning {
    /// The camera sphere a wall pulls in (m).
    #[serde(skip_serializing_if = "radius_is_default")]
    pub radius: f32,
    /// Never closer to the focus than this (m), even with a wall right behind.
    #[serde(skip_serializing_if = "arm_min_is_default")]
    pub min_distance: f32,
    /// How fast the arm grows back after a wall let go of it, and how fast its nominal length
    /// follows `ThirdPersonCamera::distance` when a game changes that (1/s, exact exponential).
    /// The arm always shortens at once. `None`: it grows back at once too (the camera never
    /// smoothed this before).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_rate: Option<f32>,
}

impl Default for ArmTuning {
    fn default() -> Self {
        Self { radius: 0.25, min_distance: 0.7, follow_rate: None }
    }
}

/// Switching the lock-on target with a look flick. The mouse and the right stick have their own
/// rules; both share the cooldown.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct FlickTuning {
    /// Horizontal mouse motion (pixels, summed) that switches the target. The default is the
    /// old threshold (0.035 rad) at the standard map's 0.0025 rad per pixel.
    #[serde(skip_serializing_if = "mouse_threshold_is_default")]
    pub mouse_threshold_px: f32,
    /// How fast the sum drains (px/s). `None`: it does not carry over from one frame to the
    /// next, so only one frame's motion counts (how the camera behaved before).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse_decay_px_s: Option<f32>,
    /// Seconds after a switch before the next one, on the simulation clock (`Time<Virtual>`:
    /// a paused game does not run it down, a slowed one runs it down slowly).
    #[serde(skip_serializing_if = "cooldown_is_default")]
    pub cooldown_s: f32,
    /// Whether a flick that found nobody to switch to starts the cooldown too.
    #[serde(skip_serializing_if = "is_false")]
    pub cooldown_on_miss: bool,
    /// Right-stick deflection (0..1, after the dead zone) that switches the target. The default
    /// is what the old threshold came to at the standard map at 60 Hz.
    #[serde(skip_serializing_if = "stick_threshold_is_default")]
    pub stick_threshold: f32,
    /// The stick must come back below this deflection before it can switch again. `None`: it
    /// can switch again as soon as the cooldown ends, held over or not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stick_rearm: Option<f32>,
}

impl Default for FlickTuning {
    fn default() -> Self {
        Self {
            mouse_threshold_px: 14.0,
            mouse_decay_px_s: None,
            cooldown_s: 0.35,
            cooldown_on_miss: false,
            stick_threshold: 0.75,
            stick_rearm: None,
        }
    }
}

/// How a hit shakes the camera. `trauma` (0..1) drains linearly at `decay` per second; the
/// shake is `trauma²` times these amplitudes.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct TraumaTuning {
    /// Trauma drained per second.
    #[serde(skip_serializing_if = "trauma_decay_is_default")]
    pub decay: f32,
    /// The most the shake moves the camera at trauma 1 (m).
    #[serde(skip_serializing_if = "shift_is_default")]
    pub shift_m: f32,
    /// The most it tilts the view up and down (degrees).
    #[serde(skip_serializing_if = "pitch_deg_is_default")]
    pub pitch_deg: f32,
    /// The most it turns the view left and right (degrees).
    #[serde(skip_serializing_if = "yaw_deg_is_default")]
    pub yaw_deg: f32,
    /// The most it rolls the view (degrees).
    #[serde(skip_serializing_if = "roll_deg_is_default")]
    pub roll_deg: f32,
}

impl Default for TraumaTuning {
    fn default() -> Self {
        Self { decay: 1.4, shift_m: 0.12, pitch_deg: 2.0, yaw_deg: 2.0, roll_deg: 3.0 }
    }
}

/// The camera's numbers. Content: `<assets>/content/**/*.camera.ron`, one map of ids to
/// definitions; the camera reads the id `default` (hot-reloadable), and without one it runs on
/// the code defaults. The resource holds what is in force, so a game may also set it from code
/// (content that exists wins whenever it loads or changes).
///
/// ```ron
/// { "default": (pitch: (-1.1, 0.5), follow_rate: Some(12.0), arm: (follow_rate: Some(6.0))) }
/// ```
///
/// Missing fields take the defaults; unknown ones are an error.
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Resource, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CameraTuning {
    /// Lowest and highest tilt (radians, positive looks down from above). A game narrows them
    /// for a while through [`CameraOffsets`].
    #[serde(skip_serializing_if = "pitch_is_default")]
    pub pitch: (f32, f32),
    /// How fast the focus catches up with the followed character (1/s, exact exponential).
    /// `None`: each camera's own [`follow_rate`](super::ThirdPersonCamera::follow_rate).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_rate: Option<f32>,
    /// A lock-on lets go at this many times its range.
    #[serde(skip_serializing_if = "release_factor_is_default")]
    pub release_factor: f32,
    /// A followed character farther than this from the focus (m) is a jump (a teleport, a
    /// respawn): the focus lands on it at once instead of catching up.
    #[serde(skip_serializing_if = "snap_distance_is_default")]
    pub snap_distance: f32,
    /// A body that moved farther than this (m) in one tick was placed, not walked (a teleport, a
    /// respawn, a region change): the render interpolation ([`Interpolated`](crate::Interpolated))
    /// starts over there instead of drawing the body flying across.
    /// Presentation tuning only: it lives in the non-simulation `camera` kind (a headless game never
    /// reads it, so it is outside the replay content check), not in a character definition.
    #[serde(skip_serializing_if = "body_snap_distance_is_default")]
    pub body_snap_distance: f32,
    /// Framing a locked target.
    #[serde(skip_serializing_if = "FramingTuning::is_default")]
    pub framing: FramingTuning,
    /// The arm and the wall.
    #[serde(skip_serializing_if = "ArmTuning::is_default")]
    pub arm: ArmTuning,
    /// Look flicks that switch the lock-on target.
    #[serde(skip_serializing_if = "FlickTuning::is_default")]
    pub flick: FlickTuning,
    /// The shake.
    #[serde(skip_serializing_if = "TraumaTuning::is_default")]
    pub trauma: TraumaTuning,
    /// Turning back behind the body when the player leaves the look alone (K2).
    #[serde(skip_serializing_if = "AutoCentreTuning::is_default")]
    pub auto_centre: AutoCentreTuning,
}

impl Default for CameraTuning {
    fn default() -> Self {
        Self {
            pitch: (-0.45, 1.35),
            follow_rate: None,
            release_factor: 1.3,
            snap_distance: 12.0,
            body_snap_distance: 4.0,
            framing: FramingTuning::default(),
            arm: ArmTuning::default(),
            flick: FlickTuning::default(),
            trauma: TraumaTuning::default(),
            auto_centre: AutoCentreTuning::default(),
        }
    }
}

macro_rules! impl_is_default {
    ($($ty:ty),*) => {$(
        impl $ty {
            fn is_default(&self) -> bool {
                *self == Self::default()
            }
        }
    )*};
}
impl_is_default!(FramingTuning, ArmTuning, FlickTuning, TraumaTuning, AutoCentreTuning);

impl ContentDef for CameraTuning {
    const KIND: &'static str = "camera";
    // The camera is only a view: a window reads its tuning, a headless game never does.
    const SIMULATION: bool = false;

    fn validate(&self, _id: &str, refs: &mut Refs) {
        for problem in self.problems() {
            refs.error(problem);
        }
    }
}

impl CameraTuning {
    /// What is wrong with these numbers, one line each; empty when they are fine.
    pub fn problems(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let finite = |name: &str, v: f32, out: &mut Vec<String>| {
            if !v.is_finite() {
                out.push(format!("{name} is {v}, it must be a finite number"));
                return false;
            }
            true
        };
        let positive_rate = |name: &str, v: f32, out: &mut Vec<String>| {
            if finite(name, v, out) && v <= 0.0 {
                out.push(format!("{name} is {v}, a rate must be above 0 (leave it out for the default)"));
            }
        };
        let non_negative = |name: &str, v: f32, out: &mut Vec<String>| {
            if finite(name, v, out) && v < 0.0 {
                out.push(format!("{name} is {v}, it cannot be negative"));
            }
        };
        // Straight up or down is where the orbit's yaw stops meaning anything.
        let limit = std::f32::consts::FRAC_PI_2 - 0.02;
        let (lo, hi) = self.pitch;
        let lo_ok = finite("pitch.0", lo, &mut out);
        let hi_ok = finite("pitch.1", hi, &mut out);
        if lo_ok && hi_ok {
            if lo >= hi {
                out.push(format!("pitch ({lo}, {hi}) has its lowest tilt at or above its highest"));
            }
            if lo < -limit || hi > limit {
                out.push(format!(
                    "pitch ({lo}, {hi}) must stay within ±{limit:.2} radians (looking straight up or down breaks the orbit)"
                ));
            }
        }
        if let Some(rate) = self.follow_rate {
            positive_rate("follow_rate", rate, &mut out);
        }
        if finite("release_factor", self.release_factor, &mut out) && self.release_factor < 1.0 {
            out.push(format!("release_factor is {}, below 1 a lock would let go before it could pick", self.release_factor));
        }
        if finite("snap_distance", self.snap_distance, &mut out) && self.snap_distance <= 0.0 {
            out.push(format!("snap_distance is {}, it must be above 0", self.snap_distance));
        }
        if finite("body_snap_distance", self.body_snap_distance, &mut out) && self.body_snap_distance <= 0.0 {
            out.push(format!("body_snap_distance is {}, it must be above 0", self.body_snap_distance));
        }
        let framing = &self.framing;
        positive_rate("framing.yaw_rate", framing.yaw_rate, &mut out);
        positive_rate("framing.pitch_rate", framing.pitch_rate, &mut out);
        non_negative("framing.min_horizontal", framing.min_horizontal, &mut out);
        if let Some(target) = framing.pitch_target
            && finite("framing.pitch_target", target, &mut out)
            && (target < lo || target > hi)
        {
            out.push(format!("framing.pitch_target {target} is outside the pitch limits ({lo}, {hi})"));
        }
        let (clamp_lo, clamp_hi) = framing.pitch_clamp;
        let clamp_ok = finite("framing.pitch_clamp.0", clamp_lo, &mut out) & finite("framing.pitch_clamp.1", clamp_hi, &mut out);
        if clamp_ok && clamp_lo > clamp_hi {
            out.push(format!("framing.pitch_clamp ({clamp_lo}, {clamp_hi}) has its low end above its high end"));
        }
        if let Some(fit) = &framing.fit {
            if finite("framing.fit.margin", fit.margin, &mut out) && !(0.0..1.0).contains(&fit.margin) {
                out.push(format!("framing.fit.margin {} must be in [0, 1): a share of half the frame's height", fit.margin));
            }
            if finite("framing.fit.tilt_max", fit.tilt_max, &mut out) && !(0.0..=limit).contains(&fit.tilt_max) {
                out.push(format!("framing.fit.tilt_max {} must be within [0, {limit:.2}] radians", fit.tilt_max));
            }
            positive_rate("framing.fit.rate", fit.rate, &mut out);
        }
        // A clamp wider than the pitch limits is fine: the framed pitch is held to them afterwards,
        // like any pitch, so narrowing `pitch` does not also need a narrower clamp.
        let arm = &self.arm;
        non_negative("arm.radius", arm.radius, &mut out);
        non_negative("arm.min_distance", arm.min_distance, &mut out);
        if let Some(rate) = arm.follow_rate {
            positive_rate("arm.follow_rate", rate, &mut out);
        }
        let flick = &self.flick;
        positive_rate("flick.mouse_threshold_px", flick.mouse_threshold_px, &mut out);
        if let Some(decay) = flick.mouse_decay_px_s {
            positive_rate("flick.mouse_decay_px_s", decay, &mut out);
        }
        non_negative("flick.cooldown_s", flick.cooldown_s, &mut out);
        if finite("flick.stick_threshold", flick.stick_threshold, &mut out) && !(flick.stick_threshold > 0.0 && flick.stick_threshold <= 1.0) {
            out.push(format!("flick.stick_threshold {} must be in (0, 1]: it is a stick deflection", flick.stick_threshold));
        }
        if let Some(rearm) = flick.stick_rearm
            && finite("flick.stick_rearm", rearm, &mut out)
            && (rearm <= 0.0 || rearm >= flick.stick_threshold)
        {
            out.push(format!(
                "flick.stick_rearm {rearm} must be above 0 and below flick.stick_threshold {}: the stick has to come back before it can switch again",
                flick.stick_threshold
            ));
        }
        let trauma = &self.trauma;
        non_negative("trauma.decay", trauma.decay, &mut out);
        non_negative("trauma.shift_m", trauma.shift_m, &mut out);
        for (name, v) in [("pitch_deg", trauma.pitch_deg), ("yaw_deg", trauma.yaw_deg), ("roll_deg", trauma.roll_deg)] {
            non_negative(&format!("trauma.{name}"), v, &mut out);
            if v > 45.0 {
                out.push(format!("trauma.{name} is {v} degrees, more than 45 is not a shake"));
            }
        }
        let centre = &self.auto_centre;
        non_negative("auto_centre.delay_s", centre.delay_s, &mut out);
        positive_rate("auto_centre.rate", centre.rate, &mut out);
        non_negative("auto_centre.min_speed", centre.min_speed, &mut out);
        if finite("auto_centre.max_turn_deg", centre.max_turn_deg, &mut out) && !(0.0..=180.0).contains(&centre.max_turn_deg) {
            out.push(format!("auto_centre.max_turn_deg {} must be within [0, 180] degrees", centre.max_turn_deg));
        }
        if let Some(pitch) = centre.pitch
            && finite("auto_centre.pitch", pitch, &mut out)
            && (pitch < lo || pitch > hi)
        {
            out.push(format!("auto_centre.pitch {pitch} is outside the pitch limits ({lo}, {hi})"));
        }
        out
    }
}

/// What a game's own systems add to the camera's base values, read by `orbit_camera` after
/// them, every frame.
///
/// **Who owns a value.** The system that writes a field owns it: it sets the value on every
/// frame it wants an effect, and puts it back to the neutral one (`0.0`, `None`, `false`)
/// itself when it is done. The camera only reads: it never decays, sums or resets an offset, so
/// a value stays as its writer left it (a writer that stops without resetting leaves its effect on),
/// and two systems that both want the same field must be combined by the game into one
/// writer (add their parts, then write once).
/// Write before `CameraSystems::Orbit` (`PostUpdate`) to be seen the same frame.
///
/// Offsets are presentation: they change the picture only, never the stored `yaw`/`pitch`
/// (apart from the pitch limits, which clamp the stored pitch as a swimming camera needs, and
/// `aiming`, which sets how fast the look turns them) and never `ThirdPersonCamera::focus`.
#[derive(Resource, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Resource, Default)]
pub struct CameraOffsets {
    /// Added to the field of view (degrees), after the player's setting and before the
    /// engine's 30..120 limit. Sprint widening, a finisher's punch.
    pub fov_add_deg: f32,
    /// How far the whole view is lowered (m; negative raises it): the camera and the point it
    /// looks at move together, so the picture drops without tilting. A landing dip.
    pub focus_dip_m: f32,
    /// A higher lowest tilt (radians) than the tuning's, for a while. Anything outside the
    /// tuning's range is held to it, so an override only narrows.
    pub pitch_min: Option<f32>,
    /// A lower highest tilt (radians) than the tuning's: a swimming camera that must stay above
    /// the water. The stored pitch is clamped to it.
    pub pitch_max: Option<f32>,
    /// The player aims (a ranged attack, a zoom: what aiming is, is the game's): free look turns
    /// by the player's `aim_sensitivity` instead of `sensitivity` (K1). A lock frames as it does.
    pub aiming: bool,
}

impl CameraOffsets {
    /// Nothing added.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(tuning: &CameraTuning) -> Vec<String> {
        tuning.problems()
    }

    #[test]
    fn the_defaults_are_the_old_constants_and_validate() {
        let t = CameraTuning::default();
        assert_eq!(t.pitch, (-0.45, 1.35));
        assert_eq!((t.release_factor, t.framing.yaw_rate, t.framing.pitch_rate), (1.3, 7.0, 7.0));
        assert_eq!((t.arm.radius, t.arm.min_distance), (0.25, 0.7));
        assert_eq!((t.flick.cooldown_s, t.trauma.decay), (0.35, 1.4));
        assert_eq!((t.auto_centre.delay_s, t.auto_centre.rate, t.auto_centre.max_turn_deg), (1.5, 2.5, 150.0));
        assert!(errors(&t).is_empty(), "{:?}", errors(&t));
    }

    #[test]
    fn a_default_serializes_to_nothing_and_an_empty_file_is_the_default() {
        let text = ron::to_string(&CameraTuning::default()).unwrap();
        assert_eq!(text, "()");
        assert_eq!(ron::from_str::<CameraTuning>("()").unwrap(), CameraTuning::default());
        let set: CameraTuning = ron::from_str("(pitch: (-1.1, 0.5), arm: (follow_rate: Some(6.0)), flick: (stick_rearm: Some(0.3)))").unwrap();
        assert_eq!(set.pitch, (-1.1, 0.5));
        assert_eq!(set.arm.follow_rate, Some(6.0));
        assert_eq!(set.arm.radius, 0.25, "what is not set stays");
        assert_eq!(ron::from_str::<CameraTuning>(&ron::to_string(&set).unwrap()).unwrap(), set);
        assert!(ron::from_str::<CameraTuning>("(pich: (0.0, 1.0))").is_err(), "a typo is an error");
    }

    #[test]
    fn validation_names_what_is_out_of_range() {
        let bad = |edit: fn(&mut CameraTuning)| {
            let mut t = CameraTuning::default();
            edit(&mut t);
            errors(&t)
        };
        assert!(bad(|t| t.pitch = (0.5, -0.5))[0].contains("pitch"));
        assert!(bad(|t| t.pitch = (-0.5, 1.6))[0].contains("straight up"));
        assert!(bad(|t| t.framing.yaw_rate = 0.0)[0].contains("framing.yaw_rate"));
        assert!(bad(|t| t.framing.pitch_target = Some(1.4)).iter().any(|e| e.contains("pitch_target")));
        assert!(bad(|t| t.arm.follow_rate = Some(-1.0))[0].contains("arm.follow_rate"));
        assert!(bad(|t| t.arm.min_distance = -0.1)[0].contains("arm.min_distance"));
        assert!(bad(|t| t.flick.stick_rearm = Some(0.9))[0].contains("stick_rearm"));
        assert!(bad(|t| t.flick.stick_threshold = 1.5)[0].contains("stick_threshold"));
        assert!(bad(|t| t.release_factor = 0.5)[0].contains("release_factor"));
        assert!(bad(|t| t.snap_distance = 0.0)[0].contains("snap_distance"));
        assert!(bad(|t| t.body_snap_distance = 0.0)[0].contains("body_snap_distance"));
        assert!(bad(|t| t.framing.pitch_clamp = (0.7, 0.15))[0].contains("pitch_clamp"));
        assert!(bad(|t| t.trauma.roll_deg = 90.0)[0].contains("roll_deg"));
        assert!(bad(|t| t.flick.mouse_decay_px_s = Some(f32::NAN))[0].contains("finite"));
        assert!(bad(|t| t.auto_centre.rate = 0.0)[0].contains("auto_centre.rate"));
        assert!(bad(|t| t.auto_centre.delay_s = -1.0)[0].contains("auto_centre.delay_s"));
        assert!(bad(|t| t.auto_centre.max_turn_deg = 200.0)[0].contains("auto_centre.max_turn_deg"));
        assert!(bad(|t| t.auto_centre.pitch = Some(1.5))[0].contains("auto_centre.pitch 1.5 is outside the pitch limits"));
    }
}
