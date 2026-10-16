//! The co-op session over Steam. A lobby host (`--steam-host`) creates a lobby (friends only, or the
//! type `--steam-lobby` names) once its content is loaded and opens the relay transport; a game
//! server (`--steam-server`) logs on to Steam anonymously and opens it with its own Steam id; each
//! is a host like any other, with its own player 0. A client enters a lobby (`--join
//! steam:<lobby>`, an invite, Steam's `+connect_lobby`) and opens the transport to the lobby's
//! owner, or opens it straight to a host by its id (`--join steam:<id>`). From there on the session
//! is `shards_net`'s, the same as over UDP or the in-memory hub.
//!
//! Relay sessions are accepted only from those who may play: a lobby host takes its lobby's
//! members, a game server anyone (the session's own join rules decide), a client its host. Every
//! frame (and before every tick's network receive) Steam's callbacks run and what they say is
//! acted on here.

use std::sync::{Arc, Mutex};

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{EngineConfig, NetLaunch, SteamLobby};
use shards_net::{net_def, ClientStage, NetClient, NetEndpoint, NetState};

use crate::backend::{data, Backend, Event, SteamId};
use crate::launch::{classify, parse_address, SteamAddress};
use crate::lobbies::{self, Build, Search};
use crate::transport::{RelayStats, SteamTransport};
use crate::Steam;

/// What this game is in a Steam session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteamRole {
    /// No Steam session: a single game, or a session over UDP.
    None,
    /// Hosting in a lobby (`--steam-host`).
    Host,
    /// Hosting as an anonymous game server (`--steam-server`).
    Server,
    /// Joining a host through its lobby or by its id (`--join steam:…`).
    Client,
}

/// Where this game is with its Steam session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SteamState {
    /// None wanted.
    None,
    /// A lobby host: the lobby is made once the content is loaded (its size is the session's).
    ToCreate,
    Creating,
    /// A game server: waits for Steam to log it on.
    LoggingOn,
    /// A game server, logged on as `id`: clients join with `--join steam:<id>`.
    Serving { id: SteamId },
    /// A client: waits for an invite (`--join steam:`) or `steam_join`.
    WaitingForInvite,
    /// A client: enters this lobby at the next frame.
    ToJoin(SteamId),
    Joining(SteamId),
    /// A client: opens the relay to this host at the next frame (no lobby).
    ToConnect(SteamId),
    /// In `lobby`, which `owner` hosts.
    InLobby { lobby: SteamId, owner: SteamId },
    /// A client playing on `host` over the relay, without a lobby.
    Direct { host: SteamId },
    /// A launch that names no lobby or host (`--join steam:abc`): said at the first frame.
    Invalid(String),
    /// Why there is no session (a host could not make its lobby or log on, a client could not
    /// enter its lobby).
    Failed(String),
}

/// This game's Steam session.
#[derive(Resource, Debug)]
pub struct SteamSession {
    pub role: SteamRole,
    pub state: SteamState,
    /// The type of the lobby a lobby host makes (`--steam-lobby`); `None` for any other role.
    pub lobby_type: Option<SteamLobby>,
    /// The lobby's members, the owner among them.
    pub members: Vec<SteamId>,
    /// Invites this game could not follow (a single game, a host, a client in a session): lobby
    /// and who sent it.
    pub invites: Vec<(SteamId, SteamId)>,
    /// Relay sessions refused (not from the lobby, not the host).
    pub refused: u32,
    /// Who was refused: the journal says it once per peer (a refused game keeps asking).
    refused_peers: std::collections::BTreeSet<SteamId>,
    pub(crate) relay: Option<Arc<Mutex<RelayStats>>>,
    /// The lobby search `steam_lobbies` runs ([`lobbies`]).
    pub(crate) search: Search,
}

