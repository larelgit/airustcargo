//! The control lock (F12): what a lock takes from `Intent` for a tick and gives back, the look cone, locks
//! held for several reasons, and the words around it (the `CONTROL` line, the tool refusals, the journal).

use ai_core::{AiIndex, AiJournal, SimSet};
use bevy::prelude::*;
use serde_json::json;
use shards_core::input_names::{JUMP, SPRINT};
use shards_core::{Gaze, Intent, Press, Target};
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

/// What the player's `Intent` held in the tick, after the lock and before the readers (`SimSet::Act`, after
/// `MotionSystems::Intent`), one entry per tick.
#[derive(Resource, Default)]
struct Probe(Vec<Held>);

#[derive(Clone, Debug, PartialEq)]
struct Held {
    move_dir: Vec3,
    held: Vec<String>,
    pressed: Vec<String>,
    look: Option<Gaze>,
}

fn arena() -> (App, Entity) {
    let mut app = bare_engine("control_lock");
    app.init_resource::<Probe>().add_systems(
        FixedUpdate,
        (|mut probe: ResMut<Probe>, intents: Query<&Intent>| {
            for intent in &intents {
                probe.0.push(Held {
                    move_dir: intent.move_dir,
                    held: intent.held.clone(),
                    pressed: intent.pressed.iter().map(|p| p.input.clone()).collect(),
                    look: intent.look,
                });
            }
        })
        .in_set(SimSet::Act)
        .after(MotionSystems::Lock)
        .before(MotionSystems::Intent),
    );
    floor(app.world_mut());
    let me = player(app.world_mut(), Vec3::ZERO);
    step(&mut app, 3);
    (app, me)
}

fn lock(app: &mut App, me: Entity, reason: &str, lock: ControlLock) {
    lock_controls(app.world_mut(), me, reason, lock).unwrap();
}

fn last(app: &App) -> Held {
    app.world().resource::<Probe>().0.last().cloned().expect("a tick ran")
}

fn journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect()
}

fn walks_east(app: &mut App, me: Entity) {
    edit_intent(app, me, |i| {
        i.move_dir = Vec3::X;
        i.set_held(SPRINT, true);
        i.set_held("block", true);
        i.set_held("crouch", true);
    });
}

#[test]
fn a_movement_lock_stops_the_walk_and_hands_the_keys_back_when_it_lifts() {
    let (mut app, me) = arena();
    walks_east(&mut app, me);
    lock(&mut app, me, "scene", ControlLock { movement: true, ..default() });
    let from = feet(&app, me);
    step(&mut app, 30);
    // The readers saw a body that asks for nothing to move by; the guard (an action input) is left.
    let seen = last(&app);
    assert_eq!(seen.move_dir, Vec3::ZERO);
    assert_eq!(seen.held, ["block"], "sprint and crouch are movement, block is not");
    assert!((feet(&app, me) - from).length() < 0.05, "it stood still: {}", feet(&app, me) - from);
    // Between ticks `Intent` is what the world wrote: the keyboard writes differences only, so nothing would put W back.
    let kept = intent(&app, me);
    assert_eq!(kept.move_dir, Vec3::X);
    assert!(kept.is_held(SPRINT) && kept.is_held("block") && kept.is_held("crouch"), "{:?}", kept.held);
    assert!(app.world().resource::<LockStash>().is_empty(), "nothing is kept across a tick");
    // The lock lifts: the same Intent walks at once.
    assert!(unlock_controls(app.world_mut(), me, "scene"));
    assert!(app.world().get::<ControlLocks>(me).is_none(), "the component goes with its last reason");
    step(&mut app, 30);
    assert!(feet(&app, me).x - from.x > 2.0, "it walks again: {}", feet(&app, me) - from);
    assert_eq!(last(&app).move_dir, Vec3::X);
}

