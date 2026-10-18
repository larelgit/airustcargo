//! The G1 spike's measurements: build cost, memory and per-query cost of the three collision
//! candidates on the test world, how they agree, an in-memory nav bake over each, the mesher and
//! generation (`voxel_spike_measurements`); sixteen real controllers walking each
//! (`controller_walks_alike`).
//!
//! - (c) `boxes`: column runs merged into cuboid colliders, one entity each (physics as it is).
//! - (a) `voxels`: the solid cells as one parry `Voxels` collider (`Geom::Voxels`, spike).
//! - (b) `field`: a grid of solid cells the physics world asks besides its colliders (spike).
//!
//! Water is the same for all three: merged water boxes and four sea boxes, sensor volumes.
//!
//! Run in an optimised build. The recorded numbers were taken in the dev profile with
//! `[profile.dev.package.<crate>] opt-level = 3` added for shards_motion, shards_voxel and ai_core
//! (their dependencies are at 3 already; the dev profile builds workspace crates at 1):
//! `cargo test -p shards_voxel --test spike -- --ignored --nocapture`.

use std::hint::black_box;
use std::time::Instant;

use ai_core::{AiId, DetRng};
use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use shards_motion::perception::{Probe, Terrain};
use shards_motion::physics::{sync_physics, VoxelField};
use shards_npc::nav::bake::spike_bake;
use shards_npc::nav::NavSettings;
use shards_motion::{
    Body, CharacterParams, Collider, ColliderId, ColliderIds, CollisionLayers, LayerMask, PhysicsWorld, QueryFilter, Shape,
    Volume, VolumeKind,
};
use shards_voxel::{column_boxes, greedy_boxes, mesh_all, test_palette, water_boxes, Meshing, TestWorld, VoxelPalette, VoxelVolume};

const ROUNDS: usize = 5;

/// The sixteen moving bodies.
#[derive(Component)]
struct Walker;
const REPS: usize = 150;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cand {
    Boxes,
    Voxels,
    Field,
}

impl Cand {
    const ALL: [Cand; 3] = [Cand::Boxes, Cand::Voxels, Cand::Field];

    fn name(self) -> &'static str {
        match self {
            Cand::Boxes => "(c) boxes",
            Cand::Voxels => "(a) voxels",
            Cand::Field => "(b) field",
        }
    }
}

/// A world of one candidate, ready for queries.
struct Built {
    world: World,
    sync: SystemId,
    /// Static solid colliders (entities) the candidate spawned.
    solids: usize,
    /// Water volumes.
    waters: usize,
    /// Extra memory the candidate's collision data takes, bytes (estimate, see `memory`).
    bytes: usize,
    terrain: Entity,
}

fn med(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn liquid(p: &VoxelPalette) -> impl Fn(u8) -> bool + '_ {
    move |id| p.def(id).is_some_and(|d| d.liquid)
}

/// Water boxes (`water_boxes`: liquid runs reaching down through the ground, top 0.05 below the
/// run's top) and four sea boxes from the grid's edge out to 3 km, from the sea floor (layer 2) to
/// the water surface.
fn water(v: &VoxelVolume, p: &VoxelPalette, w: &TestWorld) -> Vec<(Vec3, Vec3)> {
    let mut out: Vec<(Vec3, Vec3)> = water_boxes(v, liquid(p), |id| p.solid(id))
        .into_iter()
        .map(|b| {
            let (lo, hi) = (v.corner(b.min), v.corner(b.max) - Vec3::Y * 0.05);
            ((lo + hi) * 0.5, (hi - lo) * 0.5)
        })
        .collect();
    let (lo, hi) = (v.corner(IVec3::new(0, 2, 0)), v.corner(IVec3::new(w.size.x as i32, 0, w.size.z as i32)));
    let top = v.origin().y + w.water_surface();
    let reach = 3000.0;
    for (a, b) in [
        (Vec3::new(lo.x - reach, lo.y, lo.z - reach), Vec3::new(hi.x + reach, top, lo.z)),
        (Vec3::new(lo.x - reach, lo.y, hi.z), Vec3::new(hi.x + reach, top, hi.z + reach)),
        (Vec3::new(lo.x - reach, lo.y, lo.z), Vec3::new(lo.x, top, hi.z)),
        (Vec3::new(hi.x, lo.y, lo.z), Vec3::new(hi.x + reach, top, hi.z)),
    ] {
        out.push(((a + b) * 0.5, (b - a) * 0.5));
    }
    out
}

