//! The host's side of a session: its peers, the join handshake, remote inputs, state sends,
//! leaves and the end of the session.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use ai_core::journal::{self, JournalKind};
use ai_core::net_rules::{self, NetRules};
use ai_core::snapshot::{self, ENGINE_VERSION};
use ai_core::state::StateEncoder;
use ai_core::tools::NetRoute;
use ai_core::{call_tool, configure_session, sim, AiId, AiIndex, AiJournal, AiTools, Scope, SessionChange, SimGate, WorldRebuilt};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::{player_body, Intent, PlayerId};

use crate::def::{net_def, NetDef};
use crate::hooks::{hooks, JoinDecision, JoinRequest, NetLeft, NetSeat};
use crate::inputs::InputQueue;
use crate::link::{Delivered, Link, Role, SendError, Verdict};
use crate::msg::{self, Accept, CallMsg, Hello, Inputs, Msg, Welcome, PROTO};
use crate::replicate::{base_view, build_delta, close_view, DeltaInput, EncodeCache, Encoded, Fnv, SentView, ACK_PATIENCE, CLOSURE_CAP, SENT_KEPT};
use crate::session::{NetEndpoint, NetGame, NetMarks, NetState, Ticked, NET_AGENT};
use crate::transport::PeerId;
use crate::wire;

/// How long a peer that is gone may stay silent with messages unacknowledged before it is
/// forgotten, milliseconds.
const GONE_PATIENCE_MS: u64 = 3000;

/// Calls a peer may have waiting for one tick; more are refused.
pub const MAX_CALLS_PER_TICK: u32 = 16;

/// Agents (`p1/<name>`) one player may run calls as on the host.
pub const MAX_AGENTS: usize = 8;

/// Seconds between two resyncs a peer's desync reports may cause.
pub const DESYNC_RESYNC_COOLDOWN_SECS: u64 = 5;

/// Longest player name a `Hello` may carry, in characters.
pub const MAX_NAME: usize = 32;

/// Longest token a `Hello` may carry (the host issues 32 hex digits).
pub const MAX_TOKEN: usize = 64;

/// Largest character (save sections) a `Hello` may carry, as JSON bytes.
pub const MAX_CHARACTER_BYTES: usize = 64 * 1024;

/// Longest peer text (a desync's first difference, a version string) put into the journal.
pub const MAX_PEER_TEXT: usize = 200;

/// Largest message a peer that does not play yet is read, in bytes: a `Hello` with the
/// largest character. A larger one is refused unread.
pub const MAX_JOINING_MESSAGE: usize = MAX_CHARACTER_BYTES + 16 * 1024;

/// Largest reliable message a player is read, in bytes (a call and its arguments, a desync
/// report). A larger one is dropped unread.
pub const MAX_PLAYER_MESSAGE: usize = 1 << 20;

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
    /// The last view was capped: it named entities the client does not have, and the client's
    /// references to them read as nothing.
    capped: bool,
    /// JSON bytes of state values sent, by component type path (or resource name).
    pub by_component: BTreeMap<String, u64>,
    /// What the link watch saw a second ago: late inputs, RTT.
    watched: (u64, f32),
    /// Inputs of epochs before this one belong to a world whose ticks went back: dropped.
    accept_from_epoch: u32,
    /// The agents this player's calls ran as (at most [`MAX_AGENTS`]).
    agents: BTreeSet<String>,
    /// Calls waiting for the next tick.
    calls_queued: u32,
    /// The host tick of the newest desync report taken, and of the last resync one caused.
    last_desync: Option<u64>,
    desync_resync_at: Option<u64>,
    /// The host's ticks moved (a snapshot load) since this peer last got the world: what it
    /// has on its way is of the old ticks, and the world it gets next starts its inputs over.
    ticks_moved: bool,
    /// Messages its link would not take (it has too much unacknowledged), and how many of
    /// them the link watch has said.
    pub unsent: u64,
    unsent_said: u64,
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
            capped: false,
            by_component: BTreeMap::new(),
            watched: (0, 0.0),
            accept_from_epoch: 0,
            agents: BTreeSet::new(),
            calls_queued: 0,
            last_desync: None,
            desync_resync_at: None,
            ticks_moved: false,
            unsent: 0,
            unsent_said: 0,
        }
    }

    /// Queues `message`; the link refuses it when the peer has not acknowledged too much
    /// (counted: the link watch says so at most once a second).
    fn send(&mut self, message: &Msg) -> Result<(), SendError> {
        self.send_bytes(&msg::encode(message))
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), SendError> {
        let sent = self.link.send_reliable(bytes);
        if sent.is_err() {
            self.unsent += 1;
        }
        sent
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
    /// Player `player` left through `peer` (its character goes there, not to a newer link).
    Leave { player: u8, peer: PeerId, reason: String },
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
    /// Tokens issued so far.
    issued: u64,
    /// Tokens of players whose slot another player took since (the newest last): their holders
    /// come back to a slot of their own with the same token, so what the host kept of them
    /// (co-op saves) is still theirs.
    retired: Vec<String>,
    /// The endpoint runs on a wall clock (UDP): tokens are random, not derived.
    wall_clock: bool,
    /// The host tick the last send saw: a tick that is neither it nor the next moved time.
    last_tick: u64,
    /// Components already said to be left out of the state (they do not read back).
    left_out_said: BTreeSet<String>,
    /// Bytes of the last world sent again: a link with less room waits without a capture.
    rebase_bytes: usize,
}

impl NetHost {
    pub fn new() -> Self {
        Self {
            peers: BTreeMap::new(),
            slots: BTreeMap::new(),
            retired: Vec::new(),
            cache: EncodeCache::default(),
            journal_seq: 0,
            tasks: Vec::new(),
            rebase_all: None,
            encode_us: (0, 0),
            encode_total: (0, 0),
            desyncs: 0,
            ended: None,
            issued: 0,
            wall_clock: false,
            last_tick: 0,
            left_out_said: BTreeSet::new(),
            rebase_bytes: 0,
        }
    }

