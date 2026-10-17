# Spec: what the Shards engine needs to make *Осколки Предела*

13 Oct 2026 · @Larel

The Shards engine (repository airustcargo) must become a set of crates. A separate game repository builds *Осколки Предела* on it without touching the engine's code. Below are 11 modules, E1–E11: what to do in each one, where the boundary with the game is, what the AI sees, and when the module is ready. The GDD section numbers follow the game's document.

Progress on this spec is in [engine-progress.md](engine-progress.md).

## The boundary between engine and game

- The engine provides mechanisms and tools: physics, the action system, networking, saves, rule-based generation, UI widgets, and the AI layer.
- The game provides content and numbers: weapons, attacks, enemies, bosses, tiles, regions, story, balance, and appearance.
- Test: if a mechanism would help a second game with a different story, it goes in the engine. If it is a specific "what", it goes in the game.
- The engine has no code with names from the GDD (*Корнерог*, *Ветролесье*, *Эхо*, and so on).
- The sandbox remains the engine's test arena: every module is checked in it.

## General requirements for every module

1. AGENTS.md: the engine has no LLM. Code and comments are in English, documentation is in Russian. Bevy 0.20.0.
2. The simulation runs in `FixedUpdate` inside `SimSet`, and randomness comes only from `SimRng`. The components of a new module go into the replay hash and are recorded in the replay.
3. Everything that affects gameplay works the same without a window or rendering as it does with one.
4. Every module is open to the AI: state goes through describe functions, events through `journal_event`, actions through
   `add_ai_tool` and `add_ai_query`, and stop conditions through `add_ai_condition`.
5. Numbers and definitions live in asset files with hot reload. Tuning the balance does not need a rebuild.
6. A third-party crate is chosen only after checking that it supports Bevy 0.20 and that the project is still maintained. The version is pinned, and the crate itself is hidden behind the engine's API. The game does not import it directly. Then the crate can be replaced and Bevy updated without changes to the game.
7. Every module has unit tests, a scenario in the sandbox, a replay check, and a README section.
8. A module is a series of PRs. Start with phase A (the phase scheme is below).

## Architecture

```text
            game "Осколки Предела" (separate repository)
                               │ depends only on
                               ▼
                     shards (umbrella: EnginePlugins, EngineConfig, prelude)
                               │
   ┌──────────┬──────────┬─────┴─────┬──────────┬──────────┬──────────┬──────────┐
 core      motion     action     content     world       npc        net       save · ui · platform
   └──────────┴──────────┴─────┬─────┴──────────┴──────────┴──────────┴──────────┘
                               │ connected to all modules
                            ai_core  ──►  shards CLI  ◄── agents · CI · humans
                               │
               third-party crates (physics, networking, …) — only inside modules
```

The game sees only the `shards` crate. Modules and third-party crates inside the engine can change without touching the game,
and `ai_core` is connected to all modules and is available to agents, CI, and people through the `shards` CLI.

Proposed crates: `shards` (umbrella), `shards_core`, `shards_motion`, `shards_action`, `shards_content`,
`shards_world`, `shards_npc`, `shards_net`, `shards_save`, `shards_ui`, `shards_platform`, plus the existing
`ai_core`, `sandbox` and `shards_cli`. Small modules can be merged; only the boundary "the game depends on `shards`" stays fixed.

## E1. Core, build and game template

The game lives in a separate repository and connects to the engine with one dependency. Running without a window is the same engine,
only without the picture. According to the GDD (32.8), the generator and the tests run without a window on thousands of seeds.

What to do:

1. The `shards` crate is the game's only dependency: the `EnginePlugins` plugin group and the `EngineConfig` settings
   (window or no window, port for `shards`, seed, tick rate, enabled modules). The crate re-exports what is needed
   from Bevy and the modules.
2. Running without a window is built from the same plugins as with a window, minus the window, rendering and sound. Transforms,
   assets, scenes, states and physics work. Right now the sandbox without a window is built from `MinimalPlugins` — this has to be replaced.
