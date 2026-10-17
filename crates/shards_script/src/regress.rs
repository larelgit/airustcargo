//! Regression scenarios: a fuzz session that broke a rule, written as a scenario file.
//!
//! [`scenario_text`] is a pure function from what was played (the seed, the rounds, the stop) to
//! the text of a `*.scenario` file:
//!
//! ```text
//! # regress: invariant action.resources_in_range at tick 212, seed 7
//! # stop: invariant action.resources_in_range violated at tick 212: e5 Stamina.current = -3 …
//! # inputs: 41 before shrinking, 3 after (rounds: 18 before, 2 after)
//! seed 7
//! act? dodge dir=W
//! step 12
//! ```
//!
//! Only `seed`, `act?` and `step` lines follow the header, as played. Calls are `act?` because the
//! fuzzer's calls are often refused (no stamina, nothing to hit) and a refusal is not what fails;
//! the stop is: the broken rule halts the game inside a `step`, and a step cut short fails a
//! scenario. The file fails while the bug exists and is committed with its fix as a regression
//! test (`crates/sandbox/scenarios/regress/`, run by `scenario_files.rs`, each on a fresh game).

use std::fmt::Write as _;

use serde_json::Value;

/// One round as played: the calls made, then a step of `ticks`.
#[derive(Clone, Debug, PartialEq)]
pub struct Round {
    /// `(tool, arguments)` in the order they were called.
    pub calls: Vec<(String, Value)>,
    /// The length of the `step` that followed the calls.
    pub ticks: u64,
}

/// The input calls of `rounds`.
pub fn input_count(rounds: &[Round]) -> usize {
    rounds.iter().map(|r| r.calls.len()).sum()
}

/// What the header says about the run.
#[derive(Clone, Debug)]
pub struct Header<'a> {
    pub seed: u64,
    /// The stop's text (the halt reason), first line only.
    pub stop: &'a str,
    /// A short name of the stop for the first header line (`invariant action.resources_in_range`).
    pub stop_kind: &'a str,
    /// The rounds before shrinking.
    pub before: &'a [Round],
}

/// The scenario file for `rounds` (already shrunk), with the header describing the run.
pub fn scenario_text(header: &Header, rounds: &[Round]) -> String {
    let stop = header.stop.lines().next().unwrap_or_default().trim();
    let mut out = String::new();
    let _ = writeln!(out, "# regress: {}, seed {}", header.stop_kind, header.seed);
    let _ = writeln!(out, "# stop: {stop}");
    let _ = writeln!(
        out,
        "# inputs: {} before shrinking, {} after (rounds: {} before, {} after)",
        input_count(header.before),
        input_count(rounds),
        header.before.len(),
        rounds.len()
    );
    let _ = writeln!(out, "# fails while the bug exists: the stop halts the game inside a step, and a step cut short fails a scenario");
    let _ = writeln!(out, "seed {}", header.seed);
    for round in rounds {
        for (tool, args) in &round.calls {
            let _ = writeln!(out, "{}", act_line(tool, args));
        }
        if round.ticks > 0 {
            let _ = writeln!(out, "step {}", round.ticks);
        }
    }
    out
}

/// How many commands the scenario holds (`seed`, `act?` and `step` lines).
pub fn command_count(text: &str) -> usize {
    text.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).count()
}

/// `act? tool {"a":1}`; arguments that are empty are left out.
pub fn act_line(tool: &str, args: &Value) -> String {
    match args {
        Value::Null => format!("act? {tool}"),
        Value::Object(map) if map.is_empty() => format!("act? {tool}"),
        other => format!("act? {tool} {}", quote_json(&other.to_string())),
    }
}

