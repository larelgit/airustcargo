//! The data of the input action map: which physical inputs trigger which actions, in
//! which contexts. Plain serializable data, so it can be a content file, be inspected and
//! changed by an agent, and be saved as the player's overrides.

use std::fmt;
use std::path::Path;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{ContentDef, Refs};

/// Which gamepad stick.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stick {
    Left,
    Right,
}

fn deadzone() -> f32 {
    0.15
}

fn one() -> f32 {
    1.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

/// The value of a stick: the radial dead zone is cut out and the rest rescaled to 0..=1,
/// the magnitude `m` becomes `m^curve` (direction kept; `curve` 1.0 leaves it alone), and
/// each axis is multiplied by its own scale (`scale_x`/`scale_y`, or `scale` where absent).
pub(crate) fn stick_response(raw: Vec2, deadzone: f32, scale: f32, scale_x: Option<f32>, scale_y: Option<f32>, curve: f32) -> Vec2 {
    let len = raw.length();
    if len <= deadzone {
        return Vec2::ZERO;
    }
    let mut m = ((len - deadzone) / (1.0 - deadzone).max(1e-3)).min(1.0);
    if curve != 1.0 {
        m = ops::powf(m, curve);
    }
    let dir = raw / len * m;
    Vec2::new(dir.x * scale_x.unwrap_or(scale), dir.y * scale_y.unwrap_or(scale))
}

/// One physical input. Buttons give an action its pressed state; axes give it a value
/// (`move` and `look` are 2D axes the engine routes to `Intent` and the camera).
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    GamepadButton(GamepadButton),
    /// One gamepad axis as a number. Values inside `deadzone` count as 0; `scale` may be
    /// negative to flip it.
    GamepadAxis {
        axis: GamepadAxis,
        #[serde(default = "deadzone")]
        deadzone: f32,
        #[serde(default = "one")]
        scale: f32,
    },
    /// Two keys as one axis: -1 while `neg` is held, +1 while `pos` is.
    KeyAxis { neg: KeyCode, pos: KeyCode },
    /// Four keys as a 2D axis (WASD): `up` is +y.
    Keys2D { up: KeyCode, down: KeyCode, left: KeyCode, right: KeyCode },
    /// Four gamepad buttons as a 2D axis (the D-pad): `up` is +y.
    Buttons2D { up: GamepadButton, down: GamepadButton, left: GamepadButton, right: GamepadButton },
    /// A gamepad stick as a 2D axis, its dead zone cut out of the middle. `scale` applies to
    /// both axes unless `scale_x` / `scale_y` (written `Some(3.2)`) set one of them; `curve`
    /// is the exponent on the magnitude after the dead zone (1.0, the default, is linear;
    /// 1.6 gives a finer centre). `curve` must be above 0 and finite, scales finite.
    Stick {
        stick: Stick,
        #[serde(default = "deadzone")]
        deadzone: f32,
        #[serde(default = "one")]
        scale: f32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale_x: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scale_y: Option<f32>,
        #[serde(default = "one", skip_serializing_if = "is_one")]
        curve: f32,
    },
    /// Mouse movement as a 2D axis (look); `scale` converts pixels to the action's units.
    MouseMotion {
        #[serde(default = "one")]
        scale: f32,
    },
    /// The mouse wheel as a 2D axis: this frame's scroll times `scale`, `y` up (away from the
    /// player), `x` a tilt to the right. A notch is one line, or the pixels a touchpad sends.
    MouseWheel {
        #[serde(default = "one")]
        scale: f32,
    },
}

/// A physical input without its settings, to tell whether two bindings use the same one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Physical {
    Key(KeyCode),
    Mouse(MouseButton),
    Pad(GamepadButton),
    PadAxis(GamepadAxis),
    Stick(Stick),
    MouseMotion,
    MouseWheel,
}

impl Binding {
    fn physical(&self) -> Vec<Physical> {
        match *self {
            Binding::Key(k) => vec![Physical::Key(k)],
            Binding::Mouse(m) => vec![Physical::Mouse(m)],
            Binding::GamepadButton(b) => vec![Physical::Pad(b)],
            Binding::GamepadAxis { axis, .. } => vec![Physical::PadAxis(axis)],
            Binding::KeyAxis { neg, pos } => vec![Physical::Key(neg), Physical::Key(pos)],
            Binding::Keys2D { up, down, left, right } => {
                vec![Physical::Key(up), Physical::Key(down), Physical::Key(left), Physical::Key(right)]
            }
            Binding::Buttons2D { up, down, left, right } => {
                vec![Physical::Pad(up), Physical::Pad(down), Physical::Pad(left), Physical::Pad(right)]
            }
            Binding::Stick { stick, .. } => vec![Physical::Stick(stick)],
            Binding::MouseMotion { .. } => vec![Physical::MouseMotion],
            Binding::MouseWheel { .. } => vec![Physical::MouseWheel],
        }
    }

    /// Whether the two bindings read some same physical input (a key inside a WASD
    /// binding counts).
    pub fn overlaps(&self, other: &Binding) -> bool {
        let theirs = other.physical();
        self.physical().iter().any(|p| theirs.contains(p))
    }

    /// Whether this binding gives a number rather than a pressed state.
    pub fn is_axis(&self) -> bool {
        matches!(
            self,
            Binding::GamepadAxis { .. }
                | Binding::KeyAxis { .. }
                | Binding::Keys2D { .. }
                | Binding::Buttons2D { .. }
                | Binding::Stick { .. }
                | Binding::MouseMotion { .. }
                | Binding::MouseWheel { .. }
        )
    }

