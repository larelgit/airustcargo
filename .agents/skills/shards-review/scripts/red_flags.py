#!/usr/bin/env python3
"""The added lines of a diff that break a rule of this repository, or may: what a reviewer looks at first.

    python3 .agents/skills/shards-review/scripts/red_flags.py                     # the branch against origin/main, edits and new files included
    python3 .agents/skills/shards-review/scripts/red_flags.py --base HEAD         # only what is not committed yet
    python3 .agents/skills/shards-review/scripts/red_flags.py --base A --head B   # the commits A..B (a pull request: its base and head)

Two levels. `rule`: almost always a mistake here (a forbidden build flag, Bevy moved, randomness outside
`SimRng`, an LLM dependency, a model named in the code). `check`: right in some places and a bug in others
(a `HashMap`, real time, a `Local`, an observer): look at the line and decide. It reads the added lines only,
with their file for context, and skips tests for the checks that concern shipped code. It sees patterns, not
meaning: clippy, the tests and the checklist in .agents/skills/shards-review/references/checklist.md cover the
rest. Exit status 1 when a `rule` finding is left, else 0.
"""

import argparse
import re
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the repository
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "shards-verify" / "scripts"))
from which_replays import definitions  # noqa: E402  (the RON reader of shards-verify)

DETERMINISM = "docs/agents/determinism.md"


def git(root, *args, check=True):
    # UTF-8 whatever the platform's code page: the files are UTF-8.
    out = subprocess.run(["git", *args], cwd=root, capture_output=True, encoding="utf-8", errors="replace")
    if check and out.returncode != 0:
        sys.exit(f"git {' '.join(args)} failed: {out.stderr.strip()}")
    return out.stdout if out.returncode == 0 else None


def default_base(root):
    """The merge base with origin/main (else main), so the answer covers the whole branch."""
    for ref in ("origin/main", "main"):
        base = git(root, "merge-base", "HEAD", ref, check=False)
        if base:
            return base.strip()
    return "HEAD"


def added_lines(root, base, head):
    """{path: {line number: text}} of the lines the diff adds; new untracked files count whole when `head` is the working tree."""
    diff = git(root, "diff", "-U0", "--no-renames", "--no-color", "--no-ext-diff", base, *([head] if head else []))
    added, path, line = {}, None, 0
    for raw in diff.splitlines():
        if raw.startswith("+++ "):
            path = raw[6:] if raw.startswith("+++ b/") else None
        elif raw.startswith("@@") and path:
            match = re.match(r"@@ -\S+ \+(\d+)", raw)
            line = int(match.group(1)) if match else 0
        elif raw.startswith("+") and path and not raw.startswith("+++"):
            added.setdefault(path, {})[line] = raw[1:]
            line += 1
    if head is None:
        for path in git(root, "ls-files", "--others", "--exclude-standard").splitlines():
            text = read(root, path, None)
            if text is not None:
                added[path] = dict(enumerate(text.splitlines(), 1))
    return added


def read(root, path, head):
    """The file as the diff leaves it, or None for a binary or missing one."""
    if head is None:
        try:
            return (root / path).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            return None
    return git(root, "show", f"{head}:{path}", check=False)


# ---- what a line is ----------------------------------------------------------------------------------

TEST_PATH = re.compile(r"(^|/)(tests|benches|examples)/|(^|/)tests?\.rs$|_tests?\.rs$")
# Code that only draws, plays sound or serves the developer, and code that runs outside the game process:
# the simulation checks skip it.
PRESENTATION = re.compile(
    r"(^|/)(render|camera)(/|\.rs$)|(^|/)(visuals|overlay|highlight)\.rs$"
    r"|^crates/(shards_cli|shards_script|shards_ui|shards_audio|shards_look|shards_fx|shards_settings|shards_dev|shards_steam"
    r"|shards_scatter|shards_locale|shards_hitstop)/"
)


