//! What the AI sees of combat: describe fragments, journal lines, and advice for the
//! subject (`ALERT in hit zone of e6 heavy_slam (lands in 0.32s): move 2.1m S to leave
//! it`).

use ai_core::spatial::compass;
use ai_core::{label, AiAppExt};
use bevy::prelude::*;

use crate::body::{describe_barrier, describe_damage_taken, describe_proc_gates, Barrier, BarrierGranted, DamageTaken, ProcGates};
use crate::combat::{Blocked, Damaged, Died, Evaded, GuardBroken, IgnoreReason, Parried, PerfectEvaded, Reacted, Staggered};
use crate::def::EPS;
use crate::fighter::{
    describe_charges, describe_cooldowns, describe_health, describe_poise, describe_stamina, Charges, Cooldowns, Dead,
    Health, Invulnerable, Knockback, Poise, Protection, Stamina,
};
use crate::machine::{describe_action, describe_buffer, ActionMissed, ActionRejected, ActionState, Healed, InputBuffer, RunExtras, Telegraph};
use crate::status::{describe_statuses, StatusApplied, StatusExpired, Statuses};
use crate::zone::{body_radius, zones_threatening};

pub(crate) fn register(app: &mut App) {
    app.register_ai_component::<Health>(describe_health)
        .register_ai_component::<Stamina>(describe_stamina)
        .register_ai_component::<Poise>(describe_poise)
        .register_ai_component::<Dead>(|_| "dead".into())
        .register_ai_component::<ActionState>(describe_action)
        .register_ai_component::<Statuses>(describe_statuses)
        .register_ai_component::<InputBuffer>(describe_buffer)
        .register_ai_component::<Charges>(describe_charges)
        .register_ai_component::<Cooldowns>(describe_cooldowns)
        .register_ai_component::<Invulnerable>(|_| "god mode".into())
        .register_ai_component::<Protection>(describe_protection)
        .register_ai_component::<Barrier>(describe_barrier)
        .register_ai_component::<DamageTaken>(describe_damage_taken)
        .register_ai_component::<ProcGates>(describe_proc_gates)
        .register_ai_component::<Knockback>(describe_knockback)
        .register_ai_describer("run", describe_run)
        .add_ai_condition("charging", charging)
        .register_ai_advice(advice)
        .journal_event::<Telegraph>(|e, w| {
            let zone = e.zone.map(|z| format!(" {}", z.shape)).unwrap_or_default();
            Some(format!("{} winds up {} ({:.2}s) {}{zone}", label(w, e.attacker), e.action, e.lands_in, e.symbol))
        })
        .journal_event::<Damaged>(|e, w| {
            let (a, t) = (label(w, e.attacker), label(w, e.target));
            Some(match e.invulnerable {
                true => format!("{a} -> {t} {} -0 (hp {:.0}, {})", e.action, e.hp_left, e.ignore_reason.map_or("ignored", IgnoreReason::describe)),
                false => format!("{a} -> {t} {} -{:.0} (hp {:.0}){}", e.action, e.amount, e.hp_left, absorbed(e.absorbed)),
            })
        })
        .journal_event::<Blocked>(|e, w| {
            Some(format!(
                "{} blocked {} {}: -{:.0} hp, -{:.0} stamina (hp {:.0}){}",
                label(w, e.defender),
                label(w, e.attacker),
                e.action,
                e.amount,
                e.stamina,
                e.hp_left,
                absorbed(e.absorbed)
            ))
        })
        .journal_event::<Parried>(|e, w| {
            let cost = if e.stamina > 0.0 { format!(", -{:.0} stamina", e.stamina) } else { String::new() };
            Some(format!("{} parried {} {}{cost}", label(w, e.defender), label(w, e.attacker), e.action))
        })
        .journal_event::<PerfectEvaded>(|e, w| {
            Some(format!("{} perfectly evaded {} {}", label(w, e.defender), label(w, e.attacker), e.action))
        })
        .journal_event::<Reacted>(|e, w| {
            let tier = match e.tier {
                1 => "light",
                2 => "heavy",
                _ => "knockdown",
            };
            let by = e.attacker.map(|a| format!(" ({} {})", label(w, a), e.action)).unwrap_or_default();
            Some(format!("{} reacts {tier}{by}", label(w, e.entity)))
        })
        .journal_event::<Evaded>(|e, w| {
            Some(format!("{} evaded {} {}", label(w, e.defender), label(w, e.attacker), e.action))
        })
        .journal_event::<GuardBroken>(|e, w| {
            Some(format!(
                "{} broke {}'s guard with {} -{:.0} (hp {:.0}){}",
                label(w, e.attacker),
                label(w, e.defender),
                e.action,
                e.amount,
                e.hp_left,
                absorbed(e.absorbed)
            ))
        })
        .journal_event::<Staggered>(|e, w| Some(format!("{} staggered ({})", label(w, e.entity), e.cause.describe())))
        .journal_event::<Died>(|e, w| {
            Some(match (e.killer, &e.action) {
                (Some(k), Some(action)) => {
                    format!("{} died (killed by {} with {action})", label(w, e.entity), label(w, k))
                }
                (Some(k), None) => format!("{} died (killed by {})", label(w, e.entity), label(w, k)),
                _ => format!("{} died", label(w, e.entity)),
            })
        })
        .journal_event::<ActionMissed>(|e, w| {
            Some(format!("{} {} missed: +{:.2}s recovery", label(w, e.entity), e.action, e.extra))
        })
        .journal_event::<ActionRejected>(|e, w| {
            Some(format!("{} {} rejected: {}", label(w, e.entity), e.input, e.reason))
        })
        .journal_event::<StatusApplied>(|e, w| {
            Some(match e.stacks {
                n if n > 1 => format!("{} {} ×{n}", label(w, e.entity), e.status),
                _ => format!("{} {} applied", label(w, e.entity), e.status),
            })
        })
        .journal_event::<StatusExpired>(|e, w| Some(format!("{} {} expired", label(w, e.entity), e.status)))
        .journal_event::<BarrierGranted>(|e, w| {
            Some(format!("{} barrier {:.0} for {:.1}s", label(w, e.entity), e.amount, e.seconds))
        })
        .journal_event::<Healed>(|e, w| {
            Some(format!("{} healed +{:.0} (hp {:.0})", label(w, e.entity), e.amount, e.hp))
        });
}

