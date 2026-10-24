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
- **Live tags** (`forbid_while`, `solo_forbid_while`, `zone_margin`; N8d). They are read only through `LiveZones::tags`, called by `inputs()`
  and only for a plan where `AttackPlanDef::reads_live_tags()` holds: the reads (`ActionDef`, `MovesetDef`) are content reads, so reading them for
  every plan would change existing replays' `content` check. Give `FightCx::live` to every caller that judges a grant or a choice (`direct`,
  `plan_attacks`, `schedule_report`). An attack in flight carries its pressed input (`InFlight`), and `plan_attacks` adds each press with
  its input, so a later NPC on the same tick sees what the press will lay at the target; a new place that presses for the planner must add it too. A live tag is only ever an action's tag (the action a zone, a summon or a spawn runs), so the content check holds the two lists to the actions' tags, not to plan options' tags or names.
- **Gunfire, hiding, ranged fighters, turrets** (F11; "Gunfire, hiding, ranged fighters, turrets (F11)" in the manual). Everything is off until
  content asks for it, and the new state lives on new components that only such NPCs carry: `NpcThreats` (a perception with
  `notice: Threats`, inserted on its first active tick), `NpcFireLine` (an NPC whose plan has `keep_range`), `Turret`. Keep it that way: a field
  on `NpcBrain`, `NpcPlan`, `Grant` or `Npc` changes every recorded hash, and a `db.get` that existing content reaches changes their `content`
  check (tools read with `db.peek`). The line of fire is one trace per NPC per tick (`ranged::fire_lines`, chained after `brain::decide` in
  `NpcSystems::Decide`); the director, the planner and `npc_schedule` read `NpcFireLine` through `schedule::inputs()` and never trace again, and
  `Refusal::LineOfFire` sits after `Machine` (fifteen rules), so the machine is still asked where it was. `hiding_spot` is pure: it walks the
  navmesh in the bake's order, sorts by distance with node ids for ties and casts at most `HIDE_RAYS` rays; keep it free of `SimRng` and of
  query order. The `hide` state keeps its spot in `NpcBrain::point` and `step` (already hashed), not in a new field. The turret system runs after
  `drive_intents` in `NpcSystems::Drive` and writes only `Intent` (look and press); `set_turret_active` is a call where the simulation runs, not
  a net event. A threat heard (`shot_threat`) is an observer: it returns at once where the machine does not simulate.
- **Sequences** (F12; "Sequences: scripted scenes for NPCs (F12)" in the manual). A run holds an NPC with `SequenceHold` and writes its `NpcDrive` itself (`sequence::step`, `NpcSystems::Decide`, after `brain::decide`): the NPC's own navigation, avoidance and steering carry the walk out, so do not add a second walker. The brain is dormant by four places, and a new system that makes an NPC act must honour them: `decide_all` skips a held NPC (its brain, plan, timers and drive stay exactly as they were: do not give `NpcBrain` or `NpcDrive` a field for the hold, they are hashed), `FightingItem::under_orders()` is true for it (the director returns its token, the planner and the combo continuation leave it), `ranged::fire_lines` and `turret::turrets` skip it, `drive_intents` drops any order given to it while the run has it (and the tools `move`, `move_to` and `attack` refuse it in words), and `update_activity` keeps it awake wherever it stands. Perception, `shot_threat` and the memory go on. State lives in the resource `Sequences` (`add_snapshot_resource`, `add_optional_replay_check("sequences")`, a prepare that removes it for a snapshot without it) and the component `SequenceHold`; a step keeps `begun` and ages in whole ticks, so a load mid-step does not repeat what the step does once. Content is read with `get` in the step system (the simulation) and `peek` in `play_sequence`, the tools and the describers. A new step kind is an arm in `begin`, `check` and `Step::text`, a check in `def.rs`, and a test that loads a snapshot part-way through it.
- **Encounters** (`shards_encounter`, "Encounters (G9)" in the manual) spawn NPCs in waves and pull the survivors of a placed wave back home
  on a wipe or a reset: they `force_state` the behaviour state the encounter's `pull_back.state` names (so a behaviour needs that state,
  `return_home` kind, which the content check enforces: a state of any other kind is refused), and set `EncounterMember.returning` until the member is home; the NPC condition
  `encounter_returning` reads it, so a behaviour can ignore targets on the way. A dormant survivor is placed at home at once.
