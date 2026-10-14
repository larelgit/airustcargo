//! The player's controls as settings (E8.6): [`ControlSettings`] ⇄ [`InputMap`].
//!
//! The input map in play is the game's default controls plus what the player changed. What
//! the player changed lives in [`ControlSettings`] (Bevy's settings framework saves it in
//! `settings.toml`): per `context.action`, the bindings in slot order and the mode. Only
//! differences are stored, so a default action added later still reaches an old file.
//!
//! Both sides can change at any time: `rebind` and any system that edits the map write the
//! difference into the settings; `settings_set` and a hot-reloaded default turn the settings
//! (and the new defaults) into a new map. [`ControlSync`] remembers the last state both agree
//! on to tell which side moved, so neither writes the other's echo back.

use std::collections::BTreeMap;

use bevy::prelude::*;
use shards_settings::ControlSettings;

use super::map::{ActionBinding, BindMode, Binding, InputMap};

/// The map both sides last agreed on.
#[derive(Resource, Default)]
pub struct ControlSync {
    /// `None` until the first resolve.
    pub(super) agreed: Option<InputMap>,
}

impl ControlSync {
    /// Says the map and the settings match now (after a tool wrote both).
    pub(super) fn agree(&mut self, map: &InputMap) {
        self.agreed = Some(map.clone());
    }
}

fn mode_name(mode: BindMode) -> &'static str {
    match mode {
        BindMode::Press => "press",
        BindMode::Hold => "hold",
        BindMode::Toggle => "toggle",
    }
}

fn parse_mode(text: &str) -> Option<BindMode> {
    match text.trim().to_ascii_lowercase().as_str() {
        "press" => Some(BindMode::Press),
        "hold" => Some(BindMode::Hold),
        "toggle" => Some(BindMode::Toggle),
        _ => None,
    }
}

/// `context.action` split at the first dot (actions like `technique.1` have dots, contexts do not).
fn split(key: &str) -> Option<(&str, &str)> {
    key.split_once('.')
}

