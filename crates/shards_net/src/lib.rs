//! Shards networking: transports, the link layer, and the co-op session on top.
//!
//! Lower half, pure Rust (no `App`, no ECS):
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
//! * [`stats`]: [`LinkStats`], [`DirStats`].
//!
//! Every API there that needs time takes `now: u64` in milliseconds from the caller. The hub and the
//! stand use `tick × tick_ms`; UDP uses [`udp::WallClock`].
//!
//! Upper half, the session ([`NetPlugin`]): host-authoritative state, tick-stamped inputs.
//!
//! * [`session`]: the plugin, [`NetSystems`], [`NetEndpoint`], [`NetState`].
//! * [`msg`]: what host and client say to each other.
//! * [`host`] and [`client`]: the two roles. A client builds the world from the host's seed, takes the
//!   host's world (a snapshot), then applies [`msg::StateDelta`]s and sends [`inputs::InputFrame`]s.
//! * [`replicate`]: interest, the delta codec, the desync hash.
//! * [`inputs`]: input frames, the host's queue per player, the client's capture.
//! * [`events`]: typed events a client triggers again ([`NetEventAppExt::net_event`]).
//! * [`hooks`]: what a game decides ([`NetHooks`]: join policy, spawner, characters for co-op saves),
//!   and the recorded `player_join` / `player_leave`.
//! * [`predict`]: a client's own body between the host's states: the inputs it predicts with,
//!   its history, the correction ([`NetCorrection`]).
//! * [`def`]: [`NetDef`], the session's numbers as content.
//! * [`stand`]: [`Stand`], a host and its clients in one process, deterministic.

pub mod client;
pub mod conditioner;
pub mod def;
pub mod events;
pub mod hooks;
pub mod host;
pub mod hub;
pub mod inputs;
pub mod link;
pub mod msg;
pub mod predict;
pub mod replicate;
pub mod session;
pub mod stand;
pub mod stats;
mod tools;
mod track;
pub mod transport;
pub mod udp;
pub mod wire;

pub use client::{clock_ticks, ClientStage, NetClient, NetDesync};
pub use conditioner::{Conditioned, Conditioner, Fate, LinkConditions};
pub use def::{net_def, NetDef, NetDefaults};
pub use events::{NetEventAppExt, NetEvents};
pub use hooks::{JoinDecision, JoinRequest, LeaveRequest, NetHooks, NetLeft, PlayerJoined, PlayerLeft};
pub use host::{end_session, NetHost};
pub use hub::{HubEvent, HubEventKind, HubLinkStats, LinkId, MemoryHub, MemoryTransport};
pub use link::{Delivered, Footprint, Link, LinkConfig, Role, SendError, Verdict};
pub use predict::NetCorrection;
pub use replicate::NetAlways;
pub use session::{NetClock, NetEndpoint, NetGame, NetMarks, NetPlugin, NetState, NetSystems};
pub use stand::Stand;
pub use stats::{DirStats, LinkStats};
pub use transport::{MTU, PeerId, Transport};
pub use udp::{UdpTransport, WallClock};
pub use wire::Reject;

/// `transport` with the conditions of `--net-sim`, when given (else as it is).
pub(crate) fn conditioner_from<T: Transport>(config: &shards_core::EngineConfig, transport: T) -> Box<dyn Transport> {
    match config.net_sim.as_deref().map(str::parse::<LinkConditions>) {
        Some(Ok(conditions)) => Box::new(Conditioned::new(transport, config.seed, 0, conditions)),
        Some(Err(e)) => {
            bevy::log::warn!("net: --net-sim ignored: {e}");
            Box::new(transport)
        }
        None => Box::new(Conditioned::new(transport, config.seed, 0, LinkConditions::NONE)),
    }
}

#[cfg(test)]
mod tests;
