//! The engine rules `action.resources_in_range`, `action.running_sane`, `action.protection_sane`,
//! `action.knockback_sane`, `action.extras_follow_run`, `action.barrier_sane`, `action.damage_taken_sane`, `action.pairs_match`, `action.memory_sane`, `action.leap_follows_run` and `action.ghost_follows_run`.

use ai_core::{with_query, AiId, AiIndex, Offenders, SimTick};
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::context::{GuardArm, Memory, Paired, Pairing};
use crate::body::{Barrier, DamageTaken};
use crate::def::ActionDef;
use crate::fighter::{Fighter, Health, Knockback, Poise, Protection, Stamina};
use crate::machine::{ActionState, LeapRun, RunExtras};
use shards_motion::Ghost;

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

/// The engine rule `action.barrier_sane`: every [`Barrier`] holds a finite `amount >= 0` and a finite
/// `left`.
///
/// `grant_barrier` takes the larger pool and a positive time, `hurt` empties the pool and removes the
/// component, and `tick_body_state` clamps `left` at 0 and removes the component with it, so a
/// violation names a writer that put a number there by hand (a NaN time would never run out, a
/// negative pool would heal what it absorbs).
pub fn barrier_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Barrier), (), _>(world, |query| {
        for (id, b) in query.iter(world) {
            if !(b.amount.is_finite() && b.amount >= 0.0 && b.left.is_finite()) {
                bad.note(*id, || format!("Barrier amount = {}, left = {}: a finite amount >= 0 and a finite time", b.amount, b.left));
            }
        }
    });
    bad.report()
}

/// The engine rule `action.pairs_match`: a pair's two marks agree. Every [`Pairing`] runs: its
/// attacker runs `Pairing.action` with `Pairing.instance`, and its victim, while it exists, holds
/// a [`Paired`] naming the attacker; every `Paired` names an attacker, while it exists, whose
/// `Pairing` names it. A side that is gone altogether (despawned in the tick) is left to
/// `tend_pairs`, which lets go of it on the next tick.
pub fn pairs_match(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    let index = world.get_resource::<AiIndex>()?;
    with_query::<(&AiId, &Pairing, Option<&ActionState>), (), _>(world, |query| {
        for (id, p, state) in query.iter(world) {
            let running = state.and_then(|s| s.current.as_ref());
            if running.is_none_or(|r| r.instance != p.instance || r.action != p.action) {
                let now = running.map_or("no action".to_owned(), |r| format!("{} instance {}", r.action, r.instance));
                bad.note(*id, || format!("Pairing of {} instance {} while it runs {now}", p.action, p.instance));
            } else if let Some(victim) = index.get(p.victim)
                && world.get::<Paired>(victim).is_none_or(|v| v.by != *id)
            {
                let held = world.get::<Paired>(victim).map_or("nothing".to_owned(), |v| format!("Paired by {}", v.by));
                bad.note(*id, || format!("Pairing names {} whose mark says {held}", p.victim));
            }
        }
    });
    with_query::<(&AiId, &Paired), (), _>(world, |query| {
        for (id, v) in query.iter(world) {
            if let Some(attacker) = index.get(v.by)
                && world.get::<Pairing>(attacker).is_none_or(|p| p.victim != *id)
            {
                let holds = world.get::<Pairing>(attacker).map_or("no Pairing".to_owned(), |p| format!("Pairing of {}", p.victim));
                bad.note(*id, || format!("Paired by {} which has {holds}", v.by));
            }
        }
    });
    bad.report()
}

/// The engine rule `action.damage_taken_sane`: every [`DamageTaken`] holds finite multipliers `>= 0`, finite
/// times, and its keys sorted and unique.
///
/// `set_damage_taken` inserts in key order and replaces a key, so a violation names a writer that
/// pushed an entry by hand: two entries under one key would multiply twice and make `clear` leave
/// one behind, and a NaN multiplier would poison every hit the fighter takes.
pub fn damage_taken_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &DamageTaken), (), _>(world, |query| {
        for (id, t) in query.iter(world) {
            let ok = |m: &crate::TakenMult| {
                m.mult.is_finite()
                    && m.mult >= 0.0
                    && m.left.is_none_or(f32::is_finite)
                    && m.then.is_none_or(|b| b.is_finite() && b >= 0.0 && m.left.is_some())
            };
            if let Some(m) = t.0.iter().find(|m| !ok(m)) {
                bad.note(*id, || {
                    format!(
                        "DamageTaken `{}` mult = {}, left = {:?}, then = {:?}: a finite mult >= 0, a finite time, and a base only under a window",
                        m.key, m.mult, m.left, m.then
                    )
                });
            } else if let Some(w) = t.0.windows(2).find(|w| w[0].key >= w[1].key) {
                bad.note(*id, || format!("DamageTaken keys `{}`, `{}` are not sorted and unique", w[0].key, w[1].key));
            }
        }
    });
    bad.report()
}

