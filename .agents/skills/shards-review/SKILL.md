---
name: shards-review
description: Review a change to the Shards engine or a game on it, your own before pushing or a pull request, against this repository's rules - the no-LLM rule and the boundary, determinism and simulation state, content and text, tools and invariants, co-op, tests, reference replays and docs. A script flags the added lines that break a rule, the verify scripts name the checks and docs the diff owes, and a checklist covers what no script sees; each finding is reported with file, line, rule and a failing case. Use when asked to review, check or audit a change, a branch, a diff or a PR, or before a push.
---

# Reviewing a change

Done means every changed line was read against the rules, the scripts' findings are each fixed or answered, the
checks the diff needs were run or named as not run, and every finding reported has a file and line, the rule it
breaks and a concrete case where it goes wrong. The rules are in `AGENTS.md` and `docs/agents/*.md`; which checks a
diff needs is `.agents/skills/shards-verify/SKILL.md`. This skill is the order to apply them in when you read a diff.

## Before you start

- Get the diff and its claims. Your own branch: `git status` and `git diff $(git merge-base HEAD origin/main)`. A pull
  request: fetch its branch and check out its head (the scripts below read the working tree, `red_flags.py` also takes
  `--head`); read its description, its commit messages and its `CHANGELOG.md` entry, which say what it claims to do.
- Read the `AGENTS.md` index topics for the areas the diff touches (a tool: `docs/agents/tools-and-invariants.md`;
  simulation state: `docs/agents/determinism.md`; …). The checklist below points to them.
- Decide the depth: a content or docs change needs the content and docs items; any change to `crates/` or `template/src/`
  needs the whole checklist.

## Steps

1. **Run the scripts** (bash; Python 3.11 or newer):
   ```bash
   python3 .agents/skills/shards-review/scripts/red_flags.py                    # lines that break a rule, or may
   python3 .agents/skills/shards-verify/scripts/plan.py                         # the checks it needs, the docs it owes
   python3 .agents/skills/shards-verify/scripts/which_replays.py                # the reference replays its content moves
   ```
   Each compares with the merge base of `origin/main` by default; `--base <commit>` changes that (`--base HEAD`:
   only what is not committed), and `red_flags.py --base A --head B` reviews the commits `A..B` without a checkout.
   A `rule` finding is a bug unless the line proves otherwise; a `check` finding needs a look and a decision. `red_flags.py`
   exits 1 while a `rule` finding is left. Every `[todo]` in the plan's "Docs owed" is a finding unless the change
   really owes nothing there (a refactor owes no manual section).

2. **Read the diff file by file** against `.agents/skills/shards-review/references/checklist.md`. Order: the rules
   that break replays and co-op first (determinism, simulation state, observers), then the agent surface, content and
   text, tests, docs. For each changed function ask what calls it, on which machine (host, client, headless, with a
   window), at which tick, in what order, and what a snapshot taken in the middle would miss.

3. **Check the claims.** A change that says "no gameplay change" leaves the reference replays alone (`which_replays.py`
   names none, `git diff --stat -- tests/replays template/tests/replays` is empty); one that re-records them says which
   change moved each. A new tool, condition, rule or content kind has the test the checklist names. A doc sentence the
   diff adds is true of the code. When a claim matters and the diff alone cannot prove it, run the check:
   the fast loop of `.agents/skills/shards-verify/SKILL.md`, or watch it in a live game
   (`.agents/skills/shards-playtest/SKILL.md`).

4. **Verify each finding before you report it.** Trace a real path from a caller or an input to the failure (a scenario,
   a tool call, a co-op join, a snapshot load), or reproduce it in a test or a live game. Drop what you cannot support;
   say "unverified" for what you could not run.

5. **Report.** Most severe first; for each: `file:line`, what is wrong, the rule (the doc and its section), the failing
   case (input or sequence → wrong result), and the fix. Severity:
   - *blocker*: breaks a rule in `AGENTS.md`, determinism, a replay, co-op, a test or CI;
   - *should fix*: a missing test or doc the change owes, an error message that does not teach, a number that belongs in
     content;
   - *nit*: wording and naming the repository's style asks for. Leave out preferences the repository does not state.

   For your own branch, fix the blockers and should-fixes, then run `red_flags.py` and the checks again.

## See it in the game

A finding about behaviour is worth a live check: start the game the change touches
(`python3 .agents/skills/shards-playtest/scripts/live.py start`, `--game template` for the template), drive the path
you traced and quote what the game answered. A co-op finding needs a stand
(`live.py start --name stand -- --stand 2 --net-sim latency=150,loss=0.02`) and the client's view (`--peer c1 observe`).
A determinism finding is proven by a replay that diverges: `save-replay`, then `--replay FILE --verify`, and with a
window (`--verify --windowed`, under `xvfb-run -a` without a display) when `Entity` order is the suspect.

## Test it

Reviewing adds no test of its own; it asks for the ones the change owes. When you fix a finding on your own branch, the
fix comes with the test that would have caught it: the kinds are in
`.agents/skills/shards-verify/references/test-kinds.md`.

## Done when

- [ ] `red_flags.py` was run: no `rule` finding is left, and each `check` finding was looked at and decided.
- [ ] `plan.py` was run: each `[todo]` doc is written or answered, and its gate commands were run or named as not run.
- [ ] Every changed file was read against the checklist, including tests, content, scenarios and docs.
- [ ] Replay changes are explained (which change moved each) or absent when the change claims no gameplay change.
- [ ] Each reported finding has `file:line`, the rule, a failing case and a fix, and was verified or marked unverified.

## Pitfalls

- **Green CI is not a clean diff.** `HashMap` or `Entity` order, a `Local`, an observer without `simulates`, a missing
  `replay_input` pass clippy and every single-player test; they show in a replay with a window, in heavy mode, after a
  snapshot load, or on a stand, and only where a test reaches them.
- **A re-recorded replay can hide the bug.** "Recorded the replays again" without saying what moved them is a finding.
- **Scripts see patterns, not meaning.** `red_flags.py` reads added lines only: a rule broken by a deleted line (a removed
  `replay_ignore`, a dropped `simulates` guard) or by a file it skips shows only when you read the diff.
- **Presentation crates are skipped** by the simulation checks of `red_flags.py` (`shards_ui`, `shards_audio`, `shards_look`,
  `shards_fx` and the others it lists, `render/`, `camera/`): code there that changes gameplay is exactly the bug to look for.
- **Tests change with the code.** An expectation edited in the same diff as the behaviour may have been loosened to
  pass: compare the old and new expectation and ask whether the new one still proves the point.
