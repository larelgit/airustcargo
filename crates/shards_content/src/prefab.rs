//! Prefabs: entities defined in content, so a game gets data-defined enemies without
//! code.
//!
//! ```ron
//! // content/monsters.prefab.ron
//! {
//!     "skeleton": (
//!         components: {
//!             "Name": "Skeleton",
//!             "AiKind": ("Skeleton"),
//!             "Health": (current: 40.0, max: 40.0),
//!             "Intent": (),
//!         },
//!     ),
//!     "skeleton_captain": (
//!         extends: "skeleton",
//!         components: { "Health": (current: 90.0, max: 90.0) },
//!     ),
//! }
//! ```
//!
//! Components are named by short name (`Health`) or full type path, and their values are
//! read through reflection, like Bevy scenes: every component must be registered
//! (`#[derive(Reflect)]` + `#[reflect(Component)]`). A component that derives `Default`
//! with `#[reflect(Default)]` may leave fields out. `extends` chains merge parent
//! first; a child's component replaces the parent's whole. [`spawn_prefab`] can patch
//! fields at spawn time (JSON merge patch per component).

use std::collections::BTreeMap;

use ai_core::prelude::*;
use ai_core::spatial::fmt_pos;
use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::{PartialReflect, ReflectFromReflect, TypeRegistration, TypeRegistry};
use ron::value::RawValue;
use schemars::JsonSchema;
use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::{closest, ron_options, ContentDb, ContentDef, Refs};

/// An entity template: components by name, optionally on top of another prefab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefabDef {
    /// The prefab this one builds on; its components come first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Component name (`Health` or a full type path) → its value in RON, without
    /// comments and blanks: how a file is formatted changes no definition (nor a
    /// replay's content hash).
    #[serde(default, deserialize_with = "compact_components")]
    pub components: BTreeMap<String, Box<RawValue>>,
}

fn compact_components<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<BTreeMap<String, Box<RawValue>>, D::Error> {
    let components = BTreeMap::<String, Box<RawValue>>::deserialize(deserializer)?;
    components
        .into_iter()
        .map(|(name, value)| {
            let compact = compact_ron(value.get_ron()).into_boxed_str();
            RawValue::from_boxed_ron(compact).map(|value| (name, value)).map_err(serde::de::Error::custom)
        })
        .collect()
}

/// RON without comments and the blanks between tokens (string and char literals kept
/// as written): `(current: 40.0, /* full */ max: 40.0)` reads `(current:40.0,max:40.0)`.
fn compact_ron(ron: &str) -> String {
    let mut out = String::with_capacity(ron.len());
    let mut rest = ron;
    while let Some(c) = rest.chars().next() {
        let len = if c.is_whitespace() {
            c.len_utf8()
        } else if rest.starts_with("//") {
            rest.find('\n').unwrap_or(rest.len())
        } else if rest.starts_with("/*") {
            block_comment_len(rest)
        } else {
            let len = literal_len(rest).unwrap_or(c.len_utf8());
            out.push_str(&rest[..len]);
            len
        };
        rest = &rest[len..];
    }
    out
}

/// Length of the (nested) block comment `rest` starts with.
fn block_comment_len(rest: &str) -> usize {
    let mut depth = 0;
    let mut i = 0;
    while i < rest.len() {
        if rest[i..].starts_with("/*") {
            depth += 1;
            i += 2;
        } else if rest[i..].starts_with("*/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return i;
            }
        } else {
            i += rest[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    rest.len()
}

/// Length of the string (`"…"`, raw `r#"…"#`) or char literal `rest` starts with.
fn literal_len(rest: &str) -> Option<usize> {
    if let Some(raw) = rest.strip_prefix('r') {
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        if raw[hashes..].starts_with('"') {
            let close = format!("\"{}", "#".repeat(hashes));
            let body = 1 + hashes + 1;
            return Some(rest[body..].find(&close).map_or(rest.len(), |end| body + end + close.len()));
        }
        return None;
    }
    let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut chars = rest.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == quote {
            return Some(i + 1);
        }
    }
    Some(rest.len())
}

impl ContentDef for PrefabDef {
    const KIND: &'static str = "prefab";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if let Some(parent) = &self.extends
            && refs.require::<PrefabDef>(parent, "extends")
        {
            let mut chain = vec![id.to_owned()];
            let mut next = Some(parent.clone());
            while let Some(name) = next {
                if chain.contains(&name) {
                    chain.push(name);
                    refs.error(format!("extends goes round in a circle: {}", chain.join(" -> ")));
                    break;
                }
                next = refs.get::<PrefabDef>(&name).and_then(|p| p.extends.clone());
                chain.push(name);
            }
        }
        // Without an app (a bare `check_dir`) there is no registry to check against.
        let Some(registry) = refs.type_registry() else { return };
        for (name, value) in &self.components {
            let checked = resolve_component(registry, name).and_then(|reg| read_component(registry, reg, value.get_ron()).map(|_| ()));
            if let Err(why) = checked {
                refs.error(why);
            }
        }
    }
}

