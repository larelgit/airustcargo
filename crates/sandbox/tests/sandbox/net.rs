//! Co-op over the network: a host and its clients as apps of one process (`testing::Stand`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use shards::ai_core::state::{StateEncoder, StateRule};
use shards::ai_core::replay::ReplayEvent;
use shards::ai_core::journal::JournalKind;
use shards::ai_core::{sim, snapshot, AiIndex, AiJournal};
use shards::core::NetPrediction;
use shards::net::{LinkConditions, NetClient, NetCorrection, NetDesync, NetHost, NetState, Stand};
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
        if !shards::ai_core::sim::tick(app.world()).is_multiple_of(send_every) {
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
        println!("{place}: heaviest components {}", status.result["bandwidth_by_component"]);
    }
}

// --- the stand -----------------------------------------------------------------------------

pub(super) const HOST: usize = 0;
pub(super) const C1: usize = 1;
pub(super) const C2: usize = 2;

/// A sandbox stand: the host (seed 42) and `clients` clients, joined at tick 0.
pub(super) fn stand(clients: u8) -> Stand {
    stand_with(clients, LinkConditions::NONE)
}

pub(super) fn stand_with(clients: u8, links: LinkConditions) -> Stand {
    shards::testing::stand(clients, links, |c| c.seed = 42, |app| {
        app.add_plugins(sandbox::SandboxPlugin);
    })
}

pub(super) fn net_state(stand: &Stand, member: usize) -> NetState {
    stand.world(member).resource::<NetState>().clone()
}

/// The body of member `member`'s player, as an `eN`.
pub(super) fn body(stand: &Stand, member: usize) -> String {
    match net_state(stand, member) {
        NetState::Connected { body: Some(b), .. } => format!("e{b}"),
        other => panic!("member {member} is not playing: {other:?}"),
    }
}

pub(super) fn journal(stand: &Stand, member: usize) -> Vec<String> {
    let journal = stand.world(member).resource::<AiJournal>();
    journal.after(0).0.map(ToString::to_string).collect()
}

pub(super) fn has_line(stand: &Stand, member: usize, needle: &str) -> bool {
    journal(stand, member).iter().any(|l| l.contains(needle))
}

pub(super) fn position(stand: &Stand, member: usize, id: &str) -> Vec3 {
    let world = stand.world(member);
    let entity = world.resource::<AiIndex>().get(AiId::parse(id).unwrap()).unwrap_or_else(|| panic!("member {member} has no {id}"));
    world.get::<Transform>(entity).unwrap().translation
}

pub(super) fn desyncs(stand: &Stand, member: usize) -> u32 {
    stand.world(member).get_resource::<NetDesync>().map_or(0, |d| d.count)
}

