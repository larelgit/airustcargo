//! The voxel arena (`content/world/arena.voxel_region.ron`, its steps in
//! `src/gameplay/terrain.rs`), for what its scenario does not say: an NPC there plans over the
//! local grid and says why (a voxel region has no navmesh until A2e), and a body that ends up below
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
fn an_npc_in_the_voxel_arena_plans_over_the_local_grid_and_says_why() {
    let mut app = arena();
    let post = marker(&mut app, "post");
    assert_eq!(post, Vec3::new(12.5, 0.0, -399.5), "the post is 12 m east of the spawn, on the floor");
    let out = ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": post.x, "y": post.y, "z": post.z}));
    let id = text(&out).split_whitespace().nth(1).unwrap().to_owned();
    assert!(text(&out).starts_with(&format!("spawned {id} skeleton at (12.5, -399.5)")), "{}", text(&out));
    let state = ok(&mut app, "npc_state", json!({"id": id})).result;
    assert_eq!(state["nav"]["routes"], "local grid: no navmesh for this region (voxel region, nav not baked yet)", "{state}");
    let refused = crate::common::fails(&mut app, "nav_path", json!({"to": id}));
    assert!(refused.contains("no navmesh for this region (voxel region, nav not baked yet): use path_to"), "{refused}");
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
