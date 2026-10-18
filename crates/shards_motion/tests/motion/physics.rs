//! Physics layer: queries against every shape, layers, ordering, volumes, cache upkeep.

use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

fn physics(app: &App) -> &PhysicsWorld {
    app.world().resource::<PhysicsWorld>()
}

fn spawn(app: &mut App, shape: Shape, at: Vec3) -> Entity {
    app.world_mut().spawn((Collider::new(shape), Transform::from_translation(at))).id()
}

/// Lets the physics world catch up (the frame-end sync, no tick).
fn sync(app: &mut App) {
    app.update();
}

const DOWN: Dir3 = Dir3::NEG_Y;
const ANY: fn() -> QueryFilter = QueryFilter::default;

#[test]
fn raycast_hits_every_shape_on_top() {
    let mut app = app();
    let shapes = [
        (Shape::cuboid(Vec3::ONE), 1.0),
        (Shape::sphere(1.0), 1.0),
        (Shape::Capsule { half_height: 0.5, radius: 0.5 }, 1.0),
        (Shape::Cylinder { half_height: 1.0, radius: 0.5 }, 1.0),
        // Half way out on the cone's radius the surface is at mid height.
        (Shape::Cone { half_height: 1.0, radius: 1.0 }, 0.0),
    ];
    let mut entities = Vec::new();
    for (i, (shape, _)) in shapes.iter().enumerate() {
        entities.push(spawn(&mut app, *shape, Vec3::new(i as f32 * 5.0, 0.0, 0.0)));
    }
    sync(&mut app);
    for (i, (shape, top)) in shapes.iter().enumerate() {
        let x = i as f32 * 5.0 + if matches!(shape, Shape::Cone { .. }) { 0.5 } else { 0.0 };
        let hit = physics(&app).raycast(Vec3::new(x, 5.0, 0.0), DOWN, 10.0, &ANY()).unwrap_or_else(|| panic!("{shape} missed"));
        assert_eq!(hit.entity, entities[i], "{shape}");
        assert!((hit.distance - (5.0 - top)).abs() < 1e-3, "{shape}: {hit:?}");
        assert!((hit.point.y - top).abs() < 1e-3, "{shape}: {hit:?}");
        assert!(hit.normal.y > 0.4, "{shape}: normal {}", hit.normal);
    }
    // Too short, or beside everything: nothing.
    assert!(physics(&app).raycast(Vec3::new(0.0, 5.0, 0.0), DOWN, 3.0, &ANY()).is_none());
    assert!(physics(&app).raycast(Vec3::new(2.5, 5.0, 0.0), DOWN, 10.0, &ANY()).is_none());
}

#[test]
fn raycast_respects_rotation_and_offset() {
    let mut app = app();
    // A 4 m tall box standing on its origin, turned 90° about Z: now 4 m wide, 1 m tall.
    let rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let e = app
        .world_mut()
        .spawn((Collider::new(Shape::cuboid(Vec3::new(0.5, 2.0, 0.5))).with_offset(Vec3::Y * 2.0), Transform::from_rotation(rot)))
        .id();
    sync(&mut app);
    // Offset (0, 2, 0) turned about Z points to -X: center at (-2, 0, 0).
    let hit = physics(&app).raycast(Vec3::new(-3.5, 5.0, 0.0), DOWN, 10.0, &ANY()).unwrap();
    assert_eq!(hit.entity, e);
    assert!((hit.point.y - 0.5).abs() < 1e-3, "{hit:?}");
    assert!(physics(&app).raycast(Vec3::new(0.5, 5.0, 0.0), DOWN, 10.0, &ANY()).is_none());
}

