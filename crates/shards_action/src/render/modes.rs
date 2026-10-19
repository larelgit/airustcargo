//! Motion modes in the picture: what a body shows while it swims, climbs or glides, and after it
//! leaves the mode. See `mounts` (`"@mode:<mode>"`, `"@left:<mode>"`) and `drops` in
//! [`crate::anim`] for the content.
//!
//! [`ModeState`] is the bookkeeping of one body, advanced once a frame by
//! [`drive`](super::bridge::drive) from the drawn motion mode, whether an action runs and the
//! simulation's clock (so a pause freezes it and a `step` moves it). It answers three questions:
//!
//! - which `@mode` rules apply now (they step aside while an action runs in the mode and for
//!   `yield_after` seconds after it: a blade swung afloat stays in the hand);
//! - which `@left` rules are on their clock (a body that left the mode shows the props where the
//!   mode had them for `wait` seconds, or until its fade out of the mode's clip is done, then the
//!   rules' moves run from 0 and the [`ModeLeft`] event fires);
//! - how far the model is lowered ([`DropDef`](crate::DropDef)).
//!
//! The drop is written by [`apply_model`], the `AnimSystems::Model` stage: on the set's
//! `root_motion` bone, after the clips and [`hold_root_motion`](super::bridge::hold_root_motion)
//! have written it, before transforms propagate, never on the model child that carries the render
//! interpolation. Presentation only: nothing here is read by the simulation.

use bevy::prelude::*;
use shards_motion::MotionMode;

use super::bridge::AnimationBound;
use crate::anim::{equip_open, mode_name, AnimSet, Animated, MountRules};

/// The stages of the model's own writes in `PostUpdate`, after the clips are applied and the render
/// interpolation has moved the model, before transforms propagate: the root bone goes back on the
/// ground (`Root`), then the model is offset (`Model`: the drop; later slices add the facing, the
/// stride and the lunge).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnimSystems {
    /// The set's `root_motion` bone back at its rest along the ground.
    Root,
    /// The model's offsets written on that bone.
    Model,
}

/// A wait for the props to be drawn after the body left a mode.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Left {
    mode: MotionMode,
    /// The simulation clock when the body left.
    at: f64,
    /// The clock when the wait ended (the moves' time 0); `None` while it waits.
    started: Option<f64>,
}

/// What one body's frame tells [`ModeState::advance`].
pub(crate) struct ModeFrame<'a> {
    pub set: &'a AnimSet,
    /// The drawn motion mode.
    pub mode: MotionMode,
    /// An action runs.
    pub acting: bool,
    /// The run's instance and whether it ends a weapon switch's equip (a whole-body action); the
    /// same ends a pending `@left`.
    pub run: Option<(u32, bool)>,
    /// The simulation clock, between ticks too.
    pub clock: f64,
}

/// What the mounts read of a body's mode: the rules that apply and their clocks.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ModeOut<'a> {
    /// The `@mode` rules, while they apply, and the seconds since they began to.
    pub mode: Option<(&'a MountRules, f32)>,
    /// The `@left` rules on their clock (0 while they wait).
    pub left: Option<(&'a MountRules, f32)>,
    /// The `@left` wait ended this frame: the mode the body left.
    pub fired: Option<MotionMode>,
}

/// Per body: the mode clocks and the drop. Kept in [`AnimationBound`] and taken over by a new
/// binding of the same body.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModeState {
    /// The mode drawn at the last frame; `None` before the first.
    mode: Option<MotionMode>,
    /// The clock since which the mode's rules have applied without a break.
    applied_since: Option<f64>,
    /// The clock when an action last ran in the mode.
    last_acted: Option<f64>,
    /// The mode's rules stepped aside at the last frame.
    yielding: bool,
    left: Option<Left>,
    /// The run instance seen at the last frame.
    run: Option<u32>,
    /// The clock and blend of the last time the body's clip changed: the fade out of the mode's
    /// clip is done once that blend has passed.
    switched: Option<(f64, f32)>,
    last_clock: Option<f64>,
    /// Metres the model is lowered now, and the rate it moves at (m/s).
    depth: f32,
    rate: f32,
    /// What [`apply_model`] wrote on the bone at the last frame, and the part of it that was the
    /// offset: lets it tell a bone the clips wrote again from one they left alone.
    written: Option<(Vec3, Vec3)>,
}

impl ModeState {
    /// A new binding of the same body keeps the clocks; the bone it wrote on is another one now.
    pub(crate) fn taken_over(&self) -> Self {
        Self { written: None, ..self.clone() }
    }

