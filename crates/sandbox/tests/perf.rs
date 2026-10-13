//! Tick timing: the real time of every tick, against a budget, in `step` replies, the
//! journal and `ai/status`. `sandbox::test_app` keeps timing out of replies and the
//! journal (it differs from run to run), so these tests turn it on themselves. They
//! sleep in gameplay systems: a file of their own keeps them off other tests' clocks.

use std::time::Duration;

use ai_core::prelude::*;
use ai_core::replay;
use ai_core::{call_tool, AiJournal, AiPerf, AiReplay, CallOutcome, JournalKind};
use bevy::prelude::*;
use serde_json::{json, Value};

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = call_tool(app.world_mut(), "dev", tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

/// Runs `ticks` ticks; the reply's lines.
fn step(app: &mut App, ticks: u64) -> Vec<String> {
    let out = ok(app, "step", json!({"ticks": ticks}));
    out.result.as_str().unwrap().lines().map(str::to_owned).collect()
}

/// A gameplay system that sleeps `ms` in each of `ticks`.
fn slow(app: &mut App, ticks: &'static [u64], ms: u64) {
    app.add_systems(
        FixedUpdate,
        (move |tick: Res<SimTick>| {
            if ticks.contains(&tick.0) {
                std::thread::sleep(Duration::from_millis(ms));
            }
        })
        .in_set(SimSet::Cleanup),
    );
}

/// A game that has run its first tick: that one builds the fixed schedules, which is
/// not what these tests measure.
fn warmed_up() -> App {
    let mut app = sandbox::test_app(42);
    step(&mut app, 1);
    app
}

/// Timing on, with `budget`.
fn report(app: &mut App, budget: Duration) {
    let mut perf = app.world_mut().resource_mut::<AiPerf>();
    perf.report = true;
    perf.budget = Some(budget);
}

fn perf_entries(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Perf).map(ToString::to_string).collect()
}

/// `max 31.40 ms (tick 10)` in a `ticks:` line: the milliseconds and the tick.
fn max_of(line: &str) -> (f64, u64) {
    let rest = line.split_once("max ").expect("a max").1;
    let (ms, rest) = rest.split_once(" ms (tick ").unwrap();
    (ms.parse().unwrap(), rest.split_once(')').unwrap().0.parse().unwrap())
}

#[test]
fn a_slow_tick_shows_in_the_step_reply_and_the_journal() {
    let mut app = warmed_up();
    slow(&mut app, &[10], 30);
    report(&mut app, Duration::from_millis(20));

    let reply = step(&mut app, 20);
    assert_eq!(reply[0], "ran 20 ticks (1 → 21)");
    let line = &reply[1];
    assert!(line.starts_with("ticks: avg ") && line.ends_with(", budget 20.0 ms, 1 over budget"), "{line}");
    let (max, tick) = max_of(line);
    assert!(max >= 30.0, "{line}");
    assert_eq!(tick, 10, "{line}");

    let entries = perf_entries(&app);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries[0].starts_with("10 [perf] 1 tick over budget, worst ") && entries[0].ends_with(" ms at tick 10"), "{entries:?}");
    // The status keeps the same ticks.
    let status = ai_core::perf::status(app.world());
    assert_eq!((status["worst_tick"].as_u64(), status["over_budget"].as_u64()), (Some(10), Some(1)), "{status}");
    assert!(status["max_ms"].as_f64().unwrap() >= 30.0, "{status}");
}

#[test]
fn ticks_within_budget_leave_the_journal_alone() {
    let mut app = warmed_up();
    report(&mut app, Duration::from_secs(10));
    let reply = step(&mut app, 120);
    assert_eq!(reply.len(), 2, "{reply:?}");
    assert!(reply[1].starts_with("ticks: avg ") && reply[1].ends_with(", budget 10000.0 ms"), "{reply:?}");
    assert_eq!(perf_entries(&app), Vec::<String>::new());
}

#[test]
fn ticks_over_budget_are_journaled_at_most_once_a_game_second() {
    let mut app = warmed_up();
    slow(&mut app, &[10, 20, 30, 40, 75], 25);
    report(&mut app, Duration::from_millis(20));

    let reply = step(&mut app, 150);
    assert!(reply[1].ends_with(", 5 over budget"), "{reply:?}");
    let entries = perf_entries(&app);
    let tail = |entry: &str| entry.split_once(" ms at tick ").map(|(_, tick)| tick.to_owned()).unwrap();
    // The first one right away; the next three after a game-second (60 ticks), as one;
    // tick 75 waits for the second after that.
    assert_eq!(entries.len(), 3, "{entries:?}");
    assert!(entries[0].starts_with("10 [perf] 1 tick over budget, worst "), "{entries:?}");
    assert!(entries[1].starts_with("70 [perf] 3 ticks over budget, worst "), "{entries:?}");
    assert!(["20", "30", "40"].contains(&tail(&entries[1]).as_str()), "{entries:?}");
    assert!(entries[2].starts_with("130 [perf] 1 tick over budget, worst ") && tail(&entries[2]) == "75", "{entries:?}");
}

#[test]
fn timing_off_still_measures_for_the_status() {
    let mut app = warmed_up();
    slow(&mut app, &[10], 30);
    app.world_mut().resource_mut::<AiPerf>().budget = Some(Duration::from_millis(20));
    // `test_app`: no `ticks:` line, no journal entries.
    assert_eq!(step(&mut app, 20), ["ran 20 ticks (1 → 21)"]);
    assert_eq!(perf_entries(&app), Vec::<String>::new());
    let status = ai_core::perf::status(app.world());
    assert_eq!((status["worst_tick"].as_u64(), status["over_budget"].as_u64()), (Some(10), Some(1)), "{status}");
}

#[test]
fn the_status_covers_the_last_600_ticks() {
    let mut app = sandbox::test_app(42);
    step(&mut app, 700);
    let status = ai_core::perf::status(app.world());
    assert_eq!(status["ticks"], 600, "{status}");
    let (avg, max) = (status["avg_ms"].as_f64().unwrap(), status["max_ms"].as_f64().unwrap());
    assert!(avg > 0.0 && max >= avg, "{status}");
    assert!((101..=700).contains(&status["worst_tick"].as_u64().unwrap()), "{status}");
    assert!(status["over_budget"].as_u64().is_some(), "{status}");
    assert_eq!(status["budget_ms"], 16.667, "the tick's own duration at 60 Hz: {status}");
}

#[test]
fn timing_stays_out_of_replays() {
    let mut original = warmed_up();
    slow(&mut original, &[10], 30);
    report(&mut original, Duration::from_millis(20));
    ok(&mut original, "attack", json!({"target": "e6", "repeat": true}));
    step(&mut original, 120);
    assert_eq!(perf_entries(&original).len(), 1);
    let recording = replay::save(original.world());
    assert!(!serde_json::to_string(&recording).unwrap().contains("budget"), "no timing in the file");

    // A playback that ticks at another pace, with timing off: the same game.
    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), recording).unwrap();
    step(&mut copy, 121);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");
}
