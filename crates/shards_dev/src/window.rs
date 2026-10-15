//! The console and the menu in the game window. The key left of 1 opens the console, F10 the
//! menu ([`DevKeys`]); Escape closes either. While one is open the input map's `dev` context is
//! on top, so the character hears no key, the engine's debug keys wait ([`DevKeyboard`]), and
//! what is typed goes to the console.

use ai_core::dev::DevKeyboard;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseScrollPixelsPerLine;
use bevy::input::ButtonState;
use bevy::picking::events::{PointerClick, PointerOver, PointerScroll};
use bevy::picking::Pickable;
use bevy::prelude::*;
use shards_motion::InputContexts;

use crate::{complete, entry_state, menu_entries, submit, DevConsole, CONSOLE_AGENT};

/// The keys that open the console and the menu (`None`: none does).
#[derive(Resource, Clone, Copy, Debug)]
pub struct DevKeys {
    pub console: Option<KeyCode>,
    pub menu: Option<KeyCode>,
}

impl Default for DevKeys {
    fn default() -> Self {
        Self { console: Some(KeyCode::Backquote), menu: Some(KeyCode::F10) }
    }
}

/// The input context on top while the console or the menu has the keyboard.
pub const DEV_CONTEXT: &str = "dev";

/// Lines of the console's log on screen.
const SHOWN: usize = 14;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<DevKeys>()
        .init_resource::<Pending>()
        .init_resource::<FollowChosen>()
        .add_observer(on_click)
        .add_observer(on_over)
        .add_observer(on_scroll)
        .add_systems(
            Update,
            (toggle, type_into_console, choose_in_menu, run_pending, take_keyboard, show_console, show_menu)
                .chain()
                .run_if(any_with_component::<Window>),
        );
}

/// The menu scrolls to show its chosen row (set by Up/Down and when it opens; cleared once done).
#[derive(Resource, Default)]
struct FollowChosen(bool);

/// What the keys asked for this frame, run by [`run_pending`] (it needs the whole world).
#[derive(Resource, Default)]
struct Pending {
    lines: Vec<String>,
    /// An entry of the menu, by its place in [`menu_entries`].
    entry: Option<usize>,
    /// Tab was pressed.
    complete: bool,
}

#[derive(Component)]
struct ConsolePanel;

#[derive(Component)]
struct ConsoleText;

#[derive(Component)]
struct MenuPanel;

/// The menu's rows, under its title: they scroll.
#[derive(Component)]
struct MenuBody;

/// A row of the menu: its entry's place.
#[derive(Component, Clone, Copy)]
struct MenuRow(usize);

fn toggle(keys: Option<Res<ButtonInput<KeyCode>>>, dev_keys: Res<DevKeys>, mut console: ResMut<DevConsole>) {
    let Some(keys) = keys else { return };
    if dev_keys.console.is_some_and(|k| keys.just_pressed(k)) {
        console.open = !console.open;
        if console.open {
            console.menu_open = false;
        }
    }
    if dev_keys.menu.is_some_and(|k| keys.just_pressed(k)) {
        console.menu_open = !console.menu_open;
        if console.menu_open {
            console.open = false;
        }
    }
}

fn type_into_console(
    mut typed: MessageReader<KeyboardInput>,
    dev_keys: Res<DevKeys>,
    mut console: ResMut<DevConsole>,
    mut pending: ResMut<Pending>,
) {
    if !console.open {
        typed.clear();
        return;
    }
    for key in typed.read() {
        if key.state != ButtonState::Pressed || Some(key.key_code) == dev_keys.console {
            continue;
        }
        match &key.logical_key {
            Key::Enter => {
                let line = std::mem::take(&mut console.input);
                pending.lines.push(line);
            }
            Key::Escape => console.open = false,
            Key::Backspace => {
                console.input.pop();
                console.recall = None;
            }
            Key::Tab => pending.complete = true,
            Key::ArrowUp => recall(&mut console, -1),
            Key::ArrowDown => recall(&mut console, 1),
            _ => {
                if let Some(text) = &key.text
                    && text.chars().all(|c| !c.is_control())
                {
                    console.input.push_str(text);
                    console.recall = None;
                }
            }
        }
    }
}

