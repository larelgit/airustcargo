//! The datagram format shared by every transport.
//!
//! ```text
//! header (20 bytes, little endian)
//!   magic "SH" | proto u8 | flags u8 | session u32 | seq u32 | ack u32 | ack_bits u32
//! body (deflated as a whole when the flag says so)
//!   time_ms u32 | time_echo u32 | reliable_ack u32
//!   n_unreliable varint, then n × message
//!   n_reliable   varint, then n × (rseq u32, message)
//!   [fragment: msg u32 | index u16 | count u16 | len varint | bytes]        (flags: FRAG)
//! message = varint(len << 1 | deflated) | bytes
//!
//! the join's round trip (no body but the knock's padding; see `cookie`):
//!   knock     (client → host): flags KNOCK, the client's session, zeros to KNOCK_LEN bytes
//!   challenge (host → client): flags KNOCK | COOKIE, the session, the cookie in ack | ack_bits
//! ```
//!
//! A client's datagrams carry the host's cookie (flag COOKIE) in `ack` and `ack_bits` while it has
//! received nothing from the host, which is exactly when those fields say nothing. Wire protocol 1
//! had neither flag; a host answers a link datagram of protocol 1 with a refusal in protocol 1
//! ([`older_build`], [`as_version`]).
//!
//! The body is length-prefixed binary: this layer carries opaque payloads (the session layer
//! serializes its own messages). A body over 256 bytes is deflated when that makes it smaller;
//! messages too big for a datagram are deflated once, before they are fragmented (see `link`).
//! A datagram with a foreign magic, protocol or session is dropped and counted by the link.
//!
//! Inflating is budgeted (`Budget`): a datagram may make the receiver produce `INFLATE_BUDGET` (2 MiB) bytes
//! in all, its body and its inline messages together. A sender never gets near that: a body is at most
//! [`MAX_BODY`] bytes before it is deflated, and deflate expands a byte at most 1032-fold, so the most an
//! honest datagram inflates to is about 1.2 MB.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;

use crate::transport::MTU;

pub const MAGIC: [u8; 2] = *b"SH";
/// Wire protocol version; a datagram of another version is rejected. 2: the join's cookie.
pub const PROTO: u8 = 2;
/// The older version a host still recognises, to refuse it in words its client reads.
pub const PROTO_1: u8 = 1;
pub const HEADER_LEN: usize = 20;
/// Bytes of a knock: at least those of the challenge that answers it, so a host never sends more
/// than it was sent (a spoofed knock cannot make it amplify a flood).
pub const KNOCK_LEN: usize = 64;
/// Bytes a body may take.
pub const MAX_BODY: usize = MTU - HEADER_LEN;
/// Bodies above this size are deflated (when it helps).
pub const DEFLATE_MIN: usize = 256;
/// What inflating one datagram may produce in total, body and inline messages together. The most an
/// honest datagram can need is `MAX_BODY × 1032` (deflate's best ratio) = 1.2 MB.
pub(crate) const INFLATE_BUDGET: usize = 2 << 20;

/// Header flag bits.
pub mod flag {
    /// The body is a raw deflate stream.
    pub const DEFLATED: u8 = 1;
    /// A fragment follows the messages.
    pub const FRAG: u8 = 2;
    /// The fragment belongs to a reliable blob (otherwise to an unreliable message).
    pub const FRAG_RELIABLE: u8 = 4;
    /// `time_echo` is valid.
    pub const ECHO: u8 = 8;
    /// The fragmented message is deflated as a whole.
    pub const FRAG_DEFLATED: u8 = 16;
    /// A client's datagram: `ack` and `ack_bits` carry the host's cookie (it has received nothing).
    /// With `KNOCK`: the host's challenge.
    pub const COOKIE: u8 = 32;
    /// Not a link datagram: a knock (client → host), or with `COOKIE` the challenge answering it.
    pub const KNOCK: u8 = 64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub flags: u8,
    pub session: u32,
    pub seq: u32,
    pub ack: u32,
    pub ack_bits: u32,
    /// The host's cookie a client's datagram carries in place of its acknowledgements (it has
    /// received nothing yet): `ack` and `ack_bits` read as 0.
    pub cookie: Option<u64>,
}

