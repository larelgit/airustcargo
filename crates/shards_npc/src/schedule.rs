//! Why an attack option may not be chosen now: the rules of the attack planner, judged in one
//! place.
//!
//! One function, `planner::refusal`, judges an option of an NPC's plan against one set of inputs
//! (`Inputs`), built by one builder (`inputs`) from the NPC's read-only view, its target, the
//! strike notes and the attacks in flight its caller collected. The director's grant (`direct`,
//! step 3: some option must pass, distance aside) and the planner (`plan_attacks`: the options it
//! may choose or keep) both ask it, so the two never judge an option differently; whether an
//! option may be chosen is `refusal(..).is_none()`. The rules, in the order they are named (the
//! first that refuses is the answer, [`Refusal`]):
//!
//! 1. `Weight`: weight 0, a chain link that is never chosen;
//! 2. `Cooldown`: on the plan's cooldown;
//! 3. `Angle`: the target is outside the option's `angle` window off the NPC's facing;
//! 4. `Closed`: a `when` condition does not hold;
//! 5. `Players`: the living players are outside the option's `players` window;
//! 6. `Repeats`: it was chosen `max_repeats` times in a row already;
//! 7. `ForbidAfter`: the newest past attack carries one of its `forbid_after` tags;
//! 8. `Banned`: a plan ban (`SequenceBan`) after the last attacks;
//! 9. `Machine`: the action machine would start it neither now nor by waiting;
//! 10. `Budget`: it does not fit in what is left of the target's collision budget;
//! 11. `Overlap`: it may not overlap with an attack in flight on the target;
//! 12. `Spacing`: another NPC of its director began a strike less than `min_spacing` ago.
//!
//! Every rule is a pure read of the world as the system sees it, so the order changes nothing
//! but the answer's name. The action machine is asked where the rules before this module asked
//! it (after the weight, the cooldown and the bans, a closed option too), because its rule reads
//! definitions (a cancel window's action, a context binding's) that the replay's `content` check
//! records: what a session reads is what it read before. The distance window is not a rule here:
//! the planner prefers the options whose window holds and, with `in_range_only`, chooses only
//! among them.
//!
//! The players the director counts are `Players` and `living_players`.

use ai_core::{AiId, AiIndex, DetRng, SimSeed, SimTick};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use serde::Serialize;
use shards_action::{Dead, FighterView};
use shards_content::ContentDb;
use shards_core::{flat, PlayerId};
use shards_motion::PhysicsWorld;

use crate::brain::{salt, Npc};
use crate::def::{AttackPlanDef, DirectorDef};
use crate::director::{npc_site, npc_view, target_of, tokenless, FightingReadOnly, FightingReadOnlyItem, Grant, Targets};
use crate::planner::{choice_pool, in_window, pick_weighted, refusal, resolve, InFlight, NpcOptions, NpcPlan, Resolved};
use crate::strike::{chain_note, note_list, pool, spacing_ticks, strike_pressed, waiting, Note};

