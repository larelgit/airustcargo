//! Determinism: the same fight gives the same world and journal, replays verify, and
//! every system has a total order.

use ai_core::replay::{self, Replay};
use ai_core::AiReplay;
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{ActionState, InputBuffer};
use shards_core::Intent;

use crate::common::*;

/// Hero and brute, three monsters' worth of moves and a hero playing through tools
/// and the keyboard.
fn arena(app: &mut App) -> (Entity, Entity) {
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.5), S);
    fighter(world, "Dummy", "dummy", "monsters", Vec3::new(4.0, 0.0, -1.0), Vec3::NEG_X);
    (hero, brute)
}

fn fight(app: &mut App, hero: Entity, brute: Entity) {
    let b = id(app, brute);
    ok(app, "lock_on", json!({"target": b}));
    press(app, brute, "light");
    ok(app, "light_attack", json!({}));
    step(app, 18);
    ok(app, "light_attack", json!({}));
    step(app, 20);
    press(app, brute, "heavy");
    step(app, 10);
    ok(app, "dodge", json!({"dir": "S"}));
    step(app, 60);
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = N;
    step(app, 20);
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::ZERO;
    press(app, brute, "technique.1");
    ok(app, "block", json!({}));
    step(app, 90);
    ok(app, "block", json!({"on": false}));
    ok(app, "attack", json!({"target": b, "input": "heavy", "repeat": true}));
    step(app, 200);
    press(app, brute, "technique.2");
    step(app, 60);
    ok(app, "use_item", json!({"item": "flask"}));
    step(app, 100);
}

/// Every fighter's gameplay state, bit-exact.
fn state(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query::<(
        &ai_core::AiId,
        &Transform,
        &Health,
        Option<&Stamina>,
        Option<&Poise>,
        &ActionState,
        &InputBuffer,
    )>();
    let mut out: Vec<String> = q.iter(world).map(|c| format!("{c:?}")).collect();
    out.sort();
    out
}

#[test]
fn the_same_fight_twice_is_the_same_fight() {
    let (mut a, mut b) = (app(), app());
    let (ha, ba) = arena(&mut a);
    let (hb, bb) = arena(&mut b);
    fight(&mut a, ha, ba);
    fight(&mut b, hb, bb);
    assert_eq!(state(&mut a), state(&mut b));
    assert_eq!(journal(&a), journal(&b));
    let story = journal(&a);
    // A real fight: telegraphs, hits both ways, a rejected press, staggers, a kill, a heal.
    for needle in ["winds up charge", "e2 -> e1", "e1 -> e2", "heavy rejected", "staggered", "e2 died", "healed"] {
        assert!(story.iter().any(|l| l.contains(needle)), "no `{needle}` in {story:#?}");
    }
}

#[test]
fn a_recorded_fight_plays_back_identically() {
    let mut original = app();
    let (hero, brute) = arena(&mut original);
    fight(&mut original, hero, brute);
    let recording = replay::save(original.world());
    let recording: Replay = serde_json::from_str(&serde_json::to_string(&recording).unwrap()).unwrap();
    assert!(recording.checks.len() >= 8, "a check every second: {}", recording.checks.len());

    let mut copy = app();
    arena(&mut copy);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().expect("playback finished");
    assert!(report.identical(), "{report}");
    assert_eq!(state(&mut original), state(&mut copy));
    assert_eq!(journal(&original), journal(&copy));
}

#[test]
fn a_changed_number_diverges_where_it_shows() {
    let mut original = app();
    let (hero, brute) = arena(&mut original);
    fight(&mut original, hero, brute);
    let recording = replay::save(original.world());

    // The same session with a tougher brute: its health differs from the first hit on.
    let mut copy = app();
    let (_, brute) = arena(&mut copy);
    copy.world_mut().get_mut::<Health>(brute).unwrap().max = 101.0;
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().unwrap();
    assert_eq!(report.diverged_at, Some(60), "{report}");
}

#[test]
fn gameplay_systems_have_a_total_order() {
    let mut app = app();
    let strict = ScheduleBuildSettings { ambiguity_detection: LogLevel::Error, ..default() };
    let result = app.world_mut().schedule_scope(FixedUpdate, |world, schedule| {
        schedule.set_build_settings(strict);
        schedule.initialize(world).map(|_| ()).map_err(|e| e.to_string(schedule.graph(), world))
    });
    if let Err(e) = result {
        panic!("ambiguous FixedUpdate systems make replays diverge:\n{e}");
    }
}
