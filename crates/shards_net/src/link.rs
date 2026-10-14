//! The link layer: one [`Link`] per peer pair, over any [`Transport`](crate::transport::Transport).
//!
//! A link turns unreliable datagrams into two channels and keeps the numbers a session needs:
//!
//! * **unreliable, latest wins** ([`Link::send_unreliable`]): never resent, never delivered twice,
//!   never delivered after a newer message. A message too big for one datagram is fragmented; a lost
//!   fragment loses that message.
//! * **reliable, ordered, exactly once** ([`Link::send_reliable`]): sequence numbers, acknowledgements
//!   (piggybacked on every datagram) and resends after `rto = max(100 ms, srtt + 4·rttvar)` with
//!   exponential backoff. Messages above 900 bytes become **blobs**: fragments with a 32-fragment
//!   window, acknowledged selectively through the datagram acks.
//!
//! Large messages are deflated once, before they are fragmented, when that makes them smaller (a
//! 2 KB JSON state delta usually fits one datagram afterwards and then travels inline). Bodies of
//! inline traffic are deflated per datagram above 256 bytes (see [`crate::wire`]).
//!
//! The link has no clock of its own: every call that needs time takes `now` in milliseconds from the
//! caller (hub and stand: `tick × tick_ms`; UDP: [`WallClock`](crate::udp::WallClock)). Memory is bounded:
//! the reliable queue refuses (an error, not growth) past [`LinkConfig::max_reliable_messages`] or
//! [`LinkConfig::max_reliable_bytes`], messages above [`LinkConfig::max_blob_bytes`] (16 MB) are
//! refused, the unreliable queue drops its oldest, and receive buffers have fixed windows. A receiver
//! never acknowledges what it did not keep: reliable content that does not fit is **refused**
//! ([`Verdict::Refused`], the datagram stays unacknowledged) and the sender sends it again. The message
//! the receiver waits for next is always taken, so a full buffer drains.
//!
//! Nothing a peer sends can make the link inflate without bound: one datagram inflates to at most
//! [`wire::INFLATE_BUDGET`](crate::wire) bytes in all (its body and its inline messages share the
//! budget), an inline reliable message to at most [`INLINE_RELIABLE_INFLATED_MAX`] (more travels as a
//! blob), and once [`LinkConfig::max_undelivered_bytes`] of delivered messages wait for [`Link::poll`] the
//! link refuses payload until the host polls.
//!
//! Sessions are not the link's business beyond one number: a datagram with another session id is
//! dropped and counted. A host that sees a datagram from an unknown peer reads the session the client
//! chose with [`wire::peek_session`] and creates the link with it.
//!
//! Typical use, once per tick:
//!
//! ```ignore
//! transport.recv(now, &mut incoming);
//! for (peer, datagram) in incoming.drain(..) { link.on_datagram(now, &datagram); }
//! for message in link.poll(now) { /* Delivered::Reliable / Unreliable */ }
//! link.send_unreliable(&state)?;
//! link.flush(now, &mut |datagram| transport.send(peer, datagram));
//! ```

use std::collections::{BTreeMap, VecDeque};

use flate2::Compression;

use crate::stats::LinkStats;
use crate::track::{Classify, RateMeter, RecvWindow, Rtt, seq_gt, seq_lt};
use crate::wire::{self, Body, Budget, Fragment, Header, MAX_BODY, Msg, Reject};

/// Reliable messages up to this size travel inline; larger ones become blobs.
pub const INLINE_RELIABLE_MAX: usize = 900;
/// Unreliable messages up to this size travel inline in one datagram; larger ones are fragmented.
pub const INLINE_UNRELIABLE_MAX: usize = 1100;
/// An inline reliable message inflates to at most this much; a message that is bigger travels as a blob
/// however well it deflates. (900 bytes of deflate expand to 0.93 MB at the very most, so the limit only
/// ever refuses a lie.)
pub const INLINE_RELIABLE_INFLATED_MAX: usize = 1 << 20;
/// Payload bytes of one fragment.
pub const FRAG_SIZE: usize = 1024;
/// Hard ceiling of [`LinkConfig::max_reliable_messages`]: half the receiver's reorder window.
pub const MAX_RELIABLE_MESSAGES: usize = 1024;

/// Fixed part of a body: three u32, two counts (a few spare bytes for two-byte counts).
const BODY_FIXED: usize = 16;
/// Fragment header: msg u32, index u16, count u16, length varint (two bytes for up to 1024).
const FRAG_OVERHEAD: usize = 4 + 2 + 2 + 2;
const MAX_SENT_RECORDS: usize = 512;
/// How far ahead of the next expected reliable sequence number a message is buffered.
const RELIABLE_WINDOW: u32 = 2 * MAX_RELIABLE_MESSAGES as u32;
const MAX_PARTIAL_BLOBS: usize = 4;
const MAX_UNRELIABLE_ASSEMBLIES: usize = 8;
/// Fragments of one blob / one unreliable message the receiver accepts (16 MB / 1 MB plus slack).
const MAX_BLOB_PIECES: usize = 17_000;
const MAX_UNRELIABLE_PIECES: usize = 1_100;
/// A datagram this much older than the newest acknowledged one cannot be named by `ack_bits`.
const ACK_WINDOW: i32 = 32;
/// A datagram is lost once one this many numbers newer has been acknowledged.
const LOSS_THRESHOLD: i32 = 3;
/// Round-trip samples above this are ignored (a stale echo).
const MAX_RTT_SAMPLE_MS: u32 = 120_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Host,
    Client,
}

/// A message handed up by [`Link::poll`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivered {
    Unreliable(Vec<u8>),
    Reliable(Vec<u8>),
}

/// What [`Link::on_datagram`] did with a datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    /// Seen before: dropped.
    Duplicate,
    /// Older than the 256-datagram window: dropped.
    Stale,
    /// Foreign magic, protocol or session, or undecodable: dropped and counted.
    Rejected(Reject),
    /// A valid datagram whose content did not fit the receive buffers (reliable bytes over
    /// [`LinkConfig::max_reliable_bytes`], too many partial blobs, ...): **not acknowledged**, so the
    /// peer sends what it carried again. Nothing in it was delivered except reliable messages that fit.
    Refused,
}

