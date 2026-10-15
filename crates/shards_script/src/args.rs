//! `key=value` arguments of `shards act`, shared with scenario files.

use serde_json::{json, Map, Value};

/// One JSON object, or `key=value` pairs where values are JSON if they parse as JSON.
pub fn parse_args(args: &[String]) -> Result<Value, String> {
    if args.is_empty() {
        return Ok(json!({}));
    }
    if args.len() == 1 && args[0].trim_start().starts_with('{') {
        return serde_json::from_str(&args[0]).map_err(|e| format!("arguments are not valid JSON: {e}"));
    }
    let mut map = Map::new();
    for a in args {
        let (k, v) = a.split_once('=').ok_or_else(|| format!("expected key=value, got `{a}`"))?;
        map.insert(k.to_owned(), loose_value(v));
    }
    Ok(Value::Object(map))
}

/// `42` is a number, `true` a bool, `e6` a string: JSON if it parses, text otherwise.
pub fn loose_value(v: &str) -> Value {
    serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_parse() {
        assert_eq!(parse_args(&["target=42".into(), "queue=true".into()]).unwrap(), json!({"target": 42, "queue": true}));
        assert_eq!(parse_args(&["dir=W".into()]).unwrap(), json!({"dir": "W"}));
        assert_eq!(parse_args(&[r#"{"target":{"x":1,"z":2}}"#.into()]).unwrap(), json!({"target": {"x": 1, "z": 2}}));
        assert_eq!(parse_args(&[]).unwrap(), json!({}));
    }

    #[test]
    fn args_errors_name_the_word() {
        assert_eq!(parse_args(&["oops".into()]).unwrap_err(), "expected key=value, got `oops`");
        assert!(parse_args(&["{broken".into()]).unwrap_err().starts_with("arguments are not valid JSON"));
    }
}
