//! The input action map (E2.4): keyboard, mouse and gamepad become `Intent`.
//!
//! The same `Intent` is what agents, brains and the network write, so a replay of a human
//! game is just the changes this module made. It runs in `PreUpdate`, between ticks, for
//! the entity with [`LocalPlayer`], and touches `Intent` only when something changed.
//!
//! ```text
//! InputMap (contexts → actions → bindings)   InputContexts (stack: only the top fires)
//!        │  ButtonInput<KeyCode>, ButtonInput<MouseButton>, Gamepad, mouse motion
//!        ▼
//! read_input ──► ControlState   what is down this frame (the camera and UI read it)
//!        └─────► Intent        move_dir (turned by the camera), held, pressed
//! ```
//!
//! Defaults come from content (`input_map`, id `default`, hot-reloadable) or the built-in
//! [`InputMap::standard`]; the player's changes are saved as overrides in
//! `$XDG_CONFIG_HOME/<game>/input.ron` (`~/.config/<game>/input.ron`; `%APPDATA%` on
//! Windows), so new default actions still reach old files.
//!
//! A content file:
//!
//! ```ron
//! // assets/content/controls.input_map.ron
//! { "default": (contexts: [
//!     (name: "gameplay", actions: [
//!         (action: "move", bindings: [Keys2D(up: KeyW, down: KeyS, left: KeyA, right: KeyD), Stick(stick: Left)]),
//!         (action: "jump", bindings: [Key(Space), GamepadButton(South)]),
//!         (action: "sprint", bindings: [Key(ShiftLeft)], mode: Hold),
//!         (action: "dodge", bindings: [Key(ControlLeft)], directional: true),
//!     ]),
//! ]) }
//! ```

mod map;
mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ai_core::{journal, JournalKind};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::input::InputSystems;
use bevy::prelude::*;
use shards_content::{ContentAppExt, ContentDb};
use shards_core::{EngineConfig, Intent, LocalPlayer, Press};

pub use map::{ActionBinding, BindMode, Binding, InputContext, InputMap, RebindError, Stick};

use crate::camera::ThirdPersonCamera;

/// The input systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlSystems {
    /// `PreUpdate`: the defaults are resolved and the devices read into `Intent` and
    /// [`ControlState`].
    Read,
}

/// The active contexts, bottom to top. Only the top one's bindings fire: pushing `menu`
/// blocks `gameplay` (its held buttons are let go) until `menu` is popped.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct InputContexts {
    stack: Vec<String>,
}

impl Default for InputContexts {
    fn default() -> Self {
        Self { stack: vec!["gameplay".to_owned()] }
    }
}

impl InputContexts {
    /// Makes `context` the active one.
    pub fn push(&mut self, context: &str) {
        self.stack.push(context.to_owned());
    }

    /// Leaves the active context for the one below it. The bottom one stays.
    pub fn pop(&mut self) -> Option<String> {
        (self.stack.len() > 1).then(|| self.stack.pop()).flatten()
    }

    /// Replaces the whole stack with `context` alone.
    pub fn set(&mut self, context: &str) {
        self.stack = vec![context.to_owned()];
    }

    /// The active context.
    pub fn top(&self) -> &str {
        self.stack.last().map_or("gameplay", String::as_str)
    }

    /// Bottom to top.
    pub fn stack(&self) -> &[String] {
        &self.stack
    }
}

/// Where the player's changes to the controls live.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct InputSettings {
    /// The game's folder name in the user's config directory.
    pub app_name: String,
    /// The overrides file. `None`: `input.ron` in the config folder of `app_name`.
    pub file: Option<PathBuf>,
    /// Read the overrides file at start and save changes made by the `rebind` tool. On for
    /// a windowed game, off for headless and test runs, which must not touch the player's
    /// files; an explicit `file` is always read.
    pub persist: bool,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self { app_name: "shards".to_owned(), file: None, persist: false }
    }
}

impl InputSettings {
    /// Settings for the game folder `app_name`.
    pub fn for_game(app_name: &str) -> Self {
        Self { app_name: app_name.to_owned(), ..default() }
    }

    /// The overrides file: `file`, or `input.ron` in the user's config folder.
    pub fn path(&self) -> Option<PathBuf> {
        self.file.clone().or_else(|| config_dir().map(|dir| dir.join(&self.app_name).join("input.ron")))
    }
}

/// The user's config folder: `$XDG_CONFIG_HOME`, `~/.config`, or `%APPDATA%` on Windows.
fn config_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        return var("APPDATA");
    }
    var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|home| home.join(".config")))
}

