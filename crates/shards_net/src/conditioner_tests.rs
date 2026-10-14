use super::*;
use crate::hub::MemoryHub;

#[test]
fn spec_parses_and_prints_in_the_documented_form() {
    let c: LinkConditions = "latency=150,jitter=10,loss=0.02,dup=0".parse().unwrap();
    assert_eq!(c, LinkConditions { latency_ms: 150, jitter_ms: 10, loss: 0.02, duplicate: 0.0 });
    assert_eq!(c.to_string(), "latency=150,jitter=10,loss=0.02,dup=0");
    assert_eq!(c.to_string().parse::<LinkConditions>().unwrap(), c, "print then parse is the identity");
    assert_eq!(LinkConditions::NONE.to_string(), "latency=0,jitter=0,loss=0,dup=0");
    let partial: LinkConditions = "loss=0.1".parse().unwrap();
    assert_eq!(partial, LinkConditions { loss: 0.1, ..LinkConditions::NONE });
    let spaced: LinkConditions = " latency = 20 , dup = 0.5 ".parse().unwrap();
    assert_eq!((spaced.latency_ms, spaced.duplicate), (20, 0.5));
    for off in ["", "off", "none", "  "] {
        assert!(off.parse::<LinkConditions>().unwrap().is_none(), "{off:?}");
    }
}

#[test]
fn aliases_and_percentages() {
    let c: LinkConditions = "latency_ms=20ms,jitter_ms=3,loss=2%,duplicate=1.5%".parse().unwrap();
    assert_eq!(c.latency_ms, 20);
    assert_eq!(c.jitter_ms, 3);
    assert!((c.loss - 0.02).abs() < 1e-6);
    assert!((c.duplicate - 0.015).abs() < 1e-6);
}

#[test]
fn errors_name_the_bad_key() {
    let cases = [
        ("lag=5", "unknown key \"lag\""),
        ("latency=fast", "\"latency\""),
        ("latency=-5", "\"latency\""),
        ("loss=2", "\"loss\""),
        ("loss=150%", "\"loss\""),
        ("dup=lots", "\"dup\""),
        ("latency", "\"latency\" needs a value"),
        ("loss=0.1,loss=0.2", "\"loss\" is given twice"),
        ("latency=10,jitter=x", "\"jitter\""),
    ];
    for (spec, needle) in cases {
        let err = spec.parse::<LinkConditions>().unwrap_err();
        assert!(err.contains(needle), "{spec:?} -> {err}");
    }
    assert!(LinkConditions { loss: 1.5, ..LinkConditions::NONE }.validate().unwrap_err().contains("loss"));
    assert!(LinkConditions { duplicate: -0.1, ..LinkConditions::NONE }.validate().unwrap_err().contains("dup"));
    assert!(LinkConditions::NONE.validate().is_ok());
}

fn fates(seed: u64, c: LinkConditions, n: usize) -> Vec<Fate> {
    let mut k = Conditioner::new(seed, 3, 1, c);
    (0..n).map(|_| k.fate()).collect()
}

#[test]
fn fates_depend_only_on_seed_link_direction_and_packet_number() {
    let c = LinkConditions { latency_ms: 80, jitter_ms: 30, loss: 0.2, duplicate: 0.1 };
    assert_eq!(fates(5, c, 500), fates(5, c, 500));
    assert_ne!(fates(5, c, 500), fates(6, c, 500));
    let mut other_dir = Conditioner::new(5, 3, 0, c);
    assert_ne!((0..500).map(|_| other_dir.fate()).collect::<Vec<_>>(), fates(5, c, 500));
    // Changing one parameter does not reshuffle the others: the same packets are lost at any latency.
    let lost = |c: LinkConditions| fates(5, c, 500).iter().map(|f| f.delay_ms.is_none()).collect::<Vec<_>>();
    assert_eq!(lost(c), lost(LinkConditions { latency_ms: 0, jitter_ms: 0, ..c }));
    assert_eq!(lost(c), lost(LinkConditions { duplicate: 0.0, latency_ms: 500, ..c }));
}

