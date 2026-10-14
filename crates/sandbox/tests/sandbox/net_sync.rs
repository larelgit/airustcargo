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
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, with_references);
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
    stand.run(200);
    assert!(peer_of(&stand, 1).closure_capped > 0, "the cap was reached");
    for id in [PLAYER, BOSS, me.as_str()] {
        assert!(entity(&stand, C1, id).is_some(), "{id} is in c1's view and stays in its world");
    }
    // The capped references are not in c1's world, so its copy is not checked: no desync,
    // and no world sent again and again.
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    assert_eq!(rebases(&stand, C1), 0);
    let said = journal(&stand, HOST).into_iter().filter(|l| l.contains("p1's view names over 64 entities")).count();
    assert_eq!(said, 1, "said once: {:#?}", journal(&stand, HOST));
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
fn a_body_the_host_joined_itself_stays_through_a_rebuilt_world() {
    let mut stand = stand(1);
    // Co-op on the host's own machine: a player of its own, no peer.
    let joined = stand.call(HOST, "player_join", json!({"player": 2, "name": "couch"}));
    assert!(joined.ok, "{}", joined.result);
    stand.run(10);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.run(30);
    assert!(has_line(&stand, C1, "net: the host's world again"), "{:#?}", journal(&stand, C1));
    let couch = shards::core::player_body(stand.world(HOST), 2).expect("p2's body").to_string();
    let couch = entity(&stand, HOST, &couch).unwrap();
    assert!(stand.world(HOST).get::<NetLeft>(couch).is_none(), "no peer's body: it plays on");
    assert!(host_calls(&stand, "player_leave").is_empty(), "{:?}", host_calls(&stand, "player_leave"));
    assert_eq!(host_calls(&stand, "player_join").len(), 2, "c1's and the host's own; none again");
}

/// A sandbox whose departed players' bodies go after half a second.
fn short_grace(app: &mut App) {
    app.add_plugins(sandbox::SandboxPlugin);
    app.insert_resource(NetDefaults(NetDef { rejoin_grace_secs: 0.5, ..Default::default() }));
}

#[test]
fn a_number_a_guest_left_is_the_hosts_own_once_it_joins_a_player_there() {
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, short_grace);
    let me = body(&stand, C1);
    stand.run(10);
    assert!(stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"})).ok);
    run_until(&mut stand, 120, "c1's body goes", |s| entity(s, HOST, &me).is_none());
    // Co-op on the host's machine: the lowest number without a body, the one c1 held.
    let joined = stand.call(HOST, "player_join", json!({"name": "couch"}));
    assert!(joined.ok, "{}", joined.result);
    assert_eq!(joined.result["player"], 1, "{}", joined.result);
    stand.run(10);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.run(30);
    let couch = shards::core::player_body(stand.world(HOST), 1).expect("p1's body").to_string();
    let couch = entity(&stand, HOST, &couch).unwrap();
    assert!(stand.world(HOST).get::<NetLeft>(couch).is_none(), "the host's own player plays on");
    let left: Vec<_> = host_calls(&stand, "player_leave").into_iter().map(|(_, _, args)| args["reason"].clone()).collect();
    assert_eq!(left, [json!("kicked: testing")], "only c1's leave");
}