/// Sixteen bodies: eight on the arena among its test pieces, eight on the hills to the east.
fn bodies(v: &VoxelVolume, p: &VoxelPalette, w: &TestWorld) -> Vec<Vec3> {
    let mut rng = DetRng::from_keys(&[0xb0d1e5]);
    let c = w.arena_center();
    let mut out = Vec::new();
    while out.len() < 16 {
        let (center, r) = if out.len() < 8 { (c, 10.0) } else { (c + IVec2::new(60, 10), 12.0) };
        let x = center.x + rng.range(-r, r) as i32;
        let z = center.y + rng.range(-r, r) as i32;
        let top = v.top(x, z, |id| p.solid(id));
        if top == 0 || v.get(IVec3::new(x, top as i32, z)) != 0 {
            continue;
        }
        let feet = v.corner(IVec3::new(x, top as i32, z)) + Vec3::new(rng.range(0.1, 0.9), 0.02, rng.range(0.1, 0.9));
        out.push(feet);
    }
    out
}

/// A world with the shared parts (physics resources, water, bodies) and the candidate's terrain.
fn build(cand: Cand, v: &VoxelVolume, p: &VoxelPalette, w: &TestWorld, feet: &[Vec3]) -> (Built, f64, f64) {
    let mut world = World::new();
    world.init_resource::<PhysicsWorld>();
    world.init_resource::<ColliderIds>();
    let started = Instant::now();
    let solids;
    let mut bytes = 0;
    let terrain = world.spawn(Name::new("terrain")).id();
    match cand {
        Cand::Boxes => {
            let boxes = column_boxes(v, |id| p.solid(id));
            solids = boxes.len();
            world.spawn_batch(boxes.into_iter().map(|b| {
                let (center, half) = b.world(v);
                (Collider::new(Shape::Cuboid { half }), Transform::from_translation(center))
            }));
        }
        Cand::Voxels => {
            let s = v.size().as_ivec3();
            let mut cells = Vec::new();
            for x in 0..s.x {
                for z in 0..s.z {
                    for y in 0..s.y {
                        let c = IVec3::new(x, y, z);
                        if p.solid(v.get(c)) {
                            cells.push(c);
                        }
                    }
                }
            }
            bytes = PhysicsWorld::spike_voxels_bytes(&cells);
            let id = world.resource::<ColliderIds>().peek();
            world.resource_mut::<ColliderIds>().restart_at(ColliderId(id.0 + 1));
            world.resource_mut::<PhysicsWorld>().spike_insert_voxels(terrain, id, v.origin(), v.block(), &cells);
            solids = 1;
        }
        Cand::Field => {
            let field = VoxelField::new(v.origin(), v.block(), v.size(), |c| p.solid(v.get(c)));
            bytes = field.memory();
            let id = world.resource::<ColliderIds>().peek();
            world.resource_mut::<ColliderIds>().restart_at(ColliderId(id.0 + 1));
            world.resource_mut::<PhysicsWorld>().spike_insert_field(terrain, id, field);
            solids = 1;
        }
    }
    let sync = world.register_system(sync_physics);
    world.run_system(sync).unwrap();
    let terrain_ms = started.elapsed().as_secs_f64() * 1e3;
    let started = Instant::now();
    let waters = water(v, p, w);
    let n_water = waters.len();
    world.spawn_batch(waters.into_iter().map(|(center, half)| {
        (Collider::new(Shape::Cuboid { half }).with_layers(CollisionLayers::VOLUME), Volume { kind: VolumeKind::Water }, Transform::from_translation(center))
    }));
    for f in feet {
        world.spawn((
            Collider::new(Shape::capsule(0.4, 1.8)).with_layers(CollisionLayers::CHARACTER),
            Body::Kinematic,
            Walker,
            Transform::from_translation(*f + Vec3::Y * 0.9),
        ));
    }
    world.run_system(sync).unwrap();
    let water_ms = started.elapsed().as_secs_f64() * 1e3;
    if cand == Cand::Boxes {
        bytes = memory(&world);
    }
    (Built { world, sync, solids, waters: n_water, bytes, terrain }, terrain_ms, water_ms)
}

