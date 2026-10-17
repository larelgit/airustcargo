//! Geometry that breaks naive controllers: corners, ceilings, gaps, thin obstacles,
//! cramped ledges, shallow water, crowds.

use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

const RADIUS: f32 = 0.4;
const SKIN: f32 = 0.02;

fn run(app: &mut App, e: Entity, dir: Vec3, ticks: u64) {
    walk(app.world_mut(), e, dir);
    for _ in 0..ticks {
        step(app, 1);
        assert_clear(app, e);
    }
}

#[test]
fn an_inside_corner_stops_cleanly() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -10.0), Vec3::new(4.0, 3.0, 10.0));
    block(app.world_mut(), Vec3::new(-10.0, 0.0, 3.0), Vec3::new(10.0, 3.0, 4.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::new(1.0, 0.0, 1.0).normalize(), 90);
    let at = feet(&app, e);
    assert!((at.x - (3.0 - RADIUS - SKIN)).abs() < 2e-3 && (at.z - (3.0 - RADIUS - SKIN)).abs() < 2e-3, "{at}");
    run(&mut app, e, Vec3::new(1.0, 0.0, 1.0).normalize(), 30);
    assert_eq!(feet(&app, e), at, "no jitter in the corner");
    assert!(state(&app, e).velocity.length() < 1e-3);
}

#[test]
fn a_low_ceiling_cuts_a_jump_short() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(-5.0, 2.5, -5.0), Vec3::new(5.0, 3.0, 5.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    step(&mut app, 2);
    press_jump(app.world_mut(), e);
    let mut top = 0.0_f32;
    for _ in 0..60 {
        step(&mut app, 1);
        assert_clear(&app, e);
        top = top.max(feet(&app, e).y);
    }
    assert!(top <= 2.5 - 1.8 && top > 0.6, "head bumped at the ceiling: {top}");
    assert_eq!(state(&app, e).mode, MotionMode::Ground);
    assert_eq!(seen(&app).landed.len(), 1);
}

#[test]
fn gaps_narrower_than_the_body_block_it() {
    for (gap, passes) in [(0.7, false), (1.0, true)] {
        let mut app = app();
        floor(app.world_mut());
        block(app.world_mut(), Vec3::new(3.0, 0.0, -10.0), Vec3::new(4.0, 3.0, -gap * 0.5));
        block(app.world_mut(), Vec3::new(3.0, 0.0, gap * 0.5), Vec3::new(4.0, 3.0, 10.0));
        let e = hero(app.world_mut(), Vec3::ZERO);
        run(&mut app, e, Vec3::X, 120);
        assert_eq!(feet(&app, e).x > 5.0, passes, "gap {gap}: {}", feet(&app, e));
    }
}

#[test]
fn an_overhang_lower_than_the_body_blocks_it() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 1.5, -10.0), Vec3::new(5.0, 3.0, 10.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 90);
    assert!(feet(&app, e).x < 3.0 && feet(&app, e).y < 0.05, "{}", feet(&app, e));
    assert!(seen(&app).mantled.is_empty());
}

#[test]
fn no_mantle_without_room_on_top() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(20.0, 1.2, 5.0));
    // 1.3 m of headroom above the ledge: less than the 1.8 m body.
    block(app.world_mut(), Vec3::new(2.5, 2.5, -5.0), Vec3::new(20.0, 3.0, 5.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 90);
    assert!(seen(&app).mantled.is_empty(), "{:?}", seen(&app).mantled);
    assert!(feet(&app, e).x < 3.0);
}

#[test]
fn a_thin_low_wall_is_stepped_over() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(3.1, 0.3, 5.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 90);
    let at = feet(&app, e);
    assert!(at.x > 5.0 && (at.y - SKIN).abs() < 1e-3, "{at}");
    assert!(seen(&app).mantled.is_empty() && seen(&app).jumped.is_empty());
    assert!(seen(&app).landed.iter().all(|l| l.fall_height < 0.35), "{:?}", seen(&app).landed);
}

#[test]
fn falling_into_shallow_water_lands_on_the_bottom() {
    let mut app = app();
    floor(app.world_mut());
    volume(app.world_mut(), Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 1.0, 5.0), VolumeKind::Water);
    let e = hero(app.world_mut(), Vec3::new(0.0, 3.0, 0.0));
    step(&mut app, 60);
    let s = state(&app, e);
    assert_eq!(s.mode, MotionMode::Ground, "1 m of water is wading depth");
    assert_eq!(s.water_surface, Some(1.0));
    assert_eq!(seen(&app).landed.len(), 1);
}