/// Why an attack option may not be chosen now, distance aside: the first rule that refuses it,
/// in the order of this list. The boss scheduler's next slices add rules in their places in the
/// order (the angle, the players, repeats, `forbid_after`, live tags), each a new variant:
/// a game's `match` needs a `_` arm, so a new variant does not break it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Refusal {
    /// Its weight is 0: a chain link, never chosen.
    Weight,
    /// On the plan's cooldown: seconds left.
    Cooldown { left: f32 },
    /// The target's angle off the NPC's facing (degrees, 0 ahead, 180 behind) is outside the
    /// option's `angle` window.
    Angle { angle: f32, min: f32, max: f32 },
    /// A `when` condition does not hold (the NPC's `NpcOptions` does not list it open): the
    /// option's conditions as written.
    Closed { when: String },
    /// The living players (`players`) are outside the option's `players` window.
    Players { players: u32, min: u32, max: u32 },
    /// It was chosen `times` times in a row (the newest history entries) and `max_repeats` is `max`.
    Repeats { times: u32, max: u32 },
    /// The newest history entry, `after` (its name), carries `tag`, one of the option's `forbid_after`.
    ForbidAfter { tag: String, after: String },
    /// A plan ban: `bans[ban]` forbids it after `after`, the newest past attack (its name) the
    /// ban looks back to.
    Banned { after: String, ban: usize },
    /// The action machine would start it neither now nor by waiting (`Blocker::reason`).
    Machine { reason: String },
    /// It costs `cost`, more than the `left` of the target's collision budget.
    Budget { left: f32, cost: f32 },
    /// It may not overlap (the director's `allowed`) with an attack in flight on the target: the
    /// NPC `by` that holds that attack, and the attack's tags.
    Overlap { by: AiId, with: Vec<String> },
    /// NPC `by` of its director began a strike less than `min_spacing` ago: `ticks` left to wait.
    Spacing { by: AiId, ticks: u64 },
}

/// What the callers of [`inputs`] hold for every NPC they judge.
pub(crate) struct FightCx<'a> {
    pub db: &'a ContentDb,
    pub physics: &'a PhysicsWorld,
    pub tick: u64,
    pub dt: f32,
    /// The living players ([`living_players`]).
    pub players: u32,
}

/// What the rules read of the moment besides the NPC's plan: where the target stands off the
/// NPC's facing, and how many players live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Moment {
    /// Degrees between the NPC's facing and the target, 0..=180 ([`target_angle`]).
    pub angle: f32,
    /// Living players ([`living_players`]).
    pub players: u32,
}

/// Degrees between `facing` and the direction from `from` to `to`, on the ground plane, 0..=180
/// (0 straight ahead, 180 straight behind); 0 when either is shorter than 1 cm, as an NPC too
/// close to tell turns toward nothing.
pub fn target_angle(facing: Vec3, from: Vec3, to: Vec3) -> f32 {
    let (f, d) = (flat(facing), flat(to - from));
    if f.length_squared() < 1e-4 || d.length_squared() < 1e-4 {
        return 0.0;
    }
    let (f, d) = (f.normalize(), d.normalize());
    let a = ops::atan2(f.x * d.z - f.z * d.x, f.x * d.x + f.z * d.z);
    a.abs() * (180.0 / std::f32::consts::PI)
}

/// Everything [`refusal`](crate::planner::refusal) reads of one NPC and its target besides the
/// option.
pub(crate) struct Inputs<'a> {
    pub plan_def: &'a AttackPlanDef,
    pub plan: &'a NpcPlan,
    /// What the action machine reads of the fighter (`can_start`), a leap's landing toward the
    /// target included.
    pub view: FighterView<'a>,
    pub dt: f32,
    /// What is left of the target's collision budget for this NPC; `None`: no budget.
    pub budget: Option<f32>,
    /// The other NPCs' attacks in flight on the target: the holder and the attack's tags.
    pub overlapping: Vec<(AiId, Vec<String>)>,
    pub director: DirectorDef,
    /// Another NPC of its director began a strike less than `min_spacing` ago: who, and the ticks
    /// left. `None`: it may strike, or its plan `ignores_spacing`.
    pub spacing: Option<(AiId, u64)>,
    /// The options its `when` conditions open (only NPCs whose plan has gated options carry it).
    pub open: Option<&'a NpcOptions>,
    /// The target's angle and the living players.
    pub moment: Moment,
}

