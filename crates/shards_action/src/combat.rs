//! Hits and damage.
//!
//! In `SimSet::Resolve`, after the physics layer caught up with this tick's movement,
//! every open hit window is overlapped against hurtboxes (colliders with
//! `LayerMask::HURTBOX`): attackers in `AiId` order, the targets of each in `AiId`
//! order, each target at most once per action (and hit group). A hit with `LineOfSight`
//! skips a target behind a wall. Then each hit goes through the damage pipeline, in that
//! order:
//!
//! ```text
//! dev god mode     → Ignored (no damage)
//! i-frames         → Evaded (PerfectEvaded inside a perfect-evade window)
//! guard, in arc    → perfect window: Parried (the attacker staggers, unless the guard says no)
//!                    stamina short or GuardBreak: GuardBroken (chip damage, stagger)
//!                    else Blocked (chip damage, stamina per hit)
//! armor            → less damage
//! game hooks       → crits, weaknesses, ... (`add_damage_hook`, registration order)
//! apply            → health, statuses, death; poise: a break staggers, else a reaction
//!                    of the tier the hit asked for, or the old flinch
//! ```
//!
//! A guarded hit carries no status buildup beyond the guard's shares, nor does a killing
//! one. A hit whose attacker was interrupted earlier in the same tick (parried, killed) no
//! longer lands; a target killed earlier in the tick takes no more hits.
//!
//! The guard's cost and break are decided before the hooks, from the damage as the hit
//! defines it. A hook that rescales damage must also edit `outcome` and `guard_mult` if it
//! wants Blocked versus GuardBroken decided again: the engine does not redo the guard after
//! the hooks.

use ai_core::AiId;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::flat;
use shards_motion::{
    Collider, ColliderId, LayerMask, MotionState, PhysicsWorld, QueryFilter,
};

use crate::def::{touches, ActionDef, CombatTuning, HitFlag, MovesetDef, EPS};
use crate::fighter::{
    grant_protection, hostile, set_reaction_immunity, Armor, Dead, Fighter, Health, Invulnerable, Knockback, Poise, Protection, Stamina, Team,
};
use crate::machine::{force_action, ActionEnd, ActionEnded, ActionState, InputBuffer};
use crate::status::add_status_buildup;
use crate::zone::WorldShape;

/// What became of a hit. Damage hooks may change it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// It lands in full.
    Hit,
    /// I-frames.
    Evaded,
    /// Evaded inside a perfect-evade window (`ActionDef::perfect_iframes`).
    PerfectEvaded,
    /// A guard took it; `stamina` is what the block costs.
    Blocked { stamina: f32 },
    /// A perfect guard: no damage, the attacker staggers (unless the guard says otherwise);
    /// `stamina` is what it costs (0 by default).
    Parried { stamina: f32 },
    /// The guard broke: chip damage, the defender staggers, its stamina is gone.
    GuardBroken,
    /// The hit does nothing (dev god mode, or a game hook; see [`IgnoreReason`]).
    Ignored,
}

/// Why a hit is [`Outcome::Ignored`]. The journal prints it (`god mode` for `God`).
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IgnoreReason {
    /// `Invulnerable` (the text `god mode`).
    God,
    /// A grace period, such as a game's hit grace; a hook that sets `Ignored` names itself so.
    Grace,
    /// A downed body. Reserved: nothing in the engine sets it; a game hook may.
    Downed,
    /// A game hook that did not name a reason; the journal prints `ignored`. The default for a hit a hook ignores.
    #[default]
    Hook,
}

impl IgnoreReason {
    /// The word the journal line prints.
    pub fn describe(self) -> &'static str {
        match self {
            IgnoreReason::God => "god mode",
            IgnoreReason::Grace => "grace",
            IgnoreReason::Downed => "downed",
            IgnoreReason::Hook => "ignored",
        }
    }
}

/// How heavy a hit felt, for hit stop and camera. Ordered: a heavier weight wins.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum HitWeight {
    #[default]
    Light,
    /// The hit asked for reaction tier 2 or 3.
    Heavy,
    /// Reserved for a game or a later module to set on `DamageCtx::weight` (a counterattack); the engine never sets it.
    Counter,
    /// The hit broke the target's poise.
    Break,
    /// Reserved, as `Counter` (a hit from a paired action on its victim); the engine never sets it.
    Finisher,
}

