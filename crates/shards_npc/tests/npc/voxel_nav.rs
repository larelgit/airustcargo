//! The navmesh of a voxel region (A2e): baked in memory, chunk by chunk, stitched across the
//! chunk seams. A hand-made volume of 3 x 3 chunks of 8 m (48 columns across at 0.5 m) with a
//! step, a cave mouth and a lake exactly on the chunk seams; the chunks are compared with the
//! same grid baked in one piece: every node and every link equal.

use std::collections::BTreeSet;

use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::NetRole;
use shards_motion::{Collider, CollisionLayers, PhysicsWorld, Shape, Volume, VolumeKind, VoxelCollider, VoxelField};
use shards_npc::nav::settings::NavSettings;
use shards_npc::nav::{bake_volume, rebuild, Cut, NavBakes};
use shards_npc::{NavQuery, NavRegion};
use shards_world::manifest::{Layout, RegionManifest, FORMAT};
use shards_world::ActiveRegion;

use crate::common::*;

/// The edge of a cell of the test volume (m): a step of one cell is a step a body walks.
const CELL: f32 = 0.25;
/// Cells across the test volume.
const SIDE: u32 = 96;
/// The least corner of the volume in the world.
const ORIGIN: Vec3 = Vec3::new(-12.0, -2.0, -12.0);

/// Whether cell `c` of the test volume is solid, by the world position of its middle. The ground
/// tops at world y 1; the chunk seams are at x and z of -4 and 4.
fn solid(c: IVec3) -> bool {
    let p = ORIGIN + (c.as_vec3() + Vec3::splat(0.5)) * CELL;
    let (x, y, z) = (p.x, p.y, p.z);
    // A lake 2 m deep across the z = 4 seam (x -3..3, z 2..7).
    let lake = (-3.0..3.0).contains(&x) && (2.0..7.0).contains(&z) && (-1.0..1.0).contains(&y);
    // A step of a quarter metre (a body walks it) on the x = -4 seam: the ground east of it is higher.
    let step = x >= -4.0 && (-9.0..-4.0).contains(&z) && y < 1.25;
    // A mound east of the x = 4 seam with a tunnel through it (3 m wide, 3 m high above the
    // ground) whose mouth is the seam itself.
    let mound = (4.0..10.0).contains(&x) && (-3.0..3.0).contains(&z) && y < 5.0;
    let tunnel = (4.0..10.0).contains(&x) && (-1.0..2.0).contains(&z) && (1.0..4.0).contains(&y);
    ((y < 1.0 && !lake) || step || mound) && !tunnel
}

fn manifest(region: &str) -> RegionManifest {
    RegionManifest {
        format: FORMAT.to_owned(),
        region: region.to_owned(),
        layout: Layout::Volume,
        seed: 1,
        version: 1,
        attempt: 1,
        fallback: None,
        size: [3, 3],
        tile_size: 8.0,
        origin: [ORIGIN.x, ORIGIN.z],
        tiles: Vec::new(),
        pois: Vec::new(),
        links: Vec::new(),
        spawn: None,
        content_hash: String::new(),
        steps: Vec::new(),
    }
}

/// A game in the voxel region: the field, the lake's water, and the volume's manifest. Its
/// chunk bakes are its own: the tests run side by side, in one process.
fn region_app(solid: impl FnMut(IVec3) -> bool) -> App {
    region_named("vox", solid, true)
}

/// [`region_app`] of region `name`, its bakes shared with the process's unless `isolated`.
fn region_named(name: &str, solid: impl FnMut(IVec3) -> bool, isolated: bool) -> App {
    let mut app = app_in(ASSETS, 7, |_| {});
    let world = app.world_mut();
    world.get_resource_or_init::<NavBakes>().isolated = isolated;
    let field = VoxelField::new(ORIGIN, CELL, UVec3::new(SIDE, 32, SIDE), solid);
    world.spawn((Name::new("voxel terrain vox"), VoxelCollider::new(field), Transform::default()));
    // The lake's water, its surface 5 cm under the ground.
    let (min, max) = (Vec3::new(-3.0, -1.0, 2.0) , Vec3::new(3.0, 0.95, 7.0));
    world.spawn((
        Name::new("voxel water 0"),
        Collider::new(Shape::cuboid((max - min) * 0.5)).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_translation((min + max) * 0.5),
    ));
    let m = manifest(name);
    world.insert_resource(ActiveRegion::new(m, None, None, 2));
    app
}

