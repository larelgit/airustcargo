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
