//! Spawned combat objects (B4): the projectiles, ground zones and summons an action lays down
//! for its owner.
//!
//! An action's `Spawn` effect ([`SpawnDef`]) spawns prefabs in a pattern (a fan, a line, radial
//! lines) and gives each one an action to run. The objects are not fighters: they are entities
//! with [`CombatObject`] and an [`ActionState`], advanced by [`run_objects`] in `AiId` order, and
//! their hit windows go through the same detection and damage pipeline as a fighter's, credited to
//! the owner ([`DamageCtx::credit`](crate::DamageCtx)). A [`Projectile`] flies ([`move_projectiles`]),
//! sweeps the segment it flew each tick, stops at walls and runs its flight action's `impact` where
//! it stops; a zone is an object whose action has a hit window on its `active` phase. An object is
//! despawned when its action is over (unless [`SpawnDef::stay`]), when its owner dies or is gone,
//! and by [`despawn_objects_of`] (a boss's phase change).
//!
//! ```text
//! SimSet::Act      ActionSystems::Objects   run_objects: delayed starts, the objects' runs step (effects, telegraphs)
//!                  ActionSystems::Effects   Spawn effects spawn (spawn_objects)
//! SimSet::Move     after the controller     move_projectiles: homing, flight, a wall stops it
//! SimSet::Resolve  ActionSystems::Hits      detect sweeps a projectile's segment; consume_projectiles: impact
//!                  ActionSystems::Deaths    tend_objects: objects whose action is over go
//! ```
//!
//! Runaway spawning has three guards: the content check refuses a cycle of `Spawn` and `impact`
//! edges ([`ActionDef::then`](crate::ActionDef) alone may loop), the caps
//! [`CombatTuning::max_objects`](crate::CombatTuning) and `max_objects_per_owner`, and the
//! [`PrefabCache`](shards_content::PrefabCache) that keeps a spawn from building its prefab from
//! text each time.

use ai_core::journal::{self, JournalKind};
use ai_core::prelude::*;
use ai_core::{AiIndex, SimSeed};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::{flat, Intent, NetRole};
use shards_motion::{LayerMask, PhysicsWorld, QueryFilter};

use crate::context::cut_action;
use crate::def::{ActionDef, CombatTuning, Local, EPS};
use crate::combat::Downed;
use crate::fighter::{report_once, Dead, Fighter, Team};
use crate::machine::{crossed, force_action, step_run, telegraph, ActionEnd, ActionEnded, ActionState, PendingEffects};
use crate::parts::{body_of, Part};
use crate::zone::{hit_zones, yaw};

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn one_u8() -> u8 {
    1
}

fn is_one_u8(v: &u8) -> bool {
    *v == 1
}

fn is_facing(v: &SpawnAim) -> bool {
    *v == SpawnAim::Facing
}

fn is_point(v: &SpawnPattern) -> bool {
    *v == SpawnPattern::default()
}

fn is_origin(v: &Local) -> bool {
    *v == Local::default()
}

/// An action's `Spawn` effect: combat objects laid down for the spawner's owner, `count` of them
/// in `pattern`, each with a prefab and the action it runs.
///
/// ```ron
/// // three bolts in a fan at the lock-on target, the middle one homing
/// Spawn((prefab: ["bolt", "bolt_homing", "bolt"], action: ["bolt_fly"], at: (forward: 0.8, up: 1.2),
///        aim: Target(height: 0.9), count: 3, pattern: Fan(spread: 11)))
/// ```
#[derive(Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpawnDef {
    /// The prefab of each index; the last one repeats (radial lines: by the index on a line).
    pub prefab: Vec<String>,
    /// The action each index runs; the last one repeats (radial lines: by the index on a line).
    pub action: Vec<String>,
    /// Where, in the spawner's frame (forward along its facing): the pattern's centre.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub at: Local,
    #[serde(default, skip_serializing_if = "is_facing")]
    pub aim: SpawnAim,
    /// Objects in the pattern (on each line of a radial one).
    #[serde(default = "one_u8", skip_serializing_if = "is_one_u8")]
    pub count: u8,
    #[serde(default, skip_serializing_if = "is_point")]
    pub pattern: SpawnPattern,
    /// Seconds between index i and i + 1 starting their action (they wait idle meanwhile).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay: f32,
    /// Turn the whole pattern by a random angle in [0, 360) degrees, one `SimRng` draw per spawn.
    #[serde(default, skip_serializing_if = "is_false")]
    pub random_turn: bool,
    /// Put each object on the floor found within this many metres below its point (a miss keeps
    /// the point).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<f32>,
    /// The object stays when its action ends (a summon); by default it is despawned then.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stay: bool,
}

/// Which way a [`SpawnDef`] points.
#[derive(Serialize, Deserialize, Reflect, Clone, Copy, Debug, Default, PartialEq)]
pub enum SpawnAim {
    /// Along the spawner's facing.
    #[default]
    Facing,
    /// Toward the spawner's lock-on target (its `Intent::target`; an object's: the target its
    /// projectile flies at), aiming `height` metres above its feet. No target: nothing is spawned.
    Target { height: f32 },
    /// At the target's feet: the pattern is laid around it, turned like the spawner. No target:
    /// nothing is spawned.
    AtTarget,
}

/// How a [`SpawnDef`] places its objects around its point `at`.
#[derive(Serialize, Deserialize, Reflect, Clone, Copy, Debug, PartialEq)]
pub enum SpawnPattern {
    /// All at `at`, index i turned by `spread * (i - (count - 1) / 2)` degrees (a positive angle
    /// turns left): index 0 is the rightmost, the last the leftmost.
    Fan { spread: f32 },
    /// Index i at `at + forward * (start + step * i)`.
    Line { start: f32, step: f32 },
    /// `lines` lines evenly around (line k turned `k * 360 / lines` degrees left), `count` objects
    /// on each, index j at `start + step * j` along its line; spawned line by line. The index lists
    /// (`prefab`, `action`, `delay`) go by j.
    Radial { lines: u8, start: f32, step: f32 },
}