/// One hit on its way through the pipeline. The engine fills it, its modifiers change it, the
/// game's hooks may change any field, then `apply` acts on it.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageCtx {
    /// The entity whose action hit: a fighter (or a combat object).
    pub attacker: Entity,
    /// Who gets the credit: `attacker`, or the owner of a combat object. `Died.killer`,
    /// `Damaged.attacker` and friends name this entity.
    pub credit: Entity,
    pub target: Entity,
    pub action: String,
    /// Index of the hit window in the action.
    pub hit: usize,
    /// `HitDef::damage` as written (the motion value when a game applies its own formula).
    pub base: f32,
    /// What the engine made of `base`: `base * taken * guard_mult`, then `Armor`. A hook that
    /// rebuilds damage from `base` folds `taken` and `guard_mult` in itself.
    pub damage: f32,
    pub poise: f32,
    pub status: Vec<(String, f32)>,
    pub flags: Vec<HitFlag>,
    pub tags: Vec<String>,
    pub outcome: Outcome,
    /// Why the hit is `Outcome::Ignored`, when it is.
    pub ignore_reason: IgnoreReason,
    /// Flat unit direction from the attacker to the target (else the attacker's facing): pushes go this way.
    pub dir: Vec3,
    /// Reaction tier asked of the target (`HitDef::reaction`); `None`: the old flinch rule.
    pub reaction: Option<u8>,
    /// The target's reaction armour now: the moveset's `reaction_armor` and the running action's
    /// `armor` windows. A hook may raise it (a timed buff). `apply` hands this value to [`react`]
    /// (`Reaction::armor`), so a hook-raised level really stops the reaction.
    pub reaction_armor: u8,
    /// Push speed (m/s) a reaction to this hit starts with (`HitDef::knockback`).
    pub knockback: f32,
    /// Product of the target's damage-taken multipliers (1 until a game or a later module sets it),
    /// already in `damage`.
    pub taken: f32,
    /// The guard multiplier applied to `damage` (a block's or a guard break's); `None` when unguarded.
    pub guard_mult: Option<f32>,
    /// Weight floor; the engine raises it (heavy reaction, break) when it applies the hit.
    pub weight: HitWeight,
}

/// A game's step in the damage pipeline: `app.add_damage_hook(crits)`.
pub type DamageHook = fn(&mut DamageCtx, &World);

/// Damage hooks in registration order.
#[derive(Resource, Default, Clone)]
pub struct DamageHooks(pub(crate) Vec<DamageHook>);

/// Why a fighter staggered.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaggerCause {
    PoiseBroken,
    Parried,
    GuardBroken,
    /// The `stagger` tool.
    Dev,
}

