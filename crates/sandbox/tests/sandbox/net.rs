//! Co-op over the network: a host and its clients as apps of one process (`testing::Stand`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use shards::ai_core::state::{StateEncoder, StateRule};
use shards::ai_core::replay::ReplayEvent;
use shards::ai_core::journal::JournalKind;
use shards::ai_core::{sim, snapshot, AiIndex, AiJournal};
use shards::net::{LinkConditions, NetClient, NetDesync, NetHost, NetState, Stand};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::*;

/// Bodies for players 1 and 2 beside player 0, both attacking the boss.
fn three_players(app: &mut App) {
    for (n, x) in [(1u8, 2.0f32), (2, -2.0)] {
        let out = ok(app, "spawn_prefab", json!({"id": "player", "x": x, "z": 6.0}));
        let id = text(&out).split_whitespace().find(|w| w.starts_with('e')).unwrap().trim_end_matches([',', ':']).to_owned();
        let entity = app.world().resource::<AiIndex>().get(AiId::parse(&id).unwrap()).unwrap();
        app.world_mut().entity_mut(entity).insert(PlayerId(n)).remove::<(AiPlayer, LocalPlayer)>();
    }
}

fn attack_boss(app: &mut App) {
    let boss = AiId::parse(BOSS).unwrap();
    let world = app.world_mut();
    let mut q = world.query_filtered::<&mut Intent, With<PlayerId>>();
    for mut intent in q.iter_mut(world) {
        intent.orders.clear();
        intent.orders.push_back(Order::Attack { target: boss, input: "light".into(), repeat: true });
    }
}

/// What a send tick costs the host: every AI-visible entity encoded, and the bytes of what
/// changed since the last send (a client that sees everything).
#[derive(Default)]
struct Cost {
    sends: u32,
    encode: Duration,
    worst: Duration,
    bytes: usize,
    entities: usize,
    by_component: BTreeMap<String, usize>,
}

fn measure(app: &mut App, ticks: u64, send_every: u64, cost: &mut Cost, last: &mut BTreeMap<u32, BTreeMap<String, Value>>) {
    for _ in 0..ticks {
        step(app, 1);
        if shards::ai_core::sim::tick(app.world()) % send_every != 0 {
            continue;
        }
        let world = app.world_mut();
        let mut visible = world.query_filtered::<(Entity, &AiId), With<AiVisible>>();
        let mut ids: Vec<(u32, Entity)> = visible.iter(world).map(|(e, id)| (id.0, e)).collect();
        ids.sort();
        let started = Instant::now();
        let encoder = StateEncoder::new(world, StateRule::Replicated);
        let now: BTreeMap<u32, BTreeMap<String, Value>> = ids.iter().map(|(id, e)| (*id, encoder.entity(*e).unwrap().0)).collect();
        let took = started.elapsed();
        drop(encoder);
        let mut delta = Vec::new();
        for (id, values) in &now {
            let before = last.get(id);
            let set: BTreeMap<&String, &Value> = values.iter().filter(|(p, v)| before.and_then(|b| b.get(*p)) != Some(*v)).collect();
            for (path, value) in &set {
                *cost.by_component.entry((*path).clone()).or_default() += shards::serde_json::to_vec(value).unwrap().len();
            }
            if !set.is_empty() {
                delta.push(json!({"id": id, "set": set}));
            }
        }
        cost.bytes += shards::serde_json::to_vec(&delta).unwrap().len();
        cost.sends += 1;
        cost.encode += took;
        cost.worst = cost.worst.max(took);
        cost.entities = now.len();
        *last = now;
    }
}

