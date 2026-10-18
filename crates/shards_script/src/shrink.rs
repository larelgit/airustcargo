//! Shrinking a fuzz session that stopped: [`shrink`] runs [`minimize`] over the rounds, then over
//! the single commands of what is left, and [`replays_stop`] is the question each attempt asks of
//! a game: does playing these rounds stop on the same thing?
//!
//! Both are free of any game. An attempt is a closure: in-process (`shards::testing`) it builds a
//! fresh app and a [`Driver`] for it; over BRP (`shards fuzz --minimize`) it loads the snapshot
//! taken before the first input, then plays through the live game's driver.

use serde_json::{json, Value};

use crate::fuzz::Round;
use crate::minimize::minimize;
use crate::run::Driver;
use crate::stop::Stop;

/// What [`shrink`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Shrunk {
    pub rounds: Vec<Round>,
    /// Attempts made, both passes together.
    pub attempts: u32,
    /// The budget ran out before the session was proven minimal.
    pub exhausted: bool,
}

/// One command of a session: a call, or a step.
#[derive(Clone, Debug, PartialEq)]
enum Command {
    Call(String, Value),
    Step(u64),
}

fn commands(rounds: &[Round]) -> Vec<Command> {
    let mut out = Vec::new();
    for round in rounds {
        out.extend(round.calls.iter().map(|(tool, args)| Command::Call(tool.clone(), args.clone())));
        if round.ticks > 0 {
            out.push(Command::Step(round.ticks));
        }
    }
    out
}

/// The rounds of a list of commands: the calls up to each step, and a last round without a step
/// for calls that no step follows.
fn rounds_of(commands: &[Command]) -> Vec<Round> {
    let mut rounds = Vec::new();
    let mut calls = Vec::new();
    for command in commands {
        match command {
            Command::Call(tool, args) => calls.push((tool.clone(), args.clone())),
            Command::Step(ticks) => rounds.push(Round { calls: std::mem::take(&mut calls), ticks: *ticks }),
        }
    }
    if !calls.is_empty() {
        rounds.push(Round { calls, ticks: 0 });
    }
    rounds
}

/// Shrinks `rounds`, which `attempt` is assumed to find failing: whole rounds first (each is an
/// input and a step), then the single calls and steps of the rounds that remain. At most `budget`
/// attempts in all. `attempt` says whether a candidate still stops on the same thing; its error
/// (the game cannot be reached) ends the shrinking.
pub fn shrink(rounds: &[Round], budget: u32, mut attempt: impl FnMut(&[Round]) -> Result<bool, String>) -> Result<Shrunk, String> {
    let error = std::cell::RefCell::new(None::<String>);
    let mut ask = |candidate: &[Round]| -> bool {
        if error.borrow().is_some() {
            return false;
        }
        match attempt(candidate) {
            Ok(fails) => fails,
            Err(e) => {
                *error.borrow_mut() = Some(e);
                false
            }
        }
    };
    let first = minimize(rounds, &mut ask, budget);
    let mut attempts = first.attempts;
    let mut best = first.items;
    let mut exhausted = first.exhausted;
    if !exhausted && error.borrow().is_none() {
        let flat = commands(&best);
        let second = minimize(&flat, |candidate| ask(&rounds_of(candidate)), budget - attempts);
        attempts += second.attempts;
        exhausted = second.exhausted;
        let mut merged = second.items;
        // Steps that follow each other with no call between are one step: fewer commands to read.
        let mut i = 0;
        while !exhausted && i + 1 < merged.len() {
            if let (Command::Step(a), Command::Step(b)) = (&merged[i], &merged[i + 1]) {
                if attempts >= budget {
                    exhausted = true;
                    break;
                }
                let mut candidate = merged.clone();
                candidate[i] = Command::Step(a + b);
                candidate.remove(i + 1);
                attempts += 1;
                if ask(&rounds_of(&candidate)) {
                    merged = candidate;
                    continue;
                }
            }
            i += 1;
        }
        best = rounds_of(&merged);
    }
    match error.into_inner() {
        Some(e) => Err(e),
        None => Ok(Shrunk { rounds: best, attempts, exhausted }),
    }
}

