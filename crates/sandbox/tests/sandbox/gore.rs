//! Gore in the sandbox (F9), on its own content (`content/fx/gore.*.ron`, linked in `sandbox.fx.ron`): shots from the
//! hitscan rifle at a room built from `gore.prefab.ron`, 40 m above the arena, leave holes on the wall, blood on the wall
//! behind a dummy and on the floor under it, and a part that breaks off throws chunks that bounce and lie where they
//! landed. A game that counts as drawing (the offscreen image stands in for a GPU) lays all of it; one that does not lays
//! nothing, and plays the very same game: the replay's state hash is the same. The pixels are
//! `crates/shards_fx/tests/fx/offscreen_gore.rs`'s.

use shards::ai_core::display::Offscreen;
use shards::ai_core::replay;
use shards::fx::{clear_all, debris_counts, spawn_mark, Marks, Particles};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{ok, step, text, PLAYER};

/// The sandbox's game; with `drawn`, one that counts as drawing a picture (no GPU needed to lay marks).
fn game(drawn: bool) -> App {
    let mut app = sandbox::test_app(42);
    if drawn {
        app.world_mut().insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
    }
    app
}

/// Spawns `prefab` at (x, y, z) and returns its id (`spawned e15 gore_floor at (0.0, 40.0, 0.0)`).
fn spawn(app: &mut App, prefab: &str, x: f32, y: f32, z: f32) -> String {
    let spawned = ok(app, "spawn_prefab", json!({"id": prefab, "x": x, "y": y, "z": z}));
    text(&spawned).split_whitespace().nth(1).expect("the spawn names the id").to_owned()
}

/// Everyone stands down, a room is built 40 m up (a floor, a wall 6 m ahead, no ceiling), the player stands in it with the
/// rifle, and a dummy stands 3 m ahead and a metre to the left. Returns the dummy's id.
fn room(app: &mut App) -> String {
    for id in ["e6", "e7", "e8", "e14"] {
        ok(app, "npc_set_state", json!({"id": id, "state": "calm"}));
    }
    ok(app, "teleport", json!({"id": "e6", "x": 14, "z": -16, "y": 0}));
    ok(app, "teleport", json!({"id": "e7", "x": -16, "z": 16, "y": 0}));
    ok(app, "teleport", json!({"id": "e8", "x": 14, "z": 16, "y": 0}));
    ok(app, "teleport", json!({"id": "e14", "x": -16, "z": -16, "y": 0}));
    ok(app, "set", json!({"id": PLAYER, "component": "CharacterController", "path": "params", "value": "\"viewer\""}));
    spawn(app, "gore_floor", 0.0, 40.0, 0.0);
    spawn(app, "gore_wall", 0.0, 40.0, -6.0);
    let dummy = spawn(app, "range_dummy", -1.0, 40.0, -3.0);
    ok(app, "teleport", json!({"id": PLAYER, "x": 0, "y": 40, "z": 0}));
    ok(app, "give_weapon", json!({"id": PLAYER, "moveset": "hitscan_rifle"}));
    step(app, 5);
    dummy
}

/// The shots: two at the dummy's torso from a side (the wall behind it takes the blood), one at the wall, one into the floor;
/// then the dummy's head breaks off.
fn session(app: &mut App, dummy: &str) {
    ok(app, "teleport", json!({"id": PLAYER, "x": -4, "y": 40, "z": 1}));
    step(app, 3);
    for _ in 0..2 {
        ok(app, "look", json!({"at": dummy, "part": "torso"}));
        ok(app, "fire", json!({}));
        step(app, 25);
    }
    ok(app, "look", json!({"yaw": -30, "pitch": -4}));
    ok(app, "fire", json!({}));
    step(app, 25);
    ok(app, "look", json!({"yaw": 0, "pitch": -30}));
    ok(app, "fire", json!({}));
    step(app, 25);
    ok(app, "part", json!({"id": dummy, "name": "head", "op": "break"}));
    step(app, 300);
    // A frame: the chunks are stepped on the picture's side, up to the clock the ticks moved.
    app.update();
}

fn live(app: &App, kind: &str) -> usize {
    app.world().resource::<Marks>().kinds.get(kind).map_or(0, |pool| pool.live())
}

