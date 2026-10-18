//! Locomotion modifiers: what slows, caps, roots or frees a character this tick.
//!
//! [`MoveModifiers`] is **derived, stateless and recomputed every tick**: a reset puts it back to
//! neutral, then every source adds what it wants. Sources are systems in
//! [`ModifierSystems::Contribute`]: the engine's own (statuses with a `speed_factor`, `speed_cap`
//! or `root`, in `shards_action`; wading, here) and the game's (combat pace, a guard's walking
//! speed, a lock-on's, free sprint out of combat). Nothing is remembered between ticks, so the
//! component is neither in snapshots nor in replays, and a snapshot load cannot differ from the
//! run it was taken in: the sources (statuses, actions, motion) are what is saved.
//!
//! ## Where it is read
//!
//! The reset and the sources run in `SimSet::Input`, after the orders and before the NPC brains, so every reader sees
//! this tick's value: the NPC route follower (`top` speed of a walker), the action machine and
//! `update_fighters` (the sprint's stamina cost), the animation picker, the controller.
//! A client's own body does the same in `NetSet::Predict`
//! ([`ModifierSystems::PredictReset`], [`ModifierSystems::PredictContribute`]).
//!
//! ## How the controller composes them
//!
//! For the ground and the air:
//!
//! ```text
//! speed = ground_speed(input, sprint && !no_sprint)       walk / run / sprint of CharacterParams
//! speed = min(speed, cap)                                  the caps first...
//! speed = min(speed, unsprinted_cap)   unless sprinting    ...(a lock-on yields to a sprint)
//! speed = speed * factor * ground_factor                   ...then the factors
//! rooted: no walking, jumping, gliding or climbing (gravity and `external` still move the body)
//! ```
//!
//! Swimming takes `swim_speed` (or the fast one) times `factor` only: caps and `ground_factor`
//! (wading, combat pace) are for walkers. A glide is not scaled.
//!
//! A typical action-game pace (`speed = base * pace * stat * wade`, where `base` is 6.7 for
//! a sprint, else 4.5 when locked on, else 5.0; guarding walks at `3.0 * wade`) are then:
//! combat pace `ground_factor *= 0.85` (not while guarding or swimming), a status slow
//! `factor *= 0.75`, wading from [`CharacterParams`], lock-on `unsprinted_cap = 4.5`, guard
//! `cap = 3.0` plus `no_sprint`. The caps come before the factors, so a locked-on walk is
//! `4.5 * pace`, as expected.
//!
//! Every field neutral (`1.0`, no cap, not rooted) takes the exact code path of a character
//! that has no modifiers.

use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::controller::{CharacterController, CharacterParams, MotionMode, MotionState};

/// What modifies one character's movement this tick. See the [module](self) docs.
///
/// Sources use the methods ([`slow`](Self::slow), [`cap_at`](Self::cap_at), …), which combine
/// with what other sources wrote: factors multiply, caps take the lowest, flags add up. Systems
/// writing it must be ordered among themselves (the engine's order test sees to it): a product of
/// three floats depends on the order.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct MoveModifiers {
    /// Multiplies every voluntary speed: ground, air, swim (a status slow).
    pub factor: f32,
    /// Multiplies ground and air speed, but not a swim (combat pace, wading).
    pub ground_factor: f32,
    /// Highest ground speed (m/s) of any kind, a sprint included. Applied before the factors.
    pub cap: Option<f32>,
    /// Highest ground speed (m/s) of a walk or run; a sprint passes it (a lock-on's pace).
    pub unsprinted_cap: Option<f32>,
    /// `sprint` input does nothing (a guard's walk): no sprint speed, no sprint cost.
    pub no_sprint: bool,
    /// No voluntary translation: no walking, jumping, gliding, climbing. Gravity, `external`
    /// (a dodge, a push) and the water's float still move the body.
    pub rooted: bool,
    /// Multiplies the stamina a sprint costs; 0 makes it free (and needs no stamina).
    pub sprint_cost: f32,
}

impl Default for MoveModifiers {
    fn default() -> Self {
        Self {
            factor: 1.0,
            ground_factor: 1.0,
            cap: None,
            unsprinted_cap: None,
            no_sprint: false,
            rooted: false,
            sprint_cost: 1.0,
        }
    }
}

