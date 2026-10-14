//! Estimators behind the link statistics: sequence order, smoothed RTT and the retransmission timer,
//! the receive window (duplicates, acks, loss), and the byte-rate meter.
//!
//! Everything here is integer arithmetic on the caller's clock, so runs are reproducible byte for byte.

/// `a` is newer than `b` on the wrapping 32-bit sequence circle.
pub(crate) fn seq_gt(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 0x8000_0000
}

/// `a` is older than `b`.
pub(crate) fn seq_lt(a: u32, b: u32) -> bool {
    seq_gt(b, a)
}

/// Smoothed RTT and variance in the fixed-point form of Jacobson's algorithm (`sa` = 8·srtt,
/// `sv` = 4·rttvar), so the retransmission timer is exact integer arithmetic.
#[derive(Clone, Debug, Default)]
pub(crate) struct Rtt {
    sa: i64,
    sv: i64,
    samples: u64,
}

impl Rtt {
    pub fn sample(&mut self, ms: u32) {
        let m = ms as i64;
        if self.samples == 0 {
            self.sa = m << 3;
            self.sv = m << 1;
        } else {
            let mut e = m - (self.sa >> 3);
            self.sa += e;
            if e < 0 {
                e = -e;
            }
            e -= self.sv >> 2;
            self.sv += e;
        }
        self.samples += 1;
    }

    /// `srtt + 2·rttvar + slack`: how long a datagram may be out before a later acknowledgement proves
    /// it lost. `None` before the first sample.
    pub fn loss_delay(&self, slack: u64) -> Option<u64> {
        (self.samples > 0).then(|| ((self.sa >> 3) + (self.sv >> 1)).max(0) as u64 + slack)
    }

    pub fn srtt_ms(&self) -> f32 {
        self.sa as f32 / 8.0
    }

    pub fn rttvar_ms(&self) -> f32 {
        self.sv as f32 / 4.0
    }

    /// `max(min, srtt + 4·rttvar + slack)`, `initial` before any sample, never above `max`.
    pub fn rto(&self, min: u64, max: u64, initial: u64, slack: u64) -> u64 {
        if self.samples == 0 {
            return initial.clamp(min, max);
        }
        let raw = ((self.sa >> 3) + self.sv).max(0) as u64 + slack;
        raw.clamp(min, max)
    }
}

const WINDOW_BITS: u32 = 256;
/// The newest datagrams are not counted as lost: reordering may still deliver them.
const REORDER_LAG: u32 = 6;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Classify {
    New,
    Duplicate,
    Stale,
}

/// Which datagram numbers arrived: the newest, and a 256-bit window below it. Feeds `ack`/`ack_bits`,
/// duplicate rejection and the loss statistics. Numbering starts at 1, so `hi` is also "how many the
/// peer has sent".
#[derive(Clone, Debug, Default)]
pub(crate) struct RecvWindow {
    started: bool,
    hi: u32,
    /// Bit k (counting from the least significant bit of word 0) = datagram `hi - k` arrived.
    bits: [u64; 4],
    seen: u64,
}

impl RecvWindow {
    fn bit(&self, k: u32) -> bool {
        self.bits[(k / 64) as usize] >> (k % 64) & 1 == 1
    }

    pub fn classify(&self, seq: u32) -> Classify {
        if !self.started || seq_gt(seq, self.hi) {
            return Classify::New;
        }
        let k = self.hi.wrapping_sub(seq);
        if k >= WINDOW_BITS {
            Classify::Stale
        } else if self.bit(k) {
            Classify::Duplicate
        } else {
            Classify::New
        }
    }

    /// Record a datagram that [`RecvWindow::classify`] called `New`.
    pub fn mark(&mut self, seq: u32) {
        self.seen += 1;
        if !self.started {
            self.started = true;
            self.hi = seq;
            self.bits = [1, 0, 0, 0];
            return;
        }
        if seq_gt(seq, self.hi) {
            let shift = seq.wrapping_sub(self.hi);
            shl256(&mut self.bits, shift);
            self.hi = seq;
            self.bits[0] |= 1;
        } else {
            let k = self.hi.wrapping_sub(seq);
            self.bits[(k / 64) as usize] |= 1 << (k % 64);
        }
    }

