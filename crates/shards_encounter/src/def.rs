//! The two content kinds: `*.encounter.ron` ([`EncounterDef`]) and `*.encounter_rules.ron`
//! ([`EncounterRulesDef`]), and the checks that read them at content load.
//!
//! An encounter is a mechanism (anchors, waves, counts, spawn points, budgets, limits, the start,
//! wipe and reset rules); every "what" (which prefabs, how many, how many seconds, how far) is
//! content, so no number here is a default of any particular game: seconds default to 0 and
//! optional rules to none.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shards_content::db::closest;
use shards_content::{ContentDef, PrefabDef, Refs};

use crate::anchor::{prefab_component, prefab_role, PrefabRole};

/// What an encounter is: where it stands and when it starts, its waves, and what happens when
/// the players are wiped or leave. Kind `encounter`. Read by id, and only by runs, so a replay
/// without an anchor reads no encounter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncounterDef {
    /// Meters (flat) around the anchor: the volumes, markers and points of interest this
    /// encounter binds, and the zone a participant stands in to count as near (`reset_after`).
    pub radius: f32,
    pub start: StartDef,
    /// In order; at least one.
    pub waves: Vec<WaveDef>,
    /// The encounter's own tags (`special_arena`), which rule limits name in `unless`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The most one wave may cost (the sum of `EncounterRole::cost` at the highest counts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u32>,
    /// Non-empty: every member has at least one of these role tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<String>,
    /// No member has any of these role tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden: Vec<String>,
    /// `encounter_rules` id whose limits and needs every wave meets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<String>,
    /// Seconds from a participant standing again after a wipe to the pending wave.
    #[serde(default)]
    pub resume_after: f32,
    /// Seconds with no standing participant within `radius` before the run resets; none: it
    /// never resets for absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_after: Option<f32>,
    /// What an absence reset keeps.
    #[serde(default)]
    pub on_reset: OnReset,
    /// What survivors of a placed wave do when the run pulls back (a wipe or a reset).
    #[serde(default)]
    pub pull_back: PullBack,
    /// Prefab spawned at the anchor while the fight is on; none: no lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock: Option<String>,
    /// Won once, won for good: the key is remembered in snapshots and world saves.
    #[serde(default)]
    pub once: bool,
    /// Members are in every client's view while the fight is on.
    #[serde(default)]
    pub net_always: bool,
    /// Joiners wait while the fight is on.
    #[serde(default)]
    pub blocks_join: bool,
}

/// What starts a run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StartDef {
    /// Standing participants inside the trigger volume `volume` (the anchor's own, or the nearest
    /// within `radius`): any of them, or with `all_players_in` all of them and at least one.
    Trigger {
        volume: String,
        #[serde(default)]
        all_players_in: bool,
    },
    /// Standing participants within `within` meters (flat) of the anchor.
    Near {
        within: f32,
        #[serde(default)]
        all_players_in: bool,
    },
    /// Only `start_encounter` (game code) or the dev tool `encounter_start`.
    Manual,
}

/// One wave.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaveDef {
    /// Seconds from the wave becoming due (the start, the previous wave cleared) to its spawn.
    #[serde(default)]
    pub delay: f32,
    /// Spawned when the run is made, before the start (camps, room guards); only the first wave.
    #[serde(default)]
    pub placed: bool,
    /// Played when the wave becomes due, `delay` seconds before it spawns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn: Option<CueDef>,
    /// Spawned in this order.
    pub members: Vec<MemberGroup>,
}

/// A group of members of one prefab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberGroup {
    pub prefab: String,
    /// How many; with `up_to`, a number in `count..=up_to` from the simulation's randomness.
    #[serde(default = "one")]
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub up_to: Option<u32>,
    pub at: SpawnAt,
    #[serde(default)]
    pub face: Face,
    /// Where the member's `Home` is: where it appeared (default) or the anchor.
    #[serde(default)]
    pub home: HomeAt,
    /// The member's `Home::leash`; none: the prefab's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leash: Option<f32>,
    /// Points nearer a standing participant than `within` meters and inside the rear cone of
    /// `angle` degrees of its facing are not chosen. Not for `Offsets`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoid_behind: Option<AvoidBehind>,
    /// Component fields over the prefab's: component name (as a prefab writes it) to a merge
    /// patch, field by field (`"Health": (current: 10.0)` keeps the prefab's `max`), exactly as
    /// a level node's overrides and `spawn_prefab`'s. `Home` is not allowed here (`home`,
    /// `leash`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, Value>,
}

fn one() -> u32 {
    1
}

fn yes() -> bool {
    true
}

