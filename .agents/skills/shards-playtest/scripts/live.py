#!/usr/bin/env python3
"""Start, list and stop live games to drive with the `shards` CLI, without the usual traps.

    python3 .agents/skills/shards-playtest/scripts/live.py start                     # the sandbox, headless and paused
    python3 .agents/skills/shards-playtest/scripts/live.py start --game template     # the template game
    python3 .agents/skills/shards-playtest/scripts/live.py start --name duel -- --seed 7 --coverage
    python3 .agents/skills/shards-playtest/scripts/live.py start --name coop -- --stand 2 --net-sim latency=150,loss=0.02
    python3 .agents/skills/shards-playtest/scripts/live.py start --offscreen         # the GPU draws, for `shards shot`
    python3 .agents/skills/shards-playtest/scripts/live.py status
    python3 .agents/skills/shards-playtest/scripts/live.py stop [NAME | --all]

`start` builds first (a game still compiling makes `shards wait` give up), starts the game in the
background with `--port 0 --port-file target/live/<name>.port`, writes its output to
target/live/<name>.log, waits until it has loaded, and prints the command to drive it with. Everything
after `--` goes to the game (`--seed`, `--stand`, `--net-sim`, `--coverage`, `--check-every 1`,
`--replay FILE`, `--menu`, `--heavy N`, ...). `stop` ends a game the way Ctrl+C does (the `quit` tool,
then SIGTERM), never SIGKILL, which would leave its token and port file behind.
"""

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

GAMES = {
    # name: (working folder from the repository root, build command, run command)
    "sandbox": (".", ["cargo", "build", "-p", "sandbox"], ["cargo", "run", "-p", "sandbox"]),
    "template": ("template", ["cargo", "build"], ["cargo", "run"]),
}
WINDOWS = os.name == "nt"


def repo_root():
    out = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit("run this inside the engine repository")
    return Path(out.stdout.strip())


def state_dir(root):
    folder = root / "target" / "live"
    folder.mkdir(parents=True, exist_ok=True)
    return folder


def cli_binary(root):
    target = Path(os.environ.get("CARGO_TARGET_DIR") or root / "target")
    return target / "debug" / ("shards.exe" if WINDOWS else "shards")


def shards(root, game_cwd, port_file, *args, timeout=None):
    """Runs the CLI from the game's folder, where its token is (`.shards/<port>.token`)."""
    cmd = [str(cli_binary(root)), "--port-file", str(port_file), *args]
    try:
        return subprocess.run(cmd, cwd=game_cwd, capture_output=True, encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(cmd, 124, "", f"timed out after {timeout}s")


def read_state(folder, name):
    path = folder / f"{name}.json"
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


def pid_alive(pid):
    if WINDOWS:
        out = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/NH"], capture_output=True, text=True)
        return str(pid) in out.stdout
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def ours(state):
    """Whether the state's pid is still its game: alive, and (where `ps` can tell) started with its port file, so
    a pid the system gave to another process since is never signalled. None: alive, but it cannot be told."""
    pid = state["pid"]
    if not pid_alive(pid):
        return False
    if WINDOWS:
        return None
    out = subprocess.run(["ps", "-p", str(pid), "-o", "args="], capture_output=True, encoding="utf-8", errors="replace")
    if out.returncode != 0:
        return None
    return state["port_file"] in out.stdout


def tail(path, lines=25):
    try:
        return "\n".join(Path(path).read_text(encoding="utf-8", errors="replace").splitlines()[-lines:])
    except OSError:
        return "(no log)"


def drive_hint(root, state, command):
    """The line an agent types, from the repository root, to run `command` against this game. The CLI finds a
    game's tokens (its own and its clients', `.shards/<port>.token`) from the folder it runs in, so a game that
    runs in another folder (the template) is driven from there, with the CLI binary."""
    if state["game"] == "sandbox":
        return f"cargo shards --port-file {Path(state['port_file']).relative_to(root).as_posix()} {command}"
    cwd = Path(state["cwd"])
    cli = Path(os.path.relpath(cli_binary(root), cwd)).as_posix()
    port_file = Path(os.path.relpath(state["port_file"], cwd)).as_posix()
    return f"(cd {cwd.relative_to(root).as_posix()} && {cli} --port-file {port_file} {command})"


def start(root, args):
    folder = state_dir(root)
    name = args.name or args.game
    old = read_state(folder, name)
    if old and ours(old) is not False:
        sys.exit(f"a game named {name} is running (pid {old['pid']}): stop it first (live.py stop {name}) or pick another --name")
    workdir, build, run = GAMES[args.game]
    game_cwd = (root / workdir).resolve()
    release = ["--release"] if args.release else []

    # The template is a workspace of its own: it shares the engine's target folder (as CI does) rather than
    # building Bevy a second time into its own. Not on Windows, where the engine and the template pick other
    # linkers, and one folder would rebuild everything at each switch. The CLI and every cargo call see it.
    if not WINDOWS:
        os.environ.setdefault("CARGO_TARGET_DIR", str(root / "target"))
    if not args.no_build:
        print(f"building {args.game} and the shards CLI (a cold build takes 10-30 minutes) ...", flush=True)
        for cmd, cwd in [(build + release, game_cwd), (["cargo", "build", "-p", "shards_cli"], root)]:
            if subprocess.run(cmd, cwd=cwd).returncode != 0:
                sys.exit(f"build failed: {' '.join(cmd)} (in {cwd})")

    port_file = folder / f"{name}.port"
    for stale in [port_file, *folder.glob(f"{name}.port.c*")]:
        stale.unlink(missing_ok=True)
    log = folder / f"{name}.log"
    flags = ["--port", "0", "--port-file", str(port_file)]
    flags += ["--offscreen"] if args.offscreen else ["--headless"]
    flags += [] if args.realtime else ["--paused"]
    cmd = run + release + ["--", *flags, *args.game_args]
    detach = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS} if WINDOWS else {"start_new_session": True}
    with open(log, "w") as out:
        process = subprocess.Popen(cmd, cwd=game_cwd, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, **detach)
    state = {"name": name, "game": args.game, "pid": process.pid, "port_file": str(port_file), "log": str(log),
             "cwd": str(game_cwd), "command": cmd, "started": time.strftime("%Y-%m-%d %H:%M:%S")}
    (folder / f"{name}.json").write_text(json.dumps(state, indent=2), encoding="utf-8")

    stand = "--stand" in args.game_args
    waited = shards(root, game_cwd, port_file, *(["--peer", "all"] if stand else []), "wait", "--timeout", str(args.timeout),
                    timeout=args.timeout + 30)
    if waited.returncode != 0:
        if process.poll() is not None:
            (folder / f"{name}.json").unlink(missing_ok=True)
        sys.exit(f"the game did not come up (exit {process.poll()}):\n{waited.stdout}{waited.stderr}\n--- {log}:\n{tail(log)}")
    print(waited.stdout.strip())
    print(f"\n{name}: {args.game}, pid {process.pid}, log {log.relative_to(root).as_posix()}")
    print(f"drive:  {drive_hint(root, state, 'observe')}      (then: tools, act TOOL k=v, step N --until COND, inspect eN)")
    if stand:
        print(f"        {drive_hint(root, state, '--peer c1 observe')}      (a client's eyes; `stand` lists the members)")
    print(f"stop:   python3 .agents/skills/shards-playtest/scripts/live.py stop {name}")


