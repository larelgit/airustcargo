//! Open screens: the stack, the widget tree, focus, navigation and what pressing does.
//!
//! A screen is an entity tree spawned from its [`ScreenDef`]: the root carries [`UiScreen`]
//! and every widget, the root too, a [`UiWidget`], children under parents with `ChildOf` in
//! the order content lists them. Nothing here has an `AiId`, so opening a menu never shifts an
//! entity id, and nothing here is simulation: screens are this machine's, outside snapshots and
//! the replay hash. The calls that change them are tools, so a replay presses the same buttons.

use std::collections::BTreeMap;

use ai_core::journal::{self, JournalKind};
use bevy::app::AppExit;
use bevy::prelude::*;
use serde_json::json;
use shards_content::ContentDb;
use shards_core::{NetRole, PauseState};

use crate::bind::{live_value, read_source, UiActions, UiPress};
use crate::def::{assign_id, ButtonAction, CompareRow, Layer, Layout, ScreenDef, WidgetDef};

/// The source binding widgets read, which the engine gives from its input map (the `shards`
/// crate): `input.bindings:<context>.<action>` is the action's inputs as [`UiValue::Bindings`]
/// (or [`UiValue::Text`] saying why there are none: no such action);
/// `input.bindings:<context>.<action> <input>` is that one input as a player reads it, with the
/// other action of the context that reads it (`None`: the text is not an input).
pub const BINDINGS_SOURCE: &str = "input.bindings";

/// What a binding widget says while it waits, unless content says otherwise.
pub const DEFAULT_PROMPT: &str = "Press a key or button… (Esc cancels)";

/// One widget of an open screen. The root of a screen is one too: a panel named like the screen.
#[derive(Component, Clone, Debug, PartialEq)]
pub struct UiWidget {
    /// `screen/id` (`pause/resume`); the screen's name for its root.
    pub path: String,
    pub kind: WidgetKind,
    /// What it says: a button's label, a panel's title.
    pub label: String,
    /// Its value as last drawn: fixed by content, set by a player, or read from its source.
    /// Agents read the live one ([`live_value`]).
    pub value: UiValue,
    /// The source it shows (`you.health`), if any.
    pub bind: Option<String>,
    /// The player setting it shows and changes (`accessibility.text_scale`), if any.
    pub setting: Option<String>,
    pub enabled: bool,
}

/// What a widget is, with what only its kind has.
#[derive(Clone, Debug, PartialEq)]
pub enum WidgetKind {
    Panel { layout: Layout },
    List,
    Grid { columns: u32 },
    /// Its children are its pages; [`UiValue::Page`] is the one shown.
    Tabs,
    Text,
    /// A picture under the asset folder, drawn `width` × `height` logical pixels (one of them: the
    /// picture's proportions give the other).
    Image { path: String, width: Option<f32>, height: Option<f32> },
    Hint,
    Button { action: String },
    Bar { color: Option<String> },
    Cooldown,
    Compass,
    Allies,
    Compare,
    Slider { min: f32, max: f32, step: f32 },
    Toggle,
    /// `labels`: what each option says, in the language played (empty: the options themselves).
    Choice { options: Vec<String>, labels: Vec<String> },
    /// The inputs of `action` in input context `context` (only slot `slot`, with one); `prompt` is
    /// what it says while it waits for an input (in the language played), `taken` the message
    /// (`@id`) that says an input is taken.
    Binding { context: String, action: String, slot: Option<usize>, prompt: String, taken: Option<String> },
}

impl WidgetKind {
    /// The word for it (`button`).
    pub fn name(&self) -> &'static str {
        match self {
            WidgetKind::Panel { .. } => "panel",
            WidgetKind::List => "list",
            WidgetKind::Grid { .. } => "grid",
            WidgetKind::Tabs => "tabs",
            WidgetKind::Text => "text",
            WidgetKind::Image { .. } => "image",
            WidgetKind::Hint => "hint",
            WidgetKind::Button { .. } => "button",
            WidgetKind::Bar { .. } => "bar",
            WidgetKind::Cooldown => "cooldown",
            WidgetKind::Compass => "compass",
            WidgetKind::Allies => "allies",
            WidgetKind::Compare => "compare",
            WidgetKind::Slider { .. } => "slider",
            WidgetKind::Toggle => "toggle",
            WidgetKind::Choice { .. } => "choice",
            WidgetKind::Binding { .. } => "binding",
        }
    }

    /// Whether focus can rest on it: the widgets a player presses or changes.
    pub fn focusable(&self) -> bool {
        matches!(
            self,
            WidgetKind::Button { .. } | WidgetKind::Slider { .. } | WidgetKind::Toggle | WidgetKind::Choice { .. } | WidgetKind::Tabs | WidgetKind::Binding { .. }
        )
    }

    /// The inputs a binding widget shows of `inputs`: its slot's, or all of them.
    pub fn shown_inputs<'a>(&self, inputs: &'a [BoundInput]) -> &'a [BoundInput] {
        match self {
            WidgetKind::Binding { slot: Some(slot), .. } => inputs.get(*slot..=*slot).unwrap_or(&[]),
            _ => inputs,
        }
    }

    /// Whether it holds other widgets.
    pub fn container(&self) -> bool {
        matches!(self, WidgetKind::Panel { .. } | WidgetKind::List | WidgetKind::Grid { .. } | WidgetKind::Tabs)
    }

    /// What option `value` of a choice says: its label, or itself.
    pub fn option_label<'a>(&'a self, value: &'a str) -> &'a str {
        match self {
            WidgetKind::Choice { options, labels } => options.iter().position(|o| o == value).and_then(|i| labels.get(i)).map_or(value, String::as_str),
            _ => value,
        }
    }
}

/// A widget's value.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum UiValue {
    #[default]
    None,
    Text(String),
    /// A bar: `value` out of `max`.
    Ratio { value: f32, max: f32 },
    /// Seconds before an action is ready again (0: ready).
    Cooldown { left: f32 },
    /// Where the viewer faces, degrees clockwise from north, and marks around it.
    Heading { deg: f32, marks: Vec<Mark> },
    Allies(Vec<Ally>),
    Compare(Vec<CompareRow>),
    Number(f32),
    Flag(bool),
    Choice(String),
    /// The page tabs show, by index.
    Page(usize),
    /// An action's inputs (source [`BINDINGS_SOURCE`]).
    Bindings(BoundInputs),
}

/// An action's inputs, as a binding widget shows and changes them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundInputs {
    /// Slot 0 (the primary input) first.
    pub inputs: Vec<BoundInput>,
    /// The game's default inputs of the action, as text: going back to them removes the player's
    /// setting instead of saving a copy of the defaults.
    pub defaults: Vec<String>,
}

/// One input of an action.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoundInput {
    /// As settings and `ui_set` write it: `Key(KeyK)`, `GamepadButton(South)`.
    pub text: String,
    /// As a player reads it: `F`, `Mouse Right`, `Pad South`.
    pub label: String,
    /// A gamepad's: a binding widget without a slot replaces the input of the same kind of device.
    pub gamepad: bool,
    /// An axis (a stick, WASD), not a button: binding widgets take buttons only.
    pub axis: bool,
    /// For an input asked about: the other action of the context that reads it.
    pub taken_by: Option<String>,
}

