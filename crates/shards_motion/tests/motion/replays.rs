//! A scripted run with the movement tools is deterministic, records as inputs and calls, and
//! plays back as the same game.

use ai_core::replay::{self, Replay, ReplayEvent};
use ai_core::AiReplay;
use bevy::prelude::*;
use serde_json::json;
use shards_motion::*;

use crate::common::*;
use crate::engine::*;
use crate::perception::wall_with_a_gap;

/// A wall with a gap, and a character at the start.
fn world(noise: bool) -> (App, Entity) {
    let mut app = bare_engine("replays");
    if noise {
        // What a window adds: unrelated entities that shift every `Entity` index after them.
        for _ in 0..17 {
            app.world_mut().spawn(Name::new("render noise"));
        }
    }
    wall_with_a_gap(app.world_mut());
    if noise {
        app.world_mut().spawn(Name::new("render noise"));
    }
    let e = player(app.world_mut(), Vec3::new(-5.0, 0.0, 0.0));
    // The physics world catches up with what was spawned (a frame, no tick).
    app.update();
    (app, e)
}

/// What an agent would do: sprint to a point behind the wall by a planned route, jump on
/// the way, walk a bit, steer by hand.
fn script(app: &mut App, e: Entity) {
    ok(app, "sprint", json!({}));
    ok(app, "move_to", json!({"target": {"x": 3, "z": -12}, "path": true}));
    step(app, 120);
    ok(app, "jump", json!({}));
    step(app, 60);
    ok(app, "sprint", json!({"on": false}));
    ok(app, "move", json!({"dir": "E", "meters": 8}));
    step(app, 150);
    // The keyboard: straight into `Intent`, between ticks.
    edit_intent(app, e, |i| {
        i.orders.clear();
        i.move_dir = Vec3::new(1.0, 0.0, 0.5).normalize();
    });
    step(app, 30);
    edit_intent(app, e, |i| i.move_dir = Vec3::ZERO);
    step(app, 40);
}

/// Bit-exact: where the character is, and how.
fn fingerprint(app: &App, e: Entity) -> (Vec<u32>, String) {
    let tf = app.world().get::<Transform>(e).unwrap();
    let bits = [tf.translation.x, tf.translation.y, tf.translation.z, tf.rotation.x, tf.rotation.y, tf.rotation.z, tf.rotation.w]
        .map(f32::to_bits)
        .to_vec();
    let mut motion = state(app, e);
    motion.volumes.clear();
    (bits, format!("{motion:?}"))
}

#[test]
fn the_same_script_gives_the_same_game_whatever_the_entity_indices() {
    let (mut a, ea) = world(false);
    script(&mut a, ea);
    let (mut b, eb) = world(false);
    script(&mut b, eb);
    assert_eq!(fingerprint(&a, ea), fingerprint(&b, eb), "two runs differ");
    let (mut shifted, es) = world(true);
    script(&mut shifted, es);
    assert_eq!(fingerprint(&a, ea), fingerprint(&shifted, es), "Entity indices changed the run");
    assert!(feet(&a, ea).x > -5.0, "the script went somewhere: {}", feet(&a, ea));
}

#[test]
fn the_recording_plays_back_identical() {
    let (mut original, e) = world(false);
    script(&mut original, e);
    let replay = replay::save(original.world());
    let replay: Replay = serde_json::from_str(&serde_json::to_string(&replay).unwrap()).unwrap();

    let tools: Vec<&str> =
        replay.events.iter().filter_map(|ev| if let ReplayEvent::Call { tool, .. } = ev { Some(tool.as_str()) } else { None }).collect();
    assert_eq!(tools, ["sprint", "move_to", "jump", "sprint", "move"], "the tools are recorded, time control is not");
    assert!(
        replay.events.iter().any(|ev| matches!(ev, ReplayEvent::Input { component, .. } if component == "Intent")),
        "the hand-steering reached the replay as Intent"
    );

    let (mut copy, ec) = world(false);
    replay::play(copy.world_mut(), replay.clone()).unwrap();
    step(&mut copy, replay.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().expect("playback has finished");
    assert!(report.identical(), "{report}");
    assert!(report.matched as usize >= replay.checks.len(), "{report}");
    assert_eq!(fingerprint(&original, e), fingerprint(&copy, ec));
}

#[test]
fn a_different_world_diverges_and_the_report_says_when() {
    let (mut original, e) = world(false);
    script(&mut original, e);
    let replay = replay::save(original.world());

    // The same recording on a game whose characters walk slower.
    let (mut other, _) = world(false);
    other.world_mut().entity_mut(e).insert(CharacterParams { run_speed: 3.0, sprint_speed: 4.0, ..default() });
    replay::play(other.world_mut(), replay.clone()).unwrap();
    step(&mut other, replay.end.tick);
    other.update();
    let report = other.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(!report.identical() && report.diverged_at.is_some(), "{report}");
}
