//! Tools: `settings` and `settings_set`. An agent reads and changes the settings the way a
//! player does in the settings screen: by group and name, with the value as text.
//!
//! Both work on any registered group through its JSON form: the value text is read by the
//! field's reflected type (a number for a number, one of the variants for an enum), the whole
//! group is rebuilt from JSON by its own type, the group's checks run, and only then is the
//! resource replaced. A refused change leaves everything as it was.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::prelude::*;
use bevy::reflect::TypeInfo;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::registry::{changes, flatten, journal_now, GroupAccess, SettingsRegistry};
use crate::store::{save_soon_in, SettingsStorage};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "settings", Scope::Player, ToolKind::Query, settings);
    // A control tool: settings are per machine and not part of the game, so replays do not record it.
    register_tool(world, "settings_set", Scope::Player, ToolKind::Control, settings_set);
}

/// The player's settings: graphics, audio, controls, accessibility (and any a game adds),
/// as `group.key = value` lines. Say which are saved and where.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SettingsArgs {
    /// One group: `graphics`, `audio`, `controls` or `accessibility`. Default: all of them.
    pub group: Option<String>,
}

fn settings(In(call): In<Call<SettingsArgs>>, world: &World) -> ToolResult {
    let registry = world.get_resource::<SettingsRegistry>().ok_or("this game has no settings")?;
    let wanted = call.args.group;
    if let Some(name) = &wanted
        && registry.named(name).next().is_none()
    {
        return Err(unknown_group(&registry.names(), name));
    }
    let mut out = String::new();
    for group in registry.groups().filter(|g| wanted.as_deref().is_none_or(|w| g.name().eq_ignore_ascii_case(w))) {
        let Some(values) = group.read(world) else { continue };
        for (key, value) in flatten(group.name(), group.info(), &values) {
            out.push_str(&format!("{key} = {value}\n"));
        }
    }
    out.push_str(&storage_line(world));
    Ok(out)
}

fn storage_line(world: &World) -> String {
    match world.get_resource::<SettingsStorage>() {
        Some(SettingsStorage { file: Some(file) }) => {
            format!("# kept in {}; a change is written a second after the last one, and when the game exits", file.display())
        }
        Some(_) => "# the platform has no settings folder: nothing is written".to_owned(),
        None => "# not saved: this run keeps no settings file (--no-settings, a headless run or a test); changes last until it exits".to_owned(),
    }
}

fn unknown_group(names: &[&str], wanted: &str) -> String {
    let hint = closest(wanted, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    format!("no settings group `{wanted}`{hint}; groups: {}", names.join(", "))
}

/// Change a setting the way the settings screen does, and save it. `key` is a field
/// (`fov_deg`) or an entry of a table field (`bindings.gameplay.jump`); `value` is text: a
/// number, `true`/`false`, one of an enum's words (`reduced`), `1920x1080`, or JSON for a list
/// (`["Key(KeyJ)"]`). `reset` puts a setting (or, without `key`, the whole group) back to its
/// default. A value that does not fit is refused with what is allowed.
#[derive(Deserialize, JsonSchema)]
pub struct SettingsSetArgs {
    /// The group, as `settings` lists it.
    pub group: String,
    /// The setting inside the group. Leave out with `reset` to reset the group.
    pub key: Option<String>,
    /// The new value as text (a number or `true` is as good as its text).
    #[serde(default, deserialize_with = "text_or_scalar")]
    #[schemars(with = "Option<String>")]
    pub value: Option<String>,
    /// Back to the default.
    #[serde(default)]
    pub reset: bool,
}

/// `value=0.3` reaches the tool as the number 0.3, `value=true` as a boolean, `value=["a"]` as a
/// list: all of them are the text they were typed as.
fn text_or_scalar<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text),
        Some(other) => Some(other.to_string()),
    })
}

fn settings_set(In(call): In<Call<SettingsSetArgs>>, world: &mut World) -> ToolResult {
    if !world.contains_resource::<SettingsRegistry>() {
        return Err("this game has no settings".to_owned());
    }
    // The journal sees the values as they were, then as they are.
    journal_now(world);
    let outcome = world.resource_scope(|world, registry: Mut<SettingsRegistry>| set(world, &registry, call.args));
    let (before, after) = outcome?;
    journal_now(world);
    if before.iter().map(|g| &g.2).eq(after.iter().map(|g| &g.2)) {
        return Ok(format!("ok: nothing changed (already so)\n{}", storage_line(world)));
    }
    save_soon_in(world);
    let mut out = String::from("ok:");
    for ((info, name, old), (_, _, new)) in before.iter().zip(&after) {
        for line in changes(name, info, old, new) {
            out.push_str(&format!("\n  {}", line.trim_start_matches("settings: ")));
        }
    }
    out.push('\n');
    out.push_str(&storage_line(world));
    Ok(out)
}

