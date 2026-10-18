# AGENTS.md

Instructions for coding agents (Claude Code, Codex and others) that work in this repository.

## The main rule

The engine has no LLM and must not have one. Do not add built-in or local agents, models,
calls to model APIs, API keys, or "model → tool" loops inside the game. The game is an environment that
is driven from outside: by you, through the `shards` CLI. Anything an agent needs is added as a tool
(`add_ai_tool` / `add_ai_query`) or as a `shards` command, not as code that does the thinking for the agent.

How this file works: it is the short core that every agent loads. Detail lives in `docs/agents/*.md`; the **Index** at the end says which file to read for which change. Read the topic file before you touch its area.

## Building

- System packages: `pkg-config libasound2-dev libudev-dev` (for a window under Xvfb, also `xvfb mesa-vulkan-drivers
  libxkbcommon-x11-0`).
- `cargo build --workspace`, `cargo test --workspace`. The first build of Bevy is slow;
  `.cargo/config.toml` limits it to 4 jobs (`CARGO_BUILD_JOBS=12` for a faster build when RAM is free). `template/` is a separate workspace:
  `cargo test --manifest-path template/Cargo.toml`. CI runs the tests with `cargo-nextest` (`cargo install cargo-nextest --locked`; a
  process per test, see [checking-changes.md](docs/agents/checking-changes.md)); `cargo test` stays a supported local command.
- Iterating fast: content, level, locale and scenario edits need no rebuild; Rust builds and options (`rust-lld` on Windows, `--features dynamic`, `sccache`, `CARGO_BUILD_JOBS`): `docs/manual.md`, "Building and iterating fast".
- The crate map is in [docs/agents/crates.md](docs/agents/crates.md).

## Watching the game and acting in it (brief)

```bash
cargo sandbox --headless --paused --port 0 --port-file /tmp/g.port &   # no window, paused: time moves only by step
export SHARDS_PORT_FILE=/tmp/g.port
cargo shards wait      # then: observe, tools, act TOOL k=v, step N --until COND, inspect eN
cargo shards quit      # stop the game (or kill <pid>, never kill -9)
```

Run `shards` from the game's folder or below it (the token lives in `.shards/`), take numbers from `observe` and `inspect`, not from pictures, and read tool errors: they say how to fix the call. The full cookbook and the tool catalogue are in [docs/agents/watching-and-driving.md](docs/agents/watching-and-driving.md).

## Rules every change follows

- Code, comments and documentation are in English. `README.md` is a short project page (what it is, quick start, the game
  step by step, links); the detailed documentation is `docs/manual.md` (the manual), this file, the rest of `docs/`, `CHANGELOG.md`
  and `template/README.md`. Russian stays only in game text and test data (Fluent files, locale tests).
- Bevy is pinned to 0.20.0; do not update it as a side effect of another task.
- **The boundary.** The game (the template, the sandbox) depends only on the `shards` crate and does not change the engine's code. The engine has no names
  from a particular game: if a mechanism will be useful to a second game with a different story, it goes in the engine; if it is a particular
  "what" (a weapon, an enemy, a number), it goes in the game, that is, in content.
- **Determinism essentials.** Gameplay runs in `FixedUpdate` in a `SimSet`, randomness comes only from `SimRng`, simulation maths goes through `bevy::math::ops` and never through `f32` methods (clippy fails on them), physics reads `Transform` (colliders on top-level entities, size in `Shape`, never in `Transform::scale`), the world is created in `OnEnter(EngineState::InGame)`. An event observer that changes gameplay returns at once on a machine that does not simulate (`shards::core::simulates`), or co-op desyncs. Never build with `-C target-cpu=native` or `+fma`, and never enable `scalar-math` in glam (or parry3d `enhanced-determinism`): replays diverge and rendering breaks. Full rules: [determinism.md](docs/agents/determinism.md).
- **Order by `AiId` or `ColliderId`, never by `Entity`**, not by the order of a query, and never by `HashMap`/`HashSet` order (use indices and `BTreeMap`): a replay must match with a window and without one.
- **Content, not constants.** Numbers and definitions are `assets/content/**/*.<kind>.ron`; player-read text is Fluent (`@id`). See [content.md](docs/agents/content.md).
- Characters are driven only through `Intent`: the keyboard, the agent's tools, the enemy brains and the replay write to it.
  Register a new input component through `replay_input::<C>()`, otherwise replays will diverge.
