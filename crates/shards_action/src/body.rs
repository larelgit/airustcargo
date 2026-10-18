//! Body state (B2b): what sits on a fighter between hits. A [`Barrier`] takes damage before
//! health, [`DamageTaken`] holds named multipliers on the damage it takes, and [`ProcGates`]
//! holds the statuses whose next proc is gated (`StatusDef::min_proc_interval`).
//!
//! All three are lazy: a fighter has one only while it matters, and [`tick_body_state`] removes
//! each when it is empty. Time is stored the way [`Protection`](crate::Protection) and
//! [`Cooldowns`](crate::Cooldowns) store it: `f32` seconds left, read as `left > EPS`, run
//! down by the tick length after the tick's checks (`ActionSystems::Fighters`, after
//! `tick_protection`), so a timer of `s` seconds set during tick `N` acts on the checks of ticks
//! `N` to `N + ceil(60 s) - 1`. Nothing here is predicted; a co-op client shows what the host
//! replicates.

use ai_core::AiId;
use bevy::prelude::*;

use crate::def::EPS;
use crate::fighter::{Dead, Health};

/// A pool that takes damage before health. Lazy: removed when empty or out of time.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Barrier {
    /// Damage it can still take.
    pub amount: f32,
    /// Seconds left.
    pub left: f32,
}

/// Named multipliers on the damage a fighter takes, by key (sorted, unique); `left: None`
/// lasts until cleared. Their product is [`DamageCtx::taken`](crate::DamageCtx::taken).
/// Lazy: removed when the last one goes.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct DamageTaken(pub Vec<TakenMult>);

/// One named multiplier of a [`DamageTaken`].
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct TakenMult {
    pub key: String,
    pub mult: f32,
    /// Seconds left; `None`: until cleared.
    pub left: Option<f32>,
    /// For a window: the permanent multiplier the key returns to when it ends (see
    /// [`extend_damage_taken`]); `None`: the key goes.
    pub then: Option<f32>,
}

impl DamageTaken {
    /// The multiplier under `key`.
    pub fn get(&self, key: &str) -> Option<&TakenMult> {
        self.0.iter().find(|m| m.key == key)
    }

    /// Seconds left of the multiplier under `key`: `None` if there is none, `Some(None)` if it
    /// lasts until cleared.
    pub fn left(&self, key: &str) -> Option<Option<f32>> {
        self.get(key).map(|m| m.left)
    }

    /// The product of the multipliers in force (1 when there are none).
    pub fn product(&self) -> f32 {
        self.0.iter().filter(|m| m.left.is_none_or(|l| l > EPS)).fold(1.0, |p, m| p * m.mult)
    }

    fn put_then(&mut self, key: &str, then: f32) {
        if let Some(m) = self.0.iter_mut().find(|m| m.key == key) {
            m.then = Some(then);
        }
    }

    fn put(&mut self, key: &str, mult: f32, left: Option<f32>) {
        match self.0.binary_search_by(|m| m.key.as_str().cmp(key)) {
            Ok(at) => {
                self.0[at].mult = mult;
                self.0[at].left = left;
                self.0[at].then = None;
            }
            Err(at) => self.0.insert(at, TakenMult { key: key.to_owned(), mult, left, then: None }),
        }
    }
}

/// Statuses whose next proc is gated ([`StatusDef::min_proc_interval`](crate::StatusDef)), with
/// the seconds left. Sorted by status id and unique, so equal states hash equal. Lazy.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct ProcGates(pub Vec<(String, f32)>);

impl ProcGates {
    /// Whether the next proc of `status` is swallowed right now.
    pub fn gated(&self, status: &str) -> bool {
        self.0.iter().any(|(id, left)| id == status && *left > EPS)
    }

    pub(crate) fn set(&mut self, status: &str, secs: f32) {
        match self.0.binary_search_by(|(id, _)| id.as_str().cmp(status)) {
            Ok(at) => self.0[at].1 = secs,
            Err(at) => self.0.insert(at, (status.to_owned(), secs)),
        }
    }
}

/// A barrier was granted to `entity` ([`grant_barrier`]). A net event, for the HUD.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct BarrierGranted {
    pub entity: Entity,
    /// The pool now (the larger of what it held and what was granted).
    pub amount: f32,
    /// Seconds it lasts from now.
    pub seconds: f32,
}

/// A timed [`DamageTaken`] multiplier ran out on a tick. A game that keeps a base under the same
/// key puts it back from an observer of this.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct DamageTakenExpired {
    pub entity: Entity,
    pub key: String,
}

