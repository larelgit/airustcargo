//! A client's copy stays the host's: through rebuilt worlds, new bodies, resyncs, values
//! JSON cannot carry, references and a host that keeps sending.

use shards::ai_core::journal::{self, JournalKind};
use shards::ai_core::replay::ReplayEvent;
use shards::ai_core::{sim, snapshot};
use shards::net::{LinkConditions, NetClient, NetDef, NetDefaults, NetHost, NetLeft};
use shards::npc::Home;
use shards::prelude::*;
use shards::serde_json::json;

use super::net::*;
use crate::common::*;

/// The `eN` a `spawn_prefab` answer names.
fn spawned(out: &shards::ai_core::CallOutcome) -> String {
    assert!(out.ok, "{}", out.result);
    text(out).split_whitespace().find(|w| w.starts_with('e')).unwrap().trim_end_matches([',', ':']).to_owned()
}

/// The ticks at which the host recorded an input of `id` that mentions `what`.
fn inputs_of(stand: &shards::net::Stand, id: &str, what: &str) -> Vec<u64> {
    shards::ai_core::replay::save(stand.world(HOST))
        .events
        .iter()
        .filter_map(|e| match e {
            ReplayEvent::Input { at, id: who, value, .. } if who.to_string() == id && value.to_string().contains(what) => Some(*at),
            _ => None,
        })
        .collect()
}

fn peer_of(stand: &shards::net::Stand, player: u8) -> &shards::net::host::Peer {
    stand.world(HOST).resource::<NetHost>().peers.values().find(|p| p.player == player).expect("a peer of that player")
}

fn rebases(stand: &shards::net::Stand, member: usize) -> u32 {
    stand.world(member).resource::<NetClient>().rebases
}

// --- values and references -----------------------------------------------------------------

#[test]
fn a_value_json_cannot_carry_stays_on_the_host_and_is_said_once() {
    let mut stand = stand(1);
    stand.run(10);
    // An NPC that never leashes: an infinite leash (JSON has no infinity).
    let boss = entity(&stand, HOST, BOSS).unwrap();
    stand.world_mut(HOST).get_mut::<Home>(boss).unwrap().leash = f32::INFINITY;
    stand.run(200);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    assert_eq!(rebases(&stand, C1), 0);
    let said: Vec<String> = journal(&stand, HOST).into_iter().filter(|l| l.contains("is not sent to clients")).collect();
    assert_eq!(said.len(), 1, "{said:#?}");
    assert!(said[0].contains(&format!("net: Home of {BOSS} is not sent to clients (does not read back")), "{}", said[0]);
    let theirs = entity(&stand, C1, BOSS).unwrap();
    assert!(stand.world(C1).get::<Home>(theirs).is_none(), "the client goes without it, as a snapshot does");
}

/// A gameplay component that names another entity.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct Grudge(Entity);

/// A gameplay component that names many entities.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Roster(Vec<Entity>);

fn with_references(app: &mut App) {
    app.add_plugins(sandbox::SandboxPlugin);
    app.register_type::<Grudge>().register_type::<Roster>();
}

#[test]
fn a_reference_to_a_despawned_entity_is_sent_as_it_reads_now() {
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, with_references);
    let pillar = spawned(&stand.call(HOST, "spawn_prefab", json!({"id": "pillar", "x": 4.0, "z": 16.0})));
    let target = entity(&stand, HOST, &pillar).unwrap();
    let boss = entity(&stand, HOST, BOSS).unwrap();
    stand.world_mut(HOST).entity_mut(boss).insert(Grudge(target));
    stand.run(70);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    let theirs = entity(&stand, C1, &pillar).expect("the client has the pillar");
    assert_eq!(stand.world(C1).get::<Grudge>(entity(&stand, C1, BOSS).unwrap()).map(|g| g.0), Some(theirs));
    assert!(stand.call(HOST, "despawn", json!({"id": pillar})).ok);
    stand.run(200);
    assert_eq!(desyncs(&stand, C1), 0, "the boss's grudge names nothing on both: {:#?}", journal(&stand, C1));
    assert_eq!(rebases(&stand, C1), 0);
}

