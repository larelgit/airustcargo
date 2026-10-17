//! What a host takes from its peers: only what an honest client sends, within bounds. The
//! peers here are a real client whose routes or session were changed, and a [`Rogue`] that
//! speaks the protocol by hand.

use std::collections::BTreeMap;

use shards::ai_core::tools::{AiTools, NetRoute};
use shards::ai_core::{configure_session, net_rules, sim, snapshot, AgentSessions, AiIndex, Scope, SessionChange};
use shards::content::ContentDb;
use shards::core::Press;
use shards::net::inputs::InputFrame;
use shards::net::host::{max_strangers, PeerStage, JOINING_POLL_BYTES, PLAYER_POLL_BYTES, SHARED_JOINING_POLL_BYTES};
use shards::net::msg::{self, CallMsg, Hello, Inputs, Msg, StateDelta, Welcome};
use shards::net::{wire, Delivered, Link, LinkConditions, MemoryTransport, NetDef, NetDefaults, NetGame, NetHost, NetState, Role, Stand, Transport};
use shards::prelude::*;
use shards::serde_json::{json, Map, Value};

use super::net::*;
use crate::common::*;

/// A peer of the stand's host that speaks the protocol by hand: whatever a modified client
/// could send.
struct Rogue {
    transport: MemoryTransport,
    link: Link,
    got: Vec<Msg>,
    deltas: Vec<StateDelta>,
}

impl Rogue {
    /// A peer with the host's cookie for its address and session, as its knock would have brought it.
    fn new(stand: &Stand, name: &str, session: u32) -> Self {
        let mut rogue = Self::bare(stand, name, session);
        let cookie = stand.world(HOST).resource::<NetHost>().cookie_for(rogue.transport.id(), session, stand.hub.now());
        rogue.link.set_cookie(cookie);
        rogue
    }

    /// A peer that never knocked: no cookie.
    fn bare(stand: &Stand, name: &str, session: u32) -> Self {
        let transport = stand.hub.endpoint(name);
        stand.hub.connect_both(0, transport.id(), LinkConditions::NONE);
        Self { transport, link: Link::new(session, Role::Client), got: Vec::new(), deltas: Vec::new() }
    }

    fn say(&mut self, message: &Msg) {
        self.link.send_reliable(&msg::encode(message)).unwrap();
    }

    fn say_unreliable(&mut self, bytes: &[u8]) {
        self.link.send_unreliable(bytes).unwrap();
    }

    /// Any bytes as a reliable message.
    fn say_raw(&mut self, bytes: &[u8]) {
        self.link.send_reliable(bytes).unwrap();
    }

    /// Sends what is queued into the void: the link counts it as sent, and sends it again only
    /// after its timeout.
    fn lose_queued(&mut self, stand: &Stand) {
        self.link.flush(stand.hub.now(), &mut |_| {});
    }

    /// Sends what is queued and takes in nothing: a peer that never acknowledges.
    fn send_only(&mut self, stand: &Stand) {
        let now = stand.hub.now();
        let Self { transport, link, .. } = self;
        link.flush(now, &mut |datagram| transport.send(0, datagram));
    }

    /// Takes what arrived, sends what is queued.
    fn pump(&mut self, stand: &Stand) {
        let now = stand.hub.now();
        let mut datagrams = Vec::new();
        self.transport.recv(now, &mut datagrams);
        for (_, datagram) in datagrams {
            self.link.on_datagram(now, &datagram);
        }
        for delivered in self.link.poll(now) {
            match delivered {
                Delivered::Reliable(bytes) => self.got.extend(msg::decode::<Msg>(&bytes).ok()),
                Delivered::Unreliable(bytes) => self.deltas.extend(msg::decode::<StateDelta>(&bytes).ok()),
            }
        }
        let Self { transport, link, .. } = self;
        link.flush(now, &mut |datagram| transport.send(0, datagram));
    }

    /// `ticks` stand ticks, talking between them.
    fn run(&mut self, stand: &mut Stand, ticks: u64) {
        for _ in 0..ticks {
            self.pump(stand);
            stand.run(1);
        }
        self.pump(stand);
    }

    /// Stand ticks until it got a message `done` matches (at most `max`).
    fn wait_for(&mut self, stand: &mut Stand, max: u64, what: &str, done: impl Fn(&Msg) -> bool) -> Msg {
        for _ in 0..max {
            if let Some(found) = self.got.iter().find(|m| done(m)) {
                return found.clone();
            }
            self.run(stand, 1);
        }
        panic!("{what}: not after {max} stand ticks; got {:?}", self.got.iter().map(short).collect::<Vec<_>>());
    }

    /// Hello, Ready: in the session as player N with a body.
    fn join(&mut self, stand: &mut Stand, name: &str) -> Welcome {
        self.say(&Msg::Hello(hello(stand, name)));
        self.wait_for(stand, 60, "accepted", |m| matches!(m, Msg::Accept(_)));
        self.say(&Msg::Ready { checks: BTreeMap::new() });
        match self.wait_for(stand, 60, "welcomed", |m| matches!(m, Msg::Welcome(_))) {
            Msg::Welcome(welcome) => welcome,
            _ => unreachable!(),
        }
    }

    fn refused(&self) -> Option<String> {
        self.got.iter().find_map(|m| match m {
            Msg::Refused { reason } => Some(reason.clone()),
            _ => None,
        })
    }
}

fn short(message: &Msg) -> String {
    let text = format!("{message:?}");
    text.chars().take(80).collect()
}

/// The `Hello` an honest client of this stand sends.
fn hello(stand: &Stand, name: &str) -> Hello {
    let world = stand.world(HOST);
    Hello {
        proto: msg::PROTO,
        engine: snapshot::ENGINE_VERSION.to_owned(),
        game: world.resource::<NetGame>().0.clone(),
        content_hash: world.resource::<ContentDb>().hash(),
        net_rules_hash: net_rules::rules_hash(world),
        name: name.to_owned(),
        token: String::new(),
        character: None,
        tick_hz: 1.0 / sim::tick_seconds(world) as f64,
    }
}

fn host_lines(stand: &Stand, needle: &str) -> Vec<String> {
    journal(stand, HOST).into_iter().filter(|l| l.contains(needle)).collect()
}

// --- calls ---------------------------------------------------------------------------------

#[test]
fn a_guests_call_acts_as_its_own_body_only() {
    let mut stand = stand(1);
    let me = body(&stand, C1);
    // A game that runs `move_to` on the host, and a modified client that calls it as the
    // host's own player.
    for member in [HOST, C1] {
        stand.world_mut(member).resource_mut::<AiTools>().set_route("move_to", NetRoute::Host).unwrap();
    }
    let theirs = SessionChange { subject: AiId::parse(PLAYER), ..Default::default() };
    configure_session(stand.world_mut(C1), "dev", &theirs).unwrap();
    let host_at = position(&stand, HOST, PLAYER);
    let sent = stand.call(C1, "move_to", json!({"target": {"x": host_at.x + 10.0, "z": host_at.z}}));
    assert!(sent.ok, "{}", sent.result);
    stand.run(40);
    let line = journal(&stand, C1).into_iter().find(|l| l.contains("host: move_to failed at host tick")).unwrap_or_else(|| panic!("{:#?}", journal(&stand, C1)));
    assert!(line.contains(&format!("your calls act as your own body ({me}), not as {PLAYER}")), "{line}");
    assert!(host_calls(&stand, "move_to").is_empty(), "nothing ran on the host");
    let theirs = entity(&stand, HOST, PLAYER).unwrap();
    assert!(stand.world(HOST).get::<Intent>(theirs).unwrap().orders.is_empty(), "the host's player got no order");

    // As its own body, the same call runs there.
    configure_session(stand.world_mut(C1), "dev", &SessionChange { clear_subject: true, ..Default::default() }).unwrap();
    assert!(stand.call(C1, "move_to", json!({"target": {"x": host_at.x + 10.0, "z": host_at.z}})).ok);
    stand.run(40);
    assert_eq!(host_calls(&stand, "move_to").len(), 1);
}

