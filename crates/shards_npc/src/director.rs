//! The combat director: who may attack, and what the others do meanwhile.
//!
//! - **Attack tokens.** Only an NPC holding a token starts an attack (the engine's `attack`
//!   state presses only then), and it holds the token until that attack is over (or
//!   interrupted). Tokens per director are limited by the number of living players
//!   ([`DirectorDef::tokens`]), so at most that many of its NPCs attack at once. NPCs that
//!   ask are served longest-waiting first, then in `AiId` order; a token comes back when its
//!   attack ends, when it was not used within `patience`, or when its holder dies, falls
//!   dormant, loses its target or leaves the fight.
//! - **Collision budget and allowed combinations** are checked by the planner when it
//!   presses: the attacks in flight on one target may cost at most `budget` together, and
//!   only tag pairs in `allowed` may overlap. A token holder whose attack does not fit
//!   waits (and gives the token back after `patience`).
//! - **Waiting roles.** NPCs without a token that wait (the `wait_token` and `attack` states)
//!   get a slot around their target in the order they start waiting; slot `n` takes role `n
//!   % roles` ([`DirectorDef::waiting`]): circle, strafe or hold at a distance. The engine sets
//!   the steering target; how it looks is the game's business.
//!
//! Everything runs in `AiId` order on components of the NPCs themselves, so the director has
//! no state of its own: a replay or a snapshot carries it whole.

use ai_core::{AiId, AiIndex, SimRng, SimTick};
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use shards_action::{ActionDef, ActionState, Charges, Cooldowns, Dead, Fighter, InputBuffer, Stamina};
use shards_content::ContentDb;
use shards_core::{flat, PlayerId};
use shards_motion::Collider;

use crate::brain::{clamp_to_leash, salt, Home, Npc, NpcBrain, NpcDrive};
use crate::def::{AttackPlanDef, DirectorDef, Role, WaitRole};
use crate::perception::Perceived;
use crate::planner::{feasible, resolve, InFlight, NpcPlan};

/// An NPC's attack token, while it holds one, and its place in the queue while it asks.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct AttackToken {
    pub held: Option<Grant>,
    /// The tick it started asking: the longest-waiting is served first.
    pub asking_since: Option<u64>,
}

/// A token held: on whom, since when, and the attack it is for once the planner chose one.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Grant {
    pub target: AiId,
    /// The director it came from.
    pub director: String,
    /// Tick it was granted.
    pub since: u64,
    /// Seconds held.
    pub age: f32,
    /// The plan option chosen (its name), its input, cost, tags and plan cooldown.
    pub attack: Option<String>,
    pub input: Option<String>,
    pub cost: f32,
    pub tags: Vec<String>,
    pub cooldown: f32,
    /// Tick the attack was pressed.
    pub pressed: Option<u64>,
    /// The action instance the press starts (`ActionState::started` when pressed).
    pub instance: u32,
    /// The attack started.
    pub started: bool,
}

impl Grant {
    #[cfg(test)]
    pub(crate) fn for_tests(target: AiId) -> Self {
        Self::new(target, "", 0)
    }

    fn new(target: AiId, director: &str, tick: u64) -> Self {
        Self {
            target,
            director: director.to_owned(),
            since: tick,
            age: 0.0,
            attack: None,
            input: None,
            cost: 0.0,
            tags: Vec::new(),
            cooldown: 0.0,
            pressed: None,
            instance: 0,
            started: false,
        }
    }

    /// Pressed or running: it counts against the target's budget and combinations.
    pub fn in_flight(&self) -> bool {
        self.pressed.is_some()
    }
}

/// A token was granted (journaled: `e7 takes an attack token on e5 (2/2)`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct TokenGranted {
    pub entity: Entity,
    pub target: AiId,
    /// Tokens of its director held now, and how many there are.
    pub held: u32,
    pub limit: u32,
}

/// A token came back (journaled: `e7 returns its attack token after slash (1/2)`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct TokenReturned {
    pub entity: Entity,
    pub target: AiId,
    /// The attack it was used for; `None`: unused.
    pub attack: Option<String>,
    pub used: bool,
    /// Why it came back.
    pub reason: String,
    pub held: u32,
    pub limit: u32,
}

