//! Latency, jitter, loss and duplication simulation (`net_sim`, `--net-sim`; TZ E7.8).
//!
//! [`LinkConditions`] describes one direction of a link. [`Conditioner`] is the deterministic core
//! (what happens to packet number N); the in-memory hub uses it per link direction, and
//! [`Conditioned`] wraps any other transport (UDP) with the same code and a caller-supplied clock.
//! Every random draw comes from `DetRng::from_keys(&[seed, link, dir, packet_no])` and the number of draws
//! per packet is fixed, so changing the latency never changes which packets are lost.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use ai_core::DetRng;
use serde::{Deserialize, Serialize};

use crate::stats::DirStats;
use crate::transport::{PeerId, Transport};

/// One direction of a link. `latency_ms` is one way: 150 here means a 300 ms round trip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkConditions {
    /// Fixed one-way delay.
    pub latency_ms: u32,
    /// Each packet is delayed by `latency ± jitter` (uniform), so packets may reorder.
    pub jitter_ms: u32,
    /// Probability that a packet is dropped, 0..=1.
    pub loss: f32,
    /// Probability that a delivered packet is delivered twice, 0..=1.
    pub duplicate: f32,
}

impl LinkConditions {
    pub const NONE: LinkConditions = LinkConditions { latency_ms: 0, jitter_ms: 0, loss: 0.0, duplicate: 0.0 };

    /// No delay, no loss, no duplication: the packet passes straight through.
    pub fn is_none(&self) -> bool {
        self.latency_ms == 0 && self.jitter_ms == 0 && self.loss <= 0.0 && self.duplicate <= 0.0
    }

    /// Range check with a message that names the field and the fix.
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [("loss", self.loss), ("dup", self.duplicate)] {
            if !(0.0..=1.0).contains(&v) {
                return Err(format!("{name} must be between 0 and 1 (or 0% and 100%), got {v}"));
            }
        }
        Ok(())
    }
}

impl fmt::Display for LinkConditions {
    /// The form [`FromStr`] reads: `latency=150,jitter=10,loss=0.02,dup=0`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "latency={},jitter={},loss={},dup={}", self.latency_ms, self.jitter_ms, self.loss, self.duplicate)
    }
}

impl FromStr for LinkConditions {
    type Err = String;

    /// `"latency=150,jitter=10,loss=0.02,dup=0"`; every key is optional (default 0), `loss=2%` works,
    /// `latency_ms`, `jitter_ms` and `duplicate` are accepted as aliases, `""`/`off`/`none` mean no
    /// conditions. The error names the bad key.
    fn from_str(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let mut out = LinkConditions::NONE;
        if s.is_empty() || s == "off" || s == "none" {
            return Ok(out);
        }
        let mut seen: Vec<&'static str> = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((key, value)) = part.split_once('=') else {
                return Err(format!("\"{part}\" needs a value: write {part}=NUMBER (keys: latency, jitter, loss, dup)"));
            };
            let (key, value) = (key.trim(), value.trim());
            let canonical = match key {
                "latency" | "latency_ms" => "latency",
                "jitter" | "jitter_ms" => "jitter",
                "loss" => "loss",
                "dup" | "duplicate" => "dup",
                _ => return Err(format!("unknown key \"{key}\" (keys: latency, jitter, loss, dup)")),
            };
            if seen.contains(&canonical) {
                return Err(format!("key \"{canonical}\" is given twice"));
            }
            seen.push(canonical);
            match canonical {
                "latency" => out.latency_ms = parse_ms(canonical, value)?,
                "jitter" => out.jitter_ms = parse_ms(canonical, value)?,
                "loss" => out.loss = parse_probability(canonical, value)?,
                _ => out.duplicate = parse_probability(canonical, value)?,
            }
        }
        Ok(out)
    }
}

fn parse_ms(key: &str, value: &str) -> Result<u32, String> {
    let digits = value.strip_suffix("ms").unwrap_or(value).trim();
    digits.parse::<u32>().map_err(|_| format!("bad value \"{value}\" for \"{key}\": expected whole milliseconds, e.g. {key}=150"))
}

fn parse_probability(key: &str, value: &str) -> Result<f32, String> {
    let (number, scale) = match value.strip_suffix('%') {
        Some(n) => (n.trim(), 0.01),
        None => (value, 1.0),
    };
    let parsed: f32 = number
        .parse()
        .map_err(|_| format!("bad value \"{value}\" for \"{key}\": expected a number from 0 to 1 (or a percentage), e.g. {key}=0.02"))?;
    let p = parsed * scale;
    if !(0.0..=1.0).contains(&p) || !p.is_finite() {
        return Err(format!("bad value \"{value}\" for \"{key}\": must be between 0 and 1 (or 0% and 100%)"));
    }
    Ok(p)
}

