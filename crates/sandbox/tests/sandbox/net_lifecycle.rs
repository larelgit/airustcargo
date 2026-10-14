//! A session's life: joins and leaves as conditions, a full session, the end of a session
//! (once, with every character saved), and the stand's clock and shutdown.

use shards::ai_core::process::AppToken;
use shards::ai_core::sim;
use shards::net::host::PeerStage;
use shards::net::{JoinDecision, JoinRequest, LinkConditions, NetDef, NetDefaults, NetHooks, NetHost, NetLeft, NetState, Stand};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use super::net::*;
use crate::common::*;

/// The game's world saves (`on_session_end`).
#[derive(Resource, Default)]
struct WorldSaves(u32);

/// The characters a client got back from its host (`on_character_save`).
#[derive(Resource, Default)]
struct CharacterSaves(u32);

fn character(_: &mut World, _: Entity) -> Option<Value> {
    Some(json!({"level": 3}))
}

fn keep_character(world: &mut World, _tick: u64, _reason: &str, sections: &Value) {
    if sections["level"] == 3 {
        world.get_resource_or_init::<CharacterSaves>().0 += 1;
    }
}

fn save_world(world: &mut World) -> Option<String> {
    world.get_resource_or_init::<WorldSaves>().0 += 1;
    None
}

/// The sandbox with co-op saves: characters go to their players, the host saves the world.
fn with_saves(app: &mut App) {
    app.add_plugins(sandbox::SandboxPlugin);
    let mut hooks = app.world_mut().resource_mut::<NetHooks>();
    hooks.capture_character = character;
    hooks.on_character_save = keep_character;
    hooks.on_session_end = save_world;
}

fn saves<R: Resource>(stand: &Stand, member: usize, count: fn(&R) -> u32) -> u32 {
    stand.world(member).get_resource::<R>().map_or(0, count)
}

/// The game's door: while it is closed, everyone joining waits.
#[derive(Resource)]
struct DoorClosed;

fn door(world: &World, _: &JoinRequest) -> JoinDecision {
    if world.contains_resource::<DoorClosed>() {
        JoinDecision::Wait("the door is closed".into())
    } else {
        JoinDecision::Accept
    }
}

/// Co-op saves, the door, and bodies that wait half a second for their player.
fn with_door(app: &mut App) {
    with_saves(app);
    app.world_mut().resource_mut::<NetHooks>().join_policy = door;
    app.insert_resource(NetDefaults(NetDef { rejoin_grace_secs: 0.5, ..Default::default() }));
}

fn waiting(stand: &Stand, member: usize, why: &str) -> bool {
    matches!(net_state(stand, member), NetState::Joining(w) if w.contains(why))
}

fn player_of(stand: &Stand, member: usize) -> Option<u8> {
    match net_state(stand, member) {
        NetState::Connected { player, .. } => Some(player),
        _ => None,
    }
}

/// A client app of `stand` named `name`, its link to the host made now, not added yet: an
/// endpoint made early sorts before the ones made after it.
fn early_client(stand: &Stand, name: &str, setup: fn(&mut App)) -> (App, shards::net::LinkId) {
    let mut config = shards::testing::config();
    configure(&mut config);
    config.net = shards::core::NetLaunch::Join { addr: "host".into() };
    config.player_name = name.to_owned();
    let mut app = App::new();
    app.add_plugins(shards::EnginePlugins::new(config));
    setup(&mut app);
    let id = shards::net::stand::attach(&mut app, &stand.hub, name, Some(0));
    let link = stand.hub.connect_both(0, id, LinkConditions::NONE);
    app.finish();
    app.cleanup();
    (app, link)
}

/// Datagrams sent so far over member `member`'s link: (host to it, it to the host).
fn datagrams(stand: &Stand, member: usize) -> (u64, u64) {
    let stats = stand.hub.link_stats(stand.members[member].link.expect("a client's link"));
    (stats.ab.sent, stats.ba.sent)
}

// --- the end -------------------------------------------------------------------------------

