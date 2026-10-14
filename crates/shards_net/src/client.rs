//! The client's side of a session: the join handshake, taking the host's world, applying its
//! state, sending the own body's inputs, the desync check and the tick clock.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ai_core::journal::{self, JournalKind};
use ai_core::net_rules::{self, NetRules};
use ai_core::snapshot::{self, ENGINE_VERSION};
use ai_core::state::decode_value;
use ai_core::tools::{ForwardedCall, Forwarder};
use ai_core::{sim, AiId, AiIndex, AiPlayer, AiTools, SimGate, ToolResult};
use bevy::prelude::*;
use bevy::reflect::{FromReflect, TypePath};
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::{EngineConfig, EngineState, Intent, LocalPlayer, NetPrediction, Predicted};

use crate::def::net_def;
use crate::hooks::{hooks, PlayerLeft};
use crate::inputs::InputCapture;
use crate::link::{Delivered, Link, Role};
use crate::msg::{self, CallMsg, Hello, Inputs, Msg, StateDelta, Welcome, PROTO};
use crate::predict::NetCorrection;
use crate::replicate::{apply, changes_between, first_difference, world_hash, ClientRing, Fnv, LOCAL_BASE};
use crate::session::{NetClock, NetEndpoint, NetGame, NetMarks, NetState, Ticked};

/// Where a client is in the session.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientStage {
    /// Loading content before saying hello.
    Connecting,
    /// Said hello; waits for the host's answer.
    HelloSent,
    /// The host's game said "not now".
    Waiting(String),
    /// Has the seed: builds the world from it.
    Accepted,
    /// Built the world; waits for the host's.
    ReadySent,
    Connected,
    Ended(String),
}

impl ClientStage {
    /// Whether the client waits for the network (and not for its own loading).
    pub fn waits_on_network(&self) -> bool {
        matches!(self, ClientStage::HelloSent | ClientStage::Waiting(_) | ClientStage::ReadySent | ClientStage::Connected | ClientStage::Ended(_))
    }
}

/// How many ticks a client runs per host tick to keep its inputs arriving `target` ticks
/// early: the host reports each input packet's margin; adjustments made since that packet
/// count on top.
#[derive(Clone, Debug, Default)]
pub struct ClockSync {
    adjustments: VecDeque<(u64, i64)>,
    report: Option<(u64, i64)>,
    /// Ticks early the inputs should arrive.
    pub target: i64,
    /// Ticks run besides one per host tick (+) or skipped (−), all told.
    pub adjusted: i64,
}

impl ClockSync {
    pub fn report(&mut self, margin: (u64, i64)) {
        if self.report.is_none_or(|(at, _)| margin.0 >= at) {
            self.report = Some(margin);
            self.adjustments.retain(|(t, _)| *t > margin.0);
        }
    }

    /// How far the inputs are from arriving `target` ticks early (+: too early).
    pub fn error(&self) -> Option<i64> {
        let (at, margin) = self.report?;
        let since: i64 = self.adjustments.iter().filter(|(t, _)| *t > at).map(|(_, d)| d).sum();
        // Saturating: the margin is the host's number, whatever it says.
        Some(margin.saturating_add(since).saturating_sub(self.target))
    }

    /// Ticks to run now, at client tick `now`: 2 when late, 0 when too early, else 1.
    pub fn ticks(&mut self, now: u64) -> u32 {
        let k: i64 = match self.error() {
            Some(e) if e < -1 => 2,
            Some(e) if e > 1 => 0,
            _ => 1,
        };
        if k != 1 {
            self.adjustments.push_back((now, k - 1));
            self.adjusted += k - 1;
        }
        k as u32
    }

    pub fn reset(&mut self, target: i64) {
        *self = ClockSync { target, adjusted: self.adjusted, ..default() };
    }
}

/// The last desync this client found.
#[derive(Resource, Clone, Debug, Default)]
pub struct NetDesync {
    pub count: u32,
    /// Host tick, client tick, the first difference (`e12 Health: here …, host …`).
    pub last: Option<(u64, u64, String)>,
}

/// The client's session.
#[derive(Resource)]
pub struct NetClient {
    pub address: String,
    pub stage: ClientStage,
    pub(crate) link: Option<Link>,
    session: u32,
    /// Who this player is to the host: issued at the first join and kept beside the saves
    /// ([`token_store`]); empty until then.
    pub token: String,
    pub name: String,
    /// The character to bring (save sections); co-op saves set it before the join.
    pub character: Option<Value>,
    pub player: Option<u8>,
    pub body: Option<u32>,
    pub ring: ClientRing,
    pending: Option<StateDelta>,
    pub capture: InputCapture,
    pub clock: ClockSync,
    /// The newest host tick a state came from.
    pub newest_host_tick: u64,
    /// States from at or before this host tick belong to a world replaced since.
    floor: u64,
    pub lead: u64,
    pub rebases: u32,
    next_call: u64,
    calls: BTreeMap<u64, String>,
    pub host_paused: bool,
    /// Deltas applied and skipped, and their bytes.
    pub deltas: u64,
    pub skipped: u64,
    pub delta_bytes: u64,
    /// The RTT the link watch saw a second ago.
    watched_rtt: f32,
}

