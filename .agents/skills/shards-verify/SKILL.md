---
name: shards-verify
description: Verify a change to the Shards engine, the sandbox or a game the way CI will - plan the checks the diff needs, run them from the fastest to the slowest, pick the right kind of test for new behaviour (unit, crate integration, scenario, replay, snapshot, heavy mode, invariant control test), and re-record reference replays only on purpose. Use before every commit or push, when deciding how to test something, when asked to run the tests, or when a check fails after a change.
---

# Verifying a change

CI (`.github/workflows/ci.yml`) is the bar: a change is ready when what CI would run on it passes here, and
the report you give says exactly what ran. Paths below are from the repository root. The rules behind the
checks are in `docs/agents/checking-changes.md`; this is the order to apply them in.

## 1. Plan the checks from the diff

```bash
python3 .agents/skills/shards-verify/scripts/plan.py              # what changed against origin/main, committed or not
python3 .agents/skills/shards-verify/scripts/plan.py --base HEAD  # only what is not committed yet
```

(`py -3` instead of `python3` on Windows.) It maps the changed files to the commands to run, the files to
bake or regenerate, the docs to update and the reference replays that may move. It knows paths, not
intent: add what it cannot see (a behaviour change in `shards_core` reaches every module above it).

## 2. Fast loop, while you work

- One crate: `cargo test -p shards_npc`; one module of its test binary: `cargo test -p shards_npc --test npc live::`.
  Each crate has one integration binary, `crates/<crate>/tests/<short>/main.rs`; `ls crates/<crate>/tests/` names it.
- The sandbox end to end (scenarios in-process, reference replays, state audit, engine rules):
  `cargo test -p sandbox --test sandbox`; a part of it: `cargo test -p sandbox --test sandbox scenario_files::`.
- Clippy on what you touched: `cargo clippy -p <crate> --all-targets -- -D warnings`. It fails on `x.sin()` and
  the other `f32` maths methods by design: use `ops::sin(x)` (`docs/agents/determinism.md`).
- Content only: nothing to rebuild. `cargo sandbox --check-content` (template: `(cd template && cargo run -- --check-content)`),
  and in a live game the edit applies on save (`.agents/skills/shards-playtest/SKILL.md`).
- One scenario: against a live game started with `live.py start` (`.agents/skills/shards-playtest/SKILL.md`),
  `cargo shards --port-file target/live/sandbox.port scenario crates/sandbox/scenarios/<topic>.scenario`; the
  in-process run comes with the sandbox tests.

Builds are the slow part. `.cargo/config.toml` runs 4 jobs; with RAM to spare (2-4 GB a job, nothing else
building) `CARGO_BUILD_JOBS=8` is about a third faster. A cold build is 8-16 minutes, so start it in the
background early. The whole workspace's test build needs about 22 GB in `target/`: check `df -h .` first, and on
a small machine run the per-crate and sandbox tests and say that the full run is left to CI.

## 3. The gate, before you push

Run what the plan lists; for a change to engine code that is all of this:

```bash
cargo test --workspace                                    # CI: cargo nextest run --workspace --profile ci -E 'not binary_id(sandbox::perf)'
cargo clippy --workspace --all-targets -- -D warnings
cargo sandbox --check-content
cargo sandbox --worldgen-report wilds --seeds 1..1000     # and voxel_arena; both must exit 0
cargo test --manifest-path template/Cargo.toml            # the template uses every engine crate
cargo clippy --manifest-path template/Cargo.toml --all-targets -- -D warnings
```

The template's own binary checks run from its folder, as CI's `template` job does:

```bash
(cd template && cargo run -- --check-content && cargo run -- --worldgen-report valley --seeds 1..1000 \
  && for r in tests/replays/*.json; do cargo run -- --replay "$r" --verify || exit 1; done)
```

- `cargo test --workspace` already verifies the 19 reference replays (`reference_replays.rs`) and runs every scenario in
  `crates/sandbox/scenarios/` forward and backward in one game, and the stand scenarios on in-memory stands.
- A changed `Cargo.lock` must agree with `template/Cargo.lock`: CI's `template` job has the check, and the fix is
  `cp Cargo.lock template/Cargo.lock && cargo metadata --manifest-path template/Cargo.toml --format-version 1 >/dev/null`.
