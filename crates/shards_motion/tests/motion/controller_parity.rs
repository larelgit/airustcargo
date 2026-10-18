//! Controller options for an exact action-RPG rulebook (C4), tick by tick: turning speed by who
//! names the facing, the brake and the spring at water entry, the climb-out facing test and
//! its own duration. Every option is off by default; the last test pins that.

use std::f32::consts::FRAC_PI_2;

use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

/// What the test's source adds to every character's modifiers each tick.
#[derive(Resource, Default, Clone, Copy)]
struct Source(Option<fn(&mut MoveModifiers)>);

fn modded() -> App {
    let mut app = app();
    app.init_resource::<Source>().add_systems(
        FixedUpdate,
        (|source: Res<Source>, mut q: Query<&mut MoveModifiers>| {
            if let Some(add) = source.0 {
                for mut m in &mut q {
                    add(&mut m);
                }
            }
        })
        .in_set(ModifierSystems::Contribute)
        .after(ModifierSystems::Wade),
    );
    app
}

fn with(app: &mut App, add: fn(&mut MoveModifiers)) {
    app.world_mut().resource_mut::<Source>().0 = Some(add);
}

/// How far the body has turned from its first facing (north), in degrees.
fn turned(app: &App, e: Entity) -> f32 {
    let (yaw, _, _) = app.world().get::<Transform>(e).unwrap().rotation.to_euler(EulerRot::YXZ);
    yaw.to_degrees().abs()
}

/// A body on a floor that is asked to face east and does not walk.
fn facing_east(app: &mut App, params: CharacterParams) -> Entity {
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::ZERO, params);
    set_input(app.world_mut(), e, LocomotionInput { face: Some(Vec3::X), ..default() });
    e
}

#[test]
fn the_locked_rate_turns_a_named_facing_and_the_free_rate_turns_a_walk() {
    // 0.5 s toward a named facing at 100 degrees per second: 50 degrees. The free rate is 720.
    let mut app = modded();
    let e = facing_east(&mut app, CharacterParams { turn_speed_deg: 720.0, locked_turn_speed_deg: 100.0, ..default() });
    step(&mut app, 30);
    assert!((turned(&app, e) - 50.0).abs() < 1.0, "{}", turned(&app, e));

    // Without a locked rate the named facing turns at the free one: a quarter turn is done at once.
    let mut app = modded();
    let e = facing_east(&mut app, CharacterParams { turn_speed_deg: 720.0, ..default() });
    step(&mut app, 30);
    assert!((turned(&app, e) - 90.0).abs() < 1.0, "{}", turned(&app, e));

    // A walk (no named facing) turns at the free rate whatever the locked one is.
    let mut app = modded();
    floor(app.world_mut());
    let e = hero_with(
        app.world_mut(),
        Vec3::ZERO,
        CharacterParams { turn_speed_deg: 720.0, locked_turn_speed_deg: 100.0, ..default() },
    );
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 30);
    assert!((turned(&app, e) - 90.0).abs() < 1.0, "{}", turned(&app, e));
}

#[test]
fn a_stance_rate_wins_over_the_locked_rate_and_the_lowest_stance_wins() {
    let params = CharacterParams { turn_speed_deg: 720.0, locked_turn_speed_deg: 100.0, ..default() };
    let mut app = modded();
    let e = facing_east(&mut app, params.clone());
    with(&mut app, |m| {
        m.turn_at(60.0);
    });
    step(&mut app, 30);
    assert!((turned(&app, e) - 30.0).abs() < 1.0, "a guard's 60 degrees per second: {}", turned(&app, e));
    assert!(app.world().get::<MoveModifiers>(e).unwrap().describe().contains("turn 60deg/s"));

    let mut app = modded();
    let e = facing_east(&mut app, params);
    with(&mut app, |m| {
        m.turn_at(90.0).turn_at(40.0).turn_at(70.0);
    });
    step(&mut app, 30);
    assert!((turned(&app, e) - 20.0).abs() < 1.0, "the lowest source: {}", turned(&app, e));
}