/// Plays the host's recording in single player: it must come out the same.
pub(super) fn verify_host(stand: &Stand) {
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
    let heaviest = status.result["bandwidth_by_component"].as_array().cloned().unwrap_or_default();
    assert!((1..=10).contains(&heaviest.len()), "{}", status.result);
    assert!(heaviest.iter().any(|c| c["component"] == "Transform"), "{heaviest:?}");
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
pub(super) fn lossy() -> LinkConditions {
    "latency=150,loss=0.02".parse().unwrap()
}

pub(super) fn configure(config: &mut shards::EngineConfig) {
    config.seed = 42;
}

pub(super) fn sandbox_setup(app: &mut App) {
    app.add_plugins(sandbox::SandboxPlugin);
}

/// Stand ticks until `done` (at most `max`); panics saying what it waited for.
pub(super) fn run_until(stand: &mut Stand, max: u64, what: &str, done: impl Fn(&Stand) -> bool) {
    for _ in 0..max {
        if done(stand) {
            return;
        }
        stand.run(1);
    }
    assert!(done(stand), "{what}: not after {max} stand ticks");
}

pub(super) fn connected(stand: &Stand, member: usize) -> bool {
    matches!(net_state(stand, member), NetState::Connected { .. })
}

pub(super) fn host_calls(stand: &Stand, tool: &str) -> Vec<(u64, String, Value)> {
    shards::ai_core::replay::save(stand.world(HOST))
        .events
        .into_iter()
        .filter_map(|e| match e {
            ReplayEvent::Call { at, agent, tool: t, args, .. } if t == tool => Some((at, agent, args)),
            _ => None,
        })
        .collect()
}

pub(super) fn entity(stand: &Stand, member: usize, id: &str) -> Option<Entity> {
    stand.world(member).resource::<AiIndex>().get(AiId::parse(id).unwrap())
}

pub(super) fn events_of(stand: &Stand, member: usize) -> Vec<String> {
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

fn door_closed(_: &World, _: &shards::net::JoinRequest) -> shards::net::JoinDecision {
    shards::net::JoinDecision::Wait("the door is closed".into())
}

#[test]
fn a_player_the_game_keeps_waiting_is_not_dropped_for_joining_slowly_and_a_kick_reaches_it() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.world_mut().resource_mut::<shards::net::NetHooks>().join_policy = door_closed;
    };
    let mut stand = shards::testing::stand_host(1, &configure, &setup);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &setup);
    // Joining may take 6 s here; the game keeps this player waiting for more than that.
    stand.world_mut(HOST).insert_resource(shards::net::NetDefaults(shards::net::NetDef { timeout_secs: 1.0, ..Default::default() }));
    stand.run(450);
    assert!(matches!(net_state(&stand, c1), NetState::Joining(why) if why.contains("the door is closed")), "{:?}", net_state(&stand, c1));
    let waiting = |stand: &Stand| stand.world(HOST).resource::<NetHost>().peers.values().filter(|p| matches!(p.stage, shards::net::host::PeerStage::Waiting(_))).count();
    assert_eq!(waiting(&stand), 1);
    let kicked = stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"}));
    assert!(kicked.ok, "{}", kicked.result);
    stand.run(20);
    assert!(matches!(net_state(&stand, c1), NetState::Ended(why) if why.contains("kicked: testing")), "{:?}", net_state(&stand, c1));
    assert_eq!(waiting(&stand), 0);
    assert!(stand.world(HOST).resource::<NetHost>().peers.values().all(|p| p.stage == shards::net::host::PeerStage::Gone));
    assert!(!stand.call(HOST, "net_kick", json!({"player": 1})).ok, "nobody is left to kick");
}