/// The one builder of [`Inputs`], from the read-only view of a fighting NPC, its target as the
/// NPC perceives it (`(id, where, body radius)`), the strike notes and the attacks in flight its
/// caller collected. `None`: the NPC has no attack plan in content.
pub(crate) fn inputs<'a>(
    f: &'a FightingReadOnlyItem<'_, '_>,
    cx: &FightCx<'a>,
    target: (AiId, Vec3, f32),
    notes: &[Note],
    inflight: &InFlight,
) -> Option<Inputs<'a>> {
    let plan_def = f.npc.plan.as_deref().and_then(|p| cx.db.get::<AttackPlanDef>(p))?;
    let director = DirectorDef::of(cx.db, &f.npc.director);
    let (id, there, radius) = target;
    let site = npc_site(cx.physics, cx.db, f.entity, f.transform, f.intent, f.body, Some((there, radius)));
    let view = npc_view(cx.db, f.fighter, f.state, &f.drive.held, f.dead, (f.stamina, f.cooldowns, f.charges), f.motion, site);
    let (budget, overlapping) = inflight.on(id, *f.id, &director);
    // Another NPC of its director began a strike less than `min_spacing` ago: its strikes wait.
    let spacing = if plan_def.ignores_spacing {
        None
    } else {
        waiting(notes, *f.id, &pool(&f.npc.director), cx.tick, spacing_ticks(director.min_spacing, cx.dt))
    };
    let moment = Moment { angle: target_angle(*f.transform.forward(), f.transform.translation, there), players: cx.players };
    Some(Inputs { plan_def, plan: f.plan, view, dt: cx.dt, budget, overlapping, director, spacing, open: f.options, moment })
}

/// The strike notes the planner judges by: every NPC's noted strike and pending press
/// ([`note_list`]), then the presses already in an NPC's drive this tick that start a strike now
/// (a state hook's, a combo continuation's), as starts of this tick.
pub(crate) fn plan_notes<'w, 's>(
    db: &ContentDb,
    npcs: impl IntoIterator<Item = FightingReadOnlyItem<'w, 's>>,
    tick: u64,
    dt: f32,
) -> Vec<Note> {
    let npcs: Vec<_> = npcs.into_iter().collect();
    let mut notes = note_list(
        npcs.iter().map(|f| (*f.id, f.npc.director.as_str(), f.strike, f.token.held.as_ref(), chain_note(f.chain, Some(f.buffer), tick))),
    );
    // Presses already made this tick (a hook's, a continuation's) are starts of this tick.
    notes.extend(npcs.iter().filter(|f| strike_pressed(db, f, dt)).map(|f| (*f.id, pool(&f.npc.director), tick)));
    notes
}

/// The players the director counts: alive.
pub(crate) type Players<'w, 's> = Query<'w, 's, (), (With<PlayerId>, Without<Dead>)>;

/// How many players the director counts ([`Players`]).
pub(crate) fn living_players(players: &Players) -> u32 {
    players.iter().count() as u32
}

/// The strike notes the director judges a grant by: every NPC's noted strike and pending press
/// ([`note_list`]).
pub(crate) fn grant_notes<'w, 's>(npcs: impl IntoIterator<Item = FightingReadOnlyItem<'w, 's>>, tick: u64) -> Vec<Note> {
    note_list(npcs.into_iter().map(|f| (*f.id, f.npc.director.as_str(), f.strike, f.token.held.as_ref(), chain_note(f.chain, Some(f.buffer), tick))))
}

/// What `npc_schedule` reads: the same things the director and the planner read, all read-only.
#[derive(SystemParam)]
pub struct ScheduleParams<'w, 's> {
    npcs: Query<'w, 's, FightingReadOnly>,
    names: Query<'w, 's, (&'static Npc, &'static AiId)>,
    targets: Targets<'w, 's>,
    db: Res<'w, ContentDb>,
    index: Res<'w, AiIndex>,
    physics: Res<'w, PhysicsWorld>,
    seed: Res<'w, SimSeed>,
    tick: Res<'w, SimTick>,
    fixed: Res<'w, Time<Fixed>>,
    players: Players<'w, 's>,
}

impl ScheduleParams<'_, '_> {
    /// The entity `id` names.
    pub fn resolve(&self, id: AiId) -> Result<Entity, String> {
        self.index.resolve(id)
    }

    /// Whether `entity` is an NPC.
    pub fn is_npc(&self, entity: Entity) -> bool {
        self.names.contains(entity)
    }
}