/// Where the members of a group stand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SpawnAt {
    /// Points relative to the anchor, turned with its yaw: member `i` at point `i`.
    Offsets(Vec<Vec3>),
    /// `Marker` and `LevelMarker` entities named this within `radius`.
    Marker(String),
    /// `Poi` entities of this `poi` type within `radius`.
    Poi(String),
    /// Points on a ring around the anchor, `min..max` meters out.
    Ring { min: f32, max: f32 },
}

/// Where a spawned member looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Face {
    /// Toward the anchor.
    #[default]
    Center,
    /// The anchor's facing.
    Anchor,
    /// Degrees clockwise from the anchor's facing.
    Yaw(f32),
}

/// Where a spawned member's `Home` is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HomeAt {
    /// Where it appeared.
    #[default]
    Spawn,
    /// The anchor.
    Anchor,
}

/// The rear cone a spawn point is not chosen in.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AvoidBehind {
    /// Meters from a standing participant.
    pub within: f32,
    /// Degrees across the cone behind the participant's facing.
    pub angle: f32,
}

/// What an absence reset keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OnReset {
    /// Back to the start: spawned waves go, the encounter begins again.
    #[default]
    Restart,
    /// The waves cleared stay cleared; the wave in progress is rewound as a wipe does.
    KeepCleared,
}

/// What survivors of a placed wave do when the run pulls back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PullBack {
    /// Survivors to full health and poise.
    #[serde(default = "yes")]
    pub heal: bool,
    /// Behaviour state forced on survivors (`force_state`); none: placed at home at once.
    #[serde(default)]
    pub state: Option<String>,
    /// Seconds after which a survivor not yet home is placed there.
    #[serde(default)]
    pub deadline: f32,
}

impl Default for PullBack {
    fn default() -> Self {
        Self { heal: true, state: None, deadline: 0.0 }
    }
}

/// A warning or a message played with a wave: player-read text (a Fluent `@id`, with `$wave` and
/// `$waves` as its arguments), an effect and a sound. Also an event field, so it reflects.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect)]
#[serde(deny_unknown_fields)]
pub struct CueDef {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub effect: Option<String>,
    #[serde(default)]
    pub sound: Option<String>,
}

/// `*.encounter_rules.ron`: which wave compositions are allowed. Only validation reads it, so
/// it is not simulation content and changing it changes no replay.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EncounterRulesDef {
    #[serde(default)]
    pub limits: Vec<TagLimit>,
    #[serde(default)]
    pub needs: Vec<TagNeeds>,
}

/// At most `max` members tagged `tag` in one wave, unless the encounter has the tag `unless`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagLimit {
    pub tag: String,
    pub max: u32,
    #[serde(default)]
    pub unless: Option<String>,
}

/// A wave that can hold a member tagged `tag` holds at least `others` members without it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagNeeds {
    pub tag: String,
    pub others: u32,
}

impl ContentDef for EncounterRulesDef {
    const KIND: &'static str = "encounter_rules";
    const SIMULATION: bool = false;
}

impl EncounterDef {
    /// Whether the first wave stands before the start.
    pub fn placed(&self) -> bool {
        self.waves.first().is_some_and(|w| w.placed)
    }
}

/// A number of seconds that is finite and at least `min`.
fn seconds_ok(value: f32, min: f32) -> bool {
    value.is_finite() && value >= min
}

impl ContentDef for EncounterDef {
    const KIND: &'static str = "encounter";

    fn validate(&self, _id: &str, refs: &mut Refs) {
        self.check_numbers(refs);
        if self.waves.is_empty() {
            refs.error("waves: an encounter needs at least one wave");
        }
        for (w, wave) in self.waves.iter().enumerate() {
            if wave.members.is_empty() {
                refs.error(format!("waves[{w}].members: a wave needs at least one group"));
            }
            if wave.placed && w > 0 {
                refs.error(format!("waves[{w}].placed: only the first wave can be placed (it stands before the start)"));
            }
            if !seconds_ok(wave.delay, 0.0) {
                refs.error(format!("waves[{w}].delay: {} is not a number of seconds >= 0", wave.delay));
            }
            for (g, group) in wave.members.iter().enumerate() {
                self.check_group(refs, w, g, group);
            }
        }
        let rules = self.rules.as_deref().and_then(|rules| {
            if refs.require::<EncounterRulesDef>(rules, "rules") { refs.get::<EncounterRulesDef>(rules) } else { None }
        });
        self.check_roles(refs, rules);
        self.check_pull_back(refs);
        if let Some(lock) = &self.lock {
            refs.require::<PrefabDef>(lock, "lock");
        }
    }
}

