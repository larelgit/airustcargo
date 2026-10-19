//! Locomotion modifiers (C1): factors, caps, root, wading, fast swimming, the two-tier
//! climb-out and landing tiers, tick by tick. Neutral values take the plain path.

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

fn speed_of(app: &App, e: Entity) -> f32 {
    let v = state(app, e).velocity;
    Vec2::new(v.x, v.z).length()
}

fn running(app: &mut App, sprint: bool) -> Entity {
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::ZERO, CharacterParams { sprint_speed: 8.0, ..default() });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint, ..default() });
    e
}

#[test]
fn no_modifiers_is_the_plain_speed() {
    let mut app = modded();
    let e = running(&mut app, false);
    step(&mut app, 60);
    assert_eq!(speed_of(&app, e), 5.0);
    assert!(app.world().get::<MoveModifiers>(e).unwrap().is_neutral());
}

#[test]
fn a_slow_factor_scales_the_run_and_composes_with_the_ground_factor() {
    let mut app = modded();
    let e = running(&mut app, false);
    with(&mut app, |m| {
        m.slow(0.6);
    });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 3.0).abs() < 1e-4, "{}", speed_of(&app, e));
    with(&mut app, |m| {
        m.slow(0.5).slow_on_ground(0.5);
    });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 1.25).abs() < 1e-4, "{}", speed_of(&app, e));
    // Gone the tick the source stops.
    app.world_mut().resource_mut::<Source>().0 = None;
    step(&mut app, 60);
    assert_eq!(speed_of(&app, e), 5.0);
}

#[test]
fn caps_hold_before_the_factor_and_a_sprint_passes_the_walk_cap() {
    let mut app = modded();
    let e = running(&mut app, false);
    with(&mut app, |m| {
        m.cap_unsprinted_at(4.5).slow_on_ground(0.85);
    });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 4.5 * 0.85).abs() < 1e-4, "lock-on pace: {}", speed_of(&app, e));
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: true, ..default() });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 8.0 * 0.85).abs() < 1e-4, "a sprint passes it: {}", speed_of(&app, e));
    assert!(state(&app, e).sprinting);
    with(&mut app, |m| {
        m.cap_at(3.0).forbid_sprint();
    });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 3.0).abs() < 1e-4, "guard walk");
    assert!(!state(&app, e).sprinting, "no sprint in a guard");
}

#[test]
fn a_root_stops_walking_and_jumping_but_not_pushes_or_gravity() {
    let mut app = modded();
    let e = running(&mut app, false);
    with(&mut app, |m| {
        m.root();
    });
    step(&mut app, 30);
    assert_eq!(speed_of(&app, e), 0.0);
    assert!(feet(&app, e).x.abs() < 1e-4, "{}", feet(&app, e));
    press_jump(app.world_mut(), e);
    step(&mut app, 10);
    assert!(seen(&app).jumped.is_empty() && state(&app, e).grounded);
    // A push (a dodge, a knockback) still moves the body.
    app.world_mut().get_mut::<LocomotionInput>(e).unwrap().external = Vec3::X;
    step(&mut app, 1);
    assert!((feet(&app, e).x - 1.0).abs() < 0.05, "{}", feet(&app, e));
    // Rooted in the air, gravity still pulls.
    let hi = hero(app.world_mut(), Vec3::new(50.0, 4.0, 0.0));
    step(&mut app, 90);
    assert!(feet(&app, hi).y < 0.1 && state(&app, hi).mode == MotionMode::Ground);
}

#[test]
fn a_root_is_not_remembered_after_its_source_stops() {
    let mut app = modded();
    let e = running(&mut app, false);
    with(&mut app, |m| {
        m.root();
    });
    step(&mut app, 5);
    app.world_mut().resource_mut::<Source>().0 = None;
    step(&mut app, 60);
    assert_eq!(speed_of(&app, e), 5.0);
}

/// A deck, a pool 5 m deep to x = 4 (water up to y = 0), and a deck 0.3 m above the water
/// on the far side.
fn pond(app: &mut App, params: CharacterParams) -> Entity {
    block(app.world_mut(), Vec3::new(-20.0, -6.0, -20.0), Vec3::new(20.0, -5.0, 20.0));
    block(app.world_mut(), Vec3::new(4.0, -6.0, -20.0), Vec3::new(8.0, 0.3, 20.0));
    volume(app.world_mut(), Vec3::new(-20.0, -5.0, -20.0), Vec3::new(4.0, 0.0, 20.0), VolumeKind::Water);
    hero_with(app.world_mut(), Vec3::new(-10.0, -1.1, 0.0), params)
}

