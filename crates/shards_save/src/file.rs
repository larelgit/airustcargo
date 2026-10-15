//! The save file: one header line, then canonical JSON.
//!
//! ```text
//! shards-save/1 crc32=1a2b3c4d len=812
//! {
//!   "engine": "0.2.0",
//!   "game": {"title":"Shards sandbox","version":"0.1.0"},
//!   "kind": "character",
//!   "label": "camp at the ford",
//!   "sections": {
//!     "position": {"data":{...},"version":1},
//!     "vitals": {"data":{...},"version":1}
//!   },
//!   "slot": "slot1",
//!   "tick": 51230
//! }
//! ```
//!
//! `len` is the number of bytes after the first line, `crc32` their checksum (IEEE, as
//! `zip` and `png` use). A file that fails either, or whose JSON does not read, is damaged:
//! [`SaveFile::decode`] says how, in words the journal can carry.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canon;
use crate::section::SaveKind;

/// The format tag a file starts with. The number is the envelope's own version (header,
/// checksum, JSON layout); sections carry theirs.
pub const FORMAT: &str = "shards-save/1";

const FORMAT_PREFIX: &str = "shards-save/";

/// The number in [`FORMAT`]: a header with a higher one is from a newer engine.
const FORMAT_NUMBER: u32 = 1;

/// Why a file cannot be used. Damage and a newer version are told apart on purpose: damage
/// is repaired from a backup, a newer save is not damaged and is never touched (see
/// [`store`](crate::store)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// The file is damaged; the text says how, short enough to sit in parentheses
    /// (`checksum mismatch: ...`).
    Damaged(String),
    /// A newer version of the game or engine wrote it: the format number of the envelope or
    /// a section's version is above what this build reads. The text names which
    /// (`section inventory v7 > v5`).
    Newer(String),
}

impl Fault {
    /// Whether this is a newer version, not damage.
    pub fn is_newer(&self) -> bool {
        matches!(self, Fault::Newer(_))
    }

    /// The damage text, or what is newer (`section inventory v7 > v5`).
    pub fn detail(&self) -> &str {
        match self {
            Fault::Damaged(text) | Fault::Newer(text) => text,
        }
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::Damaged(text) => f.write_str(text),
            Fault::Newer(detail) => write!(f, "saved by a newer version of the game ({detail})"),
        }
    }
}

impl From<String> for Fault {
    fn from(text: String) -> Self {
        Fault::Damaged(text)
    }
}

/// One section as it is in a file: the version of its schema and its data.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SectionData {
    /// The section's schema version when it was written.
    pub version: u32,
    pub data: Value,
}

/// What the game was when a file was written.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct GameInfo {
    pub title: String,
    pub version: String,
}

/// The sections of one save, by name: what [`capture_sections`](crate::capture_sections)
/// returns and [`apply_sections`](crate::apply_sections) takes. Serializable, so co-op can
/// send a character over the network and write it on the player's machine.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SectionSet(pub BTreeMap<String, SectionData>);

impl SectionSet {
    /// `{name: {version, data}}` as canonical JSON: what `save_check` hashes, and the form to
    /// send a set between machines in ([`from_canonical`](Self::from_canonical) reads it
    /// back exactly; `serde_json` reads about one `f64` in three a bit off).
    pub fn canonical(&self) -> String {
        let value = serde_json::to_value(self).unwrap_or(Value::Null);
        canon::to_string(&value)
    }

