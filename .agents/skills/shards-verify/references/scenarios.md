# Writing a scenario file

A scenario is `shards` commands, expectations and invariants in a text file. The same file runs against a live
game (`cargo shards --port-file target/live/sandbox.port scenario FILE`) and in-process in `cargo test`, with the same meaning. The full
format is in `docs/manual.md`, "Scenarios (E11)"; this is how to write one that holds up in CI.

## Where it goes

| Scenario | Folder | Run by |
|---|---|---|
| The sandbox, one game | `crates/sandbox/scenarios/<topic>.scenario` | `scenario_files.rs`: each on a fresh game, then all in one game forward and backward; CI's `headless-cli` also against a live game, in both orders |
| A stand (host and clients) | `crates/sandbox/scenarios/stand/<topic>.scenario`, first line `stand clients=N [latency=MS] [loss=P] …` | in-process stands; CI starts a live stand per distinct `stand` line |
| A fuzz find | `crates/sandbox/scenarios/regress/<YYYYMMDD>-<stop>-seed<S>.scenario`. `shards fuzz --minimize` (a game started at tick 0) writes it into `scenarios/regress/` of the folder `shards` runs in: move it here | each on a fresh game of its seed, forward only; it fails while the bug exists |
| The template | `template/scenarios/` (`stand/` for co-op) | `template/tests/game/scenarios.rs` |

A scenario that a reference replay is recorded from (`RECORDED` in
`crates/sandbox/tests/sandbox/reference_replays.rs`) changes that replay when you edit it: record it again.

## The shape that works

Before writing one, search for the reply text you mean to check (`grep -rn "not enough stamina" crates/*/tests
crates/sandbox/scenarios`): a test may cover it already, and a scenario then adds only what an agent sees.

```text
# What this proves, in a sentence or two, and how (who presses what, which content it relies on).

seed 42

# --- the sandbox as the game spawns it, whatever ran before (this block passed in both orders with every
# scenario of the folder): the monsters stand down, the player is its own self again and holds nothing
act npc_set_state id=e7 state=calm
act npc_set_state id=e8 state=calm
act npc_set_state id=e14 state=calm
act npc_set_state id=e6 state=calm
act heal id=e5
act stop
act block on=false
act lock_on off=true
act sprint on=false
act glide on=false
act god on=false
act set id=e5 component=Fighter path=moveset value=player
act set id=e5 component=CharacterController path=params value=player
act set id=e5 component=Statuses value='[]'
step 200
act heal id=e5
act teleport id=e5 x=0 z=-3 y=0

# --- 1. the thing happens
mark
act move_to target=e8 within=3
step 900 --until near:e5:e8:3.5
expect stopped "within"
act attack target=e8 repeat=true
step 1800 --until "journal:e8 died"
expect journal "e8 died"
expect field e8 Health.current == 0

# --- leave the world as you found it
act heal id=e8
```

