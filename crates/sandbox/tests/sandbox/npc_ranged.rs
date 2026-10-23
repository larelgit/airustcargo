//! NPCs and gunfire (F11), end to end on the sandbox, with the bodies of `content/characters/ranged.prefab.ron`
//! in the yard's gate (it has a navmesh): bystanders that hide from a shooter, a gunner that keeps its band and
//! holds its fire while a body of its own side stands in its line, a strafing gunner that hits less, and a
//! turret that tracks within its limits. What `scenarios/npc_ranged.scenario` cannot say, because it names the
//! ids of spawned bodies or counts ticks: where each one stands, every tick; and the reference replay in heavy
//! mode.

use std::path::Path;
use std::process::Command;

use shards::ai_core::AiIndex;
use shards::core::Intent;
use shards::npc::{hide_status, NpcBrain, NpcFireLine, Perceived, Turret};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{events, get, lines, ok, step, text, PLAYER};

/// The yard loaded, the player unhurtable at (x, z) in its gate.
fn yard(x: f32, z: f32) -> App {
    let mut app = sandbox::test_app(42);
    let loaded = ok(&mut app, "regen", json!({"region": "yard", "seed": 1}));
    assert!(text(&loaded).starts_with("ok: yard seed 1"), "{}", text(&loaded));
    ok(&mut app, "god", json!({}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": x, "z": z, "y": 0}));
    app
}

/// Spawns `prefab` at (x, z) as part of the yard and returns its id (`spawned e16 ranged_wall at ...`).
fn spawn(app: &mut App, prefab: &str, x: f32, z: f32) -> String {
    let spawned = ok(app, "spawn_prefab", json!({"id": prefab, "x": x, "z": z, "y": 0, "overrides": {"RegionPart": {}}}));
    text(&spawned).split_whitespace().nth(1).expect("the spawn names the id").to_owned()
}

fn entity(app: &App, id: &str) -> Entity {
    app.world().resource::<AiIndex>().get(AiId::parse(id).unwrap()).unwrap()
}

fn feet(app: &App, id: &str) -> Vec3 {
    get::<Transform>(app, id).translation
}

fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

fn npc_state(app: &App, id: &str) -> String {
    get::<NpcBrain>(app, id).state
}

/// The journal's shots of `id` and how many of them hit the player (`e25 fires ranged_burst: e5 -0 (god mode)`).
fn shots(app: &App, id: &str) -> (usize, usize) {
    let fired: Vec<String> = events(app).into_iter().filter(|l| l.contains(&format!(" {id} fires "))).collect();
    let hits = fired.iter().filter(|l| l.contains(&format!(": {PLAYER} -"))).count();
    (fired.len(), hits)
}

/// The player fires the test rifle into the air: a shot heard 30 m around.
fn fire_in_the_air(app: &mut App) {
    ok(app, "give_weapon", json!({"id": PLAYER, "moveset": "hitscan_rifle"}));
    step(app, 3);
    ok(app, "look", json!({"yaw": 90, "pitch": 20}));
    let fired = ok(app, "fire", json!({}));
    assert_eq!(text(&fired), "ok: hitscan_rifle_shot");
}

#[test]
fn bystanders_run_out_of_the_shooters_sight_and_stay_there() {
    let mut app = yard(206.0, -184.0);
    spawn(&mut app, "ranged_wall", 214.0, -176.0);
    let a = spawn(&mut app, "ranged_bystander", 211.0, -176.0);
    spawn(&mut app, "ranged_wall", 216.0, -193.0);
    let b = spawn(&mut app, "ranged_bystander", 211.0, -192.0);
    step(&mut app, 30);
    // A non-combatant notices threats only: the player stands 9.4 m away, on another team, unarmed.
    for id in [&a, &b] {
        assert_eq!(npc_state(&app, id), "idle");
        assert!(get::<Perceived>(&app, id).known.is_empty(), "{id} notices nobody yet");
    }
    fire_in_the_air(&mut app);
    step(&mut app, 3);
    for id in [&a, &b] {
        assert_eq!(npc_state(&app, id), "hide", "{id} heard the shot");
        let state = ok(&mut app, "npc_state", json!({"id": id})).result;
        assert_eq!(state["threats"][0]["id"], PLAYER, "{state:#}");
        assert_eq!(state["hiding"]["from"], PLAYER, "{state:#}");
        assert!(state["hiding"]["spot"].is_string(), "{state:#}");
    }
    let spots: Vec<Vec3> = [&a, &b].iter().map(|id| hide_status(app.world(), entity(&app, id)).unwrap().0.unwrap()).collect();
    // Both get there...
    let mut arrived = [false, false];
    for _ in 0..600 {
        step(&mut app, 1);
        for (k, id) in [&a, &b].iter().enumerate() {
            arrived[k] |= flat_distance(feet(&app, id), spots[k]) <= 0.6;
        }
        if arrived == [true, true] {
            break;
        }
    }
    assert_eq!(arrived, [true, true], "{:?} vs {spots:?}", [feet(&app, &a), feet(&app, &b)]);
    // ... and stay there, out of the shooter's sight, for the next 10 s.
    for tick in 0..600 {
        step(&mut app, 1);
        for (k, id) in [&a, &b].iter().enumerate() {
            let at = feet(&app, id);
            assert!(flat_distance(at, spots[k]) <= 0.6, "{id} left its spot at tick {tick}: {at} vs {}", spots[k]);
            let me = get::<Perceived>(&app, id);
            assert!(me.known.iter().all(|k| !k.seen), "{id} sees the shooter again at tick {tick}: {me:?}");
            assert_eq!(npc_state(&app, id), "hide");
        }
    }
    let overlay = ok(&mut app, "npc_overlay", json!({}));
    assert_eq!(text(&overlay).matches(" · hiding").count(), 2, "{}", text(&overlay));
}

#[test]
fn a_bystander_with_nowhere_to_hide_cowers() {
    // The arena has no navmesh: no hiding spot anywhere, so it cowers where it stands (and looks again).
    let mut app = sandbox::test_app(42);
    for id in ["e6", "e7", "e8", "e14"] {
        ok(&mut app, "npc_set_state", json!({"id": id, "state": "calm"}));
    }
    ok(&mut app, "god", json!({}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 17, "y": 0}));
    let a = spawn(&mut app, "ranged_bystander", 6.0, 17.0);
    step(&mut app, 10);
    fire_in_the_air(&mut app);
    step(&mut app, 4);
    assert_eq!(npc_state(&app, &a), "cower", "{:#?}", events(&app));
    let here = feet(&app, &a);
    let state = ok(&mut app, "npc_state", json!({"id": a})).result;
    assert_eq!(state["hiding"]["cowering"], true, "{state:#}");
    assert!(state["line"].as_str().unwrap().starts_with("cower"), "{state:#}");
    let overlay = ok(&mut app, "npc_overlay", json!({}));
    assert!(text(&overlay).contains(" · cowering"), "{}", text(&overlay));
    // Every 2 s it looks again (`hide`), finds nothing and cowers on; it never runs.
    step(&mut app, 300);
    assert!(flat_distance(feet(&app, &a), here) < 0.3, "{} from {here}", feet(&app, &a));
    assert!(events(&app).iter().any(|l| l.contains(&format!("{a} cower → hide"))), "{:#?}", events(&app));
}

#[test]
fn a_gunner_keeps_its_band_and_never_fires_through_an_ally() {
    let mut app = yard(206.0, -184.0);
    let post = spawn(&mut app, "ranged_post", 214.0, -184.0);
    let gunner = spawn(&mut app, "ranged_gunner", 224.0, -184.0);
    // 18 m away: it closes in to its band, and holds its fire, the post being in its line.
    let mut entered = None;
    for tick in 0..300 {
        step(&mut app, 1);
        let d = flat_distance(feet(&app, &gunner), feet(&app, PLAYER));
        if d <= 14.0 {
            entered.get_or_insert(tick);
        }
        if entered.is_some() {
            assert!((7.9..=14.2).contains(&d), "out of its band at tick {tick}: {d}");
        }
    }
    assert!(entered.is_some(), "it came into its band");
    assert_eq!(shots(&app, &gunner).0, 0, "{:#?}", events(&app));
    let line = get::<NpcFireLine>(&app, &gunner);
    assert_eq!(line.blocked.and_then(|b| b.ally()), Some(AiId::parse(&post).unwrap()), "{line:?}");
    let schedule = ok(&mut app, "npc_schedule", json!({"id": gunner})).result;
    let refused = format!("no line of fire: {post} in the way");
    assert_eq!(schedule["options"][0]["refused"], refused.as_str(), "{schedule:#}");
    assert_eq!(schedule["line_of_fire"], format!("{post} in the way"), "{schedule:#}");
    let overlay = ok(&mut app, "npc_overlay", json!({}));
    assert!(text(&overlay).contains(&format!("keeping range 8–14m · {refused}")), "{}", text(&overlay));
    // The player steps aside: the line is clear and it fires, never through the post.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 206, "z": -180, "y": 0}));
    step(&mut app, 600);
    // (A shot may miss: its cone is 1.5 degrees, 0.4 m at 14 m.)
    let (fired, hits) = shots(&app, &gunner);
    assert!(fired >= 9 && hits + 2 >= fired, "{fired} shots, {hits} hits: {:#?}", events(&app));
    assert!(lines(&app, &format!(": {post} ")).is_empty(), "never at the post: {:#?}", events(&app));
    assert_eq!(get::<Health>(&app, &post).current, 60.0, "never through the post");
    // The player walks up to 3.8 m: it backs off to its band.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 216, "z": -182, "y": 0}));
    step(&mut app, 120);
    let d = flat_distance(feet(&app, &gunner), feet(&app, PLAYER));
    assert!((7.9..=14.2).contains(&d), "back in its band: {d}");
}

#[test]
fn a_gunner_hits_less_while_it_moves() {
    // The same burst from 12 m, standing and strafing along the band (`aim_error_moving` 4: a cone four
    // times as wide at a run).
    let mut rates = Vec::new();
    for prefab in ["ranged_gunner", "ranged_strafer"] {
        let mut app = yard(206.0, -180.0);
        let gunner = spawn(&mut app, prefab, 218.0, -180.0);
        step(&mut app, 2400);
        let (fired, hits) = shots(&app, &gunner);
        assert!(fired >= 30, "{prefab}: {fired} shots");
        rates.push(hits as f32 / fired as f32);
    }
    assert!(rates[0] > 0.9, "standing, it hits: {rates:?}");
    assert!(rates[1] < 0.6, "strafing, it misses more: {rates:?}");
}

#[test]
fn a_turret_tracks_within_its_limits_and_fires_when_its_look_is_on() {
    let mut app = yard(210.0, -186.0);
    let turret = spawn(&mut app, "ranged_turret", 216.0, -176.0);
    let degrees = |app: &App| {
        let rest = get::<Turret>(app, &turret).rest.unwrap();
        let look = get::<Intent>(app, &turret).look.unwrap();
        let yaw = (look.yaw - rest + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        (yaw.to_degrees(), look.pitch.to_degrees())
    };
    let within = |app: &App| {
        let (yaw, pitch) = degrees(app);
        assert!(yaw.abs() <= 60.0 + 1e-3 && (-20.0 - 1e-3..=30.0 + 1e-3).contains(&pitch), "outside its limits: {yaw} {pitch}");
    };
    step(&mut app, 2);
    assert_eq!(get::<Turret>(&app, &turret).rest, Some(0.0), "it rests the way it was placed: north");
    // The player 31 degrees to its left: it turns there at 60 degrees a second and fires.
    for _ in 0..120 {
        step(&mut app, 1);
        within(&app);
    }
    let (yaw, _) = degrees(&app);
    assert!((yaw - 31.0).abs() < 1.0, "{yaw}");
    let (fired, hits) = shots(&app, &turret);
    assert!(fired >= 2 && hits == fired, "{fired} shots, {hits} hits");
    let overlay = ok(&mut app, "npc_overlay", json!({}));
    assert!(text(&overlay).contains(&format!("turret tracking {PLAYER} (yaw 31°/60°)")), "{}", text(&overlay));
    // 80 degrees to its right: it stops at its limit and holds its fire.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 228, "z": -178, "y": 0}));
    for _ in 0..180 {
        step(&mut app, 1);
        within(&app);
    }
    assert!((degrees(&app).0 + 60.0).abs() < 1e-3, "{:?}", degrees(&app));
    let before = shots(&app, &turret).0;
    step(&mut app, 120);
    assert_eq!(shots(&app, &turret).0, before, "no shot from its limit");
    let overlay = ok(&mut app, "npc_overlay", json!({}));
    assert!(text(&overlay).contains("(yaw -60°/60°, at its limit)"), "{}", text(&overlay));
    // Off: back to its rest, no shot even with the player in front of it; on again: it fires.
    let off = ok(&mut app, "turret_active", json!({"id": turret, "on": false}));
    assert_eq!(text(&off), format!("ok: {turret} turret off"));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 212, "z": -186, "y": 0}));
    step(&mut app, 120);
    assert_eq!(degrees(&app), (0.0, 0.0));
    assert_eq!(shots(&app, &turret).0, before);
    assert!(events(&app).iter().any(|l| l.ends_with(&format!("{turret} turret off"))), "{:#?}", events(&app));
    ok(&mut app, "turret_active", json!({"on": true}));
    step(&mut app, 120);
    assert!(shots(&app, &turret).0 > before, "on again, it fires");
    let refused = crate::common::fails(&mut app, "turret_active", json!({"id": PLAYER, "on": true}));
    assert!(refused.contains("not a turret"), "{refused}");
}

#[test]
fn the_npc_ranged_replay_continues_alike_from_its_snapshots() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/npc_ranged.json");
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
