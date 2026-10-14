//! The numbers the network layer reports: [`LinkStats`] (one [`Link`](crate::link::Link)) and
//! [`DirStats`] (one direction of a simulated link). The estimators behind them are in `track`.

use serde::{Deserialize, Serialize};

/// Counters of one direction of a simulated link (hub link direction or `Conditioned` side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirStats {
    /// Packets handed to the simulation.
    pub sent: u64,
    /// Packets the simulation dropped.
    pub dropped: u64,
    /// Extra copies the simulation created.
    pub duplicated: u64,
    /// Packets (copies included) that left the simulation.
    pub delivered: u64,
    pub bytes_sent: u64,
    pub bytes_delivered: u64,
}

impl DirStats {
    /// Dropped / sent, 0 when nothing was sent.
    pub fn loss(&self) -> f32 {
        if self.sent == 0 { 0.0 } else { self.dropped as f32 / self.sent as f32 }
    }
}

/// What a link knows about its health; `net_status` and the overlay read this.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkStats {
    /// Smoothed round-trip time, ms (the peer's hold time between receiving and answering is excluded).
    pub srtt_ms: f32,
    pub rttvar_ms: f32,
    /// Current retransmission timeout, ms.
    pub rto_ms: u32,
    /// Reliable messages and blob fragments sent again at once because acknowledgements passed their
    /// datagram by (instead of after a timeout); part of `resends`.
    pub fast_retransmits: u64,
    /// Datagrams lost on the way in, as seq gaps over the last 256 datagrams (the newest few are not
    /// counted: they may still arrive reordered). 0..=1.
    pub loss: f32,
    /// The same over the whole life of the link.
    pub loss_lifetime: f32,
    /// Bytes per second over the last second, headers included.
    pub bytes_in_per_s: u32,
    pub bytes_out_per_s: u32,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub datagrams_in: u64,
    pub datagrams_out: u64,
    /// Reliable messages and blob fragments sent again.
    pub resends: u64,
    /// Reliable messages sent or queued and not yet acknowledged (blobs included).
    pub reliable_in_flight: u32,
    /// Blobs among them.
    pub blobs_in_flight: u32,
    /// Bytes held by the reliable queue, counted as the peer will hold them: before deflate.
    pub reliable_queued_bytes: u64,
    pub reliable_delivered: u64,
    pub unreliable_delivered: u64,
    /// Unreliable messages that arrived out of date (a newer one was delivered already).
    pub unreliable_stale: u64,
    /// Unreliable messages dropped by the sender's queue cap.
    pub unreliable_dropped: u64,
    /// Datagrams dropped as already seen.
    pub duplicates: u64,
    /// Datagrams dropped as older than the 256-datagram window.
    pub stale_datagrams: u64,
    /// Datagrams dropped for a foreign magic, protocol or session, and undecodable ones.
    pub rejected_magic: u64,
    pub rejected_proto: u64,
    pub rejected_session: u64,
    pub malformed: u64,
    /// Reliable messages and blob pieces the receive buffers had no room for (or that were numbered beyond
    /// the reorder window). They were refused unacknowledged, so the sender sends them again: a nonzero
    /// count is a receiver that fell behind, or a peer that does not keep to the caps.
    pub reliable_overflow: u64,
    /// Valid datagrams refused unacknowledged because their content did not fit (see
    /// [`Verdict::Refused`](crate::link::Verdict::Refused)).
    pub refused_datagrams: u64,
    /// ms since the last valid datagram arrived (0 before the first).
    pub silence_ms: u64,
}
