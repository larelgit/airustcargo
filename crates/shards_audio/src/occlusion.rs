//! Occlusion: what stands between the ears and a sound, and how much of the sound gets through.
//!
//! Off unless the mixer asks (`occlusion: Some((gain: 0.3))` in its `*.audio.ron`), so a game that
//! never asked hears every sound as it always did. When it asks, each spatial voice gets a ray from the
//! listener to its source through the physics world's `WORLD` colliders (read only, like the rest of
//! the mixer: the colliders are the simulation's, and sound changes nothing in them). Every collider on
//! the way lets a share of the sound through: its [`SoundTransmission`] if it carries one (glass 0.6, a
//! slab 0.1), else the mixer's `gain`. The shares multiply. The result is only a gain on the voice:
//! Bevy's sinks have no per-voice filter, so a muffled voice is quieter, not duller.
//!
//! This machine's, like the mixer: nothing here reaches the replay hash or a snapshot.

use bevy::prelude::*;
use shards_motion::{LayerMask, PhysicsWorld, QueryFilter};

/// How much of a sound a collider lets through (a component on the collider's entity): 1 is air, 0
/// is a wall nothing passes. Colliders that carry none let through the mixer's `occlusion.gain`.
/// Reflected, so a level file sets it on a collider's node (`SoundTransmission = 0.6` next to
/// `collider = box`).
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct SoundTransmission(pub f32);

impl Default for SoundTransmission {
    fn default() -> Self {
        Self(1.0)
    }
}

impl SoundTransmission {
    /// The share, kept to 0 to 1 (a value that is not a number lets nothing through).
    pub fn share(self) -> f32 {
        if self.0.is_nan() { 0.0 } else { self.0.clamp(0.0, 1.0) }
    }
}

/// A collider between the ears and a voice.
#[derive(Clone, Debug, PartialEq)]
pub struct Blocker {
    pub entity: Entity,
    /// What the AI calls it: its `Name` (a level node's), else its id (`e20`), else the entity.
    pub name: String,
    /// The share of the sound it lets through.
    pub passes: f32,
}

/// What stands between the ears and a voice: the share of the sound that arrives and the colliders
/// that take the rest, nearest the ears first.
#[derive(Clone, Debug, PartialEq)]
pub struct Occluded {
    pub passes: f32,
    pub by: Vec<Blocker>,
}

impl Default for Occluded {
    fn default() -> Self {
        Self::CLEAR
    }
}

impl Occluded {
    /// Nothing in the way.
    pub const CLEAR: Occluded = Occluded { passes: 1.0, by: Vec::new() };

    /// `Glass ×0.60, Wall ×0.30`: the colliders in the way and what each lets through; empty when clear.
    pub fn describe(&self) -> String {
        self.by.iter().map(|b| format!("{} ×{:.2}", b.name, b.passes)).collect::<Vec<_>>().join(", ")
    }
}

/// The most colliders one ray counts: past this many the sound is quiet whatever the rest let through.
pub const MAX_BLOCKERS: usize = 8;

/// The most casts one ray makes (the ones that find the ears' own floor included).
const MAX_CASTS: usize = 12;

/// A ray shorter than this (m) meets nothing: the source is at the ears.
const MIN_RAY: f32 = 0.05;

/// A collider the ray meets this close to the ears (m) is not between them and the source: it is what
/// the listener stands on or against (a floor under the feet, a wall at the shoulder), which a ray
/// cast from a point on its surface starts inside.
const EAR_CLEARANCE: f32 = 0.05;

/// The last stretch of the ray (m) is not tested, so a source set into a wall's face (a vent in it)
/// is not behind that wall.
const SOURCE_CLEARANCE: f32 = 0.1;

/// What stands between `ear` and `source`: the colliders of the `WORLD` layer on the segment, except
/// `skip` (the listener's body and the source's own entity). Each lets through its
/// [`SoundTransmission`], else `through`. Clear when the world has no physics.
pub fn measure(world: &World, ear: Vec3, source: Vec3, skip: &[Entity], through: f32) -> Occluded {
    let Some(physics) = world.get_resource::<PhysicsWorld>() else { return Occluded::CLEAR };
    let offset = source - ear;
    let reach = offset.length() - SOURCE_CLEARANCE;
    let Ok(dir) = Dir3::new(offset) else { return Occluded::CLEAR };
    if reach < MIN_RAY {
        return Occluded::CLEAR;
    }
    let mut filter = QueryFilter::new(LayerMask::WORLD);
    filter.exclude.extend_from_slice(skip);
    let mut occluded = Occluded::CLEAR;
    // One hit at a time: each collider found is left out of the next cast, so a wall counts once
    // however thick it is, and the ray goes on through it to the next.
    for _ in 0..MAX_CASTS {
        if occluded.by.len() == MAX_BLOCKERS {
            break;
        }
        let Some(hit) = physics.raycast(ear, dir, reach, &filter) else { break };
        filter.exclude.push(hit.entity);
        if hit.distance <= EAR_CLEARANCE {
            continue;
        }
        let passes = world.get::<SoundTransmission>(hit.entity).map_or(through, |t| t.share());
        occluded.passes *= passes;
        occluded.by.push(Blocker { entity: hit.entity, name: name_of(world, hit.entity), passes });
    }
    occluded
}

/// What the AI calls an entity here: its `Name` (a level node's) and its id (`e20`), whichever it has,
/// else the entity.
pub(crate) fn name_of(world: &World, entity: Entity) -> String {
    match (world.get::<Name>(entity), world.get::<ai_core::AiId>(entity)) {
        (Some(name), Some(id)) => format!("{} {id}", name.as_str()),
        (Some(name), None) => name.as_str().to_owned(),
        (None, _) => ai_core::label(world, entity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transmission_is_kept_between_nothing_and_everything() {
        assert_eq!(SoundTransmission(0.6).share(), 0.6);
        assert_eq!(SoundTransmission(2.0).share(), 1.0);
        assert_eq!(SoundTransmission(-1.0).share(), 0.0);
        assert_eq!(SoundTransmission(f32::NAN).share(), 0.0);
        assert_eq!(SoundTransmission::default().share(), 1.0);
    }

    #[test]
    fn a_world_without_physics_is_clear() {
        let world = World::new();
        assert_eq!(measure(&world, Vec3::ZERO, Vec3::new(0.0, 0.0, -5.0), &[], 0.3), Occluded::CLEAR);
        assert_eq!(Occluded::CLEAR.describe(), "");
    }

    #[test]
    fn what_is_in_the_way_is_said_with_its_share() {
        let blocked = Occluded {
            passes: 0.18,
            by: vec![
                Blocker { entity: Entity::PLACEHOLDER, name: "Glass".into(), passes: 0.6 },
                Blocker { entity: Entity::PLACEHOLDER, name: "Wall".into(), passes: 0.3 },
            ],
        };
        assert_eq!(blocked.describe(), "Glass ×0.60, Wall ×0.30");
    }
}
