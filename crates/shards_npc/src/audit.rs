//! The audit of an attack plan (N8e): pure functions of content, for a game's tests and the
//! `plan_audit` query. Which attack may follow which by the plan's own sequence rules
//! ([`may_follow`]), how soon after one attack's danger the next can hit at the earliest
//! ([`follow_pairs`]), and every hit an NPC of the plan can deal ([`plan_hits`]).
//!
//! The verdicts are the game's. Whether a short gap is unavoidable depends on the arena; whether an
//! attack kills in one depends on the game's damage, its co-op factor and the hunter's health. So the
//! engine has no threshold of its own: `plan_audit min_gap=` lists the pairs below the caller's.
//!
//! The times are the worst case: the earliest danger, no reaction time, the gates of the moment
//! (angle, players, `when`, cooldowns) taken as open. An attack runs through its `then` actions, each
//! once (a loop is counted one pass); the shortest rest after a finished attack is `rest.0` times the
//! caller's `rest_scale` (a difficulty's pause factor, which the engine does not know). What its
//! `Spawn` effects lay down is part of its danger, each object from its own start (`delay` times its
//! index): a zone while its action's windows last, a projectile's flight until its `lifetime` and its
//! impact from at once (it may stop where it starts) to the end of the flight. Objects that `stay` (a
//! summon) fight on their own and are not counted. The danger is in time only: where a zone lies is
//! the arena's, so a gap that is short in time may still be safe in space.

use serde::Serialize;
use std::collections::BTreeMap;

use shards_action::{ActionDef, Effect, HitFlag, MovesetDef, SpawnDef, SpawnPattern, Transition};
use shards_content::db::closest;
use shards_content::{ContentDb, PrefabDef};

use crate::chain::recovery_start;
use crate::def::AttackPlanDef;
use crate::planner::{ban_of, resolve, PastAttack, Resolved};

/// Whether `b` may be chosen right after `a` by the plan's own sequence rules: the bans (`within` 1 or
/// more), `b`'s `forbid_after` against `a`'s tags, and `b`'s `max_repeats: Some(1)` when `a` is `b`. The
/// gates of the moment are taken as open.
pub fn may_follow(plan: &AttackPlanDef, a: &Resolved, b: &Resolved) -> bool {
    if a.name == b.name && b.max_repeats.is_some_and(|max| max <= 1) {
        return false;
    }
    if b.forbid_after.iter().any(|t| a.tags.contains(t)) {
        return false;
    }
    ban_of(&plan.bans, &b.tags, &[PastAttack { name: a.name.clone(), tags: a.tags.clone() }]).is_none()
}

/// Two options in the order the plan lets them follow, and how close their danger comes.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FollowPair {
    pub a: String,
    pub b: String,
    /// `b_first - a_end`, seconds: below 0, `b` can hit while `a`'s danger lasts.
    pub gap: f32,
    /// When `a`'s last hit window closes, seconds from `a`'s start.
    pub a_end: f32,
    /// When `b`'s first hit window opens at the earliest, seconds from `a`'s start.
    pub b_first: f32,
    /// The gap is through `a`'s chain: its continuation, then `b` after the whole chain.
    pub chained: bool,
}

/// One attack as content times it, from its start: how long it runs and when its danger starts and
/// ends, through its `then` actions and the objects they spawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Timing {
    length: f32,
    first: Option<f32>,
    last: Option<f32>,
}

impl Timing {
    /// Danger from `from` to `to` seconds.
    fn widen(&mut self, from: f32, to: f32) {
        self.first = Some(self.first.map_or(from, |f| f.min(from)));
        self.last = Some(self.last.map_or(to, |l| l.max(to)));
    }
}

/// How deep the audit follows spawns (an object's own spawns, a projectile's impact). The content
/// check refuses a cycle of `Spawn` and `impact` edges; this only bounds the walk.
const SPAWN_DEPTH: u32 = 4;

/// The timing of `action` and the `then` actions after it, each once, with the danger of the objects
/// their `Spawn` effects lay down; `None` when `action` is unknown.
fn timing(db: &ContentDb, action: &str) -> Option<Timing> {
    timing_at(db, action, 0)
}

