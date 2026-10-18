//! State coverage: which states a session has reached.
//!
//! Test-only bookkeeping, off by default (`EngineConfig::coverage`, `--coverage`). When on, after
//! every tick each [`AiVisible`] entity gets a **signature**: the **parts** modules contribute
//! ([`add_coverage_part`](crate::AiAppExt::add_coverage_part)), joined in registration order, for
//! example `kind=npc motion=ground action=heavy:windup statuses=bleed npc=chase`. The resource
//! [`AiCoverage`] counts every distinct signature and remembers the tick it first appeared at.
//!
//! The `coverage` tool answers with the totals, the rarest signatures, how many are new since an
//! earlier answer (`since=<distinct>`: a fuzzer polls it to learn which of its calls led somewhere
//! new), and what content declares but the session never reached
//! ([`add_coverage_catalog`](crate::AiAppExt::add_coverage_catalog)): actions never started,
//! behaviour states never entered, statuses never applied. `coverage_reset` clears it.
//!
//! Coverage only reads the world. [`AiCoverage`] is not in the replay hash, the replay checks or a
//! snapshot, and it exists only when coverage is on: a game plays the same either way.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::describe::AiKind;
use crate::id::AiVisible;
use crate::session::Scope;
use crate::sim;
use crate::tools::{register_tool, Call, ToolKind, ToolResult};

/// One piece of an entity's signature: `Some(value)` when the entity has that aspect.
pub type CoveragePart = fn(&World, Entity) -> Option<String>;

/// What a catalog declares: ids from content.
pub type Declared = fn(&World) -> Vec<String>;

/// Which declared ids a part's value reached (`heavy:windup` reached the action `heavy`).
pub type Reached = fn(&str) -> Vec<String>;

/// A kind of thing content declares, and how a part's values tell it was reached.
#[derive(Clone)]
pub struct Catalog {
    /// `actions`, `behaviour states`, `statuses`.
    pub label: String,
    /// The part whose values show what was reached.
    pub part: String,
    pub declared: Declared,
    pub reached: Reached,
}

/// The parts and catalogs modules registered (always present; they cost nothing while
/// coverage is off).
#[derive(Resource, Default, Clone)]
pub struct CoverageParts {
    pub parts: Vec<(String, CoveragePart)>,
    pub catalogs: Vec<Catalog>,
}

impl CoverageParts {
    /// Adds a part; one with the same name is replaced in place (its position stays).
    pub fn add_part(&mut self, name: &str, part: CoveragePart) -> Result<(), String> {
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '=') {
            return Err(format!("coverage part name `{name}` must be a word without spaces or `=`"));
        }
        match self.parts.iter_mut().find(|(n, _)| n == name) {
            Some(slot) => slot.1 = part,
            None => self.parts.push((name.to_owned(), part)),
        }
        Ok(())
    }

    /// Adds a catalog; one with the same label is replaced.
    pub fn add_catalog(&mut self, catalog: Catalog) {
        match self.catalogs.iter_mut().find(|c| c.label == catalog.label) {
            Some(slot) => *slot = catalog,
            None => self.catalogs.push(catalog),
        }
    }
}

/// What is known about one signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureInfo {
    /// Entity-ticks in this state.
    pub count: u64,
    /// The tick it first appeared at.
    pub first_tick: u64,
}

/// The states reached so far. Exists only when coverage is on.
#[derive(Resource, Default, Debug, Clone)]
pub struct AiCoverage {
    signatures: BTreeMap<String, SignatureInfo>,
    /// Signatures in the order they first appeared (within one tick, sorted).
    order: Vec<String>,
    entity_ticks: u64,
    /// For each part, the values it took.
    values: BTreeMap<String, BTreeSet<String>>,
}

impl AiCoverage {
    /// Counts one entity-tick. `parts` are `(name, value)` in registration order. Whether the
    /// signature is new.
    pub fn record(&mut self, tick: u64, parts: &[(&str, &str)]) -> bool {
        let signature = parts.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(" ");
        self.entity_ticks += 1;
        for (name, value) in parts {
            if !self.values.get(*name).is_some_and(|set| set.contains(*value)) {
                self.values.entry((*name).to_owned()).or_default().insert((*value).to_owned());
            }
        }
        match self.signatures.get_mut(&signature) {
            Some(info) => {
                info.count += 1;
                false
            }
            None => {
                self.order.push(signature.clone());
                self.signatures.insert(signature, SignatureInfo { count: 1, first_tick: tick });
                true
            }
        }
    }

    /// Distinct signatures so far.
    pub fn distinct(&self) -> usize {
        self.signatures.len()
    }

    /// Entity-ticks counted so far.
    pub fn entity_ticks(&self) -> u64 {
        self.entity_ticks
    }

    pub fn info(&self, signature: &str) -> Option<&SignatureInfo> {
        self.signatures.get(signature)
    }

    /// The signatures that first appeared after the first `since` (a `distinct()` an earlier
    /// answer gave). After a reset (`since` is more than there are) all of them.
    pub fn new_since(&self, since: usize) -> &[String] {
        if since > self.order.len() { &self.order } else { &self.order[since..] }
    }