/// `Space · Pad South`, `none` for no input: what a binding widget shows, for agents and the journal.
pub fn inputs_text(inputs: &[BoundInput]) -> String {
    if inputs.is_empty() {
        return "none".to_owned();
    }
    inputs.iter().map(|i| i.label.as_str()).collect::<Vec<_>>().join(" · ")
}

/// A place on a compass.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub label: String,
    /// Bearing from the viewer, degrees clockwise from north.
    pub bearing: f32,
    pub distance: f32,
}

/// Another player, in an ally frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Ally {
    pub name: String,
    pub health: f32,
    pub max: f32,
    pub down: bool,
}

/// The root of an open screen.
#[derive(Component, Clone, Debug)]
pub struct UiScreen {
    pub name: String,
    /// The content it was built from (a hot edit builds it again).
    pub def: ScreenDef,
    /// The focused widget's path, when the screen takes input.
    pub focus: Option<String>,
    /// The binding widget that waits for an input (its path), while one does.
    pub capture: Option<String>,
    /// What the last refused binding said, and the binding widget that says it: `(path, text)`.
    pub note: Option<(String, String)>,
}

/// The open screens, in the order they opened.
#[derive(Resource, Default, Debug)]
pub struct UiStack {
    open: Vec<(String, Entity)>,
}

impl UiStack {
    /// Open screens, in the order they opened.
    pub fn screens(&self) -> impl Iterator<Item = (&str, Entity)> {
        self.open.iter().map(|(n, e)| (n.as_str(), *e))
    }

    /// The root of open screen `name`.
    pub fn get(&self, name: &str) -> Option<Entity> {
        self.open.iter().find(|(n, _)| n == name).map(|(_, e)| *e)
    }

    pub fn is_open(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
}

/// The UI paused the game (`PauseState::Paused`), so closing its last pausing screen resumes.
#[derive(Resource, Default)]
pub(crate) struct UiPaused(pub bool);

/// Someone pressed a button (any action, after it was done).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct UiPressed {
    pub widget: String,
    pub action: String,
}

/// A slider, toggle, choice, tabs or binding changed value (`value` as `ui_set` takes it; a
/// binding's: the inputs a player reads).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct UiChanged {
    pub widget: String,
    pub value: String,
}

/// Navigation moved the focus to another widget (`up`, `next`, …; not a press or a change).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct UiFocusMoved {
    pub widget: String,
}

/// A screen opened or closed.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct UiScreenChanged {
    pub screen: String,
    pub open: bool,
}

fn widget(world: &World, e: Entity) -> Option<&UiWidget> {
    world.get::<UiWidget>(e)
}

/// The widgets right under `e`, in order (not the parts that only draw it).
pub fn widget_children(world: &World, e: Entity) -> Vec<Entity> {
    world.get::<Children>(e).map(|c| c.iter().filter(|c| world.get::<UiWidget>(*c).is_some()).collect()).unwrap_or_default()
}

/// The screen root `e` belongs to.
pub fn screen_of(world: &World, mut e: Entity) -> Option<Entity> {
    loop {
        if world.get::<UiScreen>(e).is_some() {
            return Some(e);
        }
        e = world.get::<ChildOf>(e)?.parent();
    }
}

/// Whether `e` is on a shown page: no tabs above it show another page.
pub fn on_shown_page(world: &World, mut e: Entity) -> bool {
    while let Some(parent) = world.get::<ChildOf>(e).map(ChildOf::parent) {
        if let Some(UiWidget { kind: WidgetKind::Tabs, value, .. }) = widget(world, parent) {
            let page = if let UiValue::Page(i) = value { *i } else { 0 };
            if widget_children(world, parent).iter().position(|c| *c == e) != Some(page) {
                return false;
            }
        }
        e = parent;
    }
    true
}

/// The widget at `path` on an open screen.
pub fn find(world: &World, path: &str) -> Option<Entity> {
    let screen = path.split('/').next()?;
    let root = world.get_resource::<UiStack>()?.get(screen)?;
    let mut todo = vec![root];
    while let Some(e) = todo.pop() {
        if widget(world, e).is_some_and(|w| w.path == path) {
            return Some(e);
        }
        todo.extend(widget_children(world, e).into_iter().rev());
    }
    None
}

/// Every widget of the tree under `root` (itself first), depth first, with its depth.
pub fn walk(world: &World, root: Entity) -> Vec<(Entity, usize)> {
    let mut out = Vec::new();
    let mut todo = vec![(root, 0)];
    while let Some((e, depth)) = todo.pop() {
        out.push((e, depth));
        todo.extend(widget_children(world, e).into_iter().rev().map(|c| (c, depth + 1)));
    }
    out
}

/// What focus can rest on in a screen, in order: enabled widgets a player presses or
/// changes, on shown pages.
pub fn focusables(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut todo = vec![root];
    while let Some(e) = todo.pop() {
        let Some(w) = widget(world, e) else { continue };
        if w.kind.focusable() && w.enabled {
            out.push(e);
        }
        let kids = widget_children(world, e);
        let shown: Vec<Entity> = match (&w.kind, &w.value) {
            (WidgetKind::Tabs, UiValue::Page(i)) => kids.get(*i).copied().into_iter().collect(),
            (WidgetKind::Tabs, _) => kids.first().copied().into_iter().collect(),
            _ => kids,
        };
        todo.extend(shown.into_iter().rev());
    }
    out
}

/// The screen that takes input: the newest of the highest layer above the HUD.
pub fn top_screen(world: &World) -> Option<Entity> {
    let stack = world.get_resource::<UiStack>()?;
    let layer = |e: Entity| world.get::<UiScreen>(e).map_or(Layer::Hud, |s| s.def.layer);
    stack.screens().map(|(_, e)| e).filter(|e| layer(*e) != Layer::Hud).enumerate().max_by_key(|(i, e)| (layer(*e), *i)).map(|(_, e)| e)
}

/// The focused widget of the screen that takes input.
pub fn focused(world: &World) -> Option<Entity> {
    let top = top_screen(world)?;
    world.get::<UiScreen>(top)?.focus.as_deref().and_then(|p| find(world, p))
}

/// Whether a pausing screen is open.
fn wants_pause(world: &World) -> bool {
    let Some(stack) = world.get_resource::<UiStack>() else { return false };
    stack.screens().any(|(_, e)| world.get::<UiScreen>(e).is_some_and(|s| s.def.pauses))
}

/// Single player: `PauseState::Paused` while a pausing screen is open. In co-op the world goes on.
fn sync_pause(world: &mut World) {
    if world.get_resource::<NetRole>().is_some_and(|r| *r != NetRole::Single) {
        return;
    }
    let Some(state) = world.get_resource::<State<PauseState>>().map(|s| *s.get()) else { return };
    let wants = wants_pause(world);
    let ui_paused = world.resource::<UiPaused>().0;
    if wants && !ui_paused {
        world.resource_mut::<UiPaused>().0 = true;
        if state == PauseState::Running {
            world.resource_mut::<NextState<PauseState>>().set(PauseState::Paused);
        }
    } else if !wants && ui_paused {
        world.resource_mut::<UiPaused>().0 = false;
        world.resource_mut::<NextState<PauseState>>().set(PauseState::Running);
    }
}

