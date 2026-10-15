//! The grunt's brain: it notices the hero and attacks it.
//!
//! A brain is just another source of `Intent`, like the keyboard and an agent's tools:
//! it writes an order and the engine's systems carry it out (walk into reach, face the
//! target, press the input when the action can start). So the grunt obeys the same combat
//! rules as the player: stamina, windups, cooldowns, stagger. No engine code knows it.
//!
//! Determinism: the brain runs in `SimSet::Think`; the only randomness is the stream
//! `SimRng` hands out for (seed, tick, this grunt), so the result does not depend on the
//! order systems or entities are visited in, and replays reproduce it.

use shards::core::flat;
use shards::core::input_names::{HEAVY, LIGHT};
use shards::prelude::*;

/// Marks a grunt and holds its numbers (set in the prefab, `characters.prefab.ron`).
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Grunt {
    /// Notices the hero within this many meters.
    pub aggro: f32,
    /// Chance (0..1) that its next attack is the heavy one (the slam), not the light one.
    pub heavy_chance: f32,
}

impl Default for Grunt {
    fn default() -> Self {
        Self { aggro: 14.0, heavy_chance: 0.25 }
    }
}

/// Separates this decision from other random draws of the same grunt in a tick.
const SALT_ATTACK: u64 = 1;

pub struct BrainPlugin;

impl Plugin for BrainPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Grunt>().add_systems(FixedUpdate, grunt_brain.in_set(SimSet::Think));
    }
}

/// A grunt with nothing to do looks for the nearest living player within `aggro` and
/// orders itself to attack it once; the order ends when the attack starts, so the next
/// tick it chooses again, and the action's own cooldowns set the pace.
fn grunt_brain(
    rng: SimRng,
    players: Query<(&AiId, &Transform), (With<PlayerId>, Without<Dead>)>,
    mut grunts: Query<(&AiId, &Grunt, &Transform, &mut Intent), (Without<Dead>, Without<PlayerId>)>,
) {
    // Candidates in id order: a tie between two equally near players goes to the lower id,
    // whatever order the ECS happens to list them in.
    let mut targets: Vec<(AiId, Vec3)> = players.iter().map(|(id, tf)| (*id, tf.translation)).collect();
    targets.sort_by_key(|(id, _)| *id);
    for (id, grunt, tf, mut intent) in &mut grunts {
        if !intent.orders.is_empty() {
            continue;
        }
        let nearest = targets
            .iter()
            .map(|(target, at)| (flat(*at - tf.translation).length(), *target))
            .filter(|(distance, _)| *distance <= grunt.aggro)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((_, target)) = nearest else { continue };
        let input = if rng.stream(*id, SALT_ATTACK).chance(grunt.heavy_chance) { HEAVY } else { LIGHT };
        intent.orders.push_back(Order::Attack { target, input: input.to_owned(), repeat: false });
    }
}
