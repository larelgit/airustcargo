# Changelog

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the versions are [semantic](https://semver.org/).

While the version is `0.x`, a minor version (`0.2` → `0.3`) may break the engine's API, and a patch (`0.2.0` → `0.2.1`) only fixes bugs.
From `1.0.0` on, breaking changes come only in a major version. All engine crates (`ai_core`, `shards_*`,
`shards`, `sandbox`, `shards_cli`) share one version: it is set once in `[workspace.package]` of the root
`Cargo.toml`. The Bevy version is pinned (`0.20.0`); updating it is a separate entry in the "Changed" section.

## How a game depends on the engine

A game depends on one crate, `shards`, and pins the engine to a **git tag**. This way the build does not change until
you change the tag yourself. While there are no tags, pin a commit: `rev = "<hash>"` (and `--rev <hash>` for the CLI).

```toml
# The game's Cargo.toml
[dependencies]
shards = { git = "https://github.com/larelgit/airustcargo", tag = "v0.4.0" }
```

Use the `shards` CLI of the same version as the engine in the game (the `ai/*` protocol may change between minor
versions):

```bash
cargo install shards_cli --git https://github.com/larelgit/airustcargo --tag v0.4.0
```

To move to a new version, change `tag`, run `cargo update -p shards`, read the "Changed" section of this
file, and run the game's tests and replays (`--verify`). Replays recorded on an old version may diverge after the engine's
gameplay changes; in that case, record them again.

## Releasing a version (for the maintainer)

Tags are set by the maintainer, not by agents or CI. For a release `X.Y.Z`:

1. Raise `version` in `[workspace.package]` of the root `Cargo.toml` and run `cargo update --workspace`
   (it updates `Cargo.lock`). Move the entries from `Unreleased` into a new section `[X.Y.Z] — date` and fix the
   links at the bottom of the file. The section may already be prepared (phase B is recorded in `[0.3.0]`, phases C–E in `[0.4.0]`, both marked "unreleased"): then set the
   date in it. Raising the minor version rejects the snapshots of the previous one: record `tests/replays/snapshot.json` again
   (`cargo test -p sandbox --test sandbox -- --ignored record_reference_replays`).
2. Update the `vX.Y.Z` tag in `template/Cargo.toml` (the comment about the dependency), in `template/README.md` (steps 3 and 5), in
   `README.md` (step 2 of the section "Make a game on Shards, step by step") and in `docs/manual.md` (step 2 of the section "Make a game
   on Shards, step by step"), replacing `rev` with `tag`, and sync
   `template/Cargo.lock` with the root one (CI checks that the dependency versions in them do not diverge):
   `cp Cargo.lock template/Cargo.lock && cargo metadata --manifest-path template/Cargo.toml --format-version 1 >/dev/null`.
3. Make sure CI is green on the commit that becomes the release, and merge it into the main branch.
4. Create and push an annotated tag on this commit:

```bash
git checkout main && git pull
git tag -a vX.Y.Z -m "Shards X.Y.Z"
git push origin vX.Y.Z
```

The tag `v0.1.0` is not yet in the history; it points to commit `e90193d`, the last one before phase A:

```bash
git tag -a v0.1.0 -m "Shards 0.1.0" e90193d
git push origin v0.1.0
```

## [Unreleased]

Changes that land in the main branch after phases C–E (`0.4.0`) go here.

### Added

- Voxel regions (`shards_voxel`, G1, A2c): a region kind of its own behind the same region API as tiles. `shards_world` gains
  `Layout::Volume` (`"volume"`) and a registry of region kinds (`RegionKind`, `app.add_region_kind`, `WorldGen::kinds`,
  `kind::generation_problems`); the manifest gains `steps` (the steps with versions that made it, written only when set, so other
  manifests keep their bytes). The content kind `voxel_region` (`*.voxel_region.ron`: size, chunk, palette, version, anchor,
  attempts, steps and params, checks, required `marks`, `markers`, `spawn`, `water.surface_drop`, `sea` with an optional
  `current`, `near_edge`, `check_seed`, `fallback_seed`, `active_radius`; neutral defaults: no sea, no current, no drop),
  validated against the game's registered steps and checks (the content kinds' context, `ContentKinds::set_context`,
  `Refs::context`). `regen`, `change_region` and `region_load` build it: the terrain field, a water volume per water box, four
  sea boxes, `Current` on the water, the markers, then the points of interest from the steps' marks; `VoxelTerrain` holds the
  volume, palette, field and hash. A manifest made by another step version is refused, naming the step (saves, snapshots,
  `region_load`). The optional snapshot and replay check `voxel.volume` (absent outside a voxel region); the in-process rebuild
  cache (`SHARDS_VOXEL_CACHE=off`, `VoxelSettings`); hot reload (`ContentAppliedHooks`, `on_content_applied`: a changed
  definition makes the region again for its seed, a colour does not); `--check-content` generates each voxel region for its
  `check_seed` and `fallback_seed`; the worldgen report's `totals:` line (`Generated::counts`). `Current`, `current_at` and
  `edge_distance` in `shards_motion` (nothing moves bodies by it yet); `PhysicsWorld::wide_colliders`. Tools `sea_current_at`
  (player) and `voxel_info` (dev, and in the developer menu); conditions `in_current` and `near_grid_edge`; the journal line
  `region loaded: <id> seed N vV (X x Y x Z, N water boxes, …)`. Until the navigation bake, a voxel region has no navmesh:
  `npc_state`, `nav_path` (refused: `use path_to`), `nav_info` and `nav_bake` say so. Snapshot checks may be optional
  (`add_optional_snapshot_check`). Manual: "Voxel regions (A2c)".
- Sandbox: the voxel arena (`regen region=voxel_arena seed=7`), a voxel region of 256 x 32 x 256 cells north of the arena,
  made by the sandbox's own steps (`island`, `lake`, `flat_arena`, `arena_pieces`, `cave`, `trees`, then the stock `fill_water`;
  `src/gameplay/terrain.rs`) and checks (`arena_flat`, `spawn_on_ground`, `pool_depth`, `cave_open`) from
  `content/world/arena.voxel_region.ron` and `terrain.voxel_palette.ron`. The scenario `voxel_arena.scenario` (steps, a ledge, the pool, the
  overhang, the cave, the current and the conditions; no NPC chase, no `nav_path`) is the new reference replay `voxel_arena.json`;
  `--worldgen-report voxel_arena --seeds 1..1000` runs in CI and 10000 seeds every night. `bounds` gains `past_grid` (meters past a
  region's grid that still count: the arena's sea). The other reference replays were recorded again for the new content: only their
  content hash changed.
