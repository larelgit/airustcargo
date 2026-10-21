---
name: shards-playtest
description: Run the Shards sandbox or the template game headless and drive it with the shards CLI - observe, act, step until a condition, inspect, assert rules, edit content live, save replays and snapshots, take screenshots, run co-op stands, fuzz - to see a change working in the real game, reproduce a bug, explore behaviour or draft a scenario. Use whenever you need to watch or play the game itself rather than only read code or run unit tests.
---

# Playing the game from the terminal

The game is an environment driven from outside: you look with `observe`, act with tools, and move time
with `step`. A paused headless game moves only when you step it, so nothing happens while you think. The
tool catalogue, conditions and exit codes are in `docs/agents/watching-and-driving.md` (and
`docs/agents/combat.md` for fights); this is the loop. Paths are from the repository root.

## 1. Start a game

```bash
python3 .agents/skills/shards-playtest/scripts/live.py start                     # sandbox: --headless --paused, a free port
python3 .agents/skills/shards-playtest/scripts/live.py start --game template     # the template game
python3 .agents/skills/shards-playtest/scripts/live.py start --name b -- --seed 7 --check-every 1
```

It builds first, starts the game in the background (`--port 0 --port-file target/live/<name>.port`, its output
in `target/live/<name>.log`), waits until it has loaded and prints the line to drive it with. A cold build
takes 10-30 minutes: on a fresh machine, build first in the background the way `cargo sandbox` and `cargo shards`
build, `cargo build -p sandbox && cargo build -p shards_cli` (one command naming both packages unifies their
features differently and compiles some crates twice), and start when it is done. The template game builds into
the engine's `target/` too, as CI does. The same by hand is in `docs/agents/watching-and-driving.md`, "Commands".

Every command names the game: `cargo shards --port-file target/live/sandbox.port <command>` from the
repository root (the shell does not keep `export`s between your commands, so pass the flag each time). The
template game writes its token into its own folder (`.shards/` under `template/`), so `live.py start` prints the
`--token` to add.

## 2. The loop

```bash
cargo shards --port-file target/live/sandbox.port observe            # YOU, SEEN, LANDMARKS, the journal since you last looked
cargo shards --port-file target/live/sandbox.port tools              # what you can call (--json: argument schemas)
cargo shards --port-file target/live/sandbox.port act move_to target=e6 within=3
cargo shards --port-file target/live/sandbox.port step 600 --until near:e5:e6:3.5
cargo shards --port-file target/live/sandbox.port inspect e6         # every component of one entity
```

- Read the reply: an `act` answers with `CHANGES` and `JOURNAL`, a `step` with what stopped it and the tick
  time. Take numbers and positions from `observe` and `inspect`, never from a picture.
- **A tool error says how to fix the call.** Read it and change the call; do not repeat it. A combat tool that
  says `ready in 0.2s` wants a `step` of that long first.
- **Step until something, not by guesswork:** `--until journal:died`, `near:A:B:M`, `gone:ID`, `shows:ID:TEXT`,
  `field:eN:Health.current<50`, `named:NAME`, `warning`, `error`; several `--until` stop at the first.
  `step` with no `--until` runs 1 tick; with one, up to 600.
- **Exit codes:** 0 done, 1 the tool refused (the reply says why), 2 `shards` itself failed (no game, no
  token, a typo), 3 the game halted: a gameplay system panicked, an invariant broke or a heavy check failed. On
  3, stop and read `.agents/skills/shards-debug/SKILL.md`: the replay of the failure is already saved.
- Play fair when the question is "can a player do this": `cargo shards --port-file F session --scope player`
  leaves you only a player's tools, which act through `Intent` like the keyboard. `--scope dev` gives the
  cheats back (`heal`, `teleport`, `set`, `god`, `spawn_prefab`, `npc_set_state`).

## 3. Change things while it runs

- **Content:** save a `*.ron` file and the running game applies it on the next tick; the journal says
  `content reloaded: …`, and a broken edit names the file and line and keeps the old version. Text
  (`assets/locale/*/*.ftl`), levels (`act level_reload`) and settings (`act settings_set …`) do too.
- **A number to try before it goes in a file:** `act set id=e6 component=Health path=current value=400`.
- **A rule to catch a bug:** `assert add low_hp 'field:e5:Health.current<30'` makes `step` stop the moment it
  breaks (exit 3) and saves the replay; `assert list`, `assert reset`.
- **Code** needs a rebuild: `live.py stop`, rebuild, `live.py start`. `load-snapshot` brings back the state you
  saved with `save-snapshot` in the new build (same content and seed).

## 4. Keep what you found

- `save-replay replays/<what>.json`: the session from tick 0; `cargo sandbox --replay replays/<what>.json --verify`
  plays it back, and `--headless --paused` instead of `--verify` lets you step through it.
- `save-snapshot snapshots/<what>.json` and `load-snapshot …`: jump back to a moment and try again.
- **A scenario** is the durable form: write the commands you ran, with `expect` lines for what you checked,
  into a `.scenario` file and run it against the game (`scenario FILE`). How to write one that holds up in
  CI: `.agents/skills/shards-verify/references/scenarios.md`.

## 5. Special games

| You need | Start with | Then |
|---|---|---|
| A screenshot (a visual bug) | `live.py start --offscreen` (a GPU, no window; or a window under `xvfb-run -a`) | `shot -o shots/x.png`: a PNG with `eN` labels and a legend |
| Co-op (host and clients) | `live.py start --name coop -- --stand 2 --net-sim latency=150,loss=0.02` | `--peer c1 observe`, `stand`, `soak --minutes 3`; `docs/agents/net-coop-steam.md` |
| Tick time | `live.py start --release -- --tick-budget-ms 5` | `step 600`, the `ticks:` line; `status` field `perf` |
| Random play until a rule breaks | `live.py start -- --coverage` | `fuzz --seed 7 --steps 300 --chaos 0.02 --steer --minimize` |
| The main menu without a window | `live.py start -- --menu` | `act ui_read`, `ui_navigate`, `ui_click` |
| A replay to step through | `live.py start -- --replay replays/x.json` | `step N`, `inspect` |

## 6. Stop what you started

```bash
python3 .agents/skills/shards-playtest/scripts/live.py status      # what is still running
python3 .agents/skills/shards-playtest/scripts/live.py stop NAME   # or --all
```

Never `kill -9` a game: it leaves its token and port file behind. `shards quit` (what `stop` uses) ends it the
way Ctrl+C does, on every OS. Files the game wrote (`replays/`, `snapshots/`, `shots/`, `crashes/`, `saves/`)
are git-ignored; copy what is worth keeping where it belongs (`tests/replays/` only by recording it on purpose).
