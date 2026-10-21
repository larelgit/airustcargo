---
name: shards-game
description: Make a new game on the Shards engine from template/, or change a game built on it (the template in this repository, or a game in its own repository) - content first (prefabs, actions, movesets, NPC behaviour, regions, screens, text, sound, looks, effects), then game code in GamePlugin (content kinds, brains, tools, conditions, invariants, save sections, UI sources) - keeping the engine boundary, the determinism rules, scenarios, reference replays and the game's CI green. Use for gameplay, balance, level or content work on a game; for engine mechanisms use shards-engine.
---

# Making and changing a game

A game is a crate with one dependency, `shards`. It is mostly content (`assets/content/**/*.<kind>.ron`, text
in `assets/locale/`) and a little Rust that adds what is this game's own. The template (`template/`) is the
reference game; read its `README.md` and `src/lib.rs` first: they map every file. Paths are from the
repository root; in `template/` the game's commands run from that folder.

## 1. Where you are

| | The template here | A game in its own repository |
|---|---|---|
| Engine | `path = "../crates/shards"` | a git `tag` or `rev` in `Cargo.toml` |
| Workspace | its own (`template/Cargo.lock`, `template/.config/nextest.toml`) | its own |
| Run | `(cd template && CARGO_TARGET_DIR=../target cargo run -- …)`, sharing the engine's build (section 6) | `cargo run -- …` (`cargo game` is the alias) |
| The CLI | the engine's: `cargo shards --port-file … --token …` from the root (`live.py start --game template` prints the line) | `shards`, installed at the engine's version (`cargo install shards_cli --git … --tag vX.Y.Z`) |
| Docs | `docs/manual.md`, `docs/agents/*.md` here | the same files at the engine's tag |

## 2. The boundary, before anything

- The game depends only on `shards` (`template/tests/game/manifest.rs` fails otherwise): Bevy, serde and
  schemars come through it (`shards::prelude::*`, `#[serde(crate = "shards::serde")]`,
  `#[schemars(crate = "shards::schemars")]`).
- The game never changes engine code. If it needs a mechanism the engine lacks and a second game would want it
  too, that is an engine change (`.agents/skills/shards-engine/SKILL.md`), made and merged first; a mechanism
  that is only this game's lives in the game's plugins.
- Game logic never thinks for the player: no LLM, no model calls. What an agent may do is a tool.

## 3. Content first

Before writing Rust, check whether content can do it; most asks are content. The common ones, with the files
and checks: `.agents/skills/shards-game/references/recipes.md`.

- Content applies while the game runs (hot reload): save the file and the journal says `content reloaded: …`; a
  broken edit names the file, the line and the closest id, and the old version stays in force. Prefabs and the
  arena apply to new entities (`spawn_prefab`) or after a restart.
- Validate without the game: `cargo run -- --check-content` in the game's folder (exit 0 or 1). It also reads level
  files, the baked navmesh and the baked motion, and names the bake command when one is stale.
- Text the player reads is Fluent (`assets/locale/<language>/*.ftl`), named from content as `@id`, in every
  language the game has.

## 4. Then code, in the game's plugins

Add to `GamePlugin` (`template/src/lib.rs`), one module per concern, copying the template's file for it:

| You need | Copy | Registered with |
|---|---|---|
| A content kind of your own (a level, a weapon table) | `template/src/arena.rs` | `add_content::<T>()`, `require_content` |
| An enemy's brain | `template/src/brain.rs` | a system in `SimSet::Think` writing `Intent` orders |
| A tool, a condition, a rule | `template/src/ai.rs` | `add_ai_tool` / `add_ai_query`, `add_ai_condition`, `add_ai_invariant` |
| Save data of your own | `template/src/progress.rs` | `save_component`, `add_save_section`, `autosave_on` |
| A HUD value or a button action | `template/src/screens.rs` | `add_ui_source`, `add_ui_action` |
| Meshes, lights, camera | `template/src/visuals.rs` | only under the `render` feature, when `config.draws()` |

The game keeps the engine's determinism rules (`template/README.md`, "Determinism: rules for the game"):
gameplay in `FixedUpdate` inside a `SimSet` with explicit order; randomness from `SimRng`; iteration in `AiId`
order; characters driven only through `Intent`; `ops::sin` and friends, never `f32` methods (copy the template's
`clippy.toml`); the world spawned in `OnEnter(EngineState::InGame)`; an observer that changes gameplay returns at
once when `!shards::core::simulates(role)`; your own input components through `replay_input`, cosmetics through
`replay_ignore`. `.agents/skills/shards-engine/references/state-and-determinism.md` says what catches each miss.

## 5. Test it like the template does

- One test binary, `tests/game/` (`template/tests/game/main.rs` lists the modules). Add a module per feature,
  driving `my_game::test_app(seed)` with tools (`say`, `fails`, `step`, `journal`).
- A scenario per behaviour in `scenarios/` (co-op in `scenarios/stand/`): it runs in `cargo test` and against a
  live game. Writing one: `.agents/skills/shards-verify/references/scenarios.md`.
- Your rule gets a control test; your tool's errors get a test; a content kind's typo gets a test
  (`template/tests/game/content.rs` has all three).
- Watch it in the real game: `python3 .agents/skills/shards-playtest/scripts/live.py start --game template`.

## 6. The game's gate

```bash
CARGO_TARGET_DIR=$PWD/target cargo test --manifest-path template/Cargo.toml   # tests, scenarios, determinism, replays
CARGO_TARGET_DIR=$PWD/target cargo clippy --manifest-path template/Cargo.toml --all-targets -- -D warnings
(cd template && export CARGO_TARGET_DIR=../target && cargo run -- --check-content \
  && cargo run -- --worldgen-report valley --seeds 1..1000 \
  && for r in tests/replays/*.json; do cargo run -- --replay "$r" --verify || exit 1; done)
```

`CARGO_TARGET_DIR` makes the template share the engine's build folder, as CI does; without it, the template
builds Bevy a second time into a `target/` of its own (another 20 minutes and gigabytes). A game in its own
repository runs the same commands from its folder, without it.

- Reference replays (`tests/replays/smoke.json`, `valley.json`) move when gameplay or content they read changes:
  predict with `python3 .agents/skills/shards-verify/scripts/which_replays.py`; if the change is meant, record
  them again (`(cd template && CARGO_TARGET_DIR=../target cargo test --test game record_reference_replays -- --ignored)`)
  and check that only the expected files changed.
- Bakes: an edited tile or level file needs `cargo run -- --bake-nav`; an edited clip `cargo run -- --bake-motion`
  (in the game's folder); commit what they write.
- In this repository, a change to the template also updates `template/README.md` (its file table, the section,
  the test count), and `template/Cargo.lock` must agree with the root `Cargo.lock` (CI checks it).

## 7. A new game

From a copy of the template, step by step, with the check after each step:
`.agents/skills/shards-game/references/new-game.md`.
