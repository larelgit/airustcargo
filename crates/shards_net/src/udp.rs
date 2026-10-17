//! Direct UDP: the LAN transport. `std::net::UdpSocket`, non-blocking, no encryption (LAN and direct
//! play; the link layer adds sessions and reliability, a relay transport can add the rest later).
//!
//! The only code in the crate that touches the wall clock is [`WallClock`], which hands out
//! milliseconds since it was created; callers pass them as `now` like any other clock.
//!
//! Strangers: a datagram from an unknown address is dropped unless it starts like ours, of any
//! version ([`wire::is_ours`]: an older build is answered with a refusal). An address that does is
//! kept in a small table of 64 pending senders (least recently heard of replaced first, silent ones
//! forgotten after 10 s) and handed up to the session layer; it becomes a peer in the table of 512
//! linked ones when the session layer answers it with [`Transport::send`]. What the session says to
//! a stranger before the join's cookie came back goes by [`Transport::reply`], which links nothing.
//! So junk and floods never fill the table of peers that have links.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Instant;

use crate::transport::{MTU, PeerId, Transport};
use crate::wire;

/// Peers kept in the address table: the ones that have a link (the host replied to them, or the game
/// called [`UdpTransport::connect`]). Beyond this no address is added until a link calls
/// [`Transport::forget`]. Bounds memory.
const MAX_PEERS: usize = 512;
/// Addresses that sent a datagram of ours but are not linked yet. Few, and the least recently heard of is
/// replaced first, so a flood of senders can crowd the table but never lock anyone out.
const MAX_PENDING: usize = 64;
/// A pending address silent for this long (ms on the caller's clock) is forgotten.
const PENDING_TTL_MS: u64 = 10_000;
/// Datagrams taken per `recv` call, so a flood cannot starve the tick.
const MAX_PER_RECV: usize = 4096;

