//! Lock-on as a simulation rule (C6): with a `lock_on` definition (content kind of
//! `shards_motion::camera::lock`), every tick checks every body's lock, and the `lock_switch`
//! press moves a lock to the next target on one side. Both run in `FixedUpdate`, so a headless
//! run, an agent and a window agree; the camera (presentation) only picks and sends presses.
//!
//! - **Switch** ([`LockSystems::Switch`], `SimSet::Act` before `MotionSystems::Intent`): every
//!   `lock_switch` press (`dir.x` the side) is used up and carried out by [`switch_target`]: of
//!   the bodies the locker may lock on to, within range and seen from its eye, the nearest angle
//!   past the dead band on that side, measured from the locker. It works without a definition
//!   too (the definition's defaults, `CombatTuning::lock_on_range` as the range): a press nobody
//!   sends changes nothing.
//! - **Check** ([`LockSystems::Check`], `SimSet::Cleanup` after `CoreSystems::ClearPresses`, so
//!   after the deaths of the tick and after the engine's despawns of that tick (a co-op body whose
//!   rejoin grace is over goes before `ClearPresses`); a game system that despawns a body that may
//!   be locked on to runs before it): a lock is lost when its target is gone, dead ([`Dead`]),
//!   farther than `range` (3D, between the positions) or hidden from the locker's eye behind the
//!   world for `obstruction_s` without a break. The count is [`LockSight`], on the locker only
//!   while its target is hidden (part of the replay and the snapshot); the journal says why a
//!   lock went (`e5 lock lost: e6 obstructed 2.0s`, `dead`, `gone`, `out of range`).
//!
//! The journal also says when a lock is taken (`e5 locks e6`) and let go of (`e5 lock released:
//! e6`, the player's cancel): the `lock_on` tool and the camera write it where they set the
//! target ([`LockTaken`], [`LockReleased`]; the camera as a local line), a switch where it moves
//! it ([`LockSwitched`]).
//!
//! Without a definition the check does nothing at all: no component, no journal line, no lock
//! taken away (a game keeps what it had). The hooks that tell motion's lock-on who may be
//! locked on to (alive, and a `LockOnTarget` or a hostile fighter) and who is dangerous (an
//! attack whose hit zone would land on the locker) are this module's ([`LockHooks`]).

use ai_core::prelude::*;
use ai_core::{with_query, AiIndex, Offenders};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::input_names::LOCK_SWITCH;
use shards_core::{CoreSystems, EngineAppExt, Intent};
use shards_motion::camera::lock::{body_point, eye_of, lock_candidates, pick_lock, sight_clear, switch_target, PickView};
use shards_motion::camera::{LockHooks, LockOnDef, Side};
use shards_motion::{Collider, LockOnTarget, MotionSystems, PhysicsWorld};

use crate::def::CombatTuning;
use crate::fighter::{hostile, Dead, Fighter, Team};
use crate::zone::{body_radius, zones_threatening};

/// The lock-on systems, for ordering.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockSystems {
    /// `SimSet::Act`, before `MotionSystems::Intent`: `lock_switch` presses move locks.
    Switch,
    /// `SimSet::Cleanup`, after `CoreSystems::ClearPresses` (and so after the engine's despawns
    /// of the tick): every lock is checked. A game system that despawns a body that may be
    /// locked on to goes before it, or the rule `action.lock_on_valid` stops the game.
    Check,
}

/// How long a body's lock-on target has been hidden behind the world without a break, with a
/// `lock_on` definition: on the locker from the first tick its target is hidden, gone when it is
/// seen again or the lock goes. `secs` is the same time in seconds, for the AI.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct LockSight {
    /// The target the count is for.
    pub target: AiId,
    /// Ticks in a row the ray from the locker's eye to the target's aim point was blocked.
    pub ticks: u32,
    /// The same, in seconds.
    pub secs: f32,
}