/// What the director and the planner read and change of a fighting NPC.
#[derive(QueryData)]
#[query_data(mutable)]
pub(crate) struct Fighting {
    pub entity: Entity,
    pub id: &'static AiId,
    pub npc: &'static Npc,
    pub brain: &'static mut NpcBrain,
    pub drive: &'static mut NpcDrive,
    pub token: &'static mut AttackToken,
    pub plan: &'static mut NpcPlan,
    pub fighter: &'static Fighter,
    pub state: &'static ActionState,
    pub buffer: &'static InputBuffer,
    pub transform: &'static Transform,
    pub perceived: &'static Perceived,
    pub home: Option<&'static Home>,
    pub stamina: Option<&'static Stamina>,
    pub cooldowns: Option<&'static Cooldowns>,
    pub charges: Option<&'static Charges>,
    pub dead: Has<Dead>,
}

/// Where a target stands, its body radius, whether it is alive.
pub(crate) type Targets<'w, 's> = Query<'w, 's, (&'static Transform, Has<Dead>, Option<&'static Collider>)>;

/// A live target as an NPC knows it: where it perceives it (seen this tick: where it is;
/// else where it was last seen or heard) and its body radius. NPCs do not cheat: what they
/// steer and strike at is what they perceive.
pub(crate) fn target_of(index: &AiIndex, targets: &Targets, perceived: &Perceived, id: AiId) -> Option<(Vec3, f32)> {
    let (tf, dead, collider) = targets.get(index.get(id)?).ok()?;
    let at = perceived.get(id).map_or(tf.translation, |k| k.pos);
    (!dead).then(|| (at, collider.map_or(0.4, |c| c.shape.half_extents().x)))
}

/// NPCs by `AiId`.
pub(crate) fn order(npcs: &Query<Fighting>) -> Vec<(AiId, Entity)> {
    let mut all: Vec<(AiId, Entity)> = npcs.iter().map(|f| (*f.id, f.entity)).collect();
    all.sort();
    all
}

/// Whether the attack this grant pressed is running: an action with hits, started by the
/// press or after it (a combo the planner chained).
pub(crate) fn running_attack(db: &ContentDb, state: &ActionState, grant: &Grant) -> bool {
    state.current.as_ref().is_some_and(|r| {
        r.instance >= grant.instance && db.get::<ActionDef>(&r.action).is_some_and(|d| !d.hits.is_empty())
    })
}

