//! Movers (F6): doors, lifts and platforms that go through keys, carry what stands on them and
//! push what is in their way.
//!
//! A mover is an entity with [`Mover`]: the `mover` definition it moves by (content kind `mover`,
//! `*.mover.ron`, [`MoverDef`]) and, when it has one, its own collider. Other entities can move with
//! it as its [`MoverPart`]s (the children of a level node with `mover=<id>`: the level module puts
//! them there). Its state is a [`MoverState`] on the mover, which the engine adds on its first tick:
//! where it was put (keys are measured from there, in that frame), the key it rests at or left, the key
//! it heads for and how far along, a key it was sent to, how long it still waits, how long a body
//! has held it. A mover and its parts carry an `AiId` (they are in snapshots, in co-op and in
//! the replay hash through the state; a part, which is plumbing, is not `AiVisible`).
//!
//! **The tick** ([`MoverSystems::Step`], in `SimSet::Move` before `ControllerSystems::Move`):
//! first every character standing on the ground is asked what it stands on (the controller's own
//! ground probe, at the start of the tick); then each mover, in `AiId` order, takes its step:
//!
//! 1. Its state runs `dt` on (rests out a wait, leaves for the next key, passes keys where it does
//!    not stop, arrives where it does) and gives the pose it would stand in.
//! 2. Its colliders go there in the physics world, and a body that stood on one of them is carried by
//!    the mover's motion this tick (its translation, and for a turning mover the turn about the
//!    mover's pivot, the body's yaw turned with it), sliding against the rest of the world.
//! 3. A body the mover now sinks into (its sides, a closing door, a lift from below) is pushed out the
//!    shortest way, sliding against the world.
//! 4. A body that cannot be pushed out (against a wall, pinned under it) blocks the step: the mover
//!    stays where it was, and its `blocked` rule says what then (`Stop`, `Reverse`, `Crush`, see
//!    [`Blocked`]), with a [`MoverBlocked`]. Otherwise the pose, the carried and pushed bodies
//!    (their `MotionState::placed` too, so the controller goes on from there) and the state are
//!    written, with [`MoverStarted`] and [`MoverArrived`].
//!
//! A body keeps only its own velocity: stepping or jumping off a mover, it takes none of the
//! mover's along. Movers push characters only: they pass through static geometry, other movers
//! and everything else. While a mover rests its colliders are `Body::Static` (a closed door is cut
//! out of the navmesh at its next refresh, like any laid solid), while it moves `Body::Kinematic`
//! (out of the navmesh, the way kinematic colliders are). A plain kinematic collider without
//! `Mover` behaves as before: it carries and pushes nothing.
//!
//! **Co-op.** The host moves movers; a client never writes their state (it is replicated, so the
//! desync hash compares it). A client predicts its own body, and on a mover it does that against
//! the mover where it will be at the predicted tick ([`MoverSystems::Predict`], in `NetSet::Predict`
//! before the predicted move): it runs the replicated state on by the ticks between the host's tick
//! and its own (at most two seconds' worth), puts the mover's colliders there in its physics world
//! only, and carries and pushes its own body as the host does. The host stays authoritative: a call
//! or a block the client does not know of yet is mended by the usual correction.

mod ai;
mod def;
mod run;
mod step;

use ai_core::prelude::*;
use ai_core::NoTombstone;
use bevy::prelude::*;
use shards_content::{ContentAppExt, ContentDb};
use shards_core::{EngineAppExt, NetRole, NetSet};

pub use ai::mover_sane;
pub use def::{Blocked, MoverDef, MoverKey, MoverLoop, MAX_KEYS, MIN_WAY};

use crate::controller::ControllerSystems;

/// The engine rule that holds every mover's state to its definition.
pub const MOVER_SANE: &str = "motion.mover_sane";

/// A mover: it goes through the keys of a `mover` definition ([`MoverDef`]), with its own collider
/// when it has one and its [`MoverPart`]s. A level node with `mover=<id>` makes one; so does a
/// prefab with `"Mover": (def: "lift")`.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[require(AiKind = AiKind::new("Mover"))]
pub struct Mover {
    /// The `mover` definition it moves by.
    pub def: String,
}

/// An entity that moves with a mover (a level node's child): it stands where `offset` puts it
/// in the mover's frame. It carries an `AiId` (snapshots, co-op), no tombstone and no `AiVisible`.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[require(AiId, NoTombstone)]
pub struct MoverPart {
    /// The mover.
    pub mover: AiId,
    /// Its pose relative to the mover's.
    pub offset: Transform,
}

