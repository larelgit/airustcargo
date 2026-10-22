# The review checklist

Each item says what to look for in the diff, the rule behind it, and what catches it when nobody looks. "Review
only" means no test, check or script does: spend the time there. CI covers the rest, but often only for the
behaviour some test happens to exercise. `red_flags` is `.agents/skills/shards-review/scripts/red_flags.py`.

## The main rule and the boundary

- **Code that thinks for the agent**: a model call, an API key, a built-in agent loop, a "suggest the next move"
  helper. The game is driven from outside; what an agent needs is a tool, a query or a `shards` command (`AGENTS.md`,
  "The main rule"). Caught by: `red_flags` (`llm`) for dependencies and keys; the rest, review only.
- **An engine crate that knows one game**: an id, a component or a rule of the sandbox or the template, or a
  mechanism only one game would use; or a game that changes engine code or depends on anything but `shards`
  (`AGENTS.md`, "The boundary"). Caught by: `red_flags` (`boundary`, string ids only); `template/tests/game/manifest.rs`
  (the template's dependencies); the rest, review only.
- **A third-party crate**: supports Bevy 0.20, is maintained, is pinned exactly (`=x.y.z` in `[workspace.dependencies]`)
  and stays behind the engine's API: none of its types in a public signature, and no game imports it
  (`docs/agents/third-party.md`). Review only.

## Determinism

- **A gameplay system outside `FixedUpdate`, outside a `SimSet`, or unordered** against the module sets whose data it
  touches. Caught by: `gameplay_systems_have_a_total_order` (ambiguous access in `FixedUpdate`, in the sandbox's,
  the combat and the NPC tests); a gameplay system in `Update` by nothing (`red_flags` `schedule` asks).
- **Platform maths** (`x.sin()`, `powf`, `powi`, `atan2`, on `f32` or `f64`): clippy's `disallowed-methods`
  (`clippy.toml`, in the root and in `template/`).
- **Randomness** not from `SimRng` or a keyed `DetRng`; a std hasher whose result decides anything. Caught by: `red_flags`
  (`randomness`).
- **Order by `Entity`, by query order, or by `HashMap`/`HashSet` iteration** reaching the simulation, a reply, a journal
  line, a snapshot or a save. Caught by: CI's `replays-window` job and heavy mode, only where a reference replay or a
  heavy run reaches the code; a `HashMap`'s order can stay the same in every test and change with the insertion history
  or a dependency update. `red_flags` (`entity-order`, `hash-order`) asks.
- **Real time deciding gameplay** (`Instant`, `Time<Real>`, `Time<Virtual>::relative_speed()`; the player's speed is
  `sim::speed()`). Review; `red_flags` (`real-time`) asks.
- **The world built before content loads** (`Startup`, a loading state) instead of `OnEnter(EngineState::InGame)`; ids
  then differ between runs. Review; `red_flags` (`startup`) asks.
- **Physics reading `GlobalTransform`**, a collider on a child entity, a size in `Transform::scale` instead of `Shape`.
  Review; `red_flags` (`global-transform`, `scale`) asks.
- **An observer that changes gameplay** without returning at once when `!shards::core::simulates(role)`. Caught by: a
  stand that exercises it (`net: desync … here …, host …`); `red_flags` (`observer`) asks.
- **Facing read from a camera; a lethal hook that reads the network or orders bodies by `Entity`**
  (`docs/agents/determinism.md`, the last items). Review only.

## Simulation state

- **A resource the simulation keeps**: `Reflect` and `add_snapshot_resource`; a cache built from state:
  `add_snapshot_rebuild`; files or geometry the world is built around: `add_snapshot_check`; a reference to an entity
  without an `AiId`: `add_snapshot_entity_key`. Caught by: the state audit (`crates/sandbox/tests/sandbox/state_audit.rs`)
  and heavy mode, where a run reaches it.
- **A component on bodies with an `AiId`**, a private marker included: `Reflect`, `#[reflect(Component)]`,
  `register_type`; a cosmetic one also `replay_ignore`; one kept off the wire `net_ignore`. Caught by: the state audit,
  for components the sandbox's sessions put on bodies; a snapshot round trip (`crates/sandbox/tests/sandbox/snapshots.rs`)
  when the change adds one. `red_flags` (`state`) asks about every new `Component` or `Resource` without `Reflect`.
- **A new input component** without `replay_input::<C>()`: replays of a session that used it diverge. Review only,
  unless a replay test uses the input.
- **State outside `AiVisible` entities** (a resource, a projectile without an id) without `add_replay_check`: a replay
  passes while it drifts. Review only.
- **A `Local` in a gameplay system**: state no snapshot holds. Heavy mode, where a run reaches it; `red_flags`
  (`local-state`) asks.
- **An `AiId` written by hand** (`insert(AiId(..))`, `get_mut::<AiId>`): the engine assigns ids. `red_flags` (`ai-id`) asks.

## Content and text

- **A number or a definition in Rust** that someone would tune: it belongs in `*.<kind>.ron`. The manual's "Numbers
  still in Rust code (follow-ups)" lists the known ones; a new one goes there or into content. Review only.
- **A new content kind**: `ContentDef` with `KIND`, `#[serde(deny_unknown_fields)]`, a `validate` that declares every
  reference (`refs.require`), `SIMULATION = false` when only the picture, the sound or services read it, `add_content`,
  a typo test. Caught by: `--check-content` for broken references; the rest, review.
- **A field added to an existing kind** without `#[serde(default, skip_serializing_if = …)]`: existing content no longer
  parses, or its hash moves and replays diverge. Caught by: JSON-pinning tests where a kind has one
  (`existing_animsets_keep_their_json` in `crates/shards_action/tests/combat/golden.rs`) and the reference replays for
  kinds they read.
- **Content read in the wrong place**: gameplay that reads with `peek` (the replay's content check then misses it), a
  describer or a picture that reads with `get` (it counts), content read at `Startup` or cached at load
  (`docs/agents/content.md`). Caught by: `content_reads_are_the_same_headless_and_drawn` (GPU, ignored, nightly) for
  the picture; the rest, review only.
- **Text the player reads** that is not Fluent (`@id` in content, `shards::locale::text` with `Args` in code), a
  sentence glued from pieces, a message missing from a language. Caught by: `--check-content` for an `@id` with no
  message; hard-coded strings, review only. Russian outside `.ftl` and test data: `red_flags` (`language`).

## The agent surface

- **A tool**: a doc comment on the argument struct (the description), required arguments as plain fields, errors that
  say how to fix the call, ids through `Id::entity`, the right kind (`Query`, `Action`, `Control`) and scope (`Player`
  acts through `Intent` and needs a body), its co-op route, `f32` rounded in `f64` before `json!`. Caught by:
  `crates/sandbox/tests/sandbox/tool_contracts.rs` (no panic, a refusal names the signature, action sessions replay);
  `red_flags` (`undocumented`); the wording, review only.
- **An engine invariant**: named `<module>.<rule>`, reads only, names offenders lowest id first, its doc comment says what
  and why and any carve-out; a control test that plants the bug; its name in `ENGINE_RULES` and the count in
  `crates/sandbox/tests/sandbox/invariants.rs`. Caught by: that file, for the list and the count; a missing control
  test, review only.
- **A rule weakened**, an expectation loosened or a test skipped to make CI green, without a stated reason. Review only.
- **What agents read**: a describer for new state, a journal line with ids for a new event, an `ai/status` field, and a
  new tool or condition in the catalogue (`docs/agents/watching-and-driving.md`). Review only.

## Co-op

- **Who decides**: gameplay changes only where `simulates(role)` holds; a host-only decision is
  `NetRoute::HostOnly(why)`; a query a client cannot answer refuses there; a client's own body is read as it predicts it
  (`shown_state`).
- **What the client sees**: replicated state, net events for what its HUD and journal need, nothing kept off the wire
  that it reads.
- **Saves**: a character section travels with its player; world sections are the host's.
- Caught by: the stand tests (`crates/sandbox/tests/sandbox/net_*.rs`) and the live stand scenarios in CI, for what they
  exercise. A new mechanic with no stand scenario is caught by nothing.

## Tests

- **New behaviour has a test of the right kind** (`.agents/skills/shards-verify/references/test-kinds.md`): a crate test,
  a scenario for what an agent sees, a control test for a rule, an error test for a tool, a typo test for a content
  kind, a snapshot round trip for new state, a stand scenario for co-op. Review only.
- **The test proves the point**: expectations copied from the reply, not loosened until they pass; a `step --until`
  followed by `expect stopped`; a scenario that leaves the world as it found it (it runs forward and backward with the
  others: `.agents/skills/shards-verify/references/scenarios.md`).
- **Parallel safety**: a test that writes a fixed file name sits in a nextest test group (`.config/nextest.toml`); a flaky
  test is a bug (CI's profile fails a test that passes only on a retry).

## Reference replays and bakes

- **A replay changed**: the change that moved it is named (`.agents/skills/shards-verify/scripts/which_replays.py`), only
  the expected files changed (`git diff --stat -- tests/replays template/tests/replays`), and it was recorded with
  `record_reference_replays`, never edited by hand. `red_flags` (`replay`) lists each changed file.
- **Gameplay moved and no replay changed**: CI's reference replay test fails; a re-record that hides an unexplained
  divergence is a determinism bug (`.agents/skills/shards-debug/SKILL.md`, "A replay diverged").
- **Bakes**: an edited tile or level file needs `--bake-nav` and its output committed, an edited clip `--bake-motion`, a
  generator whose output changed a raised region `version`. Caught by: `--check-content`, which names a stale bake, and
  the worldgen reports.

## Docs

- The module's section of `docs/manual.md`, `docs/engine-progress.md` (statuses, test counts), `CHANGELOG.md`
  (Unreleased), the topic in `docs/agents/`, the catalogue for a new tool or condition, `template/README.md` for the
  template; `README.md` only when what it says changes. `.agents/skills/shards-verify/scripts/plan.py` lists them as
  `[todo]` or `[done]`. Review only: no test fails on a missing doc.
- `AGENTS.md` stays under 9 KiB (`crates/sandbox/tests/agents_index.rs`).
- Every sentence the change adds is true of the code it ships. Review only.

## Dependencies and builds

- Bevy stays at 0.20.0 (`red_flags` `bevy`); no `target-cpu=native`, `+fma`, glam `scalar-math` or parry3d
  `enhanced-determinism` (`red_flags` `build-flag`).
- `template/Cargo.lock` agrees with `Cargo.lock` (CI's template job; `plan.py` checks it).

## The commit

- Code, comments and docs never name the agent or model that wrote them (`red_flags` `attribution`); everything is in
  English except game text and test data.
- Nothing generated by accident: `git status` shows no stray files (a `scenarios/regress/` find left where `shards`
  wrote it, a `__pycache__`, a screenshot); `target/`, `saves/`, `replays/`, `snapshots/`, `shots/`, `crashes/` and
  `.shards/` are ignored, so a file from them in the diff was added on purpose.
- One concern per change: an unrelated refactor or a drive-by fix goes in its own commit or pull request.