/// The nav settings of the test content.
fn settings(app: &App) -> NavSettings {
    NavSettings::of(app.world().resource::<ContentDb>())
}

/// A node as the other bake sees it: its global column, floor (mm) and way of standing.
type Key = (i32, i32, i32, bool);

fn key(nav: &NavRegion, node: u32) -> Key {
    let (inst, n) = nav.split(node);
    let local = &nav.instances[inst as usize].nav.nodes[n as usize];
    let [gx, gz] = nav.node_column(node);
    (gx, gz, local.y_mm, local.swim)
}

/// A move as the other bake sees it: from, to, kind, cost.
type Move = (Key, Key, u8, u32);

/// Every node, and every move out of every node, as keys.
fn contents(nav: &NavRegion) -> (BTreeSet<Key>, BTreeSet<Move>) {
    let mut nodes = BTreeSet::new();
    let mut links = BTreeSet::new();
    let mut out = Vec::new();
    for n in 0..nav.node_count() {
        nodes.insert(key(nav, n));
        nav.moves(n, &mut out);
        for &(to, kind, cost, _) in &out {
            links.insert((key(nav, n), key(nav, to), kind as u8, cost));
        }
    }
    (nodes, links)
}

#[test]
fn the_chunks_of_a_volume_are_baked_in_memory_and_stitched() {
    let mut app = region_app(solid);
    let settings = settings(&app);
    let nav = bake_volume(app.world_mut(), &settings, Cut::Chunks).expect("a navmesh");
    assert_eq!(nav.instances.len(), 9);
    assert_eq!(nav.per, 16);
    assert_eq!(nav.size, [3, 3]);
    let voxel = nav.voxel.expect("a voxel bake");
    assert_eq!((voxel.chunks, voxel.reused), (9, 0));
    assert!(voxel.to_string().starts_with("in memory, 9 chunks, baked in "), "{voxel}");
    assert!(nav.summary().starts_with("9 chunks, "), "{}", nav.summary());
    assert!(nav.report.stitched > 0, "the chunks are joined");
    assert_eq!(nav.report.closed_edges, 0, "no seam is closed");
    // The whole grid is one area of walking and climbing: from a corner to the other.
    let corner = |x: f32, z: f32| nav.nearest_node(Vec3::new(x, 1.0, z), 1.0, 3.0).expect("a node");
    assert_eq!(nav.component(corner(-11.5, -11.5)), nav.component(corner(11.5, 11.5)));
    // Kept under their keys, nothing written.
    assert_eq!(app.world().resource::<NavBakes>().chunks.len(), 9);
    // Again: every chunk is kept, none baked.
    let again = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    assert_eq!(again.voxel.unwrap().reused, 9);
    assert_eq!(contents(&nav), contents(&again));
}

#[test]
fn chunks_stitched_are_the_grid_baked_in_one_piece() {
    let mut app = region_app(solid);
    let settings = settings(&app);
    let chunked = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    let whole = bake_volume(app.world_mut(), &settings, Cut::Whole).unwrap();
    assert_eq!(whole.instances.len(), 1);
    let (cn, cl) = contents(&chunked);
    let (wn, wl) = contents(&whole);
    // Walking nodes, climbing nodes and swimming ones: all of the volume's features are there.
    assert!(cn.iter().any(|k| k.3), "swim nodes");
    assert!(cn.iter().any(|k| !k.3 && k.2 > 900), "the ground by the step, a block higher");
    assert!(cn.iter().any(|k| !k.3 && k.2 > 2500), "the top of the mound");
    let only: Vec<&Key> = cn.symmetric_difference(&wn).take(12).collect();
    assert!(only.is_empty(), "nodes in one bake only (first 12 of {}): {only:?}", cn.symmetric_difference(&wn).count());
    let only: Vec<_> = cl.symmetric_difference(&wl).take(12).collect();
    assert!(only.is_empty(), "links in one bake only (first 12 of {}): {only:?}", cl.symmetric_difference(&wl).count());
    // The same areas, up to renumbering: nodes that reach each other in one reach each other
    // in the other.
    let mut pairs: BTreeSet<(u32, u32)> = BTreeSet::new();
    let by_key = |nav: &NavRegion| -> std::collections::BTreeMap<Key, u32> { (0..nav.node_count()).map(|n| (key(nav, n), nav.component(n))).collect() };
    let (a, b) = (by_key(&chunked), by_key(&whole));
    for (k, c) in &a {
        pairs.insert((*c, b[k]));
    }
    let left: BTreeSet<u32> = pairs.iter().map(|p| p.0).collect();
    let right: BTreeSet<u32> = pairs.iter().map(|p| p.1).collect();
    assert_eq!((left.len(), right.len(), pairs.len()), (right.len(), left.len(), left.len()), "areas differ: {pairs:?}");
}

