//! Context and paired actions: what a fighter remembers for a moment ([`Memory`]), inputs whose
//! action depends on that moment ([`ContextInputs`]: a light attack that is a finisher next to a
//! broken enemy, a counter right after a parry, a cut out of a roll), the fresh press that arms a
//! guard's perfect window ([`GuardArm`]), and paired actions that hold a victim while they run
//! ([`Pairing`] on the attacker, [`Paired`] on the victim).
//!
//! Content: `MovesetDef::remember` and `MovesetDef::context`, `ActionDef::pair` and
//! `ActionDef::weight`, `GuardDef::perfect_needs_press`. Everything here is lazy: a fighter whose
//! content uses none of it never gets one of these components.
//!
//! Order inside a tick:
//!
//! ```text
//! SimSet::Act      ActionSystems::Context   guard presses arm; every binding's conditions → ContextInputs
//!                  ActionSystems::Machine   can_start reads ContextInputs: a bound input starts its action
//!                  ActionSystems::Effects   start_pairs: a paired action that started takes its victim
//! SimSet::Resolve  ActionSystems::Hits      a pair's strike: Finisher weight, end of the break, forced reaction
//!                  ActionSystems::Fighters  age_memory; tend_pairs: a dead, gone or distant victim is lost
//! ```

use ai_core::{AiId, AiIndex, SimTick};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent, NetRole, PlayerId};
use shards_motion::{MotionState, PhysicsWorld};

use crate::combat::{clear_line, Blocked, Damaged, Died, Evaded, GuardBroken, Parried, PerfectEvaded, Reacted, Staggered};
use crate::def::{ActionDef, CombatTuning, Forget, GuardPress, MovesetDef, PairAngle, PairDef, PairSelect, PairState, PairTeam, Recall, When, EPS};
use crate::fighter::{hostile, Dead, Fighter, Health, Poise, Stamina, Team};
use crate::machine::{interrupted, ActionEnd, ActionEnded, ActionStarted, ActionState};
use crate::zone::body_radius;

// --- components ----------------------------------------------------------------------------

/// What a fighter remembers now ([`MovesetDef::remember`]), sorted by key. Lazy: present while it
/// remembers something.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Memory(pub Vec<Remembered>);

/// One memory: `key` for `left` more seconds.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Remembered {
    pub key: String,
    pub left: f32,
}

impl Memory {
    /// Whether `key` is remembered now.
    pub fn has(&self, key: &str) -> bool {
        self.0.iter().any(|m| m.key == key)
    }
}

/// This tick's context ([`MovesetDef::context`]): which inputs start which action instead of the
/// moveset's. Recomputed at the start of every tick for fighters whose moveset has `context`;
/// present while something is bound. A client reads its host's copy to predict its own starts.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct ContextInputs {
    /// In binding order; one per input.
    pub bound: Vec<Bound>,
}

/// One bound input ([`ContextInputs::bound`]).
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Bound {
    pub input: String,
    pub action: String,
    /// Index of the binding in the moveset's `context`.
    pub binding: u32,
    /// The victim a `When::Pair` condition found.
    pub victim: Option<AiId>,
}

impl ContextInputs {
    /// The binding `input` has now, if any.
    pub fn bound(&self, input: &str) -> Option<&Bound> {
        self.bound.iter().find(|b| b.input == input)
    }

    /// The first victim a bound paired action would take.
    pub fn victim(&self) -> Option<AiId> {
        self.bound.iter().find_map(|b| b.victim)
    }
}

/// The presses of a fighter's guard inputs, for a guard whose perfect window needs a fresh press
/// ([`GuardDef::perfect_needs_press`](crate::GuardDef::perfect_needs_press)). Lazy: present from a
/// press until it no longer matters (the gap since it passed, the button up, its guard over).
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct GuardArm {
    /// Tick of the last press (`None`: none that counts; a parry clears it, so the next press is fresh).
    pub pressed: Option<u64>,
    /// That press was fresh (`gap` after the one before) and no guard has started from it yet.
    pub armed: bool,
    /// A guard input was held on the last tick: a press is the button going down.
    pub down: bool,
    /// The guard run (`Running::instance`) a fresh press started: only it opens its perfect window.
    pub run: Option<u32>,
}

/// On the attacker of a running paired action ([`ActionDef::pair`]).
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Pairing {
    pub victim: AiId,
    pub action: String,
    /// The run (`Running::instance`) the pair belongs to.
    pub instance: u32,
    /// The pair's strike already ended the victim's break (`PairDef::ends_break`).
    pub broke_once: bool,
}

/// On the attacker of a running paired action whose victim just died of the pair's own blow (set when `Died` names the
/// attacker and the pair's action, taken by `release_pair`, which reports it as `PairEnded::killed`). A mark that lives from
/// the kill to the next `tend_pairs`; left out of replays, as the pair's end is already in the state it leaves.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct PairKilled;

/// On the victim of a running paired action.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Paired {
    pub by: AiId,
    /// Other attackers find it untouchable (`PairDef::exclusive`).
    pub exclusive: bool,
}

/// The run `instance` moves no more: its displacement is 0 for the rest of it (a paired action
/// whose victim died). Lazy; it goes with the run.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct RunHalted {
    pub instance: u32,
}

// --- resources and events ------------------------------------------------------------------

/// A game condition a binding names (`When::Condition(name)`): whether it holds for the fighter now.
pub type ContextCondition = fn(&World, Entity) -> bool;

