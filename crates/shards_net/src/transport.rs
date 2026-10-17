//! The transport abstraction: unreliable datagrams between peers.
//!
//! A transport knows nothing about sessions, reliability or fragments; that is the [`link`](crate::link)
//! layer, shared by every transport so the in-memory hub in CI runs the same code as a UDP socket.
//! Steam relay (E10) is one more implementation of this trait over `ISteamNetworkingMessages`.

use crate::conditioner::LinkConditions;

/// Who a datagram goes to or comes from. Hub: the endpoint index. UDP: a hash of the socket address
/// ([`crate::udp::peer_id`]). Steam later: the SteamID.
pub type PeerId = u64;

/// Largest datagram a link ever hands to a transport (bytes). Fits UDP over the 1280-byte IPv6 minimum
/// MTU after the IP and UDP headers, with room to spare.
pub const MTU: usize = 1200;

pub trait Transport: Send + Sync + 'static {
    /// Send one datagram of at most [`MTU`] bytes, unreliably: it may be dropped, delayed, reordered
    /// or duplicated. Never blocks and never fails visibly.
    fn send(&mut self, to: PeerId, datagram: &[u8]);

    /// Answer `to` without keeping anything of it: what a host says to a stranger before the join's
    /// cookie came back (a challenge, a refusal of another build). Over UDP the address stays an
    /// unknown sender instead of becoming a linked peer; a conditioned transport keeps no
    /// conditions for it. By default the same as [`send`](Self::send).
    fn reply(&mut self, to: PeerId, datagram: &[u8]) {
        self.send(to, datagram)
    }

    /// Append every datagram that is due by `now` (milliseconds on the caller's clock) to `out`, in
    /// arrival order. The in-memory hub ignores `now`: its own clock ([`crate::hub::MemoryHub::advance`])
    /// decides what is due, because the members of a stand run on different tick counts.
    fn recv(&mut self, now: u64, out: &mut Vec<(PeerId, Vec<u8>)>);

    /// One line for logs and `net_status`: `"udp 0.0.0.0:7777"`, `"hub host"`.
    fn describe(&self) -> String;

    /// The address peers can reach this transport on, when it has one (`"0.0.0.0:54321"`).
    fn local_addr(&self) -> Option<String> {
        None
    }

    /// Forget everything kept about `peer` (address tables). Called when a link is dropped. A UDP stranger
    /// that was heard from but never answered expires by itself; answering it (`send`) is what makes it a
    /// peer that needs this call.
    fn forget(&mut self, _peer: PeerId) {}

    /// The simulated conditions this transport applies to what it sends, `None` when it cannot simulate.
    fn conditions(&self) -> Option<LinkConditions> {
        None
    }

    /// Set the simulated conditions of this member's links: of the link to `peer`, or of all links.
    /// Returns `false` when this transport cannot simulate (the caller then says so).
    ///
    /// What "the link" covers differs by transport, deliberately: the in-memory hub edits **both
    /// directions** of the member's links (a hub link has one setting per direction, shared with the other
    /// end), while [`Conditioned`](crate::conditioner::Conditioned) over a real socket shapes only what this
    /// process **sends**, since it cannot shape the peer's egress; its ingress is
    /// [`Conditioned::set_ingress`](crate::conditioner::Conditioned::set_ingress). To get the same round
    /// trip on both, call it on both members (`latency=150` on each side is 300 ms either way).
    fn set_conditions(&mut self, _peer: Option<PeerId>, _conditions: LinkConditions) -> bool {
        false
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn send(&mut self, to: PeerId, datagram: &[u8]) {
        (**self).send(to, datagram)
    }
    fn reply(&mut self, to: PeerId, datagram: &[u8]) {
        (**self).reply(to, datagram)
    }
    fn recv(&mut self, now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        (**self).recv(now, out)
    }
    fn describe(&self) -> String {
        (**self).describe()
    }
    fn local_addr(&self) -> Option<String> {
        (**self).local_addr()
    }
    fn forget(&mut self, peer: PeerId) {
        (**self).forget(peer)
    }
    fn conditions(&self) -> Option<LinkConditions> {
        (**self).conditions()
    }
    fn set_conditions(&mut self, peer: Option<PeerId>, conditions: LinkConditions) -> bool {
        (**self).set_conditions(peer, conditions)
    }
}
