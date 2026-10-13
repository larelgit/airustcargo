//! E2 acceptance in miniature: stairs → ledge (mantle) → pool (swim) → cliff (glide,
//! with an updraft) → goal pad, driven only through `LocomotionInput`; bit-identical
//! on every run, whatever the `Entity` indices; replay checks cover the new components.

use ai_core::{AiJournal, AiKind};
use bevy::prelude::*;
use shards_motion::*;

use crate::common::*;

/// The goal pad's center.
const GOAL: Vec3 = Vec3::new(57.5, -6.0, 0.0);

/// Builds the course. `noise` spawns an unrelated entity after every collider, the way
/// a window spawns render entities: `Entity` indices shift, collider ids do not.
fn course(world: &mut World, noise: bool) -> Entity {
    let solid = |world: &mut World, min: Vec3, max: Vec3| {
        block(world, min, max);
        if noise {
            world.spawn((Name::new("render noise"), Transform::default()));
        }
    };
    // Start pad.
    solid(world, Vec3::new(-5.0, -1.0, -4.0), Vec3::new(3.0, 0.0, 4.0));
    // Stairs: eight 0.25 m steps up to a plateau at 2 m.
    for i in 0..8 {
        let x = 3.0 + 0.5 * i as f32;
        solid(world, Vec3::new(x, -1.0, -4.0), Vec3::new(10.0, 0.25 * (i + 1) as f32, 4.0));
    }
    // A 1.2 m ledge up to the pool deck at 3.2 m.
    solid(world, Vec3::new(10.0, -1.0, -4.0), Vec3::new(14.0, 3.2, 4.0));
    // The pool: bottom at 0.2, walls up to the decks, water up to 3.0.
    solid(world, Vec3::new(14.0, -1.0, -4.0), Vec3::new(22.0, 0.2, 4.0));
    solid(world, Vec3::new(14.0, -1.0, 3.0), Vec3::new(22.0, 3.2, 4.0));
    solid(world, Vec3::new(14.0, -1.0, -4.0), Vec3::new(22.0, 3.2, -3.0));
    volume(world, Vec3::new(14.0, 0.2, -3.0), Vec3::new(22.0, 3.0, 3.0), VolumeKind::Water);
    // The far deck ends in a cliff over a canyon at -6.
    solid(world, Vec3::new(22.0, -1.0, -4.0), Vec3::new(26.0, 3.2, 4.0));
    solid(world, Vec3::new(26.0, -7.0, -10.0), Vec3::new(80.0, -6.0, 10.0));
    volume(world, Vec3::new(32.0, -6.0, -4.0), Vec3::new(40.0, 6.0, 4.0), VolumeKind::Updraft { accel: 20.0, max_speed: 5.0 });
    let goal = volume(
        world,
        Vec3::new(55.0, -6.0, -3.0),
        Vec3::new(60.0, -3.0, 3.0),
        VolumeKind::Trigger { name: "goal".into() },
    );
    if noise {
        world.spawn((Name::new("render noise"), Transform::default()));
    }
    goal
}

/// A scripted runner: east until past the goal's near edge, gliding once over the
/// canyon; then down onto the pad and walk to its center.
fn brain(feet: Vec3, state: &MotionState) -> LocomotionInput {
    if feet.x < 56.0 && state.mode != MotionMode::Ground || feet.x < 50.0 {
        return LocomotionInput { move_dir: Vec3::X, glide: feet.x > 26.5, ..default() };
    }
    let to_goal = Vec3::new(GOAL.x - feet.x, 0.0, GOAL.z - feet.z);
    let move_dir = if to_goal.length() < 0.2 { Vec3::ZERO } else { to_goal.normalize() * 0.5 };
    LocomotionInput { move_dir, ..default() }
}

struct Run {
    /// Transform and motion state, every 60th tick.
    samples: Vec<(u64, [u32; 7], String)>,
    end: Vec3,
    reached: bool,
    modes: Vec<MotionMode>,
}

fn run_course(noise: bool) -> Run {
    let mut app = app();
    let goal = course(app.world_mut(), noise);
    let hero = hero(app.world_mut(), Vec3::new(0.0, 0.0, 0.0));
    let mut samples = Vec::new();
    for tick in 1..=3600u64 {
        let world = app.world_mut();
        let here = world.get::<Transform>(hero).unwrap().translation;
        let input = brain(here, world.get::<MotionState>(hero).unwrap());
        set_input(world, hero, input);
        step(&mut app, 1);
        assert_clear(&app, hero);
        if tick % 60 == 0 {
            let tf = app.world().get::<Transform>(hero).unwrap();
            let (t, r) = (tf.translation, tf.rotation);
            let bits = [t.x, t.y, t.z, r.x, r.y, r.z, r.w].map(f32::to_bits);
            // Volumes by collider id: `Entity` handles differ with the noise, ids do not.
            let mut s = state(&app, hero);
            let ids: Vec<ColliderId> = s.volumes.iter().map(|v| *app.world().get::<ColliderId>(*v).unwrap()).collect();
            s.volumes.clear();
            samples.push((tick, bits, format!("{s:?} in {ids:?}")));
        }
        let at = feet(&app, hero);
        if state(&app, hero).mode == MotionMode::Ground && (at - GOAL).length() < 0.3 && tick % 60 == 0 {
            break;
        }
    }
    let seen = seen(&app);
    let reached = seen.entered.iter().any(|v| v.entity == hero && v.volume == goal);
    Run { samples, end: feet(&app, hero), reached, modes: seen.modes.iter().map(|m| m.to).collect() }
}

#[test]
fn the_obstacle_course_is_finished_the_same_way_every_time() {
    let first = run_course(false);
    assert!(first.reached, "never entered the goal pad; ended at {} after {:?}", first.end, first.modes);
    assert!((first.end - GOAL).length() < 0.3, "stands on the pad: {}", first.end);
    for mode in [MotionMode::Mantle, MotionMode::Water, MotionMode::Glide] {
        assert!(first.modes.contains(&mode), "never {mode:?}: {:?}", first.modes);
    }

    let second = run_course(false);
    assert_eq!(first.samples, second.samples, "two runs differ");

    let shifted = run_course(true);
    assert_eq!(first.samples, shifted.samples, "Entity indices changed the run");
}

#[test]
fn replay_checks_cover_motion_components() {
    let mut app = app();
    floor(app.world_mut());
    let hero = app.world_mut().spawn((AiKind::new("Hero"), character(CharacterParams::default(), Vec3::ZERO))).id();
    walk(app.world_mut(), hero, Vec3::X);
    step(&mut app, 30);
    press_jump(app.world_mut(), hero);
    step(&mut app, 100); // past the checks at ticks 60 and 120

    let journal = app.world().resource::<AiJournal>();
    let lines: Vec<String> = journal.after(0).0.map(ToString::to_string).collect();
    assert!(!lines.iter().any(|l| l.contains("leave out")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.ends_with("e1 jumped")), "{lines:#?}");

    let hash = ai_core::replay::state_hash(app.world());
    app.world_mut().get_mut::<MotionState>(hero).unwrap().coyote_left += 1.0;
    assert_ne!(hash, ai_core::replay::state_hash(app.world()), "MotionState is part of the replay hash");

    let facts = ai_core::describe::facts(app.world(), hero).unwrap();
    assert_eq!(facts.fragment("MotionState"), Some("moving 5.0m/s"), "{facts:?}");
}