#[test]
fn every_entity_in_view_is_sent_when_its_references_reach_the_cap() {
    let setup = |app: &mut App| {
        with_references(app);
        // The capped references are out of the client's world: its hash differs, by design.
        app.insert_resource(NetDefaults(NetDef { resync_on_desync: false, ..Default::default() }));
    };
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, setup);
    let me = body(&stand, C1);
    // 70 pillars far out of view, all named by the boss (a roster of its adds).
    let far: Vec<Entity> = (0..70)
        .map(|i| {
            let id = spawned(&stand.call(HOST, "spawn_prefab", json!({"id": "pillar", "x": 300.0 + 3.0 * i as f32, "z": 300.0})));
            entity(&stand, HOST, &id).unwrap()
        })
        .collect();
    let boss = entity(&stand, HOST, BOSS).unwrap();
    stand.world_mut(HOST).entity_mut(boss).insert(Roster(far));
    stand.run(100);
    assert!(peer_of(&stand, 1).closure_capped > 0, "the cap was reached");
    for id in [PLAYER, BOSS, me.as_str()] {
        assert!(entity(&stand, C1, id).is_some(), "{id} is in c1's view and stays in its world");
    }
}

#[test]
fn the_encode_cache_forgets_despawned_entities() {
    let mut stand = stand(1);
    stand.run(130);
    let cache = |s: &shards::net::Stand| s.world(HOST).resource::<NetHost>().cache_len();
    let before = cache(&stand);
    let pillars: Vec<String> =
        (0..20).map(|i| spawned(&stand.call(HOST, "spawn_prefab", json!({"id": "pillar", "x": -10.0 + i as f32, "z": 18.0})))).collect();
    stand.run(10);
    let peak = cache(&stand);
    assert!(peak >= before + 20, "the pillars are encoded: {before} → {peak}");
    for id in &pillars {
        assert!(stand.call(HOST, "despawn", json!({"id": id})).ok);
    }
    stand.run(300);
    let after = cache(&stand);
    assert!(after + 20 <= peak && after <= before + 10, "{before} → {peak} → {after}");
}

// --- rebuilt worlds, new bodies ------------------------------------------------------------

#[test]
fn a_snapshot_from_before_a_player_joined_gives_them_a_body_again() {
    let mut stand = shards::testing::stand_host(1, &configure, &sandbox_setup);
    stand.run(20);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    let c1 = shards::testing::stand_client(&mut stand, "c1", lossy(), &configure, &sandbox_setup);
    run_until(&mut stand, 300, "c1 joins", |s| connected(s, c1));
    stand.run(30);
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.run(60);
    let joins = host_calls(&stand, "player_join");
    assert_eq!(joins.len(), 2, "the first join, then one in the restored world: {joins:?}");
    let now = shards::core::player_body(stand.world(HOST), 1).expect("a body for p1").to_string();
    assert_eq!(body(&stand, c1), now);
    let mine = entity(&stand, c1, &now).expect("the client has its body");
    assert!(stand.world(c1).get::<LocalPlayer>(mine).is_some());
    let dodged = stand.call(c1, "dodge", json!({"dir": "E"}));
    assert!(dodged.ok, "{}", dodged.result);
    let asked_at = sim::tick(stand.world(c1));
    stand.run(40);
    assert_eq!(inputs_of(&stand, &now, "dodge"), [asked_at], "its inputs drive that body");
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));
}

#[test]
fn a_snapshot_with_a_departed_players_body_keeps_it_waiting_for_them() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(20);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    assert!(stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"})).ok);
    stand.run(30);
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.run(30);
    let waiting = entity(&stand, HOST, &me).expect("the body is back");
    assert!(stand.world(HOST).get::<NetLeft>(waiting).is_some(), "it waits for its player again");
    let left = host_calls(&stand, "player_leave");
    assert!(left.iter().any(|(_, _, args)| args["reason"] == "not in the session"), "{left:?}");

    let again = shards::testing::stand_client(&mut stand, "c1", lossy(), &configure, &sandbox_setup);
    run_until(&mut stand, 300, "c1 rejoins", |s| connected(s, again));
    assert_eq!(body(&stand, again), me, "the same body");
}

