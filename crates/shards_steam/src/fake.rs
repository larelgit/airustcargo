//! A fake Steam in this process: users, lobbies of the four types (a search lists the public and
//! invisible ones, as Steam's does), invites, relay sessions, achievements and cloud files, kept in
//! memory, deterministic. Tests and a stand run Steam's flows on it (host, invite, search, join, the
//! relay, an unlock, a cloud copy) without the Steam client; a game that does not
//! talk to Steam (no [`shards_core::SteamLaunch::connect`]: a test, a check) runs on a fake of its
//! own with one user.
//!
//! The relay is a [`MemoryHub`]: a user's datagrams to another wait, like Steam's, until that user
//! accepts the session ([`Event::SessionRequest`]); then a hub link joins the two and carries them,
//! with the hub's latency and loss. A fake made with [`FakeSteam::new`] shares a stand's hub, whose
//! driver moves its clock; [`FakeSteam::local`] has a hub of its own that follows the wall clock.
//!
//! ```ignore
//! let steam = FakeSteam::new(stand.hub.clone());
//! shards_steam::use_fake(&mut host, steam.user(76561197960265729, "host"));
//! shards_steam::use_fake(&mut client, steam.user(76561197960265730, "c1"));
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use shards_core::SteamLobby;
use shards_net::{LinkConditions, LinkId, MemoryHub, MemoryTransport, NetClock, PeerId, Transport, WallClock};

use crate::backend::{data, over_quota, Backend, CallbackGuard, Event, Info, LobbyInfo, Relay, Route, SteamId, MAX_FOUND};

/// The first user's id of a fake Steam that makes one itself (account 1 of the public universe).
pub const FIRST_USER: SteamId = 76_561_197_960_265_729;

/// What the first lobby's id is counted from (a chat id in the lobby instance, as Steam's are).
pub const LOBBY_BASE: SteamId = 109_775_240_917_319_680;

/// An anonymous game server's id, as Steam gives one at log-on (account type 4).
pub const FIRST_SERVER: SteamId = 0x0140_0000_0000_0001;

/// Why a game server does not do what only a player does.
const SERVER_HAS_NO: &str = "an anonymous game server has no lobbies, invites, achievements or Steam Cloud: a player's game does";

/// Datagrams one user keeps from another before its session is decided.
const HELD_MAX: usize = 256;

/// The data pairs a lobby search filters on.
type Filter = Vec<(String, String)>;

/// A fake Steam. Cloning it clones the handle: every [`FakeUser`] of it sees the same lobbies.
#[derive(Clone)]
pub struct FakeSteam {
    inner: Arc<Mutex<Universe>>,
}

struct Universe {
    hub: MemoryHub,
    /// The hub follows this clock (a fake of its own); a stand moves it instead.
    wall: Option<Arc<WallClock>>,
    users: BTreeMap<SteamId, User>,
    by_endpoint: BTreeMap<PeerId, SteamId>,
    lobbies: BTreeMap<SteamId, Lobby>,
    lobbies_made: u64,
    links: BTreeMap<(SteamId, SteamId), LinkId>,
    conditions: LinkConditions,
    /// The app's achievements on this Steam: setting another fails. `None`: any name.
    defined: Option<BTreeSet<String>>,
    /// The cloud's clock in seconds, when a test sets it; else the system's.
    cloud_time: Option<i64>,
    /// Why the lobbies asked for from now on are not made (a test's failure), if they are not.
    lobby_failure: Option<String>,
    /// Why the lobby searches from now on get no list (a test's failure), if they get none.
    search_failure: Option<String>,
    /// Searches are asked but not answered until a test lets them go (Steam is slow).
    searches_held: bool,
}

struct User {
    name: String,
    /// An anonymous game server, not a player.
    server: bool,
    /// What `--steam-relay-only` asked for (the fake's routes are relayed whatever it says).
    relay_only: bool,
    transport: MemoryTransport,
    events: VecDeque<Event>,
    /// Users whose datagrams reach this one.
    accepted: BTreeSet<SteamId>,
    /// Users who sent and wait for a decision.
    requests: BTreeSet<SteamId>,
    held: Vec<(SteamId, Vec<u8>)>,
    achieved: BTreeSet<String>,
    stored: BTreeSet<String>,
    stores: u32,
    /// How many of this user's next stores Steam refuses, when a test sets it.
    refused_stores: u32,
    cloud: BTreeMap<String, (Vec<u8>, i64)>,
    cloud_on: bool,
    /// The bytes this user's cloud holds at most, when a test sets it.
    cloud_quota: Option<u64>,
    /// The next run of this user's callbacks panics (a test's failure).
    callbacks_panic: bool,
    /// Searches asked while they were held, each as the data pairs it filters on.
    held_searches: Vec<Filter>,
}

struct Lobby {
    kind: SteamLobby,
    owner: SteamId,
    members: Vec<SteamId>,
    max: u32,
    data: BTreeMap<String, String>,
}

/// What a fake user's achievements are: set, stored on "Steam", and how many stores there were.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakeStats {
    pub set: Vec<String>,
    pub stored: Vec<String>,
    pub stores: u32,
}

impl FakeSteam {
    /// A fake whose relay is `hub`, moved by its owner (a stand).
    pub fn new(hub: MemoryHub) -> Self {
        Self::with(hub, None)
    }

    /// A fake of its own, its relay on a hub that follows the wall clock.
    pub fn local(seed: u64) -> Self {
        Self::with(MemoryHub::new(seed), Some(Arc::new(WallClock::new())))
    }

    fn with(hub: MemoryHub, wall: Option<Arc<WallClock>>) -> Self {
        let universe = Universe {
            hub,
            wall,
            users: BTreeMap::new(),
            by_endpoint: BTreeMap::new(),
            lobbies: BTreeMap::new(),
            lobbies_made: 0,
            links: BTreeMap::new(),
            conditions: LinkConditions::NONE,
            defined: None,
            cloud_time: None,
            lobby_failure: None,
            search_failure: None,
            searches_held: false,
        };
        Self { inner: Arc::new(Mutex::new(universe)) }
    }

    fn lock(&self) -> MutexGuard<'_, Universe> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// User `id` called `name` (made the first time), as a backend for one game.
    pub fn user(&self, id: SteamId, name: &str) -> FakeUser {
        self.member(id, name, false)
    }

