//! Body classes of the navmesh (E6.1): every tile baked once per class of the nav settings,
//! a navmesh per class, routes on the tightest class a body fits, the local grid beyond.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde_json::json;
use shards_content::ContentDb;
use shards_motion::{CharacterParams, PhysicsWorld};
use shards_npc::nav::bake::bake;
use shards_npc::nav::settings::NavSettings;
use shards_npc::nav::tile::TileKind;
use shards_npc::prelude::*;
use shards_npc::{navmesh_path, plain_sight, routes_for, NavQuery};

use crate::common::*;

/// A tile with a wall north to south through its middle and two ways through it, in the
/// tile's frame: a 1.5 m slot (z -2.75..-1.25), wide enough for a 0.8 m body, and a 3.5 m
/// door (z 0.25..3.75).
const GATES: &str = r#"
    "gates": (
        sockets: (n: "open", e: "open", s: "open", w: "open"),
        geometry: Inline([
            Box(min: (-4.0, -1.0, -4.0), max: (4.0, 0.0, 4.0)),
            Box(min: (-0.25, 0.0, -4.0), max: (0.25, 3.0, -2.75)),
            Box(min: (-0.25, 0.0, -1.25), max: (0.25, 3.0, 0.25)),
            Box(min: (-0.25, 0.0, 3.75), max: (0.25, 3.0, 4.0)),
        ]),
    ),
"#;

/// The wall's three pieces in the world when `gates` is the middle tile of a strip of 8 m
/// tiles (x 8..16): `(min, max)` on the ground plane.
const WALLS: [(Vec2, Vec2); 3] = [
    (Vec2::new(11.75, 0.0), Vec2::new(12.25, 1.25)),
    (Vec2::new(11.75, 2.75), Vec2::new(12.25, 4.25)),
    (Vec2::new(11.75, 7.75), Vec2::new(12.25, 8.0)),
];

/// `gates` with a lintel over the slot, 2 m above the ground: a body of the default 1.8 m
/// walks under it, a taller one goes by the door.
const ARCH: &str = r#"
    "arch": (
        sockets: (n: "open", e: "open", s: "open", w: "open"),
        geometry: Inline([
            Box(min: (-4.0, -1.0, -4.0), max: (4.0, 0.0, 4.0)),
            Box(min: (-0.25, 0.0, -4.0), max: (0.25, 3.0, -2.75)),
            Box(min: (-0.25, 2.0, -2.75), max: (0.25, 3.0, -1.25)),
            Box(min: (-0.25, 0.0, -1.25), max: (0.25, 3.0, 0.25)),
            Box(min: (-0.25, 0.0, 3.75), max: (0.25, 3.0, 4.0)),
        ]),
    ),
"#;

/// The class `large`: a 1.2 m radius, too wide for the slot.
const LARGE: &str = "{ \"default\": (cell: 0.5, bodies: {\"large\": (radius: Some(1.2), height: Some(2.4))}) }\n";

/// The class `tall`: as wide as `agent`, 2.6 m tall.
const TALL: &str = "{ \"default\": (cell: 0.5, bodies: {\"tall\": (height: Some(2.6))}) }\n";

/// A temporary asset folder with the navigation test world, the `gates` tile and the nav
/// definition `nav`.
fn assets_with(name: &str, nav: &str) -> PathBuf {
    assets_with_tiles(name, nav, &[GATES])
}

/// [`assets_with`] the test tiles and `tiles` (definitions of `nav.tile.ron`).
fn assets_with_tiles(name: &str, nav: &str, tiles: &[&str]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bodies_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("content/world")).unwrap();
    for file in [
        "fighters.action.ron",
        "fighters.moveset.ron",
        "npc.attack_plan.ron",
        "npc.behavior.ron",
        "npc.director.ron",
        "npc.perception.ron",
        "tuning.npc_tuning.ron",
        "world/nav.region.ron",
        "world/nav.socket_rules.ron",
        "world/nav.poi.ron",
    ] {
        std::fs::copy(Path::new(ASSETS).join("content").join(file), dir.join("content").join(file)).unwrap();
    }
    let base = std::fs::read_to_string(Path::new(ASSETS).join("content/world/nav.tile.ron")).unwrap();
    let end = base.rfind('}').unwrap();
    std::fs::write(dir.join("content/world/nav.tile.ron"), format!("{}{}}}\n", &base[..end], tiles.concat())).unwrap();
    std::fs::write(dir.join("content/world/default.nav.ron"), nav).unwrap();
    dir
}

