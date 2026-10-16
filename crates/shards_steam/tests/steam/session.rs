//! Sessions over the fake relay: a lobby host and two friends, a game server and a player who joins
//! it by its id, an invite followed, sessions refused, and joins that fail.

use shards::ai_core::journal::JournalKind;
use shards::net::{ClientStage, LinkConditions, MemoryHub, NetClient, NetHost, NetState};
use shards::prelude::*;
use shards_steam::fake::{FIRST_USER, LOBBY_BASE};
use shards_steam::{FakeSteam, SteamRole, SteamState};

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
