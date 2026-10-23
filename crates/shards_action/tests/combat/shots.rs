//! Hitscan shots (F3): a `Fire` effect's rays from the eye along the look (or toward a target), through
//! the damage pipeline of a swing, stopped by walls, passing `pierce` bodies; pellets spread the same every
//! run; recoil turns the look; a `Look` spawn flies along the pitched look; the `fire` tool and the
//! `shot_trace` query.

use ai_core::AiId;
use bevy::prelude::*;
use serde_json::json;
use shards_action::{shot_ray, trace_shot, ActionDef, BodyParts, Dead, ProjectileFlight, ShotFired, ShotImpact};
use shards_core::{Gaze, Intent};
use shards_motion::eye_of;

use crate::common::*;

/// What the tests see of the shots.
#[derive(Resource, Default)]
struct Seen {
    fired: Vec<ShotFired>,
    impacts: Vec<ShotImpact>,
}

fn arena() -> App {
    app_with(|app| {
        app.init_resource::<Seen>();
        app.add_observer(|e: On<ShotFired>, mut seen: ResMut<Seen>| seen.fired.push(e.event().clone()));
        app.add_observer(|e: On<ShotImpact>, mut seen: ResMut<Seen>| seen.impacts.push(e.event().clone()));
    })
}

fn seen(app: &App) -> &Seen {
    app.world().resource::<Seen>()
}

/// A target that never staggers or flinches, of the monsters, at `at`: with a head and a torso when `parts`.
fn target(app: &mut App, at: Vec3, parts: bool) -> Entity {
    let world = app.world_mut();
    let e = fighter(world, "Dummy", "dummy", "monsters", at, S);
    if parts {
        world.entity_mut(e).insert(BodyParts("headed".into()));
    }
    e
}

fn look(app: &mut App, e: Entity, yaw: f32, pitch: f32) {
    app.world_mut().get_mut::<Intent>(e).unwrap().look = Some(Gaze::from_degrees(yaw, pitch));
}

/// Presses `light` (the primary fire) and runs the tick it fires in and one more.
fn shoot(app: &mut App, e: Entity) {
    press(app, e, "light");
    step(app, 2);
}

/// The hero with `moveset` at the origin, looking north, and a target with parts 10 m north; the parts are in.
fn range(moveset: &str) -> (App, Entity, Entity) {
    let mut app = arena();
    let hero = hero(app.world_mut(), moveset, Vec3::ZERO, N);
    let dummy = target(&mut app, Vec3::new(0.0, 0.0, -10.0), true);
    look(&mut app, hero, 0.0, 0.0);
    step(&mut app, 2);
    (app, hero, dummy)
}

#[test]
fn a_shot_at_the_head_doubles_and_one_at_the_torso_does_not() {
    let (mut app, hero, dummy) = range("sg_gun");
    let (from, dir) = shot_ray(app.world(), hero).unwrap();
    assert_eq!(from, eye_of(app.world(), hero).unwrap(), "a shot starts at the eye");
    assert!(dir.abs_diff_eq(Vec3::NEG_Z, 1e-5), "along the look: {dir}");
    shoot(&mut app, hero);
    assert_eq!(hp(&app, dummy), 80.0, "the head doubles 10");
    let (h, d) = (id(&app, hero), id(&app, dummy));
    let fired = lines(&app, "fires");
    assert!(fired.len() == 1 && fired[0].ends_with(&format!("{h} fires s_shot: {d} head -20 hp (hp 80)")), "{fired:?}");
    assert!(lines(&app, &format!("{h} -> {d}")).is_empty(), "the pellet's own hit line is folded into the shot's");
    let impact = seen(&app).impacts.last().cloned().unwrap();
    assert_eq!(impact.target, Some(dummy));
    assert!(impact.part.is_some() && impact.collider.is_none());
    let fired = seen(&app).fired.last().cloned().unwrap();
    assert_eq!((fired.shooter, fired.pellets, fired.noise), (hero, 1, 25.0));
    // Lower, through the torso (centre 1 m up): the hit as written.
    let eye = eye_of(app.world(), hero).unwrap();
    let pitch = ops::atan2(1.0 - eye.y, 10.0).to_degrees();
    look(&mut app, hero, 0.0, pitch);
    step(&mut app, 20);
    shoot(&mut app, hero);
    assert_eq!(hp(&app, dummy), 70.0);
    assert!(lines(&app, "fires").last().unwrap().ends_with(&format!("{d} torso -10 hp (hp 70)")), "{:?}", lines(&app, "fires"));
}

