//! Shooting (F3): hitscan shots, and the one ray every aimed system shares.
//!
//! An action's `Fire` effect ([`FireDef`]) fires a shot at its moment: one or more pellets, each an
//! instant ray from the shooter's [`shot_ray`] turned by a random spread, that stops at the first
//! `WORLD` collider and hits up to `pierce + 1` bodies on the way through the damage pipeline of a
//! swing (credit, body parts and their factors, guards, i-frames, armour, the game's hooks, reactions,
//! poise, statuses). Nothing is spawned and nothing flies: the shot resolves inside the effect.
//!
//! - **The ray** ([`shot_ray`], [`shot_aim`]): from the shooter's eye (`shards_motion::eye_of`) along
//!   its look (`look_dir`, pitch included) for a body that looks (F1a); for one that does not, toward
//!   its target's aim point (its lock-on, else the [`AimTarget`](crate::AimTarget) an NPC's brain
//!   mirrors; a body part's centre for a part), else toward its aim point (`Intent::aim`), else along
//!   the way it faces, level. Pure: it reads the world and draws nothing, so a line-of-fire check
//!   (F11) and the shot never disagree. `FireDef::aim_error_deg` (a shot aimed at a target only) and
//!   the spread are drawn when the shot fires.
//! - **Pellets** draw their turn from a keyed stream: the seed, the tick, the shooter's `AiId`, the
//!   shot's ordinal among the shooter's shots this tick and the pellet's index, uniform in the cone of
//!   `spread_deg`. The same inputs give the same pellets, with a window or without one.
//! - **What a ray takes** ([`trace_shot`]): the shooter, its parts and (for a combat object) its owner
//!   are never met; a body part stands for its body (the part a ray goes through first is the part it
//!   hits); allies (the team rule), dead and intangible bodies and bodies that cannot be hurt are looked
//!   through and do not count; a body whose own capsule takes no hits (`body_hurtbox: false`) is hit only
//!   through a part. Each pellet hits a body at most once, and the run's "once per action" ledger of
//!   hit windows does not apply: the pellets of a shot, and the shots of a burst, each land, so a
//!   shotgun's damage adds up.
//! - **Order**: shots resolve in `ActionSystems::Effects` (`SimSet::Act`), where the effects of the
//!   fighters (in `AiId` order) and then of the combat objects run; the pellets of a shot in index
//!   order, the bodies along a pellet nearest first. A pellet is traced after the ones before it have
//!   landed, so a body the first pellet killed is looked through by the second. The world is as the
//!   tick found it (the physics sync before `SimSet::Input`): a swing's hits resolve after this tick's
//!   movement, a shot's before it, so [`shot_ray`] and the `shot_trace` query between ticks say what a
//!   shot fired in the next tick meets.
//! - **Events** for the picture and the sound ([`ShotFired`], [`ShotImpact`]), both net events: a co-op
//!   client shows the host's. The NPC module turns `ShotFired::noise` into a `Noise`.
//! - **The journal** says one line per shot (`e5 fires test_rifle_shot: e7 head -45 hp (hp 55)`); the
//!   hit, block, parry and evade lines of its pellets are folded into it (the events themselves fire
//!   as for a swing). Reactions, staggers, breaks and deaths keep their own lines.
//! - **Recoil**: `FireDef::kick` turns the shooter's `Intent::look` after the shot (a body that looks
//!   only), so the next tick aims where the kick left it. The replay records only what changes between
//!   ticks, so it plays the kick back by firing again.
//!
//! What a hit window has and a shot does not: `span`, `shape`, `at` (the ray is the shape), `group`,
//! `max_targets` (`pierce` caps a pellet), `repeat`, and the `LineOfSight` flag (a ray always stops at
//! walls; the content check refuses it on a shot). A perfect guard parries a pellet and staggers the
//! shooter as it does a swinger; a game whose bullets cannot be parried gives them `NoParry`.

use ai_core::journal::{self, JournalKind};
use ai_core::prelude::*;
use ai_core::{label, AiIndex, DetRng, SimSeed};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::input_names::{HEAVY, LIGHT};
use shards_core::{Gaze, Intent, NetRole};
use shards_motion::{aim_point_of, eye_of, look_dir, params_of, Collider, LayerMask, PhysicsWorld, QueryFilter, RayHit};

use crate::combat::{land, Downed, IgnoreReason, Landed, Landing, Outcome, Payload};
use crate::context::may_hit;
use crate::def::{ActionDef, Effect, HitFlag, MovesetDef};
use crate::fighter::{hostile, Dead, Fighter, Health, Team};
use crate::machine::{intangible, ActionState};
use crate::objects::{living_owner, target_of, CombatObject};
use crate::parts::{body_hurtbox, body_of, parts_of, Part, SpawnedParts};

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn one_u8() -> u8 {
    1
}

fn is_one_u8(v: &u8) -> bool {
    *v == 1
}

fn default_range() -> f32 {
    DEFAULT_RANGE
}

fn is_default_range(v: &f32) -> bool {
    *v == DEFAULT_RANGE
}

fn is_no_kick(v: &(f32, f32)) -> bool {
    *v == (0.0, 0.0)
}

/// How far a shot reaches when its `range` is not given (m).
pub const DEFAULT_RANGE: f32 = 100.0;

/// Salts of the shooter's keyed streams: a pellet's turn, a shot's aim error.
const SPREAD_SALT: u64 = 0x5407_5e1d;
const ERROR_SALT: u64 = 0x5407_e660;
const SPAWN_ERROR_SALT: u64 = 0x5407_5a37;