/// Gives `entity` a barrier of `amount` for `seconds`: the pool is the larger of what it holds
/// and `amount`, and the time is `seconds` from now. Ignored for a dead or gone entity, one
/// without [`Health`] (a barrier only guards health), an `amount` or `seconds` that is not above 0
/// or not finite. Triggers [`BarrierGranted`].
pub fn grant_barrier(world: &mut World, entity: Entity, amount: f32, seconds: f32) {
    if world.get_entity(entity).is_err()
        || world.get::<Dead>(entity).is_some()
        || world.get::<Health>(entity).is_none()
        || !(amount > EPS && amount.is_finite() && seconds > EPS && seconds.is_finite())
    {
        return;
    }
    let pool = match world.get_mut::<Barrier>(entity) {
        Some(mut b) => {
            b.amount = b.amount.max(amount);
            b.left = seconds;
            b.amount
        }
        None => {
            world.entity_mut(entity).insert(Barrier { amount, left: seconds });
            amount
        }
    };
    world.trigger(BarrierGranted { entity, amount: pool, seconds });
}

/// Removes the barrier of `entity`, if it has one.
pub fn clear_barrier(world: &mut World, entity: Entity) {
    if world.get_entity(entity).is_ok() {
        world.entity_mut(entity).remove::<Barrier>();
    }
}

/// Sets the multiplier `key` on `entity` to `mult` for `seconds` (`None`: until cleared),
/// replacing whatever the key held, a longer window included. For a window that must not cut a
/// longer one short, use [`extend_damage_taken`]. Ignored for a gone entity, a `mult` below 0 or
/// not finite, and `seconds` of 0 or less (a window of no time; [`clear_damage_taken`] cancels).
pub fn set_damage_taken(world: &mut World, entity: Entity, key: &str, mult: f32, seconds: Option<f32>) {
    if !valid_taken(world, entity, mult, seconds) {
        return;
    }
    match world.get_mut::<DamageTaken>(entity) {
        Some(mut t) => t.put(key, mult, seconds),
        None => {
            let mut t = DamageTaken::default();
            t.put(key, mult, seconds);
            world.entity_mut(entity).insert(t);
        }
    }
}

/// [`set_damage_taken`] that never shortens what the key already holds: the end is the later of
/// the two (a window that lasts until cleared is the latest), and a timed window is never
/// cancelled by a permanent set. The multiplier is the new one, so a game keeps one key (`core`)
/// for its base and its windows without a timer of its own, nor a copy of its base: the engine
/// holds the base under the window ([`TakenMult::then`]). Returns whether the multiplier in
/// force changed (`false` also for an invalid call).
///
/// - A timed window over a held one: `left = max(held, new)`, `mult` = the new, the base stays.
/// - A timed window over a permanent multiplier: the window takes the key and the permanent
///   multiplier becomes its `then`; when the window ends the key returns to it
///   ([`DamageTakenExpired`] still fires, with the base already back).
/// - A permanent set over a timed window: the window runs on, and the new value becomes the
///   `then` (a base that changes mid-window is not lost). Returns `false`.
/// - A permanent set over a permanent multiplier or an empty key: the plain set.
pub fn extend_damage_taken(world: &mut World, entity: Entity, key: &str, mult: f32, seconds: Option<f32>) -> bool {
    if !valid_taken(world, entity, mult, seconds) {
        return false;
    }
    let held = world.get::<DamageTaken>(entity).and_then(|t| t.get(key)).map(|m| (m.mult, m.left, m.then));
    let (left, then) = match (held, seconds) {
        (Some((_, Some(held), _)), None) if held > EPS => {
            if let Some(mut t) = world.get_mut::<DamageTaken>(entity) {
                t.put_then(key, mult);
            }
            return false;
        }
        (Some((_, Some(held), then)), Some(new)) if held > EPS => (Some(held.max(new)), then),
        (Some((base, None, _)), Some(_)) => (seconds, Some(base)),
        _ => (seconds, None),
    };
    set_damage_taken(world, entity, key, mult, left);
    if then.is_some()
        && let Some(mut t) = world.get_mut::<DamageTaken>(entity)
    {
        t.put_then(key, then.unwrap_or(1.0));
    }
    true
}

/// Removes the multiplier `key` from `entity` (no event; the component goes with its last one).
pub fn clear_damage_taken(world: &mut World, entity: Entity, key: &str) {
    let Some(mut t) = world.get_mut::<DamageTaken>(entity) else { return };
    if t.get(key).is_none() {
        return;
    }
    t.0.retain(|m| m.key != key);
    if t.0.is_empty() {
        world.entity_mut(entity).remove::<DamageTaken>();
    }
}

fn valid_taken(world: &World, entity: Entity, mult: f32, seconds: Option<f32>) -> bool {
    world.get_entity(entity).is_ok()
        && mult >= 0.0
        && mult.is_finite()
        && seconds.is_none_or(|s| s > EPS && s.is_finite())
}

