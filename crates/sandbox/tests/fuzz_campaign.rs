//! The nightly fuzz campaign: many seeded fuzz runs of the sandbox, in process, with chaos, coverage
//! steering and heavy mode, each run that stops shrunk on fresh games to a regression scenario.
//!
//! Ignored: it takes a long time. `nightly.yml` runs it (job `fuzz`); by hand:
//!
//! ```text
//! SHARDS_FUZZ_SEED=7 SHARDS_FUZZ_RUNS=3 SHARDS_FUZZ_INPUTS=300 \
//!   cargo test -p sandbox --release --test fuzz_campaign -- --ignored --nocapture
//! ```
//!
//! | variable | default | meaning |
//! |---|---|---|
//! | `SHARDS_FUZZ_SEED` | 1 | run `i` plays game seed and fuzz seed `SEED + i` |
//! | `SHARDS_FUZZ_RUNS` | 50 | number of runs |
//! | `SHARDS_FUZZ_INPUTS` | 2000 | rounds per run (an input or a chaos call, then a step) |
//! | `SHARDS_FUZZ_HEAVY` | 300 | heavy mode's period in ticks, 0 = off (60 costs about 4 minutes a run: see "Fuzzing" in docs/manual.md) |
//! | `SHARDS_FUZZ_MINUTES` | 0 | no run starts after this many minutes (0 = no limit); the nightly passes 85 |
//! | `SHARDS_FUZZ_STEER` | 1 | 0 plays the plain weights |
//! | `SHARDS_FUZZ_BUDGET` | 600 | attempts a shrink may make |
//! | `SHARDS_FUZZ_CHAOS` | 0.02 | share of rounds that make a developer call |
//! | `SHARDS_FUZZ_SHRINK_SECS` | 900 | wall-clock limit of one shrink |
//! | `SHARDS_FUZZ_OUT` | `fuzz-campaign` | the coverage report and the unreproduced sessions |
//! | `SHARDS_FUZZ_REGRESS` | `scenarios/regress` | where the shrunk scenarios go |
//!
//! Every run uses `--chaos 0.02 --steer`, coverage on. The test fails if any run stopped; the
//! scenarios, the sessions' replays (`replays/`) and the coverage report are written first.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use sandbox::SandboxPlugin;
use shards::script::fuzz::FuzzPlan;
use shards::script::regress;
use shards::serde_json::{json, Value};
use shards::testing::{fuzz_and_shrink, Fuzzed};
use shards::HeavyConfig;

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(text) if !text.trim().is_empty() => text.trim().parse().unwrap_or_else(|_| panic!("{name}={text} is not a whole number")),
        _ => default,
    }
}

fn env_f64(name: &str, default: f64) -> f64 {
    match std::env::var(name) {
        Ok(text) if !text.trim().is_empty() => text.trim().parse().unwrap_or_else(|_| panic!("{name}={text} is not a number")),
        _ => default,
    }
}

fn env_path(name: &str, default: &str) -> PathBuf {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty()).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

/// The game of run `seed`: coverage on, heavy mode when `heavy` is above 0.
fn build(seed: u64, heavy: u64) -> shards::bevy::app::App {
    if heavy == 0 {
        return sandbox::test_app_with(|config| {
            config.seed = seed;
            config.coverage = true;
        });
    }
    shards::testing::heavy_app(
        move |config| {
            config.seed = seed;
            config.coverage = true;
        },
        |app| {
            app.add_plugins(SandboxPlugin);
        },
        heavy_config(seed, heavy),
    )
}

/// Points every `heavy` ticks, each checked `heavy` ticks on (at most the longest span), from a tick
/// that depends on the seed so that over many nights every phase of the tick is checked.
fn heavy_config(seed: u64, heavy: u64) -> HeavyConfig {
    HeavyConfig { every: heavy, span: heavy.min(shards::ai_core::heavy::MAX_SPAN), offset: seed % heavy }
}

/// What all the runs reached together.
#[derive(Default)]
struct Reached {
    /// Signature -> (entity-ticks, runs it appeared in).
    signatures: BTreeMap<String, (u64, u64)>,
    /// Label -> (declared, ids not reached by any run so far).
    never: BTreeMap<String, (u64, Option<Vec<String>>)>,
    runs: u64,
}

