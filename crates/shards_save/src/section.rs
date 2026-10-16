//! Sections: the save schema, written out.
//!
//! A save is not a dump of the world. The game says what goes in: each piece is a
//! *section* with a name, a version and a kind. A [`SaveKind::Character`] section belongs
//! to a player's body (position, health, inventory); a [`SaveKind::World`] section belongs
//! to the host's world (the region, which chests are open, what the bosses did). Anything
//! no section names is not saved, and the registry ([`SaveRegistry`], the `save_registry`
//! tool) is the list of what is.
//!
//! Three ways to add one:
//!
//! - [`SaveSection`]: a struct that knows how to capture itself from the world and apply
//!   itself back (`app.add_save_section::<Inventory>()`);
//! - [`save_component`](crate::SaveAppExt::save_component): "this component on the player's
//!   body" (character) or "this component on every entity with a [`SaveKey`]" (world);
//! - [`save_resource`](crate::SaveAppExt::save_resource): "this resource" (world).
//!
//! A section's data is its serde form. Changing the struct means raising
//! [`SaveSection::VERSION`] and registering a migration from the old version
//! ([`add_save_migration`](crate::SaveAppExt::add_save_migration)) that turns the old JSON
//! into the new: old structs need not exist any more. A chain with a gap panics when the
//! app is finished, naming the section and the missing step.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy::prelude::*;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canon;

/// Whose a save is.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SaveKind {
    /// A player's progress: kept on that player's machine, one file per slot.
    Character,
    /// The world: kept by the host.
    World,
}

impl SaveKind {
    /// `character` or `world`: the folder name and the word in journal lines.
    pub fn as_str(self) -> &'static str {
        match self {
            SaveKind::Character => "character",
            SaveKind::World => "world",
        }
    }

    /// Both kinds, world first: the order a full load applies them in.
    pub const ALL: [SaveKind; 2] = [SaveKind::World, SaveKind::Character];
}

impl std::fmt::Display for SaveKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Names an entity across sessions. `AiId`s are handed out per session; a world section
/// made with [`save_component`](crate::SaveAppExt::save_component) finds its entities by
/// this key, which the game gives them when it spawns them (`SaveKey::new("chest.ford")`).
/// Unique per world.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[reflect(Component)]
pub struct SaveKey(pub String);

impl SaveKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }
}

/// One thing a save holds. Implement it on a struct that is the section's data.
///
/// ```ignore
/// #[derive(Serialize, Deserialize)]
/// struct Inventory { gold: u32, items: Vec<String> }
///
/// impl SaveSection for Inventory {
///     const NAME: &'static str = "inventory";
///     const VERSION: u32 = 1;
///     const KIND: SaveKind = SaveKind::Character;
///
///     fn capture(world: &World, body: Option<Entity>) -> Result<Self, String> {
///         let bag = world.get::<Bag>(body.ok_or("no body")?).ok_or("the body has no bag")?;
///         Ok(Inventory { gold: bag.gold, items: bag.items.clone() })
///     }
///     fn apply(self, world: &mut World, body: Option<Entity>) -> Result<(), String> {
///         world.entity_mut(body.ok_or("no body")?).insert(Bag { gold: self.gold, items: self.items });
///         Ok(())
///     }
/// }
/// ```
pub trait SaveSection: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Key in the file and in the registry: lowercase letters, digits, `_`. Unique across
    /// the game. Registering a second section with a name replaces the first (a game
    /// replaces the engine's `position` with "restore at camp"), keeping its place in the
    /// order sections are applied. A replacement from another crate starts a migration history
    /// of its own: the first one's migrations are removed with it, and the new one registers the
    /// migrations of its own versions after it.
    const NAME: &'static str;
    /// The schema version, from 1. Raise it when the serde form changes in a way old files
    /// do not read, and add a migration from the version before.
    const VERSION: u32;
    const KIND: SaveKind;

    /// Reads the state from the world. `body` is the player's body for a character section
    /// (always `Some` then); a world section gets `None`.
    fn capture(world: &World, body: Option<Entity>) -> Result<Self, String>;

    /// Puts the state into the world, which was built afresh (a save is the player's
    /// progress, applied onto a world the game made the usual way).
    fn apply(self, world: &mut World, body: Option<Entity>) -> Result<(), String>;

    /// Called instead of [`apply`](Self::apply) when the file has no such section (it is
    /// older than the section): put the default in. Default: leave the world as it is.
    fn missing(world: &mut World, body: Option<Entity>) -> Result<(), String> {
        let _ = (world, body);
        Ok(())
    }

    /// Looks at the saved state and the world before **any** section of the load is applied,
    /// and refuses what cannot be applied: the place for every refusal that must leave the
    /// world untouched (content a manifest names that the game no longer has, a body that must
    /// exist, a number out of its range). Every `check` of the load, of every kind, runs after
    /// all files are read and before the first [`apply`](Self::apply); one that fails stops the
    /// load there: nothing was applied, and the call leaves the recording.
    ///
    /// It only reads the world (`&World`), so it cannot change it, and it sees the world as it
    /// is before the load: a refusal that depends on what an earlier section does belongs in
    /// `apply`. Not run for a section the file lacks ([`missing`](Self::missing) has no data to
    /// look at). Default: accept.
    ///
    /// An `Err` from [`apply`](Self::apply) or [`missing`](Self::missing) is another thing: the
    /// section ran, so the world counts as possibly changed (the load says so, restarts the
    /// recording from the world as it is and tells whoever mirrors it to take it whole again).
    fn check(&self, world: &World, body: Option<Entity>) -> Result<(), String> {
        let _ = (world, body);
        Ok(())
    }
}

