//! Alone with a picture, in real time: a hold keeps the fixed clock still for its real time, no tick
//! runs inside it, none is lost or added, the speed the player chose stays what the tools report,
//! and a pause, a `step` and a loading gate end or refuse it.
//!
//! Real time here is `TimeUpdateStrategy::FixedTimesteps(1)`: every `update` is one frame of exactly
//! one tick of real time (1/60 s), the way a window at 60 Hz runs.

use shards::ai_core::{sim, ClockHold, SimTick};
use shards::bevy::time::TimeUpdateStrategy;
use shards::hitstop::{HitStops, Outcome};

use crate::*;

const TICK: f64 = 1.0 / 60.0;

/// A drawn game in real time with a hero and a brute facing each other, running for a second.
fn running(name: &str) -> (App, Folder, Entity, Entity) {
    let assets = Folder::new(name);
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    sim::resume(app.world_mut());
    for _ in 0..60 {
        app.update();
    }
    (app, assets, hero, brute)
}

fn tick(app: &App) -> u64 {
    app.world().resource::<SimTick>().0
}

fn hit(app: &mut App, a: Entity, b: Entity, weight: HitWeight) {
    app.world_mut().trigger(damaged(a, b, weight, None));
    app.world_mut().flush();
}

/// Frames until the clock runs again (at most `max`), no tick among them but the one that releases
/// it (the frame after the last held one counts a normal frame). Returns the frames it stood still.
fn until_released(app: &mut App, max: u32) -> u32 {
    let at = tick(app);
    for n in 0..max {
        app.update();
        if !app.world().resource::<ClockHold>().is_held() {
            assert_eq!(tick(app), at + 1, "the frame that released the clock ticked once");
            return n;
        }
        assert_eq!(tick(app), at, "no tick ran while the clock was held");
    }
    panic!("still held after {max} frames");
}

#[test]
fn a_blow_holds_the_clock_for_its_real_time_and_no_tick_runs_inside() {
    let (mut app, _assets, hero, brute) = running("hold");
    let before = tick(&app);
    assert!(before >= 59, "the game runs a tick a frame: {before}");
    hit(&mut app, hero, brute, HitWeight::Light);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Clock);
    assert!(app.world().resource::<ClockHold>().is_pending());
    assert_eq!(lines(&app, JournalKind::Local, "hitstop: hit 0.06 s").len(), 1);

    // The clock stands still: 0.06 s is 3.6 ticks of real time, so 4 or 5 frames.
    app.update();
    let held_at = tick(&app);
    assert!(app.world().resource::<ClockHold>().is_held());
    assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 0.0);
    assert_eq!(sim::speed(app.world()), 1.0, "the player's speed is what the tools read");
    assert!(!sim::is_paused(app.world()), "a hold is not a pause");
    assert!(sim::gate_message(app.world()).is_none(), "the gate does not refuse");
    let frames = 1 + until_released(&mut app, 12);
    assert_eq!(tick(&app), held_at + 1, "only the frame that released the clock ticked");
    let hold = app.world().resource::<ClockHold>();
    let span = hold.last.unwrap();
    assert_eq!((span.ticks_inside, hold.ticks_inside, hold.count), (0, 0, 1));
    let real = span.real().as_secs_f64();
    assert!((0.06..0.06 + 1.1 * TICK).contains(&real), "held {real} s for a hold of 0.06 s");
    assert!((3..=5).contains(&frames), "{frames} frames");
    assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 1.0, "the speed came back");

    // And it runs on, a tick a frame, with no burst to catch up.
    let after = tick(&app);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(tick(&app), after + 10);

    // The tool and the status say what happened.
    let said = ok(&mut app, "hitstop", json!({}));
    assert!(said.starts_with("hit stop: solo: holds the real-time clock (no tick is lost)\n"), "{said}");
    assert!(said.contains("now: the clock runs"), "{said}");
    assert!(said.contains("ticks that ran inside a hold: 0 · the last: 0.0"), "{said}");
    assert!(said.contains("clock held (0.06 s)"), "{said}");
    let state = status(&app);
    assert_eq!((state["clock_holds"].as_u64(), state["ticks_inside"].as_u64(), state["clock_held_now"].as_bool()), (Some(1), Some(0), Some(false)));
}

#[test]
fn over_a_second_of_real_time_the_ticks_are_the_frames_less_the_held_ones() {
    let (mut app, _assets, hero, brute) = running("count");
    let start = tick(&app);
    let mut held_frames = 0;
    // 120 frames (2 s); a break at frame 10, a hit at frame 60, a block at 80.
    for frame in 0..120 {
        match frame {
            10 => hit(&mut app, hero, brute, HitWeight::Break),
            60 => hit(&mut app, brute, hero, HitWeight::Light),
            _ => {}
        }
        app.update();
        if app.world().resource::<ClockHold>().is_held() {
            held_frames += 1;
        }
    }
    let ran = tick(&app) - start;
    // A frame counted as held may be the one that ended the hold (the clock was released at its start).
    let held = app.world().resource::<ClockHold>().total.as_secs_f64();
    assert!((0.16..0.16 + 3.0 * TICK).contains(&held), "0.10 + 0.06 s held, measured {held}");
    assert_eq!(ran + (held / TICK).round() as u64, 120, "every frame ticked or was held: ran {ran}, held {held_frames} frames");
    assert_eq!(app.world().resource::<ClockHold>().ticks_inside, 0);
    assert_eq!(app.world().resource::<ClockHold>().count, 2);
}

