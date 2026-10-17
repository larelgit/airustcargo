//! Every file in `scenarios/` runs in-process on a fresh game, the way
//! `shards scenario FILE` runs it against a live one: the same commands, expectations
//! and invariants, no window. A file with a `stand` line runs on a stand built from it
//! (a host and its clients on an in-memory network), the others on a game of their own.

use std::path::{Path, PathBuf};

use shards::ai_core::scenario::run_file;
use shards::net::Stand;
use shards::script::Script;
use shards::serde_json::json;
use shards::testing::{run_stand_scenario, StandRun};

use crate::common::ok;

/// The `*.scenario` files directly in `dir`, in file-name order.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "scenario"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no *.scenario files in {}", dir.display());
    files
}

/// The folder of the single-game scenarios: what `shards scenario crates/sandbox/scenarios/*.scenario`
/// (docs/manual.md, CI) feeds to one live game.
fn single_game_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

/// The folder of the stand scenarios (a `stand` line): below the single-game one, so that glob
/// does not reach them.
fn stand_dir() -> PathBuf {
    single_game_dir().join("stand")
}

/// Whether the file starts with a `stand` line.
fn is_stand(file: &Path) -> bool {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
    let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    Script::parse(&name, &text).unwrap_or_else(|e| panic!("{e}")).stand().is_some()
}

/// The sandbox's single-game reference scenarios, in file-name order. None may start with a
/// `stand` line: a single game refuses it, and CI runs this folder's glob against one game.
pub fn scenario_files() -> Vec<PathBuf> {
    let files = files_in(&single_game_dir());
    let stands: Vec<&PathBuf> = files.iter().filter(|file| is_stand(file)).collect();
    assert!(
        stands.is_empty(),
        "{stands:?} start with a `stand` line; stand scenarios belong in {} (a single game cannot run them, and \
         `shards scenario crates/sandbox/scenarios/*.scenario` runs this folder against one)",
        stand_dir().display()
    );
    files
}

/// The reference scenarios that need a stand (a `stand` line), in file-name order.
pub fn stand_scenario_files() -> Vec<PathBuf> {
    let files = files_in(&stand_dir());
    let singles: Vec<&PathBuf> = files.iter().filter(|file| !is_stand(file)).collect();
    assert!(singles.is_empty(), "{singles:?} have no `stand` line; a file without one belongs in {}", single_game_dir().display());
    files
}

/// Runs a stand scenario file on a fresh stand of sandbox games.
pub fn run_stand_file(file: &Path) -> StandRun {
    run_stand_scenario(
        |app| {
            app.add_plugins(sandbox::SandboxPlugin);
        },
        file,
    )
}

/// What every member's journal says, host first: the same stand makes the same lines.
fn journals(stand: &Stand) -> Vec<Vec<String>> {
    (0..stand.len())
        .map(|member| {
            let journal = stand.world(member).resource::<shards::ai_core::AiJournal>();
            journal.after(0).0.map(|entry| entry.to_string()).collect()
        })
        .collect()
}

pub fn scenario(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios").join(name)
}

/// `snapshot.scenario` saves and loads `snapshots/boss_phase2.json`: tests that run it take
/// turns, or one would load the other's file.
pub static SNAPSHOT_FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Waits for [`SNAPSHOT_FILE`] (a test that failed holding it does not block the others).
pub fn snapshot_file_turn() -> std::sync::MutexGuard<'static, ()> {
    SNAPSHOT_FILE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn every_scenario_file_passes_on_a_fresh_game() {
    let _turn = snapshot_file_turn();
    let mut failed = Vec::new();
    for file in scenario_files() {
        let mut app = sandbox::test_app(42);
        let report = run_file(app.world_mut(), &file);
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} scenario(s) failed:\n\n{}", failed.len(), failed.join("\n\n"));
}

/// The scenarios also pass one after the other in the same game, the way
/// `shards scenario crates/sandbox/scenarios/*.scenario` runs them against one live game:
/// each puts the world back before it starts.
#[test]
fn the_scenarios_also_pass_in_one_game_after_another() {
    let _turn = snapshot_file_turn();
    let mut app = sandbox::test_app(42);
    let mut failed = Vec::new();
    for file in scenario_files() {
        let report = run_file(app.world_mut(), &file);
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} scenario(s) failed in a shared game:\n\n{}", failed.len(), failed.join("\n\n"));
}

/// And in the other order: what the region and snapshot scenarios leave behind (a region
/// in the world, a tick taken back) is no trouble for the arena's.
#[test]
fn the_scenarios_also_pass_one_after_another_backwards() {
    let _turn = snapshot_file_turn();
    let mut app = sandbox::test_app(42);
    let mut failed = Vec::new();
    for file in scenario_files().into_iter().rev() {
        let report = run_file(app.world_mut(), &file);
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} scenario(s) failed in a shared game, backwards:\n\n{}", failed.len(), failed.join("\n\n"));
}