impl SteamSession {
    /// The session the launch asks for: `--steam-host`, `--steam-server`, `--join steam:…`, or none.
    pub fn from_launch(config: &EngineConfig) -> Self {
        let (role, state) = match &config.net {
            NetLaunch::Host { .. } if config.steam.server => (SteamRole::Server, SteamState::LoggingOn),
            NetLaunch::Host { .. } if config.steam.host => (SteamRole::Host, SteamState::ToCreate),
            NetLaunch::Join { addr } => match parse_address(addr) {
                Some(Ok(SteamAddress::Lobby(id))) => (SteamRole::Client, SteamState::ToJoin(id)),
                Some(Ok(SteamAddress::Host(id))) => (SteamRole::Client, SteamState::ToConnect(id)),
                Some(Ok(SteamAddress::Invite)) => (SteamRole::Client, SteamState::WaitingForInvite),
                Some(Err(e)) => (SteamRole::Client, SteamState::Invalid(e)),
                None => (SteamRole::None, SteamState::None),
            },
            _ => (SteamRole::None, SteamState::None),
        };
        let lobby_type = (role == SteamRole::Host).then_some(config.steam.lobby);
        Self { role, state, lobby_type, members: Vec::new(), invites: Vec::new(), refused: 0, refused_peers: Default::default(), relay: None, search: Search::default() }
    }

    /// The lobby this game is in.
    pub fn lobby_id(&self) -> Option<SteamId> {
        match self.state {
            SteamState::InLobby { lobby, .. } => Some(lobby),
            _ => None,
        }
    }

    /// The session's host on Steam: the lobby's owner, the game server, the host joined directly.
    pub fn host_id(&self) -> Option<SteamId> {
        match self.state {
            SteamState::InLobby { owner, .. } => Some(owner),
            SteamState::Serving { id } => Some(id),
            SteamState::Direct { host } => Some(host),
            _ => None,
        }
    }

    /// What went over the relay so far (none before the transport opened).
    pub fn relay_stats(&self) -> Option<RelayStats> {
        self.relay.as_ref().and_then(|s| s.lock().ok().map(|s| s.clone()))
    }
}

/// Whether this launch needs the relay: the session's transport is opened here, not by `shards_net`.
pub(crate) fn wants_relay(config: &EngineConfig) -> bool {
    SteamSession::from_launch(config).role != SteamRole::None
}

fn session(world: &mut World) -> Mut<'_, SteamSession> {
    world.resource_mut::<SteamSession>()
}

fn set_state(world: &mut World, state: SteamState) {
    session(world).state = state;
}

/// A client that has not said hello to a host yet: it can still take a lobby or a host.
fn client_free(world: &World) -> bool {
    world.get_resource::<NetClient>().is_some_and(|c| c.stage == ClientStage::Connecting) && !world.contains_resource::<NetEndpoint>()
}

/// `First` and `FixedFirst` (before the session's receive): Steam's callbacks, and what the session
/// is waiting for.
pub(crate) fn pump(world: &mut World) {
    let Some(mut steam) = world.remove_resource::<Steam>() else { return };
    let backend = steam.backend.as_mut();
    start(world, backend);
    for event in backend.pump() {
        on_event(world, backend, event);
    }
    world.insert_resource(steam);
    crate::cloud::send_waiting(world);
}

pub(crate) fn unavailable(backend: &dyn Backend) -> Option<String> {
    (!backend.available()).then(|| format!("Steam is unavailable ({})", backend.info().note.unwrap_or_else(|| "no Steam".into())))
}

/// What the session waits for that this machine starts: the host's lobby, the client's join.
fn start(world: &mut World, backend: &mut dyn Backend) {
    let state = world.resource::<SteamSession>().state.clone();
    match state {
        SteamState::ToCreate if world.get_resource::<ContentDb>().is_some_and(ContentDb::is_loaded) => {
            if let Some(why) = unavailable(backend) {
                return host_failed(world, &why);
            }
            set_state(world, SteamState::Creating);
            let max = u32::from(net_def(world).max_players.max(1));
            let kind = world.resource::<SteamSession>().lobby_type.unwrap_or_default();
            if let Err(e) = backend.create_lobby(max, kind) {
                host_failed(world, &format!("Steam could not create a lobby: {e}"));
            }
        }
        SteamState::LoggingOn => {
            if let Some(why) = unavailable(backend) {
                host_failed(world, &why);
            }
        }
        SteamState::ToJoin(lobby) => {
            if let Some(why) = unavailable(backend) {
                return client_failed(world, backend, Some(lobby), &why);
            }
            set_state(world, SteamState::Joining(lobby));
            if let Err(e) = backend.join_lobby(lobby) {
                client_failed(world, backend, Some(lobby), &e);
            }
        }
        SteamState::ToConnect(host) => match unavailable(backend) {
            Some(why) => client_failed(world, backend, None, &format!("cannot reach Steam host {host}: {why}")),
            None => connect_direct(world, backend, host),
        },
        SteamState::Invalid(why) => client_failed(world, backend, None, &why),
        _ => {}
    }
}

