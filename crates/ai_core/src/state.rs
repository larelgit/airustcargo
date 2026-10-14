//! Entity state codec: components and resources as JSON values, and back into a world.
//!
//! Snapshots and the network share it. A component's value is what reflection serializes,
//! with entity references written as `e12` (an [`AiId`]) or `c7` (a key a module registered
//! with [`add_snapshot_entity_key`](crate::AiAppExt::add_snapshot_entity_key)), so they mean
//! the same thing in another world built the same way. Numbers print the way their type
//! does, so an `f32` comes back exactly.
//!
//! Which components: [`StateRule::Snapshot`] is everything a snapshot holds
//! ([`is_snapshot_type`]); [`StateRule::Replicated`] is what a host sends its clients (see
//! [`net_rules`](crate::net_rules)).
//!
//! [`apply_changes`] writes values into a world the way a snapshot load does: entities that
//! are missing are spawned under their ids first (in id order, `AiId` before anything else),
//! so every reference resolves; then every value is written twice, so a hook that changes a
//! value on insert (a collider gets a new number) cannot win.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy::ecs::relationship::RelationshipHookMode;
use bevy::platform::sync::RwLockReadGuard;
use bevy::prelude::*;
use bevy::reflect::serde::{
    ReflectDeserializerProcessor, ReflectSerializerProcessor, TypedReflectDeserializer, TypedReflectSerializer,
};
use bevy::reflect::{PartialReflect, TypeRegistration, TypeRegistry};
use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::id::{AiId, AiIndex};
use crate::net_rules::{is_replicated_type, NetRules};
use crate::snapshot::{is_snapshot_type, EntityKey, SnapshotRegistry};

/// An entity's components by type path.
pub type ComponentValues = BTreeMap<String, Value>;

/// How an entity reference with no key is written: the entity has no `AiId` and no module
/// knows another name for it. Loads back as [`Entity::PLACEHOLDER`].
pub(crate) const UNTRACKED: &str = "untracked";

/// Which components (and resources) an encoder writes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StateRule {
    /// What a snapshot holds: every reflected component a game or an engine module defines,
    /// plus `Transform` and `Name`; every snapshot resource.
    #[default]
    Snapshot,
    /// What a host sends: the snapshot's minus `replay_ignore` minus `net_ignore`.
    Replicated,
}

impl StateRule {
    fn accepts(self, world: &World, type_id: TypeId, type_path: &str) -> bool {
        match self {
            StateRule::Snapshot => is_snapshot_type(type_id, type_path),
            StateRule::Replicated => is_replicated_type(world, type_id, type_path),
        }
    }

    fn accepts_resource(self, world: &World, type_id: TypeId) -> bool {
        match self {
            StateRule::Snapshot => true,
            StateRule::Replicated => !world.get_resource::<NetRules>().is_some_and(|r| r.is_resource_ignored(type_id)),
        }
    }
}

/// What to change in a world, by entity id ([`apply_changes`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StateChanges {
    /// Components to write. An id this world does not have is spawned under it.
    pub set: Vec<(u32, ComponentValues)>,
    /// Components to take off, by type path.
    pub remove: Vec<(u32, Vec<String>)>,
    /// Entities to despawn, with why (the tombstone's note, `out of view`; empty: a plain
    /// despawn).
    pub despawn: Vec<(u32, String)>,
    /// Snapshot resources to write, by type path.
    pub resources: BTreeMap<String, Value>,
}

// --- encoding --------------------------------------------------------------------------

/// Encodes many entities of one world: holds the type registry and the modules' entity keys
/// once.
pub struct StateEncoder<'w> {
    world: &'w World,
    registry: RwLockReadGuard<'w, TypeRegistry>,
    modules: SnapshotRegistry,
    rule: StateRule,
}

impl<'w> StateEncoder<'w> {
    pub fn new(world: &'w World, rule: StateRule) -> Self {
        Self {
            world,
            registry: world.resource::<AppTypeRegistry>().read(),
            modules: world.get_resource::<SnapshotRegistry>().cloned().unwrap_or_default(),
            rule,
        }
    }

