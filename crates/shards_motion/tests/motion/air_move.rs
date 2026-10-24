//! Air movement in the style of *Half-Life* (F7): the `Projected` model of `locomotion`'s `accel`, run through the
//! whole controller. The arithmetic itself is held to hand-computed numbers in `controller/accel.rs`; here are the
//! things only a body in a world shows: the run-up and the stop on the ground, a scripted strafe run at several tick
//! rates, a turn in the air that keeps its speed, the cap content can put on a strafe, a bunny hop, and the features the
//! model must leave alone (crouch, slopes, movers, the tool).

use ai_core::{AiCorePlugin, SimConfig};
use bevy::math::ops;
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{CROUCH, JUMP};
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

/// *Half-Life*'s numbers in metres (1 unit = 0.0254 m): a run of 320 units a second, a jump of 45 units
/// under 800 units a second squared.
const RUN: f32 = 320.0 * 0.0254;

/// Half-Life's model with the content's cap on the air speed.
fn projected(max_air_speed: Option<f32>) -> AccelModel {
    AccelModel::Projected {
        ground_accel: 10.0,
        air_accel: 10.0,
        air_speed_cap: 30.0 * 0.0254,
        friction: 4.0,
        stop_speed: 100.0 * 0.0254,
        max_air_speed,
    }
}

fn runner() -> CharacterParams {
    CharacterParams {
        run_speed: RUN,
        walk_speed: RUN * 0.5,
        sprint_speed: RUN,
        jump_height: 45.0 * 0.0254,
        gravity: 800.0 * 0.0254,
        accel: projected(None),
        ..default()
    }
}

/// The world of `app()` ticking at `hz`.
fn app_at(hz: f64) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AiCorePlugin {
            remote_port: None,
            crash_replays: false,
            sim: SimConfig { start_paused: true, tick_hz: hz, ..default() },
            ..default()
        })
        .add_plugins(MotionPlugin)
        .init_resource::<Seen>();
    app.finish();
    app.cleanup();
    app.update();
    app
}

/// A flat floor and a body with `params` on it, settled, in a world that ticks at `hz`.
struct Run {
    app: App,
    e: Entity,
    hz: f64,
}

impl Run {
    fn new(hz: f64, params: CharacterParams) -> Self {
        let mut app = app_at(hz);
        floor(app.world_mut());
        let e = hero_with(app.world_mut(), Vec3::ZERO, params);
        step(&mut app, 3);
        Run { app, e, hz }
    }

    /// Ticks in `seconds`.
    fn ticks(&self, seconds: f32) -> u64 {
        (f64::from(seconds) * self.hz).round() as u64
    }

    /// Runs `seconds`, calling `wish(t)` before every tick for the input at the time `t` since this call began.
    fn run(&mut self, seconds: f32, mut wish: impl FnMut(f32) -> LocomotionInput) {
        let dt = (1.0 / self.hz) as f32;
        for i in 0..self.ticks(seconds) {
            let input = wish(i as f32 * dt);
            set_input(self.app.world_mut(), self.e, input);
            step(&mut self.app, 1);
        }
    }

    fn east(&mut self, seconds: f32) {
        self.run(seconds, |_| input(Vec3::X));
    }

    fn velocity(&self) -> Vec3 {
        state(&self.app, self.e).velocity
    }

    fn speed(&self) -> f32 {
        Vec2::new(self.velocity().x, self.velocity().z).length()
    }

    fn mode(&self) -> MotionMode {
        state(&self.app, self.e).mode
    }
}

fn input(move_dir: Vec3) -> LocomotionInput {
    LocomotionInput { move_dir, ..default() }
}

/// The unit vector `angle` radians from east toward north (north is -z).
fn heading(angle: f32) -> Vec3 {
    Vec3::new(ops::cos(angle), 0.0, -ops::sin(angle))
}

