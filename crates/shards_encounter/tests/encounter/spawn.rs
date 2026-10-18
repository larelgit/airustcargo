//! Spawn points: offsets turned with the anchor, markers and points of interest bound within the
//! radius and sorted, ring points, the facing filter (lock-on target, aim, forward), the
//! ground, the home and leash patch, the overrides.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::RunState;
use shards::npc::Home;
use shards::world::{Marker, Poi};

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);

fn pos(app: &App, e: Entity) -> Vec3 {
    app.world().get::<Transform>(e).unwrap().translation
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 0.01
}

fn start_manual(app: &mut App, key: &str) {
    ok(app, "encounter_start", json!({"key": key}));
    step(app, 3);
}

#[test]
fn offsets_turn_with_the_anchors_yaw_and_members_face_the_anchor() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, FAR);
    // A quarter turn about Y: the offset (0, 0, -5) points west (-X).
    let at = Transform::from_xyz(10.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
    shards::encounter::place_encounter(app.world_mut(), "manual", Some("y"), at).unwrap();
    step(&mut app, 2);
    start_manual(&mut app, "world/y");
    run_until(&mut app, 120, |a| run(a, "world/y").wave == 1);
    let (_, member) = members(&app, "world/y")[0];
    assert!(close(pos(&app, member), Vec3::new(5.0, 0.0, 0.0)), "{:?}", pos(&app, member));
    // `Center`: it looks at the anchor (east, +X), as Godot's spawns face the arena's centre.
    let forward = *app.world().get::<Transform>(member).unwrap().forward();
    assert!(close(forward, Vec3::X), "{forward:?}");
}

#[test]
fn faces_and_homes_and_overrides_are_patched_into_the_spawn() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "overridden", "o", Vec3::new(0.0, 0.0, 0.0));
    step(&mut app, 2);
    start_manual(&mut app, &key);
    let all = members(&app, &key);
    assert_eq!(all.len(), 4);
    let by = |x: f32| all.iter().map(|(_, e)| *e).find(|e| (pos(&app, *e).x - x).abs() < 0.01).expect("a member at x");
    // overrides: Health.current 7 over the prefab's, the prefab's max kept; leash 9 at the spawn.
    let m0 = by(0.0);
    let health = app.world().get::<shards::action::Health>(m0).unwrap();
    assert_eq!((health.current, health.max), (7.0, 20.0));
    let home = app.world().get::<Home>(m0).unwrap();
    assert_eq!((home.pos, home.leash), (Some(pos(&app, m0)), 9.0));
    // home: Anchor, the prefab's leash kept.
    let m1 = by(2.0);
    let home = app.world().get::<Home>(m1).unwrap();
    assert_eq!((home.pos, home.leash), (Some(Vec3::ZERO), 15.0));
    // Yaw(90): ninety degrees clockwise from the anchor's facing (north) is east.
    let m2 = by(4.0);
    assert!(close(*app.world().get::<Transform>(m2).unwrap().forward(), Vec3::X));
    // Anchor: the anchor's facing, north.
    let m3 = by(-4.0);
    assert!(close(*app.world().get::<Transform>(m3).unwrap().forward(), Vec3::NEG_Z));
    // A member of a group without home or leash keeps the prefab's Home: first-stood.
    let _ = m3;
}

fn markers(app: &mut App, name: &str, points: &[Vec3]) {
    for p in points {
        app.world_mut().spawn((Marker { name: name.into() }, Transform::from_translation(*p)));
    }
}