    /// `(ack, ack_bits)` for the next outgoing header: bit i of `ack_bits` = datagram `ack - 1 - i` arrived.
    pub fn ack(&self) -> (u32, u32) {
        let mut bits = 0u32;
        for i in 0..32 {
            if self.hi > i + 1 && self.bit(i + 1) {
                bits |= 1 << i;
            }
        }
        (self.hi, bits)
    }

    pub fn highest(&self) -> u32 {
        self.hi
    }

    /// Gaps over the last 256 datagrams, leaving out the newest few.
    pub fn loss_window(&self) -> f32 {
        let span = self.hi.min(WINDOW_BITS);
        if span <= REORDER_LAG {
            return 0.0;
        }
        let lost = (REORDER_LAG..span).filter(|&k| !self.bit(k)).count();
        lost as f32 / (span - REORDER_LAG) as f32
    }

    /// Gaps over everything the peer has sent.
    pub fn loss_lifetime(&self) -> f32 {
        if !self.started || self.hi == 0 {
            return 0.0;
        }
        let sent = self.hi as u64;
        sent.saturating_sub(self.seen) as f32 / sent as f32
    }
}

fn shl256(bits: &mut [u64; 4], n: u32) {
    if n >= WINDOW_BITS {
        *bits = [0; 4];
        return;
    }
    let words = (n / 64) as usize;
    let rem = n % 64;
    let mut out = [0u64; 4];
    for (i, slot) in out.iter_mut().enumerate().skip(words) {
        let src = i - words;
        let mut v = bits[src] << rem;
        if rem > 0 && src > 0 {
            v |= bits[src - 1] >> (64 - rem);
        }
        *slot = v;
    }
    *bits = out;
}

const METER_SLOTS: usize = 10;
const METER_SLOT_MS: u64 = 100;

/// Bytes per second over the last second, in ten 100 ms slots: the nine before the current one and the
/// current one up to `now`, so the window is 901..=1000 ms long and the sum is scaled to a full second.
#[derive(Clone, Debug, Default)]
pub(crate) struct RateMeter {
    slots: [(u64, u64, u64); METER_SLOTS],
}

impl RateMeter {
    fn slot(now: u64) -> u64 {
        now / METER_SLOT_MS + 1
    }

    pub fn add_in(&mut self, now: u64, bytes: usize) {
        let s = Self::slot(now);
        let e = &mut self.slots[(s % METER_SLOTS as u64) as usize];
        if e.0 != s {
            *e = (s, 0, 0);
        }
        e.1 += bytes as u64;
    }

    pub fn add_out(&mut self, now: u64, bytes: usize) {
        let s = Self::slot(now);
        let e = &mut self.slots[(s % METER_SLOTS as u64) as usize];
        if e.0 != s {
            *e = (s, 0, 0);
        }
        e.2 += bytes as u64;
    }

