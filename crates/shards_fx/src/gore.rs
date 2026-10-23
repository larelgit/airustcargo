//! Gore (F9): the marks on surfaces and the debris that lasts, from the game's side. [`spawn_mark`] lays a
//! mark of a kind (`*.mark.ron`) on a surface; the `fx` table's links ([`MarkLink`]) lay them from engine
//! events (a shot stopping at a wall, a shot going through a body, a blow, a death, a part breaking off) and
//! play the effects of the same events (`shot_impact`, `shot_hit`, `part_broken`); [`clear`] takes them away.
//!
//! Everything here is the picture's and local to a machine: no `AiId`, nothing in a replay or a snapshot, and
//! nothing at all on a machine that draws no picture. The events it answers (`ShotFired`, `ShotImpact`,
//! `Damaged`, `Died`) reach a co-op client from the host, so marks and chunks appear there too; `PartBroken`
//! is not forwarded to clients, so a client lays what `part_broken` links only for the parts it breaks
//! itself (none), and the chunks of a body's death come from `died`.
//!
//! Randomness is the picture's own: a seed from the event (the point, the normal, the pellet), never the
//! simulation's `SimRng`, so a shot lays the same mark on the host and on a client.

use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_action::{Damaged, Died, PartBroken, ShotFired, ShotImpact};
use shards_content::db::closest;
use shards_content::ContentDb;
use shards_motion::{LayerMask, PhysicsWorld, QueryFilter};

use crate::mark::{Mark, MarkDef, MarkLink, MarkPool, Place};
use crate::{clock, draws, fx_def, place, play_quietly, sim, EffectAt, Particles};

/// Rays a frame the debris steps may cast before the rest wait for the next frame.
pub const RAYS_PER_FRAME: u32 = 2048;

/// The marks alive, by kind, and what the picture needs to lay them.
#[derive(Resource, Default)]
pub struct Marks {
    pub kinds: BTreeMap<String, MarkPool>,
    /// Marks laid so far.
    pub laid: u64,
    /// Where each shooter's last shot started: a shot that went through a body has the point it hit and
    /// not its direction, which is the way from here.
    shots: BTreeMap<Entity, Vec3>,
    /// Marks named that no content defines, journaled once each.
    unknown: BTreeSet<String>,
}

impl Marks {
    /// Marks alive now, of every kind.
    pub fn live(&self) -> usize {
        self.kinds.values().map(MarkPool::live).sum()
    }

    fn note_shot(&mut self, shooter: Entity, from: Vec3) {
        if self.shots.len() >= 64 && !self.shots.contains_key(&shooter) {
            self.shots.clear();
        }
        self.shots.insert(shooter, from);
    }
}

/// What the gore costs, for the performance counters (`ai/status` → `fx`, and the `effects` query).
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoreStats {
    /// Rays cast in the last frame: the debris' steps, and the searches for the surface behind a body.
    pub rays: u32,
    /// The most rays in one frame so far.
    pub rays_max: u32,
    /// Rays cast in all.
    pub rays_total: u64,
    /// The rays the debris' steps may use in a frame.
    pub budget: u32,
    /// Searches for a surface since the last frame, folded into `rays` by it.
    pub searches: u32,
}

impl Default for GoreStats {
    fn default() -> Self {
        Self { rays: 0, rays_max: 0, rays_total: 0, budget: RAYS_PER_FRAME, searches: 0 }
    }
}

/// Lays a mark of kind `mark` at `point` on a surface whose unit normal is `normal` as soon as commands apply:
/// `commands.trigger(PlayMark::new("blood", point, normal))`. Nothing is laid while nothing is drawn; a mark no
/// content defines is journaled once.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct PlayMark {
    pub mark: String,
    pub point: Vec3,
    pub normal: Vec3,
}

impl PlayMark {
    pub fn new(mark: impl Into<String>, point: Vec3, normal: Vec3) -> Self {
        Self { mark: mark.into(), point, normal }
    }
}

/// What [`spawn_mark`] laid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marked {
    /// Marks of the kind alive now, and the most it keeps.
    pub live: usize,
    pub cap: usize,
    /// It took the place of the oldest, the kind being full.
    pub recycled: bool,
    /// Its width (m).
    pub size: f32,
}

