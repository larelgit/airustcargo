//! What the UI needs of the engine's other modules: the values a HUD shows (`you.health`,
//! `allies`, …) and the player's controls in menus.
//!
//! Sources read the viewer: the body this machine's controls drive (`LocalPlayer`), else the
//! one agents play (`AiPlayer`). The controls: the `pause` key (Escape, gamepad Start) opens the
//! screen `pause` if the game has one; while a menu is open the input map's `menu` context is on
//! top (the character hears nothing), its `navigate`, `confirm` and `back` move through the menu.
//! While a binding widget waits for an input, the menu hears nothing either: the next key, mouse
//! button or gamepad button becomes the binding (`ui_set`), and Escape or Start stops the wait
//! (`ui_navigate back`, which then leaves the screen open). All of it goes through the `ui_*`
//! tools as agent [`INPUT_AGENT`], so replays record them.
//!
//! The source `input.bindings` ([`shards_ui::BINDINGS_SOURCE`]) gives binding widgets the inputs of
//! an action, as the `controls` settings make them now.

use ai_core::AiLandmark;
use bevy::prelude::*;
use serde_json::json;
use shards_action::fighter::{Charges, Cooldowns, Dead, Health, Poise, Stamina};
use shards_content::ContentDb;
use shards_core::PlayerId;
use shards_motion::input::InputDefaults;
use shards_motion::{Binding, ControlState, ControlSystems, InputContexts, InputMap};
use shards_ui::{viewer, Ally, BoundInput, BoundInputs, CapturedClicks, Mark, Nav, ScreenDef, UiAppExt, UiValue, BINDINGS_SOURCE, INPUT_AGENT};

/// The screen the pause key opens.
pub const PAUSE_SCREEN: &str = "pause";

/// A held direction repeats after this long, then this often (seconds).
const REPEAT_AFTER: f64 = 0.35;
const REPEAT_EVERY: f64 = 0.12;

/// Joins `shards_ui` to the rest of the engine.
pub struct EngineUiPlugin;

impl Plugin for EngineUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_ui_source("you.health", |w, _| viewer(w).and_then(|e| w.get::<Health>(e)).map(|h| ratio(h.current, h.max)))
            .add_ui_source("you.stamina", |w, _| viewer(w).and_then(|e| w.get::<Stamina>(e)).map(|s| ratio(s.current, s.max)))
            .add_ui_source("you.poise", |w, _| viewer(w).and_then(|e| w.get::<Poise>(e)).map(|p| ratio(p.current, p.max)))
            .add_ui_source("you.charges", |w, name| {
                let charge = viewer(w).and_then(|e| w.get::<Charges>(e)).and_then(|c| c.get(name).cloned());
                charge.map(|c| ratio(c.current as f32, c.max as f32))
            })
            .add_ui_source("you.cooldown", |w, action| {
                viewer(w).map(|e| UiValue::Cooldown { left: w.get::<Cooldowns>(e).map_or(0.0, |c| c.left(action)) })
            })
            .add_ui_source("you.heading", heading)
            .add_ui_source("allies", allies)
            .add_ui_source(BINDINGS_SOURCE, bindings)
            .init_resource::<MenuControls>()
            .add_systems(PreUpdate, menu_controls.after(ControlSystems::Read));
    }
}

fn ratio(value: f32, max: f32) -> UiValue {
    UiValue::Ratio { value, max }
}

/// One input as a binding widget shows it.
fn bound(binding: &Binding, taken_by: Option<String>) -> BoundInput {
    BoundInput { text: binding.text(), label: binding.label(), gamepad: binding.is_gamepad(), axis: binding.is_axis(), taken_by }
}

/// `input.bindings:<context>.<action>`: the action's inputs as the `controls` settings make them
/// now (right after `settings_set`, before the map follows), and its defaults; why there are none
/// when the map has no such action. `input.bindings:<context>.<action> <input>`: that input, and
/// the other action of the context that reads it (`None`: the text is no input).
fn bindings(world: &World, arg: &str) -> Option<UiValue> {
    let (key, asked) = match arg.trim().split_once(char::is_whitespace) {
        Some((key, input)) => (key, Some(input.trim())),
        None => (arg.trim(), None),
    };
    let map = shards_motion::input::controls::map_now(world)?;
    let Some((context, action)) = key.split_once('.') else {
        return Some(UiValue::Text(format!("`{key}` is not `context.action` (`gameplay.jump`)")));
    };
    let Some(ctx) = map.context(context) else {
        let all: Vec<&str> = map.contexts.iter().map(|c| c.name.as_str()).collect();
        return Some(UiValue::Text(format!("no input context `{context}`{}; contexts: {}", InputMap::suggest(context, all.iter().copied()), all.join(", "))));
    };
    let Some(found) = ctx.action(action) else {
        let names = map.action_names(context);
        return Some(UiValue::Text(format!(
            "no action `{action}` in context `{context}`{}; actions: {}",
            InputMap::suggest(action, names.iter().copied()),
            names.join(", ")
        )));
    };
    if let Some(text) = asked {
        let input = Binding::parse(text).ok()?;
        let holder = map.used_by(context, &input, action).map(str::to_owned);
        return Some(UiValue::Bindings(BoundInputs { inputs: vec![bound(&input, holder)], defaults: Vec::new() }));
    }
    let defaults = world.get_resource::<InputDefaults>().and_then(|d| d.0.action(context, action)).map(|a| a.bindings.iter().map(Binding::text).collect());
    Some(UiValue::Bindings(BoundInputs { inputs: found.bindings.iter().map(|b| bound(b, None)).collect(), defaults: defaults.unwrap_or_default() }))
}