/// A word the scenario reader (`split_words`) gives back as `json`: single quotes keep
/// everything literal; JSON that holds a `'` takes double quotes with `\"` and `\\` escaped.
fn quote_json(json: &str) -> String {
    if !json.contains('\'') {
        return format!("'{json}'");
    }
    format!("\"{}\"", json.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The file name of a regression scenario: `<YYYYMMDD>-<stop>-seed<S>.scenario`. `stop` is made
/// into a file-name word (letters, digits, `-`, `_` and `.` stay).
pub fn file_name(date: &str, stop: &str, seed: u64) -> String {
    let word: String =
        stop.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    format!("{date}-{}-seed{seed}.scenario", word.trim_matches('-'))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{split_words, Script};

    fn round(calls: &[(&str, Value)], ticks: u64) -> Round {
        Round { calls: calls.iter().map(|(t, a)| ((*t).to_owned(), a.clone())).collect(), ticks }
    }

    fn sample() -> (Vec<Round>, Vec<Round>) {
        let before = vec![
            round(&[("move", json!({"dir": "N"})), ("jump", json!({}))], 5),
            round(&[("dodge", json!({"dir": "W"}))], 12),
            round(&[("light_attack", Value::Null)], 3),
        ];
        let after = vec![before[1].clone()];
        (before, after)
    }

    #[test]
    fn the_text_has_a_header_the_seed_and_the_calls_and_steps_as_played() {
        let (before, after) = sample();
        let header = Header {
            seed: 7,
            stop: "invariant action.resources_in_range violated at tick 212: e5 Stamina.current = -3\nmore",
            stop_kind: "invariant action.resources_in_range",
            before: &before,
        };
        let text = scenario_text(&header, &after);
        let expected = "\
# regress: invariant action.resources_in_range, seed 7
# stop: invariant action.resources_in_range violated at tick 212: e5 Stamina.current = -3
# inputs: 4 before shrinking, 1 after (rounds: 3 before, 1 after)
# fails while the bug exists: the stop halts the game inside a step, and a step cut short fails a scenario
seed 7
act? dodge '{\"dir\":\"W\"}'
step 12
";
        assert_eq!(text, expected);
        assert_eq!(command_count(&text), 3);
    }

    #[test]
    fn the_text_is_a_valid_scenario_that_parses_back_to_the_same_calls() {
        let rounds = vec![
            round(&[("say", json!({"text": "it's \"odd\" \\ ok"})), ("stop", json!({})), ("heal", Value::Null)], 4),
            round(&[], 0),
            round(&[("move_to", json!({"target": "e6", "path": true}))], 30),
        ];
        let header = Header { seed: 42, stop: "panic", stop_kind: "panic", before: &rounds };
        let text = scenario_text(&header, &rounds);
        let script = Script::parse("r.scenario", &text).unwrap();
        assert_eq!(script.seed(), Some(42));
        // Each act line gives its arguments back through the reader the runner uses.
        for line in text.lines().filter(|l| l.starts_with("act? say")) {
            let words = split_words(line).unwrap().words;
            let args = crate::parse_args(&words[2..]).unwrap();
            assert_eq!(args, json!({"text": "it's \"odd\" \\ ok"}));
        }
    }

    #[test]
    fn an_empty_round_writes_no_step_and_no_commands() {
        let rounds = vec![round(&[], 0)];
        let header = Header { seed: 1, stop: "x", stop_kind: "x", before: &rounds };
        assert_eq!(command_count(&scenario_text(&header, &rounds)), 1); // just `seed 1`
    }

    #[test]
    fn act_lines_leave_out_empty_arguments() {
        assert_eq!(act_line("jump", &json!({})), "act? jump");
        assert_eq!(act_line("jump", &Value::Null), "act? jump");
        assert_eq!(act_line("move", &json!({"dir": "N"})), "act? move '{\"dir\":\"N\"}'");
    }

    #[test]
    fn file_names_carry_the_date_the_stop_and_the_seed() {
        assert_eq!(
            file_name("20261017", "invariant action.resources_in_range", 7),
            "20261017-invariant-action.resources_in_range-seed7.scenario"
        );
        assert_eq!(file_name("20261017", "panic: boom!", 3), "20261017-panic--boom-seed3.scenario");
    }
}
