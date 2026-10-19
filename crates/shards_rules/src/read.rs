//! Reading the rules: a body's class, the rule set in play, the factors of a class, the session
//! size. Every function returns the neutral value when there are no rules (no `rules/default`, no
//! [`ActiveRules`], or a neutral one), so a hook that reads them in a game without rules changes
//! nothing.
//!
//! The functions that take a [`ContentDb`] read through [`ContentDb::get`]: they are for the
//! simulation, and what they read counts for the replay check `content`. A tool, a UI source or a
//! describer reads with the `peek_*` functions, which never count.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::PlayerId;

use crate::def::{ClassRules, RuleSetDef, RulesDef, NEUTRAL_CLASS};
use crate::state::{ActiveRules, RuleClass, SessionFilter};
use crate::{PLAYER_CLASS, RULES_ID};

/// A body's class: its [`RuleClass`] if it has one, else `player` if it has `PlayerId`, else none.
pub fn class_of(world: &World, e: Entity) -> Option<&str> {
    match world.get::<RuleClass>(e) {
        Some(class) => Some(class.0.as_str()),
        None => world.get::<PlayerId>(e).map(|_| PLAYER_CLASS),
    }
}

/// The game's rules, `rules/default` (counts as read).
pub fn rules_def(db: &ContentDb) -> Option<&RulesDef> {
    db.get::<RulesDef>(RULES_ID)
}

/// [`rules_def`] for a reader that is not the simulation.
pub fn peek_rules_def(db: &ContentDb) -> Option<&RulesDef> {
    db.peek::<RulesDef>(RULES_ID)
}

/// The rule set in play: none while `rules` is absent or neutral (counts as read).
pub fn active_set<'a>(db: &'a ContentDb, rules: Option<&ActiveRules>) -> Option<&'a RuleSetDef> {
    rules.filter(|r| !r.is_neutral()).and_then(|r| db.get::<RuleSetDef>(&r.mode))
}

/// [`active_set`] for a reader that is not the simulation.
pub fn peek_active_set<'a>(db: &'a ContentDb, rules: Option<&ActiveRules>) -> Option<&'a RuleSetDef> {
    rules.filter(|r| !r.is_neutral()).and_then(|r| db.peek::<RuleSetDef>(&r.mode))
}

/// What `set` says of `class`: the engine's numbers for a class it does not name.
pub fn class_rules<'a>(set: &'a RuleSetDef, class: &str) -> &'a ClassRules {
    set.classes.get(class).unwrap_or(&NEUTRAL_CLASS)
}

/// The session size: the number of distinct `PlayerId` values among the bodies [`SessionFilter`]
/// seats. A corpse that keeps its `PlayerId` beside the player's new body counts once; dead and downed
/// players count. Without a filter every body is seated.
pub fn session_size(world: &World) -> u32 {
    let seated = world.get_resource::<SessionFilter>().copied().unwrap_or_default();
    let Some(mut bodies) = world.try_query::<(Entity, &PlayerId)>() else { return 0 };
    let players: BTreeSet<u8> = bodies.iter(world).filter(|(e, _)| (seated.0)(world, *e)).map(|(_, p)| p.0).collect();
    players.len() as u32
}

/// A column of the session table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    MaxHealth,
    MaxPoise,
    OutgoingDamage,
}

impl Column {
    pub const ALL: [Column; 3] = [Column::MaxHealth, Column::MaxPoise, Column::OutgoingDamage];

    /// As content writes it.
    pub fn name(self) -> &'static str {
        match self {
            Column::MaxHealth => "max_health",
            Column::MaxPoise => "max_poise",
            Column::OutgoingDamage => "outgoing_damage",
        }
    }

    /// As the journal says it (`brute health ×1.5`).
    pub fn label(self) -> &'static str {
        match self {
            Column::MaxHealth => "health",
            Column::MaxPoise => "poise",
            Column::OutgoingDamage => "damage",
        }
    }

    /// This column of `class`'s row in `def`: empty when the class has no row.
    pub fn of<'a>(self, def: &'a RulesDef, class: &str) -> &'a [f32] {
        let Some(row) = def.session.get(class) else { return &[] };
        match self {
            Column::MaxHealth => &row.max_health,
            Column::MaxPoise => &row.max_poise,
            Column::OutgoingDamage => &row.outgoing_damage,
        }
    }
}

