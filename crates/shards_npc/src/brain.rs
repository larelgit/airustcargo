//! The behaviour framework: a state machine in content ([`BehaviorDef`]), logic in code.
//!
//! Every tick each active NPC (in `AiId` order) first tries the transitions of its state, in
//! order: the first whose **conditions** all hold is taken (one per tick), then the **state
//! hook** of the (new) state runs and says what the NPC wants this tick ([`NpcOutput`]): where
//! to walk, what to face, whether it wants an attack token or to strike, what to press.
//!
//! Conditions and states are plain functions registered by name, the engine's and the
//! game's alike ([`NpcAppExt`](crate::NpcAppExt)). They read the world through [`NpcCtx`] and
//! never write it: every NPC decides on the same world, so the order NPCs are visited in
//! changes nothing, and the results are applied after all of them decided.
//!
//! Engine conditions: `target_known`, `target_seen`, `target_heard`, `target_lost`,
//! `target_within {meters}`, `target_in_leash {margin}`, `beyond_leash {margin}`, `at_home
//! {within}`, `arrived`, `has_token`, `attack_done`, `resting`, `busy`, `timer_done`,
//! `in_state {secs}`, `health_below {fraction}`, `heard_noise`, `chance {p}`, `deciding`.
//!
//! Engine states: `idle`, `hold`, `patrol {speed, within, wait}`, `chase {speed, within}`,
//! `return_home {speed, heal}`, `wait_token {speed}`, `attack {speed}`, `flee {speed,
//! distance}`, `investigate {speed}`.

use std::collections::BTreeMap;

use ai_core::journal::{self, JournalKind};
use ai_core::{AiId, AiVisible, DetRng, SimSeed};
use bevy::prelude::*;
use shards_action::{ActionState, Dead, Downed, Health};
use shards_content::ContentDb;
use shards_core::flat;
use shards_world::Dormant;

use crate::def::{AttackPlanDef, BehaviorDef, Cond, DirectorDef, Params, PerceptionDef};
use crate::director::{AttackToken, Grant};
use crate::nav::NavAgent;
use crate::perception::Perceived;
use crate::planner::{NpcOptions, NpcPlan};

/// Salts of the NPC's random streams (`SimRng`: seed, tick, id, salt).
pub(crate) mod salt {
    /// A state's duration, drawn on entry.
    pub const ENTRY: u64 = 0x4E50_0001;
    /// Which attack the planner picks.
    pub const PICK: u64 = 0x4E50_0002;
    /// The rest after an attack.
    pub const REST: u64 = 0x4E50_0003;
    /// The rest after an attack that was interrupted.
    pub const REST_INTERRUPTED: u64 = 0x4E50_0004;
    /// The rest an NPC starts with.
    pub const FIRST_REST: u64 = 0x4E50_0005;
    /// Whether an attack chains.
    pub const CHAIN: u64 = 0x4E50_0006;
    /// A plan option's `when`: plus the option and condition index.
    pub const OPTION: u64 = 0x4E50_2000;
    /// Condition `chance`: plus the transition and condition index.
    pub const CONDITION: u64 = 0x4E50_1000;
}

/// An NPC: the content ids it behaves, senses and attacks by. Its brain, perception, nav
/// agent, attack token and plan state come with it, and an `AiId` (NPCs think in id order
/// and the AI sees them).
///
/// ```ron
/// "Npc": (behavior: "grunt", perception: "grunt", plan: Some("grunt")),
/// ```
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
#[require(AiVisible, NpcBrain, NpcDrive, Perceived, NavAgent, AttackToken, NpcPlan)]
pub struct Npc {
    /// `behavior` content id: its state machine.
    pub behavior: String,
    /// `perception` content id; empty: the engine's defaults.
    pub perception: String,
    /// `attack_plan` content id; `None`: it never attacks on its own.
    pub plan: Option<String>,
    /// `director` content id whose tokens it fights by; empty: `default`.
    pub director: String,
}

impl Npc {
    pub fn new(behavior: impl Into<String>, perception: impl Into<String>) -> Self {
        Self { behavior: behavior.into(), perception: perception.into(), ..default() }
    }

    pub fn with_plan(mut self, plan: impl Into<String>) -> Self {
        self.plan = Some(plan.into());
        self
    }

    pub fn with_director(mut self, director: impl Into<String>) -> Self {
        self.director = director.into();
        self
    }
}

/// Where an NPC belongs and how far it may chase from there. Beyond the leash it does not
/// go (its goals are kept inside), and conditions `target_in_leash` / `beyond_leash` let its
/// behaviour turn back.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Home {
    /// `None`: where it stands on its first tick.
    pub pos: Option<Vec3>,
    /// Meters from home.
    pub leash: f32,
}

impl Default for Home {
    fn default() -> Self {
        Self { pos: None, leash: 20.0 }
    }
}

impl Home {
    pub fn at(pos: Vec3, leash: f32) -> Self {
        Self { pos: Some(pos), leash }
    }
}

/// Waypoints for the `patrol` state, visited in order, round and round.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Patrol {
    pub points: Vec<Vec3>,
}