/// A hitscan shot, an action's `Fire` effect: `pellets` instant rays from the shooter's [`shot_ray`],
/// each turned at random within `spread_deg`, reaching `range` metres, stopping at the first `WORLD`
/// collider, hitting up to `pierce + 1` bodies with the payload below (the fields a hit window has,
/// meaning the same).
///
/// ```ron
/// // a scatter gun: eight pellets in a 6 degree cone, weaker past 8 m, as loud as 30 m
/// effects: [(at: 0.0, effect: Fire((pellets: 8, spread_deg: 6, range: 40, falloff: [(8, 1.0), (30, 0.25)],
///                                   noise: 30, damage: 9, poise: 4, reaction: 1)))]
/// ```
#[derive(Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FireDef {
    /// Rays per shot (a shotgun's pellets), at least 1.
    #[serde(default = "one_u8", skip_serializing_if = "is_one_u8")]
    pub pellets: u8,
    /// The largest angle a pellet turns from the aim, in degrees (the cone's half angle), 0 to 90: each
    /// pellet is drawn uniformly in that cone. 0: every pellet flies along the aim.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub spread_deg: f32,
    /// How far a ray reaches (m), above 0. Default 100.
    #[serde(default = "default_range", skip_serializing_if = "is_default_range")]
    pub range: f32,
    /// The share of `damage` and `poise` a pellet keeps by the distance it flew: points `(metres, share)`,
    /// distances ascending, shares never rising; between two points the share is interpolated, before the
    /// first it is the first's, past the last the last's. Empty: the whole hit at any range.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub falloff: Vec<(f32, f32)>,
    /// How many bodies a ray passes through: 0 stops at the first body it hits, 1 hits the one behind
    /// it too, and so on.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub pierce: u8,
    /// The radius (m) NPCs hear the shot within, from where it is fired (a `Noise`); 0: silent.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub noise: f32,
    /// For a shot aimed at a target (a body that does not look: an NPC), the largest angle (degrees, 0
    /// to 90) the whole shot misses its aim by, drawn per shot. A body that looks aims itself.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub aim_error_deg: f32,
    /// Recoil: `(pitch, yaw)` in degrees added to the shooter's look after the shot (a positive pitch
    /// kicks up, a positive yaw left), held to its pitch limits. A body without a look feels none.
    #[serde(default, skip_serializing_if = "is_no_kick")]
    pub kick: (f32, f32),
    /// Damage of one pellet.
    pub damage: f32,
    /// Poise damage of one pellet.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub poise: f32,
    /// Status buildup of one pellet: `[("bleed", 30)]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub status: Vec<(String, f32)>,
    /// `Unblockable`, `GuardBreak`, `NoParry`, `NoPerfectEvade` (not `LineOfSight`: a ray stops at walls).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<HitFlag>,
    /// Free-form tags for the game's damage hooks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Reaction tier asked of a body a pellet hits (0 none, 1 light, 2 heavy, 3 knockdown); `None`: the
    /// moveset's flinch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<u8>,
    /// Push (m/s) a reaction to a pellet starts with, along the pellet's flight.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub knockback: f32,
    /// Stamina a guard pays for a pellet, before its `stamina_per_damage`; `None`: the pellet's damage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_damage: Option<f32>,
}

impl FireDef {
    /// What a pellet carries into the pipeline.
    pub(crate) fn payload(&self) -> Payload<'_> {
        Payload {
            damage: self.damage,
            poise: self.poise,
            status: &self.status,
            flags: &self.flags,
            tags: &self.tags,
            reaction: self.reaction,
            knockback: self.knockback,
            guard_damage: self.guard_damage,
        }
    }

    /// The share of the hit a pellet keeps after flying `distance` metres ([`falloff`](Self::falloff)).
    pub fn share_at(&self, distance: f32) -> f32 {
        let points = &self.falloff;
        let (Some(first), Some(last)) = (points.first(), points.last()) else { return 1.0 };
        if distance <= first.0 {
            return first.1;
        }
        if distance >= last.0 {
            return last.1;
        }
        for pair in points.windows(2) {
            let ((d0, s0), (d1, s1)) = (pair[0], pair[1]);
            if distance <= d1 {
                let span = d1 - d0;
                return if span > 0.0 { s0 + (s1 - s0) * (distance - d0) / span } else { s1 };
            }
        }
        last.1
    }

    /// What is wrong with the numbers, field by field (`pellets must be at least 1`), for the content check.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.pellets == 0 {
            out.push("pellets must be at least 1".to_owned());
        }
        if !(0.0..=90.0).contains(&self.spread_deg) {
            out.push(format!("spread_deg must be within 0..90 degrees, the cone's half angle (is {})", self.spread_deg));
        }
        if !(self.range > 0.0 && self.range.is_finite()) {
            out.push(format!("range must be a number of metres above 0 (is {})", self.range));
        }
        for (i, (d, s)) in self.falloff.iter().enumerate() {
            if !(*d >= 0.0 && d.is_finite()) {
                out.push(format!("falloff[{i}] distance must be a number of metres >= 0 (is {d})"));
            }
            if !(*s >= 0.0 && s.is_finite()) {
                out.push(format!("falloff[{i}] share must be a number >= 0 (is {s})"));
            }
        }
        for (i, pair) in self.falloff.windows(2).enumerate() {
            let ((d0, s0), (d1, s1)) = (pair[0], pair[1]);
            if d1 <= d0 {
                out.push(format!(
                    "falloff is not monotonic: falloff[{}] at {d1} m does not come after falloff[{i}] at {d0} m; give the points by ascending distance",
                    i + 1
                ));
            }
            if s1 > s0 {
                out.push(format!(
                    "falloff is not monotonic: the share rises from {s0} at {d0} m to {s1} at {d1} m (falloff[{}]); a falloff only falls",
                    i + 1
                ));
            }
        }
        for (name, v) in [("noise", self.noise), ("damage", self.damage), ("poise", self.poise), ("knockback", self.knockback)] {
            if !(v >= 0.0 && v.is_finite()) {
                out.push(format!("{name} must be a number >= 0 (is {v})"));
            }
        }
        if let Some(g) = self.guard_damage
            && !(g >= 0.0 && g.is_finite())
        {
            out.push(format!("guard_damage must be a number >= 0 (is {g})"));
        }
        if !(0.0..=90.0).contains(&self.aim_error_deg) {
            out.push(format!("aim_error_deg must be within 0..90 degrees (is {})", self.aim_error_deg));
        }
        if !(self.kick.0.is_finite() && self.kick.1.is_finite()) {
            out.push(format!("kick must be two numbers of degrees, (pitch, yaw) (is ({}, {}))", self.kick.0, self.kick.1));
        }
        if let Some(tier) = self.reaction
            && tier > 3
        {
            out.push(format!("reaction must be a tier 0 to 3 (is {tier}): 1 light, 2 heavy, 3 knockdown"));
        }
        if self.flags.contains(&HitFlag::LineOfSight) {
            out.push("flags: LineOfSight does nothing on a shot (its ray always stops at walls); remove it".to_owned());
        }
        for (status, amount) in &self.status {
            if !(*amount >= 0.0 && amount.is_finite()) {
                out.push(format!("status `{status}` must be a number >= 0 (is {amount})"));
            }
        }
        out
    }
}