/// Up and Down: the line typed before, or after, the one shown.
fn recall(console: &mut DevConsole, step: i32) {
    let count = console.history.len();
    if count == 0 {
        return;
    }
    let at = match (console.recall, step) {
        (None, -1) => Some(count - 1),
        (None, _) => None,
        (Some(i), -1) => Some(i.saturating_sub(1)),
        (Some(i), _) if i + 1 < count => Some(i + 1),
        (Some(_), _) => None,
    };
    console.recall = at;
    console.input = at.map(|i| console.history[i].clone()).unwrap_or_default();
}

fn choose_in_menu(
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut console: ResMut<DevConsole>,
    mut pending: ResMut<Pending>,
    mut follow: ResMut<FollowChosen>,
) {
    let Some(keys) = keys else { return };
    if !console.menu_open {
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        console.menu_open = false;
    } else if keys.just_pressed(KeyCode::ArrowDown) {
        console.selected += 1;
        follow.0 = true;
    } else if keys.just_pressed(KeyCode::ArrowUp) {
        console.selected = console.selected.saturating_sub(1);
        follow.0 = true;
    } else if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter) {
        pending.entry = Some(console.selected);
    }
}

/// A click on a row chooses its entry.
fn on_click(mut click: On<PointerClick>, rows: Query<&MenuRow>, mut console: ResMut<DevConsole>, mut pending: ResMut<Pending>) {
    let Ok(row) = rows.get(click.entity) else { return };
    click.propagate(false);
    console.selected = row.0;
    pending.entry = Some(row.0);
}

/// The row under the mouse is the chosen one.
fn on_over(over: On<PointerOver>, rows: Query<&MenuRow>, mut console: ResMut<DevConsole>) {
    if let Ok(row) = rows.get(over.entity) {
        console.selected = row.0;
    }
}

/// The wheel scrolls the menu's rows.
fn on_scroll(
    mut scroll: On<PointerScroll>,
    mut bodies: Query<(&ComputedNode, &mut ScrollPosition), With<MenuBody>>,
    per_line: Option<Res<MouseScrollPixelsPerLine>>,
) {
    let Ok((node, mut at)) = bodies.get_mut(scroll.entity) else { return };
    scroll.propagate(false);
    let dy = per_line.map_or(scroll.y * 20.0, |per_line| scroll.to_pixels(&per_line).y);
    at.y = (at.y - dy).clamp(0.0, scroll_range(node));
}

/// How far a node's content scrolls, in logical pixels.
fn scroll_range(node: &ComputedNode) -> f32 {
    ((node.content_size().y - node.size().y) * node.inverse_scale_factor()).max(0.0)
}

/// Scrolls the menu's rows toward showing the chosen one whole, with a row of the rest around
/// it. True once nothing is left to scroll; it reads the last frame's layout, so it is called
/// until then.
fn show_chosen(world: &mut World, chosen: usize) -> bool {
    let mut bodies = world.query_filtered::<(Entity, &ComputedNode, &UiGlobalTransform), With<MenuBody>>();
    let Some((body, view, view_at)) = bodies.iter(world).next().map(|(e, n, t)| (e, *n, *t)) else { return true };
    let mut rows = world.query::<(&MenuRow, &ComputedNode, &UiGlobalTransform)>();
    let Some((row, row_at)) = rows.iter(world).find(|(r, ..)| r.0 == chosen).map(|(_, n, t)| (*n, *t)) else { return true };
    // Not laid out yet.
    if view.size().y <= 0.0 || row.size().y <= 0.0 || view_at.translation == Vec2::ZERO {
        return false;
    }
    // In logical pixels, from the top of what is shown.
    let scale = view.inverse_scale_factor();
    let height = view.size().y * scale;
    let tall = row.size().y * scale;
    let top = ((row_at.translation.y - row.size().y / 2.0) - (view_at.translation.y - view.size().y / 2.0)) * scale;
    let by = if top - tall < 0.0 {
        top - tall
    } else if top + 2.0 * tall > height {
        top + 2.0 * tall - height
    } else {
        0.0
    };
    let Some(mut at) = world.get_mut::<ScrollPosition>(body) else { return true };
    let to = (at.y + by).clamp(0.0, scroll_range(&view));
    if (to - at.y).abs() < 0.5 {
        return true;
    }
    at.y = to;
    false
}

