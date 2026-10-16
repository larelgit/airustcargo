//! The real Steam (feature `sdk`): the Steam client on this machine through Steamworks, as the
//! player, or an anonymous game server (`--steam-server`). Only a game the player runs talks to it
//! ([`shards_core::SteamLaunch::connect`]); tests, checks and stands never do. Steamworks types
//! stay in this file.
//!
//! Steam's callbacks run in [`RealBackend::pump`] (the module's frame and tick systems); each one
//! the module listens to is turned into an [`Event`]. A relay session request is kept until the
//! module decides on it: dropping Steam's request would refuse it.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};

use shards_net::{NetClock, WallClock};
use steamworks::networking_messages::{NetworkingMessages, SessionRequest};
use steamworks::networking_types::{NetworkingIdentity, SendFlags};
use steamworks::sys;
use steamworks::{
    CallbackHandle, ChatMemberStateChange, Client, GameLobbyJoinRequested, LobbyChatUpdate, LobbyId, LobbyType, Server, ServerMode, SteamAPIInitError,
    SteamServerConnectFailure, SteamServersConnected, SteamServersDisconnected, UserStatsStored,
};

use crate::backend::{Backend, Event, Info, Relay, Route, SteamId};

/// The most members Steam allows in a lobby.
const MAX_LOBBY: u32 = 250;

/// Datagrams taken from Steam per receive.
const BATCH: usize = 256;

/// The game server's own ports (its UDP for Steam; the session goes over the relay). One game
/// server per machine.
pub(crate) const SERVER_PORT: u16 = 27215;
pub(crate) const SERVER_QUERY_PORT: u16 = 27216;

/// What a game server tells Steam about its version (Steam wants four numbers).
const SERVER_VERSION: &str = "1.0.0.0";

type Shared<T> = Arc<Mutex<T>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) struct RealBackend {
    client: Client,
    /// An anonymous game server's half (`--steam-server`); `None`: the client as the player.
    server: Option<Server>,
    /// The user's id, or the game server's once it logged on.
    me: Option<SteamId>,
    name: String,
    app_id: u32,
    relay_only: bool,
    events: Shared<VecDeque<Event>>,
    requests: Shared<BTreeMap<SteamId, SessionRequest>>,
    wall: Arc<WallClock>,
    _callbacks: Vec<CallbackHandle>,
}

/// Why Steamworks did not start, as the player can act on it.
fn init_error(e: SteamAPIInitError) -> String {
    match e {
        SteamAPIInitError::NoSteamClient(m) => format!("the Steam client is not running, or this user is not logged in ({m}); start Steam and the game again"),
        SteamAPIInitError::VersionMismatch(m) => format!("the Steam client is older than this game's Steamworks ({m}); update Steam"),
        SteamAPIInitError::FailedGeneric(m) => format!("Steamworks did not start ({m}): start the game from Steam, or give its app id (--steam-app)"),
    }
}

/// Starts Steamworks (`start`), turning a panic inside it into an error.
fn started<T>(start: impl FnOnce() -> Result<T, SteamAPIInitError>) -> Result<T, String> {
    match catch_unwind(AssertUnwindSafe(start)) {
        Ok(Ok(started)) => Ok(started),
        Ok(Err(e)) => Err(init_error(e)),
        Err(_) => {
            ai_core::crash::handled("Steamworks panicked while starting");
            Err("Steamworks failed while starting (an unknown error code)".into())
        }
    }
}

/// `--steam-relay-only`: no direct routes (ICE off), so every session goes through Steam's relays.
/// Before the relay network starts.
fn relay_only() {
    // SAFETY: Steamworks is running; the call only sets one of its global config values.
    let set = unsafe {
        sys::SteamAPI_ISteamNetworkingUtils_SetGlobalConfigValueInt32(
            sys::SteamAPI_SteamNetworkingUtils_SteamAPI_v004(),
            sys::ESteamNetworkingConfigValue::k_ESteamNetworkingConfig_P2P_Transport_ICE_Enable,
            0,
        )
    };
    if !set {
        bevy::log::warn!("steam: --steam-relay-only: Steam did not take ICE off; direct routes may be used");
    }
}

