//! Motion modes on the humanoid's bones: the props a swimmer carries, the blade a swing afloat keeps
//! in the hand, the draw after leaving the water, a layer that is off in the water, and the model
//! lowered by a swing afloat. A real pool: a water volume the character controller swims in.

use shards::action::ModeLeft;
use shards::motion::{Collider, CollisionLayers, MotionMode, MotionState, Shape, Volume, VolumeKind};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::ok;

use crate::fixtures::{KIT, KIT_PROPS};
use crate::headless::{bone_entity, progress};
use crate::sockets::{bound, drawn, hangs_from, kit_app_with};

const HUMANOID: &str = "models/humanoid.glb";
const POOL: Vec3 = Vec3::new(100.0, -1.1, 0.0);

#[derive(Resource, Default)]
struct Lefts(Vec<ModeLeft>);

/// The kit with modes: in the water the sword goes to the back and the flask to the hip (a swing
/// lets the blade stay in the hand for 0.3 s), after leaving the sword stays on the back for 0.4 s
/// (or until the body's fade out of its swimming clip is done) and then for 0.1 s more; the layer
/// `upper` is off in the water; a swing afloat lowers the model by 0.15 m over 0.2 s. The swing shows
/// the `idle` clip, so the root bone does not move by the clip.
fn modes_kit(idle: &str) -> String {
    KIT.replace(r#"layers: {"upper": (root: "spine")},"#, r#"layers: {"upper": (root: "spine", off_in: ["water"])},"#)
        .replace(r#"locomotion: {"idle": "idle", "walk": "walk"},"#, r#"locomotion: {"idle": "idle", "walk": "walk", "swim": "walk"},"#)
        .replace(r#"clips: {"idle": (),"#, &format!(r#"clips: {{"idle": ({idle}),"#))
        .replace(r#""light_1": "light_1""#, r#""light_1": "idle""#)
        .replace(
            r#"carry: {"hero": [(prop: "sword", socket: "hand_r")],"#,
            r#"carry: {"hero": [(prop: "sword", socket: "hand_r"), (prop: "flask", socket: "hand_r")],"#,
        )
        .replace(
            "    mounts: {\n",
            r#"    drops: {"water": (down: 0.15, seconds: 0.2)},
    mounts: {
        "@mode:water": (
            moves: [(prop: "sword", to: Some("back"), span: Secs(0.0, 3600.0)), (prop: "flask", to: Some("hip"), span: Secs(0.0, 3600.0))],
            yield_after: 0.3,
        ),
        "@left:water": (moves: [(prop: "sword", to: Some("back"), span: Secs(0.0, 0.1))], wait: 0.4),
"#,
        )
}

fn props() -> String {
    format!("{}    \"flask\": (size: (0.06, 0.1, 0.06)),\n}}", KIT_PROPS.trim_end().trim_end_matches('}'))
}

/// The hero on the floor with a pool of 5 m deep water at x 100; every `ModeLeft` is kept.
fn pool_app(name: &str, idle: &str) -> (App, Entity) {
    let (mut app, hero) = kit_app_with(name, &modes_kit(idle), &props(), HUMANOID);
    let world = app.world_mut();
    world.spawn((Collider::new(Shape::cuboid(Vec3::new(20.0, 0.5, 20.0))), Transform::from_xyz(100.0, -5.5, 0.0)));
    world.spawn((
        Collider::new(Shape::cuboid(Vec3::new(20.0, 2.5, 20.0))).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_xyz(100.0, -2.5, 0.0),
    ));
    app.init_resource::<Lefts>();
    app.add_observer(|left: On<ModeLeft>, mut lefts: ResMut<Lefts>| lefts.0.push(left.event().clone()));
    (app, hero)
}

/// `ticks` ticks with a frame after each, so the bridge sees every moment of them as it does in a
/// game (the paused test app snaps fades, but its clocks run by the tick).
fn run(app: &mut App, ticks: u32) {
    for _ in 0..ticks {
        ok(app, "step", json!({"ticks": 1}));
        app.update();
    }
    app.update();
}

fn mode(app: &App, hero: Entity) -> MotionMode {
    app.world().get::<MotionState>(hero).unwrap().mode
}

fn put(app: &mut App, hero: Entity, at: Vec3) {
    app.world_mut().get_mut::<Transform>(hero).unwrap().translation = at;
}

/// The hero into the pool, until it swims.
fn swim(app: &mut App, hero: Entity) {
    put(app, hero, POOL);
    run(app, 8);
    assert_eq!(mode(app, hero), MotionMode::Water, "the controller swims in the pool");
    assert_eq!(bound(app, hero).mode(), Some(MotionMode::Water), "and the bridge draws it so");
}

/// The hero back on the floor.
fn leave(app: &mut App, hero: Entity) {
    put(app, hero, Vec3::ZERO);
    run(app, 3);
    assert_ne!(mode(app, hero), MotionMode::Water);
}

fn lefts(app: &App) -> usize {
    app.world().resource::<Lefts>().0.len()
}

fn press(app: &mut App, hero: Entity, input: &str) {
    app.world_mut().get_mut::<Intent>(hero).unwrap().press(input);
}

#[test]
fn afloat_the_blade_rides_the_back_and_the_flask_the_hip() {
    let (mut app, hero) = pool_app("modes_afloat", "");
    let sword = drawn(&mut app, hero, "sword");
    let flask = drawn(&mut app, hero, "flask");
    assert_eq!((hangs_from(&app, sword), hangs_from(&app, flask)), ("hand.R".to_owned(), "hand.R".to_owned()));
    swim(&mut app, hero);
    assert_eq!(hangs_from(&app, sword), "spine", "the blade on the back");
    assert_eq!(hangs_from(&app, flask), "hips", "the flask at the hip");
    assert_eq!((bound(&app, hero).prop("sword"), bound(&app, hero).prop("flask")), (Some(sword), Some(flask)), "moved, not spawned again");
    // A weapon switch afloat opens no equip: the mode's rules took over.
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    run(&mut app, 2);
    assert!(!bound(&app, hero).equipping());
    let great = drawn(&mut app, hero, "great");
    assert_eq!(hangs_from(&app, great), "hand.R", "no rule of the mode names the new weapon: it sits in its carry socket");
}

#[test]
fn a_swing_afloat_keeps_the_blade_in_hand_for_the_linger() {
    let (mut app, hero) = pool_app("modes_linger", "");
    let sword = drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    assert_eq!(hangs_from(&app, sword), "spine");
    press(&mut app, hero, "light");
    run(&mut app, 3);
    assert!(progress(&app, hero).is_some(), "the swing runs afloat");
    assert_eq!(hangs_from(&app, sword), "hand.R", "the blade in the hand for the swing");
    run(&mut app, 40); // 0.72 s: the swing (0.6 s) is over, the linger (0.3 s) is not
    assert!(progress(&app, hero).is_none());
    assert_eq!(hangs_from(&app, sword), "hand.R", "and for the linger after it");
    run(&mut app, 25); // 1.15 s: the linger ended at 0.9 s
    assert_eq!(hangs_from(&app, sword), "spine", "then the mode's rules put it on the back again");
}

#[test]
fn leaving_the_water_draws_after_the_wait() {
    // The swimming clip fades out over 0.6 s, longer than the wait: the wait (0.4 s) ends it.
    let (mut app, hero) = pool_app("modes_left_wait", "blend: 0.6");
    let sword = drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    leave(&mut app, hero); // 0.05 s
    assert!(bound(&app, hero).left_pending());
    assert_eq!(hangs_from(&app, sword), "spine", "waiting: the blade stays where the water had it");
    run(&mut app, 15); // 0.3 s
    assert_eq!((lefts(&app), hangs_from(&app, sword).as_str()), (0, "spine"), "still waiting, no event");
    run(&mut app, 10); // 0.47 s: the wait ended at about 0.42 s, the draw's move (0.1 s) runs
    assert_eq!(lefts(&app), 1);
    assert_eq!(app.world().resource::<Lefts>().0[0].mode, "water");
    assert_eq!(app.world().resource::<Lefts>().0[0].entity, hero);
    assert_eq!(app.world().resource::<Lefts>().0[0].name(), "left:water");
    assert_eq!(hangs_from(&app, sword), "spine");
    run(&mut app, 20); // 0.8 s: drawn
    assert_eq!(hangs_from(&app, sword), "hand.R", "the blade is back in the hand");
    assert!(!bound(&app, hero).left_pending());
    run(&mut app, 30);
    assert_eq!(lefts(&app), 1, "the event fires once");
}

#[test]
fn the_wait_ends_with_the_fade_out_of_the_swimming_clip() {
    // The default blend (0.2 s): the draw starts when the fade is done, long before the wait.
    let (mut app, hero) = pool_app("modes_left_fade", "");
    let sword = drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    leave(&mut app, hero); // 0.05 s
    assert_eq!(lefts(&app), 0);
    run(&mut app, 13); // 0.27 s
    assert_eq!(lefts(&app), 1, "the fade out of the swimming clip is done");
    assert_eq!(hangs_from(&app, sword), "spine", "the draw's move is under way");
    run(&mut app, 20);
    assert_eq!(hangs_from(&app, sword), "hand.R");
}

#[test]
fn leaving_while_the_blade_lingers_draws_nothing() {
    let (mut app, hero) = pool_app("modes_left_linger", "");
    let sword = drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    press(&mut app, hero, "light");
    run(&mut app, 43); // the swing is over, the linger is not
    assert_eq!(hangs_from(&app, sword), "hand.R");
    leave(&mut app, hero);
    assert!(!bound(&app, hero).left_pending(), "the blade is in the hand already: nothing to draw");
    run(&mut app, 40);
    assert_eq!((lefts(&app), hangs_from(&app, sword).as_str()), (0, "hand.R"), "no event either");
}

#[test]
fn an_action_after_leaving_ends_the_wait() {
    let (mut app, hero) = pool_app("modes_left_action", "blend: 0.6");
    let sword = drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    leave(&mut app, hero);
    assert_eq!(hangs_from(&app, sword), "spine");
    press(&mut app, hero, "light");
    run(&mut app, 3);
    assert!(progress(&app, hero).is_some());
    assert!(!bound(&app, hero).left_pending());
    assert_eq!(hangs_from(&app, sword), "hand.R", "a whole-body action ended the wait: the blade is in the hand");
    run(&mut app, 40);
    assert_eq!(lefts(&app), 0, "and nothing was drawn");
}

#[test]
fn a_layer_off_in_water_shows_nothing_while_swimming() {
    let (mut app, hero) = pool_app("modes_layer", "");
    drawn(&mut app, hero, "sword");
    // On the ground the drink plays on the upper layer.
    press(&mut app, hero, "drink");
    run(&mut app, 6);
    assert_eq!(bound(&app, hero).layer_clip("upper"), Some("drink"));
    assert!(bound(&app, hero).layer_share("upper") > 0.0);
    // Into the water in the middle of it: the play leaves (a paused test app snaps the fade).
    put(&mut app, hero, POOL);
    run(&mut app, 5);
    assert_eq!(mode(&app, hero), MotionMode::Water);
    assert!(progress(&app, hero).is_some(), "the drink goes on");
    run(&mut app, 25);
    assert_eq!(bound(&app, hero).layer_clip("upper"), None);
    assert_eq!(bound(&app, hero).layer_share("upper"), 0.0, "nothing plays on the layer: the strokes own the arms");
    // A drink that starts afloat shows nothing on the layer either.
    run(&mut app, 80);
    assert!(progress(&app, hero).is_none());
    press(&mut app, hero, "drink");
    run(&mut app, 8);
    assert!(progress(&app, hero).is_some());
    assert_eq!((bound(&app, hero).layer_clip("upper"), bound(&app, hero).layer_share("upper")), (None, 0.0));
    // Out of the water the layer is on again.
    run(&mut app, 80);
    leave(&mut app, hero);
    press(&mut app, hero, "drink");
    run(&mut app, 6);
    assert_eq!(bound(&app, hero).layer_clip("upper"), Some("drink"));
}

/// How far the root bone is below the body, in the world (m).
fn root_below_body(app: &mut App, hero: Entity) -> f32 {
    let root = bone_entity(app, hero, "root");
    let at = app.world().get::<GlobalTransform>(root).unwrap().translation();
    app.world().get::<Transform>(hero).unwrap().translation.y - at.y
}

#[test]
fn a_swing_afloat_lowers_the_model_and_raises_it_back() {
    let (mut app, hero) = pool_app("modes_drop", "");
    drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    run(&mut app, 4);
    let base = root_below_body(&mut app, hero);
    assert_eq!(bound(&app, hero).drop_depth(), 0.0);
    press(&mut app, hero, "light");
    run(&mut app, 6); // 0.1 s into the swing: halfway down (0.15 m over 0.2 s)
    let depth = bound(&app, hero).drop_depth();
    assert!(depth > 0.05 && depth < 0.1, "{depth}");
    let lowered = root_below_body(&mut app, hero) - base;
    assert!((lowered - depth).abs() < 2e-3, "the root bone is {lowered} m lower in the world, the drop is {depth}");
    run(&mut app, 12); // 0.3 s: all the way
    assert!((bound(&app, hero).drop_depth() - 0.15).abs() < 1e-5);
    // Frames between ticks do not add it again: the drop is a state, not a step.
    for _ in 0..5 {
        app.update();
    }
    let lowered = root_below_body(&mut app, hero) - base;
    assert!((lowered - 0.15).abs() < 2e-3, "{lowered}");
    // The swing ends at 0.6 s, the model comes back at the rate it went down by 0.8 s.
    run(&mut app, 40);
    assert!(progress(&app, hero).is_none());
    run(&mut app, 20);
    assert_eq!(bound(&app, hero).drop_depth(), 0.0);
    let back = root_below_body(&mut app, hero) - base;
    assert!(back.abs() < 2e-3, "the model is back: {back}");
    // On the ground a swing lowers nothing.
    leave(&mut app, hero);
    run(&mut app, 30);
    press(&mut app, hero, "light");
    run(&mut app, 12);
    assert!(progress(&app, hero).is_some());
    assert_eq!(bound(&app, hero).drop_depth(), 0.0);
}

#[test]
fn the_animation_tool_names_the_mode_and_what_hangs_on_it() {
    let (mut app, hero) = pool_app("modes_tool", "");
    drawn(&mut app, hero, "sword");
    swim(&mut app, hero);
    let id = app.world().get::<AiId>(hero).unwrap().0;
    let text = ok(&mut app, "animation", json!({"id": format!("e{id}")})).result.as_str().unwrap().to_owned();
    assert!(text.contains("mode: water (drawn)") && text.contains("@mode:water applies") && text.contains("layers off: upper"), "{text}");
    assert!(text.contains("drop 0.00/0.15 m"), "{text}");
    press(&mut app, hero, "light");
    run(&mut app, 8);
    let text = ok(&mut app, "animation", json!({"id": format!("e{id}")})).result.as_str().unwrap().to_owned();
    assert!(text.contains("@mode:water steps aside") && text.contains("sword at hand_r"), "{text}");
    assert!(!text.contains("drop 0.00/"), "{text}");
}
