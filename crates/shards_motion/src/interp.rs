//! Render interpolation: a simulated body is drawn between its last two ticks.
//!
//! The simulation steps at a fixed rate (60 Hz), and a body's `Transform` is the value of the
//! last tick, so on a 144 Hz display a body would move in 60 Hz steps and the follow camera would
//! inherit them. Here the picture is smoothed without touching the simulation:
//!
//! - **The visual child carries the history.** [`Interpolated`] sits on a *child* of a
//!   [`CharacterController`] body (the model, a held prop: anything drawn that rides the body,
//!   without an `AiId`). The engine adds it by itself ([`adopt_visuals`]), so a game that dresses its
//!   bodies with a model child gets it with no code. A child that must not be smoothed carries
//!   [`NotInterpolated`]. The body itself gets nothing: its `Transform` is what physics, `observe`
//!   and the replay read, and it is never written here.
//! - **Capture, `FixedLast`** ([`InterpSystems::Capture`]): the parent's pose after the tick
//!   becomes `cur`, the old `cur` becomes `prev`.
//! - **Blend, `PostUpdate`, before transforms propagate** ([`InterpSystems::Blend`]): the child's
//!   local `Transform` is set so that it is drawn where `lerp(prev, cur, overstep_fraction)` puts
//!   the body: its own rest transform, moved by `blend - cur` in the parent's frame. The follow
//!   camera runs after it and follows the same blended pose ([`shown_translation`]).
//!
//! **Resets** (no smear across a jump): the history starts over when a body (a) moved more than
//! `CameraTuning::body_snap_distance` (content, 4 m) in one tick, (b) was moved outside a tick
//! (its `Transform` is not what the last tick left; a `teleport` between frames), (c) was moved
//! inside a tick by someone but the controller (`MotionState::placed` was not where the feet
//! were when the tick began; noted in `FixedFirst`, before the controller consumes it), (d) the
//! tick counter did not advance by one since the last capture (a snapshot loaded, a replay
//! rewound, the setting was off for a while). Entities that a snapshot load or a region change
//! replaces start with no history at all.
//!
//! **Where it does nothing**: without a picture (`EngineConfig::draws`: headless, tests of the
//! simulation), with `graphics.interpolation` off (the child is put back exactly where the game
//! left it), while the clock is paused (the picture shows the last tick, so `shots` after a
//! `step` match `observe`), and on a client for bodies it does not predict: their
//! interpolation in `shards_net` already blends host states, and blending twice would lag.
//! Presentation only: nothing here reads or writes a game component.

use ai_core::{AiId, SimTick};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use shards_core::{EngineConfig, NetRole, Predicted};
use shards_settings::GraphicsSettings;

use crate::camera::CameraTuning;
use crate::controller::{CharacterController, MotionState};

/// A body's place in the world: what the blend is made of.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl Pose {
    pub fn of(tf: &Transform) -> Self {
        Self { translation: tf.translation, rotation: tf.rotation }
    }

    /// `self` at `t == 0`, `to` at `t == 1`; the rotation takes the short way round.
    pub fn blend(self, to: Pose, t: f32) -> Pose {
        Pose { translation: self.translation.lerp(to.translation, t), rotation: self.rotation.slerp(to.rotation, t) }
    }
}

/// Where the child's local transform must be for it to be drawn riding a body shown at `shown`
/// while the body's real `Transform` is `body`: `rest` (the child's transform relative to the
/// body) moved by the difference between the two, in the body's frame. The body is taken to be
/// unscaled (a collider's size is its `Shape`).
pub fn visual_offset(body: &Transform, shown: Pose, rest: Transform) -> Transform {
    let inverse = body.rotation.inverse();
    let turn = inverse * shown.rotation;
    Transform {
        translation: inverse * (shown.translation - body.translation) + turn * rest.translation,
        rotation: turn * rest.rotation,
        scale: rest.scale,
    }
}

/// Opts a child out of render interpolation (a trail that should stay where the tick left it).
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct NotInterpolated;

/// A drawn child of a simulated body, smoothed between the body's last two ticks. Presentation
/// only: no `AiId`, not in a snapshot, not in a replay; the child carries it so the body's own
/// components stay the simulation's. See the module docs.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component, Default)]
pub struct Interpolated {
    prev: Option<Pose>,
    cur: Option<Pose>,
    /// The body was placed inside the coming tick: its capture starts the history over.
    moved: bool,
    /// The tick of the last capture.
    tick: Option<u64>,
    /// The child's own transform, as the game left it.
    rest: Option<Transform>,
    /// What this module last wrote to the child; anything else there is a new `rest`.
    applied: Option<Transform>,
    /// The body's pose as drawn this frame; `None` when nothing is blended.
    shown: Option<Pose>,
}