#[test]
fn guest_bodies_of_a_snapshot_loaded_into_a_new_host_leave_and_free_their_numbers() {
    let saved = {
        let mut stand = shards::testing::stand(2, LinkConditions::NONE, configure, short_grace);
        stand.run(20);
        snapshot::capture(stand.world(HOST)).unwrap()
    };
    // Another host process, no session yet: the snapshot's guests are nobody's here.
    let mut stand = shards::testing::stand_host(1, &configure, &short_grace);
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.run(10);
    let left: Vec<_> = host_calls(&stand, "player_leave").into_iter().map(|(_, _, args)| (args["player"].clone(), args["reason"].clone())).collect();
    assert_eq!(left, [(json!(1), json!("not in the session")), (json!(2), json!("not in the session"))]);
    assert!(shards::core::player_body(stand.world(HOST), 0).is_some(), "the host's own player stays");
    // Once their grace is over, a new guest gets a number.
    stand.run(40);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &short_grace);
    run_until(&mut stand, 300, "a new guest joins", |s| connected(s, c1));
    assert!(matches!(net_state(&stand, c1), shards::net::NetState::Connected { player: 1, .. }), "{:?}", net_state(&stand, c1));
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
fn a_resync_does_not_set_the_clients_clock_back() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(60);
    assert!(stand.call(HOST, "net_resync", json!({})).ok);
    // 150 ms away the client runs about twice that ahead of the host: the world it takes
    // again is from a host tick it passed long ago.
    let mut last = sim::tick(stand.world(C1));
    for _ in 0..60 {
        stand.run(1);
        let now = sim::tick(stand.world(C1));
        assert!(now >= last, "c1's clock went back from {last} to {now}");
        last = now;
        if has_line(&stand, C1, "net: the host's world again (asked)") {
            break;
        }
    }
    assert!(has_line(&stand, C1, "net: the host's world again (asked)"), "{:#?}", journal(&stand, C1));
    // An input right after it is on time on the host.
    assert!(stand.call(C1, "dodge", json!({"dir": "E"})).ok);
    let asked_at = sim::tick(stand.world(C1));
    stand.run(40);
    assert_eq!(inputs_of(&stand, &me, "dodge"), [asked_at]);
    assert_eq!(peer_of(&stand, 1).inputs.late, 0, "no input came late");
}

#[test]
fn a_key_held_through_a_region_change_still_moves_the_body_on_the_host() {
    let mut stand = stand(1);
    let me = body(&stand, C1);
    stand.run(10);
    // c1's player holds a direction (its keyboard writes the intent so).
    let mine = entity(&stand, C1, &me).unwrap();
    stand.world_mut(C1).get_mut::<Intent>(mine).unwrap().move_dir = Vec3::X;
    stand.run(10);
    let on_host = |stand: &shards::net::Stand| {
        let id = shards::core::player_body(stand.world(HOST), 1).unwrap().to_string();
        stand.world(HOST).get::<Intent>(entity(stand, HOST, &id).unwrap()).unwrap().move_dir
    };
    assert_eq!(on_host(&stand), Vec3::X);
    // The region change stops every player on the host; c1's still holds it.
    assert!(stand.call(HOST, "regen", json!({"region": "wilds", "seed": 42})).ok);
    run_until(&mut stand, 300, "c1 takes the new region", |s| has_line(s, C1, "net: the host's world again (region wilds)"));
    stand.run(20);
    assert_eq!(on_host(&stand), Vec3::X, "the held direction is sent again: {:#?}", journal(&stand, C1));
}

#[test]
fn a_desync_after_a_snapshot_of_an_earlier_tick_is_heard_and_resynced() {
    let mut stand = stand(1);
    stand.run(30);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    stand.run(60);
    let corrupt = |stand: &mut shards::net::Stand| {
        let boss = entity(stand, C1, BOSS).unwrap();
        stand.world_mut(C1).get_mut::<Health>(boss).unwrap().current = 1.0;
    };
    corrupt(&mut stand);
    let host_desyncs = |s: &shards::net::Stand| s.world(HOST).resource::<NetHost>().desyncs;
    run_until(&mut stand, 120, "the first desync", |s| host_desyncs(s) == 1);
    run_until(&mut stand, 60, "its resync", |s| rebases(s, C1) == 1);
    // Back to an earlier tick: the host's ticks start over below the report it took.
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    run_until(&mut stand, 60, "the snapshot reaches c1", |s| rebases(s, C1) == 2);
    // Its clock went back with the host's: the world of other ticks starts its inputs over.
    let (host_at, c1_at) = (sim::tick(stand.world(HOST)), sim::tick(stand.world(C1)));
    assert!(c1_at < host_at + 20, "c1 at {c1_at}, the host at {host_at}");
    // Until c1 acknowledges a state, every one it gets is whole and would mend the value.
    stand.run(30);
    corrupt(&mut stand);
    // Heard at the new timeline's tick 120, the tick of the old timeline's report: a report
    // that one shadowed would be heard only at the next hash, after this wait.
    run_until(&mut stand, 90, "the second desync, on the new timeline", |s| host_desyncs(s) == 2);
    run_until(&mut stand, 60, "its resync", |s| rebases(s, C1) == 3);
    let resyncs = journal(&stand, C1).iter().filter(|l| l.contains("net: the host's world again (desync)")).count();
    assert_eq!(resyncs, 2, "{:#?}", journal(&stand, C1));
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
    // The lines the full link would not take are counted, and said once a second.
    let host = journal(&stand, HOST);
    assert!(!host.iter().any(|l| l.contains("cannot send to peer")), "a line per refused message: {host:#?}");
    assert!(host.iter().any(|l| l.contains("net: p1: ") && l.contains(" messages not sent in the last second")), "{host:#?}");
}

