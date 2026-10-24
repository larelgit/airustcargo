//! The combat director: who may attack, and what the others do meanwhile.
//!
//! - **Attack tokens.** Only an NPC holding a token starts an attack (the engine's `attack`
//!   state presses only then), and it holds the token until that attack is over (or
//!   interrupted). Tokens per director are limited by the number of living players
//!   ([`DirectorDef::tokens`]), so at most that many of its NPCs attack at once. NPCs that
//!   ask are served longest-waiting first, then in `AiId` order; a token comes back when its
//!   attack ends, when it was not used within `patience`, or when its holder dies, falls
//!   dormant, loses its target or leaves the fight; when a player dies or leaves, tokens
//!   beyond the lowered limit come back at once (the newest first) unless their attack is
//!   under way.
//! - **Collision budget and allowed combinations** are checked by the planner when it
//!   presses: the attacks in flight on one target may cost at most `budget` together, and
//!   only tag pairs in `allowed` may overlap. A token holder whose attack does not fit
//!   waits (and gives the token back after `patience`).
//! - **Waiting roles.** NPCs without a token that wait (the `wait_token` and `attack` states)
//!   get a slot around their target in the order they start waiting; slot `n` takes role `n
//!   % roles` ([`DirectorDef::waiting`]): circle, strafe or hold at a distance. The engine sets
//!   the steering target; how it looks is the game's business.
//! - **Tokenless NPCs.** An NPC whose plan is `tokenless` holds a grant that is outside all of this: not
//!   counted, limited, lowered or timed out, no token events, and its own choice and press skip the
//!   target's budget and combinations ([`AttackPlanDef::tokenless`]). A plan with its own `wait` role takes no
//!   slot and waits in that role, with or without a grant ([`AttackPlanDef::wait`]).
//!
//! Everything runs in `AiId` order on components of the NPCs themselves, so the director has
//! no state of its own: a replay or a snapshot carries it whole.

use ai_core::{AiId, AiIndex, SimRng, SimTick};
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use shards_action::{ActionState, Charges, Cooldowns, Dead, Downed, Fighter, FighterView, InputBuffer, LeapSite, Stamina};
use shards_content::ContentDb;
use shards_core::{flat, Intent};
use shards_motion::{CharacterController, CharacterParams, Collider, MotionState, PhysicsWorld};
use shards_rules::{ActiveRules, RuleClass, RulesView};

use crate::brain::{clamp_to_leash, salt, Home, Npc, NpcBrain, NpcDrive, NpcThink};
use crate::def::{AttackPlanDef, DirectorDef, Role, WaitRole};
use crate::perception::Perceived;
use crate::chain::NpcChain;
use crate::planner::{feasible, resolve, InFlight, NpcOptions, NpcPlan};
use crate::ranged::{keep_band, NpcFireLine};
use crate::schedule::{grant_notes, inputs, living_players, FightCx, LiveZones, Players};
use crate::sequence::SequenceHold;
use crate::strike::{pool, NpcStrike};

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
    pub strike: Option<&'static NpcStrike>,
    pub options: Option<&'static NpcOptions>,
    pub chain: Option<&'static mut NpcChain>,
    pub think: Option<&'static NpcThink>,
    pub fighter: &'static Fighter,
    pub state: &'static ActionState,
    pub buffer: &'static mut InputBuffer,
    pub transform: &'static Transform,
    pub perceived: &'static Perceived,
    pub home: Option<&'static Home>,
    pub stamina: Option<&'static Stamina>,
    pub cooldowns: Option<&'static Cooldowns>,
    pub charges: Option<&'static Charges>,
    /// How it moves now (the mode gate) and what it moves by (a leap's landing).
    pub motion: Option<&'static MotionState>,
    pub body: Option<(&'static CharacterController, Option<&'static CharacterParams>)>,
    pub dead: Has<Dead>,
    /// Orders in it: an agent took the NPC over (`move_to`, `attack`), it is out of the fight.
    pub intent: Option<&'static Intent>,
    /// A sequence holds it (F12): its brain is dormant, it is out of the fight like a body under orders.
    pub held: Has<SequenceHold>,
    /// Its rule class: what the rules make its actions cost ([`npc_rules`]).
    pub class: Option<&'static RuleClass>,
    /// Its line of fire, traced this tick: only an NPC whose plan keeps a range has one (F11).
    pub line: Option<&'static NpcFireLine>,
}

/// The rules in play as an NPC of `class` sees them (`shards_rules`), for its view ([`npc_view`]): an NPC
/// is never a player's body, so without a class it has none.
pub(crate) fn npc_rules<'a>(db: &'a ContentDb, rules: Option<&ActiveRules>, class: Option<&'a RuleClass>) -> Option<RulesView<'a>> {
    RulesView::of(db, rules, class, false)
}

