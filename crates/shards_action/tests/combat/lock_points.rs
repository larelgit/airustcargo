//! Lock points (R3): parts marked `lock: true` are lock-on targets of their own, a body whose
//! parts say `lock_body: false` is taken only through them, a swing that overlaps the locked part
//! goes through it, and a lock point goes with its part (broken) and with its body (dead).

use std::path::Path;

use bevy::prelude::*;
use serde_json::json;
use shards_action::lock::{lock_on_valid, may_lock_on};
use shards_action::{kill, parts_of, BodyParts, Part};
use shards_content::db::{read_dir, Source};
use shards_content::{ContentDb, ContentKinds};
use shards_core::input_names::LOCK_SWITCH;
use shards_core::{flat, Intent};
use shards_motion::camera::LockOnDef;
use shards_motion::{LockOnTarget, LockPoint};

use crate::common::*;
use crate::lock::with_rules;

/// The rules on, the hero at the origin facing north, a brute of parts `pointed` 2.4 m north
/// facing it: its `front` and `back` points straight ahead of the hero, its `leg` to the hero's
/// right (+X) and its `hand` to the left.
fn arena() -> (App, Entity, Entity) {
    let mut app = app();
    with_rules(&mut app, "()");
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.4), S);
    world.entity_mut(brute).insert(BodyParts("pointed".into()));
    step(&mut app, 1);
    (app, hero, brute)
}

fn part(app: &App, body: Entity, name: &str) -> Entity {
    parts_of(app.world(), body).into_iter().find(|p| app.world().get::<Part>(*p).is_some_and(|q| q.id == name)).unwrap()
}

fn point(app: &App, body: Entity, name: &str) -> String {
    id(app, part(app, body, name))
}

fn target(app: &App, e: Entity) -> Option<String> {
    app.world().get::<Intent>(e).unwrap().target.map(|t| t.to_string())
}

/// One light swing of the hero, to its end.
fn swing(app: &mut App, hero: Entity) {
    press(app, hero, "light");
    step(app, 40);
}

#[test]
fn a_body_is_locked_on_to_only_through_its_lock_points() {
    let (mut app, hero, brute) = arena();
    let b = id(&app, brute);
    let (back, hand, crest) = (point(&app, brute, "back"), point(&app, brute, "hand"), point(&app, brute, "crest"));
    // The markers: on every lock point, not on the crest.
    let marked = |app: &App, name: &str| {
        let p = part(app, brute, name);
        app.world().get::<LockOnTarget>(p).is_some() && app.world().get::<LockPoint>(p).is_some()
    };
    assert!(["back", "front", "hand", "leg"].iter().all(|n| marked(&app, n)) && !marked(&app, "crest"));

    // The body by id: the best of its points the hero sees by the score (the back one: straight ahead, and
    // less far below the eye than the front one).
    let answer = ok(&mut app, "lock_on", json!({ "target": b }));
    assert!(answer.starts_with(&format!("ok: locked on {back} (back of {b})")), "{answer}");
    assert_eq!(target(&app, hero), Some(back.clone()));
    // Not the body itself, by the rules ...
    let def = LockOnDef::default();
    let body_id = *app.world().get::<ai_core::AiId>(brute).unwrap();
    let err = may_lock_on(app.world(), hero, brute, body_id, &def).unwrap_err();
    assert!(err.contains(&format!("{b} is locked on to through its lock points: back, front, hand, leg")), "{err}");
    // ... nor a part that is no lock point, nor a part it does not have.
    let err = fails(&mut app, "lock_on", json!({ "target": crest }));
    assert!(err.contains(&format!("{crest} (crest of {b}) is not a lock point; its lock points: back, front, hand, leg")), "{err}");
    assert!(fails(&mut app, "lock_on", json!({ "point": "crest" })).contains("is not a lock point"));
    assert!(fails(&mut app, "lock_on", json!({ "point": "tail" })).contains("no part `tail`; its parts: back, crest, front, hand, leg"));
    // The pick never names the body.
    ok(&mut app, "lock_on", json!({ "off": true }));
    let picked = ok(&mut app, "lock_on", json!({}));
    assert!(picked.contains(&format!(" of {b})")), "{picked}");

    // A point by name, of the body the lock is on: the lock, `inspect` and the journal name it with its body.
    let answer = ok(&mut app, "lock_on", json!({ "point": "hand" }));
    assert!(answer.starts_with(&format!("ok: locked on {hand} (hand of {b})")), "{answer}");
    step(&mut app, 1);
    let h = id(&app, hero);
    assert!(ok(&mut app, "inspect", json!({ "id": h })).contains(&format!("lock {hand} (hand of {b})")));
    assert_eq!(lines(&app, &format!("{h} locks {hand} (hand of {b})")).len(), 1, "{:?}", journal(&app));

    // The body set as the target some other way (a client's input, a game system) is let go of.
    app.world_mut().get_mut::<Intent>(hero).unwrap().target = Some(body_id);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None);
    assert_eq!(lines(&app, &format!("{h} lock released: {b}")).len(), 1, "{:?}", journal(&app));
    assert_eq!(lock_on_valid(app.world()), None);
}