/// The registration of component `name` (short name or full type path), or an error that
/// says what is wrong with the name (unknown, ambiguous, not a reflected component).
/// Prefabs and level files (`shards_level`) name components this way.
pub fn resolve_component<'r>(registry: &'r TypeRegistry, name: &str) -> Result<&'r TypeRegistration, String> {
    let found = registry.get_with_type_path(name).or_else(|| registry.get_with_short_type_path(name));
    match found {
        Some(reg) if reg.data::<ReflectComponent>().is_some() => Ok(reg),
        Some(reg) => Err(format!(
            "`{name}` is not a component that reflection can insert (derive Component, Reflect and add #[reflect(Component)] to {})",
            reg.type_info().type_path()
        )),
        None if registry.is_ambiguous(name) => {
            let paths: Vec<&str> = registry
                .iter()
                .filter(|r| r.type_info().type_path_table().short_path() == name)
                .map(|r| r.type_info().type_path())
                .collect();
            Err(format!("`{name}` names several types; use the full path: {}", paths.join(", ")))
        }
        None => {
            let components = registry.iter().filter(|r| r.data::<ReflectComponent>().is_some());
            Err(match closest(name, components.map(|r| r.type_info().type_path_table().short_path())) {
                Some(close) => format!("unknown component `{name}` (did you mean `{close}`?)"),
                None => format!("unknown component `{name}`; a prefab names components that derive Reflect with #[reflect(Component)]"),
            })
        }
    }
}

fn short_name(reg: &TypeRegistration) -> &'static str {
    reg.type_info().type_path_table().short_path()
}

/// A component value from its RON text, as the concrete type (so inserting it cannot
/// fail halfway). Errors carry no line:col: the text is the compacted value alone, so
/// a position in it is none in the file.
fn read_component(registry: &TypeRegistry, reg: &TypeRegistration, ron: &str) -> Result<Box<dyn Reflect>, String> {
    let name = short_name(reg);
    let options = ron_options();
    let mut de = ron::Deserializer::from_str_with_options(ron, &options).map_err(|e| format!("{name}: {}", e.code))?;
    let value = TypedReflectDeserializer::new(reg, registry).deserialize(&mut de).map_err(|e| format!("{name}: {e}"))?;
    de.end().map_err(|e| format!("{name}: {e}"))?;
    concrete(reg, value.as_ref())
}

/// A component value from JSON (the shape the `get` tool shows), as the concrete type.
pub fn component_from_json(registry: &TypeRegistry, reg: &TypeRegistration, json: &Value) -> Result<Box<dyn Reflect>, String> {
    let value = TypedReflectDeserializer::new(reg, registry).deserialize(json).map_err(|e| format!("{}: {e}", short_name(reg)))?;
    concrete(reg, value.as_ref())
}

fn concrete(reg: &TypeRegistration, value: &dyn PartialReflect) -> Result<Box<dyn Reflect>, String> {
    let from_reflect = reg.data::<ReflectFromReflect>().ok_or_else(|| format!("{} cannot be built through reflection", short_name(reg)))?;
    from_reflect.from_reflect(value).ok_or_else(|| {
        let fields = match reg.type_info() {
            bevy::reflect::TypeInfo::Struct(info) => info.iter().map(|f| f.name()).collect::<Vec<_>>().join(", "),
            _ => String::new(),
        };
        let fields = if fields.is_empty() { String::new() } else { format!(": {fields}") };
        format!(
            "{} is missing fields; give all of them{fields} (or derive Default and add #[reflect(Default)])",
            short_name(reg)
        )
    })
}

