//! Options that let a game reproduce another engine's movement rules to the tick (C1b): a
//! surface-relative jump climb-out, analog caps and sprints, stance speeds, per-axis and
//! per-stance acceleration, braking in the air, the wading reference point. Every one is off by
//! default; here each is on, tick by tick.

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

fn velocity(app: &App, e: Entity) -> Vec3 {
    let v = state(app, e).velocity;
    Vec3::new(v.x, 0.0, v.z)
}

fn speed_of(app: &App, e: Entity) -> f32 {
    velocity(app, e).length()
}

fn on_the_run(params: CharacterParams) -> (App, Entity) {
    let mut app = modded();
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::ZERO, params);
    (app, e)
}

/// A pool to x = 4 (water up to y = 0) and a deck on the far side with its top `deck_top` over
/// the water; the swimmer floats 0.6 m from the deck.
fn pond_deck(app: &mut App, params: CharacterParams, deck_top: f32) -> Entity {
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, deck_top, 20.0));
    volume(app.world_mut(), Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
    let e = hero_with(app.world_mut(), Vec3::new(3.4, -1.1, 0.0), params);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    e
}

#[test]
fn the_jump_climb_out_limit_is_measured_from_the_water_surface() {
    // The feet float 1.1 m under the surface, so the plain reach of 1.6 m above the feet is only
    // 0.5 m over the water. With a surface-relative limit of 1.3 m a jump reaches a deck 1.2 m over
    // the water, and not one 1.4 m over it.
    for (deck, limit, climbs) in [(1.2, None, false), (1.2, Some(1.3), true), (1.4, Some(1.3), false), (0.4, None, true)] {
        let mut app = modded();
        let e = pond_deck(&mut app, CharacterParams { climb_out_max_height: limit, ..default() }, deck);
        step(&mut app, 10);
        press_jump(app.world_mut(), e);
        step(&mut app, 60);
        assert_eq!(!seen(&app).mantled.is_empty(), climbs, "deck {deck} limit {limit:?}");
        if climbs {
            assert_eq!(state(&app, e).mode, MotionMode::Ground);
            assert!(feet(&app, e).y > deck - 0.05, "{}", feet(&app, e));
        }
    }
}

#[test]
fn the_jump_limit_does_not_make_pushing_climb_higher() {
    let mut app = modded();
    let e = pond_deck(&mut app, CharacterParams { climb_out_max_height: Some(1.3), ..default() }, 1.2);
    step(&mut app, 90);
    assert!(seen(&app).mantled.is_empty(), "only a jump climbs a high ledge");
    assert_eq!(state(&app, e).mode, MotionMode::Water);
}

#[test]
fn the_jump_limit_is_one_rule_for_the_tool_the_clearance_and_the_controller() {
    let p = CharacterParams { climb_out_max_height: Some(1.3), ..default() };
    // A rise is counted from the height the feet float at: 1.1 m under the surface.
    assert!(p.can_jump_out(1.1 + 1.3) && !p.can_jump_out(1.1 + 1.32));
    let plain = CharacterParams::default();
    assert!(plain.can_jump_out(1.5) && !plain.can_jump_out(1.6), "without a limit, the ordinary swimmer's rule");
}

#[test]
fn caps_and_sprints_can_scale_with_the_input() {
    // A lock-on cap of 4.5 on a run of 5.0 (a walk of 2.5 makes the speed linear), a stick pushed to 0.7: 3.5 under an absolute cap
    // (the cap never bites), 3.15 when the cap is the speed at full input.
    for (scale, want) in [(false, 3.5), (true, 3.15)] {
        let (mut app, e) = on_the_run(CharacterParams { walk_speed: 2.5, caps_scale_with_input: scale, ..default() });
        with(&mut app, |m| {
            m.cap_unsprinted_at(4.5);
        });
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X * 0.7, ..default() });
        step(&mut app, 60);
        assert!((speed_of(&app, e) - want).abs() < 1e-3, "scale {scale}: {}", speed_of(&app, e));
    }
    // A sprint at 0.7 of the stick: full speed above 0.5 by default, 0.7 of it when it scales.
    for (scales, want) in [(false, 8.0), (true, 8.0 * 0.7)] {
        let (mut app, e) = on_the_run(CharacterParams { sprint_speed: 8.0, sprint_scales_with_input: scales, ..default() });
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X * 0.7, sprint: true, ..default() });
        step(&mut app, 60);
        assert!((speed_of(&app, e) - want).abs() < 1e-3, "scales {scales}: {}", speed_of(&app, e));
        assert!(state(&app, e).sprinting);
    }
    // ...and a light touch (0.3) is a sprint only when it scales.
    for (scales, sprinting) in [(false, false), (true, true)] {
        let (mut app, e) = on_the_run(CharacterParams { sprint_speed: 8.0, sprint_scales_with_input: scales, ..default() });
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X * 0.3, sprint: true, ..default() });
        step(&mut app, 60);
        assert_eq!(state(&app, e).sprinting, sprinting, "scales {scales}");
    }
}

