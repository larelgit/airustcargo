//! Deterministic 2D noise for generation steps: value noise on an integer lattice hashed from a
//! seed, a quintic fade, octaves summed (fBm). Only `+ - * /` and `floor`, which IEEE 754 rounds
//! the same on every platform: the same seed gives the same bits on Windows, Linux and macOS,
//! with no `sin` and no platform maths.

/// A seeded noise field. `sample` is in about `-1..1`.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    seed: u64,
    /// Lattice cells per meter.
    pub frequency: f32,
    pub octaves: u32,
    /// Frequency factor per octave.
    pub lacunarity: f32,
    /// Amplitude factor per octave.
    pub gain: f32,
}

impl Noise {
    pub fn new(seed: u64, frequency: f32) -> Self {
        Self { seed, frequency, octaves: 4, lacunarity: 2.0, gain: 0.5 }
    }

    /// fBm at `(x, z)`, normalised to about `-1..1`.
    pub fn sample(&self, x: f32, z: f32) -> f32 {
        let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, self.frequency);
        for o in 0..self.octaves {
            sum += amp * value(self.seed.wrapping_add(o as u64 * 0x9E37_79B9), x * f, z * f);
            norm += amp;
            amp *= self.gain;
            f *= self.lacunarity;
        }
        if norm > 0.0 { sum / norm } else { 0.0 }
    }
}

/// Value noise at `(x, z)` in lattice units: `-1..1`.
fn value(seed: u64, x: f32, z: f32) -> f32 {
    let (fx, fz) = (x.floor(), z.floor());
    let (ix, iz) = (fx as i64, fz as i64);
    let (tx, tz) = (fade(x - fx), fade(z - fz));
    let at = |dx: i64, dz: i64| lattice(seed, ix + dx, iz + dz);
    let a = lerp(at(0, 0), at(1, 0), tx);
    let b = lerp(at(0, 1), at(1, 1), tx);
    lerp(a, b, tz)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `6t⁵ - 15t⁴ + 10t³`: flat at both ends, so the field has no creases at lattice lines.
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// A lattice point's value in `-1..1`.
fn lattice(seed: u64, x: i64, z: i64) -> f32 {
    let h = mix(seed ^ mix((x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (z as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)));
    (h >> 40) as f32 / (1u64 << 23) as f32 - 1.0
}

/// SplitMix64's finaliser.
pub fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A cell's hash in `0..1`: per-block colour variation in the mesher (presentation only).
pub fn cell_hash(x: i32, y: i32, z: i32, salt: u64) -> f32 {
    let h = mix(salt ^ mix((x as u64) ^ ((y as u64) << 21) ^ ((z as u64) << 42)));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_field_and_another_seed_another() {
        let a = Noise::new(7, 0.02);
        let b = Noise::new(8, 0.02);
        let pts: Vec<(f32, f32)> = (0..64).map(|i| (i as f32 * 3.7, i as f32 * 1.3)).collect();
        let va: Vec<u32> = pts.iter().map(|&(x, z)| a.sample(x, z).to_bits()).collect();
        let va2: Vec<u32> = pts.iter().map(|&(x, z)| Noise::new(7, 0.02).sample(x, z).to_bits()).collect();
        let vb: Vec<u32> = pts.iter().map(|&(x, z)| b.sample(x, z).to_bits()).collect();
        assert_eq!(va, va2);
        assert_ne!(va, vb);
        assert!(pts.iter().all(|&(x, z)| a.sample(x, z).abs() <= 1.0));
    }
}