#[test]
fn routes_across_the_seams_are_as_long_in_chunks_as_in_one_piece() {
    let mut app = region_app(solid);
    let settings = settings(&app);
    let chunked = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    let whole = bake_volume(app.world_mut(), &settings, Cut::Whole).unwrap();
    let physics = app.world().resource::<PhysicsWorld>();
    // 64 pairs of nodes of the one area, picked by a fixed generator.
    let nodes: Vec<u32> = (0..chunked.node_count()).filter(|n| !chunked.is_swim(*n)).collect();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let mut compared = 0;
    for _ in 0..64 {
        let (a, b) = (nodes[next(nodes.len())], nodes[next(nodes.len())]);
        let (from, to) = (chunked.node_pos(a), chunked.node_pos(b));
        let query = NavQuery { from, to, goal_level: true, radius: 0.4, height: 1.8, step_height: 0.35 };
        let (c, w) = (chunked.path(physics, &query), whole.path(physics, &query));
        match (&c, &w) {
            (Ok(c), Ok(w)) => {
                assert!((c.length - w.length).abs() <= 0.5, "{from} -> {to}: {} m in chunks, {} m in one piece: {:?} against {:?}", c.length, w.length, c.points, w.points);
                compared += 1;
            }
            (Err(_), Err(_)) => {}
            _ => panic!("{from} -> {to}: chunks {c:?}, one piece {w:?}"),
        }
    }
    assert!(compared > 40, "most pairs have a route: {compared}");
}

#[test]
fn the_seam_features_are_in_the_navmesh() {
    let mut app = region_app(solid);
    let settings = settings(&app);
    let nav = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    let (nodes, links) = contents(&nav);
    // The step on the x = -4 seam (global columns 15 and 16 at 0.5 m): a quarter metre up going
    // east, and down going west, row by row along its z -9..-4 (rows 6..16).
    let up = links.iter().filter(|l| l.0 .0 == 15 && l.1 .0 == 16 && l.1 .2 == l.0 .2 + 250).count();
    assert!(up >= 8, "a step up across the seam: {up} moves");
    let down = links.iter().filter(|l| l.0 .0 == 16 && l.1 .0 == 15 && l.1 .2 == l.0 .2 - 250).count();
    assert!(down >= 8, "and back down: {down}");
    // The tunnel's mouth on the x = 4 seam (column 32): nodes inside, both sides of the seam.
    let inside = |x: i32| nodes.iter().any(|k| k.0 == x && !k.3 && (-1..=2).contains(&((k.1 as f32 * 0.5 - 12.0) as i32)) && k.2 < 1200 && k.2 > 800);
    assert!(inside(31) && inside(32), "ground in the tunnel on both sides of its mouth");
    assert!(links.iter().any(|l| l.0 .0 == 31 && l.1 .0 == 32 && !l.0 .3 && l.0 .2 == 1000), "walking into the tunnel across the seam");
    // The lake on the z = 4 seam (rows 32 and 31): swim nodes either side, joined.
    let swim = |row: i32| nodes.iter().any(|k| k.3 && k.1 == row);
    assert!(swim(31) && swim(32), "swimming on both sides of the seam");
    assert!(links.iter().any(|l| l.0 .3 && l.1 .3 && l.0 .1 == 31 && l.1 .1 == 32), "swimming across it");
}

