//! Hiding (F11): where an NPC goes to be out of a threat's sight, and the state hooks that take it
//! there (`hide`) or keep it where it is (`cower`).
//!
//! **The search** ([`hiding_spot`]): the navmesh nodes of the body class that serves the NPC that it
//! reaches from where it stands (a breadth-first walk over the navmesh's moves, every node of the walk
//! within `within` meters of the NPC and inside its leash), one node per meter (the nodes of every
//! other column each way), taken farthest from the threat first (ties by node id); the first one the
//! threat cannot see is the spot. Seen means a clear ray through the `WORLD` layer from the threat's
//! eye to the NPC's eye standing there, or to the middle of its body: a wall lower than the eye hides
//! nothing. The walk visits at most [`HIDE_NODES`] nodes and the search casts at most [`HIDE_RAYS`]
//! rays; with no spot in that budget, or off the navmesh (no region, outside it, no class serves the
//! body), there is none, and the NPC cowers. Pure: it reads the world and draws nothing.
//!
//! **The hooks.** `hide` runs to the spot and stays there; it checks with one or two rays a tick that
//! the threat still cannot see the spot, and searches again (at most every `every` seconds) when it
//! can (the threat moved) or when it had none. Its state keeps the spot in `NpcBrain::point` and
//! whether it found one in `NpcBrain::step` (1 hiding, 2 cowering). `cower` stands where it is (the
//! game animates it). The threat is the nearest body the NPC holds for one and knows the place of
//! (`notice: Threats`), else its target, else the noise it heard; with none it keeps to its spot.

use std::collections::{BTreeSet, VecDeque};

use ai_core::AiId;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::flat;
use shards_motion::{params_of, PhysicsWorld};

use crate::brain::{Home, Npc, NpcBrain, NpcCtx, NpcOutput};
use crate::def::{BehaviorDef, Params};
use crate::nav::region::NodeId;
use crate::perception::sight_clear;
use crate::threat::known_threats;

/// Most sight rays one hiding search casts.
pub const HIDE_RAYS: usize = 96;

/// Most navmesh nodes one hiding search walks.
pub const HIDE_NODES: usize = 4096;

/// `NpcBrain::step` of a `hide` state that found a spot.
pub(crate) const HIDING: u32 = 1;

/// `NpcBrain::step` of a `hide` state that found none: it cowers.
pub(crate) const COWERING: u32 = 2;

/// Eye height (m) of a threat whose body is not known (a noise).
const THREAT_EYE: f32 = 1.6;

/// What one hiding search came to ([`hiding_search`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HideSearch {
    /// The spot (feet), if one was found.
    pub spot: Option<Vec3>,
    /// Navmesh nodes walked.
    pub nodes: usize,
    /// Sight rays cast.
    pub rays: usize,
    /// Why there is no spot, for tools: `off the navmesh`, `every place within 12 m is in sight`.
    pub why: Option<String>,
}

/// Where NPC `npc` can hide from a threat whose eye is at `threat`: the reachable navmesh node within
/// `within` meters of the NPC (and inside its leash) farthest from the threat that the threat cannot
/// see (see the module docs); `None` when there is none within the search's budget or the NPC is off
/// the navmesh.
///
/// ```ignore
/// let eye = shards::motion::eye_of(world, shooter).unwrap();
/// if let Some(spot) = shards::npc::hiding_spot(world, npc, eye, 12.0) { /* run there */ }
/// ```
pub fn hiding_spot(world: &World, npc: Entity, threat: Vec3, within: f32) -> Option<Vec3> {
    hiding_search(world, npc, threat, within).spot
}