#[test]
fn a_host_runs_only_what_a_client_forwards_and_only_so_much() {
    let mut stand = stand(1);
    let me = body(&stand, C1);
    // `stop` runs on the client's own game (`NetRoute::Local`); a modified client sends it.
    stand.world_mut(C1).resource_mut::<AiTools>().set_route("stop", NetRoute::Host).unwrap();
    assert!(stand.call(C1, "stop", json!({})).ok);
    stand.run(20);
    assert!(has_line(&stand, C1, "host: stop failed at host tick"), "{:#?}", journal(&stand, C1));
    assert!(has_line(&stand, C1, "`stop` runs on your own game, not on the host"), "{:#?}", journal(&stand, C1));
    assert!(host_calls(&stand, "stop").is_empty());

    // A new agent name per call: at most 8 agents per player on the host.
    for i in 0..12 {
        assert!(stand.call_as(C1, &format!("a{i}"), "teleport", json!({"id": me, "x": 1.0, "z": 1.0})).ok);
        stand.run(2);
    }
    stand.run(20);
    let sessions = stand.world(HOST).resource::<AgentSessions>();
    assert!(sessions.get("p1/a7").is_some(), "{:?}", sessions.names());
    assert!(sessions.get("p1/a8").is_none(), "{:?}", sessions.names());
    let capped = journal(&stand, C1).into_iter().filter(|l| l.contains("at most 8 agents per player call the host")).count();
    assert_eq!(capped, 4, "{:#?}", journal(&stand, C1));

    // Twenty calls at once: the host takes 16 a tick and says so for the rest.
    for _ in 0..20 {
        assert!(stand.call_as(C1, "a0", "teleport", json!({"id": me, "x": 1.0, "z": 1.0})).ok);
    }
    stand.run(20);
    let busy = journal(&stand, C1).into_iter().filter(|l| l.contains("too many calls at once (at most 16 a tick)")).count();
    assert_eq!(busy, 4, "{:#?}", journal(&stand, C1));
}

// --- desync reports ------------------------------------------------------------------------

#[test]
fn desync_reports_count_only_for_what_the_host_sent_and_resync_at_most_every_5_s() {
    let mut stand = stand(1);
    // A stranger, never joined, says it saw a desync.
    let mut stranger = Rogue::new(&stand, "stranger", 77);
    stranger.say(&Msg::Desync { tick: 0, here: 1, host: 2, first: "x".into(), epoch: 0 });
    stranger.run(&mut stand, 10);
    assert_eq!(stand.world(HOST).resource::<NetHost>().desyncs, 0);
    assert!(host_lines(&stand, "desync").is_empty(), "{:#?}", journal(&stand, HOST));

    // A guest quotes hashes the host really sent, again and again, with a long first line.
    let mut guest = Rogue::new(&stand, "guest", 79);
    guest.join(&mut stand, "guest");
    guest.run(&mut stand, 70);
    let (tick, hash, epoch) = guest.deltas.iter().rev().find_map(|d| d.hash.map(|h| (d.tick, h, d.epoch))).expect("a delta with a hash");
    let first = format!("{}\nnet: forged line", "x".repeat(100_000));
    for _ in 0..50 {
        guest.say(&Msg::Desync { tick, here: 1, host: hash, first: first.clone(), epoch });
    }
    // One with a hash the host never sent.
    guest.say(&Msg::Desync { tick, here: 1, host: hash ^ 1, first: "made up".into(), epoch });
    guest.run(&mut stand, 10);
    let host = stand.world(HOST).resource::<NetHost>();
    assert_eq!(host.desyncs, 1, "one report per hash the host sent");
    let lines = host_lines(&stand, "desync at tick");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].len() < 400 && !lines[0].contains('\n'), "clipped: {} bytes", lines[0].len());
    let rebases = |r: &Rogue| r.got.iter().filter(|m| matches!(m, Msg::Rebase(_))).count();
    assert_eq!(rebases(&guest), 1, "the world once");
    // The next hash a second later: counted, but no resync within 5 s of the last.
    guest.run(&mut stand, 70);
    let (tick, hash, epoch) = guest.deltas.iter().rev().find_map(|d| d.hash.map(|h| (d.tick, h, d.epoch))).unwrap();
    guest.say(&Msg::Desync { tick, here: 1, host: hash, first: "again".into(), epoch });
    guest.run(&mut stand, 10);
    assert_eq!(stand.world(HOST).resource::<NetHost>().desyncs, 2);
    assert_eq!(rebases(&guest), 1, "no second resync within 5 s");
}

#[test]
fn a_hosts_agent_can_stop_on_a_clients_desync() {
    let mut stand = stand(1);
    stand.run(30);
    let boss = entity(&stand, C1, BOSS).unwrap();
    stand.world_mut(C1).get_mut::<Health>(boss).unwrap().current = 1.0;
    let from = sim::tick(stand.world(HOST));
    let out = stand.call(HOST, "step", json!({"ticks": 300, "until": [{"named": "net_desync"}]}));
    assert!(out.ok, "{}", out.result);
    let at = sim::tick(stand.world(HOST));
    assert!(at < from + 120, "stopped at the report (ticks {from} → {at}): {}", out.result);
    assert_eq!(stand.world(HOST).resource::<NetHost>().desyncs, 1);
}

// --- hellos --------------------------------------------------------------------------------

#[test]
fn a_hello_out_of_bounds_is_refused_and_its_text_never_reaches_the_journal() {
    let mut stand = stand(0);
    let mut forged = Rogue::new(&stand, "forged", 81);
    forged.say(&Msg::Hello(hello(&stand, "evil\nnet: p0 (host) left (quit)")));
    forged.run(&mut stand, 5);
    let reason = forged.refused().unwrap_or_else(|| panic!("{:?}", forged.got));
    assert!(reason.contains("a player name is 1 to 32 characters"), "{reason}");

    stand.run(70);
    let mut long = Rogue::new(&stand, "long", 82);
    let mut bogus = hello(&stand, "long");
    bogus.engine = format!("9.9\nnet: forged {}", "x".repeat(10_000));
    long.say(&Msg::Hello(bogus));
    long.run(&mut stand, 5);
    let reason = long.refused().unwrap();
    assert!(reason.starts_with("engine version differs") && reason.len() < 300, "{reason}");

    stand.run(70);
    let mut heavy = Rogue::new(&stand, "heavy", 83);
    let mut big = hello(&stand, "heavy");
    // Over the 64 KB of a character, under the 80 KB the host reads before it plays.
    big.character = Some(json!({"bag": "x".repeat(70_000)}));
    heavy.say(&Msg::Hello(big));
    heavy.run(&mut stand, 5);
    assert!(heavy.refused().unwrap().contains("the character is over 64 KB"), "{:?}", heavy.refused());
    // What a peer sent reaches the journal clipped and escaped, never as a line of its own.
    let lines = journal(&stand, HOST);
    assert!(lines.iter().all(|l| !l.contains("evil") && !l.contains('\n')), "a peer's text in the journal: {lines:#?}");
    assert!(!lines.iter().any(|l| l.starts_with("net: forged") || l.contains("p0 (host) left")), "{lines:#?}");
    let refused: Vec<&String> = lines.iter().filter(|l| l.contains("refused")).collect();
    assert_eq!(refused.len(), 3, "{lines:#?}");
    assert!(refused.iter().all(|l| l.len() < 300), "{refused:#?}");
    assert!(stand.world(HOST).resource::<NetHost>().slots.is_empty(), "nobody joined");
}

