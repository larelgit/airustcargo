//! Avoidance between bodies (E6.2): sampled reciprocal velocity obstacles.
//!
//! Each tick an NPC on its way ([`NavAgent::preferred`](super::NavAgent::preferred): the
//! velocity its route asks for) looks at the character bodies near it ([`AvoidanceDef`]:
//! within `range` of its own, at most `neighbours`, the nearest first) and tries a fixed fan
//! of velocities: the preferred one, ten more headings from 20° to 130° either side at three
//! paces, and standing still. A velocity costs how far it strays from the preferred one
//! (a full stop costs 1, standing still half as much again: a hemmed-in agent steps aside
//! rather than freezing), plus the first collision it would run into within the look-ahead:
//! `weight × (horizon / t − 1)` for a collision `t` seconds away, nothing beyond `horizon`.
//! Going deeper into a body that is already within its room costs too. The cheapest wins;
//! among equals the earlier in the fan, and turns to the right come first and cost a hair
//! less, so two walkers meeting head on both pass on their right. Once it goes round one
//! way it keeps to that side (turning back costs more): an agent stuck behind a ring of
//! bodies walks round it instead of dithering between two gaps too narrow for it.
//!
//! **Reciprocal**: another NPC that avoids this tick takes half of each sidestep (its own
//! velocity is taken to be halfway to its new one), so the two do not both swerve the whole
//! way and swing back. Players, NPCs standing (waiting, swinging), NPCs under an agent's
//! orders and other characters keep their course: they are walked around.
//!
//! The look-ahead never reaches past the moment the NPC stops at its goal: a body standing
//! just beyond its goal is no collision. Toward its own target no room is kept at all
//! (`margin` is for the others): it walks up to it to strike. Within its radius (and margin) of where
//! it would stop, with a body that does not avoid in its way, it stops where it is: a pack
//! round a player waits behind the ones who got there first instead of squeezing about
//! between them.
//!
//! Only arithmetic and square roots in a fixed order, and neighbours sorted by distance,
//! then `AiId`: the same tick gives the same choice on every machine, so it replays.

use ai_core::AiId;
use bevy::prelude::*;
use shards_core::flat;

use crate::def::AvoidanceDef;

/// A character body as it is this tick, for the agents near it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub id: AiId,
    /// Feet.
    pub pos: Vec3,
    /// On the ground plane (m/s).
    pub vel: Vec3,
    pub radius: f32,
    pub height: f32,
    /// It avoids this tick too: half of each sidestep is its own.
    pub reciprocal: bool,
}

/// What one agent asks avoidance.
#[derive(Clone, Copy, Debug)]
pub struct Ask<'a> {
    /// Its own body.
    pub me: Body,
    /// The velocity its route asks for (m/s, on the ground plane).
    pub preferred: Vec3,
    /// Seconds until it stops at its goal at this pace: the look-ahead ends there.
    pub until_stop: f32,
    /// Its target: no room kept toward it.
    pub target: Option<AiId>,
    /// The side it went round on last tick: 1 right, -1 left, 0 it did not avoid.
    pub side: i8,
    pub def: &'a AvoidanceDef,
}

/// What avoidance chose for an agent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Steer {
    /// Its velocity this tick (m/s): the preferred one, unless something is in its way.
    pub vel: Vec3,
    /// The body it bends its way for (the first it would run into), when it does.
    pub avoiding: Option<AiId>,
    /// The side it goes round on: 1 right, -1 left, 0 none (straight on, or no avoiding).
    pub side: i8,
}

/// The headings tried, as `(cos, sin)` of a turn to the right of the preferred heading
/// (negative: to the left): straight on, then 20°, 40°, 60°, 90° and 130°, right first.
const TURNS: [(f32, f32); 11] = [
    (1.0, 0.0),
    (0.939_692_6, 0.342_020_1),
    (0.939_692_6, -0.342_020_1),
    (0.766_044_4, 0.642_787_6),
    (0.766_044_4, -0.642_787_6),
    (0.5, 0.866_025_4),
    (0.5, -0.866_025_4),
    (0.0, 1.0),
    (0.0, -1.0),
    (-0.642_787_6, 0.766_044_4),
    (-0.642_787_6, -0.766_044_4),
];