impl NetClient {
    pub fn new(address: &str, config: &EngineConfig) -> Self {
        let name = config.player_name.clone();
        Self {
            address: address.to_owned(),
            stage: ClientStage::Connecting,
            link: None,
            session: 0,
            token: String::new(),
            name,
            character: None,
            player: None,
            body: None,
            ring: ClientRing::default(),
            pending: None,
            capture: InputCapture::default(),
            clock: ClockSync::default(),
            newest_host_tick: 0,
            floor: 0,
            lead: 0,
            rebases: 0,
            next_call: 0,
            calls: BTreeMap::new(),
            host_paused: false,
            deltas: 0,
            skipped: 0,
            watched_rtt: 0.0,
            delta_bytes: 0,
        }
    }

    pub fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    fn send(&mut self, message: &Msg) {
        if let Some(link) = self.link.as_mut()
            && let Err(e) = link.send_reliable(&msg::encode(message))
        {
            warn!("net: cannot send to the host: {e}");
        }
    }
}

fn with_client(world: &mut World, f: impl FnOnce(&mut World, &mut NetClient, &mut NetEndpoint)) {
    let Some(mut client) = world.remove_resource::<NetClient>() else { return };
    let Some(mut endpoint) = world.remove_resource::<NetEndpoint>() else {
        world.insert_resource(client);
        return;
    };
    f(world, &mut client, &mut endpoint);
    world.insert_resource(endpoint);
    world.insert_resource(client);
}

/// Ticks a client may run beyond the newest host state and its lead before it waits.
pub const STARVE_AHEAD: u64 = 30;

/// Ticks a stand runs this client for one host tick: 0 until it is connected, then what its
/// clock says (0, 1 or 2), and 0 while it is far ahead of the newest state (starved).
pub fn clock_ticks(world: &mut World) -> u32 {
    let tick = sim::tick(world);
    let Some(mut client) = world.get_resource_mut::<NetClient>() else { return 1 };
    if client.stage != ClientStage::Connected {
        return 0;
    }
    if tick > client.newest_host_tick + client.lead + STARVE_AHEAD {
        return 0;
    }
    client.clock.ticks(tick)
}

/// Whether the client is busy with its own loading (the stand waits for it, the hub's clock
/// stands still meanwhile).
pub fn loading_locally(world: &World) -> bool {
    world.get_resource::<NetClient>().is_some_and(|c| !c.stage.waits_on_network())
}

// --- systems -----------------------------------------------------------------------------

pub(crate) fn receive(world: &mut World) {
    world.resource_mut::<Ticked>().0 = true;
    with_client(world, io_in);
}

pub(crate) fn pump(world: &mut World) {
    with_client(world, |world, client, endpoint| {
        io_in(world, client, endpoint);
        // A starved client in real time is paused: only frames without a tick unpause it.
        if endpoint.clock.is_wall() {
            pace(world, client);
        }
        flush(client, endpoint);
    });
}

/// `NetSystems::ApplyState`: the newest state into the world, then the desync check.
pub(crate) fn apply_state(world: &mut World) {
    let Some(mut client) = world.remove_resource::<NetClient>() else { return };
    apply_pending(world, &mut client);
    world.insert_resource(client);
}

/// `NetSystems::Capture`: the own body's intent as an input frame, and every frame the host
/// has not acknowledged.
pub(crate) fn capture(world: &mut World) {
    let Some(mut client) = world.remove_resource::<NetClient>() else { return };
    if client.stage == ClientStage::Connected {
        let tick = sim::tick(world);
        let redundancy = net_def(world).input_redundancy as usize;
        if let Some(body) = client.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id)))
            && let Some(mut intent) = world.get_mut::<Intent>(body)
        {
            client.capture.capture(tick, &mut intent, redundancy);
        }
        let inputs = Inputs { tick, frames: client.capture.unacked.iter().cloned().collect(), state_ack: client.ring.last_applied.unwrap_or(0), epoch: client.rebases };
        if let Some(link) = client.link.as_mut() {
            let _ = link.send_unreliable(&msg::encode(&inputs));
        }
    }
    world.insert_resource(client);
}

/// `NetSystems::Send`.
pub(crate) fn send(world: &mut World) {
    with_client(world, |world, client, endpoint| {
        if endpoint.clock.is_wall() {
            pace(world, client);
        }
        watch_link(world, client);
        flush(client, endpoint);
    });
}

