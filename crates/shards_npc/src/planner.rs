//! The attack planner, shared by enemies and bosses: which attack next.
//!
//! A plan ([`AttackPlanDef`]) lists options: a moveset input or an action, with a weight, a
//! distance window, a cooldown, tags and a cost. When an NPC holds an attack token and its
//! state strikes, the planner
//!
//! 1. keeps the options that may be chosen: off cooldown, open by their conditions, not banned
//!    by the last attacks ([`SequenceBan`]), startable by the action machine's own rule
//!    ([`can_start`]: now, or by waiting), within what is left of the target's collision budget,
//!    allowed to overlap with the attacks already in flight on it, and spaced from the other
//!    NPCs' strikes (`refusal` names the rule that keeps an option out, [`crate::schedule`]);
//! 2. picks among those whose window holds the distance now (else among all of them, unless
//!    the plan is `in_range_only`), by weight, with the NPC's `SimRng` stream;
//! 3. walks into the chosen option's window, and presses its input when the machine says it
//!    starts now.
//!
//! Started attacks go into the NPC's history ([`NpcPlan`]) for the bans; a rest drawn from
//! the plan follows each attack before the NPC asks for a token again.

use ai_core::{AiId, AiIndex, DetRng, SimRng, SimTick};
use bevy::prelude::*;
use shards_action::{can_start, ActionDef, MovesetDef};
use shards_motion::PhysicsWorld;
use shards_content::ContentDb;
use shards_core::flat;

use crate::brain::{clamp_to_leash, salt};
use crate::def::{AttackPlanDef, DirectorDef, SequenceBan};
use crate::director::{order, target_of, Fighting, Grant, Targets};
use crate::schedule::{inputs, plan_notes, FightCx, Inputs, Refusal};
use crate::strike::pool;

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

