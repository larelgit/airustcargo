//! Crouching (F2) end to end on the sandbox: the test room `crouch` (`assets/levels/crouch.gltf`, written by
//! [`room`]; a duct only a crouch passes and a ledge only a crouch jump clears), an agent that walks it, a dummy
//! whose hurtbox, eye and head follow its stance, and a crouched body riding a lift. Regenerate the room's file
//! after changing [`room`]:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox crouch::the_committed_room
//! ```
//!
//! `scenarios/crouch.scenario` plays the room (its reference replay is `tests/replays/crouch.json`).

use std::path::{Path, PathBuf};
use std::process::Command;

use shards::action::Part;
use shards::ai_core::{call_tool, configure_session, AiIndex, CallOutcome, Scope, SessionChange};
use shards::motion::{eye_of, Crouch};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::{events, fails, ok, step, text, PLAYER};
use crate::gltf::{Gltf, NodeSpec};

/// Where the room's origin is in the world: on the canyon floor east of the course, clear of the rock.
const ORIGIN: Vec3 = Vec3::new(116.0, -10.0, -11.0);

/// The room, in its own frame: a floor 24 x 8 m; a duct 6 m long (x 2 to 8), 2.4 m wide, whose ceiling is 1.2 m
/// up (the crouched capsule, 1.0 m, fits; the standing one, 1.8 m, does not); and a ledge 1.7 m high from x 14 to
/// 18 (a jump peaks 1.2 m above the feet: a plain jump misses it, a jump with a crouch in the air clears it).
pub fn room() -> Gltf {
    let mut g = Gltf::new();
    let floor = g.box_mesh("Floor", [-4.0, -0.5, -4.0], [20.0, 0.0, 4.0]);
    let ceiling = g.box_mesh("Ceiling", [2.0, 1.2, -1.6], [8.0, 1.6, 1.6]);
    let left = g.box_mesh("Left", [2.0, 0.0, -1.6], [8.0, 1.2, -1.2]);
    let right = g.box_mesh("Right", [2.0, 0.0, 1.2], [8.0, 1.2, 1.6]);
    let ledge = g.box_mesh("Ledge", [14.0, 0.0, -2.0], [18.0, 1.7, 2.0]);
    let solid = json!({"collider": "auto"});
    for (name, mesh) in [("Floor", floor), ("Ceiling", ceiling), ("Left", left), ("Right", right), ("Ledge", ledge)] {
        g.root(NodeSpec::new(name).mesh(mesh).props(solid.clone()));
    }
    g
}

fn room_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/levels/crouch.gltf")
}

#[test]
fn the_committed_room_is_what_the_builder_writes() {
    let want = room().gltf();
    if std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(room_file().parent().unwrap()).unwrap();
        std::fs::write(room_file(), &want).unwrap();
    }
    let have = std::fs::read_to_string(room_file()).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", room_file().display()));
    assert!(have == want, "assets/levels/crouch.gltf is out of date; regenerate it (see the module docs)");
}

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

/// The game with the room laid on the canyon floor and the player, a `crouch_viewer`, at its origin.
fn in_the_room() -> App {
    let mut app = settled();
    let placed = ok(&mut app, "spawn_level", json!({"id": "crouch", "x": ORIGIN.x, "y": ORIGIN.y, "z": ORIGIN.z}));
    assert!(text(&placed).contains("spawned level `crouch`"), "{}", text(&placed));
    ok(&mut app, "set", json!({"id": PLAYER, "component": "CharacterController", "path": "params", "value": "crouch_viewer"}));
    at(&mut app, 0.0, 0.0);
    app
}

/// The player at (`x`, `z`) in the room's frame, standing on its floor.
fn at(app: &mut App, x: f32, z: f32) {
    ok(app, "teleport", json!({"id": PLAYER, "x": ORIGIN.x + x, "z": ORIGIN.z + z, "y": ORIGIN.y}));
    step(app, 30);
}

fn entity(app: &App, id: &str) -> Entity {
    app.world().resource::<AiIndex>().get(AiId::parse(id).unwrap()).unwrap()
}

fn feet(app: &App, id: &str) -> Vec3 {
    app.world().get::<Transform>(entity(app, id)).unwrap().translation - ORIGIN
}

/// How far above its feet a body's eye is.
fn eye_above_feet(app: &App, id: &str) -> f32 {
    let e = entity(app, id);
    eye_of(app.world(), e).unwrap().y - app.world().get::<Transform>(e).unwrap().translation.y
}

fn crouch_of(app: &App, id: &str) -> Crouch {
    app.world().get::<Crouch>(entity(app, id)).copied().unwrap_or_default()
}

fn mode_of(app: &App, id: &str) -> MotionMode {
    app.world().get::<MotionState>(entity(app, id)).unwrap().mode
}