/// Entry `min(players.max(1), len) − 1` of `row`: 1 for an empty row.
pub fn row_factor(row: &[f32], players: u32) -> f32 {
    if row.is_empty() {
        return 1.0;
    }
    let n = (players.max(1) as usize).min(row.len());
    row[n - 1]
}

/// The session table's factor of `class` in `column` at the session size `rules` holds: 1 when there
/// are no rules in play, the class has no row or the column is empty.
pub fn session_factor(db: &ContentDb, rules: Option<&ActiveRules>, class: &str, column: Column) -> f32 {
    let Some(rules) = rules.filter(|r| !r.is_neutral()) else { return 1.0 };
    rules_def(db).map_or(1.0, |def| row_factor(column.of(def, class), rules.players))
}

/// The rule set in play and the rules of `world`, with what `e`'s class is.
fn of_body(world: &World, e: Entity) -> Option<(&ContentDb, &ActiveRules, &RuleSetDef, &str)> {
    let class = class_of(world, e)?;
    let rules = world.get_resource::<ActiveRules>()?;
    let db = world.get_resource::<ContentDb>()?;
    let set = active_set(db, Some(rules))?;
    Some((db, rules, set, class))
}

/// The factor of the health damage `e` takes from hits: its class's `incoming_damage`.
pub fn incoming(world: &World, e: Entity) -> f32 {
    of_body(world, e).map_or(1.0, |(_, _, set, class)| class_rules(set, class).incoming_damage)
}

/// The factor of the health and poise damage `e`'s hits deal: its class's `outgoing_damage` × the session
/// row's. A game that captures a body's power for later damage (a burn) multiplies it by this where it captures it.
pub fn outgoing(world: &World, e: Entity) -> f32 {
    of_body(world, e).map_or(1.0, |(db, rules, set, class)| {
        class_rules(set, class).outgoing_damage * session_factor(db, Some(rules), class, Column::OutgoingDamage)
    })
}

/// The factor of `class`'s `Health::max`: the set in play's `max_health` × the session row's.
pub fn health_factor(db: &ContentDb, rules: Option<&ActiveRules>, class: &str) -> f32 {
    active_set(db, rules).map_or(1.0, |set| class_rules(set, class).max_health * session_factor(db, rules, class, Column::MaxHealth))
}

/// The factor of `class`'s `Poise::max`: the set in play's `max_poise` × the session row's.
pub fn poise_factor(db: &ContentDb, rules: Option<&ActiveRules>, class: &str) -> f32 {
    active_set(db, rules).map_or(1.0, |set| class_rules(set, class).max_poise * session_factor(db, rules, class, Column::MaxPoise))
}

/// The rule set in play as one body sees it: what the action machine's start of an action reads.
#[derive(Clone, Copy, Debug)]
pub struct RulesView<'a> {
    pub set: &'a RuleSetDef,
    pub class: &'a str,
}

impl RulesView<'_> {
    /// The factor of the stamina an action costs: the product of every cost rule of the class one of whose
    /// tags is one of the action's `tags` or its `id`; 1 when none matches.
    pub fn stamina_factor(&self, tags: &[String], id: &str) -> f32 {
        class_rules(self.set, self.class)
            .stamina_cost
            .iter()
            .filter(|rule| rule.tags.iter().any(|t| t == id || tags.contains(t)))
            .map(|rule| rule.factor)
            .product()
    }

    /// Seconds added to the end of the class's guards' perfect window.
    pub fn perfect_bonus(&self) -> f32 {
        class_rules(self.set, self.class).perfect_bonus
    }
}

/// The `max` of the named charges `set` gives `class`.
pub fn charge_rules<'a>(set: &'a RuleSetDef, class: &str) -> &'a BTreeMap<String, u32> {
    &class_rules(set, class).charges
}

/// A director's token limit `base` under `set`: the base when there is no set, its shift is 0 or the base is 0
/// (a director whose table gives no tokens stays at 0); else the base shifted, at least `min`, never below 0.
pub fn token_limit(base: u32, set: Option<&RuleSetDef>) -> u32 {
    match set.map(|s| s.attack_tokens) {
        Some(tokens) if tokens.shift != 0 && base > 0 => (base as i32 + tokens.shift).max(tokens.min as i32).max(0) as u32,
        _ => base,
    }
}