/// What an NPC's brain is doing. Gameplay state: kept in replays and snapshots.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcBrain {
    /// Current state of its behaviour; empty until its first tick.
    pub state: String,
    /// Seconds in this state.
    pub since: f32,
    /// Seconds left of the state's timer (`duration`, or set by its hook).
    pub timer: f32,
    /// A counter the state keeps (the patrol's waypoint).
    pub step: u32,
    /// A point the state keeps (where it waits, what it investigates).
    pub point: Option<Vec3>,
    /// What it acts on: its perception's focus.
    pub target: Option<AiId>,
    /// `false` while dormant (out of every active zone), dead or down: it does nothing.
    pub active: bool,
    /// Attacks it finished (token used and returned).
    pub attacks_done: u32,
    /// `attacks_done` when it entered this state (for `attack_done`).
    pub done_at_entry: u32,
    /// Its waiting slot around its target, while it waits for a token.
    pub slot: Option<u32>,
    /// The waiting role of that slot (`circle`, `strafe`, `hold`).
    pub role: Option<String>,
    /// Where it is on its circle around the target (radians).
    pub orbit: f32,
}

/// The decision timer of an NPC whose behaviour has a [`BehaviorDef::think`] cadence: only
/// those NPCs carry it. Gameplay state: kept in replays and snapshots.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcThink {
    /// Seconds until its next decision tick; counted down only while it runs no action.
    pub left: f32,
    /// The tick of its last decision: a plan with `press_on_decision` asks for a token, chooses
    /// and presses on it only (`AttackPlanDef::press_on_decision`).
    pub decided: Option<u64>,
}

/// What an NPC wants this tick: its state hook's [`NpcOutput`], refined by the director
/// (waiting roles) and the planner (approach, strike). The nav agent and the `Intent` writer
/// carry it out.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcDrive {
    pub goal: Option<Vec3>,
    pub within: f32,
    /// Movement input: 0.5 walks, 1 runs.
    pub speed: f32,
    pub face: Option<Vec3>,
    pub request_token: bool,
    pub strike: bool,
    pub waiting: bool,
    pub leashed: bool,
    pub press: Vec<String>,
    pub hold: Vec<String>,
    /// Health per second it regains (returning home).
    pub heal: f32,
    /// Inputs it holds in its `Intent` right now.
    pub held: Vec<String>,
}

/// What a state hook asks for this tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NpcOutput {
    /// Walk there (routed around walls, steering clear of other NPCs).
    pub goal: Option<Vec3>,
    /// Arrived within this many meters of the goal.
    pub within: f32,
    /// Movement input: 0.5 walks, 1 runs.
    pub speed: f32,
    /// Face this point (attacks go where an NPC faces).
    pub face: Option<Vec3>,
    /// Ask the director for an attack token.
    pub request_token: bool,
    /// With a token: let the planner choose an attack, close in and press it.
    pub strike: bool,
    /// Without a token: take the director's waiting role around the target.
    pub waiting: bool,
    /// Keep goals inside the leash.
    pub leashed: bool,
    /// Presses this tick (moveset inputs).
    pub press: Vec<String>,
    /// Inputs to hold.
    pub hold: Vec<String>,
    /// Health per second to regain.
    pub heal: f32,
    /// New value of [`NpcBrain::step`].
    pub step: Option<u32>,
    /// New value of [`NpcBrain::timer`].
    pub timer: Option<f32>,
    /// New value of [`NpcBrain::point`].
    pub point: Option<Option<Vec3>>,
    /// Clamp [`NpcPlan::rest`](crate::NpcPlan::rest) to at most this many seconds. Once, not
    /// every tick: a hook that sets it every tick clamps every pause drawn later too (a state
    /// whose `rest_clamp` content field does it on entry needs no hook).
    pub rest_cap: Option<f32>,
}

impl NpcOutput {
    fn into_drive(self, held: Vec<String>) -> NpcDrive {
        NpcDrive {
            goal: self.goal,
            within: self.within,
            speed: self.speed,
            face: self.face,
            request_token: self.request_token,
            strike: self.strike,
            waiting: self.waiting,
            leashed: self.leashed,
            press: self.press,
            hold: self.hold,
            heal: self.heal,
            held,
        }
    }
}

/// The target as the NPC knows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetView {
    pub id: AiId,
    /// `None` once it is gone from the world.
    pub entity: Option<Entity>,
    /// Where it was last seen or heard.
    pub pos: Vec3,
    /// Flat distance to `pos`.
    pub distance: f32,
    /// Seen this tick.
    pub seen: bool,
    /// Heard this tick.
    pub heard: bool,
    /// Seconds since it was last seen or heard.
    pub age: f32,
}

/// What a condition or state hook reads: the NPC, what it knows, and the whole world (read
/// only).
#[derive(Clone, Copy)]
pub struct NpcCtx<'a> {
    pub world: &'a World,
    pub entity: Entity,
    pub id: AiId,
    /// Feet.
    pub pos: Vec3,
    /// Facing on the ground plane.
    pub facing: Vec3,
    pub brain: &'a NpcBrain,
    pub perceived: &'a Perceived,
    pub home: Option<Vec3>,
    /// Meters from home it may go (infinite without a [`Home`]).
    pub leash: f32,
    pub token: Option<&'a Grant>,
    pub plan: &'a NpcPlan,
    pub target: Option<TargetView>,
    /// Seconds per tick.
    pub dt: f32,
    pub tick: u64,
    seed: u64,
    /// Separates the random streams of conditions evaluated in one tick.
    salt: u64,
    /// This tick is one of the NPC's decision ticks ([`BehaviorDef::think`]).
    deciding: bool,
}

