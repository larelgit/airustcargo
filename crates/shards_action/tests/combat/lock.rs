//! The lock-on rules (C6): with a `lock_on` definition the simulation checks every lock on every
//! tick (dead, gone, out of range, hidden too long) and carries out `lock_switch` presses; without
//! one, nothing checks a lock and the `lock_on` tool picks as before.

use std::path::Path;

use bevy::prelude::*;
use serde_json::json;
use shards_action::lock::lock_on_valid;
use shards_action::{kill, LockSight, LockSystems};
use shards_content::db::{read_dir, Source};
use shards_content::{ContentDb, ContentKinds};
use shards_core::input_names::LOCK_SWITCH;
use shards_core::{CoreSystems, Intent};

use crate::common::*;

/// The test content plus a `lock_on` definition: the rules are on.
fn with_rules(app: &mut App, def: &str) {
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources = read_dir(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/combat/assets"))).unwrap();
    sources.push(Source { path: "content/lock.lock_on.ron".into(), text: format!("{{ \"default\": {def} }}") });
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:?}");
    app.world_mut().insert_resource(db);
}

fn target(app: &App, e: Entity) -> Option<String> {
    app.world().get::<Intent>(e).unwrap().target.map(|t| t.to_string())
}

fn sight(app: &App, e: Entity) -> Option<u32> {
    app.world().get::<LockSight>(e).map(|s| s.ticks)
}

fn inspect(app: &mut App, e: Entity) -> String {
    let id = id(app, e);
    ok(app, "inspect", json!({ "id": id }))
}

#[test]
fn without_a_definition_nothing_checks_a_lock() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -3.0), S);
    wall(world, Vec3::new(-3.0, 0.0, -1.8), Vec3::new(3.0, 3.0, -1.2));
    let b = id(&app, brute);
    assert_eq!(ok(&mut app, "lock_on", json!({ "target": b })), format!("ok: locked on {b} (3.0m N)"));
    step(&mut app, 200);
    kill(app.world_mut(), brute, None, None);
    step(&mut app, 5);
    assert_eq!(target(&app, hero), Some(b.clone()), "hidden for 3 s, then dead: kept, as before the rules");
    assert_eq!(sight(&app, hero), None);
    assert!(lines(&app, "lock").is_empty(), "{:?}", journal(&app));
    assert!(inspect(&mut app, hero).contains(&format!("lock {b}")));
}

#[test]
fn a_lock_is_lost_when_its_target_dies_goes_or_leaves_the_range() {
    let mut app = app();
    with_rules(&mut app, "(range: 12.0)");
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let a = brute(world, Vec3::new(0.0, 0.0, -3.0), S);
    let b = brute(world, Vec3::new(2.0, 0.0, -3.0), S);
    let c = brute(world, Vec3::new(-2.0, 0.0, -3.0), S);
    let (ia, ib, ic, ih) = (id(&app, a), id(&app, b), id(&app, c), id(&app, hero));

    ok(&mut app, "lock_on", json!({ "target": ia }));
    step(&mut app, 1);
    assert!(lines(&app, &format!("{ih} locks {ia}")).len() == 1, "{:?}", journal(&app));
    kill(app.world_mut(), a, None, None);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None);
    assert_eq!(lines(&app, "lock lost").last().map(|l| l.split_once(' ').unwrap().1.to_owned()), Some(format!("{ih} lock lost: {ia} dead")));
    assert!(fails(&mut app, "lock_on", json!({ "target": ia })).contains("is dead"));

    ok(&mut app, "lock_on", json!({ "target": ib }));
    step(&mut app, 1);
    app.world_mut().despawn(b);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None);
    assert!(lines(&app, &format!("{ih} lock lost: {ib} gone")).len() == 1, "{:?}", journal(&app));

    ok(&mut app, "lock_on", json!({ "target": ic }));
    app.world_mut().get_mut::<Transform>(c).unwrap().translation = Vec3::new(0.0, 0.0, -12.5);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None);
    assert!(lines(&app, &format!("{ih} lock lost: {ic} out of range")).len() == 1, "{:?}", journal(&app));
    assert!(fails(&mut app, "lock_on", json!({ "target": ic })).contains("beyond the lock-on range of 12m"));
}

