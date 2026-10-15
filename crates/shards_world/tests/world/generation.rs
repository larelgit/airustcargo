//! The generator on test content: an 8×8 region that passes its checks, byte-stable
//! manifests, versions and content in the hash, many seeds, fixed cells, borders, limits,
//! the fallback, game steps and checks.

use std::time::Instant;

use shards::world::checks::check_manifest;
use shards::world::generate::{generate, GenContext};
use shards::world::manifest::Layout;
use shards::world::report;
use shards::world::{Dir, Stage, TileDef};

use shards::prelude::*;

use crate::common::*;

#[test]
fn an_8x8_region_generates_passes_its_checks_and_is_byte_stable() {
    let app = app(1, None);
    let (db, worldgen) = (db(&app), worldgen(&app));
    let made = generate(db, &worldgen, "meadow", 42).expect("meadow generates");
    let m = &made.manifest;
    assert_eq!((m.layout, m.size, m.tiles.len()), (Layout::Grid, [8, 8], 64));
    assert_eq!((m.region.as_str(), m.seed, m.version), ("meadow", 42, 1));
    assert!(m.fallback.is_none() && m.attempt >= 1, "attempt {}", m.attempt);
    let rows: Vec<[i32; 2]> = m.tiles.iter().map(|t| t.at).collect();
    let row_major: Vec<[i32; 2]> = (0..8).flat_map(|y| (0..8).map(move |x| [x, y])).collect();
    assert_eq!(rows, row_major, "tiles are row-major");
    let kinds: Vec<&str> = m.pois.iter().map(|p| p.poi.as_str()).collect();
    for required in ["spawn", "camp", "shrine"] {
        assert!(kinds.contains(&required), "{required} in {kinds:?}");
    }
    assert_eq!(kinds.iter().filter(|k| **k == "camp").count(), 2);
    assert_eq!(m.spawn.as_deref(), Some("spawn.0"));
    assert_eq!(m.links.len(), 3, "a road tree joins the four required points");
    assert!(check_manifest(db, Some(&worldgen), m).unwrap().is_empty(), "it passes its checks");

    let again = generate(db, &worldgen, "meadow", 42).unwrap().manifest;
    assert_eq!(again.to_canonical(), m.to_canonical(), "same seed, same bytes");
    assert_eq!(again.hash(), m.hash());
    let other = generate(db, &worldgen, "meadow", 43).unwrap().manifest;
    assert_ne!(other.to_canonical(), m.to_canonical(), "another seed, another region");
}

#[test]
fn the_version_and_the_content_are_part_of_the_manifest() {
    let app = app(1, None);
    let worldgen = worldgen(&app);
    let v1 = generate(db(&app), &worldgen, "meadow", 7).unwrap().manifest;
    let bumped = edited_db(&app, "world/meadow.region.ron", |t| t.replacen("version: 1", "version: 2", 1));
    let v2 = generate(&bumped, &worldgen, "meadow", 7).unwrap().manifest;
    assert_eq!(v2.version, 2);
    assert_ne!(v2.tiles, v1.tiles, "a new generator version gives the old seed a new world");
    assert_ne!(v2.content_hash, v1.content_hash);

    let unchanged = edited_db(&app, "world/meadow.region.ron", |t| t.replace("// An 8×8", "// A test 8×8"));
    let same = generate(&unchanged, &worldgen, "meadow", 7).unwrap().manifest;
    assert_eq!(same.to_canonical(), v1.to_canonical(), "comments change no definition");

    let heavier = edited_db(&app, "world/meadow.tile.ron", |t| t.replacen("weight: 6.0", "weight: 5.0", 1));
    let reweighed = generate(&heavier, &worldgen, "meadow", 7).unwrap().manifest;
    assert_ne!(reweighed.content_hash, v1.content_hash, "the hash covers the tiles");
}

#[test]
fn a_hundred_seeds_pass_their_checks() {
    let app = app(1, None);
    let report = report::run(db(&app), &worldgen(&app), "meadow", "1..100").unwrap();
    assert!(report.all_passed(), "{report}");
    assert_eq!(report.seeds, 100);
    assert!(report.panics.is_empty() && report.fell_back.is_empty(), "{report}");
    let (min, _, max) = report.distance_stats("camp–spawn").expect("camp–spawn distances");
    assert!(min >= 32.0 && max > min, "{report}");
    let text = report.to_string();
    assert!(text.starts_with("ok: worldgen meadow v1, seeds 1..100: 100/100 passed"), "{text}");
    assert!(text.contains("distances between points (m, min / median / max):"), "{text}");
}

