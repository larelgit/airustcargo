//! # shards_fx — effects and telegraph decals (E10.3)
//!
//! What an effect looks like is content: `content/**/*.effect.ron` names effects ([`EffectDef`]:
//! how many particles, how long they live, how fast and which way they fly, gravity and drag,
//! size and colour over their life, a soft spot, a cube or a shard, glowing or not), and one
//! `*.fx.ron` ([`FxDef`]) says which effect each engine event plays (`damaged`, `blocked`,
//! `parried`, `died`, `healed`, …) and each cue of a clip. The engine plays them; a game plays its
//! own with [`PlayEffect`] or [`play`]. An event with an attacker (a hit, a guard, a parry) plays
//! on the side of the body that faces the attacker, and `aim: Hit` sends the particles back out
//! of it, towards the attacker; such an effect plays at that point and does not follow the body.
//!
//! - **Particles** are the engine's own (bevy_hanabi has no Bevy 0.20 release): each effect has a
//!   **pool** of `max_live` meshes made once, when content loads, and hidden while idle; a play
//!   takes free ones, and the oldest when all are taken. A particle's state is a function of its
//!   age (its flight under gravity and drag is solved, not stepped), on the **simulation's clock**:
//!   a pause freezes it, a `step` moves it on as many ticks as it ran, and real time runs it as
//!   the ticks go. All particles of an effect share one mesh and one material (a colour per
//!   particle in its `MeshTag`), so Bevy draws them in a few batches. Quads face the camera;
//!   cubes and shards tumble.
//! - **Telegraph decals**: the ground warning of an attack (E3) is a decal on the ground under
//!   it, as well as the gizmo lines: Bevy's `ForwardDecal` with the module's own zone material,
//!   which fills the zone's footprint (a disc, a cone's sector, a box's or a line's rectangle, a
//!   capsule's stadium) faintly, with stripes, and with a fill that grows from its middle as the
//!   hit nears, warm for an enemy's attack and cool for an ally's, as the gizmos colour them. The
//!   gizmos still draw the outline, the symbol and its stem; the hatch and the growing ring are
//!   the decal's (`TelegraphStyle::decals`). Decals need a depth prepass: the module gives one to
//!   every camera with `AiCamera` while decals are on. `telegraph_decals on=false` goes back to
//!   the gizmo fill.
//! - **Agents**: `effects` lists the effects, their pools and the decals; `effect_play` plays one
//!   at an entity or a point; `telegraph_decals` switches the decals (the developer menu has
//!   `effects` under world and the switch under combat); `ai/status` has
//!   `fx: {effects, live, decals, telegraph_decals}`.
//! - **The picture's, not the simulation's**: nothing here changes the world. Only while
//!   something is drawn (a window or `--offscreen`, feature `render`); headless no particle
//!   starts and the tools say so. Pools and decals are entities without `AiId`: replays,
//!   snapshots and co-op do not see them, and a co-op client plays the effects of the events
//!   its host forwards, as it sees them.
//! - **Checks**: `--check-content` finds ranges that are upside down or out of bounds, colours
//!   that are not `#rrggbb` or `#rrggbbaa`, a pool smaller than one play, an event the engine
//!   does not have, and events and cues that name an effect no file defines.

pub mod decal;
pub mod def;
#[cfg(feature = "render")]
pub mod render;
pub mod sim;
mod tools;

use std::collections::{BTreeMap, BTreeSet};

use ai_core::journal::{self, JournalKind};
use ai_core::AiAppExt;
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_action::{AnimationCue, Blocked, Damaged, Died, Evaded, GuardBroken, Healed, Parried, Staggered, Telegraph};
use shards_content::db::closest;
use shards_content::{ContentAppExt, ContentDb};
use shards_core::EngineAppExt;

