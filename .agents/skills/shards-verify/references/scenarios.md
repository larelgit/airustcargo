# Writing a scenario file

A scenario is `shards` commands, expectations and invariants in a text file. The same file runs against a live
game (`cargo shards --port-file F scenario FILE`) and in-process in `cargo test`, with the same meaning. The full
format is in `docs/manual.md`, "Scenarios (E11)"; this is how to write one that holds up in CI.

## Where it goes

| Scenario | Folder | Run by |
|---|---|---|
| The sandbox, one game | `crates/sandbox/scenarios/<topic>.scenario` | `scenario_files.rs`: each on a fresh game, then all in one game forward and backward; CI's `headless-cli` also against a live game, in both orders |
| A stand (host and clients) | `crates/sandbox/scenarios/stand/<topic>.scenario`, first line `stand clients=N [latency=MS] [loss=P] …` | in-process stands; CI starts a live stand per distinct `stand` line |
| A fuzz find | `crates/sandbox/scenarios/regress/<YYYYMMDD>-<stop>-seed<S>.scenario` (written by `shards fuzz --minimize`) | each on a fresh game of its seed, forward only; it fails while the bug exists |
| The template | `template/scenarios/` (`stand/` for co-op) | `template/tests/game/scenarios.rs` |

A scenario that a reference replay is recorded from (`RECORDED` in
`crates/sandbox/tests/sandbox/reference_replays.rs`) changes that replay when you edit it: record it again.

## The shape that works

```text
# What this proves, in a sentence or two, and how (who presses what, which content it relies on).

seed 42

# --- the world as content made it, whatever ran before
act heal id=e5
act teleport id=e5 x=0 z=-3 y=0
act npc_set_state id=e6 state=calm
act god on=false

# --- 1. the thing happens
mark
act move_to target=e6 within=3
step 900 --until near:e5:e6:3.5
expect stopped "within"
act attack target=e6 repeat=true
step 1800 --until journal:died
expect journal "e6 died"
expect field e6 Health.current == 0
```

- **Put the world in order first.** A scenario never restarts the game: CI runs it on a fresh game and after every
  other file, forward and backward. Undo what any scenario may have left: health (`heal`), places (`teleport`),
  values (`set id= component= path= value=`), NPC minds (`npc_set_state … state=calm`), the region (`regen`),
  toggles (`god on=false`, `block on=false`, `lock_on off=true`), open menus (`act? ui_close screen=…`), settings
  (`settings_set group=… reset=true`). Then clean up at the end if you changed something global.
- **Ids come from spawn order**, which content fixes (the sandbox's player is `e5`, the template's hero `e1`):
  `observe` once and copy them. A body spawned during a scenario gets the next free id, which depends on what ran
  before: find it with `observe` or `expect result contains`, or spawn it where the order is fixed.
- **Wait on conditions, not tick counts.** `step 900 --until "journal:winds up"` is robust to timing changes;
  `step 237` is not. Give the limit room (2-3 times what it takes) and follow it with `expect stopped "…"` so a
  timeout is a failure, not a silent pass. Conditions: `journal:TEXT`, `near:A:B:M`, `gone:ID`, `shows:ID:TEXT`,
  `field:ID:Comp.path<N` (or `>`), `named:NAME`, `warning`, `error`. A wrong name lists the game's named
  conditions: `cargo shards step 1 --until named:x` answers "unknown condition `x`; known: …".
- **`mark` before what you check in the journal**, so `expect journal` and `expect !journal` look only at what
  followed; set it often (a full journal buffer makes `expect !journal` fail on purpose).
- **Expect exact, meaningful text.** `expect result contains "ok: attacking e6"` is case-sensitive and should be the
  part of the reply that proves the point; `journal`, `stopped` and `shows` ignore case. Check numbers with
  `expect field eN Comp.path == V` (`<`, `<=`, `>`, `>=`, `!=`).
- **Invariants** (`invariant field e5 Health.current > 0`, `invariant !journal "desync"`) are checked after every
  following `act` and `step`, not between the ticks of one step; guard a long step with an `--until` instead.
- **Errors you expect:** `act? tool …` allows a refusal, and `expect result contains "…"` then checks its text.
  A crash or broken rule the scenario means to cause: `act? step …`.
- **Play fair where it matters:** `session --agent bot --scope player` plays with a player's tools only, through
  `Intent`; `session --agent scenario` switches back. Expectations still see everything.
- **JSON arguments** go in single quotes: `act move_to target='{"x": 11, "z": 66}' path=true`.

## Stand scenarios

```text
stand clients=1 latency=150 loss=0.02
seed 42
@host act heal id=e1
@c1 act move_to target=e2 within=3      # a client's command goes to the host, as a player's input does
step 600 --until named:arena_cleared     # shared: the stand steps together
@c1 expect field e7 Trophies.defeated == 1
invariant !journal "desync"              # without a member: checked on each of them
```

`template/scenarios/stand/coop.scenario` and `crates/sandbox/scenarios/stand/` are the examples. Run one live with
`cargo sandbox --stand 1 --headless --paused --port 0 --port-file F --net-sim latency=150,loss=0.02 &`,
`cargo shards --port-file F wait --peer all`, then `cargo shards --port-file F scenario FILE`.

## Iterating on one

1. Start a live game (`.agents/skills/shards-playtest/SKILL.md`) and try the commands by hand.
2. Run the file against it: `cargo shards --port-file F scenario crates/sandbox/scenarios/<topic>.scenario`; the
   output prints each command with its result and the first failing line (`FAIL file:line: …`).
3. Run it twice in a row in the same game: the second run proves the setup lines.
4. Then in-process: `cargo test -p sandbox --test sandbox scenario_files::` (all files, both orders).