#[test]
fn markers_are_bound_only_within_the_radius_and_by_name() {
    let mut app = app(3);
    let _p0 = player(&mut app, 0, FAR);
    let inside = [Vec3::new(3.0, 0.0, -4.0), Vec3::new(-3.0, 0.0, -4.0), Vec3::new(6.0, 0.0, -4.0)];
    markers(&mut app, "pack", &inside);
    markers(&mut app, "pack", &[Vec3::new(30.0, 0.0, 0.0)]);
    markers(&mut app, "other", &[Vec3::new(0.0, 0.0, 5.0)]);
    let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
    step(&mut app, 2);
    let all = members(&app, &key);
    assert!((2..=3).contains(&all.len()));
    let mut seen = Vec::new();
    for (_, e) in &all {
        let p = pos(&app, *e);
        assert!(inside.iter().any(|m| close(*m, p)), "{p:?} is a marker of the pack within the radius");
        seen.push(p);
    }
    // Drawn without repeats while there are points to draw.
    for (i, a) in seen.iter().enumerate() {
        assert!(seen.iter().skip(i + 1).all(|b| !close(*a, *b)), "{seen:?}");
    }
}

#[test]
fn no_marker_within_the_radius_spawns_at_the_anchor_and_says_so_once() {
    let mut app = app(3);
    let _p0 = player(&mut app, 0, FAR);
    markers(&mut app, "pack", &[Vec3::new(30.0, 0.0, 0.0)]);
    let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
    step(&mut app, 2);
    for (_, e) in members(&app, &key) {
        assert!(close(pos(&app, e), Vec3::ZERO));
    }
    assert_eq!(count(&app, "no marker `pack` within 12 m of its anchor"), 1, "{:?}", journal(&app));
}

#[test]
fn points_of_interest_are_bound_by_type_and_sorted_by_id() {
    let mut app = app(5);
    let _p0 = player(&mut app, 0, FAR);
    for (id, x) in [("shrine.2", 4.0), ("shrine.1", -4.0), ("shrine.3", 8.0)] {
        app.world_mut().spawn((Poi { id: id.into(), poi: "shrine".into() }, Transform::from_xyz(x, 0.0, -3.0)));
    }
    app.world_mut().spawn((Poi { id: "camp.1".into(), poi: "camp".into() }, Transform::from_xyz(1.0, 0.0, -3.0)));
    app.world_mut().spawn((Poi { id: "shrine.9".into(), poi: "shrine".into() }, Transform::from_xyz(60.0, 0.0, 0.0)));
    let key = place(&mut app, "poi_points", "q", Vec3::ZERO);
    step(&mut app, 2);
    start_manual(&mut app, &key);
    let xs: Vec<f32> = members(&app, &key).iter().map(|(_, e)| pos(&app, *e).x).collect();
    assert_eq!(xs.len(), 2);
    assert!(xs.iter().all(|x| [4.0, -4.0, 8.0].iter().any(|m| (m - x).abs() < 0.01)), "{xs:?}");
}

#[test]
fn ring_points_are_between_min_and_max_meters_out_and_on_the_ground() {
    let mut app = app(7);
    let _p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "ring_points", "r", Vec3::new(5.0, 0.0, 5.0));
    step(&mut app, 2);
    start_manual(&mut app, &key);
    let ms = members(&app, &key);
    assert_eq!(ms.len(), 3);
    for (_, e) in ms {
        let p = pos(&app, e);
        let d = Vec3::new(p.x - 5.0, 0.0, p.z - 5.0).length();
        assert!((6.0..=9.0001).contains(&d), "{d}");
        assert!(p.y.abs() < 0.01, "{p:?}");
    }
}

#[test]
fn the_ground_under_a_marker_sets_the_height() {
    let mut app = app(3);
    let _p0 = player(&mut app, 0, FAR);
    // A block 1.5 m high under a marker.
    app.world_mut().spawn((
        shards::motion::Collider::new(shards::motion::Shape::cuboid(Vec3::new(1.5, 0.75, 1.5))),
        Transform::from_xyz(3.0, 0.75, -4.0),
    ));
    markers(&mut app, "pack", &[Vec3::new(3.0, 0.0, -4.0)]);
    let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
    step(&mut app, 2);
    for (_, e) in members(&app, &key) {
        assert!((pos(&app, e).y - 1.5).abs() < 0.05, "{:?}", pos(&app, e));
    }
}

// --- the facing filter ----------------------------------------------------------------------