/// One pellet's hit, on its [`DamageCtx`](crate::DamageCtx): which pellet, how far it flew, the share of the hit it
/// kept by that distance (already in `damage` and `poise`), and its direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotHit {
    /// Its index in the shot, from 0.
    pub pellet: u8,
    /// Metres from where the shot was fired to where it met the body.
    pub distance: f32,
    /// [`FireDef::share_at`] that distance.
    pub share: f32,
    /// The pellet's unit direction.
    pub dir: Vec3,
}

/// A shot was fired ([`FireDef`]): before its pellets land. For the picture (a muzzle flash, a tracer)
/// and the sound; the NPC module makes a `Noise` of `noise` metres around `from`. A net event: a co-op client shows the
/// host's.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct ShotFired {
    pub shooter: Entity,
    pub action: String,
    /// Where it was fired from (the shooter's eye).
    pub from: Vec3,
    /// The unit direction it was aimed (after a target-aimed shot's aim error, before the pellets' spread).
    pub dir: Vec3,
    pub pellets: u8,
    /// The radius NPCs hear it within (m); 0: silent.
    pub noise: f32,
}

/// Where one pellet met something: a body it hit (and the part it went through), or the world collider
/// that stopped it. One per body a pellet hits and one where it stops; none for a pellet that met nothing
/// in range. For decals, sparks and blood and the impact's sound. A net event.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct ShotImpact {
    pub shooter: Entity,
    pub action: String,
    /// The pellet's index in its shot.
    pub pellet: u8,
    pub point: Vec3,
    /// The surface normal there, facing the shooter.
    pub normal: Vec3,
    /// The body hit (through a part: the part's body); `None` where a world collider stopped it.
    pub target: Option<Entity>,
    /// The world collider that stopped it (a wall, the floor, a pillar); `None` for a body.
    pub collider: Option<Entity>,
    /// The body part it went through.
    pub part: Option<Entity>,
}

/// How a shot is aimed ([`shot_aim`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotAim {
    /// Where the rays start: the eye of a character, the position of anything else.
    pub from: Vec3,
    /// The unit direction.
    pub dir: Vec3,
    /// What the aim follows.
    pub by: AimedBy,
}

/// What a [`ShotAim`] follows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AimedBy {
    /// The body's look (`Intent::look`).
    Look,
    /// A target's aim point: the lock-on, else the brain's [`AimTarget`](crate::AimTarget) (or a body part's centre).
    Target(Entity),
    /// The body's aim point (`Intent::aim`).
    Point,
    /// The way the body faces, level.
    Facing,
}

/// The ray a shot of `shooter` would fly along now: `(origin, unit direction)`. See [`shot_aim`].
pub fn shot_ray(world: &World, shooter: Entity) -> Option<(Vec3, Vec3)> {
    shot_aim(world, shooter).map(|a| (a.from, a.dir))
}

/// How `shooter` aims a shot now, read from the simulation (never a camera), with no side effects: from
/// its eye (a character; anything else from its position) along its look (held to its pitch limits); a
/// body with no look toward the aim point of its target (its lock-on, else its brain's [`AimTarget`](crate::AimTarget),
/// else the target its projectile flies at; a body part's centre for a part), else toward its aim point
/// (`Intent::aim`), else along the way it faces (its action's facing while one runs), level. `None` for an
/// entity with no position.
pub fn shot_aim(world: &World, shooter: Entity) -> Option<ShotAim> {
    let tf = world.get::<Transform>(shooter)?;
    let from = eye_of(world, shooter).unwrap_or(tf.translation);
    let intent = world.get::<Intent>(shooter);
    if let Some(look) = intent.and_then(|i| i.look).filter(|l| l.is_finite()) {
        let dir = look_dir(world, shooter).unwrap_or_else(|| look.dir());
        return Some(ShotAim { from, dir, by: AimedBy::Look });
    }
    let target = target_of(world, shooter).and_then(|id| world.resource::<AiIndex>().get(id));
    if let Some(target) = target
        && let Some(point) = aim_point(world, target)
        && let Some(dir) = (point - from).try_normalize()
    {
        return Some(ShotAim { from, dir, by: AimedBy::Target(target) });
    }
    if let Some(dir) = intent.and_then(|i| i.aim).and_then(|p| (p - from).try_normalize()) {
        return Some(ShotAim { from, dir, by: AimedBy::Point });
    }
    let running = world.get::<ActionState>(shooter).and_then(|s| s.current.as_ref()).map(|r| r.facing);
    let dir = match running.and_then(|f| shards_core::flat(f).try_normalize()) {
        Some(dir) => dir,
        None => look_dir(world, shooter).unwrap_or_else(|| shards_core::flat(*tf.forward()).try_normalize().unwrap_or(Vec3::NEG_Z)),
    };
    Some(ShotAim { from, dir, by: AimedBy::Facing })
}

/// The point a shot at `target` aims for: a body part's centre, else its aim point (the lock-on's aim
/// height, or a lock point's middle).
fn aim_point(world: &World, target: Entity) -> Option<Vec3> {
    if world.get::<Part>(target).is_some() {
        return world.get::<Transform>(target).map(|t| t.translation);
    }
    aim_point_of(world, target)
}

/// What a ray met ([`walk_ray`]), nearest first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Met {
    /// A body (through a part, when `part` is some): its capsule or a part of it.
    Body { body: Entity, part: Option<Entity> },
    /// A collider on the `WORLD` layer that is no body: a wall, the floor, a pillar, a voxel field.
    Solid,
    /// Anything else in the mask (no body and not on `WORLD`).
    Other,
}

