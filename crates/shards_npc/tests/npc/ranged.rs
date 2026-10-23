//! Gunfire and hiding (F11): what an NPC that notices threats holds for one, and where it hides. The
//! ranged fighters, the line of fire and the turret are the sandbox's (`tests/sandbox/npc_ranged.rs`).

use bevy::prelude::*;
use shards_action::ShotFired;
use shards_core::Intent;
use shards_motion::PhysicsWorld;
use shards_npc::prelude::*;
use shards_npc::{hiding_search, sight_clear, NpcThreats, HIDE_NODES, HIDE_RAYS};

use crate::common::*;

/// A body of the monsters that notices threats only and hides from them (`hider`), at `at`.
fn hider(world: &mut World, at: Vec3) -> Entity {
    npc(world, "grunt", Npc::new("hider", "threats"), at, W)
}

fn threats(app: &App, e: Entity) -> Vec<String> {
    app.world().get::<NpcThreats>(e).map(|t| t.known.iter().map(|t| t.id.to_string()).collect()).unwrap_or_default()
}

#[test]
fn a_hiding_spot_is_out_of_the_threats_sight_and_there_is_none_off_the_navmesh() {
    let mut app = app();
    load_region(&mut app, &["floor pillar floor"]);
    let me = hider(app.world_mut(), Vec3::new(9.0, 0.0, 4.0));
    step(&mut app, 2);
    // A threat's eye 1.6 m above (4, 4): the 2 m pillar at (12, 4) hides the far side of it.
    let eye = Vec3::new(4.0, 1.6, 4.0);
    let search = hiding_search(app.world(), me, eye, 12.0);
    let spot = search.spot.unwrap_or_else(|| panic!("{search:?}"));
    assert!(spot.x > 13.0 && (spot.z - 4.0).abs() < 3.0, "behind the pillar: {spot}");
    let physics = app.world().resource::<PhysicsWorld>();
    assert!(!sight_clear(physics, eye, spot + Vec3::Y * 1.6) && !sight_clear(physics, eye, spot + Vec3::Y * 0.9), "{spot}");
    assert!(search.rays <= HIDE_RAYS && search.nodes <= HIDE_NODES && search.rays >= 2, "{search:?}");
    // Pure: the same world, the same spot.
    assert_eq!(hiding_search(app.world(), me, eye, 12.0), search);
    // Too near to reach the pillar's shadow: every place within 2 m is in sight.
    let near = hiding_search(app.world(), me, eye, 2.0);
    assert_eq!(near.spot, None);
    assert!(near.why.as_deref().is_some_and(|w| w.contains("in sight")), "{near:?}");

    // No region: no navmesh, no spot.
    let mut app = crate::common::app();
    let me = hider(app.world_mut(), Vec3::new(9.0, 0.0, 4.0));
    step(&mut app, 2);
    let search = hiding_search(app.world(), me, eye, 12.0);
    assert_eq!(search.spot, None);
    assert!(search.why.as_deref().is_some_and(|w| w.contains("off the navmesh")), "{search:?}");
}

#[test]
fn a_body_seen_firing_is_a_threat_whatever_its_team_and_a_hostile_team_alone_is_none() {
    let mut app = app();
    load_region(&mut app, &["floor pillar floor"]);
    let world = app.world_mut();
    let me = hider(world, Vec3::new(9.0, 0.0, 4.0));
    // Of its own side, 6 m west, with a silent gun (a shot no one hears): it must be seen.
    let guard = fighter(world, "Guard", "silent_gun", "monsters", Vec3::new(3.0, 0.0, 4.0), N);
    // A hostile player 3 m away, unarmed.
    let hero = player(world, 0, Vec3::new(9.0, 0.0, 7.0));
    step(&mut app, 30);
    assert_eq!(state(&app, me), "idle");
    assert!(app.world().get::<Perceived>(me).unwrap().known.is_empty(), "the player is no threat");
    assert!(threats(&app, me).is_empty());
    // The guard fires north, at nothing: seen, it is a threat.
    app.world_mut().get_mut::<Intent>(guard).unwrap().press("light");
    step(&mut app, 3);
    assert_eq!(threats(&app, me), [id(&app, guard)]);
    assert_eq!(state(&app, me), "hide", "{:#?}", journal(&app));
    assert!(lines(&app, &format!("idle → hide {}", id(&app, guard))).len() == 1, "{:#?}", journal(&app));
    // It runs behind the pillar, out of the guard's sight, and stays.
    step(&mut app, 300);
    let at = feet(&app, me);
    assert!(at.x > 13.0, "behind the pillar: {at}");
    let physics = app.world().resource::<PhysicsWorld>();
    assert!(!sight_clear(physics, Vec3::new(3.0, 1.6, 4.0), at + Vec3::Y * 1.6), "{at}");
    assert!(app.world().get::<Perceived>(me).unwrap().known.iter().all(|k| k.id != aid(&app, hero)), "still not the player");
    step(&mut app, 120);
    assert!(flat_distance(feet(&app, me), at) < 0.3, "it stays");
}

#[test]
fn a_shot_heard_is_a_threat_within_its_noise_and_none_beyond() {
    let mut app = app();
    let world = app.world_mut();
    let near = hider(world, Vec3::new(0.0, 0.0, 0.0));
    let far = hider(world, Vec3::new(0.0, 0.0, -30.0));
    // A player far off, so the NPCs are awake: hostile, and no threat.
    player(world, 0, Vec3::new(25.0, 0.0, 0.0));
    // Behind a wall from both: not seen, only heard.
    let shooter = fighter(world, "Shooter", "loud_gun", "heroes", Vec3::new(0.0, 0.0, 10.0), S);
    wall(world, Vec3::new(-5.0, 0.0, 6.0), Vec3::new(5.0, 4.0, 6.5));
    step(&mut app, 10);
    let from = Vec3::new(0.0, 1.6, 10.0);
    app.world_mut().trigger(ShotFired { shooter, action: "loud_shot".into(), from, dir: Vec3::Z, pellets: 1, noise: 20.0 });
    step(&mut app, 2);
    assert_eq!(threats(&app, near), [id(&app, shooter)], "10 m from a shot heard 20 m around");
    assert!(threats(&app, far).is_empty(), "40 m from it");
    // No navmesh here: nowhere to hide, so it cowers.
    step(&mut app, 2);
    assert_eq!(state(&app, near), "cower", "{:#?}", journal(&app));
    assert_eq!(state(&app, far), "idle");
    // Its memory is 10 s: then the shooter is no threat any more.
    step(&mut app, 620);
    assert!(threats(&app, near).is_empty());
}