#[test]
fn a_jump_press_is_movement_an_attack_press_is_an_action_and_a_roll_is_both() {
    let (mut app, me) = arena();
    let press = |app: &mut App| {
        edit_intent(app, me, |i| {
            i.pressed.push(Press { input: JUMP.into(), dir: None });
            i.pressed.push(Press { input: "light".into(), dir: None });
            i.pressed.push(Press { input: "dodge".into(), dir: None });
        });
    };
    press(&mut app);
    step(&mut app, 1);
    assert_eq!(last(&app).pressed, ["jump", "light", "dodge"], "no lock: all reach the readers");
    lock(&mut app, me, "a", ControlLock { movement: true, ..default() });
    press(&mut app);
    step(&mut app, 1);
    assert_eq!(last(&app).pressed, ["light"], "a roll is a move");
    lock(&mut app, me, "a", ControlLock { actions: true, ..default() });
    press(&mut app);
    step(&mut app, 1);
    assert_eq!(last(&app).pressed, ["jump"], "the same reason locks something else now, and a roll is an action too");
    lock(&mut app, me, "b", ControlLock { movement: true, ..default() });
    press(&mut app);
    step(&mut app, 1);
    assert!(last(&app).pressed.is_empty(), "two reasons, everything locked");
    assert_eq!(app.world().get::<ControlLocks>(me).unwrap().reasons().collect::<Vec<_>>(), ["a", "b"]);
    // Each lets go of its own.
    assert!(unlock_controls(app.world_mut(), me, "a"));
    assert!(!unlock_controls(app.world_mut(), me, "a"), "there is none to let go of any more");
    press(&mut app);
    step(&mut app, 1);
    assert_eq!(last(&app).pressed, ["light"], "b still holds movement");
    assert!(unlock_controls(app.world_mut(), me, "b"));
}

#[test]
fn a_look_cone_holds_the_look_in_the_intent_and_leaves_no_look_alone() {
    let (mut app, me) = arena();
    // North is yaw 0; the point is 10 m north of the body.
    let cone = ControlLock { look: LookLock::Cone { at: Target::Point(Vec3::new(0.0, 1.6, -10.0)), deg: 30.0 }, ..default() };
    lock(&mut app, me, "scene", cone);
    step(&mut app, 1);
    assert_eq!(intent(&app, me).look, None, "a body that does not look is not made to");
    edit_intent(&mut app, me, |i| i.look = Some(Gaze::from_degrees(10.0, 5.0)));
    step(&mut app, 1);
    assert_eq!(last(&app).look, Some(Gaze::from_degrees(10.0, 5.0)), "inside the cone it is untouched");
    edit_intent(&mut app, me, |i| i.look = Some(Gaze::from_degrees(90.0, 0.0)));
    step(&mut app, 1);
    let held = intent(&app, me).look.unwrap();
    assert!((held.yaw.to_degrees() - 30.0).abs() < 0.01 && held.pitch.abs() < 0.01, "on the edge of the cone: {held:?}");
    assert_eq!(last(&app).look, Some(held), "the readers saw the held look");
    // It is kept, not given back: the camera turns on from what the intent holds, so it does not wind up outside.
    step(&mut app, 3);
    assert_eq!(intent(&app, me).look, Some(held));
    // A look that has gone round more than once is measured the short way.
    edit_intent(&mut app, me, |i| i.look = Some(Gaze::from_degrees(-725.0, 0.0)));
    step(&mut app, 1);
    assert_eq!(intent(&app, me).look, Some(Gaze::from_degrees(-725.0, 0.0)), "a look that went round twice is 5 degrees off north: inside");
    // Free again: the look goes where it is told.
    assert!(unlock_controls(app.world_mut(), me, "scene"));
    edit_intent(&mut app, me, |i| i.look = Some(Gaze::from_degrees(90.0, 0.0)));
    step(&mut app, 1);
    assert_eq!(last(&app).look, Some(Gaze::from_degrees(90.0, 0.0)));
}

