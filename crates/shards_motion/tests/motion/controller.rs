//! Character controller behaviour, one mechanic per test, headless and deterministic.

use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

const SKIN: f32 = 0.02;
const RADIUS: f32 = 0.4;

fn params() -> CharacterParams {
    CharacterParams::default()
}

/// Ticks with `dir` held, checking every tick that the capsule stays out of walls.
fn run(app: &mut App, e: Entity, dir: Vec3, ticks: u64) {
    walk(app.world_mut(), e, dir);
    for _ in 0..ticks {
        step(app, 1);
        assert_clear(app, e);
    }
}

fn modes(app: &App) -> Vec<MotionMode> {
    seen(app).modes.iter().map(|m| m.to).collect()
}

#[test]
fn walks_runs_and_sprints_at_the_set_speeds() {
    let p = params();
    for (input, sprint, speed) in
        [(Vec3::X * 0.5, false, p.walk_speed), (Vec3::X, false, p.run_speed), (Vec3::X, true, p.sprint_speed)]
    {
        let mut app = app();
        floor(app.world_mut());
        let e = hero(app.world_mut(), Vec3::ZERO);
        step(&mut app, 3);
        set_input(app.world_mut(), e, LocomotionInput { move_dir: input, sprint, ..default() });
        let start = feet(&app, e);
        step(&mut app, 60);
        let s = state(&app, e);
        assert_eq!(s.mode, MotionMode::Ground);
        assert!((s.velocity.length() - speed).abs() < 1e-3, "{input} sprint {sprint}: {}", s.velocity);
        assert_eq!(s.sprinting, sprint);
        // One second, minus what the speed-up from 0 cost.
        let ramp = speed / p.acceleration;
        let expected = speed * (1.0 - ramp) + speed * ramp * 0.5;
        let moved = feet(&app, e) - start;
        assert!((moved.x - expected).abs() < 0.02, "{input}: moved {moved}, expected {expected}");
        assert!(moved.y.abs() < 1e-4 && moved.z.abs() < 1e-4, "{moved}");
        assert!((feet(&app, e).y - SKIN).abs() < 1e-3, "feet rest a skin above the floor: {}", feet(&app, e));
        let forward = app.world().get::<Transform>(e).unwrap().forward();
        assert!((*forward - Vec3::X).length() < 1e-3, "faces where it walks: {forward:?}");
    }
}

#[test]
fn stopping_decelerates_to_rest() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 60);
    run(&mut app, e, Vec3::ZERO, 10);
    assert_eq!(state(&app, e).velocity, Vec3::ZERO, "5 m/s at 50 m/s² stops in 6 ticks");
    let at = feet(&app, e);
    step(&mut app, 30);
    assert_eq!(feet(&app, e), at, "standing still is still");
}

#[test]
fn walls_stop_movement_even_at_sprint_speed() {
    let mut app = app();
    floor(app.world_mut());
    // A 0.2 m wall, 3 m tall.
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(3.2, 3.0, 5.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: true, ..default() });
    for _ in 0..120 {
        step(&mut app, 1);
        assert_clear(&app, e);
        assert!(feet(&app, e).x <= 3.0 - RADIUS + 1e-3, "went into the wall: {}", feet(&app, e));
    }
    let x = feet(&app, e).x;
    assert!(x > 3.0 - RADIUS - 2.0 * SKIN, "stops at the wall, a skin away: {x}");
    assert!(state(&app, e).velocity.x.abs() < 1e-3, "no speed builds up into the wall");
    assert!(seen(&app).mantled.is_empty(), "3 m is not a ledge");

    // Diagonally into the wall: slides along it.
    walk(app.world_mut(), e, Vec3::new(1.0, 0.0, 1.0).normalize());
    let z = feet(&app, e).z;
    run(&mut app, e, Vec3::new(1.0, 0.0, 1.0).normalize(), 30);
    assert!(feet(&app, e).z > z + 1.0, "slides along the wall: {}", feet(&app, e));
    assert!(feet(&app, e).x <= 3.0 - RADIUS + 1e-3);
}

