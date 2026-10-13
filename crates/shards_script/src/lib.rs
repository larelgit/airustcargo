//! # shards_script
//!
//! What the `shards` CLI and in-process tests share, without any game engine: the
//! `--until` condition syntax of `step`, `key=value` tool arguments, and **scenario
//! files**.
//!
//! A scenario is a UTF-8 text file with one command per line. `#` starts a comment,
//! blank lines are ignored, words are split like a shell would (`"…"`, `'…'`; use
//! single quotes for JSON). The commands mirror the CLI:
//!
//! ```text
//! seed 42                                 # optional, first: the game must run this seed
//! act attack target=e6 repeat=true        # call a tool; a failing call fails the scenario
//! act? teleport id=e5                     # call a tool; failure allowed (see `expect result`)
//! step 120                                # N ticks (default 1, or 600 with --until)
//! step --until named:boss_phase_2 -u journal:died
//! observe                                 # its text becomes the last result
//! mark                                    # journal expectations count from here
//! session --agent bot --scope player      # who runs the following commands; also --subject ID, --clear-subject
//!
//! expect result contains "ok: attacking"  # last result of act/act?/step/observe (exact case)
//! expect result !contains "error"
//! expect stopped "boss_phase_2"           # the last step stopped on a matching condition
//! expect journal "phase 1->2"             # since the start or the last `mark` (any case)
//! expect !journal "died"
//! expect field e5 Health.current > 0      # numeric field via `get`; < <= > >= == !=
//! expect shows e6 "phase 2"               # the entity's observation text (any case)
//!
//! invariant field e5 Health.current > 0   # any expectation, checked after every later act/step
//! ```
//!
//! [`Script::parse`] reads a file (errors name the line); [`run`] executes it against
//! a game through a [`Driver`] and returns a [`Report`]. The runner owns all the
//! rules, a driver only moves JSON, so a scenario means the same thing in
//! `cargo test` (`ai_core::scenario`) and against a running game (`shards scenario`).

mod args;
mod condition;
mod lex;
mod run;
mod script;

pub use args::{loose_value, parse_args};
pub use condition::parse_condition;
pub use run::{
    parse_journal_text, run, run_with, Driver, Failure, FailureKind, JournalLine, JournalRead, LogLine, Outcome, Report, Status,
    CHECK_AGENT, DEFAULT_AGENT,
};
pub use script::{
    id_value, AgentScope, Cmp, Command, Condition, Expect, ParseError, ParseErrors, Script, ScriptLine, SessionConfig,
};
