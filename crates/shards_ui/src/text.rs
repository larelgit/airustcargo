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
use crate::model::{focused, on_shown_page, top_screen, walk, widget_children, UiScreen, UiStack, UiValue, UiWidget, WidgetKind};
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
            let _ = write!(line, " \"{t}\"");
        }
        (WidgetKind::Hint, UiValue::Text(t)) if t.is_empty() => line.push_str(" (hidden: nothing to say)"),
        (WidgetKind::Hint, UiValue::Text(t)) => {
            let _ = write!(line, " \"{t}\"");
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
                let _ = write!(line, " · {} {} {:.0}m", m.label, compass_point(m.bearing), m.distance);
            }
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
        (WidgetKind::Choice { options }, UiValue::Choice(v)) => {
            let _ = write!(line, " {v} ({})", options.join("|"));
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
        _ => {}
    }
    if let Some(key) = &w.setting {
        let _ = write!(line, " [{key}]");
    }
    if let Some(bind) = &w.bind {
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
    if let Some(t) = &s.def.title {
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
