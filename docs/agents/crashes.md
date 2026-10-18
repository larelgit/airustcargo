# If the game crashed

Read this when `step` reports a panic or a broken invariant, or when you work with crash reports. Part of the [AGENTS.md](../../AGENTS.md) index.

A panic in a gameplay system does not kill the game. `step` stops on the tick where the failure happened and replies
`stopped: gameplay system <name> panicked at tick N: <msg>`. In real time, the game pauses.
The journal shows `system <name> panicked at tick N`. The session is already saved to `replays/crash-tick<N>.json`:

```bash
cargo sandbox --replay replays/crash-tick<N>.json --headless --paused &   # the same world from tick 0
cargo shards wait && cargo shards step <N>     # the failure repeats on tick N: look at inspect/journal
cargo sandbox --replay replays/crash-tick<N>.json --verify   # exit code 1 while the system still panics
```

After the fix, `--verify` no longer writes `gameplay system … panicked`. Exit code 1 with `DIVERGED` then
only means that the fixed system changes the world differently from the one that crashed.

A violated invariant stops the game in the same way: `stopped: invariant <name> violated at tick N`. The journal
gives the text of the violation. The replay is `replays/invariant-<name>-tick<N>.json`. The `assert_*` rules are
recorded in it, so replaying it breaks the rule on the same tick. `--verify` lists the violations and exits with
code 1 until the bug is fixed.

Along with the crash replay, a crash report is written to `crashes/crash-<time>-tick<N>.json` (`[sim] crash report saved: …`). If the working folder cannot be written to, the report goes into the player's folder, for example `%LOCALAPPDATA%\<program>\crashes`, and the same line names the path. The report holds the location and stack of the panic, the game and the machine (OS, processor, memory, graphics card), `ai/status`, the last lines of the journal and the log, and the same replay.
It is written when a panic happens on another thread (the render thread, a task), when `shards::run` handles a panic that takes the game down (exit code 101, `error: the game crashed: …; crash report: …`; such a report has no replay), and on request, through the `crash_report note="…"` tool. A crash report with a replay that a player sends plays back as a replay: `--replay crashes/crash-….json` (for a report without a replay, it says `… is a crash report without a replay (<why>)`). If you catch the panic yourself and carry on, call `ai_core::crash::handled(&message)`. Otherwise, a report is written about it (`docs/manual.md`, "Crash reports (E10.7)").