/// `OnExit(PauseState::Paused)`: the game runs again (an agent's `resume`, a region change), so
/// the screens that paused it close.
pub(crate) fn close_pausing_screens(world: &mut World) {
    world.resource_mut::<UiPaused>().0 = false;
    let pausing: Vec<Entity> = world
        .resource::<UiStack>()
        .screens()
        .map(|(_, e)| e)
        .filter(|e| world.get::<UiScreen>(*e).is_some_and(|s| s.def.pauses))
        .collect();
    for root in pausing {
        close(world, root);
    }
}

/// Opens screen `name` from content on top. An open screen stays as it is.
pub fn open(world: &mut World, name: &str) -> Result<String, String> {
    if let Some(root) = world.resource::<UiStack>().get(name) {
        return Ok(format!("{name} is open already{}", focus_note(world, root)));
    }
    let def = screen_def(world, name)?;
    let root = spawn_screen(world, name, &def);
    // A message the language lacks is in the journal before the call returns.
    shards_locale::journal_notes(world);
    world.resource_mut::<UiStack>().open.push((name.to_owned(), root));
    ensure_focus(world, root);
    if def.layer != Layer::Hud {
        journal::log(world, JournalKind::Local, format!("ui: opened {name}"));
    }
    world.trigger(UiScreenChanged { screen: name.to_owned(), open: true });
    sync_pause(world);
    let pauses = if def.pauses { ", pauses the game" } else { "" };
    Ok(format!("opened {name} ({}{pauses}){}", def.layer.name(), focus_note(world, root)))
}

fn focus_note(world: &World, root: Entity) -> String {
    world.get::<UiScreen>(root).and_then(|s| s.focus.clone()).map(|f| format!(" · focus {f}")).unwrap_or_default()
}

/// Screen `name` as content has it, or what screens there are.
pub fn screen_def(world: &World, name: &str) -> Result<ScreenDef, String> {
    let db = world.get_resource::<ContentDb>().ok_or("no content database")?;
    if let Some(def) = db.get::<ScreenDef>(name) {
        return Ok(def.clone());
    }
    let names: Vec<&str> = db.ids::<ScreenDef>().collect();
    Err(if names.is_empty() {
        format!("no screen `{name}`: this game has no screens (content `*.screen.ron`)")
    } else {
        let hint = closest(name, &names).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        format!("no screen `{name}`{hint}; screens: {}", names.join(", "))
    })
}

fn closest<'a>(wanted: &str, names: &[&'a str]) -> Option<&'a str> {
    names.iter().copied().filter(|n| n.contains(wanted) || wanted.contains(n)).min_by_key(|n| n.len().abs_diff(wanted.len()))
}

fn spawn_screen(world: &mut World, name: &str, def: &ScreenDef) -> Entity {
    let root = UiWidget {
        path: name.to_owned(),
        kind: WidgetKind::Panel { layout: def.layout },
        label: def.title.as_deref().map(|t| shards_locale::resolve(world, t)).unwrap_or_default(),
        value: UiValue::None,
        bind: None,
        setting: None,
        enabled: true,
    };
    let screen = UiScreen { name: name.to_owned(), def: def.clone(), focus: None, capture: None, note: None };
    let root = world.spawn((Name::new(format!("ui {name}")), screen, root)).id();
    let mut counters = BTreeMap::new();
    for w in &def.widgets {
        spawn_widget(world, root, name, w, &mut counters);
    }
    root
}

fn spawn_widget(world: &mut World, parent: Entity, screen: &str, def: &WidgetDef, counters: &mut BTreeMap<&'static str, u32>) {
    let path = format!("{screen}/{}", assign_id(def, counters));
    let widget = UiWidget::from_def(path.clone(), def, &|t| shards_locale::resolve(world, t));
    let e = world.spawn((Name::new(format!("ui {path}")), widget, ChildOf(parent))).id();
    for child in def.children() {
        spawn_widget(world, e, screen, child, counters);
    }
}

impl UiWidget {
    /// The widget `def` describes, its texts through `tr` (`@menu-resume` → `Resume`).
    fn from_def(path: String, def: &WidgetDef, tr: &dyn Fn(&str) -> String) -> UiWidget {
        let plain = |kind, label: &str, value| UiWidget {
            path: path.clone(),
            kind,
            label: tr(label),
            value,
            bind: None,
            setting: None,
            enabled: true,
        };
        let title = def.title().unwrap_or_default();
        match def {
            WidgetDef::Panel { layout, .. } => plain(WidgetKind::Panel { layout: *layout }, title, UiValue::None),
            WidgetDef::List { .. } => plain(WidgetKind::List, title, UiValue::None),
            WidgetDef::Grid { columns, .. } => plain(WidgetKind::Grid { columns: (*columns).max(1) }, title, UiValue::None),
            WidgetDef::Tabs { .. } => plain(WidgetKind::Tabs, "", UiValue::Page(0)),
            WidgetDef::Text { text, bind, .. } => UiWidget { bind: bind.clone(), ..plain(WidgetKind::Text, "", UiValue::Text(tr(text))) },
            WidgetDef::Image { path, label, width, height, .. } => {
                plain(WidgetKind::Image { path: path.clone(), width: *width, height: *height }, label, UiValue::None)
            }
            WidgetDef::Hint { text, bind, .. } => UiWidget { bind: bind.clone(), ..plain(WidgetKind::Hint, "", UiValue::Text(tr(text))) },
            WidgetDef::Button { label, action, enabled, .. } => {
                UiWidget { enabled: *enabled, ..plain(WidgetKind::Button { action: action.clone() }, label, UiValue::None) }
            }
            WidgetDef::Bar { label, bind, value, max, color, .. } => UiWidget {
                bind: bind.clone(),
                ..plain(WidgetKind::Bar { color: color.clone() }, label, UiValue::Ratio { value: *value, max: *max })
            },
            WidgetDef::Cooldown { label, bind, .. } => {
                UiWidget { bind: Some(bind.clone()), ..plain(WidgetKind::Cooldown, label, UiValue::Cooldown { left: 0.0 }) }
            }
            WidgetDef::Compass { bind, .. } => {
                UiWidget { bind: Some(bind.clone()), ..plain(WidgetKind::Compass, "", UiValue::Heading { deg: 0.0, marks: Vec::new() }) }
            }
            WidgetDef::Allies { bind, .. } => UiWidget { bind: Some(bind.clone()), ..plain(WidgetKind::Allies, "", UiValue::Allies(Vec::new())) },
            WidgetDef::Compare { rows, bind, .. } => {
                let rows = rows.iter().map(|r| CompareRow { stat: tr(&r.stat), ..r.clone() }).collect();
                UiWidget { bind: bind.clone(), ..plain(WidgetKind::Compare, title, UiValue::Compare(rows)) }
            }
            WidgetDef::Slider { label, min, max, step, value, setting, .. } => UiWidget {
                setting: setting.clone(),
                ..plain(WidgetKind::Slider { min: *min, max: *max, step: *step }, label, UiValue::Number(value.unwrap_or(*min)))
            },
            WidgetDef::Toggle { label, on, setting, .. } => UiWidget { setting: setting.clone(), ..plain(WidgetKind::Toggle, label, UiValue::Flag(*on)) },
            WidgetDef::Choice { label, options, labels, value, setting, .. } => {
                let value = value.clone().or_else(|| options.first().cloned()).unwrap_or_default();
                let kind = WidgetKind::Choice { options: options.clone(), labels: labels.iter().map(|l| tr(l)).collect() };
                UiWidget { setting: setting.clone(), ..plain(kind, label, UiValue::Choice(value)) }
            }
            WidgetDef::Binding { label, context, action, slot, prompt, taken, .. } => {
                let kind = WidgetKind::Binding {
                    context: context.clone(),
                    action: action.clone(),
                    slot: *slot,
                    prompt: prompt.as_deref().map_or_else(|| DEFAULT_PROMPT.to_owned(), tr),
                    taken: taken.clone(),
                };
                let bind = format!("{BINDINGS_SOURCE}:{context}.{action}");
                UiWidget { bind: Some(bind), ..plain(kind, label, UiValue::Bindings(BoundInputs::default())) }
            }
        }
    }
}