/// Which gated options of an NPC\'s plan are open: the names of those whose `when` holds,
/// decided each tick with its other decisions (`NpcSystems::Decide`). Only NPCs whose plan
/// has an option with `when` carry it; such an option is closed without it.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcOptions {
    pub allowed: Vec<String>,
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

    /// Timers run down by `dt`: the cooldowns always, the rest only while the NPC is `free`
    /// (running no action): a stagger after a parried attack is no rest.
    pub fn tick(&mut self, dt: f32, free: bool) {
        if self.rest > 0.0 && free {
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
    /// The option has `when` conditions: it may be chosen only while the NPC's [`NpcOptions`] lists it.
    pub gated: bool,
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
            gated: o.is_some_and(|o| !o.when.is_empty()),
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
    ban_of(bans, tags, history).is_some()
}

/// [`banned`] with the reason: the first ban (its index in `bans`) that forbids an attack tagged
/// `tags` after `history`, and the newest past attack it looks back to.
pub(crate) fn ban_of<'a>(bans: &[SequenceBan], tags: &[String], history: &'a [PastAttack]) -> Option<(usize, &'a PastAttack)> {
    bans.iter().enumerate().find_map(|(i, ban)| {
        if !ban.then.iter().any(|t| tags.contains(t)) {
            return None;
        }
        let after = history.iter().rev().take(ban.within as usize).find(|past| ban.after.iter().any(|a| past.tags.contains(a)))?;
        Some((i, after))
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

/// How near an NPC with nothing chosen closes in: the shortest `max` among the options
/// `allowed` now, else among those it may ever choose (weight above 0); infinite with none.
/// A window that cannot be chosen would draw it inside the ones that can.
pub(crate) fn approach_window(options: &[Resolved], allowed: impl Fn(&Resolved) -> bool) -> f32 {
    let shortest = |pick: &dyn Fn(&Resolved) -> bool| options.iter().filter(|o| pick(o)).map(|o| o.max).fold(f32::INFINITY, f32::min);
    let open = shortest(&|o| o.weight > 0.0 && allowed(o));
    if open.is_finite() { open } else { shortest(&|o| o.weight > 0.0) }
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

    /// What is left of `target`'s budget for `me`, and the others' attacks on it: their holders
    /// and tags.
    pub(crate) fn on(&self, target: AiId, me: AiId, director: &DirectorDef) -> (Option<f32>, Vec<(AiId, Vec<String>)>) {
        let others = self.0.iter().filter(|(t, holder, ..)| *t == target && *holder != me);
        let spent: f32 = others.clone().map(|(_, _, cost, _)| *cost).sum();
        (director.budget.map(|b| b - spent), others.map(|(_, holder, _, tags)| (*holder, tags.clone())).collect())
    }

    fn add(&mut self, target: AiId, holder: AiId, cost: f32, tags: Vec<String>) {
        self.0.push((target, holder, cost, tags));
    }
}

/// Why option `o` may not be chosen now, distance aside: the first rule that refuses it, in the
/// order of [`Refusal`]; `None`: it may be chosen. The director's grant and the planner both ask
/// it ([`crate::schedule`]). Every rule is a pure read, so the order decides only which rule is
/// named, never whether the option passes. The action machine is asked exactly where the rules
/// before `refusal` asked it (after the weight, the cooldown and the bans, a closed option too):
/// its rule reads definitions no other rule reads (the action a running action's cancel window
/// or a context binding routes the input to), and the replay's `content` check records what the
/// simulation read, so a recording made before verifies the same.
pub(crate) fn refusal(o: &Resolved, i: &Inputs) -> Option<Refusal> {
    let weighted = o.weight > 0.0;
    if !weighted {
        return Some(Refusal::Weight);
    }
    let left = i.plan.cooldown(&o.name);
    let cooled = left <= 0.0;
    if !cooled {
        return Some(Refusal::Cooldown { left });
    }
    let closed = o.gated && !i.open.is_some_and(|options| options.allowed.contains(&o.name));
    let ban = ban_of(&i.plan_def.bans, &o.tags, &i.plan.history);
    // Startable now, or by waiting: a busy fighter, stamina or an action cooldown only delay it
    // while the machine gives a time it would start by; with none (a held action with no end,
    // stamina that never regenerates to the cost) it refuses, as death, a missing input or action,
    // no charges, the mode gate, a leap off the ground or with no landing do. Asked of a closed
    // option too, its answer unused: see above.
    let machine = if ban.is_none() {
        can_start(&i.view, &o.input, i.dt).err().filter(|blocker| blocker.ready_in().is_none())
    } else {
        None
    };
    if closed {
        let when = i.plan_def.options.iter().find(|p| p.label() == o.name).map_or_else(String::new, |p| {
            p.when.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        });
        return Some(Refusal::Closed { when });
    }
    if let Some((ban, past)) = ban {
        return Some(Refusal::Banned { after: past.name.clone(), ban });
    }
    if let Some(blocker) = machine {
        return Some(Refusal::Machine { reason: blocker.reason() });
    }
    if let Some(left) = over_budget(o, i) {
        return Some(Refusal::Budget { left, cost: o.cost });
    }
    if let Some((by, with)) = overlap(o, i) {
        return Some(Refusal::Overlap { by: *by, with: with.clone() });
    }
    if let Some((by, ticks)) = i.spacing
        && i.director.is_strike(&o.tags)
    {
        return Some(Refusal::Spacing { by, ticks });
    }
    None
}

/// Whether option `o` may be chosen now, distance aside: no rule refuses it.
pub(crate) fn feasible(o: &Resolved, i: &Inputs) -> bool {
    refusal(o, i).is_none()
}

/// Whether the target can take option `o` now: it fits in what is left of the target's budget
/// and may overlap with the attacks in flight on it. The planner asks it again at the press.
pub(crate) fn fits(o: &Resolved, i: &Inputs) -> bool {
    over_budget(o, i).is_none() && overlap(o, i).is_none()
}

/// What is left of the target's budget, when option `o` costs more than that.
fn over_budget(o: &Resolved, i: &Inputs) -> Option<f32> {
    let within = |left: f32| o.cost <= left + 1e-4;
    i.budget.filter(|&left| !within(left))
}

/// The first attack in flight on the target that option `o` may not overlap with: its holder and
/// its tags.
fn overlap<'a>(o: &Resolved, i: &'a Inputs) -> Option<&'a (AiId, Vec<String>)> {
    i.overlapping.iter().find(|(_, tags)| !i.director.may_overlap(&o.tags, tags))
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
    physics: Res<PhysicsWorld>,
    targets: Targets,
    mut npcs: Query<Fighting>,
) {
    let dt = time.delta_secs();
    let cx = FightCx { db: &db, physics: &physics, tick: tick.0, dt };
    let mut inflight = InFlight::collect(npcs.iter().map(|f| (*f.id, f.token.held.as_ref())));
    let mut notes = plan_notes(&db, npcs.iter(), tick.0, dt);
    for (id, entity) in order(&npcs) {
        let (mut grant, target, radius, distance) = {
            let Ok(mut f) = npcs.get_mut(entity) else { continue };
            if !f.drive.strike || f.dead || !f.brain.active || f.under_orders() {
                continue;
            }
            let Some(grant) = f.token.held.clone() else { continue };
            let Some((target, radius)) = target_of(&index, &targets, f.perceived, grant.target) else { continue };
            let distance = flat(target - f.transform.translation).length();
            f.drive.waiting = false;
            f.drive.face = Some(target);
            if grant.in_flight() {
                // Pressed or swinging: stand and face it.
                f.drive.goal = None;
                continue;
            }
            (grant, target, radius, distance)
        };
        // Judged on the NPC's read-only view, by the rules the director's grant judges by.
        let (options, chosen, nearest, decides, ready) = {
            let Ok(f) = npcs.get(entity) else { continue };
            let Some(inputs) = inputs(&f, &cx, (grant.target, target, radius), &notes, &inflight) else { continue };
            let plan_def = inputs.plan_def;
            let options = resolve(plan_def, &db, &f.fighter.moveset, radius);
            // With `press_on_decision` a new choice, like the press, waits for the NPC's decision tick.
            let decides = !plan_def.press_on_decision || f.think.is_none_or(|t| t.decided == Some(tick.0));
            let ok = |o: &Resolved| feasible(o, &inputs);
            let kept = grant.attack.as_ref().and_then(|name| options.iter().position(|o| o.name == *name));
            // In range only: choose again every tick among what fits (a boss picks what
            // reaches); else keep the choice while it stays possible.
            let chosen = match kept.filter(|&i| (!plan_def.in_range_only || !decides) && ok(&options[i])) {
                Some(i) => Some(i),
                None if !decides => None,
                None => choose(&options, ok, distance, plan_def.in_range_only, &mut rng.stream(id, salt::PICK)),
            };
            // Once in its window it presses the choice when the machine starts it now, the target
            // can take it (budget, combinations), and it swings at what it perceives now, not at a
            // memory.
            let ready = chosen.is_some_and(|i| {
                let o = &options[i];
                let sensed = f.perceived.get(grant.target).is_some_and(|k| k.age <= 1e-4);
                can_start(&inputs.view, &o.input, dt).is_ok() && fits(o, &inputs) && sensed
            });
            let nearest = approach_window(&options, ok);
            (options, chosen, nearest, decides, ready)
        };
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let here = f.transform.translation;
        let home = f.home.map(|h| (h.pos.unwrap_or(here), h.leash));
        let leash = |p: Vec3| clamp_to_leash(home.map(|h| h.0), home.map_or(0.0, |h| h.1), p);
        let Some(i) = chosen else {
            grant.attack = None;
            // Nothing to choose here (between two decisions: nothing chosen yet): close in to the
            // nearest window it could choose. Only the choice and the press wait for a decision tick.
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
            if ready && decides {
                grant.pressed = Some(tick.0);
                grant.instance = f.state.started;
                f.drive.press.push(o.input.clone());
                inflight.add(grant.target, id, o.cost, o.tags.clone());
                notes.push((id, pool(&f.npc.director), tick.0));
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
            gated: false,
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
    fn with_nothing_chosen_it_closes_in_to_a_window_it_could_choose() {
        // A link only a chain reaches (weight 0) has the shortest window: it must not draw the
        // NPC inside the lunge's minimum, where nothing it may choose fits.
        let options = [option("link", 0.0, 0.0, 1.0, &[]), option("lunge", 1.0, 3.0, 6.0, &[]), option("jab", 1.0, 0.0, 2.0, &[])];
        assert_eq!(approach_window(&options, |_| true), 2.0);
        // The jab on its cooldown: the lunge's window, while the jab waits.
        assert_eq!(approach_window(&options, |o| o.name != "jab"), 6.0);
        // Nothing allowed now (spacing, budget): the shortest of what it may choose later, so it keeps coming.
        assert_eq!(approach_window(&options, |_| false), 2.0);
        assert_eq!(approach_window(&[option("link", 0.0, 0.0, 1.0, &[])], |_| true), f32::INFINITY);
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
            plan.tick(1.0 / 60.0, false);
        }
        plan.tick(1.0 / 60.0, false);
        assert_eq!(plan.cooldown("slam"), 0.0);
        assert!(plan.cooldowns.is_empty(), "cooldowns run while busy too");
    }

    #[test]
    fn the_rest_after_an_attack_waits_while_the_npc_is_busy() {
        let mut plan = NpcPlan { rest: 1.0, ..Default::default() };
        for _ in 0..96 {
            plan.tick(1.0 / 60.0, false); // staggered: no rest
        }
        assert_eq!(plan.rest, 1.0);
        for _ in 0..30 {
            plan.tick(1.0 / 60.0, true);
        }
        assert!((plan.rest - 0.5).abs() < 1e-4, "{}", plan.rest);
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

    /// The ban rule as it was written before [`ban_of`], word for word: `banned` is now
    /// `ban_of(..).is_some()`, so the old conjunction below asks this copy instead.
    fn banned_old(bans: &[SequenceBan], tags: &[String], history: &[PastAttack]) -> bool {
        bans.iter().any(|ban| {
            ban.then.iter().any(|t| tags.contains(t))
                && history.iter().rev().take(ban.within as usize).any(|past| ban.after.iter().any(|a| past.tags.contains(a)))
        })
    }

    /// The rules as they were written before [`refusal`], word for word (only `banned` reads
    /// [`banned_old`]): the conjunction the tests below hold `refusal` to (no rule added, none
    /// dropped, none changed), in its order, which is the order it asked the action machine in.
    #[allow(clippy::too_many_arguments, reason = "the old signature, kept as it was")]
    fn feasible_old(
        o: &Resolved,
        plan_def: &AttackPlanDef,
        plan: &NpcPlan,
        view: &shards_action::FighterView,
        dt: f32,
        budget_left: Option<f32>,
        overlapping: &[Vec<String>],
        director: &DirectorDef,
        spaced_out: bool,
        open: Option<&NpcOptions>,
    ) -> bool {
        o.weight > 0.0
            && plan.cooldown(&o.name) <= 0.0
            && !banned_old(&plan_def.bans, &o.tags, &plan.history)
            && match can_start(view, &o.input, dt) {
                Ok(_) => true,
                Err(blocker) => blocker.ready_in().is_some(),
            }
            && budget_left.is_none_or(|left| o.cost <= left + 1e-4)
            && overlapping.iter().all(|tags| director.may_overlap(&o.tags, tags))
            && (!spaced_out || !director.is_strike(&o.tags))
            && (!o.gated || open.is_some_and(|options| options.allowed.contains(&o.name)))
    }

    /// The planner's press check of the budget and the combinations as it was written before [`fits`].
    fn fits_old(o: &Resolved, budget: Option<f32>, overlapping: &[Vec<String>], director: &DirectorDef) -> bool {
        budget.is_none_or(|left| o.cost <= left + 1e-4) && overlapping.iter().all(|tags| director.may_overlap(&o.tags, tags))
    }

    const DT: f32 = 1.0 / 60.0;

    /// Content for the machine's rule: moveset `m` starts `act_a` by `a` and `act_b` by `b`, and
    /// `act_h` by `h`; it has no `z`. While `act_a` swings, its cancel window routes `b` into
    /// `act_c`, an action no input of the moveset names; `act_h` loops while `a` is held. The
    /// database counts what is read while the returned scope is open, as a world's does.
    fn machine_db() -> (ContentDb, ai_core::SimScope) {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        let source = |path: &str, text: &str| shards_content::db::Source { path: path.into(), text: text.into() };
        let mut db = ContentDb::build(
            &kinds,
            &[
                source(
                    "content/t.action.ron",
                    r#"{
                        "act_a": (net_id: 1, phases: [("swing", 0.5)], cancels: [(span: Phase("swing"), into: [Combo("b", "act_c")])]),
                        "act_b": (net_id: 2, phases: [("swing", 0.5)]),
                        "act_c": (net_id: 3, phases: [("swing", 0.5)]),
                        "act_h": (net_id: 4, phases: [("loop", 0.5)], hold: (input: "a", span: Phase("loop"))),
                    }"#,
                ),
                source("content/t.moveset.ron", r#"{ "m": (inputs: {"a": "act_a", "b": "act_b", "h": "act_h"}) }"#),
            ],
        );
        let scope = ai_core::SimScope::default();
        db.watch(Some(scope.clone()));
        (db, scope)
    }

    /// A fighter running `action` (of `machine_db`) `t` seconds in.
    fn running(action: &str, t: f32) -> shards_action::ActionState {
        let phase = if action == "act_h" { "loop" } else { "swing" };
        let run = shards_action::Running {
            action: action.into(),
            instance: 1,
            t,
            prev: (t - DT).max(0.0),
            length: 0.5,
            facing: Vec3::NEG_Z,
            dir: None,
            input: None,
            forced: false,
            hit: Vec::new(),
            phase: phase.into(),
            phase_end: 0.5,
            windows: default(),
            zone: None,
            zone_until: 0.0,
            anim: None,
            held: 0.0,
        };
        shards_action::ActionState { current: Some(run), started: 1 }
    }

    /// The definitions `read` reads (`kind/id`), counted from nothing.
    fn reads(db: &ContentDb, scope: &ai_core::SimScope, read: impl FnOnce()) -> Vec<String> {
        db.set_used(&std::collections::BTreeSet::new());
        let open = scope.open();
        read();
        drop(open);
        db.used_parts().into_iter().map(|(part, _)| part).collect()
    }

    /// One option judged on one set of inputs, all owned, so a test can change what any rule reads.
    #[derive(Debug)]
    struct Case {
        o: Resolved,
        plan_def: AttackPlanDef,
        plan: NpcPlan,
        /// What the fighter runs, and the inputs it holds.
        state: shards_action::ActionState,
        held: Vec<String>,
        dead: bool,
        cooldowns: Option<shards_action::Cooldowns>,
        budget: Option<f32>,
        overlapping: Vec<(AiId, Vec<String>)>,
        director: DirectorDef,
        spacing: Option<(AiId, u64)>,
        open: Option<NpcOptions>,
    }

    /// What the rules said of a case: `refusal`, `feasible`, the old conjunction, whether `fits`
    /// agrees with the old press check, and the definitions `refusal` read (`kind/id`), with
    /// whether the old conjunction read the same.
    struct Judged {
        refusal: Option<Refusal>,
        feasible: bool,
        old: bool,
        fits_agree: bool,
        reads: Vec<String>,
        reads_agree: bool,
    }

    impl Case {
        /// Everything open: option `a` (weight 1, cost 1), nothing on cooldown, no bans, no budget,
        /// no attack in flight, no spacing.
        fn open() -> Self {
            Self {
                o: option("a", 1.0, 0.0, 5.0, &["light"]),
                plan_def: AttackPlanDef::default(),
                plan: NpcPlan::default(),
                state: default(),
                held: Vec::new(),
                dead: false,
                cooldowns: None,
                budget: None,
                overlapping: Vec::new(),
                director: DirectorDef::default(),
                spacing: None,
                open: None,
            }
        }

        fn judge(&self, db: &ContentDb, scope: &ai_core::SimScope) -> Judged {
            let view = shards_action::FighterView {
                cooldowns: self.cooldowns.as_ref(),
                ..shards_action::FighterView::new(db, "m", &self.state, &self.held, self.dead)
            };
            let inputs = Inputs {
                plan_def: &self.plan_def,
                plan: &self.plan,
                view,
                dt: DT,
                budget: self.budget,
                overlapping: self.overlapping.clone(),
                director: self.director.clone(),
                spacing: self.spacing,
                open: self.open.as_ref(),
            };
            let tags: Vec<Vec<String>> = self.overlapping.iter().map(|(_, tags)| tags.clone()).collect();
            let mut old = false;
            let old_reads = reads(db, scope, || {
                old = feasible_old(
                    &self.o,
                    &self.plan_def,
                    &self.plan,
                    &inputs.view,
                    DT,
                    self.budget,
                    &tags,
                    &self.director,
                    self.spacing.is_some(),
                    self.open.as_ref(),
                );
            });
            let mut judged = None;
            let new_reads = reads(db, scope, || judged = refusal(&self.o, &inputs));
            Judged {
                refusal: judged,
                feasible: feasible(&self.o, &inputs),
                old,
                fits_agree: fits(&self.o, &inputs) == fits_old(&self.o, self.budget, &tags, &self.director),
                reads_agree: new_reads == old_reads,
                reads: new_reads,
            }
        }
    }

    /// The rules in the order `refusal` asks them.
    const RULES: [&str; 8] = ["weight", "cooldown", "closed", "banned", "machine", "budget", "overlap", "spacing"];

    fn rule(r: &Refusal) -> &'static str {
        match r {
            Refusal::Weight => "weight",
            Refusal::Cooldown { .. } => "cooldown",
            Refusal::Closed { .. } => "closed",
            Refusal::Banned { .. } => "banned",
            Refusal::Machine { .. } => "machine",
            Refusal::Budget { .. } => "budget",
            Refusal::Overlap { .. } => "overlap",
            Refusal::Spacing { .. } => "spacing",
        }
    }

    /// A change to a case that closes one rule alone, and the refusal it gives.
    type Closer = (fn(&mut Case), Refusal);

    /// For each rule of [`RULES`], in order, its [`Closer`].
    fn closers() -> [Closer; 8] {
        [
            (|c| c.o.weight = 0.0, Refusal::Weight),
            (|c| c.plan.cooldowns = vec![PlanCooldown { name: "a".into(), left: 0.5 }], Refusal::Cooldown { left: 0.5 }),
            (
                |c| {
                    c.o.gated = true;
                    c.plan_def.options = vec![crate::def::AttackOption {
                        input: Some("a".into()),
                        when: vec![crate::def::Cond::new("enraged", default()), crate::def::Cond::new("!calm", default())],
                        ..default()
                    }];
                    c.open = Some(NpcOptions { allowed: vec!["b".into()] });
                },
                Refusal::Closed { when: "enraged, !calm".into() },
            ),
            (
                |c| {
                    c.plan_def.bans = vec![SequenceBan { after: vec!["zone".into()], then: vec!["a".into()], within: 2 }];
                    let past = |name: &str, tags: &[&str]| PastAttack { name: name.into(), tags: tags.iter().map(|t| (*t).to_owned()).collect() };
                    c.plan.history = vec![past("slam", &["slam", "zone"]), past("jab", &["jab"])];
                },
                Refusal::Banned { after: "slam".into(), ban: 0 },
            ),
            (|c| c.dead = true, Refusal::Machine { reason: "dead".into() }),
            (|c| c.budget = Some(0.5), Refusal::Budget { left: 0.5, cost: 1.0 }),
            (
                |c| {
                    c.director.allowed = vec![("light".into(), "light".into())];
                    c.overlapping = vec![(AiId(4), vec!["light".into()]), (AiId(6), vec!["heavy".into()])];
                },
                Refusal::Overlap { by: AiId(6), with: vec!["heavy".into()] },
            ),
            (|c| c.spacing = Some((AiId(7), 12)), Refusal::Spacing { by: AiId(7), ticks: 12 }),
        ]
    }

    /// Each rule closed alone gives its own refusal, with the numbers that refused it; two closed
    /// give the earlier one; and wherever `refusal` refuses, the old conjunction did too, having
    /// read the same definitions.
    #[test]
    fn each_rule_refuses_by_its_name_and_the_first_in_order_is_named() {
        let (db, scope) = machine_db();
        let judged = Case::open().judge(&db, &scope);
        assert_eq!(judged.refusal, None);
        assert!(judged.feasible && judged.old && judged.fits_agree && judged.reads_agree);
        let closers = closers();
        for (k, (close, expected)) in closers.iter().enumerate() {
            let mut case = Case::open();
            close(&mut case);
            let judged = case.judge(&db, &scope);
            assert_eq!(judged.refusal.as_ref(), Some(expected), "{} alone", RULES[k]);
            assert_eq!(rule(expected), RULES[k]);
            assert!(!judged.feasible && !judged.old && judged.fits_agree && judged.reads_agree, "{} alone", RULES[k]);
            for (later, _) in &closers[k + 1..] {
                let mut case = Case::open();
                later(&mut case);
                close(&mut case);
                let judged = case.judge(&db, &scope);
                assert_eq!(judged.refusal.as_ref().map(rule), Some(RULES[k]), "{case:?}");
                assert!(!judged.feasible && !judged.old && judged.reads_agree, "{case:?}");
            }
        }
    }

    /// The machine's rule refuses only what it would start neither now nor by waiting: an action
    /// on its own cooldown waits, and so does a running action that ends; an input the moveset
    /// lacks never starts, nor does anything while a held action loops with no end.
    #[test]
    fn the_machine_refuses_only_what_waiting_does_not_start() {
        let (db, scope) = machine_db();
        let mut case = Case::open();
        case.cooldowns = Some(shards_action::Cooldowns(vec![shards_action::Cooldown { action: "act_a".into(), left: 1.0 }]));
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, None, "an action cooldown only delays it");
        assert!(judged.feasible && judged.old);
        let mut case = Case::open();
        case.o.input = "z".into();
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, Some(Refusal::Machine { reason: "no action for `z`".into() }));
        assert!(!judged.feasible && !judged.old);
        // Busy in a loop held with no end: it gives no time, so it refuses; let go, it ends and waits.
        let mut case = Case::open();
        case.state = running("act_h", 0.2);
        case.held = vec!["a".into()];
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, Some(Refusal::Machine { reason: "busy (loop act_h 0.30s)".into() }));
        assert!(!judged.feasible && !judged.old);
        case.held.clear();
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, None, "a running action that ends only delays it");
        assert!(judged.feasible && judged.old);
    }

    /// The machine reads a definition no other rule reads: the action a running action's cancel
    /// window routes the input to. A closed option still asks it where the old rules did (no ban
    /// refuses it), so a session reads, and a replay's `content` check records, what it did before.
    #[test]
    fn a_closed_option_reads_what_the_old_rules_read() {
        let (db, scope) = machine_db();
        let mut case = Case::open();
        case.o.input = "b".into();
        case.state = running("act_a", 0.2);
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, None, "act_a's window lets b through into act_c");
        assert!(judged.reads.contains(&"action/act_c".to_owned()), "{:?}", judged.reads);
        case.o.gated = true;
        case.open = Some(NpcOptions::default());
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, Some(Refusal::Closed { when: String::new() }));
        assert!(judged.reads.contains(&"action/act_c".to_owned()), "closed, it still asks the machine: {:?}", judged.reads);
        assert!(judged.reads_agree);
        // A ban kept the old rules from asking the machine: closed and banned, it reads nothing.
        case.plan_def.bans = vec![SequenceBan { after: vec!["a".into()], then: vec!["a".into()], within: 1 }];
        case.plan.history = vec![PastAttack { name: "a".into(), tags: vec!["a".into()] }];
        let judged = case.judge(&db, &scope);
        assert_eq!(judged.refusal, Some(Refusal::Closed { when: String::new() }));
        assert_eq!(judged.reads, Vec::<String>::new());
        assert!(judged.reads_agree);
    }

    /// The property: over many random options and states (a fixed seed), `refusal` passes exactly
    /// the options the old conjunction passed and reads the definitions it read, `feasible` is
    /// `refusal(..).is_none()`, the press check `fits` is the old one, `ban_of` finds a ban exactly
    /// where the old ban rule does, `Closed` quotes the option's own `when`, and every rule refuses
    /// somewhere.
    #[test]
    fn refusal_passes_exactly_what_the_old_rules_passed() {
        let (db, scope) = machine_db();
        let mut rng = DetRng::new(0x5eed_08a1);
        let names = ["a", "b", "z"];
        let tag_pool = ["light", "heavy", "zone", "move", "a", "b"];
        let tags = |rng: &mut DetRng| -> Vec<String> { tag_pool.iter().filter(|_| rng.chance(0.35)).map(|t| (*t).to_owned()).collect() };
        let conds = ["enraged", "!calm", "phase_2"];
        let mut refused = [0u32; 8];
        let mut passed = 0u32;
        for n in 0..20_000 {
            let name = *rng.pick(&names).unwrap();
            let mut o = option(name, *rng.pick(&[0.0, -1.0, 0.5, 1.0, 3.0]).unwrap(), 0.0, 5.0, &[]);
            o.tags.extend(tags(&mut rng));
            o.tags.sort();
            o.tags.dedup();
            o.cost = *rng.pick(&[0.5, 1.0, 1.5, 3.0]).unwrap();
            o.gated = rng.chance(0.3);
            if rng.chance(0.2) {
                o.input = (*rng.pick(&names).unwrap()).to_owned();
            }
            // Every name's option in the plan, each with conditions of its own (at least one).
            let options: Vec<crate::def::AttackOption> = names
                .iter()
                .map(|label| {
                    let mut when: Vec<&str> = conds.iter().copied().filter(|_| rng.chance(0.5)).collect();
                    if when.is_empty() {
                        when.push(*rng.pick(&conds).unwrap());
                    }
                    let when = when.into_iter().map(|c| crate::def::Cond::new(c, default())).collect();
                    crate::def::AttackOption { input: Some((*label).to_owned()), when, ..default() }
                })
                .collect();
            let mut plan = NpcPlan::default();
            for name in names {
                if rng.chance(0.3) {
                    plan.cooldowns.push(PlanCooldown { name: name.into(), left: *rng.pick(&[-0.01, 0.0, 0.3]).unwrap() });
                }
            }
            for _ in 0..rng.below(4) {
                let name = *rng.pick(&["a", "b", "z", "slam"]).unwrap();
                let mut tags = tags(&mut rng);
                tags.push(name.to_owned());
                plan.history.push(PastAttack { name: name.into(), tags });
            }
            let bans = (0..rng.below(3))
                .map(|_| SequenceBan { after: tags(&mut rng), then: tags(&mut rng), within: 1 + rng.below(3) })
                .collect();
            let mut director = DirectorDef::default();
            if rng.chance(0.5) {
                let pairs = [("light", "light"), ("light", "heavy"), ("*", "zone")];
                director.allowed = pairs.iter().filter(|_| rng.chance(0.5)).map(|(a, b)| ((*a).to_owned(), (*b).to_owned())).collect();
            }
            if rng.chance(0.3) {
                director.spacing_exempt_tags = vec!["move".into()];
            }
            // Free, or in `act_a` (whose window routes `b` into `act_c`), or in `act_h` (`a` held or not).
            let (state, held) = match rng.below(4) {
                0 => (running("act_a", 0.2), Vec::new()),
                1 => (running("act_h", 0.2), if rng.chance(0.5) { vec!["a".to_owned()] } else { Vec::new() }),
                _ => (default(), Vec::new()),
            };
            let case = Case {
                o,
                plan_def: AttackPlanDef { bans, options, ..default() },
                plan,
                state,
                held,
                dead: rng.chance(0.1),
                cooldowns: rng
                    .chance(0.3)
                    .then(|| shards_action::Cooldowns(vec![shards_action::Cooldown { action: "act_a".into(), left: 1.0 }])),
                budget: rng.chance(0.5).then(|| *rng.pick(&[0.0, 0.5, 1.0, 1.00005, 2.5]).unwrap()),
                overlapping: (0..rng.below(3)).map(|k| (AiId(20 + k), tags(&mut rng))).collect(),
                director,
                spacing: rng.chance(0.4).then(|| (AiId(9), 1 + u64::from(rng.below(40)))),
                open: rng.chance(0.5).then(|| NpcOptions { allowed: names.iter().filter(|_| rng.chance(0.5)).map(|n| (*n).to_owned()).collect() }),
            };
            let judged = case.judge(&db, &scope);
            assert_eq!(judged.refusal.is_none(), judged.old, "case {n}: {case:?} gives {:?}", judged.refusal);
            assert_eq!(judged.feasible, judged.refusal.is_none(), "case {n}");
            assert!(judged.fits_agree, "case {n}: {case:?}");
            assert!(judged.reads_agree, "case {n}: {case:?} reads {:?}", judged.reads);
            assert_eq!(
                ban_of(&case.plan_def.bans, &case.o.tags, &case.plan.history).is_some(),
                banned_old(&case.plan_def.bans, &case.o.tags, &case.plan.history),
                "case {n}: {case:?}"
            );
            if let Some(Refusal::Closed { when }) = &judged.refusal {
                let own = case.plan_def.options.iter().find(|p| p.input.as_deref() == Some(case.o.name.as_str())).unwrap();
                let written: Vec<String> = own.when.iter().map(ToString::to_string).collect();
                assert!(!when.is_empty() && *when == written.join(", "), "case {n}: {when:?} for {:?}", own.when);
            }
            match &judged.refusal {
                Some(r) => refused[RULES.iter().position(|x| *x == rule(r)).unwrap()] += 1,
                None => passed += 1,
            }
        }
        assert!(passed > 100, "{passed} of 20000 passed");
        for (k, count) in refused.iter().enumerate() {
            assert!(*count > 50, "the rule {} refused only {count} of 20000: {refused:?}", RULES[k]);
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