/// Bytes the box colliders take: their components in the ECS, and their slots and grid entries in
/// the physics world (an estimate from the types' sizes; hash map and archetype overheads aside).
fn memory(world: &World) -> usize {
    let physics = world.resource::<PhysicsWorld>();
    let per_entity = size_of::<Collider>()
        + size_of::<Transform>()
        + size_of::<GlobalTransform>()
        + size_of::<Body>()
        + size_of::<ColliderId>()
        + size_of::<Entity>() * 2;
    let solids = physics.colliders().filter(|c| c.volume.is_none() && c.body == Body::Static).count();
    // A slot (`ColliderInfo` and its cells), its place in `order` and `ids`, and about four grid
    // entries (a u32 each, in buckets of a hash map).
    let per_slot = size_of::<shards_motion::ColliderInfo>() + 24 + 8 + 16 + 4 * 24;
    solids * (per_entity + per_slot)
}

/// What a tick asks of the physics world per body: the controller (walk cast, ground cast,
/// surface ray, step-up top ray, depenetration, volumes), perception (terrain probe, blocker
/// top, sight) and the camera.
type Row = (&'static str, fn(&PhysicsWorld, &[Vec3], usize) -> usize);

fn rows() -> Vec<Row> {
    fn walk_dir(b: usize) -> Vec3 {
        let a = b as f32 * 2.399;
        Vec3::new(ops::cos(a), 0.0, ops::sin(a))
    }
    fn walls() -> QueryFilter {
        QueryFilter::new(LayerMask::WORLD)
    }
    vec![
        ("walk cast 0.14 m (slide)", |w, f, b| usize::from(w.spike_sweep(&Shape::capsule(0.4, 1.8), f[b] + Vec3::Y * 0.9, walk_dir(b), 0.14, &walls()).is_some())),
        ("ground cast 0.5 m down", |w, f, b| usize::from(w.spike_sweep(&Shape::capsule(0.41, 1.82), f[b] + Vec3::Y * 0.9, Vec3::NEG_Y, 0.5, &walls()).is_some())),
        ("ray 0.1 m down (surface)", |w, f, b| usize::from(w.raycast(f[b] + Vec3::new(0.42, 0.05, 0.0), Dir3::NEG_Y, 0.1, &walls()).is_some())),
        ("ray 2.6 m down (step top)", |w, f, b| usize::from(w.raycast(f[b] + walk_dir(b) * 0.45 + Vec3::Y * 1.7, Dir3::NEG_Y, 2.6, &walls()).is_some())),
        ("contacts at skin (depenetrate)", |w, f, b| w.spike_closer_than(&Shape::capsule(0.4, 1.8), f[b] + Vec3::Y * 0.9, &walls(), 0.02).len()),
        ("volumes touching + water", |w, f, b| {
            w.spike_volumes_touching(&Shape::capsule(0.4, 1.8), f[b] + Vec3::Y * 0.9) + w.volumes_at(f[b] + Vec3::Y * 0.05, None).len()
        }),
        ("capsule overlap (room)", |w, f, b| {
            w.overlaps(&Shape::capsule(0.4, 1.45), f[b] + Vec3::Y * (0.04 + 0.35 + 0.725), Quat::IDENTITY, &walls()).len()
        }),
        ("ray 61 m down (probe floor)", |w, f, b| usize::from(w.raycast(f[b] + Vec3::new(0.7, 1.7, 0.0), Dir3::NEG_Y, 61.7, &walls()).is_some())),
        ("ray to a body 5 on (sight)", |w, f, b| {
            let (from, to) = (f[b] + Vec3::Y * 1.6, f[(b + 5) % f.len()] + Vec3::Y * 1.6);
            usize::from(w.raycast(from, Dir3::new(to - from).unwrap(), (to - from).length(), &walls()).is_some())
        }),
        ("sphere cast 4 m (camera)", |w, f, b| {
            let dir = Dir3::new(-walk_dir(b) * 0.9 + Vec3::Y * 0.4).unwrap();
            usize::from(w.shapecast(&Shape::sphere(0.2), f[b] + Vec3::Y * 1.6, Quat::IDENTITY, dir, 4.0, &walls()).is_some())
        }),
        ("Terrain::probe (step to 0.5 m)", |w, f, b| {
            let params = CharacterParams::default();
            let t = Terrain::new(w, &params, &[]);
            let at = f[b] + walk_dir(b) * 0.5;
            usize::from(matches!(t.probe(at.x, at.z, f[b].y), Probe::Floor { .. }))
        }),
        ("Terrain::blocker_top (1 m on)", |w, f, b| {
            let params = CharacterParams::default();
            let t = Terrain::new(w, &params, &[]);
            let at = f[b] + walk_dir(b);
            usize::from(t.blocker_top(at.x, at.z, f[b].y).is_some())
        }),
    ]
}

/// The controller's share of a tick (two slide casts, a ground cast, a surface ray, a contact
/// query, the volumes), by row index.
const TICK: [(usize, f64); 5] = [(0, 2.0), (1, 1.0), (2, 1.0), (4, 1.0), (5, 1.0)];

fn time_rows(built: &Built, feet: &[Vec3], rows: &[Row]) -> Vec<f64> {
    let w = built.world.resource::<PhysicsWorld>();
    rows.iter()
        .map(|(_, q)| {
            let started = Instant::now();
            let mut sum = 0;
            for _ in 0..REPS {
                for b in 0..feet.len() {
                    sum += q(w, feet, b);
                }
            }
            black_box(sum);
            started.elapsed().as_secs_f64() * 1e6 / (REPS * feet.len()) as f64
        })
        .collect()
}

/// Per-tick costs that do not depend on bodies: the physics sync (16 bodies moved; it walks every
/// collider to find the kinematic ones), the navmesh's "did the solids change" scan (every static
/// solid and water volume, per body class: two classes), the snapshot check over static colliders.
fn time_fixed(built: &mut Built) -> [f64; 3] {
    let n = 60;
    let started = Instant::now();
    for i in 0..n {
        let dx = if i % 2 == 0 { 0.01 } else { -0.01 };
        let mut q = built.world.query_filtered::<&mut Transform, With<Walker>>();
        for mut tf in q.iter_mut(&mut built.world) {
            tf.translation.x += dx;
        }
        built.world.run_system(built.sync).unwrap();
    }
    let sync = started.elapsed().as_secs_f64() * 1e6 / n as f64;
    let w = built.world.resource::<PhysicsWorld>();
    let started = Instant::now();
    let mut sum = 0;
    for _ in 0..n {
        for _class in 0..2 {
            sum += w
                .colliders()
                .filter(|c| (c.volume.is_none() && c.body == Body::Static && c.layers.memberships.intersects(LayerMask::WORLD)) || matches!(c.volume, Some(VolumeKind::Water)))
                .map(|c| (c.position.x.to_bits() ^ c.id.0) as usize & 1)
                .sum::<usize>();
        }
    }
    black_box(sum);
    let differs = started.elapsed().as_secs_f64() * 1e6 / n as f64;
    let started = Instant::now();
    for _ in 0..3 {
        black_box(static_colliders_hash(&mut built.world));
    }
    let hash = started.elapsed().as_secs_f64() * 1e6 / 3.0;
    [sync, differs, hash]
}

/// The physics module's snapshot check, as it is (`static_colliders_hash` in shards_motion).
fn static_colliders_hash(world: &mut World) -> u64 {
    let mut q = world.query_filtered::<(&ColliderId, &Collider, &Transform, Option<&Body>, Option<&Volume>), Without<AiId>>();
    let mut found: Vec<(ColliderId, String)> = q
        .iter(world)
        .map(|(id, c, t, b, v)| (*id, format!("{:?} {:?} {:?} {:?}", Some(c), Some((t.translation, t.rotation, t.scale)), b, v)))
        .collect();
    found.sort_by_key(|(id, _)| *id);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for (id, facts) in found {
        for b in id.0.to_le_bytes().iter().chain(facts.as_bytes()).chain(&[0xff]) {
            h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// An in-memory nav bake (the bake's flood, numbering, areas, portals and costs, at the default
/// 0.5 m cells and body) over `cells` x `cells` cells from the cell `(x0, z0)` of the volume.
fn time_bake(built: &Built, v: &VoxelVolume, x0: i32, z0: i32, cells: u32) -> (f64, String) {
    let physics = built.world.resource::<PhysicsWorld>();
    let settings = NavSettings::default();
    let corner = v.corner(IVec3::new(x0, 0, z0));
    let started = Instant::now();
    let tile = spike_bake(physics, &settings, [cells, cells], Vec2::new(corner.x, corner.z));
    let ms = started.elapsed().as_secs_f64() * 1e3;
    let what = match tile {
        Ok(t) => format!("{} nodes ({} swim), {} links, {} areas", t.nodes.len(), t.nodes.iter().filter(|n| n.swim).count(), t.links.len(), t.areas),
        Err(e) => format!("error: {e}"),
    };
    (ms, what)
}

/// The answers the candidates give, compared: rays down from every 0.5 m over a 64 m square, and
/// the terrain probes of the nav proxy.
/// Per candidate: rays down that differ from (c), terrain probes that differ, blocker tops found
/// and blocker tops that differ.
fn agree(built: &[Built], v: &VoxelVolume, w: &TestWorld) -> (usize, [[usize; 4]; 3]) {
    let c = w.arena_center();
    let mut rays = 0;
    let mut out = [[0; 4]; 3];
    let params = CharacterParams::default();
    for j in 0..128 {
        for i in 0..128 {
            let at = v.corner(IVec3::new(c.x - 32, 31, c.y - 32)) + Vec3::new(i as f32 * 0.5 + 0.25, 0.5, j as f32 * 0.5 + 0.25);
            let hits: Vec<Option<f32>> = built
                .iter()
                .map(|b| b.world.resource::<PhysicsWorld>().raycast(at, Dir3::NEG_Y, 60.0, &QueryFilter::new(LayerMask::WORLD)).map(|h| h.distance))
                .collect();
            rays += 1;
            for (k, h) in hits.iter().enumerate() {
                out[k][0] += usize::from(differs(*h, hits[0]));
            }
            let probes: Vec<String> = built
                .iter()
                .map(|b| {
                    let t = Terrain::new(b.world.resource::<PhysicsWorld>(), &params, &[]);
                    let floor = hits[0].map_or(0.0, |d| at.y - d);
                    match t.probe(at.x, at.z, floor) {
                        Probe::Floor { y, .. } => format!("floor {:.3}", y),
                        other => format!("{other:?}"),
                    }
                })
                .collect();
            for (k, p) in probes.iter().enumerate() {
                out[k][1] += usize::from(*p != probes[0]);
            }
            let floor = hits[0].map_or(0.0, |d| at.y - d);
            let tops: Vec<Option<f32>> = built
                .iter()
                .map(|b| Terrain::new(b.world.resource::<PhysicsWorld>(), &params, &[]).blocker_top(at.x + 0.5, at.z, floor))
                .collect();
            for (k, t) in tops.iter().enumerate() {
                out[k][2] += usize::from(t.is_some());
                out[k][3] += usize::from(differs(*t, tops[0]));
            }
        }
    }
    (rays, out)
}

fn differs(a: Option<f32>, b: Option<f32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() > 1e-3,
        (None, None) => false,
        _ => true,
    }
}

#[test]
#[ignore = "a measurement: run it in an optimised build and read what it prints"]
fn voxel_spike_measurements() {
    let palette = test_palette();
    println!("\nG1 spike, {} logical CPUs; numbers are medians of {ROUNDS} interleaved rounds", std::thread::available_parallelism().map_or(0, |n| n.get()));
    for (label, w) in [("rolling hills (default)", TestWorld::default()), ("rough hills", TestWorld::rough())] {
        let seed = 1;
        // Generation and meshing.
        let (mut gen_ms, mut faces_ms, mut greedy_ms, mut hash_ms, mut to_mesh_ms) = (vec![], vec![], vec![], vec![], vec![]);
        let mut v = w.build(&palette, seed);
        let (mut faces_tris, mut greedy_tris, mut faces_bytes, mut greedy_bytes, mut water_quads) = (0, 0, 0, 0, 0);
        for _ in 0..ROUNDS {
            let t = Instant::now();
            v = black_box(w.build(&palette, seed));
            gen_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let faces = black_box(mesh_all(&v, &palette, Meshing::Faces));
            faces_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            let greedy = black_box(mesh_all(&v, &palette, Meshing::Greedy));
            greedy_ms.push(t.elapsed().as_secs_f64() * 1e3);
            faces_tris = faces.iter().map(|m| m.terrain.triangles() + m.foliage.triangles()).sum();
            greedy_tris = greedy.iter().map(|m| m.terrain.triangles() + m.foliage.triangles()).sum();
            faces_bytes = faces.iter().map(|m| m.terrain.bytes() + m.foliage.bytes() + m.water.bytes()).sum();
            greedy_bytes = greedy.iter().map(|m| m.terrain.bytes() + m.foliage.bytes() + m.water.bytes()).sum();
            water_quads = faces.iter().map(|m| m.water.triangles() / 2).sum();
            let t = Instant::now();
            let meshes: Vec<_> = faces.into_iter().flat_map(|m| [m.terrain, m.foliage, m.water]).filter(|m| !m.is_empty()).map(|m| m.into_mesh()).collect();
            black_box(meshes);
            to_mesh_ms.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            black_box(v.hash());
            hash_ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        let (dense, chunks) = v.dense_chunks();
        println!("\n## {label}, seed {seed}: {} x {} x {} cells, {dense} of {chunks} chunks dense, volume {} KiB", w.size.x, w.size.y, w.size.z, v.memory() / 1024);
        println!("generate {:.1} ms | mesh faces {:.1} ms ({faces_tris} tris, {} KiB) | mesh greedy {:.1} ms ({greedy_tris} tris, {} KiB) | to Bevy meshes {:.1} ms | water quads {water_quads} | volume hash {:.2} ms",
            med(gen_ms), med(faces_ms), faces_bytes / 1024, med(greedy_ms), greedy_bytes / 1024, med(to_mesh_ms), med(hash_ms));
        let solid = |id: u8| palette.solid(id);
        println!(
            "boxes: column runs {} | 3D greedy {} | water: plain column runs {}, reaching through the ground {}",
            column_boxes(&v, solid).len(),
            greedy_boxes(&v, solid).len(),
            column_boxes(&v, liquid(&palette)).len(),
            water_boxes(&v, liquid(&palette), solid).len()
        );

        // Collision candidates, interleaved.
        let feet = bodies(&v, &palette, &w);
        let rows = rows();
        let mut build_ms: Vec<Vec<f64>> = vec![vec![]; 3];
        let mut water_ms: Vec<f64> = vec![];
        let mut row_us: Vec<Vec<Vec<f64>>> = vec![vec![vec![]; rows.len()]; 3];
        let mut fixed_us: Vec<Vec<Vec<f64>>> = vec![vec![vec![]; 3]; 3];
        let mut bake_ms: Vec<Vec<f64>> = vec![vec![]; 3];
        let mut baked = [String::new(), String::new(), String::new()];
        let c = w.arena_center();
        let mut last: Vec<Built> = Vec::new();
        for _ in 0..ROUNDS {
            last.clear();
            for (k, cand) in Cand::ALL.iter().enumerate() {
                let (mut built, ms, wms) = build(*cand, &v, &palette, &w, &feet);
                build_ms[k].push(ms);
                water_ms.push(wms);
                for (r, us) in time_rows(&built, &feet, &rows).into_iter().enumerate() {
                    row_us[k][r].push(us);
                }
                for (r, us) in time_fixed(&mut built).into_iter().enumerate() {
                    fixed_us[k][r].push(us);
                }
                let (ms, what) = time_bake(&built, &v, c.x - 32, c.y - 32, 128);
                bake_ms[k].push(ms);
                baked[k] = what;
                last.push(built);
            }
        }
        let (rays, agreement) = agree(&last, &v, &w);
        println!("\n| | {} | {} | {} |", Cand::ALL[0].name(), Cand::ALL[1].name(), Cand::ALL[2].name());
        println!("|---|---:|---:|---:|");
        let line = |name: &str, vals: [String; 3]| println!("| {name} | {} | {} | {} |", vals[0], vals[1], vals[2]);
        line("terrain collision build, ms (merge or copy + spawn + first sync)", std::array::from_fn(|k| format!("{:.1}", med(build_ms[k].clone()))));
        line("static solid colliders (entities)", std::array::from_fn(|k| last[k].solids.to_string()));
        line("water volumes (entities, all the same)", std::array::from_fn(|k| last[k].waters.to_string()));
        line("collision data, KiB (estimate)", std::array::from_fn(|k| format!("{}", last[k].bytes / 1024)));
        for (r, (name, _)) in rows.iter().enumerate() {
            line(&format!("{name}, µs"), std::array::from_fn(|k| format!("{:.2}", med(row_us[k][r].clone()))));
        }
        line(
            "controller share of a tick per body, µs (2 walk + ground + surface + contacts + volumes)",
            std::array::from_fn(|k| format!("{:.1}", TICK.iter().map(|(r, n)| n * med(row_us[k][*r].clone())).sum::<f64>())),
        );
        for (r, name) in ["physics sync, one run (16 bodies moved; it runs twice a tick and once a frame), µs", "navmesh change scan per tick (2 classes), µs", "snapshot static-collider check, µs"].iter().enumerate() {
            line(name, std::array::from_fn(|k| format!("{:.1}", med(fixed_us[k][r].clone()))));
        }
        line("nav bake in memory, 64 m chunk (128 x 128 cells), ms", std::array::from_fn(|k| format!("{:.0}", med(bake_ms[k].clone()))));
        line("nav bake: what it found", baked.clone());
        line(&format!("rays down (of {rays}) that differ from (c)"), std::array::from_fn(|k| agreement[k][0].to_string()));
        line("terrain probes that differ from (c)", std::array::from_fn(|k| agreement[k][1].to_string()));
        line("blocker tops found", std::array::from_fn(|k| agreement[k][2].to_string()));
        line("blocker tops that differ from (c)", std::array::from_fn(|k| agreement[k][3].to_string()));
        println!("water boxes, sea boxes, bodies and their sync: {:.1} ms (the same for all three)", med(water_ms));
        // Floors under cover, seeded only through the solid above them: a 2 m square under the
        // overhang's slab and one in the cave under the mound.
        for (name, x0, z0) in [("under the overhang", c.x - 6, c.y + 4), ("in the cave", c.x - 25, c.y - 1)] {
            let found: [String; 3] = std::array::from_fn(|k| time_bake(&last[k], &v, x0, z0, 4).1);
            line(&format!("nav bake of a 2 m square {name}"), found);
        }
        // The whole map, once.
        for (k, b) in last.iter().enumerate() {
            let (ms, what) = time_bake(b, &v, 0, 0, w.size.x * 2);
            println!("{}: nav bake of the whole {} m map in one piece: {ms:.0} ms, {what}", Cand::ALL[k].name(), w.size.x);
        }
        for b in &last {
            let w = b.world.resource::<PhysicsWorld>();
            if let Some(f) = w.spike_field(b.terrain) {
                black_box(f.size);
            }
        }
    }
}


/// Mode changes the controller reported.
#[derive(Resource, Default)]
struct Modes(usize);

/// What a walk of the real controller did: ms per tick (the whole fixed tick: sync, the
/// controller, volumes), where each body ended, mode changes, ticks in the air, bodies whose feet
/// ended inside a solid cell.
struct Walked {
    ms: f64,
    ends: Vec<Vec3>,
    modes: usize,
    air: usize,
    buried: usize,
}

/// Sixteen controllers walk the terrain for `ticks` ticks: a new direction every second, every
/// other body jumping now and then (onto steps and the ledge, into the pool).
fn walk(cand: Cand, v: &VoxelVolume, p: &VoxelPalette, w: &TestWorld, feet: &[Vec3], ticks: u64) -> Walked {
    use ai_core::sim::StepEnd;
    use ai_core::{AiCorePlugin, SimConfig};
    use shards_motion::{CharacterController, LocomotionInput, ModeChanged, MotionMode, MotionPlugin, MotionState};
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin { remote_port: None, crash_replays: false, sim: SimConfig { start_paused: true, ..default() }, ..default() })
        .add_plugins(MotionPlugin)
        .init_resource::<Modes>()
        .add_observer(|_: On<ModeChanged>, mut m: ResMut<Modes>| m.0 += 1);
    app.finish();
    app.cleanup();
    app.update();
    let world = app.world_mut();
    let terrain = world.spawn(Name::new("terrain")).id();
    match cand {
        Cand::Boxes => {
            world.spawn_batch(column_boxes(v, |id| p.solid(id)).into_iter().map(|b| {
                let (center, half) = b.world(v);
                (Collider::new(Shape::Cuboid { half }), Transform::from_translation(center))
            }));
        }
        Cand::Voxels | Cand::Field => {
            let id = world.resource::<ColliderIds>().peek();
            world.resource_mut::<ColliderIds>().restart_at(ColliderId(id.0 + 1));
            if cand == Cand::Voxels {
                let s = v.size().as_ivec3();
                let cells: Vec<IVec3> = (0..s.x)
                    .flat_map(|x| (0..s.z).flat_map(move |z| (0..s.y).map(move |y| IVec3::new(x, y, z))))
                    .filter(|c| p.solid(v.get(*c)))
                    .collect();
                world.resource_mut::<PhysicsWorld>().spike_insert_voxels(terrain, id, v.origin(), v.block(), &cells);
            } else {
                let field = VoxelField::new(v.origin(), v.block(), v.size(), |c| p.solid(v.get(c)));
                world.resource_mut::<PhysicsWorld>().spike_insert_field(terrain, id, field);
            }
        }
    }
    world.spawn_batch(water(v, p, w).into_iter().map(|(center, half)| {
        (Collider::new(Shape::Cuboid { half }).with_layers(CollisionLayers::VOLUME), Volume { kind: VolumeKind::Water }, Transform::from_translation(center))
    }));
    let bodies: Vec<Entity> = feet
        .iter()
        .map(|f| world.spawn((Walker, CharacterController::default(), CharacterParams::default(), Transform::from_translation(*f))).id())
        .collect();
    let (mut ms, mut air) = (0.0, 0);
    for t in 0..ticks {
        let world = app.world_mut();
        for (b, e) in bodies.iter().enumerate() {
            let mut input = world.get_mut::<LocomotionInput>(*e).unwrap();
            if t % 60 == 0 {
                let a = b as f32 * 2.399 + (t / 60) as f32 * 1.1;
                input.move_dir = Vec3::new(ops::cos(a), 0.0, ops::sin(a));
            }
            input.jump = b % 2 == 1 && t % 150 == 75;
        }
        let started = Instant::now();
        let report = ai_core::sim::step(app.world_mut(), 1, std::time::Duration::from_secs(60), |_| None).unwrap();
        ms += started.elapsed().as_secs_f64() * 1e3;
        assert!(!matches!(report.end, StepEnd::SystemFailed(_)), "{}: {:?}", cand.name(), report.end);
        let world = app.world();
        air += bodies.iter().filter(|e| world.get::<MotionState>(**e).unwrap().mode == MotionMode::Air).count();
    }
    let world = app.world();
    let ends: Vec<Vec3> = bodies.iter().map(|e| world.get::<Transform>(*e).unwrap().translation).collect();
    let buried = ends.iter().filter(|f| p.solid(v.get(((**f + Vec3::Y * 0.05 - v.origin()) / v.block()).floor().as_ivec3()))).count();
    Walked { ms: ms / ticks as f64, ends, modes: world.resource::<Modes>().0, air, buried }
}

#[test]
#[ignore = "a measurement: run it in an optimised build and read what it prints"]
fn controller_walks_alike() {
    let palette = test_palette();
    let ticks = 600;
    for (label, w) in [("rolling hills (default)", TestWorld::default()), ("rough hills", TestWorld::rough())] {
        let v = w.build(&palette, 1);
        let feet = bodies(&v, &palette, &w);
        let mut ms: Vec<Vec<f64>> = vec![vec![]; 3];
        let mut last: Vec<Walked> = Vec::new();
        for _ in 0..3 {
            last.clear();
            for (k, cand) in Cand::ALL.iter().enumerate() {
                let walked = walk(*cand, &v, &palette, &w, &feet, ticks);
                ms[k].push(walked.ms);
                last.push(walked);
            }
        }
        println!("\n## {label}: 16 controllers, {ticks} ticks, medians of 3 interleaved rounds\n");
        println!("| | {} | {} | {} |\n|---|---:|---:|---:|", Cand::ALL[0].name(), Cand::ALL[1].name(), Cand::ALL[2].name());
        let line = |name: &str, vals: [String; 3]| println!("| {name} | {} | {} | {} |", vals[0], vals[1], vals[2]);
        line("fixed tick, ms (sync + controller + volumes, 16 bodies)", std::array::from_fn(|k| format!("{:.3}", med(ms[k].clone()))));
        line("mode changes", std::array::from_fn(|k| last[k].modes.to_string()));
        line("body-ticks in the air", std::array::from_fn(|k| last[k].air.to_string()));
        line("bodies ending with their feet in a solid cell", std::array::from_fn(|k| last[k].buried.to_string()));
        line(
            "bodies ending over 1 cm / 0.5 m from where (c) ends",
            std::array::from_fn(|k| {
                let d: Vec<f32> = last[k].ends.iter().zip(&last[0].ends).map(|(a, b)| a.distance(*b)).collect();
                format!("{} / {}", d.iter().filter(|d| **d > 0.01).count(), d.iter().filter(|d| **d > 0.5).count())
            }),
        );
    }
}
