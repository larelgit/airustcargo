//! The control lock (F12): "the player may look, but not move or act".
//!
//! A scene that keeps the player's camera takes movement, or actions, or the freedom of the look away
//! for a while, and gives them back. [`ControlLocks`] is the state on the body (a lock per *reason*, so
//! two systems can each hold one and each lets go of its own), [`lock_controls`] and
//! [`unlock_controls`] are the calls a game makes, and two systems apply it inside the tick:
//!
//! ```text
//! SimSet::Act   MotionSystems::Lock    before every reader of Intent (the lock-on switch, MotionSystems::Intent,
//!                                      the weapons, the action machine): mask what is locked, clamp the look
//! SimSet::Resolve, Cleanup             the rest of the tick sees the masked Intent (sprint drain, ...)
//! FixedLast     MotionSystems::Unlock  before the replay check: give back what the mask took
//! ```
//!
//! ## What a lock does to `Intent`
//!
//! - **`movement`**: `move_dir` is zero for the tick, the `jump` press and the `sprint`, `glide` and `crouch` holds are
//!   dropped, and the `dodge` press (a roll is a move and an action: either lock stops it). Gravity, a push and an action already under way go on (a lock does
//!   not freeze a body, it stops it from asking).
//! - **`actions`**: every other press and every other held input is dropped (and `dodge`) (attacks, `fire`,
//!   `reload`, weapon switches, `interact`, `lock_on`, `block`, ...). An action already running ends
//!   as it would.
//! - **`look`**: [`LookLock::Free`] leaves `Intent.look` alone; [`LookLock::Cone`] holds it within `deg`
//!   degrees (in yaw and in pitch, as a disc in angles) of the direction from the body's eye to `at`.
//!   Unlike the two above this one is *kept*: the clamped look is written back into `Intent.look`, so
//!   the first-person camera, which turns on from whatever the intent holds, never drifts out of the
//!   cone, and the `AIM` line and a shot read the same look. A body that has no look (`None`) has
//!   none clamped.
//!
//! The mask is given back at the end of the tick, not kept: the keyboard writes only *differences*
//! into `Intent` (a held `W` is written once), so a `move_dir` the lock zeroed for good would not
//! come back when the lock clears until the key was pressed again. Taking and returning it within one
//! tick leaves `Intent` between ticks as the world wrote it (and so the recorded inputs of a replay
//! too), and the stash that carries it is empty between ticks. A press (`Intent.pressed`, one tick's edge)
//! that a lock masks is dropped, not given back: a button pressed during a lock does not fire when it lifts.
//!
//! A body that was never locked has no [`ControlLocks`] (the component goes when its last reason
//! does), so a run that never locks anything hashes and records as before.

use ai_core::replay::ReplaySystems;
use ai_core::{label, with_query, AiAppExt, AiId, AiIndex, Offenders, SimSet};
use bevy::prelude::*;
use shards_core::input_names::{CROUCH, DODGE, GLIDE, JUMP, SPRINT};
use shards_core::{EngineAppExt, Gaze, Intent, Target};

mod tools;

use crate::eye::{aim_point_of, eye_of};
use crate::intent::{GlideUntilLanded, MotionSystems};

/// Where the look may go while a lock holds it.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Default)]
pub enum LookLock {
    /// Anywhere (the default).
    #[default]
    Free,
    /// Within `deg` degrees of the direction from the body's eye to `at`, in yaw and in pitch.
    Cone {
        /// A point in the world, or a body (its aim point: where a look at it would meet it).
        at: Target,
        /// The half-angle, degrees: above 0 and at most 180.
        deg: f32,
    },
}

/// What one reason takes away.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Default)]
pub struct ControlLock {
    /// No walking, jumping, sprinting or gliding.
    pub movement: bool,
    /// No other input: attacks, shots, reloads, weapon switches, use, lock-on, guard.
    pub actions: bool,
    /// Where the look may go.
    pub look: LookLock,
}

impl ControlLock {
    /// Movement and actions taken, the look free: the player "may look but not move or act".
    pub const LOOKING_ONLY: ControlLock = ControlLock { movement: true, actions: true, look: LookLock::Free };

    /// Nothing taken.
    pub fn is_empty(&self) -> bool {
        !self.movement && !self.actions && self.look == LookLock::Free
    }

