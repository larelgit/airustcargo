//! Tick timing: the real time every `FixedMain` run takes, in [`step`](crate::sim::step)
//! and in real-time play alike, against a budget (the tick's own duration by default).
//!
//! A system before Bevy's `run_fixed_main` marks the start of every `FixedMain` run and
//! one after it takes the measurement, so every system of `FixedFirst` … `FixedLast` is
//! inside, a game's own included. The last [`RECENT`] ticks are kept for `ai/status`.
//! When [`AiPerf::report`] is on, `step` adds a line about its own ticks
//! (`ticks: avg 0.42 ms, max 2.10 ms (tick 142), budget 16.7 ms`), and ticks over budget
//! are journaled as [`JournalKind::Perf`], at most once per game-second
//! (`12 ticks over budget, worst 31.4 ms at tick 150`).
//!
//! Timing is wall-clock and differs from run to run: it is in no replay check and no
//! replay, and with `report` off it stays out of the journal and the `step` reply
//! (tests that compare whole journals keep it off).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::app::FixedMain;
use bevy::prelude::*;
use serde_json::{json, Value};

use crate::journal::{AiJournal, JournalKind};
use crate::sim::SimTick;

/// Ticks kept for `ai/status`: ten seconds at 60 Hz.
pub const RECENT: usize = 600;

/// How a run of ticks went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickStats {
    pub ticks: u64,
    pub total: Duration,
    /// The slowest tick, and which one it was.
    pub max: Duration,
    pub worst_tick: u64,
    pub over_budget: u64,
}

impl TickStats {
    fn add(&mut self, tick: u64, took: Duration, over: bool) {
        self.ticks += 1;
        self.total += took;
        if self.ticks == 1 || took > self.max {
            self.max = took;
            self.worst_tick = tick;
        }
        self.over_budget += u64::from(over);
    }

    pub fn avg(&self) -> Duration {
        match self.ticks {
            0 => Duration::ZERO,
            n => Duration::from_nanos((self.total.as_nanos() / u128::from(n)) as u64),
        }
    }

    /// `ticks: avg 0.42 ms, max 2.10 ms (tick 142), budget 16.7 ms`, then
    /// `, 3 over budget` if any were.
    pub fn line(&self, budget: Duration) -> String {
        let mut line = format!(
            "ticks: avg {:.2} ms, max {:.2} ms (tick {}), budget {:.1} ms",
            ms(self.avg()),
            ms(self.max),
            self.worst_tick,
            ms(budget)
        );
        if self.over_budget > 0 {
            line.push_str(&format!(", {} over budget", self.over_budget));
        }
        line
    }
}

/// Tick timing: settings, the last [`RECENT`] ticks, and what is still to be journaled.
#[derive(Resource, Debug)]
pub struct AiPerf {
    /// A tick that takes longer is over budget. `None`: the tick's own duration
    /// (16.7 ms at 60 Hz).
    pub budget: Option<Duration>,
    /// Report timing: the `ticks:` line of `step` and `perf` journal entries. Measuring
    /// goes on either way, for `ai/status`.
    pub report: bool,
    started: Option<Instant>,
    recent: VecDeque<(u64, Duration)>,
    /// The ticks since [`begin_span`]: those of the `step` running.
    span: TickStats,
    /// Ticks over budget not journaled yet, and the tick of the last entry.
    pending: TickStats,
    last_entry: Option<u64>,
}

impl AiPerf {
    pub fn new(budget: Option<Duration>, report: bool) -> Self {
        Self {
            budget,
            report,
            started: None,
            recent: VecDeque::with_capacity(RECENT),
            span: TickStats::default(),
            pending: TickStats::default(),
            last_entry: None,
        }
    }

    /// The budget for ticks of `timestep`.
    pub fn budget_for(&self, timestep: Duration) -> Duration {
        self.budget.unwrap_or(timestep)
    }

    /// Tick `tick` took `took`: kept among the last [`RECENT`], counted in the span.
    fn record(&mut self, tick: u64, took: Duration, over: bool) {
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back((tick, took));
        self.span.add(tick, took, over);
    }

    /// `(tick, real time)` of the last [`RECENT`] ticks, oldest first.
    pub fn recent(&self) -> impl Iterator<Item = (u64, Duration)> + '_ {
        self.recent.iter().copied()
    }

    /// The last [`RECENT`] ticks against `budget`.
    pub fn recent_stats(&self, budget: Duration) -> TickStats {
        let mut stats = TickStats::default();
        for (tick, took) in self.recent() {
            stats.add(tick, took, took > budget);
        }
        stats
    }
}