/// Why a message was not queued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendError {
    TooLarge { len: usize, max: usize },
    QueueFull { queued_messages: usize, queued_bytes: usize },
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SendError::TooLarge { len, max } => {
                write!(f, "message of {len} bytes is over the {max}-byte limit; split it or send it as a file the peer can fetch")
            }
            SendError::QueueFull { queued_messages, queued_bytes } => write!(
                f,
                "reliable queue is full ({queued_messages} messages, {queued_bytes} bytes): the peer is not acknowledging; drop the peer or wait for it"
            ),
        }
    }
}

impl std::error::Error for SendError {}

/// Tunables. The defaults are the contract; tests lower the caps.
#[derive(Clone, Debug)]
pub struct LinkConfig {
    pub rto_min_ms: u64,
    pub rto_max_ms: u64,
    /// Timeout before the first round-trip sample.
    pub rto_initial_ms: u64,
    /// Added to every timeout: the peer answers at its next flush, up to a tick later.
    pub rto_slack_ms: u64,
    /// Silence after which an empty datagram is sent.
    pub keepalive_ms: u64,
    /// Fragments of a blob in flight at once.
    pub blob_window: usize,
    /// Unacknowledged reliable messages (hard ceiling [`MAX_RELIABLE_MESSAGES`]).
    pub max_reliable_messages: usize,
    /// Reliable bytes: queued and unacknowledged on the sending side (counted before deflate, as the peer
    /// will hold them), buffered ahead of the next message in order on the receiving side. Keep it equal on
    /// both ends; a receiver with less refuses what does not fit (unacknowledged, the sender tries again).
    pub max_reliable_bytes: usize,
    /// Largest reliable message (a blob), bytes before deflate.
    pub max_blob_bytes: usize,
    pub max_unreliable_queue: usize,
    pub max_unreliable_bytes: usize,
    /// Bytes of delivered messages that may wait for [`Link::poll`]. Once that many are waiting, the link
    /// refuses further payload ([`Verdict::Refused`], unacknowledged) until the host polls: a peer cannot
    /// make a host that is slow to poll allocate without bound. One poll per tick never gets near it.
    pub max_undelivered_bytes: usize,
    /// Datagrams one `flush` may build.
    pub max_datagrams_per_flush: usize,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            rto_min_ms: 100,
            rto_max_ms: 3000,
            rto_initial_ms: 500,
            rto_slack_ms: 40,
            keepalive_ms: 250,
            blob_window: 32,
            max_reliable_messages: MAX_RELIABLE_MESSAGES,
            max_reliable_bytes: 32 << 20,
            max_blob_bytes: 16 << 20,
            max_unreliable_queue: 256,
            max_unreliable_bytes: 1 << 20,
            max_undelivered_bytes: 24 << 20,
            max_datagrams_per_flush: 32,
        }
    }
}

/// How much a link holds right now; the soak tests watch it stay bounded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    pub sent_records: usize,
    pub reliable_out: usize,
    pub reliable_out_bytes: usize,
    pub reliable_in_buffered: usize,
    pub reliable_in_bytes: usize,
    pub partial_blobs: usize,
    pub unreliable_out: usize,
    pub unreliable_assemblies: usize,
    pub undelivered: usize,
    /// Bytes of those messages.
    pub undelivered_bytes: usize,
}

// ---- outgoing reliable state -------------------------------------------------------------------

/// `rto << n`, capped.
fn backoff(rto: u64, n: u32, max: u64) -> u64 {
    (rto << n.min(5)).min(max)
}

struct BlobOut {
    data: Vec<u8>,
    deflated: bool,
    count: usize,
    acked: Vec<bool>,
    acked_count: usize,
    next_new: usize,
    /// Index of the latest transmission of each piece (0 = the first).
    tx: Vec<u32>,
    /// Pieces sent and neither acknowledged nor known lost, with the time of their latest send.
    inflight: Vec<(usize, u64)>,
    /// Pieces known lost (acks passed them), to be sent again at once.
    resend: VecDeque<usize>,
}

impl BlobOut {
    fn new(data: Vec<u8>, deflated: bool) -> Self {
        let count = data.len().div_ceil(FRAG_SIZE);
        Self {
            data,
            deflated,
            count,
            acked: vec![false; count],
            acked_count: 0,
            next_new: 0,
            tx: vec![0; count],
            inflight: Vec::new(),
            resend: VecDeque::new(),
        }
    }

    /// The next piece to put on the wire: lost ones first, then timeouts, then new ones while the
    /// window has room. Returns the piece and its transmission index.
    fn pick(&mut self, now: u64, rto: u64, rto_max: u64, window: usize) -> Option<(usize, u32)> {
        while let Some(i) = self.resend.pop_front() {
            if self.acked[i] {
                continue;
            }
            self.tx[i] += 1;
            self.inflight.push((i, now));
            return Some((i, self.tx[i]));
        }
        for entry in self.inflight.iter_mut() {
            let (i, sent_at) = *entry;
            if now >= sent_at + backoff(rto, self.tx[i], rto_max) {
                self.tx[i] += 1;
                entry.1 = now;
                return Some((i, self.tx[i]));
            }
        }
        if self.inflight.len() < window && self.next_new < self.count {
            let i = self.next_new;
            self.next_new += 1;
            self.inflight.push((i, now));
            return Some((i, 0));
        }
        None
    }

    /// Returns true when every piece is acknowledged.
    fn ack(&mut self, idx: usize) -> bool {
        if idx < self.count && !self.acked[idx] {
            self.acked[idx] = true;
            self.acked_count += 1;
            self.inflight.retain(|&(i, _)| i != idx);
        }
        self.acked_count == self.count
    }

    /// Returns true when the piece was queued for an immediate resend.
    fn lose(&mut self, idx: usize, tx: u32) -> bool {
        if idx < self.count
            && !self.acked[idx]
            && self.tx[idx] == tx
            && let Some(p) = self.inflight.iter().position(|&(i, _)| i == idx)
        {
            self.inflight.remove(p);
            self.resend.push_back(idx);
            return true;
        }
        false
    }
}

enum RelBody {
    Inline { data: Vec<u8>, deflated: bool },
    Blob(Box<BlobOut>),
    /// Acknowledged: the data is gone, the slot stays until the messages before it are acknowledged too.
    Done,
}

