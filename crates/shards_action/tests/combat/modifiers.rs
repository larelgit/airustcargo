//! Movement modifiers from statuses, the sprint's cost multiplier, and what the AI sees.

use bevy::prelude::*;
use shards_action::{add_status_buildup, NoStatusSlow, Statuses};
use shards_action::Stamina;
use shards_core::Intent;
use shards_motion::{ModifierSystems, MotionState, MoveModifiers};

use crate::common::*;

/// Marks a body whose sprint is free (the game's source for "out of combat").
#[derive(Component)]
struct FreeRun;

#[derive(Component)]
struct HalfCost;

fn free_run(mut q: Query<(&mut MoveModifiers, Has<FreeRun>, Has<HalfCost>)>) {
    for (mut m, free, half) in &mut q {
        if free {
            m.scale_sprint_cost(0.0);
        }
        if half {
            m.scale_sprint_cost(0.5);
        }
    }
}

fn arena() -> App {
    app_with(|app| {
        app.add_systems(FixedUpdate, free_run.in_set(ModifierSystems::Contribute).after(ModifierSystems::Statuses));
    })
}

fn speed(app: &App, e: Entity) -> f32 {
    let v = app.world().get::<MotionState>(e).unwrap().velocity;
    Vec2::new(v.x, v.z).length()
}

fn run_north(app: &mut App, e: Entity, sprint: bool) {
    let mut intent = app.world_mut().get_mut::<Intent>(e).unwrap();
    intent.move_dir = N;
    intent.set_held("sprint", sprint);
}

#[test]
fn a_slow_status_slows_while_it_lasts_and_the_ai_reads_it() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    run_north(&mut app, hero, false);
    step(&mut app, 30);
    assert_eq!(speed(&app, hero), 5.0);
    add_status_buildup(app.world_mut(), hero, "slow", 100.0);
    step(&mut app, 30);
    assert!((speed(&app, hero) - 3.0).abs() < 1e-3, "{}", speed(&app, hero));
    let seen = ai_core::observe::observe(app.world(), &ai_core::observe::ObserveOptions::default());
    assert!(seen.contains("slowed x0.60"), "{seen}");
    step(&mut app, 120); // 2 s duration
    assert_eq!(speed(&app, hero), 5.0);
}

#[test]
fn a_root_status_holds_the_body_and_a_cap_status_caps_a_sprint() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    run_north(&mut app, hero, false);
    add_status_buildup(app.world_mut(), hero, "root", 100.0);
    step(&mut app, 30);
    assert_eq!(speed(&app, hero), 0.0);
    let seen = ai_core::observe::observe(app.world(), &ai_core::observe::ObserveOptions::default());
    assert!(seen.contains("rooted"), "{seen}");
    step(&mut app, 60);
    assert!(speed(&app, hero) > 4.9, "free again: {}", speed(&app, hero));

    run_north(&mut app, hero, true);
    add_status_buildup(app.world_mut(), hero, "burden", 100.0);
    step(&mut app, 30);
    assert!((speed(&app, hero) - 3.0).abs() < 1e-3, "{}", speed(&app, hero));
    let seen = ai_core::observe::observe(app.world(), &ai_core::observe::ObserveOptions::default());
    assert!(seen.contains("capped 3.0m/s"), "{seen}");
}

#[test]
fn a_body_that_takes_statuses_but_is_not_slowed_by_them() {
    let mut app = arena();
    let boss = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().entity_mut(boss).insert(NoStatusSlow);
    run_north(&mut app, boss, false);
    add_status_buildup(app.world_mut(), boss, "slow", 100.0);
    add_status_buildup(app.world_mut(), boss, "burden", 100.0);
    step(&mut app, 30);
    assert!(app.world().get::<Statuses>(boss).unwrap().get("slow").is_some_and(|s| s.active > 0.0), "it has the status");
    assert_eq!(speed(&app, boss), 5.0, "neither the slow nor the cap moves it");
    // A root is not a slow: it still holds the body.
    let mut app = arena();
    let boss = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().entity_mut(boss).insert(NoStatusSlow);
    run_north(&mut app, boss, false);
    add_status_buildup(app.world_mut(), boss, "root", 100.0);
    step(&mut app, 30);
    assert_eq!(speed(&app, boss), 0.0);
}

#[test]
fn the_sprint_cost_multiplier_scales_the_drain_and_zero_makes_it_free() {
    // Default: 30 per second.
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    run_north(&mut app, hero, true);
    step(&mut app, 30);
    let plain = 100.0 - stamina(&app, hero);
    assert!((plain - 15.0).abs() < 1.0, "{plain}");

    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().entity_mut(hero).insert(HalfCost);
    run_north(&mut app, hero, true);
    step(&mut app, 30);
    let half = 100.0 - stamina(&app, hero);
    assert!((half - plain / 2.0).abs() < 0.5, "{half} vs {plain}");

    // Free: nothing drains, even from nothing, and the sprint goes on.
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
    app.world_mut().entity_mut(hero).insert(FreeRun);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().current = 0.0;
    run_north(&mut app, hero, true);
    step(&mut app, 60);
    assert!(app.world().get::<MotionState>(hero).unwrap().sprinting, "a free sprint needs no stamina");
    assert_eq!(stamina(&app, hero), 0.0, "and holds the regeneration back");
    assert!(!app.world().get::<Stamina>(hero).unwrap().exhausted);
}