impl Default for AiPerf {
    fn default() -> Self {
        Self::new(None, true)
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Milliseconds with microsecond precision, for JSON.
fn ms_json(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

/// Before every `FixedMain` run.
fn mark(mut perf: ResMut<AiPerf>) {
    perf.started = Some(Instant::now());
}

/// After every `FixedMain` run: the tick it ran, how long it took, and the journal
/// entry for ticks over budget once a game-second has passed since the last one.
fn measure(mut perf: ResMut<AiPerf>, tick: Res<SimTick>, fixed: Res<Time<Fixed>>, mut journal: ResMut<AiJournal>) {
    let Some(started) = perf.started.take() else {
        return;
    };
    let took = started.elapsed();
    let (tick, timestep) = (tick.0, fixed.timestep());
    let over = took > perf.budget_for(timestep);
    perf.record(tick, took, over);
    if !perf.report {
        return;
    }
    if over {
        perf.pending.add(tick, took, true);
    }
    let per_second = (1.0 / timestep.as_secs_f64()).round().max(1.0) as u64;
    let due = perf.last_entry.is_none_or(|last| tick >= last + per_second);
    if perf.pending.ticks > 0 && due {
        let p = perf.pending;
        let ticks = if p.ticks == 1 { "tick" } else { "ticks" };
        let text = format!("{} {ticks} over budget, worst {:.1} ms at tick {}", p.ticks, ms(p.max), p.worst_tick);
        journal.push(tick, JournalKind::Perf, text);
        perf.pending = TickStats::default();
        perf.last_entry = Some(tick);
    }
}

/// Starts counting the ticks of a `step`.
pub(crate) fn begin_span(world: &mut World) {
    if let Some(mut perf) = world.get_resource_mut::<AiPerf>() {
        perf.span = TickStats::default();
    }
}

/// The ticks since [`begin_span`].
pub(crate) fn span(world: &World) -> TickStats {
    world.get_resource::<AiPerf>().map(|p| p.span).unwrap_or_default()
}

/// The `ticks:` line for a `step` that ran `stats`, if timing is reported.
pub(crate) fn step_line(world: &World, stats: &TickStats) -> Option<String> {
    let perf = world.get_resource::<AiPerf>().filter(|p| p.report)?;
    let budget = perf.budget_for(world.resource::<Time<Fixed>>().timestep());
    (stats.ticks > 0).then(|| stats.line(budget))
}

/// Timing of the last [`RECENT`] ticks for `ai/status`.
pub fn status(world: &World) -> Value {
    let Some(perf) = world.get_resource::<AiPerf>() else {
        return Value::Null;
    };
    let budget = perf.budget_for(world.resource::<Time<Fixed>>().timestep());
    let stats = perf.recent_stats(budget);
    json!({
        "ticks": stats.ticks,
        "avg_ms": ms_json(stats.avg()),
        "max_ms": ms_json(stats.max),
        "worst_tick": (stats.ticks > 0).then_some(stats.worst_tick),
        "over_budget": stats.over_budget,
        "budget_ms": ms_json(budget),
    })
}

pub(crate) fn plugin(app: &mut App, budget: Option<Duration>, report: bool) {
    app.insert_resource(AiPerf::new(budget, report)).add_systems(
        FixedMain,
        (mark.before(FixedMain::run_fixed_main), measure.after(FixedMain::run_fixed_main)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_name_the_worst_tick_and_count_the_slow_ones() {
        let mut stats = TickStats::default();
        let budget = Duration::from_millis(16);
        for (tick, took) in [(1, 2), (2, 20), (3, 1), (4, 31), (5, 3)] {
            let took = Duration::from_millis(took);
            stats.add(tick, took, took > budget);
        }
        assert_eq!((stats.ticks, stats.worst_tick, stats.over_budget), (5, 4, 2));
        assert_eq!(stats.max, Duration::from_millis(31));
        assert_eq!(stats.avg(), Duration::from_micros(11_400));
        assert_eq!(stats.line(budget), "ticks: avg 11.40 ms, max 31.00 ms (tick 4), budget 16.0 ms, 2 over budget");
        let calm = TickStats { ticks: 1, total: budget / 4, max: budget / 4, worst_tick: 9, over_budget: 0 };
        assert_eq!(calm.line(Duration::from_secs_f64(1.0 / 60.0)), "ticks: avg 4.00 ms, max 4.00 ms (tick 9), budget 16.7 ms");
    }

    #[test]
    fn only_the_last_ticks_are_kept() {
        let mut perf = AiPerf::default();
        let last = RECENT as u64 + 5;
        for tick in 1..=last {
            perf.record(tick, Duration::from_millis(tick), false);
        }
        assert_eq!(perf.recent().next(), Some((6, Duration::from_millis(6))), "the oldest five are gone");
        let stats = perf.recent_stats(Duration::from_millis(100));
        assert_eq!((stats.ticks, stats.worst_tick), (RECENT as u64, last));
        assert_eq!(stats.over_budget, last - 100);
        assert_eq!(perf.span.ticks, last, "a span counts every tick, not only the kept ones");
    }
}