type CaptureFn = Arc<dyn Fn(&World, Option<Entity>) -> Result<Value, String> + Send + Sync>;
type ApplyFn = Arc<dyn Fn(&mut World, Option<Entity>, Value) -> Result<(), String> + Send + Sync>;
type MissingFn = Arc<dyn Fn(&mut World, Option<Entity>) -> Result<(), String> + Send + Sync>;
type CheckFn = Arc<dyn Fn(&Value) -> Result<(), String> + Send + Sync>;
type CheckInFn = Arc<dyn Fn(&World, Option<Entity>, &Value) -> Result<(), String> + Send + Sync>;

/// A registered section, type-erased.
#[derive(Clone)]
pub struct SectionEntry {
    pub name: String,
    pub kind: SaveKind,
    pub version: u32,
    /// The crate or game that registered it (`shards`, `sandbox`): the first path segment
    /// of the type.
    pub source: String,
    capture: CaptureFn,
    apply: ApplyFn,
    missing: MissingFn,
    check: CheckFn,
    check_in: CheckInFn,
}

impl std::fmt::Debug for SectionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SectionEntry({} {} v{})", self.kind, self.name, self.version)
    }
}

/// The crate a type comes from: `sandbox::Bag` → `sandbox`.
fn source_of<T: ?Sized>() -> String {
    std::any::type_name::<T>().split("::").next().unwrap_or("?").to_owned()
}

/// Section names are file keys and tool arguments.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 48 && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// `data` as JSON that provably reads back to the same data: written, read and written again.
/// What does not (a NaN, a custom `Serialize` that drops a field) is refused here, at save
/// time, rather than found missing at load time.
fn verified<T: Serialize + DeserializeOwned>(data: &T) -> Result<Value, String> {
    let value = canon::to_value(data)?;
    let back: T = serde_json::from_value(value.clone()).map_err(|e| {
        let hint = if e.to_string().contains("expected f") { "; NaN and infinite numbers cannot be saved" } else { "" };
        format!("does not read back: {e}{hint}")
    })?;
    if canon::to_value(&back)? != value {
        return Err("does not read back to the same data (a Serialize/Deserialize pair that loses something?)".to_owned());
    }
    Ok(value)
}

fn read<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| e.to_string())
}

impl SectionEntry {
    /// The entry of a [`SaveSection`].
    pub fn of<S: SaveSection>() -> Self {
        Self {
            name: S::NAME.to_owned(),
            kind: S::KIND,
            version: S::VERSION,
            source: source_of::<S>(),
            capture: Arc::new(|world, body| verified(&S::capture(world, body)?)),
            apply: Arc::new(|world, body, value| read::<S>(value)?.apply(world, body)),
            missing: Arc::new(|world, body| S::missing(world, body)),
            check: Arc::new(|value| read::<S>(value.clone()).map(drop)),
            check_in: Arc::new(|world, body, value| read::<S>(value.clone())?.check(world, body)),
        }
    }

    /// Reads the section from the world: its data as JSON, proven to read back.
    pub fn capture(&self, world: &World, body: Option<Entity>) -> Result<Value, String> {
        (self.capture)(world, body)
    }