/// Puts focus on the screen's first focusable widget unless it rests on one already.
pub(crate) fn ensure_focus(world: &mut World, root: Entity) {
    let all = focusables(world, root);
    let now = world.get::<UiScreen>(root).and_then(|s| s.focus.clone());
    let still = now.as_deref().is_some_and(|p| all.iter().any(|e| widget(world, *e).is_some_and(|w| w.path == p)));
    if !still {
        let first = all.first().and_then(|e| widget(world, *e)).map(|w| w.path.clone());
        if let Some(mut screen) = world.get_mut::<UiScreen>(root) {
            screen.focus = first;
        }
    }
}

fn set_focus(world: &mut World, e: Entity) {
    let path = widget(world, e).map(|w| w.path.clone());
    if let Some(root) = screen_of(world, e)
        && let Some(mut screen) = world.get_mut::<UiScreen>(root)
    {
        screen.focus = path;
    }
}

/// Closes the screen at `root`.
pub fn close(world: &mut World, root: Entity) -> String {
    let Some(name) = world.get::<UiScreen>(root).map(|s| s.name.clone()) else { return "nothing to close".to_owned() };
    let hud = world.get::<UiScreen>(root).is_some_and(|s| s.def.layer == Layer::Hud);
    world.resource_mut::<UiStack>().open.retain(|(_, e)| *e != root);
    world.entity_mut(root).despawn();
    if !hud {
        journal::log(world, JournalKind::Local, format!("ui: closed {name}"));
    }
    world.trigger(UiScreenChanged { screen: name.clone(), open: false });
    sync_pause(world);
    match top_screen(world) {
        Some(top) => format!("closed {name}; {} is on top{}", world.get::<UiScreen>(top).map_or("", |s| s.name.as_str()), focus_note(world, top)),
        None => format!("closed {name}; no menu is open"),
    }
}

/// What `back` does: closes the screen that takes input, if it may be closed so.
pub fn back(world: &mut World) -> Result<String, String> {
    let top = top_screen(world).ok_or_else(no_screen)?;
    let screen = world.get::<UiScreen>(top).cloned().ok_or_else(no_screen)?;
    if !screen.def.closable {
        let buttons: Vec<String> = focusables(world, top).into_iter().filter_map(|e| widget(world, e).map(|w| w.path.clone())).collect();
        return Err(format!("{} cannot be closed with back: answer it ({})", screen.name, buttons.join(", ")));
    }
    Ok(close(world, top))
}

pub(crate) fn no_screen() -> String {
    "no menu is open (only the HUD, which takes no input): open one with `ui_open screen=NAME`".to_owned()
}

/// A direction of [`navigate`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Next,
    Prev,
    Confirm,
    Back,
}

impl Nav {
    pub fn parse(text: &str) -> Option<Nav> {
        Some(match text.to_ascii_lowercase().as_str() {
            "up" => Nav::Up,
            "down" => Nav::Down,
            "left" => Nav::Left,
            "right" => Nav::Right,
            "next" | "tab" => Nav::Next,
            "prev" | "previous" => Nav::Prev,
            "confirm" | "ok" | "enter" => Nav::Confirm,
            "back" | "cancel" | "escape" => Nav::Back,
            _ => return None,
        })
    }
}

/// Moves focus, changes the focused widget or presses it, the way a gamepad does on the screen
/// that takes input.
pub fn navigate(world: &mut World, nav: Nav, agent: &str) -> Result<String, String> {
    if nav == Nav::Back {
        // A binding that waits for an input stops waiting, and the screen stays: Escape cancels.
        if let Some(top) = top_screen(world)
            && let Some(path) = cancel_capture(world, top)
        {
            let kept = find(world, &path).map(|e| shown_value(world, e)).unwrap_or_default();
            return Ok(format!("{path} stopped waiting for an input; it stays {kept}"));
        }
        return back(world);
    }
    let top = top_screen(world).ok_or_else(no_screen)?;
    if nav != Nav::Confirm {
        // Moving on: a binding that waited stops.
        cancel_capture(world, top);
    }
    ensure_focus(world, top);
    let all = focusables(world, top);
    let Some(current) = focused(world) else {
        let name = world.get::<UiScreen>(top).map_or(String::new(), |s| s.name.clone());
        return Err(format!("{name} has nothing to focus: `back` closes it"));
    };
    let kind = widget(world, current).map(|w| w.kind.clone()).ok_or_else(no_screen)?;
    let step = |delta: isize| {
        let at = all.iter().position(|e| *e == current).unwrap_or(0) as isize;
        all[(at + delta).rem_euclid(all.len() as isize) as usize]
    };
    let target = match nav {
        Nav::Confirm => return activate(world, current, agent),
        Nav::Left | Nav::Right => {
            let delta = if nav == Nav::Left { -1 } else { 1 };
            if matches!(kind, WidgetKind::Slider { .. } | WidgetKind::Choice { .. } | WidgetKind::Tabs | WidgetKind::Toggle) {
                return adjust(world, current, delta, agent);
            }
            match sideways(world, current, delta as isize) {
                Some(e) => e,
                None => return Ok(format!("nothing {} of {}", if delta < 0 { "left" } else { "right" }, path_of(world, current))),
            }
        }
        Nav::Up | Nav::Down => {
            let delta = if nav == Nav::Up { -1 } else { 1 };
            match in_grid(world, current) {
                Some((items, columns, at)) => {
                    let to = at as isize + delta * columns as isize;
                    match usize::try_from(to).ok().and_then(|i| items.get(i)) {
                        Some(e) => *e,
                        None => return Ok(format!("nothing {} {}", if delta < 0 { "above" } else { "below" }, path_of(world, current))),
                    }
                }
                None => step(delta),
            }
        }
        Nav::Next => step(1),
        Nav::Prev => step(-1),
        Nav::Back => unreachable!("handled above"),
    };
    set_focus(world, target);
    if target != current {
        world.trigger(UiFocusMoved { widget: path_of(world, target) });
    }
    Ok(format!("focus {}", describe_short(world, target)))
}