/// One group before or after: its type information, name and values.
type Snapshot = Vec<(&'static TypeInfo, &'static str, Value)>;

fn set(world: &mut World, registry: &SettingsRegistry, args: SettingsSetArgs) -> Result<(Snapshot, Snapshot), String> {
    let SettingsSetArgs { group, key, value, reset } = args;
    let owners: Vec<&dyn GroupAccess> = registry.named(&group).collect();
    if owners.is_empty() {
        return Err(unknown_group(&registry.names(), &group));
    }
    let snapshot = |world: &World| -> Snapshot { owners.iter().filter_map(|g| Some((g.info(), g.name(), g.read(world)?))).collect() };
    let before = snapshot(world);

    // The whole group back to its defaults.
    if reset && key.is_none() {
        for owner in &owners {
            let defaults = owner.defaults(world).ok_or("cannot read the defaults")?;
            owner.replace(world, &defaults)?;
        }
        return Ok((before, snapshot(world)));
    }

    let key = key.ok_or_else(|| {
        let keys: Vec<String> = owners.iter().flat_map(|g| key_names(g.info())).collect();
        format!("name a `key` of {group}: {}", keys.join(", "))
    })?;
    let (field, entry) = match key.split_once('.') {
        Some((field, entry)) => (field, Some(entry)),
        None => (key.as_str(), None),
    };
    let Some((owner, field_info)) = owners.iter().find_map(|g| Some((*g, field_type(g.info(), field)?))) else {
        let keys: Vec<String> = owners.iter().flat_map(|g| key_names(g.info())).collect();
        let hint = closest(field, keys.iter().map(|k| k.split('.').next().unwrap_or(k))).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("{group} has no setting `{field}`{hint}; settings: {}", keys.join(", ")));
    };
    let mut json = owner.read(world).ok_or("cannot read the group")?;
    let label = format!("{}.{key}", owner.name());

    if reset {
        let defaults = owner.defaults(world).ok_or("cannot read the defaults")?;
        let object = json.as_object_mut().ok_or("the group is not a table")?;
        match entry {
            // A table entry has no default: it is removed.
            Some(entry) => {
                if let Some(table) = object.get_mut(field).and_then(Value::as_object_mut) {
                    table.remove(entry);
                }
            }
            None => {
                object.insert(field.to_owned(), defaults.get(field).cloned().unwrap_or(Value::Null));
            }
        }
    } else {
        let text = value.ok_or_else(|| format!("give a `value` for {label} ({}), or `reset=true`", expects(field_info, entry.is_some())))?;
        let object = json.as_object_mut().ok_or("the group is not a table")?;
        match entry {
            Some(entry) => {
                let TypeInfo::Map(map) = field_info else {
                    return Err(format!("{}.{field} is not a table, so it has no entries: set it as `key={field}`", owner.name()));
                };
                let parsed = parse_text(map.value_info(), &label, &text)?;
                let table = object.entry(field.to_owned()).or_insert_with(|| json!({}));
                table.as_object_mut().ok_or("not a table")?.insert(entry.to_owned(), parsed);
            }
            None => {
                object.insert(field.to_owned(), parse_text(Some(field_info), &label, &text)?);
            }
        }
    }
    // A group's own checks name the setting already; an error of the deserializer does not.
    owner.replace(world, &json).map_err(|why| if why.contains(&label) { why } else { format!("{label}: {why}") })?;
    Ok((before, snapshot(world)))
}

/// The settings of a group type: fields, with `.<entry>` after the tables.
fn key_names(info: &'static TypeInfo) -> Vec<String> {
    let TypeInfo::Struct(s) = info else { return Vec::new() };
    s.iter().map(|f| if matches!(f.type_info(), Some(TypeInfo::Map(_))) { format!("{}.<entry>", f.name()) } else { f.name().to_owned() }).collect()
}