#[test]
fn chunk_bakes_do_not_depend_on_threads() {
    let mut app = region_app(solid);
    let settings = settings(&app);
    let mut seen: Option<Vec<String>> = None;
    for round in 0..4 {
        // A cold bake each time: nothing kept, the pool free to finish in any order.
            app.world_mut().resource_mut::<NavBakes>().chunks.clear();
        let nav = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
        assert_eq!(nav.voxel.unwrap().reused, 0, "round {round}");
        let files: Vec<String> = nav.instances.iter().map(|i| i.nav.to_file()).collect();
        match &seen {
            None => seen = Some(files),
            Some(first) => assert_eq!(*first, files, "round {round}: the chunks differ from the first round's"),
        }
    }
}

#[test]
fn a_chunk_bake_is_kept_by_the_field_and_the_body_not_by_the_game() {
    let mut app = region_named("vox-kept", solid, false);
    let settings = settings(&app);
    let first = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    // Another game with the same field: its chunks come from the process's keep.
    let mut other = region_named("vox-kept", solid, false);
    let kept = bake_volume(other.world_mut(), &settings, Cut::Chunks).unwrap();
    assert_eq!(kept.voxel.unwrap().reused, 9);
    assert_eq!(contents(&first), contents(&kept));
    // Another field: baked again.
    let mut changed = region_named("vox-kept", |c| solid(c) && c != IVec3::new(2, 2, 2), false);
    let fresh = bake_volume(changed.world_mut(), &settings, Cut::Chunks).unwrap();
    assert_eq!(fresh.voxel.unwrap().reused, 0, "a field with a cell less is not the field that was baked");
    // Another body class, the same field: its own bakes.
    let mut wide = settings.clone();
    wide.body = "wide".to_owned();
    wide.agent.radius = 0.6;
    let class = bake_volume(app.world_mut(), &wide, Cut::Chunks).unwrap();
    assert_eq!(class.voxel.unwrap().reused, 0);
    assert_eq!(app.world().resource::<NavBakes>().chunks.len(), 18);
}

#[test]
fn a_solid_placed_in_the_region_carves_it_and_is_not_in_the_chunk_bake() {
    let mut app = region_app(solid);
    let settings = settings(&app);
    let before = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    // A box on the open ground, not a part of the field: it is not baked into the chunks.
    wall(app.world_mut(), Vec3::new(-10.0, 1.0, -10.0), Vec3::new(-9.0, 3.0, -9.0));
    let after = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
    assert_eq!(after.voxel.unwrap().reused, 9, "the chunks are the field's, the box is not in them");
    let (nodes, _) = contents(&before);
    let column_nodes = |nav: &NavRegion| nav.nodes_at([4, 4]).filter(|n| !nav.is_disabled(*n)).count();
    assert!(nodes.contains(&(4, 4, 1000, false)), "ground at the corner of the box's place");
    assert_eq!(column_nodes(&before), 1);
    assert_eq!(column_nodes(&after), 0, "a node the box stands on is carved off");
    assert!(after.report.carved > 0);
}

#[test]
fn a_machine_that_does_not_simulate_bakes_nothing() {
    let mut app = region_app(solid);
    app.insert_resource(NetRole::Client);
    rebuild(app.world_mut());
    assert!(app.world().get_resource::<NavRegion>().is_none());
    let info = ai_core::call_tool(app.world_mut(), "dev", "nav_info", serde_json::json!({}));
    assert!(format!("{info:?}").contains("not baked: this machine does not simulate"), "{info:?}");
    app.insert_resource(NetRole::Single);
    rebuild(app.world_mut());
    assert!(app.world().get_resource::<NavRegion>().is_some());
}

#[test]
fn a_field_without_ground_has_a_navmesh_without_nodes() {
    let mut app = region_app(|_| false);
    let settings = settings(&app);
    let nav = bake_volume(app.world_mut(), &settings, Cut::Chunks).expect("no ground is not an error");
    assert_eq!(nav.node_count(), 0);
    assert!(nav.report.failed.is_empty(), "{:?}", nav.report.failed);
}
