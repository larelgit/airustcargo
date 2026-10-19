//! The voxel arena (`content/world/arena.voxel_region.ron`, its steps in
//! `src/gameplay/terrain.rs`), for what its scenarios do not say: an NPC there plans on the
//! navmesh baked in memory when the region loads (`voxel_nav.scenario` has the chase; the chunks
//! against one piece and the seams are `shards_npc`'s tests), and a body that ends up below
//! the sea floor past the grid's edge falls. Nothing is under the sea boxes, and the engine has no
//! out-of-world rule of its own: the sandbox's `bodies_in_world` stops the game once the body is
//! below `fall_below`. The scenario cannot show that: the stop would be in its replay.

use shards::prelude::*;
use shards::serde_json::json;
use shards::world::Marker;

use crate::common::{ok, text, PLAYER};

/// The sandbox in the voxel arena of seed 7, the player at its spawn.
fn arena() -> App {
    let mut app = sandbox::test_app(42);
    let out = ok(&mut app, "regen", json!({"region": "voxel_arena", "seed": 7}));
    assert!(text(&out).starts_with("ok: voxel_arena seed 7 v1 (attempt 1, 16 chunks, 6 POIs"), "{}", text(&out));
    app
}

/// Where the region's marker `name` is.
fn marker(app: &mut App, name: &str) -> Vec3 {
    let world = app.world_mut();
    let mut q = world.query::<(&Marker, &Transform)>();
    q.iter(world).find(|(m, _)| m.name == name).map(|(_, tf)| tf.translation).unwrap_or_else(|| panic!("no marker `{name}`"))
}

#[test]
fn an_npc_in_the_voxel_arena_plans_on_the_baked_navmesh_and_says_so() {
    let mut app = arena();
    let post = marker(&mut app, "post");
    assert_eq!(post, Vec3::new(12.5, 0.0, -399.5), "the post is 12 m east of the spawn, on the floor");
    let out = ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": post.x, "y": post.y, "z": post.z}));
    let id = text(&out).split_whitespace().nth(1).unwrap().to_owned();
    assert!(text(&out).starts_with(&format!("spawned {id} skeleton at (12.5, -399.5)")), "{}", text(&out));
    let state = ok(&mut app, "npc_state", json!({"id": id})).result;
    assert_eq!(state["nav"]["routes"], "navmesh, body agent", "{state}");
    let route = ok(&mut app, "nav_path", json!({"to": id})).result;
    assert_eq!(route["provider"], "navmesh", "{route}");
}

#[test]
fn a_body_below_the_sea_floor_past_the_grid_edge_falls_out_of_the_world() {
    let mut app = arena();
    // In the sea, 2 m past the east edge (x 128) and 1 m under its floor (layer 2: y -10).
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 130.0, "y": -11.0, "z": -400.0}));
    assert_eq!(sandbox::ai::bodies_in_world(app.world()), None, "over the sea is in the world");
    let out = shards::ai_core::call_tool(app.world_mut(), "dev", "step", json!({"ticks": 300}));
    assert!(text(&out).contains("stopped: invariant bodies_in_world violated at tick "), "{}", text(&out));
    let journal = app.world().resource::<shards::ai_core::AiJournal>();
    let line = journal.after(0).0.map(ToString::to_string).find(|l| l.contains("invariant bodies_in_world violated")).unwrap();
    assert!(line.contains("e5 fell out of the world: y = -30."), "{line}");
    let feet = crate::common::player::<Transform>(&mut app).translation;
    assert!((feet.x - 130.0).abs() < 0.01 && (feet.z + 400.0).abs() < 0.01, "it falls straight down: {feet}");
}

/// Every node (global column, floor in mm, swimming) and every move (to, kind, cost) of a
/// navmesh, as keys that do not depend on how the grid was cut.
type Keys = (std::collections::BTreeSet<(i32, i32, i32, bool)>, std::collections::BTreeSet<((i32, i32, i32, bool), (i32, i32, i32, bool), u8, u32)>);

fn keys_of(nav: &shards::npc::NavRegion) -> Keys {
    let key = |node: u32| {
        let (inst, n) = nav.split(node);
        let local = &nav.instances[inst as usize].nav.nodes[n as usize];
        let [gx, gz] = nav.node_column(node);
        (gx, gz, local.y_mm, local.swim)
    };
    let (mut nodes, mut links, mut out) = (Default::default(), std::collections::BTreeSet::new(), Vec::new());
    let nodes: &mut std::collections::BTreeSet<_> = &mut nodes;
    for n in 0..nav.node_count() {
        nodes.insert(key(n));
        nav.moves(n, &mut out);
        for &(to, kind, cost, _) in &out {
            links.insert((key(n), key(to), kind as u8, cost));
        }
    }
    (std::mem::take(nodes), links)
}

