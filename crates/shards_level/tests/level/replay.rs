//! Determinism: the same level spawns the same world, a recorded session that spawns one
//! plays back identically, and a recording made against another version of the file diverges.

use ai_core::replay::{self, state_hash, Replay};
use ai_core::{AiKind, AiPlayer, AiReplay};
use bevy::prelude::*;
use serde_json::json;
use shards_core::Intent;
use shards_motion::character;

use crate::common::*;

/// A hero on the floor of the arena, spawned before the level (the same in every run).
fn hero(app: &mut App) -> Entity {
    let (controller, intent, transform) = character("", Vec3::new(0.0, 0.0, 8.0));
    app.world_mut().spawn((Name::new("Hero"), AiKind::new("Hero"), AiPlayer, controller, intent, transform)).id()
}

/// A session: spawn the arena, walk into a pillar and along it.
fn play_session(app: &mut App, hero: Entity) {
    ok(app, "spawn_level", json!({"id": "arena", "x": 2.0}));
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::new(1.0, 0.0, -1.0).normalize();
    step(app, 90);
    ok(app, "spawn_level", json!({"id": "arena", "x": 60.0}));
    ok(app, "level_reload", json!({"id": "arena"}));
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::new(-1.0, 0.0, 0.0);
    step(app, 90);
}

fn feet(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Transform>(e).unwrap().translation
}

#[test]
fn the_same_session_twice_is_the_same_world() {
    let (mut a, mut b) = (app(), app());
    let (ha, hb) = (hero(&mut a), hero(&mut b));
    play_session(&mut a, ha);
    play_session(&mut b, hb);
    assert_eq!(state_hash(a.world()), state_hash(b.world()));
    assert_eq!(feet(&a, ha), feet(&b, hb));
    assert!(feet(&a, ha).distance(Vec3::new(0.0, 0.0, 8.0)) > 3.0, "the hero went somewhere: {}", feet(&a, ha));
    assert_eq!(journal(&a), journal(&b));
}

#[test]
fn a_different_placement_is_a_different_state() {
    let (mut a, mut b) = (app(), app());
    ok(&mut a, "spawn_level", json!({"id": "arena", "x": 2.0}));
    ok(&mut b, "spawn_level", json!({"id": "arena", "x": 3.0}));
    assert_ne!(state_hash(a.world()), state_hash(b.world()));
}

#[test]
fn a_recorded_session_plays_back_identically() {
    let mut original = app();
    let h = hero(&mut original);
    play_session(&mut original, h);
    let recording: Replay = serde_json::from_str(&serde_json::to_string(&replay::save(original.world())).unwrap()).unwrap();
    assert!(recording.checks.len() >= 3, "a check every second: {}", recording.checks.len());

    let mut copy = app();
    let hc = hero(&mut copy);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().expect("playback finished");
    assert!(report.identical(), "{report}");
    assert_eq!(feet(&original, h), feet(&copy, hc));
    assert_eq!(state_hash(original.world()), state_hash(copy.world()));
}

#[test]
fn a_recording_against_another_version_of_the_file_diverges_at_its_first_check() {
    let mut original = app();
    let h = hero(&mut original);
    play_session(&mut original, h);
    let recording = replay::save(original.world());

    // The artist moved a pillar since: the same inputs meet another file.
    let mut level = arena();
    level.node_mut("Pillar.002").translation = Some([-7.0, 2.0, 6.0]);
    let mut copy = app_in(&arena_assets("moved_pillar", &level));
    hero(&mut copy);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().unwrap();
    assert_eq!(report.diverged_at, Some(60), "{report}");
}