#[test]
fn a_wall_stops_a_shot_and_pierce_passes_bodies_never_walls() {
    let (mut app, hero, dummy) = range("sg_gun");
    let behind = target(&mut app, Vec3::new(0.0, 0.0, -12.0), false);
    step(&mut app, 1);
    // `light` stops at the first body, `heavy` passes one: the body behind takes the second.
    shoot(&mut app, hero);
    assert_eq!((hp(&app, dummy), hp(&app, behind)), (80.0, 100.0));
    step(&mut app, 20);
    press(&mut app, hero, "heavy");
    step(&mut app, 2);
    assert_eq!((hp(&app, dummy), hp(&app, behind)), (60.0, 90.0), "pierce 1 hits two bodies");
    let line = lines(&app, "fires s_pierce").pop().unwrap();
    assert!(line.ends_with(&format!("{} head -20 hp (hp 60), {} -10 hp (hp 90)", id(&app, dummy), id(&app, behind))), "{line}");
    // A wall 4 m ahead: nothing gets through, whatever pierce says.
    wall(app.world_mut(), Vec3::new(-2.0, 0.0, -4.5), Vec3::new(2.0, 3.0, -4.0));
    step(&mut app, 20);
    press(&mut app, hero, "heavy");
    step(&mut app, 2);
    assert_eq!((hp(&app, dummy), hp(&app, behind)), (60.0, 90.0));
    assert!(lines(&app, "fires s_pierce").pop().unwrap().ends_with(": wall 4.0m"), "{:?}", lines(&app, "fires"));
    let stop = seen(&app).impacts.last().cloned().unwrap();
    assert!(stop.target.is_none() && stop.collider.is_some());
    assert!((stop.normal - Vec3::Z).length() < 1e-4, "the normal faces the shooter: {}", stop.normal);
}

#[test]
fn allies_the_dead_and_the_shooter_are_looked_through() {
    let (mut app, hero, dummy) = range("sg_gun");
    let ally = fighter(app.world_mut(), "Friend", "dummy", "heroes", Vec3::new(0.0, 0.0, -3.0), S);
    let corpse = target(&mut app, Vec3::new(0.0, 0.0, -6.0), false);
    app.world_mut().entity_mut(corpse).insert(Dead);
    step(&mut app, 1);
    shoot(&mut app, hero);
    assert_eq!((hp(&app, ally), hp(&app, corpse), hp(&app, dummy)), (100.0, 100.0, 80.0));
    let path = trace_shot(app.world(), hero, eye_of(app.world(), hero).unwrap(), Vec3::NEG_Z, 100.0, 5);
    assert_eq!(path.bodies.iter().map(|b| b.0).collect::<Vec<_>>(), [dummy], "only the hostile living body is taken");
}

#[test]
fn falloff_keeps_the_share_its_distance_gives() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "sg_falloff", Vec3::ZERO, N);
    let far = target(&mut app, Vec3::new(0.0, 0.0, -20.0), false);
    look(&mut app, hero, 0.0, 0.0);
    step(&mut app, 2);
    let eye = eye_of(app.world(), hero).unwrap();
    let hit = trace_shot(app.world(), hero, eye, Vec3::NEG_Z, 100.0, 0).bodies[0].2;
    // 20 at 10 m and nearer, 10 at 30 m and past, a straight line between.
    let share = 1.0 - 0.5 * (hit.distance - 10.0) / 20.0;
    shoot(&mut app, hero);
    assert!((hp(&app, far) - (100.0 - 20.0 * share)).abs() < 1e-3, "hp {} at {} m", hp(&app, far), hit.distance);
}

