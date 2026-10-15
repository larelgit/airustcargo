//! How long a sound file plays, read from its headers: the mixer knows when a voice ends without
//! decoding it, so a headless game (no audio device, no decoder) frees voices on time too.

use std::path::Path;

/// The file kinds the engine plays: `.wav` (PCM) and `.ogg` (Vorbis).
pub const EXTENSIONS: [&str; 2] = ["wav", "ogg"];

/// Seconds `path` plays, or why it cannot be told (not a file, another format, a broken header).
pub fn file_seconds(path: &Path) -> Result<f64, String> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !EXTENSIONS.contains(&ext.as_str()) {
        return Err("plays only .wav and .ogg (Vorbis) files".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot be read ({e})"))?;
    if ext == "wav" { wav_seconds(&bytes) } else { ogg_seconds(&bytes) }
}

/// A RIFF/WAVE file: the `data` chunk's size over the `fmt ` chunk's byte rate.
pub fn wav_seconds(bytes: &[u8]) -> Result<f64, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("is no WAV file (no RIFF/WAVE header)".to_owned());
    }
    let mut at = 12;
    let mut byte_rate = None;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if id == b"fmt " {
            if size < 16 || body + 16 > bytes.len() {
                return Err("has a short `fmt ` chunk".to_owned());
            }
            byte_rate = Some(u32::from_le_bytes(bytes[body + 8..body + 12].try_into().unwrap()));
        } else if id == b"data" {
            let rate = byte_rate.filter(|r| *r > 0).ok_or("has its `data` before a `fmt ` chunk")?;
            // A writer that never came back to fill the size in leaves the rest of the file.
            let size = size.min(bytes.len() - body);
            return Ok(size as f64 / f64::from(rate));
        }
        // Chunks are padded to an even size. A size past the end ends the walk.
        let Some(next) = body.checked_add(size).and_then(|n| n.checked_add(size & 1)) else { break };
        at = next;
    }
    Err("has no `data` chunk".to_owned())
}

/// An Ogg Vorbis file: the granule position (samples) of the Vorbis stream's last good page
/// over its identification header's sample rate. Pages are walked by their headers; past one
/// that is broken (cut off, another version, a wrong checksum) the next is searched for, and
/// its checksum tells a page from packet data that holds the bytes `OggS` too.
pub fn ogg_seconds(bytes: &[u8]) -> Result<f64, String> {
    if bytes.len() < 4 || &bytes[0..4] != b"OggS" {
        return Err("is no Ogg file (no `OggS` page)".to_owned());
    }
    let mut stream = None;
    let mut last = None;
    let mut at = 0;
    // What follows the last good page adds no sound.
    while at < bytes.len() {
        let Some(page) = OggPage::read(bytes, at) else {
            match next_capture(bytes, at + 1) {
                Some(next) => at = next,
                None => break,
            }
            continue;
        };
        at = page.end;
        let Some((serial, _)) = stream else {
            // `\x01vorbis`, version (4), channels (1), sample rate (4): the first packet of its stream.
            if page.body.starts_with(b"\x01vorbis") {
                let rate = page.body.get(12..16).map(|b| u32::from_le_bytes(b.try_into().unwrap())).filter(|r| *r > 0);
                stream = Some((page.serial, rate.ok_or("has a broken Vorbis header (no sample rate)")?));
            }
            continue;
        };
        // -1: no packet ends on this page.
        if page.serial == serial && page.granule >= 0 {
            last = Some(page.granule);
        }
    }
    let (_, rate) = stream.ok_or("is no Ogg Vorbis file (no Vorbis identification header)")?;
    let samples = last.ok_or("has no sample count on any page of its Vorbis stream")?;
    Ok(samples as f64 / f64::from(rate))
}

/// One Ogg page: `OggS`, version (1), flags (1), granule position (8), serial (4), sequence (4),
/// CRC (4), segment count (1), the segment sizes, then the body.
struct OggPage<'a> {
    granule: i64,
    serial: u32,
    body: &'a [u8],
    /// Where the next page starts.
    end: usize,
}

