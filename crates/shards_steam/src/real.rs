//! The real Steam (feature `sdk`): the Steam client on this machine through Steamworks, as the
//! player, or an anonymous game server (`--steam-server`). Only a game the player runs talks to it
//! ([`shards_core::SteamLaunch::connect`]); tests, checks and stands never do. Steamworks types
//! stay in this file.
//!
//! Steam's callbacks run in [`RealBackend::pump`] (the module's frame and tick systems); each one
//! the module listens to is turned into an [`Event`] (those whose Steamworks decoder can panic are
//! read as plain numbers here). One that panics stops them for the run ([`CallbackGuard`]):
//! Steamworks' lock is poisoned then. A relay session request is kept until the module decides on
//! it: dropping Steam's request would refuse it. Steam shuts down when the last Steamworks handle
//! goes, with the game's world.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_void, CString};
use std::io::Read;
use std::net::Ipv4Addr;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};

use shards_core::SteamLobby;
use shards_net::{NetClock, WallClock};
use steamworks::networking_messages::{NetworkingMessages, SessionRequest};
use steamworks::networking_types::{NetworkingIdentity, SendFlags};
use steamworks::sys;
use steamworks::{
    Callback, CallbackHandle, Client, DistanceFilter, GameLobbyJoinRequested, LobbyId, LobbyKey, LobbyListFilter, LobbyType, Server, ServerMode, SteamAPIInitError,
    SteamServersConnected, StringFilter, StringFilterKind,
};

use crate::backend::{data, over_quota, Backend, CallbackGuard, Event, Info, LobbyInfo, Relay, Route, SteamId, MAX_FOUND};

/// The largest file Steam Cloud takes in one write (`k_unMaxCloudFileChunkSize`).
const MAX_CLOUD_FILE: usize = 100 * 1024 * 1024;

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
    /// Lobby searches Steam answered, as the lobby ids it listed: [`RealBackend::pump`] reads each
    /// lobby's details, so the callback holds no Steamworks handle that would keep Steam up.
    found: Shared<VecDeque<Result<Vec<SteamId>, String>>>,
    requests: Shared<BTreeMap<SteamId, SessionRequest>>,
    wall: Arc<WallClock>,
    /// Stops Steam's callbacks after one panics.
    guard: CallbackGuard,
    _callbacks: Vec<CallbackHandle>,
}

impl Drop for RealBackend {
    fn drop(&mut self) {
        // A waiting request holds a Steamworks handle and sits in a callback Steamworks holds:
        // dropped (refused) here, nothing keeps Steamworks up once the game's handles are gone,
        // and Steam shuts down with the last of them.
        lock(&self.requests).clear();
    }
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
        Err(payload) => {
            let message = ai_core::crash::panic_text(payload.as_ref());
            // Caught: the reason is the backend's (`steam` shows it), no crash report of its own.
            ai_core::crash::handled(&message);
            Err(format!("Steamworks failed while starting ({message})"))
        }
    }
}

/// What a Steam result code says (`EResult`), from the number as Steam sent it.
fn result_text(code: i32) -> String {
    let name = match code {
        2 => "failure",
        3 => "no connection",
        16 => "timeout",
        20 => "service unavailable",
        25 => "limit exceeded",
        _ => return format!("EResult {code}"),
    };
    format!("{name}, EResult {code}")
}

// Steamworks' own decoders of these callbacks panic on values Steam may send (a result code its
// table lacks, a member change with several bits), and a panic there stops every callback for the
// run (`CallbackGuard`). These read the same structs as plain numbers instead.

/// `LobbyChatUpdate_t`: `user` entered `lobby` (`entered`), or left it however (left,
/// disconnected, kicked, banned).
struct MemberUpdate {
    lobby: SteamId,
    user: SteamId,
    entered: bool,
}

// SAFETY: `ID` is `LobbyChatUpdate_t`'s callback id, and `from_raw` reads that struct, plain
// integers, from the pointer Steam passes with it.
unsafe impl Callback for MemberUpdate {
    const ID: i32 = sys::LobbyChatUpdate_t_k_iCallback as i32;