/// What the caller of `npc_schedule` asks "what if" about; each `None` is measured.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WhatIf {
    /// Judge against this target.
    pub target: Option<AiId>,
    /// Meters, instead of the measured distance.
    pub distance: Option<f32>,
    /// Degrees 0..=180, instead of the measured angle.
    pub angle: Option<f32>,
    /// Living players, instead of the counted ones.
    pub players: Option<u32>,
}

impl WhatIf {
    fn any(&self) -> bool {
        self.target.is_some() || self.distance.is_some() || self.angle.is_some() || self.players.is_some()
    }
}

/// One option of a [`ScheduleReport`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OptionReport {
    pub name: String,
    pub weight: f32,
    /// The distance window `[min, max]`, meters.
    pub window: [f32; 2],
    /// It may be chosen now: no rule refuses it and, with `in_range_only`, the window holds.
    pub eligible: bool,
    /// Why not: the first rule that refuses it, in the order of [`Refusal`] with the distance
    /// window between the cooldown and the angle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// Without `in_range_only`, an option outside its window is eligible and walks into it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub walks: Option<String>,
    /// Its share of the draw among the eligible options (those whose window holds, if any does).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chance: Option<f32>,
}

/// The planner's next draw (a Dev caller only).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PickReport {
    pub option: String,
    /// The roll in `[0, 1)` and the total weight it was drawn against; absent for a kept choice
    /// (no draw is made).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub of: Option<f32>,
    pub on_tick: u64,
    /// The pick assumes nothing the next tick does before the planner (senses, states, a game's
    /// systems, presses of NPCs earlier in `AiId` order) changes the inputs.
    pub if_unchanged: bool,
}

/// Which options NPC `id` may choose now, why not the others, and each eligible option's chance.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ScheduleReport {
    pub id: String,
    pub plan: String,
    pub target: String,
    pub distance: f32,
    pub angle: f32,
    pub players: u32,
    /// Its plan is `tokenless`: it holds no counted token (a boss among its adds).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tokenless: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub what_if: bool,
    pub history: Vec<String>,
    pub rest: f32,
    /// What the NPC is doing about a choice.
    pub chooses: String,
    pub options: Vec<OptionReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pick: Option<PickReport>,
}

fn round2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// The text of a refusal, quoting the numbers that refused the option. `spacing` is the director's
/// spacing in ticks.
fn refusal_text(r: &Refusal, spacing: u64) -> String {
    match r {
        Refusal::Weight => "weight: 0, a chain link that is never chosen".to_owned(),
        Refusal::Cooldown { left } => format!("cooldown: {left:.2} s left"),
        Refusal::Angle { angle, min, max } => {
            format!("angle: the target is {angle:.0} degrees off the facing; this option needs {min:.0}-{max:.0}")
        }
        Refusal::Closed { when } => format!("closed: when {when} does not hold"),
        Refusal::Players { players, min, max } => format!("players: {players} living, this option needs {min}-{max}"),
        Refusal::Repeats { times, max } => format!("repeats: chosen {times} times in a row, max_repeats {max}"),
        Refusal::ForbidAfter { tag, after } => format!("forbid_after: {tag}, after {after}"),
        Refusal::Banned { after, ban } => format!("banned: after {after} (bans[{ban}])"),
        Refusal::Machine { reason } => format!("machine: {reason}"),
        Refusal::Budget { left, cost } => format!("budget: {left:.1} left of the target's, costs {cost:.1}"),
        Refusal::Overlap { by, with } => format!("overlap: with {by}'s [{}]", with.join(", ")),
        Refusal::Spacing { by, ticks } => {
            format!("spacing: {by} began a strike {} ticks ago ({ticks} to go)", spacing.saturating_sub(*ticks))
        }
    }
}