/// Walks a ray from `from` along `dir` for `range` metres through the colliders `filter` lets
/// through, nearest first, handing `visit` each thing it meets with its hit; `visit` returns `true`
/// to stop. A body is met once (its capsule or its first part, never both): a hit on a body with
/// parts looks on past the capsule for the part inside it, and a body whose capsule takes no hits
/// (`body_hurtbox: false`) is looked through unless a part is met. A part that is off or broken, and
/// an entity no longer in the world, are passed. The one ray logic of the `AIM` line ([`crate::view_hit`])
/// and of shots ([`trace_shot`]).
pub(crate) fn walk_ray(
    world: &World,
    from: Vec3,
    dir: Dir3,
    range: f32,
    mut filter: QueryFilter,
    mut visit: impl FnMut(Met, RayHit) -> bool,
) {
    let Some(physics) = world.get_resource::<PhysicsWorld>() else { return };
    let db = world.get_resource::<ContentDb>();
    let live_part = |e: Entity| world.get::<Part>(e).is_some_and(|p| p.on && !p.broken);
    let leave_body = |filter: QueryFilter, body: Entity| {
        let mut filter = filter.excluding(body);
        for part in parts_of(world, body) {
            filter = filter.excluding(part);
        }
        filter
    };
    // Each turn meets one more thing or leaves one more out of the filter, so it ends.
    while let Some(hit) = physics.raycast(from, dir, range, &filter) {
        let e = hit.entity;
        let Ok(entity) = world.get_entity(e) else {
            filter = filter.excluding(e);
            continue;
        };
        if entity.contains::<Part>() {
            if !live_part(e) {
                filter = filter.excluding(e);
                continue;
            }
            let body = body_of(world, e);
            if visit(Met::Body { body, part: Some(e) }, hit) {
                return;
            }
            filter = leave_body(filter, body);
            continue;
        }
        if is_body(entity) {
            // A body with parts: they lie inside its capsule, so look again past the capsule.
            let part = entity
                .contains::<SpawnedParts>()
                .then(|| physics.raycast(from, dir, range, &filter.clone().excluding(e)))
                .flatten()
                .filter(|h| live_part(h.entity) && body_of(world, h.entity) == e);
            let met = match part {
                Some(p) => Some((Met::Body { body: e, part: Some(p.entity) }, p)),
                None if body_hurtbox(world, db, e) => Some((Met::Body { body: e, part: None }, hit)),
                // The capsule takes no hits and no part was met: the ray goes on past the body.
                None => None,
            };
            if let Some((met, at)) = met
                && visit(met, at)
            {
                return;
            }
            filter = leave_body(filter, e);
            continue;
        }
        let solid = entity.get::<Collider>().is_none_or(|c| c.layers.memberships.intersects(LayerMask::WORLD));
        if visit(if solid { Met::Solid } else { Met::Other }, hit) {
            return;
        }
        filter = filter.excluding(e);
    }
}

/// A body: something with health, a fighter, a character, or a body with parts.
fn is_body(e: EntityRef) -> bool {
    e.contains::<Health>() || e.contains::<Fighter>() || e.contains::<SpawnedParts>() || e.contains::<shards_motion::CharacterController>()
}

/// Where one ray of a shot goes ([`trace_shot`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShotPath {
    /// The bodies it hits, nearest first: `(body, part, hit)`.
    pub bodies: Vec<(Entity, Option<Entity>, RayHit)>,
    /// The world collider that stops it, if one does within range.
    pub stop: Option<RayHit>,
}

/// Where a ray of `shooter`'s shot from `from` along `dir` goes, without firing: the bodies it would
/// hit (at most `pierce + 1`, nearest first) and what stops it, within `range` metres. The rules of a
/// shot: the shooter, its parts and its owner (an object's) are never met; allies, the dead, the
/// intangible and bodies that cannot be hurt are looked through; a `WORLD` collider that is no body
/// stops it. Reads the world only.
pub fn trace_shot(world: &World, shooter: Entity, from: Vec3, dir: Vec3, range: f32, pierce: u8) -> ShotPath {
    let mut path = ShotPath::default();
    let Ok(dir) = Dir3::new(dir) else { return path };
    let owner = world.get::<CombatObject>(shooter).and_then(|o| living_owner(world, o));
    let mut filter = QueryFilter::new(LayerMask::WORLD | LayerMask::HURTBOX).excluding(shooter);
    for own in std::iter::once(shooter).chain(owner) {
        filter = filter.excluding(own);
        for part in parts_of(world, own) {
            filter = filter.excluding(part);
        }
    }
    let team = owner.unwrap_or(shooter);
    let instance = world.get::<ActionState>(shooter).and_then(|s| s.current.as_ref()).map_or(0, |r| r.instance);
    let takes = pierce as usize + 1;
    walk_ray(world, from, dir, range, filter, |met, hit| match met {
        Met::Body { body, part } => {
            if takes_hit(world, shooter, team, instance, body) {
                path.bodies.push((body, part, hit));
            }
            path.bodies.len() >= takes
        }
        Met::Solid => {
            path.stop = Some(hit);
            true
        }
        Met::Other => false,
    });
    path
}

/// Whether a ray of `shooter` (credited to `team`'s side) takes a hit on `body`, as a swing would: it
/// has health, is not dead or intangible, is hostile, and no pair holds it for another (a downed body
/// takes the pellet, which does nothing to it).
fn takes_hit(world: &World, shooter: Entity, team: Entity, instance: u32, body: Entity) -> bool {
    body != team
        && world.get::<Health>(body).is_some()
        && world.get::<Dead>(body).is_none()
        && hostile(world.get::<Team>(team), world.get::<Team>(body))
        && may_hit(world, shooter, instance, body)
        && !intangible(world, body)
}

/// A unit vector drawn uniformly within `half_deg` degrees of `axis` (a uniform cone: the cosine of the
/// angle off the axis uniform in `[cos half, 1]`, the turn about it uniform in a full turn).
pub fn cone_dir(axis: Vec3, half_deg: f32, rng: &mut DetRng) -> Vec3 {
    let axis = axis.try_normalize().unwrap_or(Vec3::NEG_Z);
    if half_deg <= 0.0 {
        return axis;
    }
    let (u, v) = (rng.f32(), rng.f32());
    let cos_max = ops::cos(half_deg.to_radians());
    let cos_t = 1.0 - u * (1.0 - cos_max);
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    let (sin_p, cos_p) = ops::sin_cos(v * std::f32::consts::TAU);
    let (b1, b2) = axis.any_orthonormal_pair();
    (axis * cos_t + (b1 * cos_p + b2 * sin_p) * sin_t).try_normalize().unwrap_or(axis)
}

