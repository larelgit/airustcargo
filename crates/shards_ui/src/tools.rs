//! Tools: an agent goes through menus the way a player does, by the UI tree alone.
//!
//! `ui_read` reads it; `ui_navigate` is a gamepad (move focus, change, confirm, back),
//! `ui_click` the mouse on one widget, `ui_set` typing a value into it; `ui_open` and `ui_close`
//! are the keys that open and close screens (the pause key opens `pause`). All are player tools
//! and run on this machine: a client's menus are its own. The ones that change something are
//! actions, so replays record them and press the same buttons again.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::model::{self, find, Nav, UiStack};
use crate::text::ui_tree;

pub(crate) fn register(world: &mut World) {
    register_tool(world, "ui_read", Scope::Player, ToolKind::Query, ui_read);
    register_tool(world, "ui_open", Scope::Player, ToolKind::Action, ui_open);
    register_tool(world, "ui_close", Scope::Player, ToolKind::Action, ui_close);
    register_tool(world, "ui_navigate", Scope::Player, ToolKind::Action, ui_navigate);
    register_tool(world, "ui_click", Scope::Player, ToolKind::Action, ui_click);
    register_tool(world, "ui_set", Scope::Player, ToolKind::Action, ui_set);
}

/// The open screens as a tree: every widget's id (`screen/widget`), kind, label and value, the
/// focused one marked, the screen that takes input first.
#[derive(Deserialize, JsonSchema, Default)]
pub struct UiReadArgs {
    /// Only this screen.
    pub screen: Option<String>,
}

fn ui_read(In(call): In<Call<UiReadArgs>>, world: &World) -> ToolResult {
    ui_tree(world, call.args.screen.as_deref())
}

/// Open a screen on top, as the key that opens it does (`pause`, an inventory). A screen that
/// is open stays as it is.
#[derive(Deserialize, JsonSchema)]
pub struct UiOpenArgs {
    /// The screen, as content names it.
    pub screen: String,
}

fn ui_open(In(call): In<Call<UiOpenArgs>>, world: &mut World) -> ToolResult {
    model::open(world, &call.args.screen)
}

/// Close a screen: the one that takes input, or the one named. One that must be answered
/// (`back` does not close it) closes only from its buttons.
#[derive(Deserialize, JsonSchema, Default)]
pub struct UiCloseArgs {
    /// The screen; default: the one that takes input.
    pub screen: Option<String>,
}

fn ui_close(In(call): In<Call<UiCloseArgs>>, world: &mut World) -> ToolResult {
    let Some(name) = call.args.screen else { return model::back(world) };
    let root = world.resource::<UiStack>().get(&name).ok_or_else(|| {
        let open: Vec<&str> = world.resource::<UiStack>().screens().map(|(n, _)| n).collect();
        format!("screen `{name}` is not open; open: {}", if open.is_empty() { "none".to_owned() } else { open.join(", ") })
    })?;
    let screen = world.get::<model::UiScreen>(root).cloned().ok_or("not a screen")?;
    if screen.def.layer != crate::def::Layer::Hud && !screen.def.closable {
        return Err(format!("{name} cannot be closed so: answer it with one of its buttons (`ui_read screen={name}`)"));
    }
    Ok(model::close(world, root))
}

/// Move on the screen that takes input, as a gamepad does: `up`/`down`/`next`/`prev` move the
/// focus, `left`/`right` change a slider, choice, toggle or tabs (in a row or grid they move),
/// `confirm` presses the focused widget, `back` closes the screen (or stops a binding that waits
/// for an input, and the screen stays).
#[derive(Deserialize, JsonSchema)]
pub struct UiNavigateArgs {
    /// `up`, `down`, `left`, `right`, `next`, `prev`, `confirm` or `back`.
    pub dir: String,
}

fn ui_navigate(In(call): In<Call<UiNavigateArgs>>, world: &mut World) -> ToolResult {
    let nav = Nav::parse(&call.args.dir)
        .ok_or_else(|| format!("no direction `{}`: up, down, left, right, next, prev, confirm or back", call.args.dir))?;
    if let Some(name) = model::top_screen(world).and_then(|top| model::screen_name(world, top)) {
        model::mark_pressed(world, name);
    }
    model::navigate(world, nav, &call.caller.agent)
}

/// Press a widget on the screen that takes input, as a click does: a button does its action, a
/// toggle flips, a choice or tabs go to the next, a binding waits for the player's next key or
/// button. Focus moves to it.
#[derive(Deserialize, JsonSchema)]
pub struct UiClickArgs {
    /// The widget, `screen/widget` as `ui_read` lists it (`pause/resume`).
    pub id: String,
}

fn ui_click(In(call): In<Call<UiClickArgs>>, world: &mut World) -> ToolResult {
    let e = widget(world, &call.args.id)?;
    model::mark_pressed(world, call.args.id.split('/').next().unwrap_or_default());
    model::activate(world, e, &call.caller.agent)
}

/// Set a slider (a number), toggle (`on`/`off`), choice (one of its options), tabs (a page's
/// id, title or number from 1) or binding (an input: `Key(KeyK)`, `Mouse(Right)`,
/// `GamepadButton(North)`; `default` gives the action its default inputs back) on the screen that
/// takes input. One bound to a player setting changes that setting, which is saved; a binding
/// changes the `controls` settings, which refuse an input another action of the context reads.
#[derive(Deserialize, JsonSchema)]
pub struct UiSetArgs {
    /// The widget, `screen/widget` (`settings/text_size`).
    pub id: String,
    /// The new value as text (a number or `true` is as good as its text).
    #[serde(deserialize_with = "text_or_scalar")]
    #[schemars(with = "String")]
    pub value: String,
}

/// `value=1.5` reaches the tool as a number, `value=true` as a boolean: both are the text they
/// were typed as.
fn text_or_scalar<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => text,
        other => other.to_string(),
    })
}

fn ui_set(In(call): In<Call<UiSetArgs>>, world: &mut World) -> ToolResult {
    let e = widget(world, &call.args.id)?;
    model::mark_pressed(world, call.args.id.split('/').next().unwrap_or_default());
    model::set_value(world, e, &call.args.value, &call.caller.agent)
}

/// The widget at `path`, or what is there instead.
fn widget(world: &World, path: &str) -> Result<Entity, String> {
    if let Some(e) = find(world, path) {
        return Ok(e);
    }
    let screen = path.split('/').next().unwrap_or_default();
    let stack = world.resource::<UiStack>();
    let Some(root) = stack.get(screen) else {
        let open: Vec<&str> = stack.screens().map(|(n, _)| n).collect();
        return Err(format!(
            "no widget `{path}`: screen `{screen}` is not open (open: {}); ids are `screen/widget` as `ui_read` lists them",
            if open.is_empty() { "none".to_owned() } else { open.join(", ") }
        ));
    };
    let ids: Vec<String> = model::walk(world, root)
        .into_iter()
        .skip(1)
        .filter_map(|(e, _)| world.get::<model::UiWidget>(e))
        .filter(|w| w.kind.focusable())
        .map(|w| w.path.clone())
        .collect();
    Err(format!("no widget `{path}` on {screen}; it has {}", ids.join(", ")))
}
