//! Deterministic randomness.
//!
//! A single shared RNG resource makes results depend on the order in which systems
//! draw from it, which silently breaks determinism the moment two systems are
//! reordered. Instead every draw is derived from `(seed, tick, entity, salt)`:
//! the same inputs always give the same numbers, whatever the system order.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::id::AiId;
use crate::sim::SimTick;

/// World seed. Same seed + same intents = same game.
#[derive(Resource, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Resource)]
pub struct SimSeed(pub u64);

impl Default for SimSeed {
    fn default() -> Self {
        Self(0x5EED)
    }
}

/// System param that hands out order-independent random streams.
///
/// ```ignore
/// fn boss_brain(rng: SimRng, q: Query<&AiId, With<Boss>>) {
///     for id in &q {
///         let mut r = rng.stream(*id, salt::BOSS_ATTACK);
///         if r.chance(0.3) { /* ... */ }
///     }
/// }
/// ```
#[derive(SystemParam)]
pub struct SimRng<'w> {
    seed: Res<'w, SimSeed>,
    tick: Res<'w, SimTick>,
}

impl SimRng<'_> {
    /// Stream for one entity at the current tick. `salt` separates independent
    /// decisions of the same entity within a tick.
    pub fn stream(&self, id: AiId, salt: u64) -> DetRng {
        DetRng::from_keys(&[self.seed.0, self.tick.0, id.0 as u64, salt])
    }

    /// Stream not tied to an entity (spawners, weather, loot tables...).
    pub fn global(&self, salt: u64) -> DetRng {
        DetRng::from_keys(&[self.seed.0, self.tick.0, u64::MAX, salt])
    }
}

/// SplitMix64: tiny, fast, portable and bit-for-bit reproducible on every platform.
#[derive(Clone, Debug)]
pub struct DetRng {
    state: u64,
}

impl DetRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn from_keys(keys: &[u64]) -> Self {
        let mut h = 0x243F_6A88_85A3_08D3_u64;
        for &k in keys {
            h = mix(h ^ k);
        }
        Self { state: h }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.state)
    }

    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform integer in `[0, n)`; `n` must be > 0.
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u64() >> 32) * n as u64 >> 32) as u32
    }

    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            items.get(self.below(items.len() as u32) as usize)
        }
    }
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_keys_same_numbers() {
        let a: Vec<u64> = (0..8).scan(DetRng::from_keys(&[1, 2, 3]), |r, _| Some(r.next_u64())).collect();
        let b: Vec<u64> = (0..8).scan(DetRng::from_keys(&[1, 2, 3]), |r, _| Some(r.next_u64())).collect();
        assert_eq!(a, b);
        assert_ne!(DetRng::from_keys(&[1, 2, 3]).next_u64(), DetRng::from_keys(&[1, 2, 4]).next_u64());
    }

    #[test]
    fn ranges_stay_in_bounds() {
        let mut r = DetRng::new(7);
        for _ in 0..10_000 {
            let f = r.f32();
            assert!((0.0..1.0).contains(&f));
            assert!(r.below(5) < 5);
            let x = r.range(-2.0, 3.0);
            assert!((-2.0..3.0).contains(&x));
        }
    }
}
