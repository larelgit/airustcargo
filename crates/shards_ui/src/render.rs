//! Drawing (feature `render`, with a window or offscreen): every widget as Bevy UI nodes.
//!
//! The model's entities are drawn in place: a widget gets a `Node` (and its colours), and what
//! only draws it (a label, a bar's track and fill) are parts, children without [`UiWidget`]. A
//! screen's root sits in a frame, a full-screen node that places it by its anchor and, behind a
//! menu, dims the game. A widget is drawn again only when what it shows changed: a bar's value
//! moves its fill in place. The theme comes from content (`theme`, id `default`), font sizes
//! times the player's text size. The mouse presses through `ui_click` (as agent
//! [`INPUT_AGENT`](crate::INPUT_AGENT)), hovering only colours; screen readers get a role,
//! label and value for every widget, and the UI's focus.

use accesskit::{Node as A11yNode, Role, Toggled};
use bevy::a11y::AccessibilityNode;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::picking::events::{PointerClick, PointerOut, PointerOver};
use bevy::picking::Pickable;
use bevy::prelude::*;
use serde_json::json;
use shards_content::ContentDb;
use shards_settings::AccessibilitySettings;

use crate::bind::live_value;
use crate::def::{Anchor, Layer, Layout, ThemeDef};
use crate::model::{focused, on_shown_page, UiScreen, UiValue, UiWidget, WidgetKind};
use crate::model::{format_number, widget_children};
use crate::text::compass_point;
use crate::UiSystems;

/// The font of every text that names none (a theme without `font`, the dev console, overlays):
/// Fira Sans Regular, with Latin, Greek and Cyrillic letters. SIL Open Font License 1.1: the
/// licence is beside the file (`fonts/FiraSans-OFL.txt`). Bevy's own has ASCII letters only.
pub const DEFAULT_FONT: &[u8] = include_bytes!("../fonts/FiraSans-Regular.ttf");