/// Degrees clockwise from north (-Z) of a direction on the ground.
fn bearing(dir: Vec3) -> f32 {
    ops::atan2(dir.x, -dir.z).to_degrees().rem_euclid(360.0)
}

/// Where the viewer faces, and the four nearest landmarks.
fn heading(world: &World, _: &str) -> Option<UiValue> {
    let me = viewer(world)?;
    let at = world.get::<Transform>(me)?;
    let deg = bearing(at.forward().as_vec3());
    let mut landmarks = world.try_query_filtered::<(Entity, &Transform), With<AiLandmark>>()?;
    let mut marks: Vec<Mark> = landmarks
        .iter(world)
        .map(|(e, t)| {
            let to = t.translation - at.translation;
            let label = world.get::<Name>(e).map_or_else(|| ai_core::label(world, e), |n| n.as_str().to_owned());
            Mark { label, bearing: bearing(to), distance: Vec2::new(to.x, to.z).length() }
        })
        .collect();
    marks.sort_by(|a, b| a.distance.total_cmp(&b.distance).then_with(|| a.label.cmp(&b.label)));
    marks.truncate(4);
    Some(UiValue::Heading { deg, marks })
}

/// The other players, by number: name, health, down or not.
fn allies(world: &World, _: &str) -> Option<UiValue> {
    let me = viewer(world);
    let mut players = world.try_query::<(Entity, &PlayerId, &Health, Option<&Name>, Has<Dead>)>()?;
    let mut list: Vec<(u8, Ally)> = players
        .iter(world)
        .filter(|(e, ..)| Some(*e) != me)
        .map(|(_, id, hp, name, dead)| {
            let name = name.map_or_else(|| format!("p{}", u16::from(id.0) + 1), |n| format!("p{} {}", u16::from(id.0) + 1, n.as_str()));
            (id.0, Ally { name, health: hp.current, max: hp.max, down: dead })
        })
        .collect();
    list.sort_by_key(|(n, _)| *n);
    Some(UiValue::Allies(list.into_iter().map(|(_, a)| a).collect()))
}

/// What the controls did in menus: the `menu` context pushed, the direction held.
#[derive(Resource, Default)]
struct MenuControls {
    pushed: bool,
    held: Option<Nav>,
    next_repeat: f64,
    /// A binding widget took an input a moment ago: the menu moves again once nothing steers it,
    /// so an arrow key or W that became a binding does not also move the focus.
    settle: bool,
}

