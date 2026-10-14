//! `testing::run_to` and `testing::verify` when the tick that breaks a rule also starts a
//! load: a region change loads behind the gate, and the run waits it out as playback does.

use shards::ai_core::replay;
use shards::ai_core::{sim, SimGate};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::{call, ok, run_to, test_app, verify, wait_until_loaded, LOAD_TIMEOUT};

/// Files the game still loads: the requirement is pending while it is above 0.
#[derive(Resource, Default)]
struct Files(u32);

fn files_status(world: &World) -> LoadStatus {
    match world.resource::<Files>().0 {
        0 => LoadStatus::Ready,
        n => LoadStatus::Pending(format!("{n} files left")),
    }
}

/// One file arrives per frame.
fn arrive(mut files: ResMut<Files>) {
    files.0 = files.0.saturating_sub(1);
}

/// What a region change does at the end of its tick: asks for files and holds the gate.
fn start_loading(mut files: ResMut<Files>, mut gate: ResMut<SimGate>, mut ticks: Local<u32>) {
    *ticks += 1;
    if *ticks == 3 {
        files.0 = 3;
        gate.hold("files", "3 files left");
    }
}

/// Broken while the game loads: on the very tick that started it.
fn quiet_while_loading(world: &World) -> Option<String> {
    world.resource::<SimGate>().is_held().then(|| "the rule saw the gate held".to_owned())
}

fn loading_game() -> App {
    test_app(|_| {}, |app| {
        app.init_resource::<Files>()
            .require_loaded("files", files_status)
            .add_ai_invariant("quiet_while_loading", quiet_while_loading)
            .add_systems(Update, arrive)
            .add_systems(FixedUpdate, start_loading.in_set(SimSet::Resolve));
    })
}

#[test]
fn run_to_waits_out_a_load_that_starts_on_the_tick_a_rule_breaks() {
    let mut app = loading_game();
    let stops = run_to(&mut app, 8, LOAD_TIMEOUT).expect("the load is waited out, not an error");
    let violations: Vec<String> = stops.violations.iter().map(|v| v.headline()).collect();
    assert_eq!(violations, ["invariant quiet_while_loading violated at tick 3"]);
    assert!(stops.failures.is_empty());
    assert_eq!(sim::tick(app.world()), 8, "the run went on to its end after the load");
    assert!(!app.world().resource::<SimGate>().is_held());
}

#[test]
fn run_to_waits_out_a_load_that_starts_on_the_last_tick() {
    let mut app = loading_game();
    let stops = run_to(&mut app, 3, LOAD_TIMEOUT).expect("the load is waited out, not an error");
    assert_eq!(stops.violations.len(), 1);
    assert_eq!(sim::tick(app.world()), 3);
    assert!(!app.world().resource::<SimGate>().is_held(), "nothing is left holding the game");
}

#[test]
fn verify_lists_the_violation_instead_of_failing_to_play() {
    // The recording: the same ticks, driven by the tool (it stops where the rule broke).
    let mut recorded = loading_game();
    let stopped = call(&mut recorded, "step", json!({"ticks": 8}));
    assert!(stopped.result.as_str().unwrap().contains("stopped: invariant quiet_while_loading violated at tick 3"));
    wait_until_loaded(&mut recorded, LOAD_TIMEOUT).unwrap();
    ok(&mut recorded, "step", json!({"ticks": 5}));
    let recording = replay::save(recorded.world());
    assert_eq!(recording.end.tick, 8);

    let mut app = loading_game();
    let err = verify(&mut app, recording).unwrap_err();
    assert_eq!(err, "invariant quiet_while_loading violated at tick 3: the rule saw the gate held");
}
