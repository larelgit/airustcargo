---
name: shards-new-game
description: Start a new game on the Shards engine from template/ in a repository of its own - copy it with the engine's line-ending rules, rename the package and its settings folder, pin the engine and the shards CLI to one commit, prove the untouched copy passes its tests, content check, world check and replays, add the game's AGENTS.md and CI, then make the content and tests its own and record its replays. Use when asked to start, create, bootstrap or set up a new game on Shards, or to move such a game to a newer engine commit; for changes inside an existing game use shards-game.
---

# A new game from the template

Done means a repository of its own whose only dependency is `shards`, pinned to an engine commit, with the
`shards` CLI installed at the same commit; the copied tests, scenarios and replays passing before the first change
and again once the game is its own; an `AGENTS.md` and a CI workflow. Everything after the setup is game work:
`.agents/skills/shards-game/SKILL.md` and the task skills it lists. The example name is `pit`.

## Before you start

- Read `template/README.md` ("Start your own game, step by step", "Driving the game with `shards`", "The game's
  CI") and the file map at the top of `template/src/lib.rs`. The manual's "Make a game on Shards, step by step"
  (`docs/manual.md`) says why each piece is there.
- Decide the package name: lowercase words joined by `_` (`pit`, `ember_keep`). It names the crate, the binary
  and the player's settings folder (`SettingsAppId::new("ember_keep")` becomes `~/.config/ember-keep/` on Linux):
  two games with one name read and overwrite each other's keys and volume.
- Decide where it lives: outside the engine repository. Cargo reads every `.cargo/config.toml` from the working
  folder up, so a game inside the engine's tree builds with the engine's settings (on Windows its `rust-lld`).
- Decide the engine version: the commit you copy from. A tag works only if it has everything the template uses:
  the template on the main branch may use what `CHANGELOG.md` still lists under Unreleased (`GltfModel`, which
  `template/src/visuals.rs` imports, is not in `v0.4.0`). When in doubt, pin the commit.
- The machine: the toolchain of `template/rust-toolchain.toml` (rustup installs it on the first build), the
  system packages of `AGENTS.md` ("Building"), and room for a Bevy build of the game's own (many minutes, several
  gigabytes).

## Steps

1. **Copy the template without its build output and run files, and take the engine's line-ending rules along.**
   ```bash
   git -C <engine> rev-parse HEAD        # the commit you copy from: step 3 pins it
   mkdir -p ~/games && cp -r <engine>/template ~/games/pit && cp <engine>/.gitattributes ~/games/pit/
   cd ~/games/pit && rm -rf target dist .shards replays snapshots saves shots crashes && git init
   ```
   `.gitattributes` keeps `*.ron`, `*.json`, `*.gltf`, `*.scenario` and `*.ftl` at LF on every OS. Check: `ls -a`
   shows, besides `.git`, `.cargo .config .gitattributes .gitignore Cargo.lock Cargo.toml README.md assets
   clippy.toml rust-toolchain.toml scenarios src tests`.

2. **Rename the package everywhere.**
   ```bash
   grep -rIl my_game . | grep -v Cargo.lock | xargs sed -i 's/my_game/pit/g'    # macOS: sed -i ''
   ```
   That reaches `Cargo.toml` (`name`, `[[bin]]`), `src/main.rs` (`pit::build_app`), `src/lib.rs`
   (`SettingsAppId::new`, `InputSettings::for_game`), `src/arena.rs` (`require_content`), the tests and the README.
   Then by hand: set the window title in `src/lib.rs` (`title: "My game"`) and the main menu's title in every
   language (`menu-title` in each `assets/locale/*/game.ftl`); delete the `InputSettings::for_game(...)` line in
   `src/lib.rs` and the comment above it (it imports key bindings from a folder older templates wrote, which a new
   game never had). Check: `grep -rn my_game . --exclude-dir=target` finds only `Cargo.lock`, which the next build rewrites;
   `grep -rn 'my-game\|My game' . --exclude-dir=target` lists the other spellings left in comments and the README.
   The rename moves the reference replays: a replay check hashes each component under its type path, and
   `my_game::progress::Trophies` is now `pit::progress::Trophies`. Step 5 records them again.

3. **Pin the engine** in `Cargo.toml`, replacing `shards = { path = "../crates/shards", default-features = false }`:
   ```toml
   shards = { git = "https://github.com/larelgit/shards-engine", rev = "<that commit>", default-features = false }
   ```
   then `cargo update`. Keep `default-features = false` (the game's `[features]` choose the engine's) and the
   `[workspace]` table (the game is a workspace of its own). Check: `cargo tree -p shards --depth 0` shows the git
   source at that commit.

4. **Install the CLI at the same commit**:
   `cargo install shards_cli --git https://github.com/larelgit/shards-engine --rev <that commit>`. The `ai/*`
   protocol may change between engine versions, so the CLI and the game always share one; reinstall it whenever
   the pin moves.

5. **Prove the copy works**, before changing anything. Before the record, `cargo test` fails in
   `replays::reference_replays_still_match` alone, with `DIVERGED at tick 60: e1 Hero: Trophies differs; e2 Grunt:
   Grunt differs` and the values unchanged: that is the rename (step 2). Record, then run everything:
   ```bash
   cargo test --test game record_reference_replays -- --ignored   # the first Bevy build takes many minutes
   cargo test
   cargo run -- --check-content
   cargo run -- --worldgen-report valley --seeds 1..1000
   for r in tests/replays/*.json; do cargo run -- --replay "$r" --verify || break; done
   ```
   All pass, and each `--verify` says `identical to the recording`. Any other failure, or a divergence that names an
   engine component, is the copy or the pin, not the game (see Pitfalls).