#[test]
fn a_world_of_an_earlier_tick_that_waits_for_room_still_starts_the_inputs_over() {
    let mut stand = stand(1);
    stand.run(30);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    stand.run(300);
    // Back 300 ticks, with c1's link too full to take the world for a while: the host's new
    // ticks run on meanwhile, and still the world that goes is of other ticks than c1's.
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    for i in 0..1100 {
        journal::emit(stand.world_mut(HOST), JournalKind::Event, format!("filler {i}"));
    }
    stand.run(1);
    assert!(has_line(&stand, HOST, "net: p1 cannot be sent the world now"), "{:#?}", journal(&stand, HOST));
    assert_eq!(peer_of(&stand, 1).resyncs, 0, "not sent: it waits a tick at least");
    run_until(&mut stand, 600, "the snapshot reaches c1", |s| rebases(s, C1) == 1);
    let (host_at, c1_at) = (sim::tick(stand.world(HOST)), sim::tick(stand.world(C1)));
    assert!(c1_at < host_at + 20, "c1 kept its clock of the old ticks: c1 at {c1_at}, the host at {host_at}");
    stand.run(60);
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

// --- typed events across a rebuilt world ---------------------------------------------------

/// A typed event of these tests: whoever triggers it or has it triggered shows it.
#[derive(Event, Reflect, Clone)]
struct Ping {
    n: u32,
}

#[derive(Resource, Default)]
struct Pings(Vec<u32>);

fn ping_stand() -> shards::net::Stand {
    use shards::net::NetEventAppExt;
    shards::testing::stand(1, LinkConditions::NONE, |c| c.seed = 42, |app| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.net_event::<Ping>().init_resource::<Pings>();
        app.add_observer(|ping: On<Ping>, mut seen: ResMut<Pings>| seen.0.push(ping.event().n));
    })
}

fn pings(stand: &shards::net::Stand, member: usize) -> Vec<u32> {
    stand.world(member).resource::<Pings>().0.clone()
}

#[test]
fn events_of_the_world_a_snapshot_replaced_are_not_shown_and_do_not_hold_back_the_new_ones() {
    let mut stand = ping_stand();
    stand.run(50);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    stand.run(80);
    // An event of the host's tick 130, on its way when the host goes back to tick 50: it
    // reaches c1 just before the world of the earlier tick.
    stand.world_mut(HOST).trigger(Ping { n: 1 });
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    run_until(&mut stand, 60, "the snapshot reaches c1", |s| rebases(s, C1) == 1);
    assert!(sim::tick(stand.world(C1)) < 100, "c1 is on the new timeline");
    // The new world's own event, at about tick 52: it is shown soon, not when c1 reaches 130.
    stand.world_mut(HOST).trigger(Ping { n: 2 });
    stand.run(30);
    assert_eq!(pings(&stand, C1), [2], "the new world's event is shown at its tick");
    // The old world's event is not shown later, when c1's ticks pass 130.
    stand.run(100);
    assert!(sim::tick(stand.world(C1)) > 140);
    assert_eq!(pings(&stand, C1), [2], "the old world's event never is");
    assert_eq!(pings(&stand, HOST), [1, 2]);
}

// --- the host's ticks jump while a client joins --------------------------------------------

