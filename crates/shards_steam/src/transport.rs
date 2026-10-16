//! [`SteamTransport`]: the co-op session's datagrams over the Steam relay (`ISteamNetworkingMessages`
//! on channel 0, unreliable without Nagle, a broken session restarted by the next send). A peer is a
//! user's Steam id. Only users the module accepted reach it: lobby members on a host, the lobby's
//! owner on a client ([`crate::session`]).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use shards_net::{PeerId, Transport, MTU};

use crate::backend::Relay;

/// What went over the relay, both ways, for `steam` and `ai/status`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RelayStats {
    pub sent: u64,
    pub sent_bytes: u64,
    pub received: u64,
    pub received_bytes: u64,
    /// Sends Steam refused (the link layer sends again).
    pub errors: u64,
    pub last_error: Option<String>,
    /// Who this machine sent to or heard from: the peers whose routes `steam` shows.
    pub peers: BTreeSet<PeerId>,
}

/// The session's transport over the Steam relay.
pub struct SteamTransport {
    relay: Box<dyn Relay>,
    me: PeerId,
    stats: Arc<Mutex<RelayStats>>,
}

impl SteamTransport {
    pub(crate) fn new(relay: Box<dyn Relay>, me: PeerId, stats: Arc<Mutex<RelayStats>>) -> Self {
        Self { relay, me, stats }
    }

    fn count(&self, f: impl FnOnce(&mut RelayStats)) {
        if let Ok(mut stats) = self.stats.lock() {
            f(&mut stats);
        }
    }
}

impl Transport for SteamTransport {
    fn send(&mut self, to: PeerId, datagram: &[u8]) {
        if datagram.len() > MTU {
            self.count(|s| {
                s.errors += 1;
                s.last_error = Some(format!("a datagram of {} bytes is over the MTU", datagram.len()));
            });
            return;
        }
        let sent = self.relay.send(to, datagram);
        self.count(|s| match sent {
            Ok(()) => {
                s.sent += 1;
                s.sent_bytes += datagram.len() as u64;
                s.peers.insert(to);
            }
            Err(e) => {
                s.errors += 1;
                s.last_error = Some(e);
            }
        });
    }

    fn recv(&mut self, now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        let before = out.len();
        self.relay.recv(now, out);
        let new = &out[before..];
        let (n, bytes) = (new.len() as u64, new.iter().map(|(_, b)| b.len() as u64).sum::<u64>());
        if n > 0 {
            self.count(|s| {
                s.received += n;
                s.received_bytes += bytes;
                s.peers.extend(new.iter().map(|(from, _)| *from));
            });
        }
    }

    fn describe(&self) -> String {
        format!("{} as {}", self.relay.describe(), self.me)
    }

    /// The address peers reach this game at: its Steam id.
    fn local_addr(&self) -> Option<String> {
        Some(format!("steam:{}", self.me))
    }

    // `forget`: nothing to do. Steam closes a relay session that stays idle; a peer that comes
    // back asks for a new one, which the module decides on again.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::fake::{FakeSteam, FIRST_USER};
    use shards_net::MemoryHub;

    #[test]
    fn the_transport_counts_what_goes_over_the_relay_and_names_its_peer_by_steam_id() {
        let hub = MemoryHub::new(1);
        let steam = FakeSteam::new(hub.clone());
        let (a, mut b) = (steam.user(FIRST_USER, "a"), steam.user(FIRST_USER + 1, "b"));
        let stats = Arc::new(Mutex::new(RelayStats::default()));
        let mut ta = SteamTransport::new(a.relay().unwrap(), FIRST_USER, stats.clone());
        let mut tb = SteamTransport::new(b.relay().unwrap(), FIRST_USER + 1, Arc::new(Mutex::new(RelayStats::default())));
        ta.send(FIRST_USER + 1, b"hello");
        ta.send(FIRST_USER + 1, &[0; MTU + 1]);
        ta.send(42, b"nobody");
        b.pump();
        assert!(b.accept_session(FIRST_USER));
        let mut out = Vec::new();
        tb.recv(hub.now(), &mut out);
        assert_eq!(out, [(FIRST_USER, b"hello".to_vec())]);
        let s = stats.lock().unwrap().clone();
        assert_eq!((s.sent, s.sent_bytes, s.errors), (1, 5, 2), "{s:?}");
        assert!(s.last_error.unwrap().contains("no user 42"));
        assert_eq!(ta.local_addr().as_deref(), Some("steam:76561197960265729"));
        assert_eq!(ta.describe(), "steam relay (fake) as 76561197960265729");
    }
}