fn field_type(info: &'static TypeInfo, field: &str) -> Option<&'static TypeInfo> {
    let TypeInfo::Struct(s) = info else { return None };
    s.field(field)?.type_info()
}

/// What a setting of this type takes, for messages.
fn expects(info: &'static TypeInfo, entry: bool) -> String {
    match info {
        TypeInfo::Opaque(_) if info.is::<bool>() => "true or false".to_owned(),
        TypeInfo::Opaque(_) if info.is::<f32>() || info.is::<f64>() => "a number".to_owned(),
        TypeInfo::Opaque(_) if info.is::<String>() => "text".to_owned(),
        TypeInfo::Opaque(_) if int_range(info).is_some() => "a whole number".to_owned(),
        TypeInfo::Enum(e) => format!("one of {}", e.variant_names().join(", ")),
        TypeInfo::List(_) | TypeInfo::Array(_) => "a list: JSON like [\"a\",\"b\"], or one item".to_owned(),
        TypeInfo::Map(m) if !entry => format!("a table: use key={{field}}.<entry>, or JSON; entries are {}", expects_opt(m.value_info())),
        TypeInfo::Map(m) => expects_opt(m.value_info()),
        _ => "JSON".to_owned(),
    }
}

fn expects_opt(info: Option<&'static TypeInfo>) -> String {
    info.map_or_else(|| "JSON".to_owned(), |i| expects(i, true))
}

/// The range of a whole-number type, if it is one TOML holds.
fn int_range(info: &'static TypeInfo) -> Option<(i64, i64)> {
    Some(if info.is::<i8>() {
        (i8::MIN.into(), i8::MAX.into())
    } else if info.is::<i16>() {
        (i16::MIN.into(), i16::MAX.into())
    } else if info.is::<i32>() {
        (i32::MIN.into(), i32::MAX.into())
    } else if info.is::<i64>() || info.is::<isize>() {
        (i64::MIN, i64::MAX)
    } else if info.is::<u8>() {
        (0, u8::MAX.into())
    } else if info.is::<u16>() {
        (0, u16::MAX.into())
    } else if info.is::<u32>() {
        (0, u32::MAX.into())
    } else {
        return None;
    })
}

/// Reads `text` as a value of the reflected type `info`, as JSON that the type deserializes.
fn parse_text(info: Option<&'static TypeInfo>, label: &str, text: &str) -> Result<Value, String> {
    let Some(info) = info else {
        return serde_json::from_str(text).map_err(|e| format!("{label}: `{text}` is not JSON ({e})"));
    };
    let wrong = |what: &str| format!("{label}: `{text}` is not {what}");
    match info {
        TypeInfo::Opaque(_) if info.is::<bool>() => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(Value::Bool(true)),
            "false" | "off" | "no" | "0" => Ok(Value::Bool(false)),
            _ => Err(wrong("true or false")),
        },
        TypeInfo::Opaque(_) if info.is::<f32>() || info.is::<f64>() => {
            let number = text.trim().parse::<f64>().ok().filter(|n| n.is_finite()).ok_or_else(|| wrong("a number (like 0.5)"))?;
            // An f32 field reads the f64 back: keep the shortest f32 text so 0.3 stays 0.3.
            let number = if info.is::<f32>() { f64::from(number as f32) } else { number };
            serde_json::Number::from_f64(number).map(Value::Number).ok_or_else(|| wrong("a number"))
        }
        TypeInfo::Opaque(_) if info.is::<String>() => {
            let trimmed = text.trim();
            match trimmed.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                Some(_) if trimmed.len() >= 2 => serde_json::from_str::<Value>(trimmed).map_err(|_| wrong("text")),
                _ => Ok(Value::String(text.to_owned())),
            }
        }
        TypeInfo::Opaque(_) if info.is::<char>() => {
            let mut chars = text.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(Value::String(c.to_string())),
                _ => Err(wrong("one character")),
            }
        }
        TypeInfo::Opaque(_) => {
            let (lo, hi) = int_range(info).ok_or_else(|| format!("{label}: a {} setting cannot be set from text", info.type_path()))?;
            let n = text.trim().parse::<i64>().map_err(|_| wrong("a whole number"))?;
            if n < lo || n > hi {
                return Err(format!("{label}: {n} is out of range ({lo} to {hi})"));
            }
            Ok(json!(n))
        }
        TypeInfo::Enum(e) => {
            let word = text.trim();
            if let Some(variant) = e.variant_names().iter().find(|v| v.eq_ignore_ascii_case(word)) {
                return Ok(Value::String((*variant).to_owned()));
            }
            serde_json::from_str(word).map_err(|_| format!("{label}: `{text}` is not one of {}", e.variant_names().join(", ")))
        }
        TypeInfo::List(l) => list(l.item_info(), label, text),
        TypeInfo::Array(a) => list(a.item_info(), label, text),
        _ => serde_json::from_str(text).map_err(|e| format!("{label}: `{text}` is not JSON ({e})")),
    }
}

/// A list: JSON when it starts with `[`, else one item.
fn list(item: Option<&'static TypeInfo>, label: &str, text: &str) -> Result<Value, String> {
    if text.trim_start().starts_with('[') {
        return serde_json::from_str(text).map_err(|e| format!("{label}: `{text}` is not a JSON list ({e})"));
    }
    Ok(Value::Array(vec![parse_text(item, label, text)?]))
}

/// The candidate closest to `wanted` by edit distance, if it is close enough to be a typo.
fn closest<'a>(wanted: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let limit = (wanted.chars().count() / 3).max(2);
    // A name typed short (`fov` for `fov_deg`) is as good as a typo.
    let near = |c: &str| (edit_distance(wanted, c), !(c.starts_with(wanted) && !wanted.is_empty()));
    candidates
        .map(|c| (near(c), c))
        .filter(|((d, not_prefix), _)| *d <= limit || !*not_prefix)
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)))
        .map(|(_, c)| c)
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}
