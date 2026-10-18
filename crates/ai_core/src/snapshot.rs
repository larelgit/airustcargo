//! Snapshots: the simulation state at one tick, in a file, and back.
//!
//! `snapshot_save` writes everything a later `step` needs to continue exactly like this
//! session would; `snapshot_load` puts it back, in this game or in a fresh one built the
//! same way ("go to boss phase" is a snapshot loaded by name). A replay may start from one
//! ([`Replay::start`](crate::replay::Replay)): see [`replay`](crate::replay).
//!
//! ## What a snapshot holds, and what it does not
//!
//! Every item of simulation state is one of four things:
//!
//! - **Saved.** Every entity with an [`AiId`], under its id, with each reflected component a
//!   game or an engine module defines (and `Transform` and `Name`; not Bevy's render, UI and
//!   hierarchy types, which a window adds again). Entity references inside components are
//!   written as `e12` (an `AiId`) or `c7` (a key a module registered with
//!   [`add_snapshot_entity_key`](crate::AiAppExt::add_snapshot_entity_key): the motion
//!   module names colliders without ids that way), so they survive the entities being
//!   spawned again. Resources the modules register with
//!   [`add_snapshot_resource`](crate::AiAppExt::add_snapshot_resource) (`ColliderIds`), the
//!   tick, the `AiId` counter and the tombstones.
//! - **Rebuilt.** Caches derived from the above (`PhysicsWorld`, the remembered input values
//!   of the replay recorder) are made again after a load
//!   ([`add_snapshot_rebuild`](crate::AiAppExt::add_snapshot_rebuild)). Parts of the world a
//!   module builds from a saved resource (a generated region from its manifest) are put back
//!   *before* the checks below ([`add_snapshot_prepare`](crate::AiAppExt::add_snapshot_prepare)),
//!   so a snapshot taken in a region loads into a game that has another region, or none. A
//!   load refused after that puts the world back as it was ([`restore`]).
//! - **Checked.** What the world is built *around*, not what it holds: the content database,
//!   the static colliders. A snapshot records a hash of each
//!   ([`add_snapshot_check`](crate::AiAppExt::add_snapshot_check)) and refuses to load
//!   where it differs, saying which (`force` loads anyway).
//! - **Not state.** Configuration (tools, hooks, registries), the journal, agent sessions
//!   (they name entities by id), what only a window shows. The seed and tick rate are in
//!   the header and must match.
//!
//! State that lives outside entities with an `AiId` and is none of the above is the one
//! trap. A load ends by saving the restored world again and comparing it with the file, and
//! by comparing [`state_hash`](crate::replay::state_hash) (which includes every
//! [`add_replay_check`](crate::AiAppExt::add_replay_check)): a difference is reported with
//! the first places it shows, instead of a continuation that quietly goes another way.
//!
//! ## Why not `DynamicWorld`
//!
//! Bevy's scene serializer (`bevy_world_serialization`) is the same reflection layer
//! underneath, and this module uses the same pieces (`TypedReflectSerializer`,
//! `ReflectComponent::apply_or_insert_mapped`). It does not fit as is: it names entities by
//! `Entity` bits, which mean nothing in another process, where here ids are `AiId`s;
//! it inserts components in archetype order, where an `AiId` must be in place before the
//! component hooks run; and its file would not be byte-stable. So a snapshot is the
//! same data in an envelope of our own.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use bevy::prelude::*;
use bevy::reflect::{FromReflect, GetTypeRegistration, Reflect, TypePath, TypeRegistry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{AiId, AiImportance, AiIndex, AiLandmark, AiPlayer, AiVisible, Tombstone};
use crate::journal::{self, JournalKind};
use crate::rng::SimSeed;
use crate::session::Scope;
use crate::sim::{self, SimGate};
use crate::state::{self, from_json, to_json, Decoder, Encoder, StateChanges, StateEncoder, StateRule};
use crate::spatial::AiOccluder;
use crate::tools::{register_tool, Call, ToolKind, ToolResult};
use crate::replay::ReplayCheckParts;
use crate::{describe::AiKind, replay};

/// Format tag written into every snapshot file. Version 2 lists the parts of the checks with
/// parts (`check_parts`: the definitions the session read, for `content`), whose value then
/// hashes those parts: a build that does not know that refuses the file by its tag. A file
/// without the list for a check (every version 1 file) is compared by that check's whole state.
pub const FORMAT: &str = "shards-snapshot/2";

/// Snapshots before [`FORMAT`] 2: loaded as they are, every check compared whole.
const FORMAT_V1: &str = "shards-snapshot/1";

/// Whether this build loads snapshots of `format`.
fn readable(format: &str) -> bool {
    format == FORMAT || format == FORMAT_V1
}

/// Folder for saved snapshots, relative to the game's working directory. Tools read and
/// write nothing else (see [`paths`](crate::paths)).
pub const SNAPSHOTS_DIR: &str = "snapshots";

const SNAPSHOT_PATH_EXAMPLE: &str = "snapshots/boss_phase2.json";

/// The engine's version, written into snapshots (all crates share it).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

// --- the file -------------------------------------------------------------------------

/// The simulation state at one tick. Everything is in order (entities by id, maps by
/// key), so the same state gives the same bytes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// [`FORMAT`].
    pub format: String,
    /// Ticks run so far.
    pub tick: u64,
    /// The seed of the game it was taken in (random streams are keyed by it).
    pub seed: u64,
    pub tick_hz: f64,
    /// The engine version that wrote it.
    pub engine: String,
    /// [`state_hash`](crate::replay::state_hash) at this tick: what a load must reproduce.
    #[serde(with = "crate::replay::hex_u64")]
    pub state_hash: u64,
    /// Hashes of what the world is built around (hex), by check name: content, static
    /// colliders. A load refuses where they differ.
    pub checks: BTreeMap<String, String>,
    /// The parts of each check with parts (hex hashes by part name: `content` lists the
    /// definitions the session read). A load compares the parts, and names the ones that differ.
    /// A check with parts that has no list here was taken whole, and is compared whole.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub check_parts: BTreeMap<String, BTreeMap<String, String>>,
    /// The `AiId` counter and tombstones.
    pub ids: IdState,
    /// Registered resources, by type path.
    pub resources: BTreeMap<String, Value>,
    /// Every entity with an `AiId`, in id order.
    pub entities: Vec<EntitySnapshot>,
    /// Components that could not be written (they do not reflect or do not serialize), so
    /// a load can say what it cannot bring back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<String>,
}

/// [`AiIndex`] as far as it matters after a load.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IdState {
    /// The next id to hand out.
    pub next: u32,
    pub graveyard: Vec<Tombstone>,
}

/// One entity: its id and its components by type path.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct EntitySnapshot {
    pub id: u32,
    pub components: BTreeMap<String, Value>,
}

impl Snapshot {
    /// `tick 600, 14 entities, 1 resource`.
    pub fn summary(&self) -> String {
        let (n, r) = (self.entities.len(), self.resources.len());
        let entities = if n == 1 { "entity" } else { "entities" };
        let resources = if r == 1 { "resource" } else { "resources" };
        format!("tick {}, {n} {entities}, {r} {resources}", self.tick)
    }

    /// Reads a snapshot file.
    pub fn read(path: &std::path::Path) -> Result<Snapshot, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let snapshot: Snapshot =
            serde_json::from_str(&text).map_err(|e| format!("{} is not a snapshot: {e}", path.display()))?;
        if !readable(&snapshot.format) {
            return Err(format!(
                "{} is not a snapshot this build can load: format `{}`, expected `{FORMAT}` (or `{FORMAT_V1}`)",
                path.display(),
                snapshot.format
            ));
        }
        Ok(snapshot)
    }

    /// Writes the snapshot to `requested`, a path inside `snapshots/`; returns where it went.
    pub fn write(&self, requested: &str) -> Result<std::path::PathBuf, String> {
        let path = crate::paths::inside(SNAPSHOTS_DIR, requested, SNAPSHOT_PATH_EXAMPLE)?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text + "\n").map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }
}

// --- what modules register ------------------------------------------------------------

/// How a module names entities that have no `AiId`, so references to them survive a load:
/// `c7` for the collider with `ColliderId(7)`.
#[derive(Clone, Copy)]
pub struct EntityKey {
    /// Lowercase letters, not `e` (that is the `AiId`): the reference is written
    /// `<prefix><number>`.
    pub prefix: &'static str,
    /// The number of `entity`, if this module knows it by one.
    pub encode: fn(&World, Entity) -> Option<u32>,
    /// Every entity this module can name, with its number: the ones that stay in the world
    /// when a snapshot is loaded (those without an `AiId`).
    pub index: fn(&mut World) -> Vec<(u32, Entity)>,
}

#[derive(Clone, Copy)]
pub(crate) struct SnapshotResource {
    pub(crate) name: &'static str,
    pub(crate) type_id: TypeId,
    pub(crate) save: fn(&World, &TypeRegistry, &Encoder) -> Option<Result<Value, String>>,
    pub(crate) load: fn(&mut World, &TypeRegistry, &Value, &mut Decoder) -> Result<(), String>,
}