- Voxel terrain, the core (`shards_voxel`, `shards::voxel`; G1, A2b): `VoxelVolume` (chunked block storage with the chunk size as a parameter, `column_top` / `column_runs`, generation edits `raise_column`, `lower_column`, `fill`, `carve`, `clear`, `sphere`, `tube`, `flatten`, an FNV `hash` over a canonical form), the content kind `voxel_palette` (`*.voxel_palette.ron`: blocks with colours, tags, `solid` / `foliage` / `liquid` / `emissive`, face shade, corner darkening, looks; checked when content loads), deterministic value `Noise` (no transcendental maths), the registry of generation steps with versions and checks (`add_voxel_step`, `add_voxel_check`, `VoxelSteps::validate_steps`), `generate` over `DetRng` with attempts, the stock steps `noise_hills` and `fill_water`, and the merged boxes `column_boxes` and `water_boxes`. A new optional module `voxel` (`--without voxel`). A shape is checked without being built (at most 2^27 cells and 65 536 chunks), edits are clipped to the volume, and a generated world's hash covers the palette. It has no region kind, collision, navigation or meshing yet. Manual: "Voxel terrain (core)".
- Hit outcome (port gap G5, B1): a hit can ask for a reaction tier and a push and a guard can price it. Content, all off by default and unserialized at their defaults (so content hashes and the reference replays do not change): `HitDef::reaction` (tier 0-3), `knockback`, `guard_damage`; `HitFlag::LineOfSight` (a wall between the chests stops the hit; the target is not marked, so a window that opens up later still hits) and `NoPerfectEvade`; `GuardDef::perfect_cost_mult`, `parry_staggers`, `break_damage_mult`, `status_mult`, `break_status_mult` (a block now applies the status share it lets through); `Cost::stamina_mode: Any` (start on any stamina above 0, drain to 0); `ActionDef::armor` (reaction armour spans) and `perfect_iframes`; `MovesetDef::reactions` (a `ReactionDef { action, push, decel }` per tier), `reaction_armor`, `reaction_immunity`, `reaction_protection`, `break_protection`, `guard_broken`, `guard_broken_push: (scale, decel)`; `CombatTuning::sight_height`. Runtime: the lazy components `Protection { reactions, poise }` and `Knockback { velocity, decel }` (in the replay and the snapshot, inserted only when granted), the system set `ActionSystems::Push` (`push_bodies`) and `tick_protection`; the events `PerfectEvaded`, `Reacted` (net events) and `ActionEnded { entity, action, instance, end: ActionEnd }` (not a net event; `finish_actions` now ends actions in `AiId` order); the functions `react` (`Reaction::hit`, `recoil`, `forced`), `poise_damage`, `end_break`, `reset_poise`, `grant_protection`. The dev tool `react id=… tier=…`, the condition `reacting`, the describers `protected …` and `pushed …`, the journal lines `e7 reacts heavy (…)` and `… perfectly evaded …`, and the engine rules `action.protection_sane` and `action.knockback_sane`. See "Hit outcome: reactions, protection, pushes (B1)" in the manual.
- Sandbox, for the hit-outcome change: the content `combat/sparring.action.ron` and `sparring.moveset.ron` (the `spar_*` actions, the movesets `sparring` and `duelist`), the scenario `reactions.scenario` (a jab asks a light reaction and a chained jab inside the immunity none, a slam a heavy one with a push, a guard priced by the hit, a perfect guard that costs half and leaves the attacker free, a perfect evade, a pillar that stops a slam, a roll on the last of the stamina) and the reference replay `tests/replays/reactions.json`. The seven older sandbox replays were recorded again for the new content (their `content` and `hash` lines changed; no engine commit before it changed one: the files verified as they were); `snapshot.json` and `saves.json`, which carry a start snapshot, also picked up the default `MoveModifiers` blocks of the movement change that had not re-recorded them.
- Locomotion modifiers (port gap G7, motion half): `MoveModifiers` (derived each tick: speed factor, ground-only factor, caps, root, no sprint, sprint cost multiplier) with the system sets `ModifierSystems` (`Reset`, `Wade`, `Statuses`, `Contribute` and their predicted twins); `StatusDef::speed_factor`, `speed_cap`, `root`; `CharacterParams` wading (`wade_min_depth`, `wade_full_depth`, `wade_factor`), fast swimming (`swim_fast_speed`, `swim_fast_drain`), `swim_acceleration`, `swim_deceleration`, `swim_turn_speed_deg`, `climb_out_auto_height`, `landing_tiers`, `landing_drop_tiers`, `landing_wade_limit`; `Landed::tier` and `jumped`, `JumpedFlight`; the tool `move_info`; a describe line on non-neutral characters. The NPC route follower, the animation picker (`BodyStatus::pace`) and the sprint's stamina cost read the modifiers. Every new field is optional and neutral by default, and unset fields are not serialized, so content hashes and reference replays do not change. Scenario `modifiers.scenario`.
- Movement options, round three (C1b), off by default and unserialized when off: `CharacterParams::climb_out_reach` (how far in front of the body a water climb-out finds a ledge, 0.3 m by default; the probe, the controller and the `jump` tool agree through `climb_reach()`, and the mantle closes the gap to a farther wall), `sprint_in_air` (the sprint flag is decided every tick in the air: pressing or releasing sprint there starts or stops the speed and the stamina drain; a glide is none), `wade_air_depth_offset` (the wading reference offset while airborne, when it differs from the standing one); `move_info` shows `climb_out_reach` and `wade_air_depth_offset`.
- Movement options for games that must match an exact rulebook (C1b), all off by default and unserialized when off, so content hashes and reference replays do not change: `CharacterParams::climb_out_max_height` (a jump climbs out onto a ledge up to that height over the water surface; the probe, the controller, the `jump` tool, `clearance` and `CharacterParams::can_jump_out` agree), `caps_scale_with_input` and `sprint_scales_with_input` (caps are ratios of the full-input speed; a sprint grows with the stick), `axis_stepping`, `brake_only_without_input` and `air_deceleration` (how the speed changes), `wade_depth_offset` and `wade_in_air` (what depth wading reads, and whether it slows an airborne body); `MoveModifiers::stance` (`stance_speed(top, by_input)`: a stance's top speed that the status slow and the pace do not scale but wading does), `wade` (`wade_by`), `acceleration` and `deceleration` (`accelerate_at`, `decelerate_at`: per-stance rates from a source); `SprintResume` (`OnRelease` or `Fraction(f)`) in `CombatTuning::sprint_resume` (land) and `CharacterParams::swim_fast_resume` (fast swim) for when a dry sprint may start again; the marker `NoStatusSlow` (a body that takes statuses but not their `speed_factor` and `speed_cap`); `move_info` shows the stance, the rates and the wading depth used (no wading outside the modes that wade, and a rooted body swims at 0).
- CI runs the tests with cargo-nextest (`.config/nextest.toml`, `template/.config/nextest.toml`): profile `ci` with two retries where a test that passes only on a retry
  fails the run (`flaky-result = "fail"`), no fail-fast, and a JUnit file uploaded as an artifact (`junit-test`, `junit-template`); profile `ci-perf` for the sandbox's
  tick-time tests, which run alone with `-j 1`; the test groups `snapshot-file` and `shards-snapshots` for the tests that share a file name or a folder. The doctests run as a step of their own
  (`cargo test --workspace --doc --locked`). `cargo test` is still the local command.
- CI tiers: `nightly.yml` (01:30 UTC, or by hand with `seed` and `minutes`): every reference replay with `--verify --heavy 10 --heavy-span 30 --heavy-offset <seed % 10>` (job `heavy`), 100000 worldgen seeds for the sandbox and the template, the `#[ignore]`d GPU tests on a software Vulkan driver under Xvfb, and the performance tests `reference` and `reference96` as an artifact; a failed night opens an issue labelled `nightly` (or comments on the open one). `cross.yml` (weekly, by hand, on `v*` tags) verifies every reference replay on `windows-latest` and `ubuntu-22.04` and writes replay, verdict and CPU model to the job summary. A job of `ci.yml` that fails uploads what the run wrote (replays, crashes, snapshots, shots, game logs) as an artifact for 14 days.
- `.gitattributes`: level files, content, replays and other text the game hashes or compares are checked out with LF on Windows too. `cross.yml` found that `ruin.gltf` came out with CRLF under the runner's `core.autocrlf`, and three reference replays that place a level then diverged (`replay check level differs`).
- Heavy mode (`ai_core::heavy`, `shards::heavy`): `--heavy N [--heavy-span K] [--heavy-offset O]` with `--paused`, `--replay FILE --verify` or an
  in-process `--stand` (the host). Every N ticks the game saves S(t); K ticks later a fresh twin from the game's own builder loads S(t), plays the
  recorded calls, inputs and session rules, and must reach the same S(t+K), byte for byte, and the same gameplay journal. A failure halts like a
  broken invariant (`stopped: heavy check failed at tick T: …`, `step`/`fuzz`/`soak` exit code 3, `--verify` exit code 1), lists the differences
  in the journal and saves `snapshots/heavy-tick<t>.json`, `heavy-tick<T>-original.json`, `-restored.json` and `replays/heavy-tick<T>.json`;
  it stays off until `assert_reset`, and `assert_list` shows its line. `EngineConfig::heavy`, `testing::heavy_app`, `testing::verify_heavy`.
- `ai_core::snapshot::differences(a, b)`: where two snapshots differ, one line per place, in a fixed order (the snapshot tests use it too).
- `ai_core::process::quiet()`: the engine's journal lines of an app that plays beside the terminal's one stay off the console.
- `JournalEntry::background`: a line logged on Bevy's IO or async compute threads (asset loading) says so; it lands in whichever app of the process is active, and heavy mode does not compare it. Those threads have their own 64 lines per tick (`N log lines of the asset threads dropped`), so a burst of loading warnings cuts no line of the game. A heavy point whose journal lost lines before they were compared is counted as skipped, not checked.
- Tests: `heavy.rs` (the combat prototype with `--heavy 300` on every push; a counter resource, a `Local` and a cache not rebuilt, each caught
  and fixed), `state_audit.rs` (every resource after the combat prototype and the region replays is registered for snapshots or named, with its
  reason, as outside the simulation), `heavy_saves.rs` (save, load in a fresh game, save again: the same sections at five ticks of every
  reference replay), and a stand whose host is checked.
- Engine invariants: seven rules every game gets for free (nine since the hit-outcome change: `action.protection_sane` and `action.knockback_sane`), checked after every tick like a game rule and listed by `shards assert list` as `engine rule`: `ai.index_consistent`, `core.intent_sane`, `motion.finite`, `action.resources_in_range`, `action.running_sane`, `npc.state_known` and `npc.tokens_within_limit`. Names are `<module>.<rule>`; an engine rule cannot be removed, replaced or shadowed by a session or game rule, and `assert_reset` re-arms it. Modules register them with `add_engine_invariant` (shards_core). Each has a control test that breaks the state on purpose and expects `stopped: invariant <name> violated at tick N`.
- `EngineConfig::engine_invariants` and the `--no-engine-invariants` flag. The default is on, and off in a build with the `dist` feature (new in `shards_core`, forwarded by `shards`): a shipped game does not pause on a rule. `shards::testing::config()` and `--verify` always check them.
- `ai_core::Offenders`: collects the entities that break a rule and reports the lowest id first with `(+N more)`, without allocating while nothing is broken. `ai_core::with_query` keeps a rule's query state between ticks in the world (the resource `RuleQueries`, dropped with it), which is what keeps the rules under 1 % of a tick.
- `template`: an example game rule, `grunt_chance` (`src/ai.rs`), with a control test.

- `shards fuzz` plays from the tool schemas: the alphabet is every tool of scope `player` and kind `action` that the game lists, with arguments made from the tool's own
  `input_schema` by `shards_script::schema` (`--alphabet schema`, the default; `--weights move=3,dodge=2`; `--invalid P` breaks one argument on purpose in that share of the calls,
  default 0.05; `--alphabet classic` keeps the four tools and the draws of the first fuzzer). `--chaos P` makes that share of the rounds a developer call instead of an input
  (snapshot saved and loaded, `regen`, `content_reload`, game saved and loaded, pause, `speed`, `heal`); a load that is refused after its save worked (the world does not come back as it was) ends the run as a finding, exit code 3. The loop moved to `shards_script::fuzz` (`run`, a `Driver`, a `FuzzReport`), so
  a test plays the same run in-process. The schema of `Id` says `"format": "entity_id"`, which is how the generator finds the arguments that take an entity id.
- The sandbox test `tool_contracts`: every query and action tool, and `step`, is called 30 times (20 valid by schema, 10 invalid on purpose); no gameplay system fails, no tool panics, an error is not empty and
  names the signature or the bad argument, and the session's replay plays back from tick 0, which proves every successful action call was recorded. Control tests plant each bug class and show the checker names it.
- State coverage (`ai_core::coverage`): `--coverage` (`EngineConfig::coverage`, off by default) counts one signature per visible entity per tick,
  made of the parts modules register with `add_coverage_part` (the engine's: `kind`, `motion`, `action`, `statuses`, `stamina`, `npc`), and
  `add_coverage_catalog` names what content declares so the tools can say what was never reached. The `coverage` query (`shards coverage
  [--top N] [--since K]`, `json=true`) lists the totals, the rarest signatures, what is new since an earlier `distinct`, and the actions,
  statuses and behaviour states never reached; `coverage_reset` (`shards coverage reset`) clears it. Only reads the world: not in the
  hash, replays or snapshots, and a game with it on records the same replay.
- `shards_script::minimize` (`ddmin` over any list, with a budget) and `shards_script::regress` (the shrunk rounds of a fuzz run as a
  scenario: header, `seed S`, `act?` and `step` lines). `crates/sandbox/scenarios/regress/` holds such scenarios; `scenario_files.rs` runs each on a fresh game of its seed, forward only.
- `shards fuzz --steer` (weights follow new coverage signatures) and `--minimize [--budget N]` (shrinks a session that halted, or whose chaos round trip did not come back, and writes `scenarios/regress/<date>-<stop>-seed<S>.scenario`); `shards_script::{stop, shrink}` and `shards::testing::fuzz_and_shrink` (each attempt on a fresh game, in heavy mode when the run had it). A regress file for a heavy stop says `# heavy: every=N span=K offset=O` in its header, and the runner plays it that way.
- The nightly fuzz campaign: `crates/sandbox/tests/fuzz_campaign.rs` (`#[ignore]`; `SHARDS_FUZZ_SEED`, `_RUNS`, `_INPUTS`, `_HEAVY`, `_CHAOS`, `_BUDGET`, `_SHRINK_SECS`, `_MINUTES`, `_OUT`, `_REGRESS`) and the job `fuzz` of `nightly.yml`: 50 runs of 2000 rounds with chaos, steering and heavy mode, shrunk scenarios, replays and a coverage report as the artifact `fuzz`.
- Voxel fields in the physics (`shards_motion`, the voxel design's PR A2a): `VoxelCollider` and `VoxelField` (static solid cells, one bit a
  cell, in world space) with a `ColliderId` from the same counter as colliders, in id order, replaced by a new value and removed with the
  component or the entity. Rays walk the cells (a ray along a seam between solid cells is inside the solid); casts test the cells under
  the swept box, merged into column runs (a field made `with_cast_merge(false)` tests them one by one, to the bit as a cuboid collider per
  cell; the mode is in the field's hash), and
  a merged box parry's cast misses but may meet is cast cell by cell; `overlaps` and `closer_than` test the cells. Fields come after
  colliders (strictly nearer replaces), the lower id first; on a tie a cell met as a box before one met only along a seam, then the
  lowest cell in `(x, y, z)` order. `PhysicsWorld::has_solid(mask)` (perception's gate); `len`, `is_empty` and `colliders()` count
  colliders only. The hooks
  `PhysicsWorld::extent_at`, `top_under` and `solid_span` answer what readers used to read off a collider's box, and every such reader
  uses them (the gate of `clearance` and `path_to`, `Terrain::blocker_top`, the nav bake's seed, `ground_below`);
  `PhysicsWorld::subset` keeps fields (the scatter's solids take the fields near a tile); the snapshot check `static_colliders` hashes
  every field. The engine rule `voxel.body_in_solid` (`BODY_IN_SOLID`), off by default. Engine rules off by default:
  `add_opt_in_engine_invariant(name, rule)` registers one only when `EngineConfig::opt_in_invariants` names it, and `shards::run` takes
  `--engine-invariant NAME` (repeatable; only the names offered; not with `--no-engine-invariants`). A world without fields runs the code it ran
  before: the reference replays verify byte for byte. The guard against a reader that forgets fields: `PhysicsWorld::collider`,
  `PhysicsWorld::by_id` and `ColliderInfo::bounds` are `disallowed-methods` (each call says why with `#[expect]` on its statement), and
  `crates/shards_motion/tests/readers.rs` checks every collider read in `crates/*/src` (the methods, `get::<Collider>`, `&Collider` in a
  query or an argument) against `tests/readers.allow`, a row for each occurrence. Tests: `tests/motion/seams.rs` (the seam differential
  test: one cuboid per cell, merged boxes, the field with the merge on and off), `tests/motion/voxel.rs`, the field-against-cells test
  and the timing `field_timing` (`#[ignore]`) in `physics/world.rs`.

### Changed

- The palette's part of a generated voxel world's hash is `VoxelPalette::semantics` (names, order, solid, liquid, foliage, tags), not
  `fingerprint` (which also covers colours): a recoloured palette is the same world. The golden world hash changed with it.
- `regen` of a dungeon says `N rooms` (it said `N tiles`), as does `region_load`; `goto_tile` outside a region says `chunks` or
  `tiles` by its layout.
- Breaking, once, for every later combat change: `DamageCtx` gained `credit`, `ignore_reason`, `dir`, `reaction`, `reaction_armor`, `knockback`, `taken`, `guard_mult` and `weight`; `Outcome` gained `PerfectEvaded` and `Parried` became `Parried { stamina }`; `Damaged`, `Blocked`, `Parried`, `Evaded`, `GuardBroken` and `Died` gained `weight`, `absorbed`, `source` and friends (`Damaged` also `ignore_reason`, so the journal can say why a hit was ignored: `god mode`, `grace`, `downed`, `ignored`); `Cost` gained `stamina_mode`. A struct literal of these needs the new fields. The behaviour of existing content is unchanged.
- `ColliderInfo::aabb_min` and `aabb_max` are `pub(crate)`: outside `shards_motion`, `ColliderInfo::bounds()` gives the box (and says,
  by its `clippy.toml` entry, that a voxel field has none).
- Breaking: `PhysicsWorld::subset` takes a second predicate, the fields to keep (`subset(|c| …, |f| …)`; `|_| false` keeps none, as
  before).
- The test `replays::a_different_world_diverges_and_the_report_says_when` (`shards_motion`) steers the second world's own hero: two
  worlds need not spawn the same entities while they load, and under a loaded `cargo test` it failed on an `Entity` of the first.
- The UI's journal lines (`ui: opened pause`, `ui: closed pause`, `ui: … pressed`, …) are `JournalKind::Local`, not `Event`: screens are outside the simulation and the snapshot, so a heavy twin never had them, and the first fuzz campaign stopped on `ui_close` for that. They are still in the journal and in `expect journal`.
- `shards_script`: a tool call's `Outcome` carries `halted` (why the game stopped itself during the call), so a driver other than the CLI sees a halted `step`.
- `soak.yml` runs every night (it ran on Mondays).
- `shards_cli`: the test `a_stale_port_file_is_neither_a_member_nor_a_reason_to_give_up` no longer uses a port that was bound and released (another test's stand could be given it): the stale file names the discard port.
- `--replay FILE --verify` prints a `warning: replay checks leave out …` line for each gameplay component the checks cannot hash (`ReplayHashing::reported_left_out`, now public), and the reference replay test fails on one.
- The host cleans a network client's input before it reaches `Intent`: a `move_dir` that is not finite becomes zero, one longer than 1 is cut to 1, a non-finite `aim` is dropped. An honest client sends clamped input, so replays do not change. Without it a bad frame would stop the host on `core.intent_sane`.
- A world that breaks an engine rule and the game's `health_in_range` at once now stops with the engine rule's name first (`stopped: invariant action.resources_in_range violated at tick 1 (+1 more)`); the sandbox keeps `health_in_range` as the example of a game rule.

## [0.4.0] — phases C–E: co-op, saves, UI and platform (unreleased)

Everything that landed in the main branch after phase B (`0.3.0`): co-op and saves (phase C), UI, localization and audio
(phase D), the platform — rendering, performance, Steam, builds for players (phase E), and the spec follow-ups after them (the
leftovers of E4, E5, E6, E9, E11). The section is prepared in advance, like `[0.3.0]`: the version in `Cargo.toml` is not raised, the tag is not set; at
release, set the date (see "Releasing a version (for the maintainer)").

### Added

**Groundwork for phase C (`ai_core`, `shards_core`, `shards`, `shards_world`, `shards_action`)** — no network and no saves; in
single-player nothing changes (the reference replays are unchanged):

- `ai_core`: application tokens (`process`: logs and crashes of several games in one process do not get mixed up), replication rules (`net_ignore`, `net_ignore_resource`, `net_predicted`, `replicated_components`, `rules_hash`), the state codec `state` (shared with snapshots; `apply_changes`), `sim::set_tick`, `snapshot::restore_releasing`, the event
  `WorldRebuilt`, `AiIndex::set_local_base`, `replay::set_recording` and `restart_from_current`,
  `journal::set_local_events` and `push_remote`, tool routes (`NetRoute`, `Forwarder`), binding an agent to a
  player (`ai/session {player}`, `possess player=N`), the deferred step (`StepRoute`, `step_poll`, `ai/step_poll`).
- `shards_core`: `SimSet` runs only where `simulates`, `NetSet::Predict`, `Predicted`, exit hooks
  (`add_exit_hook`), `SeedLock`, `EngineConfig` fields for the network, saves and settings.
- `shards::run`: the flags `--host`, `--join`, `--stand`, `--stand-udp`, `--name`, `--net-sim`, `--save-dir`,
  `--settings-dir`, `--no-settings`, `--character`, `--world`, `--exit-with-host` (so far only `--stand-udp` exits with code 2);
  `EngineConfig::embedded` and `testing::embedded_config`.
- `shards_world`: the starting region waits for `seed_ready` and takes `SimSeed`; a region change sends `WorldRebuilt`.
- `shards_action`: action and combat events (`ActionStarted`, `Telegraph`, `Damaged`, `Died`, …) are reflected (`Reflect`).
- `ai_core`: `add_optional_replay_check` — a replay check that goes into the hash, but only while there is something to compare it with.

**The network channel layer (E7.2, `shards_net`)** — no Bevy and no ECS; the co-op session is built on top of it:

- Transports (`Transport`, datagrams up to `MTU` bytes between `PeerId`s): `MemoryHub` — in memory, for CI and for a stand in one
  process, deterministic (one seed and the same sends give the same deliveries, no clocks); `UdpTransport` — direct UDP on a
  LAN. Junk from unknown UDP addresses cannot fill the table.
- Channel simulation over any transport: `LinkConditions` (`latency=150,jitter=10,loss=0.02,dup=0`) and `Conditioned`.
- `Link` per pair of peers: unreliable messages where "the last one wins", reliable ordered ones, blobs, deflate compression with a
  budget for unpacking, acknowledgements (only what was received is acknowledged), RTT, losses and rate over a window (`LinkStats`),
  timeouts; the host opens a channel to an unknown peer by `wire::peek_session`. The caller passes in the time (`now` in ms).

**The co-op session (E7, `shards_net`) and the "host + clients" stand (E11.6)** — `--host`, `--join`, `--stand N`:

- Connecting with a build check (protocol, engine, game, content, replication rules, tick rate), input with the tick
  number, state deltas for the area of interest, the world rebuilt after a snapshot, a region change or a desync, hashes every
  `desync_every` ticks, tools `net_status`, `net_sim`, `net_kick`, `net_resync`, `net_end`, conditions `net_desync`,
  `net_joined`, `net_left`, `net_disconnected`. The stand: host and clients in one process, each with its own BRP port, shared time.
- The return token is issued by the host (random, its own for each slot); the client keeps it in `<save-dir>/net/`, and a
  restarted client gets its slot and body back (checked over UDP). Two games with one `--name` in one save folder
  share the token: the second takes the slot, and the first gets `Bye` with a reason and a hint (`--name` or
  `--save-dir`); the character of a leaving player goes with that player's connection, not with the first one found with the same player number, and that player's
  leaving does not free the slot that the second game has already taken (while it waits for `join_policy`, the slot belongs to it).
- A guest acts only through their own body: the host runs only the forwarded tools (`NetRoute::Host`), no more than 16
  calls per tick and 8 agents per player; a foreign `subject` is refused.
- Everything a peer sends is limited: the name (1–32 characters), the token, the character (64 KB), input frames (`inputs::check`),
  desync reports (only for the hash that was sent; the world is rebuilt no more than once per 5 s); the message size is checked before
  parsing (before joining the game, 80 KB; for a player, 1 MB; an input packet, 512 KB; of the input packets in one tick, only the last one is parsed);
  unreliable packets from unknown peers are not parsed; the peer's text in the journal (the name, the `Bye` reason, a parse error with their
  words) is cut short and kept to one line; refusals are logged no more than one line per second; messages that the peer's channel did not accept are logged
  one line per second per player; a tick number at the edge of `u64` does not bring down the host.
- A value that cannot be read back (NaN, infinity) stays on the host, with one line in the journal, instead of going out as
  `null` and causing a desync loop; a reference to a removed entity is sent as it reads now; the encoding cache forgets removed entities; entities of the area of interest are sent even when the reference limit is hit, and such a
  view, while the limit holds, is not checked by hash (otherwise the world would be rebuilt every 5 s), and the host's journal names the player
  once.
- After a snapshot or a world rebuild, the host matches the players with the new world (a body goes to the one who has none; the body of a player who left
  waits for them; a player the host added itself through `player_join` stays, whatever number the guest held before it: the peers' bodies are marked
  `NetSeat`, and the guests' bodies from the snapshot of another host process go away, freeing their numbers). A `Rebase`
  that the channel does not accept right now (by the number of messages or by bytes) waits without shifting the epoch and without tearing down the world every tick.
  While the host's ticks run in a row (`net_resync`, a desync, a region change), input in transit survives the world being rebuilt, the client's clock
  does not go back, and whatever the client holds that the host dropped is sent again; a snapshot that moves the tick
  starts the input over (including when its world is waiting for room in the channel), and desync reports are counted from its tick. After a respawn, the body gets its player's input;
  `stop` on the client stops the body on the host, and the client's queue of orders is not rolled back until the host applies it.
- `net_end` and the host's exit end the session once (the `on_session_end` hook too): players leave the host's world (the body
  waits, the character leaves once, with the end of the session), and anyone still joining is refused with `the session has ended`. A client whose session has ended
  stops sending and acknowledging after 3 s of silence from the host, and the host does not create a channel for a peer from a datagram that has no messages.
  `--exit-with-host`: 0 — the host ended the session; 1 — a refusal, a kick, the host was lost, or the world was not accepted (client); 2 — the session could not be opened
  (on the client, the address; with `--host`, a busy port). On exit, the stand waits until the clients get their characters and the end of the session, and runs the exit hooks
  of each member on its behalf (their lines go into that member's journal); the stand ends only with the host; the `pause`, `resume` and `speed`
  of the stand are visible in the members' `ai/status`; `speed` must not be below 0; infinity (`x=1e39`) is refused, not a panic of the stand;
  the reply names the new speed; `--host 0` names the busy port (`ai/status` → `net.udp`, `shards wait`).
- Over UDP, `pause`, `resume` and `speed` on a client are `HostOnly` (`the host owns time; call it on the host`): the route
  is set when the client opens a UDP socket, not by the `embedded`/`testing` flags, so a UDP test checks it.
  A stand's client keeps them: the stand's time is shared.
- The stand reads and writes the host player's settings (`--settings-dir`, `--no-settings`; `shards_settings::install` after
  `build`, as in a single-player game); stand clients do not store settings.

**A stand for the agent (E11.6, E11.7): `--peer`, stand scenarios, `--stand-udp`**:

- `shards`: `--peer host|c1|c2` (`SHARDS_PEER`) reads a member's port from `F`, `F.c1`, `F.c2`; `wait --peer all` waits for all of them, prints
  a line per member (`member=c1 role=client player=1 body=e15 port=… pid=… seed=… tick=…`) and checks that it is one session
  (the host lists the players and the clients' bodies; it decides from fresh statuses, so it also works for a stand over UDP); `shards stand`
  (members: role, player, body, port, ping, desyncs) and `shards stand stop` (`net_end`, SIGTERM, waits until nobody
  answers; a client that left is named, exit code 1); `step`, `pause`, `resume` and `speed` of the stand (`{"pending": id}`)
  are polled through `ai/step_poll` every 20 ms (`--timeout`, global, 60 s by default; it replaced the `--timeout` of
  `wait` and `shot`); `session --player N`; each member has its own token on any path, and single commands under `--peer cN` do too (`--token` is
  the host's; a client's is `SHARDS_TOKEN_C1`…, otherwise its `.shards/<port>.token`, and `--token` only as the last resort; the `unauthorized` reply
  names the variable). `wait` (any, and `wait --peer all`) stops at once when waiting is pointless: a member was there and has gone (the host ended the stand),
  the host's port file was read and then disappeared, `unauthorized` twice in a row, the command line
  does not name the game (`--peer c1 wait` without a port file), the same mismatch twice in a row (not the host, a foreign seed, two
  clients with one player; `Mismatch::Wrong` against `Mismatch::Forming`); the number of clients for `wait`, `stand` and a scenario is taken from
  the host's `ai/status.net.stand.clients`; a port file left by an earlier stand does not count as a member.
- `shards_script`: the header `stand clients=N [latency=MS] [jitter=MS] [loss=P] [dup=P]`, the prefix `@host|@c1|@c2`, results,
  steps, the agent and the journal marks for each member separately; an invariant without a member runs on each one; `Driver` got
  `peers`, `call_on`, `session_on`, `status_on`, `journal_since_on`, `check_stand` (by default, one game).
- Stand scenarios are in `crates/sandbox/scenarios/stand/` (`coop_smoke.scenario`): in tests they run on a stand built from
  the header (`shards::testing::run_stand_scenario`, `run_stand_str`, `run_stand_script`, `StandDriver`), and run twice they give
  the same journals; against a live stand, `shards scenario`; CI runs them with `--stand 2`. A single game rejects a file with `stand`
  and gives a hint on how to start a stand. `ai_core::scenario` provides `configure_agent`, `world_status`,
  `world_journal_since`.
- `--stand N --stand-udp`: the host over UDP and the clients as child processes (`--join`, its own `--port-file F.cK`, `--exit-with-host`), only
  in real time (`--paused` — exit code 2 with a reason); a client that did not start, did not join the session (the host did not let it in) or
  crashed after joining stops the stand with its exit code, and a client that left on its own (code 0, 129/130/143 and SIGHUP/SIGINT/SIGTERM) or
  an admitted client whose connection the host itself closed (`net_kick`, a silence timeout; the slot remembers who closed the link,
  `NetHost::link_closed_by_host`), goes into the host's journal (a kick, or silence during joining, does not count for this: such a client did not
  get in); the client's own goodbye, a refusal, and being pushed out by a second game with the same
  token do not mean this, and a client that did not accept the host's world and left with code 1 stops the stand; "got in" means
  "admitted" (`Slot::admitted`), not "the slot has a name", so a client that the host refused after `Hello` also
  stops the stand; the exit hooks run in the same system that stopped the stand, and a stand
  that ended before the children started (a signal in the frame where loading ends) does not start them; the children start after the content is loaded and
  after checking that `--stand N` fits in `max_players` (no process starts on a refusal); the host removes the port files
  of an earlier stand before it starts the children; `wait` on a client's port file (`--port-file F.c1`) does not end when that file disappears;
  `ai/status.net.stand = {clients, udp}`.
- The port files `F.c1` and `F.c2` are written through their own temporary files (`F.c1.part`): two clients that start together do not share one.
- The scenario names a member that the driver lost (`c2: cannot reach the game at …`), both at start and when checking an invariant (with its line); the verdict on the stand's links (``this scenario's `stand` line wants links …``) stays in the scenario's own words. The stand scenario does not depend on `--peer`/`SHARDS_PEER` (including `all`).

**Prediction, smoothing and the network overlay (C4: E7.4, E7.5, E7.8; `shards_net`, `shards_core`, `shards_motion`,
`shards_action`)** — the client does not wait for the host to move its own body; in single-player and on the host nothing changes (the reference
replays are unchanged):

- The client moves its own body and starts its actions on the same tick: the sets `NetSet::PredictInput` → `NetSet::Predict` →
  `NetSet::PredictEnd` (only the body with `Predicted`) run the same systems for orders, movement and actions as the host, with the
  presses sent for that tick. Only components with `net_predicted` change; the desync hash does not compare them.
- Orders drive the predicted body as on the host, and whatever they wrote is rolled back after the tick. A passed path point
  stays passed: the path and the `move_to` queue run without a stop at each point. An order finished on the host reaches the
  client through the host's queue; for an attack, with its `aim` and `target`. After the world is rebuilt, orders the host finished or dropped
  count as finished for prediction too.
- The combat tools on the client, `attack` included, reply based on the action shown (`PredictedAction`): `ok: roll (E) ·
  predicted, host confirms` or an immediate refusal. An unconfirmed action gets the line `net: predicted … not confirmed (…)`.
- Correction: the error in feet position against the host's state at its tick, minus the corrections made since then; under 2 cm, nothing is corrected; otherwise,
  each tick, a share `correction_rate` of the remaining error is corrected (per tick, not per received state). A snap to the host's movement state happens when the error is larger than `snap_distance`, in a movement mode that diverges for 6 ticks, on death, and at the first comparison
  after the world is rebuilt or a clock jump, if the body moved (`snap: world taken`, `snap: clock skipped`). After a snap,
  the body is left alone until the state taken catches up with the host's marks: a landing does not cause a second snap.
- Other entities in the window are shown between two host states, slightly in the past (`interp`); the game's `Transform` stays at the newest host state. A new entity is placed at once, instead of flying in from the origin.
- `net_overlay` (F6): channels, desyncs, prediction error, snaps, unconfirmed actions; in `net_status`, the field
  `prediction`. The error figures start over when the world is rebuilt.
- Checked on a stand with 150 ms one way and 2 % loss (`crates/sandbox/tests/sandbox/net_predict.rs`, 14 tests):
  running, a pillar, a path of points and a move toward an attack's target are predicted without error; a world rebuild on the move gives one snap and zero error
  after it.

**Saves (E8.1–5, `shards_save`)** — the character and the world, slots, and sections instead of a world dump:

- Sections (`SaveSection`, `save_component`, `save_resource`, `add_save_migration`), the registry (`save_registry`), the envelope
  `shards-save/1` with a checksum, an atomic write with three copies and a restore from a copy, with an entry in the journal,
  `save_now`, `autosave_on` with a debounce taken from `*.save.ron`, tools `save_game`, `load_game`, `save_check`, `save_list`,
  `save_damage`; the engine sections `region`, `position` and `vitals` are registered by `shards`.
- `load_game` restarts recording from the loaded state (`replay::restart_from_current`): a replay after loading
  passes `--verify`.
- The save folder and the current slots are taken from `EngineConfig` (`--save-dir` only inside the game's folder, `--world`,
  `--character`); a single-player game loads the named slots before the first tick (if there is no slot, a new game starts); the host and a client
  go through `load_launch_slot`.
- A replay in playback writes nothing to disk: `save_now`, `save_game`, the repair of the main file, `save_damage`,
  autosaves (`save: not written during replay playback`).
- Pending autosaves are written on exit (exit hook, `flush_saves`).
- A save from a newer version (a section above the build's version, envelope `shards-save/2`) is not treated as damage: it is refused with
  a clear error, without falling back to the copies, without `.damaged` and without overwriting; a write into a slot that has a newer copy is refused,
  so rotation does not lose the new file (`Fault::Newer`).
- `shards_action::revive` (used by the dev-`heal` and by loading `vitals`): loaded health revives a body that died after the
  save; a save made at zero health leaves it dead.
- The reference replay `tests/replays/saves.json` and the extended scenario `saves.scenario`.

**Invariants, divergence details of the replay, tick time (E11, tasks 9–11; larelgit/airustcargo#4, merged into this branch)**:

- Invariants (`ai_core::invariants`): game rules `add_ai_invariant(name, |w| -> Option<String>)` and session rules
  (`assert_add`, `assert_list`, `assert_remove`, `assert_reset`; CLI `shards assert add|list|remove|reset`). They are checked
  after each tick, wherever the simulation runs; a violation stops the game, as a crashed system does:
  `stopped: invariant NAME violated at tick N (replay: replays/invariant-NAME-tickN.json)`, in real time it pauses.
  A deferred step (`step_poll`) stops in the same way. The rule conditions use the `--until` syntax plus `warning` and `error`.
  A rule that has fired stays silent until `assert_reset`; rules and their firings are not part of the hash or the snapshot; loading a
  snapshot re-arms them and keeps the session rules. All four `assert_*` tools refuse on a client
  (`NetRoute::HostOnly`): the rules are checked on the host; ask the host.
- `shards fuzz --seed S --steps N`: random player inputs, from a seed, until a rule is broken; the session's replay
  plays back the same way. `shards` exits with code 3 when the game has stopped (a crashed system or a broken invariant);
  a scenario fails on a broken invariant, as on a crashed system (`act? step …` allows it).
- The sandbox: the rules `health_in_range` (health in `0..=max`) and `bodies_in_world` (bodies inside the world: corridor, obstacle course,
  region; the bounds are in `world/world.bounds.ron`). The scenarios and the reference replays do not break them.
- Divergence details: replay checks keep the parts of the hash (`Check.parts`: a hash over entities or over components, and
  the values of `add_replay_check`), and `--verify`, the report and the journal name the entities and components of the first divergent
  check, and below them, their values in the live game (`e6 now: Health { current: 390.0, max: 600.0 }`). The game flags
  `--check-every N` and `--check-detail components|entities|none`; both are written to the file. Parts are also present in replays made from a
  snapshot; `add_optional_replay_check`, while there is nothing to compare, gives no parts.
- Tick time (`ai_core::perf`): the line `ticks: avg …, max … (tick N), budget …` under the answer to `step`, a `[perf]` entry in the journal about ticks longer than the budget (not more than once per game second), the `perf` field in `ai/status`; the budget is the flag `--tick-budget-ms` (by default, the tick's length). `EngineConfig::report_perf` (`shards::testing::config()` sets it to `false`) turns off the output for tests that compare answers in full.
- One replay format: `shards-replay/3`, with the optional `check_every`, `check_detail`, `parts` and `assert` events;
  `/2` files play back; the state hash is unchanged. The reference replays of the sandbox and the template are recorded again, with the checks split by component: the sandbox has a new content file, `world.bounds.ron`, which changes the content hash (with the previous content, old files pass `--verify`); the template's old files pass anyway.
- `shards_script`: the conditions `warning` and `error`.

**Player settings (E8.6, `shards_settings`)** — graphics, audio, controls and accessibility on Bevy's own settings framework
Bevy (`bevy::settings`, feature `bevy_settings`; only `bevy-settings 0.20.0` is added to `Cargo.lock`):

- Groups `GraphicsSettings`, `AudioSettings`, `ControlSettings`, `AccessibilitySettings` (resources, `settings.toml` tables);
  the game's own groups are added with `app.add_settings_group::<G>()`. Registration rejects a group that TOML cannot hold (`u64`, a map key that is not a string), and
  a group with `#[settings_group(file = "...")]` (the engine watches only `settings.toml`).
- Storage: `shards::run` starts it after the game's `build` (`shards_settings::install`): the platform folder under `SettingsAppId`
  (the game sets the name, otherwise `shards-<title>`), `--settings-dir DIR`, `--no-settings`. A window without flags reads and writes;
  `--headless` without `--settings-dir`, tests, `--verify` and built-in games have no storage. A write comes one second after the last change
  (including changes made by the game's code) and on exit. With no change, nothing is written. `--settings-dir` on a platform without a settings folder
  is an error with advice (it used to say "kept in DIR/settings.toml", while the storage wrote nothing).
- The settings file is never lost. If it is not TOML, a copy goes to `settings.toml.bad` (names already taken are not overwritten, but the same text is not copied twice), and the start uses the last good copy `settings.toml.bak` or the defaults. An empty file (with no
  text) while a copy exists is a failed write: the copy comes back. If it does not come back, the journal names both files. A file of
  only comments is a request for the defaults, and it stays as it is. A deleted file is a reset (the old copy goes with it). A link to a
  regular file is read the way Bevy reads it. Writes after which the file is empty or unreadable (the disk is full) return the copy and retry up to three times, with a 3-second pause. The journal names the reason. After `giving up`, the changes stay in memory, and exit writes them again, whatever Bevy reports as changed. The last look at the file on exit writes nothing after the copy has come back, so on a full disk the copy stays, not an empty file. A manual edit of the file in a running game is not taken for a failed write: a readable file only updates the copy; an unreadable one goes to `settings.toml.bad` (the last good copy takes its place); an empty or deleted file is left alone. `controls` is read entry by entry; `nan`/`inf` in a number means the default.
- The last good copy `settings.toml.bak` is replaced only when the new copy is written in full (next to it, in `settings.toml.bak.new`, then `rename`). On a full disk the copy is not made, and the old one stays, not an empty `.bak`.
- `reset=true` returns the game's defaults (the group's resource before the file), not the type's `Default`, and checks the key the way `set` does. A group with two bad values from the file does not block a third `settings_set` (only the change is judged). Checks of a group that need world access: `add_settings_world_check`.
- The tools `settings {group?}` (Player, query) and `settings_set group= key= value= | reset=true` (Player, control: not recorded in the replay). The value is read by the field's type. A refusal names what is allowed. A change gives a journal line, such as `settings: accessibility.camera_shake 1 → 0.3`. `ai/status.settings`.
- Applied to: the main window (`apply_graphics`: fullscreen, vsync, size, scale); the camera (field of view, shake strength, sensitivity, Y inversion, `MotionBlur` on cameras that have it: only the strength changes, the game's sample count stays); the flicker of telegraphs and off-screen markers (`accessibility.flashes`, `draw::pulse_at`); `hold_to_toggle` in the input map. Volumes are stored for audio (E9).
- Settings are not part of snapshots, replays, their hash or replication (`net_ignore_resource`); the reference replays stay as they were.
- Sandbox: `SettingsAppId` `shards-sandbox`, `InputSettings::for_game("shards")`, scenario `settings.scenario`; template:
  `SettingsAppId::new("my_game")` (folder `my-game`; the name is written as in the package, so that search finds it on a rename, and so that games made from the template do not end up sharing one settings folder), `InputSettings::for_game("my_game")`.
- Tracy profiler: the `trace_tracy` feature of `shards`, `sandbox` and the template turns on the Bevy feature of the same name, and each system is a separate zone (`cargo run -p sandbox --release --features trace_tracy`, README, "Tick time (E11)").

**Saves in co-op (E8, `shards::coop`, `CoopPlugin` in `EnginePlugins`)** — the character travels with the player, the world is kept by the host; `shards_net` and `shards_save` still do not know about each other:

- The client reads the character slot (`--character`; without the flag, the slot named by `--name`; for `--stand`, `c1`…`cN`) and sends it in `Hello`. The host applies the character inside the recorded `player_join`; the host's replay builds the same body. Old section versions are migrated. A damaged file is restored from the copy; an unreadable one starts the character with defaults, and the journal gets a line. What is sent is checked: foreign sections are skipped; a number that a body cannot have (a ceiling below zero, infinity) is rejected before any change; a panic in a game section is caught; a failed join removes its own body.
- `save_now`, `autosave_on` and `save_game` on a remote player's body capture the character on the host and send `CharacterSave` to the client (they do not write to the host's slot). The client writes its own slot with the normal storage. A save inside a guest's forwarded call goes into the session's queue. The host's own save is sent to the guests even when its file was not written.
- The host remembers each player's character for the session (`SessionCharacters`): a player who comes back with a file the host knows gets the newest copy the host has (the body waits with its state, and after the deadline it is built from the remembered copy, and the player's file catches up). With a file the host does not know, the player gets their own. A token whose slot was taken by another player is kept by the host among the departed and returned to its owner.
- End of session (`net_end`, the host exits): the world and the host's character are saved (not over slots that have not finished loading); then the clients get their characters and `SessionEnd` with the world's slot. Nothing is written or sent twice. The host loads `--world` and `--character` before the first tick (`load_game` of the agent `launch`). The client neither writes nor loads the world (`the host keeps the world; ask the host (ADDR)`); `load_game` on a client is `HostOnly`; `load_game` of the world on the host sends the clients the world again (`WorldRebuilt`). On exit, a UDP client waits up to 300 ms for the last `CharacterSave`.
- `shards::coop::{CoopPlugin, SessionCharacters, character_value, parse_character}`; `shards_save`: `SaveRouting`, `SaveReport::target`, `slot_from_name`; `shards_net`: `NetHost::send_character`, the hook `join_character`. Apps from `shards::testing` get their own save folder, which is removed with the app.

**Phase C acceptance (C9): a long game, the region edge, saves and co-op in the template**:

- `shards soak` and `shards_script::soak` (`SoakPlan`, `SoakReport`, `run`, `SOAK_AGENT`): seeded agents play for each participant of the stand (or for one game): each round, `observe` and a random player input. A dead body is healed by the host. On a schedule the host changes the region (`--regen`, `--regen-minutes`) and saves everything (`--save-minutes`), exactly on time: a step does not jump past it. The stand steps with
  `--until named:net_desync`. The first desync, a crashed system or a broken invariant gives exit code 3. At the end, each participant's `net_status` is checked: a participant that does not answer it fails the run (`desyncs ?`). A plan that ends after the last tick the game can count is rejected before the first call. It runs through `Driver`: against a live stand and in `cargo test` (`shards::testing::StandDriver`).
  The `fuzz` and `soak` inputs are in `shards_script::inputs` (they used to be in the CLI).
- Acceptance for E7: half an hour of game time, a host and two clients, 150 ms one way and 2% loss, five region changes and nine saves, and not one desync (`shards soak --minutes 30` on a live `--stand 2`, the same run over UDP with client processes, the test `half_an_hour_of_three_players_on_lossy_links_stays_in_sync` under `#[ignore]`). Two minutes of the same run is the test `two_minutes_of_three_players_on_lossy_links_stay_in_sync`; three minutes is the `headless-cli` CI step; half an hour is `soak.yml`, every night.
- Region edge: `edge_wall: H` on `region` puts an invisible wall H m high around the region (`region::edge_wall`, `edge_wall_boxes`, `EDGE_WALL_THICKNESS`, `EDGE_WALL_DEPTH`, the `RegionEdge` marker, which the template does not draw). `region_info` shows an `edge` line. The wall does not change tiles or seed points. Its colliders come after the tiles.
- The client's `net_status` reports the number of its own desyncs (`desyncs`).
- Template: saves (`src/progress.rs`: the character section `trophies`, the world section `fallen`, an autosave of the cleared yard; `SaveKey` for the arena's characters by the `key` from content (`yard.gate_grunt`), and a repeated key is a content error; `scenarios/saves.scenario`) and co-op for two (`content/session.net.ron`,
  `scenarios/stand/coop.scenario`, `tests/game/coop.rs`; the stand-scenario test `every_stand_scenario_passes`). The template's `saves/` folder is in `.gitignore`.
- CI: `soak.yml` (half an hour on a live stand: every night, by hand, and on a push with `[soak]`) and `record-replays.yml`
  (the sandbox's and the template's reference replays, re-recorded on Linux, as an artifact: on a push with `[record replays]`).

**UI (D1, E9.1–3, `shards_ui`)** — HUD and menus from content, and an interface tree for the agent:

- Content kinds `screen` (`*.screen.ron`: layer `Hud`/`Menu`/`Modal`, `pauses`, `open_at_start`, `closable`, title, position, layout, widgets) and `theme` (colours, font sizes, margins; id `default`). Widgets: `Panel`, `List`, `Grid`, `Tabs`, `Text`, `Hint`, `Bar`, `Cooldown`, `Compass`, `Allies`, `Compare`, `Button`, `Slider`, `Toggle`, `Choice`.
  Content checks: a repeated id, `open` of a screen that does not exist, a tab page without an id, a slider range, a `setting` that is not in the form `group.key`, a HUD that pauses.
- Screen stack and focus (`UiStack`, `model::navigate`): `up`/`down`/`next`/`prev` wrap around at the edge; rows and grids move left and right; `left`/`right` change a slider, a toggle, a choice and tabs; `confirm`, `back`; a modal closes everything beneath it. A menu with `pauses` sets `PauseState::Paused` in single-player, and clears the pause only if it set it itself.
- Buttons: `close`, `back`, `open <screen>`, `quit`, `tool <name> {json}` (on behalf of the player who pressed it, with their scope), a game action (`UiAppExt::add_ui_action`). Values come from sources (`add_ui_source`; the engine gives `you.health`, `you.stamina`, `you.poise`, `you.charges:<name>`, `you.cooldown:<action>`, `you.heading`, `allies`; `shards_ui::viewer` says whose they are), or from the player's settings (`setting`: changed through `settings_set`). Events: `UiPressed`, `UiChanged`, `UiScreenChanged`.
- Tools `ui_read`, `ui_open`, `ui_close`, `ui_navigate`, `ui_click`, `ui_set` (scope Player); a `UI` section in `observe` while a menu is open; the journal line `ui: …`; a `ui` field in `ai/status`. Screens outside the simulation (no `AiId`, not in the snapshot or the hash). The tools are of kind `action`: the replay presses the same buttons.
- Drawing (feature `render`, window only): Bevy UI nodes, the theme, text size from `accessibility.text_scale`, the mouse through picking, roles, labels and AccessKit values for screen readers, and `InputFocus` follows the focus.
- `shards`: `EngineUiPlugin` — the sources `you.*` and `allies`; the `pause` key opens the `pause` screen; the `menu` input context (`navigate` with repeat, `confirm`, `back`) calls `ui_*` as the agent `input`.
- Sandbox: the `pause` menu, `settings` (all settings groups on tabs), `confirm_quit` (`content/ui/menus.screen.ron`), scenario `ui.scenario`. Template: HUD (health, stamina and trophies: the game supplies them), pause, settings, and "save and quit" (`src/screens.rs`, `screens.screen.ron`), scenario `menus.scenario`.

**Localization (D2, E9.4, `shards_locale`)** — texts in Fluent, with the language changed on the fly:

- Content kind `locale` (`*.locale.ron`: id is the language tag, `name` in the language itself, `fallback`), texts are in
  `assets/locale/<language>/*.ftl`. The content check parses the files (an error names the file, line and column), finds a message that is in two files of one language, a `fallback` cycle, and a language with no files. The third-party crates `fluent-bundle =0.16.0`,
  `fluent-syntax =0.12.0`, `unic-langid =0.9.6` are used only inside `shards_locale`.
- Messages are whole phrases with parameters: a number picks the language's plural form; a message parameter (`Arg::Message`) inserts a whole phrase (`Locale::text`, `shards::locale::text`, `Args`). What is missing is taken from the fallback language; otherwise the id is shown. Each such case, and each forgotten parameter, is named once in the journal (`locale: ru has no message …`).
- The player setting `locale.language` (group `locale`): empty means the game's language; an unknown language is refused by `settings_set`, with a list. The change applies before the call returns (journal `locale: language en → ru`, event `TextChanged`); the loading gate waits for the languages. Hot reload of `.ftl` (`locale: text reloaded (…)`) and `reload_text`. An edit with an error is refused the same way as content: the file on disk stays broken, and until it is fixed the text loaded earlier is in effect; the error with its place goes to the journal once.
- Tools `locale` (languages, which one is being played, and what each one lacks; `language=` — all messages of the language) and `text` (a message with parameters in any language), both query, scope Player; a `locale` field in `ai/status`.
- `shards_ui`: the text `@id` in titles, labels, texts, hints and `Compare` rows is a message in the game's language. When the language changes, open screens rewrite their labels, keeping values and focus. `Choice.labels` are the labels of the options (`ui_set` accepts a label too). A choice whose setting matches none of the options shows `value`. `ThemeDef.font` is the font file for all text (the content check looks for it). The screen check finds an `@id` that is not in any language.
- `shards_settings`: the event `SettingsChanged { group }` — the group changed (by a tool, by a screen or by the game's code), before `settings_set` returns.
- Sandbox: languages `en` and `ru` (`content/ui/languages.locale.ron`, `assets/locale/*/menus.ftl`), the menus take their text from `@id`, with a `language` tab,
  a theme with the Fira Sans font (OFL), and scenario `locale.scenario`. Template: the same for the HUD and the menus (`assets/locale/*/game.ftl`),
  HUD trophies: a message with a plural form (`1 trophy`, `5 трофеев`), and a language tab in the settings.

**Audio (D3, E9.5, `shards_audio`)** — buses, voices, spatial sounds and music in layers; decisions run on the simulation clock:

- Content kinds `sound` (WAV or Ogg Vorbis files, played in turn; bus, volume, priority, `spatial: (near, far)`,
  `max_instances`), `music` (stems of equal length, and layers from calm to tense; each layer after the first has a `when` condition
  by name; `!` negates it; `fade_in`, `fade_out`, `hold`), and `audio` (the mixer, one per game: the number of voices, the track from the start of the
  game, the engine's event sounds). The content check finds a file that is missing or cannot play, stems of different lengths, a layer
  with no stems, a `when` on the first layer or its absence on the next one, an event that does not exist, a second mixer, and a telegraph whose sound
  is not defined.
- Buses `music`, `sfx`, `ui`, `voice` take the player's volume (`audio.<bus>` × `audio.master`), which applies to whatever is playing.
  Voices: at most the mixer's `voices`. A stronger sound (priority, then volume, then age) takes a weaker one's voice; an equal sound is not heard;
  `max_instances` takes the oldest of its own. Spatial sound fades from `near` to `far` (`(1 − t)²`) and does not start beyond `far`. The listener is the HUD body (`shards_ui::viewer`), turned like an `AudioListener` (the umbrella crate attaches it to
  `ThirdPersonCamera`).
- Sounds that play: attack telegraphs (`SoundCue`, where the blow will land), animation keys with a sound name (`AnimationCue`), engine events through the mixer (`damaged`, `blocked`, `parried`, `evaded`, `guard_broken`, `staggered`, `died`, `healed`, `ui_moved`,
  `ui_pressed`, `ui_changed`, `ui_opened`, `ui_closed`), and game sounds (`PlaySound`, `shards::audio::play`).
- Music: the stems of a track play together in a loop; the layer is chosen by conditions (`add_ai_condition`); the tense layer comes in at once, the calm one only after `hold`; stems fade in and out; a track change fades out the old one. `Music` is part of the snapshot.
- Tools `audio` (query, Player: track, layer, stems, buses, voices, sounds not heard, listener), `sound_play` and
  `music_set` (Dev, control); a `MUSIC` line in `observe`, an `audio` field in `ai/status`.
- Playback (feature `audio` of the umbrella crate, `shards_audio/playback`): voices are synced, stems start together, effect sounds freeze during pause, and the menu plays in real time. Without a window or an output device, the mixer decides the same way.
- `shards_npc`: the conditions `npc_engaged` (an active NPC whose target is in a state like `chase`, `attack`, `wait_token`) and `npc_near` (a live player closer than one and a half times the active NPC's sight range). `shards_ui`: the event `UiFocusMoved`. `shards_settings`:
  the volume `audio.ui`.
- Sandbox: sounds, the arena's music (`explore`, `tension`, `combat`) and the mixer in `content/audio/`, the "Interface" slider in the sound tab, scenario `audio.scenario`; `cargo run -p sandbox --example make_sounds` synthesises the files. Template: the same for the yard, the game's music conditions `foe_near` and `in_fight` (the grunt uses the game's brain), the "Effects" and "Interface" sliders, tests
  `tests/game/audio.rs`: the music switches to `combat` when the hero comes near the grunt.

**Developer menu and console, NPC overlay, navmesh routes, sockets in level files (D4: E11.1, E11.2, E11.5, E4.4)**:

- New crate `shards_dev` (module `dev`, last in `EnginePlugins`). **Menu** (F10 in the window; tool `dev_menu`): the entries of the engine modules by section: time (pause, step, slow motion x0.25, normal speed, snapshots), world (colliders, region and
  seed, "Check this seed" — `worldgen_report` on the region's seed, "Generate 200 regions like this one", `goto_tile`, `regen`,
  world overlay), combat (invulnerability, healing, hitboxes), AI (NPC minds, navmesh, `npc_state`, `nav_info`), network (overlay, status,
  simulated 150 ms and 2%, no simulation), saves (`save_check`, `save_list`); then the game's own entries. A toggle shows
  the line of its other state. In the window, an entry with blanks to fill (`goto_tile x= y=`) writes its line into the console. An entry with nothing to act on right now says why. `dev_menu run=<id>` runs an entry; `open=true|false` shows the panel in the window.
  **Console** (`` ` `` in the window; tool `console`): a line `tool key=value …`, as in `shards act`, through `call_tool` as the agent `console` (scope Dev); the reply is the result and the call's journal; `help`, `clear`; in the window: history (arrow keys), Tab completes tool and argument names from the schemas, the menu scrolls with the wheel and the arrows, and the mouse works. While the panel is open, the `dev` input context is on top (`shards::dev::has_keyboard`), and the overlay keys (F2, F6, F7) wait until it closes (`ai_core::dev::DevKeyboard`, condition `keys_free`); the keys are the `DevKeys` resource.
- `ai_core::dev`: `DevMenu`, `DevEntry`, `DevCommand` (`Run`, `Fill`, `Live`), `AiAppExt::add_dev_entry`, `toggle_line`,
  `parse_line`, `run_line`; `session::caller` is now public. `shards_script` exports `split_words`.
- Content kind `devmenu` (`content/**/*.devmenu.ron`: `group`, `label`, `run` or `fill`; the line is checked on load);
  a game entry with the id of an engine entry replaces it.
- NPC overlay (`shards_npc::overlay`, `NpcOverlay`, F7; tool `npc_overlay`, Dev query): a line on each NPC closer than 40 m to the player — its state and target, token or role, leash, who it sees and hears, where it lost its target, noise, route; in the window, gizmos:
  a line above the NPC, a line to the target (red, with a token), the sight cone and proximity circle, crosses marking lost targets, noise, and the leash circle.
- `path_to` on the navmesh: asks `RoutePlanner` the way `move_to path=true` does, and names its provider (`"provider": "navmesh"` /
  `"local grid"`); inside a region the length is not limited unless `max` is set. `clearance`: a direction where the navmesh ends
  more than a metre before physics does — `closed` (`E closed 4.0m (the navmesh goes no further)`); `NavRegion::reach`.
- The content check compares the socket markers in a tile's level file with its `sockets`: the edge, the kind, the height, and the size of each region the tile takes. `ContentKinds::add_check` and `ContentAppExt::add_content_check::<T>` add a check to a kind, from another module; `TileLevelsPlugin` adds this one.
- Sandbox: the `sandbox` section of the menu (`content/dev/sandbox.devmenu.ron`: to the start of the arena and the obstacle course, the guard's second phase from a snapshot, the wilds with a seed of your choice), scenario `dev_menu.scenario`; `snapshot.scenario` loads the second phase through a menu entry;
  F7, F10 and `` ` `` are shown in the key hints; the game's own F-keys stay quiet while the console or the menu is open. Template: the README covers the menu.

**Builds for players and Windows (E10.6; `shards_cli`, `shards`, `shards_core`, `ai_core`):**

- `shards package` builds the game for players and checks the build. Profile `dist` (or `release`), game feature `dist` instead of
  the default features. On Windows (MSVC) the C runtime is linked in statically (`+crt-static`, a build with `--target`), and `assets/icon.png`
  becomes the program's icon: `shards` writes the `.res` resource itself. The program and `assets/` go into
  `dist/<name>-<version>-<OS>-<arch>/`. Checks: the PE imports are system-only (no `vcruntime*` or `msvcp*`), and a copy of the folder in
  a temporary folder with a bare environment passes `--check-content` and `--smoke` and changes nothing in its own folder (it writes and deletes nothing). Exit code 0, 1 (the check
  failed) or 2 (the build failed).
- `--smoke TICKS` in `shards::run`: the game loads without a window, runs the ticks and replies `ok: loaded in …s, N ticks in …s`
  (exit code 0), or names what got in the way (exit code 1). It writes nothing.
- The `dist` feature of `shards` turns off the developer keys (`` ` ``, F10, F2, F6, F7). The window icon comes from
  `<assets>/icon.png`, if the file exists (`shards::window_icon`, the `winit =0.30.13` dependency, the same one Bevy uses). A game with
  `windows_subsystem = "windows"` writes to the terminal it was started from.
- The `quit` tool (Dev, control) and `shards quit` stop the game without signals on any OS. 0.2 s after the reply, the game
  exits through `AppExit`: exit hooks run, and the token and `--port-file` are removed. `shards quit` waits until the game
  stops answering (an error or a cut-off reply on exit is not proof, so it asks again), and until its process ends
  (`kill(pid, 0)` on Unix, `tasklist` on Windows). No request waits longer than `--timeout`, even if the game holds the
  connection and stays silent. A stand client in one process refuses.
- On Windows, closing the console window, signing out and shutting down end the game the same way as SIGHUP. The `--stand-udp`
  clients live in their own process group, and the host stops them with `CTRL_BREAK`. A host without a console, which `CTRL_BREAK`
  does not reach, ends the client by force and writes a `warn`. `ai_core::paths::{canonical, without_verbatim}` give paths without `\\?\`:
  the path is built from parts, so a name with an unpaired UTF-16 surrogate does not change, and a name that would be read
  differently without the prefix (`/`, `.`, `..`, a dot or a space at the end, a device name such as `NUL` or `con.txt`) keeps the path as it is.
- `shards package` refuses if `assets/` holds a link (symlink), or if `assets/` itself is a link: the package would take in whatever the
  link points to. When a copy check runs past its time limit, it also ends the processes the program started (`taskkill
  /T`, a process group on Unix), and their output waits no longer than 5 s. The `dist` profile is also recognised by a header that has a comment
  or spaces in it (`[ profile."dist" ]  # …`). `--smoke` with a tick count that does not fit the counter replies with an error (exit code 1).
- Template: the `dist` feature, the `[profile.dist]` profile, `assets/icon.png`, `windows_subsystem` in the `dist` build, `dist/` in `.gitignore`.

**Crash reports (E10.7; `ai_core`, `shards`, `shards_core`):**

- `ai_core::crash`: a `shards-crash/1` report in `crashes/crash-<time UTC>-tick<N>.json`: what happened (the message, the place, the thread, the call stack), the game (`title`, the new `EngineConfig::version`, the program, the engine version, the build), the machine (OS with its version, processor, threads, memory, graphics card and driver), the tick and the seed, `ai/status`, the last 200 journal entries and 200 log lines, other panics, and the session's replay (if there is one; `no_replay` says why there is not: `--replay` does not play back such a report). If the working folder cannot be written to, the report goes to the player's folder (`%LOCALAPPDATA%\<program>\crashes`, `~/.local/state/<program>/crashes`, `~/Library/Logs/<program>`).
- When it is written: a gameplay system failed (together with a crash replay, one per system; `SystemFailure::crash_report`), a panic in another thread that nobody caught (in the next frame, one per place), a panic that took the game down (`shards::run` catches it, writes a report without a replay and exits with code 101), and on request: the `crash_report note=…` tool (Dev, query) and the developer menu item "Save a bug report…". A game with `crash_replays` writes them.
- The panic hook `ai_core::crash::install_panic_hook` (`shards::run` installs it) remembers the last eight panics, with their stack and session, and passes each panic on to the handler that was there before. Code that catches a panic itself (a tool's reply, a rule, the generator's seed, a peer's character) calls `ai_core::crash::handled`, and it gets no report of its own: the next report shows it among the other panics. A panic goes into reports only once; when the texts are the same, `handled` takes the panic from its own thread first, then from its own game. A report that could not be written does not use up its panics: the next report shows them, and a failed system or a panic in another thread gets a report at the next failure.
- `replay::load` and `--replay` accept a crash report as a replay (without a replay, it is an error with the reason). The log layer that passes `warn!` into the journal also keeps `info` lines for reports. The template and the sandbox name their own version; `crashes/` is in `.gitignore`.

**Screenshots without a window (E10.8; `shards`, `shards_core`, `ai_core`):**

- `--offscreen` (`EngineConfig::offscreen`): the game runs without a window, but with a GPU. Cameras aimed at the main window (when they appear, or later through a change of target) draw into an image the size of `window` (`ai_core::display::Offscreen`), the UI draws into the camera that draws last, and `shot` takes that image with `eN` labels and a legend. No window is opened, and no display is needed. Each frame waits until the GPU has drawn it: without a window nothing holds the frames, and software rendering (WARP) ran out of memory; a frame not finished within 30 s means the GPU has hung, and the game exits with code 1. Without a GPU it exits with code 2 and a hint (`--headless` or `WGPU_FORCE_FALLBACK_ADAPTER=1`). It does not combine with `--headless`, `--stand`, `--verify` or the replay checks.
- `EngineConfig::draws()`: there is a picture (a window or `--offscreen`). The sandbox and the template use it to add their own visuals, and the world uses it to stream tile visuals. Systems that only show things (the combat HUD, overlays, the UI, the developer menu, smoothing of other bodies on a client) run on the condition `ai_core::display::drawn` instead of "there is a window". `--offscreen`, like `--headless`, does not read the player's settings.
- `display` in `ai/status`: `window`, `offscreen` or `headless`, the picture size and the GPU. An error from `shot` when there is no picture names `--offscreen`.

**Budgets and the performance test (E10.4; `shards`, `shards_core`):**

- Frame time (`shards::frames`) for a game that draws (a window, `--offscreen`): a frame is an update of the main world (`First` … `Last`) and a frame of the render world for it, and it takes as long as the slower of the two. Waiting for the display (vsync) does not count; a frame in which `step` ran ticks while paused does not count. The `frames` field in `ai/status` covers the last 600 frames: the average, the worst, how many were over budget, `fps`, the main world and the render, GPU time and triangles per frame (Bevy render diagnostics, timestamp and pipeline statistics queries; not in the `dist` build). The frame budget is `--frame-budget-ms`, `EngineConfig::frame_budget` (1/60 s).
- The `counts` field in `ai/status`: world entities, meshes, meshes that at least one view draws (a camera or the shadows), and the meshes of the active cameras (drawing the picture before Bevy's batches).
- `--perf ID` in `shards::run`: the performance test for the new content kind `perf` (`*.perf.ron`, `shards::bench::PerfDef`): a region with a seed (its load is timed), setup lines like the console's, a warm-up, a measurement, and budgets for the tick, frame, GPU, triangles, region load, entities and meshes. Without a window the ticks run as fast as they can; with `--offscreen` it runs in real time and frames (exactly the measurement frames: warm-up frames that are still drawn during the measurement do not count; the game waits for the last measurement frames while paused; of the GPU reports that do not name their frame, only those that are surely about measurement frames count). One line per budget (`ok`, `OVER`, `-`), exit code 0, 1 or 2; a debug build measures, but does not judge. The sandbox's reference scene is `reference`: the wild lands with seed 42 (8×8), 15 enemies in the region, 13 of them in combat, 1080p; on an RTX 5080: 389 frames per second, all 7 budgets. `--check-content` checks the definition, and a typo in it does not break `--perf`: `tick_ms` and `frame_ms` are durations the game can hold, and the warm-up and the measurement are at most 216000 ticks. For tests: `shards::bench::play` and `judge`.

**Stylised materials (E10.1; `shards_look`, `shards`, `shards_content`, `shards_ui`):**

- A new crate `shards_look` (module `look`, in `EnginePlugins` after the audio). The `look` content kind (`*.look.ron`, `LookDef`): `color` (`#rgb`, `#rrggbb` or `#rrggbbaa`; without it the mesh keeps its own colour, the game's material or the glTF file's), `shading` (`Toon(bands)` with 2 to 8 bands, three by default; `Matte`; `Unlit`; `Lit(roughness, metallic)`), `flat` (flat low-poly faces on any mesh), `outline` (`width` in pixels, more than 0 and at most 16, and `color`), `glow` (on `Unlit` it is added to the colour), `materials` (names of the glTF materials that the look draws), and `extends`, for presets: a look starts from another one and changes only its own fields (`materials` is not inherited), so a low-poly style (`low_poly`: flat faces and an outline) is set up once.
- The `Look(String)` component (`Look::new`, in a prefab as `"Look": ("stone")`) styles the meshes of the entity and its children. A mesh's look is its own `Look`, then the nearest one above, then the look named after the glTF material (`GltfMaterialName`: in a look's `materials` or with the same id), so levels and tiles from Blender take their style without code. A mesh with no look keeps the game's material; its colour, texture, vertex colours and material transparency are kept (only an `#rrggbbaa` colour changes transparency). A `Look` in a prefab is an ordinary component: it goes into the replay and the snapshot and is sent to co-op clients; games put a `Look` that only the picture needs on the child meshes.
- Drawing (feature `render`, which the umbrella crate's `render` turns on; a window or `--offscreen`; without a picture nothing is styled): `Toon` is `ExtendedMaterial<StandardMaterial, Toon>` with a WESL fragment shader. The sun (each directional light, with shadows) is drawn in bands; ambient light, lamps and spotlights are drawn evenly, with no highlight; camera exposure applies to the light but not to `glow`. The outline is an inside-out shell (`OutlineMaterial`, with an `OutlineHull` child of the mesh): back faces, with the vertices pushed out by `width` screen pixels, so the width is the same up close and far away. Normals are averaged over the mesh's points, so there are no gaps on the edges. A skinned mesh's outline gets the same `SkinnedMesh`. The outline casts no shadows and does not go into the prepass; it takes `RenderLayers`, `NoFrustumCulling` and `VisibilityRange` from the mesh. Transparent materials and meshes with morph targets get no outline. `Toon` is always drawn in a forward pass, even in a game with deferred lighting. Flat faces are a copy of the mesh, made once for each source mesh. The systems run in `PostUpdate`, before transforms propagate (`LookSystems::Apply`), and only with a picture (`ai_core::display::drawn`); meshes whose material or mesh is still loading wait.
- Hot reload: an edit to `*.look.ron` recolours all meshes in the same frame. A `Look` that is set, changed or removed restyles the meshes under it. A mesh the game gave its own material or mesh is styled again, and so is a material or mesh changed in place (a glTF reload, `Assets::get_mut`). The cache of copies and hulls releases the ones whose source asset has been unloaded.
- The `looks` tools (Dev, query: the looks, the number of meshes for each, meshes without a look and meshes waiting for files; without a picture it says there is nothing to draw) and `look_outlines on=false` (Dev, query: hides or shows the outlines; they stay built); the `looks` field in `ai/status` (`looks`, `styled`, `outlined`, `outlines`); `Look` is visible in `inspect`; two items in the `world` section of the developer menu (`looks`, `look_outlines`).
- Content check: a colour not in the format, a band count outside 2…8, `roughness` or `metallic` outside 0…1, an outline width outside (0, 16], an empty material name, a material that two looks name, a material name equal to another look's id, a prefab whose `Look` names a look that does not exist (with a `did you mean …` hint; this check is added to the `prefab` kind with `add_content_check`), `extends` naming a look that does not exist, and a cycle of presets.
- `shards_content::parse_color`: one colour parser for the content, used by themes and looks (`shards_ui::parse_color` stays as a re-export).
- The sandbox: `content/look/sandbox.look.ron` (the presets `low_poly` and `cel` and nine looks: the player, a skeleton and a guard with flat faces and an outline; a rock and a wall with an outline; the floor and sand on two bands; `dark` is `Matte`, `marker` is `Unlit`), `Look` in the prefabs of the player, the skeleton, the guard and the column, and `visuals.rs` draws everything with one white material and the looks. The template: `content/looks.look.ron` (preset `low_poly`, `hero`, `grunt`, `raider`, `stone`), `Look` in the prefabs, and `visuals.rs` with one white material. Checked on a GPU with `--offscreen`: the sandbox picture shows light bands, flat faces and outlines, and `looks` gives `11 looks · 49 meshes styled (36 outlined) · 2 without a look · outlines on`.

**Foliage and small objects (E10.2; new crate `shards_scatter`, `shards`, `shards_settings`, `shards_look`,
`shards_motion`):**

- The `scatter` content kind (`*.scatter.ron`, `ScatterDef`): what to scatter over the tiles of which regions (`tiles` by id and `#tag`, `regions`), how dense (`density`, `spacing`), what size and tilt (`scale`, `tilt`), on what ground (`max_slope`, a height band `height`, `clearance` from walls and columns), with which looks (`looks`), with or without shadows (`shadows`), and which meshes up close and far away (`lods`: from 1 to 4 levels with `until`, and `fade` for dissolving). The meshes are engine primitives (`Blades`, a tuft of grass; `Cone`; `Rock` with `squash`; `Box`) or `Gltf("file.glb#Mesh0/Primitive0")`. `--check-content` names tiles, tags, regions and looks that do not exist (with `did you mean`), a look with an outline (its own, or from `extends`: the outline is a shell on each
  instance, and when levels dissolve it does not dissolve), `spacing` less than 0.01 m (except 0), more than 10 000 mesh entities (instances × levels) on the largest tile or room where the definition grows, numbers out of range, levels out of order, `fade` longer than its level, and glTF files with no label or outside the assets folder.
- The layout is a pure function of the region's seed, the region's id, the tile's cell and the definition's id (through `DetRng`), and of the static world: the same instances in every run. Ground and solid things come only from the world's static colliders (`Body::Static`, layer `WORLD`): a platform that moves is neither ground nor an obstacle. The ray and the `clearance` sphere query only the part of the world around the tile (a new `PhysicsWorld::subset` in `shards_motion`), not all of it. The `clearance` sphere lies on the ground along its normal, so a slope under an instance takes no room, but walls do. A ray that starts inside a solid finds no ground, so nothing grows inside walls or on top of columns. Up to 10 000 mesh entities of the definition per tile (extra instances are dropped, with a warning in the journal). Instances are for the picture only (a window or `--offscreen`): no `AiId` and no colliders. They sit under the tile's root (which gets `Visibility`) and go with the tile. An edit to the definition, to the tile's or room's tags, or to the setting scatters everything again; an edit to a look only recolours.
- Drawing: one entity per instance and level of detail, with a shared mesh, a shared white material and a `VisibilityRange`. The look (`Look`) sits on the entity of the "definition, look" pair, so Bevy draws each "mesh, look" pair in one batch, and nothing past the last level is drawn. The `Toon` shader (`shards_look`) dissolves the levels of detail (`VISIBILITY_RANGE_DITHER`).
- The `scatter` tools (Dev, query: the definitions, how many were scattered, what did not find room on the tiles that exist now and why, and how long the last layout took; without a picture, why nothing) and `scatter_set on=` (Dev, query); the `scatter` field in `ai/status`; the `scatter_set` and `scatter` items in the developer menu (the `world` section). The player's setting `graphics.foliage` (0 to 1, `GraphicsSettings::foliage_density()`) keeps the start of the same list of instances.
- `counts` in `ai/status` gained `mesh_assets` (how many different meshes are among the drawn ones). The `perf` budgets gained `mesh_assets`, and the measurement line is `N drawn (M distinct)`.
- The sandbox: `region/wilds.scatter.ron` (two kinds of grass on all tiles, bushes, rocks) and the looks `grass`, `grass_dry`, `bush` and `rock`; the reference scene `reference` now has foliage (budget `mesh_assets: 80`); the scenario `scatter.scenario`. The template: `region/valley.scatter.ron` (grass on the meadows, by the gates and columns, pebbles), the looks `grass` and `pebble`. The release `--perf reference --offscreen` with foliage on an RTX 5080: 379 frames per second at 1080p, worst frame 4.2 ms, 15 407 entities, 1364 drawn meshes, all 8 budgets.

**Effects and decals (E10.3; `shards_fx`, `shards_action`, `shards`):**

- A new crate `shards_fx` (module `fx`, in `EnginePlugins` after the look module). The `effect` content kind (`*.effect.ron`, `EffectDef`): `count`, `rate` and `duration` (up to 10 s), `lifetime`, `speed`, `spread`, `aim` (`Up` or `Hit`) and `lift`, `gravity`, `drag`, `size` and `size_end`, `color` and `color_end`, `shape` (`Quad`, `Cube`, `Shard`), `glow`, `offset`, `radius`, `spin`, and `max_live` (the pool, up to 4096). The `fx` kind (`*.fx.ron`, `FxDef`, one per game): `effects` maps an engine event (`damaged`, `blocked`, `parried`, `evaded`, `guard_broken`, `staggered`, `died`, `healed`, `telegraph`) to an effect, and `cues` maps a clip label to an effect (a label with no entry plays the effect of its own name). The `PlayEffect { effect, at: EffectAt::Entity | Point, toward }` event and the `shards::fx::play` function.
- The particles are written in-house (`bevy_hanabi` has no version for Bevy 0.20), on the CPU. A pool of `max_live` meshes per effect is made when the content has loaded; free ones are hidden; an overflow gives the slot to the oldest particle. All particles of an effect are one mesh and one material (`ParticleMaterial`, a WESL shader), and each particle's colour is in `MeshTag`, so they are drawn in batches. Squares turn toward the camera; cubes and shards tumble and are lit from above; `glow` is added to the picture. The position is a function of age (flight under gravity and drag is solved with a formula) on the simulation clock (`Time<Fixed>` and its remainder): a pause freezes it, and `step` moves it by its own ticks. An event with an attacker plays the effect on the side of the body that faces the attacker (the edge of the collider), and `aim: Hit` sends the particles back toward the attacker.
- Telegraph decals: an attack zone on the ground is a Bevy `ForwardDecal` with the zone material (`ZoneDecalMaterial`, a WESL shader): a circle, a cone sector, a box's rectangle and lines, a capsule's "stadium" shape. A pale zone has stripes at 45° to the world axes, which thicken toward the strike and converge on the point the fill grows from (counted from that point, not from the world's origin: far from the origin, the stripes do not flicker). The fill grows from the centre of the footprint toward the strike (`TelegraphStyle::horizon`), with an edge, and flashes at the moment of the strike (per the `flashes` setting). It is warm for an enemy and cool for an ally, the same as the gizmo. Decals come from a pool, one per zone. Cameras with `AiCamera` get `DepthPrepass` (`FxPrepass`) while decals are on.
- The `effects` tools (Dev, query: the effects, their pools, the table, decals on the ground; without a picture it says there is nothing to draw), `effect_play effect=… at=eN | point=[x,y,z]` (Dev, control; without a picture it refuses with a reason) and `telegraph_decals on=false` (Dev, query: the fill is a gizmo again); the `fx` field in `ai/status` (`effects`, `live`, `decals`, `telegraph_decals`); the developer menu items `effects` (the `world` section) and `telegraph_decals` (the `combat` section).
- Content check: ranges reversed or out of bounds, a colour not in the format, a pool smaller than one play, an effect with no particles, an event the engine does not have (with a hint), a second table, and an effect that does not exist, in `effects` or `cues`. A `rate` too large to count (`5e9`) gives the error `one play starts 4294967295 particles …`, not an overflow. An effect with an error at first load plays within the check's limits: a pool up to 4096 meshes, up to 1024 particles at once, up to 4096 from `rate`, life up to 30 s.
- `shards_action::draw`: `shown_telegraphs` (which zones the ground shows, for gizmos and for decals), `ShownTelegraph`, `centroid` became public, `TelegraphStyle::decals` (the gizmos draw only the outline, the symbol and the stem).
- The sandbox: `content/fx/sandbox.effect.ron` (`hit_spark` and `block_spark` (shards), `parry_flash` and `death_dust` (cubes), `heal_glow`) and `content/fx/sandbox.fx.ron`, the scenario `effects.scenario`. The template: `content/effects.effect.ron` (`hit_spark`, `block_spark`, `death_dust`) and `content/effects.fx.ron`. Checked on a GPU with `--offscreen` (RTX 5080): the `offscreen` test in pixels (a decal paints the floor only inside the zone's circle, and particles glow where they play) and screenshots of the sandbox: decals under the attacks of the guard and the skeletons, hit sparks, dust, the healing glow. The release `--perf reference --offscreen` (RTX 5080) with the foliage of E10.2 and effects: 327.2 frames per second, 2.84 ms per frame, worst 4.47 ms, all 8 budgets. The 15 836 entities and 14 641 meshes come from the foliage (about 13 900 mesh entities) and the 400 hidden meshes of the sandbox's pools (they are not in `drawn`); 1382 meshes are drawn, from 50 different ones. Without effects it was 378.9 frames per second (the example in the README is updated).

**The same bits on every OS of one CPU architecture, on x86_64 — Windows, Linux and Steam Deck (E10.6; `ai_core`,
`shards_action`, `shards_motion`, `shards_npc`, `shards_scatter`, `shards_fx`, `shards_net`, `shards`):**

- The simulation's transcendental math goes through the pure Rust library `libm`, not through the platform's math (on MSVC, glibc and
  macOS the last bits of `sin`, `exp`, `powf` differ, and a replay recorded on one OS diverged on another): in Bevy, glam, simba and the
  engine's code. In `[workspace.dependencies]`, the `libm` feature is enabled for Bevy (`bevy_math`, glam: `Quat`, `normalize`, `slerp`,
  curves, colours), `parry3d` uses `indexmap` (`IndexMap` instead of hash maps), and the direct dependency `glamx =0.3.1` (only in
  `shards_motion`, not used in the code) with the feature `libm` switches glamx and `simba` to `libm` (`libm_force`). All of this comes from
  `enhanced-determinism` in parry3d, except `scalar-math`: glam keeps its SIMD (SSE2 — the same instructions on any x86_64 OS).
  The template gets this through `shards`.
- Across architectures (x86_64 ↔ ARM, Apple Silicon) determinism is not promised: glam's NEON fuses a multiply and an add where
  SSE2 rounds twice. The full `enhanced-determinism` would give that through `scalar-math` (glam without SIMD), but with `scalar-math`
  `Vec4` and `Mat4` are not 16-byte aligned, and `MeshUniform` in Bevy 0.20 becomes 168 bytes instead of the shader's 176: wgpu
  reports `Binding size 168 of Buffer with '' label is less than minimum 176`, and rendering stalls (the GPU test in `shards_fx` did not
  get a frame).
- `shards` does not build with glam without SIMD: a `const` in `crates/shards/src/lib.rs` checks `align_of::<Vec4>() == 16` and
  `align_of::<Mat4>() == 16` and names the reason, including in a game that turns on `scalar-math` through its own dependency. A build with
  `-C target-cpu=native` or `+fma` also gives other bits: under `target_feature = "fma"`, glam's SSE2 code fuses a multiply and an
  add (`Quat`, `Vec3A::cross`, `slerp`, `lerp_monotonic`, `mul_add`).
- Identical bits were checked by running across OSes: nine reference replays (seven from the sandbox — `obstacle_course` for 1010 ticks,
  `combat_prototype` for 2307, `npc_director` for 1923, `npc_navmesh` for 1261, `region` for 353, `snapshot` for 369, `saves` for 157 — and
  two from the template, `smoke` and `valley`) were recorded on Windows 11 (`x86_64-pc-windows-msvc`) and on Ubuntu 24.04 (glibc 2.39) under WSL1
  (`x86_64-unknown-linux-gnu`), with Rust 1.99.0 on both sides, and the files matched byte for byte, all nine. Each OS checked the other's
  recordings (`every_reference_replay_verifies_headless` in the engine, `reference_replays_still_match` in the template): no divergences. Both
  sides checked on `e635768` (the branch tip at the time); the Linux recording was made on `5d1dbfe` (the simulation code is the same; the commits between them
  change only the highlight, which only draws, and the documents). WSL1 runs real ELF binaries on the same machine,
  there is no virtual machine, and the CPU is the same on both sides: what was checked is the OS, the C library and the build, not different x86_64 CPUs. Not
  checked: other x86_64 CPUs, macOS, ARM, Steam Deck hardware.
- All direct calls to `sin`, `cos`, `atan2`, `exp`, `powf`, `powi` and the like are replaced with `bevy::math::ops` (`ops::sin(x)`,
  `FloatPow::squared`): 148 calls. In the crates' code: `ai_core` 2, `shards` 1, `shards_action` 33, `shards_motion` 16, `shards_net` 1,
  `shards_npc` 4, `shards_scatter` 7, `shards_fx` 12 (particles and decals: they only draw, but one rule covers them all); in the tests:
  `shards_motion` 18, `shards_npc` 5, `sandbox` 2, `shards_audio` 2, `shards_level` 2, `shards_fx` 2, `shards` 1, `shards_look` 1;
  in the sandbox's `make_sounds` example, 39. WESL shaders (`shards_fx`, `shards_look`) are not Rust, so the rule does not touch them.
- `clippy.toml` (in the root and in `template/`; these are separate files, and clippy takes the nearest one) forbids the `f32` and `f64` methods from std
  (`disallowed-methods`, as Bevy itself does) and names the replacement from `ops`; `cargo clippy --workspace --all-targets -- -D warnings`
  fails on a direct call.
- The test `ai_core::determinism` checks several `ops` results bit for bit.

**Steam (E10.5; `shards_steam`, `shards`, `shards_net`, `shards_save`, `shards_core`, `shards_cli`):**

- The new crate `shards_steam` (module `steam`) is behind the `steam` feature of the umbrella crate. The feature is off by default; the template and the sandbox do not use it.
  `steam_sdk` adds the real Steam client (`steamworks = "=0.13.1"`, `steam_api64.dll`). Without it, the module runs on a fake Steam inside the process
  (`FakeSteam`: users, lobbies, invitations, a relay on `MemoryHub`, achievements, cloud), and a game that the player started
  says plainly that there is no Steam. The public API has no `steamworks` types.
- Co-op over the Steam relay: `--steam-host` (a lobby for `max_players` players, by default "friends only"; `--steam-lobby
  friends|private|invisible|public` — who can see it; the journal, `steam` and `ai/status` show the type as `lobby_type`; an invitation through
  `steam_invite` or the overlay), `--steam-server` (an anonymous Steam game server with no lobby: clients join it by its id; it is a host like
  any other, with its own player 0), `--join steam:<id>` (a lobby or
  a host by its id; `steam:` — wait for an invitation), `+connect_lobby <id>` from Steam becomes `--join steam:<id>`,
  `--steam-relay-only` (relay only, no direct routes), `--steam-app N` (the app id through `SteamAppId`, without `steam_appid.txt`).
  The transport is `ISteamNetworkingMessages`, channel 0, unreliable, without Nagle, and with a restart of a dropped session; `--net-sim` runs on top of it.
  UDP is not opened. The host accepts relay sessions only from the lobby's members, the server from anyone (its entry rules decide), and a client only from its own host; a refusal logs one `warn` per peer. Without Steam, the host exits with `cannot host` (with `--exit-with-host` the exit code is 2),
  and the client waits for another lobby (`steam_join`).
- Achievements are content, `*.achievement.ron` (`AchievementDef`: `steam` is the API name, `name` and `description` are Fluent messages,
  `hidden`). The game unlocks them with `UnlockAchievement::everyone(id)` / `player(id, n)` (a network event: the host sends it to the clients, and each
  machine unlocks it for its own player on its own account) or `unlock_achievement(world, id)`; a client does not unlock on its own, and during
  a replay nothing is unlocked. A write to Steam comes one second after the first unlock (one write covers several) and on exit; a write that Steam refused is retried. This is not
  simulation state: the journal writes it as `local`, and `Achievements` is a record of this run only. The content check covers the API name, a repeated name, text
  in place of a message, and a message that does not exist.
- Cloud: each save that is written is copied to Steam Cloud as `<save folder>/<kind>/<slot>.save` (backup copies, `.tmp` and `.damaged` files are never copied); at launch, before a slot loads, a newer cloud copy becomes the save (the old save becomes the first copy),
  a newer local copy goes up to the cloud, a tie keeps both (the cloud copy goes to the slot `<slot>-cloud`), a damaged copy is not taken (the file on disk repairs it),
  and a copy from a newer game version (format or section) is neither taken nor overwritten: in this run nothing is copied to its slot in the cloud. The cloud copy lands on disk with the same guarded write as a save (`write_guarded`: it does not touch a slot that holds a save from a newer version);
  during a replay the cloud is neither checked nor written. The file goes to the cloud in one `FileWrite`, and the reply is checked; when the quota is short, the error says how much space there is (`Steam Cloud has 105 B free of 4 KB;
  this save is 540 B`), and the `cloud` line in `steam` shows the free space. Files on disk stay the source of truth.
- Steam reliability: callbacks whose decoding can fail in `steamworks` 0.13 (`LobbyChatUpdate`, `UserStatsStored`,
  `SteamServerConnectFailure`) are read by the engine itself, as raw numbers, and the journal-only entries about losing and regaining Steam servers
  are removed. If a callback still fails, the callbacks stop until the run ends (the `steamworks` lock is poisoned), with one `error` line,
  a `problem:` line in `steam` and a `note` in `ai/status`, and a host that is waiting for a lobby and a client that is joining one
  get the error at once. A game the player launched, with `steam_sdk` on, starts the Steam client before rendering (`SteamClientPlugin` right after
  `TaskPoolPlugin`), so that the Steam overlay attaches to the graphics device (not checked in a window). On exit, the Steam hook
  sends copies of the saves written by the hooks that ran before it, writes the achievements, closes the relay sessions (`CloseSessionWithUser`) and
  leaves the lobby; Steam shuts down along with the game world.
- Tools `steam` (Player, query: who this machine is in Steam, lobby or server, relay, the route to each peer — state,
  whether it goes through the relay and through which one, ping; achievements, cloud), `steam_host`, `steam_join lobby=ID | host=ID`, `steam_invite
  friend=ID`, `achievement_unlock id=… [player=N]` (host only); the `steam` field in `ai/status`; the `steam` section of the developer menu. The readiness line: `role host · net steam lobby`, `net steam game server`, `role client · net steam:<id>`.
- `shards_net`: `ExternalTransport` — another module opens the session's transport (UDP is not opened); `conditioner_from` and
  `session::unopened` are public. `shards_save`: the `SaveWritten` event after each successful write of a slot. `shards_core`:
  `EngineConfig::steam` (`SteamLaunch`, the lobby type `SteamLobby`). `shards package`: the Steamworks SDK library (`steam_api64.dll`,
  a copy from the build-script folder of `steamworks-sys`, and only that: the system ALSA and udev do not go into the package) is put next to the
  program, the import check accepts it, and `steam_appid.txt` is removed from the package folder.
- Tests on the fake Steam: a lobby (and its type), a host and two clients over the relay, a game server and a client by id, an invitation, rejecting a
  stranger, a join that failed (and a join after it through `steam_join`, code 2 with `--exit-with-host`), a lobby that Steam did not
  create, and `steam_host` after it, a server that did not join, a game without Steam, failing callbacks, the exit order, achievements (content,
  a write, the previous run, a replay, co-op), cloud (a copy, cloud turned off, a check at launch before a slot loads, copies from a
  newer version, a replay, the quota), the tools' replies, the plugin order. On real Steam (app 480, one account),
  the game server and a player by id were checked only through the relay (`relayed via <pop>`, ping 16–17 ms, 0 desyncs), an achievement from
  the host written by a client to its own account, a refusal of a cloud copy that is not a save, and the cloud quota of 480 (4 KB). On a build with the fixes made after that check, also on real Steam: the host's lobby `--steam-lobby private`, the message about the cloud quota, the achievement of the lobby's host,
  and the server with a player through the relay again; joining the lobby with a second account, the overlay and a successful cloud write were not checked.
- Lobby search (`shards_steam`): the tool `steam_lobbies [all=true] [wait_ms=N]` (Player, query) finds this game's lobbies that can be joined: those with a free slot and the same build (in the lobby data the host now writes `game`, `engine`, `content` and `rules`, as well as `host`). One line per lobby: the id, the host, how many are in it and how many slots there are, and whether the build matches; `all=true` also shows lobbies of other builds, naming how they differ. Steam finds only `public` and `invisible` lobbies (`friends` and `private` never, the host's own lobby included); the fake behaves the same way, and the host's own reply says why its lobby is not listed. The tool runs the Steam callbacks itself until the reply comes (`wait_ms`, by default and no more than 5 s: the wait holds the game's frame), and if there is silence, it answers with an error and does not drop the search: the next call picks up a reply that arrives while the game keeps running. `steam_join` takes the id from the reply; the `steam_lobbies` item is in the developer menu. Backend: `Backend::find_lobbies` (the empty one; the fake one, with `FakeSteam::hold_searches`, `fail_searches` and `set_lobby_data` for tests; the real one, with `RequestLobbyList`, string-equality filters, one free slot and the "worldwide" distance, and the lobby details read in `pump`);
  `SteamLobby::in_search`. Tests: 4 in `src/fake.rs`, 3 in `src/lobbies.rs`, 7 in `tests/steam/lobbies.rs`. A lobby size that Steam did not give is shown in the line as `2 of ?`. A join to a lobby whose owner is this same account (a second game on the host's account) is refused right away with `this Steam account owns the lobby: a second game on the same account cannot join it (join from another account)`, and not with "its owner left". The game does not leave such a lobby: Steam keeps membership per account, and leaving used to take the host out of its own lobby as well (found on real Steam: the host's search stopped finding its lobby). `FakeSteam::lobby_of` makes such a lobby for a test, and the test checks that the lobby and its owner stayed. On real Steam (app 480, one account, a second game of the same account), the search was checked: the host's invisible lobby is found (`1 of 3 · build matches`, with `all=true` too), there is no `friends` lobby, and the host's reply says why; joining the lobby with a second account was not checked.
- A stand on top of the fake Steam (`shards`, `shards_net`, `shards_steam`): `--stand N --stand-steam [--steam-lobby TYPE]` (a game
  with the `steam` feature; without it, code 2) builds the same stand in one process, but the players meet through `FakeSteam`, with the relay on
  the stand's in-memory network, without delay (`--net-sim` runs over the relay once, as on real Steam): the host makes a lobby, the clients wait for the lobby (`--join steam:`), look for it with `steam_lobbies` and join with
  `steam_join`. `Stand::settle` and `loading_locally` do not wait for a client that needs someone else's transport
  (`shards_net::client::waits_for_transport`), `SteamSession::from_launch` also takes the lobby's host from `NetLaunch::Stand`, and the readiness line
  of such a client is `role client · joining: …`. The scenario `crates/shards_steam/scenarios/lobby_search.scenario`:
  the host makes an invisible lobby, the client searches, finds it, joins by the id from the reply, both play 300 ticks, 0 desyncs; it runs
  in the test (`tests/steam/scenarios.rs`) and against a live stand. Tests: `run` in `shards` (the flag and the stand; the second with `steam`).
- Linux package (`shards package`): the program is linked with `RUNPATH=$ORIGIN` (`cargo rustc … -- -Clink-arg=-Wl,--enable-new-dtags
  -Clink-arg=-Wl,-rpath,$ORIGIN`), so that the loader finds `libsteam_api.so`, which lies next to it: without this, a game with `steam_sdk` from the
  package would not start. Arguments after `--` apply to one program: not `RUSTFLAGS` (it would replace the game's `rustflags` from
  `.cargo/config.toml` and rebuild all dependencies), and not the `cargo:rustc-link-arg` of the `steamworks-sys` build script
  (it does not reach the program). The result is checked by running a copy of the package in a bare environment. Checked under WSL1 (Ubuntu 24.04) on a
  small program with `steamworks`: `readelf -d` gives `RUNPATH [$ORIGIN]`, and `ldd` on a copy of the package in another folder under `env -i`
  finds `libsteam_api.so` next to it (without it: `not found`); the flags from the game's `[build] rustflags` stay (the marker
  `-Wl,-rpath,/opt/game-config-marker` in its config gives `RUNPATH [$ORIGIN:/opt/game-config-marker]`, while `--config
  'target.<triple>.rustflags=[…]'` and `RUSTFLAGS` lose it); before the fix there was no RUNPATH. The whole game with `steam_sdk` did not build on Linux. Test: `package` in `shards_cli` (`build_command`).

**Levels from Blender are checked on a real export (E4.2, `shards_level`)**

- `tools/blender/outpost_level.py`: a level built the way an artist builds one (unapplied rotation and scale, an *Array* modifier,
  parents set with Ctrl+P, empty markers of all kinds, sensors, colliders of all shapes, a prefab, components from text, a dictionary and
  a flag, a collection instance, a lamp, names with spaces and non-Latin letters). It is exported by Blender 5.2.2 LTS without a window
  (`blender -b --factory-startup --python-exit-code 1 -P tools/blender/outpost_level.py -- <folder>`; the export is not byte for byte
  the same each time: the triangle order of the `Boulder` mesh can differ from run to run, the geometry is the same). The files are in `crates/shards_level/tests/level/blender/`:
  `outpost.glb`, `outpost_lights.gltf` + `.bin` (with a lamp), `outpost_draco.glb`, `outpost_meshopt.glb` and
  `outpost_meshopt_khr.glb` (meshopt with `EXT_` and with `KHR_`), `outpost_broken.glb` (the artist's mistakes).
- The tests in `tests/level/from_blender.rs` (14) read these files: node paths, world positions after Z-up → Y-up, the shapes and sizes of
  colliders, sensors, all the markers, the prefab and the components, `content check`, `level_info`, `spawn_level`, `level_reload`, hot
  reload (a broken export over the file is rejected, a fixed one replaces the level),
  errors with node paths; `crates/shards/tests/level_render.rs` draws `outpost.glb` with Bevy's loader.
- `LevelAsset::scene` (the scene the level's nodes come from) and `LevelNode::mesh_extras` (the mesh data's properties).

**The region in a save comes from its manifest (E5.4, E8; `shards`, `shards_world`, `shards_save`):**

- The `region` section of a world save keeps the region's manifest itself (`manifest`, section version 2), next to the id, the seed, the generator version and the hash. Loading builds the region from it (`change_region`), and the generator does not run: a game update that changes the generator (a new region `version`, weights, steps) does not rebuild the saved world. The journal says where the region comes from: `region wilds seed 42 v1: built from the save's manifest (the generator is v2 now)` (the brackets show up when the generator version in the content is different; a region with that id is no longer in the content — `` (`old_wilds` is no longer in the content) ``). The line is written once the region is built (after `region loaded`), not before the build, which may still fail.
- A tile, room or prefab from the manifest that the content no longer knows is a refusal before any change to the world, with the names and a way out:
  `` the save's world cannot be built: the manifest of wilds seed 42 v1 names tiles content does not have: old_ruin (…); keep that
  content in the game, or write a migration for `region` that replaces it in the saved manifest ``.
- A manifest that does not match its hash, or is of a foreign format, with the id, seed and version next to it, counts as a damaged copy (`section region
  does not read: the manifest of wilds seed 42 v1 is damaged: it hashes to …, the save says …`): loading takes the backup copy,
  as with a wrong checksum. The same goes for a version 2 section without a manifest: without one, only a region that came from version 1 by
  migration is read (the migration sets the mark `from_v1: true`), not any copy where the manifest has gone missing (`the save of wilds seed 42 v1
  keeps no manifest; only a save of version 1 of the section may lack it`).
- `shards_world::check_manifest` (`region::check_manifest`): whether the content can build the manifest without changing anything in the world (what `change_region` checks before any change).
- The sandbox scenario `world_save.scenario`; tests in `saves.rs` (the same world after an update that changes the generator; a tile
  is missing; a region is not in the content; damage and a backup copy; a version 1 save) and `net_coop.rs` (a world of another generator version:
  the host builds it from the manifest, the guest gets the same one).

**Avoiding bodies (E6.2, `shards_npc`)** — instead of a pre-emptive turn with shoving:

- Selective reciprocal velocity obstacles (`nav::avoid`): each NPC on the path takes the cheapest option from the fan of velocities around the one the route asks for (the deviation plus the threat of a collision within the horizon `horizon`), allowing for nearby character bodies:
  NPCs, players, agents' bodies. Another NPC that is avoiding takes half of the manoeuvre; a head-on meeting splits to the right; once it has started going around on one side, the NPC keeps to that side; right at the goal, behind a body that will not give way, the NPC waits instead of shoving. Neighbours are taken by gap, then by `AiId`, with no randomness; the fan is a table of constants. It writes only `Intent`: the new system `nav_avoid` runs in `NpcSystems::Drive` before `drive_intents`; the leash holds after avoidance too.
- The numbers are content: `avoidance: (weight, horizon, range, neighbours, margin)` in `npc_tuning` (for all) or in a behaviour
  (`BehaviorDef.avoidance`, which overrides it), `AvoidanceDef` with a bounds check; `weight: 0.0` turns avoidance off.
- `NavAgent`: the fields `preferred` (the speed the route asks for), `avoiding` (who it goes around) and `side` (which side).
  `npc_state` → `nav.avoiding`, `npc_overlay` → `avoiding e7`, the condition `step --until named:npc_avoiding`; the journal stays silent about
  avoidance.
- A straight route (the target in sight) no longer walks blind when the NPC has left the checked straight line: `NavAgent.planned_from` remembers
  where the plan was made, and an NPC farther than its radius from the segment "from there to the target" (it went around a body, or was shoved or knocked away)
  plans again from where it stands. Before, a straight route to a still target was never re-checked, and the NPC could
  walk off an edge that `plain_sight` did not see. No extra queries are added for NPCs that walk their own straight line: the next plan comes
  only after it drifts another radius to the side (test `an_npc_put_off_its_straight_line_plans_again_before_walking_off_a_ledge`).
- Tests in `tests/npc/avoid.rs` (a meeting in a corridor, a pack closing in on the player in an arc and in single file, no avoidance, determinism, `npc_state` and
  the overlay), unit tests of `nav::avoid` and `avoidance_numbers_are_checked_in_the_tuning_and_in_a_behaviour` (the bounds of `avoidance` in
  `npc_tuning` and in a behaviour, the removed `avoid_time`/`avoid_margin`); the sandbox scenario `npc_avoidance.scenario`.

**Navmesh body classes (E6.1, `shards_npc`)** — the navmesh is no longer built for one body only:

- `nav` has a `bodies` field: body classes by name (`"large": (locomotion: Some("warden"))`, `radius` and `height` on top of the body,
  `NavBodyDef`). The name is lowercase letters, digits and `_` (it is also the folder name); `agent` is taken; content checks the reference to `locomotion` and the numbers. A class's own `radius` and `height` must leave a capsule that `locomotion` allows (height at least the diameter, radius more than two `skin_width`). Otherwise the error names the class and what to fix (``bodies.wide: height 1.8 is less
  than the capsule's diameter 2.4; give the class a height of at least 2.4 m or a smaller radius``). Without this check, such a class was baked for a taller body than it named, and did not fit the bodies it was written for.
- Each tile is baked for `agent` and for each class; the class file goes in its own folder (`assets/nav/large/<tile>.navtile.ron`). The file format and `BAKE_VERSION` are unchanged. A class file's hash also takes in the class name, and the `agent` files are byte for byte the same as in a game without classes.
  `--bake-nav` and `nav_bake` bake all classes (`baked 18 files for 2 body classes (agent, large) into nav/`; without classes it prints the old `baked 9 tiles into nav/`). `--check-content` names the missing or stale file of each class (``tile `pillars` has no bake file for the body `large` (nav/large/pillars.navtile.ron)``).
- The region builds the navmesh of each class (`NavRegion::bodies`, `classes()`, `class(name)`), cut to the class's body. The journal has one line per class (`navmesh (body large): 64 tiles, …`; warnings carry the same label).
- A route uses the tightest class that serves the whole body of the walker (`NavRegion::serving`): for NPCs (`navmesh_path`, `plain_sight`), `move_to path=true`, `path_to`, `clearance` and `nav_path` (see "Fixed": NPCs first picked the class by radius alone). A body that no class serves plans on the local grid.
- Tools: `nav_path body=large` (`"body"` in the reply; a typo in it gives an error with a hint and the list of classes), `nav_info` → `bodies` (the body and the navmesh of each class), `npc_state` → `nav.routes` (`navmesh, body large` or `local grid: …` with the reason, including `the NPC stands outside the region`: the region does not cover the place where the NPC stands, so its routes use the local grid; with the game's own routes, the name of the game's provider), `npc_overlay` adds `body large` or `local grid` by the same rule, and with the game's own routes it adds nothing. Functions `region_nav`, `route_on`, `routes_for` (by body), `routes_at` (by body and
  place).
- Sandbox: the `large` class for the guard's body (`content/region/wilds.nav.ron`), files in `assets/nav/large/` (9 tiles); scenario `npc_bodies` (the guard in the wilds pylon hall walks to the player on the navmesh of its own class). The template has no classes; its files are unchanged.
- Tests in `tests/npc/bodies.rs` (7): a large body goes through the wide door, not the gap sized for `agent`, and keeps its clearance; an NPC wider than the `agent` class walks on the `large` navmesh, and one wider than all of them walks on the local grid; an NPC outside the region walks on the local grid, and `npc_state`, `nav_path` and `npc_overlay` say the same thing; with the game's own routes, the tools do not name the class; the bytes of both classes' files are stable, and the `agent` files are the same as without classes; content names the missing class file; the `bodies` check (and the capsules of classes with their own sizes).

**Character model, animation in games and displacement from a clip (F4: E3.1, E3.7; `shards_action`, `shards`, sandbox, template):**

- The script `tools/blender/humanoid.py` builds a low-poly humanoid in Blender without a window (`blender -b --factory-startup --python-exit-code 1 -P
  tools/blender/humanoid.py -- sandbox|template FOLDER`): 16 skinned bones, clips `idle`, `walk`, `run`,
  `light_1`, `light_2`, `light_3`, `heavy`, `roll`, `block`, `hit`, `death`; the sandbox also has `bow` and `taunt` in a second file
  (`humanoid_gestures.glb`). The strikes and the roll are timed to the action phases of each game (the sandbox is 60 frames per second, the template is
  100: the hero's windup is 0.18 s and the roll is 0.35 s, whole frames only this way). The phases go into the glTF as the `phases` property; the root bone
  is placed only on whole frames; the part shades are vertex colours `COLOR_0` (the look multiplies its own colour by them); two runs give
  the same bytes. Files: `crates/sandbox/assets/models/humanoid.glb` (250 728 bytes), `humanoid_gestures.glb` (41 084),
  `template/assets/models/humanoid.glb` (310 888).
- The `animset` content kind: a clip can live in another file with the same skeleton (`source` on the clip), and `root_motion` names the root bone. The bridge keeps that bone in place on the ground (x and z of the rest pose, after `AnimationSystems`), and the body moves by the action's displacement. With the asset folder, the content check reads the set's files: a missing file, a missing animation (with the list of those there are), a missing root bone, or a bone under a rotated node is an error. The set's graph waits for all of its files.
- Baking the displacement: the `bake: (animset, clip)` field on an action; `shards_action::bake` reads the root bone's translation from the glTF (`clipfile`: GLB and `.gltf`, buffers as files and `data:`, interpolations `LINEAR`, `STEP`, `CUBICSPLINE`) at the 60 Hz tick rate, takes only forward movement (sideways goes into the report, vertical goes to the picture), thins the keys (0.5 mm) and edits `displacement` in the action's file in place (comments stay; if there is no field, it is placed before `bake`). The clip's timing (`phases` or length) must match the action; otherwise an error names both. `--bake-motion [ACTIONS]` (`shards::run`, exit code 0 or 1, no ticks) and the `motion_bake` tool (Dev, control).
  The content check bakes again and names the stale curve: ``displacement is stale: clip `roll` of `models/humanoid.glb`
  bakes to 21 keys (4.0 m forward); bake it again with `--bake-motion roll` (or the motion_bake tool)``.
- Sandbox: `content/anim/humanoid.animset.ron` (movement clips; the player's strikes; the skeletons and the guard, on the hero's clips; the bow and
  the taunt from the second file; the `footstep` cue). Characters wear the humanoid instead of a capsule with a nose. The player's strikes and roll
  (`light_1`–`light_3`, `heavy`, `roll`) are baked, and the `step_dust` effect is on the step cue. Template: `content/anim.animset.ron`;
  characters are the humanoid (dead characters are no longer hidden: they have a `death` clip), the hero's `light_1`, `light_2` and `roll` are baked, and so is `step_dust`.
  `Animated` and the model are set up only when there is a picture (window, `--offscreen`); without a window nothing changes.
- The swing's sound is an action effect, not a clip cue. A clip cue (`AnimationCue`) plays a sound of the same name only where the model is drawn (window, `--offscreen`), and `audio` without a window did not see it. This went against the rule "sound is decided on the simulation clock and is the same without a window". Now `Effect` has `Sound("whoosh")`: on the tick when the action reaches the effect, the engine sends an `ActionSound` event (entity, action, sound), the sound module plays the sound from the body, and the host forwards the event to co-op clients as a hit (`net_event`; a client does not predict effects, it plays the host's). The player's swings in the sandbox (`light_1`–`light_3`, `heavy`) and the template hero's (`light_1`, `light_2`) name `whoosh` in `effects`; the `whoosh` cues are removed from the sets; the `footstep` cues stay for the picture. The sound content check also knows `Sound`: a sound that is not in `*.sound.ron` is reported as an error with a hint. The clip cue sound stays the only exception; it is described in the README, "Audio", and in AGENTS.md.
- Content check: an action's `anim` that no clip set of the game plays (a clip with that name, or a clip by the set's `actions` or `tags`) is an error, with a list of clips and a hint, if the game has sets. Drinking from the flask in the sandbox named the clip `drink`, which the humanoid does not have, and showed a movement clip; now it has no `anim`, and the picture is the same.
- The glTF reader (`clipfile`) does not hang or crash on a broken file: a cycle in the nodes (a node that is its own ancestor) is an error, not an endless walk. Accessors with zero elements, a `byteStride` smaller than the element, the wrong type, or pointing outside where the data lies are errors, raised before the `gltf` crate's iterator sees them (it panics on those). Sparse accessors are reported by name. Buffer file names in `.gltf`
  are read with `%XX` decoded (`My%20Rig.bin`), as Bevy's loader reads them. Baking takes a sample at the end of the action too, if it ends between ticks (0.57 s, not 34 ticks): the curve does not end half a tick early.
- Tests: `bake.rs` (10, including the set-file checks `the_animset_files_are_checked_with_an_asset_folder` and the sample at the end of the action), `clipfile.rs` (8: cycle, accessors, `%20`), `anim.rs` (`anim` without a clip), `the_root_bone_stays_in_place_along_the_ground` (the bridge),
  `Sound`: `a_sound_effect_*` (`shards_action`), `an_actions_sound_effect_plays_from_its_actor_*` (`shards_audio`), section 9 of the scenario `audio.scenario` (without a window), `a_swings_whoosh_is_the_hosts_event_*` (co-op), `the_heros_swing_whooshes_*` (template), `bake_motion_writes_the_curves_content_checks` (`run`), `the_blender_humanoid_rolls_in_place_and_bows_with_a_clip_of_its_second_file`
  (`action_render`: the real humanoid).
- Gameplay: the action phases are unchanged, and each curve reaches the same distance at the same moment as before. But lunges now speed up and slow down, and the roll slows down towards its end (in the first 0.1 s — 2 m instead of 1.1 m). The sandbox's reference replays (all seven) and the template's (both) were re-recorded on this branch (`record_reference_replays`, Windows 11): the content differs (the `content` check), and in the replays with the player's strikes and roll the positions differ too. Linux (Ubuntu 24.04 under WSL1) checks them, and there these pass:
  `every_reference_replay_verifies_headless` and `reference_replays_still_match`.

**Animation layers: upper and lower body (D7: E3.7; `shards_action`, sandbox):**

- The `animset` content kind: the `layers: {"upper": (root: "spine")}` field names a part of the body: a bone and everything under it (no more than 64). The values of `actions` and `tags` are now either a string (a clip on the whole body, the old form) or `(clip: "drink", layer: "upper")` (a clip on a layer). While such an action runs, the layer's bones follow its clip (scrubbed to the action's progress), and the rest follow the movement clip: a flask while walking, and a block while sidestepping. New types `ActionClip` and `LayerDef`, the function `select_layer` (what plays on the layer; `select_clip_for` chooses as if the layered action were not running), the set's methods `action_shown`, `is_layered`,
  `layer_index`; `AnimationBound::layer_clip` and `layer_share`; `Built::layer_node`. Picture only (feature `render`): `Animated` is not replayed, and `ActionState`, the replay and the snapshot do not see the layers.
- How it works (`render::bridge`): each layer clip in the graph has its own node, with a mask from the bones **outside** the layer (Bevy's mask switches a bone off, and a bone outside all groups always plays, so the layer's group is the complement). The group is built by `bind_players` from the bones of the player's model (a layer bone that is missing gives a warning, and the action keeps the movement clip). The movement clips run over all bones through `AnimationTransitions`, as they do without layers. The bridge tracks the layer clip's share on its bones itself: a weight of `s / (1 − s)` against the movement, which weighs 1 in total, with the share up to 0.99999. Entry takes the `blend` of the action's clip, exit takes the `blend` of the movement clip, and one layer clip passes into another with no gap. On paused clocks the layer clip settles at once, as the main one does. A set without layers builds the same graph and plays as it did before.
- Content check: an empty layer `root`; a layer over the limit; an action's or cue's layer that the set does not have (with a hint; for a set without layers, how to add one); a layer bone that is not in the set's file (with the assets folder). A typo in the `(clip, layer)` entry is a parse error that names the field.
- The script `tools/blender/humanoid.py`: the `drink` clip (hand to the face, head back, down; it moves only the spine, the head and the arms, using the phases of the sandbox's flask) is in the second file `humanoid_gestures.glb` (52 272 bytes). The rest is untouched: the `humanoid.glb` of the sandbox and of the template matched byte for byte on a second run, as did the `bow` and `taunt` clips and the skeleton, skin and meshes of the second file, by the data bytes of each animation.
- Sandbox: `layers: {"upper": ...}`, the `drink` clip, and `actions: {"drink": (clip: "drink", layer: "upper")}` in `anim/humanoid.animset.ron`: drinking from the flask uses the arm, and the legs walk or run. The `drink` action is unchanged (the set decides the picture). The content hash goes into the replay and counts the sets, although they are only for the picture. So the sandbox's seven reference replays were re-recorded (Windows 11): only `content` and the hashes it feeds changed in them, and the entities and components are the same. Ubuntu 24.04 under WSL1 verified this recording. The template's replays are untouched.
- Tests: `anim.rs` (7: two forms of the entry and a parse round trip, choosing the layer's clip and the body's clip, a layer for a dead and a staggered character, content checks, a typo in the entry, the layer limit, the example from the documentation), `bake.rs` (`a_layer_must_start_at_a_bone_of_the_model`), `render::bridge` (8: a node with a mask and a bone group, arms by the layer clip and legs by walking, a change of the movement clip under a layer, entering and leaving, a layer-to-layer transition, the layer clip's cues, a model without the layer's bone, a set without layers, shares and weights without Bevy),
  `the_blender_humanoid_drinks_with_its_arm_while_its_legs_keep_walking` (`action_render`: the real humanoid's pelvis and legs match the walking clip, and its arms, head and spine match the drinking clip; both are taken straight from the clips' curves).

**3D picture resolution: `graphics.render_scale` (E8.6, E10; `shards`, `ai_core`, `shards_settings`)** — picture only:
replays, snapshots and the simulation are the same:

- The 3D world is drawn into a picture of `render_scale` x the window's pixels (or the `--offscreen` picture), not into the window: at 0.5 that is a quarter of the pixels, at 2 it is supersampling. The picture is stretched over the whole window with a linear filter; the UI and HUD stay at the window's native resolution. The module `shards::render_scale`: each `Camera3d` aimed at the window (or at the `--offscreen` picture) draws into the `SceneImage` image (`ai_core::display`), which changes size with the window and the setting. One `Camera2d` without layers (it does not see the meshes: `counts` are the same as at 1.0) shows it over them
  as a UI `ImageNode` under all screens, and it carries the UI. At 1.0 nothing is added: the cameras draw into the window, as before. This was chosen
  instead of Bevy's `MainPassResolutionOverride`: that one only narrows the main pass's viewport inside a full-size texture, and waits
  for an upscaler (DLSS); without one, the picture stays in a corner, and tonemapping runs over the whole frame.
- The camera target's scale is `render_scale` x the window's scale, so the camera's logical size is unchanged (the window's size): `world_to_viewport`,
  HUD threat markers and UI nodes did not change. The `shot` legend is computed in picture pixels (`picture pixels / logical camera size`
  per axis), and the `eN` labels sit on the same pixels as at 1.0 (to within a pixel); `shot` captures the window, not the 3D image.
- `settings_set group=graphics key=render_scale value=0.5` takes effect in the running game. The image side is at most 8192 (the texture size limit of
  wgpu): a window larger than that at a scale above 1 is drawn at 8192 and stretched.
- `display` in `ai/status` got `scene`: `{"size": [640, 360], "scale": 0.5}` (at 1.0 — the size of the picture itself); for a game without
  a picture, `null`. The resource `ai_core::display::SceneImage` exists while 3D is drawn apart from the picture; `display::picture_size`.
- The outlines of `looks` and the gizmos (lines and label text) are drawn in the pixels of the 3D picture, so at 0.5 they are twice as thick on screen, and at 2 they are
  twice as thin; this is not compensated for.
- Tests: `shards::render_scale` (10: image size, window, the scale system, return to 1.0, other cameras' images, cameras without 3D),
  `ai_core::shot` (labels in picture pixels), under `#[ignore]` in the sandbox `render_scale_draws_the_world_at_a_fraction_of_the_picture_and_shows_it_over_all_of_it`
  (on a GPU: a 640x360 image with a 1280x720 window, the picture whole and like the one at 1.0, the `e5` label within a pixel, back to 1.0).

**Draw commands per frame: `draws` in `frames` and in the `perf` budgets (E10.4; `shards`)** — picture only: replays and the simulation are the same:

- Bevy 0.20 neither keeps nor exposes a draw-call counter: the `batch_sets` field of `BinnedRenderPhase` is private, and wgpu gives no
  counters of its own. `shards::draws` counts what the phases hand to the pass, in the render world after batching (after
  `PrepareBindGroups`, before the render graph), summed over all frame kinds, cameras and shadow maps. For binned phases (`Opaque3d`,
  `AlphaMask3d`, the depth and deferred passes, `Shadow`, 2D): a multi-draw set, a batch (`batchable_meshes`: one mesh and one
  material is one batch), and each entity that cannot be batched. For sorted phases (`Transparent3d`, `Transmissive3d`, `TransparentUi`,
  `Transparent2d`; they hold the gizmos and the UI): the item that starts a batch (the `SortedRenderPhase::render` loop).
  A multi-draw is one command, however many meshes it draws inside. Full-screen passes that are not
  phase items (upscaling, tonemapping, bloom), compute work and copies are not counted. Without GPU preprocessing (WebGL) there is no number (`null`).
- `frames` in `ai/status` got `draws` (the average over 600 frames) and `draws_max`; the autotest line is `75 draws`; `perf` has a new
  budget `draws` (commands per frame on average, at most; the line is `-` where there is no number). The commands come with the frame from the render world, as
  its time does, and there is no need to wait for a GPU report. The reference scene: 75 commands for 1395 drawn meshes (on the sandbox arena 57, at
  `render_scale` 0.5 — 62). The `draws` field does not go into a definition's JSON until it is set: the content hash (which the replays check)
  is computed from that JSON, and definitions without a `draws` budget, such as `reference.perf.ron` and `valley.perf.ron`, keep their old hash;
  the budget for the reference scene (150) is added at the next re-recording of the reference replays (below, `reference96`).
- Tests: `frames` (commands travel with their frame, the GPU with the batches closed), `bench` (the verdict and the `draws` line; an unnamed budget does not change the definition's JSON), under `#[ignore]` in
  the sandbox `draws_follow_how_bevy_batches_not_how_many_meshes_are_drawn` (on a GPU: forty cubes with forty materials of one
  pipeline add no commands; four pipelines add some, but not one command per mesh).

**Broad phase of the physics (E2.1, `shards_motion`)** — the same answers in the same order, and the same replays:

- The `PhysicsWorld` queries (`raycast`, `shapecast`, the controller's move and ground probe, `overlaps`, `penetrations`, `closer_than`,
  `volumes_at`, volumes under the capsule) no longer scan all colliders. `physics/broad.rs` is a grid on the ground plane (x, z) in four
  levels, with cells of 2, 16, 128 and 1024 m: a collider is put in the cells of its AABB on the first level whose cells are not smaller than it (as a rule, in
  four cells). Anything wider than 1024 m goes into a list that every query reads. A ray reads the cells of its segment in pieces no wider
  than a ground cell, with a margin wider than the rounding of the ray-box test. What is found is checked in `ColliderId` order with the same checks
  that used to run over all of them, so the answers and their order are those of the full scan. Below 32 colliders, a query the size of the world, a ray
  across the whole region, and a query that is not finite, use the full scan. `PhysicsWorld` is still a cache from the ECS: nothing was added to the snapshot,
  the replay or the hash; a body that moves within its own cells costs the grid nothing.
- Timing in release (`broad_phase_timing`, `#[ignore]`): 2644 colliders on 8×8 tiles of 96 m — eight body queries take 16 µs
  instead of 50 (`overlaps` 17 times faster, downward rays 15 times, pushing out 9 times, stepping 4 times); 212 colliders on tiles of 16 m — as
  the full scan. The full-size region (`reference96`) loads in 0.6 s instead of 3.9, and a tick takes 0.5 ms instead of 0.9.
- Tests: on random worlds from a fixed seed (from 40 to 2500 colliders: from pebbles to 4 km slabs, rotated, offset, with volumes, layers and moving bodies), every query through the grid answers the same as the full scan, both the set and the order;
  after moves, removals, new ids and new colliders, the grid holds each live collider in exactly its cells;
  huge, long and distant colliders, and boxes on cell edges; part of a large world. The sandbox's and
  the template's reference replays, recorded with the broad phase before the content edits, matched the previous ones byte for byte.

**The full-size region and its autotest: `reference96` (E5.7, E10.4; `shards_world`, `shards_npc`, `shards`, sandbox)**:

- `TileDef::tile_size` (`shards_world`): the cell size that a tile is made for. A region's "all tiles" (an empty `tiles`) means the tiles
  for its cells (with no field, or of the same size). A region that names a tile for other cells (by id, `#tag` or in
  `fixed`) is a content error that reports both sizes. Without the field, the definition's JSON stays as it was, and the content hash of the regions does not shift:
  the manifests of the wilds, the meadow and the valley are the same.
- The `nav_bake_s` budget of `perf` (`shards`): seconds to bake the navigation of all the region's tiles for all body classes, like
  `--bake-nav`, without writing files (`shards_npc::bake_region`). It is measured after the region loads, and only if it is set. The result line reads
  `nav bake 0.80 s, 8 tiles x body classes (at most 4.0 s)`. `--perf` writes it next to the load. Like `draws`, it stays out of the definition's JSON
  until it is set. Without a region, or with a value not above zero, it is a content error.
- The sandbox: the `expanse` region is 8×8 tiles of 96 m each, as in the spec (768 m on a side). It is built from four of its own tiles (a gate in the middle,
  a meadow with thirty boulders, a grove of 36 columns, a walled courtyard; about 1900 colliders), three camps, and its own foliage, twenty times
  sparser than in the wilds (it is scattered over all the tiles on load; the wilds' density would give about 470 000
  mesh entities). The wilds' foliage is now only in the wilds (`regions: ["wilds"]`). The navmesh of the four tiles is baked for both
  classes (8 files of 1.4 MB, about 0.1 MB in git). The sandbox window also draws the collider spheres (a low-poly sphere, look `stone`).
- `perf/reference96.perf.ron`: the expanse with seed 42, the player at the gate, twelve skeletons, 15 enemies, 1080p. Budgets for tick, frame,
  region load (3 s), baking (4 s), entities, meshes, triangles, draw calls. A release build on RTX 5080: 352 frames per second,
  2.61 ms per frame, worst 9 ms, tick 0.63 ms, load 0.61 s, baking 0.80 s, 62 draw calls, 34 976 entities; all 11 budgets.
  We did not add streaming of tile geometry for physics by distance. At these numbers it is not needed (the README, "Active zones and streaming").
- In `reference.perf.ron` the `draws` budget is 150 (measured 76). The sandbox's content hash shifted, and its reference replays were
  recorded again (Windows). The difference is only the content hash and the check and state hashes computed from it. The template's content did not
  change, and its replays are the same byte for byte. `--worldgen-report wilds --seeds 1..1000` is as before, `expanse` is 1000/1000.
- Tests: `bench` (the verdict for `nav_bake_s`; a budget that is not set does not change the JSON; definition errors), `shards_world` (a tile for
  other cells is not part of "all tiles"; an error where it is named), the sandbox (`reference96` without a window: load, 8 bakes, 15
  enemies, 12 in combat; the number of content definitions is 134 in 40 files; the `scatter` scenario has 6 definitions).

**Protecting a co-op session (E7, E8.7; `shards_net`, `shards_content`, `shards_level`, `shards_save`, `shards::coop`)**:

- Joining starts with a knock, after which the host stores nothing (`shards_net::cookie`). On a knock (`wire::knock`, 64 bytes),
  the host answers with a cookie (`wire::challenge`, 20 bytes): SipHash-2-4 under the host's random key, over the client's address and session, the channel's protocol version and a 30 s window. Only a datagram with a message and with a cookie
  issued to this address and this session creates a peer (an entry in the strangers table, a channel); the cookie lives 30–60 s. The client carries it in the acknowledgement fields of its header until it hears any byte from the host,
  and knocks again every 500 ms. A forged address no longer takes a slot in the table; its limits and parsing budgets
  are as before. `Transport::reply` sends a reply to a stranger and sets up no peer (over UDP the address stays an unknown
  sender; `Conditioned` does not create conditions for the peer). The host's `net_status`: `strangers` covers knocks, the things dropped without a cookie, and games
  of the previous build. A knock with no answer for longer than `timeout_secs` ends the join: `no answer from the host … to the hello in 10 s: is it
  running there, and of this build?`.
- A character over 64 KB (up to 1 MB, `MAX_CHARACTER_BYTES`) is sent in parts after `Hello` (`Msg::CharacterPart`, 32 KB of JSON each).
  `Hello` names the number of parts and the number of bytes. The host refuses a character that is too large straight away, and it holds the player's place for the peer while the parts
  arrive (`PeerStage::Character`). It assembles them in order and within the limits. A character that does not arrive in time for the join is refused, and the refusal gives the number of parts.
- A client saves its own character through the host: `save_game what=character [slot=…]` sends `Msg::CharacterAsk`. The host captures the body
  that the player controls on the host, and answers with `CharacterSave` (reason `save_game`) or `CharacterNotSent` with a reason, at most once a second per player. The client writes the named slot (it becomes the current one at once). The request carries a number, and the answer echoes it: the answer goes to the slot of its own request, even if a later request has already made another slot current. `shards_net::client::ask_character`,
  `SaveReport::asked`.
- Content does not change while other machines are playing on it: `ContentGuards` and `add_content_guard` (`shards_content`), `content_held`.
  A session holds the content on the host from a client's `Hello` on, and on the client while it is in the session. `content_reload`, `content_apply` and
  `level_reload` refuse (`refused: a content reload during a co-op session would desync its 2 clients; …`). An edit from disk waits and is applied when no clients are left or the session has ended (the journal says so once). Nothing is spawned from a level file whose edit is waiting (`spawn_level`, a region's tile): the asset server has already read the new version, `shards_level::held_change`.
- Tests: `shards_net` (+15: SipHash on reference vectors and against std; the cookie: address, session, lifetime, forgery; the format of the knock,
  the challenge, the cookie and protocol 1; a reply to a stranger over UDP; a knock that leaves no trace on the host; the cookie of another address or session, a made-up one and
  an expired one; the strangers table with a cookie; the previous build on its protocol and the limit on replies; a character in parts), `shards_content`
  (+1: an edit waits), `shards_level` (+1: a file with a waiting edit is not spawned), `shards_save` (the text of the report "asked the host"), the sandbox (`net_guards.rs` +5: a stranger without a cookie; a knock from hundreds of addresses; the previous build on the stand; a knock with no answer; a ban on editing content on the host and the client; `net_coop.rs` +4: the message limit before joining, for `Hello` and a part; a 1 MB character in parts at 150 ms and 2 %, and a refusal at one byte more; saving one's own character through the host and the once-a-second limit; the answer goes to the slot of its own request; a refusal outside a session; `net.rs`: the client knocks over UDP), the stand scenario
  `stand/session_guards.scenario`. The reference replays did not change: the network does nothing in single-player, and the content is the same.

**The rest of the interface: the D-pad, the key bindings screen, the main menu, the loading picture, the system language, the font with Cyrillic (E9.2–4,
E2.4, E1.3; `shards_ui`, `shards_motion`, `shards_core`, `shards_save`, `shards_locale`, `shards`, the sandbox, the template)**:

- The gamepad D-pad moves through the menu: the binding `Binding::Buttons2D { up, down, left, right }` (four buttons as a 2D axis, label `Pad D-Pad`).
  The standard `navigate` of the `menu` context reads it next to the arrows, WASD and the left stick, with the same repeat.
- Key bindings screen: the `Binding` widget (`context`, `action`, `slot`, `prompt`, `taken`) shows an action's inputs the way the player reads them
  (`Binding::label`: `Space · Pad South`, `Left Shift`, `Mouse Right`). The source `input.bindings` gives them from the `controls` settings.
  A row that is confirmed waits for the next key, mouse button or gamepad button, and the menu passes it on with a `ui_set` call
  from the `input` agent (the replay records the call). Escape and Start cancel the wait without closing the screen. An input that another action of the context reads is rejected, and the row names that action (`E is already used by Interact`; in Russian, the text comes from `taken`);
  `value=default` restores the defaults. In the sandbox and the template, the controls page has a "Keys and buttons" panel for 11 actions.
- Main menu: a game with `after_loading: EngineState::MainMenu` and a screen `main_menu: true` shows it after loading, where
  a person can be (`EngineConfig::menu`, `MenuLaunch::Auto`: a window or `--offscreen`, with no session and no named saves).
  Without a window, in tests, replays and checks, the game goes straight to the world, with the same ids. `--menu` shows the menu even without a window, and `--no-menu`
  skips it, even with a window. Time stands still in the menu. The player tool `new_game` (and `start_game` in code) starts the game inside
  the call. `continue_game` continues from the newest save of each kind (a slot that cannot be read is skipped; with no saves, it is refused with a reason).
  `shards::save::can_continue` checks whether there is anything to continue (the template's hint under "Continue"). If the world is changed in the menu (a developer step, a spawn), the replay recording starts again from the world the game
  started with. The `pause` key opens `pause` only in the game. The `Image` widget (`path`, `label`, `width`, `height`) draws
  a picture from the assets folder. The content check names a file that does not exist.
- Loading picture: `EngineConfig::loading_image` covers the whole window, above all the screens, while the engine is in `Loading` or `RegionLoading`
  (feature `render`). `--check-content` names a picture that is missing. A region change inside one frame call draws no frames, so the loading picture is not visible during it. The template has the main menu (New game, Continue, Settings, Exit) in English and Russian, and the picture
  `assets/ui/loading.png` (1280×720, 51 KB), which `cargo run -p sandbox --example make_art` draws from the seed.
- System language on first launch: if the settings are stored on disk and have no language in them, the game takes the first system language that
  it has (the same tag, otherwise the same language: `ru-RU` → `ru`, `pt-PT` → `pt-BR`), and saves it in the settings
  (`sys-locale =0.3.2`: on Windows, the user's interface languages; on Linux and macOS, `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`).
  Journal: `locale: the system's language is ru-RU; playing ru (kept in the settings)`. Tests, a run by an agent without a window and `--no-settings`
  do not keep settings and play in the game's language. `shards_locale::SystemLanguages` replaces the system languages in tests.
- The engine's default font has Cyrillic: `shards::ui::DEFAULT_FONT` (Fira Sans Regular, OFL 1.1, licence next to it) replaces
  Bevy's font (ASCII only), so a theme without `font` draws Russian, `·` and `…`. The font copies in the sandbox and the template, and their themes made for the font, are removed.
- Tests: `shards_ui` (+18: the key bindings screen — `bindings.rs` 7; the main menu and the loading picture — `main_menu.rs` 7; the D-pad and the stick, Russian text in the engine's font and the key row in the window, parsing `Binding` and the main menu), `shards_core` (+6: where the menu is shown, time in it, `new_game`, restarting the recording), `shards_save` (+4: `continue_game`, the hint under "Continue" with damaged copies), `ai_core` (+1: the letters of the engine's font), `shards_locale` (+5: the system language),
  `shards_motion` (+1: input labels), the template (+7: the path through the menu — the same game and the same replay, "Continue",
  the picture, the keys on the controls page), the sandbox scenario `remap.scenario`. The content hash of the sandbox and the template changed
  (screens, texts, the theme made for the font): seven reference replays of the sandbox and two of the template were recorded again on Windows 11; in them
  only `content`, `state_hash` and `hash` changed. Sandbox: `ok: 133 definitions in 39 files` (was 134 in 40).

**Small engine and content changes (E1.1, E4.1, E4.2, E4.4; `shards`, `shards_core`, `shards_content`, `shards_level`)**:

- The game turns engine modules off: `EngineConfig::without` (`--without audio,scatter`) names them, as `ai/status`
  (`engine.modules`) does, and `EnginePlugins` does not add their plugins. 14 modules can be removed (`shards::modules::OPTIONAL_MODULES`:
  `level`, `world`, `npc`, `net`, `save`, `coop`, `locale`, `ui`, `audio`, `look`, `scatter`, `fx`, `steam`, `dev`). A module that another one needs can be removed only together with it (`ui` needs `locale`, `steam` needs `net`), and the connecting ones (`coop`, levels as tiles) are removed along with any one of their modules. `--without net` with `--host`/`--join`/`--stand`, `--without save` with `--character`/`--world`,
  `--without steam` with the Steam flags, and `--without world` with `--worldgen-report` are errors with exit code 2. The stand's clients over UDP (`--stand-udp`, child processes) get the host's `--without`, as the clients of `--stand N` take its setting. Tests: `shards` (+4: `modules` 2, `tests/engine.rs` 2 — the engine without each module, and a refusal without the needed one; parsing `--without` in the flags test, `--without` for the stand's clients in `stand_udp`).
- Content (`shards_content`): a definition that does not fit its type (an extra or missing field, a wrong value) is an error on its own, with the line and column inside the file, the kind and the id. The other definitions of the file are loaded. Before, such
  a file gave nothing to the database, and false "unknown action …" errors followed from files that referred to its valid
  definitions. A whole file still does not load only if the file's top-level map is broken (syntax, empty, a repeated or
  invalid id). An edit to a file is applied as a whole: while even one definition in it is broken, the previous version applies to
  all of its definitions. Valid content is read as before (the same hash). Tests: `shards_content` (+9: `tests/content.rs`
  7, `db.rs` 2).
- `content check` reads the level files of tiles the same way as the `level` files (E4.4): a component or a prefab that does not exist,
  a `navlink` that leads nowhere, a property that does not parse, and a file that cannot be read are errors of the tile, with the file name and the node
  (``tile `ruin`: geometry: tiles/ruin.gltf: node `Ruin/Column`: unknown component `Helth` (did you mean `Health`?)``).
  Before, only the file's existence was checked, and the nodes only if the file was also named by a `level` definition. Such a file still
  reports itself once: through `level`, or, with no type registry and `level` unable to read the file, through the tile. Tests:
  `shards_world` (+4, `tests/world/levels.rs`), `shards` (+1, `tile_levels`).
- Levels from Blender (E4.2): the glTF *Embedded* variant is checked on a Blender 5.2.2 export (in the add-on this is the setting *Allow glTF
  Embedded format*): `outpost_embedded.gltf` reads, spawns and draws the same way as `outpost.glb`
  (`the_glb_and_the_embedded_gltf_read_and_spawn_alike`, `crates/shards/tests/level_render.rs`). The sandbox's `ruin.gltf` and
  the template's are now Blender exports in this variant, not the output of the test generator: `tools/blender/ruin_tile.py` builds it
  (`blender -b --factory-startup`; the output is the same byte for byte), and `the_committed_ruin_is_the_tile_the_spec_describes` checks
  it against the tile's description (nodes, properties, world transforms, mesh bounds). The geometry is the same: the ruin's navigation was baked
  again, the walkability grid did not change, and only the source's digest changed. Tests: `shards_level` (+1). The reference
  replays with the ruin (`npc_navmesh` and `region` of the sandbox, `valley` of the template) were recorded again on Windows 11: only
  `game.level` (the digests of the level files), the `hash` of the checks, and the `FromLevel` with the `AiId`s of the ruin's markers changed, which swapped places
  with each other (Blender writes the root nodes by name, and the generator wrote them in creation order); the game's numbers are as before.
- A relative `BEVY_ASSET_ROOT` (`BEVY_ASSET_ROOT=crates/sandbox target/debug/sandbox`) is made absolute by the engine from the working folder, and hot reload works with it (checked by editing `player.action.ron` in a running game). The README and AGENTS.md no longer require an absolute path, and the limitation in `docs/engine-progress.md` is removed.

**Navigation links in the reachability check (E5.3; `shards_world`, `shards`)**:

- A tile has a `navlinks` field: the navlink markers of its level file, as the generator takes them. Each gives the marker's name and its `links` group, and where it leads from and to (`[(name: "drop", from: 0, to: 1)]`). A link is one-way, from the marker to `to`, as in the navmesh. The `reachable` rule uses them too: a point beyond a drop is reachable if a navlink leads down from the spawn point's side, and no navlink leads up from below the drop. A `links` group with no sides (`[]`) is now allowed if a navlink from another group leads into it: a plateau or a pit in the middle of a tile. `RegionGraph` is a directed graph instead of a union of sets: `reaches(from, to)` is one way, and `connected` is both ways, as before.
- Generation is still a pure function of the seed, the version and the content: it does not open level files. `content check` verifies that a tile's `navlinks` match the markers of its file: a navlink in the file that is not in `navlinks`, a navlink named in `navlinks` that the file does not have, and two navlinks with one name are errors of the tile, with the file name and the node. The list itself is checked too: a group past the last one, an empty or repeated name, and `navlinks` on a tile with no level file.
- Empty `navlinks` stay out of the definition's JSON: the content hash, the manifests, `region.scenario` and the reference replays are unchanged. The sandbox and the template have no tiles with a navlink, so the generator's results did not change and the regions' `version` is unchanged: the `wilds` and `valley` reports for 1000 seeds match the old ones line for line, except for the run time. Tests: `shards_world` (+6: `tests/world/checks.rs` 3 — a one-way link, a plateau only by navlink, the check of the list; `tests/world/levels.rs` 3 — the check against the file).

**The rest of E11: snapshots and replays, the player agent, phase E scenarios, overlays (E11.2, E11.4, E11.6, E11.7; `ai_core`,
`shards_content`, `shards_level`, `shards_world`, `shards_npc`, `sandbox`)**:

- Content edits are included in the snapshot: `ContentEdits` (a snapshot resource, present only while the database differs from its files) names the definitions that `content_apply` set, and the `content` preparation puts them over the game's files, into which the snapshot is loaded, before the `content` check. Before, a fresh game refused (`the content files differ`), and a replay from a snapshot taken after an edit did not play. An edit that rebuilt geometry outside the entities with an `AiId` (the walls of the sandbox's `layout`) still stops at the `static_colliders` check.
- Levels placed by id (`spawn_level`, `spawn_level_by_id`) are included in the snapshot. `PlacedLevels` keeps the instance number, the place and the first `ColliderId`. Loading removes this game's levels before the region, and puts the snapshot's levels after it, under the same numbers and collider ids. A refusal gives back the old ones. `LevelInstances` are ordered by number. The hints of the `levels` and `static_colliders` checks say what the snapshot does not include: a level that the game placed itself, not by id. A snapshot taken earlier, while a level placed by id was in place, no longer loads (the level is removed, and `levels` refuses): it must be taken again. For a file that the game has not loaded yet (a tile or a level that an edit added), loading asks for it and refuses before any change: `… which this game is loading only now: load the snapshot again in a moment`.
- `ai_core`: `add_snapshot_prepare_after(name, after, covers, fn)` — a snapshot preparation that runs right after the named one, in whatever order the modules were added. `add_playback_hook(fn(&mut World))` — a module asks for what a replay will need inside `replay::play`, before the first frame and tick, and holds the loading gate. `shards_content::defs_ahead::<T>(&Replay)` — the definitions that the replay's start edits and its `content_apply` will add. The world and the levels request the tile, room and level files from them.
- `crash_report` takes `path` (a file inside `crashes/`; the next call replaces it. A path outside the folder, `..` and an absolute path are refused, with an example). A report on request is written whatever `crash_replays` is set to: the flag turns off only the reports that the game writes by itself. The documentation said otherwise.
- The tile and navmesh overlays first work out what to draw (`TileDraw`, `NavDraw`), and only then hand it to the gizmo, as `CombatDraw` does. The picture is the same as before.
- Sandbox scenarios: `budgets.scenario` (the `perf` definitions and refusals with a reason), `display.scenario` (`graphics.render_scale`, the settings screen's slider, `shot` without a window), `crash_report.scenario`, `stand/agent_player.scenario` (E11.6: an agent with the player scope on a stand client drives player 1, a refusal of a dev tool with a reason, and the body moves on the host and on both clients). The phase E module scenarios are now all in CI: look, foliage, effects, budgets, crash report, screen. The lobby search with FakeSteam is in `crates/shards_steam/tests/steam/scenarios.rs`. Without a scenario: `shards package` (the CLI build) and `quit`.
- Tests: `shards_world` (+7 the tile overlay, +7 `tests/world/levels.rs`: files ahead of the replay — a tile and a level; a snapshot with an edit that added a tile, by hand and by replay; levels by id in a fresh game, with a number collision, in a replay and on a refusal), `shards_npc` (+6 the navmesh overlay, feature `render`), `ai_core` (+3: the order of the preparations, the playback hook, `crash_report path`), the sandbox (+4 `snapshots.rs`: an edit comes back and goes away, a fight after an edit, a replay in a fresh game, a refusal on rebuilt walls).

### Fixed

- `shards_locale`: through `shards::locale::def`, a type from the fluent crate was visible (`Parsed` with `FluentResource`,
  `parse_language`, `syntax_problems`), although third-party crates are hidden behind the engine API. Games did not use them.
  Now they stay inside the crate. A check of the common spec requirements for phases C–E (`docs/engine-progress.md`,
  "General requirements of the spec") found this. The same check also recorded what C–E still lacks: the modules' own
  `--until` conditions, except for the network; the network feel and sound attenuation numbers in the content; and tests
  that the language, developer menu, effects and foliage do not change the replay hash.
- The manual's `content_list` example named the kinds and numbers of old content (`theme 1`, a total of 125 against 133 definitions), and the sandbox's content table listed the deleted `ui/arena.theme.ron`.
- Replay: `content_apply`, which adds a tile or a level with a file, and `regen` or `spawn_level` in the same tick (this is
  how a replay plays them, with no frame between them) did not find the file loaded, and the replay diverged. Now the replay
  requests such files as soon as it is armed, and the loading gate waits for them.
- Content: an edit to a file in which a definition was dropped without an error position (its id is already taken by another
  file, or it has no kind in its JSON) was applied, although a file is applied in full or not at all. Now any file error holds
  it back, and a check error (a broken reference) holds it back only when it is new. Test
  `a_file_loaded_with_a_taken_id_applies_no_edit_until_it_is_renamed`.
- NPC (`shards_npc`): the route knows the whole body of an NPC, not only its radius. Before, `NavPaths::find`, `find_path` and
  `navmesh_path` took a radius, the navmesh class was chosen by it (`NavRegion::for_radius`), and the NPC walked with the body
  of that class. An NPC taller than the class it fits by radius got a route under an arch or a lintel that its capsule does not
  pass, and it got stuck under them. Its straight line to a goal in view (`plain_sight`) and `nav_path` from an NPC were judged
  the same way. Now they take the tightest class that serves the NPC's body (`NavRegion::serving`: not narrower, not lower, the
  step, climbing and slope are no worse, the same swim depth; of those, the narrowest, then the lowest), and they walk with
  that class's own body. A body that no class serves plans on the local grid with its whole body (before, with the radius and
  the other default numbers). `npc_state` (`nav.routes`) says why: ``local grid: no body class serves the body: agent has height 1.80 m, the body 2.40 m``
  (`NavRegion::unserved`, `NavRegion::lack`; a body wider than every class keeps the former text ``no body class fits a 1.60 m radius …``),
  and `npc_overlay` shows `local grid`. A straight line to the goal outside the navmesh (`straight_clear`) sweeps the body's
  capsule from the step height to the top of the head, not a sphere above the step: under a lintel lower than the body there is
  no straight line. The NPC bodies of the sandbox and the template match their classes, so their routes and reference replays
  are unchanged. Tests `a_tall_npc_routes_on_a_class_as_tall_and_never_under_a_lintel_it_does_not_clear`,
  `a_tall_npc_no_class_serves_plans_on_the_local_grid_with_its_whole_body`,
  `a_goal_through_an_arch_is_in_plain_sight_only_for_a_body_that_clears_it`, and unit tests for choosing the class in
  `nav::region` (3).
- Levels (found by a real export from Blender): `to` on `navlink` also accepts the *Data-Block* property (an object, which the
  exporter writes as `{"name", "type"}`), with no second, false error "node is not a navlink". A file with a required extension
  that changes no nodes (`KHR_lights_punctual` with *Punctual Lights*, `KHR_texture_transform`) is read even without the
  `render` feature. Compressed meshes (Draco, meshopt with `EXT_meshopt_compression` and with `KHR_meshopt_compression`,
  quantization) are refused with one line and a checkbox to untick (for quantization, the checkbox is for a file that was not
  written by Blender). Engine properties on mesh data instead of an object are a node error, not silence. The level takes the
  file's default scene, not the first scene (Blender writes all scenes of a `.blend`, and the scene with the instanced collection
  came first).
- Saves: `load_game`, where a section failed before anything was applied, no longer writes `The world is partly
  loaded`. The reply is `loading world/gone failed at section region: …; nothing was applied: fix the cause and load again`, and no call is
  written to the record. When something was applied, the error names the sections whose `missing` had already run
  (`weather (missing from the
  file)`) and the kinds loaded earlier (`already loaded: world/camp`). Such a load restarts the
  record, even if only `missing` ran before the failure. An error from `apply` or `missing` now always means "the world may
  have changed": a section ran and could change the world before it failed. So a load no longer writes `nothing was applied`,
  does not drop the call from the record, restarts the record, and tells the host that the world was rebuilt. The error adds
  `the failed section may have changed the
  world too` (or, when nothing was applied before it, `no section before it was applied, but the failed section may have
  changed the world`). Refusals after which the world must stay untouched moved to a
  new method `SaveSection::check(&self,
  &World, body)` (it accepts by default; `&World` stops it from changing the world). All
  `check` calls of all loaded kinds run after the files are read and before the first `apply`. A refusal gives
  `nothing was applied`, and there is no call in the record, even for the last section of the load (`apply_sections` also calls
  them before `apply`). Its own `check` exists in `region` (the manifest and `plan()` of the content, for a save of version 1
  — the same region generation and hash), in `position` and `vitals` (the body, the numbers, the count and names of the
  charges), and in the body component from `save_component` (the body exists). `vitals` gives the body its `Charges` in one
  `entry` call. `SectionEntry` has a new `check_in`. `ApplyError` has new fields `missing` and `failed_section`, and a
  `changed_anything()` method (tests `a_load_that_fails_part_way_says_what_it_applied_and_records_the_load`,
  `a_section_that_refuses_in_check_applies_nothing_even_late_in_the_load`,
  `a_section_whose_apply_changes_the_world_and_fails_may_have_changed_it`).
- Windows: `cargo clippy --workspace --all-targets -- -D warnings` reports nothing: the test helpers `stand_udp`, which only the
  tests with a real `sh` (unix) need, are built only there.
- D4: `clearance` on the navmesh no longer passes diagonally past a column that an item placed after loading has cut out. The
  diagonal step of `NavRegion::reach` requires both side columns to be open (test
  `a_diagonal_reach_stops_where_a_side_column_is_switched_off`). Developer-menu lines with an error go to the console journal
  through the same limit of 200 lines as the others. The `console` agent gets the Dev scope once, when the plugin is built, not
  before each line. A scope set on it later (`configure_session`) stays.
- Windows: `shards stand stop` stops the stand. Before, it ended the session and left the host process running
  (`signals processes
  on unix only; end pid N yourself`). Windows has no signals, so now the host is asked to exit through
  `quit`. A connection to a port where nobody listens is dropped after 0.5 s, not after two seconds of loopback retries.
  Replies and files no longer contain paths like `\\?\C:\…`. The tests `shards_settings`, the wait in the test `plugins.rs`
  and the scenario `snapshot.scenario` now pass on Windows too.
- World: a body could leave the generated region, slip past its edge, or be thrown off it, and then fall without end. The first
  `shards soak` run found this: a client's body left the western edge of the wilds (`x -65.4` with the edge at `-64`), and the
  game stopped on the invariant `bodies_in_world` at tick 4896. The sandbox's wilds and the template's valley now have a wall
  along the edge (`edge_wall: 4.0`). The manifest hash of the sandbox's wilds with seed 42 is `7bbfdc3b62623fbc`, and of the
  template's valley `36930e9ac5dc441e` (these include the region definition).
- Template: on a co-op client, the `Died` observer counted a trophy a second time (the host forwards combat events for
  effects). Now, as `shards_net::events` requires, it returns at once if the machine does not simulate.
- Snapshots: a load that was refused after preparation (a region of another world, other bodies or levels) returns the world to
  what it was before. The preparation runs again with the snapshot taken before the load, the entities get their old ids, and
  the replay record goes on with no extra inputs. Instead of lines about the region being unloaded and built, the journal gets
  one line: `snapshot of tick N not loaded (…): the world is as it was before the load`. `restore_releasing` behaves the same
  way.
- Levels: the counter of instance numbers (`LevelCounter`) is snapshot state. A level placed after a load gets the number it got
  in the original session, and a change of region does not confuse it with the region's levels.
- Replays: the `level` check is registered with the module and is part of the hash while at least one level is present, not forever
  from the first level on. A snapshot taken before the first level loads exactly, including after a change of region, and a
  record made from it matches. The reference replays are unchanged.
- Replays: the inputs of one tick are written in `AiId` order, not in the order of the request, so one session gives the same
  file with a window and without one. Replays recorded earlier still play as before.
- Replays: playing back a recorded call does not reset the agent session's link to the player (`possess player=N`), and it does
  not let that link replace the recorded body. The call goes from the recorded body, and the link after it stays as it was.
- Levels: reloading a file in which a node has no valid pose (a zero scale from Blender: `S Z 0`) leaves the instance as it was
  and names the node. Before, the old level disappeared, and the instance did not come back until a new `spawn_level`.
- Snapshots: `snapshot_load` refuses before making any change if the game cannot read the value of a saved resource (before,
  this came out only in the replaced world: "loaded …, but the restored state differs"), if the file has `AiId` as a component
  (the engine writes the entity key, not the file; with `force`, such an entry is skipped), or if an entity id is `u32::MAX`.
  The advice "load with force=true" appears in the refusal only when `force` will really help. When saving, a resource that
  cannot be read back goes to `left_out`, as a component does (and `ai_core::state::encode_resources` does not return it).
  `AiId(u32::MAX)` no longer breaks the id counter.
- Actions: a running action picks up the reloaded definition in full: the zone (`observe`), the animation clip and the time up
  to which the zone is shown, not only the phases and windows.
- Animation: a character binds to the animation player again if the old one has gone (a model reload, a scene change), instead of staying
  in the T-pose until the graph changes.
- Animation: after a clip loops, the cues of the end of the previous pass arrive before the cues of the start of the new one.
- Sandbox: a layout added on the fly is built as it is at startup (with its prefabs), and a removed one takes away its geometry
  and landmarks. The characters it created stay.
- Tools: `attack` and `move_to` refuse a target that has no position (an AI-visible entity without a `Transform`), instead of
  answering `ok` and silently dropping the order on the next tick.
- Network: a query cannot be routed to the host. It does not write to the journal and runs locally. `AiTools::set_route` answers
  with an error, and `app.net_route(.., NetRoute::Host)` for a query crashes the app while it is being set up.
- Replays: an agent action during playback interrupts the replay ("… took over"), even if the game does not write a record.
- Network: `assert_add`, `assert_remove` and `assert_reset` refuse on a client (`NetRoute::HostOnly`: `the rules are checked on
  the host: call it there`), instead of going to the host. The host limits a client's calls to the player's scope, and these are
  Dev tools: the host would refuse them, and the client's call would answer `ok`.
- Replays: a replay from a snapshot whose start does not restore exactly diverges from the start tick and names the reason in
  the header (`DIVERGED at tick 70: no details: the start does not restore exactly: the state hash is …`), instead of printing a
  bare tick.
- `shards::testing::run_to` (and both `--verify` and `testing::verify`): a tick at which a rule is violated and a load has
  started (a change of region behind the loading gate) waits for the load to end, instead of exiting with "the game is still
  loading" and code 2.
- Invariants: a tick at which a system crashed in another run is checked, after a snapshot returns to it, like any other tick.
  Before, its rules were skipped while the game still remembered the old crash.
- Tools: a finished deferred step, polled with `step_poll` (`shards act step_poll id=N`), carries `halted`, and `shards` exits
  with code 3 if the step stopped on a crash or a violation. BRP `ai/step_poll` answers through the same tool (with the scope
  check).
- `shards fuzz`: a strike without a target is `light_attack`, not `attack {}`, which used to be refused (`attack` needs
  `target`). Runs of `fuzz` with the same seed now go differently, and the conclusions in the README are written again.
- `--tick-budget-ms`: a value too large (`1e30`), too small (`1e-10`, that is, a zero budget), `inf` or `nan` is a
  command-line error with code 2, not a panic and not a budget of 0 ns.
- Sandbox: `bodies_in_world` counts each beam, ramp and volume as a separate rectangle, not as one common box of the layout.
  Empty space next to the arena's corridor and next to the obstacle course's lane is no longer counted as the world.
  `LayoutDef::footprint` returns a list of rectangles. The reference replays are unchanged.
- Network: a client over UDP no longer sets its tick back when its input arrives more than 30 ticks earlier than needed (after a
  pause or a freeze of the host). Before, `pace` set it to the host's last tick plus the lead, so the new frames got tick
  numbers that the host already held frames for. The host dropped a repeated frame as a duplicate, and a press that the client
  had already taken from `Intent` was lost. The old state was applied after the new one. Now input that arrives more than 30
  ticks early is paced at half speed (`PACE_BRAKE`) until the margin comes back. A client that has moved away from the host's
  state by more than `STARVE_AHEAD` waits for it. The jump goes forward only (when more than 30 ticks behind), and it never goes
  below the current tick.
- Network: a client that has received the world again discards all the `net_event` events it has not yet shown. Before, events
  of the old world with a tick above the new one remained (the host loaded a snapshot of an earlier tick): they were shown for
  the entities of the new world, and since `retrigger_due` runs in order, one such tick also held the events of the new world
  until the client's clock reached it.
- Stand: `step` and `pause`/`resume`/`speed` of one participant share one queue (`StepQueue::take_requests` instead of
  `take_steps` and `take_clock`) and run in the order of the calls. Before, the driver took the clock before the steps, so a `step` followed by a `resume` left the stand paused until the driver's next pass, although `resume` came last.
- Snapshots: a `snapshot_load` that would restore the world inexactly (`the restored state differs …`: state outside entities
  with `AiId` that is not registered in the snapshot) returns the world as it was, together with the tick. The replay record
  goes on, and `WorldRebuilt` is not sent (the world is not sent again to the host's clients). Before, the world stayed loaded,
  the record started from this snapshot and lost everything recorded before, and the tool answered with an error. The new
  function `snapshot::restore_exact`; `restore` still returns the report with `differences` (the start of a replay names the
  divergence from it).
- Network: a player whom the host admitted in the same frame in which the host's tick jumped (a snapshot load) is no longer
  marked "ticks shifted" after it is admitted. Before, the jump was noticed only in `send_rebases`, after `admit`, and the first
  world sent again (`net_resync`, desync, a new body) went with `carry_inputs = false` without a reason: the player's input
  started over, and the client's clock went back. The jump is now noticed at the start of receiving a frame (`note_ticks`) and
  before the worlds are sent.

- Network: a view whose reference limit (64 entities per view) no longer applies gives the client the world again once
  (`the host's world again (view complete)`). Before, the keeper's references, which the client read as
  `Entity::PLACEHOLDER`, did not change in the deltas. The first hash check after the limit was lifted found a desync, and it
  cost the client a fresh world, with an error in the journal. The hash is not sent in the delta of this transition.

- Network: a region that a guest's call rebuilt (`regen`, `region_load` with `guest_scope: Dev`) is sent to the clients again,
  as a whole world. Before, the `WorldRebuilt` event was lost: the observer did not find `NetHost`, which had been taken out
  while tasks ran, and the clients stayed on the old world until a desync. The event now waits in the `PendingRebuild`
  resource, and the state delta is not sent in the tick of the rebuild (otherwise the client got the resources of the new
  world, and the fresh world found the region already built and diverged on `static_colliders`).

- Network: the host limits the work a peer does in one poll. For a peer that is not playing, it parses no more than 160 KB
  (`JOINING_POLL_BYTES`). For a player, no more than 4 MB (`PLAYER_POLL_BYTES`). The rest it drops unread (`unread` in
  `net_status`, one line in the journal per second). Before, the limit applied to each message, not to a poll: one poll could
  give up to 24 MB of JSON, and after the first `Hello` was refused the host still parsed the other messages from the same
  peer. Now the host reads nothing that comes from a released peer.
- Network: a peer that has not got past the join stage within `6 × timeout_secs` (a minute by default, `HANDSHAKE_TIMEOUTS`)
  is released with `Refused`. A peer that has left is forgotten once it has acknowledged nothing sent to it for 10 seconds in a row
  (but no later than 5 minutes). Before, a peer without `Hello` (one datagram with a message was enough) lived on as long as it
  sent acknowledgements, did not respond to `net_kick`, and took one of the 512 entries of the UDP address table. A peer that
  did not acknowledge a refusal stayed forever. The time limit does not apply to a game that holds the player itself (`join_policy`
  answers "not now"). `net_kick` takes `peer` (the number from `net_status`) instead of `player`. The entry "joining a session
  with no time limit" is removed from the known limitations.

- Network: a world bigger than 16 MB (what the channel takes as one message) is not captured or encoded again on each tick.
  `rebase_fits` answered "fits" for 16–32 MB, sending was refused for good (`TooLarge`), and the host tried again on every tick.
  A size above 32 MB was remembered and held back any later, smaller world. Now it is a final refusal: one line in the host's
  journal, the player leaves with a reason, and the remembered size is reset.

- Snapshots: a load that was refused does not lose the old journal lines. The lines of the attempt (the region unloaded and
  built again, the navmesh) were removed from the end. But in a full ring of 2048 lines, each one pushed an old line out from
  the start, and those did not come back (an agent whose cursor was in them got `missed N`). `AiJournal::mark` and `rewind`
  return the ring as it was. Line numbers are still not handed out again.
- Network: a client over UDP that ran ahead of the host by more than `STARVE_AHEAD` (the host paused, frozen, or stepped from
  the CLI) no longer hangs forever. It waited for a state while the clock was stopped (`Time<Virtual>` paused), but only a tick
  applies the state (`apply_state` in `FixedFirst`). The delta that had arrived lay in `pending`, `newest_host_tick` did not
  grow, and only a world sent again could end the wait. A delta that has arrived but is not yet applied now counts as news
  (`newest_seen`) in `pace`, in the stand's `clock_ticks`, and in the target of the forward jump.

- Network: a message that the channel of a leaving player did not accept (the kick's `Bye`, `CharacterSave`, `SessionEnd`) is
  logged in the host's journal at once (`p1: the kick not sent: reliable queue is full …`, at most one line per second for each
  player and kind of message). Before, such a refusal only raised `unsent`, and the channel check reads only connected players
  and runs once a second. So the character did not arrive, the client did not learn that the session had ended, and the host
  stayed silent.

- Documentation: the exit codes of `--exit-with-host` (0, 1, 2) are described in the flag's help and in
  `EngineConfig::exit_with_host`, not only as "the client exits when the host has ended the session". The README and CHANGELOG
  say that 2 also applies to `--host` (port in use), and that among the network flags only `--stand-udp` exits with code 2.

- Documentation: the README and `docs/engine-progress.md` have no overall test count (1078 and 1109 disagreed with each other
  and with the tables by crate). One document keeps the counts by crate, `docs/engine-progress.md`, "Tests". The rows for
  `ai_core`, `shards_net`, `shards` and `sandbox` were recounted with `cargo test -p …`. The README kept the column "what they
  check".

- Networking: the `view complete` world no longer repeats every 4 ticks. It wiped the view memory (`peer.view`, the hysteresis of the interest radius) that the link limit had been lifted from. On the next send the limit fell again, then was lifted again, and so on without a pause, while the player stood on the ring 60–70 m from the list's pillars (the client was not playable, and the host captured and encoded the world on every cycle). The view memory now stays through such a world. A region change and the other causes still wipe it, as before. The world itself goes out no more than once per `DESYNC_RESYNC_COOLDOWN_SECS`. Until the pause ends, the view counts as limited, and the hash is not checked.

- Networking: after the world is rebuilt (a region change, a snapshot load, a guest's `regen`), no deltas go to the player until the world has been sent to them again, not just for one tick. Before, the block was lifted when `run_tasks` took the event. While the world waited for room in the channel (or while `capture` failed), the deltas of the new world went out in the epoch of the old one. The client accepted the record of the new region. The world that arrived later found the region already built, failed the `static_colliders` check, and a player on a poor channel left the session because of the region change. The flag sits on the peer (`rebuild_pending`) and is cleared when the world is sent.

- Networking: a peer that has left is forgotten once it has confirmed nothing sent to it for 10 seconds in a row, not 10 seconds after it left (the ceiling is 5 minutes). Before, a player on a slow channel who had been kicked, or whose session had ended while a world of several megabytes was still on its way to them, lost `Bye`, `CharacterSave` and `SessionEnd` (they wait in the queue behind the world). The host forgot them while they were still confirming, and the client's game wrote `lost the host` instead of the reason. The `Link::reliable_acked` counter counts the confirmed messages and the blob chunks.

- Networking: a stranger costs the host less. If a peer that has not yet sent `Hello` sends a first message that is not a `Hello` (or a `Bye`), or one that does not decode, the host refuses it with `a join starts with a hello`. It no longer decodes 160 KB on every poll for a minute. Peers that are not playing yet share 4 × 80 KB per poll (`SHARED_JOINING_POLL_BYTES`). The poll starts with the peer that was skipped last time. Otherwise it goes in `PeerId` order: in the stand that is the order of creation, and over UDP it is the order of the address hash. Their number is limited (`max_strangers`, `2 × max_players + 4`). A new one pushes out, in this order: a refused peer that left earlier than the others, then the oldest silent peer (except up to `max_players` of the oldest of those who may still have a first message waiting, while they are under 5 s old), and last a peer that left earlier than the others. There is always someone to push out. The `flood` line is one line for all peers, not one key per address (`LineLimits` grew by one entry per stranger). Not done: the `calls_queued` check before the `Call` arguments are parsed for an admitted guest (their parsing is limited to 4 MB per poll).

- Documentation: the help for `--exit-with-host` and `EngineConfig::exit_with_host` again speaks about the client ("the client exits when its session has ended"): exit codes 0 and 1, and code 2 also for `--host` with a busy port. It says directly that the host does not exit on `net_end`. The previous edit said "the game exits", and a script that started `--host … --exit-with-host` would wait for a process that never ends.

- Documentation: the per-crate test counts are kept in one "Tests" section of `docs/engine-progress.md` (recounted from one run). The E7 rows and the summary of numbers do not repeat them. The README and the template's README say the template has 24 tests (and one `#[ignore]`), and the co-op list includes `net_predict.rs`.

- Networking: a new address does not push out an incoming peer until that peer has been read, and the number of protected peers never exceeds the number of player slots. Eviction used to happen when a datagram arrived, before the poll. It took the oldest peer without `Hello` first: an honest incoming peer whose `Hello` the channel had already acknowledged (it is not sent again). With a table full of refused strangers, one more datagram from a new address in the same tick lost that peer, and a `Hello` split into parts was never assembled. Now the refused go first, then a peer without `Hello`, except up to `max_players` of the oldest of those who may have a first message waiting (unread, or a blob from number 0; 5 seconds since creation). Then a peer that has left, even an admitted one. A peer that comes back from new ports and acknowledges one chunk every 10 seconds does not hold a table slot for 5 minutes. A message with a gap in front (number 1 without 0), which a stranger could have used to occupy an entry, is not protected. Peers that left go in the order in which `flush` saw them leave. The one disconnected between ticks goes last. Before, it went first and lost `Bye` and `CharacterSave`. The host writes the eviction of an admitted peer to the journal (`forgot pN (gone) …`). Known limit: someone who sends without a pause from many real addresses can delay, or even break, the join of a player whose `Hello` comes in parts. Without a handshake that holds no state on the host, this cannot be closed.

- Networking: the refusal of a first message that does not decode names both causes (`your first message does not decode (…); a
  join starts with a hello, and both ends need the same build`). A `Hello` from another build can also fail to decode.

- Networking: rebuilding the world (a region change, a snapshot load) clears the "view limited" flag and the timer that runs after the `view complete` world. Both belonged to a world that no longer exists, and they produced an extra world 1-5 seconds after the region's world. Any full world other than `view complete` (a new body, a desync, `net_resync`) also clears the "view limited" flag. The timer stays, because it holds the blinking limit.


### Changed

- **The documentation is in English; the README is a project page, the manual is `docs/manual.md`.** `README.md` is now a short project page: what the project is, why it exists, a fight driven from the terminal (a GIF, the agent's real output and screenshots in `docs/media/`), quick start, making a game step by step, how it is built, status and links. The old README, the full manual, moved to `docs/manual.md`. Links from `AGENTS.md`, the template's README, documents, tests and code comments now point there. The manual, the spec, the progress file, this changelog, `AGENTS.md`, `CLAUDE.md` and the template's README were translated from Russian, and code comments that named a Russian section name the English one. The language rule in `AGENTS.md` says so. The template needs the engine of phases C–E (saves, co-op, screens, texts). The README, the manual, the template's README and the comment in its `Cargo.toml` name `0.4.0`, not `0.3.0`.
- **Session protocol 2 (`shards_net`).** The channel protocol version (`wire::PROTO`) and the session version (`msg::PROTO`) are 2: joining uses a knock and a cookie, the header flags `COOKIE` and `KNOCK` are new, `Hello` has the fields `character_parts` and `character_bytes`, and there are new messages `CharacterPart`, `CharacterAsk`, `CharacterNotSent`, `PeerStage::Character`, `ClientStage::Knocking`. Games from the previous build do not play with this one. The host answers a protocol 1 game with a refusal on its protocol (`the host runs a newer build: wire protocol 2 …;
  run the same build`), and a game of this build does not hear the reply to its knock from the previous host. `MAX_CHARACTER_BYTES` is 1 MB (it was 64 KB; a character in `Hello` itself can be up to `MAX_INLINE_CHARACTER`, 64 KB). `SaveRouting::route` gets the slot: `fn(&mut World, SaveKind, Option<Entity>, &str, &str)`.
- **NPC routes take the body, not a radius (`shards_npc`).** `NavPaths::find` is `fn(&World, Vec3, Vec3, &CharacterParams) ->
  Option<Vec<Vec3>>`. `find_path`, `navmesh_path`, `local_grid_path`, `routes_for` and `routes_at` also take `&CharacterParams`,
  and `straight_clear` takes it instead of the radius and the step height. A game's own route provider changes the signature, and it can read
  the height and the other numbers of the body. `NavRegion::serving` takes the lowest of the classes with the same radius. New: `NavRegion::lack` and
  `NavRegion::unserved`.
- **`graphics.render_scale` is the 3D internal resolution, not the window scale.** Before, the setting set the window's `scale_factor_override`, so the UI in the window got bigger or smaller too. Now the window and the UI are at native pixels, and the 3D world is drawn at `render_scale` x the window's pixels (see "Added"). `shards_settings::apply_graphics` no longer touches the window scale. A value of 1.0 still changes nothing. A settings file with another value gives a different picture, not a different window.
- **Key rebinding is in the settings.** `rebind` writes the difference from the defaults to `controls.bindings` and
  `controls.hold_or_toggle` (written a second later). It no longer writes `input.ron`, and it has no `save` argument.
  `InputSettings` remains only to locate the old file (`app_name`, `file`, `import`, `config_dir`; there is no `persist` field).
  The move is switched on by the game (`InputSettings::for_game("<game folder>")`). It is off by default, so a game does not take someone else's shared
  `~/.config/shards/input.ron`. It only moves into the platform's settings folder. If the settings have no controls yet,
  `input.ron` is moved into the settings and renamed to `input.ron.imported`, and only after `settings.toml`
  is written. `InputMap::save_overrides` is removed. A game that set `InputSettings.app_name` needs `SettingsAppId`.
- `settings_set` for `controls` checks the same things as `rebind`: an unknown action or context, and an input that another action reads. A record that is not read stays in the file, and it is not erased at the next save. Changing the controls does not rebuild the map from the defaults. Contexts and actions the game added stay. It judges the map that the game will build. An action the game adds takes its input the same way as with `rebind`, and a record for it is refused with the words "controls the game adds itself", not "no such context". Resetting a record to its default (`reset=true`, a table without a record) is also refused if the default would give the action an input held by another action. The refusal names a path open to the caller: freeing the input with `settings_set` itself (`rebind … force=true` only for the developer, since `rebind` itself has scope Dev). An input that holds controls the game added is not moved by `settings_set`, and the refusal asks for another one. `rebind` and `input_bindings` right after `settings_set` (in a scenario, in a test, two requests in one frame) see its result, not the map the settings have already moved away from, and `rebind` no longer erases such a setting. A context name with a dot is a content error: `context.action` cannot be split at the dot.
- `shards_motion` and `shards_action` depend on `shards_settings`. `MotionPlugin` adds `ShardsSettingsPlugin` itself if it is missing. The `render` feature of `shards_motion` enables `bevy_post_process`.

- The `NetHooks` hooks changed. `capture_character` gets `&mut World` (the host remembers what it captured). `on_character_save` gets the host's tick and the reason. `on_session_end` returns the world's slot. `join_character` is new. The world's `load_game` calls `WorldRebuilt`. A network client without `--character` plays the slot named after the player, not `slot1`. In the module list of `ai/status`, `coop` is listed.
- `observe` prints the module sections after `LANDMARKS` (`AiAppExt::register_ai_observe_section`; the first is `UI`). In the input map, `gameplay` has a new action `pause` (Escape, Start). In the `menu` context, Escape is bound to `back`. `SettingsRegistry::value` reads one setting as text. In the module list of `ai/status`, `ui` is listed. The content of the sandbox and the template added screens, so the reference replays diverge on `replay check content` and are re-recorded (`record-replays.yml`).
- Audio (D3): `JournalKind` has a new kind `Local` (`[local] …`). It is for what only this machine shows (music changes). The host does not send it to its clients, and it is not a game event. A `match` on `JournalKind` in a game gets one more branch. `AudioSettings` has a field `ui` (default 1.0), and `volume("ui")`. In `observe`, there is a `MUSIC` section. In the module list of `ai/status`, `audio` is listed. The sandbox and the template turn on the `audio` feature by default. The content of both games added sounds, music and a mixer (the sandbox: `ok: 94
  definitions in 29 files`), so the reference replays are re-recorded again. The sandbox snapshot carries one more resource (`Music`).
- D4: `RoutePlanner` has a new field `reach`. The planner that the game installs itself sets it. `Hazard` has a new variant `Closed { by }` (a `match` on `Hazard` gets a branch). The `path_to` reply got a field `provider`. In a region, the route goes along the navmesh (longer than 80 m without `max`; a navmesh error is `navmesh: …`). `worldgen_report` now also accepts `seeds` as a number (before, `seeds=7` given as a string in `shards act` gave a type error). In the module list of `ai/status`, `dev` is listed. The sandbox content added menu items (`ok: 98 definitions in 30 files`), so its reference replays are re-recorded.
- E10.1: the umbrella crate re-exports `shards::look`. Its `render` feature enables `shards_look/render`. In the module list of `ai/status`, `look` is listed. `shards_ui::def::parse_color` is now a re-export of `shards_content::parse_color` (the path and the parsing are as before). In the sandbox, the enum `gameplay::Look` (the kind of box: `Stone`, `Floor`, `Wall`, `Sand`) is renamed to `Surface`, so that it is not confused with the engine's `Look` component. The field `look:` in `*.layout.ron` keeps its name. The content of the sandbox and the template added looks and `Look` in prefabs, and the content database hash changed. So the reference replays of both games are re-recorded at the top of the branch chain (E10.1–E10.3, together with E10.6, on Windows and on Linux; see E10.6 below).
- The content of the sandbox and the template added foliage and its kinds (E10.2; sandbox — `ok: 117 definitions in 33 files`, template —
  `ok: 67 definitions in 27 files`). The content hash is different, and the reference replays are re-recorded (at the top of the branch chain, together
  with E10.1, E10.3 and E10.6).
- E10.3: the umbrella crate re-exports `shards::fx`. Its `render` feature enables `shards_fx/render`. In the module list of `ai/status`, `fx` is listed (after `scatter`). While decals are on (the default), the telegraph gizmos do not draw the hatching or the growing ring; the decal draws them. `telegraph_decals on=false` brings back the old picture. The developer menu has 37 items (with the E10.2 foliage). The content of the sandbox and the template added effects and a table (with foliage: sandbox — `ok: 123 definitions in 35 files`, template — `ok: 71 definitions in 29 files`). The content database hash changed, so the reference replays of both games are re-recorded at the top of the branch chain (together with E10.1, E10.2 and E10.6).
- E10.6: the math goes through `libm` (see "Added"), so `f32` numbers in the game may differ from the old ones in the last bits:
  the reference replays of the sandbox and the template, recorded with glibc's math, diverge on any OS, and are re-recorded
  (`record_reference_replays`, `record-replays.yml`). The sandbox and template replays in this tree were re-recorded (after E10.1–E10.3 and
  E10.6 together). Windows and Linux recorded them byte for byte identically (see "Added"). A game that called `x.sin()` and
  similar functions in the simulation gets the error `clippy::disallowed_methods` if it copies the template's `clippy.toml`:
  replace them with `ops::sin(x)` (`ops` is in `shards::prelude`).
- **World save format: the `region` section is version 2** (with the region manifest; the `wilds` 8 × 8 manifest section is about
  3 KB, and the whole world file is 3392 bytes). Files with version 1 are read by the migration `region` 1 → 2 (`manifest: null`). Such a region, as before, is generated again and checked against
  the hash. The next save writes version 2. A build older than this one does not read or overwrite a save with `region` v2
  (`saved by a newer version of the game (section region v2 > v1)`). Snapshots, replays and reference replays do not change.
- `SaveRegistry::add`: a section from another crate that replaces a section with the same name starts its own migration history. The migrations of the replaced section are removed with it. (A game that replaces `region` registers the migrations of its own versions after it. Otherwise the engine's migration `region` 1 → 2 would make its version 1 section fail to load.)
- E6.2: `npc_tuning` no longer knows `avoid_time` and `avoid_margin` (content that still has them is an error ``Unexpected field named
  `avoid_time` in `NpcTuning`, expected one of …``). Their place is taken by `avoidance`. `NavAgent` has new fields (and `planned_from`).
  NPCs in a crowd move differently, and those who step off the straight line plan their route again. The reference replays of the sandbox (all seven) and of the template
  (`smoke`, `valley`) were re-recorded on this branch. The NPCs' `NavAgent` is different, and `avoidance` in `npc_tuning` changes the content hash.
- E6.1: body classes change the sandbox content (`wilds.nav.ron` with the class `large`), and the content hash is different again. They add no new components that diverge. The NPCs in the replays are skeletons, and they use the class `agent`, whose navmesh and files are as before. The region's journal got a new line `navmesh (body large): …`.

## [0.3.0] — phase B: world, enemies and snapshots (unreleased)

The engine got a world and enemies. Levels are assembled in Blender (`shards_level`). Regions are generated from a seed and checked
(`shards_world`). Enemies walk on the tile navmesh and share attacks by tokens (`shards_npc`). The state of any tick is saved to a
file and can be loaded back (snapshots). The sandbox got the "wilds" region and NPCs on the engine. The template got a valley with raiders.

The version in `[workspace.package]` is `0.2.0` for now. The maintainer raises it at release (section "Releasing a version (for the maintainer)"). A minor version
changes the API. `0.3.x` does not accept snapshots, or replays with a snapshot, recorded on `0.2.x` (`force` skips the check). The reference
`tests/replays/snapshot.json` is re-recorded at release.

### Added

**Levels from Blender (E4.2, `shards_level`)**

- glTF 2.0 with custom properties becomes entities: colliders and sensors (`collider`, `volume`), markers (`spawn`, `socket`,
  `navlink`, `trigger`, `interactable`, `camp`, `landmark`, `custom:<name>`), prefabs from the content (`prefab`) and any game components
  by name (JSON in the property value, including as text). Nodes are spawned as top-level entities with a world
  `Transform`, in file order: the same `AiId` and `ColliderId` in every run. Only the file's JSON is read
  (crate `gltf =1.4.1`), with no window and no GPU.
- Content kind `level` (`file`, `required_markers`, `at`): `content check` and `--check-content` read the file. An error names the content file, the level, the node (`Ruin/Column`) and the property, and suggests the nearest name.
- Tools `level_info` (player), `spawn_level`, `level_reload` (developer). Hot reload replaces the instances with an action that is recorded. The replay check `level` and the snapshot check `levels` know the spawned levels and the file digests.
- `shards::TileLevelsPlugin` (in `EnginePlugins`): levels as tile geometry (`geometry: Level(path: …)`). The files load behind the loading gate. A tile's marker nodes are also `Marker`.

**World (E5, `shards_world`)**

- Content kinds `tile`, `socket_rules`, `poi`, `region`, `tile_map`, `room`, `dungeon`. Errors name the file, the field and the nearest
  id. A region has: a size, a tile size, a generator version, fixed cells, limits, points of interest, roads, checks,
  a fallback map, `active_radius`, and `origin` (the corner of the region in the world, default (0, 0)).
- The generator works in stages (`Points`, `Roads`, `Cells`, `Fill`, `Secondary`, `Decorate`, `Checks`). The engine's steps work on data.
  A game adds a step (`add_worldgen_step`), replaces a stage (`replace_worldgen_stage`), or adds a check
  (`add_worldgen_check`). The socket solver (rotations, heights, fixed cells, limits, propagation, a bounded
  backtrack), checks (`spawn`, `reachable`, `slope`, `distance`, `isolated_roads`, and the game's own), attempts and the fallback map. Generation is
  a pure function of the seed, the version, the attempts and the content. It draws its randomness from `DetRng`.
- `RegionManifest` (`shards-region/1`): a canonical record, stable byte for byte, with a hash. Loading a manifest does not run
  the generator. Dungeons are built from rooms, with a guaranteed path from the entrance to the goal. Hand-made rooms are fixed nodes.
- Batch check without a window: `<game> --worldgen-report REGION --seeds 1..1000` (exit code 0 when every seed passes),
  `shards worldgen --region R --seeds S --report`, and the tool `worldgen_report`.
- Loading a region: `regen`, `region_load`, `goto_tile`, `region_info`, `world_overlay` (window), `start_region`,
  `on_region_loaded`. The change goes through `EngineState::RegionLoading` within one call. It does not advance a single tick, and it is recorded in the replay. `ActiveZone`/`Dormant`: NPCs farther than the active radius from all players do not think or move. The tiles' picture streams by distance (only with a window). `observe` gets the `TILE` and `POI` lines, `ai/status` gets the `world` field, and `step --until` gets the condition `near_poi`.

**Navigation and enemies (E6, `shards_npc`)**

- Behaviour: a state machine in the content (`behavior`), the engine's state hooks and conditions (`idle`, `patrol`, `chase`,
  `return_home`, `wait_token`, `attack`, `flee`, `hold`, `investigate`; `target_seen`, `target_within`, `target_in_leash`,
  `at_home`, `has_token`, …), and the game's own (`add_npc_condition`, `add_npc_state`). Perception (`perception`): sight by physics rays,
  hearing (`Noise`), and memory. The combat director (`director`): attack tokens by the number of players, an encounter budget, allowed combinations,
  the roles of those who wait (a circle, a sideways step, holding back). The attack planner (`attack_plan`): weights, distance windows, cooldowns, bans on sequences,
  rest. Leash: `Home`, `Patrol`. NPCs drive the body through `Intent`. All their state is in their components, so it is
  part of the replay and the snapshot.
- Navmesh by tile: baked in advance (`<game> --bake-nav [tiles|all]`, tool `nav_bake`, files `assets/nav/*.navtile.ron`,
  `.navroom.ron`) with the same movement rule as `path_to` and the controller. The file's hash is checked against the tile, the prefabs, the level file and the settings (with the kind `nav`, a stale file is a content error). When a region loads, the tiles are stitched along the open sockets and `navlink`, and solid bodies that the baking did not see are cut out. The route runs over the tile graph, along the corridor grid, with string pulling. No window is needed, and it is deterministic.
- Navigation agents: `NavPaths` (a replacement of the route source), a target in sight, avoidance, the leash. `move_to path=true` walks along the navmesh in a region. Tools `nav_path`, `nav_info`, `npc_state`, `npc_set_state`, `nav_overlay` (window); conditions `npc_attacking`,
  `npcs_calm`, `npc_beyond_leash`, `npc_detour`. `observe` shows a line with the NPC's state, and the journal shows the transitions and tokens.

**Snapshots (E11.4, `ai_core`)**

- `snapshot_save` and `snapshot_load` (Dev, `control`; files only in `snapshots/`) and `shards save-snapshot` /
  `load-snapshot`: save the simulation state at any tick to a `shards-snapshot/1` file, and load it back. After that, `step` runs the same way as the original session. The snapshot holds the entities with `AiId` and their reflected components, the tick, the `AiId` counter with its tombstones, and the resources registered with
  `add_snapshot_resource` (`ColliderIds`, `RegionBuild`). Caches are rebuilt (`add_snapshot_rebuild`:
  `PhysicsWorld`, the region's navmesh). What the world was built around is checked by a hash (`add_snapshot_check`: content,
  colliders without `AiId`, levels). Loading a snapshot with a different seed, version, content or unknown component returns an error before anything changes (`force` skips the check). After loading, the world is saved again and compared with the file.
- `add_snapshot_prepare(name, covers, fn)`: a module rebuilds what it makes from the saved data before the checks run. The snapshot's region is rebuilt from its manifest, with the same `ColliderId`s and level instance numbers, in a game with another region or with none. A snapshot without a region unloads the region.
- `shards-replay/3` replays start from a snapshot (`start`). A recording made after `snapshot_load` starts from it, and playback and `--verify` load it before the first tick. `shards-replay/2` replays play back as before.
- For modules: `add_snapshot_resource`, `add_snapshot_check`, `add_snapshot_rebuild`, `add_snapshot_prepare`, `add_snapshot_entity_key`.

**Umbrella, AI layer, movement**

- `shards::level`, `shards::world`, `shards::npc` and their preludes. `EnginePlugins` includes `LevelPlugin`, `WorldPlugin`, `TileLevelsPlugin` and `NpcPlugin` (`engine.modules` in `ai/status` lists `level`, `world`, `npc`). `shards::run`: the flags `--worldgen-report REGION [--seeds S]` and `--bake-nav [TILES]`. `--verify` plays a replay from a snapshot (exit code 2 if the build does not accept the snapshot). `shards::testing::{run_to, verify}`.
- `ai_core`: `register_ai_observe_lines` (a module adds lines under `YOU`: `TILE`, `POI`), `add_snapshot_*`. CLI: `save-snapshot`, `load-snapshot`, `worldgen`.
- `shards_motion`: the movement rule `step_onto` (and `hop_allowed`) is public. The navmesh is baked with it, and `path_to` plans with it. `RoutePlanner`: `move_to path=true` asks the region's navmesh.

**The sandbox, the template and CI**

- The sandbox: the monsters are engine NPCs. Their behaviour, perception, director and attack plans are content in `content/npc/`. Skeletons have a 30 m leash, the guard a 26 m leash and its own director. The arena has a third skeleton (`e14`). The "wilds" area is the `wilds` region: 8 × 8 tiles of 16 m each, south of the arena (`regen region=wilds seed=42`). It has nine tiles, one of them from a level file (`assets/tiles/ruin.gltf`), skeleton camps and a lair, and baked navigation in `assets/nav/`. The scenarios `npc_director`, `npc_navmesh`, `region`, `snapshot` and their reference replays (`tests/replays/`); the `three_attacking` condition; snapshots of the fight, the region and the NPCs in the tests.
- The template: the `valley` region (one tile from a level file, a baked navmesh), raiders that are engine NPCs with no game code, the `valley` scenario, the `valley` replay, `tests/game/region.rs`.
- CI: the sandbox's `--check-content` reads the level files and the navigation. `--worldgen-report wilds --seeds 1..1000` and `valley`. Scenarios against a live game run forward, then back, in the same game.

### Changed

- `RegionLoading` is now a working state: every region change goes through it.
- Sandbox: the skeletons' and the guard's own brains are replaced with engine NPCs. The guard rests between attacks while it has nothing to do (not while it is stunned). Scenarios take monsters out of combat with the `calm` state (`npc_set_state`), not with the fields of the old brains. The reference replays are recorded again.
- `set` replaces the whole list. It no longer writes over its first elements.

### Fixed

- World: dungeons place every hand-authored room (the minimum path is raised to their number). A hand-authored room with one door is a content error. Reloaded level tiles get their region tags inside the reload, and the region owns all the instances. A region change is refused before the old region leaves if a prefab is missing, and a change started from `OnEnter(InGame)` no longer re-enters the state. `ground_below` gets out of the solid body the ray started in. The error for a tile file path outside the assets folder names the tile. The solver writes a change log instead of a copy of the grid (a 64 × 64 grid fits in 8 MiB). A broken tile file shows in the journal on load. The batch report says why a seed failed, when the attempt itself is not to blame.
- NPC: an attack with no hits holds its token until it ends. A token for a forgotten target is returned. Tokens above the reduced number of players are returned at once. An NPC under an agent's order drops out of the fight. A sound that reaches an NPC does not move the target the NPC sees.
- Navmesh: carving depends on the solid bodies standing there now, not on the order they appeared in. The navmesh is rebuilt after a snapshot loads. Ledges and drops at a tile's edge are stitched together. An NPC does not walk to a target it sees across a drop, and without a route it stands still. `navlink` is only for moves the body makes. The bake's staleness check sees prefabs that extend geometry prefabs. `move_to path=true` does not guide a walker along the navmesh when it does not know that walker's moves.

### Known limitations

- Only the tiles' visuals are streamed. The region's geometry for physics is built in full.
- The navmesh is baked for one body. `path_to` and `clearance` are still physics queries (over the navmesh: `nav_path` and `move_to path=true`).
- Levels are tested on a glTF made by a test generator, not on a Blender export. Nothing checks that a file's sockets match the tile's sockets in RON.
- When a replay runs `content_apply`, which adds a tile's level file, and `regen` in the same tick, they do not load the file in time (fixed in `[Unreleased]`).

## [0.2.0] — phase A: combat prototype

The engine became a set of crates. A separate repository builds a game on it without touching the engine's code. A game depends on one crate, `shards`. The sandbox (`crates/sandbox`) is built the same way as a game, and `template/` holds a minimal game.

### Added

**Umbrella crate `shards` (E1)**

- `shards::EnginePlugins`: the whole engine from one `EngineConfig`: `DefaultPlugins`, the AI layer, the core, content, movement and combat. The mode without a window uses the same plugins minus the window, the GPU and sound. The sandbox is no longer built from `MinimalPlugins`.
- `shards::run`: the standard `main` of a game: `--headless --seed --port --port-file --no-remote --paused --hz
  --replay --verify [--windowed] --check-content`. `kill` and a closed terminal stop the game through `AppExit`, and the token and port file are removed. `--verify --windowed` plays a replay in a window (under `xvfb-run -a`) and gives the same report as without a window: rendering, the camera and the input map do not change the game.
- `shards::testing`: test apps without a window, loaded and paused at tick 0 (`test_app`, `call`, `ok`). `shards::prelude`. It re-exports `bevy`, `serde`, `schemars` and `serde_json`, so a game does not need its own dependencies on them.
- `ai/status` shows the engine version, the enabled modules and the state: `engine: {version, modules, state}`.

**`shards_core` (E1)**

- `EngineConfig`, the `EngineState` states (`Loading` → `MainMenu` | `InGame`, `RegionLoading`) and `PauseState`, and `LoadingGate`. While something loads, `ai_core::SimGate` lets no tick run, so loading does not move the ticks. Games create the world in `OnEnter(EngineState::InGame)`.
- `Intent`, `Press`, `Order`, `Target` and `input_names` are the only input of a character. The keyboard, the gamepad, the agent's tools and replays write to it. `PlayerId`, `LocalPlayer` and `NetRole` are groundwork for networking (phase C).

**`shards_content` (E4)**

- Content base: typed definitions in RON (`content/**/*.<kind>.ron`). It checks the schema, duplicate ids and references between definitions, and reports errors with the file and line. Hot reload (recorded in the replay). `--check-content` for CI. Prefabs (`*.prefab.ron`, `extends`, field overrides at spawn).
- Tools: `content_list`, `content_get`, `content_check`, `content_reload`, `content_apply`, `spawn_prefab`.

**`shards_motion` (E2)**

- A physics layer built on parry3d (behind the engine API): colliders with deterministic `ColliderId`s, layers, sensors (water, updrafts, triggers), and the queries `raycast`, `shapecast`, `overlaps` and `volumes_at`.
- A kinematic character controller: walking, running, sprinting, slopes, steps, jumping, climbing onto a ledge, swimming and gliding. The numbers are content (`*.locomotion.ron`).
- An input map: the keyboard, mouse and gamepad become `Intent`. It has contexts, press, hold, toggle, and key rebinding saved in the user's settings folder. A third-person camera (orbit, collision with walls, lock-on, aim assist, shake), and a collider overlay.
- Agent tools: `move_to` (with `path`), `move`, `stop`, `jump`, `sprint`, `glide`, the queries `clearance` and `path_to`, and `input_bindings` and `rebind`. `physics_overlay` in the window. `path_to` is a local grid A* (0.5 m) over what the controller can do: steps, climbing over, swimming, descent. The navmesh is in E6. Visibility in `observe` is worked out with physics rays.

**`shards_action` (E3)**

- An action is a timeline from content (`*.action.ron`): hit windows, invulnerability, block and perfect block, hyperarmour, cancels and combos, displacement, and cost. A moveset (`*.moveset.ron`) maps input to actions. One action machine drives the player, enemies and bosses.
- Hits go through the physics layer. A damage pipeline (immortality, invulnerability, parry, block, armour, game hooks). Fighter resources (`Health`, `Stamina`, `Poise`, `Armor`, `Team`, `Charges`), statuses (`*.status.ron`), telegraphs and hit zones.
- Tools: `light_attack`, `heavy_attack`, `block`, `parry`, `dodge`, `use_technique`, `use_item`, `lock_on`, `attack` (player); `god`, `heal`, `stagger`, `combat_overlay` (developer). The same `can_start` answers the machine and the tools: a refusal names the reason and the time. Under the `YOU` line in `observe`, a hint says where to leave an attack's zone.
- Window side (feature `render`): an animation bridge (the `animset` content kind: glTF clips, which clip goes to which state and action, blending, `AnimationCue` markers for effects and sound; gameplay leads, animation follows). Telegraphs on the ground with symbols (danger reads without colour). The fight overlay (hitboxes, hurtboxes, guard arcs, i-frames, F2). Markers at the screen edge for attacks from off-screen (`threat_indicators`). `SoundCue` for a future audio module.

**The AI layer and scenarios**

- `shards_script` (no Bevy): the scenario format and the parsing of `--until` conditions, shared by the CLI and the tests. `shards scenario FILE...` runs the files against a running game. `ai_core::scenario::run_file` runs them in-process, in `cargo test`.
- `ai_core`: `SimGate` (holds the simulation while loading), `register_ai_advice` (hints under the `YOU` line in `observe`), `add_ai_status`.
- `shards content check` (CLI) and `shards wait`, which waits for loading to finish.

**The sandbox and documentation**

- The sandbox is built from engine modules: an arena with a combat prototype (the player, skeletons, the Frost Warden guard with four attacks and two phases), and an obstacle course (steps, a ledge, a pool, a drop with a glider and an updraft, a target platform). The world, the characters and the numbers are content (`crates/sandbox/assets/content/`).
- Reference scenarios `obstacle_course` and `combat_prototype` (played by an agent with the `player` scope). Their sessions are the reference replays in `tests/replays/`. The `cargo test` test checks them with `--verify`, and CI also checks them in a window under Xvfb.
- The README was rewritten as an engine guide: a quick start, "Make a game on Shards, step by step", the architecture and the tick order, and a section for each module. The new `docs/engine-progress.md` checks the engine against the spec, item by item, E1–E11. `AGENTS.md` describes the tools and the rules for changing the engine.

**Versions, template and CI (E1)**

- `CHANGELOG.md`, version `0.2.0` for all crates, the rules for depending on the engine by its git tag.
- `template/` is a minimal game that uses only `shards`: a character, an enemy, an arena, the numbers in content, a scenario and tests. It is a separate workspace with a single dependency, `shards`.
- CI on GitHub Actions: tests, clippy, the build and tests of the template, scenarios against a running game, and `--verify` of replays (including under Xvfb).

### Changed

- The sandbox is built on `shards`, as a game is: `shards::run`, `shards::testing`. The attack numbers and skeleton prefabs are content (`assets/content`). The world is created in `OnEnter(EngineState::InGame)`. The scenarios `boss_fight`, `dodge` and `errors` run in `cargo test`. The sandbox's own combat and movement (its own `move_to`, `attack`, `dodge`, `heal`, attack timers, collision circles) are replaced by engine modules. Tools now answer by the action rules (`ok: roll (W)`, `ok: queued, heavy starts
  in 0.10s`, or a refusal with the reason and the time). Everyone has stamina and poise, and the window keys are the engine's input map. The engine draws the telegraphs (`render` in `shards_action`). Developer keys: F1 id, F2 fight overlay, F3 colliders, F4 revive, F12 screenshot.
- The root `Cargo.toml` excludes `template` from the workspace. The template builds on its own and depends only on `shards`.

### Fixed

- Hot reload sees an edit that touches several files, and also created and deleted files.
- A replay starts with the content it was recorded with, before the world is created.
- Content errors: a missing required id, a definition that does not fit in JSON, a missing content folder.
- Prefabs: a typo in a key is an error. Reformatting a file does not change the definition or the replay hash.
- `testing::ok` fails if a gameplay system's panic stopped the step.
- Actions wait for loading to finish, and a failure during loading leaves the game paused.
- A scenario fails if the game cut a step short or lost journal entries.
- A death no longer touches an entity that is already removed.
- Movement: a fast fall into water ends in swimming, not a landing; physics skips a collider with a non-numeric position; a teleport restarts the character's movement; there is no climbing in the air onto a ledge that
  you can simply step onto. Stairs do not stop a walker. `path_to` routes through the steep bottom of a pool and walks along a ramp without climbing it. The controller, the planner, `clearance` and `jump` decide what can be climbed over by one rule. `clearance` calls a high wall a wall, and does not count the subject's own collider (`around=ID`). `move_to path=true` stops at the required distance, and with `queue=true` it refuses while orders are pending. `jump` in water answers `ok` only where the jump gets out onto the shore. The floor's tilt counts in the slope tolerance.
- Input and camera: the camera does not drop a lock-on that the agent set. The applied control state belongs to the body it was recorded for. Saving a rebinding does not overwrite a controls file that could not be read. Movement tools sync the physics first.
- Combat: the tick in which an action ends still hits and defends. A held input does not drop out at some tick rates. The content check rejects windows that would open on every hold cycle. A sprint drained dry does not start until the button is released. The `attack` order overrides a lock-on to another target, leaves no aim behind, and says when it cannot start. `dodge` and `block` report what the machine will do. The "get out of the zone" hint names a compass direction that really takes you out of the zone (including for a cone wider than a half-circle). The dead get no statuses, and their statuses do not tick. Telegraphs also show attacks on the player that have no warning.

## [0.1.0] — the AI layer

The first version: the engine as an environment that is driven from outside, with no LLM inside.

### Added

- `ai_core`: stable `AiId`s with tombstones, a text view of the world (`observe`, `inspect`, `nearby`), an event journal that catches `warn!`/`error!`, typed tools (`add_ai_tool`, `add_ai_query`, scope Player and Dev), pause, single-step `step` and stop conditions, and a deterministic `SimRng`.
- `shards-replay/2` replays: input recording from tick 0, playback, state hashes, a report of the first tick that diverged, and branching. `--verify` for CI.
- Window screenshots with `eN` labels and a legend (`ai/shot`).
- The sandbox (`crates/sandbox`): the player, skeletons, the Frost Warden boss.
- The `shards` CLI on top of BRP `ai/*` on 127.0.0.1, with a token: `observe`, `step`, `act`, `shot`, `save-replay`, `tools`, `inspect`, `wait` and others. Parallel headless instances (`--port`, `--port 0`, `--port-file`).

### Fixed

- An `AiId` cannot be rewritten without going through the index. A busy port stops the game (exit code 3). BRP is locked with a token, and tool files are written only to `replays/` and `shots/`.
- A panic in a gameplay system stops `step` and pauses the game. The session is saved to `replays/crash-tick<N>.json`.
- Log lines inside `step` keep their own tick, at most 64 per tick, and are visible to `--until`.
- Replay hashes are computed from game data, not from the text for the AI. `observe` works out visibility from the subject, not from the window's camera.
- `dodge` and `attack` explain a refusal instead of an empty `ok`.

[Unreleased]: https://github.com/larelgit/airustcargo/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/larelgit/airustcargo/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/larelgit/airustcargo/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/larelgit/airustcargo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/larelgit/airustcargo/commits/v0.1.0
