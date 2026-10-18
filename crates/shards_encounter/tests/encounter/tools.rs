//! The tools, conditions, status and describer.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::RunState;

use crate::common::*;

#[test]
fn without_runs_the_queries_say_so_and_errors_teach_the_fix() {
    let mut app = app(1);
    assert_eq!(ok(&mut app, "encounters", json!({})), "no encounters");
    let text = err(&mut app, "encounter", json!({"key": "world/x"}));
    assert!(text.contains("no encounter run `world/x`; there are none (`encounters` lists them)"), "{text}");
    let text = err(&mut app, "encounter_start", json!({"key": "world/x"}));
    assert!(text.contains("no encounter run"), "{text}");
}

#[test]
fn encounters_and_encounter_show_the_run() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "manual", "m", Vec3::ZERO);
    step(&mut app, 2);
    let list = ok(&mut app, "encounters", json!({}));
    assert!(list.starts_with("1 encounter run\n  world/m manual Armed wave 0/2 members 0 alive () absent 0.0 s"), "{list}");
    assert!(list.ends_with("completed: []"), "{list}");

    ok(&mut app, "encounter_start", json!({"key": key}));
    let list = ok(&mut app, "encounters", json!({}));
    assert!(list.contains("world/m manual Between 1.0 s wave 0/2"), "{list}");
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    let ids: Vec<String> = members(&app, &key).iter().map(|(id, _)| id.to_string()).collect();
    let list = ok(&mut app, "encounters", json!({}));
    assert!(list.contains(&format!("world/m manual Running wave 1/2 members 1 alive ({})", ids[0])), "{list}");

    let full = ok(&mut app, "encounter", json!({"key": "world/m"}));
    assert!(full.starts_with("encounter run world/m (manual): Running, wave 1"), "{full}");
    assert!(full.contains("start: manual: only `encounter_start` or start_encounter starts it"), "{full}");
    assert!(full.contains(&format!("{} wave 1 counted alive", ids[0])), "{full}");
    assert!(full.contains("definition: 2 waves, budget none, tags []"), "{full}");
    assert!(full.contains("wave 1: delay 1.0 s: grunt x1"), "{full}");
}

#[test]
fn the_start_condition_is_explained_in_encounter() {
    let mut app = app(1);
    trigger(&mut app, "ring", Vec3::new(0.0, 2.0, 0.0), Vec3::new(12.0, 4.0, 12.0));
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 40.0));
    let _p1 = player(&mut app, 1, Vec3::new(0.0, 0.0, 3.0));
    let key = place(&mut app, "ring_waves", "ring", Vec3::ZERO);
    step(&mut app, 2);
    let full = ok(&mut app, "encounter", json!({"key": key}));
    assert!(full.contains("start: all_players_in: 1/2 standing participants inside ring"), "{full}");
}

#[test]
fn start_with_a_wave_skips_ahead_and_refuses_what_is_not_there() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "manual", "m", Vec3::ZERO);
    step(&mut app, 2);
    let text = err(&mut app, "encounter_start", json!({"key": key, "wave": 3}));
    assert!(text.contains("wave 3 is outside 1..=2 for encounter `manual`"), "{text}");
    ok(&mut app, "encounter_start", json!({"key": key, "wave": 2}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 2);
}

#[test]
fn misspelt_keys_get_a_did_you_mean() {
    let mut app = app(1);
    place(&mut app, "manual", "ring", Vec3::ZERO);
    step(&mut app, 2);
    let text = err(&mut app, "encounter", json!({"key": "world/rng"}));
    assert!(text.contains("no encounter run `world/rng` (did you mean `world/ring`?)") && text.contains("runs: world/ring"), "{text}");
}

#[test]
fn conditions_follow_the_state_for_step_until() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "manual", "m", Vec3::ZERO);
    step(&mut app, 2);
    let cond = |app: &App, name: &str| app.world().resource::<ai_core::AiConditions>().0[name](app.world());
    assert!(!cond(&app, "encounter_active"));
    ok(&mut app, "encounter_start", json!({"key": key}));
    assert!(cond(&app, "encounter_active") && cond(&app, "encounter_between") && !cond(&app, "encounter_won"));
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    assert!(!cond(&app, "encounter_between"));
    let out = ok(&mut app, "step", json!({"ticks": 1000, "until": [{"named": "encounter_wiped"}]}));
    let _ = out;
    for wave in 1..=2u32 {
        run_until(&mut app, 300, |a| run(a, &key).wave == wave);
        for (_, e) in members(&app, &key) {
            kill(&mut app, e);
        }
        tick(&mut app);
    }
    assert_eq!(state(&app, &key), RunState::Won);
    assert!(cond(&app, "encounter_won") && !cond(&app, "encounter_active"));
}

#[test]
fn the_status_and_the_describer_show_a_fight() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "manual", "m", Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": key}));
    run_until(&mut app, 120, |a| run(a, &key).wave == 1);
    let line = ai_core::status::status(app.world());
    assert_eq!(line["encounter"], json!(["encounter world/m wave 1/2 Running"]), "{line}");
    let observed = ok(&mut app, "observe", json!({}));
    assert!(observed.contains("encounter world/m wave 1"), "{observed}");
}
