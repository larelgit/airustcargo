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

use crate::bind::{live_value, UiActions, UiPress};
use crate::def::{assign_id, ButtonAction, CompareRow, Layer, Layout, ScreenDef, WidgetDef};

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
    Hint,
    Button { action: String },
    Bar { color: Option<String> },
    Cooldown,
    Compass,
    Allies,
    Compare,
    Slider { min: f32, max: f32, step: f32 },
    Toggle,
    Choice { options: Vec<String> },
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
        }
    }

    /// Whether focus can rest on it: the widgets a player presses or changes.
    pub fn focusable(&self) -> bool {
        matches!(self, WidgetKind::Button { .. } | WidgetKind::Slider { .. } | WidgetKind::Toggle | WidgetKind::Choice { .. } | WidgetKind::Tabs)
    }

    /// Whether it holds other widgets.
    pub fn container(&self) -> bool {
        matches!(self, WidgetKind::Panel { .. } | WidgetKind::List | WidgetKind::Grid { .. } | WidgetKind::Tabs)
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

/// A slider, toggle, choice or tabs changed value (`value` as `ui_set` takes it).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct UiChanged {
    pub widget: String,
    pub value: String,
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
    world.resource_mut::<UiStack>().open.push((name.to_owned(), root));
    ensure_focus(world, root);
    if def.layer != Layer::Hud {
        journal::log(world, JournalKind::Event, format!("ui: opened {name}"));
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
        label: def.title.clone().unwrap_or_default(),
        value: UiValue::None,
        bind: None,
        setting: None,
        enabled: true,
    };
    let screen = UiScreen { name: name.to_owned(), def: def.clone(), focus: None };
    let root = world.spawn((Name::new(format!("ui {name}")), screen, root)).id();
    let mut counters = BTreeMap::new();
    for w in &def.widgets {
        spawn_widget(world, root, name, w, &mut counters);
    }
    root
}

fn spawn_widget(world: &mut World, parent: Entity, screen: &str, def: &WidgetDef, counters: &mut BTreeMap<&'static str, u32>) {
    let path = format!("{screen}/{}", assign_id(def, counters));
    let e = world.spawn((Name::new(format!("ui {path}")), UiWidget::from_def(path, def), ChildOf(parent))).id();
    for child in def.children() {
        spawn_widget(world, e, screen, child, counters);
    }
}

impl UiWidget {
    fn from_def(path: String, def: &WidgetDef) -> UiWidget {
        let plain = |kind, label: &str, value| UiWidget {
            path: path.clone(),
            kind,
            label: label.to_owned(),
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
            WidgetDef::Text { text, bind, .. } => UiWidget { bind: bind.clone(), ..plain(WidgetKind::Text, "", UiValue::Text(text.clone())) },
            WidgetDef::Hint { text, bind, .. } => UiWidget { bind: bind.clone(), ..plain(WidgetKind::Hint, "", UiValue::Text(text.clone())) },
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
            WidgetDef::Compare { rows, bind, .. } => UiWidget { bind: bind.clone(), ..plain(WidgetKind::Compare, title, UiValue::Compare(rows.clone())) },
            WidgetDef::Slider { label, min, max, step, value, setting, .. } => UiWidget {
                setting: setting.clone(),
                ..plain(WidgetKind::Slider { min: *min, max: *max, step: *step }, label, UiValue::Number(value.unwrap_or(*min)))
            },
            WidgetDef::Toggle { label, on, setting, .. } => UiWidget { setting: setting.clone(), ..plain(WidgetKind::Toggle, label, UiValue::Flag(*on)) },
            WidgetDef::Choice { label, options, value, setting, .. } => {
                let value = value.clone().or_else(|| options.first().cloned()).unwrap_or_default();
                UiWidget { setting: setting.clone(), ..plain(WidgetKind::Choice { options: options.clone() }, label, UiValue::Choice(value)) }
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
        journal::log(world, JournalKind::Event, format!("ui: closed {name}"));
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
        return back(world);
    }
    let top = top_screen(world).ok_or_else(no_screen)?;
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

/// Presses `e`: a button does its action, a toggle flips, a choice or tabs go to the next.
pub fn activate(world: &mut World, e: Entity, agent: &str) -> Result<String, String> {
    reachable(world, e)?;
    set_focus(world, e);
    let w = widget(world, e).cloned().ok_or("not a widget")?;
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
    journal::log(world, JournalKind::Event, format!("ui: {path} pressed ({action})"));
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
        (WidgetKind::Choice { options }, UiValue::Choice(v)) => {
            let at = options.iter().position(|o| *o == v).unwrap_or(0) as i32;
            options[(at + delta).rem_euclid(options.len().max(1) as i32) as usize].clone()
        }
        (WidgetKind::Tabs, UiValue::Page(i)) => {
            let pages = widget_children(world, e).len().max(1) as i32;
            ((i as i32 + delta).rem_euclid(pages) + 1).to_string()
        }
        (WidgetKind::Toggle, UiValue::Flag(on)) => (!on).to_string(),
        (kind, _) => return Err(format!("{} is a {}: nothing to change", w.path, kind.name())),
    };
    set_value(world, e, &value, agent)
}

/// Sets a slider, toggle, choice or tabs to `text` (a number; `on`/`off`; an option; a page's
/// id, title or number from 1), and its player setting with it.
pub fn set_value(world: &mut World, e: Entity, text: &str, agent: &str) -> Result<String, String> {
    reachable(world, e)?;
    set_focus(world, e);
    let w = widget(world, e).cloned().ok_or("not a widget")?;
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
        WidgetKind::Choice { options } => {
            let found = options.iter().find(|o| o.eq_ignore_ascii_case(text)).cloned();
            UiValue::Choice(found.ok_or_else(|| format!("{} is one of {}, not `{text}`", w.path, options.join(", ")))?)
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
        other => return Err(format!("{} is a {}: `ui_set` takes sliders, toggles, choices and tabs", w.path, other.name())),
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
    journal::log(world, JournalKind::Event, format!("ui: {} {was} → {shown}", w.path));
    world.trigger(UiChanged { widget: w.path.clone(), value: shown.clone() });
    Ok(format!("{} {was} → {shown}", w.path))
}

/// A value as `ui_set` takes it: `1.25`, `on`, `reduced`, a page's id.
pub fn value_text(world: &World, e: Entity, value: &UiValue) -> String {
    match value {
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
    fn directions_have_their_words() {
        assert_eq!(Nav::parse("Down"), Some(Nav::Down));
        assert_eq!(Nav::parse("enter"), Some(Nav::Confirm));
        assert_eq!(Nav::parse("escape"), Some(Nav::Back));
        assert_eq!(Nav::parse("tab"), Some(Nav::Next));
        assert_eq!(Nav::parse("sideways"), None);
    }
}