/// The game's context conditions, by name ([`add_context_condition`](crate::ActionAppExt::add_context_condition)).
#[derive(Resource, Default, Clone)]
pub struct ContextConditions(pub(crate) Vec<(String, ContextCondition)>);

impl ContextConditions {
    pub fn get(&self, name: &str) -> Option<ContextCondition> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, c)| *c)
    }
}

/// Paired actions the machine started this tick, for [`start_pairs`]; empty between ticks.
#[derive(Resource, Default)]
pub struct PendingPairs(pub(crate) Vec<PendingPair>);

pub(crate) struct PendingPair {
    pub attacker: Entity,
    pub action: String,
    pub instance: u32,
    /// The victim the binding found when the input was resolved.
    pub victim: Option<AiId>,
}

/// A paired action took its victim.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct PairStarted {
    pub attacker: Entity,
    pub victim: Entity,
    pub action: String,
}

/// A pair ended ([`PairEnd`] says how).
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct PairEnded {
    pub attacker: Entity,
    /// `None` when the victim is gone.
    pub victim: Option<Entity>,
    pub action: String,
    pub end: PairEnd,
    /// The victim died of the pair's own blow (its `Died` named the attacker and the pair's action): the journal says
    /// `killed` for an end that is `Lost` and killed, `lost` for a victim lost otherwise. The end stays `Lost`.
    pub killed: bool,
}

impl PairEnded {
    /// The word the journal ends the line with: `killed` for a victim that died of the pair's own blow, else `PairEnd::describe`.
    pub fn describe(&self) -> &'static str {
        if self.killed && self.end == PairEnd::Lost { "killed" } else { self.end.describe() }
    }
}

/// How a pair ended ([`PairEnded`]).
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairEnd {
    /// The attacker's run ran out.
    Finished,
    /// The victim died, is gone, or got farther than `break_range`.
    Lost,
    /// The attacker's run was cut: another action, a hit (`cancel_on_hurt`), a death.
    Cut,
}

impl PairEnd {
    pub fn describe(self) -> &'static str {
        match self {
            PairEnd::Finished => "finished",
            PairEnd::Lost => "lost",
            PairEnd::Cut => "cut",
        }
    }
}

// --- the API -------------------------------------------------------------------------------

/// `entity` remembers `key` for `seconds` (again: the time starts over). Kept sorted by key.
pub fn remember(world: &mut World, entity: Entity, key: &str, seconds: f32) {
    let Ok(mut e) = world.get_entity_mut(entity) else { return };
    match e.get_mut::<Memory>() {
        Some(mut memory) => match memory.0.binary_search_by(|m| m.key.as_str().cmp(key)) {
            Ok(i) => memory.0[i].left = seconds,
            Err(i) => memory.0.insert(i, Remembered { key: key.to_owned(), left: seconds }),
        },
        None => {
            e.insert(Memory(vec![Remembered { key: key.to_owned(), left: seconds }]));
        }
    }
}

/// `entity` forgets `key`; `true` if it remembered it.
pub fn forget(world: &mut World, entity: Entity, key: &str) -> bool {
    let Some(mut memory) = world.get_mut::<Memory>(entity) else { return false };
    let Some(i) = memory.0.iter().position(|m| m.key == key) else { return false };
    memory.0.remove(i);
    if memory.0.is_empty() {
        world.entity_mut(entity).remove::<Memory>();
    }
    true
}

/// Stops `entity`'s running action now, with nothing after it (`ActionEnded` with
/// `Interrupted`). `false` if it ran none.
pub fn cut_action(world: &mut World, entity: Entity) -> bool {
    let Some(old) = world.get_mut::<ActionState>(entity).and_then(|mut s| s.current.take()) else { return false };
    interrupted(world, entity, &old);
    world.trigger(ActionEnded::of(entity, &old, ActionEnd::Interrupted));
    true
}

/// A broken poise stays broken at least `seconds` more; nothing happens to one that is not broken.
pub fn hold_break(world: &mut World, entity: Entity, seconds: f32) {
    if let Some(mut poise) = world.get_mut::<Poise>(entity)
        && poise.is_broken()
        && poise.broken < seconds
    {
        poise.broken = seconds;
    }
}

/// The context `entity` has now ([`ContextInputs`]): for each input, the first binding of its
/// moveset whose conditions all hold. `None` for a fighter whose moveset has no `context`. Pure:
/// the machine's resolve and the tools' answers both ask it.
pub fn resolve(world: &World, entity: Entity) -> Option<ContextInputs> {
    let db = world.get_resource::<ContentDb>()?;
    let moveset = db.get::<MovesetDef>(&world.get::<Fighter>(entity)?.moveset)?;
    if moveset.context.is_empty() {
        return None;
    }
    let mut out = ContextInputs::default();
    if world.get::<Dead>(entity).is_some() {
        return Some(out);
    }
    let running = world.get::<ActionState>(entity).and_then(|s| s.current.as_ref()).map(|r| r.action.as_str());
    let memory = world.get::<Memory>(entity);
    let stamina = world.get::<Stamina>(entity);
    let conditions = world.get_resource::<ContextConditions>();
    let mode = world.get::<MotionState>(entity).map(|m| m.mode);
    for (i, binding) in moveset.context.iter().enumerate() {
        if out.bound(&binding.input).is_some() || running == Some(binding.action.as_str()) {
            continue;
        }
        // The medium refuses it (`only_in`): it does not bind, so the input falls to the next binding or the
        // moveset's action, and nothing that reads the context (the HUD's finisher prompt, `finishable by`) sees it.
        if mode.is_some_and(|m| moveset.gates(m) && db.get::<ActionDef>(&binding.action).is_some_and(|d| !moveset.allows(m, d))) {
            continue;
        }
        let mut victim = None;
        let holds = binding.when.iter().all(|when| match when {
            When::Remembers(key) => memory.is_some_and(|m| m.has(key)),
            When::Pair => {
                let select = db.get::<ActionDef>(&binding.action).and_then(|d| d.pair.as_ref()).map(|p| &p.select);
                victim = select.and_then(|s| pair_victim(world, entity, s)).and_then(|v| world.get::<AiId>(v).copied());
                victim.is_some()
            }
            When::StaminaAbove(v) => stamina.is_none_or(|s| s.current > *v),
            When::Condition(name) => conditions.and_then(|c| c.get(name)).is_some_and(|f| f(world, entity)),
        });
        if holds {
            out.bound.push(Bound { input: binding.input.clone(), action: binding.action.clone(), binding: i as u32, victim });
        }
    }
    Some(out)
}