#[test]
fn a_target_hidden_for_the_obstruction_time_is_lost_and_a_break_starts_the_count_again() {
    let mut app = app();
    with_rules(&mut app, "(obstruction_s: Some(1.0))");
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -4.0), S);
    let wall = wall(world, Vec3::new(-2.0, 0.0, -2.3), Vec3::new(2.0, 3.0, -1.7));
    let (b, h) = (id(&app, brute), id(&app, hero));
    ok(&mut app, "lock_on", json!({ "target": b }));
    step(&mut app, 30);
    assert_eq!(sight(&app, hero), Some(30));
    assert!(inspect(&mut app, hero).contains(&format!("lock {b} (obstructed 0.5s)")), "{}", inspect(&mut app, hero));
    // In sight for a tick: the count goes, and starts again from 0.
    app.world_mut().get_mut::<Transform>(wall).unwrap().translation.y = -10.0;
    step(&mut app, 1);
    assert_eq!(sight(&app, hero), None);
    app.world_mut().get_mut::<Transform>(wall).unwrap().translation.y = 1.5;
    step(&mut app, 59);
    assert_eq!((target(&app, hero), sight(&app, hero)), (Some(b.clone()), Some(59)), "1 s at 60 Hz is 60 ticks");
    step(&mut app, 1);
    assert_eq!((target(&app, hero), sight(&app, hero)), (None, None));
    assert!(lines(&app, &format!("{h} lock lost: {b} obstructed 1.0s")).len() == 1, "{:?}", journal(&app));

    // `obstruction_s: None`: hiding never breaks a lock, but the count still shows.
    let mut app = crate::common::app();
    with_rules(&mut app, "(obstruction_s: None)");
    let world = app.world_mut();
    let hero = crate::common::hero(world, "hero", Vec3::ZERO, N);
    let brute = crate::common::brute(world, Vec3::new(0.0, 0.0, -4.0), S);
    crate::common::wall(world, Vec3::new(-2.0, 0.0, -2.3), Vec3::new(2.0, 3.0, -1.7));
    let b = id(&app, brute);
    ok(&mut app, "lock_on", json!({ "target": b }));
    step(&mut app, 300);
    assert_eq!((target(&app, hero), sight(&app, hero)), (Some(b), Some(300)));
}

#[test]
fn a_lock_switch_press_moves_the_lock_to_the_nearest_angle_on_that_side() {
    let mut app = app();
    with_rules(&mut app, "()");
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let ahead = brute(world, Vec3::new(0.0, 0.0, -8.0), S);
    let right_near = brute(world, Vec3::new(4.0, 0.0, -8.0), S);
    let right_far = brute(world, Vec3::new(12.0, 0.0, -4.0), S);
    let left = brute(world, Vec3::new(-5.0, 0.0, -8.0), S);
    let behind_wall = brute(world, Vec3::new(1.5, 0.0, -8.0), S);
    wall(world, Vec3::new(0.6, 0.0, -6.0), Vec3::new(1.4, 3.0, -5.0));
    let ids: Vec<String> = [ahead, right_near, right_far, left].iter().map(|e| id(&app, *e)).collect();
    let h = id(&app, hero);
    ok(&mut app, "lock_on", json!({ "target": ids[0] }));
    step(&mut app, 1);
    let press = |app: &mut App, x: f32| {
        app.world_mut().get_mut::<Intent>(hero).unwrap().press_dir(LOCK_SWITCH, Vec3::X * x);
        step(app, 1);
    };
    // Right: the one behind the wall is nearer in angle, but cannot be seen.
    press(&mut app, 1.0);
    assert_eq!(target(&app, hero), Some(ids[1].clone()));
    assert!(lines(&app, &format!("{h} locks {} (switched right from {})", ids[1], ids[0])).len() == 1, "{:?}", journal(&app));
    press(&mut app, 1.0);
    assert_eq!(target(&app, hero), Some(ids[2].clone()));
    press(&mut app, 1.0);
    assert_eq!(target(&app, hero), Some(ids[2].clone()), "nobody further right: the lock stays");
    // Two presses in one tick: carried out in order.
    app.world_mut().get_mut::<Intent>(hero).unwrap().press_dir(LOCK_SWITCH, Vec3::NEG_X);
    press(&mut app, -1.0);
    assert_eq!(target(&app, hero), Some(ids[0].clone()));
    press(&mut app, -1.0);
    assert_eq!(target(&app, hero), Some(ids[3].clone()));
    assert!(app.world().get::<Intent>(hero).unwrap().pressed.is_empty(), "the presses are used up");
    // The tool does the same, and answers.
    let id_wall = id(&app, behind_wall);
    assert_eq!(ok(&mut app, "lock_on", json!({ "side": "right" })).split(" (").next(), Some(format!("ok: locked on {}", ids[0]).as_str()));
    assert!(!ok(&mut app, "lock_on", json!({ "side": "right" })).contains(&id_wall));
    ok(&mut app, "lock_on", json!({ "off": true }));
    assert!(fails(&mut app, "lock_on", json!({ "side": "left" })).contains("you hold no lock"));
}