/// Runs after a load: a name for the journal and the code.
type Rebuild = (&'static str, fn(&mut World));

/// What a [`SnapshotPrepare`] does: `Ok(Some(note))` when it changed the world (the note goes
/// into the load's report), `Ok(None)` when the world already was what the snapshot needs,
/// `Err` when it cannot be (whatever it changed before it failed, [`restore`] puts back).
pub type PrepareFn = fn(&mut World, &Snapshot) -> Result<Option<String>, String>;

/// Runs before a snapshot is checked against the game: brings a part of the world that a
/// module builds from one of its saved resources to what the snapshot holds (a generated
/// region from its manifest), so that the checks it `covers` compare like with like.
#[derive(Clone, Copy)]
struct SnapshotPrepare {
    name: &'static str,
    /// Registered checks that only mean something once this ran (`static_colliders`).
    covers: &'static [&'static str],
    run: PrepareFn,
    /// The prepare this one runs right after, wherever the two were registered.
    after: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct SnapshotCheck {
    name: &'static str,
    hint: &'static str,
    /// The value; for a check with parts, the one it had before it had them.
    check: fn(&World) -> u64,
    parts: Option<ReplayCheckParts>,
}

impl SnapshotCheck {
    /// The parts of this check now, unless it has none or this session computes it whole.
    fn parts_now(&self, world: &World) -> Option<Vec<(String, u64)>> {
        let parts = self.parts?;
        let whole = world.get_resource::<replay::ReplayHashing>().is_some_and(|h| h.hashes_whole(self.name));
        (!whole).then(|| parts(world))
    }
}

/// The parts of check `name` as a snapshot keeps them (hex hashes), read back; a hash that does
/// not read is an error naming it: the file was damaged or edited by hand.
fn read_parts(name: &str, saved: &BTreeMap<String, String>) -> Result<BTreeMap<String, u64>, String> {
    saved
        .iter()
        .map(|(part, hex)| {
            let hash = replay::parse_hex(hex).map_err(|e| format!("the snapshot's check `{name}` does not read: part `{part}`: {e}"))?;
            Ok((part.clone(), hash))
        })
        .collect()
}

/// What modules registered with [`AiAppExt`](crate::AiAppExt)'s `add_snapshot_*`.
#[derive(Resource, Default, Clone)]
pub struct SnapshotRegistry {
    pub(crate) resources: Vec<SnapshotResource>,
    checks: Vec<SnapshotCheck>,
    prepares: Vec<SnapshotPrepare>,
    rebuilds: Vec<Rebuild>,
    pub(crate) keys: Vec<EntityKey>,
}

impl SnapshotRegistry {
    pub(crate) fn add_resource<R>(&mut self)
    where
        R: Resource + Reflect + FromReflect + TypePath,
    {
        let name = R::type_path();
        self.resources.retain(|r| r.name != name);
        self.resources.push(SnapshotResource { name, type_id: TypeId::of::<R>(), save: save_resource::<R>, load: load_resource::<R> });
    }

    /// The saved resources, by type and type path, in registration order.
    pub fn resource_types(&self) -> impl Iterator<Item = (TypeId, &'static str)> + '_ {
        self.resources.iter().map(|r| (r.type_id, r.name))
    }

    pub(crate) fn add_check(&mut self, name: &'static str, hint: &'static str, check: fn(&World) -> u64) {
        self.checks.retain(|c| c.name != name);
        self.checks.push(SnapshotCheck { name, hint, check, parts: None });
    }

    /// A check with parts: the snapshot keeps them, a load compares them part by part.
    pub(crate) fn add_parts_check(&mut self, name: &'static str, hint: &'static str, parts: ReplayCheckParts, whole: fn(&World) -> u64) {
        self.checks.retain(|c| c.name != name);
        self.checks.push(SnapshotCheck { name, hint, check: whole, parts: Some(parts) });
    }

    pub(crate) fn add_prepare(&mut self, name: &'static str, after: Option<&'static str>, covers: &'static [&'static str], run: PrepareFn) {
        self.prepares.retain(|p| p.name != name);
        self.prepares.push(SnapshotPrepare { name, covers, run, after });
    }

    /// The prepares in the order they run: registration order, except that one registered to
    /// run after another comes right after it (after the ones that came there before it). One
    /// whose `after` is not registered runs where it was registered.
    fn prepare_order(&self) -> Vec<SnapshotPrepare> {
        let known = |name: &str| self.prepares.iter().any(|p| p.name == name);
        let mut order: Vec<SnapshotPrepare> = self.prepares.iter().filter(|p| !p.after.is_some_and(known)).copied().collect();
        let mut left: Vec<SnapshotPrepare> = self.prepares.iter().filter(|p| p.after.is_some_and(known)).copied().collect();
        // A chain (`c` after `b` after `a`) places one link a round; a cycle never places, and runs last.
        while !left.is_empty() {
            let before = left.len();
            left.retain(|p| {
                let Some(at) = order.iter().position(|q| Some(q.name) == p.after) else { return true };
                // Past the ones placed after it already, and theirs.
                let (mut end, mut placed) = (at + 1, vec![order[at].name]);
                while order.get(end).is_some_and(|q| q.after.is_some_and(|a| placed.contains(&a))) {
                    placed.push(order[end].name);
                    end += 1;
                }
                order.insert(end, *p);
                false
            });
            if left.len() == before {
                order.append(&mut left);
            }
        }
        order
    }

    pub(crate) fn add_rebuild(&mut self, name: &'static str, rebuild: fn(&mut World)) {
        self.rebuilds.retain(|(n, _)| *n != name);
        self.rebuilds.push((name, rebuild));
    }

    pub(crate) fn add_key(&mut self, key: EntityKey) {
        assert!(
            !key.prefix.is_empty() && key.prefix != "e" && key.prefix.bytes().all(|b| b.is_ascii_lowercase()),
            "a snapshot entity key is lowercase letters and not `e` (that is the AiId): `{}`",
            key.prefix
        );
        self.keys.retain(|k| k.prefix != key.prefix);
        self.keys.push(key);
    }
}

fn save_resource<R: Resource + Reflect>(world: &World, registry: &TypeRegistry, encoder: &Encoder) -> Option<Result<Value, String>> {
    let resource = world.get_resource::<R>()?;
    Some(to_json(resource.as_partial_reflect(), registry, encoder))
}

fn load_resource<R: Resource + Reflect + FromReflect + TypePath>(
    world: &mut World,
    registry: &TypeRegistry,
    value: &Value,
    decoder: &mut Decoder,
) -> Result<(), String> {
    let registration = registry.get(TypeId::of::<R>()).ok_or_else(|| format!("{} is not registered for reflection", R::type_path()))?;
    let boxed = from_json(registration, registry, value, decoder)?;
    let resource = R::from_reflect(boxed.as_ref()).ok_or_else(|| format!("cannot build {} from the snapshot", R::type_path()))?;
    world.insert_resource(resource);
    Ok(())
}

// --- which components ------------------------------------------------------------------

/// Whether a component type belongs in a snapshot: everything a game or an engine module
/// defines, plus `Transform` and `Name`; not Bevy's own types (a window adds meshes,
/// visibility and hierarchy; they are made again from what is saved), and not `AiId`,
/// which is the entity's key.
pub fn is_snapshot_type(type_id: TypeId, type_path: &str) -> bool {
    type_id != TypeId::of::<AiId>()
        && (type_id == TypeId::of::<Transform>() || type_id == TypeId::of::<Name>() || !type_path.starts_with("bevy_"))
}

// --- saving ----------------------------------------------------------------------------

/// The simulation state as it is now. Reads only; the world is not touched.
pub fn capture(world: &World) -> Result<Snapshot, String> {
    let index = world.get_resource::<AiIndex>().ok_or("this world has no AiIndex: AiCorePlugin is not added")?;
    let modules = world.get_resource::<SnapshotRegistry>().cloned().unwrap_or_default();
    let encoder = StateEncoder::new(world, StateRule::Snapshot);

    let mut left_out = BTreeSet::new();
    let entities: Vec<EntitySnapshot> = index
        .live()
        .into_iter()
        .map(|(id, entity)| EntitySnapshot { id: id.0, components: encoder.encode(entity, &mut left_out) })
        .collect();
    let resources = encoder.encode_resources(&mut left_out);

    // Hashing only reads: what it reads is not the simulation's.
    let _quiet = crate::scope::suspend(world);
    let mut checks = BTreeMap::new();
    let mut check_parts = BTreeMap::new();
    for check in &modules.checks {
        let value = match check.parts_now(world) {
            Some(parts) => {
                let value = replay::parts_hash(&parts);
                check_parts.insert(check.name.to_owned(), parts.into_iter().map(|(name, hash)| (name, replay::hex(hash))).collect());
                value
            }
            None => (check.check)(world),
        };
        checks.insert(check.name.to_owned(), format!("{value:016x}"));
    }
    Ok(Snapshot {
        format: FORMAT.to_owned(),
        tick: sim::tick(world),
        seed: world.get_resource::<SimSeed>().map_or(0, |s| s.0),
        tick_hz: 1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64(),
        engine: ENGINE_VERSION.to_owned(),
        state_hash: replay::state_hash(world),
        checks,
        check_parts,
        ids: IdState { next: index.next_id(), graveyard: index.graveyard().cloned().collect() },
        resources,
        entities,
        left_out: left_out.into_iter().collect(),
    })
}

// --- loading ---------------------------------------------------------------------------

/// What a load did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReport {
    pub tick: u64,
    pub entities: usize,
    pub resources: usize,
    /// Things that were not as the snapshot had them but did not stop the load (a forced
    /// load past a mismatch, references to nothing, components this game does not know).
    pub warnings: Vec<String>,
    /// Where the world, saved again after the load, differs from the snapshot: empty when
    /// the restore is exact.
    pub differences: Vec<String>,
}

impl fmt::Display for LoadReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (n, r) = (self.entities, self.resources);
        let entities = if n == 1 { "entity" } else { "entities" };
        let resources = if r == 1 { "resource" } else { "resources" };
        write!(f, "tick {}, {n} {entities}, {r} {resources}", self.tick)?;
        for warning in &self.warnings {
            write!(f, "; {warning}")?;
        }
        Ok(())
    }
}

impl LoadReport {
    /// The restore was exact.
    pub fn exact(&self) -> bool {
        self.differences.is_empty()
    }

    /// Why a restore that was not exact cannot be trusted, for an error message.
    pub fn explain_differences(&self) -> String {
        let shown: Vec<&str> = self.differences.iter().take(5).map(String::as_str).collect();
        let more = self.differences.len().saturating_sub(shown.len());
        let mut text = format!(
            "the restored state differs from the snapshot in {} place(s): {}",
            self.differences.len(),
            shown.join("; ")
        );
        if more > 0 {
            text.push_str(&format!("; … (+{more})"));
        }
        text.push_str(
            ". Either the snapshot was taken by a build that reads this state differently, or some state is \
             neither on an entity with an AiId nor registered: save a resource with add_snapshot_resource, \
             rebuild a cache with add_snapshot_rebuild, check a fixed part with add_snapshot_check",
        );
        text
    }
}

/// Puts `snapshot` into `world`: every entity with an `AiId` is replaced by the snapshot's
/// (under the same ids), the registered resources, the tick and the id counter are set, the
/// caches are rebuilt. A later `step` continues as the original session did.
///
/// Refuses, changing nothing, when the snapshot does not fit this game: another format,
/// tick rate, seed or engine version, content or static colliders that differ, components
/// the game does not have. `force` loads past all of these except the format and the tick
/// rate (then the entities are the snapshot's, the rest is this game's, and the continuation
/// is not the original's).
///
/// The checks a prepare covers (`static_colliders` after the region is built again) can only
/// refuse once the prepare has changed the world. So the world is taken first, and a load
/// refused after a prepare ran (or by a prepare that failed half way) puts it back: the same
/// entities under the same ids, the same region, counters and resources, the journal without
/// the lines of the attempt and with one that says the load was refused. No [`WorldRebuilt`]
/// follows: id for id, nothing changed.
///
/// The caller handles the replay recorder (`snapshot_load` restarts the recording from the
/// snapshot; playback does the same for a replay's own start).
///
/// A restore that is not exact (the world, saved again, differs from the snapshot) is still
/// a load: [`LoadReport::differences`] says where, and the caller decides what that means (a
/// replay's start reports the divergence and plays on). [`restore_exact`] is the one that
/// refuses it.
pub fn restore(world: &mut World, snapshot: &Snapshot, force: bool) -> Result<LoadReport, String> {
    restore_with(world, snapshot, force, false)
}

/// [`restore`] that loads the snapshot exactly or not at all: where the restored world would
/// differ from the snapshot, the world is put back as it was before (as for a load refused
/// after a prepare, including the tick), no [`WorldRebuilt`] follows, and the error says where
/// it differed and what to register. For a caller that goes on only with the state it asked
/// for (`snapshot_load`).
pub fn restore_exact(world: &mut World, snapshot: &Snapshot, force: bool) -> Result<LoadReport, String> {
    restore_with(world, snapshot, force, true)
}