/// A deck, a pool 5 m deep to x = 4 (water up to y = 0), and a deck 0.3 m above the water on the far side.
fn pond(app: &mut App, params: CharacterParams, at: Vec3) -> Entity {
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, 0.3, 20.0));
    volume(app.world_mut(), Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
    hero_with(app.world_mut(), at, params)
}

#[test]
fn a_swimmer_turns_at_the_swim_rate_whatever_else_asks() {
    let mut app = modded();
    let params = CharacterParams { turn_speed_deg: 720.0, locked_turn_speed_deg: 400.0, swim_turn_speed_deg: 50.0, ..default() };
    let e = pond(&mut app, params, Vec3::new(-10.0, -1.1, 0.0));
    set_input(app.world_mut(), e, LocomotionInput { face: Some(Vec3::X), ..default() });
    with(&mut app, |m| {
        m.turn_at(300.0);
    });
    step(&mut app, 30);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    // Half a second at 50 degrees per second (a tick or two of the fall into the water ago
    // at the faster rate may add a little).
    assert!(turned(&app, e) < 50.0 && turned(&app, e) > 24.0, "{}", turned(&app, e));
}

/// A body falling toward the pool at `drop` m above the water with `speed` m/s sideways, in the
/// tick the water takes it: its velocity then.
fn at_water_entry(params: CharacterParams, drop: f32, speed: f32) -> (App, Entity, Vec3) {
    let mut app = modded();
    let e = pond(&mut app, params, Vec3::new(-10.0, drop, 0.0));
    step(&mut app, 1);
    app.world_mut().get_mut::<MotionState>(e).unwrap().velocity = Vec3::new(speed, 0.0, 0.0);
    for _ in 0..240 {
        step(&mut app, 1);
        if state(&app, e).mode == MotionMode::Water {
            let v = state(&app, e).velocity;
            return (app, e, v);
        }
    }
    panic!("never reached the water");
}

#[test]
fn the_entry_brake_scales_the_horizontal_speed_once() {
    let (_, _, free) = at_water_entry(CharacterParams::default(), 2.0, 8.0);
    assert!(free.x > 7.9, "no brake by default: {free}");

    let (mut app, e, braked) = at_water_entry(CharacterParams { entry_max_speed: 4.0, ..default() }, 2.0, 8.0);
    assert!((braked.x - 4.0).abs() < 1e-3 && braked.z.abs() < 1e-3, "capped to 4 m/s: {braked}");
    // The direction is kept and the brake does not hold the swimmer to 4 afterwards.
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::Z, ..default() });
    step(&mut app, 60);
    let v = state(&app, e).velocity;
    assert!((Vec2::new(v.x, v.z).length() - 3.0).abs() < 0.05, "the swim speed rules after the entry: {v}");

    // Slower than the cap: untouched.
    let (_, _, slow) = at_water_entry(CharacterParams { entry_max_speed: 4.0, ..default() }, 2.0, 2.5);
    assert!((slow.x - 2.5).abs() < 1e-3, "{slow}");
}

#[test]
fn a_spring_lets_a_dive_plunge_past_the_float_line_and_settle_on_it() {
    let params = CharacterParams {
        buoyancy_omega: 4.0,
        water_max_vertical_speed: 7.0,
        water_fall_drag: 3.0,
        ..default()
    };
    let float = -params.float_depth;
    // A 5 m dive: the first-order drift stops at the float line at once, the spring overshoots it.
    let mut deepest = [0.0f32; 2];
    for (i, p) in [CharacterParams::default(), params.clone()].into_iter().enumerate() {
        let mut app = modded();
        let e = pond(&mut app, p, Vec3::new(-10.0, 5.0, 0.0));
        let mut lowest = f32::MAX;
        let mut fastest = 0.0f32;
        for _ in 0..360 {
            step(&mut app, 1);
            lowest = lowest.min(feet(&app, e).y);
            if state(&app, e).mode == MotionMode::Water {
                fastest = fastest.max(state(&app, e).velocity.y.abs());
            }
        }
        deepest[i] = lowest;
        assert_eq!(state(&app, e).mode, MotionMode::Water);
        assert!((feet(&app, e).y - float).abs() < 0.06, "settles on the float line: {} vs {float}", feet(&app, e).y);
        if i == 1 {
            assert!(fastest <= 7.0 + 1e-3, "the vertical limit holds: {fastest}");
        }
    }
    assert!(deepest[1] < float - 0.25, "the dive plunges past the float line: {} vs {float}", deepest[1]);
    assert!(deepest[1] < deepest[0] - 0.2, "deeper than the first-order drift: {deepest:?}");
}

