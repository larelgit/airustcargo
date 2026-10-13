//! Scenario files, run in-process.
//!
//! The same file `shards scenario FILE` runs against a live game runs here through
//! [`call_tool`], in `cargo test` and without a window. The format and the runner live
//! in [`shards_script`], so both drivers give a file the same meaning; this module
//! only moves calls into the world.
//!
//! ```ignore
//! let mut app = sandbox::test_app(42);
//! let report = ai_core::scenario::run_file(app.world_mut(), "scenarios/boss_fight.scenario");
//! assert!(report.passed, "{report}");
//! ```
//!
//! A scenario acts as the agent `scenario` (dev scope) and checks its expectations as
//! `scenario-check`, so an expectation never moves the journal cursor of the agent
//! under test.

use std::path::Path;

use bevy::prelude::*;
use serde_json::{json, Value};
use shards_script::{AgentScope, Driver, JournalLine, JournalRead, Outcome, Script, SessionConfig, Status};

pub use shards_script::{Failure, FailureKind, LogLine, Report};

use crate::id::{AiId, AiIndex};
use crate::journal::{self, AiJournal, JournalKind};
use crate::rng::SimSeed;
use crate::session::{AgentSessions, Scope};
use crate::sim;
use crate::tools::call_tool;

/// Runs the scenario in `path` against `world`. The report is named after the file.
pub fn run_file(world: &mut World, path: impl AsRef<Path>) -> Report {
    let path = path.as_ref();
    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    match std::fs::read_to_string(path) {
        Ok(text) => run_str(world, &name, &text),
        Err(e) => Report::unreadable(&name, format!("cannot read {}: {e}", path.display())),
    }
}

/// Runs scenario `text` against `world`; `name` stands for the file in errors and reports.
pub fn run_str(world: &mut World, name: &str, text: &str) -> Report {
    match Script::parse(name, text) {
        Ok(script) => shards_script::run(&script, &mut WorldDriver { world }),
        Err(errors) => Report::parse_failed(name, &errors),
    }
}

/// Drives the world the way BRP does: every call goes through [`call_tool`].
struct WorldDriver<'w> {
    world: &'w mut World,
}