impl FightingItem<'_, '_> {
    /// An agent's orders drive it now, or a sequence holds it (F12): it neither asks for tokens nor presses.
    pub(crate) fn under_orders(&self) -> bool {
        self.held || self.intent.is_some_and(|i| !i.orders.is_empty())
    }
}

/// What `can_start` reads about a fighting NPC: its action state and resources, how it moves now,
/// for the landing a leap needs, `site`, and the rules it fights under ([`npc_rules`]).
#[allow(clippy::too_many_arguments, reason = "the fields of one fighter, passed apart for the borrows")]
pub(crate) fn npc_view<'a>(
    db: &'a ContentDb,
    fighter: &'a Fighter,
    state: &'a ActionState,
    held: &'a [String],
    dead: bool,
    resources: (Option<&'a Stamina>, Option<&'a Cooldowns>, Option<&'a Charges>),
    motion: Option<&MotionState>,
    site: Option<LeapSite<'a>>,
    rules: Option<RulesView<'a>>,
) -> FighterView<'a> {
    FighterView {
        stamina: resources.0,
        cooldowns: resources.1,
        charges: resources.2,
        mode: motion.map(|m| m.mode),
        off_ground: shards_action::off_ground(motion),
        leap: site,
        ..FighterView::new(db, &fighter.moveset, state, held, dead, rules)
    }
}

/// Where an NPC stands for a leap that aims at `target` (where it perceives it, and its radius): `None`
/// without a body or an `Intent` (nothing to judge a landing by).
pub(crate) fn npc_site<'a>(
    physics: &'a PhysicsWorld,
    db: &ContentDb,
    entity: Entity,
    transform: &Transform,
    intent: Option<&'a Intent>,
    body: Option<(&CharacterController, Option<&CharacterParams>)>,
    target: Option<(Vec3, f32)>,
) -> Option<LeapSite<'a>> {
    let (controller, own) = body?;
    Some(LeapSite::body(physics, entity, (transform.translation, *transform.forward()), controller.resolve(own, Some(db)), intent?, target))
}

/// Where a target stands, its body radius, whether it is alive, whether it is down.
pub(crate) type Targets<'w, 's> = Query<'w, 's, (&'static Transform, Has<Dead>, Option<&'static Collider>, Has<Downed>)>;

/// A live target as an NPC knows it: where it perceives it (seen this tick: where it is;
/// else where it was last seen or heard) and its body radius; `None` once it is dead, down,
/// gone or forgotten. NPCs do not cheat: what they steer and strike at is what they perceive.
pub(crate) fn target_of(index: &AiIndex, targets: &Targets, perceived: &Perceived, id: AiId) -> Option<(Vec3, f32)> {
    let known = perceived.get(id)?;
    let (_, dead, collider, downed) = targets.get(index.get(id)?).ok()?;
    (!dead && !downed).then(|| (known.pos, collider.map_or(0.4, |c| c.shape.half_extents().x)))
}

/// NPCs by `AiId`.
pub(crate) fn order(npcs: &Query<Fighting>) -> Vec<(AiId, Entity)> {
    let mut all: Vec<(AiId, Entity)> = npcs.iter().map(|f| (*f.id, f.entity)).collect();
    all.sort();
    all
}

/// Whether `npc` fights without the director's tokens: its plan is `tokenless`.
pub(crate) fn tokenless(db: &ContentDb, npc: &Npc) -> bool {
    npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)).is_some_and(|p| p.tokenless)
}

