//! The host's side of a session: its peers, the join handshake, remote inputs, state sends,
//! leaves and the end of the session.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use ai_core::journal::{self, JournalKind};
use ai_core::net_rules::{self, NetRules};
use ai_core::snapshot::{self, ENGINE_VERSION};
use ai_core::state::StateEncoder;
use ai_core::{call_tool, configure_session, sim, AiId, AiIndex, AiJournal, SessionChange, SimGate, WorldRebuilt};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::{player_body, Intent};

use crate::def::{net_def, NetDef};
use crate::hooks::{hooks, JoinDecision, JoinRequest, NetLeft};
use crate::inputs::InputQueue;
use crate::link::{Delivered, Link, Role, Verdict};
use crate::msg::{self, Accept, CallMsg, Hello, Inputs, Msg, Welcome, PROTO};
use crate::replicate::{base_view, build_delta, close_view, DeltaInput, EncodeCache, Encoded, SentView, ACK_PATIENCE, SENT_KEPT};
use crate::session::{NetEndpoint, NetGame, NetMarks, NetState, Ticked, NET_AGENT};
use crate::transport::PeerId;
use crate::wire;

/// How long a peer that is gone may stay silent with messages unacknowledged before it is
/// forgotten, milliseconds.
const GONE_PATIENCE_MS: u64 = 3000;

/// Where a peer is in the session.
#[derive(Clone, Debug, PartialEq)]
pub enum PeerStage {
    /// Heard from, no `Hello` yet.
    New,
    /// The game said "not now": asked again every tick.
    Waiting(String),
    /// Told the seed; building its world.
    Accepted,
    /// Built its world: admitted at the next chance.
    Ready,
    /// Playing.
    Connected,
    /// Left, refused or timed out: dropped after this tick.
    Gone,
}

/// One remote machine.
pub struct Peer {
    pub id: PeerId,
    pub link: Link,
    pub stage: PeerStage,
    pub hello: Option<Hello>,
    pub player: u8,
    /// The body's host id, once admitted.
    pub body: Option<u32>,
    pub view: BTreeSet<u32>,
    sent: VecDeque<(u64, Arc<SentView>)>,
    /// The newest state the client applied, and the host tick that ack arrived.
    acked: Option<u64>,
    acked_at: u64,
    pub inputs: InputQueue,
    /// `(client tick of the newest Inputs, its margin)`.
    pub margin: Option<(u64, i64)>,
    pub applied_input_tick: u64,
    pub desyncs: u32,
    pub resyncs: u32,
    /// The whole world goes to this peer at the next send, for this reason.
    pub rebase: Option<String>,
    pub joined_at: u64,
    /// Deltas sent, and their bytes.
    pub deltas: u64,
    pub delta_bytes: u64,
    pub held_back: u64,
    pub closure_capped: u64,
}

impl Peer {
    fn new(id: PeerId, session: u32) -> Self {
        Self {
            id,
            link: Link::new(session, Role::Host),
            stage: PeerStage::New,
            hello: None,
            player: 0,
            body: None,
            view: BTreeSet::new(),
            sent: VecDeque::new(),
            acked: None,
            acked_at: 0,
            inputs: InputQueue::default(),
            margin: None,
            applied_input_tick: 0,
            desyncs: 0,
            resyncs: 0,
            rebase: None,
            joined_at: 0,
            deltas: 0,
            delta_bytes: 0,
            held_back: 0,
            closure_capped: 0,
        }
    }

    fn send(&mut self, message: &Msg) {
        if let Err(e) = self.link.send_reliable(&msg::encode(message)) {
            warn!("net: cannot send to peer {}: {e}", self.id);
        }
    }

    fn name(&self) -> String {
        self.hello.as_ref().map_or_else(|| format!("peer {}", self.id), |h| h.name.clone())
    }
}

/// A slot of the session: who holds player number `n`.
#[derive(Clone, Debug, Default)]
pub struct Slot {
    pub token: String,
    pub name: String,
    pub peer: Option<PeerId>,
    pub left_at: Option<u64>,
}

/// What the host does at the next `ApplyCalls`.
enum Task {
    Admit(PeerId),
    Leave { player: u8, reason: String },
    Call { peer: PeerId, call: CallMsg },
}