#[test]
fn never_tunnels_through_thin_walls() {
    // 100 m/s is ten times a sprint: still stopped by a 0.2 m wall.
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(3.2, 3.0, 5.0));
    let fast = CharacterParams { sprint_speed: 100.0, acceleration: 10_000.0, ..params() };
    let e = hero_with(app.world_mut(), Vec3::ZERO, fast);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: true, ..default() });
    for _ in 0..30 {
        step(&mut app, 1);
        assert!(feet(&app, e).x < 3.0, "tunnelled: {}", feet(&app, e));
        assert_clear(&app, e);
    }
}

/// Eight 0.25 m steps with 0.5 m treads from x = 2, landing at 2 m.
fn stairs(world: &mut World) {
    for i in 0..8 {
        let x = 2.0 + 0.5 * i as f32;
        block(world, Vec3::new(x, 0.0, -3.0), Vec3::new(40.0, 0.25 * (i + 1) as f32, 3.0));
    }
}

#[test]
fn stairs_are_climbed_without_jumping() {
    let mut app = app();
    floor(app.world_mut());
    stairs(app.world_mut());
    let e = hero(app.world_mut(), Vec3::ZERO);
    let mut highest_drop = 0.0_f32;
    let mut last_y = 0.0;
    walk(app.world_mut(), e, Vec3::X);
    for _ in 0..180 {
        step(&mut app, 1);
        assert_clear(&app, e);
        let y = feet(&app, e).y;
        highest_drop = highest_drop.max(last_y - y);
        last_y = y;
    }
    let end = feet(&app, e);
    assert!(end.x > 7.0, "made it across: {end}");
    assert!((end.y - 2.0 - SKIN).abs() < 1e-3, "on the landing: {end}");
    let seen = seen(&app);
    assert!(seen.jumped.is_empty() && seen.mantled.is_empty(), "{seen:?}");
    assert!(seen.landed.iter().all(|l| l.fall_height < 0.3), "no falling on the way up: {:?}", seen.landed);
    assert!(highest_drop < 0.05, "never slipped back down a step: {highest_drop}");

    // And back down, snapping from step to step without falling.
    let modes_before = seen.modes.len();
    run(&mut app, e, Vec3::NEG_X, 180);
    let end = feet(&app, e);
    assert!(end.x < 1.5 && (end.y - SKIN).abs() < 1e-3, "back at the bottom: {end}");
    assert_eq!(seen_modes_since(&app, modes_before), Vec::<MotionMode>::new(), "stayed on the ground all the way down");
}

fn seen_modes_since(app: &App, from: usize) -> Vec<MotionMode> {
    seen(app).modes[from..].iter().map(|m| m.to).collect()
}

/// `n` steps of `rise` with 0.5 m treads from x = 2, wide enough to cross at an angle.
fn wide_stairs(world: &mut World, rise: f32, n: usize) {
    for i in 0..n {
        let x = 2.0 + 0.5 * i as f32;
        block(world, Vec3::new(x, 0.0, -80.0), Vec3::new(80.0, rise * (i + 1) as f32, 80.0));
    }
}