/// Why a lock was lost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LockLoss {
    /// The target no longer exists.
    Gone,
    /// The target died.
    Dead,
    /// The target is farther than the range: (how far, the range), m.
    OutOfRange(f32, f32),
    /// The target was hidden this long (s).
    Obstructed(f32),
}

impl std::fmt::Display for LockLoss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockLoss::Gone => write!(f, "gone"),
            LockLoss::Dead => write!(f, "dead"),
            LockLoss::OutOfRange(d, range) => write!(f, "out of range ({d:.1}m > {range:.1}m)"),
            LockLoss::Obstructed(s) => write!(f, "obstructed {s:.1}s"),
        }
    }
}

/// A body took a lock (with a `lock_on` definition, from the `lock_on` tool): `e5 locks e6`.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct LockTaken {
    pub entity: Entity,
    pub target: AiId,
}

/// A body lost its lock: `e5 lock lost: e6 dead`.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct LockLost {
    pub entity: Entity,
    pub target: AiId,
    pub reason: LockLoss,
}

/// A body let go of its lock (with a `lock_on` definition, the `lock_on` tool's `off`): `e5 lock
/// released: e6`.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct LockReleased {
    pub entity: Entity,
    pub target: AiId,
}

/// A switch moved a lock: `e5 locks e7 (switched right from e6)`.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct LockSwitched {
    pub entity: Entity,
    pub from: AiId,
    pub to: AiId,
    pub side: Side,
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<LockSight>()
        .insert_resource(LockHooks { lockable, danger })
        .configure_sets(
            FixedUpdate,
            (
                LockSystems::Switch.in_set(SimSet::Act).before(MotionSystems::Intent),
                // After the presses are cleared: whatever goes before `ClearPresses` (the co-op
                // despawn of a body whose rejoin grace is over) is gone when the check looks.
                LockSystems::Check.in_set(SimSet::Cleanup).after(CoreSystems::ClearPresses),
            ),
        )
        .add_systems(FixedUpdate, (switch_locks.in_set(LockSystems::Switch), check_locks.in_set(LockSystems::Check)))
        .journal_event::<LockTaken>(|e, w| Some(format!("{} locks {}", label(w, e.entity), e.target)))
        .journal_event::<LockReleased>(|e, w| Some(format!("{} lock released: {}", label(w, e.entity), e.target)))
        .journal_event::<LockLost>(|e, w| Some(format!("{} lock lost: {} {}", label(w, e.entity), e.target, e.reason)))
        .journal_event::<LockSwitched>(|e, w| {
            Some(format!("{} locks {} (switched {} from {})", label(w, e.entity), e.to, e.side.name(), e.from))
        })
        .register_ai_describer("lock", describe_lock)
        .add_engine_invariant("action.lock_on_valid", lock_on_valid);
    app.world_mut().register_component::<LockSight>();
}

/// Whether a body of team `viewer` may lock on to one with these parts: alive, and marked
/// [`LockOnTarget`] or a fighter of another team.
pub fn may_lock(viewer: Option<&Team>, marked: bool, fighter: bool, dead: bool, team: Option<&Team>) -> bool {
    !dead && (marked || (fighter && hostile(viewer, team)))
}

/// [`LockHooks::lockable`]: [`may_lock`] read from the world.
pub fn lockable(world: &World, viewer: Entity, candidate: Entity) -> bool {
    viewer != candidate
        && may_lock(
            world.get::<Team>(viewer),
            world.get::<LockOnTarget>(candidate).is_some(),
            world.get::<Fighter>(candidate).is_some(),
            world.get::<Dead>(candidate).is_some(),
            world.get::<Team>(candidate),
        )
}

/// [`LockHooks::danger`]: whoever has a hit zone that would land on `viewer` now (a telegraphed
/// windup or an active swing), at 1.
pub fn danger(world: &World, viewer: Entity) -> Vec<(Entity, f32)> {
    let Some(here) = world.get::<Transform>(viewer).map(|t| t.translation) else { return Vec::new() };
    let mut out: Vec<(Entity, f32)> =
        zones_threatening(world, viewer, here, body_radius(world, viewer)).into_iter().map(|(z, _)| (z.attacker, 1.0)).collect();
    out.dedup_by_key(|(e, _)| *e);
    out
}