impl Interpolated {
    /// The body's pose after the tick before the last one.
    pub fn previous(&self) -> Option<Pose> {
        self.prev
    }

    /// The body's pose after the last tick.
    pub fn current(&self) -> Option<Pose> {
        self.cur
    }

    /// The pose the body is drawn at this frame; `None` while nothing is blended (setting off,
    /// no history, paused clocks show the last tick).
    pub fn shown(&self) -> Option<Pose> {
        self.shown
    }

    /// Forget the history: the next capture shows the body where it is.
    pub fn reset(&mut self) {
        self.prev = None;
        self.cur = None;
        self.shown = None;
        self.moved = false;
    }

    /// The tick ended with the body at `now`. A jump, a gap in ticks or a placement noted before
    /// the tick start the history over there.
    pub fn capture(&mut self, now: Pose, tick: Option<u64>, snap: f32) {
        let gap = match (self.tick, tick) {
            (Some(last), Some(tick)) => tick != last + 1,
            _ => false,
        };
        let jumped = self.cur.is_some_and(|cur| cur.translation.distance(now.translation) > snap);
        if gap || jumped || self.moved || self.cur.is_none() {
            self.prev = Some(now);
        } else {
            self.prev = self.cur;
        }
        self.cur = Some(now);
        self.moved = false;
        self.tick = tick;
    }

    /// The pose between the last two ticks at `fraction` (0 the older, 1 the last).
    pub fn at(&self, fraction: f32) -> Option<Pose> {
        Some(self.prev?.blend(self.cur?, fraction.clamp(0.0, 1.0)))
    }
}

/// Ordering for a game's own presentation systems.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InterpSystems {
    /// `FixedLast`: the tick's end poses are kept.
    Capture,
    /// `PostUpdate`, before transforms propagate: the children are moved. Anything that reads
    /// [`Interpolated::shown`] (the follow camera) runs after it.
    Blend,
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<Interpolated>()
        .register_type::<NotInterpolated>()
        .configure_sets(PostUpdate, InterpSystems::Blend.before(TransformSystems::Propagate))
        .add_systems(FixedFirst, (note_placements, restore_rest).run_if(showing))
        .add_systems(FixedLast, capture.in_set(InterpSystems::Capture).run_if(showing))
        .add_systems(PostUpdate, (adopt_visuals, blend).chain().in_set(InterpSystems::Blend).run_if(drawing));
}

/// Something is drawn (a window or an offscreen image); a test app without an `EngineConfig`
/// counts as drawing, so the math can be tested without one.
fn drawing(config: Option<Res<EngineConfig>>) -> bool {
    config.is_none_or(|c| c.draws())
}

/// Drawing, with the setting on.
fn showing(config: Option<Res<EngineConfig>>, settings: Option<Res<GraphicsSettings>>) -> bool {
    config.is_none_or(|c| c.draws()) && settings.is_none_or(|s| s.interpolation)
}

/// Whether render interpolation applies to a body: everywhere that simulates it, and on a client
/// to the body it predicts. A client's other bodies are shown by `shards_net`'s interpolation of
/// host states, which already blends them; blending those twice would lag them.
fn applies(role: Option<&NetRole>, predicted: bool) -> bool {
    role.is_none_or(|r| r.simulates()) || predicted
}

/// A top-level character body: the only kind that is blended.
type TopBody = (With<CharacterController>, Without<ChildOf>);

/// A child that can be adopted: it has a transform and is neither marked nor simulated.
type Adoptable = (With<Transform>, Without<Interpolated>, Without<NotInterpolated>, Without<AiId>);

/// `PostUpdate`: every unmarked drawn child of a character body gets [`Interpolated`]. Children
/// arrive after the body (a game dresses it once its collider exists), so this looks at each body
/// whose children changed.
pub fn adopt_visuals(
    mut commands: Commands,
    bodies: Query<&Children, (With<CharacterController>, Changed<Children>)>,
    candidates: Query<(), Adoptable>,
) {
    for children in &bodies {
        for child in children.iter().filter(|c| candidates.contains(*c)) {
            commands.entity(child).try_insert(Interpolated::default());
        }
    }
}