/// The host's session.
#[derive(Resource)]
pub struct NetHost {
    pub peers: BTreeMap<PeerId, Peer>,
    pub slots: BTreeMap<u8, Slot>,
    cache: EncodeCache,
    journal_seq: u64,
    tasks: Vec<Task>,
    /// The world was rebuilt: every connected peer gets it again.
    rebase_all: Option<String>,
    /// Encode time of the last and the slowest send tick, microseconds.
    pub encode_us: (u64, u64),
    /// Encode time of every send tick, microseconds, and how many there were.
    pub encode_total: (u64, u64),
    /// Desyncs reported by clients.
    pub desyncs: u32,
    pub ended: Option<String>,
}

impl NetHost {
    pub fn new() -> Self {
        Self {
            peers: BTreeMap::new(),
            slots: BTreeMap::new(),
            cache: EncodeCache::default(),
            journal_seq: 0,
            tasks: Vec::new(),
            rebase_all: None,
            encode_us: (0, 0),
            encode_total: (0, 0),
            desyncs: 0,
            ended: None,
        }
    }

    pub fn connected(&self) -> impl Iterator<Item = &Peer> {
        self.peers.values().filter(|p| p.stage == PeerStage::Connected)
    }

    pub fn encode_counts(&self) -> (u64, u64) {
        (self.cache.encoded, self.cache.reused)
    }

    /// Asks for `player` to leave at the next tick (`net_kick`, a timeout).
    pub fn kick(&mut self, player: u8, reason: &str) -> Result<(), String> {
        let peer = self.peers.values_mut().find(|p| p.player == player && p.stage != PeerStage::Gone && p.stage != PeerStage::New);
        let Some(peer) = peer else {
            return Err(format!("no player {player} is connected; net_status lists them"));
        };
        peer.send(&Msg::Bye { reason: reason.to_owned() });
        let connected = peer.stage == PeerStage::Connected;
        peer.stage = PeerStage::Gone;
        if connected {
            self.tasks.push(Task::Leave { player, reason: reason.to_owned() });
        }
        Ok(())
    }

    /// Sends `peer` the whole world again at the next send.
    pub fn resync(&mut self, player: Option<u8>, reason: &str) -> usize {
        let mut n = 0;
        for peer in self.peers.values_mut().filter(|p| p.stage == PeerStage::Connected && player.is_none_or(|n| n == p.player)) {
            peer.rebase = Some(reason.to_owned());
            n += 1;
        }
        n
    }
}

impl Default for NetHost {
    fn default() -> Self {
        Self::new()
    }
}

/// Takes the host and the endpoint out of the world for `f`, and puts them back.
fn with_host(world: &mut World, f: impl FnOnce(&mut World, &mut NetHost, &mut NetEndpoint)) {
    let Some(mut host) = world.remove_resource::<NetHost>() else { return };
    let Some(mut endpoint) = world.remove_resource::<NetEndpoint>() else {
        world.insert_resource(host);
        return;
    };
    f(world, &mut host, &mut endpoint);
    world.insert_resource(endpoint);
    world.insert_resource(host);
}

// --- systems -----------------------------------------------------------------------------

/// `NetSystems::Receive`.
pub(crate) fn receive(world: &mut World) {
    world.resource_mut::<Ticked>().0 = true;
    with_host(world, io_in);
}

/// `NetSystems::ApplyCalls`: joins, leaves and forwarded calls, as recorded calls.
pub(crate) fn apply_calls(world: &mut World) {
    with_host(world, |world, host, _| run_tasks(world, host));
}

/// `NetSystems::ApplyInputs`: every frame stamped up to this tick, into the players' `Intent`.
pub(crate) fn apply_inputs(world: &mut World) {
    let Some(mut host) = world.remove_resource::<NetHost>() else { return };
    let tick = sim::tick(world);
    for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
        let due = peer.inputs.take_due(tick);
        let Some(entity) = peer.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id))) else { continue };
        if world.get::<NetLeft>(entity).is_some() {
            continue;
        }
        for frame in &due {
            if let Some(mut intent) = world.get_mut::<Intent>(entity) {
                crate::inputs::apply_frame(&mut intent, frame);
            }
            peer.applied_input_tick = peer.applied_input_tick.max(frame.tick);
        }
    }
    world.insert_resource(host);
}