impl MoveModifiers {
    /// Nothing modified (`Default`, as a constant).
    pub const NEUTRAL: MoveModifiers = MoveModifiers {
        factor: 1.0,
        ground_factor: 1.0,
        cap: None,
        unsprinted_cap: None,
        no_sprint: false,
        rooted: false,
        sprint_cost: 1.0,
    };

    /// Nothing changes the character's speed (the sprint's cost may still).
    pub fn speed_neutral(&self) -> bool {
        self.factor == 1.0
            && self.ground_factor == 1.0
            && self.cap.is_none()
            && self.unsprinted_cap.is_none()
            && !self.no_sprint
            && !self.rooted
    }

    /// Nothing is modified at all.
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Multiplies every voluntary speed by `f` (0.75: a quarter slower). Not below 0.
    pub fn slow(&mut self, f: f32) -> &mut Self {
        if f.is_finite() {
            self.factor *= f.max(0.0);
        }
        self
    }

    /// Multiplies the ground and air speed by `f`, not the swim.
    pub fn slow_on_ground(&mut self, f: f32) -> &mut Self {
        if f.is_finite() {
            self.ground_factor *= f.max(0.0);
        }
        self
    }

    /// No ground speed above `limit` m/s, a sprint included (the lowest of all caps wins).
    pub fn cap_at(&mut self, limit: f32) -> &mut Self {
        if limit.is_finite() {
            let limit = limit.max(0.0);
            self.cap = Some(self.cap.map_or(limit, |c| c.min(limit)));
        }
        self
    }

    /// No walking or running above `limit` m/s; a sprint is not held back.
    pub fn cap_unsprinted_at(&mut self, limit: f32) -> &mut Self {
        if limit.is_finite() {
            let limit = limit.max(0.0);
            self.unsprinted_cap = Some(self.unsprinted_cap.map_or(limit, |c| c.min(limit)));
        }
        self
    }

    /// Takes away the voluntary movement (see [`rooted`](Self::rooted)).
    pub fn root(&mut self) -> &mut Self {
        self.rooted = true;
        self
    }

    /// The sprint input does nothing.
    pub fn forbid_sprint(&mut self) -> &mut Self {
        self.no_sprint = true;
        self
    }

    /// Multiplies the sprint's stamina cost by `f` (0: free).
    pub fn scale_sprint_cost(&mut self, f: f32) -> &mut Self {
        if f.is_finite() {
            self.sprint_cost *= f.max(0.0);
        }
        self
    }

    /// The ground speed for an input of length `amount` and whether it is a sprint: the
    /// character's [`CharacterParams::ground_speed`] through the modifiers.
    pub fn ground_speed(&self, params: &CharacterParams, amount: f32, sprint: bool) -> (f32, bool) {
        if self.speed_neutral() {
            return params.ground_speed(amount, sprint);
        }
        if self.rooted {
            return (0.0, false);
        }
        let (mut speed, sprinting) = params.ground_speed(amount, sprint && !self.no_sprint);
        if let Some(cap) = self.cap {
            speed = speed.min(cap);
        }
        if !sprinting && let Some(cap) = self.unsprinted_cap {
            speed = speed.min(cap);
        }
        (speed * self.factor * self.ground_factor, sprinting)
    }

    /// A swimming speed `top` through the modifiers (the factor only).
    pub fn swim_speed(&self, top: f32) -> f32 {
        if self.factor == 1.0 { top } else { top * self.factor }
    }

    /// The factor the whole speed range is scaled by where it is a plain factor: what an
    /// animation or a planner needs to compare the speed the character moves at with the one
    /// its clips and numbers were made for. Caps do not enter it.
    pub fn pace(&self) -> f32 {
        if self.rooted { 0.0 } else { self.factor * self.ground_factor }
    }

    /// `slowed x0.60, rooted, capped 3.0m/s`: what the AI reads. Empty when nothing is modified.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.rooted {
            parts.push("rooted".to_owned());
        }
        if self.factor != 1.0 {
            parts.push(format!("{} x{:.2}", if self.factor < 1.0 { "slowed" } else { "hastened" }, self.factor));
        }
        if self.ground_factor != 1.0 {
            parts.push(format!("ground x{:.2}", self.ground_factor));
        }
        if let Some(cap) = self.cap {
            parts.push(format!("capped {cap:.1}m/s"));
        }
        if let Some(cap) = self.unsprinted_cap {
            parts.push(format!("walk capped {cap:.1}m/s"));
        }
        if self.no_sprint {
            parts.push("no sprint".to_owned());
        }
        if self.sprint_cost != 1.0 {
            parts.push(format!("sprint cost x{:.2}", self.sprint_cost));
        }
        parts.join(", ")
    }
}