/// What a clear took away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    pub marks: usize,
    pub particles: usize,
}

impl Cleared {
    pub fn total(&self) -> usize {
        self.marks + self.particles
    }
}

/// FNV-1a over `words`, the picture's seed for a mark: the same on every machine and every run (a
/// `DefaultHasher` is randomized per process).
pub fn seed_of(words: &[u32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for word in words {
        for byte in word.to_le_bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn bits(v: Vec3) -> [u32; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// The seed of a mark laid at `point` on a surface of `normal` (`salt` tells marks at one place apart: a
/// pellet's index, a link's).
pub fn mark_seed(point: Vec3, normal: Vec3, salt: u32) -> u64 {
    let (p, n) = (bits(point), bits(normal));
    seed_of(&[p[0], p[1], p[2], n[0], n[1], n[2], salt])
}

/// Lays a mark of kind `mark` at `point` on a surface with unit normal `normal`: a size from the kind's range and
/// a turn about the normal, drawn from a seed of the place. Errors: a mark no content defines (naming the
/// closest), a point or a normal that is no direction, nothing drawn.
pub fn spawn_mark(world: &mut World, mark: &str, point: Vec3, normal: Vec3) -> Result<Marked, String> {
    spawn_mark_seeded(world, mark, point, normal, mark_seed(point, normal, 0))
}

/// [`spawn_mark`] with the seed the size and the turn are drawn from.
pub fn spawn_mark_seeded(world: &mut World, mark: &str, point: Vec3, normal: Vec3, seed: u64) -> Result<Marked, String> {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    let Some(def) = db.get::<MarkDef>(mark).cloned() else {
        let known: Vec<&str> = db.iter::<MarkDef>().map(|(id, _)| id).collect();
        let hint = closest(mark, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no mark `{mark}`{hint}: content `*.mark.ron` defines marks; {} known", known.len()));
    };
    if !draws(world) {
        return Err(format!("nothing is drawn headless: marks are laid in a window or with --offscreen (mark `{mark}` is known)"));
    }
    let Some(normal) = normal.try_normalize().filter(|n| point.is_finite() && n.is_finite()) else {
        return Err(format!("a mark needs a point and a surface normal that point somewhere, not {point} and {normal}"));
    };
    let now = clock(world);
    let mut rng = sim::Rng::new(seed);
    let size = rng.between(def.size);
    let spin = if def.rotate { rng.between((0.0, TAU)) } else { 0.0 };
    let mut marks = world.resource_mut::<Marks>();
    let pool = marks.kinds.entry(mark.to_owned()).or_insert_with(|| MarkPool::new(&def));
    if pool.def != def {
        // The content changed: the marks laid under the old definition go with it.
        *pool = MarkPool::new(&def);
    }
    let before = pool.recycled;
    pool.add(Mark { born: now, point, normal, size, spin, seq: 0 });
    let marked = Marked { live: pool.live(), cap: pool.cap(), recycled: pool.recycled > before, size };
    marks.laid += 1;
    Ok(marked)
}

/// [`spawn_mark_seeded`] for what the engine lays (an event, a link, [`PlayMark`]): nothing while nothing is drawn;
/// an unknown mark is journaled once and otherwise ignored.
pub fn spawn_mark_quietly(world: &mut World, mark: &str, point: Vec3, normal: Vec3, seed: u64, why: &str) {
    if !draws(world) {
        return;
    }
    if let Err(e) = spawn_mark_seeded(world, mark, point, normal, seed)
        && e.starts_with("no mark")
        && world.resource_mut::<Marks>().unknown.insert(mark.to_owned())
    {
        journal::emit(world, JournalKind::Warn, format!("fx: {why}: {e}"));
    }
}

/// Takes away the marks of the kinds that carry `tag` (`MarkDef::tags`) and the particles of the effects that do
/// (`EffectDef::tags`): the debris a level leaves, the blood of a fight. Returns how many there were.
pub fn clear(world: &mut World, tag: &str) -> Cleared {
    clear_matching(world, |tags| tags.iter().any(|t| t == tag))
}

/// Takes away every mark and every particle.
pub fn clear_all(world: &mut World) -> Cleared {
    clear_matching(world, |_| true)
}

fn clear_matching(world: &mut World, matches: impl Fn(&[String]) -> bool) -> Cleared {
    let mut cleared = Cleared::default();
    if let Some(mut marks) = world.get_resource_mut::<Marks>() {
        for pool in marks.kinds.values_mut().filter(|p| matches(&p.def.tags)) {
            cleared.marks += pool.clear();
        }
    }
    if let Some(mut particles) = world.get_resource_mut::<Particles>() {
        let Particles { effects, emitters, .. } = &mut *particles;
        let gone: BTreeSet<String> = effects.iter().filter(|(_, p)| matches(&p.def.tags)).map(|(id, _)| id.clone()).collect();
        for id in &gone {
            if let Some(playing) = effects.get_mut(id) {
                for slot in &mut playing.pool.slots {
                    if slot.take().is_some() {
                        cleared.particles += 1;
                    }
                }
            }
        }
        emitters.retain(|e| !gone.contains(&e.effect));
    }
    cleared
}

/// Where an engine event happened, for the links that lay marks and play effects from it.
struct Happened<'a> {
    /// The event's name in the `fx` table.
    event: &'a str,
    point: Vec3,
    /// The unit normal of the surface it happened on, when it has one (a shot that stopped at a wall).
    normal: Option<Vec3>,
    /// The unit direction the blow went, when it has one: the shot's, the attacker's toward the one hit.
    way: Option<Vec3>,
    seed: u64,
}

/// Plays the effect the table gives `event`, if any, at `at`, its particles sent `toward` (see [`crate::Aim`]).
fn play_event_effect(world: &mut World, event: &str, at: EffectAt, toward: Option<Vec3>) {
    let Some(effect) = fx_def(world).and_then(|fx| fx.effects.get(event).cloned()) else { return };
    play_quietly(world, &effect, at, toward, &format!("the table's `{event}`"));
}

/// The nearest `WORLD` surface along `dir` from `from`, within `reach` metres: its point and unit normal.
pub fn surface_along(world: &World, from: Vec3, dir: Vec3, reach: f32) -> Option<(Vec3, Vec3)> {
    let physics = world.get_resource::<PhysicsWorld>()?;
    let dir = Dir3::new(dir).ok()?;
    let filter = QueryFilter { mask: LayerMask::WORLD, exclude: Vec::new(), volumes: false };
    physics.raycast(from, dir, reach, &filter).map(|hit| (hit.point, hit.normal))
}

/// Lays the marks `happened.event`'s links ask for.
fn lay_marks(world: &mut World, happened: &Happened) {
    let Some(links) = fx_def(world).and_then(|fx| fx.marks.get(happened.event).cloned()) else { return };
    for (index, MarkLink { mark, place, count, spread, offset }) in links.into_iter().enumerate() {
        let from = happened.point + Vec3::new(offset.0, offset.1, offset.2);
        for k in 0..count {
            let seed = seed_of(&[happened.seed as u32, (happened.seed >> 32) as u32, index as u32, k]);
            let found = match place {
                Place::At => happened.normal.map(|n| (happened.point, n)),
                Place::Behind(reach) => happened.way.and_then(|way| search(world, from, way, spread, reach, seed)),
                Place::Below(reach) => search(world, from, Vec3::NEG_Y, spread, reach, seed),
            };
            if let Some((point, normal)) = found {
                spawn_mark_quietly(world, &mark, point, normal, seed, &format!("the table's `{}` marks", happened.event));
            }
        }
    }
}

/// One search for a surface: a ray from `from` along `way` turned at random within `spread` degrees.
fn search(world: &mut World, from: Vec3, way: Vec3, spread: f32, reach: f32, seed: u64) -> Option<(Vec3, Vec3)> {
    let dir = if spread > 0.0 { sim::Rng::new(seed ^ 0x5eed).in_cone(way, spread) } else { way };
    if let Some(mut stats) = world.get_resource_mut::<GoreStats>() {
        stats.searches += 1;
    }
    surface_along(world, from, dir, reach)
}

/// The unit direction from `attacker` to `target`, level: the way a blow went.
fn blow_way(world: &World, attacker: Entity, target: Vec3) -> Option<Vec3> {
    let from = place(world, EffectAt::Entity(attacker))?;
    Vec3::new(target.x - from.x, 0.0, target.z - from.z).try_normalize()
}

/// A shot: where it started (its pellets' directions come from it).
pub(crate) fn on_shot_fired(shot: On<ShotFired>, mut marks: ResMut<Marks>) {
    marks.note_shot(shot.shooter, shot.from);
}

/// A pellet stopped at a world surface (a wall, the floor), or went through a body.
pub(crate) fn on_shot_impact(impact: On<ShotImpact>, mut commands: Commands) {
    let impact = impact.event().clone();
    commands.queue(move |world: &mut World| shot_impact(world, &impact));
}

fn shot_impact(world: &mut World, impact: &ShotImpact) {
    if !draws(world) {
        return;
    }
    let seed = mark_seed(impact.point, impact.normal, u32::from(impact.pellet));
    match impact.target {
        None => {
            play_event_effect(world, "shot_impact", EffectAt::Point(impact.point), Some(impact.normal));
            lay_marks(
                world,
                &Happened { event: "shot_impact", point: impact.point, normal: Some(impact.normal), way: None, seed },
            );
        }
        Some(_) => {
            let from = world.resource::<Marks>().shots.get(&impact.shooter).copied();
            let way = from.and_then(|from| (impact.point - from).try_normalize()).unwrap_or(-impact.normal);
            play_event_effect(world, "shot_hit", EffectAt::Point(impact.point), Some(-way));
            lay_marks(
                world,
                &Happened { event: "shot_hit", point: impact.point, normal: Some(impact.normal), way: Some(way), seed },
            );
        }
    }
}

/// A hit landed: marks from the body that was hit, along the blow.
pub(crate) fn on_damaged(damaged: On<Damaged>, mut commands: Commands) {
    if damaged.invulnerable {
        return;
    }
    let (attacker, target) = (damaged.attacker, damaged.target);
    commands.queue(move |world: &mut World| {
        if !draws(world) {
            return;
        }
        let Some(feet) = place(world, EffectAt::Entity(target)) else { return };
        let way = blow_way(world, attacker, feet);
        lay_marks(world, &Happened { event: "damaged", point: feet, normal: None, way, seed: mark_seed(feet, Vec3::Y, 1) });
    });
}

/// Someone died: marks from where they fell, along the killing blow when it had a direction.
pub(crate) fn on_died(died: On<Died>, mut commands: Commands) {
    let (entity, killer) = (died.entity, died.killer);
    commands.queue(move |world: &mut World| {
        if !draws(world) {
            return;
        }
        let Some(feet) = place(world, EffectAt::Entity(entity)) else { return };
        let way = killer.and_then(|killer| blow_way(world, killer, feet));
        lay_marks(world, &Happened { event: "died", point: feet, normal: None, way, seed: mark_seed(feet, Vec3::Y, 2) });
    });
}

/// A body part broke off: the effect and the marks of `part_broken`, at the part, along the blow.
pub(crate) fn on_part_broken(broken: On<PartBroken>, mut commands: Commands) {
    let (part, by) = (broken.part, broken.by);
    commands.queue(move |world: &mut World| {
        if !draws(world) {
            return;
        }
        let Some(at) = place(world, EffectAt::Entity(part)) else { return };
        let way = by.and_then(|by| blow_way(world, by, at));
        // `toward` is the way to the attacker, as for a hit: `aim: Hit` sends the chunks back, `aim: Away` on.
        play_event_effect(world, "part_broken", EffectAt::Point(at), way.map(|w| -w));
        lay_marks(world, &Happened { event: "part_broken", point: at, normal: None, way, seed: mark_seed(at, Vec3::Y, 3) });
    });
}

/// [`PlayMark`]: lays the mark.
pub(crate) fn on_play_mark(event: On<PlayMark>, mut commands: Commands) {
    let play = event.event().clone();
    commands.queue(move |world: &mut World| {
        let seed = mark_seed(play.point, play.normal, 0);
        spawn_mark_quietly(world, &play.mark, play.point, play.normal, seed, "PlayMark");
    });
}

/// A world built again (a region, a snapshot) or left (the main menu): the marks and the debris of the old one go.
pub(crate) fn clear_on_new_world(world: &mut World) {
    clear_all(world);
    if let Some(mut marks) = world.get_resource_mut::<Marks>() {
        marks.shots.clear();
    }
}