impl<'a> NpcCtx<'a> {
    /// The context of NPC `entity` with brain `brain`; `None` if it lacks the NPC components.
    pub fn new(world: &'a World, entity: Entity, brain: &'a NpcBrain, dt: f32) -> Option<Self> {
        let e = world.get_entity(entity).ok()?;
        let id = *e.get::<AiId>()?;
        let tf = e.get::<Transform>()?;
        let perceived = e.get::<Perceived>()?;
        let plan = e.get::<NpcPlan>()?;
        let pos = tf.translation;
        let home = e.get::<Home>();
        let target = brain.target.and_then(|t| perceived.known.iter().find(|k| k.id == t)).map(|k| TargetView {
            id: k.id,
            entity: world.get_resource::<ai_core::AiIndex>().and_then(|i| i.get(k.id)),
            pos: k.pos,
            distance: flat(k.pos - pos).length(),
            seen: k.seen,
            heard: k.heard,
            age: k.age,
        });
        Some(Self {
            world,
            entity,
            id,
            pos,
            facing: flat(*tf.forward()).try_normalize().unwrap_or(Vec3::NEG_Z),
            brain,
            perceived,
            home: home.map(|h| h.pos.unwrap_or(pos)),
            leash: home.map_or(f32::INFINITY, |h| h.leash),
            token: e.get::<AttackToken>().and_then(|t| t.held.as_ref()),
            plan,
            target,
            dt,
            tick: ai_core::sim::tick(world),
            seed: world.get_resource::<SimSeed>().map_or(0, |s| s.0),
            salt: 0,
            deciding: true,
        })
    }

    /// Whether this tick is one the NPC decides on: always, unless its behaviour has a
    /// [`BehaviorDef::think`] cadence, then once per period of its free time (the first free
    /// tick is one). A hook that decides (a choice, a roll) does it only when this holds; its
    /// per-tick work (aiming, moving) does not wait for it.
    pub fn deciding(&self) -> bool {
        self.deciding
    }

    /// The NPC's random stream for this tick (`SimRng`'s, keyed by its id), separated by
    /// `salt` and by which condition is asking.
    pub fn rng(&self, salt: u64) -> DetRng {
        DetRng::from_keys(&[self.seed, self.tick, self.id.0 as u64, salt.wrapping_add(self.salt)])
    }

    /// A component of the NPC.
    pub fn get<C: Component>(&self) -> Option<&'a C> {
        self.world.get::<C>(self.entity)
    }

    /// Health as a share of the maximum (1 without [`Health`]).
    pub fn health_fraction(&self) -> f32 {
        self.get::<Health>().map_or(1.0, |h| if h.max > 0.0 { h.current / h.max } else { 0.0 })
    }

    /// Running an action (attacking, staggered, ...).
    pub fn busy(&self) -> bool {
        self.get::<ActionState>().is_some_and(|s| s.current.is_some())
    }

    /// Flat meters from home (0 without a home).
    pub fn home_distance(&self) -> f32 {
        self.home.map_or(0.0, |h| flat(self.pos - h).length())
    }

    /// Whether `point` lies within the leash, give or take `margin` meters.
    pub fn in_leash(&self, point: Vec3, margin: f32) -> bool {
        self.home.is_none_or(|h| flat(point - h).length() <= self.leash + margin)
    }

    /// `point` pulled inside the leash (with room for the body).
    pub fn clamp_to_leash(&self, point: Vec3) -> Vec3 {
        clamp_to_leash(self.home, self.leash, point)
    }

    /// The nav agent reached its goal.
    pub fn arrived(&self) -> bool {
        self.get::<NavAgent>().is_some_and(|a| a.arrived)
    }
}

/// `point` pulled inside the circle of `leash` meters around `home`, keeping room for a
/// body.
pub fn clamp_to_leash(home: Option<Vec3>, leash: f32, point: Vec3) -> Vec3 {
    let Some(home) = home else { return point };
    let reach = (leash - LEASH_ROOM).max(0.0);
    let d = flat(point - home);
    if d.length() <= reach {
        return point;
    }
    let p = home + d.normalize_or_zero() * reach;
    Vec3::new(p.x, point.y, p.z)
}

/// How far inside its leash an NPC's goals stay (m).
pub(crate) const LEASH_ROOM: f32 = 0.5;

/// A condition: does it hold for this NPC now?
pub type NpcCondition = fn(&NpcCtx, &Params) -> bool;

/// A state: what the NPC wants this tick.
pub type NpcState = fn(&NpcCtx, &Params, &mut NpcOutput);

