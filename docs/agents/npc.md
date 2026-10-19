# NPCs

Read this when you touch enemy brains, behaviour, perception, the director, attack plans or avoidance. Part of the [AGENTS.md](../../AGENTS.md) index.

- **NPCs act only through `Intent`** (as the player and the agent do): the engine's NPC brain writes to it in `NpcSystems::Drive`, and does not move
  the body. Behaviour, perception, the director and attack plans are content (`*.behavior.ron`, `*.perception.ron`, `*.director.ron`,
  `*.attack_plan.ron`); a game adds its own logic as `add_npc_condition` / `add_npc_state` by name, not by copying the brain. The state of an NPC
  lives in its components (they are part of the replay and the snapshot), not in resources. Avoiding other bodies also does not move the body: in
  `NpcSystems::Drive`, before `drive_intents`, it edits the route input `NavAgent::desired` (which `drive_intents` writes to
  `Intent`), and its own state `NavAgent::avoiding` and `side`; neighbours are taken in `AiId` order, with no randomness; its numbers are `avoidance` in
  `npc_tuning` or in the behaviour, not constants in code.
- **One judge for an attack option.** Whether an option may be chosen now is `refusal()` (`shards_npc/src/planner.rs`) over the inputs that
  `schedule::inputs()` builds; the director's grant (`direct`, step 3) and `plan_attacks` both call it. A new rule for options goes into
  `refusal()` (and `Refusal`, in its place in the order), never into the director or the planner alone, so the grant and the choice cannot
  disagree. Count the players the director counts with `schedule::living_players`.
