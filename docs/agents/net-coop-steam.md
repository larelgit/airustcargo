# Co-op, stands and Steam

Read this when you run a co-op stand (`--stand`, `--net-sim`, `--peer`) or touch `shards_steam`, lobbies, relay, achievements or the cloud. Part of the [AGENTS.md](../../AGENTS.md) index.

## Stands without windows

- Co-op without windows, in one command: `cargo sandbox --stand 2 --headless --paused --port 0 --port-file F --net-sim
  latency=150,loss=0.02 &` — a host and two clients, each with its own port and token (`F`, `F.c1`, `F.c2`). Then `export
  SHARDS_PORT_FILE=F`, and: `cargo shards wait --peer all` waits for all of them (and fails at once with the reason if the game is not a stand, the host has ended the stand, or the token is wrong; a plain `shards wait` behaves the same way), `cargo shards --peer c1 observe` looks through the client's eyes (the client's token is `SHARDS_TOKEN_C1` or its `.shards/<port>.token`; `SHARDS_TOKEN` is the host's token), `cargo shards stand` shows the participants, `cargo shards stand stop` stops the stand. The stand's scenarios (`stand clients=2 …`, commands with `@c1`) are in
  `crates/sandbox/scenarios/stand/`: `shards scenario crates/sandbox/scenarios/stand/*.scenario`. `--stand 2 --stand-udp` —
  the same over real UDP, with clients as processes, in real time only; a client that did not join the session or that crashed
  stops the stand (a client that left on its own, or one the host cut off with `net_kick`, does not), and `--stand N` must be no more than
  `max_players - 1`. `--stand N --stand-steam [--steam-lobby TYPE]` (a game with the `steam` feature; the `cargo sandbox` alias passes the flags to the game, so the feature
  is set by `cargo run`: `cargo run -p sandbox --features shards/steam -- --stand 1 --stand-steam --steam-lobby invisible --headless
  --paused --port 0 --port-file F &`) — the stand runs in one process, but
  the participants meet through a fake Steam, not through the in-memory network: the host makes a lobby (by default `friends`), the clients wait for
  the lobby (`steam_lobbies` searches, `steam_join` joins; `wait --peer all` waits for the clients to join, so here wait for the host: `cargo shards
  wait`); the stand does not touch the real Steam. The scenario for such a stand is
  `crates/shards_steam/scenarios/lobby_search.scenario`.

## Steam behind the engine API

- **Steam sits behind the engine's API** (`shards_steam`, the `steam` feature; `steam_sdk` is the real client). There are no `steamworks` types in
  the public API, and nothing that affects gameplay depends on Steam: a relay session is the same as over UDP, and achievements and the cloud
  are not simulation state (they are not part of the replay hash or the snapshot; nothing is unlocked during a replay). An achievement is unlocked
  by the host (`UnlockAchievement`); a client unlocks one only on the host's event. Tests do not start the real Steam: only `FakeSteam`; the file
  `steam_appid.txt` is not created (`--steam-app N` is used instead). A Steam callback whose parsing in `steamworks` may fail on a value
  from Steam is read by hand, as numbers (`impl Callback` in `real.rs`): a panic in a callback stops all Steam callbacks until the end of the run.

## Join gates

`NetHooks::join_gates` is a list of gates, `fn(&World, &JoinRequest) -> Option<String>`; `default_join_policy` asks them in order and a reason makes the newcomer wait (`JoinDecision::Wait`). A request whose player already has a body (a rejoin within the grace) skips them. A game with a `join_policy` of its own calls `shards::net::gates_say` from it. Modules push their gate in their umbrella wiring (the encounter module's holds joiners while a `blocks_join` fight is on, `shards::encounter::join_blocked`). A gate reads simulation state only and changes nothing.
