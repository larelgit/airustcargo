//! Simulation clock: fixed tick, pause, deterministic stepping.
//!
//! An LLM thinks for seconds while a frame lasts 16 ms. Turning the real-time game
//! into a turn-based environment fixes that: pause, look, act, `step(n)`, look again.
//!
//! All gameplay lives in `FixedUpdate`, ordered by [`SimSet`]. Pausing stops
//! `Time<Virtual>`, so `FixedUpdate` stops too, while rendering, UI and the
//! `ai/*` endpoints keep running. [`step`] runs `FixedMain` directly, exactly the
//! way Bevy's own fixed loop does, so `step(600)` completes in one call (as fast
//! as the CPU allows) and returns only after the ticks ran. A gameplay system that
//! fails, or an invariant that is violated, halts the game and ends the step at its
//! tick (see [`failures`](crate::failures) and [`invariants`](crate::invariants)).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::app::FixedMain;
use bevy::prelude::*;
use serde::Serialize;

use crate::journal::{self, JournalKind};

/// Number of completed fixed ticks. Incremented at the very start of every tick,
/// so systems inside tick `N` (and journal entries they produce) see `N`.
#[derive(Resource, Reflect, Default, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[reflect(Resource)]
pub struct SimTick(pub u64);

/// Explicit gameplay order inside `FixedUpdate`. Hidden order dependencies are the
/// first thing that kills determinism; put every gameplay system in one of these.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Intents → orders → per-tick controls (player input, AI orders, replays).
    Input,
    /// NPC brains choose what to do.
    Think,
    /// Actions start/advance: attacks, dodges, abilities.
    Act,
    /// Movement integration and collision.
    Move,
    /// Damage, deaths, phase changes, regeneration.
    Resolve,
    /// Despawns and bookkeeping.
    Cleanup,
}

/// Fixed-tick configuration, read by [`AiCorePlugin`](crate::AiCorePlugin).
#[derive(Resource, Clone, Debug)]
pub struct SimConfig {
    pub tick_hz: f64,
    pub seed: u64,
    pub start_paused: bool,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self { tick_hz: 60.0, seed: 0x5EED, start_paused: false }
    }
}

/// [`SimTick`] for code outside the ECS: the log capture layer stamps every line with
/// it, from any thread. [`advance_tick`] stores each new tick.
#[derive(Resource, Clone, Default, Debug)]
pub struct TickClock(Arc<AtomicU64>);

impl TickClock {
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

pub(crate) fn advance_tick(mut tick: ResMut<SimTick>, clock: Option<Res<TickClock>>) {
    tick.0 += 1;
    if let Some(clock) = clock {
        clock.0.store(tick.0, Ordering::Relaxed);
    }
}

pub fn is_paused(world: &World) -> bool {
    world.resource::<Time<Virtual>>().is_paused()
}

pub fn pause(world: &mut World) {
    world.resource_mut::<Time<Virtual>>().pause();
}

pub fn resume(world: &mut World) {
    world.resource_mut::<Time<Virtual>>().unpause();
}

/// Game speed multiplier for real-time play (1.0 = normal).
pub fn set_speed(world: &mut World, speed: f32) {
    world.resource_mut::<Time<Virtual>>().set_relative_speed(speed.max(0.0));
}

pub fn speed(world: &World) -> f32 {
    world.resource::<Time<Virtual>>().relative_speed()
}

pub fn tick(world: &World) -> u64 {
    world.resource::<SimTick>().0
}

/// Seconds of simulated time per tick.
pub fn tick_seconds(world: &World) -> f32 {
    world.resource::<Time<Fixed>>().timestep().as_secs_f32()
}

/// The game stopped itself: a gameplay system failed or an invariant was violated.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Halt {
    /// `invariant hp violated at tick 1`, `system game::combat::apply_hits panicked at tick 50`.
    pub reason: String,
    /// The replay saved for it, relative to the game's working directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<String>,
}

/// Every halt so far: [`call_tool`](crate::call_tool) reports the one its call caused
/// (a `step` stops right there).
#[derive(Resource, Default, Debug)]
pub struct Halts {
    pub count: u64,
    pub last: Option<Halt>,
}

/// Pauses the game right after a failure or a violation (real-time play runs no further
/// tick this frame) and counts the halt. A `step` is paused already and stops by itself.
pub(crate) fn halt(world: &mut World, halt: Halt) {
    if !is_paused(world) {
        pause(world);
        let mut fixed = world.resource_mut::<Time<Fixed>>();
        let overstep = fixed.overstep();
        fixed.discard_overstep(overstep);
        journal::log(world, JournalKind::Sim, format!("paused: {}", halt.reason));
    }
    let mut halts = world.get_resource_or_init::<Halts>();
    halts.count += 1;
    halts.last = Some(halt);
}

