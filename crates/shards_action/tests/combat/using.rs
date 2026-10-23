//! Use by looking (F5): a press of `interact` that finds a `Usable`, a hold that takes time, the `Used`
//! event, the `interact` tool, and the revive that keeps its press.

use ai_core::AiKind;
use bevy::prelude::*;
use serde_json::json;
use shards_action::{go_down, kill, use_view, Downed, UseProgress, Used};
use shards_core::{Gaze, Intent, PlayerId};
use shards_motion::{Collider, CollisionLayers, Shape, Usable};

use crate::common::*;

/// Every `Used` the game heard: who, what, the name.
#[derive(Resource, Default)]
struct Seen(Vec<(Entity, Entity, String)>);

fn watch(used: On<Used>, mut seen: ResMut<Seen>) {
    seen.0.push((used.who, used.what, used.name.clone()));
}

fn seen(app: &App) -> Vec<(Entity, Entity, String)> {
    app.world().resource::<Seen>().0.clone()
}

/// The hero (`l_hero`: a moveset whose `interact` is a revive) at the origin facing north, its eye 1.62 m up.
fn arena() -> (App, Entity) {
    arena_of("l_hero")
}

/// The same with a moveset that binds nothing to `interact`.
fn plain() -> (App, Entity) {
    arena_of("l_plain")
}

fn arena_of(moveset: &str) -> (App, Entity) {
    let mut app = app_with(|app| {
        app.init_resource::<Seen>().add_observer(watch);
    });
    let hero = hero(app.world_mut(), moveset, Vec3::ZERO, N);
    app.world_mut().entity_mut(hero).insert(PlayerId(0));
    step(&mut app, 2);
    (app, hero)
}

/// A usable thing called `name`: a use area 0.2 m wide with its middle `ahead` metres in front of the
/// hero's eye, level with it.
fn thing(app: &mut App, name: &str, usable: Usable, ahead: f32) -> Entity {
    thing_at(app, name, usable, Vec3::new(0.0, 1.62, -ahead))
}

fn thing_at(app: &mut App, name: &str, usable: Usable, at: Vec3) -> Entity {
    let usable = Usable { name: name.to_owned(), ..usable };
    let e = app
        .world_mut()
        .spawn((usable, Collider::new(Shape::cuboid(Vec3::splat(0.1))).with_layers(CollisionLayers::USE), Transform::from_translation(at)))
        .id();
    // A tick, so the physics world has it before a tool between ticks looks.
    step(app, 1);
    e
}

fn progress(app: &App, e: Entity) -> Option<f32> {
    app.world().get::<UseProgress>(e).map(|p| p.elapsed)
}

fn look(app: &mut App, e: Entity, yaw: f32) {
    app.world_mut().get_mut::<Intent>(e).unwrap().look = Some(Gaze::from_degrees(yaw, 0.0));
}

// --- a press ---------------------------------------------------------------------------------

#[test]
fn a_press_that_finds_a_thing_in_reach_uses_it_once() {
    let (mut app, hero) = arena();
    let lever = thing(&mut app, "lever", Usable::default(), 1.1);
    press(&mut app, hero, "interact");
    step(&mut app, 1);
    assert_eq!(seen(&app), [(hero, lever, "lever".to_owned())]);
    let line = format!("{} uses {} \"lever\"", id(&app, hero), id(&app, lever));
    assert_eq!(lines(&app, &line).len(), 1, "journalled: {:#?}", journal(&app));
    // The press was taken: nothing buffers it for the machine, and nothing uses the lever again.
    assert!(app.world().get::<Intent>(hero).unwrap().pressed.is_empty());
    step(&mut app, 30);
    assert_eq!(seen(&app).len(), 1);
    assert_eq!(action(&app, hero), "idle");
}

#[test]
fn nothing_is_used_out_of_reach_out_of_line_or_without_a_press() {
    let (mut app, hero) = arena();
    thing(&mut app, "far", Usable::default(), 2.5);
    thing_at(&mut app, "aside", Usable::default(), Vec3::new(1.0, 1.62, -1.0));
    press(&mut app, hero, "interact");
    step(&mut app, 2);
    assert!(seen(&app).is_empty(), "2.5 m is past the 2 m reach and the other is off the line");
    // In reach and in line, but nobody presses.
    thing(&mut app, "near", Usable::default(), 1.0);
    step(&mut app, 10);
    assert!(seen(&app).is_empty());
    assert!(use_view(app.world(), hero).is_some_and(|v| v.name == "near" && v.hold_s == 0.0 && v.progress.is_none()));
}

#[test]
fn a_wall_between_blocks_the_use_and_a_glass_pane_is_a_wall() {
    let (mut app, hero) = arena();
    thing(&mut app, "behind", Usable::default(), 1.6);
    app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(1.0, 1.0, 0.02))), Transform::from_xyz(0.0, 1.62, -0.9)));
    press(&mut app, hero, "interact");
    step(&mut app, 2);
    assert!(seen(&app).is_empty() && use_view(app.world(), hero).is_none());
}