    /// `entity`'s components by type path, and what had to be left out (`path (why)`: it does
    /// not reflect, or its value does not read back). `None`: no such entity.
    pub fn entity(&self, entity: Entity) -> Option<(ComponentValues, Vec<String>)> {
        self.world.get_entity(entity).ok()?;
        let mut left_out = BTreeSet::new();
        let values = self.encode(entity, &mut left_out);
        Some((values, left_out.into_iter().collect()))
    }

    pub(crate) fn encode(&self, entity: Entity, left_out: &mut BTreeSet<String>) -> ComponentValues {
        let registry = &*self.registry;
        let encoder = Encoder { world: self.world, keys: &self.modules.keys };
        let (no_ids, no_keys) = (HashMap::new(), HashMap::new());
        let mut values = BTreeMap::new();
        for (registration, value) in components(self.world, registry, entity, self.rule, left_out) {
            let path = registration.type_info().type_path();
            // What is written must read back (a NaN or an infinity does not: JSON has none).
            let written = to_json(value, registry, &encoder).map_err(|why| format!("does not serialize: {why}")).and_then(|json| {
                from_json(registration, registry, &json, &mut Decoder::dry(&no_ids, &no_keys))
                    .map(|_| json)
                    .map_err(|why| format!("does not read back: {why}"))
            });
            match written {
                Ok(json) => {
                    values.insert(path.to_owned(), json);
                }
                Err(why) => {
                    left_out.insert(format!("{path} ({why})"));
                }
            }
        }
        values
    }

    /// The registered snapshot resources this rule takes, by type path, and what could not be
    /// written.
    pub fn resources(&self) -> (BTreeMap<String, Value>, Vec<String>) {
        let mut left_out = BTreeSet::new();
        let values = self.encode_resources(&mut left_out);
        (values, left_out.into_iter().collect())
    }

    pub(crate) fn encode_resources(&self, left_out: &mut BTreeSet<String>) -> BTreeMap<String, Value> {
        let registry = &*self.registry;
        let encoder = Encoder { world: self.world, keys: &self.modules.keys };
        let (no_ids, no_keys) = (HashMap::new(), HashMap::new());
        let mut values = BTreeMap::new();
        for resource in self.modules.resources.iter().filter(|r| self.rule.accepts_resource(self.world, r.type_id)) {
            match (resource.save)(self.world, registry, &encoder) {
                Some(Ok(json)) => {
                    // What is written must read back, as for a component (a NaN is written as null).
                    let read = registry
                        .get(resource.type_id)
                        .ok_or_else(|| "not registered for reflection".to_owned())
                        .and_then(|registration| from_json(registration, registry, &json, &mut Decoder::dry(&no_ids, &no_keys)));
                    match read {
                        Ok(_) => {
                            values.insert(resource.name.to_owned(), json);
                        }
                        Err(why) => {
                            left_out.insert(format!("{} (does not read back: {why})", resource.name));
                        }
                    }
                }
                Some(Err(why)) => {
                    left_out.insert(format!("{} (does not serialize: {why})", resource.name));
                }
                None => {}
            }
        }
        values
    }
}

/// `entity`'s components as a snapshot holds them, by type path. What does not reflect or
/// does not read back is left out (a snapshot names it in `left_out`). Errors only when there
/// is no such entity.
pub fn encode_entity(world: &World, entity: Entity) -> Result<ComponentValues, String> {
    let (values, _) = StateEncoder::new(world, StateRule::Snapshot).entity(entity).ok_or_else(|| format!("no entity {entity}"))?;
    Ok(values)
}

/// `entity`'s replicated components (what a host sends), by type path.
pub fn encode_replicated(world: &World, entity: Entity) -> Result<ComponentValues, String> {
    let (values, _) = StateEncoder::new(world, StateRule::Replicated).entity(entity).ok_or_else(|| format!("no entity {entity}"))?;
    Ok(values)
}

/// The replicated resources: the snapshot resources minus `net_ignore_resource`. One whose
/// value does not read back (a NaN) is left out, as a component is.
pub fn encode_resources(world: &World) -> BTreeMap<String, Value> {
    StateEncoder::new(world, StateRule::Replicated).resources().0
}

