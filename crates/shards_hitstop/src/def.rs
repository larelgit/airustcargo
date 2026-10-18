//! The hit stop table: content kind `hitstop` (`*.hitstop.ron`).

use serde::{Deserialize, Serialize};
use shards_action::{HitWeight, StaggerCause};
use shards_content::{ContentDef, Refs};

/// The longest a single hold can be, whatever content says (s). A hit stop is a beat, not a pause.
pub const MAX_HOLD: f32 = 0.5;

/// The combat events a rule can name.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A hit landed (`Damaged`).
    Damaged,
    /// A hit met a guard (`Blocked`).
    Blocked,
    /// A hit was parried (`Parried`).
    Parried,
    /// A hit missed inside a perfect-evade window (`PerfectEvaded`).
    PerfectEvaded,
    /// A guard broke (`GuardBroken`).
    GuardBroken,
    /// Someone staggered (`Staggered`).
    Staggered,
}

impl EventKind {
    pub const ALL: [EventKind; 6] =
        [EventKind::Damaged, EventKind::Blocked, EventKind::Parried, EventKind::PerfectEvaded, EventKind::GuardBroken, EventKind::Staggered];

    pub fn name(self) -> &'static str {
        match self {
            EventKind::Damaged => "damaged",
            EventKind::Blocked => "blocked",
            EventKind::Parried => "parried",
            EventKind::PerfectEvaded => "perfect_evaded",
            EventKind::GuardBroken => "guard_broken",
            EventKind::Staggered => "staggered",
        }
    }
}

/// [`HitWeight`] as content writes it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Weight {
    Light,
    Heavy,
    Counter,
    Break,
    Finisher,
}

impl Weight {
    pub fn of(weight: HitWeight) -> Weight {
        match weight {
            HitWeight::Light => Weight::Light,
            HitWeight::Heavy => Weight::Heavy,
            HitWeight::Counter => Weight::Counter,
            HitWeight::Break => Weight::Break,
            HitWeight::Finisher => Weight::Finisher,
        }
    }
}

/// [`StaggerCause`] as content writes it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    PoiseBroken,
    Parried,
    GuardBroken,
    Dev,
}

impl Cause {
    pub fn of(cause: StaggerCause) -> Cause {
        match cause {
            StaggerCause::PoiseBroken => Cause::PoiseBroken,
            StaggerCause::Parried => Cause::Parried,
            StaggerCause::GuardBroken => Cause::GuardBroken,
            StaggerCause::Dev => Cause::Dev,
        }
    }
}

/// What a rule asks of a combat event. Every field that is written must hold; an empty condition
/// matches every event (so a rule like that ends the list).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct When {
    /// The event.
    pub event: Option<EventKind>,
    /// The hit's weight is at least this (`Light < Heavy < Counter < Break < Finisher`). Events
    /// without a weight (a parry, an evade, a stagger) never match a rule that names one.
    pub weight_at_least: Option<Weight>,
    /// The reaction tier the hit asks of its target is at least this (`Damaged` only).
    pub reaction_at_least: Option<u8>,
    /// Why someone staggered (`Staggered` only).
    pub cause: Option<Cause>,
    /// The id of the action that hit.
    pub action: Option<String>,
    /// Match a hit that was ignored (god mode, a dodge's i-frames, a grace period)? By default a
    /// rule matches only hits that landed.
    pub ignored: bool,
    /// Only a blow the local player is in (attacker, target or defender) matches. A fight between
    /// two others, a hazard's tick or a crowd far from the player then holds nothing.
    pub involves_local: bool,
}

/// One rule of the table: a condition and the hold it gives, in seconds.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    /// A name for the journal and the tools (`finisher`, `poise break`); else the event's.
    pub name: Option<String>,
    pub when: When,
    /// How long the hold lasts (s), before the player's scale and the table's `max_seconds`.
    pub seconds: f32,
}