/// `NetSystems::Send`: journal lines, rebases, state, then every link's datagrams.
pub(crate) fn send(world: &mut World) {
    with_host(world, |world, host, endpoint| {
        send_journal(world, host);
        send_rebases(world, host);
        send_state(world, host);
        flush(host, endpoint);
    });
}

/// `NetSystems::Pump`: a frame without a tick (a paused host) still admits and answers.
pub(crate) fn pump(world: &mut World) {
    with_host(world, |world, host, endpoint| {
        io_in(world, host, endpoint);
        run_tasks(world, host);
        send_journal(world, host);
        send_rebases(world, host);
        flush(host, endpoint);
    });
}

/// Observer: the world was rebuilt (a region change, a snapshot load): every client gets it
/// again once the world is out of its loading gate.
pub(crate) fn on_world_rebuilt(event: On<WorldRebuilt>, mut host: Option<ResMut<NetHost>>) {
    if let Some(host) = host.as_mut() {
        host.rebase_all = Some(event.event().reason.clone());
    }
}

/// Exit hook: the clients are told and get their characters; saving the world is the game's
/// (`NetHooks::on_session_end`).
pub(crate) fn on_exit(world: &mut World) {
    end_session(world, "the host quit");
}

/// Ends the session: characters to their players, `SessionEnd` to everyone, a last flush.
pub fn end_session(world: &mut World, reason: &str) {
    let on_end = hooks(world).on_session_end;
    on_end(world);
    with_host(world, |world, host, endpoint| {
        if host.ended.is_some() {
            return;
        }
        let tick = sim::tick(world);
        let capture = hooks(world).capture_character;
        for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
            if let Some(body) = peer.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id)))
                && let Some(sections) = capture(world, body)
            {
                peer.send(&Msg::CharacterSave { tick, reason: "session ended".into(), sections });
            }
            peer.send(&Msg::SessionEnd { reason: reason.to_owned(), saved_slot: None });
        }
        host.ended = Some(reason.to_owned());
        flush(host, endpoint);
        // Over UDP, give the datagrams half a second to get out and be acknowledged.
        if endpoint.clock.is_wall() {
            let started = std::time::Instant::now();
            while started.elapsed() < std::time::Duration::from_millis(500) && host.peers.values().any(|p| !p.link.is_idle()) {
                std::thread::sleep(std::time::Duration::from_millis(5));
                io_in_links(host, endpoint);
                flush(host, endpoint);
            }
        }
    });
    journal::emit(world, JournalKind::Sim, format!("net: session ended ({reason})"));
    world.insert_resource(NetState::Ended(reason.to_owned()));
}

// --- receiving -----------------------------------------------------------------------------

/// Datagrams into links, without handling what they deliver (the end of a session).
fn io_in_links(host: &mut NetHost, endpoint: &mut NetEndpoint) {
    let now = endpoint.clock.now();
    let mut datagrams = Vec::new();
    endpoint.transport.recv(now, &mut datagrams);
    for (from, bytes) in datagrams {
        if let Some(peer) = host.peers.get_mut(&from) {
            peer.link.on_datagram(now, &bytes);
        }
    }
}