    /// `movement, actions, look within 30° of e9`.
    pub fn describe(&self, world: &World) -> String {
        let mut parts = Vec::new();
        if self.movement {
            parts.push("movement".to_owned());
        }
        if self.actions {
            parts.push("actions".to_owned());
        }
        if let LookLock::Cone { at, deg } = self.look {
            parts.push(format!("look within {deg:.0}° of {}", target_text(world, at)));
        }
        if parts.is_empty() { "nothing".to_owned() } else { parts.join(", ") }
    }
}

/// `e9` or `(2.0, 1.5, -4.0)`.
fn target_text(world: &World, at: Target) -> String {
    match at {
        Target::Entity(id) => match world.get_resource::<AiIndex>().and_then(|i| i.get(id)) {
            Some(entity) => label(world, entity),
            None => id.to_string(),
        },
        Target::Point(p) => format!("({:.1}, {:.1}, {:.1})", p.x, p.y, p.z),
    }
}

/// The locks on one body, one per reason, sorted by reason. Simulation state (reflected: snapshots and
/// replay checks hold it); absent while nothing is locked.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct ControlLocks {
    /// `(reason, lock)`, ascending by reason.
    pub held: Vec<(String, ControlLock)>,
}

impl ControlLocks {
    /// The lock held for `reason`.
    pub fn get(&self, reason: &str) -> Option<&ControlLock> {
        self.held.binary_search_by(|(r, _)| r.as_str().cmp(reason)).ok().map(|at| &self.held[at].1)
    }

    /// Whether any reason takes movement away.
    pub fn movement(&self) -> bool {
        self.held.iter().any(|(_, l)| l.movement)
    }

    /// Whether any reason takes the other inputs away.
    pub fn actions(&self) -> bool {
        self.held.iter().any(|(_, l)| l.actions)
    }

    /// The cones in force, in reason order.
    pub fn cones(&self) -> impl Iterator<Item = (Target, f32)> + '_ {
        self.held.iter().filter_map(|(_, l)| match l.look {
            LookLock::Cone { at, deg } => Some((at, deg)),
            LookLock::Free => None,
        })
    }

    /// The reasons, in order.
    pub fn reasons(&self) -> impl Iterator<Item = &str> {
        self.held.iter().map(|(r, _)| r.as_str())
    }

    fn set(&mut self, reason: &str, lock: ControlLock) -> bool {
        match self.held.binary_search_by(|(r, _)| r.as_str().cmp(reason)) {
            Ok(at) => {
                let changed = self.held[at].1 != lock;
                self.held[at].1 = lock;
                changed
            }
            Err(at) => {
                self.held.insert(at, (reason.to_owned(), lock));
                true
            }
        }
    }

    fn remove(&mut self, reason: &str) -> bool {
        match self.held.binary_search_by(|(r, _)| r.as_str().cmp(reason)) {
            Ok(at) => {
                self.held.remove(at);
                true
            }
            Err(_) => false,
        }
    }
}

/// A body's controls were locked, or a reason's lock changed (journaled: `e1 controls locked (scene):
/// movement, actions`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct ControlsLocked {
    pub entity: Entity,
    pub reason: String,
    pub lock: ControlLock,
}

/// A reason let go of its lock (journaled: `e1 controls unlocked (scene)`). `still` is the reasons
/// that still hold the body, in order.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct ControlsUnlocked {
    pub entity: Entity,
    pub reason: String,
    pub still: Vec<String>,
}

/// Locks `entity`'s controls for `reason` ([`ControlLock`]). Locking again under the same reason
/// replaces that reason's lock. `Err` says what is wrong: no `Intent` on the body, an empty reason, a
/// cone whose half-angle is not above 0 and at most 180 degrees.
///
/// Call it where the simulation runs (a tool, a system in a tick): the lock is state. It takes
/// effect on the next tick.
pub fn lock_controls(world: &mut World, entity: Entity, reason: &str, lock: ControlLock) -> Result<(), String> {
    if reason.trim().is_empty() {
        return Err("a lock needs a reason (a name for whoever holds it, so that it can let go of its own)".into());
    }
    if world.get::<Intent>(entity).is_none() {
        return Err(format!("{} has no controls to lock (no `Intent`)", label(world, entity)));
    }
    if let LookLock::Cone { at, deg } = lock.look {
        if !(deg.is_finite() && deg > 0.0 && deg <= 180.0) {
            return Err(format!("a look cone's half-angle is above 0 and at most 180 degrees, not {deg}"));
        }
        if let Target::Point(p) = at
            && !p.is_finite()
        {
            return Err(format!("a look cone points at a point in the world, not {p}"));
        }
    }
    let changed = match world.get_mut::<ControlLocks>(entity) {
        Some(mut locks) => locks.set(reason, lock),
        None => {
            let mut locks = ControlLocks::default();
            locks.set(reason, lock);
            world.entity_mut(entity).insert(locks);
            true
        }
    };
    if changed {
        world.trigger(ControlsLocked { entity, reason: reason.to_owned(), lock });
    }
    Ok(())
}

