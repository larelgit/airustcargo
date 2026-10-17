//! Arguments made from a tool's JSON Schema: what `shards fuzz` and the tool contracts call tools
//! with. No Bevy: the schema is a `serde_json::Value` (what `shards tools --json` prints) and
//! the randomness is a seeded [`SplitMix`], so the same seed, schema and ids make the same
//! arguments.
//!
//! The generator walks the part of JSON Schema that `schemars` writes for the engine's argument
//! structs: objects with `required`, `enum` and `const`, `$ref` into `$defs`/`definitions`,
//! strings, numbers and integers (with `minimum`/`maximum` when they are there, else the
//! boundary values 0, 1, -1, 0.5 and 1e6), booleans, short arrays, `anyOf`/`oneOf`/`allOf` and
//! nullable types (`type: [x, "null"]`). A property that takes an entity id (the schema says
//! `format: "entity_id"`) gets one of the ids an observation lists, and now and then an id that
//! does not exist.
//!
//! [`invalid_args`] breaks exactly one argument on purpose (a wrong type, a value under its
//! `minimum`, an unknown enum value, a missing required property), so a test can ask what the
//! tool's error says about it.

use serde_json::{json, Map, Value};

use crate::inputs::SplitMix;

/// The `format` of a schema that takes an entity id (`Id` of `ai_core`).
pub const ENTITY_ID_FORMAT: &str = "entity_id";

/// An id no entity has: one call in [`MISSING_ID_ONE_IN`] names it instead of a listed id.
pub const MISSING_ID: &str = "e999999";
const MISSING_ID_ONE_IN: u64 = 50;

/// How deep a schema is followed (a recursive `$ref` ends here).
const MAX_DEPTH: usize = 8;

/// Strings a free-form text argument is drawn from: empty, short, with a space, non-ASCII, a
/// path that leaves its folder. Valid for the schema; whether the tool likes them is its business.
const STRINGS: [&str; 12] = ["", "a", "x1", "hello world", "../x", "ünï", "snapshots/fuzz.json", "N", "S", "E", "W", "NE"];

/// What the generator knows of the game besides the schema.
#[derive(Clone, Copy, Debug, Default)]
pub struct Hints<'a> {
    /// The entity ids the game lists (`observe`).
    pub ids: &'a [String],
    /// Words the game knows (content ids, ...): a free-form text argument is one of them half of
    /// the time, so that a tool that wants a prefab or a screen is sometimes given one.
    pub words: &'a [String],
}

impl<'a> Hints<'a> {
    /// Only entity ids.
    pub fn ids(ids: &'a [String]) -> Self {
        Self { ids, words: &[] }
    }
}

/// Arguments that satisfy `schema` (the tool's `input_schema`): an object with every required
/// property and each optional one half of the time. `ids` are the entity ids the game lists.
pub fn schema_args(schema: &Value, rng: &mut SplitMix, hints: Hints<'_>) -> Value {
    let mut generator = Gen { root: schema, rng, ids: hints.ids, words: hints.words };
    generator.object(schema, 0)
}

/// What [`invalid_args`] broke.
#[derive(Clone, Debug, PartialEq)]
pub struct Invalid {
    /// Arguments that break the schema in exactly one place.
    pub args: Value,
    /// The property that was broken; `None`: the arguments as a whole (not an object).
    pub argument: Option<String>,
    /// What was done to it: `wrong type`, `below its minimum`, `unknown enum value`, `missing`,
    /// `not an object`.
    pub how: &'static str,
    /// The same arguments before the break: what the call was meant to be.
    pub valid: Value,
}

/// Arguments that satisfy `schema` except for one property, which is made invalid on purpose.
/// A schema with nothing to break (no properties) gets arguments that are not an object.
pub fn invalid_args(schema: &Value, rng: &mut SplitMix, hints: Hints<'_>) -> Invalid {
    let mut options = invalid_options(schema, rng, hints);
    let pick = rng.below(options.len() as u64) as usize;
    options.swap_remove(pick)
}