#[test]
fn stairs_are_climbed_from_any_start_at_any_pace_and_angle() {
    // Every riser is met at a different phase of the tick; none may stop the climb.
    const STEPS: usize = 6;
    let p = params();
    let mut failed = Vec::new();
    for rise in [0.1, 0.2, 0.25, 0.3, p.step_height] {
        for angle in [0.0_f32, 30.0, 60.0] {
            let mut app = app();
            floor(app.world_mut());
            wide_stairs(app.world_mut(), rise, STEPS);
            let dir = Vec3::new(angle.to_radians().cos(), 0.0, angle.to_radians().sin());
            // Parallel lanes, 3 m apart: bodies never meet.
            let mut runners = Vec::new();
            for (lane, (amount, start)) in
                [0.25, 0.5, 1.0].into_iter().flat_map(|a| (0..8).map(move |i| (a, i as f32 * 0.0625))).enumerate()
            {
                let e = hero(app.world_mut(), Vec3::new(start, 0.0, -50.0 + 3.0 * lane as f32));
                walk(app.world_mut(), e, dir * amount);
                runners.push((e, amount, start));
            }
            // Long enough for the slowest one, across 4 m of stairs at 60°; whoever is
            // up stops there.
            let top = 2.0 + 0.5 * STEPS as f32;
            for _ in 0..900 {
                step(&mut app, 1);
                for &(e, ..) in &runners {
                    if feet(&app, e).x > top + 0.5 {
                        walk(app.world_mut(), e, Vec3::ZERO);
                    }
                }
            }
            let seen = seen(&app);
            for (e, amount, start) in runners {
                let at = feet(&app, e);
                assert_clear(&app, e);
                let run = format!("rise {rise} at {angle}°, input {amount} from x={start}");
                if at.x < top || (at.y - rise * STEPS as f32 - SKIN).abs() > 1e-3 {
                    failed.push(format!("{run}: stopped at {at}"));
                }
                let climbs = seen.mantled.iter().filter(|m| m.entity == e).count();
                if climbs > 0 || seen.jumped.iter().any(|j| j.entity == e) {
                    failed.push(format!("{run}: {climbs} mantles or a jump on stairs"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{} climbs failed:\n{}", failed.len(), failed.join("\n"));
}

#[test]
fn a_ledge_within_reach_is_mantled() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(20.0, 1.2, 5.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 90);
    let seen = seen(&app);
    assert_eq!(seen.mantled.len(), 1, "{seen:?}");
    assert!((seen.mantled[0].height - 1.2).abs() < 0.03, "{:?}", seen.mantled);
    assert!(modes(&app).contains(&MotionMode::Mantle));
    let end = feet(&app, e);
    assert!(end.x > 3.4 && (end.y - 1.2 - SKIN).abs() < 1e-3, "on top: {end}");
    assert_eq!(state(&app, e).mode, MotionMode::Ground);
    assert!(seen.jumped.is_empty());
}

#[test]
fn a_wall_too_tall_is_not_mantled() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(20.0, 2.5, 5.0));
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 120);
    assert!(seen(&app).mantled.is_empty());
    let end = feet(&app, e);
    assert!(end.x < 3.0 - RADIUS + 1e-3 && end.y < 0.05, "{end}");
}

#[test]
fn a_ledge_is_grabbed_from_a_jump() {
    // 2 m: out of reach from the ground (1.6 m), within reach at the top of a jump.
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(3.0, 0.0, -5.0), Vec3::new(20.0, 2.0, 5.0));
    let e = hero(app.world_mut(), Vec3::new(1.5, 0.0, 0.0));
    run(&mut app, e, Vec3::X, 30);
    assert!(seen(&app).mantled.is_empty());
    press_jump(app.world_mut(), e);
    run(&mut app, e, Vec3::X, 90);
    assert_eq!(seen(&app).mantled.len(), 1, "{:?}", seen(&app));
    let end = feet(&app, e);
    assert!(end.x > 3.4 && (end.y - 2.0 - SKIN).abs() < 1e-3, "on top: {end}");
}

/// A ramp rising toward +x at `deg`, its surface starting at ground level at x = 2.
fn ramp(world: &mut World, deg: f32) {
    let a = deg.to_radians();
    let (along, normal) = (Vec3::new(a.cos(), a.sin(), 0.0), Vec3::new(-a.sin(), a.cos(), 0.0));
    let half = Vec3::new(10.0, 0.5, 3.0);
    let center = Vec3::new(2.0, 0.0, 0.0) + along * half.x - normal * half.y;
    turned_block(world, center, half, Quat::from_rotation_z(a));
}

#[test]
fn slopes_under_the_limit_are_walked_up() {
    let mut app = app();
    floor(app.world_mut());
    ramp(app.world_mut(), 30.0);
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 120);
    let end = feet(&app, e);
    // The round bottom touches the slope uphill of the feet, which hover a little; the
    // gap to the slope (along its normal) is a skin.
    let a = 30f32.to_radians();
    let surface = (end.x - 2.0) * a.tan() + (RADIUS + SKIN) / a.cos() - RADIUS;
    assert!(end.x > 6.0, "kept going: {end}");
    assert!((end.y - surface).abs() < 1e-3, "on the slope ({surface}): {end}");
    assert_eq!(state(&app, e).mode, MotionMode::Ground);
    assert!(state(&app, e).ground_normal.y < 0.9, "stands on the slope");
    // Speed is kept going up a slope.
    assert!((state(&app, e).velocity.length() - 5.0).abs() < 1e-3);

    // And down again without leaving the ground.
    let before = seen(&app).modes.len();
    run(&mut app, e, Vec3::NEG_X, 120);
    assert!(feet(&app, e).x < 1.0);
    assert_eq!(seen_modes_since(&app, before), Vec::<MotionMode>::new());
}

