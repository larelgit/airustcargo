//! The navigation overlay (`nav_overlay`), without a GPU or a window: what it hands to the
//! gizmos is kept in `NavDraw`, and these tests read it, as the combat overlay's are read
//! through `CombatDraw`. The arena has the gizmo plugin (gizmos only collect lines: drawing
//! them needs a GPU) and the engine's modules look for it as they are built.

use std::path::{Path, PathBuf};

use bevy::gizmos::GizmoPlugin;
use bevy::prelude::*;
use bevy::transform::TransformPlugin;
use serde_json::json;
use shards_level::{LevelMarker, MarkerKind};
use shards_npc::nav::overlay::{NavDraw, CARVED_COLOR, CELL_COLOR, NAVLINK_COLOR, NEAR, ROUTE_COLOR, STITCH_COLOR, WATER_COLOR};
use shards_npc::prelude::*;
use shards_world::overlay::TileDraw;
use shards_world::TileOverlay;

use crate::common::*;

/// The arena with gizmos: the assets their helper systems look at, and transforms.
fn gizmos(app: &mut App) {
    app.init_asset::<Mesh>().init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>();
    app.add_plugins((TransformPlugin, GizmoPlugin));
}

fn windowed() -> App {
    windowed_in(ASSETS)
}

fn windowed_in(dir: &str) -> App {
    let mut app = app_in_with(dir, 7, gizmos, |_| {});
    floor(app.world_mut());
    app
}

/// Frames the window would draw. (A paused game still runs `PostUpdate`.)
fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn draw(app: &App) -> &NavDraw {
    app.world().resource::<NavDraw>()
}

fn nothing(draw: &NavDraw) -> bool {
    draw.cells.is_empty() && draw.stitches.is_empty() && draw.navlinks.is_empty() && draw.routes.is_empty()
}

fn nav(app: &App) -> &NavRegion {
    app.world().resource::<NavRegion>()
}

fn count(draw: &NavDraw, color: Color) -> usize {
    draw.cells.iter().filter(|c| c.color == color).count()
}

fn flat(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

/// How many cells lie within reach of any of `players`, worked out cell by cell.
fn near(app: &App, players: &[Vec3]) -> usize {
    let nav = nav(app);
    (0..nav.node_count()).filter(|n| players.iter().any(|p| flat(nav.node_pos(*n), *p) <= NEAR)).count()
}

#[test]
fn nothing_is_drawn_until_the_tool_asks_and_it_brings_the_tile_overlay_with_it() {
    let mut app = windowed();
    load_region(&mut app, &["floor cliff floor"]);
    frames(&mut app, 2);
    assert!(nothing(draw(&app)) && draw(&app).arm == 0.0, "off by default");
    assert!(app.world().resource::<TileDraw>().bounds.is_empty());

    let out = ok(&mut app, "nav_overlay", json!({}));
    assert_eq!(text(&out), "nav overlay on");
    frames(&mut app, 2);
    // No player stands anywhere: every cell is drawn, a cross of 0.3 cells to each side.
    let cells = nav(&app).node_count() as usize;
    assert!(cells > 500, "three tiles of 8 m: {cells}");
    assert_eq!(draw(&app).cells.len(), cells);
    assert!((draw(&app).arm - nav(&app).cell * 0.3).abs() < 1e-6);
    // The tile bounds and sockets come with it, as the tool says it does.
    assert!(app.world().resource::<TileOverlay>().enabled);
    assert_eq!(app.world().resource::<TileDraw>().bounds.len(), 3);

    let out = ok(&mut app, "nav_overlay", json!({"on": false}));
    assert_eq!(text(&out), "nav overlay off");
    frames(&mut app, 2);
    assert!(nothing(draw(&app)));
    assert!(!app.world().resource::<TileOverlay>().enabled);
    assert!(app.world().resource::<TileDraw>().bounds.is_empty());
}

/// The test tiles with a pond: ground of 8 m, a 4 m square of deep water in the middle.
fn assets_with_pond() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("overlay_pond_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("content/world")).unwrap();
    for file in ["nav.region.ron", "nav.socket_rules.ron", "nav.poi.ron"] {
        std::fs::copy(Path::new(ASSETS).join("content/world").join(file), dir.join("content/world").join(file)).unwrap();
    }
    let tiles = std::fs::read_to_string(Path::new(ASSETS).join("content/world/nav.tile.ron")).unwrap();
    let pond = r#"    "pond": (
        sockets: (n: "open", e: "open", s: "open", w: "open"),
        geometry: Inline([
            Box(min: (-4.0, -1.0, -4.0), max: (4.0, 0.0, 4.0)),
            Volume(kind: Water, shape: Cuboid(half: (2.0, 2.0, 2.0)), at: (0.0, 0.0, 0.0)),
        ]),
    ),
}
"#;
    let end = tiles.rfind('}').expect("the tile file closes its map");
    std::fs::write(dir.join("content/world/nav.tile.ron"), format!("{}{pond}", &tiles[..end])).unwrap();
    dir
}