struct RelOut {
    rseq: u32,
    body: RelBody,
    acked: bool,
    sent_at: Option<u64>,
    /// Index of the latest transmission (0 = the first).
    tx: u32,
    /// Known lost: send again at the next flush.
    resend_now: bool,
    bytes: usize,
}

/// A blob piece on the wire: the blob's rseq, the piece, and its transmission index.
type PieceRef = (u32, u16, u32);
/// What a datagram carried that is worth resending: inline messages (rseq, transmission) and a blob piece.
type Carried = (Vec<(u32, u32)>, Option<PieceRef>);

struct SentRecord {
    seq: u32,
    sent_at: u64,
    /// Reliable inline messages this datagram carried, with their transmission index.
    rels: Vec<(u32, u32)>,
    /// The blob piece it carried.
    frag: Option<PieceRef>,
    acked: bool,
    resolved: bool,
}

struct UnFrag {
    msg: u32,
    count: usize,
    next: usize,
}

struct UnOut {
    data: Vec<u8>,
    deflated: bool,
    frag: Option<UnFrag>,
}

// ---- incoming state ----------------------------------------------------------------------------

struct BlobIn {
    count: usize,
    deflated: bool,
    parts: BTreeMap<u16, Vec<u8>>,
    bytes: usize,
    /// Size of the whole blob once inflated, learnt when the last piece first completed it and the
    /// buffer had no room: the piece is refused again without inflating the blob a second time.
    inflated: Option<usize>,
}

/// What receiving reliable content came to, worst last. A datagram is acknowledged only when everything
/// reliable in it was `Kept`: what was refused must come again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Take {
    /// Taken, or already had: safe to acknowledge.
    Kept,
    /// No room right now: not acknowledged, the sender sends it again.
    Refused,
    /// Cannot be used at all (does not inflate, impossible numbers).
    Malformed,
}

/// What [`Link::wants_reliable`] found out about a reliable sequence number.
enum Want {
    Yes,
    /// Delivered or buffered already.
    Have,
    /// Outside the reorder window.
    Refuse,
}

struct UnAsm {
    count: u16,
    deflated: bool,
    parts: BTreeMap<u16, Vec<u8>>,
    /// Newest datagram any piece came in; the message is as new as that.
    max_seq: u32,
}

#[derive(Default)]
struct Counters {
    datagrams_in: u64,
    datagrams_out: u64,
    bytes_in: u64,
    bytes_out: u64,
    resends: u64,
    fast_retransmits: u64,
    duplicates: u64,
    stale: u64,
    rejected_magic: u64,
    rejected_proto: u64,
    rejected_session: u64,
    malformed: u64,
    reliable_overflow: u64,
    refused: u64,
    reliable_delivered: u64,
    unreliable_delivered: u64,
    unreliable_stale: u64,
    unreliable_dropped: u64,
}

/// A datagram being assembled.
struct Build {
    body: Body,
    used: usize,
    rels: Vec<(u32, u32)>,
    frag: Option<PieceRef>,
}

impl Build {
    fn new() -> Self {
        Self { body: Body::default(), used: BODY_FIXED, rels: Vec::new(), frag: None }
    }
}

pub struct Link {
    session: u32,
    role: Role,
    cfg: LinkConfig,
    // clock
    now: u64,
    started: Option<u64>,
    last_recv: Option<u64>,
    last_send: Option<u64>,
    // datagram level
    next_seq: u32,
    sent: VecDeque<SentRecord>,
    hi_acked: Option<u32>,
    window: RecvWindow,
    ack_pending: bool,
    rtt: Rtt,
    /// The newest `time_ms` the peer sent, and when it arrived (to echo it with the hold time).
    peer_time: Option<(u32, u64)>,
    // reliable out
    rel_next: u32,
    rel_out: VecDeque<RelOut>,
    rel_out_bytes: usize,
    blobs_queued: usize,
    // reliable in
    rin_next: u32,
    rin_buf: BTreeMap<u32, Vec<u8>>,
    rin_bytes: usize,
    rin_blobs: BTreeMap<u32, BlobIn>,
    // unreliable
    un_out: VecDeque<UnOut>,
    un_msg: u32,
    un_last_key: Option<u32>,
    un_asm: BTreeMap<u32, UnAsm>,
    delivered: Vec<Delivered>,
    /// Bytes in `delivered`.
    delivered_bytes: usize,
    // statistics
    c: Counters,
    meter: RateMeter,
}

/// Deflate once; keep the result only when it is smaller. Reliable messages (blobs: snapshots, regions)
/// get the default level, latency-sensitive unreliable ones the fastest.
fn compress_message(bytes: &[u8], level: Compression) -> (Vec<u8>, bool) {
    match wire::deflate(bytes, level) {
        Some(z) if z.len() < bytes.len() => (z, true),
        _ => (bytes.to_vec(), false),
    }
}

/// Inflate a message that was reassembled from fragments: only its own cap applies.
fn open_message(data: Vec<u8>, deflated: bool, max: usize) -> Option<Vec<u8>> {
    if deflated { wire::inflate(&data, max) } else { Some(data) }
}

/// Inflate a message that arrived inline in a datagram: its cap, and what is left of the datagram's budget.
fn open_inline(data: Vec<u8>, deflated: bool, max: usize, budget: &mut Budget) -> Option<Vec<u8>> {
    if deflated { budget.inflate(&data, max) } else { Some(data) }
}

impl Link {
    pub fn new(session: u32, role: Role) -> Self {
        Self::with_config(session, role, LinkConfig::default())
    }

    pub fn with_config(session: u32, role: Role, mut cfg: LinkConfig) -> Self {
        cfg.max_reliable_messages = cfg.max_reliable_messages.clamp(1, MAX_RELIABLE_MESSAGES);
        cfg.blob_window = cfg.blob_window.clamp(1, 256);
        cfg.max_datagrams_per_flush = cfg.max_datagrams_per_flush.max(1);
        cfg.max_unreliable_queue = cfg.max_unreliable_queue.max(1);
        cfg.max_undelivered_bytes = cfg.max_undelivered_bytes.max(1);
        Self {
            session,
            role,
            cfg,
            now: 0,
            started: None,
            last_recv: None,
            last_send: None,
            next_seq: 1,
            sent: VecDeque::new(),
            hi_acked: None,
            window: RecvWindow::default(),
            ack_pending: false,
            rtt: Rtt::default(),
            peer_time: None,
            rel_next: 0,
            rel_out: VecDeque::new(),
            rel_out_bytes: 0,
            blobs_queued: 0,
            rin_next: 0,
            rin_buf: BTreeMap::new(),
            rin_bytes: 0,
            rin_blobs: BTreeMap::new(),
            un_out: VecDeque::new(),
            un_msg: 0,
            un_last_key: None,
            un_asm: BTreeMap::new(),
            delivered: Vec::new(),
            delivered_bytes: 0,
            c: Counters::default(),
            meter: RateMeter::default(),
        }
    }