#[test]
fn slopes_over_the_limit_are_refused() {
    let mut app = app();
    floor(app.world_mut());
    ramp(app.world_mut(), 60.0);
    let e = hero(app.world_mut(), Vec3::ZERO);
    run(&mut app, e, Vec3::X, 120);
    let end = feet(&app, e);
    assert!(end.y < 0.2 && end.x < 2.0, "{end}");
    assert!(seen(&app).mantled.is_empty());
}

#[test]
fn standing_on_a_steep_slope_slides_down() {
    let mut app = app();
    floor(app.world_mut());
    ramp(app.world_mut(), 60.0);
    // Dropped onto the slope 2 m up it.
    let x = 2.0 + 2.0 / 60f32.to_radians().tan();
    let e = hero(app.world_mut(), Vec3::new(x, 3.0, 0.0));
    for _ in 0..120 {
        step(&mut app, 1);
        assert_clear(&app, e);
    }
    let end = feet(&app, e);
    assert!(end.x < 2.0 && end.y < 0.05, "slid to the bottom: {end}");
    assert_eq!(state(&app, e).mode, MotionMode::Ground);
}

#[test]
fn jump_height_matches_the_param() {
    for jump_height in [1.2, 0.5, 2.5] {
        let mut app = app();
        floor(app.world_mut());
        let e = hero_with(app.world_mut(), Vec3::ZERO, CharacterParams { jump_height, ..params() });
        step(&mut app, 3);
        let start = feet(&app, e).y;
        press_jump(app.world_mut(), e);
        let mut top = start;
        for _ in 0..120 {
            step(&mut app, 1);
            top = top.max(feet(&app, e).y);
        }
        let height = top - start;
        assert!((height - jump_height).abs() < jump_height * 0.02, "jumped {height}, wanted {jump_height}");
        let seen = seen(&app);
        assert_eq!(seen.jumped.len(), 1);
        assert_eq!(seen.landed.len(), 1);
        assert!((seen.landed[0].fall_height - jump_height).abs() < jump_height * 0.02, "{:?}", seen.landed);
        assert_eq!(state(&app, e).mode, MotionMode::Ground);
    }
}

#[test]
fn a_jump_pressed_just_before_landing_still_happens() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero(app.world_mut(), Vec3::new(0.0, 1.0, 0.0));
    // Falling from 1 m takes ~0.32 s (19 ticks); press 4 ticks before touching down.
    step(&mut app, 15);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    press_jump(app.world_mut(), e);
    step(&mut app, 10);
    assert_eq!(seen(&app).landed.len(), 1);
    assert_eq!(seen(&app).jumped.len(), 1, "buffered jump");
    assert!(state(&app, e).velocity.y > 0.0);
}

/// A platform (top y = 0) ending at x = 5 above a pit floor at y = -5.
fn platform_edge(world: &mut World) {
    block(world, Vec3::new(-20.0, -6.0, -20.0), Vec3::new(40.0, -5.0, 20.0));
    block(world, Vec3::new(-20.0, -1.0, -5.0), Vec3::new(5.0, 0.0, 5.0));
}

/// Runs off the edge; returns once airborne.
fn run_off_edge(app: &mut App, e: Entity) {
    walk(app.world_mut(), e, Vec3::X);
    let ticks = drive(app, 240, |w| w.get::<MotionState>(e).unwrap().mode == MotionMode::Ground);
    assert!(ticks < 240, "never left the platform");
    assert_eq!(state(app, e).mode, MotionMode::Air);
}

#[test]
fn coyote_time_allows_a_late_jump() {
    let mut app = app();
    platform_edge(app.world_mut());
    let e = hero(app.world_mut(), Vec3::new(2.0, 0.0, 0.0));
    run_off_edge(&mut app, e);
    step(&mut app, 3); // 0.05 s after the edge: still in time
    press_jump(app.world_mut(), e);
    step(&mut app, 1);
    assert_eq!(seen(&app).jumped.len(), 1, "coyote jump");
    assert!(state(&app, e).velocity.y > 5.0);
}

#[test]
fn coyote_time_runs_out() {
    let mut app = app();
    platform_edge(app.world_mut());
    let e = hero(app.world_mut(), Vec3::new(2.0, 0.0, 0.0));
    run_off_edge(&mut app, e);
    step(&mut app, 10); // 0.17 s > 0.12 s
    press_jump(app.world_mut(), e);
    step(&mut app, 30);
    assert!(seen(&app).jumped.is_empty(), "too late: {:?}", seen(&app).jumped);
    assert!(state(&app, e).velocity.y < 0.0);
}

