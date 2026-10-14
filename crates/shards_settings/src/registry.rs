//! The registry of settings groups: what the tools list and set, what is journaled when a
//! setting changes, and how a game adds groups of its own.
//!
//! A group is any Bevy resource that derives `SettingsGroup` (Bevy's settings framework saves
//! and loads it) and reflects. The registry keeps, per group, typed accessors behind one
//! object-safe trait, so the tools and the change watcher work on any group by name and JSON,
//! never knowing its type.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;

use ai_core::{AiAppExt, AiJournal, JournalKind, SimTick};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::{FromReflect, GetTypeRegistration, PartialReflect, TypeInfo, TypeRegistry, Typed};
use bevy::settings::{ReflectSettingsGroup, SettingsGroup};
use serde::de::DeserializeSeed;
use serde_json::Value;

use crate::toml_safe::check_toml_safe;

/// One registered group, seen through JSON.
pub(crate) trait GroupAccess: Send + Sync {
    /// The TOML table: `graphics`.
    fn name(&self) -> &'static str;
    /// The Rust type, for messages.
    fn type_path(&self) -> &'static str;
    fn info(&self) -> &'static TypeInfo;
    fn type_id(&self) -> TypeId;
    /// The group's values as JSON; `None` when the resource is missing.
    fn read(&self, world: &World) -> Option<Value>;
    /// The values `Default` gives.
    fn defaults(&self, world: &World) -> Option<Value>;
    /// Whether the resource changed since this system last looked.
    fn changed(&self, world: &World) -> bool;
    /// Replaces the group from JSON: deserialized by its type, refused when a check says so.
    /// `Ok(true)` when the values changed.
    fn replace(&self, world: &mut World, json: &Value) -> Result<bool, String>;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

struct Group<G: SettingsGroup> {
    name: &'static str,
    checks: Vec<fn(&G) -> Result<(), String>>,
}

pub(crate) fn to_json(value: &dyn PartialReflect, registry: &TypeRegistry) -> Option<Value> {
    serde_json::to_value(TypedReflectSerializer::new(value, registry)).ok()
}

impl<G> GroupAccess for Group<G>
where
    G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed,
{
    fn name(&self) -> &'static str {
        self.name
    }

