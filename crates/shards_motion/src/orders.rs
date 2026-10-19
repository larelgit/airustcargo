//! A `move` or `move_to` order that makes no progress ends ("blocked").
//!
//! `follow_orders` steers towards the front [`Order::MoveTo`] until the body is within reach of
//! its goal, which a wall, a ledge or a rooted body never lets happen: the order would run for
//! ever (a sprint order drained the stamina to 0 against a wall). [`end_stalled_orders`] watches the
//! front order of every body and drops it when the body has not come closer to the goal for
//! [`STALL_SECONDS`].
//!
//! - **Progress** is a flat distance to the goal that is at least [`STALL_PROGRESS`] metres below
//!   the best one so far. For an order to an *entity* a body that moves at least that far while its
//!   target moves at least that far too also counts: a chase of a runner that keeps away as fast as
//!   the body is slow is not blocked; a body that circles a target that stands still is.
//! - **The count waits** while the body is not free to walk: an action holds it (`locked`: an attack,
//!   a dodge, a stagger) or something pushes it (`external`). A guard, a slow and a corner are all
//!   free: they are walking, and 0.1 m in 2 s is a twentieth of a metre per second.
//! - **The end**: the order leaves the queue, the steering is zeroed, [`OrderEnded`] is triggered
//!   (the journal says `e1 move_to (3.0, -4.0) ended: blocked (no closer than 0.10 m in 2.0 s)`)
//!   and the body's [`OrderProgress`] keeps the reason (`ai/observe` shows it as `last order ended:
//!   blocked`) until the next order starts. The orders behind it run on.
//!
//! The check runs in the simulation (`SimSet::Input`, after `follow_orders`), never in a client's
//! prediction: a client's orders are the host's to end.

use ai_core::{label, AiAppExt, AiId, AiIndex, SimSet};
use bevy::prelude::*;
use shards_core::{flat, CoreSystems, Intent, Order, Target};

use crate::controller::LocomotionInput;
use crate::intent::MotionSystems;

/// How long (s) a `move` or `move_to` order may go without progress before it ends. Long enough for
/// a walk around a corner, a step up a ledge, a body slowed to a crawl; the clock waits while an
/// action holds the body.
pub const STALL_SECONDS: f32 = 2.0;

/// How much closer (m, flat) to its goal a body must have come to count as progress.
pub const STALL_PROGRESS: f32 = 0.1;

/// Why an order ended without arriving.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderEnd {
    /// The body has not come closer to its goal for [`STALL_SECONDS`].
    Blocked,
}

impl OrderEnd {
    /// The word the journal and `observe` use.
    pub fn name(self) -> &'static str {
        match self {
            OrderEnd::Blocked => "blocked",
        }
    }
}

/// A `move` or `move_to` order ended without arriving ([`end_stalled_orders`]). Arriving is no event.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct OrderEnded {
    pub entity: Entity,
    pub order: Order,
    pub reason: OrderEnd,
}

/// On a body with a `move_to` order at the front of its queue: what [`end_stalled_orders`] watches,
/// and, once an order ended without arriving, why. Lazy: a body that never held such an order has none.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct OrderProgress {
    /// The order being watched.
    pub order: Option<Order>,
    /// The flat distance to the goal at the last progress.
    pub best: f32,
    /// Where the body stood at the last progress.
    pub from: Vec3,
    /// Where the goal stood at the last progress (what tells a chase of a runner from a circling).
    pub goal_at: Vec3,
    /// Ticks without progress.
    pub quiet: u32,
    /// How the last order ended, if it did not arrive: `move_to (3.0, -4.0): blocked`.
    pub ended: Option<String>,
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<OrderProgress>()
        // Bookkeeping of the order, not a fact about the world: a replay that differs here differs
        // in the queue (`Intent.orders`), which is checked.
        .replay_ignore::<OrderProgress>()
        .register_ai_describer("order_end", describe_progress)
        .journal_event::<OrderEnded>(|e, w| {
            Some(format!(
                "{} {} ended: {} (no closer than {STALL_PROGRESS:.2} m in {STALL_SECONDS:.1} s)",
                label(w, e.entity),
                e.order.describe(),
                e.reason.name()
            ))
        })
        .configure_sets(FixedUpdate, MotionSystems::StallOrders.in_set(SimSet::Input).after(CoreSystems::FollowOrders))
        .add_systems(FixedUpdate, end_stalled_orders.in_set(MotionSystems::StallOrders));
}