#[test]
fn the_dead_and_the_downed_use_nothing() {
    let (mut app, hero) = arena();
    thing(&mut app, "lever", Usable::default(), 1.1);
    assert!(go_down(app.world_mut(), hero, None, None));
    press(&mut app, hero, "interact");
    step(&mut app, 2);
    assert!(seen(&app).is_empty(), "a downed body uses nothing");
    let (mut app, hero) = arena();
    thing(&mut app, "lever", Usable::default(), 1.1);
    kill(app.world_mut(), hero, None, None);
    press(&mut app, hero, "interact");
    step(&mut app, 2);
    assert!(seen(&app).is_empty(), "nor a dead one");
}

#[test]
fn users_are_served_in_ai_id_order_not_entity_order() {
    let (mut app, _hero) = arena();
    // Two more users; the earlier `Entity` is renumbered so that it has the higher `AiId`.
    let first = fighter(app.world_mut(), "Hero", "l_plain", "heroes", Vec3::new(10.0, 0.0, 0.0), N);
    let second = fighter(app.world_mut(), "Hero", "l_plain", "heroes", Vec3::new(20.0, 0.0, 0.0), N);
    app.world_mut().entity_mut(first).insert(ai_core::AiId(0));
    assert!(first.index() < second.index(), "the premise: {first:?} {second:?}");
    let ids = (app.world().get::<ai_core::AiId>(first).copied(), app.world().get::<ai_core::AiId>(second).copied());
    assert!(ids.1 < ids.0, "the premise: {ids:?}");
    for (x, name) in [(10.0, "one"), (20.0, "two")] {
        thing_at(&mut app, name, Usable::default(), Vec3::new(x, 1.62, -1.0));
    }
    step(&mut app, 2);
    for user in [first, second] {
        press(&mut app, user, "interact");
    }
    step(&mut app, 1);
    let names: Vec<String> = seen(&app).into_iter().map(|(.., name)| name).collect();
    assert_eq!(names, ["two", "one"], "the lower AiId first, whatever the entities' order");
}

// --- a hold ----------------------------------------------------------------------------------

/// A valve that takes a second, 1.1 m ahead; the hero presses and holds.
fn holding() -> (App, Entity, Entity) {
    let (mut app, hero) = arena();
    let valve = thing(&mut app, "valve", Usable::new("valve").held(1.0), 1.1);
    press(&mut app, hero, "interact");
    hold(&mut app, hero, "interact", true);
    step(&mut app, 1);
    (app, hero, valve)
}

/// The ticks a hold of 1 s takes after the press that starts it, and the length of a tick.
fn hold_ticks(app: &App) -> (u64, f32) {
    let dt = app.world().resource::<Time<Fixed>>().timestep().as_secs_f32();
    (((1.0 - 1e-4) / dt).ceil() as u64, dt)
}

#[test]
fn a_hold_grows_while_held_and_completes_once() {
    let (mut app, hero, valve) = holding();
    let (ticks, dt) = hold_ticks(&app);
    assert_eq!(progress(&app, hero), Some(0.0), "the press starts it");
    assert_eq!(app.world().get::<UseProgress>(hero).map(|p| p.what), app.world().get::<ai_core::AiId>(valve).copied());
    assert!(seen(&app).is_empty());
    step(&mut app, 30);
    let half = progress(&app, hero).unwrap();
    assert!((half - 30.0 * dt).abs() < 1e-4, "{half}");
    let view = use_view(app.world(), hero).unwrap();
    assert!(view.progress.is_some_and(|(held, of)| of == 1.0 && (held - half).abs() < 1e-6), "{view:?}");
    step(&mut app, ticks - 31);
    assert!(seen(&app).is_empty(), "one tick short: {:?}", progress(&app, hero));
    step(&mut app, 1);
    assert_eq!(seen(&app), [(hero, valve, "valve".to_owned())], "at {ticks} ticks");
    assert_eq!(progress(&app, hero), None, "the use is over");
    // Still held: nothing starts again without a new press.
    step(&mut app, 90);
    assert_eq!(seen(&app).len(), 1);
    assert_eq!(progress(&app, hero), None);
    // A new press starts another.
    press(&mut app, hero, "interact");
    step(&mut app, 1);
    assert_eq!(progress(&app, hero), Some(0.0));
}

