//! A session's life: joins and leaves as conditions, a full session, the end of a session
//! (once, with every character saved), and the stand's clock and shutdown.

use shards::ai_core::sim;
use shards::net::{LinkConditions, NetDef, NetDefaults, NetHooks, NetLeft, NetState};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use super::net::*;
use crate::common::*;

/// The game's world saves (`on_session_end`).
#[derive(Resource, Default)]
struct WorldSaves(u32);

/// The characters a client got back from its host (`on_character_save`).
#[derive(Resource, Default)]
struct CharacterSaves(u32);

fn character(_: &World, _: Entity) -> Option<Value> {
    Some(json!({"level": 3}))
}

fn keep_character(world: &mut World, sections: &Value) {
    if sections["level"] == 3 {
        world.get_resource_or_init::<CharacterSaves>().0 += 1;
    }
}

fn save_world(world: &mut World) {
    world.get_resource_or_init::<WorldSaves>().0 += 1;
}

/// The sandbox with co-op saves: characters go to their players, the host saves the world.
fn with_saves(app: &mut App) {
    app.add_plugins(sandbox::SandboxPlugin);
    let mut hooks = app.world_mut().resource_mut::<NetHooks>();
    hooks.capture_character = character;
    hooks.on_character_save = keep_character;
    hooks.on_session_end = save_world;
}

fn saves<R: Resource>(stand: &shards::net::Stand, member: usize, count: fn(&R) -> u32) -> u32 {
    stand.world(member).get_resource::<R>().map_or(0, count)
}

// --- the end -------------------------------------------------------------------------------

#[test]
fn a_host_that_quits_ends_the_session_after_its_clients_got_their_characters() {
    // 150 ms away, 2 % lost: the end must still arrive.
    let mut stand = shards::testing::stand(2, lossy(), configure, with_saves);
    stand.run(30);
    let exit = stand.shut_down(5000);
    assert_eq!(exit, AppExit::Success);
    assert_eq!(saves::<WorldSaves>(&stand, HOST, |s| s.0), 1, "the world saved once");
    for member in [C1, C2] {
        assert!(has_line(&stand, member, "net: session ended by host (the host quit)"), "{:#?}", journal(&stand, member));
        assert!(matches!(net_state(&stand, member), NetState::Ended(_)), "{:?}", net_state(&stand, member));
        assert_eq!(saves::<CharacterSaves>(&stand, member, |s| s.0), 1, "member {member} got its character");
    }
}

#[test]
fn net_end_ends_the_session_once() {
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, with_saves);
    stand.run(10);
    let ended = stand.call(HOST, "net_end", json!({"reason": "the raid is over"}));
    assert!(ended.ok, "{}", ended.result);
    stand.run(10);
    let again = stand.call(HOST, "net_end", json!({}));
    assert!(!again.ok, "{}", again.result);
    assert!(text(&again).contains("the session has ended already (the raid is over)"), "{}", text(&again));
    // The host quits later: its exit hooks find the session over.
    shards::core::run_exit_hooks(stand.world_mut(HOST));
    assert_eq!(saves::<WorldSaves>(&stand, HOST, |s| s.0), 1, "the game's hook ran once");
    let ends = journal(&stand, HOST).into_iter().filter(|l| l.contains("net: session ended (")).count();
    assert_eq!(ends, 1, "{:#?}", journal(&stand, HOST));
    assert!(matches!(net_state(&stand, HOST), NetState::Ended(why) if why == "the raid is over"), "{:?}", net_state(&stand, HOST));
    assert!(has_line(&stand, C1, "net: session ended by host (the raid is over)"), "{:#?}", journal(&stand, C1));
    assert_eq!(saves::<CharacterSaves>(&stand, C1, |s| s.0), 1);
}

// --- joins and leaves ----------------------------------------------------------------------

#[test]
fn joins_and_leaves_wake_the_hosts_agent() {
    let mut stand = shards::testing::stand_host(1, &configure, &sandbox_setup);
    stand.run(5);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &sandbox_setup);
    let joined = stand.call(HOST, "step", json!({"ticks": 600, "until": [{"named": "net_joined"}]}));
    assert!(text(&joined).contains("stopped: condition net_joined"), "{}", text(&joined));
    assert!(connected(&stand, c1));
    let me = body(&stand, c1);
    stand.run(10);
    // The player quits (their game's exit hooks): the host hears it at once.
    shards::core::run_exit_hooks(stand.world_mut(c1));
    let left = stand.call(HOST, "step", json!({"ticks": 120, "until": [{"named": "net_left"}]}));
    assert!(text(&left).contains("stopped: condition net_left"), "{}", text(&left));
    assert!(has_line(&stand, HOST, "net: p1 left (quit)"), "{:#?}", journal(&stand, HOST));
    let waiting = entity(&stand, HOST, &me).expect("the body waits for a rejoin");
    assert!(stand.world(HOST).get::<NetLeft>(waiting).is_some());
}

