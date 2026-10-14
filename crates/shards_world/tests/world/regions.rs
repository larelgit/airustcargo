//! Regions in a running game: the start region before the game's own world, the same ids
//! every run, region changes through `RegionLoading` that keep the player and the tick
//! count, replays that regenerate mid-session, manifests loaded back, tools and observe.

use serde_json::json;
use shards::ai_core::replay::{self, state_hash};
use shards::ai_core::{AiId, AiIndex, AiLandmark};
use shards::motion::{ColliderId, PhysicsWorld};
use shards::testing;
use shards::world::generate::generate;
use shards::world::{Poi, RegionManifest, RegionPart, TileId};

use shards::prelude::*;

use crate::common::*;

/// Times `OnEnter(RegionLoading)` ran.
#[derive(Resource, Default)]
struct Screens(u32);

/// A stand-in for the levels module: one wall per level file, where the tile says.
fn fake_levels(world: &mut World, request: &shards::world::LevelSpawn) -> Result<Vec<Entity>, String> {
    assert_eq!(request.path, "levels/crypt_door.glb");
    let wall = shards::motion::Collider::new(shards::motion::Shape::Cuboid { half: Vec3::ONE });
    Ok(vec![world.spawn((wall, request.transform, Name::new(format!("level {}", request.tile_id)))).id()])
}

#[test]
fn tiles_built_from_level_files_go_through_the_level_loader() {
    let mut app = app(1, None);
    let level = r#"{ "door": (sockets: (n: "grass", e: "grass", s: "grass", w: "grass"), max_count: 0,
        geometry: Level(path: "levels/crypt_door.glb")) }"#;
    let with_level = db_with(&app, &[("level.tile.ron", level)]);
    let errors: Vec<String> = with_level.errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(shards::world::region::level_paths(&with_level), ["levels/crypt_door.glb"]);
    app.world_mut().insert_resource(with_level);
    let mut m = generate(db(&app), &worldgen(&app), "meadow", 1).unwrap().manifest;
    assert!(m.tiles.iter().all(|t| t.tile != "door"), "max_count 0 keeps it out of generated regions");
    m.tiles[0].tile = "door".into();
    let refused = err(&mut app, "region_load", json!({"manifest": m.to_canonical()}));
    assert!(refused.contains("tile `door` is built from the level file `levels/crypt_door.glb`, and no level loader is registered"), "{refused}");
    assert!(app.world().get_resource::<ActiveRegion>().is_none(), "nothing was left behind");
    assert_eq!(app.world_mut().query_filtered::<(), With<RegionPart>>().iter(app.world()).count(), 0);

    app.world_mut().insert_resource(shards::world::region::LevelSpawner(fake_levels));
    ok(&mut app, "region_load", json!({"manifest": m.to_canonical()}));
    let walls: Vec<(String, TileId)> =
        app.world_mut().query::<(&Name, &TileId)>().iter(app.world()).filter(|(n, _)| n.starts_with("level ")).map(|(n, t)| (n.to_string(), *t)).collect();
    assert_eq!(walls, [("level door".to_owned(), TileId { x: 0, y: 0 })], "tagged with its tile, despawned with the region");
}

#[test]
fn the_start_region_stands_before_the_game_spawns_its_hero() {
    let mut app = app(42, Some("meadow"));
    let expected = generate(db(&app), &worldgen(&app), "meadow", 42).unwrap().manifest;
    assert_eq!(region(&app).manifest, expected, "built from what the generator makes for the world seed");
    let hero = hero(&mut app);
    let hero_id = *app.world().get::<AiId>(hero).unwrap();
    let parts: Vec<AiId> =
        app.world_mut().query_filtered::<&AiId, With<RegionPart>>().iter(app.world()).copied().collect();
    assert!(!parts.is_empty() && parts.iter().all(|id| *id < hero_id), "region ids come first: {parts:?} < {hero_id}");
    let spawn = region(&app).spawn_point(0);
    assert!(pos(&app, hero).distance(spawn) < 0.01, "the hero stands at the spawn");
    let landmarks = app.world_mut().query_filtered::<&Poi, With<AiLandmark>>().iter(app.world()).count();
    assert_eq!(landmarks, expected.pois.len());

    ok(&mut app, "step", json!({"ticks": 20}));
    assert!(app.world().resource::<PhysicsWorld>().len() >= 64, "every tile's ground collides");
    assert!(pos(&app, hero).y.abs() < 0.05, "the hero stands on the ground: {}", pos(&app, hero));
    let seen = ok(&mut app, "observe", json!({}));
    let tile = region(&app).tile_at(spawn).map(|(cell, t)| format!("TILE ({}, {}) {} rot {} · meadow seed 42 v1", cell[0], cell[1], t.tile, t.rot));
    assert!(seen.contains(&tile.unwrap()), "{seen}");
    assert!(seen.contains("POI  ") && seen.contains("spawn.0 0.0m here"), "{seen}");
    assert!(seen.contains("LANDMARKS") && seen.contains("Spawn"), "{seen}");
    let text = journal(&app).join("\n");
    assert!(text.contains("region generated: meadow seed 42 v1 (attempt "), "{text}");
    assert!(text.contains("region loaded: meadow seed 42 v1 (64 tiles, "), "{text}");
    let at_spawn = testing::ok(&mut app, "step", json!({"ticks": 30, "until": [{"named": "near_poi"}]}));
    assert_eq!(at_spawn.tick, 21, "standing at the spawn point stops the step at once: {}", at_spawn.result);
    let status = shards::ai_core::status::status(app.world());
    assert_eq!(status["world"]["region"], "meadow");
    assert_eq!(status["world"]["seed"], 42);
    assert!(status["engine"]["modules"].to_string().contains("world"));
}