/// The components of `entity` that `rule` takes, with their registrations. What does not
/// reflect is named in `left_out`.
fn components<'w>(
    world: &'w World,
    registry: &'w TypeRegistry,
    entity: Entity,
    rule: StateRule,
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
        if !rule.accepts(world, type_id, path) {
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

// --- applying --------------------------------------------------------------------------

/// Writes `changes` into `world`; returns warnings (a value that does not read, a reference
/// to nothing, an unknown component or resource), and never stops half way.
///
/// In order: ids this world does not have are spawned (in id order, `AiId` first); every
/// value of `set` is written twice; on the entities just spawned, components a hook or
/// `#[require]` added that `set` does not have and a host would have sent (replicated types)
/// are taken off; `remove` takes components off; `despawn` despawns, leaving the note on the
/// tombstone; `resources` are written in registration order.
pub fn apply_changes(world: &mut World, changes: &StateChanges) -> Vec<String> {
    let keyed = keyed_entities(world, &BTreeSet::new());
    apply(world, changes, &keyed, StateRule::Replicated)
}

/// The entities without an `AiId` that modules can name (`c7`), except `going`.
pub(crate) fn keyed_entities(world: &mut World, going: &BTreeSet<Entity>) -> HashMap<(String, u32), Entity> {
    let keys: Vec<EntityKey> = world.get_resource::<SnapshotRegistry>().map(|m| m.keys.clone()).unwrap_or_default();
    let mut keyed = HashMap::new();
    for key in &keys {
        for (n, entity) in (key.index)(world) {
            if !going.contains(&entity) && world.get::<AiId>(entity).is_none() {
                keyed.insert((key.prefix.to_owned(), n), entity);
            }
        }
    }
    keyed
}

/// An entity and the components read for it.
type Decoded<'r> = (u32, Entity, Vec<(&'r TypeRegistration, Box<dyn PartialReflect>)>);