/// Lets go of the lock `reason` holds on `entity`; the other reasons' locks stay. Returns whether
/// there was one. The component goes with the last reason.
pub fn unlock_controls(world: &mut World, entity: Entity, reason: &str) -> bool {
    let Some(mut locks) = world.get_mut::<ControlLocks>(entity) else { return false };
    if !locks.remove(reason) {
        return false;
    }
    let still: Vec<String> = locks.reasons().map(str::to_owned).collect();
    if still.is_empty() {
        world.entity_mut(entity).remove::<ControlLocks>();
    }
    world.trigger(ControlsUnlocked { entity, reason: reason.to_owned(), still });
    true
}

/// What is taken from an `Intent` for the tick and given back by [`restore`].
struct Taken {
    entity: Entity,
    move_dir: Option<Vec3>,
    held: Vec<String>,
    /// The body held a glide the `glide` tool latched (`GlideUntilLanded`) when it was taken: if the latch is gone by the
    /// end of the tick (it landed), that glide is over and is not given back.
    glide_latched: bool,
}

/// The masks of the tick under way: filled by [`mask`], emptied by [`restore`], so empty between
/// ticks (snapshots and replays never see it).
#[derive(Resource, Default)]
pub struct LockStash(Vec<Taken>);

impl LockStash {
    /// Nothing is taken: the state between ticks.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<ControlLocks>()
        .register_type::<ControlLock>()
        .register_type::<LookLock>()
        .init_resource::<LockStash>()
        .configure_sets(FixedUpdate, MotionSystems::Lock.in_set(SimSet::Act).before(MotionSystems::Intent))
        .configure_sets(FixedLast, MotionSystems::Unlock.before(ReplaySystems::Check))
        .add_systems(FixedUpdate, mask.in_set(MotionSystems::Lock).run_if(any_locked))
        .add_systems(FixedLast, restore.in_set(MotionSystems::Unlock).run_if(any_taken))
        .journal_event::<ControlsLocked>(|e, w| {
            let lock = &e.lock;
            Some(format!("{} controls locked ({}): {}", label(w, e.entity), e.reason, lock.describe(w)))
        })
        .journal_event::<ControlsUnlocked>(|e, w| {
            let still = if e.still.is_empty() { String::new() } else { format!(", still held: {}", e.still.join(", ")) };
            Some(format!("{} controls unlocked ({}){still}", label(w, e.entity), e.reason))
        })
        .register_ai_observe_lines(observe_lines)
        .add_engine_invariant(CONTROL_LOCK_SANE, control_lock_sane);
    tools::plugin(app);
}

fn any_locked(locks: Query<(), With<ControlLocks>>) -> bool {
    !locks.is_empty()
}

fn any_taken(stash: Res<LockStash>) -> bool {
    !stash.0.is_empty()
}

/// `SimSet::Act`, before every reader of `Intent`: takes what the locks take from each locked body's
/// `Intent` (see the module), and clamps its look into its cones.
pub(crate) fn mask(world: &mut World) {
    let mut bodies: Vec<(AiId, Entity)> = {
        let mut q = world.query_filtered::<(Entity, Option<&AiId>), (With<ControlLocks>, With<Intent>)>();
        q.iter(world).map(|(e, id)| (id.copied().unwrap_or_default(), e)).collect()
    };
    // `AiId` order, entity for the bodies that have none (they are not part of the game's ids).
    bodies.sort_by_key(|(id, e)| (*id, *e));
    let mut stash = std::mem::take(&mut world.resource_mut::<LockStash>().0);
    stash.clear();
    for (_, entity) in bodies {
        let Some(locks) = world.get::<ControlLocks>(entity).cloned() else { continue };
        // The look first: it reads the world (the eye, the body looked at), the mask does not.
        let cones: Vec<(Target, f32)> = locks.cones().collect();
        if !cones.is_empty() {
            clamp_look(world, entity, &cones);
        }
        let (movement, actions) = (locks.movement(), locks.actions());
        if !movement && !actions {
            continue;
        }
        let glide_latched = world.get::<GlideUntilLanded>(entity).is_some();
        let Some(mut intent) = world.get_mut::<Intent>(entity) else { continue };
        let mut taken = Taken { entity, move_dir: None, held: Vec::new(), glide_latched };
        if movement && intent.move_dir != Vec3::ZERO {
            taken.move_dir = Some(intent.move_dir);
            intent.move_dir = Vec3::ZERO;
        }
        let drops = |input: &str| match input {
            // A roll is a move and an action: either lock stops it.
            DODGE => movement || actions,
            JUMP | SPRINT | GLIDE | CROUCH => movement,
            _ => actions,
        };
        if intent.held.iter().any(|h| drops(h)) {
            taken.held = intent.held.iter().filter(|h| drops(h)).cloned().collect();
            intent.held.retain(|h| !drops(h));
        }
        if intent.pressed.iter().any(|p| drops(&p.input)) {
            intent.pressed.retain(|p| !drops(&p.input));
        }
        if taken.move_dir.is_some() || !taken.held.is_empty() {
            stash.push(taken);
        }
    }
    world.resource_mut::<LockStash>().0 = stash;
}