#[test]
fn a_client_admitted_in_the_frame_the_hosts_ticks_jump_keeps_its_inputs_through_a_resync() {
    use shards::net::ClientStage;
    let configure = |c: &mut shards::EngineConfig| c.seed = 42;
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
    };
    let mut stand = shards::testing::stand_host(1, &configure, &setup);
    stand.run(60);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    stand.run(60);
    assert_eq!(shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &setup), C1);
    // Frames, not ticks, until c1 has built its world and says it is ready: the host hears it in
    // its next frame, and goes back to tick 60 right before.
    let started = std::time::Instant::now();
    while stand.world(C1).resource::<NetClient>().stage != ClientStage::ReadySent {
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "c1 never got ready");
        stand.frame();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.frame();
    run_until(&mut stand, 120, "c1 is admitted", |s| connected(s, C1));
    stand.run(20);
    assert!(stand.call(C1, "dodge", json!({"dir": "E"})).ok);
    stand.run(20);
    let applied = peer_of(&stand, 1).applied_input_tick;
    assert!(applied > 0, "the host applied the dodge");

    // The world again, for a reason that does not move time: c1's ticks never moved, so the
    // inputs on their way and the host's record of the applied ones stay.
    assert!(stand.call(HOST, "net_resync", json!({})).ok);
    stand.run(10);
    assert_eq!(rebases(&stand, C1), 1);
    assert_eq!(peer_of(&stand, 1).applied_input_tick, applied, "the resync started c1's inputs over");
}

#[test]
fn a_view_that_stops_being_capped_gives_the_client_the_references_it_read_as_nothing() {
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, with_references);
    // 70 pillars far out of view, all named by the boss, which every client sees wherever it is.
    let far: Vec<String> = (0..70)
        .map(|i| spawned(&stand.call(HOST, "spawn_prefab", json!({"id": "pillar", "x": 300.0 + 3.0 * i as f32, "z": 300.0}))))
        .collect();
    let boss = entity(&stand, HOST, BOSS).unwrap();
    let roster: Vec<Entity> = far.iter().map(|id| entity(&stand, HOST, id).unwrap()).collect();
    stand.world_mut(HOST).entity_mut(boss).insert((Roster(roster), shards::net::NetAlways));
    stand.run(200);
    assert!(peer_of(&stand, 1).closure_capped > 0, "the cap was reached");
    let theirs = |stand: &shards::net::Stand| -> Vec<Entity> { stand.world(C1).get::<Roster>(entity(stand, C1, BOSS).unwrap()).unwrap().0.clone() };
    assert!(theirs(&stand).contains(&Entity::PLACEHOLDER), "what the cap kept out reads as nothing on c1");

    // Eight of the pillars move into c1's view: the other 62 are within the cap.
    for (i, id) in far.iter().take(8).enumerate() {
        assert!(stand.call(HOST, "teleport", json!({"id": id, "x": -10.0 + i as f32, "z": 18.0})).ok);
    }
    stand.run(200);
    let before = peer_of(&stand, 1).closure_capped;
    stand.run(60);
    assert_eq!(peer_of(&stand, 1).closure_capped, before, "no longer capped");
    assert!(has_line(&stand, C1, "net: the host's world again (view complete)"), "{:#?}", journal(&stand, C1));
    assert_eq!(rebases(&stand, C1), 1);
    assert!(!theirs(&stand).contains(&Entity::PLACEHOLDER), "every pillar of the roster is there now");
    // The copy is checked again, and it is the host's.
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    assert_eq!(rebases(&stand, C1), 1, "and no more worlds");
}

#[test]
fn a_region_change_a_guest_forwards_reaches_every_client_as_a_world_again() {
    let mut stand = stand_with(2, lossy());
    stand.world_mut(HOST).insert_resource(dev_guests());
    stand.run(20);
    // The call runs on the host while the session is out of the world (inside its tasks): the
    // rebuild it makes is not lost.
    let sent = stand.call(C1, "regen", json!({"region": "wilds", "seed": 42}));
    assert!(sent.ok, "{}", sent.result);
    stand.run(90);
    assert!(has_line(&stand, HOST, "host: regen") || host_calls(&stand, "regen").len() == 1, "{:#?}", journal(&stand, HOST));
    let hash = stand.world(HOST).resource::<shards::world::ActiveRegion>().hash;
    for member in [C1, C2] {
        assert!(has_line(&stand, member, "net: the host's world again (region wilds)"), "{:#?}", journal(&stand, member));
        assert_eq!(stand.world(member).resource::<shards::world::ActiveRegion>().hash, hash, "the same region");
        assert_eq!(rebases(&stand, member), 1);
        assert_eq!(desyncs(&stand, member), 0, "{:#?}", journal(&stand, member));
    }
}
