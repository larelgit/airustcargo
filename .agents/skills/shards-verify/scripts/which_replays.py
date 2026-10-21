#!/usr/bin/env python3
"""Which reference replays read the content you changed, and so may diverge.

A reference replay records the content definitions its simulation read (`check_parts.content`: keys
`kind/id`, or `kind (all)` when the session read a whole kind). Changing or removing a definition it
read makes it diverge at its next check; adding a definition moves only the replays that read that kind
whole; content it never read moves nothing (docs/agents/checking-changes.md).

    python3 .agents/skills/shards-verify/scripts/which_replays.py                 # *.ron changed against origin/main
    python3 .agents/skills/shards-verify/scripts/which_replays.py --base HEAD     # only uncommitted edits
    python3 .agents/skills/shards-verify/scripts/which_replays.py crates/sandbox/assets/content/combat/downed.action.ron
    python3 .agents/skills/shards-verify/scripts/which_replays.py action/light_1 prefab/grunt

It compares each definition's text (comments and spacing ignored) with the base commit, so only the
definitions you touched count. It cannot see code: a change to gameplay code moves the replays that reach
it whatever this says. Run the replays to confirm (`cargo test -p sandbox --test sandbox reference_replays`).
"""

import argparse
import json
import subprocess
import sys
from pathlib import Path

# Where each game's content lives, and where its reference replays are.
GAMES = [
    ("sandbox", "crates/sandbox/", "tests/replays"),
    ("template", "template/", "template/tests/replays"),
]


def git(root, *args, check=True):
    out = subprocess.run(["git", *args], cwd=root, capture_output=True, text=True)
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
    tracked = git(root, "diff", "--name-only", base).splitlines()
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
    for name, prefix, replays in GAMES:
        if path.startswith(prefix + "assets/"):
            return name, replays
    return None, None


def file_changes(root, base, path):
    """`(changed, added, removed)` definition ids of one content file against `base`."""
    now_path = root / path
    now = definitions(now_path.read_text(encoding="utf-8")) if now_path.is_file() else {}
    before_text = git(root, "show", f"{base}:{path}", check=False)
    before = definitions(before_text) if before_text is not None else {}
    if now is None or before is None:
        return None
    changed = sorted(k for k in now.keys() & before.keys() if now[k] != before[k])
    return changed, sorted(now.keys() - before.keys()), sorted(before.keys() - now.keys())


def replay_reads(root, folder):
    """`{replay file name: set of check_parts.content keys}` for every replay in `folder`."""
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
        reads[file.name] = keys
    return reads


def touched_by(keys, kind, ids, added):
    """The keys of one replay that a change to `kind` reaches: the ids it read, or the whole kind."""
    hits = [f"{kind}/{i}" for i in ids if f"{kind}/{i}" in keys]
    if f"{kind} (all)" in keys and (ids or added):
        hits.append(f"{kind} (all)")
    return hits


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("items", nargs="*", help="content files, or definitions as kind/id; default: the *.ron files changed against --base")
    parser.add_argument("--base", help="the commit to compare with (default: the merge base with origin/main)")
    args = parser.parse_args()
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").strip())
    base = args.base or default_base(root)

    # (game, kind) -> {"ids": changed or removed ids, "added": added ids, "whole": whether the file did not parse}
    wanted = {}
    files = [repo_path(root, i) for i in args.items if i.endswith(".ron")] if args.items else None
    names = [i for i in args.items if not i.endswith(".ron")]
    if files is None:
        files = [f for f in changed_files(root, base) if f.endswith(".ron")]
        print(f"content files changed against {base[:12]}: {len(files)}")
    elsewhere = []
    for path in files:
        game, _ = game_of(path)
        kind = kind_of(path)
        if game is None or kind is None:
            elsewhere.append(path)
            continue
        if "/assets/nav/" in f"/{path}":
            elsewhere.append(path)
            continue
        found = file_changes(root, base, path)
        if found is None:
            print(f"  {path}: not a map of definitions (cannot tell what changed); treat every replay that reads `{kind}` as moved")
            wanted.setdefault((game, kind), {"ids": set(), "added": set(), "whole": True})
            continue
        changed, added, removed = found
        print(f"  {path} ({kind}): changed {changed or '-'}, added {added or '-'}, removed {removed or '-'}")
        entry = wanted.setdefault((game, kind), {"ids": set(), "added": set(), "whole": False})
        entry["ids"].update(changed + removed)
        entry["added"].update(added)
    for name in names:
        kind, _, ident = name.partition("/")
        for game, _, _ in GAMES:
            wanted.setdefault((game, kind), {"ids": set(), "added": set(), "whole": False})["ids"].add(ident)
    for path in elsewhere:
        if "/assets/nav/" in f"/{path}":
            print(f"  {path}: a baked navmesh, outside the content check: replays whose NPCs walk it may move")
        elif kind_of(path):
            print(f"  {path}: test content of its crate: no reference replay reads it")

    if not wanted:
        print("no content definition changed: no reference replay moves because of content")
        return
    for game, _, folder in GAMES:
        kinds = {k: v for (g, k), v in wanted.items() if g == game}
        if not kinds:
            continue
        reads = replay_reads(root, folder)
        moved = {}
        for replay, keys in reads.items():
            hits = []
            for kind, entry in sorted(kinds.items()):
                if entry["whole"] and any(k == f"{kind} (all)" or k.startswith(f"{kind}/") for k in keys):
                    hits.append(f"{kind} (some)")
                else:
                    hits += touched_by(keys, kind, sorted(entry["ids"]), entry["added"])
            if hits:
                moved[replay] = hits
        print(f"\n{game}: {len(moved)} of {len(reads)} reference replays in {folder}/ read what changed")
        for replay, hits in moved.items():
            print(f"  {replay:<24} {', '.join(hits)}")
    print(
        "\nThese diverge at their first check after the read. Confirm: cargo test -p sandbox --test sandbox reference_replays"
        "\n(template: cargo test --manifest-path template/Cargo.toml --test game replays). If the change is deliberate,"
        "\nrecord them again and check that only these changed: .agents/skills/shards-verify/SKILL.md, section 5."
    )


if __name__ == "__main__":
    main()
