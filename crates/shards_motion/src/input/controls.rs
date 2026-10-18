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

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use shards_settings::ControlSettings;

use super::map::{ActionBinding, BindMode, Binding, InputContext, InputMap};
use super::InputDefaults;

/// `ControlSettings.bindings` and `.hold_or_toggle` as they were.
type Stored = (BTreeMap<String, Vec<String>>, BTreeMap<String, String>);

/// The map both sides last agreed on, and what it was built from.
#[derive(Resource, Default)]
pub struct ControlSync {
    /// `None` until the first resolve.
    pub(super) agreed: Option<InputMap>,
    /// The defaults the agreed map was built on: what is in the map beyond them (a context or an
    /// action a game's system added) is the game's, and a rebuild keeps it.
    base: Option<InputMap>,
    /// The bindings and modes the agreed map was built from: a settings change that leaves them
    /// as they are (the look speed) is none of the map's business.
    stored: Option<Stored>,
    /// Warnings already journaled, so a rebuild does not say the same thing again.
    warned: BTreeSet<String>,
}

impl ControlSync {
    /// Says the map and the settings match now (the first resolve, a rebuild, a tool that wrote both).
    pub(super) fn agree(&mut self, map: &InputMap, defaults: &InputMap, settings: &ControlSettings) {
        self.agreed = Some(map.clone());
        self.base = Some(defaults.clone());
        self.stored = Some((settings.bindings.clone(), settings.hold_or_toggle.clone()));
    }

    /// Whether the bindings or modes in `settings` are not what the agreed map was built from.
    pub(super) fn settings_moved(&self, settings: &ControlSettings) -> bool {
        self.stored.as_ref().is_none_or(|(bindings, modes)| *bindings != settings.bindings || *modes != settings.hold_or_toggle)
    }

    /// What the map holds beyond the defaults it was built on.
    pub(super) fn extras(&self, map: &InputMap, defaults: &InputMap) -> InputMap {
        extras(map, self.base.as_ref().unwrap_or(defaults))
    }

    /// The lines of `lines` not said before.
    pub(super) fn unsaid(&mut self, lines: Vec<String>) -> Vec<String> {
        lines.into_iter().filter(|line| self.warned.insert(line.clone())).collect()
    }
}

/// The contexts and actions of `map` that `base` does not have.
pub fn extras(map: &InputMap, base: &InputMap) -> InputMap {
    let contexts = map
        .contexts
        .iter()
        .filter_map(|ctx| match base.context(&ctx.name) {
            None => Some(ctx.clone()),
            Some(known) => {
                let actions: Vec<ActionBinding> = ctx.actions.iter().filter(|a| known.action(&a.action).is_none()).cloned().collect();
                (!actions.is_empty()).then(|| InputContext { name: ctx.name.clone(), actions, drives_intent: ctx.drives_intent })
            }
        })
        .collect();
    InputMap { contexts }
}

/// Inputs that two actions of one context both read in `map` and did not in `defaults`: what
/// `rebind` refuses without `force`, found in a map a file or a tool made. One line each.
pub fn new_clashes(map: &InputMap, defaults: &InputMap) -> Vec<String> {
    let mut lines = Vec::new();
    for ctx in &map.contexts {
        let shared = |a: &ActionBinding, b: &ActionBinding| a.bindings.iter().find(|x| b.bindings.iter().any(|y| x.overlaps(y))).cloned();
        for (i, a) in ctx.actions.iter().enumerate() {
            for b in &ctx.actions[i + 1..] {
                let Some(input) = shared(a, b) else { continue };
                let before = defaults.context(&ctx.name).and_then(|c| Some((c.action(&a.action)?, c.action(&b.action)?)));
                if before.is_some_and(|(x, y)| shared(x, y).is_some()) {
                    continue;
                }
                lines.push(format!("settings: in `{}`, `{}` and `{}` both read {input}", ctx.name, a.action, b.action));
            }
        }
    }
    lines
}

fn mode_name(mode: BindMode) -> &'static str {
    match mode {
        BindMode::Press => "press",
        BindMode::Hold => "hold",
        BindMode::Toggle => "toggle",
        BindMode::PressHold => "press_hold",
    }
}

fn parse_mode(text: &str) -> Option<BindMode> {
    match text.trim().to_ascii_lowercase().as_str() {
        "press" => Some(BindMode::Press),
        "hold" => Some(BindMode::Hold),
        "toggle" => Some(BindMode::Toggle),
        "press_hold" => Some(BindMode::PressHold),
        _ => None,
    }
}