#[test]
fn fast_swimming_needs_the_params_and_a_sprint() {
    for (fast, expect, sprinting) in [(0.0, 3.0, false), (4.0, 4.0, true)] {
        let mut app = modded();
        let e = pond(&mut app, CharacterParams { swim_fast_speed: fast, ..default() });
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, sprint: true, ..default() });
        step(&mut app, 60);
        let s = state(&app, e);
        assert_eq!(s.mode, MotionMode::Water);
        assert!((speed_of(&app, e) - expect).abs() < 1e-3, "{}", speed_of(&app, e));
        assert_eq!(s.sprinting, sprinting);
    }
}

#[test]
fn a_slow_factor_slows_the_swim_but_a_cap_does_not() {
    let mut app = modded();
    let e = pond(&mut app, default());
    with(&mut app, |m| {
        m.slow(0.5).cap_at(1.0).slow_on_ground(0.1);
    });
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 60);
    assert!((speed_of(&app, e) - 1.5).abs() < 1e-3, "{}", speed_of(&app, e));
}

#[test]
fn swimmers_have_their_own_acceleration_and_deceleration() {
    let mut app = modded();
    let e = pond(&mut app, CharacterParams { swim_acceleration: 5.0, swim_deceleration: 3.0, ..default() });
    step(&mut app, 10);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 12); // 0.2 s at 5 m/s²
    assert!((speed_of(&app, e) - 1.0).abs() < 0.05, "{}", speed_of(&app, e));
    set_input(app.world_mut(), e, LocomotionInput::default());
    step(&mut app, 12); // 0.2 s at 3 m/s²
    assert!((speed_of(&app, e) - 0.4).abs() < 0.05, "{}", speed_of(&app, e));
}

#[test]
fn the_auto_climb_out_height_splits_pushing_from_jumping() {
    // The deck is 0.3 m above the water.
    for (auto, pushes) in [(None, true), (Some(0.4), true), (Some(0.2), false)] {
        let mut app = modded();
        let e = pond(&mut app, CharacterParams { climb_out_auto_height: auto, ..default() });
        app.world_mut().get_mut::<Transform>(e).unwrap().translation.x = 3.4;
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 90);
        assert_eq!(!seen(&app).mantled.is_empty(), pushes, "auto {auto:?}: pushing");
        if !pushes {
            assert_eq!(state(&app, e).mode, MotionMode::Water);
            press_jump(app.world_mut(), e);
            step(&mut app, 60);
            assert_eq!(seen(&app).mantled.len(), 1, "a jump climbs what pushing does not");
        }
    }
}

#[test]
fn wading_slows_by_depth() {
    let mut app = modded();
    floor(app.world_mut());
    volume(app.world_mut(), Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 0.5, 50.0), VolumeKind::Water);
    let params = CharacterParams { wade_min_depth: 0.1, wade_full_depth: 0.4, wade_factor: 0.5, ..default() };
    let e = hero_with(app.world_mut(), Vec3::ZERO, params);
    set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
    step(&mut app, 60);
    // Water over the feet is deeper than the full depth: the whole factor.
    assert!((speed_of(&app, e) - 2.5).abs() < 0.02, "{}", speed_of(&app, e));
    assert!(app.world().get::<MoveModifiers>(e).unwrap().ground_factor < 0.51);
}

fn landing_params() -> CharacterParams {
    CharacterParams { landing_tiers: vec![4.0, 6.5, 13.0], landing_drop_tiers: vec![6.5, 13.0], ..default() }
}

#[test]
fn landing_tiers_come_from_the_impact_speed_and_whether_a_jump_began_the_fall() {
    // A jump of 1.2 m lands at about 7 m/s: tier 2 of the jump tiers.
    let mut app = modded();
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::ZERO, landing_params());
    step(&mut app, 3);
    press_jump(app.world_mut(), e);
    step(&mut app, 80);
    let landed = seen(&app).landed.clone();
    assert_eq!(landed.len(), 1, "{landed:?}");
    assert!(landed[0].jumped && landed[0].tier == 2, "{:?}", landed[0]);
    assert!(app.world().get::<JumpedFlight>(e).is_none(), "the flight ended");

    // A 0.8 m drop lands at 5.7 m/s: tier 1 for a jump, none for a drop.
    let mut app = modded();
    floor(app.world_mut());
    hero_with(app.world_mut(), Vec3::new(0.0, 0.8, 0.0), landing_params());
    step(&mut app, 60);
    let l = seen(&app).landed[0];
    assert!(!l.jumped && l.tier == 0, "{l:?}");

    for (height, tier) in [(3.0, 1), (10.0, 2)] {
        let mut app = modded();
        floor(app.world_mut());
        hero_with(app.world_mut(), Vec3::new(0.0, height, 0.0), landing_params());
        step(&mut app, 120);
        let l = seen(&app).landed[0];
        assert_eq!((l.tier, l.jumped), (tier, false), "from {height} m at {} m/s", l.impact_speed);
    }
}

