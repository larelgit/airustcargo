//! [`CharacterParams`]: every number the controller uses, as content (kind `locomotion`,
//! hot-reloadable) that a [`CharacterController`] names.

use std::sync::LazyLock;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};

use super::{CharacterController, MotionMode};
use crate::physics::{Collider, CollisionLayers, Shape};

/// How far above `mantle_height` a ledge may stand and still be climbed (m): float noise.
pub(crate) const MANTLE_SLACK: f32 = 1e-3;
/// A jump in water climbs out onto a ledge this close in front of the body (m), unless the
/// params set `climb_out_reach`.
pub(crate) const DEFAULT_CLIMB_OUT_REACH: f32 = 0.3;

/// When a sprint that ran the stamina dry may start again. The rule is a content choice per kind
/// of sprint (`CombatTuning::sprint_resume` on land, [`CharacterParams::swim_fast_resume`] in water).
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub enum SprintResume {
    /// The sprint button must be let go once; then it works again as soon as there is any stamina
    /// (the default, and the only rule before this option existed).
    #[default]
    OnRelease,
    /// Back once the stamina is at least this fraction of its maximum, with the button held or
    /// not; letting go does not end the lock. `Fraction(0.25)`: a lock that opens at a quarter.
    /// `Fraction(0.0)`: no lock at all, the sprint stops at an empty bar and goes on as soon as
    /// there is any (how some action games treat a sprint on land).
    Fraction(f32),
}

impl SprintResume {
    /// Whether an empty bar sets the lock this rule consults (`Fraction(0)` has none).
    pub fn latches(self) -> bool {
        !matches!(self, SprintResume::Fraction(f) if f <= 0.0)
    }
}