/// `FixedLast`, before the replay check: gives back what [`mask`] took, so `Intent` is between ticks
/// what the world wrote (the keyboard writes differences only).
pub(crate) fn restore(world: &mut World) {
    let stash = std::mem::take(&mut world.resource_mut::<LockStash>().0);
    for taken in stash {
        // A latched glide that ended in this tick (the body landed: `intent_to_locomotion` let go of it and took the latch
        // away, on the masked copy) must not come back as a hold nobody asked for.
        let glide_over = taken.glide_latched && world.get::<GlideUntilLanded>(taken.entity).is_none();
        let Some(mut intent) = world.get_mut::<Intent>(taken.entity) else { continue };
        if let Some(dir) = taken.move_dir
            && intent.move_dir == Vec3::ZERO
        {
            intent.move_dir = dir;
        }
        for input in &taken.held {
            if glide_over && input == GLIDE {
                continue;
            }
            if !intent.is_held(input) {
                intent.set_held(input, true);
            }
        }
    }
}

/// Holds `entity`'s look inside every cone (in reason order), writing the clamped look back.
fn clamp_look(world: &mut World, entity: Entity, cones: &[(Target, f32)]) {
    let Some(mut look) = world.get::<Intent>(entity).and_then(|i| i.look).filter(|l| l.is_finite()) else { return };
    let Some(eye) = eye_of(world, entity) else { return };
    let before = look;
    for &(at, deg) in cones {
        let point = match at {
            Target::Point(p) => Some(p),
            Target::Entity(id) => world.get_resource::<AiIndex>().and_then(|i| i.get(id)).and_then(|e| aim_point_of(world, e)),
        };
        // A body that is gone leaves the cone with nothing to point at: the look is free of it.
        let Some(center) = point.and_then(|p| Gaze::toward(eye, p)) else { continue };
        look = within_cone(look, center, deg);
    }
    if look != before
        && let Some(mut intent) = world.get_mut::<Intent>(entity)
    {
        intent.look = Some(look);
    }
}

/// `look` held within `deg` degrees of `center`, measured in yaw and pitch as one disc of angles. A look
/// inside is returned as it is.
pub fn within_cone(look: Gaze, center: Gaze, deg: f32) -> Gaze {
    let max = deg.to_radians();
    let dyaw = crate::camera::math::wrap_angle(look.yaw - center.yaw);
    let dpitch = look.pitch - center.pitch;
    let len = (dyaw * dyaw + dpitch * dpitch).sqrt();
    if len <= max || !len.is_finite() {
        return look;
    }
    let k = max / len;
    Gaze { yaw: center.yaw + dyaw * k, pitch: center.pitch + dpitch * k }
}

/// The name of the engine rule [`control_lock_sane`].
pub const CONTROL_LOCK_SANE: &str = "motion.control_lock_sane";

/// The engine rule `motion.control_lock_sane`: every [`ControlLocks`] is one [`lock_controls`] could have made. It
/// holds at least one reason, the reasons are named and ascend without repeats, every lock takes something away,
/// and a look cone's half-angle is finite, above 0 and at most 180 degrees and points at a finite place. The call keeps all of it (and
/// takes the component away with the last reason); a violation names a writer that put a lock together by hand.
pub fn control_lock_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &ControlLocks), (), _>(world, |query| {
        for (id, locks) in query.iter(world) {
            if let Some(what) = problem(locks) {
                bad.note(*id, || what);
            }
        }
    })?;
    bad.report()
}