fn on_event(world: &mut World, backend: &mut dyn Backend, event: Event) {
    let role = world.resource::<SteamSession>().role;
    match event {
        Event::LobbyCreated(Ok(lobby)) if role == SteamRole::Host => hosting(world, backend, lobby),
        Event::LobbyCreated(Err(e)) if role == SteamRole::Host => host_failed(world, &format!("Steam could not create a lobby: {e}")),
        Event::ServerLoggedOn(id) if role == SteamRole::Server => serving(world, backend, id),
        Event::ServerLogOnFailed(why) if role == SteamRole::Server => host_failed(world, &format!("the game server did not log on to Steam: {why}")),
        Event::LobbyEntered { lobby, result } if role == SteamRole::Client && world.resource::<SteamSession>().state == SteamState::Joining(lobby) => {
            match result {
                Ok(()) => entered(world, backend, lobby),
                Err(e) => client_failed(world, backend, Some(lobby), &e),
            }
        }
        Event::LobbiesFound(answer) => lobbies::answered(world, answer),
        Event::MemberChanged { lobby, user, joined } => member_changed(world, backend, lobby, user, joined),
        Event::JoinRequested { lobby, from } => join_requested(world, backend, lobby, from),
        Event::SessionRequest { from } => session_request(world, backend, from),
        Event::SessionFailed { peer, reason } => {
            let who = who(backend, peer);
            journal::emit(world, JournalKind::Warn, format!("steam: the relay session with {who} failed: {reason}"));
        }
        Event::StatsStored(result) => crate::achievements::stored(world, result),
        Event::Note(text) => {
            journal::log(world, JournalKind::Local, format!("steam: {text}"));
        }
        Event::CallbacksStopped(why) => {
            journal::emit(world, JournalKind::Error, format!("steam: {why}"));
            // What waited for Steam's answer gets none.
            let stopped = "Steam's callbacks stopped (`steam` says why)";
            lobbies::answered(world, Err(stopped.to_owned()));
            match world.resource::<SteamSession>().state.clone() {
                SteamState::Creating | SteamState::LoggingOn => host_failed(world, stopped),
                SteamState::Joining(lobby) => client_failed(world, backend, Some(lobby), stopped),
                _ => {}
            }
        }
        // A stray answer (a lobby this game no longer waits for, a log-on it did not ask for).
        Event::LobbyCreated(_) | Event::LobbyEntered { .. } | Event::ServerLoggedOn(_) | Event::ServerLogOnFailed(_) => {}
    }
}

/// `name (id)`, or the id.
pub(crate) fn who(backend: &dyn Backend, user: SteamId) -> String {
    match backend.user_name(user) {
        Some(name) if !name.is_empty() => format!("{name} ({user})"),
        _ => user.to_string(),
    }
}

/// Hosting again after a failure that ended the session.
fn host_open(world: &mut World) {
    if matches!(world.get_resource::<NetState>(), Some(NetState::Ended(_))) {
        world.insert_resource(NetState::Hosting);
    }
}

/// The host's lobby exists: its data (what the game and its build are, who hosts: what a lobby
/// search filters on), the relay.
fn hosting(world: &mut World, backend: &mut dyn Backend, lobby: SteamId) {
    let Some(me) = backend.info().user else { return };
    for (key, value) in Build::of(world).lobby_data() {
        backend.set_lobby_data(lobby, key, value);
    }
    backend.set_lobby_data(lobby, data::HOST, &backend.info().name);
    let members = backend.lobby_members(lobby);
    {
        let mut s = session(world);
        s.state = SteamState::InLobby { lobby, owner: me };
        s.members = members;
    }
    if let Err(e) = open_relay(world, backend, None) {
        return host_failed(world, &format!("the Steam relay did not open: {e}"));
    }
    host_open(world);
    let kind = world.resource::<SteamSession>().lobby_type.unwrap_or_default();
    let how = match kind {
        SteamLobby::Private => "players come in only through an invite (steam_invite)".to_owned(),
        _ => format!("friends join through an invite (steam_invite), or with --join steam:{lobby}"),
    };
    journal::log(world, JournalKind::Local, format!("steam: hosting in lobby {lobby} ({}): {how}", kind.describe()));
}