/// How a character moves. Distances in meters, times in seconds, speeds in m/s.
/// Missing fields in a file take the defaults; unknown ones are an error, so a typo
/// (`walk_spead`) is reported with its file and line instead of silently doing nothing.
///
/// Content: `<assets>/content/**/*.locomotion.ron`, one map of ids to definitions
/// (`CharacterController { params: "hero" }` picks `"hero"`):
///
/// ```ron
/// { "hero": (walk_speed: 2.0, run_speed: 5.5, jump_height: 1.3) }
/// ```
///
/// The same struct is also a component: one on a character wins over the content id
/// (tests, one-off creatures).
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CharacterParams {
    /// Capsule radius.
    pub radius: f32,
    /// Capsule height, feet to the top of the head.
    pub height: f32,
    /// Ground speed at `move_dir` length 0.5; slower inputs scale it down.
    pub walk_speed: f32,
    /// Ground speed at `move_dir` length 1.
    pub run_speed: f32,
    /// Ground speed while `sprint` is held and the input is above walking.
    pub sprint_speed: f32,
    /// How fast speed builds up toward the input (m/s²).
    pub acceleration: f32,
    /// How fast speed drops when the input eases off or stops (m/s²).
    pub deceleration: f32,
    /// Steepest walkable slope (degrees). Steeper surfaces are walls: no walking up
    /// them, and a character standing on one slides down.
    pub max_slope_deg: f32,
    /// Highest obstacle walked onto without jumping (stairs, curbs).
    pub step_height: f32,
    /// How far the character is pulled down to stay on the ground: stairs down,
    /// slopes, small drops.
    pub snap_distance: f32,
    /// Gap kept between the capsule and the world, so every cast starts outside
    /// geometry. Feet rest this far above the ground.
    pub skin_width: f32,
    /// Downward acceleration (m/s²).
    pub gravity: f32,
    /// Apex of a jump above the take-off height.
    pub jump_height: f32,
    /// After walking off a ledge, a jump still works for this long.
    pub coyote_time: f32,
    /// A jump pressed this long before it is possible (e.g. just before landing)
    /// still happens.
    pub jump_buffer: f32,
    /// Fraction of `acceleration` available in the air.
    pub air_control: f32,
    /// Fastest fall.
    pub terminal_speed: f32,
    /// Highest ledge top (above the feet) the character climbs onto.
    pub mantle_height: f32,
    /// How long climbing onto a ledge takes.
    pub mantle_time: f32,
    /// Water deeper than this makes the character swim.
    pub swim_depth: f32,
    /// While swimming, the feet float this far below the surface. Less than
    /// `swim_depth`, so a swimmer reaching the shallows stands up.
    pub float_depth: f32,
    /// Swimming speed; sprint does nothing in water.
    pub swim_speed: f32,
    /// Fastest fall while gliding.
    pub glide_fall_speed: f32,
    /// Horizontal cruising speed of a glide.
    pub glide_speed: f32,
    /// How fast a glide turns and speeds up toward `glide_speed` (m/s²).
    pub glide_accel: f32,
    /// Turning speed (degrees per second); 0 turns instantly.
    pub turn_speed_deg: f32,
    /// Swimming speed while `sprint` is held and the input is not zero (a fast swim). 0: sprint
    /// does nothing in water. It costs [`swim_fast_drain`](Self::swim_fast_drain) stamina per
    /// second (a fighter with `Stamina`), ends when there is none, and shows as
    /// `MotionState::sprinting`.
    #[serde(skip_serializing_if = "is_zero")]
    pub swim_fast_speed: f32,
    /// Stamina per second a fast swim costs. Not scaled by `MoveModifiers::sprint_cost`.
    #[serde(skip_serializing_if = "is_zero")]
    pub swim_fast_drain: f32,
    /// How fast a swimmer speeds up toward its input (m/s²). 0: `acceleration`.
    #[serde(skip_serializing_if = "is_zero")]
    pub swim_acceleration: f32,
    /// How fast a swimmer slows with no input (m/s²). 0: the same as `swim_acceleration`
    /// (or `acceleration`).
    #[serde(skip_serializing_if = "is_zero")]
    pub swim_deceleration: f32,
    /// Turning speed of a swimmer (degrees per second). 0: `turn_speed_deg`.
    #[serde(skip_serializing_if = "is_zero")]
    pub swim_turn_speed_deg: f32,
    /// Two-tier climb-out: a swimmer pushing against a ledge climbs onto it by itself when the
    /// ledge top is at most this high **above the water surface** (m); a higher ledge (up to
    /// `mantle_height` above the feet) needs a jump. `None`: every ledge `mantle_height` allows
    /// is climbed by pushing, as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub climb_out_auto_height: Option<f32>,
    /// Jump climb-out limit **above the water surface** (m): a swimmer that jumps at a ledge
    /// climbs onto it when the ledge top is at most this far over the surface, however deep its
    /// feet float; the probe reaches that high too. `None`: the jump uses `mantle_height` above
    /// the feet like any climb (so only `mantle_height - float_depth` over the surface is
    /// reachable). A game that wants a window like "up to 1.3 m over the water" sets this; the
    /// land climb keeps `mantle_height`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub climb_out_max_height: Option<f32>,
    /// Horizontal reach of the water climb-out (m): a swimmer that jumps at a ledge climbs onto
    /// it when the ledge face is within this distance in front of the body. `None`: 0.3 m, as
    /// before. A game whose swimmers climb out of ledges half a meter away sets it to that
    /// distance; the `jump` tool and `clearance` ask the same number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub climb_out_reach: Option<f32>,
    /// Fast swim: when it may start again after the stamina ran dry. See [`SprintResume`]; the
    /// land sprint has its own rule in the combat tuning.
    #[serde(skip_serializing_if = "is_on_release")]
    pub swim_fast_resume: SprintResume,
    /// Wading: from this depth of water over the feet (m) the walk slows down. Needs
    /// `wade_factor` below 1.
    #[serde(skip_serializing_if = "is_zero")]
    pub wade_min_depth: f32,
    /// Wading: at this depth and deeper the slowdown is the whole `wade_factor`; between
    /// `wade_min_depth` and here it grows in a straight line from 1.
    #[serde(skip_serializing_if = "is_zero")]
    pub wade_full_depth: f32,
    /// Wading: the speed factor at `wade_full_depth`. 1: wading does not slow (the default).
    #[serde(skip_serializing_if = "is_one")]
    pub wade_factor: f32,
    /// Wading: the depth is measured this far (m) below the feet (`surface - feet + offset`), for
    /// a game whose rules read submersion at a reference point under the soles. The value to use
    /// is the gap from that point to the floor of a standing body: its distance below the capsule
    /// bottom plus `skin_width` (the feet rest a skin above the floor). A point 0.1 m under the
    /// capsule bottom with the default skin is `0.12`. Only wading reads it, not swimming,
    /// floating or landing. 0: the depth over the feet.
    #[serde(skip_serializing_if = "is_zero")]
    pub wade_depth_offset: f32,
    /// Wading: the offset while airborne or gliding (with `wade_in_air`), when it differs from
    /// the standing one. A reference point fixed under the capsule bottom is `skin_width`
    /// closer to the feet in the air than for a standing body (the feet rest a skin above the
    /// floor only on the ground), so the in-air value is `wade_depth_offset - skin_width`.
    /// `None`: `wade_depth_offset` in every mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wade_air_depth_offset: Option<f32>,
    /// Wading also slows a body in the air or gliding over water (the water is over the feet, not
    /// on the ground). `false`: only on the ground.
    #[serde(skip_serializing_if = "is_false")]
    pub wade_in_air: bool,
    /// Sprint speed grows with the input like a walk does: `sprint_speed * amount`, and any input
    /// above zero is a sprint (not only above walking). `false`: full sprint speed above 0.5.
    #[serde(skip_serializing_if = "is_false")]
    pub sprint_scales_with_input: bool,
    /// A speed cap (`MoveModifiers::cap` and `unsprinted_cap`) is the speed at full input, and
    /// slower inputs scale with it (a cap of 4.5 on a run of 5.0 makes a half-stick walk 2.25),
    /// like an analog stick against a lower top speed. `false`: the cap is an absolute ceiling.
    #[serde(skip_serializing_if = "is_false")]
    pub caps_scale_with_input: bool,
    /// Horizontal speed changes by the acceleration step per axis (x and z each move toward their
    /// own target by at most the step) instead of along the straight line to the target velocity.
    #[serde(skip_serializing_if = "is_false")]
    pub axis_stepping: bool,
    /// `deceleration` applies only when there is no movement input; with input the speed always
    /// changes at `acceleration`, even toward a slower target. `false`: deceleration whenever the
    /// target is slower than the current speed.
    #[serde(skip_serializing_if = "is_false")]
    pub brake_only_without_input: bool,
    /// Horizontal braking in the air with no input (m/s²). 0: the air keeps its horizontal speed.
    /// `air_control` scales the speed-up with input; this is the slow-down without it.
    #[serde(skip_serializing_if = "is_zero")]
    pub air_deceleration: f32,
    /// Sprint is decided every tick in the air too: pressing sprint in the air starts the sprint
    /// (speed and stamina drain) and letting go stops it, and a sprint that was on at take-off
    /// ends with the button. A glide is no sprint. `false`: the sprint flag is decided on the
    /// ground and in the water only, so a take-off sprint lasts the whole jump.
    #[serde(skip_serializing_if = "is_false")]
    pub sprint_in_air: bool,
    /// Landing tiers after a jump: impact speeds (m/s, ascending). `Landed::tier` is how many of
    /// them the impact reached (`[4.0, 6.5, 13.0]`: 0 below 4, 3 from 13). Empty (with
    /// `landing_drop_tiers`): tier 0 always, and `Landed::jumped` is not tracked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub landing_tiers: Vec<f32>,
    /// The same for a fall that did not follow a jump (a ledge walked off). Empty: `landing_tiers`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub landing_drop_tiers: Vec<f32>,
    /// No `Landed` while the feet stand in water deeper than this (m): the shallows break the
    /// fall. `None`: every landing is reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub landing_wade_limit: Option<f32>,
}