/// The body of `npc_schedule`: which options NPC `entity` may choose now, as the director's grant
/// and the planner judge it (the same [`inputs`] and [`refusal`]). The report is of the next tick:
/// the plan's timers run down by one tick first, as `direct` runs them before the planner chooses.
/// A `dev` caller also gets `pick`, the planner's next draw, when the next tick is a decision tick
/// of an NPC that holds a grant and has not pressed.
pub fn schedule_report(p: &ScheduleParams, entity: Entity, what_if: &WhatIf, dev: bool) -> Result<ScheduleReport, String> {
    let Ok((_, &id)) = p.names.get(entity) else {
        return Err("that is not an NPC (it has no Npc component)".to_owned());
    };
    let Ok(f) = p.npcs.get(entity) else {
        return Err(format!("{id} is an NPC without a fighter body (it lacks a Fighter, an action state or a perception)"));
    };
    let Some(plan_name) = f.npc.plan.as_deref() else {
        return Err(format!("{id} has no attack plan"));
    };
    let Some(plan_def) = p.db.get::<AttackPlanDef>(plan_name) else {
        return Err(format!("{id}'s attack plan `{plan_name}` is not in content"));
    };
    let holds = f.token.held.as_ref();
    let Some(target_id) = what_if.target.or(holds.map(|g| g.target)).or(f.brain.target) else {
        return Err(format!("{id} has no target: pass target=e5 to judge against one"));
    };
    // Where the NPC perceives the target; a what-if target it does not know is where it stands.
    // An NPC does not cheat: its own target that it does not perceive is not judged (the planner skips it).
    let known = if what_if.target.is_none() { target_of(&p.index, &p.targets, f.perceived, target_id) } else { None };
    let (there, radius) = match known {
        Some(found) => found,
        None if what_if.target.is_none() => {
            return Err(format!("{id} does not perceive {target_id}: pass target={target_id} to judge as a what-if"));
        }
        None => {
            let seen = p.index.get(target_id).and_then(|e| p.targets.get(e).ok());
            match seen {
                Some((at, false, collider)) => (at.translation, collider.map_or(0.4, |c| c.shape.half_extents().x)),
                _ => return Err(format!("{target_id} is gone or dead: pass target=<an entity that stands> to judge against one")),
            }
        }
    };
    let here = f.transform.translation;
    let measured = flat(there - here).length();
    let distance = what_if.distance.unwrap_or(measured);
    let dt = p.fixed.timestep().as_secs_f32();
    let tick = p.tick.0 + 1;
    let counted = living_players(&p.players);
    let players = what_if.players.unwrap_or(counted);
    let cx = FightCx { db: &p.db, physics: &p.physics, tick, dt, players: counted };
    let inflight = InFlight::collect(p.npcs.iter().map(|f| (*f.id, f.token.held.as_ref())));
    // As the caller that would judge the NPC next builds them: the planner's notes for one that
    // holds a grant, the director's for one that asks for it.
    let notes = if holds.is_some() { plan_notes(&p.db, p.npcs.iter(), tick, dt) } else { grant_notes(p.npcs.iter(), tick) };
    let mut next = f.plan.clone();
    next.tick(dt, f.state.current.is_none());
    let Some(mut inputs) = inputs(&f, &cx, (target_id, there, radius), &notes, &inflight) else {
        return Err(format!("{id}'s attack plan `{plan_name}` is not in content"));
    };
    inputs.plan = &next;
    inputs.moment = Moment { angle: what_if.angle.unwrap_or(inputs.moment.angle), players };
    let angle = inputs.moment.angle;
    let spacing = spacing_ticks(inputs.director.min_spacing, dt);
    let options = resolve(plan_def, &p.db, &f.fighter.moveset, radius);
    let ok = |o: &Resolved| refusal(o, &inputs).is_none();
    let draw = choice_pool(&options, ok, distance, plan_def.in_range_only);
    let total: f32 = draw.iter().map(|(_, w)| w).sum();
    let reports: Vec<OptionReport> = options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let why = refusal(o, &inputs);
            let outside = !in_window(o, distance);
            // The distance window sits between the cooldown and the angle, the order the planner judges them in.
            let early = matches!(why, Some(Refusal::Weight | Refusal::Cooldown { .. }));
            let refused = match &why {
                Some(r) if early => Some(refusal_text(r, spacing)),
                _ if plan_def.in_range_only && outside => {
                    Some(format!("range: {distance:.1} m is outside {:.1}-{:.1} m (in_range_only)", o.min, o.max))
                }
                Some(r) => Some(refusal_text(r, spacing)),
                None => None,
            };
            let walks = (refused.is_none() && outside).then(|| format!("walks into {:.1}-{:.1} m", o.min, o.max));
            let chance = draw.iter().find(|(k, _)| *k == i).map(|(_, w)| round2(w / total));
            OptionReport { name: o.name.clone(), weight: o.weight, window: [o.min, o.max], eligible: refused.is_none(), refused, walks, chance }
        })
        .collect();
    // Is the next tick a decision tick of this NPC (the think timer counts only while it runs no action).
    let decides = !plan_def.press_on_decision || (f.state.current.is_none() && f.think.is_none_or(|t| t.left - dt <= 1e-4));
    let chooses = match holds {
        Some(g) if g.pressed.is_some() && !g.started => format!("holds a choice: {} (pressed)", g.attack.as_deref().unwrap_or("?")),
        Some(g) if g.started => format!("running {}", g.attack.as_deref().unwrap_or("?")),
        Some(_) if decides => "the next tick decides".to_owned(),
        Some(g) => match &g.attack {
            Some(name) => format!("between decisions: keeps {name}"),
            None if plan_def.wait.is_some() => "between decisions: waits in its role".to_owned(),
            None => "between decisions: no choice yet".to_owned(),
        },
        None if f.plan.rest > 1e-4 => format!("resting {:.2} s", f.plan.rest),
        None if plan_def.tokenless => "no grant yet: tokenless, it takes one when it asks and an option may be chosen".to_owned(),
        None => {
            let d = pool(&f.npc.director);
            // The tokens in use are the counted ones: a tokenless grant is outside the director's books.
            let held = p
                .npcs
                .iter()
                .filter(|o| o.token.held.as_ref().is_some_and(|g| pool(&g.director) == d) && !tokenless(&p.db, o.npc))
                .count() as u32;
            let limit = inputs.director.limit(counted);
            if held >= limit {
                format!("no grant: tokens {held}/{limit} in use")
            } else {
                format!("no grant yet: tokens {held}/{limit} in use")
            }
        }
    };
    // The planner's draw on the next tick: only for a grant holder that has not pressed, on a
    // decision tick. The draw is a pure function of (seed, tick, id, salt), so it is reproduced
    // without changing any state.
    let pick = if dev && decides { holds.filter(|g| !g.in_flight()).and_then(|g| pick_of(&options, plan_def, &ok, g, &draw, total, (p.seed.0, tick, id))) } else { None };
    Ok(ScheduleReport {
        id: id.to_string(),
        plan: plan_name.to_owned(),
        target: target_id.to_string(),
        distance: round2(distance),
        angle: round2(angle),
        players,
        tokenless: plan_def.tokenless,
        what_if: what_if.any(),
        history: f.plan.history.iter().map(|h| h.name.clone()).collect(),
        rest: round2(f.plan.rest),
        chooses,
        options: reports,
        pick,
    })
}

