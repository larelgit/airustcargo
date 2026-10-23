//! Hitscan shots (F3), end to end on the sandbox: an agent with a first-person body (`viewer`) and the test
//! guns of `content/combat/hitscan.*.ron` at range dummies (`range_dummy`, a head that doubles a hit). What
//! `scenarios/hitscan.scenario` cannot say, because it names the ids of spawned bodies or depends on the tick:
//! how many shots kill, the scatter gun's pellets alike in two runs, the noise NPCs hear, and the reference
//! replay in heavy mode.

use std::path::Path;
use std::process::Command;

use shards::ai_core::{call_tool, configure_session, AiIndex, CallOutcome, Scope, SessionChange};
use shards::npc::Noise;
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::{events, get, ok, step, text, PLAYER};

/// The arena with its fighters standing down and away, the player out of the range.
fn settled() -> App {
    let mut app = sandbox::test_app(42);
    for id in ["e6", "e7", "e8", "e14"] {
        ok(&mut app, "npc_set_state", json!({"id": id, "state": "calm"}));
    }
    ok(&mut app, "teleport", json!({"id": "e6", "x": 14, "z": -16, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e7", "x": -16, "z": 16, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e8", "x": 14, "z": 16, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e14", "x": -16, "z": -16, "y": 0}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 17, "y": 0}));
    app
}

/// Spawns `prefab` at (x, z) and returns its id (`spawned e15 viewer at (0.0, 12.0)`).
fn spawn(app: &mut App, prefab: &str, x: f32, z: f32) -> String {
    let spawned = ok(app, "spawn_prefab", json!({"id": prefab, "x": x, "z": z, "y": 0}));
    text(&spawned).split_whitespace().nth(1).expect("the spawn names the id").to_owned()
}

fn bot(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "bot", tool, args)
}

fn bot_ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = bot(app, tool, args.clone());
    assert!(out.ok, "{tool} {args}: {}", out.result);
    text(&out).to_owned()
}

/// A shooter on the west side of the arena at (x, z), looking east, carrying `guns` (slot 1 in the hands),
/// played by the player-scope agent `bot`.
fn shooter(app: &mut App, x: f32, z: f32, guns: &[&str]) -> String {
    let me = spawn(app, "viewer", x, z);
    let change = SessionChange { scope: Some(Scope::Player), subject: AiId::parse(&me), ..Default::default() };
    configure_session(app.world_mut(), "bot", &change).unwrap();
    for gun in guns {
        ok(app, "give_weapon", json!({"id": me, "moveset": gun}));
    }
    step(app, 3);
    bot_ok(app, "look", json!({"yaw": -90, "pitch": 0}));
    me
}

fn hp(app: &App, id: &str) -> f32 {
    get::<Health>(app, id).current
}

fn dead(app: &App, id: &str) -> bool {
    let entity = app.world().resource::<AiIndex>().get(AiId::parse(id).unwrap()).unwrap();
    app.world().get::<Dead>(entity).is_some()
}

/// Shots aimed at `part` of `dummy` (the agent looks at it again before each, as the kick lifts the look)
/// until it dies; how many it took.
fn shots_to_kill(app: &mut App, dummy: &str, part: &str) -> u32 {
    for shots in 1..=20 {
        bot_ok(app, "look", json!({"at": dummy, "part": part}));
        let fired = bot_ok(app, "fire", json!({}));
        assert_eq!(fired, "ok: hitscan_rifle_shot");
        step(app, 25);
        if dead(app, dummy) {
            return shots;
        }
    }
    panic!("{dummy} still stands after 20 shots at its {part}: hp {}", hp(app, dummy));
}

#[test]
fn an_agent_kills_the_body_dummy_in_five_shots_and_the_head_dummy_in_three() {
    let mut app = settled();
    shooter(&mut app, -18.0, 0.0, &["hitscan_rifle"]);
    let near = spawn(&mut app, "range_dummy", -13.0, 0.0);
    let other = spawn(&mut app, "range_dummy", -13.0, -3.0);
    step(&mut app, 3);
    // The torso as written: 24 a shot at 5 m (inside the falloff's first 10 m), 100 hp.
    let body = shots_to_kill(&mut app, &near, "torso");
    // The head doubles it.
    let head = shots_to_kill(&mut app, &other, "head");
    assert_eq!((body, head), (5, 3), "{:#?}", events(&app));
    let lines: Vec<String> = events(&app).into_iter().filter(|l| l.contains("fires hitscan_rifle_shot")).collect();
    assert!(lines[0].ends_with(&format!("fires hitscan_rifle_shot: {near} torso -24 hp (hp 76)")), "{lines:#?}");
    assert!(lines[5].ends_with(&format!("fires hitscan_rifle_shot: {other} head -48 hp (hp 52)")), "{lines:#?}");
}

#[test]
fn a_dummy_behind_another_is_hit_only_by_a_shot_that_pierces_and_never_through_a_wall() {
    let mut app = settled();
    let me = shooter(&mut app, -18.0, -4.0, &["hitscan_rifle", "hitscan_piercer"]);
    let front = spawn(&mut app, "range_dummy", -13.0, -4.0);
    let back = spawn(&mut app, "range_dummy", -11.0, -4.0);
    step(&mut app, 3);
    bot_ok(&mut app, "fire", json!({}));
    step(&mut app, 25);
    assert_eq!((hp(&app, &front), hp(&app, &back)), (52.0, 100.0), "the rifle stops at the first body");
    bot_ok(&mut app, "weapon", json!({"slot": 2}));
    step(&mut app, 40);
    bot_ok(&mut app, "look", json!({"yaw": -90, "pitch": 0}));
    let trace = bot(&mut app, "shot_trace", json!({}));
    assert_eq!(trace.result["pierce"], json!(1), "{}", trace.result);
    let met: Vec<&str> = trace.result["hits"].as_array().unwrap().iter().map(|h| h["id"].as_str().unwrap()).collect();
    assert_eq!(met, [front.as_str(), back.as_str()], "{}", trace.result);
    assert_eq!(bot_ok(&mut app, "fire", json!({})), "ok: hitscan_piercer_shot");
    step(&mut app, 2);
    assert_eq!((hp(&app, &front), hp(&app, &back)), (12.0, 60.0), "the piercer hits both heads, 40 each");
    // A pillar between: nothing reaches the dummy behind it, whatever pierce says.
    let pillar = spawn(&mut app, "pillar", -14.0, 10.0);
    let hidden = spawn(&mut app, "range_dummy", -10.0, 10.0);
    ok(&mut app, "teleport", json!({"id": me, "x": -18, "z": 10, "y": 0}));
    step(&mut app, 40);
    bot_ok(&mut app, "look", json!({"yaw": -90, "pitch": 0}));
    let trace = bot(&mut app, "shot_trace", json!({}));
    assert_eq!(trace.result["hits"], json!([]), "{}", trace.result);
    assert_eq!(trace.result["stop"]["what"], json!(format!("{pillar} Pillar")), "{}", trace.result);
    bot_ok(&mut app, "fire", json!({}));
    step(&mut app, 2);
    assert_eq!(hp(&app, &hidden), 100.0);
    assert!(events(&app).iter().any(|l| l.ends_with(&format!("fires hitscan_piercer_shot: {pillar} Pillar 3.0m"))), "{:#?}", events(&app));
}

/// One scatter shot at a dummy 6 m east, in a fresh game: the shot's line and the dummy's health.
fn scatter() -> (String, f32) {
    let mut app = settled();
    shooter(&mut app, -18.0, 0.0, &["hitscan_scatter"]);
    let dummy = spawn(&mut app, "range_dummy", -12.0, 0.0);
    step(&mut app, 3);
    bot_ok(&mut app, "look", json!({"yaw": -90, "pitch": -4}));
    assert_eq!(bot_ok(&mut app, "fire", json!({})), "ok: hitscan_scatter_shot");
    step(&mut app, 2);
    let line = events(&app).into_iter().find(|l| l.contains("fires hitscan_scatter_shot")).expect("the shot's line");
    (line, hp(&app, &dummy))
}

#[test]
fn the_scatter_guns_pellets_spread_the_same_every_run() {
    let (first, hp_first) = scatter();
    let (second, hp_second) = scatter();
    assert_eq!(first, second);
    assert_eq!(hp_first, hp_second);
    assert!(first.contains("(8 pellets): "), "{first}");
    assert!(hp_first < 100.0, "some pellets hit the dummy 6 m away: {first}");
}

/// The noises NPCs heard, as `(source, radius)`.
#[derive(Resource, Default)]
struct Heard(Vec<(Option<Entity>, f32)>);

#[test]
fn a_shot_is_heard_as_far_as_its_noise() {
    let mut app = settled();
    app.world_mut().init_resource::<Heard>();
    app.world_mut().add_observer(|n: On<Noise>, mut heard: ResMut<Heard>| heard.0.push((n.event().source, n.event().radius)));
    let me = shooter(&mut app, -18.0, 0.0, &["hitscan_rifle"]);
    let shooter = app.world().resource::<AiIndex>().get(AiId::parse(&me).unwrap()).unwrap();
    // At the wall to the north: no hit, so only the shot is loud.
    bot_ok(&mut app, "look", json!({"yaw": 0, "pitch": 0}));
    bot_ok(&mut app, "fire", json!({}));
    step(&mut app, 2);
    assert_eq!(app.world().resource::<Heard>().0, [(Some(shooter), 30.0)]);
    assert!(events(&app).iter().any(|l| l.ends_with("fires hitscan_rifle_shot: wall 20.0m")), "{:#?}", events(&app));
}

#[test]
fn the_hitscan_replay_continues_alike_from_its_snapshots() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/hitscan.json");
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
