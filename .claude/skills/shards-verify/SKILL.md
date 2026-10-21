---
name: shards-verify
description: Verify a change to the Shards engine, the sandbox or a game the way CI will - plan the checks the diff needs, run them from the fastest to the slowest, pick the right kind of test for new behaviour (unit, crate integration, scenario, replay, snapshot, heavy mode, invariant control test), and re-record reference replays only on purpose. Use before every commit or push, when deciding how to test something, when asked to run the tests, or when a check fails after a change.
---

The instructions of this skill are shared by every coding agent and live in `.agents/skills/shards-verify/`.
Read `.agents/skills/shards-verify/SKILL.md` now and follow it; the files it names under `.agents/skills/` are
part of it.