fn io_in(world: &mut World, host: &mut NetHost, endpoint: &mut NetEndpoint) {
    let now = endpoint.clock.now();
    let mut datagrams = Vec::new();
    endpoint.transport.recv(now, &mut datagrams);
    for (from, bytes) in datagrams {
        if !host.peers.contains_key(&from) {
            if host.ended.is_some() {
                continue;
            }
            let Some(session) = wire::peek_session(&bytes) else { continue };
            host.peers.insert(from, Peer::new(from, session));
        }
        let peer = host.peers.get_mut(&from).expect("inserted above");
        if let Verdict::Rejected(reason) = peer.link.on_datagram(now, &bytes) {
            debug!("net: datagram from peer {from} rejected: {reason:?}");
        }
    }
    let ids: Vec<PeerId> = host.peers.keys().copied().collect();
    for id in ids {
        let delivered = host.peers.get_mut(&id).map(|p| p.link.poll(now)).unwrap_or_default();
        for message in delivered {
            match message {
                Delivered::Reliable(bytes) => match msg::decode::<Msg>(&bytes) {
                    Ok(m) => on_message(world, host, id, m),
                    Err(e) => crate::hooks::net_line(world, "undecodable", JournalKind::Warn, format!("peer {id}: {e}")),
                },
                Delivered::Unreliable(bytes) => match msg::decode::<Inputs>(&bytes) {
                    Ok(inputs) => on_inputs(world, host, id, inputs),
                    Err(e) => crate::hooks::net_line(world, "undecodable", JournalKind::Warn, format!("peer {id}: {e}")),
                },
            }
        }
    }
    // Silence: a connected player leaves, anyone else is forgotten.
    let timeout_ms = (net_def(world).timeout_secs * 1000.0) as u64;
    for peer in host.peers.values_mut() {
        if peer.stage != PeerStage::Gone && peer.link.is_timed_out(now, timeout_ms) {
            if peer.stage == PeerStage::Connected {
                host.tasks.push(Task::Leave { player: peer.player, reason: "timeout".into() });
                NetMarks::mark(world, |m| &mut m.disconnected);
            }
            peer.stage = PeerStage::Gone;
        }
    }
    // Waiting players are asked again every tick.
    let waiting: Vec<PeerId> = host.peers.iter().filter(|(_, p)| matches!(p.stage, PeerStage::Waiting(_))).map(|(id, _)| *id).collect();
    for id in waiting {
        decide(world, host, id);
    }
}

fn on_message(world: &mut World, host: &mut NetHost, id: PeerId, message: Msg) {
    match message {
        Msg::Hello(hello) => on_hello(world, host, id, hello),
        Msg::Ready { .. } => {
            if let Some(peer) = host.peers.get_mut(&id)
                && peer.stage == PeerStage::Accepted
            {
                peer.stage = PeerStage::Ready;
                host.tasks.push(Task::Admit(id));
            }
        }
        Msg::Bye { reason } => {
            let Some(peer) = host.peers.get_mut(&id) else { return };
            if peer.stage == PeerStage::Connected {
                host.tasks.push(Task::Leave { player: peer.player, reason: if reason.is_empty() { "quit".into() } else { reason } });
            }
            peer.stage = PeerStage::Gone;
        }
        Msg::Call(call) => {
            if host.peers.get(&id).is_some_and(|p| p.stage == PeerStage::Connected) {
                host.tasks.push(Task::Call { peer: id, call });
            }
        }
        Msg::Desync { tick, first, .. } => {
            let Some(peer) = host.peers.get_mut(&id) else { return };
            peer.desyncs += 1;
            host.desyncs += 1;
            let player = peer.player;
            if net_def(world).resync_on_desync {
                peer.rebase = Some("desync".into());
            }
            journal::emit(world, JournalKind::Warn, format!("net: p{player} desync at tick {tick} ({first})"));
        }
        _ => {}
    }
}

fn on_inputs(world: &World, host: &mut NetHost, id: PeerId, inputs: Inputs) {
    let tick = sim::tick(world);
    let Some(peer) = host.peers.get_mut(&id).filter(|p| p.stage == PeerStage::Connected) else { return };
    if inputs.epoch != peer.resyncs {
        // Sent before the client took the world again: ticks of another timeline.
        return;
    }
    for frame in inputs.frames {
        peer.inputs.receive(frame, tick);
    }
    peer.margin = Some((inputs.tick, inputs.tick as i64 - tick as i64));
    if peer.acked.is_none_or(|a| inputs.state_ack > a) && inputs.state_ack > 0 {
        peer.acked = Some(inputs.state_ack);
        peer.acked_at = tick;
    }
}