/// Every way to break exactly one thing in arguments of `schema` (they share one valid draw
/// for the rest): each property missing, wrong, under its minimum or an unknown variant, and
/// arguments that are not an object at all. Never empty. For a caller that wants to try each.
pub fn invalid_options(schema: &Value, rng: &mut SplitMix, hints: Hints<'_>) -> Vec<Invalid> {
    let valid = schema_args(schema, rng, hints);
    let root = schema;
    let required: Vec<&str> = schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut options: Vec<Invalid> = Vec::new();
    if let (Some(props), Some(object)) = (schema["properties"].as_object(), valid.as_object()) {
        for (name, property) in props {
            let property = resolve(root, property);
            if required.contains(&name.as_str()) {
                let mut args = object.clone();
                args.remove(name);
                options.push(Invalid { args: Value::Object(args), argument: Some(name.clone()), how: "missing", valid: valid.clone() });
            }
            for (how, bad) in breaks(root, property) {
                let mut args = object.clone();
                args.insert(name.clone(), bad);
                options.push(Invalid { args: Value::Object(args), argument: Some(name.clone()), how, valid: valid.clone() });
            }
        }
    }
    // Arguments that are not an object break every tool (and the only thing a tool without
    // properties has to break).
    options.push(Invalid { args: json!("not an object"), argument: None, how: "not an object", valid: valid.clone() });
    options
}

/// Values that break `property`'s schema, each with what was done.
fn breaks(root: &Value, property: &Value) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    if property.get("format").and_then(Value::as_str) == Some(ENTITY_ID_FORMAT) {
        // An id is a number or text; a flag is neither.
        out.push(("wrong type", json!(true)));
        return out;
    }
    if let Some(values) = property.get("enum").and_then(Value::as_array) {
        if values.iter().all(Value::is_string) {
            out.push(("unknown enum value", json!("__not_a_variant__")));
        }
        return out;
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(options) = property.get(key).and_then(Value::as_array) {
            let all_const = options.iter().all(|o| resolve(root, o).get("const").is_some_and(Value::is_string));
            if all_const {
                out.push(("unknown enum value", json!("__not_a_variant__")));
            } else {
                // Nothing a branch takes: a nested object with a key nobody has, in a list.
                out.push(("wrong type", json!([{"__not_a_field__": [null]}])));
            }
            return out;
        }
    }
    let kinds = types_of(property);
    if kinds.contains(&"string") {
        out.push(("wrong type", json!(12345)));
    } else if kinds.contains(&"integer") || kinds.contains(&"number") {
        out.push(("wrong type", json!("not a number")));
        if let Some(min) = property.get("minimum").and_then(Value::as_i64) {
            out.push(("below its minimum", json!(min - 1)));
        }
    } else if kinds.contains(&"boolean") {
        out.push(("wrong type", json!("not a flag")));
    } else if kinds.contains(&"array") {
        out.push(("wrong type", json!("not a list")));
    } else if kinds.contains(&"object") {
        out.push(("wrong type", json!("not an object")));
    }
    out
}

/// The non-null JSON types a schema allows (`type` is a name or a list of names).
fn types_of(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).filter(|t| *t != "null").collect(),
        _ => {
            if schema.get("properties").is_some() {
                vec!["object"]
            } else if schema.get("items").is_some() {
                vec!["array"]
            } else {
                Vec::new()
            }
        }
    }
}

/// Follows `$ref` (`#/$defs/Name`, `#/definitions/Name`) and a one-branch `allOf` (what `schemars`
/// writes around a reference that has a doc comment of its own) to the schema they name.
fn resolve<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    let mut current = schema;
    for _ in 0..MAX_DEPTH {
        if let Some(target) = current.get("$ref").and_then(Value::as_str) {
            match target.strip_prefix('#').and_then(|pointer| root.pointer(pointer)) {
                Some(found) => {
                    current = found;
                    continue;
                }
                None => return current,
            }
        }
        match current.get("allOf").and_then(Value::as_array) {
            Some(parts) if parts.len() == 1 => current = &parts[0],
            _ => return current,
        }
    }
    current
}

struct Gen<'a> {
    root: &'a Value,
    rng: &'a mut SplitMix,
    ids: &'a [String],
    words: &'a [String],
}