    /// Puts `data` (at this entry's version) into the world. It does not look first: whoever
    /// applies calls [`check_in`](Self::check_in) before (`load_parts` and `apply_sections` do),
    /// and an `Err` from here means the world may have changed.
    pub fn apply(&self, world: &mut World, body: Option<Entity>, data: Value) -> Result<(), String> {
        (self.apply)(world, body, data)
    }

    /// [`SaveSection::check`] on `data` (at this entry's version): whether the world can take it,
    /// asked before anything of the load is applied. Changes nothing.
    pub fn check_in(&self, world: &World, body: Option<Entity>, data: &Value) -> Result<(), String> {
        (self.check_in)(world, body, data)
    }

    /// What to do for a file that has no such section.
    pub fn missing(&self, world: &mut World, body: Option<Entity>) -> Result<(), String> {
        (self.missing)(world, body)
    }

    /// Whether `data` reads as this section's struct: part of loading a file, so one whose
    /// JSON is valid but wrong counts as damaged and a backup is tried.
    pub fn check(&self, data: &Value) -> Result<(), String> {
        (self.check)(data)
    }

    /// A character component: `Option<C>` of the body.
    pub(crate) fn character_component<C>(name: &str, version: u32) -> Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Serialize + DeserializeOwned + Clone,
    {
        let need = |body: Option<Entity>| body.ok_or_else(|| "there is no player body to take the character from".to_owned());
        Self {
            name: name.to_owned(),
            kind: SaveKind::Character,
            version,
            source: source_of::<C>(),
            capture: Arc::new(move |world, body| {
                let data: Option<C> = world.get::<C>(need(body)?).cloned();
                verified(&data)
            }),
            apply: Arc::new(move |world, body, value| {
                let body = need(body)?;
                if let Some(component) = read::<Option<C>>(value)? {
                    world.get_entity_mut(body).map_err(|e| e.to_string())?.insert(component);
                }
                Ok(())
            }),
            missing: Arc::new(|_, _| Ok(())),
            check: Arc::new(|value| read::<Option<C>>(value.clone()).map(drop)),
            // A body that must be there to take the component.
            check_in: Arc::new(move |world, body, _| world.get_entity(need(body)?).map(drop).map_err(|e| e.to_string())),
        }
    }

    /// A world component: a map from [`SaveKey`] to the component, over entities with a key.
    pub(crate) fn world_component<C>(name: &str, version: u32) -> Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Serialize + DeserializeOwned + Clone,
    {
        let label = name.to_owned();
        Self {
            name: label.clone(),
            kind: SaveKind::World,
            version,
            source: source_of::<C>(),
            capture: Arc::new(move |world, _| {
                let mut map: BTreeMap<String, C> = BTreeMap::new();
                if let Some(mut q) = world.try_query::<(&SaveKey, &C)>() {
                    for (key, component) in q.iter(world) {
                        if map.insert(key.0.clone(), component.clone()).is_some() {
                            return Err(format!("SaveKey `{}` is on two entities; a key must name one", key.0));
                        }
                    }
                }
                verified(&map)
            }),
            apply: Arc::new(move |world, _, value| {
                let saved: BTreeMap<String, C> = read(value)?;
                let mut by_key: BTreeMap<String, Entity> = BTreeMap::new();
                let mut q = world.query::<(Entity, &SaveKey)>();
                for (entity, key) in q.iter(world) {
                    by_key.insert(key.0.clone(), entity);
                }
                let mut absent = Vec::new();
                for (key, component) in saved {
                    match by_key.get(&key) {
                        Some(entity) => {
                            world.entity_mut(*entity).insert(component);
                        }
                        None => absent.push(key),
                    }
                }
                if !absent.is_empty() {
                    ai_core::journal::log(
                        world,
                        ai_core::JournalKind::Warn,
                        format!("save: section {label} names {} that this world does not have: {}", plural(absent.len(), "entity", "entities"), absent.join(", ")),
                    );
                }
                Ok(())
            }),
            missing: Arc::new(|_, _| Ok(())),
            check: Arc::new(|value| read::<BTreeMap<String, C>>(value.clone()).map(drop)),
            check_in: Arc::new(|_, _, _| Ok(())),
        }
    }