    pub fn session(&self) -> u32 {
        self.session
    }

    /// Start numbering datagrams at `seq` (tests cross the 32-bit wrap without sending four billion).
    #[cfg(test)]
    pub(crate) fn set_next_seq_for_test(&mut self, seq: u32) {
        self.next_seq = seq;
    }

    pub fn role(&self) -> Role {
        self.role
    }

    pub fn config(&self) -> &LinkConfig {
        &self.cfg
    }

    fn touch(&mut self, now: u64) {
        self.now = self.now.max(now);
        if self.started.is_none() {
            self.started = Some(self.now);
        }
    }

    fn rto(&self) -> u64 {
        self.rtt.rto(self.cfg.rto_min_ms, self.cfg.rto_max_ms, self.cfg.rto_initial_ms, self.cfg.rto_slack_ms)
    }

    // ---- sending ---------------------------------------------------------------------------------

    /// Queue a message on the latest-wins channel. It is sent at the next `flush`, never resent, and the
    /// peer drops it if a newer one arrived first. A message that does not fit a datagram after deflate
    /// is fragmented, and a lost fragment loses the message. When more than
    /// [`LinkConfig::max_unreliable_queue`] messages wait, the oldest are dropped.
    pub fn send_unreliable(&mut self, bytes: &[u8]) -> Result<(), SendError> {
        if bytes.len() > self.cfg.max_unreliable_bytes {
            return Err(SendError::TooLarge { len: bytes.len(), max: self.cfg.max_unreliable_bytes });
        }
        let (data, deflated) = if bytes.len() > INLINE_UNRELIABLE_MAX { compress_message(bytes, Compression::fast()) } else { (bytes.to_vec(), false) };
        let frag = (data.len() > INLINE_UNRELIABLE_MAX).then(|| {
            let msg = self.un_msg;
            self.un_msg = self.un_msg.wrapping_add(1);
            UnFrag { msg, count: data.len().div_ceil(FRAG_SIZE), next: 0 }
        });
        self.un_out.push_back(UnOut { data, deflated, frag });
        while self.un_out.len() > self.cfg.max_unreliable_queue {
            // The one being fragmented right now is on the wire already; drop the oldest behind it.
            let Some(at) = self.un_out.iter().position(|m| m.frag.as_ref().is_none_or(|f| f.next == 0)) else { break };
            self.un_out.remove(at);
            self.c.unreliable_dropped += 1;
        }
        Ok(())
    }

    /// Queue a message on the reliable channel: delivered to the peer exactly once and in order, however
    /// many datagrams are lost. Above 900 bytes (after deflate) it becomes a blob. Fails, without
    /// growing, when the queue is full or the message is above [`LinkConfig::max_blob_bytes`].
    pub fn send_reliable(&mut self, bytes: &[u8]) -> Result<(), SendError> {
        if bytes.len() > self.cfg.max_blob_bytes {
            return Err(SendError::TooLarge { len: bytes.len(), max: self.cfg.max_blob_bytes });
        }
        if self.rel_out.len() >= self.cfg.max_reliable_messages || self.rel_out_bytes + bytes.len() > self.cfg.max_reliable_bytes {
            return Err(SendError::QueueFull { queued_messages: self.rel_out.len(), queued_bytes: self.rel_out_bytes });
        }
        let (data, deflated) = if bytes.len() > INLINE_RELIABLE_MAX { compress_message(bytes, Compression::default()) } else { (bytes.to_vec(), false) };
        let size = data.len();
        let rseq = self.rel_next;
        self.rel_next = self.rel_next.wrapping_add(1);
        let body = if size <= INLINE_RELIABLE_MAX && bytes.len() <= INLINE_RELIABLE_INFLATED_MAX {
            RelBody::Inline { data, deflated }
        } else {
            self.blobs_queued += 1;
            RelBody::Blob(Box::new(BlobOut::new(data, deflated)))
        };
        // Counted as the receiver will hold it, inflated: a few hundred bytes of deflate can stand for
        // megabytes in its buffer, and the cap exists to keep that buffer from overflowing.
        self.rel_out_bytes += bytes.len();
        self.rel_out.push_back(RelOut { rseq, body, acked: false, sent_at: None, tx: 0, resend_now: false, bytes: bytes.len() });
        Ok(())
    }

    /// Build the datagrams due at `now` and hand each to `out`: queued messages, resends of what is
    /// unacknowledged past its timeout, acknowledgements, and a keepalive after
    /// [`LinkConfig::keepalive_ms`] of silence. At most [`LinkConfig::max_datagrams_per_flush`]; the
    /// rest waits for the next flush.
    pub fn flush(&mut self, now: u64, out: &mut dyn FnMut(&[u8])) {
        self.touch(now);
        let now = self.now;
        let mut built = 0;
        while built < self.cfg.max_datagrams_per_flush {
            let mut b = Build::new();
            self.fill_reliable(now, &mut b);
            self.fill_blob_piece(now, &mut b);
            self.fill_unreliable(&mut b);
            if !b.body.has_payload() && !(built == 0 && (self.ack_pending || self.keepalive_due(now))) {
                break;
            }
            self.emit(now, b, out);
            built += 1;
        }
    }

    fn keepalive_due(&self, now: u64) -> bool {
        if self.role == Role::Host && self.last_recv.is_none() {
            // A host does not talk to a peer that never spoke.
            return false;
        }
        self.last_send.is_none_or(|t| now.saturating_sub(t) >= self.cfg.keepalive_ms)
    }