fn timing_at(db: &ContentDb, action: &str, depth: u32) -> Option<Timing> {
    let mut t = Timing::default();
    let mut seen: Vec<String> = Vec::new();
    let mut next = Some(action.to_owned());
    while let Some(id) = next.take() {
        if seen.contains(&id) {
            break;
        }
        let Some(def) = db.get::<ActionDef>(&id) else {
            if seen.is_empty() {
                return None;
            }
            break;
        };
        let base = t.length;
        for hit in &def.hits {
            if let Some((from, to)) = def.span(&hit.span) {
                t.widen(base + from, base + to);
            }
        }
        if depth < SPAWN_DEPTH {
            for e in &def.effects {
                let Effect::Spawn(spawn) = &e.effect else { continue };
                if spawn.stay {
                    continue;
                }
                for j in 0..spawn.count as usize {
                    let start = base + e.at + spawn.delay * j as f32;
                    if let Some(Timing { first: Some(first), last: Some(last), .. }) = object_timing(db, spawn, j, depth + 1) {
                        t.widen(start + first, start + last);
                    }
                }
            }
        }
        t.length += def.length();
        seen.push(id);
        next = def.then.clone();
    }
    Some(t)
}

/// The danger of object `j` of `spawn` from the start of its action: its action's (and what that
/// spawns). A projectile's flight is cut at its `lifetime` (or ends with its action, if sooner), and
/// its impact can come from at once (it may stop where it starts) to the end of the flight.
fn object_timing(db: &ContentDb, spawn: &SpawnDef, j: usize, depth: u32) -> Option<Timing> {
    let action = by_index(&spawn.action, j)?;
    let mut t = timing_at(db, action, depth)?;
    let Some(lifetime) = by_index(&spawn.prefab, j).and_then(|p| projectile_lifetime(db, p)) else {
        return Some(t);
    };
    let end = lifetime.min(t.length);
    match (t.first, t.last) {
        (Some(first), Some(last)) if first <= end => t.last = Some(last.min(end)),
        _ => (t.first, t.last) = (None, None),
    }
    if let Some(impact) = db.get::<ActionDef>(action).and_then(|d| d.impact.clone())
        && let Some(Timing { first: Some(first), last: Some(last), .. }) = timing_at(db, &impact, depth + 1)
    {
        t.widen(first, end + last);
    }
    Some(t)
}

/// The entry of `list` for index `i`; the last one repeats (as a spawn reads it).
fn by_index(list: &[String], i: usize) -> Option<&str> {
    list.get(i).or(list.last()).map(String::as_str)
}

/// The `lifetime` of prefab `prefab`'s `Projectile` (0 when it leaves it out); `None` when the prefab
/// does not fly.
fn projectile_lifetime(db: &ContentDb, prefab: &str) -> Option<f32> {
    let projectile = prefab_component(db, prefab, "Projectile")?;
    Some(match field(&projectile, "lifetime") {
        Some(ron::Value::Number(n)) => n.into_f64() as f32,
        _ => 0.0,
    })
}

/// Component `component` (its short name or a path ending in it) of prefab `prefab` as RON, from the
/// prefab nearest in its `extends` chain that writes it (a child's component replaces the parent's
/// whole). Reads content with `peek`.
pub(crate) fn prefab_component(db: &ContentDb, prefab: &str, component: &str) -> Option<ron::Value> {
    let suffix = format!("::{component}");
    let mut seen: Vec<String> = Vec::new();
    let mut next = Some(prefab.to_owned());
    while let Some(name) = next.take() {
        if seen.contains(&name) {
            return None;
        }
        let def = db.peek::<PrefabDef>(&name)?;
        if let Some((_, raw)) = def.components.iter().find(|(n, _)| *n == component || n.ends_with(&suffix)) {
            return ron::from_str::<ron::Value>(raw.get_ron()).ok();
        }
        next = def.extends.clone();
        seen.push(name);
    }
    None
}

/// Field `name` of a component read by [`prefab_component`], when it writes it.
pub(crate) fn field(component: &ron::Value, name: &str) -> Option<ron::Value> {
    match component {
        ron::Value::Map(map) => map.iter().find(|(k, _)| matches!(k, ron::Value::String(s) if s == name)).map(|(_, v)| v.clone()),
        _ => None,
    }
}