/// Puts [`DEFAULT_FONT`] behind Bevy's default font handle, the one a `TextFont` that names no
/// font draws with. Once every plugin is built: Bevy's text plugin puts its own font there.
pub(crate) fn install_default_font(app: &mut App) {
    if let Some(mut fonts) = app.world_mut().get_resource_mut::<Assets<bevy::text::Font>>() {
        // The default id is never one of an asset that was removed.
        let _ = fonts.insert(AssetId::default(), bevy::text::Font::from_bytes(DEFAULT_FONT.to_vec()));
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Look>()
        .configure_sets(PostUpdate, UiSystems::Draw.before(bevy::ui::UiSystems::Prepare))
        .add_systems(PostUpdate, draw.in_set(UiSystems::Draw).run_if(ai_core::display::drawn))
        .add_observer(on_click)
        .add_observer(on_over)
        .add_observer(on_out);
}

/// The theme and text size the widgets were drawn with.
#[derive(Resource, Default, PartialEq)]
struct Look {
    theme: ThemeDef,
    scale: f32,
    /// The theme's font; `None`: the engine's own.
    font: Option<Handle<bevy::text::Font>>,
}

/// A full-screen node holding a screen's root, placed by its anchor.
#[derive(Component)]
struct Frame;

/// What only draws a widget.
#[derive(Component)]
struct Part;

/// A widget's parts, and the ones a value change updates in place.
#[derive(Component, Default)]
struct Parts {
    all: Vec<Entity>,
    /// A bar's or slider's fill.
    fill: Option<Entity>,
    /// The text that shows the value.
    value: Option<Entity>,
}

/// What a widget was drawn as: drawn again when this differs.
#[derive(Component, Clone, PartialEq)]
struct Drawn {
    widget: UiWidget,
    focused: bool,
    shown: bool,
}

fn color(rgba: [u8; 4]) -> Color {
    Color::srgba_u8(rgba[0], rgba[1], rgba[2], rgba[3])
}

fn backdrop_rgba(theme: &ThemeDef) -> [u8; 4] {
    theme.color(&theme.backdrop)
}

/// `PostUpdate`, before Bevy's layout: frames for new screens, widgets drawn where they changed,
/// focus for screen readers.
fn draw(world: &mut World) {
    update_look(world);
    frame_screens(world);
    let focus = focused(world);
    let mut query = world.query_filtered::<Entity, With<UiWidget>>();
    let widgets: Vec<Entity> = query.iter(world).collect();
    for e in widgets {
        let Some(stored) = world.get::<UiWidget>(e).cloned() else { continue };
        let widget = UiWidget { value: live_value(world, &stored), ..stored };
        let shown = on_shown_page(world, e) && !matches!((&widget.kind, &widget.value), (WidgetKind::Hint, UiValue::Text(t)) if t.is_empty());
        let now = Drawn { widget, focused: focus == Some(e), shown };
        let before = world.get::<Drawn>(e).cloned();
        if before.as_ref() == Some(&now) {
            continue;
        }
        let in_place = before.as_ref().is_some_and(|b| {
            b.focused == now.focused && b.shown == now.shown && UiWidget { value: UiValue::None, ..b.widget.clone() } == UiWidget { value: UiValue::None, ..now.widget.clone() }
        }) && matches!(now.widget.kind, WidgetKind::Bar { .. } | WidgetKind::Slider { .. } | WidgetKind::Cooldown | WidgetKind::Text | WidgetKind::Hint | WidgetKind::Compass);
        if in_place {
            update_value(world, e, &now.widget);
        } else {
            redraw(world, e, &now);
        }
        world.entity_mut(e).insert(now);
    }
    sync_focus(world, focus);
}

/// Theme or text size changed: everything is drawn again.
fn update_look(world: &mut World) {
    let theme = world.get_resource::<ContentDb>().and_then(|db| db.get::<ThemeDef>("default").cloned()).unwrap_or_default();
    let scale = world.get_resource::<AccessibilitySettings>().map_or(1.0, AccessibilitySettings::text);
    let now = world.resource::<Look>();
    if now.theme == theme && now.scale == scale {
        return;
    }
    let font = theme.font.clone().map(|path| world.resource::<AssetServer>().load(path));
    world.insert_resource(Look { theme, scale, font });
    let mut drawn = world.query_filtered::<Entity, With<Drawn>>();
    for e in drawn.iter(world).collect::<Vec<_>>() {
        world.entity_mut(e).remove::<Drawn>();
    }
    // Behind menus, the new backdrop.
    let backdrop = color(backdrop_rgba(&world.resource::<Look>().theme));
    let mut frames = world.query_filtered::<&mut BackgroundColor, With<Frame>>();
    for mut bg in frames.iter_mut(world) {
        bg.0 = backdrop;
    }
}

/// A frame for every screen root without one; frames left empty (their screen closed) go.
fn frame_screens(world: &mut World) {
    let mut frames = world.query_filtered::<(Entity, Option<&Children>), With<Frame>>();
    let empty: Vec<Entity> = frames.iter(world).filter(|(_, c)| c.is_none_or(|c| c.is_empty())).map(|(e, _)| e).collect();
    for f in empty {
        world.entity_mut(f).despawn();
    }
    let mut roots = world.query_filtered::<(Entity, &UiScreen), Without<ChildOf>>();
    let new: Vec<(Entity, Layer, Anchor)> = roots.iter(world).map(|(e, s)| (e, s.def.layer, s.def.anchor)).collect();
    let order = world.resource::<crate::UiStack>().screens().count() as i32;
    for (root, layer, anchor) in new {
        let backdrop = color(backdrop_rgba(&world.resource::<Look>().theme));
        let (justify, align) = match anchor {
            Anchor::TopLeft => (JustifyContent::FlexStart, AlignItems::FlexStart),
            Anchor::Top => (JustifyContent::Center, AlignItems::FlexStart),
            Anchor::TopRight => (JustifyContent::FlexEnd, AlignItems::FlexStart),
            Anchor::Left => (JustifyContent::FlexStart, AlignItems::Center),
            Anchor::Center => (JustifyContent::Center, AlignItems::Center),
            Anchor::Right => (JustifyContent::FlexEnd, AlignItems::Center),
            Anchor::BottomLeft => (JustifyContent::FlexStart, AlignItems::FlexEnd),
            Anchor::Bottom => (JustifyContent::Center, AlignItems::FlexEnd),
            Anchor::BottomRight => (JustifyContent::FlexEnd, AlignItems::FlexEnd),
        };
        let node = Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: justify,
            align_items: align,
            padding: UiRect::all(Val::Px(16.0)),
            ..default()
        };
        let z = match layer {
            Layer::Hud => 1,
            Layer::Menu => 10 + order,
            Layer::Modal => 100 + order,
        };
        let mut frame = world.spawn((Name::new("ui frame"), Frame, node, GlobalZIndex(z)));
        if layer == Layer::Hud {
            // The HUD takes no input: clicks go through it.
            frame.insert(Pickable::IGNORE);
        } else {
            frame.insert(BackgroundColor(backdrop));
        }
        let frame = frame.id();
        world.entity_mut(root).insert(ChildOf(frame));
    }
}

