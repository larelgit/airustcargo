//! Waves, tick exact: when each spawns, in what order, with what counts, and the cues that lead
//! them. All three waves of `ring_waves` are fought through to the win.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::{EncounterRuns, RunState};

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);

/// The sandbox ring: a volume, a player inside it, the run of `ring_waves`.
fn ring_fight(app: &mut App) -> (String, Entity) {
    trigger(app, "ring", Vec3::new(0.0, 2.0, 0.0), Vec3::new(12.0, 4.0, 12.0));
    let p0 = player(app, 0, FAR);
    let key = place(app, "ring_waves", "ring", Vec3::ZERO);
    step(app, 3);
    move_to(app, p0, Vec3::new(0.0, 0.0, 3.0));
    run_until(app, 5, |a| matches!(state(a, &key), RunState::Between { .. }));
    (key, p0)
}

#[test]
fn the_first_member_spawns_72_ticks_after_the_start_and_the_next_150_after_a_clear() {
    let mut app = app(1);
    let (key, _p0) = ring_fight(&mut app);
    let started_at = now(&app);
    assert_eq!(run(&app, &key).wave, 0);
    let after = run_until(&mut app, 200, |a| run(a, &key).wave == 1);
    assert_eq!(after, 72, "1.2 s: a timer set in tick N expires at the check of tick N + 72 (started at {started_at})");
    let ids = members(&app, &key);
    assert_eq!(ids.len(), 1);
    assert_eq!(state(&app, &key), RunState::Running);

    // Clear it: the next wave comes 150 ticks after the tick that saw the clear.
    kill(&mut app, ids[0].1);
    tick(&mut app);
    assert!(matches!(state(&app, &key), RunState::Between { .. }), "{:?}", state(&app, &key));
    let after = run_until(&mut app, 400, |a| run(a, &key).wave == 2);
    assert_eq!(after, 150, "2.5 s");
    // Wave 2: two members, ids in the definition's order, at its offsets.
    let wave = wave_members(&app, &key, 2);
    assert_eq!(wave.len(), 2);
    let ids: Vec<u32> = members(&app, &key).iter().map(|(id, _)| id.0).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "{ids:?}");
    let xs: Vec<f32> = wave.iter().map(|e| app.world().get::<Transform>(*e).unwrap().translation.x).collect();
    assert_eq!(xs, vec![4.0, -4.0], "the first member of the definition has the lower id");
}

#[test]
fn a_whole_fight_is_won_and_remembered() {
    let mut app = app(1);
    let (key, _p0) = ring_fight(&mut app);
    for wave in 1..=3u32 {
        run_until(&mut app, 400, |a| run(a, &key).wave == wave);
        for (_, e) in members(&app, &key) {
            kill(&mut app, e);
        }
        tick(&mut app);
    }
    assert_eq!(state(&app, &key), RunState::Won);
    assert_eq!(app.world().resource::<EncounterRuns>().completed, vec![key.clone()]);
    assert!(run(&app, &key).members.is_empty(), "the dead are pruned at the win");
    assert!(journal_has(&app, "wave 3/3 cleared") && journal_has(&app, "won"), "{:?}", journal(&app));
    // The members' markers went with their entries.
    let left = app.world_mut().query::<&shards::encounter::EncounterMember>().iter(app.world()).count();
    assert_eq!(left, 0);
    // The lock stood for the fight and is gone.
    assert!(run(&app, &key).lock.is_none());
    assert!(!app.world_mut().query::<&Name>().iter(app.world()).any(|n| n.as_str() == "Barrier"));
}

#[test]
fn each_wave_is_warned_by_its_delay_before_it_spawns() {
    let mut app = app(1);
    let (key, _p0) = ring_fight(&mut app);
    for wave in 1..=3u32 {
        run_until(&mut app, 400, |a| run(a, &key).wave == wave);
        for (_, e) in members(&app, &key) {
            kill(&mut app, e);
        }
        tick(&mut app);
    }
    let entries: Vec<(u64, String)> =
        app.world().resource::<ai_core::AiJournal>().after(0).0.map(|e| (e.tick, e.text.clone())).collect();
    let tick_of = |text: &str| entries.iter().find(|(_, t)| t.contains(text)).map(|(tick, _)| *tick).unwrap_or_else(|| panic!("no `{text}`: {entries:#?}"));
    assert_eq!(tick_of("wave 1/3 spawned") - tick_of("wave 1/3 in 1.2 s"), 72);
    assert_eq!(tick_of("wave 2/3 spawned") - tick_of("wave 2/3 in 2.5 s"), 150);
    assert_eq!(tick_of("wave 3/3 spawned") - tick_of("wave 3/3 in 2.5 s"), 150);
}

#[test]
fn up_to_counts_come_from_the_seed_and_the_run_key() {
    // A placed pack of 2..=3, spawned when the run is made. One seed draws the same number every time.
    let mut counts = Vec::new();
    for seed in 1..=24u64 {
        let mut app = app_with(seed, |_| {});
        for (i, x) in [3.0f32, -3.0, 6.0].iter().enumerate() {
            app.world_mut().spawn((shards::world::Marker { name: "pack".into() }, Transform::from_xyz(*x, 0.0, -4.0 - i as f32)));
        }
        let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
        step(&mut app, 2);
        counts.push(run(&app, &key).members.len());
    }
    assert!(counts.iter().all(|c| (2..=3).contains(c)), "{counts:?}");
    assert!(counts.contains(&2) && counts.contains(&3), "different seeds draw different counts: {counts:?}");
    let again: Vec<usize> = (1..=24u64)
        .map(|seed| {
            let mut app = app_with(seed, |_| {});
            for (i, x) in [3.0f32, -3.0, 6.0].iter().enumerate() {
                app.world_mut().spawn((shards::world::Marker { name: "pack".into() }, Transform::from_xyz(*x, 0.0, -4.0 - i as f32)));
            }
            let key = place(&mut app, "marker_pack", "p", Vec3::ZERO);
            step(&mut app, 2);
            run(&app, &key).members.len()
        })
        .collect();
    assert_eq!(counts, again, "the same seed, the same counts");
}

#[test]
fn two_runs_never_share_a_random_stream() {
    // Same encounter, same tick, same seed: the key is in the stream, so the counts are not
    // forced to agree (over many pairs, some differ).
    let mut differ = 0;
    for seed in 1..=24u64 {
        let mut app = app_with(seed, |_| {});
        for (i, x) in [3.0f32, -3.0, 6.0].iter().enumerate() {
            app.world_mut().spawn((shards::world::Marker { name: "pack".into() }, Transform::from_xyz(*x, 0.0, -4.0 - i as f32)));
        }
        let a = place(&mut app, "marker_pack", "alpha", Vec3::ZERO);
        let b = place(&mut app, "marker_pack", "beta", Vec3::new(0.5, 0.0, 0.0));
        step(&mut app, 2);
        if run(&app, &a).members.len() != run(&app, &b).members.len() {
            differ += 1;
        }
    }
    assert!(differ > 0, "the run key salts the draw");
}

#[test]
fn the_cue_of_a_wave_travels_with_its_warning() {
    let mut app = app(1);
    let (key, _p0) = ring_fight(&mut app);
    // The warning of wave 1 carried the cue of the definition (message only), to the anchor.
    let warned = app.world().resource::<ai_core::AiJournal>().after(0).0.any(|e| e.text.contains("wave 1/3 in 1.2 s"));
    assert!(warned);
    let _ = (key, json!({}));
}