/// The engine rule `action.memory_sane`: a [`Memory`] holds finite times above 0, its keys sorted
/// and unique, and is not empty (an empty one is removed); a [`GuardArm`]'s press is not in the future.
pub fn memory_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Memory), (), _>(world, |query| {
        for (id, m) in query.iter(world) {
            if m.0.is_empty() {
                bad.note(*id, || "an empty Memory: it is removed with its last memory".to_owned());
            }
            if let Some(r) = m.0.iter().find(|r| !(r.left.is_finite() && r.left > 0.0)) {
                bad.note(*id, || format!("Memory `{}` left = {}: a finite time above 0", r.key, r.left));
            }
            if let Some(w) = m.0.windows(2).find(|w| w[0].key >= w[1].key) {
                bad.note(*id, || format!("Memory keys `{}`, `{}`: sorted and unique", w[0].key, w[1].key));
            }
        }
    });
    let now = world.get_resource::<SimTick>().map_or(u64::MAX, |t| t.0);
    with_query::<(&AiId, &GuardArm), (), _>(world, |query| {
        for (id, a) in query.iter(world) {
            if let Some(p) = a.pressed.filter(|p| *p > now) {
                bad.note(*id, || format!("GuardArm pressed at tick {p}, after tick {now}"));
            }
        }
    });
    bad.report()
}

/// The engine rule `action.leap_follows_run`: a [`LeapRun`] belongs to the run its fighter is
/// running (same `instance`), whose action leaps, and its `scale` is a finite number `>= 0`.
///
/// The machine puts it in when a run of a `leap` action starts and takes it out on every path a run
/// ends by, like [`RunExtras`]; one left behind would stretch a later leap's displacement by the
/// last one's landing.
pub fn leap_follows_run(world: &World) -> Option<String> {
    let db = world.get_resource::<ContentDb>()?;
    let mut bad = Offenders::default();
    with_query::<(&AiId, &LeapRun, Option<&ActionState>), (), _>(world, |query| {
        for (id, x, state) in query.iter(world) {
            let running = state.and_then(|s| s.current.as_ref());
            if running.is_none_or(|r| r.instance != x.instance) {
                let now = running.map_or("no action".to_owned(), |r| format!("instance {} ({})", r.instance, r.action));
                bad.note(*id, || format!("LeapRun of instance {} while it runs {now}", x.instance));
            } else if !running.and_then(|r| db.get::<ActionDef>(&r.action)).is_some_and(|d| d.leap.is_some()) {
                bad.note(*id, || format!("LeapRun while it runs `{}`, which does not leap", running.map_or("", |r| &r.action)));
            } else if !(x.scale.is_finite() && x.scale >= 0.0) {
                bad.note(*id, || format!("LeapRun scale = {}: a finite number >= 0", x.scale));
            }
        }
    });
    bad.report()
}

/// The engine rule `action.ghost_follows_run`: a fighter passes through other characters
/// ([`Ghost`]) only while its running action has `ghost` and an `intangible` span open.
///
/// On a fighter the `Ghost` is the action machine's: it puts it on when such a window opens and takes
/// it off when the window closes or the run ends, by any path. One left behind keeps a body that
/// should be solid out of the separation pass; one a game puts there by hand is taken off again on
/// the next tick, so a game that wants a passing-through body without an action makes it a body
/// that is not a fighter.
pub fn ghost_follows_run(world: &World) -> Option<String> {
    let db = world.get_resource::<ContentDb>()?;
    let mut bad = Offenders::default();
    with_query::<(&AiId, &ActionState), (With<Ghost>, With<Fighter>), _>(world, |query| {
        for (id, state) in query.iter(world) {
            let due = state
                .current
                .as_ref()
                .is_some_and(|r| db.get::<ActionDef>(&r.action).is_some_and(|d| d.ghost_in(r.prev, r.t)));
            if !due {
                let now = state.current.as_ref().map_or("no action".to_owned(), |r| format!("`{}`", r.action));
                bad.note(*id, || format!("Ghost while it runs {now}, which has no open ghost window"));
            }
        }
    });
    bad.report()
}