/// The hostile hit zones the subject stands in, soonest first, one per attacker and
/// action, with the shortest way out along a compass direction, rounded up so that
/// doing exactly what it says leaves the zone.
pub fn advice(world: &World, me: Entity) -> Vec<String> {
    let Some(here) = world.get::<Transform>(me).map(|t| t.translation) else { return Vec::new() };
    let radius = body_radius(world, me);
    let mut seen: Vec<(Entity, String)> = Vec::new();
    let mut lines = Vec::new();
    for (zone, out) in zones_threatening(world, me, here, radius) {
        if seen.contains(&(zone.attacker, zone.action.clone())) {
            continue;
        }
        seen.push((zone.attacker, zone.action.clone()));
        let when = if zone.lands_in > EPS {
            format!("lands in {:.2}s", zone.lands_in)
        } else {
            format!("active, {:.2}s left", zone.active_for)
        };
        // The exact way out is rarely along a compass direction; say one that works.
        let (dir, meters) = match zone.shape.exit_by_compass(here, radius) {
            Some((dir, meters)) => (dir, ((meters + 0.02) * 10.0).ceil() / 10.0),
            None => (compass(out), out.length()),
        };
        lines.push(format!(
            "in hit zone of {} {} ({when}): move {meters:.1}m {dir} to leave it",
            label(world, zone.attacker),
            zone.action,
        ));
    }
    lines
}

/// `protected 0.82s` (hits of tiers 1-2 cause no reaction), `poise protected 2.40s`.
fn describe_protection(p: &Protection) -> String {
    let mut parts = Vec::new();
    if p.reactions > EPS {
        parts.push(format!("protected {:.2}s", p.reactions));
    }
    if p.poise > EPS {
        parts.push(format!("poise protected {:.2}s", p.poise));
    }
    parts.join("  ")
}

/// `pushed 3.2m/s`.
fn describe_knockback(k: &Knockback) -> String {
    format!("pushed {:.1}m/s", k.velocity.length())
}

/// `charging, turns in 0.12s` while the input that started a charging action is held (`turns
/// when the stamina is there` once it is due and waits for it), `missed +0.60s` after a whiff
/// lengthened the run.
fn describe_run(e: EntityRef) -> Option<String> {
    let x = e.get::<RunExtras>()?;
    let run = e.get::<ActionState>()?.current.as_ref().filter(|r| r.instance == x.instance)?;
    let mut parts = Vec::new();
    if let Some(at) = x.charge_at
        && x.held
    {
        parts.push(if run.t < at - EPS {
            format!("charging, turns in {:.2}s", at - run.t)
        } else {
            "charging, turns when the stamina is there".to_owned()
        });
    }
    if x.checked && !x.landed && x.extra > EPS {
        parts.push(format!("missed +{:.2}s", x.extra));
    }
    (!parts.is_empty()).then(|| parts.join("  "))
}

/// Some fighter holds the input of an action that may still turn into its charge.
fn charging(world: &World) -> bool {
    world.try_query::<&RunExtras>().is_some_and(|mut q| q.iter(world).any(|x| x.held && x.charge_at.is_some()))
}

/// ` [barrier -20]` when a barrier took `amount` of a hit, else nothing.
fn absorbed(amount: f32) -> String {
    if amount > 0.0 { format!(" [barrier -{amount:.0}]") } else { String::new() }
}