/// `context.action` split at the first dot (actions like `technique.1` have dots, contexts do not:
/// content refuses a context name with one).
fn split(key: &str) -> Option<(&str, &str)> {
    key.split_once('.')
}

/// Writes what `map` changes from `defaults` into `settings`. Entries that do not apply stay: for
/// actions the defaults do not know (a file written by another version), and the ones that do not
/// read (a binding with a typo): the next save must not erase what a player wrote and was told
/// about. Returns whether anything changed.
pub fn encode(map: &InputMap, defaults: &InputMap, settings: &mut ControlSettings) -> bool {
    let known = |key: &str| split(key).is_some_and(|(context, action)| defaults.action(context, action).is_some());
    let keep_bindings = |k: &str, v: &[String]| !known(k) || v.iter().any(|text| Binding::parse(text).is_err());
    let keep_mode = |k: &str, v: &str| !known(k) || parse_mode(v).is_none();
    let mut bindings: BTreeMap<String, Vec<String>> =
        settings.bindings.iter().filter(|(k, v)| keep_bindings(k, v)).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut modes: BTreeMap<String, String> =
        settings.hold_or_toggle.iter().filter(|(k, v)| keep_mode(k, v)).map(|(k, v)| (k.clone(), v.clone())).collect();
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
            .ok_or_else(|| format!("`{text}` is not press, hold, toggle or press_hold"))
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

/// What `settings_set` refuses: a binding text that does not read.
pub fn check(settings: &ControlSettings) -> Result<(), String> {
    for (key, texts) in &settings.bindings {
        for text in texts {
            Binding::parse(text).map_err(|why| format!("controls.bindings.{key}: {why}"))?;
        }
    }
    Ok(())
}

/// What `settings_set` refuses beyond that, as `rebind` does: an entry it adds or changes must name
/// a context and an action that exist (with a hint at the nearest name), and must not take an
/// input another action of its context already reads; an entry it removes (a reset) must not give
/// its action back an input another action has taken meanwhile. Only what the call changes is
/// judged, so entries a file written for another version left behind do not stop anything.
///
/// It judges the map the settings would make as the game builds it: the defaults, the player's
/// entries, and what the game added to the map itself (those are not settings, so an entry for
/// one is refused, but an input one of them reads is taken). Before the first resolve, when the
/// defaults are not known, nothing is judged.
pub fn check_known(current: &ControlSettings, next: &ControlSettings, world: &World) -> Result<(), String> {
    let (Some(sync), Some(InputDefaults(defaults))) = (world.get_resource::<ControlSync>(), world.get_resource::<InputDefaults>()) else {
        return Ok(());
    };
    if sync.agreed.is_none() {
        return Ok(());
    }
    let extras = world.get_resource::<InputMap>().map(|live| sync.extras(live, defaults)).unwrap_or_default();
    let with_extras = |settings: &ControlSettings| {
        let (mut map, _) = decode(defaults, settings);
        map.apply(&extras);
        map
    };
    // What the new settings make; what the old ones made (a clash it already had is not this call's);
    // and what the game ships (a clash in that is the game's).
    let (map, before) = (with_extras(next), with_extras(current));
    let shipped = with_extras(&ControlSettings::default());

    let bindings: Vec<(&String, &Vec<String>)> = next.bindings.iter().filter(|(k, v)| current.bindings.get(*k) != Some(*v)).collect();
    let modes: Vec<&String> = next.hold_or_toggle.iter().filter(|(k, v)| current.hold_or_toggle.get(*k) != Some(*v)).map(|(k, _)| k).collect();
    for (field, key) in bindings.iter().map(|(k, _)| ("bindings", *k)).chain(modes.iter().map(|k| ("hold_or_toggle", *k))) {
        let label = format!("controls.{field}.{key}");
        let (context, action) = split(key).ok_or_else(|| format!("{label}: `{key}` is not `context.action` (for example `gameplay.jump`)"))?;
        let Some(ctx) = map.context(context) else {
            let all: Vec<&str> = map.contexts.iter().map(|c| c.name.as_str()).collect();
            return Err(format!("{label}: no input context `{context}`{}; contexts: {}", InputMap::suggest(context, all.iter().copied()), all.join(", ")));
        };
        if ctx.action(action).is_none() {
            let names = map.action_names(context);
            return Err(format!(
                "{label}: no action `{action}` in context `{context}`{}; actions: {}",
                InputMap::suggest(action, names.iter().copied()),
                names.join(", ")
            ));
        }
        if defaults.action(context, action).is_none() {
            return Err(format!(
                "{label}: `{context}.{action}` is a control the game adds itself, not one of the default controls, and settings keep only those; \
                 a dev session can change it until the game ends with the rebind tool"
            ));
        }
    }
    // An input some other action reads is a clash.
    for (key, texts) in bindings {
        let Some((context, action)) = split(key) else { continue };
        for binding in texts.iter().filter_map(|t| Binding::parse(t).ok()) {
            if let Some(other) = map.used_by(context, &binding, action) {
                return Err(format!(
                    "controls.bindings.{key}: {binding} is already bound to `{other}` in `{context}`; {}; \
                     a dev session can take it over with `rebind context={context} action={action} binding='{}' force=true`",
                    free_it_first(defaults, context, other),
                    binding.text()
                ));
            }
        }
    }
    // An entry that goes puts its action back on the default inputs, which another action may have taken since.
    for key in current.bindings.keys().filter(|k| !next.bindings.contains_key(*k)) {
        let Some((context, action)) = split(key) else { continue };
        let Some(restored) = map.action(context, action) else { continue };
        for binding in &restored.bindings {
            let Some(other) = map.used_by(context, binding, action) else { continue };
            // Two actions that already shared this input in `m` are a clash the call did not make.
            let shared = |m: &InputMap| m.action(context, action).is_some_and(|a| a.bindings.iter().any(|b| b.overlaps(binding))) && m.used_by(context, binding, action) == Some(other);
            if shared(&before) || shared(&shipped) {
                continue;
            }
            return Err(format!(
                "controls.bindings.{key}: back to the default, `{action}` would read {binding}, which `{other}` already reads in `{context}`; {}",
                free_it_first(defaults, context, other)
            ));
        }
    }
    Ok(())
}

/// How a caller of `settings_set` (a player's agent has no `rebind`) makes an input free. A control
/// the game added itself is not a setting: `settings_set` is refused for it, so it is no way out.
fn free_it_first(defaults: &InputMap, context: &str, other: &str) -> String {
    if defaults.action(context, other).is_none() {
        return format!("`{other}` is a control the game adds itself, which settings cannot move, so pick another input");
    }
    format!("free it first: settings_set group=controls key=bindings.{context}.{other} value=<another input>")
}

/// The map in play as `controls` would make it. `settings_set` replaces the settings and the map
/// follows in the next `PreUpdate`; a tool called before that (a scenario, a test, two requests in
/// one frame) must not start from a map the settings have moved on from, or `rebind` would write
/// that map back over the change. A map edited directly wins, as it does when it is rebuilt.
pub(super) fn current_map<'a>(map: &'a InputMap, defaults: &InputMap, controls: &ControlSettings, sync: &ControlSync) -> Cow<'a, InputMap> {
    if sync.agreed.as_ref() != Some(map) || !sync.settings_moved(controls) {
        return Cow::Borrowed(map);
    }
    let (mut desired, _) = decode(defaults, controls);
    desired.apply(&sync.extras(map, defaults));
    Cow::Owned(desired)
}

