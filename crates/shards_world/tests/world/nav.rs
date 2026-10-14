//! Generated regions get a navmesh as they load (the NPC module's): a meadow with its
//! river, ponds and roads, and a crypt whose rooms meet only at their doors.

use shards::motion::PhysicsWorld;
use shards::npc::{NavQuery, NavRegion};
use shards::prelude::*;

use crate::common::*;

fn route(app: &App, from: Vec3, to: Vec3) -> Result<f32, String> {
    let nav = app.world().get_resource::<NavRegion>().expect("a navmesh");
    let query = NavQuery { from, to, goal_level: false, radius: 0.4, height: 1.8, step_height: 0.35 };
    nav.path(app.world().resource::<PhysicsWorld>(), &query).map(|p| p.length).map_err(|e| e.to_string())
}

#[test]
fn generated_regions_get_a_navmesh_that_routes_between_their_points() {
    let mut app = app(42, Some("meadow"));
    let text = journal(&app).join("\n");
    assert!(text.contains("navmesh: meadow seed 42 v1 (64 tiles, "), "{text}");
    let manifest = region(&app).manifest.clone();
    let spawn = manifest.spawn_position();
    // The region's own check says every point is reachable from the spawn: so does the navmesh.
    for poi in &manifest.pois {
        let length = route(&app, spawn, poi.position()).unwrap_or_else(|e| panic!("{}: {e}", poi.id));
        assert!(length >= spawn.distance(poi.position()) * 0.99, "{}: {length}", poi.id);
    }

    // A crypt: rooms of several cells, turned, joined at their doors.
    let out = ok(&mut app, "regen", serde_json::json!({"region": "crypt", "seed": 5}));
    assert!(out.starts_with("ok: crypt seed 5 v1"), "{out}");
    let nav = app.world().resource::<NavRegion>();
    assert!(nav.report.stitched > 0 && nav.report.failed.is_empty(), "{:?}", nav.report);
    let manifest = region(&app).manifest.clone();
    let boss = manifest.pois.iter().find(|p| p.poi == "boss").expect("a boss").position();
    route(&app, manifest.spawn_position(), boss).expect("the entrance reaches the throne");
}

#[test]
fn a_region_away_from_the_origin_is_built_and_routed_where_its_origin_says() {
    let mut app = app(7, None);
    let (near, far) = {
        let db = app.world().resource::<shards::content::ContentDb>();
        let worldgen = app.world().resource::<shards::world::WorldGen>().clone();
        let make = |id| shards::world::generate::generate(db, &worldgen, id, 7).unwrap().manifest;
        (make("walled"), make("walled_far"))
    };
    // The same tiles: only the place differs, and the canonical text says where.
    assert_eq!(near.tiles, far.tiles);
    assert!(!near.to_canonical().contains("origin") && far.to_canonical().contains("\"origin\": [-200, 64]"));
    let shift = Vec3::new(-200.0, 0.0, 64.0);
    for (a, b) in near.pois.iter().zip(&far.pois) {
        assert!(b.position().distance(a.position() + shift) < 1e-3, "{} at {:?} vs {:?}", a.id, a.pos, b.pos);
    }

    let out = ok(&mut app, "regen", serde_json::json!({"region": "walled_far", "seed": 7}));
    assert!(out.starts_with("ok: walled_far seed 7 v1"), "{out}");
    let hero = hero(&mut app);
    assert!(pos(&app, hero).distance(far.spawn_position()) < 0.01, "the hero at the spawn: {}", pos(&app, hero));
    assert_eq!(region(&app).tile_at(pos(&app, hero)).map(|(cell, _)| cell), Some([0, 0]));
    // A ray down at the middle of the far corner tile hits its ground; the same spot near the
    // world's origin is empty.
    let ground = |app: &mut App, at: Vec3| shards::world::region::ground_below(app.world_mut(), at, &[]);
    let corner = far.cell_center([5, 3]);
    assert_eq!(ground(&mut app, corner), Some(0.0));
    assert_eq!(ground(&mut app, near.cell_center([5, 3])), None, "nothing at the origin");
    let out = ok(&mut app, "goto_tile", serde_json::json!({"x": 5, "y": 3}));
    assert!(out.contains("at tile (5, 3)") && pos(&app, hero).distance(corner) < 0.01, "{out}");
    // The navmesh lies where the tiles are.
    let camp = far.pois.iter().find(|p| p.poi == "camp").unwrap().position();
    let length = route(&app, far.spawn_position(), camp).expect("the spawn reaches the camp");
    assert!(length >= far.spawn_position().distance(camp) * 0.99, "{length}");
    assert!(route(&app, near.spawn_position(), near.cell_center([5, 3])).is_err(), "no navmesh at the origin");
}