/// The order of the modifier systems. The reset and the sources run in `SimSet::Input`, after the orders and before the NPC brains;
/// a source of the game is `.in_set(ModifierSystems::Contribute)`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierSystems {
    /// `SimSet::Input`, after the orders: every [`MoveModifiers`] back to neutral.
    Reset,
    /// The wading source, inside [`Contribute`](Self::Contribute).
    Wade,
    /// The statuses source (`shards_action`), inside [`Contribute`](Self::Contribute), after
    /// [`Wade`](Self::Wade). A game's own sources go after it.
    Statuses,
    /// After [`Reset`](Self::Reset), before the NPC brains: the sources add theirs. Systems
    /// here write one component: order them among themselves (and after the engine's, `.after(ModifierSystems::Statuses)`).
    Contribute,
    /// `NetSet::Predict`: the reset for a client's own body ([`Predicted`](shards_core::Predicted)).
    PredictReset,
    /// [`Wade`](Self::Wade) for a client's own body.
    PredictWade,
    /// [`Statuses`](Self::Statuses) for a client's own body.
    PredictStatuses,
    /// `NetSet::Predict`, after the reset, before `MotionSystems::PredictIntent`: the sources
    /// for a client's own body (the engine's run here too; a game's must be added to both sets).
    PredictContribute,
}

pub(crate) fn plugin(app: &mut App) {
    use ai_core::{AiAppExt, SimSet};
    use shards_core::{NetSet, Predicted};

    app.configure_sets(
        FixedUpdate,
        (ModifierSystems::Reset, ModifierSystems::Contribute)
            .chain()
            .in_set(SimSet::Input)
            .after(shards_core::CoreSystems::FollowOrders),
    )
    .configure_sets(
        FixedUpdate,
        (ModifierSystems::PredictReset, ModifierSystems::PredictContribute)
            .chain()
            .in_set(NetSet::Predict)
            .after(crate::MotionSystems::PredictOrders)
            .before(crate::MotionSystems::PredictIntent),
    )
    .configure_sets(
        FixedUpdate,
        (ModifierSystems::Wade, ModifierSystems::Statuses).chain().in_set(ModifierSystems::Contribute),
    )
    .configure_sets(
        FixedUpdate,
        (ModifierSystems::PredictWade, ModifierSystems::PredictStatuses)
            .chain()
            .in_set(ModifierSystems::PredictContribute),
    )
    // Derived each tick from the sources, which are state: the replay hash and the network
    // have nothing to take from it. (A snapshot holds it harmlessly: it is reset before use.)
    .register_type::<MoveModifiers>()
    .replay_ignore::<MoveModifiers>()
    .net_ignore::<MoveModifiers>()
    .register_ai_component::<MoveModifiers>(|m| m.describe())
    .add_systems(
        FixedUpdate,
        (
            reset::<()>.in_set(ModifierSystems::Reset),
            reset::<With<Predicted>>.in_set(ModifierSystems::PredictReset),
            wade::<()>.in_set(ModifierSystems::Wade),
            wade::<With<Predicted>>.in_set(ModifierSystems::PredictWade),
        ),
    );
}

/// Every character's modifiers back to neutral. A character that has none (a snapshot written
/// before the component existed strips it on load) gets one here, before the sources run.
type Resetting<'w, 's, F> = Query<'w, 's, (Entity, Option<&'static mut MoveModifiers>), (With<CharacterController>, F)>;

fn reset<F: QueryFilter + 'static>(mut commands: Commands, mut characters: Resetting<F>) {
    for (entity, modifiers) in &mut characters {
        match modifiers {
            Some(mut modifiers) => {
                if !modifiers.is_neutral() {
                    *modifiers = MoveModifiers::default();
                }
            }
            None => {
                commands.entity(entity).try_insert(MoveModifiers::default());
            }
        }
    }
}