/// The `PeerId` of a socket address: FNV-1a over the address bytes, so it is the same on every
/// machine and every Rust version (std's hashers promise neither).
pub fn peer_id(addr: SocketAddr) -> PeerId {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u8| {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    match addr {
        SocketAddr::V4(a) => {
            eat(4);
            a.ip().octets().into_iter().for_each(&mut eat);
        }
        SocketAddr::V6(a) => {
            eat(6);
            a.ip().octets().into_iter().for_each(&mut eat);
        }
    }
    addr.port().to_le_bytes().into_iter().for_each(&mut eat);
    h
}

/// Milliseconds since creation. UDP is the one place where time is real.
pub struct WallClock {
    start: Instant,
}

impl WallClock {
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }

    pub fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl Default for WallClock {
    fn default() -> Self {
        Self::new()
    }
}

/// An address heard from but not (yet) linked.
struct Pending {
    addr: SocketAddr,
    /// The caller's clock when it last sent something.
    last_seen: u64,
    /// Order of arrival: the smallest is the least recently heard of.
    stamp: u64,
}

pub struct UdpTransport {
    socket: UdpSocket,
    requested: String,
    /// Linked peers: where `send` goes.
    peers: BTreeMap<PeerId, SocketAddr>,
    /// Unknown senders whose datagrams looked like ours. A datagram from an address in neither table is
    /// dropped unless it carries our header, and an address enters `peers` only when the session layer
    /// answers it with [`Transport::send`] (it created a link for it).
    pending: BTreeMap<PeerId, Pending>,
    stamp: u64,
    buf: Vec<u8>,
    refused_unknown: u64,
    send_errors: u64,
}

impl UdpTransport {
    /// Bind a non-blocking socket, e.g. `"0.0.0.0:7777"`; port 0 takes any free port
    /// ([`Transport::local_addr`] reports it).
    pub fn bind(addr: impl ToSocketAddrs) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        let requested = socket.local_addr().map(|a| a.to_string()).unwrap_or_default();
        Ok(Self { socket, requested, peers: BTreeMap::new(), pending: BTreeMap::new(), stamp: 0, buf: vec![0; 2048], refused_unknown: 0, send_errors: 0 })
    }

    /// `bind("0.0.0.0:PORT")`.
    pub fn bind_port(port: u16) -> std::io::Result<Self> {
        Self::bind(("0.0.0.0", port))
    }

    /// The actual socket address.
    pub fn socket_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Register a peer to send to (a client's host address) and get its `PeerId`.
    pub fn connect(&mut self, addr: SocketAddr) -> PeerId {
        let id = peer_id(addr);
        self.pending.remove(&id);
        self.peers.insert(id, addr);
        id
    }

    /// Resolve `"host:port"` and [`UdpTransport::connect`] to the first address.
    pub fn connect_str(&mut self, addr: &str) -> Result<PeerId, String> {
        let resolved = addr
            .to_socket_addrs()
            .map_err(|e| format!("cannot resolve \"{addr}\": {e}; write it as HOST:PORT, e.g. 192.168.1.20:7777"))?
            .next()
            .ok_or_else(|| format!("\"{addr}\" resolves to no address"))?;
        Ok(self.connect(resolved))
    }

    pub fn peer_addr(&self, peer: PeerId) -> Option<SocketAddr> {
        self.peers.get(&peer).copied().or_else(|| self.pending.get(&peer).map(|p| p.addr))
    }

    /// Datagrams dropped from unknown addresses (they did not carry our header, or the table of linked
    /// peers was full), and sends the socket refused.
    pub fn counters(&self) -> (u64, u64) {
        (self.refused_unknown, self.send_errors)
    }

    /// Remember an unknown sender; the least recently heard of makes room when the table is full.
    fn note_pending(&mut self, id: PeerId, addr: SocketAddr, now: u64) {
        self.stamp += 1;
        let stamp = self.stamp;
        if let Some(p) = self.pending.get_mut(&id) {
            p.last_seen = now;
            p.stamp = stamp;
            return;
        }
        if self.pending.len() >= MAX_PENDING
            && let Some(oldest) = self.pending.iter().min_by_key(|(_, p)| p.stamp).map(|(id, _)| *id)
        {
            self.pending.remove(&oldest);
        }
        self.pending.insert(id, Pending { addr, last_seen: now, stamp });
    }

    /// The session layer answers a pending address: it has a link now, so the address is kept.
    fn promote(&mut self, id: PeerId) -> Option<SocketAddr> {
        if self.peers.len() >= MAX_PEERS {
            self.refused_unknown += 1;
            return None;
        }
        let p = self.pending.remove(&id)?;
        self.peers.insert(id, p.addr);
        Some(p.addr)
    }
}

impl Transport for UdpTransport {
    fn send(&mut self, to: PeerId, datagram: &[u8]) {
        debug_assert!(datagram.len() <= MTU, "datagram of {} bytes exceeds the MTU of {MTU}", datagram.len());
        if datagram.len() > MTU {
            return;
        }
        let Some(addr) = self.peers.get(&to).copied().or_else(|| self.promote(to)) else { return };
        // UDP semantics: a full socket buffer or an unreachable peer is a lost datagram.
        if self.socket.send_to(datagram, addr).is_err() {
            self.send_errors += 1;
        }
    }

    fn reply(&mut self, to: PeerId, datagram: &[u8]) {
        if datagram.len() > MTU {
            return;
        }
        // A pending address stays pending: answering a stranger links nothing.
        let Some(addr) = self.peer_addr(to) else { return };
        if self.socket.send_to(datagram, addr).is_err() {
            self.send_errors += 1;
        }
    }

    fn recv(&mut self, now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        self.pending.retain(|_, p| now.saturating_sub(p.last_seen) <= PENDING_TTL_MS);
        for _ in 0..MAX_PER_RECV {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, addr)) => {
                    let id = peer_id(addr);
                    if !self.peers.contains_key(&id) {
                        // A stranger: only a datagram that starts like ours is worth a table entry.
                        if !wire::is_ours(&self.buf[..n]) {
                            self.refused_unknown += 1;
                            continue;
                        }
                        self.note_pending(id, addr, now);
                    }
                    out.push((id, self.buf[..n].to_vec()));
                }
                // Windows reports an ICMP "port unreachable" for an earlier send as a read error.
                Err(e) if matches!(e.kind(), ErrorKind::ConnectionReset | ErrorKind::Interrupted) => continue,
                Err(_) => break,
            }
        }
    }

    fn describe(&self) -> String {
        format!("udp {}", self.requested)
    }

    fn local_addr(&self) -> Option<String> {
        self.socket.local_addr().ok().map(|a| a.to_string())
    }

    fn forget(&mut self, peer: PeerId) {
        self.peers.remove(&peer);
        self.pending.remove(&peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{Link, Role};
    use crate::wire;

    /// A datagram of the link protocol, as a real client would send it.
    fn ours(session: u32) -> Vec<u8> {
        let mut link = Link::new(session, Role::Client);
        link.send_unreliable(b"hello").unwrap();
        let mut out = Vec::new();
        link.flush(0, &mut |d| out.push(d.to_vec()));
        out.remove(0)
    }

    /// Receive until `want` datagrams arrived (loopback delivers at once; the loop only covers a slow kernel).
    fn recv_n(t: &mut UdpTransport, want: usize) -> Vec<(PeerId, Vec<u8>)> {
        let mut got = Vec::new();
        for _ in 0..2000 {
            t.recv(0, &mut got);
            if got.len() >= want {
                break;
            }
            std::thread::yield_now();
        }
        got
    }

    /// `count` sockets with distinct addresses each send `datagram` to `host`; the host drains as it goes,
    /// as a game does every tick.
    fn flood(host: &mut UdpTransport, count: usize, datagram: &[u8]) -> Vec<UdpSocket> {
        let to = SocketAddr::from(([127, 0, 0, 1], host.socket_addr().unwrap().port()));
        let mut sockets = Vec::new();
        let mut sink = Vec::new();
        for i in 0..count {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.send_to(datagram, to).unwrap();
            sockets.push(s);
            if i % 32 == 31 {
                host.recv(0, &mut sink);
            }
        }
        host.recv(0, &mut sink);
        sockets
    }

    #[test]
    fn peer_id_is_stable_and_distinguishes_ports() {
        let a: SocketAddr = "127.0.0.1:7777".parse().unwrap();
        let b: SocketAddr = "127.0.0.1:7778".parse().unwrap();
        assert_eq!(peer_id(a), peer_id(a));
        assert_ne!(peer_id(a), peer_id(b));
        // Pinned: a changed hash would make saved peer tables and logs disagree between builds.
        assert_eq!(peer_id(a), 0x1ac3_40a0_2cce_9a0c);
    }

    #[test]
    fn loopback_datagrams_arrive_with_the_sender_peer_id() {
        let mut a = UdpTransport::bind("127.0.0.1:0").unwrap();
        let mut b = UdpTransport::bind("127.0.0.1:0").unwrap();
        let to_b = a.connect(b.socket_addr().unwrap());
        let ping = ours(1);
        a.send(to_b, &ping);
        // Loopback delivery is immediate on Linux, but give the kernel a moment without sleeping long.
        let got = recv_n(&mut b, 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, ping);
        assert_eq!(got[0].0, peer_id(a.socket_addr().map(|s| SocketAddr::from(([127, 0, 0, 1], s.port()))).unwrap()));
        // The reply goes back through the address learned from the datagram.
        b.send(got[0].0, b"pong");
        let mut back = Vec::new();
        for _ in 0..2000 {
            a.recv(0, &mut back);
            if !back.is_empty() {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(back[0].1, b"pong");
        assert!(a.describe().starts_with("udp 127.0.0.1:"));
    }

    #[test]
    fn connect_str_names_the_fix() {
        let mut t = UdpTransport::bind("127.0.0.1:0").unwrap();
        let err = t.connect_str("no-colon-here").unwrap_err();
        assert!(err.contains("HOST:PORT"), "{err}");
        assert!(t.connect_str("127.0.0.1:9").is_ok());
    }

    #[test]
    fn junk_from_hundreds_of_addresses_cannot_lock_a_real_client_out() {
        let mut host = UdpTransport::bind("127.0.0.1:0").unwrap();
        let junk = flood(&mut host, 600, b"x");
        let host_addr = SocketAddr::from(([127, 0, 0, 1], host.socket_addr().unwrap().port()));
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let hello = ours(0xC0DE);
        client.send_to(&hello, host_addr).unwrap();
        let got = recv_n(&mut host, 1);
        assert_eq!(got.len(), 1, "the client's datagram got through; counters {:?}", host.counters());
        assert_eq!(got[0].1, hello);
        assert_eq!(wire::peek_session(&got[0].1), Some(0xC0DE));
        drop(junk);
    }

    #[test]
    fn a_flood_of_senders_that_know_the_header_cannot_lock_a_real_client_out_either() {
        let mut host = UdpTransport::bind("127.0.0.1:0").unwrap();
        let header_only = ours(5);
        let spam = flood(&mut host, 600, &header_only);
        let host_addr = SocketAddr::from(([127, 0, 0, 1], host.socket_addr().unwrap().port()));
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let hello = ours(0xC0DE);
        client.send_to(&hello, host_addr).unwrap();
        let got = recv_n(&mut host, 1);
        assert_eq!(got.len(), 1, "the client's datagram got through; counters {:?}", host.counters());
        assert_eq!(got[0].1, hello);
        // The host answers the client: that is a link, so the address is kept for good.
        let client_id = got[0].0;
        host.send(client_id, &ours(0xC0DE));
        let mut buf = [0u8; 2048];
        client.set_nonblocking(false).unwrap();
        client.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        assert!(client.recv_from(&mut buf).is_ok(), "the reply reached the client");
        drop(spam);
    }

    #[test]
    fn unknown_senders_wait_in_a_small_lru_table_and_junk_never_enters() {
        let mut host = UdpTransport::bind("127.0.0.1:0").unwrap();
        let _junk = flood(&mut host, 100, b"not ours");
        assert_eq!((host.peers.len(), host.pending.len()), (0, 0));
        assert_eq!(host.counters().0, 100, "every junk datagram was counted and dropped");
        let spam = flood(&mut host, 200, &ours(5));
        assert_eq!(host.peers.len(), 0, "strangers are not linked peers");
        assert_eq!(host.pending.len(), MAX_PENDING);
        // The ones kept are the newest: the last sender is there, the first is not.
        let id = |s: &UdpSocket| peer_id(SocketAddr::from(([127, 0, 0, 1], s.local_addr().unwrap().port())));
        assert!(host.pending.contains_key(&id(spam.last().unwrap())));
        assert!(!host.pending.contains_key(&id(&spam[0])));
    }

    #[test]
    fn answering_a_pending_address_links_it_and_forget_removes_it() {
        let mut host = UdpTransport::bind("127.0.0.1:0").unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = SocketAddr::from(([127, 0, 0, 1], host.socket_addr().unwrap().port()));
        client.send_to(&ours(9), to).unwrap();
        let got = recv_n(&mut host, 1);
        let id = got[0].0;
        assert_eq!((host.peers.len(), host.pending.len()), (0, 1));
        assert_eq!(host.peer_addr(id), Some(client.local_addr().unwrap()));
        // The session layer made a link and replies: the address is a peer now, whoever else floods in.
        host.send(id, &ours(9));
        assert_eq!((host.peers.len(), host.pending.len()), (1, 0));
        let _spam = flood(&mut host, 300, &ours(5));
        assert!(host.peers.contains_key(&id));
        // Garbage from a linked peer still reaches its link (which counts it); strangers' garbage does not.
        client.send_to(b"garbage", to).unwrap();
        let got = recv_n(&mut host, 1);
        assert_eq!((got[0].0, got[0].1.as_slice()), (id, &b"garbage"[..]));
        host.forget(id);
        assert!(host.peer_addr(id).is_none());
        // Sending to an address nobody heard from goes nowhere.
        host.send(12345, &ours(9));
        assert_eq!(host.peers.len(), 0);
    }

    #[test]
    fn a_reply_to_a_stranger_reaches_it_and_links_nothing() {
        let mut host = UdpTransport::bind("127.0.0.1:0").unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = SocketAddr::from(([127, 0, 0, 1], host.socket_addr().unwrap().port()));
        client.send_to(&wire::knock(9), to).unwrap();
        let got = recv_n(&mut host, 1);
        assert_eq!(wire::handshake(&got[0].1), Some(wire::Handshake::Knock { session: 9 }));
        let id = got[0].0;
        host.reply(id, &wire::challenge(9, 77));
        assert_eq!((host.peers.len(), host.pending.len()), (0, 1), "still a stranger");
        let mut buf = [0u8; 2048];
        client.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        let (n, _) = client.recv_from(&mut buf).expect("the challenge reached the client");
        assert_eq!(wire::handshake(&buf[..n]), Some(wire::Handshake::Challenge { session: 9, cookie: 77 }));
        // Nobody heard from, nobody answered; a datagram of an older build is a stranger worth reading.
        host.reply(12345, b"x");
        assert_eq!(host.counters().1, 0);
        client.send_to(&wire::as_version(&ours(4), wire::PROTO_1), to).unwrap();
        assert_eq!(recv_n(&mut host, 1).len(), 1);
    }

    #[test]
    fn pending_addresses_expire_when_silent() {
        let mut host = UdpTransport::bind("127.0.0.1:0").unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let to = SocketAddr::from(([127, 0, 0, 1], host.socket_addr().unwrap().port()));
        client.send_to(&ours(9), to).unwrap();
        let mut got = Vec::new();
        for _ in 0..2000 {
            host.recv(1000, &mut got);
            if !got.is_empty() {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(host.pending.len(), 1);
        host.recv(1000 + PENDING_TTL_MS, &mut got);
        assert_eq!(host.pending.len(), 1, "silent for exactly the TTL is still fine");
        host.recv(1001 + PENDING_TTL_MS, &mut got);
        assert_eq!(host.pending.len(), 0, "forgotten after the TTL");
    }
}