3. Application states: loading, main menu, game, region loading screen, pause. A shared mechanism on Bevy
   States with extension points for the game.
4. Engine versions: semantic versions, git tags, CHANGELOG. The game connects to the engine by tag.
5. The game template in `template/`: a minimal game built only on `shards` — a character, an enemy, an arena, a save, and two-player co-op.
   The template grows together with the modules.
6. The engine's CI on GitHub Actions: tests, clippy, a template build, `--verify` of the reference replays from `tests/replays/`
   (the `replays/` folder is in .gitignore).

Boundary: the game adds its own plugins on top of `EnginePlugins` and does not change the engine's code.

For the AI: `ai/status` shows the engine version and the enabled modules.

Done when:

- the template builds and passes its tests separately from the sandbox;
- the same replay matches with a window (under Xvfb) and without a window;
- CI is green on every PR.

## E2. Movement: physics, character, input, camera

The engine needs physics and a character controller for the list from GDD 11: walking, running, jumping, climbing over, swimming,
dodging, gliding. Plus input and camera from GDD 13.2 and 29.2: lock-on, gamepad, rebinding, hold or press,
camera shake, field of view.

What to do:

1. Physics (candidate: avian3d) in `FixedUpdate`, deterministic on one machine and build. Collision layers,
   trigger volumes, a raycast and shapecast API for gameplay and AI.
2. A kinematic character controller: walking, running, sprinting, jumping, stepping up stairs, climbing over low
   obstacles, slopes up to a maximum angle, swimming on the surface, gliding (slow falling and updraft volumes). A fall from a height is an event for the game.
3. The controller's parameters are an asset with hot reload.
4. Input: an action map (candidate: bevy_enhanced_input) becomes an `Intent`. Keyboard, mouse, gamepad;
   rebinding that is saved; hold or press; contexts (game, menu, aiming). The only way to control the character is `Intent`, as it is now.
5. Third-person camera: orbit, collision with walls, lock-on and switching targets, adjustable aim assist, camera shake that can be turned off, field of view. The camera is only a view: gameplay and AI perception do not depend on it.
6. Warning about off-screen attacks (GDD 13.7): an API that gives the direction for an indicator, based on the attack zones from E3.

Boundary: speeds, jump height, and glider and air current parameters are the game's data.

For the AI: movement mode (ground, air, water, glider) and speed in describe; the tools `jump`, `sprint`,
`glide`; the `clearance` and `path_to` queries from E11 run on physics.

Done when: an agent, following a script, gets through the obstacle course in the sandbox: steps, a ledge, water, a drop with a glider. Without a window and with one the result is the same, and the course's replay matches.

## E3. Actions, combat and animation

Attacks in the GDD (13.4) are data: windup, active window, recovery, cancel windows, movement, damage, poise damage, hitbox shape, transitions, network id. The same system works for enemies and bosses. Plus stamina, an invulnerability window of about 0.18 s and an input buffer of about 0.15 s (13.3), poise (13.5), statuses (15), shared skeletons (30.2).

What to do:

1. An action is an asset: phases with durations; cancel windows and allowed transitions; a displacement curve (baked from the animation instead of root motion at runtime); a cost (stamina, cooldown); hitboxes on active frames (sphere, capsule, cone, box, line along the ground); flags such as "unblockable" or "guard break"; a network id. It replaces the sandbox's current `Action`.
2. The character's action machine: starts from `Intent`, the input buffer (from task 4), cancels by windows, combos as chains of transitions, block and perfect block as windows, dodge with an invulnerability window.
3. Hitboxes and hurtboxes are checked in `FixedUpdate` through physics (shapecast), without relying on bones or render frames. One action hits one target once.
4. The damage pipeline: hit → modifiers (block, invulnerability, armour, critical-hit conditions as game hooks) → damage to health and poise → events `Damaged`, `Staggered`, `Died`. Hits are processed in `AiId` order, not in query order.
5. Fighter resources: health; stamina with a regeneration delay; poise with regeneration and a broken state.
6. A framework for statuses with no specific effects: build-up, a threshold, an effect when full, a duration, a limit on statuses at once, resistance as a multiplier.
7. Animation: glTF import with skins; an animation graph that follows the state of the action (gameplay leads, animation catches up); blending and transitions; a shared skeleton and clip reuse. Animation events are only for effects and sound.
8. Telegraphs: attack zones on the ground and sound cues from the action's data. Danger is not read by colour alone: the zones have symbols (GDD 13.7).

