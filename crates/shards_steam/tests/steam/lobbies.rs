//! Looking for lobbies: `steam_lobbies` finds the public and invisible lobbies of this game and build
//! that have room, never a friends-only or private one (and says so), lists other builds only when
//! asked, waits for a slow Steam, and its ids go to `steam_join`.

use serde_json::json;
use shards::core::SteamLobby;
use shards::net::{MemoryHub, Stand};
use shards_steam::fake::FIRST_USER;
use shards_steam::{FakeSteam, SteamState};

use crate::*;

/// The most players the tests' game takes (`NetDef`): what a lobby's size says.
const MAX: u32 = 3;

/// A stand on `steam` with a lobby host of lobby type `kind`, and its lobby.
fn hosted(steam: &FakeSteam, kind: SteamLobby) -> (Stand, u64) {
    let host = build(
        config(|c| {
            lobby_host(c);
            c.steam.lobby = kind;
        }),
        steam.user(FIRST_USER, "host"),
        save_folder(),
    );
    let stand = stand_with_host(steam, host);
    let lobby = session(stand.world(0)).lobby_id().expect("the host made its lobby");
    (stand, lobby)
}

/// A client that waits for a lobby (`--join steam:`), as member `c1`, once its content is loaded.
fn waiting(stand: &mut Stand, steam: &FakeSteam) -> usize {
    let app = build(config(|c| client(c, "c1", None)), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = stand.add("c1", app, None);
    frames_until(stand.app(c1), |world| world.resource::<ContentDb>().is_loaded());
    assert_eq!(session(stand.world(c1)).state, SteamState::WaitingForInvite);
    c1
}

fn lobbies(stand: &mut Stand, member: usize, args: serde_json::Value) -> String {
    let out = stand.call(member, "steam_lobbies", args.clone());
    assert!(out.ok, "steam_lobbies {args} failed: {}", out.result);
    text(&out)
}

fn lobbies_err(stand: &mut Stand, member: usize, args: serde_json::Value) -> String {
    let out = stand.call(member, "steam_lobbies", args.clone());
    assert!(!out.ok, "steam_lobbies {args} should fail: {}", out.result);
    text(&out)
}

#[test]
fn a_waiting_player_finds_an_invisible_lobby_and_enters_it_by_the_id_in_the_list() {
    let steam = FakeSteam::new(MemoryHub::new(41));
    let (mut stand, lobby) = hosted(&steam, SteamLobby::Invisible);
    let c1 = waiting(&mut stand, &steam);

    let said = lobbies(&mut stand, c1, json!({}));
    println!("steam_lobbies (a waiting client):\n{said}");
    assert_eq!(
        said,
        format!(
            "1 lobby of `game` of this build with a free slot:\n\
             lobby {lobby} · host host ({FIRST_USER}) · 1 of {MAX} · build matches\n\
             enter one with steam_join lobby=ID"
        )
    );

    // The id from the list is the one `steam_join` takes.
    let joined = stand.call(c1, "steam_join", json!({ "lobby": lobby.to_string() }));
    assert!(joined.ok, "{}", joined.result);
    stand.settle().unwrap();
    assert_eq!(text(&joined), format!("entered lobby {lobby}; its host is host ({FIRST_USER})"));
    assert_eq!(session(stand.world(c1)).members.len(), 2);

    // Both in it: the host sees it listed as its own, with the room left.
    let host_said = lobbies(&mut stand, 0, json!({}));
    println!("steam_lobbies (the host):\n{host_said}");
    assert_eq!(
        host_said,
        format!("1 lobby of `game` of this build with a free slot:\nlobby {lobby} · host host ({FIRST_USER}) · 2 of {MAX} · build matches · this game's lobby")
    );
}

#[test]
fn a_public_lobby_is_found_and_a_friends_only_or_private_one_never_is() {
    for (kind, found) in [(SteamLobby::Public, true), (SteamLobby::Invisible, true), (SteamLobby::Friends, false), (SteamLobby::Private, false)] {
        let steam = FakeSteam::new(MemoryHub::new(43));
        let (mut stand, lobby) = hosted(&steam, kind);
        let c1 = waiting(&mut stand, &steam);
        let said = lobbies(&mut stand, c1, json!({}));
        assert_eq!(said.contains(&format!("lobby {lobby} ")), found, "{kind}: {said}");
        assert_eq!(said.starts_with("no lobby of `game`"), !found, "{kind}: {said}");

        // The host's own answer: its lobby is there, or the answer says why it is not.
        let own = lobbies(&mut stand, 0, json!({}));
        println!("steam_lobbies ({kind} host):\n{own}");
        if found {
            assert!(own.ends_with("· this game's lobby"), "{own}");
            assert!(!own.contains("note:"), "{own}");
        } else {
            assert_eq!(
                own,
                format!(
                    "no lobby of `game` of this build with a free slot answers the search\n\
                     Steam's search lists public and invisible lobbies only: a host started with --steam-lobby friends (the default) or private is never in it\n\
                     all=true lists the lobbies of other builds of this game too\n\
                     note: this game's own lobby {lobby} ({kind}) is not in the answer: Steam's search never lists friends-only or private lobbies; host with --steam-lobby public or invisible to be found"
                )
            );
        }
        // Not being listed keeps no friend out: the lobby is entered by its id.
        let joined = stand.call(c1, "steam_join", json!({ "lobby": lobby.to_string() }));
        assert!(joined.ok, "{kind}: {}", joined.result);
    }
}

#[test]
fn a_full_public_lobby_is_not_listed_and_the_host_is_told_why() {
    let steam = FakeSteam::new(MemoryHub::new(45));
    let (mut stand, lobby) = hosted(&steam, SteamLobby::Public);
    // Two friends and the host fill its three places.
    for friend in 0..MAX - 1 {
        let app = build(config(|c| client(c, &format!("f{friend}"), Some(lobby))), steam.user(SECOND_USER + u64::from(friend), &format!("f{friend}")), save_folder());
        stand.add(&format!("f{friend}"), app, None);
    }
    stand.settle().unwrap();
    assert_eq!(steam.lobby_members(lobby).len() as u32, MAX);
    let host_said = lobbies(&mut stand, 0, json!({}));
    assert!(host_said.starts_with("no lobby of `game` of this build with a free slot answers the search"), "{host_said}");
    assert!(
        host_said.ends_with(&format!("note: this game's own lobby {lobby} (public) is not in the answer: it has no free slot ({MAX} in it), or Steam's list has not caught up with it yet")),
        "{host_said}"
    );
}

#[test]
fn a_lobby_of_another_build_is_listed_only_when_asked_for_and_says_what_differs() {
    let steam = FakeSteam::new(MemoryHub::new(47));
    let (mut stand, lobby) = hosted(&steam, SteamLobby::Public);
    let c1 = waiting(&mut stand, &steam);
    // The same game, but its host runs other content and another engine.
    assert!(steam.set_lobby_data(lobby, "content", "0000000000000001"));
    assert!(steam.set_lobby_data(lobby, "engine", "0.0.1"));

    let said = lobbies(&mut stand, c1, json!({}));
    assert!(said.starts_with("no lobby of `game` of this build with a free slot answers the search"), "{said}");
    assert!(said.contains("all=true lists the lobbies of other builds of this game too"), "{said}");

    let all = lobbies(&mut stand, c1, json!({ "all": true }));
    println!("steam_lobbies all=true:\n{all}");
    let lines: Vec<&str> = all.lines().collect();
    assert_eq!(lines[0], "1 lobby of `game` with a free slot, of any build:");
    assert!(lines[1].starts_with(&format!("lobby {lobby} · host host ({FIRST_USER}) · 1 of {MAX} · build differs: engine 0.0.1 (yours ")), "{}", lines[1]);
    assert!(lines[1].contains(", content 0000000000000001 (yours "), "{}", lines[1]);
    assert!(!lines[1].contains("rules"), "the network rules are the same: {}", lines[1]);
    assert_eq!(lines.len(), 2, "no lobby here can be entered, so no hint to enter one: {all}");
}

#[test]
fn a_search_that_steam_has_not_answered_says_so_and_the_next_call_has_the_answer() {
    let steam = FakeSteam::new(MemoryHub::new(49));
    let (mut stand, lobby) = hosted(&steam, SteamLobby::Public);
    let c1 = waiting(&mut stand, &steam);

    steam.hold_searches(true);
    let slow = lobbies_err(&mut stand, c1, json!({ "wait_ms": 30 }));
    assert_eq!(slow, "Steam has not answered the lobby search within 30 ms; it goes on: call steam_lobbies again (wait_ms=… waits longer)");
    // The search that waits is the one asked for: another kind of search is told to wait for it.
    let other = lobbies_err(&mut stand, c1, json!({ "all": true, "wait_ms": 0 }));
    assert_eq!(other, "an earlier search (all=false) still waits for Steam's answer: ask again with all=false, or in a moment");
    let same = lobbies_err(&mut stand, c1, json!({ "wait_ms": 0 }));
    assert!(same.starts_with("Steam has not answered the lobby search within 0 ms"), "{same}");

    steam.hold_searches(false);
    let said = lobbies(&mut stand, c1, json!({}));
    assert!(said.contains(&format!("lobby {lobby} · host host ({FIRST_USER}) · 1 of {MAX} · build matches")), "{said}");

    // Steam can fail the search too.
    steam.fail_searches(Some("Steam could not list lobbies (no connection to the Steam servers)"));
    assert_eq!(lobbies_err(&mut stand, c1, json!({})), "Steam could not list lobbies (no connection to the Steam servers)");
    steam.fail_searches(None);
    assert!(lobbies(&mut stand, c1, json!({})).contains(&format!("lobby {lobby} ")), "a search after a failed one asks again");
}

#[test]
fn a_game_server_has_no_lobbies_to_search_and_a_game_without_steam_says_so() {
    let steam = FakeSteam::new(MemoryHub::new(51));
    let server = build(config(game_server), steam.server(SERVER, "server"), save_folder());
    let mut stand = stand_with_host(&steam, server);
    let said = lobbies_err(&mut stand, 0, json!({}));
    assert!(said.starts_with("Steam could not search for lobbies: an anonymous game server has no lobbies"), "{said}");

    // A game that talks to Steam (`connect`) in a build without the SDK, as the tests are: no Steam.
    #[cfg(not(feature = "sdk"))]
    {
        let mut game = build_on(
            config(|c| {
                lobby_host(c);
                c.steam.connect = true;
            }),
            None,
            save_folder(),
        );
        game.update();
        let said = err(&mut game, "steam_lobbies", json!({}));
        assert_eq!(said, format!("Steam is unavailable ({})", shards_steam::NO_SDK));
    }
}

#[test]
fn a_game_that_is_still_loading_cannot_tell_its_build() {
    let steam = FakeSteam::new(MemoryHub::new(53));
    let (mut stand, _) = hosted(&steam, SteamLobby::Public);
    let app = build(config(|c| client(c, "c1", None)), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = stand.add("c1", app, None);
    let said = lobbies_err(&mut stand, c1, json!({}));
    assert!(said.starts_with("this game is still loading its content, so its build is not known yet"), "{said}");
}