#[test]
fn a_respawned_body_gets_its_players_inputs() {
    let mut stand = stand(1);
    let old = body(&stand, C1);
    stand.run(20);
    // The game respawns player 1 as a new entity; the corpse keeps its PlayerId.
    let new = spawned(&stand.call(HOST, "spawn_prefab", json!({"id": "player", "x": 4.0, "z": 14.0})));
    let entity_new = entity(&stand, HOST, &new).unwrap();
    stand.world_mut(HOST).entity_mut(entity_new).insert(PlayerId(1)).remove::<(AiPlayer, LocalPlayer)>();
    stand.run(40);
    assert_ne!(new, old);
    assert_eq!(peer_of(&stand, 1).body.map(|b| format!("e{b}")), Some(new.clone()));
    assert_eq!(body(&stand, C1), new, "{:#?}", journal(&stand, C1));
    assert!(has_line(&stand, C1, &format!("net: the host's world again (a new body {new})")), "{:#?}", journal(&stand, C1));
    let dodged = stand.call(C1, "dodge", json!({"dir": "E"}));
    assert!(dodged.ok, "{}", dodged.result);
    let asked_at = sim::tick(stand.world(C1));
    stand.run(40);
    assert_eq!(inputs_of(&stand, &new, "dodge"), [asked_at]);
    assert!(inputs_of(&stand, &old, "dodge").is_empty());
}

// --- inputs across resyncs, orders ---------------------------------------------------------

#[test]
fn a_resync_keeps_the_inputs_on_their_way() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(40);
    assert!(stand.call(C1, "dodge", json!({"dir": "E"})).ok);
    let asked_at = sim::tick(stand.world(C1));
    assert!(stand.call(HOST, "net_resync", json!({})).ok);
    stand.run(60);
    assert!(has_line(&stand, C1, "net: the host's world again (asked)"), "{:#?}", journal(&stand, C1));
    assert_eq!(inputs_of(&stand, &me, "dodge"), [asked_at], "the dodge sent before the resync landed");
    assert_eq!(peer_of(&stand, 1).inputs.late, 0);
    stand.run(60);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn a_world_the_link_cannot_take_now_goes_later_and_the_epochs_stay_together() {
    let mut stand = stand(1);
    stand.run(20);
    // More gameplay lines in one tick than a link holds unacknowledged, then a resync.
    for i in 0..1100 {
        journal::emit(stand.world_mut(HOST), JournalKind::Event, format!("filler {i}"));
    }
    assert!(stand.call(HOST, "net_resync", json!({})).ok);
    stand.run(1);
    assert!(has_line(&stand, HOST, "net: p1 cannot be sent the world now"), "{:#?}", journal(&stand, HOST));
    assert_eq!(peer_of(&stand, 1).resyncs, 0, "not sent: the epoch stays");
    run_until(&mut stand, 600, "the world reaches c1", |s| has_line(s, C1, "net: the host's world again (asked)"));
    assert_eq!(rebases(&stand, C1), peer_of(&stand, 1).resyncs);
    stand.run(60);
    let host_tick = sim::tick(stand.world(HOST));
    let newest = stand.world(C1).resource::<NetClient>().newest_host_tick;
    assert!(newest + 10 >= host_tick, "states arrive again: newest {newest}, host {host_tick}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn a_stop_on_a_client_stops_its_body_on_the_host() {
    let mut stand = stand(1);
    let me = body(&stand, C1);
    stand.run(10);
    let start = position(&stand, HOST, &me);
    assert!(stand.call(C1, "move_to", json!({"target": {"x": -15.0, "z": 16.0}})).ok);
    stand.run(30);
    assert!(position(&stand, HOST, &me).distance(start) > 1.0, "it walks");
    assert!(stand.call(C1, "stop", json!({})).ok);
    stand.run(20);
    let there = entity(&stand, HOST, &me).unwrap();
    assert_eq!(stand.world(HOST).get::<Intent>(there).unwrap().move_dir, Vec3::ZERO, "stopped on the host");
    let stopped = position(&stand, HOST, &me);
    stand.run(30);
    assert!(position(&stand, HOST, &me).distance(stopped) < 0.05, "it stands: {stopped} → {}", position(&stand, HOST, &me));
}

#[test]
fn a_clients_order_stays_until_the_host_has_applied_it() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(40);
    assert!(stand.call(C1, "move_to", json!({"target": {"x": -15.0, "z": 16.0}})).ok);
    for _ in 0..60 {
        stand.run(1);
        let mine = entity(&stand, C1, &me).unwrap();
        let orders = stand.world(C1).get::<Intent>(mine).unwrap().orders.len();
        assert_eq!(orders, 1, "the client's queue at its tick {}: an older state of the host must not take it back", sim::tick(stand.world(C1)));
    }
    assert!(stand.call(C1, "stop", json!({})).ok);
    stand.run(40);
    let there = entity(&stand, HOST, &me).unwrap();
    assert!(stand.world(HOST).get::<Intent>(there).unwrap().orders.is_empty(), "the stop reached the host");
}