/// Conditions and states by name: the engine's, then the game's (a game's hook of the same
/// name replaces the engine's).
#[derive(Resource, Clone, Default)]
pub struct NpcHooks {
    pub(crate) conditions: BTreeMap<String, NpcCondition>,
    pub(crate) states: BTreeMap<String, NpcState>,
}

impl NpcHooks {
    pub fn condition_names(&self) -> Vec<&str> {
        self.conditions.keys().map(String::as_str).collect()
    }

    pub fn state_names(&self) -> Vec<&str> {
        self.states.keys().map(String::as_str).collect()
    }
}

/// Whether NPC `entity` thinks and moves this tick: a game's (or a module's) rule for
/// dormant NPCs. The default: unless the world module's active zones marked it
/// [`Dormant`] (far from every player); `NpcSystems` run after the zones are updated
/// (`WorldSystems::Zones`), so an NPC wakes the tick a player comes near.
#[derive(Resource, Clone, Copy)]
pub struct NpcActivity(pub fn(&World, Entity) -> bool);

impl Default for NpcActivity {
    fn default() -> Self {
        Self(|world, entity| world.get::<Dormant>(entity).is_none())
    }
}

/// An NPC's state changed (journaled: `e7 idle → chase e5`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct NpcStateChanged {
    pub entity: Entity,
    pub from: String,
    pub to: String,
    pub target: Option<AiId>,
    /// Set by a dev tool rather than by a transition.
    pub forced: bool,
}

/// `NpcSystems::Sense`, first: who is active this tick ([`NpcActivity`], alive); a [`Home`]
/// without a place gets the one its NPC stands on.
pub(crate) fn update_activity(world: &mut World) {
    let rule = world.get_resource::<NpcActivity>().copied().unwrap_or_default();
    let Some(mut q) = world.try_query_filtered::<(Entity, (Has<Dead>, Has<Downed>), Option<&Home>, &Transform), With<Npc>>() else {
        return;
    };
    // A downed NPC does nothing either, until it is revived.
    let found: Vec<(Entity, bool, Option<Vec3>)> = q
        .iter(world)
        .map(|(e, (dead, downed), home, tf)| (e, !dead && !downed && (rule.0)(world, e), home.filter(|h| h.pos.is_none()).map(|_| tf.translation)))
        .collect();
    for (e, active, settle) in found {
        if let Some(mut brain) = world.get_mut::<NpcBrain>(e)
            && brain.active != active
        {
            brain.active = active;
        }
        if let Some(at) = settle
            && let Some(mut home) = world.get_mut::<Home>(e)
        {
            home.pos = Some(at);
        }
    }
}

/// What deciding came to for one NPC.
struct Decided {
    entity: Entity,
    brain: NpcBrain,
    drive: Option<NpcOutput>,
    changed: Option<(String, String)>,
    /// The rest its plan starts with, drawn when its brain first runs.
    first_rest: Option<f32>,
    /// What its decision timer comes to.
    think: Think,
    /// Clamp of its plan's rest: a state's `rest_clamp` on entry, a hook's `rest_cap`.
    rest_cap: Option<f32>,
    /// Which of its gated plan options are open.
    options: Gates,
}

/// Which gated options of an NPC's plan are open this tick.
enum Gates {
    /// Untouched (an inactive NPC).
    Keep,
    /// The plan has no gated option: no component.
    Off,
    /// The names of the gated options whose `when` holds.
    Open(Vec<String>),
}

/// What an NPC's decision timer is after this tick.
#[derive(Clone, Copy)]
enum Think {
    /// Untouched (an inactive NPC).
    Keep,
    /// The behaviour has no cadence: no timer.
    Off,
    /// Seconds left, and whether this tick decides.
    Left(f32, bool),
}

/// `NpcSystems::Decide`: every active NPC, in `AiId` order, takes at most one transition and
/// runs its state's hook; the results apply after all of them decided.
pub(crate) fn decide(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    if !world.contains_resource::<NpcHooks>() {
        world.init_resource::<NpcHooks>();
    }
    let now = ai_core::sim::tick(world);
    let (decided, problems) = world.resource_scope(|world, hooks: Mut<NpcHooks>| decide_all(world, &hooks, dt));
    for (key, text) in problems {
        report_once(world, key, text);
    }
    for d in decided {
        let Ok(mut e) = world.get_entity_mut(d.entity) else { continue };
        let target = d.brain.target;
        if let Some(mut brain) = e.get_mut::<NpcBrain>()
            && *brain != d.brain
        {
            *brain = d.brain;
        }
        if let Some(rest) = d.first_rest
            && let Some(mut plan) = e.get_mut::<NpcPlan>()
        {
            plan.rest = rest;
        }
        if let Some(cap) = d.rest_cap
            && let Some(mut plan) = e.get_mut::<NpcPlan>()
            && plan.rest > cap
        {
            plan.rest = cap;
        }
        match d.options {
            Gates::Keep => {}
            Gates::Open(allowed) => match e.get_mut::<NpcOptions>() {
                Some(mut options) => {
                    if options.allowed != allowed {
                        options.allowed = allowed;
                    }
                }
                None => {
                    e.insert(NpcOptions { allowed });
                }
            },
            Gates::Off => {
                if e.contains::<NpcOptions>() {
                    e.remove::<NpcOptions>();
                }
            }
        }
        match d.think {
            Think::Keep => {}
            Think::Left(left, deciding) => match e.get_mut::<NpcThink>() {
                Some(mut think) => {
                    if think.left != left {
                        think.left = left;
                    }
                    if deciding {
                        think.decided = Some(now);
                    }
                }
                None => {
                    e.insert(NpcThink { left, decided: deciding.then_some(now) });
                }
            },
            Think::Off => {
                if e.contains::<NpcThink>() {
                    e.remove::<NpcThink>();
                }
            }
        }
        let heal = d.drive.as_ref().map_or(0.0, |o| o.heal);
        if let Some(mut drive) = e.get_mut::<NpcDrive>() {
            let held = std::mem::take(&mut drive.held);
            let next = d.drive.unwrap_or_default().into_drive(held);
            if *drive != next {
                *drive = next;
            }
        }
        if heal > 0.0
            && let Some(mut health) = e.get_mut::<Health>()
            && health.current < health.max
        {
            health.current = (health.current + heal * dt).min(health.max);
        }
        if let Some((from, to)) = d.changed {
            world.trigger(NpcStateChanged { entity: d.entity, from, to, target, forced: false });
        }
    }
}