/// Within a row or a grid, the focusable sibling `delta` away.
fn sideways(world: &World, e: Entity, delta: isize) -> Option<Entity> {
    let parent = world.get::<ChildOf>(e)?.parent();
    let row = matches!(widget(world, parent)?.kind, WidgetKind::Panel { layout: Layout::Row } | WidgetKind::Grid { .. });
    if !row {
        return None;
    }
    let items: Vec<Entity> = widget_children(world, parent).into_iter().filter(|c| widget(world, *c).is_some_and(|w| w.kind.focusable() && w.enabled)).collect();
    let at = items.iter().position(|c| *c == e)? as isize + delta;
    if let Some((_, columns, _)) = in_grid(world, e)
        && at.div_euclid(columns as isize) != (at - delta).div_euclid(columns as isize)
    {
        return None;
    }
    usize::try_from(at).ok().and_then(|i| items.get(i).copied())
}

/// The focusable items of the grid `e` is in, its columns and `e`'s place.
fn in_grid(world: &World, e: Entity) -> Option<(Vec<Entity>, u32, usize)> {
    let parent = world.get::<ChildOf>(e)?.parent();
    let WidgetKind::Grid { columns } = widget(world, parent)?.kind else { return None };
    let items: Vec<Entity> = widget_children(world, parent).into_iter().filter(|c| widget(world, *c).is_some_and(|w| w.kind.focusable() && w.enabled)).collect();
    let at = items.iter().position(|c| *c == e)?;
    Some((items, columns, at))
}

fn path_of(world: &World, e: Entity) -> String {
    widget(world, e).map(|w| w.path.clone()).unwrap_or_default()
}

/// `pause/resume (button "Resume")`.
pub(crate) fn describe_short(world: &World, e: Entity) -> String {
    let Some(w) = widget(world, e) else { return String::new() };
    if w.label.is_empty() { format!("{} ({})", w.path, w.kind.name()) } else { format!("{} ({} \"{}\")", w.path, w.kind.name(), w.label) }
}

/// Checks that a player could reach `e` now: on the screen that takes input, on a shown page,
/// enabled.
pub fn reachable(world: &World, e: Entity) -> Result<(), String> {
    let w = widget(world, e).ok_or("not a widget")?;
    let root = screen_of(world, e).ok_or("not on a screen")?;
    let screen = world.get::<UiScreen>(root).ok_or("not on a screen")?;
    if screen.def.layer == Layer::Hud {
        return Err(format!("{} is on the HUD, which takes no input", w.path));
    }
    if top_screen(world) != Some(root) {
        let top = top_screen(world).and_then(|t| world.get::<UiScreen>(t)).map_or(String::new(), |s| s.name.clone());
        return Err(format!("{} is under {top}: answer or close that first", w.path));
    }
    if !on_shown_page(world, e) {
        return Err(format!("{} is on a page that is not shown: turn the tabs to it first (`ui_set id=<tabs> value=<page>`)", w.path));
    }
    if !w.enabled {
        return Err(format!("{} is disabled", w.path));
    }
    Ok(())
}

/// Presses `e`: a button does its action, a toggle flips, a choice or tabs go to the next, a
/// binding waits for an input.
pub fn activate(world: &mut World, e: Entity, agent: &str) -> Result<String, String> {
    reachable(world, e)?;
    set_focus(world, e);
    let w = widget(world, e).cloned().ok_or("not a widget")?;
    if matches!(w.kind, WidgetKind::Binding { .. }) {
        return start_capture(world, e, &w);
    }
    if let Some(root) = screen_of(world, e) {
        cancel_capture(world, root);
    }
    match &w.kind {
        WidgetKind::Button { action } => press(world, e, &w.path, action, agent),
        WidgetKind::Toggle | WidgetKind::Choice { .. } | WidgetKind::Tabs => adjust(world, e, 1, agent),
        WidgetKind::Slider { .. } => Err(format!("{} is a slider: `left`/`right` move it, `ui_set` sets it", w.path)),
        other => Err(format!("{} is a {}: nothing to press", w.path, other.name())),
    }
}

fn press(world: &mut World, e: Entity, path: &str, action: &str, agent: &str) -> Result<String, String> {
    let parsed = ButtonAction::parse(action).map_err(|err| format!("{path}: {err}"))?;
    let screen = path.split('/').next().unwrap_or_default().to_owned();
    let done = match parsed {
        ButtonAction::Close => match screen_of(world, e) {
            Some(root) => close(world, root),
            None => return Err(format!("{path} is not on a screen")),
        },
        ButtonAction::Back => back(world)?,
        ButtonAction::Open(name) => open(world, &name)?,
        ButtonAction::Quit => {
            world.write_message(AppExit::Success);
            "the game ends".to_owned()
        }
        ButtonAction::Tool(tool, args) => {
            let out = ai_core::call_tool(world, agent, &tool, args);
            let text = out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned);
            if !out.ok {
                return Err(format!("{path} calls {tool}: {text}"));
            }
            format!("{tool}: {text}")
        }
        ButtonAction::Game(name) => {
            let handler = world.get_resource::<UiActions>().and_then(|a| a.get(&name));
            let Some(handler) = handler else {
                let known = world.get_resource::<UiActions>().map(UiActions::names).unwrap_or_default();
                return Err(format!(
                    "{path} does `{name}`, which this game does not handle (add_ui_action); game actions: {}",
                    if known.is_empty() { "none".to_owned() } else { known.join(", ") }
                ));
            };
            handler(world, &UiPress { widget: path.to_owned(), screen, action: name.clone(), agent: agent.to_owned() })
                .map_err(|err| format!("{path} ({name}): {err}"))?
        }
    };
    journal::log(world, JournalKind::Local, format!("ui: {path} pressed ({action})"));
    world.trigger(UiPressed { widget: path.to_owned(), action: action.to_owned() });
    Ok(format!("pressed {path}: {done}"))
}

/// Moves a slider by steps, turns a choice or tabs by one, flips a toggle.
pub fn adjust(world: &mut World, e: Entity, delta: i32, agent: &str) -> Result<String, String> {
    reachable(world, e)?;
    set_focus(world, e);
    let w = widget(world, e).cloned().ok_or("not a widget")?;
    let now = live_value(world, &w);
    let value = match (&w.kind, now) {
        (WidgetKind::Slider { min, max, step }, UiValue::Number(v)) => {
            let step = if *step > 0.0 { *step } else { (max - min) / 20.0 };
            format_number(v + delta as f32 * step)
        }
        (WidgetKind::Choice { options, .. }, UiValue::Choice(v)) => {
            let at = options.iter().position(|o| *o == v).unwrap_or(0) as i32;
            options[(at + delta).rem_euclid(options.len().max(1) as i32) as usize].clone()
        }
        (WidgetKind::Tabs, UiValue::Page(i)) => {
            let pages = widget_children(world, e).len().max(1) as i32;
            ((i as i32 + delta).rem_euclid(pages) + 1).to_string()
        }
        (WidgetKind::Toggle, UiValue::Flag(on)) => (!on).to_string(),
        (WidgetKind::Binding { .. }, _) => return Err(format!("{} is a binding: `confirm` waits for an input, `ui_set` sets one", w.path)),
        (kind, _) => return Err(format!("{} is a {}: nothing to change", w.path, kind.name())),
    };
    set_value(world, e, &value, agent)
}

