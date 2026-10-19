//! Wipes: a wipe in a running wave despawns it silently and rewinds one wave; in a wave
//! before the first it goes to 0; between waves it rewinds nothing; the resume is tick exact; a
//! placed wave is pulled back, not despawned; a downed body (B5) is out, and so is one a game's own
//! out rule counts.

use shards::prelude::*;
use shards::encounter::RunState;
use shards::npc::Home;

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);

struct Ring {
    key: String,
    p0: Entity,
}

/// The ring fight, with the events watched, at wave `wave` running.
fn ring_at(app: &mut App, wave: u32) -> Ring {
    trigger(app, "ring", Vec3::new(0.0, 2.0, 0.0), Vec3::new(12.0, 4.0, 12.0));
    let p0 = player(app, 0, FAR);
    let key = place(app, "ring_waves", "ring", Vec3::ZERO);
    step(app, 3);
    move_to(app, p0, Vec3::new(0.0, 0.0, 3.0));
    run_until(app, 5, |a| matches!(state(a, &key), RunState::Between { .. }));
    for w in 1..=wave {
        run_until(app, 400, |a| run(a, &key).wave == w);
        if w < wave {
            for (_, e) in members(app, &key) {
                kill(app, e);
            }
            tick(app);
        }
    }
    Ring { key, p0 }
}

#[test]
fn a_wipe_in_a_running_wave_despawns_it_silently_and_goes_back_one_wave() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 2);
    let old: Vec<_> = members(&app, &key);
    assert_eq!(old.len(), 2);
    kill(&mut app, p0);
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Wiped { resume: None, pulled_back: false });
    assert_eq!(run(&app, &key).wave, 1);
    assert!(run(&app, &key).members.is_empty());
    for (id, _) in &old {
        assert!(app.world().resource::<ai_core::AiIndex>().get(*id).is_none(), "{id} is despawned");
    }
    assert_eq!(saw(&app, "died"), 0, "no Died on despawned members: no loot, no death cue");
    assert!(seen(&app).contains(&"wiped 1 rewound=true between=false".to_owned()), "{:?}", seen(&app));
    assert!(journal_has(&app, "wiped in wave 2/3, back to 1/3"), "{:?}", journal(&app));
    // The lock goes with the fight, so respawned players can come back in.
    assert!(run(&app, &key).lock.is_none());
}

#[test]
fn the_resume_spawns_the_pending_wave_120_ticks_after_someone_stands_with_new_ids() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 2);
    let old: Vec<u32> = members(&app, &key).iter().map(|(id, _)| id.0).collect();
    kill(&mut app, p0);
    step(&mut app, 30);
    assert_eq!(state(&app, &key), RunState::Wiped { resume: None, pulled_back: false }, "nobody stands, nothing resumes");
    revive(&mut app, p0);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { resume: Some(_), .. }));
    assert!(journal_has(&app, "resumed, wave 2/3 in 2.0 s"), "{:?}", journal(&app));
    assert_eq!(saw(&app, "resumed 2"), 1);
    let after = run_until(&mut app, 400, |a| run(a, &key).wave == 2);
    assert_eq!(after, 120, "resume_after 2 s");
    let fresh: Vec<u32> = members(&app, &key).iter().map(|(id, _)| id.0).collect();
    assert_eq!(fresh.len(), 2);
    assert!(fresh.iter().all(|id| !old.contains(id) && *id > old[1]), "{old:?} -> {fresh:?}");
    assert_eq!(state(&app, &key), RunState::Running);
    assert!(run(&app, &key).lock.is_some(), "the lock stands again");
}

#[test]
fn a_wipe_in_the_first_wave_goes_to_zero_and_the_resume_spawns_wave_one_without_a_start() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 1);
    kill(&mut app, p0);
    tick(&mut app);
    assert_eq!(run(&app, &key).wave, 0);
    revive(&mut app, p0);
    tick(&mut app);
    let after = run_until(&mut app, 300, |a| run(a, &key).wave == 1);
    assert_eq!(after, 120);
    assert_eq!(saw(&app, "encounter-started"), 1, "no second start");
}