fn height_of(app: &App, id: &str) -> f32 {
    use shards::motion::Shape;
    match app.world().get::<shards::motion::Collider>(entity(app, id)).unwrap().shape {
        Shape::Capsule { half_height, radius } => 2.0 * (half_height + radius),
        other => panic!("not a capsule: {other:?}"),
    }
}

/// Walks east for up to `ticks`, stopping as soon as the feet are past `x` (room frame).
fn walk_east_until(app: &mut App, x: f32, ticks: u64) {
    ok(app, "move", json!({"dir": "E", "meters": 40}));
    for _ in 0..ticks {
        if feet(app, PLAYER).x >= x {
            break;
        }
        step(app, 1);
    }
}

#[test]
fn an_agent_cannot_stand_up_in_the_duct_and_passes_it_crouched() {
    let mut app = in_the_room();
    // Standing, the duct's mouth stops it.
    at(&mut app, 0.0, 0.0);
    walk_east_until(&mut app, 8.0, 200);
    assert!(feet(&app, PLAYER).x < 1.8, "stopped at the duct: {}", feet(&app, PLAYER));
    ok(&mut app, "stop", json!({}));

    // Crouched, it goes in.
    let said = text(&ok(&mut app, "crouch", json!({}))).to_owned();
    assert!(said.starts_with("ok: crouch held; you go down to 1.0m over 0.20s"), "{said}");
    step(&mut app, 20);
    assert_eq!(crouch_of(&app, PLAYER).depth, 1.0);
    walk_east_until(&mut app, 5.0, 300);
    assert!((5.0..6.0).contains(&feet(&app, PLAYER).x), "inside the duct: {}", feet(&app, PLAYER));
    ok(&mut app, "stop", json!({}));
    step(&mut app, 10);

    // Letting go in there does nothing: the ceiling is 0.2 m above the crouched head.
    let said = text(&ok(&mut app, "crouch", json!({"hold": false}))).to_owned();
    assert!(said.contains("no room to stand") && said.contains("above your head"), "{said}");
    step(&mut app, 60);
    assert_eq!(crouch_of(&app, PLAYER), Crouch { depth: 1.0, wanted: false, blocked: true });
    assert!((height_of(&app, PLAYER) - 1.0).abs() < 1e-4);
    let observed = observe(&mut app, "dev");
    assert!(observed.contains("crouched, no room to stand"), "{observed}");
    assert!(!events(&app).iter().any(|l| l.contains("e5 stands up")), "{:#?}", events(&app));

    // Out the far end it stands, once, and the journal says so.
    walk_east_until(&mut app, 9.5, 400);
    step(&mut app, 30);
    assert_eq!(crouch_of(&app, PLAYER), Crouch::default(), "feet at {}", feet(&app, PLAYER));
    assert!((height_of(&app, PLAYER) - 1.8).abs() < 1e-4);
    let lines = events(&app);
    assert_eq!(lines.iter().filter(|l| l.ends_with("e5 crouches")).count(), 1, "{lines:#?}");
    assert_eq!(lines.iter().filter(|l| l.ends_with("e5 stands up")).count(), 1, "{lines:#?}");
    ok(&mut app, "stop", json!({}));
}

/// A run at the ledge from `from` (x, room frame), jumping at x 11.9 (2.1 m short of its face), crouching 3 ticks
/// into the jump when `crouch`; where the body is after 100 more ticks.
fn jump_the_ledge(crouch: bool) -> (Vec3, MotionMode) {
    let mut app = in_the_room();
    at(&mut app, 9.0, 0.0);
    walk_east_until(&mut app, 11.9, 120);
    ok(&mut app, "jump", json!({}));
    for tick in 0..100 {
        if crouch && tick == 3 {
            ok(&mut app, "crouch", json!({}));
        }
        step(&mut app, 1);
    }
    (feet(&app, PLAYER), mode_of(&app, PLAYER))
}

#[test]
fn a_crouch_jump_clears_the_ledge_that_a_plain_jump_misses() {
    let (plain, _) = jump_the_ledge(false);
    assert!(plain.x < 14.0 && plain.y < 0.2, "a jump peaks 1.2 m up and the ledge is 1.7 m: it stays below it: {plain}");
    let (crouched, mode) = jump_the_ledge(true);
    assert!(crouched.x > 14.4 && (crouched.y - 1.7).abs() < 0.1 && mode == MotionMode::Ground, "on top of the ledge: {crouched} {mode:?}");
}

fn bot(app: &mut App, who: &str, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), who, tool, args)
}

fn bot_ok(app: &mut App, who: &str, tool: &str, args: Value) -> String {
    let out = bot(app, who, tool, args.clone());
    assert!(out.ok, "{who}: {tool} {args}: {}", out.result);
    text(&out).to_owned()
}