/// Where `entity` faces for the angle test of a pair's `select`. [`PairAngle::Facing`] (the default): its body's facing, as
/// design 6.4 and the original game's finisher candidate have it (not the lock-on direction and not the stick: a player running
/// away from a broken enemy has no finisher on it, and a locked player whose body looks elsewhere is judged by the body).
/// [`PairAngle::Intent`]: the direction to its lock-on target, else its movement input's direction, else the facing.
fn meant_facing(world: &World, entity: Entity, from: PairAngle) -> Vec3 {
    let facing = world.get::<Transform>(entity).and_then(|t| flat(*t.forward()).try_normalize()).unwrap_or(Vec3::NEG_Z);
    if from == PairAngle::Facing {
        return facing;
    }
    let here = world.get::<Transform>(entity).map(|t| t.translation);
    let intent = world.get::<Intent>(entity);
    let locked = intent
        .and_then(|i| i.target)
        .and_then(|id| world.resource::<AiIndex>().get(id))
        .and_then(|e| world.get::<Transform>(e))
        .zip(here)
        .and_then(|(there, here)| flat(there.translation - here).try_normalize());
    locked.or_else(|| intent.and_then(|i| flat(i.move_dir).try_normalize())).unwrap_or(facing)
}

/// Whether `candidate` is a victim `entity`'s pair `select` may take now (every rule but the
/// choice among several).
fn qualifies(world: &World, entity: Entity, candidate: Entity, select: &PairSelect, face: Vec3) -> bool {
    if candidate == entity || world.get::<Health>(candidate).is_none() || world.get::<Dead>(candidate).is_some() {
        return false;
    }
    let me = world.get::<AiId>(entity).copied();
    if world.get::<Paired>(candidate).is_some_and(|p| Some(p.by) != me) {
        return false;
    }
    let (mine, theirs) = (world.get::<Team>(entity), world.get::<Team>(candidate));
    let team_ok = match select.team {
        PairTeam::Hostile => hostile(mine, theirs),
        PairTeam::Ally => mine.is_some() && mine == theirs,
    };
    let state_ok = match select.state {
        PairState::Broken => world.get::<Poise>(candidate).is_some_and(Poise::is_broken),
        PairState::Any => true,
    };
    if !team_ok || !state_ok {
        return false;
    }
    let (Some(here), Some(there)) =
        (world.get::<Transform>(entity).map(|t| t.translation), world.get::<Transform>(candidate).map(|t| t.translation))
    else {
        return false;
    };
    if (there.y - here.y).abs() > select.height + EPS {
        return false;
    }
    let to = flat(there - here);
    if to.length() > select.range + body_radius(world, candidate) + EPS {
        return false;
    }
    if let Some(dir) = to.try_normalize()
        && dir.angle_between(face).to_degrees() > select.angle + EPS
    {
        return false;
    }
    if select.line_of_sight {
        let sight = CombatTuning::of(world.resource::<ContentDb>()).sight_height;
        if !clear_line(world, world.resource::<PhysicsWorld>(), entity, candidate, sight) {
            return false;
        }
    }
    if let Some(slack) = select.slack {
        let mine_flat = to.length();
        let nearer = world
            .try_query_filtered::<(Entity, &Transform, Option<&Team>), (With<PlayerId>, Without<Dead>)>()
            .is_some_and(|mut q| {
                q.iter(world).any(|(other, t, team)| {
                    other != entity && other != candidate && team == mine && flat(there - t.translation).length() + slack < mine_flat - EPS
                })
            });
        if nearer {
            return false;
        }
    }
    true
}

/// The victim `entity`'s pair `select` takes now: its lock-on target if `prefer_target` and it
/// qualifies, else the nearest that qualifies (3D distance, strictly nearer; ties: the lower `AiId`).
pub fn pair_victim(world: &World, entity: Entity, select: &PairSelect) -> Option<Entity> {
    let here = world.get::<Transform>(entity)?.translation;
    let face = meant_facing(world, entity, select.angle_from);
    if select.prefer_target
        && let Some(locked) = world.get::<Intent>(entity).and_then(|i| i.target).and_then(|id| world.resource::<AiIndex>().get(id))
        && qualifies(world, entity, locked, select, face)
    {
        return Some(locked);
    }
    let mut candidates: Vec<(u32, Entity, Vec3)> = world
        .try_query_filtered::<(Entity, &AiId, &Transform), (With<Health>, Without<Dead>)>()?
        .iter(world)
        .map(|(e, id, t)| (id.0, e, t.translation))
        .collect();
    candidates.sort_by_key(|c| c.0);
    let mut best: Option<(f32, Entity)> = None;
    for (_, candidate, there) in candidates {
        let d = here.distance(there);
        if best.is_some_and(|(b, _)| d >= b) || !qualifies(world, entity, candidate, select, face) {
            continue;
        }
        best = Some((d, candidate));
    }
    best.map(|(_, e)| e)
}