/// Why a datagram was dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    /// Shorter than a header.
    TooShort,
    /// Not ours: the first two bytes are not "SH".
    Magic,
    /// Another wire protocol version.
    Proto(u8),
    /// Another session; carries the session the datagram claims.
    Session(u32),
    /// The body does not decode.
    Malformed,
}

impl Header {
    fn write(&self, out: &mut Vec<u8>) {
        let (ack, ack_bits) = self.cookie.map_or((self.ack, self.ack_bits), |c| (c as u32, (c >> 32) as u32));
        out.extend_from_slice(&MAGIC);
        out.push(PROTO);
        out.push(self.flags);
        out.extend_from_slice(&self.session.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&ack.to_le_bytes());
        out.extend_from_slice(&ack_bits.to_le_bytes());
    }
}

/// Read the header of a datagram without touching the body: magic and protocol are checked, the
/// session is not (the caller knows which sessions it accepts). A host uses this to learn the session
/// of the first datagram from an unknown peer before it creates a link for it. A client's cookie is
/// taken out of the acknowledgements ([`Header::cookie`]).
pub fn peek(datagram: &[u8]) -> Result<Header, Reject> {
    if datagram.len() < HEADER_LEN {
        return Err(Reject::TooShort);
    }
    if datagram[..2] != MAGIC {
        return Err(Reject::Magic);
    }
    if datagram[2] != PROTO {
        return Err(Reject::Proto(datagram[2]));
    }
    let u = |at: usize| u32::from_le_bytes([datagram[at], datagram[at + 1], datagram[at + 2], datagram[at + 3]]);
    let (flags, ack, ack_bits) = (datagram[3], u(12), u(16));
    if flags & flag::COOKIE != 0 && flags & flag::KNOCK == 0 {
        let cookie = u64::from(ack) | u64::from(ack_bits) << 32;
        return Ok(Header { flags, session: u(4), seq: u(8), ack: 0, ack_bits: 0, cookie: Some(cookie) });
    }
    Ok(Header { flags, session: u(4), seq: u(8), ack, ack_bits, cookie: None })
}

/// The session a datagram claims, `None` for a datagram that is not ours.
pub fn peek_session(datagram: &[u8]) -> Option<u32> {
    peek(datagram).ok().map(|h| h.session)
}

/// Whether a datagram starts like one of ours, of any version: a stranger worth reading.
pub fn is_ours(datagram: &[u8]) -> bool {
    datagram.len() >= HEADER_LEN && datagram[..2] == MAGIC
}

/// One datagram of the join's round trip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handshake {
    /// A client asks for a cookie for its session.
    Knock { session: u32 },
    /// The host's cookie for that session.
    Challenge { session: u32, cookie: u64 },
}

/// A knock: a client of `session` asks the host for a cookie. Padded to [`KNOCK_LEN`].
pub fn knock(session: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(KNOCK_LEN);
    Header { flags: flag::KNOCK, session, seq: 0, ack: 0, ack_bits: 0, cookie: None }.write(&mut out);
    out.resize(KNOCK_LEN, 0);
    out
}

/// The host's answer to a knock: `cookie` for `session`, [`HEADER_LEN`] bytes.
pub fn challenge(session: u32, cookie: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    Header { flags: flag::KNOCK | flag::COOKIE, session, seq: 0, ack: 0, ack_bits: 0, cookie: Some(cookie) }.write(&mut out);
    out
}

/// What a datagram of the join's round trip says, `None` for anything else (a link datagram, junk,
/// a knock of the wrong length).
pub fn handshake(datagram: &[u8]) -> Option<Handshake> {
    let header = peek(datagram).ok()?;
    if header.flags == flag::KNOCK && datagram.len() == KNOCK_LEN {
        return Some(Handshake::Knock { session: header.session });
    }
    if header.flags == flag::KNOCK | flag::COOKIE && datagram.len() == HEADER_LEN {
        return Some(Handshake::Challenge { session: header.session, cookie: u64::from(header.ack) | u64::from(header.ack_bits) << 32 });
    }
    None
}

/// The session of a link datagram of wire protocol 1 that carries a message: a client of an older
/// build saying hello. Its format is this one's without the join's flags.
pub fn older_build(datagram: &[u8]) -> Option<u32> {
    if !is_ours(datagram) || datagram[2] != PROTO_1 || datagram[3] & (flag::COOKIE | flag::KNOCK) != 0 {
        return None;
    }
    let current = as_version(datagram, PROTO);
    let (header, body) = decode_with(&current, &mut Budget::new()).ok()?;
    body.has_payload().then_some(header.session)
}