/// Where a mover is on its way: the simulation's state of it, on the mover. Added by the engine on
/// the mover's first tick (from where it stands then), saved in snapshots, replicated in co-op.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct MoverState {
    /// Where it was put: its keys are measured from here...
    pub origin: Vec3,
    /// ...and in this frame.
    pub origin_rotation: Quat,
    /// The key it rests at, or the one it left.
    pub from: u32,
    /// The key it heads for: `from` while it rests.
    pub to: u32,
    /// How far along the way from `from` to `to` (0..1).
    pub t: f32,
    /// A key it was sent to and has not reached yet.
    pub goal: Option<u32>,
    /// Its last way went up the keys (a `PingPong` goes on that way).
    pub up: bool,
    /// Seconds it still rests at `from`.
    pub wait: f32,
    /// Seconds in a row a body it could not push has held it; 0 while it is free.
    pub blocked: f32,
    /// It came to rest at a key this tick (the condition `mover_arrived`).
    pub arrived: bool,
}

impl Default for MoverState {
    fn default() -> Self {
        Self {
            origin: Vec3::ZERO,
            origin_rotation: Quat::IDENTITY,
            from: 0,
            to: 0,
            t: 0.0,
            goal: None,
            up: true,
            wait: 0.0,
            blocked: 0.0,
            arrived: false,
        }
    }
}

impl MoverState {
    /// A mover put at `at`, resting at key 0.
    pub fn at(at: &Transform) -> Self {
        Self { origin: at.translation, origin_rotation: at.rotation.normalize(), ..default() }
    }

    /// Whether it rests at a key.
    pub fn resting(&self) -> bool {
        self.from == self.to
    }
}

/// A mover left the key it rested at.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct MoverStarted {
    pub mover: Entity,
    pub from: u32,
    pub to: u32,
}

/// A mover came to rest at a key.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct MoverArrived {
    pub mover: Entity,
    pub key: u32,
}

/// A body was in a mover's way and could not be pushed out of it, so the mover did not move this
/// tick. Triggered when the hold begins, and for a `Crush` mover again every `every` seconds it
/// lasts, with the damage to deal (`shards_action` deals it; 0 for `Stop` and `Reverse`).
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct MoverBlocked {
    pub mover: Entity,
    pub body: Entity,
    pub damage: f32,
}

/// Where movers run in `FixedUpdate`.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoverSystems {
    /// The host's step: `SimSet::Move`, before `ControllerSystems::Move`.
    Step,
    /// A client's prediction of its own body on movers: `NetSet::Predict`, after
    /// `MotionSystems::PredictIntent`, before `ControllerSystems::PredictMove`.
    Predict,
}

/// What [`mover_state`] tells of a mover.
#[derive(Clone, Debug, PartialEq)]
pub struct MoverInfo {
    /// Its definition's id.
    pub def: String,
    /// How many keys the definition has (0 when it is not in content).
    pub keys: u32,
    /// The key it rests at or left.
    pub key: u32,
    /// The key it heads for, while it moves.
    pub next: Option<u32>,
    /// How far along the way to `next` (0..1).
    pub progress: f32,
    /// A key it was sent to and has not reached.
    pub goal: Option<u32>,
    /// Its velocity now (m/s, world space); zero at rest.
    pub velocity: Vec3,
    /// How fast it turns now (degrees per second, the largest of its three angles).
    pub turn_rate: f32,
    /// Seconds it still rests at its key.
    pub wait: f32,
    /// Seconds a body has held it, 0 while free.
    pub blocked: f32,
    /// Seconds until it reaches its goal (or the key it heads for), by its definition's times.
    pub eta: f32,
}

/// What a mover is doing, or `None` for an entity that is no mover. A mover that has not had its
/// first tick rests at key 0.
pub fn mover_state(world: &World, mover: Entity) -> Option<MoverInfo> {
    let def_id = world.get::<Mover>(mover)?.def.clone();
    let state = match world.get::<MoverState>(mover) {
        Some(state) => state.clone(),
        None => MoverState::at(world.get::<Transform>(mover)?),
    };
    let def = world.get_resource::<ContentDb>().and_then(|db| db.get::<MoverDef>(&def_id)).cloned();
    Some(info(def_id, def.as_ref(), &state))
}

