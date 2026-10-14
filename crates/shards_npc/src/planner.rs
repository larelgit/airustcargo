//! The attack planner, shared by enemies and bosses: which attack next.
//!
//! A plan ([`AttackPlanDef`]) lists options: a moveset input or an action, with a weight, a
//! distance window, a cooldown, tags and a cost. When an NPC holds an attack token and its
//! state strikes, the planner
//!
//! 1. keeps the options that may be chosen: off cooldown, not banned by the last attacks
//!    ([`SequenceBan`]), startable by the action machine's own rule ([`can_start`]: now, or
//!    by waiting), within what is left of the target's collision budget, and allowed to
//!    overlap with the attacks already in flight on it;
//! 2. picks among those whose window holds the distance now (else among all of them, unless
//!    the plan is `in_range_only`), by weight, with the NPC's `SimRng` stream;
//! 3. walks into the chosen option's window, and presses its input when the machine says it
//!    starts now.
//!
//! Started attacks go into the NPC's history ([`NpcPlan`]) for the bans; a rest drawn from
//! the plan follows each attack before the NPC asks for a token again.

use ai_core::{AiId, AiIndex, DetRng, SimRng, SimTick};
use bevy::prelude::*;
use shards_action::{can_start, ActionDef, FighterView, MovesetDef};
use shards_content::ContentDb;
use shards_core::flat;

use crate::brain::{clamp_to_leash, salt};
use crate::def::{AttackPlanDef, DirectorDef, SequenceBan};
use crate::director::{order, target_of, Fighting, Grant, Targets};

/// An NPC's attack history and timers: gameplay state, kept in replays.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcPlan {
    /// The last attacks that started, oldest first (as many as the bans look at).
    pub history: Vec<PastAttack>,
    /// Options on the plan's cooldown.
    pub cooldowns: Vec<PlanCooldown>,
    /// Seconds before it asks for another token.
    pub rest: f32,
    /// Attacks started in all.
    pub attacks: u32,
}

/// An attack that started: its option name and tags.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct PastAttack {
    pub name: String,
    pub tags: Vec<String>,
}

#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct PlanCooldown {
    pub name: String,
    pub left: f32,
}

impl NpcPlan {
    /// Seconds before option `name` may be chosen again.
    pub fn cooldown(&self, name: &str) -> f32 {
        self.cooldowns.iter().find(|c| c.name == name).map_or(0.0, |c| c.left)
    }

    /// Timers run down by `dt`.
    pub fn tick(&mut self, dt: f32) {
        if self.rest > 0.0 {
            self.rest = (self.rest - dt).max(0.0);
        }
        if !self.cooldowns.is_empty() {
            for c in &mut self.cooldowns {
                c.left -= dt;
            }
            self.cooldowns.retain(|c| c.left > 1e-4);
        }
    }

    /// An attack started: into the history (keeping the last `keep`), on cooldown.
    pub fn remember(&mut self, name: &str, tags: &[String], cooldown: f32, keep: usize) {
        self.history.push(PastAttack { name: name.to_owned(), tags: tags.to_vec() });
        let excess = self.history.len().saturating_sub(keep.max(1));
        self.history.drain(..excess);
        self.attacks += 1;
        if cooldown > 0.0 {
            match self.cooldowns.iter_mut().find(|c| c.name == name) {
                Some(c) => c.left = cooldown,
                None => {
                    self.cooldowns.push(PlanCooldown { name: name.to_owned(), left: cooldown });
                    self.cooldowns.sort_by(|a, b| a.name.cmp(&b.name));
                }
            }
        }
    }
}

/// A plan option made concrete for one NPC and target: the input it presses, the action it
/// starts, its window in meters and all its tags.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub input: String,
    pub action: String,
    pub weight: f32,
    pub min: f32,
    pub max: f32,
    pub cooldown: f32,
    /// The option's tags, the action's tags and the option's name.
    pub tags: Vec<String>,
    pub cost: f32,
}