    /// The `n` rarest signatures: fewest entity-ticks first, then the earliest, then by text.
    pub fn rarest(&self, n: usize) -> Vec<(&str, &SignatureInfo)> {
        let mut all: Vec<(&str, &SignatureInfo)> = self.signatures.iter().map(|(s, i)| (s.as_str(), i)).collect();
        all.sort_by(|a, b| (a.1.count, a.1.first_tick, a.0).cmp(&(b.1.count, b.1.first_tick, b.0)));
        all.truncate(n);
        all
    }

    /// The values part `part` took.
    pub fn values_of(&self, part: &str) -> Option<&BTreeSet<String>> {
        self.values.get(part)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Whether this game keeps coverage (`--coverage`).
pub fn is_on(world: &World) -> bool {
    world.contains_resource::<AiCoverage>()
}

pub(crate) const OFF: &str = "coverage is off: start the game with coverage on (EngineConfig.coverage, or --coverage)";

pub(crate) fn plugin(app: &mut App, on: bool) {
    let world = app.world_mut();
    let mut parts = world.get_resource_or_init::<CoverageParts>();
    parts.add_part("kind", |world, entity| world.get::<AiKind>(entity).map(|k| k.0.to_string())).expect("kind is a word");
    register_tool(world, "coverage", Scope::Dev, ToolKind::Query, coverage_tool);
    register_tool(world, "coverage_reset", Scope::Dev, ToolKind::Control, coverage_reset_tool);
    if on {
        world.insert_resource(AiCoverage::default());
        // After the tick's other work: only reads.
        app.add_systems(FixedLast, record_tick.after(crate::failures::collect_after_tick));
    }
}

/// After every tick: one signature per visible entity.
fn record_tick(world: &mut World) {
    let tick = sim::tick(world);
    // Counting only watches: a game with coverage on reads what one without it does.
    let _quiet = crate::scope::suspend(world);
    let parts = world.resource::<CoverageParts>().parts.clone();
    let entities: Vec<Entity> = world.query_filtered::<Entity, With<AiVisible>>().iter(world).collect();
    // Each entity as its parts; sorted by signature, so the order the world yields entities in
    // (a window shifts it) never shows.
    let mut rows: Vec<(String, Vec<(usize, String)>)> = entities
        .into_iter()
        .map(|entity| {
            let values: Vec<(usize, String)> =
                parts.iter().enumerate().filter_map(|(i, (_, f))| f(world, entity).map(|v| (i, v))).collect();
            let signature = values.iter().map(|(i, v)| format!("{}={v}", parts[*i].0)).collect::<Vec<_>>().join(" ");
            (signature, values)
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    let mut coverage = world.resource_mut::<AiCoverage>();
    for (_, values) in &rows {
        let named: Vec<(&str, &str)> = values.iter().map(|(i, v)| (parts[*i].0.as_str(), v.as_str())).collect();
        coverage.record(tick, &named);
    }
}

/// What the session has reached: the distinct states (an entity's kind, motion mode, action and
/// phase, behaviour state and statuses, each module's part), the rarest ones, and what content
/// declares that was never reached. Needs a game started with coverage on.
#[derive(Deserialize, JsonSchema, Default)]
pub struct CoverageArgs {
    /// How many of the rarest signatures to list (default 10).
    pub top: Option<usize>,
    /// The `distinct` an earlier answer gave: the answer then says how many signatures are new
    /// since, and lists them. The way to ask "did what I just did lead anywhere new?".
    pub since: Option<usize>,
    /// Answer as JSON instead of text.
    #[serde(default)]
    pub json: bool,
}

fn coverage_tool(In(call): In<Call<CoverageArgs>>, world: &mut World) -> ToolResult {
    let Some(coverage) = world.get_resource::<AiCoverage>() else {
        return Err(OFF.to_owned());
    };
    let args = call.args;
    let top = args.top.unwrap_or(10);
    let distinct = coverage.distinct();
    let rarest: Vec<(String, SignatureInfo)> = coverage.rarest(top).into_iter().map(|(s, i)| (s.to_owned(), i.clone())).collect();
    let fresh: Option<Vec<String>> = args.since.map(|since| coverage.new_since(since).to_vec());
    let entity_ticks = coverage.entity_ticks();
    let catalogs = world.resource::<CoverageParts>().catalogs.clone();
    let never: Vec<(String, usize, Vec<String>)> = catalogs
        .iter()
        .map(|catalog| {
            let mut declared = (catalog.declared)(world);
            declared.sort();
            declared.dedup();
            let coverage = world.resource::<AiCoverage>();
            let reached: BTreeSet<String> = coverage
                .values_of(&catalog.part)
                .into_iter()
                .flatten()
                .flat_map(|value| (catalog.reached)(value))
                .collect();
            let total = declared.len();
            declared.retain(|id| !reached.contains(id));
            (catalog.label.clone(), total, declared)
        })
        .collect();

    if args.json {
        let value = serde_json::json!({
            "distinct": distinct,
            "entity_ticks": entity_ticks,
            "new_since": fresh.as_ref().map(Vec::len),
            "new": fresh,
            "rarest": rarest.iter().map(|(s, i)| serde_json::json!({"signature": s, "count": i.count, "first_tick": i.first_tick})).collect::<Vec<_>>(),
            "never_reached": never.iter().map(|(label, total, ids)| serde_json::json!({"what": label, "declared": total, "ids": ids})).collect::<Vec<_>>(),
        });
        return Ok(value.to_string());
    }

    let mut out = format!("coverage: {distinct} distinct signatures, {entity_ticks} entity-ticks");
    out.push_str(&format!("\ndistinct: {distinct}"));
    if let Some(fresh) = &fresh {
        out.push_str(&format!("\nnew since {}: {}", args.since.unwrap_or(0), fresh.len()));
        for signature in fresh.iter().take(20) {
            out.push_str(&format!("\n  + {signature}"));
        }
        if fresh.len() > 20 {
            out.push_str(&format!("\n  … and {} more", fresh.len() - 20));
        }
    }
    out.push_str(&format!("\nrarest {}:", rarest.len()));
    for (signature, info) in &rarest {
        out.push_str(&format!("\n  {}x first at tick {}  {signature}", info.count, info.first_tick));
    }
    for (label, total, ids) in &never {
        if ids.is_empty() {
            out.push_str(&format!("\nnever reached, {label}: none of {total}"));
        } else {
            out.push_str(&format!("\nnever reached, {label} ({} of {total}): {}", ids.len(), ids.join(", ")));
        }
    }
    Ok(out)
}

/// Forgets everything coverage counted (the game and its replay are untouched).
#[derive(Deserialize, JsonSchema, Default)]
pub struct CoverageResetArgs {}

fn coverage_reset_tool(In(_): In<Call<CoverageResetArgs>>, world: &mut World) -> ToolResult {
    let Some(mut coverage) = world.get_resource_mut::<AiCoverage>() else {
        return Err(OFF.to_owned());
    };
    let was = coverage.distinct();
    coverage.clear();
    Ok(format!("ok: coverage cleared ({was} signatures forgotten)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_is_counted_and_remembers_its_first_tick() {
        let mut c = AiCoverage::default();
        assert!(c.record(3, &[("kind", "npc"), ("action", "idle")]));
        assert!(!c.record(9, &[("kind", "npc"), ("action", "idle")]));
        assert!(c.record(9, &[("kind", "npc"), ("action", "heavy:windup")]));
        assert_eq!((c.distinct(), c.entity_ticks()), (2, 3));
        assert_eq!(c.info("kind=npc action=idle"), Some(&SignatureInfo { count: 2, first_tick: 3 }));
        assert_eq!(c.info("kind=npc action=heavy:windup").unwrap().first_tick, 9);
    }

    #[test]
    fn the_rarest_come_first_and_ties_break_by_age_then_text() {
        let mut c = AiCoverage::default();
        for _ in 0..5 {
            c.record(1, &[("a", "common")]);
        }
        c.record(7, &[("a", "late")]);
        c.record(2, &[("a", "early")]);
        c.record(2, &[("a", "also")]);
        let order: Vec<&str> = c.rarest(10).into_iter().map(|(s, _)| s).collect();
        assert_eq!(order, ["a=also", "a=early", "a=late", "a=common"]);
        assert_eq!(c.rarest(1).len(), 1);
    }

    #[test]
    fn new_since_lists_what_appeared_after_an_earlier_count() {
        let mut c = AiCoverage::default();
        c.record(1, &[("a", "x")]);
        let before = c.distinct();
        c.record(2, &[("a", "y")]);
        c.record(3, &[("a", "x")]);
        c.record(4, &[("a", "z")]);
        assert_eq!(c.new_since(before), ["a=y", "a=z"]);
        assert!(c.new_since(c.distinct()).is_empty());
        // After a reset a stale count means "everything".
        c.clear();
        c.record(5, &[("a", "q")]);
        assert_eq!(c.new_since(3), ["a=q"]);
    }

    #[test]
    fn the_values_each_part_took_are_kept() {
        let mut c = AiCoverage::default();
        c.record(1, &[("npc", "chase"), ("statuses", "bleed,slow")]);
        c.record(2, &[("npc", "idle")]);
        assert_eq!(c.values_of("npc").unwrap().iter().collect::<Vec<_>>(), ["chase", "idle"]);
        assert_eq!(c.values_of("statuses").unwrap().len(), 1);
        assert!(c.values_of("motion").is_none());
    }

    #[test]
    fn parts_keep_their_registration_order_and_a_replacement_keeps_its_place() {
        let mut p = CoverageParts::default();
        p.add_part("a", |_, _| Some("1".into())).unwrap();
        p.add_part("b", |_, _| None).unwrap();
        p.add_part("a", |_, _| Some("2".into())).unwrap();
        assert_eq!(p.parts.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(p.add_part("a b", |_, _| None).is_err());
        assert!(p.add_part("a=b", |_, _| None).is_err());
    }
}