    fn fill_reliable(&mut self, now: u64, b: &mut Build) {
        let rto = self.rto();
        let rto_max = self.cfg.rto_max_ms;
        let mut resent = 0;
        for item in self.rel_out.iter_mut() {
            if item.acked {
                continue;
            }
            let RelBody::Inline { data, deflated } = &item.body else { continue };
            let due = match item.sent_at {
                None => true,
                Some(t) => item.resend_now || now >= t + backoff(rto, item.tx, rto_max),
            };
            if !due {
                continue;
            }
            let cost = 4 + wire::msg_cost(data.len());
            if b.used + cost > MAX_BODY {
                continue;
            }
            if item.sent_at.is_some() {
                item.tx += 1;
                resent += 1;
            }
            b.rels.push((item.rseq, item.tx));
            b.body.reliable.push((item.rseq, Msg { data: data.clone(), deflated: *deflated }));
            b.used += cost;
            item.sent_at = Some(now);
            item.resend_now = false;
        }
        self.c.resends += resent;
    }

    fn fill_blob_piece(&mut self, now: u64, b: &mut Build) {
        if self.blobs_queued == 0 || b.body.frag.is_some() || b.used + FRAG_OVERHEAD + FRAG_SIZE > MAX_BODY {
            return;
        }
        let rto = self.rto();
        let (rto_max, window) = (self.cfg.rto_max_ms, self.cfg.blob_window);
        for item in self.rel_out.iter_mut() {
            if item.acked {
                continue;
            }
            let RelBody::Blob(blob) = &mut item.body else { continue };
            // Only the first unacknowledged blob is on the wire; the rest wait their turn.
            let Some((idx, tx)) = blob.pick(now, rto, rto_max, window) else { return };
            if tx > 0 {
                self.c.resends += 1;
            }
            let start = idx * FRAG_SIZE;
            let end = blob.data.len().min(start + FRAG_SIZE);
            b.body.frag = Some(Fragment {
                reliable: true,
                deflated: blob.deflated,
                msg: item.rseq,
                index: idx as u16,
                count: blob.count as u16,
                bytes: blob.data[start..end].to_vec(),
            });
            b.used += FRAG_OVERHEAD + (end - start);
            b.frag = Some((item.rseq, idx as u16, tx));
            return;
        }
    }

    /// Unreliable messages go in send order; one that needs fragments takes the fragment slot, and
    /// nothing newer rides in a datagram before its last fragment, so the receiver never sees a newer
    /// message complete before an older one.
    fn fill_unreliable(&mut self, b: &mut Build) {
        while let Some(front) = self.un_out.front_mut() {
            if front.frag.is_none() {
                let cost = wire::msg_cost(front.data.len());
                if b.used + cost > MAX_BODY {
                    break;
                }
                let Some(m) = self.un_out.pop_front() else { break };
                b.used += cost;
                b.body.unreliable.push(Msg { data: m.data, deflated: m.deflated });
                continue;
            }
            if b.body.frag.is_some() {
                break;
            }
            let deflated = front.deflated;
            let Some(f) = front.frag.as_mut() else { break };
            let start = f.next * FRAG_SIZE;
            let end = front.data.len().min(start + FRAG_SIZE);
            let cost = FRAG_OVERHEAD + (end - start);
            if b.used + cost > MAX_BODY {
                break;
            }
            b.body.frag = Some(Fragment {
                reliable: false,
                deflated,
                msg: f.msg,
                index: f.next as u16,
                count: f.count as u16,
                bytes: front.data[start..end].to_vec(),
            });
            b.used += cost;
            f.next += 1;
            if f.next < f.count {
                break;
            }
            self.un_out.pop_front();
        }
    }

    fn emit(&mut self, now: u64, b: Build, out: &mut dyn FnMut(&[u8])) {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        let (ack, ack_bits) = self.window.ack();
        let mut body = b.body;
        body.time_ms = now as u32;
        body.time_echo = self.peer_time.map(|(t, at)| t.wrapping_add(now.saturating_sub(at) as u32));
        body.reliable_ack = self.rin_next;
        let header = Header { flags: 0, session: self.session, seq, ack, ack_bits };
        let bytes = wire::encode(&header, &body);
        debug_assert!(bytes.len() <= crate::transport::MTU, "built a datagram of {} bytes", bytes.len());
        self.sent.push_back(SentRecord { seq, sent_at: now, rels: b.rels, frag: b.frag, acked: false, resolved: false });
        if self.sent.len() > MAX_SENT_RECORDS {
            self.sent.pop_front();
        }
        self.last_send = Some(now);
        self.ack_pending = false;
        self.c.datagrams_out += 1;
        self.c.bytes_out += bytes.len() as u64;
        self.meter.add_out(now, bytes.len());
        out(&bytes);
    }

    // ---- receiving -------------------------------------------------------------------------------

    /// Feed one datagram from the transport. Foreign or broken datagrams are counted and dropped.
    pub fn on_datagram(&mut self, now: u64, datagram: &[u8]) -> Verdict {
        self.touch(now);
        let now = self.now;
        self.c.datagrams_in += 1;
        self.c.bytes_in += datagram.len() as u64;
        self.meter.add_in(now, datagram.len());
        let header = match wire::peek(datagram) {
            Ok(h) => h,
            Err(r) => {
                match r {
                    Reject::Proto(_) => self.c.rejected_proto += 1,
                    Reject::Malformed => self.c.malformed += 1,
                    _ => self.c.rejected_magic += 1,
                }
                return Verdict::Rejected(r);
            }
        };
        if header.session != self.session {
            self.c.rejected_session += 1;
            return Verdict::Rejected(Reject::Session(header.session));
        }
        match self.window.classify(header.seq) {
            Classify::Duplicate => {
                self.c.duplicates += 1;
                self.last_recv = Some(now);
                return Verdict::Duplicate;
            }
            Classify::Stale => {
                self.c.stale += 1;
                return Verdict::Stale;
            }
            Classify::New => {}
        }
        if self.delivered_bytes >= self.cfg.max_undelivered_bytes {
            // The host has not polled: no more payload until it does. Not marked, so not acknowledged, and
            // what it carried comes again. The header's acknowledgements are still good, and the peer is
            // alive; the body is not even inflated.
            self.last_recv = Some(now);
            self.process_acks(header.ack, header.ack_bits);
            self.c.refused += 1;
            return Verdict::Refused;
        }
        let mut budget = Budget::new();
        let (header, body) = match wire::decode_with(datagram, &mut budget) {
            Ok(x) => x,
            Err(r) => {
                self.c.malformed += 1;
                return Verdict::Rejected(r);
            }
        };
        self.last_recv = Some(now);
        if let Some(echo) = body.time_echo {
            let sample = (now as u32).wrapping_sub(echo);
            if sample < MAX_RTT_SAMPLE_MS {
                self.rtt.sample(sample);
            }
        }
        self.process_acks(header.ack, header.ack_bits);
        self.process_reliable_ack(body.reliable_ack);
        // The datagram that arrived last, not the one stamped newest: a reordered old datagram echoed with
        // its hold time is a fair (slow) sample, while always echoing the newest stamp would favour the
        // fastest packets and make the round trip look shorter than the acknowledgements take.
        self.peer_time = Some((body.time_ms, now));
        if body.has_payload() {
            self.ack_pending = true;
        }
        // The datagram counts as received (and is acknowledged) only when its content was taken: what the
        // receive buffers refused has to come again, and a datagram the peer believes delivered never does.
        let seq = header.seq;
        match self.accept(seq, body, &mut budget) {
            Take::Kept => {
                self.window.mark(seq);
                Verdict::Accepted
            }
            Take::Refused => {
                self.c.refused += 1;
                Verdict::Refused
            }
            Take::Malformed => Verdict::Rejected(Reject::Malformed),
        }
    }

