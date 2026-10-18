//! Hit stop (C2) holds the picture and the real-time clock, never the simulation: a replay recorded
//! while it held the clock verifies identical with it off, headless, and in a window that plays it.

use sandbox::SandboxPlugin;
use shards::ai_core::display::Offscreen;
use shards::ai_core::replay::{self, Replay};
use shards::ai_core::{sim, AiReplay, ClockHold, SimTick};
use shards::bevy::time::TimeUpdateStrategy;
use shards::hitstop::{HitStops, Outcome};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{ok, BOSS};

/// A sandbox that counts as drawing (an offscreen image no GPU draws into), in real time: one
/// frame is one tick of real time, as a window at 60 Hz runs it.
fn windowed(seed: u64) -> App {
    let mut app = shards::testing::test_app(
        |config| config.seed = seed,
        |app| {
            app.add_plugins(SandboxPlugin);
            app.insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
        },
    );
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app
}

/// Plays the sandbox for `frames` frames in real time: the player fights the boss in god mode.
fn fight(app: &mut App, frames: u32) {
    app.update();
    ok(app, "god", json!({}));
    ok(app, "attack", json!({"target": BOSS, "repeat": true}));
    sim::resume(app.world_mut());
    for _ in 0..frames {
        app.update();
    }
}

/// What a game's tick count and checks say of its end: the tick and the state hash.
fn end(app: &App) -> (u64, u64) {
    (app.world().resource::<SimTick>().0, replay::state_hash(app.world()))
}

#[test]
fn a_replay_recorded_while_the_clock_was_held_verifies_with_hit_stop_off_headless_and_in_a_window() {
    // Recorded alone with a picture in real time, hit stop on: the swings hold the clock.
    let mut original = windowed(42);
    fight(&mut original, 600);
    let holds = original.world().resource::<ClockHold>();
    assert!(holds.count >= 2, "the fight held the clock {} times: the test would prove nothing", holds.count);
    assert_eq!(holds.ticks_inside, 0, "no tick ran inside a hold");
    let held_s = holds.total.as_secs_f64();
    assert!(original.world().resource::<HitStops>().clock >= 2);
    // The held frames are missing from the tick count, and only they.
    let ticks = original.world().resource::<SimTick>().0;
    let held_frames = (held_s * 60.0).round() as u64;
    assert_eq!(ticks + held_frames, 600, "ticks {ticks}, clock held {held_s} s: every frame ticked or was held");
    sim::pause(original.world_mut());
    let replay: Replay = {
        let saved = replay::save(original.world());
        shards::serde_json::from_str(&shards::serde_json::to_string(&saved).unwrap()).unwrap()
    };
    let want = end(&original);
    assert_eq!(want.0, replay.end.tick);

    // Headless (nothing is ever held): the same game, tick for tick.
    let mut headless = sandbox::test_app(42);
    replay::play(headless.world_mut(), replay.clone()).unwrap();
    ok(&mut headless, "step", json!({"ticks": replay.end.tick}));
    headless.update();
    let report = headless.world().resource::<AiReplay>().report().cloned().expect("finished");
    assert!(report.identical(), "{report}");
    assert_eq!(end(&headless), want);
    assert_eq!(headless.world().resource::<HitStops>().clock, 0);
    assert!(headless.world().resource::<HitStops>().events >= 2, "the blows were still seen");

    // Drawn, in real time, hit stop on, playing the recording back: a replay is never held either.
    let mut window = windowed(42);
    replay::play(window.world_mut(), replay.clone()).unwrap();
    window.update();
    sim::resume(window.world_mut());
    for _ in 0..replay.end.tick + 5 {
        if window.world().resource::<AiReplay>().report().is_some() {
            break;
        }
        window.update();
    }
    let report = window.world().resource::<AiReplay>().report().cloned().expect("the playback finished");
    assert!(report.identical(), "{report}");
    let stops = window.world().resource::<HitStops>();
    assert_eq!(stops.clock, 0, "a replay playing holds nothing");
    assert!(stops.log.iter().all(|r| r.outcome == Outcome::Skipped("a replay is playing".into())), "{:?}", stops.log.back());
    assert_eq!(window.world().resource::<ClockHold>().count, 0);

    // Drawn, in real time, hit stop turned off by the player: no hold, and at the same tick the same game
    // as the one that held its clock for the blows (the hold moved nothing in the simulation).
    let mut off = windowed(42);
    ok(&mut off, "settings_set", json!({"group": "accessibility", "key": "hit_stop", "value": "0"}));
    fight(&mut off, ticks as u32);
    assert_eq!(off.world().resource::<ClockHold>().count, 0);
    assert_eq!(end(&off), want, "the same tick, the same state, whether the clock was held or not");
}
