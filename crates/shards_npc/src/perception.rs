//! What NPCs notice: sight through the physics world, hearing as events, memory.
//!
//! - **Sight** ([`PerceptionDef`]): a hostile fighter within `sight_range`, inside the field
//!   of view (or within `near`, whatever the facing), with a clear ray from the eyes to its
//!   body through the `WORLD` layer.
//! - **Hearing**: a [`Noise`] (games and actions trigger them; the engine makes one for every
//!   hit, around the one hit, and every sprinting character is heard within
//!   [`NpcTuning::sprint_noise`]) reaches the NPCs within its radius times their `hearing`
//!   at once: it waits in their [`Perceived::sounds`] until their next perception pass. A
//!   noise from a hostile fighter tells where it is (at its [`Noise::origin`], else where
//!   the noise is: a hit gives its attacker's place, not the victim's); any other leaves a
//!   point to investigate.
//! - **Memory**: what was sensed stays known for `memory` seconds at its last known place.
//! - **Keep range** ([`PerceptionDef::keep_range`]): a target that is still known stays seen
//!   (its place refreshed) up to this far, with the same field of view and a clear ray;
//!   `sight_range` only decides what is noticed anew.
//!
//! The **focus** (the NPC's target) sticks to what it has while it is known, unless another
//! target is seen much closer; else the nearest seen, else the most recently sensed.
//!
//! Everything an NPC perceives, sounds not yet taken in included, lives in its own
//! [`Perceived`]: replay checks and snapshots see it whole.

use ai_core::AiId;
use bevy::prelude::*;
use shards_action::fighter::hostile;
use shards_action::{Damaged, Dead, Fighter, Team};
use shards_content::ContentDb;
use shards_core::flat;
use shards_motion::{LayerMask, MotionState, PhysicsWorld, QueryFilter};

use crate::brain::{Npc, NpcBrain};
use crate::def::{NpcTuning, PerceptionDef};

/// A sound in the world, heard by NPCs within `radius` (times their hearing).
///
/// ```ignore
/// commands.trigger(Noise { position: door, radius: 12.0, source: Some(player), origin: None });
/// ```
#[derive(Event, Clone, Debug, PartialEq)]
pub struct Noise {
    /// Where the sound is: the radius is measured from here.
    pub position: Vec3,
    pub radius: f32,
    /// Who made it; `None` for the world's own sounds.
    pub source: Option<Entity>,
    /// Where the source stands, when that is not where the sound is (a hit sounds where it
    /// lands, its attacker stands elsewhere): what the NPCs that take the source for a
    /// hostile fighter learn of it. `None`: at `position`.
    pub origin: Option<Vec3>,
}

/// A sound an NPC heard and has not taken in yet.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Sound {
    /// Where it came from: for a hostile `source`, where that stands ([`Noise::origin`]).
    pub pos: Vec3,
    pub source: Option<AiId>,
}

/// One target an NPC knows of.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Sensed {
    pub id: AiId,
    /// Where it was last seen or heard.
    pub pos: Vec3,
    /// Seen this tick.
    pub seen: bool,
    /// Heard this tick.
    pub heard: bool,
    /// Seconds since it was last seen or heard.
    pub age: f32,
}

/// A sound without a known hostile source: somewhere to investigate.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct HeardAt {
    pub pos: Vec3,
    pub age: f32,
}

/// What an NPC knows: targets by id, its focus, the last sound worth a look, and the sounds
/// it heard since its last perception pass.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Perceived {
    /// By id.
    pub known: Vec<Sensed>,
    pub focus: Option<AiId>,
    pub noise: Option<HeardAt>,
    /// Heard, not taken in yet: the next perception pass does, in the order they came.
    pub sounds: Vec<Sound>,
}

impl Perceived {
    pub fn get(&self, id: AiId) -> Option<&Sensed> {
        self.known.iter().find(|k| k.id == id)
    }
}

/// A focus switches to a seen target only when it is this much closer (share of the
/// current focus' distance).
const SWITCH: f32 = 0.6;

/// The body point sight aims at: the collider's centre, else a meter above the feet.
fn body_point(physics: &PhysicsWorld, entity: Entity, feet: Vec3) -> Vec3 {
    physics.collider(entity).map_or(feet + Vec3::Y, |c| c.position)
}

/// Whether nothing on the `WORLD` layer stands between `eye` and `point`.
pub fn sight_clear(physics: &PhysicsWorld, eye: Vec3, point: Vec3) -> bool {
    let delta = point - eye;
    let distance = delta.length();
    let Ok(dir) = Dir3::new(delta) else { return true };
    let filter = QueryFilter::new(LayerMask::WORLD);
    physics.raycast(eye, dir, distance, &filter).is_none_or(|hit| hit.distance >= distance - 0.05)
}

/// Whether an NPC at `feet` facing `facing` with perception `def` sees a body at `point`
/// (feet `target_feet`).
pub fn sees(physics: &PhysicsWorld, def: &PerceptionDef, feet: Vec3, facing: Vec3, target_feet: Vec3, point: Vec3) -> bool {
    sees_within(physics, def, def.sight_range, feet, facing, target_feet, point)
}