/// Once a second: loss over 5 % and RTT jumps (journal, rate-limited).
fn watch_link(world: &mut World, client: &mut NetClient) {
    let hz = (1.0 / sim::tick_seconds(world).max(1e-6)).round() as u64;
    if client.stage != ClientStage::Connected || !sim::tick(world).is_multiple_of(hz.max(1)) {
        return;
    }
    let Some(stats) = client.link.as_ref().map(Link::stats) else { return };
    if stats.loss > 0.05 {
        crate::hooks::net_line(world, "loss", JournalKind::Warn, format!("{:.1}% of the host's datagrams lost", stats.loss * 100.0));
    }
    if let Some(text) = crate::host::rtt_jump(client.watched_rtt, stats.srtt_ms) {
        crate::hooks::net_line(world, "rtt", JournalKind::Sim, text);
    }
    client.watched_rtt = stats.srtt_ms;
}

/// Why a client leaves when its player quits: the `Bye`'s reason, and the reason of the character
/// the host sends back as the player leaves.
const QUIT: &str = "quit";

/// Exit hook: the host is told.
pub(crate) fn on_exit(world: &mut World) {
    with_client(world, |world, client, endpoint| {
        if matches!(client.stage, ClientStage::Ended(_)) || client.link.is_none() {
            return;
        }
        client.send(&Msg::Bye { reason: QUIT.into() });
        flush(client, endpoint);
        // Over UDP, give the `Bye` 300 ms to get out and be acknowledged, and the host's last
        // word to us, our character as it leaves (`CharacterSave`), to arrive: it is final.
        if endpoint.clock.is_wall() {
            let started = std::time::Instant::now();
            let mut final_save = false;
            while started.elapsed() < std::time::Duration::from_millis(300) && (!final_save || client.link.as_ref().is_some_and(|l| !l.is_idle())) {
                std::thread::sleep(std::time::Duration::from_millis(5));
                let mut datagrams = Vec::new();
                let now = endpoint.clock.now();
                endpoint.transport.recv(now, &mut datagrams);
                let mut delivered = Vec::new();
                if let Some(link) = client.link.as_mut() {
                    for (_, bytes) in datagrams {
                        link.on_datagram(now, &bytes);
                    }
                    delivered = link.poll(now);
                }
                for message in delivered {
                    if let Delivered::Reliable(bytes) = message
                        && let Ok(Msg::CharacterSave { tick, reason, sections }) = msg::decode::<Msg>(&bytes)
                    {
                        let save = hooks(world).on_character_save;
                        save(world, tick, &reason, &sections);
                        // The answer to our `Bye` (the host leaves with the reason we gave), not
                        // a save that was on its way when we quit.
                        final_save |= reason == QUIT;
                    }
                }
                flush(client, endpoint);
            }
        }
        client.stage = ClientStage::Ended(QUIT.into());
    });
}

/// Ticks the inputs may be off before the clock does more than nudge its speed.
pub const PACE_FAR: i64 = 30;

/// How fast a client runs when its inputs arrive more than [`PACE_FAR`] ticks early: it
/// slows down hard instead of going back.
pub const PACE_BRAKE: f32 = 0.5;

/// What the client's clock does about an error (+: the inputs arrive too early).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Pace {
    /// Runs at this speed.
    Speed(f32),
    /// Jumps forward to the host's newest tick plus the lead.
    Skip,
}

/// The speed that brings an error of a few ticks back: slower when early, faster when late.
fn nudge(error: i64) -> f32 {
    (1.0 - error as f32 * 0.02).clamp(0.9, 1.1)
}

/// Late by a lot: skip ahead. Early by a lot: brake. Else nudge the speed.
///
/// The clock never goes back. Frames the client sent for the ticks it would run again are on
/// the host already, waiting for their ticks: it drops a second frame of a tick it has as a
/// duplicate, with the presses the client took out of its `Intent` for it, and applies the
/// old one after the new ones that come before it. A jump forward lands on ticks nothing
/// was sent for.
fn pace_for(error: i64) -> Pace {
    if error < -PACE_FAR {
        Pace::Skip
    } else if error > PACE_FAR {
        Pace::Speed(PACE_BRAKE)
    } else {
        Pace::Speed(nudge(error))
    }
}

/// Real time over UDP: run slightly faster or slower to keep the inputs early, skip ahead
/// when far behind, brake when far ahead.
fn pace(world: &mut World, client: &mut NetClient) {
    if client.stage != ClientStage::Connected {
        return;
    }
    // Far ahead of the newest state (the host is paused, steps, or hangs): wait for it.
    let starved = sim::tick(world) > client.newest_host_tick + client.lead + STARVE_AHEAD;
    let mut time = world.resource_mut::<Time<Virtual>>();
    if starved != time.is_paused() {
        if starved {
            time.pause();
        } else {
            time.unpause();
        }
    }
    if starved {
        return;
    }
    let Some(error) = client.clock.error() else { return };
    let speed = match pace_for(error) {
        Pace::Speed(speed) => speed,
        Pace::Skip => {
            // Forward only: the newest state can be older than this clock.
            let tick = client.newest_host_tick + client.lead;
            if tick > sim::tick(world) {
                sim::set_tick(world, tick);
                client.clock.reset(client.clock.target);
                crate::hooks::net_line(world, "clock", JournalKind::Warn, format!("clock resync to tick {tick} (was {error:+} ticks off)"));
                return;
            }
            nudge(error)
        }
    };
    world.resource_mut::<Time<Virtual>>().set_relative_speed(speed);
}