// --- guard presses -------------------------------------------------------------------------

/// The press rule of `moveset`'s guard, and the inputs that start it: the inputs whose action is a
/// guard with `perfect_needs_press` (the first such guard's rule, by input name).
///
/// The scan peeks: only the guards that have the rule count as read, so a moveset without one
/// reads nothing new and the replays of content made before the rule keep their `content` parts.
/// A rule added to a guard later is still named by a replay check: that run reads the guard, the
/// recording did not.
pub(crate) fn guard_press<'a>(db: &'a ContentDb, moveset: &'a MovesetDef) -> Option<(GuardPress, Vec<&'a str>)> {
    let mut rule = None;
    let mut inputs = Vec::new();
    for (input, action) in &moveset.inputs {
        let needs = db.peek::<ActionDef>(action).and_then(|d| d.guard.as_ref()).is_some_and(|g| g.perfect_needs_press.is_some());
        if needs && let Some(press) = db.get::<ActionDef>(action).and_then(|d| d.guard.as_ref()).and_then(|g| g.perfect_needs_press) {
            rule.get_or_insert(press);
            inputs.push(input.as_str());
        }
    }
    rule.map(|r| (r, inputs))
}

/// Seconds between two ticks.
fn since(now: u64, then: u64, dt: f32) -> f32 {
    now.saturating_sub(then) as f32 * dt
}

/// Whether a guard of rule `press`, starting now, opens its perfect window: the last press was
/// fresh, no guard started from it yet, and it came at most `entry` ago.
pub(crate) fn fresh_press(arm: Option<&GuardArm>, press: &GuardPress, now: u64, dt: f32) -> bool {
    arm.is_some_and(|a| a.armed && a.pressed.is_some_and(|p| since(now, p, dt) <= press.entry + EPS))
}

/// One tick of the press rule on `arm`: `pressed` is a press of a guard input in this tick's
/// `Intent`, `down` a guard input held, `running` the instance of the action the body runs. The
/// host's `arm_guard` and a client's prediction both call it, so they agree whenever the presses do.
pub(crate) fn step_arm(mut arm: GuardArm, press: &GuardPress, pressed: bool, down: bool, running: Option<u32>, now: u64, dt: f32) -> GuardArm {
    if pressed || (down && !arm.down) {
        arm.armed = arm.pressed.is_none_or(|p| since(now, p, dt) + EPS >= press.gap);
        arm.pressed = Some(now);
    }
    arm.down = down;
    if arm.run.is_some() && arm.run != running {
        arm.run = None;
    }
    arm
}

/// Whether `arm` says nothing an absent component would not.
pub(crate) fn arm_idle(arm: &GuardArm, press: &GuardPress, now: u64, dt: f32) -> bool {
    !arm.down && arm.run.is_none() && arm.pressed.is_none_or(|p| since(now, p, dt) + EPS >= press.gap)
}

/// This tick's guard presses of `entity` (an `Intent` press of a guard input, or one becoming
/// held): a press is fresh when the one before it is `gap` old (or there was none). The component
/// goes once it says nothing an absent one would not.
fn arm_guard(world: &mut World, entity: Entity, now: u64, dt: f32) {
    let db = world.resource::<ContentDb>();
    let moveset = world.get::<Fighter>(entity).and_then(|f| db.get::<MovesetDef>(&f.moveset));
    let Some((press, inputs)) = moveset.and_then(|m| guard_press(db, m)) else {
        if world.get::<GuardArm>(entity).is_some() {
            world.entity_mut(entity).remove::<GuardArm>();
        }
        return;
    };
    let intent = world.get::<Intent>(entity);
    let down = intent.is_some_and(|i| inputs.iter().any(|input| i.is_held(input)));
    let pressed = intent.is_some_and(|i| i.pressed.iter().any(|p| inputs.contains(&p.input.as_str())));
    let running = world.get::<ActionState>(entity).and_then(|s| s.current.as_ref()).map(|r| r.instance);
    let old = world.get::<GuardArm>(entity).copied();
    let arm = step_arm(old.unwrap_or_default(), &press, pressed, down, running, now, dt);
    let idle = arm_idle(&arm, &press, now, dt);
    match (idle, old) {
        (true, Some(_)) => {
            world.entity_mut(entity).remove::<GuardArm>();
        }
        (true, None) => {}
        (false, Some(o)) if o == arm => {}
        (false, _) => {
            world.entity_mut(entity).insert(arm);
        }
    }
}

// --- systems -------------------------------------------------------------------------------

