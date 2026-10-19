# NPCs

Read this when you touch enemy brains, behaviour, perception, the director, attack plans or avoidance. Part of the [AGENTS.md](../../AGENTS.md) index.

- **NPCs act only through `Intent`** (as the player and the agent do): the engine's NPC brain writes to it in `NpcSystems::Drive`, and does not move
  the body. Behaviour, perception, the director and attack plans are content (`*.behavior.ron`, `*.perception.ron`, `*.director.ron`,
  `*.attack_plan.ron`); a game adds its own logic as `add_npc_condition` / `add_npc_state` by name, not by copying the brain. The state of an NPC
  lives in its components (they are part of the replay and the snapshot), not in resources. Avoiding other bodies also does not move the body: in
  `NpcSystems::Drive`, before `drive_intents`, it edits the route input `NavAgent::desired` (which `drive_intents` writes to
  `Intent`), and its own state `NavAgent::avoiding` and `side`; neighbours are taken in `AiId` order, with no randomness; its numbers are `avoidance` in
  `npc_tuning` or in the behaviour, not constants in code.
- **A brain's target for spawned objects is `AimTarget`**, not `Intent.target` (the lock-on): `drive_intents` mirrors `NpcBrain::target` into it every tick (only a standing one: never a dead or downed body) and `shards_action`'s `target_of` reads it after the lock-on, again only while its body stands. Do not make a brain write `Intent.target`.
- **One judge for an attack option.** Whether an option may be chosen now is `refusal()` (`shards_npc/src/planner.rs`) over the inputs that
  `schedule::inputs()` builds; the director's grant (`direct`, step 3) and `plan_attacks` both call it. A new rule for options goes into
  `refusal()` (and `Refusal`, in its place in the order), never into the director or the planner alone, so the grant and the choice cannot
  disagree. A rule that can refuse existing content must not stop the action machine from being asked where it is asked now (after the
  weight, the cooldown and the bans, of a closed option too): what the machine reads is part of the replay's `content` check, so a
  recording made before would diverge; `refusal_passes_exactly_what_the_old_rules_passed` compares the reads. Count the players the director counts with `schedule::living_players`.
- **Tokenless and a plan's own waiting role** (`AttackPlanDef::tokenless`, `wait`; the boss scheduler, N8b). Both are read from the NPC's plan definition each
  time: there is no flag on `Grant`, `AttackToken` or `NpcBrain` (hashed), so a hot reload flips them at once. Every place that counts tokens
  (`direct` steps 1, 1b, 2 and 3, `audit_tokens`, the `chooses` text of `npc_schedule`) leaves a tokenless grant out through `director::tokenless`;
  a new place that counts grants must do the same. `Budget` and `Overlap` are skipped inside `over_budget`/`overlap`, so the grant, the choice and
  the press all skip them. A `wait` NPC takes no slot: `direct` step 4 keeps its role and orbit while it holds a grant, and the planner presses by
  clearing the role; do not seed `brain.orbit` anywhere else.
- **Gates on an option** (`angle`, `players`, `max_repeats`, `forbid_after`; the boss scheduler, N8). They are fields with a neutral default
  and `skip_serializing_if` it, so content that does not use them serializes, replays and hashes as before. History and cooldowns live on the
  NPC (`NpcPlan`), keyed by option name, and a game's phase machine swaps `Npc::plan` and keeps them; add no state to `NpcPlan`, `PastAttack`,
  `Grant` or `Npc` for a gate (they are hashed). `npc_schedule` reports what `refusal()` says by building its inputs with `inputs()`: never
  re-derive a rule in the tool. The invariant `npc.plan_sequence` judges the press with its own short code on purpose, so a planner path that
  skips `refusal()` is caught; change the two together.
- **Encounters** (`shards_encounter`, "Encounters (G9)" in the manual) spawn NPCs in waves and pull the survivors of a placed wave back home
  on a wipe or a reset: they `force_state` the behaviour state the encounter's `pull_back.state` names (so a behaviour needs that state,
  `return_home` kind, which the content check enforces: a state of any other kind is refused), and set `EncounterMember.returning` until the member is home; the NPC condition
  `encounter_returning` reads it, so a behaviour can ignore targets on the way. A dormant survivor is placed at home at once.
