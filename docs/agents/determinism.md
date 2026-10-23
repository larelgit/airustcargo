# Determinism, simulation state, snapshots and replays

Read this when you write or change anything that runs in the simulation: systems, spawning order, maths, randomness, ordering, event observers, new simulation state, input components. Part of the [AGENTS.md](../../AGENTS.md) index.

## World creation and system order

- **Create the world in `OnEnter(EngineState::InGame)`**, not in `Startup`: content exists only after loading. The order of
  spawning is the same in every run, otherwise `AiId` and `ColliderId` will diverge.
- All gameplay runs in `FixedUpdate`, in one of the `SimSet` sets; randomness comes only from `SimRng`.
  The test `gameplay_systems_have_a_total_order` catches systems without an explicit order. Place your own systems "before" and "after"
  the module sets (`CoreSystems`, `MotionSystems`, `ControllerSystems`, `ActionSystems`, `PhysicsSystems`).
- An event observer (`On<Died>` and the like) that changes gameplay returns at once on a machine that does not simulate
  (`shards::core::simulates`): the host forwards combat events to a co-op client for effects, and otherwise the score would count twice
  (a desync catches this: `net: desync … here …, host …`).
- **Physics reads `Transform`, not `GlobalTransform`** (it is not recalculated inside a fixed tick): put colliders
  on top-level entities, and set the size in `Shape`, not in `Transform::scale`.
- **Order by `AiId` or `ColliderId`, never by `Entity`**, and not by the order of a query: a window adds render entities and
  shifts the `Entity` indices, and a replay must match with a window and without one.

## Simulation maths

- **Simulation maths goes through `bevy::math::ops` (libm), never through the `f32` methods from std.** `ops::sin(x)`,
  `ops::cos`, `ops::atan2(y, x)`, `ops::exp`, `ops::powf`, `ops::sin_cos` and the rest (`ops` is already in Bevy's prelude; for the game it is
  `shards::prelude::*`), and instead of `x.powi(2)` use `FloatPow::squared` (`bevy::math::FloatPow`). `x.sin()`, `x.atan2(y)`,
  `x.exp()`, `x.powf(y)` call the platform's maths, and on MSVC, glibc and macOS the last bits differ: replays and co-op
  would diverge between Windows, Linux and Steam Deck (Proton). The engine is built with `libm` (in `[workspace.dependencies]`: `bevy/libm`, parry3d with
  `indexmap`, and `glamx` with `libm` — for `simba/libm_force`), so `ops`, glam (`Quat`, `Vec3::normalize`, `slerp`) and
  collisions must give the same bits on all OSes of one architecture (x86_64: Windows, Linux, Steam Deck). Test runs have checked
  this on Windows 11 and on Ubuntu 24.04 under WSL1 on one machine (one processor): nine reference replays were recorded on both OSes
  byte for byte identically, and each OS passed `--verify` on the other's recordings; other x86_64 processors, macOS, ARM and Steam
  Deck hardware have not been checked. glam keeps its SIMD: SSE2 is the same instructions on any x86_64 OS; do not build with
  `-C target-cpu=native` or `+fma` (with them glam fuses the multiply and the add, and the bits differ). Determinism is not promised between architectures (x86_64 ↔
  ARM). Do not enable `scalar-math` in glam (`enhanced-determinism` in parry3d turns it on): `Vec4` and `Mat4`
  lose their 16-byte alignment, Bevy 0.20's `MeshUniform` becomes 168 bytes instead of 176 for the shader, and rendering stops; `shards`
  does not build with it (`const` with `align_of` in `crates/shards/src/lib.rs`; `docs/manual.md`, "Time and determinism"). The rule is enforced by `clippy.toml` (in the root and in `template/`:
  `disallowed-methods`): `cargo clippy … -D warnings` fails on
  a direct call (in tests and in examples too). `ops` exists only for `f32`. For `f64`, do not use these functions: convert the value to `f32`.
  A large `f64` is first reduced to a small one, while it is still `f64` (the phase, modulo `TAU`): `f32` has only 24 bits.
  Exact operations (`sqrt`, `abs`, `floor`, `round`, `mul_add`, `rem_euclid`, `to_radians`) stay as methods.

- **A hold of the clock is not a pause and not the simulation (hit stop).** `ai_core::ClockHold` forces the virtual clock's speed to 0 for real time while a hit stop lasts, only alone with a picture, in real time (never headless, stepped, in a replay or in a stand): zero ticks run inside it, so ticks, replays and snapshots do not see it. `sim::speed()` reports the player's speed, so read speed through it, never `Time<Virtual>::relative_speed()`. Presentation that must keep moving through a hold reads `Time<Real>`. A co-op machine never holds the clock; `PresentationHolds` freezes the picture of single bodies by `AiId` and is not a component. Detail: `docs/manual.md`, "Hit stop".

## Snapshots, replays, tick time

- A snapshot does not see new simulation state outside entities with `AiId` (a resource, a counter): register it
  with `add_snapshot_resource::<R>()` (it needs `Reflect`); a cache built from it goes in `add_snapshot_rebuild`; what the world is built around
  (files, geometry) goes in `add_snapshot_check`. Otherwise `snapshot_load` says `the restored state differs`,
  and a replay from the snapshot will diverge. References to entities without `AiId` go in `add_snapshot_entity_key`.
- **A component a system puts on an entity with an `AiId` is state, a private marker included**: derive `Reflect`, add
  `#[reflect(Component)]` and `register_type` it, or a snapshot leaves it out (its `left_out` names it), a game loaded from the
  snapshot goes another way, and heavy mode stops on it (issue #72: the encounter umbrella's marker of its own `NetAlways`). Bookkeeping whose effect a
  checked component shows may be `replay_ignore`d as well (that marker, `OrderProgress`): snapshots still hold it. The state
  audit (`crates/sandbox/tests/sandbox/state_audit.rs`) fails on any such component it sees that its list does not excuse with a reason.
- Replay checks (`shards-replay/4`) compare the game components of entities with `AiVisible`, not the text of
  `observe`. Exclude cosmetics and caches with `replay_ignore::<C>()`; add state outside such entities
  (resources, projectiles without an id) with `add_replay_check(name, |w| …)`. Anything that is only for the window must not change
  the game components.
- Tick time is not part of the hash or the replay, and it differs from run to run. A test that compares
  whole answers or whole journals turns its output off: `report_perf: false` in `EngineConfig`
  (`shards::testing::config()` already does this).
- Keyed randomness and facing (encounters). A module that draws numbers outside an entity's own brain builds a `DetRng::from_keys(&[seed, tick, fnv(run key), wave, group, purpose, ...])` with a local FNV hash (a `DefaultHasher` is randomized per process), so two draws whose keys differ (another run, wave, group or purpose) never share a stream, and a replay draws the same with or without a window. "Which way a player faces" is read from simulation state (the body, then `Intent.target`, then `Intent.aim`; a body whose `locomotion` says `face: Look` has the yaw of `Intent.look`, and `shards::motion::{eye_of, look_dir}` give the eye and the look direction), never from a camera.
- Downed state and lethal hooks (B5). `Downed` and `Grace` are simulation state (reflected, in snapshots, with describers); a body saved down loads dead and a load into a downed body ends it. A lethal hook runs inside the simulation on the host only (`deal_damage`, `finish_if_dead` and `check_deaths` change nothing on a machine that does not simulate) and must read simulation state in `AiId` order, never `Entity` or `HashMap` order and never the network (a "solo" test reads player bodies, not connected peers). `tick_downed` and `check_deaths` order their bodies by `AiId`.
