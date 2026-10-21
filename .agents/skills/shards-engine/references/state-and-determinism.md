# What each kind of change obliges you to do

The rules are in `docs/agents/determinism.md`, `docs/agents/tools-and-invariants.md`, `docs/agents/content.md` and
`docs/agents/world-regions-nav.md`; this table turns them into "if you add this, do that". The last column is the
check that fails when you forget, so you know what will tell you, and what to run.

## Systems, order, maths, randomness

| You add | You must | Caught by |
|---|---|---|
| A system that changes gameplay | `FixedUpdate`, `.in_set(SimSet::Input / Think / Act / Move / Resolve / Cleanup)`, ordered with `.before`/`.after` against the module sets (`CoreSystems`, `ActionSystems`, `NpcSystems`, `MotionSystems`, `ControllerSystems`, `PhysicsSystems`, …; `docs/manual.md`, "Tick order") | `gameplay_systems_have_a_total_order` (sandbox, `shards_npc`, `shards_action`); ambiguity warnings in the journal |
| Spawning at the start of a world | `OnEnter(EngineState::InGame)`, in a fixed order (ids follow spawn order); a region's things through the region API, never by hand | ids that differ run to run; replays (`e15 missing`) |
| Trigonometry, `exp`, `powf`, `powi` | `ops::sin(x)`, `ops::atan2(y, x)`, `ops::powf`, `FloatPow::squared` (only `f32`; reduce an `f64` first) | `cargo clippy -- -D warnings` (`disallowed-methods` in `clippy.toml`); `cross.yml` on Windows |
| Iteration whose order reaches the world | sort by `AiId` (or `ColliderId`); `BTreeMap`/indices, never `HashMap`/`HashSet` order, never `Entity` or query order | `--verify --windowed` (a window shifts `Entity` indices); co-op desyncs |
| Randomness | in a tick, `SimRng` streams keyed by `AiId` and a salt constant; outside an entity's brain, `DetRng::from_keys(&[seed, tick, fnv(key), purpose, …])` with a local FNV hash; in generation, `GenContext::rng()` / `VoxelGen::rng` | determinism tests (same seed, same journal); replays |
| Reading where a body is drawn (camera, marker) | after `InterpSystems::Blend`, with `shown_translation`; never write a body's `Transform` for the picture | replays with a window |
| Reading time or speed | `Res<Time>` inside a tick; speed through `sim::speed()`, never `Time<Virtual>::relative_speed()` (hit stop holds the clock) | `hitstop.no_ticks_inside` |

## State: snapshots, replays, co-op

| You add | You must | Caught by |
|---|---|---|
| A component a system puts on an `AiId` entity (a private marker too) | `#[derive(Component, Reflect)]`, `#[reflect(Component)]`, `app.register_type::<C>()` | `state_audit.rs`; heavy mode; `snapshot_load` "left_out" |
| A cosmetic or cache component on such an entity | also `app.replay_ignore::<C>()` (snapshots still hold it); presentation-only also `net_ignore::<C>()` | reference replays move when they should not; `replay checks leave out …` for a type that does not serialize |
| A resource that is simulation state | `#[derive(Resource, Reflect)]`, `add_snapshot_resource::<R>()`; if replays should see it, `add_replay_check(name, fn(&World) -> u64)` (`add_optional_replay_check` when it may be absent) | `state_audit.rs` (`OUTSIDE` lists what is not state, with why); heavy mode |
| A cache built from state | `add_snapshot_rebuild(name, fn(&mut World))` | heavy mode; a replay from a snapshot diverges at its start |
| State built around files or geometry | `add_snapshot_check(name, hint, fn(&World) -> u64)`; `add_snapshot_prepare` for what must be rebuilt before entities load | `snapshot_load` refuses or differs |
| A reference to an entity without an `AiId` | `add_snapshot_entity_key` | snapshot round trip |
| An input component (something a player or agent sets between ticks) | `app.replay_input::<C>()`, or write `Intent` instead | replays diverge on playback |
| An observer of an event that changes gameplay | first line: `if !shards::core::simulates(role) { return; }` (`role: Option<Res<NetRole>>`) | co-op desync (`net: desync … here …, host …`) |
| A tool, seen from co-op | the route `NetRoute::default_for` gives it, or `app.net_route(name, …)`: a query answers on the machine asked (a client from what it holds; its own body as predicted, `shown_state`), `HostOnly(why)` when only the host can answer; a dev action is forwarded to the host; an action only the host may make is `HostOnly(why)` | stand scenarios; tests on a stand (`crates/sandbox/tests/sandbox/net_predict.rs`) |