impl Default for CharacterParams {
    fn default() -> Self {
        Self {
            radius: 0.4,
            height: 1.8,
            walk_speed: 2.0,
            run_speed: 5.0,
            sprint_speed: 8.0,
            acceleration: 40.0,
            deceleration: 50.0,
            max_slope_deg: 45.0,
            step_height: 0.35,
            snap_distance: 0.35,
            skin_width: 0.02,
            gravity: 20.0,
            jump_height: 1.2,
            coyote_time: 0.12,
            jump_buffer: 0.12,
            air_control: 0.35,
            terminal_speed: 40.0,
            mantle_height: 1.6,
            mantle_time: 0.4,
            swim_depth: 1.3,
            float_depth: 1.1,
            swim_speed: 3.0,
            glide_fall_speed: 2.0,
            glide_speed: 9.0,
            glide_accel: 8.0,
            turn_speed_deg: 720.0,
            swim_fast_speed: 0.0,
            swim_fast_drain: 0.0,
            swim_acceleration: 0.0,
            swim_deceleration: 0.0,
            swim_turn_speed_deg: 0.0,
            climb_out_auto_height: None,
            climb_out_max_height: None,
            climb_out_reach: None,
            swim_fast_resume: SprintResume::OnRelease,
            wade_min_depth: 0.0,
            wade_full_depth: 0.0,
            wade_factor: 1.0,
            wade_depth_offset: 0.0,
            wade_air_depth_offset: None,
            wade_in_air: false,
            sprint_scales_with_input: false,
            caps_scale_with_input: false,
            axis_stepping: false,
            brake_only_without_input: false,
            air_deceleration: 0.0,
            sprint_in_air: false,
            landing_tiers: Vec::new(),
            landing_drop_tiers: Vec::new(),
            landing_wade_limit: None,
        }
    }
}