impl Default for SpawnPattern {
    fn default() -> Self {
        SpawnPattern::Fan { spread: 0.0 }
    }
}

impl SpawnDef {
    /// The objects one spawn lays down: `count`, times `lines` for a radial pattern.
    pub fn total(&self) -> usize {
        let per = self.count as usize;
        match self.pattern {
            SpawnPattern::Radial { lines, .. } => per * lines as usize,
            _ => per,
        }
    }
}

/// The entry of `list` for index `i`; the last one repeats.
fn by_index(list: &[String], i: usize) -> Option<&str> {
    list.get(i).or(list.last()).map(String::as_str)
}

/// An entity an action spawned for its owner. Advanced by [`run_objects`] (unless it is a
/// fighter: a hittable summon is advanced by the machine), credited to `owner`, despawned when its
/// action is over unless `stay`, and when its owner dies or is gone. Inserted by [`spawn_objects`];
/// a combat object leaves no tombstone in the `AiIndex` (a radial cast would flush the real ones).
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[require(ai_core::id::NoTombstone)]
pub struct CombatObject {
    /// Who it belongs to: the fighter whose action spawned it (an object's own spawns belong to
    /// its owner: a puddle from a bolt is the bolt's owner's).
    pub owner: AiId,
    /// Tick it was spawned on (it acts from the next one).
    pub spawned: u64,
    /// Seconds before its action starts ([`SpawnDef::delay`] times its index); 0 once started.
    pub start_in: f32,
    /// The action it starts with.
    pub action: String,
    pub stay: bool,
}

/// A flying combat object (a prefab component): it moves along its direction at `speed`, stops at
/// walls (`LayerMask::WORLD`) and ends at `lifetime`. Its flight action's hit windows sweep the
/// segment it flew in the tick; [`ActionDef::impact`](crate::ActionDef) is the action it runs where
/// it stops.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Projectile {
    /// Metres per second.
    pub speed: f32,
    /// Seconds of flight before it ends, with no impact.
    pub lifetime: f32,
    /// Weight per second of the turn toward its target (0: straight). A dead or downed target is not
    /// followed: the projectile flies on straight.
    pub homing: f32,
    /// Homing aims this far above the target's feet.
    pub aim_height: f32,
    /// `false`: the first target it meets ends it (any outcome: a block, a dodge).
    pub pierce: bool,
}

impl Projectile {
    /// What is wrong with a prefab's flight numbers, if anything: the speed, lifetime, homing and
    /// aim height must be finite and not below 0.
    pub fn problem(&self) -> Option<String> {
        let numbers = [("speed", self.speed), ("lifetime", self.lifetime), ("homing", self.homing), ("aim_height", self.aim_height)];
        numbers.iter().find(|(_, v)| !(v.is_finite() && *v >= 0.0)).map(|(name, v)| format!("{name} = {v} (a number >= 0)"))
    }
}

/// Where the world is between the spawner's body (`base`, at the object's height) and `to`: the
/// point just off the wall, or `None` when the way is clear.
fn wall_between(world: &World, base: Vec3, to: Vec3) -> Option<Vec3> {
    let from = Vec3::new(base.x, to.y, base.z);
    let (dir, len) = (Dir3::new(to - from).ok()?, from.distance(to));
    let filter = QueryFilter::new(LayerMask::WORLD);
    let hit = world.get_resource::<PhysicsWorld>()?.raycast(from, dir, len, &filter)?;
    Some(hit.point + hit.normal * 0.02)
}

/// The flight of one projectile (inserted by [`spawn_objects`], not by a prefab: a prefab must not
/// set where it flies).
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct ProjectileFlight {
    /// Unit direction of travel; `Vec3::ZERO` once it has stopped (a wall, a target).
    pub dir: Vec3,
    /// Seconds flown.
    pub age: f32,
    /// What it flies at (the spawner's lock-on target when it was spawned): homing turns toward it.
    pub target: Option<AiId>,
    /// Where this tick's flight started: the hit windows sweep from here to where it is.
    pub from: Vec3,
}

/// When a repeating hit ([`HitDef::repeat`](crate::HitDef)) may land again on each target, on the
/// attacker; sorted by instance, group and target. Inserted by the first repeating hit; marks of an
/// earlier run go when one of a later run is written.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Repeats(pub Vec<RepeatMark>);

/// One target of a repeating hit: hit again once the run's time reaches `next`.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct RepeatMark {
    /// The run's `Running::instance`.
    pub instance: u32,
    /// The hit's `HitDef::group`.
    pub group: u8,
    pub target: AiId,
    /// `Running::t` from which it may hit this target again.
    pub next: f32,
}

impl Repeats {
    /// Whether a repeating hit of run `instance`, `group`, may hit `target` at run time `t`.
    pub fn allows(&self, instance: u32, group: u8, target: AiId, t: f32) -> bool {
        self.0.iter().find(|m| m.instance == instance && m.group == group && m.target == target).is_none_or(|m| t >= m.next - EPS)
    }

    /// Writes `mark` (replacing the one of its target), dropping marks of other runs; stays sorted.
    pub fn write(&mut self, mark: RepeatMark) {
        self.0.retain(|m| m.instance == mark.instance && !(m.group == mark.group && m.target == mark.target));
        self.0.push(mark);
        self.0.sort_by_key(|m| (m.instance, m.group, m.target));
    }
}