/// Every NPC's decision, on the world as it is, and the content problems met on the way.
fn decide_all(world: &World, hooks: &NpcHooks, dt: f32) -> (Vec<Decided>, Vec<(String, String)>) {
    let mut decided = Vec::new();
    let mut problems: Vec<(String, String)> = Vec::new();
    let Some(db) = world.get_resource::<ContentDb>() else { return (decided, problems) };
    for (id, entity) in npcs_in_order(world) {
        let (Some(npc), Some(brain)) = (world.get::<Npc>(entity), world.get::<NpcBrain>(entity)) else { continue };
        if !brain.active {
            decided.push(Decided { entity, brain: brain.clone(), drive: None, changed: None, first_rest: None, think: Think::Keep, rest_cap: None, options: Gates::Keep });
            continue;
        }
        check_ids(db, id, npc, &mut problems);
        let Some(behavior) = db.get::<BehaviorDef>(&npc.behavior) else {
            decided.push(Decided { entity, brain: brain.clone(), drive: None, changed: None, first_rest: None, think: Think::Keep, rest_cap: None, options: Gates::Keep });
            continue;
        };
        decided.push(decide_one(world, entity, brain, behavior, &npc.behavior, hooks, dt, &mut problems));
    }
    (decided, problems)
}

/// Content ids an NPC names that content does not have: said once each, with what it
/// means and the closest id.
fn check_ids(db: &ContentDb, id: AiId, npc: &Npc, problems: &mut Vec<(String, String)>) {
    fn missing<T: shards_content::ContentDef>(db: &ContentDb, name: &str, means: &str) -> Option<(String, String)> {
        if db.get::<T>(name).is_some() {
            return None;
        }
        let hint = shards_content::db::closest(name, db.ids::<T>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        Some((format!("{} {name}", T::KIND), format!("{} `{name}` is not in content{hint}; {means}", T::KIND)))
    }
    let found = [
        missing::<BehaviorDef>(db, &npc.behavior, "it stands still"),
        npc.plan.as_deref().and_then(|p| missing::<AttackPlanDef>(db, p, "it never attacks")),
        Some(npc.perception.as_str())
            .filter(|p| !p.is_empty())
            .and_then(|p| missing::<PerceptionDef>(db, p, "it senses by the engine's defaults")),
        Some(npc.director.as_str())
            .filter(|d| !d.is_empty())
            .and_then(|d| missing::<DirectorDef>(db, d, "it fights by the engine's numbers")),
    ];
    for (key, text) in found.into_iter().flatten() {
        problems.push((key, format!("{id}: {text}")));
    }
}

/// Every NPC entity, by `AiId`.
pub(crate) fn npcs_in_order(world: &World) -> Vec<(AiId, Entity)> {
    let Some(mut q) = world.try_query_filtered::<(Entity, &AiId), With<Npc>>() else { return Vec::new() };
    let mut all: Vec<(AiId, Entity)> = q.iter(world).map(|(e, id)| (*id, e)).collect();
    all.sort();
    all
}

#[allow(clippy::too_many_arguments, reason = "one NPC's decision: its world, its data and where to report")]
fn decide_one(
    world: &World,
    entity: Entity,
    brain: &NpcBrain,
    behavior: &BehaviorDef,
    behavior_id: &str,
    hooks: &NpcHooks,
    dt: f32,
    problems: &mut Vec<(String, String)>,
) -> Decided {
    let mut next = brain.clone();
    next.target = world.get::<Perceived>(entity).and_then(|p| p.focus);
    let (deciding, think) = think_tick(world, entity, behavior, dt);
    let mut changed = None;
    if behavior.states.contains_key(&next.state) {
        next.since += dt;
        next.timer = (next.timer - dt).max(0.0);
        let state = &behavior.states[&next.state];
        if let Some(ctx) = NpcCtx::new(world, entity, &next, dt).map(|c| NpcCtx { deciding, ..c }) {
            let taken = state.transitions.iter().enumerate().find(|(ti, t)| {
                t.when.iter().enumerate().all(|(ci, cond)| {
                    let salt = salt::CONDITION + (*ti as u64) * 64 + ci as u64;
                    holds(hooks, &NpcCtx { salt, ..ctx }, cond, behavior_id, problems)
                })
            });
            if let Some((_, t)) = taken {
                changed = Some((next.state.clone(), t.to.clone()));
            }
        }
    } else {
        // A new NPC, or its state left content (a hot reload): start over.
        changed = Some((next.state.clone(), behavior.initial.clone()));
    }
    if let Some((_, to)) = &changed {
        enter(world, entity, &mut next, behavior, to);
    }
    let Some(state) = behavior.states.get(&next.state) else {
        return Decided { entity, brain: next, drive: None, changed: None, first_rest: None, think, rest_cap: None, options: Gates::Keep };
    };
    let kind = behavior.kind_of(&next.state);
    let mut out = NpcOutput::default();
    match hooks.states.get(kind) {
        Some(run) => {
            if let Some(ctx) = NpcCtx::new(world, entity, &next, dt).map(|c| NpcCtx { deciding, ..c }) {
                run(&ctx, &state.params, &mut out);
            }
        }
        None => problems.push((
            format!("state {kind}"),
            format!(
                "behavior `{behavior_id}` state `{}`: no state hook `{kind}`; registered: {}",
                next.state,
                hooks.state_names().join(", ")
            ),
        )),
    }
    if let Some(step) = out.step.take() {
        next.step = step;
    }
    if let Some(timer) = out.timer.take() {
        next.timer = timer.max(0.0);
    }
    if let Some(point) = out.point.take() {
        next.point = point;
    }
    // The first state of a new NPC is no change worth a journal line, but its pause is drawn.
    let first_rest = changed.as_ref().filter(|(from, _)| from.is_empty()).and_then(|_| first_rest(world, entity));
    // Entering a state with a `rest_clamp` shortens the plan's rest once.
    let entry_clamp = changed.as_ref().and(state.rest_clamp);
    let rest_cap = match (entry_clamp, out.rest_cap) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    // Like every other decision, the options' conditions are judged on the NPC's decision ticks
    // only: between two of them the open options stay as they were decided.
    let options = if deciding { option_gates(world, entity, &next, hooks, dt, deciding, behavior_id, problems) } else { Gates::Keep };
    let changed = changed.filter(|(from, _)| !from.is_empty());
    Decided { entity, brain: next, drive: Some(out), changed, first_rest, think, rest_cap, options }
}

/// Which of the NPC's gated plan options are open now: the names of those whose every `when`
/// condition holds. `Off` for a plan without gated options (or none).
#[allow(clippy::too_many_arguments, reason = "one NPC\'s decision: its world, its data and where to report")]
fn option_gates(
    world: &World,
    entity: Entity,
    brain: &NpcBrain,
    hooks: &NpcHooks,
    dt: f32,
    deciding: bool,
    behavior_id: &str,
    problems: &mut Vec<(String, String)>,
) -> Gates {
    let Some(db) = world.get_resource::<ContentDb>() else { return Gates::Off };
    let Some(plan) = world.get::<Npc>(entity).and_then(|n| n.plan.as_deref()).and_then(|p| db.get::<AttackPlanDef>(p)) else {
        return Gates::Off;
    };
    if plan.options.iter().all(|o| o.when.is_empty()) {
        return Gates::Off;
    }
    let Some(ctx) = NpcCtx::new(world, entity, brain, dt).map(|c| NpcCtx { deciding, ..c }) else { return Gates::Off };
    let open = plan
        .options
        .iter()
        .enumerate()
        .filter(|(_, o)| !o.when.is_empty())
        .filter(|(oi, o)| {
            o.when.iter().enumerate().all(|(ci, cond)| {
                let salt = salt::OPTION + (*oi as u64) * 64 + ci as u64;
                holds(hooks, &NpcCtx { salt, ..ctx }, cond, behavior_id, problems)
            })
        })
        .map(|(_, o)| o.label().to_owned())
        .collect();
    Gates::Open(open)
}

/// Whether this tick is a decision tick of NPC `entity`, and what its timer is after it:
/// with a `think` cadence the timer counts down while the NPC runs no action, a busy NPC
/// neither counts nor decides, and it starts at zero, so the first free tick decides.
fn think_tick(world: &World, entity: Entity, behavior: &BehaviorDef, dt: f32) -> (bool, Think) {
    let Some(period) = behavior.think else { return (true, Think::Off) };
    let left = world.get::<NpcThink>(entity).map_or(0.0, |t| t.left);
    if world.get::<ActionState>(entity).is_some_and(|s| s.current.is_some()) {
        return (false, Think::Left(left, false));
    }
    // Sums of tick lengths: a hair of slack, as every timer here.
    let left = left - dt;
    if left <= 1e-4 { (true, Think::Left(period, true)) } else { (false, Think::Left(left, false)) }
}

/// The pause an NPC starts with (its plan's `first_rest`), drawn from its own stream.
fn first_rest(world: &World, entity: Entity) -> Option<f32> {
    let db = world.get_resource::<ContentDb>()?;
    let plan = world.get::<Npc>(entity)?.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p))?;
    let (lo, hi) = plan.first_rest?;
    let id = world.get::<AiId>(entity).copied().unwrap_or_default();
    let seed = world.get_resource::<SimSeed>().map_or(0, |s| s.0);
    Some(DetRng::from_keys(&[seed, ai_core::sim::tick(world), id.0 as u64, salt::FIRST_REST]).range(lo, hi.max(lo)))
}