Boundary: specific weapons, movesets, numbers, burning, cold, impulse and bosses are the game's data and code.

For the AI:

- describe for an action's phase, with the time left and the hit zone;
- output for the subject: "you are in the hit zone, step out N m";
- the tools `light_attack`, `heavy_attack`, `block`, `parry`, `use_technique`, `lock_on`;
- refused actions are written to the journal, as in task 4.

Done when: the sandbox, driven by data, reproduces the combat prototype from GDD 39 — a three-hit combo, a heavy attack, block, dodge, stamina, an enemy with four attacks, poise, healing. An agent clears this fight by following a script, the same without a window and with one, and the replay matches.

## E4. Content and levels

The GDD keeps content in data (32.1 ContentDatabase, 32.3) and builds the world from pre-made sections (10).
Bevy has no editor, so the engine needs a content database and a levels pipeline from Blender.

What to do:

1. Content database: typed definitions (actions, weapons, enemies, items, POI, events, tiles) as
   RON or JSON assets, and as BSN once Bevy has a `.bsn` loader. Every definition has a stable
   string id. Hot reload. On load, the schema and references are checked, and the errors are clear.
2. A levels pipeline from Blender: glTF with custom properties becomes engine components (candidate: Skein).
   Markers: spawn points, tile sockets, navigation links, trigger volumes, interactive objects, camps, landmarks.
3. Prefabs: an entity from an asset plus overrides for fields (BSN scenes, once there is a `.bsn` loader).
4. The `shards content check` command: all assets load, references are intact, required markers are in place.
5. Deterministic loading: the simulation starts only when all the needed assets are loaded, and loading does not shift
   the ticks.

Boundary: the tiles, POI, rooms, items and their numbers are the game's assets.

For the AI: the tools `content_list` and `content_get` (what is in the database), `spawn_prefab`, and reloading an asset by
name. Content errors are written to the journal.

Done when: the template builds a level from a Blender file with markers; an edit to a number in an action's asset shows up in the running game without a restart; `content check` finds a broken reference and names the file.

## E5. World generation and streaming

The GDD builds a region from a grid of about 8×8 macro-tiles of ~96 m, with sockets on the edges. It checks the region, and on failure it retries or takes a fallback option (10.1–10.2). The manifest and the generator version are kept forever (10.3), dungeons are built from rooms (10.4), and regions change through a loading screen (7.1). The criterion for stage 4: 1000 seeds with no unreachable required points.

What to do:

1. A generator framework in steps: seed → graph of required points → roads → required cells → filling with compatible tiles with rotations → secondary points → decoration → checks → retry or a fallback
   manifest. The steps are the game's functions, which can be replaced. The engine provides the order, the solver, retries and the report.
2. The socket solver: edge compatibility (height, road, river, drop, passage, ban), rotations, fixed cells, limits on the number of tiles. The result is deterministic from the seed and the generator version.
3. Checks: reachability over the tile graph and navigation links, allowed slopes, distances between points of interest, no isolated roads. The rules are the game's data and hooks.
4. Region manifest: seed, generator version, tiles with rotations, POI, links. It is stored in the save (E8),
   and after a game update the old world is not rebuilt.
5. Dungeons: a graph of rooms made of modules with a guaranteed path; hand-made rooms are fixed nodes of the graph.
6. Batch check without a window: `shards worldgen --seeds 1..1000 --report`. The report: how many seeds passed, the reasons for failure, the distribution of distances between points; a generator crash is reported with its seed number.
7. Streaming: a region loads in full behind the loading screen (per the GDD, up to 10–15 s on an SSD). Inside a region, tiles and their LODs load by distance to the players. Enemy AI runs only in the active zones around the players.

