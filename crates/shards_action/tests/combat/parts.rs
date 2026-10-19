//! Body parts (R1): spawned on the body's first tick, placed in its frame, hit as the body
//! through the nearest part, their factors and their own health, breaking, switching on and off.

use bevy::prelude::*;
use serde_json::json;
use shards_action::{parts_of, ActionAppExt, BodyParts, DamageCtx, Damaged, Part, PartBroken, PartHealth, SpawnedParts};

use crate::common::*;

/// What the tests see of the pipeline: the hits and the breaks.
#[derive(Resource, Default)]
struct Seen {
    damaged: Vec<Damaged>,
    broken: Vec<(String, Option<Entity>)>,
}

/// The game's hook for weak spots: a hit tagged `weak` through the part `spot` deals double.
fn weak_spots(ctx: &mut DamageCtx, _: &World) {
    if ctx.tags.iter().any(|t| t == "weak") && ctx.part.as_ref().is_some_and(|p| p.id == "spot") {
        ctx.damage *= 2.0;
    }
}

/// The hero at the origin facing north, a brute of parts `def` 2 m north facing it (south).
fn arena(def: &str) -> (App, Entity, Entity) {
    arena_with(def, Vec3::ZERO, N)
}

/// [`arena`] with the hero at `at` facing `facing`.
fn arena_with(def: &str, at: Vec3, facing: Vec3) -> (App, Entity, Entity) {
    let mut app = app_with(|app| {
        app.init_resource::<Seen>();
        app.add_observer(|e: On<Damaged>, mut seen: ResMut<Seen>| seen.damaged.push(e.event().clone()));
        app.add_observer(|e: On<PartBroken>, mut seen: ResMut<Seen>| seen.broken.push((e.event().id.clone(), e.event().by)));
        app.add_damage_hook(weak_spots);
    });
    let world = app.world_mut();
    let hero = hero(world, "hero", at, facing);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.0), S);
    world.entity_mut(brute).insert(BodyParts(def.into()));
    step(&mut app, 1);
    (app, hero, brute)
}

fn seen(app: &App) -> &Seen {
    app.world().resource::<Seen>()
}

fn part(app: &App, body: Entity, id: &str) -> Entity {
    parts_of(app.world(), body).into_iter().find(|p| app.world().get::<Part>(*p).is_some_and(|q| q.id == id)).unwrap()
}

fn part_hp(app: &App, body: Entity, id: &str) -> f32 {
    app.world().get::<PartHealth>(part(app, body, id)).unwrap().current
}

/// One light swing of the hero, to its end.
fn swing(app: &mut App, hero: Entity) {
    press(app, hero, "light");
    step(app, 40);
}

#[test]
fn parts_spawn_on_the_first_tick_in_order_and_follow_their_body() {
    let (mut app, _hero, brute) = arena("giant");
    let b = id(&app, brute);
    let parts = parts_of(app.world(), brute);
    let names: Vec<String> = parts.iter().map(|p| app.world().get::<Part>(*p).unwrap().id.clone()).collect();
    assert_eq!(names, ["arm", "crown", "leg", "spot"], "by id");
    assert_eq!(app.world().get::<SpawnedParts>(brute).unwrap().0.len(), 4);
    assert!(parts.iter().all(|p| app.world().get::<Part>(*p).unwrap().body.to_string() == b));
    // The brute faces south (+Z): its forward is +Z, its right west (-X).
    let arm = app.world().get::<Transform>(part(&app, brute, "arm")).unwrap().translation;
    let feet = app.world().get::<Transform>(brute).unwrap().translation;
    assert!((arm - feet - Vec3::new(-0.5, 1.0, 0.8)).length() < 1e-3, "{arm} on {feet}");
    assert!(!app.world().get::<Part>(part(&app, brute, "spot")).unwrap().on, "starts off");
    assert_eq!(part_hp(&app, brute, "arm"), 60.0);

    app.world_mut().get_mut::<Transform>(brute).unwrap().translation = Vec3::new(4.0, 0.0, -2.0);
    step(&mut app, 1);
    let arm = app.world().get::<Transform>(part(&app, brute, "arm")).unwrap().translation;
    let feet = app.world().get::<Transform>(brute).unwrap().translation;
    assert!(feet.x > 3.9 && (arm - feet - Vec3::new(-0.5, 1.0, 0.8)).length() < 1e-3, "the arm went with the body: {arm} on {feet}");

    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.contains(&format!("arm of {b} 60/60 hp")), "{seen}");
    assert!(seen.contains(&format!("spot of {b} off")), "{seen}");
}