#[test]
fn refusals_take_one_journal_line_a_second() {
    let mut stand = stand(0);
    let mut rogues: Vec<Rogue> = (0..5).map(|i| Rogue::new(&stand, &format!("r{i}"), 91 + 2 * i)).collect();
    for rogue in &mut rogues {
        let mut old = hello(&stand, "old");
        old.engine = "0.0.1".into();
        rogue.say(&Msg::Hello(old));
        rogue.pump(&stand);
    }
    stand.run(5);
    for rogue in &mut rogues {
        rogue.pump(&stand);
        assert!(rogue.refused().is_some_and(|r| r.contains("engine version differs: host")), "{:?}", rogue.got);
    }
    assert_eq!(host_lines(&stand, "net: refused").len(), 1, "{:#?}", journal(&stand, HOST));
}

#[test]
fn a_hello_of_another_build_is_refused_saying_what_differs() {
    let mut stand = stand(0);
    let mut other = Rogue::new(&stand, "other", 84);
    let mut changed = hello(&stand, "other");
    changed.content_hash ^= 1;
    other.say(&Msg::Hello(changed));
    other.run(&mut stand, 5);
    assert!(other.refused().is_some_and(|r| r.starts_with("content differs: host ")), "{:?}", other.got);
    let mut slow = Rogue::new(&stand, "slow", 85);
    let mut changed = hello(&stand, "slow");
    changed.tick_hz = 30.0;
    slow.say(&Msg::Hello(changed));
    slow.run(&mut stand, 5);
    assert!(slow.refused().is_some_and(|r| r.contains("start with --hz 60")), "{:?}", slow.got);
}

// --- the join's cookie ---------------------------------------------------------------------

#[test]
fn a_stranger_without_the_hosts_cookie_for_its_address_is_never_a_peer() {
    let mut stand = stand(0);
    // One that never knocked, one with another address's cookie (a spoofed sender), one with a
    // made-up cookie: each says hello, again and again, for two seconds.
    let mut bare = Rogue::bare(&stand, "bare", 3001);
    let mut spoofed = Rogue::bare(&stand, "spoofed", 3003);
    let theirs = stand.world(HOST).resource::<NetHost>().cookie_for(spoofed.transport.id() + 1, 3003, stand.hub.now());
    spoofed.link.set_cookie(theirs);
    let mut forged = Rogue::bare(&stand, "forged", 3005);
    forged.link.set_cookie(0x1234_5678_9ABC_DEF0);
    for rogue in [&mut bare, &mut spoofed, &mut forged] {
        rogue.say(&Msg::Hello(hello(&stand, "rogue")));
    }
    for _ in 0..120 {
        for rogue in [&mut bare, &mut spoofed, &mut forged] {
            rogue.pump(&stand);
        }
        stand.run(1);
    }
    let host = stand.world(HOST).resource::<NetHost>();
    assert!(host.peers.is_empty(), "{:?}", host.peers.keys().collect::<Vec<_>>());
    assert!(host.cookieless >= 3, "{}", host.cookieless);
    assert!([&bare, &spoofed, &forged].iter().all(|r| r.got.is_empty()), "nothing is said to them");
    assert!(host_lines(&stand, "refused").is_empty(), "{:#?}", journal(&stand, HOST));
    // With the cookie of its own address, the same hello is read at once.
    let mut honest = Rogue::new(&stand, "honest", 3007);
    honest.say(&Msg::Hello(hello(&stand, "honest")));
    honest.wait_for(&mut stand, 30, "accepted", |m| matches!(m, Msg::Accept(_)));
}

#[test]
fn a_client_knocks_before_its_hello_and_knocks_cost_the_host_nothing_it_keeps() {
    let mut stand = stand(1);
    let status = stand.call(HOST, "net_status", json!({}));
    assert!(status.result["strangers"]["knocks"].as_u64().is_some_and(|n| n >= 1), "{}", status.result["strangers"]);
    assert_eq!(status.result["strangers"]["cookieless"], json!(0), "the client said nothing before its cookie came");
    // A hundred addresses knock in one tick: each is answered, none is kept.
    let mut knockers: Vec<MemoryTransport> = (0..100)
        .map(|i| {
            let t = stand.hub.endpoint(&format!("k{i}"));
            stand.hub.connect_both(0, t.id(), LinkConditions::NONE);
            t
        })
        .collect();
    for (i, knocker) in knockers.iter_mut().enumerate() {
        knocker.send(0, &wire::knock(4000 + i as u32));
    }
    stand.run(1);
    let host = stand.world(HOST).resource::<NetHost>();
    assert_eq!(host.peers.len(), 1, "only the player");
    assert!(host.knocks >= 101);
    for knocker in &mut knockers {
        let mut got = Vec::new();
        knocker.recv(stand.hub.now(), &mut got);
        assert!(matches!(got.first().and_then(|(_, d)| wire::handshake(d)), Some(wire::Handshake::Challenge { .. })), "{got:?}");
    }
    assert!(connected_peers(&stand) == 1, "the player plays on");
}

#[test]
fn a_game_of_the_build_before_the_cookie_is_told_to_update_and_kept_nowhere() {
    let mut stand = stand(0);
    // A client of wire protocol 1: its hello without a knock, in the datagrams of then.
    let mut old = Rogue::bare(&stand, "old", 3101);
    old.say(&Msg::Hello(hello(&stand, "old")));
    for _ in 0..10 {
        let now = stand.hub.now();
        let Rogue { transport, link, got, .. } = &mut old;
        let mut datagrams = Vec::new();
        transport.recv(now, &mut datagrams);
        for (_, datagram) in datagrams {
            assert_eq!(datagram[2], wire::PROTO_1, "the host answers in its protocol");
            link.on_datagram(now, &wire::as_version(&datagram, wire::PROTO));
        }
        for delivered in link.poll(now) {
            if let Delivered::Reliable(bytes) = delivered {
                got.extend(msg::decode::<Msg>(&bytes).ok());
            }
        }
        link.flush(now, &mut |datagram| transport.send(0, &wire::as_version(datagram, wire::PROTO_1)));
        stand.run(1);
    }
    let reason = old.refused().unwrap_or_else(|| panic!("{:?}", old.got));
    assert!(reason.contains("wire protocol 2") && reason.contains("run the same build"), "{reason}");
    let host = stand.world(HOST).resource::<NetHost>();
    assert!(host.peers.is_empty() && host.older_builds >= 1);
    assert!(has_line(&stand, HOST, "a game of an older build (wire protocol 1)"), "{:#?}", journal(&stand, HOST));
}

#[test]
fn a_client_whose_knocks_go_unanswered_ends_its_join_saying_what_it_may_be() {
    let mut stand = stand(0);
    let deaf: LinkConditions = "loss=1".parse().unwrap();
    let c = shards::testing::stand_client(&mut stand, "lost", deaf, &configure, &|app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.insert_resource(NetDefaults(NetDef { timeout_secs: 1.0, ..Default::default() }));
    });
    run_until(&mut stand, 200, "the join ends", |s| matches!(net_state(s, c), NetState::Ended(_)));
    let NetState::Ended(why) = net_state(&stand, c) else { unreachable!() };
    assert!(why.starts_with("no answer from the host host to the hello in 1 s") && why.contains("a host of an older build does not answer it"), "{why}");
    assert!(stand.world(HOST).resource::<NetHost>().peers.is_empty());
}