/// `NpcSystems::Direct`: bookkeeping of held tokens (started attacks are remembered, finished
/// ones return their token), then grants to those who ask, then waiting roles.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub(crate) fn direct(
    mut commands: Commands,
    time: Res<Time>,
    tick: Res<SimTick>,
    rng: SimRng,
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    players: Query<(), (With<PlayerId>, Without<Dead>)>,
    targets: Targets,
    mut npcs: Query<Fighting>,
) {
    let dt = time.delta_secs();
    let players = players.iter().count() as u32;
    let order = order(&npcs);
    let limit_of = |director: &str| DirectorDef::of(&db, director).limit(players);

    // 1. Held tokens: count down, notice started attacks, return what is done.
    let mut returned = Vec::new();
    for &(id, entity) in &order {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let plan_def = f.npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p));
        f.plan.tick(dt);
        let Some(mut grant) = f.token.held.clone() else { continue };
        grant.age += dt;
        let running = running_attack(&db, f.state, &grant);
        if grant.in_flight() && !grant.started && running {
            grant.started = true;
            if let Some(name) = &grant.attack {
                let keep = plan_def.map_or(1, AttackPlanDef::memory);
                f.plan.remember(name, &grant.tags, grant.cooldown, keep);
            }
        }
        let patience = DirectorDef::of(&db, &f.npc.director).patience;
        // An attack in flight keeps its token to its end, whatever else happens: the count of
        // attackers never runs ahead of the tokens.
        let reason = if f.dead {
            Some("dead")
        } else if grant.in_flight() {
            let buffered = grant.input.as_ref().is_some_and(|i| f.buffer.queued.iter().any(|q| q.input == *i));
            (!running && !buffered).then_some(if grant.started { "attack over" } else { "attack never started" })
        } else if !f.brain.active {
            Some("dormant")
        } else if target_of(&index, &targets, f.perceived, grant.target).is_none() {
            Some("target gone")
        } else if !f.drive.strike && !f.drive.request_token {
            Some("left the fight")
        } else if grant.age > patience + 1e-4 {
            Some("unused")
        } else {
            None
        };
        match reason {
            Some(reason) => {
                f.token.held = None;
                f.token.asking_since = None;
                let rest = plan_def.map_or((0.0, 0.0), |p| p.rest);
                if grant.started {
                    f.brain.attacks_done += 1;
                    f.plan.rest = rng.stream(id, salt::REST).range(rest.0, rest.1.max(rest.0));
                } else {
                    // Back of the queue, after a short breath.
                    f.plan.rest = f.plan.rest.max(rest.0);
                }
                returned.push((entity, grant, reason));
            }
            None => f.token.held = Some(grant),
        }
    }

    // 2. Tokens held per director.
    let mut held = Pools::default();
    for &(_, entity) in &order {
        if let Ok(f) = npcs.get(entity)
            && f.token.held.is_some()
        {
            held.add(&pool(&f.npc.director));
        }
    }
    for (entity, grant, reason) in returned {
        let d = pool(&grant.director);
        let (held, limit) = (held.get(&d), limit_of(&d));
        let used = grant.started;
        commands.trigger(TokenReturned {
            entity,
            target: grant.target,
            attack: grant.attack.filter(|_| used),
            used,
            reason: reason.to_owned(),
            held,
            limit,
        });
    }

    // 3. Grants, longest-waiting first.
    let inflight = InFlight::collect(npcs.iter().map(|f| (*f.id, f.token.held.as_ref())));
    let mut asking: Vec<(u64, AiId, Entity)> = Vec::new();
    for &(id, entity) in &order {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let wants = f.drive.request_token
            && f.token.held.is_none()
            && f.brain.active
            && !f.dead
            && f.plan.rest <= 0.0
            && f.npc.plan.is_some()
            && f.brain.target.is_some_and(|t| target_of(&index, &targets, f.perceived, t).is_some());
        if !wants {
            if f.token.asking_since.is_some() && f.token.held.is_none() {
                f.token.asking_since = None;
            }
            continue;
        }
        let since = *f.token.asking_since.get_or_insert(tick.0);
        asking.push((since, id, entity));
    }
    asking.sort();
    for (_, id, entity) in asking {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let d = pool(&f.npc.director);
        let limit = limit_of(&d);
        if held.get(&d) >= limit {
            continue;
        }
        let Some(target) = f.brain.target else { continue };
        let Some((_, radius)) = target_of(&index, &targets, f.perceived, target) else { continue };
        let director = DirectorDef::of(&db, &f.npc.director);
        let Some(plan_def) = f.npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)) else { continue };
        let options = resolve(plan_def, &db, &f.fighter.moveset, radius);
        let view = shards_action::FighterView {
            db: &db,
            moveset: &f.fighter.moveset,
            state: f.state,
            stamina: f.stamina,
            cooldowns: f.cooldowns,
            charges: f.charges,
            held: &f.drive.held,
            dead: f.dead,
        };
        let (budget, overlapping) = inflight.on(target, id, &director);
        let any = options.iter().any(|o| feasible(o, plan_def, &f.plan, &view, dt, budget, &overlapping, &director));
        if !any {
            continue;
        }
        f.token.held = Some(Grant::new(target, &d, tick.0));
        f.token.asking_since = None;
        held.add(&d);
        commands.trigger(TokenGranted { entity, target, held: held.get(&d), limit });
    }

    // 4. Waiting roles: slots around each target in the order NPCs start waiting.
    let mut taken: Vec<(AiId, u32)> = Vec::new();
    let mut waiters: Vec<(AiId, Entity)> = Vec::new();
    for &(id, entity) in &order {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let waiting = f.drive.waiting && f.token.held.is_none() && f.brain.active && !f.dead;
        let target = f.brain.target.filter(|t| waiting && target_of(&index, &targets, f.perceived, *t).is_some());
        let Some(target) = target else {
            if f.brain.slot.is_some() || f.brain.role.is_some() {
                f.brain.slot = None;
                f.brain.role = None;
            }
            continue;
        };
        match f.brain.slot {
            Some(slot) if !taken.contains(&(target, slot)) => taken.push((target, slot)),
            _ => {
                f.brain.slot = None;
                f.brain.role = None;
            }
        }
        waiters.push((id, entity));
    }
    for (_, entity) in waiters {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let Some(target) = f.brain.target else { continue };
        let Some((center, _)) = target_of(&index, &targets, f.perceived, target) else { continue };
        let slot = match f.brain.slot {
            Some(slot) => slot,
            None => {
                let free = (0..).find(|s| !taken.contains(&(target, *s))).unwrap_or(0);
                taken.push((target, free));
                f.brain.slot = Some(free);
                free
            }
        };
        let director = DirectorDef::of(&db, &f.npc.director);
        let role = director.waiting.get(slot as usize % director.waiting.len().max(1)).cloned().unwrap_or_default();
        let home = f.home.map(|h| (h.pos.unwrap_or(f.transform.translation), h.leash));
        wait_in_role(&mut f, &role, center, home, dt);
    }
}