#[test]
fn a_swing_hits_the_body_once_through_its_nearest_part_with_its_factors() {
    let (mut app, hero, brute) = arena("giant");
    let (h, b) = (id(&app, hero), id(&app, brute));
    let arm = part(&app, brute, "arm");
    swing(&mut app, hero);
    // The cone overlaps the arm and the leg (and the capsule, which takes nothing): one hit, through
    // the arm, nearer the hero. 20 doubled: the arm loses 40, the body half of that.
    let hits = lines(&app, &format!("{h} -> {b}"));
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].ends_with(&format!("{h} -> {b} light_1 at arm -20 (hp 80)")), "{hits:?}");
    assert_eq!(hp(&app, brute), 80.0);
    assert_eq!(part_hp(&app, brute, "arm"), 20.0);
    let d = &seen(&app).damaged[0];
    assert_eq!((d.part, d.amount, d.poise), (Some(arm), 20.0, 10.0));
}

#[test]
fn a_part_breaks_when_its_health_runs_out_and_takes_no_more_hits() {
    let (mut app, hero, brute) = arena("giant");
    let (h, b) = (id(&app, hero), id(&app, brute));
    let arm = part(&app, brute, "arm");
    swing(&mut app, hero);
    swing(&mut app, hero);
    assert_eq!(seen(&app).broken, [("arm".to_owned(), Some(hero))]);
    let broken = lines(&app, "broken");
    assert!(broken.len() == 1 && broken[0].ends_with(&format!("{b} arm broken by {h}")), "{broken:?}");
    let state = app.world().get::<Part>(arm).unwrap();
    assert!(state.broken && !state.on);
    assert_eq!(part_hp(&app, brute, "arm"), 0.0);
    assert_eq!(hp(&app, brute), 60.0);

    // The arm takes no more hits: the next swing lands on the leg, which triples poise and passes
    // all its damage on.
    swing(&mut app, hero);
    let last = seen(&app).damaged.last().unwrap().clone();
    assert_eq!(last.part, Some(part(&app, brute, "leg")));
    assert_eq!((last.amount, last.poise), (20.0, 30.0));
    assert_eq!(hp(&app, brute), 40.0);
    assert_eq!(seen(&app).broken.len(), 1, "a part breaks once");
}

#[test]
fn a_body_hit_only_through_parts_ignores_its_capsule_and_an_open_one_does_not() {
    // From behind (north of the brute, facing south): the swing reaches the capsule, no part.
    let behind = Vec3::new(0.0, 0.0, -4.0);
    let (mut app, hero, brute) = arena_with("giant", behind, S);
    swing(&mut app, hero);
    assert!(seen(&app).damaged.is_empty(), "no part in reach: {:?}", seen(&app).damaged);
    assert_eq!(hp(&app, brute), 100.0);

    let (mut app, hero, _) = arena_with("open", behind, S);
    swing(&mut app, hero);
    let d = &seen(&app).damaged;
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!((d[0].part, d[0].amount), (None, 20.0), "the open body's capsule takes the hit");

    // From the front the cone overlaps the open body's arm and capsule: the part wins.
    let (mut app, hero, brute) = arena("open");
    swing(&mut app, hero);
    assert_eq!(seen(&app).damaged[0].part, Some(part(&app, brute, "arm")));
}

#[test]
fn a_part_that_is_off_takes_no_hits_until_it_is_on_and_its_tags_reach_the_hooks() {
    let (mut app, hero, brute) = arena("giant");
    let b = id(&app, brute);
    assert_eq!(ok(&mut app, "part", json!({"id": b, "name": "spot", "op": "on"})), format!("{b} spot on"));
    swing(&mut app, hero);
    // The spot, now on, is nearer than the arm: the hit goes through it, and the game's hook
    // doubles a hit tagged `weak` there.
    let d = &seen(&app).damaged[0];
    assert_eq!((d.part, d.amount), (Some(part(&app, brute, "spot")), 40.0));

    assert_eq!(ok(&mut app, "part", json!({"id": b, "name": "spot", "op": "off"})), format!("{b} spot off"));
    swing(&mut app, hero);
    assert_eq!(seen(&app).damaged[1].part, Some(part(&app, brute, "arm")), "off again: the arm");

    assert_eq!(ok(&mut app, "part", json!({"id": b, "name": "crown", "op": "break"})), format!("{b} crown broken"));
    assert_eq!(seen(&app).broken.last().unwrap(), &("crown".to_owned(), None));
    let refused = fails(&mut app, "part", json!({"id": b, "name": "crown", "op": "on"}));
    assert!(refused.contains("broken"), "{refused}");
    let unknown = fails(&mut app, "part", json!({"id": b, "name": "tail", "op": "on"}));
    assert!(unknown.contains("no part `tail`; its parts: arm, crown, leg, spot"), "{unknown}");
    let h = id(&app, hero);
    let none = fails(&mut app, "part", json!({"id": h, "name": "arm", "op": "on"}));
    assert!(none.contains("has no parts"), "{none}");
}

#[test]
fn parts_go_with_their_body() {
    let (mut app, _hero, brute) = arena("giant");
    let parts = parts_of(app.world(), brute);
    app.world_mut().despawn(brute);
    step(&mut app, 2);
    assert!(parts.iter().all(|p| app.world().get_entity(*p).is_err()), "every part went");
}