/// The paces tried, as shares of the preferred speed.
const PACES: [f32; 3] = [1.0, 0.6, 0.3];

/// What a turn to the left costs over the same turn to the right (in full stops): of two
/// equally good ways round, the right one.
const LEFT: f32 = 0.02;

/// What turning to the other side than last tick's costs (in full stops).
const SWITCH: f32 = 0.3;

/// A collision sooner than this (s) costs as much as one this soon: the cost stays finite.
const SOONEST: f32 = 0.02;

/// Going deeper into a body already within its room: full stops per unit of closing speed
/// (shares of the preferred speed), times `weight`.
const CROWD: f32 = 4.0;

/// Standing still costs this much on top of the stop itself: an agent hemmed in steps
/// aside, slowly if need be, rather than freezing in place.
const STILL: f32 = 0.5;

/// The velocity `ask`'s agent takes this tick among `bodies` (its own may be among them).
pub fn steer(ask: &Ask, bodies: &[Body]) -> Steer {
    let keep = Steer { vel: ask.preferred, avoiding: None, side: 0 };
    let top = ask.preferred.length();
    if !ask.def.on() || top <= 1e-4 {
        return keep;
    }
    let me = &ask.me;
    let mut near: Vec<(f32, &Body)> = bodies
        .iter()
        .filter(|b| b.id != me.id && share_height(me, b))
        .map(|b| (flat(b.pos - me.pos).length() - me.radius - b.radius, b))
        .filter(|(gap, _)| *gap <= ask.def.range)
        .collect();
    if near.is_empty() {
        return keep;
    }
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.id.cmp(&b.1.id)));
    near.truncate(ask.def.neighbours.max(1) as usize);
    let horizon = ask.def.horizon.min(ask.until_stop).max(0.0);
    // The worst collision of a velocity, and with whom.
    let threat = |v: Vec3| {
        let mut worst = (0.0f32, None);
        for (_, other) in &near {
            let cost = collision(ask, other, v, horizon, top);
            if cost > worst.0 {
                worst = (cost, Some(other.id));
            }
        }
        worst
    };
    let (danger, cause) = threat(ask.preferred);
    if danger <= 0.0 {
        return keep;
    }
    // Nearly there, behind a body that will not make way (one that stands, a player): it
    // waits there. Two that avoid each other work it out between them instead.
    let yields = near.iter().any(|(_, b)| Some(b.id) == cause && b.reciprocal);
    if !yields && ask.until_stop * top <= me.radius + ask.def.margin {
        return Steer { vel: Vec3::ZERO, avoiding: cause, side: ask.side };
    }
    let forward = ask.preferred / top;
    let right = Vec3::new(-forward.z, 0.0, forward.x);
    let mut best = (danger, ask.preferred, 0.0f32);
    for pace in PACES {
        for (k, &(cos, sin)) in TURNS.iter().enumerate() {
            if k == 0 && pace == 1.0 {
                // The preferred velocity itself, costed above.
                continue;
            }
            let v = (forward * cos + right * sin) * (top * pace);
            // Free to choose, a left turn costs a hair more; keeping to a side, the other costs more.
            let side = if sin == 0.0 {
                0.0
            } else if ask.side == 0 {
                if sin < 0.0 { LEFT } else { 0.0 }
            } else if (ask.side > 0) != (sin > 0.0) {
                SWITCH
            } else {
                0.0
            };
            let cost = (v - ask.preferred).length() / top + side + threat(v).0;
            if cost < best.0 {
                best = (cost, v, sin);
            }
        }
    }
    let still = 1.0 + STILL + threat(Vec3::ZERO).0;
    if still < best.0 {
        best = (still, Vec3::ZERO, 0.0);
    }
    let (_, vel, sin) = best;
    if vel == ask.preferred {
        return keep;
    }
    // Straight on (slower) or standing: the side it was keeping to stays.
    let side = if sin > 0.0 { 1 } else if sin < 0.0 { -1 } else { ask.side };
    Steer { vel, avoiding: cause, side }
}

/// Whether two bodies share some height (one is not on a bridge above the other).
fn share_height(a: &Body, b: &Body) -> bool {
    a.pos.y < b.pos.y + b.height && b.pos.y < a.pos.y + a.height
}

