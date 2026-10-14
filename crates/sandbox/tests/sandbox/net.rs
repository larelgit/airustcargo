//! Co-op over the network: a host and its clients as apps of one process (`testing::Stand`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use shards::ai_core::state::{StateEncoder, StateRule};
use shards::ai_core::replay::ReplayEvent;
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

