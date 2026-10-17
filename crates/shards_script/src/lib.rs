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
//! session --agent bot --scope player      # who runs the following commands; also --subject ID, --clear-subject,
//!                                         # --player N (the body of player N, found again after a respawn)
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
//! A **stand scenario** drives a host and one or two clients (`cargo sandbox --stand 2`)
//! from one file. A header line names the stand, and any command may start with the member
//! it runs on:
//!
//! ```text
//! stand clients=2 latency=150 loss=0.02   # first (after `seed`, either order): links of 150 ms one way, 2% loss
//! seed 42
//! @c1 act move_to target=e12              # @host (the default), @c1, @c2
//! @c2 act lock_on
//! step 300                                # global: N stand ticks, whoever it is called on
//! @c2 step --until journal:swing          # @peer on a step: whose journal/state the condition watches
//! @c1 expect field e23 Transform.translation.x > 3   # expect/invariant run on the member they name
//! @c2 expect field e23 Transform.translation.x > 3   # the same body, seen by the other client
//! invariant !journal "desync"             # an invariant without a member runs on every member
//! @c1 session --player 1                  # the agent plays player 1's body
//! mark                                    # without a member: every member's journal counts from here
//! ```
//!
//! `expect result` and `expect stopped` look at the last command, or `step`, on the member
//! they name (`@c1 expect result …` is c1's answer, whatever ran on the others since); without a
//! member they look at the last one wherever it ran. `expect journal`, `field` and `shows`
//! look at the member they name (the host by default). The acting agent is per member:
//! `session --agent bot` names it on every member, `@c1 session --agent bot` on c1 alone. A
//! driver that cannot run the stand the header describes refuses the scenario and says how to
//! start one.
//!
//! [`Script::parse`] reads a file (errors name the line); [`run`] executes it against
//! a game through a [`Driver`] and returns a [`Report`]. The runner owns all the
//! rules, a driver only moves JSON, so a scenario means the same thing in
//! `cargo test` (`ai_core::scenario`, `shards::testing::run_stand_scenario`) and against a
//! running game (`shards scenario`).
//!
//! A **soak** ([`soak`]) is the other way to drive a game from here: no file, but seeded
//! agents that play every member of a stand for as long as they are told (`shards soak
//! --minutes 30`) and stop at the first desync. Its inputs, and those of `shards fuzz`, are
//! [`inputs`].

mod args;
mod condition;
pub mod fuzz;
pub mod inputs;
mod lex;
pub mod minimize;
mod run;
pub mod schema;
mod script;
pub mod soak;

pub use args::{loose_value, parse_args};
pub use lex::{split_words, Words};
pub use condition::parse_condition;
pub use run::{
    parse_journal_text, run, run_with, Driver, Failure, FailureKind, Halted, JournalLine, JournalRead, LogLine, Outcome, Report, Status,
    CHECK_AGENT, DEFAULT_AGENT,
};
pub use script::{
    id_value, AgentScope, Cmp, Command, Condition, Expect, ParseError, ParseErrors, Script, ScriptLine, SessionConfig, StandHeader,
    HOST, MAX_STAND_CLIENTS,
};