/// `ActionSystems::Context`: every fighter's guard presses and context, in `AiId` order.
pub(crate) fn resolve_context(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let now = world.resource::<SimTick>().0;
    let mut fighters: Vec<(u32, Entity)> = world
        .query_filtered::<(Entity, Option<&AiId>), (With<Fighter>, With<ActionState>)>()
        .iter(world)
        .map(|(e, id)| (id.map_or(u32::MAX, |i| i.0), e))
        .collect();
    fighters.sort();
    for (_, entity) in fighters {
        arm_guard(world, entity, now, dt);
        let context = resolve(world, entity).filter(|c| !c.bound.is_empty());
        match (context, world.get::<ContextInputs>(entity)) {
            (Some(c), Some(there)) if *there == c => {}
            (Some(c), _) => {
                world.entity_mut(entity).insert(c);
            }
            (None, Some(_)) => {
                world.entity_mut(entity).remove::<ContextInputs>();
            }
            (None, None) => {}
        }
    }
}

/// `ActionSystems::Effects`, before the effects: a paired action the machine started this tick
/// takes its victim (the one its binding found, else the one its `select` finds now; none: it
/// runs unpaired), forces the victim action on it, holds its break, and says so ([`PairStarted`]).
pub(crate) fn start_pairs(world: &mut World) {
    let pending = std::mem::take(&mut world.resource_mut::<PendingPairs>().0);
    for p in pending {
        let running = world
            .get::<ActionState>(p.attacker)
            .and_then(|s| s.current.as_ref())
            .is_some_and(|r| r.instance == p.instance && r.action == p.action);
        let Some(pair) = world.resource::<ContentDb>().get::<ActionDef>(&p.action).and_then(|d| d.pair.clone()) else { continue };
        let Some(me) = world.get::<AiId>(p.attacker).copied() else { continue };
        if !running || world.get::<Dead>(p.attacker).is_some() {
            continue;
        }
        let free = |world: &World, v: Entity| {
            world.get::<Dead>(v).is_none() && world.get::<Health>(v).is_some() && world.get::<Paired>(v).is_none_or(|x| x.by == me)
        };
        let named = p.victim.and_then(|id| world.resource::<AiIndex>().get(id)).filter(|v| free(world, *v));
        let Some(victim) = named.or_else(|| pair_victim(world, p.attacker, &pair.select)) else { continue };
        let Some(victim_id) = world.get::<AiId>(victim).copied() else { continue };
        world.entity_mut(p.attacker).insert(Pairing { victim: victim_id, action: p.action.clone(), instance: p.instance, broke_once: false });
        world.entity_mut(victim).insert(Paired { by: me, exclusive: pair.exclusive });
        if let Some(action) = &pair.victim_action {
            crate::machine::force_action(world, victim, action);
        }
        hold_break(world, victim, pair.hold_break);
        world.trigger(PairStarted { attacker: p.attacker, victim, action: p.action });
    }
}

/// The pair `attacker` runs, with its definition, if it runs one.
fn pair_of(world: &World, attacker: Entity) -> Option<(&Pairing, PairDef)> {
    let pairing = world.get::<Pairing>(attacker)?;
    let def = world.resource::<ContentDb>().get::<ActionDef>(&pairing.action)?.pair.clone()?;
    Some((pairing, def))
}

/// Ends `attacker`'s pair (`end` says how): both marks go, a victim still in the pair's victim
/// action is let go (its action is cut) unless the pair finished, and [`PairEnded`] says so. The
/// one way every pair ends.
pub fn release_pair(world: &mut World, attacker: Entity, end: PairEnd) {
    let Some(pairing) = world.get_entity_mut(attacker).ok().and_then(|mut e| e.take::<Pairing>()) else { return };
    let victim_action = world.resource::<ContentDb>().get::<ActionDef>(&pairing.action).and_then(|d| d.pair.as_ref()?.victim_action.clone());
    let me = world.get::<AiId>(attacker).copied();
    let victim = world.resource::<AiIndex>().get(pairing.victim);
    if let Some(v) = victim {
        if world.get::<Paired>(v).is_some_and(|p| Some(p.by) == me) {
            world.entity_mut(v).remove::<Paired>();
        }
        let held = world.get::<ActionState>(v).and_then(|s| s.current.as_ref()).is_some_and(|r| Some(&r.action) == victim_action.as_ref());
        if end != PairEnd::Finished && held && world.get::<Dead>(v).is_none() {
            cut_action(world, v);
        }
    }
    let killed = world.entity_mut(attacker).take::<PairKilled>().is_some();
    world.trigger(PairEnded { attacker, victim, action: pairing.action, end, killed });
}

