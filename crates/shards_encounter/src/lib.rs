//! # shards_encounter — waves of enemies at a place
//!
//! An encounter is a group of enemies that come in waves where an *anchor* stands, start when the
//! players arrive, end when the last counted member falls, rewind when the group of players is
//! wiped, and reset when the players leave. The engine has the mechanism (anchors, waves, counts,
//! spawn points, budgets, limits, the start, wipe and reset rules, the events and the tools); a
//! game has every "what" (which prefabs, how many, the seconds, the radii, the messages) in
//! content.
//!
//! ```text
//! anchor entity ──► run (EncounterRuns) ──► Idle ► Armed ► Running ◄► Between
//!   EncounterAnchor     one per key             │                       │
//!   in a prefab, a      state, wave,            ▼                       ▼
//!   POI, a level node   members, timers       Resetting ◄── Wiped     Won
//! ```
//!
//! - **Anchors, not region code.** An encounter stands wherever an entity with
//!   [`EncounterAnchor`] stands: a point of interest's prefab, a tile's prefab piece, a prefab
//!   node of a level file, or a game's own [`place_encounter`] call. `shards_world` does not
//!   change.
//! - **Content.** `*.encounter.ron` ([`EncounterDef`]) and `*.encounter_rules.ron`
//!   ([`EncounterRulesDef`]). Budgets, role tags and tag limits are checked when content loads,
//!   per wave, and every error names the file, the encounter, the wave, the group and the fix.
//! - **Level-based counting.** The step polls; no observer changes gameplay. A member is out when
//!   it carries `Dead` or is gone, so every death counts: a hit, a status, a fall, a kill volume,
//!   a dev `kill`.
//! - **Participants.** Every player body is one, wherever it stands. They are *out* when dead, or
//!   when an out rule registered with [`EncounterAppExt::add_encounter_out_rule`] says so: the
//!   module registers `downed` itself (a body the action module's lethal hook brought down,
//!   `shards_action::Downed`), so a wipe counts every incapacitated player. When every participant
//!   is out the run is wiped.
//! - **The host runs it, clients see it.** The step is in `SimSet::Resolve`, which runs only where
//!   the simulation does. [`EncounterRuns`] and [`EncounterMember`] replicate; the umbrella sends
//!   the events as `net_event`s, keeps `NetAlways` on the members of a `net_always` fight and
//!   holds joiners while a `blocks_join` fight is on.
//! - **Snapshots and saves.** A snapshot resumes a fight tick for tick. A world save keeps only
//!   the keys of won `once` encounters; a load starts every other fight fresh.
//!
//! The state machine is a pure function ([`machine::advance`]) with its table in the module's
//! docs; the [step](step) observes the world, calls it and applies what it asks.
//!
//! ## Tools
//!
//! `encounters`, `encounter key=K`, `encounter_start key=K [wave=N]`, `encounter_reset
//! key=K|all [forget=true]`; conditions `encounter_active`, `encounter_between`,
//! `encounter_wiped`, `encounter_won` for `step --until named:...`; the NPC condition
//! `encounter_returning`.

mod anchor;
mod def;
mod events;
mod invariants;
pub mod machine;
mod run;
mod spawn;
mod step;
mod tools;

use ai_core::{AiAppExt, AiKind, Snapshot};
use bevy::prelude::*;
use shards_action::ActionSystems;
use shards_content::{ContentAppExt, ContentDb, ContentPlugin, PrefabDef};
use shards_core::EngineAppExt;
use shards_npc::NpcAppExt;
use shards_world::{ActiveRegion, RegionPart};

pub use anchor::{run_key, EncounterAnchor, EncounterRole};
pub use def::{
    AvoidBehind, CueDef, EncounterDef, EncounterRulesDef, Face, HomeAt, MemberGroup, OnReset, PullBack, SpawnAt, StartDef, TagLimit,
    TagNeeds, WaveDef,
};
pub use events::{
    EncounterCompleted, EncounterReset, EncounterResumed, EncounterStarted, EncounterWiped, ResetWhy, WaveCleared, WaveStarted,
    WaveWarned,
};
pub use run::{EncounterMember, EncounterRuns, Member, Run, RunState};
pub use step::{adopt, completed, reset as reset_encounter, restore_completed, start as start_encounter};