/// `datagram` with its version byte set to `proto`: a link datagram without the join's flags, in
/// the words of protocol 1.
pub fn as_version(datagram: &[u8], proto: u8) -> Vec<u8> {
    let mut out = datagram.to_vec();
    if out.len() >= HEADER_LEN {
        out[2] = proto;
    }
    out
}

/// Whether a datagram carries a message (or a piece of one), not just acks and the clock. A
/// host makes a link for an unknown address only for this: a game that ended keeps acking and
/// keeping its link alive for a while, and that alone is nobody joining.
pub fn carries_messages(datagram: &[u8]) -> bool {
    decode_with(datagram, &mut Budget::new()).is_ok_and(|(_, body)| body.has_payload())
}

/// What is left of the inflate budget of one datagram: [`decode_with`] takes the body's share, the link
/// takes each deflated inline message's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    left: usize,
}

impl Budget {
    pub fn new() -> Self {
        Self { left: INFLATE_BUDGET }
    }

    /// Inflate `data`, at most `max` bytes and at most what is left; `None` over either.
    pub fn inflate(&mut self, data: &[u8], max: usize) -> Option<Vec<u8>> {
        let out = inflate(data, max.min(self.left))?;
        self.left -= out.len();
        Some(out)
    }
}

/// One message inside a datagram: its bytes, possibly deflated by the link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Msg {
    pub data: Vec<u8>,
    pub deflated: bool,
}

/// One piece of a message too large for a datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fragment {
    pub reliable: bool,
    pub deflated: bool,
    /// Reliable: the blob's `rseq`. Unreliable: the message counter.
    pub msg: u32,
    pub index: u16,
    pub count: u16,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Body {
    pub time_ms: u32,
    pub time_echo: Option<u32>,
    pub unreliable: Vec<Msg>,
    pub reliable: Vec<(u32, Msg)>,
    pub reliable_ack: u32,
    pub frag: Option<Fragment>,
}

impl Body {
    pub fn has_payload(&self) -> bool {
        !self.unreliable.is_empty() || !self.reliable.is_empty() || self.frag.is_some()
    }
}

/// Encoded size of a varint.
pub(crate) fn varint_len(v: u32) -> usize {
    match v {
        0..=0x7F => 1,
        0x80..=0x3FFF => 2,
        0x4000..=0x1F_FFFF => 3,
        0x20_0000..=0xFFF_FFFF => 4,
        _ => 5,
    }
}