/// Where option `a`'s chain continues: seconds into `a` when the continuation starts at the earliest
/// (the press at the start of the recovery, taken by the first cancel window that lets the input
/// through), and its action. `None` without a chain or a window.
fn chain_link(db: &ContentDb, plan: &AttackPlanDef, moveset: &MovesetDef, a: &Resolved) -> Option<(f32, String)> {
    let chain = plan.options.iter().find(|o| o.label() == a.name)?.chain.as_ref()?;
    let def = db.get::<ActionDef>(&a.action)?;
    let press = recovery_start(def);
    def.cancels
        .iter()
        .filter_map(|c| {
            let (from, to) = def.span(&c.span)?;
            if to + 1e-4 < press {
                return None;
            }
            let into = c.into.iter().find_map(|t| match t {
                Transition::Combo(input, action) if *input == chain.input => Some(action.clone()),
                Transition::Input(input) if *input == chain.input => moveset.action(input).map(str::to_owned),
                Transition::Any => moveset.action(&chain.input).map(str::to_owned),
                _ => None,
            })?;
            Some((from.max(press), into))
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
}

/// The plan and the moveset, or what is wrong with their ids.
fn load<'a>(db: &'a ContentDb, plan: &str, moveset: &str) -> Result<(&'a AttackPlanDef, &'a MovesetDef), String> {
    let p = db.get::<AttackPlanDef>(plan).ok_or_else(|| {
        let hint = closest(plan, db.peek_ids::<AttackPlanDef>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        format!("no attack_plan `{plan}`{hint}")
    })?;
    let m = db.get::<MovesetDef>(moveset).ok_or_else(|| {
        let hint = closest(moveset, db.peek_ids::<MovesetDef>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        format!("no moveset `{moveset}`{hint}")
    })?;
    Ok((p, m))
}

/// Every ordered pair of options of plan `plan` with moveset `moveset` that the plan lets follow and
/// that both hit, with the shortest time between the end of `a`'s danger and the start of `b`'s first
/// hit: `b` starts at `a`'s end plus `rest.0 * rest_scale`. When `a` has a chain the pair is judged
/// without it and through it (`b` after the whole chain), and the smaller gap is kept. Sorted by gap,
/// shortest first, then by name.
pub fn follow_pairs(db: &ContentDb, plan: &str, moveset: &str, rest_scale: f32) -> Result<Vec<FollowPair>, String> {
    let (plan_def, moveset_def) = load(db, plan, moveset)?;
    let options = resolve(plan_def, db, moveset, 0.0);
    let rest = plan_def.rest.0 * rest_scale;
    let mut pairs = Vec::new();
    for a in &options {
        let Some(ta) = timing(db, &a.action) else { continue };
        // (when the danger ends, when the attack ends, through the chain)
        let mut ways: Vec<(Option<f32>, f32, bool)> = vec![(ta.last, ta.length, false)];
        if let Some((at, link)) = chain_link(db, plan_def, moveset_def, a)
            && let Some(tc) = timing(db, &link)
        {
            let last = match (ta.last.map(|l| l.min(at)), tc.last.map(|l| at + l)) {
                (Some(x), Some(y)) => Some(x.max(y)),
                (x, y) => x.or(y),
            };
            ways.push((last, at + tc.length, true));
        }
        for b in &options {
            if !may_follow(plan_def, a, b) {
                continue;
            }
            let Some(first) = timing(db, &b.action).and_then(|t| t.first) else { continue };
            let best = ways
                .iter()
                .filter_map(|&(last, length, chained)| {
                    let a_end = last?;
                    let b_first = length + rest + first;
                    Some(FollowPair { a: a.name.clone(), b: b.name.clone(), gap: b_first - a_end, a_end, b_first, chained })
                })
                .min_by(|x, y| x.gap.total_cmp(&y.gap));
            pairs.extend(best);
        }
    }
    pairs.sort_by(|x, y| x.gap.total_cmp(&y.gap).then_with(|| x.a.cmp(&y.a)).then_with(|| x.b.cmp(&y.b)));
    Ok(pairs)
}

/// A hit an NPC of the plan can deal.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PlanHit {
    /// The option whose attack deals it.
    pub option: String,
    /// The action whose hit window it is.
    pub action: String,
    /// The actions before it in the attack: the option's own, a chain's continuation, `then`, and for
    /// a spawned object's hit the action that spawned it (and a projectile's flight before its impact).
    pub via: Vec<String>,
    /// Its index in `action`'s `hits`.
    pub hit: usize,
    pub damage: f32,
    pub poise: f32,
    pub flags: Vec<HitFlag>,
    /// How often it can land on one target in one attack: 1, or for a window with `repeat`, once
    /// and then every `repeat` seconds while it lasts; times the objects that run `action` when a
    /// spawn lays it down (a fan of three bolts: 3).
    pub times: u32,
}

/// Every hit an NPC of plan `plan` with moveset `moveset` can deal, by option in the plan's order:
/// through each option's action, its chain's continuation and their `then` actions (each once), and
/// the objects their `Spawn` effects lay down (not those that `stay`: a summon fights on its own).
pub fn plan_hits(db: &ContentDb, plan: &str, moveset: &str) -> Result<Vec<PlanHit>, String> {
    let (plan_def, moveset_def) = load(db, plan, moveset)?;
    let mut hits = Vec::new();
    for a in resolve(plan_def, db, moveset, 0.0) {
        collect_hits(db, &a.name, a.action.clone(), Vec::new(), 1, 0, &mut hits);
        if let Some((_, link)) = chain_link(db, plan_def, moveset_def, &a) {
            collect_hits(db, &a.name, link, vec![a.action.clone()], 1, 0, &mut hits);
        }
    }
    Ok(hits)
}

/// The hits of `start` and the `then` actions after it, each once, run by `objects` bodies, and of
/// what they spawn; `via` is what came before `start`.
fn collect_hits(db: &ContentDb, option: &str, start: String, mut via: Vec<String>, objects: u32, depth: u32, hits: &mut Vec<PlanHit>) {
    let mut next = Some(start);
    while let Some(id) = next.take() {
        if via.contains(&id) {
            break;
        }
        let Some(def) = db.get::<ActionDef>(&id) else { break };
        for (i, hit) in def.hits.iter().enumerate() {
            let window = def.span(&hit.span).map_or(0.0, |(from, to)| (to - from).max(0.0));
            let times = if hit.repeat > 0.0 { 1 + (window / hit.repeat.max(0.05) + 1e-4) as u32 } else { 1 };
            hits.push(PlanHit {
                option: option.to_owned(),
                action: id.clone(),
                via: via.clone(),
                hit: i,
                damage: hit.damage,
                poise: hit.poise,
                flags: hit.flags.clone(),
                times: times * objects,
            });
        }
        next = def.then.clone();
        via.push(id);
        if depth >= SPAWN_DEPTH {
            continue;
        }
        for e in &def.effects {
            let Effect::Spawn(spawn) = &e.effect else { continue };
            if spawn.stay {
                continue;
            }
            // The objects of each action (on every line of a radial pattern), and whether any of them flies.
            let lines = match spawn.pattern {
                SpawnPattern::Radial { lines, .. } => lines as u32,
                _ => 1,
            };
            let mut runs: BTreeMap<&str, (u32, bool)> = BTreeMap::new();
            for j in 0..spawn.count as usize {
                let Some(action) = by_index(&spawn.action, j) else { continue };
                let flies = by_index(&spawn.prefab, j).is_some_and(|p| projectile_lifetime(db, p).is_some());
                let run = runs.entry(action).or_default();
                run.0 += lines;
                run.1 |= flies;
            }
            for (action, (n, flies)) in runs {
                collect_hits(db, option, action.to_owned(), via.clone(), objects * n, depth + 1, hits);
                if flies && let Some(impact) = db.get::<ActionDef>(action).and_then(|d| d.impact.clone()) {
                    let mut flown = via.clone();
                    flown.push(action.to_owned());
                    collect_hits(db, option, impact, flown, objects * n, depth + 1, hits);
                }
            }
        }
    }
}

/// The plan's options the moveset cannot press: no input, or no action, for them. Listed, not fatal.
pub fn unreachable_options(db: &ContentDb, plan: &str, moveset: &str) -> Result<Vec<String>, String> {
    let (plan_def, _) = load(db, plan, moveset)?;
    let reached: Vec<String> = resolve(plan_def, db, moveset, 0.0).into_iter().map(|r| r.name).collect();
    Ok(plan_def.options.iter().map(|o| o.label().to_owned()).filter(|n| !reached.contains(n)).collect())
}
