//! Avoidance between bodies (E6.2): NPCs meeting in a corridor, a pack closing in on a
//! player, avoidance turned off, the same run twice.

use bevy::prelude::*;
use serde_json::json;
use shards_npc::prelude::*;

use crate::common::*;

/// Two bodies of 0.4 m: closer than this their capsules touch.
const TOUCH: f32 = 0.8;

fn walker(app: &mut App, behavior: &str, from: Vec3, to: Vec3) -> Entity {
    let world = app.world_mut();
    let e = npc(world, "grunt", Npc::new(behavior, "eyes"), from, (to - from).normalize_or(N));
    world.entity_mut(e).insert(Patrol { points: vec![to] });
    e
}

/// A corridor along X, 2.4 m between its walls: room for two bodies to pass, not to swing
/// wide.
fn corridor(app: &mut App) {
    wall(app.world_mut(), Vec3::new(-12.0, 0.0, -2.2), Vec3::new(12.0, 3.0, -1.2));
    wall(app.world_mut(), Vec3::new(-12.0, 0.0, 1.2), Vec3::new(12.0, 3.0, 2.2));
}

/// Two walkers of `behavior` meeting head on in the corridor (one `offset` meters off the
/// middle): how close they came, and the tick both had arrived (`None`: they never did).
fn meet(behavior: &str, offset: f32) -> (f32, Option<u32>) {
    let mut app = app();
    corridor(&mut app);
    let (west, east) = (Vec3::new(-8.0, 0.0, 0.0), Vec3::new(8.0, 0.0, offset));
    let a = walker(&mut app, behavior, west, east);
    let b = walker(&mut app, behavior, east, west);
    let mut closest = f32::INFINITY;
    let mut arrived = None;
    for t in 0..900 {
        step(&mut app, 1);
        closest = closest.min(flat_distance(feet(&app, a), feet(&app, b)));
        if arrived.is_none() && flat_distance(feet(&app, a), east) < 0.6 && flat_distance(feet(&app, b), west) < 0.6 {
            arrived = Some(t);
        }
    }
    (closest, arrived)
}

#[test]
fn two_npcs_meeting_head_on_in_a_corridor_pass_without_touching_or_stopping() {
    for offset in [0.0, 0.2, -0.3] {
        let (closest, arrived) = meet("walker", offset);
        assert!(closest >= TOUCH + 0.05, "offset {offset}: they came within {closest:.2} m");
        // 16 m at 3.6 m/s is under 5 s: a sidestep costs a little, a standoff all of it.
        assert!(arrived.is_some_and(|t| t < 420), "offset {offset}: both through the corridor by tick {arrived:?}");
    }
}

/// Four chasers on an arc 8 m north of the player, 30° apart: their ways meet at it.
const ARC: [Vec3; 4] = [Vec3::new(-5.66, 0.0, -5.66), Vec3::new(-2.07, 0.0, -7.73), Vec3::new(2.07, 0.0, -7.73), Vec3::new(5.66, 0.0, -5.66)];

/// Four chasers in a file of two by two, 8 m north of the player, 0.4 m between bodies.
const FILE: [Vec3; 4] = [Vec3::new(-0.6, 0.0, -8.0), Vec3::new(0.6, 0.0, -8.0), Vec3::new(-0.6, 0.0, -9.2), Vec3::new(0.6, 0.0, -9.2)];

/// A player (god mode) at the origin and four chasers of `behavior` at `spots`:
/// the closest any two chasers came, the closest a chaser came to the player, how far the
/// player was pushed, how far each chaser ends from the player and how far it moved in the
/// last second, and where every body was after each tick.
struct Pack {
    closest: f32,
    to_player: f32,
    pushed: f32,
    ends: Vec<f32>,
    last_second: Vec<f32>,
    steered: bool,
    track: Vec<Vec<Vec3>>,
}

fn pack(behavior: &str, spots: [Vec3; 4], ticks: u32) -> Pack {
    let mut app = app();
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let chasers: Vec<Entity> = spots.iter().map(|at| npc(app.world_mut(), "grunt", Npc::new(behavior, "grunt"), *at, S)).collect();
    let mut out = Pack { closest: f32::INFINITY, to_player: f32::INFINITY, pushed: 0.0, ends: Vec::new(), last_second: Vec::new(), steered: false, track: Vec::new() };
    for _ in 0..ticks {
        step(&mut app, 1);
        let at: Vec<Vec3> = chasers.iter().map(|e| feet(&app, *e)).collect();
        for i in 0..at.len() {
            for j in i + 1..at.len() {
                out.closest = out.closest.min(flat_distance(at[i], at[j]));
            }
            out.to_player = out.to_player.min(flat_distance(at[i], feet(&app, hero)));
        }
        out.pushed = out.pushed.max(flat_distance(feet(&app, hero), Vec3::ZERO));
        for e in &chasers {
            let agent = app.world().get::<NavAgent>(*e).unwrap();
            out.steered |= agent.avoiding.is_some();
            if agent.avoiding.is_none() && agent.preferred != Vec3::ZERO {
                // Not steering: the route's own input, nothing added.
                assert!(agent.desired.normalize().dot(agent.preferred.normalize()) > 0.9999, "{agent:?}");
            }
        }
        let mut row = at;
        row.push(feet(&app, hero));
        out.track.push(row);
    }
    let (second_ago, now) = (&out.track[out.track.len() - 61], &out.track[out.track.len() - 1]);
    out.ends = now[..4].iter().map(|p| flat_distance(*p, Vec3::ZERO)).collect();
    out.last_second = (0..4).map(|i| flat_distance(second_ago[i], now[i])).collect();
    out
}

