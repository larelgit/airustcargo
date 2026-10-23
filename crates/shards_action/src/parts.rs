//! Body parts (R1): one body hit through several volumes, each with its own numbers and, when it
//! has some, its own health that breaks it.
//!
//! A body names its definition with [`BodyParts`] (`"BodyParts": ("colossus")` in a prefab); the
//! content kind `parts` (`*.parts.ron`) lists the parts by id. The engine spawns one entity per part
//! on the first tick the body is in the world ([`SpawnedParts`] holds their ids), top-level with an
//! `AiId`, a hurtbox collider and a [`Part`]; every tick it puts each part where the definition says,
//! in the body's frame (`at` from the body's feet, turned with the body). A part has no bone: bones
//! are posed only in a window, and the simulation must give the same hits without one.
//!
//! A hit that overlaps a part is a hit on the body ([`DamageCtx::part`](crate::DamageCtx::part)
//! names the part): the body's guard, i-frames, armour and reactions apply, and one swing hits a
//! body once, through the part nearest the middle of the hit among those it overlaps (half way up a
//! cone's axis; for a projectile, the start of the segment it flew this tick). The part's
//! `damage` and `poise` factors scale the hit, its tags join the hit's; the body loses
//! `damage * to_body`, the part `damage`. A part whose health runs out breaks ([`PartBroken`]):
//! it stops taking hits. What a break means (a wound, debris, an attack the body no longer has) is
//! the game's. [`set_part`] turns a part on or off (a weak spot that opens), [`break_part`] breaks
//! one; the dev tool `part` does both.
//!
//! **Lock points (R3).** A part with `lock: true` is a lock-on target of its own (`LockOnTarget` and
//! `LockPoint` on its entity until it breaks); with `lock_body: false` the body itself is taken only
//! through its points. The lock-on rules are `crate::lock`'s; [`body_of`] is how the rest of the
//! module set (orders, pairs, leaps, spawns) sees the body behind a point.
//!
//! ```ron
//! // content/combat/colossus.parts.ron
//! {
//!     "colossus": (
//!         body_hurtbox: false,
//!         lock_body: false,
//!         parts: {
//!             "core": (shape: Sphere(radius: 0.6), at: (forward: 0.9, up: 2.0), damage: 3.0, tags: ["weak"], enabled: false, lock: true),
//!             "hand": (shape: Sphere(radius: 0.5), at: (forward: 1.4, up: 1.2, right: 1.0), health: Some(60), to_body: 0.5, lock: true),
//!             "leg_left": (shape: Capsule(half_height: 0.8, radius: 0.45), at: (forward: 0.3, up: 1.2, right: -0.7), poise: 2.0, lock: true),
//!         },
//!     ),
//! }
//! ```

use std::collections::BTreeMap;

use ai_core::prelude::*;
use ai_core::{label, AiIndex, NoTombstone};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use shards_content::{ContentAppExt, ContentDb, ContentDef, PrefabDef, Refs};
use shards_core::{Intent, NetRole};
use shards_motion::{Body, Collider, CollisionLayers, Crouch, LayerMask, LockOnTarget, LockPoint, Shape};

use crate::def::{Local, EPS};

/// At most this many parts on one body.
pub const MAX_PARTS: usize = 16;

/// Content kind `parts`: the parts of one kind of body, by id.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PartsDef {
    /// The parts by id: spawned in this order (by id), so their `AiId`s follow it.
    pub parts: BTreeMap<String, PartDef>,
    /// Whether the body's own collider still takes hits beside its parts; `false`: a hit lands only
    /// through a part (a giant whose capsule is far bigger than its limbs).
    #[serde(skip_serializing_if = "is_true")]
    pub body_hurtbox: bool,
    /// Whether the lock-on may take the body itself beside its lock points (parts with `lock`);
    /// `false`: only its points (a giant locked at its core or a limb, never at its feet).
    #[serde(skip_serializing_if = "is_true")]
    pub lock_body: bool,
}

impl Default for PartsDef {
    fn default() -> Self {
        Self { parts: BTreeMap::new(), body_hurtbox: true, lock_body: true }
    }
}