/// The rules a switch goes by: the definition, or without one its defaults with the combat
/// tuning's `lock_on_range` (the `lock_on` tool's reach without a definition).
pub fn switch_rules(world: &World) -> LockOnDef {
    let db = world.get_resource::<ContentDb>();
    match LockOnDef::active(db) {
        Some(def) => def.clone(),
        None => LockOnDef { range: db.map_or(CombatTuning::default().lock_on_range, |db| CombatTuning::of(db).lock_on_range), ..default() },
    }
}

/// Moves `entity`'s lock to the next target on `side` by `rules`: `Ok((from, to))`, or
/// `Err(Some(from))` when nobody is there and `Err(None)` when it holds no lock. [`LockSwitched`]
/// is triggered on a machine that simulates (the count of hidden ticks is for the old target, so
/// the check starts it again).
pub fn switch_lock_of(world: &mut World, entity: Entity, side: Side, rules: &LockOnDef) -> Result<(AiId, AiId), Option<AiId>> {
    let from = world.get::<Intent>(entity).and_then(|i| i.target).ok_or(None)?;
    let to = switch_target(world, entity, side, rules).ok_or(Some(from))?;
    if let Some(mut intent) = world.get_mut::<Intent>(entity) {
        intent.target = Some(to);
    }
    if simulating(world) {
        world.trigger(LockSwitched { entity, from, to, side });
    }
    Ok((from, to))
}

/// The scored pick of `def` for a body with its own eyes (the `lock_on` tool): what it may lock
/// on to, within range and seen from its eye, distance and look measured from its feet along
/// its facing.
pub fn pick_for_body(world: &World, me: Entity, def: &LockOnDef) -> Option<AiId> {
    let tf = world.get::<Transform>(me)?;
    let eye = eye_of(world, me, def)?;
    let physics = world.get_resource::<PhysicsWorld>();
    let candidates: Vec<_> = lock_candidates(world, me, def)
        .into_iter()
        .filter(|(e, c)| physics.is_none_or(|p| sight_clear(p, eye, c.aim, &[me, *e])))
        .map(|(_, c)| c)
        .collect();
    let view = PickView::body(tf.translation, eye, *tf.forward());
    pick_lock(&view, def, &candidates).map(|i| AiId(candidates[i].key))
}

/// `SimSet::Act`, before `MotionSystems::Intent` (so the body faces its new target on the same
/// tick): every body's `lock_switch` presses, in `AiId` order, each in the order pressed. A
/// press with no side, with no lock to move, or with nobody on that side is used up and changes
/// nothing. The query is kept between ticks, so a tick without a press only looks at the presses.
pub fn switch_locks(world: &mut World, bodies: &mut QueryState<(Entity, &'static AiId, &'static Intent)>) {
    let mut bodies: Vec<(AiId, Entity)> =
        bodies.iter(world).filter(|(_, _, i)| i.pressed.iter().any(|p| p.input == LOCK_SWITCH)).map(|(e, id, _)| (*id, e)).collect();
    if bodies.is_empty() {
        return;
    }
    bodies.sort();
    let rules = switch_rules(world);
    for (_, entity) in bodies {
        let mut presses = Vec::new();
        if let Some(mut intent) = world.get_mut::<Intent>(entity) {
            while let Some(press) = intent.take_press(LOCK_SWITCH) {
                presses.push(press);
            }
        }
        for side in presses.into_iter().filter_map(|p| p.dir.and_then(|d| Side::from_x(d.x))) {
            if let Err(None) = switch_lock_of(world, entity, side, &rules) {
                break;
            }
        }
    }
}