/// Objects were spawned ([`spawn_objects`]). Journal only.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct ObjectsSpawned {
    /// The owner they belong to.
    pub owner: Entity,
    /// The action the first of them runs.
    pub action: String,
    pub count: u32,
}

/// The projectiles that met a target in this tick's hit detection; `consume_projectiles` takes
/// them in the same tick (empty between ticks).
#[derive(Resource, Default)]
pub struct MetProjectiles(pub(crate) Vec<Entity>);

/// The intent of a body with no controls: an object's run reads no presses and holds nothing.
static NO_INTENT: Intent =
    Intent { move_dir: Vec3::ZERO, aim: None, target: None, held: Vec::new(), pressed: Vec::new(), orders: std::collections::VecDeque::new() };

/// A salt of the spawner's random stream: the pattern's turn.
const TURN_SALT: u64 = 0x05fa_7e0b;

/// `v` turned `deg` degrees about +Y (a positive angle turns left).
fn turn(v: Vec3, deg: f32) -> Vec3 {
    let (s, c) = ops::sin_cos(deg.to_radians());
    Vec3::new(v.x * c + v.z * s, v.y, -v.x * s + v.z * c)
}

/// The owner a spawn by `spawner` credits: its own owner if it is an object, else itself.
fn owner_of(world: &World, spawner: Entity) -> Option<AiId> {
    match world.get::<CombatObject>(spawner) {
        Some(o) => Some(o.owner),
        None => world.get::<AiId>(spawner).copied(),
    }
}

/// Where a homing projectile steers for `id`, and whether that has fallen: a body's aim point
/// (`aim_height` above its feet), or a lock point's centre (a part, R3), fallen with its body.
#[allow(clippy::type_complexity, reason = "a Bevy query")]
fn homing_at(
    index: &AiIndex,
    bodies: &Query<(&Transform, Has<Dead>, Has<Downed>, Option<&Part>), Without<ProjectileFlight>>,
    id: AiId,
    aim_height: f32,
) -> Option<(Vec3, bool)> {
    let (tf, dead, down, part) = bodies.get(index.get(id)?).ok()?;
    match part {
        Some(part) => {
            let (_, dead, down, _) = bodies.get(index.get(part.body)?).ok()?;
            Some((tf.translation, dead || down))
        }
        None => Some((tf.translation + Vec3::Y * aim_height, dead || down)),
    }
}

/// The target a body that does not lock on aims its spawned objects at (`SpawnAim::Target`, `AtTarget`), read after
/// [`Intent::target`] (the lock-on) and before a flight's. An NPC's drive mirrors its brain's current target into it
/// every tick (`shards_npc`), so a spawn pressed by a brain finds its target with no `lock_on`. Derived state, never input:
/// it is left out of replays (`replay_ignore`) and written again every tick before anything reads it.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct AimTarget(pub Option<AiId>);

/// What a spawner aims at: its lock-on target, else its [`AimTarget`], else (an object) the target its projectile flies at;
/// each only while it stands (B5's [`Standing`](crate::Standing): a dead or downed body is no target). A lock or a brain's
/// target on a body that fell is let go a tick later (the lock check, the brain's perception); a spawn in between passes
/// over it to the next one, and with none it is skipped.
fn target_of(world: &World, spawner: Entity) -> Option<AiId> {
    // A lock point (a part, R3) stands while its body does.
    let stands = |id: &AiId| {
        let body = world.resource::<AiIndex>().get(*id).map(|e| crate::parts::body_of(world, e));
        body.is_some_and(|e| world.get::<Dead>(e).is_none() && world.get::<Downed>(e).is_none())
    };
    world
        .get::<Intent>(spawner)
        .and_then(|i| i.target)
        .filter(stands)
        .or_else(|| world.get::<AimTarget>(spawner).and_then(|a| a.0).filter(stands))
        .or_else(|| world.get::<ProjectileFlight>(spawner).and_then(|f| f.target).filter(stands))
}

/// One object of a spawn, placed: its index on the lists, where, which way it faces, which way it
/// flies (a projectile).
struct Placed {
    index: usize,
    point: Vec3,
    facing: Vec3,
    dir: Vec3,
}