#[test]
fn the_arenas_sixteen_chunks_are_the_island_baked_in_one_piece() {
    use shards::npc::nav::{bake_volume, Cut};
    use shards::npc::NavQuery;
    let mut app = arena();
    let settings = shards::npc::nav::settings::NavSettings::of(app.world().resource::<shards::content::ContentDb>());
    let chunked = app.world().resource::<shards::npc::NavRegion>().clone();
    assert_eq!(chunked.instances.len(), 16);
    let whole = bake_volume(app.world_mut(), &settings, Cut::Whole).expect("the island in one piece");
    assert_eq!(whole.instances.len(), 1);
    let (cn, cl) = keys_of(&chunked);
    let (wn, wl) = keys_of(&whole);
    assert!(cn.iter().any(|k| k.3) && cn.len() > 10_000, "swim nodes and ground: {} nodes", cn.len());
    let nodes: Vec<_> = cn.symmetric_difference(&wn).take(10).collect();
    assert!(nodes.is_empty(), "nodes in one bake only ({} in all): {nodes:?}", cn.symmetric_difference(&wn).count());
    let links: Vec<_> = cl.symmetric_difference(&wl).take(10).collect();
    assert!(links.is_empty(), "links in one bake only ({} in all): {links:?}", cl.symmetric_difference(&wl).count());
    // Same areas up to renumbering, from the spawn's: the nodes that reach the spawn in one
    // reach it in the other.
    let spawn = Vec3::new(0.5, 0.0, -399.5);
    let (a, b) = (chunked.nearest_node(spawn, 1.0, 1.0).unwrap(), whole.nearest_node(spawn, 1.0, 1.0).unwrap());
    let reach = |nav: &shards::npc::NavRegion, from: u32| -> std::collections::BTreeSet<(i32, i32, i32, bool)> {
        (0..nav.node_count())
            .filter(|n| nav.component(*n) == nav.component(from))
            .map(|n| {
                let (inst, i) = nav.split(n);
                let local = &nav.instances[inst as usize].nav.nodes[i as usize];
                let [gx, gz] = nav.node_column(n);
                (gx, gz, local.y_mm, local.swim)
            })
            .collect()
    };
    let (ra, rb) = (reach(&chunked, a), reach(&whole, b));
    assert!(ra.len() > 10_000, "the spawn's area is the island's walkable ground: {}", ra.len());
    assert_eq!(ra, rb, "the spawn reaches the same cells");
    // 64 pairs of cells of that area: routes of the same length, within one cell (0.5 m).
    let cells: Vec<_> = ra.iter().filter(|k| !k.3).copied().collect();
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut pick = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let physics = app.world().resource::<shards::motion::PhysicsWorld>();
    let at = |k: (i32, i32, i32, bool)| Vec3::new(-128.0 + (k.0 as f32 + 0.5) * 0.5, k.2 as f32 / 1000.0, -528.0 + (k.1 as f32 + 0.5) * 0.5);
    let mut compared = 0;
    for _ in 0..64 {
        let (from, to) = (at(cells[pick(cells.len())]), at(cells[pick(cells.len())]));
        let query = NavQuery { from, to, goal_level: true, radius: 0.4, height: 1.8, step_height: 0.35 };
        match (chunked.path(physics, &query), whole.path(physics, &query)) {
            (Ok(c), Ok(w)) => {
                assert!((c.length - w.length).abs() <= 0.5, "{from} -> {to}: {} m in chunks, {} m in one piece", c.length, w.length);
                compared += 1;
            }
            (c, w) => panic!("{from} -> {to}: chunks {c:?}, one piece {w:?}"),
        }
    }
    assert_eq!(compared, 64);
}

/// The bake time of the arena's navmesh at 64 x 32 x 64 chunks, for the ledger and the PR: run by
/// hand, in `--release` on an idle machine for a number to quote:
/// `cargo test -p sandbox --release --test sandbox -- --ignored --nocapture voxel_nav_bake_time`.
#[test]
#[ignore = "a measurement, not a check"]
fn voxel_nav_bake_time() {
    use shards::npc::nav::{bake_volume, clear_shared_bakes, Cut};
    let mut app = arena();
    let settings = shards::npc::nav::settings::NavSettings::of(app.world().resource::<shards::content::ContentDb>());
    for round in 0..3 {
        clear_shared_bakes();
        app.world_mut().remove_resource::<shards::npc::nav::NavBakes>();
        let chunks = bake_volume(app.world_mut(), &settings, Cut::Chunks).unwrap();
        let v = chunks.voxel.unwrap();
        println!(
            "round {round}: 16 chunks of 64 m on {} workers: chunks {:.1} ms, whole build {:.1} ms, {} nodes, {} links across seams",
            v.threads,
            v.chunks_in.as_secs_f64() * 1e3,
            v.built_in.as_secs_f64() * 1e3,
            chunks.node_count(),
            chunks.report.stitched
        );
    }
    // What a region change pays: the chunks of both body classes on the pool, stitched and
    // assembled, as `rebuild` does it (the journal lines it writes are not counted).
    for round in 0..3 {
        clear_shared_bakes();
        app.world_mut().remove_resource::<shards::npc::nav::NavBakes>();
        let started = std::time::Instant::now();
        shards::npc::nav::rebuild(app.world_mut());
        println!("round {round}: rebuild of every body class, cold: {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
    }
    clear_shared_bakes();
    app.world_mut().remove_resource::<shards::npc::nav::NavBakes>();
    let whole = bake_volume(app.world_mut(), &settings, Cut::Whole).unwrap();
    let v = whole.voxel.unwrap();
    println!("the island in one piece (single thread): chunk {:.1} ms, whole build {:.1} ms, {} nodes", v.chunks_in.as_secs_f64() * 1e3, v.built_in.as_secs_f64() * 1e3, whole.node_count());
}