/// Runs what the keys asked: lines through the console, a menu entry (or its line into the
/// console to be finished), Tab's completion.
fn run_pending(world: &mut World) {
    let Pending { lines, entry, complete: tab } = std::mem::take(&mut *world.resource_mut::<Pending>());
    for line in lines {
        // The answer is in the console's log.
        let _ = submit(world, CONSOLE_AGENT, &line);
    }
    if let Some(index) = entry {
        let entries = menu_entries(world);
        if let Some(entry) = entries.get(index) {
            match entry_state(world, CONSOLE_AGENT, entry) {
                Ok(line) if line.run => {
                    let _ = submit(world, CONSOLE_AGENT, &line.line);
                }
                Ok(line) => {
                    let mut console = world.resource_mut::<DevConsole>();
                    console.input = line.line;
                    console.menu_open = false;
                    console.open = true;
                }
                Err(why) => {
                    let text = format!("> {}\nerror: `{}` does nothing here: {why}", entry.label, entry.id);
                    world.resource_mut::<DevConsole>().say(&text);
                }
            }
        }
    }
    if tab {
        let input = world.resource::<DevConsole>().input.clone();
        let (completed, offered) = complete(world, &input);
        let mut console = world.resource_mut::<DevConsole>();
        console.input = completed;
        console.offered = if offered.len() > 1 { offered } else { Vec::new() };
    }
}

/// The `dev` context on top while the console or the menu is open; taken off when both close.
fn take_keyboard(
    console: Res<DevConsole>,
    mut taken: ResMut<DevKeyboard>,
    contexts: Option<ResMut<InputContexts>>,
    mut pushed: Local<bool>,
) {
    let wants = console.open || console.menu_open;
    taken.set_if_neq(DevKeyboard(wants));
    let Some(mut contexts) = contexts else { return };
    if wants && !*pushed {
        contexts.push(DEV_CONTEXT);
        *pushed = true;
    } else if !wants && *pushed {
        // A context pushed above `dev` keeps it until that one comes off.
        if contexts.top() == DEV_CONTEXT {
            contexts.pop();
            *pushed = false;
        }
    }
}

/// The built-in font has ASCII only.
fn ascii(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '·' => "|".to_owned(),
            '→' => "->".to_owned(),
            '←' => "<-".to_owned(),
            '…' => "...".to_owned(),
            '×' => "x".to_owned(),
            '°' => " deg".to_owned(),
            c if c.is_ascii() => c.to_string(),
            _ => "?".to_owned(),
        })
        .collect()
}

const PANEL_COLOR: Color = Color::srgba(0.02, 0.02, 0.04, 0.88);

/// The console: across the bottom of the window.
fn console_panel() -> (Node, BackgroundColor, GlobalZIndex) {
    let node = Node {
        position_type: PositionType::Absolute,
        left: Val::Px(0.0),
        right: Val::Px(0.0),
        bottom: Val::Px(0.0),
        padding: UiRect::all(Val::Px(8.0)),
        flex_direction: FlexDirection::Column,
        ..default()
    };
    (node, BackgroundColor(PANEL_COLOR), GlobalZIndex(100))
}

/// The room the menu has on the right: down to the window's bottom. The menu takes as much of
/// it as its rows need, and they scroll when they need more.
fn menu_room() -> (Node, Pickable, GlobalZIndex) {
    let node = Node {
        position_type: PositionType::Absolute,
        top: Val::Px(60.0),
        bottom: Val::Px(12.0),
        right: Val::Px(12.0),
        width: Val::Px(600.0),
        max_width: Val::Percent(96.0),
        flex_direction: FlexDirection::Column,
        ..default()
    };
    (node, Pickable::IGNORE, GlobalZIndex(100))
}

fn show_console(
    mut commands: Commands,
    console: Res<DevConsole>,
    panels: Query<Entity, With<ConsolePanel>>,
    mut text: Query<&mut Text, With<ConsoleText>>,
) {
    let shown = panels.iter().next();
    if !console.open {
        if let Some(panel) = shown {
            commands.entity(panel).despawn();
        }
        return;
    }
    let start = console.log.len().saturating_sub(SHOWN);
    let mut lines: Vec<String> = console.log.iter().skip(start).map(|l| ascii(l)).collect();
    if !console.offered.is_empty() {
        lines.push(ascii(&console.offered.join("  ")));
    }
    lines.push(format!("> {}_", ascii(&console.input)));
    let body = lines.join("\n");
    match (shown, text.iter_mut().next()) {
        (Some(_), Some(mut text)) => {
            if text.0 != body {
                text.0 = body;
            }
        }
        _ => {
            commands.spawn((
                console_panel(),
                ConsolePanel,
                children![(Text::new(body), TextFont::from_font_size(15.0), TextColor(Color::srgb(0.85, 0.95, 0.85)), ConsoleText)],
            ));
        }
    }
}

