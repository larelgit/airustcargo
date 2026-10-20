//! The UI as text: what `ui_read` answers and the `UI` lines of `observe`.
//!
//! ```text
//! UI  pause "Paused" (menu, pauses the game) · focus pause/resume · ui_navigate, ui_click, ui_set
//!   pause/resume  button "Resume" → close ◀ focus
//!   pause/settings  button "Settings" → open settings
//! ```

use std::fmt::Write;

use bevy::prelude::*;

use crate::bind::live_value;
use crate::def::Layer;
use crate::model::{binding_state, focused, inputs_text, on_shown_page, top_screen, walk, widget_children, UiScreen, UiStack, UiValue, UiWidget, WidgetKind};
use crate::map::{MapArea, MapPoint, MapState, MapYou};
use crate::model::format_number;

/// At most this many widget lines of the screen on top in `observe`; `ui_read` has them all.
const OBSERVE_LINES: usize = 20;

/// One line for widget `e`: path, kind, label, live value, and what else an agent needs.
pub fn widget_line(world: &World, e: Entity, focus: Option<Entity>) -> String {
    let Some(w) = world.get::<UiWidget>(e) else { return String::new() };
    let value = live_value(world, w);
    let mut line = format!("{}  {}", w.path, w.kind.name());
    if !w.label.is_empty() {
        let _ = write!(line, " \"{}\"", w.label);
    }
    match (&w.kind, &value) {
        (WidgetKind::Text, UiValue::Text(t)) => {
            let _ = write!(line, " \"{}\"", crate::markup::plain(t));
        }
        (WidgetKind::Hint, UiValue::Text(t)) if t.is_empty() => line.push_str(" (hidden: nothing to say)"),
        (WidgetKind::Hint, UiValue::Text(t)) => {
            let _ = write!(line, " \"{}\"", crate::markup::plain(t));
        }
        (WidgetKind::Image { path, width, height }, _) => {
            let size = match (width, height) {
                (Some(w), Some(h)) => format!(" {}×{}", format_number(*w), format_number(*h)),
                (Some(w), None) => format!(" {} wide", format_number(*w)),
                (None, Some(h)) => format!(" {} high", format_number(*h)),
                (None, None) => String::new(),
            };
            let _ = write!(line, " {path}{size}");
        }
        (WidgetKind::Button { action }, _) => {
            let _ = write!(line, " → {action}");
        }
        (WidgetKind::Bar { .. }, UiValue::Ratio { value, max }) => {
            let _ = write!(line, " {}/{}", format_number(*value), format_number(*max));
        }
        (WidgetKind::Cooldown, UiValue::Cooldown { left }) if *left <= 0.0 => line.push_str(" ready"),
        (WidgetKind::Cooldown, UiValue::Cooldown { left }) => {
            let _ = write!(line, " {left:.1}s");
        }
        (WidgetKind::Compass, UiValue::Heading { deg, marks }) => {
            let _ = write!(line, " {deg:.0}° {}", compass_point(*deg));
            for m in marks {
                let category = m.category.as_ref().map(|c| format!(" ({c})")).unwrap_or_default();
                let _ = write!(line, " · {}{category} {} {:.0}m", m.label, compass_point(m.bearing), m.distance);
            }
        }
        (WidgetKind::Map { area, .. }, UiValue::Map { points, you }) => {
            let _ = write!(line, " {}", map_words(area, points, you.as_ref()));
        }
        (WidgetKind::Allies, UiValue::Allies(allies)) if allies.is_empty() => line.push_str(" (nobody else)"),
        (WidgetKind::Allies, UiValue::Allies(allies)) => {
            let each: Vec<String> = allies
                .iter()
                .map(|a| {
                    let down = if a.down { " down" } else { "" };
                    format!("{} {}/{}{down}", a.name, format_number(a.health), format_number(a.max))
                })
                .collect();
            let _ = write!(line, " {}", each.join(" · "));
        }
        (WidgetKind::Compare, UiValue::Compare(rows)) => {
            let each: Vec<String> = rows
                .iter()
                .map(|r| {
                    let d = r.after - r.before;
                    let sign = if d > 0.0 { "+" } else { "" };
                    format!("{} {} → {} ({sign}{})", r.stat, format_number(r.before), format_number(r.after), format_number(d))
                })
                .collect();
            let _ = write!(line, " {}", each.join(" · "));
        }
        (WidgetKind::Slider { min, max, step }, UiValue::Number(v)) => {
            let step = if *step > 0.0 { format!(" step {}", format_number(*step)) } else { String::new() };
            let _ = write!(line, " {} ({}..{}{step})", format_number(*v), format_number(*min), format_number(*max));
        }
        (WidgetKind::Toggle, UiValue::Flag(on)) => line.push_str(if *on { " on" } else { " off" }),
        (WidgetKind::Choice { options, .. }, UiValue::Choice(v)) => {
            let said = w.kind.option_label(v);
            let said = if said == v { String::new() } else { format!(" \"{said}\"") };
            let _ = write!(line, " {v}{said} ({})", options.join("|"));
        }
        (WidgetKind::Tabs, UiValue::Page(i)) => {
            let pages: Vec<String> = widget_children(world, e)
                .iter()
                .enumerate()
                .map(|(n, p)| {
                    let id = world.get::<UiWidget>(*p).map_or("", |pw| pw.path.rsplit('/').next().unwrap_or(""));
                    if n == *i { format!("[{id}]") } else { id.to_owned() }
                })
                .collect();
            let _ = write!(line, " {}", pages.join(" "));
        }
        (WidgetKind::Grid { columns }, _) => {
            let _ = write!(line, " {columns} columns");
        }
        (WidgetKind::Binding { .. }, UiValue::Bindings(bound)) => {
            let shown = w.kind.shown_inputs(&bound.inputs);
            let texts: Vec<&str> = shown.iter().map(|i| i.text.as_str()).collect();
            let _ = write!(line, " {}", inputs_text(shown));
            if !texts.is_empty() {
                let _ = write!(line, " = {}", texts.join(", "));
            }
        }
        (WidgetKind::Binding { .. }, UiValue::Text(why)) => {
            let _ = write!(line, " ({why})");
        }
        _ => {}
    }
    if let Some(key) = &w.setting {
        let _ = write!(line, " [{key}]");
    }
    if let WidgetKind::Binding { context, action, slot, .. } = &w.kind {
        // The setting it changes; the source is the engine's.
        let slot = slot.map(|s| format!(" slot {s}")).unwrap_or_default();
        let _ = write!(line, " [controls.bindings.{context}.{action}{slot}]");
        let (waits, note) = binding_state(world, e);
        if waits {
            let _ = write!(line, " · waiting for an input (Escape cancels; ui_set id={} value=Key(KeyK))", w.path);
        }
        if let Some(note) = note {
            let _ = write!(line, " · {note}");
        }
    } else if let Some(bind) = &w.bind {
        let _ = write!(line, " ← {bind}");
    }
    if !w.enabled {
        line.push_str(" (disabled)");
    }
    if focus == Some(e) {
        line.push_str(" ◀ focus");
    }
    line
}

