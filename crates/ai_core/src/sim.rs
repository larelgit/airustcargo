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
//! fails ends the step at its tick (see [`failures`](crate::failures)).
//!
//! While something must finish first (content loading), [`SimGate`] holds the
//! simulation: no ticks run, in real time or through [`step`], so loading never shifts
//! a tick.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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

/// [`SimTick`] for code outside the ECS: the log capture layer stamps every line with
/// it, from any thread. [`advance_tick`] stores each new tick.
#[derive(Resource, Clone, Default, Debug)]
pub struct TickClock(Arc<AtomicU64>);

impl TickClock {
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    /// A snapshot moved the simulation to another tick.
    pub(crate) fn set(&self, tick: u64) {
        self.0.store(tick, Ordering::Relaxed);
    }
}

/// Named holds on the simulation. While any exists, no tick runs: real-time play is
/// paused (and resumes on release only if the gate paused it, so a `--paused` game
/// stays paused), [`step`] and `resume` refuse with [`SimGate::message`], and
/// `ai/status` lists the [`reasons`](SimGate::reasons) under `loading`.
///
/// Loading never shifts ticks: whatever a game loads, tick 1 always runs on the same
/// world.
#[derive(Resource, Default, Debug)]
pub struct SimGate {
    /// Name → what it waits for, in name order.
    holds: BTreeMap<String, String>,
    /// The gate paused `Time<Virtual>`, so its release unpauses it.
    paused_time: bool,
}

impl SimGate {
    /// Holds the simulation until [`release`](Self::release)`(name)`. Holding again
    /// under the same name only updates the reason (progress: `2 files left`).
    pub fn hold(&mut self, name: &str, reason: impl Into<String>) {
        self.holds.insert(name.to_owned(), reason.into());
    }

    /// Ends hold `name` (nothing if there is none).
    pub fn release(&mut self, name: &str) {
        self.holds.remove(name);
    }

    /// Whether anything holds the simulation.
    pub fn is_held(&self) -> bool {
        !self.holds.is_empty()
    }

    /// `content: 2 files left`, one per hold, in name order.
    pub fn reasons(&self) -> Vec<String> {
        self.holds.iter().map(|(name, reason)| format!("{name}: {reason}")).collect()
    }

    /// Why ticks cannot run now, and what to do; `None` when nothing holds them.
    pub fn message(&self) -> Option<String> {
        self.is_held()
            .then(|| format!("the game is still loading ({}); wait with `shards wait`", self.reasons().join(", ")))
    }

    /// Someone paused on purpose while the gate held: releasing it must not resume.
    /// Returns whether the gate had paused it.
    pub(crate) fn forget_pause(&mut self) -> bool {
        std::mem::take(&mut self.paused_time)
    }
}

/// `First`, before the clock advances: a held gate pauses `Time<Virtual>` before this
/// frame's time is counted, so no fixed tick is due; the last release unpauses it, if
/// the gate was the one that paused it. Systems that hold or release run before it.
pub fn apply_gate(mut gate: ResMut<SimGate>, mut time: ResMut<Time<Virtual>>) {
    if gate.is_held() {
        if !time.is_paused() {
            time.pause();
            gate.paused_time = true;
        }
    } else if gate.paused_time {
        gate.paused_time = false;
        time.unpause();
    }
}

/// [`SimGate::message`] for this world (no gate: nothing holds it).
pub fn gate_message(world: &World) -> Option<String> {
    world.get_resource::<SimGate>().and_then(SimGate::message)
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

/// Runs exactly one fixed tick right now, independent of frame timing.
/// Mirrors `bevy::time::run_fixed_main_schedule`.
pub fn run_tick(world: &mut World) -> Result<(), String> {
    crate::process::activate(world);
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
/// Refused while the [`SimGate`] holds the simulation; a hold that appears during the
/// run ends it.
pub fn step(
    world: &mut World,
    max_ticks: u64,
    wall_budget: Duration,
    mut stop: impl FnMut(&mut World) -> Option<String>,
) -> Result<StepReport, String> {
    crate::process::activate(world);
    if let Some(loading) = gate_message(world) {
        return Err(loading);
    }
    pause(world);
    // Failures from before this step are in the journal already; they are not its own.
    crate::failures::collect(world);
    crate::failures::take_unreported(world);
    let from_tick = tick(world);
    let started = Instant::now();
    let mut end = StepEnd::Completed;
    for _ in 0..max_ticks {
        run_tick(world)?;
        crate::failures::collect(world);
        let failed = crate::failures::take_unreported(world);
        if !failed.is_empty() {
            end = StepEnd::SystemFailed(failed);
            break;
        }
        if let Some(reason) = stop(world) {
            end = StepEnd::Condition(reason);
            break;
        }
        if let Some(gate) = world.get_resource::<SimGate>().filter(|g| g.is_held()) {
            end = StepEnd::Condition(format!("the game started loading ({})", gate.reasons().join(", ")));
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
    fn a_held_gate_stops_real_time_and_step_and_its_release_restores_them() {
        use bevy::time::{TimeSystems, TimeUpdateStrategy};
        let mut app = app();
        app.init_resource::<SimGate>()
            .add_systems(First, apply_gate.before(TimeSystems))
            .insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
        app.world_mut().resource_mut::<SimGate>().hold("content", "2 files left");
        for _ in 0..5 {
            app.update();
        }
        assert_eq!(app.world().resource::<SimTick>().0, 0, "held: no real-time ticks");
        let err = step(app.world_mut(), 5, Duration::from_secs(5), |_| None).unwrap_err();
        assert_eq!(err, "the game is still loading (content: 2 files left); wait with `shards wait`");

        app.world_mut().resource_mut::<SimGate>().release("content");
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(app.world().resource::<SimTick>().0, 3, "released: real time runs again");

        // A game that was paused before the gate held it stays paused after.
        pause(app.world_mut());
        app.world_mut().resource_mut::<SimGate>().hold("region", "loading");
        app.update();
        app.world_mut().resource_mut::<SimGate>().release("region");
        app.update();
        assert!(is_paused(app.world()));
        assert_eq!(app.world().resource::<SimTick>().0, 3);
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