impl ContentDef for CharacterParams {
    const KIND: &'static str = "locomotion";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if let Err(problems) = self.check() {
            refs.error(problems);
        }
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_on_release(v: &SprintResume) -> bool {
    *v == SprintResume::OnRelease
}

/// What a character without content moves by: the code defaults.
static DEFAULTS: LazyLock<CharacterParams> = LazyLock::new(CharacterParams::default);

impl CharacterController {
    /// The numbers this character moves by: its own [`CharacterParams`] component, else the
    /// `locomotion` definition named by [`params`](Self::params) (read from the live
    /// database, so a hot reload applies on the next tick), else the code defaults.
    pub fn resolve<'a>(&self, own: Option<&'a CharacterParams>, db: Option<&'a ContentDb>) -> &'a CharacterParams {
        own.or_else(|| db.and_then(|db| db.get::<CharacterParams>(&self.params))).unwrap_or(&DEFAULTS)
    }

    /// A named definition the database does not have: the character runs on the code
    /// defaults, which is worth saying once.
    pub(super) fn is_dangling(&self, own: Option<&CharacterParams>, db: Option<&ContentDb>) -> bool {
        own.is_none()
            && !self.params.is_empty()
            && db.is_some_and(|db| db.is_loaded() && db.get::<CharacterParams>(&self.params).is_none())
    }
}

impl CharacterParams {
    /// The body these params describe: a capsule standing on the entity's origin.
    pub fn collider(&self) -> Collider {
        Collider::new(Shape::capsule(self.radius, self.height))
            .with_offset(Vec3::Y * self.height * 0.5)
            .with_layers(CollisionLayers::CHARACTER)
    }