/// The spike behind the replication design: encode time per send tick and bytes per client.
/// `cargo test -p sandbox --test sandbox spike_replication_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn spike_replication_cost() {
    for (place, region) in [("arena", None), ("wilds", Some(42u64))] {
        let mut app = sandbox::test_app(42);
        if let Some(seed) = region {
            ok(&mut app, "regen", json!({"region": "wilds", "seed": seed}));
            shards::testing::wait_until_loaded(&mut app, Duration::from_secs(60)).unwrap();
        }
        three_players(&mut app);
        if region.is_none() {
            attack_boss(&mut app);
        } else {
            ok(&mut app, "move_to", json!({"target": {"x": 40.0, "z": 40.0}}));
        }
        let welcome = shards::serde_json::to_vec(&snapshot::capture(app.world()).unwrap()).unwrap().len();
        let (mut cost, mut last) = (Cost::default(), BTreeMap::new());
        measure(&mut app, 1200, 2, &mut cost, &mut last);
        let secs = 1200.0 / 60.0;
        let mut top: Vec<(&String, &usize)> = cost.by_component.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = top.iter().take(8).map(|(p, b)| format!("{} {:.1} KB/s", p.rsplit("::").next().unwrap(), **b as f64 / secs / 1024.0)).collect();
        println!(
            "{place}: {} entities, encode mean {:.3} ms worst {:.3} ms per send tick, raw delta {:.1} KB/s per client, welcome {:.1} KB\n  top: {}",
            cost.entities,
            cost.encode.as_secs_f64() * 1000.0 / cost.sends as f64,
            cost.worst.as_secs_f64() * 1000.0,
            cost.bytes as f64 / secs / 1024.0,
            welcome as f64 / 1024.0,
            top.join(", ")
        );
    }
}

/// The same through a real session: host encode time per send tick and bytes per second per
/// client, as `net_status` reports them, in the arena fight (3 players on the boss) and in
/// the wilds. `cargo test -p sandbox --test sandbox spike_session_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn spike_session_cost() {
    for place in ["arena", "wilds"] {
        let mut stand = stand(2);
        if place == "wilds" {
            assert!(stand.call(HOST, "regen", json!({"region": "wilds", "seed": 42})).ok);
            stand.run(30);
        }
        for member in [HOST, C1, C2] {
            let order = if place == "arena" { ("attack", json!({"target": BOSS, "repeat": true})) } else { ("move_to", json!({"target": {"x": 40.0, "z": 40.0}})) };
            let out = stand.call(member, order.0, order.1);
            assert!(out.ok, "{}", out.result);
        }
        let out_before: Vec<u64> = stand.world(HOST).resource::<NetHost>().peers.values().map(|p| p.link.stats().bytes_out).collect();
        let started = Instant::now();
        stand.run(600);
        let wall = started.elapsed();
        let status = stand.call(HOST, "net_status", json!({}));
        let host = stand.world(HOST).resource::<NetHost>();
        let per_client: Vec<String> = host
            .peers
            .values()
            .zip(&out_before)
            .map(|(p, before)| format!("p{} {:.1} KB/s (deltas avg {} B)", p.player, (p.link.stats().bytes_out - before) as f64 / 10.0 / 1024.0, p.delta_bytes / p.deltas.max(1)))
            .collect();
        println!(
            "{place}: encode per send tick mean {} us, max {} us, cache {}; {}; 600 stand ticks in {:.1} s",
            status.result["encode_us"]["mean"],
            status.result["encode_us"]["max"],
            status.result["encode_cache"],
            per_client.join(", "),
            wall.as_secs_f64()
        );
    }
}

// --- the stand -----------------------------------------------------------------------------

const HOST: usize = 0;
const C1: usize = 1;
const C2: usize = 2;

/// A sandbox stand: the host (seed 42) and `clients` clients, joined at tick 0.
fn stand(clients: u8) -> Stand {
    stand_with(clients, LinkConditions::NONE)
}

fn stand_with(clients: u8, links: LinkConditions) -> Stand {
    shards::testing::stand(clients, links, |c| c.seed = 42, |app| {
        app.add_plugins(sandbox::SandboxPlugin);
    })
}

fn net_state(stand: &Stand, member: usize) -> NetState {
    stand.world(member).resource::<NetState>().clone()
}

/// The body of member `member`'s player, as an `eN`.
fn body(stand: &Stand, member: usize) -> String {
    match net_state(stand, member) {
        NetState::Connected { body: Some(b), .. } => format!("e{b}"),
        other => panic!("member {member} is not playing: {other:?}"),
    }
}

fn journal(stand: &Stand, member: usize) -> Vec<String> {
    let journal = stand.world(member).resource::<AiJournal>();
    journal.after(0).0.map(ToString::to_string).collect()
}

fn has_line(stand: &Stand, member: usize, needle: &str) -> bool {
    journal(stand, member).iter().any(|l| l.contains(needle))
}

fn position(stand: &Stand, member: usize, id: &str) -> Vec3 {
    let world = stand.world(member);
    let entity = world.resource::<AiIndex>().get(AiId::parse(id).unwrap()).unwrap_or_else(|| panic!("member {member} has no {id}"));
    world.get::<Transform>(entity).unwrap().translation
}