    /// Reads [`canonical`](Self::canonical) text.
    pub fn from_canonical(text: &str) -> Result<SectionSet, String> {
        let value = canon::parse(text)?;
        serde_json::from_value(value).map_err(|e| format!("not a set of sections: {e}"))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A save file, read or about to be written.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveFile {
    pub kind: SaveKind,
    pub slot: String,
    /// What the player or the game called it (`camp at the ford`).
    pub label: String,
    pub game: GameInfo,
    /// The engine version that wrote it.
    pub engine: String,
    /// The tick it was captured at.
    pub tick: u64,
    pub sections: SectionSet,
}

/// The JSON body, as written.
#[derive(Serialize, Deserialize)]
struct Body {
    kind: SaveKind,
    #[serde(default)]
    slot: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    game: GameInfo,
    #[serde(default)]
    engine: String,
    tick: u64,
    sections: BTreeMap<String, SectionData>,
}

impl SaveFile {
    /// The file's bytes: header line and canonical body.
    pub fn encode(&self) -> Vec<u8> {
        let body = self.body();
        let mut out = format!("{FORMAT} crc32={:08x} len={}\n", crc32fast::hash(body.as_bytes()), body.len()).into_bytes();
        out.extend_from_slice(body.as_bytes());
        out
    }

    /// The canonical body: one line per top-level key, one per section.
    fn body(&self) -> String {
        let mut out = String::from("{\n");
        let line = |out: &mut String, key: &str, value: &Value, last: bool| {
            out.push_str("  ");
            canon::write_string(key, out);
            out.push_str(": ");
            canon::write(value, out);
            out.push_str(if last { "\n" } else { ",\n" });
        };
        line(&mut out, "engine", &Value::String(self.engine.clone()), false);
        let game = serde_json::to_value(&self.game).unwrap_or(Value::Null);
        line(&mut out, "game", &game, false);
        line(&mut out, "kind", &Value::String(self.kind.as_str().to_owned()), false);
        line(&mut out, "label", &Value::String(self.label.clone()), false);
        out.push_str("  \"sections\": {");
        for (i, (name, section)) in self.sections.0.iter().enumerate() {
            out.push_str(if i == 0 { "\n    " } else { ",\n    " });
            canon::write_string(name, &mut out);
            out.push_str(": ");
            canon::write(&serde_json::to_value(section).unwrap_or(Value::Null), &mut out);
        }
        out.push_str(if self.sections.is_empty() { "},\n" } else { "\n  },\n" });
        line(&mut out, "slot", &Value::String(self.slot.clone()), false);
        line(&mut out, "tick", &Value::Number(self.tick.into()), true);
        out.push_str("}\n");
        out
    }

    /// Reads a file's bytes: the header, the length, the checksum, the JSON and its shape.
    /// The error names the first thing wrong, short enough to sit in parentheses:
    /// `checksum mismatch: the header says 1a2b3c4d, the content is 99aa0011`.
    ///
    /// A format number above this build's (`shards-save/2`) is [`Fault::Newer`], told before
    /// anything else is checked: a newer layout of the rest is not this build's to judge.
    pub fn decode(bytes: &[u8]) -> Result<SaveFile, Fault> {
        let (header, body) = split(bytes)?;
        let (stated_len, stated_crc) = parse_header(header)?;
        if body.len() as u64 != stated_len {
            let relation = if (body.len() as u64) < stated_len { "so it is truncated" } else { "so it is longer than written" };
            return Err(format!("length mismatch: the header says {stated_len} bytes, the file has {}, {relation}", body.len()).into());
        }
        let crc = crc32fast::hash(body);
        if crc != stated_crc {
            return Err(format!("checksum mismatch: the header says {stated_crc:08x}, the content is {crc:08x}").into());
        }
        let text = std::str::from_utf8(body).map_err(|e| format!("the content is not UTF-8 at byte {}", e.valid_up_to()))?;
        let value = canon::parse(text)?;
        let body: Body = serde_json::from_value(value).map_err(|e| format!("not a save: {e}"))?;
        Ok(SaveFile {
            kind: body.kind,
            slot: body.slot,
            label: body.label,
            game: body.game,
            engine: body.engine,
            tick: body.tick,
            sections: SectionSet(body.sections),
        })
    }
}

/// Header line and the bytes after it.
fn split(bytes: &[u8]) -> Result<(&str, &[u8]), Fault> {
    if bytes.is_empty() {
        return Err("file is empty".to_owned().into());
    }
    let end = bytes.iter().position(|b| *b == b'\n').filter(|end| *end <= 256);
    let Some(end) = end else {
        return Err(format!("no header: the first line must read `{FORMAT} crc32=<hex> len=<bytes>`").into());
    };
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| format!("no header: the first line must read `{FORMAT} crc32=<hex> len=<bytes>`"))?;
    Ok((header, &bytes[end + 1..]))
}

/// `(len, crc32)` from `shards-save/1 crc32=1a2b3c4d len=812`.
fn parse_header(header: &str) -> Result<(u64, u32), Fault> {
    let mut parts = header.split(' ');
    let format = parts.next().unwrap_or_default();
    if format != FORMAT {
        let number = format.strip_prefix(FORMAT_PREFIX).and_then(|n| n.parse::<u32>().ok());
        return match number {
            Some(n) if n > FORMAT_NUMBER => Err(Fault::Newer(format!("save format {format} > {FORMAT}"))),
            _ => Err(format!("no header: the first line must read `{FORMAT} crc32=<hex> len=<bytes>`").into()),
        };
    }
    let (mut crc, mut len) = (None, None);
    for part in parts {
        match part.split_once('=') {
            Some(("crc32", hex)) => crc = u32::from_str_radix(hex, 16).ok(),
            Some(("len", n)) => len = n.parse::<u64>().ok(),
            _ => {}
        }
    }
    match (len, crc) {
        (Some(len), Some(crc)) => Ok((len, crc)),
        _ => Err(format!("the header is incomplete: it must read `{FORMAT} crc32=<hex> len=<bytes>`").into()),
    }
}

/// The checksum and length a header states, for the cheap integrity test the store makes
/// before it keeps a file as a backup.
pub fn is_intact(bytes: &[u8]) -> bool {
    let Ok((header, body)) = split(bytes) else {
        return false;
    };
    matches!(parse_header(header), Ok((len, crc)) if len == body.len() as u64 && crc == crc32fast::hash(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn file() -> SaveFile {
        let mut sections = BTreeMap::new();
        sections.insert("vitals".to_owned(), SectionData { version: 2, data: json!({"health": 12.5, "name": "é"}) });
        sections.insert("position".to_owned(), SectionData { version: 1, data: json!({"feet": [1.0, 0.5, -3.25]}) });
        SaveFile {
            kind: SaveKind::Character,
            slot: "slot1".into(),
            label: "camp".into(),
            game: GameInfo { title: "T".into(), version: "1.0".into() },
            engine: "0.2.0".into(),
            tick: 99,
            sections: SectionSet(sections),
        }
    }

    #[test]
    fn a_file_reads_back_and_has_the_documented_shape() {
        let f = file();
        let bytes = f.encode();
        assert_eq!(SaveFile::decode(&bytes).unwrap(), f);
        let text = String::from_utf8(bytes.clone()).unwrap();
        let (header, body) = text.split_once('\n').unwrap();
        assert_eq!(header, format!("shards-save/1 crc32={:08x} len={}", crc32fast::hash(body.as_bytes()), body.len()));
        assert!(body.starts_with("{\n  \"engine\": \"0.2.0\",\n  \"game\": {\"title\":\"T\",\"version\":\"1.0\"},\n  \"kind\": \"character\","));
        assert!(body.contains("\n    \"position\": {\"data\":{\"feet\":[1.0,0.5,-3.25]},\"version\":1},\n    \"vitals\": "));
        assert!(body.ends_with("  \"slot\": \"slot1\",\n  \"tick\": 99\n}\n"));
        assert_eq!(f.encode(), bytes, "same data, same bytes");
        let empty = SaveFile { sections: SectionSet::default(), ..f };
        assert_eq!(SaveFile::decode(&empty.encode()).unwrap(), empty);
    }

    #[test]
    fn every_kind_of_damage_is_named() {
        let good = file().encode();
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        let body_at = good.iter().position(|b| *b == b'\n').unwrap() + 1;

        assert_eq!(SaveFile::decode(&[]).unwrap_err().to_string(), "file is empty");
        assert!(SaveFile::decode(b"garbage without any line break").unwrap_err().to_string().starts_with("no header"));
        assert!(SaveFile::decode(b"{\"kind\":\"world\"}\n{}").unwrap_err().to_string().starts_with("no header"));

        let mut flipped = good.clone();
        flipped[body_at + 20] ^= 0x01;
        assert!(SaveFile::decode(&flipped).unwrap_err().to_string().starts_with("checksum mismatch: the header says "), "{}", SaveFile::decode(&flipped).unwrap_err().to_string());

        let truncated = &good[..good.len() - 7];
        let err = SaveFile::decode(truncated).unwrap_err().to_string();
        assert!(err.starts_with("length mismatch") && err.contains("so it is truncated"), "{err}");

        let mut longer = good.clone();
        longer.extend_from_slice(b"\n\n");
        assert!(SaveFile::decode(&longer).unwrap_err().to_string().contains("longer than written"));

        let wrong_len = text(&good).replacen("len=", "len=1", 1);
        assert!(SaveFile::decode(wrong_len.as_bytes()).unwrap_err().to_string().starts_with("length mismatch"));

        let newer = text(&good).replacen("shards-save/1", "shards-save/2", 1);
        let fault = SaveFile::decode(newer.as_bytes()).unwrap_err();
        assert_eq!(fault, Fault::Newer("save format shards-save/2 > shards-save/1".to_owned()));
        assert!(fault.is_newer() && fault.to_string() == "saved by a newer version of the game (save format shards-save/2 > shards-save/1)");
        // Not a number, or not above: no header, which is damage.
        for odd in ["shards-save/x", "shards-save/0", "shards-save/-1", "other/2"] {
            let text = text(&good).replacen("shards-save/1", odd, 1);
            assert!(!SaveFile::decode(text.as_bytes()).unwrap_err().is_newer(), "{odd}");
        }

        let partial = text(&good).replacen(" len=", " lenx=", 1);
        assert!(SaveFile::decode(partial.as_bytes()).unwrap_err().to_string().starts_with("the header is incomplete"));

        // A checksum that is right about garbage: the JSON is what fails.
        let forge = |body: &str| format!("{FORMAT} crc32={:08x} len={}\n{body}", crc32fast::hash(body.as_bytes()), body.len()).into_bytes();
        assert!(SaveFile::decode(&forge("{\"kind\": ")).unwrap_err().to_string().starts_with("bad JSON at byte"));
        assert!(SaveFile::decode(&forge("{\"kind\": \"world\"}")).unwrap_err().to_string().starts_with("not a save: "));
        assert!(SaveFile::decode(&forge("{\"kind\": \"boss\", \"tick\": 1, \"sections\": {}}")).unwrap_err().to_string().starts_with("not a save: "));
        assert!(SaveFile::decode(&forge("\u{0}\u{0}")).unwrap_err().to_string().starts_with("bad JSON"));
        let not_utf8 = {
            let body = [0xff_u8, 0xfe];
            let mut out = format!("{FORMAT} crc32={:08x} len=2\n", crc32fast::hash(&body)).into_bytes();
            out.extend_from_slice(&body);
            out
        };
        assert!(SaveFile::decode(&not_utf8).unwrap_err().to_string().starts_with("the content is not UTF-8"));
    }

    #[test]
    fn is_intact_is_the_cheap_test() {
        let good = file().encode();
        assert!(is_intact(&good));
        let mut bad = good.clone();
        bad[good.len() - 3] ^= 0xff;
        assert!(!is_intact(&bad) && !is_intact(&good[..10]) && !is_intact(b""));
    }
}