// --- sizes and words -----------------------------------------------------------------------

#[test]
fn a_message_larger_than_an_honest_client_sends_is_not_read() {
    let mut stand = stand(0);
    // A stranger's hello of a megabyte: refused for its size, unread.
    let mut heavy = Rogue::new(&stand, "heavy", 101);
    let mut big = hello(&stand, "heavy");
    big.character = Some(json!({"bag": "x".repeat(1 << 20)}));
    heavy.say(&Msg::Hello(big));
    heavy.run(&mut stand, 10);
    let reason = heavy.refused().unwrap_or_else(|| panic!("{:?}", heavy.got.iter().map(short).collect::<Vec<_>>()));
    assert!(reason.contains("before you play is over the 80 KB the host reads"), "{reason}");

    // A guest's inputs of 600 KB: dropped unread; an honest packet is a few KB.
    let mut guest = Rogue::new(&stand, "guest", 103);
    guest.join(&mut stand, "guest");
    let tick = sim::tick(stand.world(HOST));
    let frame = InputFrame { tick: tick + 3, presses: vec![Press { input: "x".repeat(600 << 10), dir: None }], ..Default::default() };
    let bytes = msg::encode(&Inputs { tick: tick + 3, frames: vec![frame], state_ack: 0, epoch: 0 });
    guest.say_unreliable(&bytes);
    guest.run(&mut stand, 10);
    let line = format!("net: p1 sent a message of {} KB, over 512 KB: dropped unread", bytes.len().div_ceil(1024));
    assert!(has_line(&stand, HOST, &line), "{line}: {:#?}", journal(&stand, HOST));
    assert!(host_lines(&stand, "over the limits").is_empty(), "not decoded, so not checked: {:#?}", journal(&stand, HOST));
    assert!(connected_peers(&stand) == 1, "the guest plays on");
}

fn connected_peers(stand: &Stand) -> usize {
    stand.world(HOST).resource::<NetHost>().peers.values().filter(|p| p.stage == shards::net::host::PeerStage::Connected).count()
}

#[test]
fn a_peers_words_reach_every_journal_on_one_short_line() {
    let mut stand = stand(1);
    // A stranger's message that is no message (a newline escaped in JSON, 50 KB of name):
    // the decoder's error quotes its words.
    let mut stranger = Rogue::new(&stand, "stranger", 105);
    stranger.say_raw(format!("{{\"zz\\n123 [error] net: forged {}\":{{}}}}", "y".repeat(50_000)).as_bytes());
    stranger.run(&mut stand, 70);
    // A guest's order of a kind no game has, then its goodbye: words of its own.
    let mut guest = Rogue::new(&stand, "guest", 107);
    guest.join(&mut stand, "guest");
    let tick = sim::tick(stand.world(HOST));
    let mut inputs = shards::serde_json::to_value(Inputs { tick: tick + 3, frames: vec![InputFrame { tick: tick + 3, ..Default::default() }], state_ack: 0, epoch: 0 }).unwrap();
    let mut order = Map::new();
    order.insert(format!("zz\n123 [error] net: forged {}", "y".repeat(100_000)), json!({}));
    inputs["frames"][0]["orders"] = Value::Array(vec![Value::Object(order)]);
    guest.say_unreliable(&shards::serde_json::to_vec(&inputs).unwrap());
    guest.run(&mut stand, 10);
    guest.say(&Msg::Bye { reason: format!("bye\n123 [error] net: forged {}", "z".repeat(100_000)) });
    guest.run(&mut stand, 10);

    assert_eq!(host_lines(&stand, "net: peer").iter().filter(|l| l.contains("undecodable")).count(), 2, "{:#?}", journal(&stand, HOST));
    assert!(has_line(&stand, HOST, "net: p2 left (bye\\n123 [error] net: forged zzz"), "{:#?}", journal(&stand, HOST));
    for member in [HOST, C1] {
        let lines = journal(&stand, member);
        assert!(lines.iter().all(|l| !l.contains('\n') && l.len() < 400), "member {member}: {:#?}", lines.iter().map(|l| l.chars().take(120).collect::<String>()).collect::<Vec<_>>());
    }
    let left = host_calls(&stand, "player_leave");
    let reason = left.last().and_then(|(_, _, args)| args["reason"].as_str().map(str::to_owned)).unwrap_or_default();
    assert!(reason.starts_with("bye") && reason.len() < 300 && !reason.contains('\n'), "recorded: {} bytes", reason.len());
}

// --- inputs --------------------------------------------------------------------------------

#[test]
fn inputs_over_the_limits_are_dropped_and_strangers_are_not_read() {
    let mut stand = stand(0);
    let mut stranger = Rogue::new(&stand, "stranger", 86);
    stranger.say_unreliable(b"{not json");
    stranger.run(&mut stand, 5);
    assert!(host_lines(&stand, "undecodable").is_empty(), "a stranger's datagrams are not decoded: {:#?}", journal(&stand, HOST));

    let mut guest = Rogue::new(&stand, "guest", 87);
    let welcome = guest.join(&mut stand, "guest");
    let body = AiId(welcome.body.unwrap());
    let tick = sim::tick(stand.world(HOST));
    let press = Press { input: "light".into(), dir: None };
    let flood = InputFrame { tick: tick + 3, presses: vec![press; 1000], ..Default::default() };
    guest.say_unreliable(&msg::encode(&Inputs { tick: tick + 3, frames: vec![flood], state_ack: 0, epoch: 0 }));
    guest.run(&mut stand, 10);
    assert!(
        has_line(&stand, HOST, &format!("net: p1 sent inputs over the limits (frame {}: 1000 presses", tick + 3)),
        "{:#?}",
        journal(&stand, HOST)
    );
    let host = stand.world(HOST);
    let entity = host.resource::<AiIndex>().get(body).unwrap();
    assert!(host.get::<Intent>(entity).unwrap().pressed.is_empty());
    assert_eq!(host.resource::<NetHost>().peers.values().find(|p| p.player == 1).unwrap().inputs.newest, 0, "nothing taken");
}

#[test]
fn a_packet_tick_near_the_end_of_u64_is_a_large_margin_not_a_panic() {
    let mut stand = stand(0);
    let mut guest = Rogue::new(&stand, "guest", 89);
    guest.join(&mut stand, "guest");
    guest.run(&mut stand, 10);
    guest.say_unreliable(&msg::encode(&Inputs { tick: 1 << 63, frames: vec![], state_ack: 0, epoch: 0 }));
    guest.run(&mut stand, 10);
    let host = stand.world(HOST).resource::<NetHost>();
    let margin = host.peers.values().find(|p| p.player == 1).unwrap().margin;
    assert_eq!(margin.map(|m| m.0), Some(1 << 63));
    assert!(margin.unwrap().1 > 1 << 62, "{margin:?}");
    assert!(matches!(stand.world(HOST).resource::<NetState>(), NetState::Hosting), "the host plays on");
}

// --- the work a peer can make, and peers that stay ---------------------------------------

#[test]
fn what_a_stranger_sends_after_its_refusal_is_not_read() {
    let mut stand = stand(0);
    let mut flood = Rogue::new(&stand, "flood", 111);
    let mut old = hello(&stand, "old");
    old.engine = "0.0.1".into();
    flood.say(&Msg::Hello(old));
    // Behind the refused hello, in the same poll: none of it is decoded (an undecodable
    // message would be said in the journal).
    for _ in 0..20 {
        flood.say_raw(b"not json at all");
    }
    flood.run(&mut stand, 5);
    assert!(flood.refused().is_some_and(|r| r.contains("engine version differs")), "{:?}", flood.got);
    let said = |stand: &Stand| journal(stand, HOST).into_iter().filter(|l| l.contains("net: peer ")).count();
    assert_eq!(said(&stand), 0, "{:#?}", journal(&stand, HOST));
    // The same bytes from a stranger on its own are read and said (then it is refused: an
    // unreadable first message is no hello).
    stand.run(70);
    let mut junk = Rogue::new(&stand, "junk", 112);
    junk.say_raw(b"not json at all");
    junk.run(&mut stand, 5);
    assert_eq!(said(&stand), 1, "{:#?}", journal(&stand, HOST));
}