    /// The binding as the text of a content file: `Key(Space)`, `Mouse(Left)`,
    /// `Keys2D(up: KeyW, down: KeyS, left: KeyA, right: KeyD)`. [`parse`](Self::parse) reads it.
    pub fn text(&self) -> String {
        ron::ser::to_string(self).unwrap_or_else(|_| format!("{self:?}"))
    }

    /// Whether a gamepad gives it (a button, an axis, a stick), not the keyboard or the mouse.
    pub fn is_gamepad(&self) -> bool {
        matches!(self, Binding::GamepadButton(_) | Binding::GamepadAxis { .. } | Binding::Buttons2D { .. } | Binding::Stick { .. })
    }

    /// The binding as a player reads it in a menu: `F`, `Space`, `Left Shift`, `Mouse Right`,
    /// `Pad South`, `Pad L2`, `WASD`. The keys' names in English, as keyboards print them;
    /// [`text`](Self::text) is what settings and tools take.
    pub fn label(&self) -> String {
        match self {
            Binding::Key(key) => key_label(*key),
            Binding::Mouse(button) => format!("Mouse {}", mouse_label(*button)),
            Binding::GamepadButton(button) => format!("Pad {}", pad_label(*button)),
            Binding::GamepadAxis { axis, .. } => format!("Pad {axis:?}"),
            Binding::KeyAxis { neg, pos } => format!("{}/{}", key_label(*neg), key_label(*pos)),
            Binding::Keys2D { up, down, left, right } => {
                let keys = [up, left, down, right].map(|k| key_label(*k));
                if keys.iter().all(|k| k.chars().count() == 1) { keys.concat() } else { keys.join("/") }
            }
            Binding::Buttons2D { up, down, left, right } => {
                let d_pad = [GamepadButton::DPadUp, GamepadButton::DPadLeft, GamepadButton::DPadDown, GamepadButton::DPadRight];
                if [*up, *left, *down, *right] == d_pad {
                    "Pad D-Pad".to_owned()
                } else {
                    format!("Pad {}", [up, left, down, right].map(|b| pad_label(*b)).join("/"))
                }
            }
            Binding::Stick { stick, .. } => format!("{stick:?} Stick"),
            Binding::MouseMotion { .. } => "Mouse".to_owned(),
            Binding::MouseWheel { .. } => "Mouse Wheel".to_owned(),
        }
    }

    /// Reads a binding written like [`text`](Self::text) gives it.
    pub fn parse(text: &str) -> Result<Binding, String> {
        let binding: Binding = ron::from_str(text.trim()).map_err(|e| {
            format!(
                "`{text}` is not a binding ({e}); write Key(KeyE), Mouse(Left), GamepadButton(South), \
                 GamepadAxis(axis: LeftStickX), KeyAxis(neg: KeyA, pos: KeyD), \
                 Keys2D(up: KeyW, down: KeyS, left: KeyA, right: KeyD), \
                 Buttons2D(up: DPadUp, down: DPadDown, left: DPadLeft, right: DPadRight), Stick(stick: Left) (optionally deadzone, scale, scale_x: Some(n), scale_y: Some(n), curve), MouseMotion() or MouseWheel()"
            )
        })?;
        binding.check().map_err(|e| format!("`{text}`: {e}"))?;
        Ok(binding)
    }

    /// Whether the numbers of a stick binding are usable: `curve` finite and above 0, the
    /// scales finite (a NaN would flow into `Intent`). Other bindings pass.
    fn check(&self) -> Result<(), String> {
        if let Binding::Stick { stick, scale, scale_x, scale_y, curve, .. } = self {
            if !(curve.is_finite() && *curve > 0.0) {
                return Err(format!("{stick:?} stick `curve` is {curve}; it must be finite and above 0 (1.0 is linear)"));
            }
            for (name, v) in [("scale", Some(*scale)), ("scale_x", *scale_x), ("scale_y", *scale_y)] {
                if v.is_some_and(|v| !v.is_finite()) {
                    return Err(format!("{stick:?} stick `{name}` is not a finite number"));
                }
            }
        }
        Ok(())
    }
}

/// A key as keyboards print it: `F`, `7`, `Space`, `Left Shift`, `Up`, `Num 4`, `Esc`.
fn key_label(key: KeyCode) -> String {
    let name = format!("{key:?}");
    if let Some(letter) = name.strip_prefix("Key").filter(|l| l.len() == 1) {
        return letter.to_owned();
    }
    if let Some(digit) = name.strip_prefix("Digit") {
        return digit.to_owned();
    }
    if let Some(rest) = name.strip_prefix("Numpad") {
        return format!("Num {rest}");
    }
    if let Some(direction) = name.strip_prefix("Arrow") {
        return direction.to_owned();
    }
    if key == KeyCode::Escape {
        return "Esc".to_owned();
    }
    for side in ["Left", "Right"] {
        if let Some(base) = name.strip_suffix(side).filter(|b| !b.is_empty()) {
            let base = if base == "Control" { "Ctrl" } else { base };
            return format!("{side} {base}");
        }
    }
    name
}

fn mouse_label(button: MouseButton) -> String {
    match button {
        MouseButton::Other(n) => n.to_string(),
        other => format!("{other:?}"),
    }
}

