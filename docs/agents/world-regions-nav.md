# World, regions, levels, generation and navmesh

Read this when you touch regions, tiles, levels, the generator, the navmesh or the state they keep in snapshots. Part of the [AGENTS.md](../../AGENTS.md) index.

- **World, regions and levels (phase B).** The things the gameplay takes from tiles, rooms and levels (colliders, markers, prefabs,
  points of interest) are **top-level entities with a world `Transform`**: that is how physics reads them and how `spawn_region` and
  `spawn_level` create them. A region is changed with tools (`regen`, `region_load`) or with `change_region`, through
  `EngineState::RegionLoading`, inside one call; it is not spawned or removed by hand. The starting region
  (`start_region`) is built by the engine (`build_start_region`) before `OnEnter(InGame)`, so its `AiId`s come first; the game's own
  entities still spawn in `OnEnter(InGame)`, after it.
- **Generation is a pure function of (seed, generator version, content).** The randomness of the steps is `GenContext::rng()` (`DetRng`),
  never `SimRng`, never the clock and never the order of `HashMap`/`HashSet` (the order comes from indices and `BTreeMap`). If you change the generator's output
  (steps, weights, solver rules), raise the region's `version`: the old seeds will give new worlds, and the saved manifests
  stay as they are. To rebuild the references: `--worldgen-report` on 1000 seeds, `record_reference_replays`, the manifest hash in
  `region.scenario`.
- **Voxel generation.** A voxel region is made by steps registered by name and version (`add_voxel_step("hills", 1, hills)`;
  `shards_voxel`), run in the order the region lists them, with randomness only from `VoxelGen::rng` / `VoxelGen::rng_keyed` (`DetRng`, keyed by
  seed, region version, attempt and the step's place) and `VoxelGen::noise` (keyed by seed, region version, attempt and a salt: the same salt
  is the same field in every step of an attempt), heights from `Noise` (no `powf`, no `sin`). A step is code the content hash cannot
  see: when you change what a step produces, raise its registered version and the version the content lists, so a client with other step
  code fails content validation instead of desyncing. The volume hash (`VoxelVolume::hash`, `world_hash`) is the backstop; its golden
  tests change only on purpose.
- **Navmesh.** Baked tiles are in `assets/nav/` (`cargo sandbox --bake-nav`, tool `nav_bake`) and are committed; with
  the `nav` definition, `--check-content` checks them (a stale file is an error that names the command to run). If you change what a tile is baked with
  (the ground probe and the step rule in `shards_motion`, the sampling in `shards_npc::nav`), raise `BAKE_VERSION` and bake everything.
  Editing a tile, its level file or a geometry prefab means baking again. Body classes (`bodies` in `*.nav.ron`: `agent` and,
  for example, `large` for a big enemy) are each baked into their own folder (`assets/nav/large/`); editing the `locomotion` of any class,
  a new class, or its `radius`/`height`, also means baking again (`--bake-nav` bakes all classes) and committing the class's files.
  `--check-content` names the missing or stale file of each class (``… for the body `large` (nav/large/…)``).
- **World state in the snapshot.** New region, level or navigation state outside entities with `AiId` goes in `add_snapshot_resource`;
  what is built from the saved data (the region from its manifest, the navmesh) goes in `add_snapshot_prepare` / `add_snapshot_rebuild`; what
  the world is built around goes in `add_snapshot_check`. Cover it with a test modelled on `crates/sandbox/tests/sandbox/snapshots.rs`
  and `crates/shards_world/tests/world/regions.rs` (a snapshot taken in a region, loaded into a game that does not have it).