/// Enters state `to`: its clock restarts and its timer is drawn from its `duration`.
fn enter(world: &World, entity: Entity, brain: &mut NpcBrain, behavior: &BehaviorDef, to: &str) {
    brain.state = to.to_owned();
    brain.since = 0.0;
    brain.step = 0;
    brain.point = None;
    brain.done_at_entry = brain.attacks_done;
    brain.timer = match behavior.states.get(to).and_then(|s| s.duration) {
        Some((lo, hi)) => {
            let id = world.get::<AiId>(entity).copied().unwrap_or_default();
            let seed = world.get_resource::<SimSeed>().map_or(0, |s| s.0);
            let tick = ai_core::sim::tick(world);
            DetRng::from_keys(&[seed, tick, id.0 as u64, salt::ENTRY]).range(lo, hi.max(lo))
        }
        None => 0.0,
    };
}

/// Sets NPC `entity` to `state` at once (the `npc_set_state` tool), journaled as forced.
pub fn force_state(world: &mut World, entity: Entity, state: &str) -> Result<String, String> {
    let npc = world.get::<Npc>(entity).ok_or("not an NPC (it has no Npc component)")?.clone();
    let db = world.resource::<ContentDb>();
    let behavior = db
        .get::<BehaviorDef>(&npc.behavior)
        .ok_or_else(|| format!("its behavior `{}` is not in content", npc.behavior))?
        .clone();
    if !behavior.states.contains_key(state) {
        let names: Vec<&str> = behavior.states.keys().map(String::as_str).collect();
        let hint = shards_content::db::closest(state, names.iter().copied())
            .map(|c| format!(" (did you mean `{c}`?)"))
            .unwrap_or_default();
        return Err(format!("behavior `{}` has no state `{state}`{hint}; states: {}", npc.behavior, names.join(", ")));
    }
    let mut brain = world.get::<NpcBrain>(entity).cloned().unwrap_or_default();
    let from = brain.state.clone();
    enter(world, entity, &mut brain, &behavior, state);
    let target = brain.target;
    if let Some(mut slot) = world.get_mut::<NpcBrain>(entity) {
        *slot = brain;
    }
    // Entering a state by hand clamps the rest as entering it by a transition does.
    if let Some(cap) = behavior.states.get(state).and_then(|s| s.rest_clamp)
        && let Some(mut plan) = world.get_mut::<NpcPlan>(entity)
        && plan.rest > cap
    {
        plan.rest = cap;
    }
    world.trigger(NpcStateChanged { entity, from: from.clone(), to: state.to_owned(), target, forced: true });
    Ok(from)
}

