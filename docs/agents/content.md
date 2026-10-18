# Content

Read this when you add or change numbers, definitions, a content kind or player-facing text. Part of the [AGENTS.md](../../AGENTS.md) index.

- **Content.** Numbers and definitions are files `assets/content/**/*.<kind>.ron` (hot reload), not constants in code.
  The text the player reads lives in Fluent messages in `assets/locale/<language>/*.ftl`: content names them `@id`, and the code
  formats them with `shards::locale::text(world, id, &Args)` as whole sentences with parameters, not by joining pieces.
  The sandbox does the same: its world, prefabs, attacks, movesets and movement are in `crates/sandbox/assets/content/`. A new content kind is
  a struct with `ContentDef` and `add_content`; references to other definitions are checked in `validate` (`refs.require`).
  The replay check `content` covers only the definitions the simulation read (a fixed tick, an action call, the world being built):
  read one definition by id when that is all the code needs (`iter`/`ids` make the whole kind count); code inside the simulation's
  scope that is not the simulation (a describer, a picture) reads with `peek`/`peek_iter`; a kind only the picture, the sound or
  services outside the session read sets `const SIMULATION: bool = false;`. A builder called outside a tick or an action opens the
  scope (`ai_core::scope::open`), and what only watches inside one suspends it (`scope::suspend`). The test
  `content_reads_are_the_same_headless_and_drawn` (GPU, ignored) checks that drawing reads nothing that counts. A read with the
  scope closed is never counted: content the simulation depends on is read in a fixed tick, an action or a builder, not in
  `Startup`, a loading state or a cache built at load (a change to it would then show only later, in the entities' hashes, unnamed).
  Details: `docs/manual.md`, "What the content check covers".
