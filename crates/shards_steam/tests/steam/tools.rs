//! What the tools say: `steam` on a single game, a lobby host and its client, a game server and its
//! client; `steam_host`, `steam_join` and `steam_invite` where they do nothing; `ai/status`.

use serde_json::json;
use shards::ai_core::status;
use shards::net::{LinkConditions, MemoryHub};
use shards::prelude::*;
use shards_steam::fake::FIRST_USER;
use shards_steam::FakeSteam;

use crate::*;

#[test]
fn steam_on_a_single_game_says_it_is_a_fake_and_in_no_session() {
    let (mut app, _) = single();
    let said = ok(&mut app, "steam", json!({}));
    println!("steam (single game):\n{said}");
    assert_eq!(
        said,
        format!(
            "steam: a fake Steam in this process (this run does not talk to Steam) · user player ({FIRST_USER})\n\
             session: none over Steam (start the game with --steam-host, --steam-server, or --join steam:ID)\n\
             relay: ready (fake: an in-process hub)\n\
             achievements: 2 defined, 0 unlocked in this run · stored on Steam 0 times\n\
             cloud: on · 0 saves copied as they were written"
        )
    );
    let not_host = err(&mut app, "steam_host", json!({}));
    assert!(not_host.starts_with("this game was not started as a Steam host: start it with --steam-host"), "{not_host}");
    let not_client = err(&mut app, "steam_join", json!({"lobby": "109775240917319681"}));
    assert!(not_client.starts_with("this game was not started as a Steam client"), "{not_client}");
    assert_eq!(err(&mut app, "steam_join", json!({})), "name one: lobby=ID (a lobby) or host=ID (a game server's or player's Steam id)");
    assert!(err(&mut app, "steam_join", json!({"lobby": "abc"})).starts_with("lobby: give the Steam id"));
    let no_lobby = err(&mut app, "steam_invite", json!({}));
    assert!(no_lobby.starts_with("this game is in no Steam lobby"), "{no_lobby}");

    let s = status::status(app.world())["steam"].clone();
    assert_eq!(s["kind"], "fake");
    assert_eq!(s["user"], json!(FIRST_USER.to_string()));
    assert_eq!(s["role"], "none");
}

#[test]
fn steam_on_a_lobby_host_and_its_client_shows_the_lobby_the_relay_and_the_routes() {
    let steam = FakeSteam::new(MemoryHub::new(9));
    steam.set_conditions(LinkConditions { latency_ms: 20, ..LinkConditions::NONE });
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);
    let lobby = session(stand.world(0)).lobby_id().unwrap();
    let app = build(config(|c| client(c, "c1", Some(lobby))), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = add_client(&mut stand, "c1", app);
    stand.run(30);
    let out = stand.call(0, "achievement_unlock", json!({"id": "first_blood"}));
    assert!(out.ok, "{}", out.result);
    stand.run(5);

    let host_said = text(&stand.call(0, "steam", json!({})));
    println!("steam (lobby host):\n{host_said}");
    let lines: Vec<&str> = host_said.lines().collect();
    assert_eq!(lines[0], format!("steam: a fake Steam in this process (this run does not talk to Steam) · user host ({FIRST_USER})"));
    assert_eq!(lines[1], format!("session: host · lobby {lobby}, host host ({FIRST_USER}) · 2 in it: host ({FIRST_USER}), c1 ({SECOND_USER})"));
    assert!(lines[2].starts_with("relay: ready (fake: an in-process hub) · sent "), "{}", lines[2]);
    assert_eq!(lines[3], format!("route to c1 ({SECOND_USER}): connected · relayed via fake · ping 40 ms"));
    assert!(lines[4].starts_with("achievements: 2 defined, 1 unlocked in this run: first_blood (First blood) at tick "), "{}", lines[4]);

    let client_said = text(&stand.call(c1, "steam", json!({})));
    println!("steam (client c1):\n{client_said}");
    assert!(client_said.contains(&format!("session: client · lobby {lobby}, host host ({FIRST_USER})")), "{client_said}");
    assert!(client_said.contains(&format!("route to host ({FIRST_USER}): connected · relayed via fake · ping 40 ms")), "{client_said}");
    assert!(client_said.contains("achievements: 2 defined, 1 unlocked in this run: first_blood (First blood)"), "{client_said}");

    let joined = stand.call(c1, "steam_join", json!({"lobby": lobby}));
    assert_eq!(text(&joined), format!("already in lobby {lobby}: a running session does not change lobbies"));
    let again = stand.call(0, "steam_host", json!({}));
    assert_eq!(text(&again), format!("hosting in lobby {lobby} (2 in it): invite friends with steam_invite"));

    let s = status::status(stand.world(0))["steam"].clone();
    assert_eq!((s["role"].as_str(), s["lobby"].clone()), (Some("host"), json!(lobby.to_string())));
    let route = &s["relay"]["routes"][0];
    assert_eq!((route["state"].as_str(), route["relayed"].as_bool(), route["pop"].as_str(), route["ping_ms"].as_i64()), (Some("connected"), Some(true), Some("fake"), Some(40)));
}

#[test]
fn steam_on_a_game_server_shows_its_id_and_what_it_has_not() {
    let steam = FakeSteam::new(MemoryHub::new(4));
    let server = build(
        config(|c| {
            game_server(c);
            c.steam.relay_only = true;
        }),
        steam.server(SERVER, "server"),
        save_folder(),
    );
    let mut stand = stand_with_host(&steam, server);
    let app = build(config(|c| client(c, "c1", Some(SERVER))), steam.user(SECOND_USER, "c1"), save_folder());
    add_client(&mut stand, "c1", app);
    stand.run(10);

    let said = text(&stand.call(0, "steam", json!({})));
    println!("steam (game server):\n{said}");
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines[0], format!("steam: a fake Steam game server in this process · id {SERVER}"));
    assert_eq!(lines[1], format!("session: game server · serving as {SERVER}: players join with --join steam:{SERVER}"));
    assert!(lines[2].starts_with("relay: ready (fake: an in-process hub) · relay only (direct routes off) · sent "), "{}", lines[2]);
    assert_eq!(lines[3], format!("route to c1 ({SECOND_USER}): connected · relayed via fake · ping 0 ms"));
    assert!(said.contains("cloud: off (an anonymous game server has no lobbies"), "{said}");

    assert_eq!(text(&stand.call(0, "steam_host", json!({}))), format!("serving as game server {SERVER}: players join with --join steam:{SERVER}"));
    let invite = stand.call(0, "steam_invite", json!({"friend": SECOND_USER}));
    assert!(!invite.ok && text(&invite).starts_with("a game server has no lobby to invite into"), "{}", text(&invite));
    let unlock = stand.call(0, "achievement_unlock", json!({"id": "first_blood", "player": 0}));
    assert!(text(&unlock).contains("in this run only: an anonymous game server has no"), "{}", text(&unlock));
    assert_eq!(status::status(stand.world(0))["steam"]["role"], "server");
}