/// Whether `cond` holds; an unknown name never does (and is reported once).
fn holds(hooks: &NpcHooks, ctx: &NpcCtx, cond: &Cond, behavior: &str, problems: &mut Vec<(String, String)>) -> bool {
    match hooks.conditions.get(&cond.name) {
        Some(check) => check(ctx, &cond.params) != cond.negate,
        None => {
            let names = hooks.condition_names();
            let hint = shards_content::db::closest(&cond.name, names.iter().copied())
                .map(|c| format!(" (did you mean `{c}`?)"))
                .unwrap_or_default();
            problems.push((
                format!("condition {}", cond.name),
                format!(
                    "behavior `{behavior}` state `{}`: unknown condition `{}`{hint}; it never holds. Registered: {}",
                    ctx.brain.state,
                    cond.name,
                    names.join(", ")
                ),
            ));
            false
        }
    }
}

/// Journals an engine error once per `key`.
pub(crate) fn report_once(world: &mut World, key: String, text: String) {
    let mut reported = world.get_resource_or_init::<Reported>();
    if reported.0.contains(&key) {
        return;
    }
    reported.0.push(key);
    journal::emit(world, JournalKind::Error, text);
}

#[derive(Resource, Default)]
pub(crate) struct Reported(Vec<String>);

// --- the engine's conditions ---------------------------------------------------------------