#[test]
fn landed_reports_the_fall_height() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero(app.world_mut(), Vec3::new(0.0, 5.0, 0.0));
    step(&mut app, 60);
    let landed = &seen(&app).landed;
    assert_eq!(landed.len(), 1);
    assert_eq!(landed[0].entity, e);
    assert!((landed[0].fall_height - (5.0 - SKIN)).abs() < 0.01, "{landed:?}");
    let expected_speed = (2.0 * 20.0 * 5.0f32).sqrt();
    assert!((landed[0].impact_speed - expected_speed).abs() < 0.4, "{landed:?}");
    assert_eq!(state(&app, e).fall_from_y, None);

    // Off a ledge: from the highest point, measured to where it lands.
    let mut app2 = crate::common::app();
    platform_edge(app2.world_mut());
    let e = hero(app2.world_mut(), Vec3::new(2.0, 0.0, 0.0));
    run(&mut app2, e, Vec3::X, 120);
    let landed = &seen(&app2).landed;
    assert_eq!(landed.len(), 1, "{landed:?}");
    let start = state(&app2, e);
    assert_eq!(start.mode, MotionMode::Ground);
    assert!((landed[0].fall_height - 5.0).abs() < 0.5, "walked off at the edge, the capsule rolls over it: {landed:?}");
}

/// A pool between x = 4 and 12: decks at y = 0, bottom at -3, water up to -0.3.
fn pool(world: &mut World) -> Entity {
    block(world, Vec3::new(-20.0, -4.0, -20.0), Vec3::new(4.0, 0.0, 20.0));
    block(world, Vec3::new(12.0, -4.0, -20.0), Vec3::new(40.0, 0.0, 20.0));
    block(world, Vec3::new(4.0, -4.0, 3.0), Vec3::new(12.0, 0.0, 20.0));
    block(world, Vec3::new(4.0, -4.0, -20.0), Vec3::new(12.0, 0.0, -3.0));
    block(world, Vec3::new(4.0, -4.0, -3.0), Vec3::new(12.0, -3.0, 3.0));
    volume(world, Vec3::new(4.0, -3.0, -3.0), Vec3::new(12.0, -0.3, 3.0), VolumeKind::Water)
}

#[test]
fn swims_across_a_pool_and_climbs_out() {
    let mut app = app();
    let water = pool(app.world_mut());
    let e = hero(app.world_mut(), Vec3::new(1.0, 0.0, 0.0));
    let p = params();
    let mut floated = false;
    walk(app.world_mut(), e, Vec3::X);
    for _ in 0..600 {
        step(&mut app, 1);
        assert_clear(&app, e);
        let s = state(&app, e);
        if s.mode == MotionMode::Water {
            assert!(!s.grounded);
            let float = -0.3 - p.float_depth;
            if (feet(&app, e).y - float).abs() < 0.01 {
                floated = true;
            }
            assert!(feet(&app, e).y > -3.0, "never sinks to the bottom");
        }
        if feet(&app, e).x > 14.0 {
            break;
        }
    }
    let modes = modes(&app);
    let order: Vec<MotionMode> = modes.iter().copied().filter(|m| *m != MotionMode::Air).collect();
    assert_eq!(order, vec![MotionMode::Water, MotionMode::Mantle, MotionMode::Ground], "{modes:?}");
    assert!(floated, "floats at the surface minus float_depth");
    let end = feet(&app, e);
    assert!(end.x > 13.0 && (end.y - SKIN).abs() < 1e-3, "out on the far deck: {end}");
    let seen = seen(&app);
    assert!((seen.mantled[0].height - (0.3 + p.float_depth)).abs() < 0.05, "{:?}", seen.mantled);
    assert!(seen.entered.iter().any(|v| v.volume == water) && seen.exited.iter().any(|v| v.volume == water));
    assert!(seen.landed.is_empty(), "water is not ground: {:?}", seen.landed);
}