fn put_varint(out: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        out.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Bytes a message takes inside a body: its length prefix and itself.
pub(crate) fn msg_cost(len: usize) -> usize {
    varint_len((len as u32) << 1) + len
}

fn put_msg(out: &mut Vec<u8>, m: &Msg) {
    put_varint(out, ((m.data.len() as u32) << 1) | m.deflated as u32);
    out.extend_from_slice(&m.data);
}

/// Serialize `header` + `body`. The header's flags are computed here (`header.flags` is ignored).
/// A body over [`DEFLATE_MIN`] bytes with no fragment is deflated when that makes it smaller.
pub(crate) fn encode(header: &Header, body: &Body) -> Vec<u8> {
    let mut payload = Vec::with_capacity(64);
    payload.extend_from_slice(&body.time_ms.to_le_bytes());
    payload.extend_from_slice(&body.time_echo.unwrap_or(0).to_le_bytes());
    payload.extend_from_slice(&body.reliable_ack.to_le_bytes());
    put_varint(&mut payload, body.unreliable.len() as u32);
    for m in &body.unreliable {
        put_msg(&mut payload, m);
    }
    put_varint(&mut payload, body.reliable.len() as u32);
    for (rseq, m) in &body.reliable {
        payload.extend_from_slice(&rseq.to_le_bytes());
        put_msg(&mut payload, m);
    }
    let mut flags = 0u8;
    if body.time_echo.is_some() {
        flags |= flag::ECHO;
    }
    if header.cookie.is_some() {
        flags |= flag::COOKIE;
    }
    if let Some(f) = &body.frag {
        flags |= flag::FRAG;
        if f.reliable {
            flags |= flag::FRAG_RELIABLE;
        }
        if f.deflated {
            flags |= flag::FRAG_DEFLATED;
        }
        payload.extend_from_slice(&f.msg.to_le_bytes());
        payload.extend_from_slice(&f.index.to_le_bytes());
        payload.extend_from_slice(&f.count.to_le_bytes());
        put_varint(&mut payload, f.bytes.len() as u32);
        payload.extend_from_slice(&f.bytes);
    } else if payload.len() > DEFLATE_MIN
        && let Some(z) = deflate(&payload, Compression::fast())
        && z.len() < payload.len()
    {
        payload = z;
        flags |= flag::DEFLATED;
    }
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    Header { flags, ..*header }.write(&mut out);
    out.extend_from_slice(&payload);
    out
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Reject> {
        if n > self.remaining() {
            return Err(Reject::Malformed);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u16(&mut self) -> Result<u16, Reject> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, Reject> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn varint(&mut self) -> Result<u32, Reject> {
        let mut v = 0u32;
        for shift in (0..35).step_by(7) {
            let b = self.take(1)?[0];
            v |= ((b & 0x7F) as u32).checked_shl(shift).ok_or(Reject::Malformed)?;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(Reject::Malformed)
    }

    fn msg(&mut self) -> Result<Msg, Reject> {
        let v = self.varint()?;
        let data = self.take((v >> 1) as usize)?.to_vec();
        Ok(Msg { data, deflated: v & 1 == 1 })
    }
}

/// Parse a whole datagram with a fresh [`Budget`].
#[cfg(test)]
pub(crate) fn decode(datagram: &[u8]) -> Result<(Header, Body), Reject> {
    decode_with(datagram, &mut Budget::new())
}

/// Parse a whole datagram. Magic and protocol are checked; the session is the link's business. Inflating
/// the body takes from `budget`, and a body inflates to at most [`MAX_BODY`] bytes.
pub(crate) fn decode_with(datagram: &[u8], budget: &mut Budget) -> Result<(Header, Body), Reject> {
    let header = peek(datagram)?;
    if header.flags & flag::KNOCK != 0 {
        // The join's round trip is not a link's business.
        return Err(Reject::Malformed);
    }
    let raw = &datagram[HEADER_LEN..];
    let inflated;
    let payload = if header.flags & flag::DEFLATED != 0 {
        inflated = budget.inflate(raw, MAX_BODY).ok_or(Reject::Malformed)?;
        &inflated[..]
    } else {
        raw
    };
    let mut r = Reader { buf: payload, pos: 0 };
    let time_ms = r.u32()?;
    let echo = r.u32()?;
    let reliable_ack = r.u32()?;
    let n_unreliable = r.varint()? as usize;
    // Every message takes at least one byte: a count above what is left is a lie, not an allocation.
    if n_unreliable > r.remaining() {
        return Err(Reject::Malformed);
    }
    let mut unreliable = Vec::with_capacity(n_unreliable);
    for _ in 0..n_unreliable {
        unreliable.push(r.msg()?);
    }
    let n_reliable = r.varint()? as usize;
    if n_reliable > r.remaining() / 5 {
        return Err(Reject::Malformed);
    }
    let mut reliable = Vec::with_capacity(n_reliable);
    for _ in 0..n_reliable {
        let rseq = r.u32()?;
        reliable.push((rseq, r.msg()?));
    }
    let frag = if header.flags & flag::FRAG != 0 {
        let msg = r.u32()?;
        let index = r.u16()?;
        let count = r.u16()?;
        let len = r.varint()? as usize;
        let bytes = r.take(len)?.to_vec();
        if count == 0 || index >= count {
            return Err(Reject::Malformed);
        }
        Some(Fragment {
            reliable: header.flags & flag::FRAG_RELIABLE != 0,
            deflated: header.flags & flag::FRAG_DEFLATED != 0,
            msg,
            index,
            count,
            bytes,
        })
    } else {
        None
    };
    if r.remaining() != 0 {
        return Err(Reject::Malformed);
    }
    let time_echo = (header.flags & flag::ECHO != 0).then_some(echo);
    Ok((header, Body { time_ms, time_echo, unreliable, reliable, reliable_ack, frag }))
}

/// Raw deflate. `None` only if the encoder fails, which writing to a `Vec` does not do.
pub(crate) fn deflate(data: &[u8], level: Compression) -> Option<Vec<u8>> {
    let mut enc = DeflateEncoder::new(Vec::with_capacity(data.len() / 2 + 16), level);
    enc.write_all(data).ok()?;
    enc.finish().ok()
}

/// Raw inflate, refusing to produce more than `max` bytes (a decompression bomb is a rejection).
pub(crate) fn inflate(data: &[u8], max: usize) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    DeflateDecoder::new(data).take(max as u64 + 1).read_to_end(&mut out).ok()?;
    (out.len() <= max).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Header {
        Header { flags: 0, session: 0xDEAD_BEEF, seq: 77, ack: 70, ack_bits: 0b1011, cookie: None }
    }

    fn full_body() -> Body {
        Body {
            time_ms: 123_456,
            time_echo: Some(123_000),
            unreliable: vec![Msg { data: b"state".to_vec(), deflated: false }, Msg { data: vec![], deflated: false }],
            reliable: vec![(9, Msg { data: b"hello".to_vec(), deflated: false }), (10, Msg { data: vec![1, 2, 3], deflated: true })],
            reliable_ack: 8,
            frag: Some(Fragment { reliable: true, deflated: true, msg: 10, index: 2, count: 5, bytes: vec![7; 40] }),
        }
    }

    #[test]
    fn header_and_body_round_trip() {
        let body = full_body();
        let bytes = encode(&header(), &body);
        assert!(bytes.len() <= MTU);
        let (h, b) = decode(&bytes).unwrap();
        assert_eq!(h.session, 0xDEAD_BEEF);
        assert_eq!((h.seq, h.ack, h.ack_bits), (77, 70, 0b1011));
        assert_eq!(b, body);
        assert_eq!(peek_session(&bytes), Some(0xDEAD_BEEF));
        assert_eq!(&bytes[..2], b"SH");
        assert_eq!(bytes[2], PROTO);
    }

    #[test]
    fn header_is_twenty_bytes_and_an_empty_body_is_small() {
        let bytes = encode(&header(), &Body::default());
        // header 20 + time 4 + echo 4 + reliable_ack 4 + two zero counts.
        assert_eq!(bytes.len(), HEADER_LEN + 14);
        let (_, b) = decode(&bytes).unwrap();
        assert!(!b.has_payload());
        assert_eq!(b.time_echo, None);
    }

    #[test]
    fn only_a_datagram_with_a_message_is_someone_talking() {
        assert!(!carries_messages(&encode(&header(), &Body::default())), "a keepalive");
        let acks = Body { time_echo: Some(5), reliable_ack: 9, ..Body::default() };
        assert!(!carries_messages(&encode(&header(), &acks)), "acks and the clock");
        assert!(carries_messages(&encode(&header(), &full_body())));
        let hello = Body { reliable: vec![(1, Msg { data: b"hello".to_vec(), deflated: false })], ..Body::default() };
        assert!(carries_messages(&encode(&header(), &hello)));
        let mut cut = encode(&header(), &hello);
        cut.truncate(cut.len() - 1);
        assert!(!carries_messages(&cut), "a body that does not decode");
        assert!(!carries_messages(b"SH"), "shorter than a header");
    }

    #[test]
    fn big_compressible_body_is_deflated() {
        let big = Msg { data: vec![b'a'; 800], deflated: false };
        let body = Body { reliable: vec![(1, big.clone())], ..Body::default() };
        let bytes = encode(&header(), &body);
        assert!(bytes.len() < 200, "800 equal bytes deflate to almost nothing, got {}", bytes.len());
        assert_ne!(bytes[3] & flag::DEFLATED, 0);
        let (_, back) = decode(&bytes).unwrap();
        assert_eq!(back.reliable[0].1, big);
    }

    #[test]
    fn small_and_incompressible_bodies_stay_raw() {
        let small = Body { unreliable: vec![Msg { data: b"tiny".to_vec(), deflated: false }], ..Body::default() };
        assert_eq!(encode(&header(), &small)[3] & flag::DEFLATED, 0);
        let mut x = 12345u32;
        let noise: Vec<u8> = (0..900)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        let body = Body { unreliable: vec![Msg { data: noise, deflated: false }], ..Body::default() };
        let bytes = encode(&header(), &body);
        assert_eq!(bytes[3] & flag::DEFLATED, 0, "deflate that does not shrink is not used");
        assert_eq!(decode(&bytes).unwrap().1, body);
    }

    #[test]
    fn a_fragment_body_is_never_deflated() {
        let frag = Fragment { reliable: false, deflated: false, msg: 1, index: 0, count: 2, bytes: vec![0; 1000] };
        let bytes = encode(&header(), &Body { frag: Some(frag), ..Body::default() });
        assert_eq!(bytes[3] & flag::DEFLATED, 0);
        assert_ne!(bytes[3] & flag::FRAG, 0);
        assert_eq!(bytes[3] & flag::FRAG_RELIABLE, 0);
    }

    #[test]
    fn foreign_datagrams_are_rejected() {
        let good = encode(&header(), &full_body());
        let mut bad_magic = good.clone();
        bad_magic[0] = b'X';
        assert_eq!(decode(&bad_magic).unwrap_err(), Reject::Magic);
        let mut bad_proto = good.clone();
        bad_proto[2] = PROTO + 1;
        assert_eq!(decode(&bad_proto).unwrap_err(), Reject::Proto(PROTO + 1));
        assert_eq!(decode(&good[..10]).unwrap_err(), Reject::TooShort);
        assert_eq!(peek_session(b"GET / HTTP/1.1\r\n\r\n....."), None);
        assert_eq!(peek_session(&bad_magic), None);
    }

    #[test]
    fn truncated_and_lying_bodies_are_malformed_not_panics() {
        let good = encode(&header(), &full_body());
        for cut in HEADER_LEN..good.len() {
            let r = decode(&good[..cut]);
            assert_eq!(r.unwrap_err(), Reject::Malformed, "cut at {cut}");
        }
        let mut trailing = good.clone();
        trailing.push(0);
        assert_eq!(decode(&trailing).unwrap_err(), Reject::Malformed);
        // A count that promises more messages than there are bytes.
        let mut lie = Vec::new();
        Header { flags: 0, session: 1, seq: 1, ack: 0, ack_bits: 0, cookie: None }.write(&mut lie);
        lie.extend_from_slice(&[0; 12]);
        lie.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
        assert_eq!(decode(&lie).unwrap_err(), Reject::Malformed);
        // Garbage flagged as deflated.
        let mut junk = good[..HEADER_LEN].to_vec();
        junk[3] = flag::DEFLATED;
        junk.extend_from_slice(&[0xAB; 50]);
        assert_eq!(decode(&junk).unwrap_err(), Reject::Malformed);
    }

    #[test]
    fn deflate_round_trip_and_bomb_guard() {
        let data = vec![42u8; 100_000];
        let z = deflate(&data, Compression::default()).unwrap();
        assert!(z.len() < 500);
        assert_eq!(inflate(&z, 100_000).unwrap(), data);
        assert_eq!(inflate(&z, 99_999), None, "refuses to inflate beyond the cap");
    }

    #[test]
    fn the_inflate_budget_is_shared_and_runs_out() {
        let zeros = |n: usize| deflate(&vec![0u8; n], Compression::default()).unwrap();
        let mut budget = Budget::new();
        let a = budget.inflate(&zeros(1 << 20), 1 << 20).unwrap();
        assert_eq!(a.len(), 1 << 20);
        let b = budget.inflate(&zeros(1 << 20), 1 << 20).unwrap();
        assert_eq!(b.len(), 1 << 20);
        assert_eq!(budget.left, INFLATE_BUDGET - (2 << 20));
        assert!(budget.inflate(&zeros(10), 1 << 20).is_none(), "a third megabyte, and even ten bytes, are over");
        assert!(Budget::new().inflate(&zeros(1 << 20), 1000).is_none(), "the per-message cap holds too");
        // The body takes its share: a body that inflates to 1 KB leaves the messages 1 KB less.
        let body = Body { unreliable: vec![Msg { data: vec![1; 30], deflated: false }; 30], ..Body::default() };
        let bytes = encode(&header(), &body);
        assert_ne!(bytes[3] & flag::DEFLATED, 0, "{} bytes", bytes.len());
        let mut budget = Budget::new();
        let (_, back) = decode_with(&bytes, &mut budget).unwrap();
        assert_eq!(back, body);
        assert!(budget.left < INFLATE_BUDGET, "the body's inflated bytes were charged");
        let mut tiny = Budget { left: 100 };
        assert_eq!(decode_with(&bytes, &mut tiny).unwrap_err(), Reject::Malformed);
    }

    #[test]
    fn a_body_that_inflates_past_one_datagram_is_malformed() {
        // 15 KB of body is what the old 16 KB cap let through; no honest body is over MAX_BODY.
        let streams = deflate(&vec![0u8; 1 << 20], Compression::default()).unwrap();
        let body = Body { unreliable: vec![Msg { data: streams, deflated: true }; 15], ..Body::default() };
        let bytes = encode(&header(), &body);
        assert!(bytes.len() < 300, "{}", bytes.len());
        assert_eq!(decode(&bytes).unwrap_err(), Reject::Malformed);
    }

    #[test]
    fn a_cookie_rides_in_the_acknowledgements_and_reads_as_none_acknowledged() {
        let cookie = 0x0123_4567_89AB_CDEF;
        let hello = Body { reliable: vec![(0, Msg { data: b"hello".to_vec(), deflated: false })], ..Body::default() };
        let bytes = encode(&Header { ack: 0, ack_bits: 0, cookie: Some(cookie), ..header() }, &hello);
        assert_eq!(bytes.len(), encode(&Header { ack: 0, ack_bits: 0, ..header() }, &hello).len(), "it costs no byte");
        assert_ne!(bytes[3] & flag::COOKIE, 0);
        let (h, b) = decode(&bytes).unwrap();
        assert_eq!((h.cookie, h.ack, h.ack_bits), (Some(cookie), 0, 0));
        assert_eq!(b, hello);
        assert!(carries_messages(&bytes));
        assert_eq!(peek(&encode(&header(), &hello)).unwrap().cookie, None);
    }

    #[test]
    fn a_knock_and_its_challenge_are_not_link_datagrams() {
        let knock = knock(0xC0FFEE);
        assert_eq!(knock.len(), KNOCK_LEN);
        assert_eq!(handshake(&knock), Some(Handshake::Knock { session: 0xC0FFEE }));
        let answer = challenge(0xC0FFEE, u64::MAX - 5);
        assert!(answer.len() <= knock.len(), "the answer is never bigger than the knock");
        assert_eq!(handshake(&answer), Some(Handshake::Challenge { session: 0xC0FFEE, cookie: u64::MAX - 5 }));
        for datagram in [&knock, &answer] {
            assert_eq!(decode(datagram).unwrap_err(), Reject::Malformed, "no link takes it");
            assert!(!carries_messages(datagram));
        }
        // A knock cut short or padded out, and a link datagram, are no knock.
        assert_eq!(handshake(&knock[..KNOCK_LEN - 1]), None);
        assert_eq!(handshake(&[knock.clone(), vec![0]].concat()), None);
        assert_eq!(handshake(&encode(&header(), &full_body())), None);
        assert_eq!(handshake(b"SH"), None);
    }

    #[test]
    fn a_hello_of_protocol_1_is_recognised_and_answered_in_its_words() {
        let hello = Body { reliable: vec![(0, Msg { data: b"{\"Hello\":{}}".to_vec(), deflated: false })], ..Body::default() };
        let old = as_version(&encode(&header(), &hello), PROTO_1);
        assert_eq!(decode(&old).unwrap_err(), Reject::Proto(PROTO_1), "not this version");
        assert!(is_ours(&old));
        assert_eq!(older_build(&old), Some(0xDEAD_BEEF));
        // Acknowledgements alone, another version still, junk: nothing to answer.
        assert_eq!(older_build(&as_version(&encode(&header(), &Body::default()), PROTO_1)), None);
        assert_eq!(older_build(&as_version(&encode(&header(), &hello), 0)), None);
        assert_eq!(older_build(&encode(&header(), &hello)), None, "this version is no older build");
        assert_eq!(older_build(b"SH\x01\x00 not a datagram at all"), None);
        assert!(!is_ours(b"GET / HTTP/1.1"));
    }

    #[test]
    fn varints_round_trip() {
        for v in [0u32, 1, 127, 128, 16_383, 16_384, 2_097_151, 2_097_152, 268_435_455, 268_435_456, u32::MAX] {
            let mut out = Vec::new();
            put_varint(&mut out, v);
            assert_eq!(out.len(), varint_len(v));
            let mut r = Reader { buf: &out, pos: 0 };
            assert_eq!(r.varint().unwrap(), v);
        }
    }
}