/// A body that may hold a lock, as the check reads it.
type Lockers<'w, 's> =
    Query<'w, 's, (Entity, &'static AiId, &'static mut Intent, &'static Transform, Option<&'static Collider>, Option<&'static mut LockSight>)>;

/// What the check reads of a target.
type Targets<'w, 's> = Query<'w, 's, (&'static Transform, Option<&'static Collider>, Has<Dead>)>;

/// `SimSet::Cleanup`, after `CoreSystems::ClearPresses`, with a `lock_on` definition: every
/// body's lock (the player's, an agent's: whatever `Intent.target` names), in `AiId` order.
/// Lost when the target is gone, dead, out of range or hidden for `obstruction_s` in a row
/// ([`LockSight`] counts while it is hidden). Without a definition it does nothing (a count a
/// removed definition left is taken away).
pub fn check_locks(
    mut commands: Commands,
    db: Option<Res<ContentDb>>,
    time: Res<Time>,
    physics: Option<Res<PhysicsWorld>>,
    index: Res<AiIndex>,
    mut lockers: Lockers,
    targets: Targets,
) {
    let Some(def) = LockOnDef::active(db.as_deref()) else {
        for (entity, .., sight) in &mut lockers {
            if sight.is_some() {
                commands.entity(entity).remove::<LockSight>();
            }
        }
        return;
    };
    let dt = time.delta_secs();
    let limit = def.obstruction_ticks(dt);
    let mut order: Vec<(AiId, Entity)> =
        lockers.iter().filter(|(_, _, i, _, _, s)| i.target.is_some() || s.is_some()).map(|(e, id, ..)| (*id, e)).collect();
    order.sort();
    for (_, entity) in order {
        let Ok((_, _, mut intent, tf, collider, sight)) = lockers.get_mut(entity) else { continue };
        let Some(target) = intent.target else {
            commands.entity(entity).remove::<LockSight>();
            continue;
        };
        let mut hidden_ticks = 0;
        let loss = match index.get(target).and_then(|e| targets.get(e).ok().map(|t| (e, t))) {
            None => Some(LockLoss::Gone),
            Some((_, (_, _, true))) => Some(LockLoss::Dead),
            Some((_, (there, ..))) if there.translation.distance(tf.translation) > def.range => {
                Some(LockLoss::OutOfRange(there.translation.distance(tf.translation), def.range))
            }
            Some((target_entity, (there, target_collider, _))) => {
                let eye = body_point(tf, collider, def.eye_height);
                let aim = body_point(there, target_collider, def.aim_height);
                if physics.as_deref().is_some_and(|p| !sight_clear(p, eye, aim, &[entity, target_entity])) {
                    hidden_ticks = sight.as_deref().filter(|s| s.target == target).map_or(0, |s| s.ticks) + 1;
                }
                limit.filter(|l| hidden_ticks >= *l).map(|_| LockLoss::Obstructed(hidden_ticks as f32 * dt))
            }
        };
        if let Some(reason) = loss {
            intent.target = None;
            if sight.is_some() {
                commands.entity(entity).remove::<LockSight>();
            }
            commands.trigger(LockLost { entity, target, reason });
            continue;
        }
        match (sight, hidden_ticks) {
            (Some(_), 0) => {
                commands.entity(entity).remove::<LockSight>();
            }
            (None, 0) => {}
            (Some(mut s), ticks) => *s = LockSight { target, ticks, secs: ticks as f32 * dt },
            (None, ticks) => {
                commands.entity(entity).insert(LockSight { target, ticks, secs: ticks as f32 * dt });
            }
        }
    }
}

/// `lock e6`, and how long it has been hidden: `lock e6 (obstructed 0.8s)`.
pub fn describe_lock(entity: EntityRef) -> Option<String> {
    let target = entity.get::<Intent>()?.target?;
    Some(match entity.get::<LockSight>().filter(|s| s.target == target && s.ticks > 0) {
        Some(sight) => format!("lock {target} (obstructed {:.1}s)", sight.secs),
        None => format!("lock {target}"),
    })
}

/// The engine rule `action.lock_on_valid`: with a `lock_on` definition, no body's lock-on target
/// (`Intent.target`) names an entity that no longer exists or is dead.
///
/// [`check_locks`] takes such a lock away at the end of every tick, after the deaths and the
/// engine's despawns of that tick, so after a tick none is left; a violation names a lock set
/// some other way inside the tick after the check, or a target taken away after it (a game
/// system in `SimSet::Cleanup` after `LockSystems::Check` that sets a lock or despawns a body
/// that may be locked on to: it belongs before the check), or a check that missed it. Without a
/// definition locks are not checked and the rule passes (a game keeps the old behaviour, where a
/// lock on a dead target stays until somebody lets go).
pub fn lock_on_valid(world: &World) -> Option<String> {
    LockOnDef::active(world.get_resource::<ContentDb>())?;
    let index = world.get_resource::<AiIndex>()?;
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Intent), (), _>(world, |query| {
        for (id, intent) in query.iter(world) {
            let Some(target) = intent.target else { continue };
            match index.get(target) {
                None => bad.note(*id, || format!("Intent.target = {target}, which no longer exists")),
                Some(e) if world.get::<Dead>(e).is_some() => bad.note(*id, || format!("Intent.target = {target}, which is dead")),
                Some(_) => {}
            }
        }
    });
    bad.report()
}