/// Writes what `map` changes from `defaults` into `settings`, keeping entries for actions the
/// defaults do not know (a file written by another version). Returns whether anything changed.
pub fn encode(map: &InputMap, defaults: &InputMap, settings: &mut ControlSettings) -> bool {
    let known = |key: &str| split(key).is_some_and(|(context, action)| defaults.action(context, action).is_some());
    let mut bindings: BTreeMap<String, Vec<String>> = settings.bindings.iter().filter(|(k, _)| !known(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut modes: BTreeMap<String, String> = settings.hold_or_toggle.iter().filter(|(k, _)| !known(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    for context in &map.contexts {
        for action in &context.actions {
            let Some(base) = defaults.action(&context.name, &action.action) else { continue };
            let key = format!("{}.{}", context.name, action.action);
            if action.bindings != base.bindings {
                bindings.insert(key.clone(), action.bindings.iter().map(Binding::text).collect());
            }
            if action.mode != base.mode {
                modes.insert(key, mode_name(action.mode).to_owned());
            }
        }
    }
    let changed = settings.bindings != bindings || settings.hold_or_toggle != modes;
    if changed {
        settings.bindings = bindings;
        settings.hold_or_toggle = modes;
    }
    changed
}

/// The defaults with the player's changes from `settings` applied, and what could not be
/// applied (an action the defaults no longer have, a binding that does not parse), each as a
/// line that names the setting.
pub fn decode(defaults: &InputMap, settings: &ControlSettings) -> (InputMap, Vec<String>) {
    let mut map = defaults.clone();
    let mut problems = Vec::new();
    for (key, texts) in &settings.bindings {
        let applied = texts
            .iter()
            .map(|t| Binding::parse(t))
            .collect::<Result<Vec<Binding>, String>>()
            .and_then(|bindings| with_action(&mut map, key, |action| action.bindings = bindings));
        if let Err(why) = applied {
            problems.push(format!("controls.bindings.{key}: {why}"));
        }
    }
    for (key, text) in &settings.hold_or_toggle {
        let applied = parse_mode(text)
            .ok_or_else(|| format!("`{text}` is not press, hold or toggle"))
            .and_then(|mode| with_action(&mut map, key, |action| action.mode = mode));
        if let Err(why) = applied {
            problems.push(format!("controls.hold_or_toggle.{key}: {why}"));
        }
    }
    (map, problems)
}

/// Runs `f` on the action `key` (`context.action`) names, or says why there is none.
fn with_action<R>(map: &mut InputMap, key: &str, f: impl FnOnce(&mut ActionBinding) -> R) -> Result<R, String> {
    let (context, action) = split(key).ok_or_else(|| format!("`{key}` is not `context.action`"))?;
    let ctx = map.contexts.iter_mut().find(|c| c.name == context).ok_or_else(|| format!("no input context `{context}`"))?;
    let found = ctx.actions.iter_mut().find(|a| a.action == action).ok_or_else(|| format!("no action `{action}` in context `{context}`"))?;
    Ok(f(found))
}

/// What `settings_set` refuses: a binding text that does not read, a mode that does not exist.
pub fn check(settings: &ControlSettings) -> Result<(), String> {
    for (key, texts) in &settings.bindings {
        for text in texts {
            Binding::parse(text).map_err(|why| format!("controls.bindings.{key}: {why}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_changed_goes_into_the_settings_and_comes_back() {
        let defaults = InputMap::standard();
        let mut mine = defaults.clone();
        mine.rebind("gameplay", "jump", 0, Binding::Key(KeyCode::KeyJ)).unwrap();
        mine.rebind("gameplay", "technique.1", 0, Binding::Key(KeyCode::KeyZ)).unwrap();
        mine.set_mode("gameplay", "sprint", BindMode::Toggle).unwrap();
        mine.unbind("gameplay", "parry", 0).unwrap();
        mine.unbind("gameplay", "parry", 0).unwrap();

        let mut settings = ControlSettings::default();
        assert!(encode(&mine, &defaults, &mut settings));
        assert_eq!(settings.bindings.keys().map(String::as_str).collect::<Vec<_>>(), ["gameplay.jump", "gameplay.parry", "gameplay.technique.1"]);
        assert_eq!(settings.bindings["gameplay.jump"][0], "Key(KeyJ)");
        assert_eq!(settings.bindings["gameplay.parry"], Vec::<String>::new(), "an unbound action is an empty list");
        assert_eq!(settings.hold_or_toggle["gameplay.sprint"], "toggle");
        assert!(!encode(&mine, &defaults, &mut settings), "nothing new: nothing changed");

        let (back, problems) = decode(&defaults, &settings);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(back, mine);

        // Putting everything back removes the entries.
        assert!(encode(&defaults, &defaults, &mut settings));
        assert!(settings.bindings.is_empty() && settings.hold_or_toggle.is_empty());
    }

    #[test]
    fn what_does_not_apply_is_named_and_kept() {
        let defaults = InputMap::standard();
        let mut settings = ControlSettings::default();
        settings.bindings.insert("gameplay.fly".into(), vec!["Key(KeyF)".into()]);
        settings.bindings.insert("gameplay.jump".into(), vec!["Space".into()]);
        settings.hold_or_toggle.insert("gameplay.block".into(), "tap".into());
        let (map, problems) = decode(&defaults, &settings);
        assert_eq!(map, defaults, "nothing of it applies");
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("controls.bindings.gameplay.fly") && p.contains("no action `fly`")));
        assert!(problems.iter().any(|p| p.contains("gameplay.jump") && p.contains("not a binding")));
        assert!(problems.iter().any(|p| p.contains("gameplay.block") && p.contains("press, hold or toggle")));
        assert!(check(&settings).unwrap_err().contains("gameplay.jump"));

        // An entry for an action this version does not have stays in the file when something else is saved.
        let mut mine = defaults.clone();
        mine.rebind("gameplay", "dodge", 0, Binding::Key(KeyCode::KeyV)).unwrap();
        encode(&mine, &defaults, &mut settings);
        assert!(settings.bindings.contains_key("gameplay.fly") && settings.bindings.contains_key("gameplay.dodge"));
    }
}