/// The keyed stream of one draw of `shooter`'s shot: the seed, the tick, its `AiId`, the purpose, the
/// shot's ordinal this tick and the pellet.
fn stream(world: &World, id: u32, salt: u64, ordinal: u32, pellet: u32) -> DetRng {
    let (seed, tick) = (world.resource::<SimSeed>().0, world.resource::<SimTick>().0);
    DetRng::from_keys(&[seed, tick, id as u64, salt, ordinal as u64, pellet as u64])
}

/// The bookkeeping of shots in the tick under way: each shooter's shots so far (the ordinal that keys
/// their draws), and whether a shot is being resolved (its pellets' hit lines go into its own line).
/// Nothing in it outlives its tick: [`forget_shots`] empties it at the end of every tick (and the counts
/// start again if a tick moves on without that), so a snapshot needs none of it.
#[derive(Resource, Default)]
pub(crate) struct Shooting {
    tick: u64,
    fired: Vec<(u32, u32)>,
    resolving: bool,
}

/// `FixedLast`: the tick's shots are over.
fn forget_shots(mut shooting: ResMut<Shooting>) {
    if !shooting.fired.is_empty() {
        shooting.fired.clear();
    }
}

/// Whether a shot is being resolved now: the journal lines of its pellets' hits are folded into the shot's.
pub(crate) fn resolving(world: &World) -> bool {
    world.get_resource::<Shooting>().is_some_and(|s| s.resolving)
}

/// The next ordinal of `id`'s shots in this tick.
fn next_ordinal(world: &mut World, id: u32) -> u32 {
    let tick = world.resource::<SimTick>().0;
    let mut s = world.get_resource_or_init::<Shooting>();
    if s.tick != tick {
        s.tick = tick;
        s.fired.clear();
    }
    match s.fired.iter_mut().find(|(who, _)| *who == id) {
        Some((_, n)) => {
            *n += 1;
            *n
        }
        None => {
            s.fired.push((id, 0));
            0
        }
    }
}

/// `dir` turned at random by up to `deg` degrees: the aim error of a target-aimed `Look` spawn of the body
/// with `id` ([`SpawnDef::aim_error_deg`](crate::SpawnDef)), drawn from its keyed stream (its own salt, and
/// its count of shots and spawns this tick).
pub(crate) fn aim_error(world: &mut World, id: u32, deg: f32, dir: Vec3) -> Vec3 {
    let ordinal = next_ordinal(world, id);
    cone_dir(dir, deg, &mut stream(world, id, SPAWN_ERROR_SALT, ordinal, 0))
}

/// How one pellet ended, for the shot's journal line.
enum End {
    Body { target: Entity, landed: Landed },
    Stop { name: String, distance: f32 },
    Miss,
}

/// Fires `def` for `shooter` now, as its action `action`'s `Fire` effect does. Nothing on a machine that
/// does not simulate (a co-op client shows the host's shot), for a dead or downed shooter, or for a combat
/// object whose owner is dead or gone.
pub fn fire(world: &mut World, shooter: Entity, action: &str, def: &FireDef) {
    if !world.get_resource::<NetRole>().is_none_or(|r| r.simulates()) {
        return;
    }
    if world.get::<Dead>(shooter).is_some() || world.get::<Downed>(shooter).is_some() {
        return;
    }
    if world.get::<CombatObject>(shooter).is_some_and(|o| living_owner(world, o).is_none()) {
        return;
    }
    let Some(aim) = shot_aim(world, shooter) else { return };
    let id = world.get::<AiId>(shooter).map_or(u32::MAX, |i| i.0);
    let ordinal = next_ordinal(world, id);
    let axis = match aim.by {
        AimedBy::Target(_) if def.aim_error_deg > 0.0 => cone_dir(aim.dir, def.aim_error_deg, &mut stream(world, id, ERROR_SALT, ordinal, 0)),
        _ => aim.dir,
    };
    let dirs: Vec<Vec3> = (0..def.pellets)
        .map(|k| cone_dir(axis, def.spread_deg, &mut stream(world, id, SPREAD_SALT, ordinal, k as u32)))
        .collect();
    world.trigger(ShotFired { shooter, action: action.to_owned(), from: aim.from, dir: axis, pellets: def.pellets, noise: def.noise });
    let weight = world.resource::<ContentDb>().get::<ActionDef>(action).and_then(|d| d.weight);
    let running = world.get::<ActionState>(shooter).and_then(|s| s.current.as_ref()).filter(|r| r.action == action).map(|r| r.instance);
    world.get_resource_or_init::<Shooting>().resolving = true;
    let mut ends: Vec<End> = Vec::new();
    for (k, dir) in dirs.into_iter().enumerate() {
        // Traced after the pellets before it landed: a body one of them killed is looked through.
        let path = trace_shot(world, shooter, aim.from, dir, def.range, def.pierce);
        let met = !path.bodies.is_empty();
        for (target, part, hit) in path.bodies {
            if world.get::<Dead>(shooter).is_some() {
                break;
            }
            world.trigger(ShotImpact {
                shooter,
                action: action.to_owned(),
                pellet: k as u8,
                point: hit.point,
                normal: hit.normal,
                target: Some(target),
                collider: None,
                part,
            });
            let shot = ShotHit { pellet: k as u8, distance: hit.distance, share: def.share_at(hit.distance), dir };
            let landing = Landing {
                attacker: shooter,
                target,
                part,
                action: action.to_owned(),
                instance: running,
                hit: usize::MAX,
                weight,
                payload: def.payload(),
                shot: Some(shot),
            };
            if let Some(landed) = land(world, landing) {
                ends.push(End::Body { target, landed });
            }
        }
        match path.stop {
            Some(hit) => {
                world.trigger(ShotImpact {
                    shooter,
                    action: action.to_owned(),
                    pellet: k as u8,
                    point: hit.point,
                    normal: hit.normal,
                    target: None,
                    collider: Some(hit.entity),
                    part: None,
                });
                ends.push(End::Stop { name: solid_name(world, hit.entity, hit.normal), distance: hit.distance });
            }
            None if !met => ends.push(End::Miss),
            None => {}
        }
    }
    world.get_resource_or_init::<Shooting>().resolving = false;
    kick(world, shooter, def.kick);
    let text = journal_line(world, shooter, action, def, &ends);
    journal::emit(world, JournalKind::Event, text);
}

