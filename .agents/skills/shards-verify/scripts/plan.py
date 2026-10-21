#!/usr/bin/env python3
"""The checks a change needs, from the files it touches: what to run while you work, what to bake or
regenerate, the reference replays that may move, the gate CI will run, and the docs the change owes.

    python3 .agents/skills/shards-verify/scripts/plan.py               # everything changed against origin/main
    python3 .agents/skills/shards-verify/scripts/plan.py --base HEAD   # only what is not committed yet

The rules are paths, not understanding: a change to `shards_core` reaches every module above it, and a
refactor owes no manual section. Read the plan, then add what it cannot know. The rules it applies are
in docs/agents/checking-changes.md and AGENTS.md ("Gate: checking a change").
"""

import argparse
import fnmatch
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the repository
sys.path.insert(0, str(Path(__file__).resolve().parent))
import which_replays  # noqa: E402  (beside this file)

# The manual's section and the docs/agents topic for each crate.
CRATES = {
    "ai_core": ("The AI layer; Invariants (E11); Replays; Snapshots (E11.4)", "tools-and-invariants.md determinism.md"),
    "shards": ("Architecture; Make a game on Shards, step by step", "determinism.md"),
    "shards_core": ("Core (E1)", "determinism.md"),
    "shards_content": ("Content (E4)", "content.md"),
    "shards_motion": ("Movement (E2)", "determinism.md"),
    "shards_action": ("Actions and combat (E3)", "combat.md determinism.md"),
    "shards_level": ("Levels from Blender (E4.2)", "world-regions-nav.md"),
    "shards_world": ("World and generation (E5)", "world-regions-nav.md"),
    "shards_voxel": ("Voxel terrain (core); Voxel regions (A2c)", "world-regions-nav.md"),
    "shards_npc": ("Navigation and NPCs (E6)", "npc.md"),
    "shards_encounter": ("Encounters (G9)", "npc.md tools-and-invariants.md"),
    "shards_net": ("Co-op session (E7, `shards_net`)", "net-coop-steam.md"),
    "shards_steam": ("Steam (E10.5)", "net-coop-steam.md"),
    "shards_save": ("Saves (E8.1–5)", "determinism.md"),
    "shards_settings": ("Settings (E8.6)", ""),
    "shards_ui": ("UI (E9)", "ui-audio-look-fx-scatter-animation.md"),
    "shards_locale": ("Localization (E9.4)", "content.md"),
    "shards_audio": ("Audio (E9.5)", "ui-audio-look-fx-scatter-animation.md"),
    "shards_look": ("Stylised materials (E10.1)", "ui-audio-look-fx-scatter-animation.md"),
    "shards_scatter": ("Foliage and small objects (E10.2)", "ui-audio-look-fx-scatter-animation.md"),
    "shards_fx": ("Effects and decals (E10.3)", "ui-audio-look-fx-scatter-animation.md"),
    "shards_hitstop": ("Hit stop (C2)", "determinism.md"),
    "shards_rules": ("Rules (C3)", "net-coop-steam.md"),
    "shards_dev": ("Developer menu and console (E11)", "watching-and-driving.md"),
    "shards_script": ("Scenarios (E11)", "watching-and-driving.md"),
    "shards_cli": ("Transports and the CLI", "watching-and-driving.md"),
    "sandbox": ("The sandbox", "checking-changes.md"),
}

# Content and asset patterns (fnmatch on the path from the repository root).
NAV_SOURCES = ["*.tile.ron", "*.level.ron", "*.nav.ron", "*/assets/tiles/*", "*/assets/levels/*",
               "crates/shards_npc/src/nav/*", "crates/shards_motion/src/controller/*"]
MOTION_SOURCES = ["*.animset.ron", "*/assets/models/*", "tools/blender/humanoid.py"]
WORLDGEN_SOURCES = ["*.region.ron", "*.tile.ron", "*.socket_rules.ron", "*.poi.ron", "*.tile_map.ron",
                    "*.dungeon.ron", "*.voxel_region.ron", "*.voxel_palette.ron",
                    "crates/shards_world/src/*", "crates/shards_voxel/src/*"]
NET_SOURCES = ["crates/shards_net/*", "crates/shards_steam/*", "crates/shards/src/net_wiring.rs", "crates/shards/src/coop.rs",
               "crates/shards/src/stand_udp.rs", "*.net.ron", "*/scenarios/stand/*"]
AGENT_DOCS = ["AGENTS.md", "CLAUDE.md", "docs/agents/*", ".agents/skills/*", ".claude/skills/*"]


def any_match(path, patterns):
    return any(fnmatch.fnmatch(path, p) for p in patterns)


def crate_of(path):
    parts = path.split("/")
    return parts[1] if len(parts) > 2 and parts[0] == "crates" else None