/// One part: a hurtbox placed in the body's frame and its numbers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PartDef {
    /// Its volume, centred on `at`.
    pub shape: Shape,
    /// Where its centre is, from the body's feet: meters forward (where the body faces), up and to
    /// the right.
    #[serde(default, skip_serializing_if = "is_zero_local")]
    pub at: Local,
    /// Where its centre is when the body is fully crouched (F2), in the same frame as `at`; while the
    /// body goes down or up it moves in a straight line between the two by how deep the crouch is, so a
    /// head stays on the shoulders of a body whose capsule got shorter. `None`: the same as `at` (a part
    /// that does not move with the stance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_crouched: Option<Local>,
    /// Its own turn in the body's frame, in degrees (a capsule lies along the body's forward with
    /// `pitch: 90`).
    #[serde(default, skip_serializing_if = "is_zero_turn")]
    pub turn: PartTurn,
    /// Its own health; `None`: it never breaks (a leg that only takes the body's balance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<f32>,
    /// Scales the damage of a hit through it (a weak spot above 1, an armoured plate below).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub damage: f32,
    /// The share of that damage the body loses (the part loses all of it).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub to_body: f32,
    /// Scales the poise damage of a hit through it (a leg that topples above 1).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub poise: f32,
    /// Added to the tags of a hit through it, for the game's damage hooks (`weak`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Whether it takes hits from the start; `false`: only once [`set_part`] turns it on.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// A lock point (R3): the lock-on may take it as a target of its own, and the camera frames
    /// it. It stays one while it is off (a weak spot that closes keeps the lock) and stops being
    /// one when it breaks. With `enabled: false` it is a pure aim point that is never hit.
    #[serde(default, skip_serializing_if = "is_false")]
    pub lock: bool,
}

/// A part's own turn in the body's frame, in degrees: `pitch` about the body's right, `yaw` about
/// up, `roll` about forward, in that order of `yaw`, `pitch`, `roll`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct PartTurn {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl PartTurn {
    fn rotation(self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw.to_radians(), self.pitch.to_radians(), self.roll.to_radians())
    }
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero_local(v: &Local) -> bool {
    *v == Local::default()
}

fn is_zero_turn(v: &PartTurn) -> bool {
    *v == PartTurn::default()
}

impl PartsDef {
    /// What is wrong with the definition, one line each.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.parts.is_empty() {
            out.push("no parts: a body with parts has at least one".to_owned());
        }
        if self.parts.len() > MAX_PARTS {
            out.push(format!("{} parts: at most {MAX_PARTS}", self.parts.len()));
        }
        for (id, p) in &self.parts {
            if id.is_empty() || id.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '_')) {
                out.push(format!("part `{id}`: an id is letters, digits and `_`"));
            }
            let half = p.shape.half_extents();
            if !(half.is_finite() && half.min_element() > 0.0) {
                out.push(format!("part `{id}`: shape {:?} needs sizes above 0", p.shape));
            }
            let at = p.at.to_vec3();
            let turn = Vec3::new(p.turn.pitch, p.turn.yaw, p.turn.roll);
            if !(at.is_finite() && turn.is_finite() && p.at_crouched.is_none_or(|c| c.to_vec3().is_finite())) {
                out.push(format!("part `{id}`: `at`, `at_crouched` and `turn` must be finite"));
            }
            if let Some(h) = p.health
                && !(h > 0.0 && h.is_finite())
            {
                out.push(format!("part `{id}`: health {h} must be above 0 (leave it out for a part that never breaks)"));
            }
            for (name, v) in [("damage", p.damage), ("to_body", p.to_body), ("poise", p.poise)] {
                if !(v >= 0.0 && v.is_finite()) {
                    out.push(format!("part `{id}`: {name} {v} must be a finite factor >= 0"));
                }
            }
            if p.tags.iter().any(|t| t.trim().is_empty()) {
                out.push(format!("part `{id}`: an empty tag"));
            }
        }
        if !self.lock_body && !self.parts.values().any(|p| p.lock) {
            out.push("`lock_body: false` and no part with `lock: true`: nothing to lock on to".to_owned());
        }
        out
    }
}

impl ContentDef for PartsDef {
    const KIND: &'static str = "parts";

    fn validate(&self, _id: &str, refs: &mut Refs) {
        for problem in self.problems() {
            refs.error(problem);
        }
    }
}