/// What `plan_attacks` would do on a decision tick `tick` with a grant that has chosen nothing
/// pressed: keep a choice that still passes (a plan that is not `in_range_only`), else draw.
fn pick_of(
    options: &[Resolved],
    plan_def: &AttackPlanDef,
    ok: &dyn Fn(&Resolved) -> bool,
    grant: &Grant,
    pool: &[(usize, f32)],
    total: f32,
    (seed, tick, id): (u64, u64, AiId),
) -> Option<PickReport> {
    let kept = grant.attack.as_ref().and_then(|name| options.iter().position(|o| o.name == *name));
    if let Some(k) = kept.filter(|&k| !plan_def.in_range_only && ok(&options[k])) {
        return Some(PickReport { option: options[k].name.clone(), roll: None, of: None, on_tick: tick, if_unchanged: true });
    }
    let mut rng = DetRng::from_keys(&[seed, tick, id.0 as u64, salt::PICK]);
    let roll = rng.clone().f32();
    let chosen = pick_weighted(&mut rng, pool)?;
    Some(PickReport { option: options[chosen].name.clone(), roll: Some(roll), of: Some(total), on_tick: tick, if_unchanged: true })
}

#[cfg(test)]
mod tests {
    use ai_core::AiIndex;
    use bevy::ecs::system::RunSystemOnce;
    use shards_action::{ActionDef, Fighter, MovesetDef};