fn observe(app: &mut App, who: &str) -> String {
    bot_ok(app, who, "observe", json!({}))
}

fn play_as(app: &mut App, who: &str, subject: &str) {
    let change = SessionChange { scope: Some(Scope::Player), subject: AiId::parse(subject), ..Default::default() };
    configure_session(app.world_mut(), who, &change).unwrap();
}

fn spawn(app: &mut App, prefab: &str, x: f32, z: f32) -> String {
    let spawned = ok(app, "spawn_prefab", json!({"id": prefab, "x": x, "z": z, "y": 0}));
    text(&spawned).split_whitespace().nth(1).expect("the spawn names the id").to_owned()
}

#[test]
fn a_crouched_dummy_is_a_smaller_target_whose_head_stays_a_head_and_a_crouched_shooter_shoots_from_a_lower_eye() {
    let mut app = settled();
    let me = spawn(&mut app, "viewer", -18.0, 0.0);
    ok(&mut app, "set", json!({"id": me, "component": "CharacterController", "path": "params", "value": "crouch_viewer"}));
    ok(&mut app, "give_weapon", json!({"id": me, "moveset": "hitscan_rifle"}));
    let dummy = spawn(&mut app, "crouch_dummy", -13.0, 0.0);
    play_as(&mut app, "shooter", &me);
    play_as(&mut app, "dummy", &dummy);
    step(&mut app, 3);
    bot_ok(&mut app, "shooter", "look", json!({"yaw": -90, "pitch": 0}));
    let trace = |app: &mut App| bot(app, "shooter", "shot_trace", json!({})).result;
    // The parts' heights above the dummy's feet.
    let parts = |app: &App| -> Vec<(String, f32)> {
        let world = app.world();
        let body = AiId::parse(&dummy).unwrap();
        let feet = world.get::<Transform>(entity(app, &dummy)).unwrap().translation.y;
        let mut found: Vec<(String, f32)> = world
            .iter_entities()
            .filter_map(|e| Some((e.get::<Part>()?, e.get::<Transform>()?)))
            .filter(|(p, _)| p.body == body)
            .map(|(p, tf)| (p.id.clone(), tf.translation.y - feet))
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    };

    // Standing: the level ray from the 1.6 m eye meets the head, which is 1.6 m up.
    let standing = trace(&mut app);
    assert_eq!(standing["hits"][0]["part"], json!("head"), "{standing}");
    let standing_parts = parts(&app);
    assert!((standing_parts[0].1 - 1.6).abs() < 1e-3 && (standing_parts[1].1 - 1.0).abs() < 1e-3, "{standing_parts:?}");
    let high = eye_above_feet(&app, &dummy);
    assert!((high - 1.6).abs() < 1e-4, "{high}");

    // The dummy crouches: its capsule is 1.0 m, its eye 1.6 / 1.8 of that, and its parts follow it.
    let said = bot_ok(&mut app, "dummy", "crouch", json!({}));
    assert!(said.starts_with("ok: crouch held"), "{said}");
    step(&mut app, 20);
    assert_eq!(crouch_of(&app, &dummy).depth, 1.0);
    let low = eye_above_feet(&app, &dummy);
    assert!((low - 1.6 / 1.8).abs() < 1e-4, "{low}");
    let (head, torso) = {
        let found = parts(&app);
        (found[0].1, found[1].1)
    };
    assert!((head - 0.95).abs() < 1e-3 && (torso - 0.45).abs() < 1e-3, "{head} {torso}");
    // The hurtbox shrank with the capsule: the ray at 1.6 m passes over a dummy that is 1.0 m tall.
    let over = trace(&mut app);
    assert_eq!(over["hits"], json!([]), "{over}");
    // Aimed at its head, the shot meets the head where it is now and hits it for double.
    bot_ok(&mut app, "shooter", "look", json!({"at": dummy, "part": "head"}));
    let aimed = trace(&mut app);
    assert_eq!(aimed["hits"][0]["part"], json!("head"), "{aimed}");
    bot_ok(&mut app, "shooter", "fire", json!({}));
    step(&mut app, 3);
    let hit = events(&app).into_iter().find(|l| l.contains("fires hitscan_rifle_shot")).expect("the shot's line");
    assert!(hit.contains(&format!("{dummy} head -48 hp")), "{hit}");

    // The shooter crouches too: its shots start from the lower eye.
    bot_ok(&mut app, "shooter", "crouch", json!({}));
    step(&mut app, 20);
    bot_ok(&mut app, "shooter", "look", json!({"yaw": -90, "pitch": 0}));
    let from = trace(&mut app)["from"].as_str().unwrap().to_owned();
    assert!(from.contains(", 0.9,"), "a shot starts at the crouched eye (0.89 m): {from}");
    let aim = observe(&mut app, "shooter");
    assert!(aim.lines().any(|l| l.starts_with("AIM")), "{aim}");
}

