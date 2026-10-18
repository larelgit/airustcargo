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
//! [`InputMap::standard`]; the player's changes are the `controls` settings
//! ([`ControlSettings`], saved by Bevy's settings framework in `settings.toml`), only what
//! differs from the defaults, so new default actions still reach old files (see [`controls`]).
//! The overrides file of earlier versions (`input.ron`, see [`InputSettings`]) is imported once.
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

pub mod controls;
mod map;
mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use ai_core::{journal, JournalKind};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::input::InputSystems;
use bevy::prelude::*;
use shards_content::{ContentAppExt, ContentDb};
use shards_core::{Intent, LocalPlayer, Press};
use shards_settings::{AccessibilitySettings, ControlSettings, SettingsAppExt, SettingsStorage};

pub use controls::ControlSync;
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

/// Where the player's controls were kept before they became settings (E8.6): the overrides
/// file `input.ron`. A game that shipped with it turns [`import`](Self::import) on and names
/// its folder, and the first run on the new version imports the player's changes into the
/// `controls` settings, once: the file is renamed to `input.ron.imported` when the settings
/// file really has them. Nothing is written to the old file any more.
///
/// **Off by default, so a new game never reaches into a shared folder.** The default
/// `app_name` is `shards`, the folder the engine's own sandbox used: a game that left it must not
/// take the sandbox's file (and rename it away from the sandbox). Only the platform's own
/// config folder is searched, and only when the settings are kept in the platform's own folder
/// too (never for `--settings-dir`, tests, or a headless run): an explicit [`file`](Self::file)
/// is always read.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct InputSettings {
    /// The game's folder name in the user's config directory.
    pub app_name: String,
    /// The overrides file. `None`: `input.ron` in the config folder of `app_name` when
    /// [`import`](Self::import) is on.
    pub file: Option<PathBuf>,
    /// Look for `input.ron` in the user's config folder of `app_name` (see the type's docs).
    pub import: bool,
    /// Where the user's config folder is, for tests and tools that must not touch the real one.
    /// `None`: `$XDG_CONFIG_HOME`, `~/.config`, or `%APPDATA%` on Windows.
    pub config_dir: Option<PathBuf>,
}

impl Default for InputSettings {
    fn default() -> Self {
        Self { app_name: "shards".to_owned(), file: None, import: false, config_dir: None }
    }
}

impl InputSettings {
    /// A game that shipped with `input.ron` in the config folder `app_name`: it is imported.
    pub fn for_game(app_name: &str) -> Self {
        Self { app_name: app_name.to_owned(), import: true, ..default() }
    }