#[test]
fn a_switch_works_without_a_definition_by_its_defaults() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let ahead = brute(world, Vec3::new(0.0, 0.0, -8.0), S);
    let right = brute(world, Vec3::new(4.0, 0.0, -8.0), S);
    let (a, r) = (id(&app, ahead), id(&app, right));
    ok(&mut app, "lock_on", json!({ "target": a }));
    app.world_mut().get_mut::<Intent>(hero).unwrap().press_dir(LOCK_SWITCH, Vec3::X);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), Some(r.clone()));
    assert_eq!(sight(&app, hero), None, "no count without a definition");
    assert!(ok(&mut app, "lock_on", json!({ "side": "left" })).starts_with(&format!("ok: locked on {a}")));
}

#[test]
fn the_tool_picks_by_the_score_among_what_the_body_sees() {
    let mut app = app();
    with_rules(&mut app, "()");
    let world = app.world_mut();
    hero(world, "hero", Vec3::ZERO, N);
    // The nearest is behind a wall; of the two in sight, the one ahead beats the nearer one to
    // the side.
    let hidden = brute(world, Vec3::new(-0.8, 0.0, -2.5), S);
    let ahead = brute(world, Vec3::new(0.0, 0.0, -9.0), S);
    let side = brute(world, Vec3::new(6.0, 0.0, 0.0), S);
    wall(world, Vec3::new(-1.6, 0.0, -1.6), Vec3::new(-0.3, 3.0, -1.2));
    step(&mut app, 1);
    let (_hidden, ahead, _side) = (id(&app, hidden), id(&app, ahead), id(&app, side));
    assert!(ok(&mut app, "lock_on", json!({})).starts_with(&format!("ok: locked on {ahead} ")));
    assert!(fails(&mut app, "lock_on", json!({ "side": "left", "target": ahead })).contains("not both"));
}