#[test]
fn fates_follow_the_configured_rates() {
    for (loss, dup) in [(0.0, 0.0), (0.05, 0.0), (0.10, 0.05), (0.30, 0.10), (1.0, 0.0)] {
        let c = LinkConditions { latency_ms: 100, jitter_ms: 20, loss, duplicate: dup };
        let f = fates(11, c, 10_000);
        let lost = f.iter().filter(|f| f.delay_ms.is_none()).count() as f32 / 10_000.0;
        assert!((lost - loss).abs() < 0.015, "loss {lost} for {loss}");
        let delivered = f.iter().filter(|f| f.delay_ms.is_some()).count();
        let dups = f.iter().filter(|f| f.duplicate_delay_ms.is_some()).count();
        if delivered > 0 {
            let rate = dups as f32 / delivered as f32;
            assert!((rate - dup).abs() < 0.015, "duplication {rate} for {dup}");
        }
        assert!(f.iter().all(|f| f.delay_ms.is_some() || f.duplicate_delay_ms.is_none()), "a dropped packet has no duplicate");
        for d in f.iter().flat_map(|f| f.delay_ms.into_iter().chain(f.duplicate_delay_ms)) {
            assert!((80..=120).contains(&d), "delay {d} outside latency ± jitter");
        }
    }
    // Jitter larger than the latency never produces a negative delay.
    let wild = fates(1, LinkConditions { latency_ms: 5, jitter_ms: 50, ..LinkConditions::NONE }, 2000);
    assert!(wild.iter().all(|f| f.delay_ms.unwrap() <= 55));
    assert!(wild.iter().any(|f| f.delay_ms == Some(0)));
}

#[test]
fn jitter_spreads_delays_across_the_whole_range() {
    let f = fates(2, LinkConditions { latency_ms: 100, jitter_ms: 10, ..LinkConditions::NONE }, 5000);
    let seen: std::collections::BTreeSet<u32> = f.iter().filter_map(|f| f.delay_ms).collect();
    assert_eq!(seen.len(), 21, "every millisecond of 90..=110 occurs: {seen:?}");
}

fn drain<T: Transport>(t: &mut T, now: u64) -> Vec<(PeerId, Vec<u8>)> {
    let mut out = Vec::new();
    t.recv(now, &mut out);
    out
}

#[test]
fn conditioned_delays_by_the_callers_clock() {
    let hub = MemoryHub::new(1);
    let mut a = Conditioned::new(hub.endpoint("a"), 1, 0, LinkConditions { latency_ms: 100, ..LinkConditions::NONE });
    let mut b = hub.endpoint("b");
    hub.connect_both(a.inner().id(), b.id(), LinkConditions::NONE);
    let to = b.id();
    drain(&mut a, 1000); // the clock starts here
    a.send(to, b"late");
    assert_eq!(a.held(), 1);
    a.pump(1099);
    assert!(drain(&mut b, 0).is_empty(), "99 ms in: still held");
    a.pump(1100);
    assert_eq!(drain(&mut b, 0), vec![(a.inner().id(), b"late".to_vec())]);
    assert_eq!(a.held(), 0);
    assert_eq!(a.conditions().unwrap().latency_ms, 100);
    assert!(a.describe().contains("simulated latency=100"), "{}", a.describe());
    let s = a.egress_stats();
    assert_eq!((s.sent, s.delivered, s.dropped), (1, 1, 0));
}

#[test]
fn conditioned_without_conditions_passes_straight_through() {
    let hub = MemoryHub::new(1);
    let mut a = Conditioned::new(hub.endpoint("a"), 1, 0, LinkConditions::NONE);
    let mut b = hub.endpoint("b");
    hub.connect_both(a.inner().id(), b.id(), LinkConditions::NONE);
    a.send(b.id(), b"now");
    assert_eq!(a.held(), 0);
    assert_eq!(drain(&mut b, 0).len(), 1);
    assert_eq!(a.describe(), "hub a");
}