/// A gamepad button by its place: `South`, `L1`, `R2`, `L3`, `D-Up`, `Start`.
fn pad_label(button: GamepadButton) -> String {
    match button {
        GamepadButton::LeftTrigger => "L1".to_owned(),
        GamepadButton::LeftTrigger2 => "L2".to_owned(),
        GamepadButton::RightTrigger => "R1".to_owned(),
        GamepadButton::RightTrigger2 => "R2".to_owned(),
        GamepadButton::LeftThumb => "L3".to_owned(),
        GamepadButton::RightThumb => "R3".to_owned(),
        GamepadButton::DPadUp => "D-Up".to_owned(),
        GamepadButton::DPadDown => "D-Down".to_owned(),
        GamepadButton::DPadLeft => "D-Left".to_owned(),
        GamepadButton::DPadRight => "D-Right".to_owned(),
        GamepadButton::Other(n) => n.to_string(),
        other => format!("{other:?}"),
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text())
    }
}

/// How an action reacts to its inputs.
#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BindMode {
    /// A one-shot press each time an input goes down (jump, attack).
    #[default]
    Press,
    /// Held while an input is down (sprint, block).
    Hold,
    /// A press turns it on until the next press: a hold for players who cannot keep a
    /// button down (accessibility).
    Toggle,
    /// Both: a one-shot press when an input goes down (as `Press`), and held while it stays
    /// down (as `Hold`): an attack that charges while held, an interaction that goes on while
    /// held. With "hold to toggle" the first press presses and holds, the next lets go (and
    /// presses nothing).
    PressHold,
}

/// One action of a context: its name (an `Intent` input name such as `jump`, or the axes
/// `move` and `look`), the inputs that trigger it and how.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActionBinding {
    /// `jump`, `sprint`, `light`, `technique.1`, `move`, `look`, or a game's own.
    pub action: String,
    /// Slot 0 is the primary binding, slot 1 the alternative, and so on.
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub mode: BindMode,
    /// Presses carry the direction the player is moving (a dodge goes where you steer).
    #[serde(default)]
    pub directional: bool,
}

impl ActionBinding {
    /// An action with these bindings, pressed once per input.
    pub fn new(action: &str, bindings: impl IntoIterator<Item = Binding>) -> Self {
        Self { action: action.to_owned(), bindings: bindings.into_iter().collect(), mode: BindMode::Press, directional: false }
    }

    pub fn held(mut self) -> Self {
        self.mode = BindMode::Hold;
        self
    }

    /// Pressed when it goes down and held while it stays down ([`BindMode::PressHold`]).
    pub fn press_held(mut self) -> Self {
        self.mode = BindMode::PressHold;
        self
    }

    pub fn directional(mut self) -> Self {
        self.directional = true;
        self
    }
}

fn yes() -> bool {
    true
}

/// A set of bindings that is active on its own: `gameplay`, `menu`, `aim`; games add more.
/// Only the top context of the stack ([`InputContexts`](super::InputContexts)) fires.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InputContext {
    pub name: String,
    pub actions: Vec<ActionBinding>,
    /// Whether the actions drive the player's `Intent` (movement, held buttons, presses).
    /// A menu's do not: they are only seen through `ControlState`, by the UI.
    #[serde(default = "yes")]
    pub drives_intent: bool,
}

impl Default for InputContext {
    fn default() -> Self {
        Self { name: String::new(), actions: Vec::new(), drives_intent: true }
    }
}

impl InputContext {
    pub fn action(&self, name: &str) -> Option<&ActionBinding> {
        self.actions.iter().find(|a| a.action == name)
    }
}

/// Why a rebinding was refused.
#[derive(Clone, Debug, PartialEq)]
pub enum RebindError {
    NoContext(String),
    NoAction { context: String, action: String },
    BadSlot { action: String, slot: usize, bound: usize },
    /// Another action of the same context already reads this input.
    Conflict { context: String, binding: Binding, action: String },
    /// The binding's numbers are unusable (a stick's `curve` or scale is NaN, infinite or not above 0).
    Invalid(String),
}

impl fmt::Display for RebindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RebindError::NoContext(name) => write!(f, "no input context `{name}`"),
            RebindError::NoAction { context, action } => write!(f, "no action `{action}` in context `{context}`"),
            RebindError::BadSlot { action, slot, bound } => {
                let hint = if *bound == 0 { "slot 0 adds the first".to_owned() } else { format!("slots 0..{} replace, slot {bound} adds one", bound - 1) };
                write!(f, "`{action}` has {bound} binding(s), so slot {slot} does not exist ({hint})")
            }
            RebindError::Invalid(why) => write!(f, "{why}"),
            RebindError::Conflict { context, binding, action } => write!(
                f,
                "{binding} is already bound to `{action}` in `{context}`; rebind `{action}` first, or force it to take the input over"
            ),
        }
    }
}

/// Every context's bindings: the default controls of a game, or the player's own.
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[reflect(Resource, Default)]
#[serde(default)]
pub struct InputMap {
    pub contexts: Vec<InputContext>,
}

impl InputMap {
    pub fn context(&self, name: &str) -> Option<&InputContext> {
        self.contexts.iter().find(|c| c.name == name)
    }

    pub fn action(&self, context: &str, action: &str) -> Option<&ActionBinding> {
        self.context(context)?.action(action)
    }

    fn action_mut(&mut self, context: &str, action: &str) -> Result<&mut ActionBinding, RebindError> {
        let ctx = self
            .contexts
            .iter_mut()
            .find(|c| c.name == context)
            .ok_or_else(|| RebindError::NoContext(context.to_owned()))?;
        ctx.actions
            .iter_mut()
            .find(|a| a.action == action)
            .ok_or_else(|| RebindError::NoAction { context: context.to_owned(), action: action.to_owned() })
    }

