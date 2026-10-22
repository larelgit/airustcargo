# Agent skills

Procedures for the coding agents that work on this repository. The rules stay in `AGENTS.md` and
`docs/agents/*.md`; a skill says in which order to apply them and which commands prove the work, so an agent
does the job the way the merged history does it. Use the most precise skill that fits the task: a task skill
names the files, fields, tools and tests of one kind of job and links the broad skills for the rest.

## Broad skills

| Skill | Use it to |
|---|---|
| `shards-engine` | change or extend the engine (`crates/`): design for determinism and snapshots, test in the crate and the sandbox, update the manual, `engine-progress` and the CHANGELOG |
| `shards-game` | change a game made from `template/`: content first, then the game's plugins, its tests and replays |
| `shards-verify` | plan and run the checks a diff needs, choose the kind of test, re-record reference replays on purpose |
| `shards-playtest` | start a headless game and drive it with the `shards` CLI; turn a session into a scenario |
| `shards-debug` | find the root cause of a panic, a broken invariant, a divergence, a desync, a flaky test |

## Game tasks

| Skill | Use it to |
|---|---|
| `shards-new-game` | start a game in a repository of its own from `template/`, pinned with its CLI to one engine commit |

## Before a push

| Skill | Use it to |
|---|---|
| `shards-review` | review a diff or a pull request against the repository's rules |

## Layout

- Each skill is `.agents/skills/<name>/SKILL.md` in the [Agent Skills](https://agentskills.io) format (YAML front
  matter with `name` and `description`, then Markdown), with `references/` (read when the skill says so) and
  `scripts/` (run, not read) beside it. Codex finds skills here.
- Claude Code finds skills in `.claude/skills/`. Each skill has an entry there with the same `name` and
  `description` whose body sends the agent to the file here, as `CLAUDE.md` sends it to `AGENTS.md`. Plain
  files, not symbolic links: a Windows checkout without symlink support would turn a link into a text file.
- Paths inside the skills are from the repository root. The scripts need Python 3.11 or newer (`py -3` on
  Windows) and nothing else.

## Changing a skill

- Edit it here. If you change its `name` or `description`, change its entry in `.claude/skills/` to match.
- Keep a `SKILL.md` short and procedural (a task skill under 250 lines, any skill under 500); detail goes to
  `references/`, and rules that every change follows go to `docs/agents/`, not into a skill.
- A task skill does not repeat a broad one: it links it and adds what is particular to the task. When a rule, a
  file or a command it names changes, change the skill in the same commit.
- `cargo test -p sandbox --test agents_index` checks that every skill has a valid name and description, that
  its Claude Code entry matches it, that this index has a row for it, and that every repository path and script
  it mentions exists. Run it after renaming a file a skill names.
- A new skill: a folder here, its entry in `.claude/skills/`, and a row in this index.
