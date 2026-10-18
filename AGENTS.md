# AGENTS.md

Instructions for coding agents (Claude Code, Codex and others) that work in this repository.

## The main rule

The engine has no LLM and must not have one. Do not add built-in or local agents, models,
calls to model APIs, API keys, or "model → tool" loops inside the game. The game is an environment that
is driven from outside: by you, through the `shards` CLI. Anything an agent needs is added as a tool
(`add_ai_tool` / `add_ai_query`) or as a `shards` command, not as code that does the thinking for the agent.

## Building

- System packages: `pkg-config libasound2-dev libudev-dev` (for a window under Xvfb, also `xvfb mesa-vulkan-drivers
  libxkbcommon-x11-0`).
- `cargo build --workspace`, `cargo test --workspace`. The first build of Bevy is slow;
  `.cargo/config.toml` limits it to four threads. `template/` is a separate workspace:
  `cargo test --manifest-path template/Cargo.toml`. CI runs the tests with `cargo-nextest` (`cargo install cargo-nextest --locked`; a
  process per test, see "Checking changes"); `cargo test` stays a supported local command.
- Crates: `shards` (the umbrella, the game's only dependency), `shards_core`, `shards_content`, `shards_motion`,
  `shards_action`, `shards_level` (levels from Blender), `shards_world` (regions, generation), `shards_npc` (enemies, navmesh),
  `shards_save` (saves: sections, slots, migrations, copies), `shards_ui` (screens and menus from content), `shards_locale`
  (languages: texts in Fluent, switching on the fly), `shards_audio` (sound: buses, voices, music in layers), `shards_look` (look: light
  in bands, flat faces, outlines from content), `shards_scatter` (foliage and small objects by tile), `shards_voxel` (voxel terrain core: chunked block volume,
  palette, generation steps, hash), `shards_fx`
  (effects: particles from pools, telegraph decals), `shards_steam` (Steam: lobbies, relay for co-op, achievements, cloud;
  the umbrella's `steam` feature, off by default), `shards_dev` (developer menu and console), `ai_core`, `shards_script`,
  `shards_cli`, `sandbox` (the engine's arena). The map is in `docs/manual.md`, "Architecture".

## Watching the game and acting in it

```bash
cargo sandbox --headless --paused --port 0 --port-file /tmp/g.port &   # no window, paused: time moves only by step
export SHARDS_PORT_FILE=/tmp/g.port
cargo shards wait                       # waits for loading, prints pid, seed and port
cargo shards observe                    # text slice of the world + journal since last time
cargo shards tools                      # tools and signatures (--json: argument schemas)
cargo shards act move_to target=e6      # action → CHANGES and JOURNAL
cargo shards step 120 --until journal:died
cargo shards inspect e6                 # all components of the entity
cargo sandbox --offscreen --paused --port 0 --port-file /tmp/g.port &   # instead of the first line: like --headless, but the GPU draws (for shards shot)
cargo shards save-replay                # session from tick 0 → replays/tick<T>.json
cargo shards save-snapshot              # state at this tick → snapshots/tick<T>.json
cargo shards load-snapshot snapshots/tick120.json   # go back to it (the region and navmesh come back with it); the replay continues from this snapshot
cargo shards scenario FILE...           # scenario file: commands, expectations, invariants (`docs/manual.md`, "Scenarios")
cargo shards content check              # content of the running game; without a game: cargo sandbox --check-content
cargo shards act regen region=wilds seed=42    # generate a region and go to it (loading screen, the player at the gate)
cargo shards act nav_path to=e6         # route over the region's navmesh; npc_state id=eN — what the NPC has on its mind
cargo shards worldgen --region wilds --seeds 1..1000 --report   # batch check of the generator; without a game: cargo sandbox --worldgen-report wilds --seeds 1..1000
cargo sandbox --bake-nav                # bake tile navigation into assets/nav/ for all body classes (after a change to a tile, its level file or the body of any class)
cargo sandbox --bake-motion             # bake action offsets (the action's `bake`) from the clip's root bone (after a change to the clip)
cargo sandbox --replay FILE --verify    # replay without a window; add --windowed (under xvfb-run -a) for a window
cargo sandbox --replay FILE --verify --heavy 30   # also: from every 30th tick's snapshot, a fresh game goes on the same (heavy mode)
cargo shards assert add hp 'field:6:Health.current<300'   # rule: the game stops when it is broken (`step` → exit code 3)
cargo shards fuzz --seed 7 --steps 300 --chaos 0.02 --invalid 0.05  # random player inputs from the tool schemas, 2% developer calls, 5% broken on purpose, until a rule is broken
cargo shards fuzz --seed 7 --steps 300 --chaos 0.02 --steer --minimize  # needs a game started with --coverage; on a stop, shrinks the session and writes scenarios/regress/<date>-<stop>-seed7.scenario
cargo sandbox --coverage --headless --paused --port 0 --port-file /tmp/g.port &   # also counts the states the session reaches
cargo shards coverage [--since K]       # distinct signatures, the rarest, what is new since K, what content declares and was never reached; `coverage reset` clears it
cargo shards soak --minutes 30 --regen wilds   # stand: agents with a seed play for all participants, until the first desync
kill <pid>                              # pid from `wait`; on Windows (and everywhere) — cargo shards quit
cargo shards package --manifest-path template/Cargo.toml   # build for players in template/dist/ and its check (`--smoke`, DLL)
cargo run -p sandbox --release -- --perf reference --offscreen   # performance test: a line per budget, exit code 0, 1 or 2 (no GPU)
```

- Run `shards` from the game's folder or below it: the token `.shards/<port>.token` lives there, and without it the game
  answers `unauthorized`. From another place, pass it in `SHARDS_TOKEN`.
- Stop the game with `kill` (or Ctrl+C), not `kill -9`: that way it deletes the token and `--port-file`. On Windows there are no signals:
  `shards quit` (the `quit` tool) ends the game the same way and waits until it stops answering and its process ends.
- Run a built binary through `cargo sandbox` (or with `BEVY_ASSET_ROOT=crates/sandbox`: the engine makes the relative path absolute from the working folder): otherwise it will not find `assets/`, and the world will be empty.
- The game writes files only to its own folders: replays to `replays/`, screenshots to `shots/`, state snapshots to `snapshots/`,
  crash reports to `crashes/` (`save-replay replays/x.json`, `shot -o shots/x.png`, `save-snapshot snapshots/x.json`).
  Absolute paths and `..` are not accepted.
- Take numbers and positions from `observe` and `inspect`, not from pictures. `shards shot` (needs a window, which can run under Xvfb, or
  `--offscreen` instead of `--headless`: no window, but with a GPU; `display` in `shards status` says whether there is a picture) is for visual
  bugs: a PNG with `eN` labels and a legend that says which id is which in the picture.
- Tool errors explain how to fix the call. Read them, do not repeat the call blindly.
- Engine tools (`shards tools`): **movement** — `move_to` (`path=true` follows `path_to`), `move`, `stop`, `jump`,
  `sprint`, `glide`, the `clearance` and `path_to` queries (in a region over the navmesh: `path_to` names the `provider`, `clearance` —
  `closed`, where the navmesh ends before the physics does), `input_bindings`, `rebind`; **combat** — `light_attack`, `heavy_attack`,
  `block`, `parry`, `dodge`, `use_item`, `use_technique`, `lock_on`, `attack`, `god`, `heal`, `stagger`, `react` (a developer tool: it forces a reaction), `motion_bake`
  (the same as `--bake-motion`); **content** —
  `content_list`, `content_get`, `content_check`, `content_reload`, `spawn_prefab`; **world** — `regen`, `region_load`,
  `goto_tile`, `region_info`, `worldgen_report`, `spawn_level`, `level_info`, `level_reload`; **NPCs and navigation** —
  `npc_state` (`nav.routes`: over the navmesh of which body class the NPC plans, or why it plans over the local grid), `npc_set_state`,
  `nav_path` (`body=large` — over the navmesh of a body class), `nav_info` (`bodies` — the body classes and their navmeshes), `nav_bake`, the overlays
  `world_overlay` and `nav_overlay` (window),
  `npc_overlay` (a line on each NPC near the players: state, target, token, leash, what it sees and hears; in the window — F7); **rules** —
  `assert_add`, `assert_list`, `assert_remove`, `assert_reset` (`shards assert`); **UI** — `ui_read` (the screen tree: widget ids,
  values, focus), `ui_open`, `ui_close`, `ui_navigate`, `ui_click`, `ui_set` (for a key-binding row — `value=Key(KeyK)`);
  the main menu (in a game that has one, without a window — with `--menu`) — `new_game`, `continue_game`; **language** — `locale` (the languages, which one
  people play in, what each one is missing), `text` (a message with parameters in any language); the language is changed with `settings_set group=locale
  key=language value=ru`; **audio** — `audio` (what this machine hears: the track and music layer, stems, buses, voices, sounds that were not heard, with the reason),
  `sound_play` (play a sound by the mixer's rules: `at=eN`, `point=[x,y,z]`, `times=25`), `music_set`
  (`track=…`/`none`, `layer=combat`/`auto`); volume — `settings_set group=audio key=sfx value=0.5`; **look** — `looks` (looks from
  `*.look.ron`, how many meshes each one draws; without a window and `--offscreen` nothing is styled, and it says so),
  `look_outlines on=false` (hide the outlines); **foliage** — `scatter` (`*.scatter.ron` definitions, how many of each were scattered
  and what found no room; without a window and `--offscreen` nothing is scattered, and it says so), `scatter_set on=false`
  (remove; `on=true` — scatter again), density — `settings_set group=graphics key=foliage value=0.5`; **effects** —
  `effects` (effects from `*.effect.ron`, their pools, the event table and the telegraph decals on the ground), `effect_play effect=hit_spark
  at=eN` (or `point=[x,y,z]`), `telegraph_decals on=false` (the telegraph fill becomes gizmos again); without a window and `--offscreen` effects
  do not play, and the tools say so; **Steam** (a game with the `steam` feature) — `steam` (who this machine is in Steam, the lobby and
  its type — `--steam-lobby friends|private|invisible|public` — or the game server, the relay and the route to each peer, achievements,
  cloud), `steam_lobbies` (the lobbies of this build with free seats; `all=true` — and the others), `steam_host`, `steam_join lobby=ID`
  or `host=ID`,
  `steam_invite friend=ID`, `achievement_unlock id=…` (host only); **developer menu and console** — `dev_menu` (the items by section, with a line
  for each; `run=<id>` runs an item: `boss_phase_2`, `god`,
  `region_check`), `console line="speed x=0.25"` (a line as for `shards act`); in the window this is F10 and `` ` ``, `open=true` shows
  the panel for `shot`. The game puts its own items in content (`*.devmenu.ron`), and a module does it with `add_dev_entry`. **Exit** — `quit`
  (`shards quit`: exit hooks run, the token and the port file are removed). Conditions for
  `step --until named:…`: `near_poi`, `npc_attacking`, `npcs_calm`, `npc_beyond_leash`, `npc_detour`, `npc_avoiding`,
  `npc_engaged`, `npc_near`, `reacting` (a fighter runs one of its moveset's reaction actions, not a stagger); the game adds its own (in the sandbox `boss_phase_2`, `boss_dead`, `player_dead`, `three_attacking`; in the template
  `arena_cleared`, `foe_near`, `in_fight`).
- Sound is resolved on the simulation clock and is the same without a window: listen to the game through `audio` and the `MUSIC` line in `observe`, not through
  the speakers (except the sound of the `AnimationCue` clip marker: it exists only if a model is drawn). Music changes go
  to the journal as lines like `[local] music: arena explore → combat` (they are not game events). The sandbox's and the template's sound files are synthesised by `cargo run -p sandbox --example make_sounds`.
- Combat tools (`dodge`, `light_attack`, `block`, `use_item`, `attack`) answer at once, under the game's rules: `ok: roll (W)`,
  `ok: queued, heavy starts in 0.10s` (the input waits in the buffer), or an error with the reason and the time (`not enough stamina: 10/20
  (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`). On an error, run `step`
  for the time the error names, then try again. Inputs that bypass the tools and whose time has run out show up in the journal: `e5 dodge rejected: …`.
- Enemy warnings reach the journal in advance (`e6 winds up slam (0.50s) ⚠ circle 4m`), and `observe`
  under the `YOU` line says where to leave the zone: `step --until "journal:winds up"`, then `dodge`.
- Exit codes of `shards`: 0 — done, 1 — a tool refused (the error says why), 2 — `shards` itself failed
  (no game, no token, wrong command), 3 — the game stopped: `step` (or `fuzz`, `soak`) halted on a crashed
  system, a broken invariant or a failed heavy check (`--heavy`, see "Checking changes").
- Looking for a bug? Describe it as a rule: `shards assert add NAME COND`, where `COND` uses the `--until` syntax
  (plus `warning` and `error`). After each tick the game checks the game's rules and yours; on a violation
  `step` answers `stopped: invariant NAME violated at tick N (replay: …)`, and the replay is already saved.
  A rule that has fired stays silent until `shards assert reset`; `shards assert list` shows all rules.
  `shards fuzz --seed S --steps N` feeds random player inputs (every player action tool, arguments from its schema), driven by the seed, until a rule is broken.
- The game's `warn!`/`error!` messages reach the journal with the tick they were logged on, even inside a long `step`:
  `step 600 --until journal:TEXT` catches them too. A tick holds no more than 64 lines; the rest are reported by the line
  `N log lines dropped` (the asset threads have their own 64).
- Below the first line of a `step` answer is the time of its ticks: `ticks: avg 0.42 ms, max 2.10 ms (tick 142),
  budget 16.7 ms`. Ticks longer than the budget (by default the tick length; the game has the flag
  `--tick-budget-ms`) are logged as a `perf` entry, at most once per game second:
  `150 [perf] 12 ticks over budget, worst 31.4 ms at tick 150`. The time of the last 600 ticks is
  in `shards status`, field `perf`.
- For a game that draws (a window or `--offscreen`), `shards status` also shows frames, field `frames`: the average and worst frame time over the last 600, how many of them were longer than the budget (1/60 s, flag `--frame-budget-ms`), `fps`, the share of time spent on the main world and on rendering, GPU time and triangles per frame. Field `counts`: entities, meshes, meshes that draw something, and camera meshes.
- Several games in parallel: `--port` for each instance, `SHARDS_PORT` for the CLI. The easiest way:
  `cargo sandbox --headless --paused --port 0 --port-file F` and `SHARDS_PORT_FILE=F cargo shards wait`.
  On a busy port the game does not start (exit code 3). `shards wait` prints the pid, seed and port of the game
  that answered: check that it is yours.
- Co-op without windows, in one command: `cargo sandbox --stand 2 --headless --paused --port 0 --port-file F --net-sim
  latency=150,loss=0.02 &` — a host and two clients, each with its own port and token (`F`, `F.c1`, `F.c2`). Then `export
  SHARDS_PORT_FILE=F`, and: `cargo shards wait --peer all` waits for all of them (and fails at once with the reason if the game is not a stand, the host has ended the stand, or the token is wrong; a plain `shards wait` behaves the same way), `cargo shards --peer c1 observe` looks through the client's eyes (the client's token is `SHARDS_TOKEN_C1` or its `.shards/<port>.token`; `SHARDS_TOKEN` is the host's token), `cargo shards stand` shows the participants, `cargo shards stand stop` stops the stand. The stand's scenarios (`stand clients=2 …`, commands with `@c1`) are in
  `crates/sandbox/scenarios/stand/`: `shards scenario crates/sandbox/scenarios/stand/*.scenario`. `--stand 2 --stand-udp` —
  the same over real UDP, with clients as processes, in real time only; a client that did not join the session or that crashed
  stops the stand (a client that left on its own, or one the host cut off with `net_kick`, does not), and `--stand N` must be no more than
  `max_players - 1`. `--stand N --stand-steam [--steam-lobby TYPE]` (a game with the `steam` feature; the `cargo sandbox` alias passes the flags to the game, so the feature
  is set by `cargo run`: `cargo run -p sandbox --features shards/steam -- --stand 1 --stand-steam --steam-lobby invisible --headless
  --paused --port 0 --port-file F &`) — the stand runs in one process, but
  the participants meet through a fake Steam, not through the in-memory network: the host makes a lobby (by default `friends`), the clients wait for
  the lobby (`steam_lobbies` searches, `steam_join` joins; `wait --peer all` waits for the clients to join, so here wait for the host: `cargo shards
  wait`); the stand does not touch the real Steam. The scenario for such a stand is
  `crates/shards_steam/scenarios/lobby_search.scenario`.
- To check how the game plays fairly: `shards session --scope player`. Then only
  the player's tools are available, and they act through `Intent`, as a human does.

## Code rules

- Code, comments and documentation are in English. `README.md` is a short project page (what it is, quick start, the game
  step by step, links); the detailed documentation is `docs/manual.md` (the manual), this file, the rest of `docs/`, `CHANGELOG.md`
  and `template/README.md`. Russian stays only in game text and test data (Fluent files, locale tests).
- Bevy is pinned to 0.20.0; do not update it as a side effect of another task.
- **The boundary.** The game (the template, the sandbox) depends only on the `shards` crate and does not change the engine's code. The engine has no names
  from a particular game: if a mechanism will be useful to a second game with a different story, it goes in the engine; if it is a particular
  "what" (a weapon, an enemy, a number), it goes in the game, that is, in content.
- **Content.** Numbers and definitions are files `assets/content/**/*.<kind>.ron` (hot reload), not constants in code.
  The text the player reads lives in Fluent messages in `assets/locale/<language>/*.ftl`: content names them `@id`, and the code
  formats them with `shards::locale::text(world, id, &Args)` as whole sentences with parameters, not by joining pieces.
  The sandbox does the same: its world, prefabs, attacks, movesets and movement are in `crates/sandbox/assets/content/`. A new content kind is
  a struct with `ContentDef` and `add_content`; references to other definitions are checked in `validate` (`refs.require`).
  The replay check `content` covers only the definitions the simulation read (a fixed tick, an action call, the world being built):
  read one definition by id when that is all the code needs (`iter`/`ids` make the whole kind count); code inside the simulation's
  scope that is not the simulation (a describer, a picture) reads with `peek`/`peek_iter`; a kind only the picture, the sound or
  services outside the session read sets `const SIMULATION: bool = false;`. A builder called outside a tick or an action opens the
  scope (`ai_core::scope::open`), and what only watches inside one suspends it (`scope::suspend`). The test
  `content_reads_are_the_same_headless_and_drawn` (GPU, ignored) checks that drawing reads nothing that counts. A read with the
  scope closed is never counted: content the simulation depends on is read in a fixed tick, an action or a builder, not in
  `Startup`, a loading state or a cache built at load (a change to it would then show only later, in the entities' hashes, unnamed).
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
- Characters are driven only through `Intent`: the keyboard, the agent's tools, the enemy brains and the replay write to it.
  Register a new input component through `replay_input::<C>()`, otherwise replays will diverge.
- A snapshot does not see new simulation state outside entities with `AiId` (a resource, a counter): register it
  with `add_snapshot_resource::<R>()` (it needs `Reflect`); a cache built from it goes in `add_snapshot_rebuild`; what the world is built around
  (files, geometry) goes in `add_snapshot_check`. Otherwise `snapshot_load` says `the restored state differs`,
  and a replay from the snapshot will diverge. References to entities without `AiId` go in `add_snapshot_entity_key`.
- Replay checks (`shards-replay/4`) compare the game components of entities with `AiVisible`, not the text of
  `observe`. Exclude cosmetics and caches with `replay_ignore::<C>()`; add state outside such entities
  (resources, projectiles without an id) with `add_replay_check(name, |w| …)`. Anything that is only for the window must not change
  the game components.
- Tick time is not part of the hash or the replay, and it differs from run to run. A test that compares
  whole answers or whole journals turns its output off: `report_perf: false` in `EngineConfig`
  (`shards::testing::config()` already does this).
- Write what must always hold in the game as an invariant:
  `add_ai_invariant(name, |w: &World| -> Option<String>)`. The rule only reads the world and returns
  the text of the violation with the entity's id (`e6 Health.current = 700 outside 0..=600`). Rules and their firings
  are not part of the replay hash or the snapshot; the replay records session rules (`assert_add`) as events.
  The engine's own rules are named `<module>.<rule>` and registered with `add_engine_invariant(name, rule)` (shards_core): on by
  default, off in a `dist` build and with `--no-engine-invariants` (`EngineConfig::engine_invariants`; tests, `--verify`, fuzz and soak
  keep them on). A rule not yet proven on all content (`voxel.body_in_solid`) is offered with `add_opt_in_engine_invariant`
  and checked only when named: `--engine-invariant NAME` (`EngineConfig::opt_in_invariants`). Each states in its doc comment what it checks and why any carve-out exists, and has a control test that breaks the
  state on purpose. If one fires, it is a bug in the engine or the game: fix the bug, do not weaken the rule without saying why.
- A new tool is an argument struct with doc comments (they become the schema's description)
  and a Bevy system `In<Call<Args>> -> ToolResult`. Kinds: `query` reads, `action` changes the world
  (journal, diff, replay), `control` manages time and the session. A tool error says how to fix the call.
- The AI sees only entities with `AiVisible` (`AiKind` gives it). What is said about them is decided by
  the describe functions (`register_ai_component`, `register_ai_describer`).
- The engine assigns `AiId`. Do not write it in place (`get_mut`, `set`, BRP `world.mutate_components`):
  the "id → entity" index is kept by the `on_insert` and `on_discard` hooks. Inserting a new `AiId` renumbers
  the entity, and the old id gets a tombstone `renumbered to eN`.
- **The HUD and menus are content** (`*.screen.ron`, the theme is `*.theme.ron`), not window code: the game gives widgets their values with
  `add_ui_source`, and button actions with `add_ui_action`. Screens are outside the simulation (no `AiId`, not in the snapshot); anything that changes the world,
  a button does through a tool (`tool …`) or a game action, and the replay repeats the press itself (`ui_click`, `ui_navigate`).
- **Sound is content** (`*.sound.ron`, `*.music.ron`, the mixer `*.audio.ron`): an attack names its sound in `telegraph.sound`, and an action
  plays the effect `Sound("whoosh")` at its own moment (the swing); the game plays its own `PlaySound`, and a music layer waits for a condition by name
  (`add_ai_condition`). Sound only reads the world and does not change it; what only this machine shows is written to the journal as
  `JournalKind::Local`, not `Event` (the host sends events to clients). Sound is resolved on the simulation clock and is the same without a window and on
  a co-op client, so a sound the game must always hear is `telegraph.sound` or the `Sound` effect, not a clip marker:
  its sound (`AnimationCue` with the sound's name) exists only where a model is drawn, and `audio` without a window will not show it.
- **Mesh looks are content** (`*.look.ron`): the look sets the colour, the light in bands, the flat faces, the outline and the glow; a prefab picks its look with
  the component `"Look": ("hero")`, and the code gives a mesh one white material and `Look::new("stone")` on the mesh itself. Only the
  picture changes (the `render` feature, a window or `--offscreen`), never the gameplay. A `Look` in a body's prefab is an ordinary component (replay,
  snapshot, co-op clients), so a `Look` that only the picture needs is put by the code on a child mesh without `AiId`, not on the
  body; a shared style is a preset (`extends: "low_poly"`); the colour and the outline thickness do not go into game code.
- **Foliage is content** (`*.scatter.ron`): the definition decides what grows where, how densely and in which kinds, and
  the layout comes from the region's seed, the cell and the id through `DetRng` (never `SimRng`). Instances are for the picture only: without `AiId` and
  colliders, under the tile's root; whatever must block walking or mean something in the game is placed in the tile as a prefab or a collider.
  Foliage kinds have no outline (a kind with an outline is a content error: each instance would get its own shell). The ground and
  obstacles for the layout come only from static colliders (`Body::Static`): a platform that moves does not hold the grass.
- **Effects are content** (`*.effect.ron`, the event table is one `*.fx.ron`): the effect sets the number of particles, their life, speed, colours and pool, and
  a combat event (`damaged`, `died`, …) or a clip marker picks it from the table, and the game plays it at its own moment with
  `PlayEffect`. Effects and decals are only pictures (the `render` feature, a window or `--offscreen`): they do not change the world, their entities have no
  `AiId`, and the particle clock is the simulation clock (pause freezes it). Effect numbers are not put in game code.
- **Animation is content** (`*.animset.ron`): an animset sets which clip from which glTF file shows a state or an action, the blending and the markers
  (`cues` → sound and effect); a Blender script builds the models and clips (`tools/blender/humanoid.py`, only `-b
  --factory-startup`, without a window), and its output is committed. `Animated` and the model are only a picture: the game puts them on the body when
  `config.draws()`; the replay does not check `Animated`, and the gameplay does not depend on the clip. An action's offset (`displacement`) with `bake:
  (animset, clip)` is baked from the clip's root bone: **if you change the clip, bake it again** (`cargo sandbox --bake-motion`, tool
  `motion_bake`); otherwise `--check-content` names the stale curve. Do not edit such a curve by hand. A new bake changes
  the gameplay: the reference replays then diverge and are re-recorded.
- **A new engine module** is a crate with a plugin that names itself `add_engine_module(name, version)` (so it is visible in
  `ai/status`), with system sets for ordering and one line in `EnginePlugins`. It describes its state for the AI
  (describe, `journal_event`), and gives its actions as tools (what the developer needs at hand in the window is also an item in the
  developer menu, `add_dev_entry`, which calls the same tool); it takes its numbers from content; it has unit tests, its invariants (each with a control test that
  breaks the state on purpose), a scenario in the sandbox, a replay and a section of the manual (`docs/manual.md`).
- **World, regions and levels (phase B).** The things the gameplay takes from tiles, rooms and levels (colliders, markers, prefabs,
  points of interest) are **top-level entities with a world `Transform`**: that is how physics reads them and how `spawn_region` and
  `spawn_level` create them. A region is changed with tools (`regen`, `region_load`) or with `change_region`, through
  `EngineState::RegionLoading`, inside one call; it is not spawned or removed by hand. The starting region
  (`start_region`) is built before `OnEnter(InGame)`, and its `AiId`s come first.
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
- **NPCs act only through `Intent`** (as the player and the agent do): the engine's NPC brain writes to it in `NpcSystems::Drive`, and does not move
  the body. Behaviour, perception, the director and attack plans are content (`*.behavior.ron`, `*.perception.ron`, `*.director.ron`,
  `*.attack_plan.ron`); a game adds its own logic as `add_npc_condition` / `add_npc_state` by name, not by copying the brain. The state of an NPC
  lives in its components (they are part of the replay and the snapshot), not in resources. Avoiding other bodies also does not move the body: in
  `NpcSystems::Drive`, before `drive_intents`, it edits the route input `NavAgent::desired` (which `drive_intents` writes to
  `Intent`), and its own state `NavAgent::avoiding` and `side`; neighbours are taken in `AiId` order, with no randomness; its numbers are `avoidance` in
  `npc_tuning` or in the behaviour, not constants in code.
- **World state in the snapshot.** New region, level or navigation state outside entities with `AiId` goes in `add_snapshot_resource`;
  what is built from the saved data (the region from its manifest, the navmesh) goes in `add_snapshot_prepare` / `add_snapshot_rebuild`; what
  the world is built around goes in `add_snapshot_check`. Cover it with a test modelled on `crates/sandbox/tests/sandbox/snapshots.rs`
  and `crates/shards_world/tests/world/regions.rs` (a snapshot taken in a region, loaded into a game that does not have it).
- **A third-party crate** is added only after checking that it supports Bevy 0.20 and is still maintained; the version is pinned exactly (`=x.y.z` in
  `[workspace.dependencies]`), and the crate itself is hidden behind the engine's API: its types are not in the public API, and the game does not import it.
  That way, physics (now `parry3d`) can change and Bevy can update, without changes to the game.
- **Steam sits behind the engine's API** (`shards_steam`, the `steam` feature; `steam_sdk` is the real client). There are no `steamworks` types in
  the public API, and nothing that affects gameplay depends on Steam: a relay session is the same as over UDP, and achievements and the cloud
  are not simulation state (they are not part of the replay hash or the snapshot; nothing is unlocked during a replay). An achievement is unlocked
  by the host (`UnlockAchievement`); a client unlocks one only on the host's event. Tests do not start the real Steam: only `FakeSteam`; the file
  `steam_appid.txt` is not created (`--steam-app N` is used instead). A Steam callback whose parsing in `steamworks` may fail on a value
  from Steam is read by hand, as numbers (`impl Callback` in `real.rs`): a panic in a callback stops all Steam callbacks until the end of the run.

## If the game crashed

A panic in a gameplay system does not kill the game. `step` stops on the tick where the failure happened and replies
`stopped: gameplay system <name> panicked at tick N: <msg>`. In real time, the game pauses.
The journal shows `system <name> panicked at tick N`. The session is already saved to `replays/crash-tick<N>.json`:

```bash
cargo sandbox --replay replays/crash-tick<N>.json --headless --paused &   # the same world from tick 0
cargo shards wait && cargo shards step <N>     # the failure repeats on tick N: look at inspect/journal
cargo sandbox --replay replays/crash-tick<N>.json --verify   # exit code 1 while the system still panics
```

After the fix, `--verify` no longer writes `gameplay system … panicked`. Exit code 1 with `DIVERGED` then
only means that the fixed system changes the world differently from the one that crashed.

A violated invariant stops the game in the same way: `stopped: invariant <name> violated at tick N`. The journal
gives the text of the violation. The replay is `replays/invariant-<name>-tick<N>.json`. The `assert_*` rules are
recorded in it, so replaying it breaks the rule on the same tick. `--verify` lists the violations and exits with
code 1 until the bug is fixed.

Along with the crash replay, a crash report is written to `crashes/crash-<time>-tick<N>.json` (`[sim] crash report saved: …`). If the working folder cannot be written to, the report goes into the player's folder, for example `%LOCALAPPDATA%\<program>\crashes`, and the same line names the path. The report holds the location and stack of the panic, the game and the machine (OS, processor, memory, graphics card), `ai/status`, the last lines of the journal and the log, and the same replay.
It is written when a panic happens on another thread (the render thread, a task), when `shards::run` handles a panic that takes the game down (exit code 101, `error: the game crashed: …; crash report: …`; such a report has no replay), and on request, through the `crash_report note="…"` tool. A crash report with a replay that a player sends plays back as a replay: `--replay crashes/crash-….json` (for a report without a replay, it says `… is a crash report without a replay (<why>)`). If you catch the panic yourself and carry on, call `ai_core::crash::handled(&message)`. Otherwise, a report is written about it (`docs/manual.md`, "Crash reports (E10.7)").

## Checking changes

- `cargo test --workspace`: unit tests and scenarios in `crates/sandbox/tests/`; `cargo clippy --workspace --all-targets
  -- -D warnings` must stay silent. If you changed something the template uses, run `cargo test --manifest-path template/Cargo.toml`.
- What CI runs, with `cargo-nextest` (`.config/nextest.toml`, `template/.config/nextest.toml`; profile `ci`: two retries, and a test that
  passes only on a retry **fails** the run, so a flaky test is a bug to fix, not to rerun):
  `cargo nextest run --workspace --locked --profile ci -E 'not binary_id(sandbox::perf)'`, then the tick-time tests alone,
  `cargo nextest run --workspace --locked --profile ci-perf -E 'binary_id(sandbox::perf)' -j 1`, then `cargo test --workspace --doc --locked`
  (nextest does not run doctests); the template: `cargo nextest run --manifest-path template/Cargo.toml --locked --profile ci`. nextest runs each test in
  its own process, so an in-test `static Mutex` serialises nothing there: a test that writes a fixed file name belongs in a test group of
  `.config/nextest.toml` (`snapshot-file`, `shards-snapshots`), with the lock kept for `cargo test`. Without the profile (`cargo nextest run`) there are no retries.
- A scenario a fuzz run shrank is a regression test: `crates/sandbox/scenarios/regress/<YYYYMMDD>-<stop>-seed<S>.scenario`, which fails while its
  bug exists and is committed with the fix (`scenario_files.rs` runs each on a fresh game of its seed, forward only; see "Fuzzing" in `docs/manual.md`).
- Put behaviour that you can describe with `shards` commands into a scenario file in `<crate>/scenarios/`
  (`*.scenario`, format in `docs/manual.md`, "Scenarios (E11)"). The sandbox's `tests/scenario_files.rs` runs each file
  in-process (forward, then back, in one game), and `shards scenario` runs against a live game. The file must run on a fresh game and after
  the others: start it with `heal`/`teleport`/`set`/`npc_set_state … calm`/`regen`, which return the world to its original state.
- Content: `cargo sandbox --check-content` (or `cargo run -- --check-content` in the template) finds broken references
  and names the file. It also reads the level files (nodes, components, markers) and the baked navigation. World: `cargo sandbox
  --worldgen-report wilds --seeds 1..1000` (in the template, `valley`) must return 0. CI runs both.
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
  a replay from a snapshot, a replay of the sandbox's saves and the hit-outcome sparring) are checked by `cargo test` (and by nextest). After a deliberate change to gameplay, or to a definition they read (by id, or a kind
  they read whole: `check_parts.content` in the file lists them), record them again; content they never read, and kinds outside the
  simulation (looks, sounds, effects, animation sets, screens' themes), do not make them diverge:
  `cargo test -p sandbox --test sandbox -- --ignored record_reference_replays` (for the template —
  `cargo test --test game record_reference_replays -- --ignored` in `template/`).
- The long checks are not run by hand but by `.github/workflows/nightly.yml` (every reference replay with `--heavy 10 --heavy-span 30`, worldgen on 100000 seeds, the GPU tests, performance), `soak.yml` (half an hour of a live stand) and `cross.yml` (the reference replays on Windows and Ubuntu 22.04, weekly); a failed night opens an issue labelled `nightly`. Their commands are the ones above with bigger numbers: the nightly worldgen is `--worldgen-report wilds --seeds 1..100000` (and `valley` in the template), not the 1000 seeds of `ci.yml`.
- If you change a module's behaviour, update its section in `docs/manual.md`, `docs/engine-progress.md` (statuses) and `CHANGELOG.md`
  (`Unreleased`); update `README.md` only when what it says about the project changes (features, quick start, steps).