    /// The gap the capsule keeps from the world: `skin_width`, as far as the capsule can carry it.
    pub fn skin(&self) -> f32 {
        self.skin_width.clamp(0.001, self.radius.max(0.01) * 0.5)
    }

    /// Whether a ledge whose standing surface is `height` above the feet can be climbed. The
    /// controller's rule, and the only one: the planner, `clearance` and the `jump` tool
    /// ask [`can_climb`](Self::can_climb), which turns the height they see into this one.
    pub fn can_mantle(&self, height: f32) -> bool {
        height <= self.mantle_height + MANTLE_SLACK
    }

    /// [`can_mantle`](Self::can_mantle) for a floor `rise` meters above the floor (or the
    /// water, when `swimming`) a character stands in. On the ground the feet rest a skin above
    /// the floor and the climb stands them a skin above the ledge, so the skins cancel; a
    /// swimmer floats at a height of its own and the second skin counts.
    pub fn can_climb(&self, rise: f32, swimming: bool) -> bool {
        self.can_mantle(if swimming { rise + self.skin() } else { rise })
    }

    /// Whether a swimmer's **jump** climbs onto a floor `rise` meters above the height its feet
    /// float at: [`climb_out_max_height`](Self::climb_out_max_height) above the surface when set
    /// (the feet float `float_depth` below it), else the ordinary [`can_climb`](Self::can_climb).
    /// The controller's rule for the jump, and the one the `jump` tool and `clearance` ask.
    pub fn can_jump_out(&self, rise: f32) -> bool {
        match self.climb_out_max_height {
            Some(limit) => rise - self.float_depth <= limit + MANTLE_SLACK,
            None => self.can_climb(rise, true),
        }
    }

    /// Initial upward speed that peaks at `jump_height`.
    pub fn jump_speed(&self) -> f32 {
        (2.0 * self.gravity.max(0.0) * self.jump_height.max(0.0)).sqrt()
    }

    /// Ground speed for an input of length `amount` (0..1), and whether it is a sprint.
    pub fn ground_speed(&self, amount: f32, sprint: bool) -> (f32, bool) {
        let amount = amount.clamp(0.0, 1.0);
        if self.sprint_scales_with_input {
            if sprint && amount > 0.0 {
                return (self.sprint_speed * amount, true);
            }
        } else if sprint && amount > 0.5 {
            return (self.sprint_speed, true);
        }
        if amount <= 0.5 {
            (self.walk_speed * amount * 2.0, false)
        } else {
            (self.walk_speed + (self.run_speed - self.walk_speed) * (amount - 0.5) * 2.0, false)
        }
    }

    /// The speed factor of wading in water `depth` meters over the feet: 1 up to
    /// `wade_min_depth`, `wade_factor` from `wade_full_depth`, a straight line between.
    pub fn wade_scale(&self, depth: f32) -> f32 {
        if self.wade_factor == 1.0 || depth <= self.wade_min_depth {
            return 1.0;
        }
        let span = self.wade_full_depth - self.wade_min_depth;
        let k = if span > 1e-6 { ((depth - self.wade_min_depth) / span).clamp(0.0, 1.0) } else { 1.0 };
        1.0 + (self.wade_factor - 1.0) * k
    }

    /// Whether water over the feet slows a body in `mode`: on the ground, and in the air or a
    /// glide with [`wade_in_air`](Self::wade_in_air). A swimmer is not a wader.
    pub fn wades_in(&self, mode: MotionMode) -> bool {
        self.wade_factor != 1.0
            && (mode == MotionMode::Ground
                || (self.wade_in_air && matches!(mode, MotionMode::Air | MotionMode::Glide)))
    }

    /// The depth wading reads for feet at height `feet_y` in water with this `surface`: the water
    /// over the feet plus [`wade_depth_offset`](Self::wade_depth_offset), never below 0.
    pub fn wade_depth(&self, surface: f32, feet_y: f32) -> f32 {
        self.wade_depth_for(MotionMode::Ground, surface, feet_y)
    }