    /// `(in, out)` bytes per second as of `now`: the bytes of the window divided by the window's real
    /// length (the current slot is only as long as `now` has got into it).
    pub fn rates(&self, now: u64) -> (u32, u32) {
        let cur = Self::slot(now);
        let (mut i, mut o) = (0u64, 0u64);
        for &(s, a, b) in &self.slots {
            if s != 0 && s <= cur && cur - s < METER_SLOTS as u64 {
                i += a;
                o += b;
            }
        }
        let span = (METER_SLOTS as u128 - 1) * METER_SLOT_MS as u128 + (now % METER_SLOT_MS) as u128 + 1;
        let per_s = |bytes: u64| ((bytes as u128 * 1000 + span / 2) / span).min(u32::MAX as u128) as u32;
        (per_s(i), per_s(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_order_wraps() {
        assert!(seq_gt(5, 4));
        assert!(!seq_gt(4, 5));
        assert!(seq_gt(2, u32::MAX - 1));
        assert!(seq_lt(u32::MAX, 3));
        assert!(!seq_gt(7, 7));
    }

    #[test]
    fn window_dedupes_and_reports_acks() {
        let mut w = RecvWindow::default();
        for s in [1, 2, 4, 5] {
            assert_eq!(w.classify(s), Classify::New);
            w.mark(s);
        }
        assert_eq!(w.classify(4), Classify::Duplicate);
        assert_eq!(w.classify(3), Classify::New, "a gap can still be filled");
        let (ack, bits) = w.ack();
        assert_eq!(ack, 5);
        // 4 arrived (bit 0), 3 did not (bit 1), 2 and 1 did (bits 2, 3).
        assert_eq!(bits, 0b1101);
        w.mark(3);
        assert_eq!(w.ack().1, 0b1111);
        w.mark(400);
        assert_eq!(w.classify(100), Classify::Stale);
        assert_eq!(w.classify(400), Classify::Duplicate);
        assert_eq!(w.classify(399), Classify::New);
    }

    #[test]
    fn window_shift_crosses_words() {
        let mut w = RecvWindow::default();
        w.mark(1);
        w.mark(70);
        w.mark(200);
        assert_eq!(w.classify(70), Classify::Duplicate);
        assert_eq!(w.classify(1), Classify::Duplicate);
        assert_eq!(w.classify(2), Classify::New);
        w.mark(300);
        assert_eq!(w.classify(200), Classify::Duplicate);
        assert_eq!(w.classify(70), Classify::Duplicate);
        assert_eq!(w.classify(1), Classify::Stale, "300 - 1 is beyond the window");
    }

    #[test]
    fn rtt_converges_and_rto_has_floor() {
        let mut r = Rtt::default();
        assert_eq!(r.rto(100, 3000, 500, 0), 500);
        for _ in 0..50 {
            r.sample(300);
        }
        assert!((r.srtt_ms() - 300.0).abs() < 1.0);
        assert!(r.rttvar_ms() < 5.0);
        let rto = r.rto(100, 3000, 500, 0);
        assert!((300..330).contains(&rto), "rto {rto}");
        let mut fast = Rtt::default();
        for _ in 0..50 {
            fast.sample(2);
        }
        assert_eq!(fast.rto(100, 3000, 500, 0), 100);
    }

    #[test]
    fn rate_meter_forgets_old_slots() {
        let mut m = RateMeter::default();
        m.add_in(0, 1000);
        m.add_out(50, 500);
        // At 999 ms the window is exactly the first second.
        assert_eq!(m.rates(999), (1000, 500));
        m.add_in(900, 100);
        assert_eq!(m.rates(999), (1100, 500));
        assert_eq!(m.rates(2000), (0, 0));
        // A link that is only 60 ms old reads low, never as a spike: the window is a full second long.
        let mut young = RateMeter::default();
        young.add_in(0, 1000);
        let (i, _) = young.rates(60);
        assert!((900..=1100).contains(&i), "{i}");
    }

    #[test]
    fn a_steady_rate_reads_the_same_wherever_in_a_slot_it_is_asked() {
        // 1 byte in and 2 bytes out per millisecond is 1000 and 2000 bytes per second, at any instant.
        let mut m = RateMeter::default();
        let mut worst = (0i64, 0i64);
        for now in 0..5000u64 {
            m.add_in(now, 1);
            m.add_out(now, 2);
            if now >= 1000 {
                let (i, o) = m.rates(now);
                worst.0 = worst.0.max((i as i64 - 1000).abs());
                worst.1 = worst.1.max((o as i64 - 2000).abs());
                if matches!(now, 1000 | 1050 | 1099 | 1100 | 2999) {
                    assert!((i as i64 - 1000).abs() <= 1 && (o as i64 - 2000).abs() <= 2, "at {now}: {i} in, {o} out");
                }
            }
        }
        assert!(worst.0 <= 1 && worst.1 <= 2, "off by up to {worst:?} bytes per second");
    }

    #[test]
    fn loss_stats_count_gaps() {
        let mut w = RecvWindow::default();
        for s in 1..=100u32 {
            if s % 10 != 5 {
                w.mark(s);
            }
        }
        // 95 is among the newest few (not counted in the window), 5..85 are.
        assert!((w.loss_window() - 0.1).abs() < 0.03, "{}", w.loss_window());
        assert!((w.loss_lifetime() - 0.1).abs() < 0.001);
    }
}
