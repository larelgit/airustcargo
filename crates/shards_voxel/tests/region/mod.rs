//! The voxel region kind on the whole engine: a region of the `voxel_region` content kind
//! generated, built, reported, snapshot, saved, reloaded and played in co-op through the region
//! API tiles use. The content is written for each test (nothing under the sandbox's content).

use std::path::Path;

use serde_json::{json, Value};
use shards::ai_core::snapshot::{capture, restore};
use shards::ai_core::{replay, AiConditions, AiJournal};
use shards::content::ContentDb;
use shards::motion::{ColliderId, Current, Volume};
use shards::net::{LinkConditions, NetHost};
use shards::prelude::*;
use shards::save::SaveConfig;
use shards::testing;
use shards::voxel::region::{cached_volumes, clear_cache};
use shards::voxel::{VoxelAppExt, VoxelGen, VoxelPalette, VoxelRegionDef, VoxelSettings, VoxelTerrain};
use shards::world::manifest::Layout;
use shards::world::{Marker, Poi, RegionManifest, RegionPart, TileId, WorldGen};

use super::Folder;

pub const PALETTE: &str = r##"{
    "test": (
        blocks: [
            (name: "grass", top: "#5b8f3a", tags: ["ground"]),
            (name: "dirt", top: "#6f5a3c"),
            (name: "sand", top: "#d8c890"),
            (name: "water", top: "#2a6f9f", solid: false, liquid: true),
        ],
    ),
}"##;

/// A 32 x 16 x 32 island of 16-cell chunks around (300, 0, 300): hills, a moat of water 4 cells
/// wide inside its edge, a flat plaza at its middle with the spawn, a camp and a home marker, and
/// a sea with a current around it. Layer 6 is at world y 0.
pub const ISLE: &str = r##"{
    "isle": (
        size: (32, 16, 32),
        chunk: (16, 16, 16),
        palette: "test",
        anchor: Some((16, 6, 16)),
        at: (300.0, 0.0, 300.0),
        attempts: 4,
        steps: [("noise_hills", 1), ("plaza", 1), ("fill_water", 1)],
        params: {
            "noise_hills": (top: "grass", fill: "dirt", base: 7.0, amplitude: 4.0, frequency: 0.09),
            "plaza": (radius: 4.0, height: 7, moat: 4),
            "fill_water": (level: 6),
        },
        checks: ["plaza_dry"],
        marks: ["camp.0"],
        markers: ["home"],
        spawn: Some("spawn.0"),
        water: (surface_drop: 0.05),
        sea: Some((level: 6, reach: 600.0, floor: 2.0, current: Some((starts: 6.0, full: 1.5)))),
        near_edge: 6.0,
    ),
}"##;

#[derive(serde::Deserialize)]
struct PlazaParams {
    radius: f32,
    height: u32,
    moat: i32,
}

/// The plaza: the middle flattened at `height`, the spawn on it, a camp and a home nearby; the
/// columns within `moat` cells of the edge lowered to layer 4 (sand), for the water to fill.
fn plaza(g: &mut VoxelGen) -> Result<(), String> {
    let p: PlazaParams = g.params()?;
    let (grass, dirt, sand) = (g.palette().id("grass").ok_or("no grass")?, g.palette().id("dirt").ok_or("no dirt")?, g.palette().id("sand").ok_or("no sand")?);
    let size = g.volume().size();
    let (sx, sz) = (size.x as i32, size.z as i32);
    for x in 0..sx {
        for z in 0..sz {
            if x.min(z).min(sx - 1 - x).min(sz - 1 - z) < p.moat {
                g.volume_mut().lower_column(x, z, 4, sand);
            }
        }
    }
    let mid = Vec2::new(size.x as f32 * 0.5, size.z as f32 * 0.5);
    g.volume_mut().flatten(mid, p.radius, p.height, dirt, grass);
    let on_top = |g: &VoxelGen, x: i32, z: i32| g.volume().corner(IVec3::new(x, g.volume().surface(x, z) as i32, z)) + Vec3::new(0.5, 0.0, 0.5);
    let (cx, cz) = (sx / 2, sz / 2);
    let (spawn, camp, home) = (on_top(g, cx, cz), on_top(g, cx + 2, cz + 2), on_top(g, cx - 2, cz));
    g.mark("spawn.0", spawn);
    g.mark("camp.0", camp);
    g.mark("home", home);
    Ok(())
}

/// The plaza stands above the water.
fn plaza_dry(g: &VoxelGen) -> Result<(), String> {
    let size = g.volume().size();
    let (x, z) = (size.x as i32 / 2, size.z as i32 / 2);
    let top = g.volume().get(IVec3::new(x, g.volume().surface(x, z) as i32 - 1, z));
    if g.palette().liquid(top) || !g.palette().solid(top) { Err("the plaza is not dry ground".into()) } else { Ok(()) }
}

/// Passes only for seed 1: every other seed falls back.
fn only_seed_one(g: &VoxelGen) -> Result<(), String> {
    if g.seed() == 1 { Ok(()) } else { Err(format!("seed {} is not 1", g.seed())) }
}

/// The player marker of the test game.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Hero;

/// The game: its steps and a hero (at the origin until a region moves it).
pub fn setup(app: &mut App) {
    app.add_voxel_step("plaza", 1, plaza).add_voxel_check("plaza_dry", plaza_dry).add_voxel_check("only_seed_one", only_seed_one);
    app.register_type::<Hero>().add_systems(OnEnter(EngineState::InGame), |mut commands: Commands| {
        commands.spawn((Hero, character("", Vec3::ZERO), AiPlayer, PlayerId(0), AiKind::new("Player"), Name::new("Player")));
    });
}

pub fn folder(name: &str, isle: &str) -> Folder {
    Folder::new(name, &[("content/test.voxel_palette.ron", PALETTE), ("content/world/isle.voxel_region.ron", isle)])
}

pub fn game(folder: &Folder) -> App {
    game_with(folder, |_| {}, |_| {})
}

pub fn game_with(folder: &Folder, configure: impl FnOnce(&mut EngineConfig), more: impl FnOnce(&mut App)) -> App {
    let dir = folder.path().to_string_lossy().into_owned();
    let app = testing::test_app(
        move |config| {
            config.asset_dir = dir;
            configure(config);
        },
        |app| {
            setup(app);
            more(app);
        },
    );
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "test content has errors: {errors:#?}");
    app
}