fn restore_with(world: &mut World, snapshot: &Snapshot, force: bool, exact: bool) -> Result<LoadReport, String> {
    // A load puts a state back: what it reads (a region built again, caches) is the snapshot's,
    // not the simulation's.
    let _quiet = crate::scope::suspend(world);
    let mut warnings = Vec::new();
    let modules = world.get_resource::<SnapshotRegistry>().cloned().unwrap_or_default();
    let covered: BTreeSet<&str> = modules.prepares.iter().flat_map(|p| p.covers.iter().copied()).collect();
    let mut forced = check_compatible(world, snapshot, force, &mut warnings, &|name| !covered.contains(name))?;
    let app_registry = world.resource::<AppTypeRegistry>().clone();

    let skipped = {
        let registry = app_registry.read();
        check_fits(&registry, &modules, snapshot, force, &mut warnings)?
    };

    // What modules build from their saved resources (a region from its manifest) comes
    // first: the checks it covers compare the world built so with the snapshot's. Both may
    // still refuse, after the world changed: then it is put back as it was before. So is a
    // restore that turns out not to be exact, when the caller wants it exact.
    let before = if modules.prepares.is_empty() && !exact { None } else { Some(Before::take(world)?) };
    match prepare_and_check(world, snapshot, force, &modules, &covered, &mut warnings) {
        Ok(late) => forced |= late,
        Err(why) => {
            return Err(match before {
                Some(before) => {
                    let reason = first_line(&why).to_owned();
                    before.put_back(world, &modules, snapshot.tick, &reason, why)
                }
                None => why,
            });
        }
    }

    replace_world(world, snapshot, &skipped, &mut warnings);
    sim::set_tick(world, snapshot.tick);
    for (_, rebuild) in &modules.rebuilds {
        rebuild(world);
    }
    // A check with parts the snapshot does not list was taken whole: this session computes it so
    // from here on, as the session it was taken in did (and by its parts where it lists them).
    let whole_before = world.get_resource_or_init::<replay::ReplayHashing>().whole_names();
    world.resource_mut::<replay::ReplayHashing>().follow_file(|name| snapshot.check_parts.contains_key(name));

    // Save it again: the restored world must say what the file says. A forced load past a
    // mismatch is not meant to reproduce the original's hash.
    let differences = compare_with(world, snapshot, &skipped, !forced);
    if exact && !differences.is_empty() {
        let reason = format!("the restored state differs from the snapshot in {} place(s)", differences.len());
        let why = LoadReport { differences, ..Default::default() }.explain_differences();
        // Taken above whenever `exact`.
        world.resource_mut::<replay::ReplayHashing>().set_whole_names(whole_before);
        let Some(before) = before else { return Err(why) };
        // The restore moved the clock too.
        sim::set_tick(world, before.world.tick);
        let refused = before.put_back(world, &modules, snapshot.tick, &reason, why);
        journal::log(world, JournalKind::Error, format!("snapshot of tick {}: {refused}", snapshot.tick));
        return Err(refused);
    }
    world_rebuilt(world, format!("snapshot of tick {}", snapshot.tick));
    Ok(LoadReport {
        tick: snapshot.tick,
        entities: snapshot.entities.len(),
        resources: snapshot.resources.len(),
        warnings,
        differences,
    })
}

/// The prepares, then the checks they cover. Returns whether a mismatch was forced past.
fn prepare_and_check(
    world: &mut World,
    snapshot: &Snapshot,
    force: bool,
    modules: &SnapshotRegistry,
    covered: &BTreeSet<&str>,
    warnings: &mut Vec<String>,
) -> Result<bool, String> {
    for prepare in &modules.prepare_order() {
        match (prepare.run)(world, snapshot) {
            Ok(Some(note)) => warnings.push(format!("{}: {note}", prepare.name)),
            Ok(None) => {}
            Err(why) => return Err(format!("cannot put back the {} the snapshot was taken in: {why}", prepare.name)),
        }
    }
    if covered.is_empty() {
        return Ok(false);
    }
    let mut later = Vec::new();
    let forced = check_compatible(world, snapshot, force, &mut later, &|name| covered.contains(name))?;
    for warning in later {
        if !warnings.contains(&warning) {
            warnings.push(warning);
        }
    }
    Ok(forced)
}

/// Where the world, saved again, differs from `wanted` (with the state hash when `hash`).
fn compare_with(world: &World, wanted: &Snapshot, skipped: &Skipped, hash: bool) -> Vec<String> {
    match capture(world) {
        Ok(again) => {
            let mut differences = differences_between(wanted, &again, skipped);
            if differences.is_empty() && hash && again.state_hash != wanted.state_hash {
                differences.push(format!(
                    "the state hash is {:016x}, the snapshot's {:016x}: a replay check covers state that is not \
                     on an entity with an AiId",
                    again.state_hash, wanted.state_hash
                ));
            }
            differences
        }
        Err(why) => vec![format!("cannot save the restored world again: {why}")],
    }
}

/// The world before a load's prepares ran, to put back when the load is refused after them.
struct Before {
    world: Snapshot,
    /// Which entity each id was: the replay recorder's input memories follow them.
    entities: Vec<(AiId, Entity)>,
    /// The journal then: the lines the attempt writes are taken back, and the oldest it pushed
    /// out of a full ring brought back.
    journal: Option<journal::JournalMark>,
}

impl Before {
    fn take(world: &mut World) -> Result<Before, String> {
        // Lines logged before the load belong before it.
        journal::drain_logs(world);
        Ok(Before {
            world: capture(world)?,
            entities: world.resource::<AiIndex>().live(),
            journal: world.get_resource::<journal::AiJournal>().map(journal::AiJournal::mark),
        })
    }

    /// Puts the world back (the prepares undo what they built, from the saved world's own
    /// resources: the region it had, or none), says so once in the journal (`reason` is why
    /// the load was refused, in a few words), and returns the load's error `why`, with what
    /// could not be put back if anything.
    fn put_back(self, world: &mut World, modules: &SnapshotRegistry, tick: u64, reason: &str, why: String) -> String {
        let mut failed = Vec::new();
        for prepare in &modules.prepare_order() {
            if let Err(e) = (prepare.run)(world, &self.world) {
                failed.push(format!("{}: {e}", prepare.name));
            }
        }
        replace_world(world, &self.world, &Skipped::default(), &mut Vec::new());
        for (_, rebuild) in &modules.rebuilds {
            rebuild(world);
        }
        let index = world.resource::<AiIndex>();
        let moved: bevy::platform::collections::HashMap<Entity, Entity> =
            self.entities.iter().filter_map(|(id, old)| Some((*old, index.get(*id)?))).collect();
        replay::follow_entities(world, &moved);
        failed.extend(compare_with(world, &self.world, &Skipped::default(), true));

        // The lines of the attempt and of its undoing told of regions that came and went
        // again: one line says what happened instead.
        journal::drain_logs(world);
        if let Some(mark) = self.journal
            && let Some(mut lines) = world.get_resource_mut::<journal::AiJournal>()
        {
            lines.rewind(mark);
            let outcome = if failed.is_empty() { "the world is as it was before the load" } else { "the world could not be put back entirely" };
            journal::log(world, JournalKind::Warn, format!("snapshot of tick {tick} not loaded ({reason}): {outcome}"));
        }
        if failed.is_empty() {
            return why;
        }
        let shown: Vec<&str> = failed.iter().take(5).map(String::as_str).collect();
        format!("{why}\nPutting the world back as it was before the load failed too: {}", shown.join("; "))
    }
}

/// The first line of an error, without a closing colon.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default().trim_end_matches(':')
}

/// [`restore`] while hold `gate` of the [`SimGate`] keeps the world from ticking (a client
/// that waited for the host's world): the hold is released right before the checks (they
/// refuse under a held gate) and held again, with its reason, when the restore fails.
pub fn restore_releasing(world: &mut World, snapshot: &Snapshot, gate: &str) -> Result<LoadReport, String> {
    let held = world.get_resource_mut::<SimGate>().and_then(|mut g| {
        let reason = g.reason(gate).map(str::to_owned);
        g.release(gate);
        reason
    });
    let result = restore(world, snapshot, false);
    if result.is_err()
        && let Some(reason) = held
        && let Some(mut g) = world.get_resource_mut::<SimGate>()
    {
        g.hold(gate, reason);
    }
    result
}

/// The world was built again from something other than its own ticks: a snapshot was
/// loaded, a region changed, a save was loaded. Who mirrors the world elsewhere (a host's
/// clients) starts over from here. Triggered by [`world_rebuilt`].
#[derive(Event, Clone, Debug, PartialEq)]
pub struct WorldRebuilt {
    /// What rebuilt it: `snapshot of tick 600`, `region wilds`.
    pub reason: String,
}

/// Says that the world was built again ([`WorldRebuilt`]): [`restore`] does, and so does a
/// module that rebuilds the world (a region change).
pub fn world_rebuilt(world: &mut World, reason: impl Into<String>) {
    world.trigger(WorldRebuilt { reason: reason.into() });
}

/// What a snapshot holds that this game cannot take.
#[derive(Default)]
struct Skipped {
    components: BTreeSet<String>,
    resources: BTreeSet<String>,
}

/// Format, tick rate, seed, engine version and the registered checks `checked` names against
/// this game. Returns whether a mismatch was forced past.
fn check_compatible(
    world: &World,
    snapshot: &Snapshot,
    force: bool,
    warnings: &mut Vec<String>,
    checked: &dyn Fn(&str) -> bool,
) -> Result<bool, String> {
    if let Some(loading) = sim::gate_message(world) {
        return Err(loading);
    }
    if !readable(&snapshot.format) {
        return Err(format!(
            "not a snapshot this build can load: format `{}`, expected `{FORMAT}` (or `{FORMAT_V1}`)",
            snapshot.format
        ));
    }
    let hz = 1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64();
    if (hz - snapshot.tick_hz).abs() > 1e-3 {
        return Err(format!(
            "the snapshot runs at {} ticks/s; this game at {hz:.3}: start the game with --hz {}",
            snapshot.tick_hz, snapshot.tick_hz
        ));
    }
    let mut problems = Vec::new();
    let seed = world.get_resource::<SimSeed>().map(|s| s.0);
    if seed != Some(snapshot.seed) {
        problems.push(format!(
            "it was taken with seed {}, this game runs seed {}: start the game with --seed {}",
            snapshot.seed,
            seed.map_or("none".to_owned(), |s| s.to_string()),
            snapshot.seed
        ));
    }
    if snapshot.engine != ENGINE_VERSION {
        // A patch release only fixes; a minor one (0.x) may change components.
        if same_line(&snapshot.engine, ENGINE_VERSION) {
            warnings.push(format!("it was written by engine {}, this is {ENGINE_VERSION}", snapshot.engine));
        } else {
            problems.push(format!(
                "it was written by engine {}, this is {ENGINE_VERSION}: take it again with this build",
                snapshot.engine
            ));
        }
    }
    let modules = world.get_resource::<SnapshotRegistry>();
    let checks: &[SnapshotCheck] = modules.map_or(&[], |m| &m.checks);
    for check in checks.iter().filter(|c| checked(c.name)) {
        // A check with parts is compared part by part where the snapshot lists them (this game's
        // own parts are what a prepare made them: the snapshot's), whole where it does not.
        if let (Some(parts), Some(saved)) = (check.parts, snapshot.check_parts.get(check.name)) {
            let now: Vec<(String, u64)> = parts(world);
            let then = read_parts(check.name, saved)?;
            let differences = replay::part_differences(&then, &now);
            let hash = format!("{:016x}", replay::parts_hash(&now));
            if !differences.is_empty() {
                problems.push(format!("{} differs ({}): {}", check.name, differences.join(", "), check.hint));
            } else if snapshot.checks.get(check.name).is_some_and(|saved| *saved != hash) {
                // The parts match, the hash the snapshot keeps for them does not: it was edited.
                let saved = &snapshot.checks[check.name];
                problems.push(format!("{} differs (snapshot {saved}, this game {hash}): {}", check.name, check.hint));
            }
            continue;
        }
        let now = format!("{:016x}", (check.check)(world));
        match snapshot.checks.get(check.name) {
            Some(saved) if *saved != now => problems.push(format!(
                "{} differs (snapshot {saved}, this game {now}): {}",
                check.name, check.hint
            )),
            Some(_) => {}
            None => warnings.push(format!("the snapshot has no `{}` check, so it was not compared", check.name)),
        }
    }
    for name in snapshot.checks.keys().filter(|n| checked(n)) {
        if !checks.iter().any(|c| c.name == name) {
            warnings.push(format!("the snapshot has a `{name}` check this game does not register, so it was not compared"));
        }
    }
    if problems.is_empty() {
        return Ok(false);
    }
    if force {
        warnings.extend(problems.iter().map(|p| format!("loaded anyway: {p}")));
        return Ok(true);
    }
    let list: Vec<String> = problems.iter().map(|p| format!("  - {p}")).collect();
    Err(format!(
        "this game is not the one the snapshot was taken in:\n{}\nFix that, or load with force=true to restore its \
         entities anyway (the continuation will not be the original's)",
        list.join("\n")
    ))
}