/// [`sees`] with the range spelled out: a known target is seen up to
/// [`PerceptionDef::keep_range`], a new one up to `sight_range`.
pub fn sees_within(
    physics: &PhysicsWorld,
    def: &PerceptionDef,
    range: f32,
    feet: Vec3,
    facing: Vec3,
    target_feet: Vec3,
    point: Vec3,
) -> bool {
    let to = flat(target_feet - feet);
    let distance = to.length();
    if distance > range {
        return false;
    }
    if distance > def.near && def.fov_deg < 360.0 {
        let cos = to.normalize_or_zero().dot(flat(facing).normalize_or_zero());
        if cos < ops::cos((def.fov_deg * 0.5).to_radians()) - 1e-5 {
            return false;
        }
    }
    !def.line_of_sight || sight_clear(physics, feet + Vec3::Y * def.eye_height, point)
}

/// A body others may perceive.
struct Candidate {
    entity: Entity,
    id: AiId,
    feet: Vec3,
    point: Vec3,
    team: Option<Team>,
    sprinting: bool,
}

/// `NpcSystems::Sense`: every active NPC, in `AiId` order, looks, listens and remembers.
#[allow(clippy::type_complexity, reason = "a Bevy system: one query per thing it reads")]
pub(crate) fn perceive(
    time: Res<Time>,
    db: Res<ContentDb>,
    physics: Res<PhysicsWorld>,
    bodies: Query<(Entity, &AiId, &Transform, Option<&Team>, Option<&MotionState>), (With<Fighter>, Without<Dead>)>,
    mut npcs: Query<(Entity, &AiId, &Npc, &NpcBrain, &Transform, Option<&Team>, &mut Perceived)>,
) {
    let dt = time.delta_secs();
    let tuning = NpcTuning::of(&db);
    let mut candidates: Vec<Candidate> = bodies
        .iter()
        .map(|(entity, id, tf, team, motion)| Candidate {
            entity,
            id: *id,
            feet: tf.translation,
            point: body_point(&physics, entity, tf.translation),
            team: team.cloned(),
            sprinting: motion.is_some_and(|m| m.sprinting),
        })
        .collect();
    candidates.sort_by_key(|c| c.id);
    let mut order: Vec<(AiId, Entity)> = npcs.iter().map(|(e, id, ..)| (*id, e)).collect();
    order.sort();
    for (id, entity) in order {
        let Ok((_, _, npc, brain, tf, team, mut perceived)) = npcs.get_mut(entity) else { continue };
        if !brain.active {
            if *perceived != Perceived::default() {
                *perceived = Perceived::default();
            }
            continue;
        }
        let def = db.get::<PerceptionDef>(&npc.perception).cloned().unwrap_or_default();
        let next = sense(&def, &tuning, &physics, &candidates, id, entity, tf, team, &perceived, dt);
        if *perceived != next {
            *perceived = next;
        }
    }
}

/// One NPC's perception after this tick.
#[allow(clippy::too_many_arguments, reason = "everything one NPC senses with")]
fn sense(
    def: &PerceptionDef,
    tuning: &NpcTuning,
    physics: &PhysicsWorld,
    candidates: &[Candidate],
    me: AiId,
    entity: Entity,
    tf: &Transform,
    team: Option<&Team>,
    before: &Perceived,
    dt: f32,
) -> Perceived {
    let feet = tf.translation;
    let facing = *tf.forward();
    let mut next = before.clone();
    let sounds = std::mem::take(&mut next.sounds);
    for k in &mut next.known {
        k.age += dt;
        k.seen = false;
        k.heard = false;
    }
    let sensed = |id: AiId, pos: Vec3, seen: bool, heard: bool, known: &mut Vec<Sensed>| {
        match known.binary_search_by_key(&id, |k| k.id) {
            Ok(i) => {
                let k = &mut known[i];
                k.pos = pos;
                k.age = 0.0;
                k.seen |= seen;
                k.heard |= heard;
            }
            Err(i) => known.insert(i, Sensed { id, pos, seen, heard, age: 0.0 }),
        }
    };
    let hostiles = candidates.iter().filter(|c| c.entity != entity && c.id != me && hostile(team, c.team.as_ref()));
    for c in hostiles.clone() {
        // A target still known is kept in sight farther out than a new one is noticed.
        let range = match def.keep_range {
            Some(keep) if next.known.iter().any(|k| k.id == c.id && k.age <= def.memory + 1e-4) => keep.max(def.sight_range),
            _ => def.sight_range,
        };
        let seen = sees_within(physics, def, range, feet, facing, c.feet, c.point);
        let heard = c.sprinting && flat(c.feet - feet).length() <= tuning.sprint_noise * def.hearing;
        if seen || heard {
            sensed(c.id, c.feet, seen, heard, &mut next.known);
        }
    }
    if let Some(n) = &mut next.noise {
        n.age += dt;
    }
    for sound in sounds {
        match sound.source.and_then(|s| hostiles.clone().find(|c| c.id == s)) {
            // Seen this tick: where it is seen beats where a sound came from (a hit's noise
            // comes from the one hit, not from the attacker).
            Some(c) if next.known.iter().any(|k| k.id == c.id && k.seen) => {
                if let Some(k) = next.known.iter_mut().find(|k| k.id == c.id) {
                    k.heard = true;
                }
            }
            Some(c) => sensed(c.id, sound.pos, false, true, &mut next.known),
            None if sound.source != Some(me) => next.noise = Some(HeardAt { pos: sound.pos, age: 0.0 }),
            None => {}
        }
    }
    // Forget the stale, and the dead or gone (they are no candidates any more).
    next.known.retain(|k| k.age <= def.memory + 1e-4 && candidates.iter().any(|c| c.id == k.id));
    if next.noise.is_some_and(|n| n.age > def.memory + 1e-4) {
        next.noise = None;
    }
    next.focus = focus(&next.known, before.focus, feet);
    next
}