#[test]
fn shots_in_the_room_leave_holes_blood_behind_the_dummy_and_under_it_and_a_broken_part_throws_chunks_that_stay() {
    let mut app = game(true);
    let dummy = room(&mut app);
    session(&mut app, &dummy);
    // Two shots through the dummy: a splatter on the wall behind it each (two a shot) and, from the blows, a pool under it
    // each; the wall shot and the floor shot stop at their surface and leave a hole each.
    assert_eq!(live(&app, "blood_splat"), 4, "two shots, two splatters each");
    assert_eq!(live(&app, "blood_pool"), 2);
    assert_eq!(live(&app, "bullet_hole"), 2, "one in the wall, one in the floor");
    let marks = app.world().resource::<Marks>();
    let holes: Vec<_> = marks.kinds["bullet_hole"].slots.iter().flatten().collect();
    assert!(holes.iter().any(|h| h.normal.z > 0.9 && (h.point.z + 5.75).abs() < 0.01), "a hole in the wall's face: {holes:?}");
    assert!(holes.iter().any(|h| h.normal.y > 0.9 && (h.point.y - 40.0).abs() < 0.01), "a hole in the floor: {holes:?}");
    let splats: Vec<_> = marks.kinds["blood_splat"].slots.iter().flatten().collect();
    assert!(splats.iter().all(|s| (s.point.z + 5.75).abs() < 0.01 && s.normal.z > 0.9), "on the wall behind the dummy: {splats:?}");
    assert!(
        marks.kinds["blood_pool"].slots.iter().flatten().all(|p| p.normal.y > 0.9 && (p.point.y - 40.0).abs() < 0.01),
        "the pools lie on the floor"
    );
    // The head came off (a part break with no attacker: no splatter behind it): five chunks that landed and stay.
    assert_eq!(debris_counts(app.world()), (5, 0), "five chunks, all at rest");
    let pool = &app.world().resource::<Particles>().effects["meat_chunk"].pool;
    assert!(pool.slots.iter().flatten().all(|p| (p.body.unwrap().to.y - 40.0).abs() < 0.2), "on the floor, a chunk's size up");
    let said = ok(&mut app, "effects", json!({}));
    let said = text(&said);
    assert!(said.contains("meat_chunk") && said.contains("(5 at rest)"), "{said}");
    assert!(said.contains("debris: 5 at rest, 0 in flight"), "{said}");
    // A new region takes all of it away.
    ok(&mut app, "regen", json!({"region": "yard", "seed": 1}));
    app.world_mut().flush();
    assert_eq!((app.world().resource::<Marks>().live(), app.world().resource::<Particles>().live()), (0, 0));
}

#[test]
fn a_game_that_draws_nothing_lays_nothing_and_plays_the_same_game() {
    let mut drawn = game(true);
    let mut dark = game(false);
    for app in [&mut drawn, &mut dark] {
        let dummy = room(app);
        session(app, &dummy);
    }
    assert_eq!(dark.world().resource::<Marks>().live(), 0, "no mark headless");
    assert_eq!(dark.world().resource::<Particles>().live(), 0, "no chunk headless");
    assert_eq!(debris_counts(dark.world()), (0, 0));
    assert!(drawn.world().resource::<Marks>().live() > 0);
    // The same game, tick for tick: what is drawn is not in the replay's state hash.
    assert_eq!(
        (dark.world().resource::<Time<Fixed>>().elapsed(), replay::state_hash(dark.world())),
        (drawn.world().resource::<Time<Fixed>>().elapsed(), replay::state_hash(drawn.world())),
        "the same ticks, the same state"
    );
    // The status counts what the picture holds, and says none headless.
    let status = |app: &App| shards::ai_core::status::status(app.world())["fx"].clone();
    assert_eq!((status(&dark)["marks"].as_u64(), status(&dark)["debris"]["resting"].as_u64()), (Some(0), Some(0)));
    assert_eq!((status(&drawn)["marks"].as_u64(), status(&drawn)["debris"]["resting"].as_u64()), (Some(8), Some(5)));
}

#[test]
fn ten_times_the_budget_of_every_kind_leaves_every_pool_at_its_cap() {
    let mut app = game(true);
    // The caps are the content's: 96 holes, 64 splatters, 32 pools of blood, 48 chunks.
    for (kind, cap) in [("bullet_hole", 96usize), ("blood_splat", 64), ("blood_pool", 32)] {
        for i in 0..cap * 10 {
            spawn_mark(app.world_mut(), kind, Vec3::new(i as f32 * 0.01, 40.0, 0.0), Vec3::Y).unwrap();
        }
        let pool = &app.world().resource::<Marks>().kinds[kind];
        assert_eq!((pool.live(), pool.started, pool.recycled), (cap, cap as u64 * 10, cap as u64 * 9), "{kind}");
    }
    for i in 0..96 {
        ok(&mut app, "effect_play", json!({"effect": "meat_chunk", "point": [i as f32 * 0.05, 41.0, 0.0]}));
    }
    let chunks = &app.world().resource::<Particles>().effects["meat_chunk"].pool;
    assert_eq!((chunks.live(), chunks.size(), chunks.started), (48, 48, 480), "ten times the pool of chunks");
    assert_eq!(clear_all(app.world_mut()).total(), 96 + 64 + 32 + 48);
}