    /// An anonymous game server with Steam id `id` (made the first time): no lobbies, no
    /// achievements, no cloud; it logs on at its first pump, and clients join it by `id`.
    pub fn server(&self, id: SteamId, name: &str) -> FakeUser {
        self.member(id, name, true)
    }

    fn member(&self, id: SteamId, name: &str, server: bool) -> FakeUser {
        let mut u = self.lock();
        if !u.users.contains_key(&id) {
            let mut events = VecDeque::new();
            if server {
                events.push_back(Event::ServerLoggedOn(id));
            }
            let transport = u.hub.endpoint(name);
            u.by_endpoint.insert(transport.id(), id);
            u.users.insert(
                id,
                User {
                    name: name.to_owned(),
                    server,
                    relay_only: false,
                    transport,
                    events,
                    accepted: BTreeSet::new(),
                    requests: BTreeSet::new(),
                    held: Vec::new(),
                    achieved: BTreeSet::new(),
                    stored: BTreeSet::new(),
                    stores: 0,
                    refused_stores: 0,
                    cloud: BTreeMap::new(),
                    cloud_on: true,
                    cloud_quota: None,
                    callbacks_panic: false,
                    held_searches: Vec::new(),
                },
            );
        }
        FakeUser { steam: self.clone(), id, guard: CallbackGuard::default() }
    }

    /// The relay's hub.
    pub fn hub(&self) -> MemoryHub {
        self.lock().hub.clone()
    }

    /// Whether the relay follows the wall clock (a fake of its own) rather than a stand's.
    pub fn real_time(&self) -> bool {
        self.lock().wall.is_some()
    }

    /// Conditions of the relay links made from now on (`latency=150,loss=0.02`).
    pub fn set_conditions(&self, conditions: LinkConditions) {
        self.lock().conditions = conditions;
    }

    /// The app's achievements as Steam knows them: setting one not named fails, as on Steam.
    pub fn define_achievements(&self, names: &[&str]) {
        self.lock().defined = Some(names.iter().map(|n| (*n).to_owned()).collect());
    }

    /// The lobbies there are, in the order they were made.
    pub fn lobbies(&self) -> Vec<SteamId> {
        self.lock().lobbies.keys().copied().collect()
    }

    pub fn lobby_members(&self, lobby: SteamId) -> Vec<SteamId> {
        self.lock().lobbies.get(&lobby).map(|l| l.members.clone()).unwrap_or_default()
    }

    pub fn lobby_data(&self, lobby: SteamId, key: &str) -> Option<String> {
        self.lock().lobbies.get(&lobby).and_then(|l| l.data.get(key).cloned())
    }

    /// The type a lobby was made with.
    pub fn lobby_type(&self, lobby: SteamId) -> Option<SteamLobby> {
        self.lock().lobbies.get(&lobby).map(|l| l.kind)
    }

    /// A lobby that `owner`'s account made in a game outside this process (a second game of the
    /// same account, or one this fake does not run): no game here is told of it. Its id.
    pub fn lobby_of(&self, owner: SteamId, kind: SteamLobby, max: u32) -> SteamId {
        let mut u = self.lock();
        u.lobbies_made += 1;
        let lobby = LOBBY_BASE + u.lobbies_made;
        u.lobbies.insert(lobby, Lobby { kind, owner, members: vec![owner], max: max.max(1), data: BTreeMap::new() });
        lobby
    }

    /// Sets `key` in `lobby`'s data as its owner would: a test's way to give a lobby another build.
    /// Whether the lobby exists.
    pub fn set_lobby_data(&self, lobby: SteamId, key: &str, value: &str) -> bool {
        self.lock().lobbies.get_mut(&lobby).map(|l| l.data.insert(key.to_owned(), value.to_owned())).is_some()
    }

    /// From now on a lobby asked for is not made, and Steam says `why` (`None`: they are made again).
    pub fn fail_lobbies(&self, why: Option<&str>) {
        self.lock().lobby_failure = why.map(str::to_owned);
    }

    /// From now on a lobby search gets no list, and Steam says `why` (`None`: they are answered again).
    pub fn fail_searches(&self, why: Option<&str>) {
        self.lock().search_failure = why.map(str::to_owned);
    }

    /// Lobby searches are asked but left unanswered (`true`) until they are let go (`false`): Steam
    /// takes its time. A search let go is answered with the lobbies there are by then.
    pub fn hold_searches(&self, held: bool) {
        let mut u = self.lock();
        u.searches_held = held;
        if held {
            return;
        }
        let waiting: Vec<(SteamId, Vec<Filter>)> =
            u.users.iter_mut().map(|(id, user)| (*id, std::mem::take(&mut user.held_searches))).filter(|(_, searches)| !searches.is_empty()).collect();
        for (id, searches) in waiting {
            for equal in searches {
                let answer = u.answer_search(&equal);
                u.user(id).events.push_back(answer);
            }
        }
    }

    /// Game server `server` does not log on: Steam gives up and says `why`. Before its game's first frame.
    pub fn fail_log_on(&self, server: SteamId, why: &str) {
        if let Some(user) = self.lock().users.get_mut(&server) {
            for event in &mut user.events {
                if matches!(event, Event::ServerLoggedOn(_)) {
                    *event = Event::ServerLogOnFailed(why.to_owned());
                }
            }
        }
    }

    /// User `user`'s Steam Cloud holds at most `bytes` (`None`: no limit), as an app's quota does.
    pub fn set_cloud_quota(&self, user: SteamId, bytes: Option<u64>) {
        if let Some(user) = self.lock().users.get_mut(&user) {
            user.cloud_quota = bytes;
        }
    }

    /// Steam refuses user `user`'s next `times` stores of achievements, as it does while it is unreachable.
    pub fn refuse_stores(&self, user: SteamId, times: u32) {
        if let Some(user) = self.lock().users.get_mut(&user) {
            user.refused_stores = times;
        }
    }

    /// The next run of user `user`'s callbacks panics, as a Steamworks decoder can: they stop for
    /// the rest of the run.
    pub fn panic_in_callbacks(&self, user: SteamId) {
        if let Some(user) = self.lock().users.get_mut(&user) {
            user.callbacks_panic = true;
        }
    }