/// Why `hello` cannot join this game, if it cannot.
fn refusal(world: &World, hello: &Hello) -> Option<String> {
    if hello.proto != PROTO {
        return Some(format!("session protocol differs: host {PROTO}, you {}; run the same build", hello.proto));
    }
    if hello.engine != ENGINE_VERSION {
        return Some(format!("engine version differs: host {ENGINE_VERSION}, you {}; run the same build", hello.engine));
    }
    let game = world.get_resource::<NetGame>().cloned().unwrap_or_default().0;
    if hello.game != game {
        return Some(format!("game differs: host `{game}`, you `{}`", hello.game));
    }
    let content = world.get_resource::<ContentDb>().map_or(0, ContentDb::hash);
    if hello.content_hash != content {
        return Some(format!("content differs: host {content:016x}, you {:016x}; run the same build and content", hello.content_hash));
    }
    let rules = net_rules::rules_hash(world);
    if hello.net_rules_hash != rules {
        return Some(format!("network rules differ: host {rules:016x}, you {:016x}; run the same build", hello.net_rules_hash));
    }
    let hz = 1.0 / sim::tick_seconds(world) as f64;
    if (hello.tick_hz - hz).abs() > 0.01 {
        return Some(format!("tick rate differs: host {hz:.0} Hz, you {:.0} Hz; start with --hz {hz:.0}", hello.tick_hz));
    }
    None
}

fn on_hello(world: &mut World, host: &mut NetHost, id: PeerId, hello: Hello) {
    if host.peers.get(&id).is_none_or(|p| p.stage != PeerStage::New) {
        return;
    }
    if let Some(reason) = refusal(world, &hello) {
        refuse(world, host, id, &reason);
        return;
    }
    let def = net_def(world);
    // A token the session knows takes its slot back (and its body, if it still waits).
    let known = (!hello.token.is_empty()).then(|| host.slots.iter().find(|(_, s)| s.token == hello.token).map(|(n, _)| *n)).flatten();
    let player = match known {
        Some(n) => {
            if let Some(old) = host.slots[&n].peer.filter(|p| *p != id)
                && let Some(peer) = host.peers.get_mut(&old)
                && peer.stage != PeerStage::Gone
            {
                // Reconnected before the old link timed out: the old one leaves first.
                if peer.stage == PeerStage::Connected {
                    host.tasks.push(Task::Leave { player: n, reason: "reconnected".into() });
                }
                peer.stage = PeerStage::Gone;
            }
            Some(n)
        }
        None => free_slot(world, host, &def),
    };
    let Some(player) = player else {
        refuse(world, host, id, &format!("the session is full ({} players)", def.max_players));
        return;
    };
    host.slots.insert(player, Slot { token: hello.token.clone(), name: hello.name.clone(), peer: Some(id), left_at: None });
    let peer = host.peers.get_mut(&id).expect("known peer");
    peer.player = player;
    peer.hello = Some(hello);
    peer.stage = PeerStage::Waiting(String::new());
    decide(world, host, id);
}

/// The lowest player number nobody holds: no connected peer, no body waiting for a rejoin.
fn free_slot(world: &World, host: &NetHost, def: &NetDef) -> Option<u8> {
    (1..def.max_players).find(|n| {
        let held = host.slots.get(n).is_some_and(|s| s.peer.is_some_and(|p| host.peers.get(&p).is_some_and(|peer| peer.stage != PeerStage::Gone)));
        !held && player_body(world, *n).is_none()
    })
    .or_else(|| {
        // A slot whose player left and whose body is gone.
        (1..def.max_players).find(|n| host.slots.get(n).is_some_and(|s| s.left_at.is_some()) && player_body(world, *n).is_none())
    })
}

/// Asks the game whether a waiting peer may join now.
fn decide(world: &mut World, host: &mut NetHost, id: PeerId) {
    let Some(peer) = host.peers.get_mut(&id) else { return };
    let Some(hello) = peer.hello.clone() else { return };
    let request = JoinRequest { player: peer.player, name: hello.name.clone(), token: hello.token.clone(), character: hello.character.clone() };
    match (hooks(world).join_policy)(world, &request) {
        JoinDecision::Accept => {
            let seed = world.get_resource::<ai_core::SimSeed>().map_or(0, |s| s.0);
            let accept = Accept {
                seed,
                tick_hz: 1.0 / sim::tick_seconds(world) as f64,
                session: peer.link.session(),
                player: peer.player,
                region: (hooks(world).describe_world)(world),
            };
            peer.send(&Msg::Accept(accept));
            peer.stage = PeerStage::Accepted;
            let (player, name) = (peer.player, peer.name());
            crate::hooks::net_line(world, &format!("accept {player}"), JournalKind::Sim, format!("p{player} ({name}) accepted, building the world"));
        }
        JoinDecision::Wait(reason) => {
            if peer.stage != PeerStage::Waiting(reason.clone()) {
                peer.send(&Msg::Wait { reason: reason.clone() });
                peer.stage = PeerStage::Waiting(reason.clone());
                let (player, name) = (peer.player, peer.name());
                crate::hooks::net_line(world, &format!("wait {player}"), JournalKind::Sim, format!("p{player} ({name}) waits to join: {reason}"));
            }
        }
        JoinDecision::Refuse(reason) => refuse(world, host, id, &reason),
    }
}