/// The options of `plan` for a fighter with moveset `moveset`, against a target of body
/// radius `target_radius`. Options whose input or action the moveset lacks are left out.
pub fn resolve(plan: &AttackPlanDef, db: &ContentDb, moveset: &str, target_radius: f32) -> Vec<Resolved> {
    let Some(moveset) = db.get::<MovesetDef>(moveset) else { return Vec::new() };
    let mut out: Vec<Resolved> = Vec::new();
    let make = |name: String, input: &str, action: &str, def: &ActionDef, o: Option<&crate::def::AttackOption>| {
        let mut tags: Vec<String> = o.map(|o| o.tags.clone()).unwrap_or_default();
        tags.extend(def.tags.iter().cloned());
        tags.push(name.clone());
        tags.sort();
        tags.dedup();
        let reach = def.reach() * plan.cover + target_radius;
        Resolved {
            input: input.to_owned(),
            action: action.to_owned(),
            weight: o.map_or(1.0, |o| o.weight),
            min: o.map_or(0.0, |o| o.min),
            max: o.and_then(|o| o.max).unwrap_or(reach),
            cooldown: o.map_or(0.0, |o| o.cooldown),
            cost: o.map_or(1.0, |o| o.cost),
            tags,
            name,
        }
    };
    for o in &plan.options {
        let found = match (&o.input, &o.action) {
            (Some(input), _) => moveset.action(input).map(|a| (input.clone(), a.to_owned())),
            (None, Some(action)) => {
                moveset.inputs.iter().find(|(_, a)| *a == action).map(|(i, a)| (i.clone(), a.clone()))
            }
            _ => None,
        };
        let Some((input, action)) = found else { continue };
        let Some(def) = db.get::<ActionDef>(&action) else { continue };
        out.push(make(o.label().to_owned(), &input, &action, def, Some(o)));
    }
    if plan.auto {
        for (input, action) in &moveset.inputs {
            if out.iter().any(|r| r.input == *input) {
                continue;
            }
            let Some(def) = db.get::<ActionDef>(action).filter(|d| !d.hits.is_empty()) else { continue };
            out.push(make(input.clone(), input, action, def, None));
        }
    }
    out
}

/// Whether an attack tagged `tags` is banned after `history` (oldest first).
pub fn banned(bans: &[SequenceBan], tags: &[String], history: &[PastAttack]) -> bool {
    bans.iter().any(|ban| {
        ban.then.iter().any(|t| tags.contains(t))
            && history.iter().rev().take(ban.within as usize).any(|past| ban.after.iter().any(|a| past.tags.contains(a)))
    })
}

/// Index of a weighted pick among `(index, weight)`; weights `<= 0` never win.
pub fn pick_weighted(rng: &mut DetRng, options: &[(usize, f32)]) -> Option<usize> {
    let total: f32 = options.iter().map(|(_, w)| w.max(0.0)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut roll = rng.f32() * total;
    let mut last = None;
    for &(i, w) in options {
        if w <= 0.0 {
            continue;
        }
        last = Some(i);
        if roll < w {
            return Some(i);
        }
        roll -= w;
    }
    last
}

/// Picks among `options` those `allowed` says may be chosen: the ones whose window holds
/// `distance` if any does, else (unless `in_range_only`) all of them; by weight.
pub fn choose(
    options: &[Resolved],
    allowed: impl Fn(&Resolved) -> bool,
    distance: f32,
    in_range_only: bool,
    rng: &mut DetRng,
) -> Option<usize> {
    let eligible: Vec<(usize, f32)> =
        options.iter().enumerate().filter(|(_, o)| o.weight > 0.0 && allowed(o)).map(|(i, o)| (i, o.weight)).collect();
    let fits: Vec<(usize, f32)> =
        eligible.iter().copied().filter(|(i, _)| in_window(&options[*i], distance)).collect();
    if !fits.is_empty() {
        return pick_weighted(rng, &fits);
    }
    if in_range_only {
        return None;
    }
    pick_weighted(rng, &eligible)
}

/// Whether `distance` lies in the option's window.
pub fn in_window(o: &Resolved, distance: f32) -> bool {
    distance >= o.min - 1e-4 && distance <= o.max + 1e-4
}

/// The attacks in flight (pressed or running), by target.
pub(crate) struct InFlight(Vec<(AiId, AiId, f32, Vec<String>)>);

impl InFlight {
    /// From every NPC's held token: `(holder, grant)`.
    pub(crate) fn collect<'a>(tokens: impl Iterator<Item = (AiId, Option<&'a Grant>)>) -> Self {
        let mut all: Vec<(AiId, AiId, f32, Vec<String>)> = tokens
            .filter_map(|(holder, g)| g.filter(|g| g.in_flight()).map(|g| (g.target, holder, g.cost, g.tags.clone())))
            .collect();
        all.sort_by_key(|a| (a.0, a.1));
        Self(all)
    }

    /// What is left of `target`'s budget for `me`, and the tags of the others' attacks on it.
    pub(crate) fn on(&self, target: AiId, me: AiId, director: &DirectorDef) -> (Option<f32>, Vec<Vec<String>>) {
        let others = self.0.iter().filter(|(t, holder, ..)| *t == target && *holder != me);
        let spent: f32 = others.clone().map(|(_, _, cost, _)| *cost).sum();
        (director.budget.map(|b| b - spent), others.map(|(.., tags)| tags.clone()).collect())
    }

    fn add(&mut self, target: AiId, holder: AiId, cost: f32, tags: Vec<String>) {
        self.0.push((target, holder, cost, tags));
    }
}