    /// Values kept between sends (`net_status`).
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// A token for a new player: who they are when they come back (a restarted client brings
    /// it). Random over a real network; derived from the session in a stand, whose runs must
    /// repeat. Never one a slot holds already.
    fn issue_token(&mut self, seed: u64, player: u8, name: &str) -> String {
        loop {
            self.issued += 1;
            let n = self.issued;
            let word = |salt: u64| -> u64 {
                if self.wall_clock {
                    random_word(salt, n)
                } else {
                    let mut h = Fnv::default();
                    for part in [seed, n, u64::from(player), salt] {
                        h.eat_u64(part);
                    }
                    h.eat(name.as_bytes());
                    h.0
                }
            };
            let token = format!("{:016x}{:016x}", word(1), word(2));
            if !self.slots.values().any(|s| s.token == token) && !self.retired.contains(&token) {
                return token;
            }
        }
    }

    /// Retired tokens kept: the oldest are forgotten beyond this.
    const RETIRED_KEPT: usize = 64;

    /// Puts `token` of a slot another player takes among the retired ones.
    fn retire(&mut self, token: String) {
        if token.is_empty() || self.retired.contains(&token) {
            return;
        }
        self.retired.push(token);
        if self.retired.len() > Self::RETIRED_KEPT {
            self.retired.remove(0);
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
        let _ = peer.send(&Msg::Bye { reason: reason.to_owned() });
        let connected = peer.stage == PeerStage::Connected;
        peer.stage = PeerStage::Gone;
        let id = peer.id;
        if connected {
            self.tasks.push(Task::Leave { player, peer: id, reason: reason.to_owned() });
        }
        Ok(())
    }

    /// Sends player `player`'s machine its character (co-op saves: `sections` as the game
    /// captured it on the host, at host tick `tick`, saved for `reason`). Reliable and in order
    /// with everything else the host says. `false`: that player is not connected.
    pub fn send_character(&mut self, player: u8, tick: u64, reason: &str, sections: Value) -> bool {
        let Some(peer) = self.peers.values_mut().find(|p| p.player == player && p.stage == PeerStage::Connected) else {
            return false;
        };
        peer.send(&Msg::CharacterSave { tick, reason: reason.to_owned(), sections }).is_ok()
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

/// 64 random bits: the process's random hasher keys (from the OS), the time and `salt`/`n`.
pub(crate) fn random_word(salt: u64, n: u64) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::hash::RandomState::new().build_hasher();
    h.write_u64(salt);
    h.write_u64(n);
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    h.write_u32(std::process::id());
    h.finish()
}

/// Peer text as one journal line: at most `max` characters, control characters (a newline
/// that would forge a line) escaped.
pub fn clip(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i >= max {
            out.push('…');
            break;
        }
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// Player `n`'s body, unless it waits for a rejoin (`NetLeft`).
fn live_body(world: &World, n: u8) -> Option<u32> {
    let id = player_body(world, n)?;
    let entity = world.resource::<AiIndex>().get(id)?;
    world.get::<NetLeft>(entity).is_none().then_some(id.0)
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
/// A player's body is looked up every tick: a game that respawns a player as a new entity
/// gets their inputs there, and their client is sent the world with its new body.
pub(crate) fn apply_inputs(world: &mut World) {
    let Some(mut host) = world.remove_resource::<NetHost>() else { return };
    let tick = sim::tick(world);
    for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
        // Never an id the player's body no longer has: once gone, it may be another entity's.
        let now = live_body(world, peer.player);
        if peer.body != now {
            if let Some(now) = now {
                peer.rebase.get_or_insert_with(|| format!("a new body e{now}"));
                // The session holds it for the player, as it held the old one.
                if let Some(entity) = world.resource::<AiIndex>().get(AiId(now)) {
                    world.entity_mut(entity).insert(NetSeat);
                }
            }
            peer.body = now;
        }
        let due = peer.inputs.take_due(tick);
        let Some(entity) = peer.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id))) else { continue };
        if world.get::<NetLeft>(entity).is_some() {
            continue;
        }
        for frame in &due {
            if let Some(mut intent) = world.get_mut::<Intent>(entity) {
                crate::inputs::apply_frame(&mut intent, frame);
                // However many frames are due at once, a tick takes a bounded number of presses.
                intent.pressed.truncate(crate::inputs::MAX_PRESSES_PER_TICK);
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
        watch_links(world, host);
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

/// Ends the session, once (`net_end`, then the host quitting, end it once): the game saves
/// the world, characters go to their players with `SessionEnd`, and the players leave (their
/// bodies wait, as a leaver's do). Anyone still joining is refused. A last flush.
pub fn end_session(world: &mut World, reason: &str) {
    if world.get_resource::<NetHost>().is_none_or(|h| h.ended.is_some()) {
        return;
    }
    let on_end = hooks(world).on_session_end;
    let saved_slot = on_end(world);
    with_host(world, |world, host, endpoint| {
        let tick = sim::tick(world);
        let capture = hooks(world).capture_character;
        let mut leaving = Vec::new();
        for peer in host.peers.values_mut() {
            match peer.stage {
                PeerStage::Gone => continue,
                PeerStage::Connected => {
                    if let Some(body) = peer.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id)))
                        && let Some(sections) = capture(world, body)
                    {
                        let _ = peer.send(&Msg::CharacterSave { tick, reason: "session ended".into(), sections });
                    }
                    let _ = peer.send(&Msg::SessionEnd { reason: reason.to_owned(), saved_slot: saved_slot.clone() });
                    leaving.push((peer.player, peer.id));
                }
                _ => {
                    let _ = peer.send(&Msg::Refused { reason: format!("the session has ended ({reason})") });
                }
            }
            peer.stage = PeerStage::Gone;
        }
        host.ended = Some(reason.to_owned());
        // Their characters went with `SessionEnd`: they leave without another.
        for (player, peer) in leaving {
            leave(world, host, player, peer, "session ended", false);
        }
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
    host.wall_clock = endpoint.clock.is_wall();
    // Before the tasks of this frame: a peer admitted in it gets the world as it is after the
    // jump, and starts with its ticks not moved.
    note_ticks(world, host);
    let mut datagrams = Vec::new();
    endpoint.transport.recv(now, &mut datagrams);
    for (from, bytes) in datagrams {
        if !host.peers.contains_key(&from) {
            // A game that ended (kicked, timed out, this host's previous process) keeps acking
            // for a while: only a message makes a new peer.
            if host.ended.is_some() || !wire::carries_messages(&bytes) {
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
        // Every packet repeats the frames not acknowledged: the newest holds the rest.
        let newest_inputs = delivered.iter().rposition(|m| matches!(m, Delivered::Unreliable(_)));
        for (i, message) in delivered.into_iter().enumerate() {
            let Some(playing) = host.peers.get(&id).map(|p| p.stage == PeerStage::Connected) else { break };
            if matches!(message, Delivered::Unreliable(_)) && Some(i) != newest_inputs {
                continue;
            }
            // Sizes first: nothing larger than an honest client sends is decoded.
            match message {
                Delivered::Reliable(bytes) => {
                    let max = if playing { MAX_PLAYER_MESSAGE } else { MAX_JOINING_MESSAGE };
                    if bytes.len() > max {
                        oversized(world, host, id, bytes.len(), max);
                        continue;
                    }
                    match msg::decode::<Msg>(&bytes) {
                        Ok(m) => on_message(world, host, id, m),
                        Err(e) => undecodable(world, id, &e),
                    }
                }
                // Only a player sends inputs: anyone else's are not even read.
                Delivered::Unreliable(_) if !playing => {}
                Delivered::Unreliable(bytes) if bytes.len() > crate::inputs::MAX_INPUTS_BYTES => {
                    oversized(world, host, id, bytes.len(), crate::inputs::MAX_INPUTS_BYTES);
                }
                Delivered::Unreliable(bytes) => match msg::decode::<Inputs>(&bytes) {
                    Ok(inputs) => on_inputs(world, host, id, inputs),
                    Err(e) => undecodable(world, id, &e),
                },
            }
        }
    }
    // Silence: a connected player leaves, anyone else is forgotten.
    let timeout_ms = (net_def(world).timeout_secs * 1000.0) as u64;
    for peer in host.peers.values_mut() {
        if peer.stage != PeerStage::Gone && peer.link.is_timed_out(now, timeout_ms) {
            if peer.stage == PeerStage::Connected {
                host.tasks.push(Task::Leave { player: peer.player, peer: peer.id, reason: "timeout".into() });
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

/// A message over what an honest client sends: a player's is dropped, anyone else is
/// refused (and told the limit).
fn oversized(world: &mut World, host: &mut NetHost, id: PeerId, len: usize, max: usize) {
    let Some(peer) = host.peers.get(&id) else { return };
    let (kb, max_kb) = (len.div_ceil(1024), max / 1024);
    match peer.stage {
        PeerStage::Connected => {
            let player = peer.player;
            crate::hooks::net_line(world, &format!("oversized p{player}"), JournalKind::Warn, format!("p{player} sent a message of {kb} KB, over {max_kb} KB: dropped unread"));
        }
        PeerStage::Gone => {}
        _ => refuse(world, host, id, &format!("a message of {kb} KB before you play is over the {max_kb} KB the host reads (a character takes at most {} KB)", MAX_CHARACTER_BYTES / 1024)),
    }
}

/// A message that does not decode: the error quotes the peer's text, so it is clipped.
fn undecodable(world: &mut World, id: PeerId, error: &str) {
    crate::hooks::net_line(world, "undecodable", JournalKind::Warn, format!("peer {id}: {}", clip(error, MAX_PEER_TEXT)));
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
                // The peer's words go into the journal, to every guest and into the replay.
                let reason = if reason.is_empty() { "quit".into() } else { clip(&reason, MAX_PEER_TEXT) };
                host.tasks.push(Task::Leave { player: peer.player, peer: id, reason });
            }
            peer.stage = PeerStage::Gone;
        }
        Msg::Call(call) => {
            let Some(peer) = host.peers.get_mut(&id).filter(|p| p.stage == PeerStage::Connected) else { return };
            if peer.calls_queued >= MAX_CALLS_PER_TICK {
                let text = format!("too many calls at once (at most {MAX_CALLS_PER_TICK} a tick); call again");
                let _ = peer.send(&Msg::CallReply { id: call.id, ok: false, text, tick: sim::tick(world) });
                return;
            }
            peer.calls_queued += 1;
            host.tasks.push(Task::Call { peer: id, call });
        }
        Msg::Desync { tick, host: hash, first, epoch, .. } => {
            let Some(peer) = host.peers.get_mut(&id).filter(|p| p.stage == PeerStage::Connected) else { return };
            // Only a report on a hash this host sent this peer in its current world, once.
            let sent = epoch == peer.resyncs && peer.sent.iter().any(|(t, view)| *t == tick && view.hash == Some(hash));
            if !sent || peer.last_desync.is_some_and(|t| tick <= t) {
                return;
            }
            peer.last_desync = Some(tick);
            peer.desyncs += 1;
            host.desyncs += 1;
            let player = peer.player;
            let now = sim::tick(world);
            let cooldown = DESYNC_RESYNC_COOLDOWN_SECS * (1.0 / sim::tick_seconds(world).max(1e-6)).round() as u64;
            if net_def(world).resync_on_desync && peer.desync_resync_at.is_none_or(|at| now >= at + cooldown) {
                peer.rebase = Some("desync".into());
                peer.desync_resync_at = Some(now);
            }
            NetMarks::mark(world, |m| &mut m.desync);
            let first = clip(&first, MAX_PEER_TEXT);
            crate::hooks::net_line(world, &format!("desync p{player}"), JournalKind::Warn, format!("p{player} desync at tick {tick} ({first})"));
        }
        _ => {}
    }
}

fn on_inputs(world: &mut World, host: &mut NetHost, id: PeerId, inputs: Inputs) {
    let tick = sim::tick(world);
    let Some(peer) = host.peers.get_mut(&id).filter(|p| p.stage == PeerStage::Connected) else { return };
    if inputs.epoch < peer.accept_from_epoch || inputs.epoch > peer.resyncs {
        // Sent before the client took a world whose ticks went back: another timeline.
        return;
    }
    if let Err(why) = crate::inputs::check(&inputs) {
        let player = peer.player;
        crate::hooks::net_line(world, &format!("inputs p{player}"), JournalKind::Warn, format!("p{player} sent inputs over the limits ({why}): dropped"));
        return;
    }
    for frame in inputs.frames {
        peer.inputs.receive(frame, tick);
    }
    // Saturating: a tick near `u64::MAX` from a broken client is a large margin, not a panic.
    let margin = i64::try_from(inputs.tick).unwrap_or(i64::MAX).saturating_sub(i64::try_from(tick).unwrap_or(i64::MAX));
    peer.margin = Some((inputs.tick, margin));
    if peer.acked.is_none_or(|a| inputs.state_ack > a) && inputs.state_ack > 0 {
        peer.acked = Some(inputs.state_ack);
        peer.acked_at = tick;
    }
}

/// Why `hello` cannot join this game, if it cannot. What the peer wrote is clipped before it
/// goes into the answer and the journal.
fn refusal(world: &World, hello: &Hello) -> Option<String> {
    if hello.proto != PROTO {
        return Some(format!("session protocol differs: host {PROTO}, you {}; run the same build", hello.proto));
    }
    if hello.engine != ENGINE_VERSION {
        return Some(format!("engine version differs: host {ENGINE_VERSION}, you {}; run the same build", clip(&hello.engine, 40)));
    }
    let game = world.get_resource::<NetGame>().cloned().unwrap_or_default().0;
    if hello.game != game {
        return Some(format!("game differs: host `{game}`, you `{}`", clip(&hello.game, 60)));
    }
    let name_chars = hello.name.chars().count();
    if name_chars == 0 || name_chars > MAX_NAME || hello.name.chars().any(char::is_control) {
        return Some(format!("a player name is 1 to {MAX_NAME} characters, no control characters; pick another with --name"));
    }
    if hello.token.len() > MAX_TOKEN {
        return Some(format!("the token is over {MAX_TOKEN} bytes; delete the kept one (saves/net/) to join as a new player"));
    }
    if let Some(character) = &hello.character
        && msg::encode(character).len() > MAX_CHARACTER_BYTES
    {
        return Some(format!("the character is over {} KB; the host takes at most that", MAX_CHARACTER_BYTES / 1024));
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
    // A token the session issued takes its slot back (and its body, if it still waits).
    let known = (!hello.token.is_empty()).then(|| host.slots.iter().find(|(_, s)| s.token == hello.token).map(|(n, _)| *n)).flatten();
    let player = match known {
        Some(n) => {
            if let Some(old) = host.slots[&n].peer.filter(|p| *p != id)
                && let Some(peer) = host.peers.get_mut(&old)
                && peer.stage != PeerStage::Gone
            {
                // Back before the old link timed out (a restart), or a second game with the same
                // token (two clients of one name sharing a save folder): the old one leaves first,
                // and is told why.
                let _ = peer.send(&Msg::Bye { reason: "another game joined with your token; give each game its own --name or --save-dir".into() });
                if peer.stage == PeerStage::Connected {
                    host.tasks.push(Task::Leave { player: n, peer: old, reason: "reconnected".into() });
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
    // A new player gets a token of the host's (any other they bring means nothing here), unless
    // the one they bring is of a player whose slot was taken since: it is theirs again.
    let revived = known.is_none() && !hello.token.is_empty() && host.retired.contains(&hello.token);
    let token = match known {
        Some(n) => host.slots[&n].token.clone(),
        None if revived => hello.token.clone(),
        None => {
            let seed = world.get_resource::<ai_core::SimSeed>().map_or(0, |s| s.0);
            host.issue_token(seed, player, &hello.name)
        }
    };
    if revived {
        host.retired.retain(|t| *t != token);
    }
    if let Some(old) = host.slots.get(&player).filter(|old| old.token != token) {
        let old = old.token.clone();
        host.retire(old);
    }
    host.slots.insert(player, Slot { token, name: hello.name.clone(), peer: Some(id), left_at: None });
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
    let token = host.slots.get(&peer.player).map(|s| s.token.clone()).unwrap_or_default();
    let request = JoinRequest { player: peer.player, name: hello.name.clone(), token: token.clone(), character: hello.character.clone() };
    match (hooks(world).join_policy)(world, &request) {
        JoinDecision::Accept => {
            let seed = world.get_resource::<ai_core::SimSeed>().map_or(0, |s| s.0);
            let accept = Accept {
                seed,
                tick_hz: 1.0 / sim::tick_seconds(world) as f64,
                session: peer.link.session(),
                player: peer.player,
                region: (hooks(world).describe_world)(world),
                token,
            };
            let _ = peer.send(&Msg::Accept(accept));
            peer.stage = PeerStage::Accepted;
            let (player, name) = (peer.player, peer.name());
            crate::hooks::net_line(world, &format!("accept {player}"), JournalKind::Sim, format!("p{player} ({name}) accepted, building the world"));
        }
        JoinDecision::Wait(reason) => {
            if peer.stage != PeerStage::Waiting(reason.clone()) {
                let _ = peer.send(&Msg::Wait { reason: reason.clone() });
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
    let _ = peer.send(&Msg::Refused { reason: reason.to_owned() });
    peer.stage = PeerStage::Gone;
    let name = peer.name();
    for slot in host.slots.values_mut().filter(|s| s.peer == Some(id)) {
        slot.peer = None;
    }
    // At most a line a second: anyone who can send a datagram can be refused.
    crate::hooks::net_line(world, "refused", JournalKind::Warn, format!("refused {name}: {reason}"));
}

// --- tasks ---------------------------------------------------------------------------------

fn run_tasks(world: &mut World, host: &mut NetHost) {
    // A rebuilt world, once out of its loading gate: its players first, then every client
    // gets it whole.
    if host.rebase_all.is_some() && !world.get_resource::<SimGate>().is_some_and(SimGate::is_held) {
        let reason = host.rebase_all.take().unwrap_or_default();
        reconcile_players(world, host);
        for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
            peer.rebase = Some(reason.clone());
        }
        // Values cached for ids of the old world mean nothing now.
        host.cache.clear();
    }
    for task in std::mem::take(&mut host.tasks) {
        match task {
            Task::Admit(id) => admit(world, host, id),
            Task::Leave { player, peer, reason } => leave(world, host, player, peer, &reason, true),
            Task::Call { peer, call } => forwarded(world, host, peer, call),
        }
    }
    for peer in host.peers.values_mut() {
        peer.calls_queued = 0;
    }
}

/// After the world was rebuilt (a snapshot load, a region change): every connected player has
/// a body in it, and no other player of the session's plays on. A snapshot from before a
/// player joined has none for them: they join again (recorded). One from after a player left
/// (or one of another host process's session) has their body without `NetLeft`: it leaves
/// again, and waits for their rejoin. Which bodies are the session's says the world itself
/// ([`NetSeat`]), not the slots: a body the host joined itself (`player_join` without a peer:
/// co-op on this machine) is no peer's and stays, whichever number a guest held before.
fn reconcile_players(world: &mut World, host: &mut NetHost) {
    let connected: BTreeMap<u8, PeerId> = host.peers.iter().filter(|(_, p)| p.stage == PeerStage::Connected).map(|(id, p)| (p.player, *id)).collect();
    for (&player, id) in &connected {
        if live_body(world, player).is_some() {
            continue;
        }
        let Some(hello) = host.peers.get(id).and_then(|p| p.hello.clone()) else { continue };
        let (args, overrode) = join_call(world, host, player, &hello);
        let joined = call_tool(world, NET_AGENT, "player_join", args);
        if !joined.ok {
            let why = joined.result.as_str().map_or_else(|| joined.result.to_string(), str::to_owned);
            let _ = host.kick(player, &format!("the host's world has no body for you ({why})"));
        } else if overrode {
            send_character(world, host, player, body_of(&joined.result), "rejoined");
        }
    }
    let others: BTreeSet<u8> = {
        let mut q = world.query_filtered::<&PlayerId, (With<NetSeat>, Without<NetLeft>)>();
        q.iter(world).map(|p| p.0).filter(|n| *n != 0 && !connected.contains_key(n)).collect()
    };
    for player in others {
        let out = call_tool(world, NET_AGENT, "player_leave", json!({ "player": player, "reason": "not in the session" }));
        if !out.ok {
            journal::emit(world, JournalKind::Warn, format!("net: p{player}'s body is in the world, but player_leave failed: {}", out.result));
        }
    }
}

/// Takes back the body just spawned for a player who could not be admitted: it waits for a
/// rejoin, as a leaver's does.
fn leave_body(world: &mut World, player: u8, reason: &str) {
    let out = call_tool(world, NET_AGENT, "player_leave", json!({ "player": player, "reason": reason }));
    if !out.ok {
        journal::emit(world, JournalKind::Warn, format!("net: p{player} was not admitted, but player_leave failed: {}", out.result));
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
    let had_live_body = live_body(world, player).is_some();
    let (args, overrode) = join_call(world, host, player, &hello);
    let joined = call_tool(world, NET_AGENT, "player_join", args);
    if !joined.ok {
        // A join that failed after it made a live body (a hook that panicked: a new one, or one
        // that waited for the player and no longer does) must not leave it playing for nobody and
        // holding the slot: it waits for a rejoin, a leaver's way, and keeps its seat. A body
        // that was live before is another's, not this join's to take away.
        if !had_live_body && live_body(world, player).is_some() {
            leave_body(world, player, "cannot spawn your body");
        }
        let reason = format!("cannot spawn your body: {}", joined.result.as_str().unwrap_or_default());
        refuse(world, host, id, &reason);
        return;
    }
    let body = body_of(&joined.result);
    let tick = sim::tick(world);
    let welcome = match welcome(world, player, body, "join") {
        Ok(w) => w,
        Err(e) => {
            leave_body(world, player, "the world could not be sent");
            refuse(world, host, id, &format!("the host cannot capture its world: {e}"));
            return;
        }
    };
    let peer = host.peers.get_mut(&id).expect("checked above");
    let welcome = Msg::Welcome(welcome);
    let kb = msg::encode(&welcome).len() as f64 / 1024.0;
    if let Err(e) = peer.send(&welcome) {
        // Not queued: they are not in the session; the body waits for them to join again.
        leave_body(world, player, "the world could not be sent");
        refuse(world, host, id, &format!("the host cannot send you its world ({e}); join again"));
        return;
    }
    peer.stage = PeerStage::Connected;
    peer.body = body;
    peer.joined_at = tick;
    peer.ticks_moved = false;
    reset_view(peer);
    if let Some(slot) = host.slots.get_mut(&player) {
        slot.peer = Some(id);
        slot.left_at = None;
    }
    NetMarks::mark(world, |m| &mut m.joined);
    journal::emit(world, JournalKind::Sim, format!("net: p{player} ({}) joined at tick {tick} (world {kb:.1} KB)", hello.name));
    if overrode {
        // The host has a newer character for them than the one they brought.
        send_character(world, host, player, body, "rejoined");
    }
}

/// The recorded `player_join` of `player` and whether the host chose another character than
/// the one they brought (`NetHooks::join_character`). The token stays out of the call:
/// replays are shared, and a token says who you are.
fn join_call(world: &mut World, host: &NetHost, player: u8, hello: &Hello) -> (Value, bool) {
    let token = host.slots.get(&player).map(|s| s.token.clone()).unwrap_or_default();
    let request = JoinRequest { player, name: hello.name.clone(), token, character: hello.character.clone() };
    let chosen = (hooks(world).join_character)(world, &request);
    let mut args = json!({ "player": player, "name": hello.name });
    if let Some(character) = &chosen {
        args["character"] = character.clone();
    }
    (args, chosen != hello.character)
}

/// Sends `player` their character as the host has it now.
fn send_character(world: &mut World, host: &mut NetHost, player: u8, body: Option<u32>, reason: &str) {
    let Some(entity) = body.and_then(|id| world.resource::<AiIndex>().get(AiId(id))) else { return };
    let Some(sections) = (hooks(world).capture_character)(world, entity) else { return };
    host.send_character(player, sim::tick(world), reason, sections);
}

fn reset_view(peer: &mut Peer) {
    peer.sent.clear();
    peer.acked = None;
    peer.view.clear();
}

/// The world as a client must take it.
fn welcome(world: &World, player: u8, body: Option<u32>, reason: &str) -> Result<Welcome, String> {
    let snapshot = snapshot::capture(world)?;
    Ok(Welcome {
        player,
        body,
        tick: sim::tick(world),
        reason: reason.to_owned(),
        snapshot,
        region: (hooks(world).describe_world)(world),
        carry_inputs: false,
    })
}

/// A player leaves: their character goes to them (unless `send_character` is false: it went
/// already), their body waits for a rejoin, their slot is free.
fn leave(world: &mut World, host: &mut NetHost, player: u8, peer: PeerId, reason: &str, send_character: bool) {
    let tick = sim::tick(world);
    if let Some(body) = player_body(world, player).and_then(|id| world.resource::<AiIndex>().get(id)) {
        if send_character
            && let Some(sections) = (hooks(world).capture_character)(world, body)
            && let Some(peer) = host.peers.get_mut(&peer)
        {
            let _ = peer.send(&Msg::CharacterSave { tick, reason: reason.to_owned(), sections });
        }
        if world.get::<NetLeft>(body).is_none() {
            let out = call_tool(world, NET_AGENT, "player_leave", json!({ "player": player, "reason": reason }));
            if !out.ok {
                journal::emit(world, JournalKind::Warn, format!("net: p{player} left, but player_leave failed: {}", out.result));
            }
        }
    }
    // The slot is free only if this link still held it: a second game with the player's token
    // took it over already.
    if let Some(slot) = host.slots.get_mut(&player).filter(|s| s.peer == Some(peer)) {
        slot.peer = None;
        slot.left_at = Some(tick);
    }
    NetMarks::mark(world, |m| &mut m.left);
    journal::emit(world, JournalKind::Sim, format!("net: p{player} left ({reason})"));
}

/// A name a guest's agent may have on the host (`p1/<name>`).
fn agent_name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Checks a guest's call and sets up the agent it runs as: `p<n>/<name>`, at most
/// [`MAX_AGENTS`] per player, bound to the guest's own body (whichever entity that is at the
/// time) unless it may act as another (dev scope).
fn guest_agent(world: &mut World, peer: &mut Peer, call: &CallMsg, scope: Scope) -> Result<String, String> {
    // Only what a client forwards runs here: the host has no forwarder, so the route is
    // checked here or nowhere.
    match world.resource::<AiTools>().get(&call.tool).map(|t| t.spec.net) {
        Some(NetRoute::Host) => {}
        Some(NetRoute::Local) => return Err(format!("`{}` runs on your own game, not on the host", clip(&call.tool, 40))),
        Some(NetRoute::HostOnly(_)) => return Err(format!("`{}` is the host's own; ask the host to call it", clip(&call.tool, 40))),
        None => return Err(format!("the host has no tool `{}`", clip(&call.tool, 40))),
    }
    if !agent_name_ok(&call.agent) {
        return Err(format!("`{}` is not an agent name: 1 to {MAX_NAME} letters, digits, `_`, `-` or `.`", clip(&call.agent, MAX_NAME)));
    }
    if !peer.agents.contains(&call.agent) && peer.agents.len() >= MAX_AGENTS {
        let agents: Vec<&str> = peer.agents.iter().map(String::as_str).collect();
        return Err(format!("at most {MAX_AGENTS} agents per player call the host; yours are {}", agents.join(", ")));
    }
    // A guest acts as its own body, whatever subject its game set: a tool's own arguments
    // name other entities, and the scope decides what it may do to them.
    if call.subject.is_some() && call.subject != peer.body {
        let mine = peer.body.map_or_else(|| "none".to_owned(), |b| format!("e{b}"));
        return Err(format!(
            "your calls act as your own body ({mine}), not as e{}; clear the subject and name the entity in the tool's arguments",
            call.subject.unwrap_or_default()
        ));
    }
    let agent = format!("p{}/{}", peer.player, call.agent);
    let change = SessionChange { scope: Some(scope), subject: None, player: Some(peer.player), clear_subject: false };
    configure_session(world, &agent, &change)?;
    peer.agents.insert(call.agent.clone());
    Ok(agent)
}

fn forwarded(world: &mut World, host: &mut NetHost, id: PeerId, call: CallMsg) {
    let Some(peer) = host.peers.get_mut(&id) else { return };
    let def = net_def(world);
    let scope = call.scope.min(def.guest_scope);
    let reply = match guest_agent(world, peer, &call, scope) {
        Ok(agent) => {
            if call.scope > scope {
                debug!("net: {agent} asked with {} scope, runs with {scope}", call.scope);
            }
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
        let _ = peer.send(&reply);
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
            let _ = peer.send(line);
        }
    }
}

/// Not this tick again nor the next: time moved (a snapshot load). Every peer's inputs on
/// their way are of the old ticks until it gets the world again, however long that waits
/// (a full link, a loading gate) while the new ticks run on. Called at the start of a frame's
/// receiving and again before the rebases: a jump is seen before a join of the same frame is
/// admitted (which clears the flag, its world being the one after the jump), and never again.
fn note_ticks(world: &World, host: &mut NetHost) {
    let tick = sim::tick(world);
    if !(host.last_tick..=host.last_tick + 1).contains(&tick) {
        for peer in host.peers.values_mut() {
            peer.ticks_moved = true;
        }
    }
    host.last_tick = tick;
}

fn send_rebases(world: &mut World, host: &mut NetHost) {
    let tick = sim::tick(world);
    note_ticks(world, host);
    if world.get_resource::<SimGate>().is_some_and(SimGate::is_held) {
        return;
    }
    let mut captured: Option<Result<ai_core::Snapshot, String>> = None;
    let mut sent = Vec::new();
    let mut waiting = Vec::new();
    let mut desync = false;
    for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected && p.rebase.is_some()) {
        // Its link would refuse the world now (by count or by bytes, as large as the last
        // one sent): it waits, uncaptured, and the peer keeps its epoch (as its client does)
        // until it goes.
        if !rebase_fits(&peer.link.footprint(), peer.link.config(), host.rebase_bytes) {
            waiting.push((peer.player, "its link has too much unacknowledged".to_owned()));
            continue;
        }
        let snapshot = captured.get_or_insert_with(|| snapshot::capture(world)).clone();
        let Ok(snapshot) = snapshot else { continue };
        let reason = peer.rebase.clone().unwrap_or_default();
        // The player's body in this world, or none: never an id of the old world.
        let body = live_body(world, peer.player);
        // The host's ticks went on as they were (a resync, a new body, a region change):
        // inputs on their way are still good.
        let carry = !peer.ticks_moved;
        let welcome = Msg::Rebase(Welcome {
            player: peer.player,
            body,
            tick,
            reason: reason.clone(),
            snapshot,
            region: (hooks(world).describe_world)(world),
            carry_inputs: carry,
        });
        let bytes = msg::encode(&welcome);
        host.rebase_bytes = bytes.len();
        let kb = bytes.len() as f64 / 1024.0;
        if let Err(e) = peer.send_bytes(&bytes) {
            waiting.push((peer.player, e.to_string()));
            continue;
        }
        sent.push(format!("p{} gets the world again ({reason}, {kb:.1} KB)", peer.player));
        desync |= reason == "desync";
        peer.rebase = None;
        peer.body = body;
        peer.resyncs += 1;
        peer.ticks_moved = false;
        reset_view(peer);
        if !carry {
            // Its inputs start over on the new world's ticks: what it sent before is of
            // another timeline.
            peer.inputs = InputQueue::default();
            peer.accept_from_epoch = peer.resyncs;
            peer.margin = None;
            peer.applied_input_tick = 0;
            peer.watched.0 = 0;
            // So do its desync reports, and the cooldown of the resyncs they cause.
            peer.last_desync = None;
            peer.desync_resync_at = None;
        }
    }
    if desync {
        // A client saw something else: what is sent next is encoded afresh.
        host.cache.clear();
    }
    if let Some(Err(e)) = captured {
        journal::emit(world, JournalKind::Error, format!("net: the world cannot be sent again: {e}"));
    }
    for line in sent {
        journal::emit(world, JournalKind::Sim, format!("net: {line}"));
    }
    for (player, why) in waiting {
        crate::hooks::net_line(world, &format!("rebase p{player}"), JournalKind::Warn, format!("p{player} cannot be sent the world now ({why}); it goes at a later send"));
    }
}

/// Whether a link with `footprint` takes a message of `bytes` now (as `send_reliable` would).
pub(crate) fn rebase_fits(footprint: &crate::link::Footprint, config: &crate::link::LinkConfig, bytes: usize) -> bool {
    footprint.reliable_out < config.max_reliable_messages && footprint.reliable_out_bytes + bytes <= config.max_reliable_bytes
}

/// Once a second: a peer's loss over 5 %, its RTT jumping, its inputs coming late, messages
/// its link would not take (journal, rate-limited).
fn watch_links(world: &mut World, host: &mut NetHost) {
    let hz = (1.0 / sim::tick_seconds(world).max(1e-6)).round() as u64;
    if !sim::tick(world).is_multiple_of(hz.max(1)) {
        return;
    }
    let mut lines = Vec::new();
    for peer in host.peers.values_mut().filter(|p| p.stage == PeerStage::Connected) {
        let stats = peer.link.stats();
        let (late, srtt) = peer.watched;
        let p = peer.player;
        if stats.loss > 0.05 {
            lines.push((format!("loss p{p}"), JournalKind::Warn, format!("p{p} loses {:.1}% of datagrams", stats.loss * 100.0)));
        }
        if let Some(text) = rtt_jump(srtt, stats.srtt_ms) {
            lines.push((format!("rtt p{p}"), JournalKind::Sim, format!("p{p} {text}")));
        }
        if peer.inputs.late > late {
            lines.push((format!("late p{p}"), JournalKind::Warn, format!("p{p}: {} inputs came late in the last second", peer.inputs.late - late)));
        }
        if peer.unsent > peer.unsent_said {
            let n = peer.unsent - peer.unsent_said;
            lines.push((format!("unsent p{p}"), JournalKind::Warn, format!("p{p}: {n} messages not sent in the last second (its link has too much unacknowledged)")));
            peer.unsent_said = peer.unsent;
        }
        peer.watched = (peer.inputs.late, stats.srtt_ms);
    }
    for (kind, level, text) in lines {
        crate::hooks::net_line(world, &kind, level, text);
    }
}

/// `rtt 40 → 180 ms` when the round trip changed by half and 50 ms or more.
pub(crate) fn rtt_jump(before: f32, now: f32) -> Option<String> {
    let jumped = before > 0.0 && (now - before).abs() >= 50.0 && (now - before).abs() >= before * 0.5;
    jumped.then(|| format!("rtt {before:.0} → {now:.0} ms"))
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
    // Every value written is read back (the encoder's rule): one that does not (a NaN, an
    // infinity) is left out for every client, as a snapshot leaves it out.
    let shared = &*world;
    let encoder = StateEncoder::new(shared, ai_core::state::StateRule::Replicated);
    let mut capped_first = Vec::new();
    let mut unsent_states = Vec::new();
    for (id, center, mut view) in views {
        let peer = host.peers.get_mut(&id).expect("listed above");
        // A capped view names entities the client does not get: its copy cannot hash as the
        // host's, so it is not checked (as a view held back is not).
        let capped = close_view(shared, &encoder, &mut host.cache, &mut encoded, &mut view);
        // The entities the cap kept out are in the view now (the player came closer): the
        // references to them that the client read as nothing do not change in any delta, so the
        // client is given the world once more, and its copy is not checked before it has it.
        let completed = peer.capped && !capped;
        peer.capped = capped;
        if completed {
            peer.rebase.get_or_insert_with(|| "view complete".into());
        }
        if capped {
            peer.closure_capped += 1;
            if peer.closure_capped == 1 {
                capped_first.push(peer.player);
            }
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
            with_hash: with_hash && !capped && !completed,
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
            unsent_states.push((peer.player, e.to_string()));
        }
        peer.deltas += 1;
        peer.delta_bytes += built.bytes as u64;
        peer.held_back += built.held_back.len() as u64;
        for (path, bytes) in built.by_component {
            *peer.by_component.entry(path).or_default() += bytes;
        }
        peer.view = view;
        peer.sent.push_back((tick, Arc::new(built.sent)));
        while peer.sent.len() > SENT_KEPT {
            peer.sent.pop_front();
        }
    }
    drop(encoder);
    host.cache.end_send();
    let us = started.elapsed().as_micros() as u64;
    host.encode_us = (us, host.encode_us.1.max(us));
    host.encode_total = (host.encode_total.0 + us, host.encode_total.1 + 1);
    for (player, why) in unsent_states {
        crate::hooks::net_line(world, &format!("state p{player}"), JournalKind::Warn, format!("state for p{player} not sent: {why}"));
    }
    for player in capped_first {
        journal::emit(
            world,
            JournalKind::Warn,
            format!("net: p{player}'s view names over {CLOSURE_CAP} entities beyond it: they are not sent, and its copy is not checked for desyncs while that lasts"),
        );
    }
    for (path, (id, why)) in encoded.left_out {
        if host.left_out_said.insert(path.clone()) {
            let short = path.rsplit("::").next().unwrap_or(&path);
            journal::emit(world, JournalKind::Warn, format!("net: {short} of e{id} is not sent to clients ({why}); JSON has no NaN or infinity, so they go without it"));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{rebase_fits, rtt_jump, NetHost, Slot};
    use crate::link::{Footprint, LinkConfig};

    #[test]
    fn a_world_waits_for_room_by_count_and_by_bytes() {
        let config = LinkConfig::default();
        let empty = Footprint::default();
        assert!(rebase_fits(&empty, &config, 2 << 20));
        let full = Footprint { reliable_out: config.max_reliable_messages, ..Footprint::default() };
        assert!(!rebase_fits(&full, &config, 1));
        // Few messages, but worlds the peer has not acknowledged: full by bytes.
        let heavy = Footprint { reliable_out: 3, reliable_out_bytes: config.max_reliable_bytes - (1 << 20), ..Footprint::default() };
        assert!(!rebase_fits(&heavy, &config, 2 << 20));
        assert!(rebase_fits(&heavy, &config, 1 << 20));
    }

    #[test]
    fn an_rtt_jump_is_half_again_and_50_ms() {
        assert_eq!(rtt_jump(40.0, 180.0).as_deref(), Some("rtt 40 → 180 ms"));
        assert_eq!(rtt_jump(300.0, 180.0).as_deref(), None, "120 ms less is not half of 300");
        assert_eq!(rtt_jump(20.0, 60.0).as_deref(), None, "40 ms is noise");
        assert_eq!(rtt_jump(0.0, 300.0).as_deref(), None, "the first measure is not a jump");
    }

    #[test]
    fn a_retired_token_is_remembered_bounded_and_never_issued_again() {
        let mut host = NetHost::new();
        let first = host.issue_token(7, 1, "player");
        host.retire(first.clone());
        host.retire(first.clone());
        host.retire(String::new());
        assert_eq!(host.retired, std::slice::from_ref(&first), "once, and an empty token is no one's");
        // Issuing the same inputs again would give the same token (a stand repeats): it is skipped.
        host.issued = 0;
        assert_ne!(host.issue_token(7, 1, "player"), first);
        for n in 0..NetHost::RETIRED_KEPT + 5 {
            host.retire(format!("t{n}"));
        }
        assert_eq!(host.retired.len(), NetHost::RETIRED_KEPT);
        assert!(!host.retired.contains(&first), "the oldest is forgotten first");
    }

    #[test]
    fn a_token_is_the_hosts_own_and_no_two_players_share_one() {
        let hex = |t: &str| t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit());
        // A stand (runs repeat): two players of one name still get two tokens.
        let mut host = NetHost::new();
        let first = host.issue_token(7, 1, "player");
        host.slots.insert(1, Slot { token: first.clone(), name: "player".into(), peer: None, left_at: None });
        let second = host.issue_token(7, 2, "player");
        assert!(hex(&first) && hex(&second), "{first} {second}");
        assert_ne!(first, second);
        assert!(!first.contains("player"));
        // Over a real network: random, so another host (or run) issues another one.
        let mut lan = NetHost { wall_clock: true, ..NetHost::new() };
        let mut other = NetHost { wall_clock: true, ..NetHost::new() };
        let (a, b) = (lan.issue_token(7, 1, "player"), other.issue_token(7, 1, "player"));
        assert!(hex(&a) && a != b, "{a} {b}");
    }
}