impl EncounterDef {
    fn check_numbers(&self, refs: &mut Refs) {
        if !(self.radius.is_finite() && self.radius > 0.0) {
            refs.error(format!("radius: {} is not a distance > 0 meters", self.radius));
        }
        if !seconds_ok(self.resume_after, 0.0) {
            refs.error(format!("resume_after: {} is not a number of seconds >= 0", self.resume_after));
        }
        if let Some(after) = self.reset_after
            && !seconds_ok(after, 0.0)
        {
            refs.error(format!("reset_after: {after} is not a number of seconds >= 0"));
        }
        if !seconds_ok(self.pull_back.deadline, 0.0) {
            refs.error(format!("pull_back.deadline: {} is not a number of seconds >= 0", self.pull_back.deadline));
        }
        if let StartDef::Near { within, .. } = &self.start
            && !(within.is_finite() && *within > 0.0)
        {
            refs.error(format!("start.within: {within} is not a distance > 0 meters"));
        }
    }

    fn check_group(&self, refs: &mut Refs, w: usize, g: usize, group: &MemberGroup) {
        let at = format!("waves[{w}].members[{g}]");
        refs.require::<PrefabDef>(&group.prefab, &format!("{at}.prefab"));
        if group.count == 0 {
            refs.error(format!("{at}.count: a group needs at least 1 member"));
        }
        if let Some(up_to) = group.up_to
            && up_to < group.count
        {
            refs.error(format!("{at}.up_to: {up_to} is below count {}", group.count));
        }
        let most = group.up_to.unwrap_or(group.count).max(group.count);
        match &group.at {
            SpawnAt::Offsets(points) => {
                if (points.len() as u32) < most {
                    let s = if points.len() == 1 { "" } else { "s" };
                    refs.error(format!(
                        "{at}.at: {} offset{s} for up to {most} members: add offsets or lower the count",
                        points.len()
                    ));
                }
                if group.avoid_behind.is_some() {
                    refs.error(format!(
                        "{at}.avoid_behind: offsets are placed by hand and never filtered: remove it, or spawn at markers"
                    ));
                }
            }
            SpawnAt::Ring { min, max } => {
                if !(min.is_finite() && max.is_finite() && *min >= 0.0 && min <= max) {
                    refs.error(format!("{at}.at: a ring from {min} to {max} m is not 0 <= min <= max"));
                }
            }
            SpawnAt::Marker(_) | SpawnAt::Poi(_) => {}
        }
        if let Some(behind) = &group.avoid_behind
            && !(behind.within.is_finite() && behind.within > 0.0 && behind.angle.is_finite() && (0.0..=360.0).contains(&behind.angle))
        {
            refs.error(format!("{at}.avoid_behind: within must be > 0 m and angle 0..=360 degrees"));
        }
        if let Some(leash) = group.leash
            && !(leash.is_finite() && leash > 0.0)
        {
            refs.error(format!("{at}.leash: {leash} is not a distance > 0 meters"));
        }
        self.check_overrides(refs, &at, group);
    }