#[test]
fn a_wipe_between_waves_rewinds_nothing_so_no_wave_repeats() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 1);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Between { .. }));
    kill(&mut app, p0);
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Wiped { resume: None, pulled_back: false });
    assert_eq!(run(&app, &key).wave, 1, "nothing was rewound");
    assert!(seen(&app).contains(&"wiped 1 rewound=false between=true".to_owned()), "{:?}", seen(&app));
    assert!(journal_has(&app, "wiped between waves, wave 2/3 still pending"), "{:?}", journal(&app));
    revive(&mut app, p0);
    tick(&mut app);
    run_until(&mut app, 300, |a| run(a, &key).wave == 2);
    assert_eq!(saw(&app, "started 1 "), 1, "wave 1 did not spawn again");
    assert_eq!(saw(&app, "started 2 "), 1);
}

#[test]
fn all_out_again_during_the_resume_stops_it() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 2);
    kill(&mut app, p0);
    tick(&mut app);
    revive(&mut app, p0);
    step(&mut app, 50);
    assert!(matches!(state(&app, &key), RunState::Wiped { resume: Some(_), .. }));
    kill(&mut app, p0);
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Wiped { resume: None, pulled_back: false });
    step(&mut app, 300);
    assert_eq!(run(&app, &key).wave, 1, "nothing spawned while nobody stood");
}

#[test]
fn one_body_down_is_not_a_wipe_and_every_body_down_is() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 1);
    let p1 = player(&mut app, 1, Vec3::new(2.0, 0.0, 3.0));
    kill(&mut app, p0);
    step(&mut app, 5);
    assert_eq!(state(&app, &key), RunState::Running, "p1 still stands");
    kill(&mut app, p1);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { .. }));
}

#[test]
fn a_games_out_rule_counts_a_participant_as_out() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 1);
    app.world_mut().entity_mut(p0).insert(Benched);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { .. }), "{:?}", state(&app, &key));
    // Back in (the game removes the marker) resumes it.
    app.world_mut().entity_mut(p0).remove::<Benched>();
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { resume: Some(_), .. }));
}

/// The engine's own `downed` out rule (B5): a downed body is out without a game's rule, and a wipe
/// counts every incapacitated player, dead or down.
#[test]
fn a_downed_body_is_out_and_a_wipe_counts_dead_and_down_alike() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 1);
    let p1 = player(&mut app, 1, Vec3::new(2.0, 0.0, 3.0));
    assert!(shards::action::go_down(app.world_mut(), p0, None, None));
    step(&mut app, 5);
    assert_eq!(state(&app, &key), RunState::Running, "p1 still stands");
    kill(&mut app, p1);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { .. }), "one dead, one down: {:?}", state(&app, &key));
    // Stood up again: the wipe resumes.
    assert!(shards::action::revive_downed(app.world_mut(), p0, 0.5, 0.0, None));
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { resume: Some(_), .. }), "{:?}", state(&app, &key));
}

#[test]
fn clearing_beats_wiping_on_one_tick() {
    let mut app = watched(1);
    let Ring { key, p0 } = ring_at(&mut app, 1);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    kill(&mut app, p0);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Between { .. }), "cleared first: {:?}", state(&app, &key));
    assert!(saw(&app, "cleared 1") == 1 && saw(&app, "wiped") == 0);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Wiped { .. }), "the wipe is checked from the next tick, against Between");
    assert_eq!(run(&app, &key).wave, 1);
}

// --- a placed wave ------------------------------------------------------------------------

/// The camp: a placed member at (3, 0, 0), then two spawned waves. A player at 5 m starts it.
fn camp(app: &mut App) -> (String, Entity, Entity) {
    let p0 = player(app, 0, FAR);
    let key = place(app, "camp", "camp", Vec3::ZERO);
    step(app, 3);
    let member = members(app, &key)[0].1;
    app.world_mut().get_mut::<Home>(member).unwrap().pos = Some(Vec3::new(3.0, 0.0, 0.0));
    move_to(app, p0, Vec3::new(0.0, 0.0, 5.0));
    run_until(app, 5, |a| matches!(state(a, &key), RunState::Running));
    (key, p0, member)
}