/// What velocity `v` of the asking agent costs because of `other`: the collision within
/// `horizon` (the sooner the dearer), or going deeper into it when it is already too close.
fn collision(ask: &Ask, other: &Body, v: Vec3, horizon: f32, top: f32) -> f32 {
    let me = &ask.me;
    let weight = ask.def.weight;
    let margin = if Some(other.id) == ask.target { 0.0 } else { ask.def.margin };
    let room = me.radius + other.radius + margin;
    let d = flat(other.pos - me.pos);
    // Its velocity relative to the other's; the other takes half of each sidestep when it
    // avoids too.
    let rel = if other.reciprocal { v * 2.0 - me.vel - other.vel } else { v - other.vel };
    let dist = d.length();
    if dist < room {
        let toward = if dist > 1e-4 { d / dist } else { Vec3::new(-ask.preferred.z, 0.0, ask.preferred.x) / top };
        let deeper = rel.dot(toward) / top;
        return if deeper > 0.0 { weight * CROWD * deeper } else { 0.0 };
    }
    // When |d - rel t| reaches the room: a t² - 2 b t + c = 0.
    let a = rel.length_squared();
    let b = d.dot(rel);
    if b <= 0.0 || a <= 1e-8 {
        return 0.0;
    }
    let c = dist * dist - room * room;
    let disc = b * b - a * c;
    if disc < 0.0 {
        return 0.0;
    }
    let t = (b - disc.sqrt()) / a;
    if t >= horizon {
        return 0.0;
    }
    weight * (horizon / t.max(SOONEST) - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(id: u32, x: f32, z: f32, vx: f32, vz: f32, reciprocal: bool) -> Body {
        Body { id: AiId(id), pos: Vec3::new(x, 0.0, z), vel: Vec3::new(vx, 0.0, vz), radius: 0.4, height: 1.8, reciprocal }
    }

    fn ask<'a>(me: Body, preferred: Vec3, def: &'a AvoidanceDef) -> Ask<'a> {
        Ask { me, preferred, until_stop: f32::INFINITY, target: None, side: 0, def }
    }

    #[test]
    fn head_on_walkers_each_pass_on_their_right() {
        let def = AvoidanceDef::default();
        let (a, b) = (body(1, -1.8, 0.0, 3.0, 0.0, true), body(2, 1.8, 0.0, -3.0, 0.0, true));
        let all = [a, b];
        let sa = steer(&ask(a, a.vel, &def), &all);
        let sb = steer(&ask(b, b.vel, &def), &all);
        // Facing +X the right is +Z (south); facing -X it is -Z.
        assert!(sa.vel.z > 0.1 && sb.vel.z < -0.1, "{sa:?} {sb:?}");
        assert!(sa.vel.x > 0.0 && sb.vel.x < 0.0, "neither backs up: {sa:?} {sb:?}");
        assert_eq!((sa.avoiding, sb.avoiding), (Some(AiId(2)), Some(AiId(1))));
        assert_eq!((sa.side, sb.side), (1, 1));
    }

    #[test]
    fn once_round_one_side_it_keeps_to_it() {
        let def = AvoidanceDef::default();
        let me = body(1, 0.0, 0.0, 0.0, 0.0, true);
        // A body dead ahead: free to choose, it goes round on its right (+Z).
        let post = body(2, 2.0, 0.0, 0.0, 0.0, false);
        let fresh = steer(&ask(me, Vec3::new(3.0, 0.0, 0.0), &def), &[me, post]);
        assert_eq!(fresh.side, 1, "{fresh:?}");
        assert!(fresh.vel.z > 0.1, "{fresh:?}");
        // Going round on its left since last tick: it stays left.
        let kept = steer(&Ask { side: -1, ..ask(me, Vec3::new(3.0, 0.0, 0.0), &def) }, &[me, post]);
        assert_eq!(kept.side, -1, "{kept:?}");
        assert!(kept.vel.z < -0.1, "{kept:?}");
    }

    #[test]
    fn nobody_in_the_way_keeps_the_route_exactly() {
        let def = AvoidanceDef::default();
        let me = body(1, 0.0, 0.0, 3.0, 0.0, true);
        let preferred = Vec3::new(2.9, 0.0, 0.3);
        // Far away, behind, beside (on the side it bears away from) and going the same way, on
        // a bridge above.
        let far = body(2, 40.0, 0.0, -3.0, 0.0, true);
        let behind = body(3, -2.0, 0.0, 0.0, 0.0, false);
        let alongside = body(4, 0.0, -1.5, 3.0, 0.0, true);
        let above = Body { pos: Vec3::new(2.0, 3.0, 0.0), ..body(5, 0.0, 0.0, 0.0, 0.0, false) };
        let s = steer(&ask(me, preferred, &def), &[me, far, behind, alongside, above]);
        assert_eq!(s, Steer { vel: preferred, avoiding: None, side: 0 });
    }

    #[test]
    fn weight_zero_is_no_avoidance() {
        let def = AvoidanceDef { weight: 0.0, ..default() };
        let (a, b) = (body(1, -1.0, 0.0, 3.0, 0.0, true), body(2, 1.0, 0.0, -3.0, 0.0, true));
        assert_eq!(steer(&ask(a, a.vel, &def), &[a, b]), Steer { vel: a.vel, avoiding: None, side: 0 });
    }

    #[test]
    fn a_standing_body_is_walked_around_and_the_target_walked_up_to() {
        let def = AvoidanceDef::default();
        let me = body(1, 0.0, 0.0, 3.0, 0.0, true);
        let post = body(2, 2.0, 0.1, 0.0, 0.0, false);
        let s = steer(&ask(me, Vec3::new(3.0, 0.0, 0.0), &def), &[me, post]);
        assert!(s.vel.length() > 0.5 && s.vel.z.abs() > 0.3, "around it, not to a halt: {s:?}");
        assert_eq!(s.avoiding, Some(AiId(2)));
        // The same body as its target, the goal 1.5 m short of it: it walks straight up.
        let at_goal = Ask { target: Some(AiId(2)), until_stop: 0.15, ..ask(me, Vec3::new(3.0, 0.0, 0.0), &def) };
        assert_eq!(steer(&at_goal, &[me, post]).vel, Vec3::new(3.0, 0.0, 0.0));
    }

    #[test]
    fn nearly_there_with_its_way_full_it_stops() {
        let def = AvoidanceDef::default();
        let me = body(1, 0.0, 0.0, 3.0, 0.0, true);
        let ahead = body(2, 1.1, 0.0, 0.0, 0.0, false);
        // 0.45 m short of where it stops (0.15 s at 3 m/s): it waits.
        let near = Ask { until_stop: 0.15, ..ask(me, Vec3::new(3.0, 0.0, 0.0), &def) };
        assert_eq!(steer(&near, &[me, ahead]), Steer { vel: Vec3::ZERO, avoiding: Some(AiId(2)), side: 0 });
        // 3 m short: it goes round.
        let far = Ask { until_stop: 1.0, ..near };
        assert!(steer(&far, &[me, ahead]).vel.length() > 0.5);
    }

    #[test]
    fn too_close_already_it_never_goes_deeper() {
        let def = AvoidanceDef::default();
        let me = body(1, 0.0, 0.0, 0.0, 0.0, true);
        let squeezed = body(2, 0.7, 0.0, 0.0, 0.0, false);
        let s = steer(&ask(me, Vec3::new(3.0, 0.0, 0.0), &def), &[me, squeezed]);
        assert!(s.vel.x <= 1e-4, "not into it: {s:?}");
        assert!(s.vel.z > 0.1, "but aside (to the right), not frozen: {s:?}");
    }

    #[test]
    fn the_same_question_gets_the_same_answer_whatever_the_order_of_bodies() {
        let def = AvoidanceDef { neighbours: 2, ..default() };
        let me = body(1, 0.0, 0.0, 3.0, 0.0, true);
        let others = [body(4, 2.0, 0.6, -1.0, 0.0, true), body(2, 2.0, -0.6, -1.0, 0.0, false), body(3, 1.5, 0.0, 0.0, 1.0, true)];
        let a = steer(&ask(me, me.vel, &def), &[me, others[0], others[1], others[2]]);
        let b = steer(&ask(me, me.vel, &def), &[others[2], others[1], me, others[0]]);
        assert_eq!(a, b);
    }
}