#[test]
fn a_cone_on_a_body_follows_the_body() {
    let (mut app, me) = arena();
    let target = player(app.world_mut(), Vec3::new(10.0, 0.0, 0.0));
    let id = *app.world().get::<ai_core::AiId>(target).unwrap();
    step(&mut app, 2);
    // The other body stands 10 m east (yaw -90 degrees), at its aim point 0.6 of 1.8 m.
    lock(&mut app, me, "scene", ControlLock { look: LookLock::Cone { at: Target::Entity(id), deg: 20.0 }, ..default() });
    edit_intent(&mut app, me, |i| i.look = Some(Gaze::from_degrees(0.0, 0.0)));
    step(&mut app, 1);
    let held = intent(&app, me).look.unwrap();
    assert!(held.yaw.to_degrees() > -90.0 && held.yaw.to_degrees() < -60.0, "{held:?}");
    // Gone: the cone has nothing to point at and the look is free of it.
    let gone = app.world().resource::<AiIndex>().get(id).unwrap();
    app.world_mut().despawn(gone);
    edit_intent(&mut app, me, |i| i.look = Some(Gaze::from_degrees(170.0, 0.0)));
    step(&mut app, 1);
    assert_eq!(intent(&app, me).look, Some(Gaze::from_degrees(170.0, 0.0)));
}

#[test]
fn the_lock_is_told_in_the_journal_and_in_observe_and_the_tools_say_why_they_refuse() {
    let (mut app, me) = arena();
    let said = ok(&mut app, "lock_controls", json!({"movement": true, "actions": true, "reason": "scene"}));
    assert!(said.contains("controls locked (scene): movement, actions"), "{said}");
    let observed = ai_core::observe::observe(app.world(), &ai_core::ObserveOptions::default());
    assert!(observed.lines().any(|l| l == "CONTROL looking only (scene)"), "{observed}");
    assert!(journal(&app).iter().any(|l| l.contains("controls locked (scene): movement, actions")), "{:#?}", journal(&app));
    // The player's tools act through Intent, so the lock stops them; a refusal says who holds it (the combat tools'
    // refusals are tested with the combat module, `tests/sandbox/sequence.rs`).
    let refused = fails(&mut app, "move", json!({"dir": "E", "meters": 3}));
    assert!(refused.contains("your movement is locked (scene)"), "{refused}");
    // A cone is told with its centre; a second reason adds to the first.
    let cone = ok(&mut app, "lock_controls", json!({"reason": "listen", "at": "0,1.6,-10", "deg": 25}));
    assert!(cone.contains("look within 25° of (0.0, 1.6, -10.0)"), "{cone}");
    let observed = ai_core::observe::observe(app.world(), &ai_core::ObserveOptions::default());
    assert!(observed.lines().any(|l| l == "CONTROL looking only, look within 25° of (0.0, 1.6, -10.0) (listen, scene)"), "{observed}");
    // Teaching errors.
    assert!(fails(&mut app, "lock_controls", json!({})).contains("nothing to lock"));
    assert!(fails(&mut app, "lock_controls", json!({"at": "e1"})).contains("needs `deg`"));
    assert!(fails(&mut app, "lock_controls", json!({"at": "nowhere", "deg": 10})).contains("a body (`e9`) or a point"));
    assert!(fails(&mut app, "lock_controls", json!({"movement": true, "at": "e1", "deg": 400})).contains("0 to 180"));
    assert!(fails(&mut app, "unlock_controls", json!({"reason": "nope"})).contains("holds: listen, scene"));
    let said = ok(&mut app, "unlock_controls", json!({"reason": "listen"}));
    assert!(said.contains("still locked by 1 other"), "{said}");
    ok(&mut app, "unlock_controls", json!({"all": true}));
    assert!(app.world().get::<ControlLocks>(me).is_none());
    assert!(fails(&mut app, "unlock_controls", json!({})).contains("has no locked controls"));
    assert!(journal(&app).iter().any(|l| l.contains("controls unlocked (scene)")), "{:#?}", journal(&app));
    // The words are told only while a lock holds.
    let observed = ai_core::observe::observe(app.world(), &ai_core::ObserveOptions::default());
    assert!(!observed.contains("CONTROL"), "{observed}");
    ok(&mut app, "move", json!({"dir": "E", "meters": 1}));
}