impl RealBackend {
    /// The Steam client as the player, for `app_id` (else the app Steam started the game with, or
    /// the `SteamAppId` environment variable).
    pub(crate) fn init(app_id: Option<u32>, relay: bool) -> Result<Self, String> {
        let client = started(|| match app_id {
            Some(id) => Client::init_app(id),
            None => Client::init(),
        })?;
        let me = client.user().steam_id().raw();
        let name = client.friends().name();
        let messages = client.networking_messages();
        let mut steam = Self::with(client, None, Some(me), name, relay, &messages);
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |update: LobbyChatUpdate| {
            let joined = matches!(update.member_state_change, ChatMemberStateChange::Entered);
            lock(&ev).push_back(Event::MemberChanged { lobby: update.lobby.raw(), user: update.user_changed.raw(), joined });
        }));
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |join: GameLobbyJoinRequested| {
            lock(&ev).push_back(Event::JoinRequested { lobby: join.lobby_steam_id.raw(), from: join.friend_steam_id.raw() });
        }));
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |stored: UserStatsStored| {
            lock(&ev).push_back(Event::StatsStored(stored.result.map_err(|e| e.to_string())));
        }));
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |_: SteamServersConnected| {
            lock(&ev).push_back(Event::Note("connected to the Steam servers".into()));
        }));
        Ok(steam)
    }

    /// An anonymous game server (no user, no lobby): it logs on, and players join it by its id.
    /// The app id is `app_id`, else the `SteamAppId` environment variable.
    pub(crate) fn init_server(app_id: Option<u32>, relay: bool, product: &str) -> Result<Self, String> {
        if let Some(id) = app_id {
            // SAFETY: set at startup, before Steamworks (the only reader) starts; the engine reads
            // neither variable.
            unsafe {
                std::env::set_var("SteamAppId", id.to_string());
                std::env::set_var("SteamGameId", id.to_string());
            }
        }
        let (server, client) = started(|| Server::init(Ipv4Addr::LOCALHOST, SERVER_PORT, SERVER_QUERY_PORT, ServerMode::NoAuthentication, SERVER_VERSION))?;
        server.set_product(product);
        server.set_game_description(product);
        server.set_dedicated_server(true);
        server.log_on_anonymous();
        let messages = server.networking_messages();
        let mut steam = Self::with(client, Some(server), None, product.to_owned(), relay, &messages);
        let ev = steam.events.clone();
        // The id is the server's, read when the event is taken (`pump`).
        steam._callbacks.push(steam.client.register_callback(move |_: SteamServersConnected| {
            lock(&ev).push_back(Event::ServerLoggedOn(0));
        }));
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |failure: SteamServerConnectFailure| {
            lock(&ev).push_back(if failure.still_retrying {
                Event::Note(format!("the game server cannot reach Steam yet ({}); Steam retries", failure.reason))
            } else {
                Event::ServerLogOnFailed(failure.reason.to_string())
            });
        }));
        Ok(steam)
    }

    /// The parts both share: relay access, the session callbacks, losing the Steam servers.
    fn with(client: Client, server: Option<Server>, me: Option<SteamId>, name: String, relay: bool, messages: &NetworkingMessages) -> Self {
        if relay {
            relay_only();
        }
        client.networking_utils().init_relay_network_access();
        let app_id = match server {
            // SAFETY: the game server is running. A game-server process has no user, so its
            // `SteamUtils` is null (`client.utils()` would panic or crash); it has its own.
            Some(_) => unsafe { sys::SteamAPI_ISteamUtils_GetAppID(sys::SteamAPI_SteamGameServerUtils_v010()) },
            None => client.utils().app_id().0,
        };
        let events: Shared<VecDeque<Event>> = Arc::default();
        let requests: Shared<BTreeMap<SteamId, SessionRequest>> = Arc::default();
        // Only the callbacks registered here are read: each turns Steam's struct into an event.
        // (Reading every callback, as `process_callbacks` does, would decode ones that can panic.)
        let (ev, rq) = (events.clone(), requests.clone());
        messages.session_request_callback(move |request| {
            // A request with no Steam id behind it is dropped, which refuses it.
            let Some(from) = request.remote().steam_id().map(|id| id.raw()) else { return };
            lock(&rq).insert(from, request);
            lock(&ev).push_back(Event::SessionRequest { from });
        });
        let ev = events.clone();
        messages.session_failed_callback(move |info| {
            let peer = info.identity_remote().and_then(|i| i.steam_id()).map_or(0, |id| id.raw());
            let state = info.state().map_or_else(|_| "an unknown state".to_owned(), |s| format!("{s:?}"));
            lock(&ev).push_back(Event::SessionFailed { peer, reason: format!("the relay session ended ({state})") });
        });
        let ev = events.clone();
        let lost = client.register_callback(move |lost: SteamServersDisconnected| {
            lock(&ev).push_back(Event::Note(format!("lost the Steam servers ({}); sessions wait for them", lost.reason)));
        });
        Self { client, server, me, name, app_id, relay_only: relay, events, requests, wall: Arc::new(WallClock::new()), _callbacks: vec![lost] }
    }

    fn messages(&self) -> NetworkingMessages {
        match &self.server {
            Some(server) => server.networking_messages(),
            None => self.client.networking_messages(),
        }
    }

    /// A player's call on a game server: refused.
    fn player_only(&self) -> Result<(), String> {
        match self.server {
            Some(_) => Err("an anonymous game server has no lobbies, invites, achievements or Steam Cloud: a player's game does".into()),
            None => Ok(()),
        }
    }
}