/// Sets a slider, toggle, choice, tabs or binding to `text` (a number; `on`/`off`; an option; a
/// page's id, title or number from 1; an input such as `Key(KeyK)`, or `default`), and its player
/// setting with it.
pub fn set_value(world: &mut World, e: Entity, text: &str, agent: &str) -> Result<String, String> {
    reachable(world, e)?;
    set_focus(world, e);
    let w = widget(world, e).cloned().ok_or("not a widget")?;
    if matches!(w.kind, WidgetKind::Binding { .. }) {
        return set_binding(world, e, &w, text.trim(), agent);
    }
    if let Some(root) = screen_of(world, e) {
        cancel_capture(world, root);
    }
    let before = live_value(world, &w);
    let text = text.trim();
    let value = match &w.kind {
        WidgetKind::Slider { min, max, step } => {
            let v: f32 = text.parse().map_err(|_| format!("{} is a slider: a number from {min} to {max}, not `{text}`", w.path))?;
            if !v.is_finite() {
                return Err(format!("{} is a slider: a number from {min} to {max}, not `{text}`", w.path));
            }
            let snapped = if *step > 0.0 { min + ((v - min) / step).round() * step } else { v };
            UiValue::Number(snapped.clamp(*min, *max))
        }
        WidgetKind::Toggle => UiValue::Flag(match text.to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => true,
            "false" | "off" | "no" | "0" => false,
            _ => return Err(format!("{} is a toggle: `on` or `off`, not `{text}`", w.path)),
        }),
        WidgetKind::Choice { options, labels } => {
            // An option, or what it says.
            let found = options.iter().find(|o| o.eq_ignore_ascii_case(text)).or_else(|| {
                labels.iter().position(|l| l.to_lowercase() == text.to_lowercase()).and_then(|i| options.get(i))
            });
            let each: Vec<String> = options
                .iter()
                .map(|o| match w.kind.option_label(o) {
                    label if label == o => o.clone(),
                    label => format!("{o} ({label})"),
                })
                .collect();
            UiValue::Choice(found.cloned().ok_or_else(|| format!("{} is one of {}, not `{text}`", w.path, each.join(", ")))?)
        }
        WidgetKind::Tabs => {
            let pages = widget_children(world, e);
            let named = pages.iter().position(|p| {
                widget(world, *p).is_some_and(|pw| pw.path.rsplit('/').next() == Some(text) || pw.label.eq_ignore_ascii_case(text))
            });
            // A number counts pages from 1, the way the tree reads them.
            let index = named.or_else(|| text.parse::<usize>().ok().filter(|n| (1..=pages.len()).contains(n)).map(|n| n - 1));
            let names: Vec<String> = pages.iter().filter_map(|p| widget(world, *p)).map(|pw| pw.path.rsplit('/').next().unwrap_or("").to_owned()).collect();
            UiValue::Page(index.ok_or_else(|| format!("{} has pages {} (or 1 to {}), not `{text}`", w.path, names.join(", "), names.len()))?)
        }
        other => return Err(format!("{} is a {}: `ui_set` takes sliders, toggles, choices, tabs and bindings", w.path, other.name())),
    };
    let shown = value_text(world, e, &value);
    if let Some(key) = &w.setting {
        let (group, field) = key.split_once('.').ok_or_else(|| format!("{}: setting `{key}` is not `group.key`", w.path))?;
        // Settings read a switch as `true`/`false`.
        let text = match value {
            UiValue::Flag(on) => on.to_string(),
            _ => shown.clone(),
        };
        let out = ai_core::call_tool(world, agent, "settings_set", json!({"group": group, "key": field, "value": text}));
        if !out.ok {
            let why = out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned);
            return Err(format!("{} ({key}): {why}", w.path));
        }
    }
    if let Some(mut now) = world.get_mut::<UiWidget>(e) {
        now.value = value.clone();
    }
    let was = value_text(world, e, &before);
    if was == shown {
        return Ok(format!("{} is {shown} already", w.path));
    }
    if w.kind == WidgetKind::Tabs
        && let Some(root) = screen_of(world, e)
    {
        // The new page's widgets can take focus; the tabs keep it.
        ensure_focus(world, root);
    }
    journal::log(world, JournalKind::Local, format!("ui: {} {was} → {shown}", w.path));
    world.trigger(UiChanged { widget: w.path.clone(), value: shown.clone() });
    Ok(format!("{} {was} → {shown}", w.path))
}

/// The binding widget of the screen that takes input that waits for an input (its path).
pub fn capturing(world: &World) -> Option<String> {
    let top = top_screen(world)?;
    let path = world.get::<UiScreen>(top)?.capture.clone()?;
    find(world, &path).map(|_| path)
}

/// Whether binding widget `e` waits for an input, and what it says of a refused one.
pub fn binding_state(world: &World, e: Entity) -> (bool, Option<String>) {
    let Some(w) = widget(world, e) else { return (false, None) };
    let Some(screen) = screen_of(world, e).and_then(|root| world.get::<UiScreen>(root)) else { return (false, None) };
    let waits = screen.capture.as_deref() == Some(w.path.as_str());
    let note = screen.note.as_ref().filter(|(path, _)| *path == w.path).map(|(_, text)| text.clone());
    (waits, note)
}

/// Stops the binding widget of the screen at `root` that waits for an input, if one does.
fn cancel_capture(world: &mut World, root: Entity) -> Option<String> {
    let path = world.get_mut::<UiScreen>(root)?.capture.take()?;
    journal::log(world, JournalKind::Local, format!("ui: {path} stopped waiting for an input"));
    Some(path)
}

/// What widget `e` shows now, as `ui_set` and the journal say it.
fn shown_value(world: &World, e: Entity) -> String {
    widget(world, e).map(|w| value_text(world, e, &live_value(world, w))).unwrap_or_default()
}

/// A binding widget waits for an input: the player's next key, mouse button or gamepad button,
/// which the engine's controls set with `ui_set` (as agent `input`), or an agent's `ui_set`.
fn start_capture(world: &mut World, e: Entity, w: &UiWidget) -> Result<String, String> {
    let root = screen_of(world, e).ok_or("not on a screen")?;
    let how = format!(
        "the next key, mouse button or gamepad button becomes it (Escape or Start cancels, as `back` does); \
         an agent gives one with `ui_set id={} value=Key(KeyK)`",
        w.path
    );
    if world.get::<UiScreen>(root).is_some_and(|s| s.capture.as_deref() == Some(w.path.as_str())) {
        return Ok(format!("{} is waiting for an input already: {how}", w.path));
    }
    cancel_capture(world, root);
    if let Some(mut screen) = world.get_mut::<UiScreen>(root) {
        screen.capture = Some(w.path.clone());
        screen.note = None;
    }
    journal::log(world, JournalKind::Local, format!("ui: {} waits for an input", w.path));
    world.trigger(UiPressed { widget: w.path.clone(), action: "capture".to_owned() });
    Ok(format!("{} waits for an input: {how}", w.path))
}