fn info(def_id: String, def: Option<&MoverDef>, s: &MoverState) -> MoverInfo {
    let moving = !s.resting();
    let keys = def.and_then(|def| Some((def, def.keys.get(s.from as usize)?, def.keys.get(s.to as usize)?)));
    let (velocity, turn_rate, eta) = match keys {
        Some((def, a, b)) => {
            let duration = def.duration(s.from, s.to);
            let (velocity, turn) = if moving && duration > 0.0 {
                (s.origin_rotation * (b.at - a.at) / duration, (b.turn - a.turn).abs().max_element() / duration)
            } else {
                (Vec3::ZERO, 0.0)
            };
            (velocity, turn, eta(def, s))
        }
        None => (Vec3::ZERO, 0.0, 0.0),
    };
    MoverInfo {
        def: def_id,
        keys: def.map_or(0, MoverDef::len),
        key: s.from,
        next: moving.then_some(s.to),
        progress: s.t,
        goal: s.goal,
        velocity,
        turn_rate,
        wait: s.wait,
        blocked: s.blocked,
        eta,
    }
}

/// Seconds to the goal (or to the key it heads for), along the ways between, without waits: 0 for
/// a mover at rest that is sent nowhere.
fn eta(def: &MoverDef, s: &MoverState) -> f32 {
    let (mut at, mut total) = if s.resting() { (s.from, 0.0) } else { (s.to, (1.0 - s.t) * def.duration(s.from, s.to)) };
    if let Some(goal) = s.goal {
        while at != goal && at < def.len() && goal < def.len() {
            let next = if goal > at { at + 1 } else { at - 1 };
            total += def.duration(at, next);
            at = next;
        }
    }
    total
}

/// Sends `mover` to `key`: it goes there at its own pace, through the keys between, and rests (see
/// [`MoverLoop`]; a looping mover takes up its loop again after the key's wait). A mover on its way
/// turns round when `key` is behind it. Returns what it does now. Refused on a machine that does not
/// simulate (a co-op client: the host moves movers), for a key the definition does not have, and
/// for an entity that is no mover.
pub fn mover_go(world: &mut World, mover: Entity, key: u32) -> Result<MoverInfo, String> {
    if world.get_resource::<NetRole>().is_some_and(|r| !r.simulates()) {
        return Err("movers move on the host: a co-op client cannot send one".into());
    }
    let Some(def_id) = world.get::<Mover>(mover).map(|m| m.def.clone()) else {
        return Err(format!("{} is no mover", ai_core::label(world, mover)));
    };
    let Some(def) = world.get_resource::<ContentDb>().and_then(|db| db.get::<MoverDef>(&def_id)).cloned() else {
        return Err(format!("{} moves by `{def_id}`, which content does not have (content kind `mover`)", ai_core::label(world, mover)));
    };
    if key >= def.len() {
        return Err(format!("no key {key}: `{def_id}` has keys 0..={}", def.len().saturating_sub(1)));
    }
    let mut state = match world.get::<MoverState>(mover) {
        Some(state) => state.clone(),
        None => MoverState::at(world.get::<Transform>(mover).ok_or("the mover has no Transform")?),
    };
    run::send(&def, &mut state, key);
    let out = info(def_id, Some(&def), &state);
    world.entity_mut(mover).insert(state);
    Ok(out)
}

/// The movers, in `AiId` order.
pub(crate) fn movers_in_order(world: &mut World) -> Vec<(AiId, Entity)> {
    let Some(mut q) = world.try_query_filtered::<(Entity, &AiId), With<Mover>>() else { return Vec::new() };
    let mut out: Vec<(AiId, Entity)> = q.iter(world).map(|(e, id)| (*id, e)).collect();
    out.sort_unstable();
    out
}

pub(crate) fn plugin(app: &mut App) {
    app.add_content::<MoverDef>()
        .register_type::<Mover>()
        .register_type::<MoverPart>()
        .register_type::<MoverState>()
        .configure_sets(
            FixedUpdate,
            (
                MoverSystems::Step.in_set(SimSet::Move).before(ControllerSystems::Move),
                MoverSystems::Predict.in_set(NetSet::Predict).after(crate::MotionSystems::PredictIntent).before(ControllerSystems::PredictMove),
            ),
        )
        .add_systems(
            FixedUpdate,
            (
                step::step_movers.in_set(MoverSystems::Step).run_if(any_with_component::<Mover>),
                step::predict_movers.in_set(MoverSystems::Predict).run_if(any_with_component::<Mover>),
            ),
        )
        .add_engine_invariant(MOVER_SANE, mover_sane);
    ai::plugin(app);
}