impl Backend for RealBackend {
    fn info(&self) -> Info {
        let kind = if self.server.is_some() { "steam server" } else { "steam" };
        Info { kind, user: self.me, name: self.name.clone(), app_id: Some(self.app_id), note: None, relay_only: self.relay_only }
    }

    fn pump(&mut self) -> Vec<Event> {
        let client = &self.client;
        if catch_unwind(AssertUnwindSafe(|| client.run_callbacks())).is_err() {
            ai_core::crash::handled("a Steam callback panicked");
            lock(&self.events).push_back(Event::Note("a Steam callback failed; its event is lost".into()));
        }
        let mut events: Vec<Event> = lock(&self.events).drain(..).collect();
        for event in &mut events {
            if let (Event::ServerLoggedOn(id), Some(server)) = (&mut *event, &self.server) {
                *id = server.steam_id().raw();
                self.me = Some(*id);
            }
        }
        events
    }

    fn clock(&self) -> NetClock {
        NetClock::Wall(self.wall.clone())
    }

    fn relay(&self) -> Result<Box<dyn Relay>, String> {
        Ok(Box::new(RealRelay { messages: self.messages() }))
    }

    fn relay_status(&self) -> String {
        match self.client.networking_utils().relay_network_status() {
            Ok(available) => format!("{available:?}"),
            Err(e) => format!("unavailable: {e}"),
        }
    }