#[test]
fn the_ground_reaches_the_wished_speed_and_friction_brings_it_to_rest() {
    for hz in [60.0, 120.0, 240.0] {
        let mut r = Run::new(hz, runner());
        // From rest to the run speed in a fraction of a second, and no faster than it.
        r.east(0.25);
        assert!((r.speed() - RUN).abs() < 1e-3, "{hz} Hz: {}", r.speed());
        r.east(0.5);
        assert!((r.speed() - RUN).abs() < 1e-3, "held at the wished speed: {}", r.speed());
        // Let go: 0.3 s of friction from 8.128 m/s. Down to the stop speed (2.54) takes ln(8.128 / 2.54) / 4 = 0.2907 s,
        // then 4 * 2.54 = 10.16 m/s² for the 0.0093 s left: 2.54 - 0.0945 = 2.4455 m/s, at any tick rate.
        r.run(0.3, |_| input(Vec3::ZERO));
        let after = 2.54 - 10.16 * (0.3 - ops::ln(RUN / 2.54) / 4.0);
        assert!((r.speed() - after).abs() < 0.01 && (after - 2.4455).abs() < 1e-3, "{hz} Hz: {} vs {after}", r.speed());
        // The line to rest takes 2.54 / 10.16 = 0.25 s more: at rest 0.55 s after letting go, not before 0.5.
        r.run(0.2, |_| input(Vec3::ZERO));
        assert!(r.speed() > 0.0, "{hz} Hz: still sliding at 0.5 s: {}", r.speed());
        r.run(0.06, |_| input(Vec3::ZERO));
        assert_eq!(r.velocity(), Vec3::ZERO, "{hz} Hz");
    }
}

#[test]
fn the_run_up_and_the_distance_covered_agree_at_every_tick_rate() {
    // Friction and the push act one after the other in a tick (as in the original games), so a finer tick splits the step
    // differently: the speed at 0.1 s is 7.15 m/s at 30 ticks a second, 6.86 at 60, 6.71 at 120 and 6.64 at 240, and the
    // continuous law (a push of 81 m/s² against the friction) gives 6.57. The error is first order in the tick. The run speed
    // is reached exactly at every rate, and the distance in 0.3 s is within a percent.
    let measure = |hz: f64| {
        let mut r = Run::new(hz, runner());
        r.east(0.1);
        let early = r.speed();
        r.east(0.2);
        (early, r.speed(), feet(&r.app, r.e).x)
    };
    let (early_60, speed_60, x_60) = measure(60.0);
    for hz in [30.0, 120.0, 240.0] {
        let (early, speed, x) = measure(hz);
        assert!((early - early_60).abs() / early_60 < 0.06, "{hz} Hz: {early} m/s at 0.1 s vs {early_60}");
        assert!((speed - speed_60).abs() < 1e-3, "{hz} Hz: {speed} vs {speed_60}");
        assert!((x - x_60).abs() / x_60 < 0.015, "{hz} Hz: {x} m at 0.3 s vs {x_60}");
    }
}

/// The scripted strafe run: east on the ground for 0.4 s, a jump, then in the air the wish turns from east to north in
/// 0.6 s (a function of the time, not of the body), and on landing the ground wish stays north until 1.6 s after the jump.
/// Returns where the body is (relative to the jump) and how it moves at the end.
fn strafe_run(hz: f64) -> (Vec3, Vec3) {
    let mut r = Run::new(hz, runner());
    r.east(0.4);
    let start = feet(&r.app, r.e);
    r.run(1.6, |t| LocomotionInput { jump: t == 0.0, ..input(heading((t / 0.6).min(1.0) * std::f32::consts::FRAC_PI_2)) });
    (feet(&r.app, r.e) - start, r.velocity())
}

#[test]
fn a_scripted_strafe_run_ends_in_the_same_place_at_30_60_120_and_240_ticks_a_second() {
    let (at_60, v_60) = strafe_run(60.0);
    assert!(at_60.x > 5.0 && at_60.z < -5.0, "the run goes east, then north: {at_60}");
    for hz in [30.0, 120.0, 240.0] {
        let (at, v) = strafe_run(hz);
        // The jump is the same flight whatever the tick (the controller integrates it in closed form), the push is exact for a
        // wish that is held for a tick, and the friction is the exact decay: the places agree to 3 cm after 8 m of run
        // (measured: under 1 cm) and the velocities to 2 cm/s.
        assert!(at.distance(at_60) < 0.03, "{hz} Hz: {at} vs {at_60} at 60");
        assert!((v - v_60).length() < 0.02, "{hz} Hz: {v} vs {v_60}");
    }
}

