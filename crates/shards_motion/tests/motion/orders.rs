//! A `move` or `move_to` order that makes no progress ends as "blocked"; one that is only slow does not.

use ai_core::AiJournal;
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::SPRINT;
use shards_core::Intent;
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

fn journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect()
}

/// The stall stretch in ticks at the tests' 60 Hz.
const STALL_TICKS: u64 = 120;

#[test]
fn a_move_into_a_wall_ends_as_blocked_and_the_next_order_runs() {
    assert_eq!(STALL_SECONDS, 2.0, "the tests count the stretch in ticks");
    let mut app = bare_engine("order_blocked_wall");
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(5.0, 0.0, -3.0), Vec3::new(6.0, 4.0, 3.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    // A sprint order against the wall, with a second order behind it.
    edit_intent(&mut app, e, |i| i.set_held(SPRINT, true));
    ok(&mut app, "move", json!({"dir": "E", "meters": 20}));
    ok(&mut app, "move_to", json!({"target": {"x": 0, "z": 4}, "queue": true}));
    // It reaches the wall (4.5 m) in about half a second and stands there: nothing ends for less than the stretch.
    step(&mut app, 60);
    assert_eq!(intent(&app, e).orders.len(), 2, "still walking after a second: {:?}", intent(&app, e).orders);
    step(&mut app, 60);
    assert_eq!(intent(&app, e).orders.len(), 2, "the body stood at the wall for less than the stretch");
    for _ in 0..120 {
        if journal(&app).iter().any(|l| l.contains("ended: blocked")) {
            break;
        }
        step(&mut app, 1);
    }
    let orders = intent(&app, e).orders;
    assert!(
        !matches!(orders.front(), Some(shards_core::Order::MoveTo { target: shards_core::Target::Point(p), .. }) if p.x > 4.0),
        "the order into the wall ended: {orders:?}"
    );
    let lines = journal(&app);
    assert!(
        lines.iter().any(|l| l.contains("e1 move_to (20.0, 0.0) ended: blocked (no closer than 0.10 m in 2.0 s)")),
        "{lines:#?}"
    );
    let progress = app.world().get::<OrderProgress>(e).unwrap();
    assert!(progress.ended.as_deref().is_some_and(|s| s.ends_with(": blocked")), "{progress:?}");
    // The second order was taken up: the body walks to it.
    step(&mut app, 600);
    assert!(intent(&app, e).orders.is_empty(), "the queued order ran and arrived: {:?} at {}", intent(&app, e).orders, feet(&app, e));
    assert!((feet(&app, e) - Vec3::new(0.0, 0.0, 4.0)).length() < 0.6, "{}", feet(&app, e));
}

#[test]
fn a_slow_but_progressing_walk_is_never_ended() {
    let mut app = bare_engine("order_slow_walk");
    floor(app.world_mut());
    // A crawl: 0.3 m/s is 0.6 m in a stretch, six times the progress asked for.
    let e = player(app.world_mut(), Vec3::ZERO);
    app.world_mut().entity_mut(e).insert(CharacterParams { run_speed: 0.3, walk_speed: 0.15, ..default() });
    step(&mut app, 3);
    ok(&mut app, "move", json!({"dir": "E", "meters": 3}));
    step(&mut app, STALL_TICKS * 4);
    assert_eq!(intent(&app, e).orders.len(), 1, "a slow walk goes on: {:?}", intent(&app, e).orders);
    assert!(feet(&app, e).x > 1.0, "{}", feet(&app, e));
    assert!(!journal(&app).iter().any(|l| l.contains("ended: blocked")), "{:?}", journal(&app));
    // And it arrives.
    step(&mut app, STALL_TICKS * 8);
    assert!(intent(&app, e).orders.is_empty() && feet(&app, e).x > 2.6, "{} {:?}", feet(&app, e), intent(&app, e).orders);
    assert!(app.world().get::<OrderProgress>(e).is_none_or(|p| p.ended.is_none()));
}

#[test]
fn a_walk_that_goes_round_a_corner_for_a_while_is_not_blocked() {
    // The route leaves the goal's straight line for a second and a half: the clock restarts at
    // every metre gained, so only a stretch of 2 s without 0.1 m ends an order.
    let mut app = bare_engine("order_corner");
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(-0.5, 0.0, -5.5), Vec3::new(0.5, 3.0, -4.5));
    player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    ok(&mut app, "move_to", json!({"target": {"x": 0, "z": -10}, "path": true}));
    step(&mut app, 1200);
    assert!(!journal(&app).iter().any(|l| l.contains("ended: blocked")), "{:?}", journal(&app));
}