def test_module(root, path, head, depth=0):
    """Whether the module in `path` is declared under `#[cfg(test)]` by its parent (`#[cfg(test)] mod mounting;`)."""
    parts = path.split("/")
    name = parts[-2] if parts[-1] == "mod.rs" else parts[-1][:-3]
    folder = "/".join(parts[:-2] if parts[-1] == "mod.rs" else parts[:-1])
    parents = [f"{folder}/lib.rs", f"{folder}/main.rs"] if folder.endswith("/src") else [f"{folder}.rs", f"{folder}/mod.rs"]
    if parts[-1] in ("lib.rs", "main.rs") or depth > 8:
        return False
    for parent in parents:
        text = read(root, parent, head)
        if text is None:
            continue
        lines = text.splitlines()
        for i, line in enumerate(lines):
            if re.match(rf"\s*(pub(\([a-z]+\))?\s+)?mod\s+{name}\s*;", line):
                above = [l.strip() for l in lines[max(0, i - 3) : i] if l.strip().startswith("#[")]
                return "#[cfg(test)]" in above or test_module(root, parent, head, depth + 1)
        return test_module(root, parent, head, depth + 1)
    return False


def tests_from(root, path, text, head):
    """The first line of `text` that is test code: 1 for a test file or module, the `#[cfg(test)]` of a test module, else past the end."""
    if TEST_PATH.search(path) or (path.endswith(".rs") and "/src/" in path and test_module(root, path, head)):
        return 1
    lines = text.splitlines() if text else []
    for i, line in enumerate(lines):
        if line.strip() == "#[cfg(test)]" and any(l.lstrip().startswith(("mod ", "pub mod ", "pub(crate) mod ")) for l in lines[i + 1 : i + 3]):
            return i + 1
    return len(lines) + 1


def code_of(line):
    """The Rust code of a line without its `//` comment (a `//` inside a string literal stays)."""
    stripped = line.lstrip()
    if stripped.startswith(("//", "/*", "*")):
        return ""
    quoted = False
    for i, c in enumerate(line):
        if c == '"' and (i == 0 or line[i - 1] != "\\"):
            quoted = not quoted
        elif not quoted and line.startswith("//", i):
            return line[:i]
    return line


# ---- the checks --------------------------------------------------------------------------------------
# Each: (level, name, which files, pattern, what is wrong and where the rule is). `which files` is one of
#   "rust"      Rust code that ships (no tests, no comments)
#   "sim"       "rust" minus presentation and code outside the game process
#   "rust-any"  any Rust line, tests and comments included
#   "build"     manifests, cargo config, workflows, build scripts
#   "text"      any added line outside the skills and the agent instructions

