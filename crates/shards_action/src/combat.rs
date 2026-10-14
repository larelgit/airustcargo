//! Hits and damage.
//!
//! In `SimSet::Resolve`, after the physics layer caught up with this tick's movement,
//! every open hit window is overlapped against hurtboxes (colliders with
//! `LayerMask::HURTBOX`): attackers in `AiId` order, the targets of each in `AiId`
//! order, each target at most once per action (and hit group). Then each hit goes
//! through the damage pipeline, in that order:
//!
//! ```text
//! dev god mode     → Ignored (no damage)
//! i-frames         → Evaded
//! guard, in arc    → perfect window: Parried (the attacker staggers)
//!                    stamina short or GuardBreak: GuardBroken (chip damage, stagger)
//!                    else Blocked (chip damage, stamina per damage)
//! armor            → less damage
//! game hooks       → crits, weaknesses, ... (`add_damage_hook`, registration order)
//! apply            → health, statuses, death; poise: a break staggers, else a flinch
//! ```
//!
//! A hit whose attacker was interrupted earlier in the same tick (parried, killed)
//! no longer lands; a target killed earlier in the tick takes no more hits.

use ai_core::AiId;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::flat;
use shards_motion::{ColliderId, PhysicsWorld};

use crate::def::{touches, ActionDef, HitFlag, MovesetDef, EPS};
use crate::fighter::{hostile, Armor, Dead, Fighter, Health, Invulnerable, Poise, Stamina, Team};
use crate::machine::{force_action, ActionState, InputBuffer};
use crate::status::add_status_buildup;
use crate::zone::WorldShape;

/// What became of a hit. Damage hooks may change it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// It lands in full.
    Hit,
    /// I-frames.
    Evaded,
    /// A guard took it; `stamina` is what the block costs.
    Blocked { stamina: f32 },
    /// A perfect guard: no damage, the attacker staggers.
    Parried,
    /// The guard broke: chip damage, the defender staggers, its stamina is gone.
    GuardBroken,
    /// Dev god mode.
    Ignored,
}

/// One hit on its way through the pipeline. Hooks read it all and may change
/// `damage`, `poise`, `status` and `outcome`.
#[derive(Clone, Debug, PartialEq)]
pub struct DamageCtx {
    pub attacker: Entity,
    pub target: Entity,
    pub action: String,
    /// Index of the hit window in the action.
    pub hit: usize,
    /// Damage as the action defines it.
    pub base: f32,
    pub damage: f32,
    pub poise: f32,
    pub status: Vec<(String, f32)>,
    pub flags: Vec<HitFlag>,
    pub tags: Vec<String>,
    pub outcome: Outcome,
}

/// A game's step in the damage pipeline: `app.add_damage_hook(crits)`.
pub type DamageHook = fn(&mut DamageCtx, &World);

/// Damage hooks in registration order.
#[derive(Resource, Default, Clone)]
pub struct DamageHooks(pub(crate) Vec<DamageHook>);

