//! Kinematic character controller: the only thing that moves characters.
//!
//! Gameplay writes a [`LocomotionInput`] before `SimSet::Move` (the next step fills it
//! from `Intent` and the action machine); [`move_characters`] turns it into motion with
//! shape casts against the world ([`PhysicsWorld`]) and keeps the result in
//! [`MotionState`]. Actions never move a body themselves: a dodge roll is
//! [`LocomotionInput::external`], integrated here with everything else.
//!
//! Modes ([`MotionMode`]): **Ground** walks, runs and sprints, follows slopes up to
//! `max_slope_deg`, steps onto obstacles up to `step_height` and snaps down stairs;
//! **Air** falls with gravity and some air control; **Glide** falls slowly and rides
//! updrafts; **Water** floats at the surface; **Mantle** climbs onto a ledge along a
//! path checked before it starts. Characters never collide with each other through
//! casts: after all of them moved, a separation pass pushes overlapping capsules apart.
//!
//! The transform's translation is the character's feet.

mod locomotion;
mod mover;
mod params;

use ai_core::{label, AiAppExt, AiId, SimSet};
use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use params::CharacterParams;

use crate::physics::{
    Body, Collider, ColliderId, PhysicsPlugin, PhysicsWorld, VolumeEntered, VolumeExited,
};
use locomotion::{Note, Step};
use mover::{flat, Mover};

/// Makes an entity a character moved by this controller. Its [`Collider`] is the capsule
/// `params` describe (kept in sync, layers left alone); its transform is at the feet.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
#[require(Collider = CharacterParams::default().collider(), Body = Body::Kinematic, LocomotionInput, MotionState)]
#[component(on_insert = fit_collider)]
pub struct CharacterController {
    pub params: CharacterParams,
}

impl CharacterController {
    pub fn new(params: CharacterParams) -> Self {
        Self { params }
    }
}

/// The capsule follows the params from the first moment, so queries between spawn
/// and the first tick already see the right body.
fn fit_collider(mut world: DeferredWorld, ctx: HookContext) {
    let Some(want) = world.get::<CharacterController>(ctx.entity).map(|c| c.params.collider()) else {
        return;
    };
    if let Some(mut collider) = world.get_mut::<Collider>(ctx.entity) {
        fit(&mut collider, &want);
    }
}

fn fit(collider: &mut Mut<Collider>, want: &Collider) {
    if collider.shape != want.shape || collider.offset != want.offset {
        collider.shape = want.shape;
        collider.offset = want.offset;
    }
}

/// What the character wants to do this tick. Written by gameplay before
/// `SimSet::Move`; `jump` and `external` are consumed by the controller.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct LocomotionInput {
    /// Desired movement on the ground plane, length ≤ 1: 0.5 walks, 1 runs.
    pub move_dir: Vec3,
    /// Held: run faster on the ground (needs more than walking input).
    pub sprint: bool,
    /// Pressed this tick. Buffered for `jump_buffer`, then forgotten.
    pub jump: bool,
    /// Held: glide while falling.
    pub glide: bool,
    /// Turn toward this direction; `None` faces where the character moves.
    pub face: Option<Vec3>,
    /// World-space displacement an action asks for this tick (a dodge roll, a lunge).
    pub external: Vec3,
    /// An action owns the body: no walking, jumping, gliding or climbing; only
    /// `external` and gravity move it.
    pub locked: bool,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum MotionMode {
    #[default]
    Ground,
    Air,
    Water,
    Glide,
    Mantle,
}

/// A climb onto a ledge: straight up from `from` to `corner`, then over to `to`.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Mantle {
    pub from: Vec3,
    pub corner: Vec3,
    pub to: Vec3,
    /// Progress, 0..1.
    pub t: f32,
    /// Seconds for the whole climb.
    pub duration: f32,
}