    fn type_path(&self) -> &'static str {
        G::type_path()
    }

    fn info(&self) -> &'static TypeInfo {
        G::type_info()
    }

    fn type_id(&self) -> TypeId {
        TypeId::of::<G>()
    }

    fn read(&self, world: &World) -> Option<Value> {
        let group = world.get_resource::<G>()?;
        to_json(group, &world.resource::<AppTypeRegistry>().read())
    }

    fn defaults(&self, world: &World) -> Option<Value> {
        to_json(&G::default(), &world.resource::<AppTypeRegistry>().read())
    }

    fn changed(&self, world: &World) -> bool {
        world.get_resource_ref::<G>().is_some_and(|g| g.is_changed())
    }

    fn replace(&self, world: &mut World, json: &Value) -> Result<bool, String> {
        let types = world.resource::<AppTypeRegistry>().clone();
        let types = types.read();
        let registration = types.get(TypeId::of::<G>()).ok_or_else(|| format!("{} is not registered", G::type_path()))?;
        let dynamic = TypedReflectDeserializer::new(registration, &types).deserialize(json.clone()).map_err(|e| e.to_string())?;
        let next = G::from_reflect(&*dynamic).ok_or_else(|| format!("{} cannot be rebuilt from {json}", G::type_path()))?;
        for check in &self.checks {
            check(&next)?;
        }
        let (old, new) = (world.get_resource::<G>().and_then(|g| to_json(g, &types)), to_json(&next, &types));
        if old == new {
            return Ok(false);
        }
        world.insert_resource(next);
        Ok(true)
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Every group, in registration order (the engine's four first).
#[derive(Resource, Default)]
pub struct SettingsRegistry {
    groups: Vec<Box<dyn GroupAccess>>,
    /// Each group's values as the journal last saw them, by type.
    seen: BTreeMap<&'static str, Value>,
}

impl SettingsRegistry {
    pub(crate) fn groups(&self) -> impl Iterator<Item = &dyn GroupAccess> {
        self.groups.iter().map(|g| &**g)
    }

    /// The groups named `name` (resources can share one TOML table).
    pub(crate) fn named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a dyn GroupAccess> {
        self.groups().filter(move |g| g.name().eq_ignore_ascii_case(name))
    }

    /// The names of the groups, once each, in order.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = Vec::new();
        for group in self.groups() {
            if !names.contains(&group.name()) {
                names.push(group.name());
            }
        }
        names
    }

    fn add<G>(&mut self, name: &'static str)
    where
        G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed,
    {
        if !self.groups().any(|g| g.type_id() == TypeId::of::<G>()) {
            self.groups.push(Box::new(Group::<G> { name, checks: Vec::new() }));
        }
    }

    fn add_check<G>(&mut self, check: fn(&G) -> Result<(), String>)
    where
        G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed,
    {
        let group = self.groups.iter_mut().find_map(|g| g.as_any_mut().downcast_mut::<Group<G>>());
        if let Some(group) = group {
            group.checks.push(check);
        }
    }
}

/// Registration API for games and engine modules.
pub trait SettingsAppExt {
    /// Adds a settings group: a resource that derives `SettingsGroup` and reflects, saved in
    /// `settings.toml` under its group name and listed by the `settings` tool. Call it while
    /// building the app (the settings plugin reads the registry once the game's builder is
    /// done).
    ///
    /// ```ignore
    /// #[derive(Resource, Reflect, SettingsGroup, Default)]
    /// #[reflect(Resource, SettingsGroup, Default)]
    /// #[settings_group(group = "my_game")]
    /// struct MyGameSettings { show_hints: bool, language: String }
    ///
    /// app.add_settings_group::<MyGameSettings>();
    /// ```
    ///
    /// Panics, naming the field, when the group cannot be written as TOML (a `u64`, a map keyed
    /// by anything but `String`; see [`check_toml_safe`]), or when it lacks
    /// `#[reflect(SettingsGroup)]`.
    fn add_settings_group<G>(&mut self) -> &mut Self
    where
        G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed;

    /// A rule `settings_set` enforces on `G`: it returns why the values are not allowed. A
    /// group can have several.
    fn add_settings_check<G>(&mut self, check: fn(&G) -> Result<(), String>) -> &mut Self
    where
        G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed;
}

impl SettingsAppExt for App {
    fn add_settings_group<G>(&mut self) -> &mut Self
    where
        G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed,
    {
        if let Err(why) = check_toml_safe::<G>() {
            panic!("settings group {}: {why}", G::type_path());
        }
        self.register_type::<G>();
        let name = {
            let types = self.world().resource::<AppTypeRegistry>().read();
            let Some(group) = types.get(TypeId::of::<G>()).and_then(|r| r.data::<ReflectSettingsGroup>()) else {
                panic!(
                    "settings group {} is not saved: add #[reflect(Resource, SettingsGroup, Default)] next to the derives",
                    G::type_path()
                );
            };
            group.settings_group_name()
        };
        self.init_resource::<G>()
            // Settings are per machine: never part of what a host replicates.
            .net_ignore_resource::<G>()
            .world_mut()
            .get_resource_or_init::<SettingsRegistry>()
            .add::<G>(name);
        self
    }

    fn add_settings_check<G>(&mut self, check: fn(&G) -> Result<(), String>) -> &mut Self
    where
        G: SettingsGroup + Reflect + FromReflect + GetTypeRegistration + Typed,
    {
        self.world_mut().get_resource_or_init::<SettingsRegistry>().add_check::<G>(check);
        self
    }
}

// --- reading values as `group.key = value` lines ---------------------------------------------

/// A value as a player reads it: numbers without the noise of `f32` widened to `f64`, strings
/// quoted, lists and tables as JSON.
pub(crate) fn show(value: &Value) -> String {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(x) if !n.is_i64() && !n.is_u64() => {
                let narrow = x as f32;
                if f64::from(narrow) == x { narrow.to_string() } else { x.to_string() }
            }
            _ => n.to_string(),
        },
        other => other.to_string(),
    }
}

