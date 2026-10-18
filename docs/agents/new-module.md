# A new engine module

Read this when you add a crate or a module to the engine. Part of the [AGENTS.md](../../AGENTS.md) index.

- **A new engine module** is a crate with a plugin that names itself `add_engine_module(name, version)` (so it is visible in
  `ai/status`), with system sets for ordering and one line in `EnginePlugins`. It describes its state for the AI
  (describe, `journal_event`), and gives its actions as tools (what the developer needs at hand in the window is also an item in the
  developer menu, `add_dev_entry`, which calls the same tool); it takes its numbers from content; it has unit tests, its invariants (each with a control test that
  breaks the state on purpose), a scenario in the sandbox, a replay and a section of the manual (`docs/manual.md`).
