//! Shards networking, lower half: transports and the link layer. Pure Rust, no Bevy `App`, no ECS.
//! The session layer (roles, join, replication, inputs) builds on these pieces.
//!
//! * [`transport`]: the [`Transport`] trait, unreliable datagrams of at most [`MTU`] bytes between
//!   [`PeerId`]s. Steam relay (E10) is one more implementation.
//! * [`hub`]: [`MemoryHub`], the in-memory transport of CI and of the in-process stand. Deterministic:
//!   same seed and same sends give the same deliveries; no wall clock, no sleeps.
//! * [`udp`]: [`UdpTransport`], direct UDP for the LAN, and [`WallClock`].
//! * [`conditioner`]: [`LinkConditions`] (`"latency=150,jitter=10,loss=0.02,dup=0"`) and
//!   [`Conditioned`], the same simulation over any transport (`net_sim`, `--net-sim`).
//! * [`link`]: [`Link`], one per peer pair: unreliable latest-wins messages, reliable ordered messages,
//!   blobs, deflate, acknowledgements, RTT, loss and byte-rate statistics, timeouts.
//! * [`wire`]: the datagram format. [`wire::peek_session`] lets a host create a link for an unknown peer.
//! * [`stats`]: [`LinkStats`], [`DirStats`]. (The estimators behind them, RTT and the receive window, are
//!   internal.)
//!
//! Every API that needs time takes `now: u64` in milliseconds from the caller. The hub and the stand use
//! `tick × tick_ms`; UDP uses [`udp::WallClock`]. Nothing below the UDP transport reads a clock.

pub mod conditioner;
pub mod hub;
pub mod link;
pub mod stats;
mod track;
pub mod transport;
pub mod udp;
pub mod wire;

pub use conditioner::{Conditioned, Conditioner, Fate, LinkConditions};
pub use hub::{HubEvent, HubEventKind, HubLinkStats, LinkId, MemoryHub, MemoryTransport};
pub use link::{Delivered, Footprint, Link, LinkConfig, Role, SendError, Verdict};
pub use stats::{DirStats, LinkStats};
pub use transport::{MTU, PeerId, Transport};
pub use udp::{UdpTransport, WallClock};
pub use wire::Reject;

#[cfg(test)]
mod tests;
