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
//!   so a snapshot taken in a region loads into a game that has another region, or none.
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
use std::time::Duration;

use bevy::ecs::relationship::RelationshipHookMode;
use bevy::prelude::*;
use bevy::reflect::serde::{
    ReflectDeserializerProcessor, ReflectSerializerProcessor, TypedReflectDeserializer, TypedReflectSerializer,
};
use bevy::reflect::{FromReflect, GetTypeRegistration, PartialReflect, Reflect, TypePath, TypeRegistration, TypeRegistry};
use schemars::JsonSchema;
use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{AiId, AiImportance, AiIndex, AiLandmark, AiPlayer, AiVisible, Tombstone};
use crate::journal::{self, JournalKind};
use crate::rng::SimSeed;
use crate::session::Scope;
use crate::sim::{self, SimTick, TickClock};
use crate::spatial::AiOccluder;
use crate::tools::{register_tool, Call, ToolKind, ToolResult};
use crate::{describe::AiKind, replay};

/// Format tag written into every snapshot file.
pub const FORMAT: &str = "shards-snapshot/1";

/// Folder for saved snapshots, relative to the game's working directory. Tools read and
/// write nothing else (see [`paths`](crate::paths)).
pub const SNAPSHOTS_DIR: &str = "snapshots";

const SNAPSHOT_PATH_EXAMPLE: &str = "snapshots/boss_phase2.json";

/// How an entity reference with no key is written: the entity has no `AiId` and no module
/// knows another name for it. Loads back as [`Entity::PLACEHOLDER`].
const UNTRACKED: &str = "untracked";

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
        if snapshot.format != FORMAT {
            return Err(format!(
                "{} is not a snapshot this build can load: format `{}`, expected `{FORMAT}`",
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
struct SnapshotResource {
    name: &'static str,
    save: fn(&World, &TypeRegistry, &Encoder) -> Option<Result<Value, String>>,
    load: fn(&mut World, &TypeRegistry, &Value, &mut Decoder) -> Result<(), String>,
}

/// Runs after a load: a name for the journal and the code.
type Rebuild = (&'static str, fn(&mut World));

/// What a [`SnapshotPrepare`] does: `Ok(Some(note))` when it changed the world (the note goes
/// into the load's report), `Ok(None)` when the world already was what the snapshot needs,
/// `Err` (the world left as it was) when it cannot be.
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
}

#[derive(Clone, Copy)]
struct SnapshotCheck {
    name: &'static str,
    hint: &'static str,
    check: fn(&World) -> u64,
}

/// What modules registered with [`AiAppExt`](crate::AiAppExt)'s `add_snapshot_*`.
#[derive(Resource, Default, Clone)]
pub struct SnapshotRegistry {
    resources: Vec<SnapshotResource>,
    checks: Vec<SnapshotCheck>,
    prepares: Vec<SnapshotPrepare>,
    rebuilds: Vec<Rebuild>,
    keys: Vec<EntityKey>,
}

impl SnapshotRegistry {
    pub(crate) fn add_resource<R>(&mut self)
    where
        R: Resource + Reflect + FromReflect + TypePath,
    {
        let name = R::type_path();
        self.resources.retain(|r| r.name != name);
        self.resources.push(SnapshotResource { name, save: save_resource::<R>, load: load_resource::<R> });
    }

    pub(crate) fn add_check(&mut self, name: &'static str, hint: &'static str, check: fn(&World) -> u64) {
        self.checks.retain(|c| c.name != name);
        self.checks.push(SnapshotCheck { name, hint, check });
    }

    pub(crate) fn add_prepare(&mut self, name: &'static str, covers: &'static [&'static str], run: PrepareFn) {
        self.prepares.retain(|p| p.name != name);
        self.prepares.push(SnapshotPrepare { name, covers, run });
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

// --- entity references ----------------------------------------------------------------

/// Writes `Entity` values as `e12`, `c7` or `untracked`.
struct Encoder<'w> {
    world: &'w World,
    keys: &'w [EntityKey],
}

impl Encoder<'_> {
    fn name(&self, entity: Entity) -> String {
        if let Some(id) = self.world.get::<AiId>(entity) {
            return id.to_string();
        }
        for key in self.keys {
            if let Some(n) = (key.encode)(self.world, entity) {
                return format!("{}{n}", key.prefix);
            }
        }
        UNTRACKED.to_owned()
    }
}

impl ReflectSerializerProcessor for Encoder<'_> {
    fn try_serialize<S>(
        &self,
        value: &dyn PartialReflect,
        _: &TypeRegistry,
        serializer: S,
    ) -> Result<Result<S::Ok, S>, S::Error>
    where
        S: serde::Serializer,
    {
        match value.try_downcast_ref::<Entity>() {
            Some(entity) => serializer.serialize_str(&self.name(*entity)).map(Ok),
            None => Ok(Err(serializer)),
        }
    }
}

/// Reads `Entity` values back: `e12` is the entity restored under that id, `c7` the
/// entity a module keyed so. What cannot be found becomes [`Entity::PLACEHOLDER`] and is
/// reported once.
struct Decoder<'a> {
    ids: &'a HashMap<u32, Entity>,
    keyed: &'a HashMap<(String, u32), Entity>,
    /// Checking only: no entity exists yet, everything maps to the placeholder.
    dry: bool,
    unresolved: Vec<String>,
}

impl<'a> Decoder<'a> {
    fn new(ids: &'a HashMap<u32, Entity>, keyed: &'a HashMap<(String, u32), Entity>) -> Self {
        Self { ids, keyed, dry: false, unresolved: Vec::new() }
    }

    fn dry(ids: &'a HashMap<u32, Entity>, keyed: &'a HashMap<(String, u32), Entity>) -> Self {
        Self { ids, keyed, dry: true, unresolved: Vec::new() }
    }

    fn resolve(&mut self, text: &str) -> Entity {
        if self.dry || text == UNTRACKED {
            return Entity::PLACEHOLDER;
        }
        let split = text.find(|c: char| c.is_ascii_digit()).unwrap_or(text.len());
        let (prefix, digits) = text.split_at(split);
        let found = digits.parse::<u32>().ok().and_then(|n| match prefix {
            "e" => self.ids.get(&n).copied(),
            _ => self.keyed.get(&(prefix.to_owned(), n)).copied(),
        });
        found.unwrap_or_else(|| {
            let note = format!("a reference to `{text}` points at nothing in this world");
            if !self.unresolved.contains(&note) {
                self.unresolved.push(note);
            }
            Entity::PLACEHOLDER
        })
    }
}

impl ReflectDeserializerProcessor for Decoder<'_> {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        _: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if registration.type_id() != TypeId::of::<Entity>() {
            return Ok(Err(deserializer));
        }
        let text = String::deserialize(deserializer)?;
        Ok(Ok(Box::new(self.resolve(&text))))
    }
}