    /// Whether `a` and `b` have a relay session (each accepted the other).
    pub fn in_session(&self, a: SteamId, b: SteamId) -> bool {
        let u = self.lock();
        let accepted = |x: SteamId, y: SteamId| u.users.get(&x).is_some_and(|user| user.accepted.contains(&y));
        accepted(a, b) && accepted(b, a)
    }

    /// User `user`'s achievements.
    pub fn stats(&self, user: SteamId) -> FakeStats {
        let u = self.lock();
        u.users.get(&user).map_or_else(FakeStats::default, |user| FakeStats {
            set: user.achieved.iter().cloned().collect(),
            stored: user.stored.iter().cloned().collect(),
            stores: user.stores,
        })
    }

    /// User `user`'s cloud files: name → (bytes, timestamp).
    pub fn cloud(&self, user: SteamId) -> BTreeMap<String, (Vec<u8>, i64)> {
        self.lock().users.get(&user).map(|u| u.cloud.clone()).unwrap_or_default()
    }

    /// Puts a file into user `user`'s cloud, as another machine would have, written at `timestamp`.
    pub fn put_cloud(&self, user: SteamId, name: &str, bytes: &[u8], timestamp: i64) {
        if let Some(user) = self.lock().users.get_mut(&user) {
            user.cloud.insert(name.to_owned(), (bytes.to_vec(), timestamp));
        }
    }

    /// Turns Steam Cloud on or off for user `user` (on by default).
    pub fn set_cloud_enabled(&self, user: SteamId, on: bool) {
        if let Some(user) = self.lock().users.get_mut(&user) {
            user.cloud_on = on;
        }
    }

    /// The time cloud files are written at from now on, in seconds since 1970 (`None`: the system's).
    pub fn set_cloud_time(&self, now: Option<i64>) {
        self.lock().cloud_time = now;
    }

    /// As if the player of `to` accepted an invite of `from` to `lobby` (the overlay's "join").
    pub fn request_join(&self, to: SteamId, lobby: SteamId, from: SteamId) {
        if let Some(user) = self.lock().users.get_mut(&to) {
            user.events.push_back(Event::JoinRequested { lobby, from });
        }
    }

    /// User `user` leaves every lobby it is in (its game quit without saying so).
    pub fn drop_user(&self, user: SteamId) {
        let mut u = self.lock();
        let lobbies: Vec<SteamId> = u.lobbies.iter().filter(|(_, l)| l.members.contains(&user)).map(|(id, _)| *id).collect();
        for lobby in lobbies {
            u.leave(user, lobby);
        }
    }
}

impl Universe {
    fn user(&mut self, id: SteamId) -> &mut User {
        self.users.get_mut(&id).expect("a fake user of this fake Steam")
    }

    fn endpoint(&self, id: SteamId) -> Option<PeerId> {
        self.users.get(&id).map(|u| u.transport.id())
    }

    fn now_secs(&self) -> i64 {
        self.cloud_time.unwrap_or_else(|| {
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
        })
    }

    /// Links `a` and `b` on the hub, once.
    fn link(&mut self, a: SteamId, b: SteamId) {
        let key = (a.min(b), a.max(b));
        if self.links.contains_key(&key) {
            return;
        }
        let (Some(ea), Some(eb)) = (self.endpoint(key.0), self.endpoint(key.1)) else { return };
        let link = self.hub.connect_both(ea, eb, self.conditions);
        self.links.insert(key, link);
    }