#[test]
fn with_the_rules_the_tool_refuses_what_the_pick_would_never_name() {
    let mut app = app();
    with_rules(&mut app, "()");
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let ally = fighter(world, "Squire", "brute", "heroes", Vec3::new(2.0, 0.0, -3.0), S);
    let nowhere = world.spawn((Name::new("note"), ai_core::AiVisible)).id();
    let brute = brute(world, Vec3::new(0.0, 0.0, -4.0), S);
    step(&mut app, 1);
    let (a, n, b, h) = (id(&app, ally), id(&app, nowhere), id(&app, brute), id(&app, hero));
    let err = fails(&mut app, "lock_on", json!({ "target": a }));
    assert!(err.contains(&format!("{a} is not something you can lock on to")), "{err}");
    assert!(fails(&mut app, "lock_on", json!({ "target": n })).contains("has no position"));
    assert_eq!(target(&app, hero), None);
    ok(&mut app, "lock_on", json!({ "target": b }));
    // The cancel says so, with the rules on.
    assert_eq!(ok(&mut app, "lock_on", json!({ "off": true })), "ok: lock-on released");
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None);
    assert_eq!(lines(&app, &format!("{h} lock released: {b}")).len(), 1, "{:?}", journal(&app));

    // Without the rules an id is taken as before, an ally too, and a cancel has no line.
    let mut app = crate::common::app();
    let world = app.world_mut();
    let hero = crate::common::hero(world, "hero", Vec3::ZERO, N);
    let ally = fighter(world, "Squire", "brute", "heroes", Vec3::new(2.0, 0.0, -3.0), S);
    step(&mut app, 1);
    let a = id(&app, ally);
    ok(&mut app, "lock_on", json!({ "target": a }));
    assert_eq!(target(&app, hero), Some(a));
    ok(&mut app, "lock_on", json!({ "off": true }));
    step(&mut app, 1);
    assert!(lines(&app, "lock").is_empty(), "{:?}", journal(&app));
}

/// A game system that takes the locked-on body away at the end of the tick, placed like the
/// co-op despawn of a body whose rejoin grace is over (`SimSet::Cleanup`, before
/// `CoreSystems::ClearPresses`), or after the lock-on check: on the first tick it runs.
fn despawner(app: &mut App, victim: Entity, after_check: bool) {
    let despawn = move |mut commands: Commands, mut done: Local<bool>| {
        if !std::mem::replace(&mut *done, true) {
            commands.entity(victim).despawn();
        }
    };
    let placed = despawn.in_set(ai_core::SimSet::Cleanup);
    match after_check {
        false => app.add_systems(FixedUpdate, placed.before(CoreSystems::ClearPresses)),
        true => app.add_systems(FixedUpdate, placed.after(LockSystems::Check)),
    };
}

#[test]
fn a_target_despawned_before_the_presses_are_cleared_is_let_go_of_on_that_tick() {
    let mut app = app();
    with_rules(&mut app, "()");
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -3.0), S);
    let (b, h) = (id(&app, brute), id(&app, hero));
    ok(&mut app, "lock_on", json!({ "target": b }));
    step(&mut app, 1);
    despawner(&mut app, brute, false);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), None, "the living target went in Cleanup: let go of on that tick");
    assert_eq!(lines(&app, &format!("{h} lock lost: {b} gone")).len(), 1, "{:?}", journal(&app));
    assert_eq!(lock_on_valid(app.world()), None);

    // Control: the same despawn after the check leaves a lock on nobody, and the rule says so.
    let mut app = crate::common::app();
    with_rules(&mut app, "()");
    let world = app.world_mut();
    let hero = crate::common::hero(world, "hero", Vec3::ZERO, N);
    let brute = crate::common::brute(world, Vec3::new(0.0, 0.0, -3.0), S);
    let b = id(&app, brute);
    ok(&mut app, "lock_on", json!({ "target": b }));
    step(&mut app, 1);
    despawner(&mut app, brute, true);
    step(&mut app, 1);
    assert_eq!(target(&app, hero), Some(b.clone()));
    let report = lock_on_valid(app.world()).expect("a lock on a body taken away after the check is reported");
    assert!(report.contains(&format!("Intent.target = {b}, which no longer exists")), "{report}");
}

#[test]
fn a_lock_on_definition_under_another_id_is_a_content_error() {
    let app = app();
    let kinds = app.world().resource::<ContentKinds>().clone();
    let sources = vec![Source { path: "content/lock.lock_on.ron".into(), text: "{ \"defualt\": (range: 25.0) }".into() }];
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    assert!(errors.iter().any(|e| e.contains("reads only the lock-on rules `default`")), "{errors:?}");
}
