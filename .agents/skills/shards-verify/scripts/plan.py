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
import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the repository
sys.path.insert(0, str(Path(__file__).resolve().parent))
import which_replays  # noqa: E402  (beside this file)

# The manual's section and the docs/agents topic for each crate. A new crate gets a line here.
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
# Crates whose code is not the simulation (the CLI and the scenario runner).
TOOLING = ("shards_cli", "shards_script")
TOOLING_PATHS = tuple(f"crates/{c}/" for c in TOOLING)

# Content and asset patterns (fnmatch on the path from the repository root; `*` crosses folders).
NAV_SOURCES = ["*.tile.ron", "*.level.ron", "*.nav.ron", "*.locomotion.ron", "*/assets/tiles/*", "*/assets/levels/*",
               "crates/shards_npc/src/nav/*", "crates/shards_motion/src/controller/*"]
NAV_CODE = ("crates/shards_npc/src/nav/", "crates/shards_motion/src/controller/")
MOTION_SOURCES = ["*.animset.ron", "*/assets/models/*", "tools/blender/humanoid.py"]
WORLDGEN_SOURCES = ["*.region.ron", "*.tile.ron", "*.socket_rules.ron", "*.poi.ron", "*.tile_map.ron",
                    "*.dungeon.ron", "*.voxel_region.ron", "*.voxel_palette.ron",
                    "crates/shards_world/src/*", "crates/shards_voxel/src/*"]
NET_SOURCES = ["crates/shards_net/*", "crates/shards_steam/*", "crates/shards/src/net_wiring.rs", "crates/shards/src/coop.rs",
               "crates/shards/src/stand_udp.rs", "*.net.ron"]
AGENT_DOCS = ["AGENTS.md", "CLAUDE.md", "docs/agents/*", ".agents/skills/*", ".claude/skills/*"]
LIVE = "python3 .agents/skills/shards-playtest/scripts/live.py"


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

    try:
        drift = sorted(versions("template/Cargo.lock") - versions("Cargo.lock"))
    except (OSError, ValueError, KeyError) as e:  # a lockfile with conflict markers, say
        return None, f"a lockfile does not read: {e}"
    return not drift, ", ".join(f"{n} {v}" for n, v in drift)


def recorded_scenarios(root):
    """`{scenario path: replay path}` for the scenarios the reference replays are recorded from."""
    pairs = {}
    try:
        for line in (root / "crates/sandbox/tests/sandbox/reference_replays.rs").read_text(encoding="utf-8").splitlines():
            found = re.match(r'\s*\("([^"]+\.json)",\s*"([^"]+\.scenario)"\)', line)
            if found:
                pairs[f"crates/sandbox/scenarios/{found[2]}"] = f"tests/replays/{found[1]}"
        names = re.search(r"for name in \[([^\]]*)\]", (root / "template/tests/game/replays.rs").read_text(encoding="utf-8"))
        for name in re.findall(r'"([^"]+)"', names[1] if names else ""):
            pairs[f"template/scenarios/{name}.scenario"] = f"template/tests/replays/{name}.json"
    except OSError:
        pass
    return pairs


def added_rust(root, base, files):
    """`[(path, line)]`: the lines the diff adds to Rust files outside tests (committed or not), new files whole."""
    found, path = [], None
    diff = which_replays.git(root, "diff", "-U0", "--no-renames", base, "--", "*.rs", check=False) or ""
    for line in diff.splitlines():
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else None
        elif line.startswith("+") and path:
            found.append((path, line[1:]))
    tracked = set(which_replays.git(root, "ls-files", check=False).splitlines())
    for f in files:
        if f.endswith(".rs") and f not in tracked and (root / f).is_file():
            found += [(f, line) for line in (root / f).read_text(encoding="utf-8", errors="replace").splitlines()]
    return [(f, line) for f, line in found if "/tests/" not in f and not f.startswith("tests/")]


def registered(lines, *calls):
    """The names that `calls` (`add_ai_tool`, …) register in `lines`: `add_ai_tool("taunt", …)` -> `taunt`."""
    pattern = re.compile(r"\b(?:" + "|".join(calls) + r")(?:::<[^>]*>)?\(\s*(?:world,\s*)?\"([^\"]+)\"")
    return sorted({m[1] for _, line in lines for m in pattern.finditer(line)})