/// A body made of parts: names its `parts` definition. The engine spawns the parts on the first
/// tick the body is in the world and despawns them when it goes.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[require(AiVisible)]
pub struct BodyParts(pub String);

/// The ids of the parts the engine spawned for a [`BodyParts`] body, in the definition's order.
/// Managed by the engine (a snapshot keeps it, so a loaded body does not spawn its parts again).
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct SpawnedParts(pub Vec<AiId>);

/// One part of a body ([`BodyParts`]): a hurtbox the engine places on the body every tick.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[require(NoTombstone)]
pub struct Part {
    /// The body it belongs to.
    pub body: AiId,
    /// Its id in the body's definition.
    pub id: String,
    /// It takes hits (its collider is a hurtbox); a broken part never does.
    pub on: bool,
    pub broken: bool,
}

/// A part's own health ([`PartDef::health`]); a part without one never breaks.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct PartHealth {
    pub current: f32,
    pub max: f32,
}

/// A part broke: its health ran out under a hit (`by` is the credited attacker), or the game or a
/// tool broke it ([`break_part`]). It takes no more hits.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct PartBroken {
    pub body: Entity,
    pub part: Entity,
    /// Its id in the body's definition.
    pub id: String,
    pub by: Option<Entity>,
}

/// What a hit knows of the part it landed on ([`DamageCtx::part`](crate::DamageCtx::part)).
#[derive(Clone, Debug, PartialEq)]
pub struct PartHit {
    /// The part's entity.
    pub entity: Entity,
    /// Its id in the body's definition.
    pub id: String,
    /// [`PartDef::damage`], already in the hit's `damage`.
    pub damage: f32,
    /// [`PartDef::to_body`]: the body loses `damage * to_body`.
    pub to_body: f32,
    /// [`PartDef::poise`], already in the hit's `poise`.
    pub poise: f32,
}

fn simulates(world: &World) -> bool {
    world.get_resource::<NetRole>().is_none_or(|r| r.simulates())
}

fn layers(on: bool) -> CollisionLayers {
    CollisionLayers::new(if on { LayerMask::HURTBOX } else { LayerMask::NONE }, LayerMask::NONE)
}

/// Where a part is: `at` from the body's feet in the body's frame, turned with the body. `depth` is how
/// far into a crouch the body is (0 standing, 1 crouched): a part with an `at_crouched` is that far
/// along the line from `at` to it.
fn place(body: &Transform, def: &PartDef, depth: f32) -> Transform {
    let at = match def.at_crouched {
        Some(crouched) if depth > 0.0 => def.at.to_vec3().lerp(crouched.to_vec3(), depth.min(1.0)),
        _ => def.at.to_vec3(),
    };
    Transform {
        translation: body.translation + body.rotation * at,
        rotation: body.rotation * def.turn.rotation(),
        scale: Vec3::ONE,
    }
}

/// The definition of `body`'s part `id`.
fn part_def<'a>(db: &'a ContentDb, world: &World, body: Entity, id: &str) -> Option<&'a PartDef> {
    db.get::<PartsDef>(&world.get::<BodyParts>(body)?.0)?.parts.get(id)
}

/// `SimSet::Move`, after the controller: every [`BodyParts`] body without its parts yet gets them,
/// bodies in `AiId` order, parts in the definition's. A machine that does not simulate (a co-op
/// client) spawns none: the host's state brings them.
pub(crate) fn spawn_parts(world: &mut World) {
    if !simulates(world) {
        return;
    }
    let Some(mut q) = world.try_query_filtered::<(Entity, &AiId, &BodyParts), Without<SpawnedParts>>() else { return };
    let mut bodies: Vec<(AiId, Entity, String)> = q.iter(world).map(|(e, id, p)| (*id, e, p.0.clone())).collect();
    if bodies.is_empty() {
        return;
    }
    bodies.sort_by_key(|b| b.0);
    for (body_id, body, def_id) in bodies {
        // A definition that is missing is a content error (the prefab check names it); the body waits for it.
        let Some(def) = world.resource::<ContentDb>().get::<PartsDef>(&def_id).cloned() else { continue };
        let Some(body_tf) = world.get::<Transform>(body).copied() else { continue };
        let mut ids = Vec::with_capacity(def.parts.len());
        for (id, p) in &def.parts {
            let mut e = world.spawn((
                Name::new(id.clone()),
                AiKind::new("Part"),
                Part { body: body_id, id: id.clone(), on: p.enabled, broken: false },
                place(&body_tf, p, 0.0),
                Collider::new(p.shape).with_layers(layers(p.enabled)),
                Body::Kinematic,
            ));
            if let Some(h) = p.health {
                e.insert(PartHealth { current: h, max: h });
            }
            if p.lock {
                e.insert((LockOnTarget, LockPoint));
            }
            ids.push(*e.get::<AiId>().expect("AiKind requires an AiId"));
        }
        world.entity_mut(body).insert(SpawnedParts(ids));
    }
}

