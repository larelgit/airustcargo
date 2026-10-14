//! Link-layer property tests over the in-memory hub, driven by `DetRng`. Virtual time only: no sleeps,
//! no wall clock. The one UDP test at the end uses real sockets and real time (a few hundred ms).

use ai_core::DetRng;

use crate::wire;
use crate::*;

/// One simulated step in ms.
const DT: u64 = 4;

fn payload(key: u64, size: usize) -> Vec<u8> {
    let mut rng = DetRng::from_keys(&[0xC0_FFEE, key]);
    let mut v = Vec::with_capacity(size + 8);
    while v.len() < size {
        v.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    v.truncate(size);
    v
}

/// A message whose first four bytes are its number (size >= 8).
fn numbered(n: u32, size: usize) -> Vec<u8> {
    let mut v = payload(n as u64 | 1 << 40, size.max(8));
    v[..4].copy_from_slice(&n.to_le_bytes());
    v
}

fn number_of(m: &[u8]) -> u32 {
    u32::from_le_bytes([m[0], m[1], m[2], m[3]])
}

struct Pair {
    ta: MemoryTransport,
    tb: MemoryTransport,
    a: Link,
    b: Link,
    got_a: Vec<Delivered>,
    got_b: Vec<Delivered>,
    /// FNV-1a over every datagram each side built, in order: equal hashes mean equal bytes on the wire.
    wire: [u64; 2],
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

impl Pair {
    fn new(hub: &MemoryHub, session: u32, ab: LinkConditions, ba: LinkConditions) -> Self {
        Self::with_configs(hub, session, ab, ba, LinkConfig::default(), LinkConfig::default())
    }

    fn with_configs(hub: &MemoryHub, session: u32, ab: LinkConditions, ba: LinkConditions, ca: LinkConfig, cb: LinkConfig) -> Self {
        let ta = hub.endpoint("a");
        let tb = hub.endpoint("b");
        hub.connect(ta.id(), tb.id(), ab, ba);
        Pair {
            ta,
            tb,
            a: Link::with_config(session, Role::Client, ca),
            b: Link::with_config(session, Role::Host, cb),
            got_a: Vec::new(),
            got_b: Vec::new(),
            wire: [0xcbf2_9ce4_8422_2325; 2],
        }
    }

    /// Receive everything due and collect what the links deliver.
    fn pump(&mut self, now: u64) {
        let mut buf = Vec::new();
        self.ta.recv(now, &mut buf);
        for (_, d) in buf.drain(..) {
            self.a.on_datagram(now, &d);
        }
        self.tb.recv(now, &mut buf);
        for (_, d) in buf.drain(..) {
            self.b.on_datagram(now, &d);
        }
        self.got_a.extend(self.a.poll(now));
        self.got_b.extend(self.b.poll(now));
    }

    fn flush(&mut self, now: u64) {
        let (pa, pb) = (self.ta.id(), self.tb.id());
        let (ta, wire_a) = (&mut self.ta, &mut self.wire[0]);
        self.a.flush(now, &mut |d| {
            *wire_a = fnv(*wire_a, d);
            ta.send(pb, d)
        });
        let (tb, wire_b) = (&mut self.tb, &mut self.wire[1]);
        self.b.flush(now, &mut |d| {
            *wire_b = fnv(*wire_b, d);
            tb.send(pa, d)
        });
    }

    fn reliable_b(&self) -> Vec<&Vec<u8>> {
        self.got_b.iter().filter_map(|d| if let Delivered::Reliable(m) = d { Some(m) } else { None }).collect()
    }

    fn reliable_a(&self) -> Vec<&Vec<u8>> {
        self.got_a.iter().filter_map(|d| if let Delivered::Reliable(m) = d { Some(m) } else { None }).collect()
    }

    fn unreliable_b(&self) -> Vec<&Vec<u8>> {
        self.got_b.iter().filter_map(|d| if let Delivered::Unreliable(m) = d { Some(m) } else { None }).collect()
    }
}

struct Sim {
    hub: MemoryHub,
    pairs: Vec<Pair>,
    now: u64,
}

impl Sim {
    fn new(seed: u64) -> Self {
        Sim { hub: MemoryHub::new(seed), pairs: Vec::new(), now: 0 }
    }

    fn add(&mut self, ab: LinkConditions, ba: LinkConditions) -> usize {
        let session = 0x5000 + self.pairs.len() as u32;
        let p = Pair::new(&self.hub, session, ab, ba);
        self.pairs.push(p);
        self.pairs.len() - 1
    }

    /// One step: time moves, everything due is received, `hook` queues messages, links flush.
    fn step(&mut self, hook: &mut dyn FnMut(u64, &mut [Pair])) {
        self.now += DT;
        self.hub.advance(DT);
        for p in &mut self.pairs {
            p.pump(self.now);
        }
        hook(self.now, &mut self.pairs);
        for p in &mut self.pairs {
            p.flush(self.now);
        }
    }

    fn run_for(&mut self, ms: u64, hook: &mut dyn FnMut(u64, &mut [Pair])) {
        let end = self.now + ms;
        while self.now < end {
            self.step(hook);
        }
    }
}

fn cond(latency: u32, jitter: u32, loss: f32, dup: f32) -> LinkConditions {
    LinkConditions { latency_ms: latency, jitter_ms: jitter, loss, duplicate: dup }
}

const SIZES: [usize; 15] = [1, 2, 7, 64, 300, 899, 900, 901, 1000, 1024, 1500, 3000, 12_000, 70_000, 300_000];

fn pick_size(rng: &mut DetRng) -> usize {
    // Mostly small, sometimes a blob, rarely a big one.
    let roll = rng.below(100);
    let idx = match roll {
        0..=59 => rng.below(8) as usize,
        60..=89 => 8 + rng.below(5) as usize,
        90..=97 => 13,
        _ => 14,
    };
    SIZES[idx]
}

/// Send `count` reliable messages each way over every pair under the given conditions and check that
/// each arrives exactly once, in order, intact.
fn check_reliable_exchange(seed: u64, links: usize, count: usize, ab: LinkConditions, ba: LinkConditions) {
    let mut sim = Sim::new(seed);
    let mut rng = DetRng::from_keys(&[seed, 77]);
    for _ in 0..links {
        sim.add(ab, ba);
    }
    // Message k of pair p direction d: (key, size).
    let plans: Vec<[Vec<(u64, usize)>; 2]> = (0..links)
        .map(|p| {
            let mk = |d: u64, rng: &mut DetRng| (0..count).map(|k| ((p as u64) << 32 | d << 24 | k as u64, pick_size(rng))).collect::<Vec<_>>();
            [mk(0, &mut rng), mk(1, &mut rng)]
        })
        .collect();
    let mut sent = vec![[0usize; 2]; links];
    let spacing = 25;
    let mut hook = |now: u64, pairs: &mut [Pair]| {
        for (p, pair) in pairs.iter_mut().enumerate() {
            while sent[p][0] < count && sent[p][0] as u64 * spacing <= now {
                let (key, size) = plans[p][0][sent[p][0]];
                pair.a.send_reliable(&payload(key, size)).expect("queue has room");
                sent[p][0] += 1;
            }
            while sent[p][1] < count && sent[p][1] as u64 * spacing <= now {
                let (key, size) = plans[p][1][sent[p][1]];
                pair.b.send_reliable(&payload(key, size)).expect("queue has room");
                sent[p][1] += 1;
            }
        }
    };
    let limit = 3_000_000;
    let mut done_at = None;
    while sim.now < limit {
        sim.step(&mut hook);
        let all = sim.pairs.iter().all(|p| p.reliable_a().len() >= count && p.reliable_b().len() >= count);
        if all && done_at.is_none() {
            done_at = Some(sim.now);
        }
        if let Some(t) = done_at {
            // Keep running a while longer: a duplicate delivery would show up now.
            let idle = sim.pairs.iter().all(|p| p.a.is_idle() && p.b.is_idle());
            if sim.now > t + 3000 && idle {
                break;
            }
        }
    }
    let cfg = format!("seed {seed}, {links} links, ab [{ab}], ba [{ba}]");
    assert!(done_at.is_some(), "not everything arrived within {limit} ms ({cfg})");
    for (p, pair) in sim.pairs.iter().enumerate() {
        let to_b = pair.reliable_b();
        let to_a = pair.reliable_a();
        assert_eq!(to_b.len(), count, "exactly once a→b on pair {p} ({cfg})");
        assert_eq!(to_a.len(), count, "exactly once b→a on pair {p} ({cfg})");
        for (k, m) in to_b.iter().enumerate() {
            let (key, size) = plans[p][0][k];
            assert!(**m == payload(key, size), "message {k} a→b of {size} bytes differs or is out of order on pair {p} ({cfg})");
        }
        for (k, m) in to_a.iter().enumerate() {
            let (key, size) = plans[p][1][k];
            assert!(**m == payload(key, size), "message {k} b→a of {size} bytes differs or is out of order on pair {p} ({cfg})");
        }
        assert!(pair.a.is_idle() && pair.b.is_idle(), "everything acknowledged ({cfg})");
        let f = pair.a.footprint();
        assert_eq!((f.reliable_out, f.reliable_out_bytes, f.sent_records <= 64), (0, 0, true), "{f:?} ({cfg})");
    }
}

#[test]
fn reliable_messages_arrive_exactly_once_in_order_on_a_clean_link() {
    check_reliable_exchange(1, 1, 30, LinkConditions::NONE, LinkConditions::NONE);
}

#[test]
fn reliable_messages_arrive_exactly_once_in_order_under_loss_duplication_and_jitter() {
    let losses = [0.0, 0.05, 0.15, 0.30];
    let dups = [0.0, 0.05, 0.10];
    let jitters = [0, 10, 25, 50];
    let latencies = [0, 20, 60, 150, 300];
    let mut rng = DetRng::new(2026);
    for case in 0..14u64 {
        let pick = |rng: &mut DetRng, n: usize| rng.below(n as u32) as usize;
        let loss = losses[pick(&mut rng, 4)];
        let dup = dups[pick(&mut rng, 3)];
        let jitter = jitters[pick(&mut rng, 4)];
        let latency = latencies[pick(&mut rng, 5)];
        let links = 1 + pick(&mut rng, 3);
        let ab = cond(latency, jitter, loss, dup);
        // Asymmetric: the way back is a little different.
        let ba = cond(latency / 2 + 5, jitter / 2, loss * 0.5, dup);
        check_reliable_exchange(100 + case, links, 24, ab, ba);
    }
}

#[test]
fn the_worst_corner_still_delivers() {
    // 30 % loss, 10 % duplication, 50 ms jitter, 300 ms one way, both ways.
    check_reliable_exchange(9, 2, 20, cond(300, 50, 0.30, 0.10), cond(300, 50, 0.30, 0.10));
}

#[test]
fn two_megabyte_blobs_cross_a_lossy_link() {
    let mut sim = Sim::new(31);
    sim.add(cond(100, 20, 0.10, 0.05), cond(100, 20, 0.10, 0.05));
    let big = payload(1, 2 << 20);
    let other = payload(2, 1 << 20);
    let small = payload(3, 10);
    let mut queued = false;
    let mut hook = |_now: u64, pairs: &mut [Pair]| {
        if !queued {
            queued = true;
            pairs[0].a.send_reliable(&big).unwrap();
            pairs[0].a.send_reliable(&small).unwrap();
            pairs[0].b.send_reliable(&other).unwrap();
        }
    };
    while sim.now < 600_000 && !(sim.pairs[0].reliable_b().len() == 2 && sim.pairs[0].reliable_a().len() == 1) {
        sim.step(&mut hook);
    }
    let p = &sim.pairs[0];
    let got = p.reliable_b();
    assert_eq!(got.len(), 2, "a 2 MB blob and the message behind it arrive");
    assert!(*got[0] == big, "2 MB intact");
    assert_eq!(*got[1], small, "ordered: the small message waits for the blob");
    assert!(*p.reliable_a()[0] == other);
    assert!(sim.now < 300_000, "took {} simulated ms", sim.now);
    let s = p.a.stats();
    assert!(s.resends > 0, "30 % round-trip loss needs resends: {s:?}");
}

#[test]
fn blobs_are_sent_one_at_a_time_with_a_bounded_window() {
    // No loss: the number of fragments on the wire never exceeds the window.
    let mut sim = Sim::new(5);
    sim.add(cond(80, 0, 0.0, 0.0), cond(80, 0, 0.0, 0.0));
    sim.hub.set_log(true);
    let blob = payload(4, 200_000);
    let mut queued = false;
    let mut hook = |_n: u64, pairs: &mut [Pair]| {
        if !queued {
            queued = true;
            pairs[0].a.send_reliable(&blob).unwrap();
            pairs[0].a.send_reliable(&payload(5, 100_000)).unwrap();
        }
    };
    while sim.pairs[0].reliable_b().len() < 2 && sim.now < 120_000 {
        sim.step(&mut hook);
        let f = sim.hub.in_transit();
        assert!(f <= 32 + 8, "{f} datagrams in transit: the 32-fragment window is the bound");
    }
    assert_eq!(sim.pairs[0].reliable_b().len(), 2);
    assert!(*sim.pairs[0].reliable_b()[0] == blob);
    // 200 KB of random data is ~196 fragments, 32 per round trip of 160 ms+.
    assert!(sim.now > 6 * 160, "the window paces the blob: {} ms", sim.now);
}

#[test]
fn unreliable_messages_are_never_twice_and_never_out_of_date() {
    let sizes = [20usize, 300, 1000, 1100, 1101, 3000, 20_000];
    for (case, c) in [cond(50, 30, 0.10, 0.10), cond(0, 40, 0.0, 0.20), cond(120, 5, 0.25, 0.0)].into_iter().enumerate() {
        let mut sim = Sim::new(500 + case as u64);
        sim.add(c, c);
        let mut n = 0u32;
        let mut rng = DetRng::new(case as u64);
        let mut hook = |_now: u64, pairs: &mut [Pair]| {
            let size = sizes[rng.below(sizes.len() as u32) as usize];
            // 20 KB of noise is 20 fragments: send those less often so the queue is not the story here.
            if size < 20_000 || n.is_multiple_of(8) {
                pairs[0].a.send_unreliable(&numbered(n, size)).unwrap();
            } else {
                pairs[0].a.send_unreliable(&numbered(n, 300)).unwrap();
            }
            n += 1;
        };
        sim.run_for(30_000, &mut hook);
        sim.run_for(2_000, &mut |_, _| {});
        let got = sim.pairs[0].unreliable_b();
        assert!(got.len() > 100, "case {case}: {} delivered", got.len());
        let mut last = None;
        for m in &got {
            let id = number_of(m);
            assert!(last.is_none_or(|l| id > l), "case {case}: {id} after {last:?}: out of date or twice");
            last = Some(id);
            // Content intact whatever the size.
            assert!(m[4..] == numbered(id, m.len())[4..], "case {case}: message {id} damaged");
        }
        let s = sim.pairs[0].b.stats();
        assert_eq!(s.unreliable_delivered as usize, got.len());
    }
}

#[test]
fn unreliable_messages_all_arrive_in_order_on_a_clean_link() {
    let mut sim = Sim::new(8);
    sim.add(cond(40, 0, 0.0, 0.0), cond(40, 0, 0.0, 0.0));
    let sizes = [20usize, 1100, 1101, 5000, 40_000, 300];
    let total = 300u32;
    let mut n = 0;
    let mut hook = |_now: u64, pairs: &mut [Pair]| {
        if n < total {
            pairs[0].a.send_unreliable(&numbered(n, sizes[n as usize % sizes.len()])).unwrap();
            n += 1;
        }
    };
    sim.run_for(20_000, &mut hook);
    let got = sim.pairs[0].unreliable_b();
    assert_eq!(got.len(), total as usize, "nothing lost, nothing stale on a clean link");
    for (i, m) in got.iter().enumerate() {
        assert_eq!(number_of(m), i as u32);
    }
}

#[test]
fn compressible_messages_stay_inline_incompressible_ones_become_blobs() {
    let mut sim = Sim::new(2);
    sim.add(LinkConditions::NONE, LinkConditions::NONE);
    let json = format!("{{\"entities\":[{}]}}", (0..300).map(|i| format!("{{\"id\":\"e{i}\",\"hp\":{}}}", i % 50)).collect::<Vec<_>>().join(","));
    assert!(json.len() > 5000);
    sim.pairs[0].a.send_reliable(json.as_bytes()).unwrap();
    assert_eq!(sim.pairs[0].a.stats().blobs_in_flight, 0, "{} bytes of JSON deflate under 900", json.len());
    sim.pairs[0].a.send_reliable(&payload(1, 5000)).unwrap();
    assert_eq!(sim.pairs[0].a.stats().blobs_in_flight, 1);
    sim.run_for(2000, &mut |_, _| {});
    let got = sim.pairs[0].reliable_b();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].as_slice(), json.as_bytes());
    assert!(*got[1] == payload(1, 5000));
}