Boundary: the tile set, the biome rules and the specific check thresholds are the game's.

For the AI: in `observe`, the current tile and the nearest points of interest with a direction; the tools `regen`, `goto_tile`,
`worldgen_report`.

Done when: the template generates an 8×8 region from test tiles; 1000 seeds pass the check; the same seed and version give a byte-for-byte identical manifest; the transition between regions with a loading screen works without a window.

## E6. Navigation and enemy AI

Enemies in the GDD move over pre-baked tile navigation and a global graph (32.4), are built from six behaviour archetypes, attack using tokens with an encounter budget (20), and choose attacks with bans on dangerous sequences (21, 22). This is about the game's enemy AI, not about an LLM.

What to do:

1. Navmesh: it is baked for each tile in advance by an offline command, and stitched together at the connection points when the region is built. The global route goes over the tile graph. Path queries work without a window. Candidates: vleue_navigator,
   oxidized_navigation, landmass; or the engine's own, over the tile graph.
2. Agents: path following, collision avoidance, a limit on the chase zone (an enemy does not chase across
   half the region).
3. A framework for NPC behaviour: a state machine or utility scoring, with parameters in data and logic in the game's systems.
   NPC perception: sight through physics, and hearing as events. Randomness only through `SimRng`.
4. Combat director: attack tokens with a limit based on the number of players (the numbers 2, 3 and 4 in the GDD are data), roles for those who wait;
   an encounter budget and a list of allowed combinations.
5. An attack planner shared by enemies and bosses: choosing the next attack by weights, distance and sequence bans
   (for example, no two attacks in a row that overlap the safe zone).

Boundary: the archetypes, the behaviour of specific enemies and the boss scripts are the game's.

For the AI: the NPC's state, its target and token in describe; state changes go to the journal; the tools `nav_path` and
`npc_state`; the navigation overlay (E11).

Done when: an enemy in the sandbox walks around pillars along the navmesh and does not leave its zone; with one player, no more than two of
three enemies attack at the same time; everything matches in the replay.

## E7. Networking and co-op

GDD: up to three players, a host and two guests by invitation, with no dedicated servers (24). The host is authoritative. The client predicts its own movement, shows the start of an attack at once, receives confirmation of damage, and smoothly corrects its position without rollback (32.5). The world is synchronised through the manifest. First LAN or a direct connection, then the Steam relay. A journal of network events and a simulation of latency and loss are needed (32.8). The criterion for stage 3: two players play without critical desyncs.

What to do:

1. Session: single-player, host and client share one architecture. Single-player is a host with no guests.
2. Transport: first a direct UDP connection on a LAN. A transport abstraction, so that the Steam relay from E10 can be added.
   Plus an in-memory transport for tests: the host and the clients are separate `App`s in one process that step
   in sync, and network scenarios become deterministic tests. Candidates: lightyear or bevy_replicon.
3. Input: the client sends an `Intent` with a tick number, and the host applies it on its own tick. This is the same input that replays write,
   so the host's replay records the play of all players.
4. Replication: components are marked for replication; the area of interest is around the players; events (damage, death,
   telegraph) are sent reliably; the positions of other entities are interpolated.
5. Prediction: the client shows its own movement and the start of its own actions (animation, effects) at once; the host
   confirms damage; a position mismatch is corrected smoothly.
6. The world through the manifest from E5: the host sends the generator version and the manifest, the client builds the region itself, and the hashes are compared.
7. Joining and leaving: a join request with a game hook "can join now?" (per the GDD, not during a boss fight).
   When the host leaves, the session ends after saving.
8. Debugging: a journal of network events, an overlay for ping and loss, and simulated latency and packet loss set in configuration.

