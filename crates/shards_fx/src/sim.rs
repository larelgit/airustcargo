//! Particles as numbers: where one is, how big, what colour, at any age. A particle's state is a
//! function of its age (its motion under gravity and drag is solved, not stepped), so it is the
//! same whatever the frame rate, and a `step` of many ticks between two frames puts every particle
//! where those ticks take it. A pool keeps an effect's particles in a fixed number of slots; one
//! more than it holds takes the slot of the oldest.

use std::f32::consts::TAU;

use bevy::prelude::*;

use crate::def::{Aim, EffectDef, MAX_LIFETIME};

/// A small, fast random source (SplitMix64) for what effects pick: picture only, never gameplay.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// 0 to 1, 1 left out.
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Between `min` and `max`.
    pub fn between(&mut self, (min, max): (f32, f32)) -> f32 {
        min + (max - min) * self.unit()
    }

    /// A direction in the cone of `spread` degrees (the whole angle) around the unit `axis`, every
    /// direction of it as likely.
    pub fn in_cone(&mut self, axis: Vec3, spread: f32) -> Vec3 {
        let half = (spread.clamp(0.0, 360.0) * 0.5).to_radians();
        let cos = 1.0 - self.unit() * (1.0 - ops::cos(half));
        let sin = (1.0 - cos * cos).max(0.0).sqrt();
        let phi = TAU * self.unit();
        let local = Vec3::new(sin * ops::cos(phi), cos, sin * ops::sin(phi));
        Quat::from_rotation_arc(Vec3::Y, axis.normalize_or(Vec3::Y)) * local
    }

    /// A point in the ball of `radius` around the origin, every point of it as likely.
    pub fn in_ball(&mut self, radius: f32) -> Vec3 {
        if radius <= 0.0 {
            return Vec3::ZERO;
        }
        radius * ops::cbrt(self.unit()) * self.in_cone(Vec3::Y, 360.0)
    }
}

/// One particle: where and when it started, how fast, how big, how it tumbles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// The clock (s) when it started.
    pub born: f64,
    /// Seconds it lives.
    pub life: f32,
    pub origin: Vec3,
    pub velocity: Vec3,
    /// Size at birth (m).
    pub size: f32,
    pub spin_axis: Vec3,
    /// Radians a second.
    pub spin: f32,
}

/// What a particle looks like at an age.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shown {
    pub position: Vec3,
    pub size: f32,
    /// sRGB bytes, alpha last.
    pub color: [u8; 4],
    /// How a cube or a shard is turned.
    pub rotation: Quat,
}

impl Particle {
    /// Its share of its life gone at clock `now`: 0 at birth, 1 at death (more after).
    pub fn spent(&self, now: f64) -> f32 {
        ((now - self.born) as f32 / self.life.max(1e-4)).max(0.0)
    }

    pub fn alive(&self, now: f64) -> bool {
        now - self.born < f64::from(self.life)
    }

    /// Where it is `age` seconds after its birth, under `gravity` (m/s² along Y) and `drag` (its
    /// speed falls as e^(-drag·t)): the motion solved, not stepped.
    pub fn position(&self, age: f32, gravity: f32, drag: f32) -> Vec3 {
        let g = Vec3::Y * gravity;
        if drag <= 1e-4 {
            return self.origin + self.velocity * age + 0.5 * g * age * age;
        }
        // v(t) = g/k + (v0 - g/k)·e^(-kt); x(t) = x0 + g/k·t + (v0 - g/k)·(1 - e^(-kt))/k.
        let terminal = g / drag;
        self.origin + terminal * age + (self.velocity - terminal) * ((1.0 - ops::exp(-drag * age)) / drag)
    }

    /// How it looks at clock `now`, as `def` draws it (colours from [`EffectDef::colors`]).
    pub fn shown(&self, now: f64, def: &EffectDef, colors: ([u8; 4], [u8; 4])) -> Shown {
        let age = ((now - self.born) as f32).max(0.0);
        let t = self.spent(now).min(1.0);
        let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
        let (start, end) = colors;
        Shown {
            position: self.position(age, def.gravity, def.drag),
            size: self.size * (1.0 + (def.size_end - 1.0) * t),
            color: [lerp(start[0], end[0]), lerp(start[1], end[1]), lerp(start[2], end[2]), lerp(start[3], end[3])],
            rotation: Quat::from_axis_angle(self.spin_axis, self.spin * age),
        }
    }
}

