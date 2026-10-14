//! What the AI sees of combat: describe fragments, journal lines, and advice for the
//! subject (`ALERT in hit zone of e6 heavy_slam (lands in 0.32s): move 2.1m S to leave
//! it`).

use ai_core::spatial::compass;
use ai_core::{label, AiAppExt};
use bevy::prelude::*;

use crate::combat::{Blocked, Damaged, Died, Evaded, GuardBroken, Parried, Staggered};
use crate::def::EPS;
use crate::fighter::{
    describe_charges, describe_cooldowns, describe_health, describe_poise, describe_stamina, Charges, Cooldowns, Dead,
    Health, Invulnerable, Poise, Stamina,
};
use crate::machine::{describe_action, describe_buffer, ActionRejected, ActionState, Healed, InputBuffer, Telegraph};
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
        .register_ai_advice(advice)
        .journal_event::<Telegraph>(|e, w| {
            let zone = e.zone.map(|z| format!(" {}", z.shape)).unwrap_or_default();
            Some(format!("{} winds up {} ({:.2}s) {}{zone}", label(w, e.attacker), e.action, e.lands_in, e.symbol))
        })
        .journal_event::<Damaged>(|e, w| {
            let (a, t) = (label(w, e.attacker), label(w, e.target));
            Some(match e.invulnerable {
                true => format!("{a} -> {t} {} -0 (hp {:.0}, god mode)", e.action, e.hp_left),
                false => format!("{a} -> {t} {} -{:.0} (hp {:.0})", e.action, e.amount, e.hp_left),
            })
        })
        .journal_event::<Blocked>(|e, w| {
            Some(format!(
                "{} blocked {} {}: -{:.0} hp, -{:.0} stamina (hp {:.0})",
                label(w, e.defender),
                label(w, e.attacker),
                e.action,
                e.amount,
                e.stamina,
                e.hp_left
            ))
        })
        .journal_event::<Parried>(|e, w| {
            Some(format!("{} parried {} {}", label(w, e.defender), label(w, e.attacker), e.action))
        })
        .journal_event::<Evaded>(|e, w| {
            Some(format!("{} evaded {} {}", label(w, e.defender), label(w, e.attacker), e.action))
        })
        .journal_event::<GuardBroken>(|e, w| {
            Some(format!(
                "{} broke {}'s guard with {} -{:.0} (hp {:.0})",
                label(w, e.attacker),
                label(w, e.defender),
                e.action,
                e.amount,
                e.hp_left
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
