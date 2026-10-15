//! Region changes that go wrong or come from odd places: from inside a state transition,
//! with content the manifest needs but the game lacks, onto floors above the tile plane,
//! and tiles whose level file is not a path the loader can ask for.

use std::time::{Duration, Instant};

use serde_json::json;
use shards::ai_core::{AiJournal, SimGate};
use shards::world::{region, RegionPart};

use shards::prelude::*;

use crate::common::*;
use crate::levels::{app_in, fresh_in, site};

/// Times the game's own `OnEnter(InGame)` ran.
#[derive(Resource, Default)]
struct Entered(u32);

#[test]
fn a_region_changed_from_on_enter_in_game_does_not_enter_the_game_again() {
    let mut app = app_with(3, None, |app| {
        app.init_resource::<Entered>().add_systems(OnEnter(EngineState::InGame), |world: &mut World| {
            world.resource_mut::<Entered>().0 += 1;
            region::regen(world, "meadow", 3).expect("the game makes its region on entering");
        });
    });
    for _ in 0..6 {
        app.update();
    }
    assert_eq!(app.world().resource::<Entered>().0, 1, "OnEnter(InGame) ran once, not once a frame");
    assert_eq!(region(&app).manifest.seed, 3);
    assert_eq!(*app.world().resource::<State<EngineState>>().get(), EngineState::InGame);
}

#[test]
fn a_manifest_with_a_prefab_content_lacks_is_refused_before_the_old_region_goes() {
    let mut app = app(8, Some("meadow"));
    let mut broken = region(&app).manifest.clone();
    let at = broken.tiles.iter().position(|t| t.decor.is_some()).expect("grass tiles carry decor");
    broken.tiles[at].decor = Some("rokcs".into());
    let parts = |app: &mut App| app.world_mut().query_filtered::<(), With<RegionPart>>().iter(app.world()).count();
    let (hash, before) = (region(&app).hash, parts(&mut app));
    assert!(before > 0);

    let refused = err(&mut app, "region_load", json!({"manifest": serde_json::to_value(&broken).unwrap()}));
    assert!(refused.contains("decor: unknown prefab `rokcs`"), "{refused}");
    assert_eq!(region(&app).hash, hash, "the old region still stands");
    assert_eq!(parts(&mut app), before, "with all its entities");
    assert!(!journal(&app).join("\n").contains("region unloaded"));
}

const HILL: &str = r#"{
    // A plateau: the floor is 6 m above the tile plane, the spawn slot on the plane.
    "hill": (
        sockets: (n: "grass", e: "grass", s: "grass", w: "grass"),
        slots: [(at: (0.0, 0.0, 0.0), accepts: ["spawn"])],
        geometry: Inline([Box(min: (-8.0, -1.0, -8.0), max: (8.0, 6.0, 8.0))]),
    ),
}"#;

const HILLS: &str = r#"{ "hills": (size: (1, 1), tile_size: 16.0, version: 1, tiles: ["hill"], pois: [(poi: "spawn")]) }"#;

#[test]
fn players_and_goto_tile_stand_on_a_floor_above_the_tile_plane_not_inside_it() {
    let dir = site("hills", &[("content/hills/hill.tile.ron", HILL.as_bytes()), ("content/hills/hills.region.ron", HILLS.as_bytes())]);
    let mut app = app_in(&dir, 1, None);
    let hero = hero(&mut app);
    ok(&mut app, "regen", json!({"region": "hills"}));
    assert!((pos(&app, hero).y - 6.0).abs() < 0.01, "on top of the block, not inside it: {}", pos(&app, hero));
    ok(&mut app, "step", json!({"ticks": 20}));
    assert!((pos(&app, hero).y - 6.0).abs() < 0.05, "{}", pos(&app, hero));

    ok(&mut app, "step", json!({"ticks": 1}));
    ok(&mut app, "goto_tile", json!({"x": 0, "y": 0}));
    assert!((pos(&app, hero).y - 6.0).abs() < 0.01, "{}", pos(&app, hero));
}

#[test]
fn a_level_path_the_loader_cannot_ask_for_stops_loading_and_names_its_tile() {
    let tile = r#"{ "escape": (sockets: (n: "grass", e: "grass", s: "grass", w: "grass"), geometry: Level(path: "../tiles/yard.gltf")) }"#;
    let dir = site("escape", &[("content/escape/escape.tile.ron", tile.as_bytes())]);
    let mut app = fresh_in(&dir, 1, None);
    let started = Instant::now();
    let reasons = loop {
        app.update();
        let reasons = app.world().resource::<SimGate>().reasons().join(", ");
        if reasons.contains("tile levels: failed") {
            break reasons;
        }
        assert!(started.elapsed() < Duration::from_secs(5), "never failed, still: {reasons}");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(reasons.contains("tile `escape`") && reasons.contains("`../tiles/yard.gltf` must be a path inside the asset folder"), "{reasons}");
    let log: Vec<String> = app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect();
    assert!(log.iter().any(|l| l.contains("loading tile levels failed: tile `escape`")), "{log:?}");
}