/// The warden chooses its attacks with `SimRng`, which draws from the tick: a game that
/// played on for a while before the fight sees other attacks, in another order. The
/// combat prototype is won from any of them, the same way.
#[test]
fn the_combat_prototype_is_won_whatever_the_warden_chooses() {
    let mut failed = Vec::new();
    let mut openings = Vec::new();
    for before in [17, 131, 263, 409, 587, 761] {
        let mut app = sandbox::test_app(42);
        ok(&mut app, "step", json!({"ticks": before}));
        let report = run_file(app.world_mut(), scenario("combat_prototype.scenario"));
        if !report.passed {
            failed.push(format!("after {before} ticks:\n{report}"));
        }
        let attacks: Vec<String> = report
            .log
            .iter()
            .filter_map(|l| {
                l.detail.split_once("e6 winds up ").map(|(_, rest)| rest.split(' ').next().unwrap().to_owned())
            })
            .collect();
        openings.push(attacks);
    }
    assert!(failed.is_empty(), "{} fight(s) lost:\n\n{}", failed.len(), failed.join("\n\n"));
    let distinct: std::collections::HashSet<&Vec<String>> = openings.iter().collect();
    assert!(distinct.len() > 3, "the fights should differ: {openings:#?}");
}

/// The stand scenarios pass on a fresh stand: a host and its clients on the links the `stand`
/// line describes.
#[test]
fn every_stand_scenario_file_passes_on_a_fresh_stand() {
    let mut failed = Vec::new();
    for file in stand_scenario_files() {
        let run = run_stand_file(&file);
        if !run.report.passed {
            failed.push(run.report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} stand scenario(s) failed:\n\n{}", failed.len(), failed.join("\n\n"));
}

/// A stand is one deterministic game: the same scenario twice gives the same log and, on
/// every member, the same journal.
#[test]
fn a_stand_scenario_run_twice_gives_identical_journals() {
    for file in stand_scenario_files() {
        let (first, second) = (run_stand_file(&file), run_stand_file(&file));
        assert!(first.report.passed, "{}", first.report);
        assert_eq!(first.report.log, second.report.log, "{}", file.display());
        let (a, b) = (first.stand.expect("it ran on a stand"), second.stand.expect("it ran on a stand"));
        let (a, b) = (journals(&a), journals(&b));
        assert!(a.iter().all(|journal| journal.len() > 5), "the members journaled the run: {a:#?}");
        assert_eq!(a, b, "{}", file.display());
    }
}

/// A game on its own is not a stand: it refuses the file, changing nothing, and says which
/// stand to start.
#[test]
fn a_single_game_refuses_a_stand_scenario() {
    let mut app = sandbox::test_app(42);
    for file in stand_scenario_files() {
        let report = run_file(app.world_mut(), &file);
        let failure = report.failure.expect("a single game cannot run a stand scenario");
        assert_eq!((failure.kind, failure.line), (shards::ai_core::scenario::FailureKind::Driver, 0));
        assert!(
            failure.message.starts_with("this scenario needs a stand of host + 2 clients: run cargo sandbox --stand 2 --headless --paused --port 0 --port-file F"),
            "{}",
            failure.message
        );
    }
    assert_eq!(shards::ai_core::sim::tick(app.world()), 0, "nothing ran");
}

fn run_stand_text(text: &str) -> StandRun {
    shards::testing::run_stand_str(
        |app| {
            app.add_plugins(sandbox::SandboxPlugin);
        },
        "inline.scenario",
        text,
    )
}

/// `@c1 step --until …` watches c1's own view of the world, and the stand's links are the
/// header's (`net_status` of every member says so).
#[test]
fn a_step_condition_watches_the_member_it_names() {
    let run = run_stand_text(
        r#"
stand clients=1 latency=100
seed 42
@host act npc_set_state id=e6 state=calm
@host act teleport id=e15 x=2 z=12 y=0
step 60
@c1 act move_to target='{"x":2,"z":4}'
@c1 step 400 --until field:15:Transform.translation.z<8
@c1 expect stopped "Transform.translation.z"
@c1 expect field e15 Transform.translation.z < 8
@c1 expect field e15 Transform.translation.z > 3
@host act net_status
@host expect result contains "\"conditions\": \"latency=100,jitter=0,loss=0,dup=0\""
@c1 act net_status
@c1 expect result contains "\"role\": \"client\""
@c1 expect result contains "\"conditions\": \"latency=100,jitter=0,loss=0,dup=0\""
"#,
    );
    assert!(run.report.passed, "{}", run.report);
    // The step ended when c1 saw the body there, well before its 400 ticks.
    assert!(run.report.ticks < 300, "{}", run.report);
}

/// `session --player N` binds the agent to the body of player N, on the member it names.
#[test]
fn a_session_binds_an_agent_to_a_players_body() {
    let run = run_stand_text(
        r#"
stand clients=2
seed 42
step 30
@host session --agent coach --player 2
@host observe
@host expect result contains "you e16"
@host act inspect id=e5
@c2 session --agent me --player 2
@c2 observe
@c2 expect result contains "you e16"
@c1 expect shows e16 "Player"
@host session --agent coach --player 7
"#,
    );
    // The binding to a player nobody plays is the game's refusal, and the scenario's failure.
    assert!(!run.report.passed);
    let failure = run.report.failure.as_ref().unwrap();
    assert_eq!(failure.line, 13, "{}", run.report);
    assert_eq!(
        failure.message,
        "session coach on host: there is no player 7 with a body now; players: 0 (e5), 1 (e15), 2 (e16)"
    );
}

// --- regression scenarios -----------------------------------------------------------
//
// `scenarios/regress/*.scenario` are the sessions a fuzz run shrank (`shards_script::regress`).
// Each runs on a fresh game started with the seed its `seed` line names, forward only, never in
// the shared game of the tests above: it was written for a game that starts at tick 0. A file
// fails while its bug exists and is committed with the fix.

/// The folder of the regression scenarios (may hold none).
fn regress_dir() -> PathBuf {
    single_game_dir().join("regress")
}

/// The `*.scenario` files directly in `dir`, in file-name order; none is fine.
fn regress_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "scenario"))
        .collect();
    files.sort();
    files
}