/// A jump high enough to stay up for a second and more (a peak of 3 m: 1.1 s up and down).
fn high_runner() -> CharacterParams {
    CharacterParams { jump_height: 3.0, ..runner() }
}

#[test]
fn a_wish_turned_ninety_degrees_over_a_second_in_the_air_keeps_the_speed() {
    let at = |hz: f64| {
        let mut r = Run::new(hz, high_runner());
        r.east(0.5);
        let before = r.speed();
        r.run(1.0, |t| LocomotionInput { jump: t == 0.0, ..input(heading(t * std::f32::consts::FRAC_PI_2)) });
        assert_eq!(r.mode(), MotionMode::Air, "{hz} Hz: still up after the second");
        (before, r.speed())
    };
    let (_, at_60) = at(60.0);
    for hz in [30.0, 60.0, 120.0, 240.0] {
        let (before, after) = at(hz);
        // Keeps at least 95 % of the speed it took off with (it loses none), and the ends agree to a percent.
        assert!(after >= before * 0.95, "{hz} Hz: {before} -> {after}");
        assert!((after - at_60).abs() / at_60 < 0.01, "{hz} Hz: {after} vs {at_60}");
    }
}

/// A strafe that follows the velocity: every tick the wish is a right angle to the way the body goes (to its left, north of
/// east), as a player's view turns with the strafe key held. Returns the horizontal speeds, tick by tick, and how far the
/// velocity turned in all (radians, the turns of each tick added up).
fn circle_strafe(params: CharacterParams, hz: f64, seconds: f32) -> (Vec<f32>, f32) {
    let mut r = Run::new(hz, params);
    r.east(0.5);
    let (mut speeds, mut turned, mut was) = (Vec::new(), 0.0f32, 0.0f32);
    for tick in 0..r.ticks(seconds) {
        let v = r.velocity();
        let wish = if tick == 0 { Vec3::X } else { Vec3::new(v.z, 0.0, -v.x).normalize_or_zero() };
        set_input(r.app.world_mut(), r.e, LocomotionInput { jump: tick == 0, ..input(wish) });
        step(&mut r.app, 1);
        let now = ops::atan2(-r.velocity().z, r.velocity().x);
        // The turn of a tick is small: wrap the difference of the angles into -pi..pi.
        let tau = 2.0 * std::f32::consts::PI;
        turned += (now - was + 3.0 * std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI;
        was = now;
        speeds.push(r.speed());
    }
    (speeds, turned)
}

#[test]
fn strafing_in_the_air_turns_the_body_and_can_pass_the_run_speed_unless_the_content_caps_it() {
    // A peak of 8 m: a flight of about 1.8 s to strafe in.
    let tall = CharacterParams { jump_height: 8.0, ..runner() };
    let (speeds, turned) = circle_strafe(tall.clone(), 60.0, 1.5);
    let top = speeds.iter().copied().fold(0.0f32, f32::max);
    assert!(top > RUN * 1.15, "no cap: the strafe gains speed, {top} against the run speed {RUN}");
    // A strafe never loses speed on the way (the wish is at a right angle to the velocity): the gain is the point.
    assert!(speeds.windows(2).all(|w| w[1] >= w[0] - 1e-4), "{speeds:?}");
    // It turns the body round, and hard: Half-Life's numbers at 60 ticks a second nudge it by the cap, 0.76 m/s, a tick.
    assert!(turned.to_degrees() > 300.0, "the velocity went round {} degrees", turned.to_degrees());

    // With the content's cap the strafe turns the body as before and gains nothing above the run speed.
    let capped = CharacterParams { accel: projected(Some(RUN)), ..tall };
    let (speeds, turned) = circle_strafe(capped, 60.0, 1.5);
    let top = speeds.iter().copied().fold(0.0f32, f32::max);
    assert!(top <= RUN + 1e-3, "held to the cap: {top}");
    assert!(turned.to_degrees() > 300.0, "and it still turns: {} degrees", turned.to_degrees());
}

#[test]
fn a_bunny_hop_keeps_the_speed_the_air_gave_and_a_landing_without_a_press_pays_the_friction() {
    // The body comes in at 10 m/s east (above its run speed of 8.13, as after a strafe), wishing to go east, and jumps at once.
    let hop = |press_again: bool| {
        let mut r = Run::new(60.0, runner());
        r.app.world_mut().get_mut::<MotionState>(r.e).unwrap().velocity = Vec3::X * 10.0;
        let (mut landings, mut last_mode) = (0, MotionMode::Ground);
        for tick in 0..240 {
            // A press on the first tick, and a press near the end of every flight (falling faster than 6 m/s: the last two ticks).
            let falling = r.mode() == MotionMode::Air && r.velocity().y < -6.0;
            set_input(r.app.world_mut(), r.e, LocomotionInput { jump: tick == 0 || (press_again && falling), ..input(Vec3::X) });
            step(&mut r.app, 1);
            landings += usize::from(last_mode == MotionMode::Air && r.mode() == MotionMode::Ground);
            last_mode = r.mode();
        }
        (landings, r.speed())
    };
    let (landings, speed) = hop(true);
    assert!(landings >= 5, "five hops or more in 4 s: {landings}");
    assert!((speed - 10.0).abs() < 1e-2, "no friction was paid between hops: {speed}");
    let (landings, speed) = hop(false);
    assert_eq!(landings, 1);
    assert!((speed - RUN).abs() < 1e-3, "one landing, and friction brought it down to the wished speed: {speed}");
}

#[test]
fn crouching_caps_the_wished_ground_speed_and_a_crouch_jump_keeps_its_speed() {
    let mut app = app();
    floor(app.world_mut());
    let params = CharacterParams { crouch_height: Some(1.0), crouch_speed: Some(2.0), crouch_time: 0.2, mantle_height: 0.35, ..runner() };
    let e = player(app.world_mut(), Vec3::ZERO);
    app.world_mut().entity_mut(e).insert(params);
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::X);
    step(&mut app, 30);
    let speed = |app: &App| Vec2::new(state(app, e).velocity.x, state(app, e).velocity.z).length();
    assert!((speed(&app) - RUN).abs() < 1e-3, "{}", speed(&app));
    // Crouched, the wish is 2 m/s: friction brings the run down to it and the push holds it there.
    edit_intent(&mut app, e, |i| i.set_held(CROUCH, true));
    step(&mut app, 60);
    assert!((speed(&app) - 2.0).abs() < 1e-3, "crouched on the ground: {}", speed(&app));
    // Up again, at the run speed; then a jump with a crouch in the air: the cap is for the ground only.
    edit_intent(&mut app, e, |i| i.set_held(CROUCH, false));
    step(&mut app, 60);
    assert!((speed(&app) - RUN).abs() < 1e-3, "{}", speed(&app));
    edit_intent(&mut app, e, |i| i.press(JUMP));
    step(&mut app, 3);
    edit_intent(&mut app, e, |i| i.set_held(CROUCH, true));
    step(&mut app, 12);
    assert_eq!(state(&app, e).mode, MotionMode::Air);
    assert!((speed(&app) - RUN).abs() < 1e-3, "a crouch jump keeps its speed: {}", speed(&app));
}