    /// What Steam answers a lobby search with: the lobbies it lists (public and invisible ones: a
    /// friends-only or a private lobby is never in a search) that have a free slot and every pair
    /// of `equal` in their data.
    fn answer_search(&self, equal: &[(String, String)]) -> Event {
        if let Some(why) = &self.search_failure {
            return Event::LobbiesFound(Err(why.clone()));
        }
        let found = self
            .lobbies
            .iter()
            .filter(|(_, l)| matches!(l.kind, SteamLobby::Public | SteamLobby::Invisible))
            .filter(|(_, l)| (l.members.len() as u32) < l.max)
            .filter(|(_, l)| equal.iter().all(|(key, value)| l.data.get(key) == Some(value)))
            .take(MAX_FOUND)
            .map(|(id, l)| LobbyInfo {
                lobby: *id,
                owner: Some(l.owner),
                members: l.members.len() as u32,
                max: l.max,
                data: l.data.iter().filter(|(key, _)| data::ALL.contains(&key.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
            })
            .collect();
        Event::LobbiesFound(Ok(found))
    }

    fn leave(&mut self, user: SteamId, lobby: SteamId) {
        let Some(l) = self.lobbies.get_mut(&lobby) else { return };
        let before = l.members.len();
        l.members.retain(|m| *m != user);
        if l.members.len() == before {
            return;
        }
        if l.members.is_empty() {
            self.lobbies.remove(&lobby);
            return;
        }
        // Steam hands the lobby to a member that stays.
        if l.owner == user {
            l.owner = l.members[0];
        }
        let members = l.members.clone();
        for member in members {
            self.user(member).events.push_back(Event::MemberChanged { lobby, user, joined: false });
        }
    }
}

/// One user of a [`FakeSteam`]: the backend of one game.
pub struct FakeUser {
    steam: FakeSteam,
    id: SteamId,
    guard: CallbackGuard,
}

impl FakeUser {
    pub fn id(&self) -> SteamId {
        self.id
    }

    pub fn steam(&self) -> &FakeSteam {
        &self.steam
    }

    /// What `--steam-relay-only` asked for, shown by `steam`.
    pub fn set_relay_only(&self, on: bool) {
        self.steam.lock().user(self.id).relay_only = on;
    }

    fn is_server(&self) -> bool {
        self.steam.lock().users.get(&self.id).is_some_and(|u| u.server)
    }

    /// A player's call on a game server: refused, as Steam's game server interfaces have none.
    fn player_only(&self) -> Result<(), String> {
        if self.is_server() { Err(SERVER_HAS_NO.into()) } else { Ok(()) }
    }
}

impl Backend for FakeUser {
    fn info(&self) -> Info {
        let u = self.steam.lock();
        let (name, server, relay_only) = u.users.get(&self.id).map_or((String::new(), false, false), |u| (u.name.clone(), u.server, u.relay_only));
        let note = self.guard.stopped().map(str::to_owned);
        Info { kind: if server { "fake server" } else { "fake" }, user: Some(self.id), name, app_id: None, note, relay_only }
    }

    fn pump(&mut self) -> Vec<Event> {
        let panics = std::mem::take(&mut self.steam.lock().user(self.id).callbacks_panic);
        if let Some(stopped) = self.guard.run(|| assert!(!panics, "a fake Steam callback panicked")) {
            return vec![stopped];
        }
        if self.guard.stopped().is_some() {
            return Vec::new();
        }
        self.steam.lock().user(self.id).events.drain(..).collect()
    }

    fn clock(&self) -> NetClock {
        let u = self.steam.lock();
        match &u.wall {
            Some(wall) => NetClock::Wall(wall.clone()),
            None => NetClock::Hub(u.hub.clone()),
        }
    }

    fn relay(&self) -> Result<Box<dyn Relay>, String> {
        Ok(Box::new(FakeRelay { steam: self.steam.clone(), id: self.id }))
    }

    fn relay_status(&self) -> String {
        "ready (fake: an in-process hub)".into()
    }

    fn route(&self, peer: SteamId) -> Option<Route> {
        let u = self.steam.lock();
        let (me, them) = (u.users.get(&self.id)?, u.users.get(&peer)?);
        let state = if me.accepted.contains(&peer) && them.accepted.contains(&self.id) {
            "connected"
        } else if me.requests.contains(&peer) || them.requests.contains(&self.id) {
            "connecting"
        } else {
            return None;
        };
        let key = (self.id.min(peer), self.id.max(peer));
        let ping = u.links.get(&key).map(|link| {
            let (ab, ba) = u.hub.link_conditions(*link);
            (ab.latency_ms + ba.latency_ms) as i32
        });
        Some(Route { state: state.into(), relayed: true, relay_pop: Some("fake".into()), ping_ms: ping })
    }

    fn accept_session(&mut self, peer: SteamId) -> bool {
        let mut u = self.steam.lock();
        if !u.users.contains_key(&peer) || !u.user(self.id).requests.remove(&peer) {
            return false;
        }
        u.user(self.id).accepted.insert(peer);
        // A session goes both ways: the one who asked hears the answers.
        u.user(peer).accepted.insert(self.id);
        u.link(self.id, peer);
        let held: Vec<Vec<u8>> = {
            let me = u.user(self.id);
            let (from_peer, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut me.held).into_iter().partition(|(from, _)| *from == peer);
            me.held = rest;
            from_peer.into_iter().map(|(_, bytes)| bytes).collect()
        };
        let to = u.endpoint(self.id).expect("this user");
        let sender = u.user(peer);
        for datagram in held {
            sender.transport.send(to, &datagram);
        }
        true
    }

    fn reject_session(&mut self, peer: SteamId) {
        let mut u = self.steam.lock();
        let me = u.user(self.id);
        me.requests.remove(&peer);
        me.held.retain(|(from, _)| *from != peer);
        if let Some(sender) = u.users.get_mut(&peer) {
            sender.events.push_back(Event::SessionFailed { peer: self.id, reason: "closed by the peer".into() });
        }
    }

    fn close_sessions(&mut self, peers: &[SteamId]) {
        let mut u = self.steam.lock();
        let me = self.id;
        let waiting: Vec<SteamId> = u.user(me).requests.iter().copied().collect();
        for peer in peers.iter().chain(&waiting).copied() {
            let had = u.user(me).accepted.remove(&peer) | u.user(me).requests.remove(&peer);
            u.user(me).held.retain(|(from, _)| *from != peer);
            let Some(other) = u.users.get_mut(&peer) else { continue };
            other.accepted.remove(&me);
            other.requests.remove(&me);
            other.held.retain(|(from, _)| *from != me);
            if had {
                other.events.push_back(Event::SessionFailed { peer: me, reason: "closed by the peer".into() });
            }
        }
    }

    fn create_lobby(&mut self, max_members: u32, kind: SteamLobby) -> Result<(), String> {
        self.player_only()?;
        self.guard.answering()?;
        let mut u = self.steam.lock();
        if let Some(why) = u.lobby_failure.clone() {
            u.user(self.id).events.push_back(Event::LobbyCreated(Err(why)));
            return Ok(());
        }
        u.lobbies_made += 1;
        let lobby = LOBBY_BASE + u.lobbies_made;
        u.lobbies.insert(lobby, Lobby { kind, owner: self.id, members: vec![self.id], max: max_members.max(1), data: BTreeMap::new() });
        u.user(self.id).events.push_back(Event::LobbyCreated(Ok(lobby)));
        Ok(())
    }

    fn join_lobby(&mut self, lobby: SteamId) -> Result<(), String> {
        self.player_only()?;
        self.guard.answering()?;
        let mut u = self.steam.lock();
        let result = match u.lobbies.get_mut(&lobby) {
            None => Err("no such lobby (it closed, or the id is wrong)".to_owned()),
            Some(l) if l.members.contains(&self.id) => Ok(None),
            Some(l) if l.members.len() as u32 >= l.max => Err(format!("the lobby is full ({} of {})", l.members.len(), l.max)),
            Some(l) => {
                l.members.push(self.id);
                Ok(Some(l.members.clone()))
            }
        };
        let entered = result.as_ref().map(|_| ()).map_err(Clone::clone);
        if let Ok(Some(members)) = result {
            for member in members.into_iter().filter(|m| *m != self.id) {
                u.user(member).events.push_back(Event::MemberChanged { lobby, user: self.id, joined: true });
            }
        }
        u.user(self.id).events.push_back(Event::LobbyEntered { lobby, result: entered });
        Ok(())
    }

    fn find_lobbies(&mut self, equal: &[(&str, &str)]) -> Result<(), String> {
        self.player_only()?;
        self.guard.answering()?;
        let equal: Filter = equal.iter().map(|(key, value)| ((*key).to_owned(), (*value).to_owned())).collect();
        let mut u = self.steam.lock();
        if u.searches_held {
            u.user(self.id).held_searches.push(equal);
        } else {
            let answer = u.answer_search(&equal);
            u.user(self.id).events.push_back(answer);
        }
        Ok(())
    }

    fn leave_lobby(&mut self, lobby: SteamId) {
        self.steam.lock().leave(self.id, lobby);
    }

    fn lobby_owner(&self, lobby: SteamId) -> Option<SteamId> {
        self.steam.lock().lobbies.get(&lobby).map(|l| l.owner)
    }

    fn lobby_members(&self, lobby: SteamId) -> Vec<SteamId> {
        self.steam.lobby_members(lobby)
    }

    fn set_lobby_data(&mut self, lobby: SteamId, key: &str, value: &str) -> bool {
        let mut u = self.steam.lock();
        match u.lobbies.get_mut(&lobby) {
            Some(l) if l.owner == self.id => {
                l.data.insert(key.to_owned(), value.to_owned());
                true
            }
            _ => false,
        }
    }

    fn user_name(&self, user: SteamId) -> Option<String> {
        self.steam.lock().users.get(&user).map(|u| u.name.clone())
    }

    fn invite(&mut self, lobby: SteamId, friend: Option<SteamId>) -> Result<String, String> {
        self.player_only()?;
        let mut u = self.steam.lock();
        let Some(friend) = friend else {
            let others: Vec<String> = u.users.iter().filter(|(id, _)| **id != self.id).map(|(id, user)| format!("{id} ({})", user.name)).collect();
            let others = if others.is_empty() { "none".to_owned() } else { others.join(", ") };
            return Err(format!("the fake Steam has no overlay to pick a friend in: name one, friend=STEAMID (users here: {others})"));
        };
        let Some(user) = u.users.get_mut(&friend) else {
            return Err(format!("no user {friend} on this fake Steam"));
        };
        // The fake's players always accept.
        user.events.push_back(Event::JoinRequested { lobby, from: self.id });
        Ok(format!("invited {} ({friend}) to lobby {lobby}", user.name))
    }

    fn achievement_set(&mut self, api: &str) -> Result<(), String> {
        self.player_only()?;
        let mut u = self.steam.lock();
        if u.defined.as_ref().is_some_and(|d| !d.contains(api)) {
            return Err(format!("Steam has no achievement `{api}` for this app (define it in Steamworks, or fix the name)"));
        }
        u.user(self.id).achieved.insert(api.to_owned());
        Ok(())
    }

    fn achievement_get(&self, api: &str) -> Option<bool> {
        if self.is_server() {
            return None;
        }
        let u = self.steam.lock();
        if u.defined.as_ref().is_some_and(|d| !d.contains(api)) {
            return None;
        }
        u.users.get(&self.id).map(|me| me.achieved.contains(api))
    }

    fn store_stats(&mut self) -> Result<(), String> {
        self.player_only()?;
        let mut u = self.steam.lock();
        let me = u.user(self.id);
        if me.refused_stores > 0 {
            me.refused_stores -= 1;
            return Err("Steam did not answer (the fake refuses this store)".into());
        }
        me.stored = me.achieved.clone();
        me.stores += 1;
        me.events.push_back(Event::StatsStored(Ok(())));
        Ok(())
    }

    fn cloud_enabled(&self) -> Result<(), String> {
        self.player_only()?;
        let u = self.steam.lock();
        match u.users.get(&self.id) {
            Some(me) if me.cloud_on => Ok(()),
            _ => Err("Steam Cloud is off for this account (fake)".into()),
        }
    }

    fn cloud_files(&self) -> Vec<(String, u64)> {
        let u = self.steam.lock();
        u.users.get(&self.id).map(|me| me.cloud.iter().map(|(name, (bytes, _))| (name.clone(), bytes.len() as u64)).collect()).unwrap_or_default()
    }

    fn cloud_quota(&self) -> Option<(u64, u64)> {
        let u = self.steam.lock();
        let me = u.users.get(&self.id)?;
        let total = me.cloud_quota?;
        let used: u64 = me.cloud.values().map(|(bytes, _)| bytes.len() as u64).sum();
        Some((total, total.saturating_sub(used)))
    }

    fn cloud_read(&self, name: &str) -> Result<Vec<u8>, String> {
        let u = self.steam.lock();
        u.users.get(&self.id).and_then(|me| me.cloud.get(name)).map(|(bytes, _)| bytes.clone()).ok_or_else(|| format!("no cloud file `{name}`"))
    }

    fn cloud_write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.cloud_enabled()?;
        // The file it replaces gives its room back (how Steam counts that is not written down).
        let replaced = self.cloud_files().into_iter().find(|(file, _)| file == name).map_or(0, |(_, size)| size);
        let quota = self.cloud_quota().map(|(total, free)| (total, free + replaced));
        if let Some(full) = over_quota(quota, bytes.len() as u64) {
            return Err(full);
        }
        let mut u = self.steam.lock();
        let now = u.now_secs();
        u.user(self.id).cloud.insert(name.to_owned(), (bytes.to_vec(), now));
        Ok(())
    }

    fn cloud_timestamp(&self, name: &str) -> Option<i64> {
        let u = self.steam.lock();
        u.users.get(&self.id).and_then(|me| me.cloud.get(name)).map(|(_, at)| *at)
    }
}

/// A fake user's relay: the hub, behind the sessions its user accepted.
struct FakeRelay {
    steam: FakeSteam,
    id: SteamId,
}

impl Relay for FakeRelay {
    fn send(&mut self, to: SteamId, datagram: &[u8]) -> Result<(), String> {
        let mut u = self.steam.lock();
        if !u.users.contains_key(&to) {
            return Err(format!("no user {to} on this fake Steam"));
        }
        let me = self.id;
        if u.user(to).accepted.contains(&me) {
            let endpoint = u.endpoint(to).expect("checked above");
            u.user(me).transport.send(endpoint, datagram);
            return Ok(());
        }
        // No session yet: the datagram waits for the other side to decide, as on Steam.
        let target = u.user(to);
        if target.held.len() < HELD_MAX {
            target.held.push((me, datagram.to_vec()));
        }
        if target.requests.insert(me) {
            target.events.push_back(Event::SessionRequest { from: me });
        }
        Ok(())
    }

