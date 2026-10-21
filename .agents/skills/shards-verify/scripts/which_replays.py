#!/usr/bin/env python3
"""Which reference replays read the content you changed, and so may diverge.

A reference replay records the content definitions its simulation read (`check_parts.content`: keys
`kind/id`, or `kind (all)` when the session read a whole kind). Changing or removing a definition it
read makes it diverge at its next check; adding a definition moves only the replays that read that kind
whole; content it never read moves nothing (docs/agents/checking-changes.md). A replay that places a
level also checks the level file's bytes (its `level` check), so an edited glTF level moves it.

    python3 .agents/skills/shards-verify/scripts/which_replays.py                 # what changed against origin/main
    python3 .agents/skills/shards-verify/scripts/which_replays.py --base HEAD     # only uncommitted edits
    python3 .agents/skills/shards-verify/scripts/which_replays.py crates/sandbox/assets/content/combat/downed.action.ron
    python3 .agents/skills/shards-verify/scripts/which_replays.py action/light_1 prefab/grunt

It compares each definition's text (comments and spacing ignored) with the base commit, over all the
changed files of a kind together, so only the definitions you touched count, even when they moved to
another file. It cannot see code: a change to gameplay code moves the replays that reach it whatever this
says. Run the replays to confirm (`cargo test -p sandbox --test sandbox reference_replays`).
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

# Where each game's content lives, where its reference replays are, and how to check them.
GAMES = [
    ("sandbox", "crates/sandbox/", "tests/replays", "cargo test -p sandbox --test sandbox reference_replays"),
    ("template", "template/", "template/tests/replays", "cargo test --manifest-path template/Cargo.toml --test game replays"),
]
RECORD = {
    "sandbox": "cargo test -p sandbox --test sandbox -- --ignored record_reference_replays",
    "template": "(cd template && cargo test --test game record_reference_replays -- --ignored)",
}


def git(root, *args, check=True):
    # UTF-8 whatever the platform's code page: content files and the CLI's replies are UTF-8.
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


def changed_files(root, base):
    """Files changed against `base`, committed or not, and new files. A rename counts as its two paths."""
    tracked = git(root, "diff", "--name-only", "--no-renames", base).splitlines()
    untracked = git(root, "ls-files", "--others", "--exclude-standard").splitlines()
    return sorted(set(tracked) | set(untracked))


# ---- RON: the top-level map of a content file, `{ "id": (...), ... }` -------------------------------


def _skip_string(text, i):
    """`text[i]` is `"`: the index after the closing quote."""
    i += 1
    while i < len(text):
        if text[i] == "\\":
            i += 2
            continue
        if text[i] == '"':
            return i + 1
        i += 1
    return i


def _skip_raw_string(text, i):
    """`text[i:]` starts `r#*"`: the index after the closing `"#*`, or None if it is not a raw string."""
    j = i + 1
    while j < len(text) and text[j] == "#":
        j += 1
    if j >= len(text) or text[j] != '"':
        return None
    close = '"' + "#" * (j - i - 1)
    end = text.find(close, j + 1)
    return len(text) if end < 0 else end + len(close)


def _skip_comment(text, i):
    """The index after a comment starting at `i`, or None when there is none. Block comments nest in RON."""
    if text.startswith("//", i):
        end = text.find("\n", i)
        return len(text) if end < 0 else end
    if text.startswith("/*", i):
        depth, j = 0, i
        while j < len(text):
            if text.startswith("/*", j):
                depth, j = depth + 1, j + 2
            elif text.startswith("*/", j):
                depth, j = depth - 1, j + 2
                if depth == 0:
                    return j
            else:
                j += 1
        return len(text)
    return None


def normalize(text):
    """The text without comments and without whitespace outside strings: two spellings of one value compare equal."""
    out, i = [], 0
    while i < len(text):
        skipped = _skip_comment(text, i)
        if skipped is not None:
            i = skipped
            continue
        c = text[i]
        if c == '"':
            j = _skip_string(text, i)
            out.append(text[i:j])
            i = j
        elif c == "r" and (raw := _skip_raw_string(text, i)) is not None:
            out.append(text[i:raw])
            i = raw
        elif c.isspace():
            i += 1
        else:
            out.append(c)
            i += 1
    return "".join(out).rstrip(",")


def definitions(text):
    """`{id: normalized text}` of the top-level map, or None when the file does not parse as one."""
    entries, depth, i = {}, 0, 0
    key, start, expecting_key = None, None, False
    while i < len(text):
        skipped = _skip_comment(text, i)
        if skipped is not None:
            i = skipped
            continue
        c = text[i]
        if depth == 0 and text.startswith("#![", i):  # `#![enable(...)]` extensions
            end = text.find("]", i)
            i = len(text) if end < 0 else end + 1
            continue
        if c == '"':
            j = _skip_string(text, i)
            if depth == 1 and expecting_key:
                key, expecting_key = json.loads(text[i:j]), False
                colon = text.find(":", j)
                start = j if colon < 0 else colon + 1
            i = j
            continue
        if c == "r" and (raw := _skip_raw_string(text, i)) is not None:
            i = raw
            continue
        if c == "'":  # a char literal: 'a', or an escape like '\''
            end = text.find("'", i + 3 if text[i + 1 : i + 2] == "\\" else i + 2)
            i = len(text) if end < 0 else end + 1
            continue
        if c in "([{":
            depth += 1
            if depth == 1:
                if c != "{":
                    return None
                expecting_key = True
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                if key is not None:
                    entries[key] = normalize(text[start:i])
                return entries
        elif c == "," and depth == 1:
            if key is not None:
                entries[key] = normalize(text[start:i])
            key, expecting_key = None, True
        i += 1
    return None


# ---- what changed --------------------------------------------------------------------------------------


def repo_path(root, item):
    """`item` (absolute, or relative to the current folder) as a path from the repository root, with `/`."""
    path = Path(item)
    try:
        return (path if path.is_absolute() else Path.cwd() / path).resolve().relative_to(root.resolve()).as_posix()
    except ValueError:
        return item


def kind_of(path):
    """`combat/downed.action.ron` -> `action`; None for a file that is not `name.kind.ron`."""
    parts = Path(path).name.split(".")
    return parts[-2] if len(parts) >= 3 and parts[-1] == "ron" else None


def game_of(path):
    for name, prefix, _, _ in GAMES:
        if path.startswith(prefix + "assets/"):
            return name
    return None


def is_level_file(path):
    """A glTF a level or a tile is built from: the replays' `level` check hashes its bytes."""
    return path.endswith((".gltf", ".glb")) and "/assets/" in path and "/models/" not in path