/// The factor of the rest an NPC takes after an attack it finished: `attack_rest`, or 1.
pub fn rest_factor(set: Option<&RuleSetDef>) -> f32 {
    set.map_or(1.0, |s| s.attack_rest)
}

#[cfg(test)]
mod tests {
    use shards_content::db::Source;
    use shards_content::ContentKinds;

    use super::*;
    use crate::def::{CostRule, TokenRule};

    fn db(rules: &str, sets: &str) -> ContentDb {
        let mut kinds = ContentKinds::default();
        kinds.register::<RuleSetDef>();
        kinds.register::<RulesDef>();
        let sources = [Source { path: "content/r.rules.ron".into(), text: rules.into() }, Source { path: "content/s.rule_set.ron".into(), text: sets.into() }];
        let db = ContentDb::build(&kinds, &sources);
        assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
        db
    }

    const RULES: &str = r#"{"default": (modes: ["gentle", "plain"], start: "plain", classes: ["brute"],
        session: {"brute": (max_health: [1.0, 1.5, 2.0], outgoing_damage: [1.0, 1.2])})}"#;
    const SETS: &str = r#"{
        "gentle": (name: "Gentle", classes: {
            "player": (incoming_damage: 0.5),
            "brute": (outgoing_damage: 1.5, max_health: 0.5, incoming_damage: 2),
        }),
        "plain": (name: "Plain"),
    }"#;

    fn world(mode: &str, players: u32) -> World {
        let mut world = World::new();
        world.insert_resource(db(RULES, SETS));
        world.insert_resource(ActiveRules { mode: mode.into(), generation: 1, players });
        world
    }

    #[test]
    fn a_class_comes_from_rule_class_then_player_id() {
        let mut world = World::new();
        let brute = world.spawn(RuleClass("brute".into())).id();
        let player = world.spawn(PlayerId(0)).id();
        let both = world.spawn((PlayerId(1), RuleClass("brute".into()))).id();
        let neither = world.spawn(Name::new("post")).id();
        assert_eq!(class_of(&world, brute), Some("brute"));
        assert_eq!(class_of(&world, player), Some("player"));
        assert_eq!(class_of(&world, both), Some("brute"), "the class wins over PlayerId");
        assert_eq!(class_of(&world, neither), None);
    }

    #[test]
    fn a_session_row_gives_its_entry_or_its_last() {
        let row = [1.0, 1.5, 2.0];
        let at = |n| row_factor(&row, n);
        assert_eq!([at(0), at(1), at(2), at(3), at(9)], [1.0, 1.0, 1.5, 2.0, 2.0], "0 counts as 1, more than 3 as 3");
        assert_eq!(row_factor(&[], 2), 1.0, "no row: 1");
        let world = world("gentle", 2);
        let (db, rules) = (world.resource::<ContentDb>(), world.get_resource::<ActiveRules>());
        assert_eq!(session_factor(db, rules, "brute", Column::MaxHealth), 1.5);
        assert_eq!(session_factor(db, rules, "brute", Column::MaxPoise), 1.0, "an empty column: 1");
        assert_eq!(session_factor(db, rules, "player", Column::MaxHealth), 1.0, "no row: 1");
        assert_eq!(health_factor(db, rules, "brute"), 0.75, "0.5 of the mode × 1.5 of the session");
        let neutral = ActiveRules { mode: String::new(), generation: 1, players: 2 };
        assert_eq!(session_factor(db, Some(&neutral), "brute", Column::MaxHealth), 1.0, "no rules in play: 1");
        assert_eq!(health_factor(db, None, "brute"), 1.0);
    }

    fn filter_out_player_one(world: &World, e: Entity) -> bool {
        world.get::<PlayerId>(e).is_some_and(|p| p.0 != 1)
    }

    #[test]
    fn the_session_counts_distinct_seated_players() {
        let mut world = World::new();
        assert_eq!(session_size(&world), 0);
        world.spawn(PlayerId(0));
        // A corpse keeps its PlayerId beside the new body: one player.
        world.spawn(PlayerId(0));
        world.spawn(PlayerId(1));
        assert_eq!(session_size(&world), 2, "the default filter seats every body");
        world.insert_resource(SessionFilter(filter_out_player_one));
        assert_eq!(session_size(&world), 1, "a body the filter leaves out is not counted");
        world.insert_resource(SessionFilter::default());
        assert_eq!(session_size(&world), 2);
    }

    #[test]
    fn a_token_shift_moves_the_table_and_keeps_its_floor() {
        let set = |shift, min| RuleSetDef { attack_tokens: TokenRule { shift, min }, ..default() };
        assert_eq!(token_limit(2, Some(&set(-1, 1))), 1);
        assert_eq!(token_limit(1, Some(&set(-1, 1))), 1, "at least min");
        assert_eq!(token_limit(2, Some(&set(1, 1))), 3);
        assert_eq!(token_limit(0, Some(&set(1, 1))), 0, "a limit of 0 stays 0");
        assert_eq!(token_limit(3, Some(&set(0, 5))), 3, "no shift: the base, whatever min says");
        assert_eq!(token_limit(1, Some(&set(-3, 0))), 0, "never below 0");
        assert_eq!(token_limit(2, None), 2, "no set: the base");
        assert_eq!((rest_factor(None), rest_factor(Some(&RuleSetDef { attack_rest: 0.5, ..default() }))), (1.0, 0.5));
    }

    #[test]
    fn stamina_rules_match_a_tag_or_the_id_and_multiply() {
        let mut set = RuleSetDef::default();
        let rules = ClassRules {
            stamina_cost: vec![
                CostRule { tags: vec!["roll".into()], factor: 0.5 },
                CostRule { tags: vec!["dodge".into(), "heavy".into()], factor: 0.8 },
            ],
            perfect_bonus: 0.05,
            ..default()
        };
        set.classes.insert("player".into(), rules);
        let view = RulesView { set: &set, class: "player" };
        let tags = |t: &[&str]| t.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(view.stamina_factor(&tags(&["roll"]), "back_step"), 0.5, "a tag");
        assert_eq!(view.stamina_factor(&tags(&[]), "roll"), 0.5, "the id");
        assert_eq!(view.stamina_factor(&tags(&["roll", "dodge"]), "x"), 0.5 * 0.8, "two rules multiply");
        assert_eq!(view.stamina_factor(&tags(&["light"]), "light_1"), 1.0, "no match: 1");
        assert_eq!(view.perfect_bonus(), 0.05);
        let brute = RulesView { set: &set, class: "brute" };
        assert_eq!((brute.stamina_factor(&tags(&["roll"]), "roll"), brute.perfect_bonus()), (1.0, 0.0), "a class the set does not name");
    }

    #[test]
    fn outgoing_is_the_class_times_the_session_and_incoming_the_class_alone() {
        let mut world = world("gentle", 2);
        let brute = world.spawn(RuleClass("brute".into())).id();
        let player = world.spawn(PlayerId(0)).id();
        let post = world.spawn(Name::new("post")).id();
        assert_eq!(outgoing(&world, brute), 1.5 * 1.2);
        assert_eq!(incoming(&world, brute), 2.0, "no session factor on incoming");
        assert_eq!((incoming(&world, player), outgoing(&world, player)), (0.5, 1.0));
        assert_eq!((incoming(&world, post), outgoing(&world, post)), (1.0, 1.0), "no class: untouched");
        world.resource_mut::<ActiveRules>().players = 1;
        assert_eq!(outgoing(&world, brute), 1.5);
        world.resource_mut::<ActiveRules>().mode.clear();
        assert_eq!((incoming(&world, brute), outgoing(&world, brute)), (1.0, 1.0), "neutral: 1");
        world.remove_resource::<ActiveRules>();
        assert_eq!((incoming(&world, player), outgoing(&world, brute)), (1.0, 1.0), "no rules: 1");
        let class = class_rules(world.resource::<ContentDb>().peek::<RuleSetDef>("plain").unwrap(), "brute");
        assert_eq!(class, &NEUTRAL_CLASS, "a class the set does not name keeps the engine's numbers");
        assert!(charge_rules(world.resource::<ContentDb>().peek::<RuleSetDef>("gentle").unwrap(), "player").is_empty());
    }
}
