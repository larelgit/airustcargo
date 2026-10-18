//! A definition that changes under a run (an editor session; replays never reload content).

use shards::prelude::*;
use serde_json::json;
use shards::encounter::{EncounterRuns, RunState};

use crate::common::*;

/// `extra.encounter.ron` with `ring_lock` cut to one wave.
const SHORTER: &str = r#"{ "ring_lock": (radius: 20.0, start: Manual, lock: Some("barrier"),
    waves: [(delay: 0.5, members: [(prefab: "grunt", at: Offsets([(0.0, 0.0, -6.0)]))])]) }"#;

fn in_wave_three(app: &mut App) -> String {
    let _p0 = player(app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(app, "ring_lock", "l", Vec3::ZERO);
    step(app, 2);
    ok(app, "encounter_start", json!({"key": key}));
    for wave in 1..=3u32 {
        run_until(app, 200, |a| run(a, &key).wave == wave);
        if wave < 3 {
            for (_, e) in members(app, &key) {
                kill(app, e);
            }
            tick(app);
        }
    }
    key
}

#[test]
fn a_definition_whose_waves_shrank_under_a_run_says_so_once_and_resets_the_run() {
    let mut app = watched(1);
    let key = in_wave_three(&mut app);
    assert_eq!(run(&app, &key).wave, 3);
    let db = db_edited(&app, &[("extra.encounter.ron", SHORTER)]);
    shards::content::install(app.world_mut(), db);
    step(&mut app, 3);
    assert_eq!(count(&app, "its definition now has 1 waves, the run was in wave 3; reset"), 1, "{:?}", journal(&app));
    assert_eq!(saw(&app, "reset Content"), 1);
    assert!(run(&app, &key).wave <= 1);
    assert!(matches!(state(&app, &key), RunState::Resetting { .. } | RunState::Idle | RunState::Armed), "{:?}", state(&app, &key));
    // Said once, whatever happens next.
    step(&mut app, 60);
    assert_eq!(count(&app, "its definition now has"), 1);
}

#[test]
fn a_definition_that_is_gone_drops_the_run_with_its_members() {
    let mut app = watched(1);
    let key = in_wave_three(&mut app);
    let member = members(&app, &key)[0].1;
    let db = db_edited(&app, &[("extra.encounter.ron", "{}")]);
    shards::content::install(app.world_mut(), db);
    step(&mut app, 2);
    assert!(app.world().get_entity(member).is_err());
    assert!(app.world().get_resource::<EncounterRuns>().is_none_or(|r| r.runs.is_empty()));
    assert!(journal_has(&app, "its definition `ring_lock` is gone; the run is dropped"), "{:?}", journal(&app));
}
