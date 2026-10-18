# Content

Read this when you add or change numbers, definitions, a content kind or player-facing text. Part of the [AGENTS.md](../../AGENTS.md) index.

- **Content.** Numbers and definitions are files `assets/content/**/*.<kind>.ron` (hot reload), not constants in code.
  The text the player reads lives in Fluent messages in `assets/locale/<language>/*.ftl`: content names them `@id`, and the code
  formats them with `shards::locale::text(world, id, &Args)` as whole sentences with parameters, not by joining pieces.
  The sandbox does the same: its world, prefabs, attacks, movesets and movement are in `crates/sandbox/assets/content/`. A new content kind is
  a struct with `ContentDef` and `add_content`; references to other definitions are checked in `validate` (`refs.require`).