#[test]
fn climbing_out_needs_the_facing_the_params_ask_for() {
    // The deck is 0.3 m above the water; a swimmer pushes east at it from 0.6 m away. The body
    // starts facing north and swims in, turning slowly (10 degrees per second).
    for (min_facing, climbs) in [(None, true), (Some(0.5), false)] {
        let mut app = modded();
        let params = CharacterParams { swim_turn_speed_deg: 10.0, climb_out_min_facing: min_facing, ..default() };
        let e = pond(&mut app, params, Vec3::new(3.4, -1.1, 0.0));
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 90);
        assert_eq!(!seen(&app).mantled.is_empty(), climbs, "min facing {min_facing:?}");
        if !climbs {
            assert_eq!(state(&app, e).mode, MotionMode::Water, "it keeps swimming, in control");
        }
    }
    // Facing the ledge from the start, the same test passes at once.
    let mut app = modded();
    let params = CharacterParams { swim_turn_speed_deg: 10.0, climb_out_min_facing: Some(0.5), ..default() };
    let e = pond(&mut app, params, Vec3::new(3.4, -1.1, 0.0));
    app.world_mut().get_mut::<Transform>(e).unwrap().rotation = Quat::from_rotation_y(-FRAC_PI_2);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 90);
    assert_eq!(seen(&app).mantled.len(), 1);
}

#[test]
fn a_jump_with_the_back_to_the_ledge_does_not_climb_and_does_not_hang() {
    let mut app = modded();
    let params = CharacterParams { swim_turn_speed_deg: 10.0, climb_out_min_facing: Some(0.5), ..default() };
    let e = pond(&mut app, params, Vec3::new(3.4, -1.1, 0.0));
    // Facing away from the ledge (west) and asking for east.
    app.world_mut().get_mut::<Transform>(e).unwrap().rotation = Quat::from_rotation_y(FRAC_PI_2);
    step(&mut app, 5);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, jump: true, ..default() });
    step(&mut app, 10);
    assert!(seen(&app).mantled.is_empty());
    // Nothing is stuck: the swimmer still answers its input.
    set_input(app.world_mut(), e, LocomotionInput { move_dir: -Vec3::X, ..default() });
    step(&mut app, 60);
    assert_eq!(state(&app, e).mode, MotionMode::Water);
    assert!(feet(&app, e).x < 3.0, "{}", feet(&app, e));
}

/// Ticks a climb takes from the push into a ledge until the body stands on it.
fn climb_ticks(app: &mut App, e: Entity) -> u64 {
    let mut ticks = 0;
    let mut climbing = false;
    for _ in 0..240 {
        step(app, 1);
        let mode = state(app, e).mode;
        if mode == MotionMode::Mantle {
            climbing = true;
            ticks += 1;
        } else if climbing {
            return ticks;
        }
    }
    panic!("no climb finished");
}

