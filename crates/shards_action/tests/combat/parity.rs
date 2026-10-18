//! Controller and stamina options for an exact action-RPG rulebook (C4): a backstep, a stance's
//! turning speed, regeneration holds per source and the crawl lock, tick by tick. Each option is
//! content; the defaults are pinned by the reference replays and `golden.rs`.

use bevy::prelude::*;
use serde_json::json;
use shards_action::Stamina;
use shards_core::Intent;
use shards_motion::{
    CharacterParams, Collider, CollisionLayers, ModifierSystems, MotionMode, MotionState, MoveModifiers, Shape, SprintResume,
    Volume, VolumeKind,
};

use crate::common::*;

/// Marks a body whose sprint is free (the game's source for "out of combat").
#[derive(Component)]
struct FreeRun;

fn free_run(mut q: Query<(&mut MoveModifiers, Has<FreeRun>)>) {
    for (mut m, free) in &mut q {
        if free {
            m.scale_sprint_cost(0.0);
        }
    }
}

fn arena() -> App {
    app_with(|app| {
        app.add_systems(FixedUpdate, free_run.in_set(ModifierSystems::Contribute).after(ModifierSystems::Statuses));
    })
}

fn run_north(app: &mut App, e: Entity, sprint: bool) {
    let mut intent = app.world_mut().get_mut::<Intent>(e).unwrap();
    intent.move_dir = N;
    intent.set_held("sprint", sprint);
}

/// Sets the free-sprint hold in the combat tuning the arena loaded (`None`: the tuning without it).
fn free_sprint_hold(app: &mut App, hold: Option<f32>) {
    let Some(h) = hold else { return };
    let before = json!({"input_buffer": 0.15, "lock_on_range": 20.0, "max_statuses": 2, "sprint_stamina_per_s": 30.0});
    let mut def = before.clone();
    def["free_sprint_regen_hold"] = json!(h);
    ok(app, "content_apply", json!({"changes": [{"kind": "combat_tuning", "id": "default", "def": def, "before": before}]}));
}

/// How far the body has turned from north, in degrees.
fn turned(app: &App, e: Entity) -> f32 {
    let (yaw, _, _) = app.world().get::<Transform>(e).unwrap().rotation.to_euler(EulerRot::YXZ);
    yaw.to_degrees().abs()
}

// --- backstep -------------------------------------------------------------------------------

#[test]
fn a_dodge_with_nothing_pressed_goes_backward_and_one_with_input_goes_there() {
    // Facing north: the backstep goes south; a pressed direction still wins, and so does the stick.
    for (dir, held, want) in [(None, None, S), (Some("E"), None, E), (None, Some(E), E)] {
        let mut app = arena();
        let hero = crate::common::hero(app.world_mut(), "stancer", Vec3::ZERO, N);
        if let Some(h) = held {
            app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = h;
        }
        match dir {
            Some(d) => assert_eq!(ok(&mut app, "dodge", json!({"dir": d})), format!("ok: back_roll ({d})")),
            None => press(&mut app, hero, "dodge"),
        }
        step(&mut app, 30);
        let moved = feet(&app, hero);
        assert!(moved.dot(want) > 1.0 && moved.dot(want) > moved.length() * 0.95, "{dir:?} {held:?}: {moved}");
    }
    // The tool names the way a nameless dodge goes.
    let mut app = arena();
    crate::common::hero(app.world_mut(), "stancer", Vec3::ZERO, N);
    assert_eq!(ok(&mut app, "dodge", json!({})), "ok: back_roll (back)");
    // The ordinary roll with nothing pressed rolls forward, as before.
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    press(&mut app, hero, "dodge");
    step(&mut app, 30);
    assert!(feet(&app, hero).dot(N) > 1.0, "{}", feet(&app, hero));
}

#[test]
fn a_backstep_is_back_from_the_body_facing_not_from_the_world() {
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "stancer", Vec3::ZERO, E);
    press(&mut app, hero, "dodge");
    step(&mut app, 30);
    assert!(feet(&app, hero).dot(-E) > 1.0, "{}", feet(&app, hero));
}