/// The game's hit stop (kind `hitstop`; one per game, any id): rules over the combat events, the
/// first that matches gives the hold. A hold is a pause of the picture, and of the real-time clock
/// when the game is played alone, never slow motion: no tick is lost, and none is added.
///
/// ```ron
/// {
///     "feel": (
///         max_seconds: 0.12,
///         rules: [
///             (name: "finisher", when: (event: Damaged, weight_at_least: Finisher), seconds: 0.10),
///             (name: "poise break", when: (event: Damaged, weight_at_least: Break), seconds: 0.10),
///             (name: "heavy", when: (event: Damaged, reaction_at_least: 2), seconds: 0.08),
///             (name: "hit", when: (event: Damaged), seconds: 0.06),
///             (name: "parry", when: (event: Parried), seconds: 0.10),
///         ],
///     ),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct HitstopDef {
    /// The rules, in order; the first that matches an event gives its hold, and an event no rule
    /// matches gives none.
    pub rules: Vec<Rule>,
    /// No hold is longer than this (s): the game's own ceiling (at most [`MAX_HOLD`]).
    pub max_seconds: f32,
    /// After a hold is over, no new one starts for this long (s): a flurry of hits does not turn
    /// into a stutter. Holds that overlap merge by the later end, cooldown or not.
    pub cooldown: f32,
}

impl Default for HitstopDef {
    fn default() -> Self {
        Self { rules: Vec::new(), max_seconds: 0.25, cooldown: 0.0 }
    }
}

/// What a combat event says, as far as the rules read it.
#[derive(Clone, Debug, PartialEq)]
pub struct HitInfo {
    pub event: EventKind,
    pub weight: Option<Weight>,
    pub reaction: Option<u8>,
    pub cause: Option<Cause>,
    pub action: Option<String>,
    pub ignored: bool,
    /// The local player is one of the bodies of the blow (set when the table is consulted).
    pub local: bool,
}

impl When {
    /// Whether `info` satisfies every field written here.
    pub fn matches(&self, info: &HitInfo) -> bool {
        self.event.is_none_or(|e| e == info.event)
            && self.weight_at_least.is_none_or(|w| info.weight.is_some_and(|have| have >= w))
            && self.reaction_at_least.is_none_or(|r| info.reaction.is_some_and(|have| have >= r))
            && self.cause.is_none_or(|c| info.cause == Some(c))
            && self.action.as_deref().is_none_or(|a| info.action.as_deref() == Some(a))
            && self.ignored == info.ignored
            && (!self.involves_local || info.local)
    }

    /// Whether every event `other` matches is matched here too (so a rule `other` placed after
    /// this one can never be reached).
    fn covers(&self, other: &When) -> bool {
        self.ignored == other.ignored
            && (!self.involves_local || other.involves_local)
            && self.event.is_none_or(|e| other.event == Some(e))
            && self.weight_at_least.is_none_or(|w| other.weight_at_least.is_some_and(|o| o >= w))
            && self.reaction_at_least.is_none_or(|r| other.reaction_at_least.is_some_and(|o| o >= r))
            && self.cause.is_none_or(|c| other.cause == Some(c))
            && self.action.as_deref().is_none_or(|a| other.action.as_deref() == Some(a))
    }

    /// The condition in a few words: `damaged · weight ≥ Break · reaction ≥ 2`.
    pub fn describe(&self) -> String {
        let mut parts = vec![self.event.map_or("any event", EventKind::name).to_owned()];
        if let Some(w) = self.weight_at_least {
            parts.push(format!("weight ≥ {w:?}"));
        }
        if let Some(r) = self.reaction_at_least {
            parts.push(format!("reaction ≥ {r}"));
        }
        if let Some(c) = self.cause {
            parts.push(format!("cause {c:?}"));
        }
        if let Some(a) = &self.action {
            parts.push(format!("action {a}"));
        }
        if self.ignored {
            parts.push("ignored hits".to_owned());
        }
        if self.involves_local {
            parts.push("local player involved".to_owned());
        }
        parts.join(" · ")
    }
}

impl Rule {
    /// The rule's name in the journal: its own, else `rule 3 damaged`.
    pub fn label(&self, index: usize) -> String {
        match &self.name {
            Some(name) => name.clone(),
            None => format!("rule {} {}", index + 1, self.when.event.map_or("any", EventKind::name)),
        }
    }
}

impl HitstopDef {
    /// The first rule that matches `info`: its index and the rule.
    pub fn select(&self, info: &HitInfo) -> Option<(usize, &Rule)> {
        self.rules.iter().enumerate().find(|(_, rule)| rule.when.matches(info))
    }

