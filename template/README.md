# Shards game template

A minimal game on the Shards engine: a character with movement and combat, an enemy, an arena with obstacles, and a generated valley with raider camps (the engine's NPC enemies walk the navmesh and do not leave their camp). The game depends
**only on the `shards` crate** (`Cargo.toml` has one dependency, and `tests/game/manifest.rs` makes sure it stays that way). The numbers live in content files. You can drive the game from a terminal with the `shards` CLI, as coding agents do. The template grows with the engine's modules. For what it does not have yet, see
"What the template does not have yet".

```text
cargo game                          # window: WASD, mouse, Space, Shift, Ctrl, left button, Escape for the menu
cargo game --headless --paused &    # without a window; time stands still until you run `shards step`
cargo game --offscreen --paused &   # also without a window, but the GPU draws: `shards shot` takes a picture
shards wait && shards observe       # what is in the world, as text
shards scenario scenarios/smoke.scenario
cargo test                          # tests, scenario and replay: no window, a few seconds
```

## What's inside

| where | what |
|---|---|
| `src/main.rs` | `shards::run(my_game::build_app)`: the standard `main` (command-line parsing, `--headless`, `--replay`, `--verify`, `--check-content`) |
| `src/lib.rs` | `GamePlugin` (everything that makes the game this game), `build_app` (engine + game + rendering), `test_app` (the same without a window, for tests) |
| `src/arena.rs` | its own content kind `arena` (`ContentDef`), and a system that builds the world in `OnEnter(EngineState::InGame)` |
| `src/brain.rs` | the enemy's brain: it writes an order into `Intent`, and the engine does the rest |
| `src/ai.rs` | what the agent can do in this game beyond the engine: the `taunt` tool, the `arena_cleared` condition, and the music conditions `foe_near` and `in_fight` |
| `src/progress.rs` | what the game puts into a save beyond the engine: the hero's trophies (the character section) and the fallen grunts in the yard (the world section) |
| `src/screens.rs` | the game's own parts of the HUD and menus: trophies on the HUD, a main-menu hint when there is nothing to continue (`add_ui_source`), and "save and quit" (`add_ui_action`) |
| `src/visuals.rs` | meshes, light and camera, only with rendering (feature `render`: a window or `--offscreen`); meshes get one white material, and their colour and light come from looks; characters are a humanoid model with `Animated` |
| `assets/content/*.ron` | **all the game's numbers**: movement, actions, movesets, fighters (prefabs), the level |
| `assets/content/looks.look.ron` | how bodies and walls are drawn (the engine's looks, E10.1): the preset `low_poly` gives flat faces and an outline; `hero`, `grunt` and `raider` start from it (`extends`); `stone` covers everything else; a body takes its look from `"Look"` in its own prefab |
| `assets/content/effects.effect.ron`, `assets/content/effects.fx.ron` | effects (the engine, E10.3): sparks from hits and blocks (fragments), dust from deaths and steps (cubes); the table says which combat event (`damaged`, `blocked`, `died`) and which clip label (`footstep`) plays which effect. Rendering only: in a window or with `--offscreen` |
| `assets/models/humanoid.glb`, `assets/content/anim.animset.ron` | the character: a low-poly humanoid with a skeleton and clips (`idle`, `walk`, `run`, `light_1`, `light_2`, `light_3`, `heavy`, `roll`, `block`, `hit`, `death`), built by the engine's script `tools/blender/humanoid.py` (`-- template`); the set says which clip shows which state and action, and where the steps are in the clips |
| `assets/content/session.net.ron` | co-op for two: the second player plays the hero from the same prefab `hero`; the rest are the engine's numbers |
| `assets/content/screens.screen.ron` | the main menu, HUD, pause, settings and the question before quitting: screens built from the engine's widgets |
| `assets/ui/loading.png` | the loading picture (1280×720): full screen while the game loads, and in the main menu; the engine's example `cargo run -p sandbox --example make_art` draws it |
| `assets/content/languages.locale.ron`, `assets/locale/` | languages (English, Russian) and their Fluent texts: everything the HUD and menus write; see "Languages" |
| `assets/content/audio/`, `assets/sounds/`, `assets/music/` | sounds, the yard music in three layers, and the mixer; see "Audio" |
| `assets/content/region/` | the generated valley `valley`: 16 m tiles, sockets, points of interest, an 8×8 region, a fallback map, navmesh settings |
| `assets/content/npc/` | the raider: behaviour (a state machine), perception, attack plan: the engine's NPC with no line of game code |
| `assets/tiles/ruin.gltf` | a ruin tile from Blender: floor, walls, column, markers (camp, landmark, sockets) |
| `assets/nav/` | baked tile navigation (`cargo game --bake-nav`) |
| `assets/content/hero.action.ron` (`displacement`) | the hero's lunges and roll are baked from the root bone of the clips (`bake`): after you edit a clip, run `cargo game --bake-motion` |
| `scenarios/smoke.scenario` | the hero walks to the enemy and beats it |
| `scenarios/valley.scenario` | the valley from seed 42, a path to the camp over the navmesh, a raider attacks and returns to the camp |
| `scenarios/saves.scenario` | a cleared yard saves itself; a load restores the trophy and the fallen grunt, and a second load adds no extra trophy |
| `scenarios/menus.scenario` | the HUD with the hero's numbers; the player agent goes through the pause menu and settings using only the `ui_*` tools |
| `scenarios/stand/coop.scenario` | a host and a guest on a 150 ms channel with 2 % loss: the guest beats the grunt, the guest's trophy is in every copy of the yard, and the character walks to the guest |
| `tests/game/` | tests: scenarios (and the stand's), determinism, replays, content, co-op, region, audio, screens, manifest |
| `tests/replays/*.json` | reference replays (`smoke`, `valley`): CI checks that the game still matches them |

What the engine gives, and what the game does:

- **Hero**: the character controller (walking, running, sprinting, jumping, climbing a ledge, swimming), the input map
  (keyboard, mouse, gamepad → `Intent`), a third-person camera, and actions (`light_1` → `light_2`, roll) from data.
  The game code has no line for this: the hero is described by the prefab `hero` in `characters.prefab.ron`.
- **Enemy**: the same fighter (`Fighter`) as the hero, with a different moveset. The brain (`brain.rs`) picks a target and an attack, and the engine's `Order::Attack` does the walking, looking and hitting under the same rules as the player: stamina, wind-up, cooldown.
- **Arena**: the floor, fence, walls and the placement of fighters are described in `arena.arena.ron`.
- **Look**: colour and banded light, flat faces and outlines come from the engine's looks (`shards::look`, the "Stylised materials (E10.1)" section of the engine manual). The game names the look in the prefab (`"Look": ("hero")`), and in `visuals.rs` it gives the mesh one white material; there is not a single body colour in the code.
- **Animation**: the hero and the enemies share one model (`models/humanoid.glb`) with a window or `--offscreen`; the set `anim.animset.ron` says what it shows. The engine scrubs an action's clip to the action's progress (gameplay drives it), walk and run clips loop with the body's speed, `hit` plays on a poise break, and `death` plays when the body dies (the body is no longer hidden). Without a window there is no model and nothing plays: the replay and the tests are the same. The lunge and roll offsets (`displacement` in `hero.action.ron`) are baked from the root bone of the clips: if you change a clip, run `cargo game --bake-motion`, otherwise `--check-content` will report a stale curve. A clip label: a step makes dust (`effects.fx.ron`, `cues`). The swing sound `whoosh` is the action's `Sound` effect in `hero.action.ron`: it sounds without a window too.
- **Effects**: the sparks from hits and blocks and the dust from deaths are the engine's effects (`shards::fx`, the "Effects and decals (E10.3)" section of the engine manual) from `effects.effect.ron`; the table `effects.fx.ron` says which combat event plays what. The game has no code for them. The engine draws attack telegraphs on the ground as decals.
- **Valley**: the region the engine generates (`shards::world`) from the rules in the content: tiles with sockets on their edges, rotations, points of interest, and checks (everything is reachable, camps are not at the gate), plus a fallback map. The same seed and the same generator version give the same region, byte for byte. One tile is a file from Blender (`assets/tiles/ruin.gltf`): its objects with custom properties become colliders and markers. To get there, run `shards act regen region=valley
  seed=42`: a loading screen appears, the old region goes away, the hero appears at the gate, and the ticks do not shift. The region stands next to the arena (`origin` is its north-west corner), not in place of it.
- **Raiders**: the engine's NPCs (`shards::npc`): behaviour, perception and attack plans live in the content, routes go over the navmesh (they walk around the columns), the combat director hands out attack tokens, and `Home` keeps the raider within 10 m of its camp. The grunt's brain in the arena (`brain.rs`) is an example of the game's own brain; the raider is an example of the same thing with no code.
- **Agent**: all the engine's tools (`move_to`, `attack`, `dodge`, `step`, `spawn_prefab`, `inspect`, `regen`,
  `nav_path`, `npc_state`, `snapshot_save`, …) and two of its own.
- **Saves**: position, health and region are the engine's sections; the game adds the hero's trophies and the fallen grunts as its own sections (`src/progress.rs`), and a cleared yard saves itself. See "Saves".
- **Co-op**: a second player over the network (`--host`, `--join`) or on a stand (`--stand 1`), with the hero from the same prefab; their character goes with them. The game needs one content file for this. See "Two-player co-op".
- **HUD and menus**: the screens are in the content; the engine draws them, moves the focus and shows them to the agent. The game puts trophies on the HUD and a "save and quit" button (`src/screens.rs`). See "HUD and menus".
- **Languages**: English and Russian in Fluent, switched on the fly from the settings; the trophies on the HUD are a phrase with a plural form. See "Languages".

## Running it from the engine repository

The template lives in `template/` in the engine repository, and there it points to the engine by path (`path = "../crates/shards"`).
System packages (Linux): `sudo apt install pkg-config libasound2-dev libudev-dev` (for a window under Xvfb, also
`xvfb mesa-vulkan-drivers libxkbcommon-x11-0`). The Rust version comes from `rust-toolchain.toml`.

```bash
cd template
cargo test                 # tests for the template (the first Bevy build is slow)
cargo game                 # window (`cargo game` = `cargo run --`, see .cargo/config.toml)
```

The template is a **separate workspace** with its own `Cargo.lock`: the engine's root workspace does not touch it (`exclude = ["template"]`). Do not set `target-dir` in the template's `.cargo/config.toml`: the game's `target/` sits next to its `Cargo.toml`.

## Start your own game, step by step

1. **Copy the folder.**
   ```bash
   mkdir -p ~/games && cp -r template ~/games/my-game && cd ~/games/my-game
   git init
   ```
2. **Rename the package.** The name `my_game` appears in `Cargo.toml` (`name`, `[[bin]]`), `src/main.rs`
   (`my_game::build_app`), `src/lib.rs` (`SettingsAppId::new("my_game")`, `InputSettings::for_game("my_game")`,
   the window title), `src/arena.rs` (`require_content … "my_game"`), and in `tests/` (`my_game::test_app`). To find every place, run
   `grep -rn my_game .`. `SettingsAppId::new("my_game")` names the player's settings folder (`~/.config/my-game/`; `_`
   becomes `-`). Rename it together with the package. Otherwise, games made from the template store their settings in one
   folder and overwrite each other's keys and volume (the test `the_settings_folder_is_named_after_the_package_the_way_a_rename_finds_it`
   will remind you). The line `InputSettings::for_game` migrates the keys from the old `~/.config/my_game/input.ron`,
   which earlier versions of the template wrote. A new game does not need it, and you can delete it.
