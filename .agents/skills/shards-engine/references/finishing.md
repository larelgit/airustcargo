# Saying what is now true: docs, changelog, commit

Documentation in this repository states what the code does today, in plain English, with the names an agent
will type (types, tools, fields, journal lines in backticks). It is updated in the same change as the code. Look
at how the last change of the same kind did it (`git show <commit> -- docs CHANGELOG.md`) and match it.

## `docs/manual.md`

The module's section (find it with `grep -n '^## \|^### ' docs/manual.md`). Add or correct, in the section's own
order: what the feature does and when; the content fields with their defaults and limits; the tools and their
replies; journal lines; conditions; what it does not do yet. Prefer real output (copied from a run) to
paraphrase. A behaviour that changed is rewritten where it is described, not appended as a note. A tool is
also listed in the AI layer's table of tools by scope (`grep -n '^### Tools' docs/manual.md`), a new sandbox
scenario in "The sandbox", and both in the crate's own docs that list them (the crate doc in `src/lib.rs`, the
header of `src/tools.rs`).

## `docs/engine-progress.md`

Statuses are set from the code and the tests, never from intent: ✅ done (code plus a test, scenario or command
that checks it), 🟡 partly (say what is missing), ⏳ ahead.

- The row of the module's item, or of the work item (`N8`, `R1`, `G10 / C3.2`, …) in the tables near the end:
  what is built, where (files, functions), the tests that prove it, whether the reference replays changed.
- The crate's row in "Tests": the new count, with what the change added (`N8d +5 in live.rs`). Count before and
  after with `cargo test -p <crate> -- --list 2>/dev/null | grep -c ': test$'`, less the ignored ones the table
  names apart (`-- --list --ignored`) and doctests, with the features the row counts (some give the default and
  the `render` count) and on your OS (some rows give Linux and Windows apart); change only the numbers you measured.

## `CHANGELOG.md`

One entry under `## [Unreleased]`, in `### Added`, `### Fixed` or `### Changed` (a breaking change to the API or
to content formats goes in Changed and says what a game must do), as the first bullet of its list: the newest
come first. The house style, one paragraph:

```text
- <What it is> (<work item>; the manual's "<section>"): <the API and content fields, in backticks, with their
  defaults>. <How it behaves, including co-op and replays>. Tests: <where> (<count>). <Replays: `x.json`
  re-recorded, the others byte-identical / no reference replay changes>.
```

## `docs/agents/<topic>.md` and `AGENTS.md`

A new tool, query or `named:` condition joins the catalogue in `docs/agents/watching-and-driving.md` (the engine
tools by area, and the conditions for `step --until named:…`), with the one-line description an agent needs to pick
it; combat tools are explained in `docs/agents/combat.md`, engine rules in `docs/agents/tools-and-invariants.md`
where their module has an entry.

Add a bullet to the topic file when your change creates a rule that the next agent must follow to avoid a bug
(an invariant to keep, a function every caller must go through, a field that must stay out of the hash), in the
form of the existing bullets: a bold name, the rule, why. `AGENTS.md` changes only for a rule every change
follows or a new topic file (one index row); it must stay under 9 KiB, and
`cargo test -p sandbox --test agents_index` checks the index and the size.

## The rest

- `README.md`: only when what the project page says changes (features, quick start, the steps).
- `template/README.md`: when the template changes (its file table, the section of the feature, its test count).
- `.agents/skills/`: when a procedure here becomes wrong (a command, a path, a check), fix the skill in the same
  change; `cargo test -p sandbox --test agents_index` checks their paths.

## The commit

- One logical change per commit; the branch you were given; never push to `main`.
- Subject: a sentence about the behaviour, led by the work item when there is one:
  `N8d: live tags on attack options refuse them while a zone lies at the target`. A fix answering review says
  so: `K3 review: a misspelt field of a shot point is an error (CodeRabbit)`.
- Body: why, what changed in behaviour, how it is tested, and whether replays were re-recorded. Wrap it near 100
  columns.
- Code, comments and docs never name the agent or model that wrote them; commit trailers follow your
  environment's attribution rules.
- Before you push, the gate of `.agents/skills/shards-verify/SKILL.md` (section 3) has passed, and your report
  lists what ran.