/// Runs exactly one fixed tick right now, independent of frame timing.
/// Mirrors `bevy::time::run_fixed_main_schedule`.
pub fn run_tick(world: &mut World) -> Result<(), String> {
    let result = world.try_schedule_scope(FixedMain, |world, schedule| {
        let timestep = world.resource::<Time<Fixed>>().timestep();
        world.resource_mut::<Time<Fixed>>().advance_by(timestep);
        *world.resource_mut::<Time>() = world.resource::<Time<Fixed>>().as_generic();
        schedule.run(world);
    });
    *world.resource_mut::<Time>() = world.resource::<Time<Virtual>>().as_generic();
    result.map_err(|e| format!("cannot run a tick here: {e}"))
}

/// Why a [`step`] ended.
#[derive(Debug, Clone, PartialEq)]
pub enum StepEnd {
    /// Ran all requested ticks.
    Completed,
    /// The stop condition fired; the message says what happened.
    Condition(String),
    /// Wall-clock budget exhausted before all ticks ran.
    WallClock,
    /// A gameplay system panicked (or failed) in the last tick; never empty. The game
    /// is paused, the failures are in the journal.
    SystemFailed(Vec<crate::failures::SystemFailure>),
    /// Invariants were violated in the last tick; never empty. The game is paused, the
    /// violations are in the journal.
    InvariantViolated(Vec<crate::invariants::Violation>),
}

#[derive(Debug, Clone)]
pub struct StepReport {
    pub from_tick: u64,
    pub to_tick: u64,
    pub end: StepEnd,
    pub wall: Duration,
}

impl StepReport {
    pub fn ticks(&self) -> u64 {
        self.to_tick - self.from_tick
    }
}

/// Pauses the game (if needed) and runs up to `max_ticks` ticks synchronously.
/// `stop` is checked after every tick; returning `Some(reason)` ends the run. A halt (a
/// failed gameplay system, a violated invariant) ends it first.
pub fn step(
    world: &mut World,
    max_ticks: u64,
    wall_budget: Duration,
    mut stop: impl FnMut(&mut World) -> Option<String>,
) -> Result<StepReport, String> {
    pause(world);
    // Failures and violations from before this step are in the journal already; they
    // are not its own.
    crate::failures::collect(world);
    crate::failures::take_unreported(world);
    crate::invariants::take_unreported(world);
    let from_tick = tick(world);
    let started = Instant::now();
    let mut end = StepEnd::Completed;
    for _ in 0..max_ticks {
        run_tick(world)?;
        crate::failures::collect(world);
        let failed = crate::failures::take_unreported(world);
        let violated = crate::invariants::take_unreported(world);
        if !failed.is_empty() {
            end = StepEnd::SystemFailed(failed);
            break;
        }
        if !violated.is_empty() {
            end = StepEnd::InvariantViolated(violated);
            break;
        }
        if let Some(reason) = stop(world) {
            end = StepEnd::Condition(reason);
            break;
        }
        if started.elapsed() > wall_budget {
            end = StepEnd::WallClock;
            break;
        }
    }
    Ok(StepReport { from_tick, to_tick: tick(world), end, wall: started.elapsed() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::time::TimePlugin;

    #[derive(Resource, Default)]
    struct Runs(u32);

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(TimePlugin)
            .init_resource::<SimTick>()
            .init_resource::<Runs>()
            .insert_resource(Time::<Fixed>::from_hz(60.0))
            .add_systems(FixedFirst, advance_tick)
            .add_systems(FixedUpdate, |mut runs: ResMut<Runs>| runs.0 += 1);
        app.finish();
        app.cleanup();
        app
    }

    #[test]
    fn paused_app_does_not_tick_but_step_runs_exact_ticks() {
        let mut app = app();
        pause(app.world_mut());
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(app.world().resource::<SimTick>().0, 0, "paused: no ticks from frames");

        let report = step(app.world_mut(), 5, Duration::from_secs(5), |_| None).unwrap();
        assert_eq!(report.ticks(), 5);
        assert_eq!(report.end, StepEnd::Completed);
        assert_eq!(app.world().resource::<Runs>().0, 5);
        assert_eq!(app.world().resource::<SimTick>().0, 5);
        let fixed = app.world().resource::<Time<Fixed>>();
        assert!((fixed.elapsed_secs_f64() - 5.0 / 60.0).abs() < 1e-6);

        // Frames after a step still do not tick while paused.
        app.update();
        assert_eq!(app.world().resource::<SimTick>().0, 5);
    }

    #[test]
    fn step_stops_on_condition() {
        let mut app = app();
        let report = step(app.world_mut(), 100, Duration::from_secs(5), |w| {
            (w.resource::<SimTick>().0 == 7).then(|| "tick 7".to_owned())
        })
        .unwrap();
        assert_eq!(report.to_tick, 7);
        assert_eq!(report.end, StepEnd::Condition("tick 7".into()));
        assert!(is_paused(app.world()));
    }
}
