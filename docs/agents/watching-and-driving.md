# Watching the game and acting in it

Read this before you run, drive, inspect or debug a live game with the `shards` CLI: the cookbook, the tool catalogue, exit codes, rules, fuzzing, parallel games. Part of the [AGENTS.md](../../AGENTS.md) index.

## Commands

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
cargo shards package --manifest-path template/Cargo.toml   # build for players in template/dist/ and its check (`--smoke-ticks`, DLL)
cargo run -p sandbox --release -- --perf reference --offscreen   # performance test: a line per budget, exit code 0, 1 or 2 (no GPU)
```

## Notes

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
  `block`, `parry`, `dodge`, `use_item`, `use_technique`, `lock_on` (`side=left|right` switches), `attack`, `god`, `heal`, `stagger`, `react` (a developer tool: it forces a reaction), `motion_bake`
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
- Exit codes of `shards`: 0 — done, 1 — a tool refused (the error says why), 2 — `shards` itself failed
  (no game, no token, wrong command), 3 — the game stopped: `step` (or `fuzz`, `soak`) halted on a crashed
  system, a broken invariant or a failed heavy check (`--heavy`, see [checking-changes.md](checking-changes.md)).
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
- To check how the game plays fairly: `shards session --scope player`. Then only
  the player's tools are available, and they act through `Intent`, as a human does.