pub use decal::{DecalShape, ZoneDecal};
pub use def::{Aim, EffectDef, FxDef, ParticleShape, EVENTS};
#[cfg(feature = "render")]
pub use render::{FxParticle, FxPrepass, ParticleMaterial, TelegraphDecal, ZoneDecalMaterial, ZoneMaterial};
pub use sim::{Particle, Pool, Shown};
pub use tools::{EffectPlayArgs, EffectsArgs, TelegraphDecalsArgs};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where an effect plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectAt {
    /// At an entity's feet (its `Transform`), plus the effect's `offset`. An effect with a `rate`
    /// follows it while it emits.
    Entity(Entity),
    /// At a point in the world, plus the effect's `offset`.
    Point(Vec3),
}

/// Plays effect `effect` (an [`EffectDef`] id) as soon as commands apply:
/// `commands.trigger(PlayEffect::new("coin_sparkle", EffectAt::Entity(chest)))`. Nothing plays
/// while nothing is drawn; an effect no content defines is journaled once.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct PlayEffect {
    pub effect: String,
    pub at: EffectAt,
    /// The way an effect with `aim: Hit` sends its particles (level, tilted up by its `lift`): for
    /// a hit, out of the struck side, back towards the attacker.
    pub toward: Option<Vec3>,
}

impl PlayEffect {
    pub fn new(effect: impl Into<String>, at: EffectAt) -> Self {
        Self { effect: effect.into(), at, toward: None }
    }

    /// The way an `aim: Hit` effect sends its particles.
    pub fn toward(mut self, direction: Vec3) -> Self {
        self.toward = Some(direction);
        self
    }
}

/// What the effects draw besides particles: the telegraph decals, on or off (`telegraph_decals`).
/// Off, the ground telegraphs are the gizmos' alone, hatch and fill included.
#[derive(Resource, Clone, Copy, Debug)]
pub struct FxSettings {
    pub decals: bool,
}

impl Default for FxSettings {
    fn default() -> Self {
        Self { decals: true }
    }
}

/// The effect systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FxSystems {
    /// `PostUpdate`, before transforms propagate, while something is drawn: the pools follow
    /// content, effects that emit over time emit, particles and decals are placed.
    Draw,
}

/// An effect being played: its definition as the pool was made for it, its colours, its pool.
#[derive(Clone, Debug)]
pub struct Playing {
    pub def: EffectDef,
    pub colors: ([u8; 4], [u8; 4]),
    pub pool: Pool,
}

impl Playing {
    /// A pool for `def`, empty.
    pub fn new(def: &EffectDef) -> Self {
        Self { def: def.clone(), colors: def.colors(), pool: Pool::new(def.max_live as usize) }
    }
}

/// An effect that goes on emitting at its `rate` after it started.
#[derive(Clone, Debug)]
pub struct Emitter {
    pub effect: String,
    pub at: EffectAt,
    pub aim: Vec3,
    /// The clock when it started.
    pub born: f64,
    pub rate: f32,
    /// Particles emitted so far, and in all.
    pub emitted: u32,
    pub total: u32,
}

/// The particles in flight, by effect, and the effects still emitting. Only while something is
/// drawn; nothing of it is the simulation's.
#[derive(Resource)]
pub struct Particles {
    pub effects: BTreeMap<String, Playing>,
    pub emitters: Vec<Emitter>,
    /// Effects played so far.
    pub played: u64,
    rng: sim::Rng,
    /// Effects named that no content defines, journaled once each.
    unknown: BTreeSet<String>,
}

impl Default for Particles {
    fn default() -> Self {
        Self { effects: BTreeMap::new(), emitters: Vec::new(), played: 0, rng: sim::Rng::new(0x5EED_F00D), unknown: BTreeSet::new() }
    }
}

impl Particles {
    /// Particles alive now, of every effect.
    pub fn live(&self) -> usize {
        self.effects.values().map(|p| p.pool.live()).sum()
    }
}