/// How long an ended client keeps its link once the host went quiet: it acknowledges what the
/// host still sends (a `SessionEnd`, a `Bye`), then stops sending, keepalives too.
pub const ENDED_PATIENCE_MS: u64 = 3000;

fn flush(client: &mut NetClient, endpoint: &mut NetEndpoint) {
    let (Some(link), Some(host)) = (client.link.as_mut(), endpoint.host) else { return };
    let now = endpoint.clock.now();
    let transport = &mut endpoint.transport;
    link.flush(now, &mut |datagram| transport.send(host, datagram));
    if matches!(client.stage, ClientStage::Ended(_)) && link.is_timed_out(now, ENDED_PATIENCE_MS) {
        client.link = None;
    }
}

// --- receiving -----------------------------------------------------------------------------

fn set_state(world: &mut World, state: NetState) {
    world.insert_resource(state);
}

fn io_in(world: &mut World, client: &mut NetClient, endpoint: &mut NetEndpoint) {
    let now = endpoint.clock.now();
    let Some(host) = endpoint.host else { return };
    if client.stage == ClientStage::Connecting && world.get_resource::<ContentDb>().is_some_and(ContentDb::is_loaded) {
        hello(world, client, &endpoint.clock);
        set_state(world, NetState::Joining(format!("waiting for the host {}", client.address)));
        shards_core::seed::hold_seed(world, &format!("net: waiting for the host {}", client.address));
    }
    let mut datagrams = Vec::new();
    endpoint.transport.recv(now, &mut datagrams);
    let Some(link) = client.link.as_mut() else { return };
    for (from, bytes) in datagrams {
        if from == host {
            link.on_datagram(now, &bytes);
        }
    }
    let delivered = link.poll(now);
    for message in delivered {
        match message {
            Delivered::Reliable(bytes) => match msg::decode::<Msg>(&bytes) {
                Ok(m) => on_message(world, client, &endpoint.clock, m),
                Err(e) => crate::hooks::net_line(world, "undecodable", JournalKind::Warn, format!("from the host: {e}")),
            },
            Delivered::Unreliable(bytes) => match msg::decode::<StateDelta>(&bytes) {
                Ok(delta) => {
                    client.delta_bytes += bytes.len() as u64;
                    let current = delta.epoch == client.rebases;
                    if current && delta.tick > client.floor && client.pending.as_ref().is_none_or(|p| delta.tick > p.tick) {
                        client.pending = Some(delta);
                    }
                }
                Err(e) => crate::hooks::net_line(world, "undecodable", JournalKind::Warn, format!("from the host: {e}")),
            },
        }
    }
    let timeout_ms = (net_def(world).timeout_secs * 1000.0) as u64;
    let live = !matches!(client.stage, ClientStage::Ended(_));
    if live && client.link.as_ref().is_some_and(|l| l.is_timed_out(now, timeout_ms)) {
        let secs = net_def(world).timeout_secs;
        end(world, client, &format!("lost the host {} (no answer for {secs:.0} s)", client.address), AppExit::error());
    }
    // The world is built from the seed: ask for the host's.
    if client.stage == ClientStage::Accepted && world_built(world) {
        let checks = snapshot::capture(world).map(|s| s.checks).unwrap_or_default();
        client.send(&Msg::Ready { checks });
        client.stage = ClientStage::ReadySent;
        set_state(world, NetState::Joining("waiting for the host's world".into()));
    }
}

/// In game, and nothing but the session holds the simulation.
fn world_built(world: &World) -> bool {
    let in_game = world.get_resource::<State<EngineState>>().is_some_and(|s| *s.get() == EngineState::InGame);
    let gate = world.get_resource::<SimGate>();
    in_game && gate.is_some_and(|g| g.reasons().len() == 1 && g.reason("net").is_some())
}

/// Where a client keeps the token its host issued, so that a restart (a crash, a quit) takes
/// its slot and body back: a stand's hub, which outlives its clients; over UDP the file
/// `<save_dir>/net/<name>@<host>.token`.
pub fn token_store(world: &World, client: &NetClient) -> std::path::PathBuf {
    let dir = world.get_resource::<EngineConfig>().map_or_else(|| "saves".to_owned(), |c| c.save_dir.clone());
    let safe = |text: &str| -> String { text.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect() };
    std::path::Path::new(&dir).join("net").join(format!("{}@{}.token", safe(&client.name), safe(&client.address)))
}

fn token_key(client: &NetClient) -> String {
    format!("token {}@{}", client.name, client.address)
}