/// `SimSet::Move`, after [`spawn_parts`]: every part, in `AiId` order, goes where its definition
/// puts it on its body; a part whose body is gone (or no longer lists it) goes too, on the machine that
/// simulates (a co-op client leaves that to the host's state: a part can arrive before its body, or
/// stay in view while the body is out of it). A part whose definition is gone (a hot reload took it
/// out) stays where it was.
#[allow(clippy::type_complexity, reason = "a Bevy query")]
pub(crate) fn place_parts(
    index: Res<AiIndex>,
    db: Res<ContentDb>,
    role: Option<Res<NetRole>>,
    bodies: Query<(&Transform, &BodyParts, Option<&SpawnedParts>, Option<&Crouch>), Without<Part>>,
    mut parts: Query<(Entity, &AiId, &Part, &mut Transform)>,
    mut commands: Commands,
) {
    if parts.is_empty() {
        return;
    }
    let mut order: Vec<(AiId, Entity)> = parts.iter().map(|(e, id, ..)| (*id, e)).collect();
    order.sort();
    for (id, entity) in order {
        let Ok((_, _, part, mut tf)) = parts.get_mut(entity) else { continue };
        let body = index.get(part.body).and_then(|b| bodies.get(b).ok());
        let Some((body_tf, kind, _, crouch)) = body.filter(|(_, _, s, _)| s.is_some_and(|s| s.0.contains(&id))) else {
            if role.as_ref().is_none_or(|r| r.simulates()) {
                commands.entity(entity).despawn();
            }
            continue;
        };
        let Some(def) = db.get::<PartsDef>(&kind.0).and_then(|d| d.parts.get(&part.id)) else { continue };
        // A crouched body's parts follow its stance, a head with it (`at_crouched`).
        let want = place(body_tf, def, crouch.map_or(0.0, |c| c.depth));
        if *tf != want {
            *tf = want;
        }
    }
}

/// The bodies a hit's overlaps stand for, one entry each, with the part it landed on: a part
/// that is on stands for its body (among a body's parts the hit overlaps, the one whose centre is
/// nearest `near`, ties to the lower `AiId`; `prefer`, the part the attacker is locked on to, when
/// the hit overlaps it), and a body whose definition says `body_hurtbox: false` is hit only through
/// a part. In no particular order.
pub(crate) fn hit_bodies(world: &World, overlapping: Vec<Entity>, near: Vec3, prefer: Option<Entity>) -> Vec<(Entity, Option<Entity>)> {
    struct Seen {
        body: Entity,
        own: bool,
        part: Option<(f32, u32, Entity)>,
    }
    let index = world.get_resource::<AiIndex>();
    let mut seen: Vec<Seen> = Vec::new();
    for e in overlapping {
        let (body, part) = match world.get::<Part>(e) {
            Some(p) => {
                if !p.on || p.broken {
                    continue;
                }
                let Some(body) = index.and_then(|i| i.get(p.body)) else { continue };
                let distance = match Some(e) == prefer {
                    true => f32::NEG_INFINITY,
                    false => world.get::<Transform>(e).map_or(f32::MAX, |t| t.translation.distance_squared(near)),
                };
                (body, Some((distance, world.get::<AiId>(e).map_or(u32::MAX, |i| i.0), e)))
            }
            None => (e, None),
        };
        let at = match seen.iter().position(|s| s.body == body) {
            Some(i) => i,
            None => {
                seen.push(Seen { body, own: false, part: None });
                seen.len() - 1
            }
        };
        let entry = &mut seen[at];
        match part {
            None => entry.own = true,
            Some(p) => {
                let nearer = entry.part.is_none_or(|q| p.0.total_cmp(&q.0).then(p.1.cmp(&q.1)).is_lt());
                if nearer {
                    entry.part = Some(p);
                }
            }
        }
    }
    let db = world.get_resource::<ContentDb>();
    seen.into_iter()
        .filter(|s| s.part.is_some() || (s.own && body_hurtbox(world, db, s.body)))
        .map(|s| (s.body, s.part.map(|p| p.2)))
        .collect()
}