    /// A world resource.
    pub(crate) fn resource<R>(name: &str, version: u32) -> Self
    where
        R: Resource + Serialize + DeserializeOwned + Clone,
    {
        let label = name.to_owned();
        Self {
            name: label,
            kind: SaveKind::World,
            version,
            source: source_of::<R>(),
            capture: Arc::new(|world, _| {
                let resource = world
                    .get_resource::<R>()
                    .ok_or_else(|| format!("the world has no resource {}; insert it, or do not save it", std::any::type_name::<R>()))?;
                verified(resource)
            }),
            apply: Arc::new(|world, _, value| {
                world.insert_resource(read::<R>(value)?);
                Ok(())
            }),
            missing: Arc::new(|_, _| Ok(())),
            check: Arc::new(|value| read::<R>(value.clone()).map(drop)),
            check_in: Arc::new(|_, _, _| Ok(())),
        }
    }
}

pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A migration: the section's data at version `n` in, at version `n + 1` out.
pub type Migration = fn(Value) -> Result<Value, String>;

/// Everything the game saves, in the order it is applied.
#[derive(Resource, Default, Clone)]
pub struct SaveRegistry {
    sections: Vec<SectionEntry>,
    migrations: BTreeMap<(String, u32), Migration>,
}

impl SaveRegistry {
    /// Adds a section; one with the same name is replaced in place. A section of another crate
    /// (a game's in place of the engine's) has data of its own and starts its own migration
    /// history: the migrations registered for the one it replaces, which read that one's old
    /// data, are removed. Panics on a bad name,
    /// version 0, or a name that is already registered as the other kind.
    pub fn add(&mut self, entry: SectionEntry) {
        assert!(valid_name(&entry.name), "save section name `{}` must be lowercase letters, digits and `_`, at most 48", entry.name);
        assert!(entry.version >= 1, "save section `{}`: versions start at 1", entry.name);
        match self.sections.iter_mut().find(|s| s.name == entry.name) {
            Some(old) => {
                assert!(
                    old.kind == entry.kind,
                    "save section `{}` is registered as a {} section and cannot become a {} section",
                    entry.name,
                    old.kind,
                    entry.kind
                );
                if old.source != entry.source {
                    self.migrations.retain(|(name, _), _| *name != entry.name);
                }
                *old = entry;
            }
            None => self.sections.push(entry),
        }
    }

    /// Registers the migration of section `name` from `from_version` to the next.
    pub fn add_migration(&mut self, name: &str, from_version: u32, migrate: Migration) {
        self.migrations.insert((name.to_owned(), from_version), migrate);
    }

    /// Every section, in the order sections are applied.
    pub fn sections(&self) -> &[SectionEntry] {
        &self.sections
    }

    /// The sections of one kind, in order.
    pub fn of_kind(&self, kind: SaveKind) -> impl Iterator<Item = &SectionEntry> {
        self.sections.iter().filter(move |s| s.kind == kind)
    }

    pub fn get(&self, name: &str) -> Option<&SectionEntry> {
        self.sections.iter().find(|s| s.name == name)
    }

    pub fn migration(&self, name: &str, from_version: u32) -> Option<Migration> {
        self.migrations.get(&(name.to_owned(), from_version)).copied()
    }

    /// The versions section `name` has a migration from, ascending.
    pub fn migrations_of(&self, name: &str) -> Vec<u32> {
        self.migrations.keys().filter(|(n, _)| n == name).map(|(_, v)| *v).collect()
    }

