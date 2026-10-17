//! Sessions over the fake relay: a lobby host and two friends, a game server and a player who joins
//! it by its id, an invite followed, sessions refused, and joins that fail.

use shards::ai_core::journal::JournalKind;
use shards::net::{ClientStage, LinkConditions, MemoryHub, NetClient, NetHost, NetState};
use shards::prelude::*;
use shards_steam::fake::{FIRST_USER, LOBBY_BASE};
use shards_steam::{FakeSteam, SteamRole, SteamState};
use shards::core::SteamLobby;

use crate::*;

fn connected(world: &World) -> Option<u8> {
    match world.resource::<NetState>() {
        NetState::Connected { player, .. } => Some(*player),
        _ => None,
    }
}

#[test]
fn a_lobby_host_and_two_friends_play_over_the_relay() {
    let steam = FakeSteam::new(MemoryHub::new(7));
    steam.set_conditions(LinkConditions { latency_ms: 20, ..LinkConditions::NONE });
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);

    let s = session(stand.world(0));
    assert_eq!(s.role, SteamRole::Host);
    let lobby = s.lobby_id().unwrap_or_else(|| panic!("the host made no lobby: {:?}", s.state));
    assert_eq!(steam.lobbies(), [lobby]);
    assert_eq!(lobby, LOBBY_BASE + 1);
    assert_eq!(s.host_id(), Some(FIRST_USER));
    assert_eq!(steam.lobby_data(lobby, "host").as_deref(), Some("host"));
    assert_eq!(steam.lobby_data(lobby, "engine").as_deref(), Some(shards_steam::VERSION));
    assert_eq!(stand.world(0).resource::<NetState>(), &NetState::Hosting);
    assert!(journaled(stand.world(0), &format!("steam: hosting in lobby {lobby}")));

    for (n, (name, user)) in [("c1", SECOND_USER), ("c2", THIRD_USER)].into_iter().enumerate() {
        let app = build(config(|c| client(c, name, Some(lobby))), steam.user(user, name), save_folder());
        let member = add_client(&mut stand, name, app);
        let world = stand.world(member);
        assert_eq!(connected(world), Some(n as u8 + 1), "{name}: {:?}", world.resource::<NetState>());
        assert_eq!(session(world).state, SteamState::InLobby { lobby, owner: FIRST_USER });
        assert_eq!(world.resource::<NetClient>().address, format!("steam:{FIRST_USER}"), "the token is kept per host, not per lobby");
        assert!(journaled(world, &format!("steam: entered lobby {lobby}; its host is host ({FIRST_USER})")));
    }
    stand.run(60);

    assert_eq!(steam.lobby_members(lobby), [FIRST_USER, SECOND_USER, THIRD_USER]);
    let host = stand.world(0);
    assert_eq!(host.resource::<NetHost>().peers.len(), 2);
    assert_eq!(session(host).members.len(), 3, "the host follows its lobby");
    assert!(journaled(host, &format!("steam: c1 ({SECOND_USER}) entered lobby {lobby}")));
    let relay = session(host).relay_stats().expect("the host's relay is open");
    assert!(relay.sent > 0 && relay.received > 0, "{relay:?}");
    assert_eq!(relay.peers.iter().copied().collect::<Vec<_>>(), [SECOND_USER, THIRD_USER]);
    assert_eq!(relay.errors, 0);
    // Both clients play: their ticks follow the host's.
    for member in [1, 2] {
        let tick = shards::ai_core::sim::tick(stand.world(member));
        assert!(tick > 0, "{} did not tick", stand.members[member].name);
    }
}

#[test]
fn a_game_server_logs_on_and_a_player_joins_it_by_its_id() {
    let steam = FakeSteam::new(MemoryHub::new(3));
    let server = build(config(game_server), steam.server(SERVER, "server"), save_folder());
    let mut stand = stand_with_host(&steam, server);
    let s = session(stand.world(0));
    assert_eq!((s.role, s.state.clone()), (SteamRole::Server, SteamState::Serving { id: SERVER }));
    assert_eq!(s.host_id(), Some(SERVER));
    assert!(steam.lobbies().is_empty(), "a game server makes no lobby");
    assert!(journaled(stand.world(0), &format!("steam: game server logged on as {SERVER}: players join with --join steam:{SERVER}")));

    let app = build(config(|c| client(c, "c1", Some(SERVER))), steam.user(SECOND_USER, "c1"), save_folder());
    let member = add_client(&mut stand, "c1", app);
    stand.run(30);
    let world = stand.world(member);
    assert_eq!(connected(world), Some(1), "{:?}", world.resource::<NetState>());
    assert_eq!(session(world).state, SteamState::Direct { host: SERVER });
    assert_eq!(world.resource::<NetClient>().address, format!("steam:{SERVER}"));
    assert!(journaled(world, &format!("steam: joining server ({SERVER}) over the relay")));
    assert_eq!(stand.world(0).resource::<NetHost>().peers.len(), 1);
    assert_eq!(session(stand.world(0)).refused, 0, "a game server takes whoever knows its id");
}

