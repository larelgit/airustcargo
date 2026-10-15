//! Reading the controls table entry by entry.
//!
//! Bevy's settings framework reads a group field by field: a field that does not fit its type is
//! skipped whole, without a word. For `controls.bindings` and `controls.hold_or_toggle` (tables of
//! `context.action`) that costs every rebinding when one entry is wrong: a key typed without quotes
//! (`gameplay.jump = [...]` is a nested table in TOML), a string where a list belongs. This reads the
//! file again after Bevy did and puts back what is good, naming what is not.

use std::collections::BTreeMap;

use toml::Value;

use crate::ControlSettings;

/// What a second reading found that Bevy's did not.
#[derive(Default, Debug, PartialEq)]
pub(crate) struct Recovered {
    pub bindings: BTreeMap<String, Vec<String>>,
    pub modes: BTreeMap<String, String>,
    /// One line per entry that was repaired or dropped.
    pub notes: Vec<String>,
    /// Whether some entry could not be used at all (its text is in the file only).
    pub dropped: bool,
}

/// Every leaf of `table` with its dotted key, and whether it came out of a nested table.
fn leaves<'a>(table: &'a toml::Table, prefix: &str, nested: bool, out: &mut Vec<(String, &'a Value, bool)>) {
    for (key, value) in table {
        let key = if prefix.is_empty() { key.clone() } else { format!("{prefix}.{key}") };
        match value {
            Value::Table(inner) => leaves(inner, &key, true, out),
            leaf => out.push((key, leaf, nested)),
        }
    }
}

/// The entries of the file's `[controls]` table that `loaded` (what Bevy's reading made of it) lacks.
pub(crate) fn recover(controls: &toml::Table, loaded: &ControlSettings) -> Recovered {
    let mut found = Recovered::default();
    if let Some(table) = controls.get("bindings").and_then(Value::as_table) {
        let mut entries = Vec::new();
        leaves(table, "", false, &mut entries);
        for (key, value, nested) in entries {
            let label = format!("controls.bindings.{key}");
            let list = match value {
                Value::Array(items) => items.iter().map(|i| i.as_str().map(str::to_owned)).collect::<Option<Vec<String>>>(),
                Value::String(one) => Some(vec![one.clone()]),
                _ => None,
            };
            let Some(list) = list else {
                found.notes.push(format!("settings: {label}: expected a list of binding texts, found {}; ignored", value.type_str()));
                found.dropped = true;
                continue;
            };
            if loaded.bindings.get(&key) == Some(&list) {
                continue;
            }
            if nested {
                found.notes.push(format!("settings: {label} was written without quotes, so TOML made it a nested table; read as \"{key}\" (quote it: \"{key}\" = [...])"));
            } else if value.is_str() {
                found.notes.push(format!("settings: {label} is a text, not a list; read as a list of one"));
            }
            found.bindings.insert(key, list);
        }
    }
    if let Some(table) = controls.get("hold_or_toggle").and_then(Value::as_table) {
        let mut entries = Vec::new();
        leaves(table, "", false, &mut entries);
        for (key, value, nested) in entries {
            let label = format!("controls.hold_or_toggle.{key}");
            let Some(mode) = value.as_str() else {
                found.notes.push(format!("settings: {label}: expected `press`, `hold` or `toggle`, found {}; ignored", value.type_str()));
                found.dropped = true;
                continue;
            };
            if loaded.hold_or_toggle.get(&key).map(String::as_str) == Some(mode) {
                continue;
            }
            if nested {
                found.notes.push(format!("settings: {label} was written without quotes, so TOML made it a nested table; read as \"{key}\""));
            }
            found.modes.insert(key, mode.to_owned());
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controls(text: &str) -> toml::Table {
        toml::from_str::<toml::Table>(text).unwrap().remove("controls").unwrap().try_into().unwrap()
    }

    #[test]
    fn good_entries_next_to_a_bad_one_come_back_and_the_bad_one_is_named() {
        let table = controls(
            "[controls.bindings]\n\"gameplay.jump\" = [\"Key(KeyJ)\"]\ngameplay.parry = [\"Key(KeyP)\"]\n\"gameplay.light\" = \"Mouse(Left)\"\n\"gameplay.heavy\" = 7\n\
             [controls.hold_or_toggle]\n\"gameplay.block\" = 3\n\"gameplay.sprint\" = \"toggle\"\n",
        );
        // What Bevy made of it: the whole table failed, so nothing.
        let found = recover(&table, &ControlSettings::default());
        assert_eq!(found.bindings["gameplay.jump"], ["Key(KeyJ)"]);
        assert_eq!(found.bindings["gameplay.parry"], ["Key(KeyP)"], "the nested table is read as a dotted key");
        assert_eq!(found.bindings["gameplay.light"], ["Mouse(Left)"], "a text is a list of one");
        assert!(!found.bindings.contains_key("gameplay.heavy"));
        assert_eq!(found.modes["gameplay.sprint"], "toggle");
        assert!(found.dropped);
        let notes = found.notes.join("\n");
        assert!(notes.contains("controls.bindings.gameplay.parry was written without quotes"), "{notes}");
        assert!(notes.contains("controls.bindings.gameplay.heavy: expected a list of binding texts, found integer"), "{notes}");
        assert!(notes.contains("controls.hold_or_toggle.gameplay.block: expected `press`"), "{notes}");
    }

    #[test]
    fn what_bevy_read_fine_is_left_alone() {
        let table = controls("[controls.bindings]\n\"gameplay.jump\" = [\"Key(KeyJ)\"]\n[controls.hold_or_toggle]\n\"gameplay.sprint\" = \"toggle\"\n");
        let mut loaded = ControlSettings::default();
        loaded.bindings.insert("gameplay.jump".into(), vec!["Key(KeyJ)".into()]);
        loaded.hold_or_toggle.insert("gameplay.sprint".into(), "toggle".into());
        assert_eq!(recover(&table, &loaded), Recovered::default());
    }
}
