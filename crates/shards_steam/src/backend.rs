//! What the module asks of Steam, whichever Steam answers: the real client (feature `sdk`,
//! [`crate::real`]), the fake of this process ([`crate::fake`]), or none ([`Unavailable`]). Nothing
//! here names a Steamworks type: the module's systems, tools and tests see only this.

use shards_net::NetClock;

/// A Steam id: a user's or a lobby's 64-bit number, as Steam shows it (`76561197960265729`).
pub type SteamId = u64;

/// What a backend says happened since the last [`Backend::pump`], in order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Event {
    /// The lobby this machine asked for exists, with it as owner and only member (or why not).
    LobbyCreated(Result<SteamId, String>),
    /// This machine entered `lobby` (or why it could not).
    LobbyEntered { lobby: SteamId, result: Result<(), String> },
    /// `user` entered or left (`joined: false`: left, disconnected, kicked) a lobby this machine is in.
    MemberChanged { lobby: SteamId, user: SteamId, joined: bool },
    /// The player accepted an invite to `lobby` from `from` (the overlay, a friend's "join game"),
    /// or Steam was asked to join it.
    JoinRequested { lobby: SteamId, from: SteamId },
    /// `from` sends datagrams over the relay and waits to be accepted ([`Backend::accept_session`]);
    /// none reach this machine until then.
    SessionRequest { from: SteamId },
    /// The relay session with `peer` failed or was closed.
    SessionFailed { peer: SteamId, reason: String },
    /// Achievements were stored on Steam (or why not).
    StatsStored(Result<(), String>),
    /// Something to journal as it is (the Steam servers lost, the relay network ready).
    #[cfg_attr(not(feature = "sdk"), allow(dead_code))]
    Note(String),
    /// A game server logged on to Steam (anonymously): its Steam id, which clients join by.
    ServerLoggedOn(SteamId),
    /// A game server could not log on, and Steam stopped trying.
    #[cfg_attr(not(feature = "sdk"), allow(dead_code))]
    ServerLogOnFailed(String),
}

/// Who this machine is on Steam.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Info {
    /// `steam` (the client, as the player), `steam server` (an anonymous game server), `fake`,
    /// `fake server`, or `none`.
    pub kind: &'static str,
    /// This machine's Steam id: the user's, or the game server's once it logged on.
    pub user: Option<SteamId>,
    pub name: String,
    pub app_id: Option<u32>,
    /// Why there is no Steam (an unavailable backend), or what this one is not.
    pub note: Option<String>,
    /// Direct routes are off: only Steam's relay carries sessions (`--steam-relay-only`).
    pub relay_only: bool,
}

/// How this machine reaches a peer over Steam: the session's state, whether a relay carries it and
/// which one (Steam's data-center code, such as `fra`), and the ping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// `connected`, `connecting`, `finding route`, `closed by peer`, `problem`, `none`.
    pub state: String,
    pub relayed: bool,
    pub relay_pop: Option<String>,
    pub ping_ms: Option<i32>,
}

/// Datagrams over the Steam relay, to and from users: the half of a backend the session's
/// transport ([`crate::transport::SteamTransport`]) owns. Unreliable, like UDP: reliability is the
/// link layer's.
pub(crate) trait Relay: Send + Sync + 'static {
    /// Sends one datagram to `to`. An error is counted, never fatal: the link layer resends.
    fn send(&mut self, to: SteamId, datagram: &[u8]) -> Result<(), String>;
    /// Appends what arrived from users whose sessions were accepted, in arrival order.
    fn recv(&mut self, now: u64, out: &mut Vec<(SteamId, Vec<u8>)>);
    /// `"steam relay"`, `"steam relay (fake)"`.
    fn describe(&self) -> String;
}

