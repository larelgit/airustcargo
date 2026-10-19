//! A `move` or `move_to` order that makes no progress ends as "blocked"; one that is only slow does not.

use ai_core::AiJournal;
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::SPRINT;
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