#[test]
fn walks_out_of_water_where_it_gets_shallow() {
    // A beach: the pool bottom rises as a 20° slope to the deck.
    let mut app = app();
    block(app.world_mut(), Vec3::new(-20.0, -4.0, -20.0), Vec3::new(0.0, 0.0, 20.0));
    block(app.world_mut(), Vec3::new(0.0, -4.0, -20.0), Vec3::new(30.0, -3.0, 20.0));
    let a = 20f32.to_radians();
    let half = Vec3::new(6.0, 0.5, 10.0);
    // Surface from (10, -3) up toward +x.
    let (along, normal) = (Vec3::new(a.cos(), a.sin(), 0.0), Vec3::new(-a.sin(), a.cos(), 0.0));
    turned_block(app.world_mut(), Vec3::new(10.0, -3.0, 0.0) + along * half.x - normal * half.y, half, Quat::from_rotation_z(a));
    volume(app.world_mut(), Vec3::new(0.0, -3.0, -10.0), Vec3::new(30.0, -0.5, 10.0), VolumeKind::Water);
    let e = hero(app.world_mut(), Vec3::new(-1.0, 0.0, 0.0));
    walk(app.world_mut(), e, Vec3::X);
    // The beach rises out of the water around x = 17.
    let ticks = drive(&mut app, 600, |w| w.get::<Transform>(e).unwrap().translation.x < 18.0);
    assert!(ticks < 600, "never got there: {}", feet(&app, e));
    let modes = modes(&app);
    assert!(modes.contains(&MotionMode::Water), "{modes:?}");
    assert_eq!(state(&app, e).mode, MotionMode::Ground, "{modes:?} at {}", feet(&app, e));
    assert!(seen(&app).mantled.is_empty());
    assert!(feet(&app, e).y > -2.0, "walked up the beach: {}", feet(&app, e));
}

/// A 20 m cliff ending at x = 0 over flat ground.
fn cliff(world: &mut World) {
    floor(world);
    block(world, Vec3::new(-30.0, 0.0, -10.0), Vec3::new(0.0, 20.0, 10.0));
}

/// Runs off the cliff (gliding or not); returns where and after how many ticks it lands,
/// and the fastest fall on the way.
fn fly(glide: bool, updraft: bool) -> (Vec3, u64, f32, f32) {
    let mut app = app();
    cliff(app.world_mut());
    if updraft {
        let kind = VolumeKind::Updraft { accel: 20.0, max_speed: 6.0 };
        volume(app.world_mut(), Vec3::new(8.0, 0.0, -5.0), Vec3::new(20.0, 40.0, 5.0), kind);
    }
    let e = hero(app.world_mut(), Vec3::new(-3.0, 20.0, 0.0));
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, glide, ..default() });
    let (mut ticks, mut fastest, mut top) = (0, 0.0_f32, 0.0_f32);
    let mut airborne = false;
    for _ in 0..1200 {
        step(&mut app, 1);
        assert_clear(&app, e);
        let s = state(&app, e);
        if s.mode != MotionMode::Ground {
            airborne = true;
            ticks += 1;
            fastest = fastest.max(-s.velocity.y);
            if feet(&app, e).x > 8.0 {
                top = top.max(feet(&app, e).y);
            }
        } else if airborne {
            break;
        }
    }
    assert_eq!(state(&app, e).mode, MotionMode::Ground, "landed");
    assert_eq!(seen(&app).landed.len(), 1);
    (feet(&app, e), ticks, fastest, top)
}

#[test]
fn gliding_falls_slower_and_lands_farther() {
    let (plain_at, plain_ticks, plain_fastest, _) = fly(false, false);
    let (glide_at, glide_ticks, glide_fastest, _) = fly(true, false);
    let p = params();
    assert!(glide_at.x > plain_at.x + 20.0, "glide {glide_at} vs fall {plain_at}");
    assert!(glide_ticks > plain_ticks * 3, "glide {glide_ticks} ticks vs fall {plain_ticks}");
    assert!(glide_fastest <= p.glide_fall_speed + 1e-3, "{glide_fastest}");
    assert!(plain_fastest > 15.0, "{plain_fastest}");
}

#[test]
fn an_updraft_lifts_a_glider() {
    let (still_air, ..) = fly(true, false);
    let (lifted, _, _, top) = fly(true, true);
    assert!(top > 20.0, "rose above the cliff top inside the updraft: {top}");
    assert!(lifted.x > still_air.x + 10.0, "flew farther: {lifted} vs {still_air}");
}