#[test]
fn a_guest_makes_the_host_decode_no_more_than_a_budget_in_one_poll() {
    let mut stand = stand(0);
    let mut guest = Rogue::new(&stand, "guest", 113);
    guest.join(&mut stand, "guest");
    // Each call is under the 1 MiB a player is read; together they are over what one poll decodes.
    let call = |id: u64| {
        Msg::Call(CallMsg { id, agent: "dev".into(), scope: Scope::Player, subject: None, tool: "net_status".into(), args: json!({"pad": "a".repeat(700_000)}) })
    };
    let (mut spent, mut expected) = (0usize, 0usize);
    for id in 1..=10 {
        let len = msg::encode(&call(id)).len();
        if spent + len <= PLAYER_POLL_BYTES {
            spent += len;
            expected += 1;
        }
        guest.say(&call(id));
    }
    assert!(expected < 10 && expected > 1, "{expected}");
    guest.run(&mut stand, 6);
    // Every call that is read gets its answer; the others were not read.
    let replies = guest.got.iter().filter(|m| matches!(m, Msg::CallReply { .. })).count();
    assert_eq!(replies, expected, "{:?}", guest.got.iter().map(short).collect::<Vec<_>>());
    let peer = stand.world(HOST).resource::<NetHost>().peers.values().next().unwrap();
    assert_eq!(peer.unread as usize, 10 - expected);
    assert!(!host_lines(&stand, "p1 sent over").is_empty(), "said: {:#?}", journal(&stand, HOST));
}

/// A session whose silence timeout is the shortest there is: a minute of joining becomes 6 s.
fn quick_timeout() -> NetDefaults {
    NetDefaults(NetDef { timeout_secs: 1.0, ..Default::default() })
}

#[test]
fn a_peer_that_never_says_hello_is_dropped_however_it_keeps_sending() {
    let mut stand = stand(0);
    stand.world_mut(HOST).insert_resource(quick_timeout());
    let mut stranger = Rogue::new(&stand, "stranger", 114);
    // One datagram with a message makes a peer on the host; it never says hello. It sends
    // keepalives, so its silence never times out.
    stranger.say_unreliable(b"hello?");
    stranger.run(&mut stand, 120);
    let id = stranger.transport.id();
    let stage = |stand: &Stand| stand.world(HOST).resource::<NetHost>().peers.get(&id).map(|p| p.stage.clone());
    assert_eq!(stage(&stand), Some(PeerStage::New));
    stranger.run(&mut stand, 300);
    assert!(stranger.refused().is_some_and(|r| r.contains("joining did not go on in 6 s")), "{:?}", stranger.got.iter().map(short).collect::<Vec<_>>());
    stranger.run(&mut stand, 20);
    assert_eq!(stage(&stand), None, "forgotten");
}

#[test]
fn net_kick_sends_away_a_peer_that_has_no_player_yet() {
    let mut stand = stand(0);
    let mut stranger = Rogue::new(&stand, "stranger", 115);
    stranger.say_unreliable(b"hello?");
    stranger.run(&mut stand, 5);
    let id = stranger.transport.id();
    // A player number reaches no one here; the peer number does.
    assert!(!stand.call(HOST, "net_kick", json!({"player": 0})).ok);
    assert!(!stand.call(HOST, "net_kick", json!({})).ok);
    assert!(!stand.call(HOST, "net_kick", json!({"player": 1, "peer": id})).ok);
    let status = stand.call(HOST, "net_status", json!({}));
    assert!(status.result["peers"].as_array().unwrap().iter().any(|p| p["peer"] == json!(id) && p["stage"] == json!("New")), "{}", status.result);
    let kicked = stand.call(HOST, "net_kick", json!({"peer": id, "reason": "testing"}));
    assert!(kicked.ok, "{}", kicked.result);
    stranger.run(&mut stand, 5);
    assert!(stranger.got.iter().any(|m| matches!(m, Msg::Bye { reason } if reason == "kicked: testing")), "{:?}", stranger.got.iter().map(short).collect::<Vec<_>>());
    stranger.run(&mut stand, 20);
    assert!(stand.world(HOST).resource::<NetHost>().peers.is_empty());
    assert!(!stand.call(HOST, "net_kick", json!({"peer": id})).ok, "already gone");
}

#[test]
fn a_peer_that_never_acknowledges_its_refusal_is_forgotten_all_the_same() {
    let mut stand = stand(0);
    let mut deaf = Rogue::new(&stand, "deaf", 116);
    let mut old = hello(&stand, "deaf");
    old.engine = "0.0.1".into();
    deaf.say(&Msg::Hello(old));
    // It sends (keepalives, the hello) but takes in nothing the host says, acknowledgements
    // included: the host's refusal stays unacknowledged for ever.
    for _ in 0..900 {
        let now = stand.hub.now();
        let Rogue { transport, link, .. } = &mut deaf;
        link.flush(now, &mut |datagram| transport.send(0, datagram));
        stand.run(1);
        if stand.world(HOST).resource::<NetHost>().peers.is_empty() {
            break;
        }
    }
    assert!(stand.world(HOST).resource::<NetHost>().peers.is_empty(), "still there after {} ms", stand.hub.now());
    assert!(stand.hub.now() >= 10_000, "not before its limit: {} ms", stand.hub.now());
}

// --- strangers: what they cost and how many ----------------------------------------------------

#[test]
fn a_stranger_whose_first_message_is_not_a_hello_is_refused_at_once() {
    let mut stand = stand(0);
    let mut ready = Rogue::new(&stand, "ready", 121);
    ready.say(&Msg::Ready { checks: BTreeMap::new() });
    ready.run(&mut stand, 5);
    assert!(ready.refused().is_some_and(|r| r.contains("a join starts with a hello")), "{:?}", ready.got.iter().map(short).collect::<Vec<_>>());
    let mut junk = Rogue::new(&stand, "junk", 122);
    junk.say_raw(b"not json at all");
    junk.run(&mut stand, 5);
    assert!(junk.refused().is_some_and(|r| r.contains("a join starts with a hello")), "{:?}", junk.got.iter().map(short).collect::<Vec<_>>());
    // The hello of another build is that too: it is not a stranger's junk, and the refusal says what to do.
    let mut other = Rogue::new(&stand, "other", 125);
    let mut skewed = json!(Msg::Hello(hello(&stand, "other")));
    skewed["Hello"].as_object_mut().expect("a hello").remove("tick_hz");
    other.say_raw(&msg::encode(&skewed));
    other.run(&mut stand, 5);
    let told = other.refused().expect("refused");
    assert!(told.contains("your first message does not decode") && told.contains("tick_hz") && told.contains("same build"), "{told}");
    assert!(told.contains("a join starts with a hello"), "and what a join starts with: {told}");
    // What decodes to something else is told only what a join starts with.
    let told = ready.refused().expect("refused");
    assert!(told.contains("a join starts with a hello") && !told.contains("does not decode"), "{told}");
    // A goodbye first is no offence, and a hello first is read.
    let mut bye = Rogue::new(&stand, "bye", 123);
    bye.say(&Msg::Bye { reason: "wrong number".into() });
    bye.run(&mut stand, 5);
    assert!(bye.refused().is_none(), "{:?}", bye.got.iter().map(short).collect::<Vec<_>>());
    let mut honest = Rogue::new(&stand, "honest", 124);
    honest.say(&Msg::Hello(hello(&stand, "honest")));
    honest.wait_for(&mut stand, 30, "accepted", |m| matches!(m, Msg::Accept(_)));
}

