//! What every sandbox test needs: calls as the `dev` agent, the journal, the state of
//! every AI-visible entity.

#![allow(dead_code)]

use sandbox::gameplay::Player;
use shards::ai_core::{call_tool, AiJournal, CallOutcome, JournalKind};
use shards::prelude::*;
use shards::serde_json::{json, Value};

pub const PLAYER: &str = "e5";
pub const BOSS: &str = "e6";

/// Calls `tool` as the `dev` agent (dev scope, the player's body).
pub fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

/// [`act`] that must succeed (and whose step no gameplay failure stopped).
pub fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    shards::testing::ok(app, tool, args)
}

/// [`act`] that must fail; returns the error.
pub fn fails(app: &mut App, tool: &str, args: Value) -> String {
    let out = act(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, answered: {}", out.result);
    text(&out).to_owned()
}

pub fn text(out: &CallOutcome) -> &str {
    out.result.as_str().unwrap_or_default()
}

/// Runs `ticks` ticks.
pub fn step(app: &mut App, ticks: u64) -> CallOutcome {
    ok(app, "step", json!({"ticks": ticks}))
}

pub fn player<C: Component>(app: &mut App) -> &C {
    let world = app.world_mut();
    let mut q = world.query_filtered::<&C, With<Player>>();
    q.single(world).unwrap()
}

pub fn get<C: Component + Clone>(app: &App, id: &str) -> C {
    let id = AiId::parse(id).unwrap();
    let entity = app.world().resource::<shards::ai_core::AiIndex>().get(id).unwrap();
    app.world().get::<C>(entity).unwrap().clone()
}

/// The running action of an entity, or `idle`.
pub fn action(app: &App, id: &str) -> String {
    get::<ActionState>(app, id).current.map_or_else(|| "idle".to_owned(), |r| r.action)
}

/// Gameplay journal lines (`tick text`).
pub fn events(app: &App) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == JournalKind::Event).map(ToString::to_string).collect()
}

/// Gameplay journal lines containing `needle`.
pub fn lines(app: &App, needle: &str) -> Vec<String> {
    events(app).into_iter().filter(|l| l.contains(needle)).collect()
}

/// The journal without the agents' own lines: they only echo the calls.
pub fn story(app: &App) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind != JournalKind::Agent).map(ToString::to_string).collect()
}

/// Every AI-visible entity, bit-exact: position, rotation, health, action, motion.
pub fn state(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &Transform, Option<&Health>, Option<&ActionState>, Option<&MotionState>)>();
    let mut out: Vec<String> = q
        .iter(world)
        .map(|(id, tf, hp, action, motion)| {
            format!("{id} {:?} {:?} {hp:?} {action:?} {motion:?}", tf.translation, tf.rotation)
        })
        .collect();
    out.sort();
    out
}

/// The skeletons stand still: a test fights only what it means to.
pub fn calm_skeletons(app: &mut App) {
    for id in ["e7", "e8"] {
        ok(app, "set", json!({"id": id, "component": "SkeletonBrain", "path": "aggro", "value": 0}));
    }
}

/// The warden stays at its post and never attacks by itself (its leash reaches nobody).
pub fn calm_warden(app: &mut App) {
    ok(app, "set", json!({"id": BOSS, "component": "WardenBrain", "path": "leash", "value": 0}));
}