def stop_one(root, folder, name):
    state = read_state(folder, name)
    if not state:
        print(f"{name}: no such live game")
        return False
    pid, cwd, port_file = state["pid"], state["cwd"], state["port_file"]
    mine = ours(state)
    if mine is not False:
        quit_ = shards(root, cwd, port_file, "--timeout", "15", "quit", timeout=30)
        if quit_.returncode == 0:
            print(f"{name}: {quit_.stdout.strip() or 'ended'}")
        elif mine:
            # The game did not answer: SIGTERM runs the same exit hooks (never SIGKILL). Only to a pid that is
            # still this game (`ours`): a pid can be given to another process after the game is gone.
            os.kill(pid, signal.SIGTERM)
            for _ in range(100):
                if not pid_alive(pid):
                    break
                time.sleep(0.1)
            print(f"{name}: {'ended by SIGTERM' if not pid_alive(pid) else f'still running (pid {pid}); look at its log: ' + state['log']}")
        else:
            print(f"{name}: `quit` failed ({quit_.stderr.strip()}); if pid {pid} is still the game, end it by hand (not with SIGKILL)")
    else:
        print(f"{name}: not running (pid {pid} is gone or is another process now)")
    (folder / f"{name}.json").unlink(missing_ok=True)
    return True


def stop(root, args):
    folder = state_dir(root)
    names = [p.stem for p in sorted(folder.glob("*.json"))] if args.all else [args.name or "sandbox"]
    if not names:
        print("no live games")
    ok = all([stop_one(root, folder, name) for name in names])
    sys.exit(0 if ok else 1)


def status(root, _args):
    folder = state_dir(root)
    states = [json.loads(p.read_text(encoding="utf-8")) for p in sorted(folder.glob("*.json"))]
    if not states:
        print("no live games")
        return
    for state in states:
        alive = ours(state) is not False
        line = "not running" if not alive else (shards(root, state["cwd"], state["port_file"], "status", timeout=10).stdout.strip().splitlines() or ["?"])[0]
        print(f"{state['name']}: {state['game']}, pid {state['pid']}, started {state['started']}: {line}")
        if alive:
            print(f"  drive: {drive_hint(root, state, 'observe')}")
        print(f"  log:   {Path(state['log']).relative_to(root).as_posix()}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0], formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    s = sub.add_parser("start", help="build, start a game in the background and wait until it has loaded")
    s.add_argument("--game", choices=sorted(GAMES), default="sandbox")
    s.add_argument("--name", help="a name for this game (default: the game's): several can run at once")
    s.add_argument("--offscreen", action="store_true", help="draw with the GPU without a window (for `shards shot`), instead of --headless")
    s.add_argument("--realtime", action="store_true", help="do not pass --paused: time runs on its own")
    s.add_argument("--release", action="store_true", help="a release build (tick-time measurements)")
    s.add_argument("--no-build", action="store_true", help="skip the build (it must be up to date)")
    s.add_argument("--timeout", type=int, default=180, help="seconds to wait for the game to load (default 180)")
    s.add_argument("game_args", nargs="*", help="after --: arguments for the game")
    p = sub.add_parser("stop", help="end a live game (default: sandbox)")
    p.add_argument("name", nargs="?")
    p.add_argument("--all", action="store_true")
    sub.add_parser("status", help="the live games, whether they answer, and how to drive them")
    args = parser.parse_args()
    root = repo_root()
    {"start": start, "stop": stop, "status": status}[args.command](root, args)


if __name__ == "__main__":
    main()