/// What the local player's controls say this frame. For presentation (the camera, menus);
/// gameplay reads `Intent`.
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct ControlState {
    pressed: BTreeSet<String>,
    held: BTreeSet<String>,
    axes: BTreeMap<String, Vec2>,
}

impl ControlState {
    /// Went down this frame (a toggle: switched on or off).
    pub fn just_pressed(&self, action: &str) -> bool {
        self.pressed.contains(action)
    }

    /// Down now (a toggle: switched on).
    pub fn held(&self, action: &str) -> bool {
        self.held.contains(action)
    }

    /// The value of a 2D axis action (`look`, `navigate`), zero when idle. A 1D axis is in `x`.
    pub fn axis(&self, action: &str) -> Vec2 {
        self.axes.get(action).copied().unwrap_or(Vec2::ZERO)
    }
}

/// What this module has written into `Intent`, to write only differences.
#[derive(Resource, Default)]
struct Applied {
    /// The body it was written to: the controls are the player's, not a body's, and a new
    /// body has been given none of it.
    player: Option<Entity>,
    /// The movement written last.
    move_dir: Vec3,
    /// Held buttons switched on by the controls.
    held: BTreeSet<String>,
    /// Toggled actions that are on.
    toggled: BTreeSet<String>,
    /// Defaults resolved from content (or the built-in) and the file read.
    resolved: bool,
}

pub(crate) fn plugin(app: &mut App) {
    let persist = app.world().get_resource::<EngineConfig>().is_some_and(|c| !c.headless && !c.testing);
    app.register_type::<InputMap>()
        .add_content::<InputMap>()
        .init_resource::<InputContexts>()
        .insert_resource(InputSettings { persist, ..default() })
        .insert_resource(InputMap::standard())
        .insert_resource(InputDefaults(InputMap::standard()))
        .init_resource::<ControlState>()
        .init_resource::<Applied>()
        .configure_sets(PreUpdate, ControlSystems::Read.after(InputSystems))
        .add_systems(PreUpdate, (resolve_defaults, read_input).chain().in_set(ControlSystems::Read));
    tools::register(app.world_mut());
}

/// The default controls in force: from content or the built-in map, before the player's
/// changes.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct InputDefaults(pub InputMap);

/// Makes the map the defaults from content (`input_map` `default`; the built-in when the
/// game has none) plus the player's changes, and again whenever that content is edited.
fn resolve_defaults(
    mut commands: Commands,
    db: Option<Res<ContentDb>>,
    settings: Res<InputSettings>,
    mut defaults: ResMut<InputDefaults>,
    mut map: ResMut<InputMap>,
    mut applied: ResMut<Applied>,
) {
    let first = !applied.resolved;
    let changed = db.as_ref().is_some_and(|db| db.is_changed());
    if !first && !changed {
        return;
    }
    // Content that is not loaded yet changes nothing; the engine holds the simulation until it is.
    if db.as_ref().is_some_and(|db| !db.is_loaded()) {
        return;
    }
    let wanted = db.as_ref().and_then(|db| db.get::<InputMap>("default").cloned()).unwrap_or_else(InputMap::standard);
    // What the player changed, kept across a change of defaults.
    let mine = if first {
        match settings.path().filter(|_| settings.persist || settings.file.is_some()) {
            Some(path) => InputMap::load_overrides(&path).unwrap_or_else(|why| {
                let text = format!("input: {why}; using the default controls");
                commands.queue(move |world: &mut World| {
                    journal::emit(world, JournalKind::Warn, text);
                });
                InputMap::default()
            }),
            None => InputMap::default(),
        }
    } else {
        map.overrides_from(&defaults.0)
    };
    applied.resolved = true;
    if first || wanted != defaults.0 {
        let mut next = wanted.clone();
        next.apply(&mine);
        defaults.0 = wanted;
        if *map != next {
            *map = next;
        }
    }
}

/// The controls at this moment, from the devices.
struct Devices<'a> {
    keys: Option<&'a ButtonInput<KeyCode>>,
    mouse: Option<&'a ButtonInput<MouseButton>>,
    motion: Vec2,
    pads: Vec<&'a Gamepad>,
    dt: f32,
}

