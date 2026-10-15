//! Each check on hand-made manifests: it fails on what breaks its rule, names the rule and
//! the cells, and passes once the problem is fixed.

use shards::world::checks::{custom_checks, engine_rules, CheckInput, RegionGraph};
use shards::world::content::{CheckRules, DistanceRule};
use shards::world::manifest::{Layout, ManifestPoi, PlacedTile, RegionManifest, FORMAT};
use shards::world::{CheckFailure, Dir, WorldGen};

use shards::prelude::*;

use crate::common::*;

/// A meadow manifest from rows of `tile` / `tile:degrees`, with points of interest at cell
/// centers.
fn manifest(rows: &[&str], pois: &[(&str, (i32, i32))]) -> RegionManifest {
    let tiles: Vec<PlacedTile> = rows
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.split_whitespace().enumerate().map(move |(x, token)| {
                let (tile, rot) = token.split_once(':').map_or((token, 0), |(t, r)| (t, r.parse().unwrap()));
                PlacedTile { at: [x as i32, y as i32], size: [1, 1], tile: tile.into(), rot, decor: None }
            })
        })
        .collect();
    let mut counts = std::collections::BTreeMap::new();
    let pois: Vec<ManifestPoi> = pois
        .iter()
        .map(|(poi, (x, y))| {
            let n = counts.entry(*poi).or_insert(0);
            let id = format!("{poi}.{n}");
            *n += 1;
            ManifestPoi { id, poi: (*poi).into(), cell: [*x, *y], pos: [(*x as f32 + 0.5) * 16.0, 0.0, (*y as f32 + 0.5) * 16.0], area: 0 }
        })
        .collect();
    let width = rows[0].split_whitespace().count() as u32;
    RegionManifest {
        format: FORMAT.into(),
        region: "meadow".into(),
        layout: Layout::Grid,
        seed: 0,
        version: 1,
        attempt: 1,
        fallback: None,
        size: [width, rows.len() as u32],
        tile_size: 16.0,
        origin: [0.0, 0.0],
        tiles,
        spawn: pois.iter().find(|p| p.poi == "spawn").map(|p| p.id.clone()),
        pois,
        links: Vec::new(),
        content_hash: String::new(),
    }
}

fn run(app: &App, m: &RegionManifest, rules: &CheckRules) -> Vec<CheckFailure> {
    let graph = RegionGraph::build(db(app), m).expect("a graph");
    engine_rules(&CheckInput { manifest: m, rules, graph: &graph, db: db(app) })
}

fn only(rules: impl FnOnce(&mut CheckRules)) -> CheckRules {
    let mut r = CheckRules { reachable: false, isolated_roads: false, ..CheckRules::default() };
    rules(&mut r);
    r
}

#[test]
fn a_river_without_a_bridge_cuts_the_points_beyond_it_off() {
    let app = app(1, None);
    let rows = ["grass grass grass", "river river river", "grass grass grass"];
    let cut = manifest(&rows, &[("spawn", (0, 0)), ("camp", (2, 2)), ("shrine", (2, 0))]);
    let rules = only(|r| r.reachable = true);
    let failures = run(&app, &cut, &rules);
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].rule, "reachable");
    assert_eq!(failures[0].cells, [[2, 2]]);
    assert_eq!(failures[0].to_string(), "check reachable: cannot reach camp.0 at (2, 2) from spawn.0");
    let bridged = manifest(&["grass grass grass", "river bridge:0 river", "grass grass grass"], &[("spawn", (0, 0)), ("camp", (2, 2))]);
    assert!(run(&app, &bridged, &rules).is_empty());
    // Walkers reach the river's banks from the north and the south, never along the water.
    let graph = RegionGraph::build(db(&app), &cut).unwrap();
    assert!(graph.open([1, 0], Dir::S) && graph.open([1, 1], Dir::S) && graph.open([0, 0], Dir::E));
    assert!(!graph.open([0, 1], Dir::E), "the river's sides are water");
}

#[test]
fn steep_tiles_fail_the_slope_limit() {
    let app = app(1, None);
    let m = manifest(&["grass ruin", "grass grass"], &[("spawn", (0, 0))]);
    let failures = run(&app, &m, &only(|r| r.max_slope = Some(0)));
    assert_eq!(failures.len(), 1);
    assert_eq!((failures[0].rule.as_str(), failures[0].cells.as_slice()), ("slope", &[[1, 0]][..]));
    assert!(run(&app, &m, &only(|r| r.max_slope = Some(1))).is_empty());
}

#[test]
fn points_too_close_or_too_far_fail_the_distance_rules() {
    let app = app(1, None);
    let m = manifest(&["grass grass grass grass"], &[("spawn", (0, 0)), ("camp", (1, 0)), ("shrine", (3, 0))]);
    let rule = |a: &str, b: &str, min, max| DistanceRule { a: a.into(), b: b.into(), min, max };
    let failures = run(&app, &m, &only(|r| r.distances = vec![rule("spawn", "camp", Some(32.0), None), rule("spawn", "shrine", None, Some(40.0))]));
    let text: Vec<String> = failures.iter().map(ToString::to_string).collect();
    assert_eq!(
        text,
        [
            "check distance: spawn.0 and camp.0 are 16.0m apart, closer than 32m",
            "check distance: spawn.0 has no shrine within 40m; the nearest is 48.0m away",
        ]
    );
    assert!(run(&app, &m, &only(|r| r.distances = vec![rule("spawn", "camp", Some(16.0), Some(16.0))])).is_empty());
}

#[test]
fn a_road_that_leads_to_no_point_is_isolated() {
    let app = app(1, None);
    let rows = ["grass grass grass grass", "grass road_end:90 road_end:270 grass", "grass grass grass grass"];
    let lonely = manifest(&rows, &[("spawn", (0, 0))]);
    let failures = run(&app, &lonely, &only(|r| r.isolated_roads = true));
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].rule, "isolated_roads");
    assert_eq!(failures[0].cells, [[1, 1], [2, 1]]);
    assert_eq!(failures[0].message, "the road through (1, 1), (2, 1) reaches no point of interest");
    let served = manifest(&rows, &[("spawn", (0, 0)), ("camp", (2, 1))]);
    assert!(run(&app, &served, &only(|r| r.isolated_roads = true)).is_empty());
}

#[test]
fn a_region_without_a_spawn_and_unknown_custom_checks_fail() {
    let app = app(1, None);
    let mut m = manifest(&["grass"], &[("camp", (0, 0))]);
    m.spawn = None;
    let failures = run(&app, &m, &only(|_| {}));
    assert_eq!(failures[0].rule, "spawn");
    let rules = only(|r| r.custom = vec!["rivers_cross".into()]);
    let graph = RegionGraph::build(db(&app), &m).unwrap();
    let mut worldgen = WorldGen::default();
    worldgen.add_check("roads_wide", |_| Vec::new());
    let custom = custom_checks(&worldgen, &CheckInput { manifest: &m, rules: &rules, graph: &graph, db: db(&app) });
    assert_eq!(custom.len(), 1);
    assert!(custom[0].message.contains("no check named `rivers_cross` is registered (add_worldgen_check); registered: roads_wide"));
}