Boundary: the co-op balance rules (health scaling, a limit on the guest's strength, personal loot) are the game's.
The engine provides the number of players, their ids, and the join and leave events.

For the AI:

- an agent is tied to a specific player, and `observe` works from that player's point of view;
- the host and one or two clients without a window start with one command, each with its own `shards` port;
- desync check: the hashes of the replicated state on the host and the clients every N ticks; a mismatch goes to the journal
  and to a stop condition;
- the tools `net_sim` (latency, loss) and `net_status`.

Done when: the host and two clients without a window run a 30-minute scripted session of agents with 150 ms latency and
2% loss, with no desync; joining and leaving do not lose progress; network scenarios on the in-memory transport run in CI.

## E8. Saves and settings

According to the GDD, the host keeps the world, and each player keeps their own character (24.2). Rewards and unlocks are saved at once, the position is restored at camp, and there are three rotating backup copies (32.6). Corrupted saves are one of the main risks (36). Settings and accessibility are section 29.

What to do:

1. Two kinds of saves: the character (for each player) and the world (for the host). Slots.
2. The save schema is built from explicit structures, not a dump of the world. The file has a version number and a chain of migrations "version → version";
   the migrations are checked by tests on samples of old files.
3. Writes are atomic (a temporary file and a rename), with a checksum. Three rotating backup copies;
   on corruption, the save is restored from a copy, with an entry in the journal.
4. A "save now" API for rewards and unlocks; autosave on game events (camp, beacon, boss).
5. A registry of what is saved: the game marks which components and resources go into the character's or the world's save.
6. Settings on Bevy 0.19's settings framework: graphics, sound, controls (rebinding from E2), accessibility
   (shake, blur, flashes, text size, field of view, hold or press). They are saved between runs.

Boundary: which fields of the character and the world are saved, and what the slots look like, are the game's.

For the AI: the tools `save_game`, `load_game` and `save_check` — save, load, and compare the hash of the saved
state. A corrupted file gives a clear error in the journal.

Done when: a round-trip test "save → load" on 1000 random states loses no data;
a corrupted file is restored from a copy; a file from an old version migrates and loads.

## E9. UI, audio, localization

The GDD requires a HUD (health, stamina, flask, techniques with cooldowns, *Эхо*, the target's poise, compass, allies,
quest target), about ten screens, and item comparison (27). Sound must help read the fight, and the music must change in layers (31).
Text is localized, and procedural text is put together from whole phrases (6.3, 25.2).
Accessibility — 29.2.

What to do:

1. A set of game widgets on Bevy UI and BSN: bars (health, stamina, poise), icons with cooldowns,
   a compass strip with direction marks, ally frames, tooltips, lists, grids, tabs, and a "before → after" comparison.
2. A stack of screens: open, close, back. In single-player the menu pauses the world; in a network game it does not (GDD 29.2).
   Gamepad navigation and focus.
3. Themes and text scaling; accessibility labels (AccessKit) on all elements.
4. Localization: string tables (candidate: Fluent), a language change without a restart, and procedural text assembled from
   whole localized phrases with parameters.
5. Audio: buses (master, music, effects, interface) with volumes from the settings; adaptive music with layers
   "exploration, tension, combat" and smooth transitions based on the game state; spatial sounds with priorities
   and a voice limit; sound cues for attacks from E3.

Boundary: the game's own screens (inventory, talents, forge), its texts and its music are the game.

For the AI: the UI tree in `observe` (screen, elements, values, focus); the tools `ui_click`, `ui_navigate`,
`ui_read`; the current music layer in describe.

Done when: the template has a HUD and two screens built from engine widgets; an agent goes through the menus only
through the UI tree; the language switches live; the music changes layer when combat starts.

## E10. Rendering, performance, platform

GDD: a low-poly style with vertex colours and accent effects (30); 60 FPS at 1080p on a mid-range PC, foliage
drawn with instancing, LOD, pools, region loading up to 10–15 s (32.7); Windows and Steam with lobbies and relay
(1, 24, 32.5).

What to do:

1. Stylised materials: vertex colours, simple lighting, outlines for readability, presets for low-poly.
2. Foliage and small objects: scattering across tiles with instancing, LOD through visibility ranges, culling.
3. Effects: particles (candidate: bevy_hanabi) with pools; decals for the telegraphs of attack zones from E3.
4. Budgets: frame and tick time (task 11), counts of entities and draw calls in `status`; a performance test
   on the reference scene in the release build.
5. Steam (candidate: bevy_steamworks): lobbies and invites, the relay transport for E7, achievements, cloud
   saves.
6. Windows build: a release profile, asset packing, an icon, a launch check on a clean machine.
7. Crash handler: the log, the crash replay (from task 1) and system details in one file for the report.
8. Screenshots without a window: the camera renders to an image, for CI and for agents on machines with a GPU.

Boundary: models, materials, the effects of specific attacks and art are the game.

For the AI: `perf` in `status` and `step`; the `screenshot` tool, which works without a window; draw-call counts.

Done when: the reference scene (an 8×8 region with foliage and 15 enemies) holds 60 FPS at 1080p on the target PC;
the Windows build starts on a clean machine; two players connect through the Steam relay.

## E11. Developer tools and the AI layer

The GDD (32.8) requires: viewing the seed, generating hundreds of regions, a reachability check, switching to the
boss phase, invulnerability and slowdown, the network log, checking saves, hitbox visualisation, the AI state and
simulated latency. The engine's main idea: all of this is available to an agent through `shards` the same way it
is to a person.

What to do:

1. The in-game developer menu and console call the same tools as `shards`, through one entry point
   `call_tool`.
2. Overlays: hitboxes, hurtboxes and attack zones; the navmesh and paths; NPC state; the network (ping, packet
   loss); tile borders and sockets.
3. Tools: invulnerability, teleporting to a tile or a point, switching to the boss phase, spawning a prefab;
   slowdown already exists (`speed`).
4. Snapshots: save and load the state at any tick (DynamicWorld). A replay can start from a snapshot, and the
   switch to the boss phase should load a snapshot.
5. Perception of the 3D world: `clearance` in eight directions (free, wall, drop, water, with distance),
   `path_to` with distance and turns, line of sight by physics rays instead of `AiOccluder` boxes.
6. Connect the AI layer to the modules: the UI tree (E9), binding the agent to a player and the stand "host +
   clients" (E7), the save tools (E8) and the generator tools (E5).
7. Scenario tests: a scenario file (`shards` commands, expectations and invariants from task 9) runs in CI without
   a window. Each module has its own set of reference scenarios.

Boundary: cheats and menus that the player sees are the game; everything for development is the engine.

Done when: every item in list 32.8 of the GDD is available both from the developer menu and from `shards`; the
scenarios of all modules run in CI.

## Order of the phases

The phases follow the GDD stages. The acceptance of a phase is made up of the "Done when" items of its modules,
and scripts check them in CI. E4, E6, E9 and E11 are built in parts: each phase includes only what its acceptance
needs.

The working scheme of the phases (the original diagram was not carried into this document; it is restored from the
text of the spec and the GDD stages):

| phase | GDD stage | modules |
|---|---|---|
| A | combat prototype (GDD 39) | E1; E2; E3; from E4 — the content base, prefabs, `content check`, deterministic loading; from E11 — combat overlays, invulnerability, prefab spawning, scenario tests |
| B | world | E5; E6; from E4 — levels from Blender; from E11 — `clearance`/`path_to` on the navmesh, snapshots |
| C | co-op (stage 3) | E7; E8; from E11 — the stand "host + clients" |
| D | content and UI | E9; the rest of E4 and E11 |
| E | platform | E10 |

Lay the network model down already in phase A, even though E7 itself is phase C: movement and actions are computed
per tick right away and carry network ids, and the host is authoritative. The GDD explicitly warns that co-op added
late will force a rewrite of almost all systems.

## Done when

The engine is ready for *Осколки Предела* when a separate game is built on it with no changes to the engine.

- The game template in a separate repository depends only on `shards` and contains: a character with the movement from E2, combat on data from E3, an enemy with navigation from E6, a generated region from E5, two-player co-op from E7, saves from E8, a HUD and menus from E9.
- All of this is driven through `shards` without a window; the scenarios of each module run in CI.
- The engine code has no content names from the GDD.
- The engine's README explains step by step how to make a game on Shards.