impl Devices<'_> {
    fn down(&self, binding: &Binding) -> bool {
        match binding {
            Binding::Key(k) => self.keys.is_some_and(|b| b.pressed(*k)),
            Binding::Mouse(m) => self.mouse.is_some_and(|b| b.pressed(*m)),
            Binding::GamepadButton(b) => self.pads.iter().any(|p| p.pressed(*b)),
            _ => false,
        }
    }

    fn just_down(&self, binding: &Binding) -> bool {
        match binding {
            Binding::Key(k) => self.keys.is_some_and(|b| b.just_pressed(*k)),
            Binding::Mouse(m) => self.mouse.is_some_and(|b| b.just_pressed(*m)),
            Binding::GamepadButton(b) => self.pads.iter().any(|p| p.just_pressed(*b)),
            _ => false,
        }
    }

    /// The value of an axis binding: 2D for sticks, keys and the mouse; a 1D axis in `x`.
    fn value(&self, binding: &Binding) -> Vec2 {
        let key = |k: &KeyCode| self.keys.is_some_and(|b| b.pressed(*k)) as i32 as f32;
        match binding {
            Binding::KeyAxis { neg, pos } => Vec2::new(key(pos) - key(neg), 0.0),
            Binding::Keys2D { up, down, left, right } => {
                Vec2::new(key(right) - key(left), key(up) - key(down)).normalize_or_zero()
            }
            Binding::GamepadAxis { axis, deadzone, scale } => {
                let raw = self.pads.iter().filter_map(|p| p.get(*axis)).fold(0.0, |best: f32, v| if v.abs() > best.abs() { v } else { best });
                let live = if raw.abs() < *deadzone { 0.0 } else { raw };
                Vec2::new(live * scale, 0.0)
            }
            Binding::Stick { stick, deadzone, scale } => {
                let raw = self
                    .pads
                    .iter()
                    .map(|p| match stick {
                        Stick::Left => p.left_stick(),
                        Stick::Right => p.right_stick(),
                    })
                    .fold(Vec2::ZERO, |best, v| if v.length_squared() > best.length_squared() { v } else { best });
                let len = raw.length();
                if len <= *deadzone { Vec2::ZERO } else { raw / len * ((len - deadzone) / (1.0 - deadzone).max(1e-3)).min(1.0) * *scale }
            }
            // Mouse deltas are per frame already; a stick's value is a rate, so the look
            // axis multiplies sticks by the frame time (see `read_input`).
            Binding::MouseMotion { scale } => Vec2::new(self.motion.x, -self.motion.y) * *scale,
            Binding::Key(_) | Binding::Mouse(_) | Binding::GamepadButton(_) => Vec2::ZERO,
        }
    }
}

/// Lets a body that is no longer the local player go of what the controls made it do: its
/// walk and its held buttons, as far as they are still what was written (an agent's own
/// steering of it is left alone).
fn let_go(intent: &mut Mut<Intent>, applied: &Applied) {
    if applied.move_dir != Vec3::ZERO && intent.move_dir == applied.move_dir {
        intent.move_dir = Vec3::ZERO;
    }
    for name in &applied.held {
        if intent.is_held(name) {
            intent.set_held(name, false);
        }
    }
}