/// Turns `shooter`'s look by `kick` (pitch, yaw in degrees), held to its pitch limits: recoil, for the next tick.
fn kick(world: &mut World, shooter: Entity, kick: (f32, f32)) {
    if kick == (0.0, 0.0) {
        return;
    }
    let Some(look) = world.get::<Intent>(shooter).and_then(|i| i.look).filter(|l| l.is_finite()) else { return };
    let turned = Gaze::new(crate::look::wrap(look.yaw + kick.1.to_radians()), look.pitch + kick.0.to_radians());
    let held = params_of(world, shooter).map_or(turned, |p| p.clamp_look(turned));
    if let Some(mut intent) = world.get_mut::<Intent>(shooter) {
        intent.look = Some(held);
    }
}

/// What a world collider is called in a shot's line: `e3 Pillar` for anything the AI sees, else by its
/// surface (`floor`, `ceiling`, `wall`).
fn solid_name(world: &World, entity: Entity, normal: Vec3) -> String {
    let named = world.get_entity(entity).ok().and_then(|e| {
        let id = e.get::<AiId>()?;
        e.contains::<AiVisible>().then(|| format!("{id} {}", ai_core::describe::label_of(e)))
    });
    named.unwrap_or_else(|| crate::look::surface(normal).to_owned())
}

/// The shot's journal line: `e5 fires test_rifle_shot: e7 head -45 hp (hp 55)`; a shot of several pellets
/// says how many and sums them by body and outcome (`e5 fires test_scatter_shot (8 pellets): e7 torso, head
/// -63 hp ×6 (hp 37), wall ×2`).
fn journal_line(world: &World, shooter: Entity, action: &str, def: &FireDef, ends: &[End]) -> String {
    // (target, outcome key) → parts, total, count, outcome; in the order first met.
    struct Group {
        target: Entity,
        key: &'static str,
        parts: Vec<String>,
        total: f32,
        count: u32,
        outcome: Outcome,
        ignore: IgnoreReason,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut stops: Vec<(String, f32, u32)> = Vec::new();
    let mut misses = 0u32;
    for end in ends {
        match end {
            End::Body { target, landed } => {
                let key = match landed.outcome {
                    Outcome::Hit => "hit",
                    Outcome::Blocked { .. } => "blocked",
                    Outcome::GuardBroken => "guard broken",
                    Outcome::Parried { .. } => "parried",
                    Outcome::Evaded => "evaded",
                    Outcome::PerfectEvaded => "perfectly evaded",
                    Outcome::Ignored => "ignored",
                };
                let at = match groups.iter().position(|g| g.target == *target && g.key == key) {
                    Some(i) => i,
                    None => {
                        groups.push(Group {
                            target: *target,
                            key,
                            parts: Vec::new(),
                            total: 0.0,
                            count: 0,
                            outcome: landed.outcome,
                            ignore: landed.ignore_reason,
                        });
                        groups.len() - 1
                    }
                };
                let g = &mut groups[at];
                g.total += landed.body;
                g.count += 1;
                if let Some(part) = &landed.part
                    && !g.parts.contains(part)
                {
                    g.parts.push(part.clone());
                }
            }
            End::Stop { name, distance } => match stops.iter_mut().find(|(n, _, _)| n == name) {
                Some((_, _, n)) => *n += 1,
                None => stops.push((name.clone(), *distance, 1)),
            },
            End::Miss => misses += 1,
        }
    }
    let times = |n: u32| if n > 1 { format!(" ×{n}") } else { String::new() };
    let mut said: Vec<String> = groups
        .iter()
        .map(|g| {
            let who = label(world, g.target);
            let parts = if g.parts.is_empty() { String::new() } else { format!(" {}", g.parts.join(", ")) };
            let hp = world.get::<Health>(g.target).map_or_else(String::new, |h| format!(" (hp {:.0})", h.current.max(0.0)));
            match g.outcome {
                Outcome::Hit => format!("{who}{parts} -{:.0} hp{}{hp}", g.total, times(g.count)),
                Outcome::Blocked { .. } | Outcome::GuardBroken => format!("{who} {}{parts} -{:.0} hp{}{hp}", g.key, g.total, times(g.count)),
                Outcome::Ignored => format!("{who}{parts} -0 ({}){}", g.ignore.describe(), times(g.count)),
                _ => format!("{who} {}{}", g.key, times(g.count)),
            }
        })
        .collect();
    said.extend(stops.iter().map(|(name, distance, n)| match n {
        1 => format!("{name} {distance:.1}m"),
        _ => format!("{name}{}", times(*n)),
    }));
    if misses > 0 {
        said.push(format!("nothing within {}m{}", crate::def::meters(def.range), times(misses)));
    }
    let pellets = if def.pellets > 1 { format!(" ({} pellets)", def.pellets) } else { String::new() };
    format!("{} fires {action}{pellets}: {}", label(world, shooter), said.join(", "))
}

// --- for agents ------------------------------------------------------------------------------

/// The `Fire` effects of `action`, in order.
fn fires_of(def: &ActionDef) -> impl Iterator<Item = &FireDef> {
    def.effects.iter().filter_map(|e| match &e.effect {
        Effect::Fire(f) => Some(f),
        _ => None,
    })
}

/// The action `input` starts for `body` (its moveset's binding) and that action's first `Fire`, read for a
/// tool or a line (`peek`: not a read of the simulation).
fn bound_fire<'a>(world: &'a World, body: Entity, input: &str) -> Option<(&'a str, &'a ActionDef, Option<&'a FireDef>)> {
    let db = world.get_resource::<ContentDb>()?;
    let moveset = db.peek::<MovesetDef>(&world.get::<Fighter>(body)?.moveset)?;
    let action = moveset.action(input)?;
    let def = db.peek::<ActionDef>(action)?;
    Some((action, def, fires_of(def).next()))
}

/// What a ray met, for an answer: `e7 Dummy head 14.2m`.
fn body_name(world: &World, body: Entity, part: Option<Entity>) -> String {
    let mut name = world.get_entity(body).map_or_else(|_| label(world, body), |e| format!("{} {}", label(world, body), ai_core::describe::label_of(e)));
    if let Some(p) = part.and_then(|p| world.get::<Part>(p)) {
        name.push(' ');
        name.push_str(&p.id);
    }
    name
}