#[test]
fn releasing_glide_falls_again() {
    let mut app = app();
    cliff(app.world_mut());
    let e = hero(app.world_mut(), Vec3::new(-1.0, 20.0, 0.0));
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, glide: true, ..default() });
    step(&mut app, 60);
    assert_eq!(state(&app, e).mode, MotionMode::Glide);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, glide: false, ..default() });
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    assert!(state(&app, e).velocity.y < -5.0);
}

fn two_walk_into_each_other() -> Vec<(Vec3, Quat, MotionState)> {
    let mut app = app();
    floor(app.world_mut());
    let a = hero(app.world_mut(), Vec3::new(-2.0, 0.0, 0.0));
    let b = hero(app.world_mut(), Vec3::new(2.0, 0.0, 0.1));
    walk(app.world_mut(), a, Vec3::X);
    walk(app.world_mut(), b, Vec3::NEG_X);
    let mut out = Vec::new();
    for _ in 0..120 {
        step(&mut app, 1);
        let (pa, pb) = (feet(&app, a), feet(&app, b));
        let gap = Vec3::new(pb.x - pa.x, 0.0, pb.z - pa.z).length();
        assert!(gap >= 2.0 * RADIUS - 1e-3, "overlap after separation: {gap}");
        for e in [a, b] {
            out.push((feet(&app, e), app.world().get::<Transform>(e).unwrap().rotation, state(&app, e)));
        }
    }
    out
}

#[test]
fn characters_push_each_other_apart_deterministically() {
    let first = two_walk_into_each_other();
    let second = two_walk_into_each_other();
    assert_eq!(first, second);
    // They slid past each other thanks to the small offset.
    let (end_a, end_b) = (first[first.len() - 2].0, first[first.len() - 1].0);
    assert!(end_a.x > end_b.x, "{end_a} {end_b}");
}

#[test]
fn spawned_inside_a_wall_is_pushed_out() {
    let mut app = app();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(0.0, 0.0, -5.0), Vec3::new(1.0, 3.0, 5.0));
    let e = hero(app.world_mut(), Vec3::new(1.2, 0.0, 0.0)); // 0.2 m into the wall
    step(&mut app, 1);
    assert_clear(&app, e);
    assert!(feet(&app, e).x >= 1.0 + RADIUS);
}

#[test]
fn locked_bodies_only_take_external_motion_and_gravity() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    let start = feet(&app, e);
    for _ in 0..10 {
        set_input(
            app.world_mut(),
            e,
            LocomotionInput { move_dir: Vec3::Z, jump: true, locked: true, external: Vec3::X * 0.1, ..default() },
        );
        step(&mut app, 1);
        assert_eq!(app.world().get::<LocomotionInput>(e).unwrap().external, Vec3::ZERO, "external is consumed");
    }
    let moved = feet(&app, e) - start;
    assert!((moved - Vec3::X).length() < 1e-4, "only the dodge moved it: {moved}");
    assert!(seen(&app).jumped.is_empty());
}

#[test]
fn faces_the_given_direction_at_the_turn_speed() {
    let mut app = app();
    floor(app.world_mut());
    let e = hero(app.world_mut(), Vec3::ZERO);
    // Default facing is -Z; turn to +Z (180°) at 720°/s: a quarter second.
    set_input(app.world_mut(), e, LocomotionInput { face: Some(Vec3::Z), ..default() });
    step(&mut app, 7);
    let forward = *app.world().get::<Transform>(e).unwrap().forward();
    assert!(forward.dot(Vec3::Z) < 0.99, "still turning: {forward}");
    step(&mut app, 10);
    let forward = *app.world().get::<Transform>(e).unwrap().forward();
    assert!(forward.dot(Vec3::Z) > 0.9999, "{forward}");
}

#[test]
fn controller_systems_have_a_total_order() {
    let mut app = app();
    let strict = ScheduleBuildSettings { ambiguity_detection: LogLevel::Error, ..default() };
    let result = app.world_mut().schedule_scope(FixedUpdate, |world, schedule| {
        schedule.set_build_settings(strict);
        schedule.initialize(world).map(|_| ()).map_err(|e| e.to_string(schedule.graph(), world))
    });
    if let Err(e) = result {
        panic!("ambiguous FixedUpdate systems:\n{e}");
    }
}