impl Reached {
    fn add(&mut self, coverage: &Value) {
        self.runs += 1;
        for row in coverage["rarest"].as_array().into_iter().flatten() {
            let entry = self.signatures.entry(row["signature"].as_str().unwrap_or_default().to_owned()).or_default();
            entry.0 += row["count"].as_u64().unwrap_or(0);
            entry.1 += 1;
        }
        for item in coverage["never_reached"].as_array().into_iter().flatten() {
            let ids: Vec<String> = item["ids"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect();
            let entry = self.never.entry(item["what"].as_str().unwrap_or_default().to_owned()).or_default();
            entry.0 = item["declared"].as_u64().unwrap_or(0);
            // Never reached by any run: the ids every run missed.
            entry.1 = Some(match entry.1.take() {
                None => ids,
                Some(so_far) => so_far.into_iter().filter(|id| ids.contains(id)).collect(),
            });
        }
    }

    fn json(&self) -> Value {
        let mut rows: Vec<(&String, &(u64, u64))> = self.signatures.iter().collect();
        rows.sort_by(|a, b| (a.1 .1, a.1 .0, a.0).cmp(&(b.1 .1, b.1 .0, b.0)));
        json!({
            "runs": self.runs,
            "distinct_signatures": self.signatures.len(),
            "rarest": rows.iter().take(25).map(|(s, (count, runs))| json!({"signature": s, "entity_ticks": count, "runs": runs})).collect::<Vec<_>>(),
            "never_reached": self.never.iter().map(|(what, (declared, ids))| json!({"what": what, "declared": declared, "ids": ids})).collect::<Vec<_>>(),
        })
    }

    fn text(&self) -> String {
        let mut out = format!("coverage of {} runs: {} distinct signatures\nrarest (fewest runs first):\n", self.runs, self.signatures.len());
        let mut rows: Vec<(&String, &(u64, u64))> = self.signatures.iter().collect();
        rows.sort_by(|a, b| (a.1 .1, a.1 .0, a.0).cmp(&(b.1 .1, b.1 .0, b.0)));
        for (signature, (count, runs)) in rows.iter().take(15) {
            out.push_str(&format!("  {runs} runs, {count} entity-ticks  {signature}\n"));
        }
        for (what, (declared, ids)) in &self.never {
            let ids = ids.as_deref().unwrap_or_default();
            match ids.is_empty() {
                true => out.push_str(&format!("never reached by any run, {what}: none of {declared}\n")),
                false => out.push_str(&format!("never reached by any run, {what} ({} of {declared}): {}\n", ids.len(), ids.join(", "))),
            }
        }
        out
    }
}

#[test]
#[ignore = "the nightly campaign: minutes to hours (see the module docs)"]
fn the_nightly_fuzz_campaign() {
    let seed = env_u64("SHARDS_FUZZ_SEED", 1);
    let runs = env_u64("SHARDS_FUZZ_RUNS", 50);
    let inputs = env_u64("SHARDS_FUZZ_INPUTS", 2000);
    let heavy = env_u64("SHARDS_FUZZ_HEAVY", 300);
    let budget = env_u64("SHARDS_FUZZ_BUDGET", 600) as u32;
    let shrink_time = std::time::Duration::from_secs(env_u64("SHARDS_FUZZ_SHRINK_SECS", 900));
    let out = env_path("SHARDS_FUZZ_OUT", "fuzz-campaign");
    let regress_dir = env_path("SHARDS_FUZZ_REGRESS", "scenarios/regress");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(&regress_dir).unwrap();
    let date = regress::date_of(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));
    println!("fuzz campaign: seeds {seed}..{} ({runs} runs), {inputs} rounds each, heavy {heavy}, shrink budget {budget}", seed + runs);