/// `ActionSystems::Fighters`: pairs whose victim died, is gone or (with `break_range`) got too far
/// are lost, in `AiId` order; a victim lost to its death stops the attacker's run where it is, one
/// lost to the distance cuts it. A mark whose other side is gone goes, and a halt outlives no run.
pub(crate) fn tend_pairs(world: &mut World) {
    let mut attackers: Vec<(u32, Entity)> =
        world.query::<(Entity, &AiId, &Pairing)>().iter(world).map(|(e, id, _)| (id.0, e)).collect();
    attackers.sort();
    for (_, attacker) in attackers {
        let Some((pairing, def)) = pair_of(world, attacker) else {
            release_pair(world, attacker, PairEnd::Cut);
            continue;
        };
        let instance = pairing.instance;
        let runs = world.get::<ActionState>(attacker).and_then(|s| s.current.as_ref()).is_some_and(|r| r.instance == instance);
        if !runs || world.get::<Dead>(attacker).is_some() {
            release_pair(world, attacker, PairEnd::Cut);
            continue;
        }
        let victim = world.resource::<AiIndex>().get(pairing.victim);
        match victim {
            Some(v) if world.get::<Dead>(v).is_none() => {
                let (Some(a), Some(b)) = (world.get::<Transform>(attacker), world.get::<Transform>(v)) else { continue };
                let apart = flat(b.translation - a.translation).length();
                if def.break_range.is_some_and(|r| apart > r + EPS) {
                    release_pair(world, attacker, PairEnd::Lost);
                    cut_action(world, attacker);
                }
            }
            _ => {
                release_pair(world, attacker, PairEnd::Lost);
                world.entity_mut(attacker).insert(RunHalted { instance });
            }
        }
    }
    // A victim whose attacker is gone, or no longer holds it.
    let mut victims: Vec<(u32, Entity, AiId)> =
        world.query::<(Entity, &AiId, &Paired)>().iter(world).map(|(e, id, p)| (id.0, e, p.by)).collect();
    victims.sort_by_key(|v| v.0);
    for (id, victim, by) in victims {
        let holds = world.resource::<AiIndex>().get(by).and_then(|a| world.get::<Pairing>(a)).is_some_and(|p| p.victim.0 == id);
        if !holds {
            world.entity_mut(victim).remove::<Paired>();
        }
    }
    let mut halted: Vec<(Entity, u32)> = world.query::<(Entity, &RunHalted)>().iter(world).map(|(e, h)| (e, h.instance)).collect();
    halted.sort_by_key(|h| h.1);
    for (entity, instance) in halted {
        if !world.get::<ActionState>(entity).and_then(|s| s.current.as_ref()).is_some_and(|r| r.instance == instance) {
            world.entity_mut(entity).remove::<RunHalted>();
        }
    }
}

/// `ActionSystems::Fighters`: memories age and the ones out of time go.
pub(crate) fn age_memory(time: Res<Time>, mut commands: Commands, mut memories: Query<(Entity, &mut Memory)>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (entity, mut memory) in &mut memories {
        for m in &mut memory.0 {
            m.left -= dt;
        }
        memory.0.retain(|m| m.left > EPS);
        if memory.0.is_empty() {
            commands.entity(entity).remove::<Memory>();
        }
    }
}

// --- what feeds memory and ends pairs (host only) --------------------------------------------

/// Something that happened to a fighter, for its moveset's `remember`.
#[derive(Clone, Debug)]
enum Happened {
    Parried,
    PerfectEvaded,
    Blocked,
    Evaded,
    Landed,
    Hurt,
    Reacted,
    Ended(String),
    Started(String),
}

impl Happened {
    fn recalls(&self, on: &Recall, tags: &[String]) -> bool {
        match (self, on) {
            (Happened::Parried, Recall::Parried)
            | (Happened::PerfectEvaded, Recall::PerfectEvaded)
            | (Happened::Blocked, Recall::Blocked)
            | (Happened::Evaded, Recall::Evaded)
            | (Happened::Landed, Recall::Landed)
            | (Happened::Hurt, Recall::Hurt) => true,
            (Happened::Ended(_), Recall::Ended(tag)) | (Happened::Started(_), Recall::Started(tag)) => tags.contains(tag),
            _ => false,
        }
    }

    fn forgets(&self, forget: &Forget, tags: &[String]) -> bool {
        match (self, forget) {
            (Happened::Hurt, Forget::Hurt) | (Happened::Reacted, Forget::Reacted) => true,
            (Happened::Started(_), Forget::Started(tag)) => tags.contains(tag),
            _ => false,
        }
    }
}

/// `entity`'s moveset hears `what`: memories it makes forget go first, then the ones it makes
/// remember start (again). Nothing at all for a moveset that remembers nothing.
fn hear(world: &mut World, entity: Entity, what: Happened) {
    let db = world.resource::<ContentDb>();
    let Some(moveset) = world.get::<Fighter>(entity).and_then(|f| db.get::<MovesetDef>(&f.moveset)) else { return };
    if moveset.remember.is_empty() {
        return;
    }
    let tags: Vec<String> = match &what {
        Happened::Ended(action) | Happened::Started(action) => db.get::<ActionDef>(action).map(|d| d.tags.clone()).unwrap_or_default(),
        _ => Vec::new(),
    };
    let forgotten: Vec<String> =
        moveset.remember.iter().filter(|m| m.forget.iter().any(|f| what.forgets(f, &tags))).map(|m| m.key.clone()).collect();
    let remembered: Vec<(String, f32)> =
        moveset.remember.iter().filter(|m| what.recalls(&m.on, &tags)).map(|m| (m.key.clone(), m.seconds)).collect();
    for key in forgotten {
        forget(world, entity, &key);
    }
    for (key, seconds) in remembered {
        remember(world, entity, &key, seconds);
    }
}

/// A bound start used its binding: the memories the binding needs (`Remembers`) whose `forget`
/// says `Used` go.
fn use_binding(world: &mut World, entity: Entity, input: &str, action: &str) {
    let Some(bound) = world.get::<ContextInputs>(entity).and_then(|c| c.bound(input)).filter(|b| b.action == action).map(|b| b.binding)
    else {
        return;
    };
    let db = world.resource::<ContentDb>();
    let Some(moveset) = world.get::<Fighter>(entity).and_then(|f| db.get::<MovesetDef>(&f.moveset)) else { return };
    let Some(binding) = moveset.context.get(bound as usize) else { return };
    let used: Vec<String> = binding
        .when
        .iter()
        .filter_map(|w| if let When::Remembers(key) = w { Some(key) } else { None })
        .filter(|key| moveset.remember.iter().any(|m| m.key == **key && m.forget.contains(&Forget::Used)))
        .cloned()
        .collect();
    for key in used {
        forget(world, entity, &key);
    }
}

