//! The action machine: combos, the input buffer, costs, effects, displacement.

use serde_json::json;
use shards_action::prelude::*;
use shards_action::{Charges, InputBuffer};
use shards_core::Intent;
use shards_motion::MotionState;

use crate::common::*;
use bevy::prelude::*;

#[test]
fn a_three_hit_combo_chains_only_inside_cancel_windows() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let target = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(0.0, 0.0, -1.8), S);
    let (h, t) = (id(&app, hero), id(&app, target));

    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: light_1");
    step(&mut app, 18); // light_1 at 0.30 s: its cancel window opens
    assert_eq!(action(&app, hero), "light_1");
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: light_2");
    step(&mut app, 18);
    assert_eq!(action(&app, hero), "light_2");
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: light_3");
    step(&mut app, 1);
    assert_eq!(stamina(&app, hero), 65.0, "10 + 10 + 15, too close together to regenerate");
    step(&mut app, 59);
    assert_eq!(action(&app, hero), "idle");

    let hits = lines(&app, &format!("{h} -> {t}"));
    assert_eq!(hits.len(), 3, "{hits:#?}");
    assert!(hits[0].ends_with("light_1 -20 (hp 80)") && hits[1].contains("light_2 -20 (hp 60)"), "{hits:#?}");
    assert!(hits[2].ends_with("light_3 -30 (hp 30)"), "{hits:#?}");
}

#[test]
fn a_dodge_cancels_an_attack_only_inside_its_window() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    ok(&mut app, "heavy_attack", json!({}));
    step(&mut app, 40);
    // The heavy attack has no cancel window: the roll waits for its end.
    assert_eq!(fails(&mut app, "dodge", json!({"dir": "S"})), "busy: recovery heavy 0.48s (ready in 0.48s)");
    step(&mut app, 40);
    ok(&mut app, "light_attack", json!({}));
    step(&mut app, 21);
    assert_eq!(ok(&mut app, "dodge", json!({"dir": "S"})), "ok: roll (S)");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "roll", "cut light_1's recovery short");
}

#[test]
fn a_press_outside_the_window_is_buffered_then_rejected() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let h = id(&app, hero);
    ok(&mut app, "light_attack", json!({}));
    step(&mut app, 2);

    // The tool refuses what would only expire, and says when it would work.
    let early = fails(&mut app, "light_attack", json!({}));
    assert_eq!(early, "busy: windup light_1 0.17s (ready in 0.27s, when a cancel window opens)");

    // A keyboard press is buffered anyway...
    press(&mut app, hero, "light");
    step(&mut app, 1);
    let queued = app.world().get::<InputBuffer>(hero).unwrap().queued.clone();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].input, "light");
    // ...and rejected when its 0.15 s run out, with the reason.
    step(&mut app, 9);
    assert!(app.world().get::<InputBuffer>(hero).unwrap().queued.is_empty());
    let rejected = lines(&app, "rejected");
    assert_eq!(rejected.len(), 1, "{rejected:#?}");
    assert_eq!(rejected[0], format!("12 {h} light rejected: busy (active light_1 0.10s)"));
    step(&mut app, 40);
    assert_eq!(action(&app, hero), "idle", "no light_2 ever started");
}

#[test]
fn a_press_just_before_the_window_is_queued_and_starts_on_time() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    ok(&mut app, "light_attack", json!({}));
    step(&mut app, 12); // light_1 at 0.20 s; the window opens at 0.30 s
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: queued, light starts in 0.10s");
    assert_eq!(app.world().get::<Intent>(hero).unwrap().pressed.len(), 1, "the tool only pressed");
    step(&mut app, 6);
    assert_eq!(action(&app, hero), "light_1");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "light_2", "it starts the tick the window opened");
    assert!(lines(&app, "rejected").is_empty());
}

#[test]
fn a_heavy_attack_costs_stamina_and_waits_for_the_regeneration_delay() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    assert_eq!(ok(&mut app, "heavy_attack", json!({})), "ok: heavy");
    step(&mut app, 1);
    assert_eq!(stamina(&app, hero), 60.0);
    step(&mut app, 47); // 0.8 s after spending: still nothing back
    assert_eq!(stamina(&app, hero), 60.0);
    step(&mut app, 2);
    assert!(stamina(&app, hero) > 60.0, "regenerates after its delay");

    step(&mut app, 70); // the heavy attack is over
    let have = stamina(&app, hero);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().spend(have - 20.0);
    let err = fails(&mut app, "heavy_attack", json!({}));
    assert_eq!(err, "not enough stamina: 20/40 (ready in 1.5s)");
    // 0.8 s of delay, then 20 stamina at 30/s: 48 + 40 ticks; it starts in the tick after.
    step(&mut app, 88);
    assert_eq!(ok(&mut app, "heavy_attack", json!({})), "ok: heavy");
}