    fn recv(&mut self, now: u64, out: &mut Vec<(SteamId, Vec<u8>)>) {
        let mut u = self.steam.lock();
        if u.wall.is_some() {
            u.hub.set_time(now);
        }
        let mut datagrams = Vec::new();
        u.user(self.id).transport.recv(now, &mut datagrams);
        for (from, bytes) in datagrams {
            if let Some(user) = u.by_endpoint.get(&from) {
                out.push((*user, bytes));
            }
        }
    }

    fn describe(&self) -> String {
        "steam relay (fake)".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: SteamId = FIRST_USER;
    const B: SteamId = FIRST_USER + 1;
    const C: SteamId = FIRST_USER + 2;

    fn received(relay: &mut Box<dyn Relay>, hub: &MemoryHub) -> Vec<(SteamId, Vec<u8>)> {
        let mut out = Vec::new();
        relay.recv(hub.now(), &mut out);
        out
    }

    #[test]
    fn a_lobby_is_made_entered_and_left_with_events_to_its_members() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let (mut a, mut b) = (steam.user(A, "a"), steam.user(B, "b"));
        a.create_lobby(2, SteamLobby::Friends).unwrap();
        let Event::LobbyCreated(Ok(lobby)) = a.pump()[0].clone() else { panic!("no lobby") };
        assert_eq!(lobby, LOBBY_BASE + 1);
        b.join_lobby(lobby).unwrap();
        assert_eq!(b.pump(), [Event::LobbyEntered { lobby, result: Ok(()) }]);
        assert_eq!(a.pump(), [Event::MemberChanged { lobby, user: B, joined: true }]);
        assert_eq!((b.lobby_owner(lobby), b.lobby_members(lobby)), (Some(A), vec![A, B]));

        // Full: a third is refused, and told why.
        let mut c = steam.user(C, "c");
        c.join_lobby(lobby).unwrap();
        assert_eq!(c.pump(), [Event::LobbyEntered { lobby, result: Err("the lobby is full (2 of 2)".into()) }]);
        c.join_lobby(LOBBY_BASE + 9).unwrap();
        assert!(matches!(&c.pump()[0], Event::LobbyEntered { result: Err(e), .. } if e.contains("no such lobby")));

        // The owner leaves: the lobby is the other member's.
        a.leave_lobby(lobby);
        assert_eq!(b.pump(), [Event::MemberChanged { lobby, user: A, joined: false }]);
        assert_eq!(b.lobby_owner(lobby), Some(B));
        b.leave_lobby(lobby);
        assert!(steam.lobbies().is_empty(), "an empty lobby closes");
    }