fn load_token(world: &World, client: &NetClient, clock: &NetClock) -> String {
    let token = match clock {
        NetClock::Hub(hub) => hub.kept(&token_key(client)),
        NetClock::Wall(_) => std::fs::read_to_string(token_store(world, client)).ok(),
    };
    token.map(|t| t.trim().to_owned()).filter(|t| t.len() <= crate::host::MAX_TOKEN).unwrap_or_default()
}

fn keep_token(world: &mut World, client: &NetClient, clock: &NetClock) {
    match clock {
        NetClock::Hub(hub) => hub.keep(&token_key(client), &client.token),
        NetClock::Wall(_) => {
            let path = token_store(world, client);
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, &client.token));
            if let Err(e) = written {
                journal::emit(world, JournalKind::Warn, format!("net: cannot keep the host's token in {}: {e}; a restart joins as a new player", path.display()));
            }
        }
    }
}

fn hello(world: &mut World, client: &mut NetClient, clock: &NetClock) {
    client.token = load_token(world, client, clock);
    // The link's session tells this run's datagrams from an earlier run's: random over a real
    // network, the same in every run of a stand.
    client.session = match clock {
        NetClock::Hub(_) => {
            let mut h = Fnv::default();
            h.eat(client.name.as_bytes());
            h.0 as u32
        }
        NetClock::Wall(_) => crate::host::random_word(3, u64::from(std::process::id())) as u32,
    } | 1;
    client.link = Some(Link::new(client.session, Role::Client));
    let hello = Hello {
        proto: PROTO,
        engine: ENGINE_VERSION.to_owned(),
        game: world.get_resource::<NetGame>().cloned().unwrap_or_default().0,
        content_hash: world.get_resource::<ContentDb>().map_or(0, ContentDb::hash),
        net_rules_hash: net_rules::rules_hash(world),
        name: client.name.clone(),
        token: client.token.clone(),
        character: client.character.clone(),
        tick_hz: 1.0 / sim::tick_seconds(world) as f64,
    };
    client.send(&Msg::Hello(hello));
    client.stage = ClientStage::HelloSent;
}

fn on_message(world: &mut World, client: &mut NetClient, clock: &NetClock, message: Msg) {
    match message {
        Msg::Accept(accept) => {
            if client.stage != ClientStage::HelloSent && !matches!(client.stage, ClientStage::Waiting(_)) {
                return;
            }
            if !accept.token.is_empty() && accept.token != client.token && accept.token.len() <= crate::host::MAX_TOKEN {
                client.token = accept.token.clone();
                keep_token(world, client, clock);
            }
            client.player = Some(accept.player);
            world.get_resource_or_init::<SimGate>().hold("net", "net: waiting for the host's world");
            shards_core::seed::adopt_seed(world, accept.seed);
            client.stage = ClientStage::Accepted;
            set_state(world, NetState::Joining(format!("building the world (seed {})", accept.seed)));
            journal::emit(world, JournalKind::Sim, format!("net: accepted as p{} by {} (seed {})", accept.player, client.address, accept.seed));
        }
        Msg::Wait { reason } => {
            client.stage = ClientStage::Waiting(reason.clone());
            shards_core::seed::hold_seed(world, &format!("net: waiting to join: {reason}"));
            set_state(world, NetState::Joining(format!("waiting to join: {reason}")));
            crate::hooks::net_line(world, "wait", JournalKind::Sim, format!("waiting to join: {reason}"));
        }
        Msg::Refused { reason } => {
            journal::emit(world, JournalKind::Error, format!("net: refused by the host {}: {reason}", client.address));
            end(world, client, &format!("refused: {reason}"), AppExit::error());
        }
        Msg::Welcome(welcome) => take_world(world, client, welcome, false),
        Msg::Rebase(welcome) => {
            if client.stage == ClientStage::Connected {
                take_world(world, client, welcome, true);
            }
        }
        Msg::Journal { tick, text, .. } => {
            journal::push_remote(world, tick, JournalKind::Event, text);
        }
        Msg::Event { tick, type_path, value } => {
            if let Some(mut inbox) = world.get_resource_mut::<crate::events::EventInbox>() {
                inbox.0.push_back((tick, type_path, value));
            }
        }
        Msg::CallReply { id, ok, text, tick } => {
            let tool = client.calls.remove(&id).unwrap_or_else(|| "call".into());
            let verdict = if ok { "ok" } else { "failed" };
            journal::log(world, JournalKind::Agent, format!("host: {tool} {verdict} at host tick {tick}: {text}"));
        }
        Msg::CharacterSave { tick, reason, sections } => {
            let save = hooks(world).on_character_save;
            save(world, tick, &reason, &sections);
        }
        Msg::SessionEnd { reason, saved_slot } => {
            let saved = saved_slot.map(|slot| format!(", world saved: {}", crate::host::clip(&slot, 40))).unwrap_or_default();
            journal::emit(world, JournalKind::Warn, format!("net: session ended by host ({reason}){saved}"));
            let body = client.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id)));
            if let (Some(player), Some(body)) = (client.player, body) {
                world.trigger(PlayerLeft { player, body, reason: "session ended".into() });
            }
            end(world, client, &format!("session ended by host ({reason})"), AppExit::Success);
        }
        Msg::Bye { reason } => {
            journal::emit(world, JournalKind::Warn, format!("net: the host sent us away ({reason})"));
            end(world, client, &reason, AppExit::error());
        }
        Msg::Paused(paused) => client.host_paused = paused,
        _ => {}
    }
}