impl<'a> OggPage<'a> {
    /// The good page at `at`, if one is there: complete, version 0, its checksum right.
    fn read(bytes: &'a [u8], at: usize) -> Option<Self> {
        let header = bytes.get(at..at + 27)?;
        if &header[0..4] != b"OggS" || header[4] != 0 {
            return None;
        }
        let segments = usize::from(header[26]);
        let sizes = bytes.get(at + 27..at + 27 + segments)?;
        let start = at + 27 + segments;
        let end = start + sizes.iter().map(|s| usize::from(*s)).sum::<usize>();
        let crc = u32::from_le_bytes(header[22..26].try_into().unwrap());
        if page_crc(bytes.get(at..end)?) != crc {
            return None;
        }
        Some(Self {
            granule: i64::from_le_bytes(header[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(header[14..18].try_into().unwrap()),
            body: &bytes[start..end],
            end,
        })
    }
}

/// Where the next `OggS` starts, from `from` on.
fn next_capture(bytes: &[u8], from: usize) -> Option<usize> {
    bytes.get(from..)?.windows(4).position(|w| w == b"OggS").map(|i| from + i)
}

/// Ogg's CRC-32 (polynomial 0x04c11db7, most significant bit first, starting at 0, no final
/// xor), by bytes.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04c1_1db7 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

/// A page's checksum: over the whole page, its own field (bytes 22 to 25) read as zeros.
fn page_crc(page: &[u8]) -> u32 {
    page.iter().enumerate().fold(0u32, |crc, (i, byte)| {
        let byte = if (22..26).contains(&i) { 0 } else { *byte };
        (crc << 8) ^ CRC_TABLE[usize::from((crc >> 24) as u8 ^ byte)]
    })
}

/// A WAV file of mono 16-bit PCM samples: what the tests and the sound generator write.
pub fn wav_bytes(rate: u32, samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    // PCM, one channel.
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_plays_its_data_over_its_byte_rate() {
        let wav = wav_bytes(8000, &vec![0; 4000]);
        assert_eq!(wav_seconds(&wav), Ok(0.5));
        assert!(wav_seconds(b"RIFF\0\0\0\0WAVE").unwrap_err().contains("no `data` chunk"));
        assert!(wav_seconds(b"not a wav file").unwrap_err().contains("no RIFF/WAVE header"));
        // A chunk that says it is 4 GiB long ends the walk on any target.
        let mut huge = b"RIFF\0\0\0\0WAVEjunk".to_vec();
        huge.extend(u32::MAX.to_le_bytes());
        assert!(wav_seconds(&huge).unwrap_err().contains("no `data` chunk"));
    }

    /// An Ogg page of stream `serial` with `body` (under 255 bytes: one segment).
    fn page(serial: u32, granule: i64, body: &[u8]) -> Vec<u8> {
        let mut page = b"OggS\0\0".to_vec();
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&serial.to_le_bytes());
        // The sequence number and the checksum, filled in below.
        page.extend_from_slice(&[0; 8]);
        page.push(1);
        page.push(body.len() as u8);
        page.extend_from_slice(body);
        sign(&mut page);
        page
    }

    /// Writes `page`'s checksum into it.
    fn sign(page: &mut [u8]) {
        let crc = page_crc(page);
        page[22..26].copy_from_slice(&crc.to_le_bytes());
    }

    /// The identification header of a stream at `rate` Hz.
    fn ident(rate: u32) -> Vec<u8> {
        let mut body = b"\x01vorbis".to_vec();
        body.extend_from_slice(&0u32.to_le_bytes());
        body.push(2);
        body.extend_from_slice(&rate.to_le_bytes());
        body.extend_from_slice(&[0; 14]);
        body
    }

    #[test]
    fn an_ogg_plays_its_last_granule_over_its_sample_rate() {
        let mut ogg = page(7, 0, &ident(44_100));
        ogg.extend(page(7, -1, &[0; 20]));
        ogg.extend(page(7, 88_200, &[0; 20]));
        assert_eq!(ogg_seconds(&ogg), Ok(2.0));
        assert!(ogg_seconds(b"OggS but nothing else").unwrap_err().contains("no Vorbis identification header"));
    }

    #[test]
    fn an_ogg_is_read_page_by_page_whatever_its_packets_hold() {
        let mut ogg = page(7, 0, &ident(8_000));
        ogg.extend(page(7, 4_000, b"audio data"));
        // The last packet holds `OggS` and what would read as a granule position after it.
        let mut tricky = b"xxOggS\0\0".to_vec();
        tricky.extend_from_slice(&999_999i64.to_le_bytes());
        ogg.extend(page(7, 16_000, &tricky));
        assert_eq!(ogg_seconds(&ogg), Ok(2.0));
        // Another stream's pages do not count, nor does a page cut off at the end.
        ogg.extend(page(9, 80_000, b"video"));
        let cut = page(7, 64_000, &[0; 40]);
        ogg.extend_from_slice(&cut[..cut.len() - 10]);
        assert_eq!(ogg_seconds(&ogg), Ok(2.0));
    }

    #[test]
    fn a_broken_page_is_passed_over_for_the_next_good_one() {
        let mut ogg = page(7, 0, &ident(8_000));
        ogg.extend(page(7, 8_000, b"audio"));
        // A granule position written over: the checksum no longer holds.
        let mut forged = page(7, 8_000, b"audio");
        forged[6..14].copy_from_slice(&999_999i64.to_le_bytes());
        ogg.extend(forged);
        // Another version of the format, its checksum right.
        let mut other = page(7, 800_000, b"audio");
        other[4] = 1;
        sign(&mut other);
        ogg.extend(other);
        ogg.extend(page(7, 16_000, b"audio"));
        assert_eq!(ogg_seconds(&ogg), Ok(2.0));
    }

    #[test]
    fn the_checksum_is_oggs_crc32() {
        // CRC-32/CKSUM checks `123456789` as 0x765e7680 after its final xor, which Ogg's has not.
        assert_eq!(page_crc(b"123456789"), 0x765e_7680 ^ 0xffff_ffff);
    }

    #[test]
    fn other_formats_are_named() {
        let err = file_seconds(Path::new("music/theme.mp3")).unwrap_err();
        assert_eq!(err, "plays only .wav and .ogg (Vorbis) files");
    }
}