    #[test]
    fn datagrams_wait_for_the_session_and_flow_both_ways_once_accepted() {
        let hub = MemoryHub::new(1);
        let steam = FakeSteam::new(hub.clone());
        let (mut a, mut b) = (steam.user(A, "a"), steam.user(B, "b"));
        let (mut ra, mut rb) = (a.relay().unwrap(), b.relay().unwrap());
        ra.send(B, b"hello").unwrap();
        ra.send(B, b"again").unwrap();
        assert!(received(&mut rb, &hub).is_empty(), "nothing before b accepts");
        assert_eq!(b.pump(), [Event::SessionRequest { from: A }], "one request, however many datagrams");
        assert!(b.accept_session(A));
        assert!(!b.accept_session(A), "no second request to accept");
        assert_eq!(received(&mut rb, &hub), [(A, b"hello".to_vec()), (A, b"again".to_vec())], "the held ones, in order");
        rb.send(A, b"welcome").unwrap();
        assert_eq!(received(&mut ra, &hub), [(B, b"welcome".to_vec())], "the one who asked hears the answer");
        assert!(a.pump().is_empty());
    }

    #[test]
    fn a_refused_session_drops_what_was_sent_and_tells_the_sender() {
        let hub = MemoryHub::new(1);
        let steam = FakeSteam::new(hub.clone());
        let (mut a, mut b) = (steam.user(A, "a"), steam.user(B, "b"));
        let (mut ra, mut rb) = (a.relay().unwrap(), b.relay().unwrap());
        ra.send(B, b"let me in").unwrap();
        assert_eq!(b.pump(), [Event::SessionRequest { from: A }]);
        b.reject_session(A);
        assert_eq!(a.pump(), [Event::SessionFailed { peer: B, reason: "closed by the peer".into() }]);
        assert!(received(&mut rb, &hub).is_empty());
        // Sending again asks again (Steam's AUTO_RESTART_BROKEN_SESSION).
        ra.send(B, b"please").unwrap();
        assert_eq!(b.pump(), [Event::SessionRequest { from: A }]);
        assert!(ra.send(C, b"?").is_err(), "no such user");
    }

    #[test]
    fn the_relay_keeps_the_hub_latency() {
        let hub = MemoryHub::new(1);
        let steam = FakeSteam::new(hub.clone());
        steam.set_conditions("latency=100".parse().unwrap());
        let (mut a, mut b) = (steam.user(A, "a"), steam.user(B, "b"));
        let (mut ra, mut rb) = (a.relay().unwrap(), b.relay().unwrap());
        ra.send(B, b"x").unwrap();
        b.pump();
        b.accept_session(A);
        assert!(received(&mut rb, &hub).is_empty(), "100 ms on the way");
        hub.advance(100);
        assert_eq!(received(&mut rb, &hub).len(), 1);
        assert!(a.pump().is_empty());
    }

    #[test]
    fn achievements_are_set_then_stored_and_unknown_names_fail_when_the_app_defines_its_own() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let mut a = steam.user(A, "a");
        a.achievement_set("ACH_ANY").unwrap();
        assert_eq!(steam.stats(A), FakeStats { set: vec!["ACH_ANY".into()], stored: vec![], stores: 0 });
        a.store_stats().unwrap();
        assert_eq!(steam.stats(A).stored, ["ACH_ANY"]);
        assert_eq!(a.pump(), [Event::StatsStored(Ok(()))]);
        steam.define_achievements(&["ACH_WIN"]);
        assert!(a.achievement_set("ACH_TYPO").unwrap_err().contains("no achievement `ACH_TYPO`"));
        assert_eq!(a.achievement_get("ACH_TYPO"), None);
        assert_eq!(a.achievement_get("ACH_WIN"), Some(false));
    }