/// The flat `key -> text` form of one group's values, keys as `group.field` and
/// `group.map_field.entry`, fields in the order the type declares them.
pub(crate) fn flatten(group: &str, info: &'static TypeInfo, values: &Value) -> Vec<(String, String)> {
    let Some(object) = values.as_object() else {
        return vec![(group.to_owned(), show(values))];
    };
    let declared: Vec<&str> = match info {
        TypeInfo::Struct(s) => s.iter().map(|f| f.name()).collect(),
        _ => Vec::new(),
    };
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort_by_key(|k| declared.iter().position(|d| d == k).unwrap_or(usize::MAX));
    let mut out = Vec::new();
    for key in keys {
        let value = &object[key];
        match value.as_object() {
            Some(entries) if !entries.is_empty() => {
                for (entry, v) in entries {
                    out.push((format!("{group}.{key}.{entry}"), show(v)));
                }
            }
            Some(_) => out.push((format!("{group}.{key}"), "{}".to_owned())),
            None => out.push((format!("{group}.{key}"), show(value))),
        }
    }
    out
}

/// What changed between two states of a group, as journal lines: `settings: accessibility.camera_shake 1 → 0.3`.
pub(crate) fn changes(group: &str, info: &'static TypeInfo, old: &Value, new: &Value) -> Vec<String> {
    // An empty table is only how `flatten` shows it; whether it is empty is its entries' business.
    let (before, after): (BTreeMap<String, String>, Vec<(String, String)>) = (
        flatten(group, info, old).into_iter().filter(|(_, v)| v != "{}").collect(),
        flatten(group, info, new).into_iter().filter(|(_, v)| v != "{}").collect(),
    );
    let mut lines = Vec::new();
    for (key, now) in &after {
        match before.get(key) {
            Some(was) if was == now => {}
            Some(was) => lines.push(format!("settings: {key} {was} → {now}")),
            None => lines.push(format!("settings: {key} (unset) → {now}")),
        }
    }
    let kept: Vec<&String> = after.iter().map(|(k, _)| k).collect();
    for (key, was) in &before {
        if !kept.contains(&key) {
            lines.push(format!("settings: {key} {was} → (unset)"));
        }
    }
    lines
}

/// The most settings lines one change of one group may journal.
const MAX_LINES: usize = 12;

/// Journals every setting that changed since the last look, once. The first look of a group
/// only records it (loading the file is no change). A system that runs every frame.
pub fn watch_settings(world: &mut World) {
    look(world, true);
}

/// [`watch_settings`] right now, whatever Bevy's change detection says: the tools call it
/// around a change so the journal has the line before the call returns.
pub(crate) fn journal_now(world: &mut World) {
    look(world, false);
}

fn look(world: &mut World, only_changed: bool) {
    if !world.contains_resource::<SettingsRegistry>() {
        return;
    }
    let lines = world.resource_scope(|world, mut registry: Mut<SettingsRegistry>| {
        let registry = &mut *registry;
        let mut lines: Vec<String> = Vec::new();
        for group in registry.groups.iter() {
            let key = group.type_path();
            let first = !registry.seen.contains_key(key);
            if !first && only_changed && !group.changed(world) {
                continue;
            }
            let Some(now) = group.read(world) else { continue };
            if let Some(before) = registry.seen.get(key)
                && *before != now
            {
                let mut changed = changes(group.name(), group.info(), before, &now);
                if changed.len() > MAX_LINES {
                    let more = changed.len() - MAX_LINES;
                    changed.truncate(MAX_LINES);
                    changed.push(format!("settings: … and {more} more in {}", group.name()));
                }
                lines.extend(changed);
            }
            registry.seen.insert(key, now);
        }
        lines
    });
    if lines.is_empty() {
        return;
    }
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    if let Some(mut journal) = world.get_resource_mut::<AiJournal>() {
        for line in lines {
            journal.push(tick, JournalKind::Event, line);
        }
    }
}