    fn route(&self, peer: SteamId) -> Option<Route> {
        use sys::ESteamNetworkingConnectionState as State;
        // SAFETY: Steamworks is running (this backend exists); the identity and both structs are
        // plain data Steam fills in, zeroed first as its headers do.
        let (state, info, status) = unsafe {
            let messages = match self.server {
                Some(_) => sys::SteamAPI_SteamGameServerNetworkingMessages_SteamAPI_v002(),
                None => sys::SteamAPI_SteamNetworkingMessages_SteamAPI_v002(),
            };
            if messages.is_null() {
                return None;
            }
            let mut identity: sys::SteamNetworkingIdentity = std::mem::zeroed();
            sys::SteamAPI_SteamNetworkingIdentity_SetSteamID64(&mut identity, peer);
            let mut info: sys::SteamNetConnectionInfo_t = std::mem::zeroed();
            let mut status: sys::SteamNetConnectionRealTimeStatus_t = std::mem::zeroed();
            let state = sys::SteamAPI_ISteamNetworkingMessages_GetSessionConnectionInfo(messages, &identity, &mut info, &mut status);
            (state, info, status)
        };
        let state = match state {
            State::k_ESteamNetworkingConnectionState_None => return None,
            State::k_ESteamNetworkingConnectionState_Connecting => "connecting".to_owned(),
            State::k_ESteamNetworkingConnectionState_FindingRoute => "finding route".to_owned(),
            State::k_ESteamNetworkingConnectionState_Connected => "connected".to_owned(),
            State::k_ESteamNetworkingConnectionState_ClosedByPeer => "closed by peer".to_owned(),
            State::k_ESteamNetworkingConnectionState_ProblemDetectedLocally => "problem".to_owned(),
            other => format!("{other:?}"),
        };
        let relayed = info.m_nFlags & sys::k_nSteamNetworkConnectionInfoFlags_Relayed != 0;
        let pop = info.m_idPOPRelay;
        // Steam's data-center code: three letters, sometimes a fourth in the top byte.
        let relay_pop = (pop != 0).then(|| {
            [pop >> 16, pop >> 8, pop, pop >> 24].iter().map(|b| (*b & 0xFF) as u8).filter(|b| *b != 0).map(char::from).collect::<String>()
        });
        let ping_ms = (state == "connected" && status.m_nPing >= 0).then_some(status.m_nPing);
        Some(Route { state, relayed, relay_pop, ping_ms })
    }

    fn accept_session(&mut self, peer: SteamId) -> bool {
        lock(&self.requests).remove(&peer).is_some_and(SessionRequest::accept)
    }

    fn reject_session(&mut self, peer: SteamId) {
        if let Some(request) = lock(&self.requests).remove(&peer) {
            request.reject();
        }
    }

    fn create_lobby(&mut self, max_members: u32) -> Result<(), String> {
        self.player_only()?;
        let ev = self.events.clone();
        self.client.matchmaking().create_lobby(LobbyType::FriendsOnly, max_members.clamp(1, MAX_LOBBY), move |created| {
            lock(&ev).push_back(Event::LobbyCreated(created.map(|lobby| lobby.raw()).map_err(|e| e.to_string())));
        });
        Ok(())
    }

    fn join_lobby(&mut self, lobby: SteamId) -> Result<(), String> {
        self.player_only()?;
        let ev = self.events.clone();
        self.client.matchmaking().join_lobby(LobbyId::from_raw(lobby), move |entered| {
            let result = entered.map(|_| ()).map_err(|()| "Steam refused (the lobby is gone or full, or you may not enter it)".to_owned());
            lock(&ev).push_back(Event::LobbyEntered { lobby, result });
        });
        Ok(())
    }

    fn leave_lobby(&mut self, lobby: SteamId) {
        if self.server.is_none() {
            self.client.matchmaking().leave_lobby(LobbyId::from_raw(lobby));
        }
    }

    fn lobby_owner(&self, lobby: SteamId) -> Option<SteamId> {
        self.player_only().ok()?;
        let owner = self.client.matchmaking().lobby_owner(LobbyId::from_raw(lobby)).raw();
        (owner != 0).then_some(owner)
    }

    fn lobby_members(&self, lobby: SteamId) -> Vec<SteamId> {
        if self.server.is_some() {
            return Vec::new();
        }
        self.client.matchmaking().lobby_members(LobbyId::from_raw(lobby)).into_iter().map(|id| id.raw()).collect()
    }

    fn set_lobby_data(&mut self, lobby: SteamId, key: &str, value: &str) -> bool {
        self.server.is_none() && self.client.matchmaking().set_lobby_data(LobbyId::from_raw(lobby), key, value)
    }

    fn user_name(&self, user: SteamId) -> Option<String> {
        if Some(user) == self.me {
            return Some(self.name.clone());
        }
        if self.server.is_some() {
            return None;
        }
        let name = self.client.friends().get_friend(steamworks::SteamId::from_raw(user)).name();
        (!name.is_empty() && name != "[unknown]").then_some(name)
    }

