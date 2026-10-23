//! Flicker as a pure function of a clock and a seed: the same moment of the simulation's clock gives the
//! same brightness on every machine, with a window and without. It never draws from `SimRng`: a light's
//! noise is seeded by what the light is, so no gameplay number moves.

use crate::def::Flicker;

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
}