/// Spawns `def`'s objects for `spawner`'s owner (`action`: the spawning action, for messages), at
/// `origin` (a point and a facing) or where the spawner stands and faces. Returns the objects
/// spawned, in index order (radial: line by line). Nothing when the owner is dead or gone, when the
/// aim needs a target and there is none, when this machine does not simulate (a co-op client: the
/// host's objects come over), or for the indices past a cap ([`CombatTuning::max_objects`],
/// `max_objects_per_owner`; a journal warning `{owner} spawn capped` says so). The objects whose
/// `start_in` is 0 start their action at once ([`force_action`]); they act from the next tick.
pub fn spawn_objects(world: &mut World, spawner: Entity, action: &str, def: &SpawnDef, origin: Option<(Vec3, Vec3)>) -> Vec<Entity> {
    if !world.get_resource::<NetRole>().is_none_or(|r| r.simulates()) {
        return Vec::new();
    }
    let Some(owner) = owner_of(world, spawner) else {
        report_once(world, format!("spawn {action}"), format!("action `{action}`: a spawn needs an AI-visible spawner; nothing spawned"));
        return Vec::new();
    };
    let owner_entity = world.resource::<AiIndex>().get(owner);
    // A dead or gone owner lays nothing down: its objects would go on the next tick anyway.
    if owner_entity.is_none_or(|o| world.get::<Dead>(o).is_some()) {
        debug!("{owner} spawn of {action}: the owner is dead or gone; nothing spawned");
        return Vec::new();
    }
    let (base, facing) = match origin {
        Some((point, facing)) => (point, facing),
        None => {
            let Some(tf) = world.get::<Transform>(spawner) else { return Vec::new() };
            let running = world.get::<ActionState>(spawner).and_then(|s| s.current.as_ref()).map(|r| r.facing);
            (tf.translation, running.unwrap_or(*tf.forward()))
        }
    };
    let mut facing = flat(facing).try_normalize().unwrap_or(Vec3::NEG_Z);
    let target = target_of(world, spawner);
    let aimed = target.and_then(|id| world.resource::<AiIndex>().get(id));
    let target_feet = aimed.and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
    // A lock point (a part, R3): a projectile aims at the point itself, at its centre (its position), and
    // a pattern laid at the target goes around its body's feet.
    let point = aimed.is_some_and(|e| world.get::<Part>(e).is_some());
    let body_feet = aimed.map(|e| body_of(world, e)).and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
    let tuning = CombatTuning::of(world.resource::<ContentDb>());
    let (live, mine) = count_objects(world, owner);
    let fit = (tuning.max_objects as usize).saturating_sub(live).min((tuning.max_objects_per_owner as usize).saturating_sub(mine));
    let mut center = base;
    match def.aim {
        SpawnAim::Facing => {}
        SpawnAim::Target { .. } | SpawnAim::AtTarget if target_feet.is_none() => {
            let text = format!("{owner} spawn of {action} skipped: it aims at a target and there is none (provide a target or lock_on first); nothing spawned");
            journal::emit(world, JournalKind::Event, text);
            return Vec::new();
        }
        SpawnAim::Target { .. } => {
            if let Some(to) = target_feet.and_then(|t| flat(t - base).try_normalize()) {
                facing = to;
            }
        }
        SpawnAim::AtTarget => center = body_feet.unwrap_or(base),
    }
    if def.random_turn {
        let (seed, tick) = (world.resource::<SimSeed>().0, world.resource::<SimTick>().0);
        let id = world.get::<AiId>(spawner).map_or(owner.0, |i| i.0);
        // Two turned spawns of one spawner in one tick differ by the objects the first laid down.
        let angle = DetRng::from_keys(&[seed, tick, id as u64, TURN_SALT, mine as u64]).range(0.0, 360.0);
        facing = turn(facing, angle);
    }
    let center = center + yaw(facing) * def.at.to_vec3();
    // The 3D direction a projectile of the pattern's axis flies.
    let aim3 = |at: Vec3, axis: Vec3| match (def.aim, target_feet) {
        (SpawnAim::Target { height }, Some(feet)) => (feet + Vec3::Y * if point { 0.0 } else { height } - at).try_normalize().unwrap_or(axis),
        _ => axis,
    };
    // No more than fit are kept (content can ask for 255 x 255): the rest is the cap's to journal.
    let total = def.total();
    let mut placed: Vec<Placed> = Vec::with_capacity(total.min(fit));
    match def.pattern {
        SpawnPattern::Fan { spread } => {
            let mid = (def.count as f32 - 1.0) * 0.5;
            let along = aim3(center, facing);
            for i in 0..def.count as usize {
                let deg = spread * (i as f32 - mid);
                placed.push(Placed { index: i, point: center, facing: turn(facing, deg), dir: turn(along, deg) });
            }
        }
        SpawnPattern::Line { start, step } => {
            for i in 0..def.count as usize {
                let point = center + facing * (start + step * i as f32);
                placed.push(Placed { index: i, point, facing, dir: aim3(point, facing) });
            }
        }
        SpawnPattern::Radial { lines, start, step } => {
            for k in 0..lines as usize {
                let axis = turn(facing, 360.0 * k as f32 / lines as f32);
                for j in 0..def.count as usize {
                    let point = center + axis * (start + step * j as f32);
                    placed.push(Placed { index: j, point, facing: axis, dir: axis });
                }
            }
        }
    }
    // The caps: the indices that fit, in index order.
    if fit < total {
        let text = format!(
            "{owner} spawn capped: {fit} of {total} {} (max_objects {}, max_objects_per_owner {})",
            by_index(&def.action, 0).unwrap_or(action),
            tuning.max_objects,
            tuning.max_objects_per_owner
        );
        journal::emit(world, JournalKind::Warn, text);
    }
    placed.truncate(fit);
    if let Some(depth) = def.ground {
        let physics = world.resource::<PhysicsWorld>();
        let filter = QueryFilter::new(LayerMask::WORLD);
        for p in &mut placed {
            if let Some(hit) = physics.raycast(p.point, Dir3::NEG_Y, depth, &filter) {
                p.point = hit.point;
            }
        }
    }
    let tick = world.resource::<SimTick>().0;
    let team = owner_entity.and_then(|o| world.get::<Team>(o)).cloned();
    let mut spawned = Vec::with_capacity(placed.len());
    for p in placed {
        let (Some(prefab), Some(run)) = (by_index(&def.prefab, p.index), by_index(&def.action, p.index)) else { continue };
        let tf = Transform::from_translation(p.point).looking_to(p.facing, Vec3::Y);
        let entity = match shards_content::spawn_prefab_cached(world, prefab, tf) {
            Ok(entity) => entity,
            Err(why) => {
                report_once(world, format!("spawn prefab {prefab}"), format!("action `{action}`: {why}; nothing spawned for it"));
                continue;
            }
        };
        if world.get::<AiId>(entity).is_none() {
            world.despawn(entity);
            let text = format!("action `{action}`: prefab `{prefab}` is not AI-visible (give it an AiKind); nothing spawned for it");
            report_once(world, format!("spawn kind {prefab}"), text);
            continue;
        }
        // A prefab's flight numbers are not checked with the content: a bad one is refused here,
        // where the prefab is named, instead of tripping `action.objects_sane` on the first spawn.
        if let Some(why) = world.get::<Projectile>(entity).and_then(Projectile::problem) {
            world.despawn(entity);
            report_once(world, format!("spawn projectile {prefab}"), format!("action `{action}`: prefab `{prefab}` has a Projectile with {why}; nothing spawned for it"));
            continue;
        }
        let start_in = def.delay * p.index as f32;
        // A projectile spawned behind a wall (a caster flush against it) stops at the wall.
        let blocked = (world.entity(entity).contains::<Projectile>() && def.aim != SpawnAim::AtTarget)
            .then(|| wall_between(world, base, p.point))
            .flatten();
        let mut e = world.entity_mut(entity);
        e.insert(CombatObject { owner, spawned: tick, start_in, action: run.to_owned(), stay: def.stay });
        if let Some(team) = &team {
            e.insert(team.clone());
        }
        if !e.contains::<ActionState>() {
            e.insert(ActionState::default());
        }
        if e.contains::<Projectile>() {
            let dir = p.dir.try_normalize().unwrap_or(p.facing);
            match blocked {
                // Stopped at the wall: it impacts there (its run starts, `consume_projectiles` ends it).
                Some(at) => {
                    if let Some(mut tf) = e.get_mut::<Transform>() {
                        tf.translation = at;
                    }
                    e.insert(ProjectileFlight { dir: Vec3::ZERO, age: 0.0, target, from: at });
                }
                None => {
                    e.insert(ProjectileFlight { dir, age: 0.0, target, from: p.point });
                }
            }
        }
        spawned.push(entity);
    }
    // Started once all are in the world, so a spawn their start makes counts them for its caps.
    for &entity in &spawned {
        let Some(o) = world.get::<CombatObject>(entity).filter(|o| o.start_in <= EPS) else { continue };
        let run = o.action.clone();
        force_action(world, entity, &run);
    }
    if !spawned.is_empty() {
        let first = by_index(&def.action, 0).unwrap_or(action).to_owned();
        world.trigger(ObjectsSpawned { owner: owner_entity.unwrap_or(spawner), action: first, count: spawned.len() as u32 });
    }
    spawned
}

