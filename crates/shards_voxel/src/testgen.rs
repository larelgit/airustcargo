//! A neutral test world for the spike and the sandbox: noise hills that fall to a sea at the
//! rim, a lake, trees, and in the middle a flat arena with steps, a ledge, a pool, an overhang
//! and a cave. Every random choice comes from [`DetRng`] keyed by the seed and the step's name,
//! every height from [`Noise`]: the same seed is the same world on every machine.

use ai_core::DetRng;
use bevy::prelude::*;

use crate::noise::Noise;
use crate::palette::VoxelPalette;
use crate::volume::{VoxelVolume, AIR};

/// The numbers of the test world.
#[derive(Clone, Debug)]
pub struct TestWorld {
    pub size: UVec3,
    pub chunk: UVec3,
    /// Columns whose surface is at or below this are under water (the water's surface is 0.95
    /// above it, as in a lake that does not quite reach the next layer).
    pub water_level: u32,
    /// Hill and detail noise amplitude (layers).
    pub hills: f32,
    pub detail: f32,
    pub detail_frequency: f32,
    pub trees: u32,
    /// Height of the arena floor.
    pub arena_height: u32,
    pub arena_radius: f32,
}

impl Default for TestWorld {
    fn default() -> Self {
        Self {
            size: UVec3::new(256, 32, 256),
            chunk: UVec3::new(32, 32, 32),
            water_level: 8,
            hills: 7.0,
            detail: 2.5,
            detail_frequency: 0.07,
            trees: 170,
            arena_height: 12,
            arena_radius: 16.0,
        }
    }
}

impl TestWorld {
    /// Much rougher hills: more detail, at a higher frequency (the worst case for merged colliders).
    pub fn rough() -> Self {
        Self { detail: 6.0, detail_frequency: 0.2, ..Self::default() }
    }

    pub fn arena_center(&self) -> IVec2 {
        IVec2::new(self.size.x as i32 / 2, self.size.z as i32 / 2)
    }

    /// The water surface's height in cells.
    pub fn water_surface(&self) -> f32 {
        self.water_level as f32 + 0.95
    }