    /// The overrides are a merge patch per component, read the way a level node's are: the same
    /// reading `spawn_prefab` does, without the spawn.
    fn check_overrides(&self, refs: &mut Refs, at: &str, group: &MemberGroup) {
        if group.overrides.is_empty() {
            return;
        }
        for name in group.overrides.keys() {
            if name == "Home" || name.ends_with("::Home") {
                refs.error(format!("{at}.overrides.{name}: set the home with `home` and `leash`"));
            }
        }
        let (Some(registry), Some(_)) = (refs.type_registry(), refs.get::<PrefabDef>(&group.prefab)) else { return };
        let patch = Value::Object(group.overrides.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
        if let Err(why) = shards_content::prefab::check_prefab(refs.db(), registry, &group.prefab, Some(&patch)) {
            refs.error(format!("{at}.overrides: {why}"));
        }
    }

    /// Budget, allowed and forbidden tags, and the rules' limits and needs, per wave.
    fn check_roles(&self, refs: &mut Refs, rules: Option<&EncounterRulesDef>) {
        let reads_roles = self.budget.is_some() || !self.allowed.is_empty() || !self.forbidden.is_empty() || rules.is_some();
        if !reads_roles {
            return;
        }
        for (w, wave) in self.waves.iter().enumerate() {
            // (group index, role, lowest count, highest count) of every group that has a prefab.
            let mut groups: Vec<(usize, PrefabRole, u32, u32)> = Vec::new();
            for (g, group) in wave.members.iter().enumerate() {
                if refs.get::<PrefabDef>(&group.prefab).is_none() {
                    continue;
                }
                let Some(role) = prefab_role(refs.db(), &group.prefab) else {
                    refs.error(format!(
                        "waves[{w}].members[{g}]: prefab `{}` has no EncounterRole, which budget, allowed, forbidden and rules read: add \"EncounterRole\": (cost: 1, tags: [\"weak\"]) to it",
                        group.prefab
                    ));
                    continue;
                };
                let most = group.up_to.unwrap_or(group.count).max(group.count);
                groups.push((g, role, group.count, most));
            }
            if let Some(budget) = self.budget {
                let cost: u32 = groups.iter().map(|(_, role, _, most)| role.cost * most).sum();
                if cost > budget {
                    let parts: Vec<String> =
                        groups.iter().map(|(g, role, _, most)| format!("{} x{most} at {}", wave.members[*g].prefab, role.cost)).collect();
                    refs.error(format!(
                        "waves[{w}]: costs up to {cost} ({}), over the budget of {budget}: lower a count, or raise budget",
                        parts.join(", ")
                    ));
                }
            }
            for (g, role, ..) in &groups {
                let prefab = &wave.members[*g].prefab;
                if !self.allowed.is_empty() && !role.tags.iter().any(|t| self.allowed.contains(t)) {
                    refs.error(format!(
                        "waves[{w}].members[{g}]: prefab `{prefab}` has tags [{}], none of them in allowed [{}]",
                        role.tags.join(", "),
                        self.allowed.join(", ")
                    ));
                }
                if let Some(tag) = role.tags.iter().find(|t| self.forbidden.contains(t)) {
                    refs.error(format!("waves[{w}].members[{g}]: prefab `{prefab}` has the forbidden tag `{tag}`"));
                }
            }
            let Some(rules) = rules else { continue };
            let rules_id = self.rules.as_deref().unwrap_or_default();
            for limit in &rules.limits {
                if limit.unless.as_ref().is_some_and(|u| self.tags.contains(u)) {
                    continue;
                }
                let tagged: u32 = groups.iter().filter(|(_, r, ..)| r.tags.contains(&limit.tag)).map(|(.., most)| *most).sum();
                if tagged > limit.max {
                    let unless = match &limit.unless {
                        Some(u) => format!(" without the encounter tag `{u}`"),
                        None => String::new(),
                    };
                    let fix = match &limit.unless {
                        Some(u) => format!("lower a count, or tag the encounter `{u}` if it is one"),
                        None => "lower a count".to_owned(),
                    };
                    refs.error(format!(
                        "waves[{w}]: up to {tagged} members tagged `{}`, rule `{rules_id}` allows {}{unless}: {fix}",
                        limit.tag, limit.max
                    ));
                }
            }
            for need in &rules.needs {
                if !groups.iter().any(|(_, r, ..)| r.tags.contains(&need.tag)) {
                    continue;
                }
                let others: u32 = groups.iter().filter(|(_, r, ..)| !r.tags.contains(&need.tag)).map(|(_, _, least, _)| *least).sum();
                if others < need.others {
                    refs.error(format!(
                        "waves[{w}]: a member tagged `{}` needs at least {} others, the wave can have {others}: raise a count",
                        need.tag, need.others
                    ));
                }
            }
        }
    }

    /// `pull_back.state`: it needs time to walk, and every member prefab's behaviour has it.
    fn check_pull_back(&self, refs: &mut Refs) {
        let Some(state) = &self.pull_back.state else { return };
        if self.pull_back.deadline <= 0.0 {
            refs.error(format!(
                "pull_back.deadline: 0 leaves no time to walk to the state `{state}`: give the deadline in seconds, or drop the state to place survivors home at once"
            ));
        }
        let mut seen: Vec<(String, String)> = Vec::new();
        for wave in &self.waves {
            for group in &wave.members {
                let Some(npc) = prefab_component(refs.db(), &group.prefab, "Npc") else { continue };
                let Ok(npc) = ron::from_str::<NpcBehavior>(&npc) else { continue };
                if seen.iter().any(|(p, _)| *p == group.prefab) {
                    continue;
                }
                seen.push((group.prefab.clone(), npc.behavior.clone()));
                if let Some(problem) = crate::anchor::behavior_lacks_state(refs.db(), &npc.behavior, state) {
                    refs.error(format!(
                        "pull_back.state: behaviour `{}` of prefab `{}` has no state `{state}`{}",
                        npc.behavior, group.prefab, problem
                    ));
                }
            }
        }
    }
}

/// The part of the `Npc` component a content check reads.
#[derive(Deserialize)]
struct NpcBehavior {
    behavior: String,
}

/// A suggestion for a misspelt name among `names`, as `" (did you mean `x`?)"`, or nothing.
pub(crate) fn did_you_mean<'a>(wanted: &str, names: impl Iterator<Item = &'a str>) -> String {
    closest(wanted, names).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default()
}