/// Why a fighter staggered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Event, Clone, Debug, PartialEq)]
pub struct Damaged {
    pub attacker: Entity,
    pub target: Entity,
    pub action: String,
    pub amount: f32,
    pub poise: f32,
    pub hp_left: f32,
    /// God mode took it all.
    pub invulnerable: bool,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct Blocked {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    /// Damage that got through.
    pub amount: f32,
    pub stamina: f32,
    pub hp_left: f32,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct Parried {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct Evaded {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct GuardBroken {
    pub attacker: Entity,
    pub defender: Entity,
    pub action: String,
    pub amount: f32,
    pub hp_left: f32,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct Staggered {
    pub entity: Entity,
    pub cause: StaggerCause,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct Died {
    pub entity: Entity,
    pub killer: Option<Entity>,
    pub action: Option<String>,
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

fn resolve(world: &mut World, d: Detected) {
    let swinging =
        world.get::<ActionState>(d.attacker).and_then(|s| s.current.as_ref()).is_some_and(|r| r.instance == d.instance);
    if !swinging || world.get::<Dead>(d.target).is_some() || world.get::<Dead>(d.attacker).is_some() {
        return;
    }
    let Some(def) = world.resource::<ContentDb>().get::<ActionDef>(&d.action).cloned() else { return };
    let hit = &def.hits[d.hit];
    let mut ctx = DamageCtx {
        attacker: d.attacker,
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
    };
    modifiers(world, &mut ctx);
    let hooks = world.resource::<DamageHooks>().0.clone();
    for hook in hooks {
        hook(&mut ctx, world);
    }
    apply(world, ctx);
}

/// The engine's modifiers: god mode, i-frames, guards, armor.
pub fn modifiers(world: &World, ctx: &mut DamageCtx) {
    let target = ctx.target;
    if world.get::<Invulnerable>(target).is_some() {
        (ctx.outcome, ctx.damage, ctx.poise) = (Outcome::Ignored, 0.0, 0.0);
        ctx.status.clear();
        return;
    }
    let running = world.get::<ActionState>(target).and_then(|s| s.current.as_ref());
    if running.is_some_and(|r| r.windows.iframes) {
        ctx.outcome = Outcome::Evaded;
        return;
    }
    let guard = running
        .filter(|r| r.windows.guard || r.windows.perfect)
        .and_then(|r| Some((r, world.resource::<ContentDb>().get::<ActionDef>(&r.action)?.guard.clone()?)));
    if let Some((run, guard)) = guard
        && !ctx.flags.contains(&HitFlag::Unblockable)
        && in_arc(world, ctx.attacker, target, run.facing, guard.arc)
    {
        if run.windows.perfect && !ctx.flags.contains(&HitFlag::NoParry) {
            ctx.outcome = Outcome::Parried;
            return;
        }
        let cost = ctx.damage * guard.stamina_per_damage;
        let have = world.get::<Stamina>(target).map_or(f32::INFINITY, |s| s.current);
        ctx.outcome = if ctx.flags.contains(&HitFlag::GuardBreak) || have + EPS < cost {
            Outcome::GuardBroken
        } else {
            ctx.poise *= guard.poise_mult;
            Outcome::Blocked { stamina: cost }
        };
        ctx.damage *= guard.damage_mult;
        ctx.status.clear();
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

fn apply(world: &mut World, ctx: DamageCtx) {
    let (attacker, target, action) = (ctx.attacker, ctx.target, ctx.action.clone());
    match ctx.outcome {
        Outcome::Ignored => {
            let hp_left = world.get::<Health>(target).map_or(0.0, |h| h.current);
            world.trigger(Damaged { attacker, target, action, amount: 0.0, poise: 0.0, hp_left, invulnerable: true });
        }
        Outcome::Evaded => world.trigger(Evaded { attacker, defender: target, action }),
        Outcome::Parried => {
            world.trigger(Parried { attacker, defender: target, action });
            stagger(world, attacker, StaggerCause::Parried);
        }
        Outcome::Blocked { stamina } => {
            if let Some(mut s) = world.get_mut::<Stamina>(target) {
                s.spend(stamina);
            }
            let hp_left = hurt(world, target, ctx.damage);
            world.trigger(Blocked {
                attacker,
                defender: target,
                action: action.clone(),
                amount: ctx.damage,
                stamina,
                hp_left,
            });
            if !finish_if_dead(world, target, attacker, &action) {
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
                attacker,
                defender: target,
                action: action.clone(),
                amount: ctx.damage,
                hp_left,
            });
            apply_statuses(world, target, &ctx.status);
            if !finish_if_dead(world, target, attacker, &action) {
                stagger(world, target, StaggerCause::GuardBroken);
            }
        }
        Outcome::Hit => {
            let hp_left = hurt(world, target, ctx.damage);
            world.trigger(Damaged {
                attacker,
                target,
                action: action.clone(),
                amount: ctx.damage,
                poise: ctx.poise,
                hp_left,
                invulnerable: false,
            });
            apply_statuses(world, target, &ctx.status);
            if finish_if_dead(world, target, attacker, &action) {
                return;
            }
            if !take_poise(world, target, ctx.poise) && (ctx.damage > 0.0 || ctx.poise > 0.0) {
                flinch(world, target);
            }
        }
    }
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
fn finish_if_dead(world: &mut World, target: Entity, killer: Entity, action: &str) -> bool {
    if world.get::<Health>(target).is_some_and(|h| h.current <= EPS) {
        kill(world, target, Some(killer), Some(action.to_owned()));
        return true;
    }
    false
}

/// Poise damage on `target`; `true` if it broke (and the target staggered).
/// Hyper-armor ignores poise damage; so does poise that is already broken.
fn take_poise(world: &mut World, target: Entity, amount: f32) -> bool {
    if amount <= 0.0 || hyper_armored(world, target) {
        return false;
    }
    let stagger_len = moveset(world, target)
        .and_then(|m| m.stagger.clone())
        .and_then(|a| world.resource::<ContentDb>().get::<ActionDef>(&a).map(ActionDef::length));
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
    stagger(world, target, StaggerCause::PoiseBroken)
}

fn hyper_armored(world: &World, entity: Entity) -> bool {
    world.get::<ActionState>(entity).and_then(|s| s.current.as_ref()).is_some_and(|r| r.windows.hyper_armor)
}

fn moveset(world: &World, entity: Entity) -> Option<&MovesetDef> {
    let fighter = world.get::<Fighter>(entity)?;
    world.resource::<ContentDb>().get::<MovesetDef>(&fighter.moveset)
}

/// A hit that did not stagger: the moveset's `hit` reaction, unless the fighter is
/// hyper-armored or in another forced action (a stagger is not cut short by a flinch).
fn flinch(world: &mut World, target: Entity) {
    let Some(reaction) = moveset(world, target).and_then(|m| m.hit.clone()) else { return };
    let current = world.get::<ActionState>(target).and_then(|s| s.current.as_ref());
    if hyper_armored(world, target) || current.is_some_and(|r| r.forced && r.action != reaction) {
        return;
    }
    force_action(world, target, &reaction);
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

/// `entity` dies: [`Dead`], health 0, its buffer emptied, [`Died`], then its moveset's
/// `death` action (or nothing).
pub fn kill(world: &mut World, entity: Entity, killer: Option<Entity>, action: Option<String>) {
    // Already dead, or gone (a game hook despawned it earlier in the tick).
    if world.get_entity(entity).is_err() || world.get::<Dead>(entity).is_some() {
        return;
    }
    world.entity_mut(entity).insert(Dead);
    if let Some(mut h) = world.get_mut::<Health>(entity) {
        h.current = 0.0;
    }
    if let Some(mut b) = world.get_mut::<InputBuffer>(entity) {
        b.queued.clear();
    }
    world.trigger(Died { entity, killer, action });
    let death = moveset(world, entity).and_then(|m| m.death.clone());
    match death {
        Some(death) => {
            force_action(world, entity, &death);
        }
        None => {
            if let Some(mut s) = world.get_mut::<ActionState>(entity) {
                s.current = None;
            }
        }
    }
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