#[test]
fn a_player_waiting_for_an_invite_follows_it_and_a_busy_one_keeps_it() {
    let steam = FakeSteam::new(MemoryHub::new(5));
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);
    let lobby = session(stand.world(0)).lobby_id().unwrap();

    // `--join steam:`: waits, then the host's invite brings it in.
    let app = build(config(|c| client(c, "c1", None)), steam.user(SECOND_USER, "c1"), save_folder());
    let member = stand.add("c1", app, None);
    stand.frame();
    assert_eq!(session(stand.world(member)).state, SteamState::WaitingForInvite);
    let out = stand.call(0, "steam_invite", serde_json::json!({ "friend": SECOND_USER }));
    assert!(out.ok, "{}", out.result);
    assert_eq!(text(&out), format!("invited c1 ({SECOND_USER}) to lobby {lobby}"));
    stand.settle().unwrap();
    let world = stand.world(member);
    assert_eq!(connected(world), Some(1), "{:?}", world.resource::<NetState>());
    assert!(journaled(world, &format!("steam: host ({FIRST_USER}) invites you to lobby {lobby}: joining")));

    // A game in a session does not change it: the invite is kept and said.
    steam.request_join(SECOND_USER, LOBBY_BASE + 9, THIRD_USER);
    stand.frame();
    let s = session(stand.world(member));
    assert_eq!(s.invites, [(LOBBY_BASE + 9, THIRD_USER)]);
    assert_eq!(s.lobby_id(), Some(lobby));
}

#[test]
fn relay_sessions_come_only_from_the_lobby_and_from_the_host() {
    let steam = FakeSteam::new(MemoryHub::new(11));
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);
    let lobby = session(stand.world(0)).lobby_id().unwrap();
    let app = build(config(|c| client(c, "c1", Some(lobby))), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = add_client(&mut stand, "c1", app);

    // A stranger who knows the host's id, then c1's, but is in no lobby.
    for (target, member, why) in [(FIRST_USER, 0, format!("not in lobby {lobby}")), (SECOND_USER, c1, format!("not this game's host {FIRST_USER}"))] {
        let intruder = build(config(|c| client(c, "intruder", Some(target))), steam.user(THIRD_USER, "intruder"), save_folder());
        let i = stand.add("intruder", intruder, None);
        for _ in 0..20 {
            stand.tick().unwrap();
        }
        let refused = session(stand.world(member)).refused;
        assert!(refused >= 1, "{}", stand.members[member].name);
        let line = format!("steam: refused a relay session from intruder ({THIRD_USER}): {why}");
        let warned = journal(stand.world(member), Some(JournalKind::Warn)).iter().filter(|l| **l == line).count();
        assert_eq!(warned, 1, "said once, however often it asks: {:?}", journal(stand.world(member), None));
        let world = stand.world(i);
        assert_ne!(world.resource::<NetClient>().stage, ClientStage::Connected, "its hello is unanswered");
        assert!(journaled(world, &format!("the relay session with {} ({target}) failed", stand.members[member].name)));
        stand.members.pop();
    }
    assert_eq!(stand.world(0).resource::<NetHost>().peers.len(), 1, "c1 plays on");
    assert_eq!(connected(stand.world(c1)), Some(1));
}