#[test]
fn a_host_that_quits_ends_the_session_after_its_clients_got_their_characters() {
    // 150 ms away, 2 % lost: the end must still arrive.
    let mut stand = shards::testing::stand(2, lossy(), configure, with_saves);
    stand.run(30);
    let exit = stand.shut_down(5000);
    assert_eq!(exit, AppExit::Success);
    assert_eq!(saves::<WorldSaves>(&stand, HOST, |s| s.0), 1, "the world saved once");
    for member in [C1, C2] {
        assert!(has_line(&stand, member, "net: session ended by host (the host quit)"), "{:#?}", journal(&stand, member));
        assert!(matches!(net_state(&stand, member), NetState::Ended(_)), "{:?}", net_state(&stand, member));
        assert_eq!(saves::<CharacterSaves>(&stand, member, |s| s.0), 1, "member {member} got its character");
    }
}

#[test]
fn net_end_ends_the_session_once() {
    let mut stand = shards::testing::stand(1, LinkConditions::NONE, configure, with_saves);
    stand.run(10);
    let ended = stand.call(HOST, "net_end", json!({"reason": "the raid is over"}));
    assert!(ended.ok, "{}", ended.result);
    stand.run(10);
    let again = stand.call(HOST, "net_end", json!({}));
    assert!(!again.ok, "{}", again.result);
    assert!(text(&again).contains("the session has ended already (the raid is over)"), "{}", text(&again));
    // The host quits later: its exit hooks find the session over.
    shards::core::run_exit_hooks(stand.world_mut(HOST));
    assert_eq!(saves::<WorldSaves>(&stand, HOST, |s| s.0), 1, "the game's hook ran once");
    let ends = journal(&stand, HOST).into_iter().filter(|l| l.contains("net: session ended (")).count();
    assert_eq!(ends, 1, "{:#?}", journal(&stand, HOST));
    assert!(matches!(net_state(&stand, HOST), NetState::Ended(why) if why == "the raid is over"), "{:?}", net_state(&stand, HOST));
    assert!(has_line(&stand, C1, "net: session ended by host (the raid is over)"), "{:#?}", journal(&stand, C1));
    assert_eq!(saves::<CharacterSaves>(&stand, C1, |s| s.0), 1);
}

#[test]
fn net_end_lets_the_players_go_and_refuses_whoever_waits() {
    let mut stand = shards::testing::stand_host(2, &configure, &with_door);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &with_door);
    run_until(&mut stand, 300, "c1 joins", |s| connected(s, c1));
    let me = body(&stand, c1);
    stand.world_mut(HOST).insert_resource(DoorClosed);
    let c2 = shards::testing::stand_client(&mut stand, "c2", LinkConditions::NONE, &configure, &with_door);
    run_until(&mut stand, 300, "c2 waits at the door", |s| waiting(s, c2, "the door is closed"));
    assert!(stand.call(HOST, "net_end", json!({"reason": "the raid is over"})).ok);
    // The host plays on alone: the player left, the one at the door is told.
    let theirs = entity(&stand, HOST, &me).expect("the body waits, as a leaver's does");
    assert!(stand.world(HOST).get::<NetLeft>(theirs).is_some());
    assert!(has_line(&stand, HOST, "net: p1 left (session ended)"), "{:#?}", journal(&stand, HOST));
    let host = stand.world(HOST).resource::<NetHost>();
    assert!(host.peers.values().all(|p| p.stage == PeerStage::Gone), "{:?}", host.peers.values().map(|p| &p.stage).collect::<Vec<_>>());
    stand.world_mut(HOST).remove_resource::<DoorClosed>();
    run_until(&mut stand, 60, "c2 hears it", |s| matches!(net_state(s, c2), NetState::Ended(_)));
    assert!(matches!(net_state(&stand, c2), NetState::Ended(why) if why == "refused: the session has ended (the raid is over)"), "{:?}", net_state(&stand, c2));
    stand.run(400);
    assert_eq!(saves::<CharacterSaves>(&stand, c1, |s| s.0), 1, "the character once, with the end");
    assert_eq!(host_calls(&stand, "player_join").len(), 1, "nobody joined after the end");
    assert!(stand.world(HOST).resource::<NetHost>().peers.is_empty());
    // The ended games went quiet, and so did the host.
    let before = (datagrams(&stand, c1), datagrams(&stand, c2));
    stand.run(120);
    assert_eq!((datagrams(&stand, c1), datagrams(&stand, c2)), before, "nothing sent after the end");
}