/// `FixedFirst`: a body the controller did not leave where it put it was placed by someone (a
/// teleport inside the previous frame's tools, a respawn): the history starts over at the end of
/// this tick. The controller consumes `MotionState::placed` during the tick, so this is the
/// last moment it can be seen.
fn note_placements(mut visuals: Query<(&mut Interpolated, &ChildOf)>, bodies: Query<(&Transform, &MotionState)>) {
    for (mut visual, parent) in &mut visuals {
        if let Ok((tf, state)) = bodies.get(parent.parent())
            && state.placed.is_some_and(|at| at != tf.translation)
        {
            visual.moved = true;
        }
    }
}

/// `FixedFirst`: put each blended child back to its rest transform for the length of the tick,
/// so simulation code that reads a child's local `Transform` sees what the game left, on or off.
/// (`blend` offsets it again in `PostUpdate`.) A child's `GlobalTransform` still holds last
/// frame's picture until transforms propagate: gameplay must not read it for a body child; give
/// such a child an `AiId` or [`NotInterpolated`].
fn restore_rest(mut visuals: Query<(&mut Interpolated, &mut Transform), Without<CharacterController>>) {
    for (mut visual, mut tf) in &mut visuals {
        if let Some(rest) = visual.rest
            && visual.applied == Some(*tf)
        {
            if *tf != rest {
                *tf = rest;
            }
            if visual.applied != Some(rest) {
                visual.applied = Some(rest);
            }
        }
    }
}

/// `FixedLast`: keep the pose each body ended the tick with.
fn capture(
    tick: Option<Res<SimTick>>,
    tuning: Option<Res<CameraTuning>>,
    role: Option<Res<NetRole>>,
    mut visuals: Query<(&mut Interpolated, &ChildOf)>,
    bodies: Query<(&Transform, Has<Predicted>), Without<ChildOf>>,
) {
    let snap = tuning.map_or(CameraTuning::default().body_snap_distance, |t| t.body_snap_distance);
    let tick = tick.map(|t| t.0);
    for (mut visual, parent) in &mut visuals {
        let Ok((tf, predicted)) = bodies.get(parent.parent()) else { continue };
        if applies(role.as_deref(), predicted) {
            visual.capture(Pose::of(tf), tick, snap);
        } else if visual.cur.is_some() {
            visual.reset();
        }
    }
}

/// `PostUpdate`, before transforms propagate: put each child where the blended pose draws it
/// (or back where the game left it, when nothing is blended).
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn blend(
    fixed: Res<Time<Fixed>>,
    virtual_time: Res<Time<Virtual>>,
    settings: Option<Res<GraphicsSettings>>,
    role: Option<Res<NetRole>>,
    mut visuals: Query<(&mut Interpolated, &mut Transform, &ChildOf), Without<CharacterController>>,
    bodies: Query<(&Transform, Has<Predicted>), TopBody>,
) {
    let on = settings.is_none_or(|s| s.interpolation);
    // A paused clock (and a `step`, which runs ticks without frames) shows the last tick: the
    // overstep left over is not time that is passing.
    let fraction = if virtual_time.is_paused() { 1.0 } else { fixed.overstep_fraction() };
    for (mut visual, mut tf, parent) in &mut visuals {
        let body = bodies.get(parent.parent()).ok();
        // Someone else moved the child (the game, a hot reload): that is its new rest.
        if visual.applied != Some(*tf) {
            visual.rest = Some(*tf);
            visual.applied = None;
        }
        let Some(rest) = visual.rest else { continue };
        let blended = match body {
            Some((body_tf, predicted)) if on && applies(role.as_deref(), predicted) => {
                // The body moved since the last tick ended: it was placed between ticks.
                if visual.cur.is_some_and(|cur| cur != Pose::of(body_tf)) {
                    let now = Pose::of(body_tf);
                    visual.reset();
                    visual.cur = Some(now);
                    visual.prev = Some(now);
                }
                visual.at(fraction).map(|shown| (body_tf, shown))
            }
            _ => None,
        };
        let shown = blended.map(|(_, shown)| shown);
        if visual.shown != shown {
            visual.shown = shown;
        }
        let next = match blended {
            Some((body_tf, shown)) => visual_offset(body_tf, shown, rest),
            None => rest,
        };
        if *tf != next {
            *tf = next;
        }
        if visual.applied != Some(next) {
            visual.applied = Some(next);
        }
    }
}