- **Put the world in order first.** A scenario never restarts the game: CI runs it on a fresh game and after every
  other file, forward and backward. Undo what any scenario may have left: running actions and cooldowns (`heal`
  clears neither: step past the longest, 3 s in the sandbox's content, as `step 200` does), health (`heal`),
  places (`teleport`), values (`set id= component= path= value=`; several scenarios swap
  the player's moveset), NPC minds (`npc_set_state … state=calm`), toggles (`stop`, `god on=false`,
  `block on=false`, `lock_on off=true`, `sprint on=false`, `glide on=false`), open menus
  (`act? ui_close screen=…`), settings (`settings_set group=… reset=true`). A scenario that needs a region makes
  it (`regen region=… seed=…`); one in the arena needs no undo of a region, which lies beside the arena: a
  `teleport` brings the player back. Agent sessions last from one scenario to the next: give a player-scope
  agent a name of this scenario's own, and end with `session --agent scenario`.
- **Ids come from spawn order**, which content fixes (the sandbox's player is `e5`, the template's hero `e1`):
  `observe` once and copy them. A body spawned during a scenario gets the next free id, which depends on what ran
  before: find it with `observe` or `expect result contains`, or spawn it where the order is fixed.
- **Wait on conditions, not tick counts.** `step 900 --until "journal:winds up"` is robust to timing changes;
  `step 237` is not. Give the limit room (2-3 times what it takes) and follow it with `expect stopped "…"` so a
  timeout is a failure, not a silent pass. Conditions: `journal:TEXT`, `near:A:B:M`, `gone:ID`, `shows:ID:TEXT`,
  `field:ID:Comp.path<N` (or `>`; both strict, there is no `>=`), `named:NAME`, `warning`, `error`. A wrong name lists the game's named
  conditions: `cargo shards --port-file target/live/sandbox.port step 1 --until named:x` answers
  "unknown condition `x`; known: …".
- **`mark` before what you check in the journal**, so `expect journal` and `expect !journal` look only at what
  followed; set it often (a full journal buffer makes `expect !journal` fail on purpose).
- **Expect exact, meaningful text.** `expect result contains "ok: attacking e6"` is case-sensitive and should be the
  part of the reply that proves the point; `journal`, `stopped` and `shows` ignore case. Copy the text from what
  `cargo shards act …` prints, not from a scenario run's log, which squeezes each result onto one line. A JSON
  reply is matched in its pretty-printed form (keys sorted, one value per line): expect one line of it at a time
  (`expect result contains "\"action\": null"`), and pin whole structures in a crate test. Check numbers with
  `expect field eN Comp.path == V` (`<`, `<=`, `>`, `>=`, `!=`).
- **Invariants** (`invariant field e5 Health.current > 0`, `invariant !journal "desync"`) are checked after every
  following `act` and `step`, not between the ticks of one step; guard a long step with an `--until` instead.
- **Errors you expect:** `act? tool …` allows a refusal, and `expect result contains "…"` then checks its text.
  A crash or broken rule the scenario means to cause: `act? step …`.
- **Play fair where it matters:** `session --agent bot --scope player` plays with a player's tools only, through
  `Intent`; `session --agent scenario` switches back. Expectations still see everything.
- **JSON arguments** go in single quotes: `act move_to target='{"x": 11, "z": 66}' path=true`.

## Stand scenarios

The shape of `template/scenarios/stand/coop.scenario` (the template's ids: `e1` the host's hero, `e2` the grunt,
`e7` the guest; the sandbox's stand files are in `crates/sandbox/scenarios/stand/`). Its setup is not optional: a
guest arrives with the character it saved in the last game (`saves/` in the game's folder), trophies included.

```text
stand clients=1 latency=150 loss=0.02
seed 42
# --- every body as content made it, whatever ran before; setup lines run on the host
@host act heal id=e1
@host act heal id=e2
@host act heal id=e7
@host act set id=e1 component=Trophies path=defeated value=0
@host act set id=e7 component=Trophies path=defeated value=0
@host act teleport id=e1 x=0 z=12 y=0
@host act teleport id=e7 x=2 z=12 y=0
@host act teleport id=e2 x=0 z=-10 y=0
@host act stop
@c1 act stop
step 60
invariant !journal "desync"              # without a member: on each of them, after every later act and step

# --- the guest fights: a client's command goes to the host, as a player's input does
@c1 act move_to target=e2 within=3
@c1 act attack target=e2 repeat=true
step 2400 --until named:arena_cleared    # shared: the stand steps together
@host expect field e7 Trophies.defeated == 1
step 120                                 # what the host decided reaches the client a round trip later
@c1 expect field e7 Trophies.defeated == 1
```

A live stand must match the file's `stand` line, its clients and its links, or it refuses the file (the sandbox's
stand files want two clients). Start the one it names: `python3 .agents/skills/shards-playtest/scripts/live.py start
--name stand -- --stand 2 --net-sim latency=150,loss=0.02` (`--game template` for the template's), then
`scenario FILE` with the drive line it prints. `plan.py` prints the command for each changed stand file.

## Iterating on one

1. Start a live game (`.agents/skills/shards-playtest/SKILL.md`) and try the commands by hand.
2. Run the file against it: `cargo shards --port-file target/live/sandbox.port scenario crates/sandbox/scenarios/<topic>.scenario`; the
   output prints each command with its result and the first failing line (`FAIL file:line: …`).
3. Run it twice in a row in the same game: that proves it cleans up after itself, not yet after the others.
4. Run the whole folder against one game, then in reverse, as CI does (about five minutes each way):
   `cargo shards --port-file target/live/sandbox.port scenario --quiet $(ls crates/sandbox/scenarios/*.scenario)`, then
   with `$(ls crates/sandbox/scenarios/*.scenario | sort -r)`.
5. In-process, once the file is in `crates/sandbox/scenarios/` (the test reads only that folder):
   `cargo test -p sandbox --test sandbox scenario_files::` runs each file on a fresh game and all of them in one
   game, both orders.