#[test]
fn a_swing_goes_through_the_locked_point_before_a_nearer_one() {
    let (mut app, hero, brute) = arena();
    let b = id(&app, brute);
    // Locked on the back point: the swing overlaps both on the line ahead, and goes through the back one.
    ok(&mut app, "lock_on", json!({ "point": "back" }));
    step(&mut app, 2);
    swing(&mut app, hero);
    assert_eq!(lines(&app, "at back").len(), 1, "{:?}", journal(&app));
    assert!(lines(&app, "at front").is_empty(), "{:?}", journal(&app));
    // Without a lock the part nearest the middle of the swing takes it: the front one.
    ok(&mut app, "lock_on", json!({ "off": true }));
    step(&mut app, 20);
    swing(&mut app, hero);
    assert_eq!(lines(&app, &format!("-> {b} light_1 at front")).len(), 1, "{:?}", journal(&app));
}

#[test]
fn the_body_turns_to_a_locked_point_and_a_flick_moves_between_points() {
    let (mut app, hero, brute) = arena();
    let b = id(&app, brute);
    let (front, leg) = (point(&app, brute, "front"), point(&app, brute, "leg"));
    ok(&mut app, "lock_on", json!({ "point": "front" }));
    step(&mut app, 1);
    // Right: the leg, off to the hero's right.
    app.world_mut().get_mut::<Intent>(hero).unwrap().press_dir(LOCK_SWITCH, Vec3::X);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), Some(leg.clone()));
    let h = id(&app, hero);
    let line = format!("{h} locks {leg} (leg of {b}) (switched right from {front} (front of {b}))");
    assert_eq!(lines(&app, &line).len(), 1, "{:?}", journal(&app));
    // The hero turns to face the leg, not the body.
    step(&mut app, 30);
    let at = |e: Entity| app.world().get::<Transform>(e).unwrap().translation;
    let to_leg = flat(at(part(&app, brute, "leg")) - at(hero)).normalize();
    let facing = flat(*app.world().get::<Transform>(hero).unwrap().forward()).normalize();
    assert!(facing.dot(to_leg) > 0.99, "faces {facing}, the leg is {to_leg}");
}

#[test]
fn a_lock_point_goes_with_its_part_and_with_its_body() {
    let (mut app, hero, brute) = arena();
    let b = id(&app, brute);
    let (hand, leg) = (point(&app, brute, "hand"), point(&app, brute, "leg"));
    let h = id(&app, hero);
    // The hand (20 hp) breaks under one swing: no lock point any more, so the lock is let go of.
    ok(&mut app, "lock_on", json!({ "point": "hand" }));
    step(&mut app, 30);
    swing(&mut app, hero);
    assert_eq!(lines(&app, &format!("{b} hand broken by {h}")).len(), 1, "{:?}", journal(&app));
    assert_eq!(target(&app, hero), None);
    assert_eq!(lines(&app, &format!("{h} lock released: {hand} (hand of {b})")).len(), 1, "{:?}", journal(&app));
    let err = fails(&mut app, "lock_on", json!({ "target": hand }));
    assert!(err.contains(&format!("{hand} (hand of {b}) is broken; its lock points: back, front, leg")), "{err}");
    // An attack order on the body leaves a lock on one of its points as it is.
    ok(&mut app, "lock_on", json!({ "point": "leg" }));
    ok(&mut app, "attack", json!({ "target": b }));
    step(&mut app, 1);
    assert_eq!(target(&app, hero), Some(leg.clone()));
    ok(&mut app, "stop", json!({}));
    // The body dies: the lock on its point is lost, `dead`.
    kill(app.world_mut(), brute, None, None);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None);
    assert_eq!(lines(&app, &format!("{h} lock lost: {leg} (leg of {b}) dead")).len(), 1, "{:?}", journal(&app));
    assert_eq!(lock_on_valid(app.world()), None);
    let err = fails(&mut app, "lock_on", json!({ "target": b }));
    assert!(err.contains(&format!("{b} is dead")), "{err}");
}

#[test]
fn a_prefab_that_marks_a_body_taken_only_through_its_points_is_a_content_error() {
    let app = app();
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources = read_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/combat/assets"))).unwrap();
    sources.push(Source {
        path: "content/marked.prefab.ron".into(),
        text: r#"{
            "t_marked": (components: {"BodyParts": ("pointed"), "LockOnTarget": ()}),
            "t_open": (components: {"BodyParts": ("open"), "LockOnTarget": ()}),
            "t_lost": (components: {"BodyParts": ("nowhere")}),
        }"#
        .into(),
    });
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    let marked = "LockOnTarget on a body whose parts `pointed` say `lock_body: false`";
    assert_eq!(errors.iter().filter(|e| e.contains(marked)).count(), 1, "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("t_lost") && e.contains("nowhere")), "{errors:?}");
    assert_eq!(errors.len(), 2, "a marked body that also takes the lock itself is fine: {errors:?}");
}
