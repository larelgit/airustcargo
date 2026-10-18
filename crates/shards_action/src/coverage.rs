//! The running action and the active statuses in an entity's coverage signature
//! (`ai_core::coverage`), and what content declares for the `coverage` tool to compare with.

use ai_core::AiAppExt;
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::{ActionDef, ActionState, Stamina, StatusDef, Statuses};

/// The part's value of a free fighter. An action cannot be called this in content.
pub const IDLE: &str = "idle";

/// Where stamina stands, in a few words: a resource at its extremes is a state worth reaching
/// (`below_zero` is out of range; the engine's invariants name it, and coverage shows whether a session ever got there).
fn stamina_level(current: f32, max: f32) -> &'static str {
    match current {
        c if c < 0.0 => "below_zero",
        c if c <= 0.0 => "empty",
        c if c < max * 0.25 => "low",
        c if c < max * 0.6 => "mid",
        c if c < max => "high",
        _ => "full",
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_coverage_part("action", |world, entity| {
        let state = world.get::<ActionState>(entity)?;
        Some(match &state.current {
            Some(running) => format!("{}:{}", running.action, running.phase),
            None => IDLE.to_owned(),
        })
    })
    .add_coverage_part("statuses", |world, entity| {
        let statuses = world.get::<Statuses>(entity)?;
        let mut active: Vec<&str> = statuses.0.iter().filter(|s| s.active > 0.0).map(|s| s.id.as_str()).collect();
        active.sort_unstable();
        (!active.is_empty()).then(|| active.join(","))
    })
    .add_coverage_part("stamina", |world, entity| {
        let s = world.get::<Stamina>(entity)?;
        Some(stamina_level(s.current, s.max).to_owned())
    })
    .add_coverage_catalog(
        "actions",
        "action",
        |world| world.get_resource::<ContentDb>().map_or_else(Vec::new, |db| db.ids::<ActionDef>().map(str::to_owned).collect()),
        |value| match value.split_once(':') {
            Some((action, _)) => vec![action.to_owned()],
            None => Vec::new(),
        },
    )
    .add_coverage_catalog(
        "statuses",
        "statuses",
        |world| world.get_resource::<ContentDb>().map_or_else(Vec::new, |db| db.ids::<StatusDef>().map(str::to_owned).collect()),
        |value| value.split(',').map(str::to_owned).collect(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamina_levels_name_the_extremes() {
        assert_eq!(stamina_level(-3.0, 100.0), "below_zero");
        assert_eq!(stamina_level(0.0, 100.0), "empty");
        assert_eq!(stamina_level(10.0, 100.0), "low");
        assert_eq!(stamina_level(40.0, 100.0), "mid");
        assert_eq!(stamina_level(99.0, 100.0), "high");
        assert_eq!(stamina_level(100.0, 100.0), "full");
    }

    #[test]
    fn a_value_names_the_action_it_reached_and_idle_reaches_none() {
        let reached: fn(&str) -> Vec<String> = |value| match value.split_once(':') {
            Some((action, _)) => vec![action.to_owned()],
            None => Vec::new(),
        };
        assert_eq!(reached("heavy:windup"), ["heavy"]);
        assert!(reached(IDLE).is_empty());
    }
}