    /// The picture was held for `secs` of the simulation's clock: the clocks resume where they
    /// stopped.
    pub(crate) fn shift(&mut self, secs: f64) {
        for t in [&mut self.applied_since, &mut self.last_acted, &mut self.last_clock].into_iter().flatten() {
            *t += secs;
        }
        if let Some(left) = &mut self.left {
            left.at += secs;
            if let Some(started) = &mut left.started {
                *started += secs;
            }
        }
        if let Some((at, _)) = &mut self.switched {
            *at += secs;
        }
    }

    /// The body's clip changed at `clock`, fading in over `blend` seconds.
    pub(crate) fn note_switch(&mut self, clock: f64, blend: f32) {
        self.switched = Some((clock, blend));
    }

    /// Metres the model is lowered now.
    pub(crate) fn depth(&self) -> f32 {
        self.depth
    }

    /// The mode the body is drawn in, once seen.
    pub(crate) fn mode(&self) -> Option<MotionMode> {
        self.mode
    }

    /// Whether the `@mode` rules stepped aside at the last frame.
    pub(crate) fn yielding(&self) -> bool {
        self.yielding
    }

    /// The mode a `@left` wait or draw is on, and whether it still waits.
    pub(crate) fn left(&self) -> Option<(MotionMode, bool)> {
        self.left.map(|l| (l.mode, l.started.is_none()))
    }

    /// The seconds since an action last ran in the mode, while one did.
    pub(crate) fn acted_ago(&self, clock: f64) -> Option<f32> {
        self.last_acted.map(|t| (clock - t) as f32)
    }

    /// One frame on. See the module docs.
    pub(crate) fn advance<'a>(&mut self, frame: &ModeFrame<'a>) -> ModeOut<'a> {
        let (set, clock, mode) = (frame.set, frame.clock, frame.mode);
        let dt = self.last_clock.map_or(0.0, |last| (clock - last).max(0.0)) as f32;
        self.last_clock = Some(clock);

        // A whole-body action ends a pending wait or draw, as it ends an equip.
        let instance = frame.run.map(|(i, _)| i);
        if self.run != instance {
            if frame.run.is_some_and(|(_, ends)| ends) {
                self.left = None;
            }
            self.run = instance;
        }

        // The mode changed: the body left one. It draws only if its rules were not stepping aside
        // (a blade already in the hand has nothing to draw); a mode it is back in has no left.
        if self.mode != Some(mode) {
            if let Some(prev) = self.mode
                && set.left_rules(prev).is_some()
                && !self.yielding
            {
                self.left = Some(Left { mode: prev, at: clock, started: None });
            }
            self.mode = Some(mode);
            self.applied_since = None;
            self.last_acted = None;
            self.yielding = false;
            if self.left.is_some_and(|l| l.mode == mode) {
                self.left = None;
            }
        }
        if frame.acting {
            self.last_acted = Some(clock);
        }

        // The mode's rules apply unless they yield; when they do, they take over from a pending left.
        let rules = set.mode_rules(mode);
        self.yielding = rules.is_some_and(|r| {
            r.yield_after > 0.0 && (frame.acting || self.last_acted.is_some_and(|t| clock - t < f64::from(r.yield_after)))
        });
        let mut out = ModeOut::default();
        match rules {
            Some(r) if !self.yielding => {
                let since = *self.applied_since.get_or_insert(clock);
                out.mode = Some((r, (clock - since) as f32));
                self.left = None;
            }
            _ => self.applied_since = None,
        }

        // The left: waits, then runs the rules' moves from 0.
        if let Some(mut left) = self.left {
            match set.left_rules(left.mode) {
                None => self.left = None,
                Some(rules) => {
                    if left.started.is_none() {
                        let faded = self.switched.is_none_or(|(at, blend)| clock - at >= f64::from(blend));
                        if clock - left.at >= f64::from(rules.wait) || faded {
                            left.started = Some(clock);
                            out.fired = Some(left.mode);
                        }
                    }
                    let since = left.started.map_or(0.0, |s| (clock - s) as f32);
                    // `started` is only just set: the moves are open at 0 when they have any.
                    if left.started.is_some() && !equip_open(rules, since) {
                        self.left = None;
                    } else {
                        self.left = Some(left);
                        out.left = Some((rules, since));
                    }
                }
            }
        }

        // The drop: while an action runs in a mode that has one, at a constant rate; back the same way.
        match set.drops.get(mode_name(mode)) {
            Some(drop) if frame.acting => {
                self.rate = drop.rate();
                self.depth = (self.depth + self.rate * dt).min(drop.down);
            }
            Some(drop) => {
                self.rate = drop.rate();
                self.depth = (self.depth - self.rate * dt).max(0.0);
            }
            None => self.depth = (self.depth - self.rate * dt).max(0.0),
        }
        out
    }
}

