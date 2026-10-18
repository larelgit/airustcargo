# Third-party crates

Read this before you add a dependency. Part of the [AGENTS.md](../../AGENTS.md) index.

- **A third-party crate** is added only after checking that it supports Bevy 0.20 and is still maintained; the version is pinned exactly (`=x.y.z` in
  `[workspace.dependencies]`), and the crate itself is hidden behind the engine's API: its types are not in the public API, and the game does not import it.
  That way, physics (now `parry3d`) can change and Bevy can update, without changes to the game.
