//! Flicker as a pure function of a clock and a seed: the same moment of the simulation's clock gives the
//! same brightness on every machine, with a window and without. It never draws from `SimRng`: a light's
//! noise is seeded by what the light is, so no gameplay number moves.

use shards_settings::FlashLevel;

use crate::def::Flicker;

/// The most a flicker steps or changes a second under `accessibility.flashes = "reduced"`.
pub const REDUCED_RATE: f32 = 2.0;

/// FNV-1a over `bytes`, continuing from `hash`.
fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// A light's seed: a hash of its content id and the key of its place (a node's name and where it stands), so two
/// lamps with the same definition wander differently, and the same lamp does the same in every run.
pub fn seed_of(id: &str, key: &str) -> u64 {
    fnv(fnv(fnv(FNV_BASIS, id.as_bytes()), &[0xff]), key.as_bytes())
}

/// The brightness of a pattern letter: `a` is 0, `m` is 1 (the light as written), `z` is 25/12.
pub fn letter_level(letter: char) -> f32 {
    let step = (letter as u32).saturating_sub('a' as u32).min(25);
    step as f32 / 12.0
}

/// The brightness factor of `flicker` at `clock` (s of simulation time) for a light with `seed`: 1 is the light
/// as written, 0 dark. [`Flicker::None`] is always 1.
pub fn level(flicker: &Flicker, clock: f64, seed: u64) -> f32 {
    match flicker {
        Flicker::None => 1.0,
        Flicker::Pattern { pattern, rate } => {
            let letters = pattern.chars().count();
            if letters == 0 || !rate.is_finite() {
                return 1.0;
            }
            let step = (clock * f64::from(*rate)).floor() as i64;
            let at = step.rem_euclid(letters as i64) as usize;
            pattern.chars().nth(at).map_or(1.0, letter_level)
        }
        Flicker::Noise { rate, depth } => {
            if !rate.is_finite() || !depth.is_finite() {
                return 1.0;
            }
            1.0 - depth.clamp(0.0, 1.0) * noise(clock * f64::from(*rate), seed)
        }
    }
}

/// The flicker the player's `accessibility.flashes` lets through: `Full` as written; `Reduced` no faster than
/// [`REDUCED_RATE`] a second and half as deep (noise) ; `Off` steady.
pub fn shaped(flicker: &Flicker, flashes: FlashLevel) -> Flicker {
    match (flashes, flicker) {
        (FlashLevel::Full, flicker) => flicker.clone(),
        (FlashLevel::Off, _) | (_, Flicker::None) => Flicker::None,
        (FlashLevel::Reduced, Flicker::Pattern { pattern, rate }) => Flicker::Pattern { pattern: pattern.clone(), rate: rate.min(REDUCED_RATE) },
        (FlashLevel::Reduced, Flicker::Noise { rate, depth }) => Flicker::Noise { rate: rate.min(REDUCED_RATE), depth: depth * 0.5 },
    }
}

/// [`level`] of the [`shaped`] flicker; under `Reduced` a pattern's swing round the light as written is halved too.
pub fn level_for(flicker: &Flicker, clock: f64, seed: u64, flashes: FlashLevel) -> f32 {
    let raw = level(&shaped(flicker, flashes), clock, seed);
    match (flashes, flicker) {
        (FlashLevel::Reduced, Flicker::Pattern { .. }) => 1.0 + (raw - 1.0) * 0.5,
        _ => raw,
    }
}

/// Which step of a pattern the clock is at, `None` for the other flickers.
pub fn step_of(flicker: &Flicker, clock: f64) -> Option<usize> {
    match flicker {
        Flicker::Pattern { pattern, rate } if !pattern.is_empty() && rate.is_finite() => {
            let step = (clock * f64::from(*rate)).floor() as i64;
            Some(step.rem_euclid(pattern.chars().count() as i64) as usize)
        }
        _ => None,
    }
}