/// Font size of a role, times the player's text size.
fn font(world: &World, size: fn(&ThemeDef) -> f32) -> TextFont {
    let look = world.resource::<Look>();
    let mut font = TextFont::from_font_size(size(&look.theme) * look.scale);
    if let Some(handle) = &look.font {
        font.font = handle.clone().into();
    }
    font
}

fn theme(world: &World) -> ThemeDef {
    world.resource::<Look>().theme.clone()
}

fn text_part(world: &mut World, parent: Entity, text: String, size: fn(&ThemeDef) -> f32, rgba: [u8; 4]) -> Entity {
    let font = font(world, size);
    world.spawn((Part, Text::new(text), font, TextColor(color(rgba)), Pickable::IGNORE, ChildOf(parent))).id()
}

/// The text a widget's value is drawn as.
fn value_text(w: &UiWidget) -> String {
    match (&w.kind, &w.value) {
        (WidgetKind::Text | WidgetKind::Hint, UiValue::Text(t)) => t.clone(),
        (WidgetKind::Bar { .. }, UiValue::Ratio { value, max }) => {
            let numbers = format!("{}/{}", format_number(value.round()), format_number(max.round()));
            if w.label.is_empty() { numbers } else { format!("{}  {numbers}", w.label) }
        }
        (WidgetKind::Cooldown, UiValue::Cooldown { left }) => {
            let state = if *left <= 0.0 { "ready".to_owned() } else { format!("{left:.1}s") };
            if w.label.is_empty() { state } else { format!("{}: {state}", w.label) }
        }
        (WidgetKind::Compass, UiValue::Heading { deg, marks }) => {
            let mut text = format!("{} ({:.0} deg)", compass_point(*deg), deg);
            for m in marks {
                text.push_str(&format!("  |  {} {} {:.0}m", m.label, compass_point(m.bearing), m.distance));
            }
            text
        }
        (WidgetKind::Slider { .. }, UiValue::Number(v)) => format_number(*v),
        (WidgetKind::Toggle, UiValue::Flag(on)) => if *on { "[x]" } else { "[ ]" }.to_owned(),
        (WidgetKind::Choice { .. }, UiValue::Choice(v)) => format!("< {} >", w.kind.option_label(v)),
        _ => String::new(),
    }
}

/// The share of a bar or slider that is filled, in percent.
fn fill(w: &UiWidget) -> f32 {
    let share = match (&w.kind, &w.value) {
        (WidgetKind::Bar { .. }, UiValue::Ratio { value, max }) if *max > 0.0 => value / max,
        (WidgetKind::Slider { min, max, .. }, UiValue::Number(v)) if max > min => (v - min) / (max - min),
        _ => 0.0,
    };
    share.clamp(0.0, 1.0) * 100.0
}

/// A value changed and nothing else: the fill and the value text follow.
fn update_value(world: &mut World, e: Entity, w: &UiWidget) {
    let (fill_part, value_part) = world.get::<Parts>(e).map_or((None, None), |p| (p.fill, p.value));
    if let Some(f) = fill_part
        && let Some(mut node) = world.get_mut::<Node>(f)
    {
        node.width = Val::Percent(fill(w));
    }
    let text = value_text(w);
    match value_part {
        Some(p) => {
            if let Some(mut t) = world.get_mut::<Text>(p) {
                t.0 = text;
            }
        }
        None => {
            if let Some(mut t) = world.get_mut::<Text>(e) {
                t.0 = text;
            }
        }
    }
    if let Some(mut a11y) = world.get_mut::<AccessibilityNode>(e) {
        describe(&mut a11y.0, w);
    }
}