6. **Give the repository its agent instructions**: an `AGENTS.md`, and a `CLAUDE.md` that holds `@AGENTS.md`.
   Say that the game has no LLM and is driven from outside by `shards`; the determinism rules (the README's
   "Determinism: rules for the game"); how to start and drive it ("See it in the game" below); the gate (step 9);
   content before code, player-read text in Fluent. Point to the engine's `docs/manual.md` at the pinned commit
   for the rest.

7. **Make it yours, content first** (`.agents/skills/shards-game/SKILL.md`; for one task at a time,
   `shards-add-enemy`, `shards-add-move`, `shards-level`, `shards-ui`, `shards-save-data`). The template's tests and
   scenarios name its things: `e1` the hero, `e2` the grunt, `e7` the co-op guest, the arena `yard`, the region
   `valley`, the component `Trophies`, the tool `taunt`. Rework `tests/game/` and `scenarios/` along with the
   content, keeping each kind of test (determinism, replays, content, manifest, co-op). The recorded scenarios are
   listed in `tests/game/replays.rs` (`for name in ["smoke", "valley"]` in `record_reference_replays`), and two
   tests in `tests/game/scenarios.rs` name `smoke.scenario`. Rewrite `README.md` for the game: it still describes
   the template.

8. **Record the reference replays** once the game plays as it should:
   `cargo test --test game record_reference_replays -- --ignored`, then `cargo test`. Check:
   `cargo run -- --replay tests/replays/smoke.json --verify` says `identical to the recording`.

9. **CI**: `.github/workflows/ci.yml` from the template README's "The game's CI": `cargo test --locked`,
   `--check-content`, `--worldgen-report <region> --seeds 1..1000` for each region the game has, and `--verify`
   of every file in `tests/replays/`. Add `cargo clippy --all-targets -- -D warnings`: the copied `clippy.toml` is
   what refuses platform maths (`f32::sin` and friends), and it works only when clippy runs. Commit `Cargo.lock`.

10. **Moving to a newer engine later**: change `rev`, `cargo update -p shards`, reinstall the CLI at the new
    `rev`, and read the engine's `CHANGELOG.md` since the old commit (the "Changed" entries first). What the
    template gained meanwhile, a game copied from it may want: `git -C <engine> diff <old>..<new> -- template/`.
    Run the tests and `--verify` the replays; a replay that diverges after an engine gameplay change is recorded
    again once you know which change moved it.

## See it in the game

From the game's folder (the token is in `.shards/`, and `shards` looks for it from the folder it runs in):

```bash
cargo run -- --headless --paused --port 0 --port-file /tmp/pit.port > /tmp/pit.log 2>&1 &
shards --port-file /tmp/pit.port wait --timeout 180    # prints the pid, seed and port once loaded
shards --port-file /tmp/pit.port observe               # the hero e1 and the grunt e2 in the arena
shards --port-file /tmp/pit.port scenario scenarios/smoke.scenario
shards --port-file /tmp/pit.port quit                  # never kill -9: it leaves the token behind
```

`cargo game` is the same as `cargo run --` (`.cargo/config.toml`). Inside the engine repository the template
itself starts with `python3 .agents/skills/shards-playtest/scripts/live.py start --game template`.

## Test it

- The copy's own suite is the baseline: run it before the first change (step 5) and after each piece of the
  game you replace, so a red test points at the last change.
- `tests/game/manifest.rs` keeps the game to the one dependency and the settings folder named after the package
  (`the_settings_folder_is_named_after_the_package_the_way_a_rename_finds_it`); keep it.
- A new feature gets a module in `tests/game/` (listed in `tests/game/main.rs`) and a scenario in `scenarios/`:
  `.agents/skills/shards-verify/references/test-kinds.md` and `.agents/skills/shards-verify/references/scenarios.md`.

## Done when

- [ ] `grep -rn my_game . --exclude-dir=target` finds nothing; the window and the main menu carry the game's
      title in every language.
- [ ] `shards` is pinned by `rev` (or a tag that has everything the game uses), `default-features = false`;
      `Cargo.lock` is committed; the CLI is installed at the same commit.
- [ ] `.gitattributes`, `.gitignore` and `clippy.toml` came along.
- [ ] The replays were recorded again after the rename, and `cargo test`, `cargo clippy --all-targets -- -D warnings`,
      `--check-content`, `--worldgen-report` for each region and `--verify` of each replay pass.
- [ ] `AGENTS.md`, `CLAUDE.md` and `.github/workflows/ci.yml` are in the repository.
- [ ] Once the content is the game's own: the tests and scenarios name its things, the replays are recorded again
      from them, and `README.md` describes the game.

## Pitfalls

- **A tag older than the copy.** `cargo build` fails on a name the template uses (`unresolved import`, as for
  `GltfModel` on `v0.4.0`). Pin the commit you copied from.
- **No `.gitattributes`.** A Windows checkout turns content, levels and replays to CRLF; their bytes are hashed,
  so replays diverge there (`replay check level differs`) while they pass on Linux.
- **A renamed type moves the replays.** The game's components are hashed under their type path, so renaming the
  crate, a module or a component makes the replays diverge with the same values; record them again in the same
  commit and say why.
- **The CLI from another engine commit.** Install it again at the game's `rev` after every pin move.
- **Running the binary directly** (`target/debug/pit`): it looks for `assets/` next to the executable; set
  `BEVY_ASSET_ROOT=$PWD`, or use `cargo run`.
- **Running `shards` from another folder**: it finds no token. Run it from the game's folder or below it.
- **Recording the replays before the content is final** only means recording them again; recording them to make
  a red `--verify` green, without knowing what moved them, hides a determinism bug
  (`.agents/skills/shards-debug/SKILL.md`, "A replay diverged").