fn host(role: &Option<Res<NetRole>>) -> bool {
    role.as_ref().is_none_or(|r| r.simulates())
}

fn on_parried(e: On<Parried>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if !host(&role) {
        return;
    }
    let defender = e.event().defender;
    commands.queue(move |world: &mut World| {
        rearm(world, defender);
        hear(world, defender, Happened::Parried);
    });
}

/// A parry makes the defender's next guard press fresh (its rule's `rearm`).
fn rearm(world: &mut World, defender: Entity) {
    let db = world.resource::<ContentDb>();
    let rule = world.get::<Fighter>(defender).and_then(|f| db.get::<MovesetDef>(&f.moveset)).and_then(|m| guard_press(db, m)).map(|(r, _)| r);
    if rule.is_some_and(|r| r.rearm)
        && let Some(mut arm) = world.get_mut::<GuardArm>(defender)
    {
        arm.pressed = None;
        arm.armed = false;
    }
}

fn on_perfect_evaded(e: On<PerfectEvaded>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if host(&role) {
        let defender = e.event().defender;
        commands.queue(move |world: &mut World| hear(world, defender, Happened::PerfectEvaded));
    }
}

fn on_blocked(e: On<Blocked>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if host(&role) {
        let defender = e.event().defender;
        commands.queue(move |world: &mut World| hear(world, defender, Happened::Blocked));
    }
}

fn on_evaded(e: On<Evaded>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if host(&role) {
        let defender = e.event().defender;
        commands.queue(move |world: &mut World| hear(world, defender, Happened::Evaded));
    }
}

/// `Landed` for the attacker of a hit that was not ignored; `Hurt` for a target that lost health,
/// and `cancel_on_hurt` cuts a paired action whose attacker it is.
fn on_damaged(e: On<Damaged>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if !host(&role) {
        return;
    }
    let (attacker, target, ignored, hurt) = (e.event().attacker, e.event().target, e.event().invulnerable, e.event().amount > 0.0);
    commands.queue(move |world: &mut World| {
        if !ignored {
            hear(world, attacker, Happened::Landed);
        }
        if hurt {
            hear(world, target, Happened::Hurt);
            if pair_of(world, target).is_some_and(|(_, def)| def.cancel_on_hurt) {
                cut_action(world, target);
            }
        }
    });
}

/// A victim that dies of its pair's own blow: the attacker's pair is marked [`PairKilled`], so its end says so.
fn on_died(e: On<Died>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if !host(&role) {
        return;
    }
    let (victim, killer, action) = (e.event().entity, e.event().killer, e.event().action.clone());
    let (Some(killer), Some(action)) = (killer, action) else { return };
    commands.queue(move |world: &mut World| {
        let victim = world.get::<AiId>(victim).copied();
        if victim.is_some() && world.get::<Pairing>(killer).is_some_and(|p| Some(p.victim) == victim && p.action == action) {
            world.entity_mut(killer).insert(PairKilled);
        }
    });
}

fn on_reacted(e: On<Reacted>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if host(&role) {
        let entity = e.event().entity;
        commands.queue(move |world: &mut World| hear(world, entity, Happened::Reacted));
    }
}

fn on_staggered(e: On<Staggered>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if host(&role) {
        let entity = e.event().entity;
        commands.queue(move |world: &mut World| hear(world, entity, Happened::Reacted));
    }
}

fn on_guard_broken(e: On<GuardBroken>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if host(&role) {
        let defender = e.event().defender;
        commands.queue(move |world: &mut World| hear(world, defender, Happened::Reacted));
    }
}

/// `Started(tag)` (to remember and to forget) and a bound start's `Used`.
fn on_action_started(e: On<ActionStarted>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if !host(&role) || e.event().predicted {
        return;
    }
    let (entity, action, input) = (e.event().entity, e.event().action.clone(), e.event().input.clone());
    commands.queue(move |world: &mut World| {
        if let Some(input) = &input {
            use_binding(world, entity, input, &action);
        }
        hear(world, entity, Happened::Started(action));
    });
}

/// `Ended(tag)`, and the end of a paired action's run ends its pair: `Finished` when its time ran
/// out, `Cut` otherwise (a charge's turn keeps it).
fn on_action_ended(e: On<ActionEnded>, role: Option<Res<NetRole>>, mut commands: Commands) {
    if !host(&role) || e.event().end == ActionEnd::Branched {
        return;
    }
    let (entity, action, instance, end) = (e.event().entity, e.event().action.clone(), e.event().instance, e.event().end);
    commands.queue(move |world: &mut World| {
        if world.get::<Pairing>(entity).is_some_and(|p| p.instance == instance) {
            release_pair(world, entity, if end == ActionEnd::Finished { PairEnd::Finished } else { PairEnd::Cut });
        }
        hear(world, entity, Happened::Ended(action));
    });
}

