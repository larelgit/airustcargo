# NPCs

Read this when you touch enemy brains, behaviour, perception, the director, attack plans or avoidance. Part of the [AGENTS.md](../../AGENTS.md) index.

- **NPCs act only through `Intent`** (as the player and the agent do): the engine's NPC brain writes to it in `NpcSystems::Drive`, and does not move
  the body. Behaviour, perception, the director and attack plans are content (`*.behavior.ron`, `*.perception.ron`, `*.director.ron`,
  `*.attack_plan.ron`); a game adds its own logic as `add_npc_condition` / `add_npc_state` by name, not by copying the brain. The state of an NPC
  lives in its components (they are part of the replay and the snapshot), not in resources. Avoiding other bodies also does not move the body: in
  `NpcSystems::Drive`, before `drive_intents`, it edits the route input `NavAgent::desired` (which `drive_intents` writes to
  `Intent`), and its own state `NavAgent::avoiding` and `side`; neighbours are taken in `AiId` order, with no randomness; its numbers are `avoidance` in
  `npc_tuning` or in the behaviour, not constants in code.
- **Encounters** (`shards_encounter`, "Encounters (G9)" in the manual) spawn NPCs in waves and pull the survivors of a placed wave back home
  on a wipe or a reset: they `force_state` the behaviour state the encounter's `pull_back.state` names (so a behaviour needs that state,
  `return_home` kind, and content checks it), and set `EncounterMember.returning` until the member is home; the NPC condition
  `encounter_returning` reads it, so a behaviour can ignore targets on the way. A dormant survivor is placed at home at once.