/// A value as JSON, entity references encoded. Through text and back, so numbers print the
/// way their type does (`0.1`, not the `f64` an `f32` widens to).
fn to_json(value: &dyn PartialReflect, registry: &TypeRegistry, encoder: &Encoder) -> Result<Value, String> {
    let text = serde_json::to_string(&TypedReflectSerializer::with_processor(value, registry, encoder)).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn from_json(
    registration: &TypeRegistration,
    registry: &TypeRegistry,
    value: &Value,
    decoder: &mut Decoder,
) -> Result<Box<dyn PartialReflect>, String> {
    TypedReflectDeserializer::with_processor(registration, registry, decoder).deserialize(value.clone()).map_err(|e| e.to_string())
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

/// The components of `entity` a snapshot holds, with their type paths.
fn snapshot_components<'w>(
    world: &'w World,
    registry: &'w TypeRegistry,
    entity: Entity,
    left_out: &mut BTreeSet<String>,
) -> Vec<(&'w TypeRegistration, &'w dyn PartialReflect)> {
    let (Ok(infos), Ok(e)) = (world.inspect_entity(entity), world.get_entity(entity)) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (_, info) in infos {
        let Some(type_id) = info.type_id() else { continue };
        let name = info.name().to_string();
        let Some(registration) = registry.get(type_id) else {
            // Bevy's own are not ours to save; a build without Bevy's `debug` feature has no
            // names to tell them from a game's (`<Enable the debug feature to see the name>`).
            if !name.starts_with("bevy_") && !name.starts_with('<') {
                left_out.insert(format!("{name} (not registered for reflection)"));
            }
            continue;
        };
        let path = registration.type_info().type_path();
        if !is_snapshot_type(type_id, path) {
            continue;
        }
        let Some(component) = registration.data::<ReflectComponent>() else {
            left_out.insert(format!("{path} (not a reflected component: add #[reflect(Component)])"));
            continue;
        };
        if let Some(value) = component.reflect(e) {
            found.push((registration, value.as_partial_reflect()));
        }
    }
    found
}

// --- saving ----------------------------------------------------------------------------

/// The simulation state as it is now. Reads only; the world is not touched.
pub fn capture(world: &World) -> Result<Snapshot, String> {
    let index = world.get_resource::<AiIndex>().ok_or("this world has no AiIndex: AiCorePlugin is not added")?;
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let modules = world.get_resource::<SnapshotRegistry>().cloned().unwrap_or_default();
    let encoder = Encoder { world, keys: &modules.keys };

    let mut left_out = BTreeSet::new();
    let mut entities = Vec::new();
    let (no_ids, no_keys) = (HashMap::new(), HashMap::new());
    for (id, entity) in index.live() {
        let mut components = BTreeMap::new();
        for (registration, value) in snapshot_components(world, &registry, entity, &mut left_out) {
            let path = registration.type_info().type_path();
            // What is written must read back (a NaN or an infinity does not: JSON has none).
            let written = to_json(value, &registry, &encoder).map_err(|why| format!("does not serialize: {why}")).and_then(|json| {
                from_json(registration, &registry, &json, &mut Decoder::dry(&no_ids, &no_keys))
                    .map(|_| json)
                    .map_err(|why| format!("does not read back: {why}"))
            });
            match written {
                Ok(json) => {
                    components.insert(path.to_owned(), json);
                }
                Err(why) => {
                    left_out.insert(format!("{path} ({why})"));
                }
            }
        }
        entities.push(EntitySnapshot { id: id.0, components });
    }

    let mut resources = BTreeMap::new();
    for resource in &modules.resources {
        match (resource.save)(world, &registry, &encoder) {
            Some(Ok(json)) => {
                resources.insert(resource.name.to_owned(), json);
            }
            Some(Err(why)) => {
                left_out.insert(format!("{} (does not serialize: {why})", resource.name));
            }
            None => {}
        }
    }

    let checks = modules.checks.iter().map(|c| (c.name.to_owned(), format!("{:016x}", (c.check)(world)))).collect();
    Ok(Snapshot {
        format: FORMAT.to_owned(),
        tick: sim::tick(world),
        seed: world.get_resource::<SimSeed>().map_or(0, |s| s.0),
        tick_hz: 1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64(),
        engine: ENGINE_VERSION.to_owned(),
        state_hash: replay::state_hash(world),
        checks,
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
/// The caller handles the replay recorder (`snapshot_load` restarts the recording from the
/// snapshot; playback does the same for a replay's own start).
pub fn restore(world: &mut World, snapshot: &Snapshot, force: bool) -> Result<LoadReport, String> {
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
    // first: the checks it covers compare the world built so with the snapshot's.
    for prepare in &modules.prepares {
        match (prepare.run)(world, snapshot) {
            Ok(Some(note)) => warnings.push(format!("{}: {note}", prepare.name)),
            Ok(None) => {}
            Err(why) => return Err(format!("cannot put back the {} the snapshot was taken in: {why}", prepare.name)),
        }
    }
    if !covered.is_empty() {
        let mut later = Vec::new();
        forced |= check_compatible(world, snapshot, force, &mut later, &|name| covered.contains(name))?;
        for warning in later {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
    }

    {
        let registry = app_registry.read();
        replace_world(world, &registry, &modules, snapshot, &skipped, &mut warnings);
    }
    set_tick(world, snapshot.tick);
    for (_, rebuild) in &modules.rebuilds {
        rebuild(world);
    }

    // Save it again: the restored world must say what the file says.
    let mut differences = Vec::new();
    match capture(world) {
        Ok(again) => {
            differences = differences_between(snapshot, &again, &skipped);
            // A forced load past a mismatch is not meant to reproduce the original's hash.
            if differences.is_empty() && !forced && again.state_hash != snapshot.state_hash {
                differences.push(format!(
                    "the state hash is {:016x}, the snapshot's {:016x}: a replay check covers state that is not \
                     on an entity with an AiId",
                    again.state_hash, snapshot.state_hash
                ));
            }
        }
        Err(why) => differences.push(format!("cannot save the restored world again: {why}")),
    }
    Ok(LoadReport {
        tick: snapshot.tick,
        entities: snapshot.entities.len(),
        resources: snapshot.resources.len(),
        warnings,
        differences,
    })
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
    if snapshot.format != FORMAT {
        return Err(format!("not a snapshot this build can load: format `{}`, expected `{FORMAT}`", snapshot.format));
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
    let mut seen = BTreeSet::new();
    for entity in &snapshot.entities {
        if entity.id == 0 || !seen.insert(entity.id) {
            errors.push(format!("entity id {} is {}", entity.id, if entity.id == 0 { "not an id" } else { "listed twice" }));
        }
        for (path, value) in &entity.components {
            let Some(registration) = registry.get_with_type_path(path).filter(|r| r.data::<ReflectComponent>().is_some()) else {
                if force {
                    skipped.components.insert(path.clone());
                } else {
                    errors.push(format!(
                        "e{}: unknown component `{path}` (a snapshot from another game or version?)",
                        entity.id
                    ));
                }
                continue;
            };
            if let Err(why) = from_json(registration, registry, value, &mut Decoder::dry(&ids, &keyed)) {
                errors.push(format!("e{}: {path}: {why}", entity.id));
            }
        }
    }
    // Whether a saved resource's value reads is found out when it is loaded (they are few).
    for name in snapshot.resources.keys() {
        if modules.resources.iter().all(|r| r.name != name) {
            if force {
                skipped.resources.insert(name.clone());
            } else {
                errors.push(format!("unknown saved resource `{name}` (this game does not register it)"));
            }
        }
    }
    if !errors.is_empty() {
        let shown: Vec<String> = errors.iter().take(8).map(|e| format!("  - {e}")).collect();
        let more = errors.len().saturating_sub(shown.len());
        return Err(format!(
            "the snapshot does not fit this game ({} problem(s)):\n{}{}\nLoad with force=true to skip what this game does not know",
            errors.len(),
            shown.join("\n"),
            if more > 0 { format!("\n  … (+{more})") } else { String::new() }
        ));
    }
    if !skipped.components.is_empty() {
        warnings.push(format!("left out components this game does not know: {}", join(&skipped.components)));
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

/// An entity just spawned and the components read for it.
type Decoded<'r> = (Entity, Vec<(&'r TypeRegistration, Box<dyn PartialReflect>)>);

/// Despawns every entity with an `AiId` and spawns the snapshot's, then sets the resources
/// and the id counter.
fn replace_world(
    world: &mut World,
    registry: &TypeRegistry,
    modules: &SnapshotRegistry,
    snapshot: &Snapshot,
    skipped: &Skipped,
    warnings: &mut Vec<String>,
) {
    let doomed: Vec<Entity> = world.resource::<AiIndex>().live().into_iter().map(|(_, e)| e).collect();
    let going: BTreeSet<Entity> = doomed.iter().copied().collect();
    // The entities that stay (no AiId) that modules can name: references to them in the
    // snapshot (`c7`) find them again. Taken before anything goes.
    let mut keyed: HashMap<(String, u32), Entity> = HashMap::new();
    for key in &modules.keys {
        for (n, entity) in (key.index)(world) {
            if !going.contains(&entity) {
                keyed.insert((key.prefix.to_owned(), n), entity);
            }
        }
    }
    for entity in doomed {
        // A child goes with its parent: it may be gone already.
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }

    // Every entity exists, under its id, before any component refers to another.
    let mut ids: HashMap<u32, Entity> = HashMap::new();
    for entity in &snapshot.entities {
        ids.insert(entity.id, world.spawn(AiId(entity.id)).id());
    }

    let mut decoder = Decoder::new(&ids, &keyed);
    let mut decoded: Vec<Decoded> = Vec::new();
    for entity in &snapshot.entities {
        let mut components = Vec::new();
        for (path, value) in &entity.components {
            if skipped.components.contains(path) {
                continue;
            }
            let Some(registration) = registry.get_with_type_path(path) else { continue };
            match from_json(registration, registry, value, &mut decoder) {
                Ok(boxed) => components.push((registration, boxed)),
                Err(why) => warnings.push(format!("e{}: {path}: {why}", entity.id)),
            }
        }
        decoded.push((ids[&entity.id], components));
    }

    // Inserting one component may add others (`#[require]`) and run hooks that change
    // them (a collider gets a new id, a character's collider is fitted), so every value is
    // written again, in place, once all are there. Components the hooks added that the
    // snapshot did not have go.
    for _ in 0..2 {
        for (entity, components) in &decoded {
            let mut target = world.entity_mut(*entity);
            for (registration, value) in components {
                if let Some(component) = registration.data::<ReflectComponent>() {
                    component.apply_or_insert_mapped(&mut target, value.as_ref(), registry, &mut (), RelationshipHookMode::Run);
                }
            }
        }
    }
    for (entity, components) in &decoded {
        let kept: BTreeSet<TypeId> = components.iter().map(|(r, _)| r.type_id()).collect();
        let extra: Vec<&TypeRegistration> = match world.inspect_entity(*entity) {
            Ok(infos) => infos
                .filter_map(|(_, info)| registry.get(info.type_id()?))
                .filter(|r| r.data::<ReflectComponent>().is_some())
                .filter(|r| is_snapshot_type(r.type_id(), r.type_info().type_path()) && !kept.contains(&r.type_id()))
                .collect(),
            Err(_) => Vec::new(),
        };
        for registration in extra {
            if let Some(component) = registration.data::<ReflectComponent>() {
                component.remove(&mut world.entity_mut(*entity));
            }
        }
    }

    for resource in &modules.resources {
        let Some(value) = snapshot.resources.get(resource.name) else { continue };
        if skipped.resources.contains(resource.name) {
            continue;
        }
        if let Err(why) = (resource.load)(world, registry, value, &mut decoder) {
            warnings.push(format!("resource {}: {why}", resource.name));
        }
    }
    warnings.extend(decoder.unresolved);

    world.resource_mut::<AiIndex>().restore_counters(snapshot.ids.next, snapshot.ids.graveyard.iter().cloned());
}

/// The simulation clock goes to `tick`: the counter, the log's copy of it and the fixed
/// time (a system that reads `Res<Time>` in a tick sees the same elapsed time as in the
/// original session).
fn set_tick(world: &mut World, tick: u64) {
    let before = sim::tick(world);
    world.resource_mut::<SimTick>().0 = tick;
    if let Some(clock) = world.get_resource::<TickClock>() {
        clock.set(tick);
    }
    let fixed = world.resource::<Time<Fixed>>();
    let step = fixed.timestep();
    let of = |n: u64| Duration::from_nanos((step.as_nanos() * n as u128) as u64);
    // Called from inside a tick (a replay's start, before the tick counter moves), the
    // fixed time is one step ahead of the counter: it stays so.
    let ahead = if fixed.elapsed().saturating_sub(of(before)) >= step { step } else { Duration::ZERO };
    let mut time = Time::<Fixed>::from_duration(step);
    time.advance_by(of(tick) + ahead);
    *world.resource_mut::<Time<Fixed>>() = time;
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
/// content files): the error says what differs. "Go to boss phase" is a snapshot
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
    let report = restore(world, &snapshot, force)?;
    let tick = snapshot.tick;
    let dropped = replay::restart_from(world, snapshot, true);
    journal::log(world, JournalKind::Sim, format!("snapshot loaded: {} ({report})", path.display()));
    if !report.exact() {
        let why = report.explain_differences();
        journal::log(world, JournalKind::Error, format!("snapshot {}: {why}", path.display()));
        return Err(format!("loaded {} ({report}), but {why}", path.display()));
    }
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
        let mut patch = capture(app.world()).unwrap();
        patch.engine = "0.2.99".into();
        let report = restore(app.world_mut(), &patch, false).unwrap();
        assert!(report.warnings.iter().any(|w| w == &format!("it was written by engine 0.2.99, this is {ENGINE_VERSION}")), "{report:?}");
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
        Ok(Some(format!("{} hills built", saved.hills)))
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
        assert_eq!((recording.format.as_str(), recording.start.as_ref().map(|s| s.tick)), ("shards-replay/3", Some(10)));
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
}