fn problem(locks: &ControlLocks) -> Option<String> {
    if locks.held.is_empty() {
        return Some("ControlLocks holds no reason (the component goes with the last)".to_owned());
    }
    for pair in locks.held.windows(2) {
        if pair[0].0 >= pair[1].0 {
            return Some(format!("ControlLocks reasons do not ascend: `{}` then `{}`", pair[0].0, pair[1].0));
        }
    }
    for (reason, lock) in &locks.held {
        if reason.trim().is_empty() {
            return Some("ControlLocks has a lock with no reason".to_owned());
        }
        if lock.is_empty() {
            return Some(format!("the lock `{reason}` takes nothing away"));
        }
        if let LookLock::Cone { at, deg } = lock.look {
            if !(deg.is_finite() && deg > 0.0 && deg <= 180.0) {
                return Some(format!("the lock `{reason}` has a look cone with a half-angle of {deg} degrees (above 0, at most 180)"));
            }
            if let Target::Point(p) = at
                && !p.is_finite()
            {
                return Some(format!("the lock `{reason}` has a look cone pointing at {p}"));
            }
        }
    }
    None
}

/// `CONTROL looking only (scene)`, `CONTROL locked: movement (door)`: under `YOU`, while the subject has a lock.
fn observe_lines(world: &World, subject: Entity) -> Vec<String> {
    let Some(locks) = world.get::<ControlLocks>(subject) else { return Vec::new() };
    vec![control_line(world, locks)]
}

/// The `CONTROL` line for a set of locks.
pub fn control_line(world: &World, locks: &ControlLocks) -> String {
    let (movement, actions) = (locks.movement(), locks.actions());
    let cones: Vec<String> = locks.cones().map(|(at, deg)| format!("look within {deg:.0}° of {}", target_text(world, at))).collect();
    let reasons = locks.reasons().collect::<Vec<_>>().join(", ");
    let what = if movement && actions {
        let mut text = "looking only".to_owned();
        if !cones.is_empty() {
            text.push_str(&format!(", {}", cones.join(", ")));
        }
        text
    } else {
        let mut parts = Vec::new();
        if movement {
            parts.push("movement".to_owned());
        }
        if actions {
            parts.push("actions".to_owned());
        }
        parts.extend(cones);
        format!("locked: {}", parts.join(", "))
    };
    format!("CONTROL {what} ({reasons})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_inside_the_cone_is_untouched_and_one_outside_lands_on_its_edge() {
        let center = Gaze::from_degrees(90.0, 0.0);
        let inside = Gaze::from_degrees(100.0, 5.0);
        assert_eq!(within_cone(inside, center, 20.0), inside);
        let out = within_cone(Gaze::from_degrees(180.0, 0.0), center, 30.0);
        assert!((out.yaw.to_degrees() - 120.0).abs() < 1e-3 && out.pitch.abs() < 1e-5, "{out:?}");
        // The way round: 350 is 10 degrees past 0, not 350 degrees from it.
        let near = Gaze::from_degrees(350.0, 0.0);
        assert_eq!(within_cone(near, Gaze::from_degrees(0.0, 0.0), 15.0), near);
        let diagonal = within_cone(Gaze::from_degrees(90.0 + 40.0, 30.0), center, 25.0);
        let (dy, dp) = ((diagonal.yaw - center.yaw).to_degrees(), diagonal.pitch.to_degrees());
        assert!(((dy * dy + dp * dp).sqrt() - 25.0).abs() < 1e-3, "on the edge: {dy} {dp}");
        assert!(dy > 0.0 && dp > 0.0, "towards where it was looking");
    }

    #[test]
    fn locks_are_kept_per_reason_in_order() {
        let mut locks = ControlLocks::default();
        assert!(locks.set("scene", ControlLock { movement: true, ..default() }));
        assert!(locks.set("door", ControlLock { actions: true, ..default() }));
        assert!(!locks.set("door", ControlLock { actions: true, ..default() }), "the same lock again changes nothing");
        assert_eq!(locks.reasons().collect::<Vec<_>>(), ["door", "scene"]);
        assert!(locks.movement() && locks.actions());
        assert!(locks.remove("scene"));
        assert!(!locks.movement() && locks.actions());
        assert!(!locks.remove("scene"));
    }
}