    /// Take the content of a datagram. Reliable content comes first: if any of it is refused, nothing
    /// else in the datagram is delivered, because the datagram stays unacknowledged and may arrive again
    /// (a network duplicate, or the same sequence number again) and its unreliable messages would be
    /// delivered twice.
    fn accept(&mut self, seq: u32, body: Body, budget: &mut Budget) -> Take {
        let Body { unreliable, reliable, frag, .. } = body;
        let (blob_piece, unreliable_piece) = match frag {
            Some(f) if f.reliable => (Some(f), None),
            other => (None, other),
        };
        let mut worst = Take::Kept;
        if let Some(f) = blob_piece {
            worst = worst.max(self.recv_blob_piece(f));
        }
        for (rseq, m) in reliable {
            worst = worst.max(self.recv_reliable(rseq, m, budget));
        }
        if worst != Take::Kept {
            return worst;
        }
        if let Some(f) = unreliable_piece {
            self.recv_unreliable_piece(seq, f);
        }
        for m in unreliable {
            self.recv_unreliable(seq, m, budget);
        }
        Take::Kept
    }

    /// Messages delivered since the last call, in the order they became deliverable. Reliable ones are
    /// in send order and exactly once; unreliable ones are the newest, never repeated.
    pub fn poll(&mut self, now: u64) -> Vec<Delivered> {
        self.touch(now);
        // A partial unreliable message whose datagrams left the window can never complete.
        let hi = self.window.highest();
        self.un_asm.retain(|_, a| hi.wrapping_sub(a.max_seq) < 256 || seq_lt(hi, a.max_seq));
        self.delivered_bytes = 0;
        std::mem::take(&mut self.delivered)
    }

    fn recv_unreliable(&mut self, seq: u32, m: Msg, budget: &mut Budget) {
        if self.un_last_key.is_some_and(|last| seq_lt(seq, last)) {
            self.c.unreliable_stale += 1;
            return;
        }
        let Some(data) = open_inline(m.data, m.deflated, self.cfg.max_unreliable_bytes, budget) else {
            self.c.malformed += 1;
            return;
        };
        self.un_last_key = Some(seq);
        self.deliver_unreliable(data);
    }

    fn recv_unreliable_piece(&mut self, seq: u32, f: Fragment) {
        if f.count as usize > MAX_UNRELIABLE_PIECES || f.bytes.len() > FRAG_SIZE {
            self.c.malformed += 1;
            return;
        }
        let id = f.msg;
        let asm = self.un_asm.entry(id).or_insert_with(|| UnAsm { count: f.count, deflated: f.deflated, parts: BTreeMap::new(), max_seq: seq });
        if asm.count != f.count || asm.deflated != f.deflated {
            self.c.malformed += 1;
            return;
        }
        if seq_gt(seq, asm.max_seq) {
            asm.max_seq = seq;
        }
        asm.parts.insert(f.index, f.bytes);
        if asm.parts.len() < asm.count as usize {
            if self.un_asm.len() > MAX_UNRELIABLE_ASSEMBLIES
                && let Some(oldest) = self.un_asm.keys().copied().reduce(|a, b| if seq_lt(a, b) { a } else { b })
            {
                self.un_asm.remove(&oldest);
            }
            return;
        }
        let Some(asm) = self.un_asm.remove(&id) else { return };
        // Anything older that is still incomplete is out of date now.
        self.un_asm.retain(|&m, _| !seq_lt(m, id));
        if self.un_last_key.is_some_and(|last| !seq_gt(asm.max_seq, last)) {
            self.c.unreliable_stale += 1;
            return;
        }
        let joined: Vec<u8> = asm.parts.into_values().flatten().collect();
        let Some(data) = open_message(joined, asm.deflated, self.cfg.max_unreliable_bytes) else {
            self.c.malformed += 1;
            return;
        };
        self.un_last_key = Some(asm.max_seq);
        self.deliver_unreliable(data);
    }

    /// Whether a reliable message numbered `rseq` is worth buffering: not delivered yet, not buffered,
    /// inside the reorder window.
    fn wants_reliable(&mut self, rseq: u32) -> Want {
        let d = rseq.wrapping_sub(self.rin_next);
        if (d as i32) < 0 || self.rin_buf.contains_key(&rseq) {
            self.c.duplicates += 1;
            return Want::Have;
        }
        if d >= RELIABLE_WINDOW {
            self.c.reliable_overflow += 1;
            return Want::Refuse;
        }
        Want::Yes
    }

    /// The message the receiver waits for is always taken, whatever the buffer holds: it is delivered at
    /// once and lets everything buffered behind it go too. Refusing it would deadlock the stream. Anything
    /// further ahead is taken only while the reliable buffer has room, and refused (unacknowledged, so the
    /// sender keeps it) otherwise.
    fn has_room(&self, rseq: u32, bytes: usize) -> bool {
        rseq == self.rin_next || self.rin_bytes + bytes <= self.cfg.max_reliable_bytes
    }

