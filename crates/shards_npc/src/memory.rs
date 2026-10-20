//! What an NPC's plan remembers, under the game's control (N8c). [`reset_plan`] forgets the history
//! and the cooldowns: a boss that is home again after a leash or a wipe starts its plan afresh.
//! [`set_plan_cooldown`] holds or clears one option's cooldown: a phase that really resets an option,
//! or a hold that no gate can express.
//!
//! Both act on the world at once, on the host only (the host owns NPC plans; a client sees `NpcPlan`
//! as the host sends it). A game observer queues them, `commands.queue(move |w: &mut World| ..)`, and
//! they run when the observer's commands are applied, before the planner next reads the plan. The dev
//! tools `npc_plan_reset` and `npc_plan_cooldown` call them too.

use ai_core::label;
use bevy::prelude::*;
use shards_action::Fighter;
use shards_content::db::closest;
use shards_content::ContentDb;

use crate::brain::Npc;
use crate::def::AttackPlanDef;
use crate::planner::{resolve, NpcPlan, PlanCooldown};

/// What [`reset_plan`] and [`set_plan_cooldown`] answer on a machine that does not simulate.
pub const HOST_OWNS_PLANS: &str = "the host owns NPC plans: call this on the host";

/// An NPC's plan forgot what it remembered (journaled: `e6 plan reset (encounter wiped): 3 attacks
/// forgotten, 2 cooldowns cleared`). Only when there was something to forget.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct PlanReset {
    pub entity: Entity,
    /// The caller's reason, as it passed it.
    pub why: String,
    /// Attacks taken out of the history.
    pub forgot: u32,
    /// Cooldowns cleared.
    pub cleared: u32,
}

/// One option's plan cooldown changed (journaled: `e6 heavy cooldown 0.00 s (was 4.20 s)`). Only when
/// the value changed.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct PlanCooldownSet {
    pub entity: Entity,
    pub option: String,
    /// Seconds left now; 0: off cooldown.
    pub left: f32,
    /// Seconds left before.
    pub was: f32,
}

/// How [`set_plan_cooldown`] treats the cooldown an option has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CooldownSet {
    /// At least `seconds`: the larger of the two stays.
    AtLeast,
    /// Exactly `seconds`; 0 takes the option off cooldown.
    Exactly,
}

/// Forgets what NPC `entity`'s plan remembers: its history and its cooldowns. The attack count, the
/// rest, the grant and any chain stay as they are. Triggers [`PlanReset`] when there was something to
/// forget; a plan with nothing to forget answers so, and a second call on one reset is harmless. `why`
/// goes into the journal line. The answer: `e6 plan reset: 3 attacks forgotten, 2 cooldowns cleared`.
pub fn reset_plan(world: &mut World, entity: Entity, why: &str) -> Result<String, String> {
    plan_of(world, entity)?;
    let who = label(world, entity);
    let mut plan = world.get_mut::<NpcPlan>(entity).ok_or_else(|| format!("{who} has no NpcPlan"))?;
    let forgot = plan.history.len() as u32;
    let cleared = plan.cooldowns.len() as u32;
    if forgot == 0 && cleared == 0 {
        return Ok(format!("{who} plan reset: nothing to forget"));
    }
    plan.history.clear();
    plan.cooldowns.clear();
    world.trigger(PlanReset { entity, why: why.to_owned(), forgot, cleared });
    Ok(format!("{who} plan reset: {}", forgotten(forgot, cleared)))
}

/// `3 attacks forgotten, 2 cooldowns cleared`, as the answer and the journal line say it.
pub(crate) fn forgotten(forgot: u32, cleared: u32) -> String {
    let s = |n: u32| if n == 1 { "" } else { "s" };
    format!("{forgot} attack{} forgotten, {cleared} cooldown{} cleared", s(forgot), s(cleared))
}

/// Sets the plan cooldown of option `option` of NPC `entity`'s current plan (by its name, as
/// `npc_schedule` lists it; `auto` options included) to `seconds`, as `how` says. Triggers
/// [`PlanCooldownSet`] when the value changes, so a hold the game renews every tick journals every tick:
/// renew it when it changes, or set it once for as long as it holds. The answer: `e6 heavy: cooldown
/// 0.00 s (was 4.20 s)`.
pub fn set_plan_cooldown(world: &mut World, entity: Entity, option: &str, seconds: f32, how: CooldownSet) -> Result<String, String> {
    if !(seconds >= 0.0 && seconds.is_finite()) {
        return Err(format!("seconds must be >= 0 and finite (is {seconds})"));
    }
    let plan_id = plan_of(world, entity)?;
    let who = label(world, entity);
    let names = option_names(world, entity, &plan_id).map_err(|e| format!("{who}'s {e}"))?;
    if !names.iter().any(|n| n == option) {
        let hint = closest(option, names.iter().map(String::as_str)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("{who}'s plan `{plan_id}` has no option `{option}`{hint}; options: {}", names.join(", ")));
    }
    let mut plan = world.get_mut::<NpcPlan>(entity).ok_or_else(|| format!("{who} has no NpcPlan"))?;
    let was = plan.cooldown(option);
    let left = match how {
        CooldownSet::AtLeast => was.max(seconds),
        CooldownSet::Exactly => seconds,
    };
    if left != was {
        // Kept sorted by name, as `NpcPlan::remember` keeps it; 0 takes the entry out.
        plan.cooldowns.retain(|c| c.name != option);
        if left > 0.0 {
            let at = plan.cooldowns.partition_point(|c| c.name.as_str() < option);
            plan.cooldowns.insert(at, PlanCooldown { name: option.to_owned(), left });
        }
        world.trigger(PlanCooldownSet { entity, option: option.to_owned(), left, was });
    }
    Ok(format!("{who} {option}: cooldown {left:.2} s (was {was:.2} s)"))
}

/// The plan id of NPC `entity`, on a machine that simulates.
fn plan_of(world: &World, entity: Entity) -> Result<String, String> {
    if !crate::nav::simulates(world) {
        return Err(HOST_OWNS_PLANS.to_owned());
    }
    let who = label(world, entity);
    let npc = world.get::<Npc>(entity).ok_or_else(|| format!("{who} is not an NPC (it has no Npc component)"))?;
    npc.plan
        .clone()
        .ok_or_else(|| format!("{who} has no attack plan; give it one (act set id={who} component=Npc path=plan value=\"...\")"))
}

/// The option names of plan `plan_id` for NPC `entity`: those it lists, in its order, then the `auto`
/// options its fighter's moveset adds.
fn option_names(world: &World, entity: Entity, plan_id: &str) -> Result<Vec<String>, String> {
    let db = world.resource::<ContentDb>();
    let plan = db.get::<AttackPlanDef>(plan_id).ok_or_else(|| format!("plan `{plan_id}` is not in content"))?;
    let mut names: Vec<String> = plan.options.iter().map(|o| o.label().to_owned()).collect();
    if plan.auto
        && let Some(fighter) = world.get::<Fighter>(entity)
    {
        for r in resolve(plan, db, &fighter.moveset, 0.0) {
            if !names.contains(&r.name) {
                names.push(r.name);
            }
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counts_read_as_words() {
        assert_eq!(forgotten(3, 2), "3 attacks forgotten, 2 cooldowns cleared");
        assert_eq!(forgotten(1, 0), "1 attack forgotten, 0 cooldowns cleared");
        assert_eq!(forgotten(0, 1), "0 attacks forgotten, 1 cooldown cleared");
    }
}