    /// Whether every section can read every version before its own: the chain
    /// `1..VERSION` has no gap. The first problem is named with the call that fixes it.
    pub fn check_chains(&self) -> Result<(), String> {
        for section in &self.sections {
            for from in 1..section.version {
                if self.migration(&section.name, from).is_none() {
                    return Err(format!(
                        "save section `{name}` is at version {to} but has no migration from version {from}: \
                         add_save_migration(\"{name}\", {from}, |old| ...) turns version {from} into {next}",
                        name = section.name,
                        to = section.version,
                        next = from + 1
                    ));
                }
            }
        }
        for (name, from) in self.migrations.keys() {
            match self.get(name) {
                None => return Err(format!("a save migration is registered for section `{name}`, which no one registers (a typo?)")),
                Some(s) if *from >= s.version => {
                    return Err(format!(
                        "save migration `{name}` from version {from}: the section is at version {} and this one would never run",
                        s.version
                    ))
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct Gold(u32);

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct Purse(u32);

    // A section with a schema of its own.
    #[derive(Serialize, Deserialize)]
    struct WalletData {
        gold: u32,
    }
    impl SaveSection for WalletData {
        const NAME: &'static str = "wallet";
        const VERSION: u32 = 3;
        const KIND: SaveKind = SaveKind::Character;
        fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
            Ok(WalletData { gold: 1 })
        }
        fn apply(self, _: &mut World, _: Option<Entity>) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Resource)]
    struct Closed;

    // A section that looks at its data and the world before it is applied.
    #[derive(Serialize, Deserialize)]
    struct Picky {
        limit: u32,
    }
    impl SaveSection for Picky {
        const NAME: &'static str = "picky";
        const VERSION: u32 = 1;
        const KIND: SaveKind = SaveKind::World;
        fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
            Ok(Picky { limit: 1 })
        }
        fn apply(self, _: &mut World, _: Option<Entity>) -> Result<(), String> {
            Ok(())
        }
        fn check(&self, world: &World, _: Option<Entity>) -> Result<(), String> {
            if self.limit > 10 {
                return Err(format!("limit {} is above 10", self.limit));
            }
            if world.contains_resource::<Closed>() {
                return Err("the world is closed".to_owned());
            }
            Ok(())
        }
    }

    #[test]
    fn an_entry_asks_the_section_whether_the_world_can_take_its_data() {
        let mut world = World::new();
        let picky = SectionEntry::of::<Picky>();
        assert_eq!(picky.check_in(&world, None, &json!({"limit": 3})), Ok(()));
        assert_eq!(picky.check_in(&world, None, &json!({"limit": 30})), Err("limit 30 is above 10".to_owned()));
        world.insert_resource(Closed);
        assert_eq!(picky.check_in(&world, None, &json!({"limit": 3})), Err("the world is closed".to_owned()));
        assert!(picky.check_in(&world, None, &json!("nonsense")).is_err(), "data that does not read is not accepted");
        // The default accepts; a character component wants a body that exists.
        assert_eq!(SectionEntry::of::<WalletData>().check_in(&world, None, &json!({"gold": 1})), Ok(()));
        let purse = SectionEntry::character_component::<Purse>("purse", 1);
        let body = world.spawn(Purse(1)).id();
        assert_eq!(purse.check_in(&world, Some(body), &json!(7)), Ok(()));
        assert!(purse.check_in(&world, None, &json!(7)).unwrap_err().contains("no player body"));
        world.despawn(body);
        assert!(purse.check_in(&world, Some(body), &json!(7)).is_err(), "the body is gone");
        assert_eq!(SectionEntry::world_component::<Purse>("chests", 1).check_in(&world, None, &json!({})), Ok(()));
    }

    #[test]
    fn a_gap_in_a_migration_chain_is_named() {
        let mut registry = SaveRegistry::default();
        registry.add(SectionEntry::of::<WalletData>());
        let err = registry.check_chains().unwrap_err();
        assert!(err.contains("`wallet` is at version 3 but has no migration from version 1"), "{err}");
        registry.add_migration("wallet", 1, Ok);
        let err = registry.check_chains().unwrap_err();
        assert!(err.contains("no migration from version 2") && err.contains("add_save_migration(\"wallet\", 2"), "{err}");
        registry.add_migration("wallet", 2, Ok);
        assert_eq!(registry.check_chains(), Ok(()));
        assert_eq!(registry.migrations_of("wallet"), [1, 2]);
        registry.add_migration("wallet", 3, Ok);
        assert!(registry.check_chains().unwrap_err().contains("would never run"));
        registry.migrations.remove(&("wallet".to_owned(), 3));
        registry.add_migration("walet", 1, Ok);
        assert!(registry.check_chains().unwrap_err().contains("no one registers"));
    }

    #[test]
    fn registering_again_replaces_in_place() {
        let mut registry = SaveRegistry::default();
        registry.add(SectionEntry::character_component::<Purse>("a", 1));
        registry.add(SectionEntry::character_component::<Purse>("b", 1));
        registry.add(SectionEntry::character_component::<Purse>("a", 2));
        let order: Vec<(&str, u32)> = registry.sections().iter().map(|s| (s.name.as_str(), s.version)).collect();
        assert_eq!(order, [("a", 2), ("b", 1)]);
        assert_eq!(registry.sections()[0].source, "shards_save");
    }

    #[test]
    fn a_section_of_another_crate_takes_the_name_without_the_old_ones_migrations() {
        let mut registry = SaveRegistry::default();
        registry.add(SectionEntry::of::<WalletData>());
        registry.add_migration("wallet", 1, Ok);
        registry.add_migration("wallet", 2, Ok);
        registry.add(SectionEntry::of::<WalletData>());
        assert_eq!(registry.migrations_of("wallet"), [1, 2], "the same crate's section again keeps its history");
        // A game's own `wallet` at its version 1: the history of the one it replaces would never run.
        let mut game = SectionEntry::character_component::<Purse>("wallet", 1);
        game.source = "game".into();
        registry.add(game);
        assert!(registry.migrations_of("wallet").is_empty());
        assert_eq!(registry.check_chains(), Ok(()));
        registry.add_migration("wallet", 1, Ok);
        assert!(registry.check_chains().unwrap_err().contains("would never run"), "the game's own migrations are its to keep in step");
    }

    #[test]
    #[should_panic(expected = "cannot become a world section")]
    fn a_name_keeps_its_kind() {
        let mut registry = SaveRegistry::default();
        registry.add(SectionEntry::character_component::<Purse>("a", 1));
        registry.add(SectionEntry::world_component::<Purse>("a", 1));
    }

    #[test]
    #[should_panic(expected = "must be lowercase")]
    fn names_are_file_keys() {
        SaveRegistry::default().add(SectionEntry::character_component::<Purse>("Has Space", 1));
    }

    #[test]
    fn data_that_does_not_read_back_is_refused_when_capturing() {
        #[derive(Serialize, Deserialize)]
        struct Speed(f32);
        let err = verified(&Speed(f32::NAN)).unwrap_err();
        assert!(err.contains("does not read back") && err.contains("NaN"), "{err}");
        // A pair of impls that drops what it cannot hold.
        #[derive(Serialize, Deserialize)]
        struct Lossy {
            #[serde(deserialize_with = "forget")]
            kept: u32,
        }
        fn forget<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
            u32::deserialize(d).map(|n| n / 2)
        }
        assert!(verified(&Lossy { kept: 9 }).unwrap_err().contains("same data"));
        assert_eq!(verified(&Gold(4)).unwrap(), json!(4));
        assert_eq!(verified(&vec![0.1_f32, -0.0]).unwrap(), json!([0.1, -0.0]));
    }