fn describe_progress(entity: EntityRef) -> Option<String> {
    let why = entity.get::<OrderProgress>()?.ended.as_ref()?;
    Some(format!("last order ended: {why}"))
}

/// What [`end_stalled_orders`] reads and writes of a body.
type Walkers = (Entity, &'static mut Intent, &'static Transform, Option<&'static LocomotionInput>, Option<&'static mut OrderProgress>, Option<&'static AiId>);

/// `SimSet::Input`, after `follow_orders`: ends the front `move_to` order of a body that has made
/// no progress for [`STALL_SECONDS`] (see the module).
pub fn end_stalled_orders(
    mut commands: Commands,
    time: Res<Time<Fixed>>,
    index: Res<AiIndex>,
    positions: Query<&Transform>,
    mut bodies: Query<Walkers>,
) {
    let dt = time.timestep().as_secs_f32().max(1e-4);
    let limit = bevy::math::ops::round(STALL_SECONDS / dt).max(1.0) as u32;
    let mut ended_orders: Vec<(u32, OrderEnded)> = Vec::new();
    for (entity, mut intent, tf, input, progress, id) in &mut bodies {
        let front = intent.orders.front().cloned();
        let Some(order @ Order::MoveTo { target, .. }) = front else {
            if let Some(mut p) = progress
                && p.order.is_some()
            {
                p.order = None;
                p.quiet = 0;
            }
            continue;
        };
        let here = tf.translation;
        let goal = match target {
            Target::Point(p) => Some(p),
            Target::Entity(id) => index.get(id).and_then(|e| positions.get(e).ok()).map(|t| t.translation),
        };
        // `follow_orders` drops an order whose goal is gone.
        let Some(goal) = goal else { continue };
        let dist = flat(goal - here).length();
        let Some(mut p) = progress else {
            commands.entity(entity).insert(OrderProgress { order: Some(order), best: dist, from: here, goal_at: goal, quiet: 0, ended: None });
            continue;
        };
        if p.order.as_ref() != Some(&order) {
            *p = OrderProgress { order: Some(order), best: dist, from: here, goal_at: goal, quiet: 0, ended: None };
            continue;
        }
        // An action holds the body, or something pushes it: it is not walking, so it is not stuck.
        if input.is_some_and(|i| i.locked || i.external != Vec3::ZERO) {
            continue;
        }
        let closer = dist <= p.best - STALL_PROGRESS;
        // A chase of a target that runs: the body keeps moving and so does the goal. A body that
        // circles a target that stands still, or slides along a wall, is not chasing.
        let chasing = matches!(target, Target::Entity(_))
            && flat(here - p.from).length() >= STALL_PROGRESS
            && flat(goal - p.goal_at).length() >= STALL_PROGRESS;
        if closer || chasing {
            // A chase starts the distance over: a runner that pulled away and then stopped is
            // closed on from where it stopped, not from the nearest the body ever came.
            p.best = if chasing { dist } else { p.best.min(dist) };
            p.from = here;
            p.goal_at = goal;
            p.quiet = 0;
            continue;
        }
        p.quiet += 1;
        if p.quiet < limit {
            continue;
        }
        let ended = intent.orders.pop_front().unwrap_or(order);
        intent.move_dir = Vec3::ZERO;
        p.ended = Some(format!("{}: {}", ended.describe(), OrderEnd::Blocked.name()));
        p.order = None;
        p.quiet = 0;
        ended_orders.push((id.map_or(u32::MAX, |i| i.0), OrderEnded { entity, order: ended, reason: OrderEnd::Blocked }));
    }
    // By `AiId`, never by query order: the journal reads the same with a window and without one.
    ended_orders.sort_by_key(|(id, _)| *id);
    for (_, event) in ended_orders {
        commands.trigger(event);
    }
}