fn errors(app: &App) -> Vec<String> {
    app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect()
}

/// Where the flat polyline `points` (from `start`) crosses x = 12: the z of each crossing.
fn crossings(start: Vec3, points: &[Vec3]) -> Vec<f32> {
    let mut at = start;
    let mut out = Vec::new();
    for p in points {
        if (at.x - 12.0) * (p.x - 12.0) <= 0.0 && (p.x - at.x).abs() > 1e-6 {
            let t = (12.0 - at.x) / (p.x - at.x);
            out.push(at.z + (p.z - at.z) * t);
        }
        at = *p;
    }
    out
}

/// How close the flat polyline comes to the wall pieces (m).
fn closest_to_walls(start: Vec3, points: &[Vec3]) -> f32 {
    let mut at = start;
    let mut closest = f32::INFINITY;
    for p in points {
        for k in 0..=100 {
            let q = at.lerp(*p, k as f32 / 100.0);
            for (min, max) in WALLS {
                let dx = (min.x - q.x).max(q.x - max.x).max(0.0);
                let dz = (min.y - q.z).max(q.z - max.y).max(0.0);
                closest = closest.min(Vec2::new(dx, dz).length());
            }
        }
        at = *p;
    }
    closest
}

fn points(value: &serde_json::Value) -> Vec<Vec3> {
    value["waypoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| Vec3::new(w["x"].as_f64().unwrap() as f32, 0.0, w["z"].as_f64().unwrap() as f32))
        .collect()
}

#[test]
fn a_large_body_routes_on_its_own_navmesh_through_the_door_it_fits() {
    let dir = assets_with("route", LARGE);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    load_region(&mut app, &["floor gates floor"]);
    let info = ok(&mut app, "nav_info", json!({}));
    let bodies = info.result["bodies"].as_array().unwrap();
    assert_eq!(bodies.iter().map(|b| b["body"].as_str().unwrap()).collect::<Vec<_>>(), ["agent", "large"], "{}", info.result);
    assert_eq!(bodies[1]["radius"], 1.2);
    assert!(all_journal(&app).iter().any(|l| l.contains("navmesh (body large): 3 tiles, ")), "{:#?}", all_journal(&app));

    let (from, to) = (json!({"x": 4.0, "z": 3.0}), json!({"x": 20.0, "z": 3.0}));
    let start = Vec3::new(4.0, 0.0, 3.0);
    // The agent's body: through the slot, the short way.
    let agent = ok(&mut app, "nav_path", json!({"from": from, "to": to}));
    assert_eq!(agent.result["body"], "agent", "{}", agent.result);
    let through = crossings(start, &points(&agent.result));
    assert!(through.len() == 1 && (1.25..2.75).contains(&through[0]), "through the slot: {through:?} {}", agent.result);
    // The large body: the slot is too narrow, round by the door, never nearer a wall than
    // its radius.
    let large = ok(&mut app, "nav_path", json!({"from": from, "to": to, "body": "large"}));
    assert_eq!(large.result["body"], "large", "{}", large.result);
    let large_points = points(&large.result);
    let through = crossings(start, &large_points);
    assert!(through.len() == 1 && (5.4..6.6).contains(&through[0]), "through the door: {through:?} {}", large.result);
    let clearance = closest_to_walls(start, &large_points);
    assert!(clearance >= 1.2 - 0.05, "{clearance:.2} m from a wall: {}", large.result);
    // The same for an NPC's routes (its whole body): the tightest class that serves it.
    let wide = |radius: f32| CharacterParams { radius, ..default() };
    let npc_route = navmesh_path(app.world(), start, Vec3::new(20.0, 0.0, 3.0), &wide(1.1)).unwrap();
    let through = crossings(start, &npc_route);
    assert!(through.len() == 1 && (5.4..6.6).contains(&through[0]), "an NPC of 1.1 m by the door: {npc_route:?}");
    assert_eq!(routes_for(app.world(), &wide(0.4)), "navmesh, body agent");
    assert_eq!(routes_for(app.world(), &wide(0.9)), "navmesh, body large");
    assert_eq!(routes_for(app.world(), &wide(1.5)), "local grid: no body class fits a 1.50 m radius (the widest: large, 1.20 m)");

    // Asking wrong is answered.
    let unknown = fails(&mut app, "nav_path", json!({"from": from, "to": to, "body": "larg"}));
    assert_eq!(unknown, "no body class `larg` in the nav settings (did you mean `large`?); classes: agent, large");
    let too_wide = fails(&mut app, "nav_path", json!({"from": from, "to": to, "body": "large", "radius": 1.5}));
    assert!(too_wide.contains("does not fit the body class `large` (1.20 m)"), "{too_wide}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_npc_as_wide_as_the_large_class_walks_its_navmesh_and_says_so() {
    let dir = assets_with("walk", LARGE);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    load_region(&mut app, &["floor gates floor"]);
    let world = app.world_mut();
    let (from, to) = (Vec3::new(4.0, 0.0, 3.0), Vec3::new(20.0, 0.0, 3.0));
    let big = npc(world, "grunt", Npc::new("walker", "eyes"), from, E);
    world.entity_mut(big).insert((Patrol { points: vec![to] }, CharacterParams { radius: 1.2, height: 2.4, ..default() }));
    let small = npc(world, "grunt", Npc::new("walker", "eyes"), Vec3::new(4.0, 0.0, 6.5), E);
    world.entity_mut(small).insert(Patrol { points: vec![Vec3::new(20.0, 0.0, 6.5)] });
    step(&mut app, 2);
    let (big_id, small_id) = (id(&app, big), id(&app, small));
    let state = ok(&mut app, "npc_state", json!({"id": big_id}));
    assert_eq!(state.result["nav"]["routes"], "navmesh, body large", "{}", state.result);
    let state = ok(&mut app, "npc_state", json!({"id": small_id}));
    assert_eq!(state.result["nav"]["routes"], "navmesh, body agent", "{}", state.result);
    // The overlay names a class that is not the default one.
    let lines = text(&ok(&mut app, "npc_overlay", json!({})));
    let line = |id: &str| lines.lines().find(|l| l.starts_with(&format!("{id} "))).unwrap_or_default().to_owned();
    assert!(line(&big_id).ends_with(" · body large"), "{lines}");
    assert!(!line(&small_id).contains("body "), "{lines}");
    let mut crossed = None;
    for _ in 0..600 {
        let before = feet(&app, big);
        step(&mut app, 1);
        let after = feet(&app, big);
        if before.x < 12.0 && after.x >= 12.0 {
            crossed = Some(after.z);
        }
    }
    let crossed = crossed.expect("it crossed the wall's line");
    assert!((4.25 + 1.0..7.75 - 1.0).contains(&crossed), "by the door: at z {crossed:.2}");
    assert!(flat_distance(feet(&app, big), to) < 0.6, "arrived: {}", feet(&app, big));
    // Wider than every class: the local grid, and the tools say why.
    let wide = npc(app.world_mut(), "grunt", Npc::new("walker", "eyes"), Vec3::new(4.0, 0.0, 5.0), E);
    app.world_mut().entity_mut(wide).insert((Patrol { points: vec![to] }, CharacterParams { radius: 1.6, height: 2.4, ..default() }));
    step(&mut app, 2);
    let wide_id = id(&app, wide);
    let state = ok(&mut app, "npc_state", json!({"id": wide_id}));
    assert_eq!(state.result["nav"]["routes"], "local grid: no body class fits a 1.60 m radius (the widest: large, 1.20 m)");
    let lines = text(&ok(&mut app, "npc_overlay", json!({})));
    assert!(lines.lines().any(|l| l.starts_with(&format!("{wide_id} ")) && l.ends_with(" · local grid")), "{lines}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The NPC's line in `npc_overlay`.
fn overlay_line(app: &mut App, id: &str) -> String {
    let lines = text(&ok(app, "npc_overlay", json!({})));
    lines.lines().find(|l| l.starts_with(&format!("{id} "))).unwrap_or_else(|| panic!("no line of {id}: {lines}")).to_owned()
}

#[test]
fn an_npc_outside_the_region_plans_on_the_local_grid_and_the_tools_say_so() {
    let dir = assets_with("outside", LARGE);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    load_region(&mut app, &["floor gates floor"]);
    // The strip is x 0..24, z 0..8; this ground is south of it, outside.
    let world = app.world_mut();
    wall(world, Vec3::new(0.0, -1.0, 12.0), Vec3::new(24.0, 0.0, 20.0));
    let (from, to) = (Vec3::new(4.0, 0.0, 16.0), Vec3::new(20.0, 0.0, 16.0));
    let big = npc(world, "grunt", Npc::new("walker", "eyes"), from, E);
    world.entity_mut(big).insert((Patrol { points: vec![to] }, CharacterParams { radius: 1.2, height: 2.4, ..default() }));
    step(&mut app, 2);
    let big_id = id(&app, big);
    let state = ok(&mut app, "npc_state", json!({"id": big_id}));
    assert_eq!(state.result["nav"]["routes"], "local grid: the NPC stands outside the region", "{}", state.result);
    // Its routes from there come from the local grid, as `nav_path` shows.
    let route = ok(&mut app, "nav_path", json!({"from": big_id, "to": {"x": 20.0, "z": 16.0}}));
    assert_eq!(route.result["provider"], "local grid", "{}", route.result);
    let line = overlay_line(&mut app, &big_id);
    assert!(line.ends_with(" · local grid") && !line.contains("body "), "{line}");
    // Without a position, only its body counts: the class that serves it.
    assert_eq!(routes_for(app.world(), &CharacterParams { radius: 1.2, height: 2.4, ..default() }), "navmesh, body large");
    // Back in the region, it is on its class's navmesh again.
    teleport(&mut app, big, Vec3::new(4.0, 0.0, 6.0));
    step(&mut app, 2);
    let state = ok(&mut app, "npc_state", json!({"id": big_id}));
    assert_eq!(state.result["nav"]["routes"], "navmesh, body large", "{}", state.result);
    assert!(overlay_line(&mut app, &big_id).ends_with(" · body large"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A game's routes: straight to the goal.
fn straight(_: &World, _: Vec3, to: Vec3, _: &CharacterParams) -> Option<Vec<Vec3>> {
    Some(vec![to])
}

#[test]
fn under_a_games_own_route_provider_the_tools_name_no_body_class() {
    let dir = assets_with("provider", LARGE);
    let mut app = app_in(dir.to_str().unwrap(), 7, |app| {
        app.insert_resource(NavPaths { name: "straight", find: straight });
    });
    load_region(&mut app, &["floor gates floor"]);
    let world = app.world_mut();
    // As wide as the class `large`, and wider than every class.
    let big = npc(world, "grunt", Npc::new("walker", "eyes"), Vec3::new(4.0, 0.0, 6.0), E);
    world.entity_mut(big).insert((Patrol { points: vec![Vec3::new(20.0, 0.0, 6.0)] }, CharacterParams { radius: 1.2, height: 2.4, ..default() }));
    let wide = npc(world, "grunt", Npc::new("walker", "eyes"), Vec3::new(4.0, 0.0, 2.0), E);
    world.entity_mut(wide).insert((Patrol { points: vec![Vec3::new(20.0, 0.0, 2.0)] }, CharacterParams { radius: 1.6, height: 3.2, ..default() }));
    step(&mut app, 2);
    for e in [big, wide] {
        let me = id(&app, e);
        let state = ok(&mut app, "npc_state", json!({"id": me}));
        assert_eq!(state.result["nav"]["routes"], "straight", "{}", state.result);
        assert!(!state.result["nav"]["arrived"].as_bool().unwrap(), "on its way: {}", state.result);
        let line = overlay_line(&mut app, &me);
        assert!(!line.contains(" · body ") && !line.contains("local grid"), "the game's provider plans its routes: {line}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_body_class_bakes_to_stable_bytes_and_content_checks_each() {
    let dir = assets_with("bake", LARGE);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    // No bake files: each tile is missing once per class, the class named.
    let missing = errors(&app);
    assert_eq!(missing.iter().filter(|e| e.contains("has no bake file")).count(), 2 * 8, "{missing:#?}");
    assert!(missing.iter().any(|e| e.contains("tile `pillar` has no bake file (nav/pillar.navtile.ron)")), "{missing:#?}");
    assert!(
        missing.iter().any(|e| e.contains(
            "tile `pillar` has no bake file for the body `large` (nav/large/pillar.navtile.ron); bake it with `--bake-nav pillar`"
        )),
        "{missing:#?}"
    );

    let out = text(&ok(&mut app, "nav_bake", json!({})));
    assert!(out.starts_with("baked 16 files for 2 body classes (agent, large) into nav/"), "{out}");
    assert!(out.contains("\n  gates: large/gates.navtile.ron ("), "{out}");
    assert!(text(&ok(&mut app, "content_check", json!({}))).starts_with("ok: "), "fresh bakes of both classes are clean content");
    // The same bytes when baked again, and the agent's are the very bytes a game without
    // classes bakes.
    let large = std::fs::read_to_string(dir.join("nav/large/gates.navtile.ron")).unwrap();
    let agent = std::fs::read_to_string(dir.join("nav/pillar.navtile.ron")).unwrap();
    ok(&mut app, "nav_bake", json!({"tile": "gates,pillar"}));
    assert_eq!(std::fs::read_to_string(dir.join("nav/large/gates.navtile.ron")).unwrap(), large);
    assert_eq!(std::fs::read_to_string(dir.join("nav/pillar.navtile.ron")).unwrap(), agent);
    let plain = crate::common::app();
    let settings = NavSettings::of(plain.world().resource::<ContentDb>());
    let alone = bake(plain.world(), None, TileKind::Tile, "pillar", 8.0, &settings).unwrap();
    assert_eq!(alone.to_file(), agent, "the agent's bakes do not change with the classes");
    // The large class sees less room: fewer nodes, its own hash.
    let large_pillar = std::fs::read_to_string(dir.join("nav/large/pillar.navtile.ron")).unwrap();
    assert_ne!(large_pillar, agent);

    // A class's file gone: content names it, and only it.
    std::fs::remove_file(dir.join("nav/large/pillar.navtile.ron")).unwrap();
    let check = text(&ok(&mut app, "content_check", json!({})));
    assert!(check.contains("tile `pillar` has no bake file for the body `large` (nav/large/pillar.navtile.ron)"), "{check}");
    assert!(!check.contains("(nav/pillar.navtile.ron)"), "the agent's is there: {check}");
    // A region baked while loading says which class.
    load_region(&mut app, &["floor pillar floor"]);
    let warned: Vec<String> = all_journal(&app).into_iter().filter(|l| l.contains("[warn]")).collect();
    assert!(
        warned.iter().any(|w| w.contains("navmesh (body large): 1 tile baked while loading (no bake file: pillar)")),
        "{warned:#?}"
    );
    assert!(!warned.iter().any(|w| w.contains("navmesh: ")), "the agent's bakes were all there: {warned:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn body_classes_are_checked_like_any_content() {
    // The agent is the code defaults: 0.4 m radius, 1.8 m tall, skin 0.02 m.
    let nav = "{ \"default\": (cell: 0.5, check_bakes: false, bodies: {\
               \"agent\": (), \"Big\": (), \"thin\": (radius: Some(-1.0)), \"ogre\": (locomotion: Some(\"ogre\")), \
               \"wide\": (radius: Some(1.2)), \"short\": (height: Some(0.5)), \"wisp\": (radius: Some(0.03)), \
               \"tall\": (radius: Some(1.2), height: Some(2.4)) }) }\n";
    let dir = assets_with("checked", nav);
    let app = app_in(dir.to_str().unwrap(), 7, |_| {});
    let errors = errors(&app);
    for needle in [
        "bodies: `agent` is the body `agent` names",
        "bodies: `Big` must be lowercase letters, digits and `_`",
        "bodies.thin.radius -1 must be more than 0 m",
        "bodies.ogre.locomotion",
        // A size of its own still makes a capsule the locomotion would allow.
        "bodies.wide: height 1.8 is less than the capsule's diameter 2.4; give the class a height of at least 2.4 m or a \
         smaller radius",
        "bodies.short: height 0.5 is less than the capsule's diameter 0.8",
        "bodies.wisp: skin_width 0.02 is too large for radius 0.03; give the class a radius above 0.04 m",
    ] {
        assert!(errors.iter().any(|e| e.contains(needle)), "{needle}: {errors:#?}");
    }
    // Said once, about the class that is wrong.
    assert!(!errors.iter().any(|e| e.contains("bodies.tall") || e.contains("bodies.thin:")), "{errors:#?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_classes_of_a_region_are_its_navmeshes_by_name() {
    let dir = assets_with("classes", LARGE);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    load_region(&mut app, &["floor gates floor"]);
    let nav = app.world().resource::<NavRegion>();
    assert_eq!(nav.body, "agent");
    assert_eq!(nav.classes().map(|c| c.body.as_str()).collect::<Vec<_>>(), ["agent", "large"]);
    assert_eq!(nav.for_radius(0.4).map(|c| c.body.as_str()), Some("agent"));
    assert_eq!(nav.for_radius(0.45).map(|c| c.body.as_str()), Some("large"));
    assert!(nav.for_radius(1.3).is_none());
    let large = nav.class("large").unwrap();
    assert!(large.node_count() < nav.node_count(), "less room for the large body");
    // Its own query refuses a body wider than it, as the agent's does.
    let physics = app.world().resource::<PhysicsWorld>();
    let query = NavQuery { from: Vec3::new(4.0, 0.0, 3.0), to: Vec3::new(20.0, 0.0, 3.0), goal_level: false, radius: 1.3, height: 2.4, step_height: 0.35 };
    assert!(large.path(physics, &query).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A body as wide as `agent` and taller than the arch's lintel (2.0 m): `agent` is the class
/// its radius alone would pick.
fn tall() -> CharacterParams {
    CharacterParams { height: 2.4, ..default() }
}

/// Steps `ticks` ticks: the z where `e` last crossed the wall's line (x = 12) going east.
fn crossing(app: &mut App, e: Entity, ticks: u32) -> Option<f32> {
    let mut crossed = None;
    for _ in 0..ticks {
        let before = feet(app, e);
        step(app, 1);
        let after = feet(app, e);
        if before.x < 12.0 && after.x >= 12.0 {
            crossed = Some(after.z);
        }
    }
    crossed
}

/// The slot (z 1.25..2.75) and the door (z 4.25..7.75) of the wall at x = 12.
const SLOT: std::ops::Range<f32> = 1.25..2.75;
const DOOR: std::ops::Range<f32> = 4.25..7.75;

#[test]
fn a_tall_npc_routes_on_a_class_as_tall_and_never_under_a_lintel_it_does_not_clear() {
    let dir = assets_with_tiles("tall", TALL, &[GATES, ARCH]);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    load_region(&mut app, &["floor arch floor"]);
    let (start, goal) = (Vec3::new(4.0, 0.0, 3.0), Vec3::new(20.0, 0.0, 3.0));
    // The agent's body walks under the lintel, the short way.
    let short = navmesh_path(app.world(), start, goal, &CharacterParams::default()).unwrap();
    let through = crossings(start, &short);
    assert!(through.len() == 1 && SLOT.contains(&through[0]), "under the lintel: {through:?} {short:?}");
    assert_eq!(routes_for(app.world(), &CharacterParams::default()), "navmesh, body agent");
    // As wide and taller: on `tall`, by the door, though `agent` is as wide.
    let long = navmesh_path(app.world(), start, goal, &tall()).unwrap();
    let through = crossings(start, &long);
    assert!(through.len() == 1 && DOOR.contains(&through[0]), "by the door: {through:?} {long:?}");
    assert_eq!(routes_for(app.world(), &tall()), "navmesh, body tall");
    // A line under the lintel is a straight walk only for the body that clears it.
    let (a, b) = (Vec3::new(4.0, 0.0, 2.25), Vec3::new(20.0, 0.0, 2.25));
    assert!(plain_sight(app.world(), a, b, &CharacterParams::default()), "the agent's body walks it");
    assert!(!plain_sight(app.world(), a, b, &tall()), "the tall body does not");

    // An NPC of that body: `npc_state` and `nav_path` name its class, and it walks by the door.
    let world = app.world_mut();
    let walker = npc(world, "grunt", Npc::new("walker", "eyes"), start, E);
    world.entity_mut(walker).insert((Patrol { points: vec![goal] }, tall()));
    step(&mut app, 2);
    let me = id(&app, walker);
    let state = ok(&mut app, "npc_state", json!({"id": me}));
    assert_eq!(state.result["nav"]["routes"], "navmesh, body tall", "{}", state.result);
    let preview = ok(&mut app, "nav_path", json!({"from": me, "to": {"x": 20.0, "z": 3.0}}));
    assert_eq!(preview.result["body"], "tall", "{}", preview.result);
    let through = crossings(feet(&app, walker), &points(&preview.result));
    assert!(through.len() == 1 && DOOR.contains(&through[0]), "{}", preview.result);
    let crossed = crossing(&mut app, walker, 900).expect("it crossed the wall's line");
    assert!(DOOR.contains(&crossed), "by the door: at z {crossed:.2}");
    assert!(flat_distance(feet(&app, walker), goal) < 0.6, "arrived: {}", feet(&app, walker));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tall_npc_no_class_serves_plans_on_the_local_grid_with_its_whole_body() {
    let dir = assets_with_tiles("untall", "{ \"default\": (cell: 0.5) }\n", &[GATES, ARCH]);
    let mut app = app_in(dir.to_str().unwrap(), 7, |_| {});
    load_region(&mut app, &["floor arch floor"]);
    let (start, goal) = (Vec3::new(4.0, 0.0, 3.0), Vec3::new(20.0, 0.0, 3.0));
    // `agent` is as wide, but lower: the body is not walked as `agent` under the lintel.
    let why = "local grid: no body class serves the body: agent has height 1.80 m, the body 2.40 m";
    assert_eq!(routes_for(app.world(), &tall()), why);
    let route = navmesh_path(app.world(), start, goal, &tall()).unwrap();
    let through = crossings(start, &route);
    assert!(through.len() == 1 && DOOR.contains(&through[0]), "the local grid knows its height: {through:?} {route:?}");
    // The straight walk under the lintel: the agent's on the navmesh, not the tall body's cast.
    let (a, b) = (Vec3::new(4.0, 0.0, 2.25), Vec3::new(20.0, 0.0, 2.25));
    assert!(plain_sight(app.world(), a, b, &CharacterParams::default()), "the agent's body walks it");
    assert!(!plain_sight(app.world(), a, b, &tall()), "the tall body's cast meets the lintel");

    let world = app.world_mut();
    let walker = npc(world, "grunt", Npc::new("walker", "eyes"), start, E);
    world.entity_mut(walker).insert((Patrol { points: vec![goal] }, tall()));
    step(&mut app, 2);
    let me = id(&app, walker);
    let state = ok(&mut app, "npc_state", json!({"id": me}));
    assert_eq!(state.result["nav"]["routes"], why, "{}", state.result);
    assert!(overlay_line(&mut app, &me).ends_with(" · local grid"));
    let preview = ok(&mut app, "nav_path", json!({"from": me, "to": {"x": 20.0, "z": 3.0}}));
    assert_eq!(preview.result["provider"], "local grid", "{}", preview.result);
    let through = crossings(feet(&app, walker), &points(&preview.result));
    assert!(through.len() == 1 && DOOR.contains(&through[0]), "{}", preview.result);
    let crossed = crossing(&mut app, walker, 900).expect("it crossed the wall's line");
    assert!(DOOR.contains(&crossed), "by the door: at z {crossed:.2}");
    assert!(flat_distance(feet(&app, walker), goal) < 0.6, "arrived: {}", feet(&app, walker));
    let _ = std::fs::remove_dir_all(&dir);
}
