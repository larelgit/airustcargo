//! The generated region, the engine's world module on this game's content: the valley, 8 × 8
//! tiles of 16 m (`assets/content/region/`), one of them a level file from Blender
//! (`assets/tiles/ruin.gltf`), with raiders (NPCs of the engine) at its camps and a navmesh
//! baked for its tiles (`assets/nav/`, `cargo game --bake-nav`).

use shards::ai_core::{sim, AiId};
use shards::content::ContentDb;
use shards::motion::{LayerMask, PhysicsWorld, QueryFilter};
use shards::prelude::*;
use shards::serde_json::json;
use shards::world::generate::generate;
use shards::world::{report, WorldGen};

use crate::{say, SEED};

fn db(app: &App) -> &ContentDb {
    app.world().resource::<ContentDb>()
}

fn worldgen(app: &App) -> WorldGen {
    app.world().resource::<WorldGen>().clone()
}

#[test]
fn a_thousand_seeds_of_the_valley_pass_its_checks() {
    // What `cargo game --worldgen-report valley --seeds 1..1000` prints in CI.
    let app = my_game::test_app(SEED);
    let report = report::run(db(&app), &worldgen(&app), "valley", "1..1000").unwrap();
    assert!(report.all_passed(), "{report}");
    assert!(report.to_string().starts_with("ok: worldgen valley v1, seeds 1..1000: 1000/1000 passed"), "{report}");
}

#[test]
fn the_same_seed_and_version_give_the_same_manifest_byte_for_byte() {
    let (a, b) = (my_game::test_app(SEED), my_game::test_app(SEED));
    let make = |app: &App, seed| generate(db(app), &worldgen(app), "valley", seed).unwrap().manifest;
    let first = make(&a, 42);
    assert_eq!(first.to_canonical(), make(&b, 42).to_canonical(), "another game, the same bytes");
    assert_eq!((first.size, first.tile_size, first.origin), ([8, 8], 16.0, [-64.0, 32.0]));
    assert_ne!(first.hash(), make(&a, 43).hash(), "another seed, another region");
    // Built in the game, it is the manifest the generator made.
    let mut app = a;
    say(&mut app, "regen", json!({"region": "valley", "seed": 42}));
    let info = shards::testing::ok(&mut app, "region_info", json!({})).result;
    assert_eq!(info["manifest_hash"], format!("{:016x}", first.hash()));
}

/// Times the loading screen came up.
#[derive(Resource, Default)]
struct Screens(u32);

#[test]
fn going_to_the_valley_passes_the_loading_screen_headless_and_keeps_the_hero() {
    let mut app = shards::testing::test_app(
        |config| config.seed = SEED,
        |app| {
            app.add_plugins(my_game::GamePlugin)
                .init_resource::<Screens>()
                .add_systems(OnEnter(EngineState::RegionLoading), |mut n: ResMut<Screens>| n.0 += 1);
        },
    );
    say(&mut app, "step", json!({"ticks": 30}));
    let out = say(&mut app, "regen", json!({"region": "valley", "seed": 42}));
    assert!(out.starts_with("ok: valley seed 42 v1") && out.contains("spawn at (-40.0, 56.0)"), "{out}");
    assert_eq!(app.world().resource::<Screens>().0, 1, "the loading screen came up");
    assert_eq!(*app.world().resource::<State<EngineState>>().get(), EngineState::InGame, "and went");
    assert_eq!(sim::tick(app.world()), 30, "no tick went by");
    let hero = app.world().resource::<shards::ai_core::AiIndex>().get(AiId(1)).expect("the same hero, e1");
    let at = app.world().get::<Transform>(hero).unwrap().translation;
    assert!(at.distance(Vec3::new(-40.0, 0.0, 56.0)) < 0.01, "at the gate: {at}");
    let seen = say(&mut app, "observe", json!({}));
    assert!(seen.contains("TILE (1, 1) gate rot 0 · valley seed 42 v1"), "{seen}");

    // The ruins are the level file's nodes, top-level colliders where the tile put them: a ray
    // down on a ruin's column (4 m tall, at (4, 4) in the tile, turned with it) stands on it.
    let manifest = app.world().resource::<ActiveRegion>().manifest.clone();
    let ruin = manifest.tiles.iter().find(|t| t.tile == "ruin").expect("seed 42 has a ruin");
    let turned = shards::world::grid::turn(Vec3::new(4.0, 0.0, 4.0), shards::world::grid::turns_of(ruin.rot).unwrap());
    let column = manifest.tile_center(ruin) + turned;
    say(&mut app, "step", json!({"ticks": 1}));
    let physics = app.world().resource::<PhysicsWorld>();
    let hit = physics.raycast(column + Vec3::Y * 10.0, Dir3::NEG_Y, 20.0, &QueryFilter::new(LayerMask::WORLD)).expect("the column");
    assert!((hit.point.y - 4.0).abs() < 0.01, "the column's top: {}", hit.point);
}

#[test]
fn a_raider_comes_by_the_navmesh_and_keeps_to_its_camp() {
    let mut app = my_game::test_app(SEED);
    say(&mut app, "regen", json!({"region": "valley", "seed": 42}));
    say(&mut app, "god", json!({}));
    say(&mut app, "teleport", json!({"id": "e1", "x": 11, "z": 60}));
    let step = |app: &mut App, ticks: u64, until: &[shards::serde_json::Value]| say(app, "step", json!({"ticks": ticks, "until": until}));
    let leash = json!({"named": "npc_beyond_leash"});
    let out = step(&mut app, 600, &[json!({"journal": "-> e1"}), leash.clone()]);
    assert!(out.contains("stopped: journal") && out.contains("-> e1"), "{out}");
    let out = step(&mut app, 300, std::slice::from_ref(&leash));
    assert!(out.contains("no condition met"), "{out}");
    // Out of its sight and far beyond its leash: it gives up and goes home.
    say(&mut app, "teleport", json!({"id": "e1", "x": -10, "z": 60}));
    let out = step(&mut app, 900, &[json!({"journal": "fight → home"}), leash.clone()]);
    assert!(out.contains("fight → home"), "{out}");
    let out = step(&mut app, 900, &[leash]);
    assert!(out.contains("no condition met"), "{out}");
}
