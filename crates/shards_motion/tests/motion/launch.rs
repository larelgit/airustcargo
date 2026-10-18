//! `Launch` and `Ghost` (B2c): the body's owner launches it up for a leap, and lets it pass through
//! other characters.

use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

fn params() -> CharacterParams {
    // The jump height is not the launch's: a launch peaks where it asks.
    CharacterParams { jump_height: 0.5, ..default() }
}

fn peak(app: &mut App, e: Entity, ticks: u64) -> f32 {
    let start = feet(app, e).y;
    let mut top = start;
    for _ in 0..ticks {
        step(app, 1);
        top = top.max(feet(app, e).y);
    }
    top - start
}

#[test]
fn a_launch_peaks_at_its_height_whatever_the_gravity() {
    for (gravity, height) in [(20.0, 1.2), (18.0, 1.2), (30.0, 0.6), (12.0, 2.5)] {
        let mut app = app();
        floor(app.world_mut());
        let e = hero_with(app.world_mut(), Vec3::ZERO, CharacterParams { gravity, ..params() });
        step(&mut app, 3);
        app.world_mut().entity_mut(e).insert(Launch { height });
        let rose = peak(&mut app, e, 120);
        assert!((rose - height).abs() < height * 0.02, "gravity {gravity}: launched to {rose}, wanted {height}");
        assert!(app.world().get::<Launch>(e).is_none(), "the launch is taken");
        assert_eq!(state(&app, e).mode, MotionMode::Ground, "and the body lands");
        assert_eq!(seen(&app).landed.len(), 1);
        assert!(seen(&app).jumped.is_empty(), "no one pressed jump: no Jumped event");
    }
}

#[test]
fn a_launch_takes_a_body_an_action_owns() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::ZERO, params());
    step(&mut app, 3);
    // Locked, and a jump pressed: the walker's jump is refused, the launch is not.
    set_input(app.world_mut(), e, LocomotionInput { locked: true, jump: true, ..default() });
    app.world_mut().entity_mut(e).insert(Launch { height: 1.0 });
    let rose = peak(&mut app, e, 120);
    assert!((rose - 1.0).abs() < 0.03, "{rose}");
}

#[test]
fn a_launch_in_the_air_does_nothing_and_still_goes() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::new(0.0, 6.0, 0.0), params());
    step(&mut app, 10);
    let before = state(&app, e);
    assert_eq!(before.mode, MotionMode::Air);
    assert!(before.velocity.y < 0.0);
    app.world_mut().entity_mut(e).insert(Launch { height: 3.0 });
    step(&mut app, 1);
    assert!(app.world().get::<Launch>(e).is_none(), "taken or not, a launch is for one tick");
    assert!(state(&app, e).velocity.y < before.velocity.y, "still falling, faster: {:?}", state(&app, e).velocity);
}

#[test]
fn a_launch_just_off_a_ledge_still_works_within_coyote_time() {
    let mut app = app();
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(40.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(-20.0, -1.0, -5.0), Vec3::new(5.0, 0.0, 5.0));
    let e = hero_with(app.world_mut(), Vec3::new(2.0, 0.0, 0.0), params());
    walk(app.world_mut(), e, Vec3::X);
    let ticks = drive(&mut app, 240, |w| w.get::<MotionState>(e).unwrap().mode == MotionMode::Ground);
    assert!(ticks < 240);
    step(&mut app, 3);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    app.world_mut().entity_mut(e).insert(Launch { height: 1.0 });
    step(&mut app, 1);
    assert!(state(&app, e).velocity.y > 3.0, "launched in coyote time: {:?}", state(&app, e).velocity);
}

#[test]
fn two_capsules_are_pushed_apart_and_a_ghost_is_not() {
    let gap = |ghost: bool| {
        let mut app = app();
        floor(app.world_mut());
        let a = hero(app.world_mut(), Vec3::new(0.0, 0.0, 0.0));
        let b = hero(app.world_mut(), Vec3::new(0.2, 0.0, 0.0));
        if ghost {
            app.world_mut().entity_mut(a).insert(Ghost);
        }
        step(&mut app, 30);
        let (pa, pb) = (feet(&app, a), feet(&app, b));
        Vec3::new(pb.x - pa.x, 0.0, pb.z - pa.z).length()
    };
    assert!(gap(false) >= 0.8 - 1e-3, "apart: {}", gap(false));
    assert!((gap(true) - 0.2).abs() < 1e-3, "a ghost stays where it is: {}", gap(true));
}