/// How a character is moving: gameplay state, kept in replays.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct MotionState {
    pub mode: MotionMode,
    /// m/s. On the ground only the horizontal part (slopes add the vertical one).
    pub velocity: Vec3,
    pub grounded: bool,
    /// The walkable surface stood on (`Y` when not grounded).
    pub ground_normal: Vec3,
    /// Seconds a jump still works after walking off a ledge.
    pub coyote_left: f32,
    /// Seconds the last jump press stays valid.
    pub jump_buffer_left: f32,
    pub mantle: Option<Mantle>,
    /// Highest feet height since leaving the ground; `None` on the ground.
    pub fall_from_y: Option<f32>,
    pub sprinting: bool,
    /// Surface of the water the feet are in.
    pub water_surface: Option<f32>,
    /// `(accel, max_speed)` of the updraft the character is in.
    pub updraft: Option<Vec2>,
    /// Volumes the capsule touches, in collider id order.
    pub volumes: Vec<Entity>,
}

impl Default for MotionState {
    fn default() -> Self {
        Self {
            mode: MotionMode::Ground,
            velocity: Vec3::ZERO,
            grounded: false,
            ground_normal: Vec3::Y,
            coyote_left: 0.0,
            jump_buffer_left: 0.0,
            mantle: None,
            fall_from_y: None,
            sprinting: false,
            water_surface: None,
            updraft: None,
            volumes: Vec::new(),
        }
    }
}

/// Touched walkable ground after being airborne. `fall_height` counts from the highest
/// point since leaving the ground, gliding included: base fall damage on
/// `impact_speed` (m/s downward), which a glider keeps low from any height.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct Landed {
    pub entity: Entity,
    pub fall_height: f32,
    pub impact_speed: f32,
}

#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModeChanged {
    pub entity: Entity,
    pub from: MotionMode,
    pub to: MotionMode,
}

#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Jumped {
    pub entity: Entity,
}

/// Started climbing onto a ledge `height` meters above the feet.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct Mantled {
    pub entity: Entity,
    pub height: f32,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControllerSystems {
    /// Inside `SimSet::Move`: [`move_characters`].
    Move,
}

/// The character controller (adds [`PhysicsPlugin`] if it is missing).
pub struct ControllerPlugin;

impl Plugin for ControllerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PhysicsPlugin>() {
            app.add_plugins(PhysicsPlugin);
        }
        app.register_type::<CharacterParams>()
            .register_type::<CharacterController>()
            .register_type::<LocomotionInput>()
            .register_type::<MotionMode>()
            .register_type::<Mantle>()
            .register_type::<MotionState>()
            .configure_sets(FixedUpdate, ControllerSystems::Move.in_set(SimSet::Move))
            .add_systems(FixedUpdate, move_characters.in_set(ControllerSystems::Move))
            .register_ai_component::<MotionState>(describe_motion)
            .journal_event::<Landed>(|e, w| {
                (e.fall_height >= 0.5).then(|| tracked(w, e.entity)).flatten().map(|who| {
                    format!("{who} landed: fell {:.1} m at {:.1} m/s", e.fall_height, e.impact_speed)
                })
            })
            .journal_event::<Jumped>(|e, w| tracked(w, e.entity).map(|who| format!("{who} jumped")))
            .journal_event::<Mantled>(|e, w| {
                tracked(w, e.entity).map(|who| format!("{who} climbs a {:.1} m ledge", e.height))
            })
            .journal_event::<ModeChanged>(|e, w| {
                let notable = |m: MotionMode| matches!(m, MotionMode::Water | MotionMode::Glide);
                (notable(e.from) || notable(e.to))
                    .then(|| tracked(w, e.entity))
                    .flatten()
                    .map(|who| format!("{who} {} → {}", mode_name(e.from), mode_name(e.to)))
            })
            .journal_event::<VolumeEntered>(|e, w| {
                tracked(w, e.entity).map(|who| format!("{who} entered {}", volume_name(w, e.volume)))
            })
            .journal_event::<VolumeExited>(|e, w| {
                tracked(w, e.entity).map(|who| format!("{who} left {}", volume_name(w, e.volume)))
            });
    }
}

/// `e5` for entities the AI tracks; journal lines about the rest are noise.
fn tracked(world: &World, entity: Entity) -> Option<String> {
    world.get::<AiId>(entity).is_some().then(|| label(world, entity))
}

fn volume_name(world: &World, volume: Entity) -> String {
    let kind = world.get_resource::<PhysicsWorld>().and_then(|p| p.collider(volume)).and_then(|c| c.volume.clone());
    match (kind, world.get::<AiId>(volume)) {
        (Some(kind), Some(id)) => format!("{kind} {id}"),
        (Some(kind), None) => kind.to_string(),
        (None, _) => "a volume".to_owned(),
    }
}

