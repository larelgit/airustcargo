//! State coverage (`ai_core::coverage`): the sandbox's combat prototype played with coverage on
//! lists real signatures, names what content declares that was never reached, answers "what is
//! new since" for a fuzzer, and changes nothing about the simulation.

use std::process::Command;

use shards::ai_core::{replay, AiAppExt, AiCoverage};
use shards::ai_core::scenario::run_file;
use shards::serde_json::{json, Value};

use crate::common::{act, fails, ok, text};
use crate::scenario_files::scenario;

fn covered_app(seed: u64) -> shards::bevy::app::App {
    sandbox::test_app_with(|config| {
        config.seed = seed;
        config.coverage = true;
    })
}

/// The combat prototype, played on a game with coverage on.
fn played_fight() -> shards::bevy::app::App {
    let mut app = covered_app(42);
    let report = run_file(app.world_mut(), scenario("combat_prototype.scenario"));
    assert!(report.passed, "{report}");
    app
}

#[test]
fn coverage_is_off_by_default_and_the_tools_say_how_to_turn_it_on() {
    let mut app = sandbox::test_app(42);
    assert!(app.world().get_resource::<AiCoverage>().is_none());
    for tool in ["coverage", "coverage_reset"] {
        let error = fails(&mut app, tool, json!({}));
        assert!(error.contains("coverage is off") && error.contains("--coverage"), "{tool}: {error}");
    }
}

#[test]
fn the_combat_prototype_reaches_real_signatures() {
    let mut app = played_fight();
    let answer = text(&ok(&mut app, "coverage", json!({"top": 5}))).to_owned();
    println!("{answer}");
    let coverage = app.world().resource::<AiCoverage>();
    assert!(coverage.distinct() > 4, "{answer}");
    assert!(coverage.entity_ticks() > 100, "{answer}");
    let all: Vec<&str> = coverage.rarest(10_000).into_iter().map(|(s, _)| s).collect();
    // The player and the boss, standing and fighting: kind, the motion mode and the action with its phase.
    assert!(all.iter().any(|s| s.contains("motion=ground") && s.contains("action=idle")), "{all:#?}");
    assert!(all.iter().any(|s| s.contains("action=") && !s.contains("action=idle") && s.contains(':')), "{all:#?}");
    assert!(all.iter().any(|s| s.contains("npc=")), "the boss's behaviour state is in a signature: {all:#?}");
    assert!(all.iter().all(|s| s.starts_with("kind=")), "{all:#?}");
    // The text shape a driver can rely on.
    assert!(answer.starts_with("coverage: "), "{answer}");
    assert!(answer.lines().any(|l| l.starts_with("distinct: ")), "{answer}");
    assert!(answer.contains("rarest 5:") && answer.contains("never reached, actions"), "{answer}");
}

#[test]
fn what_content_declares_and_the_session_never_reached_is_listed() {
    let mut app = played_fight();
    // A planted kind of thing: two ids in "content", one of which the session met as an entity kind.
    let met = app.world().resource::<AiCoverage>().values_of("kind").unwrap().iter().next().cloned().unwrap();
    app.add_coverage_catalog(
        "planted ids",
        "kind",
        |world| {
            let kinds = world.resource::<AiCoverage>().values_of("kind").unwrap();
            let mut ids: Vec<String> = kinds.iter().cloned().collect();
            ids.push("ghost_kind".into());
            ids
        },
        |value| vec![value.to_owned()],
    );
    let answer: Value = shards::serde_json::from_str(text(&ok(&mut app, "coverage", json!({"json": true})))).unwrap();
    let never = answer["never_reached"].as_array().unwrap();
    let planted = never.iter().find(|n| n["what"] == "planted ids").expect("the planted catalog is listed");
    assert_eq!(planted["ids"], json!(["ghost_kind"]), "{planted}; the kind `{met}` was met");
    // The engine's own catalogs: what the fight used is not listed, what it did not use is.
    let actions = never.iter().find(|n| n["what"] == "actions").expect("actions");
    let declared = actions["declared"].as_u64().unwrap();
    let missing: Vec<&str> = actions["ids"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(declared > 0 && !missing.is_empty() && (missing.len() as u64) < declared, "{actions}");
    let reached: Vec<String> = app
        .world()
        .resource::<AiCoverage>()
        .values_of("action")
        .unwrap()
        .iter()
        .filter_map(|v| v.split_once(':').map(|(a, _)| a.to_owned()))
        .collect();
    assert!(!reached.is_empty());
    for action in &reached {
        assert!(!missing.contains(&action.as_str()), "{action} was started but is listed as never reached");
    }
    for name in ["statuses", "behaviour states"] {
        assert!(never.iter().any(|n| n["what"] == name), "{name} is missing from {never:?}");
    }
}

#[test]
fn since_tells_a_driver_whether_what_it_did_led_anywhere_new() {
    let mut app = covered_app(42);
    ok(&mut app, "step", json!({"ticks": 5}));
    let first = text(&ok(&mut app, "coverage", json!({"json": true}))).to_owned();
    let first: Value = shards::serde_json::from_str(&first).unwrap();
    let before = first["distinct"].as_u64().unwrap();
    // Nothing new while nothing happens.
    ok(&mut app, "step", json!({"ticks": 5}));
    let calm: Value = shards::serde_json::from_str(text(&ok(&mut app, "coverage", json!({"json": true, "since": before})))).unwrap();
    assert_eq!(calm["new_since"], 0, "{calm}");
    // A dodge is a new state.
    act(&mut app, "dodge", json!({}));
    ok(&mut app, "step", json!({"ticks": 20}));
    let moved: Value = shards::serde_json::from_str(text(&ok(&mut app, "coverage", json!({"json": true, "since": before})))).unwrap();
    assert!(moved["new_since"].as_u64().unwrap() > 0, "{moved}");
    let plain = text(&ok(&mut app, "coverage", json!({"since": before}))).to_owned();
    assert!(plain.lines().any(|l| l.starts_with(&format!("new since {before}: "))), "{plain}");
    assert!(plain.contains("  + kind="), "{plain}");
}

#[test]
fn reset_forgets_everything_and_counting_goes_on() {
    let mut app = played_fight();
    assert!(app.world().resource::<AiCoverage>().distinct() > 0);
    let said = text(&ok(&mut app, "coverage_reset", json!({}))).to_owned();
    assert!(said.contains("coverage cleared"), "{said}");
    assert_eq!(app.world().resource::<AiCoverage>().distinct(), 0);
    ok(&mut app, "step", json!({"ticks": 3}));
    assert!(app.world().resource::<AiCoverage>().distinct() > 0);
}

#[test]
fn coverage_changes_nothing_the_simulation_does() {
    let record = |coverage: bool| {
        let mut app = sandbox::test_app_with(|config| {
            config.seed = 42;
            config.coverage = coverage;
        });
        let report = run_file(app.world_mut(), scenario("combat_prototype.scenario"));
        assert!(report.passed, "{report}");
        shards::serde_json::to_value(replay::save(app.world())).unwrap()
    };
    let (off, on) = (record(false), record(true));
    assert_eq!(off, on, "the recording of a game with coverage on differs from one with it off");
}

/// The reference replay verifies on a game that keeps coverage, the way CI verifies it without.
#[test]
fn the_reference_replay_is_identical_with_coverage_on() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/combat_prototype.json");
    let out = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .arg("--replay")
        .arg(&file)
        .arg("--verify")
        .arg("--coverage")
        .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.code() == Some(0) && stdout.contains("identical to the recording"),
        "exit {:?}\n{stdout}{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}