    /// The action of `context` that reads an input `binding` also reads, other than `skip`.
    pub fn used_by(&self, context: &str, binding: &Binding, skip: &str) -> Option<&str> {
        self.context(context)?
            .actions
            .iter()
            .filter(|a| a.action != skip)
            .find(|a| a.bindings.iter().any(|b| b.overlaps(binding)))
            .map(|a| a.action.as_str())
    }

    /// Puts `binding` in slot `slot` of `action` in `context`: replaces the one there, or
    /// adds it when `slot` is the number of bindings. Refused when another action of the
    /// context already uses the input (the error names it); [`rebind_taking`](Self::rebind_taking)
    /// takes it over instead.
    pub fn rebind(&mut self, context: &str, action: &str, slot: usize, binding: Binding) -> Result<(), RebindError> {
        binding.check().map_err(RebindError::Invalid)?;
        if let Some(other) = self.used_by(context, &binding, action) {
            return Err(RebindError::Conflict { context: context.to_owned(), binding, action: other.to_owned() });
        }
        self.place(context, action, slot, binding)
    }

    /// Like [`rebind`](Self::rebind), but the other action loses the input (its bindings
    /// that read it are removed). Returns the actions that lost something.
    pub fn rebind_taking(&mut self, context: &str, action: &str, slot: usize, binding: Binding) -> Result<Vec<String>, RebindError> {
        // Check the binding and the target first: a mistake must not strip another action for nothing.
        binding.check().map_err(RebindError::Invalid)?;
        let bound = self.action_mut(context, action)?.bindings.len();
        if slot > bound {
            return Err(RebindError::BadSlot { action: action.to_owned(), slot, bound });
        }
        let mut losers = Vec::new();
        if let Some(ctx) = self.contexts.iter_mut().find(|c| c.name == context) {
            for other in ctx.actions.iter_mut().filter(|a| a.action != action) {
                let before = other.bindings.len();
                other.bindings.retain(|b| !b.overlaps(&binding));
                if other.bindings.len() != before {
                    losers.push(other.action.clone());
                }
            }
        }
        self.place(context, action, slot, binding)?;
        Ok(losers)
    }

    fn place(&mut self, context: &str, action: &str, slot: usize, binding: Binding) -> Result<(), RebindError> {
        let entry = self.action_mut(context, action)?;
        let bound = entry.bindings.len();
        match slot {
            s if s < bound => entry.bindings[s] = binding,
            s if s == bound => entry.bindings.push(binding),
            slot => return Err(RebindError::BadSlot { action: action.to_owned(), slot, bound }),
        }
        Ok(())
    }

    /// Removes slot `slot` of `action`.
    pub fn unbind(&mut self, context: &str, action: &str, slot: usize) -> Result<Binding, RebindError> {
        let entry = self.action_mut(context, action)?;
        if slot >= entry.bindings.len() {
            return Err(RebindError::BadSlot { action: action.to_owned(), slot, bound: entry.bindings.len() });
        }
        Ok(entry.bindings.remove(slot))
    }

    /// Changes how `action` reacts (press, hold, toggle).
    pub fn set_mode(&mut self, context: &str, action: &str, mode: BindMode) -> Result<(), RebindError> {
        self.action_mut(context, action)?.mode = mode;
        Ok(())
    }

    /// Replaces or adds the actions `overrides` lists, context by context.
    pub fn apply(&mut self, overrides: &InputMap) {
        for over in &overrides.contexts {
            let ctx = match self.contexts.iter().position(|c| c.name == over.name) {
                Some(at) => &mut self.contexts[at],
                None => {
                    self.contexts.push(InputContext { name: over.name.clone(), actions: Vec::new(), drives_intent: over.drives_intent });
                    self.contexts.last_mut().unwrap_or_else(|| unreachable!("just pushed"))
                }
            };
            for action in &over.actions {
                match ctx.actions.iter_mut().find(|a| a.action == action.action) {
                    Some(mine) => *mine = action.clone(),
                    None => ctx.actions.push(action.clone()),
                }
            }
        }
    }

    /// What differs from `defaults`: the actions that were changed or added. What a
    /// player's settings file holds, so new default actions still reach old files.
    pub fn overrides_from(&self, defaults: &InputMap) -> InputMap {
        let contexts = self
            .contexts
            .iter()
            .filter_map(|ctx| {
                let base = defaults.context(&ctx.name);
                let actions: Vec<ActionBinding> =
                    ctx.actions.iter().filter(|a| base.and_then(|b| b.action(&a.action)) != Some(*a)).cloned().collect();
                (!actions.is_empty()).then(|| InputContext { name: ctx.name.clone(), actions, drives_intent: ctx.drives_intent })
            })
            .collect();
        InputMap { contexts }
    }

    /// The map as text (RON), for files.
    pub fn to_text(&self) -> Result<String, String> {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(8).indentor("    ".to_owned());
        ron::ser::to_string_pretty(self, pretty).map_err(|e| e.to_string())
    }

    /// Reads a map from RON text.
    pub fn from_text(text: &str) -> Result<InputMap, String> {
        let map: InputMap = ron::from_str(text).map_err(|e| e.to_string())?;
        for ctx in &map.contexts {
            for action in &ctx.actions {
                for binding in &action.bindings {
                    binding.check().map_err(|e| format!("context `{}`: action `{}`: {e}", ctx.name, action.action))?;
                }
            }
        }
        Ok(map)
    }