    fn recv_reliable(&mut self, rseq: u32, m: Msg, budget: &mut Budget) -> Take {
        match self.wants_reliable(rseq) {
            Want::Have => return Take::Kept,
            Want::Refuse => return Take::Refused,
            Want::Yes => {}
        }
        // Inline, so at most what 900 bytes of deflate can be; anything bigger travels as a blob.
        let cap = INLINE_RELIABLE_INFLATED_MAX.min(self.cfg.max_blob_bytes);
        let Some(data) = open_inline(m.data, m.deflated, cap, budget) else {
            self.c.malformed += 1;
            return Take::Malformed;
        };
        if !self.has_room(rseq, data.len()) {
            self.c.reliable_overflow += 1;
            return Take::Refused;
        }
        self.rin_bytes += data.len();
        self.rin_buf.insert(rseq, data);
        self.deliver_ready();
        Take::Kept
    }

    fn recv_blob_piece(&mut self, f: Fragment) -> Take {
        let rseq = f.msg;
        match self.wants_reliable(rseq) {
            Want::Have => return Take::Kept,
            Want::Refuse => return Take::Refused,
            Want::Yes => {}
        }
        if f.count as usize > MAX_BLOB_PIECES || f.bytes.len() > FRAG_SIZE {
            self.c.malformed += 1;
            return Take::Malformed;
        }
        match self.rin_blobs.get(&rseq) {
            Some(blob) if blob.count != f.count as usize || blob.deflated != f.deflated => {
                self.c.malformed += 1;
                return Take::Malformed;
            }
            Some(blob) if blob.parts.contains_key(&f.index) => return Take::Kept,
            None if rseq != self.rin_next && self.rin_blobs.len() >= MAX_PARTIAL_BLOBS => {
                self.c.reliable_overflow += 1;
                return Take::Refused;
            }
            _ => {}
        }
        if !self.has_room(rseq, f.bytes.len()) {
            self.c.reliable_overflow += 1;
            return Take::Refused;
        }
        let mut blob = self.rin_blobs.remove(&rseq).unwrap_or_else(|| BlobIn {
            count: f.count as usize,
            deflated: f.deflated,
            parts: BTreeMap::new(),
            bytes: 0,
            inflated: None,
        });
        let piece_len = f.bytes.len();
        blob.parts.insert(f.index, f.bytes);
        if blob.parts.len() < blob.count {
            blob.bytes += piece_len;
            self.rin_bytes += piece_len;
            self.rin_blobs.insert(rseq, blob);
            return Take::Kept;
        }
        // This piece completes the blob. Inflated, the blob takes more room than its pieces did, and the
        // earlier pieces were acknowledged long ago: if the whole does not fit, this last piece is the one
        // to refuse (it stays unacknowledged and comes again), never the blob.
        let others = self.rin_bytes - blob.bytes;
        let fits = |n: usize| rseq == self.rin_next || others + n <= self.cfg.max_reliable_bytes;
        let data = match blob.inflated {
            Some(n) if !fits(n) => None,
            _ => {
                let joined: Vec<u8> = blob.parts.values().flatten().copied().collect();
                let Some(data) = open_message(joined, blob.deflated, self.cfg.max_blob_bytes) else {
                    // Unusable as a whole: forget it.
                    self.rin_bytes = others;
                    self.c.malformed += 1;
                    return Take::Malformed;
                };
                blob.inflated = Some(data.len());
                fits(data.len()).then_some(data)
            }
        };
        let Some(data) = data else {
            blob.parts.remove(&f.index);
            if !blob.parts.is_empty() {
                self.rin_blobs.insert(rseq, blob);
            }
            self.c.reliable_overflow += 1;
            return Take::Refused;
        };
        self.rin_bytes = others + data.len();
        self.rin_buf.insert(rseq, data);
        self.deliver_ready();
        Take::Kept
    }

    fn deliver_unreliable(&mut self, data: Vec<u8>) {
        self.c.unreliable_delivered += 1;
        self.delivered_bytes += data.len();
        self.delivered.push(Delivered::Unreliable(data));
    }

    fn deliver_ready(&mut self) {
        while let Some(data) = self.rin_buf.remove(&self.rin_next) {
            self.rin_bytes -= data.len();
            self.rin_next = self.rin_next.wrapping_add(1);
            self.c.reliable_delivered += 1;
            self.delivered_bytes += data.len();
            self.delivered.push(Delivered::Reliable(data));
        }
    }

    // ---- acknowledgements ------------------------------------------------------------------------

    fn rel_index(&self, rseq: u32) -> Option<usize> {
        let front = self.rel_out.front()?;
        let off = rseq.wrapping_sub(front.rseq) as usize;
        (off < self.rel_out.len()).then_some(off)
    }

    /// The peer has the message: free its data; pop what is acknowledged from the front.
    fn mark_acked(&mut self, pos: usize) {
        let item = &mut self.rel_out[pos];
        if item.acked {
            return;
        }
        item.acked = true;
        self.rel_out_bytes -= item.bytes;
        item.bytes = 0;
        if matches!(item.body, RelBody::Blob(_)) {
            self.blobs_queued -= 1;
        }
        item.body = RelBody::Done;
        while self.rel_out.front().is_some_and(|i| i.acked) {
            self.rel_out.pop_front();
        }
    }

    fn process_reliable_ack(&mut self, peer_next: u32) {
        if seq_gt(peer_next, self.rel_next) {
            return; // acknowledges what was never sent
        }
        while self.rel_out.front().is_some_and(|f| seq_lt(f.rseq, peer_next)) {
            self.mark_acked(0);
        }
    }

    fn process_acks(&mut self, ack: u32, ack_bits: u32) {
        if ack == 0 || seq_gt(ack, self.next_seq.wrapping_sub(1)) {
            return; // nothing acknowledged yet, or a datagram we never sent
        }
        self.ack_record(ack);
        for i in 0..32u32 {
            if ack_bits >> i & 1 == 1 {
                self.ack_record(ack.wrapping_sub(1 + i));
            }
        }
        if self.hi_acked.is_none_or(|h| seq_gt(ack, h)) {
            self.hi_acked = Some(ack);
        }
        self.detect_loss();
        self.trim_sent();
    }