fn desyncs(stand: &Stand, member: usize) -> u32 {
    stand.world(member).get_resource::<NetDesync>().map_or(0, |d| d.count)
}

/// Plays the host's recording in single player: it must come out the same.
fn verify_host(stand: &Stand) {
    let recording = shards::ai_core::replay::save(stand.world(HOST));
    let mut config = shards::testing::config();
    config.seed = recording.seed;
    let mut app = sandbox::build_app(&config);
    app.finish();
    app.cleanup();
    let report = shards::testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "the host's replay does not play the same in single player: {report}");
}

#[test]
fn clients_join_at_tick_zero_and_see_the_hosts_world() {
    let mut stand = stand(2);
    assert_eq!(sim::tick(stand.world(HOST)), 0, "joined while the host waited at tick 0");
    let (b1, b2) = (body(&stand, C1), body(&stand, C2));
    assert_ne!(b1, b2);
    assert!(has_line(&stand, HOST, &format!("p1 joined as {b1} (c1)")), "{:#?}", journal(&stand, HOST));
    assert!(has_line(&stand, HOST, &format!("p2 joined as {b2} (c2)")));
    // Every member has the same ids for the same things.
    for member in [C1, C2] {
        assert_eq!(position(&stand, member, BOSS), position(&stand, HOST, BOSS));
        assert_eq!(position(&stand, member, &b1), position(&stand, HOST, &b1));
    }
    let seen = stand.call(C1, "observe", json!({}));
    assert!(text(&seen).contains("NET connected as p1"), "{}", text(&seen));

    stand.run(130);
    for member in [C1, C2] {
        assert_eq!(desyncs(&stand, member), 0, "{:#?}", journal(&stand, member));
        let client = stand.world(member).resource::<NetClient>();
        assert!(client.newest_host_tick >= 120, "states arrive: newest {}", client.newest_host_tick);
    }
    let status = stand.call(HOST, "net_status", json!({}));
    assert!(status.ok, "{}", status.result);
    assert_eq!(status.result["peers"].as_array().unwrap().len(), 2, "{}", status.result);
    // What `shards wait` prints of a member.
    let net = &shards::ai_core::status::status(stand.world(C1))["net"];
    let who = (net["member"].as_str(), net["role"].as_str(), net["player"].as_u64(), net["body"].as_str());
    assert_eq!(who, (Some("c1"), Some("client"), Some(1), Some(b1.as_str())), "{net}");
    verify_host(&stand);
}

#[test]
fn a_dodge_on_a_client_is_an_input_of_the_host_at_its_tick() {
    let mut stand = stand(1);
    let me = body(&stand, C1);
    stand.run(20);
    let dodged = stand.call(C1, "dodge", json!({"dir": "E"}));
    assert!(dodged.ok, "{}", dodged.result);
    let asked_at = sim::tick(stand.world(C1));
    stand.run(30);
    // The host recorded the press as its player's input, between the ticks the client stamped.
    let recording = shards::ai_core::replay::save(stand.world(HOST));
    let pressed: Vec<u64> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            ReplayEvent::Input { at, id, value, .. } if id.to_string() == me && value.to_string().contains("dodge") => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(pressed, [asked_at], "the dodge lands on the tick the client stamped");
    let host = stand.world(HOST).resource::<NetHost>();
    let peer = host.peers.values().next().unwrap();
    assert!(peer.applied_input_tick >= asked_at, "the frame stamped {asked_at} was applied: {}", peer.applied_input_tick);
    assert_eq!(peer.inputs.late, 0, "no input came late");
    verify_host(&stand);
}

// --- conditions, joins, leaves -------------------------------------------------------------

/// The links of the contract's stand tests: 150 ms one way, 2 % loss.
fn lossy() -> LinkConditions {
    "latency=150,loss=0.02".parse().unwrap()
}

fn configure(config: &mut shards::EngineConfig) {
    config.seed = 42;
}

fn sandbox_setup(app: &mut App) {
    app.add_plugins(sandbox::SandboxPlugin);
}

/// Stand ticks until `done` (at most `max`); panics saying what it waited for.
fn run_until(stand: &mut Stand, max: u64, what: &str, done: impl Fn(&Stand) -> bool) {
    for _ in 0..max {
        if done(stand) {
            return;
        }
        stand.run(1);
    }
    assert!(done(stand), "{what}: not after {max} stand ticks");
}

