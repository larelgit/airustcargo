//! Canonical JSON: the one form a save is written in, and an exact reader for it.
//!
//! Same data, same bytes: object keys are sorted and numbers print the shortest text that
//! reads back as the same number (`0.1`, not the `f64` an `f32` widens to).
//!
//! The reader is here because `serde_json` without its `float_roundtrip` feature is not
//! exact: about one `f64` in three that it printed reads back one bit off. A save that
//! lost a bit of a position is a lost save, and the feature is not ours to switch on (it
//! is global to the build), so the text goes through `str::parse`, which is exact. The
//! structure, strings and escapes follow JSON to the letter; what the reader refuses
//! (nesting beyond [`MAX_DEPTH`], numbers out of range, trailing text) says where.

use serde::Serialize;
use serde_json::{Map, Number, Value};

/// Deepest nesting the reader accepts: a damaged file must not overflow the stack.
pub const MAX_DEPTH: usize = 128;

/// `value` as canonical compact JSON: sorted keys, no spaces.
pub fn to_string(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

/// Appends `value` to `out` as canonical compact JSON.
pub fn write(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write(&map[key], out);
            }
            out.push('}');
        }
    }
}

/// A JSON string literal.
pub fn write_string(s: &str, out: &mut String) {
    // Cannot fail: a `str` always serializes.
    out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned()));
}

/// Any serializable value as a [`Value`] that reads back to the same value: through its
/// text, so an `f32` keeps its short form (`0.1`) and not the `f64` it widens to.
pub fn to_value<T: Serialize + ?Sized>(value: &T) -> Result<Value, String> {
    let text = serde_json::to_string(value).map_err(|e| e.to_string())?;
    parse(&text)
}

/// FNV-1a over `bytes`: the hash `save_check` reports.
pub fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Reads one JSON document. Numbers without a fraction or exponent are integers when they
/// fit (`u64`, else `i64`); every other number is the nearest `f64`, exactly.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader { text, bytes: text.as_bytes(), at: 0 };
    reader.skip_space();
    let value = reader.value(0)?;
    reader.skip_space();
    if reader.at != reader.bytes.len() {
        return Err(reader.fail("unexpected text after the document"));
    }
    Ok(value)
}