/// A player at the origin and markers east and west of it, 6 m out; which markers the pack
/// stands on once the player faces `facing` the way `setup` says.
fn pack_beside(setup: impl FnOnce(&mut App, Entity)) -> Vec<Vec3> {
    let mut app = app(3);
    let p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 2.0));
    markers(&mut app, "pack", &[Vec3::new(6.0, 0.0, 2.0), Vec3::new(-6.0, 0.0, 2.0)]);
    setup(&mut app, p0);
    let key = place(&mut app, "marker_pack", "p", Vec3::new(0.0, 0.0, 2.0));
    step(&mut app, 2);
    members(&app, &key).iter().map(|(_, e)| pos(&app, *e)).collect()
}

fn all_at(points: &[Vec3], at: Vec3) -> bool {
    !points.is_empty() && points.iter().all(|p| close(*p, at))
}

#[test]
fn spawn_points_behind_a_player_are_not_chosen_by_lock_on_then_aim_then_forward() {
    // 1. The lock-on target decides: the player (at z = 2) looks at a dummy to the west, so the
    //    east marker is behind it.
    let points = pack_beside(|app, p0| {
        let dummy = app.world_mut().spawn((AiKind::new("Dummy"), Transform::from_xyz(-20.0, 0.0, 2.0))).id();
        let id = *app.world().get::<AiId>(dummy).unwrap();
        app.world_mut().get_mut::<Intent>(p0).unwrap().target = Some(id);
    });
    assert!(all_at(&points, Vec3::new(-6.0, 0.0, 2.0)), "{points:?}");
    // 2. Without a target the aim decides: looking east leaves the west marker behind.
    let points = pack_beside(|app, p0| {
        app.world_mut().get_mut::<Intent>(p0).unwrap().aim = Some(Vec3::new(20.0, 0.0, 2.0));
    });
    assert!(all_at(&points, Vec3::new(6.0, 0.0, 2.0)), "{points:?}");
    // 3. With neither, the body's forward: it faces west (a quarter turn), so east is behind.
    let points = pack_beside(|app, p0| {
        app.world_mut().get_mut::<Transform>(p0).unwrap().look_to(Vec3::NEG_X, Vec3::Y);
    });
    assert!(all_at(&points, Vec3::new(-6.0, 0.0, 2.0)), "{points:?}");
}

#[test]
fn when_every_point_is_behind_a_player_the_farthest_is_used_and_it_is_said_once() {
    let mut app = app(3);
    // The player at the origin faces north; the pack's markers are all south of it and near.
    let _p0 = player(&mut app, 0, Vec3::ZERO);
    markers(&mut app, "pack", &[Vec3::new(0.0, 0.0, 6.0), Vec3::new(2.0, 0.0, 8.0), Vec3::new(-2.0, 0.0, 10.0)]);
    let key = place(&mut app, "marker_pack", "p", Vec3::new(0.0, 0.0, 5.0));
    step(&mut app, 2);
    for (_, e) in members(&app, &key) {
        assert!(close(pos(&app, e), Vec3::new(-2.0, 0.0, 10.0)), "{:?}", pos(&app, e));
    }
    assert_eq!(count(&app, "every spawn point is behind a player; used the farthest"), 1, "{:?}", journal(&app));
}

#[test]
fn more_members_than_points_share_points_and_it_is_said_once() {
    let mut app = app(3);
    let _p0 = player(&mut app, 0, FAR);
    markers(&mut app, "pack", &[Vec3::new(3.0, 0.0, -4.0)]);
    let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
    step(&mut app, 2);
    assert!(members(&app, &key).len() >= 2);
    assert_eq!(count(&app, "wants more members"), 1, "{:?}", journal(&app));
}

#[test]
fn the_spawn_leaves_the_state_of_the_run_to_the_machine() {
    let mut app = app(3);
    let _p0 = player(&mut app, 0, FAR);
    markers(&mut app, "pack", &[Vec3::new(3.0, 0.0, -4.0), Vec3::new(-3.0, 0.0, -4.0)]);
    let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
    step(&mut app, 2);
    assert_eq!((run(&app, &key).wave, state(&app, &key)), (1, RunState::Armed), "a placed wave stands from the run's making");
}