#[test]
fn a_flask_heals_on_its_frame_and_runs_out_of_charges() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let h = id(&app, hero);
    app.world_mut().get_mut::<Health>(hero).unwrap().current = 30.0;

    assert_eq!(ok(&mut app, "use_item", json!({"item": "flask"})), "ok: drink");
    step(&mut app, 29);
    assert_eq!(hp(&app, hero), 30.0, "drinking takes time");
    step(&mut app, 1);
    assert_eq!(hp(&app, hero), 70.0, "the heal lands 0.5 s in");
    assert_eq!(lines(&app, "healed"), [format!("30 {h} healed +40 (hp 70)")]);
    assert_eq!(app.world().get::<Charges>(hero).unwrap().get("flask").unwrap().current, 1);

    step(&mut app, 30);
    ok(&mut app, "use_item", json!({"item": "flask"}));
    step(&mut app, 70);
    assert_eq!(hp(&app, hero), 100.0, "capped at max");
    assert_eq!(fails(&mut app, "use_item", json!({"item": "flask"})), "no flask charges left (0/2)");
    assert_eq!(fails(&mut app, "use_item", json!({"item": "bomb"})), "no item `bomb`; you have: flask");
}

#[test]
fn a_lunge_moves_through_the_controller_and_stops_at_a_wall() {
    let mut app = app();
    let world = app.world_mut();
    let free = hero(world, "hero", Vec3::new(-10.0, 0.0, 0.0), N);
    let blocked = fighter(world, "Hero", "hero", "heroes", Vec3::new(10.0, 0.0, 0.0), N);
    wall(world, Vec3::new(8.0, 0.0, -3.0), Vec3::new(12.0, 3.0, -2.0));
    ok(&mut app, "use_technique", json!({"slot": 1}));
    press(&mut app, blocked, "technique.1");
    step(&mut app, 40);
    let moved = feet(&app, free).z;
    assert!((moved + 6.0).abs() < 0.05, "6 m north: {moved}");
    let stopped = feet(&app, blocked).z;
    assert!(stopped > -2.0 + 0.3 && stopped < -1.4, "stopped by the wall: {stopped}");
    assert!((feet(&app, blocked).x - 10.0).abs() < 1e-3, "straight into it, no slide");
    assert_eq!(fails(&mut app, "use_technique", json!({"slot": 2})), "no technique `2`; you have: 1");
}

#[test]
fn sprinting_drains_stamina_and_stops_at_zero() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 15.0;
    let mut intent = app.world_mut().get_mut::<Intent>(hero).unwrap();
    intent.move_dir = N;
    intent.set_held("sprint", true);
    step(&mut app, 10);
    assert!(app.world().get::<MotionState>(hero).unwrap().sprinting);
    assert!(stamina(&app, hero) < 15.0 - 30.0 * 8.0 * DT, "30 per second: {}", stamina(&app, hero));
    step(&mut app, 30);
    assert_eq!(stamina(&app, hero), 0.0);
    assert!(!app.world().get::<MotionState>(hero).unwrap().sprinting, "no stamina, no sprint");
}

#[test]
fn an_exhausted_sprint_stays_off_until_the_button_is_let_go() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 15.0;
    let mut intent = app.world_mut().get_mut::<Intent>(hero).unwrap();
    intent.move_dir = N;
    intent.set_held("sprint", true);
    let mut sprinting = 0;
    for _ in 0..330 {
        step(&mut app, 1);
        sprinting += usize::from(app.world().get::<MotionState>(hero).unwrap().sprinting);
    }
    // 15 stamina at 30 per second last half a second; after that, with the button still
    // down, the stamina recovers instead of pulsing back to zero.
    assert!((25..=32).contains(&sprinting), "sprinted {sprinting} ticks");
    assert!(stamina(&app, hero) > 80.0, "recovering: {}", stamina(&app, hero));
    assert_eq!(ok(&mut app, "dodge", json!({"dir": "N"})), "ok: roll (N)");
    let seen = ai_core::observe::observe(app.world(), &ai_core::observe::ObserveOptions::default());
    assert!(seen.contains("exhausted (release sprint)"), "{seen}");

    // Let go and press again: sprinting is back.
    step(&mut app, 30);
    app.world_mut().get_mut::<Intent>(hero).unwrap().set_held("sprint", false);
    step(&mut app, 2);
    app.world_mut().get_mut::<Intent>(hero).unwrap().set_held("sprint", true);
    step(&mut app, 5);
    assert!(app.world().get::<MotionState>(hero).unwrap().sprinting);
}

#[test]
fn errors_teach_the_fix() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -5.0), S);
    let b = id(&app, brute);
    assert_eq!(
        fails(&mut app, "attack", json!({"target": b, "input": "kick"})),
        "your moveset `hero` has no `kick`; it has: block, dodge, heavy, light, parry, technique.1, use_item.flask"
    );
    assert_eq!(
        fails(&mut app, "dodge", json!({"dir": "up"})),
        "unknown direction `up`; use N, NE, E, SE, S, SW, W or NW"
    );
    ok(&mut app, "god", json!({"id": b}));
    shards_action::kill(app.world_mut(), hero, None, None);
    assert_eq!(fails(&mut app, "light_attack", json!({})), "you are dead; a dev agent can `heal` you");
    assert_eq!(ok(&mut app, "heal", json!({})), format!("healed {} to 100, revived", id(&app, hero)));
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: light_1");
    let none = fails(&mut app, "stagger", json!({"id": 99}));
    assert!(none.contains("e99"), "{none}");
}
