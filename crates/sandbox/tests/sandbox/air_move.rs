//! Air movement in the style of Half-Life (F7) on the sandbox: the first-person `air_viewer` locomotion
//! (`characters/air_move.locomotion.ron`) and the `air_runner` prefab that moves by it. `scenarios/air_move.scenario`
//! plays the run, the stop by friction, the air's cap and a bunny hop; the controller's own tests are
//! `shards_motion`'s `tests/motion/air_move.rs`.

use shards::ai_core::AiIndex;
use shards::motion::{AccelModel, CharacterController, CharacterParams, Facing};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::{ok, step, text};

#[test]
fn the_air_viewer_is_half_lifes_model_written_out_and_the_runner_prefab_moves_by_it() {
    let mut app = sandbox::test_app(42);
    let params = app.world().resource::<shards::content::ContentDb>().get::<CharacterParams>("air_viewer").cloned().expect("the locomotion is content");
    assert_eq!(params.face, Facing::Look, "a first-person body");
    let AccelModel::Projected { ground_accel, air_accel, air_speed_cap, friction, stop_speed, max_air_speed } = params.accel else {
        panic!("{:?}", params.accel);
    };
    // Half-Life's numbers in metres, as the module's defaults have them (the file rounds 0.762 to 0.76).
    assert_eq!((ground_accel, air_accel, friction, stop_speed), (10.0, 10.0, 4.0, 2.54));
    assert!((air_speed_cap - 0.762).abs() < 0.01 && max_air_speed.is_none(), "{air_speed_cap} {max_air_speed:?}");
    assert!((params.run_speed - 320.0 * 0.0254).abs() < 0.01, "a run of 320 units a second: {}", params.run_speed);

    let spawned = text(&ok(&mut app, "spawn_prefab", json!({"id": "air_runner", "x": -10, "z": 12, "y": 0}))).to_owned();
    assert!(spawned.contains("air_runner"), "{spawned}");
    let id = spawned.split_whitespace().find(|w| w.starts_with('e') && w[1..].chars().all(|c| c.is_ascii_digit())).expect("the new body's id").to_owned();
    step(&mut app, 3);
    let runner = app.world().resource::<AiIndex>().get(shards::ai_core::AiId::parse(&id).unwrap()).unwrap();
    assert_eq!(app.world().get::<CharacterController>(runner).unwrap().params, "air_viewer");
    let info: Value = ok(&mut app, "move_info", json!({"id": id})).result;
    assert_eq!(info["accel"]["model"], "projected", "{info}");
    assert_eq!(info["accel"]["friction"].as_f64(), Some(4.0));
    // The player's own locomotion is the default model, untouched.
    let player: Value = ok(&mut app, "move_info", json!({"id": "e5"})).result;
    assert_eq!(player["accel"]["model"], "step", "{player}");
}

#[test]
fn the_air_move_scenario_passes_on_its_own() {
    let mut app = sandbox::test_app(42);
    let report = shards::ai_core::scenario::run_file(app.world_mut(), crate::scenario_files::scenario("air_move.scenario"));
    assert!(report.passed, "{report}");
}

#[test]
fn the_air_move_replay_continues_alike_from_its_snapshots() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/air_move.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .arg("--replay")
        .arg(file)
        .args(["--verify", "--heavy", "10"])
        .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let verdict = stdout.lines().find(|l| l.starts_with("replay ")).unwrap_or_default();
    assert!(verdict.contains("identical to the recording"), "{stdout}");
    assert!(verdict.contains(", 0 skipped"), "{verdict}");
    let checked: u64 = verdict.split("heavy: ").nth(1).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(checked >= 30, "a twin every 10 ticks: {verdict}");
}
