//! Simulation clock: fixed tick, pause, deterministic stepping.
//!
//! An LLM thinks for seconds while a frame lasts 16 ms. Turning the real-time game
//! into a turn-based environment fixes that: pause, look, act, `step(n)`, look again.
//!
//! All gameplay lives in `FixedUpdate`, ordered by [`SimSet`]. Pausing stops
//! `Time<Virtual>`, so `FixedUpdate` stops too, while rendering, UI and the
//! `ai/*` endpoints keep running. [`step`] runs `FixedMain` directly, exactly the
//! way Bevy's own fixed loop does, so `step(600)` completes in one call (as fast
//! as the CPU allows) and returns only after the ticks ran.

use std::time::{Duration, Instant};

use bevy::app::FixedMain;
use bevy::prelude::*;

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

pub(crate) fn advance_tick(mut tick: ResMut<SimTick>) {
    tick.0 += 1;
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
/// `stop` is checked after every tick; returning `Some(reason)` ends the run.
pub fn step(
    world: &mut World,
    max_ticks: u64,
    wall_budget: Duration,
    mut stop: impl FnMut(&mut World) -> Option<String>,
) -> Result<StepReport, String> {
    pause(world);
    let from_tick = tick(world);
    let started = Instant::now();
    let mut end = StepEnd::Completed;
    for _ in 0..max_ticks {
        run_tick(world)?;
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