// --- joins and leaves ----------------------------------------------------------------------

#[test]
fn joins_and_leaves_wake_the_hosts_agent() {
    let mut stand = shards::testing::stand_host(1, &configure, &sandbox_setup);
    stand.run(5);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &sandbox_setup);
    let joined = stand.call(HOST, "step", json!({"ticks": 600, "until": [{"named": "net_joined"}]}));
    assert!(text(&joined).contains("stopped: condition net_joined"), "{}", text(&joined));
    assert!(connected(&stand, c1));
    let me = body(&stand, c1);
    stand.run(10);
    // The player quits (their game's exit hooks): the host hears it at once.
    shards::core::run_exit_hooks(stand.world_mut(c1));
    let left = stand.call(HOST, "step", json!({"ticks": 120, "until": [{"named": "net_left"}]}));
    assert!(text(&left).contains("stopped: condition net_left"), "{}", text(&left));
    assert!(has_line(&stand, HOST, "net: p1 left (quit)"), "{:#?}", journal(&stand, HOST));
    let waiting = entity(&stand, HOST, &me).expect("the body waits for a rejoin");
    assert!(stand.world(HOST).get::<NetLeft>(waiting).is_some());
}

#[test]
fn a_second_game_with_a_players_token_takes_the_slot_and_the_first_is_told() {
    let mut stand = shards::testing::stand_host(1, &configure, &with_saves);
    // A second game of the same name: the hub keeps c1's token, as one save folder would.
    // Its link is made before the first game's and sorts before it among the host's peers:
    // the character goes to the game that left, not to the first of that player's links.
    let (twin_app, twin_link) = early_client(&stand, "c1", with_saves);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &with_saves);
    run_until(&mut stand, 300, "the first game joins", |s| connected(s, c1));
    stand.run(10);
    let first = body(&stand, C1);
    let first_link = stand.world(HOST).resource::<NetHost>().peers.values().next().unwrap().id;
    let twin = stand.add("c1", twin_app, Some(twin_link));
    run_until(&mut stand, 300, "the second game joins", |s| connected(s, twin));
    stand.run(10);
    let playing: Vec<_> = stand.world(HOST).resource::<NetHost>().peers.values().filter(|p| p.stage == PeerStage::Connected).map(|p| p.id).collect();
    assert!(playing.len() == 1 && playing[0] < first_link, "the second game's link sorts first: {playing:?}, the first's {first_link}");
    assert_eq!(body(&stand, twin), first, "the same slot, the same body");
    assert!(
        matches!(net_state(&stand, C1), NetState::Ended(why) if why.contains("another game joined with your token")),
        "{:?}",
        net_state(&stand, C1)
    );
    assert!(has_line(&stand, HOST, "net: p1 left (reconnected)"), "{:#?}", journal(&stand, HOST));
    assert_eq!(saves::<CharacterSaves>(&stand, C1, |s| s.0), 1, "the character went to the game that left");
    assert_eq!(saves::<CharacterSaves>(&stand, twin, |s| s.0), 0);
}