#[test]
fn a_fall_into_water_deep_enough_to_swim_never_lands() {
    // However fast the fall, it may pass the whole float depth in one tick and touch the
    // bottom: that is a swim, not a landing.
    let mut failed = Vec::new();
    for depth in [1.35_f32, 1.4, 1.5, 2.0, 4.0] {
        for height in [3.0, 10.0, 20.0, 60.0] {
            let mut app = app();
            block(app.world_mut(), Vec3::new(-20.0, -depth - 1.0, -20.0), Vec3::new(20.0, -depth, 20.0));
            volume(app.world_mut(), Vec3::new(-3.0, -depth, -3.0), Vec3::new(3.0, 0.0, 3.0), VolumeKind::Water);
            let e = hero(app.world_mut(), Vec3::new(0.0, height, 0.0));
            step(&mut app, 240);
            let seen = seen(&app);
            let modes: Vec<MotionMode> = seen.modes.iter().map(|m| m.to).collect();
            let floats = (feet(&app, e).y + 1.1).abs() < 0.01;
            if !seen.landed.is_empty() || modes != [MotionMode::Air, MotionMode::Water] || !floats {
                failed.push(format!("{depth} m deep, from {height} m: {:?}, modes {modes:?} at {}", seen.landed, feet(&app, e)));
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn a_pool_wall_too_high_keeps_the_swimmer_in() {
    let mut app = app();
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, 2.0, 20.0)); // 2 m above the water
    volume(app.world_mut(), Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
    let e = hero(app.world_mut(), Vec3::new(0.0, -1.1, 0.0));
    run(&mut app, e, Vec3::X, 120);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    assert!(seen(&app).mantled.is_empty());
    assert!(feet(&app, e).x < 4.0 - RADIUS + 1e-3);
    // Jumping does not help either.
    press_jump(app.world_mut(), e);
    run(&mut app, e, Vec3::X, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    assert!(seen(&app).jumped.is_empty());
}

#[test]
fn jumping_in_water_climbs_out_at_a_ledge() {
    let mut app = app();
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, 0.3, 20.0));
    volume(app.world_mut(), Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
    let e = hero(app.world_mut(), Vec3::new(3.4, -1.1, 0.0));
    // Facing the wall, no input toward it: swims in place.
    set_input(app.world_mut(), e, LocomotionInput { face: Some(Vec3::X), ..default() });
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    assert!(seen(&app).mantled.is_empty());
    press_jump(app.world_mut(), e);
    step(&mut app, 60);
    assert_eq!(seen(&app).mantled.len(), 1);
    let at = feet(&app, e);
    assert!(at.x > 4.0 && (at.y - 0.3 - SKIN).abs() < 1e-3, "{at}");
}

#[test]
fn gliding_into_a_wall_slides_along_it() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(-30.0, 0.0, -10.0), Vec3::new(0.0, 20.0, 10.0));
    block(app.world_mut(), Vec3::new(10.0, 0.0, -50.0), Vec3::new(11.0, 40.0, 50.0));
    let e = hero(app.world_mut(), Vec3::new(-1.0, 20.0, 0.0));
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::new(1.0, 0.0, 0.3).normalize(), glide: true, ..default() });
    for _ in 0..300 {
        step(&mut app, 1);
        assert_clear(&app, e);
        assert!(feet(&app, e).x <= 10.0 - RADIUS + 1e-3);
    }
    let at = feet(&app, e);
    assert!(at.z > 3.0, "slid along the wall: {at}");
}

#[test]
fn garbage_input_is_ignored() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero(app.world_mut(), Vec3::ZERO);
    let nan = Vec3::splat(f32::NAN);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: nan, face: Some(nan), external: nan, ..default() });
    step(&mut app, 10);
    assert!(feet(&app, e).is_finite() && state(&app, e).velocity.is_finite());
    let forward = app.world().get::<Transform>(e).unwrap().forward();
    assert!(forward.is_finite());
}

fn crowd() -> Vec<[u32; 3]> {
    let mut app = app();
    floor(app.world_mut());
    let mut heroes = Vec::new();
    for i in 0..10 {
        let a = i as f32 * std::f32::consts::TAU / 10.0;
        let at = Vec3::new(ops::cos(a), 0.0, ops::sin(a)) * 4.0;
        let e = hero(app.world_mut(), at);
        walk(app.world_mut(), e, -at.normalize());
        heroes.push(e);
    }
    step(&mut app, 120);
    for e in &heroes {
        walk(app.world_mut(), *e, Vec3::ZERO);
    }
    step(&mut app, 60);
    for (i, a) in heroes.iter().enumerate() {
        assert_clear(&app, *a);
        for b in &heroes[i + 1..] {
            let d = feet(&app, *a) - feet(&app, *b);
            assert!(Vec3::new(d.x, 0.0, d.z).length() > 2.0 * RADIUS - 0.05, "still squeezed together: {d}");
        }
    }
    heroes.iter().map(|e| feet(&app, *e).to_array().map(f32::to_bits)).collect()
}

#[test]
fn a_crowd_settles_apart_and_deterministically() {
    assert_eq!(crowd(), crowd());
}

#[test]
fn resting_against_a_turned_wall_stays_on_the_ground() {
    // Whatever the gap the body happens to rest at, sliding down along a wall is not
    // landing on it.
    let mut lifted = Vec::new();
    for (x, turn) in [(3.0_f32, 0.0_f32), (3.0, 0.3), (12.9, 1.1), (55.55, 0.7)] {
        for k in -10..=10 {
            let gap = SKIN * 0.5 + k as f32 * 2e-6;
            let mut app = app();
            floor(app.world_mut());
            let rot = Quat::from_rotation_y(turn);
            let face = Vec3::new(x, 0.0, 0.0);
            turned_block(app.world_mut(), face + Vec3::Y * 1.5 + rot * Vec3::X * 0.5, Vec3::new(0.5, 1.5, 5.0), rot);
            let e = hero(app.world_mut(), face + rot * Vec3::NEG_X * (RADIUS + gap) + Vec3::Y * SKIN);
            step(&mut app, 3);
            let s = state(&app, e);
            if s.mode != MotionMode::Ground || !s.grounded {
                lifted.push(format!("wall at {x} turned {turn}, gap {gap}: {:?} at {}", s.mode, feet(&app, e)));
            }
        }
    }
    assert!(lifted.is_empty(), "{}", lifted.join("\n"));
}