#[test]
fn shapecast_reports_time_of_impact_and_normal() {
    let mut app = app();
    let wall = spawn(&mut app, Shape::cuboid(Vec3::ONE), Vec3::ZERO);
    sync(&mut app);
    let ball = Shape::sphere(0.5);
    let hit = physics(&app).shapecast(&ball, Vec3::new(-5.0, 0.0, 0.0), Quat::IDENTITY, Dir3::X, 10.0, &ANY()).unwrap();
    assert_eq!(hit.entity, wall);
    assert!((hit.distance - 3.5).abs() < 1e-3, "{hit:?}");
    assert!((hit.normal - Vec3::NEG_X).length() < 1e-3, "{hit:?}");
    assert!((hit.point.x + 1.0).abs() < 1e-3, "{hit:?}");

    // A capsule dropped onto the box lands on its top.
    let capsule = Shape::capsule(0.4, 1.8);
    let hit = physics(&app).shapecast(&capsule, Vec3::new(0.3, 5.0, 0.0), Quat::IDENTITY, DOWN, 10.0, &ANY()).unwrap();
    assert!((hit.distance - (5.0 - 0.9 - 1.0)).abs() < 1e-3, "{hit:?}");
    assert!(hit.normal.y > 0.99, "{hit:?}");

    // Out of reach: nothing. Starting inside: distance 0.
    assert!(physics(&app).shapecast(&ball, Vec3::new(-5.0, 0.0, 0.0), Quat::IDENTITY, Dir3::X, 3.0, &ANY()).is_none());
    let inside = physics(&app).shapecast(&ball, Vec3::new(-0.8, 0.0, 0.0), Quat::IDENTITY, Dir3::NEG_X, 3.0, &ANY());
    assert_eq!(inside.map(|h| h.distance), Some(0.0));
}

#[test]
fn shapecast_every_shape_against_every_shape() {
    let shapes = [
        Shape::cuboid(Vec3::splat(0.5)),
        Shape::sphere(0.5),
        Shape::Capsule { half_height: 0.5, radius: 0.5 },
        Shape::Cylinder { half_height: 0.5, radius: 0.5 },
        Shape::Cone { half_height: 0.5, radius: 0.5 },
    ];
    for target in shapes {
        let mut app = app();
        let e = spawn(&mut app, target, Vec3::ZERO);
        sync(&mut app);
        for moving in shapes {
            let hit = physics(&app)
                .shapecast(&moving, Vec3::new(-5.0, 0.0, 0.0), Quat::IDENTITY, Dir3::X, 10.0, &ANY())
                .unwrap_or_else(|| panic!("{moving} missed {target}"));
            assert_eq!(hit.entity, e);
            // Both reach at most 0.5 toward each other; at the time of impact they touch.
            assert!((4.0 - 2e-3..=5.0).contains(&hit.distance), "{moving} → {target}: {hit:?}");
            let at = |d: f32| physics(&app).overlaps(&moving, Vec3::new(-5.0 + d, 0.0, 0.0), Quat::IDENTITY, &ANY());
            assert!(at(hit.distance - 0.01).is_empty(), "{moving} → {target}: apart before impact");
            assert_eq!(at(hit.distance + 0.01), vec![e], "{moving} → {target}: touching after impact");
        }
    }
}

#[test]
fn layers_exclusions_and_volumes_filter_queries() {
    let mut app = app();
    let world_box = spawn(&mut app, Shape::cuboid(Vec3::ONE), Vec3::new(0.0, 0.0, 0.0));
    let game_box = app
        .world_mut()
        .spawn((
            Collider::new(Shape::cuboid(Vec3::ONE)).with_layers(CollisionLayers::new(LayerMask::game(0), LayerMask::ALL)),
            Transform::from_xyz(0.0, 3.0, 0.0),
        ))
        .id();
    let water = volume(app.world_mut(), Vec3::new(-1.0, 5.0, -1.0), Vec3::new(1.0, 7.0, 1.0), VolumeKind::Water);
    sync(&mut app);
    let from = Vec3::new(0.0, 10.0, 0.0);
    let first = |filter: QueryFilter| physics(&app).raycast(from, DOWN, 20.0, &filter).map(|h| h.entity);

    assert_eq!(first(ANY()), Some(game_box), "volumes are skipped by default");
    assert_eq!(first(ANY().with_volumes()), Some(water));
    assert_eq!(first(QueryFilter::new(LayerMask::WORLD)), Some(world_box));
    assert_eq!(first(QueryFilter::new(LayerMask::game(0))), Some(game_box));
    assert_eq!(first(ANY().excluding(game_box)), Some(world_box));
    assert_eq!(first(QueryFilter::new(LayerMask::CHARACTER)), None);
}