/// The session is over for this client (`net_disconnected`, whatever the reason), once. With
/// `--exit-with-host` the game exits with `exit`: 0 when the host ended the session, not 0 when
/// it was refused, sent away, lost the host or could not take its world.
pub(crate) fn end(world: &mut World, client: &mut NetClient, reason: &str, exit: AppExit) {
    if matches!(client.stage, ClientStage::Ended(_)) {
        return;
    }
    client.stage = ClientStage::Ended(reason.to_owned());
    NetMarks::mark(world, |m| &mut m.disconnected);
    world.get_resource_or_init::<SimGate>().hold("net", format!("net: {reason}"));
    set_state(world, NetState::Ended(reason.to_owned()));
    if world.get_resource::<EngineConfig>().is_some_and(|c| c.exit_with_host) {
        world.write_message(exit);
    }
}

/// Puts the host's world in (a join, or the whole world again).
fn take_world(world: &mut World, client: &mut NetClient, welcome: Welcome, rebase: bool) {
    if rebase {
        world.get_resource_or_init::<SimGate>().hold("net", "net: taking the host's world again");
    }
    let carry = rebase && welcome.carry_inputs;
    // Presses not captured yet (an agent's dodge since the last tick) go to the new body.
    let unsent: Vec<shards_core::Press> = match client.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id))) {
        Some(body) if carry => world.get::<Intent>(body).map(|i| i.pressed.clone()).unwrap_or_default(),
        _ => Vec::new(),
    };
    // The restore sets the clock to the host's tick: what it was here is read first.
    let before = sim::tick(world);
    if let Err(e) = snapshot::restore_releasing(world, &welcome.snapshot, "net") {
        journal::emit(world, JournalKind::Error, format!("net: cannot take the host's world: {e}"));
        client.send(&Msg::Bye { reason: format!("cannot take the world: {e}") });
        end(world, client, &format!("cannot take the host's world: {e}"), AppExit::error());
        return;
    }
    // Whose body is whose: this machine's keyboard, camera and agents drive its own.
    let marked: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, Or<(With<AiPlayer>, With<LocalPlayer>, With<Predicted>)>>();
        q.iter(world).collect()
    };
    for entity in marked {
        world.entity_mut(entity).remove::<(AiPlayer, LocalPlayer, Predicted)>();
    }
    let body = welcome.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id)));
    if let Some(body) = body {
        world.entity_mut(body).insert((AiPlayer, LocalPlayer, Predicted));
    }
    let after = hooks(world).after_restore;
    after(world);
    world.resource_mut::<AiIndex>().set_local_base(LOCAL_BASE);
    ai_core::replay::set_recording(world, false);
    journal::set_local_events(world, false);

    let (lead, jitter) = lead_ticks(world, client);
    // A rebase may move the host's tick either way (a snapshot load): inputs and states of
    // the old world are told apart by the epoch (`rebases`), not by their ticks. When the
    // host's ticks did not move, the inputs sent after its tick still come, and this clock
    // does not go back (a frame stamped again would be a duplicate there).
    let tick = if carry { (welcome.tick + lead).max(before) } else { welcome.tick + lead };
    sim::set_tick(world, tick);
    client.lead = lead;
    client.clock.reset(2 + jitter as i64);
    client.ring.clear();
    client.pending = None;
    // The channel is ordered and the host sends its events before the world that replaces
    // theirs, so every event waiting now is of the old world, whatever its tick: it names
    // entities that are gone, or that the same ids give to others (a snapshot of an earlier
    // tick restores the id counter). Its tick can also be above the new world's, and
    // `retrigger_due` goes by order: it would hold the new world's events back. The new
    // world's come after this message.
    if rebase && let Some(mut inbox) = world.get_resource_mut::<crate::events::EventInbox>() {
        inbox.0.clear();
    }
    client.floor = welcome.tick;
    client.newest_host_tick = welcome.tick;
    // The own body is the host's again: nothing predicted before counts.
    if let Some(mut prediction) = world.get_resource_mut::<NetPrediction>() {
        prediction.host_tick = welcome.tick;
        prediction.lead = lead;
        prediction.snap = None;
        prediction.epoch += 1;
    }
    if let Some(mut correction) = world.get_resource_mut::<NetCorrection>() {
        correction.restart();
    }
    client.player = Some(welcome.player);
    client.body = welcome.body;
    if carry {
        // The body as this machine holds it: the host has (or will get) every frame of it,
        // and what the host changed itself (a region change stops its players) is sent again.
        // The orders the host finished by its world's tick (or cleared) are finished for the
        // prediction too, until `adopt_orders` takes the host's queue over.
        let orders = body.and_then(|b| world.get_mut::<Intent>(b)).map(|mut intent| {
            let host = intent.orders.clone();
            client.capture.hold(&mut intent);
            intent.pressed.extend(unsent);
            (intent.orders.clone(), host)
        });
        if let Some((mine, host)) = orders
            && let Some(mut correction) = world.get_resource_mut::<NetCorrection>()
        {
            correction.carry_orders(&mine, &host);
        }
    } else {
        client.capture = body.and_then(|b| world.get::<Intent>(b)).map(InputCapture::starting_from).unwrap_or_default();
    }
    client.stage = ClientStage::Connected;
    if let Some(mut gate) = world.get_resource_mut::<SimGate>() {
        gate.release("net");
    }
    set_state(world, NetState::Connected { player: welcome.player, body: welcome.body });
    let id = welcome.body.map_or_else(|| "no body".to_owned(), |b| format!("e{b}"));
    if rebase {
        client.rebases += 1;
        journal::emit(world, JournalKind::Sim, format!("net: the host's world again ({}) at host tick {}", welcome.reason, welcome.tick));
    } else {
        NetMarks::mark(world, |m| &mut m.joined);
        journal::emit(world, JournalKind::Sim, format!("net: joined {} as p{} ({id}) at host tick {}, {lead} ticks ahead", client.address, welcome.player, welcome.tick));
    }
    if let Some(region) = &welcome.region {
        journal::emit(world, JournalKind::Sim, format!("net: host region {region}"));
    }
}