/// Guests whose dev calls run with dev scope on the host (the default is player scope).
pub(super) fn dev_guests() -> shards::net::NetDefaults {
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
    // The client's own count is in its `net_status` too (what `shards soak` reads of every member).
    let status = stand.call(C1, "net_status", json!({}));
    assert_eq!((status.result["desyncs"].as_u64(), status.result["last_desync"]["host_tick"].as_u64()), (Some(1), Some(last.0)), "{}", status.result);
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

/// The hits a member's observers saw, by `eN`: `(attacker, target, action, amount, hp left)`, and
/// the `source` of each (an object, by `eN` when the member has it).
#[derive(Resource, Default)]
struct Hits(Vec<(String, String, String, String, String)>, Vec<Option<String>>);

fn watch_hits(world: &mut World) {
    world.init_resource::<Hits>();
    world.add_observer(|event: On<shards::action::Damaged>, ids: Query<&AiId>, mut hits: ResMut<Hits>| {
        let e = event.event();
        let name = |entity: Entity| ids.get(entity).map_or_else(|_| format!("{entity}"), ToString::to_string);
        hits.0.push((name(e.attacker), name(e.target), e.action.clone(), e.amount.to_string(), e.hp_left.to_string()));
        hits.1.push(e.source.map(name));
    });
}

/// The combat objects a member has, by `eN`, in id order.
fn objects(stand: &Stand, member: usize) -> Vec<String> {
    let world = stand.world(member);
    let Some(mut q) = world.try_query::<(&AiId, &shards::action::CombatObject)>() else { return Vec::new() };
    let mut ids: Vec<AiId> = q.iter(world).map(|(id, _)| *id).collect();
    ids.sort();
    ids.into_iter().map(|id| id.to_string()).collect()
}

/// Spawned combat objects (B4) on a stand of two: the host's caster volleys at c1's player and the
/// bolt that meets it lays a puddle there, while c2's player watches from the side. Both clients
/// are sent the objects with the host's ids, and the hits they see are the host's, in order, but
/// for `source`: an object that is gone by the time an event reaches a client comes as no entity
/// (the comparison modulo `source` is the test's).
#[test]
fn objects_hit_the_same_on_host_and_client() {
    use shards::ai_core::{call_tool, AgentSessions, Scope};
    let mut stand = stand_with(2, lossy());
    for member in [HOST, C1, C2] {
        watch_hits(stand.world_mut(member));
    }
    let (target, watcher) = (body(&stand, C1), body(&stand, C2));
    for id in ["e6", "e7", "e8", "e14"] {
        assert!(stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"})).ok);
    }
    let places = [(target.as_str(), 5.0, 0.0), (watcher.as_str(), -15.0, 12.0), ("e8", 5.0, -15.0), (PLAYER, -15.0, 15.0)];
    for (id, x, z) in places {
        assert!(stand.call(HOST, "teleport", json!({"id": id, "x": x, "z": z, "y": 0})).ok);
    }
    let caster = json!({"id": "e8", "component": "Fighter", "path": "moveset", "value": "caster"});
    assert!(stand.call(HOST, "set", caster).ok);
    stand.run(2);
    stand.world_mut(HOST).resource_mut::<AgentSessions>().configure("caster", Scope::Dev, Some(AiId(8)));
    for (tool, args) in [("lock_on", json!({"target": target})), ("light_attack", json!({}))] {
        let out = call_tool(stand.world_mut(HOST), "caster", tool, args);
        assert!(out.ok, "{tool}: {}", out.result);
    }
    // The volley is out at 1 s: three bolts in the air. A side bolt bursts at the pillar 0.43 s
    // later, the middle one at the player 0.92 s later, the last at the south wall 2.3 s later; from
    // then on the three puddles are the only objects for 1.5 s, and both clients have them too.
    stand.run(80);
    let flying = objects(&stand, HOST);
    assert_eq!(flying.len(), 3, "three bolts on the host: {flying:?}");
    for member in [C1, C2] {
        assert_eq!(objects(&stand, member), flying, "member {member} has the host's bolts in the air, by id");
    }
    stand.run(180);
    let puddles = objects(&stand, HOST);
    assert_eq!(puddles.len(), 3, "three puddles on the host: {puddles:?}");
    for member in [C1, C2] {
        assert_eq!(objects(&stand, member), puddles, "member {member} has the host's objects, by id");
    }
    // The bolt met the player, its burst laid a puddle, and the puddle hit again.
    stand.run(100);
    let host = &stand.world(HOST).resource::<Hits>();
    let bolt = host.0.iter().filter(|h| h.1 == target && h.2 == "bolt_fly").count();
    let puddle = host.0.iter().filter(|h| h.1 == target && h.2 == "chill_puddle").count();
    assert_eq!(bolt, 1, "one bolt meets c1's player: {:?}", host.0);
    assert!(puddle >= 2, "the puddle hits every 0.8 s: {:?}", host.0);
    assert!(host.0.iter().all(|h| h.0 == "e8" && h.1 == target), "every hit is the caster's, on c1's player: {:?}", host.0);
    assert!(host.1.iter().all(Option::is_some), "each names its object: {:?}", host.1);
    // Let the puddles run out (3 s) and the last events cross the link: then every member has seen
    // every hit the host dealt, and no object is left anywhere.
    stand.run(360);
    let host = &stand.world(HOST).resource::<Hits>();
    for member in [C1, C2] {
        let seen = &stand.world(member).resource::<Hits>().0;
        assert!(!seen.is_empty(), "member {member} saw no hit");
        assert_eq!(&host.0, seen, "member {member}: the host's hits, all of them, in its order, modulo source");
        assert!(objects(&stand, member).is_empty(), "member {member}: {:?}", objects(&stand, member));
        assert_eq!(desyncs(&stand, member), 0);
    }
    assert!(objects(&stand, HOST).is_empty());
}

/// A member's body parts (R1): `(part, its body, its id, on, broken, where it is to the cm)`, in id order.
fn parts(stand: &Stand, member: usize) -> Vec<(String, String, String, bool, bool, [i32; 3])> {
    let world = stand.world(member);
    let Some(mut q) = world.try_query::<(&AiId, &shards::action::Part, &Transform)>() else { return Vec::new() };
    let mut out: Vec<_> = q
        .iter(world)
        .map(|(id, p, tf)| {
            let at = (tf.translation * 100.0).round();
            (id.to_string(), p.body.to_string(), p.id.clone(), p.on, p.broken, [at.x as i32, at.y as i32, at.z as i32])
        })
        .collect();
    out.sort();
    out
}

/// Body parts (R1) in co-op: the host spawns the colossus by c1's player; c1 is sent its parts
/// with the host's ids, where the host has them (a client spawns none of its own), and c1's swings
/// land on the host through the hand, which breaks there: c1 sees the same hits and the broken hand.
/// The colossus gone, its parts go on both.
#[test]
fn parts_come_to_a_client_and_its_swings_land_through_them() {
    let mut stand = stand_with(1, lossy());
    for member in [HOST, C1] {
        watch_hits(stand.world_mut(member));
    }
    let player = body(&stand, C1);
    for id in ["e6", "e7", "e8", "e14"] {
        assert!(stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"})).ok);
    }
    for (id, x, z) in [(player.as_str(), 0.0, 1.4), (PLAYER, -15.0, 15.0)] {
        assert!(stand.call(HOST, "teleport", json!({"id": id, "x": x, "z": z, "y": 0})).ok);
    }
    let spawned = stand.call(HOST, "spawn_prefab", json!({"id": "colossus", "x": 0, "z": 4, "y": 0}));
    assert!(spawned.ok, "{}", spawned.result);
    let colossus = spawned.result.as_str().and_then(|t| t.split_whitespace().nth(1)).unwrap().to_owned();
    stand.run(40);
    let host = parts(&stand, HOST);
    assert_eq!(host.len(), 5, "{host:?}");
    assert!(host.iter().all(|p| p.1 == colossus));
    assert_eq!(parts(&stand, C1), host, "c1 has the host's parts, by id, where the host has them");

    // The colossus is locked on to through its lock points (R3): c1 takes the hand, and the host keeps
    // that lock (c1's input) through its check.
    let locked = stand.call(C1, "lock_on", json!({"target": colossus, "point": "hand"}));
    assert!(locked.ok, "{}", locked.result);
    assert!(locked.result.as_str().is_some_and(|t| t.contains(&format!("(hand of {colossus})"))), "{}", locked.result);
    let hand_id = parts(&stand, HOST).into_iter().find(|p| p.2 == "hand").unwrap().0;
    stand.run(40);
    let lock = |stand: &Stand, member| {
        let world = stand.world(member);
        let mut q = world.try_query::<(&AiId, &Intent)>().unwrap();
        q.iter(world).find(|(id, _)| id.to_string() == player).and_then(|(_, i)| i.target).map(|t| t.to_string())
    };
    assert_eq!(lock(&stand, HOST), Some(hand_id.clone()), "the host holds c1's lock on the hand");
    for _ in 0..3 {
        let swung = stand.call(C1, "light_attack", json!({}));
        assert!(swung.ok, "{}", swung.result);
        stand.run(90);
    }
    let hits = stand.world(HOST).resource::<Hits>().0.clone();
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(hits.iter().all(|h| h.0 == player && h.1 == colossus && h.3 == "10"), "each through the hand, half to the body: {hits:?}");
    let hand = |member| parts(&stand, member).into_iter().find(|p| p.2 == "hand").unwrap();
    assert!(!hand(HOST).3 && hand(HOST).4, "the hand broke on the host: {:?}", hand(HOST));
    assert_eq!(hand(C1), hand(HOST), "and c1 has it broken");
    assert_eq!((lock(&stand, HOST), lock(&stand, C1)), (None, None), "the broken hand is no lock point: let go of on both");
    assert_eq!(stand.world(C1).resource::<Hits>().0, hits, "c1 saw the host's hits");

    assert!(stand.call(HOST, "despawn", json!({"id": colossus})).ok);
    stand.run(40);
    for member in [HOST, C1] {
        assert!(parts(&stand, member).is_empty(), "member {member}: {:?}", parts(&stand, member));
        assert_eq!(desyncs(&stand, member), 0);
    }
}

/// Where the body `id` stands on `member`, if it has it.
fn feet_on(stand: &Stand, member: usize, id: &str) -> Option<Vec3> {
    let world = stand.world(member);
    let mut q = world.try_query::<(&AiId, &Transform)>()?;
    q.iter(world).find(|(i, _)| i.to_string() == id).map(|(_, t)| t.translation)
}

/// Temporary ground (R2) in co-op: a slab the host lays down in the middle of play (a static
/// `WORLD` collider with `AiKind`, so it has an `AiId`) comes to c1 with the host's id, and c1's
/// body stands on it on both machines (the host's simulation, c1's prediction); taken away, the
/// body falls to the floor on both, and the slab is gone from c1.
#[test]
fn ground_laid_mid_play_comes_to_a_client_and_holds_its_body() {
    use shards::ai_core::AiKind;
    use shards::motion::{Collider, Shape};
    let mut stand = stand_with(1, lossy());
    let player = body(&stand, C1);
    for id in ["e6", "e7", "e8", "e14"] {
        assert!(stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"})).ok);
    }
    stand.run(10);
    let slab = stand
        .world_mut(HOST)
        .spawn((Name::new("Slab"), AiKind("Slab".into()), Collider::new(Shape::cuboid(Vec3::new(1.5, 0.5, 1.5))), Transform::from_xyz(0.0, 0.5, 4.0)))
        .id();
    let slab = stand.world(HOST).get::<AiId>(slab).unwrap().to_string();
    assert!(stand.call(HOST, "teleport", json!({"id": player, "x": 0, "z": 4, "y": 3})).ok);
    stand.run(120);
    let slab_on = |stand: &Stand, member| {
        let world = stand.world(member);
        let mut q = world.try_query::<(&AiId, &Collider)>().unwrap();
        q.iter(world).any(|(id, _)| id.to_string() == slab)
    };
    assert!(slab_on(&stand, C1), "c1 has the slab, by the host's id, with its collider");
    for member in [HOST, C1] {
        let at = feet_on(&stand, member, &player).unwrap();
        assert!((at.y - 1.0).abs() < 0.05, "member {member}: c1's body stands on the slab: {at}");
    }

    assert!(stand.call(HOST, "despawn", json!({"id": slab})).ok);
    stand.run(120);
    for member in [HOST, C1] {
        let at = feet_on(&stand, member, &player).unwrap();
        assert!(at.y.abs() < 0.05, "member {member}: back on the floor: {at}");
        assert_eq!(desyncs(&stand, member), 0);
    }
    assert!(!slab_on(&stand, C1), "the slab is gone from c1");
}

/// The `ActionSound`s a member heard: `(actor, action, sound)`.
#[derive(Resource, Default)]
struct Sounds(Vec<(Entity, String, String)>);

fn watch_sounds(world: &mut World) {
    world.init_resource::<Sounds>();
    world.add_observer(|event: On<shards::action::ActionSound>, mut heard: ResMut<Sounds>| {
        let e = event.event();
        heard.0.push((e.entity, e.action.clone(), e.sound.clone()));
    });
}

#[test]
fn a_swings_whoosh_is_the_hosts_event_and_a_client_hears_it_once_in_its_own_mixer() {
    use shards::audio::Mixer;
    let mut stand = stand_with(1, lossy());
    watch_sounds(stand.world_mut(HOST));
    watch_sounds(stand.world_mut(C1));
    let me = body(&stand, C1);
    assert!(stand.call(C1, "light_attack", json!({})).ok);
    // The mixer's voices as they play: each whoosh voice of the body, by its id, once.
    let mut whooshes: [std::collections::BTreeSet<u64>; 2] = Default::default();
    for _ in 0..180 {
        stand.run(1);
        for (set, member) in whooshes.iter_mut().zip([HOST, C1]) {
            let mixer = stand.world(member).resource::<Mixer>();
            set.extend(mixer.voices().iter().filter(|v| v.sound == "whoosh" && v.source_id.is_some_and(|i| i.to_string() == me)).map(|v| v.id));
        }
    }
    for member in [HOST, C1] {
        let heard = &stand.world(member).resource::<Sounds>().0;
        let actor = entity(&stand, member, &me).expect("the body is in every world");
        assert_eq!(heard, &[(actor, "light_1".to_owned(), "whoosh".to_owned())], "member {member}: the swing's one sound, from the body");
    }
    assert_eq!(whooshes.clone().map(|set| set.len()), [1, 1], "one voice on the host, one on the client: {whooshes:?}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
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
    // Per client: the sum of the windowed mean errors, the largest error, the samples.
    let mut errors = [(0.0f64, 0.0f32, 0u32); 2];
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
        for (i, member) in [C1, C2].into_iter().enumerate() {
            let (mean, max) = stand.world(member).resource::<NetCorrection>().error();
            errors[i].0 += mean as f64;
            errors[i].1 = errors[i].1.max(max);
            errors[i].2 += 1;
        }
    }
    for (i, member) in [C1, C2].into_iter().enumerate() {
        let c = stand.world(member).resource::<NetCorrection>();
        let p = stand.world(member).resource::<NetPrediction>();
        println!(
            "soak c{}: prediction error mean {:.4} m, max {:.3} m; compared {}, blends {}, snaps {} ({:?}); actions predicted {}, confirmed {}, mispredicted {}",
            i + 1,
            errors[i].0 / errors[i].2.max(1) as f64,
            errors[i].1,
            c.compared,
            c.blends,
            c.snaps,
            c.last_snap,
            p.predicted,
            p.confirmed,
            p.mispredicted
        );
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

pub(super) fn udp_member(net: shards::core::NetLaunch, name: &str, save_dir: Option<&str>) -> App {
    let mut config = shards::testing::config();
    config.seed = 42;
    config.net = net;
    config.player_name = name.into();
    if let Some(dir) = save_dir {
        config.save_dir = dir.into();
    }
    let mut app = sandbox::build_app(&config);
    app.finish();
    app.cleanup();
    app
}

/// Host and client in real time until the client has played `ticks` ticks of the host's
/// connected, or `deadline` (from `started`) is over. Returns the host's tick when it joined.
pub(super) fn play_over_udp(host: &mut App, client: &mut App, ticks: u64, started: Instant, deadline: Duration) -> Option<u64> {
    let mut joined_at = None;
    while started.elapsed() < deadline {
        sim::run_tick(host.world_mut()).unwrap();
        let state = client.world().resource::<NetState>().clone();
        match state {
            NetState::Connected { .. } => {
                sim::run_tick(client.world_mut()).unwrap();
                let at = *joined_at.get_or_insert(sim::tick(host.world()));
                if sim::tick(host.world()) > at + ticks {
                    break;
                }
            }
            NetState::Ended(why) => panic!("the client ended: {why}"),
            _ => client.update(),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    joined_at
}

fn lines(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
}

/// The one test on a real socket: a host on a free UDP port of this machine and a client that
/// joins it, in real time (at most 10 s). It connects and stays in sync; it keeps the token
/// its host issued beside its saves, quits, and its next run (a new process to the host: a
/// new link) takes its slot and its body back with it.
#[test]
fn a_client_joins_a_host_over_udp_loopback() {
    let started = Instant::now();
    let deadline = Duration::from_secs(10);
    // A save folder of this run's own, inside the game's folder (as `--save-dir` must be),
    // gone when the test ends, passed or not.
    struct Gone(String);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let saves = format!("saves/test-net-udp-{}", std::process::id());
    let _gone = Gone(saves.clone());
    let _ = std::fs::remove_dir_all(&saves);
    let mut host = udp_member(shards::core::NetLaunch::Host { port: 0 }, "host", None);
    shards::testing::wait_until_loaded(&mut host, deadline).unwrap();
    let address = host.world().resource::<shards::net::NetEndpoint>().transport.local_addr().expect("a bound socket");
    let port = address.rsplit(':').next().unwrap().to_owned();
    // What `shards wait` prints of a host: the address clients join.
    assert_eq!(shards::ai_core::status::status(host.world())["net"]["udp"].as_str(), Some(address.as_str()));
    let join = shards::core::NetLaunch::Join { addr: format!("127.0.0.1:{port}") };
    let mut client = udp_member(join.clone(), "udp", Some(&saves));
    let joined_at = play_over_udp(&mut host, &mut client, 60, started, deadline);
    assert!(joined_at.is_some(), "not connected within {deadline:?}: {:#?}", lines(&client));
    // It knocked first, and said nothing the host dropped for want of its cookie.
    let strangers = &host.world().resource::<NetHost>();
    assert!(strangers.knocks >= 1 && strangers.cookieless == 0, "knocks {}, cookieless {}", strangers.knocks, strangers.cookieless);
    let newest = client.world().resource::<NetClient>().newest_host_tick;
    assert!(newest > joined_at.unwrap() + 30, "states arrive: newest host tick {newest}");
    assert_eq!(client.world().get_resource::<NetDesync>().map_or(0, |d| d.count), 0, "{:#?}", lines(&client));
    assert!(lines(&host).iter().any(|l| l.contains("net: p1 (udp) joined at tick")), "{:#?}", lines(&host));
    assert!(lines(&host).iter().any(|l| l.contains(&format!("net: hosting on UDP 0.0.0.0:{port}"))), "{:#?}", lines(&host));
    let NetState::Connected { player: 1, body: Some(body) } = client.world().resource::<NetState>().clone() else { panic!("not p1") };
    // Over a real network the host owns time.
    for tool in ["pause", "resume", "speed"] {
        let args = if tool == "speed" { json!({"x": 2.0}) } else { json!({}) };
        let out = shards::testing::call(&mut client, tool, args);
        assert!(!out.ok && out.result.as_str().is_some_and(|t| t.contains("the host owns time")), "{tool}: {}", out.result);
    }

    // The host's token: random (nothing of the name or the process), kept beside the saves.
    let token = host.world().resource::<NetHost>().slots[&1].token.clone();
    assert!(token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit()), "{token}");
    let kept = std::fs::read_to_string(format!("{saves}/net/udp@127.0.0.1_{port}.token")).unwrap_or_default();
    assert_eq!(kept, token, "kept for the next run");

    // The player quits; the host keeps the body for their return.
    shards::core::run_exit_hooks(client.world_mut());
    drop(client);
    while started.elapsed() < deadline && !lines(&host).iter().any(|l| l.contains("net: p1 left (quit)")) {
        sim::run_tick(host.world_mut()).unwrap();
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(lines(&host).iter().any(|l| l.contains("net: p1 left (quit)")), "{:#?}", lines(&host));

    let mut again = udp_member(join, "udp", Some(&saves));
    let rejoined = play_over_udp(&mut host, &mut again, 10, started, deadline);
    assert!(rejoined.is_some(), "not back within {deadline:?}: {:#?}", lines(&again));
    let state = again.world().resource::<NetState>().clone();
    assert_eq!(state, NetState::Connected { player: 1, body: Some(body) }, "the same slot and body");
}
