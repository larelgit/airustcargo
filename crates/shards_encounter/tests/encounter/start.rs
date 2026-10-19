//! Start conditions: a trigger volume (any player, all players), the edge, distance, manual.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::RunState;

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);
const IN: Vec3 = Vec3::new(0.0, 0.0, 0.0);

fn started(app: &App, key: &str) -> bool {
    !matches!(state(app, key), RunState::Idle | RunState::Armed)
}

/// A trigger volume `ring` 24 x 8 x 24 m at the anchor (origin).
fn ring(app: &mut App) {
    trigger(app, "ring", Vec3::new(0.0, 2.0, 0.0), Vec3::new(12.0, 4.0, 12.0));
}

#[test]
fn a_trigger_volume_starts_the_run_when_any_player_steps_in() {
    let mut app = app(1);
    ring(&mut app);
    let p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "trigger_any", "a", Vec3::ZERO);
    step(&mut app, 3);
    assert_eq!(state(&app, &key), RunState::Armed, "the volume is false at the first look, so the run arms");
    move_to(&mut app, p0, IN);
    run_until(&mut app, 4, |a| started(a, &key));
    assert!(matches!(state(&app, &key), RunState::Between { .. }), "{:?}", state(&app, &key));
    assert!(journal_has(&app, "started: 1/1 players in"), "{:?}", journal(&app));
}

#[test]
fn with_all_players_in_the_start_waits_for_everyone_standing() {
    let mut app = app(1);
    ring(&mut app);
    let p0 = player(&mut app, 0, FAR);
    let p1 = player(&mut app, 1, FAR);
    let p2 = player(&mut app, 2, FAR);
    kill(&mut app, p2);
    let key = place(&mut app, "ring_waves", "ring", Vec3::ZERO);
    step(&mut app, 3);
    assert_eq!(state(&app, &key), RunState::Armed);
    // One in: no start.
    move_to(&mut app, p0, IN);
    step(&mut app, 5);
    assert_eq!(state(&app, &key), RunState::Armed, "one of two standing players is in");
    // Both in (the dead third is not waited for): the start.
    move_to(&mut app, p1, Vec3::new(2.0, 0.0, 2.0));
    run_until(&mut app, 5, |a| started(a, &key));
    assert!(journal_has(&app, "started: 2/3 players in"), "{:?}", journal(&app));
    let _ = p2;
}

#[test]
fn a_run_made_under_a_players_feet_starts_only_after_the_player_leaves_and_comes_back() {
    let mut app = app(1);
    ring(&mut app);
    let p0 = player(&mut app, 0, IN);
    let key = place(&mut app, "trigger_any", "a", Vec3::ZERO);
    step(&mut app, 10);
    assert_eq!(state(&app, &key), RunState::Idle, "the player was inside when the run was made");
    move_to(&mut app, p0, FAR);
    step(&mut app, 3);
    assert_eq!(state(&app, &key), RunState::Armed);
    move_to(&mut app, p0, IN);
    run_until(&mut app, 4, |a| started(a, &key));
}

#[test]
fn near_starts_within_the_distance_and_all_players_in_waits_for_both() {
    let mut app = app(1);
    let p0 = player(&mut app, 0, FAR);
    let p1 = player(&mut app, 1, FAR);
    let key = place(&mut app, "near_all", "a", Vec3::ZERO);
    step(&mut app, 3);
    move_to(&mut app, p0, Vec3::new(0.0, 0.0, 6.0));
    step(&mut app, 5);
    assert_eq!(state(&app, &key), RunState::Armed);
    move_to(&mut app, p1, Vec3::new(5.0, 0.0, 0.0));
    run_until(&mut app, 4, |a| started(a, &key));

    // Plain `Near` starts for one.
    let mut app = self::app(1);
    let p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "single", "s", Vec3::ZERO);
    step(&mut app, 3);
    move_to(&mut app, p0, Vec3::new(0.0, 0.0, 9.0));
    run_until(&mut app, 4, |a| started(a, &key));
}

#[test]
fn a_manual_encounter_starts_only_by_start_encounter() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 2.0));
    let key = place(&mut app, "manual", "m", Vec3::ZERO);
    step(&mut app, 120);
    assert_eq!(state(&app, &key), RunState::Armed, "a player on top of it does not start it");
    let text = ok(&mut app, "encounter_start", json!({"key": key}));
    assert!(text.contains("started"), "{text}");
    assert!(started(&app, &key));
    let refused = err(&mut app, "encounter_start", json!({"key": key}));
    assert!(refused.contains("only an Idle or Armed run can be started"), "{refused}");
}

#[test]
fn a_missing_trigger_volume_is_said_once() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "ring_waves", "ring", Vec3::ZERO);
    step(&mut app, 30);
    assert_eq!(state(&app, &key), RunState::Armed);
    assert_eq!(count(&app, "no trigger volume `ring` within 20 m of its anchor"), 1, "{:?}", journal(&app));
}

#[test]
fn the_nearest_volume_of_the_name_within_the_radius_is_the_one() {
    let mut app = app(1);
    // A volume of the same name beyond the radius is not this encounter's.
    trigger(&mut app, "ring", Vec3::new(0.0, 2.0, 60.0), Vec3::new(5.0, 4.0, 5.0));
    let p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "trigger_any", "a", Vec3::ZERO);
    step(&mut app, 3);
    assert_eq!(count(&app, "no trigger volume"), 1, "the far volume is out of the radius");
    ring(&mut app);
    move_to(&mut app, p0, IN);
    run_until(&mut app, 8, |a| started(a, &key));
}

#[test]
fn starting_wave_one_of_a_placed_encounter_is_the_plain_start_and_spawns_nothing_twice() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "lone_pack", "lp", Vec3::ZERO);
    step(&mut app, 3);
    assert_eq!(members(&app, &key).len(), 2);
    ok(&mut app, "encounter_start", json!({"key": key, "wave": 1}));
    step(&mut app, 200);
    assert_eq!(members(&app, &key).len(), 2, "the placed pack stands once: {:?}", run(&app, &key).members);
    assert_eq!(state(&app, &key), RunState::Running);
}

#[test]
fn starting_wave_two_of_a_placed_encounter_skips_the_placed_wave_and_warns_and_spawns_wave_two() {
    let mut app = watched(1);
    let _p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "camp", "camp", Vec3::ZERO);
    step(&mut app, 3);
    assert_eq!(members(&app, &key).len(), 1, "the placed member stands");
    ok(&mut app, "encounter_start", json!({"key": key, "wave": 2}));
    assert!(matches!(state(&app, &key), RunState::Between { .. }), "wave 2 is due, not the placed wave fought: {:?}", state(&app, &key));
    run_until(&mut app, 200, |a| run(a, &key).wave == 2);
    let seen = app.world().resource::<Seen>().0.clone();
    assert!(seen.iter().any(|s| s.starts_with("warned 2 ")), "{seen:?}");
    assert!(seen.iter().any(|s| s.starts_with("started 2 members=1")), "{seen:?}");
    assert_eq!(members(&app, &key).len(), 2, "the placed member and wave 2's");
}
