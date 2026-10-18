# Shards: the manual

> The full engine reference: every module, its tools, content formats and checks. The short project description
> is in [README](../README.md).

Shards is a set of crates that a separate repository uses to build a game without touching the engine code. The game
depends on one crate, `shards`, and gets a deterministic tick, physics and a character controller, input and camera,
data-driven combat, a content database, levels from Blender, generated regions, navigation and enemy NPCs, and an AI
layer. The mechanisms live in the engine, and the content (weapons, enemies, numbers, story) lives in the game. The
engine code has no names from any particular game. The goal is an engine for *Осколки Предела*: the spec is in
[docs/engine-tz.md](engine-tz.md), and the check against it (what is done, what is ahead) is in
[docs/engine-progress.md](engine-progress.md).

The engine is built so that a game can be made together with external coding agents: Claude Code, Codex and similar
tools. The AI does not look at pixels. The engine already knows everything on screen, so it **describes the world as
structured text** and gives **a typed API for actions**. The same seed and the same inputs give the same game, so any
session is recorded as a replay, and that replay checks a new build.

**The engine has no LLM inside it**: no built-in agent, no local model, no API keys, no calls to models. The game is an
environment. An agent starts it, watches it and acts on it from a terminal through the `shards` CLI (JSON-RPC methods
`ai/*` over the Bevy Remote Protocol on 127.0.0.1), with no MCP layers. People, scripts and CI use the same CLI.
Instructions for agents are in [AGENTS.md](../AGENTS.md) (the short core and an index) and the topic files in [docs/agents/](agents/). How to make your own game, step by step, is below and in
[template/README.md](../template/README.md). Versions and how to depend on a tag are in [CHANGELOG.md](../CHANGELOG.md).

![sandbox: the guard winds up, and a red strike zone is on the ground](media/telegraph.png)

## Contents

1. [Quick start](#quick-start)
2. [Make a game on Shards, step by step](#make-a-game-on-shards-step-by-step)
3. [Architecture](#architecture): [crates](#crates) · [tick order](#tick-order) · [the boundary between engine and game](#the-boundary-between-engine-and-game)
4. [Core (E1)](#core-e1)
5. [Content (E4)](#content-e4)
6. [Movement (E2)](#movement-e2)
7. [Actions and combat (E3)](#actions-and-combat-e3)
8. [Levels from Blender (E4.2)](#levels-from-blender-e42)
9. [World and generation (E5)](#world-and-generation-e5)
10. [Navigation and NPCs (E6)](#navigation-and-npcs-e6)
11. [The AI layer](#the-ai-layer): [what the AI sees](#what-the-ai-sees) · [tools](#tools) · [time and determinism](#time-and-determinism) · [groundwork for networking](#groundwork-for-networking-phase-c) · [transports and the CLI](#transports-and-the-cli) · [screenshots](#screenshots) · [agents](#agents)
12. [Scenarios (E11)](#scenarios-e11)
13. [Invariants (E11)](#invariants-e11)
14. [Tick time (E11)](#tick-time-e11)
15. [Developer menu and console (E11)](#developer-menu-and-console-e11)
16. [Replays](#replays)
17. [Snapshots (E11.4)](#snapshots-e114)
18. [Heavy mode](#heavy-mode)
19. [Saves (E8.1–5)](#saves-e815)
20. [Settings (E8.6)](#settings-e86)
21. [UI (E9)](#ui-e9)
22. [Localization (E9.4)](#localization-e94)
23. [Audio (E9.5)](#audio-e95)
24. [Stylised materials (E10.1)](#stylised-materials-e101)
25. [Foliage and small objects (E10.2)](#foliage-and-small-objects-e102)
26. [Effects and decals (E10.3)](#effects-and-decals-e103)
27. [Voxel terrain (core)](#voxel-terrain-core)
28. [Steam (E10.5)](#steam-e105)
29. [Builds for players and Windows (E10.6)](#builds-for-players-and-windows-e106)
30. [Crash reports (E10.7)](#crash-reports-e107)
31. [The sandbox](#the-sandbox)
32. [Tests and CI](#tests-and-ci)
33. [What's next](#whats-next)
34. [Building and iterating fast](#building-and-iterating-fast)
35. [Conventions and building](#conventions-and-building)

## Quick start

```bash
# 1. System packages (once; needs sudo):
sudo apt install pkg-config libasound2-dev libudev-dev
# (+ libwayland-dev libxkbcommon-dev, if you need native Wayland: --features wayland)

# 2. The game without a window, paused: time moves only with `step`. A free port and a file with its number:
cargo sandbox --headless --paused --port 0 --port-file /tmp/shards.port &
export SHARDS_PORT_FILE=/tmp/shards.port

# 3. From the same or another terminal:
cargo shards wait                          # ready: tick 0 · pid 4242 · seed 42 · port 40817
cargo shards observe                       # the world as text, plus the journal since last time
cargo shards tools                         # what you can do (--json: with argument schemas)
cargo shards act attack target=e6 repeat=true
cargo shards step --until journal:winds    # the reply has CHANGES and JOURNAL
cargo shards content check                 # ok: 133 definitions in 39 files
cargo shards act regen region=wilds seed=42   # a generated region south of the arena; the player is at its gate
cargo shards act nav_path to='{"x": 24, "z": 140}'   # a route to the lair over the navmesh: path 94.5m, 4 turns: 33.9m SE → …
cargo shards worldgen --region wilds --seeds 1..1000 --report   # check the generator on a thousand seeds
cargo shards scenario crates/sandbox/scenarios/*.scenario
cargo shards save-replay                   # session from tick 0 → replays/tick<T>.json
cargo shards save-snapshot                 # state at this tick → snapshots/tick<T>.json (load-snapshot returns to it)
kill 4242                                  # pid from `wait`; not kill -9: the game deletes its token and port file

# 4. The game in a window and a screenshot with eN labels (needs a display and a GPU; without a window use --offscreen, see "Screenshots"):
cargo sandbox
cargo shards shot
cargo sandbox --offscreen --paused --port 0 --port-file /tmp/o.port &   # no window: the picture is drawn on the GPU
SHARDS_PORT_FILE=/tmp/o.port cargo shards wait && SHARDS_PORT_FILE=/tmp/o.port cargo shards shot

# 5. Replay: the seed and the tick rate are read from the file.
cargo sandbox --replay replays/tick332.json
cargo sandbox --replay tests/replays/combat_prototype.json --verify
# replay finished at tick 2307: identical to the recording (39 checks) [2307 ticks in 3.46s]
xvfb-run -a cargo sandbox --replay tests/replays/combat_prototype.json --verify --windowed   # the same, in a window

# 6. Content without the game: reads and checks all files (including the tile level files and the baked navigation), exit code 0 or 1.
cargo sandbox --check-content
# ok: 133 definitions in 39 files

# 7. The world without the game: the generator on a thousand seeds (exit code 0 if all pass) and baking the tile navigation.
cargo sandbox --worldgen-report wilds --seeds 1..1000
# ok: worldgen wilds v1, seeds 1..1000: 1000/1000 passed (42 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 0.31s, 0.3 ms/seed]
cargo sandbox --bake-nav               # baked 18 files for 2 body classes (agent, large) into nav/ (after editing a tile; with no edits the files do not change)
cargo sandbox --bake-motion            # offsets for hits and dodge rolls taken from the clips (after editing a clip; see "Offset from the clip")
```

`cargo sandbox` is `cargo run -p sandbox --`, and `cargo shards` is `cargo run -q -p shards_cli --` (aliases in
`.cargo/config.toml`). Every flag after `--` goes to `shards::run` or to the CLI. If you run the built binary directly
(`target/debug/sandbox`), the game looks for `assets/` next to the executable, so set `BEVY_ASSET_ROOT=crates/sandbox`
(at start, the engine makes a relative path absolute from the working folder, and hot reload works with it).

**Rust** is installed with rustup, which adds `~/.cargo/bin` to PATH. The version comes from `rust-toolchain.toml`. In
terminals that are already open, run `source ~/.cargo/env` or open a new one.

**While there is no sudo**, the build works through a local workaround. The `pkgconf`, `libasound2-dev` and
`libudev-dev` packages are unpacked in `~/.local/share/rustengine-sysdeps/`, along with a `pkg-config` shim. Before any `cargo
build/test/run` command, run `source ~/.local/share/rustengine-sysdeps/env.sh`. It is needed because
`bevy_remote` pulls in the chain `bevy_dev_tools` → `bevy_audio` → `alsa-sys`, and `alsa-sys` looks for `alsa.pc`
through pkg-config. After `apt install`, the workaround is not needed.

Controls in the window are the engine's input map (`InputMap::standard`, rebound with the `rebind` tool): WASD or the
arrow keys to walk, the mouse to move the camera, Space to jump, Shift to sprint, F to glide (hold), Ctrl to roll, LMB
for a light attack (combo), RMB for a heavy one, Q to block (hold), C for the flask, Tab for lock-on, E for "interaction";
the gamepad works too.
Developer keys in the sandbox: P — pause, N or `.` — one tick while paused, F1 — overlay with ids, F2 — combat overlay
(the engine's), F3 — colliders, F4 — heal and revive, F12 — screenshot with labels into `shots/`. The keyboard and the
agent's tools write to the same `Intent`, and key presses go through the same input buffer. A strike or roll that could
not start within the buffer time shows in the journal as `rejected`.

## Make a game on Shards, step by step

A game is a separate repository with one dependency, `shards`. The starting point is [`template/`](../template/README.md):
a minimal game (a hero, an enemy, an arena, numbers in content, its own tool, a scenario, a replay, tests). The steps
below are in order. Each one was checked on a copy of the template (renaming, `cargo test`, running without a window,
`shards scenario`, re-recording a replay). The details of each step, the file tables and the stubs for future modules
are in [template/README.md](../template/README.md).

1. **Copy the template and rename the package.**
   ```bash
   mkdir -p ~/games && cp -r template ~/games/pit && cd ~/games/pit && git init
   grep -rIl my_game . | grep -v Cargo.lock | xargs sed -i 's/my_game/pit/g'   # the package `pit` and the settings folder `SettingsAppId::new("pit")`; Cargo.lock updates itself
   ```
2. **Depend on the engine by tag or commit.** In `Cargo.toml`, use a git dependency instead of the path
   (`../crates/shards`). Versions and tags are described in [CHANGELOG.md](../CHANGELOG.md). The template uses saves,
   co-op, screens and texts, so it needs the engine of phases C–E (`0.4.0`). The earlier versions `0.2.0` and
   `0.3.0` will not work. Pin the tag `v0.4.0`:
   ```toml
   [dependencies]
   shards = { git = "https://github.com/larelgit/shards-engine", tag = "v0.4.0", default-features = false }
   ```
   To follow unreleased changes on the main branch instead, pin a commit with `rev = "<hash>"`. The build does not change
   until you change `tag` or `rev` yourself. `default-features = false` is needed so that the game turns on the features (`render`,
   `remote`, `hot_reload`, `audio`, `gamepad`, …) in its own `[features]`. There are no other dependencies: Bevy, serde
   and schemars come through `shards` (`shards::prelude::*`, `shards::serde`, `shards::schemars`). Install the CLI at the
   same version: `cargo install shards_cli --git … --tag v0.4.0` (or `--rev <hash>` for a pinned commit).
3. **`main` is one line.** `shards::run` reads the command line (`--headless --seed --port --port-file --no-remote
   --paused --hz --replay --verify --windowed --check-every --check-detail --tick-budget-ms --frame-budget-ms --check-content
   --worldgen-report --seeds --bake-nav --smoke --perf`), catches `kill` and a closed terminal:
   ```rust
   fn main() -> std::process::ExitCode {
       shards::run(pit::build_app)
   }
   ```
4. **`build_app` builds the engine and the game.** `EnginePlugins::new(config)` is the whole engine from one
   `EngineConfig` (`config` is already filled from the command line), and the game's plugins go on top of it.
   Everything that is only for the window (meshes, lights, camera) is a separate plugin under the `render` feature.
   ```rust
   pub fn build_app(config: &EngineConfig) -> App {
       let mut app = App::new();
       app.add_plugins(EnginePlugins::new(EngineConfig { title: "My game".into(), ..config.clone() }))
           .add_plugins(GamePlugin);
       if config.draws() {
           #[cfg(feature = "render")]
           app.add_plugins(visuals::VisualsPlugin);
       }
       app
   }
   ```
   For tests, `shards::testing::test_app(|config| config.seed = 7, |app| { app.add_plugins(GamePlugin); })`: without a
   window, loaded, and paused at tick 0.
5. **Create the world in `OnEnter(EngineState::InGame)`, not in `Startup`.** Until the content is loaded, `LoadingGate`
   lets no tick through, so loading does not shift the ticks. Entities spawn in the same order, and `AiId` and
   `ColliderId` are the same in every run.
   ```rust
   app.add_systems(OnEnter(EngineState::InGame), spawn_arena);
   ```
6. **Numbers and definitions are content files** `assets/content/**/*.<kind>.ron`, and they change on the fly. A struct of
   your own, with `ContentDef` and `app.add_content::<T>()`, is a new content kind (a level, a weapon, an enemy).
   `require_content` names the ids the code relies on. A check without the game: `cargo run -- --check-content`.
   More in "Content (E4)".
7. **The character is a prefab.** Components are named: `CharacterController` (movement numbers are an id in
   `*.locomotion.ron`), `Fighter` (moveset), `Health`, `Stamina`, `Poise`, `Team`. `PlayerId` and `LocalPlayer` mark the
   one the keyboard and the camera control. `AiPlayer` is the agent's body by default. An enemy is made by extending
   another prefab (`extends: "grunt"`).
8. **Movement, input and the camera** are already in the engine. Keys come from `InputMap::standard` or your own
   `*.input_map.ron` (id `default`), and the player rebinds them with the `rebind` tool. The camera is
   `commands.spawn((Camera3d::default(), AiCamera,
   ThirdPersonCamera::default()))` in the window plugin. The engine
   stores the player's settings itself (keys, graphics, audio, accessibility). Set the name of their folder with
   `app.insert_resource(SettingsAppId::new("pit"))` (otherwise `shards-<window title>`; the template has `my_game`
   there, and step 1 renames it along with the package, since each game has its own), see "Settings (E8.6)".
   `InputSettings` is needed only by a game that already had an old `input.ron`.
9. **The fighter and its moveset are files.** `*.action.ron` (phases, hit windows, invulnerability, cancels, offset,
   cost), `*.moveset.ron` (which input starts which action, `stagger` and `death`), `*.combat_tuning.ron` (id `default`).
   An enemy's brain is a system of its own in `SimSet::Think` that writes an order to `Intent` (`Order::Attack`). The
   engine makes it walk, turn and strike by the same rules as the player (stamina, windup, cooldown).
10. **Your own tool and condition for the agent** (`src/ai.rs`): an argument struct with doc comments, and a system
    `In<Call<Args>> -> ToolResult`.
    ```rust
    app.add_ai_tool("taunt", Scope::Player, taunt)           // action: journal, CHANGES, replay
       .add_ai_condition("arena_cleared", arena_cleared);    // `step --until named:arena_cleared`
    ```
11. **Scenarios and reference replays.** `scenarios/*.scenario` files hold `shards` commands, expectations and invariants.
    The same file runs against a live game (`shards scenario`) and in `cargo test`. A successful session is saved as a reference:
    ```bash
    shards save-replay replays/smoke.json            # from a running game (or from the recording test)
    cargo test --test game record_reference_replays -- --ignored   # re-record the template's reference: tests/replays/smoke.json
    cargo run -- --replay tests/replays/smoke.json --verify
    # replay finished at tick 258: identical to the recording (5 checks) [258 ticks in 0.05s]
    ```
12. **Tests.** One binary, `tests/game`: scenarios on a fresh game, determinism (the same seed gives the same journal, and
    `step 300` equals 5 × `step 60`), replays, content, regions, the manifest (the only dependency is `shards`). The
    template's tests take seconds.
13. **CI.** `cargo test --locked`, `cargo run --locked -- --check-content` (it reads the tile level files and the baked
    navigation too), `--worldgen-report REGION --seeds 1..1000`, and `--verify` for each file in `tests/replays/`. The
    working `ci.yml` is in "The game's CI" in the template, and in `.github/workflows/ci.yml` of the engine.
14. **A new engine version.** Change `tag`, run `cargo update -p shards`, read the "Changed" section in `CHANGELOG.md`,
    and run the tests and replays. Replays recorded on the old version can diverge after a gameplay change; then they are
    recorded again. Snapshots from another minor version of the engine do not load (`force` skips the check).
15. **Region.** Tiles with sockets on their edges (`*.tile.ron`), socket rules (`*.socket_rules.ron`), points of interest
    (`*.poi.ron`) and the region itself (`*.region.ron`: size, fixed cells, points, checks, the fallback map `tile_map`,
    `origin`, `active_radius`). The format and all the fields are in "World and generation (E5)". No code is needed. A
    check without the game:
    ```bash
    cargo run -- --check-content                          # a broken socket, tile or point names the file and the field
    cargo run -- --worldgen-report valley --seeds 1..1000   # exit code 0 if all seeds pass
    ```
    In the game, the region appears after `shards act regen region=valley seed=42` (a loading screen, with the player at
    its gate), and `app.start_region("valley")` in the game's plugin builds it while the game loads. If you change what the
    generator produces, raise the region's `version`.
16. **Enemy NPC.** A fighter prefab with the components `Npc` (behaviour, perception, attack plan) and `Home` (leash). The
    behaviour, sight, attack plan and the token director are the files `*.behavior.ron`, `*.perception.ron`,
    `*.attack_plan.ron` and `*.director.ron` in `assets/content/npc/`. A point of interest with `prefab` places it in the
    region's camp. Your own conditions and behaviour states are `app.add_npc_condition(..)` and `app.add_npc_state(..)`.
    While there is no navmesh, NPCs walk on a local grid. For them to walk on the region's navmesh, bake the tiles
    (`cargo run -- --bake-nav`) and put `assets/nav/` in the repository. The `nav` definition (`*.nav.ron`) makes
    `--check-content` name a stale file. Details are in "Navigation and NPCs (E6)".
17. **Level from Blender.** Objects with custom properties, a glTF export with Custom Properties and Apply Modifiers, a
    file in `assets/`, a `level` definition (`*.level.ron`: the file and the required markers), or a tile with
    `geometry: Level(path: "tiles/ruin.gltf")`. See "Levels from Blender (E4.2)".
18. **How meshes look.** Their colour and light come from looks: a file `assets/content/*.look.ron`, the body prefab with
    `"Look": ("hero")`, and meshes the game makes itself get one white material and `Look::new("stone")`. Only the
    picture changes (feature `render`, window or `--offscreen`), and an edit to the file shows at once. `shards act looks`
    lists the looks and what each one draws. More in "Stylised materials (E10.1)".
19. **Effects.** Sparks, dust and flashes are defined in the file `assets/content/*.effect.ron`. The table `*.fx.ron` sets
    which combat event plays which effect (`damaged`, `blocked`, `died`, …). The game plays
    `commands.trigger(PlayEffect::new(..))` at its own moment. Attack telegraphs on the ground are the engine's decals,
    with no setup. Effects and decals change only the picture: `shards act effects` lists the effects and their pools. More in
    "Effects and decals (E10.3)".

The rules that keep the game deterministic (gameplay in `FixedUpdate` inside `SimSet`, randomness only through
`SimRng`, order by `AiId`, control only through `Intent`, `replay_input`/`replay_ignore`/`add_replay_check`) are in the
section "Determinism: rules for the game" in [template/README.md](../template/README.md). The world adds its own rules:
generation is a pure function (seed, version, content) on `DetRng`, not `SimRng`. The region is changed with tools and
`RegionLoading`, not by hand. New world state goes into the snapshot (`add_snapshot_resource`, `add_snapshot_rebuild`,
`add_snapshot_check`).

## Architecture

```text
            the game (a separate repository)  ·  the sandbox (crates/sandbox) — also a game on `shards`
                               │ depends only on
                               ▼
                     shards (umbrella: EnginePlugins, EngineConfig, run, testing, prelude, TileLevelsPlugin)
                               │
          ┌────────────────────┼─────────────────────┐
     shards_npc           shards_world           shards_level
     enemies, navmesh     regions, generator,    levels from Blender
                          active zones
          └────────────────────┼─────────────────────┘
        ┌────────────┬─────────┴──────┬──────────────┐
   shards_action  shards_motion  shards_content  shards_core
   data-driven    physics,       content base,   config, states,
   combat         input, camera  prefabs         Intent, loading gate
        └────────────┴─────────┬──────┴──────────────┘
                               │ connected to every module
                            ai_core  ──►  CLI `shards`  ◄── agents · CI · people
                               │
           parry3d, gltf, ron, … — only inside the modules, not exposed outside
```

`shards_world` and `shards_level` do not know about each other (`shards::TileLevelsPlugin` connects them); `shards_npc` sits above both:
it takes the tile graph, the active zones and the "region loaded" event from the world, and the tile files for baking from the levels. `shards_dev`
(the developer menu and console) does not know any module: modules register their own menu items in `ai_core` (`add_dev_entry`),
and it calls their tools by name. `shards_look` (how meshes are drawn) knows only `ai_core`, the core and the content: it reaches prefabs through a check it adds to the `prefab` kind (`add_content_check`), and it does not touch gameplay. `shards_scatter`
(foliage) sits above the world and the looks: it scatters instances over the tiles of a region in `shards_world`, finds the ground under them with the physics of `shards_motion`, and colours them with the looks of `shards_look`. `shards_fx` (effects) knows `ai_core`, the core, the content, movement and combat: it reads combat events and attack zones, and it does not change the world. `shards_voxel` (voxel terrain, the core) knows `ai_core`, the core, the content and `shards_look` (a palette names looks, checked with `refs.require`): without `look` (`--without look`) a palette with a `look:` reports that no content kind `look` is registered, as with `shards_scatter`.

### Crates

| crate | what it is |
|---|---|
| `crates/shards` | **The game's only dependency.** `EnginePlugins`, `EngineConfig`, `run` (the standard `main`; also `--worldgen-report`, `--bake-nav` and `--bake-motion`), `testing` (test apps), `TileLevelsPlugin` (levels as tiles), `coop` (networking and saves together: the character travels with the player, the host keeps the world), `prelude`, a re-export of `bevy`, `serde`, `schemars`, `serde_json` and the modules (`shards::world`, `shards::npc`, …) |
| `crates/shards_core` | the core (E1): `EngineConfig`, states, the main menu (`new_game`), the loading gate, `Intent`, `PlayerId`/`LocalPlayer`/`NetRole`, the module registry |
| `crates/shards_content` | content (E4): `ContentDef`, the database, loading and hot reload, prefabs, content tools, `check_dir` |
| `crates/shards_motion` | movement (E2): physics on parry3d, the controller, the input map, the camera, movement tools and queries |
| `crates/shards_action` | combat (E3): actions, movesets, the state machine, hits, resources, statuses, telegraphs, the animation bridge, baking offsets from clips, overlays |
| `crates/shards_level` | levels from Blender (E4.2): glTF with custom properties → colliders, sensors, markers, prefabs, components; the `level` kind, tools |
| `crates/shards_world` | the world (E5): tiles, regions and dungeons in the content, the generator with a socket solver and checks, the manifest, batch reports, loading and changing a region, active zones, streaming of the visuals |
| `crates/shards_net` | networking and co-op (E7). The channel layer (E7.2), without Bevy: `Transport` transports (in memory: `MemoryHub`, deterministic; UDP for LAN), simulated delay and loss (`LinkConditions`, `--net-sim`), `Link` for reliable and unreliable messages, blobs, compression, acknowledgements, RTT and statistics. On top of it, on Bevy: a co-op session (`NetPlugin`: host, client, replication, `net_*` tools) and the stand, a host plus its clients in one process (`Stand`) |
| `crates/shards_save` | saves (E8.1–5): sections and their registry, slots, the `shards-save/1` envelope, migrations, atomic writes with three copies and recovery, `save_now` and autosave, the `save_*` tools, `load_game` and `continue_game` ("Continue" in the main menu); the umbrella crate registers the engine sections (`region`, `position`, `vitals`) |
| `crates/shards_npc` | enemies (E6): the behaviour state machine, perception, the combat director (tokens), the attack planner, the navmesh per tile (`nav`: baking, stitching, routes), agents and leash |
| `crates/shards_settings` | player settings (E8.6): groups for graphics, audio, controls and accessibility, built on Bevy's settings (`bevy_settings`), `settings.toml`, the `settings`/`settings_set` tools, a window for the graphics settings |
| `crates/shards_ui` | UI (E9): screens and themes in the content (`screen`, `theme`), the screen stack, focus and navigation, value sources and button actions, the `ui_*` tools, the `UI` section in `observe`, the main menu (`main_menu`); with the `render` feature, drawing on Bevy UI with AccessKit labels and the loading image |
| `crates/shards_locale` | localization (E9.4): languages in the content (`locale`), texts in Fluent (`assets/locale/<language>/*.ftl`) with parameters and plural forms, a fallback language, the `locale.language` setting and switching the language on the fly, the `locale` and `text` tools, hot reload of the files |
| `crates/shards_audio` | audio (E9.5): sounds, music and the mixer in the content (`sound`, `music`, `audio`), buses with the volume from the settings, voices with priority and a limit, spatial sounds, music in layers by the game's conditions, telegraph and event sounds, the `audio`, `sound_play` and `music_set` tools, the `MUSIC` line in `observe`; with the umbrella crate's `audio` feature, playback through Bevy audio |
| `crates/shards_look` | mesh looks (E10.1): looks in the content (`look`, `*.look.ron`): colour, banded light (`Toon`), `Matte`, `Unlit`, `Lit`, flat faces, an outline of a fixed width in pixels, glow, glTF materials; the `Look` component, the material and the shaders in WESL, the `looks` and `look_outlines` tools, the `looks` field in `ai/status`, two developer menu items; with the `render` feature, mesh styling (window or `--offscreen`) |
| `crates/shards_scatter` | foliage and small objects (E10.2): the `scatter` content kind (`*.scatter.ron`): what to scatter over the tiles of which regions, how densely, on which ground, with which looks, which meshes up close and far away; the layout follows the region's seed (the same in every run), primitive meshes (a grass tuft, a cone, a rock, a box) or glTF, levels of detail through `VisibilityRange`, the `graphics.foliage` setting, the `scatter` and `scatter_set` tools, the `scatter` field in `ai/status`, two developer menu items; with the `render` feature, instances under the tiles (window or `--offscreen`) |
| `crates/shards_voxel` | voxel terrain, the core (G1, A2b): `VoxelVolume` (chunked blocks, the chunk size a parameter, edits for generation, the FNV hash), the `voxel_palette` content kind (`*.voxel_palette.ron`), deterministic noise, the registry of generation steps and checks (`add_voxel_step` with a version, `add_voxel_check`) and `generate`, the stock steps `noise_hills` and `fill_water`, and the merged boxes `column_boxes` and `water_boxes`; no region kind, collision, navigation or meshing yet |
| `crates/shards_fx` | effects (E10.3): effects in the content (`effect`, `*.effect.ron`) and the event table (`fx`, `*.fx.ron`): particles from pools (`max_live` meshes per effect, one mesh and one material, the colour in `MeshTag`), camera-facing quads, cubes and shards, glow, flight that follows a formula on the simulation clock; `PlayEffect`, effects for combat events and clip markers; telegraph decals (`ForwardDecal` with a zone material in WESL: circle, sector, rectangle, "stadium", the fill grows toward the hit), `DepthPrepass` on the `AiCamera` cameras; the `effects`, `effect_play` and `telegraph_decals` tools, the `fx` field in `ai/status`, two developer menu items; with the `render` feature, drawing (window or `--offscreen`) |
| `crates/shards_hitstop` | hit stop (C2): the `hitstop` content kind (`*.hitstop.ron`, an ordered list of rules over the combat events, the first match gives a hold in seconds), the `HitStopped` event and its `hitstop:` journal line on every machine that sees combat, the real-time clock hold alone with a picture (`ai_core::ClockHold`: virtual speed 0, zero ticks, `speed` masked) and the pictures-only hold in co-op (`ai_core::PresentationHolds`, read by the animation bridge), the accessibility settings `hit_stop` and `hit_stop_local_only`, the tools `hitstop` and `hitstop_play` |
| `crates/shards_steam` | Steam (E10.5), behind the umbrella crate's `steam` feature (off by default; `steam_sdk` is the real client on `steamworks`): co-op over the Steam relay (`--steam-host` for a lobby with invites, `--steam-server` for an anonymous game server, `--join steam:<id>`, `+connect_lobby`), the `SteamTransport` transport for `shards_net`, achievements from the content (`achievement`, `UnlockAchievement`), copies of saves in Steam Cloud and their check at launch, a fake of the Steam process (`FakeSteam`) for tests, the `steam`, `steam_host`, `steam_join`, `steam_invite` and `achievement_unlock` tools, the `steam` field in `ai/status`, the `steam` section of the developer menu |
| `crates/shards_dev` | developer menu and console (E11): items from modules (`add_dev_entry`) and from the game (the `devmenu` content kind), a console that takes `tool key=value` lines, the `dev_menu` and `console` tools; with the `render` feature, panels in the window (F10 and `` ` ``), Tab completes names from the schemas, the `dev` input context |
| `crates/ai_core` | the AI layer, independent of any particular game: ids, the world view, the journal, tools, time, replays, transport |
| `crates/shards_script` | no Bevy: the scenario format, running scenarios, parsing `--until` conditions, long agent play (`soak`) and its random inputs; shared by `shards` and the tests |
| `crates/shards_cli` | the `shards` binary, a thin client for `ai/*` |
| `crates/sandbox` | the engine's test arena, built only on `shards`, like a game (see "The sandbox") |
| `template/` | a minimal game to copy: a separate workspace with `shards` as its only dependency |

Modules of `ai_core`:

| module | responsibility |
|---|---|
| `id` | `AiId` (`e42`): a stable, monotonic id that is never reused; tombstones for removed entities. Only the engine assigns ids: `set` and BRP `world.mutate_components` do not change them, and a new insert renumbers the entity (tombstone `renumbered to e99`) |
| `describe` | only explicitly registered components are visible to the AI, each with a short describe; hints under the `YOU` line (`register_ai_advice`) |
| `observe` | the text view: YOU / ALERT / SEEN (by threat) / NOT IN VIEW / LANDMARKS / module sections (`register_ai_observe_section`: `UI`, `MUSIC`) / SINCE, visibility from the subject's eyes; `inspect`, `nearby`, diffs |
| `spatial` | positions relative to the player ("8.4m NE"), line of sight (replaceable: movement casts rays using physics, and `AiOccluder` also works) |
| `journal` | a ring buffer of game events (observers) plus the capture of `warn!`/`error!` from tracing: a line gets the tick it was recorded on and goes into the journal after each tick; at most 64 lines per tick. Lines of the `local` kind (`[local] music: …`) show only on this machine: the host does not send them to clients |
| `tools` | the registry: an args struct and a Bevy system, JSON Schema (schemars), scope, `catch_unwind` |
| `builtin` | built-in tools: observe, inspect, step, set/get through reflection, … |
| `sim` | the tick, pause, a synchronous `step` through FixedMain, stop conditions, halting the game (`Halt`) on a crashed system or a broken invariant, `SimGate` (holds ticks while loading) |
| `invariants` | rules checked after every tick: game rules (`add_ai_invariant`) and session rules (`assert_add`); a violation halts the game, like a crashed system |
| `replay` | records inputs and action calls from tick 0, plays them back, state hashes, the divergence report |
| `perf` | the real time of each tick against the budget: the `ticks:` line in the `step` reply, `perf` entries in the journal, the `perf` field in `ai/status`; not in the hash or the replay |
| `rng` | deterministic RNG: a counter keyed by (seed, tick, AiId, salt) |
| `scenario` | runs scenario files in-process through `call_tool` (`run_file`, `run_str`) |
| `dev` | developer menu items (`DevMenu`, `add_dev_entry`: a line, a console line, or a line based on the world state) and parsing of a console line `tool key=value …` into a tool call |
| `session` | agents: name, scope (Player/Dev), "body", journal cursor |
| `status` | the fields of `ai/status`; modules add their own through `add_ai_status` |
| `failures` | a panic in a gameplay system halts the game instead of killing it: the queue of failures, crash replays |
| `snapshot` | snapshots: the simulation state at any tick to a file and back, a replay from a snapshot (see "Snapshots (E11.4)") |
| `paths` | files are written only to the game's `replays/`, `shots/` and `snapshots/` |
| `remote` | the `ai/*` methods for BRP (the `remote` feature) |
| `highlight` | gizmo highlights and an id overlay on top of the scene (the `render` feature) |
| `shot` | a capture of the window (or of the game's image without a window, `--offscreen`), with `eN` labels and a legend drawn into the image (the `render` feature) |

### Tick order

All gameplay runs in `FixedUpdate`, in one of the `SimSet` sets. Module systems sit in their own sets, so a game system can go "before" or "after" them (`.before(ActionSystems::Hits)`), and the order stays total:

```text
FixedFirst       ai_core: tick +1, apply replay events and calls between ticks
FixedUpdate
  PhysicsSystems::Sync            colliders moved between ticks (`teleport`, `set`)
  SimSet::Input    WorldSystems::Zones           ActiveZone / Dormant around players (before orders)
                   CoreSystems::FollowOrders     Order::MoveTo → Intent
                   ActionSystems::Orders         Order::Attack → Intent (after FollowOrders)
                   NpcSystems::Sense             who is active; sight, hearing, memory (after Orders and Zones)
                   NpcSystems::Decide            behaviour state machine: transitions, state hook
                   NpcSystems::Direct            attack tokens, roles of the waiting ones
                   NpcSystems::Plan              planner: pick an attack, approach, press
                   NpcSystems::Navigate          routes over the navmesh, following, leash → route speed
                   NpcSystems::Drive             avoiding bodies, then what the NPC decided → Intent
  SimSet::Think    game brains: decide what to do and write to Intent (they see NpcBrain and may add to it)
  SimSet::Act      MotionSystems::Intent         Intent → LocomotionInput
                   ActionSystems::Machine        input buffer, start and run of actions (offset, i-frames, block)
                   ActionSystems::Effects        healing, stamina, statuses, game effects
  SimSet::Move     ControllerSystems::Move       the controller is the only one that moves bodies
  PhysicsSystems::SyncAfterMove                  new body positions for hits
  SimSet::Resolve  ActionSystems::Hits           attack windows × hitboxes → damage pipeline → events
                   ActionSystems::Fighters       sprint, recovery, cooldowns, end of actions
                   ActionSystems::Statuses       accumulation and duration of statuses
                   ActionSystems::Deaths         a fighter at 0 health dies
  SimSet::Cleanup  CoreSystems::ClearPresses     presses nobody took are cleared
FixedLast        ai_core: replay checks (world hash), record inputs, journal, failure queue
```

Between ticks (`PreUpdate`) the keyboard, mouse and gamepad write to `Intent`; the camera runs in `PostUpdate` and reads nothing from the simulation. Changing a region and the navmesh are not part of a tick: `regen` and `region_load` are action tools. They run between ticks and, within one call, make the whole transition `InGame → RegionLoading → InGame` (the old region is removed, the new one is built, the navmesh is stitched), so no tick sees a half-built region and the tick does not shift. The tile visuals (`stream`) run in `Update`.

The test `gameplay_systems_have_a_total_order` catches systems without an explicit order in the sandbox, in the engine with movement, and in the combat module.

### The boundary between engine and game

- The engine provides mechanisms and tools: a deterministic tick, physics, the controller, the action system, the content database,
  the AI layer. The game provides content and numbers: weapons, attacks, enemies, bosses, tiles, story, balance, appearance.
- Check: if a mechanism would help a second game with a different story, it goes in the engine; if it is a specific "what", it goes in the game.
- The game depends only on `shards` and does not change the engine's code: it adds its own plugins on top of `EnginePlugins`. The template's test (`manifest.rs`) checks this.
- An engine module is added with one line in `EnginePlugins` and names itself in `ai/status`
  (`add_engine_module(name, version)`).
- Choose a third-party crate only after checking that it supports Bevy 0.20 and that the project is still maintained. Pin its version, and hide the crate behind the engine's API. That way it can be replaced, and Bevy can be updated without changes to the game.

## Core (E1)

The `shards` and `shards_core` crates. Everything needed to run a game: the config, the states, the loading gate, `Intent`.

**`EngineConfig`.** One value builds the engine: `EnginePlugins::new(config)`. `shards::run` fills it from the command
line, and tests take `shards::testing::config()`.

| field | what it does |
|---|---|
| `title`, `window` | the title and size of the window (1280×720) |
| `version` | the game version for crash reports (`None`; the template and the sandbox set `env!("CARGO_PKG_VERSION")`) |
| `headless` | without a window, GPU or sound: CI, servers, parallel runs |
| `offscreen` | without a window, but with a GPU: the cameras draw into an image the size of `window`, which `shot` captures (`--offscreen`, see "Screenshots"). This is not `headless`: the game adds its own view (`draws()`), and the player's settings are not read, as without a window |
| `seed` | the world seed (42 by default): the same seed and the same inputs give the same game |
| `port` | the BRP port for `shards` (`None` — no server) |
| `paused` | starts paused; ticks run through `shards step` |
| `tick_hz` | ticks per second (60) |
| `crash_replays` | a gameplay failure saves `replays/crash-tick<N>.json` and a crash report to `crashes/`; a broken invariant saves `replays/invariant-<name>-tick<N>.json` (see "Crash reports (E10.7)"); a report that the `crash_report` tool asks for is written even without it |
| `check_every`, `check_detail` | ticks between replay checks (60, `--check-every`) and what a check keeps so it can name the divergence: `components`, `entities` or `none` (`--check-detail`); both are written to the replay file |
| `tick_budget` | a tick longer than this counts as "over budget" (`--tick-budget-ms`); `None` — the tick's own duration |
| `frame_budget` | a frame longer than this counts as "over budget" (`--frame-budget-ms`; the `frames` field in `ai/status`); `None` — 1/60 s. Frames exist only in a game that draws |
| `report_perf` | the `ticks:` line under the `step` reply and `perf` entries in the journal; off in `shards::testing::config()` |
| `engine_invariants` | check the engine's own invariant rules after every tick (see "Invariants (E11)"). On by default, off in a build with the `dist` feature, and off with `--no-engine-invariants`; `shards::testing::config()` and `--verify` always turn it on. Read when the plugins are built, so set it before `EnginePlugins::new(config)` |
| `opt_in_invariants` | the engine rules that are off by default, turned on by name (`--engine-invariant NAME`, repeatable): today only `voxel.body_in_solid`. Needs `engine_invariants`; read when the plugins are built |
| `after_loading` | where `Loading` goes after loading (`InGame`; `MainMenu` for a game with a main menu). `menu` decides whether this launch shows the menu, and `start_state()` gives the result |
| `menu` | `MenuLaunch`: `Auto` (the default) shows the main menu only when the game draws (a window, `--offscreen`), with no session and no named slots (`--world`, `--character`); `Show` (`--menu`) always does, even without a window; `Skip` (`--no-menu`) never does. Replays, checks, `--smoke` and `--perf` always bypass the menu |
| `loading_image` | an image under the asset folder (`ui/loading.png`), full screen, while the game loads (`Loading`) and while a region loads for longer than one call (`RegionLoading`), if anything is drawn. There is no content yet at that point, so it is in the config and not in the content; `--check-content` checks that the file exists. `None` — nothing |
| `hot_reload` | apply content files as they are edited (needs the cargo feature `hot_reload`; off in tests and `--verify`) |
| `asset_dir` | the asset folder; the content is in `<asset_dir>/content/`. Relative to the game's folder (`CARGO_MANIFEST_DIR` under cargo, otherwise next to the executable); `BEVY_ASSET_ROOT` overrides it |
| `testing` | a test app: without a window, a logger, Ctrl+C handling or a loop; it is driven through `app.update()` and the tools |
| `embedded` | a game that runs alongside another in the same process (the stand's clients): like `testing`, but with its own BRP port, and its logs go to its own journal (`testing::embedded_config(port)`) |
| `net` | `NetLaunch`: `Off`, `Host { port }` (`--host`, 7777 = `DEFAULT_NET_PORT`), `Join { addr }`, `Stand { clients, udp }` |
| `player_name`, `net_sim` | the player's name for other players (`--name`); network conditions for its own links (`--net-sim latency=150,loss=0.02`) |
| `save_dir`, `character_slot`, `world_slot` | the save folder (`saves`); the character and world slots (`--character`, `--world`) |
| `settings` | `SettingsStore`: `Default` (the platform's settings folder), `Dir(path)` (`--settings-dir`), `Off` (`--no-settings`, tests) |
| `exit_with_host` | a client exits when the session ends instead of going back to the menu: with code 0 if the host ended it, 1 for a refusal, a kick, loss of the host, or a world that is not accepted; 2 if the session cannot be opened (an address that cannot be parsed; a busy port for `--host`). A host (`--host`) does not exit on `net_end` |
| `without` | engine modules the game leaves out (`--without audio,scatter`): their plugins are not added, see below |

**Modules the game leaves out.** `EngineConfig::without` (or `--without`) names modules the way `ai/status` lists them (`engine.modules`), and `EnginePlugins` does not add their plugins: a game without sound does not need `audio`, and a server without graphics does not need `look`, `scatter` or `fx`. You can turn off `level`, `world`, `npc`, `net`, `save`, `coop`, `locale`, `ui`, `audio`, `look`, `scatter`, `voxel`, `fx`, `hitstop`, `steam` and `dev` (`shards::modules::OPTIONAL_MODULES`); `core`, `content`, `settings`, `motion` and `action` are in every game. A module that another one needs can only leave together with it: `--without locale` gives ``--without: `ui` (screens, menus and the HUD) needs `locale`: leave out both (`--without locale,ui`) or
neither``, and `steam` (in a build that includes it) needs `net`. Something that only joins two modules goes with either of them: `coop` goes with `net` or `save`, and levels as tiles go with `level` or `world`. The command line checks the rest as well: `--without net` with `--host`, `--join` or `--stand`, `--without save` with `--character` or `--world`, `--without steam` with the Steam flags (`--steam-host`, `--steam-lobby`, `--stand-steam`, `--steam-server`, `--steam-relay-only`, `--steam-app`, `--join steam:…`), and `--without world` with `--worldgen-report`
is an error with exit code 2; `EnginePlugins` with an invalid `without` panics with the same text. Stand members leave out the same modules as the host: the clients of `--stand N` take its setting, and the child processes of `--stand-udp` get its `--without`. A game that calls the API of a module it left out (`add_ui_action` without `ui`) is responsible for that itself. Without `ui`, the main menu is skipped by default, because there is nothing to draw it with (`--menu` still sets `MainMenu`, and `new_game` leads out of it). The test `a_game_runs_without_each_module_it_may_leave_out` (`crates/shards/tests/engine.rs`) turns off each module (along with the ones that need it) and runs the engine for 60 ticks.

**With or without a window, the plugins are the same.** Without a window, `EnginePlugins` runs with no window, GPU or sound: `WindowPlugin` without a window, no winit, `RenderPlugin` without a backend (the render plugins stay registered), and a 60 Hz loop instead of the event loop. Transforms, assets, scenes, states and physics all run, so gameplay is the same. `--verify
--windowed` checks this: a replay in a window gives the same report as one without a window. `--offscreen` mode is in between: the window plugins without the window itself (no winit, a loop as in headless mode), but rendering and gizmos run on the GPU, and cameras aimed at the main window draw into an image (see "Screenshots"). The game adds its own look (meshes, lights, cameras) when `config.draws()` is true, that is, when there is a window or `--offscreen`. Systems that only display things (the HUD, overlays, smoothing other bodies on a client) run with the condition `ai_core::display::drawn`. Cargo features of `shards`, `sandbox` and the template: `render` (window, 3D, UI, keyboard and mouse; without it the game builds only for `--headless`), `remote` (`ai/*` methods), `hot_reload` (all three by default), `audio` (sound through the output device; the sandbox and the template enable it by default, see "Audio"), `gamepad`, `wayland`, `dynamic`, `trace_tracy`; `shards` and the template also have `dist`, the build for players (see "Builds for players and Windows (E10.6)"), and `shards` has `steam` and `steam_sdk` (see "Steam (E10.5)").

**`shards::run`** is the standard `main`. Flags: `--headless | --offscreen --seed N --port N|0 --port-file F --no-remote --paused --hz N
--replay FILE --verify [--windowed] --check-every N --check-detail components|entities|none --tick-budget-ms MS --frame-budget-ms MS
--check-content --worldgen-report REGION [--seeds S] --bake-nav [TILES] --bake-motion [ACTIONS] --smoke TICKS --perf ID --no-engine-invariants --engine-invariant NAME`; for phase C:
`--host [PORT] | --join ADDR | --stand N [--stand-udp | --stand-steam]` (mutually exclusive; `--stand` only with `--headless`), `--name`,
`--net-sim SPEC`, `--save-dir DIR`, `--settings-dir DIR | --no-settings`, `--character SLOT`, `--world SLOT` (a slot is
1–32 characters `a-z0-9_-`), `--exit-with-host`; `--menu | --no-menu` (the main menu, also without a window, or skipping it, also with a window; see "Main menu" below; `--menu` cannot be combined with replays, checks, sessions and slots); for Steam (feature `steam`, see "Steam (E10.5)"): `--steam-host | --steam-server`, `--steam-lobby TYPE`,
`--join steam:<id>`, `--steam-relay-only`, `--steam-app N`, and `+connect_lobby <id>` becomes `--join steam:<id>`; a game without the
feature `steam` exits with code 2 on them. `--host`, `--join` and `--stand` start a session (section "Co-op session (E7, `shards_net`)");
`--stand-udp` instead of a stand in one process starts a host over UDP and N clients as child processes (only in real time: with `--paused` it exits with code 2), and a typo in `--net-sim` is also code 2 (`latency=15O` does not start the game without a
delay). A busy port exits with code 3 before anything is built. `--check-every` and `--check-detail` set the replay recording, so they cannot be combined with `--replay`. `--tick-budget-ms` takes a number of milliseconds that fits in a `Duration` and is at least 1 ns; `inf`, `1e30` and `1e-10` give a command-line error (code 2), not a panic. `--verify` exits with code 0 (matched, nothing violated), 1 (diverged, a gameplay system panicked or an invariant was violated) or 2 (the replay or its snapshot cannot be read); `--check-content`, `--worldgen-report` (all seeds passed), `--bake-nav` (all tiles baked) and `--bake-motion` (all actions baked) exit with 0 or 1. These modes run nothing (`--bake-nav` and `--bake-motion` load the game without a window, but do not run a tick).
`kill` and closing the terminal end the game through `AppExit`: the token and the port file are removed.
A panic that takes the game down leaves a report `crashes/crash-…json` and exit code 101 (see "Crash reports (E10.7)").
`--offscreen` cannot be combined with `--headless`, `--stand`, `--verify` or the checks (`--check-content`, `--worldgen-report`, `--bake-nav`, `--bake-motion`, `--smoke`); without a GPU it exits with code 2 and a hint, and without the `render` feature it does too. It can be combined with `--perf` (the performance test, see "Frame time and the performance test (E10)"): then the test measures frames too.

**States** (`EngineState`, Bevy States): `Loading` → `InGame` (or `MainMenu`, if `after_loading` names it and this launch shows the menu: `start_state()`), `MainMenu` → `InGame` (`new_game`, `continue_game` or the game itself), `RegionLoading` is the region loading screen: `regen` goes through it (see "World and generation (E5)"). Inside `InGame` there is `PauseState::{Running, Paused}`: the game pause, which in single player also stops the simulation; `shards pause` and `--paused` are developer tools and do not touch `PauseState`. The game hooks its own systems to `OnEnter`/`OnExit` of these states; transitions are written to the journal (`state Loading ->
InGame`).

**Main menu** (`shards_core::menu`). A game with a menu sets `after_loading: EngineState::MainMenu`, and the screen itself is content (`main_menu: true` in `*.screen.ron`, see "UI (E9)"). Only a launch that a person can sit in front of shows it: a window or `--offscreen` without a session and without `--world` or `--character` (`--menu` also without a window, `--no-menu` skips it even with a window). Without a window, in tests, replays, checks and on a stand, the game goes straight to `InGame` and builds the same world with the same `AiId`s as a game without a menu. Time stands still in the menu: entering it pauses the simulation (the menu takes over the pause of the loading gate), and leaving it resumes the simulation unless the game was already paused before (`--paused`); the developer's `step` still runs ticks. The `new_game` tool (player, action; button `tool new_game`) leaves the menu within the call, so the game world it creates in `OnEnter(InGame)` is already there when it answers; outside the menu it refuses (`the game is on already`). `continue_game` from `shards_save` does the same and loads the most recent saves (see "Saves"). The game's code leaves the menu through `shards::core::start_game(world)`. A replay skips the menu: presses in the menu at tick 0 find nothing there and change nothing. If the menu moved the world (`step`, `spawn_prefab`, a region from the menu), the recording at game start starts again from a snapshot of that world (journal: `the recording
starts at tick N: the main menu moved the world`), and the session's replay still converges. The pause key opens `pause` only in the game.

**Loading gate.** Modules register what they must wait for (`app.require_loaded("content", check)`; content registers its own files). While something is not ready, `ai_core::SimGate` lets through no tick, neither in real time nor through `step` (it answers `the game is still loading (content: 2 files left); wait with \`shards wait\``), so loading does not move the ticks: tick 1 always runs on the same world. `shards wait` waits until loading ends.

**`Intent`** is the only input for a character: the keyboard, the gamepad, agent tools, enemy brains and, later, the network and replays write to one component between ticks, and gameplay reads it.

| field | what |
|---|---|
| `move_dir` | where to walk on the ground, length ≤ 1 (up to 0.5 is a walk, 1 is a run) |
| `aim`, `target` | where to look; the lock-on target |
| `held` | buttons held now: `sprint`, `block`, `glide`, … |
| `pressed` | presses since the last tick (`Press { input, dir }`); gameplay takes them, the rest are cleared at the end of the tick |
| `orders` | orders that last many ticks: `Order::MoveTo { target, within }`, `Order::Attack { target, input, repeat }` |

The engine's input names are in `shards::core::input_names` (`jump`, `sprint`, `glide`, `dodge`, `light`, `heavy`, `block`, `parry`, `technique.N`, `use_item`, `interact`, `lock_on`, and `assist_off`, held while the player has turned the attack assist off); the game adds its own. `Intent` is the replay input (the engine makes `replay_input::<Intent>()` itself): changes between ticks are recorded. `PlayerId` and `LocalPlayer` mark players, and `NetRole` (`Single` by default) is groundwork for networking.

**Groundwork for networking (phase C).** None of this changes a single-player game.

- The client does not simulate: each `SimSet` runs with `run_if(simulates)` (`NetRole::simulates()` is everything except `Client`). On a client only the prediction sets `NetSet::PredictInput` → `NetSet::Predict` → `NetSet::PredictEnd` run from `FixedUpdate` (`run_if(predicts)`, only for its own body with the `Predicted` marker; see "Co-op session (E7, `shards_net`) → Prediction and smoothing (C4)"), along with the systems outside `SimSet` and `NetSet`. The sandbox test `net_seams` checks the list of the latter: right now it is `sync_physics` twice.
- `app.add_exit_hook(name, fn(&mut World))`: exit hooks run once, in the order they were registered, when `AppExit` is written (a system in `Last`) or a signal arrives (`shards::run`).
- `SeedLock`: `hold_seed(world, reason)` holds the loading gate on `seed`, and `adopt_seed(world, seed)` writes the seed to `EngineConfig`, `SimSeed` and `SimConfig` and releases it. The start region waits for `seed_ready` and is built from `SimSeed`.
- A region change ends with a `WorldRebuilt { reason: "region <id>" }` event (as does loading a snapshot).
- `player_body(world, n)` returns the body with `PlayerId(n)`; an agent bound to a player finds its body through it.

**`ai/status`** shows the version and the modules: `engine: {version: "0.2.0", modules: [core, content, settings, motion, action, level,
world, npc, net, save, coop, locale, ui, audio, look, scatter, voxel, fx, dev], state: "InGame", pause: "Running"}` and `loading: []`, while
nothing is loading; with a region in the world there is also a `world` field.

## Content (E4)

The game's numbers and definitions live in files, not in code: attacks, prefabs, movement numbers, levels. The files are edited live.

**Content kinds and files.** A content kind is a Rust struct with `ContentDef` (`KIND = "attack"`), registered with `app.add_content::<T>()`. Definitions live in `<assets>/content/**/*.<kind>.ron`, and each file holds one map of "stable id → definition":

```ron
// assets/content/combat/attacks.attack.ron
{
    "sword": (windup: 0.15, recover: 0.25, damage: 25.0),
    "claw": (windup: 0.45, recover: 0.6, damage: 8.0),
}
```

- An id is a non-empty string without spaces or control characters; it is unique within its content kind across all files.
- The engine registers its own content kinds: `prefab`, `locomotion`, `action`, `moveset`, `status`, `combat_tuning`, `animset`,
  `input_map`, levels `level`, world `tile`, `socket_rules`, `poi`, `region`, `tile_map`, `room`, `dungeon`, enemies `behavior`,
  `perception`, `director`, `attack_plan`, `npc_tuning` and `nav`, mesh looks `look` (see "Stylised materials (E10.1)"),
  foliage `scatter` (see "Foliage and small objects (E10.2)"), effects `effect` and `fx` (see "Effects and decals
  (E10.3)"), hit stop `hitstop` (see "Hit stop (C2)"), with the `steam` feature, achievements `achievement` (see "Steam (E10.5)"). The game adds its own: the sandbox has `layout`, the template has
  `arena`.
  `content_list` (the tool) shows the content kinds and the number of definitions.
- `#[serde(deny_unknown_fields)]` on a struct turns a typo into an error with the line number; without it, a typo in an optional field silently keeps the default value.
- `require_content::<T>(&["yard"], "pit")` names the ids the code uses. Without such an id in the content, there is an error, and an edit that removes it is refused.

**Loading and checking.** The `content` folder is loaded through the asset server while the loading gate holds the simulation; then each
file is parsed, and the whole database is checked at once: the schema, repeated ids, references between definitions (`refs.require::<ActionDef>(id,
"field")`). Errors name the file, the line, the definition and the field, and for a typo also the nearest id. Definitions that
parsed still work. Real messages (three typos in a copy of the sandbox content):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/characters/bodies.locomotion.ron:12:9: locomotion `player`: Unexpected field named `walk_spead` in `CharacterParams`, expected one of `radius`, `height`, `walk_speed`, … or `turn_speed_deg` instead
content/combat/fighters.moveset.ron: moveset `player`: inputs.light -> unknown action `light_one` (did you mean `light_1`?)
content/characters/characters.prefab.ron: prefab `skeleton`: unknown component `Helth` (did you mean `Health`?)
3 content errors in /tmp/broken/assets
```

A repeated id names both files (``content/b.attack.ron: attack `sword`: already defined in content/a.attack.ron``), and a file with a wrong content kind names the nearest registered kind (``no content kind `atack` (did you mean `attack`?); registered: …``).

A definition that does not fit its type (an extra or missing field, a wrong value) is an error of its own: it has the line and column inside the file, the kind and the id. The file's other definitions still load, so the list has no errors about references to them, and a reference to the broken one is reported as missing. A typo `dmage` for `damage` in one `light_1` attack from
`player.action.ron` (the file still has `light_2`, `heavy` and others, and the same moveset refers to them):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/combat/player.action.ron:24:77: action `light_1`: Unexpected field named `dmage` in `HitDef`, expected one of `span`, `shape`, `at`, `damage`, … or `group` instead
content/combat/fighters.moveset.ron: moveset `player`: inputs.light -> unknown action `light_1` (did you mean `light_2`?)
2 content errors in /tmp/broken/assets
```

The cause is the first line; the second follows from it. The whole file adds nothing to the database only when its map is broken: a syntax error (an unpaired bracket, no comma between definitions, a cut-off value), an empty id, an id repeated within the file, or an id with a space. That is one error per file (a failure inside a definition names its id). A running game does not stop on errors: they go to the journal, and it runs with what did load (`content loaded: 2 definitions in 1 file, 1 with
errors`); `--check-content` exits with code 1 on any error.

**Reading content.** In systems, use `Content<T>` (`attacks.get("sword")`); outside them, use `ContentDb::get::<T>(id)`. Gameplay does not write content. A read the simulation makes is noted for the replay check `content`; read one definition by its id when that is all you need (going over a kind with `iter` or `ids` makes the whole kind count). Code that runs in the simulation's scope and is not the simulation (a describer, a picture) reads with `peek`, `peek_ids`, `peek_iter`. A kind only the picture, the sound or services outside the session read sets `const SIMULATION: bool = false;` (see "What the content check covers").

**Hot reload** (`hot_reload`, on by default): an edit, a creation or a deletion of a file is checked together with the whole database. A clean edit is applied with the `content_apply` tool, so it is written to the replay on its own tick and
the replay repeats it; the journal shows `content reloaded: combat/player.action.ron (1 definition changed)`. A broken
edit stays in the journal as an error with the file and the line, and the previous version stays in effect:

```text
#7 2 [error] content: content/combat/player.action.ron:15:77: action `light_1`: Unexpected field named `dmage` in `HitDef`, expected one of `span`, `shape`, `at`, `damage`, … ; kept the previous version
```

A file is applied as a whole: while even one definition in the changed file is broken (and also if it was broken since loading), the previous version applies to all of its definitions, including the valid ones, and the edit waits until the errors are fixed.

What the session's simulation read is part of the replay checks: a recording made with other numbers in a definition it read diverges at the next check (that is expected; the reference replays are recorded again), and one it never read changes nothing ("What the content check covers"). How edits apply on the fly: movement and combat numbers take effect from the next tick; levels and prefabs are read when entities are created (new entities, for example after `spawn_prefab`, see the edit, but entities that already exist do not).

**Prefabs** are entities defined as data. Components are named by short names or full paths, and their values are read through
reflection, as in Bevy scenes; each component must have `#[derive(Reflect)]` and `#[reflect(Component)]`. Fields that
`Default` covers (with `#[reflect(Default)]`) can be left out. `extends` starts from another prefab and replaces the components as a whole;
cycles are a check error.

```ron
// content/characters/monsters.prefab.ron
{
    "skeleton": (components: {
        "Name": "Skeleton", "AiKind": ("Skeleton"),
        "CharacterController": (params: "skeleton"), "Fighter": (moveset: "skeleton"),
        "Health": (current: 40.0, max: 40.0),
    }),
    "skeleton_captain": (extends: "skeleton", components: {"Health": (current: 90.0, max: 90.0)}),
}
```

In code: `spawn_prefab(world, "skeleton", transform, None)`; for an agent, the `spawn_prefab` tool.

**Tools.** Player: `content_list`, `content_get`; developer: `content_check` (rereads all files from disk and
checks them without applying anything), `content_reload` (reread and apply, like hot reload), `content_apply`
(apply definitions from JSON; the replay records them and replays them), `spawn_prefab`:

```text
$ shards act content_list
content: 133 definitions in 39 files
  action  23
  animset  1
  attack_plan  3
  audio  1
  behavior  2
  bounds  1
  combat_tuning  1
  devmenu  4
  director  2
  dungeon  0
  effect  6
  fx  1
  input_map  0
  layout  3
  level  1
  locale  2
  locomotion  3
  look  15
  moveset  4
  music  1
  nav  1
  net  0
  npc_tuning  0
  perception  3
  perf  2
  poi  4
  prefab  7
  region  2
  room  0
  save  0
  scatter  6
  screen  3
  socket_rules  1
  sound  16
  status  0
  theme  0
  tile  13
  tile_map  1
$ shards act spawn_prefab id=skeleton x=5 z=5
spawned e15 skeleton at (5.0, 5.0)
CHANGES
  + e15 Skeleton  hp 40/40  poise 20/20  8.6m NE
```

**`--check-content` and `shards content check`.** The first reads the files without the game (for CI: code 0 or 1; a missing `content` folder is an error too, so the check cannot pass without checking anything), the second asks the running game: `ok: 76 definitions in 26
files`. Modules add their own checks: the level files named by `level` definitions (each node, component and required marker), the level files that tiles are built from (they must lie in the assets folder and are read the same way as a `level` file), and the baked tile navigation (with a `nav` definition, a stale or missing file is an error that gives the command
`--bake-nav`).

A tile file is checked like a `level` file: it must be readable, and each problem in its nodes (a component that does not exist, a prefab that is not in the content, a `navlink` that leads nowhere, a property that does not parse) is a tile error with the file name and the node:

```text
content/region/wilds.tile.ron: tile `ruin`: geometry: tiles/ruin.gltf: node `Ruin/Column`: unknown component `Helth` (did you mean `Health`?)
content/region/wilds.tile.ron: tile `ruin`: geometry: `tiles/ruin.gltf`: not a valid glTF file: EOF while parsing an object at line 1 column 30
```

A file that is also named by a `level` definition (like the sandbox's `ruin.level.ron`) reports itself only as `level`: one problem in one file is one error, and the `level` check also covers the required markers. The sockets and navlinks below concern a single tile, and the tile always checks them.

The socket markers of the level file are matched against the tile's sockets in its RON: each `socket` marker stands on the edge of the tile (within a tenth of its size), in the part of the edge where `sockets` lists its kind (and the height, if the marker names one), for the size of each region that uses the tile: ``geometry: tiles/gate.gltf: socket marker `road` (node `Road.E`) is on the east edge, where
sockets.e has `grass` ``. Markers are optional. A module adds a check that needs two content kinds at once with `add_content_check::<T>`. The check is attached to a content kind, and every content check runs it after that kind's `validate`. The `TileLevelsPlugin` of the umbrella crate installs this one. It is the only one that knows both the world's tiles and the level files.

The `navlink` markers of the file are matched the same way against the tile's `navlinks` (see "World and generation (E5)" and "Content formats"): the reachability check of the generator does not open the files and knows a navlink only from the tile's RON, so every navlink of the file is named there, every named one is in the file, and two navlinks with one name are an error (the name tells them apart in `navlinks`). A node `Ledge.Drop` with `marker = navlink`, `name =
drop` and `to = Arch`, added to the sandbox's `ruin.gltf` and not named by `ruin`:

```text
content/region/wilds.tile.ron: tile `ruin`: geometry: tiles/ruin.gltf: navlink `drop` (node `Ledge.Drop`) is not in the tile's navlinks, so the generator's reachability check does not know it: list it with the links groups it starts and ends in, e.g. (name: "drop", from: 0, to: 1)
```

Once it is named in the tile's `navlinks` (`[(name: "drop", from: 0, to: 0)]`), it is no longer an error. As with any edit to a tile or its file, this one requires baking the tile's navigation again (`--bake-nav ruin`).

Not in the content yet: BSN scenes instead of RON (when Bevy has a `.bsn` loader). Levels from Blender are in a separate
section below.

## Movement (E2)

The `shards_motion` crate: physics, the character controller, the input map, the camera and the movement tools. The state in describe
(`ground 3.6m/s`, `air ↑3.1m/s`, `water`, `glide ↓2.0m/s`, `mantle 40%`), events in the journal (`e5 climbs a 1.3m
ledge`, `e5 air → water`, `e5 reached goal`).

**Physics.** Kinematic: it does not simulate forces. It answers questions such as "what does this ray hit, where will this capsule stop, what lies in this volume".

- `Collider` (shape: box, sphere, capsule, cylinder, cone; offset; layers), `Body` (`Static` | `Kinematic`),
  `Volume` (sensor: water, updraft, named trigger) and the events `VolumeEntered`/`VolumeExited`.
- Layers are a bit mask: `WORLD`, `CHARACTER`, `HURTBOX`, `HITBOX`, `VOLUME`, `PROJECTILE`; bits 16..32 are the game's layers
  (`LayerMask::game(n)`).
- Queries are `PhysicsWorld::{raycast, shapecast, overlaps, volumes_at}` and the system parameter `SpatialQuery`; they work from
  systems and from `&World` (AI tools). A hit that starts inside a shape gives distance 0.
- Determinism: each collider gets a `ColliderId` in creation order and never reuses it; all iteration and
  results follow it, not `Entity` (the window adds render entities and shifts `Entity` indices); when distances are equal,
  the smaller id wins. Positions are read from `Transform`, not from `GlobalTransform` (inside a fixed
  tick it is not recalculated): put colliders on top-level entities. `Transform::scale` is ignored: the size goes in
  `Shape`.
- The broad phase is a ground grid (x, z) with four levels, cells of 2, 16, 128 and 1024 m (`physics/broad.rs`). A collider sits in
  the cells its AABB covers, on the first level whose cell is no narrower than the AABB (as a rule, four cells); a collider wider than 1024 m goes into
  a list that every query reads. A query reads the cells of its own AABB; a ray reads the cells of its segment in pieces no wider than a ground cell
  (a ray going down is one piece), with a margin wider than the rounding. The grid only narrows the search: what it finds is checked in `ColliderId`
  order with the same checks that used to run over all colliders, so the answers and their order match a full search (tests compare them on random worlds from a fixed seed; the reference replays are re-recorded byte for byte). Where
  a query would have to read more cells than a full search costs (fewer than 32 colliders, a query the size of the world, a ray across the whole region), or
  the query is not finite, every collider is checked. `PhysicsWorld` is a cache built from the ECS, as it was before: nothing was added to the snapshot, the replay or the hash;
  a body that moves within its own cells costs the grid nothing. A measurement in the release build (`broad_phase_timing`, `#[ignore]`):
  on 8×8 tiles of 96 m (2644 colliders), eight queries from a body and the ground probe take 16 µs instead of 50 (`overlaps` 17 times faster, rays down
  15 times, pushing out 9 times, a capsule step 4 times; a ray across half the region, 300 m, 1.1 times), on 16 m tiles (212) it is the same as a full search.
- **Voxel fields.** `VoxelCollider::new(VoxelField::new(origin, cell, size, |c| solid))` puts a static grid of solid cells (a
  voxel terrain as one entity) into the physics world beside the colliders: one bit a cell, `cell` m on a side, the least corner of
  cell `(0, 0, 0)` at `origin`, in world space (the entity's `Transform` is not read), on `WORLD` unless `with_layers` says otherwise. It
  gets a `ColliderId` from the same counter as a collider and takes its place among the fields in id order; a new `VoxelCollider` on the
  entity replaces the field (an edit is a new field), and removing it or despawning the entity takes the field out. Every query asks
  the fields after the colliders: a field's hit replaces a collider's only when strictly nearer, and of two fields the lower id is asked
  first. A world without a field runs exactly the colliders' code (the reference replays verify byte for byte, heavy mode included).
  A ray walks the cells it crosses (Amanatides–Woo) and tests each solid one as the box it is, so the distance, point and normal are a
  cuboid collider's in its place. After a hit the walk goes on to the cells it reaches at the same distance (through an edge or a
  corner); of the cells met as near (the same `f32`), one met as a box wins over one met only along a seam (below), then the lowest in
  `(x, y, z)` order (x first): what a world of one cuboid collider per cell, with ids in that order, answers. A ray that starts on a
  face of a solid cell and goes in meets it at 0 with that face's normal. One difference from cuboids side by side: a ray lying in the
  plane of a face between two solid cells is inside the solid and meets it, where parry's cuboid ray can slip between the boxes (on
  0.2 m cells, 165 of 361 rays down the inner grid lines of a solid slab did; none on 0.5 m cells). A cast (`sweep`, `shapecast`)
  tests the solid cells under its swept box (an infinite reach is cut at the field's edge); when that box covers more than one cell,
  the cells outside the start box are merged into column runs and equal runs of neighbouring columns into rectangles, so a long cast
  tests a few boxes, not every cell. The union is the same, so the first touch is the same up to rounding; the cells under the start box
  stay single, so a body that starts touching the solid is let out of each cell as with one collider per cell. Among candidates the box
  with the lowest least cell in `(x, y, z)` order wins a tie. The merge is the field's own: `VoxelField::with_cast_merge(false)` (for
  tests and measurements) makes a field whose casts test every cell, exactly a world of one cuboid collider per cell with ids in `(x, y,
  z)` order, to the bit; the mode is part of the field's hash, so the snapshot check and the replay see it. parry's cast now and then
  misses a box it touches near an edge (a merged run box 13.5 mm away, in a test): a merged box the cast misses but may meet (within
  reach and drawing nearer, by convexity of the distance under a translation) is cast cell by cell. `overlaps` and `closer_than` test
  the cells under the shape's box, one penetration a cell, where the field's id puts it. The cost grows with the cells under the
  query's (swept) box, never with the size of the field, as a collider world's broad phase grows with the colliders under it; a cast
  whose box only grazes cells can still lose an ulp-thin sliver of cells at the box's edge (the box is the reach the cast was given,
  and a touch there is a graze at the reach). `PhysicsWorld::has_solid(mask)` says whether any collider or field on `mask` is solid
  (perception's gate), and `len`, `is_empty` and `colliders()` count colliders only.
- **A field has no box: readers ask the hooks.** Code that reads the top or the bottom of what a query hit (how tall a blocker is, where
  the floor under a solid starts) asks `PhysicsWorld::extent_at(entity, point)` (the run of solid cells, contiguous in y, that holds the
  cell of `point`, tried at the point, 0.1 mm below and 0.1 mm above; a collider's box, wherever the point is), `top_under(entity, shape,
  pos, rot)` (the tallest top of the runs of the cells the shape overlaps; a collider's box top) or `solid_span(mask)` (the lowest bottom
  and the highest top of every solid on a mask). Runs, not columns: under a plateau with a tunnel through it, a point in the tunnel's
  floor gets the floor and a body in the tunnel is not blocked by the plateau above. The readers that used a collider's box use them:
  `clearance` and `path_to` (their gate, whether the world has solids at all: `solid_span`; how tall a blocker is, `Terrain::blocker_top`:
  `top_under`), the nav bake's seed (`solid_span`, and `extent_at` to go on below a solid) and `ground_below` in `shards_world`
  (`extent_at` to climb out of a solid). `PhysicsWorld::subset` keeps the fields its second
  predicate keeps (the scatter's solids near a tile take the fields that reach it). **The rule for a new reader:** a field is no
  collider, so `PhysicsWorld::collider(e)`, `PhysicsWorld::by_id(id)` and `ColliderInfo::bounds()` are `disallowed-methods` in
  `clippy.toml`, and each allowed call carries `#[expect(clippy::disallowed_methods, reason = "…")]` on its statement saying why a
  field cannot be what it reads; the source scan `crates/shards_motion/tests/readers.rs` fails on any `.collider(`, `.colliders()`,
  `.by_id(`, `get::<Collider>`, `.aabb_min`, `.aabb_max`, `PhysicsWorld::collider`/`colliders`/`by_id` or `&Collider` type (a query or
  an argument: a field is no entity with a `Collider`) in `crates/*/src` outside `shards_motion/src/physics/` that is not in
  `tests/readers.allow` with its reason, a row for each time a file has the line, and on a row that matches no line.
- A field is not saved and has no `AiId` (as level geometry, the region that spawned it spawns it again); the snapshot check
  `static_colliders` hashes every field (size, cell, origin, cast mode, bits and layers), and a world without one hashes as before. The
  engine rule `voxel.body_in_solid` (`BODY_IN_SOLID`, `body_in_solid`) names a character whose capsule centre is in a solid cell of a
  field it collides with, except on the tick something other than the controller placed it (`MotionState::placed`). It is **off by
  default** (mantle and sloped-step ticks may graze a cell) until fuzz runs on voxel terrain pass (the voxel design, 6.4): the motion
  plugin offers it with `add_opt_in_engine_invariant`, which registers an engine rule only when `EngineConfig::opt_in_invariants` names
  it, and a game run turns it on with `--engine-invariant voxel.body_in_solid` (a name it does not offer is refused, and the flag does not
  go with `--no-engine-invariants`).
- Checked by `crates/shards_motion/tests/motion/seams.rs`: each scenario runs in four worlds, one cuboid collider per cell, the merged
  boxes as colliders, the field, and the field with the merge off (stand on seams, an edge and a corner; one-cell walls; mixed
  heights; steps of 0.3, 0.5 and 1 m; a mantle; tunnels, with the probe, the hooks and `clearance` at 1350 spots in and around
  them against the merged boxes; landing on a seam; a checkerboard diagonal; 16 bodies on rolling hills), and each says what must have
  happened (standing bodies stay put on the ground on every tick, sliders stay on their side of the wall, the checkerboard is crossed
  and climbed). The field with the merge off equals the cells to the bit on every tick; with the merge on, the scripted scenarios stay
  within 3e-6 m and 2.4e-4 m/s of the cells with the same modes. Ground normals are not compared: at a seam or an edge GJK's normal off
  a cell is noise (up to 56° on the checkerboard, positions equal). On the hills (16 bodies, 600 ticks; 40 seeds walked by hand with
  `SEAMS_SEEDS=1..=40`, three in the test), one step from the same state stays within 1e-4 m (at most 7e-5 m) but for two steps on
  seed 6 that went another way (a mantle taken in the field, a step in the cells), and on both the merged boxes as colliders went the
  field's way; the merged boxes take another step than the cells on 279 body-ticks of the 40 seeds. A free-running walk amplifies
  rounding at corners as any chaotic system does (a step-up decided on an edge ride): the merged field and the cells part by more than
  1e-4 m on 33 of the 40 seeds, first at tick 115 to 572 (on seed 7, 10 of 16 bodies from tick 147, one by 32 cm), and the test holds the
  first 100 ticks. The merged boxes as colliders part for more bodies (365 against the field's 117) and first or alone on 39 of the 40
  seeds. With the merge off the field never parts from the cells. The voxel design's criterion (section 15: within 1e-4 m over the free
  walk) is not met as written; that is the lead's call (the design's risk 10).
  `field_timing` (`#[ignore]`, release; the least of seven samples, the medians within 3 %): on 64 m of 0.5 m-cell hills the ground cast
  takes 8.2 µs with the merge, 16.8 µs without and 15.3 µs as column boxes; rays 0.16–0.75 µs (1.5–1.8 times faster than the boxes, the
  sight ray 16 times), `overlaps` 6.7 times faster; the walk cast (0.1 m) and `closer_than` are slower than the boxes, 5.2 against
  4.8 µs and 1.54 against 1.33 µs.

**Why parry3d and not avian3d.** The spec named avian3d as a candidate and required checking Bevy 0.20 support before taking a third-party crate. Checked against the crates.io index (13–14 October 2026): avian3d 0.7.0 requires Bevy 0.19, and Bevy is pinned
to 0.20.0 (the same goes for bevy_enhanced_input 0.26.0 and lightyear 0.30.1). A kinematic controller needs collision queries, not dynamics, so `parry3d =0.31.1` was taken (dimforge geometry, it does not depend on Bevy; avian is built on it too).
It is pinned, lives inside `shards_motion`, and stays out of the public API: the API speaks glam (`Vec3`, `Quat`) and its own
`Shape`. It can go back to avian3d once it supports Bevy 0.20, without touching the game. By the same check, the input map is its own, not
`bevy_enhanced_input`.

**Controller.** `CharacterController { params: "id" }` is a capsule that only it moves (actions such as a roll hand their displacement to it, and a wall stops a lunge). Modes: ground (walking, running, sprinting, slopes up to `max_slope_deg`,
steps up to `step_height`, sticking down on stairs), air (gravity, control in the air, a jump with "coyote time" and a buffer), glider (slow fall, updrafts), water (swimming on the surface, getting out over shallows or by a jump), climbing onto a ledge
up to `mantle_height` along a path checked in advance. Characters do not block each other as they pass: after everyone has moved, in id order, the capsules are pushed apart. A fall from height is the event `Landed {
fall_height }`, plus `Jumped`, `Mantled`, `ModeChanged`. The `Transform` position is the feet; writing to it from outside (`teleport`, respawn) places the character again, with no speed and no fall.

**Parameters are content.** Content kind `locomotion`, a map from id to numbers; missing fields take default values, a typo is an error with its line.
They are read every tick, so an edit to the file changes the next step.

```ron
// assets/content/characters.locomotion.ron
{ "hero": (radius: 0.4, height: 1.8, walk_speed: 2.0, run_speed: 5.5, sprint_speed: 8.0, jump_height: 1.3) }
```

The other fields (`acceleration`, `deceleration`, `snap_distance`, `gravity`, `coyote_time`, `jump_buffer`, `air_control`,
`terminal_speed`, `mantle_height`, `mantle_time`, `swim_depth`, `float_depth`, `swim_speed`, `glide_fall_speed`,
`glide_speed`, `glide_accel`, `turn_speed_deg`) are described in `shards_motion::CharacterParams`.

**Locomotion modifiers (port gap G7, motion half).** `MoveModifiers` is a component every `CharacterController` gets. It is *derived and
stateless*: each tick it is put back to neutral, then the sources add what they want, so nothing is remembered, and the component is
in no snapshot and no replay (the sources are). It holds: `factor` (multiplies every voluntary speed, a swim too), `ground_factor`
(ground and air only: combat pace, wading), `cap` (no ground speed above it, a sprint included), `unsprinted_cap` (a walk or run only: a
lock-on's pace, which a sprint passes), `no_sprint`, `rooted`, `sprint_cost` (a multiplier, 0 is a free sprint), `wade` (the wading factor alone, also inside `ground_factor`), `stance` (a stance's own top
speed) and `acceleration` / `deceleration` (rate overrides). Sources use `slow`, `slow_on_ground`, `wade_by`, `cap_at`, `cap_unsprinted_at`, `forbid_sprint`, `root`, `scale_sprint_cost`, `stance_speed`,
`accelerate_at`, `decelerate_at`: factors multiply, caps take the lowest, a stance takes the lowest top speed, the last rate wins.

Order inside `SimSet::Input` (after the orders, before the NPC brains), so every reader sees this tick's value:
`ModifierSystems::Reset` → `Contribute` (`Wade`, then `Statuses`, then the game's systems, which must be ordered among themselves:
`.in_set(ModifierSystems::Contribute).after(ModifierSystems::Statuses)`). A client's own body does the same in `NetSet::Predict`
(`PredictReset`, `PredictWade`, `PredictStatuses`, `PredictContribute`); a game adds its source to both. A source that reads an
action sees last tick's end state.

The controller composes for walking and air control: `speed = min(min(ground_speed, cap), unsprinted_cap unless sprinting) * factor *
ground_factor` (with `caps_scale_with_input` the caps are ratios of the full-input speed), or, under a `stance`, `top * stick * wade` (no `factor`, no `ground_factor`, no sprint); a swim is `swim_speed` (or `swim_fast_speed`) `* factor`; a root zeroes voluntary motion exactly like an action's
`locked` (no walking, jumping, gliding, climbing; gravity and `external` still apply). All neutral takes the plain path (replays
are unchanged). The NPC route follower (`top` speed), the animation picker (`BodyStatus::pace`: a slowed run still plays the run clip,
slower), the action machine and `update_fighters` (sprint cost) read the same component.

*Statuses.* `StatusDef` has `speed_factor`, `speed_cap` and `root` (all optional); an active status adds them (`shards_action::
statuses_to_modifiers`). *Wading* is in the params: `wade_min_depth`, `wade_full_depth`, `wade_factor` (1: off); the walk slows in a straight
line with the depth of water over the feet, and a swimmer does not wade.

*Composing a typical action-game pace* (`speed = base * pace * stat * wade * stick`, base 6.7 sprinting, else 4.5 locked on, else 5.0; a guard walks at `3.0 * wade * stick`, a
drinking walk at `2.5 * wade`; none of the stances is scaled by the stat slow or the pace): content `walk_speed` 2.5, `run_speed` 5.0 (so the stick maps linearly),
`sprint_speed` 6.7 with `sprint_scales_with_input`, `caps_scale_with_input`, `wade_*`; game sources: combat `slow_on_ground(0.85)` (not while guarding
or swimming), lock-on `cap_unsprinted_at(4.5)` (a ratio of the full-input speed with `caps_scale_with_input`, so a 0.7 stick walks at `4.5 * 0.7`), guard `stance_speed(3.0, true)` (the status slow and the pace do
not touch it, wading does, a sprint does nothing), a drinking walk `stance_speed(2.5, false)`, out of combat `scale_sprint_cost(0.0)` (a free sprint still holds the stamina
regeneration back), chill `speed_factor: 0.75`. The caps come before the factors, so a locked-on walk is `4.5 * 0.85`.

*Which factors reach which state.* The table is what a game that must match an exact rulebook reads off its own source, then expresses with the options:

| state | speed is | options |
|---|---|---|
| free walk or run | `base * pace * stat * wade * stick` | `factor` (stat), `ground_factor` (pace, wade), `walk_speed`/`run_speed` |
| sprint | `sprint * pace * stat * wade * stick` | `sprint_scales_with_input` (else full speed above half a stick) |
| locked on | like a walk, with a lower top speed | `cap_unsprinted_at`, `caps_scale_with_input` |
| guard, drinking walk | `top * wade [* stick]` only | `stance_speed(top, by_input)`, `accelerate_at`/`decelerate_at` |
| swim | `swim * stat * stick` (no pace, no wade, no cap) | `factor` only |
| dodge, a push | fixed | `external` |

*Acceleration.* By default the speed steps along the straight line to the target, at `acceleration` when the target is at least as fast as the body and `deceleration` otherwise, and the air
keeps its horizontal speed with no input. Options, all off by default: `axis_stepping` (x and z each step toward their own target, as some action games do),
`brake_only_without_input` (`deceleration` only with no input; with input `acceleration` always, also toward a slower target), `air_deceleration` (braking in the air with no
input; `air_control` 1 makes the speed-up there the full `acceleration`), and per-stance rates from a source: `MoveModifiers::accelerate_at` / `decelerate_at` replace the params
for that tick (ground and air, also the air braking); the last writer wins, so order the sources.

*Sprint in the air.* The sprint flag (the speed and the stamina drain) is decided on the ground and in the water. With `sprint_in_air` the air decides it every tick as well, by the same
rule: pressing sprint in the air starts the sprint and letting go ends it on that tick (the take-off value no longer lasts the whole jump), and a glide is no sprint. The host and the client's
prediction both read the flag from `MotionState`, so they agree. Off by default.

*Statuses.* A body that takes a status but is not slowed by it (a large enemy that gets a chill buildup and its other effects but keeps its speed) carries the marker
`NoStatusSlow`: the `speed_factor` and `speed_cap` of its statuses are skipped, a `root` is not. `StatusResist` is for a body that should not take the status at all. A game can also leave `speed_factor`
unset and contribute the slow itself from a `Contribute` system that reads the statuses and the kind of the body.

*Wading depth.* The depth wading reads is `surface - feet + wade_depth_offset` (never below 0), for a game that measures submersion at a reference point under the soles: the value
to use is the gap from that point to the floor of a standing body, that is its distance below the capsule bottom plus `skin_width` (a point 0.1 m under the capsule bottom with
the default skin is `0.12`). `wade_in_air` lets the water over the feet slow an airborne body too (a glide is not slowed: modifiers do not scale it). A reference point fixed under the capsule bottom is a skin closer to the feet in the air than for a standing body (the feet
rest a skin above the floor only on the ground), so `wade_air_depth_offset` (default: the standing `wade_depth_offset`) sets the airborne value, `wade_depth_offset - skin_width` for such a point. `move_info` shows `wade_depth` (the depth used), `wade_factor`, `wade_depth_offset`, `wade_air_depth_offset` and `climb_out_reach`, and no wading outside the modes
that wade. Wading and the offset affect only the speed, not floating, swimming or landing.

*Stamina after a dry sprint.* `CombatTuning::sprint_resume` (land) and `CharacterParams::swim_fast_resume` (fast swim) are a `SprintResume`: `OnRelease` (the default: the button must be let
go, then any stamina is enough) or `Fraction(f)` (back once the stamina is at least `f` of its maximum, button held or not; letting go does not open it). `Fraction(0.25)` is a lock that
opens at a quarter; `Fraction(0)` has no lock at all: the sprint stops at an empty bar and goes on as soon as there is any stamina. A lock set by one rule is consulted only by a rule that has a lock,
so a fast swim lock survives a walk on land, and a dry land sprint under `Fraction(0)` does not lock the next swim. The bar counts as empty at `<= 0.0001`, not exactly 0.

*Limits to know:* a status slow also applies under a cap (a game exempts a state by not adding the factor, or with a stance); caps do not apply to a swim.

*Controller options for an exact rulebook (C4).* Each is content, off by default and unserialized when off, so content hashes and the reference replays do not change.

- *Turning speed by who asks.* `turn_speed_deg` is the free rate. `locked_turn_speed_deg` (0: the free rate) applies while the input names a facing: a lock-on target, an aim point, or a running action's own facing (`LocomotionInput::face`). An action can set its own rate with `ActionDef::turn_speed_deg` (a raised guard); it reaches the controller through `MoveModifiers::turn_speed_deg` (`turn_at(deg)`, the lowest source wins), filled by `actions_to_modifiers` from the action the last tick left, so the rate takes hold one tick after the action starts (a client's own body reads the action it shows, its `PredictedAction`, and none while it shows none, as the prediction moves it). A swimmer keeps `swim_turn_speed_deg`. The order is: swim (in water, when set), then a stance's rate, then the locked rate, then the free one. These are rates of the body only: an action's `turn` spans (B2a, "Turning" under the action timeline) turn the run's own facing, where its hits, guard and displacement point, and the controller turns the body toward that facing once a tick at the rate above. On a tick where both apply, each bounds its own: the run turns at the spans' rate, the body follows no faster than its rate, and nothing turns the body twice. An action with turn spans (and `face: Run`) names a facing on every tick, so its body follows at the locked rate when that is set; a rate below the spans' leaves the body behind its swing. `turn_toward_input` makes the body turn toward the movement input while there is one, not toward its velocity, so a body pressed against a wall (velocity held at 0) still turns to face it; with no input it turns toward its velocity as before. `move_info` shows `turn_free_deg`, `turn_locked_deg`, `turn_swim_deg`, `turn_toward_input` and the stance's `turn_speed_deg`.
- *Water entry, in closed form.* `entry_max_speed` (m/s, 0: no brake) scales the horizontal velocity down to that speed once, in the tick the water takes the body (a run or a jump into a pool). `buoyancy_omega` (1/s, 0: the first-order drift to the float line and an entry fall held to `swim_speed`, as before) replaces the drift by a critically damped spring toward the float line, stepped by its exact solution `x(dt) = (x0 + (v0 + w x0) dt) e^(-w dt)`, `v(dt) = (v0 - w (v0 + w x0) dt) e^(-w dt)` through `ops::exp`: no step-by-step integration, so the plunge does not depend on the tick rate. With the spring the entry fall is not held to the swim speed, so a dive plunges past the float line and rises back to it; `water_max_vertical_speed` (0: `swim_speed`, so set it above `swim_speed` to let a dive plunge; with only `buoyancy_omega` the fall is still held to `swim_speed`) bounds the speed and the travel of a tick, and `water_fall_drag` slows a fall in the water as `exp(-drag t)`. Entering the water already takes the body out of the air and the ground (`MotionMode::Water`), so there is nothing to force.
- *Climb-out.* `climb_out_min_facing` (a dot product, 0.5 is within 60 degrees; `None`: no test) needs the body to face the ledge (the direction into the wall, opposite its normal) for both the push and the jump; a land mantle is never tested. A swimmer that does not face the ledge does not climb and keeps swimming, in control; there is no search to the sides (set `turn_toward_input` so that pressing against a ledge turns the body to it). `climb_out_time` (`None`: `mantle_time`) is the length of a climb that starts in the water.
- *Crawl lock.* The fast swim's lock that ends at a fraction of the bar with no sprint input is `swim_fast_resume: Fraction(0.25)` (see "Stamina after a dry sprint"); the lock arms when the crawl runs the bar dry and opens by itself once the bar is at a quarter, whether or not the button is held, so no emulation of a check that runs only while crawling is needed.

Tests: `crates/shards_motion/tests/motion/controller_parity.rs`, the unit tests of the spring in `locomotion.rs`, and `crates/sandbox/scenarios/player_controller_parity.scenario` (the scenario adds and removes its own definitions with `content_apply`).

*Swimming.* `swim_fast_speed` (a sprint in water) costs `swim_fast_drain` stamina per second, always (the sprint cost multiplier does not
apply) and ends at no stamina or `no_sprint`; `swim_acceleration`, `swim_deceleration`, `swim_turn_speed_deg` (0: the ground's numbers).
Climbing out of water has two tiers. With `climb_out_auto_height` (above the water surface), pushing against a ledge climbs it by itself up to that height. A jump climbs up to `climb_out_max_height`
**above the surface** when that is set (the probe reaches that high and the controller, the `jump` tool, `clearance` and the planner agree through `CharacterParams::can_jump_out`), so a game whose ledge window is
"up to 1.3 m over the water" sets it without raising `mantle_height` (which would also raise the climb on land); without it the jump uses `mantle_height` above the feet, that is `mantle_height - float_depth`
over the surface. `climb_out_reach` (default 0.3 m) is how far in front of the body the ledge face may be for the water climb-out: the controller's probe and the `jump` tool both read
`CharacterParams::climb_reach()`, and a larger value also closes the gap to the wall before the step over, so a swimmer half a meter off a deck climbs out. Not covered: a search to the sides of the swimmer (the facing test is `climb_out_min_facing`, see C4 below). Pushing never climbs higher than `climb_out_auto_height`. *Landing:* `Landed` has `tier` (how many of `landing_tiers` the impact speed reached) and `jumped`
(`landing_drop_tiers` for a fall that did not follow a jump; the controller marks a jump's flight with `JumpedFlight` only when tiers are set, so tiers added by hot reload while a character is airborne apply from its next jump);
`landing_wade_limit` drops the event where the water over the feet is deeper. The journal says `e5 landed from 0.4m at 4.9m/s, tier 1 (jump)`.
Coyote time is `coyote_time` (0.12 s by default).

For the AI: `observe` shows `slowed x0.60, rooted, capped 3.0m/s, no sprint, sprint cost x0.00` after the entity's motion; the query tool `move_info`
(`id` optional) gives the modifiers, the walk / run / sprint / swim speeds they leave (a rooted body swims at 0) and the wading depth and factor. Scenario:
`crates/sandbox/scenarios/modifiers.scenario`.

**Input.** The action map turns the keyboard, mouse and gamepad into an `Intent` for the entity with `LocalPlayer`. It is read between ticks (`PreUpdate`); `Intent` is written only when something changes (the replay records every change).

- `InputMap { contexts }`: context (`gameplay`, `menu`, or the game's own) → actions → bindings. A binding is one of: `Key`, `Mouse`,
  `GamepadButton`, `GamepadAxis` (dead zone, scale), `KeyAxis`, `Keys2D` (WASD as a 2D axis), `Buttons2D` (the gamepad's D-pad as a 2D axis), `Stick`,
  `MouseMotion`.
  `Stick` has `deadzone` (radial, the rest rescaled to 0..1), `scale` for both axes, and three optional fields for stick look (C9): `scale_x` / `scale_y` (`Some(3.2)`; a per-axis scale where set, `scale` where absent) and `curve` (the exponent on the magnitude after the dead zone, `m -> m^curve`, direction kept; default 1.0 is linear; it must be above 0 and finite, otherwise a content error names the file; `rebind` and a saved `controls` string are refused the same way). A look stick that turns 3.2 units per second of yaw and 2.0 of pitch (the scale is in the `look` action's units per second) with a 1.6 response is `Stick(stick: Right, deadzone: 0.2, scale_x: Some(3.2), scale_y: Some(2.0), curve: 1.6)`. The new fields are not written while they are default, so every old `input_map` file and saved `controls` string reads and writes as before; `rebind` takes the same text.
  Action mode: `Press` (one press), `Hold` (while held), `Toggle` (a press turns it on until the next press: "hold or press" for accessibility). `directional: true` — a press carries the direction the player is moving in (a roll towards where the stick points). Slot 0 is the main binding; slot 1 and up are spares.
- The context stack `InputContexts`: only the top one fires; `menu` blocks `gameplay` until it is removed.
- Defaults come from content (`*.input_map.ron`, id `default`, hot reload) or from the built-in
  `InputMap::standard`:
  ```ron
  // assets/content/controls.input_map.ron
  { "default": (contexts: [
      (name: "gameplay", actions: [
          (action: "move", bindings: [Keys2D(up: KeyW, down: KeyS, left: KeyA, right: KeyD), Stick(stick: Left)]),
          (action: "jump", bindings: [Key(Space), GamepadButton(South)]),
          (action: "sprint", bindings: [Key(ShiftLeft)], mode: Hold),
          (action: "dodge", bindings: [Key(ControlLeft)], directional: true),
      ]),
  ]) }
  ```
- Rebinding: `rebind` (developer) puts a binding into an action's slot, changes its mode (`mode`) or clears the slot (`unbind`).
  A binding that another action already uses is rejected with that action's name, unless you pass `force`:
  ```text
  $ shards act rebind action=jump binding='Key(KeyE)'
  error: Key(KeyE) is already bound to `interact` in `gameplay`; rebind `interact` first, or force it to take the input over
  $ shards act rebind action=interact binding='Key(KeyG)'
  ok: gameplay/interact: slot 0 = Key(KeyG)
  $ shards act rebind action=jump binding='Key(KeyE)'
  ok: gameplay/jump: slot 0 = Key(KeyE)
  $ shards act rebind action=sprint mode=Toggle
  ok: gameplay/sprint: mode toggle
  ```
  For the player there is a screen: a `Binding` widget in the menu content shows an action's inputs and waits for a new key, mouse button or gamepad button; it does not take an input that is in use, but names the action that holds it (see "UI (E9)", "Keys").
- Saving: the player's changes are settings of the `controls` group (`ControlSettings`), and Bevy's settings save them to `settings.toml`, see "Settings (E8.6)". The file holds only what differs from the defaults (`"gameplay.jump" =
  ["Key(KeyJ)", "GamepadButton(South)"]`, mode in `hold_or_toggle`), so new defaults reach old files too.
  `rebind` and any change to `InputMap` write the difference to the settings (to disk a second later), while `settings_set` and re-read defaults build the map again; the contexts and actions that the game added to the map itself stay
  (the map is rebuilt only from changed keys and modes, not from the defaults as a whole).
  `settings_set` for `controls` checks the same things as `rebind`: the action and context must exist (the error names similar ones and gives the list), no other action of the context may already use the input (`force=true` on `rebind`, the developer tool, takes it by force; for `settings_set` free the input first: the refusal names the call), and putting an entry back to its default must not take an input that another action already uses. It takes the game's own actions and contexts into account, but they cannot be set in the settings; a context with a dot in its name is a content error (`context.action` cannot be split at the dot). `rebind` and
  `input_bindings` right after `settings_set` (with no frame between the calls) see its result. A file entry that cannot be read or does not fit (an action the map does not have, a typo in a key, an unknown mode) does not stop the others: each one is named in the journal, the rest apply, and at the next save the entry stays in the file as it is, so the player does not lose what they wrote. The journal names an input that clashes in the file.
  The old file `input.ron` (`<user settings folder>/<game>/input.ron`) is moved once, and only if the game asked for it: `app.insert_resource(InputSettings::for_game("my_game"))` (`app_name` is the old game's folder, `config_dir` replaces the user folder in tests). So a game without this line does not pick up someone else's shared `~/.config/shards/input.ron`, and the move happens only in a window, where the settings live in the platform folder (not with `--settings-dir`, not in tests and not in a headless run). If the settings do not have controls yet and the file can be read, its content goes into `controls.bindings`, and only after `settings.toml` is written and holds them is the file renamed to `input.ron.imported`; in the journal: `settings: imported the controls from …`. A file that cannot be read or could not be written stays as it is, and the journal gets a warning. An explicit `InputSettings.file` is always read and is never renamed without the settings being written.
- `accessibility.hold_to_toggle` turns hold actions (`sprint`, `block`, `glide`) into toggles: a press holds, the next one releases. The map does not change (its mode stays the same, and `input_bindings` says so).
- `input_bindings` shows which input controls what:
  ```text
  $ shards act input_bindings
  gameplay (active)
    move         Keys2D(up:KeyW,down:KeyS,left:KeyA,right:KeyD) · Keys2D(up:ArrowUp,…) · Stick(stick:Left,deadzone:0.15,scale:1.0)
    jump         Key(Space) · GamepadButton(South)
    sprint       [hold] Key(ShiftLeft) · GamepadButton(LeftThumb)
    …
  other contexts: menu (input_bindings context=NAME)
  ```
- A real gamepad needs the feature `gamepad` (gilrs, needs libudev). Without the feature, `Gamepad` entities work if the game or a test creates them.

**Camera** (`ThirdPersonCamera`, only with a window; the maths without one). Orbit with the mouse and the right stick; collision with walls (a ray from the head to the desired position pulls the camera in); lock-on: the `lock_on` key picks the target closest to the centre of the frame within the cone and range of `CameraSettings::{lock_cone_deg, lock_range}`, a look to one side switches to a neighbour, a target that leaves the range, dies or loses its `LockOnTarget` ends the lock; aim assist, and shake (`AddTrauma`; its strength, and the field of view, sensitivity and Y-axis inversion, come from the player's settings, see "Settings (E8.6)"). The chosen target is written to `Intent.target` between ticks (it is an input, and the replay records it), but the camera never clears someone else's target. **The camera is only a view:** gameplay and `observe` do not read it (`observe` sees through the eyes of the subject), so a window with the camera plays the same game as one without.

**Camera numbers and offsets (C7).** The camera's feel numbers are not constants in code (the two exceptions are named below: the 30..120 field-of-view limit and the shake's noise frequencies). They are the resource `CameraTuning` (content kind `camera`, `*.camera.ron`, one map of ids to definitions; the camera reads the id `default`, hot-reloaded; without one it runs on the code defaults, which are the numbers it always had, so a game without a camera definition sees no change; a definition that is removed puts the defaults back; a game may also set the resource from code, and content that exists wins whenever it loads or changes). Missing fields take the defaults, unknown ones are an error, and `--check-content` names the file and the field of an out-of-range number. A default serializes to nothing, so adding a field never changes the hash of a definition that does not set it.

| field | default (the old constant) | meaning |
|---|---|---|
| `pitch` | `(-0.45, 1.35)` (`PITCH`) | lowest and highest tilt, radians, positive looks down from above; within ±(π/2 − 0.02) |
| `follow_rate` | `None`: each camera's `ThirdPersonCamera::follow_rate` (14) | how fast the focus catches up with the character, 1/s |
| `release_factor` | `1.3` (`RELEASE_FACTOR`) | a lock the camera picked lets go at this many times its range |
| `snap_distance` | `12` (was a constant) | a followed character farther than this from the focus (m) is a jump: the focus lands on it at once |
| `body_snap_distance` | `4` | a body that moved farther than this (m) in one tick was placed, not walked: render interpolation starts over there (see "Render interpolation (C8)") |
| `framing.yaw_rate` / `pitch_rate` | `7` / `7` (`LOCK_FRAMING_RATE`) | how fast a locked camera turns to frame its target, 1/s |
| `framing.pitch_target` | `None`: the pitch of the line from the target to the focus | a fixed pitch to frame at |
| `framing.pitch_clamp` | `(0.15, 0.7)` | the range that geometric pitch is held to |
| `framing.min_horizontal` | `0` (always frames) | nearer than this to the target (m, on the ground, from the focus) the camera stops framing |
| `arm.radius` / `arm.min_distance` | `0.25` / `0.7` (`CAMERA_RADIUS`, `MIN_DISTANCE`) | the sphere a wall pulls in; never closer than this to the focus |
| `arm.follow_rate` | `None`: at once | how fast the arm grows back after a wall let go of it, and how fast its nominal length follows `ThirdPersonCamera::distance`, 1/s. It always shortens at once |
| `flick.mouse_threshold_px` | `14` (`FLICK` 0.035 rad at the standard map's 0.0025 rad/px) | horizontal mouse pixels, summed, that switch the lock-on target |
| `flick.mouse_decay_px_s` | `None`: one frame's motion only | how fast the sum drains, px/s |
| `flick.cooldown_s` | `0.35` (`FLICK_COOLDOWN`) | seconds before the next switch |
| `flick.cooldown_on_miss` | `false` | whether a flick that found nobody starts the cooldown |
| `flick.stick_threshold` | `0.75` | right-stick deflection (0..1, after the dead zone) that switches |
| `flick.stick_rearm` | `None`: no latch | the stick must come back below this before it can switch again; a lock taken with the stick already over starts disarmed |
| `trauma.decay` | `1.4` (`TRAUMA_DECAY`, linear) | trauma drained per second |
| `trauma.shift_m` / `pitch_deg` / `yaw_deg` / `roll_deg` | `0.12` / `2` / `2` / `3` | the most a shake of trauma 1 moves and turns the view (scaled by trauma squared) |

The ranges of `pitch`, `follow_rate`, the rates, thresholds and amplitudes are content errors when out of range (a rate must be above 0, a re-arm level below the threshold, and so on). `framing.pitch_clamp` must not have its low end above its high end; it may be wider than `pitch`, since the framed pitch is then held to the pitch limits like any other. Two things stay constants on purpose: the engine's 30..120 limit on the field of view (the player's setting is clamped to it too) and the shake's noise frequencies (the look of the noise, not a feel number).

*Smoothing.* Every smoothing is the exact exponential `1 - exp(-rate * dt)` through `bevy::math::ops::exp`, so it does not depend on the frame rate (`damp`, `lock_framing`, `arm_reach`). `critically_damped_step(x, v, target, omega, dt)` is the closed form of a critically damped spring (for a landing dip: kick `v`, aim at 0); one step of `2 dt` equals two of `dt`. *The arm*: a wall pulls it in at once; once it lets go the shortfall below the nominal length shrinks at `arm.follow_rate` (the rate acts on the shortfall, so a plain zoom is smoothed once, by `arm_length`; `ThirdPersonCamera::arm_reach` is where it stood last frame, `arm_length` its nominal length), never past what is free and never closer than `arm.min_distance` (as far as the room allows). A jump of the focus (a teleport) resets it.

*Flick.* The flick thresholds are in raw units, so they are the old constants only at the standard map, mouse sensitivity 1.0 and 60 Hz: before, the mouse flick read the scaled, sensitivity-adjusted axis (0.035 rad) and the stick flick also carried the frame time. Now the player's sensitivity and a binding's scale do not move the flick, and the stick flick does not depend on the frame rate. An inverted stick binding (negative scale) keeps its flipped side. If both devices flick in one frame the mouse wins and the stick is not asked. The mouse is measured in raw pixels and the stick in deflection, both from `ControlState` (`look_pointer_px()`, `look_stick()`); the `look` axis stays the scaled sum. The cooldown runs on the simulation clock, `Time<Virtual>`: a paused game does not run it down, a slowed one runs it down slowly. The mouse's sum drains on the frame clock (`Time<Real>`): that is input, not the game. Follow, framing, arm and shake stay on `Time<Real>`, so a paused game still shows its camera settle. There is no look grace after a spawn. Gameplay reads the stored `ThirdPersonCamera::yaw` (`input` turns the move axes by it): the shake only moves the camera's `Transform`.

*Offsets.* `CameraOffsets` (a resource, neutral by default) is what a game's systems add to the base values; `orbit_camera` reads it after them, every frame: `fov_add_deg` is added to the player's field of view before the engine's 30..120 limit (sprint widening, a finisher's punch); `focus_dip_m` lowers the camera and what it looks at together, so the picture drops without tilting (a landing dip; negative raises); `pitch_min` / `pitch_max` narrow the tuning's pitch range (an override outside it is held to it), and the stored pitch is clamped to the result (a swim cap: the camera stays above the water; lifting the cap does not push the pitch back up). **The writer owns its value**: the system that writes a field sets it on every frame it wants an effect and puts it back to `0.0` / `None` itself when it is done. The camera only reads: it never decays, sums or stores an offset, and it never writes `CameraSettings`, `ThirdPersonCamera::focus` or the stored pitch except through the clamp above, so an offset stays as its writer left it until the writer puts it back (a writer that stops without resetting
leaves its effect on). Two systems that want the same field must be combined by the game into one writer. Write in `PostUpdate` before `CameraSystems::Orbit` to be seen on the same frame (the set is empty without the `render` feature, so the writers run and are testable headless).

**Render interpolation (C8).** The simulation steps at 60 Hz and a body's `Transform` is the value of the last tick, so on a 144 Hz display a body would move in 60 Hz steps and the follow camera would inherit them. `shards_motion::interp` draws bodies between their last two ticks. It is presentation only: it never writes a body's `Transform` (physics reads it), and it adds nothing to a simulated entity.

- **Where the state lives.** `Interpolated` sits on a *visual child* of a `CharacterController` body (the model, a held prop: any child without an `AiId`); the engine adds it by itself (`adopt_visuals`, `PostUpdate`), so a game that dresses its bodies with a model child needs no code. `NotInterpolated` on a child opts it out. Adoption is opt-out on purpose (a game gets smooth models without code; the few children gameplay reads carry `AiId` or `NotInterpolated`). `body_snap_distance` stays in the `camera` kind for now: it is presentation tuning, and that kind is non-simulation (`SIMULATION = false`), so it never enters a replay's content check. It has no `AiId` and is not in snapshots, replays or the state audit.
- **Capture** (`FixedLast`, `InterpSystems::Capture`): the parent's pose after the tick becomes `current`, the old one `previous`. **Blend** (`PostUpdate`, before transform propagation, `InterpSystems::Blend`): the child's local `Transform` is its own rest transform moved by `lerp/slerp(previous, current, Time<Fixed>::overstep_fraction()) - current` in the body's frame (`visual_offset`). The rest transform is whatever the game left on the child; a change made by someone else is taken as the new rest. The animation clip's own look-ahead (`Time<Fixed>` overstep in the bridge) is separate, so the body trails the tick by up to one step as usual for interpolation.
- **The follow camera** runs after the blend (`CameraSystems::Orbit` is `.after(InterpSystems::Blend)`) and follows the same blended pose (`shown_translation`), for the followed body and for a locked target; with nothing blended it uses the tick value as before.
- **Resets, so a jump is never smeared.** The history starts over when: the body moved farther than `CameraTuning::body_snap_distance` (content, `camera` kind, default 4 m) between two ticks (a respawn, a region change that moves the player to a gate); `MotionState::placed` shows the controller found the feet somewhere it did not leave them (noted in `FixedFirst`, before the controller consumes it); the body's `Transform` is not what the last tick left when a frame is drawn (a `teleport` between frames: shown at once); the tick counter did not advance by one between captures (a snapshot loaded, a replay rewound, the setting was off). A snapshot load or a region change that replaces an entity gives its new child no history. The engine has no teleport event, so these signals are what it has.
- **Where it does nothing.** Without a picture (headless, simulation tests: `EngineConfig::draws()` is false); with `graphics.interpolation = false` (children go back to exactly the transform the game gave them, so the picture is the old one); while the clock is paused (and after a `step`, which runs ticks without frames): the last tick is drawn, so `shot` agrees with `observe`.
- **Co-op.** A client's remote bodies already blend host states (`shards_net` interp, which shows them a little in the past). Render interpolation applies on a client only to the body it predicts (`Predicted`); the others are left to the net, because blending twice would lag them. The host and single player blend all their bodies.
- **Setting.** `graphics.interpolation` (default `true`) in `GraphicsSettings`; `settings_set group=graphics key=interpolation value=false` is the A/B in a running game.
- **Rules for children and writers.** Every unmarked non-`AiId` child of a body is adopted, so a child that gameplay reads (a weapon socket, a muzzle, an aim origin) must carry an `AiId` or `NotInterpolated`. `restore_rest` (`FixedFirst`) puts blended children back at rest for each tick, so a local `Transform` read in the simulation is exact, on or off; a `GlobalTransform` read in `FixedUpdate` still holds the last picture. A system that rewrites a child's `Transform` must write all of it, before `InterpSystems::Blend` (a partial write is taken as the new rest and drifts); the body must be top-level (a body with a parent is not blended). A child is looked at when its body's `Children` change; one that loses `NotInterpolated` later is not adopted until then. Capture is not ordered against other `FixedLast` systems: one that moves a body after it makes the history start over each tick (no smoothing, never a wrong picture). `body_snap_distance` lives in the `camera` kind but is read by interpolation even without a camera. Pausing the virtual clock (menus, `step`) shows the last tick, so the picture steps by up to one tick of lag at pause and at resume. On a client, `shards_net`'s correction smoothing of the predicted body is applied to its `Transform` and so is not a second layer.

**Warnings about attacks off screen** (E2.6): `threat_indicators(world, viewer, forward)` from the combat module gives, for each attack zone, a direction for the indicator; markers at the edge of the screen are drawn by `shards_action::render::hud` (see "Actions and combat (E3)").

**Movement tools** (player, only through `Intent`):

| tool | what it does |
|---|---|
| `move_to` | walk to an entity or to a point `{x, z}`; `within`, `queue`; `path=true` follows the route from `path_to` |
| `move`, `stop` | walk N metres by compass (N = −Z, E = +X); stop |
| `jump`, `sprint`, `glide` | jump (out of water, onto the shore); sprint and glide as a state that is on (`on=false` turns it off) |
| `clearance` | query: how far in the eight directions, and what will stop you |
| `move_info` | query: what modifies a character's movement (slow, caps, root, stance, rates, sprint cost), the walk, run, sprint and swim speeds they leave, and the wading depth |
| `path_to` | query: the route over what can be walked, swum and climbed |
| `input_bindings` (player), `rebind` (developer) | controls; `rebind` writes to the `controls` settings |
| `physics_overlay` (developer, window) | colliders and volumes |

Errors teach too (`jump` in the water by the shore: `ok: jumping out of the water onto the ledge 0.4m ahead (+1.3m)`; far from the shore: `the ledge is 2.1m ahead (+1.3m), out of reach from the water: swim within … and jump, or keep swimming into it`).

```text
$ shards act clearance                       # player at the start of the obstacle course (x = 40)
N wall 3.6m · NE wall 5.1m · E ledge 11.6m (+1.3m) · SE wall 5.1m · S wall 3.6m · SW wall 5.1m · W free 15.0m · NW wall 5.1m
$ shards act path_to target=e12              # e12 — the landmark "Cliff"
{
  "distance": 44.5,
  "path": "path 44.5m, 6 turns: 11.5m E → 0.5m E (mantle 1.3m) → 4.0m E → 9.5m E (swim) → 0.5m E (mantle 1.3m) → 18.5m E",
  "turns": 6,
  "waypoints": [
    {"mode": "walk", "x": 51.5, "y": 2.0, "z": 0.0},
    {"mode": "mantle", "x": 52.0, "y": 3.3, "z": 0.0},
    {"mode": "walk", "x": 56.0, "y": 3.3, "z": 0.0},
    {"mode": "swim", "x": 65.5, "y": 2.0, "z": 0.0},
    {"mode": "mantle", "x": 66.0, "y": 3.3, "z": 0.0},
    {"mode": "walk", "x": 84.5, "y": 8.0, "z": 0.0}
  ]
}
```

- `clearance` names, for each of the eight directions, what will stop it: `free` (within 15 m), `wall`, `cliff` (with the depth of the drop), `water` (deep enough to swim), `ledge` (a ledge you can climb, and how far up it is).
  `around=ID` measures from another entity. It uses the same method as the controller, and works without a window. In a loaded region the navmesh also checks each direction (steps on the ground and descents; for a floating body, water too); if the navmesh ends more than a metre earlier than the physics answers, the direction is `closed`: `E closed 4.0m (the navmesh goes no further)` — the edge of a tile whose sockets do not line up, or a place that is cut out right now. Past the edge of the region the navmesh knows nothing, and there the physics answers.
- `path_to` asks the same route planner as `move_to path=true`, and names it in the field `provider`. In a loaded region the navmesh answers (`"provider": "navmesh"`; the length is not limited unless `max` is set; its refusal is `navmesh: …`). Everywhere else it is a local grid A* (`"provider": "local grid"`): cells of 0.5 m, a search in a limited area around the start and the goal (`max` is the limit in metres, 80 by default; anything over that is an error with the hint `raise max, or go in stages`). The grid moves over what the controller can do: walk, swim (twice the cost), climb (costs more), descend to a safe height, and jump into water from above. The result is the same on any machine (whole-number costs, order by index).
- `shards act move_to target=e12 path=true` follows exactly this route; the same `plan_path` function serves the grid, the tool and the tests. `nav_path` also shows the route over the navmesh, with the tiles it goes through (see "Navigation and NPCs (E6)").

## Actions and combat (E3)

The `shards_action` crate. An attack, a block, a parry, a roll and a flask are all the same thing: an **action**, a timeline from content (`*.action.ron`). One action machine per fighter (`Fighter`) drives the player, enemies and bosses; a **moveset** (`*.moveset.ron`) says which input starts which action, plus the slots `stagger`, `hit` and `death`.

**Formats.** All combat numbers are files, picked up on the fly. One example for all kinds (it passes `--check-content`):

```ron
// content/combat/hero.action.ron — actions: id → timeline
{
    "light_1": (
        net_id: 1,                                         // unique id of the action "on the wire" (for the network)
        phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        cost: (stamina: 10),
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20, poise: 15, status: [("bleed", 30)])],
        cancels: [(span: Secs(0.3, 0.55), into: [Combo("light", "light_2"), Input("dodge")])],
        displacement: [(0.0, 0.0), (0.3, 0.5)],            // (time, metres forward) — a lunge
        anim: "light_1",
    ),
    "roll": (net_id: 3, phases: [("roll", 0.4)], cost: (stamina: 25), iframes: [Secs(0.0, 0.18)],
             displacement: [(0.35, 4.0)], steer: Move),
    "block": (net_id: 4, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
              hold: (input: "block", span: Phase("hold")), move_speed: 0.4,
              guard: (span: Phases("raise", "hold"), perfect: Phase("raise"), damage_mult: 0.1)),
    "drink": (net_id: 5, phases: [("drink", 1.0)], cost: (charge: "flask"), effects: [(at: 0.6, effect: Heal(40))]),
    "slam": (net_id: 6, phases: [("windup", 0.9), ("active", 0.1), ("recovery", 0.7)], cost: (cooldown: 5.0),
             hits: [(span: Phase("active"), shape: Circle(radius: 3.0), damage: 22, poise: 40)],
             hyper_armor: [Phases("windup", "active")], telegraph: (symbol: "⚠", sound: "slam")),
}
// content/combat/hero.moveset.ron — which input starts which action
{ "hero": (inputs: {"light": "light_1", "dodge": "roll", "block": "block", "use_item.flask": "drink"},
           stagger: "stagger", death: "death") }
// content/combat/statuses.status.ron — statuses: threshold, decay, duration, the game's hook name
{ "bleed": (threshold: 100, decay_per_s: 10, duration: 3, effect: "bleed") }
// content/combat/tuning.combat_tuning.ron — numbers for the whole combat system (id `default`)
{ "default": (input_buffer: 0.15, max_statuses: 3, sprint_stamina_per_s: 10, lock_on_range: 20) }
```

- *Backstep, turning speed and regeneration holds (C4).* `steer: MoveOrBack` is `Move` that goes **backward**, away from where the body faces, when no direction is pressed and nothing is held (a pressed direction or the stick still wins); the `dodge` tool names it (`ok: roll (back)`, read from the moveset's plain `dodge` input). With `face: Keep` (see the action timeline) the backstep hops back and the body keeps looking where it looked, its gameplay facing included, so a second standing backstep goes the same way again; with `face: Run` (the default) the body turns around to face the way it goes, and the next one goes back the other way. An action with `turn` spans starts from the body's facing whatever its `steer`, so the content check refuses spans on a `MoveOrBack` action (the backstep would go forward). A `turn_speed_deg` of 0 means "turns at once", as in the locomotion params. `turn_speed_deg` on an action is the body's turning speed while it runs (a raised guard; see "Controller options", C4), and `regen_hold` (seconds) keeps the stamina from regenerating while the action runs and for that long after it ends: a guard with `regen_hold: 1.3` holds the regeneration while it is up and 1.3 s after it is let go. A hold is the longer of two waits: it moves `Stamina::since_use` back so that `secs` remain (`Stamina::hold_regen`), so it never shortens the delay a spend left. A paid spend sets the delay (`Stamina::delay`). `CombatTuning::free_sprint_regen_hold` (seconds; `None`: today's behaviour) makes a sprint that costs nothing (`MoveModifiers::sprint_cost` 0, or a fast swim without drain) hold the regeneration for that time instead of restarting the whole delay (0.3 s next to a delay of 0.65 s). Tests: `crates/shards_action/tests/combat/parity.rs`.

- A window is a `Span`: a phase (`Phase("active")`), from one phase to another (`Phases("raise", "hold")`), or seconds (`Secs(0.3, 0.45)`).
  A timeline can hold: hit windows (shape: `Sphere`, `Capsule`, `Cone`, `Box`, `Circle`, `Line` — a line along the ground; damage, poise damage, status build-up, the flags `Unblockable`/`GuardBreak`/`NoParry`, tags for the game's hooks), invulnerability (`iframes`), a block and a perfect block (`guard`), hyper armour (`hyper_armor`), cancels (combos — a chain of transitions `Combo`, `Input`, `Any`), hold (`hold`: the block stays up while the input is held), timed effects (`Heal`, `RestoreStamina`, `Status`, `Sound` — a sound from the body, `Custom` — a game hook), a telegraph (`telegraph`).
- The tick in which an action ends is still part of it: hit and guard windows see it. Validation rejects a guard window that starts before a hold span and ends inside it: it would open on every cycle.
- An action hands its displacement (`displacement`) to the controller through `LocomotionInput::external`: only the controller moves the body, so a lunge stops at a wall. You do not have to write it by hand: it can be baked from a clip (`bake`, see "Displacement from a clip" below).
- Animation and sound are names only: `anim: "light_1"` (a clip; if the game has clip sets, the content check requires one of them to play it), `telegraph.sound` (a sound from `*.sound.ron`: the audio module plays it where the hit lands), and the effect `Sound("whoosh")` (it is played from the body at its moment, for example a swing: `effects: [(at: 0.2, effect: Sound("whoosh"))]`), see "Audio".

**The machine.** A press from `Intent` waits in the input buffer `CombatTuning::input_buffer` (0.15 s by default) until its action becomes possible; cancel windows let one action be interrupted by another (combos); a held action repeats (a raised block). One rule, `can_start`, answers both the machine and the tools: `ok: light_1`, `ok: queued, light starts in 0.10s`, `not enough stamina: 20/40 (ready in 1.5s)`, `busy: windup light_1 0.17s (ready in 0.27s, when a cancel window opens)`. A press whose time has run out is written to the journal: `e5 light rejected: busy (active light_1 0.10s)`. A cancel window with a `queue` span holds a press made while that span is open until the window opens (the press lives until then plus the buffer time), and a moveset can keep one press at a time by priority (`buffer: SingleSlot(..)`); a cooldown can be shared by several actions (`cost.cooldown_key`). See "Action timeline (B2a)" below.

**Hits** are detected in `SimSet::Resolve` after movement. Attack shapes overlap hurtboxes (colliders with the `HURTBOX` layer; the `Fighter` collider gets it). Attackers and targets are taken in `AiId` order. One target is hit once per action (and once per `group`). The damage pipeline: immortality (`god`) → invulnerability (`Evaded`, `PerfectEvaded` inside a perfect-evade window) → parry (`Parried`, the attacker staggers unless the guard says otherwise) / block (`Blocked`, costs stamina) / guard break (`GuardBroken`) → damage taken (`DamageTaken`, B2b) → armour → game hooks (`add_damage_hook`, in registration order) → barrier and health, statuses, poise (a break is `stagger`; otherwise a reaction of the tier the hit asked for, or the old flinch) → death; see "Hit outcome: reactions, protection, pushes (B1)" below. Events: `Damaged`, `Staggered`, `Died`, `Healed`, `ActionStarted`, `ActionRejected`, `ActionSound` (the `Sound` effect).

**Fighter resources** are components: `Health`; `Stamina` (it regenerates after a delay; sprinting spends it, and once it is drained to zero, sprint does not start again until the button is released); `Poise` (it regenerates after a delay; at 0 the poise breaks and forces a `stagger` action); `Armor`; `Team` (they hit only members of other teams); `Charges` (named consumables, such as the flask); `Cooldowns`; `Dead`.

**Statuses** are a framework with no specific effects. Hits build a status up (`status: [("bleed", 30)]`), with decay and resistance multipliers (`StatusResist`). At a threshold, the status turns on for `duration`. The number active at once is capped (`max_statuses`). A status can also gate its next proc, refresh instead of stacking, keep building up while active, empty its bar when refused or stay out of the cap, and a moveset can set its own cap; see "Body state: barrier, damage taken, status rules (B2b)" below. Burning, cold or bleeding are game hooks: `add_status_effect(name, on_apply, on_tick, on_expire)`. Action effects `Custom(name)` use `add_action_effect`. The engine runs the `Sound(name)` effect itself. Whoever simulates sends the `ActionSound` event (entity, action, sound) on its own tick. The sound module plays the sound from the body. The host forwards the event to co-op clients as a hit. A client does not run or predict its own effects. So the sound is the same without a window, with one, and on a client.

**Telegraphs and zones.** An enemy warns ahead of time. The `Telegraph` event carries a symbol and a sound label. `hit_zones(world)` is for decals and AI. It returns each zone of a hit window that has not yet played out, or is still running, at the place where the attacker will be by the time of the hit. `threat_indicators(world, viewer, forward)` gives the direction for off-screen warnings. "Leave the zone" is worked out from the zone's footprint on the ground (a sphere counts as a circle, a cone as a sector). The path runs along one of 8 compass directions, with the length rounded up, so that walking exactly that path takes you out of the zone.

**Window side** (feature `render`; without a window the same systems are built, but they draw nothing):

- **Animation** (`anim`, `render::bridge`). The `animset` content kind is a library of clips from glTF. It says which clip shows which movement state and which action, the blending, and the time cues (`cues` → `AnimationCue`; these are for effects and sound only, gameplay does not depend on them). **Gameplay leads, animation follows:** an action's clip is scrubbed to the action's progress (gameplay time, scaled to the clip's length), and movement clips loop at a speed matched to the body's speed. There is one `AnimationGraph` per set, shared by all `Animated` characters. Characters with the same skeleton (the same node names) share clips. A clip can sit in another file with the same skeleton (the clip's `source`): the graph waits for every file of the set. The model is a child entity of the character (`WorldAssetRoot`). `root_motion` names the root bone. The bridge keeps it in place on the ground (x and z as in the rest pose, after `AnimationSystems`). The body is moved by the action's displacement, which is baked from that same bone (see "Displacement from a clip"). With the asset folder (`--check-content`, `content_check`), the set is checked against the files. A missing file, a missing animation (the error lists the ones that exist), a missing root bone, or a missing layer bone is a content error.
  **Layers (upper and lower body).** An action can play on part of the body while the movement clip keeps the rest: drinking from a flask while walking, or blocking while stepping sideways. A part of the body is a layer of the set: `layers: {"upper": (root: "spine")}` is a bone and everything under it (no more than 64 layers). The action mapping picks the layer: `actions: {"drink": (clip: "drink", layer: "upper")}` (the same in `tags`). The old form `"drink": "drink"` plays on the whole body. While the action runs, the layer's bones follow its clip (scrubbed to the action's progress, like any action clip), and the rest of the body follows the movement clip. The content decides this, not the code. The mechanism uses Bevy masks. A layer clip has its own node in the graph, and a mask keeps it off the bones **outside** the layer. (The layer's mask group is built from the model's bones when the player is bound.) Movement clips run on all the bones, as they do without layers. The weight is set by the share. On the layer's bones the pose is the weighted average of the clips, and the movement clips sum to 1. So a layer clip with share `s` plays at weight `s / (1 − s)`. The bridge ramps the share up and down itself. (`AnimationTransitions` keeps one "main" clip, and a layer clip in its place would push the movement out of the legs too.) The ramp takes the same time as normal blending: into the action clip, its `blend` (or `action_blend`); back to movement, the `blend` of the movement clip. One layer clip changes into another with no gap in the movement. The share never reaches 1 (the last 0.001% of movement on the layer's bones is not visible). A layer bone that the model does not have is a warning, and the action keeps the movement clip. Layers are for the picture only: `ActionState`, the replay and the snapshot do not see them. (Animation sets are outside the replay check `content`, `SIMULATION = false`: editing a set never makes a replay diverge. A baked `displacement` lives in the action, which does count.)
  The game adds `Animated` and the model to the body only when it draws (`config.draws()`). The replay does not check `Animated`, and the host does not send it to its clients. Gameplay does not depend on the clip. Without a window nothing is loaded. A snapshot taken with the picture on records `Animated`. (Without a model, it does nothing.)
  ```ron
  // content/anim/humanoid.animset.ron (sandbox, shortened)
  { "humanoid": (
      source: "models/humanoid.glb",
      clips: {
          "idle": (), "walk": (cues: [(at: 0.0, cue: "footstep"), (at: 0.4, cue: "footstep")]), "run": (),
          "light_1": (), "roll": (blend: 0.04), "death": (),
          "stagger": (name: "hit", blend: 0.04),                     // the `hit` animation of the file, under its own name
          "slam": (name: "heavy"),                                   // a guard's slam: the hero clip on its own timeline
          "bow": (source: "models/humanoid_gestures.glb"),           // a clip from the second file, with the same skeleton
          "drink": (source: "models/humanoid_gestures.glb", blend: 0.12),
      },
      locomotion: {"idle": "idle", "walk": "walk", "run": "run", "stagger": "stagger", "dead": "death"},
      layers: {"upper": (root: "spine")},                         // upper body: spine, head, arms; legs: the rest
      // actions find their clip by their own `anim`; `actions: {"swipe": "swipe"}` (template) — by action id
      actions: {"drink": (clip: "drink", layer: "upper")},        // flask: the hand drinks, the legs keep their own clip
      blend: 0.2, action_blend: 0.06,
      speeds: (walk: 2.0, run: 4.5, sprint: 7.5),
      root_motion: "root",
  ) }
  ```
  **Model.** The sandbox and the template use a low-poly humanoid, which the script `tools/blender/humanoid.py` builds in Blender without a window: `blender -b --factory-startup --python-exit-code 1 -P tools/blender/humanoid.py -- sandbox crates/sandbox/assets/models` (or `-- template template/assets/models`). It has 16 skinned bones and the clips `idle`, `walk`, `run`, `light_1`, `light_2`, `light_3`, `heavy`, `roll`, `block`, `hit`, `death`. The sandbox also has `bow`, `taunt` and `drink`. `drink` is a layer clip: it moves only the spine, the head and the arms, and it lives in `humanoid_gestures.glb`. Hits and the roll are timed to the phases of the game's own actions, and the clip's phases go into the glTF as the `phases` property. The same script gives the same bytes. A clip's label plays the effect from the `fx` table (`footstep` → `step_dust`), and a sound with the same name if there is one. This happens only where the model is drawn. Without a window there are no clips. A sound that the game must always hear is the `Sound` effect of an action (the `whoosh` swing of the sandbox and template hero). Leave the clip labels to the picture.
- **Telegraphs on the ground** (`render::overlay`) are always drawn, not only in the overlay. They show the zone's outline, the hatching, and a ring that fills as the hit approaches. The symbol from the content is shown as a billboard that faces the camera, so the danger reads without colour. Enemy warnings are warm-coloured, ally warnings cold-coloured. Everything aimed at the local player is drawn. A symbol wider than ASCII (`⚠`) needs a font (`ThreatHud::font`). Without one, ASCII is used. With the built-in plugins, the hatching and the ring are drawn by a decal on the ground (the effects module, "Effects and decals (E10.3)"; `TelegraphStyle::decals`), and the gizmo draws the outline and the symbol. `telegraph_decals on=false` puts all of the gizmo back.
- **Combat overlay** (`combat_overlay`, F2 in the window): hitboxes of open windows (red), hurtboxes (green), guard arcs (blue, the perfect one brighter), i-frames as an outline, and each zone with its time left.
- **Off-screen markers** (`render::hud`): an arrow and a symbol at the edge of the screen, pointing to an attack that is about to hit where the camera is not looking. They grow and blink faster as the hit comes closer. For `LocalPlayer`.

**Displacement from a clip** (`bake`, `shards_action::bake`). An action with `bake: (animset: "humanoid", clip: "roll")` takes `displacement` from the root bone (`root_motion` of the set) of its clip. `--bake-motion [ACTIONS]` (or the tool `motion_bake action=…`, Dev, `control`) reads the bone's travel at the tick rate (60 Hz). It keeps only forward movement (the model's glTF +Z). Sideways movement goes into the report, and vertical movement is left to the picture. It drops the keys that a straight line between their neighbours reproduces to within 0.5 mm. It rewrites the `displacement` field in the action's file in place. Comments and the order of fields stay. If there is no such field, it goes before `bake`. The same clip gives the same bytes. The clip must follow the phases of the action: its `phases` property (or its length) is checked against the action's `phases` to within half a tick. Otherwise the error names both timings. The content check, with the asset folder, bakes again and names the stale curve and the command to run. Without a window and without ticks:
```text
$ cargo sandbox --bake-motion
baked the displacement of 5 actions at 60 Hz
  heavy: clip `heavy`, 12 keys, 0.6 m forward by 0.7 s -> content/combat/player.action.ron
  light_1: clip `light_1`, 6 keys, 0.3 m forward by 0.3 s -> content/combat/player.action.ron
  light_2: clip `light_2`, 6 keys, 0.3 m forward by 0.3 s -> content/combat/player.action.ron
  light_3: clip `light_3`, 6 keys, 0.4 m forward by 0.35 s -> content/combat/player.action.ron
  roll: clip `roll`, 21 keys, 4.0 m forward by 0.35 s -> content/combat/player.action.ron
$ cargo sandbox --check-content        # after editing the clip, without --bake-motion
content/combat/player.action.ron: action `roll`: displacement is stale: clip `roll` of `models/humanoid.glb` bakes to 21 keys (4.0 m forward); bake it again with `--bake-motion roll` (or the motion_bake tool)
```
A new curve changes gameplay (the lunge, the roll): replays recorded with the old one diverge.

**What the AI sees.** In describe: `windup slam 0.78s (zone circle 4.5m) hyper-armor`, `stam 75/100`, `poise broken 0.8s`, `bleed 60%`, `queued light 0.13s`, `roll roll 0.33s i-frames`. Under the `YOU` line, the hint `ALERT in hit zone of e6 shockwave (lands in 0.50s): move 1.3m E to leave it`. In the journal: `e2 winds up slam (0.80s) ⚠
circle 4.5m`, `e1 -> e2 heavy -45 (hp 55)`, `e1 blocked e2 swipe: -2 hp, -15 stamina (hp 98)`, `e2 staggered (parried)`.

**Tools** (for the player, only through `Intent`): `light_attack`, `heavy_attack`, `block on=…`, `parry`, `dodge dir=…|away_from=…`, `use_technique slot=1`, `use_item item=flask`, `lock_on target=…|off=true`, `attack target=… input=… repeat=true`. For the developer: `god`, `heal`, `stagger`, `react id=… tier=1..3`, `barrier id=… amount=… seconds=…`, `damage_taken id=… key=… mult=… seconds=…`, `combat_overlay`. One-shot tools answer by `can_start`:

```text
$ shards act dodge
ok: roll (W)
$ shards act heavy_attack
ok: queued, heavy starts in 0.10s
$ shards act parry
error: your moveset `player` has no `parry`; it has: block, dodge, heavy, light, use_item, use_item.flask
```

Refusals for resources and timing name the cause and the wait: `not enough stamina: 10/20 (ready in 0.2s)`, `busy: windup heavy
0.50s (ready in 0.90s, when a cancel window opens)`. On an error, run `step` for the named time and try again.

`light_attack`, `heavy_attack` and `use_technique` take `hold=true` (press and keep the button held: an action that charges
while held turns into its charged version; the answer ends in ` (holding)`) and `hold=false` (let go, no press:
`ok: heavy released`). A person does the same with a `press_hold` binding (`BindMode::PressHold`, the standard map's `heavy`
and `interact`).

### Hit outcome: reactions, protection, pushes (B1)

A hit can ask for more than damage: a **reaction tier** (what the target is put into), a **push**, a cost on a guard, a wall in the way. All of it is content with a default that leaves old content as it was (a field at its default does not serialize, so the content half of a replay check is unchanged), and all of it is generic: a game picks the numbers.

```ron
// content/combat/sparring.action.ron
{
    "spar_slam": (
        net_id: 100,
        phases: [("windup", 0.5), ("active", 0.15), ("recovery", 0.6)],
        cost: (stamina: 20, stamina_mode: Any),             // starts on any stamina above 0, drains to 0 at most
        hits: [(span: Phase("active"), shape: Cone(range: 2.4, angle: 120), damage: 40, poise: 30,
                reaction: 2, knockback: 3.0, guard_damage: 25, flags: [LineOfSight])],
        armor: [(Phase("windup"), 1)],                       // tiers 1 and below cause no reaction while it winds up
    ),
    "spar_roll": (net_id: 101, phases: [("roll", 0.4)], iframes: [Secs(0.07, 0.25)],
                  perfect_iframes: [Secs(0.105, 0.215)], steer: Move),
    "spar_guard": (net_id: 102, phases: [("raise", 0.18), ("hold", 0.3), ("lower", 0.1)],
                   hold: (input: "block", span: Phase("hold")),
                   guard: (span: Phases("raise", "hold"), perfect: Phase("raise"), stamina_per_damage: 0.8,
                           perfect_cost_mult: 0.5, parry_staggers: false, break_damage_mult: 0.5,
                           status_mult: 0.25, break_status_mult: 0.5)),
}
// content/combat/sparring.moveset.ron
{ "sparring": (inputs: {"heavy": "spar_slam", "dodge": "spar_roll", "block": "spar_guard"},
               stagger: "spar_stagger", guard_broken: "spar_guard_broken", guard_broken_push: (0.2, 10.0),
               reactions: [(action: "spar_react_light", push: 2.0, decel: 12.0),      // tiers 1, 2, 3
                           (action: "spar_react_heavy", push: 4.0, decel: 12.0),
                           (action: "spar_react_down", push: 4.0, decel: 12.0)],
               reaction_armor: 0, reaction_immunity: 0.2, reaction_protection: 1.0, break_protection: 3.0) }
```

**The pipeline after B1.** `detect` (a hit with `LineOfSight` skips a target when a ray from the attacker's chest to the target's meets a `LayerMask::WORLD` collider; the two bodies' own colliders never count; a skipped target is not marked, so it can still be hit later in the same window; the chest is `CombatTuning::sight_height`, 0.6, of the body's height) → `resolve` fills a `DamageCtx` (`credit`, `dir`, `reaction`, `reaction_armor`, `knockback`, `taken`, `guard_mult`, `weight`, `ignore_reason`) → `modifiers` (god mode, i-frames and perfect evades, guards with per-hit costs and shares, `Armor`) → the game's hooks → `apply`. `apply` works out a **plan** first (does the poise break, does the target react), so `Damaged` can carry its `weight` and `reaction` while still being triggered before death, statuses and poise, as it always was. Events name the credited entity (`DamageCtx::credit`) as `attacker` or `killer`; `source` is the combat object that hit (`DamageCtx::attacker`) when it is not the credited entity: a hook that sets `credit` to an owner gets `source` set to the object on `Damaged`, `Blocked`, `GuardBroken`, `Parried`, `Evaded`, `PerfectEvaded` and `Died` (`kill_with` for a game that kills itself; `kill` leaves it `None`). A parry staggers `DamageCtx::attacker`, the object that hit. A status a hook appends to a blocked hit is applied (the block applies whatever is left in the context after the hooks, by default the `status_mult` share). `Damaged::ignore_reason` is `None` for a hit that landed; a hook that sets `Ignored` without a reason prints `ignored`. `HitWeight::Counter` and `Finisher` and `IgnoreReason::Downed` are reserved: the engine never sets them, a game hook may.

- **Guard.** The cost of a block is `hit.guard_damage` (else the hit's damage) times `stamina_per_damage`. A perfect guard pays `perfect_cost_mult` of it (`Parried { stamina }`, journal `, -10 stamina` only when above 0); when it cannot pay, the guard breaks, perfect or not, and spends all the stamina. `parry_staggers: false` leaves the attacker alone: the game's `Parried` observer decides (poise through `poise_damage`, a recoil through `react`). A guard break lets `break_damage_mult` (else `damage_mult`) of the damage through, a block `damage_mult`; a block lets `status_mult` of the status buildup through, a break `break_status_mult` (0, the old rule, lets none). A break forces the moveset's `guard_broken` action (then `Staggered { cause: GuardBroken }`), else its `stagger`; `guard_broken_push: (scale, decel)` starts a push at `scale` times the body's current flat velocity. **The guard is decided before the hooks**, from the damage as the hit defines it: a hook that rescales damage must also change `outcome` and `guard_mult` if it wants Blocked versus GuardBroken decided again; the engine does not redo the guard after the hooks.
- **Perfect evade.** A hit that meets i-frames while a `perfect_iframes` span is also open is `Outcome::PerfectEvaded`: `Evaded`, then `PerfectEvaded`. Not for a hit with `NoPerfectEvade` (ground zones), nor for one that does no damage. The attacker's ledger marks the defender at the first detection, so it is once per swing. Every `perfect_iframes` span must lie inside the i-frames. **`perfect_iframes` and `armor` spans are sampled at the instant of the tick** (`start <= t <= end`), not over the tick: the roll's `Secs(0.105, 0.215)` is open in ticks 7 to 12 (6 ticks at 60 Hz), not 13. Content copied from a source that samples the same way keeps its literal numbers. (`iframes`, `hyper_armor`, guard and hit windows keep the older over-the-tick rule.)
- **Stamina overdraft.** `cost.stamina_mode: Any` starts the action on any stamina above 0 and drains it to 0 at most; `Full` (the default) needs the whole cost. The refusal prints the action's real cost (`not enough stamina: 0/22`); `can_start` is the one rule, so the tools, the NPC planner and a client's prediction follow.

**Reactions.** A hit's `reaction` is a tier: 0 none, 1 light, 2 heavy, 3 knockdown; no `reaction` keeps the old rule (the moveset's `hit` flinch on a hit that does not break poise). The rule (`should_react`): no reaction for tier 0; for a tier at most the target's **reaction armour** (the moveset's `reaction_armor` and the running action's `armor` spans, whichever is higher; a hook may raise `DamageCtx::reaction_armor`); for a hyper-armoured body; for a dead one; while its poise is broken or it runs its `stagger`, `guard_broken` or `death` action; and for tiers 1 and 2 inside the **reaction immunity** (`Protection::reactions`). Tier 3 ignores immunity but not armour. A hit that breaks the poise staggers and asks for no reaction. Otherwise the target is forced into `reactions[tier - 1]`, starts `reaction_immunity` (0.2 s; it is *assigned*, so a knockdown inside a longer protection cuts it to 0.2 s, and the end of the reaction it interrupts then grants `reaction_protection` on top), and is pushed at the larger of the reaction's `push` and the hit's `knockback` along `DamageCtx::dir`, slowing by the reaction's `decel` (every `decel` is above 0: a push that never stops is a content error). A moveset with no `reactions` flinches as before and gets no push, immunity or `Reacted`.

`react(world, entity, Reaction)` is the same thing for a game: `Reaction::hit(..)` (a hit's own: armour and immunity apply, starts the immunity), `Reaction::recoil(tier, from, dir)` (ignores immunity, starts none; armour still stops it: a parried attacker's recoil), `Reaction::forced(tier, from, dir)` (ignores both: a charge into a wall, a finisher's knockdown, a shockwave). Its fields are public for the rest. `Reacted` says who reacted, to which tier and into which action.

**Timers.** `Protection { reactions, poise }` is a lazy component: inserted when a timer is granted (`grant_protection(world, entity, reactions, poise)` keeps the longer time), removed when both run out. A timer granted in the fighter chain by the end of an action (`reaction_protection`, `break_protection`) is counted down in the tick it is granted, so 1.0 s acts on 59 further ticks, while a hit-granted timer (set before `tick_protection` of that tick, in `Hits`) acts on its full count. A timer of `s` seconds counts down by the tick *after* that tick's checks (`tick_protection`, last in the fighter chain), and is gone when `left <= EPS`: 0.2 s set by a hit in tick `N` blocks the hits of ticks `N+1` to `N+11` and tick `N+12` reacts. When a reaction or a stagger ends, for any reason (`ActionEnded`), the moveset's `reaction_protection` begins; the end of `guard_broken` grants none. When a poise break runs out, `break_protection` begins: poise damage is ignored for that long. `poise_damage` (the engine's rules, public), `end_break` (ends a break now, grants the protection) and `reset_poise` (a whole gauge, no break, no poise protection; a boss's phase change) are for the game. A poise break removes a push in progress.

**Pushes.** `Knockback { velocity, decel }` (flat, m/s and m/s per s) is a lazy component. `push_bodies` (`ActionSystems::Push`, after the effects, in `AiId` order) decays the speed first and moves by the decayed speed (`v1 = max(0, v0 - decel * dt)`, `external += dir * v1 * dt`): Euler, not a trapezoid; the first tick moves `(v0 - decel * dt) * dt`. It adds to `LocomotionInput::external` after the machine wrote it, so a wall stops it; a dead body, a body with no `LocomotionInput`, and a body that is no longer in one of its moveset's reaction, `stagger` or `guard_broken` actions lose their push (the push is the reaction's own motion and ends with it, however it ended). A reaction or a guard break with `guard_broken_push` also drops the body's own flat velocity, so the push alone moves it.

**`ActionEnded { entity, action, instance, end }`** is triggered when an action ends or changes (the run may go on, see `Branched`), before the `ActionStarted` of what replaces it: `Finished` (its time ran out), `Cancelled` (a cancel window started another), `Interrupted` (a forced action, a kill, a revival), `Removed` (its definition left content), `Branched` (a charge turned it into its charged action: the same run goes on under the new name, B2a), `Released` (its `while_held` input was let go, B2a). `finish_actions` ends them in `AiId` order through `Commands` (applied at the next sync point); `force_action`, `kill` and `revive` trigger it at once. It is not a net event, and a client's prediction never emits it.

**What the AI sees.** `protected 0.82s`, `poise protected 2.40s`, `pushed 3.2m/s` in describe; the journal says `e7 reacts heavy (e5 spar_react_heavy)`, `e6 perfectly evaded e5 spar_slam`, `e6 parried e5 spar_slam, -10 stamina` (the suffix only when the perfect guard costs something), and an ignored hit prints why: `... -0 (hp 100, god mode)`, `grace`, `downed` or `ignored`. The dev tool `react id=e7 tier=2` forces a reaction (backwards from where the body faces); `step --until named:reacting` waits for any fighter to run a reaction action. The engine rules `action.protection_sane` (timers finite and `>= 0`) and `action.knockback_sane` (a flat, finite velocity and a `decel` above 0) stop the game on a bad write. `PerfectEvaded` and `Reacted` travel to clients like the other combat events.

### Action timeline: charge, turning, miss recovery, cooldown keys, queue (B2a)

More of what an action does over its own time is content. Every field has a default that keeps old content and old runs
exactly as they were (it does not serialize at its default, and an action that uses none of them takes the old code path).

```ron
{
    "heavy": (
        net_id: 70,
        phases: [("windup", 0.4), ("active", 0.1), ("recovery", 0.3)],
        cost: (stamina: 20),
        hits: [(span: Phase("active"), shape: Cone(range: 3.0, angle: 120), damage: 20)],
        charge: (after: 0.2, into: "heavy_full"),           // held 0.2 s: it turns into `heavy_full`
        turn: [(Phase("windup"), 90)],                       // turns at 90 degrees a second while it winds up
    ),
    "lunge": (
        net_id: 75,
        phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.2)],
        cost: (cooldown: 3.0, cooldown_key: "lunge", cooldown_if_interrupted: 0.4),
        hits: [(span: Phase("active"), shape: Cone(range: 3.0, angle: 120), damage: 10)],
        miss_recovery: 0.6,                                  // a whiff recovers 0.6 s longer
        // never more than 22 degrees off where it started, at most 0.4 m closer for the turning,
        // and no tracking of a target behind a wall
        assist: (angle: 22, close: 0.4, sight: true),
        track_until: 0.2,
    ),
    "light": (
        net_id: 78,
        phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.4)],
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 5)],
        // The chain opens at 0.5 s; a press made in the recovery waits for it.
        cancels: [(span: Secs(0.5, 0.7), into: [Combo("light", "light_2")], queue: Phase("recovery"))],
    ),
    "beam": (net_id: 83, phases: [("beam", 2.0)], while_held: "technique.2"),   // lasts while held
    "roll": (net_id: 84, phases: [("roll", 0.4)], displacement: [(0.4, 2.0)], steer: Move, face: Keep),
}
// a moveset: one press waits at a time, the stronger first
"fencer": (inputs: {..}, buffer: SingleSlot(priority: [("dodge", 60), ("technique.1", 55), ("technique.2", 55),
                                                       ("heavy", 40), ("light", 30)])),
```

**Charge** (`charge: (after, into, until)`). While the input that started the run has been held on every tick of it, the run
turns into `into` on the first tick with `after <= t <= until` (`until` defaults to the end of the windup: the start of the
first hit window, else the end of the first phase). The turn keeps the run: the same `instance`, the same clock `t`, the same
hit ledger (a target the first action already hit is not hit again by the same hit group), and the same input. It costs what
`into` costs beyond the start: the difference of the two actions' stamina under `into`'s `stamina_mode`, one of `into`'s
charges when it uses another charge than the action it turns from, and `into`'s cooldown, whose slot must be ready unless it
is the slot of the action it turns from (this run started that one). When something is not there, nothing is paid and it is
tried again on the next tick, until `until` (no event: the client cannot see a refusal, see prediction below). It triggers
`ActionEnded { end: Branched }`, then `ActionStarted { action: into }`; the effects and the telegraph `into` has before the
turn's `t` do not fire (the run is already past them; an accepted rule of the design). A press let go once never charges again in that run. Validation:
`after` inside the action, `until >= after`, `into` exists, is not the action itself, and charges do not form a cycle.

**Turning.** `turn: [(span, degrees_per_second)]`: an action with turn spans starts facing where the body faced (no snap) and
turns at the highest rate of the spans open in the tick, toward its lock-on target, else the aim point, else the movement input
(a player steering the swing; a `steer: Move` action follows only the movement input, and a `MoveOrBack` one has no spans). The rate holds for the whole tick: a
span that opens or closes inside a tick turns it a full tick. Outside the spans the facing holds. An action with no `turn`
keeps the old `track_until` snap exactly. `turn` and `track_until` are not used together. A steering share of a design
("60% of the normal control") is a turn rate: the share times the body's own free turn rate
(`CharacterParams::turn_speed_deg`). The spans turn the run's facing (its hits, guard arc and displacement), not the body: the controller turns the body toward it once a tick at its
own rate (an action's `turn_speed_deg`, else the locked rate, else the free one; see "Controller options", C4), so a span
faster than that rate leaves the body behind the swing, and the two never both turn the body. Angles go through `ops::atan2` and
`ops::sin_cos`.

**Attack assist** (`assist: (angle, close, sight)`, for an action that tracks: `turn` or `track_until`, not `steer: Move` or `MoveOrBack`).
With a lock-on target, the facing stays within `angle` degrees of the direction to the target when the run started (with
`track_until` and no `turn`, it snaps to the clamped direction). `close: metres` caps the approach the run's turning adds: what
moving along the turned facing brings the body closer to the target than moving along the facing the run started with
(`RunExtras::from`) is counted in `RunExtras::closed`, and past `close` the excess is taken off the displacement along the
direction to the target (the body still moves sideways). `sight: true` turns the assist off while a `LayerMask::WORLD` collider
lies between the chests of the fighter and its target (the ray a `LineOfSight` hit casts, at `CombatTuning::sight_height`):
the run then does not track the target and turns toward the aim point or the movement input instead. The player turns the
assist off with the setting `controls.attack_assist = false`: the controls then hold the input `assist_off` for the
player's body (it is an `Intent` input: replays and the host see it), and every action with an `assist` acts as if out of
sight. An action without `assist` tracks its target as it always did.

**`face: Keep`.** The run moves the body along its own facing but does not turn it: the body keeps the facing it had, and only
what faces it outside actions (a lock-on target, an aim point) still turns it. A roll in a direction other than the body's
facing leaves the body looking where it looked. `face: Run` (the default) turns the body where the run points. A `face: Keep`
roll locked on a target turns the body toward the target while the run goes where it was steered
(`a_lock_on_target_still_turns_a_face_keep_body`), at the action's `turn_speed_deg` when it has one.

**Miss recovery** (`miss_recovery: seconds`, the action needs hits). On the first tick whose `t` reaches the end of its last hit
window, after that tick's hits are resolved (`check_misses`, `ActionSystems::Hits`, after `resolve_hits`, before
`finish_actions`), the run is checked once: if none of its hits landed by then (a hit, a block, a parry or a guard break all
count, the checking tick's own included), it lasts `miss_recovery` longer (`Running::length` grows; its cancel windows stay
where they are; the phase past the end stays the last one) and `ActionMissed { entity, action, extra }` is triggered. A game
adds its own bonus from an observer with `extend_recovery(world, entity, seconds)`; on a machine that does not simulate (a
co-op client, where the host's state brings the longer run) it changes nothing and returns `false`, so an observer that
calls it needs no check of its own.

**Cooldown keys and shares.** `cost.cooldown_key: "lunge"` puts the cooldown on that slot instead of the action's id, so
variants of one technique share it (`can_start` names the key: `lunge is on cooldown (ready in 2.40s)`). A key may be another
action's id: the two then share that action's slot. `cost.cooldown_if_interrupted: 0.4` (it needs a `cost.cooldown`): a run cut
before its windup is over keeps only that share of its cooldown (forced over, killed, revived, released, or cancelled into
anything but a `Combo` continuation; a dodge out of the windup counts; its own end, a charge's turn and a removal do not). The
share never lengthens a slot: a slot already below it (or ready) stays as it is. It is set before the action that cut it pays, so a new action on the same slot keeps its own full cooldown. The HUD source
`you.cooldown` reads an action's slot through its key.

**The press queue.** A cancel window's `queue: span`: a press of an input the window lets through, made while `span` is open
and before the window opens, lives until the window opens plus `input_buffer` (`press_life`, which the machine, a client's
prediction and the tools all use). A window with a `queue` that starts where the action ends (`Secs(L, L)`) never cuts the
action short: it holds its presses until the action is over, and they start the tick after. Every end of a run but a
charge's turn caps the waiting presses to `input_buffer` again. The usual press lives:

| press life | content |
|---|---|
| a chain: the buffer's time, or past the chain's opening for a press in the recovery | `(span: Secs(open, end), into: [Combo(..)], queue: Phase("recovery"))` |
| a chain pressed early: past the chain's opening from the first tick | `queue: Phases("windup", "recovery")` |
| an action with no chain: its window is the end of the action | `(span: Secs(L, L), into: [Input("light")], queue: Phase("recovery"))` |
| a technique another one may cut from `cancel`: past `cancel`, at any time | `(span: Secs(cancel, L), into: [Input("technique.1"), ..], queue: Secs(0, L))`; no window: no `queue` |

An enemy's chain uses the same rule: an NPC whose brain presses the next link early (in the first link's windup) has it start
on the tick after the combo window opens, and the director's token stays with it through the chain (the continuation is the
same attack).

**The single slot** (`MovesetDef::buffer`). `PerInput` (the default) keeps one press per input, as before. `SingleSlot {
priority }` keeps one press. `priority` is a list of `(input, rank)` (equal ranks are allowed, inputs not listed rank 0): a press
that ranks below the waiting one is dropped (`e5 light rejected: heavy is waiting and comes first`); an equal or higher one takes
its place (`e5 light rejected: heavy took its place`); the same input again keeps the later expiry. The press in the slot is tried
on every tick until it starts or its time runs out, as with `PerInput`. Only presses of inputs the moveset understands go in;
others (a jump) keep their own buffers. The tools answer the same before pressing (`heavy is waiting and comes first: one press
waits at a time, and light ranks below it`; a press that pushes one out says `(light no longer waits)`), and on a client's own
body they read the predicted run and the predictor's buffer (`shown_state`, `shown_queue`), with the presses not taken yet.

**`while_held: input`.** The run ends the tick its input is not held (`ActionEnded { end: Released }`). A tool press without
`hold=true` says so: `ok: beam (a tap: it stops as soon as technique.2 is not held; hold=true keeps it going)`. An action with
both `while_held` and a `hold` loop is a channel that loops while held and ends when let go.

**State.** What a run needs beyond `Running` lives in the lazy component `RunExtras { instance, held, landed, checked, extra,
start_facing, from, closed, charge_at }`, put in when a run whose action needs it starts and taken out when the run ends
(every way a run ends; the engine rule `action.extras_follow_run` stops the game on extras of another run). A fighter whose
actions use none of it never has one, so the replay hash of old content does not change. It is a component of an `AiId` entity:
snapshots, replay checks and co-op clients get it by the default rules. No resource is added. The setting
`controls.attack_assist` is not simulation state: what reaches the simulation is the held `assist_off` input. While a recording plays, the controls do not hold it: a recording made with the assist on verifies the same on a machine whose setting is off (test `a_replay_with_the_assist_on_verifies_on_a_machine_with_it_off`).

**Prediction.** A client predicts a charge, turning, the assist (with its sight ray), `while_held` and the queue with the same
functions (`charge_due`, `step_run`, `tracked`, `press_life`, the single slot), and reconciles by `(instance, action)`: a host
run of the same instance under another action is the host's turn of a charge (the client takes the host's action and keeps its
own `t` and hold); the host's start of a run the client has already turned (it arrives a round trip late) confirms the start
and leaves the turn shown, so the body does not go back to the uncharged action and turn again; a turn the host did not make
(its stamina, a charge or the cooldown) is kept until `until` has passed and then reverted to the host's action at the host's
time; a same-instance, same-action run adopts what only the host changes (a miss's extra recovery: the client's run does not
end before the host's); and when the host's run is gone with no new one and nothing is pending (a cut, a release), the shown
run is dropped and the body goes back to the host's state.

**What the AI sees.** Describe: `charging, turns in 0.12s` while the starting input is held (`charging, turns when the stamina is
there` once it waits for stamina), `missed +0.60s` after a whiff. Journal: `e5 lunge missed: +0.60s recovery`, and the
single slot's drops as `rejected` lines. `step --until named:charging` waits for any fighter holding a charge.

### Body state: barrier, damage taken, status rules (B2b)

What sits on a fighter between hits. Three lazy components (a fighter has one only while it matters, and the tick removes
each when it is empty), all of them in the replay and the snapshot, none predicted by a co-op client.

```ron
// combat/sparring.action.ron
"spar_ward": (net_id: 140, phases: [("cast", 0.5)],
              effects: [(at: 0.3, effect: Barrier(amount: 0.5, seconds: 8, of_max: true))]),   // half of max health, 8 s

// combat/sparring.status.ron
{
    "spar_chill": (threshold: 100, decay_per_s: 14, duration: 4, min_proc_interval: 2.0,
                   cool_while_active: false, refresh: true, reset_on_refuse: true),
    "spar_jolt":  (threshold: 100, decay_per_s: 14, duration: 0, capped: false, reset_on_refuse: true),
}
// a moveset: max_statuses: 1   (else CombatTuning::max_statuses)
```

**Barrier.** `Barrier { amount, left }` takes damage before health. `grant_barrier(world, entity, amount, seconds)` gives the
larger of the pool it holds and `amount`, for `seconds` from now (a smaller or shorter grant keeps the pool and still restarts the time to its own `seconds`: the rule of the design, since a barrier is re-cast, not extended; a game that wants the later end reads `Barrier::left` and grants `max`); it is
ignored for a dead or gone entity, one without `Health` (a barrier only guards health), and an `amount` or `seconds` that is
not a finite number above 0 (above the engine's epsilon, 1e-4). The `Barrier` action effect is the
same call; with `of_max: true` the amount is a share of the actor's max health (`0.5` is half of it). It triggers `BarrierGranted {
entity, amount, seconds }` (the pool now), a net event for the HUD. `hurt` is the one place damage reaches health: the
barrier takes `min(amount, damage)` first, `Damaged`, `Blocked` and `GuardBroken` carry it as `absorbed` (their `amount`
stays the whole hit), and the journal line gains ` [barrier -20]` only when it is above 0 (a blocked line's `-N hp` is what reached health, the hit less the barrier's share). A barrier that emptied is removed
at once; poise, statuses and reactions are decided by the hit as before. `clear_barrier(world, entity)` removes it. The dead's barrier, damage-taken timers and proc gates do not run down, as its statuses do not tick: a corpse keeps them as they stood and a `revive` resumes them (a hit never reaches a corpse, so none is spent either).

**Damage taken.** `DamageTaken(Vec<TakenMult { key, mult, left: Option<f32> }>)` holds named multipliers, sorted by key,
unique; `left: None` lasts until cleared. Their product is `DamageCtx::taken`, applied in `modifiers` after the guard and
before `Armor` (so a guard's cost is still decided from the hit as it is written, and a hook sees both `taken` and
`guard_mult`); a fighter without the component is not touched. `set_damage_taken(world, entity, key, mult, seconds:
Option<f32>)` replaces the key; `clear_damage_taken(world, entity, key)` removes it (no event). A timed multiplier that runs
out on a tick is removed and triggers `DamageTakenExpired { entity, key }`, in `AiId` order, then key order. A window that covers a base
(below) leaves the base in the key, and the event still fires. A `mult` below 0 or not finite, or `seconds` of 0 or less,
is ignored (`clear_damage_taken` cancels).

*The later end wins (E46).* The Godot game let a shorter core window replace a longer one and a permanent base cancel a timed
window; the engine does not. `extend_damage_taken(world, entity, key, mult, seconds) -> bool` is `set_damage_taken` that never
shortens what the key holds: a timed window over a held one ends at the later of the two ends (the `mult` is the new one); a
timed window over a permanent multiplier takes the key and the permanent value becomes `TakenMult::then`; a permanent set over a
timed window leaves the window running and becomes the `then` (returns `false`: the multiplier in force did not change, so a base
that changes mid-window, a phase change, is not lost). When the window ends the key returns to its `then`, permanent
(`DamageTakenExpired` still fires, with the base already back); with no `then` the key goes. The game needs no timer of its
own and no copy of its base: one key (`core`), `extend` for the windows and the base. `set_damage_taken` is the plain replace
and drops the `then` (the Godot behaviour, if a game wants it); the describer shows `x1.35 (core 1.0s, then x1.50)`. To read what is there, `DamageTaken::get(key) ->
Option<&TakenMult>`, `DamageTaken::left(key)` (`None` absent, `Some(None)` permanent, `Some(Some(s))` timed) and `product()`.
The dev tool takes `keep_later=true` for it.

**Timers.** Stored like `Protection` and `Cooldowns`: `f32` seconds left, checked as `left > EPS`, run down by the tick
length after the tick's checks in `ActionSystems::Fighters` (`tick_body_state`, after `tick_protection`, one system in `AiId`
order), removed when `left <= EPS`. A timer of `s` seconds set between ticks acts on the next `ceil(60 s)` ticks and is gone
for the one after; every end is a simulation tick, never a frame or a mesh. They are components on the fighter, so replays and
snapshots carry them.

**`deal_damage(world, target, amount, credit: Option<Entity>) -> f32`** is damage that is not a hit (a status hook's burn,
a hazard): the barrier, then health, then `kill_with(.., credit, None, None)` when health ran out; it returns the health left.
It does nothing to a dead or gone entity, to one in god mode (`Invulnerable`), or for an amount that is not above 0, and it
runs none of the pipeline (no guard, armor, `DamageTaken`, hooks, reaction or poise) and triggers no `Damaged` (that event names
a hit's action); a game that wants a line for it triggers its own event. Like `extend_recovery`, it does nothing on a machine that does not simulate (a co-op client), so an observer that calls it on every machine kills once.

**Status rules.** `StatusDef` gains `min_proc_interval` (after a proc, buildup that reaches the threshold again within this
many seconds is swallowed: a `ProcGates` entry on the fighter), `cool_while_active: false` (buildup does not decay while the
status is active), `refresh` (reaching the threshold while active restarts the duration, no stack; buildup keeps coming while
active), `reset_on_refuse` (a refused proc empties the bar; else it waits at full, the old rule) and `capped: false` (an
instant status: it needs no free slot and does not use one). A moveset's `max_statuses` replaces the tuning's cap for that
fighter. Order, as in Godot: add buildup (an active status that neither stacks nor refreshes ignores it); below the threshold:
done; reached while gated: refused; else active: `stacks` adds a stack, `refresh` restarts; else if capped and no free slot:
refused; else it procs, and `min_proc_interval` closes its gate. `Statuses::add` and `Statuses::tick` are unchanged for old
callers; `Statuses::add_with(id, def, amount, limit, gated, capped)` and `Statuses::tick_with(.., gated)` hold the new rules,
and `add_status_buildup` (hits and `Status` effects) uses them with the target's `ProcGates` and its moveset's cap. An instant
status is active for one tick: `StatusApplied`, its `on_apply` (and `on_tick` once), then `StatusExpired`. A `refresh` returns `Applied` like a proc, so `StatusApplied` and the game's `on_apply` run again and the gate closes again; a game whose `on_apply` is not repeatable checks the status's `stacks` or its own state. Chill's longer recovery and burn's explosion are the game's.

**What the AI sees.** Describe: `barrier 15 (6.2s)`, `takes x1.35 (core 3.1s)` (several: `takes x0.50 (armor), x1.35 (core
3.1s)`), `proc gate spar_chill 1.2s`. Journal: `e5 barrier 50 for 8.0s`, ` [barrier -15]` on a hit line. Dev tools: `barrier
id= amount= seconds=` (a grant; `amount=0` removes it), `damage_taken id= key= mult= seconds=` (`clear=true` removes the key,
`keep_later=true` extends). The engine rules `action.barrier_sane` (finite `amount >= 0` and finite `left`) and
`action.damage_taken_sane` (finite `mult >= 0`, finite times, keys sorted and unique) stop the game on a bad write.

## Levels from Blender (E4.2)

The `shards_level` crate. An artist builds a level in Blender, adds *Custom Properties* to objects, and exports glTF. The engine turns the file into entities: colliders and sensors for physics, markers (spawn points, sockets, navigation links, triggers, camps, landmarks), prefabs from the content database, and any game components by name. With `render`, the same file is also drawn. The engine reads only the JSON of the file (the node tree, transforms, names, `extras`, mesh bounds). It loads no images and no buffers, so the level is built and checked without a window.

```text
Blender (.blend) ── export glTF ──► tiles/ruin.gltf ──► LevelAsset ──► spawn_level ──► entities
  objects + custom properties        nodes + extras        (no GPU)       top-level entities, in world
                                                                           coordinates: Collider, Volume,
                                                                           LevelMarker, prefabs, components
```

**How to make a level in Blender.**

1. Model in metres, with Z up, as is usual in Blender. The exporter converts Z-up to +Y-up itself (Blender's point `(x, y, z)` is `(x, z, −y)` in the file). Object names can be anything, with spaces or non-Latin letters. Errors name a node by its path in the hierarchy (`Ruin/Column`). Objects of a collection instance (*Add → Collection Instance*) reach the file as children of each instancer (`Barrels.A/Barrel`). Keep the collection itself in its own scene. If it is excluded from the view layer of the level's scene, it is still exported at its place, and so it ends up in the level one more time.
2. Select the object, *Object Properties → Custom Properties*, and press `+`: name and value. Blender 5.2.2 was checked on export for text, numbers, checkboxes, arrays of numbers and strings, *Data-Block* (a reference to an object), and *Python* (a dictionary or a list, which is JSON in the file). Arrays of integers and of checkboxes were not exported. Properties are read from **objects**. The engine does not read properties on mesh data (*Object Data Properties*), and the check reports them
   as an error.
3. *File → Export → glTF 2.0*. In the *Include* section, tick **Custom Properties**. Without it, all properties are lost. In *Data → Mesh*, tick **Apply Modifiers**. Without it, the file gets the meshes from before the modifiers, so the `auto` collider wraps those and not what you see in Blender. Leave *Transform → +Y Up* on. `.glb`, `.gltf` *Separate* and `.gltf` *Embedded* all work (in Blender 5.2 the last one is hidden behind the add-on setting *Allow glTF Embedded format*), and so do lamps (*Include → Punctual Lights*) and materials. *Data → Draco Compression* and *Data → Meshopt Compression* (with either of its two extensions) do not work. Bevy does not draw such meshes, and the engine refuses with a hint about which tick box to clear.
4. Put the file in the game's asset folder (`assets/tiles/ruin.gltf`), name it in the content (content kind `level`, see below), and run `shards content check`.

**Node properties** come in two kinds: level properties (lowercase words, which the engine reads) and components (the names of the game's components in `CamelCase`).

| property | value | what it does |
|---|---|---|
| `marker` | `spawn`, `socket`, `navlink`, `trigger`, `interactable`, `camp`, `landmark`, `custom:<name>` | the node is a marker of this kind (`LevelMarker`) |
| `name` | text | the marker's name (`start`); without it, the node's name |
| `socket` | text | shorthand for `marker = socket`: the value is the socket type (`road`) |
| `to` | text or an object (*Data-Block*) | for `navlink`: the node where the link ends |
| `collider` | `box`, `sphere`, `capsule`, `cylinder`, `cone`, `auto` | a solid `Collider`; `auto` is a box around the mesh |
| `size`, `radius`, `height`, `offset` | numbers and lists | the shape's size and centre; without them, the mesh bounds are used |
| `layers`, `collides_with` | `world`, `character`, `volume`, `game0`…`game15` (a name or a list) | the collider's layers |
| `volume` | `water`, `updraft`, `trigger` | a `Volume` sensor with the `collider` shape (default `auto`); `accel` and `max_speed` are for the flow |
| `prefab` | text | spawns a prefab from the content at the node; component properties edit its fields |

Any other key is a **component**: a short name (`Health`) or the full type path. The value is JSON in the form that `inspect` and `get` show:

```text
Health  = {"current": 40.0, "max": 40.0}
Faction = "bandits"
Locked  = true                           (component with no fields)
```

A JSON object can be typed in as text (the *String* type): text that holds a JSON object or list is read as one. The *Python* type does the same without quotes: `{"current": 40.0}` becomes a dictionary, and the exporter writes it as a JSON object. A list of names (`["world", "game3"]` for `layers`) works the same way. A component must be registered for reflection (`#[derive(Reflect)]`, `#[reflect(Component)]`), as for prefabs. On a node with `prefab`, component properties merge into the prefab field by field (`Health = {"current": 10}` leaves the prefab's `max`). A node without these properties (a group, or a mesh without `collider`) spawns nothing, and the mesh stays a picture only. Keys with `_` are ignored.

**Engine markers** (`marker`). The game finds them with an ordinary `Query<(&LevelMarker, &Transform)>`.

| `marker` | entity | what the AI sees |
|---|---|---|
| `spawn` | `LevelMarker` | `SpawnPoint "start"` |
| `socket` | `LevelMarker` named after the socket type | no |
| `navlink` | `LevelMarker`, with `to` as the world position of the end | no |
| `trigger` | `LevelMarker`, a `Volume` sensor, and its collider | `Trigger "goal"` |
| `interactable` | `LevelMarker` | `Interactable "door"` |
| `camp` | `LevelMarker` | `Camp "bandits"` |
| `landmark` | `LevelMarker`, `AiLandmark`, `AiKind` from the marker's name | in the LANDMARKS line |
| `custom:<name>` | `LevelMarker` | no (needs an `AiKind`; add it as a component) |

The nodes of the file `crates/sandbox/assets/tiles/ruin.gltf` (a ruin of 16 × 16 m, exported from Blender 5.2.2 in the *Embedded* variant; `tools/blender/ruin_tile.py` builds the export, and the template keeps a copy of the same file) are: the floor `Ground`, the walls `Wall.North`, `Wall.West`, `Rubble`, `Column` with `collider = auto`; `Camp` (`marker = camp`, `name = ruin_camp`), `Arch` (`marker = landmark`, `name = Ruin`), and four `Edge.N/E/S/W` with `socket = grass`. The test `the_committed_ruin_is_the_tile_the_spec_describes` (`crates/sandbox/tests/sandbox/tiles.rs`) checks the export against the tile's description: the same nodes, properties, world transforms and mesh bounds. After you change the ruin:

```text
blender -b --factory-startup --python-exit-code 1 -P tools/blender/ruin_tile.py -- crates/sandbox/assets/tiles
cp crates/sandbox/assets/tiles/ruin.gltf template/assets/tiles/
cargo sandbox --bake-nav ruin         # and in template/: cargo run -- --bake-nav ruin
```

Exporting the ruin gives the same bytes every time.

**Checked on a real export from Blender 5.2.2 LTS** (Windows x64, exporter `Khronos glTF Blender I/O v5.2.40`). The script `tools/blender/outpost_level.py` builds the level `outpost` the way an artist would, and Blender itself exports it. The scene has: objects with unapplied rotation and scale; a wall made with an *Array* modifier; parents set with Ctrl+P (with and without the inverse matrix); empties as markers of every kind; sensors (water, updraft, trigger); colliders of every shape; a prefab with overrides; components from JSON text, from a *Python* dictionary, and from a checkbox; a collection placed twice; a lamp and a camera; names with spaces, `Säule` and `Костёр`. The files are in `crates/shards_level/tests/level/blender/` (`outpost.glb`; `outpost_lights.gltf` + `.bin` with a lamp; `outpost_embedded.gltf`, the same scene in one file, with the buffer inside as a `data:` URI; `outpost_draco.glb` with Draco; `outpost_meshopt.glb` and `outpost_meshopt_khr.glb` with meshopt, `EXT_meshopt_compression` and `KHR_meshopt_compression`; `outpost_broken.glb` with the artist's errors). The tests in `crates/shards_level/tests/level/from_blender.rs` read them without Blender, and `crates/shards/tests/level_render.rs` draws `outpost.glb` and `outpost_embedded.gltf` with the Bevy loader. Exports are not identical byte for byte (the order of the triangles in the `Boulder` mesh changes from run to run; the geometry is the same). So after you change the script, rewrite only the files that it changes (only without a window):

```text
blender -b --factory-startup --python-exit-code 1 -P tools/blender/outpost_level.py -- crates/shards_level/tests/level/blender
```

What the real export showed, and what was fixed in the engine:

- the *Data-Block* property (`to` = the object `Ladder Top`) is written by the exporter as the object `{"name": "Ladder Top", "type": "Object"}`;
  `to` now accepts that form too;
- with *Punctual Lights*, Blender marks `KHR_lights_punctual` as required (by the exporter's code, `KHR_texture_transform` is marked the same way for textures with a *Mapping* node), and the `gltf` crate rejected such a file without the right cargo feature:
  the level loaded in a build with `render` and did not load without it. Required extensions no longer get in the way (the engine reads neither lamps nor materials), except for compressed meshes (`KHR_draco_mesh_compression`, `EXT_meshopt_compression`,
  `KHR_meshopt_compression`, `KHR_mesh_quantization`): the error names the extension and the checkbox. Draco and both meshopt variants are checked on exports from Blender itself; the Blender exporter never writes `KHR_mesh_quantization` (other tools write it, for example gltfpack without `-noq`), and the error says this instead of the checkbox;
- the exporter writes properties on mesh data to the mesh's `extras`, and the engine silently dropped them; now it is a node error with a hint;
- Blender writes all the scenes in a `.blend` (in alphabetical order) and makes the active one the default scene: the scene `Kit`, with a collection, came first (index 0), and it was the level's picture (scene 0). The engine reads the default scene (`LevelAsset::scene`) and draws that one too.

The rest matched what is described here: axes and scales, node paths, all the markers, collider shapes and sizes, prefabs and components, `content check`, `level_info`, `spawn_level`, `level_reload` and hot reload. The glTF Embedded variant is read and drawn the same way as `.glb` (`the_glb_and_the_embedded_gltf_read_and_spawn_alike`). Not checked on export: animation and skins (the level pipeline does not read them).

**The file in the content.** The `level` kind (`LevelDef`): the file path, the required markers, the default place.

```ron
// content/region/ruin.level.ron
{
    "ruin": (
        file: "tiles/ruin.gltf",
        required_markers: [(kind: "camp", name: "ruin_camp"), (kind: "socket", name: "grass", min: 4), (kind: "landmark")],
        at: (0.0, 0.0, 0.0),
    ),
}
```

`shards content check`, `--check-content` and every content reload read the file itself and report a missing file or one that cannot be read (this includes compressed meshes); a node that names a component that does not exist, or a value that the component does not accept; an unknown prefab; a `navlink` without an end; engine properties that sit on the mesh data, not on the object; or a missing required marker. Each error names the content file, the level, the node and the property. Real output (in a copy of the sandbox content, with a typo in `Helth` and the marker `ruin_camp` renamed):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/region/ruin.level.ron: level `ruin`: node `Ruin/Column`: unknown component `Helth` (did you mean `Health`?)
content/region/ruin.level.ron: level `ruin`: required marker missing: `camp` named `ruin_camp` in `tiles/ruin.gltf` (found 0, need 1); add the custom property marker = "camp" and name = "ruin_camp" to an object in Blender
…
```

The loading gate holds the simulation until the files are loaded (`LevelLibrary`).

**Tools.** The player: `level_info`; the developer: `spawn_level`, `level_reload`.

```text
$ shards act level_info id=ruin
level ruin: tiles/ruin.gltf, at (0.0, 0.0)
13 nodes, 6 markers
  camp "ruin_camp" at (-2.0, 2.0)  Camp
  landmark "Ruin" at (1.0, 1.0)  Arch
  socket "grass" at (0.0, -8.0)  Edge.N
  …
checks: ok
$ shards act spawn_level id=ruin x=60 z=-40
spawned level `ruin` as instance 1 at (60.0, -40.0): 11 entities (5 solids, 0 volumes, 6 markers)
  camp "ruin_camp" at (58.0, -38.0)
  …
```

**What comes out of a spawn.** Entities for the nodes, in file order, all **top-level, with a world `Transform`** (physics reads `Transform`, and it is not recalculated inside a tick). The node's scale goes into the collider sizes; the entity's own scale is 1. Each entity has a `FromLevel` (the level, the instance number, the node path). If any node has a problem, nothing is spawned. One file can be placed several times (each time as a new instance). `despawn_level` and `reload_level` work with an instance.

**Hot reload.** The asset server rereads a file changed on disk on its own, and the instances are replaced through `level_reload` (it goes into the journal and the replay, like any action). A file with problems leaves the instances as they were. Entities that the game has already changed come back as the file describes them.

**Levels as region tiles.** A tile with `geometry: Level(path: "tiles/ruin.gltf")` is built from the file (see "World and generation (E5)"). The crates `shards_level` and `shards_world` do not know about each other; the umbrella crate joins them: `shards::TileLevelsPlugin` (part of `EnginePlugins`) asks for all the tile files while the game loads, and spawns them on each tile.

- The file lies in the tile's coordinate system: the origin is the centre of the tile, −Z is north, +Y is up, and a metre is a metre (a 16 m tile has a 16 × 16 m floor). The tile's rotation turns the file by quarter turns, clockwise.
- The nodes become entities as in any level, but with region tags (`TileId`, `RegionPart`): they leave with the region and get the same `AiId` and `ColliderId` in every run. Marker nodes (`marker`, `socket`) are also the tile's `Marker`, like the markers from inline geometry.
- **The solver reads sockets from the tile's RON, not from the file**: generation is a pure function of the seed, the version and the content, and it works without assets (`--worldgen-report`). The `socket` nodes and slots in the file are placed in the world for the game (joins, decoration at the edges); the solver does not read them back. `content check` checks that the file's socket markers match the tile's sockets in RON (section "Content (E4)").
- A tile file that is broken inside (nodes, components, prefabs) is found by `content check`: it reads the tile's file as a `level` file, and the error names the tile, the file and the node. A missing file, and a path outside the assets folder, fail the check in the same way. The loader is the second line of defence for a file that is damaged after the check (editing the `.gltf` itself in a running game does not trigger the content check again): an error with the tile names goes to the journal as soon as the file loads, and a region change that uses it is refused before anything is changed.
- A reloaded tile file gets its region tags inside the reload: the replay runs without frames, and a tag applied a frame later would diverge from the recording.

**Determinism.** A spawn is a pure function of the file and the place, in a fixed order, so `AiId` and `ColliderId` are the same in every
run. `spawn_level` and `level_reload` are actions; the replay records them. The replay check `level` hashes where each level is placed, and the digest of its file: a replay recorded on another version of the file diverges at the first check (after a change to a level, the reference replays are recorded again). While no level is placed, the check adds nothing to the hash (`add_optional_replay_check`): games without levels keep their old hashes, and the hash does not remember levels that were there and went away (a snapshot taken before the first level still loads exactly, even after a region with level tiles). The snapshot checks the same placed levels (`levels`), keeps the instance counter (`LevelCounter`) and the levels placed by id (`PlacedLevels`): loading places them again under the same numbers (see "Snapshots (E11.4)"). `LevelInstances` are ordered by number.

## World and generation (E5)

The `shards_world` crate. A region is a grid of macro tiles with sockets on the edges (or a dungeon of rooms that join door to door). The generator builds it from a seed, checks it, retries on failure, and in the end takes the fallback map. The result is a **manifest**: data from which the world is rebuilt without the generator. A region changes through the loading screen, and enemy AI works only in the active zones around the players.

The set of tiles, the socket rules and the check thresholds are the game's content; the order of the steps, the solver, the retries and the report are the engine.

```text
seed ─► Points ─► Roads ─► Cells ─► Fill (socket solver) ─► Secondary ─► Decorate ─► Checks ─► manifest
  points   roads    cells     tiles with rotations   more points     decor      rules
              any step or check fails ─► next attempt ─► … ─► fallback map (tile_map)
```

### Content formats

Seven kinds: `tile`, `socket_rules`, `poi`, `region`, `tile_map`, `room`, `dungeon`. All are checked together with the base, and errors name the file, the field and the nearest id. The sandbox's real files are in `crates/sandbox/assets/content/region/`.

**Tile** (`*.tile.ron`): edges with sockets, rotations, weight, slots for points of interest, and geometry.

```ron
// content/region/wilds.tile.ron — a 16 m tile; the tile's coordinate system: centre at the origin, north −Z, before rotation
{
    "road_bend": (
        sockets: (n: "grass road grass", e: "grass road grass", s: "grass", w: "grass"),
        rotations: [0, 90, 180, 270],            // by how many degrees clockwise the generator may turn it
        weight: 2.0,                             // relative frequency; the rotations share it
        slots: [(at: (-3.0, 0.0, 3.0), accepts: ["camp", "shrine"])],      // where a point of interest may stand
        geometry: Inline([Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0))]),
    ),
    "ruin": (                                    // geometry from a Blender file (see "Levels from Blender (E4.2)")
        sockets: (n: "grass", e: "grass", s: "grass", w: "grass"),
        rotations: [0, 90, 180, 270], max_count: 4,
        slots: [(at: (-2.0, 0.0, 2.0), accepts: ["camp"])],
        geometry: Level(path: "tiles/ruin.gltf"),
    ),
}
```

The other `TileDef` fields: `min_count` and `max_count` (how many such tiles the region has), `tags` (for the `#forest` sets and the limits), `pois` (points that the tile always has: `(poi: "spawn", at: (0, 0, 0))`), `links` (which sides are joined inside the tile: `[[N, S], [E, W]]` — a river ford with no bridge; by default any side connects to all of them), `slope` (slope class, 0 — flat), `decor` (prefabs for decoration, one per tile), `visual` (id for the picture in the window), `tile_size` (the cell size, in metres, that the tile geometry is made for: `Some(96.0)`; a region with another `tile_size` does not take it into "all tiles", and a region that names it by id, `#tag` or in `fixed` gets a content error with both sizes; without the field the tile fits any cells, and the field does not appear in the definition's JSON, so the content hash of the regions stays the same), `navlinks` (navlink markers of the tile's level file, as the reachability check takes them: `[(name: "drop", from: 0, to: 1)]` — the marker's name in the file, the `links` group the link starts from, and the group it goes to; one way only, from the marker to `to`, as the navmesh does; a `links` group with no sides, `[]`, is a place that is reached only by a navlink: a plateau, a pit; an empty field does not appear in the definition's JSON). Geometry (`TileGeometry`): `None`, `Inline([...])` or `Level(path: …)`. The pieces of inline geometry are `Box`, `Solid` (a shape of any kind), `Volume` (water, a flow, a trigger), `Marker` (a named point for the game and its modules), `Prefab`; they are spawned in list order.

**Sockets.** An edge of a tile is text made of sockets, clockwise around the tile: on the north edge from west to east, on the east edge from north to south, and so on. The clockwise order survives a rotation, and neighbours read the shared edge in opposite directions, so an edge matches the reversed edge of its neighbour. An edge of one word is the whole edge; `"grass road grass"` is a road in the middle third; `road@1` is a socket at height level 1. The `socket_rules` kind says what joins with what:

```ron
// content/region/wilds.socket_rules.ron
{
    "default": (
        kinds: ["grass", "road"],        // every socket kind that appears in the tiles (a typo in a tile is an error)
        passable: ["grass", "road"],     // you can walk through them (the reachability check, the navmesh)
        roads: ["road"],                 // these are roads (the road plan, the check "road to nowhere")
    ),
}
```

Also `pairs` (which different kinds join, symmetrically), `forbid` (they join with nothing: sealed sides, the region border), `any_height` (kinds for which the height does not matter: the face of a cliff) and `height_tolerance` (by how many height levels the sockets that join may differ).

**Point of interest** (`*.poi.ron`): what the AI calls it and what stands in it.

```ron
{
    "spawn": (label: Some("Spawn")),
    "camp":  (label: Some("Camp"), prefab: Some("camp_skeleton")),    // the camp spawns a skeleton when the region is built
}
```

**Region** (`*.region.ron`).

```ron
// content/region/wilds.region.ron
{
    "wilds": (
        size: (8, 8),                      // cells: east × south
        tile_size: 16.0,                   // metres
        version: 1,                        // generator version, see below
        origin: (-64.0, 48.0),             // where the north-west corner is in the world (x, z); the default is (0, 0)
        fixed: [(at: (1, 1), tile: "gate", rot: 0), (at: (5, 5), tile: "hall", rot: 0)],
        pois: [(poi: "spawn", cells: [(1, 1)]), (poi: "den", cells: [(5, 5)]), (poi: "shrine"), (poi: "camp", count: 2)],
        checks: (isolated_roads: false, distances: [(a: "spawn", b: "camp", min: 40.0), (a: "den", b: "camp", min: 40.0)]),
        fallback: "wilds_safe",            // tile_map for the case when no attempt passes
        active_radius: 40.0,               // NPCs farther than this from every player are asleep
        edge_wall: 4.0,                    // an invisible wall along the edge, metres above the ground; without the field the edge is open
    ),
}
```

The other fields: `rules` (id `socket_rules`, `default` by default), `tiles` (id and `#tags`; empty means all tiles made for cells of this size: with no `tile_size`, or with the same one), `border` (a socket that must stand on every edge of the border),
`limits` (a tile count by id or by `#tag`: `(of: "#forest", min: 4, max: 20)`),
`roads: true` (connect the required points with roads, as a spanning tree), `secondary` (points that go into the free slots
after the tiles, as many as fit), `attempts` (8 attempts by default), `backtracks` (2000 solver backtracks per
attempt), `spawn` (the point where players appear; the default is `spawn`). `checks` are the rules that the region must
satisfy:

| rule | what it checks |
|---|---|
| `reachable` (default: yes) | every point of interest can be reached from the spawn point over passable sockets, the tiles' `links` and their `navlinks` (one way only: a drop from a cliff takes you to the bottom, but not back; the rule does not require a way back) |
| `max_slope` | no tile is steeper |
| `distances` | every `a` is no closer than `min` to every `b`; every `a` has a `b` no farther than `max` |
| `isolated_roads` (default: yes) | every road leads to a point of interest |
| `custom` | names of the game's checks (`add_worldgen_check`) |

The check that the spawn point exists always runs too. Each failure names the rule and the cells (`check reachable`, `check
distance`).

**Fallback map** (`*.tile_map.ron`): a region made by hand, rows of cells from the north: `tile` or `tile:90` in a cell.

```ron
{ "wilds_safe": (rows: ["grass grass grass …", "grass gate grass …", "…"],
                 pois: [(poi: "spawn", at: (1, 1)), (poi: "den", at: (5, 5)), (poi: "camp", at: (6, 2))]) }
```

The checks read the manifest and the content, so they judge the fallback map and a saved manifest in the same way. A fallback map that fails a check is taken anyway, and a remark goes into `remarks`.

**What `content check` catches.** An unknown socket, tile, prefab or point; a region whose required points have nowhere to go; a spawn point that is not placed in any cell; a fallback map whose size does not match the region. Real output (in a copy of the sandbox content, with typos in a socket, a point and the fallback map):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/region/wilds.region.ron: region `wilds`: tile `road_t` uses socket kind `gras`, which rules `default` does not list (did you mean `grass`?)
content/region/wilds.region.ron: region `wilds`: pois[4].poi -> unknown poi `tower`
content/region/wilds.region.ron: region `wilds`: fallback -> unknown tile_map `wilds_safe2` (did you mean `wilds_safe`?)
3 content errors in /tmp/broken/assets
```

### Generator, solver, checks

The generator is a pipeline of named steps, by stage: `Points → Roads → Cells → Fill → Secondary → Decorate → Checks`. The engine
puts one step on each stage (`engine.points`, `engine.roads`, …), so a game that has only data works without code.
A game adds its own step after the engine's steps, or replaces a whole stage, and adds its own checks by name:

```rust
app.add_plugins(EnginePlugins::new(config))
    .start_region("wilds")                                  // generated with the world's seed while the game loads
    .add_worldgen_step(Stage::Decorate, "flowers", flowers)  // after the engine's steps on this stage
    .replace_worldgen_stage(Stage::Roads, "my_roads", my_roads)
    .add_worldgen_check("river_crossings", crossings);       // the name goes in the region's checks.custom

fn flowers(ctx: &mut GenContext) -> Result<(), String> {
    let mut rng = ctx.rng();                // the step's stream: key (seed, version, attempt, step), not SimRng
    // ... ctx.pois, ctx.roads, ctx.decor, ctx.filled ...
    Err("flowers: no meadow tile".into())   // "reason: details"; the reason groups the failures in the report
}
```

**The socket solver** fills the grid with rotated tiles whose edges join (in the spirit of WFC). Each cell starts with the
options (tile × allowed rotation) that the earlier steps left. Constraint propagation removes from the neighbours whatever does not join with what remains (AC-3 over the edge signatures), and the count limits remove a tile from everywhere once its count reaches the maximum.
The next cell solved is the one with the fewest options (on a tie, the lower index in row order), and the option is picked by weight from `DetRng`; a contradiction rolls back the last decision and forbids it, at most `backtracks` times per attempt. The decisions use only integers (weights in thousandths, ordering by the number of options), so one seed gives the same tiles on any platform. All the domain changes are written to an undo log, not copied: a decision costs memory in proportion to what it changed, and a 64 × 64 grid with backtracking fits in 8 MiB.

**Determinism.** Generation is a pure function of the seed, the region's generator version, the attempt number and the content. The random streams of the steps
come from `GenContext::rng()` (`DetRng`, keyed by these four numbers and the step's name), not from `SimRng`, and they never depend on the
order of a `HashMap`. The region's `version` field goes into every key: a new version gives old seeds new worlds, while manifests that are already saved
stay as they were. **If you change the generator's output, raise `version`.**

**Attempts and the fallback map.** An attempt that fails (a step returned `Err`, the solver found no tile, a check failed) is followed by the next one, with a
new key, up to `attempts`. If none passes, `fallback` is taken; without it, the region change is refused and names the
reasons for all the attempts. The first line of the journal says how it went: `region generated: wilds seed 42 v1 (attempt 1, 64 tiles,
5 POIs)`; a fallback map has `attempt: 0` in its manifest.

### Manifest

`RegionManifest` is the region as data, and it is kept for ever (the `region` section of the world save keeps it, E8, see
"Saves (E8.1–5)"): the seed, the generator version, the attempt that succeeded (0 — the fallback map), each tile with its rotation, the points of interest with their
positions, the roads or doors between them, and the hash of the content the region was made from. Loading a manifest **does not run the
generator**, so a game update that changes the generator does not rebuild old worlds: a saved world is built from its own
manifest, and the journal says where it comes from (`region wilds seed 42 v1: built from the save's manifest (the generator is v2 now)`).
The content must still be able to build its tiles, rooms and prefabs: a missing one is named by a refusal before anything changes
(`region::check_manifest`; `change_region` and `region_load` check it too). A save's section with a `wilds` 8 × 8 manifest is about 3 KB.

The canonical record (`to_canonical`, format `shards-region/1`) is stable byte for byte: keys in alphabetical order, numbers with no more than
three decimal places, one tile per line. One seed, version and content give the same bytes, and `RegionManifest::hash` is FNV-1a
over them. Positions are rotated by exact permutations, not by sines. The real manifest of `wilds` with seed 42 (88 lines):

```json
{
  "attempt": 1,
  "content_hash": "37778bd5b35f5e91",
  "format": "shards-region/1",
  "layout": "grid",
  "links": [],
  "origin": [-64, 48],
  "pois": [
    {"cell": [1, 1], "id": "spawn.0", "poi": "spawn", "pos": [-40, 0, 72]},
    {"cell": [5, 5], "id": "den.0", "poi": "den", "pos": [24, 0, 140]},
    …
  ],
  "region": "wilds",
  "seed": 42,
  "size": [8, 8],
  "spawn": "spawn.0",
  "tile_size": 16,
  "tiles": [
    {"at": [0, 0], "rot": 0, "tile": "grass"},
    {"at": [3, 0], "rot": 0, "tile": "pillars"},
    {"at": [4, 0], "rot": 90, "tile": "ruin"},
    …
  ],
  "version": 1
}
```

(`origin` is not written when it is `(0, 0)`, so the region manifests at the start of the world stayed the same, byte for byte and hash for hash.) The hash is shown in `region_info` (`"manifest_hash": "7bbfdc3b62623fbc"` for this region) and in `ai/status` → `world`. The `region` scenario compares it with the reference value.

### Dungeons

A dungeon is a set of rooms that join door to door. The `room` kind: size in cells, doors on the edge of its footprint, rotations, weight, points and slots, geometry. The `dungeon` kind: entrance, goal, room sets, hand-made rooms and path length.

```ron
// crates/shards_world/tests/world/assets/content/dungeon/crypt.dungeon.ron
{ "crypt": (version: 1, cell_size: 8.0, entrance: "entry", goal: "throne", rooms: ["#crypt"],
            fixed: ["vault"],            // hand-made room: a fixed node on the main path
            path: (3, 6),                // rooms between the entrance and the goal, the hand-made ones among them
            branches: (1, 3),            // extra rooms off the path
            max_size: (14, 14), secondary: [(poi: "chest", count: 2)],
            checks: (distances: [(a: "spawn", b: "boss", min: 16.0)])) }
// …crypt.room.ron
{ "vault": (size: (2, 1), doors: [(at: (0, 0), side: W), (at: (1, 0), side: E)],
            pois: [(poi: "vault", at: (0.0, 0.0, 0.0))], slots: [(at: (4.0, 0.0, 0.0), accepts: ["chest"])],
            geometry: Inline([Box(min: (-8.0, -1.0, -4.0), max: (8.0, 0.0, 4.0))])), … }
```

The main path is placed first: the entrance, then the `path` rooms (the hand-made ones among them, at random places), then the goal. Each next room joins the door that faces a free door of the previous room. The path is found by depth-first search with limited backtracking, so a dungeon that gets built is always passable from the entrance to the goal. Then branches are attached to the free doors. All the random draws come from `DetRng`, keyed by the seed, the version, the attempt and the stage, as for grid regions. A dungeon is built the same way as a region (`regen
region=crypt`). Its manifest has `layout: rooms`, and its links are `door`. A hand-made room that cannot be placed in the middle of the path (it has fewer than two doors) is a content error. The lower bound of `path` is raised to the number of hand-made rooms.

### Batch checks: `shards worldgen`, `--worldgen-report`

A region is generated for many seeds without a window, and nothing is built. The first line starts with `ok:` (all passed) or `failed:`. Then come the reasons for the failed attempts with example seeds, the distances between points (min / median / max), and the generator's panics with the seed number.

```text
$ cargo sandbox --worldgen-report wilds --seeds 1..1000
ok: worldgen wilds v1, seeds 1..1000: 1000/1000 passed (42 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 0.31s, 0.3 ms/seed]
failed attempts (43):
  43× check distance (e.g. seeds 19, 25, 71)
distances between points (m, min / median / max):
  camp–camp  9.5 / 66.0 / 155.7
  camp–den  40.0 / 65.2 / 93.8
  camp–shrine  9.5 / 68.7 / 154.8
  camp–spawn  41.0 / 78.1 / 140.0
  den–shrine  9.5 / 55.7 / 118.9
  den–spawn  93.4 / 93.4 / 93.4
  shrine–spawn  12.0 / 70.3 / 140.0
```

The exit code is 0 if all seeds passed, otherwise 1. An unknown region also gives 1, with the hint ``unknown region `wild` (did you mean `wilds`?)``. Seeds: `1..1000` (both ends included), `7`, `1,5,10..20`; no more than a million. The running game gives the same report: `shards worldgen --region wilds --seeds 1..1000 --report` (tool `worldgen_report`). A dungeon is checked the same way (`--worldgen-report crypt`): a thousand seeds pass in 0.17 s. CI runs this report for the sandbox and the template.

### Loading a region

Tools (for developers; actions that the replay records and repeats on the same tick):

- `regen region=ID seed=N` — generate a region (or a dungeon) and move into it; with no arguments, the next seed of the current region;
- `region_load manifest=…` — build a region from its manifest, as it was (from `region_info manifest=true`), without generating anything;
- `goto_tile x y [id]` — teleport to the centre of the tile, onto the ground (also a floor above the tile's plane, not inside it);
- queries: `region_info` (for the player: which region, points, manifest hash; `manifest=true` gives the whole manifest) and `worldgen_report`; with a window also `world_overlay` (tile borders, sockets, points, roads).

```text
$ shards act regen region=wilds seed=42
ok: wilds seed 42 v1 (attempt 1, 64 tiles, 5 POIs, 199 entities), spawn at (-40.0, 72.0)
CHANGES
  ~ e5 Player: moved 72.1m SW
  + e23 Spawn "spawn.0"  72.1m SW
  + e24 Den "den.0"  130.2m S
  + e25 Skeleton "Den Guard"  hp 40/40  poise 20/20  130.2m S
  …
JOURNAL
  0 region generated: wilds seed 42 v1 (attempt 1, 64 tiles, 5 POIs)
  0 region loaded: wilds seed 42 v1 (64 tiles, 203 entities, players at (-40.0, 72.0))
  0 navmesh: wilds seed 42 v1 (64 tiles, 65012 nodes, 7112 links across edges, 0 navlinks)
  0 navmesh (body large): 64 tiles, 64180 nodes, 7056 links across edges, 0 navlinks
```

The manifest from `region_info manifest=true` brings the region back as it was, generating nothing (the hash is the same):

```text
$ shards act regen region=wilds seed=7                 # another region…
$ shards act region_load manifest='{"attempt": 1, "content_hash": "37778bd5b35f5e91", "format": "shards-region/1", …}'
ok: wilds seed 42 v1 loaded from its manifest (64 tiles, 199 entities)
$ shards act region_info | grep manifest_hash
  "manifest_hash": "7bbfdc3b62623fbc",
```

What happens inside the call (`change_region`): all tiles, prefabs and level files are checked **before** anything is touched. A typo in a prefab leaves the old region in place, not a world with no region. The state goes `InGame → RegionLoading → InGame` (loading screen: ticks do not run, `OnExit(InGame)` and `OnEnter(InGame)` do not run for this trip, the players stay). The entities of the old region leave (`region unloaded: …`). The new one is built tile by tile, row by row (rooms by their north-west cell), with the geometry pieces of each tile in file order. Then come the points of interest (a landmark entity with `AiKind` from the point's label, and the point's prefab: a camp spawns a skeleton). Players are moved to the spawn point. Modules subscribed to `on_region_loaded` (the navmesh) finish building their own data. All entities are top-level, with a world `Transform`, with `TileId` and `RegionPart`. So the same manifest gives the same `AiId` and `ColliderId` on every run, with a window and without one. If some other module is still loading something (a level file), the loading gate holds the ticks until everything is ready. Changing the region does not shift a single tick.

**Starting region.** `start_region("wilds")` in the game's plugin generates a region with the world's seed while the game loads, and builds it before `OnEnter(InGame)`. Its `AiId`s come first. A game without this starts with no region. The sandbox does not do this: its "wilds" are started with `regen`.

**A region next to other places.** The `origin` field puts the region's north-west corner anywhere in the world. Cell centres, points and the navmesh are counted from it. The sandbox keeps the wilds south of the arena and the obstacle course, not instead of them. In `region_info`, such a region has `north_west_corner`. In `observe`, `TILE outside …` shows when the player is outside it.

**Region edge.** `edge_wall: H` puts an invisible wall around the region: the top at H m above the ground (y = 0), the bottom 100 m below, and 1 m thick outward from the edge. So nobody can walk out, dodge out or be knocked off the edge of the world. The wall is built with the region and leaves with it: four `region edge` boxes without an `AiId`, with the `RegionEdge` label. A game that draws every collider skips them, and the template does this. Their `ColliderId`s come after the tiles and points, so those keep their numbers. The wall does not change any tile or point for any seed. The manifest's `content_hash` changes (it includes the region definition), and with it the hash. Without the field the edge is open. Leave the field out for a region that stands right against a hand-made place, or whose outer tiles are already walls and cliffs. In `region_info` the wall has a line `edge: walled, 4m high`. The sandbox's wilds and the template's valley are built with a 4 m wall: the first `shards soak` run took a client's body past the west edge of the wilds.

### Active zones and streaming

Every tick in `SimSet::Input` (before the orders run), each character that is not a player (it has an `Intent`, and no `PlayerId` or `AiPlayer`) gets an `ActiveZone` within the radius of some player, and `Dormant` beyond it. To keep a character on the border from flickering, an active one goes dormant only `active_margin` (8 m) farther out. The radius is the region's `active_radius`, otherwise `WorldSettings::active_radius`, otherwise 96 m. Without a region and without a zone setting there are no zones, and everything works. NPC brains and navigation skip `Dormant`; it is shown in `observe` (`dormant`). When the player is in the wilds, the arena's monsters sleep:

```text
  ~ e6 Boss "Frost Warden": +dormant
  ~ e7 Skeleton: +dormant
```

**Streaming.** The region's geometry for the game is built whole when the region loads. The picture (a window or `--offscreen`) still shows tiles as they come and go: the game makes the tile's picture itself (`set_tile_visual_spawner`, by LOD), and the engine creates and removes it by distance to the nearest player (`WorldSettings::visual_lods`, 96 and 256 m by default; beyond the last boundary it is removed). This is only the picture. Without it (`--headless`) nothing streams, and this does not affect the ticks. There is no loading of tile geometry for physics by distance: the whole region is in memory. At the current sizes it is not needed. A full-size region from the spec (the sandbox's `expanse`, 8×8 tiles of 96 m, about 1900 colliders; the `reference96` performance test, section "Frame time and the performance test (E10)") loads in 0.6 s in a release build. That includes the navmesh for both body classes (2.3 million nodes each), against the spec's budget of 10–15 s. The tick is 0.5–0.6 ms on average, against a 16.7 ms budget, and the colliders take less than a megabyte. Physics queries do not grow with the region, thanks to the broad phase (section "Movement (E2)"). Without it, the same load took 3.9 s, and the tick was 0.9 ms. Geometry streaming will be needed if colliders grow by an order of magnitude (a region built from level files with thousands of details per tile), or if tiles start to load from disk during play.

Foliage (E10.2) is scattered over the tiles once, when the region is built, and leaves with its tile. How far it can be seen is set by its levels of detail, not by streaming. So on a full-size region it has to be sparser. The wilds' density (0.4 tufts per m²) on 64 tiles of 96 m would give about 470 000 mesh entities. The `expanse`'s grass is twenty times sparser (about 31 600 mesh entities with all levels of detail).

### The region as the AI sees it

```text
$ shards observe
YOU  e5 Player  hp 100/100  stam 100/100  poise 60/60  flask 3/3  god mode  at (-40.0, 72.0) facing N
TILE (1, 1) gate rot 0 · wilds seed 42 v1
POI  e23 spawn.0 0.0m here · e26 shrine.0 70.3m SE · e29 camp.1 84.0m S
```

`TILE` is the tile under the subject (`TILE outside wilds seed 42 v1 (8×8 tiles of 16m from (-64, 48))`, if it is outside the region). `POI` lists the three nearest points with direction and distance. `ai/status` gets a `world` field (region, seed, version, number of tiles, manifest and content hashes). The condition `step --until named:near_poi` stops the step when the player is within 3 m of a point of interest (it is registered with the first region).

## Navigation and NPCs (E6)

The `shards_npc` crate covers the enemies that the game describes with data, and the navigation they walk on. There is no language model here: behaviour is tables in the content and ordinary code. An NPC drives its body through `Intent`, as the player and an agent do (`Order::Attack`). So it attacks and moves by the player's rules (stamina, windup, cooldown).

- **Behaviour**: a state machine from the content (`behavior`); the logic of states and conditions is in named functions of the engine or the game.
- **Perception**: sight by rays cast in the physics world, hearing, and memory of the target's last known place.
- **Combat director** (`director`): attack tokens by the number of players, an encounter budget, allowed combinations, and roles for the NPCs that wait.
- **Attack planner** (`attack_plan`): weights, distance windows, cooldowns, bans on sequences; for enemies and bosses.
- **Navmesh**: tiles are baked in advance, stitched together when a region loads, and routes run over the graph of tiles.
- **Navigation agents**: following a route, a leash around home, and avoiding bodies (NPCs and players) by picking from a fan of velocities.

An NPC is a fighter (`Fighter`) with the `Npc` component (and the optional `Home` and `Patrol`). The rest comes with it through `#[require]`. In a prefab this is one line, and no game code is needed (`crates/sandbox/assets/content/region/camps.prefab.ron`):

```ron
"Npc": (behavior: "skeleton", perception: "guard", plan: Some("skeleton"), director: ""),
"Home": (pos: None, leash: 12.0),          // pos: None — where the NPC stands on the first tick
```

### Behaviour

Each tick, every active NPC (in `AiId` order) first tries the transitions of its state, in order. The first one whose conditions are all met is taken (no more than one per tick). Then the state's hook says what the NPC wants now: where to go, where to look, whether it needs a token, whether to strike, and what to press. The conditions and states only read the world, so the order in which the NPCs are visited changes nothing.

```ron
// content/npc/monsters.behavior.ron
{
    "skeleton": (
        initial: "idle",
        states: {
            "idle": (transitions: [(to: "fight", when: ["target_known", ("target_in_leash", {"margin": 2.0})])]),
            "fight": (kind: "attack", transitions: [
                (to: "home", when: [("!target_in_leash", {"margin": 2.0})]),    // "!" — negation
                (to: "home", when: ["target_lost"]),
            ]),
            "home": (kind: "return_home", params: {"heal": 10.0}, transitions: [
                (to: "fight", when: ["target_known", "target_in_leash"]),
                (to: "idle", when: ["at_home"]),
            ]),
        },
    ),
}
```

State: `kind` (the hook; by default, the state's own name), `params` (parameters on entry), `duration: (min, max)` (a timer on entry, from `SimRng`; condition `timer_done`), and `transitions`. A condition is `"name"` or `("name", {"param": number})`. A typo in the state a transition goes to is a content error, with a hint. An unknown condition or hook is named once in the journal and never runs.

| hook (`kind`) | what it does | parameters |
|---|---|---|
| `idle` | stands still | `face_target` |
| `hold` | stands facing the target, on its leash | |
| `patrol` | walks the `Patrol` points in a loop | `speed`, `within`, `wait` |
| `chase` | runs to the target, staying on the leash | `speed`, `within` |
| `return_home` | goes home and heals | `speed`, `within`, `heal` (health per second) |
| `wait_token` | waits for a token in one of the director's roles | `speed` |
| `attack` | with a token, the planner picks an attack, moves in and presses it; without one, it asks for a token and waits in a role | `speed` |
| `flee` | runs away from the target | `speed`, `distance` |
| `investigate` | goes to a sound or a remembered point | `speed`, `within` |

Engine conditions: `target_known`, `target_seen`, `target_heard`, `target_lost`, `target_within {meters}`, `target_in_leash
{margin}`, `beyond_leash {margin}`, `at_home {within}`, `arrived`, `has_token`, `attack_done`, `resting`, `busy`, `timer_done`,
`in_state {secs}`, `health_below {fraction}`, `heard_noise`, `chance {p}` (from `SimRng`), `deciding` (see "Decision cadence").

**Your own functions in the game**: named functions that the content refers to. A game function with the same name as an engine function replaces it.

```rust
app.add_npc_condition("low_morale", |c, p| c.health_fraction() < p.num("below", 0.3))     // fn(&NpcCtx, &Params) -> bool
   .add_npc_state("rally", |c, p, out| {                                                   // fn(&NpcCtx, &Params, &mut NpcOutput)
       out.goal = c.home;                      // go home; navigation handles the route, the avoidance and the leash
       out.within = 2.0;
       out.speed = p.num("speed", 1.0);
   });
```

`NpcCtx` gives access to the NPC itself, its target (`target`: the distance, and whether it is visible), its leash, its token, its plan, the world (read only), `rng(salt)` from `SimRng`, `deciding()` (a decision tick, see below) and `strike_ready()` (the strike spacing allows a strike now, see "Strike spacing").

**Decision cadence.** `think: Some(0.15)` on a behaviour gives each of its NPCs a decision timer (the component `NpcThink`, present only on NPCs of such behaviours). The timer counts down only while the NPC runs no action, and a busy NPC freezes it. Each time it runs out, the tick is a decision tick: `NpcCtx::deciding()` and the condition `deciding` hold, and the timer starts over (the remainder is not carried). It starts at zero, so the first tick the NPC is free is a decision tick. The period is whole ticks: `think: 0.15` at 60 Hz decides every 9 ticks (`ceil(0.15 / tick)`, with the usual hair of slack for sums of tick lengths), and `0.25` every 15. A behaviour without `think` has every tick as a decision tick, and nothing is stored. The states and conditions that *decide* (roll a die, pick a move) ask `deciding()`; what has to be done every tick (aiming, moving) does not wait for it. The engine's own states re-derive their output every tick and ignore the cadence. Transitions are not held back by it: a transition that should wait for a decision lists `deciding` among its conditions (`when: ["deciding", ("chance", {"p": 0.08})]` is a roll once per decision).

**Rest clamp.** A state can shorten the plan's rest. `rest_clamp: Some(0.25)` on a state clamps `NpcPlan.rest` to at most that many seconds once, when the NPC enters the state (a rage that makes the next attack come sooner); a state hook can do the same on any tick with `out.rest_cap = Some(0.25)` (set it once: a hook that sets it every tick clamps every pause drawn later).

### Perception

The parameters belong to the `perception` kind: `sight_range`, `fov_deg`, `eye_height`, `near`, `line_of_sight`, `hearing`, `memory`, `keep_range`.

```ron
// content/npc/monsters.perception.ron
{
    "skeleton": (sight_range: 18.0, fov_deg: 360.0, near: 3.0, hearing: 1.0, memory: 6.0),
    "guard":    (sight_range: 14.0, fov_deg: 360.0, line_of_sight: false, memory: 3.0),    // knows its hall: the pillars do not hide it
}
```

Sight: a hostile fighter (`Team`) within `sight_range`, in the field of view (or closer than `near`, whichever way it faces), with a clear ray from the eyes to the body through the `WORLD` layer. Hearing: `Noise { position, radius, source, origin }` events (the game fires them itself; the engine makes a noise on every hit and every sprint) reach an NPC within `radius × hearing` of `position` and are kept in its `Perceived`. An NPC that does not see the source learns its place from `origin` (where the source stands; `None` means `position`). A hit's noise sounds where the hit happened, and the attacker is marked where it stands. Memory: what was noticed stays known for `memory` seconds, at the last place. The focus target sticks to the one already known, until another one is much closer. What an NPC knows lies in its components (`Perceived`), and goes into the replay hash and the snapshot in full.

**Keep range.** `keep_range: Some(18.0)` makes `sight_range` govern only what is noticed *anew*: a target that is still known (inside its `memory`) is seen, and its place refreshed, up to `keep_range` metres, with the same field of view and a clear ray. Without it, a target between `sight_range` and where the NPC's chase should end is "lost" at once and chased to a stale point until the memory runs out. `keep_range` must be at least `sight_range` (a content error otherwise). Hearing and the focus rule are unchanged. `keep_range` does not drop the target: beyond it the target is merely not seen and stays known for `memory` seconds. To give up the chase at a distance, add a transition on `!target_within N` to a return or wander state. A target known only from a noise or a hit is kept out to `keep_range` too; set `hearing: 0` where that is unwanted. `npc_state` shows `perception.keep_range` and marks a target seen beyond `sight_range` as `kept`; `npc_overlay` says `keeps e5 17.2m N`.

### Combat director and tokens

```ron
// content/npc/monsters.director.ron
{
    "default": (tokens: {1: 2, 2: 3, 3: 4},                  // attack tokens by the number of living players
                waiting: [(role: Circle, distance: 3.5, orbit: 1.2), (role: Strafe, distance: 3.0, period: 1.5), (role: Hold, distance: 5.0)],
                patience: 3.0),
    "boss": (tokens: {1: 1}, waiting: [(role: Hold, distance: 2.6)]),           // the boss fights alone
}
```

Only an NPC with a token starts an attack, and it holds the token until the attack ends. The director has as many tokens as the table gives for the number of living players. (With more players than the table has, the last entry applies; with fewer, the first.) Those who ask are served in the order they started waiting, then by `AiId`. A token comes back when the attack ends; when it was not used for `patience` seconds (`unused`); when its holder dies, falls asleep (`dormant`), forgets the target (`target gone`), leaves the fight (`left the fight`), or is put under an agent's orders (`under orders`). When the number of living players drops, the tokens above the new limit come back at once, the newest first (`limit lowered`). The exception is an attack that is already running: it goes on to its end with its token. An attack is an action that a pressed input started, not a forced one (a stun, a flinch, a death). Whether it has hits does not matter. A cast with a projectile from an effect is an attack too: it holds its token to the end, and the planner remembers it (cooldown, bans on sequences, counter). A target that the NPC has forgotten (it has left its perception) is gone for the director and the planner. The planner presses only on a target that the NPC senses now. An NPC under an agent's orders (via the `move_to` and `attack` tools) drops out of the fight: it takes no tokens and no waiting places, presses nothing, and lets go of what it held.

Without a token, an NPC waits in a **role** around its target. The waiting NPCs get places in turn: place `n` takes role `n % number of roles` from `waiting` (`Circle` walks in a circle, `Strafe` steps sideways, switching sides every `period` seconds, `Hold` keeps its distance). Also: `budget` is how much danger (the `cost` of attacks) may be in flight at one target. `allowed` is which pairs of tags may hit the target at the same time (`("light", "heavy")`, and `"*"` for any tag). The planner checks them when it presses. The director has no state of its own. Everything is in the NPC's components (`AttackToken`, `NpcPlan`), and the replay and the snapshot carry all of it.

**Strike spacing.** `min_spacing: 0.7` on a director means that strikes of different NPCs of that director start at least 0.7 s apart (two nearly simultaneous blows cannot both be parried). `spacing_exempt_tags: ["reposition", "support"]` names the actions that are no strike: a retreat or a buff neither waits nor counts (matched against the action's tags, the plan option's tags and the option's name). The rules:

- Every strike an NPC of such a director starts is *noted* on the NPC, as the component `NpcStrike { last_start, gated, pool }`, at the tick its action began: a planner press, a combo continuation, or something a state hook pressed itself. A note counts only while the NPC is still in the pool (`pool`, its director) it was made in, so moving an NPC to another director leaves its old strike out of the new pool's spacing and of the rule. Exempt actions are not noted, forced actions (a stagger) are not strikes, and an NPC of a director with no `min_spacing` never gets the component (and loses it when its director stops spacing), so a game that does not use the rule stores and hashes nothing.
- A strike is *spaced* when no other NPC of the same director noted one within the last `ceil(min_spacing / tick)` ticks (0.7 s at 60 Hz is 42: the second NPC may start on the 42nd tick after the first). An NPC's own series is not held back by itself. A press that has not started yet counts from the tick it was pressed, and so do the presses of the current tick: the planner's own, in `AiId` order, and those already in an NPC's `NpcDrive` that the action machine would take this tick (a state hook's, a combo continuation's), so the planner yields to them and two planned strikes never start on the same tick.
- The director grants no token for a strike that is not spaced (only if every option it could choose is exempt), the planner does not press one and a combo continuation (`chain`, below) that is not spaced is refused and ends its chain. A token holder that has to wait keeps its token like any holder that cannot press yet (until `patience` runs out), and the options the director exempts stay open to it.
- A state hook that presses a strike itself asks `NpcCtx::strike_ready()` first (it is true for an NPC of a director without a spacing). Every NPC decides on the same world, so two hooks that are both ready on the same tick both press, and a hook never sees the planner's press of its tick (a hook decides first): for ordered strikes use the token path.
- The clock is the simulation's tick count: the spacing is the same with a window and without one, in replays and in snapshots. All NPCs of a director are one encounter; telling encounters apart by distance is not done yet.
- The engine rule `npc.strike_spacing` holds the starts that went through the rule (`gated`: the planner's presses and checked continuations) to it; a hook's own press is noted but neither held to it nor held against the planned starts that follow it.
- **Noting only.** `ignores_spacing: true` on an NPC's attack plan makes it a *noting-only* member of the director: its strike starts are noted and hold the other NPCs back exactly as above, but it never waits for the spacing itself. The grant, the planner, a combo continuation and `strike_ready()` all treat it as spaced, and `npc_state` never shows `spacing` for it. The reason is the design (CDD 27.2): a big enemy's blows pace the small ones around it, and the small ones must not hold the big one back, which a plain exemption (`spacing_exempt_tags`, neither waits nor counts) cannot say. It is a property of the plan, so the same director serves a boss and its adds, and it changes nothing for a director without a `min_spacing`. The engine rule does not hold such an NPC to the spacing, but it still holds the others to its starts; for that reason the other NPCs' chains in a director that has one should use `gate_at_start` (below): a continuation asked at its press would otherwise be overtaken by a start the noting-only NPC makes before the combo window takes it.

`npc_state` shows `strike: {last_start, gated}` and, while the NPC is held back, `spacing: {waits_for: "e7", ticks_left: 31}`; `npc_overlay` adds `waits 31t for e7` to its line.

```text
$ shards scenario crates/sandbox/scenarios/npc_director.scenario   # three skeletons against one player
  123 e7 idle → fight e5
  123 e8 idle → fight e5
  123 e14 idle → fight e5
  123 e7 takes an attack token on e5 (1/2)
  123 e8 takes an attack token on e5 (2/2)
  209 e7 -> e5 claw -0 (hp 100, god mode)
  209 e8 -> e5 claw -0 (hp 100, god mode)
  252 e7 returns its attack token after light (0/2)
  252 e8 returns its attack token after light (0/2)
  252 e14 takes an attack token on e5 (1/2)
```

### Attack plans

The `attack_plan` content kind lists the attacks an NPC picks its next one from. The skeleton and the boss use the same planner.

```ron
// content/npc/monsters.attack_plan.ron
{
    "skeleton": (options: [(input: "light", tags: ["light"])], rest: (1.4, 1.9)),
    "warden":   (auto: true, in_range_only: true,                       // any moveset attack that reaches the target now
                 bans: [(after: ["zone"], then: ["zone"], within: 1)],  // never two "zone" attacks in a row
                 rest: (1.2, 1.8)),
}
```

Option (`AttackOption`): `input` of the moveset or `action`, `name`, `weight`, the distance window `min`/`max` (by default, the action's reach times `cover`), `cooldown`, `tags`, `cost` (the share of the target's budget), `when` (conditions), `chain` (a combo continuation).
The planner keeps the options it can pick: not on cooldown, not banned by the previous attacks, whose `can_start` in the action machine says "now or later", and that fit the budget and the allowed combinations.
It picks one by weight from the options whose window holds the distance (otherwise from all of them, unless `in_range_only`), using the `SimRng` stream.
Then it moves into that window and presses the input.
The `rest` pause counts down only while the NPC is not busy with any action. A stun after a parry does not count as rest. Cooldowns always run.

**Rest semantics.** Two more fields of the plan shape the pauses (all optional; unset, the plan behaves as above):

- `rest_interrupted: Some((0.2, 0.45))`: the pause after an attack that was *interrupted*, that is, cut short by a forced action in its place (a stagger), instead of `rest`. It is drawn as it is, with no scaling. Without it an interrupted attack rests like a finished one.
- `first_rest: Some((0.5, 0.5))`: the rest an NPC starts with, drawn on the tick its brain first runs (its pause at spawn). Without it the NPC asks for a token at once.

Every draw comes from the NPC's own `SimRng` stream with its own salt.

**Press on decision.** `press_on_decision: true` on a plan makes the planner ask the director for a token, make a new choice and press only on the NPC's decision ticks (`BehaviorDef::think`; `NpcThink.decided` is the tick of the last one, shown by `npc_state` under `think`). The reason is the design: the technical design has the brain decide at 5-10 Hz, and the attack press is part of that decision, so the grant, the choice and the press happen on one tick and no token sits idle until the next decision. Between two decisions an NPC keeps the choice it made and walks into its window (with `in_range_only` it chooses again on decision ticks only); an NPC with no choice yet stands. The token keeps its place in the queue between decisions. Off (the default), any tick; a behaviour without `think` decides every tick, so there the field changes nothing. It serializes nothing at its default.

**Conditional options.** `when: ["enraged", ("health_below", {"fraction": 0.5})]` on an option (the same conditions as a behaviour's transitions, `!` negates) keeps the option out of the choice, and out of the director's reckoning of whether the NPC has anything to strike with, while the conditions do not all hold. A phase that adds a move or takes one away is a condition on the option, not a second plan. The conditions are evaluated with the NPC's other decisions (`NpcSystems::Decide`, on the world as it is) and the result is kept on the NPC as `NpcOptions { allowed }`, a component that only NPCs whose plan has gated options carry; an option that has `when` and no entry there (the NPC's first tick) is closed. An unknown condition is named once in the journal and never holds. `npc_state` lists `plan.options: {open, closed}` for such a plan.

**Combo continuation.** `chain: Some((input: "light", chance: 0.7))` on an option says that when its action starts, a die is rolled (the NPC's `SimRng` stream, its own salt) and, on success, the NPC presses `input` at the *start of the action's recovery phase* (the phase named `recovery`, else the last one). The press goes into the same press queue as anyone's, with the same buffer life (`input_buffer`, 0.15 s), so it takes effect only if the action has a cancel window that lets the input through (`Combo(...)`) opening before the press dies: a window that opens later than the buffer life makes a chain that never fires, as the content says, with no special case in the engine. The continuation is itself an action start and rolls the odds written on the option that names its action (an `action: "swing_2"` option of `weight: 0.0` carries the link of a continuation that no moveset input starts; the planner never picks it). The state is the component `NpcChain { input, instance, pressed, strike, at_start, cleared }` (`pressed` is the tick of the press; inserted when a roll succeeds, removed when the attack ends, is cut short by a forced action, or its continuation starts or is refused). If the director spaces strikes and the continuation is a strike, a continuation that is not spaced is *refused*: nothing is pressed and the chain ends. The spacing is checked when the press is made, not when the combo window takes it, so a strike another NPC starts between the two is not counted against the continuation. `chain: Some((input: "light", chance: 0.7, gate_at_start: true))` asks it on the tick the action machine would take the queued input into the combo window (`can_start` says yes) instead. The design spaces attack *starts*, and the press and the start are 5-12 ticks apart, so a chained link is gated on the tick it would start: the press is made at the start of the recovery as before (and dies in the queue as before) but is no start for the others until then; if another NPC began a strike less than `min_spacing` ago the input is withdrawn from the queue, the chain ends and `ChainRefused` is journaled; if not, the NPC counts as starting on that tick for the planners after it (`NpcChain.cleared`, the tick it was let through). A press whose window is already open when it is made is asked at once. A pending press counts as a start for the others only while its input still waits in the queue (or on the tick it was pressed): a press that died unused started nothing. A chain rolls the odds of the first *open* option that names its action (an option with `when` counts only while it is open), so one action can carry different odds under different conditions. `ChainPressed` and `ChainRefused` are journaled (`e7 chains light after swing_1`, `e7 chain refused after swing_1: e8 began a strike 12 ticks ago (30 left)`); `npc_state` shows `chain`.

### Leash, home and zones

`Home { pos, leash }`: where the NPC's home is and how far it can go from it, in metres. The NPC's targets stay inside the leash, with a 0.5 m margin for the body. The conditions `target_in_leash`, `beyond_leash` and `at_home` let the behaviour turn the NPC back home. The arena skeletons in the sandbox have a 30 m leash, the guard 26 m, the camp skeletons in the wilds 12 m, and the lair's guard 10 m. An NPC in `Dormant` (farther than the active radius from every player, see "World and generation (E5)") does not think or move. It wakes up on the tick a player comes close.

Conditions for `step --until`: `named:npc_attacking` (someone's attack is under way), `named:npcs_calm` (no one has a target), `named:npc_beyond_leash` (an active NPC is a metre beyond its leash; the step meant to keep the NPC in its zone must not stop on this), `named:npc_detour` (the NPC walks a route of several legs because the target is out of sight), `named:npc_avoiding` (an active NPC leaves its route to go around another body; see "Avoiding bodies"), `named:npc_engaged` (an active NPC with a target in one of the states `chase`, `attack` or `wait_token` is fighting; a calm NPC that sees someone is not), `named:npc_near` (a living player is closer than one and a half times the active NPC's sight range: the NPC is about to notice them). The last two are what the music listens for (see "Audio").

### Navmesh

Every tile (and every dungeon room) is **baked** ahead of time into a grid of walkable cells. The geometry of a tile is spawned alone, in an empty world, by the same spawner as the region. It is probed with the same ground probe and the same movement step rule (`step_onto`) that `path_to` and the controller use. So the navmesh, the local planner and the controller agree on what can be walked, climbed, dropped from and swum. A node is a cell on one floor level (floors are different nodes). Links are walking, swimming, climbing, a drop to one of the eight neighbours, and jumps over two cells at the foot of a ledge.

```text
$ cargo sandbox --bake-nav               # all tiles that regions use (or --bake-nav grass,ruin; in the game: tool nav_bake)
baked 18 files for 2 body classes (agent, large) into nav/
  gate: gate.navtile.ron (1000 nodes, 3 areas, 4 portals)
  gate: large/gate.navtile.ron (968 nodes, 3 areas, 4 portals)
  grass: grass.navtile.ron (1024 nodes, 1 area, 4 portals)
  grass: large/grass.navtile.ron (1024 nodes, 1 area, 4 portals)
  hall: hall.navtile.ron (972 nodes, 2 areas, 4 portals)
  hall: large/hall.navtile.ron (916 nodes, 2 areas, 4 portals)
  pillars: pillars.navtile.ron (944 nodes, 5 areas, 4 portals)
  pillars: large/pillars.navtile.ron (848 nodes, 5 areas, 4 portals)
  …
```

The bake command loads the game without a window and without ticks. It writes the files `assets/nav/<tile>.navtile.ron` (rooms: `.navroom.ron`). For each body class except `agent`, it writes into that class's folder, `assets/nav/<class>/<tile>.navtile.ron`. It exits with code 0 (all baked) or 1 (something cannot be baked, or there is no such tile: ``no tile or room `grss` to bake (did you mean `grass`?)``). The file is a RON header with numbers a person can read, and a hash of what it was baked from, around the packed grid in base64. The same tile gives the same bytes: running `--bake-nav` again changes nothing in git. A game without body classes writes only the `agent` files and reports as before: `baked 9 tiles into nav/`.

The settings are the `nav` content kind (id `default`):

```ron
// content/region/wilds.nav.ron
{ "default": (cell: 0.5, agent: Some("skeleton"), safe_drop: 2.0, bodies: {"large": (locomotion: Some("warden"))}) }
```

`cell` — the edge of a cell in metres (the edge of a tile must divide evenly by it). `agent` — the `locomotion` id of the body the tiles are baked for (radius, height, step height, slope, climbing). `bodies` — more body classes (see below). `safe_drop` — a drop deeper than this counts as a cliff. `dir` — the folder of the files (`nav`). `check_bakes` — whether to check the files against the content. **With a `nav` definition, a missing or stale file is a content error.** Each class has its own file, and `shards content check` / `--check-content` name it:

```text
content/region/wilds.nav.ron: nav `default`: the bake of tile `grass` (nav/grass.navtile.ron) is stale: the tile, its prefabs or level file, or the nav settings changed since it was baked; bake it again with `--bake-nav grass`
content/region/wilds.nav.ron: nav `default`: tile `pillars` has no bake file for the body `large` (nav/large/pillars.navtile.ron); bake it with `--bake-nav pillars` (or `--bake-nav all`)
```

The file's hash covers the format and the bake version, the tile's definition, the prefabs of its geometry together with the prefabs they extend (including the prefabs of the nodes in the level file), the bytes of the level file, the tile size, and the settings with the body (for a class, its name too). A tile without a fresh file is still baked in place when a region loads, and the journal gets a warning with the names (for a class: `navmesh (body
large): …`):

```text
  0 [warn] navmesh: 5 tiles baked while loading (stale: grass, road_bend, road_end, road_straight, road_t); bake them with `--bake-nav all` (or the nav_bake tool)
```

**Bake version.** `BAKE_VERSION` in `shards_npc::nav::tile` is part of every file's hash. If you change what a tile is baked with (the sampling, the ground probe or the step rule in `shards_motion`), raise it: every file becomes stale, and the content names each one. The files live in the game's repository.

**Body classes.** Tiles are baked for the `agent` body and once more for each class in `bodies`, so a large enemy walks on its own navmesh, not on the local grid. A class is a name (lowercase letters, digits, `_`; it is also the name of its files' folder) and a body: `locomotion` (without it, the `agent` body), and `radius` and `height` replace its radius and height. The capsule made from them must still be one that `locomotion` allows: height not less than the diameter, radius more than two `skin_width`. Otherwise the content check names the class and what to fix (`bodies.wide: height 1.8 is less than the capsule's diameter 2.4; give the class a height of at least 2.4 m or a
smaller radius`). A region builds each class's navmesh the same way (files, cut-outs, stitching, navlinks), and the journal reports each one (`navmesh (body large): 64 tiles, …`). A route uses the tightest class that serves the whole body of the walker (`NavRegion::serving`): a class that is not narrower or lower than the body, and whose step, climb, slope and swimming depth the body can manage. Of those, the narrowest is chosen, then the lowest (among equals, the first one: `agent`, then by name):

- NPC routes (`NavPaths`, `navmesh_path`), the NPC's direct line to a target in sight (`plain_sight`), `move_to path=true`, `path_to`, `clearance`, and `nav_path from=<walker>` use the walker's own body (`CharacterParams`). An NPC taller than the class it fits by radius will not walk under a canopy that only that class's body fits under. A body that no class serves, and an NPC standing outside the region, plan on the local grid with their own body. `npc_state` (`nav.routes`) and `npc_overlay` say so;
- `nav_path` from a point (there is no body there) takes the body of the tightest class that is not narrower than `radius`;
- `nav_path body=large` walks the named class's navmesh with its body (a `radius` can be narrower still; wider than the class is an error).

The sandbox's guard (`locomotion` `warden`: radius 0.8 m, height 3.2 m) is wider than the skeleton, so it uses the `large` class. The sandbox bakes 9 tiles twice. The template has no classes, and its files are the same. **If you change a tile, its level file, the geometry prefab or the `locomotion` of any class, bake again** (`--bake-nav` bakes all classes at once). Otherwise the content names the stale class file, and a region bakes it when it loads. The format of a class file is the same, `BAKE_VERSION` does not change for classes, and the `agent` files are byte for byte the same as in a game without classes.

**A region builds its navmesh when it loads** (`NavRegion`, built inside the call that loads it, so no tick sees a region without one):

1. for each tile in the manifest, its file is taken (if fresh), or it is baked in place;
2. each placed tile is an instance of the bake, rotated together with the tile;
3. **Cut-out**: solid bodies that the bake did not see (decor, props, colliders that appeared later) switch off the nodes where a body no longer fits. A node is off if such a body stands there now. It is a pure function of the bodies standing now, so the navmesh does not depend on the order in which they appeared and left. Changes are noticed every tick, and the links at the edges are worked out again;
4. **Stitching** along each edge that the sockets left open (the region graph from `shards_world`): the edge node gets the same moves that the local planner would make, into the node of the neighbouring tile, including jumps over two cells (up or down) when a ledge lies right on the edge of the tile. An edge with an impassable socket is closed, whatever the geometry is;
5. **navlink**: `navlink` level markers with `to` become one-way links (a drop of any depth, a climb within the body's climbing height, a step into the neighbouring cell). Those the body cannot manage are not used, and `nav_info` and the journal say why (bodies have no jump);
6. **Tile graph**: portals (per side and area of each tile), the costs between them inside a tile, and the links between tiles.

A route (`NavRegion::path`) is: A* over the tile graph, then A* over the grid of the tiles along the corridor, then string pulling, checked on the grid and by probing the body with the physics. Costs are whole numbers, and a tie goes to the smaller id. So the same region gives the same route, and NPCs in a replay walk the same way. Routes do not need a window. A walker (the player, the agent body, an NPC) that cannot manage the movement of any body class (wider or taller, a lower step, climbing or swimming worse; `NavRegion::serves`) plans on the local grid with its own body. So neither `move_to path=true` nor an NPC route leads it over a climb it cannot make, or under a canopy it cannot pass. `nav_path from=<it>` plans the same way, with its own body, not with the class's body. The sandbox's guard in a region walks on the `large` class's navmesh. In the arena (outside a region), it uses the local grid. A navmesh that could not be built is not tried again every tick, only when the region or the content changes.

**Queries and tools.**

| tool | what it does |
|---|---|
| `nav_path` (player, query) | the route an NPC would pick, in the `path_to` format plus the body class (`"body"`) and the tiles it goes through (`"provider": "navmesh"`); `body=large` uses that class's navmesh |
| `nav_info` (developer, query) | the region's navmesh: tiles, nodes, links, cut-outs, navlinks, and which tiles were baked while loading; `bodies`: each body class, its body and its navmesh |
| `nav_bake` (developer, `control`) | the same as `--bake-nav`: one tile, several, or all of them, for all body classes |
| `nav_overlay` (developer, window) | draws the cells (green; water blue, cut-outs red), the links across the edges, navlinks, NPC routes and tile borders |
| `npc_overlay` (developer, query; window — F7) | what each NPC near the players has on its mind: as a line in the reply and as labels in the window (see "NPC overlay") |
| `npc_state` (player, query) | everything about an NPC's mind: state, target, token, plan, route, leash |
| `npc_set_state` (developer) | forces an NPC into a state of its behaviour (in the journal as `(npc_set_state)`) |
| `move_to path=true`, `path_to` | a navmesh route in a region, and the local grid outside one; `path_to` names it as its `provider` |
| `clearance` | in a region, the direction where the navmesh ends, `closed` (see "Movement (E2)") |

```text
$ shards act nav_path from='{"x": 24, "z": 140}' to=e5          # from the guard's lair to the player behind the columns
{
  "body": "agent",
  "distance": 16.8,
  "path": "path 16.8m, 3 turns: 6.2m E → 3.5m N → 7.2m NW",
  "provider": "navmesh",
  "tiles": ["(5, 5)"],
  …
}
$ shards act nav_path from='{"x": 24, "z": 150}' to=e5 body=large   # the same walk around the colonnade, with the guard's body
{
  "body": "large",
  "distance": 24.76,
  "path": "path 24.8m, 3 turns: 14.2m NE → 3.0m N → 7.6m NW",
  "provider": "navmesh",
  …
}
$ shards act nav_path from='{"x": 24, "z": 150}' to=e5 body=larg
error: no body class `larg` in the nav settings (did you mean `large`?); classes: agent, large
$ shards act nav_info
{ "agent": {"height": 1.8, "radius": 0.4}, "baked_while_loading": [],
  "bodies": [
    {"baked_while_loading": [], "body": "agent", "failed": [], "height": 1.8, "radius": 0.4, "step_height": 0.35,
     "summary": "64 tiles, 65012 nodes, 7112 links across edges, 0 navlinks"},
    {"baked_while_loading": [], "body": "large", "failed": [], "height": 3.2, "radius": 0.8, "step_height": 0.4,
     "summary": "64 tiles, 64180 nodes, 7056 links across edges, 0 navlinks"}
  ],
  "from_files": ["gate", "grass", "hall", …], "region": "wilds seed 42 v1",
  "summary": "64 tiles, 65012 nodes, 7112 links across edges, 0 navlinks", … }
```

Movement tools ask the navmesh through the movement route planner (`RoutePlanner`, which `shards_npc` installs):
`move_to path=true` and `path_to` get a route from it (in a region, and for a body the navmesh serves; otherwise, a local
grid of 0.5 m cells), and `clearance` is how far the navmesh reaches in each direction (`reach`). Both use the tightest body class that serves the walker's body. A snapshot does not save the navmesh: after a snapshot loads, it is built again (for each class), for the region and the bodies that the snapshot brought back.

### Navigation agents

An NPC gets its route from `NavPaths` (the override: `fn(&World, from, to, &CharacterParams) -> waypoints`; the body is the NPC's own numbers: radius, height, step, climbing, slope, swimming). By default, inside a loaded region this is the navmesh of the class that serves the body, and elsewhere it is the local grid with the same body.

A target in sight (`plain_sight`: on the navmesh of the same class, a straight line of steps over the ground, never through a drop, a ledge, a cut-out or under a canopy lower than the body; outside the navmesh, a sweep of the body's capsule from the step height up to the top of the head, along the `WORLD` layer above the ground, with no holes or deep drops) needs no route, until the NPC has moved off the checked line by more than its radius: it was pushed, knocked back, or it went around another body, and the straight line from it to the target runs over ground that `plain_sight` did not see. Then the plan is built again from where it stands (`NavAgent.planned_from`), and again only after another radius to the side. An NPC that walks its own straight line makes no extra requests.

For another target, the route is built again when the target has moved, or every `repath` seconds, so there are few requests however many NPCs are running. With no route, the NPC stands still until the next plan, and never walks straight at its target (`npc_state` → `nav.no_route`). The numbers (`hit_noise`, `sprint_noise`, `avoidance`, `repath`, `waypoint_reach`) are in the `npc_tuning` content kind, id `default`.

#### Avoiding bodies

The route knows the walls, but not the other characters. So after the route, in `NpcSystems::Navigate`, comes only the speed that the route asks for (`NavAgent.preferred`, with the leash). Then in `NpcSystems::Drive`, before the write to `Intent`, each NPC on the way picks its speed by looking at the bodies nearby (selective reciprocal velocity obstacles, "RVO over fans"):

- neighbours are character bodies (NPCs, players, agent bodies) within `range` metres of its edge, at the same height as it; the nearest `neighbours` of them by gap, ties broken by `AiId`; the dead do not count;
- the candidates are the route's speed, a fan of directions to its right and left (20°–130°) at three speeds, and "stand still". A candidate's cost is its deviation from the route plus the threat: `weight × (horizon / t − 1)`, where `t` is the time until contact with the neighbour it would touch first (contact is the sum of the radii and `margin`). Up close, a move that still closes the gap costs the most. A head-on meeting splits to the right (left turns cost a little more), and standing still costs more than any step to the side. Once an NPC starts going round on one side, it keeps to that side (`NavAgent.side`; turning to the other costs more), so it does not dither between two narrow gaps;
- an NPC that is blocked by a body that does not give way (a standing NPC, the player) stops within a radius (plus `margin`) of its stopping point: the pack around the player waits behind the ones who came first, instead of pushing between them. Two NPCs that dodge each other do not stop like this; they part;
- other NPCs on the way do the same, so each takes half of the manoeuvre (the speed against them is computed as `2v − its −
  their`). An NPC goes fully round a standing body or the player. The NPC's own target (`target`) does not need the `margin` gap, so the NPC walks up to the player to striking distance and does not stop `margin` short of it. The horizon is never longer than the distance the NPC has left to its stopping point.

The order follows `AiId`, with the bodies' positions taken at the start of the system. There is no randomness and no `HashMap` order. The fan's trigonometry is a table of constants. The leash still holds after an avoidance. With no neighbours or threat, the route's speed goes into `Intent` unchanged (bit for bit as without avoidance). The journal says nothing about it. `npc_state` (`nav.avoiding: "e7"`) and `npc_overlay` (`avoiding e7`) show which body an NPC is going round now. The condition `step --until named:npc_avoiding` catches the first tick of the manoeuvre.

The numbers are `avoidance` in `npc_tuning` (for all NPCs), or in a behaviour (`*.behavior.ron`, which replaces it entirely):

```ron
// content/npc/tuning.npc_tuning.ron
{ "default": (avoidance: (weight: 1.0, horizon: 1.5, range: 3.0, neighbours: 6, margin: 0.2)) }
// content/npc/golem.behavior.ron: this one barges straight through
{ "golem": (initial: "walk", avoidance: (weight: 0.0), states: { … }) }
```

| Field | Default | What it means |
| --- | --- | --- |
| `weight` | `1.0` | the weight of the threat against the deviation from the route; `0` turns avoidance off, and the NPC walks as before |
| `horizon` | `1.5` | how many seconds ahead to look (0–10] |
| `range` | `3.0` | neighbours farther than this gap (m) do not count |
| `neighbours` | `6` | how many of the nearest neighbours to count (1–32) |
| `margin` | `0.2` | the allowance added to the sum of the radii (m); not needed for the NPC's own target |

The content check names the field and its bounds (`avoidance.horizon must be in (0, 10] seconds (is 0)`). Physics still pushes apart capsules that have met (`shards_motion`): avoidance keeps them from meeting, it does not replace the physics.

### NPCs as the AI sees them

In `observe`, each NPC has a state line (`chase e5 · token · leash 12/20m`, `attack e5 · circle`, `idle (dormant)`). The journal shows the transitions and the tokens:

```text
  2 e25 idle → fight e5
  2 e25 takes an attack token on e5 (1/2)
  183 e25 returns its attack token unused: unused (0/2)
  332 e25 -> e5 claw -0 (hp 100, god mode)
```

The full `npc_state` (excerpt): `"state": "fight"`, `"target": "e5"`, `"token": {"attack": "light", "started": true, …}`, `"plan":
{"history": ["light"], "rest": 0.0, …}`, `"nav": {"arrived": true, "direct": true, "waypoints": 0, "no_route": false}`, `"home":
{"at": "(24.0, 140.0)", "distance": 8.51, "leash": 10.0}`.

While an NPC goes round another body, `nav.avoiding` names it, and `npc_overlay` adds it to the NPC's line. In the sandbox arena, three skeletons run single file at the player (scenario `npc_avoidance`), and `step 300 --until named:npc_avoiding` stopped at the third tick:

```text
$ shards act npc_overlay
e7 skeleton: fight e5 · token · leash 0/30m · sees e5 8.0m S
e8 skeleton: fight e5 · token · leash 0/30m · sees e5 9.2m S · avoiding e7
e14 skeleton: fight e5 · circle · leash 0/30m · sees e5 10.4m S · avoiding e8
$ shards act npc_state id=e8      # excerpt
  "nav": {"arrived": false, "avoiding": "e7", "direct": true, "goal": "(0.0, 12.0)", "next": "(0.0, 12.0)",
          "no_route": false, "routes": "local grid: no region loaded", "waypoints": 1},
```

`nav.routes` says what the NPC plans its routes on: `navmesh, body agent` (or another body class), and outside the navmesh —
`local grid` and why: `no region loaded`, `the region has no navmesh`, `the NPC stands outside the region`, `no body class
fits a 1.60 m radius (the widest: large, 1.20 m)`, `no body class serves the body: agent has height 1.80 m, the body 2.40 m`
(for each class, the first number that does not fit the body: the `locomotion` field, the class value and the body value); with the game's own
routes (a `NavPaths` under another name), the name of its provider. Without the NPC's position (`routes_for`, by body), the class that
serves the body. The Warden in the pillar hall of wilds
(scenario `npc_bodies`) walks to the player around the colonnade:

```text
$ shards act npc_overlay
e6 warden: fight e5 · token · leash 1/26m · sees e5 19.2m N · detour, 3 waypoints left · body large
e25 skeleton: fight e5 · token · leash 6/10m · sees e5 9.3m NW · detour, 2 waypoints left
$ shards act npc_state id=e6      # excerpt
  "nav": {"arrived": false, "avoiding": null, "direct": false, "goal": "(24.0, 130.0)", "next": "(30.2, 137.2)",
          "no_route": false, "routes": "navmesh, body large", "waypoints": 3},
```

### NPC overlay

`npc_overlay` (for developers; in the window — F7, `NpcOverlay::key`) shows what each NPC closer than 40 m to the player is thinking, in id
order. The tool's answer is one line per NPC: state and target, token or waiting role, leash, who it sees and hears, where it
lost the target, noise, route (and the body class whose navmesh it walks on, if this is not `agent`: `body large`; `local grid`, if
in a region with a navmesh no class serves its body or it stands outside the region; the same as `nav.routes` in `npc_state`; with the game's own
routes, neither is shown):

```text
$ shards act npc_overlay
npc overlay on
e25 skeleton: fight e5 · token · leash 0/10m · sees e5 10.0m N · detour, 3 waypoints left
e26 skeleton: idle · leash 2/10m · lost e5 4.0s ago at (31.0, 120.5)
```

The words of the duel rules appear only on NPCs that use them: `waits 31t for e7` (held back by the director's strike spacing, with the ticks left), `think 0.07s` (the decision timer), `chain light` (a combo continuation rolled, `(pressed)` once it is) and `keeps e5 17.2m N` (a target seen beyond the sight range, kept by `keep_range`). `npc_state` has the same as fields (`strike`, `spacing`, `think`, `chain`, `plan.options`, `perception`, `kept`).

In the window, the same things are drawn as gizmos: a line above the NPC, a line to the target (red while it holds an attack token), the view cone and the proximity circle (yellow while it sees someone), crosses where it last knew the target, the last noise, and the leash circle around home (red when the NPC is beyond it). `npc_overlay on=false` turns it off. The overlay does not change anything in the world.

### Limitations

- The navmesh is baked for the `agent` body and for the `bodies` classes. A route follows the class that serves the walker's whole body. A body that no class serves (wider or taller than all of them, weaker at steps, climbing or slopes, or floating at another depth) plans on a local grid (this is slower, with a limit on the search area): a tall or weak NPC needs its own class in `bodies`. Each class is one more baking of all tiles and one more region navmesh in memory and on load. `nav_overlay` draws only the `agent` navmesh.
- Bodies cannot jump or use ladders: a `navlink` works for going down, and for climbing up within the climb and step heights.
- Geometry that appears after the bake is cut out, not baked in: cutting can only switch cells off. The navmesh does not know about a step that was not in the bake until the tile is baked again.
- Changing a tile, a level file, the prefab of its geometry, the `agent` body, the body of any class or the `nav` settings needs `--bake-nav` (it bakes all classes). Without it, the content names the stale file of each class, and the game bakes it when the region loads.
- Avoidance between NPCs is local (a fan of speeds over the `horizon`), with no coordinated planning: a pack that arrives single file lines up in a second row behind the first ones. It sees bodies, not walls or drops: a sidestep at an edge can carry the NPC down by itself (the direct line to the target is checked again after the step, the step itself is not). On a narrow ledge, set `weight: 0` on the behaviour.

## The AI layer

The `ai_core` crate: everything the agent uses to look at the game and act in it. It does not depend on any particular game.

### What the AI sees

```text
tick 207 (3.45s) | paused | speed 1x | seed 42 | you e5
YOU  e5 Player  hp 100/100  stam 100/100  poise 60/60  flask 3/3  at (0.0, 12.0) facing N
ALERT in hit zone of e6 shockwave (lands in 0.50s): move 1.3m E to leave it
SEEN (by threat)
  e6 Boss "Frost Warden"  hp 600/600  poise 140/140  windup shockwave 0.50s (zone line 9m×1.6m)  phase 1  7.4m N
  e7 Skeleton  ground 3.6m/s  hp 40/40  poise 20/20  4.5m NW
  e8 Skeleton  hp 40/40  poise 20/20  18.4m NE
LANDMARKS  e3 Pillar 8.6m NW · e4 Pillar 8.6m NE · e1 Pillar 20.2m N · e2 Pillar 20.2m N
SINCE seq 2
  0 [agent] dev: step {"until":[{"journal":"winds"}]}
  207 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
```

- Visibility is computed only from the subject ("YOU"): a ray from its eyes (`pos + EYE_HEIGHT`) to a target within `AiPerception::radius` (40 m). The ray is checked against physics (the `WORLD` layer, colliders) and against `AiOccluder`; if an entity is behind a column, it gets `occluded`. The window camera does not affect this. The output is the same with a window and without, even while the following camera catches up with the player after a large `step`. After `possess`, the agent sees through its body's eyes.
- NOT IN VIEW is an optional view cone: `AiPerception { fov_deg: Some(120.0), .. }`. Everything outside the cone around the subject's `facing` goes there. By default (`None`) the view is all round, and this section is absent.
- `scr(x,y)` is a separate note: if the window has an `AiCamera` and the entity is in its frame, the line gets the place on screen added, for example `e8 Skeleton  hp 40/40  6.3m NE  occluded  scr(905,342)`. It does not affect SEEN, `occluded` and NOT IN VIEW. The `shot` legend is still computed from the camera: it describes the picture.
- SEEN is sorted by threat: `AiImportance` in descending order, then by distance.
- ALERT holds the module hints for the subject (`register_ai_advice`): at the moment, "you are in the hit zone, move N m out".
- SINCE shows only new journal entries. Each agent has its own cursor.
- Compass: N = −Z, E = +X.

How the game describes itself to the AI:

```rust
app.register_ai_component::<Health>(|h| format!("hp {:.0}/{:.0}", h.current, h.max))
   .register_ai_describer("orders", |e| /* a fragment from several components */)
   .journal_event::<Damaged>(|e, w| Some(format!("{} -> {} -{:.0}", label(w, e.source), label(w, e.target), e.amount)))
   .add_ai_tool("move_to", Scope::Player, move_to)   // action: journal and list of changes
   .add_ai_query("threats", Scope::Player, threats)  // read: no journal
   .add_ai_condition("boss_phase_2", |w| /* ... */)
   .add_ai_status("my_field", |w| /* serde_json::Value in ai/status */)
   .add_ai_invariant("health_in_range", |w| /* Some(text) if violated */)  // see "Invariants (E11)"
   .replay_input::<Intent>()                        // input for replays (see "Replays")
   .replay_ignore::<Sparkle>()                      // cosmetics: not part of the replay checks
   .add_replay_check("weather", |w| /* u64 */)      // data outside AiVisible entities
   .add_playback_hook(|w| /* request what the replay will need */);  // see "Replay from a snapshot"
```

Entity markers: `AiKind("Boss")`, `AiImportance(10.0)`, `AiLandmark`, `AiOccluder`, `AiPlayer`.

### Tools

A tool is an argument struct (serde + schemars) and an ordinary Bevy system, `In<Call<Args>> -> ToolResult`. The schema and the signature are generated automatically (`shards tools`; `--json` gives the schemas; the description is taken from the doc comments).

| scope | tools |
|---|---|
| **Player** (plays fair, only through `Intent`) | observe, inspect, nearby, line_of_sight, journal, tools, highlight, overlay, content_list, content_get; movement: move_to (`path=true`), move, stop, jump, sprint, glide, clearance, path_to, input_bindings; combat: light_attack, heavy_attack, block, parry, dodge, use_item, use_technique, lock_on, attack; world: region_info, level_info, nav_path, npc_state; settings: settings, settings_set (like the settings menu for a human); sound: audio; Steam (feature `steam`): steam, steam_invite |
| **Dev** (everything, including time) | + pause, resume, speed, step, possess, teleport, despawn, get, set (reflection), spawn_prefab, heal, god, stagger, content_check, content_reload, content_apply, rebind, physics_overlay, combat_overlay, shot, replay_save, snapshot_save, snapshot_load, crash_report, quit, assert_add, assert_list, assert_remove, assert_reset, motion_bake; world: regen, region_load, goto_tile, worldgen_report, world_overlay, spawn_level, level_reload, npc_set_state, nav_bake, nav_info, nav_overlay, npc_overlay; sound: sound_play, music_set; mesh looks: looks, look_outlines; foliage: scatter, scatter_set; effects: effects, effect_play, telegraph_decals; Steam (feature `steam`): steam_host, steam_join, achievement_unlock; developer menu and console: dev_menu, console |

The game adds its own (in the template, `taunt` and the conditions `arena_cleared`, `foe_near`, `in_fight`; in the sandbox, the conditions `boss_phase_2`, `boss_dead`, `player_dead`, `three_attacking`). Modules add conditions for `step --until named:…`: `near_poi` (world, appears with the first region), `npc_attacking`, `npcs_calm`, `npc_beyond_leash`, `npc_detour`, `npc_avoiding`, `npc_engaged`, `npc_near` (NPC). Each
tool has a kind (`kind` in `shards tools`):

- `query` only reads the world: no journal entry and no diff;
- `action` changes the world: it writes to the journal, returns `CHANGES` and goes into the replay;
- `control` manages time or the session (pause, resume, speed, step, possess, content_reload, rebind, settings_set,
  snapshot_save, snapshot_load, nav_bake, motion_bake, sound_play, music_set): journal and diff as for action, but it does not go into the replay. It changes *when* something happens, not *what*.
  The exception is rules (`assert_add`, `assert_remove`, `assert_reset`): they do not change the world, but they decide where the game stops, so the replay records them as separate events.

- **An action returns its changes.** Each action tool compares a slice of the world before and after the call, and returns `CHANGES` and `JOURNAL`. The agent does not need a second `observe` to learn what happened.
- **Errors say how to fix the call:**
  ``bad arguments for teleport: missing field `x`. Expected: teleport(id: integer|string, x: number, y?: number, z: number)``,
  `e57 no longer exists (Skeleton despawned at tick 812); live: …`,
  `e7 no longer exists (Skeleton renumbered to e99 at tick 40)`,
  ``unknown component `Mana`; this entity has: ActionState, Body, CharacterController, …``,
  `AiId is managed by the engine; despawn and spawn instead`,
  ``unknown tool `fly`; available: attack, block, clearance, …``.
- **Combat tools explain a refusal.** A press waits in the input buffer until the action becomes possible, and `Intent` stays a clean input. The tool checks at once the same rule as the action machine (`can_start`): if it can run now, `ok: roll
  (W)`; if it becomes possible within the buffer, `ok: queued, heavy starts in 0.10s`; otherwise an error with the reason and the time, and `Intent` does not change: `not enough stamina: 10/20 (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a
  cancel window opens)`. The content decides what cancels what. An input that was written outside the tools (the keyboard) and whose time ran out goes into the journal: `e5 dodge rejected: stamina 7/20`.
- **A panic in a tool does not crash the game.** The agent gets an error, the entry goes into the journal, and the tool is registered again on the next call. The tick schedules stay in place; if a panic did take them away (the game turned off the catch, `catch_system_panics: false`), the answer says so: `the game lost its FixedMain, FixedUpdate schedules`. A command that the tool queued and that then failed is recorded as a tool error (`tool orphan: command … failed`): this is not a gameplay failure, and the game does not pause.

### Time and determinism

- All gameplay runs in `FixedUpdate`, at 60 Hz by default, in a fixed order `SimSet::Input → Think → Act → Move →
  Resolve → Cleanup` (inside it are the module sets, see "Tick order").
- `SimRng` is a counter-based RNG keyed by (seed, tick, AiId, salt). The result does not depend on the order of the systems, or on how many times anyone has "spun" the generator.
- `step` runs `FixedMain` for exactly N ticks, and the game stays paused. There are stop conditions: `journal:TEXT`,
  `gone:ID`, `shows:ID:TEXT`, `near:A:B:M`, `field:ID:Comp.path<N`, `named:NAME`, `warning`, `error` (the first journal entry of this level, including the game's `warn!`/`error!`). Limits: up to 216 000 ticks and 20 s of real time per call.
- **Logs inside `step`.** `warn!`/`error!` enter the journal with the tick they were written on, right after that tick, not at the end of the step. So `step --until journal:TEXT` stops on the tick of the warning. No more than 64 lines per tick go into the journal; the rest are merged into one: `70 [warn] 36 log lines dropped (limit 64 per tick)`. Lines of Bevy's IO and async compute threads (asset loading) have their own 64, and their own `… log lines of the asset threads dropped` line. A warning that an action caused comes in that action's own `JOURNAL`.
- **A gameplay failure stops the game, it does not kill it.** In Bevy 0.20 a system's panic goes to `FallbackErrorHandler`, and that by default panics again. This panic takes `FixedMain` and `FixedUpdate` out of the world: every next `step` would fail with `cannot run a tick here`, and in real time the ticks would silently stop. `AiCorePlugin` installs its own handler (`catch_system_panics`, default `true`). It writes the error to the log and to the failure queue, and the queue is processed after every tick:
  - `step` stops on the tick of the failure and answers `stopped: gameplay system <name> panicked at tick 50: <msg>; crash
    replay: replays/crash-tick50.json`, and the journal gets `system <name> panicked at tick 50: <msg>`;
  - in real time the game pauses (`[sim] paused: system <name> panicked at tick 50`), and the following ticks of this frame do not run;
  - the session is saved to `replays/crash-tick<N>.json` (one file per failing system). To reproduce the failure: `cargo sandbox --replay replays/crash-tick50.json` (or `--headless --paused` and `shards step`), and the game fails on the same tick. `--verify` exits with code 1 while the system keeps failing;
  - alongside it, a crash report `crashes/crash-<time>-tick<N>.json` is written: the place and stack of the panic, the game, the machine, the journal, the log and the same
    replay (see "Crash reports (E10.7)");
  - the next `step` carries on: the schedules are in place.
- A violated invariant stops the game in the same way: `stopped: invariant <name> violated at tick N` (see "Invariants (E11)").
- `AiCorePlugin` turns on ambiguity detection for `FixedUpdate`: two systems that touch the same data without an explicit order give a warning, and it goes into the journal. The test `gameplay_systems_have_a_total_order` turns this into an error, and a control test checks that the check itself catches such a system.
- The tests confirm that one seed and the same calls give a bit-for-bit identical world and journal; `step 300` gives the same result as 5 × `step
  60` with `observe` and render frames between them.
- A replay is the same with and without a window: `--verify --windowed` plays it in a window (rendering, camera, input map and the game's visuals work) and gives the same report as `--verify` without a window. Render entities do not shift `AiId`, `ColliderId` or the gameplay order.
- **The same bits on every OS of one CPU architecture (E10.6): on x86_64 this means Windows, Linux and the Steam Deck.**
  `sin`, `cos`, `atan2`, `exp`, `powf` and the other transcendental functions from std call the platform's math library, and the last bits of an `f32` differ between MSVC, glibc and macOS: a replay recorded on one OS diverged on another, and in co-op a client on Windows and a host on Linux (or Steam Deck, Proton) would run into a desync. So transcendental maths goes through the pure-Rust `libm` library, one on all OSes, everywhere the simulation computes it: in Bevy, glam, simba (the maths of parry) and in the engine's code:
  - In `[workspace.dependencies]`, Bevy has the feature `libm` enabled (it enables it for `bevy_math`, which enables it for glam: `Quat`,
    `Vec2::to_angle`, `Quat::from_axis_angle`, curves, colours). `parry3d` has the feature `indexmap` (the maps that parry iterates over — `IndexMap` in insertion order), and the direct dependency `glamx` (`=0.3.1`, only in `shards_motion`, the code does not call it) is needed for one feature, `libm`: with it, glamx and `simba` (`libm_force`) take their maths from `libm`. This is everything that `enhanced-determinism` does in parry3d, apart from `scalar-math` in glam (see below). The template gets this through `shards` and does not write it in its own `Cargo.toml`;
  - glam's SIMD stays enabled: its SSE2 code on Windows and Linux x86_64 is the same instructions, and simple operations (`+`, `*`, `/`, `sqrt`) round the same way under IEEE 754 on any OS; only the transcendental functions differ between the C runtimes, and those come from `libm`. A caveat: a build with `-C target-cpu=native` or `-C target-feature=+fma` changes glam's code and gives other bits: under `target_feature = "fma"` its SSE2 code fuses a multiply and an add into one instruction with one rounding (`_mm_fmadd_ps`) in the product of `Quat`, `Quat::slerp`, `Vec3A::cross`, `Vec3A::slerp`, `lerp_monotonic` and
    `mul_add` of `Vec3A` and `Vec4`; `cargo build` and `shards package` do not build it that way;
  - The engine does not build with glam without SIMD (`scalar-math`). The `enhanced-determinism` feature of parry3d turns it on, and a feature that one crate asks for is enabled in the whole build. In scalar glam, `Vec4` and `Mat4` lose their 16-byte alignment, and Bevy 0.20 computes the size of the mesh buffers for the GPU from the Rust layout (`size_of::<MeshUniform>()`): the buffer stops matching the shader's struct, 168 bytes against 176, wgpu answers `Binding size 168 of Buffer with '' label is less
    than minimum 176`, and rendering stops (this is how the GPU test of `shards_fx` failed; when the buffer holds more than one mesh, wgpu stays silent, but the buffer is still shorter than the shader expects). A build without a window is not affected by this, but in a replay and in co-op every build of the game must compute the same, so `crates/shards/src/lib.rs` checks at compile time that `align_of::<Vec4>() == 16` and
    `align_of::<Mat4>() == 16`, and fails with an explanation, including for a game that turns on `scalar-math` through its own dependency;
  - Engine and game code call `bevy::math::ops` (it is called `ops` in the Bevy prelude and in `shards::prelude`): `ops::sin(x)`,
    `ops::atan2(y, x)`, `ops::exp(x)`, `ops::powf(x, y)`, `ops::sin_cos(x)`; instead of `x.powi(2)`, use `FloatPow::squared`
    (`bevy::math::FloatPow`). Exact operations (`sqrt`, `abs`, `floor`, `round`, `mul_add`, `rem_euclid`, `to_radians`) stay
    methods. `ops` exists only for `f32`: `f64` is cast to `f32`, and a large value is first reduced to a small one while it is still
    `f64` (`f32` has only 24 bits and a limit of 3.4e38): this is how the `highlight` in `ai_core` takes its phase modulo `TAU`. The rule is the same for all Rust code, including what only draws (particles and decals of `shards_fx`, foliage of `shards_scatter`, the HUD), so it does not have to be split into "simulation" and "picture". WESL shaders are not Rust, so the rule does not cover them;
  - `clippy.toml` enforces the rule (at the root of the repository and in `template/`, as separate files: clippy takes the nearest one and does not merge them): `disallowed-methods` names `f32::sin`, `f32::exp`, `f32::powf`, `f32::powi` and the other std methods (and their `f64` versions), with a hint of which `ops` call to use; `cargo clippy --workspace --all-targets -- -D warnings` fails on a direct call, including in tests and examples;
  - the test `ai_core::determinism` pins several results of `ops` bit for bit (`f32::to_bits`): `libm` is written in Rust, so they are the same on every OS, and platform maths that comes back when the features change breaks the test, not the replays.
  **Checked across OSes by a run (E10.6): Windows and Linux on x86_64 write the same bytes.** Nine reference replays (seven from the sandbox: `obstacle_course`, `combat_prototype`, `npc_director`, `npc_navmesh`, `region`, `snapshot`, `saves`; two from the template: `smoke`, `valley`) were recorded on each OS (`record_reference_replays`), and the files matched byte for byte. Then each OS checked the other's recordings (`every_reference_replay_verifies_headless` in the engine, `reference_replays_still_match` in the template): there are no divergences. Both OSes ran from commit `e635768` (the recording on Linux was made on `5d1dbfe`: the simulation code is the same, and the commits between them change only the highlight, which only draws, and the documents). Windows is Windows 11 (`x86_64-pc-windows-msvc`), Linux is Ubuntu 24.04 (glibc 2.39) under WSL1 (`x86_64-unknown-linux-gnu`); Rust 1.99.0 on both sides. WSL1 runs real ELF binaries on the same machine, there is no virtual machine, and **the CPU is the same on both sides**: the run checks the difference of OS, C libraries and builds, not different x86_64 CPUs. **Not checked:** other x86_64 CPUs, macOS, ARM, Steam Deck hardware. So **the reference replays can be recorded on both Windows and Linux x86_64** (`record_reference_replays`, `record-replays.yml`). Replays recorded before E10.6 (with glibc's maths) diverge on any OS until they are recorded again.
  **Across architectures nothing is promised** (x86_64 ↔ ARM, for example a Mac on Apple Silicon): there glam has different SIMD code (NEON), which fuses a multiply and an add (`vfmaq_f32`) in the product of `Quat`, `Vec3A::cross` and `lerp_monotonic`, where SSE2 rounds twice, so the bits may diverge. Enabling `enhanced-determinism` in parry3d would fix this (glam without SIMD on any architecture), but its `scalar-math` is incompatible with Bevy 0.20 rendering (see above), so determinism between architectures waits for another solution. Not covered: dependencies outside the simulation (sound, UI layout, rendering) that may call the platform's maths themselves: they do not affect the game state; the Bevy crates in the simulation (`bevy_transform`, `bevy_time`, `bevy_input`, `bevy_ecs`, `bevy_math`) make no direct calls.
- CI checks it across OSes every week: `cross.yml` verifies every reference replay on Windows and on Ubuntu 22.04 (another glibc), and prints the CPU model of the runner next to each verdict in the job summary, so hosted runners with other x86_64 processors check those too. macOS and ARM are still not checked.

### Groundwork for networking (phase C)

Footholds for `shards_net` and saves. In single-player they change nothing: replays and their hashes are the same as before.

- **Several games in one process** (`ai_core::process`). Each app has its own `AppToken`. `process::activate`
  makes it the active one (`First`, `call_tool`, `sim::step`, `sim::run_tick` and `failures::collect` do this). Log lines
  go to the journal of the active game (`journal::register_capture` for a game without its own `LogPlugin`). A system
  panic goes to that game's failure queue. Games in one process update in turn, not in parallel.
- **Replication rules** (`ai_core::net_rules`): what is replicated is what the replay hashes (`state_hash`), minus
  `net_ignore::<C>()`. Resources are the snapshot ones, minus `net_ignore_resource::<R>()`. `net_predicted::<C>()` is what the client
  predicts for its own body. `replicated_components(world, e)` and `rules_hash(world)` (for checking the protocol).
  The engine excludes `AiPlayer`, `LocalPlayer`, `Predicted` and `ColliderIds`.
- **State codec** (`ai_core::state`), shared with snapshots: `encode_entity`, `encode_replicated`,
  `encode_resources`, `apply_changes(world, &StateChanges)` (missing ids are created in order, values are written
  twice, removals, despawn with a tombstone: `e7 is out of view since tick 180`).
- **Snapshots and ids:** `sim::set_tick`, `snapshot::restore_releasing(world, &snap, gate)` (loads while holding
  `SimGate`; a refusal returns the world, as `restore` does), `AiIndex::set_local_base(base)` (the client's own ids come from a second counter; the host's ids do not shift them).
- **Replay and journal:** `replay::set_recording(world, false)` (the client does not write a replay; `replay_save` says why),
  `replay::restart_from_current`, `journal::set_local_events(world, false)` (the host writes the events),
  `journal::push_remote(world, host_tick, kind, text)` pushes a host line that `journal`, `observe` and
  `--until journal:` all see.
- **Tool routes:** `ToolSpec.net` is `Local`, `Host` (the default for Dev actions) or `HostOnly`
  (`snapshot_save`, `snapshot_load`, `replay_save`; on a UDP client, `pause`, `resume` and `speed` too). With
  `AiTools::set_forwarder`, `Host` calls go to the host, and `HostOnly` calls are refused with a reason. Without a forwarder, everything works as
  before. `app.net_route(name, route)` changes a route, and `AiTools::set_route` does it after the app is built.
- **An agent bound to a player:** `ai/session {player: N}` and `possess player=N`. The body is looked up again on each call,
  so it survives a respawn. With scope `player` this is a player agent (E11.6). On a stand client (`@c1 session --agent bot --scope
  player --player 1`), its `YOU` is the body of player 1, and its tools act through that body's `Intent`, the way a human's keys do.
  The host runs them as calls of that player. The agent is refused any developer tool (`` `teleport` needs dev scope;
  agent `bot` has player scope ``). The scenario is `crates/sandbox/scenarios/stand/agent_player.scenario`.
- **Deferred step:** with `StepRoute::Deferred`, the tools `step`, `pause`, `resume` and `speed` put a request in the queue and
  reply `{"pending": id}`. The stand driver runs it between frames, and `step_poll {id}` (BRP `ai/step_poll`)
  returns the report exactly as a normal `step` does. The driver runs ticks through `sim::step`, so a deferred step stops on
  a failing system and a broken invariant just like a normal one, and its report names the stop (`halted`). The finished
  request is returned whole by the `step_poll` tool (changes, journal, `halted`), so `shards act step_poll id=N` also exits
  with code 3 if the step stopped on a failure or a violation. Requests from one participant (`step` and `pause`/`resume`/`speed`) wait in
  one queue and run in the order of the calls: `step`, then `resume` in the same frame, leave the stand running
  (before, the driver ran the clock ahead of the steps and left it paused). Requests from different participants have no order between them.
- **Rules on a client:** invariants (`InvariantSystems`) run only where the simulation runs (`shards_core::simulates`).
  The client's world follows the host, and the host checks the game's rules. All four `assert_*` tools are `NetRoute::HostOnly`.
  A client with a forwarder refuses them and says the rules are checked on the host (`the rules are checked on the host: call it
  there`). They are not forwarded to the host: the host limits a guest's calls to the player's scope (`guest_scope`), and these are Dev tools,
  so the host would refuse them, yet the client's call would answer `ok`. `assert_list` is also a query, and queries are not sent to the host.

### Co-op session (E7, `shards_net`)

The host runs the world, clients send input and receive the state. One code path serves single player, the host and the client: a launch flag
sets the role. In single player, `NetPlugin` does nothing.

```bash
cargo sandbox --headless --paused --host 0 --port 0 --port-file /tmp/h.port &   # host on a free UDP port
SHARDS_PORT_FILE=/tmp/h.port cargo shards wait     # ready: … · role host · … · udp 0.0.0.0:53122
cargo sandbox --headless --join 127.0.0.1:53122 --name ann --port 0 --port-file /tmp/c.port &
cargo sandbox --headless --paused --stand 2 --net-sim latency=150,loss=0.02 --port 0 --port-file /tmp/s.port &
```

- **Knock and cookie** (protocol 2). Before the host remembers anything about a foreign address, the client knocks. A knock datagram
  (`wire::knock`, 64 bytes: the session the client chose) gets back a challenge with a cookie (`wire::challenge`, 20 bytes, no
  bigger than the knock). The cookie is a keyed MAC (SipHash-2-4, a random 128-bit key of the host, `shards_net::cookie`) over the sender's address,
  the session, the channel's protocol version and a coarse time window (30 s). The host answers from one datagram and keeps nothing: neither an entry
  in the strangers table nor an address (over UDP the reply goes through `Transport::reply`, and the address stays an "unknown sender" rather than a bound peer).
  The client puts the cookie into every channel datagram until it hears from the host, in the header's acknowledgement fields (they carry nothing at
  this point, and add no extra byte), and knocks again every 500 ms while the host is silent. Only a datagram with a message and with a cookie
  that the host issued to this address and this session, in this window or the previous one, creates a peer (a cookie lives 30 to 60 s).
  Without one, with another address's cookie (a forged address), with an invented one or with an expired one, the datagram is dropped and counted
  (`net_status`: `strangers.knocks`, `strangers.cookieless`). The key lives only in the host process's memory: it is in no snapshot, no replay and no journal.
  A knock with no answer for longer than `timeout_secs` ends the join: `no answer from the host … to
  the hello in 10 s: is it running there, and of this build? (a host of an older build does not answer it)`. The channel protocol version is 2,
  the session version is 2. A game from an older build (protocol 1: `Hello` right away, with no knock) is refused on its own protocol, over a channel
  opened only to answer it: `the host runs a newer build: wire protocol 2 (a join starts with a
  cookie round trip), yours speaks 1; run the same build` (no more than 8 such replies a second, for everyone together).
- **Connecting:** `Hello` (the protocol and engine versions, the game, the content hash, `rules_hash`, the tick rate) → `Accept` →
  `Ready` → the player's body (`player_join`; the game's `join_policy` hook can say "wait") → `Welcome` with the world snapshot.
  A mismatch gives `Refused`, with what to fix: `content differs: host 0x… you 0x…; run the same build and content`,
  `start with --hz 60`, `the session is full (2 players)`. A player name is 1–32 characters, with no control characters. A character (see
  "Saves (E8.1–5)" → "Co-op") of up to 64 KB travels inside `Hello` itself: a `Hello` with the largest such character, a 32-character name
  and a 64-character token fits under the limit before the player enters the game (test `the_largest_hello_and_the_largest_part_of_a_character_fit_under_the_pre_play_cap`).
  A bigger character, up to 1 MB (`MAX_CHARACTER_BYTES`), comes after `Hello` in parts (`CharacterPart`, 32 KB of JSON each, in
  order). `Hello` states the number of parts and bytes, and the host checks the size at once (over 1 MB is refused with `the character is
  1025 KB, over the 1024 KB a host takes`, before the first part). It keeps the peer's place as a player while the parts come in, and assembles them
  (out of order, over 32 KB, not the bytes that `Hello` promised, or not JSON are refused). A character that does not arrive within the join time
  (`6 × timeout_secs`) is refused with `your character did not arrive in 60 s (12 of 33 parts)`. The channel delivers the parts reliably and in order,
  despite losses and reordering (`a_character_too_big_for_the_hello_comes_in_parts_over_a_lossy_link_and_one_byte_over_the_cap_is_refused`:
  1 MB at 150 ms and 2 %). A message larger than an honest client sends is not parsed by the host: before entering the game, larger than 80 KB
  (refused with the limit); for a player, larger than 1 MB (a line in the journal). A peer's text (the name, the reason in `Bye`, a parse error with its words)
  reaches the host's and guests' journals cut short and on one line, and refusals come at most once a second. In one poll the host parses no more than
  2 × 80 KB from a peer while it is not playing (`JOINING_POLL_BYTES`), and 4 × 1 MB from a player (`PLAYER_POLL_BYTES`). The rest is dropped unread
  (`net: p1 sent over N KB in one tick: the rest was dropped
  unread`, one line a second for everyone, counted as `unread` in `net_status`), as is everything
  a peer sends after it has been let go (refused, kicked). Peers who are not yet playing share 4 × 80 KB per poll (`SHARED_JOINING_POLL_BYTES`): a peer
  who is short of a whole message is not read in that poll (its messages wait in its channel, which holds a limited amount and then slows the sender;
  `skipped` in `net_status`), and the next poll starts with the peer who was skipped, so an honest newcomer does not starve. When nobody is skipped, peers
  are polled in `PeerId` order: on the stand that is the order of creation, and over UDP it is the order of the address hash, so the player numbers of
  `Hello`s that arrive in one tick do not follow the order of arrival over UDP. A stranger's first message that is not a `Hello` (and not a `Bye`) is refused
  with `a join starts with a hello`. Something that does not decode is refused with `your first message does not decode (…)`. A `Hello` from another build
  whose set of fields has changed gets the same answer, and the text names both causes. The host keeps no more than `2 × max_players + 4` (`max_strangers`)
  peers who are not playing (joining, refused, gone). A new one pushes out, in order: a refused peer who left before the others; the oldest peer who has not
  said `Hello` (a peer whose character is coming in parts holds a player's place and is not pushed out, and neither is one waiting on `join_policy`), except
  for at most `max_players` of the oldest among those who may still hold a first message, meaning an unread message in the channel or a blob coming in parts
  from number 0 (the channel has already confirmed that and will not send it again; these are protected from creation for no longer than `NEW_PEER_GRACE_MS`,
  5 s, however much the peer sends after that); a message with a gap in front (number 1 without number 0) cannot be first, so it is not protected; and finally
  a peer who left before the others, even an admitted one (it loses what did not arrive of its farewell while the table is full; the host writes
  `forgot pN (gone) …`, and a peer who was disconnected between ticks goes last). Player places and protected peers together number no more than
  `2 × max_players`, and the table holds `2 × max_players + 4`, so there is always someone to push out. Only a datagram with a cookie creates a peer
  (see "Knock and cookie"), so a forged address no longer takes up room in the table: each stranger is a knock-and-reply round trip with an address that
  really receives the reply. The known limit remains, in a narrow form: someone with many real addresses (or many ports on one address), who knocks and sends
  from each without a pause, can still delay, or even break, the entry of a player whose `Hello` comes in parts (the player is protected only if they are
  among the `max_players` oldest of those who may hold a first message), or whose `Hello` arrives in one datagram in the same tick as a burst of such
  datagrams before it. There is no limit per address (IP): `PeerId` is a hash of the address with the port.
  A peer that does not go further in the join stage (no `Hello`, still building the world) for longer than `6 × timeout_secs` (a minute by default,
  `HANDSHAKE_TIMEOUTS`) is let go with `Refused` (`joining did not go on in 60 s`), however many acknowledgements it sends. The deadline does not apply
  to one the game is holding (`join_policy` answers "not now"). A peer that has left is forgotten when, for 10 seconds in a row, it confirms nothing
  of what was sent to it (`Link::reliable_acked` does not grow). One that does not confirm the last message, but sends empty confirmations, goes after
  10 seconds, while a slow peer that does confirm (a kick while megabytes of the world are on the way) gets its `Bye`; the ceiling is 5 minutes.
  `net_kick` takes `player` or `peer` (the number from `net_status`). That is how someone who has not said `Hello` and has no player number leaves too.
- **Returning:** on the first join the host issues a random token (32 hex digits, not from the name or the pid,
  different for every slot). The client stores it in `<save-dir>/net/<name>@<address>.token`. A restarted client with the same
  token gets back its slot and its body, for as long as the host holds them (`rejoin_grace_secs`). The stand keeps tokens in memory.
  A token whose slot another player has taken (a slot is free once the player who left has had their body removed) is kept by the host
  among the players who left (the last 64): a player who comes back with it gets a free slot and the same token, and with them what the host
  remembers of their character. A token stays one per player. A second game with the same token does not prevent this (it takes the slot,
  and does not touch the players who left), and neither does a host restart (the host's memory is empty, so the client gets a new token).
  Two games with one `--name` and one save folder share a token: the second takes the slot, and the host lets the first go with `Bye`
  (`another game joined with your token; give each game its own --name or --save-dir`), and the character goes with the game that left.
  The old connection of a restarted client goes the same way, until the host notices its timeout. Its leaving does not free the slot: the slot
  already belongs to the second game, even if that game waits on `join_policy` for longer than a body waits.
- **Input:** the client sends `Intent` frames with a tick number (with some redundancy, `input_redundancy`), and the host applies a frame on its tick
  to that player's body (the body is looked up again each tick, so the input reaches the body after a respawn too). Frame and
  packet limits (`inputs::check`: 64 frames, 8 presses, 16 held, 16 orders). Anything over a limit is dropped, with a line in the
  journal. A packet over 512 KB is not parsed. Of the packets that arrive in a tick, only the last is parsed (it repeats all
  unconfirmed frames). Unreliable packets from a stranger are not even parsed. An order (`move_to`, `stop`) travels with
  the held state, so `stop` on a client stops the body on the host. The client's order queue is not
  rolled back by the host's older state until the host has applied its frame. The client's clock over UDP does not go back. A client
  that has moved further from the host's last state than the lead plus 30 ticks (`STARVE_AHEAD`: the host is paused or
  hung) stands and waits. One state that reaches it is enough (it is applied on the next tick). Input that
  arrives more than 30 ticks before it is needed (a stale report after the host stopped) runs at half speed until the margin
  is back. More than 30 ticks late is a jump forward, to the host's last state plus the lead, and only
  forward. The host has already received the frames for ticks that the client would replay: it would drop a repeated frame of the same tick as a
  duplicate, along with the presses the client had taken from `Intent`, and it would apply an old frame after the new ones.
- **State:** deltas by the replication rules, in the area of interest around the players (`interest_radius`), plus what
  its entities refer to (up to 64 references; the entities of the area itself are always sent). A view whose references hit the limit
  is incomplete on the client, so while that holds its hash is not checked (otherwise the world would be rebuilt every 5 seconds). The host's journal
  names the player once: `net: p1's view names over 64 entities beyond it`. When the limit stops applying
  (the entities the references pointed at are now in view), the client gets the world again once (`view complete`): the references
  that it read as nothing do not change in the deltas, and the hash would start being checked against them. The view's memory (what stays in
  it through the hysteresis of the area-of-interest radius) does not wipe the world: the limit is lifted with that memory counted; without it, the limit would drop back onto the view
  again and the world would be sent again every 4 ticks. And no more than once every 5 seconds (`DESYNC_RESYNC_COOLDOWN_SECS`): if the player stands where
  the limit comes and goes, the view counts as limited until the cooldown ends (the hash is not checked), and the world goes out
  when it is over. A value that does not read back
  (NaN and infinity: JSON has no such values) stays on the host, and the host's journal names it once: `net: Home of e6 is not
  sent to clients (does not read back: …)`. The encoding cache forgets removed entities. A reference name
  (`e12`) does not outlive its target.
- **World again** (`Rebase`): after a snapshot or a region change on the host, on `net_resync`, and on a desync. Before it, the host
  checks the players against the new world: a body goes to whoever has none, a departed player's body waits for them, and a player whom the host
  added itself (`player_join` with no peer: co-op on one machine) stays as it is, whatever number a guest held before it.
  The world itself says whose body it is, not the history of slots: bodies that the session gave to peers are marked `NetSeat` (the mark is part
  of the snapshot), and guests' bodies from a snapshot loaded into the host's new process go and free their numbers. If the channel
  is not accepting the world now (by message count or by bytes), sending waits, the world is not captured again every tick, and
  the epoch does not move. While the host's ticks run in a row (`net_resync`, a desync, a region change), input in flight is not
  lost, the client's clock does not go back, and whatever the host reset on the body (a region change stops the players), the client
  sends again if the key is still held. A snapshot that moves the tick starts input again, and reports of
  a desync are counted from its tick. That holds too when its world waits for room in the channel while new ticks are already running.
  The host notices a tick jump before it admits the players of the same frame: a player admitted in it gets the world only after the jump, and
  its next world does not restart input again, while the ticks run in a row.
  A world over 16 MB (the channel's `max_blob_bytes`) is not sent by any channel: the player gets `Bye`, and the host writes once to the
  journal `net: the world is 17.0 MB, over the 16 MB a link carries: p1 cannot be given it and is sent away`. The world is not
  captured or encoded again on each tick (when the queue is full, it still waits).
  `net_event` events that the client has received but not yet shown are dropped with the new world: the channel is ordered, and the host sends
  events before the world that replaces them, so all pending events belong to the old world and name its entities (after
  a snapshot of an earlier tick, the same ids go to other entities). Events of the new world arrive after it.
- **Desync:** A hash goes out every `desync_every` ticks. A client's report counts only if the host really sent
  this hash to this client, once a tick, and the world is rebuilt no more than once every 5 seconds. The `net_desync` condition fires on both
  the client and the host. The client's `net_status` names how many times its copy has diverged from the host (`desyncs`), and the last
  desync (`last_desync`: the host's tick, its own tick, the first difference).
- **Guest calls:** the host runs only what the client forwards (`NetRoute::Host`), with a scope no higher than
  `guest_scope`, always from that player's body (a foreign `subject` is refused: other entities are named as arguments
  of the tool), no more than 16 calls per tick and 8 agents per player. A world that a guest's call rebuilt (`regen` or
  `region_load` with `guest_scope: Dev`) is sent again to all clients, as after a call on the host itself: the
  `WorldRebuilt` event waits in the `PendingRebuild` resource (the host is not in the world at that moment, it is inside its own tasks), and state deltas
  do not go to the player until the new world has been sent to them (in the rebuild tick, and after it, while the world waits for room in the channel or
  is not yet captured): they would give the client the new world's resources (the region record) without its entities, and the world that arrived later would find
  the region already built and refuse it because of `static_colliders`.
- **Time on the host:** over UDP, `pause`, `resume` and `speed` are refused on the client (`the host owns time; call it on the
  host`; the `HostOnly` route is set when the client opens a UDP socket). On the stand they run on the stand's shared clock.
- **End:** `net_end` or the host quitting (Ctrl+C, `kill`) first saves the host's world and character (the `on_session_end` hook,
  once; for the umbrella crate this is `shards::coop`), then sends the clients their characters (`CharacterSave`) and `SessionEnd`,
  which names the world's slot: `net: session ended by host (the host quit), world saved: slot1`. Each save goes
  once: a client's character leaves with `SessionEnd`, and not again with the player leaving. Players leave the host's world (the body waits, as it does for a player who left,
  there is no second character), and those who are still joining or waiting on `join_policy` get `Refused` (`the session has ended (…)`). The host
  does not create new peers. A client's exit is `Bye`, and its body waits for its return. The character goes with the connection that
  left (a kick, a timeout, `Bye`, eviction by the same token). A client whose session has ended acknowledges what the host still sends, and
  after 3 seconds of silence from the host it stops altogether. The host itself does not create a peer from a datagram without messages (only
  acknowledgements). With `--exit-with-host` the client exits: 0 — the host ended the session, 1 — refused, kicked, lost the host, or the world was not
  accepted; 2 — the session cannot be opened (the address cannot be parsed; the port of `--host` is busy). The host does not exit on `net_end`. Messages that a peer's channel did not accept
  (too much unconfirmed) are counted and named, one line a second per player. A kick, a character and the end of the session
  that the channel did not accept are named at once, not counted: a player who is leaving will no longer be there for the channel's check
  (`net: p1: the end of the session not sent: reliable queue is full …`).
- **Characters and the world** (E8, `shards::coop`): the character lies with the player and enters the game with them (`Hello`). Progress goes back to it from the host (`CharacterSave`), and only the host keeps and writes the world. For details, see "Saves (E8.1–5)" → "Co-op".
- **Content does not change during a session.** The content hash is checked at join, and content is not sent during the session. While other machines play this content (the host has a client past `Hello`, a client in the session), an edit would desync them: on the host, `content_reload`, `content_apply` and `level_reload` are refused (`refused: a content reload during a co-op session would
  desync its 2 clients; do it once they have left, or after net_end`), and the client does the same (`… would desync this game from
  its host 10.0.0.2:7777; leave the session first`), and nothing is written to the replay.
  A file that changed on disk (the file watcher) is not applied and waits. The journal says so once (`content: a file changed on disk and waits:
  a content reload during a co-op session …; it applies when that is over`; for a level file, `level: …`). When no clients are left, or the session has ended, the edit applies itself.
  Nothing is spawned from such a level file while the edit waits: the asset server has already read the new file, and the other machines play on the old one. So `spawn_level` and a region tile from it are refused (``refused: `ruin.gltf` changed on disk, and a level spawn from the changed file during a co-op session …``).
  The decision is a ban, not a broadcast. The host would have to send definitions (and level files, and baked navigation) and change the content for everyone on one tick, and the world built around the old content (the region, the navmesh) would have to be built again. The ban closes the desync that ended the first half-hour `soak`, when a region file was changed in the middle of play.
  The mechanism is `ContentGuards` (`add_content_guard`) in the `shards_content` crate: a module names the reason why the content cannot be changed now.
- **Tools and conditions:** `net_status` (role, players, RTT, losses, channel conditions; on the host, the address the clients connect to, as shown in `ai/status` (`net.udp`) and the `shards wait` line, and `strangers`: knocks, what was dropped without a cookie, refused games of an older build), `net_sim` (`latency=150,loss=0.02`, `none`), `net_kick`, `net_resync`, `net_end`, `net_overlay` (the network overlay, F6 in the window); conditions for `step --until`:
  `net_desync`, `net_joined`, `net_left`, `net_disconnected` (a client whose session has ended, including a kick and a refusal).
- **Stand** (`--stand N`, only with `--headless`): the host and N clients in one process, on a network in memory, each with its own BRP port and port file (`F`, `F.c1`, …). Time is shared: `step` from any participant drives all of them. `pause`, `resume` and `speed` (not below 0; infinity, for example `x=1e39`, is refused and the stand is not touched) show in each participant's `ai/status`, and the `speed` reply names the new speed. The stand ends only with the host: a client who was kicked (`--exit-with-host`) leaves the session, and the rest keep playing. Ctrl+C ends the session the way a host exit does, and waits (in network time) until the clients have received the end of the session. Each participant's exit hooks write to its journal. On a stand, the player settings (`--settings-dir`, `--no-settings`) are the host's settings: the stand's clients keep none of their own.
- **Stand over a fake Steam** (`--stand N --stand-steam [--steam-lobby TYPE]`, a game built with the `steam` feature): the same participants in one process, but they meet through `FakeSteam` (the relay is the stand's in-memory network, with no delay; `--net-sim` works on top of the relay, as with real Steam, and applies only once: `latency=50` is 100 ms there and back, and the fake's `route to` prints `ping 0 ms`), not directly. The host makes a lobby (`friends` by default), and the clients start as `--join steam:` and wait for a lobby (the ready line `role client · joining: …`), which `steam_lobbies` looks for and which `steam_join` joins. `Stand::settle` waits for the clients to load, not for them to join: they cannot join until a lobby is chosen. So `shards wait --peer all` and `--peer c1` (they wait until the client has joined) do not return here until the client is in a lobby: wait for the host with `shards wait`. `shards scenario` and `shards stand` see the stand anyway. The stand never touches real Steam, nor `steam_sdk`: `connect` is off on the stand. The scenario is `crates/shards_steam/scenarios/lobby_search.scenario` (see "Finding a lobby").
- **Stand over UDP** (`--stand N --stand-udp`, only with `--headless`, only in real time): the process is the host on a free UDP port (as `--host 0`), and the clients are child processes of the same game: `--join 127.0.0.1:PORT --headless --port 0
  --port-file F.cK --name cK --no-settings --exit-with-host --save-dir <save-dir>/stand-cK`, with the same `--seed`, `--hz`, `--net-sim` and `--tick-budget-ms`. UDP games cannot be stepped together, so an agent "steps" such a stand by waiting (`shards
  step 120` on the host runs 120 ticks in a row, and the clients follow), and `--paused` is rejected for that reason. The clients leave together with the host (the end of the session, then SIGTERM to any that remain; on Linux also PDEATHSIG). The stand stops with the exit code of a client and a line with its name, if the client did not start, or exited before it wrote its port file (`error: stand: client c2 exited with code 2 before it was ready …`), if it exited without ever joining the session (the host did not let it in, before or after `Hello`: the session was full, the body was not created, the world was not sent, or the join did not finish in time; or it was kicked, or fell silent while joining; `… without being admitted to the host (refused, kicked or timed out while
  joining: is the session full?)`), or if it crashed after joining (any exit other than the ones listed below: `… while the stand ran: it crashed`). An exit counts as a client leaving of its own accord if it has code 0, or `128+signal` (129, 130, 143), or is the signal SIGHUP, SIGINT or SIGTERM itself (the ones the game exits on cleanly). Such a client, whether or not it said goodbye to the host, is recorded only in the host's journal (`… while the stand ran`), and the stand goes on. Likewise, an admitted client (one that got its body) whose link the host itself closed (`net_kick`, a silence
  timeout): it exits with code 1 under `--exit-with-host` (`… after the host closed its link to it`). A kick or silence while the
  client is still joining is not excused by this: it never got in, and the stand stops. These do not count: a refusal (a client the host did not accept,
  which never entered the session and so had no link that could be closed), a game that a second game with the same token pushed out (it is lost
  as a participant), and a goodbye that the client sent itself. A client that could not take the host's world (`Bye`
  "cannot take the world") and exited with code 1 stops the stand, like any crashed client. The host records who closed the link
  into the slot as soon as it is closed (`NetHost::link_closed_by_host`), rather than parsing the text of the reason. Whether a client got in is said by
  `Slot::admitted` (it got its body), not by the slot existing. The host's exit hooks (the end
  of the session for clients, saves, the other children) run in the same system that stopped the stand. If the stand ended
  earlier (a signal in the frame where loading finished), the children do not start at all. The children start not in
  `PostStartup`, but on the first frame after the content has loaded, and only after a check that the session has room for N clients
  (`max_players` in the network definition, 3 by default: the host and two clients). Otherwise the stand ends with code 2 and the line `error:
  stand: --stand 3 needs room for 4 players, but the session allows 3 …`, and no process is started (`--stand 20`
  does not start twenty games just to refuse). Before it starts the children, the host removes the port files that an earlier stand left
  (`kill -9`): `F.c1` … `F.cN`, and all the `F.cK` in a row after them, so that a leftover file is not counted as a participant or as a client
  that has already started.
- **The stand from the command line.** All the stand's participants are visible as separate games, each with its own token (`.shards/<port>.token`):

  ```text
  $ cargo sandbox --stand 2 --headless --paused --port 0 --port-file F --net-sim latency=150,loss=0.02 &
  $ export SHARDS_PORT_FILE=F                           # members are found by F, F.c1, F.c2; --port-file is not needed after this
  $ cargo shards wait --peer all                        # one line per member, plus a check that it is one session
  member=host role=host player=0 body=e5 port=44639 pid=31446 seed=42 tick=0
  member=c1 role=client player=1 body=e15 port=39311 pid=31446 seed=42 tick=14
  member=c2 role=client player=2 body=e16 port=44345 pid=31446 seed=42 tick=14
  stand: host + 2 clients · seed 42 · one session
  $ cargo shards --peer c1 observe                      # as client c1 (or SHARDS_PEER=c1); the port is read from F.c1
  tick 14 (0.23s) | paused | speed 1x | seed 42 | you e15
  YOU  e15 Player  hp 100/100  stam 100/100  poise 60/60  flask 3/3  at (2.0, 4.0) facing N
  NET connected as p1 (e15) on host host, rtt 300ms, host tick 0, 14 ticks ahead, desyncs 0, prediction error 0.00 m …
  …
  $ cargo shards step 420                               # a step on any member runs them all
  $ cargo shards stand                                  # members: role, player, body, port, ping, desyncs
  stand: host + 2 clients (in this process) · seed 42 · links latency=150,jitter=0,loss=0.02,dup=0
  MEMBER  ROLE    PLAYER  BODY  PORT   PING    DESYNCS  TICK
  host    host    0       e5    44639  -       0        420
  c1      client  1       e15   39311  300 ms  0        434
  c2      client  2       e16   44345  300 ms  0        434
  $ cargo shards stand stop                             # net_end to the host, then quit (a signal for a game without net_end); waits until nobody answers
  session ended: stand stopped
  stand stopped: host pid 31446, c1 pid 31446, c2 pid 31446
  ```

  `--peer all` works only for `wait` and `stand`. Each member has its own token in every command and scenario (`observe`, `act`, `step`,
  `status`, `wait --peer c1`, `save-replay`, a one-game scenario under `--peer c1`, and also `wait --peer all`, `stand` and
  the stand scenarios). The host's token is `--token` (`SHARDS_TOKEN`), otherwise `.shards/<port>.token` from the current folder upwards. The client's
  token is `SHARDS_TOKEN_C1`, `SHARDS_TOKEN_C2`, otherwise its `.shards/<port>.token`, and `--token` is used only if
  neither is set (so you can pass a client's token explicitly). With `--port` (one game) `--peer` does not pick the token: it is
  `--token` or that game's file. An `unauthorized` reply names the variable you need to set. `wait --peer all` waits
  as long as the launch takes, but stops at once when waiting makes no sense: the host or a client has already answered and then disappeared (the host
  ended the stand: `the host ended the stand: the game was there and is gone …`; a host port file that `wait` managed to
  read and that later disappeared means the same, even if the host never answered: `… the game's port file is gone …`;
  a stand that died before `wait` read `F` still waits until `--timeout`, as before, since there is nothing to read; this does not
  apply to client files, whether they are named by `--peer c1` or by the file itself (`--port-file F.c1` without `--peer`): the UDP stand host removes
  the ones left by the previous stand itself), a member answers
  `unauthorized` twice in a row, or the members name the same mismatch twice in a row (the first member is not the host, a foreign seed, two
  clients with the same player number). If a client is missing, or the host has not yet listed the players, the launch is still in progress, and it waits until
  `--timeout`. The stand size for `wait`, `stand` and the scenario comes from the host's `ai/status.net.stand.clients`, not from the port files
  (a file left over from the previous stand is not counted as a member; without `net.stand` on the host, the clients are the ones whose files follow one another). A client whose
  port file has already been removed (it has left) is shown by `shards stand` as the line `c2: cannot read the port file …`; `stand stop` names
  it and still stops the others; the exit code is 1 in both cases. On a stand, `step`, `pause`, `resume` and `speed`
  reply `{"pending": id}`: `shards` polls `ai/step_poll` every 20 ms (`--timeout`, 60 s by default) and prints
  a report, as it does for a plain `step`. `session --player N` (BRP `ai/session player=N`) binds the agent to the body of player N.
- **Long run: `shards soak`** (acceptance for E7). Seeded agents play for every member of the stand at once. In each round,
  each one looks (`observe`) and makes one random player input based on what it sees (walking, hitting, dodging, like
  `fuzz`); the host heals a dead body (`heal`). From time to time the host changes the region (`--regen R --regen-minutes M`, each change with its own seed) and saves everything (`--save-minutes M`). These are two cases where the clients get the world or the character anew; changes and saves run exactly on schedule (every M minutes from the start), and a step does not skip past their due time.
  Then the whole stand takes 1–30 ticks (`--max-ticks`) with `--until named:net_desync`. The first desync, a crashed system or a violated invariant stops the run with exit code 3; at the end the run reads `net_status`
  from each member, and a member that does not answer fails the run too (`desyncs ?`, `FAIL … no net_status from c1`). With one game (`--port`, no stand) only that game plays. Acceptance output: half an hour of game time,
  host and two clients, 150 ms one way and 2 % loss:

  ```text
  $ cargo shards soak --seed 30 --minutes 30 --report-minutes 5 --regen wilds --regen-minutes 5 --save-minutes 3
  soak seed 30: 30:00 of game time on host, c1, c2 from tick 0
  tick 18000 (5:00 of game time): 1178 rounds, 3526 inputs, desyncs 0; host: p1 rtt 300 ms loss 1.2% resyncs 0 late inputs 0 · p2 …
  tick 18000: the host goes to wilds (seed 925482)
  …
  tick 90053: the host goes to wilds (seed 308905)
    host: 7003 inputs (1292 refused), revived 2, desyncs 0; host: p1 rtt 300 ms loss 1.6% resyncs 5 late inputs 0 · p2 rtt 300 ms loss 0.0% resyncs 5 late inputs 0
    c1: 7005 inputs (1163 refused), revived 0, desyncs 0; client Connected: rtt 301 ms, loss 3.2%, rebases 5, prediction error mean 0.06 m max 1.00 m, snaps 14
    c2: 6998 inputs (1193 refused), revived 7, desyncs 0; client Connected: rtt 301 ms, loss 3.2%, rebases 5, prediction error mean 0.04 m max 0.67 m, snaps 21
  PASS soak seed 30: ticks 0 → 108000 (30:00 of game time), 7005 rounds, 5 region changes, 9 saves
  ```

  Here `resyncs` and `rebases` are the five region changes, not desyncs. Refusals (`refused`) are part of the game: no stamina,
  the body is busy. On an in-memory stand, half an hour of game time takes a few minutes. On a UDP stand (`--stand 2 --stand-udp`) the game runs
  in real time, but `step` runs its ticks at once, so `soak` drives the host ahead (in the run with seed 31, 2.7 times ahead): the clients' input arrives late more often, prediction resets more often, yet there are still no desyncs. In `cargo test` the same job is done by `shards::script::soak::run` through
  `shards::testing::StandDriver` (`crates/sandbox/tests/sandbox/soak.rs`: two minutes; the half-hour one is under `#[ignore]`). CI
  runs three minutes on a live stand on every push; the half-hour run is `soak.yml`, every night. The very first run found that
  a body could leave the edge of the region: now an edge wall holds it at the edge (`edge_wall`, "Content formats").

#### Prediction and smoothing (C4)

The client moves its own body and starts its actions at once, without waiting for the host; the host confirms or corrects them. Other entities in the window move smoothly, not in steps of every `send_every` ticks. In a single-player game and on the host nothing changes.

- **Own body.** On each client tick, `NetSet::PredictInput` → `NetSet::Predict` → `NetSet::PredictEnd` run, only for the
  body with `Predicted`. These are the same systems as on the host, with the filter `With<Predicted>`: orders (`follow_orders`, the walk to the target of an `attack`),
  `intent_to_locomotion`, the controller and the action machine. The body sees the inputs the client sent for
  this tick. Only components with `net_predicted` change (`Transform`, `MotionState`, `LocomotionInput`, `Intent`,
  `GlideUntilLanded`); the desync hash does not compare them, and the host's deltas do not write them.
- **Orders.** Orders drive the body the same way as on the host, but what they wrote (`move_dir`, `aim`, `target`, the queue)
  is rolled back after the tick: the host carries out orders, and the client sends only what the player changed. An order that
  prediction has carried out (the path point is reached) counts as done for it: the next ticks head for the next point, without stopping
  at each one. When the host has finished an order, the client takes its queue, and for a finished attack also its `aim` and `target`, if
  the player has not changed them since. After the world is replaced, orders that the host has finished or reset to its own tick count as finished
  for prediction too.
- **Actions.** The combat tools (`dodge`, `light_attack`, `heavy_attack`, `block`, `parry`, `use_item`,
  `use_technique`, `attack`) answer based on the action shown on the client (`PredictedAction`): `ok: roll (E) ·
  predicted, host confirms`. A refusal comes at once and is the same as in a single-player game. Only the host counts damage, death and hits. The journal names an action that the host did not start: `net: predicted dodge not confirmed (stamina 10/20)`.
- **Correction.** With each state from the host, the client compares the body's feet at the host's tick `T` with what it predicted for `T`,
  minus the corrections made since then. An error under 2 cm is left as it is. Otherwise, each tick the body moves by a share
  `correction_rate` (0.15 by default) of the remainder, until the next comparison says how much is left; the share does not
  depend on `send_every`. A snap (the body jumps to the host's place and takes its motion state; a line in the journal) happens
  when the error is larger than `snap_distance` (1.5 m), the motion mode differs for 6 ticks, the body died, or on the first comparison after
  a new world or a clock jump forward, if the body moved in that time: `net: e5 corrected by 1.2 m (snap: motion
  mode)`, `(snap: world taken)`, `(snap: clock skipped)`. The host state is taken at tick `T`, but the body is at the client's latest tick, so it lags that many ticks behind the host's stamps. Until it catches up with them, the body is left alone (except for death), and a landing or the end of a climb does not cause a second snap.
- **Other entities in the window.** Their game `Transform` is the host's newest state: physics, `observe` and the hash read it. The window
  shows them a little in the past, between two states around `host tick − (send_every + jitter)`. A jump of more than 4 m
  is not smoothed. A new entity (one that appeared or came back into the area of interest) stands in its place at once, and does not fly
  in from the origin. Without a window none of this exists.
- **Numbers.** `net_overlay` (F6 in the window) shows the channels (ping, loss, kbit/s, clock margin), desyncs and
  prediction: the mean and largest error over 600 ticks, snaps, unconfirmed actions. In `net_status` the same is in the
  `prediction` field (`error_mean_m`, `error_max_m`, `compared`, `blends`, `snaps`, `last_snap`, `actions`). The error
  figures start over when the world is replaced.

### Transports and the CLI

**BRP `ai/*`.** JSON-RPC 2.0, `POST http://127.0.0.1:15702`:

| method | params |
|---|---|
| `ai/act` | `{agent?, tool, args?}` |
| `ai/observe` | `{agent?, since_seq?}` |
| `ai/step` | `{agent?, ticks?, until?}` |
| `ai/journal` | `{agent?, since_seq?, last?}` |
| `ai/shot` | `{agent?, marks?, path?}` |
| `ai/tools` | `{agent?}` |
| `ai/session` | `{agent, scope?, subject?, clear_subject?}` |
| `ai/status` | — (in the reply, `instance: {pid, seed, port, started_at}`: which game answered; `engine`: version, modules, state; `loading`: what is still loading; `perf`: the time of the last 600 ticks) |

The standard BRP methods (`world.query`, `world.get_components`, …) are available too: `shards raw …`. BRP gives full access to the
world, so the server listens only on 127.0.0.1, is built only with the `remote` feature, and lets in only callers with the token:

- at start, the game writes a random token to `.shards/<port>.token` in its working folder (permissions 0600), and deletes the file on
  exit: on Ctrl+C, `kill`, closing the window or the terminal (unless the game was started with `nohup`). After `kill -9` the file
  stays, and the next game on that port overwrites it;
- every method — `ai/*`, the standard `world.*`, `registry.*`, `rpc.discover` and `+watch` — requires `params.token`. Without it,
  or with someone else's token, the reply is always the same: `unauthorized: run shards from the game folder or set SHARDS_TOKEN`;
- `shards` takes the token from `SHARDS_TOKEN` (`--token`) or from `.shards/<port>.token`, searching upwards from the current folder, and
  adds it to the parameters itself (a stand client, `--peer c1`, has its own: `SHARDS_TOKEN_C1`, otherwise its file; see "The stand from the command line"). Pass the token to your own client explicitly:

```bash
TOKEN=$(cat .shards/15702.token)
curl -d '{"jsonrpc":"2.0","id":1,"method":"ai/status","params":{"token":"'$TOKEN'"}}' http://127.0.0.1:15702
```

A release build without BRP (and without the file watcher):

```bash
cargo build --release -p sandbox --no-default-features --features render
cargo build --release -p sandbox --no-default-features --features remote   # only for --headless (server, generation)
```

**CLI `shards`:** observe, step, act, scenario, shot, save-replay, save-snapshot, load-snapshot, assert, fuzz, tools, inspect, nearby,
journal, pause, resume, speed, status, session, content, worldgen, raw, wait. `act` arguments are passed as a JSON object or as `key=value` pairs. The flags
`--port`/`SHARDS_PORT`, `--port-file`/`SHARDS_PORT_FILE` (the port from the game's file; `--port` takes precedence), `--token`/`SHARDS_TOKEN`,
`--agent`/`SHARDS_AGENT`, `--json` (for `tools` — with argument schemas). `shards wait` waits until the game answers and
loads (the content; while it loads, `step` replies with an error), and prints who answered: `ready: tick 0 · pid 4242 ·
seed 1 · port 15711` (the game prints the same line at start, and `shards status` prints it as its first line). It exits at once
with a reason (code 2) when waiting makes no sense: the game has already answered and disappeared, it answered `unauthorized` twice in a row,
the host's port file was read and then disappeared, or the command line does not name a game (`--peer c1 wait` without a port file). `shards
content check` re-reads the content files of the running game and exits with code 1 on errors; without a running game —
`--check-content`. `shards worldgen --region R --seeds 1..1000 --report` prints the batch report of the generator for the running game
(exit code 1 while not every seed has passed); without a game — `--worldgen-report R --seeds …` of the game itself.
Exit codes are below.

**Parallel headless instances.** Each has its own port:

```bash
cargo sandbox --headless --paused --seed 1 --port 15711 &
cargo sandbox --headless --paused --seed 2 --port 15712 &
SHARDS_PORT=15711 cargo shards step 600 --until named:player_dead
```

- A busy port is an error: the game does not start, it writes `port 15711 is busy (another game?). Pick another with --port` and
  exits with code 3. Without this check, a second game would run without a server, and `shards` would quietly control the first one. The check
  is available to any game: `ai_core::remote::ensure_port_free(port)`.
- To avoid picking ports by hand, `--port 0` takes a free port, and `--port-file F` writes its number to a file (on
  exit by Ctrl+C, `kill` or closing the window, the file is deleted). `shards` reads it through `--port-file F` or
  `SHARDS_PORT_FILE=F`:

```bash
cargo sandbox --headless --paused --seed 1 --port 0 --port-file /tmp/shards-a.port &
cargo sandbox --headless --paused --seed 2 --port 0 --port-file /tmp/shards-b.port &
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards wait   # ready: tick 0 · pid 4242 · seed 1 · port 40817
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards step 600 --until named:player_dead
```

**Inside the game** the entry point is the same as behind BRP: `ai_core::call_tool(world, agent, tool, args)`. Tests,
replays and developer hotkeys in the window call it.

**Exit codes.** `shards`:

| code | means |
|---|---|
| 0 | done |
| 1 | the tool refused; the error says why and how to fix the call (`content check` and `worldgen` also return it when they find errors) |
| 2 | `shards` itself failed: no game on the port, no token, a wrong command line |
| 3 | the game stopped: `step` (or `fuzz`) stopped at a crashed gameplay system or a violated invariant; the reply names the cause and the replay |

`shards scenario` has its own codes: 0 — all passed, 1 — a scenario failed, 2 — the file cannot be read or the game is not available (see "Scenarios").

`cargo sandbox` (and any game on `shards::run`):

| code | means |
|---|---|
| 0 | the game closed; with `--verify`, the replay matched the recording and nothing broke; `--check-content`, `--worldgen-report` — no errors; `--smoke` — the ticks ran; `--perf` — all budgets were met (or a debug build, which does not judge) |
| 1 | `--verify`: the game diverged from the recording, a gameplay system crashed or an invariant was violated; `--check-content`, `--worldgen-report` — found errors; `--smoke` — did not load, a system crashed or an invariant was violated; `--perf` — a budget was exceeded or the game did not reach the measurement |
| 2 | the replay cannot be read or does not fit the game; `--port-file` cannot be written; `--perf` — no such definition, or with `--offscreen` there is no GPU |
| 3 | the port is busy (`port 15711 is busy (another game?)`) |
| 128 + N | stopped by signal N: 130 — Ctrl+C, 143 — `kill`, 129 — a closed terminal (SIGHUP) |

**Example session (real output).** The line below the first line of a `step` reply gives the time of its ticks (see "Tick time (E11)").

```text
$ shards step --until journal:winds
ran 208 ticks (0 → 208), stopped: journal: 208 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
ticks: avg 1.49 ms, max 3.89 ms (tick 120), budget 16.7 ms
CHANGES
  ~ e6 Boss "Frost Warden": +windup shockwave 0.50s (zone line 9m×1.6m); +fight e5 · token · leash 11/26m; moved 10.6m S
  …
JOURNAL
  2 e6 guard → fight e5
  2 e6 takes an attack token on e5 (1/1)
  208 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
$ shards act dodge
ok: roll (W)
$ shards step 40
ran 40 ticks (208 → 248)
ticks: avg 1.43 ms, max 3.20 ms (tick 242), budget 16.7 ms
CHANGES
  ~ e5 Player: stam 100/100 → stam 88/100; moved 4.0m W
  ~ e6 Boss "Frost Warden": windup shockwave 0.50s (zone line 9m×1.6m) → active shockwave 0.08s (zone line 9m×1.6m)
$ shards act attack target=e6 repeat=true
ok: attacking e6 with light until it dies
CHANGES
  ~ e5 Player: +order attack e6 (repeat)
$ shards step --until journal:light_2
ran 101 ticks (248 → 349), stopped: journal: 349 e5 -> e6 light_2 -22 (hp 558)
ticks: avg 1.28 ms, max 3.55 ms (tick 338), budget 16.7 ms
CHANGES
  ~ e5 Player: stam 88/100 → stam 84/100; +active light_2 0.10s (zone cone 2.2m 110°); moved 6.2m NE
  ~ e6 Boss "Frost Warden": hp 600/600 → hp 558/600; poise 140/140 → poise 114/140; fight e5 · token · leash 11/26m → fight e5 · hold · leash 11/26m; -active shockwave 0.08s (zone line 9m×1.6m)
  …
JOURNAL
  296 e8 idle → fight e5
  …
  331 e5 -> e6 light_1 -20 (hp 580)
  349 e5 -> e6 light_2 -22 (hp 558)
$ shards act heavy_attack
ok: queued, heavy starts in 0.10s
```

### Screenshots

`shot` (scope Dev) captures the game window with Bevy's own tools. It does not capture the desktop. So a window without focus, which
the compositor redraws with a delay, still gives the current frame. A game without a window, but with a GPU (`--offscreen`), is captured the same way; see below.

```text
$ shards shot
saved /…/shots/tick90-1.png (1280x720, tick 90, marks on)
LEGEND (image pixels)
  e1 Pillar  px(496,182)
  e5 Player  px(640,387)
  e6 Boss "Frost Warden"  px(640,207)
  e7 Skeleton  px(349,260)  occluded
  …
```

- The overlay with ids is switched on for the time of the shot (a frame before the capture, so that the gizmo lines reach the GPU). The shot itself waits until the GPU
  has finished compiling the pipelines: right after start, meshes and labels would otherwise be skipped.
- The legend is computed in the frame that is captured. After `step`, the follow camera is still catching up with the player, so the coordinates at the time of
  the request would be wrong. The legend is written beside the PNG as `<name>.png.legend.json` (`path`, `width`, `height`, `tick`,
  `marks`, `legend`): this name cannot collide with someone else's `<name>.json`.
  The coordinates are in pixels of the image (the whole window image), not of the 3D scene: with `graphics.render_scale` not 1, the legend is computed as the ratio of
  the image size to the logical size of the camera, and the labels sit on exactly the same pixels as at 1.0.
- Shots are written only inside `shots/` of the game's working folder: `-o shots/boss.png` is fine, but an absolute path, `..` and an exit through
  a symlink are not (`path must be inside shots/, e.g. shots/boss.png`).
- Both files are written to temporary names and then renamed, so a PNG that appears is already fully on disk. An old shot and its
  legend with the same name are deleted when a new one is requested, and only inside `shots/`. A `screenshot saved: …` line appears in the journal.
- The tool returns the paths at once, and `shards shot` waits for the file (`--timeout`, 10 s by default). Flags: `--no-marks`, `-o PATH`.
  Without an image (`--headless`) the tool replies with an error and a hint: `observe`, a window or `--offscreen`.

**Without a window (E10.8).** `--offscreen` (`EngineConfig::offscreen`) runs the game without a window, but with a GPU: for CI and agents on a machine with
a graphics card, and, with software rendering, on a machine without one. No window opens, and no display is needed.

```bash
cargo sandbox --offscreen --paused --port 0 --port-file /tmp/g.port &
export SHARDS_PORT_FILE=/tmp/g.port
cargo shards wait && cargo shards step 30 && cargo shards shot -o shots/arena.png
```

- Instead of a window, the game draws one image the size of `window` (1280×720): the `ai_core::display::Offscreen` resource. A camera aimed at the main
  window (as the default camera is) is re-aimed at that image (`RenderTarget::Image`): when it appears, and again later, when a game system
  changes its target to the window (before Bevy resolves camera targets in `PostUpdate`). The UI goes to the camera that
  draws last (the highest `order`), as in the window; if the game marks a camera `IsDefaultUiCamera`, the UI goes to that one. The HUD,
  overlays and the developer menu panel (`dev_menu open=true`) are drawn as in the window. Only `shot` reads the picture.
- There is no window that waits for a frame to be shown, so each frame waits until the GPU has drawn it. Otherwise a slow GPU
  would get frames faster than it draws them, and memory would run out (this is how the Windows software renderer, WARP, crashed). It waits no longer than
  30 s: a GPU that has not finished a frame in that time has hung, and the game exits with code 1 (in the journal and the log: `offscreen: the GPU
  did not finish a frame in 30s: the game ends`), rather than waiting forever or queuing more frames.
- `ai/status` reports where the picture goes: `display` is `{"mode": "offscreen", "size": [1280, 720], "scene": {"size": [1280, 720],
  "scale": 1.0}, "gpu": "NVIDIA GeForce RTX 5080 (Vulkan, DiscreteGpu, driver NVIDIA 610.88)"}`; there are also `window` and `headless`
  (`headless` has a size, and `scene` and the GPU are `null`). `scene` is the size at which the 3D world is drawn (`graphics.render_scale`, see "Settings (E8.6)"); at
  a scale other than 1, the 3D cameras draw into an image of this size, and `shot` takes the whole picture: the labels in the legend are in its pixels.
- Without a GPU, `--offscreen` exits with code 2: `error: --offscreen draws on the GPU, and there is none: … Run with --headless (no
  pictures), or let a software renderer draw: WGPU_FORCE_FALLBACK_ADAPTER=1`. The software renderer is chosen by
  `WGPU_FORCE_FALLBACK_ADAPTER=1`: on Windows it is WARP (`Microsoft Basic Render Driver`, checked: the same picture,
  only slower), on Linux it is Mesa lavapipe (`mesa-vulkan-drivers`; not checked here).
- A game without a window is the same: gameplay, `observe` and the replay do not depend on where the picture goes. With `--offscreen`, as with `--headless`,
  the game reads the player's settings only from `--settings-dir`: the picture does not depend on the machine.
- The sandbox test `offscreen::an_offscreen_game_takes_shots_without_a_window` (`#[ignore]`: needs a GPU or a software renderer):
  `cargo test -p sandbox --test sandbox -- --ignored offscreen`.

Shots with a window but no display are still taken through Xvfb and software Vulkan
(`apt install xvfb mesa-vulkan-drivers libxkbcommon-x11-0`):

```bash
xvfb-run -a cargo sandbox --paused --port 0 --port-file /tmp/shards.port &
SHARDS_PORT_FILE=/tmp/shards.port cargo shards wait && cargo shards shot
# visibility does not depend on the camera: after a step with movement, two observes in a row give the same
# SEEN and NOT IN VIEW, though the camera is still catching up with the player (only scr(x,y) changes)
cargo shards act move dir=W meters=9 && cargo shards step 300
cargo shards observe; sleep 1; cargo shards observe
```

### Agents

An agent is an external coding agent (Claude Code, Codex, …) with a terminal. It needs neither an SDK nor tool definitions: it
calls `shards` through bash and reads the output as text.

```bash
cargo sandbox --headless --paused &      # the game holds still while the agent thinks
cargo shards wait
cargo shards observe                     # what is visible
cargo shards tools                       # what can be done (--json: with argument schemas)
cargo shards act attack target=e6 repeat=true
cargo shards step --until journal:winds  # in the reply: CHANGES and JOURNAL
cargo shards shot                        # a visual bug: PNG with labels and a legend (needs a window or --offscreen)
```

- Time stands still while the agent thinks: when paused, ticks run only on `step`.
- By default, an agent has the scope `Dev`. To check how the game plays fairly, switch it to `Player` (`shards session
  --scope player`): then it acts only through `Intent`, as a human does.
- Each agent has its own name (`--agent`/`SHARDS_AGENT`), and with it its own journal cursor, scope and "body". Agents can work
  with one game, or each with its own headless instance on its own port.
- A bug you find is saved as a replay (`shards save-replay`), and a human or CI reproduces it.
- A rule checks a guess about a bug: `shards assert add NAME COND`, then `step` or
  `shards fuzz`. The game stops on the tick of the violation and saves the replay itself (see "Invariants (E11)").
- The engine does not run models or call their APIs. Everything an agent needs is added as a tool
  or a `shards` command, not as code that thinks for the agent.

## Scenarios (E11)

A scenario is a text file: `shards` commands, expectations and invariants. The same file runs in `cargo test` without a window, and against a
running game (`shards scenario FILE...`). Its meaning is the same either way: the format and all the rules (what a command does, how an expectation is checked, where `mark` counts from) are in `shards_script`, and the two runners differ only in their transport:
`ai_core::call_tool` and BRP `ai/*`.

```text
# A comment. Empty lines are skipped. Words are split as in a shell: "…", '…' (for JSON, wrap it in single quotes).
seed 42                                    # optional, as the first line: the game must run with this seed
act attack target=e6 repeat=true           # call a tool (as `shards act`); a call error fails the scenario
act? teleport id=e5                        # the same, but an error is allowed; its text is read by `expect result`
step 120                                   # N ticks (default 1; with --until, 600)
step 900 --until near:e5:e6:3 -u journal:died   # the same conditions as `shards step`
observe                                    # a slice of the world; its text becomes the "last result"
mark                                       # the journal for expect is counted from here (otherwise from the start of the scenario)
session --agent bot --scope player         # who runs the following commands; also --subject ID, --clear-subject

expect result contains "ok: attacking"     # text of the result of the last act/act?/step/observe; case matters
expect result !contains "error"
expect stopped "boss_phase_2"              # the last step was stopped by a condition whose description contains the text
expect journal "e6 phase 1->2"             # a journal entry from the start of the scenario or from `mark`
expect !journal "died"
expect field e5 Health.current > 0         # a number via `get`; operators < <= > >= == !=
expect shows e6 "phase 2"                  # what `inspect` writes about the entity above the component list

invariant field e5 Health.current > 0      # any expectation; checked after each following act/act?/step
```

- **Who does what.** Commands come from the agent `scenario` with the scope `dev` (`session --agent` switches it; the `--agent` flag of
  `shards` has no effect on scenarios). Expectations and invariants read the world as another agent, `scenario-check`, with the scope `dev`:
  a scenario can play as an agent with the scope `player` and still see everything, and the checks do not move the journal cursor of
  the agent being checked. Journal entries for agent calls (`[agent] …`) are not matched by `expect journal`: `step --until
  journal:died` will not find itself.
- **Case.** `expect result` is case-sensitive, so you can check the exact text of an error (`Expected: teleport(`).
  `journal`, `stopped` and `shows` are not case-sensitive, as with `--until journal:`/`shows:`.
- **Invariants** are checked after each following `act`, `act?` and `step` command, not between the ticks of one `step`.
  The first violation fails the scenario and names the command that caused it: ``invariant `field e5 Health.current > 0`
  (line 12) broke: e5 Health.current is 0, which is not > 0``. Split a long step into parts, or guard it with a condition: `step 600
  --until field:e5:Health.current<1`.
- **A step cut short** fails the scenario even though the `step` call itself succeeded: a gameplay system crashed (`stopped: gameplay system …
  panicked`), an invariant was broken (`stopped: invariant … violated`, see "Invariants (E11)"), or the ticks did not finish within 20 s of
  real time. A crash or violation that the scenario expects is allowed by `act? step …`.
- **`expect !journal`** (and the same invariant) fails if some entries since `mark` have been pushed out of the journal buffer and not
  read: the one you are looking for could have been among them. Set `mark` more often.
- **Parse errors** name the line: ``obstacle.scenario:12: unknown command `stpe`; expected act, act?, step, …``.
  All the bad lines are shown at once, and nothing runs while any of them remain.
- **The world is used as it finds it.** A scenario does not restart the game. So that a file runs both on a fresh game and after others, it puts the world in order with its first commands (`heal`, `teleport`, `set`, `npc_set_state … calm`, `regen`). All the
  `crates/sandbox/scenarios/*.scenario` files do this: they run on a fresh game and after others, forwards and backwards (CI checks both orders).
- **A scenario for a stand** (E7, E11.6) describes the host and the clients (`cargo sandbox --stand 2`). The first line (before or after
  `seed`) is a header `stand clients=N [latency=MS] [jitter=MS] [loss=P] [dup=P]` (N is 1 or 2). Any command can
  start with the participant it runs on: `@host` (by default), `@c1`, `@c2`. The `stand` scenarios live in
  `crates/sandbox/scenarios/stand/`, not next to the others: one game will not run them (`this scenario needs a stand of
  host + 2 clients: run cargo sandbox --stand 2 …`), while `scenarios/*.scenario` runs on a single game.

  ```text
  stand clients=2 latency=150 loss=0.02     # a 150 ms link each way (300 ms there and back), 2% loss
  seed 42
  @host act heal id=e15                     # a tool on the host
  @c1 act move_to target='{"x":2,"z":4}'    # the command of client c1 goes to the host, as a player's does
  step 300                                  # shared: 300 ticks of the stand, whoever issues it
  @c2 step 100 --until journal:swing        # the condition reads the journal of c2
  @c2 expect field e15 Transform.translation.z < 6   # expect/invariant apply to the participant named
  invariant !journal "desync"               # without a participant, on each of them
  @c1 session --agent bot --player 1        # agent bot plays the body of player 1, on c1 only
  mark                                      # without a participant, each one counts its own journal from here
  ```

  `expect result` and `expect stopped` read the last reply (the last `step`) of the named participant, and without a name, the last one
  overall. The agent that runs the commands is its own for each participant (`session --agent X` without a participant sets it for all,
  `@c1 session …` sets it for c1 only). An unknown participant is a parse error (`@c3: this stand has clients c1, c2`). A live stand
  must match the header: the same number of clients (otherwise `this scenario needs a stand of host + 2 clients, but this one
  is host + 1 client`) and the same link conditions as `--net-sim` (otherwise the error says what to start it with). In a test, the file
  runs on a stand built from its header: `shards::testing::run_stand_scenario(|app| { … }, path)`; against a live one,
  `SHARDS_PORT_FILE=F shards scenario crates/sandbox/scenarios/stand/*.scenario` (participants are found by `F`, `F.c1`, `F.c2`,
  and the host says how many clients the stand has, `ai/status.net.stand.clients`: a port file left by an earlier stand does not count as a participant;
  `--peer` and `SHARDS_PEER` have no effect for such a file, even `all` or `c2` when it does not exist: `shards` searches for the game of a one-game scenario only when such a scenario exists). A participant that stops answering is named in the report: `c2: cannot reach the game at …`, and an invariant check reports it with the invariant's line.

In a test:

```rust
let mut app = sandbox::test_app(42);
let report = ai_core::scenario::run_file(app.world_mut(), "scenarios/boss_fight.scenario");
assert!(report.passed, "{report}");   // the report is a line-by-line journal and a verdict
```

Against a game (`--quiet` prints only the failed commands and verdicts, `--json` prints the reports as JSON):

```text
$ shards scenario --quiet crates/sandbox/scenarios/*.scenario
PASS crates/sandbox/scenarios/boss_fight.scenario (50 commands, 422 ticks)
PASS crates/sandbox/scenarios/combat_prototype.scenario (215 commands, 2214 ticks)
PASS crates/sandbox/scenarios/dodge.scenario (74 commands, 591 ticks)
PASS crates/sandbox/scenarios/errors.scenario (38 commands, 2 ticks)
PASS crates/sandbox/scenarios/locale.scenario (61 commands, 0 ticks)
PASS crates/sandbox/scenarios/npc_director.scenario (38 commands, 1923 ticks)
PASS crates/sandbox/scenarios/npc_navmesh.scenario (31 commands, 1271 ticks)
PASS crates/sandbox/scenarios/obstacle_course.scenario (86 commands, 1010 ticks)
PASS crates/sandbox/scenarios/region.scenario (40 commands, 353 ticks)
PASS crates/sandbox/scenarios/saves.scenario (76 commands, 157 ticks)
PASS crates/sandbox/scenarios/settings.scenario (76 commands, 0 ticks)
PASS crates/sandbox/scenarios/snapshot.scenario (51 commands, 365 ticks)
PASS crates/sandbox/scenarios/ui.scenario (74 commands, 0 ticks)
13 of 13 scenarios passed
```

Without `--quiet`, each command is printed as a line `ok  39 step 900 --until near:e5:e6:3.2 → ran 129 ticks (150 → 279),
stopped: e5 within 3.2m of e6`. Exit codes: 0 means all passed, 1 means a scenario failed (`FAIL file:line: message`), and 2 means
the file cannot be read or parsed, or the game is unavailable. If the game disappears in the middle of a file, the remaining files do not run.

## Invariants (E11)

An invariant is a named rule that is checked after every tick, both in `step` and in real time. A rule that is broken
stops the game in the same way as a crashed system:

- an error in the journal: `invariant <name> violated at tick N: <text>`;
- in real time, the game pauses (`[sim] paused: invariant <name> violated at tick N`), and the next ticks of this frame
  do not run;
- the session is saved to `replays/invariant-<name>-tick<N>.json`;
- `step` stops on this tick and replies `stopped: invariant <name> violated at tick N (replay:
  replays/invariant-<name>-tick<N>.json)`. The reply has a `halted` field, and `shards` exits with code 3. A deferred step
  (`StepRoute::Deferred`) stops the same way.

The code sets the game's rules. A function gets `&World` and returns the text of the violation, or `None`:

```rust
app.add_ai_invariant("health_in_range", |w| /* Some("e6 Health.current = 700 outside 0..=600") */);
```

The sandbox has two of them: `health_in_range` (health is in `0..=max`) and `bodies_in_world` (every character is above the world: above the arena with its corridor, the obstacle course or a loaded region, with a margin of `margin`, and not below `fall_below`; the numbers are in `world/world.bounds.ron`, and the areas come from the built layouts and the region's manifest). A layout's area is not its overall box but the rectangle of each piece (`LayoutDef::footprint`: beams, ramps, volumes). Empty space next to the arena's corridor or the lane of the obstacle course is not part of the world, while a region stays one rectangle: `e7 outside the world: x = 200.0, z = 0.0`,
`e7 fell out of the world: y = -80.0`. No sandbox scenario, test or reference replay breaks them. A panic inside
a rule is a violation too: `the rule itself panicked: …`.

**The engine's own rules.** A game with all the engine modules gets ten rules of the engine, one or two from each module (a game that leaves out `npc` has no NPC rules). They are named
`<module>.<rule>`, checked before the game's rules, and listed by `shards assert list` as `engine rule`. Like a game's
rule, an engine rule cannot be removed, replaced or shadowed (`assert_add` and `add_ai_invariant` refuse its name), and
`assert_reset` re-arms it after it fires. A module registers its rules with `add_engine_invariant(name, rule)` from
`shards_core`; a game adds its own with `add_ai_invariant`, and the template shows one (`grunt_chance` in `src/ai.rs`).

| rule | holds when |
|---|---|
| `ai.index_consistent` | the `AiIndex` and the `AiId` components agree both ways (every entity with an id is in the index under it, every entry points at an entity that carries it) and the counts match, so no id is used twice. Tombstones are history and are not looked at. Catches an id written in place, which skips the hooks |
| `core.intent_sane` | every `Intent` has a finite `move_dir` of length at most 1 (+ 1e-4) and a finite `aim`. The movement code survives garbage, so the rule names the writer that forgot to clamp. The host cleans a network client's input before it reaches `Intent`, so a bad frame cannot stop the host |
| `motion.finite` | every `AiVisible` entity has a finite `Transform` translation and a finite rotation of length 1 ± 1e-3, and a finite `MotionState` (velocity, ground normal, timers, mantle, fall height, water surface, updraft, placed position). Scale is not judged |
| `voxel.body_in_solid` | **off by default** (`--engine-invariant voxel.body_in_solid`): no character has its capsule centre in a solid cell of a voxel field it collides with, except on the tick something other than the controller placed it (`MotionState::placed`). Off until fuzz runs on voxel terrain pass, since mantle and sloped-step ticks may graze a cell (see "Voxel fields") |
| `action.resources_in_range` | `Health`, `Stamina` and `Poise` are finite with `max > 0` and `0 <= current <= max`. Dead bodies are checked too (`current` is 0). A game that wants a buff above `max` raises `max` with it |
| `action.running_sane` | a running action of a fighter exists in content, its phase is one of the action's phases (an action with no phases has nothing to compare), and its clock is between 0 and its length plus one tick. The length is the run's own when that is longer than the definition's (miss recovery and `extend_recovery` lengthen a run, B2a). Without a content database the rule passes |
| `action.protection_sane` | every `Protection` (the reaction and poise timers, B1) holds finite timers, none below 0. A NaN stays a NaN through the countdown and would shield its fighter for ever |
| `action.barrier_sane` | every `Barrier` (B2b) holds a finite `amount >= 0` and a finite `left`. `grant_barrier` takes the larger pool and a positive time and `hurt` removes an empty one, so a violation names a writer that put a number there by hand (a NaN time never runs out, a negative pool would heal what it absorbs) |
| `action.damage_taken_sane` | every `DamageTaken` (B2b) holds finite multipliers `>= 0`, finite times and its keys sorted and unique. `set_damage_taken` inserts in key order and replaces a key; two entries under one key would multiply twice and make `clear` leave one behind |
| `action.knockback_sane` | every `Knockback` (a push in progress, B1) has a finite flat velocity (`y` is 0) and a finite `decel` above 0: a push that does not slow down never stops |
| `action.extras_follow_run` | every `RunExtras` (what a charging, turning or miss-recovering run keeps, B2a) belongs to the run its fighter is running (the same `instance`), with a finite `extra >= 0` and a finite `closed >= 0`. The machine puts extras in when such a run starts and takes them out on every way a run ends, so extras left behind would hand a later run a stale hold, hit or recovery |
| `npc.state_known` | `NpcBrain.state` is a state of the NPC's behaviour. Three carve-outs, each a state the engine itself leaves behind: an empty state (before the NPC's first decided tick), an inactive NPC (dormant or dead: the brain passes it through, so a state a hot reload removed stays until it wakes), and an NPC whose behaviour is not in content (that is reported once as a content problem) |
| `npc.strike_spacing` | two NPCs of one director whose `min_spacing` is set never began strikes less than that apart (in whole ticks), for the starts that went through the spacing rule: the planner's presses and checked combo continuations (`NpcStrike::gated`). Judged from the last strike each NPC noted. Three carve-outs: an NPC whose plan has `ignores_spacing` is not the one held to the rule (its starts still hold the others), a strike a state hook pressed itself is noted but neither held to the rule nor held against other NPCs (the hook may or may not have asked `strike_ready`, and may start between another NPC's planned press and its start), and an action with one of the director's `spacing_exempt_tags` is not noted at all. A director without a `min_spacing` is never judged |
| `npc.tokens_within_limit` | after the director ran, no token is held by a dead body, and each director holds no more tokens than its limit for the living players, unless every holder beyond the limit has its attack in flight (an attack under way keeps its token to its end). It is judged right after the director, not at the end of the tick: a holder that dies in `Resolve` keeps its token until the next tick's director takes it back, so the end of the tick would fire on every kill. A small system next to the director writes what it found to the resource `TokenAudit`, which the rule reads; it is recomputed every tick and is not part of a snapshot or a replay |
| `hitstop.no_ticks_inside` | no simulation tick has run since the real-time clock was stopped for a hit stop, and none ran inside a hold that has ended (`ClockHold`): a hold consumes zero ticks, and a violation names a system that ran the fixed schedule while the clock was held. Hit stop is the picture's, never the simulation's; alone with a picture only, since nothing else ever holds the clock |

Each rule reports the lowest id first and `(+N more)`, in the style `e6 Health.current = 700 outside 0..=600`, and
allocates nothing while the world is fine. The sandbox keeps its own `health_in_range` on purpose: it is the example of a
game rule, so a boss with 700 health breaks two rules, and the stop names the first, the engine's:
`stopped: invariant action.resources_in_range violated at tick 1 (+1 more)`.

**The switch.** `EngineConfig::engine_invariants` is true by default and false when the `dist` feature is on (a shipped game
must not pause on an engine rule; a game's own rules stay). `--no-engine-invariants` turns it off in a development build, for example to measure what the
rules cost. Test apps (`shards::testing::config()`), `--verify`, `shards fuzz` and `shards soak` run with the rules on. With
the switch off, no engine rule is registered and the token audit does not run. A rule that is off by default (`add_opt_in_engine_invariant`) is registered only when `EngineConfig::opt_in_invariants` also names it. A test that writes garbage on purpose to show
that the controller survives it sets `engine_invariants: false`.
In heavy mode the twin has its own engine rules and takes over which of them have fired, so a rule that fired in the original does not halt the twin again. The rules' cached query states (`RuleQueries`) are not simulation state and are not compared.

An agent adds its own rules for the length of the session (scope Dev), as a condition in the `--until` syntax. Real output:

```text
$ shards assert add hp 'field:6:Health.current<500'      # halt when the guard has less than 500 hp
added hp: the game halts when field:e6:Health.current<500
$ shards assert add calm error                            # … on the first [error] entry in the journal
added calm: the game halts when error
$ shards assert list
17 rules, checked after every tick
  ai.index_consistent  engine rule  armed
  core.intent_sane  engine rule  armed
  motion.finite  engine rule  armed
  action.resources_in_range  engine rule  armed
  action.running_sane  engine rule  armed
  action.protection_sane  engine rule  armed
  action.barrier_sane  engine rule  armed
  action.damage_taken_sane  engine rule  armed
  action.knockback_sane  engine rule  armed
  action.extras_follow_run  engine rule  armed
  npc.state_known  engine rule  armed
  npc.strike_spacing  engine rule  armed
  npc.tokens_within_limit  engine rule  armed
  hitstop.no_ticks_inside  engine rule  armed
  health_in_range  game rule  armed
  bodies_in_world  game rule  armed
  hp  when field:e6:Health.current<500  armed
  calm  when error  armed
$ shards act attack target=e6 repeat=true
ok: attacking e6 with light until it dies
$ shards step 600                                          # exit code 3
ran 230 ticks (0 → 230), stopped: invariant hp violated at tick 230 (replay: replays/invariant-hp-tick230.json)
ticks: avg 0.76 ms, max 3.55 ms (tick 108), budget 16.7 ms
…
JOURNAL
  …
  230 e5 -> e6 light_2 -22 (hp 486)
  230 [error] invariant hp violated at tick 230: e6 Health.current = 486
  230 [sim] invariant replay saved: replays/invariant-hp-tick230.json (play it with --replay to reproduce)
$ shards assert reset                                      # re-arm the rules that fired
re-armed hp
$ shards assert remove bodies_in_world                     # exit code 1
error: `bodies_in_world` is one of the game's invariants and stays; after it fires, `assert_reset` re-arms it
$ shards assert remove motion.finite                         # exit code 1
error: `motion.finite` is one of the engine's invariants and stays; after it fires, `assert_reset` re-arms it
$ cargo sandbox --replay replays/invariant-hp-tick230.json --verify    # exit code 1
invariant hp violated at tick 230: e6 Health.current = 486
replay finished at tick 230: identical to the recording (4 checks); 1 invariant violated [230 ticks in 0.46s]
```

- Rules skip the tick on which a gameplay system crashed: that tick is unfinished, and the crash has already stopped the game. The skip applies
  only to the tick that just ran: if you return to it from a snapshot and run the same tick without a crash, the rules are checked on it.
- A rule that has fired stays off until `assert_reset`. Otherwise it would fire on every tick until the world is fixed.
  `assert_reset` turns on all the rules that fired (the engine's and the game's too), and checking goes on from the next tick.
- `warning` and `error` fire on a journal entry of that level, including the game's `warn!`/`error!`. An entry about a violation
  of another rule does not set them off.
- A rule name is up to 64 letters, digits, `_` and `-`: the replay file name is made from it. An engine rule's name is
  `<module>.<rule>` in lowercase letters, digits and `_`. An engine or game rule cannot be removed or replaced with your own.
- `assert add/remove/reset` are written to the replay (`assert` events) and repeated on the same tick, so a replay
  saved on a violation breaks the rule on the same tick. The rules are not part of the hash or the snapshot. Loading a
  snapshot (and playing back from it) keeps the session's rules and re-arms the ones that fired. A recording from that tick starts
  with the `assert` events that add the session's rules, so a replay from a snapshot checks the same rules.
- `--verify` lists the violations that happened during playback, runs to the end of the recording, and exits with code 1 if there were any.
- One replay is written per violation, and only while a recording is running (a network client does not write it). Tests do not need it:
  `crash_replays: false` on `EngineConfig` turns it off too (or `AiInvariants.replays = false`).
- The rules are checked by whoever simulates the world: on a network client, `InvariantSystems` does not run (see "Groundwork for networking (phase C)").

**Fuzzing.** `shards fuzz --seed S --steps N` plays N rounds: each sends one random player input (or, with `--chaos`, a share of them make developer calls instead) and then runs `step` for 1–30 ticks (`--max-ticks`).
The same seed against the same fresh game gives the same calls. The loop lives in `shards_script::fuzz` and moves JSON through a
`Driver`, like scenarios and soaks: `shards fuzz` plays a live game over BRP, and a test plays the same run in-process
(`ai_core::scenario::WorldDriver`, `ai_core::scenario::tool_list`). It has no model in it.

- **Inputs from the tool schemas (`--alphabet schema`, the default).** The alphabet is every tool the game lists with scope `player` and
  kind `action` (`ai/tools`, what `shards tools --json` prints), sorted by name. The arguments are made from the tool's own
  `input_schema` by `shards_script::schema`: objects (every `required` property, each optional one half of the time), `enum` and `const`,
  `$ref` into `$defs`, strings, numbers and integers (inside `minimum`/`maximum` when the schema has them, else 0, 1, -1, 0.5 and 1e6),
  booleans, short arrays, `anyOf`/`oneOf`/`allOf` and nullable types. A property that takes an entity id is marked `"format":
  "entity_id"` in the schema (the engine's `Id`); it gets one of the ids `observe` lists, and one call in fifty an id nobody has
  (`e999999`). Free text is, half of the time, a word the game knows: the ids of its content (`content_list`, asked once at the start),
  and the names that refusals list (`unknown prefab …; known: skeleton, pillar`), learned as the run goes. The same seed, tool list and
  observations make the same calls; a unit test pins it.
- `--weights move=3,dodge=2` tilts the choice (the other tools weigh 1, weight 0 drops a tool; a name that is not in the alphabet is an
  error that lists the names).
- `--invalid P` (default 0.05) is the share of the inputs that break exactly one argument on purpose: a missing required property, a value of
  the wrong type, a value under its `minimum`, an unknown enum value, or arguments that are not an object. The game must refuse them and the
  error must say how to fix the call (`Expected: move(dir: string, meters: number)`); the final line counts how many it took anyway.
- `--alphabet classic` plays `move`, `attack`, `light_attack` and `dodge` with the exact random draws of the first fuzzer, so the seeds of
  old bug reports reproduce (a test pins the first calls). `--invalid` and the vocabulary belong to the schema alphabet.
- **Chaos (`--chaos P`, default 0).** That share of the rounds makes one developer call, or a pair of them, instead of an input, drawn
  from the seed: `snapshot_save` then `snapshot_load` of that file (`snapshots/fuzz-seed<S>.json`), `regen` of a region from
  `content_list kind=region`, `content_reload`, `save_game` then `load_game` in the slot `fuzz` (the player's slot is left alone), `pause`
  (a running game is paused and resumed; a paused one is only paused again, because a resume would let real time run ticks and the seed would
  no longer make the same game), `speed` and back, `heal`. The agent needs dev scope (the default `dev` has it). Nightly runs use `--chaos
  0.02`. A chaos call the game refuses is counted (`chaos_refused`), and the first one is named in the summary line (`; 2 chaos calls refused, the first heal {}: …`), so the count is never just a number. A round trip is different: if the first half worked and the second is refused (`snapshot_load` of the snapshot just saved, `load_game` of the save just made), the world does not come back as it was, and that ends the run as a finding: `fuzz seed S: a chaos call broke the game after N rounds (ticks A → B): snapshot_load {…} was refused: <the game's answer>`, the replay is saved as for a halt, and `shards fuzz` exits with code 3. A game that halts during a chaos call stops the run like any other halt. In the sandbox every chaos call works (a test asserts `chaos_refused == 0`); a control test plants a world fact that no snapshot holds (`add_snapshot_check`) and shows the run ends with the finding.
- The final line counts the inputs, the refusals (part of play: no stamina, busy, no such target), the calls broken on purpose and the chaos
  rounds. `--chaos` and `--invalid` must be shares from 0 to 1; a bad flag exits with code 2.

If a rule is broken, `fuzz` prints the `step` reply and the path to the violation's replay, and exits with code 3. If not, it saves the
session to `replays/fuzz-seed<S>.json`. Real output:

```text
$ shards fuzz --seed 1 --steps 50 --chaos 0.02
fuzz seed 1: 50 rounds, nothing broke (ticks 0 → 857)
player inputs 49 (36 refused, 3 broken on purpose, 0 of those taken), chaos rounds 1 (0 calls refused)
replay: /…/replays/fuzz-seed1.json
$ shards assert add low 'field:5:Health.current<50'       # in a fresh game
$ shards fuzz --seed 3 --steps 300                        # exit code 3
fuzz seed 3: the game halted after 72 rounds (ticks 0 → 1053)
ran 16 ticks (1037 → 1053), stopped: invariant low violated at tick 1053 (replay: replays/invariant-low-tick1053.json)
…
replay: replays/invariant-low-tick1053.json
$ cargo sandbox --replay replays/invariant-low-tick1053.json --verify    # exit code 1
invariant low violated at tick 1053: e5 Health.current = 42
replay finished at tick 1053: identical to the recording (18 checks); 1 invariant violated [1053 ticks in 1.50s]
```

**Steering, minimising and the regression scenarios.**

- **`--steer`.** With a game that keeps coverage (`--coverage`), each tool's weight grows with the new coverage signatures that appeared in the rounds it was called in: after every round the fuzzer asks `coverage since=<distinct>` (a query: it changes nothing and is not played), and credits every tool called in that round with the new signatures; the tools of the round before share half of that credit (a state is often a sequence: a heavy attack, then a dodge in its windup); credit fades by 0.5% a round, and a tool's weight is its `--weights` weight times 16, times one plus its credit (at most 16 times the base). A weight of 0 stays 0. Plain arithmetic, tools in name order, no clock: the same seed against the same game draws the same. Without coverage `--steer` stops at once and says how to turn it on.
- **Shrinking (`shards_script::minimize`, `shrink`, `stop`).** `minimize(items, still_fails, budget)` is Zeller's `ddmin` as a pure function over any list: split into n chunks, try each chunk, then each complement, double n; then a one-at-a-time pass while the budget lasts. It never asks about the empty list or the original, remembers the candidates it asked about (a repeat costs no attempt), and returns the smallest failing list, the attempts used and whether the budget ran out. `shrink` runs it over the rounds of a session (an input, or a chaos call, and the step that follows), then over the single calls and steps of what is left, then merges steps that follow each other; one budget (600 attempts in the campaign, 300 for `--minimize`) for all of it. "Still fails" is the **same stop** (`Stop::same_as`): the same invariant by name, the same failing system by name, a heavy check within 60 ticks of the first, or, for a chaos round trip, the same tool refused after its first half worked. A session that stops on anything else is not a failure of this kind.
- **In process (`shards::testing::fuzz_and_shrink`).** Plays the plan on the game `build` makes (a test app, or a heavy one); if it stops on a halt or a chaos round trip, every attempt plays the candidate rounds on a **fresh** game from `build` (the same seed, the same heavy mode) through `replays_stop`; the first attempt plays the whole session again, to see that a fresh game stops on it at all (`reproduced`). A wall-clock limit (the campaign gives 15 minutes) makes later attempts count as not stopping, and the result `exhausted`. `Finding::scenario(seed, heavy)` is the file.
- **Over BRP (`shards fuzz --minimize [--budget N]`).** Saves `snapshots/<stem>-start.json` before the first round; each attempt loads it into the same game, re-arms the rules (`assert_reset`) and plays the candidate. It needs a run that began at tick 0 (otherwise a fresh-game scenario could not keep the starting state, and it says so and stops). The game is not a fresh one, so the command prints a warning that the result can differ from a fresh game's, and the scenario is the in-process test's to confirm. The file goes to `scenarios/regress/` of the folder `shards` runs in (the game writes only to its own folders, so the CLI writes it). A session rule added with `assert add` is not in the file: only the engine's and the game's rules, which a fresh game has, reproduce there.
- **The scenario (`shards_script::regress`).** A header comment (seed, inputs before and after, the stop text, and for a heavy stop `# heavy: every=N span=K offset=O`), `seed S`, and the `act?` and `step` lines as played, nothing else. Calls are `act?` because most random calls are refused and a refusal is not the failure; the stop is: the broken rule halts the game inside a `step`, and a step cut short fails a scenario. For a chaos stop the refused call is a strict `act`, so its refusal fails the file. Files live in `crates/sandbox/scenarios/regress/` as `<YYYYMMDD>-<stop>-seed<S>.scenario`, fail while the bug exists, and are committed with the fix; `scenario_files.rs` runs each on a fresh game of the seed it names (in heavy mode when the header asks), forward only.
- **The nightly campaign (`crates/sandbox/tests/fuzz_campaign.rs`, `#[ignore]`).** Reads `SHARDS_FUZZ_SEED` (run `i` plays game seed and fuzz seed `SEED + i`), `SHARDS_FUZZ_RUNS` (50), `SHARDS_FUZZ_INPUTS` (2000 rounds), `SHARDS_FUZZ_HEAVY` (300 ticks, 0 = off), `SHARDS_FUZZ_CHAOS` (0.02), `SHARDS_FUZZ_BUDGET` (600 attempts), `SHARDS_FUZZ_SHRINK_SECS` (900), `SHARDS_FUZZ_MINUTES` (no run starts after that long; 0 = no limit), `SHARDS_FUZZ_OUT` (`fuzz-campaign`: the coverage report and the unreproduced sessions) and `SHARDS_FUZZ_REGRESS` (`scenarios/regress`). Every run uses `--chaos 0.02 --steer` with coverage on. It writes the shrunk scenarios, the sessions' replays (`replays/`) and one coverage report (`coverage.json` and `coverage.txt`: distinct signatures over all runs, the rarest, and the actions, statuses and behaviour states that **no** run reached), and fails if any run stopped. **Cost** (Windows 11, the repository's dev profile, a busy machine): one run of 2000 rounds (31 000 ticks) takes 15 s (release: 12 s) without heavy mode, 60-100 s (release: 50-80 s) with `SHARDS_FUZZ_HEAVY=300` and 3-7 minutes with 60 (debug): the cost is the twins (a game built again at each point, about 0.4 s), which a release build does not make cheaper, so the job runs the dev profile and its cache. So the nightly uses 300, 50 runs and `SHARDS_FUZZ_MINUTES=85`, which ends the job inside its 120 minutes on a slower runner.

## Tick time (E11)

Each run of `FixedMain` (one tick) is timed by the real clock, both in `step` and in a normal game. The timing wraps
`run_fixed_main` of its app, so all the systems from `FixedFirst` … `FixedLast` are included, the game's systems too, and several
games in one process each time their own. The last 600 ticks (10 s at 60 Hz) are kept in the resource `AiPerf`. The reply to `step` gets
a line about its ticks:

```text
$ shards step 600
ran 600 ticks (0 → 600)
ticks: avg 0.52 ms, max 2.60 ms (tick 540), budget 16.7 ms
```

- The default budget is the tick length: 16.7 ms at 60 Hz. Set your own with `--tick-budget-ms 5`, and in your own game with
  `EngineConfig { tick_budget: Some(Duration::from_millis(5)), .. }`. `step` counts the ticks that run over the budget on the same line:
  `…, budget 5.0 ms, 3 over budget`.
- Ticks over the budget go to the journal as a separate kind of `perf` entry, no more than once per game second. The first entry appears
  at once; later ticks pile up and go out as one entry when 60 ticks have passed since the last one: `150 [perf] 12 ticks over budget,
  worst 31.4 ms at tick 150`. It works the same way in real time, where there is no `step`.
- `ai/status` and `shards status` show the `perf` field for the last 600 ticks (`ticks` is how many of them have been recorded):
  `{"avg_ms": 0.522, "budget_ms": 16.667, "max_ms": 2.598, "over_budget": 0, "ticks": 600, "worst_tick": 540}`.
- The first tick takes longer than the rest: Bevy builds its schedules on it.
- Measurements are not part of the state hash, the replay or the snapshot: each run has its own timings, and playback checks the game.
- Test apps (`shards::testing::config`) print no `ticks:` line and write no `perf` entries (`report_perf: false`
  in `EngineConfig`): scenarios compare answers and journals in full. The measurement still runs, and `status` shows it. A test
  that needs the timing turns the output on itself (`AiPerf::report`), as `crates/sandbox/tests/perf.rs` does (it is a separate binary, so the other tests do not disturb its clock).
- A debug build is slower than a release build. Check the exact budget on a release build: `cargo run -p sandbox --release -- --headless
  --paused --tick-budget-ms 5`, then `shards step 600`.
- The [Tracy](https://github.com/wolfpld/tracy) profiler shows where the time goes inside a tick. The `trace_tracy` feature
  is in `shards`, `sandbox` and the template: it turns on the Bevy feature of the same name, and each system is its own zone.
  The sandbox with Tracy:

  ```text
  cargo run -p sandbox --release --features trace_tracy -- --headless --paused
  ```

  The game from the template: `cargo run --release --features trace_tracy`. While the game runs, open the Tracy GUI and connect
  to `127.0.0.1`. The GUI version must match the client that pulls in Bevy 0.20: in `Cargo.lock` that is `tracy-client` 0.19.0
  and `tracy-client-sys` 0.30.0 with Tracy 0.14.1 (protocol 82), so you need GUI 0.14.1. With `--paused`, ticks run on
  `shards step`, and the zones appear on them. The feature adds overhead, so measure the budget (`ticks:`, `perf`) without it.

## Frame time and the performance test (E10)

A game that draws (in a window or with `--offscreen`) also measures frames (`shards::frames`). A frame is one update of the main world
(`First` … `Last`: input, the ticks of this frame, the game's systems) and the frame of the render world for it (the `Render` schedule). Bevy runs the two
side by side, on two threads, so a frame takes as long as the slower of them. The extraction between them and the wait for the display
(the swapchain image under vsync) are not counted. Without a window, each frame waits until the GPU has drawn it, and that wait is counted.
A frame in which a call ran ticks on pause (`step`) is one the player does not see, so it is not counted.

`ai/status` and `shards status` show the `frames` field for the last 600 frames (without a window and a GPU, `null`):

```text
"frames": {"avg_ms": 2.33, "budget_ms": 16.667, "draws": 75.0, "draws_max": 75, "fps": 389.0, "frames": 600, "gpu_ms": 0.45,
           "main_ms": 1.26, "max_ms": 8.95, "over_budget": 0, "render_ms": 2.33, "triangles": 21421.0, "worst_frame": 3120}
```

- `budget_ms` is the frame budget: 1/60 s by default. Set your own with `--frame-budget-ms 8` or `EngineConfig::frame_budget`.
  `over_budget` is how many of these frames ran longer than it, and `worst_frame` is the number of the longest one.
- `fps` is the frames per second of real time between the first and the last of them. In a window with vsync, it is the display rate. Without a window, it is how many frames the GPU manages (a game that runs on its own, without a window, runs its loop at most 60 times a second).
- `main_ms` and `render_ms` are the averages for the main world and the render world. `gpu_ms` and `triangles` are the GPU time of the passes and the triangles that reach rasterization (shadows too), averaged per frame. Bevy's render diagnostics (`RenderDiagnosticsPlugin`: timestamp and pipeline statistics queries, on Vulkan and DX12) provide them, a few frames late. A GPU that does not measure them, and a `dist` build (which has no diagnostics), give `null`: WARP measures time but not triangles.
- `draws` and `draws_max` are the draw commands per frame: the average and the largest over those frames (`null` if they were not counted). The Bevy 0.20 engine does not keep a draw-call counter, so the render world counts them (`shards::draws`) from the phases that Bevy has already
  collected into batches and not yet drawn (after `PrepareBindGroups`, before the render graph), across all kinds of frame: for cameras and for the shadow maps
  of each light, summed. The binned phases (`Opaque3d`, `AlphaMask3d`, the depth and deferred passes, `Shadow`, 2D) give one command for each
  multi-draw set (`multidrawable_meshes`: how Bevy draws with GPU culling), for each batch (`batchable_meshes`: instances
  of one mesh with one material form one batch), and for each entity that cannot be put into a batch (`unbatchable_meshes`,
  `non_mesh_items`). The sorted phases (`Transparent3d`, `Transmissive3d`, `TransparentUi`, `Transparent2d`, which hold gizmos and UI) give
  one per item that starts a batch: Bevy's loop draws an item with a non-empty `batch_range` and skips the rest of
  the range, and the count works the same way. A multi-draw (`multi_draw_indexed_indirect`) is counted once, however many meshes it draws
  inside, so the number is much smaller than `visible_meshes`: in the reference scene it is 75 commands for 1395 drawn meshes, and forty
  cubes with forty materials of one pipeline add none. Not counted: passes that are not phase items (upscaling,
  tonemapping, bloom and other post-processing, each a full-screen triangle), compute work and copies. An item whose pipeline is not yet
  built is counted, although Bevy will skip it (during warm-up the number can be higher than what is drawn; the measurement runs after it). Without
  GPU preprocessing (no compute shaders: WebGL), the binned phases' sets are hidden inside Bevy, and there is no number: `null`, and the `draws`
  budget shows `-`. At a `render_scale` other than 1, the commands of the camera that shows the image are added to each frame (5 of them in the sandbox).
  The `draws` budget in `reference.perf.ron` is 150 (measured at 75–76). The `perf` definitions are part of the content hash a co-op session compares (it is computed from the definition's JSON), so a budget that was not set (`draws`, `nav_bake_s`) does not appear in the JSON: a new field does not shift the hash for those who did not name it. They are outside the replay check `content` (`SIMULATION = false`): a budget changed never makes a replay diverge.
- The `counts` field is always there: `entities` is every entity in the world (with `AiId`, the `entities` field next to it), `meshes` is the entities with
  `Mesh3d`, `visible_meshes` is the meshes that some view draws (a camera or a light's shadow map; a directional light sees
  the whole region), `camera_meshes` is the meshes of the active cameras, each camera with its own count: the draws of the picture before Bevy puts them into batches;
  `mesh_assets` is how many distinct meshes are among the drawn ones (instances of one mesh with one material are drawn by Bevy as one batch).
  Without a window nothing is drawn, and they are 0.
- Frames, like ticks, are not part of the hash, the replay or the snapshot, and they are not written to the journal.

**The performance test** — `--perf ID` works for any game built on `shards::run`. A content definition of kind `perf` (`content/**/*.perf.ron`, `shards::bench::PerfDef`) names the scene, the warm-up, the measurement and the budgets. The sandbox's reference scene is an 8×8 region with the 15 enemies from the spec and
foliage (E10.2, `region/wilds.scatter.ron`):

```ron
// crates/sandbox/assets/content/perf/reference.perf.ron (abridged)
{
    "reference": (
        region: Some((region: "wilds", seed: 42)),   // generate the region and move into it; loading is timed
        setup: ["god", "goto_tile x=5 y=5", "teleport id=e5 x=24 z=138", "spawn_prefab id=camp_skeleton x=17 z=141", …],
        size: Some((1920, 1080)),                     // image size for --offscreen; the game's window by default
        warmup: 120,                                  // ticks before the measurement (shaders, caches); 120 by default
        ticks: 600,                                   // measured ticks; 600 by default
        budgets: (
            frame_ms: Some(16.7),        // a frame longer than this is over budget (1/60 s by default)
            frame_avg_ms: Some(10.0),    // average frame, at most
            tick_avg_ms: Some(4.0),      // average tick, at most
            region_load_s: Some(15.0),   // generating and loading the region, at most
            entities: Some(20000),
            visible_meshes: Some(2000),
            mesh_assets: Some(80),       // distinct meshes among the drawn ones (how many batches), at most
            draws: Some(150),            // draw commands per frame on average, at most
        ),
    ),
}
```

Other budgets: `tick_ms` (a tick longer than this is over budget; the default is the tick length), `over_share` (the share of ticks and frames
allowed to go over budget; 0.01), `gpu_avg_ms`, `triangles`, `draws` (draw commands, on average per frame) and `nav_bake_s`
(seconds to bake the navigation of all the region's tiles for all body classes, as `--bake-nav` does, but without writing files:
`shards_npc::bake_region`; it is measured after the region loads, and only when the budget is named, because big tiles take a long time to bake; it is
not part of the region loading). A budget that is not set is not checked;
ticks (and frames, with `--offscreen`) always keep to their budget. The `setup` lines are the same as for `shards act` and the console. They run
in order, as the agent `perf`, and each must succeed; the performance test waits for any loading that a line starts. The definition is checked by
`--check-content`: a region that does not exist, an empty line, a budget not above zero, `tick_ms` and `frame_ms` values that a duration cannot hold
(under 1 ns or too large), more than 216000 ticks of warm-up or measurement (an hour at 60 Hz), a share outside 0…1,
and `region_load_s` or `nav_bake_s` without a region.

```text
$ cargo run -p sandbox --release -- --perf reference --offscreen
perf reference: release build, offscreen 1920x1080 on NVIDIA GeForce RTX 5080 (Vulkan, DiscreteGpu, driver NVIDIA 610.88)
loaded in 0.15s, region wilds seed 42 in 0.06s of it
600 ticks in 10.00s: avg 0.56 ms, max 3.20 ms (tick 480); 3277 frames, 327.8 fps: avg 2.85 ms, max 4.50 ms (main 1.47, render 2.84, GPU 0.49 ms), 60322 triangles, 76 draws; 16266 entities, 14669 meshes, 1392 drawn (12 distinct), 1083 in cameras
ok    region load 0.06 s (at most 15.0 s)
ok    tick avg 0.56 ms (at most 4.00 ms)
ok    ticks over 16.7 ms: 0 of 600, 0.0% (at most 1.0%)
ok    frame avg 2.85 ms (at most 10.00 ms)
ok    frames over 16.7 ms: 0 of 3277, 0.0% (at most 1.0%)
ok    draws 76 a frame (at most 150)
ok    entities 16266 (at most 20000)
ok    visible meshes 1392 (at most 2000)
ok    distinct meshes drawn 12 (at most 80)
perf reference: all 9 budgets met
```

This is a release build with foliage (about 7200 instances, 13 900 mesh entities) and effects (E10.3), on 17 October 2026:
328 frames per second at 1080p, no frame longer than 16.7 ms, GPU 0.49 ms per frame, 76 draw commands for 1392 drawn meshes
(the `draws` budget is 150; the same scene on the build before the broad-phase physics change: 330 frames, the same 76 commands and 12 distinct meshes; earlier
measurements below gave 75 commands and 49 distinct meshes). The build without `render_scale` and `draws`, and the new build, were run in turn from copies of the exe on a quiet machine, three pairs:
330.9, 331.6 and 314.6 frames per second against 329.9, 327.2 and 318.9; the worst frame 4.1–4.8 ms against 4.5–6.4 ms; GPU 0.49 ms in both: no difference can be seen (one measurement on a loaded machine gave 248). The sandbox's particle pools at the time of measurement hold 400 hidden meshes (the `max_live`
of its five effects; with the sixth, `step_dust`, there are 448). They count in `entities` and `meshes`, but not in `drawn`: only live particles and telegraph decals are drawn, and the decals only while there is an attack zone on the ground. For the decals, the camera also draws a depth prepass (`DepthPrepass`). Without effects, with foliage alone, the same
scene gave 379 frames per second (15 407 entities, 1364 drawn meshes from 48 distinct ones, 52 481 triangles); before foliage,
389 (1132 entities, 217 drawn meshes). The measurement ran with 0–1 `rustc` processes from other branches on the machine; a repeat with 1–2 gave 313.6 frames
per second, worst frame 7.98 ms, all 8 budgets met. The RTX 5080 is not the spec's "average PC", and the scene has not been measured on one. The margin on the average frame is almost sixfold. The debug build measures but does not judge: its lines show `-` (with foliage, before effects, it gave 342 frames per second).

**The full-size scene: `reference96`.** The spec (E5) builds a region of 8×8 macro tiles of about 96 m each, and `wilds` is 8×8 tiles of 16 m (128 m on a side, about 230 colliders). `perf/reference96.perf.ron` is the sandbox's `expanse` region: 8×8 tiles of 96 m
(768 m on a side) made from its four own tiles (`region/expanse.tile.ron`, `tile_size: Some(96.0)`: a gate in the middle, meadows with
thirty boulders, groves of 36 columns, walled yards; about 1900 colliders), three camps no closer than 150 m to the gate, its own
foliage (`region/expanse.scatter.ron`, twenty times sparser than in the wilds: see "Active zones and streaming"), the player at the gate
and twelve skeletons nearby: 15 enemies, 1080p. Besides the tick and the frame, the scene measures the region loading (the generator, the tiles, the stitched
navmesh of both body classes, 2.3 million nodes each) and the baking of the navigation of its four tiles for both classes (`nav_bake_s`: 192×192
columns per tile, eight files in `assets/nav/` of 1.4 MB each, about 0.1 MB in git once compressed).

```text
$ cargo run -p sandbox --release -- --perf reference96 --offscreen
perf reference96: release build, offscreen 1920x1080 on NVIDIA GeForce RTX 5080 (Vulkan, DiscreteGpu, driver NVIDIA 610.88)
loaded in 0.69s, region expanse seed 42 in 0.61s of it; its navigation baked in 0.80s (8 tiles x body classes)
600 ticks in 10.00s: avg 0.63 ms, max 7.34 ms (tick 540); 3537 frames, 353.7 fps: avg 2.61 ms, max 8.79 ms (main 1.60, render 2.59, GPU 0.44 ms), 116159 triangles, 62 draws; 34976 entities, 31609 meshes, 1985 drawn (13 distinct), 1580 in cameras
ok    region load 0.61 s (at most 3.0 s)
ok    nav bake 0.80 s, 8 tiles x body classes (at most 4.0 s)
ok    tick avg 0.63 ms (at most 2.00 ms)
ok    ticks over 16.7 ms: 0 of 600, 0.0% (at most 1.0%)
ok    frame avg 2.61 ms (at most 8.00 ms)
ok    frames over 16.7 ms: 0 of 3537, 0.0% (at most 1.0%)
ok    triangles 116159 a frame (at most 300000)
ok    draws 62 a frame (at most 150)
ok    entities 34976 (at most 45000)
ok    visible meshes 1985 (at most 3000)
ok    distinct meshes drawn 13 (at most 40)
perf reference96: all 11 budgets met
```

Four runs on a quiet machine: 351–354 frames per second, average frame 2.61–2.62 ms, worst 8.7–9.0 ms; tick 0.63 ms on average,
worst 7.2–7.6 ms; loading 0.61–0.62 s, baking 0.79–0.80 s. Without `--offscreen`: tick 0.46–0.52 ms, loading 0.61–0.65 s. The same
build, where physics checks every collider on every query as it did before the broad phase (inserted for the measurement): loading
3.9 s, tick 0.93 ms without a picture; with `--offscreen`, 1.13 ms on average and 11.9 ms at worst, 317 frames per second. The budgets have a margin
over the measurement: loading 3 s (the spec allows 10–15 s, but 3 s is already a regression for this scene: it took that long without the broad phase),
baking 4 s, average tick 2 ms, average frame 8 ms. There are only 13 distinct meshes: the sandbox draws each box as one cube,
each ball as one sphere, and the foliage with the shared meshes of its levels of detail.

- Without `--offscreen`, the performance test measures only the ticks and runs them as fast as it can (the warm-up and the measurement each take one `step`). The frame budgets
  show `-` ("nothing is drawn headless"). With `--offscreen` the game runs in real time, 60 ticks per second, and the frames
  come as fast as the GPU draws them, without vsync. The measurement counts exactly its own frames: warm-up frames that finish drawing during the measurement are not counted in it, and the game waits on pause for the last frames of the measurement without advancing any ticks. GPU times do not come with their frame number: the report arrives 1–8 frames after its frame, so the measurement takes the reports that arrive from its 8th frame to the last one. They are certainly about its frames, not about the warm-up or the frames after the measurement. The same measurement on WARP (`WGPU_FORCE_FALLBACK_ADAPTER=1`) gives 0.9 frames per second
  at 1080p, two frame budgets are exceeded, exit code 1.
- Exit code: 0 — all budgets met; 1 — one is exceeded, or the game did not reach the end of the measurement (it did not load, a `setup` line failed, a gameplay system crashed, an invariant was broken, or the ticks did not finish within four times the expected time plus a minute); 2 — there is no such definition (with `did you mean`), or `--offscreen` has no GPU.
- A debug build measures but does not judge: its lines show `-`, the exit code is 0, and the last line says that the budgets are for `--release`.
- The performance test writes nothing: no settings, replays or crash reports. It has no `shards` server. It cannot be combined with `--host`, `--join`,
  `--stand` and other checks (`--smoke`, `--verify`, …).
- From a test: `shards::bench::play(&mut app, &def)` on a test app (without a window, or with `offscreen`) returns `Measured`, and
  `shards::bench::judge` returns a line per budget (`crates/sandbox/tests/perf.rs`: both scenes without a window, `reference96` with the baking of
  its own tiles; the measurement with `offscreen` is under `#[ignore]`: it needs a GPU).

## Developer menu and console (E11)

The `shards_dev` crate. Everything the developer can reach in a window, an agent reaches through `shards`, and the other way round: the menu and the console
call the same tools through `call_tool`. In a window they call them on behalf of the agent `console` (scope Dev), so their calls
are visible in the journal (`[agent] console: regen {"region":"wilds","seed":7}`) and go into the replay, like the calls of any agent. The `dev_menu` and `console` themselves are developer queries: only the calls they make go into the replay.

**The menu** (F10 in a window; the `dev_menu` tool) — entries grouped by section. The engine's modules register their own entries for their own
tools (`add_dev_entry`): time (pause, step, slow motion, snapshots, the error report), the world (the region and its seed, a check of this seed
with the generator and of 200 such regions, colliders, tiles, mesh looks and their outlines, foliage, effects), combat (immortality, healing, hitboxes, telegraph decals), AI (NPC minds, navmesh), networking
(links, simulated lag), saves (a check). After them come the game's own entries from its content. A switch shows the line for its other
state (`npc_overlay on=false` while the overlay is on). An entry that needs a value puts its line in the console for you to finish in a window, and the tool asks you to run it with values. An entry with nothing to act on right now says why:

```text
$ shards act dev_menu
dev menu: 37 entries (F10 in a window); `dev_menu run=<id>` runs one, `console line=...` runs any line
time
  pause            Pause / resume                          resume
  step             One tick (while paused)                 step ticks=1
  slow             Slow motion x0.25                       speed x=0.25
  …
world
  region_check     Check this seed: rules, reachability    (not here: no region in the world: make one with `regen region=<id>`)
  goto_tile        Go to tile...                           goto_tile x= y= (finished in the console)
  …
combat
  god              Immortality on / off                    god on=true
  …
sandbox
  boss_phase_2     Warden, phase 2 (snapshot)              snapshot_load path=snapshots/boss_phase2.json
  wilds            The wilds, seed...                      regen region=wilds seed= (finished in the console)
$ shards act dev_menu run=god
e5 is invulnerable
$ shards act dev_menu run=goto_tile
error: `goto_tile` needs values: run `goto_tile x= y=` with them filled in (`console line=...`; in a window the menu puts it in the console)
```

In a window the menu is on the right: arrow keys and Enter, or the mouse (hovering selects, a click runs). The wheel and the arrow keys scroll a long
list, and F10 or Escape closes it. The entry's reply stays in the console.

**The console** (the key to the left of 1, `` ` ``; the `console` tool) — a line, typed the way `shards act` takes it: `god`,
`speed x=0.25`, `teleport id=e5 x=3 z=4`, `move_to target='{"x": 3, "z": 4}'` (JSON goes in single quotes). The reply is the result
and the journal of the call; an error says how to fix the line. `help` shows a hint, `clear` clears the console, and `tools` lists the tools.
In a window: Tab completes a tool name, and after it the argument names from the schema (it does not suggest the ones already named, or complete values;
if there are several options, it shows them), the up and down arrows bring back earlier lines, and Escape closes it.

```text
$ shards act console line="speed x=0.25"
speed 0.25x
  0 [sim] speed 0.25x
$ shards act console line="teleport id=e5 x"
error: expected key=value, got `x`
```

While the console or the menu is open, the `dev` input context sits on top: the character ignores the keys, and what you type goes to the console (the engine overlay keys F2, F6 and F7 wait until the panel closes: `ai_core::dev::keys_free` is the condition for such systems; `shards::dev::has_keyboard(world)` tells the game that the keyboard is busy, and this way the sandbox does not turn on its own F keys). The resource `shards::dev::DevKeys` sets the keys (feature `render`; `None` means no key). `console open=true` and `dev_menu open=true` show the panel in the window, the same as the key does (`false` hides it), so `shards shot` can capture them.

**Game entries** are content of the kind `devmenu`: `run` is the string the entry runs; `fill` is the string it puts in the console for you to finish. Use one of them. `group` is the section (default `game`). A game entry with the same id as an engine entry replaces it. A broken string is a content error: ``devmenu `quote`: run: unclosed `'` quote``.

```ron
// content/dev/sandbox.devmenu.ron
{
    "boss_phase_2": (group: "sandbox", label: "Warden, phase 2 (snapshot)", run: "snapshot_load path=snapshots/boss_phase2.json"),
    "wilds": (group: "sandbox", label: "The wilds, seed...", fill: "regen region=wilds seed="),
}
```

An engine module or the game adds an entry in code with `app.add_dev_entry(group, id, label, command)`, where `command` is `DevCommand::Run` (a string), `DevCommand::Fill` (a string for the console), or `DevCommand::Live(fn(&World, &Caller) -> Result<String, String>)`: a string built from the state of the world (the toggle is `ai_core::dev::toggle_line`), and `Err` gives the reason the entry does nothing right now. Entries are sorted by section, in the order they were registered. The game's sections come after the engine's sections.

## Replays

Recording always runs from tick 0 and costs almost nothing. It records:

- **inputs**: components registered with `replay_input` (the engine registers `Intent` itself; the game adds its own). Every change made *between* ticks (keyboard, network client, agent tool) is saved, with the tick it belongs to. Changes that gameplay systems make inside a tick are deterministic and are not stored. A component is cloned only when it has changed;
- **action calls** (`teleport`, `set`, `spawn_prefab`, `attack`, `content_apply`, `regen`, `region_load`, `goto_tile`,
  `spawn_level`, `level_reload`, `npc_set_state`, …) along with the scope and the agent's "body". Calls made from inside another tool are not written: the outer call repeats them. `control` tools (pause, step, possess, content_reload, rebind) are not written. Raw edits through BRP (`world.insert_components`) bypass the tools and are not written either. A live content edit is written as `content_apply` on its own tick;
- **rules**: `assert_add`, `assert_remove`, `assert_reset` with the tick they were called on (see "Invariants (E11)");
- **checks**: a hash of the game data every `check_every` ticks (60 by default) and at the end of the recording. For each entity with `AiVisible`, in `AiId` order, it covers the entity's id and all reflectable game components (serialised through reflection, components in type-name order). It also covers the values of `add_replay_check` (`content` is one of them: the definitions the session's simulation read, see "What the content check covers"). Next to the hash is its breakdown, computed in the same pass: the hash of each component of each entity, and each `add_replay_check` value under its own name. The report uses it to say what exactly diverged.

```bash
shards save-replay                          # or ai/act {"tool": "replay_save", "args": {"path": …}}
shards save-replay replays/boss-fight.json  # your own name, but only inside replays/
cargo sandbox --replay replays/tick332.json # window, real time; the seed and rate come from the file
cargo sandbox --replay replays/tick332.json --headless --paused   # then step through it with shards step
cargo sandbox --replay tests/replays/obstacle_course.json --verify
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 1.37s]
xvfb-run -a cargo sandbox --replay tests/replays/obstacle_course.json --verify --windowed  # in the window, 60 ticks per frame
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 3.00s, 22 frames]
```

**Reference replays** are in `tests/replays/` (a folder in git, unlike `replays/`): the obstacle course, the combat prototype, the token director (`npc_director`), the lair guard on the navmesh (`npc_navmesh`), region switching (`region`), a replay from a snapshot (`snapshot`) and saves (`saves`), each played by its scenario in a fresh game. The sandbox test checks each file with the game itself (`sandbox --replay FILE --verify`), and CI also runs them in a window under Xvfb. The template keeps its own in `template/tests/replays/` (`smoke`, `valley`). If gameplay, or content they read, has changed on purpose, record them again (content they never read does not make them diverge): `cargo test -p sandbox --test sandbox -- --ignored record_reference_replays`
(for the template: `cargo test --test game record_reference_replays -- --ignored`).

- Game data follows the same rule as `inspect`: everything except the `bevy_*` types and the internal markers of `ai_core`, plus `Transform` and `Name`. So components that only the window adds (`Visibility`, `GlobalTransform`, meshes) do not change the hash, and an `Entity` inside a component is hashed as its `AiId`. Labels and describe fragments are text for the AI and are not part of the hash: a new wording or a new describer does not break old replays.
- The game sets what the hash covers: `replay_ignore::<C>()` leaves a component out (cosmetics, caches), and `add_replay_check(name, fn(&World)
  -> u64)` adds data outside `AiVisible` entities (projectiles without an id, resources). For state that may be absent, use `add_optional_replay_check(name, fn(&World) -> Option<u64>)`: while the function returns `None`, the hash is the same as without the check. This is how the levels module hashes `level`: a game without levels hashes as it did before, and the hash does not depend on whether levels were there earlier. A component that does not serialise is logged once (`replay checks leave out …`) and skipped after that.
- `replay_save` writes only inside `replays/` in the game's working folder (nested folders are fine). An absolute path, `..`, or a symlink that leads outside is an error with a hint: `path must be inside replays/, e.g. replays/boss-fight.json`.
- Playback needs a fresh world: tick 0, the same seed and the same tick rate. Events are applied between ticks, exactly where they happened. Inputs that are not in the recording (a person presses keys while watching) are rolled back. A replay started before content loads builds the world from the content it was recorded with.
- Each check is compared with the recording. The report names the first check where the game stopped matching, and what diverged there: `DIVERGED at tick 60: e6 Boss: Health differs` (more in "What diverged"). After a code change, this is a regression. `--verify` works for CI: exit code 0 means it matched and nothing was violated; 1 means it diverged, a gameplay system panicked, or an invariant was violated (the violations are listed above the verdict); 2 means the replay cannot be read or does not fit the game.
- At the end of a recording, the game becomes live: the replay works as a save point for branching. Any action call during playback takes over control and ends it (`replay interrupted at tick 130 of 332: dev took over
  (teleport)`).
- During playback, recording continues, so a branch is saved as one replay.
- The state of recording and playback is shown in `ai/status` (the `replay` field), and the result is written to the journal.
- There is one format: JSON `shards-replay/4` — `seed`, `tick_hz`, `check_every`, `check_detail`, an optional `start` (the snapshot the replay starts from), `end`, `events`, `checks`, and `check_parts` (what the `content` check was made of, as it changed: see "What the content check covers"); hashes are hex strings. Events are `call`, `input` and `assert` (rules). A check has `parts` besides `tick` and `hash`: `{"entities": {"e6": {"Health": "…", "Transform":
  "…"}}, "game": {"content": "…"}}`. Everything added to the format is optional. Version 4 is version 3 with `check_parts`, and this build plays `/3` files as they are, with the `content` check over the whole database (see "Older files" under "What the content check covers"). A `/3` file without `check_every` and `check_detail` (as the build before the breakdown wrote them) is read as checks every 60 ticks without a breakdown, and is checked by the overall hash. A divergence there names only the tick (`no details: the recording keeps hashes only (check detail none)`). The overall hash is computed the same way, byte for byte, at any detail level. This build also plays `shards-replay/2` replays (always from tick 0, with no rules, interval or breakdown) and checks them by the overall hash (`no details: replay format 2`). A `shards-replay/1` replay hashed text, and this build does not check it: `play` replies `this replay was recorded by an older
  build (…); record it again`, and `--verify` exits with code 2.
- A replay starts at tick 0 or from a snapshot: see "Snapshots (E11.4)". A replay from a snapshot keeps the same breakdown of checks.
- The reference replays (`tests/replays/` of the sandbox and of the template) are recorded with a breakdown by component: a `--verify` that fails in CI names the entity, the component and the value at once. Because of this the files are larger: the sandbox's are 66–392 KB, the template's 17–250 KB.

### What diverged

The report, the journal and the `--verify` output name the entities and components of the first check that diverged, and below them the current value of each such component in the live game, one line each. Real output: the replay was recorded with `--check-every 1`, then the argument of a recorded call was changed in copies of the file.

```bash
cargo sandbox --headless --paused --check-every 1 --port 0 --port-file /tmp/g.port &
cargo shards wait && cargo shards act set id=e6 component=Health path=current value=400
cargo shards act attack target=e6 repeat=true && cargo shards step 100
cargo shards act teleport id=e1 x=-19 z=-19 && cargo shards step 157
cargo shards save-replay replays/boss.json   # 257 ticks, 8 events, 258 checks, seed 42

# in the copy of the file, set puts health at 390 instead of 400
cargo sandbox --replay replays/boss-hp.json --verify
# replay finished at tick 257: DIVERGED at tick 1: e6 Boss: Health differs [257 ticks in 1.07s]
#   e6 now: Health { current: 390.0, max: 600.0 }

# in the copy of the file, teleport puts the pillar at x=-18 instead of -19
cargo sandbox --replay replays/boss-pillar.json --verify
# replay finished at tick 257: DIVERGED at tick 101: e1 Pillar: Transform differs [257 ticks in 0.64s]
#   e1 now: Transform { translation: Vec3(-18.0, 0.0, -19.0), rotation: Quat(0.0, -0.0, -0.0, 1.0), scale: Vec3(1.0, 1.0, 1.0) }
```

- The list can contain: `e6 Boss: Health, Transform differ` (components diverged, including missing or extra ones), `e15 missing (Skeleton despawned at tick 40)` (the entity is in the recording but not in the game; in brackets, what the index knows about its id), `e27 extra (Skeleton)` (in the game but not in the recording), `replay check weather differs` (the value of `add_replay_check`), `replay check content differs: action/light_1 differs` (a check with parts names them). The list shows no more than 10 entities; the rest are on one line: `… and 7 more`.
- A value is the debug output of reflection without module paths, no longer than 240 characters. Only the first divergence is explained. The later checks diverge because of it.
- The journal gets the error `replay diverged at tick 60: e6 Boss: Health differs` and one `replay: e6 now: …` entry for each value. The `replay.last_report` field in `ai/status` holds the same text.
- The check rate is set when recording: `--check-every N` for the game, or `check_every` in `EngineConfig` (60 by default). It is stored in the file, and playback checks on the same ticks. `--check-every` together with `--replay` is an error. At a check rate of 60, the tick in the report is the first check after the divergence. To find the exact tick, record the replay with `--check-every 1`.
- A replay from a snapshot diverges from the start tick if the game did not restore the snapshot exactly: the entities or the state hash do not match (for example, a replay check looks at state that is not in the snapshot). The header gives the reason and the first three differences. Real output: in a copy of a replay recorded after `load-snapshot`, the last digit of `start.state_hash` was changed.

  ```text
  $ cargo sandbox --replay replays/from-snap-inexact.json --verify
  replay finished at tick 120: DIVERGED at tick 70: no details: the start does not restore exactly: the state hash is a66f75973b37da0f, the snapshot's a66f75973b37da0e: a replay check covers state that is not on an entity with an AiId [120 ticks in 0.07s]
  ```

  The full text, with advice (`add_snapshot_resource`, `add_snapshot_rebuild`), is in the journal.
- Detail level: `--check-detail` or `check_detail` in `EngineConfig`. `components` (the default): a hash of each component; the report names the entities, components and values. `entities`: the hash of a whole entity; the report names the entities (`e6 Boss
  differs`). `none`: only the overall hash; the report names the tick (`no details: the recording keeps hashes only (check detail
  none)`). The overall hash does not depend on the detail level.
- Size: the replay above (the arena, 14 entities, a check on every tick) weighs 2.2 MB with `components` (about 8.6 KB per tick; at a check rate of 60, the same per second of game time), 183 KB with `entities`, and 19 KB with `none`.

### What the content check covers

The replay check `content` ties a recording to the content it was played with, but only to the definitions the session's
simulation read. Adding a definition, or changing one the session never read, leaves a recorded replay verifying; changing one it
read makes it diverge at the first check after the read, and the report names it. Real output, on copies of the sandbox's
assets (`BEVY_ASSET_ROOT` at the copy): the first with `light_1` hitting for 21 instead of 20; the second with a prefab added, the
health of a prefab the session never spawns changed, and the player's look recoloured:

```text
$ cargo sandbox --replay tests/replays/combat_prototype.json --verify
replay finished at tick 2307: DIVERGED at tick 300: replay check content differs: action/light_1 differs [2307 ticks in 1.34s]
$ cargo sandbox --replay tests/replays/combat_prototype.json --verify
replay finished at tick 2307: identical to the recording (39 checks) [2307 ticks in 2.18s]
```

What still makes a recording diverge: a change to a definition it read (by id, or a kind it read whole), a definition added to or
removed from a kind it read whole, a definition it read removed, a definition added under an id it looked for and did not find
(below), and of course any change to gameplay. What no longer does: a new definition of a kind it reads by id, a change to one it
never read, and any change to a kind outside the simulation (below). A kind read whole is a dependency of every session that reads
it so: the sandbox builds every layout there is, so `layout (all)` is a part of each of its reference replays and a new layout makes
all of them diverge (`layout/empty extra`), as it should, since the new layout is in their world; `tile (all)` is a part of
those that generate a region.

**What counts as read.** Each definition in the database carries a flag, and each kind a second one ("read whole"). A typed read
(`ContentDb::get`, `Content<T>::get`, `get_json`) sets the definition's flag; going over a kind (`iter`, `ids`, `ids_of`: the
generator choosing among all tiles, the sandbox building every layout) sets the kind's, and then every definition of the kind
counts, so a definition added to it counts too. A lookup of an id that does not exist sets nothing, and no part says it was made: a recording that
looked for an id while there was none diverges once there is one, because its playback finds and reads it. The engine looks up
its tunings this way every tick, so a game that defines no `npc_tuning` `default` and then adds one records its replays again, even
with the numbers the engine used without it (`npc_tuning/default extra`; the test
`a_definition_added_for_an_id_the_session_looked_for_and_did_not_find_is_extra`). The engine's
own readers look up by id what they need by id: a spawn reads its prefab and the prefabs it extends, the statuses tick with the
definitions of the statuses someone carries. A flag is set only while the **simulation's scope** is open (`ai_core::SimScope`, one
per world), and only for kinds the simulation depends on:

- *Open*: every fixed tick (`FixedMain`, around Bevy's `run_fixed_main`: the recorded calls too, which play back in `FixedFirst`);
  the body of every `action` tool call (live calls run between ticks, recorded ones inside a tick: both count the same); a change
  of `EngineState` (`OnEnter(InGame)` builds the world); and the builders, whatever calls them: the start region before tick 0,
  `change_region`, `spawn_region`, `spawn_level`. So the reads that build the world before tick 0 count. A prefab spawned anywhere
  inside these (a tick, an action, a region being built) counts.
- *Suspended* inside it, for what only watches: the invariants (engine invariants are off in a `dist` build and with
  `--no-engine-invariants`, so their reads must not count), coverage, replay and snapshot hashing, a snapshot load putting a state
  back, the digests of an action call (its `CHANGES`), the tiles' visuals streaming in, and the database's own work (validation and
  `refs`, `content_apply` checking an edit against what is there, a playback restoring the recording's definitions).
- *Closed*: everything else. Frames (`Update`, `PostUpdate`, the render world, a window's input), query and control tools
  (`content_list`, `observe`, `npc_state`), validation without a game (`--check-content`). Queries are not replay events: a read
  they made could never be repeated by a playback, so it never counts.
- *Kinds outside the simulation* (`ContentDef::SIMULATION = false`) never count, read anywhere: `look`, `effect`, `fx`, `hitstop`,
  `scatter`, `animset`, `sound`, `music`, `audio`, `theme`, `locale`, `devmenu`, `input_map` (a replay records `Intent`, not keys),
  `achievement`, `save` and `perf`. What only the picture, the sound, menus and services outside the session read. A game's own
  kind of that sort sets `const SIMULATION: bool = false;` in its `ContentDef`; one the simulation reads keeps the default `true`.
  `net` counts (the host spawns `player_prefab` for a joining player). `save` stays out because a playback writes no saves and
  never asks the autosave numbers (`debounce_secs`): if it counted, every recording that autosaved would list `save/world` and its
  playback would not (`save/world missing`); the test `autosave_numbers_never_change_a_playback` plays the saves replay on other
  numbers. `screen` counts only when a screen is pressed (`ui_click`, `ui_navigate`, `ui_set` are recorded, and the screen decides
  what a press does; the press finds its screen from the widget, not from its id); showing one reads it with `ContentDb::peek`,
  the read that never counts. Code that runs inside the scope and is not the simulation (a describer, a picture) reads with `peek`,
  `peek_ids` and `peek_iter`.

**Why the counted set is the same headless, offscreen and in a window.** It changes only inside the scope, and the scope holds
only what the recording and its playback both run, tick for tick: the fixed tick, the recorded action calls and the world they
build. What differs between those modes (drawing, a window's input, looks, effects, foliage, animation, the HUD, the tiles'
visuals, a person's or an agent's queries) runs outside the scope, suspends it, or reads kinds that never count. The checks:
`content_reads_are_the_same_headless_and_drawn` (an ignored test, it needs a GPU) plays one session headless and offscreen, with
frames between the ticks and some 14 700 meshes drawn, and compares what each counted, part by part; every reference replay
verifies headless (`--verify`) and plays identically in an offscreen game; CI verifies them in a window under Xvfb. Bevy's
multithreaded executor does not get in the way: the flags are atomics set once (a read after that only loads), and the scope
belongs to the world (a heavy-mode twin and a stand's players have their own), whose schedules run one at a time. A gameplay system
that panics does not leave the scope open: the engine's error handler takes the panic inside the schedule, the tick runs to its
end and closes it (`system_failures.rs` checks it).

**What the check does not see.** The parts are what the simulation read while the scope was open. A definition read with the scope
closed and used by the simulation later is not one: a read in `Startup`, in a system of a loading state, or a cache built when
content finishes loading. A change to such a definition does not show as `content differs`; what it changes in the world still
makes the replay diverge, through the entities' hashes, at a later check and without the definition's name. So a module whose
simulation depends on content reads it in the scope: in a fixed tick, an action call, or a builder that opens it
(`ai_core::scope::open`). The engine's readers do: a review build that logged every read of a simulation kind made with the scope
closed and never inside it, on every reference replay, found only the start-up and asset loading (`tile`, `room`, `level (all)`:
which files to load; what is built from them is read again, by id, when it is spawned) and a halting invariant (`bounds/world`, in
a suspension).

**What the replay keeps.** The check's value hashes its parts: `kind/id` → the definition's hash (FNV-1a of its kind, id and JSON)
for each counted definition, and `kind (all)` for each kind read whole, followed by all of its definitions. Next to `checks`, the
file has `check_parts`: for each check with parts, the parts that appeared, changed (`set`) or went (`gone`) at each check tick,
`{"tick": 60, "set": {"action/light_1": "…", …}}`. The set grows while the session meets new things and then stays, so the lists
are short (1 to 3 KB a file: 2.6 KB of the combat prototype's 372). A divergence of the check folds the lists up to its tick and names each part
that differs, is missing (counted in the recording, not now) or is extra (counted now, not in the recording): `action/light_1
differs`, `layout/empty extra`, no more than 10, then `… and N more`. A check whose recording keeps hashes only
(`--check-detail none`) still names them after its `no details` reason. Any module can make a check of this kind:
`add_parts_check(name, hint, parts, whole)` registers it for replays and snapshots at once.

**Content edits in a session.** `content_apply` (and `content_reload` and hot reload, which go through it) is a replay event: a
changed definition keeps its flag and gets its new hash, at the same tick in the recording and in its playback, which starts from
the recording's versions as before. A definition the edit adds or changes and the session never read stays out of the parts.

**Snapshots** compare the same parts. A snapshot stores the counted parts with their hashes (`check_parts.content`); a load
compares the loading game's versions of those definitions (others may differ) and refuses one that differs, naming it:
`content differs (action/light_1 differs): the content the session read differs from this game's: …`; a part whose hash does not
read is refused as a damaged file (``the snapshot's check `content` does not read: part …``). Then it sets the flags to exactly the
snapshot's, so the restored state hashes as the original did, a heavy-mode twin counts what the original counts, and a replay from
a snapshot plays in a game whose unread content has changed. A refused load puts the game's own flags back with its state. A
snapshot taken whole (below) makes the session hash every definition from the load on, as the session it was taken in did, and
leaves it no reads (none count then); a later load of a snapshot with parts makes it check by parts again. The recording restarts
at a load with the snapshot as its start, so its playback checks the same way. A refused load in a session that hashes every
definition leaves it so, with no reads.

**Co-op** keeps the hash of the whole database (`ContentDb::hash`): the host and a client compare it when the client joins (and a
Steam lobby shows it), because a client must be able to run anything the host may read later, not only what was read so far. The
desync check (`world_hash`) does not include replay checks, so nothing changes there.

**Older files.** The value of `content` changed its meaning, so the formats changed their version: replays are `shards-replay/4`
and snapshots `shards-snapshot/2`, each the version before with `check_parts`. A build from before them refuses such a file by its
tag (``not a replay this build can play: format `shards-replay/4`, expected `shards-replay/3` …``) instead of computing `content`
over its whole database and reporting a divergence that is not there (a crash report a player sends from a newer build to an older
checkout, say). This build reads the older versions (`shards-replay/3` and `/2`, `shards-snapshot/1`). A replay or snapshot without
`check_parts` (a file in one of those older formats; `shards-replay/1` is not checked at all, see above) was made when the content check hashed the whole database, and it plays, loads and verifies as
before: the game computes the check over the whole database for that session (and for the snapshots it takes and the branches it
saves), and says so in the journal (``replay: the recording does not list the parts of check `content`: …``); a divergence there
reads `replay check content differs (recorded whole: the recording lists no parts of it)`. So old recordings and crash reports keep
verifying with the content they were made with, and are recorded again only to stop depending on content they never read.

**Cost.** A read by id checks the definition's flag first (one atomic load, set already after the first read), then the scope (two
atomic loads); no lock. Going over a kind finds the kind's table and its "read whole" flag in one lookup, as it found the table
before. The parts are listed at each check (every 60 ticks), and each definition's hash is computed once, when it is loaded or
edited, instead of hashing every definition's JSON at every check as before. Measured on the combat prototype (release build,
`--replay … --verify`, six runs of each build in turns, on a machine busy with other builds): 1.02–1.33 s for its 2307 ticks
(median 1.18 s, 0.51 ms a tick), against 1.05–1.60 s (median 1.20 s) for the build before. The ranges overlap and the machine was
loaded, so this says only that the difference is within that noise, not that there is none; a clean comparison is the
`--perf reference` autotest of both builds, from copies of the programs, in turns on an idle machine.

## Snapshots (E11.4)

A snapshot is the state of the simulation at one tick, stored in a file. You can save it at any tick and return to it in this game or in a fresh one built the same way. After that, `step` runs the way the original session would have.

```bash
shards save-snapshot snapshots/boss_phase2.json   # or act snapshot_save path=…; without a path: snapshots/tick<T>.json
shards load-snapshot snapshots/boss_phase2.json   # or act snapshot_load path=…; --force — even if the game is different
```

"Go to the boss phase" is a snapshot taken in that phase and loaded by name: the scenario `snapshot.scenario` brings the guard to the second phase, saves `snapshots/boss_phase2.json`, takes the game anywhere, and one `snapshot_load` command brings the fight back to the second phase (the test `go_to_boss_phase_two_is_a_snapshot_loaded_by_name` does the same in-process). Captured snapshots are not committed to the repository (`snapshots/` is in `.gitignore`), and neither are `replays/`.

**What's in a snapshot.** Each part of the simulation state is one of four kinds:

- **saved**: all entities with `AiId` under their own id, and their reflectable components (of the game and the engine modules, plus `Transform` and
  `Name`; the `bevy_*` types that the window adds are not included); an entity reference inside a component is written as `e12`
  (`AiId`) or `c7` (a module key: a collider without an id, `add_snapshot_entity_key`); the tick; the `AiId` counter and tombstones;
  the resources a module registered with `add_snapshot_resource::<R>()` (`ColliderIds`, `LevelCounter` — the numbers of
  level instances, `RegionBuild` — the region in the world, `ContentEdits` — content edits that are not in the files,
  `PlacedLevels` — levels placed by id; see below);
- **rebuilt** after loading (`add_snapshot_rebuild`): caches derived from what was saved — `PhysicsWorld`,
  the remembered values of the replay recording's inputs, **the region's navmesh** (`navmesh`: built again for the region and the solid bodies
  that the snapshot brought back; the NPCs of a game loaded from a snapshot walk the same routes as in the original one);
- **checked** (`add_snapshot_check`, `add_parts_check`): what the world is built around. The snapshot stores hashes of the content the session
  read (each definition, `check_parts.content`), of colliders without `AiId` (level geometry) and of placed levels, and does not load where they
  differ: `content differs (action/light_1 differs): the content the session read differs …` (content it never read may differ; see "What the
  content check covers"). `force=true` loads it anyway;
- **not state**: settings (tools, hooks, registries), the journal, agent sessions (they name entities by id), and everything that
  only the window shows. The seed and the tick rate are in the header and must match (`start the game with --seed 7`);
  the engine version in the header must match too: a different minor version (`0.2` and `0.3`) is refused (`force` skips this check), and a different patch version only gives a warning.
  `ContentDb` is not saved: it is files; only the parts of the database that differ from the files are saved (see below).

**The region in a snapshot.** The world is built around the region, so it takes a special path: `add_snapshot_prepare(name, covers, fn)`
runs the module **before** the checks (after the seed, version, content and components), so that it builds again what it makes from
the saved resource. The checks from `covers` (`static_colliders`, `levels`) run after. The world saves `RegionBuild`: the canonical
region manifest, the first `ColliderId` and the level-instance number it was built with. On loading, the snapshot's region is built
again with the same ids, in a game where there is a different region or none. If the snapshot was taken without a region, the region in the world is unloaded.
A manifest that the content cannot build is refused before any change is made. The collider and level-instance counters
are set to the snapshot's values right after this: a level placed after loading gets the number of the original session, and a region tile
does not take the number of a level that the agent placed. The checks from `covers` can refuse once the region has been built again (other
level files, other geometry without `AiId`): then the load returns the world as it was before it (the same region, entities under the same
ids, counters, resources). The lines of the attempt leave the journal (and the old lines that they pushed out of the full
ring come back), and one line says what happened:
`snapshot of tick 332 not loaded (this game is not the one the snapshot was taken in): the world is as it was before the load`.
Replay recording goes on as if there had been no load. Real output: a snapshot taken in the wilds in
the middle of the lair guard's fight, loaded into a fresh game in the arena.

```text
$ shards save-snapshot snapshots/wilds_fight.json                  # game 1: region wilds, on tick 332 the guard hits the player
saved /…/snapshots/wilds_fight.json: tick 332, 30 entities, 4 resources
$ shards load-snapshot snapshots/wilds_fight.json                  # game 2: fresh, in the arena, no region
loaded /…/snapshots/wilds_fight.json: tick 332, 30 entities, 4 resources; region: wilds seed 42 v1 built again from its manifest (199 entities); the recording now starts at tick 332
  332 [sim] snapshot loaded: …/snapshots/wilds_fight.json (tick 332, 30 entities, 4 resources; region: wilds seed 42 v1 built again from its manifest (199 entities))
$ shards step 120                                                  # the same as in game 1
ran 120 ticks (332 → 452)
  ~ e25 Skeleton "Den Guard": +ground 1.6m/s; fight e5 · token · leash 9/10m → fight e5 · circle · leash 7/10m; -active claw 0.10s (zone cone 1.8m 100°); moved 1.8m S
```

**Content edits and levels placed by id.** Content is files, and the snapshot checks the hashes of the definitions its session read. But `content_apply` puts
definitions that are not in the files, and they are part of the session. `ContentEdits` lists the definitions where the database differs from
its files (the resource exists only while such an edit exists, so the snapshot of a game with no edits is the same file as before), and
the `content` preparation puts the snapshot's edits on top of the files of the game it is loaded into, before the `content` check (the game's own edits are dropped in the process).
Files that differ still cause a refusal. A hot reload of a file is not an edit: a fresh game reads
the same file. In the same way, `PlacedLevels` names the levels placed by id (`spawn_level`, `spawn_level_by_id`): the number
of the instance, the place, and the first `ColliderId`. Loading removes the levels placed by this game before the region is built
(a region tile may get the number of such a level), and places the snapshot's levels after it (`add_snapshot_prepare_after(name,
"region", covers, fn)`: the preparation runs right after the named one, whatever order the modules were added in), under their numbers and collider
ids. A load that was refused returns the previous ones. For a tile or level whose file the game has not loaded yet (an edit added it),
the load requests the file and refuses before any change: ``cannot put back the region the snapshot was taken in: its
tiles are built from the level file `tiles/annex.gltf`, which this game is loading only now: load the snapshot again in a
moment`` — repeat the load in a moment.
A level that the game places itself, not by id (`spawn_level_file`), is not part of the snapshot:
the `levels` and `static_colliders` checks require that it is also there where the snapshot is loaded. Each co-op machine keeps
its own edits and levels placed by id (`net_ignore_resource`, as with `LevelCounter`): the world a client joins a session with does not carry them.

State outside the entities with `AiId` that falls under none of the four kinds is the only trap. So loading
ends with a check: the world is saved again and compared with the file, and `state_hash` (with its `add_replay_check` values) with the hash
of the snapshot. A divergence is an error that names the first places that differ: `the restored state differs from the snapshot in 1 place(s): …` with
advice on what to register, instead of a run that would quietly go a different way. The world stays as it was: `snapshot_load`
returns the entities, tick, counters and resources (as with a refusal after the region is prepared), the replay recording does not
start again, `WorldRebuilt` is not sent, and the journal gets `snapshot of tick N not loaded (the restored state differs
from the snapshot in 1 place(s)): the world is as it was before the load` and an error with advice. Playback of a replay whose start
is restored inexactly, by contrast, goes on and names the divergence (`restore` returns a report with `differences`,
`restore_exact` — the same refusal, with the world returned). A component or resource that cannot be saved
(it is not reflected, not serialisable, infinite in `f32`, or does not read back) is named right away when saving
(`snapshot leaves out: …`) and in the file (`left_out`).

**Load** (`snapshot_load`, control tool, Dev): all entities with `AiId` are replaced by the snapshot's entities under the same
id, the tick, counters and resources are set, and caches are rebuilt. An error (another seed, content, an unknown component) is returned
before any change. Before that, the load also checks that the game can read each value in the file: a component or resource that cannot be read,
entity id `0` or `u32::MAX`, and an `AiId` written by hand (the engine sets the entity key; `bevy_*` types are built again) — refused.
`force=true` skips only what the game does not know, and such service records; it does not load a value that cannot be read; a refusal after the region has been prepared puts the world back as it was (see above). A replay recording after a load starts from this snapshot, and everything recorded before it is discarded
(`save-replay` before the load, if it is needed). A load during replay playback ends the playback.

**Replay from a snapshot** (`shards-replay/3` and `/4`, field `start`): `cargo sandbox --replay FILE --verify` loads the snapshot right after
the game loads, before the first tick, and then plays the events and compares the checks as usual. A snapshot that this build does not accept
(other content or version) makes it exit with code 2 and give the reason. The region, content edits and levels placed by id go into the snapshot (see above), so a replay of a session that edited content or placed levels before the snapshot plays in a fresh
game. Files that the replay needs (tile and level files that the edits of its start and its `content_apply` add) are requested by the modules as soon as the replay is set up: `add_playback_hook(fn(&mut World))` runs inside `replay::play`, before the first frame and tick. The module asks for the files and holds the loading gate under its own requirement (`tile levels: 1 level file left`
in the `loading` field of `shards status`) until they arrive. So a `content_apply` that adds a tile or a level, and
a `regen` or `spawn_level` in the same tick, replay the same way they went in the live session, and a replay set up in an already loaded
game (as in the tests) waits for its files until its start. Not included: a level that the game placed itself, not by id
(`spawn_level_file`), and what a content edit rebuilt outside entities with `AiId`; the `static_colliders` check stops such a load.

**Proof** (`crates/sandbox/tests/sandbox/snapshots.rs`): a session runs to tick N, a snapshot is taken there, and it runs on to tick M
(state A); a fresh game loads the snapshot, gets the same inputs and runs to M (state B). A and B are one state, down to the byte
(the full snapshot, `state_hash`), and the journal from N to M is the same. Checked in the middle of a dodge/roll, a combo, a telegraph, gliding (glide),
swimming, at 9 points of the fight with the Warden, when rewinding in the same game, and on a replay from a snapshot (`--verify`); for the world: a snapshot in the middle of a walk around the columns on the navmesh of the thickets, loaded into a fresh arena (`a_snapshot_in_the_generated_region_goes_on_the_same_in_a_fresh_arena`),
tile levels coming back under their own numbers, a dungeon in a game with another region, tokens, plans, routes and NPC perception over ten ticks of a fight, and a generated region with three NPCs on the navmesh and crates that are removed and put back (`shards_npc/tests/npc/snapshot.rs`).
Why not Bevy's `DynamicWorld`: it has the same reflection layer under it, but its entities are named by `Entity` bits, not by `AiId`; the order of component insertion depends on the archetype, while `AiId` must come before the hooks; and the file is not stable byte for byte.

## Heavy mode

The proof above checks the snapshot at the points a test picked. Heavy mode checks it all along a session: every `N` ticks the game saves its state S(t), plays on to t+K, and a fresh twin built by the same builder loads S(t), plays the same calls, inputs and session rules for (t, t+K], and must come out with the same S(t+K), byte for byte, and the same gameplay journal (its `Event`, `Warn` and `Error` lines; not those logged on Bevy's IO and async compute threads, which land in whichever game of the process is active, `JournalEntry::background`). State outside the snapshot (a resource nobody registered, a `Local` in a gameplay system, a cache that a load does not rebuild) makes the twin go another way, and the check names the first place:

```bash
cargo sandbox --headless --paused --heavy 30 --port 0 --port-file /tmp/g.port &   # a point every 30 ticks
cargo shards step 600
# with a counter on the player that no snapshot holds (the planted bug of the tests):
# ran 60 ticks (0 → 60), stopped: heavy check failed at tick 60: e5 Player: Planted differs: 1830 vs 930 (replay: replays/heavy-tick60.json)
cargo sandbox --replay tests/replays/combat_prototype.json --verify --heavy 30
# replay finished at tick 2307: identical to the recording (39 checks); heavy: 76 checked, 0 skipped [2307 ticks in 7.67s]
```

- **Flags.** `--heavy N` (a point every N ticks), `--heavy-span K` (the ticks a point plays before the twin is compared: by default N, at most 600; with K > N the points overlap) and `--heavy-offset O` (the first point, by default tick 0; a nightly run passes `seed % N`). The checks run between stepped ticks: heavy mode needs `--paused` (and `shards step`), `--replay FILE --verify` or an in-process `--stand N` (there the host is checked: the clients do not simulate). Without one of them the game does not start (`error: --heavy checks between stepped ticks: …`, exit code 2), and `resume` answers that this game is stepped. It does not go with a window, `--host`/`--join`, `--stand-udp` or the batch modes (`--check-content`, `--worldgen-report`, the bakes, `--perf`). `EngineConfig::heavy: Option<HeavyConfig { every, span, offset }>` is the same in code.
- **The twin** is built by the game's own builder (the function passed to `shards::run`) with the session's seed and tick rate, without a window, port, settings, menu or network, with its own save folder, and with its engine lines kept off the terminal. It starts from S(t) as a replay starts from a snapshot (`start`), so whatever a replay from a snapshot cannot do, a heavy check cannot either. Twin time is not tick time: neither the `ticks:` line nor the wall budget of `step` counts it. A point is skipped while the loading gate holds the clock, when S(t) cannot be saved, or when the journal lost lines of the point before they were compared, more in one tick than it holds (`heavy point at tick 120 not checked: …`, the `skipped` count); a snapshot load or a new recording drops the open points. Without a recording, or in a game that has no twin, heavy mode says so once (`heavy mode is on, but …: no heavy checks`).
- **A failed check** stops the game the way a broken invariant does: `step` answers `stopped: heavy check failed at tick T: <first difference> (replay: replays/heavy-tick<T>.json)` and exits with code 3 (`fuzz` and `soak` too), `--verify` lists every difference and exits with code 1. The journal has the error line `heavy check failed at tick T (a fresh game from tick t, N differences): …` and one `heavy: …` line per difference (ten at most). The files: `snapshots/heavy-tick<t>.json` (S(t)), `snapshots/heavy-tick<T>-original.json` and `-restored.json` (both states at T; compare them with any JSON diff), and `replays/heavy-tick<T>.json` (the session), with the line `heavy check saved: … (play it with --replay … --verify --heavy N --heavy-span K --heavy-offset O)`. The differences come from `ai_core::snapshot::differences(a, b)`: the header, entities by id (`e6 Boss: Health.current differs: 390.0 vs 400.0`), resources by type path, what the world is built around, and the state hash only when nothing else differs (then it is an `add_replay_check` value, and the line says so). After it has fired, heavy mode is off until `assert_reset`; `assert_list` ends with its line: `heavy: every 30 ticks (span 30, from tick 0): 2 checked, 0 skipped, fired at tick 60, off until assert_reset`.
- **In tests.** `shards::testing::heavy_app(configure, setup, HeavyConfig::every(30))` is `test_app` with heavy mode (the builder is the two closures, called again for each twin); `testing::verify_heavy(recording, configure, setup, heavy)` plays a replay with it and returns `HeavyVerdict { report, checked, skipped }`, or the failure.
- **Cost.** A check builds and loads a twin and plays K ticks again. Measured on the reference replays with `--heavy 30 --heavy-span 1` (so a twin plays one tick; Windows 11, 17 October 2026, with other builds running): building and loading a twin takes about 80 ms in the arena and 220–250 ms in a generated region (its navmesh is built again), in a release build and in the debug one alike (the cost is the build of the app and the load, not ticks); 30 ticks more add 7–15 ms. The combat prototype (2307 ticks) verifies in 0.7 s, and in 7.7 s with `--heavy 30` (76 checks); every reference replay of the engine together, in about 31 s (release) or 35 s (debug).
- **What it is not.** A save holds less than a snapshot (the body's place in a swing, an enemy's plan), so a game loaded from a save is not a heavy twin. Saves have their own check: at five ticks of each reference replay, `save_game what=all`, `load_game` in a fresh game and `save_game` again must write the same sections, byte for byte (`crates/sandbox/tests/sandbox/heavy_saves.rs`).

**The state audit** is the static half: after the combat prototype and the region replays have played to their end, every resource in the world (`World::iter_resources`) is either registered with `add_snapshot_resource`, or named in the audit's list as outside the simulation, with why (`crates/sandbox/tests/sandbox/state_audit.rs`: Bevy's clocks and render state, registries filled at build time, presentation, settings, transport, caches named with their rebuild). A resource that is neither fails the test with its type name: register it, or add it to the list with its reason. The reference replay test fails on `warning: replay checks leave out …`, which `--verify` prints for each gameplay component the checks cannot hash (it does not serialize through reflection; `ReplayHashing::reported_left_out`).

**Proof.** `crates/sandbox/tests/sandbox/heavy.rs`: the combat prototype verifies with `--heavy 300` (on every push), and three planted bugs on the player are each caught at tick 60 and pass once fixed: a counter resource (fixed with `add_snapshot_resource`), a `Local<u32>` every 50th call (fixed by reading the tick), a derived cache (fixed with `add_snapshot_rebuild`); a failed check halts, stays off and comes back with `assert_reset`. `crates/shards/src/run.rs`: a stand's host with a guest is checked, and a host-only counter halts the stand. Every reference replay of the engine and the template passes `--verify --heavy 30` (checked on 17 October 2026, Windows 11), and the nightly job `heavy` runs them all with `--heavy 10 --heavy-span 30` from the seed's offset (see "Tests and CI").
## Coverage

Which states has a session reached? Coverage answers it for a fuzz run or a soak, and tells a fuzzer which of its calls led somewhere new. It is test-only bookkeeping: off by default, on with `--coverage` (`EngineConfig::coverage`; `cargo sandbox --headless --paused --coverage …`). It only reads the world: the resource `AiCoverage` exists only when coverage is on, and it is not in the replay hash, the replay checks or a snapshot (heavy mode's twins keep it off). A game with coverage on records the same replay as one without; `tests/sandbox/coverage.rs` compares the two recordings, and the combat prototype's reference replay verifies with `--coverage`.

**Signatures.** After every tick each entity with `AiVisible` gets one signature: the **parts** modules contribute, joined in registration order (`add_coverage_part(name, |world, entity| Option<String>)`, a part returns `None` for an entity that has no such aspect). The engine's own: `kind` (`AiKind`), `motion` (the `MotionState` mode: `ground`, `air`, `water`, `glide`, `mantle`), `action` (`light_1:recovery`, or `idle`), `statuses` (the ids of the active statuses, sorted: `bleed,slow`), `stamina` (where it stands: `below_zero`, `empty`, `low`, `mid`, `high` or `full`, so that a session which drains it or overshoots it is a new state) and `npc` (the `NpcBrain` state). Each module registers its part in its own plugin. Example: `kind=Boss motion=ground action=slash:active stamina=mid npc=fight`. The resource counts every distinct signature (entity-ticks) and the tick it first appeared at; within one tick the entities' signatures are counted in sorted order, so a window that shifts `Entity` indices changes nothing.

**What content declares.** `add_coverage_catalog(label, part, declared, reached)` names a kind of thing content declares (`declared(world)` lists the ids) and how a part's values show it was reached (`reached(value)` gives the ids: `slash:active` reached the action `slash`). The engine registers `actions` (every `action` definition; `idle` is the value of a free fighter), `statuses` (every `status`, reached when it was active on some entity) and `behaviour states` (every state of every `behavior`). A state name shared by two behaviours counts as reached once any entity of either is in it.

**Tools.** `coverage` (query, dev scope; `shards coverage [--top N] [--since K]`):

```text
coverage: 51 distinct signatures, 32298 entity-ticks
distinct: 51
rarest 5:
  2x first at tick 436  kind=Player motion=ground action=block:lower stamina=low
  3x first at tick 1156  kind=Boss motion=ground action=shockwave_enraged:active stamina=full npc=fight
never reached, actions (7 of 23): claw, player_death, player_stagger, skeleton_death, skeleton_flinch, skeleton_stagger, slam
never reached, behaviour states (3 of 5): guard, home, idle
```

`json=true` answers as JSON (`distinct`, `entity_ticks`, `rarest`, `never_reached`, and `new_since` and `new` with `since`). **Steering:** `since=<distinct of an earlier answer>` adds `new since K: N` and lists the new signatures (the first twenty), so a driver polls it after a call and learns whether the call led anywhere new; the count is the driver's, the game keeps no cursor, so several drivers do not disturb each other (after a reset a stale count means "all"). `coverage_reset` (control; `shards coverage reset`) forgets it all. With coverage off both tools answer `coverage is off: start the game with coverage on (EngineConfig.coverage, or --coverage)`.

## Saves (E8.1–5)

A save is the player's progress, laid over the world that the game built as usual. It is not a snapshot: a snapshot (`snapshot_save`) holds the exact state of the simulation at one tick and is for the developer, while a save is a set of explicit structures that the game names itself. Nothing that is not named goes into the file. The crate is `crates/shards_save`, module `save` (`ai/status`). The umbrella crate exposes it as `shards::save`.

There are two kinds: **character** (each player's progress, kept by that player) and **world** (the host's). Each has slots (`slot1`, `ford-camp`: `[a-z0-9_-]`, up to 32 characters, not paths).

```rust
app.save_component::<Bag>(SaveKind::Character, "bag", 1)        // component on the player's body
   .save_component::<Opened>(SaveKind::World, "chests", 1)       // map SaveKey -> component, over the entities with SaveKey
   .save_resource::<Calendar>("calendar", 1)
   .add_save_section::<Quests>()                                 // a struct with capture / apply / missing
   .add_save_migration("bag", 1, |old| Ok(old))                  // JSON of version 1 -> version 2
   .autosave_on::<CampRested>(SaveWhat::All);                    // written at the end of the tick
```

- **Sections.** `SaveSection`: `NAME`, `VERSION`, `KIND`, `capture`, `check`, `apply`, `missing`. Sections are applied in the order they were registered. **`check` and `apply`.** `check(&self, &World, body)` (accepts by default) is the place for each refusal that must leave the world untouched: content that the manifest names, a body that must exist, a number outside its range. It gets `&World`, so it cannot change the world, and it sees the world as it was before the load (a refusal that depends on what the section does earlier stays in `apply`). All the `check`s of all loaded kinds run after all the files are read and before the first `apply`; a refusal means "nothing was applied", and no write call is made, however late in the load the section comes. `apply` and `missing` check nothing in advance: an error from them always means the section was running and the world may have changed (the load says so, restarts the write, and tells the co-op host that the world was rebuilt). `check` does not run for a section that is not in the file: `missing` has no data. `apply_sections` (a guest's character in co-op) also calls `check` before `apply`. A section unknown to the file gets `missing()`; one unknown to the game is skipped with a warning. A chain of migrations with a gap (a step missing from `add_save_migration`) crashes the app at build time, naming the section. The registry is the tool `save_registry`. The engine registers its own: `region` (world, version 2: the region, seed, generator version, manifest hash and the manifest itself; a world saved outside a region (in the arena) unloads nothing on load: the current region stays until `regen` or `change_region` replaces it, unlike a snapshot, which replaces the whole world and unloads the region; more below, "Region in a save"), `position` (the position and rotation of the body; in another region the body stays where it is, and the journal says why) and `vitals` (health, stamina, charges). A loaded `vitals` brings back to life a body that died after the save (`shards_action::revive`, as the dev-`heal` does, and the `Healed` event, so the game can undo what its `Died` did); a save with zero health leaves it dead (`check_deaths`).
- **File** `<save_dir>/<kind>/<slot>.save`: the first line is `shards-save/1 crc32=<hex> len=<bytes>`, then canonical JSON (keys in alphabetical order) with `{kind, slot, label, game, engine, tick, sections: {name: {version, data}}}`.
- **Writing** is atomic: `.tmp`, `sync_all`, rotation `.bak1`→`.bak2`→`.bak3`, `rename`, fsync of the folder. Three copies are enough to keep the three previous saves. A broken main file does not become a copy: it goes to `.damaged`.
- **Reading.** The main file is read. On any damage (checksum, truncated, not JSON, migration failed), the first good copy is read instead; the main file is rewritten from it, and the journal says: `save: world/slot1.save is damaged (checksum mismatch); restored from
  backup 1 (tick 50110)`. If all the copies are bad, the error names each one, and the world is untouched. A leftover `.tmp` (from a failed write) is removed.
- **A section fails during load.** The files are read, the sections are applied in order, and the one that returns an error stops the load. First the files are read, and every section of every kind is asked through `check`; a refusal there is `loading
  world/gone failed at section region: …; nothing was applied: fix the cause and load again`: nothing was applied (neither the earlier sections nor another kind), and no write call is made. Then the sections are applied in order, and the one whose `apply` (or `missing`) returned an error stops the load: it was running, the world may have changed, and the error says how much of the world is loaded: which sections of this kind applied or got `missing`, that the failed section itself may have changed the world, and which kinds loaded before it (`loading character/camp failed at section bag: …; applied before it: position, vitals; the
  failed section may have changed the world too; already loaded: world/camp. The world is partly loaded: fix the cause and
  load again, or go back with snapshot_load`; when nothing was applied before it, `no section before it was applied, but the
  failed section may have changed the world`); the write restarts from this world, and the co-op host sends the clients the world again.
- **A save from a newer version is not damage.** A section above this build's version, or an envelope `shards-save/2` (`Fault::Newer`), is refused with a clear error, `world/slot1 was saved by a newer version of the game (section inventory v7
  > v5); nothing was changed`: without falling back to an old copy (progress would be lost), without `.damaged`, and without overwriting. **Writing to such a slot is refused too**, while any copy of the slot is newer (`so this build does not overwrite it`): the rotation can neither rename the new file nor push it out through three saves. The way out is another slot or a new version of the game. This was chosen instead of "keeping it as a copy": the copy would be pushed out by the next three saves. `save_list` shows such a file as `newer`.
- **When.** `save_now(world, SaveWhat, reason)` writes at once (a reward, a find). `autosave_on::<E>(what)` and `SaveRequests::request` queue a request; it runs in `FixedLast` of the same tick, after the simulation, debounced per kind (`*.save.ron`, `SaveDef { debounce_secs }`). **On exit** (the `add_exit_hook` hook) the pending requests are written at once, without looking at the debounce; `flush_saves(world)` is for the game's own exit. A save never changes the simulation (a test compares the state hash with and without autosaves).
- **A replay writes no files.** While a replay plays (`--replay`, `--verify`), nothing goes to disk: not `save_now` from the system, not a game tool, not `save_game`, not the repair of the main file on load, not `save_damage`. The answer from `save_now` is `Ok` with `written: false` (the game's code sees the same as in a real write), the journal says `save: not written during replay
  playback (world/slot1, reward)`, and the tools answer with the error `save: not written during replay playback: …`. A replay checks the state, not the text of the answers, so it does not cause a divergence.

**Region in a save.** The `region` section (version 2) keeps the region's whole manifest, next to the id, seed, generator version and manifest hash (`{"active": {id, seed, version, manifest_hash, manifest}}`, `null` for a world outside a region). Loading builds the region from this manifest, and the generator does not run, so a game update that changes the generator (a new region `version`, other weights, steps) does not touch the saved world. The journal says where the region comes from: `region wilds seed 42 v1: built from the save's
manifest`, and if the generator version in the content is already different, also `(the generator is v2 now)`; if no region with that id is in the content any more, `` (`old_wilds` is no longer in the content) `` (the content still knows the tiles of its manifest, so it is built). Such a region is built from the manifest without what its definition gives: no wall around the edge, the default active-zone radius, and no navmesh for the region (journal: `navmesh: none for this region: content no longer describes every tile of its manifest;
NPCs plan on the local grid`).
The line is written once the region is built, after `region loaded`. If the same region is already in the world (the same hash), nothing is built.

- **No content.** A tile, a room or a prefab (a piece of a tile, a decoration, a prefab for a point of interest) that the manifest names, but that the content no longer knows, is a refusal before any change; the world stays as it was: `` section region: the save's world cannot be built: the
  manifest of wilds seed 42 v1 names tiles content does not have: old_ruin (it was made from other content, hash …); keep that
  content in the game, or write a migration for `region` that replaces it in the saved manifest ``. This is not damage: the copies are not tried one after another.
- **The manifest is wrong.** A manifest that does not match its own hash, or whose id, seed or version beside it is different, or that is of another format, makes the copy damaged (the section does not read): the load takes the first good copy, as with a wrong checksum, and the journal gives the reason (`save: world/slot1.save is damaged (section region does not read: the manifest of wilds seed 42 v1
  is damaged: it hashes to …, the save says …); restored from backup 1`).
- **Old saves.** Version 1 of the section did not keep the manifest. The `region` migration 1 → 2 gives it `manifest: null` and the mark `from_v1: true`, and such a region is generated anew from the seed, as before, and checked against the hash: if it comes out different, the load is refused before any change (`` wilds seed 42
  v1 comes out differently now (manifest … when saved, … now, generator v2): …; write a migration for `region` or keep the old
  content ``). The next save writes version 2 with the manifest. A version 2 section without a manifest (`null` or no field) and without this mark is damage to the copy, not an old save: `section region does not read: the save of wilds seed 42 v1 keeps no manifest;
  only a save of version 1 of the section may lack it`, and the load takes the backup copy.
- **Size.** The `region` section with the manifest of `wilds` 8 × 8 is 3151 bytes of canonical JSON; the whole world file is 3392 bytes (a test keeps them under 8 KB). Saves have no limit per section: 1 MB is the limit for a character, which travels over the network (up to 64 KB in `Hello`, more in parts after it), while the world stays with the host and reaches the clients as a snapshot.
- **Co-op.** The clients of a region do not generate it: the host, which loads the world, sends it to them as a snapshot with the manifest
  (`RegionBuild`), and the client builds the same region. The `RegionRef` of the `position` section is the id and seed of the region; a world built from the manifest names
  them as the saved one, so the character's position lands in it. There is no hash in it: a character saved in a world of an earlier generator version stands on its old coordinates in a world generated again with the same seed.
- A game that replaces the `region` section with its own (same name, from its own crate) starts its own migration history: the engine's migrations for `region` go away with its section,
  and the game registers its own, for the versions of its own section, after it.

**Launch.** `--save-dir DIR` is a folder inside the game's folder (not an absolute path, no `..`, like `replays/`; otherwise exit code 2),
`--world SLOT` and `--character SLOT` are the current slots that `save_now` and autosaves write to. A single-player game also **loads** them before the first tick, with a call to `load_game` (the `launch` agent; in the journal, `launch: load_game …`); the replay recording
starts from a snapshot of this state, so `--verify` passes without flags. If there is no slot, it is a new game, and the journal has a line
(`save: --world fresh: no world save in that slot yet, starting a new game`); the first save creates it. If the slot exists but cannot be
read (all copies are damaged or newer than the build), there is an `Error` in the journal; the game starts as it is and overwrites nothing.
The host loads them the same way, before the first tick, with calls to `load_game` (`shards::coop`; if it is stopped before the slots are
loaded, its exit does not write over them). A client loads nothing: the world belongs to the host, so it ignores `--world` and says so, but it reads its own character and brings it in `Hello` (see below). To set the load order yourself, use the hooks
`shards::save::load_launch_slot(world, kind, slot)` and `load_launch_slots(world)`.

**The main menu's "Continue".** The tool `continue_game` (player, action) leaves the main menu, like `new_game`, and loads the newest save of each kind into the game being started, the same way `--world` and `--character` do: the slot whose good copy was written last (by the file's time); a slot none of whose copies can be read is skipped. These slots become the current ones, and the replay recording starts from the loaded state. The reply: `the game starts at tick 0; loaded world/camp,
character/camp`. If there are no saves, it is refused and nothing changes, and the game stays in the menu: ``there is no save to continue yet (none in
saves): `new_game` starts a game, and saving it makes one``. `shards::save::newest_slot(world, kind)` reads the copies, `any_save(world)` only looks into the folder (a slot whose copies are all damaged also counts), and `can_continue(world)` says whether there is anything for `continue_game` to load: it reads the copies again only when the files in the folder have changed (name, size, time), so the menu can ask it every frame.

**Co-op (`shards::coop`, `CoopPlugin` in `EnginePlugins`).** The network (`shards_net`) and the saves (`shards_save`) know nothing of each other; the umbrella crate brings them together: it fills in `NetHooks` (`join_character`, `on_join_character`, `capture_character`,
`on_character_save`, `on_session_end`) and `SaveRouting` (`route`: where a save goes, `saved`: the game has decided to save).

- *The world is the host's.* It is written on the host: `--world`, `save_game`, autosaves and the end of a session. A client does not write the world: `save_game what=world` answers `the host keeps the world; ask the host (ADDR)`, `load_game` — `HostOnly`
  (`runs on the host only`), and an autosave that reaches the client as an event is skipped (`save: character autosave (camp)
  skipped on a client`). A `load_game` of the world on the host makes it send the world to the clients again (`WorldRebuilt`, the client's journal `net: the host's world again (save w loaded)`).
- *The character belongs to the player.* The client reads its own slot and puts it in `Hello` (a character over 64 KB, up to 1 MB, goes after `Hello` in parts; see "Co-op session (E7, `shards_net`)" and "Connecting"): `--character SLOT`; without the flag, the slot is named after the player
  (`--name`: lowercase `a-z0-9_-`, up to 32 characters, as `slot_from_name`; with `--stand` it is `c1`…`cN`, and a stand host with
  `--character c1` refuses), so that a host and a client in one folder do not write the same file. The host and a single-player game use
  `slot1` by default. A damaged file is restored from a copy (the journal `restored from backup 1`); a file with no good copies means the player joins as a new player (`Error`); if there is no slot, the player joins as a new player (`no character yet`). The host applies the character to the new body inside the recorded `player_join` (the character is in its arguments; there is no token there), so the host's replay builds the same body and passes `--verify` in a single-player game. An old version of the section is migrated. What the host does not trust: sections that the game does not save as a character (of another kind, unknown ones) are skipped with one line; a number that a body cannot have (a maximum below zero, infinity, a position at infinity, more than 64 charges) is refused by the `check` of the engine's section before any change; a panic in a game section is caught; a character that cannot be read or is refused gives `save: p1 character could not be applied (…); starting
  fresh`, and the join uses the defaults. If a section's `apply` refused, the journal says `could only be applied in part (…); applied: …;
  the failed section may have changed the body`: it may have changed the body before it failed. The peer's text in the journal is cut short and kept to one line. If `player_join` fails anyway after the body has appeared (or has been taken out of waiting), the body goes back to waiting for the player (`NetLeft`, the `NetSeat` mark stays), instead of holding the slot.
- *Progress goes backwards.* `save_now`, `autosave_on` (with `SaveWhat::Character`/`All`), and `save_game` on a remote player's body take the character off the host and send `CharacterSave` to its client instead of writing to the host's slot; the client writes its own slot with the ordinary storage (atomically, three copies): `save: character/c1 saved (camp, 1 KB, tick 51230)`, and on the host `save: character/p1 sent to p1's machine (camp, …)`. A save of the host's own character, decided by the game, also sends it to all the guests, even if the host's file could not be written (`so this build does not overwrite it`). A save that the game makes inside a forwarded call of a guest or a join (the session is not in the world then) is queued and goes out before the session is sent; it is not written to the host's slot. Leaving, a kick and a timeout send the character to the connection that went away; a client over UDP waits up to 300 ms on exit for the last `CharacterSave` (its reason `quit`).
- *The player saves on its own.* `save_game what=character [slot=…]` on a client does not take its own body off (its copy of the body is not the whole character, and taking it off would overwrite a good file), but asks the host (`CharacterAsk`): the tool's answer is `ok: character/mine asked
  of the host 10.0.0.2:7777: written here when it comes (save_game, tick 1830)`, and the named slot becomes the current one at once. The host takes off the body that the player plays with on the host, and sends `CharacterSave` with the reason `save_game` (the host's journal `net: p1
  asked for its character: sent (2 KB, tick 1832)`); the client writes it to that slot: `save: character/mine saved (save_game,
  2 KB, tick 1832)`. The request carries a number and the answer returns it, so the answer lands in the slot of its own request, even if another request has since made a different slot current. At most once a second per player: a second request within a second gets the answer `CharacterNotSent`
  (`save: the host did not send your character: it was sent less than a second ago; ask again in 0.4 s (character/b is as
  it was)`), and further ones within the same second are dropped; if there is no body or it could not be taken off, it is `CharacterNotSent` with a reason too. Not in a session: an immediate refusal (`not connected to the host yet …`, `not in a session any more (…)`). In that case the host writes nothing, and the world does not change.
  On a client, `save_game what=world` still answers that the host keeps the world.
- *The host remembers the player for the session* (`SessionCharacters`, by token): what the player came in with, and the last thing the host took off. A player who comes back brings a file; if the host knows it (it sent it itself, or applied it on the first join), the newest copy on the host wins: the body waits with its own state, and if there is no body (`rejoin_grace` has run out), it is built from the remembered one, and a `CharacterSave` (`rejoined`) is sent to the player, so that the file catches up. A file that the host does not know (played in another world) is the player's own file: it is applied, and the host remembers it. A body left over from the world before a snapshot load is older than the remembered one; the same logic hands the body to the player after "the world again".
- *The end of a session.* The world and the host's character are each written by their own call (the world's slot is named to the clients even when the host's character could not be written), and only if the game is already in play and the slots named by the flags are loaded. After that, the clients get the characters and `SessionEnd`; nothing is written or sent twice.
- *What a client writes from the host:* only the sections that the game saves as a character, no more than 1 MB, not an empty set, and nothing newer than the build; the reason for the save is cut short. If it does not fit, the file stays as it was: `save: the host's character for you … ;
  character/c1 is as it was`.
- In tests, the app is given a folder for saves, which it removes along with itself (`shards::testing`), so the tests do not share files.

**Tools.** `save_game what=world|character|all slot= label=` (control, not written to the replay; the character is Player, the world is Dev), `load_game what= slot=` (Dev, action: applies the sections and restarts the recording from the loaded state,
`replay::restart_from_current`, so a replay after a load passes `--verify`), `save_check what= slot=` (the hash of the state being saved and, with a slot, a comparison with the file by sections), `save_list` (slots, labels, ticks, copies and their health), `save_registry`, `save_damage what= slot= how=flip|truncate|garbage copy=` (Dev: damages the file only inside
`save_dir`, to test recovery), `continue_game` (Player, action: from the main menu, into the game with the newest saves, see above). The scenario `crates/sandbox/scenarios/saves.scenario` runs the whole path: save, lose the state, load, damage the file, recover from a copy, damage everything, die, load again; the reference replay `tests/replays/saves.json` was recorded from it. The scenario `world_save.scenario` saves the world in the wilds, changes the region and loads the world: the region is built from the save's manifest, and the generator does not run.

**Evidence.** `crates/shards_save/tests/save/`: 1000 random states round trip, a matrix of damage, sample files of version 1 (`tests/samples/`, never rewritten), autosave and the state hash, `load_game` and `--verify`, a launch with `--world`/`--character`, `continue_game` (the newest slot of each kind; an unreadable one is skipped; with no saves it refuses), a replay that saves and writes nothing (the slot's bytes and modification times are not touched), exit, new saves, routing of saves and the client's slot by name; `crates/sandbox/tests/sandbox/saves.rs`: the engine's sections and the revival of a dead body, a region from the save's manifest (the same world after an update that changes the generator; no tile: a refusal, with no changes; the manifest does not match the hash: damage and a copy; a version 1 save is generated again, and after an update it refuses); `crates/sandbox/tests/sandbox/net_coop.rs` (38): characters in co-op on the stand at 150 ms / 2 % and over UDP: entry with a file and without, an old version, a damaged file, distrust of what is sent, progress rolled back for the player, leaving and coming back (within the time limit and after it), the last `CharacterSave` lost, a foreign token, the end of a session, client failures, loading the world on the host (and a world of another generator version: the host builds it from the manifest, the guest gets the same), the largest character in `Hello` and a 1 MB character in parts over a lossy channel (one byte more: a refusal), saving your own character by the client through the host, the exit code at the end of a session over UDP.

## Settings (E8.6)

What the player sets in the settings menu: graphics, audio, controls and accessibility. Bevy's own settings framework stores them and keeps them between runs (`bevy::settings`, crate `bevy-settings`, feature `bevy_settings`; in `Cargo.lock` it adds only this crate), and the `shards_settings` module provides the groups, applying them and the tools. The file `settings.toml` has one table per group:

```toml
# ~/.config/shards-my-game/settings.toml
[graphics]
fullscreen = false
vsync = true
resolution = "1920x1080"     # "auto" — the game's own choice
render_scale = 1.0           # 0.5…2: the 3D world is drawn at this many times the window's pixels (UI stays at native)
shadows = "high"             # off | low | medium | high
foliage = 1.0                # 0…1: how much foliage and small objects to scatter (scatter)
interpolation = true         # draw bodies between their last two ticks (smooth at any refresh rate); false: the last tick's pose

[audio]                      # 0…1; master multiplies the others
master = 1.0
music = 0.8
sfx = 1.0
voice = 1.0
ui = 1.0                     # menu sounds

[accessibility]
camera_shake = 0.3           # 0 — no shaking at all
motion_blur = 0.5            # 0…1
flashes = "reduced"          # full | reduced | off
text_scale = 1.0             # 0.5…3, for the game's UI
fov_deg = 90.0               # 30…120
hold_to_toggle = false       # held buttons work as toggles

[controls]
mouse_sensitivity = 1.0      # 0.05…5
invert_y = false
attack_assist = true         # false: attacks do not correct their direction toward the lock-on target (holds `assist_off`)

[controls.bindings]          # only what the player changed
"gameplay.jump" = ["Key(KeyJ)", "GamepadButton(South)"]
[controls.hold_or_toggle]
"gameplay.block" = "toggle"  # press | hold | toggle
```

**What applies and where.** Each module applies its own groups, outside the simulation (systems not in `SimSet`):

| group | who and how |
|---|---|
| `graphics` | `shards_settings::apply_graphics`: the main window (fullscreen, vsync, size). The window changes when the settings change, not every frame, so the size the player dragged with the mouse is not taken back. Without a window the values are only stored. `render_scale` is applied by `shards::render_scale`: the resolution of the 3D picture, not the window scale (see "3D picture resolution" below). `shadows` is read by the game: `GraphicsSettings::shadow_quality()`; `foliage` is read by `shards_scatter` (`GraphicsSettings::foliage_density()`): at 0.5 half of the same instances stays, with no new ones (see "Foliage and small objects (E10.2)"); `interpolation` is read by `shards_motion` (render interpolation, see "Render interpolation (C8)") |
| `audio` | `shards_audio`: the volume of the buses `music`, `sfx`, `voice`, `ui` with `master` taken into account (`AudioSettings::volume("music")`), applied to what is already playing (see "Audio") |
| `controls` | `shards_motion`: `ControlSettings` ⇄ `InputMap` (see "Input" in "Movement (E2)"); sensitivity and the Y-axis inversion go to `CameraSettings`; `attack_assist = false` makes the controls hold `assist_off` for the player's body (see "Action timeline (B2a)") |
| `accessibility` | `shards_motion`: the field of view and the strength of the camera shake, `MotionBlur` on the cameras that have it (the game adds the component, the setting sets the strength, 0 turns it off), `hold_to_toggle` in the input map; `shards_action`: `flashes` for the blinking of telegraphs and of off-screen markers (`reduced` — half as big and no more than 2 times a second, `off` — a steady light, `draw::pulse_at`) |

**3D picture resolution: `graphics.render_scale` (E8.6, E10).** The 3D world is drawn into a picture of `render_scale` x the window's pixels (with `--offscreen`, the base is the size of the `--offscreen` picture, 1280×720 by default), and the picture is stretched over the whole window with a linear filter;
the UI, HUD, menus and the developer console stay at the window's native resolution. At 0.5 it is a quarter of the pixels (cheaper and softer), at 2 it is supersampling (four pixels for one). The module `shards::render_scale` (feature `render`, window and `--offscreen`):

- at 1.0 nothing is added: the cameras draw into the window (or into the `--offscreen` picture) and there is no extra pass;
- at any other scale each `Camera3d` that targets the window draws into an image (`ai_core::display::SceneImage`, `Rgba8UnormSrgb`) whose size is the window's pixel size x the scale, at most 8192 on a side. The image changes size when the window or the setting changes; `settings_set group=graphics key=render_scale value=0.5` takes effect in a running game. One `Camera2d` above the 3D cameras (its `order` is higher than theirs) shows the image over the whole window through a UI `ImageNode` node under all screens (`GlobalZIndex(i32::MIN)`) and carries the UI, while a game camera that the game itself aimed at another image (the minimap) stays as it is. The showing camera has no layers (`RenderLayers::none()`): otherwise its orthographic volume would see meshes around the origin, and `counts` (`visible_meshes`, `camera_meshes`) would count them for nothing (in the sandbox arena, 64 and 33 instead of 87 and 120), so `counts` at 0.5 gives the same numbers as at 1.0;
- the camera target's scale is `render_scale` x the window's scale, so the camera's logical size is the same as the window's: `world_to_viewport`, the HUD threat markers and the `shot` labels do not know about the scale. A camera that has to draw into the window on top of the 3D (the game's 2D camera) is put above the 3D cameras by `order`, as before;
- instead of Bevy 0.20's `MainPassResolutionOverride`: it only narrows the viewport of the main pass inside a full-size texture and leaves the stretching to the upscaler (DLSS); without it the picture stays in a corner of the frame, and the tonemapping and post-processing run over the whole frame. A stretched image is the technique that Bevy's low-resolution examples use;
- the `looks` outlines and the gizmos (the lines and the `eN` label text, the overlays) are drawn by the 3D camera, so they are in the pixels of the 3D picture: at 0.5 they are twice as thick on screen, at 2 twice as thin. This is not compensated;
- `ai/status` → `display.scene` — `{"size": [640, 360], "scale": 0.5}` (at 1.0 it is the size of the picture itself; for a game without a picture, `null`). Not part of the simulation: the image and the cameras have no `AiId`, and they are not in snapshots or replays.

Checked on a GPU with `--offscreen` (RTX 5080): the sandbox test `render_scale_draws_the_world_at_a_fraction_of_the_picture_and_shows_it_over_all_of_it` (`#[ignore]`: it needs a GPU), and `shards shot` at 0.5 (the `eN` labels in the same pixels as at 1.0, to the pixel). In a window on a display it was not checked by eye.

Values are read with clamping: a file edited by hand can contain anything (`fov_deg = 400`, `flashes = "strobe"`), and the engine uses a value within the range, or the default (`fov()`, `flash_level()`, `scale()`, …) instead of crashing. Missing keys take the defaults, unknown keys are ignored, a key with the wrong type is skipped, and a number `nan`/`inf` gets the field's default (the journal has a line for it). A file that does not parse as TOML (a typo from editing by hand) is not lost: at start it is renamed to `settings.toml.bad` (a taken name is not overwritten: `.bad.1`, …), and the game takes the last good copy (`settings.toml.bak`, updated after each successful write: the new copy is written next to it, in `settings.toml.bak.new`, and becomes `.bak` only as a whole, so on a full disk the old one stays) or the defaults, and the journal names the file and the reason. An empty file (it has nothing but spaces) while a good copy exists is a write that failed (the disk is full, the process was killed halfway): the copy is returned; if it cannot be returned, a `Warn` line names both files, the copy stays in place, and the game runs on the defaults. A file with text but no settings (all lines commented out) is a request for the defaults: it stays as it is, a copy is not restored from it, and `.bak` is not replaced. A file that is missing is a reset: an empty one is created, and the old copy goes with it. A link to a normal file (`settings.toml` from a dotfiles repository) is read through the link, as Bevy reads it; a link to a device or a folder is not read and goes to `.bad`. The `controls` table is read entry by entry: one broken binding does not take away the others. An entry with the wrong type (`"gameplay.heavy" = 7`) does not reach the game, and the file as it was is kept as the copy `settings.toml.bad`; while the entry is in the file (nothing is written by the game without changes), each start finds it again, and no copies are added: the same text does not become a copy a second time.

**Where the file is kept and when it is written.** The settings are read once, when `shards::run` starts the store (`shards_settings::install`) **after** the game's `build`: the Bevy plugin scans the type registry when it is added, and by then all the game's groups must be registered. Where:

| launch | what |
|---|---|
| window, no flags | the platform's settings folder (`$XDG_CONFIG_HOME` or `~/.config`, `~/Library/Preferences`, `%LocalAppData%`), a subfolder `SettingsAppId`: the game sets it itself (`app.insert_resource(SettingsAppId::new("my-game"))`), otherwise `shards-<window title>` |
| `--settings-dir DIR` | `DIR/settings.toml` (the folder is made absolute from the working one). The platform's settings folder is needed (Bevy joins the path to it): if it is missing (no `HOME` and `XDG_CONFIG_HOME`), the launch answers with an error and advice, and does not promise a file that will not be there |
| `--no-settings` | nothing is read or written: defaults |
| without a window (`--headless`) and without `--settings-dir` | the same: the agent's check does not read or change the player's settings |
| tests (`testing::test_app`), `--replay … --verify`, the stand's built-in games | nothing: the groups exist, with defaults |

The file is written a second after the last change (a series of edits is one write), and when the game exits (`kill`, Ctrl+C, a closed window: the exit hook `SaveSettingsSync::IfChanged`). A change to a group made by the game's code (a system changes the resource) is noticed by the same system as `settings_set`: it writes it to the journal and defers the write; there is no need to wait for an exit. A run in which nothing changed writes nothing. The write is atomic (a temporary file and a rename), but without `fsync`: this is enough for settings, not for saves (E8.1–5). Bevy does not report whether the write succeeded (on an error it still moves the temporary file into place), so the module checks the file itself: if after the write it is empty or does not parse (the disk is full, a quota), the last good copy is put back in its place, the journal gets `settings: writing … failed (…)`, and the write is retried up to three times with a pause of 3 seconds (not in the frame where the file was fixed: a disk that has just been full is still full); then the line `giving up`, and the changes stay in memory: the next change writes them again, and the exit writes once more until a write has succeeded (`SaveSettingsSync::Always`: Bevy has already marked that something changed, and `IfChanged` would find nothing). On exit the file is checked twice: the first check returns the copy and tries to write once more, the second, last, judges that attempt and writes nothing, so after a failed write on a full disk a good copy stays, not an empty file. The check judges only the writes that the store asked for or started. A change to the file that nobody asked for, an edit by the player in a running game: a readable one only updates the copy, an unreadable one goes to `settings.toml.bad` (a `Warn` line), and the last good copy takes its place, while the settings in memory stay; an empty or deleted file is not touched (an editor that first truncates the file and then writes to it keeps its write).
On the first run an empty `settings.toml` is created (otherwise Bevy logs "file not found").

**Tools** (scope Player: like the settings menu a person uses):

```text
$ shards act settings group=accessibility
accessibility.camera_shake = 1
accessibility.motion_blur = 0.5
accessibility.flashes = "full"
accessibility.text_scale = 1
accessibility.fov_deg = 70
accessibility.hold_to_toggle = false
# kept in ~/.config/shards-sandbox/settings.toml; a change is written a second after the last one, and when the game exits
$ shards act settings_set group=accessibility key=camera_shake value=0.3
ok:
  accessibility.camera_shake 1 → 0.3
$ shards act settings_set group=accessibility key=flashes value=blink
error: accessibility.flashes `blink` is not one of full, reduced, off
$ shards act settings_set group=controls key=bindings.gameplay.jump value='["Key(KeyJ)"]'
$ shards act settings_set group=controls key=bindings.gameplay.jump reset=true      # the table has no default entry, so the entry is removed
$ shards act settings_set group=audio reset=true                                    # the whole group goes back to the game's defaults
```

`settings {group?}` prints the lines `group.key = value` (table entries are `group.field.entry`), and on the last line it says whether the changes are saved, and where. `settings_set group= key= value=` reads `value` by the field's type: a boolean (`true`, `on`,
`yes`, `1`), a number (with the type's bounds: `300` for `u8` gives "out of range (0 to 255)"), an enum variant's word, text, a list as JSON (`["a","b"]`) or a single element; a number and `true` from the CLI come in as a number and a boolean, and in a text field they are taken as their own text. The whole group is rebuilt for its type, checked by the group's rules (ranges, words), and only then replaces the resource: a refusal changes nothing, and the message names what is allowed. If the group as a whole fails the rules (in a file edited by hand, two broken values), only what this call changes is judged: the defaults with this one value, so the third value can be fixed without fixing the others. A key with a typo gets a hint (`did you mean fov_deg`), and so does the call with `reset=true`: resetting a key that does not exist does not answer "nothing changed". `reset=true` returns the **game's defaults**, that is, the group's resource values at the moment the store starts, before the file (the ones the game inserted in `build`), and not the type's `Default`; resetting the key of a table entry removes the entry. A change is written to
the journal (`settings: accessibility.camera_shake 1 → 0.3`; a change to a group gives no more than 12 lines, the rest is `… and N more`). This is a control tool: **it is not written to the replay.** `ai/status` gets a `settings` field (`store`, `groups`).

**Controls.** `rebind` changes `InputMap`, and records the difference from the defaults in `controls.bindings` and `controls.hold_or_toggle` (and defers the write). An edit of `ControlSettings` (`settings_set`) or a change of the defaults in the content (hot reload) rebuilds the map; a binding that cannot be read is rejected by `settings_set` with the key's name, and it is skipped in the file with a warning in the journal (an entry for an action that does not exist in this version, and a binding that cannot be read, stay in the file as the player wrote them, and do not get in the way of the others). `settings_set` for `controls` checks the same things as `rebind`: an unknown action or context is rejected, an input that another action uses is rejected too, and so is returning an entry to its default (`reset=true`, a table without an entry) if the default would give the action an input that another action holds. It judges the map that the game will build, together with what the game added to it itself: such an action takes an input, and an entry for it is not a setting (`a control the game adds itself`: `rebind` changes it until the game ends). The refusal names the way for the caller: to free the input with `settings_set` itself (`rebind … force=true` — the developer's way); if the input is held by a control that the game added, `settings_set` will not move it, and the refusal says so and asks for another input. A context name with a dot (`ui.inventory`) is a content error: the key `context.action` cannot be split at the dot. The file is read once at start: an edit by hand in a running game is not picked up (and is not taken for a failed write, see above). `rebind` and `input_bindings` called right after `settings_set`, with no frame between the calls (a scenario, a test), see its result: they take the map that the settings make, without waiting for a frame, and `rebind` does not erase a setting that the map has not yet reached. The old `input.ron` is moved over once, if the game asked (`InputSettings::for_game`, see "Movement (E2)").

**The game's own groups.**

```rust
use shards::prelude::*;

#[derive(Resource, Reflect, SettingsGroup, Default)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "my_game")]
struct MyGameSettings { show_hints: bool, language: String, dismissed: Vec<String> }

fn check(s: &MyGameSettings) -> Result<(), String> {   // what `settings_set` rejects
    if s.language.len() < 16 { Ok(()) } else { Err("my_game.language is too long".into()) }
}

app.add_settings_group::<MyGameSettings>()                     // in build, before the store starts
   .add_settings_check::<MyGameSettings>(check);              // `add_settings_world_check` if the check needs the world too (as `controls` does)
```

A group is a resource with `#[derive(SettingsGroup)]` and `#[reflect(Resource, SettingsGroup, Default)]`; `shards::prelude` gives `SettingsGroup`, `ReflectSettingsGroup` and the alias `bevy_settings`, which the derive expects. The game's defaults are a resource inserted before the store starts (`app.insert_resource(AccessibilitySettings { fov_deg: 90.0, ..default() })`): the Bevy plugin applies the file on top of it. Only what TOML holds without surprises: `bool`, integers up to 32 bits (and `i64`), `f32`/`f64`, `String`, enum, `Vec`, `BTreeMap<String, _>`. A `u64` field (a value above `i64::MAX` breaks the write to TOML), a map with a non-string key, or a group without `#[reflect(SettingsGroup)]` is rejected at registration with a panic that names the field (`MyGroup.score: u64 is not TOML-safe …`). A group with `#[settings_group(file = "...")]` is rejected in the same way: Bevy would keep it in a separate file that the engine does not create, check, copy or watch, and `settings` would name it `settings.toml`; remove `file`. Groups with the same table name are merged into one table. Registering a group in a plugin's `finish` is too late.

**Not part of the simulation.** The settings are different on each machine: not in snapshots (`add_snapshot_resource` is not called), not in replays or their hash, and not in the replicated set (`net_ignore_resource` for each group). The engine does not read them in `SimSet`; the camera and the flashing are presentation, and `Intent` from the keyboard is written as before (the replay records `Intent`, not the settings). The reference replays were not changed after E8.6.

Tests: `crates/shards_settings/tests/settings/` (groups survive a restart; with no change, nothing is written; the exit hook;
the one-second wait; an empty directory and an empty file (including an empty file while a live copy exists, a copy that did not come back, and a file deleted as a reset;
a file of only comments is left as it is), not TOML (the copy
`.bad`, one per text, and a return to the last good copy), a link (symlink) to the settings file, a write that failed and is retried
after a pause, a write on exit after `giving up`, a failed write in the exit frame (the last good copy stays), an edit
of the file by hand in a running game (readable, unreadable, empty,
deleted), values edited by hand, values by type and refusals, reset to the game's defaults, changes to the game's code, the game's own
groups, groups that TOML does not hold or that keep their own file, isolation from the snapshot, the hash and replication), `shards_motion`
(`tests/motion/input.rs`: moving `input.ron` only at the game's request and only in the platform folder, map ⇄ settings,
contexts added by the game, `settings_set` and `rebind` refuse in the same way (including when an entry is reset to its default, and
for actions added by the game), `rebind` and `input_bindings` right after `settings_set`, a context name with a dot,
`hold_to_toggle`; `tests/motion/camera.rs`),
`crates/sandbox/tests/sandbox/settings.rs`, scenario `crates/sandbox/scenarios/settings.scenario`.

## UI (E9)

The game's HUD and menus are content. The `shards_ui` module keeps the open screens in a stack, moves focus like a gamepad, presses buttons and
changes values, and it describes all of this to the agent in text: the menu is walked as a UI tree, with no picture. With a window
(feature `render`) the screens are drawn with Bevy UI, with the theme, the player's text size and labels for screen readers.

**Screen** is a definition of the `screen` kind (`content/**/*.screen.ron`, hot reload):

```ron
// content/ui/menus.screen.ron
{
    "pause": (
        pauses: true,                       // single-player: PauseState::Paused while it is open
        title: "Paused",
        widgets: [
            Button(id: "resume", label: "Resume", action: "close"),
            Button(id: "settings", label: "Settings", action: "open settings"),
            Button(id: "quit", label: "Quit", action: "open confirm_quit"),
        ],
    ),
    "settings": (
        title: "Settings",
        widgets: [
            Tabs(id: "tabs", children: [
                Panel(id: "display", title: "Display", children: [
                    Slider(id: "text_size", label: "Text size", min: 0.5, max: 3.0, step: 0.25, setting: "accessibility.text_scale"),
                    Choice(id: "flashes", label: "Flashes", options: ["full", "reduced", "off"], setting: "accessibility.flashes"),
                ]),
                Panel(id: "controls", title: "Controls", children: [
                    Toggle(id: "hold", label: "Hold to toggle", setting: "accessibility.hold_to_toggle"),
                ]),
            ]),
            Button(id: "back", label: "Back", action: "back"),
        ],
    ),
    "confirm_quit": (
        layer: Modal, closable: false, title: "Give up?",
        widgets: [Panel(layout: Row, children: [
            Button(id: "yes", label: "Yes", action: "give_up"),   // a game action: add_ui_action
            Button(id: "no", label: "No", action: "close"),
        ])],
    ),
}
// content/ui/hud.screen.ron
{
    "hud": (
        layer: Hud, open_at_start: true, anchor: TopLeft,
        widgets: [
            Bar(id: "health", label: "HP", bind: "you.health", color: "health"),
            Bar(id: "stamina", label: "Stamina", bind: "you.stamina", color: "stamina"),
            Hint(text: "[Esc] Pause"),
            Compass(),
            Allies(id: "allies"),
        ],
    ),
}
```

| screen field | what |
|---|---|
| `layer` | `Hud` (takes no input, sits under the menus), `Menu` (the default: takes control), `Modal` (on top of the menus; while it is open, nothing below it can be pressed) |
| `pauses` | in single-player (`NetRole::Single`), an open screen sets `PauseState::Paused`, and the simulation stops; closing it resumes the game only if the menu set the pause. In co-op the world keeps going. A HUD that pauses the game is a content error |
| `open_at_start` | open on entering the game (`OnEnter(InGame)`): this is how the HUD opens |
| `main_menu` | the game's main menu: open it on entering `EngineState::MainMenu` (after loading, in the run that shows the menu; see "Main menu" in "Core (E1)"), and when the game starts, close it and anything open on top of it (settings). A HUD cannot be the main menu: it takes no input (a content error). A field with the value `false` is not written, so existing screens and their hash do not change |
| `closable` | `false`: `back` and `ui_close` do not close the screen, only its buttons do (a question that must be answered) |
| `title`, `anchor`, `layout` | the title; where on the screen (`TopLeft` … `BottomRight`, default `Center`); `Column` or `Row` |

**Widgets.** Some hold other widgets: `Panel` (title, `layout`), `List`, `Grid` (`columns`), `Tabs` (pages: `Panel`/`List`/`Grid`
with an id; only one is shown). Some show content: `Text` (its own text or `bind`), `Hint` (a hint; an empty one is not shown), `Image` (a picture:
`path` under the assets folder, `label` — what it shows, for the screen reader and the agent, `width` and `height` in logical pixels; give one size and
the other follows the picture's proportions; give neither and the picture keeps its own size; the content check names a missing file and a size that is not
greater than 0), `Bar` (`value`/`max` or
`bind`, `color` — a theme colour name or `#rrggbb`), `Cooldown` (time until ready), `Compass` (where the player looks and the four
nearest points of interest), `Allies` (ally frames: name, health, whether down), `Compare` (before → after by rows: `damage 12 →
15 (+3)`). These take input: `Button` (`action`), `Slider` (`min`, `max`, `step`), `Toggle`, `Choice` (`options`); the last three have
`setting: "group.key"` — a player setting: the widget shows it and changes it through `settings_set` (its checks, the write to
`settings.toml`, the journal entry `settings: …`). `Binding` (`context`, `action`, `slot`) — a key or button of an action that the player
changes themselves (see "Keys" below). A widget's id is `screen/id` (`pause/resume`); a widget with no id gets its kind and number (`hud/hint1`).
Content errors name the file and the widget: a repeated id, `open` of a screen that does not exist, a slider with no range, a `setting` not of the form
`group.key`, a `context` with a dot, a `taken` that is not an `@id` message; the edit that causes them is rejected, and the previous screen stays.

**Button actions:** `close` (close its own screen), `back` (close the top one), `open <screen>`, `quit` (`AppExit`), `tool <name>
{json}` — a tool run as the agent that pressed the button, with its scope (a button does not give the player a developer tool), a single word names
a game action:

```rust
app.add_ui_action("give_up", |world, press| {          // press: widget, screen, action, agent
    world.resource_mut::<Run>().over = true;
    Ok("you gave up".into())                            // the reply text of `ui_click`; Err is a refusal
})
.add_ui_source("trophies", |world, _arg| {               // bind: "trophies"; "name:arg" passes arg
    Some(UiValue::Text(format!("{} trophies", world.resource::<Trophies>().0)))
});
```

Engine sources (`shards`): `you.health`, `you.stamina`, `you.poise` (a bar), `you.charges:<charge>`, `you.cooldown:<action>`,
`you.heading` (the compass), `allies`, `input.bindings:<context>.<action>` (the inputs of an action in the input map, as the `controls`
settings make it at that moment; `Binding` reads it). "You" is the body this machine controls (`LocalPlayer`), otherwise the body
the agent plays (`AiPlayer`). An unknown source writes one warning to the journal, and the widget stays empty.

**Theme** is a definition of the `theme` kind with the id `default` (without it, the engine's theme is used): colours `text`, `dim_text`, `panel`, `backdrop`,
`button`, `button_hover`, `accent`, `focus`, and named colours (`health`, `stamina`, `poise`, `mana`, and your own), font sizes
(`font_size`, `title_size`, `hint_size`), `spacing`, `padding`, `radius`, `bar_width`, `bar_height`, `button_width`. Font sizes are
multiplied by the player's `accessibility.text_scale`.

**For the agent.** Tools (scope Player — the same menus a person uses):

```text
$ shards act ui_open screen=pause
ok: opened pause (menu, pauses the game) · focus pause/resume
$ shards act ui_navigate dir=down
ok: focus pause/settings (button "Settings")
$ shards act ui_navigate dir=confirm
ok: pressed pause/settings: opened settings (menu) · focus settings/tabs
$ shards act ui_read
SCREENS  settings ◀ takes input · pause
settings "Settings" (menu)
  settings/tabs  tabs [display] controls ◀ focus
  settings/display  panel "Display"
    settings/text_size  slider "Text size" 1 (0.5..3 step 0.25) [accessibility.text_scale]
    settings/flashes  choice "Flashes" full (full|reduced|off) [accessibility.flashes]
  settings/controls  panel "Controls" (page not shown)
  settings/back  button "Back" → back
…
$ shards act ui_set id=settings/text_size value=1.6
ok: settings/text_size 1 → 1.5                    # to the slider's step; 9 → 3, clamped to its edge
$ shards act ui_click id=settings/hold
error: settings/hold is on a page that is not shown …
$ shards act ui_navigate dir=back
ok: closed settings; pause is on top · focus pause/settings
```

`ui_read {screen?}` — the tree of open screens, top one first: id, kind, label, value, binding (`[group.key]`, `← you.health`),
focus. `ui_open screen=` and `ui_close {screen?}` open and close; `ui_navigate dir=` moves like a gamepad
(`up`/`down`/`next`/`prev` — focus that wraps at the edge; in a row and a grid also `left`/`right`; `left`/`right` on a slider,
toggle, choice and tabs change the value; `confirm` presses, `back` closes); `ui_click id=` presses (a button
runs its action, a toggle and a choice move on); `ui_set id= value=` sets a value (a number, `on`/`off`, a choice option, a page
by id, title or number). Errors say how to fix the call: ``no screen `setting` (did you mean `settings`?)``,
`pause/resume is under confirm_quit: answer or close that first`, `` settings/text_size is a slider: a number from 0.5 to 3, not `big` ``.
While a menu is open, `observe` prints a `UI` section after `LANDMARKS` (the top screen, up to 20 lines, and which tools to act with);
the HUD does not appear in `observe`. Journal: `ui: opened pause`, `ui: pause/resume pressed (close)`, `ui: settings/text_size
1 → 1.25`, `ui: closed pause`; the code listens for the events `UiFocusMoved`, `UiPressed`, `UiChanged`, `UiScreenChanged` (the audio module plays sounds for them).
`ai/status` gets a `ui` field (`screens`; `input` — the screen that takes input; `focus`).

**Main menu** is a screen with `main_menu: true`: in it, `tool new_game` starts the game, `tool continue_game` continues from the newest
save (with no saves it refuses and says why), `open settings` and `quit` work as everywhere. The agent works through it with the same
`ui_*` calls (without a window, with `--menu`): after loading, `observe` prints the menu's `UI` section, and `ui_click id=main_menu/new_game`
replies `pressed main_menu/new_game: new_game: the game starts at tick 0`. The template's main menu is an example.

**Loading image** (`EngineConfig::loading_image`, feature `render`, with a window or `--offscreen`): full screen, on top of all
screens, on every frame that is drawn while the engine is in `Loading` or `RegionLoading`. The image covers the window, centred;
the edges that do not fit are cut off. The first load takes many frames, and the image shows from the frame on which its file is read, until
the last frame of `Loading`. A region change (`regen`, `region_load`) goes through `RegionLoading` inside one call, if nothing is left to wait for:
no frame is drawn between them, the image is not seen, and the next frame already shows the new region. It shows
only on a change that waits for loading for longer than the call (the loading gate holds `RegionLoading` for several frames). Without a window nothing
is drawn. The template's image (`template/assets/ui/loading.png`, 1280×720, 51 KB) is drawn by `cargo run -p sandbox --example
make_art` from flat shapes in the engine's style (light bands, flat faces), with no third-party assets; the stars come from the seed, so the file
is the same each time.

**For a person.** The `pause` key of the input map (Escape, Start on a gamepad) opens the `pause` screen in the game (not in the main menu and not
during loading), if the content has it. While a menu is open, the `menu` input context is on top (the character gets no input):
`navigate` (arrows, WASD, the left stick, the d-pad; a held direction repeats after 0.35 s and then every 0.12 s), `confirm`
(Enter, South: A on Xbox, ✕ on PlayStation) and `back` (Escape, East: B, ○); they can be rebound (`rebind context=menu`, the `controls`
settings). The mouse goes through Bevy's picking: hovering highlights, a click is `ui_click`. All of this is `ui_*` calls from the agent `input`, so the replay
records them and presses the same buttons on the same ticks.

**Keys.** The rebinding screen is built from the same widgets: one `Binding` row per action.

```ron
Panel(id: "keys", title: "@controls-keys", children: [
    Binding(id: "jump", label: "@controls-jump", action: "jump", prompt: "@controls-press", taken: "@controls-taken"),
    Binding(id: "jump_pad", label: "@controls-jump", action: "jump", slot: 1),        // the second input only
    Binding(id: "confirm", label: "@controls-confirm", context: "menu", action: "confirm"),
]),
```

| `Binding` field | what |
|---|---|
| `context` | the input context; the default is `gameplay` |
| `action` | the action of the input map |
| `slot` | only this slot (0 is the first input; one past the last one is a new input). Without it, the row shows all inputs, and a new input replaces the first input of the same device (keyboard and mouse are one device, gamepad is another) or is added if there is none |
| `prompt` | what the row shows while it waits; the default is `Press a key or button… (Esc cancels)` |
| `taken` | an `@id` message about the input being taken, with the parameters `$input` and `$action`; the default is the English `<input> is already used by <action>` |

The row writes inputs the way the player reads them (`Binding::label`): `Space · Pad South`, `Left Shift`,
`Mouse Right`, `Pad L2`. Confirming a row (`confirm`, a click, `ui_click`) puts it into waiting: it shows
`prompt`, and the next key, mouse button or gamepad button (a gamepad is read the same way as in the game) becomes the binding —
the menu hands it over with a `ui_set id=<row> value=Key(KeyK)` call from the agent `input`, and the replay records that call. Escape and Start
cancel the wait, as `back` does, and the screen does not close (the next Escape would close it); moving to another widget also
cancels it. An arrow key that was just assigned does not move the focus, and releasing a mouse button that became the binding
presses nothing. Escape and Start in `gameplay` are `pause`, so they cannot be assigned in any case.

Only the settings change a binding: `ui_set` calls `settings_set group=controls key=bindings.<context>.<action>`, so
`settings.toml` (`[controls.bindings]`) holds only the difference from the defaults, `value=default` returns the defaults
(`reset=true`), and `ControlSync` rebuilds the input map. An input that another action of the context reads is refused: it
cannot be taken away silently. The row says who holds it (`E is already used by Interact`, `E уже используется для «Взаимодействие»`), and the reply
from `ui_set` is the settings refusal, with that action's row on the screen. Axes (the stick, `WASD`) cannot be changed this way; for them there is `settings_set`.

```text
$ shards act ui_click id=settings/jump
ok: settings/jump waits for an input: the next key, mouse button or gamepad button becomes it (Escape or Start cancels, as `back` does); an agent gives one with `ui_set id=settings/jump value=Key(KeyK)`
$ shards act ui_set id=settings/jump value='Key(KeyK)'
ok: settings/jump Space · Pad South → K · Pad South
$ shards act ui_set id=settings/dodge value='Key(KeyE)'
error: settings/dodge: controls.bindings.gameplay.dodge: Key(KeyE) is already bound to `interact` in `gameplay`; free it first: … (on this screen: settings/interact)
$ shards act ui_read screen=settings
…
    settings/jump  binding "Jump" K · Pad South = Key(KeyK), GamepadButton(South) [controls.bindings.gameplay.jump]
    settings/dodge  binding "Dodge" Left Ctrl · Pad East = Key(ControlLeft), GamepadButton(East) [controls.bindings.gameplay.dodge] · E is already used by Interact ◀ focus
$ shards act ui_set id=settings/jump value=default
ok: settings/jump K · Pad South → Space · Pad South
```

While a row waits, `ui_read` adds `· waiting for an input (Escape cancels; ui_set id=… value=Key(KeyK))` to it, and the journal gets
`ui: settings/jump waits for an input`, `ui: settings/jump stopped waiting for an input`, `ui: settings/jump Space · Pad South → K ·
Pad South`. In the sandbox and the template, the controls page has a "Keys and buttons" panel: jump, dodge, light and heavy
attacks, block, parry, interact, item, lock-on, sprint and glide.

**Outside the simulation.** Widgets have no `AiId` (the menu does not shift ids), screens are in neither the snapshot nor the replay hash, and the host does not
send them: each machine has its own. The tools that change them are action tools, and the replay repeats them; a call that a button makes
(`tool heal …`) is not written to the replay on its own: the same click makes it.

**Drawing** (feature `render`, only when there is a window): one Bevy UI node per widget, a frame per screen (a menu dims the
game, the HUD does not catch the mouse, a modal window is above the menu), values change in place, focus is an outline and `InputFocus`. Each widget gets
an AccessKit node: a role (`Dialog`, `Button`, `Slider`, `ProgressIndicator`, …), a label and a value, so a screen reader reads the
menu. Text is drawn in the theme's font (`font: "fonts/MyFont.ttf"`, a file in the assets folder; the content check looks for it); without one,
the engine's default font is used (`shards::ui::DEFAULT_FONT`, Fira Sans: Latin, Greek, Cyrillic).

Tests: `crates/shards_ui` (unit tests for parsing and navigation; `tests/ui/`: screens, menus, keyboard, drawing and accessibility labels
without a GPU, a replay with pressed buttons, an edit to a screen in a running game and the refusal of a broken one, the main menu, the image and
the loading image — `main_menu.rs`; `bindings.rs` — rebinding through `ui_set`, a refusal that names the action, waiting for
a key, a mouse button and a gamepad button, Escape, which does not close the screen, a replay), `template/tests/game/main_menu.rs` (the path
through the menu gives the same game and the same replay as without it; "Continue" with a save and without one), scenarios
`crates/sandbox/scenarios/ui.scenario` and `template/scenarios/menus.scenario`.

## Localization (E9.4)

The game's text is in [Fluent](https://projectfluent.org) files, one folder per language: `assets/locale/<language>/*.ftl`. Which languages exist
is declared by content of the `locale` kind (`content/**/*.locale.ron`): the id is the language tag (`en`, `ru`, `pt-BR`), `name` is its name in that
language, `fallback` is the language the missing text comes from. The `shards_locale` module builds the languages from content and files, keeps
the language the player uses, and switches it on the fly: open screens update all their text at once.

```ron
// content/ui/languages.locale.ron
{
    "en": (name: "English"),
    "ru": (name: "Русский", fallback: "en"),
}
```

```ftl
# assets/locale/ru/menus.ftl
pause-resume = Продолжить
defeated = { $count ->
    [one] Повержен { $count } враг
    [few] Повержено { $count } врага
   *[many] Повержено { $count } врагов
}
leave = Покинуть { $place }?
place-arena = арену
```

**Whole phrases only.** A message is a whole phrase, with parameters; the code does not glue text together from pieces. A number picks the
plural form by the language's rules (`1 враг`, `3 врага`, `5 врагов`; in English `1 enemy`, `3 enemies`), and a parameter
can be another message of the same language: `leave` with `$place = place-arena` gives `Покинуть арену?`. From code:

```rust
use shards::prelude::*;                                    // Args, Locale

let text = shards::locale::text(world, "defeated", &Args::new().number("count", n));
let ask = shards::locale::text(world, "leave", &Args::new().message("place", "place-arena"));
```

**Content names the messages** with `@`: `label: "@pause-resume"`, `title: "@settings-title"`, `text: "@hud-hint"`. This is how
the titles of screens and panels, labels, texts, hints, `Compare` rows and the labels of choice options are written — `Choice` now has
`labels` (one per option: `options: ["full", "reduced", "off"], labels: ["@flashes-full", …]`); `ui_set`
accepts both an option and its label. `@@` starts a text with `@`. A game source (`add_ui_source`) returns finished text:
the template formats `hud-trophies` with the number of trophies, and the HUD writes `1 trophy` or `5 трофеев`.

**Player language** is the `locale.language` setting (the `locale` group in `settings.toml`). It changes like any other setting: in the menu (`Choice` with `setting: "locale.language"`) or with `settings_set group=locale key=language value=ru`. An empty value means the game's own language: the first language by id that has no `fallback`. A language the game does not have is rejected: ``` `de` is not a language of this game: en (English), ru (Русский) ```.
The change applies before the call returns (the `SettingsChanged` event of the settings module). The journal writes `locale: language en → ru`, the module sends `TextChanged`, and the UI rewrites the labels of the open screens, without touching values, focus and pages.

**What is missing.** A message that the language does not have is taken from `fallback`. If it is not there either, its id is shown. The journal names each such case and each forgotten parameter once: ``locale: ru has no message `settings-fov`; it is shown in en``, ``locale: en `leave`:
Unknown variable: $place``. The content check (`--check-content`, `content_check`) reads the files of each language and names each error by file, line and column (`locale/ru/menus.ftl:9:15: Expected a token starting with "="`). It also finds a message that is in two files of one language, a `fallback` chain that loops back on itself, a language with no files, and a screen `@id` that is in neither the language nor its fallbacks: ``screen `pause`: widget `resume` label names message `pause-resum`, which en and ru have not, nor their fallbacks (did you mean
`@pause-resume`?)``.

**Hot reload.** With a file watcher (`hot_reload`), an edit to a `.ftl` file applies within a second: `locale: text
reloaded (locale/ru/menus.ftl)`, and the screens are rewritten. A reload rejects an edit with an error the same way it rejects content: the file on disk keeps the error, and until it is fixed the game keeps the text it loaded before. The journal names the place once: `[error] locale: locale/ru/menus.ftl:2:14: Expected a token starting with "="; kept the previous version` (a new file waits: `left out until it parses`). A language whose files cannot be read right now (the editor is saving the file) or are not in UTF-8 keeps its old text: `…: cannot read: …; ru keeps the text it had`. A file that is not UTF-8 is read strictly, not with replacement characters: otherwise it would give other text and other message ids. A new language is a line in `*.locale.ron` (the content reload picks it up) and a folder of files.

**For agents.** `locale` shows the language being played, how many messages each language has, and which ones it lacks. `locale
language=ru` shows all the messages of a language with their text. `text id= args={json} language=?` shows a message with its parameters, as the game shows it (a number stays a number, `"@id"` is a phrase, `"@@x"` is the text `@x`). It also prints `#` lines that say what came from the fallback language:

```text
$ shards act locale
playing: en
en English: 48 messages
ru Русский → en: 47 messages; lacks 1: settings-fov
# switch: settings_set group=locale key=language value=<id>; `locale language=<id>` lists one language's text
$ shards act text id=defeated 'args={"count": 3}' language=ru
Повержено 3 врага
$ shards act text id=pause-resum
error: en has no message `pause-resum` (did you mean `pause-resume`?)
```

`ai/status` gets a `locale` field (`language`, `chain`: the language and its fallbacks, and `languages`). Both tools are queries with scope Player.

**Outside the simulation.** The language is a setting of this machine. It is not part of the snapshot or the replay hash, the host does not send it, and two co-op players can read the game in different languages. The loading gate waits until the languages are built, so the first screen is already in the player's language.

**Font.** Text that does not name a font (menus and the HUD without a `font` in the theme, the console, the developer menu, overlays) is drawn in the engine's default font, Fira Sans (SIL Open Font Licence, `crates/shards_ui/fonts/FiraSans-OFL.txt`; the licence is also in the font file itself). It covers Latin, Greek and Cyrillic, dashes, quotes, `№` and arrows. The sandbox and the template need no font of their own. A game in another script (CJK, Arabic) names its own font in the theme (`font`). The font has no symbols such as ⚠ and ✓: the HUD and overlays write ASCII in their place, and the developer console writes `?` (`ai_core::display::default_font_has`).

**System language on first launch.** While the player has not chosen a language (`locale.language` is empty) and the settings are stored on disk, the game takes the first of the system's UI languages that it has when it starts (Windows: the user's languages; Linux and macOS: `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`). It takes either the same tag (`pt-BR`) or the same language (`ru-RU` matches `ru`, the tag without the region, tried earlier), and writes it to the settings. From then on it is the player's choice. In the journal: `[local] locale: the system's language is ru-RU; playing ru
(kept in the settings)`. Without a settings store (tests, the agent's `--headless`, `--no-settings`) the system language is not asked for: the game on any machine starts in its own language. A test supplies the system languages with the `shards::locale::SystemLanguages` resource.

Tests: `crates/shards_locale` (unit tests: plural forms, a phrase inside a phrase, the fallback language, a forgotten parameter, parsing files and errors with their position; `tests/locale/`: switching the language in a running game and the menu in the new language, the content check, hot reload of a file), the scenario `crates/sandbox/scenarios/locale.scenario`, and in the template `template/scenarios/menus.scenario` and `tests/game/screens.rs`.

## Audio (E9.5)

Three kinds of content set how the game sounds. The `shards_audio` module decides what sounds now, from where, and how loud. It decides on the simulation clock, the same way in a window and without one, so an agent hears the game through its tools, not through speakers. The only exception is the sound of a clip marker (see "Where the sounds come from").

```ron
// content/audio/arena.sound.ron: sounds (kind `sound`)
{
    "warden_slam": (files: ["sounds/warden_slam.wav"], priority: 220, spatial: (near: 4.0, far: 55.0)),
    "hit": (files: ["sounds/hit_1.wav", "sounds/hit_2.wav"], priority: 120, spatial: (near: 2.0, far: 30.0), max_instances: 4),
    "ui_press": (files: ["sounds/ui_press.wav"], bus: Ui, volume: 0.5),
}
// content/audio/arena.music.ron: music (kind `music`)
{
    "arena": (
        stems: [
            (file: "music/arena_pad.wav", layers: ["explore", "tension", "combat"]),
            (file: "music/arena_pulse.wav", layers: ["tension", "combat"]),
            (file: "music/arena_drums.wav", layers: ["combat"]),
        ],
        layers: [(name: "explore"), (name: "tension", when: "npc_near"), (name: "combat", when: "npc_engaged")],
        fade_in: 1.0, fade_out: 2.5, hold: 5.0,
    ),
}
// content/audio/mixer.audio.ron: mixer (kind `audio`, one per game)
{ "mixer": (voices: 24, music: "arena", sounds: {"damaged": "hit", "ui_pressed": "ui_press"}) }
```

**Buses.** Every sound goes through a bus: `Music` (music only), `Sfx` (the default: hits, attacks, the world), `Ui` (menus), `Voice` (speech). A bus's volume is the player's setting `audio.<bus>` multiplied by `audio.master` (the `audio` group; the new setting `audio.ui` is the menu volume). `settings_set group=audio key=sfx value=0.5` or the menu slider applies to what is already playing.

**Voices.** At most the mixer's `voices` sounds play at once (24 by default). When no voice is free, a new sound takes the voice of the weakest one (lower `priority`, then quieter, then older), if it is stronger itself. Otherwise it is not heard. A sound with `max_instances` takes the voice of its own oldest instance. A sound with `spatial` is heard from where it happened: at full volume closer than `near`, quieter by `(1 − t)²` up to `far`, and beyond `far` it does not even start. Panning follows where the listener is looking. The listener is the body that this machine's player controls (the same `viewer` as the HUD), turned like the camera (`AudioListener`: the engine attaches it to the third-person camera). A sound made of several files plays them in turn.

**Where the sounds come from.** An attack's telegraph (E3) names its own sound (`telegraph: (…, sound: "warden_slam")`). It plays where the blow lands, from the attacker. An action also names a sound, at its own moment: the effect `Sound("whoosh")` (`effects: [(at: 0.2, effect: Sound("whoosh"))]`) plays from the body that acts, on the tick when the action reaches it. So a hero's swing is heard without a window, in a window, and on a co-op client (the host forwards the `ActionSound` event, as it does for a hit). An animation cue (`AnimationCue`) plays the sound with the same name from the body, if such a sound exists. But clip markers come from the clip, and a clip plays only where the model is drawn (a window or `--offscreen`). Without a window there is no such sound, and `audio` does not show it. Do not put a sound that the game must hear on every machine on a clip marker. The engine's events play the sounds listed in the mixer's `sounds`: `damaged`, `blocked`, `parried`, `evaded`, `guard_broken`, `staggered`, `died`, `healed` (from the body it happened to), and `ui_moved`, `ui_pressed`, `ui_changed`, `ui_opened`, `ui_closed` (the menus, the same everywhere; the HUD makes no sound). The game plays its own: `commands.trigger(PlaySound::new("coin", SoundAt::Entity(hero)))` or `shards::audio::play(world, "coin", SoundAt::Point { position, source: None })`.

**Layered music.** A track's stems are files of one length. They start together and loop, and each layer switches its own stems on. Layers go from calm to tense. Every layer except the first has a `when` condition: the name of a condition, as in `step --until named:…` (`!name` negates it). The condition is either the engine's (`npc_near` and `npc_engaged` from the NPC module) or the game's (`add_ai_condition`). The most tense layer whose condition holds plays; otherwise the first layer does. A more tense layer comes in at once. A calmer one comes in only after it has been wanted for `hold` seconds in a row, so a pause in a fight does not cut the music. Stems fade in over `fade_in` and out over `fade_out`. The journal writes each change as a line: `[local] music: arena explore → tension`. `music_set track=boss` changes the track (the old one fades out), `track=none` is silence, `layer=combat` holds the layer whatever is happening in the game, and `layer=auto` releases it.

**For agents.** `audio` (query, scope Player) shows what this machine hears: the track, the layer and how long it has lasted, which calmer layer is waiting for its `hold`, the stems with their volume, the buses, the voices (sound, source, distance, volume, priority, length), and the latest sounds that were not heard, with the reason, and where the listener is:

```text
$ shards act audio
music arena: combat for 10.9s
  music/arena_pad.wav [arena] 1.00
  music/arena_pulse.wav [arena] 1.00
  music/arena_drums.wav [arena] 1.00
buses (with master): music 0.80 · sfx 1.00 · ui 1.00 · voice 1.00
voices 2/24 (started 29, dropped 1)
  #27 parry sfx e6 1.2m gain 1.00 prio 160 (0.80s)
  #29 warden_slash sfx e6 0.3m gain 1.00 prio 180 (0.32s)
  dropped whoosh at 10.3s: out of earshot: 32.5 m, heard to 20 m
listener e5 at (0.0, 0.0, -2.6)
$ shards act sound_play sound=parry at=e6
playing parry #27: sounds/parry.wav at 1.2 m, gain 1.00, 0.80s
$ shards act sound_play sound=parry at=e5 times=25
parry ×25: 24 playing, 1 not heard: all 24 voices play sounds as strong or stronger
$ shards act music_set layer=explore
arena keeps to explore until `music_set layer=auto`
```

`observe` writes a `MUSIC  arena: combat for 10.3s` line, and `ai/status` has an `audio` field (`track`, `layer`, `voices`, `limit`). `sound_play` (`sound`, `at=eN` or `point=[x,y,z]`, `times` up to 64) and `music_set` are Dev tools of the control kind. The errors name what exists: ``no sound `warden_slom` (did you mean `warden_slam`?)``, ``arena has no layer `fight`; layers: explore,
tension, combat``.

**Clock.** The mixer and the music run in `FixedPostUpdate` after each tick (`AudioSystems::Mix`): `step` moves them, and pause stops them. Menu sounds run in real time and are heard during pause, while effect voices freeze during pause.

**Outside the simulation.** Sound belongs to this machine. Conditions and events are only read, nothing about sound enters the replay hash, and the music lines in the journal are of kind `local` (they are not game events, and the host does not send them to clients). A co-op client hears its own copy of the fight (the sound of a `Sound` effect reaches it as an event from the host). The music state (`Music`: track, layer, how long it has lasted, stems) is part of the snapshot, so a loaded session hears the same music; the voices are not.

**Playback.** The `audio` feature of the `shards` crate (enabled in the sandbox and the template) plays the mixer's decisions through Bevy audio: WAV and Ogg Vorbis. On Linux it needs `libasound2-dev` (it is already in the list of packages). Without an output device nothing sounds, but the mixer decides the same way. Each voice is a sink for its own file, with the volume from the mixer. A spatial voice stands one metre from the ears, on the side the sound comes from.

**Content check.** `--check-content` and `content_check` find: a file that is missing or is not a WAV or Ogg Vorbis; stems of different lengths (``stems loop together, so they need one length: `music/calm.wav` plays 2.00 s, `sounds/beep.wav` 0.50 s``); a layer without stems; a first layer with `when` and the next one without it; a stem's layer that the track does not have (with a hint); a sound on the `Music` bus; `far` closer than `near`; `max_instances: 0`; a mixer event that the engine does not have (``no engine event `damagd` (did you
mean `damaged`?)``); a second mixer; and an attack whose telegraph names a sound that does not exist (``action `bite` warns with sound `boon`,
which no `*.sound.ron` defines (did you mean `boom`?)``) or plays it with a `Sound` effect (``action `swing` effects[0] plays sound
`bom`, which no `*.sound.ron` defines (did you mean `boom`?)``). The journal names a layer condition that the game did not register, once, when the track plays.

**Sandbox and template files** are synthesised; there are no recordings in the repository. `cargo run -p sandbox --example make_sounds` writes them again (16-bit mono WAV, noise with a seed: the same bytes on every run).

Tests: `crates/shards_audio` (unit tests: WAV and Ogg lengths, falloff with distance, buses from the settings, voices and voice stealing; `tests/audio/`: the voice limit and `max_instances`, distance and source, the simulation clock, the player's volume, events, telegraphs, the `Sound` action effect and the menu, tools, music layers and `hold`, `music_set`, `local` lines, the snapshot, the content check), the scenario `crates/sandbox/scenarios/audio.scenario`, and in the template `tests/game/audio.rs` (the music switches to `combat` when the hero approaches the ground; the hero's swing sounds without a model).

## Stylised materials (E10.1)

What a surface looks like is set by content of the `look` kind (`*.look.ron`). The `shards_look` module draws the meshes from it: light in bands, flat faces, an outline of one width in pixels, and the surface's own glow. The game gives meshes one white material and a look name. The colour and the light come from the look. This concerns only the picture: gameplay knows nothing about looks.

```ron
// crates/sandbox/assets/content/look/sandbox.look.ron (shortened)
{
    "low_poly": (flat: true, outline: Some((width: 2.0, color: "#0b0f1a"))),
    "player": (extends: "low_poly", color: Some("#408cff")),
    "stone": (color: Some("#61636e"), outline: Some((width: 1.5, color: "#16171c"))),
    "floor": (color: Some("#2b303b"), shading: Toon(bands: 2)),
    "dark": (color: Some("#1a1a1f"), shading: Matte),
    "marker": (color: Some("#ffd933"), shading: Unlit),
}
```

**Fields.** All of them are optional. A field that a look does not have is an error, reported with its line (`deny_unknown_fields`).

- `extends: "low_poly"` — a preset: a look starts from another look and changes only the fields it names itself (a chain of any length; `materials` is not inherited). This is how the low-poly style is set once: `low_poly` gives flat faces and an outline, and the bodies name only their own colour.
- `color: Some("#rrggbb")` — the colour (`#rgb`, `#rrggbb` or `#rrggbbaa`, sRGB). It is multiplied by the mesh's texture and vertex colours. The material's transparency stays unless the colour sets it (`#rrggbbaa`). Without `color` the mesh keeps its own colour: that of the game's material, or of the glTF file.
- `shading` — how light falls on it: `Toon(bands: 3)` (the default; from 2 to 8 bands), `Matte` (soft light, no highlight), `Unlit` (the colour as it is, in sun and in shadow), `Lit(roughness: 0.4, metallic: 1.0)` (Bevy's physical light; both numbers from 0 to 1).
- `flat: true` — flat faces: each triangle gets its own normal, so any mesh gets low-poly faces.
- `outline: Some((width: 2.0, color: "#000000"))` — an outline around the silhouette. `width` is in pixels of the picture (more than 0, no more than 16). It defaults to 2 and black.
- `glow: Some("#7fe0ff")` — the surface's own light (lamps, eyes, crystals). It shows under any light and at any camera exposure. With `Unlit` it is added to the colour.
- `materials: ["Crystal", "Crystal.001"]` — glTF materials (the names Blender shows) that this look draws, besides the material with the look's own name.

**Which meshes.** A mesh's look is taken in this order: a `Look` on the mesh itself; a `Look` on the nearest entity above it; the glTF material name (`GltfMaterialName`: the look that lists it in `materials`, or the look with the same id). So levels and tiles from Blender get a style without any code. `Look(String)` is a component. A prefab carries it (`"Look": ("stone")`), or code sets it (`Look::new("stone")`), and it styles the meshes of its entity and of all its children. A mesh with no look found keeps the game's material. That is how water and triggers are drawn in the sandbox. Meshes with `MeshMaterial3d<StandardMaterial>` are styled. The look takes from that material everything the look does not set itself: the colour (when there is no `color`), textures, vertex colours and transparency. A `Look` with an id that is not in the content changes nothing.

**Hot reload.** An edit to `*.look.ron` in a running game repaints all the meshes in the same frame. When a `Look` is set, changed or removed, the meshes under it follow. A mesh that the game gave its own material or mesh is styled again from them. A material or mesh that was changed in place (a glTF reloaded, or the game fixed a material through `Assets::get_mut`) is styled again too. Meshes whose material or mesh is still loading (glTF) wait, and get their look when they have loaded. One material is made for each pair of "look, source material": a thousand identical stones make one material.

**Light in bands (`Toon`).** The sun (each directional light, with shadows) lights the surface in `bands` steps: 0, 1/(bands − 1), …, 1. Any light already counts as the first step, so light and shadow split where the light ends. The edge of a step is smoothed over one pixel. Two bands make the classic two-tone cel. Ambient light (`GlobalAmbientLight`), lamps and spotlights fall evenly, like Bevy's diffuse light, without bands. Nothing shines (roughness 1, metallic 0, reflectance 0; `Matte` is the same). Camera exposure applies to the light, but not to `glow`. `Toon` materials are always drawn in a forward pass, even if the game draws deferred by default: otherwise there would be no bands. The look lights a glTF material marked unlit (`KHR_materials_unlit`) by its `shading`: only `Unlit` leaves it unlit.

**Outline.** The mesh is drawn once more, inside out. A child entity, `OutlineHull`, draws only its back faces. Its vertices move `width` screen pixels along the normal on screen, so the line stays one width near and far. The outline's normals are averaged over the mesh's points: positions closer than 0.1 mm count as one point, and each face is weighted by its corner's angle. So there are no gaps on a cube's edges or at flat faces. The outline is built from the source mesh, not from a flat copy. A skinned mesh gives its outline the same `SkinnedMesh`, so the outline bends with it. The outline casts no shadow and is not drawn in the prepass. It is only around the silhouette, so there are no edges inside. The outline takes the mesh's `RenderLayers` (and follows them), `NoFrustumCulling` and `VisibilityRange`. These meshes get no outline: transparent materials (`alpha_mode` not `Opaque`: glass, cut-out leaf cards, where the shell would show through or outline the whole card), meshes with morph targets (the shell would not follow them), a mesh whose data has gone to the GPU, and a mesh that is not a triangle list. A mesh with morph targets has no flat copy either.

**Only the picture.** Meshes change only with the `render` feature of the umbrella crate, in a window or with `--offscreen`. Without a picture (`--headless`) nothing is styled: the look systems run only when there is a picture (`ai_core::display::drawn`). A `Look` on a body from a prefab is an ordinary component of the prefab. It is part of the replay and the snapshot, and it goes to co-op clients. Otherwise a client would draw the bodies that the host spawned after the client joined, without a look. So code that sets a `Look` only for the picture sets it on mesh children without an `AiId`, not on the bodies (this is what `visuals.rs` in the sandbox and the template do). The outline is an entity without an `AiId`. Looks are part of the content hash a co-op session compares, but not of the replay check `content` (`SIMULATION = false`): a new look, or an edit to one, never makes a replay diverge.

**For agents.** `looks` (Dev, query) lists the looks (colour, light, flat faces, outline, glow, glTF materials) and how many meshes each one draws now, then the meshes without a look, then the meshes that wait for their files. `look_outlines on=false` (Dev, query; with no argument, it turns them on) hides the outlines or brings them back. They stay built, so turning them on again costs nothing. The setting is the resource `LookSettings`, not part of the replay or the snapshot.

```text
$ shards act looks
15 looks · 49 meshes styled (36 outlined) · 2 without a look · outlines on
  bush       #2f7a3a · toon 3 bands · flat
  cel        toon 2 bands
  dark       #1a1a1f · matte · 5 meshes
  floor      #2b303b · toon 2 bands · extends cel · 2 meshes
  grass      #5fae45 · toon 2 bands
  grass_dry  #a7b04e · toon 2 bands
  low_poly   toon 3 bands · flat · outline 2px #0b0f1a
  marker     #ffd933 · unlit · 4 meshes
  player     #408cff · toon 3 bands · flat · outline 2px #0b1a33 · extends low_poly · 1 mesh
  rock       #8a867c · toon 3 bands · flat
  sand       #8c8061 · toon 2 bands · extends cel · 2 meshes
  skeleton   #d9d1b3 · toon 3 bands · flat · outline 2px #2a2418 · extends low_poly · 3 meshes
  stone      #61636e · toon 3 bands · outline 1.5px #16171c · 17 meshes
  wall       #454754 · toon 3 bands · outline 1.5px #16171c · 14 meshes
  warden     #8cd9ff · toon 3 bands · flat · outline 3px #0a2433 · extends low_poly · 1 mesh
$ shards act look_outlines on=false
outlines off: 36 drawn with an outline
```

This is the sandbox arena. The foliage looks (`bush`, `grass`, `grass_dry`, `rock`) draw the `scatter` instances in the region. After `regen region=wilds seed=42` the line reads `15 looks · 14035 meshes styled (146 outlined) · 22 without a look`. A look's line shows what the look actually draws: the preset values that the look does not change are already in it, and `extends` names the preset.
Without a picture, the tool says why there is nothing to count: `15 looks · nothing is drawn headless: looks style meshes in a window or
with --offscreen`, and then the same look lines without a mesh count. Meshes that still wait for a material or a mesh are named by the last line (`3 meshes wait for their material or mesh to load`).

`ai/status` has the field `looks`: `{"looks": 15, "outlined": 36, "outlines": true, "styled": 49}` (looks, meshes with an outline, outlines on, meshes styled by looks). `inspect eN` shows the entity's `Look`. In the developer menu (section `world`) there are two entries: `looks` ("Looks and what they draw") and `look_outlines` ("Outlines of the looks"; while the outlines are visible, it runs `look_outlines on=false`).

**Checking the content.** `--check-content` and `content_check` name the file, the look and the field:

```text
content/look/sandbox.look.ron: look `stone`: shading: Toon has 2 to 8 bands, not 1
content/look/sandbox.look.ron: look `stone`: color: `red` is not a colour: `#rrggbb` or `#rrggbbaa`
content/look/sandbox.look.ron: look `wall`: outline: width is above 0 and at most 16 pixels, not 20
content/look/sandbox.look.ron: look `wall`: materials: `Rock` is drawn with look `stone` already; a material has one look
content/characters/characters.prefab.ron: prefab `skeleton`: Look -> unknown look `stoen` (did you mean `stone`?)
```

Also checked: `extends` names a look that does not exist (with `did you mean`), a cycle of presets (`extends: a -> b -> a`), `Lit` with a number outside 0…1, an empty name in `materials`, and a material name that equals the id of another look (that look already draws a material with that name). Material names are matched as written, case included. The prefab check sees the `Look` in a prefab, not `Look::new` in code. A look with a typo in code silently changes nothing, and you can see it in the `without a look` line of `looks`.

**How the game uses it.** (1) The looks file `assets/content/looks.look.ron`. (2) A body is a prefab with `"Look": ("hero")`. That is how the template's hero, grunt and raider get their look, and how the sandbox's player, skeleton, guard and column do. (3) Meshes the game makes itself (bodies built from colliders, marker posts) get one white material and a `Look` on the mesh itself, not on the body (`visuals.rs` in the sandbox and in the template). The shared style is a preset: `low_poly`, with flat faces and an outline. Body looks use `extends: "low_poly"` and their own colour:

```rust
let white = materials.add(StandardMaterial::default());   // the colour and the light come from the looks
commands.entity(body).with_child((Mesh3d(mesh), MeshMaterial3d(white), Look::new("stone")));
```

Materials from Blender are named after the look, or the look lists them in `materials`. The bands and shadows come from a `DirectionalLight` with `shadow_maps_enabled: true`, and even light comes from `GlobalAmbientLight`. A mesh without a look (glass, water) keeps the game's own material. To look at it: `cargo sandbox --offscreen --paused --port 0 --port-file F &`, `shards act looks`, `shards shot -o
shots/looks.png`. To compare without outlines, run `shards act look_outlines on=false`, then `shot` again.

**What is missing.** Only `StandardMaterial`: meshes with another material do not take a look. The outline is a shell, not a post-process based on depth and normals, so it has no edges inside the silhouette. A look has one outline width: it does not depend on the distance or the size of the mesh, so it cannot get thinner far away. Checked on a GPU with `--offscreen` (the sandbox: bands, flat faces and outlines in the picture). Not checked by eye in a window on a display.

## Foliage and small objects (E10.2)

The `shards_scatter` module scatters grass, bushes and rocks over the region's tiles, using content of the `scatter` kind (`*.scatter.ron`). The content decides what grows, where, and how thickly. The region's seed, the tile's cell and the definition's id set where each instance lies, so the layout is the same in every run and on every machine. It is only a picture: instances appear when the game draws (a window or `--offscreen`). They have no `AiId` and no colliders, and gameplay, the replay and the snapshot do not know about them.

```ron
// crates/sandbox/assets/content/region/wilds.scatter.ron (abbreviated)
{
    "grass": (
        regions: ["wilds"],              // only in the wilds: `expanse` has its own, rarer one (expanse.scatter.ron)
        density: 0.4, spacing: 0.35, scale: (0.7, 1.3), tilt: 12.0,
        looks: ["grass", "grass_dry"],
        fade: 3.0,
        lods: [
            (mesh: Blades(width: 0.09, height: 0.45, count: 6), until: 20.0),
            (mesh: Blades(width: 0.14, height: 0.4, count: 3), until: 40.0),
        ],
    ),
    "rocks": (
        regions: ["wilds"],
        density: 0.03, spacing: 1.5, scale: (0.5, 1.6), tilt: 20.0, clearance: 0.3,
        looks: ["rock"], shadows: true,
        lods: [(mesh: Rock(radius: 0.35, detail: 1, squash: 0.6), until: 60.0)],
    ),
}
```

**Fields.** Only `looks` and `lods` are required. A field that does not exist is an error that names the line (`deny_unknown_fields`). The value in brackets is the default.

- `tiles: ["grass", "#meadow"]` — the tiles and rooms it grows on, by id or by `#tag` (empty means all of them).
- `regions: ["wilds"]` — the regions and dungeons (empty means all of them).
- `density` — instances per square metre of tile, from 0 to 20 (0.5). This is before the player's `graphics.foliage` setting. Together with the levels of detail, there may be at most 10 000 mesh entities (instances × levels) on the largest tile or room where the definition grows (`MAX_TILE_ENTITIES`). More is a content error.
- `spacing` — the smallest distance between two instances of this definition on one tile, in metres. 0 means any distance, otherwise not less than 0.01 (0).
- `scale: (0.7, 1.3)` — the size of an instance, a multiplier on the mesh, picked evenly between the two values ((1, 1)).
- `tilt` — the largest tilt from vertical, in a random direction, from 0 to 60 degrees (0).
- `max_slope` — nothing grows on ground steeper than this many degrees (30).
- `height: (-0.5, 0.5)` — the heights of the ground, relative to the tile's origin, on which an instance may stand. By default that is the floor of the tile, not the top of a column or a wall. The ray down starts just above this band. If the ray starts inside a solid (a wall above the band), there is no ground, so nothing grows inside walls.
- `clearance` — how much room an instance needs above the ground, in metres (0). A sphere of this radius, lying on the ground (along its normal, 5 cm above it), must not touch a solid. An instance does not stand closer to a wall or a column. The slope under the instance does not use up room: a sphere raised straight up would sink into any slope steeper than about 20°.
- `looks` — the looks (`*.look.ron`, E10.1): each instance takes one of them. The meshes are white, and the looks give the colour and the light. A look with an outline (its own, or one inherited from the look it extends, `extends`) is a content error. The outline is a shell for each instance and each level of detail, and on a level that is fading the shell does not fade.
- `shadows` — instances cast shadows (false: grass and pebbles do not need shadows).
- `fade` — how many metres a level of detail fades into the next one, and the last one into nothing (0 means they switch at once). It must be less than the shortest level.
- `lods` — from 1 to 4 levels of detail, nearest first: `mesh` and `until` (how many metres from the camera it is drawn up to). Beyond the last level, nothing is drawn.

**Meshes.** An engine primitive is built once and shared by every definition that names the same one:

- `Blades(width: 0.09, height: 0.45, count: 6)` — a tuft of grass: `count` (from 1 to 16) narrow blades in a fan, darker at the root (vertex colours), visible from both sides;
- `Cone(radius: 0.6, height: 1.4, sides: 6)` — a faceted cone (from 3 to 32 sides): a fir tree, a thorn;
- `Rock(radius: 0.35, detail: 1, squash: 0.6)` — a lumpy ball, its bottom slightly in the ground: a rock, or a bush with a leafy look;
  `detail` from 0 to 3 (from 20 to 320 faces), `squash` — the ratio of height to width, from 0.1 to 2 (1);
- `Box(size: (1.0, 0.4, 1.0))` — a box, a slab;
- `Gltf("foliage/bush.glb#Mesh0/Primitive0")` — a mesh from a glTF file in the assets folder; it is drawn white, and the look gives the colour.

Each one stands on its own origin, with +Y up.

**Layout.** For each tile and definition, `DetRng` keyed by "the region seed, the region id, the tile cell, the definition id" throws points into the tile's square: `density × area` points, with at most four times as many throws, and no closer than `spacing` to one another. Each point has its own rotation, tilt, size and look. Then each one falls to the ground on a ray. The ones that fail are dropped: those with no ground within the `height` band (or whose ray starts inside a solid), those on ground steeper than `max_slope`, and those closer than `clearance` to a solid. A tile holds at most 10 000 mesh entities of the definition: the layout keeps the first `10 000 / levels` instances of the same list and writes to the journal `` scatter `x`: N tiles would get … each keeps its first M `` (content that asks for more is already an error). The `graphics.foliage` setting (from 0 to 1) keeps the start of the same list: at 0.5, half of the same instances remain, and none are new. Tiles are scattered in cell order as soon as the region is built. A tile that goes away (a region change, `regen`) takes its instances and its count of those that found no place with it. Editing `*.scatter.ron`, the tags of a tile or room (`tags` in `*.tile.ron`, `*.room.ron`), or the setting scatters everything again in the same frame. Editing a look only recolours.

**What counts as ground and what as solid.** Only the world's static colliders count: `Body::Static`, layer `WORLD`, not volumes. What moves (`Body::Kinematic`: platforms, lifts) is neither ground nor an obstacle: an instance would stay where the ground is no longer. The rays and the `clearance` spheres do not ask about the whole world, only about the part around the tile (`place::solids`, `PhysicsWorld::subset`: colliders whose bounds reach into the tile's column). So rays and spheres do not walk through every collider in the region. In the wilds, 8×8 with seed 42 (debug build), the throws, rays and spheres for all 64 tiles take 4.2–4.8 ms, against 6.6–7.0 ms over the whole world. The rest of the layout time (20–35 ms) goes to creating and removing entities. The region's geometry (tiles, their levels and prefabs) is built before the layout, and the layout always counts it. A static collider that the game places itself is counted when the tile is scattered. A tile is scattered once, so a box placed later does not clear the grass under it until the next layout (a setting change, `scatter_set`, or a content edit). The layout is a function of the seed, the content and the static world at the moment of layout.

**How it is drawn.** Under the tile's root (it gets `Visibility`) there is one entity for each pair of definition and look, with a `Look`. Under it there is one entity per instance and level of detail: the level's shared mesh, a shared white material (the look makes its own from it), and the level's `VisibilityRange`. Bevy draws identical meshes with the same material as one batch. In the reference scene, 1382 meshes are drawn (with the effect particles and decals), and 50 of them are distinct (the field `mesh_assets` in `counts`). Past the `until` of the last level, an instance is not drawn, and Bevy culls what the camera cannot see. Banded light (`Toon`) fades the levels with the same smoothing as the Bevy material.

**For agents.** `scatter` (Dev, query) shows the definitions, how many instances each one scattered, how many throws found no place on the tiles that exist now, and why. Its last line gives how many tiles and instances the last layout scattered, and how many milliseconds it took. The figure in brackets covers throwing the points and the `clearance` rays and spheres, not creating entities. The time is wall-clock, so it differs from run to run. `scatter_set on=false` (Dev, query; with no argument, it turns the scatter on) removes everything, and `on=true` scatters again. The switch is the resource `ScatterState`, not part of the replay or the snapshot.

```text
$ shards act regen region=wilds seed=42
$ shards step 5
$ shards act scatter
6 scatters · 7182 instances (13864 mesh entities) on 64 tiles · foliage 1 · scatter on
  bushes  0.02/m² · spacing 3 m · 2 LODs to 80 m · looks bush · shadows · on grass, pillars, ruin, hall, road_bend, road_end · in wilds · 203 on 45 tiles · 22 found no place (4 no ground in the band, 18 too close to a solid)
  grass   0.4/m² · spacing 0.35 m · 2 LODs to 40 m · looks grass, grass_dry · in wilds · 6479 on 64 tiles · 49 found no place (49 no ground in the band)
  rocks   0.03/m² · spacing 1.5 m · 1 LOD to 60 m · looks rock · shadows · in wilds · 500 on 64 tiles · 12 found no place (5 no ground in the band, 7 too close to a solid)
  shrubs  0.002/m² · spacing 4 m · 2 LODs to 90 m · looks bush · shadows · in expanse
  stones  0.003/m² · spacing 2 m · 1 LOD to 70 m · looks rock · shadows · in expanse
  tufts   0.02/m² · spacing 1 m · 2 LODs to 50 m · looks grass, grass_dry · in expanse
last pass: 64 tiles, 7182 instances in 20.5 ms (4.2 ms finding their ground)
$ shards act settings_set group=graphics key=foliage value=0.5
$ shards act scatter
6 scatters · 3609 instances (6971 mesh entities) on 64 tiles · foliage 0.5 · scatter on
…
$ shards act scatter_set on=false
scatter off: 0 instances on 0 tiles
```

Without a picture, the tool says why there is nothing: `6 scatters · nothing is strewn headless: scatter is drawn in a window or
with --offscreen`, and then the same definition lines. Until the first region loads, it says `no region is loaded: …`. `ai/status` has the field
`scatter`: `{"entities": 13864, "instances": 7182, "on": true, "scatters": 3, "tiles": 64}`. In the developer menu (section
`world`) there are two entries: `scatter_set` ("Foliage and props (scatter)", a switch) and `scatter` ("Scatters and what they strew").

**Checking the content.** `--check-content` and `content_check` name the file, the definition and the field:

```text
content/region/wilds.scatter.ron: scatter `grass`: looks[1] -> unknown look `grass_dyr` (did you mean `grass_dry`?)
content/region/wilds.scatter.ron: scatter `grass`: tiles[0]: no tile or room `gras` (did you mean `grass`?)
content/region/wilds.scatter.ron: scatter `grass`: lods[1].until: 15 must be farther than 20 m, where the level before ends
content/region/wilds.scatter.ron: scatter `grass`: fade: 25 m is not shorter than the shortest level of detail (20 m)
content/region/wilds.scatter.ron: scatter `bushes`: lods[0].mesh: Gltf `foliage/bush.glb` names no mesh in the file; add `#Mesh0/Primitive0` (the first mesh's first primitive)
content/region/wilds.scatter.ron: scatter `bushes`: looks[0]: look `bush` has an outline: foliage looks are drawn without outlines (one hull per instance); give the scatter a look without `outline` (nor one it extends)
content/region/wilds.scatter.ron: scatter `grass`: spacing: meters, 0 (any) or 0.01 or more, not 0.001
content/region/wilds.scatter.ron: scatter `grass`: density: 20/m² on a tile of `wilds` (256 m²) with 2 levels of detail is 10240 mesh entities a tile, more than 10000; lower the density or the levels (each tile keeps its first 5000 instances)
```

Also checked: a tag that no tile has, a region that does not exist, an empty `looks`, more than 4 levels of detail, numbers outside their allowed ranges, and a glTF file that is not in the assets folder. Definitions are part of the content hash a co-op session compares, but not of the replay check `content` (`SIMULATION = false`): a new or edited definition never makes a replay diverge.

**How the game uses it.** (1) Foliage looks without an outline in `*.look.ron` (in the sandbox: `grass`, `grass_dry`, `bush`,
`rock`). (2) A `*.scatter.ron` file with tiles, density and meshes: in the sandbox, `region/wilds.scatter.ron`, and in the template,
`region/valley.scatter.ron` (grass on the meadows, by the gates and the columns, pebbles everywhere). No code is needed. To look at it: `cargo sandbox
--offscreen --paused --port 0 --port-file F &`, `shards act regen region=wilds seed=42`, `shards step 5`, `shards act scatter`,
`shards shot -o shots/scatter.png`.

**What is missing.** The grass does not sway (there is no wind). Density is set by tiles and tags; there is no mask inside a tile. Instances do not
block movement: anything that must block it goes on the tile as a collider or a prefab. Each instance and each level of detail is its own entity, and
Bevy assembles them into batches: in an 8×8 region this is about 14 000 entities. The reference scene with foliage, in a release build
(`--offscreen`, 1080p, RTX 5080), runs at 379 frames per second, with a worst frame of 4.2 ms. It has not been measured on an "average PC". The `Gltf` mesh
has been checked only by the content check; it has not been drawn from a real file. Not checked by eye in a window on a display, only the `--offscreen` pictures.

## Effects and decals (E10.3)

What an effect looks like is decided by content of the `effect` kind (`*.effect.ron`). Which engine event plays it is decided by a table of the `fx` kind (`*.fx.ron`, one per game). The `shards_fx` module plays particles from pools and draws attack telegraphs (E3) as decals on the ground. It is only a picture: it does not change the world, and without a picture nothing plays.

```ron
// crates/sandbox/assets/content/fx/sandbox.effect.ron (abbreviated)
{
    "hit_spark": (count: 18, lifetime: (0.2, 0.4), speed: (3.0, 8.0), spread: 120.0, aim: Hit, lift: 35.0,
        gravity: -9.0, drag: 2.0, size: (0.2, 0.34), size_end: 0.3, color: "#fff2b3", color_end: "#ff6a1a00",
        glow: true, offset: (0.0, 1.1, 0.0), max_live: 128),
    "block_spark": (count: 10, shape: Shard, spin: 720.0, aim: Hit, gravity: -12.0, color: "#bfe6ff", max_live: 64),
    "death_dust": (count: 16, rate: 24.0, duration: 0.8, shape: Cube, size_end: 1.6, color: "#8c8061cc", max_live: 96),
}
// crates/sandbox/assets/content/fx/sandbox.fx.ron
{
    "sandbox": (effects: {"damaged": "hit_spark", "blocked": "block_spark", "parried": "parry_flash",
        "died": "death_dust", "healed": "heal_glow"}),
}
```

**Effect fields.** All are optional. A field that does not exist is an error that names the line (`deny_unknown_fields`). A pair `(min, max)` is a range: each particle takes its own number from it.

- `count: 16` — particles at once. `rate: 30.0` and `duration: 0.5` — that many more particles per second, for as long as `duration` runs (up to 10 s). An effect
  with a `rate` spawns its particles where the entity is now: dust follows the body.
- `lifetime: (0.4, 0.8)` — how long a particle lives (s, up to 30); `speed: (1.0, 3.0)` — the starting speed (m/s); `spread: 60.0` —
  the width of the cone of directions (degrees, 360 means all directions); `aim` — the axis of the cone: `Up` (upwards, the default) or `Hit` (for an event
  with an attacker: outwards from the point of impact, back towards the attacker); `lift: 25.0` — how far the `Hit` axis is raised above the horizontal.
- `gravity: -9.0` — acceleration along Y (m/s²); `drag: 2.0` — resistance (1/s: the speed decays as e^(−drag·t)).
- `size: (0.08, 0.12)` — the size at birth (m); `size_end: 0.0` — the size at the end of life, as a share of the starting size (0 means it vanishes, 1.6 means it grows).
- `color: "#ffffff"` and `color_end` — the colour at birth and at the end (`#rrggbb` or `#rrggbbaa`, sRGB). Without `color_end`, it keeps the same colour and turns transparent. `glow: true` — the particle is added to the picture (sparks, flashes); otherwise it lies on top (dust).
- `shape` — `Quad` (a soft spot, turned towards the camera; the default), `Cube` or `Shard` (a shard): these are lit from above and tumble at a rate of `spin: 360.0` (degrees per second, each particle has its own share of it).
- `offset: (0.0, 1.0, 0.0)` — where they start, as an offset from the entity's feet or from the point; `radius: 0.5` — the particles are born in a sphere of this radius.
- `max_live: 64` — the size of the pool (up to 4096): the effect creates this many meshes once, and it never has more live particles than that.

**The `fx` table.** `effects` maps an engine event to an effect; `cues` maps a clip label (`AnimationCue`: `footstep`, `swoosh`) to an effect, on the body;
a label with no entry plays the effect of its own name, if there is one. Events: `damaged` (on the one who was hit), `blocked`, `parried`, `evaded`,
`guard_broken` (on the defender), `staggered`, `died`, `healed` (on the one it happened to), `telegraph` (on the attacker). An event with an
attacker plays the effect on the side of the body that faces the attacker (at the edge of the collider), and `aim: Hit` sends the particles from there back toward the
attacker; such an effect plays at the point and does not follow the body. The game plays its own effect with the event
`commands.trigger(PlayEffect::new("coin_sparkle", EffectAt::Entity(chest)))` (`shards::fx::{PlayEffect, EffectAt}`) (or `EffectAt::Point`, `.toward(dir)` — the axis for
`aim: Hit`), or with the function `shards::fx::play(world, …)`, which returns how many particles started.

**Particles.** The engine has its own particles, which run on the CPU (`bevy_hanabi` has no version for Bevy 0.20). Each effect has a pool of `max_live` meshes. They are made when the content loads (and again when the effect is changed), and they are hidden while free. A play takes the free ones; when none are free, it takes the oldest. All particles of an effect share one mesh and one material. Each particle's colour is in its `MeshTag`, so Bevy draws them in batches. A particle's position is a function of its age (the flight under gravity and drag is solved with a formula, not with steps), on the **simulation clock** (`Time<Fixed>` and its remainder). A pause freezes the sparks; `step` moves them by as many ticks as have passed; in real time they fly along with the ticks. A snapshot that sets the time back removes the particles that were born "later". The pools and particles are entities without an `AiId`: replay, snapshot and co-op do not see them. A co-op client plays the effects of the events that the host sends it, on its own.

**Telegraph decals.** An attack warning on the ground is a decal under it (Bevy's `ForwardDecal` with the module's zone material, a shader in WESL).
The zone's shape in the decal's square is a circle, a cone sector, a box's rectangle, a line, or a capsule's "stadium". The zone is pale, with stripes at 45° to the world axes, and it gets lighter toward the edge. The stripes get denser as the hit comes (`hatch_far` → `hatch_near`, like the gizmo's hatching) and converge on the point the fill grows from. They are counted from that point, not from the origin of the world, so no more stripes pass that point than fit across the zone, however far from the world origin the zone lies. The copy of the zone grows from its middle (the centre of the footprint, like the gizmo ring) as the hit gets closer (`TelegraphStyle::horizon`, as for the gizmo), with a bright edge. At the moment of the hit it covers the zone and flickers (the flicker follows the accessibility setting `flashes`). The colour is as for the gizmo: warm for an enemy's attack, cold for an ally's. The decal lies on whatever is under it, and fades out by 0.6 m in height from its plane (the ground, and the legs of those standing in the zone, not walls and bodies). While decals are on, the gizmos draw only the outline, the symbol and its stem; the hatching and the growing ring go to the decals (`TelegraphStyle::decals`). Decals need the scene's depth: each camera with `AiCamera` gets a `DepthPrepass` while they are on (and loses it when they are switched off, if the module gave it; the camera's own prepass stays). That is one extra depth pass per frame. `telegraph_decals on=false` brings back the gizmos' fill. Decals also come from a pool: one entity per zone, and the spare ones are hidden.

**For agents.** `effects` (Dev, query) lists the effects (what and how many, colours, life, axis, gravity, pool: live/size), the table of events and cues, and the decals on the ground now: whose attack, shape, size, and how far the fill has grown. `effect_play effect=hit_spark at=e6` (or
`point=[4, 0, -2]`, `toward=[1, 0, 0]` for `aim: Hit`; Dev, control) plays the effect and replies how many particles started and how many are alive in the
pool. `telegraph_decals on=false` (Dev, query; without an argument it turns them on) switches the decals. The setting is the resource `FxSettings`, not part of the
replay or the snapshot.

```text
$ shards act effects
6 effects · 36 particles alive in pools of 448 · telegraph decals on, on the ground: 2
  block_spark  10 shard #bfe6ff → #4d9fff00 · 0.3-0.5 s · aim hit · gravity -12 · pool 0/64
  death_dust   16 cube + 24/s for 0.8s #8c8061cc → #5c554400 · 0.8-1.4 s · gravity -0.6 · pool 0/96
  heal_glow    8 quad + 30/s for 0.6s glow #7dff9e → #2bd96b00 · 0.6-1 s · pool 0/48
  hit_spark    18 quad glow #fff2b3 → #ff6a1a00 · 0.2-0.4 s · aim hit · gravity -9 · pool 36/128
  parry_flash  24 quad glow #ffffff → #ffd24d00 · 0.12-0.22 s · pool 0/64
  step_dust    4 cube #8c806199 → #5c554400 · 0.3-0.5 s · gravity -1 · pool 0/48
events: blocked → block_spark · damaged → hit_spark · died → death_dust · healed → heal_glow · parried → parry_flash
cues: footstep → step_dust
  decal e7 claw: sector 3.1×2.2 m, fill 0.82, hostile
  decal e8 claw: sector 3.1×2.2 m, fill 1.00, hostile
$ shards act effect_play effect=death_dust at=e7
played death_dust at e7 (-5.1, 0.0, 2.8): 16 particles (+20 as it goes on), 16/96 alive in its pool
$ shards act telegraph_decals on=false
telegraph decals off: the gizmos draw the fill
```

Without rendering, `effects` says `6 effects · nothing is drawn headless: effects play in a window or with --offscreen · telegraph
decals on` and lists the effects with their pool sizes, and `effect_play` refuses: ``nothing is drawn headless: effects play in a window
or with --offscreen (effect `hit_spark` is known)``; events then play nothing. For an effect that is not in the content, `effect_play`
suggests the nearest name (``no effect `hit_sprak` (did you mean `hit_spark`?)``), and `PlayEffect` writes to the journal once
(`fx: PlayEffect: no effect …`). `ai/status` has the field `fx`: `{"effects": 6, "live": 36, "decals": 2, "telegraph_decals": true}`. In the developer menu there is the item `effects` ("Effects, their pools and decals", section `world`) and the switch `telegraph_decals` ("Telegraphs as
decals on the ground", section `combat`).

**Checking the content.** `--check-content` and `content_check` name the file, the effect and the field:

```text
content/fx/sandbox.effect.ron: effect `smoke`: one play starts 40 particles and max_live keeps 8: raise max_live to 40 at least, or a play takes back its own first particles
content/fx/sandbox.effect.ron: effect `smoke`: lifetime: (min, max) with min above 0 and at most max, not (0.8, 0.2)
content/fx/sandbox.effect.ron: effect `smoke`: color: `grey` is not a colour: `#rrggbb` or `#rrggbbaa`
content/fx/sandbox.fx.ron: fx `sandbox`: effects: no engine event `damged` (did you mean `damaged`?); events: damaged, blocked, …
content/fx/sandbox.fx.ron: fx `sandbox`: effects.damaged -> unknown effect `hit_sprak` (did you mean `hit_spark`?)
```

Also checked: an effect that starts no particles (neither `count` nor `rate` with `duration`), `spread` outside 0…360, `lift` outside −90…90, negative `drag`, `size_end`, `radius`, `max_live` outside 1…4096, a second `fx` table in the game, an empty cue label. An effect with an error on its first load still plays (like any definition that was read), but within the limits that the check names: the pool is at most 4096 meshes, one play starts at most 1024 particles, a `rate` of at most 4096, and a particle lives at most 30 s. A typo such as `max_live: 4000000000` or `rate: 5e9` does not hang the game; the check names it (`one play starts 4294967295 particles …`).

**How the game uses it.** (1) The effects are in `assets/content/*.effect.ron`, and the table is one `*.fx.ron`. The sandbox has the hit and block sparks (shard particles), the parry flash, the death dust (cubes) and the heal glow. The template has the hit and block sparks and the death dust. (2) Its own moments are `PlayEffect` from the game's code, or the clip labels in `cues`. (3) To see it: `cargo sandbox --offscreen --paused --port 0
--port-file F &`, `shards act god`, `shards step 1800 --until "journal:winds up"`, `shards shot -o shots/fx.png` — in the picture, the decal under the attack and the sparks on the hit.

**What is missing.** Particles run on the CPU, so tens of thousands alive at once are too much for them (an effect's pool is up to 4096). Particles have no textures or animated sprites, no trails, no collisions with the ground, no light from the particles, and no sound (sound is the audio module). Decals are for telegraphs only: there is no general API for spots (blood, tracks). A decal projects straight down, within 0.6 m: on a steep slope its edge fades. Checked on the GPU with `--offscreen` (the `offscreen` test in pixels and the sandbox's screenshots); it has not been checked by eye in a window on a display.

## Hit stop (C2)

When a blow lands, the picture stands still for a beat (60-100 ms). It is what makes a hit feel heavy, and it is a **hold, never slow motion**: the simulation does not pass at a fraction of its speed, and no tick is lost or added. `shards_hitstop` turns the combat events into holds by a table in content, and each machine does with a hold what suits it: alone with a picture it keeps the real-time clock still, in co-op it holds only the pictures of the bodies in the blow, and a headless game, a stepped one and a replay hold nothing at all.

```ron
// crates/sandbox/assets/content/fx/sandbox.hitstop.ron (abbreviated)
{
    "sandbox": (
        max_seconds: 0.12,
        cooldown: 0.0,
        rules: [
            (name: "finisher", when: (event: Damaged, weight_at_least: Finisher), seconds: 0.10),
            (name: "poise break", when: (event: Damaged, weight_at_least: Break), seconds: 0.10),
            (name: "counter", when: (event: Damaged, weight_at_least: Counter), seconds: 0.08),
            (name: "heavy hit", when: (event: Damaged, reaction_at_least: 2), seconds: 0.08),
            (name: "hit", when: (event: Damaged), seconds: 0.06),
            (name: "block", when: (event: Blocked), seconds: 0.06),
            (name: "parry", when: (event: Parried), seconds: 0.10),
            (name: "perfect dodge", when: (event: PerfectEvaded), seconds: 0.10),
        ],
    ),
}
```

**The table (kind `hitstop`, `*.hitstop.ron`, one per game, not simulation content).** The engine has no names from any game: the ladder of a game (which blow holds how long) is its own content, and the sandbox's is a neutral example. `rules` are tried in order and **the first rule that matches an event gives the hold**; an event no rule matches gives none. A rule has an optional `name` (the journal and the tools show it; else `rule 3 damaged`), `when` and `seconds`. `when` asks, all of it at once, of the combat event:

| field | the event has it | matches |
|---|---|---|
| `event` | always | `Damaged`, `Blocked`, `Parried`, `PerfectEvaded`, `GuardBroken`, `Staggered` (left out: any event) |
| `weight_at_least` | `Damaged`, `Blocked`, `GuardBroken` | the hit's `HitWeight` is at least this: `Light < Heavy < Counter < Break < Finisher` (the engine raises it to `Heavy` for a reaction of tier 2 or 3 and to `Break` for a broken poise; a game's hook sets `Counter` and `Finisher`) |
| `reaction_at_least` | `Damaged` | the reaction tier the hit asks of its target |
| `cause` | `Staggered` | `PoiseBroken`, `Parried`, `GuardBroken`, `Dev` |
| `action` | all but `Staggered` | the id of the action that hit |
| `ignored` | `Damaged` | by default a rule matches only a hit that landed; `ignored: true` matches one that was ignored (god mode, i-frames) |
| `involves_local` | all | `true`: only a blow the local player is in (attacker, target or defender) matches; a fight between two others, a hazard tick or a crowd far away then holds nothing. Put it on the rules of a game that should never stutter for the fights around the player (the sandbox does); the setting `hit_stop_local_only` does the same for every rule |

So "finisher, then poise break, then counter, then a heavy reaction, then the rest" is five rules in that order, and a game that wants a different order writes it. `max_seconds` (at most 0.5 s, the engine's own ceiling) bounds every hold whatever a rule says, and `cooldown` (s) keeps a flurry of hits from turning into a stutter: a hold does not start within that time after the last one ended. Holds that overlap **merge by the later end**: a hold asked for while one runs lasts as long as the longer of the two has left. The content check names what cannot work: a second table, a rule longer than `max_seconds`, a rule the ones before it cover (it can never match), a `weight_at_least` on a parry (it has no weight), a `reaction_at_least` or a `cause` on the wrong event.

**What a machine does with a hold.** Every machine that sees the combat events (it simulates them, or it is a co-op client whose host forwards them) triggers `HitStopped { event, rule, name, seconds, bodies, ids }` and writes `hitstop: finisher 0.10 s (damaged) on e5, e6` to the journal as a `local` line (it is this machine's own, never sent to another and never a gameplay event). The observer changes nothing in the world, so it needs no `simulates` guard. Then:

| machine | the hold |
|---|---|
| alone (`NetRole::Single`), a window or `--offscreen`, in real time | the **clock hold** (`ai_core::ClockHold`): the virtual clock's relative speed is forced to 0 for the real time of the hold (not a pause: no pause menu, no muted sound, the gate does not refuse). `Time<Fixed>` gathers nothing, so no tick is due and none is added when it ends; everything on the simulation's clock stands still, the pose and the particles with it; the camera and the input run on real time. A hold lasts its time plus less than a frame. `speed` still reports the player's speed, and a `speed` call during a hold changes what the clock comes back to |
| a co-op host or client | **no clock hold at all**: the picture of each body in the blow stands still (`ai_core::PresentationHolds`, by `AiId`, read by the animation bridge, which pauses the body's `AnimationPlayer` and switches nothing until it ends). A client decides for itself from the combat events its host forwards; a headless host or stand records the same holds, and nothing draws them |
| headless, `--paused`, a `step` (it ends a hold that runs), a replay playing back (`--verify`, also with `--windowed`), a stand's driven clocks, a paused clock, a loading gate | nothing is held. The event and the journal line are still there, and `hitstop` says why nothing was held |

A hold is never part of the simulation: `ClockHold` and `PresentationHolds` are not in snapshots or replays, the `hitstop` kind is outside the replay's content hash, and a replay recorded with the clock held verifies identical with hit stop off, headless and in a window that plays it (a test in `crates/sandbox/tests/sandbox/hitstop.rs`).

**The player's settings** (`settings`, group `accessibility`): `hit_stop` is the scale of every hold, 0 to 1 (0 turns hit stop off; the sandbox's settings menu has a slider), and `hit_stop_local_only` holds only for the blows the local player (`LocalPlayer`) is in, not for the fights around them.

**For agents.** `hitstop` (Dev, query; `last=N` lists the last N holds) shows how this machine holds, the settings, the table with each rule's hold, whether the clock is held now (and for how much longer, and the speed it reads), the pictures held, the totals and, for the clock, the real time it held in all and **the ticks that ran inside a hold (0 when it works)**. `hitstop_play` (Dev, control; not recorded in replays) makes a hold as a blow would: `event=damaged weight=Break attacker=e5 target=e6`, or `seconds=0.08` to skip the table; it answers the rule and what this machine did with it. `ai/status` has `hitstop: {mode, rules, events, clock_holds, picture_holds, skipped, clock_held_ms, ticks_inside, clock_held_now}`, and the developer menu has `hitstop` under combat.

```
$ shards act hitstop_play event=damaged weight=Break attacker=e5 target=e6
poise break 0.10 s (damaged): clock held (0.10 s)
$ shards act hitstop
hit stop: solo: holds the real-time clock (no tick is lost)
...
clock: 0.117 s held in all over 1 holds · ticks that ran inside a hold: 0 · the last: 0.117 s real, 0 ticks inside
```

**Limits.** In co-op only the animation of the bodies stands still; the interpolation of a client's other bodies and what the pictures do on their own clocks (camera, UI) run on. The held time is the time asked for plus up to about two frames: it starts from the start of the frame in which the blow is seen and is checked at the start of each frame (a measured 0.5 s hold took 0.51 s). The `Time<Virtual>` of a held clock is 0, so presentation that runs on it (a particle's life, `Res<Time>` in `Update`) stands still with the world; what must not stand still reads `Time<Real>`.

**Deviations from the design, on purpose.** (1) A co-op picture hold freezes the animation only; the position of a held client body still follows interpolation (`shards_net/src/interp.rs` is not touched in C2). A body in a blow can slide while its pose stands still. Follow-up for the net lane (M8/M9): hold a body's interpolation by its `AiId` as well. (2) The hold's systems are unnamed `First` systems in `ai_core` and the plugin has no `SystemSet` of its own: nothing orders against them, the hold is not simulation. (3) No reference replay is recorded with hit stop on: a hold changes no tick, so a recording is the same file with or without it; the test in `crates/sandbox/tests/sandbox/hitstop.rs` records one while the clock is held and verifies it with hit stop off, headless and in a window. (4) `hitstop_play seconds=` keeps the table's `max_seconds` and is journalled as `manual`.

## Voxel terrain (core)

`shards_voxel` (G1, PR A2b of the voxel design) is the data half of voxel terrain: a region's ground as a box of blocks that
generation builds and play never changes. It has no region kind, no collision, no navigation and no meshing yet (the later PRs
add them on top of this core), so a game that does not use it pays for a content kind and a registry and nothing else: the
module adds no system. `shards::voxel` re-exports it; `voxel` is a module a game may leave out (`--without voxel`).

### The volume

`VoxelVolume::new(size, block, origin, chunk)`: `size` cells, each `block` m on a side, cell (0, 0, 0) with its least corner at
`origin`, stored in chunks of `chunk` cells (a parameter, not a constant: the port uses 64 x 32 x 64). `size` must be a whole
number of chunks on every axis, no side zero, at most 65 535 cells tall and 2^27 cells in all; `block` above 0 and finite;
each is an error with its numbers. `VoxelVolume::origin_for_anchor(anchor, block)` gives the origin that puts a lattice point
at the world origin ("this cell's top is world y = 0").

- A cell is a block id, a byte: 0 is air (`AIR`), 1 to 255 are the blocks of a palette. Outside the volume every read is air
  and every write is ignored, so steps never clip. A chunk that holds one block only is one byte (`compact()`); the others hold
  a byte per cell, y fastest.
- Queries: `get`, `inside`, `cell_at(world)`, `corner(cell)`, `count(filter)`, `surface(x, z)`, `column_top(x, z, filter)` and
  `column_runs(x, z, filter)` (the `[y0, y1)` runs of a column, bottom to top; a cave under a hill is two). Whether a block is
  solid, liquid or foliage is the palette's (`VoxelPalette::solid`, `liquid`, `foliage`); pass the answer as the filter.
- The **surface** is the natural ground of a column, a layer count: set by `raise_column` and `lower_column`, not by carving or
  filling. Rules ask it ("where does water stand?"); `column_top` is the real top, which a tunnel or a tower changes.
- Edits, for generation steps (all in cell coordinates, each clipped to the volume): `set`, `fill(min, max, id)` (max
  exclusive), `carve(min, max)` (to air), `clear(min, max, filter)` (only the blocks the filter picks, "clear the trees near
  the arena"), `sphere(center, radius, id)`, `tube(a, b, radius, id)`, `raise_column(x, z, h, fill, top)`,
  `lower_column(x, z, h, top)`, `flatten(center, radius, h, fill, top)`. A column is never lower than one layer. Once a region
  is built the volume is read-only. (The design's `box` is `fill`: `box` is a Rust keyword.)
- `hash()`: FNV-1a over a canonical form: the size, the chunk size, the block size and origin as `f32` bits, every cell chunk by
  chunk in chunk-index order (a uniform chunk hashes as its dense form, so `compact()` changes nothing) and every surface. No
  hash map, pointer or clock goes in, so it is the same on every run. Its golden test changes only when the canonical form
  does.

### Palettes: `*.voxel_palette.ron`

```ron
{
    "island": (
        blocks: [
            (name: "grass",  top: "#5b8f3a", side: "#6f5a3c", bottom: "#5c4730", tags: ["ground"]),
            (name: "dirt",   top: "#6f5a3c"),
            (name: "leaves", top: "#3f7f2f", solid: false, foliage: true, tags: ["tree"]),
            (name: "glow",   top: "#ffd27f", emissive: true),
            (name: "water",  top: "#2a6f9f", solid: false, liquid: true),
        ],
        shade: (up: 1.0, down: 0.5, x: 0.75, z: 0.85),
        ao: (both: 0.5, step: 0.2),
        variation: (block: 0.0, patch: 0.0),
        meshing: Faces,
        look: "voxel_terrain",
        foliage_look: "voxel_foliage",
        water_look: "voxel_water",
    ),
}
```

| field | meaning |
|---|---|
| `blocks` | at most 255, at least 1; a block's id is its place plus one. Names are unique, not empty, not `air` |
| block `top`, `side`, `bottom` | colours (`#rgb`, `#rrggbb`; sRGB). `side` defaults to `top`, `bottom` to `side` |
| block `solid` (true) | bodies stand on it (physics, navigation). A `liquid` or `foliage` block must say `solid: false` (an error, not silently fixed), and a block is not both |
| block `foliage`, `liquid`, `emissive` | foliage mesh and sight and bodies pass through; water (never meshed as a block); lit by itself |
| block `tags` | free words for a game's rules; `VoxelPalette::tagged(tag)` lists the ids |
| `shade` | brightness of the faces, 0 to 1: `up` 1, `down` 0.5, `x` 0.75, `z` 0.85 by default (neutral numbers; a game sets its own) |
| `ao` | corner darkening, 0 to 1: `both` (two filled sides) 0.5, else `1 - step * (side1 + side2 + corner)` with `step` 0.2 |
| `variation` | colour variation per block and per patch, 0 to 1, default none |
| `meshing` | `Faces` (default; exact corner darkening) or `Greedy` |
| `look`, `foliage_look`, `water_look` | looks (`*.look.ron`) of the terrain, foliage and water meshes, if set; a missing one is an error with the closest name |

`VoxelPalette::from_def(&def)` (or `from_db(&db, id)`) builds the runtime palette: flat tables by id (`solid`, `liquid`,
`foliage`, `emissive`, `visible`, `occludes`), `id(name)`, `name(id)`, `face_color(id, dir)`. The same checks run in `validate`
(so `--check-content` and `content_reload` report them with the file) and in `from_def` (a palette built in code).

### Noise

`Noise::new(seed, frequency)` with `octaves`, `lacunarity` and `gain`: value noise on an integer lattice hashed with SplitMix64,
a quintic fade, octaves summed; `sample(x, z)` in -1 to 1, `sample01` in 0 to 1. Only `+ - * /`, `floor` and integer maths, so
the same seed gives the same bits on every OS (golden values are tested). Also `smoothstep` (products only), `lerp`, `fade`,
`cell_hash`. Another engine's noise (FastNoiseLite) is matched by look, not by value.

### Generation: steps, checks, `generate`

Generation is a pure function of (seed, region version, step list with versions, params, palette). Randomness is `DetRng`, never
`SimRng`; ordered maps only.

- **Steps** are game code registered by name and version: `app.add_voxel_step("hills", 1, hills)` with
  `fn(&mut VoxelGen) -> Result<(), String>`, in `VoxelSteps` (a resource; the plugin adds it with the stock steps). A name is
  lowercase letters, digits and `_`; a name taken twice, or version 0, panics at build time (`VoxelSteps::add_step` returns the
  error). A region lists `steps: [("hills", 1), ...]` and the version it expects: `VoxelSteps::validate_steps` reports an unknown
  step (with the closest name), a version other than the registered one, and an empty list, with the index, so it can be run from
  a content `validate`. The co-op handshake compares the content hash and not step code, so a step's output changing means
  raising its registered version and the content's.
- **`VoxelGen`** gives a step `volume()`/`volume_mut()`, `palette()`, `params::<T>()` (the step's entry of `params`, a `serde_json::Value`, the same type the engine's tool arguments use; it is part of the API on purpose: a region's params are free-form data, a JSON
  value; a step without an entry reads an empty map), `rng()` (`DetRng::from_keys(&[domain, seed, version, attempt, step_index])`, the domain a tag that keeps step, keyed and noise streams apart; every
  call starts the same stream, take it once), `rng_keyed(key)`, `noise(salt, frequency)` (coordinates in cells; the same salt
  gives the same field in every step of an attempt), `mark(name, world_pos)` (the spawn, points of interest; a name used again
  replaces the point), `remark(text)`, `seed()`, `version()`, `attempt()`.
- **Checks**: `app.add_voxel_check("flat", fn(&VoxelGen) -> Result<(), String>)` run after the steps, in the order a region
  names them; a failed step or check fails the attempt and the next attempt has other rng keys. After `attempts` (1 to 64)
  `generate` fails with every attempt's reason.
- `generate(&steps, &palette, &GenSpec)` returns `Generated { volume, marks, remarks, attempt, steps, hash }`. `GenSpec::problems`
  reports a bad spec whole (volume shape, attempts, unknown steps and checks, a `params` key that names no listed step).
  `hash` is `world_hash(volume.hash(), palette, steps)`: the volume's hash with the palette fingerprint and the step list with versions folded in, so the same cells
  made by another step set (or read through another palette) are another world.
- **Stock steps**: `noise_hills { top, fill, base, amplitude, frequency, octaves, salt }` (column heights `base + amplitude *
  noise`, laid with `raise_column`; the block names are the palette's) and `fill_water { level, block }` (liquid from each
  column's natural surface up to `level` over air cells). They take every number from params; the engine has no number of a
  particular game.

### Merged boxes

`column_boxes(&volume, keep)`: the runs of `keep` cells per column, equal runs of neighbouring columns merged into rectangles; a
box's top and bottom are the real ones, so what reads a collider's bounding box stays true. It is the fallback collision
(one static collider per box) and the reference the voxel field is measured against. `water_boxes(&volume, liquid, solid)`:
liquid runs reach down through the ground and cross columns that are solid through the run, so a lake over a bowl is a few
boxes whose tops are the surface; air under water stops a box. A box ends on a column with a run of its own (it is not stretched over a hill to the edge), and each row it takes has one. Every `keep` (liquid) cell is in exactly one box, and a water box
holds only liquid and solid cells. Both are tested against a brute-force reference over seeded random volumes.

### What is not here yet

The region kind and spawning, the voxel field in `shards_motion`, the current and sea boxes, the volume hash in replay and
snapshot checks (`voxel.volume`), navigation, meshing and the AI tools are the next PRs of the design; this core exports what
they need (`VoxelVolume`, `VoxelPalette`, `generate`, `world_hash`, the boxes).

**Limits.** A shape is checked without being built (`VoxelVolume::check_shape`, also what `GenSpec::problems` calls): at most 2^27 cells, 65 536 chunks and 65 535 cells tall, so a content file with a tiny `chunk` is an error that says to use a bigger one, not an out-of-memory. `sphere`, `tube` and `flatten` are clipped to the volume, so a huge radius from a param costs the volume and not its box. `chunk_uniform` and `chunk_cells` let a mesher skip a one-block chunk without reading its cells. `world_hash` folds in the volume, the palette (`VoxelPalette::fingerprint`: block order, names, flags, colours) and the step list. Block colours are opaque (`#rrggbbaa` with an alpha is a content error). The finished volume is read-only by convention: hand out `&VoxelVolume`.

## Steam (E10.5)

The `shards_steam` module adds Steam to the game: co-op over the Steam relay (a lobby with invitations, or a game server), achievements from the content, and saves in Steam Cloud. It is behind the umbrella crate's `steam` feature and is off by default: the template and the sandbox build without it, and their `ai/status` has no `steam` module. Nothing that affects gameplay depends on Steam. A session over the relay is the same `shards_net` session as over UDP, an achievement does not change the world, and the copy in the cloud is only a copy of the file on disk.

```toml
# the game's Cargo.toml
shards = { git = "…", tag = "…", features = ["steam_sdk"] }   # the real Steam client
# features = ["steam"] — the module with a fake Steam process: for the game's tests, without steam_api64.dll
```

- **`steam`** — the module, its tools, and the `achievement` content. Steam in it is fake (`shards_steam::FakeSteam`: users, lobbies, invitations, a relay on `MemoryHub` with its delay and losses, achievements, cloud), and a game that a player started answers `steam: unavailable: this build has no Steamworks SDK …`.
- **`steam_sdk`** — the real client as well: `steamworks = "=0.13.1"` (the SDK is in the `steamworks-sys` crate, so nothing needs downloading). The program imports `steam_api64.dll`, and without that file beside it, it does not start (`shards package` puts it there). Only a running game connects to Steam (`shards::run`): `--check-content`, `--verify`, `--smoke`, the tests, and a stand in one process all run on the fake Steam. No `steamworks` types appear in the public API. Such a game starts the Steam client earlier than Bevy creates the renderer (`shards_steam::SteamClientPlugin` goes into `EnginePlugins` right after `TaskPoolPlugin`): the Steam overlay (invitations, the achievement popup, Shift+Tab) attaches to the graphics device while it is being created. A test checks the order of the plugins; whether the overlay really appears in the window has not been checked.

### Sessions over the relay

| flag | what it does |
|---|---|
| `--steam-host` | hosts in a new lobby for `max_players` players (from `NetDef`), by default "friends only". The lobby data holds `game`, `engine`, `content` and `rules` (the build: what the host's `hello` checks), and `host`; lobbies are searched by them (`steam_lobbies`). Friends are invited with `steam_invite friend=ID` or with the Steam overlay |
| `--steam-lobby TYPE` | with `--steam-host`: who sees the lobby, as Steam lobby types. `friends` (the default: friends see it and join), `private` (joining is only possible by invitation), `invisible` (friends do not see it, search finds it), `public` (friends see it, and search finds it). `friends` and `private` are never found by search. The journal names the type (`steam: hosting in lobby … (private: invite only): …`), `steam` (`lobby … (friends only)`) and `ai/status` (`lobby_type`). Without `--steam-host` (including with `--steam-server`) the flag is a parse error |
| `--steam-server` | the host is an anonymous Steam game server (`127.0.0.1`, ports 27215 and 27216, no authentication): no lobby and no Steam user, so it runs beside a Steam client on the same machine. It is a host like any other: it has its own player 0 with a body (`shards_net` has no dedicated server without a player). Its id (type 4) is named by the journal (`steam: game server logged on as 90071992547409921: players join with --join steam:90071992547409921`), `steam` and `ai/status` |
| `--join steam:<id>` | a lobby id (type 8) joins the lobby; its host is the owner; a player or server id (types 1, 3, 4) connects to the host directly, without a lobby. Any other type is an error, and the error gives its number |
| `--join steam:` | wait for an invitation (or `steam_join`) |
| `+connect_lobby <id>` | Steam starts the game this way for a friend's lobby; `shards::run` turns it into `--join steam:<id>` before the flags are parsed |
| `--steam-relay-only` | only the Steam relay: direct routes (ICE) are off |
| `--steam-app N` | the game's app id (`480` is Valve's test app), when the game was not started by Steam: it sets `SteamAppId` and `SteamGameId`. The engine does not write `steam_appid.txt` and does not put it in the package |

No UDP socket is opened in this case. The session's transport is `ISteamNetworkingMessages`, channel 0, unreliable messages with Nagle's algorithm off, and a dropped session is restarted. Reliability, ordering and blobs, as over UDP, belong to the `shards_net` link layer, and `--net-sim` runs on top of the relay. The relay accepts a session only from those who may play: a lobby host accepts it from the lobby's members, a game server from everyone who knows its id (the session's join rules decide after that), and a client only from its own host. A refusal goes to the journal (`steam: refused a relay session from intruder (76561197960265731): not in lobby 109775240917319681`) once per peer, and the count goes to `steam`. The ready line names the session: `role host · net steam lobby`, `role host · net steam game server`, `role client · net steam:109775240917319681`.

If the session does not open, the host writes `steam: cannot host: …` (`NetState::Ended`; a lobby host tries again with the `steam_host` tool), and with `--exit-with-host` it exits with code 2. A client that did not join the lobby (it does not exist, or it is full) writes the reason and waits for another (`steam_join`, an invitation). Without Steam its session ends (`NetState::Ended`), and with `--exit-with-host` it exits with code 2. The same happens to a game with the `steam` feature but without the SDK when Steam starts it with `+connect_lobby`. An invitation that arrives while the game is in a session does not pull it out of that session: `steam` shows it as the line `invite not followed`. A game without the `steam` feature answers the Steam flags with code 2 and says which feature to turn on. It does not know `+connect_lobby` at all, and the flag parser (clap) answers: `error:
unexpected argument '+connect_lobby' found`, also code 2.

If a Steam callback handler panics, Steam's callbacks stop until the end of the run: the panic poisons the lock inside `steamworks`. The journal writes one `error` line (`steam: a Steam callback panicked (…), so Steam's callbacks stop for this run:
lobbies, invites, relay sessions and achievement stores no longer answer; restart the game`), `steam` — the line `problem: …`,
`ai/status` — the same in `note`. A host that was waiting for a lobby, and a client that was joining one, get no answer: they write `cannot
host` or a join error at once; `steam_host` and `steam_join` answer with the same reason. There is no crash report: the panic was caught. For the callbacks whose
parsing in `steamworks` 0.13 may fail on a value from Steam (an `EResult` code missing from its table, or several bits set in a lobby member change), the engine reads that value itself, as numbers.

### Finding a lobby

A player joins by invitation or by lobby id. `steam_lobbies` adds a third way: find this game's lobbies yourself. The host writes these into the lobby's data: `game` (the game's name and version, `NetGame`), `engine` (the engine version), `content` (the content hash, `ContentDb::hash`), `rules` (the hash of the network rules, `net_rules::rules_hash`; both in hexadecimal) and `host` (its own name on Steam). The host's `hello` checks the first four when a client joins, so a lobby of "this build" is one the host lets the client into. The search (`ISteamMatchmaking::RequestLobbyList`) asks Steam for lobbies whose `game`, `engine`, `content` and `rules` equal its own, with a free slot (`AddRequestLobbyListFilterSlotsAvailable(1)`), worldwide (`k_ELobbyDistanceFilterWorldwide`: friends may be anywhere), and at most 50. With `all=true` only `game` is compared, and a lobby from another build in the answer says how it differs (`build differs: engine 0.8.0 (yours 0.9.0), content … (yours …)`): the host of such a lobby would refuse `hello`.

```text
$ shards --peer c1 act steam_lobbies          # test lobbies.rs: fake Steam, host with --steam-lobby invisible
1 lobby of `game` of this build with a free slot:
lobby 109775240917319681 · host host (76561197960265729) · 1 of 3 · build matches
enter one with steam_join lobby=ID
$ shards act steam_lobbies                    # host with the default lobby (friends)
no lobby of `game` of this build with a free slot answers the search
Steam's search lists public and invisible lobbies only: a host started with --steam-lobby friends (the default) or private is never in it
all=true lists the lobbies of other builds of this game too
note: this game's own lobby 109775240917319681 (friends) is not in the answer: Steam's search never lists friends-only or private lobbies; host with --steam-lobby public or invisible to be found
```

- **Which lobbies Steam finds.** Only `public` and `invisible`. `friends` and `private` never appear in the search, and neither does the host's own lobby (Steam's documentation says so; the fake behaves the same way). When the host's lobby is not in the answer, the answer says why: its type is `friends` or `private` (start the host with `--steam-lobby public` or `invisible`), or there is no free slot. Such a lobby is not closed to friends: people join it by invitation or by id (`steam_join`, `--join steam:<id>`). The search just does not show it.
- **Waiting for the answer.** Steam answers with a callback, later. The tool runs the callbacks itself (as the game's frame does) until the answer comes or `wait_ms` runs out (by default, and at most 5000: waiting stalls the game's frame, the picture, the session traffic and the simulation, which run in real time, and the session's silence timeout is 10 s by default). If no answer comes in time, it returns the error `Steam has
  not answered the lobby search within … ms; it goes on: call steam_lobbies again for its answer`, and the search is not dropped: the callbacks and the game's frames keep running, and an answer that arrives later waits for the next call (that call does not search again). A second search does not start while the first is still running. With a different `all`, the call refuses and names the one to wait for.
- **While the content is not loaded**, the build is unknown (the content hash is incomplete): `steam_lobbies` answers `this game is still loading
  its content`.
- Without Steam, and on a game server, the answer is the reason (`Steam is unavailable (…)`, `an anonymous game server has no lobbies, …`).
- **Stand.** The search also runs on a stand on top of the fake Steam (`cargo run -p sandbox --features shards/steam -- --stand 1 --stand-steam
  --steam-lobby invisible --headless --paused --port 0 --port-file F &`, see "Stand on top of the fake Steam" in "Co-op session (E7, `shards_net`)"): the host makes an invisible lobby, the client waits, searches, joins by the id from the answer, and plays. The scenario is `crates/shards_steam/scenarios/lobby_search.scenario`: in the test (`tests/steam/scenarios.rs`) it runs on the stand that the test builds, and against a running game it runs as `SHARDS_PORT_FILE=F shards scenario crates/shards_steam/scenarios/lobby_search.scenario`. The lobby in it is always the fake Steam's first lobby, `109775240917319681`: the scenario names its id.

### Achievements

Content `*.achievement.ron`. The name and description are Fluent messages (`@ach-…`), and the player reads them in their own language:

```ron
{
    "first_blood": (steam: "ACH_FIRST_BLOOD", name: "@ach-first-blood", description: "@ach-first-blood-desc"),
    "warden_down": (steam: "ACH_WARDEN", name: "@ach-warden", description: "@ach-warden-desc", hidden: true),
}
```

- `steam` — the API name in Steamworks (letters, digits, `_`, up to 128); one name is one achievement. `hidden` — Steam shows it only when it is unlocked. The content check names an empty or wrong API name, a repeated one, text in place of a message, and a message that is missing in some language (with `did you mean`).
- The game unlocks it where its simulation decides: `commands.trigger(UnlockAchievement::everyone("first_blood"))` (or `UnlockAchievement::player(id, n)` — for one player), or `shards::steam::unlock_achievement(world, id)` — for the player on this machine. In co-op the event goes from the host to the clients, and each machine unlocks the achievement for its own player on its own account. A client does not unlock on its own (`unlock_achievement` and the tool give it an error). Nothing is unlocked during a replay.
- Steam learns of an unlock at once. The write (`StoreStats`; the overlay shows the achievement after it) follows one second after the first unlock, one write covers several unlocks, and there is one more at exit. A write that Steam refused is retried one second later and at exit (the journal warns once per cause). An achievement that Steam already knows from a previous run is not written again.
- This is not simulation state. The journal has a `local` line (`achievement unlocked: First blood (first_blood)`). The `Achievements` resource is the record of this run (it exists without Steam too). Neither the replay hash nor the snapshot has them.

### Cloud

Every save that is written (a slot's new main file, the event `shards_save::SaveWritten`) is copied to Steam Cloud as
`<saves folder>/<kind>/<slot>.save`, for example `saves/world/slot1.save`. The `.bak`, `.tmp` and `.damaged` copies are never copied. At
startup, in `PreStartup`, before `--world` and `--character` load a slot, each cloud copy is compared with the file on disk:

| case | what happens |
|---|---|
| identical bytes | nothing |
| the cloud copy is newer by more than 2 s (or there is no file) | it becomes the save, and the old one becomes the first copy (`slot1.save.bak1`) |
| the file on disk is newer | it goes to the cloud |
| neither is clearly newer | the file on disk stays, the cloud copy is put next to it as the slot `<slot>-cloud`, and the journal (`warn`) says so; the cloud gets the file from disk |
| the cloud copy is broken | it is not taken; a good file on disk repairs it |
| the cloud copy is from a newer version of the game (format `shards-save/2`, or a section newer than this build) | it is not taken and not overwritten: in this run nothing is copied to this cloud slot, even a new save. Journal (`warn`): `steam: the cloud copy of world/slot1 is from a newer version of the game (…): this build neither takes it nor overwrites it, …`; `steam`: `at startup from a newer version of the game, left alone: world/slot1` |
| the file on disk is from a newer version of the game | the cloud copy does not land on top of it, just as a save would not (`steam: the cloud copy of world/slot1 was not taken: world/slot1 was saved by a newer version …`) |
| the save exists only on disk | it goes to the cloud (except `…-cloud` slots) |

The cloud copy is written to disk in the same write as the save (`write_guarded`). It does not touch a slot that holds a save from a newer version (the main file or its copy). While a replay plays, the cloud is not compared and not written (`cloud: off
(not used while a replay plays: the saves on this machine stay as they are)`): the saves on disk stay as they are.

The files on disk stay the source of truth: a copy that the cloud did not accept (quota, or Steam Cloud turned off) changes nothing here, and `steam` says why. A file goes to the cloud in one call to `ISteamRemoteStorage::FileWrite` (all of it or nothing, up to 100 MB), and its reply is checked. When there is not enough quota, the error says how much room there is: `Steam Cloud has 105 B free of 4 KB; this save is
540 B` (the room that the replaced file frees counts as free; how Steam counts it is not described). The `cloud` line in `steam` shows the free room: `cloud: on · 3 saves copied as they were written · 2 KB free of 4 KB`.

On exit, the Steam hook runs after the session and save hooks. First the copies of what they wrote go to the cloud (the character that the host hands back to a leaving client, the deferred autosaves). Then achievements are written, the relay sessions with peers are closed (`CloseSessionWithUser`; a peer sees `the relay session with … failed: closed by the peer`), and the game leaves the lobby. Steam itself (`SteamAPI_Shutdown`) shuts down when the game world that holds the last `steamworks` object is removed. The order is checked on the fake Steam.

### For agents

```text
$ shards act steam          # lobby host (test tools.rs: fake Steam, client c1, 20 ms one way)
steam: a fake Steam in this process (this run does not talk to Steam) · user host (76561197960265729)
session: host · lobby 109775240917319681 (friends only), host host (76561197960265729) · 2 in it: host (76561197960265729), c1 (76561197960265730)
relay: ready (fake: an in-process hub) · sent 36 (5.5 KB), received 37 (3.2 KB)
route to c1 (76561197960265730): connected · relayed via fake · ping 40 ms
achievements: 2 defined, 1 unlocked in this run: first_blood (First blood) at tick 30 · stored on Steam 0 times, a store waits
cloud: on · 0 saves copied as they were written
$ shards --peer c1 act steam
steam: a fake Steam in this process (this run does not talk to Steam) · user c1 (76561197960265730)
session: client · lobby 109775240917319681, host host (76561197960265729) · 2 in it: host (76561197960265729), c1 (76561197960265730)
relay: ready (fake: an in-process hub) · sent 39 (3.4 KB), received 34 (5.2 KB)
route to host (76561197960265729): connected · relayed via fake · ping 40 ms
achievements: 2 defined, 1 unlocked in this run: first_blood (First blood) at tick 41 · stored on Steam 0 times, a store waits
cloud: on · 0 saves copied as they were written
```

For a game server, the first line is `steam: an anonymous Steam game server · id 90071992547409921 · app 480`, and the second is `session: game
server · serving as 90071992547409921: players join with --join steam:90071992547409921`. On real Steam, the `route to` line comes from `GetSessionConnectionInfo`: `connected · relayed via <pop> · ping 16 ms` (`<pop>` is the relay's data centre code, `direct` means no relay).

- `steam` (Player, query) — who this machine is in Steam (the Steam client as a player, an anonymous game server, a fake Steam in this process, or "unavailable" with the reason), a `problem:` line if the Steam callbacks have stopped, the session (for a lobby host, also the lobby type), the relay (what was sent and received, send errors, refused sessions, `relay only`), the route to each peer, invitations that the game did not follow, the achievements of this run and the writes to Steam, the cloud (free room, what was done at startup, the last error).
- `steam_lobbies [all=true] [wait_ms=N]` (Player, query) — searches for this game's lobbies that can be joined (see "Finding a lobby"): a line per lobby with its id, its host, how many are in it and how many places there are, and whether the build matches. It waits for Steam's reply, by default and for no more than 5 s.
- `steam_host` (Dev, control) — the lobby host creates the lobby again after a failure; the reply says whether the host got back in. A running game does not become a host: only the flags make it one.
- `steam_join lobby=ID` or `host=ID` (Dev, control) — a client started with `--join steam:`, or one that has not joined its session, joins a lobby (the id comes from `steam_lobbies`, from `steam` or from the host's journal) or joins the host. A game that is already in a session does not change it. For a lobby owned by this same Steam account (a second game on the same account as the host), the join is refused and the game says so: `cannot enter Steam
  lobby <id>: this Steam account owns the lobby: a second game on the same account cannot join it (join from another
  account)`; `the lobby has no host (its owner left)` is still used for a lobby whose owner Steam did not name.
- `steam_invite friend=ID` (Player, control) — invites a friend to the lobby; without `friend`, the overlay's invite window opens.
- `achievement_unlock id=… [player=N]` (Dev, control, host only) — unlocks it as the game would; on the host, the event goes to the clients too.
- `ai/status` — the `steam` field: `kind`, `user`, `note` (why there is no Steam or why its callbacks stopped), `role`, `lobby`, `lobby_type` (`friends`, `private`, `invisible`, `public`; only for a lobby host), `host`, `members`, `relay` (`sent`, `received`, `errors`, `relay_only`, `routes`: `peer`, `state`, `relayed`, `pop`, `ping_ms`), `achievements`, `cloud`. The developer menu has a `steam` section: `steam`, `steam_lobbies`, `steam_invite`, `steam_join`, `achievement_unlock`.

### What was checked on real Steam

Once, on 2026-10-16, on the test app 480 with one account, with this branch's build before the fixes below (the sandbox with `steam_sdk`, both games on one machine):

- a game server (`--steam-server --steam-relay-only`) and a player who joined it by id (`--join steam:<server id> --steam-relay-only`), only through the relay: `route to …: connected · relayed via <pop> · ping 16 ms` on the client and 17 ms on the server, the client is player 1, 0 desyncs, and after a move order on the client both machines see the body in the same place;
- an achievement unlocked on the server (`achievement_unlock`) went to the client, which unlocked it on its own account and wrote it (`stored on Steam
  1 times`), and Steam showed it as unlocked on the account;
- the game does not take, at startup, a cloud copy that is not a save (`the cloud copy of … is not used (no header …)`);
- app 480's Steam Cloud quota is 4 KB for everything, and about 100 bytes were free: no save copy was written. That is why the quota message above exists: the old one (`other error (quota, or Steam Cloud off)`) did not name the cause.

Second time, the same day, with the build that has the fixes below (the cloud write as one `FileWrite`, the quota message, closing the relay sessions and the order on exit, the lobby type, starting the Steam client before rendering, the game's own callback decoders):

- a host with `--steam-host --steam-lobby private`: Steam created the lobby, `steam` writes `lobby <id> (private: invite only)`, the journal says `hosting in lobby <id> (private: invite only): players come in only through an invite (steam_invite)`, and `ai/status` shows `"lobby_type": "private"`; joining it with a second account was not tried (there is none);
- a save copied to the cloud with 105 free bytes: `the cloud copy of world/cloud1 was not written: Steam Cloud has 105 B
  free of 4 KB; this save is 292 B; the save on this machine is fine`, and the save on disk is in place; on exit, the same lines for the copies that the exit hooks make;
- an achievement on a lobby host was unlocked, and it was written on exit: Steam showed it as unlocked on the account;
- the game server and the player by id again only through the relay: `relayed via <pop> · ping 18 ms`, 0 desyncs, the body in the same place on both machines; the server sees the client leave (`p1 left (quit)`).

On the client's exit, `steam_api` still prints, even after the fixes, `ipcclient.cpp (98) : !"Invalid pipe handle specified"` and `IPC call to IClientNetworkingUtilsSerialized::PostConnectionStateMsg failed to read result code` — after the game's last line, once the sessions are closed. A program that only connects to Steam through the relay and closes nothing prints the same: this message comes from the Steam library itself when the process exits, and it does not hurt the game or the server. Not checked in a window: the overlay (invitations, the pop-up achievement) and that the Steam client now starts before rendering.

Third time, 2026-10-17, with the build that has lobby search (app 480, one account, one machine, both games are the sandbox with `steam_sdk`; the host is `--steam-host`, the second game is `--join steam:`, see "Finding a lobby"):

- a host with `--steam-lobby invisible`: the second game on the same account found its lobby with the `steam_lobbies` tool (`1 lobby of … of this build with a free slot: lobby <id> · host <name> (<id>) · 1 of 3 · build matches`), and with `all=true` too; the host's own reply marks its lobby `this game's lobby`;
- a host with `--steam-lobby friends`: the search found nothing, and the host's reply has a `note:` saying that the lobby is for friends only and that a private search never shows it;
- Steam returned the lobby size as a number (`1 of 3`), not as `of ?`;
- `steam_join lobby=<id>` from the second game on the same account was refused: Steam let it into the lobby and named this same account as the owner, so there was no one to play with. The journal then wrote `the lobby has no host (its owner left)`, although the owner had not left; now it writes `this Steam account owns the lobby: a second game on the same account cannot join it (join from another account)` (checked on real Steam too);
- Steam keeps lobby membership per account. The previous version left the lobby after the refusal, and the host on the same account left it too (its next `steam_lobbies` no longer found the lobby, although the host's session still showed it). Now, on a refusal for "the same account", the game does not leave the lobby: after the refusal, and after the second game closed, the host's search still finds its lobby (`this game's lobby`). The account's state (achievements and cloud files) after the runs matched the starting state.

Joining a lobby with a second account was not checked (there is none), and neither was a search for a `public` lobby.

### What was checked only with the fake

The tests (`crates/shards_steam/tests/steam/`, unit tests in `src/`) run on `FakeSteam`: a lobby, a host and two friends over the stand's relay, the lobby type, lobby search (filters on the data: `public` and `invisible` lobbies are in the reply, `friends` and `private` ones are not, a full lobby, a slow Steam and one that refuses, a foreign build), a game server and a player who joined by id, an invitation, a refusal to a stranger (to the host and to the client), a failed join (no lobby, its own id, an id of the wrong type, `--exit-with-host` with code 2) and a join after it through `steam_join` (into a lobby and to the server), a lobby that Steam did not create (and `steam_host` after it), a server that did not sign in to Steam, a game without Steam (host and client), callbacks that failed, exit (copies to the cloud, closed sessions, leaving the lobby, in this order), achievements (content check, a write a second later, a previous run, a name Steam does not know, replay, co-op), the cloud (a copy of each write, the cloud turned off, a check at startup before the slot loads, copies from a newer version, replay, quota), tool replies and `ai/status`. The fake does not know the entry rules for each lobby type. Those rules are Steam's, as its documentation describes them: `private` lobbies are joined by invitation only, and `invisible` ones are hidden from friends. The engine only passes the type. The fake repeats, from the same documentation, that search finds `public` and `invisible` lobbies and does not find `friends` or `private` ones. In the tests, the real backend (`src/real.rs`) is only compiled and passes clippy (`cargo clippy -p shards_steam --all-targets --features sdk`). These are checked only on real Steam: lobbies of any type and joining them (`private`, `invisible` and `friends` lobbies existed on real Steam, but no second account joined them), lobby search (from one account, `invisible` was checked and is found, and `friends` is checked and is not; `public`, the distance filter and a second machine are not checked), `+connect_lobby`, the overlay (invitations, the pop-up achievement) in a window, writes to Steam Cloud for an app where there is enough quota, and the spec's acceptance check "two players connect through Steam relay" on two machines.

**Manual check on one machine with one account** (the Steam client is running and signed in; test app 480; the game is built with `steam_sdk`, for example the sandbox with `--features shards/steam_sdk`):

```bash
# 1. Host: an anonymous game server, relay only:
cargo run -p sandbox --features shards/steam_sdk -- --headless --steam-server --steam-app 480 --steam-relay-only --port 0 --port-file /tmp/srv.port &
SHARDS_PORT_FILE=/tmp/srv.port cargo shards act steam    # line 1: the server's id, 9007…
# 2. Client: a Steam user, by the server's id:
cargo run -p sandbox --features shards/steam_sdk -- --headless --join steam:<server id> --steam-app 480 --steam-relay-only --port 0 --port-file /tmp/cli.port &
SHARDS_PORT_FILE=/tmp/cli.port cargo shards act steam    # route to …: connected · relayed via <code> · ping N ms
SHARDS_PORT_FILE=/tmp/cli.port cargo shards status       # net: connected, player 1
SHARDS_PORT_FILE=/tmp/cli.port cargo shards quit; SHARDS_PORT_FILE=/tmp/srv.port cargo shards quit
# 3. Cloud: a single-player game (a Steam player): a copy of a save and the free room in the cloud.
cargo run -p sandbox --features shards/steam_sdk -- --headless --steam-app 480 --port 0 --port-file /tmp/one.port &
SHARDS_PORT_FILE=/tmp/one.port cargo shards act save_game what=world slot=cloud1
SHARDS_PORT_FILE=/tmp/one.port cargo shards act steam    # cloud: on · 1 saves copied … · N B free of 4 KB
#    or, if there is no room: … · the last copy failed: saves/world/cloud1.save: Steam Cloud has N B free of 4 KB; this save is M KB
SHARDS_PORT_FILE=/tmp/one.port cargo shards quit
# 4. A lobby that players join by invitation only (a player cannot join their own lobby: joining needs a second account):
cargo run -p sandbox --features shards/steam_sdk -- --headless --steam-host --steam-lobby private --steam-app 480 --port 0 --port-file /tmp/host.port &
SHARDS_PORT_FILE=/tmp/host.port cargo shards act steam   # session: host · lobby <id> (private: invite only), host <name> (<id>) · 1 in it: …
SHARDS_PORT_FILE=/tmp/host.port cargo shards quit
# 5. Lobby search (the second process looks for the first one's lobby, on the same account; it cannot join it, a second account is needed):
cargo run -p sandbox --features shards/steam_sdk -- --headless --steam-host --steam-lobby invisible --steam-app 480 --port 0 --port-file /tmp/host.port &
cargo run -p sandbox --features shards/steam_sdk -- --headless --join steam: --steam-app 480 --port 0 --port-file /tmp/cli.port &
SHARDS_PORT_FILE=/tmp/cli.port cargo shards act steam_lobbies    # 1 lobby of `…` of this build …: lobby <id> · host <name> (<id>) · 1 of 3 · build matches
SHARDS_PORT_FILE=/tmp/host.port cargo shards act steam_lobbies   # the same; if not, see note: in the reply (Steam may not have listed the host's own lobby)
SHARDS_PORT_FILE=/tmp/cli.port cargo shards act steam_join lobby=<id from the reply>   # on the same account: "cannot enter Steam lobby <id>: this Steam account owns the lobby: a second game on the same account cannot join it (join from another account)"
SHARDS_PORT_FILE=/tmp/cli.port cargo shards quit; SHARDS_PORT_FILE=/tmp/host.port cargo shards quit
```

With two accounts on two machines, for a lobby: on the first, `--steam-host --steam-app 480` (or with `--steam-lobby private`), `shards act
steam` gives the lobby id; on the second, `--join steam:<lobby id> --steam-app 480` (or `shards act steam_invite friend=<the second account's id>` on the first and "Accept" in the second game's overlay), then `shards act steam` on both: `lobby …, host …`, `route to …: connected`. Achievements are checked only with an app that has its own achievements in Steamworks: for 480 the names are different (`ACH_WIN_ONE_GAME` and others like it).

**What is missing.** The lobby type is set only at start (`--steam-lobby`); there is no list of game servers (`steam_lobbies` searches for lobbies). There is no voice chat, no statistics (except achievements), no workshop and no rich presence (an invitation through Steam's rich presence does not arrive: `steamworks` 0.13 fails to parse it, so the handler is not installed). A session changes only on restart: a running game does not become a host and does not move to another lobby.

## Builds for players and Windows (E10.6)

`shards package` builds the game for players and checks the build the way another machine will run it. A running game is not needed. Run the command from the game's folder; in a workspace of several packages, use `--package NAME`, and from anywhere, use `--manifest-path`:

```bash
cd my_game && shards package        # in the engine repository: cargo shards package --manifest-path template/Cargo.toml
building my_game my_game (profile dist, feature dist, C runtime linked in, icon assets/icon.png)
    Finished `dist` profile [optimized] target(s) in 5m 59s
built: …\target\x86_64-pc-windows-msvc\dist\my_game.exe
package: …\my_game\dist\my_game-0.1.0-windows-x86_64 (program 123.6 MB, assets/ 52 files, 1.3 MB)
subsystem: windows (no console window)
imports: system DLLs only (kernel32.dll, oleaut32.dll, uiautomationcore.dll, user32.dll, …, dxgi.dll, ws2_32.dll)
check-content: ok: 71 definitions in 29 files
smoke: ok: loaded in 0.05s, 300 ticks in 0.07s
writes: nothing in its own folder
```

- **Profile**: `dist` if the game's workspace sets it, otherwise `release` (`--profile` sets another). In the template, `dist` is `release` with `lto = "thin"`; `panic` stays `unwind`, so the engine can catch a gameplay system that has crashed.
- **The game's `dist` feature**, if it has one, is used instead of the default features (`--no-default-features --features dist`). In the template, `dist = ["render", "audio", "gamepad", "shards/dist"]`: the window, sound and gamepads, without the `shards` server (`remote`) and hot reload. `shards/dist` turns off the developer keys: the console and menu (`` ` ``, F10), overlays (F2, F6, F7). The tools stay, but nobody can call them.
- **Windows (MSVC).** The C runtime is linked in (`-C target-feature=+crt-static`; the build runs with `--target
  x86_64-pc-windows-msvc`, so the flag does not reach the build scripts), so no Visual C++ Redistributable is needed. `assets/icon.png` (a square PNG up to 256×256) becomes the program's icon, which Explorer and shortcuts show: `shards` writes a `.res` resource from it itself and passes it to the linker. The template built with `dist` uses `windows_subsystem = "windows"`, so no console window opens next to the game. If the game was started from a terminal, `shards::run` writes to it (`--check-content`, `--help`, errors).
- **Linux.** The program is linked with `RUNPATH=$ORIGIN` (`-Clink-arg=-Wl,--enable-new-dtags` and `-Clink-arg=-Wl,-rpath,$ORIGIN`): the loader looks for a library that sits next to the program (`libsteam_api.so`) in its own folder, not only in the system folders and in `LD_LIBRARY_PATH`. Without this, a game with `steam_sdk` from the package does not start (`error while loading shared libraries:
  libsteam_api.so: cannot open shared object file`). The arguments go after the `--` of the `cargo rustc` command, so they apply to the program itself, not to its dependencies: `RUSTFLAGS` would replace the game's `rustflags` from `.cargo/config.toml` and rebuild all dependencies, and the `cargo:rustc-link-arg` of the `steamworks-sys` build script does not reach the program (for a dependency it applies only to that dependency itself). The same check as on Windows verifies the result: the package's copy is run in an empty environment (`env_clear`: no `LD_LIBRARY_PATH`, no cargo), and a program with `libsteam_api.so` next to it finds it only through `RUNPATH`. `shards package` does not build or check macOS (`@loader_path`). Checked under WSL1 (Ubuntu 24.04) on a small program with `steamworks` (`libsteam_api.so` in `NEEDED`, nothing from Steam is called): after `shards package`, `readelf -d` shows `RUNPATH
  [$ORIGIN]`, and `ldd` on the package's copy in another folder with `env -i` finds `libsteam_api.so` next to it, while without RUNPATH it says `not found`. Before the RUNPATH change there was none, and `ldd` said `not found`. The game's flags from `[build] rustflags` stay (the marker `-Wl,-rpath,/opt/game-config-marker` in its config gave `RUNPATH [$ORIGIN:/opt/game-config-marker]`), while `--config
  'target.<triple>.rustflags=[…]'` and `RUSTFLAGS` lose the marker. Before this fix, a whole game with `steam_sdk` did not build on Linux.
- **Folder** `dist/<name>-<version>-<OS>-<arch>/` next to the game's `Cargo.toml`: the program and a copy of `assets/`. The Steamworks SDK library of a game with `steam_sdk` (`steam_api64.dll`, `libsteam_api.so`, `libsteam_api.dylib`: the copy that the `steamworks-sys` build script puts in its own folder) goes next to the program; no other libraries that build scripts link are in the package (ALSA and udev, which pkg-config finds on Linux, the player has their own). `steam_appid.txt` is deleted from the folder: the player's game is started by Steam with its app id. It replaces the program and the copy of `assets/`, and leaves the rest of the folder alone. A different place is set with `--out DIR`. The command does not copy a link (symlink, junction) inside `assets/`, or `assets/` itself if it is a link: it refuses, because the package would take in whatever the link points to.
- **Checks** (`--no-check` skips them):
  - on Windows: the program's import table has only system DLLs (or DLLs next to the program, such as `steam_api64.dll`), no `vcruntime*` or `msvcp*`, and no DLL that is in neither `System32` nor next to the program; plus the program's subsystem;
  - the copy of the folder is put in a temporary folder and run with the player's environment: only system variables, `PATH` is only `System32`, there is no cargo and no `BEVY_ASSET_ROOT`. There `--check-content` and `--smoke 300` (`--smoke-ticks N`) must exit with code 0, and the program must not change anything in its own folder (neither write nor delete). A run longer than 5 minutes is stopped, along with everything the program started.

Exit codes: 0 — built, and all checks passed; 1 — a check failed (the `FAILED …` line says which one and what to do); 2 —
it did not build, or there is no such game.

**`--smoke TICKS`** works in every game that runs on `shards::run`. The game loads without a window and runs TICKS ticks as fast as it can.
Then it either prints `ok: loaded in 1.20s, 600 ticks in 0.84s` and exits with code 0, or says what got in the way (it did not load, a gameplay system crashed, an invariant was broken) and exits with code 1. It writes nothing: no settings and no crash replay. This is how you check a build that has no `shards` server for `shards wait` to connect to.

**Window icon.** If the assets folder has an `icon.png`, the engine sets it on the window: in the title bar, and on Windows also on the taskbar. Without the file, the platform icon stays. If the file cannot be read, a `warn` line goes to the journal and the game keeps running.

**How to stop the game on Windows.** Windows has no signals like the ones `kill` uses to stop the game on unix. So there is a `quit` tool (scope Dev) and a `shards quit` command. The game answers, and 0.2 s later it exits through `AppExit`: the exit hooks run (the host ends the session, the settings are written), and the token and `--port-file` are removed. `shards quit` waits until nothing answers on the game's port and no process with its pid is left (`--timeout`, 15 s), and then prints `the game (pid 4242) ended`. An error or a cut-off answer while the game is exiting does not count as proof: `shards` asks again. The process is checked by `kill(pid, 0)` on unix and by `tasklist` on Windows; where that check is not possible, the line says so instead: `the game (pid 4242) answers no more (whether its process is gone
cannot be checked here)`. A process still alive after `--timeout` is an error (exit code 2). No request waits longer than the time left: a game that holds the connection and says nothing gets the same error, `still holds its port, but answers nothing`. On unix this works the same way.
- A stand client in one process (`--stand`) refuses to quit on its own: its process ends together with the host (`shards quit` on the host or `shards stand stop`).
- `shards stand stop` also asks the host to exit through `quit`; a game without that tool gets a signal instead.
- Closing the console window, logging off and shutting down (`CTRL_CLOSE_EVENT` and its neighbours) count as SIGHUP: the game exits through `AppExit` and has time to clean up after itself.
- The `--stand-udp` clients on Windows live in their own process group, and the host stops them with `CTRL_BREAK`. Clients with exit codes 0, 129 and 130 count as having left on their own. `CTRL_BREAK` reaches only a client that shares a console with the host. A host started without a console ends the client's process by force and writes a `warn` about it: the client's port file and token stay (the next stand removes the port file).

Also for Windows: paths in replies and files come without the `\\?\` prefix (`ai_core::paths`), and a `shards` connection to a port where nobody answers is cut off after 0.5 s (Windows repeats the refusal on loopback for about two seconds).

**Limitations.**
- The game writes replays, screenshots, snapshots and saves to the working folder, and when started from Explorer that is the game's folder. Writing to `C:\Program Files` is not allowed, so for now install the game where writing is allowed. There is no player data folder yet; the Steam module makes the copies of saves for Steam Cloud (see "Steam (E10.5)"). A crash report, if the working folder cannot be written, already goes to the player's folder (see "Crash reports (E10.7)").
- The build has not been run on a real, clean Windows (a VM): the check above is only an approximation.
- Determinism between Windows and Linux on x86_64 is verified by a run, but on one machine: `sin`, `exp`, `powf` and the rest go through `libm` (in Bevy, glam, simba and `bevy::math::ops`), not through the system maths. glam keeps its SIMD. `clippy.toml` stops direct calls from being put back (see "Time and determinism"). Nine reference replays, recorded on Windows 11 and on Ubuntu 24.04 (glibc 2.39) under WSL1, matched byte for byte, and each OS passed `--verify` on the other's recordings. The processor was the same one, so other x86_64 processors are not checked; macOS, ARM and Steam Deck hardware are not checked either. Between architectures (x86_64 ↔ ARM, Apple Silicon) determinism is not promised: `enhanced-determinism` in parry3d would give it through glam without SIMD (`scalar-math`), but with it Bevy 0.20 stops rendering (`MeshUniform` 168 bytes against 176 in the shader), and `shards` does not build that way.

## Crash reports (E10.7)

When the game crashes, everything needed for a bug report goes into one file: what happened, the game and the machine, the session and its replay.
There is no replay in a report about a panic that took the game down, or for a game that does not record its session (`no_replay` says why).
The player sends the file. The developer plays the report with its replay as a replay (for a report without one, `--replay` answers `… is a
crash report without a replay (<why>)`):

```bash
cargo sandbox --replay crashes/crash-20261016-123456-tick50.json --headless --paused &   # the same session from tick 0
cargo sandbox --replay crashes/crash-20261016-123456-tick50.json --verify                # exit code 1 while the system is still crashing
```

The file is JSON with `format: "shards-crash/1"` (`ai_core::crash`):

| field | what it holds |
|---|---|
| `what` | the headline (`system game::combat::apply_hits panicked at tick 50`, `thread 'worker' panicked`, `bug report: …`), the panic message, the place (`src/combat.rs:120:9`), the thread, the time and the call stack |
| `game` | the game's `title` and `version` (`EngineConfig::title`, `EngineConfig::version`; the template and the sandbox set the version of their own crate), the program name, the engine version, the `debug` or `release` build |
| `system` | the OS with its version (`Windows 11 (10.0.22631)`, `Ubuntu 24.04.1 LTS (Linux 6.8.0-45-generic)`), architecture, processor, number of threads, memory (total and free, MiB), graphics card and driver (`none (no renderer: headless)` without rendering) |
| `session` | tick, seed, ticks per second |
| `status` | `ai/status` at the time of the report: engine modules and their versions, tick time, network, sound, UI |
| `journal`, `log` | the last 200 journal entries and 200 log lines (info and above; the log takes the same layer that carries `warn!` into the journal) |
| `panics` | other panics from the last eight that the handler saw, if they are not in any saved report (a report that could not be saved leaves them for the next one): those caught by code (a tool's reply) and those nobody caught; a panic that the report itself is about does not appear here a second time |
| `replay`, `replay_file` | the session's replay from its start, and the path of the crash replay saved with the report, relative to the working folder (`replays/crash-tick50.json`; absent if it could not be saved); `no_replay` — why there is no replay |

When a report is written:
- **A gameplay system crashed** (a system, an observer, a command). The report is written together with the crash replay `replays/crash-tick<N>.json`, also one per crashed system, and the journal writes `[sim] crash report saved:
  crashes/crash-…-tick50.json (send it with a bug report; --replay plays it)`. If the working folder is closed for writing, there is no crash replay, and the report goes to the player's folder (see "Where"); the session's replay is still inside the report. If the report was not saved anywhere (`crash report not saved: …` in the journal), the next failure of the same system tries again.
- **A panic in another thread that nobody caught** (the render thread, a task). The report is written in the next frame, while the game keeps running; the journal gets `[error] thread 'worker' panicked: <msg>`, and stderr gets the path to the report. One place writes one report; if it could not be saved, the next panic there tries again. A panic in Bevy's render thread ends the game with code 1, but the frame with the report gets through before that.
- **A panic that takes the game down**: outside the main world's systems, for example in rendering without a separate thread or in window code. `shards::run` catches it, writes the report and exits with code 101, as after any Rust panic; stderr gets `error: the game
  crashed: <msg>; crash report: crashes/…`. The world went down with the panic, so the report has no replay, `status` or journal: the panic handler remembered the game, the tick and the seed from the last frame. Exit hooks do not run in this case (the host does not end the session, the settings are not written), but the BRP token and `--port-file` are removed.
- **On request**: the `crash_report` tool (scope Dev; in the developer menu, "Save a bug report…") writes the same report without stopping the game. Its headline is `note`. A report on request is always written, whatever `crash_replays` says: it is requested, not written by the game itself. With `path` (a file inside `crashes/`, as with `snapshot_save` and `replay_save`) it is written exactly there and replaces a file with that name; without `path`, it is a new `crash-<time>-tick<N>.json`:

```bash
cargo shards act crash_report note="the boss fell through the floor"
saved crashes/crash-20261016-123456-tick30.json: tick 30, 120 log lines, 48 journal entries, a replay of 30 ticks and 4 events
cargo shards act crash_report note="the boss fell through the floor" path=crashes/boss.json
saved crashes/boss.json: tick 30, 120 log lines, 48 journal entries, a replay of 30 ticks and 4 events
cargo shards act crash_report path=../x.json                  # exit code 1
error: path must be inside crashes/, e.g. crashes/bug.json (`../x.json` goes up with `..`)
```

**Where.** In the `crashes/` folder of the game's working folder (next to `replays/`). If it cannot be written there, for example when the game is in `C:\Program Files`, the report goes to the player's folder: `%LOCALAPPDATA%\<program>\crashes` on Windows, `~/Library/Logs/<program>` on macOS, `$XDG_STATE_HOME/<program>/crashes` (or `~/.local/state/<program>/crashes`) on Linux. A report from the same second is not overwritten: the next one gets the name with `-2`, `-3`. A report with `path` goes only into the `crashes/` of the working folder (an absolute path, `..`, a path outside `crashes/`, and a symlink pointing outside are refused, and nothing is written), and it never goes to the player's folder: the file with that name is replaced; if the write fails, the old file stays (the report is written beside it and then renamed).

**Who does what.** The panic handler (`ai_core::crash::install_panic_hook`) is installed by `shards::run`. It remembers the last eight panics, with their stack and the session they happened in, and then prints the panic as before; it writes no files. Without it (your own `main`, tests) the report exists, but without the place and stack of the panic. The files the game writes on its own are written when `crash_replays` is on (the default); the tests, `--verify`, `--smoke`, `--check-content`, `--worldgen-report` and `--bake-nav` turn it off. The switch does not apply to a report that an agent asks for (`crash_report`). Code that catches a panic itself and carries on (a tool's reply, a broken rule, a generator seed, a peer's character) calls `ai_core::crash::handled(&message)`: such a panic gets no separate report; the next report shows it in `panics`. If two threads or two games in one process panic with the same text, `handled` takes the panic of its own thread first, then of its own game, and only then the newest one.

**Limitations.**
- A crash that is not a panic leaves no report: an access violation, a driver error, `abort`, a stack overflow or running out of memory cuts the process short before the handler learns anything.
- Function names in the stack need symbols. The debug build has them. The build for players (`dist`) needs them from the `.pdb` file (Windows), and `shards package` does not put it into the package (`target/…/dist/<game>.pdb`, about 90 MB for the template), and the report does not store the addresses that would let the stack be rebuilt later.
- There is no window with a message for the player: the path to the report is shown only in the terminal and the journal.
- The report contains the player's machine paths (program, stack) and everything the game wrote to the log.

## The sandbox

The sandbox (`crates/sandbox`) is the engine's test arena, where every module is checked. It is built only on `shards`, like a game. It has little code of its own: the arena world built from content, the boss phases, the remains, the conditions for `step`, and the window's visuals. The monsters are the engine's NPCs (`shards_npc`): their behaviour, perception, the token director and attack plans are content too. Four areas:

- **Arena** (from point (0, 0)): the player, three skeletons and the Frost Warden guard — the combat prototype;
- **Obstacle course** from x = 40 (E2);
- **Wilds** (`wilds`): a generated region of 8 × 8 tiles of 16 m, south of the arena, with its north-west corner at (−64, 48), that is x −64..64, z 48..176 (the arena's south wall is at z 21.5). The player gets there with `regen region=wilds seed=42` (`shards act`), at the gate in cell (1, 1). Nine tiles: grass, four roads, columns, a ruin from a Blender file (`assets/tiles/ruin.gltf`), the gate and a hall with a colonnade and a lair (the gate and the hall are placed by the region, not by chance). Points of interest: the gate, the lair with its guard, a shrine, two skeleton camps. The tiles are baked into `assets/nav/` (they are committed), and `ruin.level.ron` makes `content check` read the ruin's file node by node;
- **Expanse** (`expanse`): a region at full size from the spec, 8 × 8 tiles of 96 m (768 m on a side), with its north-west corner at (−384, 200), that is x −384..384, z 200..968 (`regen region=expanse seed=42`, the player at the gate in the middle, in cell (4, 4), at point (48, 632)). Four tiles of its own with `tile_size: Some(96.0)`: the gate, a meadow with thirty boulders, a grove of 36 columns, a courtyard inside walls; about 1900 colliders, three camps, and its own sparse foliage. The performance test `reference96` runs on it (section "Frame time and the performance test (E10)").

Everything that is numbers and definitions is content in `crates/sandbox/assets/content/`, with hot reload:

| file | what it holds |
|---|---|
| `world/arena.layout.ron` | the 40×40 m arena: floor, walls, columns, a gate to the east; who to spawn and where |
| `world/course.layout.ron` | the obstacle course from x = 40: steps, a 1.3 m ledge, a 10 m pool, a ramp to a cliff 18 m above the canyon, a goal platform across a 32 m chasm, an updraft in the middle |
| `characters/characters.prefab.ron` | prefabs `player`, `skeleton`, `skeleton_captain`, `frost_warden`, `pillar`; the bodies and the column have a `Look`, which the window uses to draw them |
| `look/sandbox.look.ron` | looks (E10.1): the presets `low_poly` (flat faces, outline) and `cel` (two bands); `player`, `skeleton`, `warden` (`extends: "low_poly"`), `stone`, `wall` (outline), `floor`, `sand` (two bands), `dark` (`Matte`), `marker` (`Unlit`); the arena box's look is the `look` field in `*.layout.ron` (`Stone`, `Floor`, `Wall`, `Sand`), and `visuals.rs` sets the characters' noses (`dark`) and the landmark posts (`marker`) |
| `fx/sandbox.effect.ron`, `fx/sandbox.fx.ron` | effects (E10.3): `hit_spark` (hit sparks, glowing, flying back toward the attacker), `block_spark` (block shards), `parry_flash` (parry flash), `death_dust` (death dust: cubes, which keep following the body for another 0.8 s), `heal_glow` (healing glow); the table covers `damaged`, `blocked`, `parried`, `died`, `healed` |
| `characters/bodies.locomotion.ron` | how the player, the skeleton and the guard walk |
| `combat/player.action.ron` | the combo `light_1`–`light_3`, `heavy`, the roll (i-frames 0.18 s), the block with a perfect block, the flask |
| `combat/warden.action.ron` | the guard's four attacks (`slash`, `slam`, `sweep`, `shockwave`), calm and enraged |
| `combat/sparring.action.ron`, `combat/sparring.moveset.ron` | the hit-outcome mechanics (B1) on neutral numbers: the `spar_*` actions (a jab chain asking a light reaction, a slam asking a heavy one with a push and a wall check, a roll with a perfect-evade window that starts on any stamina, a guard that prices the hit and costs half when perfect) and the movesets `sparring` (reactions, immunity, protection, a guard-broken action) and `duelist`, which `reactions.scenario` lends to two arena fighters; for the action timeline (B2a), `spar_heavy` (charges into `spar_heavy_full` when held 0.25 s; a light or a dodge pressed in its windup waits for its recovery), `spar_lunge` and `spar_lunge_far` (one `lunge` cooldown slot; the lunge turns at 360°/s within 45°, at most 0.4 m closer for the turning and not toward a target behind a wall, recovers 0.6 s longer on a whiff and keeps 40% of its cooldown when cut in its windup), `spar_channel` (`while_held`), `spar_roll_keep` (`face: Keep`), and the moveset `timeline` (a single slot by priority), which `timeline.scenario` lends to the player; for body state (B2b), `spar_ward` (a barrier of half the caster's health for 8 s), `spar_frost` and `spar_zap` (jabs that carry a full bar of chill or jolt), the moveset `bodystate`, and in `combat/sparring.status.ron` the statuses `spar_chill` (a 2 s proc gate, no cooling and a restart while active, a refused proc empties the bar) and `spar_jolt` (an instant status outside the cap), which `body_state.scenario` lends to the player |
| `combat/skeleton.action.ron`, `combat/fighters.moveset.ron`, `combat/tuning.combat_tuning.ron` | the skeleton's claw, the movesets, the input buffer |
| `npc/monsters.behavior.ron`, `monsters.perception.ron`, `monsters.director.ron`, `monsters.attack_plan.ron` | how the monsters think: the `skeleton` and `warden` state machines (with a `calm` state that the scenarios do not let the monster leave), sight, the `default` director (two tokens for one player) and the `boss` director (the guard strikes on its own), the guard's attack plans |
| `region/wilds.*.ron` | the wilds: tiles, socket rules, points of interest, the region, a fallback map, navmesh settings |
| `region/wilds.scatter.ron` | the wilds' foliage (E10.2): tufts of grass in two kinds on every tile, bushes and stones; the looks `grass`, `grass_dry`, `bush`, `rock` are in `look/sandbox.look.ron` |
| `region/expanse.*.ron` | the expanse: a region of 8 × 8 tiles of 96 m, its four tiles and the foliage (`tufts`, `shrubs`, `stones`: twenty times sparser than in the wilds) |
| `perf/reference.perf.ron`, `perf/reference96.perf.ron` | the performance tests: the wilds and the expanse, 15 enemies each, 1080p |
| `region/camps.prefab.ron`, `region/ruin.level.ron` | the camp skeletons (`Home` 12 m) and the lair's guard (10 m); the ruin file and the markers it must have |
| `world/watch.layout.ron` | the arena's third skeleton (`e14`) |
| `world/world.bounds.ron` | the world bounds for the `bodies_in_world` rule: a margin around the built layouts and the region, and the height below which a body counts as having fallen |
| `ui/menus.screen.ron` | the menus, built from the engine's widgets: `pause`, `settings` (all the settings groups and the language, on tabs), the `confirm_quit` prompt; the sandbox has its own HUD, in the window code (`visuals.rs`) |
| `ui/languages.locale.ron` | the menu languages: English and Russian, with English as the fallback; the texts are in `assets/locale/<language>/menus.ftl`. The sandbox has no theme of its own: the menus are drawn with the engine's theme and its Fira Sans font, which has Cyrillic |
| `dev/sandbox.devmenu.ron` | the `sandbox` section of the developer menu: the player to the start of the arena or the course, the guard's second phase from a snapshot, the wilds with a seed of your choice |
| `audio/arena.sound.ron`, `audio/arena.music.ron`, `audio/mixer.audio.ron` | sounds (the guard's attacks, hits, block, parry, death, healing, menus), the arena music in three layers, and the mixer: 24 voices, the engine's event sounds; the files are in `assets/sounds/` and `assets/music/` (synthesised by `cargo run -p sandbox --example make_sounds`) |

`layout` is a content kind of the sandbox itself: boxes (`solids`), ramps, volumes (water, a current, a trigger), landmarks for the AI and prefabs to spawn. Layers spawn in id order, and within a layer in the order of their lists, so `AiId` and `ColliderId` are the same in every run: e1–e4 are columns, e5 the player, e6 the guard, e7–e8 the skeletons, e9–e13 the course's landmarks `Stairs`, `Ledge`, `Pool`, `Cliff`, `Goal`, and e14 the third skeleton; the region (`regen`) adds its own entities after them. Editing a layout on the fly rebuilds the geometry on the next tick; the landmarks keep their ids, and the characters remain. A new `*.layout.ron` file is built on the fly the same way as at start-up (with its prefabs), and a deleted one removes its geometry and landmarks.

**Combat is the prototype from GDD 39.** The player has a combo of three light hits through cancel windows, a heavy hit with hyper-armour during the windup, a roll, a block (hold it; the first 0.15 s is a perfect block, and a perfect block is the parry: no damage, and the attacker staggers), and a flask (45 HP, 3 charges). Stamina is spent on hits, rolls, blocks and sprinting; everyone has poise. The guard is an NPC: it guards its post and fights players within a leash of 26 m (the arena's skeletons, 30 m), and it does not heal when it returns. The `warden` attack plan picks, from the moveset's attacks, the ones that reach the target (`SimRng`), and never makes two "zone" attacks in a row (the `zone` tag in the content); the pause after each one is 1.2–1.8 s, or 1.0–1.4 s when enraged, and it counts down only while the guard does nothing (being staggered after a parry does not count as rest). The warning (`e6 winds up slam (0.50s) ⚠ circle 4m`) arrives exactly 0.50 s before the hit. From half health, phase 2 begins: a second moveset, shorter windups and shorter rest. Skeletons: one claw each; they flinch when hit, and their bodies disappear after 1.5 s. The guard's body stays (the dev `heal` brings it back).

**Rules (invariants).** After every tick the sandbox checks `health_in_range` (health in `0..=max`) and `bodies_in_world` (every character is above the arena's geometry, the corridor and the course, or above the loaded region, see "Invariants (E11)"). No scenario, test or reference replay breaks them.

**Obstacle course (E2).** A jump (1.2 m) does not clear any obstacle: the 1.3 m ledge is taken by climbing, the 10 m pool is swum and left with `jump`, and only a glider reaches the goal platform, and only by climbing in the updraft. The platform is a trigger: `e5 reached goal`.

**Reference scenarios** (`crates/sandbox/scenarios/`), the agent with scope `player`; the scenario's agent advances time:

- `obstacle_course.scenario` — `path_to` and `clearance`, the steps, the ledge, the pool, the ramp, the glider through the updraft, the goal;
  invariants: alive, never below the start;
- `combat_prototype.scenario` — the guard is beaten in 11 rounds. Each round starts from its warning (`step --until
  "journal:e6 winds up"`), then a roll out of the zone, a block, a perfect block with a heavy hit, an exchange of blows under hyper-armour, the flask, the combo. The test runs it after six different pauses (other attacks in another order): a win every time;
- `timeline.scenario` (B2a) — the player borrows the `timeline` moveset (`combat/sparring.*.ron`) against a still skeleton: a heavy held from the press shows `charging, turns in 0.23s` (`step --until named:charging`) and turns into `spar_heavy_full` (-25), a tapped one stays `spar_heavy` (-12); a light pressed while the heavy winds up answers `ok: queued, light starts in 0.5…` and the jab lands with no `light rejected`; a lunge into nothing writes `e5 spar_lunge missed: +0.60s recovery` and shows `missed +0.60s`; a lunge staggered in its windup keeps 40% of its cooldown on the `lunge` slot, so its variant on `technique.2` answers `lunge is on cooldown (ready in 0.5…)`; a channel started with `hold=true` lasts while held and ends on `hold=false`; locked on the skeleton to the north, a roll east (`spar_roll_keep`, `face: Keep`) keeps the player `facing N`. At the end the player has its own moveset, no `RunExtras` and nothing queued;
- `body_state.scenario` (B2b) — the player borrows the `bodystate` moveset and the skeleton the `sparring` one: a ward gives the player `barrier 50 (7.…)` and journals `e5 barrier 50 for 8.0s`; three slams of 15 come out of it (`e7 -> e5 spar_slam -15 (hp 100) [barrier -15]`, health whole, `barrier 35`, `20`, `5`) and the fourth hurts for 10 (`(hp 90) [barrier -5]`, no `Barrier` left); `damage_taken id=e7 key=core mult=2 seconds=1` doubles a jab (`-12`) and the next one a second later is `-6` again with no `DamageTaken`; a jab that chills procs `spar_chill`, shows `proc gate spar_chill`, a second inside 2 s is swallowed and empties the bar (`Statuses.0[0].buildup == 0`, the chill not restarted), one past the gate restarts it, and a jolt procs and expires in a tick with no slot. At the end both have their own movesets and no barrier, multiplier, proc gate or chill;
- `boss_fight`, `dodge`, `errors` — the switch to phase 2, a roll to dodge a hit, errors that teach;
- `npc_director.scenario` (E6) — three skeletons against one player in god mode, 30 s of combat: the sandbox condition `three_attacking`
  (three hit at once) is never met on any tick, but each skeleton gets its own turn, and the tokens rotate;
- `npc_navmesh.scenario` (E6) — in the wilds the lair's guard sees the player through the colonnade, walks round the columns on the navmesh
  (`npc_detour`; the player's `nav_path` shows the same 16.8 m instead of the straight 10, the player's `path_to` to the lair also goes by the navmesh, and `npc_overlay` shows the guard's line: fight, leash, detour), strikes, and never goes past its leash
  (`npc_beyond_leash`); when the player goes north, it reaches the edge of the zone and returns to the lair;
- `npc_avoidance.scenario` (E6) — three arena skeletons run single file toward the player: the rear ones go round the front ones (`npc_avoiding`,
  `npc_overlay` writes `avoiding e7`), five seconds of combat, and the journal stays silent about the avoidance;
- `npc_bodies.scenario` (E6) — the wilds with seed 42 are baked for both the skeletons (`agent`) and the guard (class `large`); the arena's guard in the pylon hall: `npc_state` names its class (`navmesh, body large`), and `nav_path body=large` and `nav_path` without it each follow the navmesh of their own class (a typo in the name gets a hint), the guard walks round the colonnade to the player along its own navmesh (`npc_overlay`
  writes `body large`), does not go past its leash, and at the end returns to the arena;
- `region.scenario` (E5) — `regen region=wilds seed=42` and the manifest hash `7bbfdc3b62623fbc`, the player at the gate, a path two tiles forward and back along the navmesh (`move_to path=true`, stopping at `near_poi`), a different seed gives a different manifest, and the same seed gives the same one, down to the byte;
- `snapshot.scenario` (E11.4) — the fight with the guard is taken to phase 2, a snapshot is taken, the game moves on, the developer menu item `boss_phase_2` (`snapshot_load` of this file) brings the fight back, and it can be continued;
- `dev_menu.scenario` (E11) — the developer menu and console: every item in the GDD list (32.8) has an item with a live tool, a toggle shows the line for the other state, the sandbox items move the player, an item with blanks to fill asks for a value, the console answers a line and teaches from errors, and in the region the "Check this seed" item runs the generator on its seed;
- `settings.scenario` (E8.6) — `settings` and `settings_set`: values by type, refusals that name what is allowed, reset, `rebind` as the `controls` setting; at the start and at the end all groups go back to their defaults (it runs both with and without a settings file);
- `ui.scenario` (E9) — the menus walk the UI tree: pause, the settings tabs, a slider and a choice change the player's settings, a question over the menu, errors that teach; at the start and at the end the menus are closed, and the settings are at their defaults;
- `remap.scenario` (E9) — the key rows on the controls page: waiting for an input and cancelling without closing the screen, a new key as the `controls` setting, a taken input is refused, naming the action (in English and in Russian), `default` restores the defaults;
- `locale.scenario` (E9.4) — `locale` and `text`: plural forms by language, a phrase inside a phrase, a hint for a typo;
  the menu's "Language" tab switches it to Russian while the menu is open, what is missing in Russian is taken from English, and a language that does not exist is refused; at the start and at the end the game language is the starting one and the menus are closed;
- `audio.scenario` (E9.5) — the arena music follows the game: in the canyon, far from the monsters, it is `explore`; near the guard, `tension`; when the guard fights, `combat` at once; when it calms down, `tension` again after five seconds; `music_set` holds a layer and teaches from errors; 25 sounds on 24 voices: a loud sound takes the voice of a quiet one, and the voices are freed; a sound beyond `far` does not start; the player's volume applies to what is playing; healing and the menus play through the mixer's sounds;
- `looks.scenario` (E10.1) — without a picture, `looks` says there is nothing to draw and lists the looks with their presets (`extends`);
  `content_get` of a look, the bodies' `Look` in `inspect`, `look_outlines` and the developer menu's toggle (at the end the outlines are on), and a look with an error is refused by `content_apply`;
- `scatter.scenario` (E10.2) — without a picture, `scatter` says that nothing is scattered and lists the wilds' definitions;
  `content_get` of a definition and of a look, `graphics.foliage` in the journal and a refusal outside 0…1, `scatter_set` and the developer menu's toggle (at the end the foliage is on and full), and a definition with errors is refused by `content_apply`;
- `effects.scenario` (E10.3) — without a picture, `effects` says there is nothing to draw and lists the effects with their pools and the event table;
  `content_get` of an effect and of the table, `effect_play` without a picture and with a typo, `telegraph_decals` and the developer menu's toggle (at the end the decals are on), and an effect and a second table with errors are refused by `content_apply`;
- `budgets.scenario` (E10.4) — the performance test's budgets, which are content of the `perf` kind: `content_list` names the sandbox's two definitions, `content_get` shows the scene (the region and seed, the setting lines) and the named budgets; a definition that cannot be run is refused by `content_apply`, with a reason for each error (there is no such region, a setting line is not a call, zero ticks, a budget not above 0, a share outside 0…1, a load budget without a region, a typo in a budget's name), and nothing is applied. The scenario does not read the time: the `ticks: avg … max … budget …` line from `step` is printed by a live game, while the test game in the process (`report_perf: false`) does not print it, so there is no expectation common to both runs (`crates/sandbox/tests/perf.rs` checks them);
- `display.scenario` (E10.8, E10.9) — without a window and `--offscreen` there is no picture, and `graphics.render_scale` lives as a setting: it is read, changed and written to the journal; a value outside 0.5…2, or one that is not a number, is refused with the range and changes nothing; the settings screen's slider (`ui_read`) is bound to it; `shot` answers `nothing to capture: this game runs headless` and names `--offscreen`; at the end the scale is 1 and the settings screen is closed. The 3D image and `display.scene` are checked by a test marked `#[ignore]` (it needs a GPU);
- `crash_report.scenario` (E10.7) — `crash_report note=… path=crashes/scenario.json` writes the report to that file and answers with the path, the tick, the number of log and journal lines and the session's replay; a second call with the same `path` replaces the file, so a run leaves exactly one file (`crashes/` is in `.gitignore`: in the tests it is `crates/sandbox/crashes/`, in a live game it is the `crashes/` of the working folder); a path outside `crashes/`, a path with `..` and an absolute path are refused, with an example. A report without `path` is new each time, and the scenario cannot remove it: the Rust tests `system_failures.rs` check it;
- not covered by a scenario: `shards package` (the build is a CLI command, not a game tool) and `quit` (it ends the game the scenario drives). Steam: `crates/shards_steam/scenarios/lobby_search.scenario` (a stand running on a fake Steam, see "Finding a lobby");
- `world_save.scenario` (E8) — the world is saved in the wilds with seed 42, another seed replaces the region (`save_check`: `differs in
  region`), loading builds the saved region from the save's manifest (`built from the save's manifest`, not a single `region
  generated`), the hash is `7bbfdc3b62623fbc` again, and `save_check` agrees.

The scenarios start with `heal`/`teleport`/`set`/`npc_set_state … calm`, so they run on a fresh game and after other scenarios, in any order:
CI runs them forward and then backward in one game. Sessions of nine of them are recorded as reference replays in `tests/replays/`
(`obstacle_course`, `combat_prototype`, `npc_director`, `npc_navmesh`, `region`, `snapshot`, `saves`, `reactions`, `timeline`; see "Replays").

## Tests and CI

```bash
cargo test --workspace          # unit tests, scenarios and reference replays; the first Bevy build is slow
cargo nextest run --workspace --profile ci -E 'not binary_id(sandbox::perf)'   # the same tests, as CI runs them: a process per test, retries that fail the run
cargo nextest run --workspace --profile ci-perf -E 'binary_id(sandbox::perf)' -j 1   # the tick-time tests alone
cargo test --workspace --doc    # doctests: nextest does not run them
cargo test -p sandbox --test sandbox          # the sandbox: scenarios, replays, content, the obstacle course, combat
cargo test --manifest-path template/Cargo.toml # the template on its own: its own workspace and its own Cargo.lock
cargo nextest run --manifest-path template/Cargo.toml --profile ci   # the same with nextest (reads template/.config/nextest.toml)
```

The test count for each crate is kept in `docs/engine-progress.md`, "Tests"; there is no overall total here: it drifted from the table
after every edit, and it can only be checked with a full run. Some tests are marked `#[ignore]`: recording the reference
replays, checking the window under Xvfb, the screenshot without a window (needs a GPU), network measurements and the examples in the documentation. The template has its own set (`template/tests/game`,
a separate workspace; one `#[ignore]`: recording its references).

| crate | what it checks |
|---|---|
| `ai_core` | snapshots (saving and loading, refusal of a snapshot from another game, references to entities, a replay from a snapshot without loading), ids (renumbering, a taken id), the journal, RNG, the compass and line of sight (LOS), schemas, simulation steps, the paths for screenshots, replays and snapshots (only `shots/`, `replays/`, `snapshots/`; absolute paths, `..`, symlinks), `shot` labels in image pixels at any `render_scale`, the replay format and report, the ban on editing `AiId` through BRP, a busy port, `instance` in `ai/status`, the port file, the token on every BRP method, running scenarios in-process, invariants (game and session rules, firing once, the route to the host), tick time, the parts of the replay checks and the divergence report, bit-for-bit results of `ops` and of glam on `libm` (`determinism`) |
| `shards_script` | without Bevy: conditions and arguments, words as in a shell, parsing of each scenario command and the text of each error, runs against a fake game (expectations, `mark`, invariants, `session`, `seed`, a lost game, a step, a run cut short by a violated invariant), the `warning` and `error` conditions; the inputs of `fuzz` and `soak` from a seed, the argument generator (each schema construct, ids, invalid arguments, a seed gives the same arguments), `fuzz` on a fake game (the alphabet and weights, `classic` draws as the first fuzzer, chaos, a halt), `soak` on a fake stand (region changes and saves on a schedule, a dead body healed by the host, a stop on a desync, a summary per participant) |
| `shards_cli` | the port from `--port-file`, the `ready` line, the token by port number; `shards scenario` against a fake game on localhost (exit codes 0/1/2); the exit code from the outcome of `step` (3 — the game stopped) |
| `shards_core` | loading does not shift ticks, pause, `after_loading`, presses last one tick, `move_to`, `ai/status` names the modules |
| `shards_content` | definitions and their files, duplicates and references name the file and the field, `check_dir`, prefabs (`extends`, overrides), a reload is written to a replay and played back, a broken edit is rejected (and so is an edit to a file where even one definition is broken, as a whole), a broken definition gets its own error, the other definitions of the file are loaded, an edit to a file in a running game |
| `shards_motion` | physics (queries, layers, order by `ColliderId`, the broad phase: the same answers as a scan over all colliders, on random worlds; the grid after moves, removals and new ids; voxel fields: the same answers as a cuboid collider per cell on random fields, the seam differential test `seams.rs`, the hooks in a tunnel under a plateau, the reader guard `tests/readers.rs`), the controller (steps, climbing over, swimming, glider, jump, walls), numbers from content, input (modes, contexts, rebinding, saving), the camera, `clearance` and `path_to`, tools, replays and independence from `Entity` indices |
| `shards_action` | actions, the buffer, cancels and combos, `can_start`, hits and the damage pipeline (dodge, block, parry, poise, hyper armour), statuses, zones and the "get out" hint, animation (`select_clip`, the files of the set: clip, source, root bone), baking of the offset (clip timing, thinning, an in-place file edit, a stale curve), reading glTF clips, telegraph geometry, tools and their errors, the combat replay |
| `shards_level` | glTF and nodes, properties (`collider`, `volume`, `prefab`, components by name, text JSON), markers and their errors, colliders of the right sizes and layers, the `level` kind and its check in `content check`, spawning (world positions, order, ids), hot reload, the replay and the `level` hash, the level image; a real export from Blender 5.2.2 (`from_blender.rs`: markers, collider shapes, prefabs, components, the artist's errors, lamps, Draco and both meshopt, the default scene) |
| `shards_world` | the format of tiles, regions and rooms (errors name the file and the nearest id; a tile for cells of a different size, `tile_size`, is not part of "all tiles", and the error is reported where it was named); the solver, checks (unreachable areas, slope, distances, a road to nowhere), hundreds and a thousand seeds, `worldgen_report` with a single seed number, the manifest byte for byte, the fallback map, a panic in a game step with the seed number; dungeons; a region change through `RegionLoading`, the start region, `goto_tile`, a replay with region changes, a snapshot in a region; tiles from level files in all four quarter turns; the socket markers of a level file against the sockets of a tile; a tile file that cannot be read, or that has an error in one of its nodes, is a content error that names the tile, the file and the node, and it is exactly one error if the file is also named by a `level` definition; active zones; a wall at the edge of a region (it holds a body; an open edge drops it; the collider numbers of tiles stay as before, and it leaves with the region) |
| `shards_npc` | the behaviour state machine and its errors, sight/hearing/memory, director tokens (including a change in the number of players and the agent's orders), the scheduler (bans, budget, rest), baking and stitching (including steps at the edge of a tile), carving in any order, routes and going around columns, `path_to` and `clearance` on the navmesh (`provider`, `closed`), the leash, `npc_state`, `npc_overlay`, a replay and a snapshot over ten combat ticks and in a generated region |
| `shards_save` | sections and the registry, the `shards-save/1` envelope and checksum, atomic writes, three copies and recovery, the corruption matrix, migrations, saves from a newer version, slots (including a client's slot by name), autosave, the `save_*` and `load_game` tools, `SaveRouting` |
| `shards_dev` | every item in the GDD 32.8 list has a menu entry with a live tool, and the order of the sections; entries and lines go through the same tools (toggles, entries with a skip, entries that do nothing, game entries from content); the console answers and remembers lines; `open` shows panels; overlay keys wait until a panel is open; Tab completes tool and argument names; a broken entry is a content error |
| `shards_look` | the look of a mesh: its own `Look`, its ancestor's, the glTF material name; the material from the source (colour and transparency, glow, `unlit`), `extends` presets and their errors, the flat copy and the outline (layers, transparent materials and morph targets without an outline), hot reload and assets changed in place; without an image nothing is styled; the image on the GPU (`#[ignore]`): the bands and the outline in pixels |
| `shards_scatter` | the layout: one seed, the same points, `spacing`, `graphics.foliage` keeps the start of the same list, ground, slope, the band of heights and `clearance`; the content check names every error; in a game without an image nothing is scattered; on every tile of the test region, not on a column, not in a wall, the same in two runs; batches (two meshes for thousands of entities), the `VisibilityRange` of levels, shadows; `graphics.foliage`, `scatter_set` and the developer menu, a content edit and a region change (without warning B0004); the image on the GPU (`#[ignore]`): the grass is green in the pixels; foliage that is switched off, or beyond the last level, is not drawn |
| `shards_fx` | effects and the table in content and their errors (a pool smaller than one run, an event with a typo, a second table), tools, the menu and the status without an image; mesh pools from content (one mesh and material per effect), starting, pool overflow (the oldest ones give way), simulation clocks (pause freezes them, `step` advances them), `rate` and `duration`, effects of events (on the side of the hit), clip labels and `PlayEffect`; a decal on a zone (circle, sector), the fill grows towards the hit, the colour of an enemy and of an ally, gizmos give way to the fill, `DepthPrepass` on the camera and back; the image on the GPU (`#[ignore]`): the decal paints the floor only inside the zone, the particles glow where they play |
| `shards_voxel` | the volume and its limits (chunk size a parameter, shape checked without allocating, edits clipped), the hash, the palette and each error with the file named, noise golden values, the step registry, retries and the golden world, the merged boxes against a brute-force reference; the content kind alone changes no content hash |
| `shards_steam` | on a fake Steam: lobbies and invitations, relay sessions (held datagrams, refusal, hub delay), the game server, achievements and the fake's cloud, `steam:` and `+connect_lobby` addresses, the cloud check and its file names; `tests/steam/`: a lobby host and two friends over the stand's relay, the game server and a client by id, an invitation, refusal of a stranger, a failed join and `--exit-with-host`, a check of the achievement content, a write to Steam after a second and a retry of a refused write, the previous run, a replay, co-op (the host decides, each one opens its own), a copy of each record, cloud turned off, the check at start-up before a slot is loaded, the answers of `steam` and `ai/status`. The real backend is only built (`--features sdk`) |
| `shards_net` | `src/`, without Bevy: transports (an in-memory hub, UDP, a reply to a stranger that binds no one), link simulation, the channel (reliable and latest-only messages, blobs, acknowledgements, RTT, loss, timeouts), the datagram format (knock, challenge, cookie in the acknowledgement fields, protocol 1 is recognised), the cookie (SipHash-2-4 on reference vectors, address, session, expiry, forgery); the session (a knock leaves no trace on the host, a peer only with the cookie of its own address and session, an expired cookie, the table of strangers as before, refusal of an older build on its protocol and no more than 8 times a second, the character in parts: the place is held, order, size, `Hello` bytes, JSON; the limits on input frames and on the largest honest packet, what is held after the world is reloaded, replication rules, waiting for room in the channel by count and by bytes, `NetDef`, departed tokens). The whole session is covered in the sandbox tests `net*.rs` |
| `shards` | how networking and saves fit together (`coop`: what the host knows about the player's character, how it travels as text), engine sections (a number that a body never has), the stand's slot, a replay from a snapshot (`--verify`: the same result, code 2 for a foreign snapshot), the mode without a window: the same plugins, but no image (`display` and the `shot` hint), `--offscreen`: the window's plugins without a window (an image for `shot`, into which the cameras aimed at the window draw, also after a change of target), the internal 3D resolution (`render_scale`: an image of the right size, the cameras into it and back, shown on the whole window, the window changes size, the system scale, the game camera with its own image), the test app, `testing::ok` on a gameplay failure, `testing::run_to` and `verify`, when a violated rule matched the start of loading, the tick budget on the command line, frame time (`frames`: a frame is the slower of the two worlds, a frame with `step` does not count, the GPU report, draw commands travel with their own frame), the performance test (`bench`: verdicts, including `draws` and `nav_bake_s`, a definition check, `--perf` and its codes), `counts` without a window, the animation bridge on a real model with a skin and on a humanoid from Blender (a roll in place, a clip from a second file), `--bake-motion` |
| `sandbox` | in one binary, `tests/sandbox`: snapshots (`snapshots.rs`: the state after loading is the same as in the original session, during a roll, a combo, a held charge, a whiff's extra recovery, a telegraph, planning, swimming, a flask, NPC fights and in a generated region), the ruin tile (`tiles.rs`: the file from the test glTF generator matches the one committed), scenarios (`scenario_files`: each file on a fresh game, all of them in a row in one game, the combat prototype after six different pauses), reference replays, actions, content, the obstacle course, determinism, visibility, scope permissions, replays, `kill` and SIGHUP; invariants (`invariants.rs`, the engine's rules `engine_rules.rs` (a control test per rule): the sandbox rules and their limits, sessions, a stop in real time, the replay of a violation, a snapshot; `net_seams.rs`: the client does not check the rules); co-op on a stand (`net.rs`, `net_guards.rs`, `net_lifecycle.rs`, `net_sync.rs`, `net_predict.rs`, `net_seams.rs`, `net_coop.rs`, of which 2 are measurements under `#[ignore]`: joining and leaving, limits and the peer's text, the world reloaded, input, desync, the end of a session and exit codes, stand clocks, UDP on loopback, client prediction, characters and saves); a long game (`soak.rs`: two minutes with a host and two clients at 150 ms and 2 % with a region change and saves, half an hour under `#[ignore]`); the screenshot without a window (`offscreen.rs`, `#[ignore]`: needs a GPU or software rendering); separately `system_failures` (a gameplay failure, a crash replay, a crash report, a panic in another thread, `crash_report`, a return to a snapshot after a failure), `log_capture` (log lines in `step`) and `perf` (tick time: budget, journal, `ai/status`; the performance test on the reference scene without a window, with `offscreen` under `#[ignore]`): these run in their own processes |

The template's tests (`template/tests/game`, 45 and one `#[ignore]`) are built into one binary: scenarios (and the stand scenario), determinism,
replays, content, co-op (host and guest at 150 ms and 2 %: the guest's trophy on each copy and in its file, the next game with it,
the host's replay in a single-player game), the region
(a thousand seeds of the valley, the manifest byte for byte, the loading screen without a window, the column of the ruin, the raider on the navmesh), the manifest (its only
dependency is `shards`).

**Tool contracts** (`crates/sandbox/tests/sandbox/tool_contracts.rs`, in the sandbox test binary, about 15 s in debug). Every
tool of the sandbox that is a query or an action, at any scope, plus `step` (the other control tools move time or the session
itself), is called 30 times with arguments made from its own schema by the fuzzer's generator (see "Fuzzing"): 20 that satisfy the schema and
10 that break exactly one argument on purpose, each of the ways in turn. Each tool plays on a fresh game, so tools cannot affect each other. For
every call: no gameplay system fails and the tool does not panic (neither the error nor the journal says so); an error is not empty; and the
error of a call the schema rejects names the tool's signature or the bad argument (the house rule: an error says how to fix the call; a call
whose error is the same without the break, because the other arguments are what the game refuses, is not held to it). At the end of a tool the
session's replay is played from tick 0 in a fresh game and must match, which proves that every successful action call was recorded. `SKIP` in
the file is the one list of tools that cannot run in a test app, each with its reason (it is empty: `quit` and the Steam tools are not
registered in a test app, and `shot` answers without a GPU). The checker proves itself: control tests plant each kind of bug in a tool of a
test game (a tool that panics, one that fails with an empty error, one whose error does not say how to fix the call, an action that is not
recorded, a gameplay system that panics) and show that the checker names it. The generator does not know what a tool wants, so tools with a
narrow domain (`spawn_prefab`, `use_item`) take few of their 20 valid calls; the fuzzer, which makes thousands, goes deeper. `fuzz.rs`
beside it runs the fuzzer in-process: a run with chaos leaves a session that plays back, the same seed plays the same game, `classic` plays
the sandbox, and a planted panic is found and reported. These tests write `snapshots/` and `replays/` in `crates/sandbox/`, so they share the
`snapshot-file` group of `.config/nextest.toml` with the other tests that do.

**The window against the mode without a window.** The window needs a display and a GPU; the screenshot without a window (`--offscreen`) needs a GPU or software rendering,
so they are not run by `cargo test` (the screenshot test without a window is under `#[ignore]`). The replays under Xvfb and software Vulkan check that the window and the mode without a window are identical (`apt install xvfb mesa-vulkan-drivers
libxkbcommon-x11-0`):

```bash
for r in tests/replays/*.json; do xvfb-run -a cargo sandbox --replay $r --verify --windowed; done
# replay finished at tick 2307: identical to the recording (39 checks) [2307 ticks in 6.69s, 44 frames]
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 3.00s, 22 frames]
# …and so on for each file: npc_director (1923 ticks), npc_navmesh (1261), region (353), snapshot (369)
xvfb-run -a cargo test -p shards --test action_render -- --ignored --nocapture   # telegraphs, overlay, markers: two images
```

**CI** (`.github/workflows/ci.yml`, on every push and pull request; the shared steps — `.github/actions/setup`: system packages,
the toolchain from `rust-toolchain.toml`, the cargo cache). All of the jobs' commands can be run by hand.

| job | what it does |
|---|---|
| `test` | `cargo nextest run --workspace --locked --profile ci` without the sandbox's `perf` binary: unit tests, sandbox scenarios (including the world and NPCs), reference replays (`--verify`); then the `perf` binary alone with `-j 1` (profile `ci-perf`); then `cargo test --workspace --doc --locked`. The JUnit files are uploaded as the artifact `junit-test` | 
| `clippy` | `cargo clippy --all-targets --locked -- -D warnings` for the engine and for the template |
| `template` | `template/Cargo.lock` does not diverge from the root one; the template's tests with `cargo nextest run --manifest-path template/Cargo.toml --locked --profile ci` (artifact `junit-template`); `cargo run -- --check-content`, `--verify` of its replays and `--worldgen-report valley --seeds 1..1000` through the game binary |
| `headless-cli` | `cargo sandbox --check-content` (the tile level files and the baked navigation too), `--worldgen-report wilds --seeds 1..1000`; a live sandbox without a window (`--port 0 --port-file`), `shards wait`, `shards scenario crates/sandbox/scenarios/*.scenario`, then the same scenarios in reverse order in the same game; `kill` at the end; live stands: the files in `crates/sandbox/scenarios/stand/` grouped by their `stand` line, each group on a stand started with the same clients and `--net-sim` links (`shards wait --peer all`, `shards scenario`, `shards stand`, `shards stand stop`); then three minutes of `shards soak` on a fresh E7 stand (`--stand 2 --net-sim latency=150,loss=0.02`, a region change and a save every minute) |
| `replays-window` | the same replays (the sandbox's and the template's) in a window under Xvfb: `--verify --windowed`; optional (`continue-on-error`) until it proves stable |

**nextest.** CI runs the tests with [cargo-nextest](https://nexte.st) (`cargo install cargo-nextest --locked`, pinned in the workflow to `0.9.148`);
`cargo test` runs the same tests and stays the local command. nextest starts a process for every test and runs many at once, so what a test shares with
others on disk or in the machine's time has to be handled by the configuration, `.config/nextest.toml` (the template has its own,
`template/.config/nextest.toml`, because it is a separate workspace):

- Profile `ci`: `retries = 2`, `flaky-result = "fail"` (a test that fails and then passes on a retry is reported as flaky and **fails the run**: a flaky
  test is fixed, not retried), `fail-fast = false` (a run shows every failure), and a JUnit file, `target/nextest/ci/junit.xml` (`template/target/nextest/ci/junit.xml` for the template), which the `test` and
  `template` jobs upload (`junit-test`, `junit-template`). Without `--profile ci` there are no retries.
- Test group `snapshot-file` (`max-threads = 1`): the sandbox tests that write a file with a fixed name or remove a folder in `crates/sandbox/` —
  `snapshots/boss_phase2.json`, `crashes/scenario.json`, `crashes/` and `replays/crash-tick50.json` (the `system_failures` binary),
  `replays/invariant-fuse-tick31.json`, `remove_dir("snapshots")` after a test. These are the scenario files that run the whole set, the `snapshots` module, two
  tests of `invariants`, the recorder of the reference replays and the `system_failures` binary. A group works across binaries, so one limit covers
  all of them. Test group `shards-snapshots`: the two `verify_…snapshot…` tests of `shards` (`run.rs`), which use `snapshots/` of `crates/shards/`.
- The locks that stand in the test code (`static Mutex`) stay for `cargo test`, which runs a binary's tests in one process: `SNAPSHOT_FILE` and `Scratch` (the
  files above), `ONE_AT_A_TIME` in `system_failures` and `log_capture` (the process-global panic memory and tracing subscriber), `TURN` in the
  crash tests of `ai_core`, `FOLDER` in `shards`, `CLOCK` in `perf`. Under nextest a lock inside a process serialises nothing; where the
  tests share a file, the group above does it; where they shared a process, the process per test does.
- `perf` (`crates/sandbox/tests/perf.rs`) measures wall time (a tick that sleeps 200 ms against a 120 ms budget), and a busy machine would break it. CI leaves
  the binary out of the main run (`-E 'not binary_id(sandbox::perf)'`) and runs it afterwards, alone, with `-j 1`, under the profile `ci-perf` (it inherits `ci` and keeps its own JUnit file).
- nextest does not run doctests, so `cargo test --workspace --doc --locked` is a step of its own. Today the workspace has no doctest that runs (the code in the documentation is `ignore`, `ron`
  or `text`); the step is there for the first one.
- To check that nothing is lost: `cargo test --workspace -- --list` and `cargo nextest list --workspace` name the same tests (nextest has no doctests).

Three tiers of workflows. Per push and pull request: `ci.yml` (the jobs above). When a job fails, it uploads what the run wrote (`replays/`, `crashes/`, `snapshots/`, `shots/` in the repository root, `crates/*/` and `template/`, and the game logs in the runner's temp folder) as the artifact `failure-<job>`, kept 14 days (`.github/actions/upload-failure`). Every night: `nightly.yml` (01:30 UTC, or by hand with `seed` and `minutes`; the seed is the date as `YYYYMMDD` unless given) with the jobs `heavy` (every reference replay of the engine and the template with `--verify --heavy 10 --heavy-span 30 --heavy-offset <seed % 10>`: a verdict per replay in the job summary; see "Heavy mode"), `worldgen` (the generator's check on 100000 seeds, the sandbox's `wilds` and the template's `valley`), `gpu` (the `#[ignore]`d tests that need a GPU, picked by name and run with nextest `--run-ignored only` under `xvfb-run -a` on Mesa's software Vulkan with `WGPU_FORCE_FALLBACK_ADAPTER=1`: the offscreen tests of the sandbox, `shards_fx`, `shards_scatter` and `shards_look`, the offscreen test of `perf`, and the window test of `action_render`; never the recorders, the measurements or the half-hour soak test; two of those tests assume a real GPU and fail on lavapipe, `offscreen::draws_follow_how_bevy_batches_not_how_many_meshes_are_drawn` and the `perf` one, so they stay for a machine with a GPU) and `perf` (a release build, `--perf reference` and `--perf reference96`, ticks only; the output is the artifact `perf`, there is no gate). If any of them fails, the job `issue` opens an issue labelled `nightly`, or comments on the open one: the failed job and step, the seed, and the links to the run and its artifacts. `fuzz` plays the fuzz campaign (`crates/sandbox/tests/fuzz_campaign.rs`, see "Fuzzing": 50 runs of 2000 rounds from the night's seed, chaos, steering, heavy mode every 300 ticks, no run started after 85 minutes), shrinks every run that stops to a regression scenario and keeps the scenarios, replays and the coverage report as the artifact `fuzz` (inputs `fuzz_runs` and `fuzz_inputs` of a manual dispatch change its size). Also every night: `soak.yml` — the E7 acceptance check, run again: half an hour of `shards soak` on a live stand of a host and two clients (150 ms,
2 %), with a region change every 5 minutes and a save every 3; by hand (`minutes`, `seed`) and on a push with
`[soak]` in the commit message. `record-replays.yml` records the sandbox's and the template's reference replays again on Linux (CI checks them there; Windows records the same bytes: the nine files in the tree were recorded on Windows and again on Linux, byte for byte, see "Time and determinism") and delivers them as the artifact
`reference-replays`: a push with `[record replays]` in the commit message (or a run by hand, once the file is on the default branch), then `gh run download <id> --name reference-replays --dir .` in
the root of the repository. Every Sunday (and by hand, and on a `v*` tag) `cross.yml` verifies every reference replay on `windows-latest` and on `ubuntu-22.04` (another glibc than the `ubuntu-latest` of `ci.yml`) and writes a table of replay, verdict and CPU model into each job's summary; the job fails if a replay does not say `identical to the recording`. The reference replays were re-recorded after
E10.1–E10.3 (looks, foliage and effects change the content hash) and E10.6 (`libm`); Windows and Linux recorded them
identically.

## What's next

The details, item by item against the spec, are in [docs/engine-progress.md](engine-progress.md). All five phases are built:
A (the combat prototype: E1–E3 and parts of E4 and E11), B (the world: E5, E6, levels from Blender, snapshots), C (co-op:
networking E7, saves and settings E8, the "host + clients" stand; acceptance: half an hour of a host and two clients at 150 ms
and 2 % loss without a desync, `shards soak`), D (UI E9.1–3 in `shards_ui`, localization E9.4 in `shards_locale`, audio E9.5
in `shards_audio`, the developer menu, the console and the NPC overlay in `shards_dev` and `shards_npc`, `path_to` and
`clearance` on the navmesh, the check of level file sockets) and E (the platform, E10: stylised materials, foliage, effects
and decals, budgets and the performance test, Steam, Windows builds, the crash report and screenshots without a window). The
simulation maths goes through `libm`, the same on all x86_64 operating systems (section "Time and determinism"): Windows and
Linux record reference replays byte for byte identically and check each other's recordings.

CI (`.github/workflows/ci.yml`) runs on every push and pull request: `test`, `clippy`, `template`, `headless-cli` (with a
three-minute soak on a live stand) and `replays-window` (optional: `continue-on-error`). `soak.yml` plays the half hour of
E7's acceptance every night, and `record-replays.yml` records the reference replays on Linux on demand. Still open:

1. **What needs hardware or accounts the maintainer has:** the packaged build (`shards package`, section "Builds for players
   and Windows (E10.6)") on a real clean Windows machine, and 60 FPS at 1080p on the target PC. Steam (section "Steam
   (E10.5)") is checked on a fake Steam inside the same process, and on the real one: the game server and a player over the
   relay, achievements and lobby search (the test app, one account, one machine); joining the lobby with a second account,
   the overlay and a successful write to the cloud — not yet.
2. **The rest of phase B:** streaming of tile geometry for physics (now only the image is loaded by distance) is not needed yet: a full-size region
   (`reference96`) with the broad phase loads in 0.6 s and ticks in 0.5–0.6 ms (section "Active zones and streaming");
   foliage is not scattered by distance either, so it is sparser on a large region. The navmesh for several bodies is already there
   (body classes, E6.1).
3. **Minidumps and a crash dialog for the player** (E10.7 beyond the one-file crash report): postponed by the owner's decision.
4. Hot reload of code (not done: `--features dynamic` is only a link-time option, see "Building and iterating fast"; patching a running game, for example with subsecond, is still open).

## Building and iterating fast

A cold build of the sandbox takes 8 to 16 minutes, and a Bevy rebuild is the slowest part of an agent's loop. These are the levers, from the cheapest to the most invasive, with what was measured for each. The variants checked against the reference replays, `link.exe`, `rust-lld` and `--features dynamic`, do not change what the code does: all eight verify byte for byte (`--verify`) with each. The job count and `sccache` were not replay-checked; neither changes the code rustc generates.

### Change content, not code

An edit of these needs no rebuild and no restart of a running game (`cargo sandbox --headless --paused` keeps running):

- **Every `*.<kind>.ron` content file** (actions, movesets, prefabs, attacks, movement and camera parameters that are in content, NPC behaviours, perception, attack plans, directors, looks, effects, sounds, music, screens, themes, regions, generators, perf budgets): hot reload checks the edit together with the whole database and applies it (`content_apply`; `content_reload` rereads everything). A broken edit leaves the previous version in effect and prints the file and the line. Check without a game: `cargo sandbox --check-content`.
- **Levels** (`level_reload`), **text** (`assets/locale/<language>/*.ftl`, hot reload; `settings_set group=locale key=language value=ru`), **settings** (`settings_set`), **bindings** (`rebind`).
- **Behaviour you can describe with `shards` commands**: a `.scenario` file runs against a live game (`cargo shards scenario FILE`), and a rule is a `shards assert add`, with no Rust.
- **Tools that set state**: `set`, `teleport`, `heal`, `god`, `spawn_prefab`, `regen`, `npc_set_state`, `console line="speed x=0.25"`. To try a number before it goes into a file, `set` it on the entity.

Two limits: during a co-op session the host refuses `content_apply`, `content_reload` and `level_reload` (clients would desync), and a change to a baked file needs its bake command (`--bake-nav`, `--bake-motion`).

### Faster Rust builds: what was measured

Measured on one Windows 11 PC (16 threads, 31 GB, other agents building on it), `x86_64-pc-windows-msvc`, Rust 1.99.0, the dev profile of `Cargo.toml`. Times are seconds.

**Method.** The two configurations are built in their own target directories and sampled one after the other (A B A B ...), four or five samples each; the table gives the median and the spread (min-max). Before each sample the `rustc` processes on the machine were counted (`Get-Process rustc`), because other builds ran on it; the load column is the range of that count, and series are compared only inside one block of the table (they ran at different loads). Sampled changes: **leaf** adds a public function to `crates/sandbox/src/main.rs` (one crate compiles, then the executable links); **deep** adds one to `crates/shards_action/src/machine.rs` (every crate above it recompiles, then the executable links). The rebuild is `cargo build -p sandbox`. A cold build is one sample per configuration (it takes 7-16 minutes), so its spread is unknown and it only tells large differences. Rerun the samples on your machine before relying on a number: load on a shared machine moves them by tens of percent.

| change | configuration | metric | median | spread | load (rustc running) |
|---|---|---|---|---|---|
| linker | MSVC `link.exe` | leaf rebuild + link, n=5 | 7.1 | 5.4-11.5 | 1-4 |
| linker | `rust-lld` | leaf rebuild + link, n=5 | 3.0 | 2.9-3.7 | 1-4 |
| linker | MSVC `link.exe` | deep rebuild + link, n=4 | 17.1 | 11.5-20.1 | 0-1 |
| linker | `rust-lld` | deep rebuild + link, n=4 | 11.2 | 9.1-14.4 | 0-1 |
| linker | `link.exe` / `rust-lld` | cold build, n=1 each | 929 / 977 | - | 4-5 |
| dynamic | `rust-lld`, static Bevy | leaf rebuild + link, n=4 | 4.9 | 4.1-21.0 (one outlier) | 9 |
| dynamic | `rust-lld`, `--features dynamic` | leaf rebuild + link, n=4 | 2.3 | 2.1-2.9 | 9 |
| dynamic | `rust-lld`, static Bevy | deep rebuild + link, n=7 | 13.9 | 12.6-22.4 | 9-11 |
| dynamic | `rust-lld`, `--features dynamic` | deep rebuild + link, n=7 | 16.2 | 10.2-27.9 | 9-11 |
| test binary | MSVC `link.exe` | `perf` test relinked after a change, n=4 | 14.6 | 9.6-23.5 | 8-9 |
| test binary | `rust-lld` | `perf` test relinked after a change, n=4 | 6.0 | 4.8-31.0 (first sample) | 9-11 |
| test build size | static Bevy / `--features sandbox/dynamic` | `cargo test --workspace --no-run`, 51 executables, disk | 22 GB / 17 GB | - | - |
| jobs | 4 / 8 / 12 jobs, `rust-lld` | cold build, n=1 each | 977 / 633 / 445 | - | 4 / 1 / 4 |
| jobs baseline | no wrapper, 8 jobs (the 633 s of the row above; the reference for the sccache rows, not an A/B: the load differs) | cold build, n=1 | 633 | - | 1 |
| sccache | `RUSTC_WRAPPER=sccache`, empty cache | cold build, n=1 | 494 | - | 5 |
| sccache | same, new target directory, full cache | cold build, n=1 | 526 (0 of 399 hits) | - | 16 |
| sccache | same, the first directory emptied, full cache | cold build, n=1 | 339 (337 of 399 hits) | - | 10 |

- **Linker: `rust-lld` is faster and is the default of the engine repository on Windows.** One changed crate plus the link takes about 3 s instead of 7 s (the spreads do not overlap). A deep rebuild was 2-7 s shorter in each of the four A/B pairs, but with n=4 and overlapping spreads (11.5-20.1 against 9.1-14.4) that is suggestive, not shown. The cold build is the same within what one sample can tell. `.cargo/config.toml` of the engine sets `linker = "rust-lld"` for `x86_64-pc-windows-msvc`; the linker is part of cargo's fingerprint, so switching rebuilds everything once. `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=link.exe` brings the old linker back. The template stays on `link.exe`: the executables a game ships (`shards package`, a `dist` build with thin LTO) still come from it, because the PE file (PDB, CFG, debug info, imports, how antivirus products judge it) was not compared. Cargo reads every `.cargo/config.toml` from the working folder up, so inside this repository the template would inherit the root's `rust-lld`; `template/.cargo/config.toml` therefore sets `linker = "link.exe"`, and `shards package`, which runs cargo in the game's folder, links with it (a game kept outside the repository gets `link.exe` either way). A build run from the repository root with `--manifest-path template/Cargo.toml` (the tests, CI) reads only the root's config and links with `rust-lld`: those are test builds, not shipped ones. A game may opt in for its dev builds with `linker = "rust-lld"` in its own `.cargo/config.toml`. With no `LIB` set in the shell the build still works: rustc finds the Visual Studio and Windows SDK libraries itself for this linker, so a missing VS or SDK install fails the link, as it does with `link.exe`. CI: the only Windows job is `cross.yml` (weekly, also `workflow_dispatch`), so Windows CI first sees `rust-lld` in its next run; `ci.yml` is Linux only. `rust-lld` is in every rustup toolchain (`lib/rustlib/<host>/bin`), so nothing is installed. Not measured: Linux. Per its release notes Rust has linked `x86_64-unknown-linux-gnu` with `rust-lld` by default since 1.90, so there is probably nothing to switch on there, but that was not tried; `mold` was neither measured nor configured.
- **`--features dynamic` (Bevy as one DLL): a small win for the link, none for a deep rebuild.** It builds and runs on Windows MSVC (`bevy_dylib.dll` is 140 MB; the 65535-export limit is not hit), and the eight reference replays verify identically with it. With `rust-lld` the static link is only about 3-5 s, so the DLL seems to save about 2-3 s on a leaf rebuild (n=4, all at load 9, one static outlier: suggestive, not shown) and nothing visible on a deep one (seven samples each, medians 13.9 s static and 16.2 s dynamic, spreads 12.6-22.4 and 10.2-27.9, load 9-11). The blocks ran at different loads, so compare only inside a block: a deep rebuild is dominated by compiling the crates above the changed one. What it does save is disk: the workspace's 51 test executables take 17 GB instead of 22 GB, and `cargo test -p sandbox --features dynamic` passes, reference replays included. It stays an option: `cargo sandbox --features dynamic`, or `--features sandbox/dynamic` together with `--workspace`. Do not use it for `dist` builds. Run the executable through cargo (`cargo run`, `cargo test`): cargo puts the DLL and Rust's `std-*.dll` on the search path. Started by hand, both folders (`target/debug`, and `lib/rustlib/<host>/lib` of the toolchain) must be in `PATH`.
- **Jobs: more is faster, until the machine runs out of memory.** A cold sandbox build took 977 s with 4 jobs, 633 s with 8 and 445 s with 12 (single samples, other builds running; at one moment 21 `rustc` processes on the machine held 8.7 GB). But a `cargo test --workspace --no-run` with sixteen jobs next to ten other `rustc` processes ran out of memory (`memory allocation failed` in `rustc`). The same run had `rust-lld` exit with `0xc000001d` (`STATUS_ILLEGAL_INSTRUCTION`), seen there once and not explained; it is not an out-of-memory status. So `.cargo/config.toml` keeps `jobs = 4`, and a machine with free RAM sets `CARGO_BUILD_JOBS=8` (or 12) for the run; the environment beats the file. Rule of thumb: about 2-4 GB of RAM per job at peak. Do not raise it when several agents or builds share the machine: each of them setting 12 jobs hits the same out-of-memory failure.
- **sccache: not adopted; it helps only a rebuild into the same target directory.** It is not in `.cargo/config.toml`. To try it: `cargo install sccache --locked`, then `RUSTC_WRAPPER=sccache cargo build`. A new target directory in another place got 0 hits out of 399 compilations, also with `SCCACHE_BASEDIRS` set, so the cache does not carry a Bevy build over to a new worktree whose `target/` is elsewhere. In the same directory (after `cargo clean`, for instance) 337 of 399 compilations hit (84%) and the build took 339 s against 494 s for the filling build (single samples at loads 10 and 5). The workspace's own crates build incrementally and are never cached.
- **Test binaries: not consolidated.** The workspace builds 51 test executables; only `shards` (5 integration-test files), `sandbox` (5) and `shards_save` (2) have more than one, the others are already one `tests/<crate>/main.rs`. Relinking the `perf` test binary took a median 6.0 s with `rust-lld` against 14.6 s with `link.exe` (this includes recompiling the test crate), so the linker already cut most of what a binary costs; merging the three crates would save perhaps 8 links of 3-5 s, well under a minute of a full test build, while `perf` has to stay a binary of its own (CI selects `binary_id(sandbox::perf)` and runs it with `-j 1`), and `log_capture` and `system_failures` rely on process-wide state. Nothing was merged.

### Target directory size

Each worktree has a `target/` of its own: 6-7 GB after a sandbox build, 22 GB after `cargo test --workspace --no-run` (17 GB with `--features sandbox/dynamic`). Run `cargo clean` in finished worktrees, or point a long job at a scratch `CARGO_TARGET_DIR` and remove it afterwards. For the sandbox build alone the dynamic build is no smaller (6 GB against 6-7 GB); it pays off with many test executables, each of which links Bevy statically otherwise.

### Numbers still in Rust code (follow-ups)

These are constants in gameplay code that a designer would tune, so changing one needs a rebuild and, if it changes gameplay, new reference replays. Moving them into content (for example a `*.camera.ron`) is a separate change; none was moved here. Epsilons and tolerances (`TINY`, `EPS`, `STRAIGHT`, `ROTATION_TOLERANCE`) belong in code.

| place | constants |
|---|---|
| camera and lock-on, `shards_motion/src/camera/` | `FLICK`, `FLICK_COOLDOWN`, `LOCK_FRAMING_RATE`, `ASSIST_CONE`, `RELEASE_FACTOR`, `MIN_DISTANCE`, `CAMERA_RADIUS`, `TRAUMA_DECAY` |
| controller, `shards_motion/src/controller/` | `FLOAT_RATE`, `SHALLOW_MARGIN`, `LAND_PROBE`, `PUSH_COS` (`locomotion.rs`); `EDGE_PROBE`, `MIN_LEDGE_ROOM`, `MIN_GAP` (`mover.rs`); the defaults of `CharacterParams` (`params.rs`) |
| movement planning, `shards_motion/src/perception/path.rs` | `MANTLE_COST`, `DROP_COST`, `SWIM_FACTOR`, `WATER_DROP`, `GOAL_BAND` |
| navmesh routes, `shards_npc/src/nav/query.rs` | `START_REACH`, `GOAL_REACH`, `GOAL_BAND` |
| NPC avoidance and senses, `shards_npc/src/` | `nav/avoid.rs`: `CROWD`, `SWITCH`, `STILL`; `ai.rs`: `LEASH_SLACK`, `NEAR_SIGHT`; `perception.rs`: `SWITCH` |
| orders, `shards_action/src/` | `orders.rs`: `APPROACH`, `MIN_REACH`; `def.rs`: `SIGHT_HEIGHT` |

## Conventions and building

- The engine has no LLM: no built-in agents, no model calls, no API keys (see [AGENTS.md](../AGENTS.md)).
- Code, comments and documentation are in English. `README.md` is a short project page; the detailed documentation is this
  manual, the rest of `docs/`, `CHANGELOG.md` and `template/README.md`.
- The Bevy version is pinned (`0.20.0`). Update it deliberately, as a separate step: Bevy's API changes in every release.
  Third-party crates (now `parry3d =0.31.1` and its `glamx =0.3.1`, `ron =0.12.2`, `gltf =1.4.1` in `shards_level`) are added after their Bevy 0.20 support is checked, are pinned to an exact
  version, and are hidden behind the engine's API.
- `.cargo/config.toml` sets `jobs = 4` (a small or shared machine can run out of memory with more; `CARGO_BUILD_JOBS=8` or `12` when RAM is free, about 2-4 GB per job) and, on Windows, links with `rust-lld`. See "Building and iterating fast". Dependencies
  are built with `opt-level = 3`, the project's own code with `opt-level = 1`.
- Versions and tags: SemVer; all engine crates share one version (`[workspace.package]`); the maintainer sets the tag; the rules are in
  [CHANGELOG.md](../CHANGELOG.md).
- Features of `sandbox` (and `shards`): `render`, `remote` and `hot_reload` (by default), `audio`, `gamepad`, `wayland`,
  `dynamic`, `trace_tracy`; `shards` and the template also have `dist` (a build for players, `shards package`). Without `render`, the game builds only for `--headless`.
