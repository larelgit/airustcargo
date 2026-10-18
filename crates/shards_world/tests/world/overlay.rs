//! The tile overlay (`world_overlay`), without a GPU or a window: what it hands to the gizmos
//! is kept in `TileDraw`, and these tests read it, as the combat overlay's are read through
//! `CombatDraw`. A game with gizmos (headless ones too: only their drawing needs a GPU) works
//! the shapes out every frame while the overlay is on.

use std::collections::BTreeMap;

use serde_json::json;
use shards::bevy::gizmos::GizmoPlugin;
use shards::content::ContentDb;
use shards::save::SaveConfig;
use shards::world::generate::generate;
use shards::world::manifest::{Layout, PlacedTile, RegionManifest, FORMAT};
use shards::world::overlay::{kind_color, Seg, BOUNDS_COLOR, LINK_COLOR, POI_COLOR, POI_RADIUS, ROAD_COLOR};
use shards::world::{TileDef, TileDraw, TileOverlay};
use shards::{testing, EnginePlugins};

use shards::prelude::*;

use crate::common::*;

/// Frames the window would draw. (A paused game still runs `PostUpdate`.)
fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn drawn(app: &App) -> &TileDraw {
    app.world().resource::<TileDraw>()
}

fn nothing(draw: &TileDraw) -> bool {
    draw.bounds.is_empty() && draw.sockets.is_empty() && draw.pois.is_empty() && draw.links.is_empty()
}

fn close(a: Vec3, b: Vec3) -> bool {
    a.distance(b) < 1e-3
}

/// The socket parts of a tile: how many there are in all, and how many are of each kind.
fn parts(db: &ContentDb, tile: &str) -> (usize, BTreeMap<String, usize>) {
    let edges = db.get::<TileDef>(tile).expect("a tile").sockets.parse().expect("sockets");
    let mut kinds = BTreeMap::new();
    for socket in edges.iter().flatten() {
        *kinds.entry(socket.kind.clone()).or_insert(0) += 1;
    }
    (edges.iter().map(Vec::len).sum(), kinds)
}

#[test]
fn nothing_is_drawn_until_the_tool_asks_for_it_and_the_tool_turns_it_off_again() {
    let mut app = app(42, Some("meadow"));
    frames(&mut app, 2);
    assert!(!app.world().resource::<TileOverlay>().enabled, "off by default");
    assert!(nothing(drawn(&app)));

    assert_eq!(ok(&mut app, "world_overlay", json!({})), "world overlay on");
    frames(&mut app, 2);
    assert!(app.world().resource::<TileOverlay>().enabled);
    assert!(!drawn(&app).bounds.is_empty() && !drawn(&app).sockets.is_empty() && !drawn(&app).pois.is_empty());

    assert_eq!(ok(&mut app, "world_overlay", json!({"on": false})), "world overlay off");
    frames(&mut app, 2);
    assert!(!app.world().resource::<TileOverlay>().enabled);
    assert!(nothing(drawn(&app)), "everything gone: {:?}", drawn(&app).bounds.len());

    assert_eq!(ok(&mut app, "world_overlay", json!({"on": true})), "world overlay on");
    frames(&mut app, 1);
    assert!(!drawn(&app).bounds.is_empty());
}

