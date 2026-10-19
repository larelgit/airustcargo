//! Absence resets (the absence clock): nobody near for `reset_after` resets the run; `Restart` and
//! `KeepCleared`; survivors of a placed wave are healed and placed home; the dev reset.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::{EncounterRuns, RunState};
use shards::npc::Home;

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);
const NEAR: Vec3 = Vec3::new(0.0, 0.0, 6.0);

fn start(app: &mut App, encounter: &str) -> (String, Entity) {
    let p0 = player(app, 0, NEAR);
    let key = place(app, encounter, "r", Vec3::ZERO);
    step(app, 2);
    ok(app, "encounter_start", json!({"key": key}));
    (key, p0)
}

#[test]
fn the_run_resets_on_the_1800th_tick_with_nobody_near() {
    let mut app = watched(1);
    let (key, p0) = start(&mut app, "manual");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    // The player leaves for good (beyond the 20 m radius).
    move_to(&mut app, p0, FAR);
    let after = run_until(&mut app, 2000, |a| matches!(state(a, &key), RunState::Resetting { .. } | RunState::Idle));
    assert_eq!(after, 1800, "reset_after 30 s: the 1800th tick of nobody near");
    assert_eq!(saw(&app, "reset Absent"), 1);
    assert!(journal_has(&app, "reset (nobody near for 30.0 s)"), "{:?}", journal(&app));
    // The spawned wave is gone and the run starts over.
    assert!(run(&app, &key).members.is_empty());
    assert_eq!(run(&app, &key).wave, 0);
    assert_eq!(saw(&app, "died"), 0, "despawned silently");
    run_until(&mut app, 5, |a| state(a, &key) == RunState::Idle);
}

#[test]
fn someone_coming_back_near_restarts_the_clock() {
    let mut app = watched(1);
    let (key, p0) = start(&mut app, "manual");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    move_to(&mut app, p0, FAR);
    step(&mut app, 1000);
    assert_eq!(run(&app, &key).absent, 1000);
    move_to(&mut app, p0, NEAR);
    tick(&mut app);
    assert_eq!(run(&app, &key).absent, 0);
    move_to(&mut app, p0, FAR);
    step(&mut app, 1700);
    assert_eq!(state(&app, &key), RunState::Running, "1700 ticks away is not 1800");
}

#[test]
fn a_wipe_with_nobody_standing_near_resets_too() {
    let mut app = watched(1);
    let (key, p0) = start(&mut app, "manual");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    kill(&mut app, p0);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { .. }));
    // Nobody stands: the clock runs in `Wiped` as it does in the other fighting states.
    let after = run_until(&mut app, 2000, |a| matches!(state(a, &key), RunState::Resetting { .. } | RunState::Idle));
    assert!(after > 1700 && after <= 1800, "{after}");
    assert_eq!(saw(&app, "reset Absent"), 1);
}

#[test]
fn a_resume_in_progress_resets_for_absence_as_well() {
    // Finding 9: `Wiped { Some }` has an absence row. Someone stands again, but far away, and the
    // resume is long (the countdown would end first with the 2 s the other encounters have).
    let base = self::app(1);
    let db = db_edited(
        &base,
        &[(
            "slow.encounter.ron",
            r#"{ "slow": (radius: 20.0, start: Manual, resume_after: 100.0, reset_after: Some(30.0),
                waves: [(delay: 1.0, members: [(prefab: "grunt", at: Offsets([(0.0, 0.0, -5.0)]))])]) }"#,
        )],
    );
    let mut app = app_with_db(db);
    let (key, p0) = start(&mut app, "slow");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    kill(&mut app, p0);
    tick(&mut app);
    revive(&mut app, p0);
    move_to(&mut app, p0, FAR);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { resume: Some(_), .. }));
    run_until(&mut app, 2000, |a| matches!(state(a, &key), RunState::Resetting { .. } | RunState::Idle));
    assert_eq!(saw(&app, "reset Absent"), 1, "{:?}", seen(&app));
}

fn app_with_db(db: shards::content::ContentDb) -> App {
    let mut app = watched(1);
    shards::content::install(app.world_mut(), db);
    app
}