#[test]
fn a_thousand_seeds_pass_their_checks_quickly() {
    let app = app(1, None);
    let started = Instant::now();
    let report = report::run(db(&app), &worldgen(&app), "meadow", "1..1000").unwrap();
    let took = started.elapsed();
    assert!(report.all_passed(), "{report}");
    println!("{report}");
    // Well under 10 ms a seed with optimized dependencies; a debug build of this crate is
    // slower, and still has to finish.
    assert!(took.as_secs_f64() < 60.0, "1000 seeds took {took:?}");
}

#[test]
fn fixed_cells_borders_and_limits_hold() {
    let app = app(1, None);
    let (db, worldgen) = (db(&app), worldgen(&app));
    for seed in 1..=20 {
        let m = generate(db, &worldgen, "walled", seed).unwrap_or_else(|e| panic!("seed {seed}: {e}")).manifest;
        assert!(m.fallback.is_none());
        let corner = &m.tiles[0];
        assert_eq!((corner.tile.as_str(), corner.rot), ("road_end", 90), "seed {seed}: the fixed tile");
        assert_eq!(m.tiles.iter().filter(|t| t.tile == "ruin").count(), 1, "seed {seed}: exactly one #old tile");
        let spawn = m.pois.iter().find(|p| p.poi == "spawn").unwrap();
        assert_eq!(spawn.cell, [0, 0]);
        assert_eq!(m.pois.iter().find(|p| p.poi == "camp").unwrap().cell, [5, 3]);
        // Every side on the border shows grass.
        for t in &m.tiles {
            let def = db.get::<TileDef>(&t.tile).unwrap();
            let edges = def.sockets.parse().unwrap();
            let turns = (t.rot / 90) as usize;
            for dir in Dir::ALL {
                let (dx, dy) = dir.offset();
                let (x, y) = (t.at[0] + dx, t.at[1] + dy);
                if x < 0 || y < 0 || x >= 6 || y >= 4 {
                    let edge = &edges[(dir.index() + 4 - turns) % 4];
                    assert!(edge.iter().all(|s| s.kind == "grass"), "seed {seed}: {} at {:?} shows {edge:?} on {dir}", t.tile, t.at);
                }
            }
        }
    }
}

#[test]
fn when_no_attempt_passes_the_fallback_is_used() {
    let app = app(1, None);
    let made = generate(db(&app), &worldgen(&app), "impossible", 3).unwrap();
    let m = &made.manifest;
    assert_eq!(m.fallback.as_deref(), Some("meadow_safe"));
    assert_eq!(m.attempt, 0);
    assert_eq!(made.failures.len(), 3, "three attempts first");
    assert!(made.failures.iter().all(|f| f.rule == "points: no cell"), "{:?}", made.failures);
    assert_eq!((m.tiles[0].tile.as_str(), m.tiles[0].rot), ("road_end", 90));
    let ids: Vec<&str> = m.pois.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["spawn.0", "camp.0", "shrine.0", "camp.1"]);
    assert!(made.remarks.iter().any(|r| r.contains("used the fallback `meadow_safe`")), "{:?}", made.remarks);
    // A region without a fallback says what to do.
    let hopeless = r#"{ "hopeless": (size: (4, 4), tile_size: 16.0, version: 1, tiles: ["grass", "road_end"],
        fixed: [(at: (0, 0), tile: "grass")], pois: [(poi: "spawn", cells: [(0, 0)])], attempts: 2) }"#;
    let db = db_with(&app, &[("hopeless.region.ron", hopeless)]);
    assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
    let err = generate(&db, &worldgen(&app), "hopeless", 3).unwrap_err();
    assert_eq!(err.failures.len(), 2);
    assert!(err.message.contains("no attempt of 2 passed") && err.message.contains("give it a `fallback`"), "{err}");
}