// --- turn rate of a stance ------------------------------------------------------------------

#[test]
fn a_guard_turns_at_its_own_rate_while_it_is_up() {
    // Aiming east from facing north, the free turn is done in a handful of ticks.
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().get_mut::<Intent>(hero).unwrap().aim = Some(Vec3::new(10.0, 0.0, 0.0));
    hold(&mut app, hero, "block", true);
    press(&mut app, hero, "block");
    step(&mut app, 30);
    assert!((turned(&app, hero) - 90.0).abs() < 1.0, "{}", turned(&app, hero));

    // The stance block turns at 90 degrees per second: half a second is about 45 degrees (plus the
    // first tick, which still runs at the free rate: the action is seen from the next tick on).
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "stancer", Vec3::ZERO, N);
    app.world_mut().get_mut::<Intent>(hero).unwrap().aim = Some(Vec3::new(10.0, 0.0, 0.0));
    hold(&mut app, hero, "block", true);
    press(&mut app, hero, "block");
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "stance_block");
    assert!((turned(&app, hero) - 55.0).abs() < 6.0, "{}", turned(&app, hero));
    assert!(app.world().get::<MoveModifiers>(hero).unwrap().describe().contains("turn 90deg/s"));

    // Let go: the override ends with the action.
    hold(&mut app, hero, "block", false);
    step(&mut app, 60);
    assert_eq!(action(&app, hero), "idle");
    assert!(app.world().get::<MoveModifiers>(hero).unwrap().turn_speed_deg.is_none());
}

// --- regeneration holds ------------------------------------------------------------------

/// Stamina regenerated in the 0.5 s after a 40-tick sprint ended, 50 stamina to begin with.
fn regained(free: bool, hold: Option<f32>) -> f32 {
    let mut app = arena();
    free_sprint_hold(&mut app, hold);
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    if free {
        app.world_mut().entity_mut(hero).insert(FreeRun);
    }
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 50.0;
    run_north(&mut app, hero, true);
    step(&mut app, 40);
    run_north(&mut app, hero, false);
    let at_release = stamina(&app, hero);
    step(&mut app, 30);
    stamina(&app, hero) - at_release
}

#[test]
fn a_free_sprint_holds_the_regeneration_for_its_own_time_and_a_paid_one_for_the_delay() {
    // The delay is 0.8 s, regeneration 30 per second; the free sprint holds 0.3 s: 0.2 s of regeneration.
    let held = regained(true, Some(0.3));
    assert!(held > 4.0 && held < 8.0, "{held}");
    assert!(regained(true, None) < 0.01, "today a free sprint restarts the whole delay");
    // A paid sprint sets the delay whatever the tuning says.
    assert!(regained(false, Some(0.3)) < 0.01);
}

#[test]
fn a_hold_is_the_longer_of_the_waits_and_never_shortens_one() {
    let mut s = Stamina::new(100.0, 30.0, 0.65);
    s.spend(10.0);
    s.hold_regen(0.3);
    assert_eq!(s.since_use, 0.0, "the spend's wait of 0.65 s is longer than a 0.3 s hold");
    s.since_use = 0.65;
    s.hold_regen(0.3);
    assert!((s.since_use - 0.35).abs() < 1e-6, "{}", s.since_use);
    s.hold_regen(1.3);
    assert!((s.since_use + 0.65).abs() < 1e-6, "a hold past the delay goes below zero: {}", s.since_use);
    s.hold_regen(0.2);
    assert!((s.since_use + 0.65).abs() < 1e-6, "a shorter hold does not shorten it");
}

