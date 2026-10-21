# Recipes: the common asks, the files they touch, and how to check them

File names are the template's (`template/assets/content/…`); the sandbox has the same kinds under
`crates/sandbox/assets/content/`. Every content file starts with comments that explain its fields; read them.
The full format of each kind is in `docs/manual.md` (the section named in brackets). After any recipe:
`cargo run -- --check-content` in the game's folder, the scenario that covers it, and the reference replays
(`.agents/skills/shards-verify/SKILL.md`, section 5).

## Balance: a number

Damage, timing and stamina cost: `hero.action.ron`, `grunt.action.ron` (`phases`, `hits[].damage`, `poise`,
`cost`, `cancels`). Speed and jumping: `characters.locomotion.ron`. Health, stamina, poise: `characters.prefab.ron`.
The input buffer and limits: `tuning.combat_tuning.ron`. ["Actions and combat (E3)", "Movement (E2)"]

- Try it live before you write it: `act set id=e2 component=Health path=current value=120`, then save the file;
  the next tick uses it (`content reloaded: …` in the journal).
- A number the simulation reads moves the reference replays that read it (`which_replays.py` names them):
  record them again once the change is what you want.

## An enemy

1. A prefab in `characters.prefab.ron`: `extends: "grunt"` and only what differs (`Health`, `Fighter.moveset`,
   `Look`, the game's `Grunt` numbers). A body needs `AiKind` to be seen by agents (with `extends`, it inherits it).
2. Its moves: a moveset in `movesets.moveset.ron` (`inputs`, `stagger`, `death`) and actions in a `*.action.ron`;
   every action's `net_id` is unique across all actions.
3. Its look: a line in `looks.look.ron` (`extends: "low_poly"`, `color`), named by `"Look": ("brute")` in the prefab.
4. Place it: a spawn in `arena.arena.ron` with a new, unique `key`. Spawns get ids in order (`e1` the hero, `e2`
   the grunt, then the next): **appending keeps every existing id**; inserting before others renumbers them and
   breaks scenarios and replays that name them. Or spawn it on the fly: `act spawn_prefab id=grunt_brute x=3 z=-4`.
5. Check: `observe` shows it; a scenario fights it; `check-content` catches a misspelt prefab, moveset or look
   with `did you mean …`.

## An attack

An action in `*.action.ron`: `phases` (named stretches; their lengths add up), `hits` (a `span` such as
`Phase("active")`, a `shape` such as `Cone(range:, angle:)`, `damage`, `poise`), `cost`, `cancels` (combos,
dodges out), `iframes`, `effects` (`Sound("whoosh")` at a time), a `telegraph` for enemies (with its sound), `anim`
(the clip). Bind it in the moveset (`inputs: {"heavy": "slam"}`). A `displacement` baked from a clip
(`bake: (animset, clip)`) needs `cargo run -- --bake-motion` after the clip changes. ["Actions and combat (E3)";
fights from the terminal: `docs/agents/combat.md`]

## An NPC enemy with no code

A prefab with `"Npc": (behavior: "raider", perception: "raider", plan: Some("raider"), director: "")` and
`"Home": (pos: None, leash: 10.0)`, plus the files in `assets/content/npc/`: `*.behavior.ron` (states and
transitions with the engine's conditions), `*.perception.ron` (sight, hearing), `*.attack_plan.ron` (options,
weights, cooldowns, bans, rest), and optionally a `*.director.ron` (attack tokens). A point of interest with
`prefab` places it in a region's camp (`region/valley.poi.ron`). It walks the baked navmesh: after changing tiles,
`cargo run -- --bake-nav`. Look inside one while it plays: `act npc_state id=eN`, `act npc_schedule id=eN`,
`act npc_overlay`; calm it in a scenario with `act npc_set_state id=eN state=calm`. A behaviour the engine lacks
is `add_npc_condition` / `add_npc_state` in the game, not a copy of the brain. ["Navigation and NPCs (E6)";
`docs/agents/npc.md`]

## A content kind of the game's own

Copy `template/src/arena.rs`: a struct with `Serialize, Deserialize`, `#[serde(crate = "shards::serde",
deny_unknown_fields)]`, `impl ContentDef` with `KIND` and a `validate` that checks numbers and calls
`refs.require::<PrefabDef>(&id, "field")` for every reference, `app.add_content::<T>()`, and
`require_content::<T>(&["id"], "my_game")` for ids the code relies on. Read it in `OnEnter(EngineState::InGame)`
or inside a tick (`world.resource::<ContentDb>().get::<T>(id)`), never at `Startup`. A kind only the picture
reads: `const SIMULATION: bool = false;`. Test: a good file loads, a typo names the fix
(`a_typo_in_the_level_is_an_error_that_names_the_fix`). ["Content (E4)"; `docs/agents/content.md`]

## A tool, a condition, a rule

Copy `template/src/ai.rs`. A tool is an args struct whose doc comments become its schema, and a system
`In<Call<Args>> -> ToolResult`; `add_ai_tool(name, Scope::Player, f)` changes the world (journaled, replayed)
and must act through `Intent` and the game's rules; `Scope::Dev` is for cheats; `add_ai_query` only reads. Every
error says how to fix the call. A condition is `fn(&World) -> bool` for `step --until named:NAME`. A rule
(`add_ai_invariant`) only reads and names the offender; give it a control test that breaks it on purpose
(`a_grunt_chance_outside_zero_to_one_stops_the_game`). Something agents should see on an entity:
`register_ai_component` or `register_ai_describer`; an event they should hear: `journal_event`.

## A brain of the game's own

Copy `template/src/brain.rs`: a system in `SimSet::Think`; candidates sorted by `AiId`; randomness from
`SimRng` (`rng.stream(id, SALT)` with a constant salt per decision); the result written as an `Order` into
`Intent` (`Order::Attack { target, input, repeat }`); it never moves the body itself. Its numbers live on a
component set by the prefab, so content tunes them.

## Save data

Copy `template/src/progress.rs`. Data that belongs to a character: a component plus
`save_component::<C>(SaveKind::Character, "name", 1)`. Data of the world: a `SaveSection` with `capture` and
`apply` (explicit data keyed by `SaveKey`, never a dump of the world). `autosave_on::<Event>(SaveWhat::All)` saves
at the end of that tick. Changing a section's format: raise its version and add `add_save_migration`. Check with
`act save_game what=all`, `save_check`, `load_game`, and a scenario like `template/scenarios/saves.scenario`.
["Saves (E8.1–5)"]

## HUD and menus

Screens are content: `screens.screen.ron` (widgets, ids, `@` text ids), the theme `*.theme.ron`. A value the game
computes: `add_ui_source` (`template/src/screens.rs`, `trophies`); a button that changes the world calls a tool
(`tool new_game`) or a game action (`add_ui_action`). Screens are outside the simulation. Check without a window:
`act ui_read`, `ui_navigate`, `ui_click`, `ui_set` (`template/scenarios/menus.scenario`). ["UI (E9)"]

## Text and languages

A message in every `assets/locale/<language>/game.ftl`, named `@message-id` from content; from code,
`shards::locale::text(world, "hud-trophies", &Args::new().number("count", n))`: whole sentences with parameters
and plural forms, never pieces glued together. A new language: a line in `languages.locale.ron` and its folder.
Check: `check-content` names a missing `@id`; `act text id=hud-trophies 'args={"count": 3}' language=ru`;
`act locale` lists what each language lacks. ["Localization (E9.4)"]

## Sound, looks, effects, foliage

All content, all presentation (they never change the simulation, so the reference replays do not move):
`audio/*.sound.ron`, `*.music.ron`, `mixer.audio.ron`; `looks.look.ron`; `effects.effect.ron` and the event table
`effects.fx.ron`; `region/*.scatter.ron`. A sound the game must always hear is a `telegraph.sound` or an action's
`Sound` effect, not a clip cue. Check: `act audio` (headless too), and with `live.py start --game template
--offscreen`: `act looks`, `act effects`, `act scatter`, `shot`. [`docs/agents/ui-audio-look-fx-scatter-animation.md`]

## A region, tiles, a level from Blender

`region/*.region.ron`, `*.tile.ron`, `*.socket_rules.ron`, `*.poi.ron`, `*.tile_map.ron`; a tile from Blender is a
glTF in `assets/tiles/` with a `*.level.ron` (required markers). After any of these: `cargo run -- --bake-nav`
(commit `assets/nav/`), `cargo run -- --worldgen-report valley --seeds 1..1000` (exit 0), and if the generator's
output changed, raise the region's `version` and update the manifest hash its scenario expects
(`template/scenarios/valley.scenario`). Visit it: `act regen region=valley seed=42`. ["World and generation (E5)",
"Levels from Blender (E4.2)"; `docs/agents/world-regions-nav.md`]

## Co-op

`session.net.ron` (`max_players`, `player_prefab`) is all the content co-op needs. In code, an observer of a
combat event that changes gameplay starts with `if !shards::core::simulates(role) { return; }`, or the client
counts it twice. Check on a stand: `template/scenarios/stand/coop.scenario` with `invariant !journal "desync"`;
live, `live.py start --game template --name coop -- --stand 1 --net-sim latency=150,loss=0.02`.
[`docs/agents/net-coop-steam.md`]