#[test]
fn a_slot_a_second_game_took_stays_its_own_while_it_waits() {
    let mut stand = shards::testing::stand_host(2, &configure, &with_door);
    let c1 = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &with_door);
    run_until(&mut stand, 300, "c1 joins", |s| connected(s, c1));
    let first = body(&stand, c1);
    stand.world_mut(HOST).insert_resource(DoorClosed);
    // The second game takes p1's slot and waits at the door; the first game's body waits out
    // its half second and goes.
    let twin = shards::testing::stand_client(&mut stand, "c1", LinkConditions::NONE, &configure, &with_door);
    run_until(&mut stand, 300, "the second game waits", |s| waiting(s, twin, "the door is closed"));
    run_until(&mut stand, 300, "the first game's body goes", |s| entity(s, HOST, &first).is_none());
    // A stranger now: p1's slot is held, whatever the first game's leaving did to it.
    let stranger = shards::testing::stand_client(&mut stand, "c2", LinkConditions::NONE, &configure, &with_door);
    run_until(&mut stand, 300, "the stranger waits", |s| waiting(s, stranger, "the door is closed"));
    stand.world_mut(HOST).remove_resource::<DoorClosed>();
    run_until(&mut stand, 300, "both join", |s| connected(s, twin) && connected(s, stranger));
    assert_eq!((player_of(&stand, twin), player_of(&stand, stranger)), (Some(1), Some(2)), "{:#?}", journal(&stand, HOST));
    assert!(!has_line(&stand, HOST, "refused"), "{:#?}", journal(&stand, HOST));
}

#[test]
fn a_client_sent_away_is_disconnected_for_its_agent() {
    let mut stand = stand(1);
    stand.run(10);
    assert!(stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"})).ok);
    run_until(&mut stand, 60, "c1 is sent away", |s| matches!(net_state(s, C1), NetState::Ended(_)));
    let out = stand.call(C1, "step", json!({"ticks": 30, "until": [{"named": "net_disconnected"}]}));
    assert!(text(&out).contains("stopped: condition net_disconnected"), "{}", text(&out));
    // Its game acknowledges what the host still sends, then goes quiet: the host makes no
    // new peer of it, and nothing goes either way after that.
    stand.run(400);
    assert!(stand.world(HOST).resource::<NetHost>().peers.is_empty(), "{:?}", stand.world(HOST).resource::<NetHost>().peers.keys().collect::<Vec<_>>());
    let before = datagrams(&stand, C1);
    stand.run(120);
    assert_eq!(datagrams(&stand, C1), before, "nothing sent after the end");
}

#[test]
fn a_full_session_refuses_the_next_player() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.insert_resource(NetDefaults(NetDef { max_players: 2, ..Default::default() }));
    };
    let stand = shards::testing::stand(2, LinkConditions::NONE, configure, setup);
    assert!(connected(&stand, C1));
    assert!(
        matches!(net_state(&stand, C2), NetState::Ended(why) if why.contains("the session is full (2 players)")),
        "{:?}",
        net_state(&stand, C2)
    );
}

#[test]
fn net_sim_changes_a_members_links() {
    let mut stand = stand(1);
    let set = stand.call(C1, "net_sim", json!({"conditions": "latency=150,loss=0.02"}));
    assert!(set.ok, "{}", set.result);
    let status = stand.call(C1, "net_status", json!({}));
    let conditions = status.result["conditions"].as_str().unwrap_or_default().to_owned();
    assert!(conditions.contains("150"), "{}", status.result);
    stand.run(180);
    let rtt = stand.world(HOST).resource::<shards::net::NetHost>().peers.values().next().unwrap().link.stats().srtt_ms;
    assert!(rtt > 250.0, "150 ms each way: rtt {rtt}");
    assert!(stand.call(C1, "net_sim", json!({"conditions": "none"})).ok);
    let typo = stand.call(C1, "net_sim", json!({"conditions": "latency=15O"}));
    assert!(!typo.ok, "{}", typo.result);
    assert!(connected(&stand, C1));
}

// --- the stand -----------------------------------------------------------------------------