#[test]
fn conditioned_drops_and_duplicates_deterministically() {
    let run = |seed: u64| {
        let hub = MemoryHub::new(1);
        let mut a = Conditioned::new(hub.endpoint("a"), seed, 0, LinkConditions { loss: 0.3, duplicate: 0.2, ..LinkConditions::NONE });
        let mut b = hub.endpoint("b");
        hub.connect_both(a.inner().id(), b.id(), LinkConditions::NONE);
        let mut got = Vec::new();
        for i in 0..2000u32 {
            a.send(b.id(), &i.to_le_bytes());
            for (_, d) in drain(&mut b, 0) {
                got.push(u32::from_le_bytes([d[0], d[1], d[2], d[3]]));
            }
        }
        (got, a.egress_stats())
    };
    let (one, s1) = run(4);
    let (two, _) = run(4);
    assert_eq!(one, two);
    assert_ne!(one, run(5).0);
    assert!((s1.loss() - 0.3).abs() < 0.04, "{}", s1.loss());
    assert!(s1.duplicated > 100 && s1.delivered == s1.sent - s1.dropped + s1.duplicated, "{s1:?}");
}

#[test]
fn conditioned_ingress_delays_what_arrives() {
    let hub = MemoryHub::new(1);
    let mut a = hub.endpoint("a");
    let mut b = Conditioned::new(hub.endpoint("b"), 1, 1, LinkConditions::NONE);
    b.set_ingress(LinkConditions { latency_ms: 50, ..LinkConditions::NONE });
    hub.connect_both(a.id(), b.inner().id(), LinkConditions::NONE);
    a.send(b.inner().id(), b"x");
    assert!(drain(&mut b, 10).is_empty());
    assert!(drain(&mut b, 59).is_empty());
    assert_eq!(drain(&mut b, 60).len(), 1);
    assert_eq!(b.ingress_stats().delivered, 1);
}

#[test]
fn per_peer_overrides_and_set_for_all() {
    let hub = MemoryHub::new(1);
    let mut a = Conditioned::new(hub.endpoint("a"), 1, 0, LinkConditions::NONE);
    let (mut b, mut c) = (hub.endpoint("b"), hub.endpoint("c"));
    hub.connect_both(a.inner().id(), b.id(), LinkConditions::NONE);
    hub.connect_both(a.inner().id(), c.id(), LinkConditions::NONE);
    let slow = LinkConditions { latency_ms: 200, ..LinkConditions::NONE };
    assert!(a.set_conditions(Some(b.id()), slow));
    drain(&mut a, 0);
    a.send(b.id(), b"b");
    a.send(c.id(), b"c");
    assert_eq!(drain(&mut c, 0).len(), 1, "c follows the default (none)");
    assert!(drain(&mut b, 0).is_empty());
    a.pump(200);
    assert_eq!(drain(&mut b, 0).len(), 1);
    // Setting for all links takes the override away.
    assert!(a.set_conditions(None, LinkConditions { latency_ms: 10, ..LinkConditions::NONE }));
    a.send(b.id(), b"b2");
    a.pump(205);
    assert!(drain(&mut b, 0).is_empty());
    a.pump(210);
    assert_eq!(drain(&mut b, 0).len(), 1);
    a.forget(b.id());
}

#[test]
fn boxed_transports_forward_everything() {
    let hub = MemoryHub::new(1);
    let mut a: Box<dyn Transport> = Box::new(Conditioned::new(hub.endpoint("a"), 1, 0, LinkConditions::NONE));
    let mut b = hub.endpoint("b");
    hub.connect_both(0, b.id(), LinkConditions::NONE);
    assert!(a.set_conditions(None, LinkConditions { latency_ms: 30, ..LinkConditions::NONE }));
    assert_eq!(a.conditions().unwrap().latency_ms, 30);
    a.send(b.id(), b"boxed");
    let mut out = Vec::new();
    a.recv(30, &mut out);
    assert_eq!(drain(&mut b, 0).len(), 1);
}