/// RFC 7386: objects merge key by key, `null` removes, anything else replaces.
fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(serde_json::Map::new());
    }
    let Value::Object(target) = target else { return };
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// `skeleton_captain` and the prefabs it extends, root first.
fn chain<'a>(db: &'a ContentDb, id: &str) -> Result<Vec<&'a PrefabDef>, String> {
    let mut chain: Vec<(String, &PrefabDef)> = Vec::new();
    let mut next = Some(id.to_owned());
    while let Some(name) = next {
        if chain.iter().any(|(seen, _)| *seen == name) {
            return Err(format!("prefab `{id}` extends itself in a circle"));
        }
        // By id: a spawn reads its prefab and what it extends, not every prefab (the error
        // only lists them).
        let Some(def) = db.get::<PrefabDef>(&name) else {
            let hint = closest(&name, db.peek_ids::<PrefabDef>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            let known: Vec<&str> = db.peek_ids::<PrefabDef>().collect();
            return Err(format!("unknown prefab `{name}`{hint}; known: {}", known.join(", ")));
        };
        next = def.extends.clone();
        chain.push((name, def));
    }
    Ok(chain.into_iter().rev().map(|(_, def)| def).collect())
}

/// The components of a prefab by type path, as built from its files.
type Built<'r> = BTreeMap<&'static str, (&'r TypeRegistration, Box<dyn Reflect>)>;

/// Reads prefab `id` (its `extends` chain first) and applies `overrides`, without
/// spawning anything.
fn build<'r>(db: &ContentDb, registry: &'r TypeRegistry, id: &str, overrides: Option<&Value>) -> Result<Built<'r>, String> {
    let mut components: Built<'r> = BTreeMap::new();
    for def in chain(db, id)? {
        for (name, value) in &def.components {
            let reg = resolve_component(registry, name).map_err(|e| format!("prefab `{id}`: {e}"))?;
            let value = read_component(registry, reg, value.get_ron()).map_err(|e| format!("prefab `{id}`: {e}"))?;
            components.insert(reg.type_info().type_path(), (reg, value));
        }
    }
    match overrides {
        None | Some(Value::Null) => {}
        Some(Value::Object(patches)) => {
            for (name, patch) in patches {
                // A short name several types share means the prefab's own component.
                let own = components.values().map(|(reg, _)| *reg).find(|reg| short_name(reg) == name);
                let reg = resolve_component(registry, name).or_else(|e| own.ok_or(e)).map_err(|e| format!("overrides: {e}"))?;
                let mut json = match components.get(reg.type_info().type_path()) {
                    Some((_, value)) => serde_json::to_value(TypedReflectSerializer::new(value.as_partial_reflect(), registry))
                        .map_err(|e| format!("overrides: {name}: {e}"))?,
                    None => Value::Null,
                };
                merge_patch(&mut json, patch);
                let value = component_from_json(registry, reg, &json).map_err(|e| format!("overrides: {e}"))?;
                components.insert(reg.type_info().type_path(), (reg, value));
            }
        }
        Some(other) => return Err(format!("overrides must be an object of components, e.g. {{\"Health\": {{\"current\": 10}}}}; got {other}")),
    }
    Ok(components)
}

/// Whether prefab `id` with `overrides` can be spawned: the same reading
/// [`spawn_prefab`] does, without the spawn. Level files check their prefab nodes with it.
pub fn check_prefab(db: &ContentDb, registry: &TypeRegistry, id: &str, overrides: Option<&Value>) -> Result<(), String> {
    build(db, registry, id, overrides).map(|_| ())
}

/// Spawns prefab `id` at `transform`. `overrides` patch component fields as JSON
/// (`{"Health": {"current": 10}}`, merge patch per component; a component the prefab
/// lacks is added whole). The `transform` given always wins over a `Transform` in the
/// prefab. Nothing is spawned when any component fails.
pub fn spawn_prefab(world: &mut World, id: &str, transform: Transform, overrides: Option<&Value>) -> Result<Entity, String> {
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let components = build(world.resource::<ContentDb>(), &registry, id, overrides)?;
    let mut entity = world.spawn_empty();
    for (reg, value) in components.values() {
        if let Some(component) = reg.data::<ReflectComponent>() {
            component.insert(&mut entity, value.as_partial_reflect(), &registry);
        }
    }
    entity.insert(transform);
    Ok(entity.id())
}

/// Prefabs [`spawn_prefab_cached`] built, kept as values until a definition of their chain
/// changes: a spell that spawns many objects a second reads each prefab's text once. A cache, not
/// simulation state: what it holds follows from content.
#[derive(Resource, Default)]
pub struct PrefabCache {
    built: BTreeMap<String, Cached>,
    builds: u64,
}