/// Whether `body`'s own collider takes hits: always, unless its parts definition says no.
pub(crate) fn body_hurtbox(world: &World, db: Option<&ContentDb>, body: Entity) -> bool {
    let def = world.get::<BodyParts>(body).zip(db).and_then(|(p, db)| db.get::<PartsDef>(&p.0));
    def.is_none_or(|d| d.body_hurtbox)
}

/// What a hit on `body` through `part` takes from the part's definition, with its tags; `None`
/// when the part is not `body`'s or no longer on.
pub(crate) fn part_hit(world: &World, body: Entity, part: Entity) -> Option<(PartHit, Vec<String>)> {
    let p = world.get::<Part>(part).filter(|p| p.on && !p.broken)?;
    if world.get::<AiId>(body) != Some(&p.body) {
        return None;
    }
    let def = part_def(world.resource::<ContentDb>(), world, body, &p.id)?;
    let hit = PartHit { entity: part, id: p.id.clone(), damage: def.damage, to_body: def.to_body, poise: def.poise };
    Some((hit, def.tags.clone()))
}

/// Takes `damage` off a part's own health; whether that broke it (it had health, was on and not
/// broken, and has none left). A part without health takes nothing.
pub(crate) fn hurt_part(world: &mut World, part: Entity, damage: f32) -> bool {
    if damage.is_nan() || damage <= 0.0 || world.get::<Part>(part).is_none_or(|p| !p.on || p.broken) {
        return false;
    }
    let Some(mut health) = world.get_mut::<PartHealth>(part) else { return false };
    health.current = (health.current - damage).max(0.0);
    health.current <= EPS
}

/// Breaks the part entity `part`: it is off for good, its health 0, its collider no hurtbox, and
/// [`PartBroken`] is triggered. Nothing for a part that is broken already.
pub(crate) fn break_part_entity(world: &mut World, part: Entity, by: Option<Entity>) {
    let Some(p) = world.get::<Part>(part).filter(|p| !p.broken).cloned() else { return };
    let Some(body) = world.get_resource::<AiIndex>().and_then(|i| i.get(p.body)) else { return };
    if let Some(mut state) = world.get_mut::<Part>(part) {
        state.on = false;
        state.broken = true;
    }
    if let Some(mut health) = world.get_mut::<PartHealth>(part) {
        health.current = 0.0;
    }
    if let Some(mut collider) = world.get_mut::<Collider>(part) {
        collider.layers = layers(false);
    }
    // No lock point any more: a lock on it is let go of by the lock check (`lock released`).
    world.entity_mut(part).remove::<(LockOnTarget, LockPoint)>();
    world.trigger(PartBroken { body, part, id: p.id, by });
}

/// The body `entity` stands for: its body when it is a part (and the body is there), else itself.
/// What the lock-on names may be a part (R3); what fights, dies and pairs is its body.
pub fn body_of(world: &World, entity: Entity) -> Entity {
    world
        .get::<Part>(entity)
        .and_then(|p| world.get_resource::<AiIndex>()?.get(p.body))
        .unwrap_or(entity)
}

/// Whether the lock-on may take `body` itself: not when its parts definition says `lock_body: false`
/// (only its lock points then).
pub fn body_takes_lock(world: &World, body: Entity) -> bool {
    let Some(parts) = world.get::<BodyParts>(body) else { return true };
    world.get_resource::<ContentDb>().and_then(|db| db.get::<PartsDef>(&parts.0)).is_none_or(|d| d.lock_body)
}