def content_at(root, base, path):
    """`(definitions now, definitions at base)` of one content file; `{}` where it does not exist, None where it does not parse."""
    now_path = root / path
    now = definitions(now_path.read_text(encoding="utf-8")) if now_path.is_file() else {}
    before_text = git(root, "show", f"{base}:{path}", check=False)
    before = definitions(before_text) if before_text is not None else {}
    return now, before


def replay_reads(root, folder):
    """`{replay file name: (check_parts.content keys, whether it has a level check)}` for every replay in `folder`."""
    reads = {}
    for file in sorted((root / folder).glob("*.json")):
        try:
            data = json.loads(file.read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            print(f"  (cannot read {file}: {e})")
            continue
        keys = set()
        for part in (data.get("check_parts") or {}).get("content") or []:
            keys.update((part.get("set") or {}).keys())
        level = any("level" in ((c.get("parts") or {}).get("game") or {}) for c in data.get("checks") or [])
        reads[file.name] = (keys, level)
    return reads


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("items", nargs="*", help="content files, or definitions as kind/id; default: the files changed against --base")
    parser.add_argument("--base", help="the commit to compare with (default: the merge base with origin/main)")
    args = parser.parse_args()
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip())
    base = args.base or default_base(root)

    if args.items:
        files = [repo_path(root, i) for i in args.items if i.endswith((".ron", ".gltf", ".glb"))]
        names = [i for i in args.items if not i.endswith((".ron", ".gltf", ".glb"))]
    else:
        files = [f for f in changed_files(root, base) if f.endswith((".ron", ".gltf", ".glb"))]
        names = []
        print(f"content and level files changed against {base[:12]}: {len(files)}")

    # (game, kind) -> definitions of all its changed files, now and at base; None: a file did not parse.
    now, before, unparsed = {}, {}, set()
    levels = {}  # game -> changed level files
    for path in files:
        game, kind = game_of(path), kind_of(path)
        if game and is_level_file(path):
            levels.setdefault(game, []).append(path)
            print(f"  {path}: a level file, whose bytes the replays that place it check")
        elif "/assets/nav/" in f"/{path}":
            print(f"  {path}: a baked navmesh, outside the content check: replays whose NPCs walk it may move")
        elif game is None or kind is None:
            if kind:
                print(f"  {path}: test content of its crate: no reference replay reads it")
        else:
            defs_now, defs_before = content_at(root, base, path)
            if defs_now is None or defs_before is None:
                print(f"  {path}: not a map of definitions (cannot tell what changed): every replay that reads `{kind}` may move")
                unparsed.add((game, kind))
                continue
            now.setdefault((game, kind), {}).update(defs_now)
            before.setdefault((game, kind), {}).update(defs_before)

    # (game, kind) -> (changed or removed ids, added ids)
    changes = {}
    for key in sorted(set(now) | set(before)):
        a, b = now.get(key, {}), before.get(key, {})
        changed = sorted(i for i in a.keys() & b.keys() if a[i] != b[i])
        removed, added = sorted(b.keys() - a.keys()), sorted(a.keys() - b.keys())
        if changed or removed or added:
            print(f"  {key[0]} {key[1]}: changed {changed or '-'}, removed {removed or '-'}, added {added or '-'}")
            changes[key] = (changed + removed, added)
    for name in names:
        kind, _, ident = name.partition("/")
        for game, _, _, _ in GAMES:
            changes.setdefault((game, kind), ([], []))[0].append(ident)

    if not changes and not unparsed and not levels:
        print("no content definition or level file changed: no reference replay moves because of content")
        return
    moved_any = False
    for game, _, folder, check in GAMES:
        kinds = {k: v for (g, k), v in changes.items() if g == game}
        whole = {k for (g, k) in unparsed if g == game}
        if not kinds and not whole and not levels.get(game):
            continue
        reads = replay_reads(root, folder)
        moved = {}
        for replay, (keys, level) in reads.items():
            hits = []
            for kind, (ids, added) in sorted(kinds.items()):
                hits += [f"{kind}/{i}" for i in ids if f"{kind}/{i}" in keys]
                if f"{kind} (all)" in keys and (ids or added):
                    hits.append(f"{kind} (all)")
            for kind in sorted(whole):
                if any(k == f"{kind} (all)" or k.startswith(f"{kind}/") for k in keys):
                    hits.append(f"{kind} (some)")
            if level and levels.get(game):
                hits.append("level")
            if hits:
                moved[replay] = hits
        print(f"\n{game}: {len(moved)} of {len(reads)} reference replays in {folder}/ read what changed")
        for replay, hits in moved.items():
            print(f"  {replay:<24} {', '.join(hits)}")
        if moved:
            moved_any = True
            print(f"  confirm: {check}\n  if the change is meant, record them again: {RECORD[game]}")
    if moved_any:
        print("\nThey diverge at their first check after the read. After recording, `git diff --stat` must show only them"
              "\n(.agents/skills/shards-verify/SKILL.md, section 5).")


if __name__ == "__main__":
    main()
