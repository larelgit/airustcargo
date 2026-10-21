---
name: shards-engine
description: Change or extend the Shards engine in crates/ (ai_core, shards, any shards_* module, shards_script, shards_cli, or the sandbox that exercises them) the way this repository expects - a mechanism, system, tool, invariant, describer, content kind, field or new module - with determinism and snapshot state designed in, crate tests, a sandbox scenario, reference replays handled on purpose, and the manual, engine-progress and CHANGELOG updated. Use for any engine feature, fix or refactor; for a game on the engine (template/ or a game repository) use shards-game.
---

# Changing the engine

A finished engine change in this repository is: the code; tests in the crate (and a control test for every
invariant); what an agent needs to see and do it (a tool, a describer, a journal line); a sandbox scenario when
the behaviour is visible to an agent; reference replays that verify unchanged, or are re-recorded on purpose;
and the manual, `docs/engine-progress.md` and `CHANGELOG.md` saying what is now true. Every merged change in
`git log` has this shape. Paths are from the repository root.

## 1. Orient before you write

1. `AGENTS.md` is the rulebook. Its index names the topic file for every area: read each one your change
   touches before you touch it (`docs/agents/determinism.md` for anything in the simulation).
2. Find the owner: `docs/agents/crates.md`, then the module's section in `docs/manual.md` (`grep -n '^## '
   docs/manual.md`). The manual says what the module promises; your change keeps or extends that promise.
3. Copy the newest change of the same kind: `git log --oneline -- crates/<crate> | head`, then
   `git show --stat <commit>`. `crates/shards_rules/src/lib.rs` is a compact whole module: content kinds, a
   snapshot resource, a replay check, systems in ordered sets, an engine rule, a condition, a status field, a
   dev entry, a journal line and tools.
4. **The boundary.** A mechanism a second game with another story would use goes in the engine; a particular
   "what" (a weapon, a boss, a number) goes in content or the sandbox. The engine never names a game's things,
   never has an LLM, agent loop or model call, and tunable numbers go in content (`*.ron`), not constants.
5. After your first edit, run `python3 .agents/skills/shards-verify/scripts/plan.py`: it names the topic files,
   tests, bakes, replays and docs that the files you touched reach.

## 2. Design against the simulation contract

Answer these before coding; `.agents/skills/shards-engine/references/state-and-determinism.md` has the table of
what each answer obliges you to do and which check catches a miss.

- **Where does it run?** Gameplay in `FixedUpdate`, in a `SimSet`, ordered against the module sets; the world is
  built in `OnEnter(EngineState::InGame)`; presentation in `Update`/`PostUpdate`, never writing simulation state.
- **What state does it add?** Every component on an `AiId` entity, every resource and cache: snapshot-registered,
  replay-checked or excused with a reason. Inputs go through `Intent` or `replay_input`.
- **What does it read from content, and where?** By id, inside a tick, an action or a builder; new fields
  serialize only when set, so existing content, hashes and replays do not move.
- **Who simulates it?** Only the host in co-op: event observers that change gameplay return on a client.
- **What does an agent see and do?** A tool (its errors say how to fix the call), a describer, a journal line, a
  condition for `step --until`, an invariant for what must always hold.
- **Which reference replays may change?** Decide now. An addition that existing content does not use must leave
  all 19 byte-identical; a deliberate gameplay change names the ones it moves.

## 3. Implement

Patterns with the files to copy from: `.agents/skills/shards-engine/references/patterns.md` (a tool, a
condition, an invariant and its control test, a content kind, a describer and journal line, snapshot state, a
dev menu entry, a new module). The rules that bite most often:

- Maths in the simulation: `ops::sin`, `ops::atan2`, `FloatPow::squared`, never `f32` methods (clippy fails).
- Order by `AiId` or `ColliderId`; `BTreeMap`, not `HashMap`, wherever order can reach the world.
- Randomness: `SimRng` in a tick (`rng.stream(id, salt)`), `DetRng` keyed by seed and purpose in generation and
  in draws outside an entity's brain.
- Colliders on top-level entities, size in `Shape`, never in `Transform::scale`; physics reads `Transform`.
- A third-party crate only per `docs/agents/third-party.md` (pinned `=x.y.z`, hidden behind the engine's API),
  then `cp Cargo.lock template/Cargo.lock` and `cargo metadata --manifest-path template/Cargo.toml
  --format-version 1 >/dev/null` so the two lockfiles agree. Bevy stays at 0.20.0.
- A new crate: `docs/agents/new-module.md` (one line in `EnginePlugins`, `add_engine_module`, its tools, content,
  invariants, a scenario, a replay, a manual section).

## 4. Test it where this repository tests it

`.agents/skills/shards-verify/references/test-kinds.md` has the patterns and the helpers. In short:

- Unit tests for pure logic beside it; integration tests in the crate's one test binary,
  `crates/<crate>/tests/<short>/<topic>.rs` plus `mod <topic>;` in its `main.rs`, with test content in
  `tests/<short>/assets/content/` (`crates/shards_npc/tests/npc/live.rs` is a good model).
- A control test for each invariant (`crates/sandbox/tests/sandbox/engine_rules.rs`), and the success and error
  replies of each tool.
- A sandbox scenario for behaviour an agent can see: content in `crates/sandbox/assets/content/` if the sandbox
  needs some to show it, the file in `crates/sandbox/scenarios/`
  (`.agents/skills/shards-verify/references/scenarios.md`). A new reference replay for a module's showcase:
  a row in `RECORDED` (`crates/sandbox/tests/sandbox/reference_replays.rs`).
- New simulation state: a snapshot round trip; the state audit must pass.
- See it work in the real game, not only in tests: `.agents/skills/shards-playtest/SKILL.md`.

Then run the checks: `.agents/skills/shards-verify/SKILL.md` (plan, fast loop, gate, replays). When something
fails, `.agents/skills/shards-debug/SKILL.md`.

## 5. Say what is now true

`.agents/skills/shards-engine/references/finishing.md` has the formats. Every behaviour change updates:

- `docs/manual.md`: the module's section (what it does, the API, content fields, tools, journal lines, limits);
- `docs/engine-progress.md`: the status row of the module or slice (from code and tests, not intent) and the
  crate's count in "Tests";
- `CHANGELOG.md`: one entry under `## [Unreleased]` in Added, Fixed or Changed, naming the tests and any
  re-recorded replays;
- `docs/agents/<topic>.md` when agents must follow a new rule (`AGENTS.md` stays under 9 KiB:
  `cargo test -p sandbox --test agents_index`); `README.md` only when features or the quick start change;
  `template/README.md` when the template changes.

## 6. Commit

One logical change per commit, on the branch you were given. The subject is a sentence about the behaviour,
led by the work item when there is one: `N8d: live tags on attack options …`; a review fix says so:
`K3 review: … (CodeRabbit)`. The body says why and how it is tested. Do not put model names in commits, code or
docs.