/// What [`play`] started.
#[derive(Clone, Debug, PartialEq)]
pub struct Played {
    /// Particles started now.
    pub particles: u32,
    /// Particles it adds over its `duration` (an effect with a `rate`).
    pub later: u32,
    /// Where it started (the entity's feet or the point, before the effect's offset).
    pub position: Vec3,
    /// Particles of the effect alive now, and its pool's size.
    pub live: usize,
    pub pool: usize,
}

/// The effects module: the `effect` and `fx` content kinds, [`PlayEffect`], the events' and
/// cues' effects, the tools and the status; with feature `render`, the pools, the particle and
/// decal materials and shaders, and the systems that draw them. Needs the engine core, content
/// and actions first; `shards::EnginePlugins` adds it after them.
pub struct FxPlugin;

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "FxPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<EffectDef>()
            .add_content::<FxDef>()
            .init_resource::<FxSettings>()
            .init_resource::<Particles>()
            .add_engine_module("fx", VERSION)
            .add_ai_status("fx", status)
            .add_dev_entry("world", "effects", "Effects, their pools and decals", ai_core::DevCommand::Run("effects".into()))
            .add_dev_entry(
                "combat",
                "telegraph_decals",
                "Telegraphs as decals on the ground",
                ai_core::DevCommand::Live(|w, _| Ok(ai_core::dev::toggle_line("telegraph_decals", w.resource::<FxSettings>().decals))),
            )
            .add_observer(on_play_effect)
            .add_observer(on_animation_cue)
            .add_observer(|e: On<Damaged>, c: Commands| event_effect(c, "damaged", e.target, Some(e.attacker)))
            .add_observer(|e: On<Blocked>, c: Commands| event_effect(c, "blocked", e.defender, Some(e.attacker)))
            .add_observer(|e: On<Parried>, c: Commands| event_effect(c, "parried", e.defender, Some(e.attacker)))
            .add_observer(|e: On<Evaded>, c: Commands| event_effect(c, "evaded", e.defender, Some(e.attacker)))
            .add_observer(|e: On<GuardBroken>, c: Commands| event_effect(c, "guard_broken", e.defender, Some(e.attacker)))
            .add_observer(|e: On<Staggered>, c: Commands| event_effect(c, "staggered", e.entity, None))
            .add_observer(|e: On<Died>, c: Commands| event_effect(c, "died", e.entity, None))
            .add_observer(|e: On<Healed>, c: Commands| event_effect(c, "healed", e.entity, None))
            .add_observer(|e: On<Telegraph>, c: Commands| event_effect(c, "telegraph", e.attacker, None));
        tools::register(app.world_mut());
        #[cfg(feature = "render")]
        render::plugin(app);
    }
}

/// Whether this game draws a picture: a window, or `--offscreen`.
pub fn draws(world: &World) -> bool {
    world.contains_resource::<ai_core::display::Offscreen>()
        || world.try_query_filtered::<(), With<bevy::window::Window>>().is_some_and(|mut q| q.iter(world).next().is_some())
}

/// The clock particles live by (s): the simulation's time, between ticks too. A pause stops it,
/// a `step` moves it on by the ticks it runs (they run the fixed schedule directly, without
/// frames), real time moves it as the ticks go.
pub fn clock(world: &World) -> f64 {
    world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.elapsed_secs_f64() + t.overstep().as_secs_f64())
}

/// The game's table of effects (the one `*.fx.ron`), if it has one.
pub fn fx_def(world: &World) -> Option<&FxDef> {
    world.get_resource::<ContentDb>().and_then(|db| db.iter::<FxDef>().next().map(|(_, def)| def))
}

/// Where `at` is now: an entity's feet (its `Transform`, its `GlobalTransform` under a parent).
fn place(world: &World, at: EffectAt) -> Option<Vec3> {
    match at {
        EffectAt::Point(p) => Some(p),
        EffectAt::Entity(e) => {
            let entity = world.get_entity(e).ok()?;
            if entity.contains::<ChildOf>() {
                entity.get::<GlobalTransform>().map(GlobalTransform::translation)
            } else {
                entity.get::<Transform>().map(|t| t.translation)
            }
        }
    }
}

