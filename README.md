# Shards

**A deterministic game engine in Rust that coding agents can play, test and debug from the terminal.**

Shards is built on Bevy 0.20. A game depends on one crate, `shards`, and gets a fixed-tick simulation, physics, a character controller, data-driven combat, generated regions, NPC enemies, co-op, saves, UI, audio and the tools to package it for players. All of it is open to agents (Claude Code, Codex and the like) through the `shards` CLI: the world as text, typed actions, time on demand, replays and snapshots.

![The sandbox: a guard winds up an attack and the red strike zone shows on the ground](docs/sandbox.png)

## Why Shards

- **Agents are first-class players.** `observe` describes the world as text, `act` calls typed tools, `step` moves time. An agent does not need to read pixels.
- **No AI inside.** The engine has no LLM, built-in agent, model call or API key. The game is an environment; an agent, a person, a script or CI drives it from outside.
- **Deterministic by design.** The same seed and inputs give the same game, on Windows and on Linux. Every session is a replay that checks the next build.
- **Content, not code.** Numbers, attacks, enemies, screens, texts, sounds, looks and effects are files that reload while the game runs.
- **Bugs stop where they happen.** A broken invariant, a panicking system or a co-op desync stops the game on that tick, with a replay saved.
- **One dependency.** A game never changes engine code. The template game shows the whole path, from a copy to a build for players.

## Features

**For agents**