#[test]
fn a_stance_speed_ignores_the_slow_and_the_pace_but_not_the_water() {
    let (mut app, e) = on_the_run(CharacterParams::default());
    // A guard walk: 3.0 at a full stick. The status slow and the combat pace do not touch it.
    with(&mut app, |m| {
        m.slow(0.5).slow_on_ground(0.85).stance_speed(3.0, true);
    });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X * 0.7, sprint: true, ..default() });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 2.1).abs() < 1e-3, "3.0 x 0.7: {}", speed_of(&app, e));
    assert!(!state(&app, e).sprinting, "a stance does not sprint");
    // Wading does scale it.
    with(&mut app, |m| {
        m.slow(0.5).slow_on_ground(0.85).wade_by(0.8).stance_speed(3.0, true);
    });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 2.4).abs() < 1e-3, "{}", speed_of(&app, e));
    // A flat stance ignores how far the stick is pushed (a drinking walk).
    with(&mut app, |m| {
        m.stance_speed(2.5, false);
    });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X * 0.2, ..default() });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 2.5).abs() < 1e-3, "{}", speed_of(&app, e));
    // The same walk without the stance is slowed by all of it.
    with(&mut app, |m| {
        m.slow(0.5).slow_on_ground(0.85).wade_by(0.8);
    });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 5.0 * 0.5 * 0.85 * 0.8).abs() < 1e-3, "{}", speed_of(&app, e));
}

#[test]
fn acceleration_steps_per_axis_when_asked() {
    // Running +X at 5 m/s, then the stick swings to +Z. 6 ticks of 25 m/s² at 60 Hz = 2.5 m/s a
    // step along the line to the new velocity, or on each axis by itself.
    let along = 2.5 * std::f32::consts::FRAC_1_SQRT_2;
    for (axis, vx, vz) in [(true, 2.5, 2.5), (false, 5.0 - along, along)] {
        let params = CharacterParams { acceleration: 25.0, deceleration: 25.0, axis_stepping: axis, ..default() };
        let (mut app, e) = on_the_run(params);
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 60);
        assert!((velocity(&app, e).x - 5.0).abs() < 1e-3);
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::Z, ..default() });
        step(&mut app, 6);
        let v = velocity(&app, e);
        assert!((v.x - vx).abs() < 0.02 && (v.z - vz).abs() < 0.02, "axis {axis}: {v}");
    }
}

#[test]
fn deceleration_can_be_for_no_input_only() {
    // Running at 5, then the stick eases to a walk of 2.5: by default that is braking (50 m/s²).
    // Three ticks later: 2.5, or 5 - 3 * 40 / 60 with the speed-up rate when only no input brakes.
    for (only_without_input, after_3_ticks) in [(false, 2.5), (true, 5.0 - 3.0 * 40.0 / 60.0)] {
        let params = CharacterParams { brake_only_without_input: only_without_input, ..default() };
        let (mut app, e) = on_the_run(params);
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 60);
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X * 0.5, ..default() });
        step(&mut app, 3);
        assert!((speed_of(&app, e) - after_3_ticks).abs() < 0.01, "{only_without_input}: {}", speed_of(&app, e));
    }
}

#[test]
fn a_modifier_can_override_the_rates_for_a_stance() {
    let (mut app, e) = on_the_run(CharacterParams::default());
    with(&mut app, |m| {
        m.accelerate_at(10.0).decelerate_at(20.0);
    });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 12); // 0.2 s at 10 m/s²
    assert!((speed_of(&app, e) - 2.0).abs() < 0.01, "{}", speed_of(&app, e));
    set_input(app.world_mut(), e, LocomotionInput::default());
    step(&mut app, 6); // 0.1 s at 20 m/s²
    assert!(speed_of(&app, e).abs() < 0.01, "{}", speed_of(&app, e));
    let m = *app.world().get::<MoveModifiers>(e).unwrap();
    assert!(m.describe().contains("accel 10.0") && m.describe().contains("decel 20.0"), "{}", m.describe());
}

