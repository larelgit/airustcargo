//! The in-memory hub: the transport of CI and of the in-process stand.
//!
//! Host and clients are separate `App`s in one process; each owns a [`MemoryTransport`], and the stand
//! owns the [`MemoryHub`] that connects them and moves the clock. Latency, jitter, loss and duplication
//! per link direction come from the same [`Conditioner`] the UDP wrapper uses, keyed by
//! `(seed, link, direction, packet number)`: the same seed and the same sends give the same deliveries,
//! byte for byte, and no wall clock is read anywhere.
//!
//! The hub's clock is whatever unit the caller advances it in (the stand uses milliseconds: one tick is
//! `1000 / hz` ms). 150 ms of latency means one way. Delivery order is `(due time, send order)`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::conditioner::{Conditioner, LinkConditions};
use crate::stats::DirStats;
use crate::transport::{MTU, PeerId, Transport};

/// Index of a link in creation order; part of the random key.
pub type LinkId = usize;

/// What the hub log records (opt-in, [`MemoryHub::set_log`]): the "network event log" of TZ E7.8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubEventKind {
    Sent,
    Dropped,
    Duplicated,
    Delivered,
    /// A datagram to an endpoint this one has no link to.
    Unroutable,
    /// A datagram above the MTU, refused.
    Oversize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HubEvent {
    /// Hub clock when it happened.
    pub at: u64,
    pub link: LinkId,
    /// 0: from the first endpoint given to `connect`, 1: from the second.
    pub dir: u8,
    pub packet_no: u64,
    pub kind: HubEventKind,
    pub len: usize,
}

/// Counters of one link, both directions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HubLinkStats {
    pub ab: DirStats,
    pub ba: DirStats,
}

struct Packet {
    from: PeerId,
    link: LinkId,
    dir: usize,
    packet_no: u64,
    bytes: Vec<u8>,
}

struct Endpoint {
    name: String,
    inbox: BTreeMap<(u64, u64), Packet>,
}

struct Direction {
    conditioner: Conditioner,
    stats: DirStats,
}

struct HubLink {
    a: PeerId,
    b: PeerId,
    dirs: [Direction; 2],
}

struct Hub {
    now: u64,
    seed: u64,
    seq: u64,
    endpoints: Vec<Endpoint>,
    links: Vec<HubLink>,
    route: BTreeMap<(PeerId, PeerId), (LinkId, usize)>,
    unroutable: u64,
    oversize: u64,
    log: Option<Vec<HubEvent>>,
}

const LOG_CAP: usize = 200_000;

impl Hub {
    fn record(&mut self, link: LinkId, dir: usize, packet_no: u64, kind: HubEventKind, len: usize) {
        let at = self.now;
        if let Some(log) = &mut self.log
            && log.len() < LOG_CAP
        {
            log.push(HubEvent { at, link, dir: dir as u8, packet_no, kind, len });
        }
    }

    fn send(&mut self, from: PeerId, to: PeerId, datagram: &[u8]) {
        if datagram.len() > MTU {
            // The link never builds one; a transport that gets one is asked to refuse it.
            debug_assert!(false, "datagram of {} bytes exceeds the MTU of {MTU}", datagram.len());
            self.oversize += 1;
            return;
        }
        let Some(&(link, dir)) = self.route.get(&(from, to)) else {
            self.unroutable += 1;
            return;
        };
        let d = &mut self.links[link].dirs[dir];
        let packet_no = d.conditioner.next_packet_no();
        let fate = d.conditioner.fate();
        d.stats.sent += 1;
        d.stats.bytes_sent += datagram.len() as u64;
        self.record(link, dir, packet_no, HubEventKind::Sent, datagram.len());
        let Some(delay) = fate.delay_ms else {
            self.links[link].dirs[dir].stats.dropped += 1;
            self.record(link, dir, packet_no, HubEventKind::Dropped, datagram.len());
            return;
        };
        self.deliver_later(link, dir, to, delay, packet_no, datagram);
        if let Some(delay) = fate.duplicate_delay_ms {
            self.links[link].dirs[dir].stats.duplicated += 1;
            self.record(link, dir, packet_no, HubEventKind::Duplicated, datagram.len());
            self.deliver_later(link, dir, to, delay, packet_no, datagram);
        }
    }

    fn deliver_later(&mut self, link: LinkId, dir: usize, to: PeerId, delay: u32, packet_no: u64, datagram: &[u8]) {
        self.seq += 1;
        let key = (self.now + delay as u64, self.seq);
        let from = if dir == 0 { self.links[link].a } else { self.links[link].b };
        self.endpoints[to as usize].inbox.insert(key, Packet { from, link, dir, packet_no, bytes: datagram.to_vec() });
    }