fn show_menu(world: &mut World) {
    let open = world.resource::<DevConsole>().menu_open;
    let mut panels = world.query_filtered::<Entity, With<MenuPanel>>();
    let shown = panels.iter(world).next();
    if !open {
        if let Some(panel) = shown {
            world.despawn(panel);
        }
        return;
    }
    let entries = menu_entries(world);
    let count = entries.len();
    {
        let mut console = world.resource_mut::<DevConsole>();
        console.selected = console.selected.min(count.saturating_sub(1));
    }
    let selected = world.resource::<DevConsole>().selected;
    // What each entry does now: switches change with the state they switch.
    let rows: Vec<(String, bool)> = entries
        .iter()
        .map(|entry| match entry_state(world, CONSOLE_AGENT, entry) {
            Ok(line) if line.run => (format!("{}   {}", entry.label, line.line), true),
            Ok(line) => (format!("{}   {} ...", entry.label, line.line), true),
            Err(why) => (format!("{}   ({why})", entry.label), false),
        })
        .collect();
    if let Some(panel) = shown {
        let mut texts = world.query::<(&MenuRow, &Children)>();
        let rows_now: Vec<(usize, Entity)> =
            texts.iter(world).filter_map(|(row, children)| children.first().map(|c| (row.0, *c))).collect();
        if rows_now.len() == count {
            for (index, child) in rows_now {
                let (label, usable) = &rows[index];
                let color = match (index == selected, usable) {
                    (true, _) => Color::srgb(1.0, 0.85, 0.35),
                    (false, true) => Color::srgb(0.85, 0.9, 0.95),
                    (false, false) => Color::srgb(0.5, 0.5, 0.55),
                };
                let body = ascii(label);
                if let Some(mut text) = world.get_mut::<Text>(child)
                    && text.0 != body
                {
                    text.0 = body;
                }
                if let Some(mut tint) = world.get_mut::<TextColor>(child) {
                    tint.0 = color;
                }
            }
            if world.resource::<FollowChosen>().0 && show_chosen(world, selected) {
                world.resource_mut::<FollowChosen>().0 = false;
            }
            return;
        }
        // The entries changed (content was edited): built again.
        world.despawn(panel);
    }
    let title = "DEV MENU   Up/Down, Enter or click; F10 / Esc closes; ` opens the console";
    let room = world.spawn((menu_room(), MenuPanel)).id();
    // Shrinks below its rows (they scroll) rather than past the window.
    let panel = Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(8.0)), min_height: Val::Px(0.0), ..default() };
    let panel = world.spawn((panel, BackgroundColor(PANEL_COLOR), ChildOf(room))).id();
    world.spawn((Text::new(title), TextFont::from_font_size(13.0), TextColor(Color::srgb(0.6, 0.8, 1.0)), ChildOf(panel)));
    let body = Node { flex_direction: FlexDirection::Column, min_height: Val::Px(0.0), overflow: Overflow::scroll_y(), ..default() };
    let body = world.spawn((body, ScrollPosition::default(), MenuBody, ChildOf(panel))).id();
    let mut group = String::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.group != group {
            group = entry.group.clone();
            world.spawn((
                Text::new(ascii(&group.to_uppercase())),
                TextFont::from_font_size(13.0),
                TextColor(Color::srgb(0.55, 0.65, 0.75)),
                Node { margin: UiRect::top(Val::Px(6.0)), flex_shrink: 0.0, ..default() },
                ChildOf(body),
            ));
        }
        let row = Node { padding: UiRect::horizontal(Val::Px(6.0)), flex_shrink: 0.0, ..default() };
        let row = world.spawn((row, MenuRow(index), ChildOf(body))).id();
        // The row takes the click, not its text.
        world.spawn((Text::new(ascii(&rows[index].0)), TextFont::from_font_size(14.0), TextColor(Color::WHITE), Pickable::IGNORE, ChildOf(row)));
    }
    world.resource_mut::<FollowChosen>().0 = true;
}