    let mut reached = Reached::default();
    let mut stopped: Vec<String> = Vec::new();
    let started = Instant::now();
    let minutes = env_u64("SHARDS_FUZZ_MINUTES", 0);
    let mut played = 0;
    for i in 0..runs {
        if minutes > 0 && started.elapsed().as_secs() > minutes * 60 {
            println!("fuzz campaign: {minutes} minutes are up after {i} of {runs} runs; the rest is left for another night");
            break;
        }
        played += 1;
        let run_seed = seed + i;
        let plan = FuzzPlan {
            rounds: inputs as u32,
            chaos: env_f64("SHARDS_FUZZ_CHAOS", 0.02),
            steer: env_u64("SHARDS_FUZZ_STEER", 1) != 0,
            save_replay: true,
            stem: format!("campaign-seed{run_seed}"),
            ..FuzzPlan::new(run_seed)
        };
        let run_started = Instant::now();
        let Fuzzed { report, finding, coverage } = fuzz_and_shrink(|with_heavy| build(run_seed, if with_heavy { heavy } else { 0 }), &plan, budget, shrink_time)
            .unwrap_or_else(|e| panic!("seed {run_seed}: the run could not be played: {e}"));
        println!("[{}/{runs}] seed {run_seed}: {report} ({:.1}s)", i + 1, run_started.elapsed().as_secs_f64());
        if let Some(coverage) = &coverage {
            reached.add(coverage);
        }
        let Some(finding) = finding else {
            // A refused step ends a run too, and nothing was learned from it: that is a failure.
            if !matches!(report.ended, shards::script::fuzz::Ended::Done) {
                stopped.push(format!("seed {run_seed}: {report}"));
            }
            continue;
        };
        let heavy_header = (heavy > 0).then(|| {
            let config = heavy_config(run_seed, heavy);
            (config.every, config.span, config.offset)
        });
        let scenario = finding.scenario(run_seed, heavy_header);
        let name = finding.file_name(&date, run_seed);
        let path = match finding.reproduced {
            // A fresh game stops on it again: a regression test to commit with the fix.
            true => regress_dir.join(&name),
            // It did not: the long session is kept for a person to look at, not as a test.
            false => out.join(format!("unreproduced-{name}.txt")),
        };
        std::fs::write(&path, &scenario).unwrap();
        println!(
            "    stopped on {}: {} rounds -> {} ({} attempts{}), {} commands, written to {}",
            finding.stop.title(),
            finding.before.len(),
            finding.shrunk.rounds.len(),
            finding.shrunk.attempts,
            if finding.shrunk.exhausted { ", budget spent" } else { "" },
            regress::command_count(&scenario),
            path.display()
        );
        stopped.push(format!(
            "seed {run_seed}: {} ({} -> {} rounds, {}; replay {})",
            finding.text.lines().next().unwrap_or_default(),
            finding.before.len(),
            finding.shrunk.rounds.len(),
            path.display(),
            report.replay.as_deref().unwrap_or("none")
        ));
    }

    std::fs::write(out.join("coverage.json"), shards::serde_json::to_string_pretty(&reached.json()).unwrap() + "\n").unwrap();
    std::fs::write(out.join("coverage.txt"), reached.text()).unwrap();
    println!("\n{}", reached.text());
    println!("fuzz campaign: {played} runs in {:.1} minutes, {} stopped", started.elapsed().as_secs_f64() / 60.0, stopped.len());
    assert!(stopped.is_empty(), "{} of {played} runs stopped:\n{}", stopped.len(), stopped.join("\n"));
}

/// The report adds up what runs reached: signatures sum, and a never-reached id must be missed by all.
#[test]
fn the_coverage_report_merges_runs() {
    let run = |sigs: &[(&str, u64)], never: &[&str]| {
        json!({
            "rarest": sigs.iter().map(|(s, c)| json!({"signature": s, "count": c, "first_tick": 1})).collect::<Vec<_>>(),
            "never_reached": [{"what": "actions", "declared": 4, "ids": never}],
        })
    };
    let mut reached = Reached::default();
    reached.add(&run(&[("kind=A", 3), ("kind=B", 1)], &["slam", "claw", "bite"]));
    reached.add(&run(&[("kind=A", 4)], &["claw", "bite", "kick"]));
    let report = reached.json();
    assert_eq!(report["runs"], 2);
    assert_eq!(report["distinct_signatures"], 2);
    assert_eq!(report["rarest"][0], json!({"signature": "kind=B", "entity_ticks": 1, "runs": 1}));
    assert_eq!(report["never_reached"][0]["ids"], json!(["claw", "bite"]));
    assert!(reached.text().contains("never reached by any run, actions (2 of 4): claw, bite"), "{}", reached.text());
}

/// Run `i` of a campaign is the game and the fuzzer of seed `SEED + i`, and heavy mode's first
/// point depends on it.
#[test]
fn heavy_points_start_at_a_tick_that_depends_on_the_seed() {
    assert_eq!(heavy_config(123, 60), HeavyConfig { every: 60, span: 60, offset: 3 });
    assert_eq!(heavy_config(7, 1000).span, shards::ai_core::heavy::MAX_SPAN);
}

