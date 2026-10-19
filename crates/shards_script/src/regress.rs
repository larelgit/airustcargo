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

use crate::fuzz::Round;

/// The input calls of `rounds`.
pub fn input_count(rounds: &[Round]) -> usize {
    rounds.iter().map(|r| r.calls.len()).sum()
}

/// What the header says about the run.
#[derive(Clone, Debug, Default)]
pub struct Header<'a> {
    pub seed: u64,
    /// The stop's text (the halt reason), first line only.
    pub stop: &'a str,
    /// A short name of the stop for the first header line (`invariant action.resources_in_range`).
    pub stop_kind: &'a str,
    /// The rounds before shrinking.
    pub before: &'a [Round],
    /// Heavy mode the run had on (`every`, `span`, `offset`): a heavy stop does not happen without it,
    /// so the header says how to play the file (`# heavy: every=60 span=60 offset=3`, read by
    /// [`heavy_of`]).
    pub heavy: Option<(u64, u64, u64)>,
    /// A stop that is a refused call, not a halt (a chaos round trip): the last call of this tool is
    /// written `act`, not `act?`, so that its refusal fails the file.
    pub strict: Option<&'a str>,
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
    if let Some((every, span, offset)) = header.heavy {
        let _ = writeln!(out, "# heavy: every={every} span={span} offset={offset}");
    }
    let _ = writeln!(out, "seed {}", header.seed);
    // The one call that must work, when the stop is a refusal.
    let strict_at = header.strict.and_then(|wanted| {
        rounds.iter().enumerate().rev().find_map(|(r, round)| round.calls.iter().rposition(|(t, _)| t == wanted).map(|c| (r, c)))
    });
    for (r, round) in rounds.iter().enumerate() {
        for (c, (tool, args)) in round.calls.iter().enumerate() {
            let line = act_line(tool, args);
            let _ = writeln!(out, "{}", if strict_at == Some((r, c)) { line.replacen("act?", "act", 1) } else { line });
        }
        if round.ticks > 0 {
            let _ = writeln!(out, "step {}", round.ticks);
        }
    }
    out
}

/// The heavy mode a scenario file asks for in its header (`# heavy: every=60 span=60 offset=3`).
pub fn heavy_of(text: &str) -> Option<(u64, u64, u64)> {
    let line = text.lines().find_map(|l| l.strip_prefix("# heavy: "))?;
    let field = |key: &str| line.split_whitespace().find_map(|w| w.strip_prefix(key)?.strip_prefix('=')?.parse().ok());
    Some((field("every")?, field("span")?, field("offset")?))
}

/// `YYYYMMDD` of the day `unix_seconds` falls on (UTC).
pub fn date_of(unix_seconds: u64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = (unix_seconds / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}")
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
            ..Header::default()
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
            // A fuzz run's malformed call: arguments that are not an object (issue #72: its
            // scenario did not parse).
            round(&[("attack", json!("not an object"))], 3),
        ];
        let header = Header { seed: 42, stop: "panic", stop_kind: "panic", before: &rounds, ..Header::default() };
        let text = scenario_text(&header, &rounds);
        let script = Script::parse("r.scenario", &text).unwrap();
        assert_eq!(script.seed(), Some(42));
        // Each act line gives its arguments back through the reader the runner uses.
        for line in text.lines().filter(|l| l.starts_with("act? say")) {
            let words = split_words(line).unwrap().words;
            let args = crate::parse_args(&words[2..]).unwrap();
            assert_eq!(args, json!({"text": "it's \"odd\" \\ ok"}));
        }
        let line = text.lines().find(|l| l.starts_with("act? attack")).unwrap();
        assert_eq!(crate::parse_args(&split_words(line).unwrap().words[2..]).unwrap(), json!("not an object"), "{line}");
    }

    #[test]
    fn an_empty_round_writes_no_step_and_no_commands() {
        let rounds = vec![round(&[], 0)];
        let header = Header { seed: 1, stop: "x", stop_kind: "x", before: &rounds, ..Header::default() };
        assert_eq!(command_count(&scenario_text(&header, &rounds)), 1); // just `seed 1`
    }

    #[test]
    fn act_lines_leave_out_empty_arguments() {
        assert_eq!(act_line("jump", &json!({})), "act? jump");
        assert_eq!(act_line("jump", &Value::Null), "act? jump");
        assert_eq!(act_line("move", &json!({"dir": "N"})), "act? move '{\"dir\":\"N\"}'");
    }

    #[test]
    fn a_heavy_stop_says_how_to_play_it_and_a_refusal_stop_is_a_strict_act() {
        let rounds = vec![round(&[("snapshot_save", json!({"path": "s"})), ("snapshot_load", json!({"path": "s"}))], 3)];
        let header = Header { seed: 5, stop: "x", stop_kind: "x", before: &rounds, heavy: Some((60, 60, 5)), strict: Some("snapshot_load") };
        let text = scenario_text(&header, &rounds);
        assert!(text.contains("# heavy: every=60 span=60 offset=5
"), "{text}");
        assert!(text.contains("act? snapshot_save") && text.contains("
act snapshot_load"), "{text}");
        assert_eq!(heavy_of(&text), Some((60, 60, 5)));
        assert_eq!(heavy_of("seed 1
step 3
"), None);
        Script::parse("r.scenario", &text).unwrap();
    }

    #[test]
    fn dates_are_civil_days_in_utc() {
        assert_eq!(date_of(0), "19700101");
        assert_eq!(date_of(951_782_400), "20000229");
        assert_eq!(date_of(1_792_281_600), "20261018");
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
