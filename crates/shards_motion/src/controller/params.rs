//! [`CharacterParams`]: every number the controller uses, as content (kind `locomotion`,
//! hot-reloadable) that a [`CharacterController`] names.

use std::sync::LazyLock;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};

use super::CharacterController;
use crate::physics::{Collider, CollisionLayers, Shape};

/// How far above `mantle_height` a ledge may stand and still be climbed (m): float noise.
const MANTLE_SLACK: f32 = 1e-3;

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

    /// Initial upward speed that peaks at `jump_height`.
    pub fn jump_speed(&self) -> f32 {
        (2.0 * self.gravity.max(0.0) * self.jump_height.max(0.0)).sqrt()
    }

    /// Ground speed for an input of length `amount` (0..1), and whether it is a sprint.
    pub fn ground_speed(&self, amount: f32, sprint: bool) -> (f32, bool) {
        let amount = amount.clamp(0.0, 1.0);
        if sprint && amount > 0.5 {
            (self.sprint_speed, true)
        } else if amount <= 0.5 {
            (self.walk_speed * amount * 2.0, false)
        } else {
            (self.walk_speed + (self.run_speed - self.walk_speed) * (amount - 0.5) * 2.0, false)
        }
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