fn connected(stand: &Stand, member: usize) -> bool {
    matches!(net_state(stand, member), NetState::Connected { .. })
}

fn host_calls(stand: &Stand, tool: &str) -> Vec<(u64, String, Value)> {
    shards::ai_core::replay::save(stand.world(HOST))
        .events
        .into_iter()
        .filter_map(|e| match e {
            ReplayEvent::Call { at, agent, tool: t, args, .. } if t == tool => Some((at, agent, args)),
            _ => None,
        })
        .collect()
}

fn entity(stand: &Stand, member: usize, id: &str) -> Option<Entity> {
    stand.world(member).resource::<AiIndex>().get(AiId::parse(id).unwrap())
}

fn events_of(stand: &Stand, member: usize) -> Vec<String> {
    let journal = stand.world(member).resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == JournalKind::Event).map(|e| e.text.clone()).collect()
}

#[test]
fn a_dodge_over_a_lossy_link_lands_on_its_stamped_tick() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(40);
    let dodged = stand.call(C1, "dodge", json!({"dir": "W"}));
    assert!(dodged.ok, "{}", dodged.result);
    let asked_at = sim::tick(stand.world(C1));
    stand.run(60);
    let recording = shards::ai_core::replay::save(stand.world(HOST));
    let pressed: Vec<u64> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            ReplayEvent::Input { at, id, value, .. } if id.to_string() == me && value.to_string().contains("dodge") => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(pressed, [asked_at], "150 ms away, the client runs early enough");
    let host = stand.world(HOST).resource::<NetHost>();
    assert_eq!(host.peers.values().next().unwrap().inputs.late, 0);
    assert_eq!(desyncs(&stand, C1), 0);
    verify_host(&stand);
}

fn boss_fight_on(world: &World, _: &shards::net::JoinRequest) -> shards::net::JoinDecision {
    // The game's flag: a boss fight for the first 90 ticks.
    if sim::tick(world) < 90 {
        shards::net::JoinDecision::Wait("a boss fight is on".into())
    } else {
        shards::net::JoinDecision::Accept
    }
}

#[test]
fn a_player_waits_out_the_boss_fight_then_joins_mid_session() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.world_mut().resource_mut::<shards::net::NetHooks>().join_policy = boss_fight_on;
    };
    let mut stand = shards::testing::stand_host(1, &configure, &setup);
    assert!(stand.call(HOST, "attack", json!({"target": BOSS, "repeat": true})).ok);
    stand.run(30);
    let c1 = shards::testing::stand_client(&mut stand, "c1", lossy(), &configure, &setup);
    stand.run(40);
    assert!(matches!(net_state(&stand, c1), NetState::Joining(why) if why.contains("a boss fight is on")), "{:?}", net_state(&stand, c1));
    assert!(has_line(&stand, HOST, "net: p1 (c1) waits to join: a boss fight is on"), "{:#?}", journal(&stand, HOST));
    assert!(has_line(&stand, c1, "net: waiting to join: a boss fight is on"), "{:#?}", journal(&stand, c1));
    run_until(&mut stand, 120, "c1 joins after the fight", |s| connected(s, c1));
    let joined = host_calls(&stand, "player_join");
    assert_eq!(joined.len(), 1, "{joined:?}");
    assert!(joined[0].0 >= 90, "accepted once the fight was over: {joined:?}");
    assert_eq!(joined[0].1, "net");
    stand.run(90);
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));
    verify_host(&stand);
}

/// Guests whose dev calls run with dev scope on the host (the default is player scope).
fn dev_guests() -> shards::net::NetDefaults {
    shards::net::NetDefaults(shards::net::NetDef { guest_scope: Scope::Dev, ..Default::default() })
}