LINE_CHECKS = [
    ("rule", "build-flag", "build", re.compile(r"target-cpu\s*=\s*native|\+fma\b|scalar-math|enhanced-determinism"),
     "`-C target-cpu=native`, `+fma`, glam `scalar-math` and parry3d `enhanced-determinism` change the bits of the maths "
     f"(replays diverge between machines) or break rendering: never ({DETERMINISM}, \"Simulation maths\")"),
    ("rule", "bevy", "build", re.compile(r'^\s*bevy(_[a-z_]+)?\s*=\s*(\{[^}]*\bversion\s*=\s*"(?!=?0\.20\.)[^"]*"|"(?!=?0\.20\.)[^"]*")'),
     "Bevy is pinned to 0.20.0: do not move it as a side effect of another task (AGENTS.md, \"Rules every change follows\")"),
    ("rule", "llm", "build", re.compile(r"(?i)^\s*\"?(async-openai|openai[\w-]*|anthropic[\w-]*|ollama[\w-]*|llm[\w-]*|llama[\w-]*|langchain[\w-]*|genai|mistralai|cohere[\w-]*)\"?\s*="),
     "the engine has no LLM and must not have one: what an agent needs is a tool or a `shards` command (AGENTS.md, \"The main rule\")"),
    ("rule", "llm", "rust", re.compile(r"(?i)\b(openai|anthropic|gemini|mistral|cohere)_api_key\b|api\.openai\.com|api\.anthropic\.com"),
     "the engine has no LLM and must not have one: no model APIs or keys in the game (AGENTS.md, \"The main rule\")"),
    ("rule", "randomness", "rust", re.compile(r"\brand::|\bthread_rng\s*\(|\bfastrand::|\bgetrandom\b|\bRandomState\b|\bDefaultHasher\b|Uuid::new_v4"),
     "randomness comes only from `SimRng` or a keyed `DetRng`, and a hash that decides anything is a local FNV "
     f"(`crates/shards_encounter/src/spawn.rs`): std's hashers are seeded per process or may change between Rust releases ({DETERMINISM})"),
    ("rule", "leftover", "rust-any", re.compile(r"\b(dbg|todo|unimplemented)!\s*\("), "debugging or unfinished code"),
    ("rule", "attribution", "text", re.compile(
        r"(?i)\b(generated|written|authored|created|drafted|co-authored)[ -](by|with)\s+(an?\s+)?(ai|llm|claude|codex|chatgpt|gpt|copilot|gemini)\b"
        r"|\bclaude-(opus|sonnet|haiku|fable|instant|\d)|\bgpt-\d|\b(opus|sonnet|haiku|fable) \d(\.\d)?\b"),
     "code, comments and docs never name the agent or model that wrote them; attribution goes in a commit's trailers, as your environment says"),
    ("check", "hash-order", "sim", re.compile(r"\bHash(Map|Set)\b"),
     "if its iteration order reaches the simulation, a tool's reply, a journal line, a snapshot or a save, use a BTreeMap/BTreeSet "
     f"or sort by `AiId`; a lookup that is never iterated is fine ({DETERMINISM})"),
    ("check", "real-time", "sim", re.compile(r"Instant::now|SystemTime::now|Time<Real>|Time<Virtual>|relative_speed\s*\("),
     "real time differs from run to run and must not decide gameplay; read the player's speed through `sim::speed()`, and only "
     f"presentation that keeps moving through a hit stop reads `Time<Real>` ({DETERMINISM})"),
    ("check", "local-state", "sim", re.compile(r"\bLocal<"),
     "a `Local` in a gameplay system is state no snapshot, replay or co-op peer holds: make it a component or a resource "
     "registered with `add_snapshot_resource` (a warn-once set or presentation state is fine)"),
    ("check", "observer", "sim", re.compile(r"\badd_observer\s*\(|\.observe\s*\("),
     "an observer that changes gameplay returns at once when `!shards::core::simulates(role)`, or a co-op client counts it "
     f"a second time ({DETERMINISM})"),
    ("check", "startup", "rust", re.compile(r"add_systems\s*\(\s*Startup\b"),
     "the world (anything with an `AiId` or a collider) is created in `OnEnter(EngineState::InGame)`, never in `Startup`: content "
     f"exists only after loading ({DETERMINISM})"),
    ("check", "schedule", "sim", re.compile(r"add_systems\s*\(\s*(Update|PostUpdate|PreUpdate)\b"),
     "gameplay runs in `FixedUpdate` inside a `SimSet` with an explicit order; `Update` is for reading input, presentation and UI"),
    ("check", "entity-order", "rust", re.compile(r"(?i)\b\w*(entity|body)\w*\.(index|to_bits|generation)\s*\(\)|\be\.(index|to_bits)\s*\(\)"),
     "order by `AiId` or `ColliderId`, never by `Entity`: a window adds render entities and shifts the indices (AGENTS.md)"),
    ("check", "global-transform", "sim", re.compile(r"\bGlobalTransform\b"),
     f"physics and the simulation read `Transform`: `GlobalTransform` is not recalculated inside a fixed tick ({DETERMINISM})"),
    ("check", "scale", "sim", re.compile(r"\bwith_scale\s*\(|\bfrom_scale\s*\(|\.scale\s*=[^=]"),
     f"a collider's size goes in its `Shape`, never in `Transform::scale`; colliders sit on top-level entities ({DETERMINISM})"),
    ("check", "ai-id", "rust", re.compile(r"\b(insert|spawn)\s*\(.*\bAiId\s*\("),
     "the engine assigns `AiId`; never write one (docs/agents/tools-and-invariants.md)"),
    ("check", "lint-escape", "rust-any", re.compile(r"#\[(allow|expect)\(clippy::(?!.*\breason\s*=)"),
     "say why with `reason = \"...\"` (and prefer `expect` to `allow`); `clippy.toml` says when `disallowed_methods` may be excused"),
    ("check", "unsafe", "rust", re.compile(r"\bunsafe\s*(\{|fn\b|impl\b|extern\b)"),
     "unsafe code: it needs a `// SAFETY:` comment and a careful second reader"),
]