#[test]
fn ids_follow_insertion_and_order_every_result() {
    let mut app = app();
    let world = app.world_mut();
    // Entities first, colliders later and in the other order: ids follow the
    // colliders, not the entity indices.
    let a = world.spawn(Transform::default()).id();
    world.spawn_empty(); // an unrelated entity in between
    let b = world.spawn(Transform::default()).id();
    world.entity_mut(b).insert(Collider::new(Shape::cuboid(Vec3::ONE)));
    world.entity_mut(a).insert(Collider::new(Shape::cuboid(Vec3::ONE)));
    let (id_a, id_b) = (*world.get::<ColliderId>(a).unwrap(), *world.get::<ColliderId>(b).unwrap());
    assert!(id_b < id_a, "b got its collider first: {id_b} < {id_a}");
    sync(&mut app);

    // Two identical boxes: the tie goes to the lower id, in every query.
    let p = physics(&app);
    assert_eq!(p.raycast(Vec3::new(0.0, 5.0, 0.0), DOWN, 10.0, &ANY()).unwrap().entity, b);
    let ball = Shape::sphere(0.5);
    assert_eq!(p.shapecast(&ball, Vec3::new(0.0, 5.0, 0.0), Quat::IDENTITY, DOWN, 10.0, &ANY()).unwrap().entity, b);
    assert_eq!(p.overlaps(&ball, Vec3::ZERO, Quat::IDENTITY, &ANY()), vec![b, a]);
    assert_eq!(p.colliders().map(|c| c.entity).collect::<Vec<_>>(), vec![b, a]);

    // Ids are never reused.
    app.world_mut().despawn(b);
    let c = spawn(&mut app, Shape::sphere(1.0), Vec3::ZERO);
    let id_c = *app.world().get::<ColliderId>(c).unwrap();
    assert!(id_c > id_a, "{id_c} is new");
    sync(&mut app);
    assert_eq!(physics(&app).overlaps(&ball, Vec3::ZERO, Quat::IDENTITY, &ANY()), vec![a, c]);
}

#[test]
fn volumes_at_finds_containing_volumes_in_order() {
    let mut app = app();
    let pool = volume(app.world_mut(), Vec3::new(0.0, -2.0, 0.0), Vec3::new(10.0, 0.0, 10.0), VolumeKind::Water);
    let goal = volume(
        app.world_mut(),
        Vec3::new(4.0, -1.0, 4.0),
        Vec3::new(6.0, 3.0, 6.0),
        VolumeKind::Trigger { name: "goal".into() },
    );
    spawn(&mut app, Shape::cuboid(Vec3::ONE), Vec3::new(5.0, -1.0, 5.0)); // solid, not a volume
    sync(&mut app);
    let at = |p: Vec3| physics(&app).volumes_at(p, None);
    assert_eq!(at(Vec3::new(1.0, -1.0, 1.0)), vec![(pool, VolumeKind::Water)]);
    assert_eq!(
        at(Vec3::new(5.0, -0.5, 5.0)),
        vec![(pool, VolumeKind::Water), (goal, VolumeKind::Trigger { name: "goal".into() })]
    );
    assert_eq!(at(Vec3::new(5.0, 2.0, 5.0)).len(), 1);
    assert!(at(Vec3::new(20.0, -1.0, 1.0)).is_empty());
    let only_triggers = QueryFilter::new(LayerMask::VOLUME).excluding(pool);
    assert_eq!(physics(&app).volumes_at(Vec3::new(5.0, -0.5, 5.0), Some(&only_triggers)).len(), 1);
}