#[test]
fn a_client_sent_away_is_disconnected_for_its_agent() {
    let mut stand = stand(1);
    stand.run(10);
    assert!(stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"})).ok);
    run_until(&mut stand, 60, "c1 is sent away", |s| matches!(net_state(s, C1), NetState::Ended(_)));
    let out = stand.call(C1, "step", json!({"ticks": 30, "until": [{"named": "net_disconnected"}]}));
    assert!(text(&out).contains("stopped: condition net_disconnected"), "{}", text(&out));
}

#[test]
fn a_full_session_refuses_the_next_player() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.insert_resource(NetDefaults(NetDef { max_players: 2, ..Default::default() }));
    };
    let stand = shards::testing::stand(2, LinkConditions::NONE, configure, setup);
    assert!(connected(&stand, C1));
    assert!(
        matches!(net_state(&stand, C2), NetState::Ended(why) if why.contains("the session is full (2 players)")),
        "{:?}",
        net_state(&stand, C2)
    );
}

#[test]
fn net_sim_changes_a_members_links() {
    let mut stand = stand(1);
    let set = stand.call(C1, "net_sim", json!({"conditions": "latency=150,loss=0.02"}));
    assert!(set.ok, "{}", set.result);
    let status = stand.call(C1, "net_status", json!({}));
    let conditions = status.result["conditions"].as_str().unwrap_or_default().to_owned();
    assert!(conditions.contains("150"), "{}", status.result);
    stand.run(180);
    let rtt = stand.world(HOST).resource::<shards::net::NetHost>().peers.values().next().unwrap().link.stats().srtt_ms;
    assert!(rtt > 250.0, "150 ms each way: rtt {rtt}");
    assert!(stand.call(C1, "net_sim", json!({"conditions": "none"})).ok);
    let typo = stand.call(C1, "net_sim", json!({"conditions": "latency=15O"}));
    assert!(!typo.ok, "{}", typo.result);
    assert!(connected(&stand, C1));
}

// --- the stand -----------------------------------------------------------------------------

#[test]
fn a_stands_members_say_how_the_stand_runs_them() {
    let mut stand = stand(1);
    let paused = |stand: &shards::net::Stand, m: usize| shards::ai_core::status::status(stand.world(m))["paused"].as_bool();
    assert_eq!((paused(&stand, HOST), paused(&stand, C1)), (Some(true), Some(true)));
    assert!(stand.call(HOST, "resume", json!({})).ok);
    assert!(stand.running);
    assert_eq!((paused(&stand, HOST), paused(&stand, C1)), (Some(false), Some(false)), "running, as the stand runs");
    // A step pauses, as it does in a game of its own.
    assert!(stand.call(C1, "step", json!({"ticks": 2})).ok);
    assert!(!stand.running);
    assert_eq!((paused(&stand, HOST), paused(&stand, C1)), (Some(true), Some(true)));
    // No negative speed.
    assert!(stand.call(HOST, "speed", json!({"x": -2.0})).ok);
    assert_eq!(stand.speed, 0.0);
    assert_eq!((sim::speed(stand.world(HOST)), sim::speed(stand.world(C1))), (0.0, 0.0));
}

#[test]
fn only_the_host_ends_a_stand() {
    let exit_with_host = |config: &mut shards::EngineConfig| {
        config.seed = 42;
        config.exit_with_host = true;
    };
    let mut stand = shards::testing::stand(2, LinkConditions::NONE, exit_with_host, sandbox_setup);
    stand.run(10);
    assert!(stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"})).ok);
    run_until(&mut stand, 60, "c1 is sent away", |s| matches!(net_state(s, C1), NetState::Ended(_)));
    assert!(stand.members[C1].app.should_exit().is_some(), "c1 exits with its session");
    assert!(!stand.serve(), "the stand goes on without it");
    stand.run(30);
    assert!(connected(&stand, C2));
}