def stand_line(root, path):
    """The words after `stand` on a stand scenario's header line (`clients=2 latency=150 loss=0.02`)."""
    try:
        for line in (root / path).read_text(encoding="utf-8").splitlines():
            words = line.split("#")[0].split()
            if words[:1] == ["stand"]:
                return " ".join(words[1:])
    except OSError:
        pass
    return None


def stand_command(line, game="sandbox"):
    """`live.py start` for a stand that runs the files with this `stand` line (the same clients and links)."""
    clients, links = "1", []
    for word in line.split():
        if word.startswith("clients="):
            clients = word.split("=", 1)[1]
        else:
            links.append(word)
    net = f" --net-sim {','.join(links)}" if links else ""
    which = " --game template" if game == "template" else ""
    return f"{LIVE} start{which} --name stand -- --stand {clients}{net}"


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
    gameplay = [c for c in code if c not in TOOLING]  # the sandbox's own code is what its replays play
    template = [f for f in files if f.startswith("template/")]
    sandbox_content = [f for f in files if f.startswith("crates/sandbox/assets/")]
    template_content = [f for f in files if f.startswith("template/assets/")]
    scenarios = [f for f in files if f.endswith(".scenario") and (f.startswith("crates/sandbox/scenarios/") or f.startswith("template/scenarios/"))]
    single = [f for f in scenarios if f.count("/") == 3 and f.startswith("crates/")]
    stands = [f for f in scenarios if "/stand/" in f]
    regress = [f for f in scenarios if "/regress/" in f]
    levels = [f for f in files if which_replays.is_level_file(f)]
    print(f"changed against {base[:12]}: {len(files)} files" + (f"; crates: {', '.join(crates)}" if crates else "")
          + ("; template" if template else ""))

    read, fast, regen, world, replays, gate, docs, notes = [], [], [], [], [], [], [], []

    # --- rules to read first
    topics = sorted({t for c in crates for t in CRATES.get(c, ("", ""))[1].split() if t})
    read += [f"docs/agents/{t}" for t in topics]
    if gameplay and "determinism.md" not in topics:
        read.append("docs/agents/determinism.md (simulation code changed)")
    if [c for c in crates if c not in CRATES]:
        notes.append(f"crates this script does not know ({', '.join(c for c in crates if c not in CRATES)}): add them to CRATES in plan.py")

    # --- fast checks
    for c in crates:
        if c != "sandbox":
            fast.append(f"cargo test -p {c}")
        fast.append(f"cargo clippy -p {c} --all-targets -- -D warnings")
    sandbox_game = ("crates/sandbox/src/", "crates/sandbox/assets/", "crates/sandbox/scenarios/", "crates/sandbox/tests/sandbox/")
    if gameplay or any(f.startswith(sandbox_game) for f in files):
        fast.append("cargo test -p sandbox --test sandbox      # end to end: scenarios both ways, reference replays, engine rules, state audit")
    if any(f.startswith("crates/sandbox/src/") for f in files):
        fast.append("cargo test -p sandbox                     # all of the sandbox's test binaries")
    agents_index = "cargo test -p sandbox --test agents_index  # AGENTS.md, docs/agents and the skills agree"
    for f in files:
        parts = f.split("/")
        if len(parts) == 4 and parts[:3] == ["crates", "sandbox", "tests"] and parts[3].endswith(".rs"):
            fast.append(agents_index if parts[3] == "agents_index.rs" else f"cargo test -p sandbox --test {parts[3][:-3]}")
    if any(any_match(f, AGENT_DOCS) for f in files):
        fast.append(agents_index)
    if single:
        fast.append(f"each changed scenario against one live game, twice in a row: {LIVE} start, then "
                    "cargo shards --port-file target/live/sandbox.port scenario <file>")
    for game, line in sorted({("template" if f.startswith("template/") else "sandbox", stand_line(root, f) or "?") for f in stands}):
        files_of = [f for f in stands if f.startswith("template/") == (game == "template") and (stand_line(root, f) or "?") == line]
        if line == "?":
            notes.append(f"stand scenarios without a `stand` line (CI refuses them): {', '.join(files_of)}")
            continue
        fast.append(f"{game} stand `{line}`: {stand_command(line, game)}, then `scenario` with the drive line it prints, "
                    f"for {' '.join(files_of)}   # a stand of other clients or links refuses the file")
    if regress:
        fast.append("cargo test -p sandbox --test sandbox scenario_files::every_regression   # each on a fresh game of its own seed")
    if template:
        fast.append("cargo test --manifest-path template/Cargo.toml")

    # --- regenerate
    nav = [f for f in files if any_match(f, NAV_SOURCES)]
    if nav:
        regen.append("cargo sandbox --bake-nav   # tiles, level files, nav settings, a body class's locomotion or the bake code changed; commit assets/nav/")
        if any(f.startswith(NAV_CODE) for f in files):
            notes.append("the bake code changed: if what a tile bakes to changed, raise BAKE_VERSION and bake everything, "
                         "the template's too (docs/agents/world-regions-nav.md)")
        if any(f.startswith("template/") or f.startswith(NAV_CODE) for f in nav):
            regen.append("(cd template && cargo run -- --bake-nav)")
        if any(f.endswith(".locomotion.ron") for f in nav):
            notes.append("a *.locomotion.ron changed: re-bake only if a nav body class (`bodies` in *.nav.ron) uses it")
    if any(any_match(f, MOTION_SOURCES) for f in files):
        regen.append("cargo sandbox --bake-motion   # an animset or a clip changed: re-bake the displacement curves")
        if any(f.startswith("template/") for f in files if any_match(f, MOTION_SOURCES)):
            regen.append("(cd template && cargo run -- --bake-motion)")

    # --- content and world
    if sandbox_content or gameplay or any(f.startswith("crates/shards_content/") for f in files):
        world.append("cargo sandbox --check-content")
    if template_content or any(f.startswith("template/src/") for f in files):
        world.append("(cd template && cargo run -- --check-content)")
    if any(any_match(f, WORLDGEN_SOURCES) for f in files):
        world.append("cargo sandbox --worldgen-report wilds --seeds 1..1000")
        world.append("cargo sandbox --worldgen-report voxel_arena --seeds 1..1000")
        if template or any(f.startswith("crates/shards_world/src/") for f in files):
            world.append("(cd template && cargo run -- --worldgen-report valley --seeds 1..1000)")
        notes.append("the generator's output changed? raise the region's `version` and update the manifest hash its scenario expects")
    if any(any_match(f, NET_SOURCES) for f in files):
        world.append("co-op: the stand scenarios run in-process with the sandbox tests; live, CI starts one stand per distinct "
                     "`stand` line of crates/sandbox/scenarios/stand/ and soaks one: "
                     f"{stand_command('clients=2 latency=150 loss=0.02')}, then "
                     "cargo shards --port-file target/live/stand.port soak --minutes 3")

    # --- reference replays, each game its own
    recorded = recorded_scenarios(root)
    for s in scenarios:
        if s in recorded:
            replays.append(f"{s} records {recorded[s]}: record it again (and only it should change)")
    if sandbox_content or template_content or levels:
        replays.append("content or a level file changed: python3 .agents/skills/shards-verify/scripts/which_replays.py names the replays that read it")
    if gameplay:
        replays.append(f"simulation code changed ({', '.join(gameplay)}): the reference replays must verify unchanged unless "
                       "the change is meant to change gameplay; a divergence you did not predict is a bug (shards-debug)")
    if gameplay or sandbox_content or [f for f in levels if f.startswith("crates/")] or [s for s in recorded if s in scenarios and s.startswith("crates/")]:
        replays.append("sandbox: cargo test -p sandbox --test sandbox reference_replays")
    if [c for c in gameplay if c != "sandbox"] or template:
        replays.append("template: cargo test --manifest-path template/Cargo.toml --test game replays")

    # --- the gate
    engine = bool(crates) or any(f in ("Cargo.toml", "Cargo.lock") or f.startswith((".cargo/", ".config/", "clippy.toml")) for f in files)
    if engine:
        gate += ["cargo test --workspace     # CI: cargo nextest run --workspace --profile ci -E 'not binary_id(sandbox::perf)'",
                 "cargo clippy --workspace --all-targets -- -D warnings"]
    if gameplay or sandbox_content:
        # CI's headless-cli job runs these on every pull request, whatever it touches.
        gate += ["cargo sandbox --check-content",
                 "cargo sandbox --worldgen-report wilds --seeds 1..1000 && cargo sandbox --worldgen-report voxel_arena --seeds 1..1000"]
    if template or [c for c in crates if c not in ("sandbox", "shards_cli")]:
        gate += ["cargo test --manifest-path template/Cargo.toml",
                 "cargo clippy --manifest-path template/Cargo.toml --all-targets -- -D warnings",
                 "(cd template && cargo run -- --check-content && cargo run -- --worldgen-report valley --seeds 1..1000 "
                 "&& for r in tests/replays/*.json; do cargo run -- --replay \"$r\" --verify || exit 1; done)"]
        notes.append("on Linux and macOS, `CARGO_TARGET_DIR=$PWD/target` before a template command (from the root; "
                     "`../target` inside template/) reuses the engine's build, as CI does; not on Windows (another linker there)")
    if any(f.startswith(TOOLING_PATHS) for f in files) or single:
        gate.append("live, as CI's headless-cli: every crates/sandbox/scenarios/*.scenario against one live game, then again in reverse order")
    if any(f.endswith(("Cargo.toml", "Cargo.lock")) for f in files):
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

    # --- what the added code registers, and what each obliges
    added = added_rust(root, base, files)
    tools = registered(added, "add_ai_tool", "add_ai_query", "register_tool")
    if tools:
        fast.append("cargo test -p sandbox --test sandbox tool_contracts::   # calls every tool with 30 argument sets")
        notes.append(f"new tools ({', '.join(tools)}): describe each in the catalogue of docs/agents/watching-and-driving.md "
                     "(a combat tool in docs/agents/combat.md too); its errors say how to fix the call; docs/agents/tools-and-invariants.md")
    conditions = registered(added, "add_ai_condition")
    if conditions:
        notes.append(f"new conditions ({', '.join(conditions)}): list them among the `named:` conditions in docs/agents/watching-and-driving.md")
    rules = registered(added, "add_engine_invariant", "add_opt_in_engine_invariant")
    if rules:
        fast.append("cargo test -p sandbox --test sandbox -- invariants:: engine_rules::")
        notes.append(f"new engine rules ({', '.join(rules)}): a control test each; their names in ENGINE_RULES of "
                     "crates/sandbox/tests/sandbox/invariants.rs and the rule count there")
    if registered(added, "add_ai_invariant"):
        notes.append("a new game rule (add_ai_invariant): a control test that breaks it on purpose")
    if any("impl ContentDef for" in line for _, line in added):
        notes.append("a new content kind: `validate` with refs.require for its references, a typo test, "
                     "`const SIMULATION: bool = false` if only the picture reads it (docs/agents/content.md)")
    if any(re.search(r"add_snapshot_resource|register_type::<|#\[reflect\(Component", line) for _, line in added):
        fast.append("cargo test -p sandbox --test sandbox -- state_audit:: snapshots::   # new state: the audit and snapshot round trips")

    # Every diff, last: the review of what no command checks.
    gate.append("python3 .agents/skills/shards-review/scripts/red_flags.py   # then read the diff with .agents/skills/shards-review/SKILL.md")

    # --- docs owed
    def owed(path, what):
        docs.append(f"[{'done' if path in files else 'todo'}] {path}: {what}")

    if code:
        owed("CHANGELOG.md", "an entry under Unreleased (Added, Fixed or Changed)")
        sections = "; ".join(CRATES.get(c, ("?", ""))[0] for c in code)
        owed("docs/manual.md", f"the section(s) of what changed: {sections}")
        owed("docs/engine-progress.md", "the status row of the module or slice, and the crate's test count in 'Tests'")
        notes.append("a new tool or `named:` condition joins the catalogue in docs/agents/watching-and-driving.md; "
                     "docs/agents/<topic>.md if agents must now follow a new rule; README.md only if features or the quick start changed")
    if any(f.startswith(("template/src/", "template/assets/", "template/scenarios/", "template/tests/")) for f in files):
        owed("template/README.md", "the table of files, the section of what you changed, the test count")

    for title, items in [("Read first", read), ("Fast checks, while you work", fast), ("Regenerate, then commit what they write", regen),
                         ("Content, world and co-op", world), ("Reference replays", replays), ("Before you push (CI's gate for this diff)", gate),
                         ("Docs this change owes", docs), ("Notes", notes)]:
        if items:
            print(f"\n{title}:")
            for item in dict.fromkeys(items):  # each once, in order
                print(f"  {item}")


if __name__ == "__main__":
    main()