#[test]
fn a_forwarded_teleport_runs_on_the_host_and_is_recorded_there() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(20);
    // A guest's calls run with player scope at most: teleport is refused there.
    let sent = stand.call(C1, "teleport", json!({"id": me, "x": 4.0, "z": -2.0}));
    assert!(sent.ok, "{}", sent.result);
    stand.run(40);
    assert!(has_line(&stand, C1, "host: teleport failed at host tick"), "{:#?}", journal(&stand, C1));
    assert!(host_calls(&stand, "teleport").is_empty());

    stand.world_mut(HOST).insert_resource(dev_guests());
    let sent = stand.call(C1, "teleport", json!({"id": me, "x": 4.0, "z": -2.0}));
    assert!(sent.ok, "{}", sent.result);
    assert!(text(&sent).starts_with("sent to the host; applies at host tick ~"), "{}", text(&sent));
    stand.run(40);
    let calls = host_calls(&stand, "teleport");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].1, "p1/dev", "the client's agent, under its player");
    assert!(has_line(&stand, C1, "host: teleport ok at host tick"), "{:#?}", journal(&stand, C1));
    let there = position(&stand, HOST, &me);
    assert!((there.x - 4.0).abs() < 0.01 && (there.z + 2.0).abs() < 0.01, "{there}");
    let seen = position(&stand, C1, &me);
    assert!(seen.distance(there) < 0.5, "the client sees its body moved: {seen} vs {there}");
    verify_host(&stand);
}

#[test]
fn a_kicked_player_rejoins_with_their_token_and_gets_their_body_back() {
    let mut stand = stand_with(1, lossy());
    let me = body(&stand, C1);
    stand.run(20);
    let kicked = stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"}));
    assert!(kicked.ok, "{}", kicked.result);
    stand.run(30);
    assert!(has_line(&stand, HOST, "net: p1 left (kicked: testing)"), "{:#?}", journal(&stand, HOST));
    assert!(matches!(net_state(&stand, C1), NetState::Ended(why) if why.contains("kicked: testing")), "{:?}", net_state(&stand, C1));
    let left = host_calls(&stand, "player_leave");
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0].2["reason"], "kicked: testing");
    let waiting = entity(&stand, HOST, &me).expect("the body waits for a rejoin");
    assert!(stand.world(HOST).get::<shards::net::NetLeft>(waiting).is_some());

    let again = shards::testing::stand_client(&mut stand, "c1", lossy(), &configure, &sandbox_setup);
    run_until(&mut stand, 300, "c1 rejoins", |s| connected(s, again));
    assert_eq!(body(&stand, again), me, "the same body");
    assert!(matches!(net_state(&stand, again), NetState::Connected { player: 1, .. }));
    assert!(stand.world(HOST).get::<shards::net::NetLeft>(waiting).is_none());
    assert_eq!(host_calls(&stand, "player_join").len(), 2);
    stand.run(60);
    assert_eq!(desyncs(&stand, again), 0, "{:#?}", journal(&stand, again));
    verify_host(&stand);
}

#[test]
fn a_silent_client_times_out_and_its_body_waits_for_it() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.insert_resource(shards::net::NetDefaults(shards::net::NetDef { timeout_secs: 1.0, ..Default::default() }));
    };
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, setup);
    let me = body(&stand, C1);
    stand.run(20);
    stand.set_link(C1, "loss=1".parse().unwrap()).unwrap();
    stand.run(90);
    assert!(has_line(&stand, HOST, "net: p1 left (timeout)"), "{:#?}", journal(&stand, HOST));
    assert_eq!(host_calls(&stand, "player_leave")[0].2["reason"], "timeout");
    assert!(matches!(net_state(&stand, C1), NetState::Ended(why) if why.contains("no answer for 1 s")), "{:?}", net_state(&stand, C1));
    let waiting = entity(&stand, HOST, &me).expect("the body waits");
    assert!(stand.world(HOST).get::<shards::net::NetLeft>(waiting).is_some());
    // The host forgets the silent peer.
    stand.run(200);
    assert!(stand.world(HOST).resource::<NetHost>().peers.is_empty());
    verify_host(&stand);
}

// --- the world again -----------------------------------------------------------------------