/// Who an NPC at `feet` acts on: the current focus while it is known, unless another target
/// is seen much closer; else the nearest seen; else the most recently sensed.
fn focus(known: &[Sensed], current: Option<AiId>, feet: Vec3) -> Option<AiId> {
    let distance = |k: &Sensed| flat(k.pos - feet).length();
    let nearest_seen = known.iter().filter(|k| k.seen).min_by(|a, b| distance(a).total_cmp(&distance(b)));
    if let Some(cur) = current.and_then(|id| known.iter().find(|k| k.id == id)) {
        return match nearest_seen {
            Some(n) if n.id != cur.id && distance(n) < distance(cur) * SWITCH => Some(n.id),
            _ => Some(cur.id),
        };
    }
    nearest_seen
        .or_else(|| known.iter().min_by(|a, b| a.age.total_cmp(&b.age).then(distance(a).total_cmp(&distance(b)))))
        .map(|k| k.id)
}

/// A [`Noise`] reaches every active NPC within its radius (from its `position`) times their
/// hearing, at once. An NPC that takes its source for a hostile fighter hears it from its
/// `origin`: that is where the source is known to be, whatever the noise sounded.
pub(crate) fn hear_noise(
    noise: On<Noise>,
    db: Res<ContentDb>,
    sources: Query<(&AiId, Option<&Team>)>,
    mut npcs: Query<(&Npc, &NpcBrain, &Transform, Option<&Team>, &mut Perceived)>,
) {
    let n = noise.event();
    if !(n.radius > 0.0 && n.position.is_finite()) {
        return;
    }
    let source = n.source.and_then(|e| sources.get(e).ok());
    let origin = n.origin.filter(|o| o.is_finite());
    for (npc, brain, tf, team, mut perceived) in &mut npcs {
        if !brain.active {
            continue;
        }
        let hearing = db.get::<PerceptionDef>(&npc.perception).map_or(PerceptionDef::default().hearing, |d| d.hearing);
        if (n.position - tf.translation).length() <= n.radius * hearing {
            let pos = match (origin, source) {
                (Some(origin), Some((_, source_team))) if hostile(team, source_team) => origin,
                _ => n.position,
            };
            perceived.sounds.push(Sound { pos, source: source.map(|(id, _)| *id) });
        }
    }
}

/// A hit is loud: a noise around the one hit, made by the attacker, who stands where it
/// stands (a shot from behind does not put the attacker at the victim's feet).
pub(crate) fn hit_noise(hit: On<Damaged>, db: Res<ContentDb>, positions: Query<&Transform>, mut commands: Commands) {
    let radius = NpcTuning::of(&db).hit_noise;
    let hit = hit.event();
    if radius <= 0.0 {
        return;
    }
    let Ok(at) = positions.get(hit.target) else { return };
    let origin = positions.get(hit.attacker).ok().map(|t| t.translation);
    commands.trigger(Noise { position: at.translation, radius, source: Some(hit.attacker), origin });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensed(id: u32, x: f32, seen: bool, age: f32) -> Sensed {
        Sensed { id: AiId(id), pos: Vec3::new(x, 0.0, 0.0), seen, heard: false, age }
    }

    #[test]
    fn focus_sticks_unless_another_is_seen_much_closer() {
        let known = [sensed(3, 10.0, true, 0.0), sensed(4, 7.0, true, 0.0)];
        assert_eq!(focus(&known, Some(AiId(3)), Vec3::ZERO), Some(AiId(3)), "7m is not much closer than 10m");
        let known = [sensed(3, 10.0, true, 0.0), sensed(4, 5.0, true, 0.0)];
        assert_eq!(focus(&known, Some(AiId(3)), Vec3::ZERO), Some(AiId(4)));
        let known = [sensed(3, 10.0, false, 2.0), sensed(4, 12.0, false, 1.0)];
        assert_eq!(focus(&known, None, Vec3::ZERO), Some(AiId(4)), "most recently sensed");
        assert_eq!(focus(&[], Some(AiId(3)), Vec3::ZERO), None, "forgotten");
    }
}