    #[test]
    fn a_component_section_works_on_a_world() {
        let mut world = World::new();
        let body = world.spawn(Purse(7)).id();
        let entry = SectionEntry::character_component::<Purse>("purse", 1);
        assert_eq!(entry.capture(&world, Some(body)).unwrap(), json!(7));
        assert!(entry.capture(&world, None).unwrap_err().contains("no player body"));
        world.get_mut::<Purse>(body).unwrap().0 = 1;
        entry.apply(&mut world, Some(body), json!(7)).unwrap();
        assert_eq!(world.get::<Purse>(body), Some(&Purse(7)));
        // Absent on the body: saved as null, applied as nothing.
        let other = world.spawn_empty().id();
        assert_eq!(entry.capture(&world, Some(other)).unwrap(), Value::Null);
        entry.apply(&mut world, Some(other), Value::Null).unwrap();
        assert!(world.get::<Purse>(other).is_none());
        assert!(entry.check(&json!("seven")).is_err());
    }

    #[test]
    fn a_world_component_section_goes_by_key() {
        let mut world = World::new();
        world.insert_resource(ai_core::AiJournal::default());
        let a = world.spawn((SaveKey::new("chest.a"), Purse(1))).id();
        world.spawn((SaveKey::new("chest.b"), Purse(2)));
        world.spawn(Purse(99));
        let entry = SectionEntry::world_component::<Purse>("chests", 1);
        let data = entry.capture(&world, None).unwrap();
        assert_eq!(data, json!({"chest.a": 1, "chest.b": 2}));
        world.get_mut::<Purse>(a).unwrap().0 = 50;
        entry.apply(&mut world, None, json!({"chest.a": 5, "chest.zz": 6})).unwrap();
        assert_eq!(world.get::<Purse>(a), Some(&Purse(5)));
        let journal = world.resource::<ai_core::AiJournal>();
        let lines: Vec<String> = journal.after(0).0.map(ToString::to_string).collect();
        assert!(lines.iter().any(|l| l.contains("names 1 entity that this world does not have: chest.zz")), "{lines:?}");
        world.spawn((SaveKey::new("chest.a"), Purse(0)));
        assert!(entry.capture(&world, None).unwrap_err().contains("is on two entities"));
    }
}