- New simulation state outside entities with `AiId` must be registered: `add_snapshot_resource` (and `add_snapshot_rebuild`/`add_snapshot_check`) for snapshots, `replay_ignore::<C>()` for cosmetics and `add_replay_check` for state outside `AiVisible` entities in replays ([determinism.md](docs/agents/determinism.md)); invariants (`add_ai_invariant`): if one fires, fix the bug, do not weaken the rule without saying why; the engine assigns `AiId`, never write it ([tools-and-invariants.md](docs/agents/tools-and-invariants.md)).

## Gate: checking a change

- `cargo test --workspace`: unit tests and scenarios in `crates/sandbox/tests/`; `cargo clippy --workspace --all-targets
  -- -D warnings` must stay silent. If you changed something the template uses, run `cargo test --manifest-path template/Cargo.toml`.
- Content: `cargo sandbox --check-content`; world: `cargo sandbox --worldgen-report wilds --seeds 1..1000` must return 0. After a deliberate gameplay or content change, record the reference replays again (`cargo test -p sandbox --test sandbox -- --ignored record_reference_replays`; template: `cargo test --test game record_reference_replays -- --ignored` in `template/`). Check a replay with `cargo sandbox --replay FILE --verify`; the template's worldgen region is `valley`. Everything else (CI, nextest, scenarios, replays, heavy mode, perf, nightly): [checking-changes.md](docs/agents/checking-changes.md).
- If you change a module's behaviour, update its section in `docs/manual.md`, `docs/engine-progress.md` (statuses) and `CHANGELOG.md`
  (`Unreleased`); update `README.md` only when what it says about the project changes (features, quick start, steps).

## Index: if your change touches X, read Y

| If your change touches | Read |
|---|---|
| Running, driving or debugging a live game; the `shards` CLI, tool catalogue, exit codes, asserts, fuzz, parallel games, `step --until` conditions | [docs/agents/watching-and-driving.md](docs/agents/watching-and-driving.md) |
| Combat tools, enemy warnings, dodge/block/attack calls | [docs/agents/combat.md](docs/agents/combat.md) |
| Systems, spawn order, maths, randomness, event observers, snapshots, replay checks, tick time | [docs/agents/determinism.md](docs/agents/determinism.md) |
| A new tool, an invariant, `AiVisible` describers, `AiId` | [docs/agents/tools-and-invariants.md](docs/agents/tools-and-invariants.md) |
| Content definitions, a new content kind, Fluent text | [docs/agents/content.md](docs/agents/content.md) |
| HUD and menus, sound and music, looks, effects, foliage, animation and baked motion | [docs/agents/ui-audio-look-fx-scatter-animation.md](docs/agents/ui-audio-look-fx-scatter-animation.md) |
| Regions, tiles, levels, generation (tiles and voxels), the navmesh and its baking | [docs/agents/world-regions-nav.md](docs/agents/world-regions-nav.md) |
| NPC brains, behaviour, perception, director, attack plans, avoidance | [docs/agents/npc.md](docs/agents/npc.md) |
| Co-op stands, `--net-sim`, Steam, lobbies, achievements | [docs/agents/net-coop-steam.md](docs/agents/net-coop-steam.md) |
| A new engine module (crate) | [docs/agents/new-module.md](docs/agents/new-module.md) |
| A new third-party dependency | [docs/agents/third-party.md](docs/agents/third-party.md) |
| A panic, a broken invariant, crash reports | [docs/agents/crashes.md](docs/agents/crashes.md) |
| How to verify: tests, CI, nextest, scenarios, replays, heavy mode, perf, nightly | [docs/agents/checking-changes.md](docs/agents/checking-changes.md) |
| Which crate owns what | [docs/agents/crates.md](docs/agents/crates.md) |