/// One built prefab: the definitions of its chain (root first) it was built from, and its
/// components by type path.
struct Cached {
    chain: Vec<PrefabDef>,
    components: Vec<(std::any::TypeId, Box<dyn Reflect>)>,
}

impl PrefabCache {
    /// How many times a prefab was built from text (a first spawn, or one after its definitions
    /// changed).
    pub fn builds(&self) -> u64 {
        self.builds
    }
}

/// [`spawn_prefab`] with no overrides, through the [`PrefabCache`]: the prefab is built from its
/// text on its first spawn and again only when a definition of its `extends` chain changed. Each
/// spawn reads the chain from content all the same (a replay's content check sees what it used).
pub fn spawn_prefab_cached(world: &mut World, id: &str, transform: Transform) -> Result<Entity, String> {
    let defs: Vec<PrefabDef> = chain(world.resource::<ContentDb>(), id)?.into_iter().cloned().collect();
    let Some(cache) = world.get_resource::<PrefabCache>() else { return spawn_prefab(world, id, transform, None) };
    let fresh = cache.built.get(id).is_some_and(|c| c.chain == defs);
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    if !fresh {
        let built = build(world.resource::<ContentDb>(), &registry, id, None)?;
        let components = built.into_values().map(|(reg, value)| (reg.type_id(), value)).collect();
        let mut cache = world.resource_mut::<PrefabCache>();
        cache.builds += 1;
        cache.built.insert(id.to_owned(), Cached { chain: defs, components });
    }
    // Out of the world while its values are inserted (an insert's hooks and observers may spawn).
    world.resource_scope(|world, cache: Mut<PrefabCache>| {
        let cached = cache.built.get(id).expect("built above");
        let mut entity = world.spawn_empty();
        for (type_id, value) in &cached.components {
            if let Some(component) = registry.get(*type_id).and_then(|reg| reg.data::<ReflectComponent>()) {
                component.insert(&mut entity, value.as_partial_reflect(), &registry);
            }
        }
        entity.insert(transform);
        Ok(entity.id())
    })
}

/// Spawn a prefab (an entity defined in content/**/*.prefab.ron) at (x, z). List them
/// with `content_list kind=prefab`.
#[derive(Deserialize, JsonSchema)]
pub struct SpawnPrefabArgs {
    /// Prefab id, e.g. "skeleton".
    pub id: String,
    /// East (+X) in meters.
    pub x: f32,
    /// South (+Z) in meters.
    pub z: f32,
    /// Height. Default 0.
    pub y: Option<f32>,
    /// Component fields to change, e.g. {"Health": {"current": 10}}.
    pub overrides: Option<Value>,
}

pub(crate) fn spawn_prefab_tool(In(call): In<Call<SpawnPrefabArgs>>, world: &mut World) -> ToolResult {
    let SpawnPrefabArgs { id, x, z, y, overrides } = call.args;
    let at = Vec3::new(x, y.unwrap_or(0.0), z);
    let entity = spawn_prefab(world, &id, Transform::from_translation(at), overrides.as_ref())?;
    let shown = world.get::<AiId>(entity).map_or_else(|| format!("{entity} (not AI-visible)"), ToString::to_string);
    Ok(format!("spawned {shown} {id} at {}", fmt_pos(at)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compacting_keeps_literals_and_drops_formatting() {
        let written = " (current: 40.0, /* full /* nested */ */ max: 40.0) // hp\n";
        assert_eq!(compact_ron(written), "(current:40.0,max:40.0)");
        assert_eq!(compact_ron("\n(\n  current:40.0,\n  max:  40.0,\n)"), "(current:40.0,max:40.0,)");
        let literals = r##"( name: "a // b \" c", raw: r#"x " y"#, c: ' ', bar: r#ident )"##;
        assert_eq!(compact_ron(literals), r##"(name:"a // b \" c",raw:r#"x " y"#,c:' ',bar:r#ident)"##);
    }

    #[test]
    fn merge_patch_follows_rfc_7386() {
        let mut target = json!({"current": 40.0, "max": 40.0, "tags": ["a"]});
        merge_patch(&mut target, &json!({"current": 10, "tags": null, "new": {"x": 1}}));
        assert_eq!(target, json!({"current": 10, "max": 40.0, "new": {"x": 1}}));
        merge_patch(&mut target, &json!("Idle"));
        assert_eq!(target, json!("Idle"));
    }
}