#[test]
fn a_replaced_order_starts_its_stretch_again() {
    let mut app = bare_engine("order_replaced");
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(2.0, 0.0, -3.0), Vec3::new(3.0, 4.0, 3.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    ok(&mut app, "move", json!({"dir": "E", "meters": 20}));
    step(&mut app, STALL_TICKS);
    // 100 ticks into the stretch, a different order replaces it: its own stretch begins.
    ok(&mut app, "move", json!({"dir": "N", "meters": 1.0}));
    ok(&mut app, "move", json!({"dir": "E", "meters": 20}));
    step(&mut app, STALL_TICKS - 10);
    assert_eq!(intent(&app, e).orders.len(), 1, "a new order has its whole stretch");
}

/// Runs one tick at a time until the body has no order left; the tick it ended on.
fn ticks_until_no_order(app: &mut App, e: Entity, limit: u64) -> u64 {
    for _ in 0..limit {
        if intent(app, e).orders.is_empty() {
            break;
        }
        step(app, 1);
    }
    assert!(intent(app, e).orders.is_empty(), "the order never ended: {:?}", intent(app, e).orders);
    app.world().resource::<ai_core::SimTick>().0
}

#[test]
fn the_count_waits_while_an_action_holds_the_body() {
    // `locked` is what the action machine sets for an attack, a dodge or a stagger. A held body is
    // not walking, so a hold of several stretches ends nothing; the count starts when it is free.
    let mut app = bare_engine("order_held");
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(5.0, 0.0, -3.0), Vec3::new(6.0, 4.0, 3.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    ok(&mut app, "move", json!({"dir": "E", "meters": 20}));
    // 100 ticks of walking: 4.5 m to the wall, nothing ended.
    step(&mut app, 60);
    drive(&mut app, STALL_TICKS * 3, |w| {
        w.get_mut::<LocomotionInput>(e).unwrap().locked = true;
        true
    });
    assert_eq!(intent(&app, e).orders.len(), 1, "held for three stretches: {:?}", journal(&app));
    // Free again: the order ends after its own stretch, not at once.
    step(&mut app, STALL_TICKS - 30);
    assert_eq!(intent(&app, e).orders.len(), 1, "the count starts again when the body is free");
    step(&mut app, 30);
    assert!(intent(&app, e).orders.is_empty(), "{:?}", journal(&app));
    assert!(journal(&app).iter().any(|l| l.contains("ended: blocked")), "{:?}", journal(&app));
}

#[test]
fn a_chase_of_a_runner_is_not_blocked_but_a_body_stuck_against_a_wall_is() {
    let mut app = bare_engine("order_chase");
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    // A crawl after a target that walks away at the same pace: the distance never drops.
    app.world_mut().entity_mut(e).insert(CharacterParams { run_speed: 0.5, walk_speed: 0.5, ..default() });
    let runner = app.world_mut().spawn((ai_core::AiKind::new("Runner"), Transform::from_xyz(3.0, 0.0, 0.0))).id();
    step(&mut app, 3);
    let id = *app.world().get::<ai_core::AiId>(runner).unwrap();
    ok(&mut app, "move_to", json!({"target": id.to_string(), "within": 0.5}));
    drive(&mut app, STALL_TICKS * 5, |w| {
        w.get_mut::<Transform>(runner).unwrap().translation.x += 0.5 / 60.0;
        true
    });
    assert_eq!(intent(&app, e).orders.len(), 1, "a chase goes on: {:?} at {}", journal(&app), feet(&app, e));
    assert!(feet(&app, e).x > 2.0, "it did walk: {}", feet(&app, e));

    // The same target behind a wall the body cannot pass: it moves away, the body does not, and the order ends.
    block(app.world_mut(), Vec3::new(10.0, 0.0, -20.0), Vec3::new(11.0, 4.0, 20.0));
    app.world_mut().entity_mut(e).insert(CharacterParams::default());
    app.world_mut().get_mut::<Transform>(runner).unwrap().translation = Vec3::new(14.0, 0.0, 0.0);
    ok(&mut app, "move_to", json!({"target": id.to_string(), "within": 0.5}));
    drive(&mut app, STALL_TICKS * 5, |w| {
        let mut t = w.get_mut::<Transform>(runner).unwrap();
        t.translation.x += 0.5 / 60.0;
        true
    });
    assert!(intent(&app, e).orders.is_empty(), "stuck at the wall: {} {:?}", feet(&app, e), intent(&app, e).orders);
    assert!(journal(&app).iter().any(|l| l.contains("ended: blocked")), "{:?}", journal(&app));
}

#[test]
fn a_runner_that_pulled_away_and_stopped_is_still_closed_on() {
    // The runner outpaces the body, so the distance grows past the nearest the body came; then it
    // stops. The walk to it is progress from where it stopped, not a stall measured from before.
    let mut app = bare_engine("order_chase_stop");
    floor(app.world_mut());
    let e = player(app.world_mut(), Vec3::ZERO);
    app.world_mut().entity_mut(e).insert(CharacterParams { run_speed: 1.0, walk_speed: 1.0, ..default() });
    let runner = app.world_mut().spawn((ai_core::AiKind::new("Runner"), Transform::from_xyz(2.0, 0.0, 0.0))).id();
    step(&mut app, 3);
    let id = *app.world().get::<ai_core::AiId>(runner).unwrap();
    ok(&mut app, "move_to", json!({"target": id.to_string(), "within": 0.5}));
    drive(&mut app, STALL_TICKS * 2, |w| {
        w.get_mut::<Transform>(runner).unwrap().translation.x += 3.0 / 60.0;
        true
    });
    let gap = app.world().get::<Transform>(runner).unwrap().translation.x - feet(&app, e).x;
    assert!(gap > 6.0, "the runner pulled away: {gap}");
    // It stands still now, more than a stretch of walking away: the order must not end as blocked.
    step(&mut app, STALL_TICKS + 30);
    assert_eq!(intent(&app, e).orders.len(), 1, "still closing in: {:?} at {}", journal(&app), feet(&app, e));
    assert!(!journal(&app).iter().any(|l| l.contains("ended: blocked")), "{:?}", journal(&app));
}

#[test]
fn a_restored_snapshot_ends_the_order_on_the_same_tick() {
    // The stall count (`OrderProgress`) is a lazy component the snapshot keeps: a game saved
    // 1.5 s into a stall and restored ends the order when the one that went on would have.
    let mut app = bare_engine("order_snapshot");
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(5.0, 0.0, -3.0), Vec3::new(6.0, 4.0, 3.0));
    let e = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    ok(&mut app, "move", json!({"dir": "E", "meters": 20}));
    // The wall at 4.5 m is reached after ~1 s; 90 more ticks of standing there.
    step(&mut app, 160);
    assert_eq!(intent(&app, e).orders.len(), 1);
    let saved = ai_core::snapshot::capture(app.world_mut()).expect("capture");
    let first = ticks_until_no_order(&mut app, e, 300);
    ai_core::snapshot::restore(app.world_mut(), &saved, true).expect("restore");
    let e = app.world_mut().query_filtered::<Entity, With<Intent>>().single(app.world()).expect("the body");
    assert_eq!(intent(&app, e).orders.len(), 1, "the order is back");
    let second = ticks_until_no_order(&mut app, e, 300);
    assert_eq!(first, second, "the restored game ends the order on the same tick");
}

#[test]
fn the_tool_descriptions_state_the_stretch_and_the_distance_the_constants_name() {
    let app = bare_engine("order_docs");
    let specs = app.world().resource::<ai_core::tools::AiTools>().specs(ai_core::Scope::Dev);
    let text = |name: &str| specs.iter().find(|s| s.name == name).map(|s| s.description.clone()).unwrap_or_default();
    let (seconds, metres) = (format!("{STALL_SECONDS} s"), format!("{STALL_PROGRESS} m"));
    assert!(text("move_to").contains(&format!("{seconds} ({metres})")), "{}", text("move_to"));
    assert!(text("move").contains(&seconds), "{}", text("move"));
}