fn refuse(world: &mut World, host: &mut NetHost, id: PeerId, reason: &str) {
    let Some(peer) = host.peers.get_mut(&id) else { return };
    peer.send(&Msg::Refused { reason: reason.to_owned() });
    peer.stage = PeerStage::Gone;
    let name = peer.name();
    for slot in host.slots.values_mut().filter(|s| s.peer == Some(id)) {
        slot.peer = None;
    }
    journal::emit(world, JournalKind::Warn, format!("net: refused {name}: {reason}"));
}

// --- tasks ---------------------------------------------------------------------------------

fn run_tasks(world: &mut World, host: &mut NetHost) {
    for task in std::mem::take(&mut host.tasks) {
        match task {
            Task::Admit(id) => admit(world, host, id),
            Task::Leave { player, reason } => leave(world, host, player, &reason),
            Task::Call { peer, call } => forwarded(world, host, peer, call),
        }
    }
}

/// The body id in a `player_join`/`player_leave` answer.
fn body_of(result: &Value) -> Option<u32> {
    result.get("body").and_then(Value::as_str).and_then(AiId::parse).map(|id| id.0)
}

fn admit(world: &mut World, host: &mut NetHost, id: PeerId) {
    let Some(peer) = host.peers.get(&id).filter(|p| p.stage == PeerStage::Ready) else { return };
    let Some(hello) = peer.hello.clone() else { return };
    let player = peer.player;
    let mut args = json!({ "player": player, "name": hello.name, "token": hello.token });
    if let Some(character) = &hello.character {
        args["character"] = character.clone();
    }
    let joined = call_tool(world, NET_AGENT, "player_join", args);
    if !joined.ok {
        let reason = format!("cannot spawn your body: {}", joined.result.as_str().unwrap_or_default());
        refuse(world, host, id, &reason);
        return;
    }
    let body = body_of(&joined.result);
    let tick = sim::tick(world);
    let welcome = match welcome(world, player, body, "join") {
        Ok(w) => w,
        Err(e) => {
            refuse(world, host, id, &format!("the host cannot capture its world: {e}"));
            return;
        }
    };
    let peer = host.peers.get_mut(&id).expect("checked above");
    peer.send(&Msg::Welcome(welcome));
    peer.stage = PeerStage::Connected;
    peer.body = body;
    peer.joined_at = tick;
    reset_view(peer);
    if let Some(slot) = host.slots.get_mut(&player) {
        slot.peer = Some(id);
        slot.left_at = None;
    }
    NetMarks::mark(world, |m| &mut m.joined);
    journal::emit(world, JournalKind::Sim, format!("net: p{player} ({}) joined at tick {tick}", hello.name));
}

fn reset_view(peer: &mut Peer) {
    peer.sent.clear();
    peer.acked = None;
    peer.view.clear();
}

/// The world as a client must take it.
fn welcome(world: &World, player: u8, body: Option<u32>, reason: &str) -> Result<Welcome, String> {
    let snapshot = snapshot::capture(world)?;
    Ok(Welcome { player, body, tick: sim::tick(world), reason: reason.to_owned(), snapshot, region: (hooks(world).describe_world)(world) })
}

