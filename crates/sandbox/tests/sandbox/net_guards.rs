//! What a host takes from its peers: only what an honest client sends, within bounds. The
//! peers here are a real client whose routes or session were changed, and a [`Rogue`] that
//! speaks the protocol by hand.

use std::collections::BTreeMap;

use shards::ai_core::tools::{AiTools, NetRoute};
use shards::ai_core::{configure_session, net_rules, sim, snapshot, AgentSessions, AiIndex, SessionChange};
use shards::content::ContentDb;
use shards::core::Press;
use shards::net::inputs::InputFrame;
use shards::net::msg::{self, Hello, Inputs, Msg, StateDelta, Welcome};
use shards::net::{Delivered, Link, LinkConditions, MemoryTransport, NetGame, NetHost, NetState, Role, Stand, Transport};
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
    fn new(stand: &Stand, name: &str, session: u32) -> Self {
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