#[test]
fn a_join_that_fails_says_why_and_waits_for_another_or_exits() {
    let steam = FakeSteam::new(MemoryHub::new(13));
    let missing = LOBBY_BASE + 42;
    let mut waiting = build(config(|c| client(c, "c1", Some(missing))), steam.user(SECOND_USER, "c1"), save_folder());
    frames_until(&mut waiting, |w| matches!(session(w).state, SteamState::Failed(_)));
    let why = format!("cannot enter Steam lobby {missing}: no such lobby (it closed, or the id is wrong)");
    assert_eq!(session(waiting.world()).state, SteamState::Failed(why.clone()));
    assert!(journal(waiting.world(), Some(JournalKind::Error)).contains(&format!("steam: {why}")));
    assert!(matches!(waiting.world().resource::<NetState>(), NetState::Joining(w) if w.contains("steam_join lobby=ID")));
    assert_eq!(waiting.world().resource::<NetClient>().stage, ClientStage::Connecting, "it can still join another");

    // `--exit-with-host`: the session could not be opened, the game ends (exit 2).
    let mut exits = build(
        config(|c| {
            client(c, "c2", Some(missing));
            c.exit_with_host = true;
        }),
        steam.user(THIRD_USER, "c2"),
        save_folder(),
    );
    frames_until(&mut exits, |w| matches!(w.resource::<NetClient>().stage, ClientStage::Ended(_)));
    assert_eq!(exits.should_exit(), Some(AppExit::from_code(2)));

    // An id that is no lobby and no host is said at the first frame.
    let mut clan = build(config(|c| client(c, "c3", Some(0x0170_0000_0000_0005))), steam.user(FIRST_USER + 3, "c3"), save_folder());
    frames_until(&mut clan, |w| matches!(session(w).state, SteamState::Failed(_)));
    assert!(matches!(&session(clan.world()).state, SteamState::Failed(why) if why.contains("account type is 7")), "{:?}", session(clan.world()).state);
}

#[test]
fn a_game_that_is_its_own_host_is_refused() {
    let steam = FakeSteam::new(MemoryHub::new(17));
    let mut app = build(config(|c| client(c, "me", Some(FIRST_USER))), steam.user(FIRST_USER, "me"), save_folder());
    frames_until(&mut app, |w| matches!(session(w).state, SteamState::Failed(_)));
    assert_eq!(session(app.world()).state, SteamState::Failed(format!("{FIRST_USER} is this machine's own Steam id")));
}

#[test]
fn a_lobby_is_friends_only_unless_the_launch_names_its_type() {
    let steam = FakeSteam::new(MemoryHub::new(41));
    let friends = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let stand = stand_with_host(&steam, friends);
    let lobby = session(stand.world(0)).lobby_id().unwrap();
    assert_eq!(steam.lobby_type(lobby), Some(SteamLobby::Friends));
    let line = format!("steam: hosting in lobby {lobby} (friends only): friends join through an invite (steam_invite), or with --join steam:{lobby}");
    assert!(journal(stand.world(0), None).contains(&line), "{:#?}", journal(stand.world(0), None));

    let private = build(
        config(|c| {
            lobby_host(c);
            c.steam.lobby = SteamLobby::Private;
        }),
        steam.user(SECOND_USER, "host2"),
        save_folder(),
    );
    let stand = stand_with_host(&steam, private);
    let lobby = session(stand.world(0)).lobby_id().unwrap();
    assert_eq!(steam.lobby_type(lobby), Some(SteamLobby::Private));
    assert_eq!(session(stand.world(0)).lobby_type, Some(SteamLobby::Private));
    let line = format!("steam: hosting in lobby {lobby} (private: invite only): players come in only through an invite (steam_invite)");
    assert!(journal(stand.world(0), None).contains(&line), "{:#?}", journal(stand.world(0), None));
}

#[test]
fn a_client_whose_join_failed_enters_another_lobby_with_steam_join() {
    let steam = FakeSteam::new(MemoryHub::new(19));
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);
    let lobby = session(stand.world(0)).lobby_id().unwrap();

    let app = build(config(|c| client(c, "c1", Some(LOBBY_BASE + 42))), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = stand.add("c1", app, None);
    for _ in 0..3 {
        stand.frame();
    }
    let state = session(stand.world(c1)).state.clone();
    assert!(matches!(&state, SteamState::Failed(why) if why.contains("no such lobby")), "{state:?}");
    let out = stand.call(c1, "steam_join", serde_json::json!({ "lobby": lobby.to_string() }));
    assert!(out.ok, "{}", out.result);
    assert_eq!(text(&out), format!("entered lobby {lobby}; its host is host ({FIRST_USER})"));
    stand.settle().unwrap();
    let world = stand.world(c1);
    assert_eq!(connected(world), Some(1), "{:?}", world.resource::<NetState>());
    assert_eq!(session(world).state, SteamState::InLobby { lobby, owner: FIRST_USER });
    assert_eq!(steam.lobby_members(lobby), [FIRST_USER, SECOND_USER]);
}