/// Whether option `o` may be chosen now, distance aside.
#[allow(clippy::too_many_arguments, reason = "every rule an option passes")]
pub(crate) fn feasible(
    o: &Resolved,
    plan_def: &AttackPlanDef,
    plan: &NpcPlan,
    view: &FighterView,
    dt: f32,
    budget_left: Option<f32>,
    overlapping: &[Vec<String>],
    director: &DirectorDef,
) -> bool {
    o.weight > 0.0
        && plan.cooldown(&o.name) <= 0.0
        && !banned(&plan_def.bans, &o.tags, &plan.history)
        && match can_start(view, &o.input, dt) {
            Ok(_) => true,
            Err(blocker) => blocker.ready_in().is_some(),
        }
        && budget_left.is_none_or(|left| o.cost <= left + 1e-4)
        && overlapping.iter().all(|tags| director.may_overlap(&o.tags, tags))
}

/// `NpcSystems::Plan`: every token holder that strikes, in `AiId` order, chooses an attack,
/// closes in and presses it when the machine says it starts now and the target can take it
/// (budget, combinations).
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub(crate) fn plan_attacks(
    time: Res<Time>,
    tick: Res<SimTick>,
    rng: SimRng,
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    targets: Targets,
    mut npcs: Query<Fighting>,
) {
    let dt = time.delta_secs();
    let mut inflight = InFlight::collect(npcs.iter().map(|f| (*f.id, f.token.held.as_ref())));
    for (id, entity) in order(&npcs) {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        if !f.drive.strike || f.dead || !f.brain.active {
            continue;
        }
        let Some(mut grant) = f.token.held.clone() else { continue };
        let Some((target, radius)) = target_of(&index, &targets, grant.target) else { continue };
        let here = f.transform.translation;
        let distance = flat(target - here).length();
        f.drive.waiting = false;
        f.drive.face = Some(target);
        if grant.in_flight() {
            // Pressed or swinging: stand and face it.
            f.drive.goal = None;
            continue;
        }
        let Some(plan_def) = f.npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)) else { continue };
        let director = DirectorDef::of(&db, &f.npc.director);
        let options = resolve(plan_def, &db, &f.fighter.moveset, radius);
        let (budget, overlapping) = inflight.on(grant.target, id, &director);
        let chosen = {
            let view = FighterView {
                db: &db,
                moveset: &f.fighter.moveset,
                state: f.state,
                stamina: f.stamina,
                cooldowns: f.cooldowns,
                charges: f.charges,
                held: &f.drive.held,
                dead: f.dead,
            };
            let ok = |o: &Resolved| feasible(o, plan_def, &f.plan, &view, dt, budget, &overlapping, &director);
            let kept = grant.attack.as_ref().and_then(|name| options.iter().position(|o| o.name == *name));
            // In range only: choose again every tick among what fits (a boss picks what
            // reaches); else keep the choice while it stays possible.
            match kept.filter(|&i| !plan_def.in_range_only && ok(&options[i])) {
                Some(i) => Some(i),
                None => choose(&options, ok, distance, plan_def.in_range_only, &mut rng.stream(id, salt::PICK)),
            }
        };
        let home = f.home.map(|h| (h.pos.unwrap_or(here), h.leash));
        let leash = |p: Vec3| clamp_to_leash(home.map(|h| h.0), home.map_or(0.0, |h| h.1), p);
        let Some(i) = chosen else {
            grant.attack = None;
            // Nothing to choose here: close in to the nearest window that could fit.
            let nearest = options.iter().map(|o| o.max).fold(f32::INFINITY, f32::min);
            if nearest.is_finite() && distance > nearest {
                f.drive.goal = Some(leash(target));
                f.drive.within = (nearest * 0.85).max(0.3);
            } else {
                f.drive.goal = None;
            }
            f.token.held = Some(grant);
            continue;
        };
        let o = &options[i];
        grant.attack = Some(o.name.clone());
        grant.input = Some(o.input.clone());
        grant.cost = o.cost;
        grant.tags = o.tags.clone();
        grant.cooldown = o.cooldown;
        if distance > o.max {
            f.drive.goal = Some(leash(target));
            f.drive.within = (o.max * 0.85).max(o.min + 0.2).min(o.max);
        } else if distance < o.min {
            let away = flat(here - target).try_normalize().unwrap_or(-*f.transform.forward());
            f.drive.goal = Some(leash(target + away * (o.min + 0.3)));
            f.drive.within = 0.2;
        } else {
            f.drive.goal = None;
            let view = FighterView {
                db: &db,
                moveset: &f.fighter.moveset,
                state: f.state,
                stamina: f.stamina,
                cooldowns: f.cooldowns,
                charges: f.charges,
                held: &f.drive.held,
                dead: f.dead,
            };
            let starts = can_start(&view, &o.input, dt).is_ok();
            let fits = budget.is_none_or(|left| o.cost <= left + 1e-4)
                && overlapping.iter().all(|tags| director.may_overlap(&o.tags, tags));
            if starts && fits {
                grant.pressed = Some(tick.0);
                grant.instance = f.state.started;
                f.drive.press.push(o.input.clone());
                inflight.add(grant.target, id, o.cost, o.tags.clone());
            }
        }
        f.token.held = Some(grant);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(name: &str, weight: f32, min: f32, max: f32, tags: &[&str]) -> Resolved {
        let mut tags: Vec<String> = tags.iter().map(|t| (*t).to_owned()).collect();
        tags.push(name.to_owned());
        Resolved {
            name: name.to_owned(),
            input: name.to_owned(),
            action: name.to_owned(),
            weight,
            min,
            max,
            cooldown: 0.0,
            tags,
            cost: 1.0,
        }
    }

    fn zone_ban() -> Vec<SequenceBan> {
        vec![SequenceBan { after: vec!["zone".into()], then: vec!["zone".into()], within: 1 }]
    }

    #[test]
    fn weights_hold_over_many_seeds() {
        let options = [option("a", 1.0, 0.0, 5.0, &[]), option("b", 2.0, 0.0, 5.0, &[]), option("c", 3.0, 0.0, 5.0, &[])];
        let mut counts = [0u32; 3];
        let n = 60_000;
        for seed in 0..n {
            let mut rng = DetRng::from_keys(&[seed, 7, 3, salt::PICK]);
            counts[choose(&options, |_| true, 1.0, false, &mut rng).unwrap()] += 1;
        }
        for (i, expected) in [1.0 / 6.0, 2.0 / 6.0, 3.0 / 6.0].into_iter().enumerate() {
            let share = counts[i] as f64 / n as f64;
            assert!((share - expected).abs() < 0.01, "option {i}: {share:.3} instead of {expected:.3} ({counts:?})");
        }
    }

    #[test]
    fn a_zero_weight_never_wins() {
        let options = [option("a", 0.0, 0.0, 5.0, &[]), option("b", 1.0, 0.0, 5.0, &[])];
        for seed in 0..2000 {
            assert_eq!(choose(&options, |_| true, 1.0, false, &mut DetRng::new(seed)), Some(1));
        }
        assert_eq!(pick_weighted(&mut DetRng::new(1), &[(0, 0.0)]), None);
    }

    #[test]
    fn distance_windows_choose_what_reaches() {
        let options = [option("jab", 1.0, 0.0, 2.0, &[]), option("lunge", 1.0, 3.0, 6.0, &[])];
        for seed in 0..500 {
            let mut rng = DetRng::new(seed);
            assert_eq!(choose(&options, |_| true, 1.5, false, &mut rng), Some(0), "only the jab reaches 1.5m");
            assert_eq!(choose(&options, |_| true, 4.0, false, &mut rng), Some(1), "only the lunge fits 4m");
            // 2.5 m: neither fits; it picks one to walk into, unless it may only pick in range.
            assert!(choose(&options, |_| true, 2.5, false, &mut rng).is_some());
            assert_eq!(choose(&options, |_| true, 2.5, true, &mut rng), None);
            assert_eq!(choose(&options, |_| true, 9.0, true, &mut rng), None);
        }
    }

    #[test]
    fn cooldowns_keep_an_option_out_until_they_run_out() {
        let mut plan = NpcPlan::default();
        plan.remember("slam", &["slam".into()], 1.0, 1);
        assert!(plan.cooldown("slam") > 0.0 && plan.cooldown("jab") == 0.0);
        let options = [option("slam", 100.0, 0.0, 5.0, &[]), option("jab", 1.0, 0.0, 5.0, &[])];
        for t in 0..60 {
            let pick = choose(&options, |o| plan.cooldown(&o.name) <= 0.0, 1.0, false, &mut DetRng::new(t)).unwrap();
            assert_eq!(options[pick].name, "jab", "slam is on cooldown at {t}/60 s");
            plan.tick(1.0 / 60.0);
        }
        plan.tick(1.0 / 60.0);
        assert_eq!(plan.cooldown("slam"), 0.0);
        assert!(plan.cooldowns.is_empty());
    }

    #[test]
    fn bans_look_back_as_far_as_they_say() {
        let bans = zone_ban();
        let past = |name: &str, zone: bool| PastAttack {
            name: name.into(),
            tags: if zone { vec!["zone".into(), name.into()] } else { vec![name.into()] },
        };
        let zone_tags = vec!["zone".to_owned(), "slam".to_owned()];
        assert!(banned(&bans, &zone_tags, &[past("sweep", true)]));
        assert!(!banned(&bans, &zone_tags, &[past("sweep", true), past("slash", false)]), "one plain attack between");
        assert!(!banned(&bans, &["slash".to_owned()], &[past("sweep", true)]));
        let two = [SequenceBan { after: vec!["sweep".into()], then: vec!["slam".into()], within: 2 }];
        assert!(banned(&two, &zone_tags, &[past("sweep", true), past("slash", false)]), "sweep two attacks ago");
        assert!(!banned(&two, &zone_tags, &[past("sweep", true), past("a", false), past("b", false)]));
    }

    /// Thousands of picks in a row with the history the planner keeps: never two zone
    /// attacks in a row, never slam right after sweep, and every option still played.
    #[test]
    fn no_forbidden_sequence_ever() {
        let mut bans = zone_ban();
        bans.push(SequenceBan { after: vec!["sweep".into()], then: vec!["slam".into()], within: 2 });
        let plan_def = AttackPlanDef { bans, ..default() };
        let options = [
            option("slash", 1.0, 0.0, 5.0, &[]),
            option("slam", 2.0, 0.0, 5.0, &["zone"]),
            option("sweep", 2.0, 0.0, 5.0, &["zone"]),
            option("shockwave", 2.0, 0.0, 5.0, &["zone"]),
            option("jab", 1.0, 0.0, 5.0, &[]),
        ];
        for seed in 0..20u64 {
            let mut plan = NpcPlan::default();
            let mut played: Vec<usize> = Vec::new();
            for tick in 0..2000u64 {
                let mut rng = DetRng::from_keys(&[seed, tick, 9, salt::PICK]);
                let allowed = |o: &Resolved| !banned(&plan_def.bans, &o.tags, &plan.history);
                let i = choose(&options, allowed, 1.0, false, &mut rng).expect("something is always allowed");
                plan.remember(&options[i].name, &options[i].tags, 0.0, plan_def.memory());
                played.push(i);
            }
            for w in played.windows(2) {
                let (a, b) = (&options[w[0]], &options[w[1]]);
                assert!(!(a.tags.contains(&"zone".into()) && b.tags.contains(&"zone".into())), "{} then {}", a.name, b.name);
            }
            for w in played.windows(3) {
                let names: Vec<&str> = w.iter().map(|i| options[*i].name.as_str()).collect();
                assert!(!(names[0] == "sweep" && names[2] == "slam") && !(names[1] == "sweep" && names[2] == "slam"), "{names:?}");
            }
            for (i, o) in options.iter().enumerate() {
                assert!(played.contains(&i), "seed {seed}: {} never played", o.name);
            }
        }
    }

    #[test]
    fn budgets_and_combinations_come_from_the_attacks_in_flight() {
        let grant = |target: u32, cost: f32, tags: &[&str], pressed: bool| Grant {
            cost,
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            pressed: pressed.then_some(1),
            ..Grant::for_tests(AiId(target))
        };
        let (a, b, c) = (grant(1, 1.0, &["light"], true), grant(1, 1.5, &["heavy"], true), grant(1, 9.0, &["heavy"], false));
        let flight = InFlight::collect([(AiId(5), Some(&a)), (AiId(6), Some(&b)), (AiId(7), Some(&c)), (AiId(8), None)].into_iter());
        let director = DirectorDef { budget: Some(3.0), ..default() };
        let (left, tags) = flight.on(AiId(1), AiId(7), &director);
        assert_eq!(left, Some(0.5), "pressed attacks count, chosen-but-not-pressed ones do not");
        assert_eq!(tags.len(), 2);
        let (left, tags) = flight.on(AiId(1), AiId(5), &director);
        assert_eq!((left, tags.len()), (Some(1.5), 1), "its own attack is not in its way");
        assert_eq!(flight.on(AiId(2), AiId(5), &DirectorDef::default()), (None, Vec::new()));
    }
}