/// [`apply_changes`] with the keyed entities taken beforehand and the rule for what a new
/// entity must not have beyond its values (`strip`).
pub(crate) fn apply(
    world: &mut World,
    changes: &StateChanges,
    keyed: &HashMap<(String, u32), Entity>,
    strip: StateRule,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let modules = world.get_resource::<SnapshotRegistry>().cloned().unwrap_or_default();

    // Every entity exists, under its id, before any component refers to another.
    let mut order: Vec<&(u32, ComponentValues)> = changes.set.iter().collect();
    order.sort_by_key(|(id, _)| *id);
    let mut spawned: BTreeSet<Entity> = BTreeSet::new();
    let mut targets: Vec<(u32, Entity, &ComponentValues)> = Vec::new();
    for (id, values) in order {
        if *id == 0 {
            warnings.push("entity id 0 is not an id".to_owned());
            continue;
        }
        let entity = match world.resource::<AiIndex>().get(AiId(*id)) {
            Some(entity) => entity,
            None => {
                let entity = world.spawn(AiId(*id)).id();
                spawned.insert(entity);
                entity
            }
        };
        targets.push((*id, entity, values));
    }
    let ids: HashMap<u32, Entity> = world.resource::<AiIndex>().live().into_iter().map(|(id, e)| (id.0, e)).collect();

    let mut decoder = Decoder::new(&ids, keyed);
    let mut decoded: Vec<Decoded> = Vec::new();
    for (id, entity, values) in targets {
        let mut components = Vec::new();
        for (path, value) in values {
            let Some(registration) = registry.get_with_type_path(path).filter(|r| r.data::<ReflectComponent>().is_some()) else {
                warnings.push(format!("e{id}: unknown component `{path}`"));
                continue;
            };
            match from_json(registration, &registry, value, &mut decoder) {
                Ok(boxed) => components.push((registration, boxed)),
                Err(why) => warnings.push(format!("e{id}: {path}: {why}")),
            }
        }
        decoded.push((id, entity, components));
    }

    // Inserting one component may add others (`#[require]`) and run hooks that change
    // them (a collider gets a new id, a character's collider is fitted), so every value is
    // written again, in place, once all are there. On new entities, components the hooks
    // added that the values do not have go.
    for _ in 0..2 {
        for (_, entity, components) in &decoded {
            let mut target = world.entity_mut(*entity);
            for (registration, value) in components {
                if let Some(component) = registration.data::<ReflectComponent>() {
                    component.apply_or_insert_mapped(&mut target, value.as_ref(), &registry, &mut (), RelationshipHookMode::Run);
                }
            }
        }
    }
    for (_, entity, components) in decoded.iter().filter(|(_, e, _)| spawned.contains(e)) {
        let kept: BTreeSet<TypeId> = components.iter().map(|(r, _)| r.type_id()).collect();
        let extra: Vec<&TypeRegistration> = match world.inspect_entity(*entity) {
            Ok(infos) => infos
                .filter_map(|(_, info)| registry.get(info.type_id()?))
                .filter(|r| r.data::<ReflectComponent>().is_some())
                .filter(|r| strip.accepts(world, r.type_id(), r.type_info().type_path()) && !kept.contains(&r.type_id()))
                .collect(),
            Err(_) => Vec::new(),
        };
        for registration in extra {
            if let Some(component) = registration.data::<ReflectComponent>() {
                component.remove(&mut world.entity_mut(*entity));
            }
        }
    }

    for (id, paths) in &changes.remove {
        let Some(entity) = world.resource::<AiIndex>().get(AiId(*id)) else {
            warnings.push(format!("e{id}: no such entity to take {} off", paths.join(", ")));
            continue;
        };
        for path in paths {
            match registry.get_with_type_path(path).and_then(|r| r.data::<ReflectComponent>()) {
                Some(component) => component.remove(&mut world.entity_mut(entity)),
                None => warnings.push(format!("e{id}: unknown component `{path}`")),
            }
        }
    }

    for (id, note) in &changes.despawn {
        let Some(entity) = world.resource::<AiIndex>().get(AiId(*id)) else { continue };
        world.despawn(entity);
        if !note.is_empty() {
            world.resource_mut::<AiIndex>().annotate(AiId(*id), note.clone());
        }
    }

    for name in changes.resources.keys() {
        if modules.resources.iter().all(|r| r.name != name) {
            warnings.push(format!("unknown resource `{name}` (this game does not register it)"));
        }
    }
    for resource in &modules.resources {
        let Some(value) = changes.resources.get(resource.name) else { continue };
        if let Err(why) = (resource.load)(world, &registry, value, &mut decoder) {
            warnings.push(format!("resource {}: {why}", resource.name));
        }
    }
    warnings.extend(decoder.unresolved);
    warnings
}

// --- entity references ----------------------------------------------------------------

/// Writes `Entity` values as `e12`, `c7` or `untracked`.
pub(crate) struct Encoder<'w> {
    pub(crate) world: &'w World,
    pub(crate) keys: &'w [EntityKey],
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
pub(crate) struct Decoder<'a> {
    ids: &'a HashMap<u32, Entity>,
    keyed: &'a HashMap<(String, u32), Entity>,
    /// Checking only: no entity exists yet, everything maps to the placeholder.
    dry: bool,
    pub(crate) unresolved: Vec<String>,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(ids: &'a HashMap<u32, Entity>, keyed: &'a HashMap<(String, u32), Entity>) -> Self {
        Self { ids, keyed, dry: false, unresolved: Vec::new() }
    }

    pub(crate) fn dry(ids: &'a HashMap<u32, Entity>, keyed: &'a HashMap<(String, u32), Entity>) -> Self {
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
pub(crate) fn to_json(value: &dyn PartialReflect, registry: &TypeRegistry, encoder: &Encoder) -> Result<Value, String> {
    let text = serde_json::to_string(&TypedReflectSerializer::with_processor(value, registry, encoder)).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

pub(crate) fn from_json(
    registration: &TypeRegistration,
    registry: &TypeRegistry,
    value: &Value,
    decoder: &mut Decoder,
) -> Result<Box<dyn PartialReflect>, String> {
    TypedReflectDeserializer::with_processor(registration, registry, decoder).deserialize(value.clone()).map_err(|e| e.to_string())
}