def lockfiles_agree(root):
    """CI's check (job `template`): every registry package the template pins, the engine pins the same way."""
    try:
        import tomllib
    except ImportError:
        return None, "python 3.11 or newer is needed to read the lockfiles"

    def versions(path):
        with open(root / path, "rb") as f:
            return {(p["name"], p["version"]) for p in tomllib.load(f)["package"] if "source" in p}

    drift = sorted(versions("template/Cargo.lock") - versions("Cargo.lock"))
    return not drift, ", ".join(f"{n} {v}" for n, v in drift)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--base", help="the commit to compare with (default: the merge base with origin/main)")
    args = parser.parse_args()
    root = Path(which_replays.git(Path.cwd(), "rev-parse", "--show-toplevel").strip())
    base = args.base or which_replays.default_base(root)
    files = which_replays.changed_files(root, base)
    if not files:
        print(f"nothing changed against {base[:12]}")
        return

    crates = sorted({c for f in files if (c := crate_of(f))})
    code = sorted({c for f in files if (c := crate_of(f)) and f.startswith(f"crates/{c}/src/")})
    gameplay_code = [c for c in code if c not in ("shards_cli", "shards_script", "sandbox")]
    template = [f for f in files if f.startswith("template/")]
    print(f"changed against {base[:12]}: {len(files)} files" + (f"; crates: {', '.join(crates)}" if crates else "")
          + ("; template" if template else ""))

    read, fast, regen, world, replays, gate, docs, notes = [], [], [], [], [], [], [], []

    # --- rules to read first
    topics = sorted({t for c in crates for t in CRATES.get(c, ("", ""))[1].split() if t})
    read += [f"docs/agents/{t}" for t in topics]
    if gameplay_code and "determinism.md" not in topics:
        read.append("docs/agents/determinism.md (gameplay code changed)")

    # --- fast checks per crate
    for c in crates:
        if c == "sandbox":
            continue
        fast.append(f"cargo test -p {c}")
        fast.append(f"cargo clippy -p {c} --all-targets -- -D warnings")
    sandbox_game = ("crates/sandbox/src/", "crates/sandbox/assets/", "crates/sandbox/scenarios/", "crates/sandbox/tests/sandbox/")
    if gameplay_code or any(f.startswith(sandbox_game) for f in files):
        fast.append("cargo test -p sandbox --test sandbox      # end to end: every scenario both ways, reference replays, engine rules, state audit")
    if any(f.startswith("crates/sandbox/src/") for f in files):
        fast.append("cargo test -p sandbox                     # all of the sandbox's test binaries")
    agents_index = "cargo test -p sandbox --test agents_index  # AGENTS.md, docs/agents and the skills agree"
    for f in files:
        parts = f.split("/")
        if len(parts) == 4 and parts[:3] == ["crates", "sandbox", "tests"] and parts[3].endswith(".rs"):
            fast.append(agents_index if parts[3] == "agents_index.rs" else f"cargo test -p sandbox --test {parts[3][:-3]}")
    scenarios = [f for f in files if f.startswith("crates/sandbox/scenarios/") and f.endswith(".scenario")]
    if scenarios:
        fast.append("each changed scenario against a live game, twice in a row: python3 .agents/skills/shards-playtest/scripts/live.py start, "
                    "then cargo shards --port-file target/live/sandbox.port scenario <file>")
        recorded = recorded_scenarios(root)
        for s in scenarios:
            if Path(s).name in recorded:
                replays.append(f"{Path(s).name} records tests/replays/{recorded[Path(s).name]}: record it again (and only it should change)")
    if any(any_match(f, AGENT_DOCS) for f in files):
        fast.append(agents_index)
    if template:
        fast.append("cargo test --manifest-path template/Cargo.toml")

    # --- regenerate
    if any(any_match(f, NAV_SOURCES) for f in files):
        regen.append("cargo sandbox --bake-nav   # tiles, level files, nav settings or the bake code changed; commit assets/nav/")
        if any(f.startswith(("crates/shards_npc/src/nav/", "crates/shards_motion/src/controller/")) for f in files):
            notes.append("the bake code changed: if what a tile bakes to changed, raise BAKE_VERSION and bake everything (docs/agents/world-regions-nav.md)")
        if any(f.startswith("template/") and any_match(f, NAV_SOURCES) for f in files):
            regen.append("(cd template && cargo run -- --bake-nav)")
    if any(any_match(f, MOTION_SOURCES) for f in files):
        regen.append("cargo sandbox --bake-motion   # an animset or a clip changed: re-bake the displacement curves (template: (cd template && cargo run -- --bake-motion))")

    # --- content and world
    if any(f.startswith("crates/sandbox/assets/") or f.startswith("crates/shards_content/") for f in files) or gameplay_code:
        world.append("cargo sandbox --check-content")
    if any(f.startswith("template/assets/") for f in files) or (template and any(f.endswith(".rs") for f in template)):
        world.append("(cd template && cargo run -- --check-content)")
    if any(any_match(f, WORLDGEN_SOURCES) for f in files):
        world.append("cargo sandbox --worldgen-report wilds --seeds 1..1000")
        world.append("cargo sandbox --worldgen-report voxel_arena --seeds 1..1000")
        if template or any(f.startswith("crates/shards_world/src/") for f in files):
            world.append("(cd template && cargo run -- --worldgen-report valley --seeds 1..1000)")
        notes.append("the generator's output changed? raise the region's `version` and update the manifest hash its scenario expects")
    if any(any_match(f, NET_SOURCES) for f in files):
        world.append("stand: cargo sandbox --stand 2 --headless --paused --port 0 --port-file F --net-sim latency=150,loss=0.02 &, "
                     "cargo shards --port-file F wait --peer all, then the stand scenarios and `cargo shards --port-file F soak --minutes 3`")

    # --- reference replays
    content = [f for f in files if f.endswith(".ron") and "/assets/" in f]
    if content:
        replays.append("content the simulation may read changed: python3 .agents/skills/shards-verify/scripts/which_replays.py")
    if gameplay_code:
        replays.append(f"gameplay code changed ({', '.join(gameplay_code)}): the reference replays must verify unchanged unless "
                       "the change is meant to change gameplay; a divergence you did not predict is a bug (shards-debug)")
    if content or gameplay_code or scenarios:
        replays.append("check: cargo test -p sandbox --test sandbox reference_replays")
    if template:
        replays.append("template: cargo test --manifest-path template/Cargo.toml --test game replays")

    # --- the gate
    engine = bool(crates) or any(f in ("Cargo.toml", "Cargo.lock") or f.startswith((".cargo/", ".config/", "clippy.toml")) for f in files)
    if engine:
        gate += ["cargo test --workspace     # CI: cargo nextest run --workspace --profile ci -E 'not binary_id(sandbox::perf)'",
                 "cargo clippy --workspace --all-targets -- -D warnings"]
    if template or [c for c in crates if c not in ("sandbox", "shards_cli")]:
        gate += ["cargo test --manifest-path template/Cargo.toml",
                 "cargo clippy --manifest-path template/Cargo.toml --all-targets -- -D warnings",
                 "(cd template && cargo run -- --check-content && cargo run -- --worldgen-report valley --seeds 1..1000 "
                 "&& for r in tests/replays/*.json; do cargo run -- --replay \"$r\" --verify || exit 1; done)"]
    if any(f.startswith(("crates/shards_cli/", "crates/shards_script/")) for f in files) or scenarios:
        gate.append("live, as CI's headless-cli: every crates/sandbox/scenarios/*.scenario against one live game, then again in reverse order")
    if any(f in ("Cargo.lock", "template/Cargo.lock", "Cargo.toml", "template/Cargo.toml") or f.endswith("/Cargo.toml") for f in files):
        agree, why = lockfiles_agree(root)
        if agree is None:
            gate.append(f"lockfiles: cannot check here ({why}); CI's `template` job checks them")
        elif agree:
            gate.append("lockfiles: template/Cargo.lock agrees with Cargo.lock (checked now)")
        else:
            gate.append(f"lockfiles DISAGREE ({why}): cp Cargo.lock template/Cargo.lock && "
                        "cargo metadata --manifest-path template/Cargo.toml --format-version 1 >/dev/null")
    if any(f.startswith(".github/") for f in files):
        notes.append("CI files changed: nothing runs them here; compare with what docs/agents/checking-changes.md and the manual's 'Tests and CI' say")

    # --- docs owed
    def owed(path, what):
        docs.append(f"[{'done' if path in files else 'todo'}] {path}: {what}")

    if code:
        owed("CHANGELOG.md", "an entry under Unreleased (Added, Fixed or Changed)")
        sections = "; ".join(CRATES.get(c, ("?", ""))[0] for c in code)
        owed("docs/manual.md", f"the section(s) of what changed: {sections}")
        owed("docs/engine-progress.md", "the status row of the module or slice, and the crate's test count in 'Tests'")
        notes.append("docs/agents/<topic>.md if agents must now follow a new rule; README.md only if features or the quick start changed")
    if any(f.startswith(("template/src/", "template/assets/", "template/scenarios/", "template/tests/")) for f in files):
        owed("template/README.md", "the table of files, the section of what you changed, the test count")

    for title, items in [("Read first", read), ("Fast checks, while you work", fast), ("Regenerate, then commit what they write", regen),
                         ("Content, world and co-op", world), ("Reference replays", replays), ("Before you push (CI's gate for this diff)", gate),
                         ("Docs this change owes", docs), ("Notes", notes)]:
        if items:
            print(f"\n{title}:")
            for item in dict.fromkeys(items):  # each once, in order
                print(f"  {item}")


def recorded_scenarios(root):
    """`{scenario file name: replay file name}` from RECORDED in the sandbox's reference_replays.rs."""
    source = root / "crates/sandbox/tests/sandbox/reference_replays.rs"
    pairs = {}
    try:
        for line in source.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line.startswith('("') and ".json" in line and ".scenario" in line:
                replay, scenario = [part.strip(' "(),') for part in line.split(",")[:2]]
                pairs[scenario] = replay
    except OSError:
        pass
    return pairs


if __name__ == "__main__":
    main()
