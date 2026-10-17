//! The engine rules `action.resources_in_range` and `action.running_sane`.

use ai_core::{AiId, Offenders};
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::def::ActionDef;
use crate::fighter::{Fighter, Health, Poise, Stamina};
use crate::machine::ActionState;

/// The engine rule `action.resources_in_range`: `Health`, `Stamina` and `Poise` hold finite
/// numbers with `max > 0` and `0 <= current <= max`.
///
/// Every writer in the engine clamps (damage floors at 0, healing and regeneration stop at
/// `max`, `Stamina::spend` floors at 0, a broken poise is reset to 0 or `max` before the tick
/// ends), and nothing in the engine lifts a value above its maximum, so a violation names a
/// writer that forgot to. Dead bodies are checked like the living: a dead body has
/// `Health.current = 0`, which is in range. A game that wants a buff above `max` raises `max`
/// with it, and an indestructible body has no `Health` rather than an infinite one. A game that
/// really needs otherwise has to turn the engine rules off.
pub fn resources_in_range(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    if let Some(mut query) = world.try_query::<(&AiId, &Health)>() {
        for (id, h) in query.iter(world) {
            if let Some(what) = out_of_range("Health", h.current, h.max) {
                bad.note(*id, || what);
            }
        }
    }
    if let Some(mut query) = world.try_query::<(&AiId, &Stamina)>() {
        for (id, s) in query.iter(world) {
            if let Some(what) = out_of_range("Stamina", s.current, s.max) {
                bad.note(*id, || what);
            }
        }
    }
    if let Some(mut query) = world.try_query::<(&AiId, &Poise)>() {
        for (id, p) in query.iter(world) {
            if let Some(what) = out_of_range("Poise", p.current, p.max) {
                bad.note(*id, || what);
            }
        }
    }
    bad.report()
}

/// `None` while `0 <= current <= max` with a finite `max > 0` (the comparison fails for NaN).
fn out_of_range(name: &str, current: f32, max: f32) -> Option<String> {
    let fits = max.is_finite() && max > 0.0 && current >= 0.0 && current <= max;
    (!fits).then(|| format!("{name}.current = {current} outside 0..={max}"))
}

/// The engine rule `action.running_sane`: a running action exists in content, its phase is one
/// of the action's phases, and its clock `t` is between 0 and its length plus one tick.
///
/// Checked for fighters (`Fighter`), the entities the action machine advances; an `ActionState`
/// on anything else is never advanced or cleared, so it says nothing. The machine clears an
/// action the tick its clock reaches the length and drops one whose definition vanished (a hot
/// reload) the same tick, so the extra tick is generous slack rather than a state the
/// machine uses. An action with no phases has no phase to compare. Without a `ContentDb` there
/// is nothing to look up and the rule passes. A co-op client holds predicted actions, and the
/// engine's rules do not run there.
pub fn running_sane(world: &World) -> Option<String> {
    let db = world.get_resource::<ContentDb>()?;
    let tick = world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.timestep().as_secs_f32());
    let mut query = world.try_query_filtered::<(&AiId, &ActionState), With<Fighter>>()?;
    let mut bad = Offenders::default();
    for (id, state) in query.iter(world) {
        let Some(run) = &state.current else { continue };
        let Some(def) = db.get::<ActionDef>(&run.action) else {
            bad.note(*id, || format!("runs action `{}`, which is not in content", run.action));
            continue;
        };
        if !def.phases.is_empty() && !def.phases.iter().any(|p| p.0 == run.phase) {
            bad.note(*id, || format!("action `{}` is in phase `{}`, which is not one of its phases", run.action, run.phase));
        } else if !(run.t >= 0.0 && run.t <= def.length() + tick) {
            bad.note(*id, || format!("action `{}` has t = {} outside 0..={} (+ one tick)", run.action, run.t, def.length()));
        }
    }
    bad.report()
}