#[test]
fn letting_go_looking_away_or_walking_off_cancels_a_hold() {
    // Let go.
    let (mut app, hero, _) = holding();
    step(&mut app, 20);
    assert!(progress(&app, hero).is_some());
    hold(&mut app, hero, "interact", false);
    step(&mut app, 1);
    assert_eq!(progress(&app, hero), None);
    step(&mut app, 120);
    assert!(seen(&app).is_empty());

    // Look away: it ends, and looking back while the button is still down does not restart it.
    let (mut app, hero, _) = holding();
    step(&mut app, 20);
    look(&mut app, hero, 90.0);
    step(&mut app, 1);
    assert_eq!(progress(&app, hero), None);
    look(&mut app, hero, 0.0);
    step(&mut app, 120);
    assert!(progress(&app, hero).is_none() && seen(&app).is_empty());

    // Out of reach: the body is carried 3 m away.
    let (mut app, hero, _) = holding();
    step(&mut app, 20);
    app.world_mut().get_mut::<Transform>(hero).unwrap().translation.z = 3.0;
    step(&mut app, 1);
    assert_eq!(progress(&app, hero), None);

    // Hurt to the ground, or killed.
    let (mut app, hero, _) = holding();
    step(&mut app, 20);
    assert!(go_down(app.world_mut(), hero, None, None) && app.world().get::<Downed>(hero).is_some());
    step(&mut app, 2);
    assert_eq!(progress(&app, hero), None);
    assert!(seen(&app).is_empty());
}

#[test]
fn the_thing_going_away_cancels_a_hold() {
    let (mut app, hero, valve) = holding();
    step(&mut app, 20);
    app.world_mut().despawn(valve);
    step(&mut app, 2);
    assert_eq!(progress(&app, hero), None);
    assert!(seen(&app).is_empty());
}

#[test]
fn a_tap_on_a_thing_that_takes_time_does_nothing() {
    let (mut app, hero) = arena();
    thing(&mut app, "valve", Usable::new("valve").held(1.0), 1.1);
    press(&mut app, hero, "interact");
    step(&mut app, 3);
    assert!(progress(&app, hero).is_none() && seen(&app).is_empty(), "no hold: it started and ended with the press");
}

// --- the revive keeps its press -------------------------------------------------------------

/// A mate of the hero's team 1 m east, down.
fn downed_mate(app: &mut App) -> Entity {
    let mate = fighter(app.world_mut(), "Mate", "l_hero", "heroes", Vec3::new(1.0, 0.0, 0.0), N);
    app.world_mut().entity_mut(mate).insert(PlayerId(1));
    step(app, 1);
    assert!(go_down(app.world_mut(), mate, None, None));
    step(app, 2);
    mate
}

#[test]
fn a_revive_in_reach_wins_the_press() {
    let (mut app, hero) = arena();
    let lever = thing(&mut app, "lever", Usable::default(), 1.1);
    let mate = downed_mate(&mut app);
    let out = ok(&mut app, "interact", json!({"hold": true}));
    assert!(out.contains("ok: l_revive"), "the tool answers by the rule the tick uses: {out}");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "l_revive");
    assert!(app.world().get::<shards_action::Pairing>(hero).is_some());
    assert!(seen(&app).is_empty(), "the lever is not used: the press was the revive's");
    // The hold of the revive goes on and the lever stays unused to the end.
    step(&mut app, 200);
    assert!(seen(&app).is_empty() && app.world().get::<Downed>(mate).is_none());
    // The ally is up: the next press finds nothing to revive and is a use again (the revive turned the hero to its ally: look back).
    hold(&mut app, hero, "interact", false);
    look(&mut app, hero, 0.0);
    step(&mut app, 2);
    press(&mut app, hero, "interact");
    step(&mut app, 2);
    assert_eq!(seen(&app), [(hero, lever, "lever".to_owned())]);
}

#[test]
fn without_an_ally_to_revive_the_press_is_a_use() {
    let (mut app, hero) = arena();
    let lever = thing(&mut app, "lever", Usable::default(), 1.1);
    let out = ok(&mut app, "interact", json!({}));
    assert!(out.starts_with("ok: using ") && out.contains("\"lever\" 1.0m"), "{out}");
    step(&mut app, 2);
    assert_eq!(seen(&app), [(hero, lever, "lever".to_owned())]);
    assert_eq!(action(&app, hero), "idle", "no revive started");
    // The mate went down after the use was seen to: now the revive wins.
    let _mate = downed_mate(&mut app);
    let out = ok(&mut app, "interact", json!({}));
    // With a target named, the tool says the revive has the button instead of promising a use.
    let lever_id = id(&app, lever);
    let err = fails(&mut app, "interact", json!({"target": lever_id}));
    assert!(err.contains("a revive is in reach and takes the button"), "{err}");
    assert!(out.contains("l_revive"), "{out}");
}

// --- the tool --------------------------------------------------------------------------------

