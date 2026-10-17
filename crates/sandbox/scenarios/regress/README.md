# Regression scenarios

Scenarios the fuzzer wrote after it broke a rule (an engine invariant, a panicking system, a
heavy check), shrunk by `shards_script::minimize` to the few inputs that still break it. See
`docs/manual.md`, "Fuzzing".

- Name: `<YYYYMMDD>-<stop>-seed<S>.scenario`. The header comment holds the seed, the number of
  inputs before and after shrinking, and the stop text. Then `seed S` and the `act?` and `step`
  lines as played, nothing else.
- A file **fails while the bug exists**: the broken rule halts the game inside a `step`, and a
  step cut short fails a scenario. Commit the file together with the fix, as its regression test.
- `crates/sandbox/tests/sandbox/scenario_files.rs` runs every `*.scenario` here, each on a fresh
  game of the seed its `seed` line names, forward only (not in the shared game of the others).
  `shards scenario crates/sandbox/scenarios/*.scenario` does not reach this folder: the files
  need a game that starts at tick 0.