#[test]
fn four_npcs_closing_in_on_a_player_keep_their_bodies_apart() {
    for (name, spots) in [("arc", ARC), ("file", FILE)] {
        let p = pack("chaser", spots, 480);
        assert!(p.steered, "{name}: they had to steer round each other");
        assert!(p.closest >= TOUCH + 0.05, "{name}: two chasers came within {:.2} m", p.closest);
        assert!(p.to_player >= TOUCH - 0.02, "{name}: a chaser ran into the player: {:.2} m", p.to_player);
        assert!(p.pushed <= 0.02, "{name}: the player was pushed {:.2} m", p.pushed);
        // Two get to their places 1.5 m from the player; the others, their way there full,
        // wait behind them within a body's width of theirs instead of squeezing about
        // between them: in the end nobody moves.
        assert!(p.ends.iter().filter(|d| **d <= 1.5 + 0.1).count() >= 2, "{name}: how far they stand: {:?}", p.ends);
        assert!(p.ends.iter().all(|d| *d <= 1.5 + TOUCH + 0.2 + 0.1), "{name}: how far they stand: {:?}", p.ends);
        assert!(p.last_second.iter().all(|d| *d < 0.02), "{name}: they still shuffle about: {:?}", p.last_second);
    }
}

#[test]
fn without_avoidance_npcs_walk_their_routes_as_if_alone() {
    // Weight 0 (the behaviour's own avoidance): they never steer, the route's input goes
    // straight into `Intent`, and only the physics keeps the bodies from passing through
    // each other: they bump.
    let p = pack("chaser_plough", ARC, 480);
    assert!(!p.steered);
    assert!(p.closest < TOUCH + 0.02, "they pressed into each other: {:.2} m", p.closest);
    let (closest, _) = meet("plough", 0.0);
    assert!(closest < TOUCH + 0.02, "head on they bump: {closest:.2} m");
}

#[test]
fn the_same_pack_ends_in_the_same_places_twice() {
    let (a, b) = (pack("chaser", ARC, 300), pack("chaser", ARC, 300));
    for (t, (x, y)) in a.track.iter().zip(&b.track).enumerate() {
        assert_eq!(x, y, "tick {t}: the runs part");
    }
}

#[test]
fn npc_state_and_the_overlay_say_whom_an_npc_steers_around() {
    let mut app = app();
    corridor(&mut app);
    let a = walker(&mut app, "walker", Vec3::new(-8.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0));
    let b = walker(&mut app, "walker", Vec3::new(8.0, 0.0, 0.0), Vec3::new(-8.0, 0.0, 0.0));
    player(app.world_mut(), 0, Vec3::new(0.0, 0.0, 6.0));
    let (ia, ib) = (id(&app, a), id(&app, b));
    let first = step_until(&mut app, 300, |app| app.world().get::<NavAgent>(a).unwrap().avoiding.is_some());
    assert!(first, "they met and steered");
    let state = ok(&mut app, "npc_state", json!({"id": ia}));
    assert_eq!(state.result["nav"]["avoiding"], json!(ib), "{}", state.result);
    let lines = text(&ok(&mut app, "npc_overlay", json!({})));
    assert!(lines.contains(&format!("avoiding {ib}")), "{lines}");
    // Past each other: nothing to steer around, and nothing in the journal about it.
    step(&mut app, 240);
    let state = ok(&mut app, "npc_state", json!({"id": ia}));
    assert_eq!(state.result["nav"]["avoiding"], json!(null));
    assert!(all_journal(&app).iter().all(|l| !l.contains("avoid")), "{:#?}", all_journal(&app));
}

/// Steps one tick at a time until `done` holds (true) or `ticks` ran out (false).
fn step_until(app: &mut App, ticks: u32, done: impl Fn(&App) -> bool) -> bool {
    for _ in 0..ticks {
        step(app, 1);
        if done(app) {
            return true;
        }
    }
    false
}