    /// The file to import from: `file` when there is one (always), else `input.ron` in the
    /// config folder of `app_name` when `import` is on and `platform_store` (the settings are in
    /// the platform's own folder, so this is the same user's own game).
    pub fn path(&self, platform_store: bool) -> Option<PathBuf> {
        if let Some(file) = &self.file {
            return Some(file.clone());
        }
        let folder = self.config_dir.clone().or_else(config_dir)?;
        (self.import && platform_store).then(|| folder.join(&self.app_name).join("input.ron"))
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
    look_pointer_px: Vec2,
    look_stick: Vec2,
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

    /// The mouse's part of the `look` axis as the raw motion of this frame in pixels (right and
    /// up are positive), before the binding's scale and the player's sensitivity. What a flick
    /// is measured in.
    pub fn look_pointer_px(&self) -> Vec2 {
        self.look_pointer_px
    }

    /// The sticks' part of the `look` axis as a deflection (right and up are positive, length
    /// up to 1, after the dead zone), before the binding's scale and the frame time.
    pub fn look_stick(&self) -> Vec2 {
        self.look_stick
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
    app.register_type::<InputMap>()
        .add_content::<InputMap>()
        .init_resource::<InputContexts>()
        .init_resource::<InputSettings>()
        .insert_resource(InputMap::standard())
        .insert_resource(InputDefaults(InputMap::standard()))
        .init_resource::<ControlState>()
        .init_resource::<ControlSync>()
        .init_resource::<Applied>()
        // `settings_set` refuses what `rebind` refuses: a binding that does not read, an action that
        // does not exist, an input another action has.
        .add_settings_check::<ControlSettings>(controls::check)
        .add_settings_world_check::<ControlSettings>(controls::check_known)
        .configure_sets(PreUpdate, ControlSystems::Read.after(InputSystems))
        .add_systems(PreUpdate, (resolve_defaults, read_input).chain().in_set(ControlSystems::Read));
    tools::register(app.world_mut());
}

/// The default controls in force: from content or the built-in map, before the player's
/// changes.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct InputDefaults(pub InputMap);

/// Keeps the map the defaults from content (`input_map` `default`; the built-in when the game
/// has none) plus the player's changes from the `controls` settings: it is built once content
/// is there, again when the content is edited or the bindings and modes in the settings change,
/// and a change made to the map itself (`rebind`, a game's system) is written back into the
/// settings. What a game added to the map beyond the defaults (a context, an action) survives a
/// rebuild; the settings keep only differences on default actions.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn resolve_defaults(
    mut commands: Commands,
    db: Option<Res<ContentDb>>,
    legacy: Res<InputSettings>,
    storage: Option<Res<SettingsStorage>>,
    mut controls: ResMut<ControlSettings>,
    mut defaults: ResMut<InputDefaults>,
    mut map: ResMut<InputMap>,
    mut applied: ResMut<Applied>,
    mut sync: ResMut<ControlSync>,
) {
    let first = !applied.resolved;
    // Content that is not loaded yet changes nothing; the engine holds the simulation until it is.
    if db.as_ref().is_some_and(|db| !db.is_loaded()) {
        return;
    }
    let mut defaults_changed = false;
    if first || db.as_ref().is_some_and(|db| db.is_changed()) {
        let wanted = db.as_ref().and_then(|db| db.get::<InputMap>("default").cloned()).unwrap_or_else(InputMap::standard);
        if first || wanted != defaults.0 {
            defaults.0 = wanted;
            defaults_changed = true;
        }
    }

    if first {
        applied.resolved = true;
        if let Some(imported) = import_legacy_file(&mut commands, &legacy, storage.as_deref(), &defaults.0, &controls) {
            *controls = imported;
        }
        let (desired, mut lines) = controls::decode(&defaults.0, &controls);
        lines = lines.into_iter().map(|problem| format!("settings: {problem}; using the default control")).collect();
        lines.extend(controls::new_clashes(&desired, &defaults.0));
        say(&mut commands, sync.unsaid(lines));
        if *map != desired {
            *map = desired;
        }
        sync.agree(&map, &defaults.0, &controls);
        return;
    }

    if map.is_changed() && sync.agreed.as_ref() != Some(&*map) {
        // Someone edited the map: what differs from the defaults is the player's, so it is a setting.
        let mut next = controls.clone();
        if controls::encode(&map, &defaults.0, &mut next) {
            *controls = next;
            shards_settings::save_soon(&mut commands);
        }
        sync.agree(&map, &defaults.0, &controls);
    } else if defaults_changed || (controls.is_changed() && sync.settings_moved(&controls)) {
        let (mut desired, problems) = controls::decode(&defaults.0, &controls);
        desired.apply(&sync.extras(&map, &defaults.0));
        let mut lines: Vec<String> = problems.into_iter().map(|problem| format!("settings: {problem}; using the default control")).collect();
        lines.extend(controls::new_clashes(&desired, &defaults.0));
        say(&mut commands, sync.unsaid(lines));
        if *map != desired {
            *map = desired;
        }
        sync.agree(&map, &defaults.0, &controls);
    }
}

/// Journals `lines` as warnings.
fn say(commands: &mut Commands, lines: Vec<String>) {
    for text in lines {
        commands.queue(move |world: &mut World| {
            journal::emit(world, JournalKind::Warn, text);
        });
    }
}

/// The controls of the overrides file earlier versions kept (`input.ron`) as the settings they
/// are now, once: only when the settings hold no controls yet and [`InputSettings::path`] finds a
/// file. Settings kept in a file get the controls written right away; the old file is renamed to
/// `input.ron.imported` only when the settings file is read back and holds every imported entry,
/// otherwise it stays where it is to be imported again, and the journal says so. `None`: nothing
/// to import.
fn import_legacy_file(
    commands: &mut Commands,
    legacy: &InputSettings,
    storage: Option<&SettingsStorage>,
    defaults: &InputMap,
    controls: &ControlSettings,
) -> Option<ControlSettings> {
    let pristine = controls.bindings.is_empty() && controls.hold_or_toggle.is_empty();
    let path = legacy.path(storage.is_some_and(|s| s.platform_default)).filter(|p| pristine && p.is_file())?;
    let overrides = match InputMap::load_overrides(&path) {
        Ok(overrides) => overrides,
        Err(why) => {
            say(commands, vec![format!("input: {why}; using the default controls")]);
            return None;
        }
    };
    let mut mine = defaults.clone();
    mine.apply(&overrides);
    let mut imported = controls.clone();
    controls::encode(&mine, defaults, &mut imported);
    let keeps_file = storage.is_some_and(|s| s.file.is_some());
    let (bindings, modes): (Vec<String>, Vec<String>) = (imported.bindings.keys().cloned().collect(), imported.hold_or_toggle.keys().cloned().collect());
    commands.queue(move |world: &mut World| {
        let from = path.display();
        if !keeps_file {
            let text = format!("settings: imported the controls from {from} into controls.bindings (not saved: this run keeps no settings file; {from} stays)");
            journal::emit(world, JournalKind::Event, text);
            return;
        }
        // The old file goes aside only once the settings file is read back with what was in it.
        match written(world, &bindings, &modes) {
            Ok(()) => {
                let _ = std::fs::rename(&path, path.with_extension("ron.imported"));
                journal::emit(world, JournalKind::Event, format!("settings: imported the controls from {from} into controls.bindings ({from} is now input.ron.imported)"));
            }
            Err(why) => {
                let text = format!("settings: could not write the imported controls ({why}); {from} stays where it is and is imported again next time");
                journal::emit(world, JournalKind::Warn, text);
            }
        }
    });
    Some(imported)
}

/// Writes the settings now and reads the file back: `Ok` when it holds every entry of the
/// imported `bindings` and `modes` (nothing imported needs no file).
fn written(world: &mut World, bindings: &[String], modes: &[String]) -> Result<(), String> {
    if bindings.is_empty() && modes.is_empty() {
        return Ok(());
    }
    shards_settings::flush_checked(world)?;
    let holds = |field: &str, wanted: &[String]| {
        wanted.is_empty() || shards_settings::stored_entries(world, "controls", field).is_some_and(|have| wanted.iter().all(|k| have.contains(k)))
    };
    if holds("bindings", bindings) && holds("hold_or_toggle", modes) { Ok(()) } else { Err("the file has not got the controls".to_owned()) }
}

/// The controls at this moment, from the devices.
struct Devices<'a> {
    keys: Option<&'a ButtonInput<KeyCode>>,
    mouse: Option<&'a ButtonInput<MouseButton>>,
    motion: Vec2,
    scroll: Vec2,
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
            Binding::Buttons2D { up, down, left, right } => {
                let pad = |b: &GamepadButton| self.pads.iter().any(|p| p.pressed(*b)) as i32 as f32;
                Vec2::new(pad(right) - pad(left), pad(up) - pad(down)).normalize_or_zero()
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
            Binding::MouseWheel { scale } => self.scroll * *scale,
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
    scroll: Option<Res<AccumulatedMouseScroll>>,
    gamepads: Query<(Entity, &Gamepad)>,
    cameras: Query<&ThirdPersonCamera>,
    map: Res<InputMap>,
    contexts: Res<InputContexts>,
    accessibility: Option<Res<AccessibilitySettings>>,
    mut state: ResMut<ControlState>,
    mut applied: ResMut<Applied>,
    mut players: Query<(Entity, &mut Intent), With<LocalPlayer>>,
    mut others: Query<&mut Intent, Without<LocalPlayer>>,
) {
    let hold_to_toggle = accessibility.is_some_and(|a| a.hold_to_toggle);
    let mut pads: Vec<(Entity, &Gamepad)> = gamepads.iter().collect();
    pads.sort_by_key(|(e, _)| *e);
    let devices = Devices {
        keys: keys.as_deref(),
        mouse: mouse.as_deref(),
        motion: motion.map_or(Vec2::ZERO, |m| m.delta),
        scroll: scroll.map_or(Vec2::ZERO, |s| s.delta),
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
            if action.action == "look" {
                // What a flick is measured in: the mouse's pixels and the stick's deflection, each before its scale.
                for binding in action.bindings.iter().filter(|b| b.is_axis()) {
                    match binding {
                        Binding::MouseMotion { .. } => frame.look_pointer_px += Vec2::new(devices.motion.x, -devices.motion.y),
                        Binding::Stick { scale, .. } if *scale != 0.0 => {
                            // Divided by the size of the scale, not the scale: an inverted binding (negative scale) keeps its flipped side, as the look axis does.
                            let deflection = devices.value(binding) / scale.abs();
                            if deflection.length_squared() > frame.look_stick.length_squared() {
                                frame.look_stick = deflection;
                            }
                        }
                        _ => {}
                    }
                }
            }
            if action.action == "move" {
                move_axis = axis.clamp_length_max(1.0);
            }
            // "Hold to toggle" (accessibility): a held button is pressed once to hold, again to let go.
            let mode = if hold_to_toggle && action.mode == BindMode::Hold { BindMode::Toggle } else { action.mode };
            if mode != BindMode::Toggle {
                applied.toggled.remove(&action.action);
            }
            match mode {
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
    fn the_old_overrides_file_is_looked_for_only_where_a_game_asked() {
        let folder = PathBuf::from("/tmp/c");
        // A file somebody named is always the one.
        let named = InputSettings { file: Some("/tmp/x/input.ron".into()), ..default() };
        assert_eq!((named.path(false), named.path(true)), (Some("/tmp/x/input.ron".into()), Some("/tmp/x/input.ron".into())));
        // A new game: nothing is looked for in the shared `shards` folder, whatever is in it.
        let new_game = InputSettings { config_dir: Some(folder.clone()), ..default() };
        assert_eq!((new_game.path(true), new_game.path(false)), (None, None));
        // A game that shipped with `input.ron` asks, and only the platform's own settings folder is its own.
        let shipped = InputSettings { config_dir: Some(folder.clone()), ..InputSettings::for_game("my-game") };
        assert_eq!(shipped.path(true), Some(folder.join("my-game/input.ron")));
        assert_eq!(shipped.path(false), None, "a folder given with --settings-dir, a test, a headless run: not the user's");
    }
}
