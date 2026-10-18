//! The voxel arena (`content/world/arena.voxel_region.ron`), a voxel region far north of the
//! arena: hills that fall to a sea at the rim, a lake, trees, and in the middle a flat arena with
//! steps, a ledge, a pool, an overhang and a cave under a mound. These are the sandbox's
//! generation steps and checks: the engine runs them by name, with the versions the content
//! names, and every number a step reads is a param of the content. Randomness is the step's
//! `rng()` and `noise()`, so a seed makes the same arena on every machine.
//!
//! The marks: `spawn.0` (the arena's middle), `steps.0`, `ledge.0`, `pool.0`, `overhang.0`,
//! `cave.0` (points of interest) and `post` (a marker beside the arena, where the scenario puts a
//! standing NPC).

use shards::prelude::*;
use shards::voxel::noise::smoothstep;
use shards::voxel::{VoxelAppExt, VoxelGen, AIR};

pub fn plugin(app: &mut App) {
    app.add_voxel_step("island", 1, island)
        .add_voxel_step("lake", 1, lake)
        .add_voxel_step("flat_arena", 1, flat_arena)
        .add_voxel_step("arena_pieces", 1, arena_pieces)
        .add_voxel_step("cave", 1, cave)
        .add_voxel_step("trees", 1, trees)
        .add_voxel_check("arena_flat", arena_flat)
        .add_voxel_check("spawn_on_ground", spawn_on_ground)
        .add_voxel_check("pool_depth", pool_depth)
        .add_voxel_check("cave_open", cave_open);
}

/// A block of the palette by name, or the step's error.
fn block(g: &VoxelGen, name: &str) -> Result<u8, String> {
    g.palette().id(name).ok_or_else(|| format!("the palette has no block `{name}`"))
}

/// The middle column of the volume.
fn middle(g: &VoxelGen) -> IVec2 {
    let s = g.volume().size();
    IVec2::new(s.x as i32 / 2, s.z as i32 / 2)
}

/// The world point on top of cell `c`'s least corner, in the middle of its column.
fn on(g: &VoxelGen, c: IVec3) -> Vec3 {
    g.volume().corner(c) + Vec3::new(0.5, 0.0, 0.5) * g.volume().block()
}

#[derive(Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
struct IslandParams {
    /// The height of the land before the noise, in layers.
    base: f32,
    /// The broad hills: amplitude (layers) and frequency (per cell).
    hills: f32,
    hills_frequency: f32,
    /// The detail on them.
    detail: f32,
    detail_frequency: f32,
    /// From this share of the half-width out, the land falls to the sea, by `falloff` layers at
    /// the rim.
    falloff_from: f32,
    falloff: f32,
    /// Columns at most this high are shore; at least `peak_from`, peaks.
    shore_below: u32,
    peak_from: u32,
    /// The blocks: the top of a column, under it (`soil_depth` layers), and below that.
    ground: String,
    soil: String,
    rock: String,
    shore: String,
    peak: String,
    soil_depth: u32,
}

