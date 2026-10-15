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
        // Chunks are padded to an even size.
        at = body + size + (size & 1);
    }
    Err("has no `data` chunk".to_owned())
}

/// An Ogg Vorbis file: the last page's granule position (samples) over the identification
/// header's sample rate.
pub fn ogg_seconds(bytes: &[u8]) -> Result<f64, String> {
    if bytes.len() < 4 || &bytes[0..4] != b"OggS" {
        return Err("is no Ogg file (no `OggS` page)".to_owned());
    }
    let ident = find(bytes, b"\x01vorbis").ok_or("is no Ogg Vorbis file (no Vorbis identification header)")?;
    // `\x01vorbis`, version (4), channels (1), sample rate (4).
    let rate_at = ident + 7 + 4 + 1;
    let rate = bytes.get(rate_at..rate_at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).filter(|r| *r > 0);
    let rate = rate.ok_or("has a broken Vorbis header (no sample rate)")?;
    let last = rfind(bytes, b"OggS").ok_or("has no Ogg page")?;
    // `OggS`, version (1), flags (1), granule position (8).
    let granule = bytes.get(last + 6..last + 14).map(|b| i64::from_le_bytes(b.try_into().unwrap())).ok_or("ends inside an Ogg page header")?;
    if granule < 0 {
        return Err("has no sample count on its last page".to_owned());
    }
    Ok(granule as f64 / f64::from(rate))
}

fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).position(|w| w == needle)
}

fn rfind(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes.windows(needle.len()).rposition(|w| w == needle)
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
    }

    #[test]
    fn an_ogg_plays_its_last_granule_over_its_sample_rate() {
        let mut ogg = b"OggS\0\x02".to_vec();
        ogg.extend_from_slice(&0i64.to_le_bytes());
        ogg.extend_from_slice(&[0; 13]);
        ogg.extend_from_slice(b"\x01vorbis");
        ogg.extend_from_slice(&0u32.to_le_bytes());
        ogg.push(2);
        ogg.extend_from_slice(&44_100u32.to_le_bytes());
        ogg.extend_from_slice(&[0; 20]);
        ogg.extend_from_slice(b"OggS\0\x04");
        ogg.extend_from_slice(&88_200i64.to_le_bytes());
        assert_eq!(ogg_seconds(&ogg), Ok(2.0));
        assert!(ogg_seconds(b"OggS but nothing else").unwrap_err().contains("no Vorbis identification header"));
    }

    #[test]
    fn other_formats_are_named() {
        let err = file_seconds(Path::new("music/theme.mp3")).unwrap_err();
        assert_eq!(err, "plays only .wav and .ogg (Vorbis) files");
    }
}