/// What the conditioner decided for one packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fate {
    /// Delay of the packet in ms, `None` when it is dropped.
    pub delay_ms: Option<u32>,
    /// Delay of the duplicate in ms, when the packet is delivered twice.
    pub duplicate_delay_ms: Option<u32>,
}

/// The deterministic core: the fate of packet number N of one link direction.
#[derive(Clone, Debug)]
pub struct Conditioner {
    conditions: LinkConditions,
    seed: u64,
    link: u64,
    dir: u64,
    packet_no: u64,
}

impl Conditioner {
    pub fn new(seed: u64, link: u64, dir: u64, conditions: LinkConditions) -> Self {
        Self { conditions, seed, link, dir, packet_no: 0 }
    }

    pub fn conditions(&self) -> LinkConditions {
        self.conditions
    }

    /// Change the conditions; the packet counter (and so the random stream) continues.
    pub fn set_conditions(&mut self, conditions: LinkConditions) {
        self.conditions = conditions;
    }

    /// Packets decided so far.
    pub fn packets(&self) -> u64 {
        self.packet_no
    }

    /// The number the next packet gets (for logs).
    pub fn next_packet_no(&self) -> u64 {
        self.packet_no
    }

    /// Decide the next packet. Four draws, always: loss, jitter, duplication, duplicate jitter.
    pub fn fate(&mut self) -> Fate {
        let mut rng = DetRng::from_keys(&[self.seed, self.link, self.dir, self.packet_no]);
        self.packet_no += 1;
        let c = self.conditions;
        let lost = rng.f32() < c.loss;
        let jitter = rng.below(c.jitter_ms.saturating_mul(2).saturating_add(1).max(1)) as i64 - c.jitter_ms as i64;
        let dup = rng.f32() < c.duplicate;
        let dup_jitter = rng.below(c.jitter_ms.saturating_mul(2).saturating_add(1).max(1)) as i64 - c.jitter_ms as i64;
        let delay = |j: i64| (c.latency_ms as i64 + j).max(0) as u32;
        if lost {
            Fate { delay_ms: None, duplicate_delay_ms: None }
        } else {
            Fate { delay_ms: Some(delay(jitter)), duplicate_delay_ms: dup.then(|| delay(dup_jitter)) }
        }
    }
}

struct PeerCond {
    cond: Conditioner,
    /// Set by `set_conditions(Some(peer), ..)`: the peer no longer follows the default.
    overridden: bool,
}

/// A transport with simulated conditions on top: wraps UDP (or anything) with the same code the
/// in-memory hub uses. The clock is the caller's (`recv` and [`Conditioned::pump`] take `now`), never
/// the wall clock. Delays are applied when sending (`egress`, per peer when set so) and, if configured,
/// again when receiving (`ingress`). With no conditions a packet passes straight through.
///
/// `send` has no clock argument, so packets are stamped with the `now` of the latest `recv`/`pump`
/// call; a link that polls the transport once per tick before it flushes gets exact times.
pub struct Conditioned<T: Transport> {
    inner: T,
    seed: u64,
    link: u64,
    egress: Conditioner,
    peers: BTreeMap<PeerId, PeerCond>,
    ingress: Conditioner,
    out_q: BTreeMap<(u64, u64), (PeerId, Vec<u8>)>,
    in_q: BTreeMap<(u64, u64), (PeerId, Vec<u8>)>,
    seq: u64,
    now: u64,
    egress_stats: DirStats,
    ingress_stats: DirStats,
}

impl<T: Transport> Conditioned<T> {
    /// `link` keys the random stream (use the member index so two members of a stand differ).
    pub fn new(inner: T, seed: u64, link: u64, egress: LinkConditions) -> Self {
        Self {
            inner,
            seed,
            link,
            egress: Conditioner::new(seed, link, 0, egress),
            peers: BTreeMap::new(),
            ingress: Conditioner::new(seed, link, 1, LinkConditions::NONE),
            out_q: BTreeMap::new(),
            in_q: BTreeMap::new(),
            seq: 0,
            now: 0,
            egress_stats: DirStats::default(),
            ingress_stats: DirStats::default(),
        }
    }

    /// Conditions applied to everything this transport receives, on top of the peer's own egress.
    pub fn set_ingress(&mut self, conditions: LinkConditions) {
        self.ingress.set_conditions(conditions);
    }

