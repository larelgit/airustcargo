//! The leap's landing search ([`LeapSearch`]): where a body that is about to leap would come down.
//!
//! A pure function of the world's colliders and the body, so the machine, a client's prediction,
//! the agent tools and the NPC planner all ask the same question and get the same answer
//! ([`can_start`](crate::can_start) refuses a leap that has no landing, with
//! [`Blocker::NoLanding`](crate::Blocker::NoLanding)).

use ai_core::AiIndex;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent};
use shards_motion::{CharacterController, CharacterParams, Collider, LayerMask, PhysicsWorld, QueryFilter, Shape};

use crate::def::{ActionDef, LeapSearch, EPS};
use crate::machine::start_facing;

/// What a landing search needs to know about the body that leaps. Built from a system's query or
/// from `&World` ([`LeapSite::of`]).
#[derive(Clone, Copy)]
pub struct LeapSite<'a> {
    pub physics: &'a PhysicsWorld,
    pub entity: Entity,
    /// Where the body stands.
    pub feet: Vec3,
    /// Where the body faces (`Transform::forward`).
    pub forward: Vec3,
    /// The body's capsule: radius and total height (m).
    pub radius: f32,
    pub height: f32,
    /// The fighter's intent: the aim point and the movement input decide where an action points.
    pub intent: &'a Intent,
    /// The lock-on target: where it stands and how wide its body is.
    pub target: Option<(Vec3, f32)>,
}

/// A chosen leap: where it points and how far it goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeapPlan {
    /// The flat direction the run faces.
    pub dir: Vec3,
    /// Metres on the ground plane from the take-off to the landing.
    pub distance: f32,
}

/// Where the leap of `def` (`search`) would go for a press toward `press` (the press's direction,
/// if it had one): the farthest candidate distance, from the aimed one down by `step`, whose
/// landing point
///
/// - has no `WORLD` collider on the wall ray from `probe_height` above the feet to the same height
///   above the landing,
/// - has a floor under it (a ray from `probe_height + max_rise` above the landing down by
///   `probe_height + max_rise + max_drop`) at most `max_rise` above and `max_drop` below the feet,
/// - has room for the body there (its capsule `lift` above the floor overlaps no `WORLD`
///   collider), and
/// - has water no deeper than `max_water_depth`, when the search names one (the top of the
///   highest water volume at the landing above the floor).
///
/// With `toward_target` and a lock-on target more than 5 cm away on the ground plane, the leap
/// points at it and is aimed as far as its edge (`flat distance - both radii`, within `max`); else
/// it points where the action would start and is aimed at `max`. `None`: no candidate from there
/// down to `min` qualifies (or the target is nearer than `min`).
pub fn plan_leap(def: &ActionDef, search: &LeapSearch, site: &LeapSite, press: Option<Vec3>) -> Option<LeapPlan> {
    let here = site.feet;
    let target = site.target.map(|(p, _)| p);
    let start = start_facing(def, press, site.intent, here, site.forward, target);
    let (dir, want) = match site.target {
        Some((there, radius)) if search.toward_target && flat(there - here).length() > 0.05 => {
            let to = flat(there - here);
            let gap = to.length() - radius - site.radius;
            (to.normalize(), gap.clamp(0.0, search.max))
        }
        _ => (start, search.max),
    };
    let filter = QueryFilter::new(LayerMask::WORLD).excluding(site.entity);
    let mut k = 0.0_f32;
    loop {
        let d = (want - k * search.step).max(0.0);
        if d < search.min - EPS {
            return None;
        }
        if lands(search, site, &filter, dir, d) {
            return Some(LeapPlan { dir, distance: d });
        }
        // Nothing shorter than none at all (a `min` of 0 would otherwise stay at 0 for ever).
        if d <= EPS || search.step <= EPS {
            return None;
        }
        k += 1.0;
    }
}

/// Whether the landing `d` metres along `dir` is a place to come down.
fn lands(search: &LeapSearch, site: &LeapSite, filter: &QueryFilter, dir: Vec3, d: f32) -> bool {
    let physics = site.physics;
    let feet = site.feet;
    let point = feet + dir * d;
    // The way there: nothing tall enough to stop a body in the air.
    let from = feet + Vec3::Y * search.probe_height;
    if d > EPS && physics.raycast(from, Dir3::new(dir).unwrap_or(Dir3::X), d, filter).is_some() {
        return false;
    }
    // The floor under the landing.
    let reach = search.probe_height + search.max_rise + search.max_drop;
    let above = point + Vec3::Y * (search.probe_height + search.max_rise);
    let Some(floor) = physics.raycast(above, Dir3::NEG_Y, reach, filter).map(|hit| hit.point.y) else { return false };
    if floor - feet.y > search.max_rise + EPS || feet.y - floor > search.max_drop + EPS {
        return false;
    }
    // Room for the body.
    let at = Vec3::new(point.x, floor + site.height * 0.5 + search.lift, point.z);
    if !physics.overlaps(&Shape::capsule(site.radius, site.height), at, Quat::IDENTITY, filter).is_empty() {
        return false;
    }
    // Not into deep water.
    if let Some(limit) = search.max_water_depth
        && let Some(surface) = physics.water_surface_at(Vec3::new(point.x, floor + 0.05, point.z))
        && surface - floor > limit
    {
        return false;
    }
    true
}

impl<'a> LeapSite<'a> {
    /// The body `entity` as the world has it now; `None` if it is no character with an `Intent`
    /// (nothing to judge a leap by).
    pub fn of(world: &'a World, entity: Entity) -> Option<Self> {
        let physics = world.get_resource::<PhysicsWorld>()?;
        let e = world.get_entity(entity).ok()?;
        let controller = e.get::<CharacterController>()?;
        let params = controller.resolve(e.get::<CharacterParams>(), world.get_resource::<ContentDb>());
        let intent = e.get::<Intent>()?;
        let target = intent
            .target
            .and_then(|id| world.get_resource::<AiIndex>()?.get(id))
            .and_then(|t| Some((world.get::<Transform>(t)?.translation, radius_of(world.get::<Collider>(t)))));
        Some(Self::body(physics, entity, (e.get::<Transform>()?.translation, *e.get::<Transform>()?.forward()), params, intent, target))
    }

    /// A site for a body at `at` (feet, forward) moved by `params`.
    pub fn body(
        physics: &'a PhysicsWorld,
        entity: Entity,
        at: (Vec3, Vec3),
        params: &CharacterParams,
        intent: &'a Intent,
        target: Option<(Vec3, f32)>,
    ) -> Self {
        let radius = params.radius.max(0.01);
        Self {
            physics,
            entity,
            feet: at.0,
            forward: at.1,
            radius,
            height: params.height.max(2.0 * radius),
            intent,
            target,
        }
    }
}

/// The radius of a body for a leap: its collider's widest horizontal half extent, else 0.4 m (the
/// same default [`body_radius`](crate::zone::body_radius) has).
pub(crate) fn radius_of(collider: Option<&Collider>) -> f32 {
    collider.map_or(0.4, |c| {
        let half = c.shape.half_extents();
        half.x.max(half.z)
    })
}