/// Fire the weapon in your hands: presses the input its shot is bound to (`light`; `alt=true` the
/// alternative fire, `heavy`). Answers like an attack: `ok: test_rifle_shot`, `ok: queued, ...`, or why
/// not (an empty magazine says `reload`; a weapon that reloads itself answers with its reload). Where it
/// will go: `observe` shows a `SHOT` line, the `shot_trace` query the whole ray. `hold=true` keeps the
/// trigger held (a weapon that fires while held), `hold=false` lets go.
#[derive(Deserialize, JsonSchema, Default)]
pub struct FireArgs {
    /// true: the alternative fire (the `heavy` input) instead of the primary (`light`).
    pub alt: Option<bool>,
    /// true presses and keeps the trigger held; false lets go of it (no press). Default: one press.
    pub hold: Option<bool>,
}

fn fire_tool(In(call): In<Call<FireArgs>>, world: &mut World) -> ToolResult {
    let me = crate::tools::me(world, &call.caller)?;
    let input = if call.args.alt == Some(true) { HEAVY } else { LIGHT };
    if call.args.hold != Some(false) {
        let moveset = world.get::<Fighter>(me).map(|f| f.moveset.clone()).unwrap_or_default();
        match bound_fire(world, me, input) {
            None => {
                let db = world.resource::<ContentDb>();
                let inputs = db.peek::<MovesetDef>(&moveset).map(|m| m.inputs.keys().cloned().collect::<Vec<_>>().join(", ")).unwrap_or_default();
                return Err(format!("your moveset `{moveset}` binds nothing to {input}; it has: {inputs}"));
            }
            Some((action, def, None)) if !def.effects.iter().any(|e| matches!(e.effect, Effect::Spawn(_))) => {
                return Err(format!(
                    "your moveset `{moveset}` binds {input} to `{action}`, which fires nothing (no Fire or Spawn effect); a swing is light_attack"
                ));
            }
            Some(_) => {}
        }
    }
    crate::tools::press_held(world, &call.caller, input, None, "", call.args.hold)
}

/// Where a shot would go now and what it would hit, without firing: the centre of the shot (no spread,
/// no aim error) from the shooter's eye along its look (or toward its target), through the bodies it
/// would pass (`pierce`) to what stops it, with the share of the damage each keeps by its distance. The
/// numbers are those of the `Fire` of the action bound to `input` (default `light`) in the shooter's
/// moveset; without one, a ray of 100 m that stops at the first body.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ShotTraceArgs {
    /// Whose shot (default: your body).
    pub id: Option<Id>,
    /// The input whose action's shot to trace (default `light`; the alternative fire is `heavy`).
    pub input: Option<String>,
}