fn leave(world: &mut World, host: &mut NetHost, player: u8, reason: &str) {
    let tick = sim::tick(world);
    if let Some(body) = player_body(world, player).and_then(|id| world.resource::<AiIndex>().get(id)) {
        if let Some(sections) = (hooks(world).capture_character)(world, body)
            && let Some(peer) = host.peers.values_mut().find(|p| p.player == player)
        {
            peer.send(&Msg::CharacterSave { tick, reason: reason.to_owned(), sections });
        }
        if world.get::<NetLeft>(body).is_none() {
            let out = call_tool(world, NET_AGENT, "player_leave", json!({ "player": player, "reason": reason }));
            if !out.ok {
                journal::emit(world, JournalKind::Warn, format!("net: p{player} left, but player_leave failed: {}", out.result));
            }
        }
    }
    if let Some(slot) = host.slots.get_mut(&player) {
        slot.peer = None;
        slot.left_at = Some(tick);
    }
    NetMarks::mark(world, |m| &mut m.left);
    journal::emit(world, JournalKind::Sim, format!("net: p{player} left ({reason})"));
}

fn forwarded(world: &mut World, host: &mut NetHost, id: PeerId, call: CallMsg) {
    let Some(peer) = host.peers.get(&id) else { return };
    let player = peer.player;
    let def = net_def(world);
    let agent = format!("p{player}/{}", call.agent);
    let scope = call.scope.min(def.guest_scope);
    // Acting as their own body: bound to the player, whichever entity that is at the time.
    let own = call.subject.is_none() || call.subject == peer.body;
    let change = SessionChange { scope: Some(scope), subject: (!own).then(|| call.subject.map(AiId)).flatten(), player: own.then_some(player), clear_subject: false };
    let reply = match configure_session(world, &agent, &change) {
        Ok(_) => {
            let out = call_tool(world, &agent, &call.tool, call.args);
            let text = match &out.result {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            Msg::CallReply { id: call.id, ok: out.ok, text, tick: out.tick }
        }
        Err(e) => Msg::CallReply { id: call.id, ok: false, text: e, tick: sim::tick(world) },
    };
    if let Some(peer) = host.peers.get_mut(&id) {
        peer.send(&reply);
    }
    if call.scope > scope {
        debug!("net: {agent} asked with {} scope, ran with {scope}", call.scope);
    }
}

// --- sending -------------------------------------------------------------------------------

/// Gameplay journal lines since the last send, to every connected peer.
/// Then the typed events (`net_event`) of those ticks.
fn send_journal(world: &mut World, host: &mut NetHost) {
    let journal = world.resource::<AiJournal>();
    let (entries, _) = journal.after(host.journal_seq);
    let mut lines: Vec<Msg> = entries
        .filter(|e| e.kind == JournalKind::Event)
        .map(|e| Msg::Journal { tick: e.tick, kind: "event".into(), text: e.text.clone() })
        .collect();
    host.journal_seq = journal.last_seq();
    if let Some(mut outbox) = world.get_resource_mut::<crate::events::EventOutbox>() {
        lines.extend(outbox.0.drain(..).map(|(tick, type_path, value)| Msg::Event { tick, type_path, value }));
    }
    if lines.is_empty() {
        return;
    }
    for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
        for line in &lines {
            peer.send(line);
        }
    }
}

fn send_rebases(world: &World, host: &mut NetHost) {
    if world.get_resource::<SimGate>().is_some_and(SimGate::is_held) {
        return;
    }
    if let Some(reason) = host.rebase_all.take() {
        for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
            peer.rebase = Some(reason.clone());
        }
        // Values cached for ids of the old world mean nothing now.
        host.cache.clear();
    }
    let mut captured: Option<Result<ai_core::Snapshot, String>> = None;
    for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
        let Some(reason) = peer.rebase.take() else { continue };
        let snapshot = captured.get_or_insert_with(|| snapshot::capture(world)).clone();
        let Ok(snapshot) = snapshot else { continue };
        // The body may be another entity now (a region change spawns players anew).
        let body = player_body(world, peer.player).map(|id| id.0).or(peer.body);
        peer.body = body;
        let welcome = Welcome { player: peer.player, body, tick: sim::tick(world), reason, snapshot, region: (hooks(world).describe_world)(world) };
        peer.send(&Msg::Rebase(welcome));
        peer.resyncs += 1;
        reset_view(peer);
        // Its inputs start over on the new world's ticks.
        peer.inputs = InputQueue::default();
        peer.margin = None;
        peer.applied_input_tick = 0;
    }
}

