//! Tick timing: the real time of every tick, against a budget, in `step` replies, the
//! journal and `ai/status`. `sandbox::test_app` keeps timing out of replies and the
//! journal (it differs from run to run), so these tests turn it on themselves. They
//! sleep in gameplay systems far longer than a tick of this sandbox takes, with budgets
//! well above a tick too. The games of one process share Bevy's task pools, and a thread
//! that waits for its own systems may run another game's meanwhile, a sleeping one
//! included: so these tests have a file of their own, away from the other tests, and
//! run one game at a time ([`one_clock`]).

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use shards::ai_core::{self, replay, AiJournal, AiPerf, AiReplay, JournalKind};
use shards::bench::PerfDef;
use shards::prelude::*;
use shards::serde_json::{self, json};
use shards::testing::ok;

/// How long a slow tick sleeps, and the budget it breaks.
const SLOW_MS: u64 = 200;
const BUDGET_MS: u64 = 120;

/// Held by every test while its game ticks: one clock at a time in this process.
fn one_clock() -> MutexGuard<'static, ()> {
    static CLOCK: Mutex<()> = Mutex::new(());
    CLOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `ticks` ticks; the reply's lines.
fn step(app: &mut App, ticks: u64) -> Vec<String> {
    let out = ok(app, "step", json!({"ticks": ticks}));
    out.result.as_str().unwrap().lines().map(str::to_owned).collect()
}

