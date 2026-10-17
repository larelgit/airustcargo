//! `shards fuzz` in process: the seeded inputs of `shards::script::fuzz` (every player action
//! tool, arguments from its schema, developer calls in between) play the sandbox through
//! `WorldDriver`, the way the CLI plays a live game through BRP.

use shards::ai_core::replay;
use shards::ai_core::scenario::{tool_list, WorldDriver};
use shards::prelude::*;
use shards::script::fuzz::{self, Alphabet, Ended, FuzzPlan, FuzzReport};

/// Plays `plan` on `app`. A fuzz run writes snapshots/ and replays/ in the working folder.
fn play(app: &mut App, plan: &FuzzPlan) -> FuzzReport {
    let tools = tool_list(app.world());
    fuzz::run(&mut WorldDriver { world: app.world_mut() }, plan, &tools).unwrap()
}

fn plan(seed: u64, stem: &str) -> FuzzPlan {
    FuzzPlan { rounds: 120, chaos: 0.1, invalid: 0.1, stem: format!("{stem}-{}", std::process::id()), ..FuzzPlan::new(seed) }
}

#[test]
fn a_fuzz_run_with_chaos_leaves_a_session_that_plays_back() {
    let _turn = crate::scenario_files::snapshot_file_turn();
    let mut app = sandbox::test_app(42);
    let plan = plan(7, "fuzz-chaos");
    let report = play(&mut app, &plan);
    assert_eq!(report.ended, Ended::Done, "{report}: {report:#?}");
    // It played player actions of the game's own list, broke some calls on purpose, and shook the world.
    for tool in ["move", "attack", "dodge", "light_attack"] {
        assert!(report.calls.contains_key(tool), "{tool} was never played: {:?}", report.calls);
    }
    assert!(report.invalid > 0 && report.chaos_rounds > 0, "{report:?}");
    // In the sandbox every chaos call works: a refusal here is a bug, and the report names it.
    assert_eq!(report.chaos_refused, 0, "{:?}", report.first_chaos_refusal);
    assert!(report.calls.contains_key("snapshot_load") || report.calls.contains_key("load_game"), "{:?}", report.calls);
    // The session, snapshots loads and all, is a replay that plays back.
    let recording = replay::save(app.world());
    let played = shards::testing::verify(&mut sandbox::test_app(42), recording).expect("plays");
    assert!(played.identical(), "{}", played.headline());
    let _ = std::fs::remove_file(format!("snapshots/{}.json", plan.stem));
    let _ = std::fs::remove_dir("snapshots");
}

#[test]
fn the_same_seed_plays_the_same_game() {
    let _turn = crate::scenario_files::snapshot_file_turn();
    let run = |seed| {
        let mut app = sandbox::test_app(42);
        let report = play(&mut app, &plan(seed, "fuzz-same"));
        (report.played, report.to_tick)
    };
    assert_eq!(run(3), run(3));
    assert_ne!(run(3).0, run(4).0);
    let _ = std::fs::remove_file(format!("snapshots/fuzz-same-{}.json", std::process::id()));
    let _ = std::fs::remove_dir("snapshots");
}

#[test]
fn the_classic_alphabet_plays_the_sandbox_too() {
    let mut app = sandbox::test_app(42);
    let report = play(&mut app, &FuzzPlan { alphabet: Alphabet::Classic, rounds: 60, chaos: 0.0, ..FuzzPlan::new(1) });
    assert_eq!(report.ended, Ended::Done, "{report}");
    assert!(report.calls.keys().all(|t| ["observe", "move", "attack", "light_attack", "dodge", "step"].contains(&t.as_str())), "{:?}", report.calls);
}

fn cracks_after_tick_20(tick: Res<SimTick>) {
    if tick.0 >= 20 {
        panic!("planted: a gameplay system gives way");
    }
}

#[test]
fn a_game_that_breaks_halts_the_run_and_the_report_says_why() {
    let _turn = crate::scenario_files::snapshot_file_turn();
    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, cracks_after_tick_20.in_set(SimSet::Resolve));
    let report = play(&mut app, &FuzzPlan { rounds: 200, chaos: 0.0, ..FuzzPlan::new(5) });
    let Ended::Halted { reason, .. } = &report.ended else { panic!("the planted panic was not found: {report}") };
    assert!(reason.contains("cracks_after_tick_20 panicked"), "{reason}");
    assert!(report.rounds < 200 && report.halted(), "{report}");
    // The rounds that led there are kept, for whoever shrinks them.
    assert_eq!(report.played.len() as u32, report.rounds);
}

/// A world fact no snapshot holds: it differs every time it is read, so a snapshot never loads.
fn drifts(_: &World) -> u64 {
    static READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn a_snapshot_that_does_not_load_back_ends_the_run_as_a_finding() {
    let _turn = crate::scenario_files::snapshot_file_turn();
    let mut app = sandbox::test_app(42);
    app.add_snapshot_check("drifts", "state outside the snapshot", drifts);
    let plan = FuzzPlan { rounds: 300, chaos: 0.3, ..plan(7, "fuzz-drift") };
    let report = play(&mut app, &FuzzPlan { save_replay: true, ..plan.clone() });
    let Ended::ChaosBroke { call, why } = &report.ended else { panic!("the planted drift was not found: {report}") };
    assert!(call.starts_with("snapshot_load"), "{call}");
    assert!(!why.is_empty(), "the game's answer is kept");
    assert!(report.chaos_broke() && report.chaos_refused >= 1 && report.first_chaos_refusal.is_some(), "{report:?}");
    assert!(report.to_string().contains(call.as_str()), "{report}");
    assert!(report.replay.as_deref().is_some_and(|r| r.contains("fuzz-drift") && r.contains("-tick")), "{:?}", report.replay);
    let _ = std::fs::remove_file(format!("snapshots/{}.json", plan.stem));
    let _ = std::fs::remove_dir("snapshots");
}