/// The game server logged on: its id is what clients join.
fn serving(world: &mut World, backend: &mut dyn Backend, id: SteamId) {
    if world.resource::<SteamSession>().state != SteamState::LoggingOn {
        return;
    }
    set_state(world, SteamState::Serving { id });
    if let Err(e) = open_relay(world, backend, None) {
        return host_failed(world, &format!("the Steam relay did not open: {e}"));
    }
    host_open(world);
    journal::log(world, JournalKind::Local, format!("steam: game server logged on as {id}: players join with --join steam:{id}"));
}

/// A client entered its lobby: the owner is the host, and the relay opens to it.
fn entered(world: &mut World, backend: &mut dyn Backend, lobby: SteamId) {
    let me = backend.info().user;
    let owner = match backend.lobby_owner(lobby) {
        Some(owner) if Some(owner) != me && owner != 0 => owner,
        _ => {
            backend.leave_lobby(lobby);
            return client_failed(world, backend, Some(lobby), "the lobby has no host (its owner left)");
        }
    };
    let members = backend.lobby_members(lobby);
    {
        let mut s = session(world);
        s.state = SteamState::InLobby { lobby, owner };
        s.members = members;
    }
    if let Err(e) = open_relay(world, backend, Some(owner)) {
        backend.leave_lobby(lobby);
        return client_failed(world, backend, Some(lobby), &format!("the Steam relay did not open: {e}"));
    }
    let host = who(backend, owner);
    journal::log(world, JournalKind::Local, format!("steam: entered lobby {lobby}; its host is {host}"));
}

/// A client opens the relay straight to a host it knows by id (a game server).
fn connect_direct(world: &mut World, backend: &mut dyn Backend, host: SteamId) {
    if Some(host) == backend.info().user {
        return client_failed(world, backend, None, &format!("{host} is this machine's own Steam id"));
    }
    set_state(world, SteamState::Direct { host });
    if let Err(e) = open_relay(world, backend, Some(host)) {
        return client_failed(world, backend, None, &format!("the Steam relay did not open: {e}"));
    }
    let name = who(backend, host);
    journal::log(world, JournalKind::Local, format!("steam: joining {name} over the relay"));
}

/// The session's transport over the relay: inserted now, the session takes it from here. A
/// client's host by its Steam id: a rejoin (another lobby of the same host) keeps its slot, as the
/// token is kept per host address.
fn open_relay(world: &mut World, backend: &dyn Backend, host: Option<SteamId>) -> Result<(), String> {
    if world.contains_resource::<NetEndpoint>() {
        return Ok(());
    }
    let me = backend.info().user.ok_or("no Steam id yet")?;
    let relay = backend.relay()?;
    let stats = Arc::new(Mutex::new(RelayStats::default()));
    let config = world.get_resource::<EngineConfig>().cloned().unwrap_or_default();
    let transport = shards_net::conditioner_from(&config, SteamTransport::new(relay, me, stats.clone()));
    if let (Some(host), Some(mut client)) = (host, world.get_resource_mut::<NetClient>()) {
        client.address = format!("steam:{host}");
    }
    world.insert_resource(NetEndpoint { transport, clock: backend.clock(), host });
    session(world).relay = Some(stats);
    Ok(())
}

/// The host has no session: it ended before it began, as when a UDP port is taken. A lobby host's
/// `steam_host` tries again; `--exit-with-host` exits with 2.
fn host_failed(world: &mut World, why: &str) {
    set_state(world, SteamState::Failed(why.to_owned()));
    let again = if world.resource::<SteamSession>().role == SteamRole::Host { "; try again with steam_host" } else { "" };
    journal::emit(world, JournalKind::Error, format!("steam: cannot host: {why}{again}"));
    world.insert_resource(NetState::Ended(format!("cannot host: {why}")));
    if world.get_resource::<EngineConfig>().is_some_and(|c| c.exit_with_host) {
        world.write_message(AppExit::from_code(shards_net::session::NO_SESSION_EXIT));
    }
}

