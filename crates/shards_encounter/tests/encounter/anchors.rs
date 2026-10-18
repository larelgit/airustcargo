//! Anchors and keys: where a run's key comes from, duplicates, runs that outlive no anchor, and
//! `place_encounter`.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::{EncounterRuns, RunState};

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);

#[test]
fn the_key_comes_from_the_anchors_key_or_its_position_in_decimeters() {
    let mut app = app(1);
    shards::encounter::place_encounter(app.world_mut(), "single", Some("named"), Transform::from_xyz(1.0, 0.0, 1.0)).unwrap();
    shards::encounter::place_encounter(app.world_mut(), "single", None, Transform::from_xyz(3.04, 0.0, -5.06)).unwrap();
    step(&mut app, 2);
    let keys: Vec<String> = runs(&app).iter().map(|r| r.key.clone()).collect();
    assert_eq!(keys, vec!["world/named".to_owned(), "world/single@30,-51".to_owned()], "sorted by key");
}

#[test]
fn two_anchors_with_one_key_are_said_once_and_the_first_in_position_order_wins() {
    let mut app = app(1);
    let _p0 = player(&mut app, 0, FAR);
    shards::encounter::place_encounter(app.world_mut(), "single", Some("twin"), Transform::from_xyz(8.0, 0.0, 0.0)).unwrap();
    shards::encounter::place_encounter(app.world_mut(), "single", Some("twin"), Transform::from_xyz(-8.0, 0.0, 0.0)).unwrap();
    step(&mut app, 30);
    assert_eq!(runs(&app).len(), 1);
    assert_eq!(run(&app, "world/twin").at, Vec3::new(-8.0, 0.0, 0.0), "the lower x is first");
    assert_eq!(
        count(&app, "encounter: two anchors are keyed world/twin; give one its own key"),
        1,
        "{:?}",
        journal(&app)
    );
}

#[test]
fn an_anchor_for_an_encounter_that_is_not_in_content_stands_for_nothing_and_says_so_once() {
    let mut app = app(1);
    app.world_mut().spawn((shards::encounter::EncounterAnchor { encounter: "sngle".into(), key: Some("x".into()) }, Transform::default()));
    step(&mut app, 20);
    assert!(runs(&app).is_empty());
    assert_eq!(count(&app, "no encounter `sngle` in content (did you mean `single`?)"), 1, "{:?}", journal(&app));
}

#[test]
fn place_encounter_teaches_the_fix() {
    let mut app = app(1);
    let text = shards::encounter::place_encounter(app.world_mut(), "ring_wave", None, Transform::default()).unwrap_err();
    assert!(text.contains("no encounter `ring_wave` (did you mean `ring_waves`?)") && text.contains("encounters: "), "{text}");
}

#[test]
fn a_run_whose_anchor_is_gone_is_dropped_with_its_members_and_a_won_key_stays() {
    let mut app = watched(1);
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let a = shards::encounter::place_encounter(app.world_mut(), "manual", Some("m"), Transform::default()).unwrap();
    let b = shards::encounter::place_encounter(app.world_mut(), "ring_waves", Some("ring"), Transform::from_xyz(30.0, 0.0, 0.0)).unwrap();
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": "world/m"}));
    run_until(&mut app, 120, |x| run(x, "world/m").wave == 1);
    let member = members(&app, "world/m")[0].1;
    // Mark the other as won, as the win would.
    remember(&mut app, "world/ring");
    app.world_mut().despawn(a);
    tick(&mut app);
    assert!(app.world().get_entity(member).is_err(), "its members went silently");
    assert_eq!(runs(&app).iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), vec!["world/ring"]);
    assert!(journal_has(&app, "encounter world/m: gone with its anchor"), "{:?}", journal(&app));
    app.world_mut().despawn(b);
    tick(&mut app);
    assert!(runs(&app).is_empty());
    assert_eq!(app.world().resource::<EncounterRuns>().completed, vec!["world/ring".to_owned()], "a once key stays");
    assert_eq!(saw(&app, "died"), 0);
}

#[test]
fn a_run_of_a_remembered_key_is_won_with_no_members_and_no_event() {
    let mut app = watched(1);
    remember(&mut app, "world/ring");
    let _p0 = player(&mut app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(&mut app, "ring_waves", "ring", Vec3::ZERO);
    step(&mut app, 5);
    assert_eq!(state(&app, &key), RunState::Won);
    assert!(run(&app, &key).members.is_empty());
    assert_eq!(saw(&app, "completed"), 0, "no EncounterCompleted by a load or a revisit");
}