/// A ramp rising toward +x at `deg`, its surface starting at ground level at x = 2.
fn ramp(world: &mut World, deg: f32) {
    let a = deg.to_radians();
    let (along, normal) = (Vec3::new(ops::cos(a), ops::sin(a), 0.0), Vec3::new(-ops::sin(a), ops::cos(a), 0.0));
    let half = Vec3::new(10.0, 0.5, 3.0);
    let center = Vec3::new(2.0, 0.0, 0.0) + along * half.x - normal * half.y;
    turned_block(world, center, half, Quat::from_rotation_z(a));
}

#[test]
fn a_slope_is_walked_up_and_down_at_the_wished_speed() {
    let mut app = app();
    floor(app.world_mut());
    ramp(app.world_mut(), 30.0);
    let e = hero_with(app.world_mut(), Vec3::ZERO, runner());
    step(&mut app, 3);
    set_input(app.world_mut(), e, input(Vec3::X));
    for _ in 0..60 {
        step(&mut app, 1);
        assert_clear(&app, e);
    }
    let end = feet(&app, e);
    assert!(end.x > 6.0 && end.y > 1.5, "walked up: {end}");
    let s = state(&app, e);
    assert_eq!(s.mode, MotionMode::Ground);
    assert!(s.ground_normal.y < 0.9, "stands on the slope");
    assert!((s.velocity.length() - RUN).abs() < 1e-3, "the wished speed up the slope: {}", s.velocity);
    // And back down, without leaving the ground.
    let before = seen(&app).modes.len();
    set_input(app.world_mut(), e, input(Vec3::NEG_X));
    for _ in 0..90 {
        step(&mut app, 1);
        assert_clear(&app, e);
    }
    assert!(feet(&app, e).x < 1.5 && state(&app, e).mode == MotionMode::Ground);
    assert_eq!(seen(&app).modes.len(), before, "no mode change: it stays on the ground down the slope");
}