/// The map in play as the `controls` settings make it now, even right after `settings_set` changed
/// them and before the next frame builds [`InputMap`] again (see [`current_map`]): what a menu that
/// shows the controls reads. `None` without the input module.
pub fn map_now(world: &World) -> Option<Cow<'_, InputMap>> {
    let map = world.get_resource::<InputMap>()?;
    let (Some(InputDefaults(defaults)), Some(controls), Some(sync)) =
        (world.get_resource::<InputDefaults>(), world.get_resource::<ControlSettings>(), world.get_resource::<ControlSync>())
    else {
        return Some(Cow::Borrowed(map));
    };
    Some(current_map(map, defaults, controls, sync))
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
        assert!(problems.iter().any(|p| p.contains("gameplay.block") && p.contains("press, hold, toggle or press_hold")));
        assert!(check(&settings).unwrap_err().contains("gameplay.jump"));

        // An entry for an action this version does not have stays in the file when something else is saved.
        let mut mine = defaults.clone();
        mine.rebind("gameplay", "dodge", 0, Binding::Key(KeyCode::KeyV)).unwrap();
        encode(&mine, &defaults, &mut settings);
        assert!(settings.bindings.contains_key("gameplay.fly") && settings.bindings.contains_key("gameplay.dodge"));
        assert_eq!(settings.bindings["gameplay.jump"], ["Space"], "a binding that does not parse is kept as the player wrote it");
        assert_eq!(settings.hold_or_toggle["gameplay.block"], "tap");
    }
}