/// A screen reader's view of a widget's value.
fn describe(node: &mut A11yNode, w: &UiWidget) {
    match (&w.kind, &w.value) {
        (WidgetKind::Bar { .. }, UiValue::Ratio { value, max }) => {
            node.set_numeric_value(f64::from(*value));
            node.set_min_numeric_value(0.0);
            node.set_max_numeric_value(f64::from(*max));
        }
        (WidgetKind::Slider { min, max, step }, UiValue::Number(v)) => {
            node.set_numeric_value(f64::from(*v));
            node.set_min_numeric_value(f64::from(*min));
            node.set_max_numeric_value(f64::from(*max));
            if *step > 0.0 {
                node.set_numeric_value_step(f64::from(*step));
            }
        }
        (WidgetKind::Toggle, UiValue::Flag(on)) => node.set_toggled(if *on { Toggled::True } else { Toggled::False }),
        (WidgetKind::Text | WidgetKind::Hint, UiValue::Text(t)) => node.set_label(t.clone()),
        (WidgetKind::Choice { .. }, UiValue::Choice(v)) => node.set_value(w.kind.option_label(v).to_owned()),
        _ => {
            let text = value_text(w);
            if !text.is_empty() {
                node.set_value(text);
            }
        }
    }
}

fn role(w: &UiWidget, screen: Option<Layer>) -> Role {
    match (&w.kind, screen) {
        (_, Some(Layer::Hud)) => Role::Group,
        (_, Some(_)) => Role::Dialog,
        (WidgetKind::Panel { .. }, _) => Role::Group,
        (WidgetKind::List, _) => Role::List,
        (WidgetKind::Grid { .. }, _) => Role::Grid,
        (WidgetKind::Tabs, _) => Role::TabList,
        (WidgetKind::Text | WidgetKind::Compass | WidgetKind::Cooldown | WidgetKind::Compare, _) => Role::Label,
        (WidgetKind::Hint, _) => Role::Status,
        (WidgetKind::Button { .. }, _) => Role::Button,
        (WidgetKind::Bar { .. }, _) => Role::ProgressIndicator,
        (WidgetKind::Allies, _) => Role::List,
        (WidgetKind::Slider { .. }, _) => Role::Slider,
        (WidgetKind::Toggle, _) => Role::CheckBox,
        (WidgetKind::Choice { .. }, _) => Role::ComboBox,
    }
}

