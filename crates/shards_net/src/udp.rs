//! Direct UDP: the LAN transport. `std::net::UdpSocket`, non-blocking, no encryption (LAN and direct
//! play; the link layer adds sessions and reliability, a relay transport can add the rest later).
//!
//! The only code in the crate that touches the wall clock is [`WallClock`], which hands out
//! milliseconds since it was created; callers pass them as `now` like any other clock.

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::Instant;

use crate::transport::{MTU, PeerId, Transport};

/// Peers kept in the address table; datagrams from a new address beyond this are ignored until a
/// link calls [`Transport::forget`]. Bounds memory against spoofed senders.
const MAX_PEERS: usize = 512;
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

pub struct UdpTransport {
    socket: UdpSocket,
    requested: String,
    peers: BTreeMap<PeerId, SocketAddr>,
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
        Ok(Self { socket, requested, peers: BTreeMap::new(), buf: vec![0; 2048], refused_unknown: 0, send_errors: 0 })
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
        self.peers.get(&peer).copied()
    }

    /// Datagrams ignored because the address table was full, and sends the socket refused.
    pub fn counters(&self) -> (u64, u64) {
        (self.refused_unknown, self.send_errors)
    }
}

impl Transport for UdpTransport {
    fn send(&mut self, to: PeerId, datagram: &[u8]) {
        debug_assert!(datagram.len() <= MTU, "datagram of {} bytes exceeds the MTU of {MTU}", datagram.len());
        let Some(addr) = self.peers.get(&to) else { return };
        if datagram.len() > MTU {
            return;
        }
        // UDP semantics: a full socket buffer or an unreachable peer is a lost datagram.
        if self.socket.send_to(datagram, addr).is_err() {
            self.send_errors += 1;
        }
    }

    fn recv(&mut self, _now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        for _ in 0..MAX_PER_RECV {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, addr)) => {
                    let id = peer_id(addr);
                    if !self.peers.contains_key(&id) {
                        if self.peers.len() >= MAX_PEERS {
                            self.refused_unknown += 1;
                            continue;
                        }
                        self.peers.insert(id, addr);
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        a.send(to_b, b"ping");
        let mut got = Vec::new();
        // Loopback delivery is immediate on Linux, but give the kernel a moment without sleeping long.
        for _ in 0..2000 {
            b.recv(0, &mut got);
            if !got.is_empty() {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, b"ping");
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
}