/// A gameplay system that sleeps [`SLOW_MS`] in each of `ticks`.
fn slow(app: &mut App, ticks: &'static [u64]) {
    app.add_systems(
        FixedUpdate,
        (move |tick: Res<SimTick>| {
            if ticks.contains(&tick.0) {
                std::thread::sleep(Duration::from_millis(SLOW_MS));
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
    let _clock = one_clock();
    let mut app = warmed_up();
    slow(&mut app, &[10]);
    report(&mut app, Duration::from_millis(BUDGET_MS));

    let reply = step(&mut app, 20);
    assert_eq!(reply[0], "ran 20 ticks (1 → 21)");
    let line = &reply[1];
    assert!(line.starts_with("ticks: avg ") && line.ends_with(", budget 120.0 ms, 1 over budget"), "{line}");
    let (max, tick) = max_of(line);
    assert!(max >= SLOW_MS as f64, "{line}");
    assert_eq!(tick, 10, "{line}");

    let entries = perf_entries(&app);
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert!(entries[0].starts_with("10 [perf] 1 tick over budget, worst ") && entries[0].ends_with(" ms at tick 10"), "{entries:?}");
    // The status keeps the same ticks.
    let status = ai_core::perf::status(app.world());
    assert_eq!((status["worst_tick"].as_u64(), status["over_budget"].as_u64()), (Some(10), Some(1)), "{status}");
    assert!(status["max_ms"].as_f64().unwrap() >= SLOW_MS as f64, "{status}");
}

#[test]
fn ticks_within_budget_leave_the_journal_alone() {
    let _clock = one_clock();
    let mut app = warmed_up();
    report(&mut app, Duration::from_secs(10));
    let reply = step(&mut app, 120);
    assert_eq!(reply.len(), 2, "{reply:?}");
    assert!(reply[1].starts_with("ticks: avg ") && reply[1].ends_with(", budget 10000.0 ms"), "{reply:?}");
    assert_eq!(perf_entries(&app), Vec::<String>::new());
}

#[test]
fn ticks_over_budget_are_journaled_at_most_once_a_game_second() {
    let _clock = one_clock();
    let mut app = warmed_up();
    slow(&mut app, &[10, 20, 30, 40, 75]);
    report(&mut app, Duration::from_millis(BUDGET_MS));

    let reply = step(&mut app, 150);
    let entries = perf_entries(&app);
    assert!(reply[1].ends_with(", 5 over budget"), "{reply:?} {entries:?}");
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
    let _clock = one_clock();
    let mut app = warmed_up();
    slow(&mut app, &[10]);
    app.world_mut().resource_mut::<AiPerf>().budget = Some(Duration::from_millis(BUDGET_MS));
    // `test_app`: no `ticks:` line, no journal entries.
    assert_eq!(step(&mut app, 20), ["ran 20 ticks (1 → 21)"]);
    assert_eq!(perf_entries(&app), Vec::<String>::new());
    let status = ai_core::perf::status(app.world());
    assert_eq!((status["worst_tick"].as_u64(), status["over_budget"].as_u64()), (Some(10), Some(1)), "{status}");
}

#[test]
fn the_status_covers_the_last_600_ticks() {
    let _clock = one_clock();
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
    let _clock = one_clock();
    let mut original = warmed_up();
    slow(&mut original, &[10]);
    report(&mut original, Duration::from_millis(BUDGET_MS));
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

/// The sandbox's reference scene (`--perf reference`).
fn reference(app: &App) -> PerfDef {
    app.world().resource::<shards::content::ContentDb>().get::<PerfDef>("reference").cloned().expect("the reference scene")
}

/// Monsters where `near` says, given where the player (e5) is.
fn monsters(app: &mut App, near: impl Fn(Vec3, Vec3) -> bool) -> usize {
    let world = app.world_mut();
    let player = ai_core::Id(ai_core::AiId(5)).entity(world).unwrap();
    let at = world.get::<Transform>(player).unwrap().translation;
    let mut monsters = world.query_filtered::<&Transform, With<shards::npc::Npc>>();
    monsters.iter(world).filter(|t| near(t.translation, at)).count()
}

/// Headless the reference scene loads the wilds (timed), sets the fight up and measures the ticks;
/// nothing is judged here (a test build, on a machine busy with other tests).
#[test]
fn the_reference_scene_loads_sets_up_and_measures_its_ticks_headless() {
    let _clock = one_clock();
    let mut app = sandbox::test_app(42);
    let def = reference(&app);
    let measured = shards::bench::play(&mut app, &def).unwrap();
    assert_eq!(measured.ticks.ticks, def.ticks);
    assert!(measured.region_load.is_some() && measured.frames.is_none(), "{measured:?}");
    // The wilds span x -64..64, z 48..176: its camps, its den and the twelve of the setup.
    let in_the_region = monsters(&mut app, |m, _| (-64.0..64.0).contains(&m.x) && (48.0..176.0).contains(&m.z));
    let in_the_fight = monsters(&mut app, |m, player| m.distance(player) < 12.0);
    assert_eq!((in_the_region, in_the_fight), (15, 13), "the camps are 40 m away at least");
    let verdicts = shards::bench::judge(&def.budgets, &measured, false);
    assert!(verdicts.iter().all(|v| v.met.is_none()), "{verdicts:?}");
    let lines: Vec<String> = verdicts.iter().map(|v| v.line()).collect();
    assert!(lines[0].starts_with("-     region load ") && lines.iter().any(|l| l.contains("nothing is drawn headless")), "{lines:#?}");
}

/// The scene at a region's full size (`--perf reference96`): the expanse, 8 x 8 tiles of 96 m, loads
/// (timed), the navigation of its four tiles is baked for both body classes (timed, nothing
/// written), the fight is set up at its gate and the ticks are measured; nothing is judged here.
#[test]
fn the_full_size_scene_loads_bakes_its_tiles_and_measures_its_ticks_headless() {
    let _clock = one_clock();
    let mut app = sandbox::test_app(42);
    let def = app.world().resource::<shards::content::ContentDb>().get::<PerfDef>("reference96").cloned().expect("reference96");
    let measured = shards::bench::play(&mut app, &def).unwrap();
    assert_eq!(measured.ticks.ticks, def.ticks);
    assert!(measured.region_load.is_some() && measured.frames.is_none(), "{measured:?}");
    assert_eq!(measured.nav_bake.map(|(bakes, _)| bakes), Some(8), "4 tiles x 2 body classes: {measured:?}");
    // The expanse spans x -384..384, z 200..968: its three camps 150 m away at least, and the
    // twelve of the setup at the gate.
    let in_the_region = monsters(&mut app, |m, _| (-384.0..384.0).contains(&m.x) && (200.0..968.0).contains(&m.z));
    let in_the_fight = monsters(&mut app, |m, player| m.distance(player) < 12.0);
    assert_eq!((in_the_region, in_the_fight), (15, 12));
    let physics = app.world().resource::<shards::motion::PhysicsWorld>();
    assert!(physics.len() > 1800, "a region of this size has some 1900 colliders: {}", physics.len());
    let lines: Vec<String> = shards::bench::judge(&def.budgets, &measured, false).iter().map(|v| v.line()).collect();
    assert!(lines[0].starts_with("-     region load ") && lines[1].starts_with("-     nav bake "), "{lines:#?}");
}

/// Offscreen the game plays the scene in real time and its frames are measured: the main and the
/// render world, the GPU (where it measures), the meshes drawn.
#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p sandbox --test perf -- --ignored offscreen"]
fn an_offscreen_run_measures_its_frames_and_what_they_draw() {
    let _clock = one_clock();
    let mut app = shards::testing::test_app(
        |config| (config.headless, config.offscreen) = (false, true),
        |app| {
            app.add_plugins((sandbox::SandboxPlugin, sandbox::visuals::VisualsPlugin));
        },
    );
    let def = PerfDef { warmup: 60, ticks: 120, ..reference(&app) };
    let measured = shards::bench::play(&mut app, &def).unwrap();
    assert_eq!(measured.ticks.ticks, 120, "real time, then paused again");
    assert!(ai_core::sim::is_paused(app.world()));
    let frames = measured.frames.expect("frames are drawn offscreen");
    assert!(frames.frames > 10 && frames.max >= frames.avg && frames.avg > Duration::ZERO, "{frames:?}");
    assert!(frames.fps.is_some_and(|fps| fps > 0.0) && frames.render > Duration::ZERO, "{frames:?}");
    assert!(measured.camera_meshes > 0 && measured.visible_meshes > 0 && measured.visible_meshes <= measured.meshes, "{measured:?}");
    let status = ai_core::status::status(app.world());
    let shown = &status["frames"];
    assert!(shown["frames"].as_u64().is_some_and(|n| n > 0) && shown["budget_ms"] == 16.667, "{shown}");
    assert_eq!(status["counts"]["camera_meshes"], measured.camera_meshes, "{status}");
}