#[test]
fn a_crouched_body_rides_a_lift_as_a_standing_one_does() {
    let mut app = settled();
    let placed = ok(&mut app, "spawn_level", json!({"id": "movers", "x": 112.0, "y": -10.0, "z": 0.0}));
    assert!(text(&placed).contains("spawned level `movers`"), "{}", text(&placed));
    ok(&mut app, "set", json!({"id": PLAYER, "component": "CharacterController", "path": "params", "value": "crouch_viewer"}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 112.0, "z": -10.0, "y": -9.7}));
    step(&mut app, 30);
    ok(&mut app, "crouch", json!({}));
    step(&mut app, 20);
    assert_eq!(crouch_of(&app, PLAYER).depth, 1.0);
    let before = app.world().get::<Transform>(entity(&app, PLAYER)).unwrap().translation;
    ok(&mut app, "mover_go", json!({"name": "Lift", "key": 1}));
    step(&mut app, 260);
    let after = app.world().get::<Transform>(entity(&app, PLAYER)).unwrap().translation;
    assert!((after.y - before.y - 4.0).abs() < 0.05, "the lift took it 4 m up: {before} -> {after}");
    assert!((after.x - before.x).abs() < 0.05 && (after.z - before.z).abs() < 0.05, "and it did not slide: {before} -> {after}");
    assert_eq!(crouch_of(&app, PLAYER).depth, 1.0);
    assert_eq!(mode_of(&app, PLAYER), MotionMode::Ground);
    assert!(fails(&mut app, "mover_go", json!({"name": "Nothing", "key": 1})).contains("Nothing"));
    ok(&mut app, "level_despawn", json!({"id": "movers"}));
}

/// A client crouches its own body (the host gave it the `crouch_viewer` locomotion): the client predicts the stance and
/// the capsule, the host's state agrees with it, nothing snaps or desyncs, and the host's recording plays the same alone.
#[test]
fn a_client_crouches_its_own_body_and_the_host_agrees() {
    use crate::net::{body as body_of, desyncs, entity as entity_of, lossy, stand_with, verify_host, C1, HOST};

    let mut stand = stand_with(1, lossy());
    for id in ["e6", "e7", "e8", "e14"] {
        let out = stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"}));
        assert!(out.ok, "{}", out.result);
    }
    let id = body_of(&stand, C1);
    let set = stand.call(HOST, "set", json!({"id": id, "component": "CharacterController", "path": "params", "value": "crouch_viewer"}));
    assert!(set.ok, "{}", set.result);
    stand.run(60);
    let states = |stand: &shards::net::Stand| -> Vec<(f32, f32)> {
        [HOST, C1]
            .into_iter()
            .map(|member| {
                let world = stand.world(member);
                let e = entity_of(stand, member, &id).expect("the body");
                let height = match world.get::<shards::motion::Collider>(e).unwrap().shape {
                    shards::motion::Shape::Capsule { half_height, radius } => 2.0 * (half_height + radius),
                    other => panic!("not a capsule: {other:?}"),
                };
                (world.get::<Crouch>(e).map_or(0.0, |c| c.depth), height)
            })
            .collect()
    };
    let said = stand.call(C1, "crouch", json!({}));
    assert!(said.ok, "{}", said.result);
    stand.run(150);
    for (member, (depth, height)) in states(&stand).into_iter().enumerate() {
        assert!(depth == 1.0 && (height - 1.0).abs() < 1e-4, "member {member} is crouched: depth {depth}, height {height}");
    }
    let up = stand.call(C1, "crouch", json!({"hold": false}));
    assert!(up.ok, "{}", up.result);
    stand.run(150);
    for (member, (depth, height)) in states(&stand).into_iter().enumerate() {
        assert!(depth == 0.0 && (height - 1.8).abs() < 1e-4, "member {member} stands: depth {depth}, height {height}");
    }
    assert_eq!(desyncs(&stand, C1), 0);
    verify_host(&stand);
}

#[test]
fn the_crouch_replay_continues_alike_from_its_snapshots() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/crouch.json");
    let out = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .arg("--replay")
        .arg(file)
        .args(["--verify", "--heavy", "10"])
        .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let verdict = stdout.lines().find(|l| l.starts_with("replay ")).unwrap_or_default();
    assert!(verdict.contains("identical to the recording"), "{stdout}");
    assert!(verdict.contains(", 0 skipped"), "{verdict}");
    let checked: u64 = verdict.split("heavy: ").nth(1).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(checked >= 30, "a twin every 10 ticks: {verdict}");
}