/// Plays `effect` at `at` now, sending its particles `toward` for `aim: Hit`: its burst, and
/// what its `rate` adds over its `duration`. Errors: an effect no content defines (naming the
/// closest), an entity with no place, nothing drawn.
pub fn play(world: &mut World, effect: &str, at: EffectAt, toward: Option<Vec3>) -> Result<Played, String> {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    let Some(def) = db.get::<EffectDef>(effect).cloned() else {
        let known: Vec<&str> = db.iter::<EffectDef>().map(|(id, _)| id).collect();
        let hint = closest(effect, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no effect `{effect}`{hint}: content `*.effect.ron` defines effects; {} known", known.len()));
    };
    if !draws(world) {
        return Err(format!("nothing is drawn headless: effects play in a window or with --offscreen (effect `{effect}` is known)"));
    }
    let position = place(world, at).ok_or_else(|| match at {
        EffectAt::Entity(e) => format!("{} has no place in the world to play at", ai_core::label(world, e)),
        EffectAt::Point(_) => unreachable!("a point is a place"),
    })?;
    let now = clock(world);
    let aim = sim::aim_of(&def, toward);
    let mut particles = world.resource_mut::<Particles>();
    let Particles { effects, emitters, rng, played, .. } = &mut *particles;
    let playing = {
        let fresh = effects.get(effect).is_none_or(|p| p.def != def);
        if fresh {
            effects.insert(effect.to_owned(), Playing::new(&def));
        }
        effects.get_mut(effect).expect("just made")
    };
    for _ in 0..def.count {
        playing.pool.add(sim::spawn(&def, position, aim, now, rng));
    }
    let later = if def.rate > 0.0 && def.duration > 0.0 { (def.rate * def.duration).ceil() as u32 } else { 0 };
    if later > 0 {
        emitters.push(Emitter { effect: effect.to_owned(), at, aim, born: now, rate: def.rate, emitted: 0, total: later });
    }
    *played += 1;
    Ok(Played { particles: def.count, later, position, live: playing.pool.live(), pool: playing.pool.size() })
}

/// [`play`] for what the engine plays (an event, a cue, [`PlayEffect`]): nothing while nothing is
/// drawn; an unknown effect is journaled once and otherwise ignored.
pub fn play_quietly(world: &mut World, effect: &str, at: EffectAt, toward: Option<Vec3>, why: &str) {
    if !draws(world) {
        return;
    }
    if let Err(e) = play(world, effect, at, toward)
        && e.starts_with("no effect")
        && world.resource_mut::<Particles>().unknown.insert(effect.to_owned())
    {
        journal::emit(world, JournalKind::Warn, format!("fx: {why}: {e}"));
    }
}

/// The particles that effects with a `rate` emit up to clock `now`, each born at its own moment,
/// where its entity is now; an emitter whose entity is gone, or that the clock went back past
/// (a snapshot), stops.
pub fn emit_due(world: &mut World) {
    let now = clock(world);
    let emitters = std::mem::take(&mut world.resource_mut::<Particles>().emitters);
    let mut kept = Vec::new();
    for mut emitter in emitters {
        if emitter.born > now + 1e-6 {
            continue;
        }
        let Some(position) = place(world, emitter.at) else { continue };
        let def = world.get_resource::<ContentDb>().and_then(|db| db.get::<EffectDef>(&emitter.effect).cloned());
        let Some(def) = def else { continue };
        let due = (((now - emitter.born) * f64::from(emitter.rate)).floor() as u32).min(emitter.total);
        let mut particles = world.resource_mut::<Particles>();
        let Particles { effects, rng, .. } = &mut *particles;
        if let Some(playing) = effects.get_mut(&emitter.effect).filter(|p| p.def == def) {
            for i in emitter.emitted..due {
                let born = emitter.born + f64::from(i + 1) / f64::from(emitter.rate);
                playing.pool.add(sim::spawn(&def, position, emitter.aim, born, rng));
            }
        }
        emitter.emitted = due;
        if emitter.emitted < emitter.total {
            kept.push(emitter);
        }
    }
    world.resource_mut::<Particles>().emitters = kept;
}

fn on_play_effect(event: On<PlayEffect>, mut commands: Commands) {
    let PlayEffect { effect, at, toward } = event.event().clone();
    commands.queue(move |world: &mut World| play_quietly(world, &effect, at, toward, "PlayEffect"));
}

/// A clip's cue: the effect the table gives it, else the effect of its name, from the body.
fn on_animation_cue(cue: On<AnimationCue>, mut commands: Commands) {
    let AnimationCue { entity, cue, .. } = cue.event().clone();
    commands.queue(move |world: &mut World| {
        if !draws(world) {
            return;
        }
        let mapped = fx_def(world).and_then(|fx| fx.cues.get(&cue).cloned());
        let named = || world.resource::<ContentDb>().get::<EffectDef>(&cue).is_some().then(|| cue.clone());
        if let Some(effect) = mapped.or_else(named) {
            play_quietly(world, &effect, EffectAt::Entity(entity), None, "a clip's cue");
        }
    });
}

/// The effect the table gives engine event `event` at `entity`; with an attacker (`from`), on the
/// side of `entity`'s body that faces it, aimed back out towards it.
fn event_effect(mut commands: Commands, event: &'static str, entity: Entity, from: Option<Entity>) {
    commands.queue(move |world: &mut World| {
        if !draws(world) {
            return;
        }
        let Some(effect) = fx_def(world).and_then(|fx| fx.effects.get(event).cloned()) else { return };
        // On a co-op client an event may name an entity it does not have: nothing to play at.
        let Some(at) = place(world, EffectAt::Entity(entity)) else { return };
        let (at, toward) = match from.and_then(|from| place(world, EffectAt::Entity(from))) {
            Some(attacker) => struck_side(world, entity, at, attacker),
            None => (EffectAt::Entity(entity), None),
        };
        play_quietly(world, &effect, at, toward, &format!("the table's `{event}`"));
    });
}

/// Where a blow from `attacker` meets `entity` (standing at `at`): the side of its body (its
/// collider's width) that faces the attacker, and the way out of it, back towards the attacker.
/// Right on top of each other, the body itself.
fn struck_side(world: &World, entity: Entity, at: Vec3, attacker: Vec3) -> (EffectAt, Option<Vec3>) {
    let Some(out) = Vec3::new(attacker.x - at.x, 0.0, attacker.z - at.z).try_normalize() else {
        return (EffectAt::Entity(entity), None);
    };
    let half = world.get::<shards_motion::Collider>(entity).map_or(Vec3::ZERO, |c| c.shape.half_extents());
    (EffectAt::Point(at + out * half.x.min(half.z)), Some(out))
}

/// Telegraph decals on the ground now (none without feature `render`).
pub fn decals(world: &World) -> usize {
    #[cfg(feature = "render")]
    return render::decals_shown(world);
    #[cfg(not(feature = "render"))]
    {
        let _ = world;
        0
    }
}

/// `ai/status` → `fx: {effects, live, decals, telegraph_decals}`.
fn status(world: &World) -> Value {
    json!({
        "effects": world.get_resource::<ContentDb>().map_or(0, |db| db.ids::<EffectDef>().count()),
        "live": world.get_resource::<Particles>().map_or(0, Particles::live),
        "decals": decals(world),
        "telegraph_decals": world.get_resource::<FxSettings>().is_none_or(|s| s.decals),
    })
}