/// `N`, `NE`, … for a bearing in degrees clockwise from north.
pub fn compass_point(deg: f32) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[((deg.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8]
}

/// `pause "Paused" (menu, pauses the game)`.
fn screen_title(world: &World, root: Entity) -> String {
    let Some(s) = world.get::<UiScreen>(root) else { return String::new() };
    let mut title = s.name.clone();
    // The root's label: the title in the language played.
    if let Some(t) = world.get::<UiWidget>(root).map(|w| &w.label).filter(|t| !t.is_empty()) {
        let _ = write!(title, " \"{t}\"");
    }
    let pauses = if s.def.pauses { ", pauses the game" } else { "" };
    let closable = if s.def.closable || s.def.layer == Layer::Hud { "" } else { ", back does not close it" };
    let _ = write!(title, " ({}{pauses}{closable})", s.def.layer.name());
    title
}

/// Every open screen, the one that takes input first, as indented widget lines; or just
/// screen `only`.
pub fn ui_tree(world: &World, only: Option<&str>) -> Result<String, String> {
    let stack = world.get_resource::<UiStack>().ok_or("this game has no UI")?;
    let top = top_screen(world);
    let focus = focused(world);
    let mut roots: Vec<Entity> = stack.screens().map(|(_, e)| e).collect();
    // The one that takes input first, then the newest down to the oldest.
    roots.reverse();
    roots.sort_by_key(|e| Some(*e) != top);
    if let Some(name) = only {
        roots.retain(|e| world.get::<UiScreen>(*e).is_some_and(|s| s.name == name));
        if roots.is_empty() {
            let open: Vec<&str> = stack.screens().map(|(n, _)| n).collect();
            return Err(format!("screen `{name}` is not open; open: {}", if open.is_empty() { "none".to_owned() } else { open.join(", ") }));
        }
    }
    if roots.is_empty() {
        return Ok("no screens are open".to_owned());
    }
    let mut out = String::new();
    if only.is_none() {
        let names: Vec<String> = roots
            .iter()
            .filter_map(|e| world.get::<UiScreen>(*e).map(|s| if Some(*e) == top { format!("{} ◀ takes input", s.name) } else { s.name.clone() }))
            .collect();
        let _ = writeln!(out, "SCREENS  {}", names.join(" · "));
    }
    for root in roots {
        let _ = writeln!(out, "{}", screen_title(world, root));
        let mut hidden_below: Option<usize> = None;
        for (e, depth) in walk(world, root).into_iter().skip(1) {
            if hidden_below.is_some_and(|d| depth > d) {
                continue;
            }
            hidden_below = None;
            let mut line = widget_line(world, e, focus);
            if !on_shown_page(world, e) {
                line.push_str(" (page not shown)");
                hidden_below = Some(depth);
            }
            let _ = writeln!(out, "{}{line}", "  ".repeat(depth));
        }
    }
    Ok(out.trim_end().to_owned())
}

/// `observe`: the screen that takes input, if a menu is open, with its widgets. Only the HUD
/// open: nothing (`ui_read` shows it).
pub(crate) fn observe_section(world: &World) -> Vec<String> {
    let Some(top) = top_screen(world) else { return Vec::new() };
    let focus = focused(world);
    let mut header = format!("UI  {}", screen_title(world, top));
    if let Some(path) = focus.and_then(|f| world.get::<UiWidget>(f)).map(|w| w.path.clone()) {
        let _ = write!(header, " · focus {path}");
    }
    header.push_str(" · ui_navigate, ui_click, ui_set");
    let mut lines = vec![header];
    let shown: Vec<Entity> = walk(world, top).into_iter().skip(1).map(|(e, _)| e).filter(|e| on_shown_page(world, *e)).collect();
    let listed: Vec<Entity> = shown
        .into_iter()
        .filter(|e| world.get::<UiWidget>(*e).is_some_and(|w| !w.kind.container() || !w.label.is_empty() || w.kind == WidgetKind::Tabs))
        .collect();
    for e in listed.iter().take(OBSERVE_LINES) {
        lines.push(format!("  {}", widget_line(world, *e, focus)));
    }
    if listed.len() > OBSERVE_LINES {
        lines.push(format!("  … {} more (ui_read)", listed.len() - OBSERVE_LINES));
    }
    lines
}

/// A map in words, as agents and screen readers get it: the viewer, then each place, a rumour as
/// the compass direction toward it from the viewer (from the map's middle without one).
pub(crate) fn map_words(area: &MapArea, points: &[MapPoint], you: Option<&MapYou>) -> String {
    let mut parts = Vec::new();
    if let Some(y) = you {
        parts.push(format!("you ({:.0}, {:.0}) facing {}", y.at.x, y.at.y, compass_point(y.deg)));
    }
    let from = you.map_or(Vec2::new(area.min.0 + area.max.0, area.min.1 + area.max.1) * 0.5, |y| y.at);
    for p in points {
        let category = p.category.as_ref().map(|c| format!(" ({c})")).unwrap_or_default();
        let at = match p.state {
            MapState::Rumor => {
                let to = p.at - from;
                compass_point(ops::atan2(to.x, -to.y).to_degrees().rem_euclid(360.0)).to_owned()
            }
            MapState::Discovered | MapState::Completed => format!("({:.0}, {:.0})", p.at.x, p.at.y),
        };
        parts.push(format!("{}{category} {} {at}", p.label, p.state.name()));
    }
    if points.is_empty() {
        parts.push("no places".to_owned());
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearings_have_points() {
        assert_eq!(compass_point(0.0), "N");
        assert_eq!(compass_point(44.0), "NE");
        assert_eq!(compass_point(90.0), "E");
        assert_eq!(compass_point(200.0), "S");
        assert_eq!(compass_point(-90.0), "W");
        assert_eq!(compass_point(359.0), "N");
    }
}