- Changed scenarios, the CLI or `shards_script`: also run them against one live game, as CI's `headless-cli` job
  does: `cargo shards --port-file target/live/sandbox.port scenario crates/sandbox/scenarios/*.scenario`, then the
  same files in reverse order (`$(ls crates/sandbox/scenarios/*.scenario | sort -r)`).
- `cargo test` and nextest must agree. nextest runs each test in its own process (`.config/nextest.toml`): a test
  that writes a fixed file name needs the test group there, and CI's profile fails a test that passes only on a retry.

## 4. Choose the kind of test

| What you added or changed | The test that proves it | Where |
|---|---|---|
| A pure function (maths, parsing, a rule over plain data) | unit test | `#[cfg(test)] mod tests` beside it |
| Behaviour of a crate's systems on a headless app | integration test with its own test content | `crates/<crate>/tests/<short>/<topic>.rs`, `mod <topic>;` in its `main.rs`, content in `tests/<short>/assets/content/` |
| Something an agent can see or do (a tool, a journal line, an NPC's choice, a fight) | scenario file | `crates/sandbox/scenarios/<topic>.scenario` (a stand's in `scenarios/stand/`) |
| An invariant (`add_engine_invariant`, `add_ai_invariant`) | control test that breaks the state on purpose and expects the stop | `crates/sandbox/tests/sandbox/engine_rules.rs` (`plant`, `stops_on`) |
| A tool | the reply on success and the error that says how to fix a bad call | the crate's tests, or `tool_contracts.rs` |
| A content kind or field | a good file loads; a typo is an error naming the file, the field and the fix | the crate's tests (`content_check.rs` in the sandbox) |
| New simulation state (component, resource, cache) | snapshot round trip, and heavy mode | modelled on `crates/sandbox/tests/sandbox/snapshots.rs`; `shards::testing::heavy_app` |
| A deliberate gameplay change | the reference replays move, and only those you expect | `tests/replays/` (section 5) |
| A bug found by fuzzing | its shrunk scenario, committed with the fix | `crates/sandbox/scenarios/regress/` |

How to write each one, with the helpers and real examples: `.agents/skills/shards-verify/references/test-kinds.md`.
Scenario files (format, setup lines, expectations, stands): `.agents/skills/shards-verify/references/scenarios.md`.

## 5. Reference replays

The 19 files in `tests/replays/` (the template's in `template/tests/replays/`) are played by their scenarios and
checked tick by tick. They move only when gameplay changes or when content the simulation read changes.

1. Predict before you look. For content edits: `python3 .agents/skills/shards-verify/scripts/which_replays.py`
   names the replays that read the definitions you changed (from their `check_parts.content`). A definition
   added to a kind they read by id moves none. Code that changes gameplay moves those that reach it.
2. Run them: `cargo test -p sandbox --test sandbox reference_replays` (template:
   `cargo test --manifest-path template/Cargo.toml --test game replays`).
3. A divergence you did not predict is a bug in the change: read `.agents/skills/shards-debug/SKILL.md`.
   Never re-record to make it go away.
4. A divergence you meant: record again, then confirm that only the expected files changed.

```bash
cargo test -p sandbox --test sandbox -- --ignored record_reference_replays
(cd template && cargo test --test game record_reference_replays -- --ignored)  # the template's
git diff --stat tests/replays/ template/tests/replays/                          # only the ones you predicted
```

Name the re-recorded replays in the CHANGELOG entry ("`parts.json` re-recorded, the eighteen others
byte-identical"). A new reference replay is a scenario plus a row in `RECORDED`
(`crates/sandbox/tests/sandbox/reference_replays.rs`).

## 6. Rules that are not negotiable

- Never skip, ignore, loosen or delete a test, an expectation or an invariant to get green. If a rule is wrong,
  say why in the change that alters it.
- A flaky test is a bug in the test or the game; find what it shares (a fixed file name, a process-wide
  resource, wall time, the order of scenarios) and fix that.
- Scenarios start by putting the world in order (`heal`, `teleport`, `set`, `npc_set_state … calm`, `regen`): CI runs
  them on a fresh game and after each other, forward and backward.
- Report what you ran and what it said, what you did not run, and why. "Tests pass" without the commands is not a report.