/// Smooth value noise in 0 to 1: a hashed value at every whole `x`, eased between (smoothstep).
fn noise(x: f64, seed: u64) -> f32 {
    let cell = x.floor();
    let t = (x - cell) as f32;
    let at = |i: f64| {
        let hash = fnv(fnv(FNV_BASIS, &seed.to_le_bytes()), &(i as i64).to_le_bytes());
        // 24 bits: exact in an f32.
        ((hash >> 40) as f32) / 16_777_216.0
    };
    let (a, b) = (at(cell), at(cell + 1.0));
    a + (b - a) * (t * t * (3.0 - 2.0 * t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_steps_through_its_letters_at_its_rate_and_loops() {
        let flicker = Flicker::Pattern { pattern: "amz".into(), rate: 2.0 };
        let at = |clock: f64| level(&flicker, clock, 0);
        assert_eq!(at(0.0), 0.0, "a is dark");
        assert_eq!(at(0.49), 0.0);
        assert_eq!(at(0.5), 1.0, "m is the light as written");
        assert_eq!(at(1.0), 25.0 / 12.0, "z is about twice");
        assert_eq!(at(1.5), 0.0, "and it loops");
        assert_eq!(at(2.0), 1.0, "the second lap: m again");
        assert_eq!(step_of(&flicker, 1.2), Some(2));
        assert_eq!(step_of(&Flicker::None, 1.2), None);
        assert_eq!(level(&flicker, -0.25, 0), 25.0 / 12.0, "before zero it counts back from the end");
    }

    #[test]
    fn noise_stays_inside_its_depth_and_is_the_same_every_time() {
        let flicker = Flicker::Noise { rate: 6.0, depth: 0.4 };
        let seed = seed_of("sodium", "Lamp.001@1");
        let run: Vec<f32> = (0..600).map(|i| level(&flicker, f64::from(i) / 60.0, seed)).collect();
        assert!(run.iter().all(|l| (0.6..=1.0).contains(l)), "between 1 - depth and 1: {:?}", run.iter().fold((9.0f32, 0.0f32), |a, l| (a.0.min(*l), a.1.max(*l))));
        let (low, high) = run.iter().fold((9.0f32, 0.0f32), |a, l| (a.0.min(*l), a.1.max(*l)));
        assert!(high - low > 0.2, "it moves: {low}..{high}");
        let again: Vec<f32> = (0..600).map(|i| level(&flicker, f64::from(i) / 60.0, seed)).collect();
        assert_eq!(run, again);
        let other: Vec<f32> = (0..600).map(|i| level(&flicker, f64::from(i) / 60.0, seed_of("sodium", "Lamp.002@2"))).collect();
        assert_ne!(run, other, "another lamp wanders another way");
        // Smooth: neighbouring ticks differ by little.
        assert!(run.windows(2).all(|w| (w[0] - w[1]).abs() < 0.1), "no jumps");
        assert_eq!(level(&Flicker::Noise { rate: 6.0, depth: 0.0 }, 3.3, seed), 1.0, "no depth is steady");
        assert_eq!(level(&Flicker::None, 3.3, seed), 1.0);
    }

    #[test]
    fn the_players_flash_setting_shapes_a_flicker() {
        let pattern = Flicker::Pattern { pattern: "ma".into(), rate: 10.0 };
        assert_eq!(shaped(&pattern, FlashLevel::Full), pattern, "full: as written");
        assert_eq!(level_for(&pattern, 0.15, 0, FlashLevel::Full), 0.0, "the dark letter is dark");
        // Reduced: no faster than twice a second, and the swing round the light as written halved.
        assert_eq!(shaped(&pattern, FlashLevel::Reduced), Flicker::Pattern { pattern: "ma".into(), rate: REDUCED_RATE });
        assert_eq!(level_for(&pattern, 0.6, 0, FlashLevel::Reduced), 0.5, "the dark letter is half light");
        assert_eq!(level_for(&pattern, 0.1, 0, FlashLevel::Reduced), 1.0, "and the written one is as written");
        let slow = Flicker::Pattern { pattern: "ma".into(), rate: 1.0 };
        assert_eq!(shaped(&slow, FlashLevel::Reduced), slow, "a flicker already slow keeps its rate");
        // Off: steady.
        assert_eq!(shaped(&pattern, FlashLevel::Off), Flicker::None);
        assert!((0..100).all(|i| level_for(&pattern, f64::from(i) / 17.0, 0, FlashLevel::Off) == 1.0));
        // Noise: slower and half as deep.
        let noise = Flicker::Noise { rate: 20.0, depth: 0.4 };
        assert_eq!(shaped(&noise, FlashLevel::Reduced), Flicker::Noise { rate: REDUCED_RATE, depth: 0.2 });
        let run: Vec<f32> = (0..600).map(|i| level_for(&noise, f64::from(i) / 60.0, 7, FlashLevel::Reduced)).collect();
        assert!(run.iter().all(|l| (0.8..=1.0).contains(l)), "inside half the depth");
        assert!(run.windows(2).all(|w| (w[0] - w[1]).abs() < 0.011), "and slow");
        assert_eq!(shaped(&Flicker::None, FlashLevel::Reduced), Flicker::None);
    }
}