CYRILLIC = re.compile(r"[Ѐ-ӿ]")
DERIVE = re.compile(r"#\[derive\(([^)]*)\)\]")
STRING = re.compile(r'"([A-Za-z][A-Za-z0-9_\-]{2,})"')
EXEMPT_TEXT = re.compile(r"^(\.agents/|\.claude/|AGENTS\.md$|CLAUDE\.md$)")


def kind_of(path):
    if path.endswith(".rs"):
        return "rust"
    if (path.endswith((".toml", ".yml", ".yaml", ".sh", ".ps1")) and not path.endswith("clippy.toml")) or path.startswith(".cargo/"):
        return "build"
    return "other"


def applies(scope, path, kind, is_test, code):
    if scope == "text":
        return not EXEMPT_TEXT.search(path)
    if scope == "build":
        return kind == "build"
    if scope == "rust-any":
        return kind == "rust"
    if kind != "rust" or is_test or not code.strip():
        return False
    return scope == "rust" or not PRESENTATION.search(path)


GAME_KINDS = ("prefab", "region", "voxel_region", "arena", "encounter")


def game_ids(root, head):
    """Ids the two games give their own things (prefabs, regions, arenas, encounters): names the engine must not know."""
    ids = set()
    for folder in ("crates/sandbox/assets/content", "template/assets/content"):
        if head is None:
            names = [str(f.relative_to(root)).replace("\\", "/") for f in (root / folder).rglob("*.ron")]
        else:
            names = (git(root, "ls-tree", "-r", "--name-only", head, "--", folder, check=False) or "").splitlines()
        for name in names:
            if name.endswith(".ron") and name.split(".")[-2] in GAME_KINDS:
                try:
                    ids.update(definitions(read(root, name, head) or ""))
                except ValueError:
                    continue  # a file the content check refuses anyway
    return ids


def engine_literals(root, head, skip):
    """String literals in the engine's shipped code outside the lines in `skip` ({path: set of line numbers})."""
    seen = set()
    if head is None:
        files = [str(p.relative_to(root)).replace("\\", "/") for p in (root / "crates").glob("*/src/**/*.rs")]
    else:
        files = [f for f in (git(root, "ls-tree", "-r", "--name-only", head, "--", "crates", check=False) or "").splitlines() if "/src/" in f and f.endswith(".rs")]
    for path in files:
        if path.startswith("crates/sandbox/") or TEST_PATH.search(path):
            continue
        text = read(root, path, head) or ""
        end = tests_from(root, path, text, head)
        for n, line in enumerate(text.splitlines(), 1):
            if n >= end:
                break
            if n not in skip.get(path, ()):
                seen.update(STRING.findall(code_of(line)))
    return seen