/// Where a followed body is drawn this frame: the blended pose of its first interpolated child,
/// or its own translation when nothing is blended. The camera follows this, not the tick value.
pub fn shown_translation(
    body: Entity,
    tf: &Transform,
    children: &Query<&Children>,
    visuals: &Query<&Interpolated>,
) -> Vec3 {
    children
        .get(body)
        .into_iter()
        .flatten()
        .find_map(|child| visuals.get(*child).ok().and_then(Interpolated::shown))
        .map_or(tf.translation, |pose| pose.translation)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::time::TimePlugin;
    use bevy::transform::TransformPlugin;

    use super::*;

    fn at(x: f32, turn: f32) -> Pose {
        Pose { translation: Vec3::new(x, 0.0, 0.0), rotation: Quat::from_rotation_y(turn) }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-4
    }

    #[test]
    fn the_pose_blend_is_the_older_pose_at_0_the_last_at_1_and_between_at_half() {
        let mut v = Interpolated::default();
        v.capture(at(0.0, 0.0), Some(1), 4.0);
        v.capture(at(1.0, 1.0), Some(2), 4.0);
        assert_eq!(v.at(0.0).unwrap().translation, Vec3::ZERO);
        assert_eq!(v.at(1.0).unwrap().translation, Vec3::X);
        let half = v.at(0.5).unwrap();
        assert!(near(half.translation, Vec3::new(0.5, 0.0, 0.0)));
        assert!(half.rotation.angle_between(Quat::from_rotation_y(0.5)) < 1e-3);
        // A fraction outside the tick is held to it.
        assert_eq!(v.at(2.0).unwrap().translation, Vec3::X);
    }

    #[test]
    fn the_first_capture_has_nothing_to_blend_from() {
        let mut v = Interpolated::default();
        assert!(v.at(0.5).is_none());
        v.capture(at(3.0, 0.0), Some(1), 4.0);
        assert_eq!(v.at(0.0).unwrap().translation, Vec3::new(3.0, 0.0, 0.0));
    }

    #[test]
    fn a_jump_a_gap_in_ticks_or_a_noted_placement_starts_the_history_over() {
        let mut v = Interpolated::default();
        v.capture(at(0.0, 0.0), Some(1), 4.0);
        v.capture(at(0.2, 0.0), Some(2), 4.0);
        // A teleport: farther than the snap distance in one tick.
        v.capture(at(50.0, 0.0), Some(3), 4.0);
        assert_eq!(v.at(0.0).unwrap().translation.x, 50.0);
        // A tick missing (a snapshot loaded, the setting was off).
        v.capture(at(50.2, 0.0), Some(4), 4.0);
        v.capture(at(50.4, 0.0), Some(9), 4.0);
        assert_eq!(v.at(0.0).unwrap().translation.x, 50.4);
        // A placement the controller noticed, however short.
        v.capture(at(50.6, 0.0), Some(10), 4.0);
        v.moved = true;
        v.capture(at(51.0, 0.0), Some(11), 4.0);
        assert_eq!(v.at(0.0).unwrap().translation.x, 51.0);
        // And walking on blends again.
        v.capture(at(51.2, 0.0), Some(12), 4.0);
        assert_eq!(v.at(0.0).unwrap().translation.x, 51.0);
    }

    #[test]
    fn the_offset_puts_the_child_where_the_blended_body_would_carry_it() {
        // The body turned a quarter and stands at x = 10; the child rides 2 m in front of it and
        // faces away. Shown half way back, the child's world place must follow the shown pose.
        let body = Transform::from_xyz(10.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(1.0));
        let shown = at(9.0, 0.5);
        let rest = Transform::from_xyz(0.0, 0.5, -2.0).with_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        let local = visual_offset(&body, shown, rest);
        let drawn = body.mul_transform(local);
        let wanted = Transform { translation: shown.translation, rotation: shown.rotation, scale: Vec3::ONE }.mul_transform(rest);
        assert!(near(drawn.translation, wanted.translation));
        assert!(drawn.rotation.angle_between(wanted.rotation) < 1e-3);
        // Shown at the body's own pose: the child stays at rest.
        let same = visual_offset(&body, Pose::of(&body), rest);
        assert!(near(same.translation, rest.translation));
        assert!(same.rotation.angle_between(rest.rotation) < 1e-3);
    }

    fn rig(settings: Option<GraphicsSettings>) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((TaskPoolPlugin::default(), TimePlugin, TransformPlugin));
        if let Some(settings) = settings {
            app.insert_resource(settings);
        }
        plugin(&mut app);
        let body = app.world_mut().spawn((CharacterController::default(), Transform::default())).id();
        let child = app.world_mut().spawn((Transform::from_xyz(0.0, 1.0, 0.0), ChildOf(body))).id();
        // The first frame adopts the child.
        app.world_mut().run_schedule(PostUpdate);
        (app, body, child)
    }

    /// One tick: the body moves, `FixedLast` runs, then `overstep` ms of the next are pending.
    fn tick(app: &mut App, body: Entity, x: f32, overstep_ms: u64) {
        app.world_mut().get_mut::<Transform>(body).unwrap().translation.x = x;
        app.world_mut().run_schedule(FixedLast);
        let mut fixed = app.world_mut().resource_mut::<Time<Fixed>>();
        let pending = fixed.overstep();
        fixed.discard_overstep(pending);
        fixed.accumulate_overstep(Duration::from_millis(overstep_ms));
    }

    fn world_x(app: &App, child: Entity) -> f32 {
        app.world().get::<GlobalTransform>(child).unwrap().translation().x
    }

    #[test]
    fn between_ticks_the_child_is_drawn_between_the_poses_and_the_body_is_never_touched() {
        let (mut app, body, child) = rig(None);
        tick(&mut app, body, 0.0, 0);
        tick(&mut app, body, 1.0, 0);
        // The default fixed step is 1/64 s = 15.625 ms: half of it.
        let half = (Time::<Fixed>::default().timestep().as_micros() / 2) as u64;
        app.world_mut().resource_mut::<Time<Fixed>>().accumulate_overstep(Duration::from_micros(half));
        app.world_mut().run_schedule(PostUpdate);
        assert!((world_x(&app, child) - 0.5).abs() < 1e-3, "{}", world_x(&app, child));
        assert_eq!(app.world().get::<Transform>(body).unwrap().translation.x, 1.0);
        let shown = app.world().get::<Interpolated>(child).unwrap().shown().unwrap();
        assert!((shown.translation.x - 0.5).abs() < 1e-3);
        // Nothing owed on the next tick: the last tick's pose.
        let pending = app.world().resource::<Time<Fixed>>().overstep();
        app.world_mut().resource_mut::<Time<Fixed>>().discard_overstep(pending);
        app.world_mut().run_schedule(PostUpdate);
        assert!((world_x(&app, child) - 0.0).abs() < 1e-3, "fraction 0 draws the older pose");
    }

    #[test]
    fn a_teleport_between_ticks_resets_at_once_and_one_inside_a_tick_does_not_smear() {
        let (mut app, body, child) = rig(None);
        tick(&mut app, body, 0.0, 8);
        tick(&mut app, body, 0.1, 8);
        app.world_mut().run_schedule(PostUpdate);
        // Between ticks: the body is somewhere no tick left it.
        app.world_mut().get_mut::<Transform>(body).unwrap().translation.x = 100.0;
        app.world_mut().run_schedule(PostUpdate);
        assert!((world_x(&app, child) - 100.0).abs() < 1e-3);
        // Inside a tick: farther than the snap distance.
        tick(&mut app, body, 200.0, 8);
        app.world_mut().run_schedule(PostUpdate);
        assert!((world_x(&app, child) - 200.0).abs() < 1e-3, "{}", world_x(&app, child));
    }

    #[test]
    fn with_the_setting_off_the_child_is_exactly_where_the_game_left_it() {
        let (mut app, body, child) = rig(None);
        tick(&mut app, body, 0.0, 8);
        tick(&mut app, body, 1.0, 8);
        app.world_mut().run_schedule(PostUpdate);
        assert!((world_x(&app, child) - 1.0).abs() > 0.01, "on, it is behind the tick's pose");
        app.insert_resource(GraphicsSettings { interpolation: false, ..default() });
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(*app.world().get::<Transform>(child).unwrap(), Transform::from_xyz(0.0, 1.0, 0.0));
        assert!(app.world().get::<Interpolated>(child).unwrap().shown().is_none());
        // And on again: the rest is still the game's.
        app.insert_resource(GraphicsSettings::default());
        tick(&mut app, body, 1.5, 8);
        tick(&mut app, body, 2.0, 8);
        app.world_mut().run_schedule(PostUpdate);
        assert!(app.world().get::<Interpolated>(child).unwrap().shown().is_some());
    }

    #[test]
    fn a_paused_clock_draws_the_last_tick() {
        let (mut app, body, child) = rig(None);
        tick(&mut app, body, 0.0, 8);
        tick(&mut app, body, 1.0, 8);
        app.world_mut().resource_mut::<Time<Virtual>>().pause();
        app.world_mut().run_schedule(PostUpdate);
        assert!((world_x(&app, child) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_client_blends_only_the_body_it_predicts() {
        let (mut app, body, child) = rig(None);
        app.insert_resource(NetRole::Client);
        tick(&mut app, body, 0.0, 8);
        tick(&mut app, body, 1.0, 8);
        app.world_mut().run_schedule(PostUpdate);
        assert!(app.world().get::<Interpolated>(child).unwrap().shown().is_none(), "a remote body is shown by the net's interpolation");
        assert_eq!(*app.world().get::<Transform>(child).unwrap(), Transform::from_xyz(0.0, 1.0, 0.0));
        app.world_mut().entity_mut(body).insert(Predicted);
        tick(&mut app, body, 1.5, 8);
        tick(&mut app, body, 2.0, 8);
        app.world_mut().run_schedule(PostUpdate);
        assert!(app.world().get::<Interpolated>(child).unwrap().shown().is_some());
    }

    #[test]
    fn children_are_adopted_unless_they_opt_out_or_are_simulated() {
        let (mut app, body, child) = rig(None);
        let skip = app.world_mut().spawn((Transform::default(), NotInterpolated, ChildOf(body))).id();
        app.world_mut().run_schedule(PostUpdate);
        assert!(app.world().get::<Interpolated>(child).is_some());
        assert!(app.world().get::<Interpolated>(skip).is_none());
    }

    #[test]
    fn a_placement_noted_before_the_tick_starts_the_history_over() {
        let (mut app, body, child) = rig(None);
        app.world_mut().entity_mut(body).insert(MotionState::default());
        tick(&mut app, body, 0.0, 8);
        tick(&mut app, body, 0.1, 8);
        // Placed 1 m away (under the snap distance), noted in FixedFirst, then the tick ends there.
        app.world_mut().get_mut::<MotionState>(body).unwrap().placed = Some(Vec3::new(1.0, 0.0, 0.0));
        app.world_mut().get_mut::<Transform>(body).unwrap().translation.x = 1.0;
        app.world_mut().get_mut::<MotionState>(body).unwrap().placed = Some(Vec3::new(0.5, 0.0, 0.0));
        app.world_mut().run_schedule(FixedFirst);
        assert!(app.world().get::<Interpolated>(child).unwrap().moved);
        tick(&mut app, body, 1.1, 8);
        let v = app.world().get::<Interpolated>(child).unwrap();
        assert_eq!(v.previous().unwrap().translation.x, 1.1);
    }

    #[test]
    fn a_child_that_opts_out_stays_at_rest_and_a_tick_reads_the_rest_transform() {
        let (mut app, body, child) = rig(None);
        let skip = app.world_mut().spawn((Transform::from_xyz(0.0, 2.0, 0.0), NotInterpolated, ChildOf(body))).id();
        tick(&mut app, body, 0.0, 8);
        tick(&mut app, body, 1.0, 8);
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(*app.world().get::<Transform>(skip).unwrap(), Transform::from_xyz(0.0, 2.0, 0.0));
        assert_ne!(*app.world().get::<Transform>(child).unwrap(), Transform::from_xyz(0.0, 1.0, 0.0));
        // The simulation's view: FixedFirst puts the blended child back at rest.
        app.world_mut().run_schedule(FixedFirst);
        assert_eq!(*app.world().get::<Transform>(child).unwrap(), Transform::from_xyz(0.0, 1.0, 0.0));
        app.world_mut().run_schedule(PostUpdate);
        assert_ne!(*app.world().get::<Transform>(child).unwrap(), Transform::from_xyz(0.0, 1.0, 0.0));
    }

    #[test]
    fn a_child_despawned_in_the_frame_it_would_be_adopted_does_not_panic() {
        let (mut app, body, _child) = rig(None);
        let late = app.world_mut().spawn((Transform::default(), ChildOf(body))).id();
        app.add_systems(PostUpdate, (move |mut c: Commands| { c.entity(late).despawn(); }).before(adopt_visuals));
        app.world_mut().run_schedule(PostUpdate);
    }
}