#[test]
fn the_air_keeps_its_speed_unless_the_params_brake_it() {
    for (air_deceleration, left) in [(0.0, 4.0), (10.0, 2.0)] {
        let mut app = modded();
        floor(app.world_mut());
        let e = hero_with(app.world_mut(), Vec3::new(0.0, 20.0, 0.0), CharacterParams { air_deceleration, ..default() });
        step(&mut app, 1);
        app.world_mut().get_mut::<MotionState>(e).unwrap().velocity = Vec3::X * 4.0;
        step(&mut app, 12); // 0.2 s: 2 m/s of braking at 10 m/s²
        assert_eq!(state(&app, e).mode, MotionMode::Air);
        assert!((velocity(&app, e).x - left).abs() < 0.02, "{air_deceleration}: {}", velocity(&app, e));
    }
}

#[test]
fn a_modifier_rate_also_brakes_the_air() {
    let mut app = modded();
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::new(0.0, 20.0, 0.0), CharacterParams { air_deceleration: 10.0, ..default() });
    step(&mut app, 1);
    app.world_mut().get_mut::<MotionState>(e).unwrap().velocity = Vec3::X * 4.0;
    with(&mut app, |m| {
        m.decelerate_at(25.0);
    });
    step(&mut app, 6); // 0.1 s at 25 m/s²
    assert!((velocity(&app, e).x - 1.5).abs() < 0.02, "{}", velocity(&app, e));
}

#[test]
fn wading_can_read_a_lower_reference_point_and_work_in_the_air() {
    // Water 0.15 m over the floor: the feet (a skin above it) are 0.13 m deep, under the 0.15 minimum.
    let shallow = |offset: f32| {
        let mut app = modded();
        floor(app.world_mut());
        volume(app.world_mut(), Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 0.15, 50.0), VolumeKind::Water);
        let params = CharacterParams {
            wade_min_depth: 0.15,
            wade_full_depth: 0.45,
            wade_factor: 0.75,
            wade_depth_offset: offset,
            ..default()
        };
        let e = hero_with(app.world_mut(), Vec3::ZERO, params);
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 60);
        (app, e)
    };
    let (app, e) = shallow(0.0);
    assert!((speed_of(&app, e) - 5.0).abs() < 1e-3, "dry boots: {}", speed_of(&app, e));
    let (app, e) = shallow(0.12);
    let m = app.world().get::<MoveModifiers>(e).unwrap();
    assert!(m.wade < 1.0 && speed_of(&app, e) < 5.0, "measured from 0.12 m under the feet: {}", speed_of(&app, e));

    // In the air over water: slowed only with `wade_in_air`.
    for (in_air, wades) in [(false, false), (true, true)] {
        let mut app = modded();
        floor(app.world_mut());
        volume(app.world_mut(), Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 10.0, 50.0), VolumeKind::Water);
        let params = CharacterParams {
            swim_depth: 30.0,
            wade_min_depth: 0.15,
            wade_full_depth: 0.45,
            wade_factor: 0.75,
            wade_in_air: in_air,
            ..default()
        };
        let e = hero_with(app.world_mut(), Vec3::new(0.0, 6.0, 0.0), params);
        step(&mut app, 3);
        assert_eq!(state(&app, e).mode, MotionMode::Air);
        assert_eq!(app.world().get::<MoveModifiers>(e).unwrap().wade < 1.0, wades, "wade_in_air {in_air}");
    }
}

#[test]
fn the_wade_depth_adds_the_offset_and_never_goes_negative() {
    let p = CharacterParams { wade_depth_offset: 0.12, ..default() };
    assert!((p.wade_depth(1.0, 0.9) - 0.22).abs() < 1e-6);
    assert!((p.wade_depth(1.0, 1.1) - 0.02).abs() < 1e-6, "a little above the surface still counts the offset");
    assert_eq!(p.wade_depth(1.0, 2.0), 0.0);
    assert!((CharacterParams::default().wade_depth(1.0, 0.9) - 0.1).abs() < 1e-6);
}

