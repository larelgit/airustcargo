//! Deterministic 2D noise for generation steps: value noise on an integer lattice hashed from a
//! seed, a quintic fade, octaves summed (fBm).
//!
//! Only `+ - * /` and `floor`, which IEEE 754 rounds the same on every platform, and integer
//! hashing: the same seed gives the same bits on Windows, Linux and macOS, with no `sin`, no
//! `powf` and no platform maths anywhere (the fade is a polynomial, written with products).
//! FastNoiseLite and the like are not reproduced bit for bit: a game matches another engine's
//! terrain by look, not by value.

/// A seeded noise field. `sample` is in `-1..=1`.
#[derive(Clone, Copy, Debug)]
pub struct Noise {
    seed: u64,
    /// Lattice cells per unit of the coordinates given to `sample`.
    pub frequency: f32,
    pub octaves: u32,
    /// Frequency factor per octave.
    pub lacunarity: f32,
    /// Amplitude factor per octave.
    pub gain: f32,
}

impl Noise {
    /// Four octaves, lacunarity 2, gain 0.5.
    pub fn new(seed: u64, frequency: f32) -> Self {
        Self { seed, frequency, octaves: 4, lacunarity: 2.0, gain: 0.5 }
    }

    /// fBm at `(x, z)`, normalised to `-1..=1`.
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

    /// [`sample`](Self::sample) mapped to `0..=1`.
    pub fn sample01(&self, x: f32, z: f32) -> f32 {
        self.sample(x, z) * 0.5 + 0.5
    }
}

/// Value noise at `(x, z)` in lattice units: `-1..=1`.
fn value(seed: u64, x: f32, z: f32) -> f32 {
    let (fx, fz) = (x.floor(), z.floor());
    let (ix, iz) = (fx as i64, fz as i64);
    let (tx, tz) = (fade(x - fx), fade(z - fz));
    let at = |dx: i64, dz: i64| lattice(seed, ix + dx, iz + dz);
    let a = lerp(at(0, 0), at(1, 0), tx);
    let b = lerp(at(0, 1), at(1, 1), tx);
    lerp(a, b, tz)
}

/// `a + (b - a) * t`.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `6t⁵ - 15t⁴ + 10t³`: flat at both ends, so the field has no creases at lattice lines.
pub fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// `3t² - 2t³` on `t` clamped to `0..=1`: for falloffs and blends in steps. Products only.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
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

/// A cell's hash in `0..1`, from its position and a salt: per-block variation that needs no
/// stream of random numbers (so no order of calls to keep).
pub fn cell_hash(x: i32, y: i32, z: i32, salt: u64) -> f32 {
    let h = mix(mix(mix(salt ^ (x as u32 as u64)) ^ (y as u32 as u64)) ^ (z as u32 as u64));
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

    /// Golden values: these bits are what every machine must compute. If this test fails after a
    /// change, generation changed for every seed: raise the step versions that use noise and say
    /// so in the changelog; never "fix" the numbers silently.
    #[test]
    fn golden_values() {
        let n = Noise::new(7, 0.05);
        let got: Vec<u32> = [(0.0, 0.0), (1.0, 1.0), (12.5, -3.25), (100.0, 200.0), (-77.7, 31.1), (255.0, 255.0)].iter().map(|&(x, z)| n.sample(x, z).to_bits()).collect();
        assert_eq!(got, GOLDEN_SAMPLES, "{got:#x?}");
        let one = Noise { octaves: 1, ..Noise::new(1, 1.0) };
        assert_eq!(one.sample(0.5, 0.5).to_bits(), GOLDEN_ONE, "{:#x}", one.sample(0.5, 0.5).to_bits());
        assert_eq!(mix(1), 0x5692161d100b05e5);
        assert_eq!(cell_hash(3, 4, 5, 9).to_bits(), GOLDEN_CELL, "{:#x}", cell_hash(3, 4, 5, 9).to_bits());
    }

    const GOLDEN_SAMPLES: [u32; 6] = [0xbf2c2600, 0xbf27d711, 0xbec305e0, 0x3e98b9dd, 0x3e936c46, 0xbe93e8ad];
    const GOLDEN_ONE: u32 = 0xbd97a81c;
    const GOLDEN_CELL: u32 = 0x3d8fa660;

    #[test]
    fn octaves_and_gain_shape_the_field() {
        let flat = Noise { octaves: 0, ..Noise::new(3, 0.1) };
        assert_eq!(flat.sample(5.0, 5.0), 0.0);
        let one = Noise { octaves: 1, ..Noise::new(3, 0.1) };
        let four = Noise::new(3, 0.1);
        assert_ne!(one.sample(5.3, 9.1).to_bits(), four.sample(5.3, 9.1).to_bits());
        for i in 0..200 {
            let (x, z) = (i as f32 * 0.37, i as f32 * 0.91 - 40.0);
            let v = four.sample01(x, z);
            assert!((0.0..=1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn the_field_is_continuous_across_lattice_lines() {
        let n = Noise { octaves: 1, ..Noise::new(5, 1.0) };
        for k in -3..4 {
            let (a, b) = (n.sample(k as f32 - 1e-4, 0.3), n.sample(k as f32 + 1e-4, 0.3));
            assert!((a - b).abs() < 1e-3, "{a} {b}");
        }
    }

    #[test]
    fn the_curves_are_exact_at_their_ends() {
        assert_eq!((fade(0.0), fade(1.0), fade(0.5)), (0.0, 1.0, 0.5));
        assert_eq!((smoothstep(-1.0), smoothstep(0.0), smoothstep(0.5), smoothstep(1.0), smoothstep(7.0)), (0.0, 0.0, 0.5, 1.0, 1.0));
        assert_eq!(lerp(2.0, 4.0, 0.25), 2.5);
    }

    #[test]
    fn cell_hash_keeps_far_and_negative_cells_apart() {
        // With the three coordinates packed into one word, (2^21, 0, 0) met (0, 1, 0).
        assert_ne!(cell_hash(1 << 21, 0, 0, 5), cell_hash(0, 1, 0, 5));
        assert_ne!(cell_hash(-1, 0, 0, 5), cell_hash(0, 0, 0, 5));
        assert_ne!(cell_hash(0, 0, 1 << 10, 5), cell_hash(1 << 21, 0, 0, 5));
        assert_ne!(cell_hash(1, 2, 3, 5), cell_hash(3, 2, 1, 5), "the axes are not interchangeable");
    }
}