/// The ids of `body`'s lock points that are not broken, in the definition's order.
pub fn lock_points_of(world: &World, body: Entity) -> Vec<String> {
    parts_of(world, body)
        .into_iter()
        .filter(|p| world.get::<LockOnTarget>(*p).is_some())
        .filter_map(|p| world.get::<Part>(p).map(|q| q.id.clone()))
        .collect()
}

/// The part entity `id` of `body`, or why there is none (the error lists the body's parts).
pub fn part_of(world: &World, body: Entity, id: &str) -> Result<Entity, String> {
    let who = label(world, body);
    let Some(spawned) = world.get::<SpawnedParts>(body) else {
        return Err(match world.get::<BodyParts>(body) {
            Some(_) => format!("{who}'s parts are not spawned yet (they come on its first tick)"),
            None => format!("{who} has no parts"),
        });
    };
    let index = world.resource::<AiIndex>();
    let parts: Vec<(Entity, &Part)> =
        spawned.0.iter().filter_map(|id| index.get(*id)).filter_map(|e| world.get::<Part>(e).map(|p| (e, p))).collect();
    match parts.iter().find(|(_, p)| p.id == id) {
        Some((e, _)) => Ok(*e),
        None => {
            let names: Vec<&str> = parts.iter().map(|(_, p)| p.id.as_str()).collect();
            Err(format!("{who} has no part `{id}`; its parts: {}", names.join(", ")))
        }
    }
}

/// `body`'s part entities, in the definition's order (empty for a body without parts).
pub fn parts_of(world: &World, body: Entity) -> Vec<Entity> {
    let Some(spawned) = world.get::<SpawnedParts>(body) else { return Vec::new() };
    let index = world.resource::<AiIndex>();
    spawned.0.iter().filter_map(|id| index.get(*id)).collect()
}

/// Turns `body`'s part `id` on (it takes hits) or off. A broken part stays off.
pub fn set_part(world: &mut World, body: Entity, id: &str, on: bool) -> Result<(), String> {
    let part = part_of(world, body, id)?;
    let p = world.get::<Part>(part).expect("part_of finds parts");
    if p.broken && on {
        return Err(format!("{} {id} is broken; a broken part stays off", label(world, body)));
    }
    if p.on != on {
        world.get_mut::<Part>(part).expect("checked").on = on;
        if let Some(mut collider) = world.get_mut::<Collider>(part) {
            collider.layers = layers(on);
        }
    }
    Ok(())
}

/// Breaks `body`'s part `id` as if its health ran out ([`PartBroken`], with `by` as the breaker).
pub fn break_part(world: &mut World, body: Entity, id: &str, by: Option<Entity>) -> Result<(), String> {
    let part = part_of(world, body, id)?;
    if world.get::<Part>(part).is_some_and(|p| p.broken) {
        return Err(format!("{} {id} is broken already", label(world, body)));
    }
    break_part_entity(world, part, by);
    Ok(())
}

/// `horn of e12 30/80 hp`, `core of e12 off`, `horn of e12 broken`.
fn describe_part(e: EntityRef) -> Option<String> {
    let p = e.get::<Part>()?;
    let mut line = format!("{} of {}", p.id, p.body);
    if let Some(h) = e.get::<PartHealth>() {
        line.push_str(&format!(" {:.0}/{:.0} hp", h.current, h.max));
    }
    if p.broken {
        line.push_str(" broken");
    } else if !p.on {
        line.push_str(" off");
    }
    Some(line)
}

/// A prefab's `BodyParts` names a parts definition some file defines.
fn check_prefab(prefab: &PrefabDef, _id: &str, refs: &mut Refs) {
    let is = |refs: &Refs, name: &str, ty: std::any::TypeId, short: &str| match refs.type_registry() {
        Some(registry) => shards_content::prefab::resolve_component(registry, name).is_ok_and(|reg| reg.type_id() == ty),
        None => name == short,
    };
    let marked = prefab.components.keys().any(|name| is(refs, name, std::any::TypeId::of::<LockOnTarget>(), "LockOnTarget"));
    for (name, value) in &prefab.components {
        // A value that does not read is the prefab's own error.
        if is(refs, name, std::any::TypeId::of::<BodyParts>(), "BodyParts")
            && let Ok(parts) = ron::from_str::<BodyParts>(value.get_ron())
            && refs.require::<PartsDef>(&parts.0, "BodyParts")
            && marked
            && refs.get::<PartsDef>(&parts.0).is_some_and(|d| !d.lock_body)
        {
            refs.error(format!(
                "LockOnTarget on a body whose parts `{}` say `lock_body: false`: the lock-on takes only its lock points; leave the marker out",
                parts.0
            ));
        }
    }
}