#[test]
fn overlapping_holds_merge_by_the_later_end() {
    let (mut app, _assets, hero, brute) = running("merge");
    hit(&mut app, hero, brute, HitWeight::Light); // 0.06 s
    app.update();
    app.update();
    hit(&mut app, brute, hero, HitWeight::Break); // 0.10 s more, from now
    let frames = 2 + until_released(&mut app, 20);
    let hold = app.world().resource::<ClockHold>();
    assert_eq!(hold.count, 1, "one hold, not two");
    let real = hold.last.unwrap().real().as_secs_f64();
    // From the first frame held to the end of the second: 2 frames + 0.10 s, not 0.06 + 0.10.
    assert!((0.10 + TICK..0.10 + 4.0 * TICK).contains(&real), "{real} s over {frames} frames");
    // A shorter hold inside a longer one changes nothing.
    hit(&mut app, hero, brute, HitWeight::Break);
    app.update();
    let until = app.world().resource::<ClockHold>().until();
    hit(&mut app, hero, brute, HitWeight::Light);
    assert_eq!(app.world().resource::<ClockHold>().until(), until);
}

#[test]
fn a_speed_set_during_a_hold_is_the_players_and_applies_after_it() {
    let (mut app, _assets, hero, brute) = running("speed");
    hit(&mut app, hero, brute, HitWeight::Break);
    app.update();
    assert!(app.world().resource::<ClockHold>().is_held());
    ok(&mut app, "speed", json!({"x": 0.5}));
    assert_eq!(sim::speed(app.world()), 0.5);
    assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 0.0, "still held");
    let at = tick(&app);
    while app.world().resource::<ClockHold>().is_held() {
        assert_eq!(tick(&app), at, "no tick while held");
        app.update();
    }
    assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 0.5);
    assert_eq!(sim::speed(app.world()), 0.5);
}

#[test]
fn a_pause_a_step_and_the_gate_are_not_the_holds() {
    let (mut app, _assets, hero, brute) = running("pause");
    // The player's pause ends a hold, and stays a pause.
    hit(&mut app, hero, brute, HitWeight::Break);
    app.update();
    assert!(app.world().resource::<ClockHold>().is_held());
    sim::pause(app.world_mut());
    app.update();
    assert!(!app.world().resource::<ClockHold>().is_held());
    assert!(sim::is_paused(app.world()));
    // Paused, a blow holds nothing.
    hit(&mut app, hero, brute, HitWeight::Break);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Skipped("the clock is paused or stepped".into()));
    assert!(!app.world().resource::<ClockHold>().is_pending());
    sim::resume(app.world_mut());

    // A step ends a hold that runs, and runs its ticks.
    hit(&mut app, hero, brute, HitWeight::Break);
    app.update();
    assert!(app.world().resource::<ClockHold>().is_held());
    let at = tick(&app);
    step(&mut app, 5);
    assert_eq!(tick(&app), at + 5);
    assert!(!app.world().resource::<ClockHold>().is_pending());
    assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 1.0);

    // A gate that holds the simulation (loading) holds nothing more.
    sim::resume(app.world_mut());
    app.world_mut().resource_mut::<shards::ai_core::SimGate>().hold("test", "loading");
    app.update();
    hit(&mut app, hero, brute, HitWeight::Break);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Skipped("the game is loading".into()));
    app.world_mut().resource_mut::<shards::ai_core::SimGate>().release("test");
}

#[test]
fn the_cooldown_runs_after_a_hold_not_from_the_start_of_the_app() {
    // The longest cooldown the engine allows: the first blow comes well within it of the app's start.
    let assets = Folder::new("cooldown_first");
    assets.put(
        "content/feel/test.hitstop.ron",
        r#"{"feel": (max_seconds: 0.12, cooldown: 5.0, rules: [(name: "hit", when: (event: Damaged), seconds: 0.06)])}"#,
    );
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    sim::resume(app.world_mut());
    app.update();
    assert_eq!(app.world().resource::<HitStops>().ends, None, "no hold yet");
    hit(&mut app, hero, brute, HitWeight::Light);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Clock);
}

#[test]
fn the_cooldown_keeps_a_flurry_from_stuttering() {
    let assets = Folder::new("cooldown");
    assets.put(
        "content/feel/test.hitstop.ron",
        r#"{"feel": (max_seconds: 0.12, cooldown: 0.2, rules: [(name: "hit", when: (event: Damaged), seconds: 0.06)])}"#,
    );
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    sim::resume(app.world_mut());
    for _ in 0..10 {
        app.update();
    }
    hit(&mut app, hero, brute, HitWeight::Light);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Clock);
    until_released(&mut app, 12);
    // Right after the hold: inside the cooldown.
    hit(&mut app, hero, brute, HitWeight::Light);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Skipped("cooldown".into()));
    for _ in 0..14 {
        app.update();
    }
    hit(&mut app, hero, brute, HitWeight::Light);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Clock);
    assert_eq!(app.world().resource::<ClockHold>().count, 1, "the second is held on the next frame");
}