#[test]
fn a_region_change_on_the_host_sends_every_client_the_new_world() {
    let mut stand = stand_with(2, lossy());
    stand.run(20);
    let regen = stand.call(HOST, "regen", json!({"region": "wilds", "seed": 42}));
    assert!(regen.ok, "{}", regen.result);
    stand.run(60);
    let hash = stand.world(HOST).resource::<shards::world::ActiveRegion>().hash;
    for (member, n) in [(C1, 1u8), (C2, 2)] {
        assert!(has_line(&stand, member, "net: the host's world again (region wilds)"), "{:#?}", journal(&stand, member));
        assert!(has_line(&stand, member, &format!("{hash:016x}")), "the region's hash is journaled: {:#?}", journal(&stand, member));
        assert_eq!(stand.world(member).resource::<shards::world::ActiveRegion>().hash, hash, "the same region");
        let host_body = shards::core::player_body(stand.world(HOST), n).expect("a body in the new region");
        assert_eq!(body(&stand, member), host_body.to_string());
        assert_eq!(desyncs(&stand, member), 0, "{:#?}", journal(&stand, member));
    }
    stand.run(60);
    for member in [C1, C2] {
        assert_eq!(desyncs(&stand, member), 0, "{:#?}", journal(&stand, member));
    }
    verify_host(&stand);
}

#[test]
fn a_snapshot_loaded_on_the_host_goes_to_its_clients() {
    let mut stand = stand_with(1, lossy());
    stand.run(30);
    let saved = snapshot::capture(stand.world(HOST)).unwrap();
    assert!(stand.call(HOST, "teleport", json!({"id": BOSS, "x": 5.0, "z": 5.0})).ok);
    stand.run(60);
    snapshot::restore(stand.world_mut(HOST), &saved, false).unwrap();
    stand.run(40);
    assert!(has_line(&stand, C1, &format!("net: the host's world again (snapshot of tick {})", saved.tick)), "{:#?}", journal(&stand, C1));
    assert_eq!(stand.world(C1).resource::<NetClient>().rebases, 1);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    // The client runs on the host's new timeline again.
    let (host_tick, client_tick) = (sim::tick(stand.world(HOST)), sim::tick(stand.world(C1)));
    assert!(client_tick > host_tick && client_tick < host_tick + 30, "host {host_tick}, client {client_tick}");
    let dodged = stand.call(C1, "dodge", json!({"dir": "E"}));
    assert!(dodged.ok, "{}", dodged.result);
    stand.run(40);
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.values().next().unwrap().inputs.late, 0);
}

#[test]
fn a_client_whose_copy_differs_is_told_and_sent_the_world_again() {
    let mut stand = stand(1);
    stand.run(30);
    // Something on c1 writes what only the host may: the boss's health.
    let boss = entity(&stand, C1, BOSS).unwrap();
    stand.world_mut(C1).get_mut::<Health>(boss).unwrap().current = 1.0;
    run_until(&mut stand, 90, "the desync check", |s| desyncs(s, C1) > 0);
    let last = stand.world(C1).resource::<NetDesync>().last.clone().unwrap();
    assert!(last.2.starts_with(&format!("{BOSS} Health: here")), "{last:?}");
    assert!(has_line(&stand, C1, &format!("net: desync at host tick {}: {BOSS} Health", last.0)), "{:#?}", journal(&stand, C1));
    stand.run(20);
    assert!(has_line(&stand, HOST, &format!("net: p1 desync at tick {}", last.0)), "{:#?}", journal(&stand, HOST));
    assert!(has_line(&stand, C1, "net: the host's world again (desync)"), "{:#?}", journal(&stand, C1));
    let fixed = stand.world(C1).get::<Health>(entity(&stand, C1, BOSS).unwrap()).unwrap().current;
    assert_eq!(fixed, stand.world(HOST).get::<Health>(entity(&stand, HOST, BOSS).unwrap()).unwrap().current);
    stand.run(150);
    assert_eq!(desyncs(&stand, C1), 1, "one desync, then the same again: {:#?}", journal(&stand, C1));
    assert_eq!(stand.world(HOST).resource::<NetHost>().desyncs, 1);
}

// --- what a client sees --------------------------------------------------------------------