/// Whether two engine versions differ in the patch number at most (`0.2.0` and `0.2.3`; for
/// `1.x` and later the minor number too). Anything that does not parse as `X.Y.Z` has to be
/// equal.
fn same_line(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Option<(u64, u64)> {
        let mut parts = v.split('.').map(|p| p.parse::<u64>());
        let (major, minor, _patch) = (parts.next()?.ok()?, parts.next()?.ok()?, parts.next()?.ok()?);
        Some((major, if major == 0 { minor } else { 0 }))
    };
    match (parse(a), parse(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

/// Every component and resource of the snapshot can be read by this game (dry run, no
/// entity is touched). What it cannot is an error, or with `force` left out.
fn check_fits(
    registry: &TypeRegistry,
    modules: &SnapshotRegistry,
    snapshot: &Snapshot,
    force: bool,
    warnings: &mut Vec<String>,
) -> Result<Skipped, String> {
    let (ids, keyed) = (HashMap::new(), HashMap::new());
    let mut skipped = Skipped::default();
    let mut errors: Vec<String> = Vec::new();
    // Errors `force` would turn into warnings; the others it cannot load past.
    let mut skippable = 0;
    // Left out under `force` because they are not snapshot state, not because they are unknown.
    let mut reserved_paths = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for entity in &snapshot.entities {
        // 0 is "not assigned yet"; `u32::MAX` has no id after it for the counter to move to.
        let reserved = entity.id == 0 || entity.id == u32::MAX;
        if reserved || !seen.insert(entity.id) {
            errors.push(format!("entity id {} is {}", entity.id, if reserved { "not an id" } else { "listed twice" }));
        }
        for (path, value) in &entity.components {
            let Some(registration) = registry.get_with_type_path(path).filter(|r| r.data::<ReflectComponent>().is_some()) else {
                if force {
                    skipped.components.insert(path.clone());
                } else {
                    skippable += 1;
                    errors.push(format!(
                        "e{}: unknown component `{path}` (a snapshot from another game or version?)",
                        entity.id
                    ));
                }
                continue;
            };
            // Written in place by `apply_or_insert_mapped`, a hand-written `AiId` would skip the
            // hooks that keep `AiIndex` right; Bevy's own types are made again, not loaded.
            if !is_snapshot_type(registration.type_id(), path) {
                if force {
                    skipped.components.insert(path.clone());
                    reserved_paths.insert(path.clone());
                } else {
                    skippable += 1;
                    errors.push(format!(
                        "e{}: `{path}` is not snapshot state (AiId is the entity's key; Bevy's own types are rebuilt)",
                        entity.id
                    ));
                }
                continue;
            }
            if let Err(why) = from_json(registration, registry, value, &mut Decoder::dry(&ids, &keyed)) {
                errors.push(format!("e{}: {path}: {why}", entity.id));
            }
        }
    }
    // A resource that does not read would fail inside `replace_world`, with the world already
    // replaced: `force` does not skip it, only a resource this game does not register.
    for (name, value) in &snapshot.resources {
        let Some(resource) = modules.resources.iter().find(|r| r.name == name.as_str()) else {
            if force {
                skipped.resources.insert(name.clone());
            } else {
                skippable += 1;
                errors.push(format!("unknown saved resource `{name}` (this game does not register it)"));
            }
            continue;
        };
        let read = registry
            .get(resource.type_id)
            .ok_or_else(|| "not registered for reflection".to_owned())
            .and_then(|registration| from_json(registration, registry, value, &mut Decoder::dry(&ids, &keyed)));
        if let Err(why) = read {
            errors.push(format!("resource `{name}`: {why}"));
        }
    }
    if !errors.is_empty() {
        let shown: Vec<String> = errors.iter().take(8).map(|e| format!("  - {e}")).collect();
        let more = errors.len().saturating_sub(shown.len());
        let hint = if skippable == errors.len() {
            "Load with force=true to leave these out"
        } else if skippable > 0 {
            "force=true leaves out the unknown and reserved entries, not the others: fix the file or take the snapshot again"
        } else {
            "force=true does not load past these: fix the file or take the snapshot again"
        };
        return Err(format!(
            "the snapshot does not fit this game ({} problem(s)):\n{}{}\n{hint}",
            errors.len(),
            shown.join("\n"),
            if more > 0 { format!("\n  … (+{more})") } else { String::new() }
        ));
    }
    let unknown: BTreeSet<String> = skipped.components.difference(&reserved_paths).cloned().collect();
    if !unknown.is_empty() {
        warnings.push(format!("left out components this game does not know: {}", join(&unknown)));
    }
    if !reserved_paths.is_empty() {
        warnings.push(format!("left out components that are not snapshot state: {}", join(&reserved_paths)));
    }
    if !skipped.resources.is_empty() {
        warnings.push(format!("left out resources this game does not register: {}", join(&skipped.resources)));
    }
    for note in &snapshot.left_out {
        warnings.push(format!("the snapshot could not hold: {note}"));
    }
    Ok(skipped)
}

fn join(set: &BTreeSet<String>) -> String {
    set.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// Despawns every entity with an `AiId` and spawns the snapshot's (through
/// [`state::apply_changes`]'s steps), then sets the resources and the id counter.
fn replace_world(world: &mut World, snapshot: &Snapshot, skipped: &Skipped, warnings: &mut Vec<String>) {
    let doomed: Vec<Entity> = world.resource::<AiIndex>().live().into_iter().map(|(_, e)| e).collect();
    let going: BTreeSet<Entity> = doomed.iter().copied().collect();
    // The entities that stay (no AiId) that modules can name: references to them in the
    // snapshot (`c7`) find them again. Taken before anything goes.
    let keyed = state::keyed_entities(world, &going);
    for entity in doomed {
        // A child goes with its parent: it may be gone already.
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }

    let set = snapshot
        .entities
        .iter()
        .map(|entity| {
            let components =
                entity.components.iter().filter(|(path, _)| !skipped.components.contains(*path)).map(|(p, v)| (p.clone(), v.clone()));
            (entity.id, components.collect())
        })
        .collect();
    let resources =
        snapshot.resources.iter().filter(|(name, _)| !skipped.resources.contains(*name)).map(|(n, v)| (n.clone(), v.clone())).collect();
    let changes = StateChanges { set, resources, ..Default::default() };
    warnings.extend(state::apply(world, &changes, &keyed, StateRule::Snapshot));

    world.resource_mut::<AiIndex>().restore_counters(snapshot.ids.next, snapshot.ids.graveyard.iter().cloned());
}

/// Where `again` (the restored world, saved) is not what `wanted` says, ignoring what was
/// left out on purpose.
fn differences_between(wanted: &Snapshot, again: &Snapshot, skipped: &Skipped) -> Vec<String> {
    let mut out = Vec::new();
    if wanted.ids != again.ids {
        out.push(format!(
            "ids: next e{} with {} tombstones, expected e{} with {}",
            again.ids.next,
            again.ids.graveyard.len(),
            wanted.ids.next,
            wanted.ids.graveyard.len()
        ));
    }
    if wanted.tick != again.tick {
        out.push(format!("tick {}, expected {}", again.tick, wanted.tick));
    }
    let ids = |s: &Snapshot| s.entities.iter().map(|e| e.id).collect::<Vec<_>>();
    if ids(wanted) != ids(again) {
        out.push(format!("entities {:?}, expected {:?}", ids(again), ids(wanted)));
        return out;
    }
    for (a, b) in wanted.entities.iter().zip(&again.entities) {
        let paths: BTreeSet<&String> = a.components.keys().chain(b.components.keys()).collect();
        for path in paths {
            if skipped.components.contains(path.as_str()) {
                continue;
            }
            match (a.components.get(path), b.components.get(path)) {
                (Some(x), Some(y)) if x == y => {}
                (Some(x), Some(y)) => out.push(format!("e{} {}: {} instead of {}", a.id, short(path), clip(y), clip(x))),
                (Some(_), None) => out.push(format!("e{} lost {}", a.id, short(path))),
                (None, Some(_)) => out.push(format!("e{} gained {}", a.id, short(path))),
                (None, None) => {}
            }
        }
    }
    for (name, wanted_value) in &wanted.resources {
        if skipped.resources.contains(name) {
            continue;
        }
        match again.resources.get(name) {
            Some(v) if v == wanted_value => {}
            Some(v) => out.push(format!("resource {}: {} instead of {}", short(name), clip(v), clip(wanted_value))),
            None => out.push(format!("resource {} is gone", short(name))),
        }
    }
    out
}

/// Where two snapshots of the simulation differ, one line each, in a fixed order: the
/// header (tick, seed, ids), then entities by id, then resources by type path, then what the
/// world is built around; the state hash only when nothing else differs. Empty when they are
/// the same state. A component or resource names its first differing field and both values:
/// `e6 Boss: Health.current differs: 390.0 vs 400.0`, `resource shards_npc::Director
/// differs: …` (`a`'s value first).
pub fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    for (name, x, y) in [
        ("format", Value::from(a.format.as_str()), Value::from(b.format.as_str())),
        ("engine", Value::from(a.engine.as_str()), Value::from(b.engine.as_str())),
        ("tick", Value::from(a.tick), Value::from(b.tick)),
        ("seed", Value::from(a.seed), Value::from(b.seed)),
        ("tick_hz", Value::from(a.tick_hz), Value::from(b.tick_hz)),
    ] {
        if x != y {
            out.push(format!("{name} differs: {x} vs {y}"));
        }
    }
    let json = |x: Result<Value, serde_json::Error>| x.unwrap_or_default();
    if let Some((at, x, y)) = first_difference(&json(serde_json::to_value(&a.ids)), &json(serde_json::to_value(&b.ids))) {
        out.push(format!("ids{at} differ: {x} vs {y}"));
    }
    let theirs: BTreeMap<u32, &EntitySnapshot> = b.entities.iter().map(|e| (e.id, e)).collect();
    let mine: BTreeMap<u32, &EntitySnapshot> = a.entities.iter().map(|e| (e.id, e)).collect();
    let ids: BTreeSet<u32> = mine.keys().chain(theirs.keys()).copied().collect();
    for id in ids {
        match (mine.get(&id), theirs.get(&id)) {
            (Some(x), Some(y)) => {
                let label = entity_label(x);
                let paths: BTreeSet<&String> = x.components.keys().chain(y.components.keys()).collect();
                for path in paths {
                    match (x.components.get(path), y.components.get(path)) {
                        (Some(p), Some(q)) => {
                            if let Some((at, p, q)) = first_difference(p, q) {
                                out.push(format!("e{id}{label}: {}{at} differs: {p} vs {q}", short(path)));
                            }
                        }
                        (Some(_), None) => out.push(format!("e{id}{label}: {} only in the first", short(path))),
                        (None, Some(_)) => out.push(format!("e{id}{label}: {} only in the second", short(path))),
                        (None, None) => {}
                    }
                }
            }
            (Some(x), None) => out.push(format!("e{id}{} only in the first", entity_label(x))),
            (None, Some(y)) => out.push(format!("e{id}{} only in the second", entity_label(y))),
            (None, None) => {}
        }
    }
    let names: BTreeSet<&String> = a.resources.keys().chain(b.resources.keys()).collect();
    for name in names {
        match (a.resources.get(name), b.resources.get(name)) {
            (Some(x), Some(y)) => {
                if let Some((at, x, y)) = first_difference(x, y) {
                    out.push(format!("resource {name}{at} differs: {x} vs {y}"));
                }
            }
            (Some(_), None) => out.push(format!("resource {name} only in the first")),
            (None, Some(_)) => out.push(format!("resource {name} only in the second")),
            (None, None) => {}
        }
    }
    for (name, x, y) in [
        ("checks", serde_json::to_value(&a.checks), serde_json::to_value(&b.checks)),
        ("check_parts", serde_json::to_value(&a.check_parts), serde_json::to_value(&b.check_parts)),
        ("left_out", serde_json::to_value(&a.left_out), serde_json::to_value(&b.left_out)),
    ] {
        if let Some((at, x, y)) = first_difference(&json(x), &json(y)) {
            out.push(format!("{name}{at} differs: {x} vs {y}"));
        }
    }
    if out.is_empty() && a.state_hash != b.state_hash {
        out.push(format!("state_hash differs: {:016x} vs {:016x}", a.state_hash, b.state_hash));
    }
    out
}

/// ` Boss`: the entity's `Name`, when the snapshot has one.
fn entity_label(entity: &EntitySnapshot) -> String {
    let name = entity
        .components
        .iter()
        .find(|(path, _)| path.as_str() == "bevy_ecs::name::Name")
        .and_then(|(_, value)| value.as_str().map(str::to_owned).or_else(|| value.get("name")?.as_str().map(str::to_owned)));
    name.map(|n| format!(" {n}")).unwrap_or_default()
}

/// The first place `a` and `b` differ, depth first in key order: the path to it (`.current`,
/// `[2].hp`; empty at the top) and both values there, clipped.
fn first_difference(a: &Value, b: &Value) -> Option<(String, String, String)> {
    if a == b {
        return None;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for key in keys {
                match (x.get(key), y.get(key)) {
                    (Some(p), Some(q)) => {
                        if let Some((at, p, q)) = first_difference(p, q) {
                            return Some((format!(".{key}{at}"), p, q));
                        }
                    }
                    (p, q) => {
                        let shown = |v: Option<&Value>| v.map_or_else(|| "nothing".to_owned(), clip);
                        return Some((format!(".{key}"), shown(p), shown(q)));
                    }
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            x.iter().zip(y).enumerate().find_map(|(i, (p, q))| first_difference(p, q).map(|(at, p, q)| (format!("[{i}]{at}"), p, q)))
        }
        _ => Some((String::new(), clip(a), clip(b))),
    }
}

/// `game::combat::Health` → `Health`.
fn short(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

fn clip(value: &Value) -> String {
    let text = value.to_string();
    if text.len() <= 100 {
        return text;
    }
    let cut = (0..=100).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}…", &text[..cut])
}

// --- tools -----------------------------------------------------------------------------

/// Save the whole simulation state at this tick as a file: every entity with its id and
/// components, the id counter, the tick and what the engine modules keep outside entities.
/// `snapshot_load` brings it back, here or in a fresh game with the same seed and content;
/// a replay recorded after loading one starts from it.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SnapshotSaveArgs {
    /// Where to write it, inside snapshots/ in the game's working directory, e.g.
    /// "snapshots/boss_phase2.json". Default: snapshots/tick<T>.json.
    pub path: Option<String>,
}

/// Load a snapshot written by `snapshot_save`: every entity is replaced by the snapshot's
/// (same ids), the tick goes back (or forward) to the snapshot's, and the next `step`
/// continues exactly as the original session did. The recording starts from the snapshot
/// from now on: save the replay first if you want what came before. Refused, changing
/// nothing, when this game is not the one it was taken in (other seed, engine version or
/// content files) or would not come back exactly as saved: the error says what differs. "Go to boss phase" is a snapshot
/// taken there, loaded by name.
#[derive(Deserialize, JsonSchema)]
pub struct SnapshotLoadArgs {
    /// The file, inside snapshots/: "snapshots/boss_phase2.json".
    pub path: String,
    /// Load although the seed, the engine version or the content differ. The entities are
    /// the snapshot's, the rest is this game's, and the continuation will not be the
    /// original's.
    #[serde(default)]
    pub force: bool,
}

fn snapshot_save_tool(In(call): In<Call<SnapshotSaveArgs>>, world: &mut World) -> ToolResult {
    if let Some(loading) = sim::gate_message(world) {
        return Err(format!("{loading}; a snapshot of a world that is not spawned yet would be empty"));
    }
    let snapshot = capture(world)?;
    let requested = call.args.path.unwrap_or_else(|| format!("{SNAPSHOTS_DIR}/tick{}.json", snapshot.tick));
    let path = snapshot.write(&requested)?;
    let mut text = format!("saved {}: {}", path.display(), snapshot.summary());
    if !snapshot.left_out.is_empty() {
        let note = format!("snapshot leaves out: {}", snapshot.left_out.join("; "));
        journal::log(world, JournalKind::Warn, note.clone());
        text.push_str(&format!("; {note}"));
    }
    Ok(text)
}

fn snapshot_load_tool(In(call): In<Call<SnapshotLoadArgs>>, world: &mut World) -> ToolResult {
    let SnapshotLoadArgs { path: requested, force } = call.args;
    let path = crate::paths::inside(SNAPSHOTS_DIR, &requested, SNAPSHOT_PATH_EXAMPLE)?;
    let snapshot = Snapshot::read(&path)?;
    // Exactly or not at all: a load that reports failure leaves the world, the tick and the
    // recording as they were.
    let report = restore_exact(world, &snapshot, force)?;
    let tick = snapshot.tick;
    let dropped = replay::restart_from(world, snapshot, true);
    journal::log(world, JournalKind::Sim, format!("snapshot loaded: {} ({report})", path.display()));
    let earlier = match dropped {
        0 => String::new(),
        n => format!(" ({n} earlier events dropped: `replay_save` first to keep them)"),
    };
    Ok(format!("loaded {}: {report}; the recording now starts at tick {tick}{earlier}", path.display()))
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<SnapshotRegistry>()
        // Explicitly, for apps built without `reflect_auto_register`: a snapshot reads and
        // writes these through the type registry.
        .register_type::<AiVisible>()
        .register_type::<AiKind>()
        .register_type::<AiLandmark>()
        .register_type::<AiImportance>()
        .register_type::<AiPlayer>()
        .register_type::<AiOccluder>()
        .register_type::<AiId>()
        // A snapshot without positions is none: apps that do not add Bevy's transform plugin
        // (headless test apps) have them registered here.
        .register_type::<Transform>()
        .register_type::<Name>();
    register_tool(app.world_mut(), "snapshot_save", Scope::Dev, ToolKind::Control, snapshot_save_tool);
    register_tool(app.world_mut(), "snapshot_load", Scope::Dev, ToolKind::Control, snapshot_load_tool);
    // The world a snapshot holds is the host's; a client holds a view of it.
    let mut tools = app.world_mut().resource_mut::<crate::AiTools>();
    for name in ["snapshot_save", "snapshot_load"] {
        let _ = tools.set_route(name, crate::tools::NetRoute::HostOnly(crate::tools::ON_THE_HOST));
    }
}

/// Registration for [`AiAppExt::add_snapshot_resource`](crate::AiAppExt::add_snapshot_resource).
pub(crate) fn register_resource<R>(app: &mut App)
where
    R: Resource + Reflect + FromReflect + GetTypeRegistration + TypePath,
{
    app.register_type::<R>();
    app.world_mut().get_resource_or_init::<SnapshotRegistry>().add_resource::<R>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use crate::sim::TickClock;
    use std::time::Duration;
    use crate::AiAppExt;

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct Hp {
        current: f32,
        max: f32,
    }

    /// A component that points at another entity.
    #[derive(Component, Reflect, Clone, Debug)]
    #[reflect(Component)]
    struct Grudge {
        against: Entity,
        others: Vec<Entity>,
    }

    /// Cosmetic, not state: its entity has a `Hp` and a `Grudge` too.
    #[derive(Resource, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Resource, Default)]
    struct Score {
        points: u32,
        best: Option<f32>,
    }

    fn decay(mut q: Query<&mut Hp>, mut score: ResMut<Score>) {
        for mut hp in &mut q {
            hp.current -= 0.1;
        }
        score.points += 1;
    }

    fn game() -> App {
        game_with(|_| {})
    }

    /// [`game`] with `setup` before it is finished.
    fn game_with(setup: impl FnOnce(&mut App)) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: SimConfig { seed: 42, start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .register_type::<Hp>()
            .register_type::<Grudge>()
            .init_resource::<Score>()
            .add_snapshot_resource::<Score>()
            .add_systems(FixedUpdate, decay.in_set(SimSet::Resolve));
        let a = app.world_mut().spawn((AiKind::new("Hero"), AiPlayer, Hp { current: 10.0, max: 10.0 }, Transform::default())).id();
        let b = app.world_mut().spawn((AiKind::new("Imp"), Hp { current: 3.0, max: 3.0 }, Transform::from_xyz(1.0, 0.0, 2.0))).id();
        app.world_mut().entity_mut(b).insert(Grudge { against: a, others: vec![a, b] });
        setup(&mut app);
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn step(app: &mut App, ticks: u64) {
        sim::step(app.world_mut(), ticks, Duration::from_secs(5), |_| None).unwrap();
    }

    #[test]
    fn a_snapshot_round_trips_through_its_file_text() {
        let mut app = game();
        step(&mut app, 7);
        let snapshot = capture(app.world()).unwrap();
        assert_eq!((snapshot.tick, snapshot.entities.len()), (7, 2));
        assert!(snapshot.left_out.is_empty(), "{:?}", snapshot.left_out);
        let imp = &snapshot.entities[1];
        assert_eq!(imp.components["ai_core::snapshot::tests::Grudge"], serde_json::json!({"against": "e1", "others": ["e1", "e2"]}));
        assert_eq!(imp.components["ai_core::describe::AiKind"], serde_json::json!("Imp"));
        assert_eq!(snapshot.resources["ai_core::snapshot::tests::Score"], serde_json::json!({"points": 7, "best": null}));

        let text = serde_json::to_string_pretty(&snapshot).unwrap();
        let back: Snapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(back, snapshot);
        assert_eq!(serde_json::to_string_pretty(&capture(app.world()).unwrap()).unwrap(), text, "same state, same bytes");
    }

    #[test]
    fn differences_name_the_first_field_that_differs_in_a_fixed_order() {
        let mut app = game();
        step(&mut app, 3);
        let a = capture(app.world()).unwrap();
        assert!(differences(&a, &a).is_empty());

        let mut b = a.clone();
        b.tick = 4;
        b.entities[1].components.get_mut("ai_core::snapshot::tests::Hp").unwrap()["current"] = serde_json::json!(1.5);
        b.entities[0].components.remove("bevy_transform::components::transform::Transform");
        b.entities.push(EntitySnapshot { id: 9, components: BTreeMap::new() });
        b.resources.get_mut("ai_core::snapshot::tests::Score").unwrap()["points"] = serde_json::json!(70);
        b.state_hash ^= 1;
        let found = differences(&a, &b);
        let hp = &a.entities[1].components["ai_core::snapshot::tests::Hp"]["current"];
        assert_eq!(
            found,
            [
                "tick differs: 3 vs 4".to_owned(),
                "e1: Transform only in the first".to_owned(),
                format!("e2: Hp.current differs: {hp} vs 1.5"),
                "e9 only in the second".to_owned(),
                "resource ai_core::snapshot::tests::Score.points differs: 3 vs 70".to_owned(),
            ],
            "the state hash is named only when nothing else is"
        );

        let mut c = a.clone();
        c.state_hash ^= 1;
        assert_eq!(differences(&a, &c), [format!("state_hash differs: {:016x} vs {:016x}", a.state_hash, c.state_hash)]);
        // A long value is clipped, and an array of another length differs as a whole.
        let long = Value::from("x".repeat(300));
        let (at, x, _) = first_difference(&serde_json::json!({"a": [long]}), &serde_json::json!({"a": [1, 2]})).unwrap();
        assert_eq!((at.as_str(), x.chars().count()), (".a", 101));
    }

    #[test]
    fn loading_in_a_fresh_game_continues_like_the_original() {
        let mut original = game();
        step(&mut original, 10);
        let snapshot = capture(original.world()).unwrap();
        step(&mut original, 25);

        let mut copy = game();
        let report = restore(copy.world_mut(), &snapshot, false).unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        assert_eq!(sim::tick(copy.world()), 10);
        assert_eq!(crate::replay::state_hash(copy.world()), snapshot.state_hash);
        step(&mut copy, 25);

        assert_eq!(sim::tick(copy.world()), 35);
        assert_eq!(crate::replay::state_hash(copy.world()), crate::replay::state_hash(original.world()));
        assert_eq!(capture(copy.world()).unwrap().entities, capture(original.world()).unwrap().entities);
        // The reference points at the entity that is there now, not at one that went.
        let world = copy.world_mut();
        let (grudge, imp) = world.query::<(&Grudge, Entity)>().iter(world).map(|(g, e)| (g.clone(), e)).next().unwrap();
        assert_eq!(world.get::<AiId>(grudge.against), Some(&AiId(1)));
        assert_eq!(world.get::<AiId>(imp), Some(&AiId(2)));
        assert_eq!(grudge.others.len(), 2);
    }

    #[test]
    fn loading_into_the_same_game_goes_back_in_time() {
        let mut app = game();
        step(&mut app, 5);
        let snapshot = capture(app.world()).unwrap();
        step(&mut app, 20);
        // Something that only exists later is gone, something removed is back.
        app.world_mut().spawn((AiKind::new("Late"), Transform::default()));
        let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        app.world_mut().despawn(hero);
        assert_eq!(app.world().resource::<AiIndex>().len(), 2);

        let report = restore(app.world_mut(), &snapshot, false).unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        assert_eq!(sim::tick(app.world()), 5);
        assert_eq!(app.world().resource::<Score>().points, 5);
        let index = app.world().resource::<AiIndex>();
        assert_eq!(index.ids(), [AiId(1), AiId(2)]);
        assert_eq!(index.next_id(), snapshot.ids.next, "the next id is the snapshot's, not the later one");
        // Ticks and the log's clock agree.
        assert_eq!(app.world().resource::<TickClock>().get(), 5);
        step(&mut app, 1);
        assert_eq!(app.world().resource::<TickClock>().get(), 6);
    }

    #[test]
    fn a_snapshot_from_another_game_is_refused_and_says_why() {
        let mut app = game();
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.seed = 7;
        snapshot.engine = "0.0.1".into();
        let err = restore(app.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("it was taken with seed 7, this game runs seed 42: start the game with --seed 7"), "{err}");
        assert!(err.contains("written by engine 0.0.1"), "{err}");
        assert!(err.contains("force=true"), "{err}");

        // A patch release of the same engine line still loads, with a note.
        // The patch version is taken from this build's line, so a release does not break the test.
        let line = ENGINE_VERSION.rsplit_once('.').map_or(ENGINE_VERSION, |(line, _)| line);
        let other_patch = if ENGINE_VERSION.ends_with(".99") { format!("{line}.98") } else { format!("{line}.99") };
        let mut patch = capture(app.world()).unwrap();
        patch.engine = other_patch.clone();
        let report = restore(app.world_mut(), &patch, false).unwrap();
        assert!(report.warnings.iter().any(|w| w == &format!("it was written by engine {other_patch}, this is {ENGINE_VERSION}")), "{report:?}");
        assert!(same_line("0.2.0", "0.2.7") && !same_line("0.2.0", "0.3.0") && same_line("1.4.0", "1.5.2") && !same_line("1.4.0", "2.0.0"));
        assert!(same_line("weird", "weird") && !same_line("weird", "0.2.0"));

        snapshot.tick_hz = 30.0;
        assert!(restore(app.world_mut(), &snapshot, true).unwrap_err().contains("start the game with --hz 30"));
        snapshot.tick_hz = 60.0;
        let forced = restore(app.world_mut(), &snapshot, true).unwrap();
        assert!(forced.warnings.iter().any(|w| w.starts_with("loaded anyway: it was taken with seed 7")), "{forced:?}");
    }

    #[test]
    fn a_snapshot_with_components_this_game_lacks_is_refused_before_anything_changes() {
        let mut app = game();
        step(&mut app, 3);
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.entities[0].components.insert("somewhere::Else".into(), serde_json::json!({"x": 1}));
        snapshot.entities[1].components.insert(
            "ai_core::snapshot::tests::Hp".into(),
            serde_json::json!({"current": "much", "max": 1}),
        );
        let before = capture(app.world()).unwrap();
        let err = restore(app.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("e1: unknown component `somewhere::Else`"), "{err}");
        assert!(err.contains("e2: ai_core::snapshot::tests::Hp:"), "{err}");
        assert_eq!(capture(app.world()).unwrap(), before, "nothing changed");
    }

    #[test]
    fn a_snapshot_with_a_resource_that_does_not_read_is_refused_before_anything_changes() {
        let mut app = game();
        step(&mut app, 3);
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.resources.insert("ai_core::snapshot::tests::Score".into(), serde_json::json!({"points": "many", "best": null}));
        step(&mut app, 2);
        let before = capture(app.world()).unwrap();
        assert_eq!(before.tick, 5);

        // Not even `force` loads a value the game cannot read: it would fail half way.
        for force in [false, true] {
            let err = restore(app.world_mut(), &snapshot, force).unwrap_err();
            assert!(err.contains("resource `ai_core::snapshot::tests::Score`:"), "{err}");
            assert!(err.contains("force=true does not load past these"), "it does not send the caller to force: {err}");
            assert_eq!(capture(app.world()).unwrap(), before, "nothing changed (force={force})");
        }
    }

    #[test]
    fn a_snapshot_cannot_write_the_ai_id_in_place() {
        let mut app = game();
        step(&mut app, 3);
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.entities[1].components.insert("ai_core::id::AiId".into(), serde_json::json!(77));
        step(&mut app, 2);
        let before = capture(app.world()).unwrap();

        let err = restore(app.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("e2: `ai_core::id::AiId` is not snapshot state"), "{err}");
        assert!(err.contains("Load with force=true to leave these out"), "{err}");
        assert_eq!(capture(app.world()).unwrap(), before, "nothing changed");

        // Forced, it is left out and said so (not as unknown): the entity keeps
        // the id the snapshot gave it, and the index still finds it.
        let report = restore(app.world_mut(), &snapshot, true).unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        assert!(
            report.warnings.iter().any(|w| w == "left out components that are not snapshot state: ai_core::id::AiId"),
            "{:?}",
            report.warnings
        );
        assert!(!report.warnings.iter().any(|w| w.contains("does not know")), "{:?}", report.warnings);
        let index = app.world().resource::<AiIndex>();
        assert_eq!((index.ids(), index.get(AiId(77))), ([AiId(1), AiId(2)].to_vec(), None));
        let imp = index.get(AiId(2)).unwrap();
        assert_eq!(app.world().get::<AiId>(imp), Some(&AiId(2)));
    }

    #[test]
    fn an_entity_id_nothing_can_follow_is_refused_before_anything_changes() {
        let mut app = game();
        step(&mut app, 3);
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.entities[1].id = u32::MAX;
        let before = capture(app.world()).unwrap();
        for force in [false, true] {
            let err = restore(app.world_mut(), &snapshot, force).unwrap_err();
            assert!(err.contains(&format!("entity id {} is not an id", u32::MAX)), "{err}");
            assert_eq!(capture(app.world()).unwrap(), before, "nothing changed (force={force})");
        }
    }

    #[test]
    fn force_leaves_out_what_the_game_does_not_know() {
        let mut app = game();
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.entities[0].components.insert("somewhere::Else".into(), serde_json::json!({"x": 1}));
        let report = restore(app.world_mut(), &snapshot, true).unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        assert!(report.warnings.iter().any(|w| w.contains("somewhere::Else")), "{:?}", report.warnings);
    }

    #[test]
    fn state_outside_the_entities_that_no_one_saves_is_reported() {
        #[derive(Resource, Default)]
        struct Weather(u64);
        let mut app = game();
        app.init_resource::<Weather>().add_replay_check("weather", |w| w.resource::<Weather>().0);
        app.world_mut().resource_mut::<Weather>().0 = 4;
        let snapshot = capture(app.world()).unwrap();
        app.world_mut().resource_mut::<Weather>().0 = 9;
        let report = restore(app.world_mut(), &snapshot, false).unwrap();
        assert!(!report.exact());
        let text = report.explain_differences();
        assert!(text.contains("the state hash is") && text.contains("add_snapshot_resource"), "{text}");
    }

    #[test]
    fn a_load_that_would_not_be_exact_is_refused_and_puts_the_world_back() {
        #[derive(Resource, Default)]
        struct Weather(u64);
        #[derive(Resource, Default)]
        struct Rebuilt(u32);
        let mut app = game();
        app.init_resource::<Weather>().init_resource::<Rebuilt>().add_replay_check("weather", |w| w.resource::<Weather>().0);
        app.add_observer(|_: On<WorldRebuilt>, mut n: ResMut<Rebuilt>| n.0 += 1);
        app.world_mut().resource_mut::<Weather>().0 = 4;
        step(&mut app, 3);
        let snapshot = capture(app.world()).unwrap();
        // The weather is state no one saves: the load cannot bring it back.
        app.world_mut().resource_mut::<Weather>().0 = 9;
        step(&mut app, 5);

        let before = capture(app.world()).unwrap();
        let seen = app.world().resource::<journal::AiJournal>().last_seq();
        let err = restore_exact(app.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("the restored state differs from the snapshot") && err.contains("add_snapshot_resource"), "{err}");
        assert_eq!(capture(app.world()).unwrap(), before, "the entities, the counters and the tick are as they were");
        assert_eq!(sim::tick(app.world()), 8);
        assert_eq!(app.world().resource::<Weather>().0, 9);
        assert_eq!(app.world().resource::<Rebuilt>().0, 0, "nothing was rebuilt for anyone to follow");
        let lines: Vec<(JournalKind, String)> =
            app.world().resource::<journal::AiJournal>().after(seen).0.map(|e| (e.kind, e.text.clone())).collect();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(
            lines[0],
            (JournalKind::Warn, "snapshot of tick 3 not loaded (the restored state differs from the snapshot in 1 place(s)): the world is as it was before the load".into())
        );
        assert!(lines[1].0 == JournalKind::Error && lines[1].1.starts_with("snapshot of tick 3: the restored state differs"), "{lines:?}");

        // The game goes on from where it was, and a plain restore still loads what it can.
        step(&mut app, 2);
        assert_eq!(sim::tick(app.world()), 10);
        let report = restore(app.world_mut(), &snapshot, false).unwrap();
        assert!(!report.exact());
        assert_eq!((sim::tick(app.world()), app.world().resource::<Rebuilt>().0), (3, 1));
    }

    /// Ground a module builds from a saved resource: entities without ids, checked by a hash.
    #[derive(Resource, Reflect, Default, Clone, PartialEq, Debug, Serialize, Deserialize)]
    #[reflect(Resource, Default)]
    struct Terrain {
        hills: u32,
    }

    #[derive(Component)]
    struct Hill;

    fn hills_hash(world: &World) -> u64 {
        world.iter_entities().filter(|e| e.contains::<Hill>()).count() as u64
    }

    fn build_hills(world: &mut World, n: u32) {
        let old: Vec<Entity> = world.iter_entities().filter(|e| e.contains::<Hill>()).map(|e| e.id()).collect();
        for e in old {
            world.despawn(e);
        }
        for _ in 0..n {
            world.spawn(Hill);
        }
        world.insert_resource(Terrain { hills: n });
    }

    fn prepare_hills(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
        let saved = snapshot.resources.get(Terrain::type_path()).ok_or("no terrain saved")?;
        let saved: Terrain = serde_json::from_value(saved.clone()).map_err(|e| e.to_string())?;
        if world.resource::<Terrain>().hills == saved.hills {
            return Ok(None);
        }
        if saved.hills > 9 {
            return Err(format!("{} hills do not fit", saved.hills));
        }
        build_hills(world, saved.hills);
        journal::emit(world, JournalKind::Event, format!("{} hills raised", saved.hills));
        if saved.hills == 8 {
            return Err("the eighth hill slid down".into());
        }
        Ok(Some(format!("{} hills built", saved.hills)))
    }

    /// An input the replay records, as a keyboard would set it.
    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug, Serialize, Deserialize)]
    #[reflect(Component, Default)]
    struct Stance(u8);

    #[test]
    fn a_load_refused_after_a_prepare_ran_puts_the_world_back() {
        let mut app = game_with(|app| {
            app.add_snapshot_resource::<Terrain>()
                .add_snapshot_check("hills", "build the same hills first", hills_hash)
                .add_snapshot_prepare("terrain", &["hills"], prepare_hills)
                .register_type::<Stance>()
                .replay_input::<Stance>();
        });
        for id in [AiId(1), AiId(2)] {
            let entity = app.world().resource::<AiIndex>().get(id).unwrap();
            app.world_mut().entity_mut(entity).insert(Stance(0));
        }
        build_hills(app.world_mut(), 3);
        step(&mut app, 4);
        // The prepare builds the snapshot's 3 hills, and only then does the check it covers
        // find another ground than the snapshot's.
        let mut snapshot = capture(app.world()).unwrap();
        snapshot.checks.insert("hills".into(), format!("{:016x}", 7));
        build_hills(app.world_mut(), 1);
        step(&mut app, 2);
        // Pressed after the last tick: an input the next tick records.
        let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        app.world_mut().entity_mut(hero).insert(Stance(2));

        let before = capture(app.world()).unwrap();
        let seen = app.world().resource::<journal::AiJournal>().last_seq();
        let recorded = app.world().resource::<crate::AiReplay>().recorded_events();
        let err = restore(app.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("hills differs"), "{err}");
        assert_eq!((capture(app.world()).unwrap(), hills_hash(app.world())), (before.clone(), 1), "put back");
        let lines: Vec<String> = app.world().resource::<journal::AiJournal>().after(seen).0.map(|e| e.text.clone()).collect();
        assert_eq!(lines, ["snapshot of tick 4 not loaded (this game is not the one the snapshot was taken in): the world is as it was before the load"]);

        // The recording goes on as if nothing happened: the press, and no other input.
        step(&mut app, 1);
        let inputs: Vec<(AiId, Value)> = crate::replay::save(app.world())
            .events
            .into_iter()
            .skip(recorded)
            .filter_map(|e| match e {
                crate::replay::ReplayEvent::Input { id, value, .. } => Some((id, value)),
                _ => None,
            })
            .collect();
        assert_eq!(inputs, [(AiId(1), serde_json::json!(2))]);

        // A prepare that fails half way is undone the same.
        let mut slid = snapshot.clone();
        slid.resources.insert(Terrain::type_path().into(), serde_json::json!({"hills": 8}));
        let before = capture(app.world()).unwrap();
        let err = restore(app.world_mut(), &slid, false).unwrap_err();
        assert_eq!(err, "cannot put back the terrain the snapshot was taken in: the eighth hill slid down");
        assert_eq!((capture(app.world()).unwrap(), hills_hash(app.world())), (before, 1));

        // Forced, the same snapshot loads.
        let report = restore(app.world_mut(), &snapshot, true).unwrap();
        assert!(report.warnings.iter().any(|w| w.starts_with("loaded anyway: hills differs")), "{:?}", report.warnings);
        assert_eq!((hills_hash(app.world()), sim::tick(app.world())), (3, 4));
    }

    fn hilly(hills: u32, prepare: bool) -> App {
        let mut app = game();
        app.add_snapshot_resource::<Terrain>().add_snapshot_check("hills", "build the same hills first", hills_hash);
        if prepare {
            app.add_snapshot_prepare("terrain", &["hills"], prepare_hills);
        }
        build_hills(app.world_mut(), hills);
        app
    }

    #[test]
    fn a_prepare_registered_to_run_after_another_runs_right_after_it() {
        fn none(_: &mut World, _: &Snapshot) -> Result<Option<String>, String> {
            Ok(None)
        }
        let mut registry = SnapshotRegistry::default();
        registry.add_prepare("content", None, &[], none);
        registry.add_prepare("placed", Some("region"), &[], none);
        registry.add_prepare("levels", None, &[], none);
        registry.add_prepare("decor", Some("placed"), &[], none);
        registry.add_prepare("lights", Some("region"), &[], none);
        registry.add_prepare("region", None, &[], none);
        registry.add_prepare("lost", Some("nowhere"), &[], none);
        registry.add_prepare("tick", Some("tock"), &[], none);
        registry.add_prepare("tock", Some("tick"), &[], none);
        let names: Vec<&str> = registry.prepare_order().iter().map(|p| p.name).collect();
        // `decor` follows `placed` before `lights` comes after them; one after a prepare that is not
        // there runs where it was registered; a cycle runs last.
        assert_eq!(names, ["content", "levels", "region", "placed", "decor", "lights", "lost", "tick", "tock"]);
    }

    #[test]
    fn a_prepare_builds_what_the_checks_it_covers_compare() {
        let mut original = hilly(3, true);
        step(&mut original, 4);
        let snapshot = capture(original.world()).unwrap();

        // Without the module's prepare the ground differs, and the load is refused.
        let mut plain = hilly(1, false);
        let err = restore(plain.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("hills differs") && err.contains("build the same hills first"), "{err}");

        let mut copy = hilly(1, true);
        let report = restore(copy.world_mut(), &snapshot, false).unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        assert_eq!(report.warnings, ["terrain: 3 hills built"]);
        assert_eq!((hills_hash(copy.world()), sim::tick(copy.world())), (3, 4));
        // Already the snapshot's ground: nothing to do, nothing said.
        assert!(restore(copy.world_mut(), &snapshot, false).unwrap().warnings.is_empty());

        // A ground the module cannot build: refused, and the world is as it was.
        let mut big = snapshot.clone();
        big.resources.insert(Terrain::type_path().into(), serde_json::json!({"hills": 12}));
        let mut other = hilly(2, true);
        step(&mut other, 2);
        let before = capture(other.world()).unwrap();
        let err = restore(other.world_mut(), &big, false).unwrap_err();
        assert_eq!(err, "cannot put back the terrain the snapshot was taken in: 12 hills do not fit");
        assert_eq!((capture(other.world()).unwrap(), hills_hash(other.world())), (before, 2));
        // Another seed is refused before the module builds anything.
        let mut seeded = snapshot.clone();
        seeded.seed = 9;
        assert!(restore(other.world_mut(), &seeded, false).unwrap_err().contains("seed 9"));
        assert_eq!(hills_hash(other.world()), 2);
    }

    #[test]
    fn a_refused_load_does_not_push_the_oldest_journal_lines_out() {
        let mut original = hilly(3, true);
        step(&mut original, 2);
        let mut snapshot = capture(original.world()).unwrap();
        // Refused after the prepare built the snapshot's hills, by a check it covers.
        snapshot.checks.insert("hills".into(), format!("{:016x}", 7));

        let mut app = hilly(1, true);
        // A full ring of 20 lines: whatever the attempt writes pushes the oldest out.
        app.world_mut().insert_resource(journal::AiJournal::with_capacity(20));
        for i in 0..30 {
            journal::log(app.world_mut(), JournalKind::Sim, format!("line {i}"));
        }
        let lines = |app: &App| -> Vec<String> { app.world().resource::<journal::AiJournal>().after(0).0.map(|e| e.text.clone()).collect() };
        let before = lines(&app);
        assert_eq!((before.len(), before[0].as_str()), (20, "line 10"));
        let err = restore(app.world_mut(), &snapshot, false).unwrap_err();
        assert!(err.contains("hills differs"), "{err}");
        let after = lines(&app);
        // The attempt wrote lines (the hills raised, and raised back) and they are gone; the one
        // line that says what happened pushes out one old line, as any line would.
        assert_eq!(after.len(), 20);
        assert_eq!(after[..19], before[1..], "the lines before the load are still there");
        assert!(after[19].starts_with("snapshot of tick 2 not loaded"), "{}", after[19]);
        // The journal still counts every line that was written.
        assert!(app.world().resource::<journal::AiJournal>().last_seq() > 31);
    }

    #[test]
    fn a_replay_from_a_snapshot_plays_in_a_game_with_nothing_to_load() {
        let at = |app: &App| sim::tick(app.world());
        let fixed_elapsed = |app: &App| app.world().resource::<Time<Fixed>>().elapsed();
        let tick_time = |n: u64| Duration::from_nanos((1_000_000_000.0 / 60.0_f64).round() as u64 * n);

        let mut original = game();
        step(&mut original, 10);
        let start = capture(original.world()).unwrap();
        let dropped = crate::replay::restart_from(original.world_mut(), start, true);
        assert_eq!(dropped, 0);
        step(&mut original, 25);
        let recording = crate::replay::save(original.world());
        assert_eq!((recording.format.as_str(), recording.start.as_ref().map(|s| s.tick)), (replay::FORMAT, Some(10)));
        assert_eq!(recording.end.tick, 35);
        assert_eq!(fixed_elapsed(&original), tick_time(35), "the fixed clock follows the ticks");

        // The game has nothing to load, so the snapshot goes in during the first tick.
        let mut copy = game();
        crate::replay::play(copy.world_mut(), recording.clone()).unwrap();
        step(&mut copy, 25);
        copy.update();
        let report = copy.world().resource::<crate::AiReplay>().report().cloned().expect("finished");
        assert!(report.identical(), "{report}");
        assert_eq!((at(&copy), report.matched), (35, 1));
        assert_eq!(fixed_elapsed(&copy), tick_time(35), "also when the snapshot went in during a tick");
        assert_eq!(capture(copy.world()).unwrap().entities, capture(original.world()).unwrap().entities);

        // A game that is not the recording's seed cannot play it.
        let mut other = game();
        let mut wrong = recording.clone();
        wrong.start.as_mut().unwrap().seed = 5;
        assert!(crate::replay::play(other.world_mut(), wrong).unwrap_err().contains("snapshot is from seed 5"));
        let mut early = recording.clone();
        early.end.tick = 5;
        assert!(crate::replay::play(other.world_mut(), early).unwrap_err().contains("before its snapshot"));
    }

    #[test]
    fn a_component_that_does_not_reflect_is_named_when_saving() {
        #[derive(Component)]
        struct Plain;
        let mut app = game();
        let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        app.world_mut().entity_mut(hero).insert(Plain);
        let snapshot = capture(app.world()).unwrap();
        assert_eq!(snapshot.left_out.len(), 1, "{:?}", snapshot.left_out);
        assert!(snapshot.left_out[0].ends_with("Plain (not registered for reflection)"), "{:?}", snapshot.left_out);
        assert!(snapshot.left_out[0].contains("snapshot"), "the full type path: {:?}", snapshot.left_out);
    }

    #[test]
    fn what_cannot_be_written_back_is_said_at_save_time() {
        let mut app = game();
        let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        app.world_mut().get_mut::<Hp>(hero).unwrap().max = f32::INFINITY;
        let snapshot = capture(app.world()).unwrap();
        assert_eq!(snapshot.left_out.len(), 1, "{:?}", snapshot.left_out);
        assert!(snapshot.left_out[0].starts_with("ai_core::snapshot::tests::Hp (does not read back"), "{:?}", snapshot.left_out);
        assert!(!snapshot.entities[0].components.contains_key("ai_core::snapshot::tests::Hp"));
        // A load tells what the snapshot could not hold.
        let report = restore(app.world_mut(), &snapshot, false).unwrap();
        assert!(report.warnings.iter().any(|w| w.starts_with("the snapshot could not hold: ai_core::snapshot::tests::Hp")), "{:?}", report.warnings);
    }

    #[test]
    fn a_resource_that_does_not_read_back_is_said_at_save_time() {
        #[derive(Resource, Reflect, Default, Clone, PartialEq, Debug)]
        #[reflect(Resource, Default)]
        struct Gauge {
            level: f32,
        }
        let mut app = game_with(|app| {
            app.register_type::<Gauge>().init_resource::<Gauge>().add_snapshot_resource::<Gauge>();
        });
        app.world_mut().resource_mut::<Gauge>().level = f32::NAN;
        let snapshot = capture(app.world()).unwrap();
        assert_eq!(snapshot.left_out.len(), 1, "{:?}", snapshot.left_out);
        assert!(snapshot.left_out[0].starts_with("ai_core::snapshot::tests::Gauge (does not read back"), "{:?}", snapshot.left_out);
        assert!(!snapshot.resources.contains_key("ai_core::snapshot::tests::Gauge"));
        assert!(snapshot.resources.contains_key("ai_core::snapshot::tests::Score"), "the others are still written");
    }

    #[test]
    fn the_tools_keep_to_the_snapshots_folder() {
        let mut app = game();
        for bad in ["/tmp/x.json", "snapshots/../x.json", "x.json", "replays/x.json"] {
            let out = crate::call_tool(app.world_mut(), "dev", "snapshot_save", serde_json::json!({"path": bad}));
            assert!(!out.ok && out.result.as_str().unwrap().starts_with("path must be inside snapshots/"), "{bad}: {}", out.result);
            let out = crate::call_tool(app.world_mut(), "dev", "snapshot_load", serde_json::json!({"path": bad}));
            assert!(!out.ok && out.result.as_str().unwrap().starts_with("path must be inside snapshots/"), "{bad}: {}", out.result);
        }
    }

    #[test]
    fn the_tools_save_and_load_a_file() {
        let dir = std::env::temp_dir().join(format!("ai_core_snapshot_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = game();
        step(&mut app, 4);
        // Tools write relative to the working directory: go through the path helpers of
        // the module instead of changing it for the whole test process.
        let snapshot = capture(app.world()).unwrap();
        let file = crate::paths::inside_of(&dir, SNAPSHOTS_DIR, "snapshots/a.json", SNAPSHOT_PATH_EXAMPLE).unwrap();
        std::fs::write(&file, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();
        assert_eq!(Snapshot::read(&file).unwrap(), snapshot);

        std::fs::write(dir.join("snapshots/bad.json"), "{\"format\": \"shards-snapshot/9\"}").unwrap();
        assert!(Snapshot::read(&dir.join("snapshots/bad.json")).unwrap_err().contains("is not a snapshot"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[derive(Resource, Default)]
    struct Rebuilt(Vec<String>);

    #[test]
    fn restore_releasing_loads_under_a_held_gate_and_holds_it_again_when_it_fails() {
        let mut app = game();
        app.init_resource::<Rebuilt>()
            .add_observer(|rebuilt: On<WorldRebuilt>, mut seen: ResMut<Rebuilt>| seen.0.push(rebuilt.reason.clone()));
        step(&mut app, 3);
        let snapshot = capture(app.world()).unwrap();
        step(&mut app, 4);
        app.world_mut().resource_mut::<SimGate>().hold("net", "waiting for the world");
        assert!(restore(app.world_mut(), &snapshot, false).unwrap_err().contains("still loading"));

        let report = restore_releasing(app.world_mut(), &snapshot, "net").unwrap();
        assert!(report.exact(), "{:?}", report.differences);
        assert_eq!(sim::tick(app.world()), 3);
        assert!(!app.world().resource::<SimGate>().is_held());
        assert_eq!(app.world().resource::<Rebuilt>().0, ["snapshot of tick 3"]);

        app.world_mut().resource_mut::<SimGate>().hold("net", "waiting for the world");
        let mut other = snapshot.clone();
        other.seed = 7;
        assert!(restore_releasing(app.world_mut(), &other, "net").unwrap_err().contains("seed"));
        assert_eq!(app.world().resource::<SimGate>().reasons(), ["net: waiting for the world"], "held again");
        assert_eq!(app.world().resource::<Rebuilt>().0.len(), 1, "a refused load rebuilt nothing");
    }

    fn encode_all(world: &World) -> Vec<(u32, state::ComponentValues)> {
        let index = world.resource::<AiIndex>();
        index.live().into_iter().map(|(id, e)| (id.0, state::encode_entity(world, e).unwrap())).collect()
    }

    #[test]
    fn the_state_codec_carries_a_world_into_another_built_the_same_way() {
        let (mut a, mut b) = (game(), game());
        step(&mut a, 6);
        let late = a.world_mut().spawn((AiKind::new("Late"), Hp { current: 1.0, max: 2.0 })).id();
        let hero = a.world().resource::<AiIndex>().get(AiId(1)).unwrap();
        a.world_mut().entity_mut(late).insert(Grudge { against: hero, others: vec![late] });
        let resources = StateEncoder::new(a.world(), StateRule::Snapshot).resources().0;
        let changes = StateChanges { set: encode_all(a.world()), resources, ..Default::default() };
        let warnings = state::apply_changes(b.world_mut(), &changes);
        assert!(warnings.is_empty(), "{warnings:?}");
        sim::set_tick(b.world_mut(), 6);
        assert_eq!(encode_all(b.world()), encode_all(a.world()), "a new entity is spawned under its id, references resolve");
        assert_eq!(replay::state_hash(b.world()), replay::state_hash(a.world()));
        assert_eq!(b.world().resource::<Score>().points, 6);

        // A value is written whole: a list that got shorter loses its tail.
        a.world_mut().get_mut::<Grudge>(late).unwrap().others.clear();
        let changes = StateChanges { set: encode_all(a.world()), ..Default::default() };
        assert!(state::apply_changes(b.world_mut(), &changes).is_empty());
        assert_eq!(encode_all(b.world()), encode_all(a.world()), "the list is empty there too");

        // Removing a component and despawning with a note.
        let imp = a.world().resource::<AiIndex>().get(AiId(2)).unwrap();
        a.world_mut().entity_mut(imp).remove::<Grudge>();
        a.world_mut().despawn(late);
        let grudge = std::any::type_name::<Grudge>().to_owned();
        let changes = StateChanges { remove: vec![(2, vec![grudge])], despawn: vec![(3, "out of view".into())], ..Default::default() };
        assert!(state::apply_changes(b.world_mut(), &changes).is_empty());
        assert_eq!(encode_all(b.world()), encode_all(a.world()));
        let gone = b.world().resource::<AiIndex>().resolve(AiId(3)).unwrap_err();
        assert!(gone.starts_with("e3 is out of view since tick 6"), "{gone}");

        // What this game does not know is a warning, not a stop.
        let mut values = state::ComponentValues::new();
        values.insert("game::Nope".into(), serde_json::json!(1));
        let changes = StateChanges { set: vec![(9, values)], resources: [("game::Gone".to_owned(), serde_json::json!(0))].into(), ..Default::default() };
        let warnings = state::apply_changes(b.world_mut(), &changes);
        assert_eq!(warnings, ["e9: unknown component `game::Nope`", "unknown resource `game::Gone` (this game does not register it)"]);

        // Ids that are not ids are left out, as a snapshot refuses them: nothing is spawned.
        let before = encode_all(b.world());
        let changes = StateChanges { set: vec![(0, Default::default()), (u32::MAX, Default::default())], ..Default::default() };
        let warnings = state::apply_changes(b.world_mut(), &changes);
        assert_eq!(warnings, ["entity id 0 is not an id".to_owned(), format!("entity id {} is not an id", u32::MAX)]);
        assert_eq!(encode_all(b.world()), before);
        assert_eq!(b.world().resource::<AiIndex>().get(AiId(u32::MAX)), None);
    }
}
