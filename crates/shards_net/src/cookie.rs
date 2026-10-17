//! The cookie of a join: what a host answers a knock with, and what a client's first datagrams
//! must carry before the host keeps anything of it.
//!
//! A host that kept a link for every address a datagram claims to come from could be made to hold
//! strangers that never were (a spoofed address costs nothing to send from). So a join starts with
//! a round trip that leaves no state on the host: the client knocks ([`wire::knock`]), the host
//! answers with a cookie ([`wire::challenge`]), and only a datagram that carries a cookie the host
//! made for that address and that session, not long ago, makes a peer. The cookie is a keyed MAC
//! (SipHash-2-4, the host's random 128-bit key) of the sender, the session the client chose, the
//! wire protocol and a coarse time bucket ([`BUCKET_MS`]): checking one is recomputing it, and a
//! cookie is good in the bucket it was made in and the next one.
//!
//! [`wire::knock`]: crate::wire::knock
//! [`wire::challenge`]: crate::wire::challenge

use crate::transport::PeerId;
use crate::wire;

/// Length of a time bucket, milliseconds on the session's clock. A cookie is good in the bucket it
/// was made in and the next: from 30 to 60 seconds.
pub const BUCKET_MS: u64 = 30_000;

/// The host's key. Random (from the OS) in every host process; it never leaves the process, is
/// not in snapshots or replays, and its `Debug` does not show it.
pub struct Cookies {
    key: (u64, u64),
}

impl std::fmt::Debug for Cookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cookies { key: <secret> }")
    }
}

impl Default for Cookies {
    fn default() -> Self {
        Self::random()
    }
}

impl Cookies {
    pub fn random() -> Self {
        Self { key: (crate::host::random_word(0xC0, 1), crate::host::random_word(0xC1, 2)) }
    }

    #[cfg(test)]
    pub(crate) fn with_key(k0: u64, k1: u64) -> Self {
        Self { key: (k0, k1) }
    }

    fn mac(&self, peer: PeerId, session: u32, bucket: u64) -> u64 {
        let mut data = [0u8; 21];
        data[..8].copy_from_slice(&peer.to_le_bytes());
        data[8..12].copy_from_slice(&session.to_le_bytes());
        data[12] = wire::PROTO;
        data[13..].copy_from_slice(&bucket.to_le_bytes());
        siphash24(self.key.0, self.key.1, &data)
    }

    /// The cookie for `peer`, whose link has session `session`, at `now` (milliseconds).
    pub fn issue(&self, peer: PeerId, session: u32, now: u64) -> u64 {
        self.mac(peer, session, now / BUCKET_MS)
    }

    /// Whether `cookie` is one this host made for `peer` and `session` in this bucket or the one
    /// before.
    pub fn check(&self, peer: PeerId, session: u32, now: u64, cookie: u64) -> bool {
        let bucket = now / BUCKET_MS;
        // Not a short-circuit on the first bucket: both are always computed.
        let fresh = self.mac(peer, session, bucket) == cookie;
        let recent = bucket > 0 && self.mac(peer, session, bucket - 1) == cookie;
        fresh | recent
    }
}

/// SipHash-2-4 of `data` under the key `(k0, k1)` (Aumasson and Bernstein, 2012): a PRF made for
/// short inputs, so a MAC of them.
pub(crate) fn siphash24(k0: u64, k1: u64, data: &[u8]) -> u64 {
    let mut v = [0x736f_6d65_7073_6575 ^ k0, 0x646f_7261_6e64_6f6d ^ k1, 0x6c79_6765_6e65_7261 ^ k0, 0x7465_6462_7974_6573 ^ k1];
    let round = |v: &mut [u64; 4]| {
        v[0] = v[0].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(13) ^ v[0];
        v[0] = v[0].rotate_left(32);
        v[2] = v[2].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(16) ^ v[2];
        v[0] = v[0].wrapping_add(v[3]);
        v[3] = v[3].rotate_left(21) ^ v[0];
        v[2] = v[2].wrapping_add(v[1]);
        v[1] = v[1].rotate_left(17) ^ v[2];
        v[2] = v[2].rotate_left(32);
    };
    let (blocks, rest) = data.as_chunks::<8>();
    for block in blocks {
        let m = u64::from_le_bytes(*block);
        v[3] ^= m;
        round(&mut v);
        round(&mut v);
        v[0] ^= m;
    }
    let mut last = (data.len() as u64 & 0xff) << 56;
    for (i, byte) in rest.iter().enumerate() {
        last |= u64::from(*byte) << (8 * i);
    }
    v[3] ^= last;
    round(&mut v);
    round(&mut v);
    v[0] ^= last;
    v[2] ^= 0xff;
    for _ in 0..4 {
        round(&mut v);
    }
    v[0] ^ v[1] ^ v[2] ^ v[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key of the reference vectors: bytes 0 to 15.
    const K0: u64 = 0x0706_0504_0302_0100;
    const K1: u64 = 0x0f0e_0d0c_0b0a_0908;

    #[test]
    fn siphash_gives_the_reference_vectors() {
        // From the SipHash paper and its reference implementation: key 00..0f, message 00..(n-1).
        let message: Vec<u8> = (0..64u8).collect();
        assert_eq!(siphash24(K0, K1, &message[..0]), 0x726f_db47_dd0e_0e31);
        assert_eq!(siphash24(K0, K1, &message[..1]), 0x74f8_39c5_93dc_67fd);
        assert_eq!(siphash24(K0, K1, &message[..15]), 0xa129_ca61_49be_45e5);
        // And std's own SipHash-2-4 (deprecated as a hasher, kept as a reference here) on every length.
        for n in 0..64 {
            #[allow(deprecated)]
            let mut std = std::hash::SipHasher::new_with_keys(K0, K1);
            std::hash::Hasher::write(&mut std, &message[..n]);
            assert_eq!(siphash24(K0, K1, &message[..n]), std::hash::Hasher::finish(&std), "{n} bytes");
        }
    }

    #[test]
    fn a_cookie_is_good_for_its_peer_and_session_in_its_bucket_and_the_next() {
        let cookies = Cookies::with_key(1, 2);
        let made = cookies.issue(7, 99, 10_000);
        assert!(cookies.check(7, 99, 10_000, made));
        assert!(cookies.check(7, 99, BUCKET_MS - 1, made), "the same bucket");
        assert!(cookies.check(7, 99, 2 * BUCKET_MS - 1, made), "the next one");
        assert!(!cookies.check(7, 99, 2 * BUCKET_MS, made), "expired: two buckets on");
        assert!(!cookies.check(8, 99, 10_000, made), "another address");
        assert!(!cookies.check(7, 98, 10_000, made), "another session");
        assert!(!cookies.check(7, 99, 10_000, made ^ 1), "forged");
        assert!(!Cookies::with_key(1, 3).check(7, 99, 10_000, made), "another host's key");
        // Made at the very start of the clock: no bucket before it.
        assert!(Cookies::with_key(1, 2).check(7, 99, 0, cookies.issue(7, 99, 0)));
    }

    #[test]
    fn the_key_is_random_and_never_printed() {
        let (a, b) = (Cookies::random(), Cookies::random());
        assert_ne!(a.issue(1, 1, 0), b.issue(1, 1, 0), "two hosts, two keys");
        assert_eq!(format!("{a:?}"), "Cookies { key: <secret> }");
    }
}