/// [`hiding_spot`] with what the search cost and why it found nothing.
pub fn hiding_search(world: &World, npc: Entity, threat: Vec3, within: f32) -> HideSearch {
    let none = |why: &str| HideSearch { why: Some(why.to_owned()), ..default() };
    let (Some(tf), Some(params)) = (world.get::<Transform>(npc), params_of(world, npc)) else { return none("not a character") };
    let Some(physics) = world.get_resource::<PhysicsWorld>() else { return none("no physics world") };
    let Some(nav) = crate::nav::serving(world, params) else { return none("off the navmesh (no region, or no body class serves it)") };
    let feet = tf.translation;
    let Some(start) = nav.nearest_node(feet, 1.0, 1.0) else { return none("off the navmesh (it stands on no node)") };
    let home = world.get::<Home>(npc).map(|h| (h.pos.unwrap_or(feet), h.leash));
    let in_leash = |p: Vec3| home.is_none_or(|(at, leash)| flat(p - at).length() <= leash - crate::brain::LEASH_ROOM);
    let (eye, middle) = (params.eye_above_feet(), params.height * 0.5);
    // The walk: breadth first over the moves (their order is the bake's), within `within` of the NPC.
    let mut visited: BTreeSet<NodeId> = BTreeSet::from([start]);
    let mut queue: VecDeque<NodeId> = VecDeque::from([start]);
    let mut candidates: Vec<(f32, NodeId)> = Vec::new();
    let mut moves = Vec::new();
    while let Some(node) = queue.pop_front() {
        let at = nav.node_pos(node);
        let [gx, gz] = nav.node_column(node);
        if !nav.is_swim(node) && gx.rem_euclid(2) == 0 && gz.rem_euclid(2) == 0 && in_leash(at) {
            candidates.push((flat(at - threat).length(), node));
        }
        nav.moves(node, &mut moves);
        for &(to, ..) in &moves {
            if visited.len() >= HIDE_NODES {
                break;
            }
            if flat(nav.node_pos(to) - feet).length() <= within && visited.insert(to) {
                queue.push_back(to);
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut search = HideSearch { nodes: visited.len(), ..default() };
    for (_, node) in candidates {
        if search.rays + 2 > HIDE_RAYS {
            search.why = Some(format!("no place out of sight among the {} rays' worth farthest from the threat", HIDE_RAYS));
            return search;
        }
        let at = nav.node_pos(node);
        search.rays += 1;
        if sight_clear(physics, threat, at + Vec3::Y * eye) {
            continue;
        }
        search.rays += 1;
        if sight_clear(physics, threat, at + Vec3::Y * middle) {
            continue;
        }
        search.spot = Some(at);
        return search;
    }
    search.why = Some(format!("every place it reaches within {within:.0} m is in sight"));
    search
}

/// Whether a body of eye height `eye` and height `height` standing at `spot` is out of sight of a
/// threat whose eye is at `threat` (both rays blocked).
pub(crate) fn hidden(physics: &PhysicsWorld, threat: Vec3, spot: Vec3, eye: f32, height: f32) -> bool {
    !sight_clear(physics, threat, spot + Vec3::Y * eye) && !sight_clear(physics, threat, spot + Vec3::Y * (height * 0.5))
}

/// What an NPC hides from: the nearest threat it knows (`notice: Threats`), else its target, else
/// the noise it heard: the id (none for a noise) and the eye it looks from (the body's eye height above
/// where the NPC knows it, at the height its feet stand).
pub fn threat_eye(c: &NpcCtx) -> Option<(Option<AiId>, Vec3)> {
    let index = c.world.get_resource::<ai_core::AiIndex>();
    let body_eye = |id: AiId, pos: Vec3| {
        let body = index.and_then(|i| i.get(id));
        let feet_y = body.and_then(|e| c.world.get::<Transform>(e)).map_or(pos.y, |t| t.translation.y);
        let above = body.and_then(|e| params_of(c.world, e)).map_or(THREAT_EYE, |p| p.eye_above_feet());
        Vec3::new(pos.x, feet_y + above, pos.z)
    };
    if let Some((id, pos)) = known_threats(c).into_iter().next() {
        return Some((Some(id), body_eye(id, pos)));
    }
    if let Some(t) = c.target {
        return Some((Some(t.id), body_eye(t.id, t.pos)));
    }
    c.perceived.noise.map(|n| (None, n.pos + Vec3::Y * THREAT_EYE))
}

/// `hide`: run to a hiding spot from the threat and stay there; with none, stand (cower). Params:
/// `within` (m, 12: how far it looks for a spot), `speed` (1: runs), `every` (s, 0.5: the least time
/// between two searches), `arrive` (m, 0.3).
pub(crate) fn hide(c: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    out.leashed = true;
    let speed = p.num("speed", 1.0);
    let arrive = p.num("arrive", 0.3);
    let go = |out: &mut NpcOutput, spot: Vec3| {
        out.goal = Some(spot);
        out.within = arrive;
        out.speed = speed;
    };
    let kept = c.brain.point.filter(|_| c.brain.step == HIDING);
    let Some((_, threat)) = threat_eye(c) else {
        // Nothing to hide from now: it keeps to the spot it has.
        if let Some(spot) = kept {
            go(out, spot);
        }
        return;
    };
    let Some(physics) = c.world.get_resource::<PhysicsWorld>() else { return };
    let (eye, height) = params_of(c.world, c.entity).map_or((1.6, 1.8), |b| (b.eye_above_feet(), b.height));
    let seen = kept.is_some_and(|spot| !hidden(physics, threat, spot, eye, height));
    let again = c.brain.step == 0 || ((seen || c.brain.step == COWERING) && c.brain.timer <= 1e-4);
    let spot = if again {
        out.timer = Some(p.num("every", 0.5));
        let found = hiding_spot(c.world, c.entity, threat, p.num("within", 12.0));
        out.point = Some(found);
        out.step = Some(if found.is_some() { HIDING } else { COWERING });
        found
    } else {
        kept
    };
    match spot {
        Some(spot) => {
            go(out, spot);
            if flat(spot - c.pos).length() <= arrive + 0.2 {
                out.face = Some(threat);
            }
        }
        None => out.face = Some(threat),
    }
}

/// `cower`: stand where it is, turned toward the threat (the game animates it: a crouch, hands up).
pub(crate) fn cower(c: &NpcCtx, _: &Params, out: &mut NpcOutput) {
    out.face = threat_eye(c).map(|(_, eye)| eye);
}

/// Whether the NPC's state is a `hide` state (its kind), by its behaviour.
fn hiding_state(world: &World, entity: Entity, brain: &NpcBrain) -> bool {
    let (Some(db), Some(npc)) = (world.get_resource::<ContentDb>(), world.get::<Npc>(entity)) else { return false };
    db.get::<BehaviorDef>(&npc.behavior).is_some_and(|b| b.kind_of(&brain.state) == "hide")
}

/// `no_hiding_spot`: in a `hide` state, its last search found no spot (it cowers there).
pub(crate) fn no_hiding_spot(c: &NpcCtx, _: &Params) -> bool {
    c.brain.step == COWERING && hiding_state(c.world, c.entity, c.brain)
}

/// What an NPC in a `hide` or `cower` state is doing, for tools: `Some((spot, cowering))`; `None` in
/// any other state. Reads content with `peek` (tools are no simulation).
pub fn hide_status(world: &World, entity: Entity) -> Option<(Option<Vec3>, bool)> {
    let (db, npc, brain) = (world.get_resource::<ContentDb>()?, world.get::<Npc>(entity)?, world.get::<NpcBrain>(entity)?);
    let kind = db.peek::<BehaviorDef>(&npc.behavior).map(|b| b.kind_of(&brain.state).to_owned())?;
    match kind.as_str() {
        "hide" if brain.step == HIDING => Some((brain.point, false)),
        "hide" if brain.step == COWERING => Some((None, true)),
        "hide" => Some((None, false)),
        "cower" => Some((None, true)),
        _ => None,
    }
}
