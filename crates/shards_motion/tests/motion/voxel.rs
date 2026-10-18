//! A voxel field in a running world: its `ColliderId`, its place among the colliders, despawn,
//! replace and renumber; a body standing on it.

use std::sync::Arc;

use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

fn physics(app: &App) -> &PhysicsWorld {
    app.world().resource::<PhysicsWorld>()
}

/// A floor of 1 m cells, its top at `top`, over x and z in -4..4.
fn floor(top: f32) -> VoxelCollider {
    VoxelCollider::new(VoxelField::new(Vec3::new(-4.0, top - 2.0, -4.0), 1.0, UVec3::new(8, 2, 8), |_| true))
}

/// What a ray straight down at (0.5, 0.5) meets first, and how high.
fn under(app: &App) -> Option<(Entity, f32)> {
    physics(app).raycast(Vec3::new(0.5, 10.0, 0.5), Dir3::NEG_Y, 20.0, &QueryFilter::default()).map(|h| (h.entity, h.point.y))
}

fn ids(app: &App) -> Vec<(Entity, ColliderId)> {
    physics(app).fields().map(|(e, id, _)| (e, id)).collect()
}

#[test]
fn a_field_takes_a_collider_id_in_turn_and_leaves_with_its_entity() {
    let mut app = app();
    let world = app.world_mut();
    let post = world.spawn((Collider::new(Shape::cuboid(Vec3::splat(0.5))), Transform::from_xyz(20.0, 0.0, 0.0))).id();
    let low = world.spawn(floor(0.0)).id();
    let high = world.spawn(floor(1.0)).id();
    let id = |app: &App, e: Entity| *app.world().get::<ColliderId>(e).unwrap();
    assert!(id(&app, post) < id(&app, low) && id(&app, low) < id(&app, high), "ids in the order the colliders came");
    app.update();
    assert_eq!(ids(&app), vec![(low, id(&app, low)), (high, id(&app, high))], "fields in id order");
    assert_eq!(under(&app), Some((high, 1.0)));
    assert!(physics(&app).field(low).is_some() && physics(&app).field(post).is_none());

    // Two fields over the same cells: the one with the lower id wins a tie.
    let world = app.world_mut();
    world.entity_mut(high).insert(floor(0.0));
    app.update();
    assert_eq!(under(&app), Some((low, 0.0)), "the same distance: the lower id");
    assert_eq!(id(&app, high), ids(&app)[1].1, "a replaced field keeps its id");

    // Renumbered (a snapshot does it): the slot moves to the new id's turn.
    app.world_mut().entity_mut(low).insert(ColliderId(1000));
    app.update();
    assert_eq!(ids(&app), vec![(high, id(&app, high)), (low, ColliderId(1000))]);
    assert_eq!(under(&app), Some((high, 0.0)));

    // Despawned or stripped of its field: gone from the world.
    app.world_mut().despawn(high);
    app.update();
    assert_eq!(ids(&app), vec![(low, ColliderId(1000))]);
    app.world_mut().entity_mut(low).remove::<VoxelCollider>();
    app.update();
    assert!(ids(&app).is_empty());
    assert_eq!(under(&app), None);
    assert!(physics(&app).solid_span(LayerMask::WORLD).is_some_and(|(lo, hi)| (lo, hi) == (-0.5, 0.5)), "only the post is left");
}

#[test]
fn a_body_stands_and_walks_on_a_field_and_a_changed_field_moves_its_ground() {
    let mut app = app();
    let ground = app.world_mut().spawn(floor(0.0)).id();
    let hero = hero(app.world_mut(), Vec3::new(0.5, 0.5, 0.5));
    step(&mut app, 30);
    assert!(state(&app, hero).grounded && feet(&app, hero).y.abs() < 0.05, "{:?}", feet(&app, hero));
    set_input(app.world_mut(), hero, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 30);
    assert!(state(&app, hero).grounded && feet(&app, hero).x > 2.0, "walked: {:?}", feet(&app, hero));

    // A cell removed under the body: a new field (edits are a new `VoxelCollider`); it falls.
    set_input(app.world_mut(), hero, LocomotionInput::default());
    let at = feet(&app, hero);
    let old = app.world().get::<VoxelCollider>(ground).unwrap().field.clone();
    let hole = VoxelField::new(old.origin(), old.cell(), old.size(), |c| {
        let centre = old.center(c);
        old.solid(c) && (centre.x - at.x).abs() > 1.5
    });
    app.world_mut().entity_mut(ground).insert(VoxelCollider { field: Arc::new(hole) });
    step(&mut app, 30);
    assert!(feet(&app, hero).y < -1.0, "fell through the hole: {:?}", feet(&app, hero));
}