    /// [`wade_depth`](Self::wade_depth) for a body in `mode`: an airborne or gliding body uses
    /// [`wade_air_depth_offset`](Self::wade_air_depth_offset) when the params have one.
    pub fn wade_depth_for(&self, mode: MotionMode, surface: f32, feet_y: f32) -> f32 {
        let offset = match mode {
            MotionMode::Air | MotionMode::Glide => self.wade_air_depth_offset.unwrap_or(self.wade_depth_offset),
            _ => self.wade_depth_offset,
        };
        if offset == 0.0 { (surface - feet_y).max(0.0) } else { (surface - feet_y + offset).max(0.0) }
    }

    /// How far in front of the body a water climb-out finds a ledge (m):
    /// [`climb_out_reach`](Self::climb_out_reach), else 0.3.
    pub fn climb_reach(&self) -> f32 {
        self.climb_out_reach.unwrap_or(DEFAULT_CLIMB_OUT_REACH)
    }

    /// Whether landings are told apart (any tier thresholds): the controller then tracks
    /// whether a fall followed a jump.
    pub fn tracks_landings(&self) -> bool {
        !self.landing_tiers.is_empty() || !self.landing_drop_tiers.is_empty()
    }

    /// The landing tier of an impact at `impact_speed` m/s: how many thresholds it reached.
    pub fn landing_tier(&self, impact_speed: f32, jumped: bool) -> u8 {
        let tiers =
            if !jumped && !self.landing_drop_tiers.is_empty() { &self.landing_drop_tiers } else { &self.landing_tiers };
        tiers.iter().filter(|t| impact_speed >= **t).count().min(u8::MAX as usize) as u8
    }

