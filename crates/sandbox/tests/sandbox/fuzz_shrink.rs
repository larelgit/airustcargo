//! A fuzz run that stops is shrunk on fresh games (`shards::testing::fuzz_and_shrink`) and written as
//! a regression scenario that fails while the bug is there and passes without it.

use shards::ai_core::scenario::run_str;
use shards::prelude::*;
use shards::script::fuzz::{Ended, FuzzPlan};
use shards::script::regress::command_count;
use shards::script::shrink::replays_stop;
use shards::script::stop::Stop;
use shards::testing::fuzz_and_shrink;

/// A planted bug that needs the player to have spent stamina: it takes a few attacks or dodges.
fn gives_way_when_winded(player: Query<&shards::action::Stamina, With<sandbox::gameplay::Player>>) {
    if player.iter().any(|s| s.current < 80.0) {
        panic!("planted: a gameplay system gives way when winded");
    }
}

fn planted() -> App {
    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, gives_way_when_winded.in_set(SimSet::Resolve));
    app
}

#[test]
fn a_planted_bug_is_found_shrunk_and_written_as_a_scenario_that_fails_with_it_and_passes_without() {
    let plan = FuzzPlan { rounds: 300, chaos: 0.0, ..FuzzPlan::new(5) };
    let fuzzed = fuzz_and_shrink(|_| planted(), &plan, 300, std::time::Duration::from_secs(300)).unwrap();
    let finding = fuzzed.finding.unwrap_or_else(|| panic!("the planted bug was not found: {} {:?}", fuzzed.report, fuzzed.report.calls));
    assert!(matches!(&finding.stop, Stop::Failure { name, .. } if name.contains("gives_way_when_winded")), "{:?}", finding.stop);
    assert!(finding.reproduced, "a fresh game stops on it too");
    let (before, after) = (finding.before.len(), finding.shrunk.rounds.len());
    assert!(after < before || before <= 3, "{before} rounds shrank to {after}");
    assert!(finding.shrunk.attempts <= 300, "{}", finding.shrunk.attempts);

    // The scenario: a header, the seed, calls and steps; it fails on the planted game...
    let text = finding.scenario(42, None);
    println!("{text}");
    assert!(text.starts_with("# regress: "), "{text}");
    assert!(command_count(&text) <= 40, "{text}");
    let mut with_bug = planted();
    let report = run_str(with_bug.world_mut(), "planted.scenario", &text);
    assert!(!report.passed, "the scenario must fail while the bug exists:\n{report}");
    // ...and passes on the game without it.
    let mut fixed = sandbox::test_app(42);
    let report = run_str(fixed.world_mut(), "fixed.scenario", &text);
    assert!(report.passed, "{report}");

    // The shrunk session stops on the same thing, asked directly.
    let mut again = planted();
    let mut driver = shards::ai_core::scenario::WorldDriver { world: again.world_mut() };
    assert!(replays_stop(&mut driver, &plan.agent, &finding.shrunk.rounds, &finding.stop).unwrap());
}

#[test]
fn a_run_that_does_not_stop_has_nothing_to_shrink() {
    let plan = FuzzPlan { rounds: 15, chaos: 0.0, ..FuzzPlan::new(3) };
    let fuzzed = fuzz_and_shrink(|_| sandbox::test_app(42), &plan, 50, std::time::Duration::from_secs(60)).unwrap();
    assert_eq!(fuzzed.report.ended, Ended::Done);
    assert!(fuzzed.finding.is_none());
}

/// The planted heavy bugs of `heavy.rs`: state outside the snapshot makes the twin go another way.
#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Drift(u32);

#[derive(Component, Reflect, Default, Clone)]
#[reflect(Component)]
struct Marked(u32);

fn mark(mut commands: Commands, bodies: Query<Entity, (With<sandbox::gameplay::Player>, Without<Marked>)>) {
    for body in &bodies {
        commands.entity(body).insert(Marked(0));
    }
}

fn drift(mut drift: ResMut<Drift>, mut marked: Query<&mut Marked>) {
    drift.0 += 1;
    for mut m in &mut marked {
        m.0 = m.0.wrapping_add(drift.0);
    }
}