#[test]
fn measured_loss_matches_the_configured_loss() {
    for loss in [0.0f32, 0.05, 0.10, 0.30] {
        let mut sim = Sim::new(60 + (loss * 100.0) as u64);
        sim.add(cond(30, 0, loss, 0.05), cond(30, 0, 0.0, 0.0));
        let mut hook = |_n: u64, pairs: &mut [Pair]| {
            pairs[0].a.send_unreliable(&[1, 2, 3, 4]).unwrap();
        };
        // One datagram per step: 10,000 of them.
        sim.run_for(10_000 * DT, &mut hook);
        sim.run_for(500, &mut |_, _| {});
        let s = sim.pairs[0].b.stats();
        let hub = sim.hub.link_stats(0).ab;
        assert!(hub.sent >= 10_000, "{hub:?}");
        assert!((hub.loss() - loss).abs() < 0.015, "hub drops {} for {loss}", hub.loss());
        assert!((s.loss_lifetime - loss).abs() < 0.015, "link measures {} for configured {loss}: {s:?}", s.loss_lifetime);
        // The 256-datagram window is a small sample: sane, not exact.
        assert!((s.loss - loss).abs() < 0.10, "windowed loss {} for {loss}", s.loss);
        // Duplicated datagrams do not count as received twice and are never delivered twice.
        assert!(s.duplicates > 0 && s.unreliable_delivered <= s.datagrams_in - s.duplicates, "{s:?}");
    }
}

