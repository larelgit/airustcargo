# A new game from the template, step by step

Each step ends with the check that proves it. The same steps, with background, are in `template/README.md`
("Start your own game, step by step") and `docs/manual.md` ("Make a game on Shards, step by step"). The
example name is `pit`; the new game lives outside the engine repository.

1. **Copy the template, without build output.**
   ```bash
   mkdir -p ~/games && cp -r template ~/games/pit && cd ~/games/pit
   rm -rf target dist .shards replays snapshots saves shots crashes && git init
   ```
   Check: `ls` shows `Cargo.toml src assets scenarios tests README.md clippy.toml rust-toolchain.toml`.

2. **Rename the package everywhere** (the crate, the binary, `SettingsAppId`, the tests, the docs):
   ```bash
   grep -rIl my_game . | grep -v Cargo.lock | xargs sed -i 's/my_game/pit/g'
   ```
   Also set the window title in `src/lib.rs` (`title: "My game"`). `SettingsAppId::new("pit")` names the
   player's settings folder: two games with one name would overwrite each other's settings. Check:
   `grep -rn my_game .` finds nothing outside `Cargo.lock`.

3. **Pin the engine** instead of the path, at the tag the game is written for (or a `rev`):
   ```toml
   shards = { git = "https://github.com/larelgit/shards-engine", tag = "v0.4.0", default-features = false }
   ```
   then `cargo update`. Keep `default-features = false` (the game's `[features]` choose the engine's) and the
   `[workspace]` table. Check: `cargo tree -p shards --depth 0` shows the git source.

4. **Install the CLI at the same version**: `cargo install shards_cli --git
   https://github.com/larelgit/shards-engine --tag v0.4.0`. The `ai/*` protocol may change between minor
   versions, so the CLI and the game always share one engine version.

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
   (`.agents/skills/shards-game/references/recipes.md`); then the code in `src/` that is the game's own.
   Rename or remove the template's scenarios with what they test, and update the list of recorded scenarios in
   `tests/game/replays.rs` (`record_reference_replays`) and the test that names `smoke.scenario`.

8. **Record the reference replays** once the game plays as it should:
   `cargo test --test game record_reference_replays -- --ignored`, then `cargo test`. Check:
   `cargo run -- --replay tests/replays/smoke.json --verify` says `identical to the recording`.

9. **CI.** `.github/workflows/ci.yml` from `template/README.md`, "The game's CI": `cargo test --locked`,
   `--check-content`, `--worldgen-report <region> --seeds 1..1000` for each region, and `--verify` of every file in
   `tests/replays/`. Keep `clippy.toml`: it is what forbids platform maths.

10. **Moving to a new engine version later:** change `tag`, `cargo update -p shards`, read the "Changed" section of
    the engine's `CHANGELOG.md`, reinstall the CLI at the new tag, run the tests and `--verify` the replays.
    Replays may diverge after the engine's gameplay changes: record them again once you have checked why.