/// Wading: a walker standing in water slows by the depth over its feet, by `wade_*` of its
/// params. Swimmers are not waders; characters without `wade_factor` are not touched.
fn wade<F: QueryFilter + 'static>(
    db: Option<Res<ContentDb>>,
    mut characters: Query<
        (&CharacterController, Option<&CharacterParams>, &MotionState, &Transform, &mut MoveModifiers),
        F,
    >,
) {
    let db = db.as_deref();
    for (controller, own, state, tf, mut modifiers) in &mut characters {
        let params = controller.resolve(own, db);
        if params.wade_factor == 1.0 || state.mode != MotionMode::Ground {
            continue;
        }
        if let Some(surface) = state.water_surface {
            modifiers.slow_on_ground(params.wade_scale((surface - tf.translation.y).max(0.0)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CharacterParams {
        CharacterParams { walk_speed: 2.5, run_speed: 5.0, sprint_speed: 6.7, ..default() }
    }

    #[test]
    fn neutral_is_the_plain_ground_speed() {
        let (p, m) = (params(), MoveModifiers::default());
        assert!(m.is_neutral() && m.speed_neutral());
        for (amount, sprint) in [(0.0, false), (0.3, false), (0.5, false), (1.0, false), (1.0, true)] {
            assert_eq!(m.ground_speed(&p, amount, sprint), p.ground_speed(amount, sprint));
        }
    }

    #[test]
    fn factors_multiply_and_caps_come_first() {
        let p = params();
        let mut m = MoveModifiers::default();
        m.slow(0.75).slow_on_ground(0.85);
        let (speed, _) = m.ground_speed(&p, 1.0, false);
        assert!((speed - 5.0 * 0.75 * 0.85).abs() < 1e-6, "{speed}");
        // A lock-on cap: 4.5 first, then the pace.
        let mut locked = MoveModifiers::default();
        locked.cap_unsprinted_at(4.5).slow_on_ground(0.85);
        assert!((locked.ground_speed(&p, 1.0, false).0 - 4.5 * 0.85).abs() < 1e-6);
        // ...and a sprint beats the lock-on.
        let (sprint, sprinting) = locked.ground_speed(&p, 1.0, true);
        assert!(sprinting && (sprint - 6.7 * 0.85).abs() < 1e-6, "{sprint}");
    }

    #[test]
    fn a_cap_holds_back_the_sprint_and_the_lowest_wins() {
        let p = params();
        let mut m = MoveModifiers::default();
        m.cap_at(4.0).cap_at(3.0);
        assert_eq!(m.cap, Some(3.0));
        assert_eq!(m.ground_speed(&p, 1.0, true), (3.0, true));
        assert_eq!(m.ground_speed(&p, 0.5, false), (2.5, false), "a slower walk is not raised");
    }

    #[test]
    fn no_sprint_and_root() {
        let p = params();
        let mut m = MoveModifiers::default();
        m.forbid_sprint();
        assert_eq!(m.ground_speed(&p, 1.0, true), (5.0, false));
        m.root();
        assert_eq!(m.ground_speed(&p, 1.0, true), (0.0, false));
        assert_eq!(m.pace(), 0.0);
    }

    #[test]
    fn sources_that_make_no_sense_change_nothing() {
        let mut m = MoveModifiers::default();
        m.slow(f32::NAN).cap_at(f32::INFINITY).slow_on_ground(f32::NAN).scale_sprint_cost(f32::NAN);
        assert!(m.is_neutral());
        m.slow(-2.0);
        assert_eq!(m.factor, 0.0, "never negative");
    }

    #[test]
    fn swimming_takes_the_factor_only() {
        let mut m = MoveModifiers::default();
        m.slow(0.5).slow_on_ground(0.1).cap_at(1.0);
        assert_eq!(m.swim_speed(2.4), 1.2);
    }

    #[test]
    fn the_description_names_what_is_modified() {
        assert_eq!(MoveModifiers::default().describe(), "");
        let mut m = MoveModifiers::default();
        m.slow(0.6).root().cap_at(3.0).forbid_sprint().scale_sprint_cost(0.0);
        assert_eq!(m.describe(), "rooted, slowed x0.60, capped 3.0m/s, no sprint, sprint cost x0.00");
    }

    #[test]
    fn wading_slows_in_a_straight_line_with_depth() {
        let p = CharacterParams { wade_min_depth: 0.15, wade_full_depth: 0.75, wade_factor: 0.75, ..default() };
        assert_eq!(p.wade_scale(0.0), 1.0);
        assert_eq!(p.wade_scale(0.15), 1.0);
        assert!((p.wade_scale(0.45) - 0.875).abs() < 1e-6);
        assert_eq!(p.wade_scale(0.75), 0.75);
        assert_eq!(p.wade_scale(3.0), 0.75);
        assert_eq!(CharacterParams::default().wade_scale(5.0), 1.0, "off by default");
    }
}