    /// Everything wrong with these numbers, so content errors name the field.
    pub fn check(&self) -> Result<(), String> {
        let mut errors = Vec::new();
        let positive = [
            ("radius", self.radius),
            ("height", self.height),
            ("gravity", self.gravity),
            ("skin_width", self.skin_width),
            ("mantle_time", self.mantle_time),
            ("terminal_speed", self.terminal_speed),
        ];
        for (name, v) in positive {
            if !(v > 0.0 && v.is_finite()) {
                errors.push(format!("{name} must be > 0 (is {v})"));
            }
        }
        if self.height < 2.0 * self.radius {
            errors.push(format!("height {} is less than the capsule's diameter {}", self.height, 2.0 * self.radius));
        }
        if !(0.0..90.0).contains(&self.max_slope_deg) {
            errors.push(format!("max_slope_deg must be in 0..90 (is {})", self.max_slope_deg));
        }
        if self.float_depth >= self.swim_depth {
            errors.push(format!(
                "float_depth {} must be less than swim_depth {}, or swimmers never stand up in the shallows",
                self.float_depth, self.swim_depth
            ));
        }
        if self.mantle_height < self.step_height {
            errors.push(format!("mantle_height {} is below step_height {}", self.mantle_height, self.step_height));
        }
        if self.skin_width >= self.radius * 0.5 {
            errors.push(format!("skin_width {} is too large for radius {}", self.skin_width, self.radius));
        }
        let non_negative = [
            ("swim_fast_speed", self.swim_fast_speed),
            ("swim_fast_drain", self.swim_fast_drain),
            ("swim_acceleration", self.swim_acceleration),
            ("swim_deceleration", self.swim_deceleration),
            ("swim_turn_speed_deg", self.swim_turn_speed_deg),
            ("wade_min_depth", self.wade_min_depth),
            ("wade_full_depth", self.wade_full_depth),
            ("wade_factor", self.wade_factor),
            ("wade_depth_offset", self.wade_depth_offset),
            ("air_deceleration", self.air_deceleration),
        ];
        for (name, v) in non_negative {
            if !(v >= 0.0 && v.is_finite()) {
                errors.push(format!("{name} must be >= 0 (is {v})"));
            }
        }
        if self.wade_factor != 1.0 && self.wade_full_depth < self.wade_min_depth {
            errors.push(format!(
                "wade_full_depth {} is below wade_min_depth {}",
                self.wade_full_depth, self.wade_min_depth
            ));
        }
        if self.wade_factor > 1.0 {
            errors.push(format!("wade_factor {} is above 1, wading would speed the body up", self.wade_factor));
        }
        if self.swim_fast_speed > 0.0 && self.swim_fast_speed < self.swim_speed {
            errors.push(format!("swim_fast_speed {} is below swim_speed {}", self.swim_fast_speed, self.swim_speed));
        }
        if self.swim_fast_drain > 0.0 && self.swim_fast_speed <= 0.0 {
            errors.push("swim_fast_drain is set but swim_fast_speed is 0, so the drain never applies".to_string());
        }
        if self.climb_out_auto_height.is_some_and(|h| h > self.mantle_height) {
            errors.push(format!(
                "climb_out_auto_height {:?} is above mantle_height {}, the second tier could never apply",
                self.climb_out_auto_height, self.mantle_height
            ));
        }
        if let (Some(auto), Some(max)) = (self.climb_out_auto_height, self.climb_out_max_height)
            && auto > max
        {
            errors.push(format!(
                "climb_out_auto_height {auto} is above climb_out_max_height {max}, the push would climb higher than the jump"
            ));
        }
        if let SprintResume::Fraction(f) = self.swim_fast_resume
            && !(0.0..=1.0).contains(&f)
        {
            errors.push(format!("swim_fast_resume Fraction({f}) must be between 0 and 1"));
        }
        for (name, tiers) in [("landing_tiers", &self.landing_tiers), ("landing_drop_tiers", &self.landing_drop_tiers)] {
            if tiers.iter().any(|t| !(*t > 0.0 && t.is_finite())) || tiers.windows(2).any(|w| w[0] >= w[1]) {
                errors.push(format!("{name} must be speeds > 0 in ascending order (is {tiers:?})"));
            }
        }
        for (name, v) in [
            ("climb_out_auto_height", self.climb_out_auto_height),
            ("climb_out_max_height", self.climb_out_max_height),
            ("climb_out_reach", self.climb_out_reach),
            ("wade_air_depth_offset", self.wade_air_depth_offset),
            ("landing_wade_limit", self.landing_wade_limit),
        ] {
            if v.is_some_and(|v| !(v >= 0.0 && v.is_finite())) {
                errors.push(format!("{name} must be >= 0 (is {v:?})"));
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("; ")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        CharacterParams::default().check().unwrap();
    }

    #[test]
    fn validation_names_the_field() {
        let p = CharacterParams { float_depth: 2.0, radius: -1.0, ..default() };
        let err = p.check().unwrap_err();
        assert!(err.contains("radius must be > 0") && err.contains("float_depth"), "{err}");
    }

    #[test]
    fn one_rule_says_what_can_be_climbed() {
        let p = CharacterParams::default();
        assert!(p.can_mantle(1.6) && !p.can_mantle(1.61), "mantle_height is the limit, with float noise to spare");
        assert!(p.can_climb(1.6, false), "on the ground the skins cancel");
        assert!(!p.can_climb(1.6, true) && p.can_climb(1.57, true), "a swimmer's second skin counts");
    }

    #[test]
    fn input_length_picks_walk_run_or_sprint() {
        let p = CharacterParams::default();
        assert_eq!(p.ground_speed(0.5, false), (p.walk_speed, false));
        assert_eq!(p.ground_speed(0.25, false), (p.walk_speed * 0.5, false));
        assert_eq!(p.ground_speed(1.0, false), (p.run_speed, false));
        assert_eq!(p.ground_speed(1.0, true), (p.sprint_speed, true));
        assert_eq!(p.ground_speed(0.4, true), (p.walk_speed * 0.8, false), "no sprint at walking pace");
    }
}
