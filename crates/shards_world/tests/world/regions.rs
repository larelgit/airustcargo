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
    // One seed as a number, as `seeds=7` on a line reads.
    let one = ok(&mut app, "worldgen_report", json!({"region": "meadow", "seeds": 7}));
    assert!(one.starts_with("ok: worldgen meadow v1, seeds 7: 1/1 passed"), "{one}");
    let typo = err(&mut app, "worldgen_report", json!({"region": "medow", "seeds": "1..2"}));
    assert!(typo.contains("did you mean `meadow`?"), "{typo}");
    let seeds = err(&mut app, "worldgen_report", json!({"region": "meadow", "seeds": "ten"}));
    assert!(seeds.contains("both ends included"), "{seeds}");
    let none = err(&mut app, "goto_tile", json!({"x": 0, "y": 0}));
    assert!(none.contains("no region in the world"), "{none}");
}

#[test]
fn a_snapshot_taken_in_a_region_builds_it_again_where_it_is_loaded() {
    use shards::ai_core::snapshot::{capture, restore};
    // A session that changes region, walks the navmesh and is saved on the way.
    let mut original = app(5, Some("meadow"));
    ok(&mut original, "step", json!({"ticks": 10}));
    ok(&mut original, "regen", json!({"region": "crypt", "seed": 2}));
    let boss = region(&original).manifest.pois.iter().find(|p| p.poi == "boss").unwrap().position();
    ok(&mut original, "move_to", json!({"target": {"x": boss.x, "z": boss.z}, "path": true}));
    ok(&mut original, "step", json!({"ticks": 40}));
    let snapshot = capture(original.world()).unwrap();
    ok(&mut original, "step", json!({"ticks": 60}));

    // A fresh game stands in its start meadow: the crypt is built again, with the same ids,
    // before the load compares static colliders.
    let mut copy = app(5, Some("meadow"));
    let report = restore(copy.world_mut(), &snapshot, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert!(report.warnings.iter().any(|w| w.starts_with("region: crypt seed 2 v1 built again from its manifest")), "{:?}", report.warnings);
    assert_eq!(region(&copy).manifest, region(&original).manifest);
    assert!(copy.world().get_resource::<shards::npc::NavRegion>().is_some_and(|nav| nav.hash == region(&copy).hash));
    ok(&mut copy, "step", json!({"ticks": 60}));
    assert_eq!(state_hash(copy.world()), state_hash(original.world()));
    assert_eq!(capture(copy.world()).unwrap().entities, capture(original.world()).unwrap().entities);
    let hero = hero(&mut copy);
    assert!(pos(&copy, hero).distance(snapshot_pos(&snapshot)) > 1.0, "the hero walked on");

    // Loaded where it was taken: the region is already the one, nothing is built.
    let again = restore(copy.world_mut(), &snapshot, false).unwrap();
    assert!(again.exact() && again.warnings.iter().all(|w| !w.starts_with("region:")), "{:?}", again.warnings);

    // A snapshot of a game without a region unloads the region of the game it goes into.
    let bare = capture(app(5, None).world()).unwrap();
    let report = restore(copy.world_mut(), &bare, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert!(copy.world().get_resource::<ActiveRegion>().is_none());
    assert!(report.warnings.iter().any(|w| w.contains("unloaded: the snapshot was taken without one")), "{:?}", report.warnings);

    // A manifest content cannot build is refused before anything changes.
    let mut foreign = snapshot.clone();
    let key = "shards_world::region::RegionBuild";
    let mut record: serde_json::Value = foreign.resources[key].clone();
    record["manifest"] = json!(record["manifest"].as_str().unwrap().replace("\"tile\": \"", "\"tile\": \"lost_"));
    foreign.resources.insert(key.into(), record);
    let mut third = app(5, Some("meadow"));
    let err = restore(third.world_mut(), &foreign, false).unwrap_err();
    assert!(err.starts_with("cannot put back the region the snapshot was taken in: the manifest of crypt seed 2 v1 names rooms content does not have: lost_"), "{err}");
    assert_eq!(region(&third).manifest.region, "meadow", "the meadow stays");
}

/// Where the snapshot's player stood.
fn snapshot_pos(snapshot: &shards::ai_core::Snapshot) -> Vec3 {
    let hero = snapshot.entities.iter().find(|e| e.components.contains_key("world::common::Hero")).expect("the hero");
    let t = &hero.components["bevy_transform::components::transform::Transform"]["translation"];
    Vec3::new(t[0].as_f64().unwrap() as f32, t[1].as_f64().unwrap() as f32, t[2].as_f64().unwrap() as f32)
}

/// `WorldRebuilt` reasons, in order.
#[derive(Resource, Default)]
struct Rebuilt(Vec<String>);

#[test]
fn a_held_seed_holds_the_start_region_until_the_hosts_seed_is_adopted() {
    let mut app = fresh(1, Some("meadow"));
    app.init_resource::<Rebuilt>()
        .add_observer(|rebuilt: On<shards::ai_core::WorldRebuilt>, mut seen: ResMut<Rebuilt>| seen.0.push(rebuilt.reason.clone()));
    shards::core::hold_seed(app.world_mut(), "net: waiting for the host's seed");
    let waiting = testing::wait_until_loaded(&mut app, std::time::Duration::from_secs(2)).unwrap_err();
    assert!(waiting.contains("seed: net: waiting for the host's seed"), "{waiting}");
    assert!(db(&app).is_loaded(), "content loaded meanwhile");
    assert!(app.world().get_resource::<ActiveRegion>().is_none(), "no region before the seed is known");
    assert!(!shards::core::seed_ready(app.world()));

    shards::core::adopt_seed(app.world_mut(), 42);
    testing::wait_until_loaded(&mut app, testing::LOAD_TIMEOUT).unwrap();
    let expected = generate(db(&app), &worldgen(&app), "meadow", 42).unwrap().manifest;
    assert_eq!(region(&app).manifest, expected, "built with the adopted seed");
    assert_eq!(app.world().resource::<shards::ai_core::SimSeed>().0, 42);
    assert_eq!(app.world().resource::<EngineConfig>().seed, 42);
    assert_eq!(app.world().resource::<Rebuilt>().0, ["region meadow"]);
    hero(&mut app);

    ok(&mut app, "regen", json!({"seed": 5}));
    assert_eq!(app.world().resource::<Rebuilt>().0, ["region meadow", "region meadow"], "a region change rebuilds the world");
}

#[test]
fn an_edge_wall_keeps_a_body_in_the_region_and_an_open_edge_lets_it_fall() {
    for walled in [false, true] {
        let mut app = app(7, None);
        if walled {
            let db = edited_db(&app, "world/meadow.region.ron", |t| t.replacen("size: (6, 4),", "size: (6, 4),\n        edge_wall: 3.0,", 1));
            assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
            app.world_mut().insert_resource(db);
        }
        ok(&mut app, "regen", json!({"region": "walled", "seed": 3}));
        let info = testing::ok(&mut app, "region_info", json!({})).result;
        assert_eq!(info.get("edge").and_then(|e| e.as_str()), walled.then_some("walled, 3m high"), "{info}");
        // The gate is the north-west cell: west of it is the edge.
        let corner = region(&app).manifest.corner();
        let hero = hero(&mut app);
        ok(&mut app, "move", json!({"dir": "W", "meters": 40}));
        ok(&mut app, "step", json!({"ticks": 600}));
        let at = pos(&app, hero);
        if walled {
            assert!(at.x > corner.x && at.y > -0.5, "the wall held it in the region: {at} (edge at x = {})", corner.x);
        } else {
            assert!(at.x < corner.x && at.y < -5.0, "with an open edge it walks off the end of the world: {at}");
        }
    }
}

#[test]
fn an_edge_wall_takes_no_collider_id_from_the_tiles_and_goes_with_the_region() {
    let colliders = |app: &mut App| {
        let mut ids: Vec<u32> =
            app.world_mut().query_filtered::<&ColliderId, With<RegionPart>>().iter(app.world()).map(|c| c.0).collect();
        ids.sort_unstable();
        ids
    };
    let mut open = app(7, None);
    ok(&mut open, "regen", json!({"region": "walled", "seed": 3}));
    let without = colliders(&mut open);

    let mut app = app(7, None);
    let db = edited_db(&app, "world/meadow.region.ron", |t| t.replacen("size: (6, 4),", "size: (6, 4),\n        edge_wall: 3.0,", 1));
    app.world_mut().insert_resource(db);
    ok(&mut app, "regen", json!({"region": "walled", "seed": 3}));
    let with = colliders(&mut app);
    assert_eq!(with.len(), without.len() + 4, "four walls");
    assert_eq!(with[..without.len()], without[..], "the tiles keep their ids: the walls come last");
    // Marked, so a game that draws every collider can leave them out.
    let marked = app.world_mut().query_filtered::<&Name, With<shards::world::RegionEdge>>().iter(app.world()).filter(|n| n.as_str() == "region edge").count();
    assert_eq!(marked, 4);
    // A region change takes the walls away with the rest of the region.
    ok(&mut app, "regen", json!({"region": "meadow", "seed": 3}));
    let walls = app.world_mut().query::<&Name>().iter(app.world()).filter(|n| n.as_str() == "region edge").count();
    assert_eq!(walls, 0, "meadow has an open edge, and the walled region's walls are gone");
}