/// The way an effect's particles leave: `aim` of the effect, along `toward` when it has one
/// (`Hit`: for a hit, back towards the attacker), level, tilted up by `lift`.
pub fn aim_of(def: &EffectDef, toward: Option<Vec3>) -> Vec3 {
    match (def.aim, toward.map(|d| Vec3::new(d.x, 0.0, d.z)).and_then(Vec3::try_normalize)) {
        (Aim::Hit, Some(level)) => {
            let lift = def.lift.to_radians();
            (level * ops::cos(lift) + Vec3::Y * ops::sin(lift)).normalize_or(Vec3::Y)
        }
        _ => Vec3::Y,
    }
}

/// A new particle of `def` at `at` (offset and start ball included), leaving along `aim`, born at
/// clock `born`.
pub fn spawn(def: &EffectDef, at: Vec3, aim: Vec3, born: f64, rng: &mut Rng) -> Particle {
    let (x, y, z) = def.offset;
    let origin = at + Vec3::new(x, y, z) + rng.in_ball(def.radius);
    let velocity = rng.in_cone(aim, def.spread) * rng.between(def.speed);
    Particle {
        born,
        life: rng.between(def.lifetime).clamp(1e-3, MAX_LIFETIME),
        origin,
        velocity,
        size: rng.between(def.size),
        spin_axis: rng.in_cone(Vec3::Y, 360.0),
        spin: def.spin.to_radians() * rng.between((-1.0, 1.0)),
    }
}

/// An effect's particles: a fixed number of slots, each empty or holding one.
#[derive(Clone, Debug, PartialEq)]
pub struct Pool {
    pub slots: Vec<Option<Particle>>,
    /// Particles started in it.
    pub started: u64,
    /// Particles that took the slot of a live one (the pool was full).
    pub recycled: u64,
}

impl Pool {
    pub fn new(size: usize) -> Self {
        Self { slots: vec![None; size], started: 0, recycled: 0 }
    }

