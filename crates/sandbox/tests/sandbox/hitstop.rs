//! Hit stop (C2) holds the picture and the real-time clock, never the simulation: a replay recorded
//! while it held the clock verifies identical with it off, headless, and in a window that plays it.

use std::time::Duration;

use sandbox::SandboxPlugin;
use shards::ai_core::display::Offscreen;
use shards::ai_core::replay::{self, Replay};
use shards::ai_core::{sim, AiReplay, ClockHold, SimTick};
use shards::bevy::time::TimeUpdateStrategy;
use shards::hitstop::{HitStops, Outcome};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{ok, text, BOSS, PLAYER, SKELETONS};

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

#[test]
fn a_blow_between_two_others_holds_nothing_and_the_players_blow_does() {
    let mut app = windowed(42);
    app.update();
    sim::resume(app.world_mut());
    // The boss strikes a skeleton: neither is the local player, so no rule of the table gives a hold.
    let out = ok(&mut app, "hitstop_play", json!({"event": "damaged", "attacker": BOSS, "target": SKELETONS[0]}));
    assert!(text(&out).contains("no hold"), "{}", text(&out));
    assert!(!app.world().resource::<ClockHold>().is_pending());
    assert_eq!(app.world().resource::<HitStops>().clock, 0);
    // The same blow on the player holds the clock.
    let out = ok(&mut app, "hitstop_play", json!({"event": "damaged", "attacker": BOSS, "target": PLAYER}));
    assert!(text(&out).contains("hit 0.06 s"), "{}", text(&out));
    assert!(app.world().resource::<ClockHold>().is_pending());
    assert_eq!(app.world().resource::<HitStops>().clock, 1);
}

#[test]
fn a_manual_hold_keeps_the_tables_ceiling_and_is_named_manual() {
    let mut app = windowed(42);
    app.update();
    sim::resume(app.world_mut());
    let out = ok(&mut app, "hitstop_play", json!({"seconds": 5.0}));
    // The sandbox table's max_seconds is 0.12, below the engine's 0.5.
    assert!(text(&out).contains("manual 0.12 s"), "{}", text(&out));
    // Event names read without caring for case or underscores.
    ok(&mut app, "hitstop_play", json!({"event": "PerfectEvaded", "attacker": BOSS, "target": PLAYER}));
}

#[test]
fn a_tick_run_by_hand_inside_a_hold_breaks_the_rule() {
    // The rule's real path, not a hand-written counter: the clock is held, then something runs the
    // fixed schedule on its own (as a system that ticked the game inside a hold would).
    let mut app = windowed(42);
    app.update();
    sim::resume(app.world_mut());
    app.world_mut().resource_mut::<ClockHold>().request(std::time::Duration::from_secs(3600));
    app.update();
    app.update();
    assert!(app.world().resource::<ClockHold>().is_held(), "the hold began");
    assert_eq!(shards::hitstop::no_ticks_inside(app.world()), None, "a hold that works leaves the rule silent");
    let before = app.world().resource::<SimTick>().0;
    app.world_mut().run_schedule(shards::bevy::app::FixedMain);
    assert!(app.world().resource::<SimTick>().0 > before, "the schedule ran a tick");
    let said = shards::hitstop::no_ticks_inside(app.world()).expect("a tick inside a hold is caught");
    assert!(said.contains("1 simulation tick(s) ran while the clock was held"), "{said}");
}

/// Where member `member` draws the entity `id`: its `GlobalTransform`, which the net's
/// interpolation writes for a remote body on a client that draws.
fn drawn_at(stand: &shards::net::Stand, member: usize, id: &str) -> Vec3 {
    let world = stand.world(member);
    let entity = world.resource::<shards::ai_core::AiIndex>().get(AiId::parse(id).unwrap()).unwrap();
    world.get::<GlobalTransform>(entity).unwrap().translation()
}

/// One co-op session of two clients over 150 ms links; the second draws and, when `hold`, holds
/// the first's body for 0.2 s of its real time. Returns the second client's state hash at the end.
fn held_session(hold: bool) -> u64 {
    use crate::net::{body, lossy, position, stand_with, C1, C2, HOST};
    let mut stand = stand_with(2, lossy());
    stand.world_mut(C2).insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
    let mover = body(&stand, C1);
    stand.run(30);
    let start = position(&stand, C2, &mover);
    let walking = stand.call(C1, "move", json!({"dir": "E", "meters": 40.0}));
    assert!(walking.ok, "{}", walking.result);
    // Wait until C2 shows the walker well under way, so its drawn pose moves every frame.
    for _ in 0..120 {
        frame(&mut stand);
    }
    assert!(drawn_at(&stand, C2, &mover).distance(start) > 1.5, "C2 draws the walker moving");
    let before = drawn_at(&stand, C2, &mover);
    frame(&mut stand);
    frame(&mut stand);
    assert!(drawn_at(&stand, C2, &mover).distance(before) > 0.01, "not held, the drawn body moves");

    let id = AiId::parse(&mover).unwrap();
    let now = stand.world(C2).resource::<Time<Real>>().elapsed();
    let until = now + Duration::from_millis(200);
    if hold {
        stand.world_mut(C2).resource_mut::<shards::ai_core::PresentationHolds>().hold(id, now, until);
    }
    frame(&mut stand);
    let frozen = drawn_at(&stand, C2, &mover);
    let truth = position(&stand, C2, &mover);
    let mut steps = vec![frozen];
    let mut caught = frozen;
    for _ in 0..40 {
        frame(&mut stand);
        let at = stand.world(C2).resource::<Time<Real>>().elapsed();
        let drawn = drawn_at(&stand, C2, &mover);
        if hold && at < until {
            assert!(drawn.distance(frozen) < 1e-3, "held: {frozen} -> {drawn} at {at:?}");
        } else {
            steps.push(drawn);
        }
        caught = drawn;
        if at >= until + Duration::from_millis(220) {
            break;
        }
    }
    assert!(position(&stand, C2, &mover).distance(truth) > 0.5, "the simulated body ran on");
    if hold {
        // Back to the live drawn pose, a good way past where it stood.
        assert!(caught.distance(frozen) > 1.0, "caught up: {frozen} -> {caught}");
        // No jump larger than the blend allows: the walker's own pace plus the gap closing over 0.2 s.
        let worst = steps.windows(2).map(|w| w[0].distance(w[1])).fold(0.0, f32::max);
        assert!(worst < 0.6, "a step of {worst} m while catching up");
    }
    // The host's copy is the simulation's, whatever C2 draws.
    assert!(position(&stand, HOST, &mover).x >= position(&stand, C2, &mover).x - 0.01);
    replay::state_hash(stand.world(C2))
}

#[test]
fn a_client_draws_a_held_remote_body_standing_still_then_catching_up_and_the_simulation_is_the_same() {
    let with = held_session(true);
    let without = held_session(false);
    assert_eq!(with, without, "a picture hold changes what is drawn, never the state hash");
}

/// One frame of the client that draws: a stand tick, then the real time of a tick passes on it and
/// its `PostUpdate` runs (the stand runs ticks, not frames).
fn frame(stand: &mut shards::net::Stand) {
    stand.run(1);
    let world = stand.world_mut(crate::net::C2);
    world.resource_mut::<Time<Real>>().advance_by(Duration::from_micros(16_667));
    world.run_schedule(PostUpdate);
}