pub(crate) fn plugin(app: &mut App) {
    use crate::{machine, ActionSystems};
    app.add_systems(
        FixedUpdate,
        (
            resolve_context.in_set(ActionSystems::Context),
            start_pairs.in_set(ActionSystems::Effects).before(machine::apply_effects),
            (age_memory, tend_pairs).chain().in_set(ActionSystems::Fighters).after(crate::body::tick_body_state),
        ),
    )
    .add_observer(on_parried)
        .add_observer(on_died)
        .add_observer(on_perfect_evaded)
        .add_observer(on_blocked)
        .add_observer(on_evaded)
        .add_observer(on_damaged)
        .add_observer(on_reacted)
        .add_observer(on_staggered)
        .add_observer(on_guard_broken)
        .add_observer(on_action_started)
        .add_observer(on_action_ended);
}

// --- what the machine and the pipeline ask -------------------------------------------------

/// The pair strike `ctx`'s hit is, if it is one: the attacker's pair holds this target, in the
/// pair's run, and the target is in the pair's state.
pub(crate) struct Strike {
    pub tier: u8,
    pub end_break: bool,
}

pub(crate) fn strike(world: &World, attacker: Entity, target: Entity, action: &str) -> Option<Strike> {
    let (pairing, def) = pair_of(world, attacker)?;
    let runs = world.get::<ActionState>(attacker).and_then(|s| s.current.as_ref()).is_some_and(|r| r.instance == pairing.instance);
    if !runs || pairing.action != action || world.get::<AiId>(target) != Some(&pairing.victim) {
        return None;
    }
    let broken = world.get::<Poise>(target).is_some_and(Poise::is_broken);
    let in_state = match def.select.state {
        PairState::Broken => broken,
        PairState::Any => true,
    };
    in_state.then_some(Strike { tier: def.strike_reaction, end_break: def.ends_break && !pairing.broke_once && broken })
}

/// Who holds `target` in a pair, if anyone does.
pub(crate) fn held_by(world: &World, target: Entity) -> Option<Entity> {
    world.get::<Paired>(target).and_then(|p| world.resource::<AiIndex>().get(p.by))
}

/// Whether `target` runs the victim action of the pair that holds it.
pub(crate) fn in_victim_action(world: &World, target: Entity) -> bool {
    let Some(attacker) = held_by(world, target) else { return false };
    let Some((_, def)) = pair_of(world, attacker) else { return false };
    def.victim_action.as_ref().is_some_and(|a| world.get::<ActionState>(target).is_some_and(|s| s.is(a)))
}

/// Whether `attacker`'s hits may find `target` while pairs run: a pair attacker with `only_victim`
/// finds only its victim; a victim held `exclusive` is found only by its own attacker.
pub(crate) fn may_hit(world: &World, attacker: Entity, instance: u32, target: Entity) -> bool {
    if let Some((pairing, def)) = pair_of(world, attacker)
        && def.only_victim
        && pairing.instance == instance
        && world.get::<AiId>(target) != Some(&pairing.victim)
    {
        return false;
    }
    // A pair whose victim was lost to its death (or its despawn) leaves the run halted, not free to
    // swing at whoever else stands in the cone: `only_victim` holds to the end of the run.
    if world.get::<RunHalted>(attacker).is_some_and(|h| h.instance == instance)
        && world
            .get::<ActionState>(attacker)
            .and_then(|s| s.current.as_ref())
            .and_then(|r| world.resource::<ContentDb>().get::<ActionDef>(&r.action)?.pair.as_ref().map(|p| p.only_victim))
            .unwrap_or(false)
    {
        return false;
    }
    match world.get::<Paired>(target) {
        Some(p) if p.exclusive => world.get::<AiId>(attacker) == Some(&p.by),
        _ => true,
    }
}

// --- describing ------------------------------------------------------------------------------

/// `remembers parried 0.32s`.
pub fn describe_memory(m: &Memory) -> String {
    m.0.iter().map(|r| format!("remembers {} {:.2}s", r.key, r.left.max(0.0))).collect::<Vec<_>>().join("  ")
}

/// `light -> spar_finisher on e7`.
pub fn describe_context(c: &ContextInputs) -> String {
    c.bound
        .iter()
        .map(|b| match b.victim {
            Some(v) => format!("{} -> {} on {v}", b.input, b.action),
            None => format!("{} -> {}", b.input, b.action),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// `pairing e7`.
pub fn describe_pairing(p: &Pairing) -> String {
    format!("pairing {}", p.victim)
}

/// `held by e5`.
pub fn describe_paired(p: &Paired) -> String {
    format!("held by {}", p.by)
}

/// `guard armed (perfect window)` for the guard a fresh press started, `guard press armed` for a
/// fresh press no guard took yet, `guard press not armed` for a press too soon after the last.
pub fn describe_guard_arm(a: &GuardArm) -> String {
    match (a.run, a.armed) {
        (Some(_), _) => "guard armed (perfect window)".to_owned(),
        (None, true) => "guard press armed".to_owned(),
        (None, false) => "guard press not armed".to_owned(),
    }
}

/// The players that could pair with `victim` now: the fighters with `PlayerId` whose context binds
/// a paired action on it, in `AiId` order (`finishable by e5`).
pub fn finishable_by(world: &World, victim: Entity) -> Vec<AiId> {
    let Some(id) = world.get::<AiId>(victim).copied() else { return Vec::new() };
    let mut by: Vec<AiId> = world
        .try_query_filtered::<(&AiId, &ContextInputs), With<PlayerId>>()
        .map(|mut q| q.iter(world).filter(|(_, c)| c.bound.iter().any(|b| b.victim == Some(id))).map(|(a, _)| *a).collect())
        .unwrap_or_default();
    by.sort();
    by
}
