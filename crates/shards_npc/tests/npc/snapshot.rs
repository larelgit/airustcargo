//! Snapshots with NPCs on the field: behaviour states, attack tokens, plans, routes, what
//! they perceive and remember come back, so a fight loaded from a snapshot goes on exactly
//! like the one it was taken in (the proof in `crates/sandbox/tests/sandbox/snapshots.rs`,
//! here for the NPC module's state).

use ai_core::snapshot::{self, Snapshot};
use ai_core::{sim, AiId, AiIndex, AiJournal, JournalKind};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_core::Intent;

use crate::common::*;
use crate::replay::arena;

/// What the hero does, and when (absolute ticks).
enum Do {
    Tool(&'static str, Value),
    Walk(Vec3),
}

fn script() -> Vec<(u64, Do)> {
    vec![
        (60, Do::Tool("light_attack", json!({}))),
        (150, Do::Tool("move", json!({"dir": "E", "meters": 3}))),
        (270, Do::Tool("dodge", json!({"dir": "N"}))),
        (330, Do::Walk(Vec3::new(-0.5, 0.0, 0.0))),
        (420, Do::Walk(Vec3::ZERO)),
        (420, Do::Tool("light_attack", json!({}))),
    ]
}

fn tick(app: &App) -> u64 {
    sim::tick(app.world())
}

/// Plays the script from the current tick to `until`; what is due at `until` is for the next call.
fn drive(app: &mut App, script: &[(u64, Do)], until: u64) {
    loop {
        let now = tick(app);
        if now >= until {
            break;
        }
        for (_, what) in script.iter().filter(|(at, _)| *at == now) {
            match what {
                Do::Tool(tool, args) => {
                    call(app, tool, args.clone());
                }
                Do::Walk(dir) => {
                    let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
                    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = *dir;
                }
            }
        }
        let next = script.iter().map(|(at, _)| *at).filter(|at| *at > now).min().unwrap_or(until);
        step(app, next.min(until) - now);
    }
}

fn story_after(app: &App, n: u64) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal
        .after(0)
        .0
        .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

fn differences(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    if a.resources != b.resources || a.ids != b.ids || a.state_hash != b.state_hash {
        out.push("resources, ids or state hash".to_owned());
    }
    let ids = |s: &Snapshot| s.entities.iter().map(|e| e.id).collect::<Vec<_>>();
    if ids(a) != ids(b) {
        out.push(format!("entities {:?} vs {:?}", ids(a), ids(b)));
        return out;
    }
    for (x, y) in a.entities.iter().zip(&b.entities) {
        for path in x.components.keys().chain(y.components.keys()) {
            if x.components.get(path) != y.components.get(path) {
                out.push(format!("e{} {path}: {:?} vs {:?}", x.id, x.components.get(path), y.components.get(path)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[test]
fn an_npc_fight_loaded_at_any_tick_ends_the_same() {
    let mut original = app();
    arena(&mut original);
    let script = script();
    let end = 720;
    let checkpoints = [20, 60, 61, 100, 150, 200, 290, 345, 430, 600];
    let mut snapshots = Vec::new();
    for &at in &checkpoints {
        drive(&mut original, &script, at);
        snapshots.push((at, snapshot::capture(original.world()).unwrap()));
    }
    drive(&mut original, &script, end);
    let story = journal(&original);
    for needle in ["takes an attack token", "returns its attack token", "idle → fight"] {
        assert!(story.iter().any(|l| l.contains(needle)), "no `{needle}` in {story:#?}");
    }
    let last = snapshot::capture(original.world()).unwrap();

    for (at, snapshot) in snapshots {
        // A fresh arena: the floor, nothing else. The fighters are the snapshot's.
        let mut copy = app();
        let report = snapshot::restore(copy.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("tick {at}: {e}"));
        assert!(report.exact(), "tick {at}: {:?}", report.differences);
        drive(&mut copy, &script, end);
        let diff = differences(&last, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at}:\n{}", diff.join("\n"));
        assert_eq!(story_after(&copy, at), story_after(&original, at), "journal after tick {at}");
    }
}

#[test]
fn what_an_npc_holds_is_in_the_snapshot() {
    let mut original = app();
    arena(&mut original);
    drive(&mut original, &script(), 150);
    let snapshot = snapshot::capture(original.world()).unwrap();
    assert!(snapshot.left_out.is_empty(), "{:?}", snapshot.left_out);
    // e2..e4 are the grunts, e5 the warden.
    let grunt = &snapshot.entities.iter().find(|e| e.id == 2).unwrap().components;
    for component in ["Transform", "Npc", "NpcBrain", "NpcDrive", "NpcPlan", "NavAgent", "AttackToken", "Perceived", "Fighter", "ActionState"] {
        assert!(grunt.keys().any(|k| k.ends_with(&format!("::{component}"))), "{component} missing from {:?}", grunt.keys());
    }
}
