//! Snapshots with NPCs on the field: behaviour states, attack tokens, plans, routes, what
//! they perceive and remember come back, so a fight loaded from a snapshot goes on exactly
//! like the one it was taken in (the proof in `crates/sandbox/tests/sandbox/snapshots.rs`,
//! here for the NPC module's state).

use ai_core::snapshot::{self, differences};
use ai_core::{sim, AiId, AiIndex, AiJournal, JournalKind};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_core::Intent;
use shards_npc::prelude::*;

use crate::common::*;
use crate::replay::arena;

/// What the hero does, and when (absolute ticks).
enum Do {
    Tool(&'static str, Value),
    Walk(Vec3),
}

fn script() -> Vec<(u64, Do)> {
    vec![
        (60, Do::Tool("light_attack", json!({}))),
        (150, Do::Tool("move", json!({"dir": "E", "meters": 3}))),
        (270, Do::Tool("dodge", json!({"dir": "N"}))),
        (330, Do::Walk(Vec3::new(-0.5, 0.0, 0.0))),
        (420, Do::Walk(Vec3::ZERO)),
        (420, Do::Tool("light_attack", json!({}))),
    ]
}

fn tick(app: &App) -> u64 {
    sim::tick(app.world())
}

/// Plays the script from the current tick to `until`; what is due at `until` is for the next call.
fn drive(app: &mut App, script: &[(u64, Do)], until: u64) {
    loop {
        let now = tick(app);
        if now >= until {
            break;
        }
        for (_, what) in script.iter().filter(|(at, _)| *at == now) {
            match what {
                Do::Tool(tool, args) => {
                    call(app, tool, args.clone());
                }
                Do::Walk(dir) => {
                    let hero = app.world().resource::<AiIndex>().get(AiId(1)).unwrap();
                    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = *dir;
                }
            }
        }
        let next = script.iter().map(|(at, _)| *at).filter(|at| *at > now).min().unwrap_or(until);
        step(app, next.min(until) - now);
    }
}

fn story_after(app: &App, n: u64) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal
        .after(0)
        .0
        .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn an_npc_fight_loaded_at_any_tick_ends_the_same() {
    let mut original = app();
    arena(&mut original);
    let script = script();
    let end = 720;
    let checkpoints = [20, 60, 61, 100, 150, 200, 290, 345, 430, 600];
    let mut snapshots = Vec::new();
    for &at in &checkpoints {
        drive(&mut original, &script, at);
        snapshots.push((at, snapshot::capture(original.world()).unwrap()));
    }
    drive(&mut original, &script, end);
    let story = journal(&original);
    for needle in ["takes an attack token", "returns its attack token", "idle → fight"] {
        assert!(story.iter().any(|l| l.contains(needle)), "no `{needle}` in {story:#?}");
    }
    let last = snapshot::capture(original.world()).unwrap();

    for (at, snapshot) in snapshots {
        // A fresh arena: the floor, nothing else. The fighters are the snapshot's.
        let mut copy = app();
        let report = snapshot::restore(copy.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("tick {at}: {e}"));
        assert!(report.exact(), "tick {at}: {:?}", report.differences);
        drive(&mut copy, &script, end);
        let diff = differences(&last, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at}:\n{}", diff.join("\n"));
        assert_eq!(story_after(&copy, at), story_after(&original, at), "journal after tick {at}");
    }
}

#[test]
fn what_an_npc_holds_is_in_the_snapshot() {
    let mut original = app();
    arena(&mut original);
    drive(&mut original, &script(), 150);
    let snapshot = snapshot::capture(original.world()).unwrap();
    assert!(snapshot.left_out.is_empty(), "{:?}", snapshot.left_out);
    // e2..e4 are the grunts, e5 the warden.
    let grunt = &snapshot.entities.iter().find(|e| e.id == 2).unwrap().components;
    for component in ["Transform", "Npc", "NpcBrain", "NpcDrive", "NpcPlan", "NavAgent", "AttackToken", "Perceived", "Fighter", "ActionState"] {
        assert!(grunt.keys().any(|k| k.ends_with(&format!("::{component}"))), "{component} missing from {:?}", grunt.keys());
    }
}

// --- the navmesh across snapshots ----------------------------------------------------------

/// A static solid a game spawns and takes away (a crate, a door): it has an `AiId`, so
/// snapshots hold it.
fn crate_at(world: &mut World, center: Vec3, half: f32) -> Entity {
    let e = wall(world, center - Vec3::new(half, 0.0, half), center + Vec3::new(half, 2.5, half));
    world.entity_mut(e).insert((ai_core::AiKind::new("crate"), Name::new("crate")));
    e
}

/// The nodes of the navmesh switched off now.
fn nav_disabled(app: &App) -> Vec<u32> {
    let nav = app.world().resource::<shards_npc::NavRegion>();
    (0..nav.node_count()).filter(|n| nav.is_disabled(*n)).collect()
}

/// A snapshot loaded back into the game it was taken in carves the solids it brings back,
/// exactly like a fresh game loading it does.
#[test]
fn a_snapshot_loaded_into_its_own_game_carves_what_it_brings_back() {
    let rows = ["floor floor floor"];
    let mut game = app();
    load_region(&mut game, &rows);
    let block = crate_at(game.world_mut(), Vec3::new(11.5, 0.0, 4.0), 1.0);
    step(&mut game, 1);
    let carved = nav_disabled(&game);
    assert!(!carved.is_empty(), "the crate is carved out");
    let saved = snapshot::capture(game.world()).unwrap();
    game.world_mut().despawn(block);
    step(&mut game, 1);
    assert!(nav_disabled(&game).is_empty(), "and back in when it goes");

    let report = snapshot::restore(game.world_mut(), &saved, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    let mut fresh = app();
    load_region(&mut fresh, &rows);
    let report = snapshot::restore(fresh.world_mut(), &saved, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    for app in [&mut game, &mut fresh] {
        assert_eq!(nav_disabled(app), carved, "right after the load");
        step(app, 1);
        assert_eq!(nav_disabled(app), carved, "and on the next tick");
    }
}

/// What happens in the generated region, by absolute tick.
enum Nav {
    /// Spawn crate `n` (of [`crate_spots`]).
    Spawn(usize),
    /// Take crate `n` away.
    Remove(usize),
}

fn nav_script() -> Vec<(u64, Nav)> {
    vec![(30, Nav::Spawn(0)), (60, Nav::Spawn(1)), (90, Nav::Remove(0)), (150, Nav::Remove(1)), (160, Nav::Spawn(2)), (260, Nav::Remove(2))]
}

/// Where the crates go: two overlapping in the middle of the region, one further on.
fn crate_spots(nav: &shards_npc::NavRegion) -> [Vec3; 3] {
    let ground = |at: Vec3| nav.node_pos(nav.nearest_node(at, 6.0, 0.5).unwrap_or_else(|| panic!("no ground near {at}")));
    let middle = ground(Vec3::new(32.0, 0.0, 32.0));
    [middle, middle + Vec3::new(0.7, 0.0, 0.6), ground(Vec3::new(44.0, 0.0, 20.0))]
}

/// A generated 8×8 region, a player and three NPCs walking and chasing on its navmesh.
fn generated_region(app: &mut App) {
    ok(app, "regen", json!({"region": "grid", "seed": 11}));
    let nav = app.world().resource::<shards_npc::NavRegion>().clone();
    let ground = |at: Vec3| nav.node_pos(nav.nearest_node(at, 6.0, 0.5).unwrap_or_else(|| panic!("no ground near {at}")));
    let corners = [ground(Vec3::new(6.0, 0.0, 6.0)), ground(Vec3::new(58.0, 0.0, 58.0)), ground(Vec3::new(58.0, 0.0, 6.0)), ground(Vec3::new(6.0, 0.0, 58.0))];
    let world = app.world_mut();
    player(world, 0, ground(Vec3::new(40.0, 0.0, 30.0)));
    for (from, to) in [(corners[0], corners[1]), (corners[2], corners[3])] {
        let walker = npc(world, "grunt", Npc::new("walker", "eyes"), from, (to - from).normalize_or(N));
        world.entity_mut(walker).insert(Patrol { points: vec![to, from] });
    }
    let at = ground(Vec3::new(20.0, 0.0, 44.0));
    let sentry = npc(world, "grunt", Npc::new("sentry", "boss"), at, E);
    home(world, sentry, at, 30.0);
}

/// Plays [`nav_script`] from the current tick to `until`; crates by the `AiId`s in `ids`.
fn drive_nav(app: &mut App, spots: &[Vec3; 3], ids: &mut [Option<AiId>; 3], until: u64) {
    let script = nav_script();
    loop {
        let now = tick(app);
        if now >= until {
            break;
        }
        for (_, what) in script.iter().filter(|(at, _)| *at == now) {
            match *what {
                Nav::Spawn(n) => {
                    let e = crate_at(app.world_mut(), spots[n], 0.6);
                    ids[n] = Some(*app.world().get::<AiId>(e).unwrap());
                }
                Nav::Remove(n) => {
                    let e = app.world().resource::<AiIndex>().get(ids[n].unwrap()).unwrap();
                    app.world_mut().despawn(e);
                }
            }
        }
        let next = script.iter().map(|(at, _)| *at).filter(|at| *at > now).min().unwrap_or(until);
        step(app, next.min(until) - now);
    }
}

/// Routes across the region, for comparing navmeshes.
fn routes(app: &App) -> Vec<String> {
    let nav = app.world().resource::<shards_npc::NavRegion>();
    let physics = app.world().resource::<shards_motion::PhysicsWorld>();
    let ground = |at: Vec3| nav.node_pos(nav.nearest_node(at, 6.0, 0.5).unwrap());
    let ends = [ground(Vec3::new(6.0, 0.0, 6.0)), ground(Vec3::new(58.0, 0.0, 58.0)), ground(Vec3::new(26.0, 0.0, 30.0)), ground(Vec3::new(38.0, 0.0, 34.0))];
    let mut out = Vec::new();
    for a in ends {
        for b in ends {
            let query = shards_npc::NavQuery { from: a, to: b, goal_level: false, radius: 0.4, height: 1.8, step_height: 0.35 };
            out.push(format!("{:?}", nav.path(physics, &query).map(|p| p.points)));
        }
    }
    out
}

/// TZ E11.4 with the navmesh: in a generated region, with NPCs walking its routes and crates
/// carved out and back in, a snapshot taken at any tick and loaded into a fresh game goes on
/// exactly like the game it was taken in: the same navmesh, the same NPCs, the same journal.
#[test]
fn a_generated_region_with_npcs_on_its_navmesh_loads_and_goes_on_identically() {
    let end = 420;
    let checkpoints = [20, 45, 75, 100, 130, 200];
    let mut original = app();
    generated_region(&mut original);
    let spots = crate_spots(original.world().resource::<shards_npc::NavRegion>());
    let mut ids = [None; 3];
    let mut saved = Vec::new();
    for &at in &checkpoints {
        drive_nav(&mut original, &spots, &mut ids, at);
        saved.push((at, ids, snapshot::capture(original.world()).unwrap()));
    }
    drive_nav(&mut original, &spots, &mut ids, end);
    let last = snapshot::capture(original.world()).unwrap();
    let (last_disabled, last_routes) = (nav_disabled(&original), routes(&original));
    assert!(last_routes.iter().any(|r| r.starts_with("Ok")), "{last_routes:#?}");
    let moved = last.entities.iter().filter(|e| e.components.keys().any(|k| k.ends_with("::NavAgent"))).count();
    assert_eq!(moved, 3, "three NPCs on the navmesh");

    for (at, mut ids, snapshot) in saved {
        let mut copy = app();
        ok(&mut copy, "regen", json!({"region": "grid", "seed": 11}));
        let report = snapshot::restore(copy.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("tick {at}: {e}"));
        assert!(report.exact(), "tick {at}: {:?}", report.differences);
        drive_nav(&mut copy, &spots, &mut ids, end);
        assert_eq!(nav_disabled(&copy), last_disabled, "loaded at tick {at}: the navmesh");
        assert_eq!(routes(&copy), last_routes, "loaded at tick {at}: routes");
        let diff = differences(&last, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at}:\n{}", diff.join("\n"));
        assert_eq!(story_after(&copy, at), story_after(&original, at), "journal after tick {at}");
    }
}
