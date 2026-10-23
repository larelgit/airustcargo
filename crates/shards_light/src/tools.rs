//! The module's tool: `lights`, a query of what the picture's lights and grade are doing now.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::db::closest;
use shards_content::ContentDb;
use shards_motion::camera::{picture_clock, CameraTuning, Grading, GradingBlend, GradingDef};

use crate::{update_state, Flicker, LightDef, LightInfo, LightState};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "lights", Scope::Player, ToolKind::Query, lights);
}

/// What the picture's lights and colour grade are doing now: the content lights by kind, who flickers and how bright
/// each is at this moment, who casts a shadow and why the others do not, and the grade the camera shows with the
/// progress of its blend toward or away from a grading volume. Pictures only: it is the same headless, from the
/// simulation's clock.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LightsArgs {
    /// Only the lights of this `light` definition (`id=sodium`).
    pub id: Option<String>,
    /// How many lights to list (default 24).
    pub limit: Option<usize>,
}

fn at(v: Vec3) -> String {
    format!("({:.1}, {:.1}, {:.1})", v.x, v.y, v.z)
}

fn flicker_text(light: &LightInfo, def: &LightDef) -> String {
    match &def.flicker {
        Flicker::None => "steady".to_owned(),
        Flicker::Pattern { pattern, rate } => {
            let step = light.step.unwrap_or(0);
            let letter = pattern.chars().nth(step).unwrap_or('m');
            format!("pattern `{pattern}` at {rate}/s, step {step} = {letter}: {:.0} % ({:.0} lm)", light.level * 100.0, light.lumens)
        }
        Flicker::Noise { rate, depth } => format!("noise {rate}/s depth {depth}: {:.0} % ({:.0} lm)", light.level * 100.0, light.lumens),
    }
}

fn shadow_text(light: &LightInfo, state: &LightState) -> &'static str {
    match (light.wants_shadow, light.casts) {
        (true, true) => "shadow: yes",
        (true, false) if state.budget == 0 => "shadow: no (budget 0)",
        (true, false) => "shadow: no (over the budget)",
        (false, _) => "shadow: not asked",
    }
}

/// The fields of a grade that are not neutral, as words.
pub fn grade_text(grade: &Grading) -> String {
    let neutral = Grading::default();
    let fields = [
        ("exposure", grade.exposure, neutral.exposure),
        ("temperature", grade.temperature, neutral.temperature),
        ("tint", grade.tint, neutral.tint),
        ("hue_deg", grade.hue_deg, neutral.hue_deg),
        ("saturation", grade.saturation, neutral.saturation),
        ("contrast", grade.contrast, neutral.contrast),
        ("lift", grade.lift, neutral.lift),
        ("gamma", grade.gamma, neutral.gamma),
        ("gain", grade.gain, neutral.gain),
    ];
    let changed: Vec<String> = fields.iter().filter(|(_, v, n)| (v - n).abs() > 0.0005).map(|(name, v, _)| format!("{name} {v:.3}")).collect();
    if changed.is_empty() { "neutral".to_owned() } else { changed.join(", ") }
}

/// The `grading:` lines: the grade the camera shows and where its blend is.
fn grading_lines(world: &World) -> Vec<String> {
    let (Some(blend), Some(fixed)) = (world.get_resource::<GradingBlend>(), world.get_resource::<Time<Fixed>>()) else {
        return vec!["grading: not in this game".to_owned()];
    };
    let now = picture_clock(fixed);
    let db = world.get_resource::<ContentDb>();
    let resolve = |id: &str| db.and_then(|db| db.peek::<GradingDef>(id));
    let base = world.get_resource::<CameraTuning>().and_then(|t| t.picture.grading.as_ref());
    let progress = blend.progress(now);
    let head = match (blend.volume(), blend.is_blending(now)) {
        (Some(volume), true) => format!("grading: blending toward `{volume}`, {:.0} % done", progress * 100.0),
        (Some(volume), false) => format!("grading: holding `{volume}` (blend done)"),
        (None, true) => format!("grading: blending back to the camera's own, {:.0} % done", progress * 100.0),
        (None, false) => "grading: the camera's own".to_owned(),
    };
    let shown = blend.current(now, base, &resolve).or_else(|| base.cloned());
    let line = match &shown {
        Some(grade) => format!("  shows: {}", grade_text(grade)),
        None => "  shows: no grade of the content (the camera keeps whatever the game gave it)".to_owned(),
    };
    let mut out = vec![head, line];
    if base.is_some() {
        out.push("  the camera content's `picture.grading` is its own grade".to_owned());
    }
    out.push(format!("  the target has changed {} times", blend.changes()));
    out
}

fn lights(In(call): In<Call<LightsArgs>>, world: &mut World) -> ToolResult {
    // The same refresh a frame makes, so a stepped game with no frame in between answers for now.
    world.run_system_cached(update_state).map_err(|e| format!("could not refresh the lights: {e}"))?;
    let state = world.resource::<LightState>();
    let LightsArgs { id, limit } = call.args;
    if let Some(id) = &id {
        let known: Vec<&str> = state.lights.iter().map(|l| l.id.as_str()).collect();
        if !known.contains(&id.as_str()) && !state.lights.is_empty() {
            let hint = closest(id, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            return Err(format!("no light of definition `{id}`{hint}; the lights here: {}", dedup(&known)));
        }
    }
    let shown: Vec<&LightInfo> = state.lights.iter().filter(|l| id.as_deref().is_none_or(|id| l.id == id)).collect();

    let points = state.lights.iter().filter(|l| l.def.as_ref().is_some_and(|d| d.kind.name() == "point")).count();
    let spots = state.lights.iter().filter(|l| l.def.as_ref().is_some_and(|d| d.kind.name() == "spot")).count();
    let undefined = state.lights.iter().filter(|l| l.def.is_none()).count();
    let wanting = state.lights.iter().filter(|l| l.wants_shadow).count();
    let mut kinds = format!("point {points}, spot {spots}");
    if undefined > 0 {
        kinds.push_str(&format!(", without a definition {undefined}"));
    }
    let mut out = vec![format!(
        "lights: {} ({kinds}) · {} flicker · {wanting} want a shadow, {} cast one (budget {}: graphics.shadow_lights) · clock {:.3} s",
        state.lights.len(),
        state.flickering().count(),
        state.casting().count(),
        state.budget,
        state.clock,
    )];
    out.push(match state.viewer {
        Some((place, what)) => format!("viewer: the {what} at {}", at(place)),
        None => "viewer: none (no camera and no local player: distances are unknown)".to_owned(),
    });
    let limit = limit.unwrap_or(24);
    for light in shown.iter().take(limit) {
        let name = if light.name.is_empty() { String::new() } else { format!(" \"{}\"", light.name) };
        let distance = light.distance.map_or(String::new(), |d| format!(" {d:.1} m away"));
        match &light.def {
            Some(def) => out.push(format!(
                "  {}{name} {} at {}{distance} · {:.0} lm of {:.0} · {} · {}",
                light.id,
                def.kind.name(),
                at(light.position),
                light.lumens,
                def.intensity,
                flicker_text(light, def),
                shadow_text(light, state),
            )),
            None => out.push(format!("  {}{name} at {}: no `light` definition `{}` (content_list kind=light)", light.id, at(light.position), light.id)),
        }
    }
    if shown.len() > limit {
        out.push(format!("  … and {} more (limit={limit})", shown.len() - limit));
    }
    out.extend(grading_lines(world));
    Ok(out.join("\n"))
}

/// The distinct ids, comma separated.
fn dedup(ids: &[&str]) -> String {
    let mut ids = ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids.join(", ")
}
