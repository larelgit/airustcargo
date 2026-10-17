//! What the module asks of Steam, whichever Steam answers: the real client (feature `sdk`,
//! [`crate::real`]), the fake of this process ([`crate::fake`]), or none ([`Unavailable`]). Nothing
//! here names a Steamworks type: the module's systems, tools and tests see only this.

use std::panic::{catch_unwind, AssertUnwindSafe};

use shards_core::SteamLobby;
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
    ServerLogOnFailed(String),
    /// Steam's callbacks stopped for this run (one panicked): nothing more comes from Steam.
    CallbacksStopped(String),
}

/// Runs Steam's callbacks until one panics, then never again. A panic inside Steamworks' callback
/// loop poisons the lock it holds: every later run would panic at once and lose what Steam queued
/// behind it, so after the first the callbacks stay stopped, the journal says so once (an
/// [`Event::CallbacksStopped`]), and `steam` shows why.
#[derive(Debug, Default)]
pub(crate) struct CallbackGuard {
    stopped: Option<String>,
}

impl CallbackGuard {
    /// Runs `callbacks` unless they stopped; the event that says they did, the first time.
    pub fn run(&mut self, callbacks: impl FnOnce()) -> Option<Event> {
        if self.stopped.is_some() {
            return None;
        }
        let payload = catch_unwind(AssertUnwindSafe(callbacks)).err()?;
        let message = ai_core::crash::panic_text(payload.as_ref());
        // Caught here: no crash report of its own (the journal and `steam` say it).
        ai_core::crash::handled(&message);
        let why = format!(
            "a Steam callback panicked ({message}), so Steam's callbacks stop for this run: lobbies, invites, relay sessions and \
             achievement stores no longer answer; restart the game"
        );
        self.stopped = Some(why.clone());
        Some(Event::CallbacksStopped(why))
    }

    /// Why the callbacks stopped, if they did.
    pub fn stopped(&self) -> Option<&str> {
        self.stopped.as_deref()
    }

    /// Ok while the callbacks run; once they stopped, why: a call whose answer comes through them
    /// (a lobby made, a lobby entered) would wait for ever, so it is refused.
    pub fn answering(&self) -> Result<(), String> {
        self.stopped.as_ref().map_or(Ok(()), |why| Err(why.clone()))
    }
}

/// Why Steam Cloud refused a file of `size` bytes, when its quota is the reason: `Steam Cloud has
/// 105 B free of 4 KB; this save is 540 B`. `None`: it would fit (`quota`: total and free bytes).
pub(crate) fn over_quota(quota: Option<(u64, u64)>, size: u64) -> Option<String> {
    let (total, free) = quota?;
    let text = shards_save::ops::size_text;
    (size > free).then(|| format!("Steam Cloud has {} free of {}; this save is {}", text(free), text(total), text(size)))
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
    /// Why there is no Steam (an unavailable backend), or why its callbacks stopped ([`CallbackGuard`]).
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
    /// Closes the relay sessions with `peers` (the game is quitting: they hear it now, not when
    /// the sessions time out) and refuses every request still waiting.
    fn close_sessions(&mut self, peers: &[SteamId]);

    /// Starts creating a lobby of type `kind` for `max_members`, owned by this user.
    fn create_lobby(&mut self, max_members: u32, kind: SteamLobby) -> Result<(), String>;
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
    /// This user's Steam Cloud quota for the app: total and free bytes, when Steam says.
    fn cloud_quota(&self) -> Option<(u64, u64)>;
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
    fn close_sessions(&mut self, _: &[SteamId]) {}
    fn create_lobby(&mut self, _: u32, _: SteamLobby) -> Result<(), String> {
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
    fn cloud_quota(&self) -> Option<(u64, u64)> {
        None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callbacks_that_panic_once_stop_and_say_so_once() {
        let mut guard = CallbackGuard::default();
        let mut runs = 0;
        assert_eq!(guard.run(|| runs += 1), None, "callbacks that return are run again");
        let Some(Event::CallbacksStopped(why)) = guard.run(|| panic!("entered unreachable code")) else { panic!("no event") };
        assert!(why.starts_with("a Steam callback panicked (entered unreachable code), so Steam's callbacks stop for this run"), "{why}");
        assert_eq!(guard.stopped(), Some(why.as_str()));
        for _ in 0..3 {
            assert_eq!(guard.run(|| runs += 1), None, "not run again, and not said again");
        }
        assert_eq!(runs, 1);
    }

    #[test]
    fn a_full_cloud_says_how_full_and_how_big_the_save_is() {
        assert_eq!(over_quota(Some((4096, 105)), 540).as_deref(), Some("Steam Cloud has 105 B free of 4 KB; this save is 540 B"));
        assert_eq!(over_quota(Some((102_400, 2_000)), 10_000).as_deref(), Some("Steam Cloud has 2 KB free of 100 KB; this save is 10 KB"));
        assert_eq!(over_quota(Some((4096, 600)), 540), None, "it fits: something else refused it");
        assert_eq!(over_quota(None, 540), None, "Steam did not say");
    }
}