impl StaggerCause {
    pub fn describe(self) -> &'static str {
        match self {
            StaggerCause::PoiseBroken => "poise broken",
            StaggerCause::Parried => "parried",
            StaggerCause::GuardBroken => "guard broken",
            StaggerCause::Dev => "dev",
        }
    }
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Damaged {
    /// The credited entity ([`DamageCtx::credit`]).
    pub attacker: Entity,
    pub target: Entity,
    pub action: String,
    pub amount: f32,
    pub poise: f32,
    pub hp_left: f32,
    /// The hit was ignored (god mode, a grace period...); `ignore_reason` says why.
    pub invulnerable: bool,
    /// Why the hit was ignored; `None` when it was not.
    pub ignore_reason: Option<IgnoreReason>,
    pub weight: HitWeight,
    /// The reaction tier the target is put into by this hit; `None`: none (or the old flinch).
    pub reaction: Option<u8>,
    /// Damage a barrier took (0 until a game or a later module sets a barrier).
    pub absorbed: f32,
    /// The combat object that hit, when it is not the credited attacker.
    pub source: Option<Entity>,
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Blocked {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    /// Damage that got through.
    pub amount: f32,
    pub stamina: f32,
    pub hp_left: f32,
    pub weight: HitWeight,
    pub absorbed: f32,
    pub source: Option<Entity>,
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Parried {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    /// What the perfect guard cost (0 by default).
    pub stamina: f32,
    pub source: Option<Entity>,
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Evaded {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    pub source: Option<Entity>,
}

/// A defender evaded a hit inside a perfect-evade window (right after its [`Evaded`]).
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct PerfectEvaded {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    pub source: Option<Entity>,
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct GuardBroken {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    pub amount: f32,
    pub hp_left: f32,
    pub weight: HitWeight,
    pub absorbed: f32,
    pub source: Option<Entity>,
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Staggered {
    pub entity: Entity,
    pub cause: StaggerCause,
}

/// A fighter was put into a reaction action by [`react`].
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Reacted {
    pub entity: Entity,
    /// Who caused it, when someone did.
    pub attacker: Option<Entity>,
    pub tier: u8,
    /// The reaction action that was forced.
    pub action: String,
}

#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Died {
    pub entity: Entity,
    pub killer: Option<Entity>,
    pub action: Option<String>,
    pub source: Option<Entity>,
}

/// A hit found by the overlap pass, resolved afterwards.
struct Detected {
    attacker: Entity,
    target: Entity,
    action: String,
    instance: u32,
    hit: usize,
}

/// Deterministic order of entities: `AiId`, then collider id.
fn order_key(world: &World, entity: Entity) -> (u32, u32) {
    (world.get::<AiId>(entity).map_or(u32::MAX, |i| i.0), world.get::<ColliderId>(entity).map_or(u32::MAX, |c| c.0))
}

/// `SimSet::Resolve`: finds this tick's hits and resolves them in order.
pub(crate) fn resolve_hits(world: &mut World) {
    for hit in detect(world) {
        resolve(world, hit);
    }
}

fn detect(world: &mut World) -> Vec<Detected> {
    let mut attackers: Vec<((u32, u32), Entity)> = world
        .query_filtered::<(Entity, &ActionState), (With<Fighter>, Without<Dead>)>()
        .iter(world)
        .filter(|(_, s)| s.current.is_some())
        .map(|(e, _)| e)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|e| (order_key(world, e), e))
        .collect();
    attackers.sort();
    let mut found = Vec::new();
    let mut marks: Vec<(Entity, u8, Entity)> = Vec::new();
    let (db, physics) = (world.resource::<ContentDb>(), world.resource::<PhysicsWorld>());
    let sight = CombatTuning::of(db).sight_height;
    for &(_, attacker) in &attackers {
        let Some(run) = world.get::<ActionState>(attacker).and_then(|s| s.current.as_ref()) else { continue };
        let Some(def) = db.get::<ActionDef>(&run.action) else { continue };
        let Some(feet) = world.get::<Transform>(attacker).map(|t| t.translation) else { continue };
        let team = world.get::<Team>(attacker);
        let mut seen: Vec<(u8, Entity)> = Vec::new();
        for (i, hit) in def.hits.iter().enumerate() {
            if !def.span(&hit.span).is_some_and(|w| touches(w, run.prev, run.t)) {
                continue;
            }
            let shape = WorldShape::new(hit.shape, hit.at, feet, run.facing);
            let mut targets: Vec<((u32, u32), Entity)> = shape
                .overlapping(physics, attacker)
                .into_iter()
                .filter(|t| world.get::<Health>(*t).is_some() && world.get::<Dead>(*t).is_none())
                .filter(|t| hostile(team, world.get::<Team>(*t)))
                .map(|t| (order_key(world, t), t))
                .collect();
            targets.sort();
            targets.dedup();
            for (_, target) in targets {
                let key = (hit.group, target);
                if run.hit.contains(&key) || seen.contains(&key) {
                    continue;
                }
                // Behind a wall: skipped, and not marked, so it may still be hit later in the window.
                if hit.flags.contains(&HitFlag::LineOfSight) && !clear_line(world, physics, attacker, target, sight) {
                    continue;
                }
                seen.push(key);
                found.push(Detected { attacker, target, action: run.action.clone(), instance: run.instance, hit: i });
            }
        }
        marks.extend(seen.into_iter().map(|(group, target)| (attacker, group, target)));
    }
    for (attacker, group, target) in marks {
        if let Some(run) = world.get_mut::<ActionState>(attacker).and_then(|s| s.into_inner().current.as_mut()) {
            run.hit.push((group, target));
        }
    }
    found
}

/// The point at `sight` (a share of the body's height) above an entity's feet: where a
/// `LineOfSight` ray starts and ends. The body's height is its collider's, else 1.7 m.
fn chest(world: &World, entity: Entity, sight: f32) -> Option<Vec3> {
    let feet = world.get::<Transform>(entity)?.translation;
    let height = world.get::<Collider>(entity).map_or(1.7, |c| c.shape.half_extents().y * 2.0);
    Some(feet + Vec3::Y * (sight * height))
}

/// Whether no `LayerMask::WORLD` collider lies between the chests of `attacker` and `target`. Their
/// own colliders never count: the ray excludes both (a fighter whose collider has the default
/// layers, which are WORLD, would otherwise block its own ray).
fn clear_line(world: &World, physics: &PhysicsWorld, attacker: Entity, target: Entity, sight: f32) -> bool {
    let (Some(from), Some(to)) = (chest(world, attacker, sight), chest(world, target, sight)) else { return true };
    let along = to - from;
    let Ok(dir) = Dir3::new(along) else { return true };
    let filter = QueryFilter::new(LayerMask::WORLD).excluding(attacker).excluding(target);
    physics.raycast(from, dir, along.length(), &filter).is_none()
}

fn resolve(world: &mut World, d: Detected) {
    let swinging =
        world.get::<ActionState>(d.attacker).and_then(|s| s.current.as_ref()).is_some_and(|r| r.instance == d.instance);
    if !swinging || world.get::<Dead>(d.target).is_some() || world.get::<Dead>(d.attacker).is_some() {
        return;
    }
    let Some(def) = world.resource::<ContentDb>().get::<ActionDef>(&d.action).cloned() else { return };
    let hit = &def.hits[d.hit];
    let facing = world.get::<ActionState>(d.attacker).and_then(|s| s.current.as_ref()).map(|r| r.facing);
    let dir = match (world.get::<Transform>(d.attacker), world.get::<Transform>(d.target)) {
        (Some(a), Some(t)) => flat(t.translation - a.translation).try_normalize(),
        _ => None,
    }
    .or_else(|| facing.and_then(|f| flat(f).try_normalize()))
    .unwrap_or(Vec3::NEG_Z);
    let mut ctx = DamageCtx {
        attacker: d.attacker,
        credit: d.attacker,
        target: d.target,
        action: d.action,
        hit: d.hit,
        base: hit.damage,
        damage: hit.damage,
        poise: hit.poise,
        status: hit.status.clone(),
        flags: hit.flags.clone(),
        tags: hit.tags.clone(),
        outcome: Outcome::Hit,
        ignore_reason: IgnoreReason::Hook,
        dir,
        reaction: hit.reaction,
        reaction_armor: reaction_armor(world, d.target),
        knockback: hit.knockback,
        taken: 1.0,
        guard_mult: None,
        weight: HitWeight::Light,
    };
    modifiers(world, &mut ctx);
    let hooks = world.resource::<DamageHooks>().0.clone();
    for hook in hooks {
        hook(&mut ctx, world);
    }
    apply(world, ctx);
}

/// The engine's modifiers: god mode, i-frames (and perfect evades), guards, armor.
pub fn modifiers(world: &World, ctx: &mut DamageCtx) {
    let target = ctx.target;
    if world.get::<Invulnerable>(target).is_some() {
        (ctx.outcome, ctx.damage, ctx.poise) = (Outcome::Ignored, 0.0, 0.0);
        ctx.ignore_reason = IgnoreReason::God;
        ctx.status.clear();
        return;
    }
    let db = world.resource::<ContentDb>();
    let running = world.get::<ActionState>(target).and_then(|s| s.current.as_ref());
    if let Some(run) = running.filter(|r| r.windows.iframes) {
        let perfect = !ctx.flags.contains(&HitFlag::NoPerfectEvade)
            && ctx.base > 0.0
            && db.get::<ActionDef>(&run.action).is_some_and(|d| d.any_open_at(&d.perfect_iframes, run.t));
        ctx.outcome = if perfect { Outcome::PerfectEvaded } else { Outcome::Evaded };
        return;
    }
    let guard = running
        .filter(|r| r.windows.guard || r.windows.perfect)
        .and_then(|r| Some((r, db.get::<ActionDef>(&r.action)?.guard.clone()?)));
    if let Some((run, guard)) = guard
        && !ctx.flags.contains(&HitFlag::Unblockable)
        && in_arc(world, ctx.attacker, target, run.facing, guard.arc)
    {
        let hit_cost = db.get::<ActionDef>(&ctx.action).and_then(|d| d.hits.get(ctx.hit)).and_then(|h| h.guard_damage);
        let cost = hit_cost.unwrap_or(ctx.damage) * guard.stamina_per_damage;
        let have = world.get::<Stamina>(target).map_or(f32::INFINITY, |s| s.current);
        if run.windows.perfect && !ctx.flags.contains(&HitFlag::NoParry) {
            let pcost = cost * guard.perfect_cost_mult;
            // A perfect guard that cannot be paid falls through to the ordinary guard below (a block, or a break
            // when the plain cost cannot be paid either; validation keeps `perfect_cost_mult` at most 1).
            if !(pcost > 0.0 && have + EPS < pcost) {
                ctx.outcome = Outcome::Parried { stamina: pcost };
                return;
            }
        }
        let (broken, share, status_mult) =
            if ctx.flags.contains(&HitFlag::GuardBreak) || have + EPS < cost {
                (true, guard.break_damage_mult.unwrap_or(guard.damage_mult), guard.break_status_mult)
            } else {
                (false, guard.damage_mult, guard.status_mult)
            };
        if broken {
            ctx.outcome = Outcome::GuardBroken;
        } else {
            ctx.poise *= guard.poise_mult;
            ctx.outcome = Outcome::Blocked { stamina: cost };
        }
        ctx.guard_mult = Some(share);
        ctx.damage *= share;
        if status_mult == 0.0 {
            ctx.status.clear();
        } else {
            for (_, amount) in &mut ctx.status {
                *amount *= status_mult;
            }
        }
    }
    if let Some(armor) = world.get::<Armor>(target) {
        ctx.damage = armor.reduce(ctx.damage);
    }
}

/// Whether `attacker` stands within the guard's arc around `facing`.
fn in_arc(world: &World, attacker: Entity, defender: Entity, facing: Vec3, arc: f32) -> bool {
    let (Some(a), Some(d)) = (world.get::<Transform>(attacker), world.get::<Transform>(defender)) else { return true };
    let Some(to) = flat(a.translation - d.translation).try_normalize() else { return true };
    to.angle_between(flat(facing).normalize_or(Vec3::NEG_Z)).to_degrees() <= arc * 0.5 + EPS
}

/// What `apply` will do, worked out before anything changes: so `Damaged` can carry the weight and
/// the reaction while it is still triggered where it always was (before death, statuses, poise).
struct Plan {
    /// The poise damage breaks the target's poise.
    breaks: bool,
    /// The tier the target is put into, if it survives.
    reacts: Option<u8>,
}

fn plan(world: &World, ctx: &DamageCtx) -> Plan {
    let breaks = matches!(ctx.outcome, Outcome::Hit | Outcome::Blocked { .. }) && poise_breaks(world, ctx.target, ctx.poise);
    let reacts = match (ctx.outcome, ctx.reaction) {
        (Outcome::Hit, Some(tier)) if !breaks => {
            should_react(world, ctx.target, &Reaction::hit(tier, Some(ctx.credit), ctx.dir, ctx.knockback, ctx.reaction_armor), ctx.reaction_armor)
                .then_some(tier)
        }
        _ => None,
    };
    Plan { breaks, reacts }
}

/// The weight an event reports: the context's floor, raised by a heavy reaction and by a break.
fn weight_of(ctx: &DamageCtx, breaks: bool) -> HitWeight {
    let mut weight = ctx.weight;
    if ctx.reaction >= Some(2) {
        weight = weight.max(HitWeight::Heavy);
    }
    if breaks {
        weight = weight.max(HitWeight::Break);
    }
    weight
}

fn apply(world: &mut World, ctx: DamageCtx) {
    let (attacker, credit, target, action) = (ctx.attacker, ctx.credit, ctx.target, ctx.action.clone());
    // The combat object that hit, when it is not the credited entity.
    let source = (ctx.attacker != ctx.credit).then_some(ctx.attacker);
    let plan = plan(world, &ctx);
    let weight = weight_of(&ctx, plan.breaks);
    match ctx.outcome {
        Outcome::Ignored => {
            let hp_left = world.get::<Health>(target).map_or(0.0, |h| h.current);
            world.trigger(Damaged {
                attacker: credit,
                target,
                action,
                amount: 0.0,
                poise: 0.0,
                hp_left,
                invulnerable: true,
                ignore_reason: Some(ctx.ignore_reason),
                weight: HitWeight::Light,
                reaction: None,
                absorbed: 0.0,
                source,
            });
        }
        Outcome::Evaded => world.trigger(Evaded { attacker: credit, defender: target, action, source }),
        Outcome::PerfectEvaded => {
            world.trigger(Evaded { attacker: credit, defender: target, action: action.clone(), source });
            world.trigger(PerfectEvaded { attacker: credit, defender: target, action, source });
        }
        Outcome::Parried { stamina } => {
            if stamina > 0.0
                && let Some(mut s) = world.get_mut::<Stamina>(target)
            {
                s.spend(stamina);
            }
            world.trigger(Parried { attacker: credit, defender: target, action, stamina, source });
            if parry_staggers(world, target) {
                stagger(world, attacker, StaggerCause::Parried);
            }
        }
        Outcome::Blocked { stamina } => {
            if let Some(mut s) = world.get_mut::<Stamina>(target) {
                s.spend(stamina);
            }
            let hp_left = hurt(world, target, ctx.damage);
            world.trigger(Blocked {
                attacker: credit,
                defender: target,
                action: action.clone(),
                amount: ctx.damage,
                stamina,
                hp_left,
                weight,
                absorbed: 0.0,
                source,
            });
            if !finish_if_dead(world, target, credit, &action, source) {
                apply_statuses(world, target, &ctx.status);
                take_poise(world, target, ctx.poise);
            }
        }
        Outcome::GuardBroken => {
            if let Some(mut s) = world.get_mut::<Stamina>(target) {
                let all = s.current;
                s.spend(all);
            }
            let hp_left = hurt(world, target, ctx.damage);
            world.trigger(GuardBroken {
                attacker: credit,
                defender: target,
                action: action.clone(),
                amount: ctx.damage,
                hp_left,
                weight,
                absorbed: 0.0,
                source,
            });
            if !finish_if_dead(world, target, credit, &action, source) {
                apply_statuses(world, target, &ctx.status);
                guard_break(world, target);
            }
        }
        Outcome::Hit => {
            let hp_left = hurt(world, target, ctx.damage);
            world.trigger(Damaged {
                attacker: credit,
                target,
                action: action.clone(),
                amount: ctx.damage,
                poise: ctx.poise,
                hp_left,
                invulnerable: false,
                ignore_reason: None,
                weight,
                reaction: plan.reacts,
                absorbed: 0.0,
                source,
            });
            if finish_if_dead(world, target, credit, &action, source) {
                return;
            }
            apply_statuses(world, target, &ctx.status);
            match ctx.reaction {
                // The old rule, line for line.
                None => {
                    if !take_poise(world, target, ctx.poise) && (ctx.damage > 0.0 || ctx.poise > 0.0) {
                        flinch(world, target);
                    }
                }
                Some(tier) => {
                    let broke = take_poise(world, target, ctx.poise);
                    if !broke && plan.reacts == Some(tier) {
                        react(world, target, Reaction::hit(tier, Some(credit), ctx.dir, ctx.knockback, ctx.reaction_armor));
                    }
                }
            }
        }
    }
}

/// Whether the defender's running guard lets a perfect guard stagger the attacker.
fn parry_staggers(world: &World, defender: Entity) -> bool {
    let db = world.resource::<ContentDb>();
    world
        .get::<ActionState>(defender)
        .and_then(|s| s.current.as_ref())
        .and_then(|r| db.get::<ActionDef>(&r.action)?.guard.as_ref().map(|g| g.parry_staggers))
        .unwrap_or(true)
}

fn apply_statuses(world: &mut World, target: Entity, status: &[(String, f32)]) {
    for (id, amount) in status {
        if world.get::<Dead>(target).is_some() {
            return;
        }
        add_status_buildup(world, target, id, *amount);
    }
}

/// Takes `damage` off `target`'s health; returns what is left.
fn hurt(world: &mut World, target: Entity, damage: f32) -> f32 {
    let Some(mut health) = world.get_mut::<Health>(target) else { return 0.0 };
    if damage > 0.0 {
        health.current = (health.current - damage).max(0.0);
    }
    health.current
}

/// Kills `target` if its health ran out; `true` if it is dead now.
fn finish_if_dead(world: &mut World, target: Entity, killer: Entity, action: &str, source: Option<Entity>) -> bool {
    if world.get::<Health>(target).is_some_and(|h| h.current <= EPS) {
        kill_with(world, target, Some(killer), Some(action.to_owned()), source);
        return true;
    }
    false
}

// --- poise ---------------------------------------------------------------------------------

/// The length of `entity`'s stagger action, if its moveset has one.
fn stagger_length(world: &World, entity: Entity) -> Option<f32> {
    moveset(world, entity)
        .and_then(|m| m.stagger.clone())
        .and_then(|a| world.resource::<ContentDb>().get::<ActionDef>(&a).map(ActionDef::length))
}

/// Whether poise damage of `amount` would break `target`'s poise now: [`take_poise`]'s tests,
/// without writing.
fn poise_breaks(world: &World, target: Entity, amount: f32) -> bool {
    if amount <= 0.0 || hyper_armored(world, target) || poise_protected(world, target) {
        return false;
    }
    let Some(poise) = world.get::<Poise>(target) else { return false };
    !poise.is_broken() && poise.current - amount <= EPS && stagger_length(world, target).is_some()
}

fn poise_protected(world: &World, entity: Entity) -> bool {
    world.get::<Protection>(entity).is_some_and(|p| p.poise > EPS)
}

/// Poise damage on `target`; `true` if it broke (and the target staggered).
/// Hyper-armor ignores poise damage; so does poise that is already broken, and poise under
/// [`Protection`]. A break ends a push in progress.
fn take_poise(world: &mut World, target: Entity, amount: f32) -> bool {
    if amount <= 0.0 || hyper_armored(world, target) || poise_protected(world, target) {
        return false;
    }
    let stagger_len = stagger_length(world, target);
    let Some(mut poise) = world.get_mut::<Poise>(target) else { return false };
    if poise.is_broken() {
        return false;
    }
    poise.current -= amount;
    poise.since_hit = 0.0;
    if poise.current > EPS {
        return false;
    }
    let Some(len) = stagger_len else {
        // Nothing to stagger into: it shrugs it off.
        poise.current = poise.max;
        return false;
    };
    poise.current = 0.0;
    poise.broken = len.max(EPS);
    let staggered = stagger(world, target, StaggerCause::PoiseBroken);
    world.entity_mut(target).remove::<Knockback>();
    staggered
}

/// Poise damage on `entity` by the engine's rules (the game's parry poise, a dodge's, a wall
/// charge's): nothing for a hyper-armoured body, a broken poise or one under [`Protection`];
/// `true` if it broke and the target staggered.
pub fn poise_damage(world: &mut World, entity: Entity, amount: f32) -> bool {
    take_poise(world, entity, amount)
}

/// Ends a break now: the poise is whole again and the moveset's `break_protection` begins. Nothing
/// happens to a poise that is not broken.
pub fn end_break(world: &mut World, entity: Entity) {
    let Some(mut poise) = world.get_mut::<Poise>(entity) else { return };
    if !poise.is_broken() {
        return;
    }
    poise.refill();
    let secs = moveset(world, entity).map_or(0.0, |m| m.break_protection);
    grant_protection(world, entity, 0.0, secs);
}

/// Clears the whole poise state at once: a full gauge, no break, no poise protection. Unlike
/// [`end_break`] it grants no protection (a boss's phase change).
pub fn reset_poise(world: &mut World, entity: Entity) {
    if let Some(mut poise) = world.get_mut::<Poise>(entity) {
        poise.refill();
    }
    let drop = match world.get_mut::<Protection>(entity) {
        Some(mut p) => {
            if p.poise != 0.0 {
                p.poise = 0.0;
            }
            p.reactions <= EPS
        }
        None => false,
    };
    if drop {
        world.entity_mut(entity).remove::<Protection>();
    }
}

fn hyper_armored(world: &World, entity: Entity) -> bool {
    world.get::<ActionState>(entity).and_then(|s| s.current.as_ref()).is_some_and(|r| r.windows.hyper_armor)
}

fn moveset(world: &World, entity: Entity) -> Option<&MovesetDef> {
    let fighter = world.get::<Fighter>(entity)?;
    world.resource::<ContentDb>().get::<MovesetDef>(&fighter.moveset)
}

// --- reactions -----------------------------------------------------------------------------

/// What asks a fighter to react ([`react`]). Callers differ in what they ask for, so the flags
/// are spelled out; the constructors fill the usual rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reaction {
    /// 1 light, 2 heavy, 3 knockdown; 0 never reacts.
    pub tier: u8,
    /// Who caused it, for the [`Reacted`] event.
    pub from: Option<Entity>,
    /// Flat direction of the push.
    pub dir: Vec3,
    /// Push speed (m/s), used when larger than the reaction's own.
    pub knockback: f32,
    /// Tiers 1-2 react even inside [`Protection::reactions`].
    pub ignore_immunity: bool,
    /// Skip the armour tests (the reaction level and hyper-armor).
    pub ignore_armor: bool,
    /// The reaction armour to test against; `None`: the target's own now (its moveset's and the
    /// running action's windows).
    pub armor: Option<u8>,
    /// Start the moveset's `reaction_immunity` after reacting.
    pub sets_immunity: bool,
}

impl Reaction {
    /// A hit's own reaction: immunity and armour apply, and it starts the immunity. `armor` is
    /// what the hit's [`DamageCtx::reaction_armor`] says.
    pub fn hit(tier: u8, from: Option<Entity>, dir: Vec3, knockback: f32, armor: u8) -> Self {
        Self {
            tier,
            from,
            dir,
            knockback,
            ignore_immunity: false,
            ignore_armor: false,
            armor: Some(armor),
            sets_immunity: true,
        }
    }

    /// A reaction that ignores immunity and armour and starts no immunity: a finisher's
    /// knockdown, a shockwave that pushes everything around it.
    pub fn forced(tier: u8, from: Option<Entity>, dir: Vec3) -> Self {
        Self {
            tier,
            from,
            dir,
            knockback: 0.0,
            ignore_immunity: true,
            ignore_armor: true,
            armor: None,
            sets_immunity: false,
        }
    }

    /// A recoil: it ignores immunity (and starts none) but armour still stops it: a parried
    /// attacker's recoil. A charge that hits a wall ignores armour too: use [`Reaction::forced`].
    pub fn recoil(tier: u8, from: Option<Entity>, dir: Vec3) -> Self {
        Self { ignore_armor: false, ..Self::forced(tier, from, dir) }
    }
}

/// The reaction armour of `entity` now: its moveset's `reaction_armor` and the `armor` windows of
/// its running action, whichever is higher.
fn reaction_armor(world: &World, entity: Entity) -> u8 {
    let permanent = moveset(world, entity).map_or(0, |m| m.reaction_armor);
    let window = world
        .get::<ActionState>(entity)
        .and_then(|s| s.current.as_ref())
        .and_then(|r| Some(world.resource::<ContentDb>().get::<ActionDef>(&r.action)?.armor_at(r.t)))
        .unwrap_or(0);
    permanent.max(window)
}

/// The reaction rule: whether `r` puts `target` into a reaction now. False for tier 0; for a tier
/// at most `armor` (or a hyper-armoured body); for a dead body, a broken poise or a body running its
/// moveset's `stagger`, `guard_broken` or `death` action; for tiers 1-2 inside the reaction immunity.
fn should_react(world: &World, target: Entity, r: &Reaction, armor: u8) -> bool {
    if r.tier == 0 || world.get::<Dead>(target).is_some() {
        return false;
    }
    if !r.ignore_armor && (r.tier <= armor || hyper_armored(world, target)) {
        return false;
    }
    if world.get::<Poise>(target).is_some_and(Poise::is_broken) {
        return false;
    }
    if let Some(run) = world.get::<ActionState>(target).and_then(|s| s.current.as_ref())
        && let Some(m) = moveset(world, target)
        && [&m.stagger, &m.guard_broken, &m.death].into_iter().flatten().any(|a| *a == run.action)
    {
        return false;
    }
    if r.tier < 3 && !r.ignore_immunity && world.get::<Protection>(target).is_some_and(|p| p.reactions > EPS) {
        return false;
    }
    true
}

/// Puts `entity` into the reaction of tier `r.tier` if the rule allows ([`Reaction`] says what is
/// asked): forces the moveset's reaction action for the tier, starts the moveset's immunity when
/// asked, starts the push (replacing any running one) and triggers [`Reacted`]. A moveset with no
/// `reactions` gets the old flinch (its `hit` action, no push, no immunity, no event). Returns whether
/// it reacted.
pub fn react(world: &mut World, entity: Entity, r: Reaction) -> bool {
    let armor = r.armor.unwrap_or_else(|| reaction_armor(world, entity));
    if !should_react(world, entity, &r, armor) {
        return false;
    }
    let Some(m) = moveset(world, entity) else { return false };
    let Some(entry) = m.reactions.get(usize::from(r.tier.clamp(1, 3)) - 1).cloned() else {
        return flinch(world, entity);
    };
    let immunity = m.reaction_immunity;
    // The immunity is assigned (not raised) and before the action is forced: a knockdown inside a
    // longer protection cuts it to the reaction's immunity, and the end of the reaction it
    // interrupts then grants its `reaction_protection` on top (the longer of the two).
    let before = (r.sets_immunity && immunity > 0.0).then(|| set_reaction_immunity(world, entity, immunity));
    if !force_action(world, entity, &entry.action) {
        if let Some(before) = before {
            set_reaction_immunity(world, entity, before);
        }
        return false;
    }
    // The reaction's push replaces the body's own momentum.
    cancel_momentum(world, entity);
    let speed = entry.push.max(r.knockback);
    match flat(r.dir).try_normalize() {
        Some(dir) if speed > 0.0 => {
            world.entity_mut(entity).insert(Knockback { velocity: dir * speed, decel: entry.decel });
        }
        _ => {
            world.entity_mut(entity).remove::<Knockback>();
        }
    }
    world.trigger(Reacted { entity, attacker: r.from, tier: r.tier, action: entry.action });
    true
}

/// Drops the flat part of the body's own velocity: a reaction's push is the whole of its motion.
fn cancel_momentum(world: &mut World, entity: Entity) {
    if let Some(mut m) = world.get_mut::<MotionState>(entity)
        && (m.velocity.x != 0.0 || m.velocity.z != 0.0)
    {
        m.velocity.x = 0.0;
        m.velocity.z = 0.0;
    }
}

/// A hit that did not stagger: the moveset's `hit` reaction, unless the fighter is
/// hyper-armored or in another forced action (a stagger is not cut short by a flinch).
/// `true` if it flinched.
fn flinch(world: &mut World, target: Entity) -> bool {
    let Some(reaction) = moveset(world, target).and_then(|m| m.hit.clone()) else { return false };
    let current = world.get::<ActionState>(target).and_then(|s| s.current.as_ref());
    if hyper_armored(world, target) || current.is_some_and(|r| r.forced && r.action != reaction) {
        return false;
    }
    force_action(world, target, &reaction)
}

/// A guard break: the moveset's `guard_broken` action (forced, then [`Staggered`] with cause
/// `GuardBroken`), else its `stagger`. Then the moveset's `guard_broken_push`, if any, starts a push
/// at its share of the body's current flat velocity.
fn guard_break(world: &mut World, target: Entity) {
    let Some(m) = moveset(world, target) else { return };
    let (broken, push) = (m.guard_broken.clone(), m.guard_broken_push);
    match broken {
        Some(action) => {
            if world.get::<Dead>(target).is_none() && force_action(world, target, &action) {
                world.trigger(Staggered { entity: target, cause: StaggerCause::GuardBroken });
            }
        }
        None => {
            stagger(world, target, StaggerCause::GuardBroken);
        }
    }
    if let Some((scale, decel)) = push {
        let velocity = world.get::<MotionState>(target).map_or(Vec3::ZERO, |m| flat(m.velocity)) * scale;
        if world.get::<Dead>(target).is_none() {
            // The push is read above, then replaces the body's own velocity.
            cancel_momentum(world, target);
            if velocity.length() > EPS {
                world.entity_mut(target).insert(Knockback { velocity, decel });
            }
        }
    }
}

/// Forces the moveset's `stagger` action on `entity`, cutting its action short.
/// `false` (and nothing happens) if its moveset has none, or it is dead.
pub fn stagger(world: &mut World, entity: Entity, cause: StaggerCause) -> bool {
    if world.get::<Dead>(entity).is_some() {
        return false;
    }
    let Some(action) = moveset(world, entity).and_then(|m| m.stagger.clone()) else { return false };
    if !force_action(world, entity, &action) {
        return false;
    }
    world.trigger(Staggered { entity, cause });
    true
}

/// `entity` dies: [`Dead`], health 0, its buffer emptied, no push, [`Died`], then its moveset's
/// `death` action (or nothing).
pub fn kill(world: &mut World, entity: Entity, killer: Option<Entity>, action: Option<String>) {
    kill_with(world, entity, killer, action, None);
}

/// [`kill`], naming the combat object that dealt the blow (`Died::source`) when it is not the killer.
pub fn kill_with(
    world: &mut World,
    entity: Entity,
    killer: Option<Entity>,
    action: Option<String>,
    source: Option<Entity>,
) {
    // Already dead, or gone (a game hook despawned it earlier in the tick).
    if world.get_entity(entity).is_err() || world.get::<Dead>(entity).is_some() {
        return;
    }
    world.entity_mut(entity).insert(Dead);
    world.entity_mut(entity).remove::<Knockback>();
    if let Some(mut h) = world.get_mut::<Health>(entity) {
        h.current = 0.0;
    }
    if let Some(mut b) = world.get_mut::<InputBuffer>(entity) {
        b.queued.clear();
    }
    world.trigger(Died { entity, killer, action, source });
    let death = moveset(world, entity).and_then(|m| m.death.clone());
    match death {
        Some(death) => {
            force_action(world, entity, &death);
        }
        None => {
            let ended = world.get_mut::<ActionState>(entity).and_then(|mut s| s.current.take());
            if let Some(run) = ended {
                world.trigger(ActionEnded::of(entity, &run, ActionEnd::Interrupted));
            }
        }
    }
}

/// Brings a dead fighter back: [`Dead`] goes, the action it was in (its death) is cut and
/// its poise is whole again. `false`, and nothing happens, if it was not dead. Health is
/// the caller's to set first (the `heal` tool makes it full, a loaded game gives back the
/// saved one), and so is [`Healed`](crate::Healed), which lets a game undo what its `Died`
/// hooks did (a lock-on target again, no corpse).
pub fn revive(world: &mut World, entity: Entity) -> bool {
    if world.get_entity(entity).is_err() || world.get::<Dead>(entity).is_none() {
        return false;
    }
    world.entity_mut(entity).remove::<Dead>();
    let ended = world.get_mut::<ActionState>(entity).and_then(|mut s| s.current.take());
    if let Some(run) = ended {
        world.trigger(ActionEnded::of(entity, &run, ActionEnd::Interrupted));
    }
    if let Some(mut poise) = world.get_mut::<Poise>(entity) {
        poise.refill();
    }
    true
}

/// `SimSet::Resolve`, last: fighters whose health ran out some other way (a status
/// hook, a dev `set`) die too.
pub(crate) fn check_deaths(world: &mut World) {
    let mut dying: Vec<((u32, u32), Entity)> = world
        .query_filtered::<(Entity, &Health), (With<Fighter>, Without<Dead>)>()
        .iter(world)
        .filter(|(_, h)| h.current <= EPS)
        .map(|(e, _)| e)
        .collect::<Vec<_>>()
        .into_iter()
        .map(|e| (order_key(world, e), e))
        .collect();
    dying.sort();
    for (_, entity) in dying {
        kill(world, entity, None, None);
    }
}