/// A `Hello` of a character of 60 KB: valid, and 60 KB to read.
fn heavy_hello(stand: &Stand, name: &str) -> Msg {
    let mut heavy = hello(stand, name);
    heavy.character = Some(json!({"bag": "x".repeat(60_000)}));
    Msg::Hello(heavy)
}

fn peer_with(stand: &Stand, id: shards::net::PeerId) -> &shards::net::host::Peer {
    stand.world(HOST).resource::<NetHost>().peers.get(&id).expect("the peer is there")
}

#[test]
fn a_stranger_that_is_not_refused_is_read_for_so_many_bytes_a_poll_and_no_more() {
    let mut stand = stand(0);
    let mut stranger = Rogue::new(&stand, "stranger", 131);
    // Five hellos at once: the first is the join, the rest repeat it (and are read all the same, up to a budget).
    let size = msg::encode(&heavy_hello(&stand, "stranger")).len();
    for _ in 0..5 {
        stranger.say(&heavy_hello(&stand, "stranger"));
    }
    stranger.run(&mut stand, 3);
    let read = JOINING_POLL_BYTES / size;
    assert!((2..5).contains(&read), "{read} of 5 fit in {JOINING_POLL_BYTES}");
    let peer = peer_with(&stand, stranger.transport.id());
    assert_eq!(peer.unread as usize, 5 - read, "the rest was dropped unread");
    assert_ne!(peer.stage, PeerStage::Gone, "a peer that joins and repeats itself is not an offence");
    let id = stranger.transport.id();
    assert!(host_lines(&stand, &format!("net: peer {id} sent over")).len() == 1, "{:#?}", journal(&stand, HOST));
}

#[test]
fn peers_that_do_not_play_yet_share_a_budget_and_each_takes_a_turn() {
    let mut stand = stand(0);
    stand.world_mut(HOST).insert_resource(NetDefaults(NetDef { max_players: 8, ..Default::default() }));
    let mut flooders: Vec<Rogue> = (0..3).map(|i| Rogue::new(&stand, &format!("f{i}"), 141 + 2 * i)).collect();
    let mut honest = Rogue::new(&stand, "honest", 150);
    honest.say(&Msg::Hello(hello(&stand, "honest")));
    // Three peers that join and then repeat themselves with 60 KB, every tick: more than the poll
    // of peers that do not play reads. The honest one is last in line: it is not starved.
    for _ in 0..12 {
        for (i, flooder) in flooders.iter_mut().enumerate() {
            for _ in 0..3 {
                flooder.say(&heavy_hello(&stand, &format!("f{i}")));
            }
            flooder.pump(&stand);
        }
        honest.pump(&stand);
        stand.run(1);
    }
    honest.pump(&stand);
    assert!(honest.got.iter().any(|m| matches!(m, Msg::Accept(_))), "the honest peer was not served: {:?}", honest.got.iter().map(short).collect::<Vec<_>>());
    let skipped: u64 = stand.world(HOST).resource::<NetHost>().peers.values().map(|p| p.skipped).sum();
    assert!(skipped > 0, "the budget they share ({SHARED_JOINING_POLL_BYTES} B) ran out");
}

#[test]
fn players_that_say_hello_in_one_tick_take_their_numbers_in_the_order_of_their_ids() {
    let mut stand = stand(0);
    stand.world_mut(HOST).insert_resource(NetDefaults(NetDef { max_players: 8, ..Default::default() }));
    // Polls that found nobody, or somebody, move nothing: a poll reads in the order of the ids
    // unless it left a peer unread.
    stand.run(5);
    let mut arrivals: Vec<Rogue> = (0..3).map(|i| Rogue::new(&stand, &format!("o{i}"), 400 + 2 * i)).collect();
    for (i, arrival) in arrivals.iter_mut().enumerate() {
        arrival.say(&Msg::Hello(hello(&stand, &format!("o{i}"))));
        arrival.pump(&stand);
    }
    stand.run(2);
    let numbers: Vec<Option<u8>> = arrivals
        .iter_mut()
        .map(|arrival| {
            arrival.pump(&stand);
            arrival.got.iter().find_map(|m| if let Msg::Accept(a) = m { Some(a.player) } else { None })
        })
        .collect();
    assert_eq!(numbers, vec![Some(1), Some(2), Some(3)], "{:#?}", journal(&stand, HOST));
}

#[test]
fn the_host_holds_so_many_strangers_and_the_oldest_makes_room() {
    let mut stand = stand(0);
    let cap = max_strangers(NetDef::default().max_players);
    let mut strangers: Vec<Rogue> = (0..cap + 2).map(|i| Rogue::new(&stand, &format!("s{i}"), 200 + 2 * i as u32)).collect();
    // One a tick, so which is oldest is plain: each makes a peer that never says hello.
    for stranger in &mut strangers {
        stranger.say_unreliable(b"hello?");
        stranger.pump(&stand);
        stand.run(1);
    }
    let held = |stand: &Stand, r: &Rogue| stand.world(HOST).resource::<NetHost>().peers.contains_key(&r.transport.id());
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.len(), cap, "no more than {cap} strangers at once");
    assert!(!held(&stand, &strangers[0]) && !held(&stand, &strangers[1]), "the oldest made room");
    assert!(held(&stand, &strangers[2]) && held(&stand, &strangers[cap + 1]));
    // A player joining finds room in the same way, and joins.
    let mut honest = Rogue::new(&stand, "honest", 300);
    honest.join(&mut stand, "honest");
    assert!(stand.world(HOST).resource::<NetHost>().peers.len() <= cap + 1);
}

/// A refused stranger: its first message is not a hello. It does not acknowledge the refusal, so
/// the host holds it as gone for a while (it is the stranger the host would like to forget first).
fn refused_stranger(stand: &Stand, name: &str, session: u32) -> Rogue {
    let mut stranger = Rogue::new(stand, name, session);
    stranger.say(&Msg::Ready { checks: BTreeMap::new() });
    stranger.pump(stand);
    stranger
}

fn held(stand: &Stand, r: &Rogue) -> bool {
    stand.world(HOST).resource::<NetHost>().peers.contains_key(&r.transport.id())
}

#[test]
fn a_hello_is_not_lost_to_the_strangers_that_arrive_with_it() {
    let mut stand = stand(0);
    let cap = max_strangers(NetDef::default().max_players);
    // A table full of refused strangers.
    let refused: Vec<Rogue> = (0..cap).map(|i| refused_stranger(&stand, &format!("r{i}"), 500 + 2 * i as u32)).collect();
    stand.run(2);
    let peers = |stand: &Stand| stand.world(HOST).resource::<NetHost>().peers.values().filter(|p| p.stage == PeerStage::Gone).count();
    assert_eq!(peers(&stand), cap, "the table is full of peers that are gone");
    assert!(refused.iter().all(|r| held(&stand, r)));
    // One tick: fresh strangers, an honest hello among them, more fresh strangers after it. Each
    // of them finds the table full: none may take the place of one that has not had its turn.
    let mut honest = Rogue::new(&stand, "honest", 560);
    let mut junk: Vec<Rogue> = (0..5).map(|i| Rogue::new(&stand, &format!("j{i}"), 570 + 2 * i as u32)).collect();
    for fresh in &mut junk[..2] {
        fresh.say(&Msg::Ready { checks: BTreeMap::new() });
        fresh.pump(&stand);
    }
    honest.say(&Msg::Hello(hello(&stand, "honest")));
    honest.pump(&stand);
    for fresh in &mut junk[2..4] {
        fresh.say(&Msg::Ready { checks: BTreeMap::new() });
        fresh.pump(&stand);
    }
    stand.run(1);
    // And another one in the next tick, before the honest client has been answered anything.
    junk[4].say(&Msg::Ready { checks: BTreeMap::new() });
    junk[4].pump(&stand);
    stand.run(1);
    // Well within the time its link waits to send the hello again: it was not lost.
    honest.wait_for(&mut stand, 20, "accepted", |m| matches!(m, Msg::Accept(_)));
}