fn shot_trace(In(call): In<Call<ShotTraceArgs>>, world: &World) -> ToolResult<Value> {
    let shooter = match call.args.id {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let input = call.args.input.as_deref().unwrap_or(LIGHT);
    let aim = shot_aim(world, shooter).ok_or("it has no position to shoot from")?;
    let bound = bound_fire(world, shooter, input);
    let fire = bound.and_then(|(_, _, f)| f);
    let (range, pierce) = fire.map_or((DEFAULT_RANGE, 0), |f| (f.range, f.pierce));
    let path = trace_shot(world, shooter, aim.from, aim.dir, range, pierce);
    let round = |v: f32| (v * 100.0).round() / 100.0;
    let gaze = Gaze::toward(Vec3::ZERO, aim.dir).unwrap_or_default();
    let aimed = match aim.by {
        AimedBy::Look => "look".to_owned(),
        AimedBy::Target(t) => format!("target {}", label(world, body_of(world, t))),
        AimedBy::Point => "aim point".to_owned(),
        AimedBy::Facing => "facing".to_owned(),
    };
    let mut said: Vec<String> = Vec::new();
    let hits: Vec<Value> = path
        .bodies
        .iter()
        .map(|(body, part, hit)| {
            let share = fire.map_or(1.0, |f| f.share_at(hit.distance));
            said.push(format!("{} {:.1}m (x{share:.2})", body_name(world, *body, *part), hit.distance));
            json!({
                "id": label(world, *body),
                "part": part.and_then(|p| world.get::<Part>(p)).map(|p| p.id.clone()),
                "distance": round(hit.distance),
                "share": round(share),
                "at": ai_core::spatial::fmt_pos(hit.point),
            })
        })
        .collect();
    let stop = path.stop.map(|hit| {
        let name = solid_name(world, hit.entity, hit.normal);
        said.push(format!("stops at {name} {:.1}m", hit.distance));
        json!({ "what": name, "distance": round(hit.distance), "at": ai_core::spatial::fmt_pos(hit.point) })
    });
    if path.bodies.is_empty() && path.stop.is_none() {
        said.push(format!("nothing within {}m", crate::def::meters(range)));
    }
    Ok(json!({
        "shooter": label(world, shooter),
        "action": bound.map(|(a, _, _)| a),
        "fires": fire.is_some(),
        "aimed": aimed,
        "from": ai_core::spatial::fmt_pos(aim.from),
        "yaw": round(gaze.yaw.to_degrees()),
        "pitch": round(gaze.pitch.to_degrees()),
        "range": round(range),
        "pierce": pierce,
        "pellets": fire.map_or(1, |f| f.pellets),
        "spread_deg": fire.map_or(0.0, |f| f.spread_deg),
        "hits": hits,
        "stop": stop,
        "line": said.join(", then "),
    }))
}

/// The `SHOT` line under `YOU` while the subject's primary fire is a shot: what its centre would hit now
/// (`SHOT test_rifle_shot -> e7 Dummy head 14.2m (x1.00)`, `SHOT test_rifle_shot -> wall 8.0m`).
fn shot_lines(world: &World, viewer: Entity) -> Vec<String> {
    let Some((action, _, Some(fire))) = bound_fire(world, viewer, LIGHT) else { return Vec::new() };
    let Some(aim) = shot_aim(world, viewer) else { return Vec::new() };
    let path = trace_shot(world, viewer, aim.from, aim.dir, fire.range, fire.pierce);
    let mut said: Vec<String> = path
        .bodies
        .iter()
        .map(|(body, part, hit)| format!("{} {:.1}m (x{:.2})", body_name(world, *body, *part), hit.distance, fire.share_at(hit.distance)))
        .collect();
    if let Some(hit) = path.stop {
        said.push(format!("{} {:.1}m", solid_name(world, hit.entity, hit.normal), hit.distance));
    }
    if said.is_empty() {
        said.push(format!("nothing within {}m", crate::def::meters(fire.range)));
    }
    vec![format!("SHOT {action} -> {}", said.join(", then "))]
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<ShotFired>()
        .register_type::<ShotImpact>()
        .init_resource::<Shooting>()
        .add_systems(FixedLast, forget_shots)
        .add_ai_tool("fire", Scope::Player, fire_tool)
        .add_ai_query("shot_trace", Scope::Player, shot_trace)
        .register_ai_observe_lines(shot_lines);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fire(falloff: &[(f32, f32)]) -> FireDef {
        ron::from_str::<FireDef>("(damage: 10)").map(|f| FireDef { falloff: falloff.to_vec(), ..f }).unwrap()
    }

    #[test]
    fn the_defaults_are_one_ray_of_100_m() {
        let f = fire(&[]);
        assert_eq!((f.pellets, f.spread_deg, f.range, f.pierce, f.noise, f.kick), (1, 0.0, DEFAULT_RANGE, 0, 0.0, (0.0, 0.0)));
        assert!(f.problems().is_empty(), "{:?}", f.problems());
        assert_eq!(serde_json::to_string(&f).unwrap(), r#"{"damage":10.0}"#, "defaults stay out of the written form");
    }

    #[test]
    fn falloff_interpolates_between_its_points_and_holds_past_them() {
        let f = fire(&[(10.0, 1.0), (30.0, 0.5)]);
        assert_eq!(f.share_at(0.0), 1.0);
        assert_eq!(f.share_at(10.0), 1.0);
        assert!((f.share_at(20.0) - 0.75).abs() < 1e-6);
        assert_eq!(f.share_at(30.0), 0.5);
        assert_eq!(f.share_at(300.0), 0.5);
        assert_eq!(fire(&[]).share_at(1000.0), 1.0);
    }

    #[test]
    fn bad_numbers_are_named_by_field() {
        let bad = FireDef { pellets: 0, spread_deg: 95.0, range: -1.0, falloff: vec![(10.0, 0.5), (5.0, 0.8)], ..fire(&[]) };
        let problems = bad.problems().join("\n");
        for needle in ["pellets must be at least 1", "spread_deg must be within 0..90", "range must be a number of metres above 0", "falloff is not monotonic"] {
            assert!(problems.contains(needle), "{needle} in {problems}");
        }
        let rising = fire(&[(5.0, 0.5), (10.0, 0.8)]);
        assert!(rising.problems().join(" ").contains("the share rises"), "{:?}", rising.problems());
        let sight = FireDef { flags: vec![HitFlag::LineOfSight], ..fire(&[]) };
        assert!(sight.problems().join(" ").contains("LineOfSight"));
    }

    #[test]
    fn a_cone_draw_stays_inside_its_angle_and_repeats_with_its_keys() {
        let axis = Vec3::new(0.3, -0.2, -1.0).normalize();
        let mut widest = 0.0f32;
        for k in 0..2000u64 {
            let d = cone_dir(axis, 6.0, &mut DetRng::from_keys(&[1, 2, k]));
            assert!((d.length() - 1.0).abs() < 1e-5);
            let off = d.angle_between(axis).to_degrees();
            assert!(off <= 6.0 + 1e-3, "{off}");
            widest = widest.max(off);
            assert_eq!(d, cone_dir(axis, 6.0, &mut DetRng::from_keys(&[1, 2, k])), "the same keys, the same pellet");
        }
        assert!(widest > 5.5, "the draws reach the edge of the cone: {widest}");
        assert!(cone_dir(axis, 0.0, &mut DetRng::new(3)).abs_diff_eq(axis, 1e-6), "no spread: along the aim");
    }

    #[test]
    fn the_content_check_names_the_file_the_action_and_the_field() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<crate::status::StatusDef>();
        kinds.register::<shards_content::PrefabDef>();
        let actions = r#"{
            "bad": (net_id: 1, phases: [("a", 0.3)], effects: [(at: 0.0, effect: Fire((pellets: 0, spread_deg: 120, range: -5,
                    falloff: [(20, 0.5), (10, 1.0)], damage: 5, status: [("nope", 10)])))]),
            "aimless": (net_id: 2, phases: [("a", 0.3)], effects: [(at: 0.0, effect: Spawn((prefab: ["p"], action: ["bad"], aim_error_deg: 5)))]),
            "fine": (net_id: 3, phases: [("a", 0.3)], effects: [(at: 0.0, effect: Spawn((prefab: ["p"], action: ["bad"], aim: Look, aim_error_deg: 5)))]),
        }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/p.prefab.ron".into(), text: r#"{"p": (components: {})}"#.into() },
        ];
        let errors: Vec<String> = ContentDb::build(&kinds, &sources).errors().map(|e| e.to_string()).collect();
        let bad = "content/a.action.ron: action `bad`: effects[0] Fire ";
        for line in [
            format!("{bad}pellets must be at least 1"),
            format!("{bad}spread_deg must be within 0..90 degrees, the cone's half angle (is 120)"),
            format!("{bad}range must be a number of metres above 0 (is -5)"),
            format!("{bad}falloff is not monotonic: falloff[1] at 10 m does not come after falloff[0] at 20 m; give the points by ascending distance"),
            format!("{bad}falloff is not monotonic: the share rises from 0.5 at 20 m to 1 at 10 m (falloff[1]); a falloff only falls"),
            "content/a.action.ron: action `aimless`: effects[0] Spawn.aim_error_deg applies to `aim: Look` only (the aim is Facing); remove it or aim by Look".to_owned(),
        ] {
            assert!(errors.contains(&line), "missing `{line}` in {errors:#?}");
        }
        assert!(errors.iter().any(|e| e.starts_with(bad) && e.contains("status") && e.contains("nope")), "{errors:#?}");
        assert!(!errors.iter().any(|e| e.contains("`fine`")), "{errors:#?}");
    }
}