fn heavy_planted(fixed: bool) -> App {
    shards::testing::heavy_app(
        |config| config.seed = 42,
        move |app| {
            app.add_plugins(sandbox::SandboxPlugin)
                .register_type::<Marked>()
                .init_resource::<Drift>()
                .add_systems(FixedUpdate, (mark.in_set(SimSet::Cleanup), drift.in_set(SimSet::Cleanup).after(mark)));
            if fixed {
                app.add_snapshot_resource::<Drift>();
            }
        },
        shards::HeavyConfig { every: 30, span: 30, offset: 0 },
    )
}

#[test]
fn a_heavy_failure_is_shrunk_on_heavy_games_and_the_scenario_asks_for_heavy_mode() {
    let plan = FuzzPlan { rounds: 60, chaos: 0.0, ..FuzzPlan::new(9) };
    let fuzzed = fuzz_and_shrink(|_| heavy_planted(false), &plan, 100, std::time::Duration::from_secs(300)).unwrap();
    let finding = fuzzed.finding.expect("the planted heavy bug was not found");
    assert!(matches!(finding.stop, Stop::Heavy { .. }), "{:?}", finding.stop);
    // The headline, then what differed: the campaign's log names it, not only the tick.
    let mut lines = finding.text.lines();
    assert!(lines.next().is_some_and(|l| l.starts_with("heavy check failed at tick")), "{}", finding.text);
    assert!(lines.any(|l| l.contains("Marked") && l.contains("differs")), "{}", finding.text);
    assert!(finding.reproduced);
    assert!(finding.shrunk.rounds.len() <= finding.before.len());
    let text = finding.scenario(42, Some((30, 30, 0)));
    println!("{text}");
    assert!(text.contains("# heavy: every=30 span=30 offset=0"), "{text}");
    // Played with heavy mode on it fails; without the bug it passes.
    let mut with_bug = heavy_planted(false);
    assert!(!run_str(with_bug.world_mut(), "heavy.scenario", &text).passed);
    let mut fixed = heavy_planted(true);
    let report = run_str(fixed.world_mut(), "heavy.scenario", &text);
    assert!(report.passed, "{report}");
}

#[test]
fn a_chaos_round_trip_that_does_not_come_back_is_shrunk_to_the_save_and_the_load() {
    use shards::ai_core::AiAppExt;
    // The planted fact of `fuzz.rs`: it differs every time it is read, so a snapshot never loads.
    fn drifts(_: &World) -> u64 {
        static READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        READS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
    let _turn = crate::scenario_files::snapshot_file_turn();
    let build = || {
        let mut app = sandbox::test_app(42);
        app.add_snapshot_check("drifts", "state outside the snapshot", drifts);
        app
    };
    let plan = FuzzPlan { rounds: 300, chaos: 0.3, stem: format!("shrink-drift-{}", std::process::id()), ..FuzzPlan::new(7) };
    let fuzzed = fuzz_and_shrink(|_| build(), &plan, 200, std::time::Duration::from_secs(300)).unwrap();
    let finding = fuzzed.finding.expect("the planted drift was not found");
    assert!(matches!(&finding.stop, Stop::Chaos { tool } if tool == "snapshot_load"), "{:?}", finding.stop);
    let calls: Vec<&str> = finding.shrunk.rounds.iter().flat_map(|r| r.calls.iter().map(|(t, _)| t.as_str())).collect();
    assert_eq!(calls, ["snapshot_save", "snapshot_load"], "{:?}", finding.shrunk);
    // The load that is refused is a strict `act`: a refusal fails the file.
    let text = finding.scenario(42, None);
    assert!(text.contains("act? snapshot_save") && text.contains("\nact snapshot_load"), "{text}");
    let mut with_bug = build();
    assert!(!run_str(with_bug.world_mut(), "drift.scenario", &text).passed);
    let mut fixed = sandbox::test_app(42);
    let report = run_str(fixed.world_mut(), "drift.scenario", &text);
    assert!(report.passed, "{report}");
    let _ = std::fs::remove_file(format!("snapshots/{}.json", plan.stem));
    let _ = std::fs::remove_dir("snapshots");
}
