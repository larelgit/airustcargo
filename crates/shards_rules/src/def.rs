//! The rules' content: the kinds `rule_set` (`*.rule_set.ron`, one definition per mode) and `rules`
//! (`*.rules.ron`, the one definition `default`), their validation, and the prefab check of
//! [`RuleClass`](crate::RuleClass).

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{ContentDef, PrefabDef, Refs};

use crate::state::RuleClass;
use crate::{PLAYER_CLASS, RULES_ID};

/// The largest factor a rule set or a session row may give (1 changes nothing).
pub const MAX_FACTOR: f32 = 10.0;
/// The longest perfect-window bonus (s).
pub const MAX_BONUS: f32 = 1.0;
/// The largest max of a named charge.
pub const MAX_CHARGE: u32 = 255;
/// The longest charge name, in characters (a save names charges the same way).
pub const MAX_CHARGE_NAME: usize = 64;
/// The longest rule set id, in bytes (a save names the rule set in play by its id and refuses a longer one).
pub const MAX_MODE: usize = 64;
/// The largest shift of a director's attack tokens, either way.
pub const MAX_TOKEN_SHIFT: i32 = 8;
/// The largest `attack_tokens.min`.
pub const MAX_TOKEN_MIN: u32 = 64;
/// The most entries a session row's list may have (entry n − 1 is for n players).
pub const MAX_SESSION_ENTRIES: usize = 8;

fn one() -> f32 {
    1.0
}

fn is_one(x: &f32) -> bool {
    *x == 1.0
}

fn is_zero(x: &f32) -> bool {
    *x == 0.0
}

fn is_zero_i32(x: &i32) -> bool {
    *x == 0
}

fn one_u32() -> u32 {
    1
}

fn is_one_u32(x: &u32) -> bool {
    *x == 1
}

/// What one mode changes. A class it does not name, and every field left out, keeps the engine's numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleSetDef {
    /// What the player reads: a Fluent id (`@rules-gentle`).
    pub name: String,
    /// One line on what it changes (`@rules-gentle-summary`); empty: none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// By class (`player`, or a `RuleClass`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub classes: BTreeMap<String, ClassRules>,
    /// Attack tokens of every director: the table's limit shifted, at least `min` (a limit of 0 stays 0).
    #[serde(default, skip_serializing_if = "TokenRule::is_neutral")]
    pub attack_tokens: TokenRule,
    /// The rest an NPC takes after an attack it finished (not one cut short), × the drawn rest.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub attack_rest: f32,
}