/// Draws `e` from scratch: its node and colours, its parts, its accessibility node.
fn redraw(world: &mut World, e: Entity, state: &Drawn) {
    if let Some(old) = world.entity_mut(e).take::<Parts>() {
        for p in old.all {
            if let Ok(p) = world.get_entity_mut(p) {
                p.despawn();
            }
        }
    }
    let t = theme(world);
    let w = &state.widget;
    let screen = world.get::<UiScreen>(e).map(|s| s.def.layer);
    let text_rgba = if w.enabled { t.color(&t.text) } else { t.color(&t.dim_text) };
    let gap = Val::Px(t.spacing);
    let display = if state.shown { Display::Flex } else { Display::None };
    let column = |flex_direction| Node { display, flex_direction, row_gap: gap, column_gap: gap, ..default() };
    let mut parts = Parts::default();
    let mut first: Vec<Entity> = Vec::new();
    match &w.kind {
        WidgetKind::Panel { layout } => {
            let direction = if *layout == Layout::Row { FlexDirection::Row } else { FlexDirection::Column };
            let mut node = column(direction);
            if let Some(layer) = screen {
                node.padding = UiRect::all(Val::Px(t.padding));
                node.border_radius = BorderRadius::all(Val::Px(t.radius));
                if layer != Layer::Hud {
                    world.entity_mut(e).insert(BackgroundColor(color(t.color(&t.panel))));
                }
            }
            world.entity_mut(e).insert(node);
            if !w.label.is_empty() {
                let size = if screen.is_some() { |t: &ThemeDef| t.title_size } else { |t: &ThemeDef| t.font_size };
                first.push(text_part(world, e, w.label.clone(), size, text_rgba));
            }
        }
        WidgetKind::List => {
            world.entity_mut(e).insert(column(FlexDirection::Column));
            if !w.label.is_empty() {
                first.push(text_part(world, e, w.label.clone(), |t| t.font_size, t.color(&t.dim_text)));
            }
        }
        WidgetKind::Grid { columns } => {
            let node = Node { display: if state.shown { Display::Grid } else { Display::None }, grid_template_columns: vec![RepeatedGridTrack::flex(*columns as u16, 1.0)], row_gap: gap, column_gap: gap, ..default() };
            world.entity_mut(e).insert(node);
            if !w.label.is_empty() {
                first.push(text_part(world, e, w.label.clone(), |t| t.font_size, t.color(&t.dim_text)));
            }
        }
        WidgetKind::Tabs => {
            world.entity_mut(e).insert(column(FlexDirection::Column));
            let header = world.spawn((Part, Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(t.spacing * 2.0), ..default() }, ChildOf(e))).id();
            let page = if let UiValue::Page(i) = w.value { i } else { 0 };
            let names: Vec<String> = widget_children(world, e)
                .iter()
                .map(|p| world.get::<UiWidget>(*p).map(|pw| if pw.label.is_empty() { pw.path.rsplit('/').next().unwrap_or("").to_owned() } else { pw.label.clone() }).unwrap_or_default())
                .collect();
            for (n, name) in names.into_iter().enumerate() {
                let rgba = if n == page { t.color(&t.accent) } else { t.color(&t.dim_text) };
                text_part(world, header, name, |t| t.font_size, rgba);
            }
            first.push(header);
        }
        WidgetKind::Text | WidgetKind::Hint | WidgetKind::Cooldown | WidgetKind::Compass => {
            let size = if w.kind == WidgetKind::Hint { |t: &ThemeDef| t.hint_size } else { |t: &ThemeDef| t.font_size };
            let rgba = if w.kind == WidgetKind::Hint { t.color(&t.dim_text) } else { text_rgba };
            let font = font(world, size);
            world.entity_mut(e).insert((Node { display, ..default() }, Text::new(value_text(w)), font, TextColor(color(rgba))));
        }
        WidgetKind::Button { .. } => {
            let node = Node {
                display,
                min_width: Val::Px(t.button_width),
                padding: UiRect::axes(Val::Px(t.padding), Val::Px(t.padding * 0.5)),
                justify_content: JustifyContent::Center,
                border_radius: BorderRadius::all(Val::Px(t.radius)),
                ..default()
            };
            world.entity_mut(e).insert((node, BackgroundColor(color(t.color(&t.button)))));
            text_part(world, e, w.label.clone(), |t| t.font_size, text_rgba);
        }
        WidgetKind::Bar { color: bar } => {
            world.entity_mut(e).insert(Node { display, flex_direction: FlexDirection::Column, row_gap: Val::Px(2.0), ..default() });
            let label = text_part(world, e, value_text(w), |t| t.hint_size, text_rgba);
            let track = world
                .spawn((Part, Node { width: Val::Px(t.bar_width), height: Val::Px(t.bar_height), ..default() }, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)), ChildOf(e)))
                .id();
            let fill_node = Node { width: Val::Percent(fill(w)), height: Val::Percent(100.0), ..default() };
            let filled = world.spawn((Part, fill_node, BackgroundColor(color(t.bar_color(bar.as_deref()))), ChildOf(track))).id();
            parts.fill = Some(filled);
            parts.value = Some(label);
            parts.all.push(track);
            parts.all.push(label);
        }
        WidgetKind::Slider { .. } => {
            world.entity_mut(e).insert(Node { display, flex_direction: FlexDirection::Row, column_gap: gap, align_items: AlignItems::Center, ..default() });
            let label = text_part(world, e, w.label.clone(), |t| t.font_size, text_rgba);
            let track = world
                .spawn((Part, Node { width: Val::Px(t.bar_width * 0.6), height: Val::Px(t.bar_height), ..default() }, BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)), ChildOf(e)))
                .id();
            let fill_node = Node { width: Val::Percent(fill(w)), height: Val::Percent(100.0), ..default() };
            let filled = world.spawn((Part, fill_node, BackgroundColor(color(t.color(&t.accent))), ChildOf(track))).id();
            let value = text_part(world, e, value_text(w), |t| t.font_size, text_rgba);
            parts.fill = Some(filled);
            parts.value = Some(value);
            parts.all.extend([label, track, value]);
        }
        WidgetKind::Toggle | WidgetKind::Choice { .. } => {
            world.entity_mut(e).insert(Node { display, flex_direction: FlexDirection::Row, column_gap: gap, align_items: AlignItems::Center, ..default() });
            let label = text_part(world, e, w.label.clone(), |t| t.font_size, text_rgba);
            let value = text_part(world, e, value_text(w), |t| t.font_size, t.color(&t.accent));
            parts.value = Some(value);
            parts.all.extend([label, value]);
        }
        WidgetKind::Allies => {
            world.entity_mut(e).insert(column(FlexDirection::Column));
            if let UiValue::Allies(allies) = &w.value {
                for a in allies {
                    let down = if a.down { "  (down)" } else { "" };
                    let line = format!("{}  {}/{}{down}", a.name, format_number(a.health.round()), format_number(a.max.round()));
                    let rgba = if a.down { t.color(&t.dim_text) } else { text_rgba };
                    let p = text_part(world, e, line, |t| t.hint_size, rgba);
                    parts.all.push(p);
                }
            }
        }
        WidgetKind::Compare => {
            world.entity_mut(e).insert(column(FlexDirection::Column));
            if !w.label.is_empty() {
                first.push(text_part(world, e, w.label.clone(), |t| t.font_size, text_rgba));
            }
            if let UiValue::Compare(rows) = &w.value {
                for r in rows {
                    let d = r.after - r.before;
                    let sign = if d > 0.0 { "+" } else { "" };
                    let line = format!("{}  {} -> {}  ({sign}{})", r.stat, format_number(r.before), format_number(r.after), format_number(d));
                    let rgba = match d.partial_cmp(&0.0) {
                        Some(std::cmp::Ordering::Greater) => [110, 200, 110, 255],
                        Some(std::cmp::Ordering::Less) => [220, 100, 90, 255],
                        _ => text_rgba,
                    };
                    let p = text_part(world, e, line, |t| t.font_size, rgba);
                    parts.all.push(p);
                }
            }
        }
    }
    if !first.is_empty() {
        world.entity_mut(e).insert_children(0, &first);
        parts.all.extend(first);
    }
    // The button's label and other parts that are text only.
    if let Some(children) = world.get::<Children>(e) {
        for c in children.iter().collect::<Vec<_>>() {
            if world.get::<Part>(c).is_some() && !parts.all.contains(&c) {
                parts.all.push(c);
            }
        }
    }
    if state.focused {
        world.entity_mut(e).insert(Outline::new(Val::Px(2.0), Val::Px(2.0), color(t.color(&t.focus))));
    } else {
        world.entity_mut(e).remove::<Outline>();
    }
    let mut a11y = A11yNode::new(role(w, screen));
    if !w.label.is_empty() {
        a11y.set_label(w.label.clone());
    }
    if !w.enabled {
        a11y.set_disabled();
    }
    describe(&mut a11y, w);
    world.entity_mut(e).insert((AccessibilityNode(a11y), parts));
}