    #[test]
    fn cloud_files_keep_their_time_and_follow_the_switch() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let mut a = steam.user(A, "a");
        steam.set_cloud_time(Some(1_000));
        a.cloud_write("saves/world/slot1.save", b"one").unwrap();
        assert_eq!(a.cloud_files(), [("saves/world/slot1.save".to_owned(), 3)]);
        assert_eq!((a.cloud_read("saves/world/slot1.save").unwrap(), a.cloud_timestamp("saves/world/slot1.save")), (b"one".to_vec(), Some(1_000)));
        steam.set_cloud_enabled(A, false);
        assert!(a.cloud_write("x", b"").unwrap_err().contains("off"));
    }

    #[test]
    fn an_invite_reaches_the_friend_and_the_overlay_is_a_real_steams() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let (mut a, mut b) = (steam.user(A, "a"), steam.user(B, "b"));
        assert!(a.invite(7, None).unwrap_err().contains(&format!("{B} (b)")));
        assert_eq!(a.invite(7, Some(B)).unwrap(), format!("invited b ({B}) to lobby 7"));
        assert_eq!(b.pump(), [Event::JoinRequested { lobby: 7, from: A }]);
    }

    #[test]
    fn a_game_server_logs_on_takes_sessions_by_id_and_does_nothing_a_player_does() {
        let hub = MemoryHub::new(1);
        let steam = FakeSteam::new(hub.clone());
        steam.set_conditions("latency=20".parse().unwrap());
        let (mut server, a) = (steam.server(FIRST_SERVER, "server"), steam.user(A, "a"));
        assert_eq!(server.pump(), [Event::ServerLoggedOn(FIRST_SERVER)]);
        assert_eq!(server.info().kind, "fake server");
        assert_eq!(a.info().kind, "fake");
        for refused in [server.create_lobby(4, SteamLobby::Friends), server.join_lobby(LOBBY_BASE + 1), server.achievement_set("ACH"), server.store_stats(), server.cloud_enabled()] {
            assert_eq!(refused.unwrap_err(), SERVER_HAS_NO);
        }
        assert_eq!(server.achievement_get("ACH"), None);

        let (mut rs, mut ra) = (server.relay().unwrap(), a.relay().unwrap());
        assert_eq!(a.route(FIRST_SERVER), None, "no session yet");
        ra.send(FIRST_SERVER, b"hello").unwrap();
        assert_eq!(a.route(FIRST_SERVER).unwrap().state, "connecting");
        assert_eq!(server.pump(), [Event::SessionRequest { from: A }]);
        assert!(server.accept_session(A));
        hub.advance(20);
        assert_eq!(received(&mut rs, &hub), [(A, b"hello".to_vec())]);
        let route = server.route(A).unwrap();
        assert_eq!(route, Route { state: "connected".into(), relayed: true, relay_pop: Some("fake".into()), ping_ms: Some(40) });
        rs.send(A, b"welcome").unwrap();
        hub.advance(20);
        assert_eq!(received(&mut ra, &hub), [(FIRST_SERVER, b"welcome".to_vec())]);
        a.set_relay_only(true);
        assert!(a.info().relay_only);
    }

    #[test]
    fn lobbies_keep_their_type_and_a_test_can_make_them_fail() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let mut a = steam.user(A, "a");
        a.create_lobby(4, SteamLobby::Private).unwrap();
        let Event::LobbyCreated(Ok(lobby)) = a.pump()[0].clone() else { panic!("no lobby") };
        assert_eq!(steam.lobby_type(lobby), Some(SteamLobby::Private));
        steam.fail_lobbies(Some("k_EResultFail"));
        a.create_lobby(4, SteamLobby::Friends).unwrap();
        assert_eq!(a.pump(), [Event::LobbyCreated(Err("k_EResultFail".into()))]);
        assert_eq!(steam.lobbies(), [lobby], "none made");

        let mut server = steam.server(FIRST_SERVER, "server");
        steam.fail_log_on(FIRST_SERVER, "no connection");
        assert_eq!(server.pump(), [Event::ServerLogOnFailed("no connection".into())]);
    }

    #[test]
    fn a_full_cloud_refuses_a_file_and_says_how_full_it_is() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let mut a = steam.user(A, "a");
        steam.set_cloud_quota(A, Some(1_000));
        a.cloud_write("one", &[0; 600]).unwrap();
        assert_eq!(a.cloud_quota(), Some((1_000, 400)));
        assert_eq!(a.cloud_write("two", &[0; 540]).unwrap_err(), "Steam Cloud has 400 B free of 1000 B; this save is 540 B");
        a.cloud_write("one", &[0; 900]).unwrap();
        assert_eq!(a.cloud_files(), [("one".to_owned(), 900)], "a file that replaces another takes its room");
    }

    #[test]
    fn closed_sessions_end_on_both_sides_and_a_waiting_request_is_refused() {
        let hub = MemoryHub::new(1);
        let steam = FakeSteam::new(hub.clone());
        let (mut a, mut b, c) = (steam.user(A, "a"), steam.user(B, "b"), steam.user(C, "c"));
        let (mut ra, mut rc) = (a.relay().unwrap(), c.relay().unwrap());
        ra.send(B, b"hello").unwrap();
        b.pump();
        assert!(b.accept_session(A));
        rc.send(B, b"me too").unwrap();
        assert!(steam.in_session(A, B));
        b.close_sessions(&[A]);
        assert!(!steam.in_session(A, B));
        assert_eq!(a.pump(), [Event::SessionFailed { peer: B, reason: "closed by the peer".into() }]);
        assert_eq!(b.route(C), None, "c's request went with the rest");
        drop(c);
    }

    #[test]
    fn callbacks_that_panic_stop_and_nothing_comes_after() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let (mut a, b) = (steam.user(A, "a"), steam.user(B, "b"));
        steam.request_join(A, 7, B);
        steam.panic_in_callbacks(A);
        assert!(matches!(&a.pump()[..], [Event::CallbacksStopped(why)] if why.contains("a fake Steam callback panicked")));
        assert!(a.info().note.is_some_and(|n| n.contains("restart the game")));
        steam.request_join(A, 8, B);
        assert!(a.pump().is_empty(), "stopped for the rest of the run");
        assert!(b.info().note.is_none(), "another user's Steam goes on");
    }

    const D: SteamId = FIRST_USER + 3;
    const E: SteamId = FIRST_USER + 4;

    /// A lobby of `kind` for `max` made by `owner`, with `data` set as its host would.
    fn lobby(steam: &FakeSteam, owner: SteamId, kind: SteamLobby, max: u32, data: &[(&str, &str)]) -> SteamId {
        let mut host = steam.user(owner, "host");
        host.create_lobby(max, kind).unwrap();
        let Event::LobbyCreated(Ok(lobby)) = host.pump()[0].clone() else { panic!("no lobby") };
        for (key, value) in data {
            assert!(host.set_lobby_data(lobby, key, value));
        }
        lobby
    }

    /// What `user`'s search for lobbies whose data holds `equal` answers.
    fn found(user: &mut FakeUser, equal: &[(&str, &str)]) -> Vec<LobbyInfo> {
        user.find_lobbies(equal).unwrap();
        match &user.pump()[..] {
            [Event::LobbiesFound(Ok(found))] => found.clone(),
            other => panic!("not a list: {other:?}"),
        }
    }

    fn ids(found: &[LobbyInfo]) -> Vec<SteamId> {
        found.iter().map(|l| l.lobby).collect()
    }

    #[test]
    fn a_search_lists_the_lobbies_with_room_whose_data_holds_what_it_asks_for() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let a = lobby(&steam, A, SteamLobby::Public, 4, &[("game", "g 1"), ("content", "aa"), ("host", "a"), ("note", "not read back")]);
        let b = lobby(&steam, B, SteamLobby::Invisible, 2, &[("game", "g 1"), ("content", "bb"), ("host", "b")]);
        let c = lobby(&steam, C, SteamLobby::Public, 4, &[("game", "another game")]);
        let mut me = steam.user(D, "d");

        let g1 = found(&mut me, &[("game", "g 1")]);
        assert_eq!(ids(&g1), [a, b], "the other game's lobby is none of ours, and they come in the order they were made");
        let data = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect::<BTreeMap<_, _>>();
        assert_eq!(
            g1[0],
            LobbyInfo { lobby: a, owner: Some(A), members: 1, max: 4, data: data(&[("content", "aa"), ("game", "g 1"), ("host", "a")]) },
            "the lobby's size and the data a search reads (not every key)"
        );
        assert_eq!((g1[1].members, g1[1].max, g1[1].owner), (1, 2, Some(B)));
        assert_eq!(ids(&found(&mut me, &[("game", "g 1"), ("content", "bb")])), [b], "every pair must hold");
        assert_eq!(ids(&found(&mut me, &[("game", "g 1"), ("content", "cc")])), Vec::<SteamId>::new(), "a value no lobby has");
        assert_eq!(ids(&found(&mut me, &[("game", "g 1"), ("rules", "aa")])), Vec::<SteamId>::new(), "a key no lobby has set");
        assert_eq!(ids(&found(&mut me, &[])), [a, b, c], "no filter, every lobby the search lists");
    }

    #[test]
    fn only_public_and_invisible_lobbies_are_ever_found() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let made: Vec<(SteamLobby, SteamId)> =
            SteamLobby::ALL.iter().zip([A, B, C, D]).map(|(kind, owner)| (*kind, lobby(&steam, owner, *kind, 4, &[("game", "g")]))).collect();
        let mut me = steam.user(E, "e");
        let listed = ids(&found(&mut me, &[("game", "g")]));
        let expected: Vec<SteamId> = made.iter().filter(|(kind, _)| kind.in_search()).map(|(_, id)| *id).collect();
        assert_eq!(listed, expected);
        assert_eq!(expected.len(), 2);
        for (kind, id) in &made {
            assert_eq!(listed.contains(id), matches!(kind, SteamLobby::Public | SteamLobby::Invisible), "{kind}: Steam's search lists public and invisible lobbies only");
            // The type is the lobby's, not the search's: a friend can still enter a friends-only lobby by its id.
            assert_eq!(steam.lobby_type(*id), Some(*kind));
        }
    }

    #[test]
    fn a_full_lobby_is_not_listed_and_is_again_when_someone_leaves() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let lobby_id = lobby(&steam, A, SteamLobby::Public, 2, &[("game", "g")]);
        let mut me = steam.user(C, "c");
        assert_eq!(ids(&found(&mut me, &[("game", "g")])), [lobby_id], "one of two");
        let mut b = steam.user(B, "b");
        b.join_lobby(lobby_id).unwrap();
        b.pump();
        assert_eq!(ids(&found(&mut me, &[("game", "g")])), Vec::<SteamId>::new(), "two of two: nobody else can enter");
        b.leave_lobby(lobby_id);
        let again = found(&mut me, &[("game", "g")]);
        assert_eq!((ids(&again), again[0].members), ([lobby_id].to_vec(), 1));
    }

    #[test]
    fn a_search_can_be_held_or_fail_and_a_game_server_cannot_search() {
        let steam = FakeSteam::new(MemoryHub::new(1));
        let lobby_id = lobby(&steam, A, SteamLobby::Invisible, 4, &[("game", "g")]);
        let mut me = steam.user(B, "b");

        // Steam takes its time: nothing comes until it does, and then what is there by then.
        steam.hold_searches(true);
        me.find_lobbies(&[("game", "g")]).unwrap();
        assert!(me.pump().is_empty());
        let late = lobby(&steam, C, SteamLobby::Public, 4, &[("game", "g")]);
        steam.hold_searches(false);
        let Event::LobbiesFound(Ok(list)) = me.pump()[0].clone() else { panic!("no list") };
        assert_eq!(ids(&list), [lobby_id, late]);

        steam.fail_searches(Some("k_EResultTimeout"));
        me.find_lobbies(&[]).unwrap();
        assert_eq!(me.pump(), [Event::LobbiesFound(Err("k_EResultTimeout".into()))]);
        steam.fail_searches(None);

        let mut server = steam.server(FIRST_SERVER, "server");
        assert_eq!(server.find_lobbies(&[]).unwrap_err(), SERVER_HAS_NO);
        steam.panic_in_callbacks(B);
        me.pump();
        assert!(me.find_lobbies(&[]).unwrap_err().contains("callbacks stop for this run"), "no answer would come");
    }
}