    /// The world for `seed`, with `palette`'s blocks (it must have the test palette's names).
    pub fn build(&self, palette: &VoxelPalette, seed: u64) -> VoxelVolume {
        let id = |name: &str| palette.id(name).unwrap_or_else(|| panic!("the palette has no `{name}`"));
        let (grass, dirt, stone, sand, snow, wood, leaves, brick, glow, water) =
            (id("grass"), id("dirt"), id("stone"), id("sand"), id("snow"), id("wood"), id("leaves"), id("brick"), id("glow"), id("water"));
        let half = Vec3::new(self.size.x as f32, 0.0, self.size.z as f32) * 0.5;
        // The arena centre at the world origin, its floor at y = 0.
        let origin = Vec3::new(-half.x, -(self.arena_height as f32), -half.z);
        let mut v = VoxelVolume::new(self.size, 1.0, origin, self.chunk).expect("the test sizes are whole chunks");

        // Hills.
        let base = Noise::new(DetRng::from_keys(&[seed, 1]).next_u64(), 0.016);
        let mut detail = Noise::new(DetRng::from_keys(&[seed, 2]).next_u64(), self.detail_frequency);
        detail.octaves = 3;
        let center = self.arena_center().as_vec2();
        let max_h = self.size.y.saturating_sub(7).max(3);
        for x in 0..self.size.x as i32 {
            for z in 0..self.size.z as i32 {
                let (fx, fz) = (x as f32, z as f32);
                let mut n = base.sample(fx, fz) * self.hills + detail.sample(fx, fz) * self.detail;
                let edge = (Vec2::new(fx, fz) - Vec2::new(half.x, half.z)).length() / half.x;
                let fall = (edge - 0.55).max(0.0);
                n -= fall * fall * 60.0;
                let mut h = (10.0 + n).clamp(2.0, max_h as f32) as u32;
                // The arena is flat and blends into the hills.
                let d = (Vec2::new(fx, fz) - center).length();
                let t = smoothstep(self.arena_radius, self.arena_radius + 14.0, d);
                h = (self.arena_height as f32 + (h as f32 - self.arena_height as f32) * t).round() as u32;
                let top = if h <= self.water_level + 1 {
                    sand
                } else if h >= 21 {
                    snow
                } else {
                    grass
                };
                for y in 0..h {
                    let block = if y + 1 == h {
                        top
                    } else if y + 4 >= h {
                        if top == sand { sand } else { dirt }
                    } else {
                        stone
                    };
                    v.set(IVec3::new(x, y as i32, z), block);
                }
                v.set_surface(x, z, h);
            }
        }

        // A lake: a bowl north-west of the arena.
        let lake = center + Vec2::new(-60.0, -50.0);
        for x in (lake.x as i32 - 20)..(lake.x as i32 + 20) {
            for z in (lake.y as i32 - 14)..(lake.y as i32 + 14) {
                let d = sq((x as f32 - lake.x) / 18.0) + sq((z as f32 - lake.y) / 12.0);
                if d < 1.0 {
                    let floor = (self.water_level as f32 - 4.0 * (1.0 - d)).round().max(2.0) as u32;
                    v.lower_column(x, z, floor, sand);
                }
            }
        }

        // The arena's test pieces, all on the arena floor (layer `ah`, the floor's top at `ah`).
        let (cx, cz) = (center.x as i32, center.y as i32);
        let ah = self.arena_height as i32;
        // Steps: four steps of one block going east, two wide.
        for s in 0..4 {
            for z in cz - 7..cz - 5 {
                for x in (cx + 4 + s)..(cx + 8) {
                    v.set(IVec3::new(x, ah + s, z), stone);
                }
            }
        }
        // A ledge: a 3 m block to climb onto or drop from.
        v.fill(IVec3::new(cx - 9, ah, cz - 8), IVec3::new(cx - 4, ah + 3, cz - 3), brick);
        // A pool: 6 x 6, three blocks deep, filled to just below the floor.
        for x in cx + 3..cx + 9 {
            for z in cz + 3..cz + 9 {
                v.lower_column(x, z, self.arena_height - 3, sand);
                v.fill(IVec3::new(x, ah - 3, z), IVec3::new(x + 1, ah, z + 1), water);
            }
        }
        // An overhang: a pillar with a slab reaching out at 2.2 m.
        v.fill(IVec3::new(cx - 9, ah, cz + 4), IVec3::new(cx - 7, ah + 4, cz + 6), stone);
        v.fill(IVec3::new(cx - 7, ah + 3, cz + 3), IVec3::new(cx - 3, ah + 4, cz + 7), stone);
        // A glowing marker on the pillar.
        v.set(IVec3::new(cx - 8, ah + 4, cz + 5), glow);

        // A cave: a mound of rock at the arena's west rim, 6 m over the floor, and a tunnel through
        // it at floor height, 3 m high, open at both ends.
        for x in cx - 30..cx - 18 {
            for z in cz - 6..cz + 6 {
                v.raise_column(x, z, self.arena_height + 6, stone, grass);
            }
        }
        let a = Vec3::new(center.x - 31.0, ah as f32 + 1.5, center.y + 0.5);
        let b = Vec3::new(center.x - 17.0, ah as f32 + 1.5, center.y + 0.5);
        v.tube(a, b, 1.6, AIR);

        // Trees: a wooden trunk and a ball of leaves, on grass, away from the arena.
        let mut rng = DetRng::from_keys(&[seed, 3]);
        let (mut planted, mut attempts) = (0, 0);
        while planted < self.trees && attempts < self.trees * 3 {
            attempts += 1;
            let x = 4 + rng.below(self.size.x - 8) as i32;
            let z = 4 + rng.below(self.size.z - 8) as i32;
            let h = v.surface(x, z) as i32;
            if (Vec2::new(x as f32, z as f32) - center).length() < self.arena_radius + 14.0
                || h <= self.water_level as i32 + 1
                || v.get(IVec3::new(x, h - 1, z)) != grass
                || v.get(IVec3::new(x, h, z)) != AIR
            {
                continue;
            }
            let trunk = 4 + rng.below(3) as i32;
            for y in h..h + trunk {
                v.set(IVec3::new(x, y, z), wood);
            }
            let crown = IVec3::new(x, h + trunk, z);
            for dx in -2..=2 {
                for dz in -2..=2 {
                    for dy in -2..=2 {
                        let p = crown + IVec3::new(dx, dy, dz);
                        if dx * dx + dy * dy + dz * dz <= 5 && v.get(p) == AIR {
                            v.set(p, leaves);
                        }
                    }
                }
            }
            planted += 1;
        }

        // Water: every column whose natural ground is at or below the water level, up to it
        // (the lake and the sea at the rim). A tunnel under a lake bed stays dry.
        for x in 0..self.size.x as i32 {
            for z in 0..self.size.z as i32 {
                let s = v.surface(x, z);
                for y in s..=self.water_level {
                    let p = IVec3::new(x, y as i32, z);
                    if v.get(p) == AIR {
                        v.set(p, water);
                    }
                }
            }
        }
        v.compact();
        v
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn sq(x: f32) -> f32 {
    x * x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::test_palette;

    #[test]
    fn the_same_seed_builds_the_same_world() {
        let small = TestWorld { size: UVec3::new(64, 32, 64), trees: 10, arena_radius: 8.0, ..TestWorld::default() };
        let p = test_palette();
        let a = small.build(&p, 5);
        assert_eq!(a.hash(), small.build(&p, 5).hash());
        assert_ne!(a.hash(), small.build(&p, 6).hash());
    }

    #[test]
    fn the_arena_has_its_pieces() {
        let w = TestWorld::default();
        let p = test_palette();
        let v = w.build(&p, 1);
        let c = w.arena_center();
        let ah = w.arena_height;
        assert_eq!(v.surface(c.x, c.y), ah, "a flat floor at the arena height");
        assert_eq!(v.surface(c.x + 5, c.y + 5), ah - 3, "the pool is three blocks deep");
        assert_eq!(v.get(IVec3::new(c.x + 5, ah as i32 - 1, c.y + 5)), p.id("water").unwrap(), "and full");
        assert_eq!(v.top(c.x - 6, c.y - 5, |id| p.solid(id)), ah + 3, "the ledge is 3 m tall");
        let under = IVec3::new(c.x - 5, ah as i32 + 1, c.y + 5);
        assert_eq!(v.get(under), AIR, "air under the overhang");
        assert!(p.solid(v.get(under + IVec3::Y * 2)), "and the slab above it");
        let cave = IVec3::new(c.x - 24, ah as i32, c.y);
        assert!(p.solid(v.get(cave - IVec3::Y)) && v.get(cave) == AIR && v.get(cave + IVec3::Y * 2) == AIR, "a cave with a floor at the arena's");
        assert!(p.solid(v.get(cave + IVec3::Y * 3)) && v.top(cave.x, cave.z, |id| p.solid(id)) == ah + 6, "under 3 m of rock");
        let world = v.corner(IVec3::new(c.x, ah as i32, c.y));
        assert_eq!(world, Vec3::ZERO, "the arena floor's centre is the world origin");
    }
}
