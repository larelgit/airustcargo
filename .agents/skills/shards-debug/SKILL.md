---
name: shards-debug
description: Find and fix the root cause when the Shards engine or a game built on it misbehaves - a step that stops on a panicking gameplay system or a violated invariant, a replay or reference replay that DIVERGED, a snapshot that does not restore, a heavy-mode failure, a co-op desync, a flaky or order-dependent test, a content check error, clippy's disallowed maths, or a game that will not start or shows an empty world. Use as soon as a test, a scenario, --verify, CI or a live game reports one of these.
---

# Debugging

The engine is deterministic and records everything, so every failure here can be replayed to the tick it
happened on. Work in this order: **reproduce, find the first wrong tick, explain it, fix the cause, keep a
regression test.** Paths are from the repository root.

Never make a failure go away by changing the check: do not loosen an invariant, re-record a replay you cannot
explain, add a retry, `#[ignore]` a test or delete an expectation. If a rule is genuinely wrong, the change
that alters it says why (`docs/agents/tools-and-invariants.md`).

## Triage

| You see | It means | Go to |
|---|---|---|
| `stopped: gameplay system <name> panicked at tick N` (`step` exit 3) | a system panicked; the game caught it and saved `replays/crash-tick<N>.json` | [Panic](#a-panic-or-a-broken-invariant) |
| `stopped: invariant <name> violated at tick N` | a rule broke; the journal names the entity; `replays/invariant-<name>-tick<N>.json` | [Panic or invariant](#a-panic-or-a-broken-invariant) |
| `DIVERGED at tick T: e6 Boss: Health differs` (`--verify` exit 1, `reference_replays` fails) | the game no longer plays the recording the same way | [Divergence](#a-replay-diverged) |
| `the restored state differs`, `heavy check failed at tick T: … differs: a vs b`, `state_audit` fails | simulation state that snapshots do not hold | [Snapshot state](#state-a-snapshot-misses) |
| `net: desync at host tick T: e7 Trophies: here …, host …` | a client's world drifted from the host's | [Desync](#a-co-op-desync) |
| a test passes alone and fails in the suite, or passes only on a retry | shared state between tests | [Flaky](#a-flaky-or-order-dependent-test) |
| `--check-content` errors, `displacement is stale`, a stale navmesh file | content and its bakes disagree | [Content](#content-errors) |
| clippy `disallowed-methods` on `sin`, `powf`, … | platform maths in simulation code | `ops::sin(x)`, `FloatPow::squared`; `docs/agents/determinism.md` |
| `gameplay_systems_have_a_total_order` fails | a gameplay system has no explicit order | give it `.before`/`.after` the module sets (`docs/manual.md`, "Tick order") |
| the game exits 3 at start, `shards wait` times out, `unauthorized`, an empty world | a launch problem | [Launch](#the-game-will-not-start-or-is-empty) |

## A panic or a broken invariant

```bash
cargo sandbox --replay replays/crash-tick<N>.json --verify     # reproduces: exit 1 while the bug is there
python3 .agents/skills/shards-playtest/scripts/live.py start --name crash -- --replay replays/crash-tick<N>.json
cargo shards --port-file target/live/crash.port step <N-1>      # one tick before; then inspect what the message names
cargo shards --port-file target/live/crash.port inspect e6
cargo shards --port-file target/live/crash.port step 1          # the failure repeats on tick N
```

- The journal line and `crashes/crash-<time>-tick<N>.json` hold the panic message, its location and stack.
  `docs/agents/crashes.md` has the details (a panic on another thread, crash reports from players).
- An invariant's text names the offender (`e6 Health.current = 700 outside 0..=600`). Find which system wrote
  that value on tick N (the invariant runs after the tick): read the systems that touch the component, in tick
  order (`docs/manual.md`, "Tick order"). An engine rule firing on a game's state is still a bug in the engine
  or the game, not in the rule.
- Fixed when `--verify` on the same file no longer reports the panic or the violation. It may then say
  `DIVERGED`: that only means the fixed system plays differently from the broken one.
- Regression test: a test or a scenario that reaches the same state. For a fuzz find, commit the shrunk
  scenario in `crates/sandbox/scenarios/regress/` with the fix (it fails while the bug exists).

## A replay diverged

Read the report first: `DIVERGED at tick 60: e6 Boss: Health differs` and the `e6 now: …` lines under it name
the entity, the component and its value now. The tick is the first check after the divergence (checks run
every 60 ticks unless the recording says otherwise).

1. **Did you mean to change gameplay or content it reads?** Predict with
   `python3 .agents/skills/shards-verify/scripts/which_replays.py`; if the divergence is exactly what you
   changed, record again (`.agents/skills/shards-verify/SKILL.md`, section 5). If not, it is a regression:
   keep going.
2. **What it names:**
   - `replay check content differs: action/light_1 differs`: a definition the session read changed.
   - `e15 missing (Skeleton despawned at tick 40)` / `e27 extra (Skeleton)`: entities appear in another order or
     not at all; the spawn order changed, or something spawns in `Startup` or orders by `Entity`.
   - A component of one entity: find the systems that write it (`rg 'mut .*<Component>' crates`), and compare
     with what your diff changed in them or in what they read.
3. **The exact tick:** record the same scenario with checks on every tick on the build before your change
   (`git stash`, or a worktree of the base commit; that build costs another `target/`), with
   `--check-every 1` (`live.py start -- --check-every 1`, `scenario FILE`, `save-replay replays/before.json`),
   then `cargo sandbox --replay replays/before.json --verify` on your build names the first tick that differs.
4. **Diverges only with a window** (`--verify --windowed` under `xvfb-run -a`): something orders by `Entity` (a
   window adds render entities), presentation writes a simulation component, or the picture reads content
   inside the simulation's scope (`docs/agents/content.md`).
5. **Diverges only on another OS** (`cross.yml`): an `f32` maths method instead of `ops`, a build with
   `target-cpu=native` or `+fma`, or bytes that differ by line endings (`.gitattributes` keeps hashed files LF).

## State a snapshot misses

`snapshot_load` says `the restored state differs`, heavy mode stops (`heavy check failed at tick T: e5 Player:
Planted differs: 1830 vs 930`), a replay from a snapshot diverges at its start, or `state_audit.rs` names a type.

- The name in the message is the state: a component on an `AiId` entity needs `Reflect`,
  `#[reflect(Component)]` and `register_type`; a resource needs `add_snapshot_resource`; a cache built from
  state needs `add_snapshot_rebuild`; what the world is built around needs `add_snapshot_check`
  (`docs/agents/determinism.md`, "Snapshots, replays, tick time").
- Heavy mode writes `snapshots/heavy-tick<t>.json`, `heavy-tick<T>-original.json` and `-restored.json`: diff the
  last two to see exactly what the twin lacked. Reproduce with `cargo sandbox --replay FILE --verify --heavy 30`.
- `state_audit.rs`: register the type, or add it to `OUTSIDE` / `COMPONENTS_OUTSIDE` with the reason it is not
  simulation state. "It was not needed" is not a reason.

## A co-op desync

The desync line names the host tick, the entity, the component and both values. The usual causes:

- An observer of a combat event that changes gameplay without `if !shards::core::simulates(role) { return; }`: the
  host forwards events to clients for effects, so the client counts twice (`count_the_fallen` in
  `template/src/progress.rs` is the pattern).
- Iteration in `HashMap`/`HashSet` or `Entity` order, or randomness outside `SimRng`/`DetRng`.
- Host-only state the client predicts or changes, or a presentation component sent over the network
  (`net_ignore`).

Reproduce on a stand: `live.py start --name coop -- --stand 2 --net-sim latency=150,loss=0.02`, then the stand
scenario or `cargo shards --port-file target/live/coop.port soak --minutes 3`; keep `invariant !journal "desync"`
in the stand scenario that covers the fix. `docs/agents/net-coop-steam.md` has the rules.

## A flaky or order-dependent test

CI's nextest profile fails a test that passes only on a retry. Find what the test shares:

- A fixed file name in the crate folder (`snapshots/`, `replays/`, `crashes/`): nextest runs tests in parallel
  processes; add the test to a group in `.config/nextest.toml` and keep the `static Mutex` for `cargo test`.
- Process-wide state under `cargo test` (one process per binary): a panic hook, a tracing subscriber, a static.
- Wall time: only `crates/sandbox/tests/perf.rs` may measure it.
- A scenario that relies on what ran before it: CI runs them forward, backward and alone; add the setup lines.

Run it the CI way to reproduce: `cargo nextest run -p <crate> --profile ci -E 'test(<name>)'`, and alone under
`cargo test -p <crate> --test <bin> <name>`.

## Content errors

`cargo sandbox --check-content` (template: `(cd template && CARGO_TARGET_DIR=../target cargo run -- --check-content)`) names the file, the line
and column, the definition and usually the fix (`did you mean grunt?`). Bakes it reports as stale:
`cargo sandbox --bake-nav` (tiles, level files, nav settings), `cargo sandbox --bake-motion` (clips of an animset);
commit what they write. A missing `@id` text is a message to add in every `assets/locale/<language>/*.ftl`.

## The game will not start or is empty

- Exit 3 at start: the port is busy; use `--port 0 --port-file F` (what `live.py` does).
- `shards wait` timed out: the game was still compiling; build first, or read `target/live/<name>.log`.
- `unauthorized`: `shards` runs outside the game's folder; run it from the repository root for the sandbox, or
  pass the template's token (`live.py start` prints it).
- An empty world: a built binary run without `cargo` finds no `assets/` (`BEVY_ASSET_ROOT=crates/sandbox`), or the
  world spawns in `Startup` instead of `OnEnter(EngineState::InGame)`.
- `built without the render feature`: `--offscreen` or a window needs the `render` feature (on by default).

## When it is fixed

Name the cause in the commit message, not the symptom, and keep the regression test with the fix. Then run the
checks the change needs: `.agents/skills/shards-verify/SKILL.md`.