#[test]
fn rtt_and_rates_are_sane() {
    let mut sim = Sim::new(3);
    sim.add(cond(100, 0, 0.0, 0.0), cond(100, 0, 0.0, 0.0));
    let mut hook = |now: u64, pairs: &mut [Pair]| {
        if now.is_multiple_of(40) {
            pairs[0].a.send_reliable(&[7; 100]).unwrap();
            pairs[0].b.send_unreliable(&[9; 60]).unwrap();
        }
    };
    sim.run_for(10_000, &mut hook);
    let (a, b) = (sim.pairs[0].a.stats(), sim.pairs[0].b.stats());
    for s in [&a, &b] {
        assert!((s.srtt_ms - 200.0).abs() < 12.0, "srtt {} for a 100 ms one-way link: {s:?}", s.srtt_ms);
        assert!(s.rttvar_ms < 12.0, "{s:?}");
        assert!(s.rto_ms >= 200 && s.rto_ms < 400, "rto {}", s.rto_ms);
        assert_eq!(s.loss, 0.0);
    }
    let hub = sim.hub.link_stats(0);
    let measured = a.bytes_out_per_s as f64;
    let truth = hub.ab.bytes_sent as f64 / 10.0;
    assert!((measured - truth).abs() / truth < 0.2, "{measured} B/s measured, {truth} B/s truth");
    assert_eq!(a.bytes_out, hub.ab.bytes_sent);
    assert_eq!(b.bytes_in, hub.ab.bytes_delivered);
    assert_eq!(a.resends, 0, "a clean link needs no resends: {a:?}");
}