/// Whether the attack this grant pressed is running: an action started by an input (not one
/// the engine forced: a stagger, a flinch, a death) at the press or after it. Until it is
/// seen to start, only the pressed input's action counts; once started, what follows it by
/// input (a combo) is the same attack. Whether the action has hits does not matter: a cast
/// that throws a projectile from an effect is an attack too.
pub(crate) fn running_attack(state: &ActionState, grant: &Grant) -> bool {
    state.current.as_ref().is_some_and(|r| {
        !r.forced && r.instance >= grant.instance && (grant.started || r.input.is_some() && r.input == grant.input)
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
    physics: Res<PhysicsWorld>,
    players: Players,
    targets: Targets,
    rules: Option<Res<ActiveRules>>,
    live: LiveZones,
    mut npcs: Query<Fighting>,
) {
    let dt = time.delta_secs();
    let players = living_players(&players);
    let order = order(&npcs);
    let limit_of = |director: &str| DirectorDef::of(&db, director).limit(players);

    // 1. Held tokens: count down, notice started attacks, return what is done.
    let mut returned = Vec::new();
    for &(id, entity) in &order {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let plan_def = f.npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p));
        f.plan.tick(dt, f.state.current.is_none());
        // A director that no longer spaces its strikes (a hot reload) leaves no notes behind.
        if f.strike.is_some() && DirectorDef::of(&db, &f.npc.director).min_spacing <= 0.0 {
            commands.entity(entity).remove::<NpcStrike>();
        }
        let Some(mut grant) = f.token.held.clone() else { continue };
        grant.age += dt;
        let running = running_attack(f.state, &grant);
        if grant.in_flight() && !grant.started && running {
            grant.started = true;
            if let Some(name) = &grant.attack {
                let keep = plan_def.map_or(1, AttackPlanDef::memory);
                f.plan.remember(name, &grant.tags, grant.cooldown, keep);
            }
        }
        let patience = DirectorDef::of(&db, &f.npc.director).patience;
        // A tokenless grant is outside the director's books: nobody waits for it, so patience is pointless.
        let tokenless = plan_def.is_some_and(|p| p.tokenless);
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
        } else if f.under_orders() {
            Some("under orders")
        } else if !f.drive.strike && !f.drive.request_token {
            Some("left the fight")
        } else if !tokenless && grant.age > patience + 1e-4 {
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
                    // A forced action in the attack's place (a stagger) cut it short.
                    let interrupted = f.state.current.as_ref().is_some_and(|r| r.forced);
                    f.plan.rest = match plan_def.and_then(|p| p.rest_interrupted).filter(|_| interrupted) {
                        Some((lo, hi)) => rng.stream(id, salt::REST_INTERRUPTED).range(lo, hi.max(lo)),
                        None => rng.stream(id, salt::REST).range(rest.0, rest.1.max(rest.0)),
                    };
                } else {
                    // Back of the queue, after a short breath.
                    f.plan.rest = f.plan.rest.max(rest.0);
                }
                returned.push((entity, grant, reason, !tokenless));
            }
            None => f.token.held = Some(grant),
        }
    }

    // 1b. Fewer living players than when the tokens went out: those beyond the limit come
    // back, the newest grants first, unless their attack is in flight (it keeps its token to
    // its end, and the count falls as those end).
    let mut spare: Vec<(String, u64, AiId, Entity)> = Vec::new();
    let mut out = Pools::default();
    for &(id, entity) in &order {
        if let Ok(f) = npcs.get(entity)
            && let Some(grant) = &f.token.held
            && !tokenless(&db, f.npc)
        {
            out.add(&grant.director);
            if !grant.in_flight() {
                spare.push((grant.director.clone(), grant.since, id, entity));
            }
        }
    }
    spare.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(b.2.cmp(&a.2)));
    for (director, _, _, entity) in spare {
        if out.get(&director) <= limit_of(&director) {
            continue;
        }
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let Some(grant) = f.token.held.take() else { continue };
        f.token.asking_since = None;
        let rest = f.npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)).map_or(0.0, |p| p.rest.0);
        f.plan.rest = f.plan.rest.max(rest);
        out.remove(&director);
        returned.push((entity, grant, "limit lowered", true));
    }

    // 2. Tokens held per director (a tokenless grant is not one).
    let mut held = Pools::default();
    for &(_, entity) in &order {
        if let Ok(f) = npcs.get(entity)
            && f.token.held.is_some()
            && !tokenless(&db, f.npc)
        {
            held.add(&pool(&f.npc.director));
        }
    }
    for (entity, grant, reason, counted) in returned {
        if !counted {
            continue;
        }
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
    let notes = grant_notes(npcs.iter(), tick.0);
    let mut asking: Vec<(u64, AiId, Entity)> = Vec::new();
    for &(id, entity) in &order {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let wants = f.drive.request_token
            && !f.under_orders()
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
        // With `press_on_decision` the token, like the choice and the press, is taken on the NPC's
        // decision tick only; it keeps its place in the queue between decisions, and no token
        // sits idle until the next one.
        let on_decision = f.npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)).is_none_or(|p| !p.press_on_decision)
            || f.think.is_none_or(|t| t.decided == Some(tick.0));
        if on_decision {
            asking.push((since, id, entity));
        }
    }
    asking.sort();
    let cx = FightCx { db: &db, physics: &physics, tick: tick.0, dt, players, rules: rules.as_deref(), live: Some(&live) };
    for (_, _, entity) in asking {
        // Judged on the NPC's read-only view, by the rules the planner chooses by: some option
        // must pass, distance aside (no token for a strike that would come too soon after
        // another NPC's, among the rest).
        // A tokenless NPC is not held to the director's limit and takes none of its tokens.
        let (target, d, limit, free) = {
            let Ok(f) = npcs.get(entity) else { continue };
            let d = pool(&f.npc.director);
            let limit = limit_of(&d);
            let free = tokenless(&db, f.npc);
            if !free && held.get(&d) >= limit {
                continue;
            }
            let Some(target) = f.brain.target else { continue };
            let Some((there, radius)) = target_of(&index, &targets, f.perceived, target) else { continue };
            let Some(inputs) = inputs(&f, &cx, (target, there, radius), &notes, &inflight) else { continue };
            let options = resolve(inputs.plan_def, &db, &f.fighter.moveset, radius);
            if !options.iter().any(|o| feasible(o, &inputs)) {
                continue;
            }
            (target, d, limit, free)
        };
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        f.token.held = Some(Grant::new(target, &d, tick.0));
        f.token.asking_since = None;
        if !free {
            held.add(&d);
            commands.trigger(TokenGranted { entity, target, held: held.get(&d), limit });
        }
    }

    // 4. Waiting roles: slots around each target in the order NPCs start waiting. An NPC whose plan
    // names its own `wait` role takes no slot and reserves none: it waits in that role, and keeps the
    // role and its orbit while it holds a grant with nothing chosen (the planner steers it then).
    let mut taken: Vec<(AiId, u32)> = Vec::new();
    let mut waiters: Vec<(AiId, Entity)> = Vec::new();
    for &(id, entity) in &order {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        // A plan that keeps a range (F11) takes no slot either: it keeps its band.
        let own = plan_wait(&db, f.npc).is_some() || plan_band(&db, f.npc).is_some();
        let live = f.brain.active && !f.dead && !f.under_orders();
        let waiting = f.drive.waiting && f.token.held.is_none() && live;
        let target = f.brain.target.filter(|t| waiting && target_of(&index, &targets, f.perceived, *t).is_some());
        let Some(target) = target else {
            if own {
                // Waiting or holding a grant with a target it knows: the role and the orbit stay, so the
                // next wait advances the orbit instead of seeding it again.
                let engaged = live
                    && (f.drive.waiting || f.token.held.is_some())
                    && f.brain.target.is_some_and(|t| target_of(&index, &targets, f.perceived, t).is_some());
                if f.brain.slot.is_some() {
                    f.brain.slot = None;
                }
                if !engaged && f.brain.role.is_some() {
                    f.brain.role = None;
                }
            } else if f.brain.slot.is_some() || f.brain.role.is_some() {
                f.brain.slot = None;
                f.brain.role = None;
            }
            continue;
        };
        if own {
            if f.brain.slot.is_some() {
                f.brain.slot = None;
            }
            waiters.push((id, entity));
            continue;
        }
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
        if let Some(band) = plan_band(&db, f.npc) {
            let home = f.home.map(|h| (h.pos.unwrap_or(f.transform.translation), h.leash));
            let role = plan_wait(&db, f.npc);
            keep_band(&mut f, band, role.as_ref(), center, home, dt);
            continue;
        }
        let role = match plan_wait(&db, f.npc) {
            Some(role) => role,
            None => {
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
                director.waiting.get(slot as usize % director.waiting.len().max(1)).cloned().unwrap_or_default()
            }
        };
        let home = f.home.map(|h| (h.pos.unwrap_or(f.transform.translation), h.leash));
        wait_in_role(&mut f, &role, center, home, dt);
    }
}