#[test]
fn cells_are_green_to_walk_blue_to_swim_and_red_where_a_solid_carved_them_out() {
    let dir = assets_with_pond();
    let mut app = windowed_in(dir.to_str().unwrap());
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    load_region(&mut app, &["pond floor"]);
    ok(&mut app, "nav_overlay", json!({}));
    frames(&mut app, 2);

    let swimmable = nav(&app).instances.iter().map(|i| i.nav.nodes.iter().filter(|n| n.swim).count()).sum::<usize>();
    let all = nav(&app).node_count() as usize;
    assert!(swimmable > 20 && swimmable < all / 2, "a pond of 4 × 4 m: {swimmable} of {all}");
    let d = draw(&app);
    assert_eq!(count(d, WATER_COLOR), swimmable, "water is blue");
    assert_eq!(count(d, CARVED_COLOR), 0, "nothing carved yet");
    assert_eq!(count(d, CELL_COLOR), all - swimmable, "the rest is green");
    // Blue is the pond's: the middle of the first tile, floating at the water's surface.
    for cell in d.cells.iter().filter(|c| c.color == WATER_COLOR) {
        assert!((1.5..=6.5).contains(&cell.at.x) && (1.5..=6.5).contains(&cell.at.z), "{cell:?} outside the pond");
        assert!(cell.at.y > 0.5, "afloat: {cell:?}");
    }
    assert!(d.cells.iter().filter(|c| c.color == CELL_COLOR).all(|c| c.at.y < 0.1), "walkers stand on the ground");

    // A wall the bakes never saw, in the second tile: the cells round it are switched off.
    wall(app.world_mut(), Vec3::new(11.5, 0.0, 2.0), Vec3::new(12.5, 2.5, 6.0));
    step(&mut app, 1);
    frames(&mut app, 1);
    let carved = nav(&app).report.carved;
    assert!(carved > 0, "the wall carves");
    let d = draw(&app);
    assert_eq!(count(d, CARVED_COLOR), carved, "carved cells are red");
    assert_eq!(count(d, WATER_COLOR), swimmable);
    assert_eq!(count(d, CELL_COLOR), all - swimmable - carved);
    assert!(d.cells.iter().filter(|c| c.color == CARVED_COLOR).all(|c| (9.0..=15.0).contains(&c.at.x) && (1.0..=7.0).contains(&c.at.z)), "round the wall");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_cells_and_links_near_a_player_are_drawn() {
    let mut app = windowed();
    load_region(&mut app, &["floor floor floor floor floor floor floor floor"]);
    ok(&mut app, "nav_overlay", json!({}));
    frames(&mut app, 1);
    let all = nav(&app).node_count() as usize;
    let stitched = nav(&app).report.stitched;
    assert_eq!((draw(&app).cells.len(), draw(&app).stitches.len()), (all, stitched), "no player, everything");

    // A player at the west end sees 24 m round it: a part of the 64 m strip.
    let west = Vec3::new(4.0, 0.0, 4.0);
    let one = player(app.world_mut(), 0, west);
    frames(&mut app, 1);
    let d = draw(&app);
    assert_eq!(d.cells.len(), near(&app, &[west]));
    assert!(d.cells.len() > 500 && d.cells.len() < all, "{} of {all}", d.cells.len());
    assert!(d.cells.iter().all(|c| flat(c.at, west) <= NEAR + 1e-3), "a cell out of reach");
    assert!(!d.stitches.is_empty() && d.stitches.len() < stitched, "{} of {stitched} links", d.stitches.len());
    assert!(d.stitches.iter().all(|s| flat(s.a, west) <= NEAR + 1e-3), "a link out of reach");

    // A second player, far from the first, brings its own surroundings.
    let east = Vec3::new(60.0, 0.0, 4.0);
    let two = player(app.world_mut(), 1, east);
    frames(&mut app, 1);
    assert_eq!(draw(&app).cells.len(), near(&app, &[west, east]));
    assert!(draw(&app).cells.iter().any(|c| flat(c.at, east) <= NEAR) && draw(&app).cells.iter().any(|c| flat(c.at, west) <= NEAR));

    // Both gone: back to the whole region.
    app.world_mut().despawn(one);
    app.world_mut().despawn(two);
    frames(&mut app, 1);
    assert_eq!((draw(&app).cells.len(), draw(&app).stitches.len()), (all, stitched));
}

#[test]
fn links_across_tile_edges_are_cyan_lines_and_navlinks_yellow_arrows() {
    let mut app = windowed();
    load_region(&mut app, &["floor floor floor"]);
    ok(&mut app, "nav_overlay", json!({}));
    frames(&mut app, 2);
    let stitched = nav(&app).report.stitched;
    let d = draw(&app);
    assert!(stitched > 30, "two seams of 16 cells, straight and diagonal: {stitched}");
    assert_eq!(d.stitches.len(), stitched);
    assert!(d.navlinks.is_empty(), "no navlinks here");
    for link in &d.stitches {
        assert_eq!(link.color, STITCH_COLOR);
        // From one tile to the next (the seams are at x = 8 and 16), a step or a diagonal.
        assert_ne!((link.a.x / 8.0).floor(), (link.b.x / 8.0).floor(), "{link:?} stays in its tile");
        assert!(link.a.distance(link.b) < 1.0, "{link:?} is a short hop");
    }

    // A rope from the plateau's edge down to the ground at its foot.
    let mut app = windowed();
    app.world_mut().spawn((
        LevelMarker { kind: MarkerKind::NavLink, name: "rope".into(), to: Some(Vec3::new(11.0, 0.0, 4.0)) },
        Transform::from_xyz(12.6, 4.0, 4.0),
    ));
    load_region(&mut app, &["floor cliff floor"]);
    ok(&mut app, "nav_overlay", json!({}));
    frames(&mut app, 2);
    let rope = &nav(&app).navlinks[0];
    let (from, to) = rope.nodes.expect("the rope is used");
    let d = draw(&app);
    assert_eq!(d.navlinks.len(), 1, "one navlink, one arrow");
    assert_eq!(d.stitches.len(), nav(&app).report.stitched, "the navlink is not a stitch");
    let arrow = d.navlinks[0];
    assert_eq!(arrow.color, NAVLINK_COLOR);
    // From the node it starts at to the node it lands on, lifted to be seen.
    let lifted = Vec3::Y * (0.12 + 0.3);
    assert!(arrow.a.distance(nav(&app).node_pos(from) + lifted) < 1e-4, "{arrow:?}");
    assert!(arrow.b.distance(nav(&app).node_pos(to) + lifted) < 1e-4, "{arrow:?}");
    assert!((arrow.a.y - arrow.b.y - 4.0).abs() < 0.1, "down the cliff: {arrow:?}");
    assert_eq!(rope.name, "rope");
}

#[test]
fn an_npc_with_a_route_has_it_drawn_from_where_it_stands_to_its_goal() {
    let mut app = windowed();
    load_region(&mut app, &["floor pillar floor"]);
    let world = app.world_mut();
    let home_at = Vec3::new(4.0, 0.0, 4.0);
    // It senses through walls: what stands between it and the hero is the pillar.
    let sentry = npc(world, "grunt", Npc::new("sentry", "boss"), home_at, Vec3::X);
    home(world, sentry, home_at, 10.0);
    player(world, 0, Vec3::new(14.6, 0.0, 4.0));
    ok(&mut app, "nav_overlay", json!({}));
    frames(&mut app, 1);
    assert!(draw(&app).routes.is_empty(), "no one walks yet");

    let mut walking = false;
    for _ in 0..240 {
        step(&mut app, 1);
        let agent = app.world().get::<NavAgent>(sentry).unwrap();
        if !agent.direct && agent.path.len() > 1 {
            walking = true;
            break;
        }
    }
    assert!(walking, "it routes round the pillar");
    frames(&mut app, 1);
    let agent = app.world().get::<NavAgent>(sentry).unwrap().clone();
    let at = app.world().get::<Transform>(sentry).unwrap().translation;
    let d = draw(&app);
    assert_eq!(d.routes.len(), 1, "one NPC walks");
    let route = &d.routes[0];
    assert_eq!(route.color, ROUTE_COLOR);
    // From the NPC through the waypoints it has left, lifted off the ground.
    let lift = Vec3::Y * 0.24;
    let left = &agent.path[agent.index as usize..];
    assert_eq!(route.points.len(), left.len() + 1);
    assert!(route.points[0].distance(at + lift) < 1e-4, "starts at the NPC: {:?} / {at}", route.points[0]);
    for (point, waypoint) in route.points[1..].iter().zip(left) {
        assert!(point.distance(*waypoint + lift) < 1e-4, "{point} / {waypoint}");
    }
    assert!(route.points.len() > 2, "round the pillar, not straight: {:?}", route.points);

    // Waypoints it has passed are not drawn: with one left, the NPC and that point.
    app.world_mut().get_mut::<NavAgent>(sentry).unwrap().index = agent.path.len() as u32 - 1;
    frames(&mut app, 1);
    let route = &draw(&app).routes[0];
    assert_eq!(route.points.len(), 2, "{:?}", route.points);
    assert!(route.points[1].distance(*agent.path.last().unwrap() + lift) < 1e-4, "the goal");

    // Without a route (the goal reached, or none) nothing is drawn for it.
    app.world_mut().get_mut::<NavAgent>(sentry).unwrap().path.clear();
    frames(&mut app, 1);
    assert!(draw(&app).routes.is_empty());
}

#[test]
fn without_the_gizmo_plugin_the_tool_answers_and_nothing_is_drawn() {
    // The arena of the other tests minus gizmos: a game that never draws. The tool answers
    // and flips the switches; the systems that would draw are not even registered.
    let mut app = crate::common::app();
    load_region(&mut app, &["floor cliff floor"]);
    let out = ok(&mut app, "nav_overlay", json!({}));
    assert_eq!(text(&out), "nav overlay on");
    assert!(app.world().resource::<TileOverlay>().enabled);
    frames(&mut app, 2);
    assert!(nothing(draw(&app)));
    assert!(app.world().resource::<TileDraw>().bounds.is_empty());
    let out = ok(&mut app, "nav_overlay", json!({"on": false}));
    assert_eq!(text(&out), "nav overlay off");
    assert!(!app.world().resource::<TileOverlay>().enabled);
}
