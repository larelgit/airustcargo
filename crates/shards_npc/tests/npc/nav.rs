//! Nav agents: routes around walls (through the path seam), avoidance, the leash.

use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::prelude::*;
use shards_action::Health;
use shards_motion::CharacterParams;
use shards_npc::{plain_sight, NpcDrive};
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
fn detour(_: &World, _: Vec3, to: Vec3, _: &CharacterParams) -> Option<Vec<Vec3>> {
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

/// A target at the very edge of a fighter's leash (the den guard of `npc_navmesh.scenario`): the
/// planner walks to a goal the leash pulls 0.5 m short of the target, and must stop within its
/// window of the target, not within its window of that goal. The jab's window ends at 1.84 m
/// (its 1.8 m cone at the plan's cover 0.8, plus the target's body); stopping within 0.85 of it
/// from the pulled goal left the grunt up to 2.06 m away, where only the timing of its start
/// decided whether it ever struck.
#[test]
fn a_grunt_strikes_a_target_at_the_edge_of_its_leash() {
    let mut app = app_with(record_starts);
    let world = app.world_mut();
    let grunt = grunt(world, "grunt", "jabber", "default", Vec3::ZERO);
    home(world, grunt, Vec3::ZERO, 6.0);
    let target = Vec3::new(0.0, 0.0, -6.0);
    player(world, 0, target);
    let (mut pulled_ticks, mut widest, mut farthest) = (0, 0.0_f32, 0.0_f32);
    for _ in 0..300 {
        step(&mut app, 1);
        farthest = farthest.max(flat_distance(feet(&app, grunt), Vec3::ZERO));
        let drive = app.world().get::<NpcDrive>(grunt).expect("an NPC has a drive");
        // Where the drive may stop it: anywhere within `within` of the goal the leash pulled 0.5 m
        // short of the target.
        if let Some(goal) = drive.goal.filter(|g| (flat_distance(*g, target) - 0.5).abs() < 0.05) {
            pulled_ticks += 1;
            widest = widest.max(drive.within + flat_distance(goal, target));
        }
    }
    assert!(pulled_ticks > 0, "the leash never pulled its goal short: the test does not reach the edge");
    assert!(widest <= 1.8, "it may stop {widest:.2} m from its target, beyond its 1.8 m jab");
    let jabs = app.world().resource::<Started>().0.iter().filter(|(_, e, a)| *e == grunt && a == "jab").count();
    let off = flat_distance(feet(&app, grunt), target);
    assert!(jabs > 0, "it never struck: it stands {off:.2} m from its target");
    assert!(farthest <= 6.0, "within its leash and the room for a body: {farthest:.2} m");
}

/// Outside any region: an NPC on a 4 m block sees its target below. A straight walk would
/// fall off (deeper than the safe drop) and the local grid knows no way down: it stays up.
#[test]
fn an_npc_off_the_navmesh_does_not_walk_off_a_ledge_either() {
    let mut app = app();
    wall(app.world_mut(), Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 4.0, 2.0));
    let world = app.world_mut();
    let sentry = npc(world, "grunt", Npc::new("sentry", "boss"), Vec3::new(0.0, 4.0, 0.0), E);
    player(world, 0, Vec3::new(9.0, 0.0, 0.0));
    for t in 0..240 {
        step(&mut app, 1);
        let at = feet(&app, sentry);
        assert!(at.y > 3.9, "tick {t}: it went over the edge: {at}");
    }
    assert_eq!(state(&app, sentry), "chase");
}

/// Outside any region: a wall east to west with an arch, 1.5 m wide under a lintel 2 m up, and
/// an open end 4 m east of it. A goal through the arch is in plain sight for a body that
/// clears the lintel; a taller one plans a route round the end of the wall and walks it.
#[test]
fn a_goal_through_an_arch_is_in_plain_sight_only_for_a_body_that_clears_it() {
    let mut app = app();
    let world = app.world_mut();
    wall(world, Vec3::new(-8.0, 0.0, -0.25), Vec3::new(-0.75, 3.0, 0.25));
    wall(world, Vec3::new(-0.75, 2.0, -0.25), Vec3::new(0.75, 3.0, 0.25));
    wall(world, Vec3::new(0.75, 0.0, -0.25), Vec3::new(4.0, 3.0, 0.25));
    let (from, goal) = (Vec3::new(0.0, 0.0, -5.0), Vec3::new(0.0, 0.0, 5.0));
    let tall = CharacterParams { height: 2.4, ..default() };
    let e = walker(&mut app, from, goal);
    app.world_mut().entity_mut(e).insert(tall.clone());
    step(&mut app, 1);
    assert!(plain_sight(app.world(), from, goal, &CharacterParams::default()), "1.8 m clears the lintel");
    assert!(!plain_sight(app.world(), from, goal, &tall), "2.4 m does not");
    let agent = app.world().get::<NavAgent>(e).unwrap().clone();
    assert!(!agent.direct && agent.path.len() > 1, "a route, not a straight line: {agent:?}");
    let mut crossed = None;
    for _ in 0..600 {
        let before = feet(&app, e);
        step(&mut app, 1);
        let after = feet(&app, e);
        if before.z < 0.0 && after.z >= 0.0 {
            crossed = Some(after.x);
        }
    }
    let crossed = crossed.expect("it crossed the wall's line");
    assert!(!(-8.0..4.0).contains(&crossed), "round the end of the wall, not under the lintel: at x {crossed:.2}");
    assert!(flat_distance(feet(&app, e), goal) < 0.6, "arrived: {}", feet(&app, e));
}

/// Outside any region: a walker on a direct route (its goal in plain sight along a 4 m high
/// walkway) is put down beside the line that was checked, on a pad that joins the walkway at
/// its start. The straight line from there to the goal leaves the pad over the drop: it plans
/// again from where it stands and goes round by the walkway instead of walking off.
#[test]
fn an_npc_put_off_its_straight_line_plans_again_before_walking_off_a_ledge() {
    let mut app = app();
    // The walkway, along z = 0, and the pad north of its start (both 4 m high).
    wall(app.world_mut(), Vec3::new(-2.0, 0.0, -1.5), Vec3::new(14.0, 4.0, 1.0));
    wall(app.world_mut(), Vec3::new(-2.0, 0.0, 1.0), Vec3::new(3.0, 4.0, 6.0));
    let goal = Vec3::new(12.0, 4.0, 0.0);
    let e = walker(&mut app, Vec3::new(0.0, 4.0, 0.0), goal);
    step(&mut app, 1);
    let agent = app.world().get::<NavAgent>(e).unwrap().clone();
    assert!(agent.direct && agent.path == [goal], "the goal is in plain sight along the walkway: {agent:?}");
    // Pushed, knocked back or carried off its line: on the pad, 4 m from it.
    let aside = Vec3::new(1.0, 4.0, 4.0);
    teleport(&mut app, e, aside);
    step(&mut app, 1);
    let agent = app.world().get::<NavAgent>(e).unwrap().clone();
    assert!(!agent.direct && agent.path.len() > 1, "planned again from where it stands: {agent:?}");
    assert!(agent.planned_from.is_some_and(|p| flat_distance(p, aside) < 0.2), "{agent:?}");
    for t in 0..360 {
        step(&mut app, 1);
        let at = feet(&app, e);
        assert!(at.y > 3.9, "tick {t}: it walked off the pad: {at}");
    }
    assert!(flat_distance(feet(&app, e), goal) < 0.6, "arrived by the walkway: {}", feet(&app, e));
}