#[test]
fn a_stands_members_say_how_the_stand_runs_them() {
    let mut stand = stand(1);
    let paused = |stand: &shards::net::Stand, m: usize| shards::ai_core::status::status(stand.world(m))["paused"].as_bool();
    assert_eq!((paused(&stand, HOST), paused(&stand, C1)), (Some(true), Some(true)));
    assert!(stand.call(HOST, "resume", json!({})).ok);
    assert!(stand.running);
    assert_eq!((paused(&stand, HOST), paused(&stand, C1)), (Some(false), Some(false)), "running, as the stand runs");
    // A step pauses, as it does in a game of its own.
    assert!(stand.call(C1, "step", json!({"ticks": 2})).ok);
    assert!(!stand.running);
    assert_eq!((paused(&stand, HOST), paused(&stand, C1)), (Some(true), Some(true)));
    // No negative speed.
    assert!(stand.call(HOST, "speed", json!({"x": -2.0})).ok);
    assert_eq!(stand.speed, 0.0);
    assert_eq!((sim::speed(stand.world(HOST)), sim::speed(stand.world(C1))), (0.0, 0.0));
    // The answer says the new speed.
    let half = stand.call(C1, "speed", json!({"x": 0.5}));
    assert!(half.ok && text(&half) == "speed 0.5x", "{}", half.result);
    // A number past f32 arrives as an infinity: refused, the stand untouched.
    let absurd = stand.call(HOST, "speed", json!({"x": 1e39}));
    assert!(!absurd.ok && text(&absurd).contains("is not a speed a clock can run at"), "{}", absurd.result);
    assert_eq!(stand.speed, 0.5);
    assert_eq!((sim::speed(stand.world(HOST)), sim::speed(stand.world(C1))), (0.5, 0.5));
    assert!(stand.call(C1, "resume", json!({})).ok && stand.call(C1, "pause", json!({})).ok);
}

/// Whether the app was the active one when its exit hooks ran.
#[derive(Resource)]
struct ActiveAtExit(bool);

fn note_active(world: &mut World) {
    let mine = world.resource::<AppToken>().0;
    world.insert_resource(ActiveAtExit(shards::ai_core::process::active() == mine));
}

#[test]
fn a_stands_members_are_each_active_while_their_exit_hooks_run() {
    let setup = |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.add_exit_hook("note", note_active);
    };
    let mut stand = shards::testing::stand(2, LinkConditions::NONE, configure, setup);
    stand.run(10);
    stand.shut_down(5000);
    for member in [HOST, C1, C2] {
        let active = stand.world(member).get_resource::<ActiveAtExit>().map(|a| a.0);
        assert_eq!(active, Some(true), "member {member}: what its hooks log goes to its own journal");
    }
}

#[test]
fn only_the_host_ends_a_stand() {
    let exit_with_host = |config: &mut shards::EngineConfig| {
        config.seed = 42;
        config.exit_with_host = true;
    };
    let mut stand = shards::testing::stand(2, LinkConditions::NONE, exit_with_host, sandbox_setup);
    stand.run(10);
    assert!(stand.call(HOST, "net_kick", json!({"player": 1, "reason": "testing"})).ok);
    run_until(&mut stand, 60, "c1 is sent away", |s| matches!(net_state(s, C1), NetState::Ended(_)));
    assert_eq!(stand.members[C1].app.should_exit(), Some(AppExit::error()), "c1 exits with its session, sent away: not 0");
    assert!(!stand.serve(), "the stand goes on without it");
    stand.run(30);
    assert!(connected(&stand, C2));
    // The host ends the session: an end, not a failure.
    assert!(stand.call(HOST, "net_end", json!({})).ok);
    run_until(&mut stand, 60, "c2 hears the end", |s| matches!(net_state(s, C2), NetState::Ended(_)));
    assert_eq!(stand.members[C2].app.should_exit(), Some(AppExit::Success));
}

#[test]
fn a_game_that_cannot_reach_its_host_exits_with_2() {
    let mut config = shards::testing::config();
    config.seed = 42;
    config.net = shards::core::NetLaunch::Join { addr: "not-a-host".into() };
    config.exit_with_host = true;
    let mut app = sandbox::build_app(&config);
    app.finish();
    app.cleanup();
    app.update();
    assert_eq!(app.should_exit(), Some(AppExit::from_code(2)));
    let state = app.world().resource::<NetState>().clone();
    assert!(matches!(&state, NetState::Ended(why) if why.starts_with("cannot reach not-a-host")), "{state:?}");
}