/// Screen readers follow the UI's focus (Bevy's `InputFocus`), when it is on a widget or nowhere.
fn sync_focus(world: &mut World, focus: Option<Entity>) {
    let Some(current) = world.get_resource::<InputFocus>().map(InputFocus::get) else { return };
    let ours = current.is_none_or(|c| world.get::<UiWidget>(c).is_some());
    if !ours || current == focus {
        return;
    }
    let mut input = world.resource_mut::<InputFocus>();
    match focus {
        Some(e) => input.set(e, FocusCause::Navigated),
        None => input.clear(),
    }
}

/// A click on a widget that takes input presses it, as `ui_click` (recorded like any call).
fn on_click(mut click: On<PointerClick>, widgets: Query<&UiWidget>, mut commands: Commands) {
    let Ok(widget) = widgets.get(click.entity) else { return };
    if !widget.kind.focusable() {
        return;
    }
    click.propagate(false);
    let path = widget.path.clone();
    commands.queue(move |world: &mut World| {
        let out = ai_core::call_tool(world, crate::INPUT_AGENT, "ui_click", json!({"id": path}));
        if !out.ok {
            debug!("ui: click on {path}: {}", out.result);
        }
    });
}

fn on_over(over: On<PointerOver>, widgets: Query<&UiWidget>, look: Res<Look>, mut colors: Query<&mut BackgroundColor>) {
    if widgets.get(over.entity).is_ok_and(|w| matches!(w.kind, WidgetKind::Button { .. }) && w.enabled)
        && let Ok(mut bg) = colors.get_mut(over.entity)
    {
        bg.0 = color(look.theme.color(&look.theme.button_hover));
    }
}

fn on_out(out: On<PointerOut>, widgets: Query<&UiWidget>, look: Res<Look>, mut colors: Query<&mut BackgroundColor>) {
    if widgets.get(out.entity).is_ok_and(|w| matches!(w.kind, WidgetKind::Button { .. }))
        && let Ok(mut bg) = colors.get_mut(out.entity)
    {
        bg.0 = color(look.theme.color(&look.theme.button));
    }
}