    use super::*;
    use crate::brain::Npc;
    use crate::director::{FightingReadOnly, Grant};
    use crate::strike::NpcStrike;

    const DT: f32 = 1.0 / 60.0;
    const TICK: u64 = 100;
    const TARGET: AiId = AiId(999);

    /// Director `arena` spaces strikes 0.7 s apart (42 ticks at 60 Hz) and gives a target a
    /// budget of 2; plan `p` attacks with `a` (`act_a`), plan `solo` too but ignores the spacing.
    fn world() -> World {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<DirectorDef>();
        kinds.register::<AttackPlanDef>();
        let source = |path: &str, text: &str| shards_content::db::Source { path: path.into(), text: text.into() };
        let db = ContentDb::build(
            &kinds,
            &[
                source("content/t.action.ron", r#"{ "act_a": (net_id: 1, phases: [("swing", 0.5)]) }"#),
                source("content/t.moveset.ron", r#"{ "m": (inputs: {"a": "act_a"}) }"#),
                source("content/t.director.ron", r#"{ "arena": (min_spacing: 0.7, budget: Some(2.0)) }"#),
                source(
                    "content/t.attack_plan.ron",
                    r#"{ "p": (options: [(input: Some("a"))]), "solo": (options: [(input: Some("a"))], ignores_spacing: true) }"#,
                ),
            ],
        );
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        w.insert_resource(db);
        w.init_resource::<PhysicsWorld>();
        w
    }

    fn npc(w: &mut World, plan: &str) -> (Entity, AiId) {
        let e = w.spawn((Npc::default().with_director("arena").with_plan(plan), Fighter::new("m"), Transform::default())).id();
        (e, *w.get::<AiId>(e).unwrap())
    }

    /// What `inputs` built for each NPC, in `AiId` order, from the planner's notes
    /// (`plan_notes`) and the attacks in flight: `None` without a plan in content, else the
    /// budget left, the attacks in flight on the target and the spacing.
    type Built = Vec<(AiId, Option<(Option<f32>, Vec<(AiId, Vec<String>)>, Option<(AiId, u64)>)>)>;

    fn built(w: &mut World) -> Built {
        w.run_system_once(|db: Res<ContentDb>, physics: Res<PhysicsWorld>, npcs: Query<FightingReadOnly>| -> Built {
            let cx = FightCx { db: &db, physics: &physics, tick: TICK, dt: DT, players: 1 };
            let inflight = InFlight::collect(npcs.iter().map(|f| (*f.id, f.token.held.as_ref())));
            let notes = plan_notes(&db, npcs.iter(), TICK, DT);
            let mut out: Built = npcs
                .iter()
                .map(|f| {
                    let built = inputs(&f, &cx, (TARGET, Vec3::ZERO, 0.4), &notes, &inflight);
                    (*f.id, built.map(|i| (i.budget, i.overlapping.clone(), i.spacing)))
                })
                .collect();
            out.sort_by_key(|(id, _)| *id);
            out
        })
        .unwrap()
    }

    /// Degrees off the facing: 0 ahead, 90 beside on either hand, 180 behind, 0 on top of it.
    #[test]
    fn target_angle_is_degrees_off_the_facing_on_the_ground() {
        let (here, facing) = (Vec3::new(1.0, 5.0, 1.0), Vec3::NEG_Z);
        let at = |dx: f32, dz: f32| target_angle(facing, here, here + Vec3::new(dx, 3.0, dz));
        assert!(at(0.0, -2.0).abs() < 1e-3, "ahead");
        assert!((at(2.0, 0.0) - 90.0).abs() < 1e-3 && (at(-2.0, 0.0) - 90.0).abs() < 1e-3, "beside, either hand");
        assert!((at(0.0, 2.0) - 180.0).abs() < 1e-3, "behind");
        assert!((at(1.0, -1.0) - 45.0).abs() < 1e-3 && (at(-1.0, -1.0) - 45.0).abs() < 1e-3, "symmetric");
        assert!((at(1.0, 1.0) - 135.0).abs() < 1e-3);
        assert_eq!(at(0.0, 0.0), 0.0, "on top of it");
        assert_eq!(at(0.0, -0.005), 0.0, "closer than a centimetre");
        assert_eq!(target_angle(Vec3::ZERO, here, here + Vec3::X), 0.0, "no facing");
        // The height does not count: a target straight above is on top of it.
        assert_eq!(target_angle(facing, here, here + Vec3::Y * 4.0), 0.0);
        // The facing's height does not count either.
        assert!((target_angle(Vec3::new(0.0, 0.7, -1.0), here, here + Vec3::X) - 90.0).abs() < 1e-3);
    }

    /// The one builder reads the plan, the budget and the overlaps from the attacks in flight on
    /// the target (never the NPC's own), the spacing from the strike notes (a press made this
    /// tick among them, through `plan_notes`), and none for a plan that ignores the spacing.
    #[test]
    fn inputs_reads_the_plan_the_attacks_in_flight_and_the_strike_notes() {
        let mut w = world();
        // `striker` began a heavy strike 10 ticks ago and still swings at the target.
        let (striker, striker_id) = npc(&mut w, "p");
        w.entity_mut(striker).insert(NpcStrike { last_start: TICK - 10, gated: true, pool: "arena".into() });
        let grant = Grant { director: "arena".into(), cost: 1.5, tags: vec!["heavy".into()], pressed: Some(TICK - 12), started: true, ..Grant::for_tests(TARGET) };
        w.get_mut::<crate::director::AttackToken>(striker).unwrap().held = Some(grant);
        let (_, waiter) = npc(&mut w, "p");
        let (_, solo) = npc(&mut w, "solo");
        let (_, gone) = npc(&mut w, "gone");
        let (presser, presser_id) = npc(&mut w, "p");
        let heavy = vec![(striker_id, vec!["heavy".to_owned()])];
        let found = built(&mut w);
        let of = |found: &Built, id: AiId| found.iter().find(|(i, _)| *i == id).unwrap().1.clone();
        assert_eq!(of(&found, striker_id), Some((Some(2.0), Vec::new(), None)), "its own attack is not in its way");
        assert_eq!(of(&found, waiter), Some((Some(0.5), heavy.clone(), Some((striker_id, 32)))), "42 ticks, 10 gone");
        assert_eq!(of(&found, solo), Some((Some(0.5), heavy.clone(), None)), "a plan that ignores the spacing");
        assert_eq!(of(&found, gone), None, "no plan in content");
        // A press already in a drive this tick is a start of this tick for the others.
        w.get_mut::<crate::brain::NpcDrive>(presser).unwrap().press.push("a".into());
        let found = built(&mut w);
        assert_eq!(of(&found, waiter), Some((Some(0.5), heavy.clone(), Some((presser_id, 42)))));
        assert_eq!(of(&found, presser_id).unwrap().2, Some((striker_id, 32)), "its own press does not hold it back");
    }
}