/// `PreUpdate`, after the controls are read: the `menu` context while a menu is open, its
/// actions as `ui_navigate`, and the pause key as `ui_open pause`.
fn menu_controls(world: &mut World) {
    let menu_open = shards_ui::top_screen(world).is_some();
    let pushed = world.resource::<MenuControls>().pushed;
    if menu_open != pushed
        && let Some(mut contexts) = world.get_resource_mut::<InputContexts>()
    {
        // A context pushed above `menu` keeps it until that one comes off: popping now would
        // take the other one away instead.
        let changed = if menu_open {
            contexts.push("menu");
            true
        } else if contexts.top() == "menu" {
            contexts.pop();
            true
        } else {
            false
        };
        if changed {
            world.resource_mut::<MenuControls>().pushed = menu_open;
            // This frame's controls were read in the other context.
            return;
        }
    }
    forget_released_clicks(world);
    if let Some(path) = shards_ui::capturing(world) {
        capture(world, &path);
        return;
    }
    let Some(controls) = world.get_resource::<ControlState>().cloned() else { return };
    if world.resource::<MenuControls>().settle {
        if controls.axis("navigate") != Vec2::ZERO {
            return;
        }
        world.resource_mut::<MenuControls>().settle = false;
    }
    if !menu_open {
        let has_pause = world.get_resource::<ContentDb>().is_some_and(|db| db.get::<ScreenDef>(PAUSE_SCREEN).is_some());
        if controls.just_pressed("pause") && has_pause {
            call(world, "ui_open", json!({"screen": PAUSE_SCREEN}));
        }
        return;
    }
    if controls.just_pressed("back") {
        call(world, "ui_navigate", json!({"dir": "back"}));
        return;
    }
    if controls.just_pressed("confirm") {
        call(world, "ui_navigate", json!({"dir": "confirm"}));
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let axis = controls.axis("navigate");
    let dir = if axis.y.abs() >= axis.x.abs() && axis.y.abs() > 0.5 {
        Some(if axis.y > 0.0 { Nav::Up } else { Nav::Down })
    } else if axis.x.abs() > 0.5 {
        Some(if axis.x > 0.0 { Nav::Right } else { Nav::Left })
    } else {
        None
    };
    let mut state = world.resource_mut::<MenuControls>();
    let fire = match dir {
        None => {
            state.held = None;
            None
        }
        Some(d) if state.held != Some(d) => {
            state.held = Some(d);
            state.next_repeat = now + REPEAT_AFTER;
            Some(d)
        }
        Some(d) if now >= state.next_repeat => {
            state.next_repeat = now + REPEAT_EVERY;
            Some(d)
        }
        Some(_) => None,
    };
    if let Some(d) = fire {
        let word = match d {
            Nav::Up => "up",
            Nav::Down => "down",
            Nav::Left => "left",
            Nav::Right => "right",
            Nav::Next => "next",
            Nav::Prev => "prev",
            Nav::Confirm => "confirm",
            Nav::Back => "back",
        };
        call(world, "ui_navigate", json!({"dir": word}));
    }
}

/// The order mouse buttons are taken in when two go down in one frame.
fn mouse_rank(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(n) => n.saturating_add(5),
    }
}

/// A binding widget waits: Escape or Start stops the wait (`ui_navigate back`), else the first key,
/// mouse button or gamepad button that went down this frame becomes the binding (`ui_set`). The
/// menu hears none of it.
fn capture(world: &mut World, path: &str) {
    let mut gamepads = world.query::<(Entity, &Gamepad)>();
    let mut pads: Vec<(Entity, &Gamepad)> = gamepads.iter(world).collect();
    let keys = world.get_resource::<ButtonInput<KeyCode>>();
    let mouse = world.get_resource::<ButtonInput<MouseButton>>();
    pads.sort_by_key(|(e, _)| *e);
    let cancel = keys.is_some_and(|k| k.just_pressed(KeyCode::Escape)) || pads.iter().any(|(_, p)| p.just_pressed(GamepadButton::Start));
    let key = keys.and_then(|k| k.get_just_pressed().min().copied());
    let click = mouse.and_then(|m| m.get_just_pressed().copied().min_by_key(|b| mouse_rank(*b)));
    let pad = pads.iter().find_map(|(_, p)| p.get_just_pressed().min().copied());
    let input = key.map(Binding::Key).or(click.map(Binding::Mouse)).or(pad.map(Binding::GamepadButton));
    if cancel {
        call(world, "ui_navigate", json!({"dir": "back"}));
    } else if let Some(input) = input {
        if let Binding::Mouse(button) = input {
            world.resource_mut::<CapturedClicks>().0.push(button);
        }
        call(world, "ui_set", json!({"id": path, "value": input.text()}));
    } else {
        return;
    }
    world.resource_mut::<MenuControls>().settle = true;
}

/// A mouse button a binding took stops guarding its click a frame after it is let go: the click
/// comes on the frame it is let go, and a release away from any widget brings none.
fn forget_released_clicks(world: &mut World) {
    let Some(mouse) = world.get_resource::<ButtonInput<MouseButton>>() else { return };
    let gone: Vec<MouseButton> = world
        .get_resource::<CapturedClicks>()
        .map(|c| c.0.iter().copied().filter(|b| !mouse.pressed(*b) && !mouse.just_released(*b)).collect())
        .unwrap_or_default();
    if !gone.is_empty() {
        world.resource_mut::<CapturedClicks>().0.retain(|b| !gone.contains(b));
    }
}

fn call(world: &mut World, tool: &str, args: serde_json::Value) {
    let out = ai_core::call_tool(world, INPUT_AGENT, tool, args);
    if !out.ok {
        debug!("ui: {tool}: {}", out.result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearings_are_clockwise_from_north() {
        assert!((bearing(Vec3::NEG_Z) - 0.0).abs() < 1e-3);
        assert!((bearing(Vec3::X) - 90.0).abs() < 1e-3);
        assert!((bearing(Vec3::Z) - 180.0).abs() < 1e-3);
        assert!((bearing(Vec3::NEG_X) - 270.0).abs() < 1e-3);
    }
}