/// A client did not get into its session. It waits for another (`steam_join`, an invite), unless
/// nothing can come (no Steam) or it exits with its session (`--exit-with-host`, code 2).
fn client_failed(world: &mut World, backend: &dyn Backend, lobby: Option<SteamId>, why: &str) {
    let reason = match lobby {
        Some(lobby) => format!("cannot enter Steam lobby {lobby}: {why}"),
        None => why.to_owned(),
    };
    set_state(world, SteamState::Failed(reason.clone()));
    journal::emit(world, JournalKind::Error, format!("steam: {reason}"));
    let exits = world.get_resource::<EngineConfig>().is_some_and(|c| c.exit_with_host);
    if exits || !backend.available() {
        shards_net::session::unopened(world, &reason);
    } else {
        world.insert_resource(NetState::Joining(format!("{reason}; join another with steam_join lobby=ID (or host=ID), or accept an invite")));
    }
}

fn member_changed(world: &mut World, backend: &mut dyn Backend, lobby: SteamId, user: SteamId, joined: bool) {
    let SteamState::InLobby { lobby: ours, owner } = world.resource::<SteamSession>().state else { return };
    if ours != lobby {
        return;
    }
    let members = backend.lobby_members(lobby);
    session(world).members = members;
    let name = who(backend, user);
    let line = if joined { format!("steam: {name} entered lobby {lobby}") } else { format!("steam: {name} left lobby {lobby}") };
    journal::log(world, JournalKind::Local, line);
    if !joined && user == owner && world.resource::<SteamSession>().role == SteamRole::Client {
        journal::emit(world, JournalKind::Warn, format!("steam: the host {name} left the lobby; the session ends when it goes quiet"));
    }
}

fn join_requested(world: &mut World, backend: &mut dyn Backend, lobby: SteamId, from: SteamId) {
    let s = world.resource::<SteamSession>();
    let (role, state) = (s.role, s.state.clone());
    let name = who(backend, from);
    match state {
        SteamState::Joining(l) | SteamState::InLobby { lobby: l, .. } if l == lobby => {}
        SteamState::WaitingForInvite | SteamState::Failed(_) if role == SteamRole::Client && client_free(world) => {
            journal::log(world, JournalKind::Local, format!("steam: {name} invites you to lobby {lobby}: joining"));
            set_state(world, SteamState::Joining(lobby));
            if let Err(e) = backend.join_lobby(lobby) {
                client_failed(world, backend, Some(lobby), &e);
            }
        }
        _ => {
            session(world).invites.push((lobby, from));
            journal::log(
                world,
                JournalKind::Local,
                format!("steam: {name} invites you to lobby {lobby}: start the game with --join steam:{lobby} to play there (a running game does not change sessions)"),
            );
        }
    }
}

fn session_request(world: &mut World, backend: &mut dyn Backend, from: SteamId) {
    let s = world.resource::<SteamSession>();
    let refused = match (s.role, &s.state) {
        (SteamRole::Host, SteamState::InLobby { lobby, .. }) => (!backend.lobby_members(*lobby).contains(&from)).then(|| format!("not in lobby {lobby}")),
        // A game server has no lobby: whoever knows its id may ask, and the session's join rules decide.
        (SteamRole::Server, SteamState::Serving { .. }) => None,
        (SteamRole::Client, SteamState::InLobby { owner: host, .. } | SteamState::Direct { host }) => (from != *host).then(|| format!("not this game's host {host}")),
        _ => Some("this game is in no Steam session".to_owned()),
    };
    match refused {
        None => {
            backend.accept_session(from);
        }
        Some(why) => {
            backend.reject_session(from);
            let mut s = session(world);
            s.refused += 1;
            if s.refused_peers.insert(from) {
                let name = who(backend, from);
                journal::emit(world, JournalKind::Warn, format!("steam: refused a relay session from {name}: {why}"));
            }
        }
    }
}

/// Exit hook, after the session's (its last words are out: the host's end, a client's goodbye and
/// the character the host sent back): the cloud copies of what the hooks before this one saved, the
/// unlocks stored, the relay sessions closed, the lobby left. Steam itself shuts down when the game's
/// world goes, with the last Steamworks handle in it, after every hook.
pub(crate) fn on_exit(world: &mut World) {
    // A save written in an earlier hook queued its cloud copy (`SaveWritten`): it goes up now,
    // before Steam is left.
    world.flush();
    crate::cloud::send_waiting(world);
    crate::achievements::flush_achievements(world);
    let (lobby, peers) = world.get_resource::<SteamSession>().map_or((None, Vec::new()), |s| {
        (s.lobby_id(), s.relay_stats().map(|r| r.peers.into_iter().collect::<Vec<_>>()).unwrap_or_default())
    });
    let Some(mut steam) = world.get_resource_mut::<Steam>() else { return };
    steam.backend.close_sessions(&peers);
    if let Some(lobby) = lobby {
        steam.backend.leave_lobby(lobby);
    }
}