/// What the dev tool `part` does to a part.
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PartOp {
    /// It takes hits.
    On,
    /// It takes none.
    Off,
    /// It breaks, as if its health ran out.
    Break,
}

/// Turn a body's part on or off, or break it (dev). `part id=e12 name=core op=on`; without `id`,
/// the body you are locked on. A broken part stays off.
#[derive(Deserialize, JsonSchema)]
pub struct PartArgs {
    /// The body, or one of its parts (default: the one you are locked on, or the body of the lock
    /// point you are locked on).
    pub id: Option<Id>,
    /// The part's id in the body's definition (`observe` lists them: `horn of e12`).
    pub name: String,
    pub op: PartOp,
}

fn part_tool(In(call): In<Call<PartArgs>>, world: &mut World) -> ToolResult {
    let who = match call.args.id {
        Some(id) => id,
        None => call
            .caller
            .subject
            .and_then(|me| world.get::<Intent>(me)?.target)
            .map(Id)
            .ok_or("you are locked on to nothing; pass `id`")?,
    };
    // A lock on a lock point (R3) stands for its body, as an `id` naming a part does.
    let body = body_of(world, who.entity(world)?);
    let who = world.get::<AiId>(body).map_or(who, |id| Id(*id));
    let name = call.args.name;
    match call.args.op {
        PartOp::On => set_part(world, body, &name, true).map(|()| format!("{who} {name} on")),
        PartOp::Off => set_part(world, body, &name, false).map(|()| format!("{who} {name} off")),
        PartOp::Break => break_part(world, body, &name, None).map(|()| format!("{who} {name} broken")),
    }
}