#[test]
fn a_client_sees_the_hosts_npcs_only_within_its_interest() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        let def = shards::net::NetDef { interest_radius: 3.0, interest_hysteresis: 0.5, ..dev_guests().0 };
        app.insert_resource(shards::net::NetDefaults(def));
    };
    let mut stand = shards::testing::stand(1, lossy(), configure, setup);
    let me = body(&stand, C1);
    let boss_at = position(&stand, HOST, BOSS);
    assert!(position(&stand, HOST, &me).distance(boss_at) > 4.0, "the boss starts out of reach");
    stand.run(30);
    assert!(entity(&stand, C1, BOSS).is_none(), "out of view");
    let inspected = stand.call(C1, "inspect", json!({"id": BOSS}));
    assert!(text(&inspected).contains("out of view"), "{}", text(&inspected));
    assert!(entity(&stand, HOST, BOSS).is_some());

    assert!(stand.call(C1, "teleport", json!({"id": me, "x": boss_at.x + 1.5, "z": boss_at.z})).ok);
    stand.run(40);
    assert!(entity(&stand, C1, BOSS).is_some(), "in view again");
    let seen = stand.call(C1, "observe", json!({}));
    assert!(text(&seen).contains(BOSS), "{}", text(&seen));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn the_hosts_gameplay_lines_reach_a_client_in_order() {
    let mut stand = stand_with(1, lossy());
    assert!(stand.call(C1, "attack", json!({"target": BOSS, "repeat": true})).ok);
    let waited = stand.call(C1, "step", json!({"ticks": 900, "until": [{"journal": "winds up"}]}));
    assert!(waited.ok, "{}", waited.result);
    assert!(text(&waited).contains("winds up"), "{}", text(&waited));
    stand.run(60);
    let (host, client) = (events_of(&stand, HOST), events_of(&stand, C1));
    assert!(client.iter().any(|l| l.contains("winds up")), "{client:#?}");
    let first = host.iter().position(|l| *l == client[0]).expect("the client's first line is the host's");
    assert_eq!(&host[first..first + client.len()], &client[..], "the same lines, in the same order");
}

/// What a member's observers of the engine's typed events saw.
#[derive(Resource, Default)]
struct Seen(Vec<String>, Vec<Entity>);

fn watch_events(world: &mut World) {
    world.init_resource::<Seen>();
    world.add_observer(|event: On<shards::action::Damaged>, mut seen: ResMut<Seen>| {
        let e = event.event();
        seen.0.push(format!("damaged {} {} hp {}", e.action, e.amount, e.hp_left));
        seen.1.push(e.target);
    });
    world.add_observer(|event: On<shards::action::Telegraph>, mut seen: ResMut<Seen>| {
        let e = event.event();
        seen.0.push(format!("telegraph {} in {}", e.action, e.lands_in));
        seen.1.push(e.attacker);
    });
}

#[test]
fn the_hosts_typed_events_reach_a_client_with_its_entities() {
    let mut stand = stand_with(1, lossy());
    watch_events(stand.world_mut(HOST));
    watch_events(stand.world_mut(C1));
    assert!(stand.call(C1, "attack", json!({"target": BOSS, "repeat": true})).ok);
    stand.run(600);
    let (host, client) = (&stand.world(HOST).resource::<Seen>().0, &stand.world(C1).resource::<Seen>().0);
    assert!(client.iter().any(|l| l.starts_with("damaged")), "{client:#?}");
    assert!(client.iter().any(|l| l.starts_with("telegraph")), "{client:#?}");
    assert_eq!(&host[..client.len()], &client[..], "the host's events, in its order");
    // Entities travel as their `eN`: the client's boss is its own entity of the host's boss.
    let boss = entity(&stand, C1, BOSS).expect("the client has the boss");
    let client_entities = &stand.world(C1).resource::<Seen>().1;
    assert!(client_entities.contains(&boss), "{client_entities:?} has no {boss:?}");
    assert!(!client_entities.contains(&Entity::PLACEHOLDER));
}

// --- determinism, soak ---------------------------------------------------------------------

/// One scripted session: the host's replay as JSON and both clients' journals.
fn scripted_session() -> (String, Vec<String>, Vec<String>) {
    let mut stand = stand_with(2, lossy());
    let b2 = body(&stand, C2);
    assert!(stand.call(C1, "attack", json!({"target": BOSS, "repeat": true})).ok);
    stand.run(40);
    assert!(stand.call(C2, "dodge", json!({"dir": "W"})).ok);
    assert!(stand.call(C2, "teleport", json!({"id": b2, "x": 3.0, "z": 0.0})).ok);
    stand.run(150);
    let replay = shards::serde_json::to_string(&shards::ai_core::replay::save(stand.world(HOST))).unwrap();
    (replay, journal(&stand, C1), journal(&stand, C2))
}

#[test]
fn the_same_calls_make_the_same_session_twice() {
    let (a, b) = (scripted_session(), scripted_session());
    assert_eq!(a.0, b.0, "the host's replay");
    assert_eq!(a.1, b.1, "c1's journal");
    assert_eq!(a.2, b.2, "c2's journal");
}