/// On send ticks: every connected peer's view, encoded once, as a delta against what it
/// acknowledged.
fn send_state(world: &mut World, host: &mut NetHost) {
    let def = net_def(world);
    let tick = sim::tick(world);
    if !tick.is_multiple_of(def.send_every.max(1) as u64) || host.connected().next().is_none() {
        return;
    }
    let started = std::time::Instant::now();
    let paused = sim::is_paused(world);
    let with_hash = tick.is_multiple_of(def.desync_every.max(1) as u64);
    let predicted: BTreeSet<String> = {
        let rules = world.resource::<NetRules>();
        let registry = world.resource::<AppTypeRegistry>().read();
        rules.predicted().filter_map(|t| registry.get(t).map(|r| r.type_info().type_path().to_owned())).collect()
    };
    let mut encoded = Encoded { resources: Encoded::resources_of(world), ..default() };
    let ids: Vec<PeerId> = host.peers.iter().filter(|(_, p)| p.stage == PeerStage::Connected).map(|(id, _)| *id).collect();
    // Who sees what (a query: the world mutably), then one encoder for every peer.
    let views: Vec<(PeerId, Option<Vec3>, BTreeSet<u32>)> = ids
        .into_iter()
        .map(|id| {
            let peer = &host.peers[&id];
            let center = peer.body.and_then(|b| world.resource::<AiIndex>().get(AiId(b))).and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
            (id, center, base_view(world, center, &peer.view, def.interest_radius, def.interest_hysteresis))
        })
        .collect();
    let world = &*world;
    let encoder = StateEncoder::new(world, ai_core::state::StateRule::Replicated).without_read_back();
    for (id, center, mut view) in views {
        let peer = host.peers.get_mut(&id).expect("listed above");
        if close_view(world, &encoder, &mut host.cache, &mut encoded, &mut view) {
            peer.closure_capped += 1;
        }
        // The newest acknowledged view, unless the client has been quiet too long.
        let baseline = peer
            .acked
            .filter(|_| tick.saturating_sub(peer.acked_at) <= ACK_PATIENCE)
            .and_then(|a| peer.sent.iter().find(|(t, _)| *t == a).map(|(t, s)| (*t, s.clone())));
        let built = build_delta(DeltaInput {
            tick,
            view: &view,
            encoded: &encoded,
            baseline: baseline.as_ref().map(|(t, s)| (*t, &**s)),
            own: peer.body.map(|b| (b, &predicted)),
            applied_input_tick: peer.applied_input_tick,
            with_hash,
            cap_bytes: def.delta_cap_kb as usize * 1024,
            center,
        });
        let mut delta = built.delta;
        delta.paused = paused;
        delta.inputs_ack = peer.inputs.newest;
        delta.margin = peer.margin;
        delta.epoch = peer.resyncs;
        let bytes = msg::encode(&delta);
        if let Err(e) = peer.link.send_unreliable(&bytes) {
            warn!("net: state for p{} not sent: {e}", peer.player);
        }
        peer.deltas += 1;
        peer.delta_bytes += built.bytes as u64;
        peer.held_back += built.held_back.len() as u64;
        peer.view = view;
        peer.sent.push_back((tick, Arc::new(built.sent)));
        while peer.sent.len() > SENT_KEPT {
            peer.sent.pop_front();
        }
    }
    let us = started.elapsed().as_micros() as u64;
    host.encode_us = (us, host.encode_us.1.max(us));
    host.encode_total = (host.encode_total.0 + us, host.encode_total.1 + 1);
}

fn flush(host: &mut NetHost, endpoint: &mut NetEndpoint) {
    let now = endpoint.clock.now();
    let NetEndpoint { transport, .. } = endpoint;
    for (id, peer) in host.peers.iter_mut() {
        peer.link.flush(now, &mut |datagram| transport.send(*id, datagram));
    }
    // Gone peers are dropped once nothing of theirs waits (a refusal, a kick), or once they
    // stopped answering (a timeout, a peer that went before acknowledging).
    let gone: Vec<PeerId> = host
        .peers
        .iter()
        .filter(|(_, p)| p.stage == PeerStage::Gone && (p.link.is_idle() || p.link.is_timed_out(now, GONE_PATIENCE_MS)))
        .map(|(id, _)| *id)
        .collect();
    for id in gone {
        host.peers.remove(&id);
        transport.forget(id);
    }
}