3. **Connect the engine.** In `Cargo.toml`, replace the path with a git dependency. The template uses saves, co-op, screens and the texts of phases C–E, so you need the engine of those phases (the coming `0.4.0`); the earlier versions `0.2.0` and `0.3.0` will not do.
   Until the repository has any tags (the maintainer adds `v0.4.0`, see the engine's `CHANGELOG.md`), pin a commit:
   ```toml
   [dependencies]
   shards = { git = "https://github.com/larelgit/airustcargo", rev = "<hash>", default-features = false }
   ```
   Then run `cargo update`. When `v0.4.0` is released, replace `rev = "<hash>"` with `tag = "v0.4.0"`. The build does not change until you change `rev` (or `tag`) yourself. To move to a new version, change `rev` (or `tag`), run `cargo update -p shards`, read the "Changed" section of `CHANGELOG.md`, and run the tests and replays.
   The `[features]` list (`render`, `remote`, `hot_reload`, …) passes the engine's features through; `default-features = false` is needed so that the game's own list chooses them.
4. **Build and run it.** `cargo test`, then `cargo game`. If the game is run without `cargo` (directly, as `target/debug/my_game`), it looks for `assets/` next to the executable; set `BEVY_ASSET_ROOT=$PWD`.
5. **Install the `shards` CLI** (once; use the same version of the engine as the game, the same `rev`):
   ```bash
   cargo install shards_cli --git https://github.com/larelgit/airustcargo --rev <hash>   # after the release: --tag v0.4.0
   ```
   Inside the engine repository, you can do it this way: `cargo install --path ../crates/shards_cli`.
6. **Change the content first, then the code.** First check whether files can do the job (section "Content").
7. **Add your own things to `GamePlugin`**: your own content kind (`arena.rs`), brain (`brain.rs`), tool (`ai.rs`).
8. **Write scenarios and record replays** (sections below), and set up CI (section "The game's CI").

## Driving the game with `shards`

The game runs without a window and paused: time moves only with `step`, so the agent (or you) has time to think.

```bash
cargo game --headless --paused --port 0 --port-file /tmp/my-game.port &
export SHARDS_PORT_FILE=/tmp/my-game.port
shards wait                                   # waits for the load, prints the pid, seed and port
shards observe                                # a text slice of the world + the journal since last time
shards tools                                  # tools and signatures (--json: argument schemas)
shards act move_to target=e2 within=3         # the hero walks to the enemy
shards step 200 --until near:e1:e2:3.5        # step until the condition; the reply has CHANGES and JOURNAL
shards act attack target=e2 repeat=true       # hit until the target dies
shards step 1800 --until named:arena_cleared  # a game condition (src/ai.rs)
shards act taunt target=e2                    # a game tool (src/ai.rs)
shards inspect e2                             # all components of the entity
shards save-replay                            # the session from tick 0 → replays/tick<T>.json
```

Run `shards` from the game's folder or below it: the token is in `.shards/<port>.token`. Stop the game with `kill` or
Ctrl+C, not `kill -9`; on Windows, use `shards quit`. For details (the transport, parallel instances, screenshots with `shards shot`,
including without a window with `--offscreen`, and the `Player` and `Dev` permissions), see the engine manual (`docs/manual.md`). To test the game honestly, as a person would,
switch the agent to the player:
`shards session --scope player`.

Controls in the window: WASD or the arrow keys: walking; mouse: camera; Space: jump; Shift: sprint; Ctrl: roll;
left button: attack (a repeated press within the `cancels` window gives a second hit); Tab: lock-on. The layout lives in the
engine (`InputMap::standard`). You can set your own in the content (`*.input_map.ron`, id `default`), or rebind keys with the
`rebind` tool (it is saved in the user's settings folder under the game's name).

For the developer, the engine gives a menu in the window (F10: pause and slow motion, snapshots, the region seed and its check, invulnerability,
hitboxes, NPC minds, network lag, save check) and a console (`` ` ``: a line like `shards act`; Tab completes names).
The agent has the same through the `dev_menu` and `console` tools. The game puts its own items into the content
(`assets/content/dev/*.devmenu.ron`: `run` is the line the item runs, `fill` is the line it lets you complete in the
console); for details, see the engine manual (`docs/manual.md`), "Developer menu and console (E11)".

## Content

The numbers and definitions live in `assets/content/**/*.<kind>.ron`. You do not need to rebuild to edit them: with or
without a window (`hot_reload` is on by default), the file applies as soon as you save it. The change goes into the
journal (`content reloaded: …`) and into the replay. An error in the file names the file, the line and the identifier,
and the previous version stays in force.

| file | kind | what |
|---|---|---|
| `characters.locomotion.ron` | `locomotion` | speed, jump, capsule radius, turning |
| `hero.action.ron`, `grunt.action.ron` | `action` | a timeline: phases, hit windows, invulnerability, cancels, displacement, cost |
| `movesets.moveset.ron` | `moveset` | which input starts which action, the `stagger` and `death` actions |
| `tuning.combat_tuning.ron` | `combat_tuning` | input buffer, limits |
| `characters.prefab.ron` | `prefab` | entities: components by name, `extends`; for `hero`, `grunt` and `raider`, the `Look` component names the look the window draws the body with |
| `looks.look.ron` | `look` | looks (E10.1): colour, banded light (`Toon`), flat faces, outline in pixels; the preset `low_poly`, and `hero`, `grunt` and `raider` with `extends: "low_poly"`, and `stone` |
| `effects.effect.ron`, `effects.fx.ron` | `effect`, `fx` | effects (E10.3): particles, their lifetime, flight, colours and pool (`max_live`); the table of combat events → effect |
| `arena.arena.ron` | `arena` | **the game's own content kind**: floor, walls, who stands where |
| `region/valley.tile.ron` | `tile` | tiles: sockets on the edges, rotations, weight, points-of-interest slots, geometry (`Inline` or `Level(path: …)`) |
| `region/valley.socket_rules.ron` | `socket_rules` | which edges may sit next to each other, and which ones can be walked over |
| `region/valley.poi.ron` | `poi` | points of interest: a label for the AI and a prefab (the camp spawns a raider) |
| `region/valley.region.ron` | `region` | size, tiles, fixed cells, points, checks, `origin`, fallback map |
| `region/valley_safe.tile_map.ron` | `tile_map` | the fallback map, used if no generator attempt passes the checks |
| `region/ruin.level.ron` | `level` | the ruin's level file and its markers; without them it will not pass `content check` |
| `region/valley.nav.ron` | `nav` | the navmesh cell and the body that the tiles are baked for |
| `region/valley.perf.ron` | `perf` | the performance test (`--perf valley`): the valley with seed 42, the hero and six raiders at the gate, the budgets for tick, frame and region loading |
| `region/valley.scatter.ron` | `scatter` | the valley's foliage (the engine, E10.2): grass in the meadows, at the gate and the columns, pebbles on all tiles; density, height band, meshes near and far, the `grass` and `pebble` looks from `looks.look.ron`. Rendering only (a window or `--offscreen`); `shards act scatter` shows how much is scattered |
| `npc/raider.*.ron` | `behavior`, `perception`, `attack_plan` | the raider: states and transitions, sight, choice of attacks |
| `audio/*.ron` | `sound`, `music`, `audio` | sounds (file, bus, volume, priority, distances), music in layers, the mixer: voices and event sounds |

What applies without a restart, and what does not: the movement and combat numbers (`locomotion`, `action`, `moveset`,
`combat_tuning`), and the NPC behaviour and attack plans take effect from the next tick. Foliage (`scatter`) is scattered again
in the same frame. The level (`arena`) and the prefabs are read when entities are created: an edit shows after the game
restarts (or on new entities, for example after `spawn_prefab`). Looks (`looks.look.ron`) change the picture at once: an
edited colour or outline shows as soon as you save the file (with a window or `--offscreen`). Effects (`effects.effect.ron`)
take effect at the next start of the effect.

A check without the game, for CI: `cargo game --check-content` (it reads all the files and starts nothing; exit code 0 or 1), or
`shards content check` against a running game. It also reads the ruin's level file (every node, component and marker), and checks the
baked navigation against the tiles: an edited tile without `cargo game --bake-nav` is an error with the file name. It also checks
the hero's action displacement against the model's clips: an edited clip without `cargo game --bake-motion` is an error,
`displacement is
stale`, with the command to run; it also reads the files of the `anim.animset.ron` set (model, clips, root bone).
The generator is checked on a thousand seeds, without the game: `cargo game --worldgen-report valley --seeds 1..1000`
(exit code 0 if all of them pass).

An automatic test checks performance: `cargo run --release -- --perf valley --offscreen` (without `--offscreen`, it runs only the
ticks, as fast as possible). It loads the valley, sets up the scene from the lines in `region/valley.perf.ron`, measures 600 ticks
and frames at 1080p, and prints a line per budget: `ok`, `OVER` or `-` (not measured here). Exit code 0 means all the budgets are
met, 1 means not, 2 means `--offscreen` has no GPU. A debug build measures but does not judge. The frame times of a running game
are in the `frames` field of `shards status` (see the engine manual, "Frame time and the performance test (E10)").

**Add an enemy.** Copy the `grunt` prefab into `characters.prefab.ron` under a new id (or use `extends: "grunt"` with
changes), add a moveset and actions, and place it in `spawns` in `arena.arena.ron`. A typo in a prefab or action name is a content error with a `did you mean …` hint, not a crash in combat. Spawn it on the fly:
`shards act spawn_prefab id=grunt_brute x=3 z=-4`.

**Add a look.** A line in `looks.look.ron`, for example `"brute": (color: Some("#a04dd9"), flat: true, outline: Some((width: 2.0)))`
(colour `#rrggbb`; the outline is black by default; `shading: Toon(bands: 2)` gives two bands of light), and `"Look": ("brute")` in the prefab. A typo
in a look name in a prefab is a content error with a `did you mean …` hint. What is drawn, and how many meshes each look takes, `shards act
looks` shows (in a game with a window or `--offscreen`; without a picture it says so). In `src/visuals.rs`, a body without a `Look` in its prefab gets the `stone` look;
the game code does not set body colours.

**Add an effect.** A line in `effects.effect.ron`, for example `"heal_glow": (count: 8, rate: 30.0, duration: 0.6, color: "#7dff9e",
glow: true, radius: 0.8, max_live: 48)`, and an event in `effects.fx.ron` (`"healed": "heal_glow"`). The game plays its own moment with
`commands.trigger(PlayEffect::new("heal_glow", EffectAt::Entity(body)))` (`shards::fx::{PlayEffect, EffectAt}`). A pool smaller than one burst, a colour that is not in the right format, or a misspelled event is a content error. To see it: `shards act effect_play effect=hit_spark at=e1` and `shards shot` in a game with
`--offscreen`; `shards act effects` lists the effects and their pools.

**Add your own content kind.** A struct with `Serialize`, `Deserialize` and an implementation of `ContentDef` (like `ArenaDef` in
`src/arena.rs`), `app.add_content::<T>()`, and files `*.<kind>.ron`. `validate` checks the numbers and the references to other
content (`refs.require::<PrefabDef>(…)`).

## Your own tool and condition

`src/ai.rs`: an argument struct with doc comments (they become the schema description, `shards tools --json`) and
a system `In<Call<Args>> -> ToolResult`. The registration call sets the kind:

- `add_ai_tool(name, scope, system)`: an **action**. It changes the world, and it goes into the journal, into `CHANGES` and into the replay;
- `add_ai_query(…)`: reads only, with no journal entry and no replay entry;
- `add_ai_condition(name, |world| bool)`: a name for `step --until named:NAME`.
- `add_ai_invariant(name, |world| Option<String>)`: a rule that must hold after every tick. The function reads the world and returns the explanation while the rule is broken (`e2 Grunt.heavy_chance = 25 outside 0..=1`); the game then stops like after a crashed system: `step` answers `stopped: invariant grunt_chance violated at tick N`, and the session is saved as a replay. `grunt_chance` in `src/ai.rs` is the example, with a control test (`tests/game/content.rs`) that breaks it on purpose. `Offenders` reports the lowest id first with `(+N more)`.

The engine adds its own rules (`shards assert list` shows them as `engine rule`): they check ids, `Intent`, transforms, health, stamina, poise, running actions, NPC states and attack tokens. They are on in tests and in development builds, and off in a build with the `dist` feature, so a shipped game never pauses on a rule; `--no-engine-invariants` turns them off in a development build.

`Scope::Player` is for what a person could do (the tool works through `Intent` and the game rules).
`Scope::Dev` is for cheats and debugging. A tool error must say how to fix the call (see `taunt`).
A new component that the agent should see is exposed through `register_ai_component` or
`register_ai_describer`. Game events go into the journal through `journal_event`.

## Saves

The engine saves on its own where the hero stands and how much health they have (the `position` and `vitals` sections, both belong to the character),
and which region the world is in (`region`). The game adds only its own data, as explicit structures, not a copy of the world
(`src/progress.rs`):

- `Trophies` is the **character** section `trophies`: how many grunts this hero has defeated. The component is in the `hero` prefab,
  and one line puts it into the save: `save_component::<Trophies>(SaveKind::Character, "trophies", 1)`.
- `Fallen` is the **world** section `fallen`: which of the yard's grunts are down. It is a `SaveSection` with `capture`/`apply`. Grunts are identified by their `SaveKey` (`yard.gate_grunt` is the `key` of a spawn in `arena.arena.ron`: it stays with the character when spawns are added, removed and reordered; a repeated key is a content error). Loading puts the named ones down again (without the killer: nobody gets a trophy a second time), and stands up the ones that are down but not named.
- The yard is cleared (`YardCleared`): `autosave_on::<YardCleared>(SaveWhat::All)` writes the world and the character at the end of this tick.

```bash
cargo game --world slot1 --character slot1          # load at start; autosaves write to the same slots
shards act save_game what=all                       # save now (into the current slots)
shards act save_check what=all slot=slot1           # how the world now differs from the file, by section
shards act load_game what=all slot=slot1            # load without restarting
shards act save_registry                            # this game's sections: the engine's and its own
```

The files are in `saves/` next to `Cargo.toml` (in `.gitignore`). In tests, each game gets a temporary folder.
`scenarios/saves.scenario` checks all of this in one file. When a section changes its format, raise its version and add a migration (`add_save_migration`), as the engine manual (`docs/manual.md`) describes in "Saves (E8.1–5)".

## Two-player co-op

A second player joins the host with their own character and leaves with it. The game code has nothing for this. `assets/content/session.net.ron` is enough (two spots: the second hero comes from the `hero` prefab and stands 2 m from the first), together with the determinism rules below. The host runs the world and decides everything. The client sends its inputs with tick stamps and predicts its own body.

```bash
# host and client in one process, each with its own `shards` port; a channel with 150 ms one way and 2 % loss
cargo game --stand 1 --headless --paused --port 0 --port-file F --net-sim latency=150,loss=0.02 &
SHARDS_PORT_FILE=F shards wait --peer all
SHARDS_PORT_FILE=F shards --peer c1 act move_to target=e2           # the guest's body is e7
SHARDS_PORT_FILE=F shards scenario scenarios/stand/coop.scenario
cargo game --host                       # over the network: the host listens on UDP port 7777 (`--host PORT` to use another)
cargo game --join 192.168.1.5:7777 --character slot1   # a second computer, with its own character
```

The guest's trophies are a section of the guest's character. The host sends them to the guest when the yard is cleared, when the guest leaves and when the session ends. The guest brings them to the next game (`tests/game/coop.rs`). Only the host stores the yard (`fallen`).

One rule is easy to break: **an observer that changes gameplay returns at once on the client**. The client simulates nothing, but the host forwards combat events (`Died`, `Damaged`, …) to it for effects, and the observer would fire there as well. `count_the_fallen` in `progress.rs` starts with `shards::core::simulates(role)`. Without this check, the guest would get a trophy twice, and the desync check would itself report it:
`net: desync at host tick 300: e7 Trophies: here {"defeated":2}, host {"defeated":1}`.

## HUD and menus

Screens are content (`assets/content/screens.screen.ron`). The engine draws them and handles their input (`shards::ui`, the section "UI (E9)" of the engine manual).

**The main menu** `main_menu` (`main_menu: true`) greets the player after loading: a picture, "New game" (`tool new_game`),
"Continue" (`tool continue_game`: the newest save of the world and the hero; while there is no readable save, a hint `menu-no-save` from the game's `continue_note` source (`shards::save::can_continue`) appears under the button, and pressing it is refused and the game says why), "Settings" and "Quit". Time stands still in the menu. The world (yard, hero, grunt) appears once the game has started. `build_app` sets `after_loading: EngineState::MainMenu`, but only a run with a window or `--offscreen` shows the menu. `--headless`, tests (`test_app`), replays, `--check-content`, `--smoke` and co-op go straight into the game and play the same game as before. The flags `--menu` (the menu, also without a window: the agent goes through it with `ui_*`) and `--no-menu` (straight into the game, also with a window) set this explicitly:

```bash
cargo game --headless --paused --menu &
shards wait && shards observe                   # UI  main_menu "My game" … main_menu/new_game ◀ focus
shards act ui_click id=main_menu/new_game       # pressed main_menu/new_game: new_game: the game starts at tick 0
```

While the game loads, the window shows the picture `assets/ui/loading.png` (`loading_image` in `build_app` names it). When the region changes (`regen`), the picture is not shown: the region is set up within one call, and there is no other frame between the old region and the new one.

The HUD `hud` is open from the start of the game: bars for the hero's health and stamina (engine sources `you.health`, `you.stamina`), the trophies (game source `trophies`) and a hint. Escape (Start on a gamepad) opens `pause`: "Resume", "Settings" (tabs Display, Sound, Controls: each widget changes a player setting) and "Save and quit", with a confirmation prompt. The menus work with the arrow keys, WASD, the stick, Enter, Escape and the mouse.

The game adds two functions of its own in `src/screens.rs`: the source `trophies` (what to show: `UiValue::Text`, `Ratio` for a bar, …; whose it is: `shards::ui::viewer`) and the action of the `save_and_quit` button. The button saves everything through the `save_game` tool, on behalf of the player who pressed it, and then ends the game. An agent with the `player` scope saves only the hero; the world is saved by the host. A co-op guest simply leaves, and the host sends the guest their hero. Unsaved progress is not lost: if saving fails, the game keeps running. Everything else lives in the screen file, which is edited live.

The agent goes through the menus the same way, using the tree, without a picture:

```bash
shards act ui_open screen=pause                 # opened pause (menu, pauses the game) · focus pause/resume
shards act ui_read                              # each widget: id, value, focus
shards act ui_navigate dir=down                 # like a gamepad: up/down/left/right/confirm/back
shards act ui_click id=pause/settings
shards act ui_set id=settings/music value=0.5   # slider: settings: audio.music 0.8 → 0.5
```

`scenarios/menus.scenario` does this with an agent in the `player` scope. `tests/game/screens.rs` checks the trophies on the HUD, "Save and quit" and the language change. `tests/game/main_menu.rs` checks the main menu.

## Languages

The HUD and menu text is [Fluent](https://projectfluent.org) messages in `assets/locale/<language>/game.ftl`. Screens name them with `@` (`label: "@pause-resume"`). The languages are content in `assets/content/languages.locale.ron`: English and Russian. Anything missing in Russian is taken from English. The player picks a language on the Language tab of the settings (or the agent does it: `shards act settings_set
group=locale key=language value=ru`), and the HUD and menus switch to Russian at once. On first launch, the game takes the system language if it has one (`ru-RU` is Russian), and remembers it in the settings. The engine's default font, Fira Sans (OFL), has Cyrillic letters, so the template has no theme with its own font.

Text from code is whole phrases with parameters, not pieces glued together. The `trophies` source writes `shards::locale::text(world, "hud-trophies", &Args::new().number("count", defeated))`, and the language picks the plural form (`1 trophy`, `5 trophies`; `1 трофей`, `3 трофея`, `5 трофеев`):

```ftl
hud-trophies = { $count ->
    [one] { $count } трофей
    [few] { $count } трофея
   *[many] { $count } трофеев
}
```

A new language is a line in `languages.locale.ron` and a folder `assets/locale/<language>/`. `cargo run -- --check-content` names the errors in the files (with the file, line and column) and any `@id` that is in no language. An edit to a `.ftl` file in a running game takes effect within a second. The game rejects an edit with an error, names it in the journal, and keeps the old text until the error is fixed. The agent sees the languages and their gaps through `shards act locale`, and reads any message with `shards act text id=hud-trophies 'args={"count": 3}'
language=ru`.

## Audio

The content in `assets/content/audio/` decides what sounds. The files are in `assets/sounds/` and `assets/music/`:

- `yard.sound.ron` holds the sounds: grunt attacks (`grunt_swipe`, `grunt_slam`: the telegraphs in `grunt.action.ron` name them, and the sound plays wherever the blow lands), hits, blocks, parries, deaths, healing and menus. Each has a bus (`Sfx` by default, `Ui` for menus), a volume, a priority and, for world sounds, the distances `near`/`far` over which it fades;
- `yard.music.ron` is the yard's music: three stems of one loop and three layers, `explore` → `tension` → `combat`;
- `mixer.audio.ron` is the mixer: 24 voices, the `yard` music from the start of the game, and the engine's event sounds (`damaged` → `hit`,
  `ui_pressed` → `ui_press`, …).

The music layer is chosen by the game's conditions in `src/ai.rs`: `foe_near` (the hero is closer than one and a half times the distance at which a grunt notices them, or next to a raider: the engine's `npc_near` condition), and `in_fight` (a grunt is fighting: the hero is within its `aggro`, or the grunt has an order to attack; or a raider is fighting: `npc_engaged`). Grunts act on the game's brain, not on the engine's NPC brain, so the conditions are the game's own. That is how the game teaches the music its own fights. The tense layer comes in at once; the calm one comes in after five seconds.

Volume is on the Sound tab of the settings: master, music, effects and UI (`audio.master`, `audio.music`, `audio.sfx`,
`audio.ui`). The agent hears the game through the `audio` tool (track, layer, stems, voices, bus volumes). `observe` writes the line `MUSIC
yard: combat for 6.2s`, and the journal writes `[local] music: yard explore → tension`. Sound plays through feature `audio` (on by default). Without a window and without an output device nothing is heard, but the mixer decides the same way, so the tests hear what the player hears.

## Scenarios

`scenarios/*.scenario` are text files with `shards` commands, expectations (`expect`) and invariants (`invariant`). The same file runs against a running game and in `cargo test` without a window:

```bash
shards scenario scenarios/smoke.scenario     # against a running game; exit code 0, 1 (failed) or 2
cargo test --test game scenarios             # in process, without a window, on a fresh game
```

The format (commands, `step --until`, `expect`, `invariant`, `mark`) is described in the section "Scenarios (E11)" of the engine manual. A scenario does not restart the game: it runs on a fresh game, and if it has to run after others too, its first commands put the world back in order. The ids in a scenario (`e1` the hero, `e2` an enemy) follow from the spawn order: `arena.arena.ron` creates the fighters first.

## Replays

The engine records the session from tick 0: inputs and tool calls. Any session can be saved and played again on the same build:

```bash
shards save-replay replays/my-fight.json     # from a running game
cargo game --replay replays/my-fight.json --headless --paused   # play it back and step on from there
cargo game --replay tests/replays/smoke.json --verify           # without a window, exit code 0: the game matched the recording
```

A divergence names the first tick (`DIVERGED at tick 60`). After an edit to gameplay, a divergence is a regression. After an edit to numbers in the content, it is expected: record the reference again. To use a good session as a reference, put its file in `tests/replays/`. These files are checked by the test
`reference_replays_still_match` and in CI through `--verify`. To re-record the template's reference:

```bash
cargo test --test game record_reference_replays -- --ignored
```

The `replays/` folder (where the game writes) is in `.gitignore`, but `tests/replays/` is not. A replay recorded in a window matches the same run without one: rendering does not affect gameplay.

### Determinism: rules for the game

The same seed and the same inputs give the same game. Replays, tests across thousands of seeds, and co-op all depend on this.
The game keeps it that way if:

- all gameplay runs in `FixedUpdate`, in one of `SimSet::{Input, Think, Act, Move, Resolve, Cleanup}`, with an explicit order for the systems (`.chain()`, `.before()`, `.after()`). Two systems that touch the same data with no order between them give a warning in the journal;
- randomness comes only from `SimRng` (`brain.rs`: `rng.stream(id, salt)`), and time only from `Res<Time>` inside a tick;
- entities whose order matters are iterated by `AiId`, not by `Entity` and not in query order;
- characters are controlled only through `Intent` (the keyboard, tools, the brain and the replay write to it);
- `sin`, `cos`, `atan2`, `exp`, `powf` and the like are computed by `ops` (`ops::sin(x)`, `ops::atan2(y, x)`; it is in `shards::prelude`), not by `f32` methods (`x.sin()`). Those use the platform's maths, and a replay recorded on Linux would diverge on Windows. The engine is built with `libm` (Bevy, glam, simba and the engine itself take their transcendental functions from it; glam keeps its SIMD), and `clippy.toml` in this folder (`disallowed-methods`) makes clippy fail on a direct call, so copy it into your game. The promise: the same bits on every OS of one CPU architecture (x86_64: Windows, Linux, Steam Deck), as long as no build of the game uses `-C target-cpu=native` or `-C target-feature=+fma` (with them, glam fuses a multiply and an add into one instruction, and the bits differ). Between x86_64 and ARM (a Mac on Apple Silicon), it does not hold. Verified on one machine: Windows 11 ↔ Ubuntu 24.04 (under WSL1). Both template replays (`smoke`, `valley`) were recorded byte for byte the same on both OSes, and each OS passed `--verify` on the other's recordings. Other x86_64 CPUs, macOS, ARM and Steam Deck hardware are not tested (engine manual, "Time and determinism"). A dependency that turns on glam without SIMD (`scalar-math`, for example parry3d with `enhanced-determinism`) will stop the game from building: with it, Bevy 0.20 stops drawing, and `shards` checks this at compile time;
- the world is created in `OnEnter(EngineState::InGame)`, once the content is loaded (not in `Startup`), in the same order each time;
- an observer of an event that changes gameplay returns at once if the machine does not simulate (`shards::core::simulates`): on a co-op client, combat events are only for effects;
- your own input component is registered with `replay_input::<C>()`, cosmetics are excluded with `replay_ignore::<C>()`, and data outside entities with `AiVisible` (resources) is added to the check with `add_replay_check(name, |world| …)`.

The tests in `tests/game/determinism.rs` check this in your game: the same seed gives the same journal, another seed gives another fight, and `step 300` equals 5 × `step 60`.

## Tests

```bash
cargo test                   # everything: 45 tests (and one `#[ignore]`: recording the references), takes seconds
cargo test --test game determinism
```

One binary, `tests/game` (Bevy is linked once). The game is built for tests through `my_game::test_app(seed)`: without a window, without a logger or a server, loaded and paused at tick 0. The tests drive it with the same tools as the agent: `shards::testing::{call, ok}`.

| module | checks |
|---|---|
| `scenarios.rs` | every file in `scenarios/` runs on a fresh game, and every file in `scenarios/stand/` runs on a stand, started from its own `stand` line |
| `determinism.rs` | the same seed gives the same journal and world; another seed gives another fight; steps in pieces |
| `replays.rs` | record, then play back, and it matches; a changed game names the tick where it diverges; the reference replays |
| `content.rs` | the content has no errors (in the game and as in `--check-content`); typos name the fix; editing a number changes the next blow without a rebuild; `taunt`, `arena_cleared`; the game's rule `grunt_chance` stops the game when it is broken, and is listed next to the engine's rules |
| `coop.rs` | host and guest on a 150 ms channel with 2 % loss: the guest's body is `e7`, the guest's trophy is only with the guest and in the guest's file, no desyncs; with the file from the previous game, the guest arrives with the trophy; the host's replay matches in single-player |
| `region.rs` | a thousand seeds of the valley pass the checks; the same seed and version give the same manifest, byte for byte; going into a region without a window, through the loading screen (the same hero, the tick count did not change); a column from a ruin in a Blender file stands where the tile put it; a raider walks the navmesh and does not go past its leash |
| `audio.rs` | music moves from `explore` to `tension` and `combat` when the hero comes near a grunt, and the drums come in; a grunt's warning plays its own sound, and an open menu plays its own |
| `screens.rs` | the HUD shows the hero's trophies (0, then 1 after a fight); "Save and quit" saves first and then ends the game; while the confirmation is open, the menu buttons cannot be pressed |
| `main_menu.rs` | the player meets the main menu, and the tests go straight past it; "New game" gives the same world and the same fight as a run without the menu, and a session started in the menu plays back without it; "Continue" with no save is refused and the player stays in the menu, and with a save it brings back the hero's trophy; the loading picture is a small 16:9 PNG |
| `manifest.rs` | the only dependency is `shards`, `target-dir` is not overridden, and the settings folder is named after the package (a rename would be caught) |

## The game's CI

A minimal `.github/workflows/ci.yml` for the game's repository (in the engine's repository, the `template` job does the same):

```yaml
name: CI
on: [push, pull_request]
jobs:
  game:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: sudo apt-get update && sudo apt-get install -y pkg-config libasound2-dev libudev-dev
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --locked
      - run: cargo run --locked -- --check-content
      - run: cargo run --locked -- --worldgen-report valley --seeds 1..1000
      - run: |
          for replay in tests/replays/*.json; do
            cargo run --locked -- --replay "$replay" --verify
          done
```

## Builds for players

```bash
shards package        # dist/my_game-0.1.0-<OS>-<arch>/: the program and assets/, then the checks
```

`shards package` builds the `dist` profile (`release` with `lto = "thin"`) with the `dist` feature: window, sound and gamepads without a BRP
server, hot reload and developer keys (console, menu, overlays). On Windows the C runtime is built in, so the Visual C++
Redistributable is not needed. `assets/icon.png` becomes the icon of the program and window, and there is no console window
(`windows_subsystem` in `src/main.rs`). Then `shards package` runs the copy of the folder in a bare environment (`--check-content`, `--smoke 300`) and
checks that the program changes nothing in its own folder and (on Windows) loads only system DLLs. Details: the engine
manual, "Builds for players and Windows (E10.6)".

Without `render`, the game builds only for `--headless` (a server, world generation over thousands of seeds). To check such a build
without `shards`, you can run `cargo run --release --no-default-features -- --smoke 600`: loading and 600 ticks, exit code 0 or 1.

If the game crashes on a player's computer, it writes a report `crash-<time>-tick<N>.json` to `crashes/` in its working folder (when started from
Explorer, this is the program's folder). If it cannot write there (the game is in `Program Files`), the report goes to the player's
folder: `%LOCALAPPDATA%\my_game\crashes` on Windows, `~/Library/Logs/my_game` on macOS, `~/.local/state/my_game/crashes` (or
`$XDG_STATE_HOME/my_game/crashes`) on Linux; stderr names the path. The report has what crashed and where, the game version (`version` in
`build_app` — the version from `Cargo.toml`), the system, the graphics card, the journal, the log and the session's replay. A report about a
panic that took the game down has no replay. Players send it with a bug report, and a report with a replay is played back as a
replay: `cargo run -- --replay crashes/crash-….json`. Details: the engine manual, "Crash reports (E10.7)".

## What the template does not have yet

The template grows along with the engine modules (phases A, B and C are done: movement, combat, content, levels from Blender,
generated regions, NPCs and navigation, snapshots, saves, player settings, two-player co-op; from phase D: HUD and menus, the main menu and
the loading screen image, languages, sound). Placeholders that will be filled in:

- **TODO · own models and animation layers.** Characters use one humanoid model from the engine's script (`models/humanoid.glb`). The grunt and the raider have
  their own look (`looks.look.ron`), but not their own model. The engine has one `AnimationPlayer` per character (there are no "upper body and lower body" layers), so a
  strike while running plays the whole strike clip. Walls and floors are drawn from their own colliders
  (`src/visuals.rs`) with one white material; the ruin comes from its own glTF file.