    fn recv(&mut self, me: PeerId, out: &mut Vec<(PeerId, Vec<u8>)>) {
        let now = self.now;
        while let Some(entry) = self.endpoints[me as usize].inbox.first_entry() {
            if entry.key().0 > now {
                break;
            }
            let p = entry.remove();
            let st = &mut self.links[p.link].dirs[p.dir].stats;
            st.delivered += 1;
            st.bytes_delivered += p.bytes.len() as u64;
            let len = p.bytes.len();
            self.record(p.link, p.dir, p.packet_no, HubEventKind::Delivered, len);
            out.push((p.from, p.bytes));
        }
    }
}

/// The hub. Cloning it clones the handle, not the hub.
#[derive(Clone)]
pub struct MemoryHub {
    inner: Arc<Mutex<Hub>>,
}

impl MemoryHub {
    /// `seed` keys every random draw of every link.
    pub fn new(seed: u64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Hub {
                now: 0,
                seed,
                seq: 0,
                endpoints: Vec::new(),
                links: Vec::new(),
                route: BTreeMap::new(),
                unroutable: 0,
                oversize: 0,
                log: None,
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Hub> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new endpoint; its `PeerId` is its creation index (the first endpoint is 0).
    pub fn endpoint(&self, name: &str) -> MemoryTransport {
        let mut h = self.lock();
        let id = h.endpoints.len() as PeerId;
        h.endpoints.push(Endpoint { name: name.to_string(), inbox: BTreeMap::new() });
        MemoryTransport { hub: self.clone(), id, name: name.to_string() }
    }

    /// Link two endpoints. `ab` applies to packets from `a` to `b`, `ba` the other way. Returns the
    /// link's index. Connecting the same pair twice replaces nothing: the first link keeps routing.
    pub fn connect(&self, a: PeerId, b: PeerId, ab: LinkConditions, ba: LinkConditions) -> LinkId {
        let mut h = self.lock();
        assert!((a as usize) < h.endpoints.len() && (b as usize) < h.endpoints.len(), "connect: unknown endpoint {a} or {b}");
        assert!(a != b, "connect: an endpoint cannot be linked to itself");
        let link = h.links.len();
        let seed = h.seed;
        h.links.push(HubLink {
            a,
            b,
            dirs: [
                Direction { conditioner: Conditioner::new(seed, link as u64, 0, ab), stats: DirStats::default() },
                Direction { conditioner: Conditioner::new(seed, link as u64, 1, ba), stats: DirStats::default() },
            ],
        });
        h.route.entry((a, b)).or_insert((link, 0));
        h.route.entry((b, a)).or_insert((link, 1));
        link
    }

    /// `connect` with the same conditions both ways.
    pub fn connect_both(&self, a: PeerId, b: PeerId, conditions: LinkConditions) -> LinkId {
        self.connect(a, b, conditions, conditions)
    }

    /// Change the conditions of a link (`net_sim`); the random streams continue.
    pub fn set_link_conditions(&self, link: LinkId, ab: LinkConditions, ba: LinkConditions) {
        let mut h = self.lock();
        h.links[link].dirs[0].conditioner.set_conditions(ab);
        h.links[link].dirs[1].conditioner.set_conditions(ba);
    }

    /// Conditions of a link, `(a→b, b→a)`.
    pub fn link_conditions(&self, link: LinkId) -> (LinkConditions, LinkConditions) {
        let h = self.lock();
        (h.links[link].dirs[0].conditioner.conditions(), h.links[link].dirs[1].conditioner.conditions())
    }

    /// Move the clock forward by `ms`.
    pub fn advance(&self, ms: u64) {
        self.lock().now += ms;
    }

    /// Move the clock to `now`. A clock never goes back: an earlier value is ignored.
    pub fn set_time(&self, now: u64) {
        let mut h = self.lock();
        h.now = h.now.max(now);
    }

    pub fn now(&self) -> u64 {
        self.lock().now
    }

    pub fn link_count(&self) -> usize {
        self.lock().links.len()
    }

    pub fn link_stats(&self, link: LinkId) -> HubLinkStats {
        let h = self.lock();
        HubLinkStats { ab: h.links[link].dirs[0].stats, ba: h.links[link].dirs[1].stats }
    }

    /// Datagrams sent to an endpoint without a link, and datagrams refused for their size.
    pub fn misrouted(&self) -> (u64, u64) {
        let h = self.lock();
        (h.unroutable, h.oversize)
    }

    /// Datagrams still on their way (queued, not yet due or not yet collected).
    pub fn in_transit(&self) -> usize {
        self.lock().endpoints.iter().map(|e| e.inbox.len()).sum()
    }

    /// Start or stop recording [`HubEvent`]s (capped at 200,000; stopping discards them).
    pub fn set_log(&self, on: bool) {
        let mut h = self.lock();
        h.log = on.then(Vec::new);
    }

    /// Take the events recorded so far.
    pub fn take_log(&self) -> Vec<HubEvent> {
        self.lock().log.as_mut().map(std::mem::take).unwrap_or_default()
    }
}

/// One endpoint of the hub. Implements [`Transport`]; the clock of `recv` is the hub's.
pub struct MemoryTransport {
    hub: MemoryHub,
    id: PeerId,
    name: String,
}

impl MemoryTransport {
    /// This endpoint's `PeerId`, the one its peers see datagrams from.
    pub fn id(&self) -> PeerId {
        self.id
    }

    pub fn hub(&self) -> &MemoryHub {
        &self.hub
    }

    /// The endpoints this one has a link to, ascending.
    pub fn peers(&self) -> Vec<PeerId> {
        let h = self.hub.lock();
        h.route.keys().filter(|(from, _)| *from == self.id).map(|(_, to)| *to).collect()
    }

    /// Name of an endpoint (`"host"`, `"c1"`), for logs.
    pub fn name_of(&self, id: PeerId) -> Option<String> {
        self.hub.lock().endpoints.get(id as usize).map(|e| e.name.clone())
    }
}

impl Transport for MemoryTransport {
    fn send(&mut self, to: PeerId, datagram: &[u8]) {
        self.hub.lock().send(self.id, to, datagram);
    }

    fn recv(&mut self, _now: u64, out: &mut Vec<(PeerId, Vec<u8>)>) {
        self.hub.lock().recv(self.id, out);
    }

    fn describe(&self) -> String {
        format!("hub {}", self.name)
    }

    /// Edit the links of this endpoint in both directions: the link to `peer`, or all of them.
    /// (The other end's own `set_conditions` would overwrite it: a hub link has one setting per direction.)
    fn set_conditions(&mut self, peer: Option<PeerId>, conditions: LinkConditions) -> bool {
        let mut h = self.hub.lock();
        let links: Vec<LinkId> = h
            .route
            .iter()
            .filter(|((from, to), _)| *from == self.id && peer.is_none_or(|p| p == *to))
            .map(|(_, (link, _))| *link)
            .collect();
        for link in links {
            for d in &mut h.links[link].dirs {
                d.conditioner.set_conditions(conditions);
            }
        }
        true
    }

    fn conditions(&self) -> Option<LinkConditions> {
        let h = self.hub.lock();
        h.route.iter().find(|((from, _), _)| *from == self.id).map(|(_, (link, dir))| h.links[*link].dirs[*dir].conditioner.conditions())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cond(latency: u32) -> LinkConditions {
        LinkConditions { latency_ms: latency, ..LinkConditions::NONE }
    }

    fn drain(t: &mut MemoryTransport) -> Vec<(PeerId, Vec<u8>)> {
        let mut out = Vec::new();
        t.recv(0, &mut out);
        out
    }

    #[test]
    fn latency_is_one_way_and_ordered() {
        let hub = MemoryHub::new(1);
        let (mut a, mut b) = (hub.endpoint("a"), hub.endpoint("b"));
        hub.connect_both(a.id(), b.id(), cond(150));
        a.send(b.id(), b"one");
        hub.advance(100);
        a.send(b.id(), b"two");
        hub.advance(49);
        assert!(drain(&mut b).is_empty(), "149 ms: nothing yet");
        hub.advance(1);
        assert_eq!(drain(&mut b), vec![(a.id(), b"one".to_vec())]);
        hub.advance(100);
        assert_eq!(drain(&mut b), vec![(a.id(), b"two".to_vec())]);
        assert!(drain(&mut a).is_empty());
        assert_eq!(hub.link_stats(0).ab.delivered, 2);
        assert_eq!(hub.link_stats(0).ba.sent, 0);
    }

    #[test]
    fn equal_due_times_keep_send_order() {
        let hub = MemoryHub::new(1);
        let (mut a, mut b) = (hub.endpoint("a"), hub.endpoint("b"));
        hub.connect_both(a.id(), b.id(), LinkConditions::NONE);
        for i in 0..5u8 {
            a.send(b.id(), &[i]);
        }
        let got: Vec<u8> = drain(&mut b).into_iter().map(|(_, d)| d[0]).collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn unlinked_peers_are_counted_not_delivered() {
        let hub = MemoryHub::new(1);
        let (mut a, b) = (hub.endpoint("a"), hub.endpoint("b"));
        a.send(b.id(), b"lost");
        assert_eq!(hub.misrouted(), (1, 0));
        assert_eq!(hub.in_transit(), 0);
    }

    #[test]
    fn star_topology_routes_by_peer() {
        let hub = MemoryHub::new(5);
        let (mut host, mut c1, mut c2) = (hub.endpoint("host"), hub.endpoint("c1"), hub.endpoint("c2"));
        hub.connect_both(host.id(), c1.id(), LinkConditions::NONE);
        hub.connect_both(host.id(), c2.id(), LinkConditions::NONE);
        assert_eq!(host.peers(), vec![c1.id(), c2.id()]);
        host.send(c1.id(), b"to1");
        host.send(c2.id(), b"to2");
        c1.send(host.id(), b"from1");
        c1.send(c2.id(), b"nolink");
        assert_eq!(drain(&mut c1), vec![(host.id(), b"to1".to_vec())]);
        assert_eq!(drain(&mut c2), vec![(host.id(), b"to2".to_vec())]);
        assert_eq!(drain(&mut host), vec![(c1.id(), b"from1".to_vec())]);
        assert_eq!(hub.misrouted().0, 1);
        assert_eq!(c1.describe(), "hub c1");
    }

    fn run(seed: u64) -> (Vec<(u64, PeerId, Vec<u8>)>, HubLinkStats) {
        let hub = MemoryHub::new(seed);
        let (mut a, mut b) = (hub.endpoint("a"), hub.endpoint("b"));
        let c = LinkConditions { latency_ms: 40, jitter_ms: 25, loss: 0.2, duplicate: 0.1 };
        hub.connect(a.id(), b.id(), c, LinkConditions { latency_ms: 10, ..c });
        let mut log = Vec::new();
        for step in 0..400u32 {
            let payload = step.to_le_bytes();
            a.send(b.id(), &payload);
            b.send(a.id(), &payload);
            hub.advance(3);
            for t in [&mut a, &mut b] {
                for (from, d) in drain(t) {
                    log.push((hub.now(), from, d));
                }
            }
        }
        (log, hub.link_stats(0))
    }

    #[test]
    fn same_seed_same_deliveries_different_seed_differs() {
        let (one, s1) = run(7);
        let (two, s2) = run(7);
        assert_eq!(one, two, "byte for byte");
        assert_eq!(s1, s2);
        let (other, _) = run(8);
        assert_ne!(one, other);
        assert!(s1.ab.dropped > 0 && s1.ab.duplicated > 0, "{s1:?}");
    }

    #[test]
    fn set_conditions_edits_both_directions_of_this_endpoints_links() {
        let hub = MemoryHub::new(1);
        let (mut host, mut c1, c2) = (hub.endpoint("host"), hub.endpoint("c1"), hub.endpoint("c2"));
        let l1 = hub.connect_both(host.id(), c1.id(), LinkConditions::NONE);
        let l2 = hub.connect_both(host.id(), c2.id(), LinkConditions::NONE);
        assert!(c1.set_conditions(None, cond(80)));
        assert_eq!(hub.link_conditions(l1), (cond(80), cond(80)));
        assert_eq!(hub.link_conditions(l2), (LinkConditions::NONE, LinkConditions::NONE));
        assert!(host.set_conditions(Some(c2.id()), cond(20)));
        assert_eq!(hub.link_conditions(l2), (cond(20), cond(20)));
        assert_eq!(c1.conditions(), Some(cond(80)));
        host.send(c1.id(), b"x");
        hub.advance(79);
        assert!(drain(&mut c1).is_empty());
        hub.advance(1);
        assert_eq!(drain(&mut c1).len(), 1);
    }

    #[test]
    fn event_log_records_what_happened() {
        let hub = MemoryHub::new(3);
        hub.set_log(true);
        let (mut a, mut b) = (hub.endpoint("a"), hub.endpoint("b"));
        hub.connect_both(a.id(), b.id(), LinkConditions { loss: 1.0, ..LinkConditions::NONE });
        a.send(b.id(), b"gone");
        let log = hub.take_log();
        assert_eq!(log.iter().map(|e| e.kind).collect::<Vec<_>>(), vec![HubEventKind::Sent, HubEventKind::Dropped]);
        assert!(hub.take_log().is_empty());
        assert!(drain(&mut b).is_empty());
    }
}