/// Runs a regression scenario on a fresh game of the seed it names (42 without a `seed` line), in heavy
/// mode when its header asks for it.
fn run_regress_file(file: &Path) -> shards::script::Report {
    let text = std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
    let name = file.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let seed = Script::parse(&name, &text).ok().and_then(|script| script.seed()).unwrap_or(42);
    // A heavy stop needs heavy mode on to happen; the file's header says which.
    let mut app = match shards::script::regress::heavy_of(&text) {
        Some((every, span, offset)) => shards::testing::heavy_app(
            move |config| config.seed = seed,
            |app| {
                app.add_plugins(sandbox::SandboxPlugin);
            },
            shards::HeavyConfig { every, span, offset },
        ),
        None => sandbox::test_app(seed),
    };
    run_file(app.world_mut(), file)
}

#[test]
fn every_regression_scenario_passes_on_a_fresh_game() {
    let _turn = snapshot_file_turn();
    let mut failed = Vec::new();
    for file in regress_files(&regress_dir()) {
        let report = run_regress_file(&file);
        if !report.passed {
            failed.push(report.to_string());
        }
    }
    assert!(failed.is_empty(), "{} regression scenario(s) fail; fix the bug they name:\n\n{}", failed.len(), failed.join("\n\n"));
}

fn temp_regress_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sandbox_regress_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The runner: a file that passes, one whose planted rule halts the game inside a step (a step
/// cut short fails a scenario), and one whose expectation is wrong; the folder is read in name
/// order and a seed other than 42 starts a game of that seed.
#[test]
fn the_regression_runner_passes_a_good_file_and_fails_a_planted_one() {
    let dir = temp_regress_dir("runner");
    let good = dir.join("20261017-fine-seed42.scenario");
    std::fs::write(&good, "seed 42\nact? heal\nstep 10\nexpect field e5 Health.current > 0\n").unwrap();
    // Written the way the fuzzer writes one: the rule fires inside a step.
    let planted = dir.join("20261017-planted-seed9.scenario");
    std::fs::write(
        &planted,
        "seed 9\nact? assert_add name=planted when='{\"gone\":999}'\nact? heal\nstep 10\n",
    )
    .unwrap();
    let wrong = dir.join("20261017-wrong-seed42.scenario");
    std::fs::write(&wrong, "seed 42\nstep 5\nexpect result contains \"never printed\"\n").unwrap();
    let names: Vec<String> = regress_files(&dir).iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["20261017-fine-seed42.scenario", "20261017-planted-seed9.scenario", "20261017-wrong-seed42.scenario"]);

    let report = run_regress_file(&good);
    assert!(report.passed, "{report}");
    let report = run_regress_file(&planted);
    assert!(!report.passed, "a step cut short must fail the file:\n{report}");
    assert!(report.to_string().contains("planted"), "{report}");
    let report = run_regress_file(&wrong);
    assert!(!report.passed, "{report}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The folder in the repository is there (with a README saying what goes in it) and holds
/// only files the runner reads.
#[test]
fn the_regression_folder_exists_and_explains_itself() {
    let readme = std::fs::read_to_string(regress_dir().join("README.md")).expect("scenarios/regress/README.md");
    assert!(readme.contains("fuzz"), "{readme}");
}