#[test]
fn locks_are_state_a_snapshot_keeps() {
    let (mut app, me) = arena();
    lock(&mut app, me, "scene", ControlLock { movement: true, look: LookLock::Cone { at: Target::Point(Vec3::new(1.0, 2.0, 3.0)), deg: 40.0 }, ..default() });
    lock(&mut app, me, "door", ControlLock { actions: true, ..default() });
    step(&mut app, 5);
    let saved = ai_core::snapshot::capture(app.world()).unwrap();
    assert!(saved.left_out.is_empty(), "{:?}", saved.left_out);
    let before = app.world().get::<ControlLocks>(me).unwrap().clone();
    let id = *app.world().get::<ai_core::AiId>(me).unwrap();
    unlock_controls(app.world_mut(), me, "scene");
    unlock_controls(app.world_mut(), me, "door");
    assert!(app.world().get::<ControlLocks>(me).is_none());
    let report = ai_core::snapshot::restore(app.world_mut(), &saved, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    // The load puts the bodies back under their ids (the entities may be new ones).
    let me = app.world().resource::<AiIndex>().get(id).unwrap();
    assert_eq!(app.world().get::<ControlLocks>(me), Some(&before));
}

#[test]
fn a_lock_needs_a_reason_a_body_with_controls_and_a_cone_that_means_something() {
    let (mut app, me) = arena();
    let bare = app.world_mut().spawn_empty().id();
    assert!(lock_controls(app.world_mut(), bare, "x", ControlLock::LOOKING_ONLY).unwrap_err().contains("no controls to lock"));
    assert!(lock_controls(app.world_mut(), me, " ", ControlLock::LOOKING_ONLY).unwrap_err().contains("needs a reason"));
    for deg in [0.0, -5.0, 181.0, f32::NAN, f32::INFINITY] {
        let cone = ControlLock { look: LookLock::Cone { at: Target::Point(Vec3::ZERO), deg }, ..default() };
        assert!(lock_controls(app.world_mut(), me, "x", cone).unwrap_err().contains("0 to 180"), "{deg}");
    }
    let cone = ControlLock { look: LookLock::Cone { at: Target::Point(Vec3::NAN), deg: 10.0 }, ..default() };
    assert!(lock_controls(app.world_mut(), me, "x", cone).is_err());
    assert!(app.world().get::<ControlLocks>(me).is_none(), "nothing was locked by a refused call");
    // The same lock again is no news.
    lock(&mut app, me, "x", ControlLock::LOOKING_ONLY);
    let lines = journal(&app).len();
    lock(&mut app, me, "x", ControlLock::LOOKING_ONLY);
    assert_eq!(journal(&app).len(), lines);
}

#[test]
fn a_glide_the_tool_latched_that_ends_under_the_lock_is_not_given_back() {
    use shards_core::input_names::GLIDE;
    let (mut app, me) = arena();
    app.world_mut().get_mut::<Transform>(me).unwrap().translation.y = 6.0;
    ok(&mut app, "glide", json!({}));
    assert!(intent(&app, me).is_held(GLIDE) && app.world().get::<GlideUntilLanded>(me).is_some(), "latched");
    step(&mut app, 5);
    lock(&mut app, me, "scene", ControlLock { movement: true, ..default() });
    // The body is let down by the glide it no longer asks for, lands, and the latch is gone with the hold.
    step(&mut app, 240);
    assert!(feet(&app, me).y < 0.1, "landed: {}", feet(&app, me));
    assert!(app.world().get::<GlideUntilLanded>(me).is_none(), "the latch ended with the landing");
    assert!(!intent(&app, me).is_held(GLIDE), "and the glide it ended is not put back: {:?}", intent(&app, me).held);
    // An ordinary hold (a key) comes back as it always does.
    unlock_controls(app.world_mut(), me, "scene");
    edit_intent(&mut app, me, |i| i.set_held(GLIDE, true));
    lock(&mut app, me, "scene", ControlLock { movement: true, ..default() });
    step(&mut app, 3);
    assert!(intent(&app, me).is_held(GLIDE), "a held key survives the lock");
}
