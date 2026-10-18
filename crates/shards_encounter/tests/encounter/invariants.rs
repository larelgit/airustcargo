//! The engine rules `encounter.*`: each one is shown to catch the bug it exists for, and none
//! fires on a whole fight.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::{EncounterMember, EncounterRuns, RunState};
use shards::testing;

use crate::common::*;

/// Steps and returns the stop line.
fn stop(app: &mut App) -> String {
    let out = testing::call(app, "step", json!({"ticks": 20}));
    let text = out.result.as_str().unwrap_or_default().to_owned();
    assert!(out.halted.is_some(), "the game did not stop: {text}");
    text
}

/// Runs `corrupt` at the end of tick `at` (after the module's own step, before the rules), the
/// way a buggy system of a game would leave the world.
fn plant(app: &mut App, at: u64, corrupt: fn(&mut World)) {
    app.add_systems(
        FixedUpdate,
        (move |world: &mut World| {
            if ai_core::sim::tick(world) == at {
                corrupt(world);
            }
        })
        .in_set(SimSet::Cleanup),
    );
}

fn fight(app: &mut App) -> String {
    let _p0 = player(app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(app, "ring_lock", "l", Vec3::ZERO);
    step(app, 2);
    ok(app, "encounter_start", json!({"key": key}));
    run_until(app, 120, |a| run(a, &key).wave == 1);
    key
}

#[test]
fn a_stray_marker_or_a_missing_one_stops_the_game() {
    let mut app = app(1);
    let key = fight(&mut app);
    let stray = app.world_mut().spawn((AiKind::new("Stray"), EncounterMember { key: key.clone(), wave: 1, counted: true, returning: false })).id();
    let text = stop(&mut app);
    assert!(text.contains("stopped: invariant encounter.members_tracked violated"), "{text}");
    let line = journal(&app).into_iter().find(|l| l.contains("encounter.members_tracked")).unwrap();
    assert!(line.contains("which does not list it"), "{line}");
    app.world_mut().entity_mut(stray).despawn();

    // A listed member without its marker.
    let mut app = self::app(1);
    let key = fight(&mut app);
    let (_, member) = members(&app, &key)[0];
    app.world_mut().entity_mut(member).remove::<EncounterMember>();
    let text = stop(&mut app);
    assert!(text.contains("encounter.members_tracked"), "{text}");
    let line = journal(&app).into_iter().find(|l| l.contains("encounter.members_tracked")).unwrap();
    assert!(line.contains(&format!("is listed by run `{key}` but does not carry its EncounterMember")), "{line}");
}

#[test]
fn a_marker_that_disagrees_with_its_run_stops_the_game() {
    let mut app = app(1);
    let key = fight(&mut app);
    let (_, member) = members(&app, &key)[0];
    app.world_mut().get_mut::<EncounterMember>(member).unwrap().wave = 9;
    let text = stop(&mut app);
    assert!(text.contains("encounter.members_tracked"), "{text}");
}

#[test]
fn a_wave_past_the_end_or_a_timer_past_its_limit_or_unsorted_runs_stop_the_game() {
    let mut app = app(1);
    let key = fight(&mut app);
    let at = now(&app) + 1;
    plant(&mut app, at, |w| w.resource_mut::<EncounterRuns>().runs[0].wave = 9);
    let text = stop(&mut app);
    assert!(text.contains("encounter.state_sane"), "{text}");
    assert!(journal(&app).iter().any(|l| l.contains(&format!("run `{key}` is in wave 9 of 3"))), "{:?}", journal(&app));

    let mut app = self::app(1);
    let key = fight(&mut app);
    let at = now(&app) + 1;
    plant(&mut app, at, |w| w.resource_mut::<EncounterRuns>().runs[0].state = RunState::Between { left: 99_999 });
    let text = stop(&mut app);
    assert!(text.contains("encounter.state_sane"), "{text}");
    assert!(journal(&app).iter().any(|l| l.contains(&format!("run `{key}` Between has 99999 ticks left"))), "{:?}", journal(&app));

    // Two runs with one key.
    let mut app = self::app(1);
    let _key = fight(&mut app);
    let copy = app.world().resource::<EncounterRuns>().runs[0].clone();
    app.world_mut().resource_mut::<EncounterRuns>().runs.push(copy);
    let text = stop(&mut app);
    assert!(text.contains("encounter.state_sane"), "{text}");
}

#[test]
fn a_won_once_run_that_is_not_remembered_stops_the_game() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "once_one", "o", Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": key}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    app.world_mut().resource_mut::<EncounterRuns>().runs[0].state = RunState::Won;
    let text = stop(&mut app);
    assert!(text.contains("encounter.state_sane"), "{text}");
    assert!(journal(&app).iter().any(|l| l.contains("is Won and `once`, but its key is not in completed")), "{:?}", journal(&app));
}

#[test]
fn a_lock_that_vanishes_mid_wave_stops_the_game() {
    let mut app = app(1);
    let key = fight(&mut app);
    // The step would put a lock back that vanished between ticks; one that goes at the end of a
    // tick is what the rule is for.
    let at = now(&app) + 1;
    plant(&mut app, at, |w| {
        let lock = w.resource::<EncounterRuns>().runs[0].lock.expect("the lock stands");
        let entity = w.resource::<ai_core::AiIndex>().get(lock).unwrap();
        w.despawn(entity);
    });
    let text = stop(&mut app);
    assert!(text.contains("encounter.lock_matches"), "{text}");
    assert!(journal(&app).iter().any(|l| l.contains(&format!("run `{key}` is Running: its lock `barrier` should stand"))), "{:?}", journal(&app));
}

#[test]
fn a_whole_fight_breaks_no_rule() {
    // The rules stay armed through a fight with a win, a wipe and a resume.
    let mut app = watched(1);
    let key = fight(&mut app);
    let p0 = app.world_mut().query_filtered::<Entity, With<PlayerId>>().iter(app.world()).next().unwrap();
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    tick(&mut app);
    kill(&mut app, p0);
    step(&mut app, 3);
    revive(&mut app, p0);
    run_until(&mut app, 400, |a| run(a, &key).wave == 2);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    step(&mut app, 400);
    assert!(journal(&app).iter().all(|l| !l.contains("invariant")), "{:?}", journal(&app));
}