#[test]
fn a_player_waiting_for_an_invite_joins_a_game_server_with_steam_join() {
    let steam = FakeSteam::new(MemoryHub::new(23));
    let server = build(config(game_server), steam.server(SERVER, "server"), save_folder());
    let mut stand = stand_with_host(&steam, server);
    let app = build(config(|c| client(c, "c1", None)), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = stand.add("c1", app, None);
    stand.frame();
    assert_eq!(session(stand.world(c1)).state, SteamState::WaitingForInvite);
    let out = stand.call(c1, "steam_join", serde_json::json!({ "host": SERVER }));
    assert!(out.ok, "{}", out.result);
    assert_eq!(text(&out), format!("joining server ({SERVER}) over the relay"));
    stand.settle().unwrap();
    assert_eq!(connected(stand.world(c1)), Some(1), "{:?}", stand.world(c1).resource::<NetState>());
    assert_eq!(session(stand.world(c1)).state, SteamState::Direct { host: SERVER });
}

#[test]
fn a_lobby_host_whose_lobby_fails_ends_its_session_and_steam_host_tries_again() {
    let steam = FakeSteam::new(MemoryHub::new(29));
    steam.fail_lobbies(Some("Steam is busy"));
    let mut host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    frames_until(&mut host, |w| matches!(session(w).state, SteamState::Failed(_)));
    let why = "cannot host: Steam could not create a lobby: Steam is busy";
    let line = format!("steam: {why}; try again with steam_host");
    assert!(journal(host.world(), Some(JournalKind::Error)).contains(&line), "{:#?}", journal(host.world(), None));
    assert_eq!(host.world().resource::<NetState>(), &NetState::Ended(why.into()));
    assert_eq!(host.should_exit(), None, "a host without --exit-with-host stays");
    assert_eq!(err(&mut host, "steam_host", serde_json::json!({})), why);

    // Steam makes lobbies again: `steam_host` makes one, and the session is open.
    steam.fail_lobbies(None);
    let lobby = LOBBY_BASE + 1;
    assert_eq!(ok(&mut host, "steam_host", serde_json::json!({})), format!("hosting in lobby {lobby}: invite friends with steam_invite"));
    assert_eq!(host.world().resource::<NetState>(), &NetState::Hosting);
    assert_eq!(steam.lobbies(), [lobby]);

    // With --exit-with-host the game ends with code 2, as a host whose port is taken does.
    steam.fail_lobbies(Some("Steam is busy"));
    let mut exits = build(
        config(|c| {
            lobby_host(c);
            c.exit_with_host = true;
        }),
        steam.user(THIRD_USER, "host2"),
        save_folder(),
    );
    frames_until(&mut exits, |w| matches!(session(w).state, SteamState::Failed(_)));
    assert_eq!(exits.should_exit(), Some(AppExit::from_code(2)));
}

#[test]
fn a_game_server_that_does_not_log_on_says_why_and_exits_with_its_session() {
    let steam = FakeSteam::new(MemoryHub::new(31));
    let user = steam.server(SERVER, "server");
    steam.fail_log_on(SERVER, "no connection, EResult 3");
    let mut server = build(
        config(|c| {
            game_server(c);
            c.exit_with_host = true;
        }),
        user,
        save_folder(),
    );
    frames_until(&mut server, |w| matches!(session(w).state, SteamState::Failed(_)));
    let line = "steam: cannot host: the game server did not log on to Steam: no connection, EResult 3".to_owned();
    assert!(journal(server.world(), Some(JournalKind::Error)).contains(&line), "{:#?}", journal(server.world(), None));
    assert_eq!(server.should_exit(), Some(AppExit::from_code(2)));
    assert_eq!(err(&mut server, "steam_host", serde_json::json!({})), "the game server did not log on to Steam (`steam` says why): start it again");
}

#[test]
fn when_steam_s_callbacks_stop_a_host_makes_no_lobby_and_says_why() {
    let steam = FakeSteam::new(MemoryHub::new(43));
    let user = steam.user(FIRST_USER, "host");
    steam.panic_in_callbacks(FIRST_USER);
    let mut host = build(config(lobby_host), user, save_folder());
    frames_until(&mut host, |w| matches!(session(w).state, SteamState::Failed(_)));
    let state = session(host.world()).state.clone();
    assert!(matches!(&state, SteamState::Failed(why) if why.contains("a Steam callback panicked")), "{state:?}");
    let refused = err(&mut host, "steam_host", serde_json::json!({}));
    assert!(refused.starts_with("cannot host: Steam could not create a lobby: a Steam callback panicked"), "{refused}");
    assert!(steam.lobbies().is_empty());

    // A host already waiting for its lobby when they stop gets no answer: it says so instead.
    let steam = FakeSteam::new(MemoryHub::new(47));
    let mut host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    frames_until(&mut host, |w| w.get_resource::<shards::content::ContentDb>().is_some_and(|db| db.is_loaded()));
    assert_eq!(session(host.world()).state, SteamState::ToCreate, "the lobby is asked for at the next frame");
    steam.panic_in_callbacks(FIRST_USER);
    frames_until(&mut host, |w| matches!(session(w).state, SteamState::Failed(_)));
    assert_eq!(session(host.world()).state, SteamState::Failed("Steam's callbacks stopped (`steam` says why)".into()));
}

/// A game that talks to Steam (`connect`) in a build without the SDK, as the tests are: no Steam.
#[cfg(not(feature = "sdk"))]
#[test]
fn without_steam_a_host_cannot_host_and_a_client_cannot_join() {
    let unavailable = format!("Steam is unavailable ({})", shards_steam::NO_SDK);
    let mut host = build_on(
        config(|c| {
            lobby_host(c);
            c.steam.connect = true;
        }),
        None,
        save_folder(),
    );
    frames_until(&mut host, |w| matches!(session(w).state, SteamState::Failed(_)));
    let line = format!("steam: cannot host: {unavailable}; try again with steam_host");
    assert!(journal(host.world(), Some(JournalKind::Error)).contains(&line), "{:#?}", journal(host.world(), None));
    assert_eq!(host.world().resource::<NetState>(), &NetState::Ended(format!("cannot host: {unavailable}")));
    let said = ok(&mut host, "steam", serde_json::json!({}));
    assert_eq!(said.lines().next(), Some(format!("steam: unavailable: {}", shards_steam::NO_SDK).as_str()), "{said}");
    assert!(!said.contains("problem:"), "{said}");
    let status = shards::ai_core::status::status(host.world())["steam"].clone();
    assert_eq!((status["kind"].as_str(), status["note"].as_str()), (Some("none"), Some(shards_steam::NO_SDK)));

    // A client has nothing to wait for: its session ends, and with --exit-with-host the game too.
    for exit_with_host in [false, true] {
        let mut client_app = build_on(
            config(|c| {
                client(c, "c1", Some(LOBBY_BASE + 1));
                c.steam.connect = true;
                c.exit_with_host = exit_with_host;
            }),
            None,
            save_folder(),
        );
        frames_until(&mut client_app, |w| matches!(w.resource::<NetClient>().stage, ClientStage::Ended(_)));
        let why = format!("cannot enter Steam lobby {}: {unavailable}", LOBBY_BASE + 1);
        assert_eq!(client_app.world().resource::<NetClient>().stage, ClientStage::Ended(why));
        assert_eq!(client_app.should_exit(), exit_with_host.then(|| AppExit::from_code(2)));
    }
}

#[test]
fn quitting_sends_the_last_saves_up_then_closes_the_relay_and_leaves_the_lobby() {
    let steam = FakeSteam::new(MemoryHub::new(37));
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);
    let lobby = session(stand.world(0)).lobby_id().unwrap();
    // An exit hook before Steam's writes a save, as the session's does with the character the host
    // sends back as a client quits: its cloud copy is queued, and Steam's hook makes it.
    let last_save = |app: &mut App| {
        use shards::core::EngineAppExt;
        app.add_exit_hook("test: the last save", |world| {
            world.trigger(shards::save::SaveWritten { kind: shards::save::SaveKind::Character, slot: "last".into(), bytes: b"the last save".to_vec() });
        });
    };
    let app = build_between(config(|c| client(c, "c1", Some(lobby))), Some(steam.user(SECOND_USER, "c1")), save_folder(), last_save);
    let c1 = add_client(&mut stand, "c1", app);
    stand.run(10);
    assert!(steam.in_session(FIRST_USER, SECOND_USER));

    shards::core::run_exit_hooks(stand.world_mut(c1));
    assert_eq!(steam.cloud(SECOND_USER)["saves/character/last.save"].0, b"the last save", "copied up before Steam was left");
    assert!(!steam.in_session(FIRST_USER, SECOND_USER), "the relay session is closed");
    assert_eq!(steam.lobby_members(lobby), [FIRST_USER], "the lobby is left");
    stand.frame();
    assert!(journaled(stand.world(0), &format!("steam: the relay session with c1 ({SECOND_USER}) failed: closed by the peer")));
}