    fn invite(&mut self, lobby: SteamId, friend: Option<SteamId>) -> Result<String, String> {
        self.player_only()?;
        let Some(friend) = friend else {
            self.client.friends().activate_invite_dialog(LobbyId::from_raw(lobby));
            return Ok(format!("Steam's invite dialog is open for lobby {lobby} (in the window's overlay)"));
        };
        // SAFETY: Steamworks is running (this backend exists), and both ids are plain numbers.
        let sent = unsafe { sys::SteamAPI_ISteamMatchmaking_InviteUserToLobby(sys::SteamAPI_SteamMatchmaking_v009(), lobby, friend) };
        let name = self.user_name(friend).unwrap_or_else(|| friend.to_string());
        if sent { Ok(format!("invited {name} ({friend}) to lobby {lobby}")) } else { Err(format!("Steam did not send the invite to {name} ({friend}): are you in lobby {lobby}?")) }
    }

    fn achievement_set(&mut self, api: &str) -> Result<(), String> {
        self.player_only()?;
        self.client
            .user_stats()
            .achievement(api)
            .set()
            .map_err(|()| format!("Steam has no achievement `{api}` for app {} (define it in Steamworks, or fix the name)", self.app_id))
    }

    fn achievement_get(&self, api: &str) -> Option<bool> {
        self.player_only().ok()?;
        self.client.user_stats().achievement(api).get().ok()
    }

    fn store_stats(&mut self) -> Result<(), String> {
        self.player_only()?;
        self.client.user_stats().store_stats().map_err(|()| "Steam did not take the store (no stats for this user yet)".to_owned())
    }

    fn cloud_enabled(&self) -> Result<(), String> {
        self.player_only()?;
        let storage = self.client.remote_storage();
        if !storage.is_cloud_enabled_for_account() {
            return Err("Steam Cloud is off for this account (Steam, Settings, Cloud)".into());
        }
        if !storage.is_cloud_enabled_for_app() {
            return Err("Steam Cloud is off for this game (its Properties in the library)".into());
        }
        Ok(())
    }

    fn cloud_files(&self) -> Vec<(String, u64)> {
        if self.server.is_some() {
            return Vec::new();
        }
        self.client.remote_storage().files().into_iter().map(|f| (f.name, f.size)).collect()
    }

    fn cloud_read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.player_only()?;
        let file = self.client.remote_storage().file(name);
        if !file.exists() {
            return Err(format!("no cloud file `{name}`"));
        }
        let mut bytes = Vec::new();
        file.read().read_to_end(&mut bytes).map_err(|e| format!("cannot read `{name}` from Steam Cloud: {e}"))?;
        Ok(bytes)
    }

    fn cloud_write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.player_only()?;
        let mut writer = self.client.remote_storage().file(name).write();
        writer.write_all(bytes).map_err(|e| format!("Steam Cloud did not take `{name}`: {e} (quota, or Steam Cloud off)"))
    }

    fn cloud_timestamp(&self, name: &str) -> Option<i64> {
        self.player_only().ok()?;
        let file = self.client.remote_storage().file(name);
        file.exists().then(|| file.timestamp())
    }
}

/// Datagrams over Steam's relay (`ISteamNetworkingMessages`, channel 0): the player's or the game
/// server's interface.
struct RealRelay {
    messages: NetworkingMessages,
}

impl Relay for RealRelay {
    fn send(&mut self, to: SteamId, datagram: &[u8]) -> Result<(), String> {
        let identity = NetworkingIdentity::new_steam_id(steamworks::SteamId::from_raw(to));
        self.messages
            .send_message_to_user(identity, SendFlags::UNRELIABLE_NO_NAGLE | SendFlags::AUTO_RESTART_BROKEN_SESSION, datagram, 0)
            .map_err(|e| e.to_string())
    }

    fn recv(&mut self, _now: u64, out: &mut Vec<(SteamId, Vec<u8>)>) {
        loop {
            let messages = self.messages.receive_messages_on_channel(0, BATCH);
            let n = messages.len();
            for message in messages {
                if let Some(from) = message.identity_peer().steam_id() {
                    out.push((from.raw(), message.data().to_vec()));
                }
            }
            if n < BATCH {
                break;
            }
        }
    }

    fn describe(&self) -> String {
        "steam relay".into()
    }
}