    unsafe fn from_raw(raw: *mut c_void) -> Self {
        // SAFETY: (caller) `raw` points to a `LobbyChatUpdate_t`.
        let update = unsafe { raw.cast::<sys::LobbyChatUpdate_t>().read_unaligned() };
        let entered = update.m_rgfChatMemberStateChange & sys::EChatMemberStateChange::k_EChatMemberStateChangeEntered as u32 != 0;
        Self { lobby: update.m_ulSteamIDLobby, user: update.m_ulSteamIDUserChanged, entered }
    }
}

/// `UserStatsStored_t`: Steam's answer to a store, its result code as a number.
struct StatsStored {
    result: i32,
}

// SAFETY: `ID` is `UserStatsStored_t`'s callback id; `from_raw` reads only the result's four bytes
// as an integer (as an enum, a code Steamworks does not know would be undefined behaviour).
unsafe impl Callback for StatsStored {
    const ID: i32 = sys::UserStatsStored_t_k_iCallback as i32;

    unsafe fn from_raw(raw: *mut c_void) -> Self {
        let stored = raw.cast::<sys::UserStatsStored_t>();
        // SAFETY: (caller) `raw` points to a `UserStatsStored_t`; `EResult` is `repr(i32)`.
        let result = unsafe { std::ptr::addr_of!((*stored).m_eResult).cast::<i32>().read_unaligned() };
        Self { result }
    }
}

/// `SteamServerConnectFailure_t`: a game server could not reach Steam, and whether Steam retries.
struct ConnectFailure {
    result: i32,
    retrying: bool,
}

// SAFETY: `ID` is `SteamServerConnectFailure_t`'s callback id; `from_raw` reads its result as an
// integer and its flag as a byte.
unsafe impl Callback for ConnectFailure {
    const ID: i32 = sys::SteamServerConnectFailure_t_k_iCallback as i32;