    fn ack_record(&mut self, seq: u32) {
        let Some(front) = self.sent.front() else { return };
        let off = seq.wrapping_sub(front.seq) as usize;
        let Some(rec) = self.sent.get_mut(off) else { return };
        if rec.acked {
            return;
        }
        rec.acked = true;
        let rels = std::mem::take(&mut rec.rels);
        let frag = rec.frag.take();
        for (rseq, _) in rels {
            if let Some(pos) = self.rel_index(rseq) {
                self.mark_acked(pos);
            }
        }
        if let Some((rseq, idx, _)) = frag
            && let Some(pos) = self.rel_index(rseq)
        {
            let all = match &mut self.rel_out[pos].body {
                RelBody::Blob(b) => b.ack(idx as usize),
                _ => false,
            };
            if all {
                self.mark_acked(pos);
            }
        }
    }

    /// Datagrams inside the latest ack window that acknowledgements have passed by three and that have
    /// been out longer than a round trip with room for jitter (`srtt + 2·rttvar` plus the slack): lost. What they
    /// carried is sent again at the next flush instead of waiting for the timeout. Without the time
    /// guard, a jittery link that reorders datagrams a few places would look lossy and be flooded with
    /// needless resends.
    fn detect_loss(&mut self) {
        let (Some(hi), Some(front), Some(guard)) = (self.hi_acked, self.sent.front(), self.rtt.loss_delay(self.cfg.rto_slack_ms)) else { return };
        let front_seq = front.seq;
        let now = self.now;
        let mut lost: Vec<Carried> = Vec::new();
        for off in 0..self.sent.len() {
            let behind = hi.wrapping_sub(front_seq.wrapping_add(off as u32)) as i32;
            if behind < LOSS_THRESHOLD {
                break;
            }
            if behind > ACK_WINDOW {
                continue;
            }
            let rec = &mut self.sent[off];
            if rec.acked || rec.resolved || now < rec.sent_at + guard {
                continue;
            }
            rec.resolved = true;
            lost.push((std::mem::take(&mut rec.rels), rec.frag.take()));
        }
        for (rels, frag) in lost {
            for (rseq, tx) in rels {
                if let Some(pos) = self.rel_index(rseq) {
                    let item = &mut self.rel_out[pos];
                    if !item.acked && item.tx == tx && !item.resend_now {
                        item.resend_now = true;
                        self.c.fast_retransmits += 1;
                    }
                }
            }
            if let Some((rseq, idx, tx)) = frag
                && let Some(pos) = self.rel_index(rseq)
                && let RelBody::Blob(b) = &mut self.rel_out[pos].body
                && b.lose(idx as usize, tx)
            {
                self.c.fast_retransmits += 1;
            }
        }
    }

    /// Forget records that are settled, or too old for an ack to name (their content falls back to the
    /// retransmission timer).
    fn trim_sent(&mut self) {
        let hi = self.hi_acked;
        while let Some(front) = self.sent.front() {
            let too_old = hi.is_some_and(|h| h.wrapping_sub(front.seq) as i32 > ACK_WINDOW);
            if front.acked || front.resolved || too_old {
                self.sent.pop_front();
            } else {
                break;
            }
        }
    }

    // ---- state -----------------------------------------------------------------------------------

    /// True when the peer has not been heard for more than `timeout_ms` (a link never heard from times
    /// out `timeout_ms` after it was first used).
    pub fn is_timed_out(&self, now: u64, timeout_ms: u64) -> bool {
        match self.last_recv.or(self.started) {
            Some(t) => now.saturating_sub(t) > timeout_ms,
            None => false,
        }
    }

    /// Nothing waits: no reliable message unacknowledged, no unreliable message queued.
    pub fn is_idle(&self) -> bool {
        self.rel_out.is_empty() && self.un_out.is_empty()
    }

    pub fn stats(&self) -> LinkStats {
        let (bytes_in_per_s, bytes_out_per_s) = self.meter.rates(self.now);
        LinkStats {
            srtt_ms: self.rtt.srtt_ms(),
            rttvar_ms: self.rtt.rttvar_ms(),
            rto_ms: self.rto() as u32,
            loss: self.window.loss_window(),
            loss_lifetime: self.window.loss_lifetime(),
            bytes_in_per_s,
            bytes_out_per_s,
            bytes_in: self.c.bytes_in,
            bytes_out: self.c.bytes_out,
            datagrams_in: self.c.datagrams_in,
            datagrams_out: self.c.datagrams_out,
            resends: self.c.resends,
            fast_retransmits: self.c.fast_retransmits,
            reliable_in_flight: self.rel_out.iter().filter(|i| !i.acked).count() as u32,
            blobs_in_flight: self.blobs_queued as u32,
            reliable_queued_bytes: self.rel_out_bytes as u64,
            reliable_delivered: self.c.reliable_delivered,
            unreliable_delivered: self.c.unreliable_delivered,
            unreliable_stale: self.c.unreliable_stale,
            unreliable_dropped: self.c.unreliable_dropped,
            duplicates: self.c.duplicates,
            stale_datagrams: self.c.stale,
            rejected_magic: self.c.rejected_magic,
            rejected_proto: self.c.rejected_proto,
            rejected_session: self.c.rejected_session,
            malformed: self.c.malformed,
            reliable_overflow: self.c.reliable_overflow,
            refused_datagrams: self.c.refused,
            silence_ms: self.last_recv.map_or(0, |t| self.now.saturating_sub(t)),
        }
    }

    /// What the link holds in memory right now.
    pub fn footprint(&self) -> Footprint {
        Footprint {
            sent_records: self.sent.len(),
            reliable_out: self.rel_out.len(),
            reliable_out_bytes: self.rel_out_bytes,
            reliable_in_buffered: self.rin_buf.len(),
            reliable_in_bytes: self.rin_bytes,
            partial_blobs: self.rin_blobs.len(),
            unreliable_out: self.un_out.len(),
            unreliable_assemblies: self.un_asm.len(),
            undelivered: self.delivered.len(),
            undelivered_bytes: self.delivered_bytes,
        }
    }
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").field("session", &self.session).field("role", &self.role).field("footprint", &self.footprint()).finish()
    }
}