#[test]
fn a_burst_of_strangers_in_one_tick_never_fills_more_than_the_cap_and_spares_the_first_few() {
    let mut stand = stand(0);
    let def = NetDef::default();
    let cap = max_strangers(def.max_players);
    let mut strangers: Vec<Rogue> = (0..cap + 3).map(|i| Rogue::new(&stand, &format!("b{i}"), 700 + 2 * i as u32)).collect();
    for stranger in &mut strangers {
        stranger.say_unreliable(b"hello?");
        stranger.pump(&stand);
    }
    stand.run(1);
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.len(), cap, "no more than {cap} strangers at once");
    // The first `max_players` to come are the seats a hello could still take: those after them did
    // not take their places before a poll read them. The rest of the table turns over.
    let seats = usize::from(def.max_players);
    assert!(strangers[..seats].iter().all(|s| held(&stand, s)), "the first {seats} are held");
    assert!(strangers[seats..cap].iter().any(|s| !held(&stand, s)), "the others made room for the ones after them");
    assert!(strangers[cap..].iter().all(|s| held(&stand, s)), "the last to come are held");
    // The next tick they have had their turn, and said nothing: the oldest makes room.
    let mut late = Rogue::new(&stand, "late", 800);
    late.say_unreliable(b"hello?");
    late.pump(&stand);
    stand.run(1);
    assert!(held(&stand, &late) && !held(&stand, &strangers[0]));
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.len(), cap);
}

#[test]
fn a_message_ahead_of_a_gap_pins_no_seat_and_a_hello_still_gets_in() {
    let mut stand = stand(0);
    let cap = max_strangers(NetDef::default().max_players);
    // Strangers whose one datagram holds message number 1: number 0 never comes, so nothing is
    // delivered or refused, and the host would have to wait out the silence to be rid of them.
    let mut gappy: Vec<Rogue> = (0..cap).map(|i| Rogue::new(&stand, &format!("g{i}"), 1300 + 2 * i as u32)).collect();
    for stranger in &mut gappy {
        stranger.say(&Msg::Ready { checks: BTreeMap::new() });
        stranger.lose_queued(&stand);
        stranger.say(&Msg::Ready { checks: BTreeMap::new() });
        stranger.send_only(&stand);
    }
    stand.run(2);
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.len(), cap, "the table is full of them");
    let footprint = peer_with(&stand, gappy[0].transport.id()).link.footprint();
    assert_eq!((footprint.reliable_in_buffered, footprint.partial_blobs, footprint.undelivered), (1, 0, 0), "{footprint:?}");
    // An honest hello finds a seat all the same, at once and not by luck.
    let mut honest = Rogue::new(&stand, "honest", 1390);
    honest.say(&Msg::Hello(hello(&stand, "honest")));
    honest.wait_for(&mut stand, 20, "accepted", |m| matches!(m, Msg::Accept(_)));
}

/// A stranger that sent 32 of the pieces of a Hello of 70 KB, and no more.
fn part_of_a_hello(stand: &Stand, name: &str, session: u32) -> Rogue {
    let mut stranger = Rogue::new(stand, name, session);
    let mut big = hello(stand, name);
    big.character = Some(json!({"bag": letters(70_000)}));
    stranger.say(&Msg::Hello(big));
    stranger.pump(stand);
    stranger
}

#[test]
fn a_hello_in_pieces_is_spared_before_messages_that_can_never_be_a_hello() {
    let mut stand = stand(0);
    let def = NetDef::default();
    let cap = max_strangers(def.max_players);
    // As many strangers as a session has players, each with one message ahead of a gap: they come first.
    let gappy: Vec<Rogue> = (0..def.max_players)
        .map(|i| {
            let mut stranger = Rogue::new(&stand, &format!("g{i}"), 2000 + 2 * u32::from(i));
            stranger.say(&Msg::Ready { checks: BTreeMap::new() });
            stranger.lose_queued(&stand);
            stranger.say(&Msg::Ready { checks: BTreeMap::new() });
            stranger.send_only(&stand);
            stranger
        })
        .collect();
    stand.run(1);
    // Then a Hello part-way in, and the rest of the table with strangers that say nothing.
    let slow = part_of_a_hello(&stand, "slow", 2100);
    stand.run(1);
    let quiet: Vec<Rogue> = (0..cap - usize::from(def.max_players) - 1)
        .map(|i| {
            let mut stranger = Rogue::new(&stand, &format!("q{i}"), 2200 + 2 * i as u32);
            stranger.say_unreliable(b"hello?");
            stranger.pump(&stand);
            stranger
        })
        .collect();
    stand.run(2);
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.len(), cap);
    // One more takes the place of the oldest that is not spared: a message ahead of a gap is no hello
    // coming, so it is not the pieces of one that make way.
    let mut more = Rogue::new(&stand, "more", 2300);
    more.say_unreliable(b"hello?");
    more.pump(&stand);
    stand.run(1);
    assert!(held(&stand, &slow) && held(&stand, &more), "the hello in pieces is spared");
    assert!(!held(&stand, &gappy[0]), "and a message ahead of a gap made room");
    assert!(quiet.iter().all(|q| held(&stand, q)));
}

#[test]
fn strangers_holding_part_of_a_hello_pin_no_more_seats_than_a_session_has_players() {
    let mut stand = stand(0);
    let def = NetDef::default();
    let cap = max_strangers(def.max_players);
    for round in 0..2u32 {
        // A table full of Hellos part-way in, and nothing more of them to come.
        let strangers: Vec<Rogue> = (0..cap).map(|i| part_of_a_hello(&stand, &format!("p{round}_{i}"), 1400 + 100 * round + 2 * i as u32)).collect();
        stand.run(2);
        let host = stand.world(HOST).resource::<NetHost>();
        assert!(host.peers.values().filter(|p| p.link.footprint().partial_blobs == 1).count() >= usize::from(def.max_players), "pieces are in");
        assert!(strangers.iter().filter(|s| held(&stand, s)).count() >= usize::from(def.max_players));
        // An honest hello (one datagram) finds a seat: only the oldest `max_players` are spared.
        let mut honest = Rogue::new(&stand, &format!("honest{round}"), 1450 + 100 * round);
        honest.say(&Msg::Hello(hello(&stand, &format!("honest{round}"))));
        honest.wait_for(&mut stand, 20, &format!("accepted in round {round}"), |m| matches!(m, Msg::Accept(_)));
        // Past the time they are spared the same strangers are only ordinary peers, and the next
        // round takes their places again.
        stand.run(330);
    }
}

/// Letters that do not compress much: a message of this size travels in pieces.
fn letters(len: usize) -> String {
    let mut x = 0x2545_F491_4F6C_DD1Du64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b'a' + (x % 26) as u8)
        })
        .collect()
}