impl Driver for WorldDriver<'_> {
    fn call(&mut self, agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
        let out = call_tool(self.world, agent, tool, args);
        Ok(Outcome { ok: out.ok, result: out.result, tick: Some(out.tick) })
    }

    /// What `ai/session` does, minus the transport.
    fn session(&mut self, agent: &str, config: &SessionConfig) -> Result<Outcome, String> {
        let subject = match &config.subject {
            Some(text) => {
                let Some(id) = AiId::parse(text) else {
                    return Ok(Outcome::err(format!("`{text}` is not an entity id; use e6 or 6")));
                };
                if let Err(message) = self.world.resource::<AiIndex>().resolve(id) {
                    return Ok(Outcome::err(message));
                }
                Some(id)
            }
            None => None,
        };
        let mut sessions = self.world.resource_mut::<AgentSessions>();
        let current = sessions.get_or_create(agent).clone();
        let subject = if config.clear_subject { None } else { subject.or(current.subject) };
        let scope = config.scope.map_or(current.scope, |s| match s {
            AgentScope::Player => Scope::Player,
            AgentScope::Dev => Scope::Dev,
        });
        let now = sessions.configure(agent, scope, subject).clone();
        Ok(Outcome::ok(json!({
            "agent": now.name,
            "scope": now.scope,
            "subject": now.subject.map(|i| i.to_string()),
            "seen_seq": now.seen_seq,
        })))
    }

    fn status(&mut self) -> Result<Status, String> {
        // Log lines of the last tick belong before the position a `mark` is taken at.
        journal::drain_logs(self.world);
        Ok(Status {
            tick: sim::tick(self.world),
            seed: self.world.get_resource::<SimSeed>().map(|s| s.0),
            journal_seq: self.world.resource::<AiJournal>().last_seq(),
        })
    }

    fn journal_since(&mut self, after: u64) -> Result<JournalRead, String> {
        journal::drain_logs(self.world);
        let (entries, missed) = self.world.resource::<AiJournal>().after(after);
        // The agents' own call echoes only repeat the scenario's commands.
        let lines = entries
            .filter(|e| e.kind != JournalKind::Agent)
            .map(|e| JournalLine { seq: e.seq, text: e.to_string() })
            .collect();
        Ok(JournalRead { lines, missed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::*;
    use crate::{AiJournal, JournalKind};

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Hp {
        current: f32,
    }

    /// Loses 1 hp a tick; journals when it gets to 5.
    fn decay(mut q: Query<&mut Hp>, mut journal: ResMut<AiJournal>, tick: Res<SimTick>) {
        for mut hp in &mut q {
            hp.current -= 1.0;
            if hp.current == 5.0 {
                journal.push(tick.0, JournalKind::Event, "e1 is low (5)");
            }
        }
    }

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: SimConfig { seed: 42, start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .register_type::<Hp>()
            .register_ai_component::<Hp>(|h| format!("hp {:.0}", h.current))
            .add_systems(FixedUpdate, decay.in_set(SimSet::Resolve));
        app.world_mut().spawn((AiKind::new("Thing"), AiPlayer, Hp { current: 10.0 }, Transform::default()));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    #[test]
    fn a_scenario_plays_in_process() {
        let mut app = game();
        let report = run_str(
            app.world_mut(),
            "decay.scenario",
            r#"
seed 42
observe
expect result contains "Thing"
expect field e1 Hp.current == 10
step 3
expect field e1 Hp.current == 7
step --until journal:low
expect stopped "journal: "
expect journal "e1 is low"
expect shows e1 "hp 5"
mark
expect !journal "low"
act? fly
expect result contains "unknown tool `fly`; available:"
step 2
invariant field e1 Hp.current > 0
step 1
"#,
        );
        assert!(report.passed, "{report}");
        assert_eq!((report.commands, report.ticks), (17, 8), "{report}");
        assert_eq!(app.world().resource::<SimTick>().0, 8);
    }

    #[test]
    fn a_broken_invariant_names_the_command_that_broke_it() {
        let mut app = game();
        let report = run_str(app.world_mut(), "decay.scenario", "invariant field e1 Hp.current > 5\nstep 3\nstep 10\nstep 1");
        let failure = report.failure.as_ref().expect("the invariant must fail");
        assert_eq!((failure.kind, failure.line, failure.command.as_str()), (FailureKind::Scenario, 3, "step 10"));
        assert_eq!(failure.message, "invariant `field e1 Hp.current > 5` (line 1) broke: e1 Hp.current is -3, which is not > 5");
        assert_eq!(report.commands, 3, "nothing runs after the failure");
        assert!(report.summary().starts_with("FAIL decay.scenario:3: invariant"), "{}", report.summary());
    }

    #[test]
    fn expectations_see_everything_while_the_scenario_plays_as_a_player() {
        let mut app = game();
        let report = run_str(
            app.world_mut(),
            "fair.scenario",
            "session --agent bot --scope player\nact? step ticks=5\nexpect result contains \"needs dev scope\"\nexpect field e1 Hp.current == 10\nsession --agent scenario\nstep 1\nexpect field e1 Hp.current == 9",
        );
        assert!(report.passed, "{report}");
        assert_eq!(report.log[0].detail, "acting as bot, player scope");
    }

    #[test]
    fn a_session_for_a_missing_entity_is_a_scenario_failure() {
        let mut app = game();
        let report = run_str(app.world_mut(), "s.scenario", "session --subject e99");
        let failure = report.failure.unwrap();
        assert_eq!((failure.kind, failure.line), (FailureKind::Scenario, 1));
        assert!(failure.message.starts_with("session scenario: e99 does not exist"), "{}", failure.message);
    }

    #[test]
    fn the_seed_line_is_checked_against_the_game() {
        let mut app = game();
        let report = run_str(app.world_mut(), "s.scenario", "seed 7");
        assert!(report.failure.unwrap().message.starts_with("this scenario is written for seed 7, but the game runs seed 42"));
    }

    #[test]
    fn parse_errors_and_missing_files_are_reports_not_panics() {
        let mut app = game();
        let report = run_str(app.world_mut(), "typo.scenario", "observe\nstpe 5\n");
        assert_eq!(report.failure.as_ref().map(|f| f.kind), Some(FailureKind::Parse));
        assert!(report.to_string().contains("typo.scenario:2: unknown command `stpe`; expected act, act?, step"), "{report}");
        assert_eq!(app.world().resource::<SimTick>().0, 0, "nothing ran");

        let report = run_file(app.world_mut(), "does/not/exist.scenario");
        assert_eq!(report.failure.as_ref().map(|f| f.kind), Some(FailureKind::Parse));
        assert!(report.summary().starts_with("FAIL exist.scenario: cannot read does/not/exist.scenario"), "{}", report.summary());
    }

    #[test]
    fn run_file_reads_a_file() {
        let dir = std::env::temp_dir().join(format!("ai_core_scenario_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.scenario");
        std::fs::write(&path, "step 2\nexpect field e1 Hp.current == 8\n").unwrap();
        let mut app = game();
        let report = run_file(app.world_mut(), &path);
        assert!(report.passed, "{report}");
        assert_eq!(report.name, "tiny.scenario");
        let _ = std::fs::remove_dir_all(dir);
    }
}