    /// Puts `particle` in a free slot, else in the oldest one's; the slot it took.
    pub fn add(&mut self, particle: Particle) -> Option<usize> {
        let slot = match self.slots.iter().position(Option::is_none) {
            Some(free) => free,
            None => {
                // The oldest: the earliest born, the first slot between equals.
                let (oldest, _) = self
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.map(|p| (i, p.born)))
                    .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))?;
                self.recycled += 1;
                oldest
            }
        };
        self.slots[slot] = Some(particle);
        self.started += 1;
        Some(slot)
    }

    /// Empties the slots of the particles dead at clock `now` (and of any born after it: the clock
    /// went back, a snapshot was loaded).
    pub fn expire(&mut self, now: f64) {
        for slot in &mut self.slots {
            if slot.is_some_and(|p| !p.alive(now) || p.born > now + 1e-6) {
                *slot = None;
            }
        }
    }

    pub fn live(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn size(&self) -> usize {
        self.slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn particle(velocity: Vec3) -> Particle {
        Particle { born: 1.0, life: 2.0, origin: Vec3::ZERO, velocity, size: 0.2, spin_axis: Vec3::Y, spin: 1.0 }
    }

    #[test]
    fn motion_is_solved_so_any_age_is_where_small_steps_lead() {
        let p = particle(Vec3::new(3.0, 4.0, 0.0));
        // Without drag: a parabola.
        assert!(p.position(1.0, -10.0, 0.0).abs_diff_eq(Vec3::new(3.0, -1.0, 0.0), 1e-5));
        // With drag: the same as stepping v' = g - k·v in many small steps.
        let (g, k) = (-9.8, 1.5);
        let (mut x, mut v, dt) = (Vec3::ZERO, p.velocity, 1e-4);
        for _ in 0..10_000 {
            v += (Vec3::Y * g - v * k) * dt;
            x += v * dt;
        }
        let solved = p.position(1.0, g, k);
        assert!(solved.abs_diff_eq(x, 2e-3), "{solved} vs stepped {x}");
        // Drag alone ends at a standstill: v0/k away.
        assert!(p.position(100.0, 0.0, 2.0).abs_diff_eq(p.velocity / 2.0, 1e-4));
    }

    #[test]
    fn a_particle_shrinks_and_fades_over_its_life() {
        let def = EffectDef { size_end: 0.5, ..EffectDef::default() };
        let p = particle(Vec3::ZERO);
        let colors = ([255, 200, 100, 255], [55, 0, 100, 0]);
        let born = p.shown(1.0, &def, colors);
        assert_eq!((born.size, born.color), (0.2, [255, 200, 100, 255]));
        let half = p.shown(2.0, &def, colors);
        assert!((half.size - 0.15).abs() < 1e-6, "{}", half.size);
        assert_eq!(half.color, [155, 100, 100, 128]);
        assert_eq!(p.shown(3.0, &def, colors).color, [55, 0, 100, 0]);
        assert!(p.alive(2.99) && !p.alive(3.0));
        assert!(half.rotation.abs_diff_eq(Quat::from_rotation_y(1.0), 1e-6), "it tumbles a radian a second");
    }

    #[test]
    fn particles_leave_within_their_cone_and_start_within_their_ball() {
        let mut rng = Rng::new(7);
        let axis = Vec3::new(1.0, 1.0, 0.0).normalize();
        for _ in 0..2000 {
            let dir = rng.in_cone(axis, 60.0);
            assert!((dir.length() - 1.0).abs() < 1e-4);
            assert!(dir.angle_between(axis).to_degrees() <= 30.01, "{dir}");
            assert!(rng.in_ball(0.5).length() <= 0.5 + 1e-5);
        }
        // Every way: some go down.
        assert!((0..200).any(|_| rng.in_cone(Vec3::Y, 360.0).y < -0.5));
        // A line.
        assert!(rng.in_cone(Vec3::X, 0.0).abs_diff_eq(Vec3::X, 1e-5));
        // The same seed, the same particles.
        let def = EffectDef { radius: 0.3, ..EffectDef::default() };
        let a = spawn(&def, Vec3::ONE, Vec3::Y, 0.0, &mut Rng::new(3));
        assert_eq!(a, spawn(&def, Vec3::ONE, Vec3::Y, 0.0, &mut Rng::new(3)));
        assert!((def.lifetime.0..=def.lifetime.1).contains(&a.life) && (def.size.0..=def.size.1).contains(&a.size));
        assert!((def.speed.0..=def.speed.1).contains(&a.velocity.length()));
    }

    #[test]
    fn a_hit_sends_its_particles_the_way_given_level_and_tilted_up() {
        let hit = EffectDef { aim: Aim::Hit, lift: 30.0, ..EffectDef::default() };
        let aim = aim_of(&hit, Some(Vec3::new(0.0, -5.0, 2.0)));
        assert!(aim.abs_diff_eq(Vec3::new(0.0, 0.5, ops::cos(30f32.to_radians())), 1e-5), "{aim}");
        assert_eq!(aim_of(&hit, None), Vec3::Y, "no attacker: up");
        assert_eq!(aim_of(&hit, Some(Vec3::Y)), Vec3::Y, "straight above: up");
        assert_eq!(aim_of(&EffectDef::default(), Some(Vec3::X)), Vec3::Y, "`Up` ignores the hit");
    }

    #[test]
    fn a_full_pool_takes_the_slot_of_its_oldest() {
        let mut pool = Pool::new(3);
        let born = |t: f64| Particle { born: t, ..particle(Vec3::ZERO) };
        assert_eq!([pool.add(born(2.0)), pool.add(born(1.0)), pool.add(born(3.0))], [Some(0), Some(1), Some(2)]);
        assert_eq!((pool.live(), pool.recycled), (3, 0));
        assert_eq!(pool.add(born(4.0)), Some(1), "the one born at 1.0");
        assert_eq!(pool.add(born(5.0)), Some(0), "then the one born at 2.0");
        assert_eq!((pool.live(), pool.started, pool.recycled), (3, 5, 2));
        // Lives of 2 s: at 5.5 the one born at 3.0 is over.
        pool.expire(5.5);
        assert_eq!(pool.slots.iter().map(|s| s.map(|p| p.born)).collect::<Vec<_>>(), [Some(5.0), Some(4.0), None]);
        // The clock went back (a snapshot): what was born after it goes.
        pool.expire(4.5);
        assert_eq!(pool.live(), 1);
        assert_eq!(Pool::new(0).add(born(1.0)), None, "an empty pool takes nothing");
    }
}