/// Where each pellet ended and the body it hit, if any.
type Impacts = Vec<(Vec3, Option<Entity>)>;

/// Fires the scatter gun once at a target 6 m north in a fresh arena: the impacts, the target's health and
/// the pellets' directions from the eye.
fn scatter_once() -> (Impacts, f32, Vec<Vec3>) {
    let mut app = arena();
    let hero = hero(app.world_mut(), "sg_scatter", Vec3::ZERO, N);
    let dummy = target(&mut app, Vec3::new(0.0, 0.0, -6.0), true);
    // A wall behind it takes the pellets that miss.
    wall(app.world_mut(), Vec3::new(-6.0, 0.0, -9.0), Vec3::new(6.0, 4.0, -8.5));
    look(&mut app, hero, 0.0, -4.0);
    step(&mut app, 2);
    shoot(&mut app, hero);
    let impacts = seen(&app).impacts.iter().map(|i| (i.point, i.target)).collect();
    let dirs = impacts_dirs(&app, hero);
    (impacts, hp(&app, dummy), dirs)
}

fn impacts_dirs(app: &App, hero: Entity) -> Vec<Vec3> {
    let eye = eye_of(app.world(), hero).unwrap();
    seen(app).impacts.iter().map(|i| (i.point - eye).normalize()).collect()
}

#[test]
fn a_scatter_gun_spreads_its_pellets_the_same_every_run() {
    let (a, hp_a, dirs) = scatter_once();
    let (b, hp_b, _) = scatter_once();
    assert_eq!(a, b, "the same seed, tick and shooter: the same pellets");
    assert_eq!(hp_a, hp_b);
    assert_eq!(a.len(), 8, "each pellet meets the target or the wall: {a:?}");
    let aim = Gaze::from_degrees(0.0, -4.0).dir();
    let off: Vec<f32> = dirs.iter().map(|d| d.angle_between(aim).to_degrees()).collect();
    assert!(off.iter().all(|o| *o <= 6.0 + 0.05), "inside the 6 degree cone: {off:?}");
    assert!(off.iter().any(|o| *o > 1.0), "and spread in it: {off:?}");
    assert!(a.iter().any(|i| i.1.is_some()), "some pellets hit the target: {a:?}");
}

#[test]
fn a_body_without_a_look_shoots_at_its_target_with_its_aim_error() {
    let run = || {
        let mut app = arena();
        let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, -8.0), S);
        let gunner = fighter(app.world_mut(), "Gunner", "sg_aimed", "monsters", Vec3::ZERO, N);
        let hero_id = *app.world().get::<AiId>(hero).unwrap();
        app.world_mut().get_mut::<Intent>(gunner).unwrap().target = Some(hero_id);
        step(&mut app, 2);
        let (from, dir) = shot_ray(app.world(), gunner).unwrap();
        let aim = shards_motion::aim_point_of(app.world(), hero).unwrap();
        assert!(dir.abs_diff_eq((aim - from).normalize(), 1e-5), "toward the target's aim point");
        shoot(&mut app, gunner);
        let fired = seen(&app).fired.last().cloned().unwrap();
        fired.dir.angle_between(dir).to_degrees()
    };
    let (first, second) = (run(), run());
    assert_eq!(first, second, "the aim error is drawn from the keyed stream");
    assert!(first > 0.0 && first <= 10.0 + 1e-3, "within aim_error_deg: {first}");
}