const DIRS: [&str; 4] = ["N", "E", "S", "W"];

/// 3 minutes of 2 clients doing random things (a seeded script, no agent): no desync.
#[test]
fn a_three_minute_session_stays_in_sync() {
    let mut stand = stand_with(2, lossy());
    let bodies = [body(&stand, C1), body(&stand, C2)];
    let mut rng = DetRng::new(7);
    let ticks = 10_800u64;
    let mut done = 0;
    while done < ticks {
        for (i, member) in [C1, C2].into_iter().enumerate() {
            let call = match rng.below(6) {
                0 => ("move_to", json!({"target": {"x": rng.range(-8.0, 8.0), "z": rng.range(-8.0, 8.0)}})),
                1 => ("dodge", json!({"dir": DIRS[rng.below(4) as usize]})),
                2 => ("light_attack", json!({})),
                3 => ("attack", json!({"target": BOSS, "repeat": true})),
                4 => ("block", json!({})),
                _ => ("stop", json!({})),
            };
            // Refusals (no stamina, busy) are part of play.
            let _ = stand.call(member, call.0, call.1);
            if done % 900 < 60 {
                let _ = stand.call(HOST, "heal", json!({"id": bodies[i]}));
            }
        }
        if done % 900 < 60 {
            let _ = stand.call(HOST, "heal", json!({"id": BOSS}));
        }
        let gap = 20 + rng.below(40) as u64;
        stand.run(gap);
        done += gap;
    }
    for member in [C1, C2] {
        assert!(connected(&stand, member), "{:?}", net_state(&stand, member));
        assert_eq!(desyncs(&stand, member), 0, "{:#?}", journal(&stand, member));
        assert_eq!(stand.world(member).resource::<NetClient>().rebases, 0);
    }
    let host = stand.world(HOST).resource::<NetHost>();
    for peer in host.peers.values() {
        assert_eq!(peer.inputs.late, 0, "p{} inputs came late", peer.player);
    }
}

fn udp_member(net: shards::core::NetLaunch, name: &str) -> App {
    let mut config = shards::testing::config();
    config.seed = 42;
    config.net = net;
    config.player_name = name.into();
    let mut app = sandbox::build_app(&config);
    app.finish();
    app.cleanup();
    app
}

/// The one test on a real socket: a host on a free UDP port of this machine and a client that
/// joins it, in real time (at most 10 s). Only that it connects and stays in sync.
#[test]
fn a_client_joins_a_host_over_udp_loopback() {
    let started = Instant::now();
    let deadline = Duration::from_secs(10);
    let mut host = udp_member(shards::core::NetLaunch::Host { port: 0 }, "host");
    shards::testing::wait_until_loaded(&mut host, deadline).unwrap();
    let address = host.world().resource::<shards::net::NetEndpoint>().transport.local_addr().expect("a bound socket");
    let port = address.rsplit(':').next().unwrap().to_owned();
    let mut client = udp_member(shards::core::NetLaunch::Join { addr: format!("127.0.0.1:{port}") }, "udp");
    let mut joined_at = None;
    while started.elapsed() < deadline {
        sim::run_tick(host.world_mut()).unwrap();
        let state = client.world().resource::<NetState>().clone();
        match state {
            NetState::Connected { .. } => {
                sim::run_tick(client.world_mut()).unwrap();
                let at = *joined_at.get_or_insert(sim::tick(host.world()));
                if sim::tick(host.world()) > at + 90 {
                    break;
                }
            }
            NetState::Ended(why) => panic!("the client ended: {why}"),
            _ => client.update(),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let journal: Vec<String> = client.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect();
    assert!(joined_at.is_some(), "not connected within {deadline:?}: {journal:#?}");
    let newest = client.world().resource::<NetClient>().newest_host_tick;
    assert!(newest > joined_at.unwrap() + 60, "states arrive: newest host tick {newest}");
    assert_eq!(client.world().get_resource::<NetDesync>().map_or(0, |d| d.count), 0, "{journal:#?}");
    let host_journal: Vec<String> = host.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect();
    assert!(host_journal.iter().any(|l| l.contains("net: p1 (udp) joined at tick")), "{host_journal:#?}");
    assert!(host_journal.iter().any(|l| l.contains(&format!("net: hosting on UDP 0.0.0.0:{port}"))), "{host_journal:#?}");
}

