//! A model follows the action machine on the headless engine.

use std::time::{Duration, Instant};

use shards::action::render::{AnimGraphs, AnimationBound};
use shards::action::{ActionState, Animated, AnimationCue};
use shards::bevy::animation::graph::AnimationGraphHandle;
use shards::bevy::animation::AnimationPlayer;
use shards::bevy::gltf::GltfAssetLabel;
use shards::bevy::world_serialization::WorldAssetRoot;
use shards::motion::{character, Collider, Shape};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::ok;

use crate::fixtures::assets;

#[derive(Resource, Default)]
struct Cues(Vec<AnimationCue>);

fn spawn_hero(app: &mut App, at: Vec3) -> Entity {
    let model = app.world().resource::<AssetServer>().load(GltfAssetLabel::Scene(0).from_asset("models/model.gltf"));
    let (controller, intent, transform) = character("", at);
    app.world_mut()
        .spawn((
            Name::new("hero"),
            AiKind::new("Hero"),
            Fighter::new("hero"),
            Health::new(100.0),
            Stamina::new(100.0, 30.0, 0.8),
            controller,
            intent,
            transform,
            Animated::new("humanoid"),
        ))
        .with_child(WorldAssetRoot(model))
        .id()
}

/// Updates until `done` holds.
fn update_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(30), "gave up waiting for: {what}");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The angle (degrees) the spine bone of `character` is turned about Z.
fn spine_angle(app: &mut App, character: Entity) -> f32 {
    let world = app.world_mut();
    let mut names = world.query::<(Entity, &Name)>();
    let spine = names
        .iter(world)
        .find(|(e, name)| name.as_str() == "Spine" && is_below(world, *e, character))
        .map(|(e, _)| e)
        .expect("the model has a Spine");
    world.get::<Transform>(spine).unwrap().rotation.to_euler(EulerRot::ZYX).0.to_degrees()
}

fn is_below(world: &World, mut entity: Entity, ancestor: Entity) -> bool {
    while let Some(parent) = world.get::<ChildOf>(entity) {
        entity = parent.parent();
        if entity == ancestor {
            return true;
        }
    }
    false
}

fn progress(app: &App, character: Entity) -> Option<f32> {
    let run = app.world().get::<ActionState>(character)?.current.as_ref()?;
    Some(run.t / run.length)
}

#[test]
fn a_model_follows_the_action_machine_through_clips_of_another_file() {
    let dir = assets("action_render_headless");
    let mut app = shards::testing::test_app(
        |config| config.asset_dir = dir.to_string_lossy().into_owned(),
        |app| {
            app.init_resource::<Cues>();
            app.add_observer(|cue: On<AnimationCue>, mut cues: ResMut<Cues>| cues.0.push(cue.event().clone()));
        },
    );
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(30.0, 0.5, 30.0))), Transform::from_xyz(0.0, -0.5, 0.0)));
    let hero = spawn_hero(&mut app, Vec3::ZERO);
    let guard = spawn_hero(&mut app, Vec3::new(3.0, 0.0, 0.0));

    // The model loads, the library loads, the bridge binds the player it gave the skeleton.
    update_until(&mut app, "both characters bound", |app| {
        app.world().get::<AnimationBound>(hero).is_some() && app.world().get::<AnimationBound>(guard).is_some()
    });
    let graphs = app.world().resource::<AnimGraphs>();
    let built = graphs.built("humanoid").expect("the set's graph");
    assert_eq!(built.clips(), ["idle", "slash"]);
    let player = app.world().get::<AnimationBound>(hero).unwrap().player;
    assert!(app.world().get::<AnimationPlayer>(player).is_some());
    let named = app.world().get::<Name>(player).map(|n| n.to_string());
    assert_eq!(named.as_deref(), Some("Armature"), "the player sits on the skeleton's root");
    let other = app.world().get::<AnimationBound>(guard).unwrap().player;
    assert_ne!(player, other);
    let (a, b) =
        (app.world().get::<AnimationGraphHandle>(player).unwrap(), app.world().get::<AnimationGraphHandle>(other).unwrap());
    assert_eq!(a.id(), b.id(), "one graph for both characters: the clips are shared");

    // Standing: the idle clip.
    update_until(&mut app, "idle", |app| app.world().get::<AnimationBound>(hero).unwrap().clip() == Some("idle"));
    let bone = spine_angle(&mut app, hero);
    assert!((0.0..=10.0).contains(&bone), "somewhere in the idle's 0..10 degrees: {bone}");

    // The hero swings. Gameplay leads: after N ticks the spine is where the machine says.
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("light");
    ok(&mut app, "step", json!({"ticks": 6}));
    app.update();
    app.update();
    let p = progress(&app, hero).expect("the swing is running");
    assert!(p > 0.1 && p < 0.9, "{p}");
    assert_eq!(app.world().get::<AnimationBound>(hero).unwrap().clip(), Some("slash"));
    let angle = spine_angle(&mut app, hero);
    assert!((angle - 90.0 * p).abs() < 1.5, "the clip is at the action's {p:.2}: {angle:.1} degrees");
    // The other character did not move with it.
    assert_eq!(app.world().get::<AnimationBound>(guard).unwrap().clip(), Some("idle"));

    ok(&mut app, "step", json!({"ticks": 12}));
    app.update();
    app.update();
    let p2 = progress(&app, hero).expect("still swinging");
    assert!(p2 > p + 0.25, "{p} -> {p2}");
    let angle = spine_angle(&mut app, hero);
    assert!((angle - 90.0 * p2).abs() < 1.5, "{angle:.1} degrees at {p2:.2}");
    let swoosh = |app: &App| app.world().resource::<Cues>().0.iter().filter(|c| c.cue == "swoosh").count();
    assert_eq!(swoosh(&app), usize::from(p2 >= 0.5), "the cue fires when the clip passes it (progress {p2:.2})");

    // The swing ends; so does the slash clip.
    ok(&mut app, "step", json!({"ticks": 30}));
    app.update();
    assert!(progress(&app, hero).is_none());
    assert_eq!(app.world().get::<AnimationBound>(hero).unwrap().clip(), Some("idle"));
    assert_eq!(swoosh(&app), 1, "once");
    // The simulation is paused, so Bevy's clock stands still and nothing loops: no idle cue.
    let heard = &app.world().resource::<Cues>().0;
    assert!(heard.iter().all(|c| c.entity == hero && c.cue == "swoosh"), "{heard:?}");
}