#[test]
fn without_tiers_landing_is_as_before() {
    let mut app = modded();
    floor(app.world_mut());
    hero(app.world_mut(), Vec3::new(0.0, 10.0, 0.0));
    step(&mut app, 120);
    let l = seen(&app).landed[0];
    assert_eq!((l.tier, l.jumped), (0, false));
}

#[test]
fn the_shallows_break_a_fall_when_the_limit_says_so() {
    for (limit, landings) in [(None, 1), (Some(0.15), 0), (Some(1.0), 1)] {
        let mut app = modded();
        floor(app.world_mut());
        volume(app.world_mut(), Vec3::new(-5.0, 0.0, -5.0), Vec3::new(5.0, 0.5, 5.0), VolumeKind::Water);
        let e = hero_with(app.world_mut(), Vec3::new(0.0, 5.0, 0.0), CharacterParams { landing_wade_limit: limit, ..default() });
        step(&mut app, 90);
        assert_eq!(seen(&app).landed.len(), landings, "limit {limit:?}");
        assert_eq!(state(&app, e).mode, MotionMode::Ground);
    }
}

#[test]
fn coyote_time_defaults_to_twelve_hundredths_and_is_content() {
    // The default is 0.12 s; a jump 0.10 s after the edge still works, 0.17 s does not
    // (and `coyote_time` is a field of the locomotion content, like every other number).
    assert_eq!(CharacterParams::default().coyote_time, 0.12);
    let p: CharacterParams = serde_json::from_str(r#"{"coyote_time": 0.2}"#).unwrap();
    assert_eq!(p.coyote_time, 0.2);
}

#[test]
fn new_params_are_checked_and_neutral_ones_do_not_serialize() {
    assert!(CharacterParams::default().check().is_ok());
    let bad = CharacterParams {
        landing_tiers: vec![6.5, 4.0],
        wade_factor: 0.5,
        wade_min_depth: 1.0,
        wade_full_depth: 0.2,
        swim_fast_speed: -1.0,
        ..default()
    };
    let err = bad.check().unwrap_err();
    for word in ["landing_tiers", "wade_full_depth", "swim_fast_speed"] {
        assert!(err.contains(word), "{err}");
    }
    // Content hashes (and so replay checks) see only what is set.
    let json = serde_json::to_value(CharacterParams::default()).unwrap();
    for field in ["swim_fast_speed", "wade_factor", "landing_tiers", "climb_out_auto_height", "landing_wade_limit"] {
        assert!(json.get(field).is_none(), "{field} serialized by default");
    }
    assert!(serde_json::to_value(landing_params()).unwrap().get("landing_tiers").is_some());
}

#[test]
fn likely_mistakes_in_the_new_params_are_errors() {
    for (params, word) in [
        (CharacterParams { wade_factor: 1.5, ..default() }, "wade_factor"),
        (CharacterParams { swim_fast_speed: 1.0, ..default() }, "swim_fast_speed"),
        (CharacterParams { swim_fast_drain: 5.0, ..default() }, "swim_fast_drain"),
        (CharacterParams { climb_out_auto_height: Some(9.0), ..default() }, "climb_out_auto_height"),
    ] {
        let err = params.check().unwrap_err();
        assert!(err.contains(word), "{err}");
    }
}

#[test]
fn the_auto_climb_out_height_takes_the_mantle_slack() {
    // The deck is 0.3 m above the water: a push climbs it with the limit exactly there, and not
    // with the limit more than the slack (1 mm) below it.
    for (auto, pushes) in [(0.3, true), (0.3 - 0.0005, true), (0.3 - 0.003, false)] {
        let mut app = modded();
        let e = pond(&mut app, CharacterParams { climb_out_auto_height: Some(auto), ..default() });
        app.world_mut().get_mut::<Transform>(e).unwrap().translation.x = 3.4;
        set_input(app.world_mut(), e, LocomotionInput { move_dir: Vec3::X, ..default() });
        step(&mut app, 90);
        assert_eq!(!seen(&app).mantled.is_empty(), pushes, "auto {auto}: pushing");
    }
}