#[test]
fn every_tile_gets_its_bounds_every_socket_part_a_mark_every_point_a_ball_and_every_road_a_line() {
    let mut app = app(42, Some("meadow"));
    ok(&mut app, "world_overlay", json!({}));
    frames(&mut app, 2);
    let m = region(&app).manifest.clone();
    assert_eq!((m.layout, m.tiles.len(), m.tile_size), (Layout::Grid, 64, 16.0));
    let draw = drawn(&app);
    let lift = Vec3::Y * 0.05;

    // A rectangle per tile, in the manifest's order, 16 m a side, just above the ground.
    assert_eq!(draw.bounds.len(), m.tiles.len());
    for (outline, tile) in draw.bounds.iter().zip(&m.tiles) {
        let min = Vec3::new(tile.at[0] as f32 * 16.0, 0.0, tile.at[1] as f32 * 16.0) + lift;
        let expected = [min, min + Vec3::X * 16.0, min + Vec3::new(16.0, 0.0, 16.0), min + Vec3::Z * 16.0];
        for (corner, want) in outline.corners.iter().zip(expected) {
            assert!(close(*corner, want), "{:?} of {tile:?}: {corner} / {want}", outline.corners);
        }
        assert_eq!(outline.color, BOUNDS_COLOR);
    }

    // A mark per socket part, all four edges of every tile; roads orange, a color per kind.
    let (mut all, mut kinds) = (0, BTreeMap::<String, usize>::new());
    for tile in &m.tiles {
        let (n, k) = parts(db(&app), &tile.tile);
        all += n;
        for (kind, count) in k {
            *kinds.entry(kind).or_insert(0) += count;
        }
    }
    assert_eq!(draw.sockets.len(), all);
    assert!(kinds["road"] > 5 && kinds["water"] > 0 && kinds["grass"] > kinds["road"], "a meadow with roads and a river: {kinds:?}");
    let (grass, water) = (kind_color("grass", false), kind_color("water", false));
    assert!(grass != water && grass != ROAD_COLOR && water != ROAD_COLOR, "three colors");
    let count = |color: Color| draw.sockets.iter().filter(|s| s.color == color).count();
    assert_eq!(count(ROAD_COLOR), kinds["road"], "roads are orange");
    assert_eq!(count(grass), kinds["grass"]);
    assert_eq!(count(water), kinds["water"]);
    assert_eq!(count(ROAD_COLOR) + count(grass) + count(water), draw.sockets.len(), "no other color");

    // The marks of a tile lie along its four sides, 0.4 m inside, in its own footprint.
    let mut next = 0;
    for tile in &m.tiles {
        let (n, _) = parts(db(&app), &tile.tile);
        let min = Vec3::new(tile.at[0] as f32 * 16.0, 0.05, tile.at[1] as f32 * 16.0);
        for mark in &draw.sockets[next..next + n] {
            for p in [mark.a, mark.b] {
                let (x, z) = (p.x - min.x, p.z - min.z);
                assert!((0.0..=16.0).contains(&x) && (0.0..=16.0).contains(&z), "{tile:?}: {p} outside");
                let inside = x.min(16.0 - x).min(z.min(16.0 - z));
                assert!((inside - 0.4).abs() < 1e-3, "{tile:?}: {p} is {inside} m from the edge");
                assert!((p.y - 0.05).abs() < 1e-6);
            }
        }
        next += n;
    }

    // A ball on each point of interest, half a meter above it.
    assert_eq!(draw.pois.len(), m.pois.len());
    assert!(m.pois.len() >= 4, "spawn, camps, shrine and chests: {}", m.pois.len());
    for (ball, poi) in draw.pois.iter().zip(&m.pois) {
        assert!(close(ball.at, poi.position() + Vec3::Y * 0.5), "{ball:?} for {poi:?}");
        assert_eq!((ball.radius, ball.color), (POI_RADIUS, POI_COLOR));
    }

    // A line from cell center to cell center along each road.
    let cells: usize = m.links.iter().map(|l| l.cells.len() - 1).sum();
    assert!(!m.links.is_empty() && cells > 3, "the meadow has roads: {} links", m.links.len());
    assert!(m.links.iter().all(|l| l.kind == "road"));
    assert_eq!(draw.links.len(), cells);
    let mut lines = draw.links.iter();
    for link in &m.links {
        for pair in link.cells.windows(2) {
            let line = lines.next().unwrap();
            assert!(close(line.a, m.cell_center(pair[0]) + Vec3::Y * 0.2), "{line:?} from {:?}", pair[0]);
            assert!(close(line.b, m.cell_center(pair[1]) + Vec3::Y * 0.2), "{line:?} to {:?}", pair[1]);
            assert_eq!(line.color, LINK_COLOR);
        }
    }
}