/// The tool a chaos round trip's first half used, for the refused second half `tool`.
fn partner(tool: &str) -> Option<&'static str> {
    match tool {
        "snapshot_load" => Some("snapshot_save"),
        "load_game" => Some("save_game"),
        _ => None,
    }
}

/// Plays `rounds` through `driver` as `agent` (each round's calls, then its step) and says
/// whether the game stops on `stop`: a halt that is the same stop, or, for a chaos stop, the
/// second half of the round trip refused after its first half worked. Another halt, or a refused
/// step, ends the attempt as not failing. `Err` only when the game cannot be reached.
pub fn replays_stop(driver: &mut dyn Driver, agent: &str, rounds: &[Round], stop: &Stop) -> Result<bool, String> {
    let mut saved = false;
    for round in rounds {
        for (tool, args) in &round.calls {
            let out = driver.call(agent, tool, args.clone())?;
            if let Some(halted) = &out.halted {
                return Ok(Stop::from_reason(&halted.reason).is_some_and(|got| stop.same_as(&got)));
            }
            if let Stop::Chaos { tool: wanted } = stop {
                if partner(tool).is_none() && matches!(tool.as_str(), "snapshot_save" | "save_game") {
                    saved = out.ok;
                }
                if tool == wanted && partner(tool).is_some() && saved && !out.ok {
                    return Ok(true);
                }
            }
        }
        if round.ticks > 0 {
            let out = driver.call(agent, "step", json!({"ticks": round.ticks}))?;
            if let Some(halted) = &out.halted {
                return Ok(Stop::from_reason(&halted.reason).is_some_and(|got| stop.same_as(&got)));
            }
            if !out.ok {
                return Ok(false);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Halted, JournalRead, Outcome, Status};
    use crate::script::SessionConfig;

    fn round(tools: &[&str], ticks: u64) -> Round {
        Round { calls: tools.iter().map(|t| ((*t).to_owned(), json!({}))).collect(), ticks }
    }

    #[test]
    fn rounds_survive_a_trip_through_commands() {
        let rounds = vec![round(&["a", "b"], 5), round(&[], 3), round(&["c"], 0)];
        assert_eq!(rounds_of(&commands(&rounds)), vec![round(&["a", "b"], 5), round(&[], 3), round(&["c"], 0)]);
    }

    #[test]
    fn it_cuts_whole_rounds_and_then_single_calls() {
        // Fails when `trigger` is called after `prime`: they sit in rounds with a lot of noise.
        let mut rounds: Vec<Round> = (0..12).map(|i| round(&[&format!("noise{i}")], 4)).collect();
        rounds[3] = round(&["noise3", "prime", "more"], 4);
        rounds[9] = round(&["x", "trigger", "y"], 4);
        let fails = |c: &[Round]| {
            let mut order: Vec<&str> = Vec::new();
            for r in c {
                order.extend(r.calls.iter().map(|(t, _)| t.as_str()));
                if r.ticks > 0 {
                    order.push("step");
                }
            }
            let prime = order.iter().position(|t| *t == "prime");
            let trigger = order.iter().position(|t| *t == "trigger");
            matches!((prime, trigger), (Some(a), Some(b)) if a < b && order[b..].contains(&"step"))
        };
        let shrunk = shrink(&rounds, 500, |c| Ok(fails(c))).unwrap();
        let calls: Vec<&str> = shrunk.rounds.iter().flat_map(|r| r.calls.iter().map(|(t, _)| t.as_str())).collect();
        assert_eq!(calls, ["prime", "trigger"], "{shrunk:?}");
        assert!(!shrunk.exhausted && shrunk.attempts > 0);
        assert!(fails(&shrunk.rounds));
    }

    #[test]
    fn steps_with_nothing_between_them_are_merged_when_the_stop_survives_it() {
        let rounds = vec![round(&["a"], 5), round(&[], 7), round(&[], 9), round(&["b"], 2)];
        // Fails when `a` is called and at least 20 ticks follow.
        let fails = |c: &[Round]| c.first().is_some_and(|r| r.calls.iter().any(|(t, _)| t == "a")) && c.iter().map(|r| r.ticks).sum::<u64>() >= 20;
        let shrunk = shrink(&rounds, 200, |c| Ok(fails(c))).unwrap();
        assert_eq!(shrunk.rounds, vec![round(&["a"], 21)], "{shrunk:?}");
    }

    #[test]
    fn a_driver_error_ends_the_shrinking() {
        let rounds: Vec<Round> = (0..6).map(|_| round(&["a"], 1)).collect();
        let error = shrink(&rounds, 100, |_| Err("the game went away".to_owned())).unwrap_err();
        assert_eq!(error, "the game went away");
    }

    #[test]
    fn the_budget_is_shared_by_both_passes() {
        let rounds: Vec<Round> = (0..30).map(|i| round(&["a", "b"], i + 1)).collect();
        let shrunk = shrink(&rounds, 7, |c| Ok(c.iter().any(|r| r.ticks == 20))).unwrap();
        assert_eq!(shrunk.attempts, 7);
        assert!(shrunk.exhausted);
    }

    /// A game that halts when a call named `boom` has been made and a step follows.
    #[derive(Default)]
    struct Game {
        armed: bool,
        saved: bool,
        reason: &'static str,
    }

    impl Driver for Game {
        fn call(&mut self, _agent: &str, tool: &str, _args: Value) -> Result<Outcome, String> {
            let mut out = Outcome::ok("ok");
            match tool {
                "boom" => self.armed = true,
                "snapshot_save" => self.saved = true,
                "snapshot_load" => out = Outcome::err("differs"),
                "step" if self.armed => out.halted = Some(Halted { reason: self.reason.to_owned(), replay: None }),
                _ => {}
            }
            Ok(out)
        }

        fn session(&mut self, _: &str, _: &SessionConfig) -> Result<Outcome, String> {
            unreachable!()
        }

        fn status(&mut self) -> Result<Status, String> {
            unreachable!()
        }

        fn journal_since(&mut self, _: u64) -> Result<JournalRead, String> {
            unreachable!()
        }
    }

    #[test]
    fn an_attempt_fails_only_on_the_same_stop() {
        let inv = |name: &str| Stop::Invariant { name: name.into(), tick: 1 };
        let rounds = vec![round(&["boom"], 3)];
        let mut game = Game { reason: "invariant hp violated at tick 3", ..Game::default() };
        assert!(replays_stop(&mut game, "fuzz", &rounds, &inv("hp")).unwrap());
        let mut game = Game { reason: "invariant hp violated at tick 3", ..Game::default() };
        assert!(!replays_stop(&mut game, "fuzz", &rounds, &inv("other")).unwrap(), "another rule is another stop");
        let mut game = Game { reason: "invariant hp violated at tick 3", ..Game::default() };
        assert!(!replays_stop(&mut game, "fuzz", &[round(&["boom"], 0)], &inv("hp")).unwrap(), "no step, no halt");
        let mut game = Game { reason: "heavy check failed at tick 300", ..Game::default() };
        assert!(replays_stop(&mut game, "fuzz", &rounds, &Stop::Heavy { tick: 330 }).unwrap());
        let mut game = Game { reason: "heavy check failed at tick 300", ..Game::default() };
        assert!(!replays_stop(&mut game, "fuzz", &rounds, &Stop::Heavy { tick: 900 }).unwrap());
    }

    #[test]
    fn a_chaos_stop_needs_the_first_half_of_the_round_trip_to_have_worked() {
        let stop = Stop::Chaos { tool: "snapshot_load".into() };
        let mut game = Game::default();
        assert!(replays_stop(&mut game, "fuzz", &[round(&["snapshot_save", "snapshot_load"], 1)], &stop).unwrap());
        // A load alone is refused for want of a file, which is no finding.
        let mut game = Game::default();
        assert!(!replays_stop(&mut game, "fuzz", &[round(&["snapshot_load"], 1)], &stop).unwrap());
    }
}