/// The waiting role an NPC's plan names for itself (`AttackPlanDef::wait`), if it does.
fn plan_wait(db: &ContentDb, npc: &Npc) -> Option<WaitRole> {
    npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)).and_then(|p| p.wait.clone())
}

/// The band an NPC's plan keeps (`AttackPlanDef::keep_range`, F11), if it does.
fn plan_band(db: &ContentDb, npc: &Npc) -> Option<(f32, f32)> {
    npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)).and_then(|p| p.keep_range)
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

    fn remove(&mut self, director: &str) {
        if let Some((_, n)) = self.0.iter_mut().find(|(d, _)| d == director) {
            *n = n.saturating_sub(1);
        }
    }
}

/// Steers a waiter by its role around a target at `center`.
pub(crate) fn wait_in_role(f: &mut FightingItem, role: &WaitRole, center: Vec3, home: Option<(Vec3, f32)>, dt: f32) {
    let here = f.transform.translation;
    let bearing = flat(here - center);
    let distance = role.distance.max(0.5);
    let fresh = f.brain.role.as_deref() != Some(role.role.name());
    if fresh {
        f.brain.role = Some(role.role.name().to_owned());
        f.brain.orbit = ops::atan2(bearing.z, bearing.x);
    }
    let around = |angle: f32| center + Vec3::new(ops::cos(angle), 0.0, ops::sin(angle)) * distance;
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