#[test]
fn a_hello_that_comes_in_pieces_is_held_for_a_while_and_then_makes_room_as_any_other() {
    let mut stand = stand(0);
    let cap = max_strangers(NetDef::default().max_players);
    // A hello with a 70 KB character: some 40 pieces, and the window lets 32 of them through.
    let mut slow = Rogue::new(&stand, "slow", 1000);
    let mut big = hello(&stand, "slow");
    big.character = Some(json!({"bag": letters(70_000)}));
    slow.say(&Msg::Hello(big));
    slow.pump(&stand);
    stand.run(1);
    let id = slow.transport.id();
    assert_eq!(peer_with(&stand, id).link.footprint().partial_blobs, 1, "its hello is part-way in");
    // The table fills with strangers that say nothing (all younger than it).
    let mut strangers: Vec<Rogue> = (1..cap).map(|i| Rogue::new(&stand, &format!("t{i}"), 1000 + 2 * i as u32)).collect();
    for stranger in &mut strangers {
        stranger.say_unreliable(b"hello?");
        stranger.pump(&stand);
    }
    stand.run(2);
    assert_eq!(stand.world(HOST).resource::<NetHost>().peers.len(), cap);
    // One more: it takes the place of a stranger that said nothing, not of the hello that is on its way.
    let mut one_more = Rogue::new(&stand, "more", 1100);
    one_more.say_unreliable(b"hello?");
    one_more.pump(&stand);
    stand.run(1);
    assert!(held(&stand, &slow) && held(&stand, &one_more) && !held(&stand, &strangers[0]), "the oldest that said nothing made room");
    // Its pieces never come: after a while it is a stranger like the others, and the oldest.
    stand.run(330);
    assert!(held(&stand, &slow), "silent for a few seconds, not yet for the ten the link allows");
    let mut last = Rogue::new(&stand, "last", 1200);
    last.say_unreliable(b"hello?");
    last.pump(&stand);
    stand.run(1);
    assert!(held(&stand, &last) && !held(&stand, &slow), "after its time it made room");
}

#[test]
fn a_table_full_of_players_that_were_put_out_and_still_owe_acknowledgements_makes_room_for_a_hello() {
    let mut stand = stand(0);
    let cap = max_strangers(NetDef::default().max_players);
    // One game that joins again and again with the token it was given, from a new address each
    // time: the game it had before is put out (gone, admitted), and takes in nothing of what the
    // host sends after, so its link is never idle and it is not forgotten for as long as it wants.
    let (mut token, mut squats) = (String::new(), Vec::new());
    for i in 0..=cap {
        let mut game = Rogue::new(&stand, &format!("q{i}"), 900 + 2 * i as u32);
        let mut hi = hello(&stand, "squatter");
        hi.token = token.clone();
        game.say(&Msg::Hello(hi));
        if let Msg::Accept(accept) = game.wait_for(&mut stand, 60, "accepted", |m| matches!(m, Msg::Accept(_))) {
            token = accept.token;
        }
        game.say(&Msg::Ready { checks: BTreeMap::new() });
        game.send_only(&stand);
        let id = game.transport.id();
        run_until(&mut stand, 30, &format!("game {i} is in"), |s| s.world(HOST).resource::<NetHost>().peers.get(&id).is_some_and(|p| p.stage == PeerStage::Connected));
        squats.push(game);
    }
    let gone = |stand: &Stand| stand.world(HOST).resource::<NetHost>().peers.values().filter(|p| p.stage == PeerStage::Gone).count();
    assert_eq!(gone(&stand), cap, "the table is full of games that were put out: {:#?}", journal(&stand, HOST));
    // A newcomer's hello is answered all the same: the oldest of them makes room.
    let mut honest = Rogue::new(&stand, "honest", 990);
    honest.say(&Msg::Hello(hello(&stand, "honest")));
    honest.wait_for(&mut stand, 20, "accepted", |m| matches!(m, Msg::Accept(_)));
    assert!(!held(&stand, &squats[0]), "the oldest made room");
}

#[test]
fn a_guest_put_out_between_ticks_is_the_last_a_full_table_forgets_not_the_first() {
    let mut stand = stand(0);
    let cap = max_strangers(NetDef::default().max_players);
    // A guest that has been playing for a while, and then games that were put out and take in nothing.
    let mut guest = Rogue::new(&stand, "guest", 1700);
    let welcome = guest.join(&mut stand, "guest");
    stand.run(20);
    let (mut token, mut squats) = (String::new(), Vec::new());
    for i in 0..=cap {
        let mut game = Rogue::new(&stand, &format!("s{i}"), 1800 + 2 * i as u32);
        let mut hi = hello(&stand, "squatter");
        hi.token = token.clone();
        game.say(&Msg::Hello(hi));
        if let Msg::Accept(accept) = game.wait_for(&mut stand, 60, "accepted", |m| matches!(m, Msg::Accept(_))) {
            token = accept.token;
        }
        game.say(&Msg::Ready { checks: BTreeMap::new() });
        game.send_only(&stand);
        let id = game.transport.id();
        run_until(&mut stand, 30, &format!("game {i} is in"), |s| s.world(HOST).resource::<NetHost>().peers.get(&id).is_some_and(|p| p.stage == PeerStage::Connected));
        squats.push(game);
    }
    // The operator puts the guest out between two ticks, and a stranger's datagram is the first thing
    // the next tick takes in: the table is full, and the only peers that could make room are gone.
    let kicked = stand.call(HOST, "net_kick", json!({"player": welcome.player, "reason": "testing"}));
    assert!(kicked.ok, "{}", kicked.result);
    let mut stranger = Rogue::new(&stand, "stranger", 1900);
    stranger.say_unreliable(b"hello?");
    stranger.pump(&stand);
    stand.run(1);
    // A game that was put out before it makes room, not the guest whose goodbye is still to be sent.
    guest.run(&mut stand, 5);
    assert!(guest.got.iter().any(|m| matches!(m, Msg::Bye { reason } if reason.contains("kicked: testing"))), "{:?}", guest.got.iter().map(short).collect::<Vec<_>>());
    assert!(guest.got.iter().any(|m| matches!(m, Msg::CharacterSave { .. })), "and its character: {:?}", guest.got.iter().map(short).collect::<Vec<_>>());
    assert!(!squats.iter().all(|s| held(&stand, s)), "one of the games made room");
    assert!(has_line(&stand, HOST, "forgot p2 (gone)"), "and the host said so: {:#?}", journal(&stand, HOST));
    assert!(!has_line(&stand, HOST, &format!("forgot p{} (gone)", welcome.player)), "{:#?}", journal(&stand, HOST));
}

#[test]
fn peers_over_their_budget_share_one_journal_line_a_second_and_one_key() {
    let mut stand = stand(0);
    stand.world_mut(HOST).insert_resource(NetDefaults(NetDef { max_players: 8, ..Default::default() }));
    let mut strangers: Vec<Rogue> = (0..3).map(|i| Rogue::new(&stand, &format!("g{i}"), 160 + 2 * i)).collect();
    for (i, stranger) in strangers.iter_mut().enumerate() {
        for _ in 0..5 {
            stranger.say(&heavy_hello(&stand, &format!("g{i}")));
        }
    }
    for stranger in &mut strangers {
        stranger.pump(&stand);
    }
    stand.run(1);
    // Three peers over budget in one poll: one line, not a line (and a key) for each.
    let lines = host_lines(&stand, "sent over");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("(and ") && lines[0].contains(" more)"), "{}", lines[0]);
    let limits = stand.world(HOST).resource::<shards::net::hooks::LineLimits>();
    assert!(limits.0.keys().all(|k| !k.starts_with("flood ")), "{:?}", limits.0.keys().collect::<Vec<_>>());
}