#[test]
fn restart_goes_back_to_the_start_and_keep_cleared_keeps_the_waves_cleared() {
    // KeepCleared: wave 1 cleared, wave 2 in progress when everyone leaves for 10 s.
    let mut app = watched(1);
    let (key, p0) = start(&mut app, "keeper");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    run_until(&mut app, 120, |a| run(a, &key).wave == 2);
    move_to(&mut app, p0, FAR);
    run_until(&mut app, 700, |a| matches!(state(a, &key), RunState::Resetting { .. } | RunState::Idle));
    assert_eq!(run(&app, &key).wave, 1, "the wave in progress is rewound as a wipe does, the cleared one stays cleared");
    assert!(run(&app, &key).members.is_empty());
    // Started again, wave 2 comes next.
    move_to(&mut app, p0, NEAR);
    run_until(&mut app, 5, |a| state(a, &key) == RunState::Idle);
    tick(&mut app);
    ok(&mut app, "encounter_start", json!({"key": key}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 2);

    // Restart: the same leaves the encounter at wave 0.
    let mut app = watched(1);
    let (key, p0) = start(&mut app, "manual");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    run_until(&mut app, 120, |a| run(a, &key).wave == 2);
    move_to(&mut app, p0, FAR);
    run_until(&mut app, 2000, |a| matches!(state(a, &key), RunState::Resetting { .. } | RunState::Idle));
    assert_eq!(run(&app, &key).wave, 0);
}

#[test]
fn survivors_of_a_placed_wave_are_healed_and_placed_home_and_spawned_waves_go() {
    let mut app = watched(1);
    let p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "camp", "camp", Vec3::ZERO);
    step(&mut app, 3);
    let placed = members(&app, &key)[0].1;
    app.world_mut().get_mut::<Home>(placed).unwrap().pos = Some(Vec3::new(3.0, 0.0, 0.0));
    move_to(&mut app, p0, Vec3::new(0.0, 0.0, 5.0));
    run_until(&mut app, 5, |a| state(a, &key) == RunState::Running);
    // The placed member is hurt and has wandered off; the player kills nothing and walks away.
    app.world_mut().get_mut::<shards::action::Health>(placed).unwrap().current = 3.0;
    app.world_mut().get_mut::<Transform>(placed).unwrap().translation = Vec3::new(10.0, 0.0, 10.0);
    move_to(&mut app, p0, FAR);
    let after = run_until(&mut app, 2000, |a| matches!(state(a, &key), RunState::Resetting { .. } | RunState::Idle));
    assert_eq!(after, 1800);
    assert_eq!(app.world().get::<shards::action::Health>(placed).unwrap().current, 20.0, "healed");
    assert_eq!(app.world().get::<Transform>(placed).unwrap().translation, Vec3::new(3.0, 0.0, 0.0), "placed at home");
    assert_eq!(saw(&app, "healed"), 1);
    assert_eq!(run(&app, &key).wave, 1, "the placed wave's survivors are still there");
    assert_eq!(run(&app, &key).members.len(), 1);
}

#[test]
fn the_dev_reset_resets_from_any_fighting_state_and_forget_leaves_the_world_as_found() {
    let mut app = watched(1);
    let (key, _p0) = start(&mut app, "manual");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    let text = ok(&mut app, "encounter_reset", json!({"key": key}));
    assert!(text.contains("reset"), "{text}");
    assert_eq!(saw(&app, "reset Dev"), 1);
    assert!(matches!(state(&app, &key), RunState::Resetting { .. } | RunState::Idle));
    assert!(run(&app, &key).members.is_empty());
    // An Idle run has nothing to reset.
    run_until(&mut app, 5, |a| state(a, &key) == RunState::Idle);
    assert!(ok(&mut app, "encounter_reset", json!({"key": key})).contains("nothing to reset"));
    // `forget` drops the run; it is made again from the anchor.
    ok(&mut app, "encounter_reset", json!({"key": "all", "forget": true}));
    assert!(app.world().get_resource::<EncounterRuns>().is_none(), "no runs, nothing remembered");
    step(&mut app, 2);
    assert_eq!(state(&app, &key), RunState::Armed, "made fresh: it waits for its start again");
}