const MOVERS: &str = r#"{
    "lift": (keys: [(), (at: (0.0, 4.0, 0.0))], speed: 1.2),
}"#;

#[test]
fn a_lift_carries_a_body_that_stands_on_it_and_a_walk_across_it_is_the_wished_speed_with_the_model_on() {
    let dir = temp_dir("air_move_movers");
    write(&dir, "content/test.mover.ron", MOVERS);
    let mut app = engine_app(&dir);
    floor(app.world_mut());
    let lift = app
        .world_mut()
        .spawn((
            Name::new("Lift"),
            Mover { def: "lift".into() },
            Collider::new(Shape::cuboid(Vec3::new(1.0, 0.1, 1.0))),
            Transform::from_xyz(0.0, 0.1, 0.0),
        ))
        .id();
    let body = player(app.world_mut(), Vec3::new(0.2, 0.25, -0.1));
    app.world_mut().entity_mut(body).insert(runner());
    step(&mut app, 30);
    assert!(state(&app, body).grounded, "it stands on the lift");
    assert_eq!(state(&app, body).velocity, Vec3::ZERO, "friction keeps a standing body still");
    let top = |app: &App| app.world().get::<Transform>(lift).unwrap().translation.y + 0.1;
    let rest = feet(&app, body) - Vec3::Y * top(&app);
    ok(&mut app, "mover_go", json!({"name": "lift", "key": 1}));
    for _ in 0..260 {
        step(&mut app, 1);
        let off = feet(&app, body) - Vec3::Y * top(&app);
        assert!((off.y - rest.y).abs() < 0.03, "the body keeps to the platform: {off} (at rest {rest})");
        assert!(Vec2::new(off.x - rest.x, off.z - rest.z).length() < 0.01, "and does not slide: {off}");
        assert!(state(&app, body).grounded);
    }
    assert!((feet(&app, body).y - 4.2).abs() < 0.1, "carried to the top: {}", feet(&app, body));
    // A walk across the lift is the wished speed (a half stick is the walk speed).
    edit_intent(&mut app, body, |i| i.move_dir = Vec3::Z * 0.5);
    step(&mut app, 8);
    assert!(state(&app, body).grounded);
    let speed = state(&app, body).velocity.length();
    assert!((speed - RUN * 0.5).abs() < 0.02, "a walk across the lift: {speed}");
}