/// One Steam, as the module uses it. Methods that start something on Steam (a lobby, a join)
/// answer at once whether it started; how it ended comes as an [`Event`] from [`pump`](Self::pump).
pub(crate) trait Backend: Send + Sync + 'static {
    fn info(&self) -> Info;
    /// Whether this is a Steam at all (not [`Unavailable`]).
    fn available(&self) -> bool {
        self.info().kind != "none"
    }
    /// Runs Steam's callbacks: what happened since the last call.
    fn pump(&mut self) -> Vec<Event>;
    /// The session's clock over this relay: the wall's, or a stand's hub.
    fn clock(&self) -> NetClock;
    /// The relay's half for the session's transport.
    fn relay(&self) -> Result<Box<dyn Relay>, String>;
    /// One line about the relay network: `ready`, `connecting`, why not.
    fn relay_status(&self) -> String;
    /// How the session with `peer` goes; `None`: there is none.
    fn route(&self, peer: SteamId) -> Option<Route>;
    /// Lets `peer`'s datagrams in (and ours out to it). False when there was no request from it.
    fn accept_session(&mut self, peer: SteamId) -> bool;
    /// Refuses `peer`: what it sent is dropped.
    fn reject_session(&mut self, peer: SteamId);

    /// Starts creating a lobby for `max_members` (friends only), owned by this user.
    fn create_lobby(&mut self, max_members: u32) -> Result<(), String>;
    /// Starts entering `lobby`.
    fn join_lobby(&mut self, lobby: SteamId) -> Result<(), String>;
    fn leave_lobby(&mut self, lobby: SteamId);
    fn lobby_owner(&self, lobby: SteamId) -> Option<SteamId>;
    fn lobby_members(&self, lobby: SteamId) -> Vec<SteamId>;
    fn set_lobby_data(&mut self, lobby: SteamId, key: &str, value: &str) -> bool;
    /// A user's name as Steam shows it, when known.
    fn user_name(&self, user: SteamId) -> Option<String>;
    /// Invites `friend` to `lobby`, or opens Steam's invite dialog (`None`). Says what it did.
    fn invite(&mut self, lobby: SteamId, friend: Option<SteamId>) -> Result<String, String>;

    /// Sets achievement `api` (its name on Steam) on this user; stored with [`store_stats`](Self::store_stats).
    fn achievement_set(&mut self, api: &str) -> Result<(), String>;
    /// Whether this user has achievement `api` on Steam; `None`: Steam does not know it.
    fn achievement_get(&self, api: &str) -> Option<bool>;
    /// Sends what was set to Steam (the overlay shows the unlocks then).
    fn store_stats(&mut self) -> Result<(), String>;

    /// Ok when Steam Cloud is on for the app and the account, else why not.
    fn cloud_enabled(&self) -> Result<(), String>;
    /// Files in this user's Steam Cloud for the app: name and size.
    fn cloud_files(&self) -> Vec<(String, u64)>;
    fn cloud_read(&self, name: &str) -> Result<Vec<u8>, String>;
    fn cloud_write(&mut self, name: &str, bytes: &[u8]) -> Result<(), String>;
    /// When the file was written, in seconds since 1970.
    fn cloud_timestamp(&self, name: &str) -> Option<i64>;
}

/// No Steam: the reason is the answer to everything that needs one. Achievements are still kept
/// by the module (this run's record).
pub(crate) struct Unavailable {
    pub reason: String,
    pub app_id: Option<u32>,
}

impl Backend for Unavailable {
    fn info(&self) -> Info {
        Info { kind: "none", user: None, name: String::new(), app_id: self.app_id, note: Some(self.reason.clone()), relay_only: false }
    }
    fn pump(&mut self) -> Vec<Event> {
        Vec::new()
    }
    fn clock(&self) -> NetClock {
        NetClock::Wall(std::sync::Arc::new(shards_net::WallClock::new()))
    }
    fn relay(&self) -> Result<Box<dyn Relay>, String> {
        Err(self.reason.clone())
    }
    fn relay_status(&self) -> String {
        format!("none: {}", self.reason)
    }
    fn route(&self, _: SteamId) -> Option<Route> {
        None
    }
    fn accept_session(&mut self, _: SteamId) -> bool {
        false
    }
    fn reject_session(&mut self, _: SteamId) {}
    fn create_lobby(&mut self, _: u32) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn join_lobby(&mut self, _: SteamId) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn leave_lobby(&mut self, _: SteamId) {}
    fn lobby_owner(&self, _: SteamId) -> Option<SteamId> {
        None
    }
    fn lobby_members(&self, _: SteamId) -> Vec<SteamId> {
        Vec::new()
    }
    fn set_lobby_data(&mut self, _: SteamId, _: &str, _: &str) -> bool {
        false
    }
    fn user_name(&self, _: SteamId) -> Option<String> {
        None
    }
    fn invite(&mut self, _: SteamId, _: Option<SteamId>) -> Result<String, String> {
        Err(self.reason.clone())
    }
    fn achievement_set(&mut self, _: &str) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn achievement_get(&self, _: &str) -> Option<bool> {
        None
    }
    fn store_stats(&mut self) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn cloud_enabled(&self) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn cloud_files(&self) -> Vec<(String, u64)> {
        Vec::new()
    }
    fn cloud_read(&self, _: &str) -> Result<Vec<u8>, String> {
        Err(self.reason.clone())
    }
    fn cloud_write(&mut self, _: &str, _: &[u8]) -> Result<(), String> {
        Err(self.reason.clone())
    }
    fn cloud_timestamp(&self, _: &str) -> Option<i64> {
        None
    }
}