fn mode_name(mode: MotionMode) -> &'static str {
    match mode {
        MotionMode::Ground => "ground",
        MotionMode::Air => "air",
        MotionMode::Water => "water",
        MotionMode::Glide => "glide",
        MotionMode::Mantle => "climbing",
    }
}

/// `run 5.0m/s`, `air 4.0m/s ↓7.2`, `swimming 2.0m/s`, `gliding 9.0m/s ↓2.0`,
/// `climbing 40%`; nothing while standing still.
pub fn describe_motion(s: &MotionState) -> String {
    let speed = flat(s.velocity).length();
    let vertical = if s.velocity.y >= 0.0 { format!("↑{:.1}", s.velocity.y) } else { format!("↓{:.1}", -s.velocity.y) };
    match s.mode {
        MotionMode::Ground if speed < 0.05 => String::new(),
        MotionMode::Ground if s.sprinting => format!("sprint {speed:.1}m/s"),
        MotionMode::Ground => format!("moving {speed:.1}m/s"),
        MotionMode::Air => format!("air {speed:.1}m/s {vertical}"),
        MotionMode::Glide => format!("gliding {speed:.1}m/s {vertical}"),
        MotionMode::Water => format!("swimming {speed:.1}m/s"),
        MotionMode::Mantle => format!("climbing {:.0}%", s.mantle.map_or(0.0, |m| m.t * 100.0)),
    }
}

/// Bundle-free spawning: `commands.spawn(character(params, at))`.
pub fn character(params: CharacterParams, feet: Vec3) -> (CharacterController, Transform) {
    (CharacterController::new(params), Transform::from_translation(feet))
}

type CharacterQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ColliderId,
        &'static CharacterController,
        &'static mut LocomotionInput,
        &'static mut MotionState,
        &'static mut Transform,
        &'static mut Collider,
    ),
>;

/// Where a character ended up, for the separation pass.
struct Placed {
    entity: Entity,
    feet: Vec3,
    radius: f32,
    height: f32,
    /// Climbing: follows its checked path, others give way.
    pinned: bool,
}

/// Moves every character one tick, in [`ColliderId`] order, then pushes overlapping
/// characters apart.
pub fn move_characters(
    time: Res<Time>,
    physics: Res<PhysicsWorld>,
    mut commands: Commands,
    mut characters: CharacterQuery,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut order: Vec<(ColliderId, Entity)> = characters.iter().map(|(e, id, ..)| (*id, e)).collect();
    order.sort_unstable();
    let mut placed = Vec::with_capacity(order.len());
    for &(_, entity) in &order {
        let Ok((_, _, controller, mut input, mut state, mut tf, mut collider)) = characters.get_mut(entity) else {
            continue;
        };
        let params = &controller.params;
        fit(&mut collider, &params.collider());
        let now = sanitized(&input);
        if input.jump || input.external != Vec3::ZERO {
            input.jump = false;
            input.external = Vec3::ZERO;
        }
        if !tf.translation.is_finite() {
            warn!("{entity}: character at {} cannot move; teleport it somewhere real", tf.translation);
            continue;
        }
        let mover = Mover::new(&physics, params, entity, collider.layers);
        let facing = flat(*tf.forward()).try_normalize().unwrap_or(Vec3::NEG_Z);
        let mut step = Step { mover: &mover, params, input: &now, state: &mut state, dt, facing, notes: Vec::new() };
        let feet = step.run(tf.translation);
        let notes = std::mem::take(&mut step.notes);
        if tf.translation != feet {
            tf.translation = feet;
        }
        turn(&mut tf, look_dir(&now, &state), params.turn_speed_deg, dt);
        for note in notes {
            trigger(&mut commands, entity, note);
        }
        let pinned = state.mode == MotionMode::Mantle;
        placed.push(Placed { entity, feet, radius: mover.radius, height: params.height.max(2.0 * mover.radius), pinned });
    }
    separate(&physics, &mut characters, &placed);
}