#[test]
fn the_same_seed_builds_the_same_world_down_to_ids() {
    let snapshot = |app: &mut App| -> Vec<(u32, String, [i32; 2], u32)> {
        let mut parts: Vec<(u32, String, [i32; 2], u32)> = app
            .world_mut()
            .query_filtered::<(Option<&AiId>, &Name, &TileId, Option<&ColliderId>), With<RegionPart>>()
            .iter(app.world())
            .map(|(id, name, tile, collider)| (id.map_or(0, |i| i.0), name.to_string(), [tile.x, tile.y], collider.map_or(0, |c| c.0)))
            .collect();
        parts.sort();
        parts
    };
    let mut a = app(9, Some("meadow"));
    let mut b = app(9, Some("meadow"));
    assert_eq!(snapshot(&mut a), snapshot(&mut b));
    ok(&mut a, "step", json!({"ticks": 30}));
    ok(&mut b, "step", json!({"ticks": 30}));
    assert_eq!(state_hash(a.world()), state_hash(b.world()));
}

#[test]
fn regen_replaces_the_region_through_the_loading_screen_and_keeps_the_hero() {
    let mut app = app_with(3, Some("meadow"), |app| {
        app.init_resource::<Screens>().add_systems(OnEnter(EngineState::RegionLoading), |mut n: ResMut<Screens>| n.0 += 1);
    });
    ok(&mut app, "step", json!({"ticks": 10}));
    let hero = hero(&mut app);
    let hero_id = *app.world().get::<AiId>(hero).unwrap();
    let old_poi = app.world_mut().query_filtered::<&AiId, With<Poi>>().iter(app.world()).next().copied().unwrap();

    let out = ok(&mut app, "regen", json!({"seed": 99}));
    assert!(out.starts_with("ok: meadow seed 99 v1 (attempt "), "{out}");
    assert_eq!(app.world().resource::<Screens>().0, 1, "the loading screen came up");
    assert_eq!(*app.world().resource::<State<EngineState>>().get(), EngineState::InGame, "and went");
    assert!(testing::loading(app.world()).is_none(), "nothing else loads: the change is done");
    assert_eq!(ai_core_tick(&app), 10, "no tick went by");
    assert_eq!(region(&app).manifest.seed, 99);
    assert_eq!(heroes(&mut app), 1, "OnEnter(InGame) did not run again");
    assert_eq!(*app.world().get::<AiId>(hero).unwrap(), hero_id, "the same hero");
    assert!(pos(&app, hero).distance(region(&app).spawn_point(0)) < 0.01, "at the new spawn");
    let gone = app.world().resource::<AiIndex>().resolve(old_poi).unwrap_err();
    assert!(gone.contains("no longer exists"), "{gone}");

    app.update();
    let text = journal(&app).join("\n");
    for line in ["region unloaded: meadow seed 3 v1", "region generated: meadow seed 99 v1", "state InGame -> RegionLoading", "state RegionLoading -> InGame"] {
        assert!(text.contains(line), "{line} in\n{text}");
    }
    ok(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(ai_core_tick(&app), 20);
    // Without a seed: the next one of the same region; another region: the world seed.
    assert!(ok(&mut app, "regen", json!({})).starts_with("ok: meadow seed 100 v1"));
    assert!(ok(&mut app, "regen", json!({"region": "crypt"})).starts_with("ok: crypt seed 3 v1"));
    let unknown = err(&mut app, "regen", json!({"region": "medow"}));
    assert!(unknown.contains("unknown region `medow` (did you mean `meadow`?)"), "{unknown}");
}

fn ai_core_tick(app: &App) -> u64 {
    shards::ai_core::sim::tick(app.world())
}

#[test]
fn a_session_that_changes_regions_replays_identically() {
    let mut app = app(5, Some("meadow"));
    ok(&mut app, "step", json!({"ticks": 30}));
    ok(&mut app, "move", json!({"dir": "E", "meters": 10}));
    ok(&mut app, "step", json!({"ticks": 20}));
    ok(&mut app, "regen", json!({"seed": 11}));
    ok(&mut app, "step", json!({"ticks": 70}));
    let [x, y] = region(&app).manifest.cell_of(region(&app).spawn_point(0));
    ok(&mut app, "goto_tile", json!({"x": x, "y": y}));
    ok(&mut app, "regen", json!({"region": "crypt", "seed": 2}));
    ok(&mut app, "step", json!({"ticks": 45}));
    let recording = replay::save(app.world());
    assert_eq!(recording.end.tick, 165);
    assert!(recording.checks.len() >= 2);

    let mut fresh = fresh(5, Some("meadow"));
    let report = testing::verify(&mut fresh, recording).expect("plays");
    assert!(report.identical(), "{report}");
    assert_eq!(region(&fresh).manifest.region, "crypt");
}

#[test]
fn a_saved_manifest_loads_back_as_it_was() {
    let mut app = app(8, Some("meadow"));
    let info = testing::ok(&mut app, "region_info", json!({"manifest": true})).result;
    assert_eq!(info["region"], "meadow seed 8 v1");
    let saved: RegionManifest = serde_json::from_value(info["manifest"].clone()).unwrap();
    let before: Vec<(String, [i32; 2])> = poi_cells(&mut app);
    ok(&mut app, "regen", json!({"seed": 1}));
    assert_ne!(poi_cells(&mut app), before);
    let out = ok(&mut app, "region_load", json!({"manifest": saved.to_canonical()}));
    assert!(out.starts_with("ok: meadow seed 8 v1 loaded from its manifest (64 tiles"), "{out}");
    assert_eq!(region(&app).hash, saved.hash());
    assert_eq!(poi_cells(&mut app), before);
    let mut foreign = saved.clone();
    foreign.tiles[0].tile = "lava".into();
    let refused = err(&mut app, "region_load", json!({"manifest": serde_json::to_value(&foreign).unwrap()}));
    assert!(refused.contains("names tiles content does not have: lava"), "{refused}");
    assert_eq!(region(&app).hash, saved.hash(), "a refused manifest changes nothing");
}

fn poi_cells(app: &mut App) -> Vec<(String, [i32; 2])> {
    let mut cells: Vec<(String, [i32; 2])> =
        app.world_mut().query::<(&Poi, &TileId)>().iter(app.world()).map(|(p, t)| (p.id.clone(), [t.x, t.y])).collect();
    cells.sort();
    cells
}

#[test]
fn goto_tile_lands_on_the_ground_of_a_tile() {
    let mut app = app(42, Some("meadow"));
    let spawn_cell = region(&app).manifest.cell_of(region(&app).spawn_point(0));
    let out = ok(&mut app, "goto_tile", json!({"x": spawn_cell[0], "y": spawn_cell[1]}));
    assert!(out.contains(&format!("at tile ({}, {})", spawn_cell[0], spawn_cell[1])), "{out}");
    let hero = hero(&mut app);
    let center = region(&app).manifest.cell_center(spawn_cell);
    assert!(pos(&app, hero).distance(center) < 0.01, "{} vs {center}", pos(&app, hero));
    let outside = err(&mut app, "goto_tile", json!({"x": 9, "y": 0}));
    assert!(outside.contains("(9, 0) is outside meadow seed 42 v1 (tiles 0..8 × 0..8)"), "{outside}");
}

#[test]
fn worldgen_report_runs_in_a_live_game() {
    let mut app = app(1, None);
    let out = ok(&mut app, "worldgen_report", json!({"region": "meadow", "seeds": "1..20"}));
    assert!(out.starts_with("ok: worldgen meadow v1, seeds 1..20: 20/20 passed"), "{out}");
    let typo = err(&mut app, "worldgen_report", json!({"region": "medow", "seeds": "1..2"}));
    assert!(typo.contains("did you mean `meadow`?"), "{typo}");
    let seeds = err(&mut app, "worldgen_report", json!({"region": "meadow", "seeds": "ten"}));
    assert!(seeds.contains("both ends included"), "{seeds}");
    let none = err(&mut app, "goto_tile", json!({"x": 0, "y": 0}));
    assert!(none.contains("no region in the world"), "{none}");
}