struct Reader<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn fail(&self, what: &str) -> String {
        format!("bad JSON at byte {}: {what}", self.at)
    }

    fn skip_space(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn word(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.fail(&format!("expected `{word}`")))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.fail(&format!("nesting deeper than {MAX_DEPTH}")));
        }
        match self.bytes.get(self.at) {
            None => Err(self.fail("the document ends here")),
            Some(b'n') => self.word("null", Value::Null),
            Some(b't') => self.word("true", Value::Bool(true)),
            Some(b'f') => self.word("false", Value::Bool(false)),
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.fail("expected a value")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, String> {
        self.at += 1;
        let mut items = Vec::new();
        self.skip_space();
        if self.eat(b']') {
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_space();
            items.push(self.value(depth + 1)?);
            self.skip_space();
            if self.eat(b',') {
                continue;
            }
            if self.eat(b']') {
                return Ok(Value::Array(items));
            }
            return Err(self.fail("expected `,` or `]`"));
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, String> {
        self.at += 1;
        let mut map = Map::new();
        self.skip_space();
        if self.eat(b'}') {
            return Ok(Value::Object(map));
        }
        loop {
            self.skip_space();
            if self.bytes.get(self.at) != Some(&b'"') {
                return Err(self.fail("expected a key"));
            }
            let key = self.string()?;
            self.skip_space();
            if !self.eat(b':') {
                return Err(self.fail("expected `:`"));
            }
            self.skip_space();
            let value = self.value(depth + 1)?;
            map.insert(key, value);
            self.skip_space();
            if self.eat(b',') {
                continue;
            }
            if self.eat(b'}') {
                return Ok(Value::Object(map));
            }
            return Err(self.fail("expected `,` or `}`"));
        }
    }

    /// A string literal: found here, unescaped by `serde_json` (escapes, surrogate pairs and
    /// control characters follow the standard to the letter).
    fn string(&mut self) -> Result<String, String> {
        let start = self.at;
        self.at += 1;
        loop {
            match self.bytes.get(self.at) {
                None => return Err(self.fail("a string is not closed")),
                Some(b'"') => {
                    self.at += 1;
                    break;
                }
                Some(b'\\') => self.at += 2,
                Some(_) => self.at += 1,
            }
        }
        let literal = self.text.get(start..self.at).ok_or_else(|| self.fail("a string ends inside an escape"))?;
        serde_json::from_str::<String>(literal).map_err(|e| format!("bad JSON string at byte {start}: {e}"))
    }

    fn digits(&mut self) -> usize {
        let from = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at - from
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.at;
        self.eat(b'-');
        match self.bytes.get(self.at) {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.fail("a number needs digits")),
        }
        let mut integer = true;
        if self.eat(b'.') {
            integer = false;
            if self.digits() == 0 {
                return Err(self.fail("a fraction needs digits"));
            }
        }
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            integer = false;
            self.at += 1;
            if !self.eat(b'+') {
                self.eat(b'-');
            }
            if self.digits() == 0 {
                return Err(self.fail("an exponent needs digits"));
            }
        }
        let token = &self.text[start..self.at];
        if integer {
            if let Ok(n) = token.parse::<u64>() {
                return Ok(Value::Number(n.into()));
            }
            if let Ok(n) = token.parse::<i64>() {
                return Ok(Value::Number(n.into()));
            }
        }
        let float: f64 = token.parse().map_err(|_| format!("bad JSON at byte {start}: `{token}` is not a number"))?;
        Number::from_f64(float)
            .map(Value::Number)
            .ok_or_else(|| format!("bad JSON at byte {start}: `{token}` is out of range"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keys_are_sorted_and_floats_are_short() {
        let value = parse(r#"{"b": [1, 2.5, -0.0, 1e-7], "a": {"z": null, "y": "é\n"}}"#).unwrap();
        assert_eq!(to_string(&value), r#"{"a":{"y":"é\n","z":null},"b":[1,2.5,-0.0,1e-7]}"#);
        // An f32 keeps its short text.
        assert_eq!(to_string(&to_value(&0.1_f32).unwrap()), "0.1");
        assert_eq!(to_string(&to_value(&vec![1.5_f32, f32::MAX, f32::MIN_POSITIVE]).unwrap()), "[1.5,3.4028235e+38,1.1754944e-38]");
    }

    #[test]
    fn numbers_read_back_exactly() {
        // The case `serde_json` alone gets wrong for about a third of random doubles.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let f = f64::from_bits(x);
            if !f.is_finite() {
                continue;
            }
            let text = to_string(&Value::Number(Number::from_f64(f).unwrap()));
            assert_eq!(parse(&text).unwrap().as_f64().unwrap().to_bits(), f.to_bits(), "{text}");
            let g = f32::from_bits((x >> 32) as u32);
            if g.is_finite() {
                let back: f32 = serde_json::from_value(to_value(&g).unwrap()).unwrap();
                assert_eq!(back.to_bits(), g.to_bits());
            }
        }
        assert_eq!(parse("18446744073709551615").unwrap(), json!(u64::MAX));
        assert_eq!(parse("-9223372036854775808").unwrap(), json!(i64::MIN));
        assert!(parse("18446744073709551616").unwrap().is_f64());
        assert!(parse("1e999").unwrap_err().contains("out of range"));
    }

    #[test]
    fn what_json_forbids_is_refused_and_located() {
        for (text, why) in [
            ("", "ends"),
            ("{", "expected a key"),
            ("{\"a\" 1}", "expected `:`"),
            ("[1 2]", "expected `,` or `]`"),
            ("[1,]", "expected a value"),
            ("01", "after the document"),
            ("1.", "fraction"),
            ("1e", "exponent"),
            ("-", "needs digits"),
            ("\"open", "not closed"),
            ("{\"a\":1} x", "after the document"),
            ("nul", "expected `null`"),
            ("\"\\q\"", "bad JSON string"),
            ("\"a\u{1}b\"", "bad JSON string"),
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.contains(why), "{text:?}: {err}");
        }
        let deep = format!("{}{}", "[".repeat(MAX_DEPTH + 2), "]".repeat(MAX_DEPTH + 2));
        assert!(parse(&deep).unwrap_err().contains("nesting"));
        let shallow = format!("{}{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&shallow).is_ok());
    }

    #[test]
    fn agrees_with_serde_json_on_structure_and_strings() {
        let text = r#" { "k\u00e9y" : [ true , false , null , "\ud83d\ude00 \" \\ \/ \t" , { } , [ ] , 0 , -12 ] } "#;
        assert_eq!(parse(text).unwrap(), serde_json::from_str::<Value>(text).unwrap());
    }

    #[test]
    fn fnv_is_the_standard_one() {
        assert_eq!(fnv(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