#[test]
fn move_info_names_the_model_and_its_numbers_and_the_content_loads_it() {
    let dir = temp_dir("air_move_info");
    write(
        &dir,
        "content/test.locomotion.ron",
        r#"{
    "step": (run_speed: 6.0),
    "quake": (run_speed: 8.1, accel: Projected(air_accel: 12.0, max_air_speed: Some(9.5))),
    "broken": (accel: Projected(ground_accel: 0.0)),
}"#,
    );
    let mut app = engine_app(&dir);
    floor(app.world_mut());
    let errors: Vec<String> = app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.iter().any(|e| e.contains("broken") && e.contains("ground_accel must be > 0")), "{errors:?}");
    assert!(!errors.iter().any(|e| e.contains("quake") || e.contains("\"step\"")), "the good definitions load: {errors:?}");
    let e = player_with(app.world_mut(), Vec3::ZERO, "quake");
    let s = player_with(app.world_mut(), Vec3::new(5.0, 0.0, 0.0), "step");
    step(&mut app, 3);
    let id = |e: Entity| format!("e{}", app.world().get::<ai_core::AiId>(e).unwrap().0);
    let (qid, sid) = (id(e), id(s));
    let info = |app: &mut App, id: &str| -> serde_json::Value {
        let said = ok(app, "move_info", json!({"id": id}));
        serde_json::from_str(&said).unwrap_or_else(|_| panic!("{said}"))
    };
    let value = info(&mut app, &qid);
    let accel = &value["accel"];
    assert_eq!(accel["model"], "projected", "{value}");
    assert_eq!((accel["ground_accel"].as_f64(), accel["air_accel"].as_f64()), (Some(10.0), Some(12.0)));
    assert!((accel["air_speed_cap"].as_f64().unwrap() - 0.762).abs() < 1e-4);
    assert!((accel["max_air_speed"].as_f64().unwrap() - 9.5).abs() < 1e-5);
    assert_eq!((accel["friction"].as_f64(), accel["stop_speed"].as_f64().map(|v| (v * 100.0).round())), (Some(4.0), Some(254.0)));
    assert_eq!(value["speed"].as_f64(), Some(0.0));
    let plain = info(&mut app, &sid);
    assert_eq!(plain["accel"]["model"], "step");
    assert_eq!(plain["accel"]["acceleration"].as_f64(), Some(40.0));
    // The model moves the body that names it: a run for a second reaches the content's run speed.
    edit_intent(&mut app, e, |i| i.move_dir = Vec3::X);
    step(&mut app, 60);
    let after = info(&mut app, &qid);
    assert!((after["speed"].as_f64().unwrap() - 8.1).abs() < 1e-3, "{after}");
}

/// What the test's source adds to every character's modifiers each tick.
#[derive(Resource, Default, Clone, Copy)]
struct Source(Option<fn(&mut MoveModifiers)>);

#[test]
fn the_modifiers_rate_overrides_steer_nothing_under_the_model_and_the_air_drag_is_the_params() {
    let rates = |params: CharacterParams| {
        let mut app = app_at(60.0);
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
        floor(app.world_mut());
        // High in the air with 4 m/s east and no input, then 0.1 s with a source that overrides both rates.
        let e = hero_with(app.world_mut(), Vec3::new(0.0, 20.0, 0.0), params);
        step(&mut app, 1);
        app.world_mut().get_mut::<MotionState>(e).unwrap().velocity = Vec3::X * 4.0;
        app.world_mut().resource_mut::<Source>().0 = Some(|m| {
            m.decelerate_at(25.0).accelerate_at(1.0);
        });
        step(&mut app, 6);
        Vec2::new(state(&app, e).velocity.x, state(&app, e).velocity.z).length()
    };
    // `Step`: the override brakes the air (25 m/s² for 0.1 s takes 2.5 m/s off: the params' 10 would take 1).
    let step_model = rates(CharacterParams { air_deceleration: 10.0, ..default() });
    assert!((step_model - 1.5).abs() < 0.02, "{step_model}");
    // `Projected`: the drag is the params' `air_deceleration`, 10 m/s² for 0.1 s.
    let projected = rates(CharacterParams { air_deceleration: 10.0, accel: projected(None), ..default() });
    assert!((projected - 3.0).abs() < 0.02, "{projected}");

    // On the ground the override of the speed-up changes nothing under the model: the run speed in the same 0.25 s.
    let mut app = app_at(60.0);
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
    floor(app.world_mut());
    let e = hero_with(app.world_mut(), Vec3::ZERO, runner());
    app.world_mut().resource_mut::<Source>().0 = Some(|m| {
        m.accelerate_at(1.0);
    });
    step(&mut app, 3);
    set_input(app.world_mut(), e, input(Vec3::X));
    step(&mut app, 15);
    assert!((state(&app, e).velocity.x - RUN).abs() < 1e-3, "{}", state(&app, e).velocity);
}