pub(crate) fn plugin(app: &mut App) {
    use crate::ActionSystems;
    app.register_type::<BodyParts>()
        .register_type::<SpawnedParts>()
        .register_type::<Part>()
        .register_type::<PartHealth>()
        .register_type::<PartBroken>()
        .add_content::<PartsDef>()
        .add_content_check::<PrefabDef>("parts", check_prefab);
    let world = app.world_mut();
    // Registered up front: the rule `action.parts_sane` queries them through `with_query`, which
    // sees nothing while a component in its query is unknown to the world.
    world.register_component::<BodyParts>();
    world.register_component::<SpawnedParts>();
    world.register_component::<Part>();
    world.register_component::<PartHealth>();
    app.add_systems(
        FixedUpdate,
        (spawn_parts, place_parts)
            .chain()
            .in_set(SimSet::Move)
            .after(crate::objects::move_projectiles)
            .before(ActionSystems::Hits),
    )
    .register_ai_describer("part", describe_part)
    .add_ai_tool("part", Scope::Dev, part_tool)
    .journal_event::<PartBroken>(|e, w| {
        Some(match e.by {
            Some(by) => format!("{} {} broken by {}", label(w, e.body), e.id, label(w, by)),
            None => format!("{} {} broken", label(w, e.body), e.id),
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(parts: &[(&str, PartDef)]) -> PartsDef {
        PartsDef { parts: parts.iter().map(|(id, p)| ((*id).to_owned(), p.clone())).collect(), ..default() }
    }

    fn sphere() -> PartDef {
        ron::from_str("(shape: Sphere(radius: 0.5))").unwrap()
    }

    #[test]
    fn a_part_reads_with_its_defaults() {
        let p = sphere();
        assert_eq!((p.damage, p.to_body, p.poise, p.enabled, p.health), (1.0, 1.0, 1.0, true, None));
        assert_eq!(p.at, Local::default());
        let written = ron::to_string(&p).unwrap();
        assert_eq!(written, "(shape:Sphere(radius:0.5))", "defaults are not written");
    }

    #[test]
    fn bad_numbers_and_ids_are_named() {
        let mut bad = sphere();
        bad.health = Some(0.0);
        bad.poise = -1.0;
        bad.shape = Shape::Sphere { radius: 0.0 };
        let problems = def(&[("left arm", bad)]).problems();
        let all = problems.join("\n");
        for needle in ["letters, digits", "health 0", "poise -1", "sizes above 0"] {
            assert!(all.contains(needle), "{needle}: {all}");
        }
        assert!(def(&[]).problems()[0].contains("no parts"));
        assert!(def(&[("horn", sphere())]).problems().is_empty());
    }

    #[test]
    fn lock_points_read_and_a_body_without_any_is_named() {
        let set: PartsDef = ron::from_str("(lock_body: false, parts: {\"hand\": (shape: Sphere(radius: 0.5), lock: true)})").unwrap();
        assert!(!set.lock_body && set.parts["hand"].lock);
        assert!(set.problems().is_empty(), "{:?}", set.problems());
        let written = ron::to_string(&set).unwrap();
        assert!(written.contains("lock_body:false") && written.contains("lock:true"), "{written}");
        let plain = def(&[("hand", sphere())]);
        assert!(plain.lock_body && !plain.parts["hand"].lock, "a body is a target and a part is none by default");
        assert!(!ron::to_string(&plain).unwrap().contains("lock"), "defaults are not written");
        let blind = PartsDef { lock_body: false, ..plain };
        assert!(blind.problems().join("\n").contains("`lock_body: false` and no part with `lock: true`"), "{:?}", blind.problems());
    }

    #[test]
    fn a_part_sits_in_the_body_frame() {
        let mut p = sphere();
        p.at = Local { forward: 2.0, up: 3.0, right: 1.0 };
        // A body at (10, 0, 0) facing east (+X): forward is +X, right is +Z.
        let body = Transform::from_xyz(10.0, 0.0, 0.0).looking_to(Vec3::X, Vec3::Y);
        let at = place(&body, &p, 0.0).translation;
        assert!((at - Vec3::new(12.0, 3.0, 1.0)).length() < 1e-4, "{at}");
    }

    #[test]
    fn a_part_with_a_crouched_place_moves_along_the_line_by_how_deep_the_crouch_is() {
        let head: PartDef = ron::from_str("(shape: Sphere(radius: 0.2), at: (up: 1.6), at_crouched: Some((up: 0.8, forward: 0.2)))").unwrap();
        let body = Transform::from_xyz(1.0, 0.0, 1.0);
        let at = |depth: f32| place(&body, &head, depth).translation;
        assert!(at(0.0).distance(Vec3::new(1.0, 1.6, 1.0)) < 1e-5, "standing: `at`");
        assert!(at(1.0).distance(Vec3::new(1.0, 0.8, 0.8)) < 1e-5, "crouched: `at_crouched` (forward is -Z)");
        assert!(at(0.5).distance(Vec3::new(1.0, 1.2, 0.9)) < 1e-5, "half way: half way");
        assert!(at(7.0).distance(at(1.0)) < 1e-5, "a depth past 1 is a full crouch");
        let still = sphere();
        assert_eq!(place(&body, &still, 1.0), place(&body, &still, 0.0), "a part without `at_crouched` stays where `at` puts it");
        let written = ron::to_string(&head).unwrap();
        assert!(written.contains("at_crouched:Some("), "{written}");
        assert!(!ron::to_string(&still).unwrap().contains("crouched"), "a part without one writes none");
        let mut bad = sphere();
        bad.at_crouched = Some(Local { up: f32::NAN, ..default() });
        assert!(def(&[("head", bad)]).problems().join("\n").contains("must be finite"));
    }

    #[test]
    fn a_pitched_capsule_lies_along_forward() {
        let mut p: PartDef = ron::from_str("(shape: Capsule(half_height: 1.0, radius: 0.3), turn: (pitch: 90))").unwrap();
        p.at = Local::default();
        let body = Transform::default().looking_to(Vec3::NEG_Z, Vec3::Y);
        let axis = place(&body, &p, 0.0).rotation * Vec3::Y;
        assert!(axis.y.abs() < 1e-4 && (axis.z.abs() - 1.0).abs() < 1e-4, "{axis}");
    }
}