def findings(root, base, head):
    added = added_lines(root, base, head)
    found = []
    engine_added = {p: set(lines) for p, lines in added.items() if p.startswith("crates/") and not p.startswith("crates/sandbox/")}
    ids = game_ids(root, head) if engine_added else set()
    known = engine_literals(root, head, engine_added) if ids else set()
    for path in sorted(added):
        kind = kind_of(path)
        text = read(root, path, head) if kind == "rust" else None
        test_line = tests_from(root, path, text, head) if kind == "rust" else 1
        lines = text.splitlines() if text else []
        file_added = "\n".join(added[path].values())
        for n, line in sorted(added[path].items()):
            is_test = kind == "rust" and n >= test_line
            code = code_of(line) if kind == "rust" else line
            for level, name, scope, pattern, why in LINE_CHECKS:
                subject = line if scope in ("text", "build", "rust-any") else code
                if applies(scope, path, kind, is_test, code) and pattern.search(subject):
                    if name == "observer" and "simulates" in file_added:
                        continue
                    found.append((level, name, path, n, line, why))
            # Code, comments and docs in English; Russian only in game text and test data.
            if CYRILLIC.search(line) and not (
                path.endswith((".ftl", ".scenario", ".json")) or "/locale/" in path or is_test or TEST_PATH.search(path) or EXEMPT_TEXT.search(path)
            ):
                found.append(("check", "language", path, n, line, "code, comments and docs are in English and Russian stays in game text "
                              "(Fluent `.ftl`) and test data (AGENTS.md, \"Rules every change follows\"); quoting what a game shows (a "
                              "language's own name, a line of its text) is fine"))
            if kind == "rust" and not is_test:
                derive = DERIVE.search(code)
                if derive:
                    names = {d.strip().split("::")[-1] for d in derive.group(1).split(",")}
                    if names & {"Component", "Resource"} and "Reflect" not in names and not PRESENTATION.search(path):
                        found.append(("check", "state", path, n, line, "a component a system puts on a body with an `AiId`, or a resource the "
                                      "simulation keeps, is state: derive `Reflect`, `#[reflect(Component)]` or `#[reflect(Resource)]`, `register_type`, and "
                                      "`add_snapshot_resource` for a resource; or it is not state (a cache: `add_snapshot_rebuild`; cosmetics: "
                                      f"`replay_ignore`) ({DETERMINISM})"))
                    if "JsonSchema" in names and not documented(lines, n):
                        found.append(("check", "undocumented", path, n, line, "a struct that derives `JsonSchema` is described by its doc comment: for "
                                      "a tool's arguments it is the tool's description (one on the `fn` is never read)"))
                if path in engine_added:
                    for literal in STRING.findall(code):
                        if literal in ids and literal not in known:
                            found.append(("check", "boundary", path, n, line, f"`{literal}` is a game's own name (the sandbox's or the template's "
                                          "content): the engine takes ids from content or its caller, never from one game (AGENTS.md, \"The boundary\")"))
    for path in sorted(added):
        if re.match(r"^(template/)?tests/replays/[^/]+\.json$", path):
            found.append(("check", "replay", path, 0, "", "a reference replay changed: it moves only on purpose; say which change moved it "
                          "(`.agents/skills/shards-verify/scripts/which_replays.py`) and that only the expected replays changed"))
    return found


def documented(lines, n):
    """Whether the item whose `#[derive]` is on line `n` (1-based) has a `///` doc comment above its attributes."""
    i = n - 2
    while i >= 0 and lines[i].lstrip().startswith("#["):
        i -= 1
    return i >= 0 and lines[i].lstrip().startswith("///")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the commit to compare with (default: the merge base with origin/main)")
    parser.add_argument("--head", help="the commit to review (default: the working tree, uncommitted edits and new files included)")
    args = parser.parse_args()
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip())
    base = args.base or default_base(root)
    found = findings(root, base, args.head)
    span = f"{base[:12]}..{args.head or 'the working tree'}"
    if not found:
        print(f"red flags in {span}: none. Clippy, the tests and the checklist still apply.")
        return 0
    rules = sum(1 for f in found if f[0] == "rule")
    print(f"red flags in {span}: {rules} rule, {len(found) - rules} check")
    for level, name, path, n, line, why in sorted(found, key=lambda f: (f[0] != "rule", f[2], f[3])):
        where = f"{path}:{n}" if n else path
        print(f"\n[{level}] {name}  {where}")
        if line.strip():
            print(f"    {line.strip()[:160]}")
        print(f"    {why}")
    return 1 if rules else 0


if __name__ == "__main__":
    sys.exit(main())
