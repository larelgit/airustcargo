# How to write each kind of test

Every pattern here has a living example in the repository; open it and copy its shape. Tests drive the game
the way an agent does (tools, `step`, the journal), and assert on exact text: a reply, a journal line, an error.
Name a test as a sentence about the behaviour (`a_capture_at_the_target_refuses_the_option_until_its_fade`).

## Unit tests

Beside the code, in `#[cfg(test)] mod tests`. The engine pulls logic out into pure functions so that it can be
tested without Bevy (`mount_places`, `refusal()`): when the logic you add can be a function of plain data, make
it one and test it here.

## A crate's integration tests

- One test binary per crate, found by Cargo: `crates/<crate>/tests/<short>/main.rs` (`shards_npc` → `npc`,
  `shards_action` → `combat`). Add `<topic>.rs` beside it and `mod <topic>;` to `main.rs`. Do not add a binary:
  each one links Bevy (time and gigabytes), and the docs count them.
- Test content lives in `crates/<crate>/tests/<short>/assets/content/` (`live.action.ron`, `live.attack_plan.ron`);
  the binary's `common.rs` builds a headless app over that folder and has helpers to spawn bodies.
- A crate that needs the whole engine uses the umbrella as a dev-dependency and `shards::testing::test_app`, like
  `crates/shards_rules/tests/rules/`.
- Example: `crates/shards_npc/tests/npc/live.rs`: the header says what the test content is, small helpers
  first, then tick-exact assertions on tool replies (`npc_schedule`) and on what the systems did.

## The helpers

```rust
use shards::testing::{call, ok, test_app};
let mut app = test_app(|config| config.seed = 42, |app| { app.add_plugins(MyPlugin); }); // headless, loaded, paused at tick 0
ok(&mut app, "step", json!({"ticks": 60}));            // must succeed; panics if the game halted
let out = call(&mut app, "teleport", json!({}));       // may fail: assert on out.ok and out.result
```

- `shards::testing::config()` is the test config: headless, paused, engine invariants on, no tick timing in
  replies (`report_perf: false`), so whole journals compare equal run to run.
- A test app gets a save folder of its own, removed with it.
- The sandbox: `sandbox::test_app(seed)`; the template: `my_game::test_app(seed)`, with `say`, `fails`, `step`,
  `journal`, `story` (gameplay events only) and `state` in `template/tests/game/main.rs`.
- Entities by id: `app.world().resource::<AiIndex>().get(AiId(2))`. Never compare `Entity` values across runs.

## A game's tests

One binary, `tests/game/` (`template/tests/game/main.rs` lists the modules and what each checks). Add a module per
feature; keep `manifest.rs` (only `shards` as a dependency), `determinism.rs` and `replays.rs` green.

## Invariants and their control tests

An invariant is `fn(&World) -> Option<String>`: it only reads, and names the offending ids lowest first
(`shards::ai_core::Offenders`). Its control test breaks the state on purpose and expects the stop:

- Engine rules: `crates/sandbox/tests/sandbox/engine_rules.rs`. `plant(app, tick, corrupt)` adds a system in
  `SimSet::Cleanup` that corrupts the world at that tick; `stops_on(app, "<module>.<rule>", tick)` expects
  `stopped: invariant <name> violated at tick N` and returns the journal line, which the test checks names the entity.
- A game's rule: `a_grunt_chance_outside_zero_to_one_stops_the_game` in `template/tests/game/content.rs` (`set` a bad
  value with the dev tool, `step`, compare the exact reply).

## Tools

Test the reply on success and every refusal; a refusal must say how to fix the call (`taunt_errors_say_what_to_do`
in `template/tests/game/content.rs`; `crates/sandbox/tests/sandbox/tool_contracts.rs` for the engine's). An action
tool is journaled and recorded in replays, a query is neither: a test that replays a session covers the first.

## Content kinds

A clean file loads; a typo is an error that names the file, the definition, the field and the closest id. Write the
bad file into a temporary folder and read it with `check_dir` (`a_typo_in_the_level_is_an_error_that_names_the_fix`
in `template/tests/game/content.rs`; `crates/sandbox/tests/sandbox/content_check.rs`). Show that an edit applies on
the next tick without a rebuild when that is the promise (`editing_a_number_in_content_changes_the_next_hit`).

## Snapshots, heavy mode and the state audit

- New state outside entities with an `AiId`, or a component a system adds: take a snapshot at tick N, run on to M,
  and compare with a game that loaded the snapshot (one state, one journal). Models:
  `crates/sandbox/tests/sandbox/snapshots.rs`; for world state, `crates/shards_world/tests/world/regions.rs`.
- Heavy mode, the same along a whole session: `shards::testing::heavy_app(configure, setup, HeavyConfig { .. })`
  and `step`, or `verify_heavy(recording, configure, setup, heavy)`; examples in `crates/sandbox/tests/sandbox/heavy.rs`.
- `crates/sandbox/tests/sandbox/state_audit.rs` fails on any resource or `AiId` component that is neither saved by
  snapshots nor listed in `OUTSIDE` / `COMPONENTS_OUTSIDE` with the reason it is not simulation state.

## Replays and determinism

- Round trip: play, `replay::save(world)`, through JSON, `replay::play` on a fresh app of the same seed, step to
  the end, `app.update()`, then `AiReplay::report()` must be `identical()`: `template/tests/game/replays.rs`. Or
  `shards::testing::verify(&mut app, recording)`.
- Same seed twice gives the same journal and state; `step 300` equals five `step 60` with frames between:
  `template/tests/game/determinism.rs`.
- `gameplay_systems_have_a_total_order` (the sandbox, `shards_npc`, `shards_action`) fails on a gameplay system
  with no explicit order against the others: order it, never exempt it.

## Stands (co-op)

A stand scenario file is the first choice (`scenarios/stand/`, run by `shards::testing::run_stand_scenario`). For
assertions a scenario cannot express: `shards::testing::stand(clients, links, configure, setup)` and its driver;
examples in `crates/sandbox/tests/sandbox/net_*.rs`.

## What tests must not do

- Write a fixed file name in a crate folder (`snapshots/x.json`, `replays/`, `crashes/`) without the nextest test
  group (`snapshot-file`, `shards-snapshots` in `.config/nextest.toml`) and the `static Mutex` kept for `cargo test`.
- Measure wall time anywhere but `crates/sandbox/tests/perf.rs`, which CI runs alone.
- Depend on another test having run first, on `HashMap` order, or on `Entity` values.
