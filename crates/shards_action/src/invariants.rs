//! The engine rules `action.resources_in_range`, `action.running_sane`, `action.protection_sane`,
//! `action.knockback_sane` and `action.extras_follow_run`.

use ai_core::{with_query, AiId, Offenders};
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::def::ActionDef;
use crate::fighter::{Fighter, Health, Knockback, Poise, Protection, Stamina};
use crate::machine::{ActionState, RunExtras};

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
    // A component that is not registered has no entities to judge: its query is skipped.
    with_query::<(&AiId, &Health), (), _>(world, |query| {
        for (id, h) in query.iter(world) {
            if let Some(what) = out_of_range("Health", h.current, h.max) {
                bad.note(*id, || what);
            }
        }
    });
    with_query::<(&AiId, &Stamina), (), _>(world, |query| {
        for (id, s) in query.iter(world) {
            if let Some(what) = out_of_range("Stamina", s.current, s.max) {
                bad.note(*id, || what);
            }
        }
    });
    with_query::<(&AiId, &Poise), (), _>(world, |query| {
        for (id, p) in query.iter(world) {
            if let Some(what) = out_of_range("Poise", p.current, p.max) {
                bad.note(*id, || what);
            }
        }
    });
    bad.report()
}

/// `None` while `0 <= current <= max` with a finite `max > 0` (the comparison fails for NaN).
fn out_of_range(name: &str, current: f32, max: f32) -> Option<String> {
    let fits = max.is_finite() && max > 0.0 && current >= 0.0 && current <= max;
    (!fits).then(|| format!("{name}.current = {current} outside 0..={max}"))
}

/// The engine rule `action.running_sane`: a running action exists in content, its phase is one
/// of the action's phases, and its clock `t` is between 0 and its length plus one tick. The length
/// is the run's own when that is longer than the definition's: miss recovery and
/// `extend_recovery` make a run last beyond its phases (`Running::length`).
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
    let mut bad = Offenders::default();
    with_query::<(&AiId, &ActionState), With<Fighter>, _>(world, |query| {
        for (id, state) in query.iter(world) {
            let Some(run) = &state.current else { continue };
            let Some(def) = db.get::<ActionDef>(&run.action) else {
                bad.note(*id, || format!("runs action `{}`, which is not in content", run.action));
                continue;
            };
            if !def.phases.is_empty() && !def.phases.iter().any(|p| p.0 == run.phase) {
                bad.note(*id, || format!("action `{}` is in phase `{}`, which is not one of its phases", run.action, run.phase));
            } else if !(run.t >= 0.0 && run.t <= run.length.max(def.length()) + tick) {
                let length = run.length.max(def.length());
                bad.note(*id, || format!("action `{}` has t = {} outside 0..={} (+ one tick)", run.action, run.t, length));
            }
        }
    })?;
    bad.report()
}

/// The engine rule `action.protection_sane`: every [`Protection`] holds finite timers, none below 0.
///
/// `grant_protection` takes the longer of two times and `tick_protection` clamps at 0, so a
/// violation names a writer that put a timer there by hand (a NaN stays a NaN through the countdown
/// and would shield its fighter for ever).
pub fn protection_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Protection), (), _>(world, |query| {
        for (id, p) in query.iter(world) {
            if !(p.reactions.is_finite() && p.reactions >= 0.0 && p.poise.is_finite() && p.poise >= 0.0) {
                bad.note(*id, || {
                    format!("Protection reactions = {}, poise = {}: timers are finite and >= 0", p.reactions, p.poise)
                });
            }
        }
    });
    bad.report()
}

/// The engine rule `action.knockback_sane`: every [`Knockback`] has a finite flat velocity (`y` is 0)
/// and a finite `decel` above 0.
///
/// A push moves the body on the ground plane (the controller owns the vertical axis), and
/// `ReactionDef::decel` is validated above 0 because any hit's `knockback` may start a push: a push
/// that does not slow down would never stop, and a vertical component would be thrown into the
/// displacement the machine writes.
pub fn knockback_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Knockback), (), _>(world, |query| {
        for (id, k) in query.iter(world) {
            let v = k.velocity;
            if !(v.is_finite() && v.y == 0.0 && k.decel.is_finite() && k.decel > 0.0) {
                bad.note(*id, || {
                    format!("Knockback velocity = {v}, decel = {}: a flat finite velocity and a deceleration above 0", k.decel)
                });
            }
        }
    });
    bad.report()
}

/// The engine rule `action.extras_follow_run`: a [`RunExtras`] belongs to the run its fighter is
/// running (same `instance`), and its `extra` and `closed` are finite numbers `>= 0`.
///
/// The machine puts extras in when a run that needs them starts and takes them out when it ends,
/// on every path a run ends by (its time runs out, a cancel, a forced action, a death, a revive,
/// a release, a hot reload); extras left behind would hand a later run a stale hold, landing
/// flag or extra recovery. `extra` only grows by `miss_recovery` (validated `>= 0`) and
/// [`extend_recovery`](crate::extend_recovery) (which refuses a negative number), and `closed` only
/// by the approach the closing cap lets through (`> 0`).
pub fn extras_follow_run(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &RunExtras, Option<&ActionState>), (), _>(world, |query| {
        for (id, x, state) in query.iter(world) {
            let running = state.and_then(|s| s.current.as_ref());
            if running.is_none_or(|r| r.instance != x.instance) {
                let now = running.map_or("no action".to_owned(), |r| format!("instance {} ({})", r.instance, r.action));
                bad.note(*id, || format!("RunExtras of instance {} while it runs {now}", x.instance));
            } else if !(x.extra.is_finite() && x.extra >= 0.0 && x.closed.is_finite() && x.closed >= 0.0) {
                bad.note(*id, || format!("RunExtras extra = {}, closed = {}: finite numbers >= 0", x.extra, x.closed));
            }
        }
    });
    bad.report()
}
