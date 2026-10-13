//! The `--until` conditions of `step`, in their command-line spelling.

use serde_json::{json, Value};

use crate::args::loose_value;

/// `journal:phase` · `gone:57` · `shows:42:windup` · `near:12:42:3` ·
/// `field:12:Health.current<30` · `named:boss_phase_2` · raw JSON.
///
/// Returns the JSON the `step` tool takes in its `until` list.
pub fn parse_condition(s: &str) -> Result<Value, String> {
    if s.trim_start().starts_with('{') {
        return serde_json::from_str(s).map_err(|e| format!("bad condition JSON: {e}"));
    }
    let (kind, rest) = s.split_once(':').ok_or_else(|| format!("bad condition `{s}`"))?;
    Ok(match kind {
        "journal" => json!({"journal": rest}),
        "gone" => json!({"gone": loose_value(rest)}),
        "named" => json!({"named": rest}),
        "shows" => {
            let (id, text) = rest.split_once(':').ok_or("shows:ID:TEXT")?;
            json!({"shows": {"id": loose_value(id), "text": text}})
        }
        "near" => {
            let parts: Vec<&str> = rest.split(':').collect();
            let [a, b, m] = parts[..] else { return Err("near:A:B:METERS".into()) };
            let meters: f32 = m.parse().map_err(|_| "near: METERS must be a number")?;
            json!({"near": {"a": loose_value(a), "b": loose_value(b), "meters": meters}})
        }
        "field" => {
            let (id, expr) = rest.split_once(':').ok_or("field:ID:Component.path<N")?;
            let (lhs, op, rhs) = if let Some((l, r)) = expr.split_once('<') {
                (l, "below", r)
            } else if let Some((l, r)) = expr.split_once('>') {
                (l, "above", r)
            } else {
                return Err("field condition needs < or >".into());
            };
            let (component, path) = lhs.split_once('.').ok_or("field: use Component.path")?;
            let n: f64 = rhs.parse().map_err(|_| "field: threshold must be a number")?;
            json!({"field": {"id": loose_value(id), "component": component, "path": path, op: n}})
        }
        _ => return Err(format!("unknown condition kind `{kind}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_parse() {
        assert_eq!(parse_condition("journal:phase").unwrap(), json!({"journal": "phase"}));
        assert_eq!(parse_condition("gone:57").unwrap(), json!({"gone": 57}));
        assert_eq!(parse_condition("named:boss_phase_2").unwrap(), json!({"named": "boss_phase_2"}));
        assert_eq!(parse_condition("shows:e42:windup").unwrap(), json!({"shows": {"id": "e42", "text": "windup"}}));
        assert_eq!(parse_condition("near:12:42:3").unwrap(), json!({"near": {"a": 12, "b": 42, "meters": 3.0}}));
        assert_eq!(
            parse_condition("field:12:Health.current<30").unwrap(),
            json!({"field": {"id": 12, "component": "Health", "path": "current", "below": 30.0}})
        );
        assert_eq!(
            parse_condition("field:e5:Stamina.current>10").unwrap(),
            json!({"field": {"id": "e5", "component": "Stamina", "path": "current", "above": 10.0}})
        );
        assert_eq!(parse_condition(r#"{"gone": 3}"#).unwrap(), json!({"gone": 3}));
    }

    #[test]
    fn bad_conditions_say_what_is_missing() {
        assert_eq!(parse_condition("nope").unwrap_err(), "bad condition `nope`");
        assert_eq!(parse_condition("fly:1").unwrap_err(), "unknown condition kind `fly`");
        assert_eq!(parse_condition("near:1:2").unwrap_err(), "near:A:B:METERS");
        assert_eq!(parse_condition("field:1:Health.current").unwrap_err(), "field condition needs < or >");
        assert!(parse_condition("{broken").unwrap_err().starts_with("bad condition JSON"));
    }
}