    /// Reads overrides from `path`; a missing file is no overrides. The format of the
    /// `input.ron` that earlier versions kept the player's controls in, which the first run on
    /// the settings imports (`controls` settings are the controls now).
    pub fn load_overrides(path: &Path) -> Result<InputMap, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => InputMap::from_text(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(InputMap::default()),
            Err(e) => Err(format!("cannot read {}: {e}", path.display())),
        }
    }

    /// The action names of a context, for messages.
    pub fn action_names(&self, context: &str) -> Vec<&str> {
        self.context(context).map(|c| c.actions.iter().map(|a| a.action.as_str()).collect()).unwrap_or_default()
    }

    /// `" (did you mean `x`?)"` for a misspelled action or context.
    pub fn suggest<'a>(wanted: &str, names: impl Iterator<Item = &'a str>) -> String {
        closest(wanted, names).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default()
    }
}

impl ContentDef for InputMap {
    const KIND: &'static str = "input_map";
    // Only what a machine shows or plays, or what lies outside a session: never in the replay check.
    const SIMULATION: bool = false;

    /// The default controls: `content/**/*.input_map.ron`, one map with id `default`.
    fn validate(&self, _: &str, refs: &mut Refs) {
        for ctx in &self.contexts {
            for action in &ctx.actions {
                for binding in &action.bindings {
                    if let Err(e) = binding.check() {
                        refs.error(format!("context `{}`: action `{}`: {e}", ctx.name, action.action));
                    }
                }
            }
            if ctx.name.is_empty() {
                refs.error("a context has no name");
            }
            // The player's changes are stored as `context.action`, split at the first dot (actions
            // like `technique.1` have dots), so a context with one could never be set or read back.
            if ctx.name.contains('.') {
                refs.error(format!(
                    "context `{}`: a context name cannot contain `.` (controls are saved as `context.action`, and actions may have dots); \
                     use `_` or `-`, for example `{}`",
                    ctx.name,
                    ctx.name.replace('.', "_")
                ));
            }
            let mut seen: Vec<&str> = Vec::new();
            for (n, action) in ctx.actions.iter().enumerate() {
                if seen.contains(&action.action.as_str()) {
                    refs.error(format!("context `{}`: action `{}` is listed twice", ctx.name, action.action));
                }
                seen.push(&action.action);
                for other in &ctx.actions[n + 1..] {
                    let clash = action.bindings.iter().find(|a| other.bindings.iter().any(|b| a.overlaps(b)));
                    if let Some(binding) = clash {
                        refs.error(format!(
                            "context `{}`: `{}` and `{}` both read the input of {binding}; give one of them another input",
                            ctx.name, action.action, other.action
                        ));
                    }
                }
            }
        }
    }
}

