# A new game from the template, step by step

Each step ends with the check that proves it. The same steps, with background, are in `template/README.md`
("Start your own game, step by step") and `docs/manual.md` ("Make a game on Shards, step by step"). The
example name is `pit`; the new game lives outside the engine repository.

1. **Copy the template, without build output, and the engine's line-ending rules.**
   ```bash
   git rev-parse HEAD                     # in the engine repository: the commit you copy from (step 3)
   mkdir -p ~/games && cp -r template ~/games/pit && cp .gitattributes ~/games/pit/ && cd ~/games/pit
   rm -rf target dist .shards replays snapshots saves shots crashes && git init
   ```
   `.gitattributes` keeps `*.ron`, `*.json`, `*.gltf`, `*.scenario` and `*.ftl` at LF on every OS: their bytes are
   hashed, and a Windows checkout with CRLF makes replays diverge. Check: `ls -a` shows `.gitattributes Cargo.toml
   src assets scenarios tests README.md clippy.toml rust-toolchain.toml`.

2. **Rename the package everywhere** (the crate, the binary, `SettingsAppId`, the tests, the docs):
   ```bash
   grep -rIl my_game . | grep -v Cargo.lock | xargs sed -i 's/my_game/pit/g'
   ```
   Also set the window title in `src/lib.rs` (`title: "My game"`). `SettingsAppId::new("pit")` names the
   player's settings folder: two games with one name would overwrite each other's settings. Check:
   `grep -rn my_game .` finds nothing outside `Cargo.lock`.

3. **Pin the engine** instead of the path, at the commit you copied the template from:
   ```toml
   shards = { git = "https://github.com/larelgit/shards-engine", rev = "<that commit>", default-features = false }
   ```
   then `cargo update`. The template on the main branch can use what no tag has yet (its `src/visuals.rs` uses
   `GltfModel`, which `CHANGELOG.md` lists under Unreleased), so a tag older than the copy does not build; a tag
   works once one includes everything the template uses. Keep `default-features = false` (the game's `[features]`
   choose the engine's) and the `[workspace]` table. Check: `cargo tree -p shards --depth 0` shows the git source.

4. **Install the CLI at the same version**: `cargo install shards_cli --git
   https://github.com/larelgit/shards-engine --rev <that commit>` (`--tag` for a tag). The `ai/*` protocol may
   change between minor versions, so the CLI and the game always share one engine version.

5. **Build and test.** `cargo test` (the first Bevy build takes many minutes), then
   `cargo run -- --check-content`, then a live game:
   ```bash
   cargo run -- --headless --paused --port 0 --port-file /tmp/pit.port > /tmp/pit.log 2>&1 &
   shards --port-file /tmp/pit.port wait --timeout 180 && shards --port-file /tmp/pit.port scenario scenarios/smoke.scenario
   shards --port-file /tmp/pit.port quit
   ```
   Check: all pass on the unchanged template before you change anything.

6. **Give the repository its agent instructions.** An `AGENTS.md` (and a `CLAUDE.md` holding `@AGENTS.md`) that
   says: the game has no LLM and is driven from outside; the determinism rules (`README.md`, "Determinism: rules
   for the game"); how to start and drive it (step 5); the gate (step 9); content before code.

7. **Make it yours, content first.** Replace the yard, the hero, the grunt and the valley in `assets/content/`
   (`.agents/skills/shards-game/references/recipes.md`); then the code in `src/` that is the game's own. The
   template's tests and scenarios name its things (`e1` the hero, `e2` the grunt, `e7` the guest, `yard`, `valley`,
   `Trophies`, `taunt`): rework `tests/game/` and `scenarios/` with them, keeping the kinds of test (determinism,
   replays, content, manifest, co-op). The list of recorded scenarios is in `tests/game/replays.rs`
   (`record_reference_replays`), and two tests in `tests/game/scenarios.rs` name `smoke.scenario`.

8. **Record the reference replays** once the game plays as it should:
   `cargo test --test game record_reference_replays -- --ignored`, then `cargo test`. Check:
   `cargo run -- --replay tests/replays/smoke.json --verify` says `identical to the recording`.

9. **CI.** `.github/workflows/ci.yml` from `template/README.md`, "The game's CI": `cargo test --locked`,
   `--check-content`, `--worldgen-report <region> --seeds 1..1000` for each region, and `--verify` of every file in
   `tests/replays/`. Keep `clippy.toml`: it is what forbids platform maths.

10. **Moving to a new engine version later:** change `rev` (or `tag`), `cargo update -p shards`, read the engine's
    `CHANGELOG.md` since your version (the "Changed" sections), reinstall the CLI at the same `rev`, run the tests and
    `--verify` the replays.
    Replays may diverge after the engine's gameplay changes: record them again once you have checked why.