/// `PreUpdate`: reads the devices through the top context of the map and writes what
/// changed into the local player's `Intent`.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn read_input(
    time: Res<Time<Real>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    motion: Option<Res<AccumulatedMouseMotion>>,
    gamepads: Query<(Entity, &Gamepad)>,
    cameras: Query<&ThirdPersonCamera>,
    map: Res<InputMap>,
    contexts: Res<InputContexts>,
    mut state: ResMut<ControlState>,
    mut applied: ResMut<Applied>,
    mut players: Query<(Entity, &mut Intent), With<LocalPlayer>>,
    mut others: Query<&mut Intent, Without<LocalPlayer>>,
) {
    let mut pads: Vec<(Entity, &Gamepad)> = gamepads.iter().collect();
    pads.sort_by_key(|(e, _)| *e);
    let devices = Devices {
        keys: keys.as_deref(),
        mouse: mouse.as_deref(),
        motion: motion.map_or(Vec2::ZERO, |m| m.delta),
        pads: pads.into_iter().map(|(_, g)| g).collect(),
        dt: time.delta_secs(),
    };

    // Evaluate the top context's actions.
    let drives_intent = map.context(contexts.top()).is_none_or(|c| c.drives_intent);
    let mut frame = ControlState::default();
    let mut presses: Vec<(String, bool)> = Vec::new();
    let mut desired_held: BTreeSet<String> = BTreeSet::new();
    let mut move_axis = Vec2::ZERO;
    if let Some(context) = map.context(contexts.top()) {
        for action in &context.actions {
            let buttons = || action.bindings.iter().filter(|b| !b.is_axis());
            let (down, just) = (buttons().any(|b| devices.down(b)), buttons().any(|b| devices.just_down(b)));
            let axis = action.bindings.iter().filter(|b| b.is_axis()).fold(Vec2::ZERO, |sum, b| {
                // A stick turns the camera at a rate: scale it by the frame; the mouse is already a delta.
                let v = devices.value(b);
                sum + if matches!(b, Binding::Stick { .. }) && action.action == "look" { v * devices.dt } else { v }
            });
            if axis != Vec2::ZERO {
                frame.axes.insert(action.action.clone(), axis);
            }
            if action.action == "move" {
                move_axis = axis.clamp_length_max(1.0);
            }
            match action.mode {
                BindMode::Press => {
                    if just {
                        frame.pressed.insert(action.action.clone());
                        presses.push((action.action.clone(), action.directional));
                    }
                    if down {
                        frame.held.insert(action.action.clone());
                    }
                }
                BindMode::Hold => {
                    if just {
                        frame.pressed.insert(action.action.clone());
                    }
                    if down {
                        frame.held.insert(action.action.clone());
                        desired_held.insert(action.action.clone());
                    }
                }
                BindMode::Toggle => {
                    if just {
                        frame.pressed.insert(action.action.clone());
                        if !applied.toggled.remove(&action.action) {
                            applied.toggled.insert(action.action.clone());
                        }
                    }
                    if applied.toggled.contains(&action.action) {
                        frame.held.insert(action.action.clone());
                        desired_held.insert(action.action.clone());
                    }
                }
            }
        }
    }
    if *state != frame {
        *state = frame;
    }
    if !drives_intent {
        // A menu: the UI reads `ControlState`; the character hears nothing, and lets go of what it held.
        presses.clear();
        desired_held.clear();
        move_axis = Vec2::ZERO;
    }

    // Nobody to hear it: nothing counts as written, so a body that appears later (or
    // takes over the marker) gets everything that is down.
    let Ok((player, mut intent)) = players.single_mut() else {
        return;
    };
    if applied.player != Some(player) {
        if let Some(old) = applied.player.replace(player)
            && let Ok(mut old) = others.get_mut(old)
        {
            let_go(&mut old, &applied);
        }
        applied.move_dir = Vec3::ZERO;
        applied.held.clear();
    }

    // Menu closes, an action removed from the map: whatever we held and no longer should.
    let release: Vec<String> = applied.held.difference(&desired_held).cloned().collect();
    let hold: Vec<String> = desired_held.difference(&applied.held).cloned().collect();
    let yaw = cameras.iter().next().map_or(0.0, |c| c.yaw);
    let world_move = Quat::from_rotation_y(yaw) * Vec3::new(move_axis.x, 0.0, -move_axis.y);
    let moved = world_move != applied.move_dir;
    if !moved && release.is_empty() && hold.is_empty() && presses.is_empty() {
        return;
    }
    applied.move_dir = world_move;
    applied.held = desired_held;
    if moved {
        if world_move != Vec3::ZERO {
            // The human takes over: drop whatever an agent ordered.
            intent.orders.clear();
        }
        intent.move_dir = world_move;
    }
    for name in release {
        intent.set_held(&name, false);
    }
    for name in hold {
        intent.set_held(&name, true);
    }
    for (name, directional) in presses {
        let dir = (directional && world_move != Vec3::ZERO).then_some(world_move);
        intent.pressed.push(Press { input: name, dir });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_stack_keeps_its_bottom() {
        let mut contexts = InputContexts::default();
        assert_eq!(contexts.top(), "gameplay");
        contexts.push("menu");
        assert_eq!(contexts.top(), "menu");
        assert_eq!(contexts.pop().as_deref(), Some("menu"));
        assert_eq!(contexts.pop(), None, "the bottom context stays");
        contexts.set("aim");
        assert_eq!(contexts.stack(), ["aim"]);
    }

    #[test]
    fn the_overrides_file_is_in_the_game_folder() {
        let settings = InputSettings { file: Some("/tmp/x/input.ron".into()), ..default() };
        assert_eq!(settings.path(), Some(PathBuf::from("/tmp/x/input.ron")));
        let named = InputSettings::for_game("my-game").path();
        assert!(named.is_none_or(|p| p.ends_with("my-game/input.ron")));
    }
}