#[test]
fn the_road_marks_of_a_turned_tile_sit_on_the_turned_sides_and_meet_their_neighbors() {
    let app = app(1, None);
    // Three attempts at "impossible" cannot be made: the hand-made meadow, a road along the
    // north edge, from `road_end:90` through six `road_straight:90` to `road_end:270`.
    let made = generate(db(&app), &worldgen(&app), "impossible", 3).unwrap();
    let m = &made.manifest;
    assert_eq!(m.attempt, 0, "the fallback");
    assert_eq!(m.tiles[0].tile, "road_end");
    assert_eq!((m.tiles[0].rot, m.tiles[1].tile.as_str(), m.tiles[1].rot), (90, "road_straight", 90));
    let draw = TileDraw::of(m, db(&app));

    // `road_end` has its road on the north edge; turned 90 degrees it is on the east edge.
    // The marks go north edge, east, south, west: 1 + 3 + 1 + 1 for a tile of 16 m. The
    // middle part of the east edge is the road, at x = 16 - 0.4, a third of the 15.2 m
    // between the corners' insets, less 0.2 m at each end.
    let first: Vec<&Seg> = draw.sockets[..6].iter().collect();
    let part = 15.2 / 3.0;
    let road = first[2];
    assert_eq!(road.color, ROAD_COLOR);
    assert!(close(road.a, Vec3::new(15.6, 0.05, 0.4 + part + 0.2)), "{road:?}");
    assert!(close(road.b, Vec3::new(15.6, 0.05, 0.4 + 2.0 * part - 0.2)), "{road:?}");
    let others: Vec<Color> = first.iter().enumerate().filter(|(i, _)| *i != 2).map(|(_, s)| s.color).collect();
    assert!(others.iter().all(|c| *c == kind_color("grass", false)), "the rest is grass: {others:?}");

    // Along the row, 14 road marks in all: one on each end tile, two on each tile between.
    let roads: Vec<&Seg> = draw.sockets.iter().filter(|s| s.color == ROAD_COLOR).collect();
    assert_eq!(roads.len(), 1 + 6 * 2 + 1);
    let stretch = |s: &Seg| (s.a.z.min(s.b.z), s.a.z.max(s.b.z));
    assert!(roads.iter().all(|s| stretch(s).0 > 5.6 && stretch(s).1 < 10.4), "all along the same stretch of the row");
    // Neighbors face each other: the east mark of one tile and the west mark of the next
    // are on either side of the shared edge, over the same stretch.
    for seam in 1..8 {
        let edge = 16.0 * seam as f32;
        let east = roads.iter().find(|s| (s.a.x - (edge - 0.4)).abs() < 1e-3).expect("a mark east of the tile before the seam");
        let west = roads.iter().find(|s| (s.a.x - (edge + 0.4)).abs() < 1e-3).expect("a mark west of the tile after the seam");
        // (The west side is read south to north, the east side north to south: the ends swap.)
        assert!((stretch(east).0 - stretch(west).0).abs() < 1e-3 && (stretch(east).1 - stretch(west).1).abs() < 1e-3, "seam {seam}: {east:?} / {west:?}");
    }
}

/// A one-tile meadow of `tile` turned `rot` degrees, for `TileDraw::of`.
fn lone(tile: &str, rot: u16) -> RegionManifest {
    RegionManifest {
        format: FORMAT.into(),
        region: "meadow".into(),
        layout: Layout::Grid,
        seed: 0,
        version: 1,
        attempt: 1,
        fallback: None,
        size: [1, 1],
        tile_size: 16.0,
        origin: [0.0, 0.0],
        tiles: vec![PlacedTile { at: [0, 0], size: [1, 1], tile: tile.into(), rot, decor: None }],
        pois: Vec::new(),
        links: Vec::new(),
        spawn: None,
        content_hash: String::new(),
        steps: Vec::new(),
    }
}