use ai_core::sim::SimSet;
use ai_core::DevCommand;

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{
        EncounterAnchor, EncounterAppExt, EncounterCompleted, EncounterDef, EncounterMember, EncounterPlugin, EncounterReset,
        EncounterResumed, EncounterRole, EncounterRuns, EncounterStarted, EncounterSystems, EncounterWiped, WaveCleared, WaveStarted,
        WaveWarned,
    };
}

/// The encounter module's system.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EncounterSystems {
    /// Anchors, runs, counting, waves, wipes and resets: one exclusive system in
    /// `SimSet::Resolve`, after `ActionSystems::Deaths`.
    Step,
}

/// A rule that says a participant is out.
pub type OutRule = fn(&World, Entity) -> bool;

/// A cleanup run before a silent despawn.
pub type DespawnHook = fn(&mut World, Entity);

/// What a game plugs into the module at build time. Not simulation state: registered when the app
/// is built, the same on every machine.
#[derive(Resource, Clone, Default)]
pub struct EncounterHooks {
    pub(crate) out_rules: Vec<(&'static str, OutRule)>,
    pub(crate) despawn: Vec<(&'static str, DespawnHook)>,
}

/// Registration API for games and for the engine's other modules.
pub trait EncounterAppExt {
    /// A way for a participant to be *out* besides being dead: the run counts it as fallen when
    /// `out(world, body)` holds. A downed state registers one; nothing else changes when it does.
    /// A second rule with the same name replaces the first.
    fn add_encounter_out_rule(&mut self, name: &'static str, out: fn(&World, Entity) -> bool) -> &mut Self;

    /// Cleans up what an entity owns before the module despawns it silently (a wipe, a reset, a
    /// run whose anchor went, a world save loaded): the projectiles and zones it put in the
    /// world, the summons it called. The hook runs for every member and add, in `AiId` order, with
    /// the entity still alive. A second hook with the same name replaces the first.
    fn add_encounter_despawn_hook(&mut self, name: &'static str, hook: fn(&mut World, Entity)) -> &mut Self;
}

impl EncounterAppExt for App {
    fn add_encounter_out_rule(&mut self, name: &'static str, out: fn(&World, Entity) -> bool) -> &mut Self {
        let mut hooks = self.world_mut().get_resource_or_init::<EncounterHooks>();
        hooks.out_rules.retain(|(n, _)| *n != name);
        hooks.out_rules.push((name, out));
        self
    }

    fn add_encounter_despawn_hook(&mut self, name: &'static str, hook: fn(&mut World, Entity)) -> &mut Self {
        let mut hooks = self.world_mut().get_resource_or_init::<EncounterHooks>();
        hooks.despawn.retain(|(n, _)| *n != name);
        hooks.despawn.push((name, hook));
        self
    }
}

/// The runs, or none.
pub fn runs(world: &World) -> &[Run] {
    world.get_resource::<EncounterRuns>().map_or(&[], |r| r.runs.as_slice())
}

/// The run keyed `key`.
pub fn run<'a>(world: &'a World, key: &str) -> Option<&'a Run> {
    world.get_resource::<EncounterRuns>()?.get(key)
}

/// The key of the run `entity` is a member of.
pub fn encounter_of(world: &World, entity: Entity) -> Option<&str> {
    world.get::<EncounterMember>(entity).map(|m| m.key.as_str())
}

/// Spawns a bare anchor for a game that places encounters from code instead of prefabs. It is a
/// region part when a region is active (it goes with the region), and the AI sees it.
pub fn place_encounter(world: &mut World, encounter: &str, key: Option<&str>, at: Transform) -> Result<Entity, String> {
    let db = world.get_resource::<ContentDb>().ok_or("content is not loaded")?;
    if db.peek::<EncounterDef>(encounter).is_none() {
        let ids: Vec<&str> = db.peek_ids::<EncounterDef>().collect();
        return Err(format!("no encounter `{encounter}`{}; encounters: {}", def::did_you_mean(encounter, ids.iter().copied()), ids.join(", ")));
    }
    if key.is_none() && db.peek::<EncounterDef>(encounter).is_some_and(|d| d.once) {
        return Err(format!("encounter `{encounter}` is `once`: pass a key (a won encounter is remembered by it)"));
    }
    let in_region = world.contains_resource::<ActiveRegion>();
    let mut entity = world.spawn((
        Name::new(format!("Encounter {encounter}")),
        AiKind::new("Encounter"),
        EncounterAnchor { encounter: encounter.to_owned(), key: key.map(str::to_owned) },
        at,
    ));
    if in_region {
        entity.insert(RegionPart);
    }
    Ok(entity.id())
}

/// Why joining waits: the first run, in key order, with `blocks_join` that is in `Running`,
/// `Between` or `Wiped`. The umbrella's join gate asks this; a game with a policy of its own
/// calls it from there.
pub fn join_blocked(world: &World) -> Option<String> {
    let runs = world.get_resource::<EncounterRuns>()?;
    let db = world.get_resource::<ContentDb>()?;
    runs.runs
        .iter()
        .find(|r| {
            matches!(r.state, RunState::Running | RunState::Between { .. } | RunState::Wiped { .. })
                && db.peek::<EncounterDef>(&r.encounter).is_some_and(|d| d.blocks_join)
        })
        .map(|r| format!("an encounter is under way ({}): you join when it ends", r.key))
}

/// The error for a run key that does not exist, with the closest key.
pub(crate) fn no_run(world: &World, key: &str) -> String {
    let keys: Vec<&str> = runs(world).iter().map(|r| r.key.as_str()).collect();
    if keys.is_empty() {
        return format!("no encounter run `{key}`; there are none (`encounters` lists them)");
    }
    format!("no encounter run `{key}`{}; runs: {} (`encounters` lists them)", def::did_you_mean(key, keys.iter().copied()), keys.join(", "))
}

/// FNV-1a over the state of every run, for the replay check `encounters`. `None` without the
/// resource: a game with no encounter has the replay hashes it had before the module.
fn runs_hash(world: &World) -> Option<u64> {
    let runs = world.get_resource::<EncounterRuns>()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for run in &runs.runs {
        eat(run.key.as_bytes());
        eat(&[0]);
        eat(run.encounter.as_bytes());
        eat(&[0]);
        for f in [run.at.x, run.at.y, run.at.z, run.yaw] {
            eat(&f.to_bits().to_le_bytes());
        }
        let (tag, a, b) = match &run.state {
            RunState::Idle => (0u8, 0, 0),
            RunState::Armed => (1, 0, 0),
            RunState::Running => (2, 0, 0),
            RunState::Between { left } => (3, *left, 0),
            RunState::Wiped { resume, pulled_back } => (4, resume.unwrap_or(u32::MAX), u32::from(*pulled_back)),
            RunState::Resetting { left } => (5, *left, 0),
            RunState::Won => (6, 0, 0),
        };
        eat(&[tag]);
        eat(&a.to_le_bytes());
        eat(&b.to_le_bytes());
        eat(&run.wave.to_le_bytes());
        eat(&run.absent.to_le_bytes());
        eat(&run.lock.map_or(0, |l| l.0).to_le_bytes());
        eat(&[run.noted, u8::from(run.begun)]);
        for m in &run.members {
            eat(&m.id.0.to_le_bytes());
            eat(&m.wave.to_le_bytes());
            eat(&[u8::from(m.counted)]);
        }
        eat(&[0xff]);
    }
    for key in &runs.completed {
        eat(key.as_bytes());
        eat(&[0]);
    }
    for text in &runs.reported {
        eat(text.as_bytes());
        eat(&[1]);
    }
    Some(h)
}

/// Snapshot prepare `encounters`: a snapshot that holds no runs removes the runs this world has
/// (their members were replaced by the load), instead of leaving stale ones whose members no
/// longer resolve and so count as out.
fn drop_absent(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    if snapshot.resources.contains_key(<EncounterRuns as TypePath>::type_path()) || !world.contains_resource::<EncounterRuns>() {
        return Ok(None);
    }
    // A removal does not replicate: a co-op host keeps an empty resource (see ).
    if world.get_resource::<shards_core::NetRole>().is_some_and(|r| *r == shards_core::NetRole::Host) {
        world.insert_resource(EncounterRuns::default());
    } else {
        world.remove_resource::<EncounterRuns>();
    }
    Ok(Some("encounter runs removed (the snapshot holds none)".to_owned()))
}

/// Encounters: the content kinds, the anchor and member components, the step, the tools, the
/// invariants. Needs `ContentPlugin`, `NpcPlugin` and `WorldPlugin` (`shards::EnginePlugins`
/// adds all).
pub struct EncounterPlugin;

impl Plugin for EncounterPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ContentPlugin>(),
            "EncounterPlugin needs ContentPlugin added first (shards::EnginePlugins adds both)"
        );
        app.register_type::<EncounterAnchor>()
            .register_type::<EncounterRole>()
            .register_type::<EncounterMember>()
            .register_type::<EncounterRuns>()
            .register_type::<Run>()
            .register_type::<Member>()
            .register_type::<RunState>()
            .register_type::<CueDef>()
            .register_type::<ResetWhy>()
            .register_type::<EncounterStarted>()
            .register_type::<WaveWarned>()
            .register_type::<WaveStarted>()
            .register_type::<WaveCleared>()
            .register_type::<EncounterCompleted>()
            .register_type::<EncounterWiped>()
            .register_type::<EncounterResumed>()
            .register_type::<EncounterReset>()
            .add_content::<EncounterDef>()
            .add_content::<EncounterRulesDef>()
            .add_content_check::<PrefabDef>("encounter anchors", anchor::check_anchor)
            .init_resource::<EncounterHooks>()
            // A downed player is out (B5): a fight where every player is down or dead is wiped.
            .add_encounter_out_rule("downed", |world, entity| world.get::<shards_action::Downed>(entity).is_some())
            // The runs are simulation state outside entities with an `AiId`; a snapshot mid-wave
            // resumes the fight exactly. A snapshot without runs removes this world's.
            .add_snapshot_resource::<EncounterRuns>()
            .add_snapshot_prepare_after("encounters", "region", &[], drop_absent)
            .add_optional_replay_check("encounters", runs_hash)
            .add_engine_module("encounter", VERSION)
            .add_ai_condition("encounter_active", tools::active)
            .add_ai_condition("encounter_between", tools::between)
            .add_ai_condition("encounter_wiped", tools::wiped)
            .add_ai_condition("encounter_won", tools::won)
            .add_npc_condition("encounter_returning", |ctx, _| {
                ctx.world.get::<EncounterMember>(ctx.entity).is_some_and(|m| m.returning)
            })
            .register_ai_describer("encounter", tools::describe_member)
            .add_ai_status("encounter", tools::status)
            .add_dev_entry("encounter", "encounters", "Encounters", DevCommand::Run("encounters".to_owned()))
            .add_dev_entry("encounter", "encounter_reset_all", "Reset encounters", DevCommand::Run("encounter_reset key=all".to_owned()))
            .journal_event::<EncounterStarted>(events::started)
            .journal_event::<WaveWarned>(events::warned)
            .journal_event::<WaveStarted>(events::wave_started)
            .journal_event::<WaveCleared>(events::cleared)
            .journal_event::<EncounterCompleted>(events::completed)
            .journal_event::<EncounterWiped>(events::wiped)
            .journal_event::<EncounterResumed>(events::resumed)
            .journal_event::<EncounterReset>(events::reset)
            .add_engine_invariant("encounter.members_tracked", invariants::members_tracked)
            .add_engine_invariant("encounter.state_sane", invariants::state_sane)
            .add_engine_invariant("encounter.lock_matches", invariants::lock_matches)
            .configure_sets(FixedUpdate, EncounterSystems::Step.in_set(SimSet::Resolve).after(ActionSystems::Deaths))
            .add_systems(FixedUpdate, step::step.in_set(EncounterSystems::Step));
        tools::register(app.world_mut());
    }
}
