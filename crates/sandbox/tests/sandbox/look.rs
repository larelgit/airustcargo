//! Look as input (F1a), end to end on the sandbox: the `look` tool and the `AIM` line, a body
//! that faces its look and swings where it looks, and a run with looks that plays back the same.
//! `scenarios/look.scenario` covers the tool and the line on the arena's own bodies; these tests
//! spawn a `viewer` (a first-person fighter, locomotion `viewer`) and a `range_dummy` (a body with
//! a head and a torso) and name the ids a spawn returns, which a scenario cannot.

use shards::action::ActionState;
use shards::ai_core::replay::{self, ReplayEvent};
use shards::ai_core::{call_tool, configure_session, AiIndex, CallOutcome, Scope, SessionChange};
use shards::core::Gaze;
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::{act, events, get, ok, state, step, text, PLAYER};

/// The arena with its fighters standing down and away, so nothing but the test's bodies is near.
fn settled() -> App {
    let mut app = sandbox::test_app(42);
    for id in ["e6", "e7", "e8", "e14"] {
        ok(&mut app, "npc_set_state", json!({"id": id, "state": "calm"}));
    }
    ok(&mut app, "teleport", json!({"id": "e6", "x": -18, "z": -18, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e7", "x": -18, "z": 18, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e8", "x": 18, "z": -18, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e14", "x": 18, "z": 18, "y": 0}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 17, "y": 0}));
    app
}

/// Spawns `prefab` at (x, z) and returns its id (`spawned e15 viewer at (0.0, 12.0)`).
fn spawn(app: &mut App, prefab: &str, x: f32, z: f32) -> String {
    let spawned = ok(app, "spawn_prefab", json!({"id": prefab, "x": x, "z": z, "y": 0}));
    text(&spawned).split_whitespace().nth(1).expect("the spawn names the id").to_owned()
}

/// A player-scope agent `bot` that plays `subject`.
fn play_as(app: &mut App, subject: &str) {
    let change = SessionChange { scope: Some(Scope::Player), subject: AiId::parse(subject), ..Default::default() };
    configure_session(app.world_mut(), "bot", &change).unwrap();
}

fn bot(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "bot", tool, args)
}

/// What the bot's `observe` says of the view: the `AIM` line, if there is one.
fn aim(app: &mut App) -> String {
    let seen = bot(app, "observe", json!({}));
    assert!(seen.ok, "{}", seen.result);
    text(&seen).lines().find(|l| l.starts_with("AIM")).unwrap_or("(no AIM line)").to_owned()
}

fn intent_look(app: &App, id: &str) -> Option<Gaze> {
    get::<Intent>(app, id).look
}

/// The way a body faces, as the compass `observe` prints.
fn facing(app: &App, id: &str) -> Vec3 {
    let entity = app.world().resource::<AiIndex>().get(AiId::parse(id).unwrap()).unwrap();
    *app.world().get::<Transform>(entity).unwrap().forward()
}

#[test]
fn the_view_names_the_part_of_a_body_it_passes_through() {
    let mut app = settled();
    let viewer = spawn(&mut app, "viewer", 0.0, 12.0);
    let dummy = spawn(&mut app, "range_dummy", 0.0, 4.0);
    play_as(&mut app, &viewer);
    step(&mut app, 3);

    // The viewer's eye is 1.6 m up (its locomotion), level with the head's centre 8 m ahead.
    let said = bot(&mut app, "look", json!({"yaw": 0, "pitch": 0}));
    assert!(said.ok, "{}", said.result);
    assert_eq!(text(&said), "ok: looking N (yaw 0.0°, pitch 0.0°)");
    assert_eq!(aim(&mut app), format!("AIM {dummy} Dummy head 7.8m"), "the head sphere is inside the 0.4 m capsule, which the view looks past");
    // At the part, by name, and at the body: its aim point is lower, in the torso.
    let at_head = bot(&mut app, "look", json!({"at": dummy, "part": "head"}));
    assert_eq!(text(&at_head), format!("ok: looking at {dummy} head, 8.0m N (yaw 0.0°, pitch 0.0°)"));
    assert_eq!(aim(&mut app), format!("AIM {dummy} Dummy head 7.8m"));
    let at_torso = bot(&mut app, "look", json!({"at": dummy, "part": "torso"}));
    assert!(text(&at_torso).starts_with(&format!("ok: looking at {dummy} torso, 8.0m N (yaw 0.0°, pitch -4.3°)")), "{}", text(&at_torso));
    assert_eq!(aim(&mut app), format!("AIM {dummy} Dummy torso 7.7m"));
    let at_body = bot(&mut app, "look", json!({"at": dummy}));
    assert!(text(&at_body).starts_with(&format!("ok: looking at {dummy}, 8.0m N (yaw 0.0°, pitch -3.7°)")), "{}", text(&at_body));
    assert!(aim(&mut app).starts_with(&format!("AIM {dummy} Dummy torso ")));
    // Below the torso only the body's own capsule takes the view: no part named.
    let legs = Gaze::toward(Vec3::new(0.0, 1.6, 12.0), Vec3::new(0.0, 0.2, 4.4)).unwrap();
    ok_look(&mut app, json!({"yaw": legs.yaw.to_degrees(), "pitch": legs.pitch.to_degrees()}));
    let line = aim(&mut app);
    assert!(line.starts_with(&format!("AIM {dummy} Dummy 7.")), "{line}");
    // A part that is not there is named with the ones that are.
    let missing = bot(&mut app, "look", json!({"at": dummy, "part": "tail"}));
    assert!(!missing.ok && text(&missing).contains("has no part `tail`; its parts: head, torso"), "{}", text(&missing));
    // A part turned off takes no view, as it takes no hit: the capsule is all that is left.
    ok(&mut app, "part", json!({"id": dummy, "name": "head", "op": "off"}));
    step(&mut app, 1);
    ok_look(&mut app, json!({"yaw": 0, "pitch": 0}));
    assert!(aim(&mut app).starts_with(&format!("AIM {dummy} Dummy ")), "{}", aim(&mut app));
    assert!(!aim(&mut app).contains("head"), "{}", aim(&mut app));
}

fn ok_look(app: &mut App, args: Value) {
    let said = bot(app, "look", args);
    assert!(said.ok, "{}", said.result);
}

#[test]
fn a_view_goes_through_any_number_of_bodies_that_take_no_hits() {
    let mut app = settled();
    // The dummy's own capsule takes no hits and its parts are lifted clear of the view, so nothing in it stops the ray.
    let before = ok(&mut app, "content_get", json!({"kind": "parts", "id": "range_dummy"})).result["def"].clone();
    let mut ghost = before.clone();
    ghost["body_hurtbox"] = json!(false);
    for part in ["head", "torso"] {
        ghost["parts"][part]["at"]["up"] = json!(4.0);
    }
    ok(&mut app, "content_apply", json!({"changes": [{"kind": "parts", "id": "range_dummy", "def": ghost, "before": before}]}));
    let viewer = spawn(&mut app, "viewer", 0.0, 12.0);
    // More of them in a row than any limit on how many a view passes through would allow.
    for i in 0..12 {
        spawn(&mut app, "range_dummy", 0.0, 10.0 - 1.2 * i as f32);
    }
    play_as(&mut app, &viewer);
    step(&mut app, 3);
    ok_look(&mut app, json!({"yaw": 0, "pitch": 0}));
    let line = aim(&mut app);
    assert!(line.starts_with("AIM wall 3"), "twelve bodies that take no hits are looked through to the wall beyond: {line}");
}

#[test]
fn a_body_that_faces_its_look_turns_at_once_and_walks_the_worlds_directions() {
    let mut app = settled();
    let viewer = spawn(&mut app, "viewer", 0.0, 12.0);
    play_as(&mut app, &viewer);
    step(&mut app, 3);
    assert!(get::<Intent>(&app, &viewer).look.is_none());

    // Looking west turns the body in one tick, however far it has to go (the player turns at 720 deg/s).
    ok_look(&mut app, json!({"yaw": 90, "pitch": 20}));
    step(&mut app, 1);
    assert!(facing(&app, &viewer).distance(Vec3::NEG_X) < 1e-4, "{}", facing(&app, &viewer));
    ok_look(&mut app, json!({"yaw": -90}));
    step(&mut app, 1);
    assert!(facing(&app, &viewer).distance(Vec3::X) < 1e-4, "{}", facing(&app, &viewer));
    // The pitch does not tilt the body, and the view stops at its limits (-85..85 for the viewer).
    let held = bot(&mut app, "look", json!({"pitch": 90}));
    assert!(text(&held).ends_with("(yaw -90.0°, pitch 85.0°) (pitch held to your limits, -85..85)"), "{}", text(&held));
    step(&mut app, 1);
    assert!(facing(&app, &viewer).y.abs() < 1e-6);
    // Walking north while looking east: the world's directions, and the body goes on facing east.
    ok_look(&mut app, json!({"yaw": -90, "pitch": 0}));
    assert!(bot(&mut app, "move", json!({"dir": "N", "meters": 3})).ok);
    step(&mut app, 30);
    let at = get::<Transform>(&app, &viewer).translation;
    assert!(at.z < 11.5 && at.x.abs() < 0.2, "{at}");
    assert!(facing(&app, &viewer).distance(Vec3::X) < 1e-4);
    // A body that is not told to look keeps turning by its walk: the player, with a look given.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 17, "y": 0}));
    let mut player_says = call_tool(app.world_mut(), "dev", "look", json!({"yaw": 90}));
    assert!(player_says.ok, "{}", player_says.result);
    step(&mut app, 30);
    assert!(facing(&app, PLAYER).distance(Vec3::NEG_X) > 0.5, "the player faces N, not W: {}", facing(&app, PLAYER));
    player_says = call_tool(app.world_mut(), "dev", "look", json!({"clear": true}));
    assert!(player_says.ok);
    assert_eq!(intent_look(&app, PLAYER), None);
}

#[test]
fn a_swing_goes_where_the_body_looks_not_where_it_walks() {
    let mut app = settled();
    let viewer = spawn(&mut app, "viewer", 0.0, 12.0);
    let dummy = spawn(&mut app, "range_dummy", -1.3, 12.0);
    play_as(&mut app, &viewer);
    step(&mut app, 3);
    // West at the dummy while an order walks east: a swing for a body that faces its movement would
    // go east, away from it.
    ok_look(&mut app, json!({"at": dummy}));
    assert!(bot(&mut app, "move", json!({"dir": "E", "meters": 5})).ok);
    step(&mut app, 2);
    let hit = bot(&mut app, "light_attack", json!({}));
    assert!(hit.ok, "{}", hit.result);
    step(&mut app, 1);
    let run = get::<ActionState>(&app, &viewer).current.expect("the swing runs");
    assert!(run.facing.distance(Vec3::NEG_X) < 0.2, "the swing faces the look (west), not the walk (east): {}", run.facing);
    step(&mut app, 40);
    let hurt = events(&app).into_iter().filter(|l| l.contains(&format!("{viewer} -> {dummy}")) || l.contains("light_1")).collect::<Vec<_>>();
    assert!(!hurt.is_empty(), "{:?}", events(&app));
    assert!(get::<Health>(&app, &dummy).current < 100.0, "the dummy was hit: {:?}", get::<Health>(&app, &dummy));
    // The control: the same body, the same look and walk, but its yaw follows its movement (the player's
    // locomotion): the swing goes the way it walks.
    step(&mut app, 60);
    ok(&mut app, "set", json!({"id": viewer, "component": "CharacterController", "path": "params", "value": "player"}));
    assert!(bot(&mut app, "move", json!({"dir": "E", "meters": 5})).ok);
    step(&mut app, 2);
    assert!(bot(&mut app, "light_attack", json!({})).ok);
    step(&mut app, 1);
    let run = get::<ActionState>(&app, &viewer).current.expect("the swing runs");
    assert!(run.facing.distance(Vec3::X) < 0.2, "a body that does not face its look swings the way it walks: {}", run.facing);
}

#[test]
fn the_view_of_a_dead_or_eyeless_body_says_so_and_a_bad_angle_is_refused() {
    let mut app = settled();
    // The boss has a controller but no player behind it: a player-scope agent may drive it.
    let viewer = spawn(&mut app, "viewer", 0.0, 12.0);
    play_as(&mut app, &viewer);
    step(&mut app, 2);
    for (args, wants) in [
        (json!({"yaw": "north"}), "invalid"),
        (json!({}), "nothing to do"),
        (json!({"clear": true, "pitch": 3}), "takes no other argument"),
    ] {
        let refused = bot(&mut app, "look", args);
        assert!(!refused.ok && text(&refused).contains(wants), "{}", refused.result);
    }
    // A pillar has no eye.
    play_as(&mut app, "e1");
    let eyeless = bot(&mut app, "look", json!({"yaw": 10}));
    assert!(!eyeless.ok, "{}", eyeless.result);
    assert!(text(&eyeless).contains("no Intent") || text(&eyeless).contains("no eye"), "{}", text(&eyeless));
    // The dead cannot look.
    play_as(&mut app, &viewer);
    ok(&mut app, "set", json!({"id": viewer, "component": "Health", "path": "current", "value": 0}));
    step(&mut app, 2);
    let dead = bot(&mut app, "look", json!({"yaw": 10}));
    assert!(!dead.ok && text(&dead).contains("you are dead"), "{}", dead.result);
}

/// A session with every kind of look a run has: angles, a target, a part, a clear, a body that faces
/// it, a swing, a walk.
fn session(app: &mut App) -> (String, String) {
    let viewer = spawn(app, "viewer", 0.0, 12.0);
    let dummy = spawn(app, "range_dummy", -1.3, 12.0);
    play_as(app, &viewer);
    step(app, 3);
    ok_look(app, json!({"yaw": 45, "pitch": -10}));
    step(app, 20);
    ok_look(app, json!({"at": dummy, "part": "head"}));
    assert!(bot(app, "move", json!({"dir": "E", "meters": 2})).ok);
    step(app, 10);
    let _ = bot(app, "light_attack", json!({}));
    step(app, 40);
    ok_look(app, json!({"at": dummy}));
    step(app, 30);
    ok_look(app, json!({"yaw": -135, "pitch": 30}));
    step(app, 25);
    let _ = bot(app, "look", json!({"clear": true}));
    step(app, 20);
    (viewer, dummy)
}

#[test]
fn a_run_with_looks_plays_back_identically() {
    let mut original = settled();
    let (viewer, _) = session(&mut original);
    let recorded = replay::save(original.world());
    let recorded: replay::Replay = shards::serde_json::from_str(&shards::serde_json::to_string(&recorded).unwrap()).unwrap();
    let mine = AiId::parse(&viewer).unwrap();
    let looks: Vec<bool> = recorded
        .events
        .iter()
        .filter_map(|e| match e {
            ReplayEvent::Input { id, component, value, .. } if *id == mine && component == "Intent" => Some(value.get("look").is_some()),
            _ => None,
        })
        .collect();
    assert!(looks.iter().filter(|set| **set).count() >= 4, "every new look is recorded as an input of the viewer: {looks:?}");
    assert_eq!(looks.last(), Some(&false), "clearing it is recorded too (the field is left out again): {looks:?}");

    let mut copy = sandbox::test_app(recorded.seed);
    let end = recorded.end.tick;
    replay::play(copy.world_mut(), recorded).unwrap();
    ok(&mut copy, "step", json!({"ticks": end}));
    copy.update();
    let report = copy.world().resource::<shards::ai_core::AiReplay>().report().cloned().expect("the playback finished");
    assert!(report.identical(), "{report}");
    assert!(report.matched >= 3, "{report}");
    assert_eq!(state(&mut original), state(&mut copy), "every body, bit for bit");
    assert_eq!(intent_look(&original, &viewer), intent_look(&copy, &viewer));
    assert_eq!(get::<Transform>(&original, &viewer).rotation, get::<Transform>(&copy, &viewer).rotation);
}

#[test]
fn an_intent_that_never_looks_is_written_without_the_field() {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "move", json!({"dir": "W", "meters": 2}));
    step(&mut app, 5);
    let recorded = replay::save(app.world());
    let text = shards::serde_json::to_string(&recorded).unwrap();
    assert!(!text.contains("look"), "a run that never looks is what it was before the field existed");
    let looking = act(&mut app, "look", json!({"yaw": 10}));
    assert!(looking.ok);
    step(&mut app, 2);
    let recorded = replay::save(app.world());
    assert!(shards::serde_json::to_string(&recorded).unwrap().contains(r#""look":{"pitch""#));
}

/// What `member` of a stand has for the body `id`: its look and the way it faces.
fn view_on(stand: &shards::net::Stand, member: usize, id: &str) -> (Option<Gaze>, Vec3) {
    let world = stand.world(member);
    let mut q = world.try_query::<(&AiId, &Intent, &Transform)>().unwrap();
    q.iter(world).find(|(i, ..)| i.to_string() == id).map(|(_, intent, tf)| (intent.look, *tf.forward())).expect("the body is there")
}

/// A client's look in co-op: the `look` tool writes the client's own `Intent`, the input frame carries
/// it to the host (the net mirror of `Intent`), and the body whose locomotion faces its look turns to
/// it on the host's simulation and on the client's prediction; nobody desyncs.
#[test]
fn a_clients_look_reaches_the_host_and_turns_its_body_on_both() {
    use crate::net::{desyncs, lossy, stand_with, C1, HOST};
    let mut stand = stand_with(1, lossy());
    let player = crate::net::body(&stand, C1);
    for id in ["e6", "e7", "e8", "e14"] {
        assert!(stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"})).ok);
    }
    assert!(stand.call(HOST, "teleport", json!({"id": PLAYER, "x": -15, "z": 15, "y": 0})).ok);
    assert!(stand.call(HOST, "teleport", json!({"id": player, "x": 0, "z": 12, "y": 0})).ok);
    let first_person = json!({"id": player, "component": "CharacterController", "path": "params", "value": "viewer"});
    assert!(stand.call(HOST, "set", first_person).ok);
    stand.run(30);
    let looked = stand.call(C1, "look", json!({"yaw": 90, "pitch": 15}));
    assert!(looked.ok, "{}", looked.result);
    stand.run(60);
    let want = Gaze::from_degrees(90.0, 15.0);
    for member in [HOST, C1] {
        let (look, facing) = view_on(&stand, member, &player);
        assert_eq!(look, Some(want), "member {member} has c1's look");
        assert!(facing.distance(Vec3::NEG_X) < 1e-3, "member {member}: the body faces the look: {facing}");
    }
    // Looking away again, and clearing it, reach the host the same way.
    assert!(stand.call(C1, "look", json!({"yaw": -90})).ok);
    stand.run(60);
    assert!(view_on(&stand, HOST, &player).1.distance(Vec3::X) < 1e-3);
    assert!(stand.call(C1, "look", json!({"clear": true})).ok);
    stand.run(60);
    assert_eq!(view_on(&stand, HOST, &player).0, None);
    assert_eq!(view_on(&stand, C1, &player).0, None);
    for member in [HOST, C1] {
        assert_eq!(desyncs(&stand, member), 0, "member {member}");
    }
}

/// Heavy mode (`--heavy N`): from every 30th tick's snapshot a fresh game must go on like the run did,
/// which holds only if the look and what follows from it are all in the snapshot.
#[test]
fn a_run_with_looks_continues_alike_from_its_snapshots() {
    let mut original = settled();
    session(&mut original);
    let recorded = replay::save(original.world());
    let dir = std::env::temp_dir().join(format!("shards-look-heavy-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("look.json");
    std::fs::write(&file, shards::serde_json::to_string(&recorded).unwrap()).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .arg("--replay")
        .arg(&file)
        .args(["--verify", "--heavy", "30"])
        .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let verdict = stdout.lines().find(|l| l.starts_with("replay ")).unwrap_or_default();
    assert!(verdict.contains("identical to the recording") && verdict.contains(", 0 skipped"), "{stdout}");
}
