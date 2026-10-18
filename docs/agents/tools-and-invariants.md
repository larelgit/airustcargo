# Tools, invariants, AI visibility and AiId

Read this when you add a tool, an invariant, a describer, or touch `AiId` or `AiVisible`. Part of the [AGENTS.md](../../AGENTS.md) index.

- Write what must always hold in the game as an invariant:
  `add_ai_invariant(name, |w: &World| -> Option<String>)`. The rule only reads the world and returns
  the text of the violation with the entity's id (`e6 Health.current = 700 outside 0..=600`). Rules and their firings
  are not part of the replay hash or the snapshot; the replay records session rules (`assert_add`) as events.
  The engine's own rules are named `<module>.<rule>` and registered with `add_engine_invariant(name, rule)` (shards_core): on by
  default, off in a `dist` build and with `--no-engine-invariants` (`EngineConfig::engine_invariants`; tests, `--verify`, fuzz and soak
  keep them on). A rule not yet proven on all content is offered with `add_opt_in_engine_invariant`
  and checked only when named: `--engine-invariant NAME` (`EngineConfig::opt_in_invariants`); none is today (`voxel.body_in_solid`
  was, until its fuzz runs). Each states in its doc comment what it checks and why any carve-out exists, and has a control test that breaks the
  state on purpose. If one fires, it is a bug in the engine or the game: fix the bug, do not weaken the rule without saying why.
- A new tool is an argument struct with doc comments (they become the schema's description)
  and a Bevy system `In<Call<Args>> -> ToolResult`. Kinds: `query` reads, `action` changes the world
  (journal, diff, replay), `control` manages time and the session. A tool error says how to fix the call.
- The AI sees only entities with `AiVisible` (`AiKind` gives it). What is said about them is decided by
  the describe functions (`register_ai_component`, `register_ai_describer`).
- The engine assigns `AiId`. Do not write it in place (`get_mut`, `set`, BRP `world.mutate_components`):
  the "id → entity" index is kept by the `on_insert` and `on_discard` hooks. Inserting a new `AiId` renumbers
  the entity, and the old id gets a tombstone `renumbered to eN`.
- Encounters (`shards_encounter`): the tools are `encounters`, `encounter key=K`, `encounter_start key=K [wave=N]` and `encounter_reset key=K|all
  [forget=true]`; the conditions `encounter_active`, `encounter_between`, `encounter_wiped`, `encounter_won` and the NPC condition
  `encounter_returning`; the engine rules `encounter.members_tracked`, `encounter.state_sane` and `encounter.lock_matches`. A prefab a run counts
  needs an `AiId` (`AiKind`). A game plugs a downed state in with `add_encounter_out_rule` and cleans up what a member owns with
  `add_encounter_despawn_hook`.