- `observe` returns the world as text (what is in view, threats, landmarks, what changed); `act` calls typed tools, and `shards tools --json` gives their argument schemas.
- A journal of game events and `warn!`/`error!` lines, each with its tick; enemy attacks are announced before they land (`e6 winds up slam (0.50s)`).
- `step N --until <condition>` runs the simulation up to a journal line, a distance or a game-defined condition; when the game is paused, time moves only on `step`.
- Invariants (`shards assert add`) and panicking systems stop the game on the tick they happen and save a replay; `shards fuzz` and `shards soak` play with seeded inputs until a rule or the sync breaks.
- Scenario files (`*.scenario`) hold commands, expectations and invariants; the same file runs against a live game and inside `cargo test`.
- Replays record every session from tick 0; `--replay FILE --verify` names the first check that diverged, with or without a window.
- Snapshots save the state at any tick and load it back; a replay can start from a snapshot.
- `shards shot` takes a screenshot with `eN` entity labels and a legend, also without a window (`--offscreen`).
- A stand of one host and several clients starts with one command, with simulated latency and packet loss.
- A developer menu (F10) and a console (`` ` ``) call the same tools as the CLI.

**Gameplay**

- A kinematic character controller (walk, sprint, jump, ledge climb, swim) on a physics layer, an input map for keyboard, mouse and gamepad, a third-person camera.
- Combat as data: actions with phases, hit windows, invulnerability, cancels and stamina costs; movesets, block, parry, dodge, lock-on, statuses, ground telegraphs.
- Animation from content: a set maps states and actions to the clips of a glTF skeleton; attack and roll displacement is baked from the clips.
- NPCs with behaviour state machines, sight, hearing and memory, and attack plans, all in content; a leash keeps an enemy near its home.
- A combat director hands out attack tokens, so a limited number of enemies attack at once.
- Navigation on a navmesh baked per tile and stitched across the region; `move_to` and `path_to` use the same routes.

**World**

- Procedural regions: tiles with sockets on their edges, rotations, weights and points of interest, assembled by a solver with checks, retries and a fallback map.
- The same seed and generator version give the same region byte for byte; `shards worldgen` checks a thousand seeds in one command.
- Dungeons from the same generator.
- Levels from Blender: glTF with custom properties becomes colliders, sensors, markers, prefabs and components.
- Regions load through a loading screen; active zones around the players keep the rest dormant, and tile visuals stream in by distance.

**Co-op**

- Host-authoritative sessions: clients send input, the host runs the world and sends the state; one code path for single player, host and client.
- Prediction of the player's own body and smoothing of the other players' bodies.
- Desync detection (`net: desync …`).
- In-memory, UDP (LAN) and Steam relay transports (the relay sits behind the optional `steam` feature); `--net-sim latency=150,loss=0.02` simulates a bad link.
- Saves work in co-op: the character travels with the player, the world is stored by the host.

**Content, UI and audio**

- Numbers and definitions are RON files (`assets/content/**/*.<kind>.ron`) that reload while the game runs; an error names the file, the line and the id and keeps the previous version.
- Prefabs by component name with `extends`; `--check-content` finds broken references without starting the game.
- HUD and menus are content (`*.screen.ron`, themes); agents read the UI tree and click through it with `ui_read` and `ui_click`.
- Fluent localization: languages as content, parameters and plurals, the language switched while the game runs.
- Audio: sounds, buses with the player's volumes, voices with priorities and limits, positional sounds and music in layers that follow the game; agents listen through the `audio` tool.

**Saves and settings**

- Saves: engine and game sections, slots, a versioned format with migrations, atomic writes with three copies and recovery, autosave, Continue in the main menu.
- Player settings (graphics, audio, controls, accessibility) in `settings.toml`, with key rebinding.

**Look and platform**

- Stylised materials from content: toon bands, flat low-poly faces, outlines, glow.
- Foliage and small props scattered over region tiles from content, with levels of detail.
- Particle effects from pools, and attack telegraphs drawn as decals on the ground.
- Performance budgets: tick time in every `step` reply, frame time in `shards status`, and an autotest (`--perf`) that checks the budgets of a reference scene.
- Crash reports with the stack, the machine, the log and a replay of the session.
- `shards package` builds a game for players and checks the result (Windows and Linux).
- The game runs without a window (`--headless`), or with the GPU drawing but no window (`--offscreen`).

## Quick start

```bash
# 1. System packages (once, Linux; needs sudo):
sudo apt install pkg-config libasound2-dev libudev-dev

# 2. The game without a window, paused: time moves only on `step`. A free port, written to a file:
cargo sandbox --headless --paused --port 0 --port-file /tmp/shards.port &
export SHARDS_PORT_FILE=/tmp/shards.port

# 3. From the same or another terminal:
cargo shards wait                          # ready: tick 0 · pid 4242 · seed 42 · port 40817
cargo shards observe                       # a text snapshot of the world + the journal since last time
cargo shards tools                         # what can be done (--json: with argument schemas)
cargo shards act attack target=e6 repeat=true
cargo shards step --until journal:winds    # the reply has CHANGES and JOURNAL
cargo shards content check                 # ok: 133 definitions in 39 files
cargo shards save-replay                   # the session from tick 0 → replays/tick<T>.json
kill 4242                                  # the pid from `wait`; not kill -9: the game removes its token and port file

# 4. A recorded session replayed without a window; --verify compares it with the recording:
cargo sandbox --replay tests/replays/combat_prototype.json --verify
# replay finished at tick 2307: identical to the recording (39 checks)

# 5. A window with the game, and a screenshot with eN labels (needs a display and a GPU):
cargo sandbox
cargo shards shot
```

`cargo sandbox` and `cargo shards` are aliases from `.cargo/config.toml` (`cargo run -p sandbox --` and `cargo run -q -p shards_cli --`); flags after them go to the game or to the CLI. Rust is installed through rustup, and the version comes from `rust-toolchain.toml`. The first build of Bevy is slow, and `.cargo/config.toml` limits it to four parallel jobs (raise `jobs` if you have the RAM).

Run `shards` from the game folder or below it: the token that lets it talk to the game lies in `.shards/<port>.token`. On Windows, where there are no signals, stop the game with `cargo shards quit`. In the window: WASD or arrows to walk, mouse for the camera, Space to jump, Shift to sprint, Ctrl to roll, left and right mouse buttons for light and heavy attacks, Q to block.

## Make a game on Shards, step by step

A game is a separate repository with one dependency, `shards`. The starting point is [`template/`](template/README.md): a minimal game with a hero, an enemy, an arena, a generated valley, numbers in content, its own agent tool, a scenario, a replay and tests. Details for every step are in [`template/README.md`](template/README.md).

1. **Copy the template and rename the package.**
   ```bash
   cp -r template ~/games/pit && cd ~/games/pit && git init
   grep -rIl my_game . | grep -v Cargo.lock | xargs sed -i 's/my_game/pit/g'   # the package `pit` and the settings folder `SettingsAppId::new("pit")`; Cargo.lock fixes itself
   ```
2. **Depend on the engine by git.** In `Cargo.toml`, replace the path (`../crates/shards`) with a git dependency. The template uses saves, co-op, screens and texts, so it needs the engine of phases C–E (the future `0.4.0`); the `v0.2.0` and `v0.3.0` tags will not do. Until the maintainer sets a tag, pin a commit:
   ```toml
   [dependencies]
   shards = { git = "https://github.com/larelgit/airustcargo", rev = "<hash>", default-features = false }
   ```
   After `v0.4.0` is released, replace `rev = "<hash>"` with `tag = "v0.4.0"`. The build does not change until you change `rev` or `tag`. `default-features = false` lets your game switch features (`render`, `remote`, `hot_reload`, `audio`, `gamepad`, …) on in its own `[features]`. There are no other dependencies: Bevy, serde and schemars come through `shards`. Install the CLI of the same version with `cargo install shards_cli --git https://github.com/larelgit/airustcargo --rev <hash>` (after the release: `--tag v0.4.0`). Versions and tags are described in [CHANGELOG.md](CHANGELOG.md).
3. **`main` is one line, `build_app` assembles the engine and the game.** `shards::run` reads the command line and handles `kill` and a closed terminal; `EnginePlugins` is the whole engine from one `EngineConfig`.
   ```rust
   fn main() -> std::process::ExitCode {
       shards::run(pit::build_app)
   }

   pub fn build_app(config: &EngineConfig) -> App {
       let mut app = App::new();
       app.add_plugins(EnginePlugins::new(EngineConfig { title: "My game".into(), ..config.clone() }))
           .add_plugins(GamePlugin);
       if !config.headless && !config.testing {
           app.add_plugins(visuals::VisualsPlugin);   // meshes, light, camera: window only
       }
       app
   }
   ```
4. **Build, run, check.**
   ```bash
   cargo test                       # the template's tests, scenarios and replay: no window, seconds
   cargo game                       # a window (`cargo game` is `cargo run --`, see .cargo/config.toml)
   cargo run -- --check-content     # broken references name the file and the field
   cargo game --headless --paused --port 0 --port-file /tmp/my-game.port &   # then drive it with `shards`
   ```
5. **Put numbers in content, then add code.** Movement, actions, movesets, fighters (prefabs) and enemy behaviour are files in `assets/content/`, and they reload while the game runs. Code adds what files cannot: your own content kind (`ContentDef`), an enemy brain that writes into `Intent`, a tool or a condition for agents (`add_ai_tool`, `add_ai_condition`). Create the world in `OnEnter(EngineState::InGame)`, not in `Startup`.
6. **Write scenarios, record replays, set up CI.** A scenario in `scenarios/*.scenario` runs against a live game (`shards scenario`) and in `cargo test`. Save a good session as a reference (`shards save-replay`) and check it with `cargo run -- --replay tests/replays/smoke.json --verify`. CI runs `cargo test --locked`, `--check-content`, `--worldgen-report valley --seeds 1..1000` and `--verify` for every replay; a working `ci.yml` is in the template's guide.
7. **Package it for players.**
   ```bash
   shards package        # writes the program and assets/ to dist/, then checks them
   ```
   The build uses the `dist` profile and feature: window, sound and gamepads, without the `shards` server, hot reload and developer keys. The check runs a copy of the package in a bare environment (`--check-content`, `--smoke 300`) and confirms that the program writes nothing to its own folder.

The rules that keep a game deterministic (gameplay in `FixedUpdate`, randomness only through `SimRng`, order by `AiId`, control only through `Intent`) are in the template's guide, section "Детерминизм: правила игры" (determinism: the rules of the game). Regions, NPC enemies, Blender levels, looks, effects and moving to a newer engine version continue in the [full step list](docs/manual.md#как-сделать-игру-на-shards-по-шагам) of the manual.

## How it works

- **Engine and game.** The engine gives mechanisms and tools: a deterministic tick, physics, a character controller, an action system, a content database and the AI layer. A game gives the content: weapons, attacks, enemies, tiles, story, balance, looks. A mechanism that a second game with a different story would also need belongs to the engine; a concrete "what" belongs to the game's content. So engine code has no names from any particular game, and a game depends only on `shards` and never changes engine code (a test in the template, `manifest.rs`, checks that).
- **The fixed tick.** All gameplay runs in `FixedUpdate`, inside a `SimSet`. Randomness comes only from `SimRng`, and entities are ordered by `AiId`, not by `Entity`. That is what lets a seed and a list of inputs reproduce a game.
- **One way to steer a character.** The keyboard, agent tools, NPC brains and replays all write to `Intent`.
- **The CLI.** `shards` talks to the running game over JSON-RPC: `ai/*` methods on the Bevy Remote Protocol, on 127.0.0.1, with a token per game. There is no MCP layer in between.

| Crate | What it does |
|---|---|
| `shards` | The only dependency of a game: `EnginePlugins`, `EngineConfig`, the standard `main` (`run`), test apps, the prelude |
| `shards_core` | Config, engine states, the loading gate, `Intent` and orders, the module registry |
| `shards_content` | Typed RON definitions, validation, references, hot reload, prefabs |
| `shards_motion` | Physics layer, kinematic character controller, input map, third-person camera |
| `shards_action` | Data-driven action machine, hits, damage, fighter resources, statuses, telegraphs |
| `shards_level` | Levels from Blender: glTF becomes entities, colliders, volumes and markers |
| `shards_world` | Tiles and sockets, the region generator, manifests, dungeons, region loading, active zones, tile streaming |
| `shards_npc` | Behaviour state machines, perception, the combat director with attack tokens, the attack planner, navmesh navigation |
| `shards_net` | Transports (in-memory, UDP), the link layer, the co-op session, the in-process stand |
| `shards_steam` | Steam lobbies and relay transport, achievements as content, Steam Cloud saves (optional `steam` feature) |
| `shards_save` | Save sections, versioned slots with migrations, atomic writes with backups, autosave |
| `shards_settings` | Player settings: graphics, audio, controls, accessibility |
| `shards_ui` | Screens and widgets from content, screen stack, focus and navigation, the UI tree for agents |
| `shards_locale` | Languages as content, Fluent text with parameters and plurals, language switch at runtime |
| `shards_audio` | Buses, voices with priorities, positional sounds, music in layers that follow the game |
| `shards_look` | Stylised materials as content: colours, toon bands, flat faces, outlines |
| `shards_scatter` | Foliage and small props strewn over region tiles from content, with levels of detail |
| `shards_fx` | Particle effects from pools, and attack telegraphs as ground decals |
| `shards_dev` | The in-game developer menu and console |
| `ai_core` | The AI layer: ids, the world as text, the journal, tools, time control, replays, snapshots, invariants |
| `shards_script` | Scenario files and `--until` conditions (no Bevy) |
| `shards_cli` | The `shards` binary: a thin client for the game's `ai/*` methods |
| `sandbox` | The engine's test arena, built on `shards` like any game |
| `template/` | A minimal game to copy; a separate workspace with `shards` as its only dependency |

## Status

All five phases of the engine spec are built and checked:

- **A:** the combat prototype;
- **B:** the world: regions, NPCs, navmesh and Blender levels;
- **C:** co-op, saves and settings;
- **D:** UI, localization and audio;
- **E:** the platform: looks, foliage, effects, performance budgets, Steam, packaging, crash reports and screenshots without a window.

What remains needs hardware or accounts that these checks cannot reach:

- 60 FPS at 1080p on the target PC;
- the packaged build on a clean Windows machine;
- two players over the Steam relay from two accounts.

No version is tagged yet, so games pin a commit (step 2 above). Progress against the spec, item by item, is in [docs/engine-progress.md](docs/engine-progress.md).

Shards is being built for the game *Осколки Предела*, which lives in its own repository. Nothing from that game is in the engine code.

## Documentation

- [docs/manual.md](docs/manual.md): the full reference, every module, its tools, content formats and checks (Russian)
- [template/README.md](template/README.md): the game template's guide (Russian)
- [docs/engine-tz.md](docs/engine-tz.md): the engine specification (Russian)
- [docs/engine-progress.md](docs/engine-progress.md): progress against the specification (Russian)
- [CHANGELOG.md](CHANGELOG.md): versions and changes (Russian)
- [AGENTS.md](AGENTS.md): rules for coding agents that work in this repository (Russian)

## Working on the engine with a coding agent

- There is no LLM in the engine and there must not be one: no built-in or local agents, no model API calls, no API keys. What an agent needs is added as a tool (`add_ai_tool` / `add_ai_query`) or a `shards` command, not as code that thinks for the agent.
- Code and comments are in English; the detailed documentation is in Russian.
- Bevy is pinned at `0.20.0`; do not upgrade it as a side effect of another task.
- Check changes with `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`; if the template can see the change, also `cargo test --manifest-path template/Cargo.toml`.
