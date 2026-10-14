//! `input_bindings` and `rebind`: an agent reads and changes the controls like a player
//! does in the settings screen.

use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

use shards_settings::{AccessibilitySettings, ControlSettings, SettingsStorage};

use super::controls::{self, ControlSync};
use super::map::{BindMode, Binding, InputMap, RebindError};
use super::{InputContexts, InputDefaults};

pub(super) fn register(world: &mut World) {
    register_tool(world, "input_bindings", Scope::Player, ToolKind::Query, input_bindings);
    // A control tool: the controls do not change the game, so replays do not record it.
    register_tool(world, "rebind", Scope::Dev, ToolKind::Control, rebind);
}

/// Which keys, buttons and sticks do what. Lists the active context's actions with their
/// bindings by slot (slot 0 first), or the context you name.
#[derive(Deserialize, JsonSchema, Default)]
pub struct InputBindingsArgs {
    /// A context such as `gameplay` or `menu`. Default: the active one.
    pub context: Option<String>,
}

fn input_bindings(
    In(call): In<Call<InputBindingsArgs>>,
    map: Res<InputMap>,
    contexts: Res<InputContexts>,
    accessibility: Option<Res<AccessibilitySettings>>,
) -> ToolResult {
    let name = call.args.context.unwrap_or_else(|| contexts.top().to_owned());
    let Some(context) = map.context(&name) else {
        let names: Vec<&str> = map.contexts.iter().map(|c| c.name.as_str()).collect();
        return Err(format!("no input context `{name}`{}; contexts: {}", InputMap::suggest(&name, names.iter().copied()), names.join(", ")));
    };
    let mut out = format!("{name}{}", if contexts.top() == name { " (active)" } else { "" });
    let width = context.actions.iter().map(|a| a.action.len()).max().unwrap_or(0);
    for action in &context.actions {
        let mode = match action.mode {
            BindMode::Press => "",
            BindMode::Hold => "[hold] ",
            BindMode::Toggle => "[toggle] ",
        };
        let bindings: Vec<String> = action.bindings.iter().map(Binding::text).collect();
        out.push_str(&format!("\n  {:<width$}  {mode}{}", action.action, bindings.join(" · ")));
    }
    if accessibility.is_some_and(|a| a.hold_to_toggle) {
        out.push_str("\naccessibility.hold_to_toggle is on: [hold] actions work as toggles (a press holds, the next lets go)");
    }
    let others: Vec<&str> = map.contexts.iter().map(|c| c.name.as_str()).filter(|n| *n != name).collect();
    if !others.is_empty() {
        out.push_str(&format!("\nother contexts: {} (input_bindings context=NAME)", others.join(", ")));
    }
    Ok(out)
}

/// Change a control the way the settings screen does: put `binding` in a slot of an
/// action, change how it reacts (`mode`), or remove a slot (`unbind`). A binding another
/// action of the context already uses is refused unless `force` takes it over.
#[derive(Deserialize, JsonSchema)]
pub struct RebindArgs {
    /// Context; default the active one.
    pub context: Option<String>,
    /// Action name, e.g. `jump`, `sprint`, `light`.
    pub action: String,
    /// The new input as text: `Key(KeyE)`, `Mouse(Right)`, `GamepadButton(South)`,
    /// `Keys2D(up: KeyW, down: KeyS, left: KeyA, right: KeyD)`, `Stick(stick: Left)`.
    pub binding: Option<String>,
    /// Which binding to replace: 0 is the primary; one past the last adds an alternative.
    #[serde(default)]
    pub slot: usize,
    /// `press`, `hold` or `toggle`.
    pub mode: Option<String>,
    /// Remove the binding in `slot` instead.
    #[serde(default)]
    pub unbind: bool,
    /// Take the input from the action that uses it.
    #[serde(default)]
    pub force: bool,
}

#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn rebind(
    In(call): In<Call<RebindArgs>>,
    mut commands: Commands,
    contexts: Res<InputContexts>,
    defaults: Res<InputDefaults>,
    storage: Option<Res<SettingsStorage>>,
    mut controls: ResMut<ControlSettings>,
    mut sync: ResMut<ControlSync>,
    mut map: ResMut<InputMap>,
) -> ToolResult {
    let RebindArgs { context, action, binding, slot, mode, unbind, force } = call.args;
    let context = context.unwrap_or_else(|| contexts.top().to_owned());
    let names: Vec<String> = map.action_names(&context).iter().map(|n| (*n).to_owned()).collect();
    if map.context(&context).is_some() && !names.contains(&action) {
        let hint = InputMap::suggest(&action, names.iter().map(String::as_str));
        return Err(format!("no action `{action}` in context `{context}`{hint}; actions: {}", names.join(", ")));
    }
    if binding.is_none() && mode.is_none() && !unbind {
        return Err("nothing to change: give a `binding`, a `mode`, or `unbind`".into());
    }
    let explain = |e: RebindError| match e {
        RebindError::NoContext(name) => {
            let all: Vec<&str> = map.contexts.iter().map(|c| c.name.as_str()).collect();
            format!("no input context `{name}`{}; contexts: {}", InputMap::suggest(&name, all.iter().copied()), all.join(", "))
        }
        other => other.to_string(),
    };
    let mut done: Vec<String> = Vec::new();
    // Work on a copy: a refusal halfway leaves the controls as they were.
    let mut next = map.clone();
    if unbind {
        let gone = next.unbind(&context, &action, slot).map_err(explain)?;
        done.push(format!("removed {gone} from slot {slot}"));
    }
    if let Some(text) = binding {
        let binding = Binding::parse(&text)?;
        if force {
            let lost = next.rebind_taking(&context, &action, slot, binding.clone()).map_err(explain)?;
            done.push(format!("slot {slot} = {binding}"));
            if !lost.is_empty() {
                done.push(format!("taken from {}", lost.join(", ")));
            }
        } else {
            next.rebind(&context, &action, slot, binding.clone()).map_err(explain)?;
            done.push(format!("slot {slot} = {binding}"));
        }
    }
    if let Some(text) = mode {
        let mode = match text.to_ascii_lowercase().as_str() {
            "press" => BindMode::Press,
            "hold" => BindMode::Hold,
            "toggle" => BindMode::Toggle,
            other => return Err(format!("unknown mode `{other}`; use press, hold or toggle")),
        };
        next.set_mode(&context, &action, mode).map_err(explain)?;
        done.push(format!("mode {mode:?}").to_lowercase());
    }
    let mut message = format!("ok: {context}/{action}: {}", done.join(", "));
    // The player's controls are settings: what differs from the defaults goes into
    // `controls.bindings` / `controls.hold_or_toggle`, written a second later.
    let mut changed = controls.clone();
    if controls::encode(&next, &defaults.0, &mut changed) {
        *controls = changed;
        shards_settings::save_soon(&mut commands);
    }
    if let Some(SettingsStorage { file: Some(file), .. }) = storage.as_deref() {
        // Settings keep differences on the default controls only; an action a game added to the map by
        // itself is the game's to set up each time.
        if defaults.0.action(&context, &action).is_some() {
            message.push_str(&format!("; saved to {} in a second", file.display()));
        } else {
            message.push_str(&format!("; not saved: `{context}.{action}` is not one of the default controls, so it lasts until the game ends"));
        }
    }
    sync.agree(&next, &defaults.0, &controls);
    *map = next;
    Ok(message)
}