    pub fn inner(&self) -> &T {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    pub fn into_inner(self) -> T {
        self.inner
    }

    pub fn egress_stats(&self) -> DirStats {
        self.egress_stats
    }

    pub fn ingress_stats(&self) -> DirStats {
        self.ingress_stats
    }

    /// Packets held back by the simulation right now (both directions).
    pub fn held(&self) -> usize {
        self.out_q.len() + self.in_q.len()
    }

    /// Move the clock to `now` and pass every delayed outgoing packet that is due on to the transport.
    pub fn pump(&mut self, now: u64) {
        self.now = self.now.max(now);
        while let Some(entry) = self.out_q.first_entry() {
            if entry.key().0 > self.now {
                break;
            }
            let (to, bytes) = entry.remove();
            self.egress_stats.delivered += 1;
            self.egress_stats.bytes_delivered += bytes.len() as u64;
            self.inner.send(to, &bytes);
        }
    }

    fn queue(q: &mut BTreeMap<(u64, u64), (PeerId, Vec<u8>)>, seq: &mut u64, due: u64, peer: PeerId, bytes: &[u8]) {
        *seq += 1;
        q.insert((due, *seq), (peer, bytes.to_vec()));
    }

    fn peer_key(&self, peer: PeerId) -> u64 {
        self.link ^ peer.rotate_left(17)
    }
}

impl<T: Transport> Transport for Conditioned<T> {
    fn send(&mut self, to: PeerId, datagram: &[u8]) {
        let default = self.egress.conditions();
        let key = self.peer_key(to);
        let seed = self.seed;
        let peer = self.peers.entry(to).or_insert_with(|| PeerCond { cond: Conditioner::new(seed, key, 0, default), overridden: false });
        if !peer.overridden {
            peer.cond.set_conditions(default);
        }
        self.egress_stats.sent += 1;
        self.egress_stats.bytes_sent += datagram.len() as u64;
        if peer.cond.conditions().is_none() {
            self.egress_stats.delivered += 1;
            self.egress_stats.bytes_delivered += datagram.len() as u64;
            self.inner.send(to, datagram);
            return;
        }
        let fate = peer.cond.fate();
        match fate.delay_ms {
            None => self.egress_stats.dropped += 1,
            Some(d) => {
                Self::queue(&mut self.out_q, &mut self.seq, self.now + d as u64, to, datagram);
                if let Some(d2) = fate.duplicate_delay_ms {
                    self.egress_stats.duplicated += 1;
                    Self::queue(&mut self.out_q, &mut self.seq, self.now + d2 as u64, to, datagram);
                }
            }
        }
        // Zero-delay copies leave at once.
        self.pump(self.now);
    }

    fn recv(&mut self, now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        self.pump(now);
        let mut raw = Vec::new();
        self.inner.recv(now, &mut raw);
        for (peer, bytes) in raw {
            self.ingress_stats.sent += 1;
            self.ingress_stats.bytes_sent += bytes.len() as u64;
            if self.ingress.conditions().is_none() {
                Self::queue(&mut self.in_q, &mut self.seq, self.now, peer, &bytes);
                continue;
            }
            let fate = self.ingress.fate();
            match fate.delay_ms {
                None => self.ingress_stats.dropped += 1,
                Some(d) => {
                    Self::queue(&mut self.in_q, &mut self.seq, self.now + d as u64, peer, &bytes);
                    if let Some(d2) = fate.duplicate_delay_ms {
                        self.ingress_stats.duplicated += 1;
                        Self::queue(&mut self.in_q, &mut self.seq, self.now + d2 as u64, peer, &bytes);
                    }
                }
            }
        }
        while let Some(entry) = self.in_q.first_entry() {
            if entry.key().0 > self.now {
                break;
            }
            let (peer, bytes) = entry.remove();
            self.ingress_stats.delivered += 1;
            self.ingress_stats.bytes_delivered += bytes.len() as u64;
            out.push((peer, bytes));
        }
    }

    fn describe(&self) -> String {
        let c = self.egress.conditions();
        if c.is_none() { self.inner.describe() } else { format!("{} (simulated {c})", self.inner.describe()) }
    }

    fn local_addr(&self) -> Option<String> {
        self.inner.local_addr()
    }

    fn forget(&mut self, peer: PeerId) {
        self.peers.remove(&peer);
        self.inner.forget(peer);
    }

    fn conditions(&self) -> Option<LinkConditions> {
        Some(self.egress.conditions())
    }

    fn set_conditions(&mut self, peer: Option<PeerId>, conditions: LinkConditions) -> bool {
        match peer {
            Some(p) => {
                let key = self.peer_key(p);
                let seed = self.seed;
                let entry = self.peers.entry(p).or_insert_with(|| PeerCond { cond: Conditioner::new(seed, key, 0, conditions), overridden: true });
                entry.cond.set_conditions(conditions);
                entry.overridden = true;
            }
            None => {
                self.egress.set_conditions(conditions);
                for p in self.peers.values_mut() {
                    p.overridden = false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "conditioner_tests.rs"]
mod tests;
