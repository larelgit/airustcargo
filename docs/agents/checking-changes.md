# Checking changes

Read this when you choose how to verify a change: tests, CI and nextest, scenarios, content checks, replays, heavy mode, performance, nightly jobs. The gate commands are in [AGENTS.md](../../AGENTS.md). Part of the AGENTS.md index.

- When CI runs: on every pull request (one run per push to its branch), every push to `main`, release tags `v*`, and by hand (`gh workflow run CI --ref BRANCH`). A branch pushed with no pull request gets no run; open the pull request or start one by hand.
- What CI runs, with `cargo-nextest` (`.config/nextest.toml`, `template/.config/nextest.toml`; profile `ci`: two retries, and a test that
  passes only on a retry **fails** the run, so a flaky test is a bug to fix, not to rerun):
  `cargo nextest run --workspace --locked --profile ci -E 'not binary_id(sandbox::perf)'`, then the tick-time tests alone,
  `cargo nextest run --workspace --locked --profile ci-perf -E 'binary_id(sandbox::perf)' -j 1`, then `cargo test --workspace --doc --locked`
  (nextest does not run doctests); the template: `cargo nextest run --manifest-path template/Cargo.toml --locked --profile ci`. nextest runs each test in
  its own process, so an in-test `static Mutex` serialises nothing there: a test that writes a fixed file name belongs in a test group of
  `.config/nextest.toml` (`snapshot-file`, `shards-snapshots`), with the lock kept for `cargo test`. Without the profile (`cargo nextest run`) there are no retries.
  Under plain `cargo test` the tests of a binary are threads of one process, so every app they build shares the process: a system failure must reach only its own app (`ai_core::failures`: one error-handler slot per live app, 256 of them, failures of an app without one are logged and collected by nobody). A test that plants a panic is safe in-process; if another test reports its panic, that is this bug again.
- A scenario a fuzz run shrank is a regression test: `crates/sandbox/scenarios/regress/<YYYYMMDD>-<stop>-seed<S>.scenario`, which fails while its
  bug exists and is committed with the fix (`scenario_files.rs` runs each on a fresh game of its seed, forward only; see "Fuzzing" in `docs/manual.md`).
- Put behaviour that you can describe with `shards` commands into a scenario file in `<crate>/scenarios/`
  (`*.scenario`, format in `docs/manual.md`, "Scenarios (E11)"). The sandbox's `tests/scenario_files.rs` runs each file
  in-process (forward, then back, in one game), and `shards scenario` runs against a live game. The file must run on a fresh game and after
  the others: start it with `heal`/`teleport`/`set`/`npc_set_state … calm`/`regen`, which return the world to its original state.
- Content: `cargo sandbox --check-content` (or `cargo run -- --check-content` in the template) finds broken references
  and names the file. It also reads the level files (nodes, components, markers) and the baked navigation. World: `cargo sandbox
  --worldgen-report wilds --seeds 1..1000` (and `voxel_arena`; in the template, `valley`) must return 0. CI runs them.
- Check a new component or state resource with a snapshot: `crates/sandbox/tests/sandbox/snapshots.rs` takes
  a snapshot at tick N, runs on to tick M, and compares the result with a game that loaded the snapshot (one state, one journal).
- Check gameplay changes with a replay: `cargo sandbox --replay FILE --verify` names the first
  check that diverged, the entities and components, and below them their values in this run (exit code 1);
  `--verify --windowed` (under `xvfb-run -a`) does the same in a window:
  `DIVERGED at tick 60: e6 Boss: Health differs`, `  e6 now: Health { current: 390.0, max: 600.0 }`.
  If the divergence is expected, record the replay again.
- By default, checks run every 60 ticks, and the tick in the report is the first check after the divergence. To find a
  bug, record a replay with `--check-every 1` (`cargo sandbox --headless --paused --check-every 1`). Then the
  report names the exact tick. The value is stored in the file, so the flag is not needed when the replay plays back.
- Heavy mode checks snapshots along a whole session: `--heavy N` (with `--paused` and `shards step`, with
  `--replay FILE --verify`, or with `--stand N`, where the host is checked) saves the state every N ticks, and a fresh twin
  that loads it must play the next `--heavy-span K` ticks (by default N, at most 600) to the same bytes and the same gameplay journal.
  A failure stops the game like a broken invariant (`stopped: heavy check failed at tick T: e5 Player: Planted differs: 1830 vs 930`,
  `step` exit code 3, `--verify` exit code 1) and saves `snapshots/heavy-tick<t>.json`, `heavy-tick<T>-original.json` and `-restored.json`, and
  `replays/heavy-tick<T>.json`. What it names is state outside the snapshot: register it (`add_snapshot_resource`, `add_snapshot_rebuild`).
  The static half is the state audit (`crates/sandbox/tests/sandbox/state_audit.rs`): a new resource is either registered for snapshots
  or added to its list with the reason it is not simulation state. In tests: `shards::testing::heavy_app`, `verify_heavy`.
- Check tick speed on a release build: the debug build compiles its own code with `opt-level = 1`.
  `cargo run -p sandbox --release -- --headless --paused --tick-budget-ms 5`, then `shards step 600`.
  The `ticks:` line shows the average, the worst tick and the number of ticks over budget. The Tracy profiler is enabled with
  `--features trace_tracy` (see `docs/manual.md`, "Tick time (E11)").
- The autotest checks performance as a whole: `cargo run -p sandbox --release -- --perf reference --offscreen` (the scene,
  warm-up, measurement and budgets are content of kind `perf`, `*.perf.ron`; without `--offscreen`, it runs only ticks). It prints a line per budget
  (`ok`, `OVER`, `-`) and exits with code 0, 1 (a budget is exceeded or the game did not reach the measurement) or 2 (no such definition, or `--offscreen`
  has no GPU). A debug build gives no verdict.
- The reference replays `tests/replays/*.json` (obstacle course, combat prototype, token director, guard on the navmesh, region changes,
  a replay from a snapshot, a replay of the sandbox's saves, the hit-outcome sparring, the voxel arena and its navmesh chase (`voxel_nav.json`), the timeline (`timeline.json`), the context and body-state replays, the encounter and the spawned combat objects (`objects.json`, the boss scheduler's gates `boss_schedule.json`, the downed state and the revive `downed.json`, the body parts `parts.json`, the weapons `weapons.json`, the movers' room `movers.json`): 21 files in all) are checked by `cargo test` (and by nextest). After a deliberate change to gameplay, or to a definition they read (by id, or a kind
  they read whole: `check_parts.content` in the file lists them), record them again; content they never read, and kinds outside the
  simulation (looks, sounds, effects, animation sets, screens' themes), do not make them diverge:
  `cargo test -p sandbox --test sandbox -- --ignored record_reference_replays` (for the template —
  `cargo test --test game record_reference_replays -- --ignored` in `template/`).
- The long checks are not run by hand but by `.github/workflows/nightly.yml` (every reference replay with `--heavy 10 --heavy-span 30`, worldgen on 100000 seeds, the GPU tests, performance), `soak.yml` (half an hour of a live stand) and `cross.yml` (the reference replays on Windows and Ubuntu 22.04, weekly); a failed night opens an issue labelled `nightly`. Their commands are the ones above with bigger numbers: the nightly worldgen is `--worldgen-report wilds --seeds 1..100000` (and `valley` in the template; the voxel arena, at about 60 ms a seed, `--seeds 1..10000`), not the 1000 seeds of `ci.yml`.