impl InputMap {
    /// The engine's own controls, used until content says otherwise: `gameplay` (WASD,
    /// mouse look, the standard `Intent` buttons, a gamepad layout) and `menu`.
    pub fn standard() -> InputMap {
        use Binding::{GamepadButton as Pad, Key, Mouse};
        let keys = |up, down, left, right| Binding::Keys2D { up, down, left, right };
        let gameplay = vec![
            ActionBinding::new(
                "move",
                [
                    keys(KeyCode::KeyW, KeyCode::KeyS, KeyCode::KeyA, KeyCode::KeyD),
                    keys(KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight),
                    Binding::Stick { stick: Stick::Left, deadzone: 0.15, scale: 1.0, scale_x: None, scale_y: None, curve: 1.0 },
                ],
            ),
            ActionBinding::new(
                "look",
                [Binding::MouseMotion { scale: 0.0025 }, Binding::Stick { stick: Stick::Right, deadzone: 0.15, scale: 2.8, scale_x: None, scale_y: None, curve: 1.0 }],
            ),
            ActionBinding::new("jump", [Key(KeyCode::Space), Pad(GamepadButton::South)]),
            ActionBinding::new("sprint", [Key(KeyCode::ShiftLeft), Pad(GamepadButton::LeftThumb)]).held(),
            ActionBinding::new("glide", [Key(KeyCode::KeyF), Pad(GamepadButton::RightTrigger2)]).held(),
            ActionBinding::new("dodge", [Key(KeyCode::ControlLeft), Pad(GamepadButton::East)]).directional(),
            ActionBinding::new("light", [Mouse(MouseButton::Left), Pad(GamepadButton::West)]),
            // Pressed and held: a charged heavy turns while the button stays down.
            ActionBinding::new("heavy", [Mouse(MouseButton::Right), Pad(GamepadButton::RightTrigger)]).press_held(),
            ActionBinding::new("block", [Key(KeyCode::KeyQ), Pad(GamepadButton::LeftTrigger)]).held(),
            ActionBinding::new("parry", [Key(KeyCode::KeyR), Pad(GamepadButton::LeftTrigger2)]),
            ActionBinding::new("technique.1", [Key(KeyCode::Digit1), Pad(GamepadButton::DPadUp)]),
            ActionBinding::new("technique.2", [Key(KeyCode::Digit2), Pad(GamepadButton::DPadLeft)]),
            ActionBinding::new("technique.3", [Key(KeyCode::Digit3), Pad(GamepadButton::DPadRight)]),
            ActionBinding::new("use_item", [Key(KeyCode::KeyC), Pad(GamepadButton::DPadDown)]),
            ActionBinding::new("interact", [Key(KeyCode::KeyE), Pad(GamepadButton::North)]).press_held(),
            ActionBinding::new("lock_on", [Key(KeyCode::Tab), Mouse(MouseButton::Middle), Pad(GamepadButton::RightThumb)]),
            // The wheel switches the lock-on target (the camera turns it into `lock_switch` presses).
            ActionBinding::new("lock_switch", [Binding::MouseWheel { scale: 1.0 }]),
            // Opens the game's `pause` screen (shards_ui); in a menu `back` closes it.
            ActionBinding::new("pause", [Key(KeyCode::Escape), Pad(GamepadButton::Start)]),
        ];
        let menu = vec![
            ActionBinding::new(
                "navigate",
                [
                    keys(KeyCode::KeyW, KeyCode::KeyS, KeyCode::KeyA, KeyCode::KeyD),
                    keys(KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::ArrowLeft, KeyCode::ArrowRight),
                    Binding::Stick { stick: Stick::Left, deadzone: 0.3, scale: 1.0, scale_x: None, scale_y: None, curve: 1.0 },
                    Binding::Buttons2D {
                        up: GamepadButton::DPadUp,
                        down: GamepadButton::DPadDown,
                        left: GamepadButton::DPadLeft,
                        right: GamepadButton::DPadRight,
                    },
                ],
            ),
            ActionBinding::new("confirm", [Key(KeyCode::Enter), Pad(GamepadButton::South)]),
            ActionBinding::new("back", [Key(KeyCode::Escape), Pad(GamepadButton::East)]),
        ];
        InputMap {
            contexts: vec![
                InputContext { name: "gameplay".into(), actions: gameplay, drives_intent: true },
                InputContext { name: "menu".into(), actions: menu, drives_intent: false },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_map_is_valid_and_has_no_clashes() {
        let map = InputMap::standard();
        for ctx in &map.contexts {
            for (n, a) in ctx.actions.iter().enumerate() {
                for b in &ctx.actions[n + 1..] {
                    let clash = a.bindings.iter().find(|x| b.bindings.iter().any(|y| x.overlaps(y)));
                    assert!(clash.is_none(), "{}: {} and {} share {clash:?}", ctx.name, a.action, b.action);
                }
            }
        }
    }

    #[test]
    fn a_binding_reads_back_from_its_text() {
        for binding in [
            Binding::Key(KeyCode::Space),
            Binding::Mouse(MouseButton::Left),
            Binding::GamepadButton(GamepadButton::South),
            Binding::Stick { stick: Stick::Left, deadzone: 0.2, scale: 1.0, scale_x: None, scale_y: None, curve: 1.0 },
            Binding::Keys2D { up: KeyCode::KeyW, down: KeyCode::KeyS, left: KeyCode::KeyA, right: KeyCode::KeyD },
            Binding::KeyAxis { neg: KeyCode::KeyA, pos: KeyCode::KeyD },
            Binding::Buttons2D {
                up: GamepadButton::DPadUp,
                down: GamepadButton::DPadDown,
                left: GamepadButton::DPadLeft,
                right: GamepadButton::DPadRight,
            },
        ] {
            assert_eq!(Binding::parse(&binding.text()), Ok(binding.clone()), "{}", binding.text());
        }
        assert_eq!(Binding::parse("GamepadAxis(axis: LeftStickX)").unwrap(), Binding::GamepadAxis { axis: GamepadAxis::LeftStickX, deadzone: 0.15, scale: 1.0 });
        // A D-pad button inside the menu's D-pad is the same input.
        let d_pad = Binding::parse("Buttons2D(up: DPadUp, down: DPadDown, left: DPadLeft, right: DPadRight)").unwrap();
        assert!(d_pad.is_axis() && d_pad.overlaps(&Binding::GamepadButton(GamepadButton::DPadLeft)));
        let err = Binding::parse("Space").unwrap_err();
        assert!(err.contains("not a binding") && err.contains("Key(KeyE)"), "{err}");
    }

    #[test]
    fn a_binding_has_a_label_a_player_reads() {
        let labels: Vec<String> = [
            Binding::Key(KeyCode::KeyF),
            Binding::Key(KeyCode::Digit7),
            Binding::Key(KeyCode::Space),
            Binding::Key(KeyCode::ShiftLeft),
            Binding::Key(KeyCode::ControlRight),
            Binding::Key(KeyCode::ArrowUp),
            Binding::Key(KeyCode::Numpad4),
            Binding::Key(KeyCode::Escape),
            Binding::Key(KeyCode::Tab),
            Binding::Mouse(MouseButton::Right),
            Binding::Mouse(MouseButton::Other(5)),
            Binding::GamepadButton(GamepadButton::South),
            Binding::GamepadButton(GamepadButton::LeftTrigger2),
            Binding::GamepadButton(GamepadButton::RightThumb),
            Binding::GamepadButton(GamepadButton::DPadUp),
            Binding::Keys2D { up: KeyCode::KeyW, down: KeyCode::KeyS, left: KeyCode::KeyA, right: KeyCode::KeyD },
            Binding::Keys2D { up: KeyCode::ArrowUp, down: KeyCode::ArrowDown, left: KeyCode::ArrowLeft, right: KeyCode::ArrowRight },
            Binding::Buttons2D {
                up: GamepadButton::DPadUp,
                down: GamepadButton::DPadDown,
                left: GamepadButton::DPadLeft,
                right: GamepadButton::DPadRight,
            },
            Binding::Buttons2D {
                up: GamepadButton::North,
                down: GamepadButton::South,
                left: GamepadButton::West,
                right: GamepadButton::East,
            },
            Binding::Stick { stick: Stick::Left, deadzone: 0.15, scale: 1.0, scale_x: None, scale_y: None, curve: 1.0 },
        ]
        .iter()
        .map(Binding::label)
        .collect();
        assert_eq!(
            labels,
            [
                "F", "7", "Space", "Left Shift", "Right Ctrl", "Up", "Num 4", "Esc", "Tab", "Mouse Right", "Mouse 5", "Pad South", "Pad L2", "Pad R3",
                "Pad D-Up", "WASD", "Up/Left/Down/Right", "Pad D-Pad", "Pad North/West/South/East", "Left Stick",
            ]
        );
        assert!(Binding::GamepadButton(GamepadButton::North).is_gamepad() && Binding::Stick { stick: Stick::Right, deadzone: 0.1, scale: 1.0, scale_x: None, scale_y: None, curve: 1.0 }.is_gamepad());
        assert!(!Binding::Key(KeyCode::KeyF).is_gamepad() && !Binding::Mouse(MouseButton::Left).is_gamepad());
    }

    #[test]
    fn rebinding_replaces_adds_and_detects_conflicts() {
        let mut map = InputMap::standard();
        assert_eq!(map.rebind("gameplay", "jump", 0, Binding::Key(KeyCode::KeyJ)), Ok(()));
        assert_eq!(map.action("gameplay", "jump").unwrap().bindings[0], Binding::Key(KeyCode::KeyJ));
        // The next free slot adds.
        assert_eq!(map.rebind("gameplay", "jump", 2, Binding::Key(KeyCode::KeyK)), Ok(()));
        // W belongs to `move` (inside its Keys2D): a conflict that names it.
        let err = map.rebind("gameplay", "jump", 0, Binding::Key(KeyCode::KeyW)).unwrap_err();
        assert!(matches!(&err, RebindError::Conflict { action, .. } if action == "move"), "{err}");
        assert!(err.to_string().contains("already bound to `move` in `gameplay`"), "{err}");
        assert_eq!(map.action("gameplay", "jump").unwrap().bindings[0], Binding::Key(KeyCode::KeyJ), "refused: unchanged");
        // Taking it over strips the other action of that input only.
        let lost = map.rebind_taking("gameplay", "jump", 0, Binding::Key(KeyCode::KeyW)).unwrap();
        assert_eq!(lost, ["move"]);
        assert_eq!(map.action("gameplay", "move").unwrap().bindings.len(), 2, "the WASD binding went, the arrows and the stick stayed");
        // Mistakes are named.
        assert!(matches!(map.rebind("gameplay", "fly", 0, Binding::Key(KeyCode::KeyZ)), Err(RebindError::NoAction { .. })));
        assert!(matches!(map.rebind("race", "jump", 0, Binding::Key(KeyCode::KeyZ)), Err(RebindError::NoContext(_))));
        let slot = map.rebind("gameplay", "jump", 9, Binding::Key(KeyCode::KeyZ)).unwrap_err().to_string();
        assert!(slot.contains("slot 9 does not exist"), "{slot}");
        // The same input in another context is no conflict.
        assert_eq!(map.rebind("menu", "confirm", 0, Binding::Key(KeyCode::Space)), Ok(()));
    }

    #[test]
    fn overrides_hold_only_what_changed_and_apply_back() {
        let defaults = InputMap::standard();
        let mut mine = defaults.clone();
        mine.rebind("gameplay", "jump", 0, Binding::Key(KeyCode::KeyJ)).unwrap();
        mine.set_mode("gameplay", "sprint", BindMode::Toggle).unwrap();
        let overrides = mine.overrides_from(&defaults);
        assert_eq!(overrides.contexts.len(), 1);
        let names: Vec<&str> = overrides.contexts[0].actions.iter().map(|a| a.action.as_str()).collect();
        assert_eq!(names, ["jump", "sprint"]);
        let mut rebuilt = defaults.clone();
        rebuilt.apply(&overrides);
        assert_eq!(rebuilt, mine);
        // A text round trip loses nothing.
        assert_eq!(InputMap::from_text(&overrides.to_text().unwrap()).unwrap(), overrides);
    }

    #[test]
    fn stick_curve_one_is_exactly_the_old_response() {
        // The old formula, written out: curve 1.0 and no per-axis scales must match it bit for bit.
        let old = |raw: Vec2, deadzone: f32, scale: f32| {
            let len = raw.length();
            if len <= deadzone { Vec2::ZERO } else { raw / len * ((len - deadzone) / (1.0 - deadzone).max(1e-3)).min(1.0) * scale }
        };
        for xi in -10..=10 {
            for yi in -10..=10 {
                let raw = Vec2::new(xi as f32 / 10.0, yi as f32 / 10.0);
                for (deadzone, scale) in [(0.15, 1.0), (0.2, 2.8), (0.3, -1.5), (0.0, 1.0)] {
                    let got = stick_response(raw, deadzone, scale, None, None, 1.0);
                    assert_eq!(got.to_array().map(f32::to_bits), old(raw, deadzone, scale).to_array().map(f32::to_bits), "{raw} dz {deadzone} scale {scale}");
                }
            }
        }
    }

    #[test]
    fn stick_curve_shapes_the_magnitude_after_the_dead_zone() {
        // Deadzone 0.2: a stick at 0.6 is (0.6 - 0.2) / 0.8 = 0.5 past it; 0.5^1.6 is about 0.3299.
        let v = stick_response(Vec2::new(0.6, 0.0), 0.2, 1.0, None, None, 1.6);
        assert!((v.x - ops::powf(0.5_f32, 1.6)).abs() < 1e-5 && (v.x - 0.3299).abs() < 1e-3 && v.y == 0.0, "{v}");
        // The direction is kept on a diagonal; full deflection stays 1; inside the dead zone is 0.
        let d = stick_response(Vec2::new(0.6, 0.6) * 0.5, 0.2, 1.0, None, None, 1.6);
        assert!((d.x - d.y).abs() < 1e-6 && d.x > 0.0);
        assert!((stick_response(Vec2::new(1.0, 0.0), 0.2, 1.0, None, None, 1.6).x - 1.0).abs() < 1e-6);
        assert_eq!(stick_response(Vec2::new(0.1, 0.1), 0.2, 1.0, None, None, 1.6), Vec2::ZERO);
    }

    #[test]
    fn stick_scales_per_axis_and_falls_back_to_scale() {
        let raw = Vec2::new(1.0, 1.0).normalize();
        let both = stick_response(raw, 0.2, 2.0, None, None, 1.0);
        let split = stick_response(raw, 0.2, 2.0, Some(3.2), Some(-2.0), 1.0);
        assert!((split.x - both.x * 1.6).abs() < 1e-5 && (split.y + both.y).abs() < 1e-5, "{both} {split}");
        // One absent: that axis keeps `scale`.
        let x_only = stick_response(raw, 0.2, 2.0, Some(3.2), None, 1.0);
        assert!((x_only.x - split.x).abs() < 1e-6 && x_only.y == both.y);
    }

    #[test]
    fn old_stick_forms_parse_and_serialize_unchanged_and_new_ones_round_trip() {
        let old = Binding::Stick { stick: Stick::Left, deadzone: 0.15, scale: 1.0, scale_x: None, scale_y: None, curve: 1.0 };
        // Every old spelling reads as before...
        assert_eq!(Binding::parse("Stick(stick: Left)"), Ok(old.clone()));
        assert_eq!(Binding::parse("Stick(stick: Left, deadzone: 0.15, scale: 1.0)"), Ok(old.clone()));
        // ...and writes the same bytes as before the new fields existed.
        assert_eq!(old.text(), "Stick(stick:Left,deadzone:0.15,scale:1.0)");
        let right = Binding::Stick { stick: Stick::Right, deadzone: 0.15, scale: 2.8, scale_x: None, scale_y: None, curve: 1.0 };
        assert_eq!(right.text(), "Stick(stick:Right,deadzone:0.15,scale:2.8)");
        // A whole old map file round trips with no new field in it.
        let map = InputMap::standard();
        let text = map.to_text().unwrap();
        assert!(!text.contains("curve") && !text.contains("scale_x"), "{text}");
        assert_eq!(InputMap::from_text(&text).unwrap(), map);
        // The new fields go both ways through the text form.
        let look = Binding::Stick { stick: Stick::Right, deadzone: 0.2, scale: 1.0, scale_x: Some(3.2), scale_y: Some(2.0), curve: 1.6 };
        assert_eq!(look.text(), "Stick(stick:Right,deadzone:0.2,scale:1.0,scale_x:Some(3.2),scale_y:Some(2.0),curve:1.6)");
        assert_eq!(Binding::parse(&look.text()), Ok(look.clone()));
        assert_eq!(Binding::parse("Stick(stick: Right, deadzone: 0.2, scale_x: Some(3.2), scale_y: Some(2.0), curve: 1.6)"), Ok(look.clone()));
        let only_curve = Binding::parse("Stick(stick: Left, curve: 1.6)").unwrap();
        assert!(matches!(only_curve, Binding::Stick { curve, scale_x: None, scale_y: None, .. } if curve == 1.6));
        assert_eq!(Binding::parse(&only_curve.text()), Ok(only_curve));
        // In a map file too.
        let mut custom = InputMap::standard();
        custom.rebind("gameplay", "look", 1, look.clone()).unwrap();
        assert_eq!(InputMap::from_text(&custom.to_text().unwrap()).unwrap(), custom);
    }

    #[test]
    fn a_rebind_or_a_saved_string_with_a_bad_stick_number_is_refused() {
        for bad in [
            "Stick(stick: Right, curve: 0.0)",
            "Stick(stick: Right, curve: -1.0)",
            "Stick(stick: Right, curve: inf)",
            "Stick(stick: Right, curve: NaN)",
            "Stick(stick: Right, scale: NaN)",
            "Stick(stick: Right, scale_x: Some(NaN))",
            "Stick(stick: Right, scale_y: Some(inf))",
        ] {
            let err = Binding::parse(bad).expect_err(bad);
            assert!(err.contains("Right"), "{bad}: {err}");
        }
        assert!(Binding::parse("Stick(stick: Right, scale_x: Some(3.2), scale_y: Some(2.0), curve: 1.6)").is_ok());
        // A whole saved map is checked the same way.
        let mut map = InputMap::standard();
        let bad = Binding::Stick { stick: Stick::Right, deadzone: 0.2, scale: 1.0, scale_x: None, scale_y: None, curve: f32::NAN };
        // A caller's binding is checked too, and a refusal changes nothing.
        let err = map.rebind("gameplay", "look", 1, bad).unwrap_err();
        assert!(matches!(err, RebindError::Invalid(_)) && err.to_string().contains("curve"), "{err}");
        assert_eq!(map, InputMap::standard());
        // `rebind_taking` refuses before it strips the other action: `move` reads the left stick.
        let bad_left = Binding::Stick { stick: Stick::Left, deadzone: 0.2, scale: 1.0, scale_x: None, scale_y: None, curve: f32::NAN };
        let err = map.rebind_taking("gameplay", "look", 1, bad_left).unwrap_err();
        assert!(matches!(err, RebindError::Invalid(_)), "{err}");
        assert_eq!(map, InputMap::standard());
        assert!(InputMap::from_text(&map.to_text().unwrap()).is_ok());
    }
}
