//! Where widgets get their values (sources) and what game buttons do (actions).
//!
//! A source is a pure function of the world: `you.health` reads the viewer's `Health`, and a
//! widget bound to it (`Bar(bind: "you.health")`) shows it. What follows a `:` is the source's
//! argument (`you.cooldown:fireball`). Agents read values live, from the source, so `observe`
//! right after a `step` is up to date; the window draws what [`refresh_values`] last stored.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::AiPlayer;
use bevy::prelude::*;
use shards_core::LocalPlayer;
use shards_settings::SettingsRegistry;

use crate::model::{UiValue, UiWidget, WidgetKind};

/// The body a HUD is about: the one this machine's controls drive (`LocalPlayer`), else the one
/// agents play (`AiPlayer`). A game's own sources read it too (`trophies` of the hero).
pub fn viewer(world: &World) -> Option<Entity> {
    let mut local = world.try_query_filtered::<Entity, With<LocalPlayer>>()?;
    local.iter(world).next().or_else(|| {
        let mut played = world.try_query_filtered::<Entity, With<AiPlayer>>()?;
        played.iter(world).next()
    })
}

/// A source: the value for `arg` (the text after `:` in a binding, empty without one), `None`
/// when there is nothing to show (no viewer).
pub type UiSourceFn = fn(&World, &str) -> Option<UiValue>;

/// The sources widgets bind to, by name.
#[derive(Resource, Default, Clone)]
pub struct UiSources(BTreeMap<String, UiSourceFn>);

impl UiSources {
    pub fn insert(&mut self, name: &str, source: UiSourceFn) {
        self.0.insert(name.to_owned(), source);
    }

    pub fn get(&self, name: &str) -> Option<UiSourceFn> {
        self.0.get(name).copied()
    }

    pub fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

/// A button's game action was pressed.
#[derive(Clone, Debug, PartialEq)]
pub struct UiPress {
    /// The button: `death/respawn`.
    pub widget: String,
    pub screen: String,
    /// The action's name: `respawn`.
    pub action: String,
    /// The agent that pressed it (`input` for the local player's controls): tools the action
    /// calls run as it.
    pub agent: String,
}

/// What a game action does; the text is the answer of `ui_click`, an error is refused.
pub type UiActionFn = fn(&mut World, &UiPress) -> Result<String, String>;

/// The game's button actions, by name.
#[derive(Resource, Default, Clone)]
pub struct UiActions(BTreeMap<String, UiActionFn>);

impl UiActions {
    pub fn insert(&mut self, name: &str, action: UiActionFn) {
        self.0.insert(name.to_owned(), action);
    }

    pub fn get(&self, name: &str) -> Option<UiActionFn> {
        self.0.get(name).copied()
    }

    pub fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

/// What source `bind` (`name` or `name:arg`) says now. `Err` names the sources there are.
pub fn read_source(world: &World, bind: &str) -> Result<Option<UiValue>, String> {
    let (name, arg) = bind.split_once(':').unwrap_or((bind, ""));
    let sources = world.get_resource::<UiSources>();
    match sources.and_then(|s| s.get(name)) {
        Some(source) => Ok(source(world, arg)),
        None => {
            let names = sources.map(UiSources::names).unwrap_or_default();
            Err(format!("no UI source `{name}`; sources: {}", if names.is_empty() { "none".to_owned() } else { names.join(", ") }))
        }
    }
}

/// The value `widget` has now: its source's, its player setting's, or its own.
pub fn live_value(world: &World, widget: &UiWidget) -> UiValue {
    if let Some(bind) = &widget.bind {
        return match read_source(world, bind) {
            Ok(Some(value)) => value,
            // Nothing to show (no viewer): the widget keeps what it had.
            Ok(None) | Err(_) => widget.value.clone(),
        };
    }
    if let Some(key) = &widget.setting
        && let Some(text) = world.get_resource::<SettingsRegistry>().and_then(|r| r.value(world, key))
    {
        return match &widget.value {
            UiValue::Number(_) => text.parse().map(UiValue::Number).unwrap_or_else(|_| widget.value.clone()),
            UiValue::Flag(_) => UiValue::Flag(text == "true"),
            // A setting that is none of the options (a language left to the game) shows `value`.
            UiValue::Choice(_) => match (&widget.kind, text.trim_matches('"')) {
                (WidgetKind::Choice { options, .. }, now) if options.iter().any(|o| o == now) => UiValue::Choice(now.to_owned()),
                _ => widget.value.clone(),
            },
            other => other.clone(),
        };
    }
    widget.value.clone()
}

/// `Update`: stores every bound widget's live value for drawing, where it changed. A binding to
/// a source nobody gives is warned about once.
pub(crate) fn refresh_values(world: &mut World, mut warned: Local<BTreeSet<String>>) {
    let mut query = world.query::<(Entity, &UiWidget)>();
    let mut changed = Vec::new();
    for (e, widget) in query.iter(world) {
        if widget.bind.is_none() && widget.setting.is_none() {
            continue;
        }
        if let Some(bind) = &widget.bind
            && let Err(e) = read_source(world, bind)
        {
            if warned.insert(widget.path.clone()) {
                warn!("ui: {} is bound to `{bind}`: {e}", widget.path);
            }
            continue;
        }
        let now = live_value(world, widget);
        if now != widget.value {
            changed.push((e, now));
        }
    }
    for (e, value) in changed {
        if let Some(mut widget) = world.get_mut::<UiWidget>(e) {
            widget.value = value;
        }
    }
}