#[test]
fn the_parts_of_an_edge_run_clockwise_round_the_tile_whichever_way_it_is_turned() {
    let app = app(1, None);
    let shore = r#"{
        "shore": (
            sockets: (n: "grass road water", e: "water", s: "grass", w: "grass"),
            rotations: [0, 90, 180, 270],
            geometry: Inline([Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0))]),
        ),
    }"#;
    let db = db_with(&app, &[("world/shore.tile.ron", shore)]);
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let (grass, water) = (kind_color("grass", false), kind_color("water", false));
    // The long edge starts on the north side (turned 0) and goes round with the tile: west
    // to east on the north side, north to south on the east, east to west on the south,
    // south to north on the west.
    let runs: [fn(Vec3) -> f32; 4] = [|p| p.x, |p| p.z, |p| -p.x, |p| -p.z];
    for (turns, along) in runs.into_iter().enumerate() {
        let rot = turns as u16 * 90;
        let draw = TileDraw::of(&lone("shore", rot), &db);
        assert_eq!(draw.sockets.len(), 6, "rot {rot}");
        // Marks come north, east, south, west: the long edge is on side `turns`, after one
        // mark for each of the sides before it.
        let long = &draw.sockets[turns..turns + 3];
        assert_eq!([long[0].color, long[1].color, long[2].color], [grass, ROAD_COLOR, water], "rot {rot}: {long:?}");
        let centers: Vec<f32> = long.iter().map(|s| along((s.a + s.b) * 0.5)).collect();
        assert!(centers[0] < centers[1] && centers[1] < centers[2], "rot {rot}: {centers:?}");
        // All three on the same side of the tile, 0.4 m inside it.
        let side = |p: Vec3| match turns {
            0 => p.z,
            1 => 16.0 - p.x,
            2 => 16.0 - p.z,
            _ => p.x,
        };
        assert!(long.iter().all(|s| (side(s.a) - 0.4).abs() < 1e-3 && (side(s.b) - 0.4).abs() < 1e-3), "rot {rot}: {long:?}");
        // The east edge's single `water` part turned along with it: on side `turns + 1`, one
        // mark further on than its place in the list when the long edge comes before it.
        let side = (turns + 1) % 4;
        let at = side + if turns < side { 2 } else { 0 };
        let singles: Vec<(usize, Color)> =
            draw.sockets.iter().enumerate().filter(|(i, _)| !(turns..turns + 3).contains(i)).map(|(i, s)| (i, s.color)).collect();
        assert_eq!(singles.len(), 3, "rot {rot}");
        for (i, color) in singles {
            assert_eq!(color, if i == at { water } else { grass }, "rot {rot}: mark {i}, the water one is {at}");
        }
    }

    // A rotation that is no quarter turn has no sockets to place, only its bounds.
    let odd = TileDraw::of(&lone("shore", 45), &db);
    assert_eq!((odd.bounds.len(), odd.sockets.len()), (1, 0));
}

#[test]
fn a_crypt_has_room_bounds_doors_and_no_sockets() {
    let mut app = app(4, Some("crypt"));
    ok(&mut app, "world_overlay", json!({}));
    frames(&mut app, 2);
    let m = region(&app).manifest.clone();
    assert_eq!(m.layout, Layout::Rooms);
    let draw = drawn(&app);
    assert!(draw.sockets.is_empty(), "rooms have doors, not sockets");

    // A box per room, as big as the room's cells: the throne is 2 × 2 cells of 8 m.
    assert_eq!(draw.bounds.len(), m.tiles.len());
    for (outline, room) in draw.bounds.iter().zip(&m.tiles) {
        let [c0, c1, c2, c3] = outline.corners;
        let (w, h) = (room.size[0] as f32 * m.tile_size, room.size[1] as f32 * m.tile_size);
        assert!(close(c1 - c0, Vec3::X * w) && close(c3 - c0, Vec3::Z * h) && close(c2 - c0, Vec3::new(w, 0.0, h)), "{room:?}: {:?}", outline.corners);
        assert!(close(c0, Vec3::new(room.at[0] as f32 * m.tile_size, 0.05, room.at[1] as f32 * m.tile_size)), "{room:?}");
    }
    let throne = m.tiles.iter().position(|t| t.tile == "throne").unwrap();
    let c = draw.bounds[throne].corners;
    assert!(close(c[2] - c[0], Vec3::new(16.0, 0.0, 16.0)), "{c:?}");

    // A line through each door, from the cell on one side to the cell on the other.
    assert!(!m.links.is_empty() && m.links.iter().all(|l| l.kind == "door" && l.cells.len() == 2));
    assert_eq!(draw.links.len(), m.links.len());
    for (line, door) in draw.links.iter().zip(&m.links) {
        assert!(close(line.a, m.cell_center(door.cells[0]) + Vec3::Y * 0.2) && close(line.b, m.cell_center(door.cells[1]) + Vec3::Y * 0.2), "{line:?} / {door:?}");
        assert!((line.a.distance(line.b) - m.tile_size).abs() < 1e-3, "a door joins neighboring cells");
        assert_eq!(line.color, LINK_COLOR);
    }

    // The boss's throne room has a ball on its point.
    assert_eq!(draw.pois.len(), m.pois.len());
    let boss = m.pois.iter().position(|p| p.poi == "boss").expect("the throne's boss");
    assert!(close(draw.pois[boss].at, m.pois[boss].position() + Vec3::Y * 0.5));
}