/// Whether this machine runs the simulation (not a co-op client): gameplay events are its.
fn simulating(world: &World) -> bool {
    world.get_resource::<shards_core::NetRole>().is_none_or(|r| r.simulates())
}

/// `entity` locks on to `target` (the `lock_on` tool): the target is set and, with a `lock_on`
/// definition on a machine that simulates, the journal says so ([`LockTaken`]). A co-op client's
/// lock reaches the host as its input, without a line.
pub fn take_lock(world: &mut World, entity: Entity, target: AiId) {
    let Some(mut intent) = world.get_mut::<Intent>(entity) else { return };
    if intent.target == Some(target) {
        return;
    }
    intent.target = Some(target);
    if LockOnDef::active(world.get_resource::<ContentDb>()).is_some() && simulating(world) {
        world.trigger(LockTaken { entity, target });
    }
}

/// `entity` lets go of its lock (the `lock_on` tool's `off`): with a `lock_on` definition on a
/// machine that simulates, the journal says so ([`LockReleased`]). Returns the target it held.
pub fn release_lock(world: &mut World, entity: Entity) -> Option<AiId> {
    let target = world.get_mut::<Intent>(entity)?.target.take()?;
    if LockOnDef::active(world.get_resource::<ContentDb>()).is_some() && simulating(world) {
        world.trigger(LockReleased { entity, target });
    }
    Some(target)
}

/// Whether `me` may lock on to `target` by the lock-on rules (a `lock_on` definition): `Err`
/// says why not, the way the `lock_on` tool answers. What the check would take away at once (a
/// dead target, one beyond `def.range`) and what the pick would never name (yourself, an ally,
/// something that is neither a fighter of another team nor a `LockOnTarget`, a body with no
/// position) are refused.
pub fn may_lock_on(world: &World, me: Entity, target: Entity, id: AiId, def: &LockOnDef) -> Result<(), String> {
    if world.get::<Dead>(target).is_some() {
        return Err(format!("{id} is dead; lock on to a living target"));
    }
    let (Some(here), Some(there)) = (world.get::<Transform>(me), world.get::<Transform>(target)) else {
        return Err(format!("{id} has no position to lock on to; lock on to a body in the world"));
    };
    let distance = there.translation.distance(here.translation);
    if distance > def.range {
        return Err(format!("{id} is {distance:.1}m away, beyond the lock-on range of {:.0}m; come closer", def.range));
    }
    let hooks = world.get_resource::<LockHooks>().copied().unwrap_or_default();
    if !(hooks.lockable)(world, me, target) {
        return Err(format!(
            "{id} is not something you can lock on to (an ally, or neither a fighter of another team nor a lock-on target); \
             lock on to an enemy, or call `lock_on` without `target` for the best one you can see"
        ));
    }
    Ok(())
}