#[test]
fn static_cache_follows_moves_and_removals() {
    let mut app = app();
    let e = spawn(&mut app, Shape::cuboid(Vec3::ONE), Vec3::ZERO);
    step(&mut app, 1);
    let top = |app: &App| physics(app).raycast(Vec3::new(0.0, 10.0, 0.0), DOWN, 20.0, &ANY()).map(|h| h.point.y);
    assert!((top(&app).unwrap() - 1.0).abs() < 1e-3);

    // Moved between ticks: the next tick sees it (and so does the frame-end sync).
    app.world_mut().get_mut::<Transform>(e).unwrap().translation.y = 3.0;
    step(&mut app, 1);
    assert!((top(&app).unwrap() - 4.0).abs() < 1e-3);

    // Reshaped.
    app.world_mut().get_mut::<Collider>(e).unwrap().shape = Shape::cuboid(Vec3::new(1.0, 2.0, 1.0));
    step(&mut app, 1);
    assert!((top(&app).unwrap() - 5.0).abs() < 1e-3);

    // Removed while the game sits paused for many frames: gone at once, not after
    // a removal event that may have expired.
    app.world_mut().entity_mut(e).remove::<Collider>();
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(top(&app), None);
    assert!(physics(&app).is_empty());

    let e2 = spawn(&mut app, Shape::sphere(1.0), Vec3::ZERO);
    step(&mut app, 1);
    assert!(top(&app).is_some());
    app.world_mut().despawn(e2);
    assert_eq!(top(&app), None);
}

#[test]
fn removing_a_volume_makes_the_collider_solid() {
    let mut app = app();
    let e = volume(app.world_mut(), Vec3::splat(-1.0), Vec3::ONE, VolumeKind::Water);
    sync(&mut app);
    let hit = |app: &App| physics(app).raycast(Vec3::new(0.0, 5.0, 0.0), DOWN, 10.0, &QueryFilter::default()).is_some();
    assert!(!hit(&app));
    app.world_mut().entity_mut(e).remove::<Volume>();
    assert!(hit(&app), "a plain collider now");
    app.world_mut().entity_mut(e).insert(Volume { kind: VolumeKind::Water });
    sync(&mut app);
    assert!(!hit(&app));
}

#[test]
#[expect(clippy::disallowed_methods, reason = "tests what the world keeps for a collider")]
fn kinematic_bodies_refresh_every_tick() {
    let mut app = app();
    let mover = app
        .world_mut()
        .spawn((Collider::new(Shape::sphere(0.5)), Body::Kinematic, Transform::from_xyz(0.0, 0.0, 0.0)))
        .id();
    step(&mut app, 1);
    for x in 1..4 {
        app.world_mut().get_mut::<Transform>(mover).unwrap().translation.x = x as f32;
        step(&mut app, 1);
        let c = physics(&app).collider(mover).unwrap();
        assert_eq!(c.position, Vec3::new(x as f32, 0.0, 0.0));
        assert_eq!(c.body, Body::Kinematic);
    }
}

#[test]
#[expect(clippy::disallowed_methods, reason = "tests what the world keeps for a collider")]
fn a_collider_without_a_real_transform_is_left_out_until_it_has_one() {
    // A NaN pose would make every ray in the world "hit" it at distance 0.
    let mut app = app();
    let ground = floor(app.world_mut());
    let nan = Vec3::splat(f32::NAN);
    let shapes = [
        Shape::cuboid(Vec3::ONE),
        Shape::Cylinder { half_height: 1.0, radius: 0.5 },
        Shape::sphere(1.0),
        Shape::capsule(0.5, 2.0),
    ];
    let mut broken: Vec<Entity> = shapes.iter().map(|s| spawn(&mut app, *s, nan)).collect();
    let turned = Transform::from_rotation(Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0));
    broken.push(app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::ONE)), turned)).id());
    let off = Collider::new(Shape::cuboid(Vec3::ONE)).with_offset(Vec3::splat(f32::INFINITY));
    broken.push(app.world_mut().spawn((off, Transform::default())).id());
    sync(&mut app);
    let from = Vec3::new(50.0, 5.0, -30.0);
    let down = physics(&app).raycast(from, DOWN, 10.0, &ANY()).unwrap();
    assert_eq!((down.entity, down.distance), (ground, 5.0));
    assert_eq!(physics(&app).raycast(from, Dir3::X, 10.0, &ANY()), None);
    assert!(broken.iter().all(|e| physics(&app).collider(*e).is_none()));

    // Put somewhere real, it is back.
    app.world_mut().get_mut::<Transform>(broken[0]).unwrap().translation = Vec3::new(50.0, 2.0, -30.0);
    sync(&mut app);
    let hit = physics(&app).raycast(from, DOWN, 10.0, &ANY()).unwrap();
    assert_eq!(hit.entity, broken[0]);
    assert!((hit.distance - 2.0).abs() < 1e-4, "{hit:?}");
}