/// How far ahead of the host to run: one way in ticks, the jitter, two ticks of margin.
fn lead_ticks(world: &World, client: &NetClient) -> (u64, u64) {
    let tick_ms = (sim::tick_seconds(world) * 1000.0).max(1.0);
    let stats = client.link.as_ref().map(Link::stats);
    let one_way = stats.as_ref().map_or(0.0, |s| s.srtt_ms / 2.0);
    let jitter = stats.as_ref().map_or(0.0, |s| s.rttvar_ms);
    let one_way = (one_way / tick_ms).ceil() as u64;
    let jitter = (jitter / tick_ms).ceil() as u64;
    (one_way + jitter + 2, jitter)
}

fn apply_pending(world: &mut World, client: &mut NetClient) {
    if client.stage != ClientStage::Connected {
        return;
    }
    let Some(delta) = client.pending.take() else { return };
    let (state, previous) = match client.ring.accept(&delta) {
        Ok(accepted) => accepted,
        Err(_) => {
            client.skipped += 1;
            return;
        }
    };
    client.deltas += 1;
    let predicted: BTreeSet<String> = {
        let rules = world.resource::<NetRules>();
        let registry = world.resource::<AppTypeRegistry>().read();
        rules.predicted().filter_map(|t| registry.get(t).map(|r| r.type_info().type_path().to_owned())).collect()
    };
    let keep: BTreeSet<(u32, String)> = client.body.map(|b| predicted.iter().map(|p| (b, p.clone())).collect()).unwrap_or_default();
    let changes = changes_between(world, previous.as_deref(), &state, &keep);
    for warning in apply(world, &changes) {
        crate::hooks::net_line(world, "apply", JournalKind::Warn, warning);
    }
    client.newest_host_tick = delta.tick;
    client.host_paused = delta.paused;
    // What the own body's prediction needs: the host's tick, how far ahead this runs, and
    // the host's state of the body (compared in `NetSystems::Correct`).
    if let Some(mut prediction) = world.get_resource_mut::<NetPrediction>() {
        prediction.host_tick = delta.tick;
        prediction.lead = client.lead;
    }
    if let Some(own) = &delta.own
        && client.body == Some(own.body)
        && let Some(mut correction) = world.get_resource_mut::<NetCorrection>()
    {
        correction.host_state(delta.tick, own.components.clone());
    }
    crate::events::retrigger_due(world, delta.tick);
    client.capture.ack(delta.inputs_ack);
    if let Some(margin) = delta.margin {
        client.clock.report(margin);
    }
    // The host's order queue for the body (orders finish there), unless ours is on its way;
    // with an order that ended, the aim and lock-on it left.
    if let Some(own) = &delta.own
        && let Some(value) = own.components.get(Intent::type_path())
        && let Ok(reflected) = decode_value(world, Intent::type_path(), value)
        && let Some(host_intent) = Intent::from_reflect(reflected.as_ref())
        && let Some(body) = client.body.and_then(|id| world.resource::<AiIndex>().get(AiId(id)))
        && let Some(mut intent) = world.get_mut::<Intent>(body)
    {
        client.capture.adopt_orders(own.applied_input_tick, &mut intent, &host_intent);
    }
    // The desync check: what this world holds of the host's, against the host's hash.
    if let Some(host_hash) = delta.hash {
        let own = client.body.map(|b| (b, &predicted));
        let (here, held) = world_hash(world, own);
        if here != host_hash {
            let first = first_difference(&held, &state);
            let tick = sim::tick(world);
            let mut desync = world.get_resource_or_init::<NetDesync>();
            desync.count += 1;
            desync.last = Some((delta.tick, tick, first.clone()));
            NetMarks::mark(world, |m| &mut m.desync);
            journal::emit(world, JournalKind::Error, format!("net: desync at host tick {}: {first}", delta.tick));
            client.send(&Msg::Desync { tick: delta.tick, here, host: host_hash, first, epoch: client.rebases });
        }
    }
}