#[test]
fn the_interact_tool_uses_what_you_look_at_and_says_what_it_needs() {
    let (mut app, hero) = plain();
    let valve = thing(&mut app, "valve", Usable::new("valve").held(1.0), 1.1);
    // A thing that takes time must be held.
    let err = fails(&mut app, "interact", json!({}));
    assert!(err.contains("\"valve\" takes 1.0s to use: hold the button, `interact hold=true`, step the time, then `interact hold=false`"), "{err}");
    let out = ok(&mut app, "interact", json!({"hold": true}));
    assert!(out.starts_with("ok: holding ") && out.contains("\"valve\" 1.0m for 1.0s"), "{out}");
    step(&mut app, 70);
    assert_eq!(seen(&app), [(hero, valve, "valve".to_owned())]);
    let out = ok(&mut app, "interact", json!({"hold": false}));
    assert!(out.contains("released"), "{out}");
    // Nothing in reach, and a moveset that binds nothing to the button: the reason, not a complaint about the moveset.
    app.world_mut().despawn(valve);
    step(&mut app, 1);
    let err = fails(&mut app, "interact", json!({}));
    assert!(err.starts_with("nothing to use: you look at nothing usable within 2.0m"), "{err}");
    thing(&mut app, "far", Usable::default(), 3.5);
    let err = fails(&mut app, "interact", json!({}));
    assert!(err.starts_with("nothing to use: ") && err.contains("\"far\" is 3.4m away and can be used from 2.0m"), "{err}");
}

#[test]
fn interact_with_a_target_turns_the_look_to_it_first() {
    let (mut app, hero) = plain();
    // East of the hero and a little above: nothing in front of it.
    let panel = thing_at(&mut app, "panel", Usable::default(), Vec3::new(-1.2, 1.9, -0.2));
    let target = id(&app, panel);
    let out = ok(&mut app, "interact", json!({"target": target}));
    assert!(out.starts_with("ok: using ") && out.contains("\"panel\""), "{out}");
    let looked = app.world().get::<Intent>(hero).unwrap().look.expect("the tool turned the look");
    assert!(looked.yaw > 1.0 && looked.pitch > 0.0, "west and up: {looked:?}");
    step(&mut app, 2);
    assert_eq!(seen(&app), [(hero, panel, "panel".to_owned())]);
}

#[test]
fn interact_with_a_target_says_why_it_cannot_and_changes_nothing() {
    let (mut app, hero) = plain();
    look(&mut app, hero, 0.0);
    let before = app.world().get::<Intent>(hero).unwrap().look;
    let far = thing(&mut app, "far", Usable::default(), 5.0);
    let far_id = id(&app, far);
    let err = fails(&mut app, "interact", json!({"target": far_id}));
    assert!(err.contains("\"far\" is 4.9m away and can be used from 2.0m: walk closer"), "{err}");
    // Behind a wall: a pane of glass is one.
    let glass = app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(2.0, 2.0, 0.02))), Transform::from_xyz(0.0, 1.62, -0.9))).id();
    let behind = thing(&mut app, "behind", Usable::default(), 1.6);
    let behind_id = id(&app, behind);
    let err = fails(&mut app, "interact", json!({"target": behind_id}));
    assert!(err.contains("\"behind\" is behind wall 0.9m ahead: a wall or glass blocks a use"), "{err}");
    app.world_mut().despawn(glass);
    // A nearer thing on the same line wins, and the tool says so.
    let near = thing(&mut app, "near", Usable::default(), 1.0);
    let near_id = id(&app, near);
    let err = fails(&mut app, "interact", json!({"target": behind_id}));
    assert!(err.contains(&format!("{near_id} \"near\" is nearer on the same line")), "{err}");
    // Yourself, something that is nothing to use, and a release that names a target.
    let hero_id = id(&app, hero);
    let err = fails(&mut app, "interact", json!({"target": hero_id}));
    assert!(err.contains("that is you"), "{err}");
    let rock = app.world_mut().spawn((AiKind::new("Rock"), Transform::default())).id();
    let rock_id = id(&app, rock);
    let err = fails(&mut app, "interact", json!({"target": rock_id}));
    assert!(err.contains("is nothing you can use"), "{err}");
    let err = fails(&mut app, "interact", json!({"target": near_id, "hold": false}));
    assert!(err.contains("hold=false lets go of the button and takes no target"), "{err}");
    assert_eq!(app.world().get::<Intent>(hero).unwrap().look, before, "an error leaves the look where it was");
}

#[test]
fn a_bare_marker_has_no_use_area_and_the_tool_says_so() {
    let (mut app, hero) = plain();
    let _ = hero;
    let bare = app.world_mut().spawn((Usable::new("lever"), Transform::from_xyz(0.0, 1.0, -1.0))).id();
    step(&mut app, 1);
    let bare_id = id(&app, bare);
    let err = fails(&mut app, "interact", json!({"target": bare_id}));
    assert!(err.contains("has no use area, so no look can meet it"), "{err}");
}