/// The input with anything non-finite dropped: a NaN direction from a buggy brain must
/// not poison the position (and the collision queries behind it).
fn sanitized(input: &LocomotionInput) -> LocomotionInput {
    let finite = |v: Vec3| if v.is_finite() { v } else { Vec3::ZERO };
    LocomotionInput {
        move_dir: finite(input.move_dir),
        face: input.face.filter(|f| f.is_finite()),
        external: finite(input.external),
        ..input.clone()
    }
}

fn trigger(commands: &mut Commands, entity: Entity, note: Note) {
    match note {
        Note::Mode(from, to) => commands.trigger(ModeChanged { entity, from, to }),
        Note::Jumped => commands.trigger(Jumped { entity }),
        Note::Landed { fall_height, impact_speed } => commands.trigger(Landed { entity, fall_height, impact_speed }),
        Note::Mantled { height } => commands.trigger(Mantled { entity, height }),
        Note::Entered(volume) => commands.trigger(VolumeEntered { entity, volume }),
        Note::Exited(volume) => commands.trigger(VolumeExited { entity, volume }),
    }
}

/// Where to face: the input's `face`, else the climb, else the way the character moves.
fn look_dir(input: &LocomotionInput, state: &MotionState) -> Option<Vec3> {
    if let Some(face) = input.face.and_then(|f| flat(f).try_normalize()) {
        return Some(face);
    }
    if let Some(climb) = state.mantle {
        return flat(climb.to - climb.from).try_normalize();
    }
    let moving = flat(state.velocity);
    (!input.locked && moving.length() > 0.1).then(|| moving.normalize())
}

/// Turns the transform about Y toward `dir` at `speed_deg` per second (0: at once).
fn turn(tf: &mut Mut<Transform>, dir: Option<Vec3>, speed_deg: f32, dt: f32) {
    let Some(dir) = dir else {
        return;
    };
    let forward = tf.forward();
    let yaw = |d: Vec3| (-d.x).atan2(-d.z);
    let (current, target) = (yaw(*forward), yaw(dir));
    let mut delta = target - current;
    delta = (delta + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if delta.abs() < 1e-5 && forward.y.abs() < 1e-5 {
        return;
    }
    let max = if speed_deg > 0.0 { speed_deg.to_radians() * dt } else { f32::INFINITY };
    tf.rotation = Quat::from_rotation_y(current + delta.clamp(-max, max));
}

/// Overlapping capsules (sharing some height) are pushed apart on the ground plane,
/// half each, in a fixed pair order; the pushes go through move-and-slide so nobody
/// ends up inside a wall.
fn separate(physics: &PhysicsWorld, characters: &mut CharacterQuery, placed: &[Placed]) {
    let mut push = vec![Vec3::ZERO; placed.len()];
    for i in 0..placed.len() {
        for j in (i + 1)..placed.len() {
            let (a, b) = (&placed[i], &placed[j]);
            if a.feet.y >= b.feet.y + b.height || b.feet.y >= a.feet.y + a.height {
                continue;
            }
            let d = flat(b.feet - a.feet);
            let dist = d.length();
            let overlap = a.radius + b.radius - dist;
            if overlap <= 0.0 {
                continue;
            }
            let n = if dist > 1e-5 { d / dist } else { Vec3::X };
            let (wa, wb) = match (a.pinned, b.pinned) {
                (true, true) => continue,
                (true, false) => (0.0, 1.0),
                (false, true) => (1.0, 0.0),
                (false, false) => (0.5, 0.5),
            };
            push[i] -= n * overlap * wa;
            push[j] += n * overlap * wb;
        }
    }
    for (body, push) in placed.iter().zip(push) {
        if push == Vec3::ZERO {
            continue;
        }
        let Ok((_, _, controller, _, _, mut tf, collider)) = characters.get_mut(body.entity) else {
            continue;
        };
        let mover = Mover::new(physics, &controller.params, body.entity, collider.layers);
        let feet = mover.depenetrate(mover.slide(body.feet, push, true).feet);
        if tf.translation != feet {
            tf.translation = feet;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_is_short_and_silent_when_idle() {
        assert_eq!(describe_motion(&MotionState::default()), "");
        let s = MotionState { mode: MotionMode::Glide, velocity: Vec3::new(9.0, -2.0, 0.0), ..default() };
        assert_eq!(describe_motion(&s), "gliding 9.0m/s ↓2.0");
    }
}