#[test]
fn memory_stays_bounded_over_a_hundred_thousand_messages() {
    let mut sim = Sim::new(44);
    let c = cond(30, 8, 0.05, 0.02);
    sim.add(c, c);
    let total = 100_000u32;
    let mut next = 0u32;
    let mut queue_full = 0u64;
    let mut rng = DetRng::new(5);
    let mut max = Footprint::default();
    let mut unreliable_n = 0u32;
    let a_to_b_expected = std::cell::Cell::new(0u32);
    {
        let mut hook = |_now: u64, pairs: &mut [Pair]| {
            let p = &mut pairs[0];
            while next < total {
                let size = 8 + rng.below(300) as usize;
                match p.a.send_reliable(&numbered(next, size)) {
                    Ok(()) => next += 1,
                    Err(SendError::QueueFull { .. }) => {
                        queue_full += 1;
                        break;
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            // Traffic the other way too, unreliable, more than the queue can hold per step.
            for _ in 0..3 {
                p.b.send_unreliable(&numbered(unreliable_n, 40)).unwrap();
                unreliable_n += 1;
            }
            for f in [p.a.footprint(), p.b.footprint()] {
                max.sent_records = max.sent_records.max(f.sent_records);
                max.reliable_out = max.reliable_out.max(f.reliable_out);
                max.reliable_out_bytes = max.reliable_out_bytes.max(f.reliable_out_bytes);
                max.reliable_in_buffered = max.reliable_in_buffered.max(f.reliable_in_buffered);
                max.reliable_in_bytes = max.reliable_in_bytes.max(f.reliable_in_bytes);
                max.partial_blobs = max.partial_blobs.max(f.partial_blobs);
                max.unreliable_out = max.unreliable_out.max(f.unreliable_out);
                max.unreliable_assemblies = max.unreliable_assemblies.max(f.unreliable_assemblies);
            }
            // Receivers drain as they go; the test does not keep what it has verified.
            for d in std::mem::take(&mut p.got_b) {
                let Delivered::Reliable(m) = d else { continue };
                assert_eq!(number_of(&m), a_to_b_expected.get(), "in order, exactly once");
                a_to_b_expected.set(a_to_b_expected.get() + 1);
            }
            p.got_a.clear();
        };
        let mut guard = 0;
        while a_to_b_expected.get() < total && guard < 2_000_000 {
            sim.step(&mut hook);
            guard += 1;
        }
    }
    assert_eq!(a_to_b_expected.get(), total, "all {total} messages delivered");
    assert!(queue_full > 0, "the queue cap was reached, so it was exercised");
    assert!(max.reliable_out <= MAX_RELIABLE, "{max:?}");
    assert!(max.reliable_out_bytes <= 1024 * 310, "{max:?}");
    assert!(max.sent_records <= 512, "{max:?}");
    assert!(max.reliable_in_buffered <= MAX_RELIABLE * 2, "{max:?}");
    assert!(max.reliable_in_bytes <= 1024 * 700, "{max:?}");
    assert!(max.unreliable_out <= 256 && max.unreliable_assemblies <= 8 && max.partial_blobs <= 4, "{max:?}");
    assert!(sim.pairs[0].a.stats().unreliable_delivered > 1000, "the unreliable stream flowed all along");
}

const MAX_RELIABLE: usize = link::MAX_RELIABLE_MESSAGES;

#[test]
fn the_reliable_queue_refuses_instead_of_growing() {
    let hub = MemoryHub::new(1);
    let mut p = Pair::new(&hub, 9, LinkConditions::NONE, LinkConditions::NONE);
    // b never speaks, so nothing is ever acknowledged.
    let mut accepted = 0;
    let err = loop {
        match p.a.send_reliable(&[5; 100]) {
            Ok(()) => accepted += 1,
            Err(e) => break e,
        }
    };
    assert_eq!(accepted, MAX_RELIABLE);
    assert!(matches!(err, SendError::QueueFull { queued_messages: 1024, queued_bytes: 102_400 }), "{err:?}");
    assert!(err.to_string().contains("not acknowledging"), "{err}");
    assert_eq!(p.a.footprint().reliable_out, MAX_RELIABLE);
    // Flushing and retrying for a long time does not grow anything either.
    for step in 0..2000u64 {
        let now = step * DT;
        hub.advance(DT);
        p.flush(now);
        let _ = p.a.send_reliable(&[5; 100]);
    }
    let f = p.a.footprint();
    assert_eq!((f.reliable_out, f.reliable_out_bytes), (MAX_RELIABLE, 102_400));
    assert!(f.sent_records <= 512);
    // A message over the blob limit is refused outright, with the limit named.
    let mut small = Link::with_config(1, Role::Client, LinkConfig { max_blob_bytes: 1 << 20, ..LinkConfig::default() });
    let e = small.send_reliable(&vec![0; (1 << 20) + 1]).unwrap_err();
    assert_eq!(e, SendError::TooLarge { len: (1 << 20) + 1, max: 1 << 20 });
    assert!(Link::new(1, Role::Client).send_reliable(&vec![0; (16 << 20) + 1]).is_err());
    assert!(Link::new(1, Role::Client).send_reliable(&vec![0; 16 << 20]).is_ok(), "16 MB is the cap, inclusive");
    // The byte cap refuses too.
    let mut tiny = Link::with_config(1, Role::Client, LinkConfig { max_reliable_bytes: 1000, ..LinkConfig::default() });
    assert!(tiny.send_reliable(&[0; 600]).is_ok());
    assert!(matches!(tiny.send_reliable(&[0; 600]), Err(SendError::QueueFull { .. })));
    // Unreliable: the queue drops its oldest, and says so.
    let mut u = Link::with_config(1, Role::Client, LinkConfig { max_unreliable_queue: 4, ..LinkConfig::default() });
    for i in 0..10u8 {
        u.send_unreliable(&[i]).unwrap();
    }
    assert_eq!(u.footprint().unreliable_out, 4);
    assert_eq!(u.stats().unreliable_dropped, 6);
}

#[test]
fn timeouts_fire_only_when_the_peer_goes_quiet() {
    let mut sim = Sim::new(12);
    sim.add(cond(20, 0, 0.0, 0.0), cond(20, 0, 0.0, 0.0));
    sim.run_for(30_000, &mut |_, _| {});
    let now = sim.now;
    for l in [&sim.pairs[0].a, &sim.pairs[0].b] {
        assert!(!l.is_timed_out(now, 1000), "keepalives keep an idle link alive");
        assert!(l.stats().silence_ms < 400, "{}", l.stats().silence_ms);
    }
    // Pull the cable.
    sim.hub.set_link_conditions(0, cond(20, 0, 1.0, 0.0), cond(20, 0, 1.0, 0.0));
    let cut = sim.now;
    let mut fired_at = None;
    while sim.now < cut + 20_000 {
        sim.run_for(100, &mut |_, _| {});
        if fired_at.is_none() && sim.pairs[0].b.is_timed_out(sim.now, 3000) {
            fired_at = Some(sim.now - cut);
        }
    }
    let at = fired_at.expect("a silent link times out");
    assert!((3000..3600).contains(&at), "3 s timeout fired {at} ms after the cut");
    assert!(sim.pairs[0].a.is_timed_out(sim.now, 3000));
    // Reliable traffic in flight does not mask it: resends are not signs of life.
    sim.pairs[0].a.send_reliable(b"anyone?").unwrap();
    sim.run_for(5000, &mut |_, _| {});
    assert!(sim.pairs[0].a.is_timed_out(sim.now, 3000));
    // Cable back: the message gets through and the link is alive again.
    sim.hub.set_link_conditions(0, cond(20, 0, 0.0, 0.0), cond(20, 0, 0.0, 0.0));
    sim.run_for(8000, &mut |_, _| {});
    assert!(!sim.pairs[0].a.is_timed_out(sim.now, 3000));
    assert_eq!(sim.pairs[0].reliable_b().len(), 1);
    // A link nobody ever answered times out too, measured from first use.
    let mut lonely = Link::new(1, Role::Client);
    lonely.flush(1000, &mut |_| {});
    assert!(!lonely.is_timed_out(2000, 3000));
    assert!(lonely.is_timed_out(4001, 3000));
    assert!(!Link::new(1, Role::Host).is_timed_out(999_999, 1), "never used: nothing to time out");
}

#[test]
fn an_idle_link_sends_only_keepalives() {
    let mut sim = Sim::new(1);
    sim.add(LinkConditions::NONE, LinkConditions::NONE);
    sim.pairs[0].a.send_reliable(b"hi").unwrap();
    sim.run_for(2000, &mut |_, _| {});
    let before = sim.pairs[0].a.stats().datagrams_out + sim.pairs[0].b.stats().datagrams_out;
    sim.run_for(10_000, &mut |_, _| {});
    let after = sim.pairs[0].a.stats().datagrams_out + sim.pairs[0].b.stats().datagrams_out;
    // Keepalive every 250 ms of silence from each side: about 8 per second between them.
    let rate = (after - before) as f64 / 10.0;
    assert!((6.0..=10.0).contains(&rate), "{rate} datagrams/s when idle");
    let s = sim.pairs[0].a.stats();
    assert!(s.bytes_out_per_s < 400, "idle traffic {} B/s", s.bytes_out_per_s);
}

#[test]
fn foreign_datagrams_are_dropped_and_counted() {
    let mut mine = Link::new(0xAAAA, Role::Host);
    let mut other = Link::new(0xBBBB, Role::Client);
    other.send_reliable(b"not for you").unwrap();
    let mut grams = Vec::new();
    other.flush(0, &mut |d| grams.push(d.to_vec()));
    assert_eq!(wire::peek_session(&grams[0]), Some(0xBBBB));
    assert_eq!(mine.on_datagram(5, &grams[0]), Verdict::Rejected(Reject::Session(0xBBBB)));
    assert_eq!(mine.on_datagram(5, b"GET / HTTP/1.1\r\nHost: x\r\n\r\n"), Verdict::Rejected(Reject::Magic));
    let mut wrong_proto = grams[0].clone();
    wrong_proto[2] = 99;
    assert_eq!(mine.on_datagram(5, &wrong_proto), Verdict::Rejected(Reject::Proto(99)));
    let mut cut = Link::new(0xAAAA, Role::Client);
    cut.send_reliable(&[1; 50]).unwrap();
    let mut mine_grams = Vec::new();
    cut.flush(0, &mut |d| mine_grams.push(d.to_vec()));
    let short = &mine_grams[0][..mine_grams[0].len() - 5];
    assert_eq!(mine.on_datagram(5, short), Verdict::Rejected(Reject::Malformed));
    let s = mine.stats();
    assert_eq!((s.rejected_session, s.rejected_magic, s.rejected_proto, s.malformed), (1, 1, 1, 1));
    assert!(mine.poll(6).is_empty(), "nothing foreign is delivered");
    // The good one passes, once.
    assert_eq!(mine.on_datagram(7, &mine_grams[0]), Verdict::Accepted);
    assert_eq!(mine.on_datagram(7, &mine_grams[0]), Verdict::Duplicate);
    assert_eq!(mine.poll(8), vec![Delivered::Reliable(vec![1; 50])]);
}

#[test]
fn a_host_creates_the_link_from_the_session_the_client_chose() {
    let hub = MemoryHub::new(3);
    let mut client_t = hub.endpoint("client");
    let mut host_t = hub.endpoint("host");
    hub.connect_both(client_t.id(), host_t.id(), LinkConditions::NONE);
    let mut client = Link::new(0x1234_5678, Role::Client);
    client.send_reliable(b"Hello").unwrap();
    client.flush(0, &mut |d| client_t.send(host_t.id(), d));
    let mut incoming = Vec::new();
    host_t.recv(0, &mut incoming);
    let (peer, datagram) = &incoming[0];
    let session = wire::peek_session(datagram).expect("a datagram of ours");
    let mut link = Link::new(session, Role::Host);
    assert_eq!(link.on_datagram(0, datagram), Verdict::Accepted);
    assert_eq!(link.poll(0), vec![Delivered::Reliable(b"Hello".to_vec())]);
    // The host answers and the client hears it.
    link.send_reliable(b"Accept").unwrap();
    link.flush(0, &mut |d| host_t.send(*peer, d));
    let mut back = Vec::new();
    client_t.recv(0, &mut back);
    for (_, d) in back {
        client.on_datagram(1, &d);
    }
    assert_eq!(client.poll(1), vec![Delivered::Reliable(b"Accept".to_vec())]);
}

/// Everything observable about a run: the hub's event log, a hash of every datagram built, and what was
/// delivered, in order.
fn transcript(seed: u64) -> (Vec<HubEvent>, Vec<[u64; 2]>, Vec<Vec<u8>>) {
    let mut sim = Sim::new(seed);
    sim.hub.set_log(true);
    let c = cond(60, 25, 0.12, 0.08);
    sim.add(c, c);
    sim.add(cond(10, 0, 0.0, 0.0), cond(200, 40, 0.2, 0.1));
    let mut rng = DetRng::new(1);
    let mut hook = |now: u64, pairs: &mut [Pair]| {
        for p in pairs.iter_mut() {
            if now.is_multiple_of(24) {
                let size = 1 + rng.below(2500) as usize;
                p.a.send_reliable(&payload(now, size)).unwrap();
                p.b.send_unreliable(&numbered((now / 24) as u32, 50 + size % 400)).unwrap();
            }
        }
    };
    sim.run_for(12_000, &mut hook);
    let mut delivered = Vec::new();
    for p in &sim.pairs {
        for d in p.got_a.iter().chain(p.got_b.iter()) {
            match d {
                Delivered::Reliable(m) | Delivered::Unreliable(m) => delivered.push(m.clone()),
            }
        }
    }
    let wire = sim.pairs.iter().map(|p| p.wire).collect();
    (sim.hub.take_log(), wire, delivered)
}

#[test]
fn the_whole_stack_is_deterministic_on_the_hub() {
    let (log1, w1, d1) = transcript(77);
    let (log2, w2, d2) = transcript(77);
    assert!(log1.len() > 1000);
    assert_eq!(log1, log2, "same seed, same datagrams at the same times with the same sizes");
    assert_eq!(w1, w2, "the links built the same bytes");
    assert_eq!(d1, d2, "same messages delivered in the same order, byte for byte");
    let (log3, w3, _) = transcript(78);
    assert_ne!(log1, log3, "a different seed is a different run");
    assert_ne!(w1, w3);
}

#[test]
fn per_message_overhead_is_small() {
    // 1000 reliable messages of 100 bytes, as fast as the link takes them.
    let mut sim = Sim::new(1);
    sim.add(LinkConditions::NONE, LinkConditions::NONE);
    for k in 0..1000u64 {
        // Incompressible: the overhead is measured without help from deflate.
        sim.pairs[0].a.send_reliable(&payload(k, 100)).unwrap();
    }
    sim.run_for(3000, &mut |_, _| {});
    assert_eq!(sim.pairs[0].reliable_b().len(), 1000);
    let a = sim.pairs[0].a.stats();
    let per_message = a.bytes_out as f64 / 1000.0;
    eprintln!("reliable 100 B messages in bulk: {per_message:.1} B on the wire per message, {} datagrams", a.datagrams_out);
    assert!(per_message < 112.0, "{per_message}");
    // And one by one, a datagram each: header 20 + time/echo/ack 12 + counts 2 + rseq 4 + length 2.
    let mut single = Sim::new(1);
    single.add(LinkConditions::NONE, LinkConditions::NONE);
    single.pairs[0].a.send_reliable(&payload(1, 100)).unwrap();
    let mut size = 0;
    single.pairs[0].a.flush(0, &mut |d| size = d.len());
    eprintln!("one reliable 100 B message alone: {size} B datagram");
    assert_eq!(size, 20 + 12 + 2 + 4 + 2 + 100);
}

#[test]
fn losses_are_repaired_by_acks_not_only_by_timeouts() {
    // A steady stream both ways: acknowledgements pass over lost datagrams, which are sent again
    // without waiting for the timer.
    let mut sim = Sim::new(21);
    let c = cond(80, 0, 0.10, 0.0);
    sim.add(c, c);
    let mut n = 0u32;
    let mut hook = |_now: u64, pairs: &mut [Pair]| {
        pairs[0].a.send_reliable(&numbered(n, 60)).unwrap();
        n += 1;
    };
    sim.run_for(20_000, &mut hook);
    sim.run_for(3_000, &mut |_, _| {});
    let s = sim.pairs[0].a.stats();
    let got = sim.pairs[0].reliable_b();
    assert_eq!(got.len(), n as usize, "every message arrived: {s:?}");
    for (i, m) in got.iter().enumerate() {
        assert_eq!(number_of(m), i as u32);
    }
    assert!(s.fast_retransmits > 20, "{s:?}");
    assert!(s.fast_retransmits * 2 > s.resends, "most repairs come from acks, not timers: {s:?}");
}

#[test]
fn the_public_types_can_live_in_a_bevy_resource() {
    fn send_sync<T: Send + Sync + 'static>() {}
    send_sync::<Link>();
    send_sync::<MemoryHub>();
    send_sync::<MemoryTransport>();
    send_sync::<UdpTransport>();
    send_sync::<Conditioned<UdpTransport>>();
    send_sync::<Conditioned<Box<dyn Transport>>>();
    send_sync::<Box<dyn Transport>>();
    send_sync::<LinkStats>();
}

#[test]
fn reordering_alone_does_not_cause_a_resend_storm() {
    // 50 ms of jitter at one datagram per 4 ms step reorders datagrams a dozen places deep. The link
    // may resend some messages needlessly (acks that arrive late look like losses) but must not flood.
    let mut sim = Sim::new(23);
    let c = cond(100, 50, 0.0, 0.0);
    sim.add(c, c);
    let mut n = 0u32;
    let mut hook = |_now: u64, pairs: &mut [Pair]| {
        pairs[0].a.send_reliable(&numbered(n, 100)).unwrap();
        n += 1;
    };
    sim.run_for(20_000, &mut hook);
    sim.run_for(3_000, &mut |_, _| {});
    let s = sim.pairs[0].a.stats();
    eprintln!("jitter only: {n} messages, {} resends ({} fast), {} datagrams out", s.resends, s.fast_retransmits, s.datagrams_out);
    assert_eq!(sim.pairs[0].reliable_b().len(), n as usize);
    assert!(s.resends * 50 < n as u64, "{} resends for {n} messages: {s:?}", s.resends);
}

#[test]
fn a_lost_ack_only_costs_a_resend_that_is_dropped_as_duplicate() {
    // Everything b says is lost for a while: a keeps resending; b delivers each message once.
    let mut sim = Sim::new(22);
    sim.add(cond(10, 0, 0.0, 0.0), cond(10, 0, 1.0, 0.0));
    sim.pairs[0].a.send_reliable(b"once").unwrap();
    // Timeouts back off (500, 1000, 2000 ms...): three resends take a few seconds.
    sim.run_for(8000, &mut |_, _| {});
    assert_eq!(sim.pairs[0].reliable_b().len(), 1, "delivered exactly once");
    let s = sim.pairs[0].a.stats();
    assert!(s.resends >= 3, "no ack ever came back, so it kept trying: {s:?}");
    assert_eq!(s.reliable_in_flight, 1);
    assert!(sim.pairs[0].b.stats().duplicates >= 3);
    // Acks flow again: the queue empties.
    sim.hub.set_link_conditions(0, cond(10, 0, 0.0, 0.0), cond(10, 0, 0.0, 0.0));
    sim.run_for(6000, &mut |_, _| {});
    assert!(sim.pairs[0].a.is_idle());
    assert_eq!(sim.pairs[0].reliable_b().len(), 1);
}

#[test]
fn garbage_and_mutated_datagrams_never_panic_or_break_the_link() {
    let mut rng = DetRng::new(404);
    // Real datagrams from a busy link: acks, inline messages, blob pieces, unreliable fragments.
    let mut sender = Link::new(0xABCD, Role::Client);
    let mut genuine: Vec<Vec<u8>> = Vec::new();
    for i in 0..40u64 {
        let size = [10usize, 500, 950, 3000, 40_000][i as usize % 5];
        sender.send_reliable(&payload(i, size)).unwrap();
        sender.send_unreliable(&payload(1000 + i, size)).unwrap();
        sender.flush(i * 20, &mut |d| genuine.push(d.to_vec()));
    }
    assert!(genuine.len() > 100);
    let mut link = Link::new(0xABCD, Role::Host);
    let mut now = 0;
    for round in 0..4000 {
        now += 3;
        let mut d = genuine[rng.below(genuine.len() as u32) as usize].clone();
        match round % 5 {
            0 => {
                // Flip a few bits anywhere.
                for _ in 0..1 + rng.below(4) {
                    let at = rng.below(d.len() as u32) as usize;
                    d[at] ^= 1 << rng.below(8);
                }
            }
            1 => d.truncate(rng.below(d.len() as u32) as usize),
            2 => {
                // Keep the header, replace the body with noise.
                d.truncate(wire::HEADER_LEN);
                d.extend((0..rng.below(300)).map(|_| rng.next_u64() as u8));
            }
            3 => d = (0..rng.below(1300)).map(|_| rng.next_u64() as u8).collect(),
            _ => {}
        }
        link.on_datagram(now, &d);
        let _ = link.poll(now);
        link.flush(now, &mut |out| assert!(out.len() <= MTU));
        let f = link.footprint();
        assert!(f.reliable_in_bytes <= 40 << 20 && f.partial_blobs <= 4 && f.unreliable_assemblies <= 9, "{f:?}");
    }
    // Still a working link afterwards: a clean peer with a fresh session is understood.
    let mut clean = Link::new(0xABCD, Role::Client);
    let mut fresh = Link::new(0xABCD, Role::Host);
    clean.send_reliable(b"still fine").unwrap();
    clean.flush(0, &mut |d| {
        fresh.on_datagram(1, d);
    });
    assert_eq!(fresh.poll(1), vec![Delivered::Reliable(b"still fine".to_vec())]);
}

#[test]
fn sequence_numbers_wrap_without_losing_messages() {
    // Start both directions a few datagrams below u32::MAX: acks, loss detection and the receive
    // window cross the wrap.
    let hub = MemoryHub::new(2);
    let mut p = Pair::new(&hub, 3, cond(30, 5, 0.05, 0.0), cond(30, 5, 0.05, 0.0));
    p.a.set_next_seq_for_test(u32::MAX - 40);
    p.b.set_next_seq_for_test(u32::MAX - 10);
    let mut sim = Sim { hub, pairs: vec![p], now: 0 };
    let mut n = 0u32;
    let mut hook = |_now: u64, pairs: &mut [Pair]| {
        pairs[0].a.send_reliable(&numbered(n, 200)).unwrap();
        pairs[0].b.send_reliable(&numbered(n, 100)).unwrap();
        n += 1;
    };
    sim.run_for(6_000, &mut hook);
    sim.run_for(3_000, &mut |_, _| {});
    assert_eq!(sim.pairs[0].reliable_b().len(), n as usize);
    assert_eq!(sim.pairs[0].reliable_a().len(), n as usize);
    assert!(sim.pairs[0].a.stats().datagrams_out > 200, "enough datagrams to cross the wrap");
    assert!(sim.pairs[0].a.is_idle() && sim.pairs[0].b.is_idle());
}

#[test]
fn udp_loopback_smoke_with_five_percent_loss() {
    let clock = WallClock::new();
    let mut ta = Conditioned::new(UdpTransport::bind("127.0.0.1:0").expect("bind"), 1, 0, cond(0, 0, 0.05, 0.0));
    let mut tb = Conditioned::new(UdpTransport::bind("127.0.0.1:0").expect("bind"), 1, 1, cond(0, 0, 0.05, 0.0));
    let addr_b = tb.inner().socket_addr().unwrap();
    let to_b = ta.inner_mut().connect(std::net::SocketAddr::from(([127, 0, 0, 1], addr_b.port())));
    let mut a = Link::new(0xFEED, Role::Client);
    let mut b = Link::new(0xFEED, Role::Host);
    let mut queued = 0u32;
    let mut next = 0u32;
    let mut peer_of_a = None;
    let mut buf = Vec::new();
    let started = std::time::Instant::now();
    while next < 1000 && started.elapsed().as_secs() < 5 {
        let now = clock.now_ms();
        // A few messages per iteration, so there are hundreds of datagrams for the 5 % to act on.
        for _ in 0..5 {
            if queued < 1000 {
                let mut m = queued.to_le_bytes().to_vec();
                m.extend_from_slice(&payload(queued as u64, 40));
                a.send_reliable(&m).unwrap();
                queued += 1;
            }
        }
        ta.recv(now, &mut buf);
        for (_, d) in buf.drain(..) {
            a.on_datagram(now, &d);
        }
        tb.recv(now, &mut buf);
        for (peer, d) in buf.drain(..) {
            peer_of_a = Some(peer);
            b.on_datagram(now, &d);
        }
        for d in b.poll(now) {
            let Delivered::Reliable(m) = d else { continue };
            assert_eq!(number_of(&m), next, "in order, exactly once");
            next += 1;
        }
        a.poll(now);
        a.flush(now, &mut |d| ta.send(to_b, d));
        if let Some(p) = peer_of_a {
            b.flush(now, &mut |d| tb.send(p, d));
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(next, 1000, "all 1000 messages through 5 % loss in {:?}", started.elapsed());
    let dropped = ta.egress_stats().dropped + tb.egress_stats().dropped;
    assert!(dropped > 0, "the simulation did drop datagrams");
    assert!(a.stats().resends > 0, "and the link resent what they carried: {:?}", a.stats());
}