#[test]
fn a_new_region_is_drawn_as_soon_as_it_has_loaded_and_the_overlay_stays_on() {
    let mut app = app(3, Some("meadow"));
    ok(&mut app, "world_overlay", json!({}));
    frames(&mut app, 2);
    assert_eq!(drawn(&app).bounds.len(), 64);

    ok(&mut app, "regen", json!({"region": "walled", "seed": 3}));
    frames(&mut app, 2);
    assert!(app.world().resource::<TileOverlay>().enabled);
    let m = region(&app).manifest.clone();
    assert_eq!((m.region.as_str(), m.tiles.len()), ("walled", 24));
    let draw = drawn(&app);
    assert_eq!((draw.bounds.len(), draw.pois.len()), (24, m.pois.len()));
    let kinds: Vec<&str> = m.pois.iter().map(|p| p.poi.as_str()).collect();
    assert!(kinds.contains(&"spawn") && kinds.contains(&"camp"), "{kinds:?}");
    // The fixed road end in the north-west corner: its road mark on the east edge.
    assert!(draw.sockets.iter().any(|s| s.color == ROAD_COLOR && s.a.x > 15.0 && s.a.z < 16.0), "{:?}", draw.sockets.len());
    assert!(draw.bounds.iter().all(|o| o.corners.iter().all(|c| c.x >= 0.0 && c.x <= 6.0 * 16.0 && c.z >= 0.0 && c.z <= 4.0 * 16.0)), "inside the new region");

    // The region's origin moves its drawing with it.
    ok(&mut app, "regen", json!({"region": "walled_far", "seed": 3}));
    frames(&mut app, 2);
    let first = drawn(&app).bounds[0].corners[0];
    assert!(close(first, Vec3::new(-200.0, 0.05, 64.0)), "{first}");
}

#[test]
fn without_the_gizmo_plugin_the_tool_still_answers_and_nothing_is_drawn() {
    // A game that never added gizmos (the engine's group minus `GizmoPlugin`): no store of
    // gizmo configs, so the overlay's systems are not even registered.
    let saves = std::env::temp_dir().join(format!("shards_world_overlay_{}", std::process::id()));
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config(1)).build().disable::<GizmoPlugin>());
    // Saves go to a folder of this test's own, like `testing::test_app` does.
    app.world_mut().resource_mut::<SaveConfig>().dir = saves.clone();
    app.start_region("meadow");
    game(&mut app);
    app.finish();
    app.cleanup();
    testing::wait_until_loaded(&mut app, testing::LOAD_TIMEOUT).unwrap();
    assert!(!app.is_plugin_added::<GizmoPlugin>());
    assert_eq!(ok(&mut app, "world_overlay", json!({})), "world overlay on");
    assert!(app.world().resource::<TileOverlay>().enabled);
    frames(&mut app, 3);
    assert!(nothing(drawn(&app)), "nothing was worked out: {} tiles", region(&app).manifest.tiles.len());
    assert_eq!(ok(&mut app, "world_overlay", json!({"on": false})), "world overlay off");
    let _ = std::fs::remove_dir_all(saves);
}

#[test]
fn a_volume_has_no_tile_outlines_and_its_points_are_drawn() {
    let app = app(1, None);
    let mut m = lone("meadow", 0);
    m.layout = Layout::Volume;
    m.tiles.clear();
    m.size = [2, 2];
    m.steps = vec![("hills".into(), 1)];
    m.pois = vec![shards::world::manifest::ManifestPoi { id: "spawn.0".into(), poi: "spawn".into(), cell: [1, 1], pos: [20.0, 1.0, 20.0], area: 0 }];
    let draw = TileDraw::of(&m, db(&app));
    assert!(draw.bounds.is_empty() && draw.sockets.is_empty() && draw.links.is_empty(), "{draw:?}");
    assert_eq!(draw.pois.len(), 1);
    assert!(close(draw.pois[0].at, Vec3::new(20.0, 1.5, 20.0)));
}