/// The combat objects in the world, and those of `owner`.
fn count_objects(world: &World, owner: AiId) -> (usize, usize) {
    let Some(mut q) = world.try_query::<&CombatObject>() else { return (0, 0) };
    q.iter(world).fold((0, 0), |(all, mine), o| (all + 1, mine + usize::from(o.owner == owner)))
}

/// `owner`'s combat objects, in `AiId` order.
pub fn objects_of(world: &World, owner: AiId) -> Vec<Entity> {
    let Some(mut q) = world.try_query::<(Entity, &AiId, &CombatObject)>() else { return Vec::new() };
    let mut found: Vec<(AiId, Entity)> = q.iter(world).filter(|(_, _, o)| o.owner == owner).map(|(e, id, _)| (*id, e)).collect();
    found.sort();
    found.into_iter().map(|(_, e)| e).collect()
}

/// Despawns `owner`'s combat objects now, in `AiId` order (a boss's phase change clears its
/// zones; the owner's death does it too). Returns how many went.
pub fn despawn_objects_of(world: &mut World, owner: AiId) -> usize {
    let objects = objects_of(world, owner);
    for &entity in &objects {
        world.despawn(entity);
    }
    objects.len()
}

/// The tags of the combat objects' live zones (a hit window still to come or open) that a body of
/// `margin` metres at `point` touches, sorted, each once: what a planner reads before it lays a
/// zone where one is ([`hit_zones`] with [`HitZone::tags`](crate::HitZone)).
pub fn live_tags_at(world: &World, point: Vec3, margin: f32) -> Vec<String> {
    let mut tags: Vec<String> = hit_zones(world)
        .into_iter()
        .filter(|z| z.owner.is_some() && !z.tags.is_empty() && z.shape.contains(point, margin))
        .flat_map(|z| z.tags)
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Whether the owner of an object resolves and is alive: `Some(owner)`, or `None` (its zone deals
/// nothing, and it goes).
pub(crate) fn living_owner(world: &World, object: &CombatObject) -> Option<Entity> {
    world.resource::<AiIndex>().get(object.owner).filter(|o| world.get::<Dead>(*o).is_none())
}

// --- systems ---------------------------------------------------------------------------------

/// The combat objects that are not fighters: what [`run_objects`] steps.
type ObjectsOnly = (With<CombatObject>, Without<Fighter>);

/// `SimSet::Act`, [`ActionSystems::Objects`](crate::ActionSystems::Objects): every combat object that
/// is not a fighter, in `AiId` order, from the tick after its spawn. One whose owner is gone or dead
/// is despawned (no impact); a delayed one counts down and starts its action when the delay is
/// over; a running one steps its action like the machine does (effects and telegraphs it reaches),
/// with no controls and no target.
pub(crate) fn run_objects(world: &mut World, objects: &mut QueryState<(Entity, &AiId), ObjectsOnly>) {
    let mut order: Vec<(AiId, Entity)> = objects.iter(world).map(|(e, id)| (*id, e)).collect();
    if order.is_empty() {
        return;
    }
    order.sort();
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let tick = world.resource::<SimTick>().0;
    for (_, entity) in order {
        let Some((spawned, start_in, alive)) =
            world.get::<CombatObject>(entity).map(|o| (o.spawned, o.start_in, living_owner(world, o).is_some()))
        else {
            continue;
        };
        if spawned == tick {
            continue;
        }
        if !alive {
            world.despawn(entity);
            continue;
        }
        if start_in > EPS {
            let left = start_in - dt;
            let Some(mut object) = world.get_mut::<CombatObject>(entity) else { continue };
            if left > EPS {
                object.start_in = left;
                continue;
            }
            object.start_in = 0.0;
            let run = object.action.clone();
            force_action(world, entity, &run);
            continue;
        }
        step_object(world, entity, dt);
    }
}

/// One tick of an object's running action: its time moves on, and the effects and telegraph it
/// crosses are queued and triggered as the machine does them. An object's run does not move its
/// body (`displacement` is a fighter's).
fn step_object(world: &mut World, entity: Entity, dt: f32) {
    let Some(here) = world.get::<Transform>(entity).map(|t| t.translation) else { return };
    let Some(mut run) = world.get_mut::<ActionState>(entity).and_then(|mut s| s.current.take()) else { return };
    let db = world.resource::<ContentDb>();
    let Some(def) = db.get::<ActionDef>(&run.action) else {
        // Its definition left content (a hot reload): it stops.
        world.trigger(ActionEnded::of(entity, &run, ActionEnd::Removed));
        return;
    };
    let (spans, _) = step_run(&mut run, def, None, 1.0, &NO_INTENT, here, None, dt);
    let mut effects = Vec::new();
    let mut telegraphs = Vec::new();
    crossed(def, &spans, |e| effects.push(e.clone()), || telegraphs.push(telegraph(entity, def, &run, here)));
    run.refresh(def, 0.0);
    if !effects.is_empty() {
        let action = run.action.clone();
        world.resource_mut::<PendingEffects>().0.extend(effects.into_iter().map(|e| (entity, action.clone(), e)));
    }
    if let Some(mut state) = world.get_mut::<ActionState>(entity) {
        state.current = Some(run);
    }
    for t in telegraphs {
        world.trigger(t);
    }
}

/// The spherical interpolation of the unit vectors `a` toward `b` by `w`, written with `ops`.
fn blend(a: Vec3, b: Vec3, w: f32) -> Vec3 {
    let theta = ops::acos(a.dot(b).clamp(-1.0, 1.0));
    let s = ops::sin(theta);
    if s < 1e-4 {
        // Aligned (nothing to turn), or opposite (no one way to turn): it keeps going.
        return a;
    }
    let mixed = a * (ops::sin((1.0 - w) * theta) / s) + b * (ops::sin(w * theta) / s);
    mixed.try_normalize().unwrap_or(a)
}

/// `SimSet::Move`, after the controller (homing sees where its target is this tick): every flying
/// projectile in `AiId` order, from the tick after its spawn and once its action started. Its age
/// grows, and at `lifetime` it ends (its action is cut: no impact). Homing turns its direction toward
/// its living target by `homing * dt`. It flies `speed * dt`; a wall (`LayerMask::WORLD`) on the way
/// stops it there (`dir` zero), and `consume_projectiles` runs its impact after the hits. The run's
/// facing follows the flight.
#[allow(clippy::type_complexity, reason = "a Bevy query")]
pub(crate) fn move_projectiles(
    time: Res<Time>,
    tick: Res<SimTick>,
    index: Res<AiIndex>,
    physics: Res<PhysicsWorld>,
    mut projectiles: Query<(Entity, &AiId, &CombatObject, &Projectile, &mut ProjectileFlight, &mut Transform, &mut ActionState)>,
    bodies: Query<(&Transform, Has<Dead>, Has<Downed>, Option<&Part>), Without<ProjectileFlight>>,
    mut commands: Commands,
) {
    if projectiles.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut order: Vec<(AiId, Entity)> = projectiles.iter().map(|(e, id, ..)| (*id, e)).collect();
    order.sort();
    for (_, entity) in order {
        let Ok((_, _, object, p, mut flight, mut tf, mut state)) = projectiles.get_mut(entity) else { continue };
        let pos = tf.translation;
        if flight.from != pos {
            flight.from = pos;
        }
        // A run that has not stepped yet (t = 0: a delayed start this tick) flies from the next tick.
        if object.spawned == tick.0 || object.start_in > EPS || flight.dir == Vec3::ZERO || state.current.as_ref().is_none_or(|r| r.t <= EPS) {
            continue;
        }
        flight.age += dt;
        if flight.age >= p.lifetime - EPS {
            commands.queue(move |world: &mut World| {
                cut_action(world, entity);
            });
            continue;
        }
        if p.homing > 0.0
            && let Some((there, false)) = flight.target.and_then(|id| homing_at(&index, &bodies, id, p.aim_height))
        {
            let want = there - pos;
            if want.length_squared() > 0.01 {
                flight.dir = blend(flight.dir, want.normalize(), (p.homing * dt).clamp(0.0, 1.0));
            }
        }
        let step = p.speed * dt;
        let filter = QueryFilter::new(LayerMask::WORLD).excluding(entity);
        let dir = flight.dir;
        match Dir3::new(dir).ok().and_then(|d| physics.raycast(pos, d, step, &filter)) {
            Some(hit) => {
                // Just off the surface, so what it lays there stands outside the wall.
                tf.translation = hit.point + hit.normal * 0.02;
                flight.dir = Vec3::ZERO;
            }
            None => tf.translation = pos + dir * step,
        }
        if let Some(facing) = flat(dir).try_normalize()
            && let Some(run) = state.current.as_mut()
        {
            run.facing = facing;
            tf.look_to(dir, Vec3::Y);
        }
    }
}

/// `SimSet::Resolve`, [`ActionSystems::Hits`](crate::ActionSystems::Hits) after the hits: a projectile
/// still in its flight action that a wall stopped this tick, or that met a target in this tick's
/// detection and does not pierce (whatever came of the hit: a block, a dodge), impacts in `AiId`
/// order: it stops, and runs its flight action's `impact` (else its action is cut and it goes).
pub(crate) fn consume_projectiles(world: &mut World, flying: &mut QueryState<(Entity, &AiId, &CombatObject, &ProjectileFlight, &Projectile, &ActionState)>) {
    let met = world.get_resource_mut::<MetProjectiles>().map(|mut m| std::mem::take(&mut m.0)).unwrap_or_default();
    let mut due: Vec<(AiId, Entity)> = flying
        .iter(world)
        .filter(|(e, _, object, flight, p, state)| {
            let in_flight = state.current.as_ref().is_some_and(|r| r.action == object.action);
            in_flight && (flight.dir == Vec3::ZERO || (!p.pierce && met.contains(e)))
        })
        .map(|(e, id, ..)| (*id, e))
        .collect();
    if due.is_empty() {
        return;
    }
    due.sort();
    for (_, entity) in due {
        impact(world, entity);
    }
}

/// `entity` stops where it is and runs its flight action's `impact`, or ends.
fn impact(world: &mut World, entity: Entity) {
    if let Some(mut flight) = world.get_mut::<ProjectileFlight>(entity) {
        flight.dir = Vec3::ZERO;
    }
    let running = world.get::<ActionState>(entity).and_then(|s| s.current.as_ref()).map(|r| r.action.clone());
    let then = running.and_then(|a| world.resource::<ContentDb>().get::<ActionDef>(&a).and_then(|d| d.impact.clone()));
    match then {
        Some(impact) => {
            force_action(world, entity, &impact);
        }
        None => {
            cut_action(world, entity);
        }
    }
}

/// `SimSet::Resolve`, [`ActionSystems::Deaths`](crate::ActionSystems::Deaths) before the deaths: in
/// `AiId` order, the objects whose action is over (started, not `stay`, no running action) and those
/// whose owner is gone or dead are despawned.
pub(crate) fn tend_objects(world: &mut World, objects: &mut QueryState<(Entity, &AiId, &CombatObject, Option<&ActionState>)>) {
    let seen: &World = world;
    let mut gone: Vec<(AiId, Entity)> = objects
        .iter(seen)
        .filter(|(_, _, o, state)| {
            let over = !o.stay && o.start_in <= EPS && state.is_none_or(|s| s.current.is_none());
            over || living_owner(seen, o).is_none()
        })
        .map(|(e, id, ..)| (*id, e))
        .collect();
    if gone.is_empty() {
        return;
    }
    gone.sort();
    for (_, entity) in gone {
        world.despawn(entity);
    }
}

// --- the AI layer ----------------------------------------------------------------------------

/// `of e6 (chill_puddle)`, `starts in 0.22s`: what a combat object is, unless it is a projectile
/// (its own line says it).
fn describe_object(e: EntityRef) -> Option<String> {
    let o = e.get::<CombatObject>()?;
    if e.contains::<Projectile>() {
        return None;
    }
    let mut line = format!("of {} ({})", o.owner, o.action);
    if o.start_in > EPS {
        line.push_str(&format!("  starts in {:.2}s", o.start_in));
    }
    Some(line)
}

/// `projectile of e6 15.0m/s -> e5 0.40s` (the time is the distance to the target over the speed),
/// `projectile of e6 stopped`, `starts in 0.22s`.
fn describe_projectile(world: &World, entity: Entity) -> Option<String> {
    let e = world.entity(entity);
    let (o, p, flight) = (e.get::<CombatObject>()?, e.get::<Projectile>()?, e.get::<ProjectileFlight>()?);
    let mut line = format!("projectile of {}", o.owner);
    if o.start_in > EPS {
        line.push_str(&format!(" starts in {:.2}s", o.start_in));
    } else if flight.dir == Vec3::ZERO {
        line.push_str(" stopped");
    } else {
        line.push_str(&format!(" {:.1}m/s", p.speed));
        let here = e.get::<Transform>()?.translation;
        if let Some(id) = flight.target
            && let Some(t) = world.resource::<AiIndex>().get(id)
            && let Some(there) = world.get::<Transform>(t)
        {
            line.push_str(&format!(" -> {id}"));
            if p.speed > 0.0 {
                let aim = if world.get::<Part>(t).is_some() { 0.0 } else { p.aim_height };
                line.push_str(&format!(" {:.2}s", (there.translation + Vec3::Y * aim - here).length() / p.speed));
            }
        }
    }
    Some(line)
}

/// Some combat object is in the world.
fn objects_live(world: &World) -> bool {
    world.try_query::<&CombatObject>().is_some_and(|mut q| q.iter(world).next().is_some())
}

/// Some projectile is flying (started, not stopped).
fn projectile_flying(world: &World) -> bool {
    world
        .try_query::<(&CombatObject, &ProjectileFlight)>()
        .is_some_and(|mut q| q.iter(world).any(|(o, f)| o.start_in <= EPS && f.dir != Vec3::ZERO))
}

/// The combat objects in the world (or of one owner), in `AiId` order: each one's id, owner, kind,
/// the action it runs, its phase and the seconds left of it (or when it starts). Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct CombatObjectsArgs {
    /// Only this owner's: 6 or "e6". Default: every object.
    pub owner: Option<Id>,
}

fn combat_objects(In(call): In<Call<CombatObjectsArgs>>, world: &World) -> ToolResult<Value> {
    let owner = match call.args.owner {
        Some(id) => {
            let entity = id.entity(world)?;
            Some(world.get::<AiId>(entity).copied().unwrap_or(id.0))
        }
        None => None,
    };
    let Some(mut q) = world.try_query::<(Entity, &AiId, &CombatObject)>() else { return Ok(json!([])) };
    let mut found: Vec<(AiId, Entity)> =
        q.iter(world).filter(|(_, _, o)| owner.is_none_or(|w| o.owner == w)).map(|(e, id, _)| (*id, e)).collect();
    found.sort();
    let round = |v: f32| (v * 100.0).round() / 100.0;
    let list: Vec<Value> = found
        .into_iter()
        .map(|(id, entity)| {
            let e = world.entity(entity);
            let o = e.get::<CombatObject>().expect("queried");
            let run = e.get::<ActionState>().and_then(|s| s.current.as_ref());
            let mut row = json!({
                "id": id.to_string(),
                "owner": o.owner.to_string(),
                "kind": e.get::<AiKind>().map_or("", |k| k.0.as_ref()),
                "action": run.map_or(o.action.as_str(), |r| r.action.as_str()),
            });
            match run {
                Some(r) => {
                    row["phase"] = json!(r.phase);
                    row["left"] = json!(round((r.length - r.t).max(0.0)));
                }
                None if o.start_in > EPS => row["starts_in"] = json!(round(o.start_in)),
                None => {}
            }
            if let Some(tf) = e.get::<Transform>() {
                row["at"] = json!(ai_core::spatial::fmt_pos(tf.translation));
            }
            row
        })
        .collect();
    Ok(json!(list))
}

/// The module's registrations: types, resources, systems, the AI layer.
pub(crate) fn plugin(app: &mut App) {
    use crate::ActionSystems;
    app.register_type::<CombatObject>()
        .register_type::<Projectile>()
        .register_type::<ProjectileFlight>()
        .register_type::<Repeats>()
        .register_type::<ObjectsSpawned>()
        .init_resource::<MetProjectiles>();
    let world = app.world_mut();
    world.register_component::<CombatObject>();
    world.register_component::<Projectile>();
    world.register_component::<ProjectileFlight>();
    world.register_component::<Repeats>();
    app.add_systems(
        FixedUpdate,
        (
            run_objects.in_set(ActionSystems::Objects),
            move_projectiles.in_set(SimSet::Move).after(shards_motion::ControllerSystems::Move),
            consume_projectiles.in_set(ActionSystems::Hits).after(crate::machine::check_misses),
            tend_objects.in_set(ActionSystems::Deaths).before(crate::combat::tick_downed),
        ),
    )
    .register_ai_describer("object", describe_object)
    .register_ai_world_describer("projectile", describe_projectile)
    .add_ai_condition("objects_live", objects_live)
    .add_ai_condition("projectile_flying", projectile_flying)
    .add_ai_query("combat_objects", Scope::Player, combat_objects)
    .journal_event::<ObjectsSpawned>(|e, w| Some(format!("{} spawns {} x {}", label(w, e.owner), e.count, e.action)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_positive_turn_goes_left() {
        let left = turn(Vec3::NEG_Z, 90.0);
        assert!((left - Vec3::NEG_X).length() < 1e-5, "{left}");
        let back = turn(Vec3::NEG_Z, 180.0);
        assert!((back - Vec3::Z).length() < 1e-5, "{back}");
    }

    #[test]
    fn blend_turns_by_its_weight_and_keeps_unit_length() {
        let half = blend(Vec3::NEG_Z, Vec3::X, 0.5);
        assert!((half.length() - 1.0).abs() < 1e-5);
        assert!((half.angle_between(Vec3::NEG_Z).to_degrees() - 45.0).abs() < 1e-3, "{half}");
        assert_eq!(blend(Vec3::NEG_Z, Vec3::NEG_Z, 0.3), Vec3::NEG_Z);
        assert_eq!(blend(Vec3::NEG_Z, Vec3::X, 0.0), Vec3::NEG_Z);
    }

    #[test]
    fn repeat_marks_stay_sorted_and_forget_older_runs() {
        let mark = |instance, group, target, next| RepeatMark { instance, group, target: AiId(target), next };
        let mut r = Repeats::default();
        r.write(mark(1, 0, 9, 0.8));
        r.write(mark(1, 0, 5, 0.9));
        r.write(mark(1, 1, 5, 1.0));
        assert_eq!(r.0.iter().map(|m| (m.group, m.target.0)).collect::<Vec<_>>(), vec![(0, 5), (0, 9), (1, 5)]);
        assert!(!r.allows(1, 0, AiId(5), 0.5));
        assert!(r.allows(1, 0, AiId(5), 0.9));
        assert!(r.allows(1, 0, AiId(7), 0.0));
        r.write(mark(2, 0, 5, 0.3));
        assert_eq!(r.0, vec![mark(2, 0, 5, 0.3)], "a later run's mark drops the earlier run's");
    }

    #[test]
    fn spawn_fields_keep_their_defaults_out_of_the_json() {
        let def = SpawnDef {
            prefab: vec!["bolt".into()],
            action: vec!["fly".into()],
            at: Local::default(),
            aim: SpawnAim::Facing,
            count: 1,
            pattern: SpawnPattern::default(),
            delay: 0.0,
            random_turn: false,
            ground: None,
            stay: false,
        };
        assert_eq!(serde_json::to_string(&def).unwrap(), r#"{"prefab":["bolt"],"action":["fly"]}"#);
        assert_eq!(def.total(), 1);
        let radial = SpawnDef { count: 4, pattern: SpawnPattern::Radial { lines: 8, start: 1.0, step: 2.0 }, ..def };
        assert_eq!(radial.total(), 32);
    }
}