#[test]
fn a_seed_that_fails_with_no_attempt_to_blame_says_why_in_the_report() {
    let app = app(1, None);
    // The attempts fail (the spawn must sit in a grass cell, which has no slot), and the
    // fallback breaks too: two points on a tile with one slot, which content check cannot see.
    let region = r#"{ "clash": (size: (2, 1), tile_size: 16.0, version: 1, tiles: ["grass", "road_end"],
        fixed: [(at: (0, 0), tile: "grass")], pois: [(poi: "spawn", cells: [(0, 0)])], attempts: 2, fallback: "clash_safe") }"#;
    let map = r#"{ "clash_safe": (rows: ["road_end road_end"], pois: [(poi: "spawn", at: (0, 0)), (poi: "camp", at: (0, 0))]) }"#;
    let db = db_with(&app, &[("clash.region.ron", region), ("clash.tile_map.ron", map)]);
    assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
    let broken = "fallback `clash_safe`: no slot: `camp` at (0, 0) found no free slot in tile `road_end`";
    let failed = generate(&db, &worldgen(&app), "clash", 1).unwrap_err();
    assert_eq!(failed.message, broken);

    let report = report::run(&db, &worldgen(&app), "clash", "1..3").unwrap();
    assert_eq!(report.failed, [1, 2, 3]);
    assert_eq!(report.reasons["points: no cell"].count, 6, "two attempts a seed");
    assert_eq!(report.errors[broken].count, 3, "{report}");
    assert_eq!(report.errors[broken].examples, [1, 2, 3]);
    let text = report.to_string();
    assert!(text.contains(&format!("errors (3):\n  3× {broken} (e.g. seeds 1, 2, 3)")), "{text}");

    // A region that runs out of attempts has its attempts to show, and no error line for them.
    let hopeless = r#"{ "hopeless": (size: (4, 4), tile_size: 16.0, version: 1, tiles: ["grass", "road_end"],
        fixed: [(at: (0, 0), tile: "grass")], pois: [(poi: "spawn", cells: [(0, 0)])], attempts: 2) }"#;
    let db = db_with(&app, &[("hopeless.region.ron", hopeless)]);
    let report = report::run(&db, &worldgen(&app), "hopeless", "1..3").unwrap();
    assert!(report.errors.is_empty(), "{report}");
    assert_eq!(report.reasons["points: no cell"].count, 6, "{report}");
}

/// A game step that panics for seed 7 and a game check that fails the first attempt of
/// even seeds.
fn picky(ctx: &mut GenContext) -> Result<(), String> {
    assert!(ctx.seed != 7, "the game's step broke on seed 7");
    if ctx.attempt == 0 && ctx.seed.is_multiple_of(2) {
        ctx.failures.push(shards::world::CheckFailure::new("even", vec![[0, 0]], "even seeds retry once"));
    }
    Ok(())
}

#[test]
fn game_steps_join_the_pipeline_and_their_panics_are_reported_with_the_seed() {
    let app = app(1, None);
    let mut worldgen = worldgen(&app);
    worldgen.add_step(Stage::Checks, "game.picky", picky);
    let report = report::run(db(&app), &worldgen, "meadow", "1..10").unwrap();
    assert_eq!(report.panics.len(), 1, "{report}");
    assert_eq!(report.panics[0].0, 7);
    assert!(report.panics[0].1.contains("broke on seed 7"), "{report}");
    assert!(report.failed.contains(&7) && report.passed == 9, "{report}");
    assert!(report.retried >= 5, "every even seed needed a second attempt: {report}");
    // An attempt is named after its first failure: an engine check may come before `even`.
    assert!(report.reasons["check even"].count >= 4, "{report}");
    let text = report.to_string();
    assert!(text.starts_with("FAILED:") && text.contains("seed 7: the game's step broke on seed 7"), "{text}");

    // Replacing a stage drops the engine's steps: no roads planned, so links are gone.
    let mut no_roads = self::worldgen(&app);
    no_roads.replace_stage(Stage::Roads, "game.none", |_| Ok(()));
    let m = generate(db(&app), &no_roads, "meadow", 5);
    if let Ok(made) = m {
        assert!(made.manifest.links.is_empty());
    }
}

#[test]
fn content_mistakes_name_the_file_and_the_fix() {
    let app = app(1, None);
    let db = db_with(
        &app,
        &[
            ("bad.tile.ron", r#"{ "bad": (sockets: (n: "Grass", e: "grass", s: "gras", w: "grass"), rotations: [45]) }"#),
            (
                "bad.region.ron",
                r##"{ "bad": (size: (2, 2), tile_size: 16.0, version: 1, tiles: ["grass", "#oldd"], pois: [(poi: "cmap")], fallback: "meadow_safe") }"##,
            ),
        ],
    );
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    let has = |needle: &str| errors.iter().any(|e| e.contains(needle));
    assert!(has("content/bad.tile.ron: tile `bad`: sockets.n: `Grass`: socket kinds are lowercase"), "{errors:#?}");
    assert!(has("rotations: 45 is not a quarter turn"), "{errors:#?}");
    assert!(has("region `bad`: tiles: no tile has tag `oldd` (did you mean `#old`?)"), "{errors:#?}");
    assert!(has("pois[0].poi -> unknown poi `cmap` (did you mean `camp`?)"), "{errors:#?}");
    let unknown_kind = db_with(&app, &[("odd.tile.ron", r#"{ "odd": (sockets: (n: "gras", e: "grass", s: "grass", w: "grass")) }"#)]);
    let errors: Vec<String> = unknown_kind.errors().map(ToString::to_string).collect();
    assert!(
        errors.iter().any(|e| e.contains("tile `odd` uses socket kind `gras`, which rules `default` does not list (did you mean `grass`?)")),
        "{errors:#?}"
    );
}