#[test]
fn the_new_options_are_checked_and_default_ones_do_not_serialize() {
    let bad = CharacterParams {
        climb_out_max_height: Some(1.0),
        climb_out_auto_height: Some(1.2),
        mantle_height: 2.0,
        swim_fast_resume: SprintResume::Fraction(2.0),
        air_deceleration: -1.0,
        ..default()
    };
    let err = bad.check().unwrap_err();
    for word in ["climb_out_auto_height", "swim_fast_resume", "air_deceleration"] {
        assert!(err.contains(word), "{err}");
    }
    let json = serde_json::to_value(CharacterParams::default()).unwrap();
    for field in [
        "climb_out_max_height",
        "climb_out_reach",
        "wade_air_depth_offset",
        "sprint_in_air",
        "swim_fast_resume",
        "wade_depth_offset",
        "wade_in_air",
        "sprint_scales_with_input",
        "caps_scale_with_input",
        "axis_stepping",
        "brake_only_without_input",
        "air_deceleration",
    ] {
        assert!(json.get(field).is_none(), "{field} serialized by default");
    }
    let p: CharacterParams = serde_json::from_str(r#"{"swim_fast_resume": {"Fraction": 0.25}, "axis_stepping": true}"#).unwrap();
    assert_eq!(p.swim_fast_resume, SprintResume::Fraction(0.25));
    assert!(p.axis_stepping);
}

#[test]
fn the_water_climb_out_reach_is_a_parameter() {
    // A swimmer 0.9 m off a deck that only a jump climbs (1.2 m over the water, limit 1.3): the
    // default reach of 0.3 m does nothing, a reach of 1.0 m climbs out.
    for (reach, climbs) in [(None, false), (Some(1.0), true), (Some(0.3), false)] {
        let mut app = modded();
        let params = CharacterParams { climb_out_max_height: Some(1.3), climb_out_reach: reach, ..default() };
        let e = pond_deck(&mut app, params, 1.2);
        app.world_mut().get_mut::<Transform>(e).unwrap().translation.x = 2.7;
        step(&mut app, 1);
        assert_eq!(state(&app, e).mode, MotionMode::Water);
        press_jump(app.world_mut(), e);
        step(&mut app, 60);
        assert_eq!(!seen(&app).mantled.is_empty(), climbs, "reach {reach:?}");
    }
}

#[test]
fn the_climb_reach_defaults_to_the_old_constant() {
    assert_eq!(CharacterParams::default().climb_reach(), 0.3);
    assert_eq!(CharacterParams { climb_out_reach: Some(0.55), ..default() }.climb_reach(), 0.55);
}

/// Hold sprint on the ground, jump, then change the button in the air.
fn sprint_jump(sprint_in_air: bool, hold_at_start: bool, hold_in_air: bool) -> (bool, bool) {
    let (mut app, e) = on_the_run(CharacterParams { sprint_in_air, ..default() });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: hold_at_start, ..default() });
    step(&mut app, 40);
    let on_ground = state(&app, e).sprinting;
    press_jump(app.world_mut(), e);
    step(&mut app, 3);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: hold_in_air, ..default() });
    step(&mut app, 3);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    (on_ground, state(&app, e).sprinting)
}

#[test]
fn sprint_can_be_decided_every_tick_in_the_air() {
    // Default: the flag is the take-off one for the whole jump, pressed or released in the air.
    assert_eq!(sprint_jump(false, true, false), (true, true));
    assert_eq!(sprint_jump(false, false, true), (false, false));
    // With the option: releasing in the air ends the sprint, pressing there starts it.
    assert_eq!(sprint_jump(true, true, false), (true, false));
    assert_eq!(sprint_jump(true, false, true), (false, true));
    assert_eq!(sprint_jump(true, true, true), (true, true));
}

#[test]
fn a_glide_is_no_sprint() {
    let (mut app, e) = on_the_run(CharacterParams { sprint_in_air: true, ..default() });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: true, ..default() });
    step(&mut app, 40);
    press_jump(app.world_mut(), e);
    step(&mut app, 30);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: true, glide: true, ..default() });
    step(&mut app, 5);
    assert_eq!(state(&app, e).mode, MotionMode::Glide);
    assert!(!state(&app, e).sprinting);
}

#[test]
fn the_airborne_wade_offset_can_differ_from_the_standing_one() {
    let p = CharacterParams { wade_depth_offset: 0.12, wade_air_depth_offset: Some(0.10), ..default() };
    assert!((p.wade_depth_for(MotionMode::Ground, 1.0, 0.9) - 0.22).abs() < 1e-6);
    assert!((p.wade_depth_for(MotionMode::Air, 1.0, 0.9) - 0.20).abs() < 1e-6);
    assert!((p.wade_depth_for(MotionMode::Glide, 1.0, 0.9) - 0.20).abs() < 1e-6);
    let same = CharacterParams { wade_depth_offset: 0.12, ..default() };
    assert_eq!(same.wade_depth_for(MotionMode::Air, 1.0, 0.9), same.wade_depth(1.0, 0.9));
    assert!(CharacterParams { wade_air_depth_offset: Some(-0.1), ..default() }.check().is_err());
}