/// What the source says of binding widget `w` now: its action's inputs, or why it has none.
fn bound_now(world: &World, w: &UiWidget) -> Result<BoundInputs, String> {
    let bind = w.bind.as_deref().unwrap_or(BINDINGS_SOURCE);
    match read_source(world, bind).map_err(|why| format!("{}: {why}", w.path))? {
        Some(UiValue::Bindings(inputs)) => Ok(inputs),
        Some(UiValue::Text(why)) => Err(format!("{}: {why}", w.path)),
        _ => Err(format!("{}: `{bind}` says nothing of the controls", w.path)),
    }
}

/// Input `text` as the source reads it for the action of binding widget `w`: what a player
/// reads, its device and the other action that holds it. `None`: no input reads so.
fn ask_input(world: &World, w: &UiWidget, text: &str) -> Option<BoundInput> {
    let bind = w.bind.as_deref()?;
    match read_source(world, &format!("{bind} {text}")) {
        Ok(Some(UiValue::Bindings(asked))) => asked.inputs.into_iter().next(),
        _ => None,
    }
}

/// The label of the binding widget of `action` on the screen at `root`, and its path; the action
/// itself without one.
fn holder_name(world: &World, root: Entity, context: &str, action: &str) -> (String, Option<String>) {
    for (e, _) in walk(world, root) {
        if let Some(UiWidget { kind: WidgetKind::Binding { context: c, action: a, .. }, label, path, .. }) = widget(world, e)
            && c == context
            && a == action
        {
            let name = if label.is_empty() { action.to_owned() } else { label.clone() };
            return (name, Some(path.clone()));
        }
    }
    (action.to_owned(), None)
}

/// Gives binding widget `e` input `text` (`Key(KeyK)`), or its action back its default inputs
/// (`default`), through the `controls` settings: what `settings_set` refuses (an input another
/// action reads) is refused, and the widget says which action holds it.
fn set_binding(world: &mut World, e: Entity, w: &UiWidget, text: &str, agent: &str) -> Result<String, String> {
    let WidgetKind::Binding { context, action, slot, taken, .. } = &w.kind else { return Err(format!("{} is not a binding", w.path)) };
    let root = screen_of(world, e).ok_or("not on a screen")?;
    // This widget's wait is answered; another one's stops.
    if let Some(mut screen) = world.get_mut::<UiScreen>(root)
        && screen.capture.as_deref() == Some(w.path.as_str())
    {
        screen.capture = None;
    }
    cancel_capture(world, root);
    let now = bound_now(world, w)?;
    let was = inputs_text(w.kind.shown_inputs(&now.inputs));
    let list: Vec<String> = now.inputs.iter().map(|i| i.text.clone()).collect();
    let mut asked = None;
    let next = if matches!(text.to_ascii_lowercase().as_str(), "default" | "reset") {
        now.defaults.clone()
    } else {
        let input = ask_input(world, w, text);
        if let Some(input) = input.as_ref().filter(|i| i.axis) {
            return Err(format!("{} takes a key, a mouse button or a gamepad button, not an axis like {}", w.path, input.text));
        }
        // Text that is no input goes as it is: the settings say what is wrong with it.
        let new = input.as_ref().map_or_else(|| text.to_owned(), |i| i.text.clone());
        let gamepad = input.as_ref().map(|i| i.gamepad);
        asked = input;
        let mut next = list.clone();
        match slot {
            Some(slot) if *slot > next.len() => {
                return Err(format!(
                    "{}: `{action}` has {} input(s), so it has no slot {slot} (slot {} adds one)",
                    w.path,
                    next.len(),
                    next.len()
                ));
            }
            Some(slot) => {
                if *slot == next.len() {
                    next.push(new.clone());
                } else {
                    next[*slot] = new.clone();
                }
                // The input moves to this slot from another one of the action.
                let mut at = 0;
                next.retain(|t| {
                    at += 1;
                    at - 1 == *slot || *t != new
                });
            }
            // The action reads it already.
            None if next.contains(&new) => {}
            None => {
                // The first input of the same kind of device; a gamepad's is added when there is none.
                let same = match gamepad {
                    Some(pad) => now.inputs.iter().position(|i| i.gamepad == pad),
                    None => Some(0),
                };
                match same.filter(|at| *at < next.len()) {
                    Some(at) => next[at] = new,
                    None => next.push(new),
                }
            }
        }
        next
    };
    if next == list {
        return Ok(format!("{} is {was} already", w.path));
    }
    let key = format!("bindings.{context}.{action}");
    // Back to the defaults is no setting at all: the file keeps only what differs.
    let args = if next == now.defaults {
        json!({"group": "controls", "key": key, "reset": true})
    } else {
        json!({"group": "controls", "key": key, "value": next})
    };
    let out = ai_core::call_tool(world, agent, "settings_set", args);
    if !out.ok {
        let why = out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned);
        let mut refusal = format!("{}: {why}", w.path);
        if let Some(BoundInput { label: input, taken_by: Some(holder), .. }) = asked {
            let (name, holder_path) = holder_name(world, root, context, &holder);
            let note = match taken.as_deref().and_then(shards_locale::message_ref) {
                Some(id) => shards_locale::text(world, id, &shards_locale::Args::new().text("input", input).text("action", name)),
                None => format!("{input} is already used by {name}"),
            };
            if let Some(mut screen) = world.get_mut::<UiScreen>(root) {
                screen.note = Some((w.path.clone(), note));
            }
            if let Some(path) = holder_path {
                refusal.push_str(&format!(" (on this screen: {path})"));
            }
        }
        return Err(refusal);
    }
    if let Some(mut screen) = world.get_mut::<UiScreen>(root) {
        screen.note = None;
    }
    let after = bound_now(world, w)?;
    let shown = inputs_text(w.kind.shown_inputs(&after.inputs));
    if let Some(mut stored) = world.get_mut::<UiWidget>(e) {
        stored.value = UiValue::Bindings(after);
    }
    journal::log(world, JournalKind::Local, format!("ui: {} {was} → {shown}", w.path));
    world.trigger(UiChanged { widget: w.path.clone(), value: shown.clone() });
    Ok(format!("{} {was} → {shown}", w.path))
}

/// A value as `ui_set` takes it: `1.25`, `on`, `reduced`, a page's id; a binding's inputs as a
/// player reads them.
pub fn value_text(world: &World, e: Entity, value: &UiValue) -> String {
    match value {
        UiValue::Bindings(bound) => widget(world, e).map_or_else(|| inputs_text(&bound.inputs), |w| inputs_text(w.kind.shown_inputs(&bound.inputs))),
        UiValue::Number(v) => format_number(*v),
        UiValue::Flag(on) => if *on { "on" } else { "off" }.to_owned(),
        UiValue::Choice(c) | UiValue::Text(c) => c.clone(),
        UiValue::Page(i) => widget_children(world, e)
            .get(*i)
            .and_then(|p| widget(world, *p))
            .map_or_else(|| i.to_string(), |p| p.path.rsplit('/').next().unwrap_or_default().to_owned()),
        _ => String::new(),
    }
}