    /// The longest a hold can be here (s).
    pub fn ceiling(&self) -> f32 {
        self.max_seconds.clamp(0.0, MAX_HOLD)
    }

    /// The hold a rule gives (s), within the table's ceiling and the engine's.
    pub fn hold(&self, rule: &Rule) -> f32 {
        if rule.seconds.is_finite() { rule.seconds.clamp(0.0, self.ceiling()) } else { 0.0 }
    }
}

impl ContentDef for HitstopDef {
    const KIND: &'static str = "hitstop";
    // Only what a machine shows and the real-time clock does: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, id: &str, refs: &mut Refs) {
        let others: Vec<&str> = refs.iter::<HitstopDef>().map(|(other, _)| other).filter(|other| *other != id).collect();
        if !others.is_empty() {
            refs.error(format!("a game has one hit stop table, and this one has more: {}; keep one", others.join(", ")));
        }
        if !self.max_seconds.is_finite() || self.max_seconds <= 0.0 || self.max_seconds > MAX_HOLD {
            refs.error(format!("max_seconds {}: a number above 0 and at most {MAX_HOLD} (a hit stop is a beat, not a pause)", self.max_seconds));
        }
        if !self.cooldown.is_finite() || self.cooldown < 0.0 || self.cooldown > 5.0 {
            refs.error(format!("cooldown {}: a number from 0 to 5 seconds", self.cooldown));
        }
        for (i, rule) in self.rules.iter().enumerate() {
            let at = format!("rules[{i}] ({})", rule.label(i));
            if !rule.seconds.is_finite() || rule.seconds < 0.0 {
                refs.error(format!("{at}: seconds {} is not a duration; write 0.08 for 80 ms", rule.seconds));
            } else if rule.seconds > self.max_seconds.min(MAX_HOLD) {
                refs.error(format!(
                    "{at}: seconds {} is longer than max_seconds {} and would be cut; raise max_seconds (at most {MAX_HOLD}) or shorten the rule",
                    rule.seconds,
                    self.max_seconds.min(MAX_HOLD)
                ));
            }
            let w = &rule.when;
            if w.reaction_at_least.is_some() && w.event.is_some_and(|e| e != EventKind::Damaged) {
                refs.error(format!("{at}: reaction_at_least is read from a hit that landed (event: Damaged), not from {}", w.event.map_or("", EventKind::name)));
            }
            if w.cause.is_some() && w.event.is_some_and(|e| e != EventKind::Staggered) {
                refs.error(format!("{at}: cause is read from a stagger (event: Staggered), not from {}", w.event.map_or("", EventKind::name)));
            }
            if w.weight_at_least.is_some() && w.event.is_some_and(|e| matches!(e, EventKind::Parried | EventKind::PerfectEvaded | EventKind::Staggered)) {
                refs.error(format!("{at}: {} carries no weight, so a weight_at_least would never match", w.event.map_or("", EventKind::name)));
            }
            if let Some(action) = &w.action
                && action.trim().is_empty()
            {
                refs.error(format!("{at}: an empty action; name the action's id or leave it out"));
            }
            if let Some(first) = self.rules[..i].iter().position(|before| before.when.covers(&rule.when)) {
                refs.error(format!(
                    "{at}: never matches, rule {} ({}) before it matches everything it does; put the narrower rule first",
                    first + 1,
                    self.rules[first].label(first)
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> HitstopDef {
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).unwrap()
    }

    fn info(event: EventKind, weight: Option<Weight>, reaction: Option<u8>) -> HitInfo {
        HitInfo { event, weight, reaction, cause: None, action: None, ignored: false, local: false }
    }

    const LADDER: &str = r#"(
        max_seconds: 0.12,
        rules: [
            (name: "finisher", when: (event: Damaged, weight_at_least: Finisher), seconds: 0.10),
            (name: "break", when: (event: Damaged, weight_at_least: Break), seconds: 0.10),
            (name: "counter", when: (event: Damaged, weight_at_least: Counter), seconds: 0.08),
            (name: "heavy", when: (event: Damaged, reaction_at_least: 2), seconds: 0.08),
            (when: (event: Damaged), seconds: 0.06),
            (when: (event: Blocked), seconds: 0.06),
            (name: "parry", when: (event: Parried), seconds: 0.10),
            (when: (event: Staggered, cause: PoiseBroken), seconds: 0.10),
        ],
    )"#;

    #[test]
    fn the_first_rule_that_matches_gives_the_hold() {
        let def = read(LADDER);
        let name = |i: &HitInfo| def.select(i).map(|(n, r)| (r.label(n), def.hold(r)));
        use EventKind::*;
        use Weight::*;
        assert_eq!(name(&info(Damaged, Some(Finisher), Some(3))), Some(("finisher".into(), 0.10)));
        assert_eq!(name(&info(Damaged, Some(Break), Some(3))), Some(("break".into(), 0.10)), "break beats a heavy reaction");
        assert_eq!(name(&info(Damaged, Some(Counter), None)), Some(("counter".into(), 0.08)));
        assert_eq!(name(&info(Damaged, Some(Heavy), Some(2))), Some(("heavy".into(), 0.08)));
        assert_eq!(name(&info(Damaged, Some(Light), Some(1))), Some(("rule 5 damaged".into(), 0.06)));
        assert_eq!(name(&info(Blocked, Some(Heavy), None)), Some(("rule 6 blocked".into(), 0.06)));
        assert_eq!(name(&info(Parried, None, None)), Some(("parry".into(), 0.10)));
        assert_eq!(name(&info(PerfectEvaded, None, None)), None, "no rule, no hold");
        let mut stagger = info(Staggered, None, None);
        stagger.cause = Some(Cause::Dev);
        assert_eq!(name(&stagger), None);
        stagger.cause = Some(Cause::PoiseBroken);
        assert_eq!(name(&stagger).map(|n| n.1), Some(0.10));
        // A hit that was ignored (a dodge's i-frames) matches no rule that does not ask for it.
        let mut ignored = info(Damaged, Some(Light), None);
        ignored.ignored = true;
        assert_eq!(name(&ignored), None);
    }

    #[test]
    fn a_hold_is_cut_to_the_ceiling_and_a_bad_number_is_no_hold() {
        let def = HitstopDef { max_seconds: 0.1, rules: Vec::new(), cooldown: 0.0 };
        assert_eq!(def.hold(&Rule { seconds: 0.4, ..Rule::default() }), 0.1);
        assert_eq!(def.hold(&Rule { seconds: f32::NAN, ..Rule::default() }), 0.0);
        assert_eq!(def.hold(&Rule { seconds: -1.0, ..Rule::default() }), 0.0);
        let wild = HitstopDef { max_seconds: 40.0, ..HitstopDef::default() };
        assert_eq!(wild.ceiling(), MAX_HOLD, "the engine's ceiling stands whatever content says");
    }

    #[test]
    fn a_rule_the_ones_before_it_cover_is_found() {
        let def = read(r#"(rules: [(when: (event: Damaged), seconds: 0.06), (when: (event: Damaged, weight_at_least: Break), seconds: 0.1)])"#);
        let ok = read(r#"(rules: [(when: (event: Damaged, weight_at_least: Break), seconds: 0.1), (when: (event: Damaged), seconds: 0.06)])"#);
        assert!(def.rules[0].when.covers(&def.rules[1].when));
        assert!(!ok.rules[0].when.covers(&ok.rules[1].when));
        assert!(!When { ignored: true, ..When::default() }.covers(&When::default()), "ignored hits are another set");
    }

    #[test]
    fn typos_are_refused() {
        assert!(ron::from_str::<HitstopDef>("(rule: [])").is_err());
        assert!(ron::from_str::<HitstopDef>("(rules: [(when: (event: Hit), seconds: 0.1)])").is_err());
        assert!(ron::from_str::<HitstopDef>("(rules: [(when: (wieght_at_least: Break), seconds: 0.1)])").is_err());
    }

    #[test]
    fn a_local_rule_matches_only_blows_the_local_player_is_in() {
        let def = read(r#"(rules: [(when: (event: Damaged, involves_local: true), seconds: 0.06)])"#);
        let mut hit = info(EventKind::Damaged, Some(Weight::Light), None);
        assert!(def.select(&hit).is_none(), "two others fighting");
        hit.local = true;
        assert!(def.select(&hit).is_some());
        let all = When::default();
        let local = When { involves_local: true, ..When::default() };
        assert!(all.covers(&local) && !local.covers(&all), "a rule for every blow shadows a local one, not the other way round");
    }
}