// --- forwarded calls -----------------------------------------------------------------------

/// The client's forwarder: dev actions go to the host.
pub(crate) fn forwarder(address: &str) -> Forwarder {
    Forwarder { forward, host: address.to_owned() }
}

fn forward(world: &mut World, call: ForwardedCall) -> ToolResult<Value> {
    let Some(mut client) = world.get_resource_mut::<NetClient>() else {
        return Err("this game is not a client of a host".into());
    };
    if client.stage != ClientStage::Connected {
        return Err(format!("not connected to the host yet ({:?}); wait for `net_joined`", client.stage));
    }
    client.next_call += 1;
    let id = client.next_call;
    client.calls.insert(id, call.tool.clone());
    let at = client.newest_host_tick + client.lead;
    let message = Msg::Call(CallMsg { id, agent: call.agent, scope: call.scope, subject: call.subject.map(|a| a.0), tool: call.tool, args: call.args });
    client.send(&message);
    Ok(json!(format!("sent to the host; applies at host tick ~{at}")))
}

/// Installs the forwarder (a client's `Startup`).
pub(crate) fn install_forwarder(world: &mut World) {
    let Some(address) = world.get_resource::<NetClient>().map(|c| c.address.clone()) else { return };
    if let Some(mut tools) = world.get_resource_mut::<AiTools>() {
        tools.set_forwarder(Some(forwarder(&address)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::SimTick;

    /// A connected client at `tick`, the newest state from `newest` and a lead of 5, whose
    /// inputs arrive `error` ticks too early.
    fn paced(tick: u64, newest: u64, error: i64) -> (World, NetClient) {
        let mut world = World::new();
        world.insert_resource(SimTick(tick));
        world.insert_resource(Time::<Fixed>::from_hz(60.0));
        world.insert_resource(Time::<Virtual>::default());
        let mut client = NetClient::new("host", &EngineConfig::default());
        client.stage = ClientStage::Connected;
        client.newest_host_tick = newest;
        client.lead = 5;
        client.clock.target = 2;
        client.clock.report((tick, error + client.clock.target));
        assert_eq!(client.clock.error(), Some(error));
        (world, client)
    }

    fn speed(world: &World) -> f32 {
        world.resource::<Time<Virtual>>().relative_speed()
    }

    #[test]
    fn a_far_error_skips_forward_or_brakes_and_a_near_one_nudges() {
        assert_eq!(pace_for(PACE_FAR + 1), Pace::Speed(PACE_BRAKE));
        assert_eq!(pace_for(i64::MAX), Pace::Speed(PACE_BRAKE));
        assert_eq!(pace_for(-PACE_FAR - 1), Pace::Skip);
        assert_eq!(pace_for(PACE_FAR), Pace::Speed(0.9));
        assert_eq!(pace_for(-PACE_FAR), Pace::Speed(1.1));
        assert_eq!(pace_for(0), Pace::Speed(1.0));
        assert!(matches!(pace_for(-3), Pace::Speed(s) if s > 1.0 && s < 1.1));
    }

    #[test]
    fn a_client_far_ahead_of_the_host_slows_down_and_never_sets_its_tick_back() {
        // 40 ticks early: the ticks from the newest state's tick on are stamped on frames the
        // host holds. Going back to them would drop the presses of their repeats as duplicates.
        let (mut world, mut client) = paced(180, 150, 40);
        pace(&mut world, &mut client);
        assert_eq!(sim::tick(&world), 180, "the tick stays");
        assert_eq!(speed(&world), PACE_BRAKE);
        assert_eq!(client.clock.error(), Some(40), "the clock is not reset either");
    }

    #[test]
    fn a_client_far_behind_the_host_skips_forward_to_its_newest_state_plus_the_lead() {
        let (mut world, mut client) = paced(100, 300, -40);
        pace(&mut world, &mut client);
        assert_eq!(sim::tick(&world), 305);
        assert_eq!(client.clock.error(), None, "it waits for a fresh report");
    }

    #[test]
    fn a_skip_that_would_go_back_runs_faster_instead() {
        // Late inputs, but the newest state is older than this clock: no tick to skip to.
        let (mut world, mut client) = paced(120, 100, -40);
        pace(&mut world, &mut client);
        assert_eq!(sim::tick(&world), 120);
        assert_eq!(speed(&world), 1.1);
    }
}