#[test]
fn a_climb_out_has_its_own_duration_and_a_land_mantle_keeps_the_mantle_time() {
    for (out, want) in [(None, 24), (Some(0.7), 42)] {
        let mut app = modded();
        let params = CharacterParams { mantle_time: 0.4, climb_out_time: out, ..default() };
        let e = pond(&mut app, params, Vec3::new(3.4, -1.1, 0.0));
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        let ticks = climb_ticks(&mut app, e);
        assert!((ticks as i64 - want).abs() <= 2, "climb_out_time {out:?}: {ticks} ticks");
    }
    // A 1 m block on dry ground: climb_out_time does not apply.
    let mut app = modded();
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(2.0, 0.0, -5.0), Vec3::new(4.0, 1.0, 5.0));
    let params = CharacterParams { mantle_time: 0.4, climb_out_time: Some(0.7), ..default() };
    let e = hero_with(app.world_mut(), Vec3::new(0.0, 0.0, 0.0), params);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    let ticks = climb_ticks(&mut app, e);
    assert!((ticks as i64 - 24).abs() <= 2, "a land climb: {ticks} ticks");
}

#[test]
fn a_body_pressed_against_a_wall_turns_to_it_only_when_asked_to() {
    // A wall 3 m east; the body faces north and pushes east. The wall holds its velocity at zero.
    for (toward, faces) in [(false, false), (true, true)] {
        let mut app = modded();
        floor(app.world_mut());
        block(app.world_mut(), Vec3::new(3.0, 0.0, -10.0), Vec3::new(4.0, 5.0, 10.0));
        let params = CharacterParams { mantle_height: 0.4, turn_toward_input: toward, ..default() };
        let e = hero_with(app.world_mut(), Vec3::new(2.0, 0.0, 0.0), params);
        // Come in sideways along the wall, then push into it.
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 90);
        let (yaw, _, _) = app.world().get::<Transform>(e).unwrap().rotation.to_euler(EulerRot::YXZ);
        let east = (yaw.to_degrees() + 90.0).abs() < 1.0;
        // Moving east, the velocity points east until the wall stops it; either way the body faced
        // east on the way in, so turn it north again by hand and push on.
        app.world_mut().get_mut::<Transform>(e).unwrap().rotation = Quat::IDENTITY;
        step(&mut app, 60);
        let (yaw, _, _) = app.world().get::<Transform>(e).unwrap().rotation.to_euler(EulerRot::YXZ);
        assert!(east, "it faced east while it ran in");
        assert_eq!((yaw.to_degrees() + 90.0).abs() < 1.0, faces, "toward input {toward}: yaw {yaw}");
    }
}

#[test]
fn the_new_options_are_off_by_default_and_unserialized_when_off() {
    let json = serde_json::to_value(CharacterParams::default()).unwrap();
    for field in [
        "locked_turn_speed_deg",
        "turn_toward_input",
        "entry_max_speed",
        "buoyancy_omega",
        "water_max_vertical_speed",
        "water_fall_drag",
        "climb_out_min_facing",
        "climb_out_time",
    ] {
        assert!(json.get(field).is_none(), "{field} serialized by default");
    }
    let on = CharacterParams { buoyancy_omega: 4.0, climb_out_time: Some(0.7), ..default() };
    let json = serde_json::to_value(&on).unwrap();
    assert!(json.get("buoyancy_omega").is_some() && json.get("climb_out_time").is_some());
    assert!(MoveModifiers::default().is_neutral() && MoveModifiers::default().turn_speed_deg.is_none());
}

#[test]
fn mistakes_in_the_new_options_are_content_errors() {
    for (params, word) in [
        (CharacterParams { locked_turn_speed_deg: -1.0, ..default() }, "locked_turn_speed_deg"),
        (CharacterParams { entry_max_speed: f32::NAN, ..default() }, "entry_max_speed"),
        (CharacterParams { water_fall_drag: 3.0, ..default() }, "buoyancy_omega"),
        (CharacterParams { climb_out_time: Some(0.0), ..default() }, "climb_out_time"),
        (CharacterParams { climb_out_min_facing: Some(1.5), ..default() }, "climb_out_min_facing"),
    ] {
        let err = params.check().unwrap_err();
        assert!(err.contains(word), "{err}");
    }
}
