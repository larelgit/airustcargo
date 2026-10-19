//! Temporary ground (R2): a collider a game lays down in the middle of play, from a system inside
//! the simulation, is ground and solid for a character from the next tick, and its going drops what
//! stood on it. A game's toppled boss is such ground while it lies still (the manual's "Ground laid
//! in the middle of play (R2)"). Static or kinematic: the controller asks only for the `WORLD` layer;
//! a kinematic one that moves does not carry what stands on it.

use ai_core::SimSet;
use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

/// What the test's game system does on its next tick.
#[derive(Resource, Default)]
struct Plan {
    lay: Option<Body>,
    lift: bool,
    slab: Option<Entity>,
}

/// A game system in `SimSet::Act`: lays a 4 x 1 x 4 m slab of the body asked for, its top at
/// y = 1 around the origin, or takes it away.
fn lay_and_lift(mut commands: Commands, mut plan: ResMut<Plan>) {
    if let Some(body) = plan.lay.take() {
        let slab = commands.spawn((Collider::new(Shape::cuboid(Vec3::new(2.0, 0.5, 2.0))), body, Transform::from_xyz(0.0, 0.5, 0.0)));
        plan.slab = Some(slab.id());
    }
    if std::mem::take(&mut plan.lift)
        && let Some(slab) = plan.slab.take()
    {
        commands.entity(slab).despawn();
    }
}

fn arena() -> (App, Entity) {
    let mut app = app();
    app.init_resource::<Plan>().add_systems(FixedUpdate, lay_and_lift.in_set(SimSet::Act));
    let world = app.world_mut();
    floor(world);
    // In the air above where the slab will lie.
    let hero = hero(world, Vec3::new(0.0, 4.0, 0.0));
    (app, hero)
}

fn ground_under(app: &App, hero: Entity) -> (f32, bool) {
    (feet(app, hero).y, state(app, hero).grounded)
}

#[test]
fn a_slab_laid_mid_play_is_ground_to_walk_on_until_it_goes() {
    for body in [Body::Static, Body::Kinematic] {
        let (mut app, hero) = arena();
        app.world_mut().resource_mut::<Plan>().lay = Some(body);
        step(&mut app, 90);
        let (y, grounded) = ground_under(&app, hero);
        assert!((y - 1.0).abs() < 0.02 && grounded, "{body:?}: lands on the slab's top, at {y} (grounded {grounded})");
        assert_eq!(seen(&app).landed.len(), 1, "{body:?}: one landing, on the slab");
        assert_clear(&app, hero);

        // It walks on it like any ground ...
        walk(app.world_mut(), hero, Vec3::X);
        step(&mut app, 20);
        walk(app.world_mut(), hero, Vec3::ZERO);
        step(&mut app, 30);
        let (y, grounded) = ground_under(&app, hero);
        let x = feet(&app, hero).x;
        assert!(x > 0.5 && x < 2.0 && (y - 1.0).abs() < 0.02 && grounded, "{body:?}: walked to x {x} at y {y} (grounded {grounded})");

        // ... and when the game takes it away, it falls to the floor.
        let slab = app.world().resource::<Plan>().slab.unwrap();
        app.world_mut().resource_mut::<Plan>().lift = true;
        step(&mut app, 90);
        let (y, grounded) = ground_under(&app, hero);
        assert!(y.abs() < 0.02 && grounded, "{body:?}: back on the floor, at {y} (grounded {grounded})");
        assert_eq!(seen(&app).landed.len(), 2, "{body:?}");
        assert!(app.world().get_entity(slab).is_err(), "{body:?}: the slab is gone");
    }
}

#[test]
fn a_slab_laid_mid_play_is_climbed_like_any_ledge() {
    let (mut app, _) = arena();
    // A walker on the floor heading for where the slab will lie: 1 m tall, too high for a step,
    // within a mantle's reach.
    let walker = hero(app.world_mut(), Vec3::new(-5.0, 0.0, 0.0));
    app.world_mut().resource_mut::<Plan>().lay = Some(Body::Static);
    step(&mut app, 30);
    let before = seen(&app).mantled.len();
    walk(app.world_mut(), walker, Vec3::X);
    step(&mut app, 120);
    walk(app.world_mut(), walker, Vec3::ZERO);
    step(&mut app, 30);
    let at = feet(&app, walker);
    assert!(seen(&app).mantled.len() > before, "it mantled: {:?}", seen(&app).mantled);
    assert!(at.x > -2.0 && at.x < 2.0 && (at.y - 1.0).abs() < 0.02 && state(&app, walker).grounded, "stands on top: {at}");
    assert_clear(&app, walker);
}

/// How far (m) a game system moves the slab along +X each tick, after it is laid: a kinematic
/// platform a game moves.
#[derive(Resource, Default)]
struct Slide(f32);

fn slide(slide: Res<Slide>, plan: Res<Plan>, mut transforms: Query<&mut Transform>) {
    if slide.0 != 0.0
        && let Some(slab) = plan.slab
        && let Ok(mut tf) = transforms.get_mut(slab)
    {
        tf.translation.x += slide.0;
    }
}

#[test]
fn a_kinematic_slab_that_moves_leaves_what_stands_on_it_behind() {
    let (mut app, hero) = arena();
    app.init_resource::<Slide>().add_systems(FixedUpdate, slide.in_set(SimSet::Act).after(lay_and_lift));
    app.world_mut().resource_mut::<Plan>().lay = Some(Body::Kinematic);
    step(&mut app, 90);
    let x = feet(&app, hero).x;
    assert!(ground_under(&app, hero).1, "stands on the slab");
    // 3 m along +X a second: the slab slides out from under the body, which stays where it stood
    // (no platform motion) and falls to the floor.
    app.world_mut().resource_mut::<Slide>().0 = 0.05;
    step(&mut app, 120);
    let at = feet(&app, hero);
    assert!((at.x - x).abs() < 0.05, "not carried: from x {x} to {at}");
    let (y, grounded) = ground_under(&app, hero);
    assert!(y.abs() < 0.02 && grounded, "fell to the floor: {at} (grounded {grounded})");
    assert_eq!(seen(&app).landed.len(), 2);
}