pub(crate) fn engine_conditions() -> Vec<(&'static str, NpcCondition)> {
    vec![
        ("target_known", |c, _| c.target.is_some()),
        ("target_seen", |c, _| c.target.is_some_and(|t| t.seen)),
        ("target_heard", |c, _| c.target.is_some_and(|t| t.heard)),
        ("target_lost", |c, _| c.target.is_none()),
        ("target_within", |c, p| c.target.is_some_and(|t| t.distance <= p.num("meters", 2.0))),
        ("target_in_leash", |c, p| c.target.is_some_and(|t| c.in_leash(t.pos, p.num("margin", 0.0)))),
        ("beyond_leash", |c, p| c.home.is_some() && c.home_distance() > c.leash + p.num("margin", 0.0)),
        ("at_home", |c, p| c.home_distance() <= p.num("within", 0.6)),
        ("arrived", |c, _| c.arrived()),
        ("has_token", |c, _| c.token.is_some()),
        ("attack_done", |c, _| c.brain.attacks_done > c.brain.done_at_entry),
        ("resting", |c, _| c.plan.rest > 0.0),
        ("busy", |c, _| c.busy()),
        // Sums of tick lengths: a hair of slack.
        ("timer_done", |c, _| c.brain.timer <= 1e-4),
        ("in_state", |c, p| c.brain.since + 1e-4 >= p.num("secs", 1.0)),
        ("health_below", |c, p| c.health_fraction() < p.num("fraction", 0.3)),
        ("heard_noise", |c, _| c.perceived.noise.is_some()),
        ("chance", |c, p| c.rng(0).chance(p.num("p", 0.5))),
        // A decision tick of the NPC: always, or once per `think` of its free time.
        ("deciding", |c, _| c.deciding()),
    ]
}

// --- the engine's states -------------------------------------------------------------------

pub(crate) fn engine_states() -> Vec<(&'static str, NpcState)> {
    vec![
        ("idle", idle),
        ("hold", hold),
        ("patrol", patrol),
        ("chase", chase),
        ("return_home", return_home),
        ("wait_token", wait_token),
        ("attack", attack),
        ("flee", flee),
        ("investigate", investigate),
    ]
}

/// Stand. `face_target` (flag): keep facing the target.
fn idle(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    if p.flag("face_target", false) {
        out.face = c.target.map(|t| t.pos);
    }
}

/// Stand facing the target.
fn hold(c: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    out.face = c.target.map(|t| t.pos);
    out.leashed = true;
}

/// Walk the [`Patrol`] waypoints round and round, waiting `wait` seconds at each.
fn patrol(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    let Some(points) = c.get::<Patrol>().map(|p| &p.points).filter(|p| !p.is_empty()) else { return };
    let point = points[c.brain.step as usize % points.len()];
    let within = p.num("within", 0.4);
    let wait = p.num("wait", 0.0);
    let here = flat(point - c.pos).length() <= within + 0.1;
    if c.brain.point == Some(point) {
        // Waiting there.
        if c.brain.timer <= 0.0 {
            out.step = Some(c.brain.step + 1);
            out.point = Some(None);
        }
        return;
    }
    if here {
        if wait > 0.0 {
            out.point = Some(Some(point));
            out.timer = Some(wait);
        } else {
            out.step = Some(c.brain.step + 1);
        }
        return;
    }
    out.goal = Some(point);
    out.within = within;
    out.speed = p.num("speed", 0.5);
}

/// Run at the target (where it was last seen or heard), up to `within` meters, never out of
/// the leash.
fn chase(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    let Some(t) = c.target else { return };
    let goal = c.clamp_to_leash(t.pos);
    out.goal = Some(goal);
    // A target beyond the leash: as far as the leash goes.
    out.within = if goal == t.pos { p.num("within", 1.5) } else { 0.3 };
    out.speed = p.num("speed", 1.0);
    out.leashed = true;
    if t.distance <= out.within + 1.0 {
        out.face = Some(t.pos);
    }
}

/// Go home, regaining `heal` health per second on the way.
fn return_home(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    out.goal = Some(c.home.unwrap_or(c.pos));
    out.within = p.num("within", 0.3);
    out.speed = p.num("speed", 1.0);
    out.heal = p.num("heal", 0.0);
}

/// Wait for an attack token in the director's waiting role around the target.
fn wait_token(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    let Some(t) = c.target else { return };
    out.face = Some(t.pos);
    out.request_token = true;
    out.waiting = true;
    out.leashed = true;
    out.speed = p.num("speed", 0.5);
}

/// Fight: with a token, the planner chooses an attack, closes in and presses it; without
/// one, ask for a token and take the waiting role meanwhile.
fn attack(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    let Some(t) = c.target else { return };
    out.face = Some(t.pos);
    out.request_token = true;
    out.strike = true;
    out.waiting = true;
    out.leashed = true;
    out.speed = p.num("speed", 1.0);
}

/// Run `distance` meters away from the target.
fn flee(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    let Some(t) = c.target else { return };
    let away = flat(c.pos - t.pos).try_normalize().unwrap_or(-c.facing);
    out.goal = Some(c.clamp_to_leash(c.pos + away * p.num("distance", 6.0)));
    out.within = 0.5;
    out.speed = p.num("speed", 1.0);
    out.leashed = true;
}

/// Walk to what it heard (or its kept point) and look.
fn investigate(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    let Some(point) = c.brain.point.or(c.perceived.noise.map(|n| n.pos)) else { return };
    if c.brain.point.is_none() {
        out.point = Some(Some(point));
    }
    out.goal = Some(c.clamp_to_leash(point));
    out.within = p.num("within", 1.0);
    out.speed = p.num("speed", 0.5);
    out.leashed = true;
}
