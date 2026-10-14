//! Nav agents: routes around walls (through the path seam), avoidance, the leash.

use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::prelude::*;
use shards_action::Health;
use shards_npc::prelude::*;

use crate::common::*;

fn walker(app: &mut App, from: Vec3, to: Vec3) -> Entity {
    let world = app.world_mut();
    let e = npc(world, "grunt", Npc::new("walker", "eyes"), from, (to - from).normalize_or(N));
    world.entity_mut(e).insert(Patrol { points: vec![to] });
    e
}

/// Two walkers running opposite ways along one line (offset sideways by `offset`): how close
/// they came, and where they ended.
fn head_on(offset: f32) -> (f32, Vec3, Vec3) {
    let mut app = app();
    let a = walker(&mut app, Vec3::new(-6.0, 0.0, 0.0), Vec3::new(6.0, 0.0, offset));
    let b = walker(&mut app, Vec3::new(6.0, 0.0, offset), Vec3::new(-6.0, 0.0, 0.0));
    let mut closest = f32::INFINITY;
    for _ in 0..420 {
        step(&mut app, 1);
        closest = closest.min(flat_distance(feet(&app, a), feet(&app, b)));
    }
    (closest, feet(&app, a), feet(&app, b))
}

#[test]
fn two_npcs_walking_into_each_other_pass_without_touching() {
    for offset in [0.0, 0.25, -0.4] {
        let (closest, a, b) = head_on(offset);
        // Two bodies of 0.4 m: they never touch, let alone overlap.
        assert!(closest >= 0.8 + 0.05, "offset {offset}: they came within {closest:.2} m");
        assert!(flat_distance(a, Vec3::new(6.0, 0.0, offset)) < 0.6, "offset {offset}: a stopped at {a}");
        assert!(flat_distance(b, Vec3::new(-6.0, 0.0, 0.0)) < 0.6, "offset {offset}: b stopped at {b}");
    }
}

#[test]
fn routes_go_around_a_pillar() {
    let mut app = app();
    // A 2 x 2 m pillar right between the walker and its goal.
    wall(app.world_mut(), Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 4.0, 1.0));
    let e = walker(&mut app, Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0));
    step(&mut app, 1);
    let agent = app.world().get::<NavAgent>(e).unwrap().clone();
    assert!(!agent.direct && agent.path.len() > 1, "a route, not a straight line: {agent:?}");
    for _ in 0..360 {
        step(&mut app, 1);
        let p = feet(&app, e);
        assert!(p.x.abs() > 1.3 || p.z.abs() > 1.3, "inside the pillar at {p}");
    }
    let at = feet(&app, e);
    assert!(flat_distance(at, Vec3::new(0.0, 0.0, 5.0)) < 0.6, "arrived: {at}");
}

static CALLS: AtomicUsize = AtomicUsize::new(0);

/// A game's (or a navmesh's) routes: always by way of (3, 0, 0).
fn detour(_: &World, _: Vec3, to: Vec3, _: f32) -> Option<Vec<Vec3>> {
    CALLS.fetch_add(1, Ordering::SeqCst);
    Some(vec![Vec3::new(3.0, 0.0, 0.0), to])
}

#[test]
fn the_path_seam_takes_another_provider() {
    let mut app = app_with(|app| {
        app.insert_resource(NavPaths { name: "detour", find: detour });
    });
    wall(app.world_mut(), Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 4.0, 1.0));
    let e = walker(&mut app, Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0));
    let before = CALLS.load(Ordering::SeqCst);
    step(&mut app, 1);
    assert!(CALLS.load(Ordering::SeqCst) > before, "the agent asked the provider");
    let path = app.world().get::<NavAgent>(e).unwrap().path.clone();
    assert_eq!(path, [Vec3::new(3.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 5.0)]);
    step(&mut app, 30);
    assert!(feet(&app, e).x > 0.5, "it walks the provider's route: {}", feet(&app, e));
    step(&mut app, 300);
    assert!(flat_distance(feet(&app, e), Vec3::new(0.0, 0.0, 5.0)) < 0.6);
    // The query tool asks the same provider.
    let out = ok(&mut app, "nav_path", serde_json::json!({"from": {"x": 0, "z": -5}, "to": {"x": 0, "z": 5}}));
    assert_eq!(out.result["provider"], "detour");
    assert_eq!(out.result["waypoints"][0]["x"], 3.0);
}

#[test]
fn a_sentry_chases_to_its_leash_goes_home_healing_and_never_crosses() {
    let mut app = app();
    let world = app.world_mut();
    let sentry = npc(world, "grunt", Npc::new("sentry", "grunt"), Vec3::ZERO, N);
    home(world, sentry, Vec3::ZERO, 8.0);
    world.get_mut::<Health>(sentry).unwrap().current = 40.0;
    let hero = player(world, 0, Vec3::new(0.0, 0.0, -3.0));
    let mut farthest: f32 = 0.0;
    let mut run = |app: &mut App, ticks: u32| {
        for _ in 0..ticks {
            step(app, 1);
            farthest = farthest.max(flat_distance(feet(app, sentry), Vec3::ZERO));
        }
    };
    // The target walks north to 11 m: within the leash and its 4 m margin, so the sentry
    // follows it to the edge of its 8 m and waits there.
    for z in [-5.0, -7.0, -9.0, -11.0] {
        teleport(&mut app, hero, Vec3::new(0.0, 0.0, z));
        run(&mut app, 60);
    }
    assert_eq!(state(&app, sentry), "chase");
    let out = flat_distance(feet(&app, sentry), Vec3::ZERO);
    assert!(out > 7.0, "it chased out to its leash: {out:.2} m");
    // 14 m: out of reach. It walks home, healing, and stays there.
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, -14.0));
    run(&mut app, 300);
    assert_eq!(state(&app, sentry), "idle");
    assert!(flat_distance(feet(&app, sentry), Vec3::ZERO) < 0.6, "home: {}", feet(&app, sentry));
    assert!(app.world().get::<Health>(sentry).unwrap().current > 40.0, "healed on the way");
    assert!(farthest <= 8.0 + 0.05, "never beyond its leash: {farthest:.2} m");
    let me = id(&app, sentry);
    let story = journal(&app);
    for change in ["idle → chase", "chase → home", "home → idle"] {
        assert!(story.iter().any(|l| l.contains(&format!("{me} {change}"))), "no `{change}` in {story:#?}");
    }
}