/// The director an NPC fights by: its own, or `default`.
fn pool(director: &str) -> String {
    if director.is_empty() { DirectorDef::DEFAULT.to_owned() } else { director.to_owned() }
}

/// Tokens held per director, in the order directors first come up.
#[derive(Default)]
struct Pools(Vec<(String, u32)>);

impl Pools {
    fn get(&self, director: &str) -> u32 {
        self.0.iter().find(|(d, _)| d == director).map_or(0, |(_, n)| *n)
    }

    fn add(&mut self, director: &str) {
        match self.0.iter_mut().find(|(d, _)| d == director) {
            Some((_, n)) => *n += 1,
            None => self.0.push((director.to_owned(), 1)),
        }
    }
}

/// Steers a waiter by its role around a target at `center`.
fn wait_in_role(f: &mut FightingItem, role: &WaitRole, center: Vec3, home: Option<(Vec3, f32)>, dt: f32) {
    let here = f.transform.translation;
    let bearing = flat(here - center);
    let distance = role.distance.max(0.5);
    let fresh = f.brain.role.as_deref() != Some(role.role.name());
    if fresh {
        f.brain.role = Some(role.role.name().to_owned());
        f.brain.orbit = bearing.z.atan2(bearing.x);
    }
    let around = |angle: f32| center + Vec3::new(angle.cos(), 0.0, angle.sin()) * distance;
    let (goal, within) = match role.role {
        Role::Hold => {
            let out = bearing.try_normalize().unwrap_or(Vec3::Z);
            (if bearing.length() > distance + 0.25 { Some(center + out * distance) } else { None }, 0.25)
        }
        Role::Circle => {
            if !fresh {
                f.brain.orbit = wrap(f.brain.orbit + role.orbit / distance * dt);
            }
            (Some(around(f.brain.orbit)), 0.3)
        }
        Role::Strafe => {
            let turn = (f.brain.since / role.period.max(0.1)) as i64 % 2 == 0;
            if !fresh {
                let way = if turn { 1.0 } else { -1.0 };
                f.brain.orbit = wrap(f.brain.orbit + way * role.orbit / distance * dt);
            }
            (Some(around(f.brain.orbit)), 0.3)
        }
    };
    let goal = goal.map(|g| if f.drive.leashed { clamp_to_leash(home.map(|h| h.0), home.map_or(0.0, |h| h.1), g) } else { g });
    f.drive.goal = goal;
    f.drive.within = within;
    f.drive.speed = role.speed;
    f.drive.face = Some(center);
}

fn wrap(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}