impl Default for RuleSetDef {
    /// A rule set that changes nothing (written by hand: the rest factor is 1, not `f32::default`).
    fn default() -> Self {
        Self { name: String::new(), summary: String::new(), classes: BTreeMap::new(), attack_tokens: TokenRule::default(), attack_rest: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClassRules {
    /// Health damage a body of the class takes from hits, ×: a hit, a block's chip, a guard break; and what a game's
    /// `deal_ruled_damage` deals it. Not `deal_damage`, and not poise.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub incoming_damage: f32,
    /// Health and poise damage its hits deal, ×, applied after the damage hooks (a body's power).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub outgoing_damage: f32,
    /// `Health::max` = base × this × the session row's; the share of health is kept.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub max_health: f32,
    /// `Poise::max` = base × this × the session row's; the share kept, a break untouched.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub max_poise: f32,
    /// Stamina an action costs, ×: the product of every rule one of whose tags is the action's tag or id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stamina_cost: Vec<CostRule>,
    /// Seconds added to the end of a guard's perfect window (0 to 1).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub perfect_bonus: f32,
    /// The `max` of named charges (`{"flask": 2}`): charges spent stay spent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub charges: BTreeMap<String, u32>,
}

/// The rules of a class no rule set names: every factor 1, no cost rules, no bonus, no charges.
pub static NEUTRAL_CLASS: ClassRules = ClassRules {
    incoming_damage: 1.0,
    outgoing_damage: 1.0,
    max_health: 1.0,
    max_poise: 1.0,
    stamina_cost: Vec::new(),
    perfect_bonus: 0.0,
    charges: BTreeMap::new(),
};

impl Default for ClassRules {
    /// Every factor 1, no cost rules, no bonus, no charges (written by hand: `one()` is not `f32::default`).
    fn default() -> Self {
        NEUTRAL_CLASS.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CostRule {
    /// Matched against the action's `tags` and its id (`roll`, `block`).
    pub tags: Vec<String>,
    pub factor: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenRule {
    /// Added to a director's limit; 0: the limit as the table gives it.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub shift: i32,
    /// With a shift, never fewer tokens than this; without one it is not read (validation refuses it then).
    #[serde(default = "one_u32", skip_serializing_if = "is_one_u32")]
    pub min: u32,
}

impl Default for TokenRule {
    /// No shift (`min` 1, unread).
    fn default() -> Self {
        Self { shift: 0, min: 1 }
    }
}

impl TokenRule {
    /// The table's limit as it is: no shift.
    pub fn is_neutral(&self) -> bool {
        self.shift == 0
    }
}

/// The game's rules: the rule sets in order, where a new world starts, the classes, the session table and the
/// texts of a refused change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesDef {
    /// The rule sets, in the order a menu lists them and `cycle` steps through them.
    pub modes: Vec<String>,
    /// The rule set a new world starts with (never the previous world's).
    pub start: String,
    /// The classes a rule set, a session row and a `RuleClass` may name; `player` is always one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classes: Vec<String>,
    /// Factors by session size, by class: entry n − 1 is for n players; more players than entries: the last.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub session: BTreeMap<String, SessionRow>,
    /// What a player reads when a guard refuses a change (`@rules-blocked`); empty: nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub blocked: String,
    /// What a co-op guest reads: the host chooses (`@rules-host-only`); empty: nothing.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_only: String,
}

impl RulesDef {
    /// Whether `class` is one the rules know: `player`, or one of `classes`.
    pub fn knows_class(&self, class: &str) -> bool {
        class == PLAYER_CLASS || self.classes.iter().any(|c| c == class)
    }

    /// Every class the rules know, `player` first: `player, brute`.
    pub fn class_names(&self) -> Vec<&str> {
        let mut names = vec![PLAYER_CLASS];
        names.extend(self.classes.iter().map(String::as_str).filter(|c| *c != PLAYER_CLASS));
        names
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRow {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub max_health: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub max_poise: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outgoing_damage: Vec<f32>,
}

impl SessionRow {
    /// The three columns, named as content writes them.
    pub fn columns(&self) -> [(&'static str, &[f32]); 3] {
        [("max_health", &self.max_health), ("max_poise", &self.max_poise), ("outgoing_damage", &self.outgoing_damage)]
    }
}

impl ContentDef for RuleSetDef {
    const KIND: &'static str = "rule_set";

    fn validate(&self, id: &str, refs: &mut Refs) {
        let rules = refs.get::<RulesDef>(RULES_ID);
        if id.len() > MAX_MODE {
            refs.error(format!(
                "rule set id `{id}` is {} bytes: an id is at most {MAX_MODE} bytes (a save names the rule set in play by it): shorten it",
                id.len()
            ));
        }
        match rules {
            None => refs.error(format!(
                "rule sets need `content/**/*.rules.ron` with the definition `{RULES_ID}`, which lists them in `modes`"
            )),
            Some(rules) if !rules.modes.iter().any(|m| m == id) => {
                refs.error(format!("rule set `{id}` is not in `rules/{RULES_ID}` `modes`: add it there or delete it"));
            }
            Some(_) => {}
        }
        if self.name.trim().is_empty() {
            refs.error("`name` is empty: a rule set has a name the player reads (a Fluent id, `@rules-gentle`)");
        }
        shards_locale::check_messages(refs, [(self.name.as_str(), "`name`".to_owned()), (self.summary.as_str(), "`summary`".to_owned())]);
        for (class, rules_of) in &self.classes {
            if let Some(rules) = rules
                && !rules.knows_class(class)
            {
                let names = rules.class_names();
                let hint = closest(class, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                refs.error(format!("class `{class}` is not one of `rules/{RULES_ID}` `classes`{hint}: {}", names.join(", ")));
            }
            check_class(class, rules_of, refs);
        }
        let tokens = self.attack_tokens;
        if !(-MAX_TOKEN_SHIFT..=MAX_TOKEN_SHIFT).contains(&tokens.shift) {
            refs.error(format!("`attack_tokens.shift` {}: a shift is from -{MAX_TOKEN_SHIFT} to {MAX_TOKEN_SHIFT}", tokens.shift));
        }
        if tokens.min > MAX_TOKEN_MIN {
            refs.error(format!("`attack_tokens.min` {}: a minimum is from 0 to {MAX_TOKEN_MIN}", tokens.min));
        }
        if tokens.shift == 0 && tokens.min != 1 {
            refs.error(format!("`attack_tokens.min` {} without a `shift` changes nothing: give a shift or leave `min` out", tokens.min));
        }
        if let Some(problem) = factor_problem(self.attack_rest) {
            refs.error(format!("`attack_rest` {}: {problem}", self.attack_rest));
        }
    }
}

/// What is wrong with `factor`, if anything.
fn factor_problem(factor: f32) -> Option<String> {
    (!(factor.is_finite() && factor > 0.0 && factor <= MAX_FACTOR))
        .then(|| format!("a factor is above 0 and at most {MAX_FACTOR} (1 changes nothing)"))
}

/// What is wrong with a charge's name, if anything (as a save checks one).
fn charge_name_problem(name: &str) -> Option<String> {
    let chars = name.chars().count();
    (chars == 0 || chars > MAX_CHARGE_NAME || name.chars().any(char::is_control))
        .then(|| format!("a charge's name is 1 to {MAX_CHARGE_NAME} characters with no control characters"))
}

fn check_class(class: &str, rules: &ClassRules, refs: &mut Refs) {
    for (field, factor) in [
        ("incoming_damage", rules.incoming_damage),
        ("outgoing_damage", rules.outgoing_damage),
        ("max_health", rules.max_health),
        ("max_poise", rules.max_poise),
    ] {
        if let Some(problem) = factor_problem(factor) {
            refs.error(format!("`{field}` {factor} of class `{class}`: {problem}"));
        }
    }
    let bonus = rules.perfect_bonus;
    if !(bonus.is_finite() && (0.0..=MAX_BONUS).contains(&bonus)) {
        refs.error(format!("`perfect_bonus` {bonus} of class `{class}`: a bonus is from 0 to {MAX_BONUS} seconds"));
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut doubled: BTreeSet<&str> = BTreeSet::new();
    for (i, rule) in rules.stamina_cost.iter().enumerate() {
        if let Some(problem) = factor_problem(rule.factor) {
            refs.error(format!("`stamina_cost[{i}].factor` {} of class `{class}`: {problem}", rule.factor));
        }
        if rule.tags.is_empty() {
            refs.error(format!("class `{class}`: `stamina_cost[{i}]` names no tag: give it the tags or ids of the actions it scales (`roll`)"));
        }
        if rule.tags.iter().any(|t| t.trim().is_empty()) {
            refs.error(format!("class `{class}`: `stamina_cost[{i}]` has an empty tag"));
        }
        let tags: BTreeSet<&str> = rule.tags.iter().map(String::as_str).filter(|t| !t.trim().is_empty()).collect();
        for tag in tags {
            if !seen.insert(tag) && doubled.insert(tag) {
                refs.error(format!("class `{class}`: tag `{tag}` is in two stamina rules: merge them"));
            }
        }
    }
    for (name, max) in &rules.charges {
        if let Some(problem) = charge_name_problem(name) {
            refs.error(format!("class `{class}`: charge `{name}`: {problem}"));
        }
        if !(1..=MAX_CHARGE).contains(max) {
            refs.error(format!("class `{class}`: charge `{name}` max {max}: a charge's max is from 1 to {MAX_CHARGE}"));
        }
    }
}

impl ContentDef for RulesDef {
    const KIND: &'static str = "rules";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if id != RULES_ID {
            refs.error(format!("the rules are one definition, `{RULES_ID}`; `{id}` would be a second"));
        }
        if self.modes.is_empty() {
            refs.error("`modes` is empty: list the rule sets, in the order a menu shows them");
        }
        let mut modes: BTreeSet<&str> = BTreeSet::new();
        for mode in &self.modes {
            if !modes.insert(mode) {
                refs.error(format!("`modes` names `{mode}` twice"));
            }
        }
        for mode in &modes {
            refs.require::<RuleSetDef>(mode, "modes");
        }
        if !modes.contains(self.start.as_str()) {
            let hint = closest(&self.start, self.modes.iter().map(String::as_str)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            refs.error(format!("`start` `{}` is not in `modes`{hint}: {}", self.start, self.modes.join(", ")));
        }
        let mut classes: BTreeSet<&str> = BTreeSet::new();
        for class in &self.classes {
            if !classes.insert(class) {
                refs.error(format!("`classes` names `{class}` twice"));
            }
            if class.is_empty() || !class.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_') {
                refs.error(format!("class `{class}`: a class is lowercase letters, digits and `_`"));
            }
        }
        let names = self.class_names();
        for (class, row) in &self.session {
            if !self.knows_class(class) {
                let hint = closest(class, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                refs.error(format!("`session` row `{class}` is not a class{hint}: {}", names.join(", ")));
            }
            // A column left out and an empty one read the same: a row needs one with entries (1 to 8).
            if row.columns().iter().all(|(_, list)| list.is_empty()) {
                refs.error(format!("`session.{class}` gives no column: give `max_health`, `max_poise` or `outgoing_damage`, or remove the row"));
            }
            for (column, list) in row.columns() {
                if list.len() > MAX_SESSION_ENTRIES {
                    refs.error(format!(
                        "`session.{class}.{column}` has {} entries: at most {MAX_SESSION_ENTRIES} (entry n − 1 is for n players)",
                        list.len()
                    ));
                }
                for (i, factor) in list.iter().enumerate() {
                    if let Some(problem) = factor_problem(*factor) {
                        refs.error(format!("`session.{class}.{column}[{i}]` {factor}: {problem}"));
                    }
                }
            }
        }
        shards_locale::check_messages(refs, [(self.blocked.as_str(), "`blocked`".to_owned()), (self.host_only.as_str(), "`host_only`".to_owned())]);
        // Every rule set names the same charges for a class: the rules keep no base for a charge, so a mode that
        // left one out would leave its max undefined after a change.
        let sets: Vec<(&str, &RuleSetDef)> = self.modes.iter().filter_map(|m| refs.get::<RuleSetDef>(m).map(|s| (m.as_str(), s))).collect();
        let mut named: BTreeMap<(&str, &str), &str> = BTreeMap::new();
        for (mode, set) in &sets {
            for (class, rules) in &set.classes {
                for charge in rules.charges.keys() {
                    named.entry((class.as_str(), charge.as_str())).or_insert(mode);
                }
            }
        }
        for ((class, charge), by) in named {
            for (mode, set) in &sets {
                if !set.classes.get(class).is_some_and(|r| r.charges.contains_key(charge)) {
                    refs.error(format!(
                        "class `{class}`: charge `{charge}` is set by `{by}` but not by `{mode}`: every rule set sets the same charges"
                    ));
                }
            }
        }
    }
}

/// Whether a prefab's component `name` is [`RuleClass`]: by the type registry when there is one, else by its short
/// name or its full type path.
fn is_rule_class(refs: &Refs, name: &str) -> bool {
    match refs.type_registry() {
        Some(registry) => shards_content::prefab::resolve_component(registry, name)
            .is_ok_and(|reg| reg.type_id() == std::any::TypeId::of::<RuleClass>()),
        None => name == "RuleClass" || name == "shards_rules::state::RuleClass",
    }
}

/// The content check `rule_class`: a prefab's `RuleClass` names a class of `rules/default`.
pub fn check_prefab(prefab: &PrefabDef, _id: &str, refs: &mut Refs) {
    for (name, value) in &prefab.components {
        if !is_rule_class(refs, name) {
            continue;
        }
        // A value that does not read is the prefab's own error where there is a type registry to read it by;
        // without one (a bare `check_dir`) nobody else reads it, so it is said here.
        let class = match ron::from_str::<RuleClass>(value.get_ron()) {
            Ok(class) => class,
            Err(_) if refs.type_registry().is_some() => continue,
            Err(e) => {
                refs.error(format!("RuleClass: {e}: a class is a string in parentheses (`(\"brute\")`)"));
                continue;
            }
        };
        let Some(rules) = refs.get::<RulesDef>(RULES_ID) else {
            refs.error(format!("RuleClass needs `content/**/*.rules.ron` (definition `{RULES_ID}`) naming the class"));
            continue;
        };
        if !rules.knows_class(&class.0) {
            let names = rules.class_names();
            let hint = closest(&class.0, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            refs.error(format!("RuleClass `{}` is not a class of `rules/{RULES_ID}`{hint}: {}", class.0, names.join(", ")));
        }
    }
}

#[cfg(test)]
mod tests {
    use shards_content::db::Source;
    use shards_content::{ContentDb, ContentKinds};

    use super::*;

    /// The rules and three rule sets of the tests: `gentle`, `plain` and `harsh`, class `brute`, charge `flask`.
    pub(crate) const RULES: &str = r#"{
        "default": (
            modes: ["gentle", "plain", "harsh"],
            start: "plain",
            classes: ["brute"],
            session: {"brute": (max_health: [1.0, 1.5])},
        ),
    }"#;

    pub(crate) const SETS: &str = r#"{
        "gentle": (
            name: "Gentle",
            classes: {
                "player": (incoming_damage: 0.5, perfect_bonus: 0.05, stamina_cost: [(tags: ["roll"], factor: 0.5)], charges: {"flask": 3}),
                "brute": (max_health: 0.5),
            },
        ),
        "plain": (name: "Plain", classes: {"player": (charges: {"flask": 3})}),
        "harsh": (
            name: "Harsh",
            classes: {"player": (incoming_damage: 1.5, charges: {"flask": 2}), "brute": (max_health: 1.5)},
            attack_tokens: (shift: 1),
            attack_rest: 0.5,
        ),
    }"#;

    fn kinds() -> ContentKinds {
        let mut kinds = ContentKinds::default();
        kinds.register::<RuleSetDef>();
        kinds.register::<RulesDef>();
        kinds.register::<PrefabDef>();
        kinds.add_check::<PrefabDef>("rule_class", check_prefab);
        kinds
    }

    fn errors(files: &[(&str, &str)]) -> Vec<String> {
        let sources: Vec<Source> = files.iter().map(|(path, text)| Source { path: (*path).into(), text: (*text).into() }).collect();
        ContentDb::build(&kinds(), &sources).errors().map(ToString::to_string).collect()
    }

    const RULES_FILE: &str = "content/r.rules.ron";
    const SETS_FILE: &str = "content/s.rule_set.ron";

    #[test]
    fn the_test_rules_are_clean_and_read_back() {
        assert_eq!(errors(&[(RULES_FILE, RULES), (SETS_FILE, SETS)]), Vec::<String>::new());
        let db = ContentDb::build(&kinds(), &[Source { path: RULES_FILE.into(), text: RULES.into() }, Source { path: SETS_FILE.into(), text: SETS.into() }]);
        let harsh = db.peek::<RuleSetDef>("harsh").unwrap();
        assert_eq!(harsh.attack_tokens, TokenRule { shift: 1, min: 1 });
        assert_eq!(harsh.attack_rest, 0.5);
        let plain = db.peek::<RuleSetDef>("plain").unwrap();
        assert_eq!((plain.attack_rest, plain.attack_tokens), (1.0, TokenRule::default()), "left out: the engine's numbers");
        assert_eq!(plain.classes["player"].incoming_damage, 1.0);
        assert_eq!(db.peek::<RulesDef>("default").unwrap().class_names(), ["player", "brute"]);
    }

    #[test]
    fn an_overlong_rule_set_id_is_refused_at_content_load() {
        let long = "m".repeat(MAX_MODE + 1);
        let rules = format!(r#"{{"default": (modes: ["plain", "{long}"], start: "plain")}}"#);
        let sets = format!(r#"{{"plain": (name: "Plain"), "{long}": (name: "Long")}}"#);
        assert_eq!(
            errors(&[(RULES_FILE, &rules), (SETS_FILE, &sets)]),
            [format!(
                "content/s.rule_set.ron: rule_set `{long}`: rule set id `{long}` is {} bytes: an id is at most {MAX_MODE} bytes (a save names the rule set in play by it): shorten it",
                MAX_MODE + 1
            )]
        );
        let ok = "m".repeat(MAX_MODE);
        let rules = format!(r#"{{"default": (modes: ["{ok}"], start: "{ok}")}}"#);
        let sets = format!(r#"{{"{ok}": (name: "Edge")}}"#);
        assert_eq!(errors(&[(RULES_FILE, &rules), (SETS_FILE, &sets)]), Vec::<String>::new(), "exactly the limit is allowed");
    }

    #[test]
    fn a_rule_set_needs_the_rules_and_a_place_in_their_modes() {
        assert_eq!(
            errors(&[(SETS_FILE, r#"{"gentle": (name: "Gentle")}"#)]),
            ["content/s.rule_set.ron: rule_set `gentle`: rule sets need `content/**/*.rules.ron` with the definition `default`, which lists them in `modes`"]
        );
        let sets = SETS.replacen("\"plain\":", "\"loose\": (name: \"Loose\", classes: {\"player\": (charges: {\"flask\": 3})}),\n        \"plain\":", 1);
        assert_eq!(
            errors(&[(RULES_FILE, RULES), (SETS_FILE, &sets)]),
            ["content/s.rule_set.ron: rule_set `loose`: rule set `loose` is not in `rules/default` `modes`: add it there or delete it"]
        );
    }

    #[test]
    fn a_rule_set_has_a_name_and_knows_its_classes() {
        let sets = SETS.replace("name: \"Plain\"", "name: \"\"").replace("\"brute\": (max_health: 1.5)", "\"brut\": (max_health: 1.5)");
        assert_eq!(
            errors(&[(RULES_FILE, RULES), (SETS_FILE, &sets)]),
            [
                "content/s.rule_set.ron: rule_set `harsh`: class `brut` is not one of `rules/default` `classes` (did you mean `brute`?): player, brute",
                "content/s.rule_set.ron: rule_set `plain`: `name` is empty: a rule set has a name the player reads (a Fluent id, `@rules-gentle`)",
            ]
        );
    }

    #[test]
    fn factors_bonuses_and_cost_rules_are_checked() {
        let sets = SETS
            .replace("\"brute\": (max_health: 0.5)", "\"brute\": (max_health: 0, outgoing_damage: 11)")
            .replace("perfect_bonus: 0.05", "perfect_bonus: 1.5")
            .replace(
                "stamina_cost: [(tags: [\"roll\"], factor: 0.5)]",
                "stamina_cost: [(tags: [\"roll\"], factor: 0.5), (tags: [\"roll\", \"\"], factor: 20), (tags: [], factor: 2)]",
            )
            .replace("attack_rest: 0.5", "attack_rest: -1");
        assert_eq!(
            errors(&[(RULES_FILE, RULES), (SETS_FILE, &sets)]),
            [
                "content/s.rule_set.ron: rule_set `gentle`: `outgoing_damage` 11 of class `brute`: a factor is above 0 and at most 10 (1 changes nothing)",
                "content/s.rule_set.ron: rule_set `gentle`: `max_health` 0 of class `brute`: a factor is above 0 and at most 10 (1 changes nothing)",
                "content/s.rule_set.ron: rule_set `gentle`: `perfect_bonus` 1.5 of class `player`: a bonus is from 0 to 1 seconds",
                "content/s.rule_set.ron: rule_set `gentle`: `stamina_cost[1].factor` 20 of class `player`: a factor is above 0 and at most 10 (1 changes nothing)",
                "content/s.rule_set.ron: rule_set `gentle`: class `player`: `stamina_cost[1]` has an empty tag",
                "content/s.rule_set.ron: rule_set `gentle`: class `player`: tag `roll` is in two stamina rules: merge them",
                "content/s.rule_set.ron: rule_set `gentle`: class `player`: `stamina_cost[2]` names no tag: give it the tags or ids of the actions it scales (`roll`)",
                "content/s.rule_set.ron: rule_set `harsh`: `attack_rest` -1: a factor is above 0 and at most 10 (1 changes nothing)",
            ]
        );
    }

    #[test]
    fn charges_and_tokens_are_checked() {
        let long = "f".repeat(65);
        let sets = SETS
            .replace("charges: {\"flask\": 3}),\n                \"brute\"", &format!("charges: {{\"flask\": 0, \"{long}\": 1}}),\n                \"brute\""))
            .replace("attack_tokens: (shift: 1)", "attack_tokens: (shift: 9, min: 65)")
            .replace("(name: \"Plain\",", "(name: \"Plain\", attack_tokens: (min: 2),");
        let errors = errors(&[(RULES_FILE, RULES), (SETS_FILE, &sets)]);
        assert_eq!(
            errors[..5],
            [
                format!("content/s.rule_set.ron: rule_set `gentle`: class `player`: charge `{long}`: a charge's name is 1 to 64 characters with no control characters"),
                "content/s.rule_set.ron: rule_set `gentle`: class `player`: charge `flask` max 0: a charge's max is from 1 to 255".to_owned(),
                "content/s.rule_set.ron: rule_set `harsh`: `attack_tokens.shift` 9: a shift is from -8 to 8".to_owned(),
                "content/s.rule_set.ron: rule_set `harsh`: `attack_tokens.min` 65: a minimum is from 0 to 64".to_owned(),
                "content/s.rule_set.ron: rule_set `plain`: `attack_tokens.min` 2 without a `shift` changes nothing: give a shift or leave `min` out".to_owned(),
            ],
            "{errors:#?}"
        );
        // The long name is named by `gentle` alone: the rules see that too.
        assert_eq!(
            errors[5..],
            [
                format!("content/r.rules.ron: rules `default`: class `player`: charge `{long}` is set by `gentle` but not by `plain`: every rule set sets the same charges"),
                format!("content/r.rules.ron: rules `default`: class `player`: charge `{long}` is set by `gentle` but not by `harsh`: every rule set sets the same charges"),
            ],
            "{errors:#?}"
        );
    }

    #[test]
    fn every_rule_set_sets_the_same_charges() {
        let sets = SETS.replace("(name: \"Plain\", classes: {\"player\": (charges: {\"flask\": 3})})", "(name: \"Plain\")");
        assert_eq!(
            errors(&[(RULES_FILE, RULES), (SETS_FILE, &sets)]),
            ["content/r.rules.ron: rules `default`: class `player`: charge `flask` is set by `gentle` but not by `plain`: every rule set sets the same charges"]
        );
    }

    #[test]
    fn the_rules_are_one_definition_with_known_modes_and_classes() {
        let rules = r#"{
            "default": (
                modes: ["gentle", "plain", "harsh", "plain", "wild"],
                start: "plan",
                classes: ["brute", "brute", "Big One"],
                session: {"brut": (max_health: [1.0, 0.0, 1, 1, 1, 1, 1, 1, 1])},
            ),
            "second": (modes: [], start: "x"),
        }"#;
        assert_eq!(
            errors(&[(RULES_FILE, rules), (SETS_FILE, SETS)]),
            [
                "content/r.rules.ron: rules `default`: `modes` names `plain` twice",
                "content/r.rules.ron: rules `default`: modes -> unknown rule_set `wild`",
                "content/r.rules.ron: rules `default`: `start` `plan` is not in `modes` (did you mean `plain`?): gentle, plain, harsh, plain, wild",
                "content/r.rules.ron: rules `default`: `classes` names `brute` twice",
                "content/r.rules.ron: rules `default`: class `Big One`: a class is lowercase letters, digits and `_`",
                "content/r.rules.ron: rules `default`: `session` row `brut` is not a class (did you mean `brute`?): player, brute, brute, Big One",
                "content/r.rules.ron: rules `default`: `session.brut.max_health` has 9 entries: at most 8 (entry n − 1 is for n players)",
                "content/r.rules.ron: rules `default`: `session.brut.max_health[1]` 0: a factor is above 0 and at most 10 (1 changes nothing)",
                "content/r.rules.ron: rules `second`: the rules are one definition, `default`; `second` would be a second",
                "content/r.rules.ron: rules `second`: `modes` is empty: list the rule sets, in the order a menu shows them",
                "content/r.rules.ron: rules `second`: `start` `x` is not in `modes`: ",
            ]
        );
    }

    #[test]
    fn a_session_row_gives_a_column() {
        let rules = RULES.replace("session: {\"brute\": (max_health: [1.0, 1.5])}", "session: {\"brute\": (), \"player\": (max_poise: [])}");
        assert_eq!(
            errors(&[(RULES_FILE, &rules), (SETS_FILE, SETS)]),
            [
                "content/r.rules.ron: rules `default`: `session.brute` gives no column: give `max_health`, `max_poise` or `outgoing_damage`, or remove the row",
                "content/r.rules.ron: rules `default`: `session.player` gives no column: give `max_health`, `max_poise` or `outgoing_damage`, or remove the row",
            ]
        );
    }

    #[test]
    fn a_prefab_names_a_class_the_rules_know() {
        let prefabs = r#"{
            "rules_brute": (components: {"RuleClass": ("brute")}),
            "rules_typo": (components: {"RuleClass": ("brut")}),
            "rules_player": (components: {"RuleClass": ("player")}),
        }"#;
        assert_eq!(
            errors(&[(RULES_FILE, RULES), (SETS_FILE, SETS), ("content/p.prefab.ron", prefabs)]),
            ["content/p.prefab.ron: prefab `rules_typo`: RuleClass `brut` is not a class of `rules/default` (did you mean `brute`?): player, brute"]
        );
        // Without a type registry (a bare check) nobody else reads the value: the check says it does not read.
        let unread = errors(&[(RULES_FILE, RULES), (SETS_FILE, SETS), ("content/p.prefab.ron", r#"{"rules_five": (components: {"RuleClass": (5)})}"#)]);
        assert_eq!(unread.len(), 1, "{unread:?}");
        assert!(unread[0].starts_with("content/p.prefab.ron: prefab `rules_five`: RuleClass: "), "{unread:?}");
        assert!(unread[0].ends_with(": a class is a string in parentheses (`(\"brute\")`)"), "{unread:?}");
        assert_eq!(
            errors(&[("content/p.prefab.ron", r#"{"rules_brute": (components: {"RuleClass": ("brute")})}"#)]),
            ["content/p.prefab.ron: prefab `rules_brute`: RuleClass needs `content/**/*.rules.ron` (definition `default`) naming the class"]
        );
    }
}
