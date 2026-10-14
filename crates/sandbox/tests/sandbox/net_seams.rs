//! The seams networking builds on, on the sandbox: what a client runs per tick, and the
//! state codec that carries a host's world into another app.

use std::collections::BTreeSet;

use shards::ai_core::state::{apply_changes, encode_replicated, encode_resources, StateChanges};
use shards::ai_core::{replay, sim, AiVisible};
use shards::bevy::ecs::schedule::{ScheduleLabel, SystemSet};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;

/// `FixedUpdate` systems outside every `SimSet`: all a client runs of `FixedUpdate`
/// besides `NetSet::Predict` (the client allowlist). Anything new here runs on clients,
/// which do not simulate: it must not change gameplay state. `sync_physics` runs twice
/// (`PhysicsSystems::Sync` and `SyncAfterMove`): proxies' colliders follow replicated
/// transforms.
const CLIENT_ALLOWLIST: &[&str] = &["shards_motion::physics::sync_physics", "shards_motion::physics::sync_physics"];

/// Names of the `FixedUpdate` systems of `app` that are in no `SimSet`, sorted; Bevy's own
/// sync points (`apply_deferred`) left out.
fn outside_sim_sets(app: &mut App) -> Vec<String> {
    app.world_mut().schedule_scope(FixedUpdate.intern(), |world, schedule| {
        schedule.initialize(world).unwrap();
        let graph = schedule.graph();
        let mut inside = BTreeSet::new();
        for set in [SimSet::Input, SimSet::Think, SimSet::Act, SimSet::Move, SimSet::Resolve, SimSet::Cleanup] {
            inside.extend(graph.systems_in_set(set.intern()).unwrap().iter().copied());
        }
        let mut names: Vec<String> = schedule
            .systems()
            .unwrap()
            .filter(|(key, _)| !inside.contains(key))
            .map(|(_, system)| system.name().to_string())
            .filter(|name| name != "bevy_ecs::apply_deferred")
            .collect();
        names.sort();
        names
    })
}

#[test]
fn a_client_runs_only_the_allowlisted_fixed_update_systems() {
    let mut app = sandbox::test_app(42);
    let outside = outside_sim_sets(&mut app);
    assert_eq!(outside, CLIENT_ALLOWLIST, "FixedUpdate systems outside SimSets (they run on clients):\n{outside:#?}");
}

#[test]
fn a_client_sandbox_simulates_nothing() {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    app.insert_resource(NetRole::Client);
    let before = replay::state_hash(app.world());
    let story = events(&app);
    let ran = step(&mut app, 120);
    assert_eq!(sim::tick(app.world()), 120);
    assert!(ran.changes.is_empty(), "{:?}", ran.changes);
    assert_eq!(events(&app), story, "no gameplay event");
    assert_ne!(before, replay::state_hash(app.world()), "only the tick moved");
    sim::set_tick(app.world_mut(), 0);
    assert_eq!(before, replay::state_hash(app.world()), "nothing else did");

    app.insert_resource(NetRole::Single);
    let played = step(&mut app, 60);
    assert!(!played.changes.is_empty(), "single player simulates again");
}

/// Everything a host would send a client that has nothing: every AI-visible entity's
/// replicated components, the ids `to` has and `from` does not, the replicated resources.
fn full_state(from: &mut App, to: &mut App) -> StateChanges {
    let ids = |app: &mut App| -> Vec<(u32, Entity)> {
        let world = app.world_mut();
        let mut q = world.query_filtered::<(Entity, &AiId), With<AiVisible>>();
        let mut ids: Vec<(u32, Entity)> = q.iter(world).map(|(e, id)| (id.0, e)).collect();
        ids.sort();
        ids
    };
    let here = ids(from);
    let there = ids(to);
    let live: BTreeSet<u32> = here.iter().map(|(id, _)| *id).collect();
    StateChanges {
        set: here.iter().map(|(id, e)| (*id, encode_replicated(from.world(), *e).unwrap())).collect(),
        despawn: there.iter().filter(|(id, _)| !live.contains(id)).map(|(id, _)| (*id, "out of view".to_owned())).collect(),
        resources: encode_resources(from.world()),
        ..Default::default()
    }
}

#[test]
fn the_state_codec_carries_a_played_world_into_another_app() {
    let mut host = sandbox::test_app(42);
    ok(&mut host, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut host, 150);
    ok(&mut host, "despawn", json!({"id": "e7"}));
    step(&mut host, 30);

    let mut client = sandbox::test_app(42);
    let changes = full_state(&mut host, &mut client);
    assert!(changes.despawn.iter().any(|(id, _)| *id == 7), "{:?}", changes.despawn);
    sim::set_tick(client.world_mut(), sim::tick(host.world()));
    let warnings = apply_changes(client.world_mut(), &changes);
    assert!(warnings.is_empty(), "{warnings:?}");

    let again = full_state(&mut client, &mut host);
    assert_eq!(again.set, changes.set, "every entity encodes the same");
    assert!(again.despawn.is_empty());
    assert_eq!(again.resources, changes.resources);
    assert_eq!(replay::state_hash(client.world()), replay::state_hash(host.world()));
    let gone = act(&mut client, "inspect", json!({"id": "e7"}));
    assert!(text(&gone).contains("e7 is out of view since tick 180"), "{}", text(&gone));
}