    unsafe fn from_raw(raw: *mut c_void) -> Self {
        let failure = raw.cast::<sys::SteamServerConnectFailure_t>();
        // SAFETY: (caller) `raw` points to a `SteamServerConnectFailure_t`; `EResult` is
        // `repr(i32)`, and a C++ `bool` is one byte.
        let (result, retrying) = unsafe {
            (
                std::ptr::addr_of!((*failure).m_eResult).cast::<i32>().read_unaligned(),
                std::ptr::addr_of!((*failure).m_bStillRetrying).cast::<u8>().read_unaligned() != 0,
            )
        };
        Self { result, retrying }
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
        steam._callbacks.push(steam.client.register_callback(move |update: MemberUpdate| {
            lock(&ev).push_back(Event::MemberChanged { lobby: update.lobby, user: update.user, joined: update.entered });
        }));
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |join: GameLobbyJoinRequested| {
            lock(&ev).push_back(Event::JoinRequested { lobby: join.lobby_steam_id.raw(), from: join.friend_steam_id.raw() });
        }));
        let ev = steam.events.clone();
        steam._callbacks.push(steam.client.register_callback(move |stored: StatsStored| {
            let result = if stored.result == sys::EResult::k_EResultOK as i32 { Ok(()) } else { Err(result_text(stored.result)) };
            lock(&ev).push_back(Event::StatsStored(result));
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
        steam._callbacks.push(steam.client.register_callback(move |failure: ConnectFailure| {
            let why = result_text(failure.result);
            lock(&ev).push_back(if failure.retrying {
                Event::Note(format!("the game server cannot reach Steam yet ({why}); Steam retries"))
            } else {
                Event::ServerLogOnFailed(why)
            });
        }));
        Ok(steam)
    }

    /// The parts both share: relay access and the session callbacks.
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
        // (Reading every callback, as `process_callbacks` does, would decode ones that can panic;
        // so would Steamworks' decoders of losing and finding the Steam servers, which only the
        // journal would have heard.)
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
        Self {
            client,
            server,
            me,
            name,
            app_id,
            relay_only: relay,
            events,
            found: Arc::default(),
            requests,
            wall: Arc::new(WallClock::new()),
            guard: CallbackGuard::default(),
            _callbacks: Vec::new(),
        }
    }

    /// The session interface's raw pointer: the game server's or the player's; null without one.
    fn raw_messages(&self) -> *mut sys::ISteamNetworkingMessages {
        // SAFETY: Steamworks is running (this backend exists); the accessors only return pointers.
        unsafe {
            match self.server {
                Some(_) => sys::SteamAPI_SteamGameServerNetworkingMessages_SteamAPI_v002(),
                None => sys::SteamAPI_SteamNetworkingMessages_SteamAPI_v002(),
            }
        }
    }

    /// The player's Steam Cloud interface; null on a game server.
    fn raw_storage(&self) -> *mut sys::ISteamRemoteStorage {
        if self.server.is_some() {
            return std::ptr::null_mut();
        }
        // SAFETY: Steamworks is running as the player; the accessor only returns a pointer.
        unsafe { sys::SteamAPI_SteamRemoteStorage_v016() }
    }

    fn messages(&self) -> NetworkingMessages {
        match &self.server {
            Some(server) => server.networking_messages(),
            None => self.client.networking_messages(),
        }
    }

    /// What Steam holds on the lobbies a search listed (it caches them with the list).
    fn lobby_infos(&self, lobbies: &[SteamId]) -> Vec<LobbyInfo> {
        let matchmaking = self.client.matchmaking();
        lobbies
            .iter()
            .map(|lobby| {
                let id = LobbyId::from_raw(*lobby);
                let owner = matchmaking.lobby_owner(id).raw();
                LobbyInfo {
                    lobby: *lobby,
                    owner: (owner != 0).then_some(owner),
                    members: u32::try_from(matchmaking.lobby_member_count(id)).unwrap_or(0),
                    max: matchmaking.lobby_member_limit(id).and_then(|max| u32::try_from(max).ok()).unwrap_or(0),
                    data: data::ALL.iter().filter_map(|key| Some(((*key).to_owned(), matchmaking.lobby_data(id, key)?))).collect(),
                }
            })
            .collect()
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
        let note = self.guard.stopped().map(str::to_owned);
        Info { kind, user: self.me, name: self.name.clone(), app_id: Some(self.app_id), note, relay_only: self.relay_only }
    }

    fn pump(&mut self) -> Vec<Event> {
        let client = &self.client;
        let stopped = self.guard.run(|| client.run_callbacks());
        let mut events: Vec<Event> = lock(&self.events).drain(..).collect();
        let found: Vec<_> = lock(&self.found).drain(..).collect();
        events.extend(found.into_iter().map(|answer| Event::LobbiesFound(answer.map(|ids| self.lobby_infos(&ids)))));
        events.extend(stopped);
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
        let messages = self.raw_messages();
        if messages.is_null() {
            return None;
        }
        // SAFETY: Steamworks is running (this backend exists); the identity and both structs are
        // plain data Steam fills in, zeroed first as its headers do.
        let (state, info, status) = unsafe {
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

    fn close_sessions(&mut self, peers: &[SteamId]) {
        // Dropping a waiting request refuses it.
        lock(&self.requests).clear();
        let messages = self.raw_messages();
        if messages.is_null() {
            return;
        }
        for peer in peers {
            // SAFETY: Steamworks is running; the identity is plain data, zeroed first as Steam's
            // headers do, and only read by the call.
            unsafe {
                let mut identity: sys::SteamNetworkingIdentity = std::mem::zeroed();
                sys::SteamAPI_SteamNetworkingIdentity_SetSteamID64(&mut identity, *peer);
                sys::SteamAPI_ISteamNetworkingMessages_CloseSessionWithUser(messages, &identity);
            }
        }
    }

    fn create_lobby(&mut self, max_members: u32, kind: SteamLobby) -> Result<(), String> {
        self.player_only()?;
        self.guard.answering()?;
        let kind = match kind {
            SteamLobby::Friends => LobbyType::FriendsOnly,
            SteamLobby::Private => LobbyType::Private,
            SteamLobby::Invisible => LobbyType::Invisible,
            SteamLobby::Public => LobbyType::Public,
        };
        let ev = self.events.clone();
        self.client.matchmaking().create_lobby(kind, max_members.clamp(1, MAX_LOBBY), move |created| {
            lock(&ev).push_back(Event::LobbyCreated(created.map(|lobby| lobby.raw()).map_err(|e| e.to_string())));
        });
        Ok(())
    }

    fn join_lobby(&mut self, lobby: SteamId) -> Result<(), String> {
        self.player_only()?;
        self.guard.answering()?;
        let ev = self.events.clone();
        self.client.matchmaking().join_lobby(LobbyId::from_raw(lobby), move |entered| {
            let result = entered.map(|_| ()).map_err(|()| "Steam refused (the lobby is gone or full, or you may not enter it)".to_owned());
            lock(&ev).push_back(Event::LobbyEntered { lobby, result });
        });
        Ok(())
    }

    fn find_lobbies(&mut self, equal: &[(&str, &str)]) -> Result<(), String> {
        self.player_only()?;
        self.guard.answering()?;
        // Steamworks panics on a key over 255 bytes and on a NUL inside a string.
        if let Some((key, value)) = equal.iter().find(|(key, value)| key.len() > 255 || key.contains('\0') || value.contains('\0')) {
            return Err(format!("`{key}={value}` is no lobby filter Steam takes (a key is up to 255 bytes, and no text holds a NUL)"));
        }
        let strings: Vec<StringFilter> = equal.iter().map(|(key, value)| StringFilter(LobbyKey::new(key), value, StringFilterKind::Equal)).collect();
        // One free slot: only lobbies a player can still enter. Worldwide: the friends of a player
        // may be anywhere, and the build filters leave few lobbies.
        let filter = LobbyListFilter::default().set_string(Some(strings)).set_open_slots(Some(1)).set_distance(Some(DistanceFilter::Worldwide)).set_count(Some(MAX_FOUND as u64));
        let found = self.found.clone();
        let matchmaking = self.client.matchmaking();
        matchmaking.set_lobby_list_filter(filter).request_lobby_list(move |answer| {
            lock(&found).push_back(answer.map(|lobbies| lobbies.into_iter().map(|lobby| lobby.raw()).collect()).map_err(|e| format!("Steam could not list lobbies ({e})")));
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

    fn cloud_quota(&self) -> Option<(u64, u64)> {
        let storage = self.raw_storage();
        if storage.is_null() {
            return None;
        }
        let (mut total, mut free) = (0u64, 0u64);
        // SAFETY: Steamworks is running as the player; Steam writes the two numbers.
        let known = unsafe { sys::SteamAPI_ISteamRemoteStorage_GetQuota(storage, &mut total, &mut free) };
        known.then_some((total, free))
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
        // One call that writes the whole file or nothing, and says which: a stream (Steamworks'
        // `SteamFileWriter`) commits on drop and drops the answer.
        let len = i32::try_from(bytes.len())
            .ok()
            .filter(|_| bytes.len() <= MAX_CLOUD_FILE)
            .ok_or_else(|| format!("this save is {}: Steam Cloud takes files of up to 100 MB", shards_save::ops::size_text(bytes.len() as u64)))?;
        let file = CString::new(name).map_err(|_| format!("`{name}` is no name Steam Cloud takes"))?;
        let storage = self.raw_storage();
        // SAFETY: Steamworks is running as the player (a game server's storage is null); the name
        // is a C string and `bytes` holds `len` bytes, both alive for the call, which copies them.
        let written = !storage.is_null() && unsafe { sys::SteamAPI_ISteamRemoteStorage_FileWrite(storage, file.as_ptr(), bytes.as_ptr().cast(), len) };
        if written {
            return Ok(());
        }
        // The file it replaces gives its room back (how Steam counts that is not written down).
        let replaced = self.cloud_files().into_iter().find(|(f, _)| f == name).map_or(0, |(_, size)| size);
        let quota = self.cloud_quota().map(|(total, free)| (total, free + replaced));
        Err(over_quota(quota, bytes.len() as u64).unwrap_or_else(|| "Steam did not take it (Steam Cloud is off for the game or the account, or the file is busy)".into()))
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