/// The world's down by `depth` metres as a move of a bone whose parent is at `parent`: a model scaled to
/// its body drops by metres, not by rig units.
fn local_drop(parent: &GlobalTransform, depth: f32) -> Vec3 {
    parent.to_matrix().inverse().transform_vector3(Vec3::NEG_Y * depth)
}

/// `PostUpdate`, `AnimSystems::Model`: the drop written on the `root_motion` bone, on top of what the
/// clips and `hold_root_motion` left this frame. The offset is the world's down turned into the
/// bone's parent space, so a model scaled to its body drops by metres, not by rig units.
///
/// The clips write the bone every frame they have a curve for it; when none does the bone still has
/// last frame's offset, so that one is taken off first (a component that is exactly what this stage
/// wrote is not the clips'). Held bodies keep their drop, written again as the paused clips are.
pub(crate) fn apply_model(
    mut animated: Query<&mut AnimationBound, With<Animated>>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    mut transforms: Query<&mut Transform>,
) {
    for mut bound in &mut animated {
        let Some((bone, _)) = bound.root else { continue };
        let depth = bound.modes.depth;
        if depth <= 0.0 && bound.modes.written.is_none() {
            continue;
        }
        let Ok(mut transform) = transforms.get_mut(bone) else { continue };
        let mut base = transform.translation;
        if let Some((written, offset)) = bound.modes.written {
            for axis in 0..3 {
                if base[axis] == written[axis] {
                    base[axis] -= offset[axis];
                }
            }
        }
        let offset = parents
            .get(bone)
            .ok()
            .and_then(|parent| globals.get(parent.parent()).ok())
            .map_or(Vec3::NEG_Y * depth, |parent| local_drop(parent, depth));
        transform.translation = base + offset;
        bound.modes.written = (depth > 0.0).then_some((transform.translation, offset));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::render::bridge::AnimationBound;

    fn set() -> AnimSet {
        let text = r#"{ "h": (
            source: "models/h.glb",
            clips: {"idle": ()},
            root_motion: "root",
            sockets: {"back": (bone: "spine")},
            mounts: {
                "@mode:water": (moves: [(prop: "sword", to: Some("back"), span: Secs(0.0, 3600.0))], yield_after: 1.0),
                "@left:water": (moves: [(prop: "sword", to: Some("back"), span: Secs(0.0, 0.2))], wait: 1.0),
                "@mode:mantle": (moves: [(prop: "sword", to: Some("back"), span: Secs(0.0, 3600.0))]),
                "@left:mantle": (moves: [(prop: "sword", to: Some("back"), span: Secs(0.0, 0.2))]),
            },
            drops: {"water": (down: 0.15, seconds: 0.3)},
        ) }"#;
        let sets: BTreeMap<String, AnimSet> = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .unwrap_or_else(|e| panic!("{e}"));
        sets["h"].clone()
    }

    fn frame<'a>(set: &'a AnimSet, mode: MotionMode, acting: bool, clock: f64) -> ModeFrame<'a> {
        ModeFrame { set, mode, acting, run: acting.then_some((1, true)), clock }
    }

    #[test]
    fn the_mode_rules_apply_while_in_the_mode_and_count_from_its_start() {
        let set = set();
        let mut s = ModeState::default();
        assert!(s.advance(&frame(&set, MotionMode::Ground, false, 0.0)).mode.is_none());
        let out = s.advance(&frame(&set, MotionMode::Water, false, 1.0));
        assert_eq!(out.mode.map(|(_, since)| since), Some(0.0));
        let out = s.advance(&frame(&set, MotionMode::Water, false, 1.5));
        assert_eq!(out.mode.map(|(_, since)| since), Some(0.5));
        assert!(out.left.is_none() && out.fired.is_none());
    }

    #[test]
    fn an_action_afloat_makes_the_rules_step_aside_for_the_linger() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        assert!(s.advance(&frame(&set, MotionMode::Water, true, 1.0)).mode.is_none(), "while the action runs");
        assert!(s.yielding());
        assert!(s.advance(&frame(&set, MotionMode::Water, false, 1.9)).mode.is_none(), "0.9 s after it ended: still");
        let out = s.advance(&frame(&set, MotionMode::Water, false, 2.1));
        assert_eq!(out.mode.map(|(_, since)| since), Some(0.0), "1.1 s after: the rules begin again, from 0");
        // Mantle's rules have no linger: an action does not make them step aside.
        let mut s = ModeState::default();
        assert!(s.advance(&frame(&set, MotionMode::Mantle, true, 0.0)).mode.is_some());
    }

    #[test]
    fn leaving_the_mode_waits_then_runs_the_moves_and_fires_the_event_once() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        s.note_switch(1.0, 0.5);
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.0));
        assert_eq!(out.left.map(|(_, since)| since), Some(0.0), "waiting: read at time 0");
        assert!(out.fired.is_none() && s.left() == Some((MotionMode::Water, true)));
        // The fade out of the swim clip is done at 1.5 s, before the 1 s wait would be.
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.4));
        assert!(out.fired.is_none() && out.left.is_some());
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.5));
        assert_eq!(out.fired, Some(MotionMode::Water));
        assert_eq!(out.left.map(|(_, since)| since), Some(0.0));
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.6));
        assert!(out.fired.is_none(), "once");
        assert_eq!(out.left.map(|(_, since)| (since * 10.0).round()), Some(1.0));
        // The moves last 0.2 s: then the left is over.
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.8));
        assert!(out.left.is_none() && s.left().is_none());
    }

    #[test]
    fn the_wait_is_the_most_it_takes_when_the_fade_is_slower() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        s.note_switch(1.0, 5.0);
        s.advance(&frame(&set, MotionMode::Ground, false, 1.0));
        assert!(s.advance(&frame(&set, MotionMode::Ground, false, 1.9)).fired.is_none());
        assert_eq!(s.advance(&frame(&set, MotionMode::Ground, false, 2.0)).fired, Some(MotionMode::Water), "after `wait` 1 s");
    }

    #[test]
    fn a_wait_of_nothing_fires_at_once() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Mantle, false, 0.0));
        assert!(s.advance(&frame(&set, MotionMode::Mantle, false, 0.5)).mode.is_some());
        s.note_switch(1.0, 0.3);
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.0));
        assert_eq!(out.fired, Some(MotionMode::Mantle));
        assert_eq!(out.left.map(|(_, since)| since), Some(0.0));
    }

    #[test]
    fn a_body_that_left_while_lingering_has_nothing_to_draw() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        s.advance(&frame(&set, MotionMode::Water, true, 1.0));
        s.advance(&frame(&set, MotionMode::Water, false, 1.2));
        let out = s.advance(&frame(&set, MotionMode::Ground, false, 1.3));
        assert!(out.left.is_none() && out.fired.is_none() && s.left().is_none(), "the blade is in the hand already");
    }

    #[test]
    fn a_whole_body_action_or_the_mode_again_ends_a_pending_left() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        s.advance(&frame(&set, MotionMode::Ground, false, 1.0));
        assert!(s.left().is_some());
        let out = s.advance(&frame(&set, MotionMode::Ground, true, 1.1));
        assert!(out.left.is_none() && out.fired.is_none() && s.left().is_none(), "an action ended the wait");
        s.advance(&frame(&set, MotionMode::Water, false, 2.0));
        s.advance(&frame(&set, MotionMode::Ground, false, 3.0));
        assert!(s.left().is_some());
        s.advance(&frame(&set, MotionMode::Water, false, 3.1));
        assert!(s.left().is_none(), "back in the water");
        // A layered action (it does not end the equip) leaves the wait alone.
        s.advance(&frame(&set, MotionMode::Ground, false, 4.0));
        let mut layered = frame(&set, MotionMode::Ground, true, 4.1);
        layered.run = Some((2, false));
        s.advance(&layered);
        assert!(s.left().is_some());
        // Entering a mode that has rules takes over from a left (water to the climb-out).
        s.advance(&frame(&set, MotionMode::Mantle, false, 4.2));
        assert!(s.left().is_none());
    }

    #[test]
    fn the_drop_goes_down_at_a_constant_rate_and_comes_back_the_same_way() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        for (clock, want) in [(0.1, 0.05), (0.2, 0.10), (0.3, 0.15), (0.5, 0.15)] {
            s.advance(&frame(&set, MotionMode::Water, true, clock));
            assert!((s.depth() - want).abs() < 1e-5, "at {clock} s: {}", s.depth());
        }
        for (clock, want) in [(0.6, 0.10), (0.7, 0.05), (0.8, 0.0), (1.0, 0.0)] {
            s.advance(&frame(&set, MotionMode::Water, false, clock));
            assert!((s.depth() - want).abs() < 1e-5, "at {clock} s: {}", s.depth());
        }
        // The mode changes under a drop: it comes back at the rate it went down at.
        s.advance(&frame(&set, MotionMode::Water, true, 1.1));
        s.advance(&frame(&set, MotionMode::Water, true, 1.2));
        assert!(s.depth() > 0.09);
        s.advance(&frame(&set, MotionMode::Ground, true, 1.3));
        assert!((s.depth() - 0.05).abs() < 1e-5, "{}", s.depth());
        // A mode that has no drop does not lower the model.
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Ground, false, 0.0));
        s.advance(&frame(&set, MotionMode::Ground, true, 1.0));
        assert_eq!(s.depth(), 0.0);
    }

    #[test]
    fn the_drop_is_written_once_whether_or_not_the_clips_write_the_bone() {
        let mut app = App::new();
        app.add_systems(Update, apply_model);
        let parent = app.world_mut().spawn((Transform::default(), GlobalTransform::default())).id();
        let bone = app.world_mut().spawn((Transform::from_xyz(0.1, 0.9, 0.2), ChildOf(parent))).id();
        let mut bound = AnimationBound::new(Entity::PLACEHOLDER, AssetId::invalid());
        bound.root = Some((bone, Vec3::ZERO));
        bound.modes.depth = 0.15;
        let body = app.world_mut().spawn((Animated::new("h"), bound)).id();
        let at = |app: &App| app.world().get::<Transform>(bone).unwrap().translation;
        let near = |a: Vec3, b: Vec3| a.distance(b) < 1e-6;
        app.update();
        assert!(near(at(&app), Vec3::new(0.1, 0.75, 0.2)));
        // No clip writes the bone: last frame's drop is taken off before it is put on, not added again.
        for _ in 0..3 {
            app.update();
        }
        assert!(near(at(&app), Vec3::new(0.1, 0.75, 0.2)), "{}", at(&app));
        // A clip writes it every frame: the drop goes on top of what it wrote, once.
        app.world_mut().get_mut::<Transform>(bone).unwrap().translation.y = 0.95;
        app.update();
        assert!(near(at(&app), Vec3::new(0.1, 0.80, 0.2)), "{}", at(&app));
        // The drop changes while the clips leave the bone alone (it still holds the last write).
        app.world_mut().get_mut::<AnimationBound>(body).unwrap().modes.depth = 0.05;
        app.update();
        assert!(near(at(&app), Vec3::new(0.1, 0.90, 0.2)), "{}", at(&app));
        // Back up: the bone is where the clip had it, and the stage lets go of it.
        app.world_mut().get_mut::<AnimationBound>(body).unwrap().modes.depth = 0.0;
        app.update();
        assert!(near(at(&app), Vec3::new(0.1, 0.95, 0.2)), "{}", at(&app));
        app.update();
        assert!(near(at(&app), Vec3::new(0.1, 0.95, 0.2)));
        assert!(app.world().get::<AnimationBound>(body).unwrap().modes.written.is_none());
    }

    #[test]
    fn the_drop_is_in_metres_whatever_the_models_scale() {
        let at = |scale: f32| GlobalTransform::from(Transform::from_xyz(3.0, 1.0, 2.0).with_scale(Vec3::splat(scale)));
        assert!((local_drop(&at(1.0), 0.15) - Vec3::new(0.0, -0.15, 0.0)).length() < 1e-6);
        assert!((local_drop(&at(2.0), 0.15) - Vec3::new(0.0, -0.075, 0.0)).length() < 1e-6, "a body twice as big: half the rig units");
        // A model turned about the vertical keeps the drop vertical; one lying down turns it into its own axis.
        let turned = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_y(1.0)));
        assert!((local_drop(&turned, 0.15) - Vec3::new(0.0, -0.15, 0.0)).length() < 1e-6);
        let lying = GlobalTransform::from(Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)));
        assert!((local_drop(&lying, 0.15).abs() - Vec3::new(0.0, 0.0, 0.15)).length() < 1e-5);
    }

    #[test]
    fn a_pause_does_not_age_the_clocks_and_a_hold_shifts_them() {
        let set = set();
        let mut s = ModeState::default();
        s.advance(&frame(&set, MotionMode::Water, false, 0.0));
        s.advance(&frame(&set, MotionMode::Water, true, 1.0));
        s.advance(&frame(&set, MotionMode::Water, true, 1.1));
        let depth = s.depth();
        // The clock does not move: nothing does.
        s.advance(&frame(&set, MotionMode::Water, true, 1.1));
        assert_eq!(s.depth(), depth);
        // Held for 5 s: the linger is not eaten by it.
        s.advance(&frame(&set, MotionMode::Water, false, 1.2));
        s.shift(5.0);
        assert!(s.advance(&frame(&set, MotionMode::Water, false, 6.4)).mode.is_none(), "0.3 s of linger passed, not 5.3");
        assert!(s.advance(&frame(&set, MotionMode::Water, false, 7.2)).mode.is_some());
    }
}