## Content

| You add | You must | Caught by |
|---|---|---|
| A content kind | a struct with `Serialize`, `Deserialize`, `#[serde(deny_unknown_fields)]`, `impl ContentDef` (`KIND`, `validate` with `refs.require::<Other>(id, field)` for every reference), `app.add_content::<T>()`; `require_content` for ids the code needs | `--check-content` and a typo test |
| A kind only the picture, the sound or the menus read | `const SIMULATION: bool = false;` | reference replays move when a look is edited |
| A field on an existing definition | `#[serde(default, skip_serializing_if = "…")]` at a neutral default, so content that does not use it serializes and hashes as before | reference replays; the co-op content hash; a JSON-pinning test where one exists (`existing_animsets_keep_their_json`) |
| A read of content by the simulation | inside a tick, an action or a builder, by id (`get`); `iter`/`ids` only when the whole kind matters (it makes the whole kind count for replays); a describer, a picture or a query reads with `peek` | the replay `content` check; `content_reads_are_the_same_headless_and_drawn` (GPU, nightly) |
| A tunable number | content, not a `const` (see "Numbers still in Rust code" in `docs/manual.md`) | review |

## World, navigation, generation

| You change | You must | Caught by |
|---|---|---|
| What the generator produces (steps, weights, solver rules) | raise the region's `version`; re-run `--worldgen-report` on 1000 seeds; update the manifest hash its scenario expects; re-record replays | `--worldgen-report` (exit 1); the region scenario |
| A voxel generation step | raise its registered version (`add_voxel_step(name, version, f)`) and the version the content lists | content validation in co-op; golden volume hashes |
| Tiles, level files, nav settings | `cargo sandbox --bake-nav`, commit `assets/nav/` (every body class) | `--check-content` names the stale file |
| The ground probe, the step rule or the nav sampler | raise `BAKE_VERSION`, bake everything | `--check-content`; `chunks_stitched_are_the_grid_baked_in_one_piece` |
| An animset clip that an action bakes from | `cargo sandbox --bake-motion` | `--check-content`: `displacement is stale` |

## Agent surface

| You add | You must | Caught by |
|---|---|---|
| A tool | an args struct with doc comments (they become the schema), a system `In<Call<Args>> -> ToolResult`; in an engine crate `register_tool(world, name, scope, ToolKind::Action / Query, f)` (a game: `add_ai_tool` / `add_ai_query`): an action changes the world and is journaled and recorded, a query only reads, a control moves time or the session and is never recorded; `Scope::Player` works through `Intent`, `Scope::Dev` for cheats; every error says how to fix the call | `tool_contracts.rs` (calls every tool with 20 valid and 10 broken argument sets: no panic, errors that name the signature or the bad argument, action calls that replay); your tool's own tests |
| An invariant | `add_engine_invariant("<module>.<rule>", fn(&World) -> Option<String>)` (a game: `add_ai_invariant`); a doc comment saying what it checks and why any carve-out exists; it only reads, through `with_query`; names offenders lowest id first (`Offenders`); a control test; its name in `ENGINE_RULES` (`crates/sandbox/tests/sandbox/invariants.rs`) and the rule count there | the control test; the sandbox's rule list; it is on in tests, `--verify`, fuzz and soak |
| Something agents must see on an entity | `register_ai_component::<C>(describe)`, `register_ai_describer(key, fn(EntityRef) -> Option<String>)`, or `register_ai_world_describer` | tests on `observe`/`inspect` text |
| An event agents must hear about | `app.journal_event::<E>(format)`; machine-local lines (music, picture) are `JournalKind::Local` | scenario `expect journal` |
| A condition to wait on | `add_ai_condition(name, fn(&World) -> bool)` | scenario `step --until named:…` |
| A developer shortcut | `add_dev_entry(group, id, label, DevCommand::Run("tool k=v".into()))` calling the same tool | `dev_menu` tool |

Never write `AiId` yourself (`get_mut`, `set`): the engine assigns it; inserting a new one renumbers the entity.