/// `steam_host`: a lobby for a game started as a Steam lobby host (again, after a failure); a game
/// server's state.
pub(crate) fn host_again(world: &mut World) -> Result<String, String> {
    let s = world.resource::<SteamSession>();
    match (s.role, s.state.clone()) {
        (SteamRole::Server, SteamState::Serving { id }) => Ok(format!("serving as game server {id}: players join with --join steam:{id}")),
        (SteamRole::Server, SteamState::LoggingOn) => Ok("the game server is logging on to Steam: `steam` shows its id once it is on".into()),
        (SteamRole::Server, _) => Err("the game server did not log on to Steam (`steam` says why): start it again".into()),
        (SteamRole::Host, SteamState::InLobby { lobby, .. }) => Ok(format!("hosting in lobby {lobby} ({} in it): invite friends with steam_invite", s.members.len())),
        (SteamRole::Host, SteamState::ToCreate | SteamState::Creating) => Ok("the lobby is being created: `steam` shows it once it is there".into()),
        (SteamRole::Host, _) => {
            set_state(world, SteamState::ToCreate);
            pump(world);
            match world.resource::<SteamSession>().state.clone() {
                SteamState::InLobby { lobby, .. } => Ok(format!("hosting in lobby {lobby}: invite friends with steam_invite")),
                SteamState::Failed(why) => Err(format!("cannot host: {why}")),
                _ => Ok("creating a lobby: `steam` shows it once it is there".into()),
            }
        }
        _ => Err("this game was not started as a Steam host: start it with --steam-host (a lobby) or --steam-server (a game server); a running game does not become a host".into()),
    }
}

/// `steam_join`: a client that is in no session enters a lobby, or joins a host by its id.
pub(crate) fn join(world: &mut World, id: SteamId) -> Result<String, String> {
    let target = classify(id)?;
    let s = world.resource::<SteamSession>();
    if s.role != SteamRole::Client {
        return Err("this game was not started as a Steam client: start it with --join steam:ID, or --join steam: to wait for an invite (a running game does not change sessions)".into());
    }
    match s.state.clone() {
        SteamState::InLobby { lobby: now, .. } => return Err(format!("already in lobby {now}: a running session does not change lobbies")),
        SteamState::Direct { host } => return Err(format!("already playing on {host}: a running session does not change hosts")),
        SteamState::Joining(now) | SteamState::ToJoin(now) => return Err(format!("already entering lobby {now}")),
        _ => {}
    }
    if !client_free(world) {
        return Err("this client's session is over: start the game again with --join steam:ID".into());
    }
    set_state(world, match target {
        SteamAddress::Lobby(lobby) => SteamState::ToJoin(lobby),
        _ => SteamState::ToConnect(id),
    });
    pump(world);
    let name = |world: &World, user: SteamId| world.get_resource::<Steam>().map_or_else(|| user.to_string(), |s| who(s.backend.as_ref(), user));
    match world.resource::<SteamSession>().state.clone() {
        SteamState::InLobby { lobby, owner } => Ok(format!("entered lobby {lobby}; its host is {}", name(world, owner))),
        SteamState::Direct { host } => Ok(format!("joining {} over the relay", name(world, host))),
        SteamState::Failed(why) => Err(why),
        _ => Ok(format!("entering lobby {id}: `steam` shows it once in")),
    }
}

/// `steam_invite`: a friend into this game's lobby, or Steam's invite dialog.
pub(crate) fn invite(world: &mut World, friend: Option<SteamId>) -> Result<String, String> {
    let Some(lobby) = world.resource::<SteamSession>().lobby_id() else {
        let server = world.resource::<SteamSession>().role == SteamRole::Server;
        return Err(if server {
            "a game server has no lobby to invite into: players join it with --join steam:ID (`steam` shows its id)".into()
        } else {
            "this game is in no Steam lobby: host one (--steam-host) or enter one (--join steam:LOBBY) first".into()
        });
    };
    world.resource_mut::<Steam>().backend.invite(lobby, friend)
}