/// `left` after a tick of `dt`; a NaN stays one, for the invariant to name.
fn run_down(left: f32, dt: f32) -> f32 {
    if left.is_nan() { left } else { (left - dt).max(0.0) }
}

/// `ActionSystems::Fighters`, after `tick_protection`: barriers, timed multipliers and proc gates
/// run down by the tick, each component goes when it is empty, and a multiplier that ran out
/// triggers [`DamageTakenExpired`]. In `AiId` order (the events come out in it, then in key order).
pub fn tick_body_state(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let mut order: Vec<(u32, Entity)> = world
        .query_filtered::<(Entity, Option<&AiId>), Or<(With<Barrier>, With<DamageTaken>, With<ProcGates>)>>()
        .iter(world)
        .map(|(e, id)| (id.map_or(u32::MAX, |i| i.0), e))
        .collect();
    order.sort();
    let mut expired = Vec::new();
    for (_, entity) in order {
        let mut gone = false;
        if let Some(mut b) = world.get_mut::<Barrier>(entity) {
            let left = run_down(b.left, dt);
            gone = b.amount <= EPS || left <= EPS;
            if !gone && b.left != left {
                b.left = left;
            }
        }
        if gone {
            world.entity_mut(entity).remove::<Barrier>();
        }
        let mut emptied = false;
        if let Some(mut t) = world.get_mut::<DamageTaken>(entity) {
            let mut ran_out = Vec::new();
            for m in &mut t.0 {
                if let Some(left) = m.left {
                    let next = run_down(left, dt);
                    if next <= EPS {
                        ran_out.push(m.key.clone());
                        if let Some(base) = m.then.take() {
                            // The window ends and the base it covered returns.
                            m.mult = base;
                            m.left = None;
                            continue;
                        }
                        m.left = Some(0.0);
                    } else {
                        m.left = Some(next);
                    }
                }
            }
            if !ran_out.is_empty() {
                t.0.retain(|m| m.left != Some(0.0));
                emptied = t.0.is_empty();
                expired.extend(ran_out.into_iter().map(|key| DamageTakenExpired { entity, key }));
            }
        }
        if emptied {
            world.entity_mut(entity).remove::<DamageTaken>();
        }
        let mut emptied = false;
        if let Some(mut g) = world.get_mut::<ProcGates>(entity) {
            for (_, left) in &mut g.0 {
                *left = run_down(*left, dt);
            }
            g.0.retain(|(_, left)| *left > EPS || left.is_nan());
            emptied = g.0.is_empty();
        }
        if emptied {
            world.entity_mut(entity).remove::<ProcGates>();
        }
    }
    for event in expired {
        world.trigger(event);
    }
}

/// `barrier 15 (6.2s)`.
pub fn describe_barrier(b: &Barrier) -> String {
    format!("barrier {:.0} ({:.1}s)", b.amount, b.left)
}

/// `takes x1.35 (core 3.1s)`, `takes x1.20 (core)` for one until cleared; several are joined with `, x`.
pub fn describe_damage_taken(t: &DamageTaken) -> String {
    let parts: Vec<String> = t
        .0
        .iter()
        .map(|m| match m.left {
            Some(left) => match m.then {
                Some(then) => format!("x{:.2} ({} {left:.1}s, then x{then:.2})", m.mult, m.key),
                None => format!("x{:.2} ({} {left:.1}s)", m.mult, m.key),
            },
            None => format!("x{:.2} ({})", m.mult, m.key),
        })
        .collect();
    format!("takes {}", parts.join(", "))
}

/// `proc gate chill 1.2s`.
pub fn describe_proc_gates(g: &ProcGates) -> String {
    let parts: Vec<String> = g.0.iter().map(|(id, left)| format!("{id} {left:.1}s")).collect();
    format!("proc gate {}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_and_put_keep_keys_sorted_and_unique() {
        let mut t = DamageTaken::default();
        t.put("core", 1.35, Some(2.0));
        t.put("armor", 0.5, None);
        t.put("core", 1.5, Some(1.0));
        assert_eq!(t.0.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["armor", "core"]);
        assert_eq!(t.product(), 0.75);
        assert_eq!(describe_damage_taken(&t), "takes x0.50 (armor), x1.50 (core 1.0s)");
    }

    #[test]
    fn gates_are_sorted_and_read_as_left_above_eps() {
        let mut g = ProcGates::default();
        g.set("chill", 1.0);
        g.set("burn", 0.0);
        assert_eq!(g.0.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), ["burn", "chill"]);
        assert!(g.gated("chill") && !g.gated("burn") && !g.gated("shock"));
    }
}