impl Gen<'_> {
    fn coin(&mut self) -> bool {
        self.rng.below(2) == 0
    }

    fn pick<'s, T>(&mut self, items: &'s [T]) -> &'s T {
        &items[self.rng.below(items.len() as u64) as usize]
    }

    fn value(&mut self, schema: &Value, depth: usize) -> Value {
        let schema = resolve(self.root, schema);
        if depth > MAX_DEPTH {
            return Value::Null;
        }
        match schema {
            Value::Bool(true) => return self.any(),
            Value::Bool(false) => return Value::Null,
            _ => {}
        }
        if let Some(value) = schema.get("const") {
            return value.clone();
        }
        if let Some(values) = schema.get("enum").and_then(Value::as_array)
            && !values.is_empty()
        {
            return self.pick(values).clone();
        }
        if schema.get("format").and_then(Value::as_str) == Some(ENTITY_ID_FORMAT) {
            return self.id(schema);
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(options) = schema.get(key).and_then(Value::as_array)
                && !options.is_empty()
            {
                let option = self.pick(options).clone();
                return self.value(&option, depth + 1);
            }
        }
        if let Some(parts) = schema.get("allOf").and_then(Value::as_array)
            && let Some(first) = parts.first()
        {
            let first = first.clone();
            return self.value(&first, depth + 1);
        }
        let nullable = schema.get("type").and_then(Value::as_array).is_some_and(|ts| ts.iter().any(|t| t == "null"));
        let kinds = types_of(schema);
        if kinds.is_empty() {
            return if nullable { Value::Null } else { self.any() };
        }
        if nullable && self.rng.below(10) == 0 {
            return Value::Null;
        }
        let kind = *self.pick(&kinds);
        match kind {
            "object" => self.object(schema, depth),
            "array" => self.array(schema, depth),
            "string" => self.string(schema),
            "integer" => self.number(schema, true),
            "number" => self.number(schema, false),
            "boolean" => json!(self.coin()),
            _ => Value::Null,
        }
    }

    /// A value for a schema that says nothing about it.
    fn any(&mut self) -> Value {
        match self.rng.below(4) {
            0 => json!(0),
            1 => json!("a"),
            2 => json!(true),
            _ => json!([]),
        }
    }

    /// One of the listed ids, or (one call in fifty, and always when none is listed) one that
    /// no entity has. An id is a number or text; text (`e6`) is what an observation prints.
    fn id(&mut self, schema: &Value) -> Value {
        let nullable = schema.get("type").and_then(Value::as_array).is_some_and(|ts| ts.iter().any(|t| t == "null"));
        if nullable && self.rng.below(10) == 0 {
            return Value::Null;
        }
        if self.ids.is_empty() || self.rng.below(MISSING_ID_ONE_IN) == 0 {
            return json!(MISSING_ID);
        }
        json!(self.pick(self.ids))
    }

    fn string(&mut self, schema: &Value) -> Value {
        let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
        let max = schema.get("maxLength").and_then(Value::as_u64).map_or(usize::MAX, |m| m as usize);
        let known = |s: &str| (min..=max).contains(&s.chars().count());
        // A word of the game, half of the time when there are any.
        if !self.words.is_empty() && self.coin() {
            let fitting: Vec<&String> = self.words.iter().filter(|w| known(w)).collect();
            if !fitting.is_empty() {
                return json!(self.pick(&fitting));
            }
        }
        let fits: Vec<&str> = STRINGS.iter().copied().filter(|s| known(s)).collect();
        if let Some(text) = fits.first().map(|_| *self.pick(&fits)) {
            return json!(text);
        }
        // Nothing of the list is the right length: pad to the shortest allowed.
        json!("x".repeat(min.min(max).min(64)))
    }

    fn number(&mut self, schema: &Value, integer: bool) -> Value {
        let bound = |keys: [&str; 2], exclusive_step: f64| -> Option<f64> {
            if let Some(v) = schema.get(keys[0]).and_then(Value::as_f64) {
                return Some(v);
            }
            schema.get(keys[1]).and_then(Value::as_f64).map(|v| v + exclusive_step)
        };
        let min = bound(["minimum", "exclusiveMinimum"], if integer { 1.0 } else { 0.001 });
        let max = bound(["maximum", "exclusiveMaximum"], if integer { -1.0 } else { -0.001 });
        let mut candidates: Vec<f64> = vec![0.0, 1.0, -1.0, 1e6];
        if !integer {
            candidates.extend([0.5, -0.5]);
        }
        candidates.extend(min);
        candidates.extend(max);
        if let (Some(lo), Some(hi)) = (min, max)
            && hi >= lo
        {
            // Inside the range too, not only on its edges.
            let span = hi - lo;
            let inside = lo + span * (self.rng.below(1001) as f64 / 1000.0);
            candidates.push(if integer { inside.round() } else { inside });
        }
        if integer {
            candidates.push(2.0 + self.rng.below(19) as f64);
        }
        candidates.retain(|c| min.is_none_or(|m| *c >= m) && max.is_none_or(|m| *c <= m));
        let chosen = if candidates.is_empty() { min.or(max).unwrap_or(0.0) } else { *self.pick(&candidates) };
        if integer {
            json!(chosen.round() as i64)
        } else {
            json!(chosen)
        }
    }

    fn array(&mut self, schema: &Value, depth: usize) -> Value {
        let min = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
        let max = schema.get("maxItems").and_then(Value::as_u64).map_or(3, |m| m as usize).min(3).max(min);
        let len = min + self.rng.below((max - min + 1) as u64) as usize;
        if let Some(prefix) = schema.get("prefixItems").and_then(Value::as_array) {
            return Value::Array(prefix.clone().iter().map(|item| self.value(item, depth + 1)).collect());
        }
        let items = schema.get("items").cloned().unwrap_or(Value::Bool(true));
        Value::Array((0..len).map(|_| self.value(&items, depth + 1)).collect())
    }

    fn object(&mut self, schema: &Value, depth: usize) -> Value {
        let schema = resolve(self.root, schema);
        let required: Vec<&str> = schema["required"].as_array().map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let mut out = Map::new();
        if let Some(props) = schema["properties"].as_object() {
            for (name, property) in props {
                if required.contains(&name.as_str()) || self.coin() {
                    let property = property.clone();
                    let value = self.value(&property, depth + 1);
                    out.insert(name.clone(), value);
                }
            }
        }
        Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: u64) -> SplitMix {
        SplitMix(seed)
    }

    fn ids() -> Vec<String> {
        vec!["e6".into(), "e7".into()]
    }

    /// `n` argument sets for `schema`.
    fn many(schema: &Value, n: usize) -> Vec<Value> {
        let mut r = rng(11);
        (0..n).map(|_| schema_args(schema, &mut r, Hints::ids(&ids()))).collect()
    }

    #[test]
    fn required_properties_are_always_there_and_optional_ones_sometimes() {
        let schema = json!({"type": "object", "properties": {"a": {"type": "integer"}, "b": {"type": "boolean"}}, "required": ["a"]});
        let sets = many(&schema, 100);
        assert!(sets.iter().all(|s| s["a"].is_i64()), "{sets:?}");
        assert!(sets.iter().any(|s| s.get("b").is_some()) && sets.iter().any(|s| s.get("b").is_none()));
    }

    #[test]
    fn enums_and_constants_take_a_listed_value() {
        let schema = json!({"type": "object", "required": ["dir", "k"], "properties": {
            "dir": {"type": "string", "enum": ["N", "S"]},
            "k": {"oneOf": [{"const": "x", "description": "ex"}, {"const": "y"}]}}});
        let sets = many(&schema, 60);
        assert!(sets.iter().all(|s| ["N", "S"].contains(&s["dir"].as_str().unwrap()) && ["x", "y"].contains(&s["k"].as_str().unwrap())));
        assert!(sets.iter().any(|s| s["dir"] == "N") && sets.iter().any(|s| s["dir"] == "S"));
    }

    #[test]
    fn references_are_followed() {
        let schema = json!({"type": "object", "required": ["mode", "n"],
            "properties": {"mode": {"$ref": "#/$defs/Mode"}, "n": {"allOf": [{"$ref": "#/definitions/Count"}], "description": "how many"}},
            "$defs": {"Mode": {"type": "string", "enum": ["fast", "slow"]}},
            "definitions": {"Count": {"type": "integer", "minimum": 3, "maximum": 5}}});
        for s in many(&schema, 80) {
            assert!(["fast", "slow"].contains(&s["mode"].as_str().unwrap()), "{s}");
            assert!((3..=5).contains(&s["n"].as_i64().unwrap()), "{s}");
        }
    }

    #[test]
    fn numbers_keep_to_their_bounds_and_use_the_boundary_values_without_them() {
        let bounded = json!({"type": "object", "required": ["x"], "properties": {"x": {"type": "number", "minimum": -2.0, "maximum": 2.0}}});
        for s in many(&bounded, 100) {
            assert!((-2.0..=2.0).contains(&s["x"].as_f64().unwrap()), "{s}");
        }
        let unsigned = json!({"type": "object", "required": ["n"], "properties": {"n": {"type": "integer", "minimum": 0}}});
        let sets = many(&unsigned, 100);
        assert!(sets.iter().all(|s| s["n"].as_i64().is_some_and(|n| n >= 0)), "{sets:?}");
        let free = json!({"type": "object", "required": ["f", "i"], "properties": {"f": {"type": "number"}, "i": {"type": "integer"}}});
        let sets = many(&free, 300);
        let floats: Vec<f64> = sets.iter().map(|s| s["f"].as_f64().unwrap()).collect();
        for edge in [0.0, 1.0, -1.0, 0.5, 1e6] {
            assert!(floats.contains(&edge), "{edge} never drawn: {floats:?}");
        }
        assert!(sets.iter().all(|s| s["i"].is_i64()), "an integer is a whole number");
    }

    #[test]
    fn strings_booleans_arrays_and_nullable_types() {
        let schema = json!({"type": "object", "required": ["s", "b", "list", "maybe"], "properties": {
            "s": {"type": "string", "minLength": 2, "maxLength": 5},
            "b": {"type": "boolean"},
            "list": {"type": "array", "items": {"type": "integer", "minimum": 1, "maximum": 1}, "maxItems": 2},
            "maybe": {"type": ["string", "null"]}}});
        let sets = many(&schema, 200);
        for s in &sets {
            let len = s["s"].as_str().unwrap().chars().count();
            assert!((2..=5).contains(&len), "{s}");
            assert!(s["b"].is_boolean());
            let list = s["list"].as_array().unwrap();
            assert!(list.len() <= 2 && list.iter().all(|v| *v == 1), "{s}");
        }
        assert!(sets.iter().any(|s| s["maybe"].is_null()) && sets.iter().any(|s| s["maybe"].is_string()));
        assert!(sets.iter().any(|s| s["b"] == true) && sets.iter().any(|s| s["b"] == false));
    }

    #[test]
    fn any_of_takes_one_branch() {
        let schema = json!({"type": "object", "required": ["t"], "properties": {"t": {"anyOf": [
            {"type": "integer", "minimum": 7, "maximum": 7}, {"type": "string", "enum": ["p"]}]}}});
        let sets = many(&schema, 60);
        assert!(sets.iter().all(|s| s["t"] == 7 || s["t"] == "p"));
        assert!(sets.iter().any(|s| s["t"] == 7) && sets.iter().any(|s| s["t"] == "p"));
    }

    #[test]
    fn an_entity_id_is_one_that_was_seen_and_rarely_one_that_was_not() {
        let id = json!({"type": ["integer", "string"], "format": ENTITY_ID_FORMAT});
        let schema = json!({"type": "object", "required": ["id"], "properties": {"id": id,
            "maybe": {"anyOf": [{"type": ["integer", "string"], "format": ENTITY_ID_FORMAT}, {"type": "null"}]}}});
        let sets = many(&schema, 2000);
        let drawn: Vec<&str> = sets.iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert!(drawn.contains(&"e6") && drawn.contains(&"e7"));
        let missing = drawn.iter().filter(|d| **d == MISSING_ID).count();
        assert!((10..=80).contains(&missing), "about 1 in 50 names a missing id, got {missing} in 2000");
        // Without ids in sight, the only id there is is a missing one.
        let mut r = rng(1);
        assert_eq!(schema_args(&schema, &mut r, Hints::default())["id"], MISSING_ID);
        // An id inside a branch is found too.
        assert!(sets.iter().any(|s| s.get("maybe").is_some_and(|m| m == "e6")));
    }


    #[test]
    fn text_is_sometimes_a_word_the_game_knows() {
        let schema = json!({"type": "object", "required": ["name"], "properties": {"name": {"type": "string", "minLength": 3}}});
        let words = vec!["skeleton".to_owned(), "ab".to_owned()];
        let mut r = rng(4);
        let drawn: Vec<Value> = (0..200).map(|_| schema_args(&schema, &mut r, Hints { ids: &[], words: &words })).collect();
        let known = drawn.iter().filter(|s| s["name"] == "skeleton").count();
        assert!((60..=140).contains(&known), "about half, got {known} of 200");
        assert!(drawn.iter().all(|s| s["name"].as_str().unwrap().chars().count() >= 3), "a word too short for the schema is not used");
    }
    #[test]
    fn recursion_ends() {
        let schema = json!({"type": "object", "required": ["child"], "properties": {"child": {"$ref": "#/$defs/Node"}},
            "$defs": {"Node": {"type": "object", "required": ["child"], "properties": {"child": {"$ref": "#/$defs/Node"}}}}});
        assert!(schema_args(&schema, &mut rng(1), Hints::default()).is_object());
    }

    #[test]
    fn the_same_seed_makes_the_same_arguments() {
        let schema = json!({"type": "object", "required": ["id", "n"], "properties": {
            "id": {"type": ["integer", "string"], "format": ENTITY_ID_FORMAT}, "n": {"type": "number"}, "s": {"type": "string"}}});
        let run = |seed| {
            let mut r = rng(seed);
            (0..30).map(|_| schema_args(&schema, &mut r, Hints::ids(&ids()))).collect::<Vec<_>>()
        };
        assert_eq!(run(5), run(5));
        assert_ne!(run(5), run(6));
    }

    #[test]
    fn invalid_arguments_break_exactly_one_thing() {
        let schema = json!({"type": "object", "required": ["id", "n", "dir"], "properties": {
            "id": {"type": ["integer", "string"], "format": ENTITY_ID_FORMAT},
            "n": {"type": "integer", "minimum": 0},
            "dir": {"type": "string", "enum": ["N", "S"]},
            "flag": {"type": "boolean"}}});
        let mut r = rng(3);
        let mut hows = std::collections::BTreeSet::new();
        for _ in 0..300 {
            let bad = invalid_args(&schema, &mut r, Hints::ids(&ids()));
            hows.insert(bad.how);
            match (&bad.argument, bad.how) {
                (None, how) => {
                    assert_eq!(how, "not an object");
                    assert!(!bad.args.is_object());
                }
                (Some(name), "missing") => assert!(bad.args.get(name).is_none(), "{bad:?}"),
                (Some(name), "wrong type") => match name.as_str() {
                    "id" => assert_eq!(bad.args["id"], true),
                    "n" | "flag" | "dir" => assert!(bad.args[name].is_string() || bad.args[name].is_number() || bad.args[name].is_boolean()),
                    _ => unreachable!(),
                },
                (Some(name), "below its minimum") => assert!(bad.args[name].as_i64().unwrap() < 0),
                (Some(name), "unknown enum value") => assert_eq!((name.as_str(), &bad.args["dir"]), ("dir", &json!("__not_a_variant__"))),
                other => panic!("{other:?}"),
            }
        }
        for how in ["missing", "wrong type", "below its minimum", "unknown enum value", "not an object"] {
            assert!(hows.contains(how), "never drew `{how}`: {hows:?}");
        }
        // A tool without arguments can still be called wrongly.
        let none = json!({"type": "object", "properties": {}});
        assert_eq!(invalid_args(&none, &mut r, Hints::default()).how, "not an object");
    }
}