#[test]
fn a_placed_wave_is_pulled_back_healed_and_placed_home_not_despawned() {
    let mut app = watched(1);
    let (key, p0, member) = camp(&mut app);
    app.world_mut().get_mut::<shards::action::Health>(member).unwrap().current = 4.0;
    app.world_mut().get_mut::<Transform>(member).unwrap().translation = Vec3::new(9.0, 0.0, 9.0);
    kill(&mut app, p0);
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Wiped { resume: None, pulled_back: true });
    assert!(app.world().get_entity(member).is_ok(), "the survivor stays");
    assert_eq!(app.world().get::<shards::action::Health>(member).unwrap().current, 20.0, "healed");
    assert_eq!(app.world().get::<Transform>(member).unwrap().translation, Vec3::new(3.0, 0.0, 0.0), "placed at home");
    assert_eq!(saw(&app, "healed"), 1);
    assert_eq!(run(&app, &key).wave, 1, "not rewound");
    // The resume goes back to waiting for the start: Idle, the survivors are home.
    revive(&mut app, p0);
    tick(&mut app);
    let after = run_until(&mut app, 300, |a| state(a, &key) == RunState::Idle);
    assert_eq!(after, 120);
}

#[test]
fn a_wipe_in_a_spawned_wave_after_a_placed_wave_keeps_the_pending_wave() {
    // Finding 2: `wave == 1` after a wipe in spawned wave 2 is not a placed wave with survivors.
    let mut app = watched(1);
    let (key, p0, member) = camp(&mut app);
    kill(&mut app, member);
    tick(&mut app);
    run_until(&mut app, 200, |a| run(a, &key).wave == 2);
    assert_eq!(state(&app, &key), RunState::Running);
    kill(&mut app, p0);
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Wiped { resume: None, pulled_back: false });
    assert_eq!(run(&app, &key).wave, 1);
    revive(&mut app, p0);
    tick(&mut app);
    let after = run_until(&mut app, 300, |a| run(a, &key).wave == 2);
    assert_eq!(after, 120, "the pending wave 2 comes back, no fresh start needed");
    assert_eq!(state(&app, &key), RunState::Running);
}

#[test]
fn a_placed_pack_killed_before_the_start_is_won_and_does_not_sit_armed_forever() {
    // Finding 1: one placed wave, every member out before the start. The run is made under the
    // player's feet (`Idle` waits for the start condition to be false once), the pack is shot
    // from outside the start distance, and the player steps out and in again.
    let mut app = watched(1);
    let p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 4.0));
    let key = place(&mut app, "lone_pack", "pack", Vec3::ZERO);
    step(&mut app, 3);
    assert_eq!(state(&app, &key), RunState::Idle);
    assert_eq!(members(&app, &key).len(), 2);
    for (_, e) in members(&app, &key) {
        kill(&mut app, e);
    }
    step(&mut app, 3);
    assert_eq!(state(&app, &key), RunState::Idle, "the start condition has not been false yet");
    move_to(&mut app, p0, FAR);
    // Armed, and a member of the pack is out: the fight is on, the clear path runs, the pack is won.
    run_until(&mut app, 8, |a| state(a, &key) == RunState::Won);
    assert!(saw(&app, "completed") == 1 && saw(&app, "cleared 1") == 1 && saw(&app, "encounter-started") == 1, "{:?}", seen(&app));
    assert!(run(&app, &key).members.is_empty());
}

#[test]
fn a_load_that_spawned_nothing_does_not_leave_a_placed_pack_armed_forever_either() {
    let mut app = watched(1);
    let p0 = player(&mut app, 0, FAR);
    let key = place(&mut app, "lone_pack", "pack", Vec3::ZERO);
    step(&mut app, 3);
    // The members are gone from the world (despawned by something else) and the list is empty.
    for (_, e) in members(&app, &key) {
        app.world_mut().despawn(e);
    }
    app.world_mut().resource_mut::<shards::encounter::EncounterRuns>().runs[0].members.clear();
    step(&mut app, 3);
    move_to(&mut app, p0, Vec3::new(0.0, 0.0, 4.0));
    run_until(&mut app, 8, |a| state(a, &key) == RunState::Won);
}