/// Hills that fall to the rim, layered rock, soil and a top that is shore, ground or peak by
/// height.
fn island(g: &mut VoxelGen) -> Result<(), String> {
    let p: IslandParams = g.params()?;
    let (ground, soil, rock, shore, peak) = (block(g, &p.ground)?, block(g, &p.soil)?, block(g, &p.rock)?, block(g, &p.shore)?, block(g, &p.peak)?);
    let hills = g.noise(1, p.hills_frequency);
    let mut detail = g.noise(2, p.detail_frequency);
    detail.octaves = 3;
    let size = g.volume().size();
    let half = Vec2::new(size.x as f32, size.z as f32) * 0.5;
    let max_h = size.y.saturating_sub(7).max(3) as f32;
    for x in 0..size.x as i32 {
        for z in 0..size.z as i32 {
            let (fx, fz) = (x as f32, z as f32);
            let mut n = hills.sample(fx, fz) * p.hills + detail.sample(fx, fz) * p.detail;
            let edge = (Vec2::new(fx, fz) - half).length() / half.x;
            let fall = (edge - p.falloff_from).max(0.0);
            n -= fall * fall * p.falloff;
            let h = (p.base + n).clamp(2.0, max_h) as u32;
            let top = if h <= p.shore_below {
                shore
            } else if h >= p.peak_from {
                peak
            } else {
                ground
            };
            for y in 0..h {
                let id = if y + 1 == h {
                    top
                } else if y + 1 + p.soil_depth >= h {
                    if top == shore { shore } else { soil }
                } else {
                    rock
                };
                g.volume_mut().set(IVec3::new(x, y as i32, z), id);
            }
            g.volume_mut().set_surface(x, z, h);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
struct LakeParams {
    /// The lake's middle, in cells from the volume's middle.
    at: [f32; 2],
    /// Its half-widths along x and z, cells.
    radius: [f32; 2],
    /// How far its middle goes below `level`, layers; it is never lower than layer 2.
    level: u32,
    depth: f32,
    /// The lake bed.
    bed: String,
}

/// A bowl: the columns of an ellipse lowered toward its middle.
fn lake(g: &mut VoxelGen) -> Result<(), String> {
    let p: LakeParams = g.params()?;
    let bed = block(g, &p.bed)?;
    let c = middle(g).as_vec2() + Vec2::from(p.at);
    let r = Vec2::from(p.radius);
    for x in (c.x - r.x - 2.0) as i32..(c.x + r.x + 2.0) as i32 {
        for z in (c.y - r.y - 2.0) as i32..(c.y + r.y + 2.0) as i32 {
            let (dx, dz) = ((x as f32 - c.x) / r.x, (z as f32 - c.y) / r.y);
            let d = dx * dx + dz * dz;
            if d < 1.0 {
                let floor = (p.level as f32 - p.depth * (1.0 - d)).round().max(2.0) as u32;
                g.volume_mut().lower_column(x, z, floor, bed);
            }
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
struct FlatArenaParams {
    /// The floor's height (layers: the floor's top is this layer's bottom).
    height: u32,
    /// Flat within `radius` cells of the middle, blending into the hills over `blend` more.
    radius: f32,
    blend: f32,
    top: String,
    fill: String,
}

/// The middle flattened at `height`, its rim blending into the hills.
fn flat_arena(g: &mut VoxelGen) -> Result<(), String> {
    let p: FlatArenaParams = g.params()?;
    let (top, fill) = (block(g, &p.top)?, block(g, &p.fill)?);
    let c = middle(g).as_vec2();
    let reach = p.radius + p.blend;
    for x in (c.x - reach - 1.0) as i32..=(c.x + reach + 1.0) as i32 {
        for z in (c.y - reach - 1.0) as i32..=(c.y + reach + 1.0) as i32 {
            let d = (Vec2::new(x as f32, z as f32) - c).length();
            if d > reach {
                continue;
            }
            let h = g.volume().surface(x, z) as f32;
            let t = smoothstep((d - p.radius) / p.blend.max(1e-3));
            let want = (p.height as f32 + (h - p.height as f32) * t).round() as u32;
            g.volume_mut().lower_column(x, z, want, top);
            g.volume_mut().raise_column(x, z, want, fill, top);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
struct PiecesParams {
    /// The arena floor's height, as `flat_arena`'s.
    height: u32,
    /// The ledge's height, layers: more than a mantle from the floor, so it takes a jump.
    ledge: u32,
    stone: String,
    brick: String,
    sand: String,
    water: String,
    glow: String,
}

/// The arena's test pieces on its floor: four steps of one block going east, a ledge `ledge`
/// layers high, a pool three blocks deep, a pillar with a slab reaching out 3 m up (and a glowing
/// block on it), and the marks of each, the spawn in the middle and a post beside the arena.
fn arena_pieces(g: &mut VoxelGen) -> Result<(), String> {
    let p: PiecesParams = g.params()?;
    let (stone, brick, sand, water, glow) = (block(g, &p.stone)?, block(g, &p.brick)?, block(g, &p.sand)?, block(g, &p.water)?, block(g, &p.glow)?);
    let IVec2 { x: cx, y: cz } = middle(g);
    let ah = p.height as i32;
    let v = g.volume_mut();
    for s in 0..4 {
        for z in cz - 7..cz - 5 {
            for x in (cx + 4 + s)..(cx + 8) {
                v.set(IVec3::new(x, ah + s, z), stone);
            }
        }
    }
    v.fill(IVec3::new(cx - 9, ah, cz - 8), IVec3::new(cx - 4, ah + p.ledge as i32, cz - 3), brick);
    for x in cx + 3..cx + 9 {
        for z in cz + 3..cz + 9 {
            v.lower_column(x, z, p.height - 3, sand);
            v.fill(IVec3::new(x, ah - 3, z), IVec3::new(x + 1, ah, z + 1), water);
        }
    }
    v.fill(IVec3::new(cx - 9, ah, cz + 4), IVec3::new(cx - 7, ah + 4, cz + 6), stone);
    v.fill(IVec3::new(cx - 7, ah + 3, cz + 3), IVec3::new(cx - 3, ah + 4, cz + 7), stone);
    v.set(IVec3::new(cx - 8, ah + 4, cz + 5), glow);
    let marks = [
        ("spawn.0", IVec3::new(cx, ah, cz)),
        ("steps.0", IVec3::new(cx + 3, ah, cz - 6)),
        ("ledge.0", IVec3::new(cx - 3, ah, cz - 6)),
        ("pool.0", IVec3::new(cx + 5, ah, cz + 5)),
        ("overhang.0", IVec3::new(cx - 5, ah, cz + 5)),
        ("post", IVec3::new(cx + 12, ah, cz)),
    ];
    for (name, cell) in marks {
        let at = on(g, cell);
        g.mark(name, at);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
struct CaveParams {
    /// The arena floor's height.
    height: u32,
    /// The mound west of the arena: how many layers over the floor, and its blocks.
    mound: u32,
    rock: String,
    top: String,
    /// The tunnel's radius, cells: through the mound at floor height, open at both ends.
    radius: f32,
}

/// A mound of rock at the arena's west rim and a tunnel through it, with `cave.0` in it.
fn cave(g: &mut VoxelGen) -> Result<(), String> {
    let p: CaveParams = g.params()?;
    let (rock, top) = (block(g, &p.rock)?, block(g, &p.top)?);
    let IVec2 { x: cx, y: cz } = middle(g);
    for x in cx - 30..cx - 18 {
        for z in cz - 6..cz + 6 {
            g.volume_mut().raise_column(x, z, p.height + p.mound, rock, top);
        }
    }
    let y = p.height as f32 + 1.5;
    let (a, b) = (Vec3::new(cx as f32 - 31.0, y, cz as f32 + 0.5), Vec3::new(cx as f32 - 17.0, y, cz as f32 + 0.5));
    g.volume_mut().tube(a, b, p.radius, AIR);
    let at = on(g, IVec3::new(cx - 24, p.height as i32, cz));
    g.mark("cave.0", at);
    Ok(())
}

#[derive(Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
struct TreesParams {
    /// How many, at most (a place that does not fit is tried again, up to three times as many).
    count: u32,
    /// None closer than this to the middle (cells), none on a column at most `dry_above` high.
    clear: f32,
    dry_above: u32,
    /// The ground they grow on, the trunk and the crown.
    ground: String,
    trunk: String,
    leaves: String,
}

/// Trees on the ground, away from the arena: a trunk of 4 to 6 blocks and a ball of leaves.
fn trees(g: &mut VoxelGen) -> Result<(), String> {
    let p: TreesParams = g.params()?;
    let (ground, trunk, leaves) = (block(g, &p.ground)?, block(g, &p.trunk)?, block(g, &p.leaves)?);
    let size = g.volume().size();
    let c = middle(g).as_vec2();
    let mut rng = g.rng();
    let (mut planted, mut tries) = (0, 0);
    while planted < p.count && tries < p.count * 3 {
        tries += 1;
        let x = 4 + rng.below(size.x - 8) as i32;
        let z = 4 + rng.below(size.z - 8) as i32;
        let h = g.volume().surface(x, z) as i32;
        if (Vec2::new(x as f32, z as f32) - c).length() < p.clear
            || h <= p.dry_above as i32
            || g.volume().get(IVec3::new(x, h - 1, z)) != ground
            || g.volume().get(IVec3::new(x, h, z)) != AIR
        {
            continue;
        }
        let tall = 4 + rng.below(3) as i32;
        let v = g.volume_mut();
        for y in h..h + tall {
            v.set(IVec3::new(x, y, z), trunk);
        }
        let crown = IVec3::new(x, h + tall, z);
        for dx in -2..=2 {
            for dz in -2..=2 {
                for dy in -2..=2 {
                    let at = crown + IVec3::new(dx, dy, dz);
                    if dx * dx + dy * dy + dz * dz <= 5 && v.get(at) == AIR {
                        v.set(at, leaves);
                    }
                }
            }
        }
        planted += 1;
    }
    Ok(())
}

/// The cell of mark `name`, or the check's error.
fn mark_cell(g: &VoxelGen, name: &str) -> Result<IVec3, String> {
    g.find_mark(name).map(|at| g.volume().cell_at(at)).ok_or_else(|| format!("no `{name}` mark"))
}

fn solid(g: &VoxelGen, c: IVec3) -> bool {
    g.palette().solid(g.volume().get(c))
}

/// The floor around the spawn is flat and clear: the top solid block of every column within 2
/// cells of it is the floor's.
fn arena_flat(g: &VoxelGen) -> Result<(), String> {
    let s = mark_cell(g, "spawn.0")?;
    for x in s.x - 2..=s.x + 2 {
        for z in s.z - 2..=s.z + 2 {
            let h = g.volume().column_top(x, z, |id| g.palette().solid(id)) as i32;
            if h != s.y {
                return Err(format!("column ({x}, {z}) stands at layer {h}, the spawn's floor at {}", s.y));
            }
        }
    }
    Ok(())
}

/// The spawn stands on a solid block with two air cells above it.
fn spawn_on_ground(g: &VoxelGen) -> Result<(), String> {
    let s = mark_cell(g, "spawn.0")?;
    if !solid(g, s - IVec3::Y) {
        return Err(format!("nothing solid under the spawn at cell {s}"));
    }
    if (0..2).any(|dy| g.volume().get(s + IVec3::Y * dy) != AIR) {
        return Err(format!("the spawn at cell {s} is not in the open"));
    }
    Ok(())
}

/// The pool holds three layers of water over a solid bed.
fn pool_depth(g: &VoxelGen) -> Result<(), String> {
    let c = mark_cell(g, "pool.0")?;
    for dy in 1..=3 {
        if !g.palette().liquid(g.volume().get(c - IVec3::Y * dy)) {
            return Err(format!("the pool is not water {dy} below its top at cell {c}"));
        }
    }
    if !solid(g, c - IVec3::Y * 4) {
        return Err(format!("the pool at cell {c} has no bed"));
    }
    Ok(())
}

/// The cave is open: a floor, two cells of air, and rock overhead.
fn cave_open(g: &VoxelGen) -> Result<(), String> {
    let c = mark_cell(g, "cave.0")?;
    if !solid(g, c - IVec3::Y) || (0..2).any(|dy| g.volume().get(c + IVec3::Y * dy) != AIR) {
        return Err(format!("the cave at cell {c} has no floor or is not open"));
    }
    if !(2..8).any(|dy| solid(g, c + IVec3::Y * dy)) {
        return Err(format!("the cave at cell {c} has no rock over it"));
    }
    Ok(())
}