#[test]
fn a_raised_guard_holds_the_regeneration_and_for_1_3_s_after_it_is_let_go() {
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "stancer", Vec3::ZERO, N);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 40.0;
    // Up for a second, the stamina does not come back, though 0.8 s passed since it was spent.
    hold(&mut app, hero, "block", true);
    press(&mut app, hero, "block");
    step(&mut app, 60);
    assert_eq!(action(&app, hero), "stance_block");
    assert_eq!(stamina(&app, hero), 40.0);
    // Let go: the lowering ends within 0.1 s, then 1.3 s more of waiting counted from the release.
    hold(&mut app, hero, "block", false);
    step(&mut app, 6 + 60);
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(stamina(&app, hero), 40.0, "still held a second after the guard went down");
    step(&mut app, 40);
    assert!(stamina(&app, hero) > 40.0, "{}", stamina(&app, hero));
}

#[test]
fn a_block_without_the_option_lets_the_stamina_come_back_at_once() {
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 40.0;
    hold(&mut app, hero, "block", true);
    press(&mut app, hero, "block");
    step(&mut app, 60);
    assert!(stamina(&app, hero) > 40.0, "{}", stamina(&app, hero));
}

// --- crawl lock -----------------------------------------------------------------------------

/// A pool 5 m deep out past the arena's floor, and a swimmer in it with a fast swim.
fn crawler(app: &mut App, resume: SprintResume) -> Entity {
    let world = app.world_mut();
    wall(world, Vec3::new(80.0, -6.0, -20.0), Vec3::new(120.0, -5.0, 20.0));
    world.spawn((
        Collider::new(Shape::cuboid(Vec3::new(20.0, 2.5, 20.0))).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_translation(Vec3::new(100.0, -2.5, 0.0)),
    ));
    let hero = crate::common::hero(world, "hero", Vec3::new(100.0, -1.1, 0.0), N);
    world.entity_mut(hero).insert(CharacterParams {
        swim_speed: 2.4,
        swim_fast_speed: 4.0,
        swim_fast_drain: 14.0,
        swim_fast_resume: resume,
        ..default()
    });
    hero
}

fn crawling(app: &App, e: Entity) -> bool {
    app.world().get::<MotionState>(e).unwrap().sprinting
}

#[test]
fn the_crawl_lock_opens_at_a_quarter_of_the_bar_without_a_sprint_input() {
    let mut app = arena();
    let hero = crawler(&mut app, SprintResume::Fraction(0.25));
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 3.0;
    run_north(&mut app, hero, true);
    step(&mut app, 30);
    assert_eq!(app.world().get::<MotionState>(hero).unwrap().mode, MotionMode::Water);
    assert!(app.world().get::<Stamina>(hero).unwrap().exhausted, "the crawl ran the bar dry");
    // Let go of sprint, keep swimming: the lock stays shut below a quarter (0.8 s of delay, then 30
    // per second: 25 is about a second and a half off).
    run_north(&mut app, hero, false);
    step(&mut app, 50);
    assert!(stamina(&app, hero) < 25.0, "{}", stamina(&app, hero));
    run_north(&mut app, hero, true);
    step(&mut app, 2);
    assert!(!crawling(&app, hero), "pressing again does not crawl; the bar is at {}", stamina(&app, hero));
    // Let go again and wait for a quarter with no sprint input at all: the lock opens by itself.
    run_north(&mut app, hero, false);
    step(&mut app, 60);
    assert!(stamina(&app, hero) >= 25.0, "{}", stamina(&app, hero));
    assert!(!app.world().get::<Stamina>(hero).unwrap().exhausted, "opened with no sprint input");
    run_north(&mut app, hero, true);
    step(&mut app, 2);
    assert!(crawling(&app, hero), "the first press after a quarter crawls at once");
}

#[test]
fn without_the_rule_the_crawl_lock_opens_on_release() {
    let mut app = arena();
    let hero = crawler(&mut app, SprintResume::OnRelease);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 3.0;
    run_north(&mut app, hero, true);
    step(&mut app, 30);
    run_north(&mut app, hero, false);
    step(&mut app, 70);
    run_north(&mut app, hero, true);
    step(&mut app, 2);
    assert!(crawling(&app, hero), "any stamina is enough after the button was let go");
}