#[test]
fn recoil_turns_the_look_and_is_held_to_the_pitch_limits() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "sg_kick", Vec3::ZERO, N);
    look(&mut app, hero, 0.0, 0.0);
    step(&mut app, 2);
    shoot(&mut app, hero);
    let kicked = app.world().get::<Intent>(hero).unwrap().look.unwrap();
    assert!((kicked.pitch.to_degrees() - 2.0).abs() < 1e-4 && (kicked.yaw.to_degrees() + 1.0).abs() < 1e-4, "{kicked:?}");
    look(&mut app, hero, 0.0, 88.5);
    step(&mut app, 10);
    shoot(&mut app, hero);
    let held = app.world().get::<Intent>(hero).unwrap().look.unwrap();
    assert!((held.pitch.to_degrees() - 89.0).abs() < 1e-3, "held to 89: {held:?}");
}

#[test]
fn a_guard_pays_the_shots_guard_damage() {
    let mut app = arena();
    let gunner = fighter(app.world_mut(), "Gunner", "sg_guarded", "monsters", Vec3::ZERO, N);
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, -6.0), S);
    look(&mut app, gunner, 0.0, 0.0);
    hold(&mut app, hero, "block", true);
    press(&mut app, hero, "block");
    // Past the raise (a perfect guard there would parry).
    step(&mut app, 12);
    let before = stamina(&app, hero);
    shoot(&mut app, gunner);
    let spd = app.world().resource::<shards_content::ContentDb>().get::<ActionDef>("block").unwrap().guard.as_ref().unwrap().stamina_per_damage;
    assert!((before - stamina(&app, hero) - 3.0 * spd).abs() < 0.6, "guard_damage 3, not the damage 10: {before} -> {}", stamina(&app, hero));
    assert!(lines(&app, "fires s_guarded").pop().unwrap().contains(" blocked -1 hp"), "{:?}", lines(&app, "fires"));
}

#[test]
fn a_look_spawn_flies_along_the_pitched_look_from_the_eye() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "sg_launcher", Vec3::ZERO, N);
    look(&mut app, hero, 30.0, 20.0);
    step(&mut app, 2);
    let eye = eye_of(app.world(), hero).unwrap();
    press(&mut app, hero, "light");
    step(&mut app, 1);
    let mut flights = app.world_mut().query::<(&ProjectileFlight, &Transform)>();
    let (flight, tf) = flights.iter(app.world()).next().map(|(f, t)| (f.clone(), *t)).expect("a bolt");
    let along = Gaze::from_degrees(30.0, 20.0).dir();
    assert!(flight.dir.abs_diff_eq(along, 1e-4), "flies along the look, pitch included: {} vs {along}", flight.dir);
    assert!(flight.from.abs_diff_eq(eye + along * 0.5, 1e-3), "starts 0.5 m along the ray from the eye: {} vs {}", flight.from, eye + along * 0.5);
    assert!(tf.translation.y > eye.y, "climbing: {}", tf.translation);
}

#[test]
fn the_fire_tool_and_the_shot_trace_query() {
    let (mut app, _hero, dummy) = range("sg_gun");
    let trace = call(&mut app, "shot_trace", json!({}));
    assert!(trace.ok, "{}", trace.result);
    assert_eq!(trace.result["hits"][0]["id"], json!(id(&app, dummy)));
    assert_eq!(trace.result["hits"][0]["part"], json!("head"));
    assert_eq!(trace.result["aimed"], json!("look"));
    assert!(trace.result["line"].as_str().unwrap().starts_with(&format!("{} Dummy head 9.8m (x1.00)", id(&app, dummy))), "{}", trace.result);
    assert_eq!(ok(&mut app, "fire", json!({})), "ok: s_shot");
    step(&mut app, 2);
    assert_eq!(hp(&app, dummy), 80.0);
    let observed = text(&call(&mut app, "observe", json!({})));
    assert!(observed.contains("SHOT s_shot -> "), "{observed}");
    // A swing in the hands is no gun.
    let mut app = arena();
    hero(app.world_mut(), "sg_blade", Vec3::ZERO, N);
    step(&mut app, 1);
    let refused = fails(&mut app, "fire", json!({}));
    assert!(refused.contains("binds light to `s_swing`, which fires nothing"), "{refused}");
}