/// A number without float noise: `1.25`, `3`, `0.1`.
pub fn format_number(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    let text = format!("{rounded:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0".to_owned() } else { text.to_owned() }
}

/// `OnEnter(EngineState::InGame)`: the screens content opens at the start (a HUD), by id.
pub(crate) fn open_start_screens(world: &mut World) {
    let Some(db) = world.get_resource::<ContentDb>() else { return };
    let start: Vec<String> = db.iter::<ScreenDef>().filter(|(_, d)| d.open_at_start).map(|(id, _)| id.to_owned()).collect();
    for name in start {
        if let Err(e) = open(world, &name) {
            warn!("ui: {e}");
        }
    }
}

/// `OnEnter(EngineState::MainMenu)`: the game's main menu (`main_menu: true`), by id.
pub(crate) fn open_main_menu(world: &mut World) {
    let Some(db) = world.get_resource::<ContentDb>() else { return };
    let menus: Vec<String> = db.iter::<ScreenDef>().filter(|(_, d)| d.main_menu).map(|(id, _)| id.to_owned()).collect();
    if menus.is_empty() {
        warn!("ui: the game is in its main menu, and no screen is one (`main_menu: true` in a `*.screen.ron`)");
    }
    for name in menus {
        if let Err(e) = open(world, &name) {
            warn!("ui: {e}");
        }
    }
}

/// `OnExit(EngineState::MainMenu)`: the game starts, so the main menu closes, and every screen
/// opened over it (settings) with it, the newest first.
pub(crate) fn close_main_menu(world: &mut World) {
    let open: Vec<Entity> = world.resource::<UiStack>().screens().map(|(_, e)| e).collect();
    for root in open.into_iter().rev() {
        close(world, root);
    }
}

/// The text the open screens say things in: the [`Locale`](shards_locale::Locale) version they
/// were last given.
#[derive(Resource, Default)]
pub(crate) struct UiTextVersion(u64);

/// `Update`, after the locale: another language, or its files edited. Every open screen says
/// what it says again in the new text; values, focus and pages stay as they are.
pub(crate) fn retell_screens(world: &mut World) {
    let Some(version) = world.get_resource::<shards_locale::Locale>().map(shards_locale::Locale::version) else { return };
    if world.resource::<UiTextVersion>().0 == version {
        return;
    }
    world.resource_mut::<UiTextVersion>().0 = version;
    let open: Vec<(String, Entity)> = world.resource::<UiStack>().screens().map(|(n, e)| (n.to_owned(), e)).collect();
    for (name, root) in open {
        let Some(def) = world.get::<UiScreen>(root).map(|s| s.def.clone()) else { continue };
        let mut fresh = BTreeMap::new();
        fresh.insert(name.clone(), def.title.as_deref().map(|t| shards_locale::resolve(world, t)).unwrap_or_default());
        let mut told = BTreeMap::new();
        let mut counters = BTreeMap::new();
        for w in &def.widgets {
            retold(world, &name, w, &mut counters, &mut told);
        }
        for (e, _) in walk(world, root) {
            let Some(mut w) = world.get_mut::<UiWidget>(e) else { continue };
            if let Some(title) = fresh.get(&w.path) {
                w.label = title.clone();
                continue;
            }
            let Some(new) = told.remove(&w.path) else { continue };
            w.label = new.label;
            if matches!(w.kind, WidgetKind::Choice { .. } | WidgetKind::Binding { .. }) {
                w.kind = new.kind;
            }
            // Text fixed by content; a source's says what the source says.
            if w.bind.is_none() && matches!(w.kind, WidgetKind::Text | WidgetKind::Hint | WidgetKind::Compare) {
                w.value = new.value;
            }
        }
    }
}

/// The widgets of `def` as they are told now, by path.
fn retold(world: &World, screen: &str, def: &WidgetDef, counters: &mut BTreeMap<&'static str, u32>, out: &mut BTreeMap<String, UiWidget>) {
    let path = format!("{screen}/{}", assign_id(def, counters));
    out.insert(path.clone(), UiWidget::from_def(path, def, &|t| shards_locale::resolve(world, t)));
    for child in def.children() {
        retold(world, screen, child, counters, out);
    }
}

/// `Update`: an open screen whose content changed is built again (focus stays where it can);
/// one whose content is gone closes.
pub(crate) fn rebuild_edited_screens(world: &mut World) {
    if !world.get_resource_ref::<ContentDb>().is_some_and(|db| db.is_changed()) {
        return;
    }
    let open: Vec<(String, Entity)> = world.resource::<UiStack>().screens().map(|(n, e)| (n.to_owned(), e)).collect();
    for (name, root) in open {
        let Some(old) = world.get::<UiScreen>(root).cloned() else { continue };
        match world.resource::<ContentDb>().get::<ScreenDef>(&name).cloned() {
            Some(def) if def == old.def => {}
            Some(def) => {
                let at = world.resource::<UiStack>().open.iter().position(|(_, e)| *e == root);
                world.entity_mut(root).despawn();
                let new = spawn_screen(world, &name, &def);
                if let Some(at) = at {
                    world.resource_mut::<UiStack>().open[at].1 = new;
                }
                if let Some(mut screen) = world.get_mut::<UiScreen>(new) {
                    screen.focus = old.focus;
                }
                ensure_focus(world, new);
                sync_pause(world);
                info!("ui: {name} built again from its edited content");
            }
            None => {
                close(world, root);
                warn!("ui: {name} closed: its content is gone");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_without_float_noise() {
        assert_eq!(format_number(1.25), "1.25");
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(0.1 + 0.2), "0.3");
        assert_eq!(format_number(-0.0001), "0");
        assert_eq!(format_number(0.5), "0.5");
    }

    #[test]
    fn a_binding_shows_its_slot_or_every_input() {
        let input = |text: &str, label: &str, gamepad| BoundInput { text: text.into(), label: label.into(), gamepad, ..default() };
        let inputs = [input("Key(Space)", "Space", false), input("GamepadButton(South)", "Pad South", true)];
        let kind = |slot| WidgetKind::Binding { context: "gameplay".into(), action: "jump".into(), slot, prompt: DEFAULT_PROMPT.into(), taken: None };
        assert_eq!(inputs_text(kind(None).shown_inputs(&inputs)), "Space · Pad South");
        assert_eq!(inputs_text(kind(Some(1)).shown_inputs(&inputs)), "Pad South");
        assert_eq!(inputs_text(kind(Some(2)).shown_inputs(&inputs)), "none", "a slot past the last shows nothing yet");
        assert_eq!(inputs_text(&[]), "none");
        assert!(kind(None).focusable() && !kind(None).container());
        assert_eq!(kind(None).name(), "binding");
    }

    #[test]
    fn directions_have_their_words() {
        assert_eq!(Nav::parse("Down"), Some(Nav::Down));
        assert_eq!(Nav::parse("enter"), Some(Nav::Confirm));
        assert_eq!(Nav::parse("escape"), Some(Nav::Back));
        assert_eq!(Nav::parse("tab"), Some(Nav::Next));
        assert_eq!(Nav::parse("sideways"), None);
    }
}