pub fn ok(app: &mut App, tool: &str, args: Value) -> Value {
    testing::ok(app, tool, args).result
}

pub fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_else(|| panic!("not a text: {value}"))
}

pub fn err(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail: {}", out.result);
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

pub fn journal(world: &World) -> Vec<String> {
    world.resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect()
}

fn lines(journal: &[String], needle: &str) -> Vec<String> {
    journal.iter().filter(|l| l.contains(needle)).cloned().collect()
}

pub fn manifest(app: &App) -> RegionManifest {
    app.world().resource::<ActiveRegion>().manifest.clone()
}

pub fn hero(app: &mut App) -> Entity {
    app.world_mut().query_filtered::<Entity, With<Hero>>().single(app.world()).expect("one hero")
}

fn feet(app: &mut App) -> Vec3 {
    let hero = hero(app);
    app.world().get::<Transform>(hero).unwrap().translation
}

fn terrain(app: &App) -> &VoxelTerrain {
    app.world().resource::<VoxelTerrain>()
}

/// The region's parts that have a collider, in collider id order: (id, name).
fn colliders(app: &mut App) -> Vec<(ColliderId, String)> {
    let mut q = app.world_mut().query_filtered::<(&ColliderId, &Name), With<RegionPart>>();
    let mut out: Vec<(ColliderId, String)> = q.iter(app.world()).map(|(id, name)| (*id, name.to_string())).collect();
    out.sort();
    out
}

fn holds(app: &App, name: &str) -> bool {
    let condition = app.world().resource::<AiConditions>().0.get(name).copied().unwrap_or_else(|| panic!("no condition {name}"));
    condition(app.world())
}

#[test]
fn a_voxel_region_is_generated_built_and_answered_for_like_a_region_of_tiles() {
    let folder = folder("build", ISLE);
    let mut app = game(&folder);
    // The conditions are there before any voxel region (content may name them in any region),
    // and hold nowhere yet.
    for name in ["in_water", "in_current", "near_grid_edge"] {
        assert!(!holds(&app, name), "{name} with no region");
    }
    let out = ok(&mut app, "regen", json!({"region": "isle", "seed": 7}));
    assert!(text(&out).starts_with("ok: isle seed 7 v1 (attempt 1, 4 chunks, 2 POIs, "), "{out}");

    let m = manifest(&app);
    assert_eq!((m.layout, m.size, m.tile_size, m.tiles.len()), (Layout::Volume, [2, 2], 16.0, 0));
    assert_eq!((m.origin, m.spawn.as_deref(), m.attempt, m.fallback.as_deref()), ([284.0, 284.0], Some("spawn.0"), 1, None));
    assert_eq!(m.steps, vec![("noise_hills".to_owned(), 1), ("plaza".to_owned(), 1), ("fill_water".to_owned(), 1)]);
    let pois: Vec<(&str, &str, [i32; 2])> = m.pois.iter().map(|p| (p.id.as_str(), p.poi.as_str(), p.cell)).collect();
    assert_eq!(pois, vec![("spawn.0", "spawn", [1, 1]), ("camp.0", "camp", [1, 1])], "every mark but the markers is a point of interest");
    let back = RegionManifest::from_json(&m.to_canonical()).unwrap();
    assert_eq!(back, m, "the canonical text reads back to the same manifest");

    let story = journal(app.world());
    let loaded = lines(&story, "region loaded: isle seed 7 v1 (32 x 16 x 32, ");
    assert_eq!(loaded.len(), 1, "{story:#?}");
    assert!(loaded[0].contains(" water box") && loaded[0].contains(", 4 sea boxes, "), "{}", loaded[0]);
    assert_eq!(lines(&story, "region generated: isle seed 7 v1").len(), 1, "{story:#?}");

    // What was spawned, in its order: the terrain, the water, the sea, the marker, then the points.
    let names: Vec<String> = colliders(&mut app).into_iter().map(|(_, n)| n).collect();
    assert_eq!(names[0], "voxel terrain isle", "{names:?}");
    let water = names.iter().filter(|n| n.starts_with("voxel water ")).count();
    assert!(water > 0, "the moat: {names:?}");
    assert_eq!(names.iter().filter(|n| n.starts_with("voxel sea ")).count(), 4, "{names:?}");
    assert!(names[1..=water + 4].iter().all(|n| n.starts_with("voxel water ") || n.starts_with("voxel sea ")), "{names:?}");
    let t = terrain(&app);
    assert_eq!((t.region.as_str(), t.seed, t.made_with, t.water_boxes, t.sea_boxes), ("isle", 7, 7, water, 4));
    let currents = app.world_mut().query_filtered::<&Current, With<Volume>>().iter(app.world()).count();
    assert_eq!(currents, water + 4, "every water volume carries the current");
    let markers: Vec<(String, TileId)> = app.world_mut().query::<(&Marker, &TileId)>().iter(app.world()).map(|(m, t)| (m.name.clone(), *t)).collect();
    assert_eq!(markers, vec![("home".to_owned(), TileId { x: 0, y: 1 })], "in its chunk, as a tile's marker is in its tile");
    let mut points: Vec<String> = app.world_mut().query::<&Poi>().iter(app.world()).map(|p| p.id.clone()).collect();
    points.sort();
    assert_eq!(points, vec!["camp.0".to_owned(), "spawn.0".to_owned()]);

    // The player stands on the plaza: its top is layer 7, world y 1.
    assert_eq!(feet(&mut app), Vec3::new(300.5, 1.0, 300.5), "on the plaza");
    let info = ok(&mut app, "region_info", json!({}));
    assert_eq!(info["size"], "2×2 chunks of 16m", "{info}");
    let goto = ok(&mut app, "goto_tile", json!({"x": 1, "y": 1}));
    assert!(text(&goto).contains("at chunk (1, 1)"), "{goto}");
    let outside = err(&mut app, "goto_tile", json!({"x": 2, "y": 0}));
    assert!(outside.contains("is outside isle seed 7 v1 (chunks 0..2 × 0..2)"), "{outside}");
    let observed = ok(&mut app, "observe", json!({}));
    assert!(observed.to_string().contains("chunk"), "{observed}");

    // The developer's view.
    let info = ok(&mut app, "voxel_info", json!({}));
    assert_eq!(info["size"], "32 x 16 x 32 cells of 1 m", "{info}");
    assert_eq!(info["chunks"], "2 x 1 x 2");
    assert_eq!(info["steps"], json!(["noise_hills v1", "plaza v1", "fill_water v1"]));
    assert!(info["wide_volumes"].as_u64().unwrap() >= 2, "the north and south sea boxes are wider than the broad phase's widest cell: {info}");
    assert!(info["blocks"]["water"].as_u64().unwrap() > 0 && info["blocks"]["grass"].as_u64().unwrap() > 0, "{info}");
    assert!(info["nav"].as_str().unwrap().contains("nav not baked yet"), "{info}");

    // The current: nothing on the plaza, half 3.75 m in, full near the edge and in the sea;
    // toward the middle.
    let at = |app: &mut App, p: [f32; 3]| ok(app, "sea_current_at", json!({"at": p}));
    assert_eq!(at(&mut app, [300.0, 1.5, 300.0])["strength"], 0.0);
    let half = at(&mut app, [287.75, -0.5, 300.0]);
    assert_eq!((half["strength"].as_f64(), half["current"][0].as_f64(), half["edge"].as_f64()), (Some(0.5), Some(0.5), Some(3.75)), "{half}");
    let near = at(&mut app, [285.0, -0.5, 300.0]);
    assert_eq!((near["strength"].as_f64(), near["current"][0].as_f64()), (Some(1.0), Some(1.0)), "pushed east, inward: {near}");
    let sea = at(&mut app, [250.0, -0.5, 300.0]);
    assert_eq!((sea["strength"].as_f64(), sea["edge"].as_f64()), (Some(1.0), Some(-34.0)), "{sea}");
    let dry = at(&mut app, [285.0, 5.0, 300.0]);
    assert_eq!(dry["strength"], 0.0, "above the water: {dry}");
    let bad = err(&mut app, "sea_current_at", json!({"at": [1.0, f64::MAX, 3.0]}));
    assert!(bad.contains("sea_current_at at=[x,y,z]"), "{bad}");

    // The conditions, now that a voxel region exists.
    assert!(!holds(&app, "near_grid_edge") && !holds(&app, "in_current") && !holds(&app, "in_water"), "on the plaza");
    let hero = hero(&mut app);
    app.world_mut().get_mut::<Transform>(hero).unwrap().translation = Vec3::new(286.0, -0.5, 300.0);
    assert!(holds(&app, "near_grid_edge"), "2 m from the edge, within 6");
    assert!(holds(&app, "in_current") && holds(&app, "in_water"), "in the moat near the edge");
    app.world_mut().get_mut::<Transform>(hero).unwrap().translation = Vec3::new(300.5, 1.0, 300.5);

    // Navigation: no navmesh yet, said plainly.
    let nav = ok(&mut app, "nav_info", json!({}));
    assert_eq!(nav["navmesh"], "none", "{nav}");
    assert!(nav["why"].as_str().unwrap().contains("voxel region, nav not baked yet"), "{nav}");
    let refused = err(&mut app, "nav_path", json!({"to": {"x": 290.0, "z": 290.0}}));
    assert!(refused.contains("no navmesh for this region (voxel region, nav not baked yet)") && refused.contains("use path_to"), "{refused}");
    let bake = testing::call(&mut app, "nav_bake", json!({}));
    assert!(bake.result.to_string().contains("voxel region, nav not baked yet"), "{}", bake.result);
    let story = journal(app.world());
    assert_eq!(lines(&story, "navmesh: none for this region: voxel region, nav not baked yet").len(), 1, "{story:#?}");

    // An unknown region names the voxel ones too.
    let unknown = err(&mut app, "worldgen_report", json!({"region": "nowhere"}));
    assert!(unknown.contains("isle"), "{unknown}");
}

#[test]
fn every_seed_of_a_report_generates_and_the_report_counts_what_it_made() {
    let folder = folder("report", ISLE);
    let mut app = game(&folder);
    let out = ok(&mut app, "worldgen_report", json!({"region": "isle", "seeds": "1..40"}));
    let report = text(&out);
    assert!(report.starts_with("ok: worldgen isle v1, seeds 1..40: 40/40 passed"), "{report}");
    assert!(report.contains("totals: ") && report.contains("solid cells ") && report.contains("water boxes ") && report.contains("marks 120 (3 a region)"), "{report}");

    // A check that passes only for seed 1: every attempt of the others fails by name, and the
    // fallback seed stands in.
    let fallback = ISLE.replace("checks: [\"plaza_dry\"]", "checks: [\"plaza_dry\", \"only_seed_one\"], fallback_seed: Some(1)");
    let folder = folder_with("report_fallback", &fallback);
    let mut app = game(&folder);
    let out = testing::call(&mut app, "worldgen_report", json!({"region": "isle", "seeds": "2..4"}));
    let report = text(&out.result);
    assert!(report.contains("seeds 2..4: 0/3 passed (0 after a retry), 3 fell back, 0 failed"), "{report}");
    assert!(report.contains("12× check only_seed_one"), "four attempts of three seeds: {report}");
    let made = ok(&mut app, "regen", json!({"region": "isle", "seed": 5}));
    assert!(text(&made).contains("the fallback `seed 1`: no attempt of 4 passed"), "{made}");
    let m = manifest(&app);
    assert_eq!((m.attempt, m.fallback.as_deref()), (0, Some("seed 1")));
    assert_eq!((terrain(&app).seed, terrain(&app).made_with), (5, 1));

    // Without a fallback seed, a seed that fails says why and what to do.
    let none = ISLE.replace("checks: [\"plaza_dry\"]", "checks: [\"plaza_dry\", \"only_seed_one\"]");
    let folder = folder_with("report_none", &none);
    let mut app = game(&folder);
    let refused = err(&mut app, "regen", json!({"region": "isle", "seed": 5}));
    assert!(refused.contains("voxel region `isle` seed 5: no attempt passed (4 attempts)") && refused.contains("fallback_seed"), "{refused}");
}

#[test]
fn a_snapshot_taken_in_a_voxel_region_builds_it_again_where_it_is_loaded() {
    let folder = folder("snapshot", ISLE);
    let mut original = game(&folder);
    ok(&mut original, "regen", json!({"region": "isle", "seed": 7}));
    ok(&mut original, "move", json!({"dir": "W", "meters": 3.0}));
    ok(&mut original, "step", json!({"ticks": 20}));
    let snapshot = capture(original.world()).unwrap();
    assert!(snapshot.checks.contains_key("voxel.volume"), "{:?}", snapshot.checks.keys());
    ok(&mut original, "step", json!({"ticks": 40}));

    // A fresh game with no region: the island is made again before the load compares.
    let mut copy = game(&folder);
    assert!(copy.world().get_resource::<VoxelTerrain>().is_none());
    let report = restore(copy.world_mut(), &snapshot, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert!(report.warnings.iter().any(|w| w.starts_with("region: isle seed 7 v1 built again from its manifest")), "{:?}", report.warnings);
    assert_eq!(manifest(&copy), manifest(&original));
    assert_eq!(terrain(&copy).hash, terrain(&original).hash);
    ok(&mut copy, "step", json!({"ticks": 40}));
    assert_eq!(replay::state_hash(copy.world()), replay::state_hash(original.world()));
    assert_eq!(capture(copy.world()).unwrap().entities, capture(original.world()).unwrap().entities);

    // A game in another seed of the same region: replaced.
    let mut other = game(&folder);
    ok(&mut other, "regen", json!({"region": "isle", "seed": 8}));
    let report = restore(other.world_mut(), &snapshot, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert_eq!((manifest(&other).seed, terrain(&other).seed), (7, 7));

    // A snapshot taken without a region unloads the voxel region, and its terrain goes too.
    let bare = capture(game(&folder).world()).unwrap();
    assert!(!bare.checks.contains_key("voxel.volume"), "absent: a world without a voxel region keeps its bytes");
    restore(other.world_mut(), &bare, false).unwrap();
    assert!(other.world().get_resource::<VoxelTerrain>().is_none() && other.world().get_resource::<ActiveRegion>().is_none());
    assert_eq!(other.world_mut().query::<&shards::motion::VoxelCollider>().iter(other.world()).count(), 0);

    // A snapshot whose volume this build would make otherwise is refused, naming the check.
    let mut forged = snapshot.clone();
    forged.checks.insert("voxel.volume".into(), "0000000000000001".into());
    let mut third = game(&folder);
    let refused = restore(third.world_mut(), &forged, false).unwrap_err();
    assert!(refused.contains("voxel.volume") && refused.contains("the voxel volume differs"), "{refused}");
}

#[test]
fn a_cached_build_and_a_cold_one_spawn_the_same_colliders_in_the_same_order() {
    // A seed no other test builds: the tests of this binary share its process, and the cache.
    let folder = folder("cache", ISLE);
    let build = |cache: bool| {
        let mut app = game_with(&folder, |_| {}, move |app| {
            app.insert_resource(VoxelSettings { cache });
        });
        ok(&mut app, "regen", json!({"region": "isle", "seed": 90_001}));
        let snapshot = capture(app.world()).unwrap();
        let cached = terrain(&app).cached;
        (colliders(&mut app), snapshot.checks["static_colliders"].clone(), snapshot.checks["voxel.volume"].clone(), replay::state_hash(app.world()), cached)
    };
    clear_cache();
    let cold = build(true);
    assert!(!cold.4, "the first build generates");
    assert!(cached_volumes() >= 1);
    let warm = build(true);
    assert!(warm.4, "the second build is the cache's");
    clear_cache();
    let cleared = build(true);
    assert!(!cleared.4);
    let off = build(false);
    assert!(!off.4, "with the cache off nothing is read");
    for (what, other) in [("warm", &warm), ("cleared", &cleared), ("off", &off)] {
        assert_eq!((&other.0, &other.1, &other.2, other.3), (&cold.0, &cold.1, &cold.2, cold.3), "{what}: the same colliders, ids and hashes");
    }
    assert_eq!(cold.0[0].1, "voxel terrain isle");
}

#[test]
fn a_save_in_a_voxel_region_comes_back_and_one_made_by_another_step_version_is_refused() {
    let saves = std::env::temp_dir().join(format!("shards_voxel_saves_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    let folder = folder("save", ISLE);
    let save_here = |app: &mut App| app.world_mut().resource_mut::<SaveConfig>().dir = saves.clone();
    let mut app = game(&folder);
    save_here(&mut app);
    ok(&mut app, "regen", json!({"region": "isle", "seed": 7}));
    let id = { let hero = hero(&mut app); app.world().get::<AiId>(hero).unwrap().to_string() };
    ok(&mut app, "teleport", json!({"id": id, "x": 302.5, "z": 300.5}));
    ok(&mut app, "step", json!({"ticks": 30}));
    let (saved, hash, at) = (manifest(&app), terrain(&app).hash, feet(&mut app));
    ok(&mut app, "save_game", json!({"what": "all", "slot": "isle"}));

    // A fresh game: the island is built from the save's manifest, the player where they stood.
    let mut fresh = game(&folder);
    save_here(&mut fresh);
    let out = ok(&mut fresh, "load_game", json!({"what": "all", "slot": "isle"}));
    assert!(text(&out).starts_with("ok: world/isle loaded"), "{out}");
    assert_eq!(manifest(&fresh), saved);
    assert_eq!(terrain(&fresh).hash, hash);
    assert_eq!(feet(&mut fresh), at);
    let story = journal(fresh.world());
    assert_eq!(lines(&story, "region isle seed 7 v1: built from the save's manifest").len(), 1, "{story:#?}");
    assert!(lines(&story, "region generated").is_empty(), "nothing is generated: {story:#?}");
    let check = ok(&mut fresh, "save_check", json!({"slot": "isle"}));
    assert_eq!(check["world"]["file"]["same"].as_bool(), Some(true), "{check}");

    // A build whose `plaza` step is at version 2 (and content that says so): the save's world
    // was made by version 1, so it is refused, naming the step, and nothing is built.
    let newer = folder_with("save_newer", &ISLE.replace("(\"plaza\", 1)", "(\"plaza\", 2)"));
    let dir2 = newer.path().to_string_lossy().into_owned();
    let mut later = testing::test_app(
        |config| config.asset_dir = dir2,
        |app| {
            app.add_voxel_step("plaza", 2, plaza).add_voxel_check("plaza_dry", plaza_dry).add_voxel_check("only_seed_one", only_seed_one);
        },
    );
    save_here(&mut later);
    let refused = err(&mut later, "load_game", json!({"what": "world", "slot": "isle"}));
    assert!(refused.contains("was made by voxel step `plaza` v1, and this build has `plaza` v2"), "{refused}");
    assert!(later.world().get_resource::<ActiveRegion>().is_none(), "nothing was built");
    drop((app, fresh, later));
    let _ = std::fs::remove_dir_all(&saves);
}

fn folder_with(name: &str, isle: &str) -> Folder {
    folder(name, isle)
}

#[test]
fn a_content_change_makes_the_region_again_and_a_colour_does_not() {
    let folder = folder("reload", ISLE);
    let mut app = game(&folder);
    ok(&mut app, "regen", json!({"region": "isle", "seed": 7}));
    let before = terrain(&app).hash;
    let hero = hero(&mut app);

    // A colour: the same world, the picture's palette replaced.
    let palette = ok(&mut app, "content_get", json!({"kind": "voxel_palette", "id": "test"}))["def"].clone();
    let mut recoloured = palette.clone();
    recoloured["blocks"][2]["top"] = json!("#ffeeaa");
    ok(&mut app, "content_apply", json!({"changes": [{"kind": "voxel_palette", "id": "test", "before": palette, "def": recoloured}]}));
    assert_eq!(terrain(&app).hash, before);
    assert!(lines(&journal(app.world()), "regenerated").is_empty(), "{:#?}", journal(app.world()));
    assert_eq!(terrain(&app).palette.def().blocks[2].top, "#ffeeaa", "the picture reads the new colour");

    // Higher hills: the region is made again for its seed; the player keeps their place.
    let def = ok(&mut app, "content_get", json!({"kind": "voxel_region", "id": "isle"}))["def"].clone();
    let mut higher = def.clone();
    higher["params"]["noise_hills"]["amplitude"] = json!(6.0);
    let answer = ok(&mut app, "content_apply", json!({"changes": [{"kind": "voxel_region", "id": "isle", "before": def, "def": higher}]}));
    let after = terrain(&app).hash;
    assert_ne!(after, before);
    let line = format!("voxel region isle seed 7 v1 regenerated (hash {before:016x} -> {after:016x})");
    let said = lines(&journal(app.world()), &line);
    assert_eq!(said.len(), 1, "{:#?}", journal(app.world()));
    assert!(text(&answer).contains(&line), "the call says it too: {answer}");
    let db = app.world().resource::<ContentDb>();
    let palette = VoxelPalette::from_db(db, "test").unwrap();
    assert_eq!(manifest(&app).content_hash, VoxelRegionDef::content_hash(db, "isle", &palette));
    assert_eq!(app.world().get::<Transform>(hero).unwrap().translation, Vec3::new(300.5, 1.0, 300.5), "the plaza is where it was");

    // A change that does not validate is refused whole: the region stays.
    let def = ok(&mut app, "content_get", json!({"kind": "voxel_region", "id": "isle"}))["def"].clone();
    let mut broken = def.clone();
    broken["steps"][1] = json!(["plazza", 1]);
    let out = testing::call(&mut app, "content_apply", json!({"changes": [{"kind": "voxel_region", "id": "isle", "before": def, "def": broken}]}));
    assert!(!out.ok && out.result.to_string().contains("unknown voxel step `plazza` (did you mean `plaza`?)"), "{}", out.result);
    assert_eq!(terrain(&app).hash, after);
}

#[test]
fn content_errors_name_the_step_the_version_and_the_file() {
    let bad = ISLE
        .replace("(\"plaza\", 1)", "(\"plaza\", 3)")
        .replace("chunk: (16, 16, 16)", "chunk: (16, 16, 8)")
        .replace("\"fill_water\": (level: 6),", "\"fill_water\": (level: 6), \"stray\": (),")
        .replace("checks: [\"plaza_dry\"]", "checks: [\"plaza_dryy\"]");
    let errors = folder("errors", &bad);
    let dir = errors.path().to_string_lossy().into_owned();
    let app = testing::test_app(move |config| config.asset_dir = dir, setup);
    let all: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    let has = |needle: &str| {
        assert!(all.iter().any(|e| e.contains("content/world/isle.voxel_region.ron") && e.contains("voxel_region `isle`") && e.contains(needle)), "{needle}: {all:#?}");
    };
    has("steps[1]: voxel step `plaza` is at version 1 in this build, the content expects 3");
    has("square in x and z");
    has("params: `stray` is not a listed step");
    has("checks[0]: unknown voxel check `plaza_dryy` (did you mean `plaza_dry`?)");

    // A chunk lower than the volume, a sea out past where a position keeps its centimetres, and a
    // current at full strength farther in than where it starts.
    let bad = ISLE.replace("chunk: (16, 16, 16)", "chunk: (16, 8, 16)").replace("reach: 600.0", "reach: 200000.0").replace("starts: 6.0, full: 1.5", "starts: 1.0, full: 5.0");
    let far = folder("errors_far", &bad);
    let dir = far.path().to_string_lossy().into_owned();
    let app = testing::test_app(move |config| config.asset_dir = dir, setup);
    let all: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    for needle in ["chunk 16 x 8 x 16: as tall as the volume (16 cells)", "sea.reach 200000: above 0 and at most 100000 m", "sea.current: starts 1 and full 5: starts above 0"] {
        assert!(all.iter().any(|e| e.contains("voxel_region `isle`") && e.contains(needle)), "{needle}: {all:#?}");
    }
}

#[test]
fn check_content_generates_every_voxel_region_for_its_check_seed_and_its_fallback() {
    let failing = ISLE.replace("checks: [\"plaza_dry\"]", "checks: [\"plaza_dry\", \"only_seed_one\"], check_seed: 2, fallback_seed: Some(3)");
    let folder = folder("check", &failing);
    let app = game(&folder);
    let problems = shards::world::kind::generation_problems(app.world().resource::<ContentDb>(), app.world().resource::<WorldGen>());
    let problems: Vec<String> = problems.iter().map(ToString::to_string).collect();
    assert_eq!(problems.len(), 2, "{problems:#?}");
    assert!(problems.iter().all(|p| p.contains("content/world/isle.voxel_region.ron") && p.contains("voxel_region `isle`")), "{problems:#?}");
    assert!(problems[0].contains("check_seed 2: no attempt generates (attempt 1: check `only_seed_one`: seed 2 is not 1;"), "{problems:#?}");
    assert!(problems[1].contains("fallback_seed 3: its first attempt fails (check `only_seed_one`: seed 3 is not 1)"), "{problems:#?}");
    let fine = folder_with("check_fine", ISLE);
    let app = game(&fine);
    assert!(shards::world::kind::generation_problems(app.world().resource::<ContentDb>(), app.world().resource::<WorldGen>()).is_empty());
}

/// Copies the folder `from` into `to`, file by file.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The shape of a JSON answer: its keys, each with the kind of its value.
fn shape(value: &Value) -> Vec<(String, &'static str)> {
    let kind = |v: &Value| match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    let mut out: Vec<(String, &'static str)> = value.as_object().expect("an object").iter().map(|(k, v)| (k.clone(), kind(v))).collect();
    out.sort();
    out
}

/// The kinds of the values of a few fields of a JSON answer (absent: `missing`).
fn kinds_of(value: &Value, keys: &[&str]) -> Vec<(String, &'static str)> {
    let all = shape(value);
    keys.iter().map(|k| (k.to_string(), all.iter().find(|(name, _)| name == k).map_or("missing", |(_, kind)| *kind))).collect()
}

/// Whether cell `t` is one of `region`'s.
fn in_grid(region: &ActiveRegion, t: &TileId) -> bool {
    let size = region.manifest.size;
    (0..size[0] as i32).contains(&t.x) && (0..size[1] as i32).contains(&t.y)
}

/// The region API a game uses, asked the same way of a region of tiles (the world module's test
/// meadow) and of a volume (the island): the same Rust types and the same answers, so content can
/// swap one for the other without a change to gameplay code. What differs is said where it
/// differs (the manual's "Voxel regions (A2c)", "Swapping a region of tiles for a volume").
#[test]
fn the_region_api_answers_with_the_same_types_on_tiles_and_on_a_volume() {
    // The world module's test content (a meadow of tiles) and the island, in one game.
    let folder = folder("same_api", ISLE);
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../shards_world/tests/world/assets/content"), &folder.path().join("content"));
    let saves = std::env::temp_dir().join(format!("shards_voxel_same_api_saves_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    let save_here = |app: &mut App| app.world_mut().resource_mut::<SaveConfig>().dir = saves.clone();
    let mut app = game(&folder);
    save_here(&mut app);
    let mut answers = Vec::new();
    for (region, seed) in [("meadow", 3), ("isle", 7)] {
        let out = ok(&mut app, "regen", json!({"region": region, "seed": seed}));
        assert!(text(&out).starts_with(&format!("ok: {region} seed {seed} v1 (")), "{out}");
        let info = ok(&mut app, "region_info", json!({"manifest": true}));
        let active = app.world().resource::<ActiveRegion>();
        // The same Rust types answer for both: the manifest, its POIs, the spawn point, the area.
        let built: &RegionManifest = &active.manifest;
        let spawn: Vec3 = active.spawn_point(0);
        let area: Rect = built.area();
        let poi: Option<&shards::world::manifest::ManifestPoi> = built.pois.first();
        assert!(poi.is_some() && spawn.is_finite() && area.contains(spawn.xz()), "{region}: the spawn {spawn} in {area:?}");
        let mut types: Vec<String> = built.pois.iter().map(|p| p.poi.clone()).collect();
        types.sort();
        types.dedup();
        assert!(types.contains(&"camp".to_owned()) && types.contains(&"spawn".to_owned()), "{region}: {types:?}");
        // Points of interest by type (a camp, the spawn), each a `Poi` in a cell of the grid.
        let pois: Vec<(Poi, TileId)> = app.world_mut().query::<(&Poi, &TileId)>().iter(app.world()).map(|(p, t)| (p.clone(), *t)).collect();
        assert!(pois.iter().any(|(p, _)| p.poi == "camp"), "{region}: a camp is a Poi: {pois:?}");
        assert!(pois.iter().all(|(_, t)| in_grid(app.world().resource::<ActiveRegion>(), t)), "{region}: {pois:?}");
        // Named markers, each in a cell of the grid.
        let markers: Vec<(Marker, TileId)> = app.world_mut().query::<(&Marker, &TileId)>().iter(app.world()).map(|(m, t)| (m.clone(), *t)).collect();
        assert!(!markers.is_empty() && markers.iter().all(|(_, t)| in_grid(app.world().resource::<ActiveRegion>(), t)), "{region}: {markers:?}");
        let parts = app.world_mut().query_filtered::<(), With<RegionPart>>().iter(app.world()).count();
        assert!(parts > 0, "{region}");
        let spawned_at = feet(&mut app);
        assert!(spawned_at.distance(spawn) < 1.5, "{region}: the player at the spawn ({spawned_at} vs {spawn})");

        // The route provider: `path_to` answers on both, naming who planned the route.
        let to = spawn + Vec3::new(2.0, 0.0, 0.0);
        let route = ok(&mut app, "path_to", json!({"target": {"x": to.x, "z": to.z}}));
        assert!(route["provider"].is_string() && route["path"].is_string(), "{region}: {route}");
        // The navmesh tools answer on both (a volume has none yet, and says why).
        let nav = testing::call(&mut app, "nav_info", json!({}));
        let nav_path = testing::call(&mut app, "nav_path", json!({"to": {"x": to.x, "z": to.z}}));
        assert!(!nav.result.is_null() && !nav_path.result.is_null(), "{region}");
        if region == "isle" {
            assert_eq!(nav.result["navmesh"], "none", "{}", nav.result);
            assert!(!nav_path.ok && nav_path.result.to_string().contains("use path_to"), "{}", nav_path.result);
            assert_eq!(route["provider"], "local grid", "{route}");
        }
        // The `region` save section: saved and loaded the same way, into a game with no region.
        ok(&mut app, "save_game", json!({"what": "world", "slot": region}));
        let saved = app.world().resource::<ActiveRegion>().manifest.clone();
        let mut fresh = game(&folder);
        save_here(&mut fresh);
        let loaded = ok(&mut fresh, "load_game", json!({"what": "world", "slot": region}));
        assert!(text(&loaded).starts_with(&format!("ok: world/{region} loaded")), "{loaded}");
        assert_eq!(manifest(&fresh), saved, "{region}: the save's manifest built again");
        let check = ok(&mut fresh, "save_check", json!({"what": "world", "slot": region}));
        assert_eq!(check["world"]["file"]["same"].as_bool(), Some(true), "{region}: {check}");
        answers.push((shape(&info), shape(&info["manifest"]), kinds_of(&route, &["provider", "path", "turns", "waypoints"]), shape(&check["world"])));
    }
    let (tiles, volume) = (&answers[0], &answers[1]);
    let without = |s: &[(String, &'static str)], skip: &[&str]| s.iter().filter(|(k, _)| !skip.contains(&k.as_str())).cloned().collect::<Vec<_>>();
    // A volume has an origin away from the world's (so the corner is said) and no edge wall;
    // its manifest names steps. Everything else is the same field with the same kind of value.
    assert_eq!(without(&tiles.0, &["north_west_corner", "edge"]), without(&volume.0, &["north_west_corner", "edge"]));
    assert_eq!(without(&tiles.1, &["steps", "origin"]), without(&volume.1, &["steps", "origin"]));
    assert_eq!(tiles.2, volume.2, "path_to answers with the same fields");
    assert_eq!(tiles.3, volume.3, "save_check answers with the same fields");

    // Back to the tiles: the volume, its field and its water go with the region.
    let isle = manifest(&app);
    ok(&mut app, "regen", json!({"region": "meadow", "seed": 3}));
    assert!(app.world().get_resource::<VoxelTerrain>().is_none());
    assert_eq!(app.world_mut().query::<&shards::motion::VoxelCollider>().iter(app.world()).count(), 0);
    assert_eq!(app.world_mut().query::<&Current>().iter(app.world()).count(), 0);
    let story = journal(app.world());
    assert_eq!(lines(&story, "region unloaded: isle seed 7 v1").len(), 1, "{story:#?}");
    // And to the island again from its manifest, as `region_load` takes it.
    let out = ok(&mut app, "region_load", json!({"manifest": isle.to_canonical()}));
    assert!(text(&out).starts_with("ok: isle seed 7 v1 loaded from its manifest (4 chunks, "), "{out}");
    assert_eq!(manifest(&app), isle);
    assert!(app.world().get_resource::<VoxelTerrain>().is_some());
    drop(app);
    let _ = std::fs::remove_dir_all(&saves);
}

/// A manifest is data a player can edit (a save, a snapshot file): one whose grid is not the
/// one its definition makes is refused before anything changes, in the world it was loaded into.
#[test]
fn a_manifest_whose_grid_was_changed_is_refused_and_the_world_stays() {
    let folder = folder("forged", ISLE);
    let mut app = game(&folder);
    ok(&mut app, "regen", json!({"region": "isle", "seed": 7}));
    let (m, hash) = (manifest(&app), terrain(&app).hash);
    type Forge = fn(&mut RegionManifest);
    let forgeries: [(&str, Forge); 5] = [
        ("no chunks", |m| m.size = [0, 0]),
        ("chunks past counting", |m| m.size = [400_000, 400_000]),
        ("another chunk edge", |m| m.tile_size = 8.0),
        ("another corner", |m| m.origin = [0.0, 0.0]),
        ("a point outside", |m| m.pois[0].cell = [5, 0]),
    ];
    for (what, forge) in forgeries {
        let mut forged = m.clone();
        forge(&mut forged);
        let refused = err(&mut app, "region_load", json!({"manifest": forged.to_canonical()}));
        assert!(refused.contains("the manifest was changed"), "{what}: {refused}");
        assert_eq!((manifest(&app), terrain(&app).hash), (m.clone(), hash), "{what}: the region in the world stays");
    }
    // In a snapshot file: refused whole, into a game with the island and into one with no region.
    let snapshot = capture(app.world()).unwrap();
    let file = serde_json::to_string(&snapshot).unwrap();
    let size = r#"\"size\": [2, 2]"#;
    assert_eq!(file.matches(size).count(), 1, "the manifest's size in the snapshot's region record: {file}");
    let forged: shards::ai_core::snapshot::Snapshot = serde_json::from_str(&file.replace(size, r#"\"size\": [400000, 400000]"#)).unwrap();
    let refused = restore(app.world_mut(), &forged, false).unwrap_err();
    assert!(refused.contains("the manifest was changed"), "{refused}");
    assert_eq!((manifest(&app), terrain(&app).hash), (m.clone(), hash));
    let mut empty = game(&folder);
    let refused = restore(empty.world_mut(), &forged, false).unwrap_err();
    assert!(refused.contains("the manifest was changed"), "{refused}");
    assert!(empty.world().get_resource::<ActiveRegion>().is_none() && empty.world().get_resource::<VoxelTerrain>().is_none());
    assert_eq!(empty.world_mut().query::<&Marker>().iter(empty.world()).count(), 0, "no marker of the forged region");
}

/// A step that marks a point beside the island, off its cells.
fn stray_mark(g: &mut VoxelGen) -> Result<(), String> {
    g.mark("stray.0", Vec3::new(270.0, 1.0, 300.0));
    Ok(())
}

/// A step that marks a point off the volume fails its attempt by name: a point of interest is in
/// one of the region's chunks.
#[test]
fn a_mark_off_the_volume_fails_the_attempt() {
    let stray = ISLE.replace("(\"fill_water\", 1)]", "(\"fill_water\", 1), (\"stray_mark\", 1)]").replace("attempts: 4,", "attempts: 1,");
    assert_ne!(stray, ISLE);
    let folder = folder("stray", &stray);
    let mut app = game_with(&folder, |_| {}, |app| {
        app.add_voxel_step("stray_mark", 1, stray_mark);
    });
    let refused = err(&mut app, "regen", json!({"region": "isle", "seed": 7}));
    assert!(refused.contains("mark `stray.0`: at (270.00, 1.00, 300.00), outside the volume (x 284 to 316, z 284 to 316)"), "{refused}");
    assert!(app.world().get_resource::<ActiveRegion>().is_none(), "nothing was built");
}

/// The voxel module registers its region kind with the world module, so it is added after it:
/// the other way round is said when the app is built, not found at the first `regen`.
#[test]
#[should_panic(expected = "VoxelPlugin was added before WorldPlugin")]
fn the_voxel_module_added_before_the_world_module_is_a_build_error() {
    let mut app = App::new();
    let plugins = EnginePlugins::new(testing::config()).build().add_before::<shards::world::WorldPlugin>(shards::voxel::VoxelPlugin);
    app.add_plugins(plugins);
    app.finish();
}

/// `plaza` as another build might have it: the same name and version, one more block.
fn plaza_otherwise(g: &mut VoxelGen) -> Result<(), String> {
    plaza(g)?;
    let dirt = g.palette().id("dirt").ok_or("no dirt")?;
    g.volume_mut().set(IVec3::new(0, 15, 0), dirt);
    Ok(())
}

/// Content may be the same and the step code not: the volume a client makes from the host's
/// manifest is compared with the host's (`voxel.volume`) when it takes the host's world (the
/// join, a region change, a resync), and a client whose volume differs, or that has none, is
/// refused there, naming the check. The net layer's periodic desync hash covers the replicated
/// view; the volume never changes after it is built (no runtime edits), so it is compared there.
#[test]
fn a_client_that_makes_another_volume_or_none_cannot_take_the_hosts_world() {
    // The body a joining player gets (the engine's default spawner names this prefab).
    let players = r#"{ "player": (components: {"AiKind": ("Player")}) }"#;
    let folder = Folder::new("stand", &[("content/test.voxel_palette.ron", PALETTE), ("content/world/isle.voxel_region.ron", ISLE), ("content/players.prefab.ron", players)]);
    let dir = folder.path().to_string_lossy().into_owned();
    let configure = |c: &mut EngineConfig| c.asset_dir = dir.clone();
    // A client of the same build makes the same volume from the host's manifest.
    let mut same = testing::stand(1, LinkConditions::NONE, configure, setup);
    assert!(same.call(0, "regen", json!({"region": "isle", "seed": 7})).ok);
    same.run(90);
    let story = journal(same.world(1));
    let hash = same.world(0).resource::<VoxelTerrain>().hash;
    assert_eq!(same.world(1).get_resource::<VoxelTerrain>().map(|t| t.hash), Some(hash), "the client built the same island: {story:#?}");
    assert_eq!(lines(&story, "net: the host's world again (region isle)").len(), 1, "{story:#?}");
    same.run(130);
    assert_eq!(same.world(0).resource::<NetHost>().desyncs, 0, "{:#?}", journal(same.world(0)));

    let mut stand = testing::stand_host(2, &configure, &setup);
    // c1's `plaza` makes one more block; c2 hides its volume after building it (a test hook:
    // a prepare after the voxel module's).
    let other_code = |app: &mut App| {
        app.add_voxel_step("plaza", 1, plaza_otherwise).add_voxel_check("plaza_dry", plaza_dry).add_voxel_check("only_seed_one", only_seed_one);
        app.register_type::<Hero>();
    };
    testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &other_code);
    let hides = |app: &mut App| {
        setup(app);
        app.add_snapshot_prepare_after("hide_volume", "voxel", &[], |world, _| {
            world.remove_resource::<VoxelTerrain>();
            Ok(None)
        });
    };
    testing::stand_client(&mut stand, "c2", LinkConditions::NONE, &configure, &hides);
    stand.settle().unwrap();
    let regen = stand.call(0, "regen", json!({"region": "isle", "seed": 7}));
    assert!(regen.ok, "{}", regen.result);
    stand.run(90);
    let hash = stand.world(0).resource::<VoxelTerrain>().hash;
    let refused = |stand: &shards::net::Stand, member: usize| {
        let story = journal(stand.world(member));
        let line = lines(&story, "net: cannot take the host's world: ");
        assert_eq!(line.len(), 1, "member {member}: {story:#?}");
        line[0].clone()
    };
    let c1 = refused(&stand, 1);
    assert!(c1.contains(&format!("voxel.volume differs (snapshot {hash:016x}, this game ")) && c1.contains("the voxel volume differs"), "{c1}");
    let c2 = refused(&stand, 2);
    assert!(c2.contains(&format!("voxel.volume differs (snapshot {hash:016x}, this game has none)")), "{c2}");
    stand.run(130);
    assert_eq!(stand.world(0).resource::<NetHost>().desyncs, 0, "{:#?}", journal(stand.world(0)));
}

/// The game built for a playback, not loaded, with the voxel cache on or off.
fn unloaded(folder: &Folder, seed: u64, cache: bool) -> App {
    let mut config = testing::config();
    config.asset_dir = folder.path().to_string_lossy().into_owned();
    config.seed = seed;
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config));
    setup(&mut app);
    app.insert_resource(VoxelSettings { cache });
    app.finish();
    app.cleanup();
    app
}

#[test]
fn a_session_in_a_voxel_region_replays_the_same_with_the_cache_on_off_and_in_heavy_mode() {
    let folder = folder("replay", ISLE);
    let mut app = game(&folder);
    ok(&mut app, "step", json!({"ticks": 10}));
    ok(&mut app, "regen", json!({"region": "isle", "seed": 7}));
    ok(&mut app, "move", json!({"dir": "W", "meters": 4.0}));
    ok(&mut app, "step", json!({"ticks": 60}));
    ok(&mut app, "regen", json!({"region": "isle", "seed": 8}));
    ok(&mut app, "move", json!({"dir": "N", "meters": 3.0}));
    ok(&mut app, "step", json!({"ticks": 60}));
    let recording = replay::save(app.world());
    for cache in [true, false] {
        let mut player = unloaded(&folder, recording.seed, cache);
        let report = testing::verify(&mut player, recording.clone()).unwrap();
        assert!(report.identical(), "cache {cache}: {report}");
    }
    let dir = folder.path().to_string_lossy().into_owned();
    let verdict = testing::verify_heavy(recording, move |c| c.asset_dir = dir.clone(), setup, shards::HeavyConfig::every(30)).unwrap();
    assert!(verdict.report.identical(), "{}", verdict.report);
    assert!(verdict.checked >= 4, "{} heavy points checked, {} skipped", verdict.checked, verdict.skipped);
}
