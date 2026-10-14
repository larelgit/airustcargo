//! Active zones follow the players; tile visuals stream by distance.

use serde_json::json;
use shards::ai_core::AiId;
use shards::world::stream::TileVisualSpawn;
use shards::world::{ActiveZone, Dormant, TileId, WorldAppExt, WorldSettings};

use shards::prelude::*;

use crate::common::*;

fn guard(app: &mut App, x: f32, z: f32) -> Entity {
    let out = ok(app, "spawn_prefab", json!({"id": "guard", "x": x, "z": z}));
    let id = AiId::parse(out.split_whitespace().nth(1).unwrap()).unwrap();
    app.world().resource::<shards::ai_core::AiIndex>().get(id).unwrap()
}

fn zone(app: &App, e: Entity) -> &'static str {
    match (app.world().get::<ActiveZone>(e).is_some(), app.world().get::<Dormant>(e).is_some()) {
        (true, false) => "active",
        (false, true) => "dormant",
        (false, false) => "unmarked",
        (true, true) => "both",
    }
}

#[test]
fn npcs_far_from_every_player_go_dormant_and_wake_when_one_comes() {
    // The meadow's active radius is 40 m, with the default 8 m margin.
    let mut app = app(42, Some("meadow"));
    let hero = hero(&mut app);
    let at = pos(&app, hero);
    let near = guard(&mut app, at.x + 10.0, at.z);
    let far = guard(&mut app, at.x + 100.0, at.z);
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!((zone(&app, near), zone(&app, far)), ("active", "dormant"));
    assert_eq!(zone(&app, hero), "unmarked", "players are never marked");
    let far_id = app.world().get::<AiId>(far).unwrap().to_string();
    let seen = ok(&mut app, "inspect", json!({"id": far_id}));
    assert!(seen.contains("dormant"), "{seen}");

    // 45 m away: beyond the radius, within the margin; an active guard stays active.
    let hero_id = app.world().get::<AiId>(hero).unwrap().to_string();
    ok(&mut app, "teleport", json!({"id": hero_id, "x": at.x - 35.0, "z": at.z}));
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!(zone(&app, near), "active", "the margin keeps it awake");
    // At the far guard: it wakes, the near one sleeps.
    ok(&mut app, "teleport", json!({"id": hero_id, "x": at.x + 100.0, "z": at.z + 5.0}));
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!((zone(&app, near), zone(&app, far)), ("dormant", "active"));
}

#[test]
fn without_a_region_or_a_radius_nothing_is_marked() {
    let mut app = app(1, None);
    let g = guard(&mut app, 500.0, 0.0);
    ok(&mut app, "step", json!({"ticks": 2}));
    assert_eq!(zone(&app, g), "unmarked");
    app.world_mut().resource_mut::<WorldSettings>().active_radius = Some(100.0);
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!(zone(&app, g), "dormant", "a radius from the settings turns zones on");
}

/// A visual for a tile: remembers what it was made for.
#[derive(Component, Debug)]
struct Shown {
    tile: TileId,
    lod: usize,
}

fn show(world: &mut World, request: &TileVisualSpawn) -> Option<Entity> {
    (request.visual == Some("grass") || request.lod == 0).then(|| world.spawn((Shown { tile: request.tile, lod: request.lod }, Transform::default())).id())
}

fn shown(app: &mut App) -> Vec<(TileId, usize)> {
    let mut v: Vec<(TileId, usize)> = app.world_mut().query::<&Shown>().iter(app.world()).map(|s| (s.tile, s.lod)).collect();
    v.sort_by_key(|(t, lod)| (t.y, t.x, *lod));
    v
}

#[test]
fn tile_visuals_come_and_go_with_the_players() {
    let mut app = app_with(42, Some("meadow"), |app| {
        app.set_tile_visual_spawner(show);
        let mut settings = app.world_mut().resource_mut::<WorldSettings>();
        settings.stream_visuals = Some(true);
        settings.visual_lods = vec![20.0, 40.0];
    });
    app.update();
    let near = shown(&mut app);
    assert!(!near.is_empty() && near.len() < 64, "only tiles near the hero: {near:?}");
    assert!(near.iter().any(|(_, lod)| *lod == 0));
    let hero = hero(&mut app);
    let id = app.world().get::<AiId>(hero).unwrap().to_string();
    ok(&mut app, "teleport", json!({"id": id, "x": 500.0, "z": 500.0}));
    app.update();
    assert!(shown(&mut app).is_empty(), "far from every tile, nothing is drawn");
    ok(&mut app, "teleport", json!({"id": id, "x": 8.0, "z": 8.0}));
    app.update();
    let corner = shown(&mut app);
    assert!(corner.contains(&(TileId { x: 0, y: 0 }, 0)), "{corner:?}");
}
